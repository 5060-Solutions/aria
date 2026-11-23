pub mod account;
pub mod auth;
pub mod builder;
pub mod codec;
pub mod connection;
pub mod diagnostics;
mod handlers;
#[allow(dead_code)]
pub mod ice;
mod emergency;
mod instance;
pub mod media;
pub mod mwi;
pub mod presence;
#[allow(dead_code)]
pub mod srtp;
pub mod srv;
pub mod state;
pub mod transfer;
pub mod transport;
#[cfg(test)]
mod voip_stack_harness;
#[cfg(test)]
mod pbx_e2e;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub use account::AccountConfig;
pub use transport::TransportType;

use self::builder::{
    build_200_ok_invite_srtp, build_bye_with_routes, build_cancel, build_options, build_refer,
    build_refer_with_replaces, build_register, build_subscribe, extract_header, extract_via_branch, is_request,
    parse_sdp_connection,
};
use self::diagnostics::DiagnosticLog;
use self::media::MediaSessionExt;
use self::presence::{BlfEntry, EventType, SubscriptionState};
use self::state::{AccountState, CallFSM, CallFSMEvent, EndReason, RegisterTimeoutPolicy};
use self::transport::{SipMessage, SipTransport};

pub use self::state::RegistrationStatus as RegistrationState;
pub use self::presence::Subscription;

/// Extract display name from a SIP URI like "Display Name" <sip:user@domain>
fn extract_display_name(uri: &str) -> Option<String> {
    // Check for quoted display name: "Display Name" <sip:...>
    if let Some(start) = uri.find('"') {
        if let Some(end) = uri[start + 1..].find('"') {
            let name = uri[start + 1..start + 1 + end].trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    // Check for unquoted display name: Display Name <sip:...>
    if let Some(bracket) = uri.find('<') {
        let prefix = uri[..bracket].trim();
        if !prefix.is_empty() && !prefix.starts_with("sip:") {
            return Some(prefix.to_string());
        }
    }
    None
}

/// Backward compatibility alias for CallEventPayload
pub type CallEvent = CallEventPayload;

impl CallEventPayload {
    /// Create a call event from a CallFSM
    #[allow(dead_code)]
    pub fn from_fsm(call: &CallFSM, state_override: Option<&str>) -> Self {
        Self {
            account_id: call.account_id.clone(),
            call_id: call.id.clone(),
            state: state_override.map(String::from).unwrap_or_else(|| call.state_name().to_string()),
            remote_uri: call.remote_uri.clone(),
            remote_name: extract_display_name(&call.remote_uri),
            direction: call.direction_str().to_string(),
            sip_call_id: Some(call.call_id_header.clone()),
            recording_path: None,
            srtp_active: Some(call.srtp_active),
            has_early_media: Some(call.has_early_media()),
        }
    }

    /// Create a call event with basic info
    pub fn new(
        account_id: impl Into<String>,
        call_id: impl Into<String>,
        state: impl Into<String>,
        remote_uri: impl Into<String>,
        direction: impl Into<String>,
    ) -> Self {
        let uri: String = remote_uri.into();
        Self {
            account_id: account_id.into(),
            call_id: call_id.into(),
            state: state.into(),
            remote_uri: uri.clone(),
            remote_name: extract_display_name(&uri),
            direction: direction.into(),
            sip_call_id: None,
            recording_path: None,
            srtp_active: None,
            has_early_media: None,
        }
    }

    /// Whether the call's media is encrypted, as negotiated; for "connected".
    pub fn with_srtp_active(mut self, active: bool) -> Self {
        self.srtp_active = Some(active);
        self
    }

    /// Set the SIP Call-ID header value
    pub fn with_sip_call_id(mut self, sip_call_id: impl Into<String>) -> Self {
        self.sip_call_id = Some(sip_call_id.into());
        self
    }

    /// Attach the path of the recording this call produced.
    pub fn with_recording_path(mut self, path: impl Into<String>) -> Self {
        self.recording_path = Some(path.into());
        self
    }

    /// Set whether early media is active on this call.
    pub fn with_early_media(mut self, has_early_media: bool) -> Self {
        self.has_early_media = Some(has_early_media);
        self
    }
}

#[derive(Debug, Clone, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CallEventPayload {
    #[serde(default)]
    pub account_id: String,
    pub call_id: String,
    pub state: String,
    pub remote_uri: String,
    pub remote_name: Option<String>,
    pub direction: String,
    /// SIP Call-ID header value (for linking to PCAP/diagnostic traces)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sip_call_id: Option<String>,
    /// Path of the recording this call produced, on the event that ends it.
    ///
    /// The frontend files call history from the "ended" event. When the remote
    /// party hangs up it has never been told where the recording went, so
    /// without this the WAV exists on disk with nothing in the UI pointing at
    /// it. Only the local-hangup path knew the path before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_path: Option<String>,
    /// Whether the media is encrypted, on the "connected" event. The call
    /// screen's padlock reads this; the backend computed it but never sent
    /// it, so every call showed "Not encrypted".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub srtp_active: Option<bool>,
    /// Whether early media is active (180/183 with SDP).
    /// Used by the frontend to suppress local ringback tone when in-band
    /// network audio is already playing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_early_media: Option<bool>,
}

/// Protocol timers.
///
/// Tests shrink these so that a lost REGISTER, its retransmissions, the
/// deadline and the reconnect that follows play out in a few seconds rather
/// than the minute the RFC values take.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// RFC 3261 T1: the first retransmission interval over UDP.
    pub t1: std::time::Duration,
    /// Timer F: how long a non-INVITE transaction waits for a final response.
    pub timer_f: std::time::Duration,
    /// The shortest interval between registration refreshes.
    pub min_refresh: std::time::Duration,
    /// How long unregistering waits for the registrar's answer.
    pub unregister_wait: std::time::Duration,
    /// How often the SUBSCRIBE refresh timer looks for subscriptions due.
    pub subscribe_tick: std::time::Duration,
    /// How often the OPTIONS keepalive goes out.
    pub options_interval: std::time::Duration,
    /// How long an OPTIONS keepalive may go unanswered before it is a miss.
    pub options_wait: std::time::Duration,
    /// The longest a reconnect waits between rounds, whatever a `Retry-After`
    /// asked for.
    pub max_wait: std::time::Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            t1: std::time::Duration::from_millis(500),
            timer_f: std::time::Duration::from_secs(32),
            min_refresh: std::time::Duration::from_secs(30),
            unregister_wait: std::time::Duration::from_secs(3),
            subscribe_tick: std::time::Duration::from_secs(15),
            options_interval: std::time::Duration::from_secs(15),
            options_wait: std::time::Duration::from_secs(5),
            max_wait: srv::MAX_WAIT,
        }
    }
}

#[derive(Clone)]
pub struct SipManager {
    state: Arc<RwLock<ManagerState>>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
    /// Fired by the first `start_background_tasks`; shared by every clone so
    /// the process-wide loops are started once however many handles exist.
    background_tasks: Arc<std::sync::Once>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferEvent {
    pub account_id: String,
    pub call_id: String,
    pub status: u16,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationEvent {
    pub account_id: String,
    pub state: RegistrationState,
    pub error: Option<String>,
}

#[derive(Debug)]
pub enum SipEvent {
    RegistrationChanged(RegistrationEvent),
    /// The step-by-step picture behind the status chip changed.
    ConnectionReport(connection::ConnectionReport),
    CallStateChanged(CallEventPayload),
    DiagnosticMessage(DiagnosticLog),
    TransferProgress(TransferEvent),
    PresenceChanged(String, Vec<BlfEntry>), // account_id, entries
    ConferenceCreated { conference_id: String, call_ids: Vec<String> },
    ConferenceSplit { conference_id: String, call_id: String },
    ConferenceEnded { conference_id: String },
    VoicemailStatus(VoicemailStatusEvent),
    RecordingStateChanged(RecordingEvent),
}

/// Emitted when the backend starts or stops recording a call on its own.
///
/// Manual recording is driven from the UI, which already knows the state it
/// asked for. Auto-record is not: without this event the call's `recording`
/// flag stayed false, so the recording indicator never appeared and hangup
/// never stopped the recording or filed its path into call history.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEvent {
    pub account_id: String,
    pub call_id: String,
    pub recording: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// MWI (Message Waiting Indicator) status from a message-summary NOTIFY.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicemailStatusEvent {
    pub account_id: String,
    pub messages_waiting: bool,
    pub new_messages: u32,
    pub old_messages: u32,
}

/// Manager state with multi-account support
///
/// Each account has its own independent state including registration,
/// transport, active calls, and presence subscriptions.
pub(crate) struct ManagerState {
    /// Per-account states, keyed by account ID
    pub(crate) accounts: HashMap<String, AccountState>,
    /// Protocol timers. Production uses RFC 3261 values; tests shrink them.
    pub(crate) timing: Timing,
    /// How SIP server names are looked up.
    pub(crate) dns: Arc<dyn srv::DnsResolver>,
    /// The STUN servers asked for our public address when a call's SDP is
    /// built. Production uses `rtp_engine`'s public list; the harness points
    /// it at a local stub so no test waits on the internet.
    pub(crate) stun_servers: Arc<[String]>,
    /// Currently active/default account ID for backward-compatible single-account operations
    pub(crate) active_account_id: Option<String>,

    // === Shared state across all accounts ===
    /// Last OPTIONS keepalive round-trip time in ms (from active account)
    pub(crate) last_latency_ms: Option<f64>,
    /// Shared diagnostic store
    pub(crate) diagnostic_store: Arc<diagnostics::DiagnosticStore>,
    /// What the status chip's panel shows, per account. Kept outside
    /// `AccountState` so a failure before the account exists (DNS, a refused
    /// connect) and the attempt after an unregister both have somewhere to go.
    pub(crate) connection_reports: HashMap<String, connection::ConnectionReport>,

    // === Audio device preferences ===
    /// Preferred input (microphone) device name (None = system default)
    pub(crate) preferred_input_device: Option<String>,
    /// Preferred output (speaker) device name (None = system default)
    pub(crate) preferred_output_device: Option<String>,

    // === Recording ===
    /// Directory recordings are written to, resolved from the Tauri app data
    /// dir at startup.
    ///
    /// The auto-record paths used to build this themselves from
    /// `dirs::data_dir()` joined with a hardcoded `com.5060.aria`, which is not
    /// the bundle identifier (`com.5060solutions.aria`). Auto-recordings
    /// therefore landed in a directory that nothing in the UI ever opened.
    /// Resolving it once at startup keeps every recording path in agreement.
    pub(crate) recordings_dir: Option<std::path::PathBuf>,
}

/// Save the recording in progress on `media`, if any, and queue it for
/// transcription when the user has turned that on.
///
/// This writes the whole recording to disk synchronously, so teardown paths
/// must not call it with the state lock held; they go through
/// `finish_call_media` instead.
pub(crate) fn save_recording(
    call_id: &str,
    media: &media::MediaSession,
) -> Result<Option<std::path::PathBuf>, String> {
    let path = media
        .stop_recording()
        .map_err(|e| format!("Failed to stop recording: {}", e))?;
    if let Some(p) = path.as_ref() {
        log::info!("Call recording saved: {}", p.display());
        // Runs on its own thread, so a slow transcription cannot hold up
        // whatever called this.
        crate::ai::maybe_transcribe_in_background(call_id, p);
    }
    Ok(path)
}

/// Stop a call's media and finish its recording. Returns the recording's path
/// for the call's `ended` event, so history can link to it.
///
/// Every way a call ends goes through here: local hangup, BYE, transfer,
/// Replaces, transport loss. Each used to carry its own copy of this, and all
/// but two forgot the recording, so whether a call got a transcript depended
/// on how it ended.
///
/// Callers take the session out of the call with `take_media` and release the
/// state lock first. Saving a long call writes tens of megabytes, and doing it
/// under the lock stalled every other SIP task until it finished.
pub(crate) async fn finish_call_media(
    call_id: &str,
    media: Option<media::MediaSession>,
) -> Option<String> {
    let media = media?;
    let call_id = call_id.to_string();
    tokio::task::spawn_blocking(move || {
        let path = save_recording(&call_id, &media).unwrap_or_else(|e| {
            log::warn!("{} for call {}", e, call_id);
            None
        });
        media.stop();
        path.map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .unwrap_or_else(|e| {
        log::error!("Media teardown task failed: {}", e);
        None
    })
}

impl ManagerState {
    /// Start auto-recording `media` if the owning account asks for it.
    ///
    /// Returns the event to emit once the state lock is released, or None if
    /// the account has auto-record off or recording could not be started.
    ///
    /// This is a method rather than three copies at the call sites because it
    /// previously was three copies, and one of them — the early-media path in
    /// handle_invite_response — never got written. Any call whose peer sent a
    /// 183 with SDP, which is ordinary PSTN ringback, connected on the early
    /// media session and was silently never recorded.
    pub(crate) fn start_auto_record(
        &self,
        account_id: &str,
        call_id: &str,
        media: &media::MediaSession,
    ) -> Option<RecordingEvent> {
        let auto_record = self
            .get_account(account_id)
            .map(|a| a.config.auto_record)
            .unwrap_or(false);
        if !auto_record {
            return None;
        }

        // Startup installs this before any call can start; if it is somehow
        // unset we would have to guess at a path, so skip recording rather
        // than write to the wrong directory.
        let Some(recordings_dir) = self.recordings_dir.as_ref() else {
            log::warn!(
                "Auto-record enabled but no recordings directory is configured; \
                 skipping recording for call {}",
                call_id
            );
            return None;
        };

        let output_path = rtp_engine::generate_recording_filename(call_id, recordings_dir);
        if let Err(e) = media.start_recording(output_path.clone()) {
            log::warn!("Failed to start auto-recording: {}", e);
            return None;
        }

        log::info!("Auto-recording started for call {}", call_id);
        Some(RecordingEvent {
            account_id: account_id.to_string(),
            call_id: call_id.to_string(),
            recording: true,
            path: Some(output_path.to_string_lossy().to_string()),
        })
    }

    /// Get the active account (first registered or explicitly set)
    pub(crate) fn active_account(&self) -> Option<&AccountState> {
        self.active_account_id
            .as_ref()
            .and_then(|id| self.accounts.get(id))
            .or_else(|| self.accounts.values().next())
    }

    /// Get an account by ID
    pub(crate) fn get_account(&self, account_id: &str) -> Option<&AccountState> {
        self.accounts.get(account_id)
    }

    /// Get an account by ID mutably
    pub(crate) fn get_account_mut(&mut self, account_id: &str) -> Option<&mut AccountState> {
        self.accounts.get_mut(account_id)
    }

    /// Find a call across all accounts
    pub(crate) fn find_call(&self, call_id: &str) -> Option<(&AccountState, &CallFSM)> {
        for account in self.accounts.values() {
            if let Some(call) = account.calls.iter().find(|c| c.id == call_id) {
                return Some((account, call));
            }
        }
        None
    }

    /// Find a call across all accounts mutably
    pub(crate) fn find_call_mut(&mut self, call_id: &str) -> Option<(&str, &mut CallFSM)> {
        for (account_id, account) in self.accounts.iter_mut() {
            if let Some(call) = account.calls.iter_mut().find(|c| c.id == call_id) {
                return Some((account_id.as_str(), call));
            }
        }
        None
    }

    /// Find a call by SIP Call-ID header across all accounts
    pub(crate) fn find_call_by_header(&self, call_id_header: &str) -> Option<(&AccountState, &CallFSM)> {
        for account in self.accounts.values() {
            if let Some(call) = account.calls.iter().find(|c| c.call_id_header == call_id_header) {
                return Some((account, call));
            }
        }
        None
    }

    /// Find a call by SIP Call-ID header across all accounts mutably
    pub(crate) fn find_call_by_header_mut(&mut self, call_id_header: &str) -> Option<(&str, &mut CallFSM)> {
        for (account_id, account) in self.accounts.iter_mut() {
            if let Some(call) = account.calls.iter_mut().find(|c| c.call_id_header == call_id_header) {
                return Some((account_id.as_str(), call));
            }
        }
        None
    }

    /// Get all active calls across all accounts
    pub(crate) fn all_active_calls(&self) -> Vec<&CallFSM> {
        self.accounts.values().flat_map(|a| a.calls.iter()).collect()
    }
}

/// Per-account status information
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub account_id: String,
    pub username: String,
    pub domain: String,
    pub registration_state: String,
    pub registration_error: Option<String>,
    pub server_address: Option<String>,
    pub transport_type: Option<String>,
    pub local_address: Option<String>,
    pub public_address: Option<String>,
    pub uptime_secs: Option<u64>,
    pub active_calls: Vec<CallStatusInfo>,
}

/// Rich status information for the diagnostics panel
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub accounts: Vec<AccountStatus>,
    pub latency_ms: Option<f64>,
    pub total_active_calls: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallStatusInfo {
    pub id: String,
    pub account_id: String,
    pub remote_uri: String,
    pub state: String,
    pub direction: String,
    pub duration_secs: Option<u64>,
    pub codec: Option<String>,
    pub rtp_stats: Option<media::RtpStats>,
    /// Whether the media is actually encrypted, as negotiated.
    pub srtp_active: bool,
    /// SIP Call-ID header, for matching the call to its trace.
    pub sip_call_id: String,
}

/// What an account is configured as, without anything secret.
///
/// For callers outside the SIP stack (the MCP server) that need to list
/// accounts: `AccountConfig` carries the password, so it never leaves here.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
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
    pub auto_record: bool,
    pub is_active: bool,
}

impl SipManager {
    /// Create a new SipManager and return the event receiver separately.
    /// This allows the caller to set up event forwarding before managing the state.
    ///
    /// Construction is pure: it needs no runtime and spawns nothing. The
    /// background loops are started by `start_background_tasks`.
    pub fn new_with_receiver() -> (Self, mpsc::UnboundedReceiver<SipEvent>) {
        Self::new_with_timing(Timing::default())
    }

    pub(crate) fn new_with_timing(timing: Timing) -> (Self, mpsc::UnboundedReceiver<SipEvent>) {
        Self::new_with_dns(timing, Arc::new(srv::SystemResolver::default()))
    }

    pub(crate) fn new_with_dns(
        timing: Timing,
        dns: Arc<dyn srv::DnsResolver>,
    ) -> (Self, mpsc::UnboundedReceiver<SipEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let manager = Self {
            state: Arc::new(RwLock::new(ManagerState {
                accounts: HashMap::new(),
                timing,
                dns,
                stun_servers: media::default_stun_servers().into(),
                active_account_id: None,
                last_latency_ms: None,
                diagnostic_store: Arc::new(diagnostics::DiagnosticStore::new(500)),
                connection_reports: HashMap::new(),
                preferred_input_device: None,
                preferred_output_device: None,
                recordings_dir: None,
            })),
            event_tx,
            background_tasks: Arc::new(std::sync::Once::new()),
        };
        (manager, event_rx)
    }

    /// Start the process-wide loops: the ended-call sweeper, the session
    /// timer and the subscription refresh timer.
    ///
    /// Must be called from inside a Tokio runtime, once the manager is in
    /// place — `setup()` in `lib.rs` does it from a task on Tauri's runtime,
    /// and tests that need the loops do it from `#[tokio::test]`. Calling it
    /// again is harmless: the loops are started exactly once per manager.
    ///
    /// The constructor used to spawn these, which made construction demand a
    /// runtime it had no way to ask for: v0.5.0 built the manager on the bare
    /// main thread and panicked on launch ("there is no reactor running").
    /// Keeping the spawns out of construction is what makes
    /// `manager_constructs_without_a_runtime` possible.
    pub fn start_background_tasks(&self) {
        self.background_tasks.call_once(|| {
            self.start_ended_call_sweeper();
            // Process-wide loops, started once. They used to be spawned by
            // every `register()` call and never stopped.
            self.start_session_timer();
            self.start_subscribe_refresh_timer();
        });
    }

    /// How long an ended call's record is kept.
    ///
    /// Timer H (RFC 3261 §17.1.1): the longest a far end will keep
    /// retransmitting a final response we have to be able to match.
    const ENDED_CALL_RETENTION: std::time::Duration = std::time::Duration::from_secs(32);

    /// Drop the records of calls that ended a while ago.
    ///
    /// Every way a call ends leaves its record in place, and only local hangup
    /// used to remove it (after 2 s). Calls ended by the far end, by a
    /// rejection or by a timeout accumulated for the life of the process and
    /// were counted as active by the status report.
    fn start_ended_call_sweeper(&self) {
        let state = self.state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let mut s = state.write().await;
                for account in s.accounts.values_mut() {
                    account.calls.retain(|call| {
                        call.ended_at
                            .is_none_or(|at| at.elapsed() < Self::ENDED_CALL_RETENTION)
                    });
                }
            }
        });
    }

    fn emit(&self, event: SipEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Emit a registration state change event
    fn emit_registration_event(&self, account_id: &str, state: RegistrationState, error: Option<String>) {
        self.emit(SipEvent::RegistrationChanged(RegistrationEvent {
            account_id: account_id.to_string(),
            state,
            error,
        }));
    }

    /// The step-by-step picture for `account_id`, if an attempt was ever made.
    pub async fn connection_report(&self, account_id: &str) -> Option<connection::ConnectionReport> {
        self.state.read().await.connection_reports.get(account_id).cloned()
    }

    /// The status chip's "Retry now" / "Re-register".
    ///
    /// A registered account gets a refresh REGISTER on the connection it has,
    /// so nothing is signed out and a call in progress is untouched. An
    /// account that is failed, reconnecting or unknown is taken through the
    /// whole path again, from DNS, with the config the caller holds — which
    /// is why this takes the config and not just an id: a DNS or connect
    /// failure leaves no `AccountState` to read it from.
    pub async fn register_now(&self, account: AccountConfig) -> Result<String, String> {
        let account_id = account.id.clone();
        let refresh = {
            let mut s = self.state.write().await;
            match s.accounts.get_mut(&account_id) {
                Some(existing) if existing.transport.is_some() => match existing.registration.status() {
                    RegistrationState::Registering => return Ok("already_registering".into()),
                    RegistrationState::Registered => {
                        existing.adopt_config(&account);
                        existing.refreshing = false;
                        existing.on_register_timeout = RegisterTimeoutPolicy::Reconnect;
                        existing.registration.begin_re_registration();
                        if let Some(r) = s.connection_reports.get_mut(&account_id) {
                            r.outcome = connection::Outcome::Registering;
                            r.pending(connection::StepKind::Authenticate);
                            r.pending(connection::StepKind::Registered);
                        }
                        true
                    }
                    _ => {
                        existing.transport = None;
                        existing.timer_generation += 1;
                        false
                    }
                },
                Some(existing) => {
                    existing.transport = None;
                    existing.timer_generation += 1;
                    false
                }
                None => false,
            }
        };
        if !refresh {
            return self.register(account).await;
        }
        if let Err(e) = send_register(&self.state, &self.event_tx, &account_id, None).await {
            log::warn!("Forced refresh for {} could not be sent; reconnecting: {}", account_id, e);
            Self::handle_transport_death(self.state.clone(), self.event_tx.clone(), account_id, true).await;
            return Ok("reconnecting".into());
        }
        Ok("refreshing".into())
    }

    pub async fn register(&self, account: AccountConfig) -> Result<String, String> {
        let account_id = account.id.clone();
        if self
            .state
            .read()
            .await
            .accounts
            .get(&account_id)
            .is_some_and(|a| emergency_call_active(a) && a.transport.is_some())
        {
            return Err("An emergency call is in progress; try again when it ends".into());
        }

        // Already up, or on its way up, with a live transport?
        let (replace, current_status) = {
            let mut s = self.state.write().await;
            match s.accounts.get_mut(&account_id) {
                Some(existing)
                    if existing.transport.is_some()
                        && matches!(
                            existing.registration.status(),
                            RegistrationState::Registering | RegistrationState::Registered
                        ) =>
                {
                    if connection_changed(existing, &account) {
                        // Server, credentials or transport changed: the binding
                        // we hold is for the old settings. Editing a registered
                        // account used to only overwrite the config, and the
                        // new server was never contacted until a restart.
                        (true, None)
                    } else {
                        // Settings that take effect per call (SRTP, codecs,
                        // auto-record) only need the config refreshed.
                        log::info!("Account {} already registered — updating config", account_id);
                        existing.adopt_config(&account);
                        (false, Some(existing.registration.status()))
                    }
                }
                _ => (false, None),
            }
        };
        if let Some(status) = current_status {
            // Say what the state is. Callers mark the account "registering"
            // before asking, and with no event in reply the UI stayed on
            // "Connecting…" until the next refresh happened to emit one.
            self.emit_registration_event(&account_id, status, None);
            return Ok("already_registering".into());
        }
        if replace {
            log::info!("Account {} settings changed; re-registering", account_id);
            self.unregister_account(Some(&account_id)).await?;
        }

        // A fresh report for this attempt. Started from the config, not
        // restarted, because the server, transport or proxy may have changed.
        let mut report = connection::ConnectionReport::start(&account);
        // What this account has learned about its servers survives a failed
        // attempt, so a retry does not start the list over.
        let mut failover = {
            let s = self.state.read().await;
            s.accounts.get(&account_id).map(|a| a.failover.clone()).unwrap_or_default()
        };
        failover.restart();
        let Connected { server_addr, transport, rx, local_addr, target } =
            match resolve_and_connect(&self.state, &self.event_tx, &account, &mut report, &mut failover).await {
                Ok(v) => v,
                Err(ConnectFailure { message: e, .. }) => {
                    // An account the backend already knows must end up in
                    // Error like any other failed registration, so the chip,
                    // the panel and the retry loop all see the same thing. A
                    // brand-new account has no state yet; the caller records
                    // the error from the returned message.
                    let marked = {
                        let mut s = self.state.write().await;
                        match s.accounts.get_mut(&account_id) {
                            Some(a) => {
                                a.registration.start_registration(account.clone());
                                a.registration.registration_failed(0, &e);
                                a.transport = None;
                                a.failover = failover;
                                true
                            }
                            None => false,
                        }
                    };
                    if marked {
                        self.emit_registration_event(&account_id, RegistrationState::Error, Some(e.clone()));
                    }
                    return Err(e);
                }
            };

        log::info!(
            "Registering {}@{} via {:?} to {} ({}) (local: {})",
            account.username,
            account.domain,
            target.transport,
            server_addr,
            target.host,
            local_addr
        );
        // The account as it will be run: the configured transport can be
        // `Auto`, the target's never is.
        let configured = account.clone();
        let account = with_transport(&account, &target.transport);

        let generation = {
            let mut state = self.state.write().await;
            if !state.accounts.contains_key(&account_id) {
                state.accounts.insert(account_id.clone(), AccountState::new(account.clone()));
            }
            if state.active_account_id.is_none() {
                state.active_account_id = Some(account_id.clone());
            }
            let account_state = state.accounts.get_mut(&account_id).unwrap();
            account_state.config = account.clone();
            account_state.configured_transport = configured.transport.clone();
            account_state.failover = failover;
            account_state.target = Some(target);
            account_state.flow_timer = None;
            account_state.registration.start_registration(account.clone());
            account_state.registration.set_server_addr(server_addr);
            account_state.registration.set_local_addr(local_addr);
            account_state.transport = Some(transport);
            account_state.server_addr = Some(server_addr);
            account_state.local_addr = Some(local_addr);
            account_state.refreshing = false;
            account_state.unregistering = false;
            account_state.requested_expires = state::DEFAULT_REGISTER_EXPIRES;
            account_state.on_register_timeout = RegisterTimeoutPolicy::Error;
            account_state.timer_generation += 1;
            account_state.timer_generation
        };

        self.emit_registration_event(&account_id, RegistrationState::Registering, None);

        Self::start_receive_loop_for_account(
            self.state.clone(),
            self.event_tx.clone(),
            rx,
            account_id.clone(),
            generation,
        );

        if let Err(e) = send_register(&self.state, &self.event_tx, &account_id, None).await {
            // Leave the account in a state a retry can start from. It used to
            // stay "registering" with no transport, and every later attempt
            // was waved through as "already registering".
            log::error!("Could not send REGISTER for {}: {}", account_id, e);
            {
                let mut s = self.state.write().await;
                if let Some(a) = s.accounts.get_mut(&account_id) {
                    a.registration.registration_failed(0, &e);
                    a.transport = None;
                }
            }
            self.emit_registration_event(&account_id, RegistrationState::Error, Some(e.clone()));
            return Err(e);
        }

        Self::start_account_timers(self.state.clone(), self.event_tx.clone(), account_id, generation);

        Ok("registering".into())
    }

    /// Create the account's transport and work out the local address other
    /// hosts can reach it on.
    /// Return the host and port packets for this account are sent to:
    /// the outbound proxy if configured (RFC 3261 §10.2 / §16.12),
    /// otherwise the registrar (or domain).
    pub(crate) fn target_server(account: &AccountConfig) -> (&str, u16) {
        let (host, port) = Self::target_endpoint(account);
        (host, port.unwrap_or(0))
    }

    /// Like `target_server`, but a port is `Some` only when one was given: in
    /// the proxy or registrar string, or as the account's non-zero port. No
    /// port means the name is a service name and DNS SRV decides the port
    /// (RFC 3263 section 4.2).
    pub(crate) fn target_endpoint(account: &AccountConfig) -> (&str, Option<u16>) {
        let raw = account
            .outbound_proxy
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                account
                    .registrar
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or(account.domain.trim());

        // Only split off a port when the host cannot itself contain a colon:
        // a bracketed IPv6 literal (`[2001:db8::1]:5061`) or a name with a
        // single colon. A bare IPv6 literal (`2001:db8::1`) keeps the
        // account's port instead of losing its last group to the port.
        let host_port = match raw.strip_prefix('[') {
            Some(rest) => rest.split_once("]:"),
            None if raw.matches(':').count() == 1 => raw.split_once(':'),
            None => None,
        };
        if let Some((host, port_str)) = host_port {
            if let Ok(port) = port_str.parse::<u16>() {
                return (host, Some(port).filter(|p| *p != 0));
            }
        }
        (
            raw.trim_start_matches('[').trim_end_matches(']'),
            Some(account.port).filter(|p| *p != 0),
        )
    }

    /// Start the refresh and keepalive loops for one transport generation.
    ///
    /// Called when a transport is created, by `register()` and by the
    /// reconnect loop. Both loops exit when the account is gone, is
    /// unregistered, or has moved on to a newer generation, so an account
    /// never has more than one of each.
    fn start_account_timers(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        account_id: String,
        generation: u64,
    ) {
        tokio::spawn(Self::re_register_loop(state.clone(), event_tx.clone(), account_id.clone(), generation));
        tokio::spawn(Self::options_keepalive_loop(state.clone(), event_tx.clone(), account_id.clone(), generation));
        tokio::spawn(Self::crlf_keepalive_loop(state, event_tx, account_id, generation));
    }

    /// Whether the account still exists on this transport generation.
    fn account_current(s: &ManagerState, account_id: &str, generation: u64) -> bool {
        s.accounts
            .get(account_id)
            .is_some_and(|a| a.timer_generation == generation)
    }

    /// Refresh the registration before the granted Expires runs out.
    async fn re_register_loop(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        aid: String,
        generation: u64,
    ) {
        loop {
            // Polled rather than slept for the whole interval: the grant is
            // only known once the 200 arrives, which is after this loop
            // starts, and a registrar can change it on any refresh.
            let tick = state.read().await.timing.min_refresh / 2;
            tokio::time::sleep(tick).await;

            {
                let mut s = state.write().await;
                if !Self::account_current(&s, &aid, generation) {
                    return;
                }
                let min_refresh = s.timing.min_refresh;
                let account = s.accounts.get_mut(&aid).unwrap();
                match account.registration.status() {
                    RegistrationState::Unregistered | RegistrationState::Reconnecting => return,
                    RegistrationState::Registered => {}
                    // Registering (a refresh is in flight and has its own
                    // deadline) or Error (the frontend decides on retries).
                    _ => continue,
                }
                // Half of what was granted, so one lost refresh still leaves
                // time for another before the binding expires. The refresh
                // used to be a fixed 120 s whatever the server said; a
                // registrar granting 60 s left the phone unreachable half the
                // time.
                let interval = std::time::Duration::from_secs(u64::from(account.granted_expires) / 2)
                    .max(min_refresh);
                let due = account
                    .registration
                    .registered_at()
                    .is_some_and(|at| at.elapsed() >= interval);
                if !due {
                    continue;
                }
                account.refreshing = true;
                account.on_register_timeout = RegisterTimeoutPolicy::Reconnect;
                // So the 401 handler, which only acts while Registering,
                // answers the challenge the refresh will get.
                account.registration.begin_re_registration();
            }

            // Each refresh is also when the server list is renewed if its TTL
            // has run out, and when a better server that has come back is
            // moved back to.
            if Self::maintain_targets(&state, &event_tx, &aid).await {
                return;
            }

            if let Err(e) = send_register(&state, &event_tx, &aid, None).await {
                log::error!("Re-REGISTER failed for {} — transport dead: {}", aid, e);
                Self::handle_transport_death(state.clone(), event_tx.clone(), aid.clone(), true).await;
                return;
            }
        }
    }

    /// Renew the account's server list when its TTL has run out, and move
    /// back to a higher-priority server that has started answering again.
    /// Returns true when it did move (the reconnect takes it from here).
    async fn maintain_targets(
        state: &Arc<RwLock<ManagerState>>,
        event_tx: &mpsc::UnboundedSender<SipEvent>,
        aid: &str,
    ) -> bool {
        let now = std::time::Instant::now();
        let (config, failover, dns, busy) = {
            let s = state.read().await;
            let Some(a) = s.accounts.get(aid) else { return false };
            let mut cfg = a.config.clone();
            cfg.transport = a.configured_transport.clone();
            (cfg, a.failover.clone(), s.dns.clone(), a.calls.iter().any(|c| !c.is_ended()))
        };

        if failover.needs_resolve(now) && !failover.is_empty() {
            let (host, port) = Self::target_endpoint(&config);
            match srv::resolve(&*dns, host, port, &config.transport, &mut srv::random_draw).await {
                Ok(resolved) => {
                    log::debug!("Server list for {aid} renewed: {} target(s)", resolved.targets.len());
                    if let Some(a) = state.write().await.accounts.get_mut(aid) {
                        a.failover.install(resolved, std::time::Instant::now());
                    }
                }
                Err(e) => log::warn!("Could not renew the server list for {aid}: {e}"),
            }
        }

        // Never move a call that is up.
        if busy {
            return false;
        }
        let Some(candidate) = state.read().await.accounts.get(aid).and_then(|a| a.failover.failback_candidate(now)) else {
            return false;
        };
        let (tls_name, local_addr) = {
            let s = state.read().await;
            let Some(a) = s.accounts.get(aid) else { return false };
            let name = config.tls_name_for(a.failover.via_srv(), a.failover.lookup_host()).to_string();
            let Some(local) = a.local_addr else { return false };
            (name, local)
        };
        let probe_config = with_transport(&config, &candidate.transport);
        if probe_target(&probe_config, &candidate, &tls_name, local_addr).await {
            log::info!("Higher-priority server {} ({}) answers again; moving back to it", candidate.addr, candidate.host);
            {
                let mut s = state.write().await;
                let Some(a) = s.accounts.get_mut(aid) else { return false };
                // A call may have started during the probe.
                if a.calls.iter().any(|c| !c.is_ended()) {
                    log::info!("A call started while probing; staying on the current server");
                    return false;
                }
                a.failover.prefer(&candidate);
            }
            SipManager::handle_transport_death(state.clone(), event_tx.clone(), aid.to_string(), true).await;
            true
        } else {
            if let Some(a) = state.write().await.accounts.get_mut(aid) {
                a.failover.report_failure(&candidate, &srv::Failure::Connect, std::time::Instant::now());
            }
            false
        }
    }

    /// Keep the NAT pinhole open with an OPTIONS, and notice when nothing
    /// answers: a send that fails, or two OPTIONS in a row with no response of
    /// any kind, means the server is gone. Over UDP a send never fails, so the
    /// missing response is the only way a dead server is ever seen.
    async fn options_keepalive_loop(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        aid: String,
        generation: u64,
    ) {
        let mut misses = 0u32;
        loop {
            let (interval, wait) = {
                let s = state.read().await;
                (s.timing.options_interval, s.timing.options_wait)
            };
            tokio::time::sleep(interval).await;

            let (account_config, local_addr, server_addr, transport, cseq) = {
                let mut s = state.write().await;
                if !Self::account_current(&s, &aid, generation) {
                    return;
                }
                let account = s.accounts.get_mut(&aid).unwrap();
                match account.registration.status() {
                    RegistrationState::Unregistered | RegistrationState::Reconnecting => return,
                    RegistrationState::Registered => {}
                    _ => {
                        misses = 0;
                        continue;
                    }
                }
                let (Some(local_addr), Some(server_addr), Some(transport)) =
                    (account.local_addr, account.server_addr, account.transport.clone())
                else {
                    continue;
                };
                account.options_cseq += 1;
                (account.config.clone(), local_addr, server_addr, transport, account.options_cseq)
            };

            let msg = build_options(
                &account_config.domain,
                local_addr,
                &builder::generate_call_id(),
                cseq,
                &builder::generate_tag(),
                &account_config.username,
                account_config.transport.param(),
            );

            let sent_at = std::time::Instant::now();
            let answered = if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                log::warn!("OPTIONS keepalive could not be sent for {}: {}", aid, e);
                false
            } else {
                tokio::time::sleep(wait).await;
                let s = state.read().await;
                if !Self::account_current(&s, &aid, generation) {
                    return;
                }
                s.accounts
                    .get(&aid)
                    .and_then(|a| a.options_last_response)
                    .is_some_and(|at| at >= sent_at)
            };

            if answered {
                misses = 0;
                continue;
            }
            misses += 1;
            log::warn!("OPTIONS keepalive unanswered for {} ({}/2)", aid, misses);
            if misses >= 2 {
                // A call in progress is not dropped for a server that has gone
                // quiet: its media may well still be flowing. Failover waits
                // until the calls have ended.
                let on_call = state
                    .read()
                    .await
                    .accounts
                    .get(&aid)
                    .is_some_and(|a| a.calls.iter().any(|c| !c.is_ended()));
                if on_call {
                    log::warn!("Server of {} is not answering, but a call is up; not failing over", aid);
                    misses = 1;
                    continue;
                }
                log::error!("OPTIONS keepalive lost for {} — the server has stopped answering", aid);
                fail_over(&state, &event_tx, &aid, srv::Failure::Timeout, false).await;
                return;
            }
        }
    }

    /// RFC 5626 section 3.5.1 keep-alive: a CRLF CRLF ping on a stream
    /// connection, at the registrar's Flow-Timer (80 to 100 percent of it) or
    /// every 25 s when it gave none. Holds the connection open through NATs
    /// that time out idle flows. A ping that cannot be written means the
    /// connection is dead.
    async fn crlf_keepalive_loop(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        aid: String,
        generation: u64,
    ) {
        loop {
            let interval = {
                let s = state.read().await;
                if !Self::account_current(&s, &aid, generation) {
                    return;
                }
                let account = s.accounts.get(&aid).unwrap();
                match account.transport.as_ref() {
                    Some(t) if t.is_stream() => {}
                    _ => return,
                }
                srv::keepalive_interval(account.flow_timer, srv::random_draw(21))
            };
            tokio::time::sleep(interval).await;

            let transport = {
                let s = state.read().await;
                if !Self::account_current(&s, &aid, generation) {
                    return;
                }
                let account = s.accounts.get(&aid).unwrap();
                match account.registration.status() {
                    RegistrationState::Unregistered | RegistrationState::Reconnecting => return,
                    RegistrationState::Registered => {}
                    _ => continue,
                }
                match account.transport.clone() {
                    Some(t) => t,
                    None => return,
                }
            };
            if let Err(e) = transport.send_crlf_ping().await {
                log::error!("CRLF keep-alive could not be written for {}: {}", aid, e);
                // The connection is gone, but a call on it is not ended here:
                // reconnecting keeps the calls.
                Self::handle_transport_death(state.clone(), event_tx.clone(), aid.clone(), true).await;
                return;
            }
            log::trace!("CRLF keep-alive sent for {}", aid);
        }
    }

    /// Start session timer for connected calls (RFC 4028)
    fn start_session_timer(&self) {
        let state = self.state.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;

                let s = state.read().await;
                for call in s.all_active_calls() {
                    if !call.is_connected() {
                        continue;
                    }
                    if let Some(connected_at) = call.connected_at() {
                        let elapsed = connected_at.elapsed().as_secs();
                        let session_expires = call.session_expires() as u64;
                        let warn_threshold = session_expires.saturating_sub(120);
                        if elapsed >= warn_threshold && elapsed < session_expires {
                            log::warn!(
                                "Session timer: call {} nearing expiry ({}/{}s)",
                                call.id,
                                elapsed,
                                session_expires
                            );
                        }
                        if elapsed >= session_expires {
                            log::warn!(
                                "Session timer: call {} expired after {}s",
                                call.id,
                                elapsed
                            );
                        }
                    }
                }
            }
        });
    }

    pub async fn unregister(&self) -> Result<(), String> {
        self.unregister_account(None).await
    }

    /// Clear the account's binding on the registrar and forget the account.
    ///
    /// The REGISTER with Expires: 0 is challenged like any other, so the
    /// account stays until the authenticated one is answered (or the wait
    /// runs out). It used to be removed the instant the first, unauthenticated
    /// request went out; the 401 then arrived for an account that no longer
    /// existed, and the binding stood until it expired on its own — callers
    /// kept ringing a phone that had signed out.
    pub async fn unregister_account(&self, account_id: Option<&str>) -> Result<(), String> {
        let aid = match account_id {
            Some(id) => id.to_string(),
            None => {
                let s = self.state.read().await;
                match s.active_account_id.clone().or_else(|| s.accounts.keys().next().cloned()) {
                    Some(id) => id,
                    None => return Ok(()),
                }
            }
        };

        let (needs_request, wait) = {
            let mut s = self.state.write().await;
            let wait = s.timing.unregister_wait;
            let Some(account) = s.accounts.get_mut(&aid) else { return Ok(()) };
            // Stop the refresh and keepalive; the receive loop stays for the answer.
            account.timer_generation += 1;
            let registered = account.registration.status() == RegistrationState::Registered
                && account.transport.is_some();
            if registered {
                account.unregistering = true;
                account.refreshing = false;
                account.on_register_timeout = RegisterTimeoutPolicy::Forget;
                account.registration.begin_re_registration();
            }
            (registered, wait)
        };

        if !needs_request || send_register(&self.state, &self.event_tx, &aid, None).await.is_err() {
            finish_unregister(&self.state, &self.event_tx, &aid).await;
            return Ok(());
        }

        // The response handler removes the account on the 200. Wait for it, so
        // a caller re-registering with new settings starts from a clean slate.
        let deadline = tokio::time::Instant::now() + wait;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if !self.state.read().await.accounts.contains_key(&aid) {
                return Ok(());
            }
        }
        log::warn!("No answer to unregister for {}; forgetting the account", aid);
        finish_unregister(&self.state, &self.event_tx, &aid).await;
        Ok(())
    }

    /// Unregister every account, in parallel. For quitting: each waits for
    /// its answer at most `Timing::unregister_wait`.
    pub async fn unregister_all(&self) {
        let ids: Vec<String> = self.state.read().await.accounts.keys().cloned().collect();
        let tasks: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let mgr = self.clone();
                tokio::spawn(async move { mgr.unregister_account(Some(&id)).await })
            })
            .collect();
        for task in tasks {
            let _ = task.await;
        }
    }

    /// Set the active account for outbound calls without affecting registrations.
    /// This just changes which account is used by default for new outbound calls.
    pub async fn set_active_account(&self, account_id: &str) -> Result<(), String> {
        let mut s = self.state.write().await;
        if s.accounts.contains_key(account_id) {
            s.active_account_id = Some(account_id.to_string());
            log::info!("Active account set to: {}", account_id);
            Ok(())
        } else {
            Err(format!("Account {} not found or not registered", account_id))
        }
    }

    /// Set preferred audio devices for future calls.
    pub async fn set_audio_devices(
        &self,
        input_device: Option<String>,
        output_device: Option<String>,
    ) {
        let mut s = self.state.write().await;
        log::info!(
            "Audio devices set: input={:?}, output={:?}",
            input_device,
            output_device
        );
        s.preferred_input_device = input_device;
        s.preferred_output_device = output_device;
    }

    /// Set the directory recordings are written to. Called once at startup with
    /// the Tauri app data dir, which is the only component that can resolve it
    /// correctly for the current bundle identifier and platform.
    pub async fn set_recordings_dir(&self, dir: std::path::PathBuf) {
        let mut s = self.state.write().await;
        log::info!("Recordings directory set: {}", dir.display());
        s.recordings_dir = Some(dir);
    }

    /// Get the configured recordings directory, if startup has set it.
    pub async fn recordings_dir(&self) -> Option<std::path::PathBuf> {
        self.state.read().await.recordings_dir.clone()
    }

    /// Get current preferred audio devices.
    #[allow(dead_code)]
    pub async fn get_preferred_audio_devices(&self) -> (Option<String>, Option<String>) {
        let s = self.state.read().await;
        (
            s.preferred_input_device.clone(),
            s.preferred_output_device.clone(),
        )
    }

    pub async fn make_call(&self, uri: &str) -> Result<String, String> {
        self.make_call_on_account(uri, None, None).await
    }

    /// Place a call under an id the caller chose.
    ///
    /// The frontend shows the call the moment the user dials, before this
    /// returns, and events for the call are emitted before it returns too. When
    /// the backend picked the id, the frontend could not match those early
    /// events to the call it was showing and dropped them; a call answered
    /// instantly then sat on "dialing" and was filed as missed.
    pub async fn make_call_with_id(&self, uri: &str, call_id: &str) -> Result<String, String> {
        // The id ends up in the recording's filename, so it must not be able
        // to carry a path.
        let call_id = uuid::Uuid::parse_str(call_id)
            .map_err(|_| "Call id must be a UUID".to_string())?
            .to_string();
        if self.state.read().await.find_call(&call_id).is_some() {
            return Err("Call id already in use".into());
        }
        self.make_call_on_account(uri, None, Some(call_id)).await
    }

    pub async fn make_call_on_account(
        &self,
        uri: &str,
        account_id: Option<&str>,
        call_id_override: Option<String>,
    ) -> Result<String, String> {
        let (account_config, local_addr, server_addr, transport, aid, public_addr) = {
            let s = self.state.read().await;
            let aid = match account_id {
                Some(id) => id.to_string(),
                None => s.active_account_id.clone()
                    .or_else(|| s.accounts.keys().next().cloned())
                    .ok_or("No account configured")?,
            };
            let account = s.get_account(&aid).ok_or("Account not found")?;
            if account.registration.status() != RegistrationState::Registered {
                // An emergency call is placed whatever the registration says:
                // no backoff, Retry-After or failover state may delay it.
                if emergency::is_emergency(uri) {
                    drop(s);
                    let (cfg, local, server, transport, public) = self.emergency_transport(&aid).await?;
                    return self.place_call(uri, aid, cfg, local, server, transport, public, call_id_override).await;
                }
                return Err("Not registered".into());
            }
            let local_addr = account.local_addr.ok_or("No local addr")?;
            let server_addr = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            let public_addr = account.public_addr;
            (account.config.clone(), local_addr, server_addr, transport, aid, public_addr)
        };
        self.place_call(uri, aid, account_config, local_addr, server_addr, transport, public_addr, call_id_override)
            .await
    }

    /// A transport an emergency call can be sent on while the account is not
    /// registered: the one it has, or a new one to the best target in the
    /// server list. Nothing here waits for a backoff or consults what has been
    /// marked failed; every target is tried in priority order until one
    /// connects.
    async fn emergency_transport(
        &self,
        aid: &str,
    ) -> Result<(AccountConfig, SocketAddr, SocketAddr, SipTransport, Option<SocketAddr>), String> {
        let (config, failover, dns) = {
            let s = self.state.read().await;
            let a = s.get_account(aid).ok_or("Account not found")?;
            if let (Some(t), Some(l), Some(sv)) = (a.transport.clone(), a.local_addr, a.server_addr) {
                return Ok((a.config.clone(), l, sv, t, a.public_addr));
            }
            let mut cfg = a.config.clone();
            cfg.transport = a.configured_transport.clone();
            (cfg, a.failover.clone(), s.dns.clone())
        };
        let mut failover = failover;
        if failover.is_empty() {
            let (host, port) = Self::target_endpoint(&config);
            let resolved = srv::resolve(&*dns, host, port, &config.transport, &mut srv::random_draw).await?;
            failover.install(resolved, std::time::Instant::now());
        }
        let tls_name = config.tls_name_for(failover.via_srv(), failover.lookup_host()).to_string();
        let mut last = "no server".to_string();
        for target in failover.targets().to_vec() {
            let effective = with_transport(&config, &target.transport);
            match open_transport_for(&self.state, &self.event_tx, &effective, target.addr, &tls_name).await {
                Ok((transport, rx, local_addr)) => {
                    log::warn!("Emergency call: using {} ({}) without a registration", target.addr, target.host);
                    let generation = {
                        let mut s = self.state.write().await;
                        let a = s.accounts.get_mut(aid).ok_or("Account not found")?;
                        a.config = effective.clone();
                        a.target = Some(target.clone());
                        a.transport = Some(transport.clone());
                        a.server_addr = Some(target.addr);
                        a.local_addr = Some(local_addr);
                        a.timer_generation += 1;
                        a.timer_generation
                    };
                    Self::start_receive_loop_for_account(
                        self.state.clone(),
                        self.event_tx.clone(),
                        rx,
                        aid.to_string(),
                        generation,
                    );
                    let public = self.state.read().await.get_account(aid).and_then(|a| a.public_addr);
                    return Ok((effective, local_addr, target.addr, transport, public));
                }
                Err(e) => {
                    log::warn!("Emergency call: {} failed: {}", target.addr, e);
                    last = e;
                }
            }
        }
        Err(format!("No server reachable for an emergency call: {last}"))
    }

    #[allow(clippy::too_many_arguments)]
    async fn place_call(
        &self,
        uri: &str,
        aid: String,
        account_config: AccountConfig,
        local_addr: SocketAddr,
        server_addr: SocketAddr,
        transport: SipTransport,
        public_addr: Option<SocketAddr>,
        call_id_override: Option<String>,
    ) -> Result<String, String> {

        let call_id = builder::generate_call_id();
        let from_tag = builder::generate_tag();

        // Allocate RTP port and discover public IP via STUN for NAT traversal
        let stun_servers = self.state.read().await.stun_servers.clone();
        let (rtp_port, stun_public_ip, _stun_public_port) = media::allocate_port_with_stun(&stun_servers)
            .await
            .map_err(|e| format!("Failed to allocate RTP port: {}", e))?;

        // STUN first, then what the registrar reported; for a server on a
        // private network, neither (see `media::sdp_address`). This was
        // `Some(stun).or_else(..)`, which is always Some — so the fallback never
        // ran, and a failed STUN lookup's `0.0.0.0` went into the SDP (#4).
        let sdp_ip = media::sdp_address(
            local_addr.ip(),
            stun_public_ip.or_else(|| public_addr.map(|a| a.ip())),
            Some(server_addr.ip()),
        );
        let public_ip = Some(sdp_ip.to_string());
        let (invite, local_srtp_key) = builder::build_invite_with_public_ip(
            &account_config, uri, local_addr, rtp_port, &call_id, 1, &from_tag, None,
            public_ip.as_deref(),
        );

        let branch = extract_via_branch(&invite).unwrap_or_default();

        let local_uri = format!("sip:{}@{}", account_config.username, account_config.domain);
        let mut call = CallFSM::new_outbound(&aid, uri, call_id, from_tag, rtp_port, branch, local_uri);
        call.local_srtp_key = local_srtp_key;
        // The address this offer carries; the authenticated retry reuses it.
        call.sdp_public_ip = Some(sdp_ip);
        if let Some(id) = call_id_override {
            call.id = id;
        }
        let id = call.id.clone();

        transport.send_to(invite.as_bytes(), server_addr).await?;

        log::info!("Sending INVITE to {}", uri);

        let sip_call_id_header = call.call_id_header.clone();
        self.emit(SipEvent::CallStateChanged(CallEvent::new(
            &aid, &id, "dialing", uri, "outbound"
        ).with_sip_call_id(&sip_call_id_header)));

        {
            let mut s = self.state.write().await;
            if let Some(account) = s.get_account_mut(&aid) {
                account.calls.push(call);
            }
        }

        // Start INVITE timeout timer (32 seconds per RFC 3261)
        self.start_invite_timeout(&id, &aid);

        Ok(id)
    }

    /// Start a timer that ends the call if it stays in pre-connected states too long
    fn start_invite_timeout(&self, call_id: &str, account_id: &str) {
        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        let manager = self.clone();
        let call_id = call_id.to_string();
        let account_id = account_id.to_string();

        tokio::spawn(async move {
            // First check: 32 seconds for initial response (RFC 3261 INVITE transaction timeout)
            tokio::time::sleep(tokio::time::Duration::from_secs(32)).await;

            {
                let s = state.read().await;
                if let Some((_, call)) = s.find_call(&call_id) {
                    // If still dialing with no response at all, timeout immediately
                    if call.is_dialing() {
                        drop(s);
                        let mut s = state.write().await;
                        if let Some((_, call)) = s.find_call_mut(&call_id) {
                            if call.is_dialing() {
                                log::warn!("INVITE timeout (no response) for call {}", call_id);
                                let remote_uri = call.remote_uri.clone();
                                call.end(EndReason::Failed("Request timeout".to_string()));
                                let _ = event_tx.send(SipEvent::CallStateChanged(
                                    CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                                ));
                            }
                        }
                        return;
                    }
                    // If ringing, let it continue - we'll check again later
                    if !call.is_ringing() {
                        return; // Already connected or ended
                    }
                } else {
                    return; // Call no longer exists
                }
            }

            // Second check: additional 60 seconds for ringing to be answered
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

            let still_ringing = state
                .read()
                .await
                .find_call(&call_id)
                .is_some_and(|(_, call)| call.is_ringing());
            if still_ringing {
                // Give up the same way the user would: with a CANCEL. Ending
                // the call locally left the far end ringing until its own
                // timer expired.
                log::warn!("Call timeout (unanswered) for call {}", call_id);
                if let Err(e) = manager.hangup(&call_id).await {
                    log::warn!("Could not cancel unanswered call {}: {}", call_id, e);
                }
            }
        });
    }

    pub async fn hangup(&self, call_id: &str) -> Result<(), String> {
        let (action_msg, server_addr, transport, account_id, remote_uri, direction, sent_cancel) = {
            let mut s = self.state.write().await;
            let (aid, cseq, remote_uri, direction, our_tag, their_tag, call_id_header, last_invite_branch, invite_cseq, local_uri, route_set, is_incoming, is_dialing_or_ringing, raw_invite, remote_contact) = {
                let (aid, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
                let aid = aid.to_string();
                let cseq = call.next_cseq();
                let remote_uri = call.remote_uri.clone();
                let direction = call.direction_str().to_string();
                let (our_tag, their_tag) = call.dialog_tags();
                // An unanswered inbound call has our tag already (we chose it
                // for the 180); an unanswered outbound one has no remote tag
                // yet, and the 486 path below needs one for ours.
                let our_tag = if our_tag.is_empty() { builder::generate_tag() } else { our_tag };
                let call_id_header = call.call_id_header.clone();
                let last_invite_branch = call.last_invite_branch.clone();
                let invite_cseq = call.invite_cseq;
                let local_uri = call.local_uri.clone();
                let route_set = call.route_set().to_vec();
                let is_incoming = call.is_incoming();
                let is_dialing_or_ringing = call.is_dialing() || call.is_ringing();
                let raw_invite = call.raw_invite().map(|str_| str_.to_string());
                let remote_contact = call.remote_contact.clone();
                (aid, cseq, remote_uri, direction, our_tag, their_tag, call_id_header, last_invite_branch, invite_cseq, local_uri, route_set, is_incoming, is_dialing_or_ringing, raw_invite, remote_contact)
            };

            let account = s.get_account(&aid).ok_or("Account not found")?;
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            let transport_str = account.config.transport.param().to_string();

            // `is_incoming` is the unanswered-inbound state, which is neither
            // dialing nor ringing (those are outbound states), so it is tested
            // on its own.
            let msg = if is_incoming {
                // Reject incoming call while ringing with 486 Busy Here (RFC 3261 Section 21.4.17)
                raw_invite
                    .as_deref()
                    .and_then(|raw| builder::build_final_response(raw, 486, "Busy Here", &our_tag))
            } else if is_dialing_or_ringing {
                // Outbound call not answered yet: cancel INVITE (RFC 3261 Section 9.1).
                // The CANCEL carries the INVITE's CSeq, which is 2 after an
                // auth retry; a literal 1 made FreeSWITCH answer 481 and keep
                // ringing the callee.
                let cancel_msg = build_cancel(
                    &remote_uri,
                    la,
                    &call_id_header,
                    invite_cseq,
                    &our_tag,
                    &transport_str,
                    &last_invite_branch.unwrap_or_default(),
                    &local_uri,
                    &remote_uri,
                );
                Some(cancel_msg)
            } else {
                // Established dialog: send BYE. RFC 3261 Section 15: the sender
                // of the BYE puts its own tag in From and the remote party's
                // tag in To, whichever side sent the INVITE.
                let target_uri = remote_contact.as_deref().unwrap_or(&remote_uri);

                let bye = build_bye_with_routes(
                    target_uri,
                    la,
                    &call_id_header,
                    cseq,
                    &our_tag,
                    &their_tag,
                    &transport_str,
                    &route_set,
                    &local_uri,
                    &remote_uri,
                );
                Some(bye)
            };

            (msg, sa, transport, aid, remote_uri, direction, is_dialing_or_ringing)
        };

        if let Some(msg) = action_msg {
            transport.send_to(msg.as_bytes(), server_addr).await?;
        }

        let (media, late_bye) = {
            let mut s = self.state.write().await;
            let Some((aid, call)) = s.find_call_mut(call_id) else {
                return Ok(());
            };
            let aid = aid.to_string();
            // The 200 OK may have arrived while the CANCEL was going out, and
            // the response handler will have ACKed it. That dialog is now
            // confirmed and the CANCEL does nothing to it: only a BYE ends it.
            let late_bye = if sent_cancel && call.is_established() {
                let cseq = call.next_cseq();
                let (our_tag, their_tag) = call.dialog_tags();
                let target = call.remote_contact.clone().unwrap_or_else(|| call.remote_uri.clone());
                let route_set = call.route_set().to_vec();
                let (call_id_header, local_uri, remote_uri) =
                    (call.call_id_header.clone(), call.local_uri.clone(), call.remote_uri.clone());
                let account = s.get_account(&aid).ok_or("Account not found")?;
                let la = account.local_addr.ok_or("No local addr")?;
                Some(build_bye_with_routes(
                    &target,
                    la,
                    &call_id_header,
                    cseq,
                    &our_tag,
                    &their_tag,
                    account.config.transport.param(),
                    &route_set,
                    &local_uri,
                    &remote_uri,
                ))
            } else {
                None
            };
            let (_, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
            let media = call.take_media();
            let _ = call.process(CallFSMEvent::LocalHangup);
            (media, late_bye)
        };
        if let Some(bye) = late_bye {
            log::info!("Call {} was answered as it was cancelled; sending BYE", call_id);
            let _ = transport.send_to(bye.as_bytes(), server_addr).await;
        }
        let recording_path = finish_call_media(call_id, media).await;

        let mut ended = CallEvent::new(&account_id, call_id, "ended", &remote_uri, &direction);
        if let Some(path) = recording_path {
            ended = ended.with_recording_path(path);
        }
        self.emit(SipEvent::CallStateChanged(ended));

        // The record is pruned by `start_ended_call_sweeper`, not here: it has
        // to outlive the call long enough to answer a 200 OK that crossed the
        // CANCEL, or a retransmitted final response.

        Ok(())
    }

    pub async fn answer(&self, call_id: &str) -> Result<(), String> {
        let (raw_invite, local_addr, server_addr, rtp_port, to_tag, username, remote_uri, account_id, transport, srtp_mode) = {
            let s = self.state.read().await;
            let (account, call) = s.find_call(call_id).ok_or("Call not found")?;
            if !call.is_incoming() {
                return Err("Call is not waiting to be answered".into());
            }
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            let user = account.config.username.clone();
            let account_id = account.config.id.clone();

            (
                call.raw_invite().ok_or("No raw INVITE")?.to_string(),
                la,
                sa,
                call.local_rtp_port,
                call.to_tag.clone().unwrap_or_else(builder::generate_tag),
                user,
                call.remote_uri.clone(),
                account_id,
                transport,
                account.config.srtp_mode,
            )
        };

        // Media first, 200 OK second. The 200 used to go out before the audio
        // devices were opened; when they failed (microphone permission denied,
        // device busy) the PBX had a confirmed dialog while this side still
        // had an unanswered call, so hangup answered 486 to a finished
        // transaction and the caller sat in silence. A call we cannot carry
        // audio for is declined instead.
        let sdp = raw_invite.split("\r\n\r\n").nth(1).unwrap_or("");
        let Some((ip, port)) = parse_sdp_connection(sdp) else {
            self.decline_incoming(call_id, &raw_invite, &to_tag, 488, "Not Acceptable Here").await;
            return Err("The caller offered no media".into());
        };
        let remote_rtp: SocketAddr = match format!("{}:{}", ip, port).parse() {
            Ok(addr) => addr,
            Err(e) => {
                self.decline_incoming(call_id, &raw_invite, &to_tag, 488, "Not Acceptable Here").await;
                return Err(format!("Bad RTP addr: {}", e));
            }
        };
        let negotiated_codec = codec::negotiate_codec(sdp);
        log::info!("Negotiated codec for inbound: {:?}", negotiated_codec);

        // SRTP answer: when the account offers SDES and the caller's offer is
        // RTP/SAVP with a key, generate our own key, answer encrypted, and run
        // the session with both contexts. Inbound calls used to be answered
        // RTP/AVP whatever the offer said, so a call to an SRTP extension was
        // always plaintext — and a PBX that requires SRTP rejected it.
        let remote_srtp_key = builder::parse_sdp_crypto(sdp);
        let use_srtp = srtp_mode == crate::sip::account::SrtpMode::Sdes
            && builder::sdp_offers_savp(sdp)
            && remote_srtp_key.is_some();
        let our_srtp_key = if use_srtp {
            match rtp_engine::srtp::SrtpContext::generate() {
                Ok((_, key)) => Some(key),
                Err(e) => {
                    log::error!("Could not generate SRTP key for answer: {:?}", e);
                    None
                }
            }
        } else {
            None
        };
        // Encrypt only when we have both a key to send and the context pair built.
        let answer_crypto = our_srtp_key.clone();

        let (input_dev, output_dev) = {
            let s = self.state.read().await;
            (s.preferred_input_device.clone(), s.preferred_output_device.clone())
        };

        let media_result = match (&our_srtp_key, &remote_srtp_key) {
            (Some(our_key), Some(their_key)) => {
                match (
                    rtp_engine::srtp::SrtpContext::from_base64(our_key),
                    rtp_engine::srtp::SrtpContext::from_base64(their_key),
                ) {
                    (Ok(tx), Ok(rx)) => {
                        log::info!("Answering incoming call with SRTP");
                        media::MediaSession::start_with_srtp_keys_and_devices(
                            rtp_port, remote_rtp, negotiated_codec, tx, rx, input_dev, output_dev,
                        )
                        .await
                    }
                    _ => {
                        log::error!("Could not build SRTP contexts; answering plain");
                        media::MediaSession::start_with_devices(
                            rtp_port, remote_rtp, negotiated_codec, input_dev, output_dev,
                        )
                        .await
                    }
                }
            }
            _ => {
                media::MediaSession::start_with_devices(
                    rtp_port, remote_rtp, negotiated_codec, input_dev, output_dev,
                )
                .await
            }
        };
        // Encrypted only when both keys were present and both contexts built.
        let srtp_active = use_srtp && answer_crypto.is_some() && media_result.is_ok();
        let media = match media_result {
            Ok(media) => media,
            Err(e) => {
                log::error!("Could not start audio for incoming call {}: {}", call_id, e);
                self.decline_incoming(call_id, &raw_invite, &to_tag, 480, "Temporarily Unavailable").await;
                return Err(format!("Could not start audio: {}", e));
            }
        };

        // Audio and DTMF go on the payload types the caller's offer named.
        media::apply_negotiated_payload_types(&media, sdp);

        // Discover public IP via STUN for NAT traversal in SDP
        // A server on a private network is answered with the local address:
        // don't even ask STUN, whose answer it could not use.
        let stun_ip = if media::is_private_network(server_addr.ip()) {
            None
        } else {
            let stun_servers = self.state.read().await.stun_servers.clone();
            match media::discover_public_ip(&stun_servers).await {
                Ok(ip) => Some(ip),
                Err(e) => {
                    log::warn!("STUN discovery failed for answer: {}", e);
                    None
                }
            }
        };
        let sdp_ip = media::sdp_address(local_addr.ip(), stun_ip, Some(server_addr.ip()));
        let public_ip = Some(sdp_ip.to_string());

        // Our crypto line only when the session actually came up encrypted,
        // so the answer never claims SRTP the media is not using.
        let answer_crypto = if srtp_active { answer_crypto.as_deref() } else { None };
        let response = build_200_ok_invite_srtp(
            &raw_invite,
            local_addr,
            rtp_port,
            &to_tag,
            Some(&username),
            public_ip.as_deref(),
            answer_crypto,
        )
        .ok_or("Failed to build 200 OK")?;
        let invite_cseq = cseq_number(&raw_invite).unwrap_or(1);

        // Built when auto-record starts. Emitted after the call-state event
        // below, so the frontend has the call in its list before it is told the
        // call is being recorded.
        let recording_event = {
            let mut s = self.state.write().await;
            let (_, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
            let result = call.process(CallFSMEvent::LocalAnswer { media, remote_rtp });
            if !matches!(result, state::call::TransitionResult::Ok) {
                // The caller gave up while the devices were opening. The CANCEL
                // handler has answered the INVITE; there is nothing to say yes
                // to, and the UI has had its ended event.
                return Err("Call ended before it could be answered".into());
            }
            call.set_to_tag(to_tag.clone());
            call.pending_2xx = Some(state::Pending2xx { cseq: invite_cseq, response: response.clone() });
            call.srtp_active = srtp_active;
            // Remember our key for the session, so a re-INVITE refresh answers
            // with the same crypto line instead of downgrading to plain RTP.
            if srtp_active {
                call.local_srtp_key = our_srtp_key.clone();
            }
            // And the address this answer carries, so a re-INVITE is answered
            // with the same one the media is already flowing to.
            call.sdp_public_ip = Some(sdp_ip);

            // Only once the session is on the call, so a recording is never
            // started for a call that is already gone.
            s.find_call(call_id)
                .and_then(|(_, call)| call.media())
                .and_then(|media| s.start_auto_record(&account_id, call_id, media))
        };

        transport.send_to(response.as_bytes(), server_addr).await?;

        // RFC 3261 §13.3.1.4: the 200 is ours to deliver. Once the caller has
        // our 180 it stops resending its INVITE, so a 200 lost on the wire
        // used to leave the caller ringing and this side "connected" (#69).
        let timing = self.state.read().await.timing;
        let mgr = self.clone();
        let internal_id = call_id.to_string();
        tokio::spawn(async move {
            let acked = retransmit_2xx_until_ack(
                &mgr.state, &internal_id, invite_cseq, &response, &transport, server_addr, timing,
            )
            .await;
            if !acked {
                log::warn!(
                    "No ACK for the 200 OK to call {} within {} ms; ending the call",
                    internal_id,
                    (timing.t1 * 64).as_millis()
                );
                // The UAS core SHOULD end the dialog with a BYE (§13.3.1.4).
                let _ = mgr.hangup(&internal_id).await;
            }
        });

        self.emit(SipEvent::CallStateChanged(
            CallEvent::new(&account_id, call_id, "connected", &remote_uri, "inbound")
                .with_srtp_active(srtp_active),
        ));

        if let Some(event) = recording_event {
            self.emit(SipEvent::RecordingStateChanged(event));
        }

        Ok(())
    }

    /// Turn down an incoming call that cannot be answered, with a final
    /// response to its INVITE, and report it ended.
    async fn decline_incoming(&self, call_id: &str, raw_invite: &str, to_tag: &str, code: u16, reason: &str) {
        let sent = {
            let mut s = self.state.write().await;
            let Some((aid, call)) = s.find_call_mut(call_id) else { return };
            let aid = aid.to_string();
            if !call.is_incoming() {
                return;
            }
            let remote_uri = call.remote_uri.clone();
            call.end(EndReason::Failed(format!("{code} {reason}")));
            let account = s.get_account(&aid);
            let transport = account.and_then(|a| a.transport.clone());
            let server_addr = account.and_then(|a| a.server_addr);
            (aid, remote_uri, transport, server_addr)
        };
        let (aid, remote_uri, transport, server_addr) = sent;
        if let (Some(transport), Some(server_addr), Some(resp)) =
            (transport, server_addr, builder::build_final_response(raw_invite, code, reason, to_tag))
        {
            let _ = transport.send_to(resp.as_bytes(), server_addr).await;
        }
        self.emit(SipEvent::CallStateChanged(
            CallEvent::new(&aid, call_id, "ended", &remote_uri, "inbound"),
        ));
    }

    /// Put the call on hold, or take it off.
    ///
    /// The FSM's `hold()`/`unhold()` move the call between Connected and
    /// Held and mute the media. This used to feed the FSM `CallFSMEvent::Hold`
    /// instead, which the transition table treats as a no-op, so the backend
    /// kept reporting a held call as "connected"; the UI did not notice
    /// because it tracks hold itself. The PBX end-to-end test asserts the
    /// state from the status report, which is how it surfaced.
    pub async fn hold(&self, call_id: &str, hold: bool) -> Result<(), String> {
        let mut s = self.state.write().await;
        let (_, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
        let result = if hold { call.hold() } else { call.unhold() };
        if let state::call::TransitionResult::InvalidTransition { from, event } = result {
            // Already there, or not in a call that can be held: not an error
            // for the caller, same as before.
            log::warn!("Call {call_id}: {event} ignored in state {from}");
        }
        Ok(())
    }

    pub async fn mute(&self, call_id: &str, mute: bool) -> Result<(), String> {
        let s = self.state.read().await;
        if let Some((_, call)) = s.find_call(call_id) {
            if let Some(media) = call.media() {
                media.set_mute(mute);
            }
            Ok(())
        } else {
            Err("Call not found".into())
        }
    }

    pub async fn send_dtmf(&self, call_id: &str, digit: &str) -> Result<(), String> {
        log::info!("DTMF: {} on call {}", digit, call_id);
        let s = self.state.read().await;
        let (_, call) = s.find_call(call_id).ok_or("Call not found")?;
        if let Some(media) = call.media() {
            media.send_dtmf(digit);
        } else {
            return Err("No active media session".into());
        }
        Ok(())
    }

    /// Start recording the call audio
    pub async fn start_recording(&self, call_id: &str, recordings_dir: &std::path::Path) -> Result<String, String> {
        log::info!("Starting recording for call {}", call_id);
        let s = self.state.read().await;
        let (_, call) = s.find_call(call_id).ok_or("Call not found")?;
        if let Some(media) = call.media() {
            let output_path = rtp_engine::generate_recording_filename(call_id, recordings_dir);
            media.start_recording(output_path.clone())
                .map_err(|e| format!("Failed to start recording: {}", e))?;
            Ok(output_path.to_string_lossy().to_string())
        } else {
            Err("No active media session".into())
        }
    }

    /// Stop recording the call audio
    ///
    /// Saving writes the whole call's audio to disk synchronously, and this
    /// used to do it under the state read lock. tokio's lock is fair, so the
    /// first writer to queue behind it — the receive loop with a BYE, a
    /// refresh timer, a hangup — stopped every other reader too, stalling all
    /// SIP for the length of the write. The session is lifted out of the call
    /// for the duration instead; the call stays up throughout.
    pub async fn stop_recording(&self, call_id: &str) -> Result<Option<String>, String> {
        log::info!("Stopping recording for call {}", call_id);
        let media = {
            let mut s = self.state.write().await;
            let (_, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
            if !call.is_established() {
                return Err("No active media session".into());
            }
            call.take_media().ok_or("No active media session")?
        };

        let id = call_id.to_string();
        let (media, path) = tokio::task::spawn_blocking(move || {
            let path = save_recording(&id, &media);
            (media, path)
        })
        .await
        .map_err(|e| format!("Recording task failed: {}", e))?;

        // Put the session back. If the call ended meanwhile, its teardown
        // found no media to finish, so finish it here.
        let mut s = self.state.write().await;
        match s.find_call_mut(call_id) {
            Some((_, call)) if call.is_established() => call.set_media(media),
            _ => media.stop(),
        }
        Ok(path?.map(|p| p.to_string_lossy().into_owned()))
    }

    /// Check if the call is being recorded
    pub async fn is_recording(&self, call_id: &str) -> Result<bool, String> {
        let s = self.state.read().await;
        let (_, call) = s.find_call(call_id).ok_or("Call not found")?;
        if let Some(media) = call.media() {
            Ok(media.is_recording())
        } else {
            Ok(false)
        }
    }

    /// Blind transfer: send REFER to transfer the remote party to target_uri
    #[allow(dead_code)]
    pub async fn transfer_blind(&self, call_id: &str, target_uri: &str) -> Result<(), String> {
        let (call_info, local_addr, server_addr, transport_str, account_id, transport) = {
            let mut s = self.state.write().await;
            let (aid, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
            let aid = aid.to_string();

            if !call.is_connected() && !call.is_held() {
                return Err("Call not in connected or held state".into());
            }

            let cseq = call.next_cseq();
            // Ours in From, theirs in To, whichever way the call was set up.
            let (our_tag, their_tag) = call.dialog_tags();
            let info = (
                call.remote_contact.clone().unwrap_or_else(|| call.remote_uri.clone()),
                call.remote_uri.clone(),
                call.call_id_header.clone(),
                cseq,
                our_tag,
                their_tag,
                call.route_set().to_vec(),
                call.local_uri.clone(),
            );

            let account = s.get_account(&aid).ok_or("Account not found")?;
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let tp = account.config.transport.param().to_string();
            let transport = account.transport.clone().ok_or("No transport")?;

            (info, la, sa, tp, aid, transport)
        };

        let (request_target, remote_uri, sip_call_id, cseq, from_tag, to_tag, route_set, local_uri) = call_info;

        let refer = build_refer(
            &request_target,
            target_uri,
            local_addr,
            &sip_call_id,
            cseq,
            &from_tag,
            &to_tag,
            &transport_str,
            &route_set,
            &local_uri,
            &remote_uri,
        );

        transport.send_to(refer.as_bytes(), server_addr).await?;


        log::info!(
            "Sent blind REFER to transfer call {} -> {}",
            call_id,
            target_uri
        );

        self.emit(SipEvent::TransferProgress(TransferEvent {
            account_id,
            call_id: call_id.to_string(),
            status: 100,
            message: "Transfer initiated".into(),
        }));

        Ok(())
    }

    /// Attended transfer: send REFER with Replaces to connect two existing calls
    #[allow(dead_code)]
    pub async fn transfer_attended(&self, call_id_a: &str, call_id_b: &str) -> Result<(), String> {
        let (call_a_info, call_b_info, local_addr, server_addr, transport_str, account_id, transport) = {
            let mut s = self.state.write().await;

            let (_, call_b) = s.find_call(call_id_b).ok_or("Call B not found")?;
            // Replaces names B's dialog from the transfer target's side (RFC
            // 3891 §3): to-tag is the tag that party chose, which is the
            // remote tag from our side, and from-tag is ours.
            let (b_our_tag, b_their_tag) = call_b.dialog_tags();
            let b_info = (
                call_b.remote_uri.clone(),
                call_b.call_id_header.clone(),
                b_their_tag,
                b_our_tag,
            );

            let (aid, call_a) = s.find_call_mut(call_id_a).ok_or("Call A not found")?;
            let aid = aid.to_string();

            if !call_a.is_connected() && !call_a.is_held() {
                return Err("Call A not in connected or held state".into());
            }

            let cseq = call_a.next_cseq();
            let (a_our_tag, a_their_tag) = call_a.dialog_tags();
            let a_info = (
                call_a.remote_contact.clone().unwrap_or_else(|| call_a.remote_uri.clone()),
                call_a.remote_uri.clone(),
                call_a.call_id_header.clone(),
                cseq,
                a_our_tag,
                a_their_tag,
                call_a.route_set().to_vec(),
                call_a.local_uri.clone(),
            );

            let account = s.get_account(&aid).ok_or("Account not found")?;
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let tp = account.config.transport.param().to_string();
            let transport = account.transport.clone().ok_or("No transport")?;

            (a_info, b_info, la, sa, tp, aid, transport)
        };

        let (a_target, a_remote_uri, a_sip_call_id, a_cseq, a_from_tag, a_to_tag, a_route_set, a_local_uri) = call_a_info;
        let (b_remote_uri, b_sip_call_id, b_replaces_to_tag, b_replaces_from_tag) = call_b_info;

        let refer = build_refer_with_replaces(
            &a_target,
            &b_remote_uri,
            &b_sip_call_id,
            &b_replaces_to_tag,
            &b_replaces_from_tag,
            local_addr,
            &a_sip_call_id,
            a_cseq,
            &a_from_tag,
            &a_to_tag,
            &transport_str,
            &a_route_set,
            &a_local_uri,
            &a_remote_uri,
        );

        transport.send_to(refer.as_bytes(), server_addr).await?;


        log::info!(
            "Sent attended REFER to transfer call {} (with Replaces for call {})",
            call_id_a,
            call_id_b
        );

        self.emit(SipEvent::TransferProgress(TransferEvent {
            account_id,
            call_id: call_id_a.to_string(),
            status: 100,
            message: "Attended transfer initiated".into(),
        }));

        Ok(())
    }

    /// Merge multiple calls into a local conference with audio mixing
    pub async fn conference_merge(&self, call_ids: &[String]) -> Result<String, String> {
        if call_ids.len() < 2 {
            return Err("Need at least 2 calls to create a conference".into());
        }

        let conference_id = format!("conf-{}", uuid::Uuid::new_v4());
        
        let mut s = self.state.write().await;
        let mut media_count = 0;
        let mut account_id = None;
        
        // Validate all calls exist and have active media
        for call_id in call_ids {
            if let Some((aid, call)) = s.find_call_mut(call_id) {
                if call.media().is_some() {
                    media_count += 1;
                    if account_id.is_none() {
                        account_id = Some(aid.to_string());
                    }
                }
            } else {
                return Err(format!("Call {} not found", call_id));
            }
        }
        
        if media_count < 2 {
            return Err("Need at least 2 calls with active media".into());
        }
        
        // Enable conference mode on all media sessions
        for call_id in call_ids {
            if let Some((_, call)) = s.find_call_mut(call_id) {
                if let Some(media) = call.media() {
                    media.set_mute(false);
                    media.set_conference_mode(true);
                }
            }
        }
        
        // Store conference state
        if let Some(aid) = &account_id {
            if let Some(account) = s.get_account_mut(aid) {
                account.conferences.insert(conference_id.clone(), call_ids.to_vec());
            }
        }
        
        log::info!("Created conference {} with {} calls", conference_id, call_ids.len());
        
        // Emit conference created event
        drop(s);
        self.emit(SipEvent::ConferenceCreated {
            conference_id: conference_id.clone(),
            call_ids: call_ids.to_vec(),
        });
        
        Ok(conference_id)
    }
    
    /// Split a call from a conference
    pub async fn conference_split(&self, conference_id: &str, call_id: &str) -> Result<(), String> {
        let (remaining_call_ids, conference_ended) = {
            let mut s = self.state.write().await;
            
            // Find and update the conference
            let mut found_conference = false;
            let mut remaining = Vec::new();
            let mut ended = false;
            
            for account in s.accounts.values_mut() {
                if let Some(call_ids) = account.conferences.get_mut(conference_id) {
                    call_ids.retain(|id| id != call_id);
                    remaining = call_ids.clone();
                    found_conference = true;
                    
                    // If only one call left, end the conference
                    if call_ids.len() < 2 {
                        account.conferences.remove(conference_id);
                        ended = true;
                    }
                    break;
                }
            }
            
            if !found_conference {
                return Err("Conference not found".into());
            }
            
            // Disable conference mode and put the split call on hold
            if let Some((_, call)) = s.find_call_mut(call_id) {
                if let Some(media) = call.media() {
                    media.set_conference_mode(false);
                    media.set_mute(true);
                }
            }
            
            (remaining, ended)
        };
        
        // If conference ended, disable conference mode for remaining call
        if conference_ended && !remaining_call_ids.is_empty() {
            let mut s = self.state.write().await;
            for remaining_id in &remaining_call_ids {
                if let Some((_, call)) = s.find_call_mut(remaining_id) {
                    if let Some(media) = call.media() {
                        media.set_conference_mode(false);
                    }
                }
            }
        }
        
        log::info!("Split call {} from conference {}", call_id, conference_id);
        
        // Emit event
        self.emit(SipEvent::ConferenceSplit {
            conference_id: conference_id.to_string(),
            call_id: call_id.to_string(),
        });
        
        Ok(())
    }
    
    /// End a conference and hang up all calls in it
    pub async fn conference_end(&self, conference_id: &str) -> Result<(), String> {
        let call_ids_to_hangup = {
            let s = self.state.read().await;
            let mut call_ids = Vec::new();
            for account in s.accounts.values() {
                if let Some(ids) = account.conferences.get(conference_id) {
                    call_ids = ids.clone();
                    break;
                }
            }
            call_ids
        };
        
        // Hang up each call
        for call_id in &call_ids_to_hangup {
            if let Err(e) = self.hangup(call_id).await {
                log::error!("Failed to hang up call {} in conference: {}", call_id, e);
            }
        }
        
        // Remove conference
        {
            let mut s = self.state.write().await;
            for account in s.accounts.values_mut() {
                account.conferences.remove(conference_id);
            }
        }
        
        log::info!("Ended conference {} with {} calls", conference_id, call_ids_to_hangup.len());
        
        // Emit event
        self.emit(SipEvent::ConferenceEnded {
            conference_id: conference_id.to_string(),
        });
        
        Ok(())
    }
    
    /// Swap between two calls (put one on hold, resume other)
    pub async fn swap_calls(&self, hold_call_id: &str, resume_call_id: &str) -> Result<(), String> {
        // Put the first call on hold
        self.hold(hold_call_id, true).await?;
        
        // Resume the second call
        self.hold(resume_call_id, false).await?;
        
        log::info!("Swapped calls: held {}, resumed {}", hold_call_id, resume_call_id);
        
        Ok(())
    }

    pub async fn registration_state(&self) -> (RegistrationState, Option<String>) {
        self.registration_state_for_account(None).await
    }

    pub async fn registration_state_for_account(&self, account_id: Option<&str>) -> (RegistrationState, Option<String>) {
        let s = self.state.read().await;
        let aid = match account_id {
            Some(id) => id.to_string(),
            None => match s.active_account_id.clone().or_else(|| s.accounts.keys().next().cloned()) {
                Some(id) => id,
                None => return (RegistrationState::Unregistered, None),
            },
        };
        match s.get_account(&aid) {
            Some(account) => (
                account.registration.status(),
                account.registration.error_reason().map(|e| e.to_string()),
            ),
            None => (RegistrationState::Unregistered, None),
        }
    }

    pub async fn get_diagnostics(&self) -> Vec<DiagnosticLog> {
        let s = self.state.read().await;
        s.diagnostic_store.get_all().await
    }

    /// Tests only: put an entry into the diagnostic store as a transport would.
    #[cfg(test)]
    pub(crate) async fn push_diagnostic_for_test(&self, entry: DiagnosticLog) {
        let s = self.state.read().await;
        s.diagnostic_store.push(entry).await;
    }

    /// Record something the app did (not a SIP message) in the trace, so it
    /// shows in the Diagnostics panel in sequence with the signalling.
    pub async fn log_app_event(&self, account_id: &str, summary: &str, detail: &str) {
        let diag = DiagnosticLog::app_event(account_id, summary, detail);
        {
            let s = self.state.read().await;
            s.diagnostic_store.push(diag.clone()).await;
        }
        self.emit(SipEvent::DiagnosticMessage(diag));
    }

    /// Every configured account, without credentials.
    pub async fn account_summaries(&self) -> Vec<AccountSummary> {
        let s = self.state.read().await;
        let mut out: Vec<AccountSummary> = s
            .accounts
            .values()
            .map(|a| AccountSummary {
                account_id: a.config.id.clone(),
                display_name: a.config.display_name.clone(),
                username: a.config.username.clone(),
                domain: a.config.domain.clone(),
                transport: format!("{:?}", a.config.transport).to_lowercase(),
                port: a.config.port,
                registrar: a.config.registrar.clone(),
                registration_state: a.registration.state_name().to_string(),
                registration_error: a.registration.error_reason().map(|e| e.to_string()),
                srtp_mode: format!("{:?}", a.config.srtp_mode).to_lowercase(),
                auto_record: a.config.auto_record,
                is_active: s.active_account_id.as_deref() == Some(a.config.id.as_str()),
            })
            .collect();
        out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        out
    }

    pub async fn clear_diagnostics(&self) {
        let s = self.state.read().await;
        s.diagnostic_store.clear().await;
    }

    pub async fn get_rtp_stats(&self) -> Option<media::RtpStats> {
        let s = self.state.read().await;
        s.all_active_calls()
            .iter()
            .find_map(|c| c.media().map(|m| m.get_stats()))
    }

    /// Whether echo cancellation is actually running on the call in progress.
    ///
    /// `None` when there is no call. `Some(false)` while the setting is on
    /// means it could not start — usually because the microphone and speaker
    /// report different sample rates; the engine log says which.
    pub async fn voice_processing_active(&self) -> Option<bool> {
        let s = self.state.read().await;
        s.all_active_calls()
            .iter()
            .find_map(|c| c.media().map(media::MediaSession::voice_processing_active))
    }

    /// Get live audio levels (TX mic, RX speaker) for the active call.
    pub async fn get_audio_levels(&self) -> Option<(f32, f32)> {
        let s = self.state.read().await;
        s.all_active_calls()
            .iter()
            .find_map(|c| c.media().map(|m| (m.tx_audio_level(), m.rx_audio_level())))
    }

    /// Probe registration health for all registered accounts.
    ///
    /// Sends an immediate OPTIONS ping to each account that thinks it is registered.
    /// If the send fails, triggers transport death and reconnection.
    /// Called by the frontend on wake from sleep, network online, or window re-focus.
    pub async fn probe_registration_health(&self) {
        let accounts_to_probe: Vec<(String, AccountConfig, SocketAddr, SocketAddr, SipTransport, u32)> = {
            let mut s = self.state.write().await;
            s.accounts.values_mut()
                .filter(|a| a.registration.status() == RegistrationState::Registered)
                .filter_map(|a| {
                    let la = a.local_addr?;
                    let sa = a.server_addr?;
                    let t = a.transport.clone()?;
                    a.options_cseq += 1;
                    Some((a.config.id.clone(), a.config.clone(), la, sa, t, a.options_cseq))
                })
                .collect()
        };

        if accounts_to_probe.is_empty() {
            return;
        }

        log::info!("Probing registration health for {} account(s)", accounts_to_probe.len());

        for (account_id, config, local_addr, server_addr, transport, cseq) in accounts_to_probe {
            let call_id = builder::generate_call_id();
            let from_tag = builder::generate_tag();
            let msg = build_options(
                &config.domain,
                local_addr,
                &call_id,
                cseq,
                &from_tag,
                &config.username,
                config.transport.param(),
            );

            if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                log::warn!("Health probe failed for {} — triggering reconnect: {}", account_id, e);
                Self::handle_transport_death(
                    self.state.clone(),
                    self.event_tx.clone(),
                    account_id,
                    false,
                ).await;
            } else {
                log::info!("Health probe OK for {}", account_id);
            }
        }
    }

    /// Re-subscribe to all BLF/presence targets that were active before reconnect.
    /// Called by the frontend after it sees RegistrationState::Registered.
    pub async fn process_pending_resubscriptions(&self) -> Vec<String> {
        let targets: Vec<(String, crate::sip::state::EventType)> = {
            let mut s = self.state.write().await;
            let aid = s.active_account_id.clone()
                .or_else(|| s.accounts.keys().next().cloned());
            if let Some(ref aid) = aid {
                if let Some(account) = s.get_account_mut(aid) {
                    std::mem::take(&mut account.pending_resubscriptions)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        };

        let mut results = Vec::new();
        for (uri, event_type) in targets {
            match self.subscribe_presence(&uri, event_type).await {
                Ok(id) => {
                    log::info!("Re-subscribed to {} (sub_id={})", uri, id);
                    results.push(uri);
                }
                Err(e) => {
                    log::warn!("Failed to re-subscribe to {}: {}", uri, e);
                }
            }
        }
        results
    }

    /// Subscribe to presence/dialog events for a target URI (RFC 6665)
    pub async fn subscribe_presence(
        &self,
        target_uri: &str,
        event_type: EventType,
    ) -> Result<String, String> {
        let (account_config, local_addr, server_addr, transport, aid) = {
            let s = self.state.read().await;
            let aid = s.active_account_id.clone()
                .or_else(|| s.accounts.keys().next().cloned())
                .ok_or("No account configured")?;
            let account = s.get_account(&aid).ok_or("Account not found")?;
            if account.registration.status() != RegistrationState::Registered {
                return Err("Not registered".into());
            }
            let local_addr = account.local_addr.ok_or("No local addr")?;
            let server_addr = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            (account.config.clone(), local_addr, server_addr, transport, aid)
        };

        let call_id = builder::generate_call_id();
        let from_tag = builder::generate_tag();
        let cseq = 1u32;
        let expires = 600u32;

        let sub = Subscription {
            id: uuid::Uuid::new_v4().to_string(),
            target_uri: target_uri.to_string(),
            event_type: event_type.clone(),
            state: SubscriptionState::Pending,
            expires,
            cseq,
            call_id: call_id.clone(),
            from_tag: from_tag.clone(),
            to_tag: None,
            auth_attempts: 0,
            expires_at: None,
        };

        let sub_id = sub.id.clone();

        let msg = build_subscribe(
            &account_config,
            target_uri,
            local_addr,
            &call_id,
            cseq,
            &from_tag,
            None,
            &event_type,
            expires,
            None,
        );

        // On the list before the request goes out: a challenge that beat the
        // insertion found no subscription and was dropped, leaving it Pending
        // for good.
        {
            let mut s = self.state.write().await;
            if let Some(account) = s.get_account_mut(&aid) {
                account.subscriptions.push(sub);
            }
        }

        if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
            let mut s = self.state.write().await;
            if let Some(account) = s.get_account_mut(&aid) {
                account.subscriptions.retain(|s| s.id != sub_id);
            }
            return Err(e);
        }

        log::info!(
            "Sent SUBSCRIBE for {} (event={})",
            target_uri,
            event_type.event_header()
        );

        Ok(sub_id)
    }

    /// Unsubscribe by sending SUBSCRIBE with Expires: 0
    pub async fn unsubscribe(&self, subscription_id: &str) -> Result<(), String> {
        let (account_config, local_addr, server_addr, transport, sub_info, aid) = {
            let mut s = self.state.write().await;
            let aid = s.active_account_id.clone()
                .or_else(|| s.accounts.keys().next().cloned())
                .ok_or("No account configured")?;
            let account = s.get_account_mut(&aid).ok_or("Account not found")?;
            let sub = account.subscriptions
                .iter_mut()
                .find(|s| s.id == subscription_id)
                .ok_or("Subscription not found")?;
            sub.cseq += 1;
            // Expires 0 on the record too, so a 200 that arrives before the
            // entry is removed is not mistaken for a renewal.
            sub.expires = 0;
            let info = (
                sub.target_uri.clone(),
                sub.call_id.clone(),
                sub.cseq,
                sub.from_tag.clone(),
                sub.to_tag.clone(),
                sub.event_type.clone(),
            );
            let local_addr = account.local_addr.ok_or("No local addr")?;
            let server_addr = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            (account.config.clone(), local_addr, server_addr, transport, info, aid)
        };

        let (target_uri, call_id, cseq, from_tag, to_tag, event_type) = sub_info;

        let msg = build_subscribe(
            &account_config,
            &target_uri,
            local_addr,
            &call_id,
            cseq,
            &from_tag,
            to_tag.as_deref(),
            &event_type,
            0,
            None,
        );

        transport.send_to(msg.as_bytes(), server_addr).await?;


        log::info!("Sent SUBSCRIBE (Expires: 0) for {}", target_uri);

        {
            let mut s = self.state.write().await;
            if let Some(account) = s.get_account_mut(&aid) {
                let extension = account.subscriptions
                    .iter()
                    .find(|sub| sub.id == subscription_id)
                    .map(|sub| presence::extract_extension_from_uri(&sub.target_uri));
                account.subscriptions.retain(|s| s.id != subscription_id);
                if let Some(ext) = extension {
                    account.blf_states.remove(&ext);
                }
            }
        }

        Ok(())
    }

    /// Get current BLF states for all subscriptions
    pub async fn get_blf_states(&self) -> Vec<BlfEntry> {
        let s = self.state.read().await;
        s.active_account()
            .map(|a| a.blf_states.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Get the registered domain
    pub async fn get_domain(&self) -> Option<String> {
        let s = self.state.read().await;
        s.active_account().map(|a| a.config.domain.clone())
    }

    /// Find a subscription ID by extension (user part of the target URI)
    pub async fn find_subscription_by_extension(&self, extension: &str) -> Option<String> {
        let s = self.state.read().await;
        s.active_account().and_then(|a| {
            a.subscriptions
                .iter()
                .filter(|sub| sub.event_type != EventType::MessageSummary)
                .find(|sub| presence::extract_extension_from_uri(&sub.target_uri) == extension)
                .map(|sub| sub.id.clone())
        })
    }

    /// Get rich system status for the diagnostics panel
    pub async fn get_system_status(&self) -> SystemStatus {
        let s = self.state.read().await;

        let accounts: Vec<AccountStatus> = s.accounts.values().map(|account| {
            let registration_state = account.registration.state_name().to_string();
            let registration_error = account.registration.error_reason().map(|e| e.to_string());
            let server_address = account.server_addr.map(|a| a.to_string());
            let transport_type = account.transport.as_ref().map(|t| match t {
                SipTransport::Udp(_) => "UDP".to_string(),
                SipTransport::Tcp(_) => "TCP".to_string(),
                SipTransport::Tls(_) => "TLS".to_string(),
            });
            let local_address = account.transport.as_ref().map(|t| {
                let raw = t.local_addr();
                let ip = raw.ip();
                let port = raw.port();
                if ip.is_unspecified() {
                    if let Some(server) = account.server_addr {
                        let real_ip = std::net::UdpSocket::bind("0.0.0.0:0")
                            .and_then(|sock| { sock.connect(server)?; sock.local_addr() })
                            .map(|a| a.ip())
                            .unwrap_or(ip);
                        format!("{}:{}", real_ip, port)
                    } else {
                        format!("{}:{}", ip, port)
                    }
                } else {
                    format!("{}:{}", ip, port)
                }
            });
            let public_address = account.public_addr.map(|a| a.to_string());
            let uptime_secs = if let state::RegistrationState::Registered { registered_at, .. } =
                account.registration.state()
            {
                Some(registered_at.elapsed().as_secs())
            } else {
                None
            };

            let account_id = account.config.id.clone();
            let active_calls: Vec<CallStatusInfo> = account.calls
                .iter()
                .map(|c| {
                    let duration_secs = c.connected_at().map(|t| t.elapsed().as_secs());
                    let (codec, rtp_stats) = c
                        .media()
                        .map(|m| (Some(format!("{:?}", m.get_codec())), Some(m.get_stats())))
                        .unwrap_or((None, None));
                    CallStatusInfo {
                        id: c.id.clone(),
                        account_id: account_id.clone(),
                        remote_uri: c.remote_uri.clone(),
                        state: c.state_name().to_lowercase(),
                        direction: c.direction_str().to_lowercase(),
                        duration_secs,
                        codec,
                        rtp_stats,
                        srtp_active: c.srtp_active,
                        sip_call_id: c.call_id_header.clone(),
                    }
                })
                .collect();

            AccountStatus {
                account_id,
                username: account.config.username.clone(),
                domain: account.config.domain.clone(),
                registration_state,
                registration_error,
                server_address,
                transport_type,
                local_address,
                public_address,
                uptime_secs,
                active_calls,
            }
        }).collect();

        let total_active_calls = s.all_active_calls().len();

        SystemStatus {
            accounts,
            latency_ms: s.last_latency_ms,
            total_active_calls,
        }
    }

    /// Refresh subscriptions before the server's grant runs out, and send the
    /// renewals of ones the server terminated.
    ///
    /// Refreshes go inside the existing subscription (with the server's tag)
    /// at half of the granted lifetime. The old timer re-sent every
    /// subscription every 60 s, without the tag, so each refresh opened a new
    /// subscription at the server while the original ran down.
    fn start_subscribe_refresh_timer(&self) {
        let state = self.state.clone();

        tokio::spawn(async move {
            loop {
                let tick = state.read().await.timing.subscribe_tick;
                tokio::time::sleep(tick).await;

                // (account id, subscription call-id) of everything due.
                let due: Vec<(String, String)> = {
                    let s = state.read().await;
                    s.accounts
                        .iter()
                        .filter(|(_, a)| a.registration.status() == RegistrationState::Registered)
                        .flat_map(|(aid, a)| {
                            a.subscriptions
                                .iter()
                                .filter(|sub| sub.refresh_due())
                                .map(move |sub| (aid.clone(), sub.call_id.clone()))
                        })
                        .collect()
                };

                for (aid, call_id) in due {
                    let sent = {
                        let mut s = state.write().await;
                        let Some(account) = s.get_account_mut(&aid) else { continue };
                        let (Some(local_addr), Some(server_addr), Some(transport)) =
                            (account.local_addr, account.server_addr, account.transport.clone())
                        else {
                            continue;
                        };
                        let config = account.config.clone();
                        let Some(sub) = account.subscriptions.iter_mut().find(|s| s.call_id == call_id) else {
                            continue;
                        };
                        sub.cseq += 1;
                        if sub.expires == 0 {
                            continue;
                        }
                        let msg = build_subscribe(
                            &config,
                            &sub.target_uri,
                            local_addr,
                            &sub.call_id,
                            sub.cseq,
                            &sub.from_tag,
                            sub.to_tag.as_deref(),
                            &sub.event_type,
                            sub.expires,
                            None,
                        );
                        // Not due again until this one is answered.
                        sub.expires_at = sub.expires_at.map(|_| {
                            std::time::Instant::now() + std::time::Duration::from_secs(u64::from(sub.expires))
                        });
                        (msg, transport, server_addr, sub.target_uri.clone(), sub.cseq)
                    };
                    let (msg, transport, server_addr, target_uri, cseq) = sent;
                    if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                        log::error!("SUBSCRIBE refresh failed for {}: {}", target_uri, e);
                    } else {
                        log::debug!("Sent SUBSCRIBE refresh for {} (cseq={})", target_uri, cseq);
                    }
                }
            }
        });
    }

    /// Is this datagram/frame from the peer we are actually talking to?
    ///
    /// The UDP socket is unconnected, so `recv_from` will happily hand us
    /// packets from anywhere on the internet. Without this check any host that
    /// can reach our ephemeral SIP port can ring the phone, tear down calls
    /// (BYE), drive outbound dialling (REFER), force auto-answer
    /// (INVITE/Replaces), or inject responses that redirect our media.
    ///
    /// This client only ever talks to one registrar / outbound proxy per
    /// account, so requiring the source to equal the resolved server address is
    /// both correct and sufficient.
    async fn is_expected_peer(
        state: &Arc<RwLock<ManagerState>>,
        account_id: &str,
        remote: SocketAddr,
    ) -> bool {
        let s = state.read().await;
        match s.accounts.get(account_id).and_then(|a| a.server_addr) {
            Some(server) => server == remote,
            // No server address resolved yet — nothing legitimate can arrive.
            None => false,
        }
    }

    /// Run the receive loop for one transport generation, and treat its end
    /// as the transport dying — unless the account has since moved on to a
    /// newer transport, in which case this loop is simply obsolete.
    fn start_receive_loop_for_account(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        rx: mpsc::Receiver<SipMessage>,
        account_id: String,
        generation: u64,
    ) {
        tokio::spawn(async move {
            let died = Self::run_receive_loop(state.clone(), event_tx.clone(), account_id.clone(), rx, generation).await;
            if died {
                log::warn!("Receive loop ended for account {} — transport dead", account_id);
                Self::handle_transport_death(state, event_tx, account_id, false).await;
            }
        });
    }

    /// Handle transport death: clean up stale state, end active calls, and start reconnection.
    ///
    /// With `keep_calls`, established calls are left in place: a refresh
    /// REGISTER that went unanswered says the registrar is unreachable, not
    /// that the call's media has stopped, and the new transport can still
    /// carry the BYE.
    fn handle_transport_death(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        account_id: String,
        keep_calls: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        Box::pin(async move {
        let ended_calls = {
            let mut s = state.write().await;
            let account = match s.accounts.get_mut(&account_id) {
                Some(a) => a,
                None => return, // Account removed (user unregistered) — do nothing
            };

            let was_active = matches!(
                account.registration.status(),
                RegistrationState::Registered | RegistrationState::Registering
            );

            if !was_active || account.unregistering {
                // Already in error/unregistered/reconnecting — don't double-trigger
                return;
            }

            // An emergency call outlives its transport: its dialog is kept, and
            // the reconnect starts at the target it was on, so a BYE or
            // re-INVITE goes to the same server over the new connection.
            let keep_calls = if emergency_call_active(account) {
                if let Some(target) = account.target.clone() {
                    account.failover.prefer(&target);
                }
                true
            } else {
                keep_calls
            };

            // Transition to reconnecting
            account.registration.transport_lost();
            account.refreshing = false;
            // The old transport's loops are obsolete; the reconnect starts new ones.
            account.timer_generation += 1;

            // End all active calls for this account. Their media is finished
            // after the lock is released, below.
            let ended_calls: Vec<(CallEventPayload, Option<media::MediaSession>)> = if keep_calls {
                Vec::new()
            } else {
                account
                    .calls
                    .drain(..)
                    .map(|mut call| {
                        let event = CallEventPayload::new(
                            &account_id,
                            &call.id,
                            "ended",
                            &call.remote_uri,
                            call.direction_str(),
                        );
                        (event, call.take_media())
                    })
                    .collect()
            };

            // Save subscription targets for re-subscription after reconnect
            let resub_targets: Vec<(String, crate::sip::state::EventType)> = account
                .subscriptions
                .iter()
                .map(|s| (s.target_uri.clone(), s.event_type.clone()))
                .collect();
            account.pending_resubscriptions = resub_targets;

            // Clear stale transport state
            account.transport = None;
            account.subscriptions.clear();
            account.conferences.clear();
            account.blf_states.clear();
            // Reset realm fallback so reconnect gets a fresh auth attempt
            account.realm_fallback = None;
            account.realm_fallback_exhausted = false;

            ended_calls
        };

        for (mut event, media) in ended_calls {
            if let Some(path) = finish_call_media(&event.call_id, media).await {
                event = event.with_recording_path(path);
            }
            let _ = event_tx.send(SipEvent::CallStateChanged(event));
        }

        // Emit reconnecting state to UI
        let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
            account_id: account_id.clone(),
            state: RegistrationState::Reconnecting,
            error: Some("Connection lost — reconnecting...".into()),
        }));
        with_report(&state, &event_tx, &account_id, |r| {
            // Keep what went wrong (a refresh that got no reply, say) so the
            // panel can show why it is reconnecting.
            let why = r.error.take();
            r.restart();
            r.error = why;
            r.outcome = connection::Outcome::Reconnecting;
            r.reconnect_attempt = 0;
        })
        .await;

        // Start auto-reconnect with exponential backoff (spawned as a new task)
        tokio::spawn(Self::start_reconnect_loop(state, event_tx, account_id));
        })
    }

    /// Reconnection loop: walks the account's server list, backing off once
    /// every target has failed, and never gives up.
    ///
    /// The first attempt is immediate when the list has a target left to try
    /// (the one that just failed has been marked), and otherwise waits what the
    /// list says: the backoff, or a server's `Retry-After`, whichever is
    /// longer.
    async fn start_reconnect_loop(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        account_id: String,
    ) {
        const BACKOFF_SECS: &[u64] = &[1, 2, 4, 8, 16, 30];

        let mut delay = {
            let s = state.read().await;
            match s.accounts.get(&account_id) {
                Some(a) if a.failover.has_alternative(std::time::Instant::now()) => std::time::Duration::ZERO,
                _ => std::time::Duration::from_secs(BACKOFF_SECS[0]),
            }
        };

        for attempt in 0u32.. {
            // Check if account still exists and is still in reconnecting state
            {
                let s = state.read().await;
                match s.accounts.get(&account_id) {
                    Some(account) => {
                        let status = account.registration.status();
                        if status != RegistrationState::Reconnecting {
                            // User manually unregistered or re-registered — stop reconnecting
                            log::info!(
                                "Reconnect loop stopping for {} — state changed to {:?}",
                                account_id,
                                status
                            );
                            return;
                        }
                    }
                    None => {
                        // Account removed — stop reconnecting
                        log::info!("Reconnect loop stopping for {} — account removed", account_id);
                        return;
                    }
                }
            }

            log::info!(
                "Reconnect attempt {} for {} — waiting {}s",
                attempt + 1,
                account_id,
                delay.as_secs()
            );

            // Update attempt counter in state
            {
                let mut s = state.write().await;
                if let Some(account) = s.accounts.get_mut(&account_id) {
                    account.registration.reconnect_attempt();
                }
            }
            with_report(&state, &event_tx, &account_id, |r| r.reconnect_attempt = attempt + 1).await;

            let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                account_id: account_id.clone(),
                state: RegistrationState::Reconnecting,
                error: Some(format!("Reconnecting (attempt {})...", attempt + 1)),
            }));

            // Shortened in tests, where the backoff would otherwise dominate.
            let delay_now = {
                let s = state.read().await;
                delay.min(s.timing.max_wait)
            };
            if !delay_now.is_zero() {
                tokio::time::sleep(delay_now).await;
            }

            // Re-check state after sleeping (user may have unregistered during the wait)
            let account_config = {
                let s = state.read().await;
                match s.accounts.get(&account_id) {
                    Some(account) if account.registration.status() == RegistrationState::Reconnecting => {
                        // The configured transport, which `Auto` leaves to DNS.
                        let mut cfg = account.config.clone();
                        cfg.transport = account.configured_transport.clone();
                        cfg
                    }
                    _ => {
                        log::info!("Reconnect loop stopping for {} — state changed during backoff", account_id);
                        return;
                    }
                }
            };

            // Attempt to reconnect: resolve, connect transport, send REGISTER
            match Self::attempt_reconnect(&state, &event_tx, &account_id, &account_config).await {
                Ok((rx, generation)) => {
                    log::info!("Reconnect transport established for {} — waiting for registration", account_id);
                    Self::start_receive_loop_for_account(
                        state.clone(),
                        event_tx.clone(),
                        rx,
                        account_id.clone(),
                        generation,
                    );
                    // The refresh and keepalive loops exited when the old
                    // transport died. Without new ones the account looked
                    // registered but its binding expired and its NAT pinhole
                    // closed; nothing noticed until the next network event.
                    Self::start_account_timers(state, event_tx, account_id, generation);
                    return;
                }
                Err(e) => {
                    log::warn!("Reconnect attempt {} failed for {}: {}", attempt + 1, account_id, e.message);
                    // The list says how long (backoff, `Retry-After`); a
                    // failure that is not about the targets, DNS say, backs
                    // off by attempt.
                    delay = e.wait.unwrap_or_else(|| {
                        std::time::Duration::from_secs(BACKOFF_SECS[(attempt as usize + 1).min(BACKOFF_SECS.len() - 1)])
                    });
                }
            }
        }
    }

    /// Core receive loop logic. Returns true if the transport died while it
    /// was still the account's current one.
    async fn run_receive_loop(
        state: Arc<RwLock<ManagerState>>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        account_id: String,
        mut rx: mpsc::Receiver<SipMessage>,
        generation: u64,
    ) -> bool {
        let mut dropped_from_stranger = 0u64;
        let mut check = tokio::time::interval(std::time::Duration::from_secs(2));

        loop {
            let msg = tokio::select! {
                msg = rx.recv() => match msg {
                    Some(msg) => msg,
                    None => return true,
                },
                _ = check.tick() => {
                    // Obsolete once the account is gone or has a newer
                    // transport. Dropping `rx` here closes the socket's
                    // reader too; replaced transports used to live on.
                    let s = state.read().await;
                    if !Self::account_current(&s, &account_id, generation) {
                        log::debug!("Receive loop for {} generation {} retired", account_id, generation);
                        return false;
                    }
                    continue;
                }
            };

            // Drop anything that did not come from the account's own server.
            // See `is_expected_peer` — without this, arbitrary internet hosts
            // can drive the SIP state machine.
            if !Self::is_expected_peer(&state, &account_id, msg.remote).await {
                dropped_from_stranger += 1;
                // Rate-limit the log so a flood cannot fill the disk, and do
                // not put unsolicited traffic into the diagnostic store.
                if dropped_from_stranger.is_power_of_two() {
                    log::warn!(
                        "Dropped {} SIP message(s) for account {} from unexpected source {} \
                         (expected the registered server)",
                        dropped_from_stranger,
                        account_id,
                        msg.remote
                    );
                }
                continue;
            }

            let text = match String::from_utf8(msg.data) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let redacted = diagnostics::redact_sip(&text);
            log::debug!(
                "Received SIP for account {} from {}:\n{}",
                account_id,
                msg.remote,
                redacted
            );

            let diag = DiagnosticLog {
                timestamp: diagnostics::now_millis(),
                account_id: account_id.clone(),
                direction: diagnostics::MessageDirection::Received,
                remote_addr: msg.remote.to_string(),
                summary: diagnostics::summarize_sip(&text),
                raw: redacted,
                call_id: diagnostics::extract_sip_call_id(&text),
            };
            {
                let s = state.read().await;
                s.diagnostic_store.push(diag.clone()).await;
            }
            let _ = event_tx.send(SipEvent::DiagnosticMessage(diag));

            if is_request(&text) {
                handlers::handle_incoming_request(&state, &event_tx, &text, msg.remote, &account_id).await;
            } else {
                handlers::handle_response(&state, &event_tx, &text, &account_id).await;
            }
        }
    }

    /// Attempt a single reconnection: resolve DNS if the list has gone stale,
    /// connect to the first target that accepts, send REGISTER.
    /// Returns the new receive channel and transport generation on success.
    async fn attempt_reconnect(
        state: &Arc<RwLock<ManagerState>>,
        event_tx: &mpsc::UnboundedSender<SipEvent>,
        account_id: &str,
        account: &AccountConfig,
    ) -> Result<(mpsc::Receiver<SipMessage>, u64), ConnectFailure> {
        // The transport an emergency call is on is not replaced; this attempt
        // (and any REGISTER behind it) waits until the call ends.
        {
            let s = state.read().await;
            if s.accounts.get(account_id).is_some_and(|a| emergency_call_active(a) && a.transport.is_some()) {
                return Err(ConnectFailure {
                    message: "an emergency call is in progress".into(),
                    wait: Some(std::time::Duration::from_secs(1)),
                });
            }
        }
        // Another attempt of the same account: the steps start over and the
        // report says it is a reconnect, with the attempt number.
        let mut report = {
            let s = state.read().await;
            let mut r = s
                .connection_reports
                .get(account_id)
                .cloned()
                .unwrap_or_else(|| connection::ConnectionReport::start(account));
            r.restart();
            r.outcome = connection::Outcome::Reconnecting;
            r.reconnect_attempt = s
                .accounts
                .get(account_id)
                .map_or(0, |a| a.registration.reconnect_attempt_count());
            r
        };
        let mut failover = {
            let s = state.read().await;
            s.accounts.get(account_id).map(|a| a.failover.clone()).unwrap_or_default()
        };
        let connected = resolve_and_connect(state, event_tx, account, &mut report, &mut failover).await;
        let Connected { server_addr, transport, rx, local_addr, target } = match connected {
            Ok(v) => v,
            Err(e) => {
                // Keep the failures and the round count for the next attempt.
                if let Some(a) = state.write().await.accounts.get_mut(account_id) {
                    a.failover = failover;
                }
                // The backend is still trying; the panel must say so,
                // with the failed step left visible until the next attempt.
                with_report(state, event_tx, account_id, |r| {
                    r.outcome = connection::Outcome::Reconnecting;
                })
                .await;
                return Err(e);
            }
        };

        let generation = {
            let mut s = state.write().await;
            let account_state = match s.accounts.get_mut(account_id) {
                Some(a) => a,
                None => return Err("Account removed during reconnect".to_string().into()),
            };

            let effective = with_transport(account, &target.transport);
            account_state.config = effective.clone();
            account_state.configured_transport = account.transport.clone();
            account_state.failover = failover;
            account_state.target = Some(target);
            account_state.flow_timer = None;
            account_state.registration.start_registration(effective);
            account_state.registration.set_server_addr(server_addr);
            account_state.registration.set_local_addr(local_addr);
            account_state.transport = Some(transport);
            account_state.server_addr = Some(server_addr);
            account_state.local_addr = Some(local_addr);
            account_state.refreshing = false;
            // A reconnect that gets no answer is still a connection problem.
            account_state.on_register_timeout = RegisterTimeoutPolicy::Reconnect;
            account_state.timer_generation += 1;
            account_state.timer_generation
        };

        let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
            account_id: account_id.to_string(),
            state: RegistrationState::Registering,
            error: None,
        }));

        send_register(state, event_tx, account_id, None).await?;
        log::info!("Sent reconnect REGISTER for {} to {}", account_id, server_addr);

        Ok((rx, generation))
    }
}


/// Create the account's transport and work out the local address other
/// hosts can reach it on. `account.transport` is the concrete transport for
/// this target, and `tls_name` the name its certificate is checked against.
async fn open_transport_for(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account: &AccountConfig,
    server_addr: SocketAddr,
    tls_name: &str,
) -> Result<(SipTransport, mpsc::Receiver<SipMessage>, SocketAddr), String> {
    log::info!("Creating {:?} transport to {}", account.transport, server_addr);
    // One target must not hold up the next: a black-holed address would
    // otherwise cost the transport's own 10 s per step.
    let (mut transport, rx) = match account.transport {
        TransportType::Udp | TransportType::Auto => {
            let (t, rx) = transport::UdpTransport::bind("0.0.0.0:0").await?;
            (SipTransport::Udp(t), rx)
        }
        TransportType::Tcp => {
            let (t, rx) = tokio::time::timeout(srv::CONNECT_TIMEOUT, transport::TcpTransport::connect(server_addr))
                .await
                .map_err(|_| format!("TCP connect to {server_addr} timed out"))??;
            (SipTransport::Tcp(t), rx)
        }
        TransportType::Tls => {
            let (t, rx) = tokio::time::timeout(
                srv::CONNECT_TIMEOUT,
                transport::TlsTransport::connect(server_addr, tls_name, account.tls_insecure),
            )
            .await
            .map_err(|_| format!("TLS handshake with {server_addr} timed out"))?
            .inspect_err(|e| log::error!("TLS connection failed: {}", e))?;
            (SipTransport::Tls(t), rx)
        }
    };

    // Attach diagnostic sender so all outbound messages are logged automatically
    {
        let s = state.read().await;
        let diag_sender = diagnostics::DiagnosticSender::new(
            s.diagnostic_store.clone(),
            event_tx.clone(),
            account.id.clone(),
        );
        transport.set_diagnostic_sender(diag_sender);
    }

    // Resolve `0.0.0.0` to the actual outbound interface IP
    let raw_addr = transport.local_addr();
    let local_addr = if raw_addr.ip().is_unspecified() {
        let bind: &str = if server_addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        let real_ip = std::net::UdpSocket::bind(bind)
            .and_then(|s| { s.connect(server_addr)?; s.local_addr() })
            .map(|a| a.ip())
            .unwrap_or(raw_addr.ip());
        std::net::SocketAddr::new(real_ip, raw_addr.port())
    } else {
        raw_addr
    };
    Ok((transport, rx, local_addr))
}

/// A transport opened to one target of the account's server list.
struct Connected {
    server_addr: SocketAddr,
    transport: SipTransport,
    rx: mpsc::Receiver<SipMessage>,
    local_addr: SocketAddr,
    target: srv::Target,
}

/// Why no target could be connected to, and how long to wait before asking
/// the list again (`None`: not a matter of the targets, so the caller's own
/// backoff applies).
struct ConnectFailure {
    message: String,
    wait: Option<std::time::Duration>,
}

impl From<String> for ConnectFailure {
    fn from(message: String) -> Self {
        Self { message, wait: None }
    }
}

/// The account as it is configured to use `transport`, which is the target's.
fn with_transport(account: &AccountConfig, transport: &TransportType) -> AccountConfig {
    let mut effective = account.clone();
    effective.transport = transport.clone();
    effective
}

/// What the account's server list is a list *for*. A different answer to this
/// means the list is for some other server.
fn failover_key(account: &AccountConfig) -> String {
    let (host, port) = SipManager::target_endpoint(account);
    format!("{host}|{port:?}|{}", account.transport.param_static_or_auto())
}

/// Resolve the server (when the list is missing or past its TTL) and connect
/// to the first target that accepts a connection, recording both as steps of
/// `report`. A target that cannot be connected to is marked failed in
/// `failover` and the next one is tried at once. On failure the report is
/// stored and pushed with the failed step before the error is returned, so
/// the panel can show what broke even when no `AccountState` exists yet.
///
/// `account.transport` is the *configured* transport, possibly `Auto`.
async fn resolve_and_connect(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account: &AccountConfig,
    report: &mut connection::ConnectionReport,
    failover: &mut srv::Failover,
) -> Result<Connected, ConnectFailure> {
    use connection::StepKind;

    let (target_host, target_port) = SipManager::target_endpoint(account);
    let key = failover_key(account);
    if failover.key != key {
        *failover = srv::Failover::default();
        failover.key = key;
    }

    if failover.needs_resolve(std::time::Instant::now()) {
        report.running(StepKind::Resolve);
        save_report(state, event_tx, report).await;
        let started = tokio::time::Instant::now();
        let dns = state.read().await.dns.clone();
        match srv::resolve(&*dns, target_host, target_port, &account.transport, &mut srv::random_draw).await {
            Ok(resolved) => {
                let elapsed = started.elapsed();
                if target_host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok() {
                    report.skipped(StepKind::Resolve, "literal address");
                } else {
                    let first = &resolved.targets[0];
                    let how = if resolved.via_srv { "SRV" } else { "A/AAAA" };
                    report.ok(
                        StepKind::Resolve,
                        format!(
                            "{} target(s) via {how}, first {} ({}) in {} ms",
                            resolved.targets.len(),
                            first.addr,
                            first.host,
                            elapsed.as_millis()
                        ),
                        Some(elapsed),
                    );
                }
                failover.install(resolved, std::time::Instant::now());
            }
            // An answer we already hold outlives a lookup that failed.
            Err(e) if !failover.is_empty() => {
                log::warn!("DNS refresh for {target_host} failed ({e}); keeping the list we have");
            }
            Err(e) => {
                report.failed(
                    StepKind::Resolve,
                    connection::classify_resolve(&e),
                    e.clone(),
                    Some(started.elapsed()),
                );
                save_report(state, event_tx, report).await;
                return Err(e.into());
            }
        }
    } else {
        report.skipped(StepKind::Resolve, "server list still valid");
    }

    let mut last_error = String::new();
    loop {
        let target = match failover.next_action(std::time::Instant::now()) {
            srv::Action::Try(t) => t,
            srv::Action::Wait(wait) => {
                let message = if last_error.is_empty() {
                    format!("No server of {target_host} can be tried right now")
                } else {
                    last_error
                };
                return Err(ConnectFailure { message, wait: Some(wait) });
            }
        };
        let effective = with_transport(account, &target.transport);
        let tls_name = account.tls_name_for(failover.via_srv(), failover.lookup_host()).to_string();

        report.running(StepKind::Connect);
        save_report(state, event_tx, report).await;
        let started = tokio::time::Instant::now();
        match open_transport_for(state, event_tx, &effective, target.addr, &tls_name).await {
            Ok((transport, rx, local_addr)) => {
                let elapsed = started.elapsed();
                let ms = elapsed.as_millis();
                let detail = match effective.transport {
                    TransportType::Udp | TransportType::Auto => {
                        format!("socket open on {local_addr}; UDP cannot confirm the server is reachable")
                    }
                    TransportType::Tcp => format!("connected in {ms} ms"),
                    TransportType::Tls if effective.tls_insecure => {
                        format!("handshake in {ms} ms, certificate not checked (allow untrusted is on)")
                    }
                    TransportType::Tls => format!("handshake in {ms} ms, certificate OK for {tls_name}"),
                };
                report.ok(StepKind::Connect, detail, Some(elapsed));
                save_report(state, event_tx, report).await;
                return Ok(Connected { server_addr: target.addr, transport, rx, local_addr, target });
            }
            Err(e) => {
                log::warn!("Connect to {} ({}) failed: {}", target.addr, target.host, e);
                failover.report_failure(&target, &srv::Failure::Connect, std::time::Instant::now());
                report.failed(
                    StepKind::Connect,
                    connection::classify_connect(&effective.transport, &e),
                    e.clone(),
                    Some(started.elapsed()),
                );
                save_report(state, event_tx, report).await;
                last_error = e;
            }
        }
    }
}

/// Store `report` as the account's current one and push it to the UI.
async fn save_report(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    report: &connection::ConnectionReport,
) {
    state
        .write()
        .await
        .connection_reports
        .insert(report.account_id.clone(), report.clone());
    push_report(event_tx, report);
}

/// Change the account's report under the lock and push the result. A missing
/// report (no attempt was ever recorded) is left missing.
pub(crate) async fn with_report(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account_id: &str,
    edit: impl FnOnce(&mut connection::ConnectionReport),
) {
    let updated = {
        let mut s = state.write().await;
        s.connection_reports.get_mut(account_id).map(|r| {
            edit(r);
            r.clone()
        })
    };
    if let Some(report) = updated {
        push_report(event_tx, &report);
    }
}

/// Push a connection report to the UI from code that has no `&SipManager`.
pub(crate) fn push_report(
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    report: &connection::ConnectionReport,
) {
    let _ = event_tx.send(SipEvent::ConnectionReport(report.clone()));
}

/// Send a REGISTER for `account_id` and see its transaction through.
///
/// Every REGISTER — first, authenticated retry, refresh, unregister — goes
/// through here, so each gets the same retransmission (Timer A, over UDP) and
/// the same deadline (Timer F). Only the first one used to: a refresh whose
/// request or answer was lost left the account "registering" for good, with
/// nothing left running that could get it out, while the binding expired.
///
/// Retransmission stops when the FSM leaves Registering or moves on to a
/// later CSeq (a challenge was answered). What the deadline means is decided
/// by the account's `on_register_timeout`.
pub(crate) async fn send_register(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account_id: &str,
    auth_header: Option<&str>,
) -> Result<(), String> {
    let (msg, transport, server_addr, cseq, timing) = {
        let mut s = state.write().await;
        let mut timing = s.timing;
        let account = s.get_account_mut(account_id).ok_or("Account not found")?;
        // Timer F is 32 s, far too long to sit on a server that is not
        // answering when there may be another to try. That applies to the
        // first REGISTER of an attempt; a refresh keeps the full Timer F
        // unless another target is there to move to.
        if !account.unregistering
            && (!account.refreshing || account.failover.has_alternative(std::time::Instant::now()))
        {
            timing.timer_f = timing.timer_f.min(srv::REGISTER_FAILOVER_TIMEOUT);
        }
        let local_addr = account.local_addr.ok_or("No local addr")?;
        let server_addr = account.server_addr.ok_or("No server addr")?;
        let transport = account.transport.clone().ok_or("No transport")?;
        let cseq = account.registration.next_cseq();
        let expires = if account.unregistering { 0 } else { account.requested_expires };
        let call_id = account.registration.call_id().to_string();
        let from_tag = account.registration.local_tag().to_string();
        let msg = build_register(&account.config, local_addr, &call_id, cseq, &from_tag, auth_header, expires);
        let first_of_attempt = auth_header.is_none() && !account.refreshing && !account.unregistering;
        if first_of_attempt {
            if let Some(report) = s.connection_reports.get_mut(account_id) {
                report.running(connection::StepKind::Register);
            }
        }
        (msg, transport, server_addr, cseq, timing)
    };
    transport.send_to(msg.as_bytes(), server_addr).await?;
    if let Some(report) = state.read().await.connection_reports.get(account_id) {
        push_report(event_tx, report);
    }

    let state = state.clone();
    let event_tx = event_tx.clone();
    let aid = account_id.to_string();
    tokio::spawn(async move {
        // No retransmission over a reliable transport; just the deadline.
        let reliable = !matches!(transport, SipTransport::Udp(_));
        let started = tokio::time::Instant::now();
        let mut interval = timing.t1;
        let mut sent: u32 = 1;
        loop {
            let remaining = timing.timer_f.saturating_sub(started.elapsed());
            let wait = if reliable { remaining } else { interval.min(remaining) };
            tokio::time::sleep(wait).await;

            let outstanding = {
                let s = state.read().await;
                s.accounts.get(&aid).is_some_and(|a| {
                    a.registration.is_registering() && a.registration.current_cseq() == cseq
                })
            };
            if !outstanding {
                return;
            }
            if started.elapsed() >= timing.timer_f {
                register_timed_out(&state, &event_tx, &aid, sent, started.elapsed()).await;
                return;
            }
            log::debug!("Retransmitting REGISTER cseq {} for {}", cseq, aid);
            if transport.send_to(msg.as_bytes(), server_addr).await.is_err() {
                register_timed_out(&state, &event_tx, &aid, sent, started.elapsed()).await;
                return;
            }
            sent += 1;
            // T2 caps the interval at 4 s.
            interval = (interval * 2).min(std::time::Duration::from_secs(4));
        }
    });
    Ok(())
}

/// The CSeq number of a SIP message.
pub(crate) fn cseq_number(msg: &str) -> Option<u32> {
    extract_header(msg, "CSeq")?.split_whitespace().next()?.parse().ok()
}

/// RFC 3261 T2: the longest interval between retransmissions.
const T2: std::time::Duration = std::time::Duration::from_secs(4);

/// See a 2xx to an incoming INVITE through to its ACK (RFC 3261 §13.3.1.4).
///
/// Over UDP the response is resent at T1, doubling up to T2 (Timer G). Over
/// a reliable transport the transport delivers it, so it is only waited on.
/// Returns `true` once the ACK has cleared the call's pending 2xx, or the
/// call has ended or moved on (nothing left to deliver); `false` when 64*T1
/// (Timer H) passed with no ACK, which is the caller's to act on.
pub(crate) async fn retransmit_2xx_until_ack(
    state: &Arc<RwLock<ManagerState>>,
    call_id: &str,
    cseq: u32,
    response: &str,
    transport: &SipTransport,
    dest: SocketAddr,
    timing: Timing,
) -> bool {
    let reliable = !matches!(transport, SipTransport::Udp(_));
    let timer_h = timing.t1 * 64;
    let started = tokio::time::Instant::now();
    let mut interval = timing.t1;
    loop {
        let remaining = timer_h.saturating_sub(started.elapsed());
        let wait = if reliable { remaining } else { interval.min(remaining) };
        tokio::time::sleep(wait).await;

        let outstanding = {
            let s = state.read().await;
            s.find_call(call_id)
                .is_some_and(|(_, c)| !c.is_ended() && c.pending_2xx.as_ref().is_some_and(|p| p.cseq == cseq))
        };
        if !outstanding {
            return true;
        }
        if started.elapsed() >= timer_h {
            if let Some((_, c)) = state.write().await.find_call_mut(call_id) {
                c.pending_2xx = None;
            }
            return false;
        }
        log::debug!("No ACK yet for call {}; retransmitting the 200 OK", call_id);
        if let Err(e) = transport.send_to(response.as_bytes(), dest).await {
            log::warn!("Could not retransmit the 200 OK for call {}: {}", call_id, e);
        }
        interval = (interval * 2).min(T2);
    }
}

/// A REGISTER got no final response within Timer F.
async fn register_timed_out(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account_id: &str,
    sent: u32,
    elapsed: std::time::Duration,
) {
    let policy = {
        let s = state.read().await;
        match s.accounts.get(account_id) {
            // A first REGISTER that goes unanswered is a failure to report
            // only when there is nowhere else to go; with another server in
            // the list it is a reason to move on.
            Some(a)
                if a.registration.is_registering()
                    && a.on_register_timeout == RegisterTimeoutPolicy::Error
                    && a.failover.has_alternative(std::time::Instant::now()) =>
            {
                RegisterTimeoutPolicy::Reconnect
            }
            Some(a) if a.registration.is_registering() => a.on_register_timeout,
            _ => return,
        }
    };
    if policy != RegisterTimeoutPolicy::Forget {
        let detail = format!("no reply in {} s (sent {} times)", elapsed.as_secs(), sent);
        with_report(state, event_tx, account_id, |r| {
            r.failed(
                connection::StepKind::Register,
                connection::FailureKind::Timeout,
                detail,
                Some(elapsed),
            );
            if policy == RegisterTimeoutPolicy::Reconnect {
                r.outcome = connection::Outcome::Reconnecting;
            }
        })
        .await;
    }
    match policy {
        RegisterTimeoutPolicy::Forget => {
            log::warn!("Unregister for {} was not answered", account_id);
            finish_unregister(state, event_tx, account_id).await;
        }
        RegisterTimeoutPolicy::Reconnect => {
            log::warn!("REGISTER for {} was not answered; trying another server", account_id);
            fail_over(state, event_tx, account_id, srv::Failure::Timeout, true).await;
        }
        RegisterTimeoutPolicy::Error => {
            let error = {
                let mut s = state.write().await;
                let Some(account) = s.accounts.get_mut(account_id) else { return };
                account.registration.registration_timeout();
                account.transport = None;
                account.registration.error_reason().map(str::to_owned)
            };
            let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                account_id: account_id.to_string(),
                state: RegistrationState::Error,
                error,
            }));
        }
    }
}

/// An emergency call is up on this account. While it is, nothing replaces or
/// closes the transport it runs on: reconnect, failover and failback wait.
fn emergency_call_active(account: &AccountState) -> bool {
    account.calls.iter().any(|c| !c.is_ended() && emergency::is_emergency(&c.remote_uri))
}

/// Whether a server answers at all, without registering anything: a connect
/// for TCP and TLS (the TLS handshake included, so the certificate is checked
/// as it will be), an OPTIONS for UDP.
async fn probe_target(
    account: &AccountConfig,
    target: &srv::Target,
    tls_name: &str,
    local_addr: SocketAddr,
) -> bool {
    match target.transport {
        TransportType::Tcp => {
            matches!(
                tokio::time::timeout(srv::CONNECT_TIMEOUT, tokio::net::TcpStream::connect(target.addr)).await,
                Ok(Ok(_))
            )
        }
        TransportType::Tls => matches!(
            tokio::time::timeout(
                srv::CONNECT_TIMEOUT,
                transport::TlsTransport::connect(target.addr, tls_name, account.tls_insecure),
            )
            .await,
            Ok(Ok(_))
        ),
        TransportType::Udp | TransportType::Auto => {
            let bind = if target.addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
            let Ok(sock) = tokio::net::UdpSocket::bind(bind).await else { return false };
            let Ok(sock_addr) = sock.local_addr() else { return false };
            let msg = build_options(
                &account.domain,
                SocketAddr::new(local_addr.ip(), sock_addr.port()),
                &builder::generate_call_id(),
                1,
                &builder::generate_tag(),
                &account.username,
                "udp",
            );
            if sock.send_to(msg.as_bytes(), target.addr).await.is_err() {
                return false;
            }
            let mut buf = [0u8; 2048];
            matches!(
                tokio::time::timeout(std::time::Duration::from_secs(2), sock.recv_from(&mut buf)).await,
                Ok(Ok((_, from))) if from == target.addr
            )
        }
    }
}

/// The account's current server is no good: remember that, and let the
/// reconnect loop move to the next one (or back off, when it was the last).
pub(crate) async fn fail_over(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account_id: &str,
    failure: srv::Failure,
    keep_calls: bool,
) {
    {
        let mut s = state.write().await;
        if let Some(account) = s.accounts.get(account_id) {
            if emergency_call_active(account) && account.transport.is_some() {
                log::warn!("{account_id}: {failure:?}, but an emergency call is up; failing over after it");
                // Queued: the same failure is acted on when the call has ended.
                let (state, event_tx, id) = (state.clone(), event_tx.clone(), account_id.to_string());
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        let waiting = state
                            .read()
                            .await
                            .accounts
                            .get(&id)
                            .is_some_and(|a| emergency_call_active(a) && a.transport.is_some());
                        if !waiting {
                            break;
                        }
                    }
                    {
                        let mut s = state.write().await;
                        if let Some(account) = s.accounts.get_mut(&id) {
                            if let Some(target) = account.target.clone() {
                                account.failover.report_failure(&target, &failure, std::time::Instant::now());
                            }
                        }
                    }
                    SipManager::handle_transport_death(state, event_tx, id, keep_calls).await;
                });
                return;
            }
        }
        if let Some(account) = s.accounts.get_mut(account_id) {
            if let Some(target) = account.target.clone() {
                log::warn!("Server {} ({}) failed for {}: {:?}", target.addr, target.host, account_id, failure);
                account.failover.report_failure(&target, &failure, std::time::Instant::now());
            }
        }
    }
    SipManager::handle_transport_death(state.clone(), event_tx.clone(), account_id.to_string(), keep_calls).await;
}

/// Forget an account and tell the UI it is unregistered.
pub(crate) async fn finish_unregister(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account_id: &str,
) {
    // The panel's last word on this account: it was taken down on purpose.
    with_report(state, event_tx, account_id, |r| r.finish(connection::Outcome::Unregistered)).await;
    {
        let mut s = state.write().await;
        if s.accounts.remove(account_id).is_none() {
            return;
        }
        if s.active_account_id.as_deref() == Some(account_id) {
            s.active_account_id = s.accounts.keys().next().cloned();
        }
    }
    let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
        account_id: account_id.to_string(),
        state: RegistrationState::Unregistered,
        error: None,
    }));
}

/// Whether two configurations register differently: a different server,
/// credentials or transport means the existing binding is for the wrong
/// thing and the account has to be registered afresh.
fn connection_changed(existing: &AccountState, next: &AccountConfig) -> bool {
    let mut current = existing.config.clone();
    // The transport in use differs from the configured one under `Auto`.
    current.transport = existing.configured_transport.clone();
    let current = &current;
    let key = |c: &AccountConfig| {
        (
            c.domain.clone(),
            c.registrar.clone(),
            c.outbound_proxy.clone(),
            c.port,
            c.transport.clone(),
            c.username.clone(),
            c.password.clone(),
            c.auth_username.clone(),
            c.auth_realm.clone(),
            c.tls_server_name.clone(),
            c.tls_insecure,
        )
    };
    key(current) != key(next)
}


#[cfg(test)]
mod construction_tests {
    use super::SipManager;

    /// The v0.5.0 launch panic: `run()` builds the manager on the bare main
    /// thread, where there is no Tokio runtime. Construction must not need
    /// one. Deliberately a plain `#[test]` on a fresh thread — `#[tokio::test]`
    /// would supply the runtime the app does not have at that point and hide
    /// the regression, which is why this test could not exist before the
    /// spawns moved out of the constructor.
    #[test]
    fn manager_constructs_without_a_runtime() {
        let built = std::thread::spawn(|| {
            let (manager, _rx) = SipManager::new_with_receiver();
            // A clone must not start anything either.
            let _handle = manager.clone();
        })
        .join();
        assert!(built.is_ok(), "constructing a SipManager without a runtime panicked");
    }

    /// `start_background_tasks` is the one place the loops start, and calling
    /// it twice (or on a clone) must not start them twice.
    #[tokio::test]
    async fn background_tasks_start_once() {
        let (manager, _rx) = SipManager::new_with_receiver();
        manager.start_background_tasks();
        manager.clone().start_background_tasks();
        assert!(manager.background_tasks.is_completed());
    }
}

#[cfg(test)]
mod call_id_tests {
    use super::SipManager;

    // The id becomes part of the recording filename, so anything that is not
    // a UUID must be refused before a call is created.
    #[tokio::test]
    async fn make_call_with_id_rejects_non_uuid_ids() {
        let (manager, _rx) = SipManager::new_with_receiver();
        for bad in ["../../etc/passwd", "call-1", ""] {
            let err = manager
                .make_call_with_id("sip:100@example.com", bad)
                .await
                .expect_err("non-UUID id must be rejected");
            assert!(err.contains("UUID"), "unexpected error for {:?}: {}", bad, err);
        }
    }
}
