//! Call State Machine
//!
//! This module implements a formal finite state machine for SIP calls.
//! Each call has a well-defined set of states and transitions, with guards
//! that prevent invalid state transitions.
//!
//! NOTE: This FSM infrastructure is ready for use but the migration from the
//! legacy Call struct is pending. The types are kept for future integration.

#![allow(dead_code)]
//!
//! # State Diagram
//!
//! ```text
//!                              ┌─────────────┐
//!                              │    Idle     │
//!                              └──────┬──────┘
//!                                     │
//!                    ┌────────────────┼────────────────┐
//!                    │ make_call()    │                │ incoming INVITE
//!                    ▼                │                ▼
//!             ┌─────────────┐         │         ┌─────────────┐
//!             │   Dialing   │         │         │  Incoming   │
//!             └──────┬──────┘         │         └──────┬──────┘
//!                    │                │                │
//!                    │ 180 Ringing    │                │ answer()
//!                    ▼                │                │
//!             ┌─────────────┐         │                │
//!             │   Ringing   │         │                │
//!             └──────┬──────┘         │                │
//!                    │ 200 OK         │                │
//!                    └────────────────┼────────────────┘
//!                                     │
//!                                     ▼
//!                              ┌─────────────┐
//!                         ┌───▶│  Connected  │◀───┐
//!                         │    └──────┬──────┘    │
//!                         │           │           │
//!                 unhold()│     hold()│           │unhold()
//!                         │           ▼           │
//!                         │    ┌─────────────┐    │
//!                         └────│    Held     │────┘
//!                              └──────┬──────┘
//!                                     │
//!            hangup() / BYE / CANCEL  │
//!            ─────────────────────────┼─────────────────────────
//!                                     ▼
//!                              ┌─────────────┐
//!                              │   Ended     │
//!                              └─────────────┘
//! ```

use std::net::SocketAddr;
use std::time::Instant;

use super::super::media::MediaSession;

/// Reason why a call ended
#[derive(Debug, Clone, PartialEq)]
pub enum EndReason {
    /// Local user hung up
    LocalHangup,
    /// Remote party hung up (BYE received)
    RemoteHangup,
    /// Call was cancelled before answer
    Cancelled,
    /// Call was rejected (486 Busy, 603 Decline, etc.)
    Rejected(u16),
    /// Call failed (network error, timeout, etc.)
    Failed(String),
    /// Call was transferred
    Transferred,
}

/// Call direction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CallDirection {
    Inbound,
    Outbound,
}

/// Events that can trigger state transitions in the CallFSM
#[derive(Debug)]
pub enum CallFSMEvent {
    /// Start an outbound call
    Initiate,
    /// Received provisional response (100 Trying)
    Trying,
    /// Received ringing response (180/183)
    RemoteRinging,
    /// Received 200 OK (call answered)
    Answered {
        to_tag: String,
        remote_rtp: Option<SocketAddr>,
        route_set: Vec<String>,
        session_expires: u32,
    },
    /// Local user answered incoming call
    LocalAnswer {
        media: MediaSession,
        remote_rtp: SocketAddr,
    },
    /// Call authentication challenged
    AuthChallenge { status: u16 },
    /// Put call on hold
    Hold,
    /// Resume call from hold
    Unhold,
    /// Local user hung up
    LocalHangup,
    /// Remote party hung up
    RemoteHangup,
    /// Call was cancelled
    Cancel,
    /// Call was rejected
    Reject { status: u16 },
    /// Call failed
    Fail { reason: String },
    /// Media session established
    MediaEstablished { media: MediaSession },
}

/// Result of a state transition
#[derive(Debug)]
pub enum TransitionResult {
    /// Transition was successful
    Ok,
    /// Transition was invalid from the current state
    InvalidTransition {
        from: &'static str,
        event: &'static str,
    },
    /// State machine is in terminal state
    AlreadyEnded,
}

/// Call state - uses enum variants with data to enforce valid states
#[derive(Debug)]
pub enum CallState {
    /// Initial state before call setup
    Idle,
    /// Outbound call initiated, waiting for response
    Dialing {
        invite_branch: String,
        auth_attempted: bool,
    },
    /// Received 180/183, waiting for answer
    Ringing {
        to_tag: Option<String>,
        early_media: Option<MediaSession>,
    },
    /// Incoming call, waiting for local answer
    Incoming { raw_invite: String },
    /// Call is connected with active media
    Connected {
        media: Option<MediaSession>,
        route_set: Vec<String>,
        connected_at: Instant,
        session_expires: u32,
    },
    /// Call is on hold
    Held {
        media: Option<MediaSession>,
        route_set: Vec<String>,
        connected_at: Instant,
        session_expires: u32,
    },
    /// Call has ended
    Ended { reason: EndReason },
}

impl CallState {
    /// Get a string representation of the state for logging/events
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Dialing { .. } => "dialing",
            Self::Ringing { .. } => "ringing",
            Self::Incoming { .. } => "incoming",
            Self::Connected { .. } => "connected",
            Self::Held { .. } => "held",
            Self::Ended { .. } => "ended",
        }
    }

    /// Check if the call is in a connected state (connected or held)
    pub fn is_established(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::Held { .. })
    }

    /// Check if the call has ended
    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended { .. })
    }
}

/// The Call Finite State Machine
///
/// Manages the lifecycle of a single SIP call with well-defined state transitions.
pub struct CallFSM {
    /// Unique identifier for this call
    pub id: String,
    /// Account ID this call belongs to
    pub account_id: String,
    /// Remote party URI
    pub remote_uri: String,
    /// Call direction
    pub direction: CallDirection,
    /// SIP Call-ID header
    pub call_id_header: String,
    /// From tag (ours for outbound, theirs for inbound)
    pub from_tag: String,
    /// To tag (theirs for outbound, ours for inbound)
    pub to_tag: Option<String>,
    /// Current CSeq number
    pub cseq: u32,
    /// Local RTP port
    pub local_rtp_port: u16,
    /// Remote RTP address
    pub remote_rtp_addr: Option<SocketAddr>,
    /// Last INVITE branch (for CANCEL matching)
    pub last_invite_branch: Option<String>,
    /// Whether auth has been attempted (to prevent loops)
    pub auth_attempted: bool,
    /// Local SRTP key (base64 encoded) for outbound encryption
    pub local_srtp_key: Option<String>,
    /// Remote Contact URI from 200 OK (the URI to send BYE/re-INVITE to)
    pub remote_contact: Option<String>,
    /// Our local SIP URI (e.g., sip:1001@lyonscomm.com)
    pub local_uri: String,
    /// Current state
    state: CallState,
}

impl CallFSM {
    /// Create a new outbound call FSM
    pub fn new_outbound(
        account_id: &str,
        remote_uri: &str,
        call_id: String,
        from_tag: String,
        local_rtp_port: u16,
        invite_branch: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: account_id.to_string(),
            remote_uri: remote_uri.to_string(),
            direction: CallDirection::Outbound,
            call_id_header: call_id,
            from_tag,
            to_tag: None,
            cseq: 1,
            local_rtp_port,
            remote_rtp_addr: None,
            last_invite_branch: Some(invite_branch.clone()),
            auth_attempted: false,
            local_srtp_key: None,
            state: CallState::Dialing {
                invite_branch,
                auth_attempted: false,
            },
        }
    }

    /// Create a new inbound call FSM
    pub fn new_inbound(
        account_id: &str,
        remote_uri: &str,
        call_id: String,
        from_tag: String,
        to_tag: String,
        local_rtp_port: u16,
        raw_invite: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: account_id.to_string(),
            remote_uri: remote_uri.to_string(),
            direction: CallDirection::Inbound,
            call_id_header: call_id,
            from_tag,
            to_tag: Some(to_tag),
            cseq: 1,
            local_rtp_port,
            remote_rtp_addr: None,
            last_invite_branch: None,
            auth_attempted: false,
            local_srtp_key: None,
            state: CallState::Incoming { raw_invite },
        }
    }

    /// Get the current state
    pub fn state(&self) -> &CallState {
        &self.state
    }

    /// Get the state name for events
    pub fn state_name(&self) -> &'static str {
        self.state.name()
    }

    /// Check if call is established
    pub fn is_established(&self) -> bool {
        self.state.is_established()
    }

    /// Check if call has ended
    pub fn is_ended(&self) -> bool {
        self.state.is_ended()
    }

    /// Get the media session if available
    pub fn media(&self) -> Option<&MediaSession> {
        match &self.state {
            CallState::Connected { media, .. } => media.as_ref(),
            CallState::Held { media, .. } => media.as_ref(),
            _ => None,
        }
    }

    /// Get mutable media session if available
    pub fn media_mut(&mut self) -> Option<&mut MediaSession> {
        match &mut self.state {
            CallState::Connected { media, .. } => media.as_mut(),
            CallState::Held { media, .. } => media.as_mut(),
            _ => None,
        }
    }

    /// Get the route set for in-dialog requests
    pub fn route_set(&self) -> &[String] {
        match &self.state {
            CallState::Connected { route_set, .. } => route_set,
            CallState::Held { route_set, .. } => route_set,
            _ => &[],
        }
    }

    /// Get the time the call was connected
    pub fn connected_at(&self) -> Option<std::time::Instant> {
        match &self.state {
            CallState::Connected { connected_at, .. } |
            CallState::Held { connected_at, .. } => Some(*connected_at),
            _ => None,
        }
    }

    /// Get the session expiry time in seconds
    pub fn session_expires(&self) -> u32 {
        match &self.state {
            CallState::Connected { session_expires, .. } |
            CallState::Held { session_expires, .. } => *session_expires,
            _ => 1800,
        }
    }

    /// Get the raw INVITE for incoming calls
    pub fn raw_invite(&self) -> Option<&str> {
        match &self.state {
            CallState::Incoming { raw_invite } => Some(raw_invite),
            _ => None,
        }
    }

    /// Get the invite branch for outbound calls
    pub fn invite_branch(&self) -> Option<&str> {
        match &self.state {
            CallState::Dialing { invite_branch, .. } => Some(invite_branch),
            _ => None,
        }
    }

    /// Check if auth was already attempted (for loop prevention)
    pub fn auth_attempted(&self) -> bool {
        match &self.state {
            CallState::Dialing { auth_attempted, .. } => *auth_attempted,
            _ => false,
        }
    }

    /// Mark auth as attempted
    pub fn set_auth_attempted(&mut self) {
        self.auth_attempted = true;
        if let CallState::Dialing { auth_attempted, .. } = &mut self.state {
            *auth_attempted = true;
        }
    }

    /// Update the invite branch (after re-INVITE with auth)
    pub fn update_invite_branch(&mut self, branch: String) {
        self.last_invite_branch = Some(branch.clone());
        if let CallState::Dialing { invite_branch, .. } = &mut self.state {
            *invite_branch = branch;
        }
    }

    /// Set the to_tag (received in response)
    pub fn set_to_tag(&mut self, tag: String) {
        self.to_tag = Some(tag.clone());
        if let CallState::Ringing { to_tag, .. } = &mut self.state {
            *to_tag = Some(tag);
        }
    }

    /// Increment and return the next CSeq number
    pub fn next_cseq(&mut self) -> u32 {
        self.cseq += 1;
        self.cseq
    }

    /// Set remote RTP address
    pub fn set_remote_rtp(&mut self, addr: SocketAddr) {
        self.remote_rtp_addr = Some(addr);
    }

    /// Set media session for a connected call
    pub fn set_media(&mut self, media: MediaSession) {
        match &mut self.state {
            CallState::Connected { media: m, .. } => *m = Some(media),
            CallState::Held { media: m, .. } => *m = Some(media),
            _ => {}
        }
    }

    /// Set early media session for a ringing call (183 with SDP)
    pub fn set_early_media(&mut self, media: MediaSession) {
        if let CallState::Ringing { early_media, .. } = &mut self.state {
            *early_media = Some(media);
        }
    }

    /// Check if early media is already established
    pub fn has_early_media(&self) -> bool {
        matches!(&self.state, CallState::Ringing { early_media: Some(_), .. })
    }

    /// Take early media session out of a ringing call (consumes it)
    pub fn take_early_media(&mut self) -> Option<MediaSession> {
        if let CallState::Ringing { early_media, .. } = &mut self.state {
            early_media.take()
        } else {
            None
        }
    }

    /// Check if call state is Dialing
    pub fn is_dialing(&self) -> bool {
        matches!(self.state, CallState::Dialing { .. })
    }

    /// Check if call state is Ringing
    pub fn is_ringing(&self) -> bool {
        matches!(self.state, CallState::Ringing { .. })
    }

    /// Check if call state is Incoming
    pub fn is_incoming(&self) -> bool {
        matches!(self.state, CallState::Incoming { .. })
    }

    /// Check if call state is Connected
    pub fn is_connected(&self) -> bool {
        matches!(self.state, CallState::Connected { .. })
    }

    /// Check if call state is Held
    pub fn is_held(&self) -> bool {
        matches!(self.state, CallState::Held { .. })
    }

    /// Get the direction as a string
    pub fn direction_str(&self) -> &str {
        match self.direction {
            CallDirection::Inbound => "inbound",
            CallDirection::Outbound => "outbound",
        }
    }

    /// Process an event and transition to the next state
    ///
    /// Returns the result of the transition attempt.
    pub fn process(&mut self, event: CallFSMEvent) -> TransitionResult {
        // Check for terminal state
        if self.state.is_ended() {
            return TransitionResult::AlreadyEnded;
        }

        let (new_state, result) = self.compute_transition(event);
        if let Some(state) = new_state {
            self.state = state;
        }
        result
    }

    /// Compute the next state for an event without mutating
    fn compute_transition(&self, event: CallFSMEvent) -> (Option<CallState>, TransitionResult) {
        match (&self.state, event) {
            // Dialing transitions
            (CallState::Dialing { .. }, CallFSMEvent::Trying) => {
                // Stay in dialing, just acknowledging the server received it
                (None, TransitionResult::Ok)
            }
            (CallState::Dialing { .. }, CallFSMEvent::RemoteRinging) => (
                Some(CallState::Ringing { to_tag: None, early_media: None }),
                TransitionResult::Ok,
            ),
            (
                CallState::Dialing { .. },
                CallFSMEvent::Answered {
                    to_tag: _,
                    remote_rtp: _,
                    route_set,
                    session_expires,
                },
            ) => {
                // Direct answer without ringing (some endpoints do this)
                (
                    Some(CallState::Connected {
                        media: None,
                        route_set,
                        connected_at: Instant::now(),
                        session_expires,
                    }),
                    TransitionResult::Ok,
                )
            }
            (CallState::Dialing { .. }, CallFSMEvent::AuthChallenge { .. }) => {
                // Stay in dialing, auth will be handled externally
                (None, TransitionResult::Ok)
            }
            (CallState::Dialing { .. }, CallFSMEvent::Cancel) => (
                Some(CallState::Ended {
                    reason: EndReason::Cancelled,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Dialing { .. }, CallFSMEvent::Reject { status }) => (
                Some(CallState::Ended {
                    reason: EndReason::Rejected(status),
                }),
                TransitionResult::Ok,
            ),
            (CallState::Dialing { .. }, CallFSMEvent::Fail { reason }) => (
                Some(CallState::Ended {
                    reason: EndReason::Failed(reason),
                }),
                TransitionResult::Ok,
            ),
            (CallState::Dialing { .. }, CallFSMEvent::LocalHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::Cancelled,
                }),
                TransitionResult::Ok,
            ),

            // Ringing transitions
            (
                CallState::Ringing { .. },
                CallFSMEvent::Answered {
                    to_tag: _,
                    route_set,
                    session_expires,
                    ..
                },
            ) => (
                Some(CallState::Connected {
                    media: None,
                    route_set,
                    connected_at: Instant::now(),
                    session_expires,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Ringing { .. }, CallFSMEvent::Cancel) => (
                Some(CallState::Ended {
                    reason: EndReason::Cancelled,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Ringing { .. }, CallFSMEvent::Reject { status }) => (
                Some(CallState::Ended {
                    reason: EndReason::Rejected(status),
                }),
                TransitionResult::Ok,
            ),
            (CallState::Ringing { .. }, CallFSMEvent::LocalHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::Cancelled,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Ringing { .. }, CallFSMEvent::RemoteHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::RemoteHangup,
                }),
                TransitionResult::Ok,
            ),

            // Incoming call transitions
            (
                CallState::Incoming { .. },
                CallFSMEvent::LocalAnswer { media, remote_rtp: _ },
            ) => (
                Some(CallState::Connected {
                    media: Some(media),
                    route_set: Vec::new(),
                    connected_at: Instant::now(),
                    session_expires: 1800,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Incoming { .. }, CallFSMEvent::LocalHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::LocalHangup,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Incoming { .. }, CallFSMEvent::Cancel) => (
                Some(CallState::Ended {
                    reason: EndReason::Cancelled,
                }),
                TransitionResult::Ok,
            ),

            // Connected transitions
            (
                CallState::Connected { .. },
                CallFSMEvent::Hold,
            ) => {
                // Move media to Held state - handled by hold() method
                (None, TransitionResult::Ok)
            }
            (CallState::Connected { .. }, CallFSMEvent::LocalHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::LocalHangup,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Connected { .. }, CallFSMEvent::RemoteHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::RemoteHangup,
                }),
                TransitionResult::Ok,
            ),
            (
                CallState::Connected { .. },
                CallFSMEvent::MediaEstablished { media: _ },
            ) => {
                // Update media session - handled by set_media() method
                (None, TransitionResult::Ok)
            }

            // Held transitions
            (CallState::Held { .. }, CallFSMEvent::Unhold) => {
                // Move back to Connected - handled by unhold() method
                (None, TransitionResult::Ok)
            }
            (CallState::Held { .. }, CallFSMEvent::LocalHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::LocalHangup,
                }),
                TransitionResult::Ok,
            ),
            (CallState::Held { .. }, CallFSMEvent::RemoteHangup) => (
                Some(CallState::Ended {
                    reason: EndReason::RemoteHangup,
                }),
                TransitionResult::Ok,
            ),

            // Invalid transitions
            (state, event) => (
                None,
                TransitionResult::InvalidTransition {
                    from: state.name(),
                    event: event_name(&event),
                },
            ),
        }
    }

    /// Put the call on hold
    pub fn hold(&mut self) -> TransitionResult {
        match std::mem::replace(&mut self.state, CallState::Idle) {
            CallState::Connected {
                media,
                route_set,
                connected_at,
                session_expires,
            } => {
                if let Some(ref m) = media {
                    m.set_mute(true);
                }
                self.state = CallState::Held {
                    media,
                    route_set,
                    connected_at,
                    session_expires,
                };
                TransitionResult::Ok
            }
            other => {
                self.state = other;
                TransitionResult::InvalidTransition {
                    from: self.state.name(),
                    event: "hold",
                }
            }
        }
    }

    /// Resume the call from hold
    pub fn unhold(&mut self) -> TransitionResult {
        match std::mem::replace(&mut self.state, CallState::Idle) {
            CallState::Held {
                media,
                route_set,
                connected_at,
                session_expires,
            } => {
                if let Some(ref m) = media {
                    m.set_mute(false);
                }
                self.state = CallState::Connected {
                    media,
                    route_set,
                    connected_at,
                    session_expires,
                };
                TransitionResult::Ok
            }
            other => {
                self.state = other;
                TransitionResult::InvalidTransition {
                    from: self.state.name(),
                    event: "unhold",
                }
            }
        }
    }

    /// Stop and remove the media session
    pub fn stop_media(&mut self) {
        match &mut self.state {
            CallState::Connected { media, .. } => {
                if let Some(m) = media.take() {
                    m.stop();
                }
            }
            CallState::Held { media, .. } => {
                if let Some(m) = media.take() {
                    m.stop();
                }
            }
            _ => {}
        }
    }

    /// Transition to ended state with cleanup
    pub fn end(&mut self, reason: EndReason) {
        self.stop_media();
        self.state = CallState::Ended { reason };
    }
}

/// Get a string name for an event (for error messages)
fn event_name(event: &CallFSMEvent) -> &'static str {
    match event {
        CallFSMEvent::Initiate => "initiate",
        CallFSMEvent::Trying => "trying",
        CallFSMEvent::RemoteRinging => "remote_ringing",
        CallFSMEvent::Answered { .. } => "answered",
        CallFSMEvent::LocalAnswer { .. } => "local_answer",
        CallFSMEvent::AuthChallenge { .. } => "auth_challenge",
        CallFSMEvent::Hold => "hold",
        CallFSMEvent::Unhold => "unhold",
        CallFSMEvent::LocalHangup => "local_hangup",
        CallFSMEvent::RemoteHangup => "remote_hangup",
        CallFSMEvent::Cancel => "cancel",
        CallFSMEvent::Reject { .. } => "reject",
        CallFSMEvent::Fail { .. } => "fail",
        CallFSMEvent::MediaEstablished { .. } => "media_established",
    }
}

impl std::fmt::Debug for CallFSM {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallFSM")
            .field("id", &self.id)
            .field("remote_uri", &self.remote_uri)
            .field("direction", &self.direction)
            .field("state", &self.state.name())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outbound_call_happy_path() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        assert_eq!(call.state_name(), "dialing");

        // Receive 180 Ringing
        let result = call.process(CallFSMEvent::RemoteRinging);
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "ringing");

        // Receive 200 OK
        let result = call.process(CallFSMEvent::Answered {
            to_tag: "to-tag".to_string(),
            remote_rtp: None,
            route_set: vec![],
            session_expires: 1800,
        });
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "connected");

        // Remote hangup
        let result = call.process(CallFSMEvent::RemoteHangup);
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "ended");
    }

    #[test]
    fn test_outbound_call_rejected() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        let result = call.process(CallFSMEvent::Reject { status: 486 });
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "ended");
        assert!(matches!(
            call.state(),
            CallState::Ended {
                reason: EndReason::Rejected(486)
            }
        ));
    }

    #[test]
    fn test_inbound_call_happy_path() {
        let mut call = CallFSM::new_inbound(
            "test-account",
            "sip:alice@example.com",
            "call-456".to_string(),
            "from-tag".to_string(),
            "to-tag".to_string(),
            10000,
            "INVITE sip:...".to_string(),
        );

        assert_eq!(call.state_name(), "incoming");

        // Cannot answer without media in this test, so simulate end
        let result = call.process(CallFSMEvent::LocalHangup);
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "ended");
    }

    #[test]
    fn test_hold_unhold() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        // Get to connected state
        call.process(CallFSMEvent::Answered {
            to_tag: "to-tag".to_string(),
            remote_rtp: None,
            route_set: vec![],
            session_expires: 1800,
        });
        assert_eq!(call.state_name(), "connected");

        // Hold
        let result = call.hold();
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "held");

        // Unhold
        let result = call.unhold();
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "connected");
    }

    #[test]
    fn test_invalid_transition() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        // Try to hold while dialing (invalid)
        let result = call.hold();
        assert!(matches!(result, TransitionResult::InvalidTransition { .. }));
        assert_eq!(call.state_name(), "dialing"); // State unchanged
    }

    #[test]
    fn test_already_ended() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        call.end(EndReason::LocalHangup);
        assert_eq!(call.state_name(), "ended");

        // Try to process more events
        let result = call.process(CallFSMEvent::RemoteRinging);
        assert!(matches!(result, TransitionResult::AlreadyEnded));
    }

    #[test]
    fn test_auth_challenge() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        assert!(!call.auth_attempted());
        
        // Receive 401/407 auth challenge
        let result = call.process(CallFSMEvent::AuthChallenge { status: 401 });
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "dialing"); // Back to dialing to retry with auth
        
        call.set_auth_attempted();
        assert!(call.auth_attempted());
    }

    #[test]
    fn test_cancel_while_dialing() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        let result = call.process(CallFSMEvent::Cancel);
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "ended");
        assert!(matches!(
            call.state(),
            CallState::Ended {
                reason: EndReason::Cancelled
            }
        ));
    }

    #[test]
    fn test_cancel_while_ringing() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        call.process(CallFSMEvent::RemoteRinging);
        assert_eq!(call.state_name(), "ringing");

        let result = call.process(CallFSMEvent::Cancel);
        assert!(matches!(result, TransitionResult::Ok));
        assert_eq!(call.state_name(), "ended");
    }

    #[test]
    fn test_call_direction() {
        let outbound = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );
        assert_eq!(outbound.direction_str(), "outbound");
        assert!(outbound.is_dialing());

        let inbound = CallFSM::new_inbound(
            "test-account",
            "sip:alice@example.com",
            "call-456".to_string(),
            "from-tag".to_string(),
            "to-tag".to_string(),
            10000,
            "INVITE sip:...".to_string(),
        );
        assert_eq!(inbound.direction_str(), "inbound");
        assert!(inbound.is_incoming());
    }

    #[test]
    fn test_remote_hangup_while_connected() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        // Get to connected state
        call.process(CallFSMEvent::Answered {
            to_tag: "to-tag".to_string(),
            remote_rtp: None,
            route_set: vec![],
            session_expires: 1800,
        });
        assert!(call.is_connected());

        let result = call.process(CallFSMEvent::RemoteHangup);
        assert!(matches!(result, TransitionResult::Ok));
        assert!(call.is_ended());
        assert!(matches!(
            call.state(),
            CallState::Ended {
                reason: EndReason::RemoteHangup
            }
        ));
    }

    #[test]
    fn test_local_hangup_while_connected() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        // Get to connected state
        call.process(CallFSMEvent::Answered {
            to_tag: "to-tag".to_string(),
            remote_rtp: None,
            route_set: vec![],
            session_expires: 1800,
        });

        let result = call.process(CallFSMEvent::LocalHangup);
        assert!(matches!(result, TransitionResult::Ok));
        assert!(matches!(
            call.state(),
            CallState::Ended {
                reason: EndReason::LocalHangup
            }
        ));
    }

    #[test]
    fn test_call_fail() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        let result = call.process(CallFSMEvent::Fail {
            reason: "Network error".to_string(),
        });
        assert!(matches!(result, TransitionResult::Ok));
        assert!(matches!(
            call.state(),
            CallState::Ended {
                reason: EndReason::Failed(_)
            }
        ));
    }

    #[test]
    fn test_cseq_increment() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        assert_eq!(call.cseq, 1);
        assert_eq!(call.next_cseq(), 2);
        assert_eq!(call.cseq, 2);
        assert_eq!(call.next_cseq(), 3);
        assert_eq!(call.cseq, 3);
    }

    #[test]
    fn test_route_set_and_session_expires() {
        let mut call = CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-123".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
        );

        // Connect with route set and session expires
        call.process(CallFSMEvent::Answered {
            to_tag: "to-tag".to_string(),
            remote_rtp: None,
            route_set: vec!["<sip:proxy.example.com;lr>".to_string()],
            session_expires: 900,
        });

        assert_eq!(call.route_set(), &["<sip:proxy.example.com;lr>"]);
        assert_eq!(call.session_expires(), 900);
    }
}
