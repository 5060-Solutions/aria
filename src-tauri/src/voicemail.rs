//! Visual voicemail against an Aria PBX: find the extension's mailbox, then
//! list, play, mark read and delete its messages. Sign-in and sessions are
//! `pbx_api`'s.

use crate::pbx_api::{net_error, route, status_error, Mailbox, PbxClient, PbxError, Session};
use serde::{Deserialize, Serialize};

/// One message as the PBX lists it, serialised for the frontend in camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub caller_id: String,
    #[serde(default)]
    pub duration_secs: u32,
    #[serde(default)]
    pub read: bool,
    /// RFC 3339, as the server sent it.
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub transcription: Option<String>,
}

#[derive(Deserialize)]
struct BoxReply {
    id: String,
}

impl PbxClient {
    /// The id of the extension's mailbox, found once per session.
    async fn box_id(&self, mailbox: &Mailbox, password: &str) -> Result<String, PbxError> {
        if let Some(Session { box_id: Some(id), .. }) = self.cached(&mailbox.account_id) {
            return Ok(id);
        }
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                http.get(route(api, &["api", "v1", "extensions", &session.extension_id, "voicemail"]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        match reply.status() {
            s if s.is_success() => {}
            reqwest::StatusCode::NOT_FOUND => return Err(PbxError::NoMailbox),
            s => return Err(status_error(s)),
        }
        let vbox: BoxReply = reply.json().await.map_err(|_| PbxError::Malformed)?;
        let id = vbox.id.clone();
        self.update_session(&mailbox.account_id, |session| session.box_id = Some(id));
        Ok(vbox.id)
    }

    /// The mailbox's messages as the PBX orders them (newest first).
    pub async fn list(&self, mailbox: &Mailbox, password: &str) -> Result<Vec<Message>, PbxError> {
        let box_id = self.box_id(mailbox, password).await?;
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                let mut url = route(api, &["api", "v1", "voicemail", "boxes", &box_id, "messages"]);
                url.query_pairs_mut().append_pair("limit", "100");
                http.get(url).bearer_auth(&session.jwt)
            })
            .await?;
        if !reply.status().is_success() {
            return Err(status_error(reply.status()));
        }
        reply.json().await.map_err(|_| PbxError::Malformed)
    }

    pub async fn mark_read(&self, mailbox: &Mailbox, password: &str, message_id: &str) -> Result<(), PbxError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                http.patch(route(api, &["api", "v1", "voicemail", "messages", message_id, "read"]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        if reply.status().is_success() { Ok(()) } else { Err(status_error(reply.status())) }
    }

    pub async fn delete(&self, mailbox: &Mailbox, password: &str, message_id: &str) -> Result<(), PbxError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                http.delete(route(api, &["api", "v1", "voicemail", "messages", message_id]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        if reply.status().is_success() { Ok(()) } else { Err(status_error(reply.status())) }
    }

    /// The message's audio as the PBX stores it (WAV).
    pub async fn audio(&self, mailbox: &Mailbox, password: &str, message_id: &str) -> Result<Vec<u8>, PbxError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                http.get(route(api, &["api", "v1", "voicemail", "messages", message_id, "audio"]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        if !reply.status().is_success() {
            return Err(status_error(reply.status()));
        }
        reply.bytes().await.map(|b| b.to_vec()).map_err(net_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbx_api::stub::{json, login, mailbox, test_password, Handler, Seen, Stub};
    use std::sync::Arc;

    const MESSAGES: &str = r#"[
      {"id":"m2","box_id":"b1","caller_id":"+15551230002","duration_secs":42,"read":false,
       "created_at":"2026-11-24T10:00:00Z","transcription":"Call me back about the invoice."},
      {"id":"m1","box_id":"b1","caller_id":"1002","duration_secs":5,"read":true,
       "created_at":"2026-11-23T09:00:00Z","transcription":null}
    ]"#;

    /// The PBX as the mobile apps see it: `login` plus the voicemail routes.
    fn pbx(tokens: &'static [&'static str], password: String) -> Handler {
        let signin = login(tokens, password);
        Arc::new(move |req: &Seen| {
            if let Some(reply) = signin(req) {
                return reply;
            }
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/api/v1/extensions/ext-1/voicemail") => json(200, r#"{"id":"b1","extension_id":"ext-1"}"#),
                ("GET", "/api/v1/voicemail/boxes/b1/messages?limit=100") => json(200, MESSAGES),
                ("PATCH", "/api/v1/voicemail/messages/m2/read") => json(200, r#"{"id":"m2","read":true}"#),
                ("DELETE", "/api/v1/voicemail/messages/m2") => (204, "text/plain", Vec::new()),
                ("GET", "/api/v1/voicemail/messages/m2/audio") => (200, "audio/wav", b"RIFFwav".to_vec()),
                _ => json(404, r#"{"error":"not found"}"#),
            }
        })
    }

    #[tokio::test]
    async fn lists_messages_after_logging_in_with_the_sip_credentials() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = PbxClient::new();
        let messages = state.list(&mailbox(&stub), &pass).await.unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "m2");
        assert_eq!(messages[0].caller_id, "+15551230002");
        assert_eq!(messages[0].duration_secs, 42);
        assert!(!messages[0].read);
        assert_eq!(messages[0].created_at, "2026-11-24T10:00:00Z");
        assert_eq!(messages[0].transcription.as_deref(), Some("Call me back about the invoice."));
        assert_eq!(messages[1].transcription, None);

        let seen = stub.seen();
        assert_eq!(seen[0].method, "POST");
        assert_eq!(seen[0].path, "/api/v1/auth/extension-login");
        let login: serde_json::Value = serde_json::from_str(&seen[0].body).unwrap();
        assert_eq!(login["extension_number"], "1001");
        assert_eq!(login["password"], pass.as_str());
        assert_eq!(login["tenant_domain"], "test.ariaroute.com");
        assert!(seen[1..].iter().all(|r| r.authorization.as_deref() == Some("Bearer jwt-1")), "{seen:?}");
        assert_eq!(seen[1].path, "/api/v1/extensions/ext-1/voicemail");
        assert_eq!(seen[2].path, "/api/v1/voicemail/boxes/b1/messages?limit=100");
    }

    #[tokio::test]
    async fn a_second_list_reuses_the_session_and_the_box() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = PbxClient::new();
        state.list(&mailbox(&stub), &pass).await.unwrap();
        state.list(&mailbox(&stub), &pass).await.unwrap();
        let seen = stub.seen();
        assert_eq!(seen.iter().filter(|r| r.path == "/api/v1/auth/extension-login").count(), 1, "{seen:?}");
        assert_eq!(seen.iter().filter(|r| r.path == "/api/v1/extensions/ext-1/voicemail").count(), 1, "{seen:?}");
        assert_eq!(seen.len(), 4);
    }

    #[tokio::test]
    async fn an_expired_token_logs_in_again_once_and_retries() {
        let pass = test_password();
        // The first login's token is already stale when it is used.
        let stub = Stub::start(pbx(&["jwt-1", "jwt-2"], pass.clone())).await;
        let state = PbxClient::new();
        let messages = state.list(&mailbox(&stub), &pass).await.unwrap();
        assert_eq!(messages.len(), 2);
        let seen = stub.seen();
        assert_eq!(seen.iter().filter(|r| r.path == "/api/v1/auth/extension-login").count(), 2, "{seen:?}");
        assert_eq!(seen.last().unwrap().authorization.as_deref(), Some("Bearer jwt-2"));
    }

    #[tokio::test]
    async fn a_wrong_password_is_refused_without_a_retry_loop() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = PbxClient::new();
        let err = state.list(&mailbox(&stub), &format!("not-{pass}")).await.unwrap_err();
        assert!(matches!(err, PbxError::Unauthorized), "{err:?}");
        assert_eq!(stub.seen().len(), 1);
    }

    #[tokio::test]
    async fn an_extension_without_a_mailbox_is_said_so() {
        let pass = test_password();
        let inner = pbx(&["jwt-1"], pass.clone());
        let handler: Handler = Arc::new(move |req: &Seen| {
            if req.path == "/api/v1/extensions/ext-1/voicemail" && req.authorization.is_some() {
                return json(404, r#"{"error":"no voicemail box"}"#);
            }
            inner(req)
        });
        let stub = Stub::start(handler).await;
        let err = PbxClient::new().list(&mailbox(&stub), &pass).await.unwrap_err();
        assert!(matches!(err, PbxError::NoMailbox), "{err:?}");
    }

    #[tokio::test]
    async fn audio_is_fetched_with_the_bearer_token() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let bytes = PbxClient::new().audio(&mailbox(&stub), &pass, "m2").await.unwrap();
        assert_eq!(bytes, b"RIFFwav");
        let last = stub.seen().pop().unwrap();
        assert_eq!(last.path, "/api/v1/voicemail/messages/m2/audio");
        assert_eq!(last.authorization.as_deref(), Some("Bearer jwt-1"));
    }

    #[tokio::test]
    async fn mark_read_and_delete_use_their_routes() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = PbxClient::new();
        state.mark_read(&mailbox(&stub), &pass, "m2").await.unwrap();
        state.delete(&mailbox(&stub), &pass, "m2").await.unwrap();
        let seen = stub.seen();
        assert!(seen.iter().any(|r| r.method == "PATCH" && r.path == "/api/v1/voicemail/messages/m2/read"), "{seen:?}");
        assert!(seen.iter().any(|r| r.method == "DELETE" && r.path == "/api/v1/voicemail/messages/m2"), "{seen:?}");
    }

    #[tokio::test]
    async fn a_message_id_is_a_path_segment_not_a_path() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let err = PbxClient::new().delete(&mailbox(&stub), &pass, "m2/read?x=../..").await.unwrap_err();
        assert!(matches!(err, PbxError::Status(404)), "{err:?}");
        let last = stub.seen().pop().unwrap();
        assert_eq!(last.method, "DELETE");
        assert_eq!(last.path, "/api/v1/voicemail/messages/m2%2Fread%3Fx=..%2F..");
    }

    /// Against a real Aria PBX, on demand: `ARIA_E2E_PBX_API` (e.g.
    /// https://api.ariaroute.com), `ARIA_E2E_PBX_DOMAIN`, `ARIA_E2E_EXT_A`
    /// and `ARIA_E2E_PASS_A`, the same extension `scripts/pbx-e2e.sh` uses.
    /// Proves sign-in, mailbox lookup and the message list end to end; prints
    /// the messages so a human can compare with the PBX admin.
    #[tokio::test]
    #[ignore = "needs a live Aria PBX; see ARIA_E2E_PBX_API in the doc comment"]
    async fn lists_the_mailbox_of_a_live_pbx() {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (Some(api), Some(domain), Some(ext), Some(pass)) =
            (var("ARIA_E2E_PBX_API"), var("ARIA_E2E_PBX_DOMAIN"), var("ARIA_E2E_EXT_A"), var("ARIA_E2E_PASS_A"))
        else {
            eprintln!("voicemail e2e: ARIA_E2E_PBX_API, ARIA_E2E_PBX_DOMAIN, ARIA_E2E_EXT_A, ARIA_E2E_PASS_A not all set");
            return;
        };
        let mailbox = Mailbox { account_id: "e2e".into(), api_url: api, extension: ext, domain };
        let state = PbxClient::new();
        let messages = state.list(&mailbox, &pass).await.expect("mailbox listed");
        eprintln!("voicemail e2e: {} message(s)", messages.len());
        for m in &messages {
            eprintln!(
                "  {} from {} {}s read={} at {} transcription={}",
                m.id, m.caller_id, m.duration_secs, m.read, m.created_at, m.transcription.is_some()
            );
            // Audio must come back for every listed message.
            let audio = state.audio(&mailbox, &pass, &m.id).await.expect("audio");
            assert!(audio.len() > 44, "{} bytes of audio for {}", audio.len(), m.id);
        }
        // Marking the newest unread message read sticks on the PBX.
        if let Some(unread) = messages.iter().find(|m| !m.read) {
            state.mark_read(&mailbox, &pass, &unread.id).await.expect("mark read");
            let again = state.list(&mailbox, &pass).await.expect("second list");
            let now = again.iter().find(|m| m.id == unread.id).expect("message still listed");
            assert!(now.read, "{} should be read after mark_read", unread.id);
            eprintln!("voicemail e2e: marked {} read", unread.id);
        } else {
            // The second call must not sign in again.
            state.list(&mailbox, &pass).await.expect("second list");
        }
    }
}
