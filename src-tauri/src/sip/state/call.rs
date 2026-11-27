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

use std::net::{IpAddr, SocketAddr};
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
    /// The RTP and RTCP sockets on `local_rtp_port`, bound since the port was
    /// chosen and held until the call's first media session takes them
    /// (`take_reserved_rtp`). Our SDP names the port before media starts —
    /// in the INVITE we send, or in the answer to one that rings first — and
    /// a released port can be handed to another socket in between, which
    /// failed the call's media with "address in use" on Windows (#93).
    /// Dropped when the call ends.
    pub reserved_rtp: Option<crate::sip::media::RtpSockets>,
    /// The connection address (`c=`) our SDP for this call carries.
    ///
    /// Decided once, when the first offer or answer is built — from a per-call
    /// STUN probe, the registrar's view of us, or the local interface — and
    /// reused by everything that rebuilds the SDP later: the authenticated
    /// INVITE after a 401/407, the 200 OK to a re-INVITE. Those used to
    /// recompute it from the account's registration-time address alone, so
    /// the retry could offer a different address from the INVITE it replaced
    /// (the LAN address after a STUN-discovered public one), and a server
    /// that sends RTP where the SDP says got one-way audio.
    pub sdp_public_ip: Option<IpAddr>,
    /// Remote RTP address
    pub remote_rtp_addr: Option<SocketAddr>,
    /// Last INVITE branch (for CANCEL matching)
    pub last_invite_branch: Option<String>,
    /// CSeq of the most recent INVITE we sent. A CANCEL must carry it (RFC
    /// 3261 §9.1); after an auth retry it is no longer 1.
    pub invite_cseq: u32,
    /// Whether auth has been attempted (to prevent loops)
    pub auth_attempted: bool,
    /// When the call reached `Ended`, so the record can be pruned later.
    ///
    /// The record outlives the call on purpose: a 200 OK that crosses our
    /// CANCEL, or a retransmitted final response, still has to be matched to
    /// the dialog and answered (ACK, then BYE) for up to Timer H (32 s).
    pub ended_at: Option<Instant>,
    /// Local SRTP key (base64 encoded) for outbound encryption
    pub local_srtp_key: Option<String>,
    /// Whether this call's media is actually encrypted.
    ///
    /// Set when the media session starts, from what SRTP negotiation produced
    /// — not from configuration. The call screen used to derive its padlock
    /// from `srtp_mode`, so it claimed encryption whenever the setting was on,
    /// including for the since-removed DTLS option and whenever a peer stripped the
    /// crypto line from its answer.
    pub srtp_active: bool,
    /// Remote Contact URI from 200 OK (the URI to send BYE/re-INVITE to)
    pub remote_contact: Option<String>,
    /// Our local SIP URI (e.g., sip:1001@lyonscomm.com)
    pub local_uri: String,
    /// The 200 OK we answered an incoming INVITE with, until its ACK arrives.
    ///
    /// RFC 3261 §13.3.1.4: the UAS core retransmits its 2xx (Timer G) until
    /// the ACK, and gives the call up after 64*T1 (Timer H). It is kept here
    /// so the ACK handler can stop that, and a retransmitted INVITE can be
    /// answered with the same 200.
    pub pending_2xx: Option<Pending2xx>,
    /// The 4xx-6xx we answered an incoming INVITE with (a decline, a 487
    /// after CANCEL), until its ACK arrives or Timer H passes. Resent on
    /// Timer G meanwhile, and sent again for a retransmitted INVITE. It
    /// outlives the call, which ended when the response was sent.
    pub pending_final: Option<Pending2xx>,
    /// Whether we have sent a BYE for this dialog. A 2xx retransmitted after
    /// it is ACKed again but must not bring another BYE.
    pub bye_sent: bool,
    /// The user's Mute button. Kept apart from hold: the session's TX mute
    /// is [`Self::tx_muted`], so resuming a held call (or merging it) can no
    /// longer open a microphone the user had muted, and a mute pressed before
    /// there is any media is applied when the session attaches.
    pub user_muted: bool,
    /// The far end asked us not to send: its offer was `sendonly` or
    /// `inactive`, so our answer is `recvonly` or `inactive`.
    pub remote_hold: bool,
    /// What the running media session was started against, to tell whether
    /// a later SDP from the far end needs a new session.
    pub remote_media: Option<crate::sip::media::RemoteMedia>,
    /// The `o=` session id of every SDP this side sends in the dialog.
    pub sdp_session_id: u64,
    /// The `o=` version of the last SDP this side sent.
    pub sdp_version: u64,
    /// The last SDP this side sent, without its `o=` line, so an unchanged
    /// one keeps its version (RFC 3264 §8).
    last_local_sdp: Option<String>,
    /// Current state
    state: CallState,
}

/// A 2xx response to an INVITE that has not been ACKed yet.
#[derive(Debug, Clone)]
pub struct Pending2xx {
    /// The CSeq number of the INVITE it answers; the ACK carries the same.
    pub cseq: u32,
    /// The response as sent, for retransmission.
    pub response: String,
}

/// Parameters for creating an inbound call FSM.
pub struct InboundCallParams {
    pub account_id: String,
    pub remote_uri: String,
    pub call_id: String,
    pub from_tag: String,
    pub to_tag: String,
    pub local_rtp_port: u16,
    pub raw_invite: String,
    pub local_uri: String,
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
        local_uri: String,
    ) -> Self {
        let sdp_session_id = crate::sip::builder::sdp_session_id(&call_id);
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
            reserved_rtp: None,
            sdp_public_ip: None,
            remote_rtp_addr: None,
            last_invite_branch: Some(invite_branch.clone()),
            invite_cseq: 1,
            auth_attempted: false,
            ended_at: None,
            local_srtp_key: None,
            srtp_active: false,
            remote_contact: None,
            local_uri,
            pending_2xx: None,
            pending_final: None,
            bye_sent: false,
            user_muted: false,
            remote_hold: false,
            remote_media: None,
            sdp_session_id,
            // Our INVITE's offer went out as version 1.
            sdp_version: 1,
            last_local_sdp: None,
            state: CallState::Dialing {
                invite_branch,
                auth_attempted: false,
            },
        }
    }

    /// Create a new inbound call FSM
    pub fn new_inbound(params: InboundCallParams) -> Self {
        let sdp_session_id = crate::sip::builder::sdp_session_id(&params.call_id);
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: params.account_id,
            remote_uri: params.remote_uri,
            direction: CallDirection::Inbound,
            call_id_header: params.call_id,
            from_tag: params.from_tag,
            to_tag: Some(params.to_tag),
            cseq: 1,
            local_rtp_port: params.local_rtp_port,
            reserved_rtp: None,
            sdp_public_ip: None,
            remote_rtp_addr: None,
            last_invite_branch: None,
            invite_cseq: 1,
            auth_attempted: false,
            ended_at: None,
            local_srtp_key: None,
            srtp_active: false,
            remote_contact: None,
            local_uri: params.local_uri,
            pending_2xx: None,
            pending_final: None,
            bye_sent: false,
            user_muted: false,
            remote_hold: false,
            remote_media: None,
            sdp_session_id,
            sdp_version: 0,
            last_local_sdp: None,
            state: CallState::Incoming {
                raw_invite: params.raw_invite,
            },
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
            CallState::Connected { connected_at, .. } | CallState::Held { connected_at, .. } => {
                Some(*connected_at)
            }
            _ => None,
        }
    }

    /// Get the session expiry time in seconds
    pub fn session_expires(&self) -> u32 {
        match &self.state {
            CallState::Connected {
                session_expires, ..
            }
            | CallState::Held {
                session_expires, ..
            } => *session_expires,
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
        // The field, not the Dialing state's copy: a second challenge can
        // arrive after a 180 has moved the call to Ringing, and reading only
        // the Dialing variant made that look like a first challenge, which
        // sent a third INVITE on the same dialog.
        self.auth_attempted
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
        media.set_mute(self.tx_muted());
        match &mut self.state {
            CallState::Connected { media: m, .. } => *m = Some(media),
            CallState::Held { media: m, .. } => *m = Some(media),
            _ => {}
        }
    }

    /// Set early media session for a ringing call (183 with SDP)
    pub fn set_early_media(&mut self, media: MediaSession) {
        media.set_mute(self.tx_muted());
        if let CallState::Ringing { early_media, .. } = &mut self.state {
            *early_media = Some(media);
        }
    }

    /// The session carrying this call's audio, including early media on a
    /// call that is still ringing. Mute and DTMF go here: an IVR played in a
    /// 183 has to hear the keypad, and a mute pressed while ringing has to
    /// hold for the early-media session the 200 OK then carries on.
    pub fn media_any(&self) -> Option<&MediaSession> {
        match &self.state {
            CallState::Ringing { early_media, .. } => early_media.as_ref(),
            _ => self.media(),
        }
    }

    /// Whether the microphone must be kept off the wire: the user muted, the
    /// call is on hold here, or the far end asked us not to send.
    pub fn tx_muted(&self) -> bool {
        self.user_muted || self.is_held() || self.remote_hold
    }

    /// Push [`Self::tx_muted`] to the session, if there is one.
    pub fn apply_mute(&self) {
        if let Some(media) = self.media_any() {
            media.set_mute(self.tx_muted());
        }
    }

    /// The user pressed Mute or Unmute. Stored on the call whatever state it
    /// is in, so it outlives hold and reaches a session that starts later.
    pub fn set_user_mute(&mut self, muted: bool) {
        self.user_muted = muted;
        self.apply_mute();
    }

    /// The far end's offer asked us to stop sending, or to start again.
    pub fn set_remote_hold(&mut self, held: bool) {
        self.remote_hold = held;
        self.apply_mute();
    }

    /// Give `sdp`, about to be sent in this dialog, the dialog's `o=` session
    /// id and the right version: the last one again when nothing else
    /// changed, the next one when something did (RFC 3264 §8).
    pub fn stamp_local_sdp(&mut self, sdp: &str) -> String {
        let body = crate::sip::builder::sdp_without_origin(sdp);
        let unchanged = self.last_local_sdp.as_deref() == Some(body.as_str());
        if !unchanged {
            self.sdp_version += 1;
        }
        self.last_local_sdp = Some(body);
        crate::sip::builder::with_sdp_origin(
            sdp,
            crate::sip::builder::SdpOrigin {
                id: self.sdp_session_id,
                version: self.sdp_version,
            },
        )
    }

    /// Check if early media is already established
    pub fn has_early_media(&self) -> bool {
        matches!(
            &self.state,
            CallState::Ringing {
                early_media: Some(_),
                ..
            }
        )
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
            if self.state.is_ended() {
                self.ended_at = Some(Instant::now());
                // The record lingers for retransmissions; the ports need not.
                self.reserved_rtp = None;
            }
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
                Some(CallState::Ringing {
                    to_tag: None,
                    early_media: None,
                }),
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
                CallFSMEvent::LocalAnswer {
                    media,
                    remote_rtp: _,
                },
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
            (CallState::Connected { .. }, CallFSMEvent::Hold) => {
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
            (CallState::Connected { .. }, CallFSMEvent::MediaEstablished { media: _ }) => {
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
                self.state = CallState::Held {
                    media,
                    route_set,
                    connected_at,
                    session_expires,
                };
                self.apply_mute();
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
                self.state = CallState::Connected {
                    media,
                    route_set,
                    connected_at,
                    session_expires,
                };
                // Resuming opens the microphone only if the user had not
                // muted it: hold and mute used to share one flag, so Mute,
                // Hold, Resume put the user back on the air while the button
                // still said "Unmute".
                self.apply_mute();
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

    /// Remove the media session, including unanswered early media, and hand
    /// it to the caller.
    ///
    /// Teardown uses this instead of `stop_media` so the session is dropped
    /// outside the state lock: dropping it saves any active recording, which
    /// is a synchronous write of the whole call to disk.
    pub fn take_media(&mut self) -> Option<MediaSession> {
        match &mut self.state {
            CallState::Connected { media, .. } | CallState::Held { media, .. } => media.take(),
            CallState::Ringing { early_media, .. } => early_media.take(),
            _ => None,
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
        // Nothing left to retransmit for a call that is over.
        self.pending_2xx = None;
        self.state = CallState::Ended { reason };
        self.ended_at = Some(Instant::now());
        self.reserved_rtp = None;
    }

    /// The call's held RTP and RTCP sockets, for the media session about to
    /// start on `local_rtp_port`. `None` once a session has taken them, or if
    /// the port was never held, and the session then binds the port itself.
    pub fn take_reserved_rtp(&mut self) -> Option<crate::sip::media::RtpSockets> {
        self.reserved_rtp
            .take()
            .filter(|sockets| sockets.port() == self.local_rtp_port)
    }

    /// The dialog's tags as (ours, theirs).
    ///
    /// `from_tag`/`to_tag` are stored as the INVITE carried them, so which one
    /// is ours depends on who sent the INVITE. Every in-dialog request we
    /// originate — BYE, REFER, NOTIFY — puts our tag in From and theirs in To,
    /// whichever direction the call was. REFER used to pass `from_tag` as
    /// ours regardless, so transferring an inbound call got a 481.
    pub fn dialog_tags(&self) -> (String, String) {
        let to_tag = self.to_tag.clone().unwrap_or_default();
        match self.direction {
            CallDirection::Outbound => (self.from_tag.clone(), to_tag),
            CallDirection::Inbound => (to_tag, self.from_tag.clone()),
        }
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
        let mut call = CallFSM::new_inbound(InboundCallParams {
            account_id: "test-account".to_string(),
            remote_uri: "sip:alice@example.com".to_string(),
            call_id: "call-456".to_string(),
            from_tag: "from-tag".to_string(),
            to_tag: "to-tag".to_string(),
            local_rtp_port: 10000,
            raw_invite: "INVITE sip:...".to_string(),
            local_uri: "sip:bob@example.com".to_string(),
        });

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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
        );
        assert_eq!(outbound.direction_str(), "outbound");
        assert!(outbound.is_dialing());

        let inbound = CallFSM::new_inbound(InboundCallParams {
            account_id: "test-account".to_string(),
            remote_uri: "sip:alice@example.com".to_string(),
            call_id: "call-456".to_string(),
            from_tag: "from-tag".to_string(),
            to_tag: "to-tag".to_string(),
            local_rtp_port: 10000,
            raw_invite: "INVITE sip:...".to_string(),
            local_uri: "sip:bob@example.com".to_string(),
        });
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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
            "sip:alice@example.com".to_string(),
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

    fn outbound() -> CallFSM {
        CallFSM::new_outbound(
            "test-account",
            "sip:bob@example.com",
            "call-mute".to_string(),
            "from-tag".to_string(),
            10000,
            "branch-1".to_string(),
            "sip:alice@example.com".to_string(),
        )
    }

    fn answer(call: &mut CallFSM) {
        let result = call.process(CallFSMEvent::Answered {
            to_tag: "to-tag".to_string(),
            remote_rtp: None,
            route_set: vec![],
            session_expires: 1800,
        });
        assert!(matches!(result, TransitionResult::Ok));
    }

    /// A real session to loopback; no device needs to open for the mute flag.
    async fn session() -> MediaSession {
        let sockets = crate::sip::media::allocate_rtp_sockets()
            .await
            .expect("a port");
        MediaSession::start_on_sockets(
            sockets,
            "127.0.0.1:9".parse().unwrap(),
            rtp_engine::CodecType::Pcmu,
        )
        .expect("a session")
    }

    /// S5/M2: Mute, Hold, Resume used to open the microphone, because hold
    /// and mute shared the session's one flag.
    #[tokio::test]
    async fn mute_then_hold_then_resume_stays_muted() {
        let mut call = outbound();
        answer(&mut call);
        call.set_media(session().await);
        call.set_user_mute(true);
        assert!(call.media().unwrap().is_muted());
        call.hold();
        assert!(call.media().unwrap().is_muted());
        call.unhold();
        assert!(
            call.media().unwrap().is_muted(),
            "resume opened a muted microphone"
        );
        call.set_user_mute(false);
        assert!(!call.media().unwrap().is_muted());
    }

    /// Unmuting a held call must not put the user on the air; resuming does.
    #[tokio::test]
    async fn unmute_while_held_keeps_the_microphone_off() {
        let mut call = outbound();
        answer(&mut call);
        call.set_media(session().await);
        call.hold();
        call.set_user_mute(true);
        call.set_user_mute(false);
        assert!(
            call.media().unwrap().is_muted(),
            "unmuting a held call opened the microphone"
        );
        call.unhold();
        assert!(!call.media().unwrap().is_muted());
    }

    /// M7: a mute pressed while the call rings reaches the early-media
    /// session, and the session the call connects on.
    #[tokio::test]
    async fn mute_while_ringing_applies_to_early_media_and_on_connect() {
        let mut call = outbound();
        call.process(CallFSMEvent::RemoteRinging);
        call.set_user_mute(true);
        call.set_early_media(session().await);
        assert!(
            call.media_any().unwrap().is_muted(),
            "early media started unmuted"
        );

        // Connects on the early session (the 200 handler takes and re-sets it).
        let early = call.take_early_media().unwrap();
        answer(&mut call);
        early.set_mute(false); // whatever the session said, the call decides
        call.set_media(early);
        assert!(call.media().unwrap().is_muted(), "connect dropped the mute");

        // And with no early media at all: muted while ringing, fresh session.
        let mut call = outbound();
        call.process(CallFSMEvent::RemoteRinging);
        call.set_user_mute(true);
        answer(&mut call);
        call.set_media(session().await);
        assert!(call.media().unwrap().is_muted());
    }

    /// The far end holding us (a sendonly offer) keeps us from sending, and
    /// leaves the user's own mute alone.
    #[tokio::test]
    async fn remote_hold_mutes_without_touching_the_users_mute() {
        let mut call = outbound();
        answer(&mut call);
        call.set_media(session().await);
        call.set_remote_hold(true);
        assert!(call.media().unwrap().is_muted());
        assert!(!call.user_muted);
        call.set_remote_hold(false);
        assert!(!call.media().unwrap().is_muted());
    }

    /// RFC 3264 §8: one `o=` id per dialog, and the version moves only when
    /// the SDP does. Our INVITE's offer was version 1.
    #[test]
    fn local_sdp_keeps_the_dialog_origin_and_bumps_the_version_on_change() {
        let mut call = outbound();
        let a = "v=0\r\no=aria 1 1 IN IP4 10.0.0.2\r\nm=audio 30000 RTP/AVP 0\r\n";
        let b = "v=0\r\no=aria 5 5 IN IP4 10.0.0.2\r\nm=audio 30002 RTP/AVP 0\r\n";
        let id = crate::sip::builder::sdp_session_id("call-mute");
        let first = call.stamp_local_sdp(a);
        assert!(
            first.contains(&format!("o=aria {id} 2 IN IP4 10.0.0.2\r\n")),
            "{first}"
        );
        let again = call.stamp_local_sdp(a);
        assert_eq!(again, first, "an unchanged SDP keeps its version");
        let moved = call.stamp_local_sdp(b);
        assert!(
            moved.contains(&format!("o=aria {id} 3 IN IP4 10.0.0.2\r\n")),
            "{moved}"
        );

        // An inbound call's first answer is version 1.
        let mut inbound = CallFSM::new_inbound(InboundCallParams {
            account_id: "a".into(),
            remote_uri: "sip:x@y".into(),
            call_id: "in-1".into(),
            from_tag: "f".into(),
            to_tag: "t".into(),
            local_rtp_port: 10000,
            raw_invite: String::new(),
            local_uri: "sip:me@y".into(),
        });
        let id = crate::sip::builder::sdp_session_id("in-1");
        assert!(inbound
            .stamp_local_sdp(a)
            .contains(&format!("o=aria {id} 1 IN")));
    }
}
