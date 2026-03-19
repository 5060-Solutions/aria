use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::builder::{
    self, build_200_ok_invite_with_user, build_202_accepted, build_notify_refer,
    build_simple_response, extract_from_tag, extract_header, extract_via_branch,
    parse_replaces_header, parse_sdp_connection, parse_sipfrag_status,
};
use super::state::{CallFSM, CallFSMEvent};
use super::codec;
use super::media;
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
    let (notify_target, notify_call_id, notify_from_tag, notify_to_tag, transport_str, transport) = {
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
        (
            call.remote_uri.clone(),
            call.call_id_header.clone(),
            call.from_tag.clone(),
            call.to_tag.clone().unwrap_or_default(),
            tp,
            transport,
        )
    };

    // Send initial NOTIFY (100 Trying)
    {
        let mut s = state.write().await;
        let account = match s.get_account_mut(account_id) {
            Some(a) => a,
            None => return,
        };
        if let Some(call) = account.calls.iter_mut().find(|c| c.id == call_internal_id) {
            call.cseq += 1;
            let cseq = call.cseq;
            let local_addr = transport.local_addr();
            let notify = build_notify_refer(
                &notify_target,
                local_addr,
                &notify_call_id,
                cseq,
                &notify_from_tag,
                &notify_to_tag,
                &transport_str,
                100,
                "Trying",
                "active;expires=60",
            );
            let _ = transport.send_to(notify.as_bytes(), remote).await;
        }
    }

    // Attempt the transfer: make a new call to the Refer-To target
    let new_call_result = {
        let (account_config, local_addr, server_addr, transport, public_addr) = {
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
            (account.config.clone(), local_addr, server_addr, transport, public_addr)
        };

        let new_call_id = builder::generate_call_id();
        let new_from_tag = builder::generate_tag();
        // Allocate RTP port and discover public IP via STUN for NAT traversal
        let (rtp_port, stun_public_ip) = match super::media::allocate_port_with_stun().await {
            Ok((port, ip, _)) => (port, Some(ip)),
            Err(e) => {
                log::error!("Failed to allocate RTP port with STUN: {}", e);
                return;
            }
        };

        // Use STUN-discovered public IP for SDP, fallback to registration-discovered public IP
        let public_ip = stun_public_ip.map(|ip| ip.to_string())
            .or_else(|| public_addr.map(|a| a.ip().to_string()));
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
        let new_id = new_call.id.clone();

        let send_result = transport.send_to(invite.as_bytes(), server_addr).await;

        if send_result.is_ok() {
            let mut s = state.write().await;
            if let Some(account) = s.get_account_mut(account_id) {
                account.calls.push(new_call);
            }
            let _ = event_tx.send(SipEvent::CallStateChanged(
                CallEvent::new(account_id, &new_id, "dialing", &refer_to, "outbound")
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

        let mut s = state.write().await;
        let account = match s.get_account_mut(account_id) {
            Some(a) => a,
            None => return,
        };
        if let Some(call) = account.calls.iter_mut().find(|c| c.id == call_internal_id) {
            call.cseq += 1;
            let cseq = call.cseq;
            let local_addr = transport.local_addr();
            let notify = build_notify_refer(
                &notify_target,
                local_addr,
                &notify_call_id,
                cseq,
                &notify_from_tag,
                &notify_to_tag,
                &transport_str,
                sf_status,
                sf_reason,
                sf_sub,
            );
            let _ = transport.send_to(notify.as_bytes(), remote).await;
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
    if sipfrag_status == 200 {
        let _ = event_tx.send(SipEvent::CallStateChanged(
            CallEvent::new(account_id, &call_id, "ended", &call_remote_uri, &call_direction)
        ));
    }

    // If transfer succeeded, clean up the call
    if sipfrag_status == 200 {
        let mut s = state.write().await;
        if let Some(account) = s.get_account_mut(account_id) {
            if let Some(call) = account.calls.iter_mut().find(|c| c.call_id_header == call_id_header) {
                if let Some(media) = call.media() {
                    media.stop();
                }
                let _ = call.process(CallFSMEvent::LocalHangup);
            }
        }
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
    let (replaces_call_id, replaces_to_tag, replaces_from_tag) =
        match parse_replaces_header(replaces_hdr) {
            Some(v) => v,
            None => {
                log::warn!("Invalid Replaces header: {}", replaces_hdr);
                if let Some(resp) = build_simple_response(text, 400, "Bad Request") {
                    let s = state.read().await;
                    if let Some(account) = s.get_account(account_id) {
                        if let Some(ref transport) = account.transport {
                            let _ = transport.send_to(resp.as_bytes(), remote).await;
                        }
                    }
                }
                return;
            }
        };

    // Find the call to replace
    let replaced_call_id = {
        let s = state.read().await;
        let account = match s.get_account(account_id) {
            Some(a) => a,
            None => return,
        };
        account.calls
            .iter()
            .find(|c| {
                c.call_id_header == replaces_call_id
                    && c.to_tag.as_deref() == Some(&replaces_to_tag)
                    && c.from_tag == replaces_from_tag
            })
            .or_else(|| {
                // Try swapped tags (the perspective matters)
                account.calls.iter().find(|c| {
                    c.call_id_header == replaces_call_id
                        && c.from_tag == replaces_to_tag
                        && c.to_tag.as_deref() == Some(&replaces_from_tag)
                })
            })
            .map(|c| c.id.clone())
    };

    let replaced_id = match replaced_call_id {
        Some(id) => id,
        None => {
            log::warn!(
                "No matching call for Replaces: {} (to-tag={}, from-tag={})",
                replaces_call_id,
                replaces_to_tag,
                replaces_from_tag
            );
            if let Some(resp) =
                build_simple_response(text, 481, "Call/Transaction Does Not Exist")
            {
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    if let Some(ref transport) = account.transport {
                        let _ = transport.send_to(resp.as_bytes(), remote).await;
                    }
                }
            }
            return;
        }
    };

    // Terminate the replaced call
    {
        let mut s = state.write().await;
        if let Some(account) = s.get_account_mut(account_id) {
            if let Some(call) = account.calls.iter_mut().find(|c| c.id == replaced_id) {
                if let Some(media) = call.media() {
                    media.stop();
                }
                let call_id = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                let direction = call.direction_str().to_string();
                let _ = call.process(CallFSMEvent::LocalHangup);

                let _ = event_tx.send(SipEvent::CallStateChanged(
                    CallEvent::new(account_id, &call_id, "ended", &remote_uri, &direction)
                ));
            }
        }
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

    let call = CallFSM::new_inbound(
        account_id,
        &remote_uri,
        new_call_id,
        from_tag,
        to_tag.clone(),
        rtp_port,
        text.to_string(),
        local_uri,
    );

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
            let negotiated_codec = codec::negotiate_codec(sdp);
            log::info!(
                "Negotiated codec for INVITE with Replaces: {:?}",
                negotiated_codec
            );

            let (input_dev, output_dev) = {
                let s = state.read().await;
                (s.preferred_input_device.clone(), s.preferred_output_device.clone())
            };
            match media::MediaSession::start_with_devices(rtp_port, remote_rtp, negotiated_codec, input_dev, output_dev).await {
                Ok(session) => {
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
