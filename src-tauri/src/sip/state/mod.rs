//! SIP State Machines
//!
//! This module contains the formal finite state machines for SIP protocol handling.
//! Each state machine has well-defined states and guarded transitions, making the
//! protocol behavior predictable and testable.
//!
//! # Architecture
//!
//! - [`CallFSM`] - Manages the lifecycle of a single SIP call
//! - [`RegistrationFSM`] - Manages SIP registration with a server
//! - [`AccountState`] - Per-account state holding registration, transport, and calls
//!
//! # Design Principles
//!
//! 1. **Explicit States** - Each state is a distinct enum variant with associated data
//! 2. **Guarded Transitions** - Invalid transitions return errors instead of corrupting state
//! 3. **Testability** - Pure state logic separated from I/O for easy unit testing
//! 4. **Type Safety** - Rust's type system enforces valid state combinations

pub mod call;
pub mod registration;

use std::collections::HashMap;
use std::net::SocketAddr;

use super::account::AccountConfig;
use super::presence::{BlfEntry, Subscription};
use super::transport::SipTransport;

pub use call::{CallFSM, CallFSMEvent, EndReason, InboundCallParams};
pub use registration::{RegistrationFSM, RegistrationState, RegistrationStatus};
#[allow(unused_imports)]
pub use registration::RegistrationTransitionResult;
pub use super::presence::EventType;

/// Per-account state for multi-account support
///
/// Each account has its own:
/// - Registration FSM and state
/// - Transport connection
/// - Active calls
/// - Network addresses
/// - Presence subscriptions
pub struct AccountState {
    /// Account configuration
    pub config: AccountConfig,
    /// Registration FSM managing state transitions
    pub registration: RegistrationFSM,
    /// Network transport (UDP/TCP/TLS)
    pub transport: Option<SipTransport>,
    /// Active calls for this account
    pub calls: Vec<CallFSM>,
    /// Server address
    pub server_addr: Option<SocketAddr>,
    /// Resolved local address (never 0.0.0.0 — used in Contact/Via headers)
    pub local_addr: Option<SocketAddr>,
    /// Public IP/port as discovered via Via received/rport (RFC 3581)
    pub public_addr: Option<SocketAddr>,
    /// CSeq counter for OPTIONS keepalive
    pub options_cseq: u32,
    /// Presence / BLF subscriptions
    pub subscriptions: Vec<Subscription>,
    /// BLF states
    pub blf_states: HashMap<String, BlfEntry>,
    /// Active conferences (conference_id -> list of call_ids)
    pub conferences: HashMap<String, Vec<String>>,
    /// Realm fallback for 403 retry: when auth fails with 403, retry with
    /// the server IP as realm (common FreeSwitch misconfiguration).
    /// None = no fallback tried, Some(realm) = currently trying this realm.
    pub realm_fallback: Option<String>,
    /// Whether we've exhausted realm fallback attempts
    pub realm_fallback_exhausted: bool,
    /// Subscription targets to re-subscribe after reconnect.
    /// Saved when transport dies, consumed when registration succeeds.
    pub pending_resubscriptions: Vec<(String, EventType)>,
    /// Which transport/registration cycle the account's background tasks
    /// belong to. Bumped whenever a new transport is created or the account
    /// is unregistered; a task whose generation no longer matches exits.
    ///
    /// Each `register()` used to spawn a fresh refresh timer and keepalive
    /// without stopping the old ones, so an account that took several
    /// attempts to come up refreshed several times as often, forever.
    pub timer_generation: u64,
    /// The Expires we ask for.
    pub requested_expires: u32,
    /// The Expires the registrar actually granted in its last 200 OK. The
    /// refresh is scheduled from this, not from what we asked for.
    pub granted_expires: u32,
    /// A refresh REGISTER is outstanding: the account was Registered when it
    /// was sent, so a failure means the connection is in doubt, not that the
    /// credentials are.
    pub refreshing: bool,
    /// The user asked to unregister: the REGISTER in flight has Expires: 0,
    /// and its answer (or a timeout) removes the account.
    pub unregistering: bool,
    /// What to do if the REGISTER in flight gets no final response.
    pub on_register_timeout: RegisterTimeoutPolicy,
}

/// What a REGISTER that never gets a final response means for the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterTimeoutPolicy {
    /// First registration: tell the user it failed.
    Error,
    /// Refresh or reconnect: the server has gone quiet; reconnect with backoff.
    Reconnect,
    /// Unregister: we tried; drop the account either way.
    Forget,
}

/// The Expires we ask a registrar for, unless it tells us otherwise (423).
pub const DEFAULT_REGISTER_EXPIRES: u32 = 300;

impl AccountState {
    /// Create a new account state from configuration
    pub fn new(config: AccountConfig) -> Self {
        Self {
            config,
            registration: RegistrationFSM::new(),
            transport: None,
            calls: Vec::new(),
            server_addr: None,
            local_addr: None,
            public_addr: None,
            options_cseq: 0,
            subscriptions: Vec::new(),
            blf_states: HashMap::new(),
            conferences: HashMap::new(),
            realm_fallback: None,
            realm_fallback_exhausted: false,
            pending_resubscriptions: Vec::new(),
            timer_generation: 0,
            requested_expires: DEFAULT_REGISTER_EXPIRES,
            granted_expires: DEFAULT_REGISTER_EXPIRES,
            refreshing: false,
            unregistering: false,
            on_register_timeout: RegisterTimeoutPolicy::Error,
        }
    }
}
