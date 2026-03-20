//! Incoming SIP request handlers.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::sip::builder::{self, build_200_ok_subscribe, extract_header, extract_method};
use crate::sip::state::{CallFSM, CallFSMEvent, InboundCallParams};
use crate::sip::transfer;
use crate::sip::{CallEvent, ManagerState, SipEvent, VoicemailStatusEvent};

use super::presence::handle_notify_presence;

/// Handle incoming SIP request (INVITE, BYE, OPTIONS, NOTIFY, etc.).
pub async fn handle_incoming_request(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    remote: SocketAddr,
    account_id: &str,
) {
    let method = match extract_method(text) {
        Some(m) => m,
        None => return,
    };

    match method.as_str() {
        "INVITE" => {
            if let Some(replaces_hdr) = extract_header(text, "Replaces") {
                log::info!("Incoming INVITE with Replaces from {}", remote);
                transfer::handle_invite_with_replaces(state, event_tx, text, remote, &replaces_hdr, account_id)
                    .await;
            } else {
                log::info!("Incoming INVITE from {}", remote);
                let call_id = extract_header(text, "Call-ID").unwrap_or_default();
                let from = extract_header(text, "From").unwrap_or_default();
                let from_tag = from
                    .find("tag=")
                    .map(|p| {
                        let start = p + 4;
                        let end = from[start..]
                            .find([';', '>', ' '])
                            .map(|e| start + e)
                            .unwrap_or(from.len());
                        from[start..end].to_string()
                    })
                    .unwrap_or_default();

                let to_tag = builder::generate_tag();

                // Allocate RTP port dynamically - let OS pick an available port
                let rtp_port = match crate::sip::media::MediaSession::allocate_port().await {
                    Ok(port) => port,
                    Err(e) => {
                        log::error!("Failed to allocate RTP port for incoming call: {}", e);
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

                let sip_call_id = call_id.clone();
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
                    call_id,
                    from_tag,
                    to_tag: to_tag.clone(),
                    local_rtp_port: rtp_port,
                    raw_invite: text.to_string(),
                    local_uri,
                });

                let call_id_for_event = call.id.clone();
                let remote_uri_for_event = call.remote_uri.clone();

                // Send 180 Ringing
                {
                    let s = state.read().await;
                    if let Some(account) = s.get_account(account_id) {
                        if let Some(ref transport) = account.transport {
                            let ringing = build_ringing_response(text, &to_tag);
                            if let Some(resp) = ringing {
                                let _ = transport.send_to(resp.as_bytes(), remote).await;
                            }
                        }
                    }
                }

                // Add call to account
                {
                    let mut s = state.write().await;
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.calls.push(call);
                    }
                }

                let event = CallEvent::new(
                    account_id,
                    &call_id_for_event,
                    "incoming",
                    &remote_uri_for_event,
                    "inbound",
                ).with_sip_call_id(&sip_call_id);
                let _ = event_tx.send(SipEvent::CallStateChanged(event));
            }
        }
        "BYE" => {
            log::info!("Received BYE from {}", remote);
            let call_id = extract_header(text, "Call-ID").unwrap_or_default();

            let ok_response = build_simple_response(text, 200, "OK");

            // Send 200 OK response
            {
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    if let Some(ref transport) = account.transport {
                        if let Some(resp) = ok_response {
                            let _ = transport.send_to(resp.as_bytes(), remote).await;
                        }
                    }
                }
            }

            // End the call
            let mut s = state.write().await;
            if let Some((found_account_id, call)) = s.find_call_by_header_mut(&call_id) {
                let found_account_id = found_account_id.to_string();
                if let Some(media) = call.media() {
                    // Stop recording if active (saves the WAV file)
                    if media.is_recording() {
                        if let Ok(Some(path)) = media.stop_recording() {
                            log::info!("Call recording saved: {}", path.display());
                        }
                    }
                    media.stop();
                }
                let call_id_for_event = call.id.clone();
                let remote_uri = call.remote_uri.clone();
                let _ = call.process(CallFSMEvent::RemoteHangup);

                let _ = event_tx.send(SipEvent::CallStateChanged(
                    CallEvent::new(&found_account_id, &call_id_for_event, "ended", &remote_uri, "inbound")
                ));
            }
        }
        "OPTIONS" => {
            if let Some(base) = build_simple_response(text, 200, "OK") {
                let resp = base.replacen(
                    "Content-Length: 0\r\n",
                    "Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, REFER, INFO\r\nContent-Length: 0\r\n",
                    1,
                );
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    if let Some(ref transport) = account.transport {
                        let _ = transport.send_to(resp.as_bytes(), remote).await;
                        log::debug!("Responded 200 OK to OPTIONS from {}", remote);
                    }
                }
            }
        }
        "REFER" => {
            log::info!("Received REFER from {}", remote);
            transfer::handle_incoming_refer(state, event_tx, text, remote, account_id).await;
        }
        "NOTIFY" => {
            let event_hdr = extract_header(text, "Event").unwrap_or_default();
            let event_trimmed = event_hdr.trim().to_lowercase();
            if event_trimmed == "refer" {
                transfer::handle_notify_refer(state, event_tx, text, remote, account_id).await;
            } else if event_trimmed == "dialog" || event_trimmed == "presence" {
                handle_notify_presence(state, event_tx, text, remote, &event_trimmed, account_id).await;
            } else if event_trimmed == "message-summary" {
                // RFC 3842 MWI NOTIFY — parse the body for voicemail status
                let mwi = parse_mwi_body(text);
                log::info!(
                    "MWI NOTIFY from {}: waiting={}, new={}, old={}",
                    remote, mwi.messages_waiting, mwi.new_messages, mwi.old_messages
                );

                let _ = event_tx.send(SipEvent::VoicemailStatus(VoicemailStatusEvent {
                    account_id: account_id.to_string(),
                    messages_waiting: mwi.messages_waiting,
                    new_messages: mwi.new_messages,
                    old_messages: mwi.old_messages,
                }));

                // Respond 200 OK
                let ok = build_simple_response(text, 200, "OK");
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    if let Some(ref transport) = account.transport {
                        if let Some(resp) = ok {
                            let _ = transport.send_to(resp.as_bytes(), remote).await;
                        }
                    }
                }
            } else {
                let ok = build_simple_response(text, 200, "OK");
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    if let Some(ref transport) = account.transport {
                        if let Some(resp) = ok {
                            let _ = transport.send_to(resp.as_bytes(), remote).await;
                        }
                    }
                }
            }
        }
        "SUBSCRIBE" => {
            let ok = build_200_ok_subscribe(text, 600);
            let s = state.read().await;
            if let Some(account) = s.get_account(account_id) {
                if let Some(ref transport) = account.transport {
                    if let Some(resp) = ok {
                        let _ = transport.send_to(resp.as_bytes(), remote).await;
                    }
                }
            }
        }
        _ => {
            log::debug!("Unhandled incoming request: {}", method);
        }
    }
}

/// Build a 180 Ringing response from an INVITE request.
pub fn build_ringing_response(request: &str, to_tag: &str) -> Option<String> {
    let via = extract_header(request, "Via")?;
    let from = extract_header(request, "From")?;
    let to_raw = extract_header(request, "To")?;
    let call_id = extract_header(request, "Call-ID")?;
    let cseq = extract_header(request, "CSeq")?;

    let to = if to_raw.contains("tag=") {
        to_raw
    } else {
        format!("{};tag={}", to_raw, to_tag)
    };

    Some(format!(
        "SIP/2.0 180 Ringing\r\n\
         Via: {via}\r\n\
         From: {from}\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq}\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
    ))
}

/// Build a simple SIP response (200 OK, etc.) from a request.
pub fn build_simple_response(request: &str, code: u16, reason: &str) -> Option<String> {
    let via = extract_header(request, "Via")?;
    let from = extract_header(request, "From")?;
    let to = extract_header(request, "To")?;
    let call_id = extract_header(request, "Call-ID")?;
    let cseq = extract_header(request, "CSeq")?;

    Some(format!(
        "SIP/2.0 {} {}\r\n\
         Via: {}\r\n\
         From: {}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {}\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
        code, reason, via, from, to, call_id, cseq,
    ))
}

/// Parsed MWI status from a message-summary NOTIFY body.
struct MwiBody {
    messages_waiting: bool,
    new_messages: u32,
    old_messages: u32,
}

/// Parse an RFC 3842 message-summary body.
///
/// Expected format:
/// ```text
/// Messages-Waiting: yes
/// Voice-Message: 3/1 (0/0)
/// ```
///
/// The `Voice-Message` line format is `new/old (urgent_new/urgent_old)`.
fn parse_mwi_body(sip_message: &str) -> MwiBody {
    // The body comes after the blank line separating headers from body
    let body = sip_message
        .find("\r\n\r\n")
        .map(|pos| &sip_message[pos + 4..])
        .or_else(|| sip_message.find("\n\n").map(|pos| &sip_message[pos + 2..]))
        .unwrap_or("");

    let mut waiting = false;
    let mut new_msgs = 0u32;
    let mut old_msgs = 0u32;

    for line in body.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Messages-Waiting:") {
            waiting = value.trim().eq_ignore_ascii_case("yes");
        } else if let Some(value) = line.strip_prefix("Voice-Message:") {
            // Format: "3/1 (0/0)" or just "3/1"
            let counts = value.trim().split_once('(')
                .map_or(value.trim(), |(before, _)| before.trim());
            if let Some((new_str, old_str)) = counts.split_once('/') {
                new_msgs = new_str.trim().parse().unwrap_or(0);
                old_msgs = old_str.trim().parse().unwrap_or(0);
            }
        }
    }

    MwiBody {
        messages_waiting: waiting,
        new_messages: new_msgs,
        old_messages: old_msgs,
    }
}
