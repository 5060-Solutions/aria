pub mod account;
pub mod auth;
pub mod builder;
pub mod codec;
pub mod diagnostics;
mod handlers;
#[allow(dead_code)]
pub mod ice;
pub mod media;
pub mod presence;
#[allow(dead_code)]
pub mod srtp;
pub mod state;
pub mod transfer;
pub mod transport;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

pub use account::AccountConfig;
pub use transport::TransportType;

use self::builder::{
    build_200_ok_invite_with_public_ip, build_bye_with_routes, build_cancel, build_options,
    build_refer, build_refer_with_replaces, build_register, build_subscribe, extract_via_branch,
    is_request, parse_sdp_connection,
};
use self::diagnostics::DiagnosticLog;
use self::media::MediaSessionExt;
use self::presence::{BlfEntry, EventType, SubscriptionState};
use self::state::{AccountState, CallFSM, CallFSMEvent, EndReason};
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
        }
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
}

pub struct SipManager {
    state: Arc<RwLock<ManagerState>>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
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
    CallStateChanged(CallEventPayload),
    DiagnosticMessage(DiagnosticLog),
    TransferProgress(TransferEvent),
    PresenceChanged(String, Vec<BlfEntry>), // account_id, entries
    ConferenceCreated { conference_id: String, call_ids: Vec<String> },
    ConferenceSplit { conference_id: String, call_id: String },
    ConferenceEnded { conference_id: String },
}

/// Manager state with multi-account support
///
/// Each account has its own independent state including registration,
/// transport, active calls, and presence subscriptions.
pub(crate) struct ManagerState {
    /// Per-account states, keyed by account ID
    pub(crate) accounts: HashMap<String, AccountState>,
    /// Currently active/default account ID for backward-compatible single-account operations
    pub(crate) active_account_id: Option<String>,

    // === Shared state across all accounts ===
    /// Last OPTIONS keepalive round-trip time in ms (from active account)
    pub(crate) last_latency_ms: Option<f64>,
    /// Shared diagnostic store
    pub(crate) diagnostic_store: Arc<diagnostics::DiagnosticStore>,
}

impl ManagerState {
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
}

impl SipManager {
    /// Create a new SipManager and return the event receiver separately.
    /// This allows the caller to set up event forwarding before managing the state.
    pub fn new_with_receiver() -> (Self, mpsc::UnboundedReceiver<SipEvent>) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let manager = Self {
            state: Arc::new(RwLock::new(ManagerState {
                accounts: HashMap::new(),
                active_account_id: None,
                last_latency_ms: None,
                diagnostic_store: Arc::new(diagnostics::DiagnosticStore::new(500)),
            })),
            event_tx,
        };
        (manager, event_rx)
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

    pub async fn register(&self, account: AccountConfig) -> Result<String, String> {
        let account_id = account.id.clone();

        // Check if this account is already registered/registering
        {
            let mut s = self.state.write().await;
            if let Some(existing) = s.accounts.get_mut(&account_id) {
                let status = existing.registration.status();
                if status == RegistrationState::Registering || status == RegistrationState::Registered {
                    // Update the config even if already registered (so SRTP mode, codecs, etc. take effect)
                    log::info!("Account {} already registered — updating config (SRTP mode: {:?})", 
                               account_id, account.srtp_mode);
                    existing.config = account;
                    return Ok("already_registering".into());
                }
            }
        }

        // Resolve server address with SRV lookup fallback to A-record
        let registrar = account.registrar.as_deref().unwrap_or(&account.domain);
        let server_addr = Self::resolve_server(registrar, &account.transport, account.port).await?;

        // Create transport based on account config
        log::info!("Creating {:?} transport to {}", account.transport, server_addr);
        let (mut transport, rx) = match account.transport {
            TransportType::Udp => {
                let (t, rx) = transport::UdpTransport::bind("0.0.0.0:0").await?;
                log::info!("UDP transport created, local addr: {}", t.local_addr());
                (SipTransport::Udp(t), rx)
            }
            TransportType::Tcp => {
                log::info!("Connecting TCP to {}...", server_addr);
                let (t, rx) = transport::TcpTransport::connect(server_addr).await?;
                log::info!("TCP transport connected, local addr: {}", t.local_addr());
                (SipTransport::Tcp(t), rx)
            }
            TransportType::Tls => {
                let tls_name = account.registrar.as_deref().unwrap_or(&account.domain);
                log::info!("Connecting TLS to {} (server name: {})...", server_addr, tls_name);
                match transport::TlsTransport::connect(server_addr, tls_name).await {
                    Ok((t, rx)) => {
                        log::info!("TLS transport connected, local addr: {}", t.local_addr());
                        (SipTransport::Tls(t), rx)
                    }
                    Err(e) => {
                        log::error!("TLS connection failed: {}", e);
                        return Err(e);
                    }
                }
            }
        };

        // Attach diagnostic sender so all outbound messages are logged automatically
        {
            let s = self.state.read().await;
            let diag_sender = diagnostics::DiagnosticSender::new(
                s.diagnostic_store.clone(),
                self.event_tx.clone(),
                account.id.clone(),
            );
            transport.set_diagnostic_sender(diag_sender);
        }

        // Resolve `0.0.0.0` to the actual outbound interface IP
        let raw_addr = transport.local_addr();
        let local_addr = if raw_addr.ip().is_unspecified() {
            let real_ip = std::net::UdpSocket::bind("0.0.0.0:0")
                .and_then(|s| { s.connect(server_addr)?; s.local_addr() })
                .map(|a| a.ip())
                .unwrap_or(raw_addr.ip());
            std::net::SocketAddr::new(real_ip, raw_addr.port())
        } else {
            raw_addr
        };

        log::info!(
            "Registering {}@{} via {:?} to {} port {} (local: {})",
            account.username,
            account.domain,
            account.transport,
            server_addr,
            account.port,
            local_addr
        );

        // Create or update account state
        let (call_id, from_tag, cseq) = {
            let mut state = self.state.write().await;
            
            // Create new account state or update existing
            if !state.accounts.contains_key(&account_id) {
                state.accounts.insert(account_id.clone(), AccountState::new(account.clone()));
            }
            
            // Set as active account if none set
            if state.active_account_id.is_none() {
                state.active_account_id = Some(account_id.clone());
            }
            
            // Now get mutable reference to account state
            let account_state = state.accounts.get_mut(&account_id).unwrap();
            
            // Update account state
            account_state.config = account.clone();
            account_state.registration.start_registration(account.clone());
            account_state.registration.set_server_addr(server_addr);
            account_state.registration.set_local_addr(local_addr);
            account_state.transport = Some(transport);
            account_state.server_addr = Some(server_addr);
            account_state.local_addr = Some(local_addr);
            
            (
                account_state.registration.call_id().to_string(),
                account_state.registration.local_tag().to_string(),
                account_state.registration.current_cseq(),
            )
        };

        self.emit_registration_event(&account_id, RegistrationState::Registering, None);

        let register_msg =
            build_register(&account, local_addr, &call_id, cseq, &from_tag, None, 3600);

        {
            let s = self.state.read().await;
            if let Some(account_state) = s.accounts.get(&account_id) {
                if let Some(ref t) = account_state.transport {
                    t.send_to(register_msg.as_bytes(), server_addr).await?;
                }
            }
        }


        log::debug!("Sent REGISTER:\n{}", register_msg);

        // Start the receive loop for this account
        self.start_receive_loop_for_account(rx, account_id.clone());

        // Start periodic re-REGISTER (refresh before expiry)
        self.start_re_register_timer_for_account(account_id.clone());

        // Start OPTIONS keepalive (every 30s)
        self.start_options_keepalive_for_account(account_id.clone());

        // Start session timer monitoring (RFC 4028)
        self.start_session_timer();

        // Start subscription refresh timer for BLF/presence
        self.start_subscribe_refresh_timer();

        // Registration timeout: if still registering after 30s, surface an error
        // (30s allows time for auth challenges and retries)
        {
            let state = self.state.clone();
            let event_tx = self.event_tx.clone();
            let aid = account_id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let status = {
                    let s = state.read().await;
                    s.accounts.get(&aid).map(|a| a.registration.status())
                };
                if status == Some(RegistrationState::Registering) {
                    let mut s = state.write().await;
                    if let Some(account_state) = s.accounts.get_mut(&aid) {
                        account_state.registration.registration_timeout();
                        let error = account_state.registration.error_reason().map(|e| e.to_string());
                        let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                            account_id: aid,
                            state: RegistrationState::Error,
                            error,
                        }));
                    }
                }
            });
        }

        Ok("registering".into())
    }

    /// DNS SRV lookup with fallback to A-record
    async fn resolve_server(
        registrar: &str,
        transport: &TransportType,
        port: u16,
    ) -> Result<SocketAddr, String> {
        // Try SRV lookup pattern: _sip._udp.domain, _sip._tcp.domain, _sips._tcp.domain
        let srv_name = match transport {
            TransportType::Udp => format!("_sip._udp.{}:{}", registrar, port),
            TransportType::Tcp => format!("_sip._tcp.{}:{}", registrar, port),
            TransportType::Tls => format!("_sips._tcp.{}:{}", registrar, port),
        };

        // 5 second timeout for SRV lookup
        if let Ok(Ok(mut addrs)) = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::lookup_host(&srv_name)
        ).await {
            if let Some(addr) = addrs.next() {
                log::info!("SRV lookup resolved {} -> {}", srv_name, addr);
                return Ok(addr);
            }
        }
        log::debug!(
            "SRV lookup failed for {}, falling back to A-record",
            srv_name
        );

        // Fallback: direct A-record lookup with 5 second timeout
        let addr_str = format!("{}:{}", registrar, port);
        let server_addr = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::lookup_host(&addr_str)
        )
            .await
            .map_err(|_| format!("DNS lookup timed out for {}", addr_str))?
            .map_err(|e| format!("DNS resolve failed for {}: {}", addr_str, e))?
            .next()
            .ok_or_else(|| format!("No address found for {}", addr_str))?;
        Ok(server_addr)
    }

    fn start_re_register_timer_for_account(&self, account_id: String) {
        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        let aid = account_id.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;

                let (account_config, local_addr, server_addr, reg_call_id, reg_from_tag, cseq, transport) = {
                    let mut s = state.write().await;
                    let account = match s.get_account_mut(&aid) {
                        Some(a) => a,
                        None => continue,
                    };
                    if account.registration.status() != RegistrationState::Registered {
                        continue;
                    }
                    let (local_addr, server_addr) = match (account.local_addr, account.server_addr) {
                        (Some(la), Some(sa)) => (la, sa),
                        _ => continue,
                    };
                    let transport = match &account.transport {
                        Some(t) => t.clone(),
                        None => continue,
                    };
                    let cseq = account.registration.next_cseq();
                    let call_id = account.registration.call_id().to_string();
                    let from_tag = account.registration.local_tag().to_string();
                    (account.config.clone(), local_addr, server_addr, call_id, from_tag, cseq, transport)
                };

                let msg = build_register(
                    &account_config,
                    local_addr,
                    &reg_call_id,
                    cseq,
                    &reg_from_tag,
                    None,
                    3600,
                );

                if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                    log::error!("Re-REGISTER failed for {}: {}", aid, e);
                    let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                        account_id: aid.clone(),
                        state: RegistrationState::Error,
                        error: Some(format!("Re-registration failed: {}", e)),
                    }));
                } else {
                    log::info!("Sent re-REGISTER for {} (cseq={})", aid, cseq);
                }
            }
        });
    }

    fn start_options_keepalive_for_account(&self, account_id: String) {
        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        let aid = account_id.clone();

        tokio::spawn(async move {
            let mut fail_count = 0u32;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;

                let (account_config, local_addr, server_addr, transport, cseq) = {
                    let mut s = state.write().await;
                    let account = match s.get_account_mut(&aid) {
                        Some(a) => a,
                        None => continue,
                    };
                    if account.registration.status() != RegistrationState::Registered {
                        fail_count = 0;
                        continue;
                    }
                    let (local_addr, server_addr) = match (account.local_addr, account.server_addr) {
                        (Some(la), Some(sa)) => (la, sa),
                        _ => continue,
                    };
                    let transport = match &account.transport {
                        Some(t) => t.clone(),
                        None => continue,
                    };
                    account.options_cseq += 1;
                    (account.config.clone(), local_addr, server_addr, transport, account.options_cseq)
                };

                let call_id = builder::generate_call_id();
                let from_tag = builder::generate_tag();

                let msg = build_options(
                    &account_config.domain,
                    local_addr,
                    &call_id,
                    cseq,
                    &from_tag,
                    &account_config.username,
                    account_config.transport.param(),
                );

                if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                    fail_count += 1;
                    log::warn!("OPTIONS keepalive failed for {} ({}/3): {}", aid, fail_count, e);
                    if fail_count >= 3 {
                        let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                            account_id: aid.clone(),
                            state: RegistrationState::Error,
                            error: Some("Connection lost (OPTIONS keepalive failed)".into()),
                        }));
                    }
                } else {
                    fail_count = 0;
                }
            }
        });
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

        let (account_config, local_addr, server_addr, cseq, call_id, from_tag, transport) = {
            let mut s = self.state.write().await;
            let account = match s.get_account_mut(&aid) {
                Some(a) => a,
                None => return Ok(()),
            };
            let local_addr = match account.local_addr {
                Some(a) => a,
                None => return Ok(()),
            };
            let server_addr = match account.server_addr {
                Some(a) => a,
                None => return Ok(()),
            };
            let transport = match &account.transport {
                Some(t) => t.clone(),
                None => return Ok(()),
            };
            let cseq = account.registration.next_cseq();
            let call_id = account.registration.call_id().to_string();
            let from_tag = account.registration.local_tag().to_string();
            (account.config.clone(), local_addr, server_addr, cseq, call_id, from_tag, transport)
        };

        let msg = build_register(&account_config, local_addr, &call_id, cseq, &from_tag, None, 0);
        let _ = transport.send_to(msg.as_bytes(), server_addr).await;

        {
            let mut s = self.state.write().await;
            s.accounts.remove(&aid);
            if s.active_account_id.as_ref() == Some(&aid) {
                s.active_account_id = s.accounts.keys().next().cloned();
            }
        }

        self.emit_registration_event(&aid, RegistrationState::Unregistered, None);

        Ok(())
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

    pub async fn make_call(&self, uri: &str) -> Result<String, String> {
        self.make_call_on_account(uri, None).await
    }

    pub async fn make_call_on_account(&self, uri: &str, account_id: Option<&str>) -> Result<String, String> {
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
                return Err("Not registered".into());
            }
            let local_addr = account.local_addr.ok_or("No local addr")?;
            let server_addr = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            let public_addr = account.public_addr;
            (account.config.clone(), local_addr, server_addr, transport, aid, public_addr)
        };

        let call_id = builder::generate_call_id();
        let from_tag = builder::generate_tag();

        // Allocate RTP port and discover public IP via STUN for NAT traversal
        let (rtp_port, stun_public_ip, _stun_public_port) = media::allocate_port_with_stun()
            .await
            .map_err(|e| format!("Failed to allocate RTP port: {}", e))?;

        // Use STUN-discovered public IP for SDP, fallback to registration-discovered public IP
        let public_ip = Some(stun_public_ip.to_string())
            .or_else(|| public_addr.map(|a| a.ip().to_string()));
        let (invite, local_srtp_key) = builder::build_invite_with_public_ip(
            &account_config, uri, local_addr, rtp_port, &call_id, 1, &from_tag, None,
            public_ip.as_deref(),
        );

        let branch = extract_via_branch(&invite).unwrap_or_default();

        let mut call = CallFSM::new_outbound(&aid, uri, call_id, from_tag, rtp_port, branch);
        call.local_srtp_key = local_srtp_key;
        let id = call.id.clone();

        transport.send_to(invite.as_bytes(), server_addr).await?;

        log::info!("Sending INVITE to {}", uri);

        self.emit(SipEvent::CallStateChanged(CallEvent::new(
            &aid, &id, "dialing", uri, "outbound"
        )));

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

            let mut s = state.write().await;
            if let Some((_, call)) = s.find_call_mut(&call_id) {
                // Timeout if still ringing after 60 more seconds
                if call.is_ringing() {
                    log::warn!("Call timeout (unanswered) for call {}", call_id);
                    let remote_uri = call.remote_uri.clone();
                    call.end(EndReason::Failed("No answer".to_string()));
                    let _ = event_tx.send(SipEvent::CallStateChanged(
                        CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                    ));
                }
            }
        });
    }

    pub async fn hangup(&self, call_id: &str) -> Result<(), String> {
        let (call_info, local_addr, server_addr, transport, account_id) = {
            let mut s = self.state.write().await;
            let (aid, call) = s.find_call_mut(call_id).ok_or("Call not found")?;
            let aid = aid.to_string();

            let cseq = call.next_cseq();
            let needs_cancel = call.is_dialing() || call.is_ringing();
            let info = (
                call.remote_uri.clone(),
                call.call_id_header.clone(),
                cseq,
                call.from_tag.clone(),
                call.to_tag.clone().unwrap_or_default(),
                needs_cancel,
                call.last_invite_branch.clone(),
                call.route_set().to_vec(),
                call.direction_str().to_string(),
            );

            let account = s.get_account(&aid).ok_or("Account not found")?;
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            let transport_str = account.config.transport.param().to_string();

            (info, la, sa, (transport, transport_str), aid)
        };

        let (remote_uri, sip_call_id, cseq, from_tag, to_tag, needs_cancel, branch, route_set, direction) = call_info;
        let (transport, transport_str) = transport;

        let msg = if needs_cancel {
            build_cancel(
                &remote_uri,
                local_addr,
                &sip_call_id,
                1,
                &from_tag,
                &transport_str,
                &branch.unwrap_or_default(),
            )
        } else {
            build_bye_with_routes(
                &remote_uri,
                local_addr,
                &sip_call_id,
                cseq,
                &from_tag,
                &to_tag,
                &transport_str,
                &route_set,
            )
        };

        transport.send_to(msg.as_bytes(), server_addr).await?;

        {
            let mut s = self.state.write().await;
            if let Some((_, call)) = s.find_call_mut(call_id) {
                if let Some(media) = call.media() {
                    // Stop recording if active (saves the WAV file)
                    if media.is_recording() {
                        if let Ok(Some(path)) = media.stop_recording() {
                            log::info!("Call recording saved: {}", path.display());
                        }
                    }
                    media.stop();
                }
                let _ = call.process(CallFSMEvent::LocalHangup);
            }
        }

        self.emit(SipEvent::CallStateChanged(
            CallEvent::new(&account_id, call_id, "ended", &remote_uri, &direction)
        ));

        let state = self.state.clone();
        let id = call_id.to_string();
        let aid = account_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let mut s = state.write().await;
            if let Some(account) = s.get_account_mut(&aid) {
                account.calls.retain(|c| c.id != id);
            }
        });

        Ok(())
    }

    pub async fn answer(&self, call_id: &str) -> Result<(), String> {
        let (raw_invite, local_addr, server_addr, rtp_port, to_tag, username, remote_uri, account_id, transport) = {
            let s = self.state.read().await;
            let (account, call) = s.find_call(call_id).ok_or("Call not found")?;
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
            )
        };

        // Discover public IP via STUN for NAT traversal in SDP
        let public_ip = match media::discover_public_ip().await {
            Ok(ip) => Some(ip.to_string()),
            Err(e) => {
                log::warn!("STUN discovery failed for answer: {}", e);
                None
            }
        };

        let response = build_200_ok_invite_with_public_ip(
            &raw_invite,
            local_addr,
            rtp_port,
            &to_tag,
            Some(&username),
            public_ip.as_deref(),
        )
        .ok_or("Failed to build 200 OK")?;

        transport.send_to(response.as_bytes(), server_addr).await?;

        let sdp = raw_invite.split("\r\n\r\n").nth(1).unwrap_or("");
        if let Some((ip, port)) = parse_sdp_connection(sdp) {
            let remote_rtp: SocketAddr = format!("{}:{}", ip, port)
                .parse()
                .map_err(|e| format!("Bad RTP addr: {}", e))?;

            let negotiated_codec = codec::negotiate_codec(sdp);
            log::info!("Negotiated codec for inbound: {:?}", negotiated_codec);

            let media = media::MediaSession::start(rtp_port, remote_rtp, negotiated_codec)
                .await
                .map_err(|e| e.to_string())?;

            let mut s = self.state.write().await;
            
            // Check if auto_record is enabled for this account
            let auto_record = s.get_account(&account_id)
                .map(|a| a.config.auto_record)
                .unwrap_or(false);
            
            // Start auto-recording if enabled
            if auto_record {
                if let Some(data_dir) = dirs::data_dir() {
                    let recordings_dir = data_dir.join("com.5060.aria").join("recordings");
                    let output_path = rtp_engine::generate_recording_filename(call_id, &recordings_dir);
                    if let Err(e) = media.start_recording(output_path) {
                        log::warn!("Failed to start auto-recording: {}", e);
                    } else {
                        log::info!("Auto-recording started for inbound call {}", call_id);
                    }
                }
            }
            
            if let Some((_, call)) = s.find_call_mut(call_id) {
                let _ = call.process(CallFSMEvent::LocalAnswer {
                    media,
                    remote_rtp,
                });
                call.set_to_tag(to_tag.clone());
            }
        }

        self.emit(SipEvent::CallStateChanged(
            CallEvent::new(&account_id, call_id, "connected", &remote_uri, "inbound")
        ));

        Ok(())
    }

    pub async fn hold(&self, call_id: &str, hold: bool) -> Result<(), String> {
        let mut s = self.state.write().await;
        if let Some((_, call)) = s.find_call_mut(call_id) {
            if hold {
                if let Some(media) = call.media_mut() {
                    media.set_mute(true);
                }
                let _ = call.process(CallFSMEvent::Hold);
            } else {
                if let Some(media) = call.media_mut() {
                    media.set_mute(false);
                }
                let _ = call.process(CallFSMEvent::Unhold);
            }
            Ok(())
        } else {
            Err("Call not found".into())
        }
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
    pub async fn stop_recording(&self, call_id: &str) -> Result<Option<String>, String> {
        log::info!("Stopping recording for call {}", call_id);
        let s = self.state.read().await;
        let (_, call) = s.find_call(call_id).ok_or("Call not found")?;
        if let Some(media) = call.media() {
            let path = media.stop_recording()
                .map_err(|e| format!("Failed to stop recording: {}", e))?;
            Ok(path.map(|p| p.to_string_lossy().to_string()))
        } else {
            Err("No active media session".into())
        }
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
            let info = (
                call.remote_uri.clone(),
                call.call_id_header.clone(),
                cseq,
                call.from_tag.clone(),
                call.to_tag.clone().unwrap_or_default(),
                call.route_set().to_vec(),
            );

            let account = s.get_account(&aid).ok_or("Account not found")?;
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let tp = account.config.transport.param().to_string();
            let transport = account.transport.clone().ok_or("No transport")?;

            (info, la, sa, tp, aid, transport)
        };

        let (remote_uri, sip_call_id, cseq, from_tag, to_tag, route_set) = call_info;

        let refer = build_refer(
            &remote_uri,
            target_uri,
            local_addr,
            &sip_call_id,
            cseq,
            &from_tag,
            &to_tag,
            &transport_str,
            &route_set,
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
            let b_info = (
                call_b.remote_uri.clone(),
                call_b.call_id_header.clone(),
                call_b.from_tag.clone(),
                call_b.to_tag.clone().unwrap_or_default(),
            );

            let (aid, call_a) = s.find_call_mut(call_id_a).ok_or("Call A not found")?;
            let aid = aid.to_string();

            if !call_a.is_connected() && !call_a.is_held() {
                return Err("Call A not in connected or held state".into());
            }

            let cseq = call_a.next_cseq();
            let a_info = (
                call_a.remote_uri.clone(),
                call_a.call_id_header.clone(),
                cseq,
                call_a.from_tag.clone(),
                call_a.to_tag.clone().unwrap_or_default(),
                call_a.route_set().to_vec(),
            );

            let account = s.get_account(&aid).ok_or("Account not found")?;
            let la = account.local_addr.ok_or("No local addr")?;
            let sa = account.server_addr.ok_or("No server addr")?;
            let tp = account.config.transport.param().to_string();
            let transport = account.transport.clone().ok_or("No transport")?;

            (a_info, b_info, la, sa, tp, aid, transport)
        };

        let (a_remote_uri, a_sip_call_id, a_cseq, a_from_tag, a_to_tag, a_route_set) = call_a_info;
        let (b_remote_uri, b_sip_call_id, b_from_tag, b_to_tag) = call_b_info;

        let refer = build_refer_with_replaces(
            &a_remote_uri,
            &b_remote_uri,
            &b_sip_call_id,
            &b_to_tag,
            &b_from_tag,
            local_addr,
            &a_sip_call_id,
            a_cseq,
            &a_from_tag,
            &a_to_tag,
            &transport_str,
            &a_route_set,
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
            
            for (_aid, account) in s.accounts.iter_mut() {
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
            for (_aid, account) in s.accounts.iter() {
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
            for (_aid, account) in s.accounts.iter_mut() {
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
        };

        let sub_id = sub.id.clone();

        let msg = build_subscribe(
            &account_config,
            target_uri,
            local_addr,
            &call_id,
            cseq,
            &from_tag,
            &event_type,
            expires,
            None,
        );

        transport.send_to(msg.as_bytes(), server_addr).await?;


        log::info!(
            "Sent SUBSCRIBE for {} (event={})",
            target_uri,
            event_type.event_header()
        );

        {
            let mut s = self.state.write().await;
            if let Some(account) = s.get_account_mut(&aid) {
                account.subscriptions.push(sub);
            }
        }

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
            let info = (
                sub.target_uri.clone(),
                sub.call_id.clone(),
                sub.cseq,
                sub.from_tag.clone(),
                sub.event_type.clone(),
            );
            let local_addr = account.local_addr.ok_or("No local addr")?;
            let server_addr = account.server_addr.ok_or("No server addr")?;
            let transport = account.transport.clone().ok_or("No transport")?;
            (account.config.clone(), local_addr, server_addr, transport, info, aid)
        };

        let (target_uri, call_id, cseq, from_tag, event_type) = sub_info;

        let msg = build_subscribe(
            &account_config,
            &target_uri,
            local_addr,
            &call_id,
            cseq,
            &from_tag,
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

    /// Start subscription refresh timers for all active subscriptions
    fn start_subscribe_refresh_timer(&self) {
        let state = self.state.clone();

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;

                // Collect subs to refresh from all accounts
                let subs_to_refresh: Vec<(String, String, String, u32, String, EventType, u32)> = {
                    let s = state.read().await;
                    let mut result = Vec::new();
                    for (aid, account) in s.accounts.iter() {
                        if account.registration.status() != RegistrationState::Registered {
                            continue;
                        }
                        for sub in account.subscriptions.iter() {
                            if sub.state == SubscriptionState::Active && sub.expires > 0 {
                                result.push((
                                    aid.clone(),
                                    sub.target_uri.clone(),
                                    sub.call_id.clone(),
                                    sub.cseq,
                                    sub.from_tag.clone(),
                                    sub.event_type.clone(),
                                    sub.expires,
                                ));
                            }
                        }
                    }
                    result
                };

                if subs_to_refresh.is_empty() {
                    continue;
                }

                for (aid, target_uri, call_id, _old_cseq, from_tag, event_type, expires) in subs_to_refresh {
                    let (account_config, local_addr, server_addr, transport, cseq) = {
                        let mut s = state.write().await;
                        let account = match s.get_account_mut(&aid) {
                            Some(a) => a,
                            None => continue,
                        };
                        let sub = match account.subscriptions.iter_mut().find(|s| s.call_id == call_id) {
                            Some(s) => s,
                            None => continue,
                        };
                        sub.cseq += 1;
                        let cseq = sub.cseq;
                        let local_addr = match account.local_addr {
                            Some(a) => a,
                            None => continue,
                        };
                        let server_addr = match account.server_addr {
                            Some(a) => a,
                            None => continue,
                        };
                        let transport = match &account.transport {
                            Some(t) => t.clone(),
                            None => continue,
                        };
                        (account.config.clone(), local_addr, server_addr, transport, cseq)
                    };

                    let msg = build_subscribe(
                        &account_config,
                        &target_uri,
                        local_addr,
                        &call_id,
                        cseq,
                        &from_tag,
                        &event_type,
                        expires,
                        None,
                    );

                    if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                        log::error!("SUBSCRIBE refresh failed for {}: {}", target_uri, e);
                    } else {
                        log::debug!("Sent SUBSCRIBE refresh for {} (cseq={})", target_uri, cseq);
                    }
                }
            }
        });
    }

    fn start_receive_loop_for_account(&self, mut rx: mpsc::Receiver<SipMessage>, account_id: String) {
        let state = self.state.clone();
        let event_tx = self.event_tx.clone();
        let aid = account_id.clone();

        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let text = match String::from_utf8(msg.data) {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                log::debug!("Received SIP for account {} from {}:\n{}", aid, msg.remote, text);

                let diag = DiagnosticLog {
                    timestamp: diagnostics::now_millis(),
                    account_id: aid.clone(),
                    direction: diagnostics::MessageDirection::Received,
                    remote_addr: msg.remote.to_string(),
                    summary: diagnostics::summarize_sip(&text),
                    raw: text.clone(),
                };
                {
                    let s = state.read().await;
                    s.diagnostic_store.push(diag.clone()).await;
                }
                let _ = event_tx.send(SipEvent::DiagnosticMessage(diag));

                if is_request(&text) {
                    handlers::handle_incoming_request(&state, &event_tx, &text, msg.remote, &aid).await;
                } else {
                    handlers::handle_response(&state, &event_tx, &text, &aid).await;
                }
            }
        });
    }
}

