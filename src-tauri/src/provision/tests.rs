//! Setup-link tests against a stub HTTP server on 127.0.0.1. Nothing here
//! touches the network beyond loopback, or the credential store.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

const SHORT: Duration = Duration::from_secs(5);

/// How the stub answers every request.
#[derive(Clone)]
enum Reply {
    /// A full raw HTTP response.
    Raw(String),
    /// Accept, read the request, and never answer.
    Hang,
}

struct Stub {
    port: u16,
    /// The request line of every request received, e.g. `GET /provision/... HTTP/1.1`.
    requests: Arc<Mutex<Vec<String>>>,
}

impl Stub {
    async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let reply = reply.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).to_string();
                    seen.lock().unwrap().push(head.lines().next().unwrap_or_default().to_string());
                    match reply {
                        Reply::Raw(r) => {
                            let _ = sock.write_all(r.as_bytes()).await;
                            let _ = sock.shutdown().await;
                        }
                        Reply::Hang => tokio::time::sleep(Duration::from_secs(30)).await,
                    }
                });
            }
        });
        Self { port, requests }
    }

    async fn json(status: &str, body: &serde_json::Value) -> Self {
        Self::start(Reply::Raw(response(status, "application/json", &body.to_string()))).await
    }

    fn url(&self) -> reqwest::Url {
        validate_redeem_url(
            &format!("http://127.0.0.1:{}/provision/softphone/tok123", self.port),
            true,
        )
        .unwrap()
    }

    fn methods(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|l| l.split(' ').next().unwrap_or_default().to_string())
            .collect()
    }
}

fn response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn preview_body() -> serde_json::Value {
    json!({
        "v": 1,
        "tenant_name": "Regular Test",
        "extension": "1001",
        "display_name": "Front Desk",
        "domain": "regular-test.ariaroute.com",
        "expires_at": "2026-11-05T12:00:00Z"
    })
}

fn redeem_body() -> serde_json::Value {
    json!({
        "v": 1,
        "tenant_name": "Regular Test",
        "display_name": "Front Desk",
        "username": "1001",
        "auth_username": "1001-auth",
        "password": "s3cret-pw",
        "domain": "regular-test.ariaroute.com",
        "outbound_proxy": "sip.ariaroute.com",
        "port": 5061,
        "transport": "tls",
        "voicemail": "*97"
    })
}

fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => unreachable!(),
    }
}

// ── URL validation ──────────────────────────────────────────────────────────

#[test]
fn accepts_an_https_redeem_url() {
    let url = validate_redeem_url("https://api.ariaroute.com/provision/softphone/abc", false).unwrap();
    assert_eq!(server_host(&url), "api.ariaroute.com");
}

#[test]
fn rejects_urls_outside_the_contract() {
    for (raw, local) in [
        ("http://api.ariaroute.com/provision/softphone/abc", true),
        ("http://localhost:8787/provision/softphone/abc", false),
        ("http://127.0.0.1/provision/softphone/abc", false),
        ("https://user:pw@api.ariaroute.com/provision/softphone/abc", false),
        ("https://user@api.ariaroute.com/provision/softphone/abc", false),
        ("https://api.ariaroute.com/provision/softphone/", false),
        ("https://api.ariaroute.com/provision/other/abc", false),
        ("https://api.ariaroute.com/x/provision/softphone/abc", false),
        ("ftp://api.ariaroute.com/provision/softphone/abc", false),
        ("file:///provision/softphone/abc", false),
        ("not a url", false),
        ("", false),
    ] {
        assert!(validate_redeem_url(raw, local).is_err(), "{raw} should be refused");
    }
}

#[test]
fn allows_local_http_only_when_asked() {
    assert!(validate_redeem_url("http://localhost:8787/provision/softphone/abc", true).is_ok());
    assert!(validate_redeem_url("http://127.0.0.1:8787/provision/softphone/abc", true).is_ok());
    assert_eq!(
        server_host(&validate_redeem_url("http://localhost:8787/provision/softphone/abc", true).unwrap()),
        "localhost:8787"
    );
}

#[test]
fn errors_never_carry_the_token() {
    let all = [
        ProvisionError::InvalidUrl("x"),
        ProvisionError::Expired,
        ProvisionError::Status(500),
        ProvisionError::BadReply("x"),
        ProvisionError::Version,
        ProvisionError::TooLarge,
        ProvisionError::Timeout,
        ProvisionError::Unreachable,
    ];
    for e in all {
        assert!(!e.user_message().contains("tok123"));
        assert!(!e.user_message().is_empty());
    }
}

// ── Preview ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn preview_ok_uses_get() {
    let stub = Stub::json("200 OK", &preview_body()).await;
    let preview = fetch_preview(&stub.url(), SHORT).await.unwrap();
    assert_eq!(
        preview,
        ProvisionPreview {
            tenant_name: "Regular Test".into(),
            extension: "1001".into(),
            display_name: "Front Desk".into(),
            domain: "regular-test.ariaroute.com".into(),
            expires_at: Some("2026-11-05T12:00:00Z".into()),
            server_host: format!("127.0.0.1:{}", stub.port),
        }
    );
    assert_eq!(stub.methods(), vec!["GET"]);
}

#[tokio::test]
async fn not_found_means_expired_or_used() {
    let stub = Stub::json("404 Not Found", &json!({"error": "not_found"})).await;
    assert_eq!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::Expired));
    assert_eq!(redeem(&stub.url(), SHORT).await, Err(ProvisionError::Expired));
    assert_eq!(
        ProvisionError::Expired.user_message(),
        "This setup link has expired or was already used. Ask for a new one."
    );
}

#[tokio::test]
async fn other_statuses_fail() {
    let stub = Stub::json("500 Internal Server Error", &preview_body()).await;
    assert_eq!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::Status(500)));
}

#[tokio::test]
async fn bad_json_fails() {
    let stub = Stub::start(Reply::Raw(response("200 OK", "application/json", "{not json"))).await;
    assert!(matches!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::BadReply(_))));
    let stub = Stub::start(Reply::Raw(response("200 OK", "text/html", "<html></html>"))).await;
    assert!(matches!(redeem(&stub.url(), SHORT).await, Err(ProvisionError::BadReply(_))));
    let stub = Stub::json("200 OK", &json!([1, 2])).await;
    assert!(matches!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::BadReply(_))));
}

#[tokio::test]
async fn wrong_version_fails() {
    for v in [json!(2), json!("1"), serde_json::Value::Null] {
        let mut body = preview_body();
        body["v"] = v;
        let stub = Stub::json("200 OK", &body).await;
        assert_eq!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::Version));
    }
}

#[tokio::test]
async fn missing_required_preview_fields_fail() {
    for key in ["extension", "domain"] {
        let mut body = preview_body();
        body.as_object_mut().unwrap().remove(key);
        let stub = Stub::json("200 OK", &body).await;
        assert!(
            matches!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::BadReply(_))),
            "missing {key}"
        );
    }
}

#[tokio::test]
async fn oversize_body_fails() {
    let mut body = preview_body();
    body["display_name"] = json!("x".repeat(MAX_BODY_BYTES));
    let stub = Stub::json("200 OK", &body).await;
    assert_eq!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::TooLarge));

    // No Content-Length: the cap applies while reading, too.
    let big = body.to_string();
    let raw = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{big}");
    let stub = Stub::start(Reply::Raw(raw)).await;
    assert_eq!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::TooLarge));
}

#[tokio::test]
async fn redirects_are_not_followed() {
    // The redirect target would answer 200 with a valid preview.
    let target = Stub::json("200 OK", &preview_body()).await;
    let raw = format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        target.url()
    );
    let stub = Stub::start(Reply::Raw(raw)).await;
    assert_eq!(fetch_preview(&stub.url(), SHORT).await, Err(ProvisionError::Status(302)));
    assert!(target.methods().is_empty(), "the redirect target must not be contacted");
}

#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let stub = Stub::start(Reply::Hang).await;
    assert_eq!(
        fetch_preview(&stub.url(), Duration::from_millis(300)).await,
        Err(ProvisionError::Timeout)
    );
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let url = validate_redeem_url(&format!("http://127.0.0.1:{port}/provision/softphone/t"), true).unwrap();
    assert_eq!(fetch_preview(&url, SHORT).await, Err(ProvisionError::Unreachable));
}

// ── Redeem ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn redeem_ok_uses_post_and_maps_the_proxy() {
    let stub = Stub::json("200 OK", &redeem_body()).await;
    let line = redeem(&stub.url(), SHORT).await.unwrap();
    assert_eq!(stub.methods(), vec!["POST"]);
    assert_eq!(line.username, "1001");
    assert_eq!(line.auth_username.as_deref(), Some("1001-auth"));
    assert_eq!(line.password, "s3cret-pw");
    assert_eq!(line.domain, "regular-test.ariaroute.com");
    assert_eq!(line.outbound_proxy.as_deref(), Some("sip.ariaroute.com:5061"));
    assert_eq!(line.tls_server_name.as_deref(), Some("sip.ariaroute.com"));
    assert_eq!(line.port, 5061);
    assert_eq!(line.transport, "tls");
    assert_eq!(line.voicemail.as_deref(), Some("*97"));
    assert_eq!(line.display_name, "Front Desk");
    // The password never reaches a Debug print.
    assert!(!format!("{line:?}").contains("s3cret-pw"));
}

#[test]
fn redeem_defaults_and_optional_fields() {
    let mut body = redeem_body();
    let m = body.as_object_mut().unwrap();
    for key in ["auth_username", "outbound_proxy", "port", "transport", "voicemail", "display_name"] {
        m.remove(key);
    }
    let line = parse_redeem(&obj(body)).unwrap();
    assert_eq!(line.auth_username, None);
    assert_eq!(line.outbound_proxy, None);
    assert_eq!(line.tls_server_name, None);
    assert_eq!(line.port, 5061);
    assert_eq!(line.transport, "tls");
    assert_eq!(line.display_name, "1001");
}

#[test]
fn srtp_is_read_when_present() {
    for (raw, want) in [("sdes", "sdes"), ("disabled", "disabled"), (" SDES ", "sdes")] {
        let mut body = redeem_body();
        body["srtp"] = json!(raw);
        assert_eq!(parse_redeem(&obj(body)).unwrap().srtp.as_deref(), Some(want), "{raw:?}");
    }
}

#[test]
fn srtp_absent_is_none() {
    let line = parse_redeem(&obj(redeem_body())).unwrap();
    assert_eq!(line.srtp, None);
}

#[test]
fn unknown_srtp_counts_as_absent() {
    for raw in [json!("dtls"), json!("zrtp"), json!(""), json!(true), json!(1), serde_json::Value::Null] {
        let mut body = redeem_body();
        body["srtp"] = raw.clone();
        // The rest of the line is still usable.
        let line = parse_redeem(&obj(body)).unwrap();
        assert_eq!(line.srtp, None, "{raw}");
        assert_eq!(line.username, "1001");
    }
}

#[tokio::test]
async fn redeem_passes_srtp_through_to_the_ui() {
    let mut body = redeem_body();
    body["srtp"] = json!("sdes");
    let stub = Stub::json("200 OK", &body).await;
    let (line, _) =
        redeem_checked(&stub.url(), &expected("1001", "regular-test.ariaroute.com"), &[], "new-id", SHORT)
            .await
            .unwrap();
    assert_eq!(line.srtp.as_deref(), Some("sdes"));
    assert_eq!(line.transport, "tls");
}

#[test]
fn proxy_forms() {
    for (raw, want) in [
        ("sip.ariaroute.com", "sip.ariaroute.com:5061"),
        ("sip.ariaroute.com:5071", "sip.ariaroute.com:5071"),
        ("sips:sip.ariaroute.com:5061", "sip.ariaroute.com:5061"),
        ("SIP:Sip.AriaRoute.com", "sip.ariaroute.com:5061"),
    ] {
        let mut body = redeem_body();
        body["outbound_proxy"] = json!(raw);
        assert_eq!(parse_redeem(&obj(body)).unwrap().outbound_proxy.as_deref(), Some(want), "{raw}");
    }
    for raw in ["sip.ariaroute.com;transport=tls", "evil.com\r\nVia: x", "a b", "host:0", "host:99999"] {
        let mut body = redeem_body();
        body["outbound_proxy"] = json!(raw);
        assert!(parse_redeem(&obj(body)).is_err(), "{raw:?}");
    }
}

#[test]
fn redeem_rejects_unusable_fields() {
    let cases: [(&str, serde_json::Value); 8] = [
        ("username", serde_json::Value::Null),
        ("username", json!("10 01")),
        ("username", json!("1001@evil")),
        ("password", json!("")),
        ("domain", json!("evil.com\r\nX: y")),
        ("transport", json!("ws")),
        ("port", json!(0)),
        ("port", json!("5061")),
    ];
    for (key, value) in cases {
        let mut body = redeem_body();
        body[key] = value.clone();
        assert!(parse_redeem(&obj(body)).is_err(), "{key} = {value}");
    }
}

fn known(id: &str, username: &str, domain: &str, from: Option<&str>) -> KnownAccount {
    KnownAccount {
        id: id.into(),
        username: username.into(),
        domain: domain.into(),
        provisioned_from: from.map(String::from),
    }
}

fn expected(extension: &str, domain: &str) -> ExpectedLine {
    ExpectedLine { extension: extension.into(), domain: domain.into() }
}

#[test]
fn only_an_account_from_the_same_host_is_updated() {
    let line = parse_redeem(&obj(redeem_body())).unwrap();
    let host = "api.ariaroute.com";
    let other_line = known("a", "1002", "regular-test.ariaroute.com", Some(host));
    let by_hand = known("h", "1001", "Regular-Test.AriaRoute.com", None);
    let other_host = known("o", "1001", "regular-test.ariaroute.com", Some("evil.example"));
    let own = known("b", "1001", "Regular-Test.AriaRoute.com", Some("API.ariaroute.com"));

    assert_eq!(pick_account(&line, &[], host, "new"), ("new".into(), LineMode::Added));
    assert_eq!(pick_account(&line, &[known("a", "1002", "regular-test.ariaroute.com", Some(host))], host, "new"), ("new".into(), LineMode::Added));
    assert_eq!(pick_account(&line, std::slice::from_ref(&by_hand), host, "new"), ("new".into(), LineMode::Separate));
    assert_eq!(pick_account(&line, std::slice::from_ref(&other_host), host, "new"), ("new".into(), LineMode::Separate));
    assert_eq!(pick_account(&line, &[other_line, by_hand, other_host, own], host, "new"), ("b".into(), LineMode::Updated));
}

#[tokio::test]
async fn a_redeem_for_a_different_line_than_previewed_is_refused() {
    // Previewed 1001@regular-test; the POST tries to return someone else's line.
    let stub = Stub::json("200 OK", &redeem_body()).await;
    let accounts = [known("real", "1001", "regular-test.ariaroute.com", None)];
    for exp in [expected("1002", "regular-test.ariaroute.com"), expected("1001", "other.ariaroute.com")] {
        let r = redeem_checked(&stub.url(), &exp, &accounts, "new", SHORT).await;
        // Err means the command never reaches the credential store.
        assert_eq!(r.map(|(l, _)| l), Err(ProvisionError::Mismatch));
    }
}

#[tokio::test]
async fn a_redeem_matching_a_hand_made_account_adds_a_separate_one() {
    let stub = Stub::json("200 OK", &redeem_body()).await;
    let accounts = [known("real", "1001", "regular-test.ariaroute.com", None)];
    let (line, password) =
        redeem_checked(&stub.url(), &expected("1001", "Regular-Test.ariaroute.com"), &accounts, "new-id", SHORT)
            .await
            .unwrap();
    assert_eq!(line.account_id, "new-id");
    assert_eq!(line.mode, LineMode::Separate);
    assert_eq!(line.provisioned_from, format!("127.0.0.1:{}", stub.port));
    assert_eq!(password.0, "s3cret-pw");
    assert_eq!(format!("{password:?}"), "Password(..)");
}

#[tokio::test]
async fn a_redeem_from_the_same_host_updates_its_own_account() {
    let stub = Stub::json("200 OK", &redeem_body()).await;
    let host = format!("127.0.0.1:{}", stub.port);
    let accounts = [known("mine", "1001", "regular-test.ariaroute.com", Some(&host))];
    let (line, _) =
        redeem_checked(&stub.url(), &expected("1001", "regular-test.ariaroute.com"), &accounts, "new-id", SHORT)
            .await
            .unwrap();
    assert_eq!((line.account_id.as_str(), line.mode), ("mine", LineMode::Updated));
}

#[test]
fn display_text_cannot_break_a_quoted_string() {
    assert_eq!(clean_text("Front \"Desk\" <x>\\\r\n", 100), "Front Desk x");
    let mut body = redeem_body();
    body["display_name"] = json!("A\"B\\C<D>");
    assert_eq!(parse_redeem(&obj(body)).unwrap().display_name, "ABCD");
}

#[tokio::test]
async fn a_new_account_id_that_is_already_taken_is_refused_before_redeeming() {
    let stub = Stub::json("200 OK", &redeem_body()).await;
    let accounts = [known("taken", "2002", "elsewhere.example.com", None)];
    let r = redeem_checked(&stub.url(), &expected("1001", "regular-test.ariaroute.com"), &accounts, "taken", SHORT).await;
    assert_eq!(r.map(|(l, _)| l), Err(ProvisionError::AccountIdInUse));
    assert!(stub.methods().is_empty(), "the link must not be used up");
}
