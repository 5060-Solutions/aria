//! Business texting (SMS and MMS) against an Aria PBX.
//!
//! Signs in the way visual voicemail does (`voicemail::VoicemailState`: the
//! extension's SIP credentials at `/api/v1/auth/extension-login`, a JWT held in
//! memory) and then uses the shared-inbox routes under
//! `/api/v1/tenants/{tenant}/messaging/...`. The server decides who may use an
//! inbox (assigned to the extension, directly or through a ring group), the
//! destination policy, the segment limit and the rate limits; this module
//! carries the answer, including the server's own sentence on a refusal, to
//! the frontend.
//!
//! Live updates: one WebSocket per account to `/api/v1/ws` (the JWT in an `Authorization`
//! header), which
//! the PBX sends `sms_received` events on (ids only, no text). It lives here
//! rather than in the webview because the webview's CSP allows no sockets and
//! the JWT never leaves this process. The frontend polls too, so a dropped
//! socket costs delay and not messages.

use crate::voicemail::{
    net_error, route, status_error, Mailbox, Session, VoicemailError, VoicemailState,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Over IPC an error is `{ kind, message, status?, field? }`. `rejected` is an
/// answer from the PBX that is not a success: `message` is its sentence, shown
/// to the person as it is, and `field` names what it objected to (`to`, `body`).
#[derive(Debug, Serialize)]
pub struct MessagingError {
    pub kind: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl fmt::Display for MessagingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<VoicemailError> for MessagingError {
    fn from(e: VoicemailError) -> Self {
        let status = if let VoicemailError::Status(s) = &e {
            Some(*s)
        } else {
            None
        };
        Self {
            kind: e.kind(),
            message: e.to_string(),
            status,
            field: None,
        }
    }
}

/// A non-success answer. A 4xx is `rejected`: the PBX read the request and
/// said no, so the next attempt is a new request. A 5xx is `server`: it may or
/// may not have acted (a carrier that refused leaves the text stored as
/// failed), so the caller keeps its idempotency key for the retry.
pub(crate) fn rejected(status: u16, body: &[u8]) -> MessagingError {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let text = |key: &str| {
        parsed
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    // `error` carries the sentence a person reads.
    let message = text("error")
        .or_else(|| text("message"))
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| format!("The PBX answered with HTTP {status}."));
    let kind = if status >= 500 { "server" } else { "rejected" };
    MessagingError {
        kind,
        message,
        status: Some(status),
        field: text("field"),
    }
}

type Build<'a> = &'a (dyn Fn(&reqwest::Client, &reqwest::Url, &Session) -> reqwest::RequestBuilder
         + Send
         + Sync);

/// Send a signed request; a success is its body, anything else an error.
pub(crate) async fn call(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    build: Build<'_>,
) -> Result<Vec<u8>, MessagingError> {
    let reply = state.send(mailbox, password, build).await?;
    let status = reply.status();
    let body = reply.bytes().await.map_err(net_error)?.to_vec();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(status_error(status).into());
    }
    if !status.is_success() {
        return Err(rejected(status.as_u16(), &body));
    }
    Ok(body)
}

async fn call_json(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    build: Build<'_>,
) -> Result<Value, MessagingError> {
    let body = call(state, mailbox, password, build).await?;
    serde_json::from_slice(&body).map_err(|_| VoicemailError::Malformed.into())
}

/// `.../tenants/{tenant}/messaging/<rest>`, each part one path segment.
fn messaging_route(api: &reqwest::Url, tenant_id: &str, rest: &[&str]) -> reqwest::Url {
    let mut segments = vec!["api", "v1", "tenants", tenant_id, "messaging"];
    segments.extend_from_slice(rest);
    route(api, &segments)
}

/// The numbers this extension may text from.
pub async fn inboxes(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
) -> Result<Value, MessagingError> {
    call_json(state, mailbox, password, &|http, api, s| {
        http.get(messaging_route(api, &s.tenant_id, &["inboxes"]))
            .bearer_auth(&s.jwt)
    })
    .await
}

pub async fn conversations(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    did_id: &str,
) -> Result<Value, MessagingError> {
    call_json(state, mailbox, password, &|http, api, s| {
        let mut url = messaging_route(api, &s.tenant_id, &["inboxes", did_id, "conversations"]);
        url.query_pairs_mut().append_pair("limit", "200");
        http.get(url).bearer_auth(&s.jwt)
    })
    .await
}

/// A thread, newest first; `before` (RFC 3339) pages back.
pub async fn thread(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    did_id: &str,
    number: &str,
    before: Option<&str>,
) -> Result<Value, MessagingError> {
    call_json(state, mailbox, password, &|http, api, s| {
        let mut url = messaging_route(
            api,
            &s.tenant_id,
            &["inboxes", did_id, "conversations", number],
        );
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("limit", "100");
            if let Some(before) = before {
                q.append_pair("before", before);
            }
        }
        http.get(url).bearer_auth(&s.jwt)
    })
    .await
}

/// What a send carries. `idempotency_key` makes a retry of the same request
/// return the first message instead of texting twice.
pub struct Outgoing<'a> {
    pub did_id: &'a str,
    pub to: &'a str,
    pub body: &'a str,
    pub attachment_ids: &'a [String],
    pub idempotency_key: &'a str,
}

pub async fn send(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    out: &Outgoing<'_>,
) -> Result<Value, MessagingError> {
    let payload =
        serde_json::json!({ "to": out.to, "body": out.body, "attachment_ids": out.attachment_ids });
    call_json(state, mailbox, password, &|http, api, s| {
        http.post(messaging_route(
            api,
            &s.tenant_id,
            &["inboxes", out.did_id, "messages"],
        ))
        .bearer_auth(&s.jwt)
        .header("Idempotency-Key", out.idempotency_key)
        .json(&payload)
    })
    .await
}

/// Upload one picture for an MMS; returns `{id, content_type, size_bytes}`.
pub async fn upload(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    file_name: &str,
    bytes: &[u8],
) -> Result<Value, MessagingError> {
    call_json(state, mailbox, password, &|http, api, s| {
        let part = reqwest::multipart::Part::bytes(bytes.to_vec()).file_name(file_name.to_string());
        http.post(messaging_route(api, &s.tenant_id, &["attachments"]))
            .bearer_auth(&s.jwt)
            .multipart(reqwest::multipart::Form::new().part("file", part))
    })
    .await
}

/// An attachment's bytes.
pub async fn attachment(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    id: &str,
) -> Result<Vec<u8>, MessagingError> {
    call(state, mailbox, password, &|http, api, s| {
        http.get(messaging_route(api, &s.tenant_id, &["attachments", id]))
            .bearer_auth(&s.jwt)
    })
    .await
}

// ── Live events ─────────────────────────────────────────────────────────────

/// What the frontend is told when a text arrives: ids only, as the PBX sends
/// them. The text is fetched through the API, which checks access again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsArrived {
    pub account_id: String,
    pub did_id: String,
    pub message_id: String,
    pub remote_number: String,
}

/// The `sms_received` event in a WebSocket text frame; anything else is `None`.
pub fn parse_sms_event(account_id: &str, frame: &str) -> Option<SmsArrived> {
    let v: Value = serde_json::from_str(frame).ok()?;
    if v.get("type")?.as_str()? != "sms_received" {
        return None;
    }
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Some(SmsArrived {
        account_id: account_id.to_string(),
        did_id: field("did_id")?,
        message_id: field("message_id")?,
        remote_number: field("remote_number").unwrap_or_default(),
    })
}

/// The call-move events on the same stream (`call_active_elsewhere`,
/// `call_moved`, `call_ended`) and the handoff suggestion, passed on as the
/// PBX sent them. Anything else is `None`.
pub fn parse_call_event(frame: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(frame).ok()?;
    matches!(
        v.get("type")?.as_str()?,
        "call_active_elsewhere" | "call_moved" | "call_ended" | "handoff_suggested"
    )
    .then_some(v)
}

/// `https://host/` as the event stream's `wss://host/api/v1/ws`. The session
/// goes in an `Authorization` header, never the address: a URL is logged by
/// proxies and servers.
pub fn ws_url(api: &reqwest::Url) -> Option<reqwest::Url> {
    let mut url = route(api, &["api", "v1", "ws"]);
    url.set_scheme(if api.scheme() == "https" { "wss" } else { "ws" })
        .ok()?;
    Some(url)
}

/// How long to wait before reconnect `attempt` (0-based): 2 s doubling to 60 s.
pub fn backoff(attempt: u32) -> Duration {
    Duration::from_secs((2u64 << attempt.min(5)).min(60))
}

/// The running event listeners, one per account.
#[derive(Default)]
pub struct MessagingLive {
    tasks: Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

impl MessagingLive {
    /// Replace the account's listener with a new one running `task`.
    pub fn start(
        &self,
        account_id: &str,
        task: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tauri::async_runtime::spawn(task);
        if let Some(old) = self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account_id.to_string(), handle)
        {
            old.abort();
        }
    }

    pub fn stop(&self, account_id: &str) {
        if let Some(old) = self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account_id)
        {
            old.abort();
        }
    }
}

fn tls_connector() -> Result<tokio_tungstenite::Connector, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone())
        .map_err(|e| format!("platform verifier: {e}"))?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
}

/// One connection of the event stream, until it closes or fails.
async fn run_once(
    state: &VoicemailState,
    mailbox: &Mailbox,
    password: &str,
    on_event: &(impl Fn(SmsArrived) + Send + Sync),
    on_call_event: &(impl Fn(Value) + Send + Sync),
    on_live: &(impl Fn(bool) + Send + Sync),
) -> Result<(), String> {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as Frame;

    let api = crate::voicemail::validate_api_url(&mailbox.api_url, cfg!(debug_assertions))
        .map_err(|e| e.to_string())?;
    let session = state
        .session(&api, mailbox, password)
        .await
        .map_err(|e| e.to_string())?;
    let url = ws_url(&api).ok_or("bad event stream address")?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let bearer = format!("Bearer {}", session.jwt)
        .parse()
        .map_err(|_| "unusable session token".to_string())?;
    request.headers_mut().insert("Authorization", bearer);
    let connector = tls_connector()?;
    let mut socket = match tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(connector),
    )
    .await
    {
        Ok((socket, _)) => socket,
        Err(e) => {
            // A refused upgrade is most often a token that has expired.
            if matches!(&e, tokio_tungstenite::tungstenite::Error::Http(r) if r.status() == 401) {
                state.forget(&mailbox.account_id);
            }
            return Err(e.to_string());
        }
    };
    on_live(true);
    while let Some(frame) = socket.next().await {
        match frame.map_err(|e| e.to_string())? {
            Frame::Text(text) => {
                if let Some(event) = parse_sms_event(&mailbox.account_id, text.as_str()) {
                    on_event(event);
                } else if let Some(event) = parse_call_event(text.as_str()) {
                    on_call_event(event);
                }
            }
            Frame::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// Hold the account's event stream open for as long as the task lives,
/// reconnecting with a backoff. `on_event` gets each arrived text; `on_live`
/// is told when the stream goes up or down.
pub async fn listen(
    state: Arc<VoicemailState>,
    mailbox: Mailbox,
    password: String,
    on_event: impl Fn(SmsArrived) + Send + Sync,
    on_call_event: impl Fn(Value) + Send + Sync,
    on_live: impl Fn(bool) + Send + Sync,
) {
    let mut attempt = 0u32;
    loop {
        let began = std::time::Instant::now();
        let outcome = run_once(
            &state,
            &mailbox,
            &password,
            &on_event,
            &on_call_event,
            &on_live,
        )
        .await;
        on_live(false);
        if let Err(e) = &outcome {
            log::warn!("[messaging] event stream: {e}");
        }
        // A stream that stayed up a while starts the backoff over.
        if began.elapsed() > Duration::from_secs(60) {
            attempt = 0;
        }
        tokio::time::sleep(backoff(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn a_refusal_carries_the_servers_sentence_and_field() {
        let e = rejected(
            422,
            br#"{"error":"That person opted out (STOP).","field":"to","status":422}"#,
        );
        assert_eq!(e.kind, "rejected");
        assert_eq!(e.message, "That person opted out (STOP).");
        assert_eq!(e.field.as_deref(), Some("to"));
        assert_eq!(e.status, Some(422));
    }

    #[test]
    fn a_refusal_without_a_body_names_the_status() {
        let e = rejected(502, b"");
        assert_eq!(e.message, "The PBX answered with HTTP 502.");
        assert_eq!(e.field, None);
    }

    #[test]
    fn a_server_error_is_not_a_refusal() {
        assert_eq!(rejected(502, b"{\"error\":\"carrier\"}").kind, "server");
        assert_eq!(rejected(500, b"").kind, "server");
        assert_eq!(rejected(429, b"").kind, "rejected");
        assert_eq!(rejected(404, b"").kind, "rejected");
    }

    #[test]
    fn parses_only_sms_received_events() {
        let frame = r#"{"type":"sms_received","tenant_id":"t","did_id":"d1","message_id":"m1","remote_number":"+13035550100","extension_ids":["e"]}"#;
        assert_eq!(
            parse_sms_event("acct", frame),
            Some(SmsArrived {
                account_id: "acct".into(),
                did_id: "d1".into(),
                message_id: "m1".into(),
                remote_number: "+13035550100".into()
            })
        );
        assert_eq!(parse_sms_event("acct", r#"{"type":"call_started"}"#), None);
        assert_eq!(parse_sms_event("acct", "not json"), None);
    }

    #[test]
    fn passes_on_call_move_events_and_nothing_else() {
        for t in [
            "call_active_elsewhere",
            "call_moved",
            "call_ended",
            "handoff_suggested",
        ] {
            let frame = format!(r#"{{"type":"{t}","call_id":"c"}}"#);
            assert_eq!(parse_call_event(&frame).unwrap()["type"], t);
        }
        assert!(parse_call_event(r#"{"type":"sms_received"}"#).is_none());
        assert!(parse_call_event("nope").is_none());
    }

    #[test]
    fn the_event_stream_address_follows_the_api_scheme() {
        let https = reqwest::Url::parse("https://api.ariaroute.com/").unwrap();
        assert_eq!(
            ws_url(&https).unwrap().as_str(),
            "wss://api.ariaroute.com/api/v1/ws"
        );
        let http = reqwest::Url::parse("http://127.0.0.1:4000/").unwrap();
        assert_eq!(
            ws_url(&http).unwrap().as_str(),
            "ws://127.0.0.1:4000/api/v1/ws"
        );
    }

    #[test]
    fn backoff_doubles_to_a_minute() {
        let secs: Vec<u64> = (0..8).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn a_number_is_one_path_segment() {
        let api = reqwest::Url::parse("https://api.ariaroute.com/").unwrap();
        let url = messaging_route(
            &api,
            "t-1",
            &["inboxes", "d1", "conversations", "+1303/../x"],
        );
        assert_eq!(
            url.path(),
            "/api/v1/tenants/t-1/messaging/inboxes/d1/conversations/+1303%2F..%2Fx"
        );
    }

    /// A one-shot stub PBX: records each request line and headers, answers
    /// with `reply(path)`.
    async fn stub(reply: fn(&str, &str) -> (u16, String)) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let record = record.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head_end, length) = loop {
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                            let length = head
                                .lines()
                                .find_map(|l| {
                                    l.strip_prefix("content-length:")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            break (i + 4, length);
                        }
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    };
                    while buf.len() < head_end + length {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let body =
                        String::from_utf8_lossy(&buf[head_end..head_end + length]).to_string();
                    let request_line = head.lines().next().unwrap_or_default().to_string();
                    let (status, reply_body) = reply(&request_line, &body);
                    record.lock().unwrap().push(format!(
                        "{request_line}\n{}\n{body}",
                        head.to_ascii_lowercase()
                    ));
                    let out = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply_body}",
                        reply_body.len()
                    );
                    let _ = sock.write_all(out.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (base, seen)
    }

    fn pbx(request_line: &str, _body: &str) -> (u16, String) {
        if request_line.starts_with("POST /api/v1/auth/extension-login") {
            return (
                200,
                r#"{"token":"jwt-1","extension_id":"e1","tenant_id":"t1"}"#.into(),
            );
        }
        if request_line.starts_with("GET /api/v1/tenants/t1/messaging/inboxes ") {
            return (200, r#"[{"did_id":"d1","number":"+13035550100","last_inbound_at":null,"enabled":true}]"#.into());
        }
        if request_line.starts_with("POST /api/v1/tenants/t1/messaging/inboxes/d1/messages ") {
            return (
                422,
                r#"{"error":"That person opted out (STOP).","field":"to","status":422}"#.into(),
            );
        }
        (404, r#"{"error":"nope"}"#.into())
    }

    fn mailbox(base: &str) -> Mailbox {
        Mailbox {
            account_id: "a".into(),
            api_url: base.into(),
            extension: "1001".into(),
            domain: "t.example".into(),
        }
    }

    #[tokio::test]
    async fn lists_inboxes_under_the_logged_in_tenant() {
        let (base, seen) = stub(pbx).await;
        let state = VoicemailState::new();
        let v = inboxes(&state, &mailbox(&base), "pw").await.unwrap();
        assert_eq!(v[0]["number"], "+13035550100");
        let seen = seen.lock().unwrap().clone();
        assert!(seen[1].contains("authorization: bearer jwt-1"), "{seen:?}");
    }

    #[tokio::test]
    async fn a_refused_send_carries_the_servers_message_and_the_idempotency_key() {
        let (base, seen) = stub(pbx).await;
        let state = VoicemailState::new();
        let ids: Vec<String> = vec![];
        let out = Outgoing {
            did_id: "d1",
            to: "+13035550101",
            body: "hi",
            attachment_ids: &ids,
            idempotency_key: "key-1",
        };
        let e = send(&state, &mailbox(&base), "pw", &out).await.unwrap_err();
        assert_eq!(e.kind, "rejected");
        assert_eq!(e.message, "That person opted out (STOP).");
        assert_eq!(e.field.as_deref(), Some("to"));
        let seen = seen.lock().unwrap().clone();
        let post = seen.last().unwrap();
        assert!(post.contains("idempotency-key: key-1"), "{post}");
        assert!(post.contains(r#""to":"+13035550101""#), "{post}");
    }
}
