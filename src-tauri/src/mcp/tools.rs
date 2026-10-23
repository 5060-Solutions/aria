//! The MCP tools: what an assistant can see and do.
//!
//! Read-only tools work whenever the server is enabled. Call-control tools
//! also need "allow assistants to place and control calls" in Settings, and
//! `place_call` still asks the user for each call. Every control action,
//! allowed or not, is written to the diagnostics trace.
//!
//! Nothing here returns a credential: the SIP trace goes through
//! `diagnostics::redact_sip`, and accounts are listed from `AccountSummary`,
//! which has no password field to leak.

use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{ErrorCode, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::consent::{ConsentRegistry, Decision, CONSENT_TIMEOUT};
use crate::sip::diagnostics::{self, DiagnosticLog, MessageDirection};
use crate::sip::{CallStatusInfo, SipManager};

/// A call an assistant wants placed. Shown to the user before anything is
/// dialled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallRequest {
    pub request_id: String,
    pub number: String,
    pub account_id: Option<String>,
}

/// Where a `CallRequest` goes to be shown to the user. In the app this emits
/// the `mcp-call-request` event to the webview; tests substitute their own.
pub trait CallRequestSink: Send + Sync {
    fn request(&self, request: CallRequest);
}

/// The switches the tools consult on every call, shared with the Settings
/// commands so a change applies without restarting the server.
#[derive(Debug, Clone, Default)]
pub struct ToolSettings {
    pub allow_call_control: bool,
}

/// Everything a tool call needs. Cheap to clone: the server builds one
/// handler per request.
#[derive(Clone)]
pub struct ToolContext {
    pub manager: SipManager,
    pub consent: Arc<ConsentRegistry>,
    pub settings: Arc<RwLock<ToolSettings>>,
    pub requests: Arc<dyn CallRequestSink>,
}

const DEFAULT_TRACE_LIMIT: usize = 50;
const MAX_TRACE_LIMIT: usize = 500;

// ── Parameters ──────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SipTraceParams {
    /// Only messages for this account.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Only messages with this SIP Call-ID header.
    #[serde(default)]
    pub call_id: Option<String>,
    /// How many of the most recent messages to return (default 50, max 500).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct AccountParams {
    /// Only this account. Defaults to all accounts.
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PlaceCallParams {
    /// Number, extension or SIP address to call. The account's dialing rules
    /// are applied as if it had been typed in the dialer.
    pub number: String,
    /// Account to call from. Defaults to the active account.
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CallParams {
    /// The call (Aria call id from list_calls). Defaults to the only
    /// eligible call when there is exactly one.
    #[serde(default)]
    pub call_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DtmfParams {
    /// The call. Defaults to the only connected call.
    #[serde(default)]
    pub call_id: Option<String>,
    /// Digits to send in order: 0-9, *, #, A-D.
    pub digits: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HoldParams {
    pub call_id: String,
    /// true to put the call on hold, false to resume it.
    pub held: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransferParams {
    pub call_id: String,
    /// Number or SIP address to transfer the call to (blind transfer).
    pub target: String,
}

// ── Results ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccountInfo {
    pub account_id: String,
    pub display_name: String,
    pub username: String,
    pub domain: String,
    pub transport: String,
    pub port: u16,
    pub registrar: Option<String>,
    pub registration_state: String,
    pub registration_error: Option<String>,
    pub srtp_mode: String,
    pub is_active: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CallInfo {
    pub call_id: String,
    pub account_id: String,
    pub remote: String,
    pub direction: String,
    pub state: String,
    pub duration_secs: Option<u64>,
    pub codec: Option<String>,
    pub srtp_active: bool,
    pub sip_call_id: String,
    /// Packet and jitter counters, when media is up.
    pub rtp_stats: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatusResult {
    pub app_version: String,
    pub call_control_enabled: bool,
    pub latency_ms: Option<f64>,
    pub accounts: Vec<AccountInfo>,
    pub active_calls: Vec<CallInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AccountsResult {
    pub accounts: Vec<AccountInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CallsResult {
    pub calls: Vec<CallInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TraceEntry {
    pub timestamp_ms: u64,
    /// "sent", "received" or "app" (something Aria did, not a SIP message).
    pub direction: String,
    pub account_id: String,
    pub peer: String,
    pub sip_call_id: Option<String>,
    pub first_line: String,
    /// The whole message, with digest responses and SRTP keys redacted.
    pub message: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TraceResult {
    pub total_in_store: usize,
    pub returned: usize,
    pub messages: Vec<TraceEntry>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecordingInfo {
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
    pub modified_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RecordingsResult {
    pub directory: Option<String>,
    pub recordings: Vec<RecordingInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FailureHint {
    /// SIP status codes or the condition the hint applies to.
    pub matches: String,
    pub likely_cause: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RegistrationSnapshot {
    pub account_id: String,
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FailureResult {
    pub found: bool,
    /// "registration", "call" or "other", from the request that failed.
    pub kind: Option<String>,
    pub status_code: Option<u16>,
    pub reason: Option<String>,
    pub method: Option<String>,
    pub request_line: Option<String>,
    pub account_id: Option<String>,
    pub peer: Option<String>,
    pub sip_call_id: Option<String>,
    pub timestamp_ms: Option<u64>,
    /// How many 401/407 challenges that dialog received.
    pub challenges_seen: u32,
    /// The hint that matches this failure, if any.
    pub likely_cause: Option<String>,
    /// The last redacted response and the request it answered, when found.
    pub response: Option<String>,
    pub request: Option<String>,
    pub registration: Vec<RegistrationSnapshot>,
    pub hints: Vec<FailureHint>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PlaceCallResult {
    pub call_id: String,
    pub number: String,
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ActionResult {
    pub call_id: String,
    pub action: String,
    pub ok: bool,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn call_info(c: CallStatusInfo) -> CallInfo {
    CallInfo {
        call_id: c.id,
        account_id: c.account_id,
        remote: c.remote_uri,
        direction: c.direction,
        state: c.state,
        duration_secs: c.duration_secs,
        codec: c.codec,
        srtp_active: c.srtp_active,
        sip_call_id: c.sip_call_id,
        rtp_stats: c.rtp_stats.and_then(|s| serde_json::to_value(s).ok()),
    }
}

async fn account_infos(manager: &SipManager) -> Vec<AccountInfo> {
    manager
        .account_summaries()
        .await
        .into_iter()
        .map(|a| AccountInfo {
            account_id: a.account_id,
            display_name: a.display_name,
            username: a.username,
            domain: a.domain,
            transport: a.transport,
            port: a.port,
            registrar: a.registrar,
            registration_state: a.registration_state,
            registration_error: a.registration_error,
            srtp_mode: a.srtp_mode,
            is_active: a.is_active,
        })
        .collect()
}

async fn active_calls(manager: &SipManager) -> Vec<CallInfo> {
    manager
        .get_system_status()
        .await
        .accounts
        .into_iter()
        .flat_map(|a| a.active_calls)
        .map(call_info)
        .collect()
}

fn direction_name(d: &MessageDirection) -> &'static str {
    match d {
        MessageDirection::Sent => "sent",
        MessageDirection::Received => "received",
        MessageDirection::App => "app",
    }
}

/// `(code, reason)` from a response's status line.
fn status_line(raw: &str) -> Option<(u16, String)> {
    let first = raw.lines().next()?;
    let rest = first.strip_prefix("SIP/2.0 ")?;
    let (code, reason) = rest.split_once(' ').unwrap_or((rest, ""));
    Some((code.parse().ok()?, reason.trim().to_string()))
}

/// The value of header `name`, matched case-insensitively.
fn header(raw: &str, name: &str) -> Option<String> {
    raw.lines().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
    })
}

/// The method named in a CSeq header ("CSeq: 2 REGISTER" → "REGISTER").
fn cseq_method(raw: &str) -> Option<String> {
    header(raw, "CSeq")?
        .split_whitespace()
        .nth(1)
        .map(|m| m.to_ascii_uppercase())
}

/// The static table: what each failing status usually means, from the
/// phone's point of view. Data for the assistant, not advice to the user.
pub fn failure_hints() -> Vec<FailureHint> {
    let rows = [
        ("401, 407 (repeated for one request)", "Credentials rejected: wrong password, username or auth username, or an auth realm that does not match what the server verifies against."),
        ("401, 407 (single, then success)", "Normal digest challenge; not a failure."),
        ("403", "Authentication or policy refusal: wrong password or realm, account disabled, or the PBX's IP ACL / fail2ban blocking this address."),
        ("404", "User or domain unknown to the server: check the username and the domain or registrar."),
        ("408, timeout, no response", "Nothing answered: wrong server address or port, wrong transport (UDP/TCP/TLS), or a firewall or NAT dropping the packets."),
        ("480, 503", "Server-side unavailability: the PBX or trunk is down, overloaded or rejecting registrations right now."),
        ("486", "The far end is busy."),
        ("487", "The request was cancelled before it completed."),
        ("488", "No usable media: codec or SRTP settings do not match what the far end accepts."),
        ("423", "Registration interval too brief: the server wants a longer Expires."),
        ("500, 502", "The server hit an internal error; retrying later usually works, otherwise it is a PBX problem."),
        ("603", "The call was declined by the far end."),
    ];
    rows.iter()
        .map(|(m, c)| FailureHint {
            matches: m.to_string(),
            likely_cause: c.to_string(),
        })
        .collect()
}

fn likely_cause(code: u16, challenges: u32) -> Option<String> {
    let text = match code {
        401 | 407 if challenges >= 2 => "Credentials rejected: wrong password, username or auth username, or an auth realm that does not match what the server verifies against.",
        401 | 407 => "The challenge was never answered with credentials, or the retry is still pending.",
        403 => "Authentication or policy refusal: wrong password or realm, account disabled, or the PBX's IP ACL / fail2ban blocking this address.",
        404 => "User or domain unknown to the server: check the username and the domain or registrar.",
        408 => "Nothing answered: wrong server address or port, wrong transport (UDP/TCP/TLS), or a firewall or NAT dropping the packets.",
        480 | 503 => "Server-side unavailability: the PBX or trunk is down, overloaded or rejecting registrations right now.",
        486 => "The far end is busy.",
        487 => "The request was cancelled before it completed.",
        488 => "No usable media: codec or SRTP settings do not match what the far end accepts.",
        423 => "Registration interval too brief: the server wants a longer Expires.",
        500 | 502 => "The server hit an internal error; retrying later usually works, otherwise it is a PBX problem.",
        603 => "The call was declined by the far end.",
        _ => return None,
    };
    Some(text.to_string())
}

fn kind_for_method(method: Option<&str>) -> Option<String> {
    Some(
        match method? {
            "REGISTER" => "registration",
            "INVITE" | "BYE" | "CANCEL" | "REFER" | "ACK" | "UPDATE" => "call",
            _ => "other",
        }
        .to_string(),
    )
}

/// The most recent failure in `logs` (oldest first), with its request.
///
/// A 401/407 that the next request answered and got a 2xx for is a normal
/// handshake, not a failure, so those are skipped; a challenge that was never
/// resolved counts.
pub fn find_last_failure(logs: &[DiagnosticLog]) -> Option<(usize, u16, String, u32)> {
    let mut resolved: std::collections::HashSet<String> = Default::default();
    for (idx, log) in logs.iter().enumerate().rev() {
        if !matches!(log.direction, MessageDirection::Received) {
            continue;
        }
        let Some((code, reason)) = status_line(&log.raw) else {
            continue;
        };
        let call_id = log.call_id.clone().unwrap_or_default();
        if (200..300).contains(&code) {
            resolved.insert(call_id);
            continue;
        }
        if code < 300 {
            continue;
        }
        if matches!(code, 401 | 407) && resolved.contains(&call_id) {
            continue;
        }
        let challenges = logs
            .iter()
            .filter(|l| {
                matches!(l.direction, MessageDirection::Received)
                    && l.call_id.as_deref().unwrap_or_default() == call_id
                    && status_line(&l.raw).is_some_and(|(c, _)| matches!(c, 401 | 407))
            })
            .count() as u32;
        return Some((idx, code, reason, challenges));
    }
    None
}

fn control_disabled() -> ErrorData {
    ErrorData::new(
        ErrorCode::INVALID_REQUEST,
        "Call control over MCP is disabled. Turn on \"Allow assistants to place and control calls\" under Settings → Assistants (MCP) in Aria.",
        None,
    )
}

fn tool_error(message: impl Into<String>) -> ErrorData {
    ErrorData::new(ErrorCode::INVALID_PARAMS, message.into(), None)
}

/// Pick the call a control tool should act on when the caller named none.
fn pick_call(
    calls: &[CallInfo],
    explicit: Option<&str>,
    eligible: impl Fn(&CallInfo) -> bool,
    what: &str,
) -> Result<CallInfo, ErrorData> {
    if let Some(id) = explicit {
        return calls
            .iter()
            .find(|c| c.call_id == id)
            .map(clone_call)
            .ok_or_else(|| tool_error(format!("No active call with id {id}")));
    }
    let mut candidates = calls.iter().filter(|c| eligible(c));
    match (candidates.next(), candidates.next()) {
        (Some(only), None) => Ok(clone_call(only)),
        (None, _) => Err(tool_error(format!("No call to {what}"))),
        (Some(_), Some(_)) => Err(tool_error(format!(
            "More than one call could be {what}; pass call_id (see list_calls)"
        ))),
    }
}

fn clone_call(c: &CallInfo) -> CallInfo {
    CallInfo {
        call_id: c.call_id.clone(),
        account_id: c.account_id.clone(),
        remote: c.remote.clone(),
        direction: c.direction.clone(),
        state: c.state.clone(),
        duration_secs: c.duration_secs,
        codec: c.codec.clone(),
        srtp_active: c.srtp_active,
        sip_call_id: c.sip_call_id.clone(),
        rtp_stats: c.rtp_stats.clone(),
    }
}

// ── The server ──────────────────────────────────────────────────────────────

/// One MCP server handler. The streamable HTTP transport makes one per
/// request from the shared `ToolContext`.
#[derive(Clone)]
pub struct AriaMcp {
    ctx: ToolContext,
    tool_router: ToolRouter<Self>,
}

impl AriaMcp {
    pub fn new(ctx: ToolContext) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }

    async fn require_call_control(&self) -> Result<(), ErrorData> {
        if self.ctx.settings.read().await.allow_call_control {
            Ok(())
        } else {
            Err(control_disabled())
        }
    }

    /// Record a control action in the diagnostics trace.
    async fn audit(&self, account_id: &str, summary: &str, detail: &str) {
        self.ctx
            .manager
            .log_app_event(account_id, &format!("MCP: {summary}"), detail)
            .await;
    }

    async fn run_action(
        &self,
        call: &CallInfo,
        action: &str,
        result: Result<(), String>,
    ) -> Result<Json<ActionResult>, ErrorData> {
        match result {
            Ok(()) => {
                self.audit(
                    &call.account_id,
                    &format!("{action} {}", call.remote),
                    &format!("{action} call {} ({}) via MCP", call.call_id, call.remote),
                )
                .await;
                Ok(Json(ActionResult {
                    call_id: call.call_id.clone(),
                    action: action.to_string(),
                    ok: true,
                }))
            }
            Err(e) => {
                self.audit(
                    &call.account_id,
                    &format!("{action} {} failed", call.remote),
                    &format!("{action} call {} via MCP failed: {e}", call.call_id),
                )
                .await;
                Err(tool_error(format!("Could not {action}: {e}")))
            }
        }
    }
}

#[tool_router]
impl AriaMcp {
    /// Registration state of every account, the active calls and the app
    /// version. Start here.
    #[tool(name = "get_status", annotations(read_only_hint = true))]
    async fn get_status(&self) -> Result<Json<StatusResult>, ErrorData> {
        let status = self.ctx.manager.get_system_status().await;
        let active_calls = status
            .accounts
            .into_iter()
            .flat_map(|a| a.active_calls)
            .map(call_info)
            .collect();
        Ok(Json(StatusResult {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            call_control_enabled: self.ctx.settings.read().await.allow_call_control,
            latency_ms: status.latency_ms,
            accounts: account_infos(&self.ctx.manager).await,
            active_calls,
        }))
    }

    /// The configured SIP accounts: identity, server, transport, SRTP mode
    /// and registration state. Never includes passwords.
    #[tool(name = "list_accounts", annotations(read_only_hint = true))]
    async fn list_accounts(&self) -> Result<Json<AccountsResult>, ErrorData> {
        Ok(Json(AccountsResult {
            accounts: account_infos(&self.ctx.manager).await,
        }))
    }

    /// The calls in progress, with direction, state, duration, codec, whether
    /// SRTP is active, and RTP statistics when media is up.
    #[tool(name = "list_calls", annotations(read_only_hint = true))]
    async fn list_calls(&self) -> Result<Json<CallsResult>, ErrorData> {
        Ok(Json(CallsResult {
            calls: active_calls(&self.ctx.manager).await,
        }))
    }

    /// Recent SIP messages from the live trace, oldest first. Credentials
    /// (digest responses, SRTP keys) are redacted. Filter by account or SIP
    /// Call-ID; limit defaults to 50 and is capped at 500.
    #[tool(name = "get_sip_trace", annotations(read_only_hint = true))]
    async fn get_sip_trace(
        &self,
        Parameters(params): Parameters<SipTraceParams>,
    ) -> Result<Json<TraceResult>, ErrorData> {
        let limit = params
            .limit
            .unwrap_or(DEFAULT_TRACE_LIMIT)
            .clamp(1, MAX_TRACE_LIMIT);
        let logs = self.ctx.manager.get_diagnostics().await;
        let total_in_store = logs.len();
        let filtered: Vec<&DiagnosticLog> = logs
            .iter()
            .filter(|l| {
                params
                    .account_id
                    .as_deref()
                    .is_none_or(|a| l.account_id == a)
            })
            .filter(|l| {
                params
                    .call_id
                    .as_deref()
                    .is_none_or(|c| l.call_id.as_deref() == Some(c))
            })
            .collect();
        let start = filtered.len().saturating_sub(limit);
        let messages: Vec<TraceEntry> = filtered[start..]
            .iter()
            .map(|l| TraceEntry {
                timestamp_ms: l.timestamp,
                direction: direction_name(&l.direction).to_string(),
                account_id: l.account_id.clone(),
                peer: l.remote_addr.clone(),
                sip_call_id: l.call_id.clone(),
                first_line: l.summary.clone(),
                message: diagnostics::redact_sip(&l.raw),
            })
            .collect();
        Ok(Json(TraceResult {
            total_in_store,
            returned: messages.len(),
            messages,
        }))
    }

    /// Call recordings on disk, newest first, with size and date.
    #[tool(name = "list_recordings", annotations(read_only_hint = true))]
    async fn list_recordings(&self) -> Result<Json<RecordingsResult>, ErrorData> {
        let Some(dir) = self.ctx.manager.recordings_dir().await else {
            return Ok(Json(RecordingsResult {
                directory: None,
                recordings: vec![],
            }));
        };
        let mut recordings: Vec<RecordingInfo> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    if !meta.is_file() {
                        return None;
                    }
                    let modified_ms = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64);
                    Some(RecordingInfo {
                        file_name: e.file_name().to_string_lossy().into_owned(),
                        path: e.path().to_string_lossy().into_owned(),
                        size_bytes: meta.len(),
                        modified_ms,
                    })
                })
                .collect(),
            // No directory yet means no recordings, not an error.
            Err(_) => vec![],
        };
        recordings.sort_by_key(|r| std::cmp::Reverse(r.modified_ms));
        Ok(Json(RecordingsResult {
            directory: Some(dir.to_string_lossy().into_owned()),
            recordings,
        }))
    }

    /// The most recent registration or call failure: the final SIP status,
    /// the request it answered, the current registration state, and a table
    /// of what each status usually means.
    #[tool(name = "explain_last_failure", annotations(read_only_hint = true))]
    async fn explain_last_failure(
        &self,
        Parameters(params): Parameters<AccountParams>,
    ) -> Result<Json<FailureResult>, ErrorData> {
        let logs: Vec<DiagnosticLog> = self
            .ctx
            .manager
            .get_diagnostics()
            .await
            .into_iter()
            .filter(|l| {
                params
                    .account_id
                    .as_deref()
                    .is_none_or(|a| l.account_id == a)
            })
            .collect();
        let registration: Vec<RegistrationSnapshot> = self
            .ctx
            .manager
            .account_summaries()
            .await
            .into_iter()
            .filter(|a| {
                params
                    .account_id
                    .as_deref()
                    .is_none_or(|id| a.account_id == id)
            })
            .map(|a| RegistrationSnapshot {
                account_id: a.account_id,
                state: a.registration_state,
                error: a.registration_error,
            })
            .collect();

        let mut result = FailureResult {
            found: false,
            kind: None,
            status_code: None,
            reason: None,
            method: None,
            request_line: None,
            account_id: None,
            peer: None,
            sip_call_id: None,
            timestamp_ms: None,
            challenges_seen: 0,
            likely_cause: None,
            response: None,
            request: None,
            registration,
            hints: failure_hints(),
        };

        if let Some((idx, code, reason, challenges)) = find_last_failure(&logs) {
            let response = &logs[idx];
            let method = cseq_method(&response.raw);
            let request = logs[..idx].iter().rev().find(|l| {
                matches!(l.direction, MessageDirection::Sent)
                    && l.call_id == response.call_id
                    && cseq_method(&l.raw) == method
            });
            result.found = true;
            result.kind = kind_for_method(method.as_deref());
            result.status_code = Some(code);
            result.reason = Some(reason);
            result.method = method;
            result.request_line = request.map(|r| r.summary.clone());
            result.account_id = Some(response.account_id.clone());
            result.peer = Some(response.remote_addr.clone());
            result.sip_call_id = response.call_id.clone();
            result.timestamp_ms = Some(response.timestamp);
            result.challenges_seen = challenges;
            result.likely_cause = likely_cause(code, challenges);
            result.response = Some(diagnostics::redact_sip(&response.raw));
            result.request = request.map(|r| diagnostics::redact_sip(&r.raw));
        } else if let Some(reg) = result
            .registration
            .iter()
            .find(|r| r.state == "error" || r.state == "reconnecting")
        {
            // No failing response in the trace, but the stack gave up on
            // something — usually a timeout, which never gets a status line.
            result.found = true;
            result.kind = Some("registration".to_string());
            result.reason = reg.error.clone();
            result.account_id = Some(reg.account_id.clone());
            result.likely_cause = Some(
                "No reply from the server: wrong server address or port, wrong transport (UDP/TCP/TLS), or a firewall or NAT dropping the packets.".to_string(),
            );
        }

        Ok(Json(result))
    }

    /// Place a call. Aria asks the user to allow it first; the call is only
    /// dialled if they say yes within 30 seconds. Returns the new call id.
    #[tool(name = "place_call")]
    async fn place_call(
        &self,
        Parameters(params): Parameters<PlaceCallParams>,
    ) -> Result<Json<PlaceCallResult>, ErrorData> {
        self.require_call_control().await?;
        let number = crate::commands::checked_target(&params.number)
            .map_err(tool_error)?
            .to_string();
        let account_label = params.account_id.clone().unwrap_or_else(|| "aria".to_string());

        let (request_id, rx) = self.ctx.consent.begin();
        self.ctx.requests.request(CallRequest {
            request_id: request_id.clone(),
            number: number.clone(),
            account_id: params.account_id.clone(),
        });
        let decision: Decision = self.ctx.consent.wait(&request_id, rx, CONSENT_TIMEOUT).await;

        match decision {
            Decision {
                allowed: true,
                call_id: Some(call_id),
                ..
            } => {
                self.audit(
                    &account_label,
                    &format!("call {number} allowed"),
                    &format!("User allowed an assistant to call {number}; call {call_id}"),
                )
                .await;
                Ok(Json(PlaceCallResult {
                    call_id,
                    number,
                    account_id: params.account_id,
                }))
            }
            Decision { reason, .. } => {
                let reason = reason.unwrap_or_else(|| "denied".to_string());
                self.audit(
                    &account_label,
                    &format!("call {number} {reason}"),
                    &format!("An assistant asked to call {number}: {reason}"),
                )
                .await;
                Err(tool_error(match reason.as_str() {
                    "timeout" => "The user did not answer the call request in time".to_string(),
                    "denied" => "The user denied the call".to_string(),
                    other => format!("The call could not be placed: {other}"),
                }))
            }
        }
    }

    /// Answer an incoming call. Defaults to the one ringing call.
    #[tool(name = "answer_call")]
    async fn answer_call(
        &self,
        Parameters(params): Parameters<CallParams>,
    ) -> Result<Json<ActionResult>, ErrorData> {
        self.require_call_control().await?;
        let calls = active_calls(&self.ctx.manager).await;
        let call = pick_call(
            &calls,
            params.call_id.as_deref(),
            |c| c.state == "incoming",
            "answer",
        )?;
        let result = self.ctx.manager.answer(&call.call_id).await;
        self.run_action(&call, "answer", result).await
    }

    /// Hang up a call. Defaults to the only call in progress.
    #[tool(name = "hangup_call")]
    async fn hangup_call(
        &self,
        Parameters(params): Parameters<CallParams>,
    ) -> Result<Json<ActionResult>, ErrorData> {
        self.require_call_control().await?;
        let calls = active_calls(&self.ctx.manager).await;
        let call = pick_call(
            &calls,
            params.call_id.as_deref(),
            |c| c.state != "ended",
            "hang up",
        )?;
        let result = self.ctx.manager.hangup(&call.call_id).await;
        self.run_action(&call, "hangup", result).await
    }

    /// Send DTMF digits (0-9, *, #, A-D) on a connected call, in order.
    #[tool(name = "send_dtmf")]
    async fn send_dtmf(
        &self,
        Parameters(params): Parameters<DtmfParams>,
    ) -> Result<Json<ActionResult>, ErrorData> {
        self.require_call_control().await?;
        let digits: String = params.digits.chars().filter(|c| !c.is_whitespace()).collect();
        if digits.is_empty() {
            return Err(tool_error("digits must not be empty"));
        }
        if let Some(bad) = digits
            .chars()
            .find(|c| !matches!(c, '0'..='9' | '*' | '#' | 'A'..='D' | 'a'..='d'))
        {
            return Err(tool_error(format!("'{bad}' is not a DTMF digit")));
        }
        let calls = active_calls(&self.ctx.manager).await;
        let call = pick_call(
            &calls,
            params.call_id.as_deref(),
            |c| c.state == "connected",
            "send digits to",
        )?;
        let mut result = Ok(());
        for d in digits.chars() {
            result = self
                .ctx
                .manager
                .send_dtmf(&call.call_id, &d.to_ascii_uppercase().to_string())
                .await;
            if result.is_err() {
                break;
            }
            // Keep tones distinguishable at the far end.
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }
        self.run_action(&call, &format!("dtmf {digits}"), result).await
    }

    /// Put a call on hold, or resume it.
    #[tool(name = "hold_call")]
    async fn hold_call(
        &self,
        Parameters(params): Parameters<HoldParams>,
    ) -> Result<Json<ActionResult>, ErrorData> {
        self.require_call_control().await?;
        let calls = active_calls(&self.ctx.manager).await;
        let call = pick_call(&calls, Some(&params.call_id), |_| true, "hold")?;
        let result = self.ctx.manager.hold(&call.call_id, params.held).await;
        let action = if params.held { "hold" } else { "resume" };
        self.run_action(&call, action, result).await
    }

    /// Blind-transfer a call to a number or SIP address.
    #[tool(name = "transfer_call")]
    async fn transfer_call(
        &self,
        Parameters(params): Parameters<TransferParams>,
    ) -> Result<Json<ActionResult>, ErrorData> {
        self.require_call_control().await?;
        let target = crate::commands::checked_target(&params.target).map_err(tool_error)?;
        let calls = active_calls(&self.ctx.manager).await;
        let call = pick_call(&calls, Some(&params.call_id), |_| true, "transfer")?;
        let target_uri = if target.contains('@') {
            if target.starts_with("sip:") || target.starts_with("sips:") {
                target.to_string()
            } else {
                format!("sip:{target}")
            }
        } else {
            let domain = self
                .ctx
                .manager
                .account_summaries()
                .await
                .into_iter()
                .find(|a| a.account_id == call.account_id)
                .map(|a| a.domain)
                .ok_or_else(|| tool_error("The call's account is no longer configured"))?;
            format!("sip:{target}@{domain}")
        };
        let result = self.ctx.manager.transfer_blind(&call.call_id, &target_uri).await;
        self.run_action(&call, &format!("transfer to {target_uri}"), result)
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AriaMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "aria",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Aria is a SIP softphone running on this computer. Use get_status first. \
                 get_sip_trace and explain_last_failure answer \"why won't it register\" \
                 and \"why did the call fail\" from the live SIP trace (credentials are \
                 redacted). Call-control tools only work when the user has enabled them \
                 in Settings, and place_call always asks the user to allow each call.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received(call_id: &str, raw: &str) -> DiagnosticLog {
        DiagnosticLog {
            timestamp: 1,
            account_id: "a".into(),
            direction: MessageDirection::Received,
            remote_addr: "10.0.0.1:5060".into(),
            summary: raw.lines().next().unwrap_or("").into(),
            raw: raw.into(),
            call_id: Some(call_id.into()),
        }
    }

    fn sent(call_id: &str, raw: &str) -> DiagnosticLog {
        DiagnosticLog {
            direction: MessageDirection::Sent,
            ..received(call_id, raw)
        }
    }

    #[test]
    fn status_line_parses_code_and_reason() {
        assert_eq!(
            status_line("SIP/2.0 404 Not Found\r\nVia: x\r\n"),
            Some((404, "Not Found".into()))
        );
        assert_eq!(status_line("REGISTER sip:pbx SIP/2.0\r\n"), None);
    }

    #[test]
    fn cseq_method_is_upper_cased() {
        assert_eq!(
            cseq_method("SIP/2.0 200 OK\r\ncseq: 3 register\r\n"),
            Some("REGISTER".into())
        );
    }

    #[test]
    fn answered_challenge_is_not_a_failure() {
        let logs = vec![
            sent("c1", "REGISTER sip:pbx SIP/2.0\r\nCSeq: 1 REGISTER\r\n"),
            received("c1", "SIP/2.0 401 Unauthorized\r\nCSeq: 1 REGISTER\r\n"),
            sent("c1", "REGISTER sip:pbx SIP/2.0\r\nCSeq: 2 REGISTER\r\n"),
            received("c1", "SIP/2.0 200 OK\r\nCSeq: 2 REGISTER\r\n"),
        ];
        assert_eq!(find_last_failure(&logs), None);
    }

    #[test]
    fn repeated_challenges_are_a_credentials_failure() {
        let logs = vec![
            sent("c1", "REGISTER sip:pbx SIP/2.0\r\nCSeq: 1 REGISTER\r\n"),
            received("c1", "SIP/2.0 401 Unauthorized\r\nCSeq: 1 REGISTER\r\n"),
            sent("c1", "REGISTER sip:pbx SIP/2.0\r\nCSeq: 2 REGISTER\r\n"),
            received("c1", "SIP/2.0 401 Unauthorized\r\nCSeq: 2 REGISTER\r\n"),
        ];
        let (idx, code, _, challenges) = find_last_failure(&logs).unwrap();
        assert_eq!((idx, code, challenges), (3, 401, 2));
        assert!(likely_cause(code, challenges)
            .unwrap()
            .contains("Credentials rejected"));
    }

    #[test]
    fn latest_failure_wins_over_an_older_success() {
        let logs = vec![
            received("c1", "SIP/2.0 200 OK\r\nCSeq: 2 REGISTER\r\n"),
            sent("c2", "INVITE sip:100@pbx SIP/2.0\r\nCSeq: 1 INVITE\r\n"),
            received("c2", "SIP/2.0 404 Not Found\r\nCSeq: 1 INVITE\r\n"),
        ];
        let (idx, code, reason, _) = find_last_failure(&logs).unwrap();
        assert_eq!((idx, code, reason.as_str()), (2, 404, "Not Found"));
        assert_eq!(kind_for_method(Some("INVITE")).as_deref(), Some("call"));
    }

    #[test]
    fn hint_table_covers_the_documented_codes() {
        let hints = failure_hints();
        for code in ["401", "403", "404", "408", "480", "503"] {
            assert!(
                hints.iter().any(|h| h.matches.contains(code)),
                "no hint for {code}"
            );
        }
    }
}
