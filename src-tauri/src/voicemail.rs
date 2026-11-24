//! Visual voicemail against an Aria PBX.
//!
//! The same REST surface the mobile apps use: sign in with the extension's SIP
//! credentials (`POST /api/v1/auth/extension-login`), find the extension's
//! mailbox, then list, play, mark read and delete its messages. The JWT the
//! login mints lives in memory for the life of the process and nowhere else;
//! a 401 on any call drops it and signs in again once, so an expired token
//! costs one extra round trip rather than a reconnect prompt.
//!
//! Only `https://` is accepted for the API, except plain-http loopback in
//! debug builds for a local stub — the same rule as setup links. Message ids
//! go into the URL as single path segments, so an id from the server can never
//! reach another route.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

/// Which mailbox: the account's PBX API and the extension it is signed in as.
/// `domain` is the SIP domain, which on an Aria PBX is the tenant's domain.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub account_id: String,
    pub api_url: String,
    pub extension: String,
    pub domain: String,
}

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

#[derive(Debug)]
pub enum VoicemailError {
    /// Not an `https://` URL (or a loopback `http://` in a debug build).
    InvalidUrl,
    /// The PBX refused the extension's password, or every token it minted.
    Unauthorized,
    /// The extension exists but has no voicemail box.
    NoMailbox,
    /// The request never got an answer.
    Network(String),
    /// An answer other than the one expected.
    Status(u16),
    /// A 2xx whose body was not what the API documents.
    Malformed,
    /// The account has no saved password to sign in with.
    NoPassword,
}

impl VoicemailError {
    /// A stable name the frontend can switch on; the Display text is for people.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidUrl => "invalidUrl",
            Self::Unauthorized => "unauthorized",
            Self::NoMailbox => "noMailbox",
            Self::Network(_) => "network",
            Self::Status(_) => "status",
            Self::Malformed => "malformed",
            Self::NoPassword => "noPassword",
        }
    }
}

/// Over IPC an error is `{ "kind": ..., "message": ... }`, so the UI can tell
/// "no mailbox" from "the PBX is down" without parsing prose.
impl Serialize for VoicemailError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("VoicemailError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl fmt::Display for VoicemailError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "The PBX API address must be an https:// URL."),
            Self::Unauthorized => write!(f, "The PBX did not accept this extension's password for sign-in."),
            Self::NoMailbox => write!(f, "This extension has no voicemail mailbox on the PBX."),
            Self::Network(e) => write!(f, "Could not reach the PBX: {e}"),
            Self::Status(s) => write!(f, "The PBX answered with HTTP {s}."),
            Self::Malformed => write!(f, "The PBX sent a reply Aria could not read."),
            Self::NoPassword => write!(f, "This account has no saved password to sign in to the PBX with."),
        }
    }
}

impl std::error::Error for VoicemailError {}

/// Check an API address and reduce it to its origin (`scheme://host[:port]/`):
/// the API lives at the root of the host, and a pasted path or trailing slash
/// should not change where requests go.
pub fn validate_api_url(raw: &str, allow_local_http: bool) -> Result<reqwest::Url, VoicemailError> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| VoicemailError::InvalidUrl)?;
    let host = url.host_str().ok_or(VoicemailError::InvalidUrl)?;
    match url.scheme() {
        "https" => {}
        "http" if allow_local_http && matches!(host, "localhost" | "127.0.0.1") => {}
        _ => return Err(VoicemailError::InvalidUrl),
    }
    let origin = match url.port() {
        Some(port) => format!("{}://{host}:{port}/", url.scheme()),
        None => format!("{}://{host}/", url.scheme()),
    };
    reqwest::Url::parse(&origin).map_err(|_| VoicemailError::InvalidUrl)
}

/// `api` plus path segments, each one encoded as a single segment.
fn route(api: &reqwest::Url, segments: &[&str]) -> reqwest::Url {
    let mut url = api.clone();
    if let Ok(mut path) = url.path_segments_mut() {
        path.pop_if_empty().extend(segments);
    }
    url
}

#[derive(Debug, Clone)]
struct Session {
    jwt: String,
    extension_id: String,
    box_id: Option<String>,
}

#[derive(Deserialize)]
struct LoginReply {
    token: String,
    extension_id: String,
}

#[derive(Deserialize)]
struct BoxReply {
    id: String,
}

/// Sessions per account, shared by every voicemail command.
pub struct VoicemailState {
    http: reqwest::Client,
    sessions: Mutex<HashMap<String, Session>>,
}

impl Default for VoicemailState {
    fn default() -> Self {
        Self::new()
    }
}

impl VoicemailState {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("Aria/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self { http, sessions: Mutex::new(HashMap::new()) }
    }

    fn cached(&self, account_id: &str) -> Option<Session> {
        self.sessions.lock().unwrap_or_else(|p| p.into_inner()).get(account_id).cloned()
    }

    fn remember(&self, account_id: &str, session: Session) {
        self.sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account_id.to_string(), session);
    }

    fn forget(&self, account_id: &str) {
        self.sessions.lock().unwrap_or_else(|p| p.into_inner()).remove(account_id);
    }

    async fn login(&self, api: &reqwest::Url, mailbox: &Mailbox, password: &str) -> Result<Session, VoicemailError> {
        let reply = self
            .http
            .post(route(api, &["api", "v1", "auth", "extension-login"]))
            .json(&serde_json::json!({
                "extension_number": mailbox.extension,
                "password": password,
                "tenant_domain": mailbox.domain,
            }))
            .send()
            .await
            .map_err(net_error)?;
        let status = reply.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(VoicemailError::Unauthorized);
        }
        if !status.is_success() {
            return Err(VoicemailError::Status(status.as_u16()));
        }
        let login: LoginReply = reply.json().await.map_err(|_| VoicemailError::Malformed)?;
        Ok(Session { jwt: login.token, extension_id: login.extension_id, box_id: None })
    }

    /// The account's session, signing in when there is none.
    async fn session(&self, api: &reqwest::Url, mailbox: &Mailbox, password: &str) -> Result<Session, VoicemailError> {
        if let Some(session) = self.cached(&mailbox.account_id) {
            return Ok(session);
        }
        let session = self.login(api, mailbox, password).await?;
        self.remember(&mailbox.account_id, session.clone());
        Ok(session)
    }

    /// Send a signed request built by `build`. A 401 drops the session, signs
    /// in again and resends once; a second 401 is the caller's to report.
    async fn send(
        &self,
        mailbox: &Mailbox,
        password: &str,
        build: &(dyn Fn(&reqwest::Client, &reqwest::Url, &Session) -> reqwest::RequestBuilder + Send + Sync),
    ) -> Result<reqwest::Response, VoicemailError> {
        let api = validate_api_url(&mailbox.api_url, cfg!(debug_assertions))?;
        let mut session = self.session(&api, mailbox, password).await?;
        let mut retried = false;
        loop {
            let reply = build(&self.http, &api, &session).send().await.map_err(net_error)?;
            if reply.status() == reqwest::StatusCode::UNAUTHORIZED && !retried {
                retried = true;
                self.forget(&mailbox.account_id);
                session = self.session(&api, mailbox, password).await?;
                continue;
            }
            return Ok(reply);
        }
    }

    /// The id of the extension's mailbox, found once per session.
    async fn box_id(&self, mailbox: &Mailbox, password: &str) -> Result<String, VoicemailError> {
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
            reqwest::StatusCode::NOT_FOUND => return Err(VoicemailError::NoMailbox),
            s => return Err(status_error(s)),
        }
        let vbox: BoxReply = reply.json().await.map_err(|_| VoicemailError::Malformed)?;
        if let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(&mailbox.account_id)
        {
            session.box_id = Some(vbox.id.clone());
        }
        Ok(vbox.id)
    }

    /// The mailbox's messages as the PBX orders them (newest first).
    pub async fn list(&self, mailbox: &Mailbox, password: &str) -> Result<Vec<Message>, VoicemailError> {
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
        reply.json().await.map_err(|_| VoicemailError::Malformed)
    }

    pub async fn mark_read(&self, mailbox: &Mailbox, password: &str, message_id: &str) -> Result<(), VoicemailError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                http.patch(route(api, &["api", "v1", "voicemail", "messages", message_id, "read"]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        if reply.status().is_success() { Ok(()) } else { Err(status_error(reply.status())) }
    }

    pub async fn delete(&self, mailbox: &Mailbox, password: &str, message_id: &str) -> Result<(), VoicemailError> {
        let reply = self
            .send(mailbox, password, &|http, api, session| {
                http.delete(route(api, &["api", "v1", "voicemail", "messages", message_id]))
                    .bearer_auth(&session.jwt)
            })
            .await?;
        if reply.status().is_success() { Ok(()) } else { Err(status_error(reply.status())) }
    }

    /// The message's audio as the PBX stores it (WAV).
    pub async fn audio(&self, mailbox: &Mailbox, password: &str, message_id: &str) -> Result<Vec<u8>, VoicemailError> {
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

fn status_error(status: reqwest::StatusCode) -> VoicemailError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        VoicemailError::Unauthorized
    } else {
        VoicemailError::Status(status.as_u16())
    }
}

/// reqwest's Display includes the URL; the API host is not secret, but keep
/// the habit the rest of the app has and strip it anyway.
fn net_error(e: reqwest::Error) -> VoicemailError {
    let e = e.without_url();
    if e.is_timeout() {
        VoicemailError::Network("timed out".into())
    } else {
        VoicemailError::Network(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One request as the stub PBX saw it.
    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        authorization: Option<String>,
        body: String,
    }

    type Handler = Arc<dyn Fn(&Seen) -> (u16, &'static str, Vec<u8>) + Send + Sync>;

    /// A fake Aria PBX API on loopback: every request is parsed, recorded and
    /// answered by `handler`.
    struct Stub {
        base: String,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Stub {
        async fn start(handler: Handler) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let record = seen.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else { return };
                    let handler = handler.clone();
                    let record = record.clone();
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 2048];
                        let head_end = loop {
                            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                break i + 4;
                            }
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                        let mut lines = head.lines();
                        let request_line = lines.next().unwrap_or_default();
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or_default().to_string();
                        let path = parts.next().unwrap_or_default().to_string();
                        let mut authorization = None;
                        let mut content_length = 0usize;
                        for line in lines {
                            if let Some((name, value)) = line.split_once(':') {
                                match name.trim().to_ascii_lowercase().as_str() {
                                    "authorization" => authorization = Some(value.trim().to_string()),
                                    "content-length" => content_length = value.trim().parse().unwrap_or(0),
                                    _ => {}
                                }
                            }
                        }
                        while buf.len() < head_end + content_length {
                            match sock.read(&mut chunk).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                            }
                        }
                        let body = String::from_utf8_lossy(&buf[head_end..head_end + content_length]).to_string();
                        let seen = Seen { method, path, authorization, body };
                        let (status, content_type, reply) = handler(&seen);
                        record.lock().unwrap().push(seen);
                        let head = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            reply.len()
                        );
                        let _ = sock.write_all(head.as_bytes()).await;
                        let _ = sock.write_all(&reply).await;
                        let _ = sock.shutdown().await;
                    });
                }
            });
            Self { base, seen }
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    const MESSAGES: &str = r#"[
      {"id":"m2","box_id":"b1","caller_id":"+15551230002","duration_secs":42,"read":false,
       "created_at":"2026-11-24T10:00:00Z","transcription":"Call me back about the invoice."},
      {"id":"m1","box_id":"b1","caller_id":"1002","duration_secs":5,"read":true,
       "created_at":"2026-11-23T09:00:00Z","transcription":null}
    ]"#;

    fn json(status: u16, body: &str) -> (u16, &'static str, Vec<u8>) {
        (status, "application/json", body.as_bytes().to_vec())
    }

    /// A fresh SIP password per test, so no secret is written down anywhere.
    fn test_password() -> String {
        format!("pw-{}", uuid::Uuid::new_v4())
    }

    /// The PBX as the mobile apps see it. `tokens` is the sequence of JWTs
    /// successive logins mint; only the last one is accepted afterwards, so a
    /// stub with two tokens treats the first as expired. `password` is what
    /// the login must carry.
    fn pbx(tokens: &'static [&'static str], password: String) -> Handler {
        let logins = Arc::new(Mutex::new(0usize));
        Arc::new(move |req: &Seen| {
            if req.method == "POST" && req.path == "/api/v1/auth/extension-login" {
                let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
                if body["password"] != password.as_str() || body["extension_number"] != "1001" || body["tenant_domain"] != "test.ariaroute.com" {
                    return json(401, r#"{"error":"invalid credentials"}"#);
                }
                let mut n = logins.lock().unwrap();
                let token = tokens[(*n).min(tokens.len() - 1)];
                *n += 1;
                return json(200, &format!(r#"{{"token":"{token}","extension_id":"ext-1","tenant_id":"t-1"}}"#));
            }
            let valid = format!("Bearer {}", tokens[tokens.len() - 1]);
            if req.authorization.as_deref() != Some(valid.as_str()) {
                return json(401, r#"{"error":"unauthorized"}"#);
            }
            match (req.method.as_str(), req.path.as_str()) {
                ("GET", "/api/v1/extensions/ext-1/voicemail") => json(200, r#"{"id":"b1","extension_number":"1001","enabled":true}"#),
                ("GET", "/api/v1/voicemail/boxes/b1/messages?limit=100") => json(200, MESSAGES),
                ("PATCH", "/api/v1/voicemail/messages/m2/read") => (204, "text/plain", Vec::new()),
                ("DELETE", "/api/v1/voicemail/messages/m2") => (204, "text/plain", Vec::new()),
                ("GET", "/api/v1/voicemail/messages/m2/audio") => (200, "audio/wav", b"RIFFwav".to_vec()),
                _ => json(404, r#"{"error":"not found"}"#),
            }
        })
    }

    fn mailbox(stub: &Stub) -> Mailbox {
        Mailbox {
            account_id: "acct-1".into(),
            api_url: stub.base.clone(),
            extension: "1001".into(),
            domain: "test.ariaroute.com".into(),
        }
    }

    #[tokio::test]
    async fn lists_messages_after_logging_in_with_the_sip_credentials() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = VoicemailState::new();
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
        assert_eq!(login["password"], &pass);
        assert_eq!(login["tenant_domain"], "test.ariaroute.com");
        assert!(seen[1..].iter().all(|r| r.authorization.as_deref() == Some("Bearer jwt-1")), "{seen:?}");
        assert_eq!(seen[1].path, "/api/v1/extensions/ext-1/voicemail");
        assert_eq!(seen[2].path, "/api/v1/voicemail/boxes/b1/messages?limit=100");
    }

    #[tokio::test]
    async fn a_second_list_reuses_the_session_and_the_box() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = VoicemailState::new();
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
        let state = VoicemailState::new();
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
        let state = VoicemailState::new();
        let err = state.list(&mailbox(&stub), &format!("not-{pass}")).await.unwrap_err();
        assert!(matches!(err, VoicemailError::Unauthorized), "{err:?}");
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
        let err = VoicemailState::new().list(&mailbox(&stub), &pass).await.unwrap_err();
        assert!(matches!(err, VoicemailError::NoMailbox), "{err:?}");
    }

    #[tokio::test]
    async fn audio_is_fetched_with_the_bearer_token() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let bytes = VoicemailState::new().audio(&mailbox(&stub), &pass, "m2").await.unwrap();
        assert_eq!(bytes, b"RIFFwav");
        let last = stub.seen().pop().unwrap();
        assert_eq!(last.path, "/api/v1/voicemail/messages/m2/audio");
        assert_eq!(last.authorization.as_deref(), Some("Bearer jwt-1"));
    }

    #[tokio::test]
    async fn mark_read_and_delete_use_their_routes() {
        let pass = test_password();
        let stub = Stub::start(pbx(&["jwt-1"], pass.clone())).await;
        let state = VoicemailState::new();
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
        let err = VoicemailState::new().delete(&mailbox(&stub), &pass, "m2/read?x=../..").await.unwrap_err();
        assert!(matches!(err, VoicemailError::Status(404)), "{err:?}");
        let last = stub.seen().pop().unwrap();
        assert_eq!(last.method, "DELETE");
        assert_eq!(last.path, "/api/v1/voicemail/messages/m2%2Fread%3Fx=..%2F..");
    }

    #[test]
    fn plain_http_is_only_for_local_stubs() {
        assert!(validate_api_url("https://api.ariaroute.com", false).is_ok());
        assert!(validate_api_url("https://api.ariaroute.com/", false).is_ok());
        assert_eq!(
            validate_api_url("https://api.ariaroute.com:8443/some/path", false).unwrap().as_str(),
            "https://api.ariaroute.com:8443/"
        );
        assert!(validate_api_url("http://127.0.0.1:4000", true).is_ok());
        assert!(validate_api_url("http://pbx.example.com", true).is_err());
        assert!(validate_api_url("http://127.0.0.1:4000", false).is_err());
        assert!(validate_api_url("ftp://api.ariaroute.com", true).is_err());
        assert!(validate_api_url("api.ariaroute.com", false).is_err());
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
        let state = VoicemailState::new();
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

    #[test]
    fn errors_name_the_problem_without_the_secret() {
        let shown = format!("{}", VoicemailError::Unauthorized);
        assert!(shown.to_lowercase().contains("password") || shown.to_lowercase().contains("sign"), "{shown}");
        assert!(!format!("{}", VoicemailError::Network("boom".into())).is_empty());
        assert!(format!("{}", VoicemailError::NoMailbox).to_lowercase().contains("mailbox"));
    }
}
