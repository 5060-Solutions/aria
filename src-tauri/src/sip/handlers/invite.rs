//! INVITE response handler.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::sip::auth::DigestAuth;
use crate::sip::builder::{
    self, build_ack, build_bye_with_routes, build_invite_with_public_ip, extract_all_headers,
    extract_header, extract_to_tag, extract_via_branch, parse_sdp_connection, AuthHeaderType,
};
use crate::sip::state::call::TransitionResult;
use crate::sip::state::CallFSMEvent;
use crate::sip::{codec, media, CallEvent, ManagerState, SipEvent};

/// The sequence number in a response's CSeq header.
fn response_cseq(text: &str) -> Option<u32> {
    extract_header(text, "CSeq")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Whether a final non-2xx response belongs to an INVITE we have since
/// superseded.
///
/// After an auth retry the live INVITE has CSeq 2. The server keeps
/// retransmitting the 407 for CSeq 1 until Timer H if it never matched our
/// ACK, and each retransmission used to be handled as a fresh challenge:
/// one more ended the call as an "auth loop", another sent a third INVITE on
/// the same dialog. A response for an earlier CSeq is a finished transaction
/// and needs nothing.
async fn is_stale_final_response(
    state: &Arc<RwLock<ManagerState>>,
    call_id_header: &str,
    text: &str,
) -> bool {
    let Some(cseq) = response_cseq(text) else { return false };
    let s = state.read().await;
    s.find_call_by_header(call_id_header)
        .is_some_and(|(_, call)| cseq < call.invite_cseq)
}

/// Send a BYE for a dialog the far end confirmed after we had already ended
/// the call on our side.
///
/// Our CANCEL and their 200 OK crossed. The ACK (already sent) confirms the
/// dialog, and the only thing that ends a confirmed dialog is a BYE (RFC 3261
/// §9.1, §15). Without it the callee stayed connected to silence until their
/// PBX gave up on its own.
async fn bye_crossed_dialog(
    state: &Arc<RwLock<ManagerState>>,
    call_id_header: &str,
    to_tag: &str,
    remote_contact: Option<&str>,
    route_set: &[String],
) {
    let bye = {
        let mut s = state.write().await;
        let Some((_, call)) = s.find_call_by_header_mut(call_id_header) else { return };
        let cseq = call.next_cseq();
        let remote_uri = call.remote_uri.clone();
        let local_uri = call.local_uri.clone();
        let from_tag = call.from_tag.clone();
        let account_id = call.account_id.clone();
        let Some(account) = s.get_account(&account_id) else { return };
        let (Some(transport), Some(server_addr)) = (account.transport.clone(), account.server_addr)
        else {
            return;
        };
        let local_addr = account.local_addr.unwrap_or_else(|| transport.local_addr());
        let msg = build_bye_with_routes(
            remote_contact.unwrap_or(&remote_uri),
            local_addr,
            call_id_header,
            cseq,
            &from_tag,
            to_tag,
            account.config.transport.param(),
            route_set,
            &local_uri,
            &remote_uri,
        );
        (msg, transport, server_addr)
    };
    let (msg, transport, server_addr) = bye;
    log::info!("200 OK crossed our CANCEL for {}; sending BYE", call_id_header);
    let _ = transport.send_to(msg.as_bytes(), server_addr).await;
}

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
        180 | 181 | 183 => {
            log::info!("Call ringing/progress ({})", status);

            // Check for SDP in 183 (early media)
            let sdp = text.split("\r\n\r\n").nth(1).unwrap_or("");
            let has_sdp = status == 183 && !sdp.trim().is_empty() && sdp.contains("m=audio");

            // Extract data needed for early media before taking write lock
            let early_media_data = if has_sdp {
                let rtp_target = parse_sdp_connection(sdp);
                let remote_srtp_key = rtp_engine::srtp::parse_sdp_crypto(sdp);
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
                        // The 183's SDP is the answer this session runs on,
                        // and a DTMF sent during early media (an IVR prompt)
                        // must already be on the negotiated payload type.
                        media::apply_negotiated_payload_types(&session, sdp);
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
                let transitioned = matches!(
                    call.process(CallFSMEvent::RemoteRinging),
                    TransitionResult::Ok
                );

                if let Some(session) = early_session {
                    call.set_early_media(session);
                }

                // Only when the call actually moved to ringing. A stray 180 for
                // a connected call must not tell the UI it is ringing again.
                if transitioned && !was_ringing {
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
                        call.is_established() || call.is_ended(),
                    )
                })
            };

            let (transport, server_addr, local_addr, transport_param, remote_uri, sip_call_id, cseq, from_tag, call_internal_id, local_rtp_port, account_id, local_srtp_key, already_answered) = match call_data {
                Some((Some(t), Some(sa), la, tp, ru, sci, cs, ft, cid, rp, aid, lsk, done)) => (t, sa, la, tp, ru, sci, cs, ft, cid, rp, aid, lsk, done),
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

            // Extract remote Contact header URI from 200 OK (RFC 3261 Section 13.2.2.4)
            let remote_contact_uri =
                extract_header(text, "Contact").map(|c| builder::contact_uri(&c));
            let ack_target_uri = remote_contact_uri.as_deref().unwrap_or(&remote_uri);

            let ack = build_ack(
                ack_target_uri,
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

            // Aria never sends a re-INVITE, so a 200 for a call already past
            // ringing is a retransmission: the peer did not see our ACK. It
            // needs the ACK above and nothing else. Running the rest again
            // started a second media session, and a second auto-recording
            // that split the call's audio across two files.
            //
            // A 200 for a call we already ended is different: the far end
            // answered as we cancelled. The ACK has just confirmed that
            // dialog, so it has to be closed with a BYE.
            if already_answered {
                let ended_unconfirmed = state
                    .read()
                    .await
                    .find_call_by_header(&call_id_header)
                    .is_some_and(|(_, call)| call.is_ended());
                if ended_unconfirmed {
                    bye_crossed_dialog(
                        state,
                        &call_id_header,
                        &to_tag,
                        remote_contact_uri.as_deref(),
                        &route_set,
                    )
                    .await;
                } else {
                    log::info!("Retransmitted 200 OK for {}, re-sent ACK", call_internal_id);
                }
                return;
            }

            let remote_rtp_addr = rtp_target.and_then(|(ip, port)| {
                format!("{}:{}", ip, port).parse::<SocketAddr>().ok()
            });

            // Built when auto-record starts. Sent after the call-state event
            // below, so the frontend has the call in its list before it is told
            // the call is being recorded.
            let mut recording_event: Option<crate::sip::RecordingEvent> = None;

            let has_media;
            {
                let mut s = state.write().await;

                // Take the early-media session out first, so the mutable borrow
                // of the call ends before start_auto_record reads the account.
                let early_media = if let Some((_, call)) =
                    s.find_call_by_header_mut(&call_id_header)
                {
                    call.remote_contact = remote_contact_uri.clone();
                    let early_media = call.take_early_media();
                    call.set_to_tag(to_tag.clone());
                    let result = call.process(CallFSMEvent::Answered {
                        to_tag: to_tag.clone(),
                        remote_rtp: remote_rtp_addr,
                        route_set: route_set.clone(),
                        session_expires,
                    });
                    if matches!(result, TransitionResult::AlreadyEnded) {
                        // Hung up between the check above and here: same
                        // crossed CANCEL/200 case, one lock scope later.
                        drop(s);
                        drop(early_media);
                        bye_crossed_dialog(
                            state,
                            &call_id_header,
                            &to_tag,
                            remote_contact_uri.as_deref(),
                            &route_set,
                        )
                        .await;
                        return;
                    }
                    early_media
                } else {
                    None
                };
                has_media = early_media.is_some();

                // If we had early media, set it on the now-connected call.
                // Auto-record has to start here too: this branch skips the media
                // setup below entirely, so a peer that sent a 183 with SDP —
                // ordinary PSTN ringback — used to connect on the early media
                // session and was silently never recorded.
                if let Some(session) = early_media {
                    if let Some((_, call)) = s.find_call_by_header_mut(&call_id_header) {
                        call.set_media(session);
                    }
                    recording_event = s
                        .find_call_by_header(&call_id_header)
                        .and_then(|(_, call)| call.media())
                        .and_then(|media| {
                            s.start_auto_record(&account_id, &call_internal_id, media)
                        });
                }
            }

            if has_media {
                log::info!("Early media session already active, skipping new media setup");
            } else if let Some(remote_rtp) = remote_rtp_addr {
                let negotiated_codec = codec::negotiate_codec(sdp);
                log::info!("Negotiated codec: {:?}", negotiated_codec);

                // Get preferred audio devices, and whether this account asked
                // for encryption. srtp_mode was only ever consulted when
                // *offering* a crypto line; nothing checked it against what came
                // back, so a peer could strip the line and get cleartext.
                let (input_dev, output_dev, srtp_preferred) = {
                    let s = state.read().await;
                    let preferred = s
                        .get_account(&account_id)
                        .is_some_and(|a| a.config.srtp_mode != crate::sip::account::SrtpMode::Disabled);
                    (
                        s.preferred_input_device.clone(),
                        s.preferred_output_device.clone(),
                        preferred,
                    )
                };

                // Start media session with SRTP if both keys are available
                let media_result = match (&local_srtp_key, &remote_srtp_key) {
                    (Some(local_key), Some(remote_key)) => {
                        // NOTE: never log the key material itself — these are the
                        // SDES-SRTP master keys and would let anyone with the log
                        // decrypt the call's media.
                        log::info!("Starting SRTP media session with separate TX/RX keys");
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
                                log::warn!("This call is NOT encrypted");
                                media::MediaSession::start_with_devices(
                                    local_rtp_port, remote_rtp, negotiated_codec,
                                    input_dev.clone(), output_dev.clone(),
                                ).await
                            }
                        }
                    }
                    _ => {
                        // We offered a crypto line and the answer had none, or
                        // vice versa. If this account asked for SRTP, that is a
                        // downgrade, and continuing in the clear is the one
                        // outcome the user cannot detect — the call screen would
                        // still be showing whatever the configuration says.
                        // SRTP is preferred, not required: a peer that answers
                        // without a crypto line still gets a working call. What
                        // must not happen is the UI claiming encryption anyway,
                        // so the outcome is recorded on the call and surfaced.
                        if srtp_preferred {
                            log::warn!(
                                "SRTP was offered but the peer answered without usable \
                                 key material; this call is NOT encrypted"
                            );
                        } else {
                            log::info!("Starting plain RTP media session (no SRTP keys)");
                        }
                        media::MediaSession::start_with_devices(
                            local_rtp_port, remote_rtp, negotiated_codec,
                            input_dev, output_dev,
                        ).await
                    }
                };

                // True only when both keys were present and both contexts
                // built — the same condition the SRTP arm above requires.
                let srtp_active = matches!(
                    (&local_srtp_key, &remote_srtp_key),
                    (Some(_), Some(_))
                ) && media_result.is_ok();

                match media_result {
                    Ok(session) => {
                        // Audio and DTMF go on the payload types the answer
                        // named, not a hardcoded 111 and 101.
                        media::apply_negotiated_payload_types(&session, sdp);
                        let mut s = state.write().await;
                        if let Some((_, call)) = s.find_call_mut(&call_internal_id) {
                            call.srtp_active = srtp_active;
                            call.set_remote_rtp(remote_rtp);
                            call.set_media(session);
                        }

                        // Only once the session is on the call. Media startup
                        // ran without the lock, and a call hung up in that
                        // window is gone; recording it would leave an orphaned
                        // WAV and announce a recording for a dead call.
                        recording_event = s
                            .find_call(&call_internal_id)
                            .and_then(|(_, call)| call.media())
                            .and_then(|media| {
                                s.start_auto_record(&account_id, &call_internal_id, media)
                            });
                    }
                    Err(e) => {
                        log::error!("Failed to start media: {}", e);
                    }
                }
            }

            // Hung up while media was starting: the call has already sent its
            // ended event, and a late "connected" would bring it back to life
            // in the UI.
            let srtp_active = {
                let s = state.read().await;
                match s.find_call(&call_internal_id) {
                    Some((_, call)) if call.is_established() => call.srtp_active,
                    _ => {
                        log::info!("Call {} ended while media was starting", call_internal_id);
                        return;
                    }
                }
            };

            let _ = event_tx.send(SipEvent::CallStateChanged(
                CallEvent::new(&account_id, call_internal_id, "connected", remote_uri, "outbound")
                    .with_sip_call_id(&call_id_header)
                    .with_srtp_active(srtp_active)
            ));

            if let Some(event) = recording_event {
                let _ = event_tx.send(SipEvent::RecordingStateChanged(event));
            }
        }
        486 | 600 | 603 => {
            if is_stale_final_response(state, &call_id_header, text).await {
                return;
            }
            log::info!("Call rejected ({})", status);
            send_non_2xx_ack(state, &call_id_header, text).await;
            let mut s = state.write().await;
            if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                let account_id = account_id.to_string();
                let call_id = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                // A rejection for an already-ended call changes nothing; the
                // UI has had its ended event.
                if matches!(call.process(CallFSMEvent::Reject { status }), TransitionResult::Ok) {
                    let _ = event_tx.send(SipEvent::CallStateChanged(
                        CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                    ));
                }
            }
        }
        401 | 407 => {
            if is_stale_final_response(state, &call_id_header, text).await {
                log::debug!("Ignoring retransmitted {} for a superseded INVITE", status);
                return;
            }

            // A non-2xx final response is ACKed with the INVITE's branch so
            // the server transaction can match it (RFC 3261 §17.1.1.3). This
            // arm used to build its own ACK with a fresh branch; the PBX never
            // matched it and retransmitted the 407 for 32 s, and each
            // retransmission came back through here.
            send_non_2xx_ack(state, &call_id_header, text).await;

            // Check if auth was already attempted
            {
                let mut s = state.write().await;
                if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                    let account_id = account_id.to_string();
                    if call.auth_attempted() {
                        log::error!("INVITE auth already attempted, giving up (loop guard)");
                        let call_id = call.id.clone();
                        let remote_uri = call.remote_uri.clone();
                        let result = call.process(CallFSMEvent::Fail {
                            reason: "Auth loop prevented".to_string(),
                        });
                        if matches!(result, TransitionResult::Ok) {
                            let _ = event_tx.send(SipEvent::CallStateChanged(
                                CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                            ));
                        }
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
            // The same decision as the INVITE being retried, so the offer
            // does not change address between the two attempts.
            let public_ip = Some(
                crate::sip::media::sdp_address(
                    local_addr.ip(),
                    public_addr.map(|a| a.ip()),
                    Some(server_addr.ip()),
                )
                .to_string(),
            );
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
                    call.invite_cseq = new_cseq;
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
            if is_stale_final_response(state, &call_id_header, text).await {
                return;
            }
            log::warn!("Call failed ({})", status);
            send_non_2xx_ack(state, &call_id_header, text).await;
            let mut s = state.write().await;
            if let Some((account_id, call)) = s.find_call_by_header_mut(&call_id_header) {
                let account_id = account_id.to_string();
                let call_id = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                let result = call.process(CallFSMEvent::Fail {
                    reason: format!("Call failed: {}", status),
                });
                if matches!(result, TransitionResult::Ok) {
                    let _ = event_tx.send(SipEvent::CallStateChanged(
                        CallEvent::new(&account_id, &call_id, "ended", &remote_uri, "outbound")
                    ));
                }
            }
        }
        _ => {}
    }
}

/// Send a hop-by-hop ACK request for non-2xx final responses (RFC 3261 Section 17.1.1.3).
async fn send_non_2xx_ack(
    state: &Arc<RwLock<ManagerState>>,
    call_id_header: &str,
    text: &str,
) {
    let ack_info = {
        let s = state.read().await;
        s.find_call_by_header(call_id_header).and_then(|(account, call)| {
            let transport = account.transport.clone()?;
            let server_addr = account.server_addr?;
            let local_addr = account.local_addr.unwrap_or_else(|| transport.local_addr());
            let transport_param = account.config.transport.param().to_string();
            let via_branch = call.last_invite_branch.clone().unwrap_or_else(builder::generate_branch);
            Some((
                transport,
                server_addr,
                local_addr,
                transport_param,
                call.remote_uri.clone(),
                call.call_id_header.clone(),
                call.cseq,
                call.from_tag.clone(),
                call.local_uri.clone(),
                via_branch,
            ))
        })
    };

    if let Some((transport, server_addr, local_addr, transport_param, remote_uri, sip_call_id, cseq, from_tag, local_uri, via_branch)) = ack_info {
        let to_tag = extract_to_tag(text).unwrap_or_default();
        let ack = build_ack(
            &remote_uri,
            local_addr,
            &sip_call_id,
            cseq,
            &from_tag,
            &to_tag,
            &transport_param,
            &via_branch,
            &local_uri,
            &remote_uri,
        );
        let _ = transport.send_to(ack.as_bytes(), server_addr).await;
    }
}
