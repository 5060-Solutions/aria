//! The Aria PBX REST API as a signed-in extension: what visual voicemail and
//! messaging share.
//!
//! The same surface the mobile apps use: sign in with the extension's SIP
//! credentials (`POST /api/v1/auth/extension-login`) and call the API with
//! the JWT the login mints. The token lives in memory for the life of the
//! process and nowhere else; a 401 on any call drops it and signs in again
//! once, so an expired token costs one extra round trip rather than a
//! reconnect prompt.
//!
//! Only `https://` is accepted for the API, except plain-http loopback in
//! debug builds for a local stub — the same rule as setup links. Ids and
//! numbers go into URLs as single path segments, so a value from the server
//! can never reach another route.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

/// Which line: the account's PBX API and the extension it is signed in as.
/// `domain` is the SIP domain, which on an Aria PBX is the tenant's domain.
/// Named for its first use, voicemail; the frontend's `mailboxFor` builds it
/// for every PBX feature.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailbox {
    pub account_id: String,
    pub api_url: String,
    pub extension: String,
    pub domain: String,
}

#[derive(Debug)]
pub enum PbxError {
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
    /// The PBX refused the request and said why, in words meant for the
    /// person (an opted-out recipient, a daily limit, a number not theirs).
    Rejected(String),
}

impl PbxError {
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
            Self::Rejected(_) => "rejected",
        }
    }
}

/// Over IPC an error is `{ "kind": ..., "message": ... }`, so the UI can tell
/// "no mailbox" from "the PBX is down" without parsing prose.
impl Serialize for PbxError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("PbxError", 2)?;
        s.serialize_field("kind", self.kind())?;
        s.serialize_field("message", &self.to_string())?;
        s.end()
    }
}

impl fmt::Display for PbxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl => write!(f, "The PBX API address must be an https:// URL."),
            Self::Unauthorized => write!(f, "The PBX did not accept this extension's password for sign-in."),
            Self::NoMailbox => write!(f, "This extension has no voicemail mailbox on the PBX."),
            Self::Network(e) => write!(f, "Could not reach the PBX: {e}"),
            Self::Status(s) => write!(f, "The PBX answered with HTTP {s}."),
            Self::Malformed => write!(f, "The PBX sent a reply Aria could not read."),
            Self::NoPassword => write!(f, "This account has no saved password to sign in to the PBX with."),
            Self::Rejected(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for PbxError {}

/// Check an API address and reduce it to its origin (`scheme://host[:port]/`):
/// the API lives at the root of the host, and a pasted path or trailing slash
/// should not change where requests go.
pub fn validate_api_url(raw: &str, allow_local_http: bool) -> Result<reqwest::Url, PbxError> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| PbxError::InvalidUrl)?;
    let host = url.host_str().ok_or(PbxError::InvalidUrl)?;
    match url.scheme() {
        "https" => {}
        "http" if allow_local_http && matches!(host, "localhost" | "127.0.0.1") => {}
        _ => return Err(PbxError::InvalidUrl),
    }
    let origin = match url.port() {
        Some(port) => format!("{}://{host}:{port}/", url.scheme()),
        None => format!("{}://{host}/", url.scheme()),
    };
    reqwest::Url::parse(&origin).map_err(|_| PbxError::InvalidUrl)
}

/// `api` plus path segments, each one encoded as a single segment.
pub(crate) fn route(api: &reqwest::Url, segments: &[&str]) -> reqwest::Url {
    let mut url = api.clone();
    if let Ok(mut path) = url.path_segments_mut() {
        path.pop_if_empty().extend(segments);
    }
    url
}

/// One extension's sign-in: the token and what the login said about it.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub(crate) jwt: String,
    pub(crate) extension_id: String,
    /// The tenant the extension belongs to; messaging routes live under it.
    pub(crate) tenant_id: String,
    /// The voicemail box, once looked up.
    pub(crate) box_id: Option<String>,
}

#[derive(Deserialize)]
struct LoginReply {
    token: String,
    extension_id: String,
    #[serde(default)]
    tenant_id: String,
}

/// Sessions per account, shared by every command that talks to the PBX API.
pub struct PbxClient {
    http: reqwest::Client,
    sessions: Mutex<HashMap<String, Session>>,
}

impl Default for PbxClient {
    fn default() -> Self {
        Self::new()
    }
}

impl PbxClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("Aria/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self { http, sessions: Mutex::new(HashMap::new()) }
    }

    pub(crate) fn cached(&self, account_id: &str) -> Option<Session> {
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

    /// Change the account's cached session in place (nothing when there is none).
    pub(crate) fn update_session(&self, account_id: &str, change: impl FnOnce(&mut Session)) {
        if let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_mut(account_id)
        {
            change(session);
        }
    }

    async fn login(&self, api: &reqwest::Url, mailbox: &Mailbox, password: &str) -> Result<Session, PbxError> {
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
            return Err(PbxError::Unauthorized);
        }
        if !status.is_success() {
            return Err(PbxError::Status(status.as_u16()));
        }
        let login: LoginReply = reply.json().await.map_err(|_| PbxError::Malformed)?;
        Ok(Session { jwt: login.token, extension_id: login.extension_id, tenant_id: login.tenant_id, box_id: None })
    }

    /// The account's session, signing in when there is none.
    async fn session(&self, api: &reqwest::Url, mailbox: &Mailbox, password: &str) -> Result<Session, PbxError> {
        if let Some(session) = self.cached(&mailbox.account_id) {
            return Ok(session);
        }
        let session = self.login(api, mailbox, password).await?;
        self.remember(&mailbox.account_id, session.clone());
        Ok(session)
    }

    /// Send a signed request built by `build`. A 401 drops the session, signs
    /// in again and resends once; a second 401 is the caller's to report.
    pub(crate) async fn send(
        &self,
        mailbox: &Mailbox,
        password: &str,
        build: &(dyn Fn(&reqwest::Client, &reqwest::Url, &Session) -> reqwest::RequestBuilder + Send + Sync),
    ) -> Result<reqwest::Response, PbxError> {
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
}

pub(crate) fn status_error(status: reqwest::StatusCode) -> PbxError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        PbxError::Unauthorized
    } else {
        PbxError::Status(status.as_u16())
    }
}

/// The error a failed reply stands for: a 4xx whose body says why (the API's
/// `{"error": "..."}`) is `Rejected` with those words; anything else is the
/// bare status.
pub(crate) async fn error_from(reply: reqwest::Response) -> PbxError {
    let status = reply.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return PbxError::Unauthorized;
    }
    if status.is_client_error() {
        if let Ok(body) = reply.json::<serde_json::Value>().await {
            if let Some(reason) = body.get("error").and_then(|e| e.as_str()).filter(|s| !s.trim().is_empty()) {
                return PbxError::Rejected(reason.trim().to_string());
            }
        }
    }
    PbxError::Status(status.as_u16())
}

/// reqwest's Display includes the URL; the API host is not secret, but keep
/// the habit the rest of the app has and strip it anyway.
pub(crate) fn net_error(e: reqwest::Error) -> PbxError {
    let e = e.without_url();
    if e.is_timeout() {
        PbxError::Network("timed out".into())
    } else {
        PbxError::Network(e.to_string())
    }
}

/// A fake Aria PBX API on loopback for the voicemail and messaging tests.
#[cfg(test)]
pub(crate) mod stub {
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// One request as the stub PBX saw it.
    #[derive(Debug, Clone)]
    pub(crate) struct Seen {
        pub(crate) method: String,
        pub(crate) path: String,
        pub(crate) authorization: Option<String>,
        pub(crate) idempotency_key: Option<String>,
        pub(crate) body: String,
    }

    pub(crate) type Reply = (u16, &'static str, Vec<u8>);
    pub(crate) type Handler = Arc<dyn Fn(&Seen) -> Reply + Send + Sync>;

    /// Every request is parsed, recorded and answered by `handler`.
    pub(crate) struct Stub {
        pub(crate) base: String,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Stub {
        pub(crate) async fn start(handler: Handler) -> Self {
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
                        let mut idempotency_key = None;
                        let mut content_length = 0usize;
                        for line in lines {
                            if let Some((name, value)) = line.split_once(':') {
                                match name.trim().to_ascii_lowercase().as_str() {
                                    "authorization" => authorization = Some(value.trim().to_string()),
                                    "idempotency-key" => idempotency_key = Some(value.trim().to_string()),
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
                        let seen = Seen { method, path, authorization, idempotency_key, body };
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

        pub(crate) fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    pub(crate) fn json(status: u16, body: &str) -> Reply {
        (status, "application/json", body.as_bytes().to_vec())
    }

    /// A fresh SIP password per test, so no secret is written down anywhere.
    pub(crate) fn test_password() -> String {
        format!("pw-{}", uuid::Uuid::new_v4())
    }

    /// The sign-in side of a stub PBX. `tokens` is the sequence of JWTs
    /// successive logins mint; only the last one is accepted afterwards, so
    /// two tokens treat the first as expired. `password` is what the login
    /// must carry (extension 1001 on test.ariaroute.com, tenant `t-1`).
    /// Answers the login and a stale or missing token; `None` leaves the
    /// request to the feature's own handler.
    pub(crate) fn login(tokens: &'static [&'static str], password: String) -> impl Fn(&Seen) -> Option<Reply> {
        let logins = Mutex::new(0usize);
        move |req: &Seen| {
            if req.method == "POST" && req.path == "/api/v1/auth/extension-login" {
                let body: serde_json::Value = serde_json::from_str(&req.body).unwrap_or_default();
                if body["password"] != password.as_str()
                    || body["extension_number"] != "1001"
                    || body["tenant_domain"] != "test.ariaroute.com"
                {
                    return Some(json(401, r#"{"error":"bad credentials"}"#));
                }
                let mut n = logins.lock().unwrap();
                let token = tokens[(*n).min(tokens.len() - 1)];
                *n += 1;
                return Some(json(
                    200,
                    &format!(r#"{{"token":"{token}","extension_id":"ext-1","extension_number":"1001","display_name":"Front Desk","tenant_id":"t-1"}}"#),
                ));
            }
            let valid = format!("Bearer {}", tokens[tokens.len() - 1]);
            if req.authorization.as_deref() != Some(valid.as_str()) {
                return Some(json(401, r#"{"error":"expired"}"#));
            }
            None
        }
    }

    pub(crate) fn mailbox(stub: &Stub) -> super::Mailbox {
        super::Mailbox {
            account_id: "acct-1".into(),
            api_url: stub.base.clone(),
            extension: "1001".into(),
            domain: "test.ariaroute.com".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn errors_name_the_problem_without_the_secret() {
        let shown = format!("{}", PbxError::Unauthorized);
        assert!(shown.to_lowercase().contains("password") || shown.to_lowercase().contains("sign"), "{shown}");
        assert!(!format!("{}", PbxError::Network("boom".into())).is_empty());
        assert!(format!("{}", PbxError::NoMailbox).to_lowercase().contains("mailbox"));
        assert_eq!(format!("{}", PbxError::Rejected("+13035550100 has opted out.".into())), "+13035550100 has opted out.");
    }

    #[test]
    fn errors_serialise_with_a_kind_the_ui_can_switch_on() {
        let v = serde_json::to_value(PbxError::Rejected("Too many messages today.".into())).unwrap();
        assert_eq!(v["kind"], "rejected");
        assert_eq!(v["message"], "Too many messages today.");
    }
}
