//! Moving a call's media when the far end's SDP changes mid-call.
//!
//! rtp-engine cannot redirect, re-key or re-codec a running session, so a
//! change means a new session. The PBX re-INVITEs Aria onto another relay
//! port (with a new SRTP key on an SRTP call) for every attended transfer
//! that lands on an Aria user; answering that 200 OK and carrying on with the
//! old session sent the user's audio to a released port and failed SRTP
//! authentication on everything coming back.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

use crate::sip::account::SrtpMode;
use crate::sip::builder;
use crate::sip::media::{self, MediaPlan};
use crate::sip::recording_parts;
use crate::sip::{CallEvent, ManagerState, SipEvent};

/// How long a session that must keep its local port waits for the old one
/// to let go of it: rtp-engine's RTCP task only notices a stop on its next
/// five-second tick.
const SAME_PORT_RESTART_WINDOW: Duration = Duration::from_secs(6);

/// Replace a call's media with a session on `plan`, on the same local port.
///
/// For the 200 OK that answers our offer differently from the 183 that came
/// before it: our offer named the port, so the new session must have it, and
/// waits (retrying) until the stopped early-media session has released it.
pub(crate) async fn restart_media_on_offer_port(
    state: Arc<RwLock<ManagerState>>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
    call_id: String,
    plan: MediaPlan,
    sdp: String,
) {
    let deadline = tokio::time::Instant::now() + SAME_PORT_RESTART_WINDOW;
    let started = loop {
        let alive = state.read().await.find_call(&call_id).is_some_and(|(_, c)| c.is_established());
        if !alive {
            return;
        }
        match media::start_call_media(&plan, &sdp).await {
            Ok(started) => break started,
            Err(e) if tokio::time::Instant::now() < deadline => {
                log::debug!("Media restart for call {} waiting for its port: {}", call_id, e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                log::error!("Could not restart media for call {}: {}", call_id, e);
                return;
            }
        }
    };
    let mut s = state.write().await;
    let (account_id, security_changed) = match s.find_call_mut(&call_id) {
        Some((account_id, call)) if call.is_established() && call.media().is_none() => {
            let security_changed = security_update(call, started.srtp_active);
            call.srtp_active = started.srtp_active;
            call.set_remote_rtp(plan.remote.addr);
            call.remote_media = Some(plan.remote.for_session(started.srtp_active));
            call.set_media(started.session);
            (account_id.to_string(), security_changed)
        }
        _ => {
            started.session.stop();
            return;
        }
    };
    let recording = s
        .find_call(&call_id)
        .and_then(|(_, call)| call.media())
        .and_then(|m| s.start_auto_record(&account_id, &call_id, m));
    drop(s);
    if let Some(event) = security_changed {
        let _ = event_tx.send(SipEvent::CallStateChanged(event));
    }
    if let Some(event) = recording {
        let _ = event_tx.send(SipEvent::RecordingStateChanged(event));
    }
}

/// The call event that moves the call screen's padlock, when a new session
/// changes whether `call`'s media is encrypted. It carries the call's state
/// as it is, so it changes nothing else on screen; the event forwarder adds
/// the signalling transport, as it does for every event with `srtp_active`.
fn security_update(call: &crate::sip::state::CallFSM, srtp_active: bool) -> Option<CallEvent> {
    (call.srtp_active != srtp_active).then(|| {
        let mut event = CallEvent::from_fsm(call, None);
        event.srtp_active = Some(srtp_active);
        event
    })
}

/// Answer a re-INVITE in an established call: the 200 OK (or a 488 when no
/// offered codec can be carried, or the new media cannot be started).
///
/// When the offer moves media — another address or port, codec, or SRTP key
/// — the call gets a new session on a new local port, and the answer names
/// that port. A plain session refresh (the same SDP again) leaves the running
/// session alone. The answer mirrors the offer's direction and keeps the
/// dialog's `o=` session id.
pub(crate) async fn answer_reinvite(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    call_id: &str,
    account_id: &str,
    text: &str,
) -> Option<String> {
    let offer = builder::request_body(text);
    let ctx = {
        let s = state.read().await;
        let account = s.get_account(account_id)?;
        let (_, call) = s.find_call(call_id)?;
        let la = account.local_addr?;
        let media_ip = call.sdp_public_ip.unwrap_or_else(|| {
            media::sdp_address(
                la.ip(),
                account.public_addr.map(|a| a.ip()),
                account.server_addr.map(|a| a.ip()),
            )
        });
        ReinviteContext {
            local_addr: la,
            username: account.config.username.clone(),
            codecs: account.config.codecs.clone(),
            srtp_mode: account.config.srtp_mode,
            media_ip: media_ip.to_string(),
            our_tag: call.to_tag.clone().unwrap_or_default(),
            local_port: call.local_rtp_port,
            local_srtp_key: call.local_srtp_key.clone(),
            srtp_active: call.srtp_active,
            has_media: call.media().is_some(),
            current: call.remote_media.clone(),
            input_device: s.preferred_input_device.clone(),
            output_device: s.preferred_output_device.clone(),
        }
    };
    let ReinviteContext { local_addr, ref username, our_tag: ref tag, .. } = ctx;

    if !offer.contains("m=audio") {
        // No offer: ours goes in the 200 and the answer comes in the ACK,
        // which nothing here reads yet. As before.
        let key = if ctx.srtp_active { ctx.local_srtp_key.as_deref() } else { None };
        return builder::build_200_ok_invite_srtp(
            text,
            local_addr,
            ctx.local_port,
            tag,
            Some(username),
            Some(&ctx.media_ip),
            key,
        );
    }

    let Some(mut next) = media::remote_media(offer, Some(&ctx.codecs)) else {
        log::warn!("re-INVITE for call {} offers nothing this phone can carry; 488", call_id);
        return builder::build_final_response(text, 488, "Not Acceptable Here", tag);
    };
    let use_srtp = ctx.srtp_mode == SrtpMode::Sdes
        && builder::sdp_offers_savp(offer)
        && next.srtp_key.is_some();
    if !use_srtp {
        next.srtp_key = None;
    }
    // A hold the old way (`c=0.0.0.0`) is not somewhere to send to.
    let unspecified = next.addr.ip().is_unspecified() || next.addr.port() == 0;
    if unspecified {
        if let Some(current) = &ctx.current {
            next.addr = current.addr;
        }
    }
    let restart = !unspecified
        && (!ctx.has_media
            || use_srtp != ctx.srtp_active
            || ctx.current.as_ref().is_none_or(|c| c.needs_restart_for(&next)));
    let our_key = match (use_srtp, &ctx.local_srtp_key) {
        (false, _) => None,
        (true, Some(key)) => Some(key.clone()),
        (true, None) => rtp_engine::srtp::SrtpContext::generate().ok().map(|(_, key)| key),
    };
    if use_srtp && our_key.is_none() {
        log::error!("Could not generate an SRTP key for call {}'s re-INVITE; 488", call_id);
        return builder::build_final_response(text, 488, "Not Acceptable Here", tag);
    }

    let mut local_port = ctx.local_port;
    let mut replaced = None;
    if restart {
        log::info!(
            "re-INVITE moves call {}'s media to {} ({:?} on {}); starting a new session",
            call_id, next.addr, next.audio.codec, next.audio.pt
        );
        let started = match media::allocate_free_rtp_port().await {
            Ok(port) => {
                let plan = MediaPlan {
                    local_port: port,
                    remote: next.clone(),
                    local_srtp_key: our_key.clone(),
                    input_device: ctx.input_device.clone(),
                    output_device: ctx.output_device.clone(),
                };
                media::start_call_media(&plan, offer).await.map(|s| (port, s))
            }
            Err(e) => Err(e),
        };
        match started {
            Ok((port, started)) => {
                local_port = port;
                replaced = Some(started);
            }
            Err(e) => {
                log::error!("Could not start the moved media for call {}: {}; 488", call_id, e);
                return builder::build_final_response(text, 488, "Not Acceptable Here", tag);
            }
        }
    }

    let answer = builder::build_sdp_answer_with(
        offer,
        &builder::AnswerOptions {
            ip: ctx.media_ip.clone(),
            rtp_port: local_port,
            crypto_b64_key: our_key.as_deref(),
            codecs: Some(&ctx.codecs),
            origin: None,
        },
    )?;
    let remote_hold = !builder::offer_direction(offer).mirror().sends();

    let (answer, old, continued_in, security_changed) = {
        let mut s = state.write().await;
        let recordings_dir = s.recordings_dir.clone();
        let (_, call) = s.find_call_mut(call_id)?;
        let mut old = None;
        let mut continued_in = None;
        let mut security_changed = None;
        if let Some(started) = replaced {
            old = call.take_media();
            let was_recording = old.as_ref().is_some_and(|m| m.is_recording());
            security_changed = security_update(call, started.srtp_active);
            call.local_rtp_port = local_port;
            call.srtp_active = started.srtp_active;
            call.set_remote_rtp(next.addr);
            call.remote_media = Some(next.clone().for_session(started.srtp_active));
            if use_srtp {
                call.local_srtp_key = our_key.clone();
            }
            // The recording carries on in the new session, and is joined
            // onto the part the old session saves below. Registered before
            // the lock goes, so a hangup that saves the new part first
            // still waits for the old one.
            if let (true, Some(dir)) = (was_recording, recordings_dir) {
                let path = recording_parts::continuation_path(call_id, &dir);
                match started.session.start_recording(path.clone()) {
                    Ok(()) => {
                        recording_parts::expect(&path);
                        continued_in = Some(path);
                    }
                    Err(e) => {
                        log::warn!("Could not keep recording call {} after its media moved: {}", call_id, e);
                    }
                }
            }
            call.set_media(started.session);
        } else if !unspecified {
            call.set_remote_rtp(next.addr);
        }
        call.set_remote_hold(remote_hold);
        (call.stamp_local_sdp(&answer), old, continued_in, security_changed)
    };
    if let Some(event) = security_changed {
        let _ = event_tx.send(SipEvent::CallStateChanged(event));
    }
    // Saving a recording writes it all to disk: not under the lock.
    match (old, continued_in) {
        (Some(old), Some(path)) => crate::sip::finish_replaced_media(old, path).await,
        // Not recording, or the recording could not carry on: the part so
        // far is all there will be. History only links the recording on
        // the call's "ended" event, so it is not reported here.
        (old @ Some(_), None) => {
            crate::sip::finish_call_media(call_id, old).await;
        }
        (None, Some(path)) => recording_parts::resolve(&path, None),
        (None, None) => {}
    }
    builder::build_200_ok_invite_with_sdp(text, local_addr, tag, Some(username), &answer)
}

/// What [`answer_reinvite`] reads from the call and account, under one lock.
struct ReinviteContext {
    local_addr: SocketAddr,
    username: String,
    codecs: Vec<crate::sip::account::CodecPreference>,
    srtp_mode: SrtpMode,
    media_ip: String,
    our_tag: String,
    local_port: u16,
    local_srtp_key: Option<String>,
    srtp_active: bool,
    has_media: bool,
    current: Option<media::RemoteMedia>,
    input_device: Option<String>,
    output_device: Option<String>,
}
