use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::builder::{
    self, build_200_ok_invite_with_user, build_202_accepted, build_notify_refer,
    build_simple_response, extract_from_tag, extract_header, extract_via_branch,
    parse_sdp_connection, parse_sipfrag_status,
};
use super::state::{CallFSM, CallFSMEvent, InboundCallParams};
use super::media;
use super::replaces;
use super::{CallEvent, ManagerState, SipEvent, TransferEvent};

/// Handle an incoming REFER request: accept, start transfer, send NOTIFY updates
pub async fn handle_incoming_refer(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    remote: SocketAddr,
    account_id: &str,
) {
    let call_id_header = match extract_header(text, "Call-ID") {
        Some(c) => c,
        None => return,
    };
    let refer_to = match extract_header(text, "Refer-To") {
        Some(r) => r.trim_matches(|c| c == '<' || c == '>').to_string(),
        None => {
            log::warn!("REFER missing Refer-To header");
            return;
        }
    };

    let call_internal_id = {
        let s = state.read().await;
        let account = match s.get_account(account_id) {
            Some(a) => a,
            None => return,
        };
        match account
            .calls
            .iter()
            .find(|c| c.call_id_header == call_id_header)
        {
            Some(c) => c.id.clone(),
            None => {
                log::warn!("REFER for unknown call {}", call_id_header);
                return;
            }
        }
    };

    let to_tag = {
        let s = state.read().await;
        let account = match s.get_account(account_id) {
            Some(a) => a,
            None => return,
        };
        account
            .calls
            .iter()
            .find(|c| c.id == call_internal_id)
            .and_then(|c| c.to_tag.clone())
            .unwrap_or_else(builder::generate_tag)
    };

    // Send 202 Accepted
    if let Some(resp) = build_202_accepted(text, &to_tag) {
        let s = state.read().await;
        if let Some(account) = s.get_account(account_id) {
            if let Some(ref transport) = account.transport {
                let _ = transport.send_to(resp.as_bytes(), remote).await;
            }
        }
    }

    let _ = event_tx.send(SipEvent::TransferProgress(TransferEvent {
        account_id: account_id.to_string(),
        call_id: call_internal_id.clone(),
        status: 100,
        message: format!("Received REFER to {}", refer_to),
    }));

    // Extract dialog info for sending NOTIFY
    let (notify_target, notify_remote_uri, notify_local_uri, notify_call_id, notify_from_tag, notify_to_tag, transport_str, transport) = {
        let s = state.read().await;
        let account = match s.get_account(account_id) {
            Some(a) => a,
            None => return,
        };
        let call = match account.calls.iter().find(|c| c.id == call_internal_id) {
            Some(c) => c,
            None => return,
        };
        let transport = match &account.transport {
            Some(t) => t.clone(),
            None => return,
        };
        let tp = account.config.transport.param().to_string();
        // Ours in From, theirs in To (RFC 3261 §12.2.1.1); `from_tag` is the
        // caller's tag on an inbound call, and sending that as ours got every
        // NOTIFY a 481. The request goes to the remote Contact, like the BYE.
        let (our_tag, their_tag) = call.dialog_tags();
        (
            call.remote_contact.clone().unwrap_or_else(|| call.remote_uri.clone()),
            call.remote_uri.clone(),
            call.local_uri.clone(),
            call.call_id_header.clone(),
            our_tag,
            their_tag,
            tp,
            transport,
        )
    };

    // Send initial NOTIFY (100 Trying). Built under the lock, sent after it:
    // the transaction layer needs the state too.
    let notify = {
        let mut s = state.write().await;
        let account = match s.get_account_mut(account_id) {
            Some(a) => a,
            None => return,
        };
        account.calls.iter_mut().find(|c| c.id == call_internal_id).map(|call| {
            call.cseq += 1;
            let cseq = call.cseq;
            let local_addr = transport.local_addr();
            build_notify_refer(
                &notify_target,
                &notify_local_uri,
                &notify_remote_uri,
                local_addr,
                &notify_call_id,
                cseq,
                &notify_from_tag,
                &notify_to_tag,
                &transport_str,
                100,
                "Trying",
                "active;expires=60",
            )
        })
    };
    if let Some(notify) = notify {
        let _ = super::send_non_invite(state, &transport, remote, notify).await;
    }

    // Attempt the transfer: make a new call to the Refer-To target
    let new_call_result = {
        let (account_config, local_addr, server_addr, transport, public_addr, stun_servers) = {
            let s = state.read().await;
            let account = match s.get_account(account_id) {
                Some(a) => a,
                None => return,
            };
            let transport = match &account.transport {
                Some(t) => t.clone(),
                None => return,
            };
            let local_addr = match account.local_addr {
                Some(a) => a,
                None => return,
            };
            let server_addr = match account.server_addr {
                Some(a) => a,
                None => return,
            };
            let public_addr = account.public_addr;
            (account.config.clone(), local_addr, server_addr, transport, public_addr, s.stun_servers.clone())
        };

        let new_call_id = builder::generate_call_id();
        let new_from_tag = builder::generate_tag();
        // Allocate RTP port and discover public IP via STUN for NAT traversal
        let (rtp_port, stun_public_ip) = match super::media::allocate_port_with_stun(&stun_servers).await {
            Ok((port, ip, _)) => (port, ip),
            Err(e) => {
                log::error!("Failed to allocate RTP port with STUN: {}", e);
                return;
            }
        };

        // Use STUN-discovered public IP for SDP, fallback to registration-discovered public IP
        let sdp_ip = super::media::sdp_address(
            local_addr.ip(),
            stun_public_ip.or_else(|| public_addr.map(|a| a.ip())),
            Some(server_addr.ip()),
        );
        let public_ip = Some(sdp_ip.to_string());
        let (invite, local_srtp_key) = builder::build_invite_with_public_ip(
            &account_config,
            &refer_to,
            local_addr,
            rtp_port,
            &new_call_id,
            1,
            &new_from_tag,
            None,
            public_ip.as_deref(),
        );

        let branch = extract_via_branch(&invite).unwrap_or_default();
        let local_uri = format!("sip:{}@{}", account_config.username, account_config.domain);
        let mut new_call = CallFSM::new_outbound(account_id, &refer_to, new_call_id, new_from_tag, rtp_port, branch, local_uri);
        new_call.local_srtp_key = local_srtp_key;
        new_call.sdp_public_ip = Some(sdp_ip);
        let new_id = new_call.id.clone();

        // The call is in the list before its INVITE goes out, so a fast
        // response finds it; the INVITE is resent until answered.
        if let Some(account) = state.write().await.get_account_mut(account_id) {
            account.calls.push(new_call);
        }
        let _ = event_tx.send(SipEvent::CallStateChanged(
            CallEvent::new(account_id, &new_id, "dialing", &refer_to, "outbound")
        ));
        let send_result = super::send_invite(state, event_tx, &new_id, &transport, server_addr, invite).await;
        if send_result.is_err() {
            if let Some(account) = state.write().await.get_account_mut(account_id) {
                account.calls.retain(|c| c.id != new_id);
            }
            let _ = event_tx.send(SipEvent::CallStateChanged(
                CallEvent::new(account_id, &new_id, "ended", &refer_to, "outbound")
            ));
        }

        send_result
    };

    // Send final NOTIFY based on result
    {
        let (sf_status, sf_reason, sf_sub) = if new_call_result.is_ok() {
            (200u16, "OK", "terminated;reason=noresource")
        } else {
            (503, "Service Unavailable", "terminated;reason=noresource")
        };

        let notify = {
            let mut s = state.write().await;
            let account = match s.get_account_mut(account_id) {
                Some(a) => a,
                None => return,
            };
            account.calls.iter_mut().find(|c| c.id == call_internal_id).map(|call| {
                call.cseq += 1;
                let cseq = call.cseq;
                let local_addr = transport.local_addr();
                build_notify_refer(
                    &notify_target,
                    &notify_local_uri,
                    &notify_remote_uri,
                    local_addr,
                    &notify_call_id,
                    cseq,
                    &notify_from_tag,
                    &notify_to_tag,
                    &transport_str,
                    sf_status,
                    sf_reason,
                    sf_sub,
                )
            })
        };
        if let Some(notify) = notify {
            let _ = super::send_non_invite(state, &transport, remote, notify).await;
        }
    }

    let _ = event_tx.send(SipEvent::TransferProgress(TransferEvent {
        account_id: account_id.to_string(),
        call_id: call_internal_id,
        status: if new_call_result.is_ok() { 200 } else { 503 },
        message: if new_call_result.is_ok() {
            "Transfer completed".into()
        } else {
            "Transfer failed".into()
        },
    }));
}

/// Handle NOTIFY with Event: refer (sipfrag body reporting transfer progress)
pub async fn handle_notify_refer(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    remote: SocketAddr,
    account_id: &str,
) {
    // Send 200 OK for the NOTIFY
    let ok = build_simple_response(text, 200, "OK");
    {
        let s = state.read().await;
        if let Some(account) = s.get_account(account_id) {
            if let Some(ref transport) = account.transport {
                if let Some(resp) = ok {
                    let _ = transport.send_to(resp.as_bytes(), remote).await;
                }
            }
        }
    }

    let call_id_header = match extract_header(text, "Call-ID") {
        Some(c) => c,
        None => return,
    };

    // Parse sipfrag body
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let sipfrag_status = parse_sipfrag_status(body).unwrap_or(0);

    let (call_id, call_remote_uri, call_direction) = {
        let s = state.read().await;
        let account = match s.get_account(account_id) {
            Some(a) => a,
            None => return,
        };
        match account.calls.iter().find(|c| c.call_id_header == call_id_header) {
            Some(call) => (call.id.clone(), call.remote_uri.clone(), call.direction_str().to_string()),
            None => return,
        }
    };

    let message = match sipfrag_status {
        100 => "Transfer: Trying".to_string(),
        180 => "Transfer: Ringing".to_string(),
        200 => "Transfer: Successful".to_string(),
        frag_s if frag_s >= 400 => format!("Transfer: Failed ({})", frag_s),
        _ => format!("Transfer: Status {}", sipfrag_status),
    };
    log::info!("REFER NOTIFY: {} for call {}", message, call_id);
    let _ = event_tx.send(SipEvent::TransferProgress(TransferEvent {
        account_id: account_id.to_string(),
        call_id: call_id.clone(),
        status: sipfrag_status,
        message,
    }));
    // The transfer succeeded, so our leg of the call is over.
    if sipfrag_status == 200 {
        let media = {
            let mut s = state.write().await;
            s.get_account_mut(account_id).and_then(|account| {
                account
                    .calls
                    .iter_mut()
                    .find(|c| c.call_id_header == call_id_header)
                    .and_then(|call| {
                        let media = call.take_media();
                        let _ = call.process(CallFSMEvent::LocalHangup);
                        media
                    })
            })
        };
        let recording_path = crate::sip::finish_call_media(&call_id, media).await;

        let mut ended = CallEvent::new(account_id, &call_id, "ended", &call_remote_uri, &call_direction);
        if let Some(path) = recording_path {
            ended = ended.with_recording_path(path);
        }
        let _ = event_tx.send(SipEvent::CallStateChanged(ended));
    }
}

/// Handle an incoming INVITE with Replaces header (RFC 3891)
pub async fn handle_invite_with_replaces(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    remote: SocketAddr,
    replaces_hdr: &str,
    account_id: &str,
) {
    let new_call_header = extract_header(text, "Call-ID").unwrap_or_default();
    let (outcome, replaced_id) = {
        let s = state.read().await;
        let Some(account) = s.get_account(account_id) else { return };
        let dialogs: Vec<replaces::Dialog> = account
            .calls
            .iter()
            .map(|c| {
                let (local_tag, remote_tag) = c.dialog_tags();
                replaces::Dialog {
                    call_id_header: c.call_id_header.clone(),
                    local_tag,
                    remote_tag,
                    initiated_here: !c.is_incoming() && c.direction_str() == "outbound",
                    confirmed: c.is_established(),
                    ended: c.is_ended(),
                }
            })
            .collect();
        let outcome = replaces::decide(replaces_hdr, &new_call_header, &dialogs);
        let id = match outcome {
            replaces::Outcome::Replace(i) => Some(account.calls[i].id.clone()),
            _ => None,
        };
        (outcome, id)
    };

    let refusal = match outcome {
        replaces::Outcome::BadRequest | replaces::Outcome::Loop => Some((400, "Bad Request")),
        replaces::Outcome::NoSuchDialog => Some((481, "Call/Transaction Does Not Exist")),
        replaces::Outcome::Busy => Some((486, "Busy Here")),
        replaces::Outcome::Replace(_) => None,
    };
    if let Some((code, reason)) = refusal {
        log::warn!("INVITE with Replaces refused: {code} ({replaces_hdr})");
        if let Some(resp) = build_simple_response(text, code, reason) {
            let s = state.read().await;
            if let Some(transport) = s.get_account(account_id).and_then(|a| a.transport.clone()) {
                let _ = transport.send_to(resp.as_bytes(), remote).await;
            }
        }
        return;
    }
    let Some(replaced_id) = replaced_id else { return };

    // Terminate the replaced call: a BYE for an answered dialog, a CANCEL for
    // one this app is still placing. Ending it only here left the far end
    // connected to nothing.
    let replaced = end_replaced_dialog(state, account_id, &replaced_id).await;
    if let Some((call_id, remote_uri, direction, media)) = replaced {
        let recording_path = crate::sip::finish_call_media(&call_id, media).await;
        let mut ended = CallEvent::new(account_id, &call_id, "ended", &remote_uri, &direction);
        if let Some(path) = recording_path {
            ended = ended.with_recording_path(path);
        }
        let _ = event_tx.send(SipEvent::CallStateChanged(ended));
    }

    // Now handle this INVITE as a new incoming call
    let from = extract_header(text, "From").unwrap_or_default();
    let new_call_id = extract_header(text, "Call-ID").unwrap_or_default();
    let from_tag = extract_from_tag(text).unwrap_or_default();
    let to_tag = builder::generate_tag();

    // Allocate RTP port dynamically - let OS pick an available port
    let rtp_port = match super::media::MediaSession::allocate_port().await {
        Ok(port) => port,
        Err(e) => {
            log::error!("Failed to allocate RTP port for INVITE with Replaces: {}", e);
            return;
        }
    };

    let remote_uri = from
        .find('<')
        .and_then(|start| {
            from[start + 1..]
                .find('>')
                .map(|end| from[start + 1..start + 1 + end].to_string())
        })
        .unwrap_or_else(|| format!("sip:unknown@{}", remote.ip()));

    let local_uri = {
        let s = state.read().await;
        match s.get_account(account_id) {
            Some(a) => format!("sip:{}@{}", a.config.username, a.config.domain),
            None => format!("sip:unknown@{}", remote.ip()),
        }
    };

    let call = CallFSM::new_inbound(InboundCallParams {
        account_id: account_id.to_string(),
        remote_uri: remote_uri.clone(),
        call_id: new_call_id,
        from_tag,
        to_tag: to_tag.clone(),
        local_rtp_port: rtp_port,
        raw_invite: text.to_string(),
        local_uri,
    });

    let new_internal_id = call.id.clone();
    let new_remote_uri = call.remote_uri.clone();

    // Auto-answer the replacing call: send 200 OK
    let (local_addr, username, transport) = {
        let s = state.read().await;
        let account = match s.get_account(account_id) {
            Some(a) => a,
            None => return,
        };
        let transport = match &account.transport {
            Some(t) => t.clone(),
            None => return,
        };
        let la = transport.local_addr();
        let user = account.config.username.clone();
        (la, user, transport)
    };

    if let Some(resp) = build_200_ok_invite_with_user(
        text,
        local_addr,
        rtp_port,
        &to_tag,
        Some(&username),
    ) {
        let _ = transport.send_to(resp.as_bytes(), remote).await;
    }

    // Start media
    let sdp = text.split("\r\n\r\n").nth(1).unwrap_or("");
    if let Some((ip, port)) = parse_sdp_connection(sdp) {
        if let Ok(remote_rtp) = format!("{}:{}", ip, port).parse::<SocketAddr>() {
            // The same pick as the answer's first payload type (the 200
            // above was built from the offer's order).
            let negotiated = builder::negotiate_audio(sdp, None);
            log::info!("Negotiated codec for INVITE with Replaces: {:?}", negotiated);

            let (input_dev, output_dev) = {
                let s = state.read().await;
                (s.preferred_input_device.clone(), s.preferred_output_device.clone())
            };
            let codec = negotiated.map_or(media::CodecType::Pcmu, |n| n.codec);
            match media::MediaSession::start_with_devices(rtp_port, remote_rtp, codec, input_dev, output_dev).await {
                Ok(session) => {
                    if let Some(audio) = negotiated {
                        media::apply_negotiated_payload_types(&session, audio, sdp);
                    }
                    let mut s = state.write().await;
                    let mut new_call = call;
                    let _ = new_call.process(CallFSMEvent::Answered {
                        to_tag: to_tag.clone(),
                        remote_rtp: Some(remote_rtp),
                        route_set: vec![],
                        session_expires: 0,
                    });
                    new_call.set_remote_rtp(remote_rtp);
                    new_call.set_media(session);
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.calls.push(new_call);
                    }
                }
                Err(e) => {
                    log::error!("Failed to start media for Replaces call: {}", e);
                    let mut s = state.write().await;
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.calls.push(call);
                    }
                }
            }
        } else {
            let mut s = state.write().await;
            if let Some(account) = s.get_account_mut(account_id) {
                account.calls.push(call);
            }
        }
    } else {
        let mut s = state.write().await;
        if let Some(account) = s.get_account_mut(account_id) {
            account.calls.push(call);
        }
    }

    let _ = event_tx.send(SipEvent::CallStateChanged(
        CallEvent::new(account_id, &new_internal_id, "connected", &new_remote_uri, "inbound")
    ));
}

/// Send what ends a dialog replaced by an INVITE with Replaces and mark it
/// ended. Returns what the caller needs to announce it.
async fn end_replaced_dialog(
    state: &Arc<RwLock<ManagerState>>,
    account_id: &str,
    call_id: &str,
) -> Option<(String, String, String, Option<media::MediaSession>)> {
    let (msg, transport, server_addr) = {
        let mut s = state.write().await;
        let account = s.get_account_mut(account_id)?;
        let local_addr = account.local_addr.or_else(|| account.transport.as_ref().map(|t| t.local_addr()));
        let (transport, server_addr) = (account.transport.clone(), account.server_addr);
        let transport_param = account.config.transport.param().to_string();
        let call = account.calls.iter_mut().find(|c| c.id == call_id)?;
        let msg = match (local_addr, &transport) {
            (Some(la), Some(_)) if call.is_established() && !call.bye_sent => {
                call.bye_sent = true;
                let cseq = call.next_cseq();
                let (ours, theirs) = call.dialog_tags();
                let target = call.remote_contact.clone().unwrap_or_else(|| call.remote_uri.clone());
                Some(builder::build_bye_with_routes(
                    &target,
                    la,
                    &call.call_id_header,
                    cseq,
                    &ours,
                    &theirs,
                    &transport_param,
                    call.route_set(),
                    &call.local_uri,
                    &call.remote_uri,
                ))
            }
            (Some(la), Some(_)) if call.is_dialing() || call.is_ringing() => Some(builder::build_cancel(
                &call.remote_uri,
                la,
                &call.call_id_header,
                call.invite_cseq,
                &call.dialog_tags().0,
                &transport_param,
                &call.last_invite_branch.clone().unwrap_or_default(),
                &call.local_uri,
                &call.remote_uri,
            )),
            _ => None,
        };
        (msg, transport, server_addr)
    };
    if let (Some(msg), Some(transport), Some(addr)) = (msg, transport, server_addr) {
        // The error text is not logged: it is derived from the account's
        // transport, and the log must never carry account data.
        if super::send_non_invite(state, &transport, addr, msg).await.is_err() {
            log::warn!("Could not end the replaced dialog {call_id}; the far end may still think it is up");
        }
    }
    let mut s = state.write().await;
    let call = s.get_account_mut(account_id)?.calls.iter_mut().find(|c| c.id == call_id)?;
    let media = call.take_media();
    let _ = call.process(CallFSMEvent::LocalHangup);
    Some((call.id.clone(), call.remote_uri.clone(), call.direction_str().to_string(), media))
}
