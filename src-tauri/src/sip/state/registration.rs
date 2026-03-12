//! Registration State Machine
//!
//! This module implements a formal finite state machine for SIP registration.
//!
//! NOTE: The RegistrationFSM is actively used. RegistrationEvent and
//! RegistrationTransitionResult are part of the formal FSM interface and
//! kept for completeness, though the current integration uses simpler methods.
//!
//! # State Diagram
//!
//! ```text
//!                         ┌──────────────────┐
//!                         │   Unregistered   │
//!                         └────────┬─────────┘
//!                                  │ register()
//!                                  ▼
//!                         ┌──────────────────┐
//!                    ┌───▶│   Registering    │◀───┐
//!                    │    └────────┬─────────┘    │
//!                    │             │              │
//!           re-register           │              │ 401/407 challenge
//!           (timer)               │              │ (retry with auth)
//!                    │             ▼              │
//!                    │    ┌──────────────────┐    │
//!                    └────│   Registered     │────┘
//!                         └────────┬─────────┘
//!                                  │
//!              unregister() /      │      / timeout / 4xx error
//!              network error       │
//!                                  ▼
//!         ┌──────────────────┐    │    ┌──────────────────┐
//!         │   Unregistered   │◀───┴───▶│      Error       │
//!         └──────────────────┘         └──────────────────┘
//! ```

use std::net::SocketAddr;
use std::time::Instant;

use super::super::account::AccountConfig;

/// Events that can trigger registration state transitions
#[derive(Debug)]
#[allow(dead_code, clippy::large_enum_variant)]
pub enum RegistrationEvent {
    /// Start registration process
    Register { account: AccountConfig },
    /// Received 401/407 challenge
    AuthChallenge {
        status: u16,
        challenge: String,
    },
    /// Registration succeeded (200 OK)
    Success {
        public_addr: Option<SocketAddr>,
    },
    /// Registration failed with error
    Failure { status: u16, reason: String },
    /// Registration timed out
    Timeout,
    /// Network error occurred
    NetworkError { reason: String },
    /// Unregister requested
    Unregister,
    /// Re-registration timer fired
    ReRegister,
}

/// Result of a state transition
#[derive(Debug)]
#[allow(dead_code)]
pub enum RegistrationTransitionResult {
    /// Transition was successful
    Ok,
    /// Transition requires sending a message
    SendRegister {
        auth_header: Option<String>,
    },
    /// Transition requires sending unregister
    SendUnregister,
    /// Transition was invalid from the current state
    InvalidTransition {
        from: &'static str,
        event: &'static str,
    },
}

/// Simple serializable registration state for UI events
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RegistrationStatus {
    Unregistered,
    Registering,
    Registered,
    Error,
}

/// Registration state with associated data
#[derive(Debug, Clone, PartialEq)]
pub enum RegistrationState {
    /// Not registered
    Unregistered,
    /// Registration in progress
    Registering {
        /// Number of auth attempts (for loop prevention)
        auth_attempts: u32,
    },
    /// Successfully registered
    Registered {
        /// When registration succeeded
        registered_at: Instant,
        /// Public address discovered via Via received/rport
        public_addr: Option<SocketAddr>,
    },
    /// Registration failed
    Error {
        /// Error message
        reason: String,
    },
}

impl RegistrationState {
    /// Get a string representation of the state
    pub fn name(&self) -> &'static str {
        match self {
            Self::Unregistered => "unregistered",
            Self::Registering { .. } => "registering",
            Self::Registered { .. } => "registered",
            Self::Error { .. } => "error",
        }
    }

    /// Check if registered
    #[allow(dead_code)]
    pub fn is_registered(&self) -> bool {
        matches!(self, Self::Registered { .. })
    }

    /// Get the serializable status for UI events
    pub fn status(&self) -> RegistrationStatus {
        match self {
            Self::Unregistered => RegistrationStatus::Unregistered,
            Self::Registering { .. } => RegistrationStatus::Registering,
            Self::Registered { .. } => RegistrationStatus::Registered,
            Self::Error { .. } => RegistrationStatus::Error,
        }
    }

    /// Get the error reason if in error state
    pub fn error_reason(&self) -> Option<&str> {
        match self {
            Self::Error { reason } => Some(reason),
            _ => None,
        }
    }
}

/// The Registration Finite State Machine
///
/// Manages the SIP registration lifecycle with well-defined state transitions.
pub struct RegistrationFSM {
    /// Current state
    state: RegistrationState,
    /// Account configuration
    account: Option<AccountConfig>,
    /// Call-ID for registration dialog
    call_id: String,
    /// From tag for registration dialog
    from_tag: String,
    /// Current CSeq
    cseq: u32,
    /// Server address
    server_addr: Option<SocketAddr>,
    /// Local address (for Contact header)
    local_addr: Option<SocketAddr>,
}

impl RegistrationFSM {
    /// Create a new registration FSM
    pub fn new() -> Self {
        Self {
            state: RegistrationState::Unregistered,
            account: None,
            call_id: crate::sip::builder::generate_call_id(),
            from_tag: crate::sip::builder::generate_tag(),
            cseq: 0,
            server_addr: None,
            local_addr: None,
        }
    }

    /// Get the current state
    pub fn state(&self) -> &RegistrationState {
        &self.state
    }

    /// Get the state name
    pub fn state_name(&self) -> &'static str {
        self.state.name()
    }

    /// Check if registered
    #[allow(dead_code)]
    pub fn is_registered(&self) -> bool {
        self.state.is_registered()
    }

    /// Get the serializable status for UI events
    pub fn status(&self) -> RegistrationStatus {
        self.state.status()
    }

    /// Get the error reason if in error state
    pub fn error_reason(&self) -> Option<&str> {
        self.state.error_reason()
    }

    /// Get the account configuration
    #[allow(dead_code)]
    pub fn account(&self) -> Option<&AccountConfig> {
        self.account.as_ref()
    }

    /// Get the server address
    #[allow(dead_code)]
    pub fn server_addr(&self) -> Option<SocketAddr> {
        self.server_addr
    }

    /// Get the local address
    #[allow(dead_code)]
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Get the public address (discovered via Via)
    #[allow(dead_code)]
    pub fn public_addr(&self) -> Option<SocketAddr> {
        match &self.state {
            RegistrationState::Registered { public_addr, .. } => *public_addr,
            _ => None,
        }
    }

    /// Get the Call-ID for the registration dialog
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Get the From tag
    pub fn local_tag(&self) -> &str {
        &self.from_tag
    }

    /// Get and increment CSeq
    pub fn next_cseq(&mut self) -> u32 {
        self.cseq += 1;
        self.cseq
    }

    /// Get current CSeq without incrementing
    pub fn current_cseq(&self) -> u32 {
        self.cseq
    }

    /// Get current auth attempts count
    pub fn auth_attempts(&self) -> u32 {
        match &self.state {
            RegistrationState::Registering { auth_attempts } => *auth_attempts,
            _ => 0,
        }
    }

    /// Check if currently in registering state
    pub fn is_registering(&self) -> bool {
        matches!(&self.state, RegistrationState::Registering { .. })
    }

    /// Increment auth attempts counter
    pub fn increment_auth_attempts(&mut self) {
        if let RegistrationState::Registering { auth_attempts } = &mut self.state {
            *auth_attempts += 1;
            log::info!("Incremented auth_attempts to {}", *auth_attempts);
        } else {
            log::warn!("Cannot increment auth_attempts - not in Registering state: {:?}", self.state.name());
        }
    }

    /// Set the server address
    pub fn set_server_addr(&mut self, addr: SocketAddr) {
        self.server_addr = Some(addr);
    }

    /// Set the local address
    pub fn set_local_addr(&mut self, addr: SocketAddr) {
        self.local_addr = Some(addr);
    }

    /// Start registration
    pub fn start_registration(&mut self, account: AccountConfig) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Unregistered | RegistrationState::Error { .. } => {
                self.account = Some(account);
                self.cseq += 1;
                self.state = RegistrationState::Registering { auth_attempts: 0 };
                RegistrationTransitionResult::SendRegister { auth_header: None }
            }
            RegistrationState::Registering { .. } | RegistrationState::Registered { .. } => {
                RegistrationTransitionResult::InvalidTransition {
                    from: self.state.name(),
                    event: "register",
                }
            }
        }
    }

    /// Process an authentication challenge
    #[allow(dead_code)]
    pub fn auth_challenged(
        &mut self,
        _status: u16,
        challenge: &str,
    ) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Registering { auth_attempts } => {
                if *auth_attempts >= 2 {
                    // Too many auth attempts - likely wrong credentials
                    self.state = RegistrationState::Error {
                        reason: "Authentication failed after multiple attempts".to_string(),
                    };
                    return RegistrationTransitionResult::Ok;
                }

                let account = match &self.account {
                    Some(a) => a,
                    None => {
                        self.state = RegistrationState::Error {
                            reason: "No account configured".to_string(),
                        };
                        return RegistrationTransitionResult::Ok;
                    }
                };

                let registrar = account.registrar.as_deref().unwrap_or(&account.domain);
                let uri = format!("sip:{}", registrar);

                let auth = crate::sip::auth::DigestAuth::from_challenge_with_realm(
                    challenge,
                    account.effective_auth_username(),
                    &account.password,
                    &uri,
                    "REGISTER",
                    account.auth_realm.as_deref(),
                );

                match auth {
                    Some(a) => {
                        self.cseq += 1;
                        self.state = RegistrationState::Registering {
                            auth_attempts: auth_attempts + 1,
                        };
                        RegistrationTransitionResult::SendRegister {
                            auth_header: Some(a.to_header()),
                        }
                    }
                    None => {
                        self.state = RegistrationState::Error {
                            reason: "Failed to parse auth challenge".to_string(),
                        };
                        RegistrationTransitionResult::Ok
                    }
                }
            }
            _ => RegistrationTransitionResult::InvalidTransition {
                from: self.state.name(),
                event: "auth_challenge",
            },
        }
    }

    /// Registration succeeded
    pub fn registration_success(
        &mut self,
        public_addr: Option<SocketAddr>,
    ) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Registering { .. } => {
                self.state = RegistrationState::Registered {
                    registered_at: Instant::now(),
                    public_addr,
                };
                RegistrationTransitionResult::Ok
            }
            _ => RegistrationTransitionResult::InvalidTransition {
                from: self.state.name(),
                event: "success",
            },
        }
    }

    /// Registration failed
    pub fn registration_failed(&mut self, _status: u16, reason: &str) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Registering { .. } => {
                self.state = RegistrationState::Error {
                    reason: reason.to_string(),
                };
                RegistrationTransitionResult::Ok
            }
            _ => RegistrationTransitionResult::InvalidTransition {
                from: self.state.name(),
                event: "failure",
            },
        }
    }

    /// Registration timed out
    pub fn registration_timeout(&mut self) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Registering { .. } => {
                self.state = RegistrationState::Error {
                    reason: "Registration timed out".to_string(),
                };
                RegistrationTransitionResult::Ok
            }
            _ => RegistrationTransitionResult::InvalidTransition {
                from: self.state.name(),
                event: "timeout",
            },
        }
    }

    /// Start unregistration
    #[allow(dead_code)]
    pub fn start_unregistration(&mut self) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Registered { .. } | RegistrationState::Registering { .. } => {
                self.cseq += 1;
                // We'll transition to Unregistered after sending
                RegistrationTransitionResult::SendUnregister
            }
            _ => RegistrationTransitionResult::InvalidTransition {
                from: self.state.name(),
                event: "unregister",
            },
        }
    }

    /// Complete unregistration
    #[allow(dead_code)]
    pub fn unregistration_complete(&mut self) {
        self.state = RegistrationState::Unregistered;
        self.account = None;
        self.server_addr = None;
        self.local_addr = None;
    }

    /// Re-registration (called by timer)
    #[allow(dead_code)]
    pub fn re_register(&mut self) -> RegistrationTransitionResult {
        match &self.state {
            RegistrationState::Registered { .. } => {
                self.cseq += 1;
                // Stay registered but send a refresh
                RegistrationTransitionResult::SendRegister { auth_header: None }
            }
            _ => RegistrationTransitionResult::InvalidTransition {
                from: self.state.name(),
                event: "re_register",
            },
        }
    }

    /// Network error occurred
    #[allow(dead_code)]
    pub fn network_error(&mut self, reason: &str) {
        self.state = RegistrationState::Error {
            reason: reason.to_string(),
        };
    }

}

impl Default for RegistrationFSM {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sip::account::SrtpMode;
    use crate::sip::transport::TransportType;

    fn test_account() -> AccountConfig {
        AccountConfig {
            id: "test-account-id".to_string(),
            username: "testuser".to_string(),
            password: "testpass".to_string(),
            domain: "example.com".to_string(),
            display_name: "Test User".to_string(),
            auth_username: None,
            transport: TransportType::Udp,
            port: 5060,
            registrar: None,
            outbound_proxy: None,
            enabled: true,
            auto_record: false,
            srtp_mode: SrtpMode::Disabled,
            codecs: vec![],
        }
    }

    #[test]
    fn test_registration_happy_path() {
        let mut fsm = RegistrationFSM::new();
        assert_eq!(fsm.state_name(), "unregistered");

        // Start registration
        let result = fsm.start_registration(test_account());
        assert!(matches!(
            result,
            RegistrationTransitionResult::SendRegister { auth_header: None }
        ));
        assert_eq!(fsm.state_name(), "registering");

        // Registration succeeds
        let result = fsm.registration_success(None);
        assert!(matches!(result, RegistrationTransitionResult::Ok));
        assert_eq!(fsm.state_name(), "registered");
        assert!(fsm.is_registered());
    }

    #[test]
    fn test_registration_with_auth() {
        let mut fsm = RegistrationFSM::new();

        fsm.start_registration(test_account());
        assert_eq!(fsm.state_name(), "registering");

        // Receive 401 challenge
        let result = fsm.auth_challenged(
            401,
            "Digest realm=\"example.com\", nonce=\"abc123\", algorithm=MD5",
        );
        assert!(matches!(
            result,
            RegistrationTransitionResult::SendRegister {
                auth_header: Some(_)
            }
        ));
        assert_eq!(fsm.state_name(), "registering");

        // Registration succeeds
        fsm.registration_success(None);
        assert_eq!(fsm.state_name(), "registered");
    }

    #[test]
    fn test_registration_auth_loop_prevention() {
        let mut fsm = RegistrationFSM::new();

        fsm.start_registration(test_account());

        // Multiple auth challenges should eventually fail
        fsm.auth_challenged(401, "Digest realm=\"example.com\", nonce=\"1\"");
        fsm.auth_challenged(401, "Digest realm=\"example.com\", nonce=\"2\"");
        let result = fsm.auth_challenged(401, "Digest realm=\"example.com\", nonce=\"3\"");

        assert!(matches!(result, RegistrationTransitionResult::Ok));
        assert_eq!(fsm.state_name(), "error");
    }

    #[test]
    fn test_registration_timeout() {
        let mut fsm = RegistrationFSM::new();

        fsm.start_registration(test_account());
        assert_eq!(fsm.state_name(), "registering");

        fsm.registration_timeout();
        assert_eq!(fsm.state_name(), "error");
    }

    #[test]
    fn test_unregistration() {
        let mut fsm = RegistrationFSM::new();

        fsm.start_registration(test_account());
        fsm.registration_success(None);
        assert!(fsm.is_registered());

        let result = fsm.start_unregistration();
        assert!(matches!(result, RegistrationTransitionResult::SendUnregister));

        fsm.unregistration_complete();
        assert_eq!(fsm.state_name(), "unregistered");
        assert!(!fsm.is_registered());
    }

    #[test]
    fn test_re_registration() {
        let mut fsm = RegistrationFSM::new();

        fsm.start_registration(test_account());
        fsm.registration_success(None);

        // Re-register while registered
        let result = fsm.re_register();
        assert!(matches!(
            result,
            RegistrationTransitionResult::SendRegister { auth_header: None }
        ));
        // Should stay registered
        assert_eq!(fsm.state_name(), "registered");
    }

    #[test]
    fn test_invalid_transitions() {
        let mut fsm = RegistrationFSM::new();

        // Can't auth challenge when unregistered
        let result = fsm.auth_challenged(401, "Digest realm=\"example.com\"");
        assert!(matches!(
            result,
            RegistrationTransitionResult::InvalidTransition { .. }
        ));

        // Can't succeed when unregistered
        let result = fsm.registration_success(None);
        assert!(matches!(
            result,
            RegistrationTransitionResult::InvalidTransition { .. }
        ));
    }
}
