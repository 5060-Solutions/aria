//! Config, token and end-to-end tests for the MCP server.
//!
//! The integration tests bind a real listener on 127.0.0.1 with a random
//! port and speak MCP over HTTP with `reqwest`, against a fresh
//! `SipManager` that has no accounts and no audio devices. Nothing here
//! touches the keychain.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::RwLock;

use super::consent::{ConsentRegistry, Decision};
use super::tools::{CallRequest, CallRequestSink, ToolContext, ToolSettings};
use super::*;
use crate::sip::diagnostics::{DiagnosticLog, MessageDirection};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aria-mcp-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ── Config ──────────────────────────────────────────────────────────────────

#[test]
fn config_defaults_are_all_off() {
    let config = McpConfig::default();
    assert!(!config.enabled);
    assert!(!config.allow_call_control);
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(DEFAULT_PORT, 48060);
}

#[test]
fn missing_config_file_loads_defaults() {
    let dir = temp_dir("missing");
    assert_eq!(McpConfig::load(&dir.join("mcp.json")), McpConfig::default());
}

#[test]
fn config_round_trips_through_the_file() {
    let dir = temp_dir("roundtrip");
    let path = dir.join("mcp.json");
    let config = McpConfig {
        enabled: true,
        port: 50123,
        allow_call_control: true,
    };
    config.save(&path).unwrap();
    assert_eq!(McpConfig::load(&path), config);

    // The file uses the documented snake_case keys.
    let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(raw["enabled"], json!(true));
    assert_eq!(raw["port"], json!(50123));
    assert_eq!(raw["allow_call_control"], json!(true));
}

#[test]
fn partial_or_broken_config_falls_back_safely() {
    let dir = temp_dir("partial");
    let path = dir.join("mcp.json");
    std::fs::write(&path, r#"{ "port": 1234 }"#).unwrap();
    let config = McpConfig::load(&path);
    assert_eq!(config.port, 1234);
    assert!(!config.enabled);

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(McpConfig::load(&path), McpConfig::default());
}

#[test]
fn port_zero_is_refused_from_settings() {
    let config = McpConfig {
        port: 0,
        ..Default::default()
    };
    assert!(config.validate().is_err());
    assert!(McpConfig::default().validate().is_ok());
}

// ── Token ───────────────────────────────────────────────────────────────────

#[test]
fn tokens_are_32_random_bytes_base64url() {
    let a = generate_token();
    let b = generate_token();
    assert_ne!(a, b);
    // 32 bytes → 43 chars without padding.
    assert_eq!(a.len(), 43);
    assert!(a
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&a)
        .unwrap();
    assert_eq!(decoded.len(), 32);
}

#[test]
fn constant_time_eq_compares_whole_values() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(!constant_time_eq(b"", b"a"));
}

#[cfg(feature = "dev-insecure")]
#[tokio::test]
async fn dev_token_store_round_trips() {
    let store = TokenStore::new(temp_dir("token"));
    assert_eq!(store.read().await.unwrap(), None);
    let token = store.ensure().await.unwrap();
    assert_eq!(store.read().await.unwrap(), Some(token.clone()));
    assert_eq!(store.ensure().await.unwrap(), token);
}

// ── End to end over HTTP ────────────────────────────────────────────────────

/// Records what the tools asked the user, instead of showing a dialog.
#[derive(Default)]
struct RecordingSink {
    requests: Mutex<Vec<CallRequest>>,
}

impl CallRequestSink for RecordingSink {
    fn request(&self, request: CallRequest) {
        self.requests.lock().unwrap().push(request);
    }
}

struct Harness {
    client: reqwest::Client,
    url: String,
    token: String,
    manager: SipManager,
    settings: Arc<RwLock<ToolSettings>>,
    consent: Arc<ConsentRegistry>,
    sink: Arc<RecordingSink>,
    server: Option<ServerHandle>,
    // Keeps the event channel open; the manager logs a warning otherwise.
    _events: tokio::sync::mpsc::UnboundedReceiver<crate::sip::SipEvent>,
}

impl Harness {
    async fn start() -> Self {
        let (manager, events) = SipManager::new_with_receiver();
        let settings = Arc::new(RwLock::new(ToolSettings::default()));
        let consent = Arc::new(ConsentRegistry::new());
        let sink = Arc::new(RecordingSink::default());
        let ctx = ToolContext {
            manager: manager.clone(),
            consent: consent.clone(),
            settings: settings.clone(),
            requests: sink.clone(),
        };
        let token = generate_token();
        let shared = Arc::new(RwLock::new(token.clone()));
        let server = start_server("127.0.0.1:0".parse().unwrap(), shared, ctx)
            .await
            .expect("server binds a random port");
        let url = format!("http://{}{}", server.addr, MCP_PATH);
        Self {
            client: reqwest::Client::new(),
            url,
            token,
            manager,
            settings,
            consent,
            sink,
            server: Some(server),
            _events: events,
        }
    }

    fn request(&self, body: Value) -> reqwest::RequestBuilder {
        self.client
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(&body)
    }

    async fn rpc(&self, id: u64, method: &str, params: Value) -> Value {
        let response = self
            .request(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .bearer_auth(&self.token)
            .send()
            .await
            .expect("request reaches the server");
        assert_eq!(
            response.status(),
            200,
            "{method} failed: {}",
            response.text().await.unwrap_or_default()
        );
        response.json().await.expect("JSON-RPC reply")
    }

    async fn call_tool(&self, id: u64, name: &str, arguments: Value) -> Value {
        self.rpc(id, "tools/call", json!({ "name": name, "arguments": arguments }))
            .await
    }

    async fn stop(mut self) {
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": "2025-03-26",
        "capabilities": {},
        "clientInfo": { "name": "aria-test", "version": "0" }
    })
}

/// `structuredContent` of a successful tools/call reply.
fn structured(reply: &Value) -> &Value {
    assert!(
        reply.get("error").is_none(),
        "tool call failed: {}",
        reply["error"]
    );
    let result = &reply["result"];
    assert_ne!(result["isError"], json!(true), "tool reported an error: {result}");
    &result["structuredContent"]
}

#[tokio::test]
async fn requests_without_the_bearer_are_rejected() {
    let h = Harness::start().await;

    let no_header = h
        .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize_params() }))
        .send()
        .await
        .unwrap();
    assert_eq!(no_header.status(), 401);
    assert!(no_header.headers().contains_key("www-authenticate"));

    let wrong = h
        .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize_params() }))
        .bearer_auth("not-the-token")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 401);

    h.stop().await;
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let h = Harness::start().await;

    let init = h.rpc(1, "initialize", initialize_params()).await;
    assert_eq!(init["result"]["serverInfo"]["name"], json!("aria"));
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("softphone"));

    let listed = h.rpc(2, "tools/list", json!({})).await;
    let mut names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "answer_call",
            "explain_last_failure",
            "get_sip_trace",
            "get_status",
            "hangup_call",
            "hold_call",
            "list_accounts",
            "list_calls",
            "list_recordings",
            "place_call",
            "send_dtmf",
            "transfer_call",
        ]
    );
    // Schemas come from schemars: the trace tool documents its filters.
    let trace = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "get_sip_trace")
        .unwrap();
    assert!(trace["inputSchema"]["properties"]["limit"].is_object());
    assert!(trace["inputSchema"]["properties"]["account_id"].is_object());
    assert!(trace["inputSchema"]["properties"]["call_id"].is_object());

    h.stop().await;
}

#[tokio::test]
async fn get_status_reports_version_and_no_accounts() {
    let h = Harness::start().await;
    let reply = h.call_tool(1, "get_status", json!({})).await;
    let status = structured(&reply);
    assert_eq!(status["app_version"], json!(env!("CARGO_PKG_VERSION")));
    assert_eq!(status["call_control_enabled"], json!(false));
    assert_eq!(status["accounts"], json!([]));
    assert_eq!(status["active_calls"], json!([]));
    // The text block is there for clients that ignore structured content.
    assert!(reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("app_version"));
    h.stop().await;
}

#[tokio::test]
async fn sip_trace_is_redacted_filtered_and_limited() {
    let h = Harness::start().await;
    // The manager has no transport, so feed the store directly through the
    // app-event path and a hand-made entry with a digest response in it.
    h.manager.log_app_event("acct-1", "hello", "detail").await;
    let logs = h.manager.get_diagnostics().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].summary, "hello");

    // get_sip_trace must redact even if a raw entry ever reached the store.
    let raw = "REGISTER sip:pbx SIP/2.0\r\nCall-ID: c1\r\nCSeq: 2 REGISTER\r\nAuthorization: Digest username=\"u\", response=\"deadbeef\"\r\n\r\n";
    let mut entry = DiagnosticLog::app_event("acct-1", "REGISTER sip:pbx SIP/2.0", raw);
    entry.direction = MessageDirection::Sent;
    entry.call_id = Some("c1".into());
    // Push through the same store the tool reads.
    h.manager.log_app_event("acct-2", "other account", "x").await;
    push_raw(&h.manager, entry).await;

    let reply = h.call_tool(1, "get_sip_trace", json!({})).await;
    let trace = structured(&reply);
    assert_eq!(trace["total_in_store"], json!(3));
    assert_eq!(trace["returned"], json!(3));
    let messages = trace["messages"].as_array().unwrap();
    let last = &messages[2];
    assert_eq!(last["direction"], json!("sent"));
    assert_eq!(last["sip_call_id"], json!("c1"));
    assert_eq!(last["first_line"], json!("REGISTER sip:pbx SIP/2.0"));
    let text = last["message"].as_str().unwrap();
    assert!(!text.contains("deadbeef"));
    assert!(text.contains("response=\"REDACTED\""));

    let filtered = h
        .call_tool(2, "get_sip_trace", json!({ "account_id": "acct-2" }))
        .await;
    assert_eq!(structured(&filtered)["returned"], json!(1));

    let limited = h.call_tool(3, "get_sip_trace", json!({ "limit": 1 })).await;
    let limited = structured(&limited);
    assert_eq!(limited["returned"], json!(1));
    assert_eq!(limited["messages"][0]["sip_call_id"], json!("c1"));

    let by_call = h
        .call_tool(4, "get_sip_trace", json!({ "call_id": "c1" }))
        .await;
    assert_eq!(structured(&by_call)["returned"], json!(1));

    h.stop().await;
}

/// Put an entry into the manager's store as the transport would.
async fn push_raw(manager: &SipManager, entry: DiagnosticLog) {
    manager.push_diagnostic_for_test(entry).await;
}

#[tokio::test]
async fn explain_last_failure_reads_the_trace() {
    let h = Harness::start().await;
    let mut req = DiagnosticLog::app_event(
        "acct-1",
        "REGISTER sip:pbx SIP/2.0",
        "REGISTER sip:pbx SIP/2.0\r\nCall-ID: r1\r\nCSeq: 1 REGISTER\r\n\r\n",
    );
    req.direction = MessageDirection::Sent;
    req.call_id = Some("r1".into());
    push_raw(&h.manager, req).await;
    for cseq in [1, 2] {
        let mut rsp = DiagnosticLog::app_event(
            "acct-1",
            "SIP/2.0 401 Unauthorized",
            &format!("SIP/2.0 401 Unauthorized\r\nCall-ID: r1\r\nCSeq: {cseq} REGISTER\r\n\r\n"),
        );
        rsp.direction = MessageDirection::Received;
        rsp.call_id = Some("r1".into());
        push_raw(&h.manager, rsp).await;
    }

    let reply = h.call_tool(1, "explain_last_failure", json!({})).await;
    let failure = structured(&reply);
    assert_eq!(failure["found"], json!(true));
    assert_eq!(failure["kind"], json!("registration"));
    assert_eq!(failure["status_code"], json!(401));
    assert_eq!(failure["reason"], json!("Unauthorized"));
    assert_eq!(failure["challenges_seen"], json!(2));
    assert_eq!(failure["request_line"], json!("REGISTER sip:pbx SIP/2.0"));
    assert!(failure["likely_cause"]
        .as_str()
        .unwrap()
        .contains("Credentials"));
    assert!(failure["hints"].as_array().unwrap().len() >= 6);

    let none = h
        .call_tool(2, "explain_last_failure", json!({ "account_id": "someone-else" }))
        .await;
    assert_eq!(structured(&none)["found"], json!(false));

    h.stop().await;
}

#[tokio::test]
async fn call_control_is_refused_until_enabled() {
    let h = Harness::start().await;

    for (name, args) in [
        ("place_call", json!({ "number": "100" })),
        ("hangup_call", json!({})),
        ("answer_call", json!({})),
        ("send_dtmf", json!({ "digits": "1" })),
        ("hold_call", json!({ "call_id": "x", "held": true })),
        ("transfer_call", json!({ "call_id": "x", "target": "200" })),
    ] {
        let reply = h.call_tool(1, name, args).await;
        let error = reply
            .get("error")
            .unwrap_or_else(|| panic!("{name} should be refused: {reply}"));
        assert!(
            error["message"].as_str().unwrap().contains("Settings"),
            "{name}: {error}"
        );
    }
    assert!(h.sink.requests.lock().unwrap().is_empty());
    // Refusals are not logged as actions: nothing was attempted.
    assert!(h.manager.get_diagnostics().await.is_empty());

    h.stop().await;
}

#[tokio::test]
async fn place_call_asks_the_user_and_returns_the_call_id() {
    let h = Harness::start().await;
    h.settings.write().await.allow_call_control = true;

    // Answer the dialog as the frontend would, once it has been shown.
    let consent = h.consent.clone();
    let sink = h.sink.clone();
    let answerer = tokio::spawn(async move {
        for _ in 0..200 {
            let pending = sink.requests.lock().unwrap().first().cloned();
            if let Some(req) = pending {
                assert_eq!(req.number, "100");
                assert_eq!(req.account_id.as_deref(), Some("acct-1"));
                consent
                    .decide(&req.request_id, Decision::allowed("call-42"))
                    .unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no call request was shown");
    });

    let reply = h
        .call_tool(1, "place_call", json!({ "number": "100", "account_id": "acct-1" }))
        .await;
    answerer.await.unwrap();
    let placed = structured(&reply);
    assert_eq!(placed["call_id"], json!("call-42"));
    assert_eq!(placed["number"], json!("100"));

    // The action is in the diagnostics trace as an app-level entry.
    let logs = h.manager.get_diagnostics().await;
    assert!(logs.iter().any(|l| {
        matches!(l.direction, MessageDirection::App) && l.summary.contains("allowed")
    }));

    h.stop().await;
}

#[tokio::test]
async fn denied_place_call_is_an_error_and_is_logged() {
    let h = Harness::start().await;
    h.settings.write().await.allow_call_control = true;

    let consent = h.consent.clone();
    let sink = h.sink.clone();
    tokio::spawn(async move {
        loop {
            let pending = sink.requests.lock().unwrap().first().cloned();
            if let Some(req) = pending {
                consent
                    .decide(&req.request_id, Decision::denied("denied"))
                    .unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let reply = h.call_tool(1, "place_call", json!({ "number": "100" })).await;
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("denied"));
    let logs = h.manager.get_diagnostics().await;
    assert!(logs
        .iter()
        .any(|l| matches!(l.direction, MessageDirection::App) && l.summary.contains("denied")));

    // Header injection in the number is refused before anyone is asked.
    let bad = h
        .call_tool(2, "place_call", json!({ "number": "100\r\nVia: x" }))
        .await;
    assert!(bad.get("error").is_some());
    assert_eq!(h.sink.requests.lock().unwrap().len(), 1);

    h.stop().await;
}

#[tokio::test]
async fn control_tools_report_missing_calls() {
    let h = Harness::start().await;
    h.settings.write().await.allow_call_control = true;

    let reply = h.call_tool(1, "hangup_call", json!({})).await;
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("No call"));
    let reply = h.call_tool(2, "send_dtmf", json!({ "digits": "1x" })).await;
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not a DTMF digit"));
    let reply = h
        .call_tool(3, "hold_call", json!({ "call_id": "nope", "held": true }))
        .await;
    assert!(reply["error"]["message"]
        .as_str()
        .unwrap()
        .contains("No active call"));

    h.stop().await;
}

#[tokio::test]
async fn list_accounts_and_recordings_are_empty_on_a_fresh_manager() {
    let h = Harness::start().await;
    let accounts = h.call_tool(1, "list_accounts", json!({})).await;
    assert_eq!(structured(&accounts)["accounts"], json!([]));
    let calls = h.call_tool(2, "list_calls", json!({})).await;
    assert_eq!(structured(&calls)["calls"], json!([]));

    let dir = temp_dir("recordings");
    std::fs::write(dir.join("a.wav"), b"RIFF").unwrap();
    h.manager.set_recordings_dir(dir.clone()).await;
    let recordings = h.call_tool(3, "list_recordings", json!({})).await;
    let recordings = structured(&recordings);
    assert_eq!(recordings["recordings"][0]["file_name"], json!("a.wav"));
    assert_eq!(recordings["recordings"][0]["size_bytes"], json!(4));
    assert!(recordings["directory"]
        .as_str()
        .unwrap()
        .ends_with(dir.file_name().unwrap().to_str().unwrap()));

    h.stop().await;
}

#[tokio::test]
async fn stopping_the_server_frees_the_port() {
    let h = Harness::start().await;
    let addr = h.server.as_ref().unwrap().addr;
    h.stop().await;
    // The port is free again: a second listener can take it.
    let listener = tokio::net::TcpListener::bind(addr).await;
    assert!(listener.is_ok(), "port {addr} still held after stop");
}
