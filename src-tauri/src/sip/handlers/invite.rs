//! INVITE response handler.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::sip::auth::DigestAuth;
use crate::sip::builder::{
    self, build_ack, build_invite_with_public_ip, extract_all_headers, extract_header, extract_to_tag,
    extract_via_branch, parse_sdp_connection, AuthHeaderType,
};
use crate::sip::state::CallFSMEvent;
use crate::sip::{codec, media, CallEvent, ManagerState, SipEvent};

/// Handle INVITE response (100 Trying, 180 Ringing, 200 OK, auth challenges, errors).
pub async fn handle_invite_response(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    status: u16,
) {
    let call_id_header = match extract_header(text, "Call-ID") {
        Some(c) => c,
        None => return,
    };

    match status {
        100 => {
            log::info!("Call trying (100)");
        }
        180 | 183 => {
            log::info!("Call ringing ({})", status);

            // Check for SDP in 183 (early media)
            let sdp = text.split("\r\n\r\n").nth(1).unwrap_or("");
            let has_sdp = status == 183 && !sdp.trim().is_empty() && sdp.contains("m=audio");

            // Extract data needed for early media before taking write lock
            let early_media_data = if has_sdp {
                let rtp_target = parse_sdp_connection(sdp);
                let remote_srtp_key = rtp_engine::srtp::parse_sdp_crypto(sdp).map(String::from);
                let negotiated_codec = codec::negotiate_codec(sdp);

                let s = state.read().await;
                s.find_call_by_header(&call_id_header).and_then(|(account, call)| {
                    if call.has_early_media() {
                        return None; // Already have early media
                    }
                    let remote_rtp_addr = rtp_target.and_then(|(ip, port)| {
                        format!("{}:{}", ip, port).parse::<SocketAddr>().ok()
                    })?;
                    let input_dev = s.preferred_input_device.clone();
                    let output_dev = s.preferred_output_device.clone();
                    Some((
                        call.local_rtp_port,
                        remote_rtp_addr,
                        negotiated_codec,
                        call.local_srtp_key.clone(),
                        remote_srtp_key,
                        call.id.clone(),
                        account.config.id.clone(),
                        input_dev,
                        output_dev,
                    ))
                })
            } else {
                None
            };

            // Start early media session if we have the data
            let early_session = if let Some((local_rtp_port, remote_rtp, negotiated_codec, local_srtp_key, remote_srtp_key, _call_id, _account_id, input_dev, output_dev)) = early_media_data {
                log::info!("Starting early media session for 183 response");
                let result = match (&local_srtp_key, &remote_srtp_key) {
                    (Some(local_key), Some(remote_key)) => {
                        match (
                            rtp_engine::srtp::SrtpContext::from_base64(local_key),
                            rtp_engine::srtp::SrtpContext::from_base64(remote_key),
                        ) {
                            (Ok(tx_ctx), Ok(rx_ctx)) => {
                                media::MediaSession::start_with_srtp_keys_and_devices(
                                    local_rtp_port, remote_rtp, negotiated_codec, tx_ctx, rx_ctx,
                                    input_dev, output_dev,
                                ).await
                            }
                            (Err(e), _) | (_, Err(e)) => {
                                log::error!("Failed to create SRTP context for early media: {:?}", e);
                                media::MediaSession::start_with_devices(
                                    local_rtp_port, remote_rtp, negotiated_codec,
                                    input_dev, output_dev,
                                ).await
                            }
                        }
                    }
                    _ => {
                        media::MediaSession::start_with_devices(
                            local_rtp_port, remote_rtp, negotiated_codec,
                            input_dev, output_dev,
                        ).await
                    }
                };
                match result {
                    Ok(session) => {
                        log::info!("Early media session started successfully");
                        Some(session)
                    }
                    Err(e) => {
                        log::error!("Failed to start early media: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            let mut s = state.write().await;
            if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                let account_id = account_id.to_string();
                let call_id = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                let was_ringing = call.is_ringing();
                let _ = call.process(CallFSMEvent::RemoteRinging);

                if let Some(session) = early_session {
                    call.set_early_media(session);
                }

                // Only emit ringing event if we weren't already ringing
                if !was_ringing {
                    let _ = event_tx.send(SipEvent::CallStateChanged(
                        CallEvent::new(&account_id, &call_id, "ringing", &remote_uri, "outbound")
                            .with_sip_call_id(&call_id_header)
                    ));
                }
            }
        }
        200 => {
            log::info!("Call answered (200 OK)");
            let to_tag = extract_to_tag(text).unwrap_or_default();
            let sdp = text.split("\r\n\r\n").nth(1).unwrap_or("");
            let rtp_target = parse_sdp_connection(sdp);
            
            // Parse remote SRTP key from SDP answer
            let remote_srtp_key = rtp_engine::srtp::parse_sdp_crypto(sdp);
            if remote_srtp_key.is_some() {
                log::info!("Remote party provided SRTP key in SDP answer");
            }

            let mut route_set = extract_all_headers(text, "Record-Route");
            route_set.reverse();

            let session_expires = extract_header(text, "Session-Expires")
                .and_then(|v| {
                    v.split(';')
                        .next()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                })
                .unwrap_or(1800);

            // Extract all needed data in one read lock scope
            let call_data = {
                let s = state.read().await;
                s.find_call_by_header(&call_id_header).map(|(account, call)| {
                    let transport = account.transport.clone();
                    let server_addr = account.server_addr;
                    let local_addr = account.local_addr.unwrap_or_else(|| {
                        account.transport.as_ref().map(|t| t.local_addr()).unwrap_or_else(|| "0.0.0.0:0".parse().unwrap())
                    });
                    let transport_param = account.config.transport.param().to_string();
                    (
                        transport,
                        server_addr,
                        local_addr,
                        transport_param,
                        call.remote_uri.clone(),
                        call.call_id_header.clone(),
                        call.cseq,
                        call.from_tag.clone(),
                        call.id.clone(),
                        call.local_rtp_port,
                        call.account_id.clone(),
                        call.local_srtp_key.clone(),
                    )
                })
            };

            let (transport, server_addr, local_addr, transport_param, remote_uri, sip_call_id, cseq, from_tag, call_internal_id, local_rtp_port, account_id, local_srtp_key) = match call_data {
                Some((Some(t), Some(sa), la, tp, ru, sci, cs, ft, cid, rp, aid, lsk)) => (t, sa, la, tp, ru, sci, cs, ft, cid, rp, aid, lsk),
                _ => return,
            };

            // Determine From/To URIs for in-dialog ACK based on call direction.
            // For outbound calls: From = our local URI, To = remote URI
            // For inbound calls: From = remote URI, To = our local URI
            // But within a dialog, the From/To match the original INVITE direction.
            let (ack_from_uri, ack_to_uri) = {
                let s = state.read().await;
                if let Some((_, call)) = s.find_call_by_header(&call_id_header) {
                    (call.local_uri.clone(), call.remote_uri.clone())
                } else {
                    (format!("sip:unknown@{}", local_addr.ip()), remote_uri.clone())
                }
            };

            let ack = build_ack(
                &remote_uri,
                local_addr,
                &sip_call_id,
                cseq,
                &from_tag,
                &to_tag,
                &transport_param,
                &builder::generate_branch(),
                &ack_from_uri,
                &ack_to_uri,
            );

            let _ = transport.send_to(ack.as_bytes(), server_addr).await;

            let remote_rtp_addr = rtp_target.and_then(|(ip, port)| {
                format!("{}:{}", ip, port).parse::<SocketAddr>().ok()
            });

            let has_media;
            {
                let mut s = state.write().await;
                if let Some((_, call)) = s.find_call_by_header_mut(&call_id_header) {
                    let early_media = call.take_early_media();
                    has_media = early_media.is_some();
                    call.set_to_tag(to_tag.clone());
                    let _ = call.process(CallFSMEvent::Answered {
                        to_tag: to_tag.clone(),
                        remote_rtp: remote_rtp_addr,
                        route_set,
                        session_expires,
                    });
                    // If we had early media, set it on the now-connected call
                    if let Some(session) = early_media {
                        call.set_media(session);
                    }
                } else {
                    has_media = false;
                }
            }

            if has_media {
                log::info!("Early media session already active, skipping new media setup");
            } else if let Some(remote_rtp) = remote_rtp_addr {
                let negotiated_codec = codec::negotiate_codec(sdp);
                log::info!("Negotiated codec: {:?}", negotiated_codec);

                // Get preferred audio devices
                let (input_dev, output_dev) = {
                    let s = state.read().await;
                    (s.preferred_input_device.clone(), s.preferred_output_device.clone())
                };

                // Start media session with SRTP if both keys are available
                let media_result = match (&local_srtp_key, &remote_srtp_key) {
                    (Some(local_key), Some(remote_key)) => {
                        log::info!("Starting SRTP media session with separate TX/RX keys");
                        log::info!("  TX key (our key, for encrypt outgoing): {}", local_key);
                        log::info!("  RX key (their key, for decrypt incoming): {}", remote_key);
                        if local_key == remote_key {
                            log::warn!("TX and RX keys are IDENTICAL - remote echoed our key (symmetric mode)");
                        }
                        match (
                            rtp_engine::srtp::SrtpContext::from_base64(local_key),
                            rtp_engine::srtp::SrtpContext::from_base64(remote_key),
                        ) {
                            (Ok(tx_ctx), Ok(rx_ctx)) => {
                                media::MediaSession::start_with_srtp_keys_and_devices(
                                    local_rtp_port, remote_rtp, negotiated_codec, tx_ctx, rx_ctx,
                                    input_dev.clone(), output_dev.clone(),
                                ).await
                            }
                            (Err(e), _) | (_, Err(e)) => {
                                log::error!("Failed to create SRTP context: {:?}", e);
                                media::MediaSession::start_with_devices(
                                    local_rtp_port, remote_rtp, negotiated_codec,
                                    input_dev.clone(), output_dev.clone(),
                                ).await
                            }
                        }
                    }
                    _ => {
                        log::info!("Starting plain RTP media session (no SRTP keys)");
                        media::MediaSession::start_with_devices(
                            local_rtp_port, remote_rtp, negotiated_codec,
                            input_dev, output_dev,
                        ).await
                    }
                };

                match media_result {
                    Ok(session) => {
                        let mut s = state.write().await;
                        // Check if auto_record is enabled for this account
                        let auto_record = s.get_account(&account_id)
                            .map(|a| a.config.auto_record)
                            .unwrap_or(false);
                        
                        if let Some((_, call)) = s.find_call_mut(&call_internal_id) {
                            call.set_remote_rtp(remote_rtp);
                            
                            // Start auto-recording if enabled
                            if auto_record {
                                if let Some(data_dir) = dirs::data_dir() {
                                    let recordings_dir = data_dir.join("com.5060.aria").join("recordings");
                                    let output_path = rtp_engine::generate_recording_filename(&call_internal_id, &recordings_dir);
                                    if let Err(e) = session.start_recording(output_path) {
                                        log::warn!("Failed to start auto-recording: {}", e);
                                    } else {
                                        log::info!("Auto-recording started for call {}", call_internal_id);
                                    }
                                }
                            }
                            
                            call.set_media(session);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to start media: {}", e);
                    }
                }
            }

            let _ = event_tx.send(SipEvent::CallStateChanged(
                CallEvent::new(&account_id, call_internal_id, "connected", remote_uri, "outbound")
                    .with_sip_call_id(&call_id_header)
            ));
        }
        486 | 600 | 603 => {
            log::info!("Call rejected ({})", status);
            let mut s = state.write().await;
            if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                let account_id = account_id.to_string();
                let call_id = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                let _ = call.process(CallFSMEvent::Reject { status });
                let _ = event_tx.send(SipEvent::CallStateChanged(
                    CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                ));
            }
        }
        401 | 407 => {
            // Check if auth was already attempted
            {
                let mut s = state.write().await;
                if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                    let account_id = account_id.to_string();
                    if call.auth_attempted() {
                        log::error!("INVITE auth already attempted, giving up (loop guard)");
                        let call_id = call.id.clone();
                        let remote_uri = call.remote_uri.clone();
                        let _ = call.process(CallFSMEvent::Fail {
                            reason: "Auth loop prevented".to_string(),
                        });
                        let _ = event_tx.send(SipEvent::CallStateChanged(
                            CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                        ));
                        return;
                    }
                    call.set_auth_attempted();
                } else {
                    return;
                }
            }

            log::info!("INVITE challenged ({}), sending auth", status);
            let proxy_auth = extract_header(
                text,
                if status == 407 {
                    "Proxy-Authenticate"
                } else {
                    "WWW-Authenticate"
                },
            );

            let proxy_auth = match proxy_auth {
                Some(h) => h,
                None => {
                    log::error!("No auth challenge header for INVITE");
                    return;
                }
            };

            // Extract ACK info
            let ack_data = {
                let s = state.read().await;
                s.find_call_by_header(&call_id_header).map(|(account, call)| {
                    let transport = account.transport.clone();
                    let server_addr = account.server_addr;
                    let local_addr = account.local_addr.unwrap_or_else(|| {
                        account.transport.as_ref().map(|t| t.local_addr()).unwrap_or_else(|| "0.0.0.0:0".parse().unwrap())
                    });
                    let transport_param = account.config.transport.param().to_string();
                    (
                        transport,
                        server_addr,
                        local_addr,
                        transport_param,
                        call.remote_uri.clone(),
                        call.call_id_header.clone(),
                        call.cseq,
                        call.from_tag.clone(),
                    )
                })
            };

            let (transport, server_addr, local_addr, transport_param, remote_uri, sip_call_id, cseq, from_tag) = match ack_data {
                Some((Some(t), Some(sa), la, tp, ru, sci, cs, ft)) => (t, sa, la, tp, ru, sci, cs, ft),
                _ => return,
            };

            let (ack_from_uri, ack_to_uri) = {
                let s = state.read().await;
                if let Some((_, call)) = s.find_call_by_header(&call_id_header) {
                    (call.local_uri.clone(), call.remote_uri.clone())
                } else {
                    (format!("sip:unknown@{}", local_addr.ip()), remote_uri.clone())
                }
            };

            let ack = build_ack(
                &remote_uri,
                local_addr,
                &sip_call_id,
                cseq,
                &from_tag,
                &extract_to_tag(text).unwrap_or_default(),
                &transport_param,
                &builder::generate_branch(),
                &ack_from_uri,
                &ack_to_uri,
            );
            let _ = transport.send_to(ack.as_bytes(), server_addr).await;

            // Get account and call info for auth retry
            let retry_data = {
                let s = state.read().await;
                s.find_call_by_header(&call_id_header).map(|(account, call)| {
                    (
                        account.config.id.clone(),
                        account.config.clone(),
                        account.local_addr,
                        account.server_addr,
                        account.transport.clone(),
                        call.remote_uri.clone(),
                        call.call_id_header.clone(),
                        call.from_tag.clone(),
                        call.local_rtp_port,
                        account.public_addr,
                        call.local_srtp_key.clone(),
                    )
                })
            };

            let (_account_id, account_config, local_addr, server_addr, transport, remote_uri, sip_call_id, from_tag, rtp_port, public_addr, _existing_srtp_key) = match retry_data {
                Some((aid, ac, Some(la), Some(sa), Some(t), ru, sci, ft, rp, pa, sk)) => (aid, ac, la, sa, t, ru, sci, ft, rp, pa, sk),
                _ => return,
            };

            let auth = DigestAuth::from_challenge_with_realm(
                &proxy_auth,
                account_config.effective_auth_username(),
                &account_config.password,
                &remote_uri,
                "INVITE",
                account_config.auth_realm.as_deref(),
            );

            let auth_header = match auth {
                Some(a) => a.to_header(),
                None => {
                    log::error!("Failed to parse INVITE auth challenge");
                    return;
                }
            };

            let new_cseq = {
                let mut s = state.write().await;
                if let Some((_, call)) = s.find_call_by_header_mut(&sip_call_id) {
                    call.cseq += 1;
                    call.cseq
                } else {
                    return;
                }
            };

            let auth_type = if status == 407 {
                AuthHeaderType::ProxyAuthorization
            } else {
                AuthHeaderType::Authorization
            };

            // Use public IP in SDP for NAT traversal if discovered during registration
            let public_ip = public_addr.map(|a| a.ip().to_string());
            let (invite, new_srtp_key) = build_invite_with_public_ip(
                &account_config,
                &remote_uri,
                local_addr,
                rtp_port,
                &sip_call_id,
                new_cseq,
                &from_tag,
                Some((&auth_header, auth_type)),
                public_ip.as_deref(),
            );

            let branch = extract_via_branch(&invite);
            {
                let mut s = state.write().await;
                if let Some((_, call)) = s.find_call_by_header_mut(&sip_call_id) {
                    call.last_invite_branch = branch;
                    // Update SRTP key (new key for auth retry SDP)
                    if new_srtp_key.is_some() {
                        call.local_srtp_key = new_srtp_key;
                    }
                }
            }

            if let Err(e) = transport.send_to(invite.as_bytes(), server_addr).await {
                log::error!("Failed to send authenticated INVITE: {}", e);
            }
        }
        _ if status >= 400 => {
            log::warn!("Call failed ({})", status);
            let mut s = state.write().await;
            if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                let account_id = account_id.to_string();
                let call_id = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                let _ = call.process(CallFSMEvent::Fail {
                    reason: format!("Call failed: {}", status),
                });
                let _ = event_tx.send(SipEvent::CallStateChanged(
                    CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                ));
            }
        }
        _ => {}
    }
}
