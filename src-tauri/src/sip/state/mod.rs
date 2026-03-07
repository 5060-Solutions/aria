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

pub use call::{CallFSM, CallFSMEvent, EndReason};
pub use registration::{RegistrationFSM, RegistrationState, RegistrationStatus};
#[allow(unused_imports)]
pub use registration::RegistrationTransitionResult;

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
}

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
        }
    }
}
