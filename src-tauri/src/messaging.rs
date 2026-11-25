//! SMS/MMS through an Aria PBX: the messaging numbers (inboxes) an extension
//! reads and replies on, their conversations, one conversation's thread, and
//! sending. Sign-in and sessions are `pbx_api`'s; every route lives under the
//! tenant the login named.
//!
//! Only what the API offers: there is no read flag on a text, so "unread" is
//! the frontend's own bookkeeping, and nothing here deletes a message.

use crate::pbx_api::{error_from, net_error, route, Mailbox, PbxClient, PbxError};
use serde::{Deserialize, Serialize};

/// A messaging number the extension may use. `enabled: false` is a number a
/// tenant admin still reads but nobody sends from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Inbox {
    pub did_id: String,
    pub number: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub last_inbound_at: Option<String>,
}

fn yes() -> bool {
    true
}

/// One remote number's conversation on an inbox, as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Conversation {
    pub remote_number: String,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub last_body: String,
    /// `inbound` or `outbound`.
    #[serde(default)]
    pub last_direction: String,
    #[serde(default)]
    pub last_status: String,
    /// RFC 3339, as the server sent it.
    #[serde(default)]
    pub last_at: String,
    #[serde(default)]
    pub last_has_media: bool,
    #[serde(default)]
    pub opted_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Attachment {
    pub id: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub size_bytes: i64,
}

/// One text, either way, with its attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct SmsMessage {
    pub id: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub remote_number: String,
    #[serde(default)]
    pub body: String,
    /// `pending`, `sent`, `delivered`, `failed` or `received`.
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub auto_reply: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Thread {
    pub remote_number: String,
    #[serde(default)]
    pub opted_out: bool,
    /// Newest first, as the PBX orders them.
    #[serde(default)]
    pub messages: Vec<SmsMessage>,
}

/// A text to send: from which inbox, to whom, what, and the sender's own
/// idempotency key so a retried send is stored once.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outgoing {
    pub did_id: String,
    pub to: String,
    pub body: String,
    pub client_key: String,
}

impl PbxClient {
    /// The tenant the session's login named; messaging routes live under it.
    fn tenant_segment(session: &crate::pbx_api::Session) -> Result<&str, PbxError> {
        if session.tenant_id.is_empty() { Err(PbxError::Malformed) } else { Ok(&session.tenant_id) }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        mailbox: &Mailbox,
        password: &str,
        build: &(dyn Fn(&reqwest::Url, &crate::pbx_api::Session) -> Result<reqwest::Url, PbxError> + Send + Sync),
    ) -> Result<T, PbxError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| match build(api, session) {
                Ok(url) => http.get(url).bearer_auth(&session.jwt),
                // An unusable session cannot build a URL; the request fails
                // at once and the error is told below.
                Err(_) => http.get(route(api, &["api", "v1", "tenants", "", "messaging", "inboxes"])).bearer_auth(&session.jwt),
            })
            .await?;
        if !reply.status().is_success() {
            return Err(error_from(reply).await);
        }
        reply.json().await.map_err(|_| PbxError::Malformed)
    }

    /// The messaging numbers the extension reads and replies on.
    pub async fn inboxes(&self, mailbox: &Mailbox, password: &str) -> Result<Vec<Inbox>, PbxError> {
        self.get_json(mailbox, password, &|api, session| {
            let tenant = Self::tenant_segment(session)?;
            Ok(route(api, &["api", "v1", "tenants", tenant, "messaging", "inboxes"]))
        })
        .await
    }

    /// One inbox's conversations, newest first (at most 200).
    pub async fn conversations(&self, mailbox: &Mailbox, password: &str, did_id: &str) -> Result<Vec<Conversation>, PbxError> {
        self.get_json(mailbox, password, &|api, session| {
            let tenant = Self::tenant_segment(session)?;
            let mut url = route(api, &["api", "v1", "tenants", tenant, "messaging", "inboxes", did_id, "conversations"]);
            url.query_pairs_mut().append_pair("limit", "200");
            Ok(url)
        })
        .await
    }

    /// The thread with `number` on an inbox: the newest 100 messages, or the
    /// 100 before `before` (RFC 3339) when loading older ones.
    pub async fn thread(
        &self,
        mailbox: &Mailbox,
        password: &str,
        did_id: &str,
        number: &str,
        before: Option<&str>,
    ) -> Result<Thread, PbxError> {
        self.get_json(mailbox, password, &|api, session| {
            let tenant = Self::tenant_segment(session)?;
            let mut url = route(api, &["api", "v1", "tenants", tenant, "messaging", "inboxes", did_id, "conversations", number]);
            let mut query = url.query_pairs_mut();
            query.append_pair("limit", "100");
            if let Some(before) = before {
                query.append_pair("before", before);
            }
            drop(query);
            Ok(url)
        })
        .await
    }

    /// Send a text. A refusal (an opted-out recipient, a limit, a number that
    /// is not the extension's) comes back as `Rejected` with the PBX's words.
    pub async fn send_sms(&self, mailbox: &Mailbox, password: &str, out: &Outgoing) -> Result<SmsMessage, PbxError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                let tenant = if session.tenant_id.is_empty() { "" } else { session.tenant_id.as_str() };
                http.post(route(api, &["api", "v1", "tenants", tenant, "messaging", "inboxes", &out.did_id, "messages"]))
                    .bearer_auth(&session.jwt)
                    .header("Idempotency-Key", &out.client_key)
                    .json(&serde_json::json!({
                        "to": out.to,
                        "body": out.body,
                        "attachment_ids": [],
                    }))
            })
            .await?;
        if !reply.status().is_success() {
            return Err(error_from(reply).await);
        }
        reply.json().await.map_err(|_| PbxError::Malformed)
    }

    /// An attachment's bytes; the content type is the one the thread listed.
    pub async fn attachment(&self, mailbox: &Mailbox, password: &str, attachment_id: &str) -> Result<Vec<u8>, PbxError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                let tenant = if session.tenant_id.is_empty() { "" } else { session.tenant_id.as_str() };
                http.get(route(api, &["api", "v1", "tenants", tenant, "messaging", "attachments", attachment_id]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        if !reply.status().is_success() {
            return Err(error_from(reply).await);
        }
        reply.bytes().await.map(|b| b.to_vec()).map_err(net_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pbx_api::stub::{json, login, mailbox, test_password, Handler, Seen, Stub};
    use std::sync::Arc;

    const INBOXES: &str = r#"[
      {"did_id":"did-1","number":"+13035550100","last_inbound_at":"2026-11-24T10:00:00Z","enabled":true},
      {"did_id":"did-2","number":"+13035550101","last_inbound_at":null,"enabled":false}
    ]"#;

    const CONVERSATIONS: &str = r#"[
      {"remote_number":"+12125550123","message_count":3,"last_body":"See you at 4","last_direction":"inbound",
       "last_status":"received","last_at":"2026-11-24T10:00:00Z","last_has_media":false,"opted_out":false}
    ]"#;

    const THREAD: &str = r#"{"remote_number":"+12125550123","opted_out":false,"messages":[
      {"id":"s2","tenant_id":"t-1","did_id":"did-1","direction":"inbound","from_number":"+12125550123",
       "to_number":"+13035550100","remote_number":"+12125550123","body":"See you at 4","status":"received",
       "auto_reply":false,"error_message":null,"created_at":"2026-11-24T10:00:00Z","updated_at":"2026-11-24T10:00:00Z",
       "attachments":[{"id":"a1","content_type":"image/jpeg","size_bytes":123456}]},
      {"id":"s1","direction":"outbound","from_number":"+13035550100","to_number":"+12125550123",
       "remote_number":"+12125550123","body":"Running late","status":"delivered","auto_reply":false,
       "created_at":"2026-11-24T09:00:00Z","attachments":[]}
    ]}"#;

    const SENT: &str = r#"{"id":"s3","direction":"outbound","from_number":"+13035550100","to_number":"+12125550123",
      "remote_number":"+12125550123","body":"On my way","status":"sent","auto_reply":false,
      "created_at":"2026-11-24T11:00:00Z","attachments":[]}"#;

    /// A PBX with one tenant (`t-1`, from the login) and two messaging numbers.
    fn pbx(tokens: &'static [&'static str], password: String) -> Handler {
        let signin = login(tokens, password);
        Arc::new(move |req: &Seen| {
            if let Some(reply) = signin(req) {
                return reply;
            }
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/api/v1/tenants/t-1/messaging/inboxes") => json(200, INBOXES),
                ("GET", "/api/v1/tenants/t-1/messaging/inboxes/did-1/conversations?limit=200") => json(200, CONVERSATIONS),
                ("GET", p) if p.starts_with("/api/v1/tenants/t-1/messaging/inboxes/did-1/conversations/+12125550123?limit=100") => {
                    json(200, THREAD)
                }
                ("POST", "/api/v1/tenants/t-1/messaging/inboxes/did-1/messages") => {
                    let body: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
                    if body["to"] == "+12125550199" {
                        return json(422, r#"{"error":"+12125550199 has opted out of messages from this number.","status":422}"#);
                    }
                    json(201, SENT)
                }
                ("GET", "/api/v1/tenants/t-1/messaging/attachments/a1") => (200, "image/jpeg", b"\xFF\xD8jpeg".to_vec()),
                _ => json(404, r#"{"error":"not found","status":404}"#),
            }
        })
    }

    #[tokio::test]
    async fn lists_the_inboxes_of_the_tenant_the_login_named() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let inboxes = PbxClient::new().inboxes(&mailbox(&stub), &pass).await.unwrap();
        assert_eq!(inboxes.len(), 2);
        assert_eq!(inboxes[0].did_id, "did-1");
        assert_eq!(inboxes[0].number, "+13035550100");
        assert!(inboxes[0].enabled);
        assert_eq!(inboxes[0].last_inbound_at.as_deref(), Some("2026-11-24T10:00:00Z"));
        assert!(!inboxes[1].enabled);
        let seen = stub.seen();
        assert_eq!(seen[1].path, "/api/v1/tenants/t-1/messaging/inboxes");
        assert_eq!(seen[1].authorization.as_deref(), Some("Bearer jwt-1"));
    }

    #[tokio::test]
    async fn conversations_and_threads_put_the_number_in_one_segment() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let client = PbxClient::new();
        let conversations = client.conversations(&mailbox(&stub), &pass, "did-1").await.unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].remote_number, "+12125550123");
        assert_eq!(conversations[0].last_body, "See you at 4");
        assert_eq!(conversations[0].last_direction, "inbound");
        assert_eq!(conversations[0].message_count, 3);

        let thread = client.thread(&mailbox(&stub), &pass, "did-1", "+12125550123", None).await.unwrap();
        assert_eq!(thread.remote_number, "+12125550123");
        assert_eq!(thread.messages.len(), 2);
        assert_eq!(thread.messages[0].id, "s2");
        assert_eq!(thread.messages[0].direction, "inbound");
        assert_eq!(thread.messages[0].attachments[0].content_type, "image/jpeg");
        assert_eq!(thread.messages[1].status, "delivered");
        assert!(thread.messages[1].attachments.is_empty());

        // Older messages: the page before a timestamp.
        client.thread(&mailbox(&stub), &pass, "did-1", "+12125550123", Some("2026-11-24T09:00:00Z")).await.unwrap();
        let seen = stub.seen();
        assert_eq!(seen[1].path, "/api/v1/tenants/t-1/messaging/inboxes/did-1/conversations?limit=200");
        assert_eq!(seen[2].path, "/api/v1/tenants/t-1/messaging/inboxes/did-1/conversations/+12125550123?limit=100");
        assert_eq!(
            seen[3].path,
            "/api/v1/tenants/t-1/messaging/inboxes/did-1/conversations/+12125550123?limit=100&before=2026-11-24T09%3A00%3A00Z"
        );
        assert_eq!(seen.len(), 4, "one login, three calls: {seen:?}");
    }

    #[tokio::test]
    async fn sending_posts_the_text_with_the_idempotency_key() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let out = Outgoing { did_id: "did-1".into(), to: "+12125550123".into(), body: "On my way".into(), client_key: "k-1".into() };
        let sent = PbxClient::new().send_sms(&mailbox(&stub), &pass, &out).await.unwrap();
        assert_eq!(sent.id, "s3");
        assert_eq!(sent.status, "sent");
        let last = stub.seen().pop().unwrap();
        assert_eq!(last.method, "POST");
        assert_eq!(last.path, "/api/v1/tenants/t-1/messaging/inboxes/did-1/messages");
        assert_eq!(last.idempotency_key.as_deref(), Some("k-1"));
        let body: serde_json::Value = serde_json::from_str(&last.body).unwrap();
        assert_eq!(body["to"], "+12125550123");
        assert_eq!(body["body"], "On my way");
        assert_eq!(body["attachment_ids"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_refused_send_carries_the_reason_the_pbx_gave() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let out = Outgoing { did_id: "did-1".into(), to: "+12125550199".into(), body: "Hi".into(), client_key: "k-2".into() };
        let err = PbxClient::new().send_sms(&mailbox(&stub), &pass, &out).await.unwrap_err();
        match err {
            PbxError::Rejected(reason) => assert_eq!(reason, "+12125550199 has opted out of messages from this number."),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn an_attachment_is_fetched_with_the_bearer_token() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let bytes = PbxClient::new().attachment(&mailbox(&stub), &pass, "a1").await.unwrap();
        assert_eq!(bytes, b"\xFF\xD8jpeg");
        let last = stub.seen().pop().unwrap();
        assert_eq!(last.path, "/api/v1/tenants/t-1/messaging/attachments/a1");
        assert_eq!(last.authorization.as_deref(), Some("Bearer jwt-1"));
    }

    #[tokio::test]
    async fn a_number_nobody_may_use_is_rejected_with_the_pbx_words() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let err = PbxClient::new().conversations(&mailbox(&stub), &pass, "did-9").await.unwrap_err();
        assert!(matches!(err, PbxError::Rejected(ref r) if r == "not found"), "{err:?}");
    }

    /// Against a real Aria PBX, on demand, with the same variables as the
    /// voicemail live test. Lists the extension's inboxes and, for each, its
    /// conversations and the newest thread; sends nothing.
    #[tokio::test]
    #[ignore = "needs a live Aria PBX; see ARIA_E2E_PBX_API in voicemail.rs"]
    async fn lists_the_inboxes_of_a_live_pbx() {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let (Some(api), Some(domain), Some(ext), Some(pass)) =
            (var("ARIA_E2E_PBX_API"), var("ARIA_E2E_PBX_DOMAIN"), var("ARIA_E2E_EXT_A"), var("ARIA_E2E_PASS_A"))
        else {
            eprintln!("messaging e2e: ARIA_E2E_PBX_API, ARIA_E2E_PBX_DOMAIN, ARIA_E2E_EXT_A, ARIA_E2E_PASS_A not all set");
            return;
        };
        let mailbox = Mailbox { account_id: "e2e".into(), api_url: api, extension: ext, domain };
        let client = PbxClient::new();
        let inboxes = client.inboxes(&mailbox, &pass).await.expect("inboxes listed");
        eprintln!("messaging e2e: {} inbox(es)", inboxes.len());
        for inbox in &inboxes {
            let conversations = client.conversations(&mailbox, &pass, &inbox.did_id).await.expect("conversations");
            eprintln!("  {} enabled={} {} conversation(s)", inbox.number, inbox.enabled, conversations.len());
            if let Some(c) = conversations.first() {
                let thread = client.thread(&mailbox, &pass, &inbox.did_id, &c.remote_number, None).await.expect("thread");
                eprintln!("    newest with {}: {} message(s)", c.remote_number, thread.messages.len());
            }
        }
    }
}
