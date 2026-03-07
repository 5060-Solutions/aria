//! Presence/BLF handlers (SUBSCRIBE response and NOTIFY).

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::sip::auth::DigestAuth;
use crate::sip::builder::{build_subscribe, extract_header, extract_to_tag};
use crate::sip::diagnostics::{self, DiagnosticLog};
use crate::sip::{presence, BlfEntry, ManagerState, SipEvent, SubscriptionState};

use super::request::build_simple_response;

/// Handle SUBSCRIBE response (200 OK, auth challenges, errors).
pub async fn handle_subscribe_response(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    status: u16,
    account_id: &str,
) {
    let call_id_header = match extract_header(text, "Call-ID") {
        Some(c) => c,
        None => return,
    };

    match status {
        200 => {
            log::info!("SUBSCRIBE accepted (200 OK)");
            let to_tag = extract_to_tag(text);
            let mut s = state.write().await;
            if let Some(account) = s.get_account_mut(account_id) {
                if let Some(sub) = account
                    .subscriptions
                    .iter_mut()
                    .find(|s| s.call_id == call_id_header)
                {
                    sub.state = SubscriptionState::Active;
                    if let Some(tag) = to_tag {
                        sub.to_tag = Some(tag);
                    }
                }
            }
        }
        401 | 407 => {
            log::info!("SUBSCRIBE challenged ({}), sending auth", status);
            let auth_hdr_name = if status == 401 {
                "WWW-Authenticate"
            } else {
                "Proxy-Authenticate"
            };
            let www_auth = match extract_header(text, auth_hdr_name) {
                Some(h) => h,
                None => {
                    log::error!("No auth challenge header for SUBSCRIBE");
                    return;
                }
            };

            let (account_config, local_addr, server_addr, transport) = {
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
                (account.config.clone(), local_addr, server_addr, transport)
            };

            let sub_info = {
                let mut s = state.write().await;
                let account = match s.get_account_mut(account_id) {
                    Some(a) => a,
                    None => return,
                };
                let sub = match account
                    .subscriptions
                    .iter_mut()
                    .find(|s| s.call_id == call_id_header)
                {
                    Some(sub) => sub,
                    None => return,
                };
                sub.cseq += 1;
                (
                    sub.target_uri.clone(),
                    sub.call_id.clone(),
                    sub.cseq,
                    sub.from_tag.clone(),
                    sub.event_type.clone(),
                    sub.expires,
                )
            };

            let (target_uri, sub_call_id, cseq, from_tag, event_type, expires) = sub_info;

            let auth = DigestAuth::from_challenge(
                &www_auth,
                account_config.effective_auth_username(),
                &account_config.password,
                &target_uri,
                "SUBSCRIBE",
            );

            let auth_header = match auth {
                Some(a) => a.to_header(),
                None => {
                    log::error!("Failed to parse SUBSCRIBE auth challenge");
                    return;
                }
            };

            let msg = build_subscribe(
                &account_config,
                &target_uri,
                local_addr,
                &sub_call_id,
                cseq,
                &from_tag,
                &event_type,
                expires,
                Some(&auth_header),
            );

            if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                log::error!("Failed to send authenticated SUBSCRIBE: {}", e);
            }

            let diag = DiagnosticLog {
                timestamp: diagnostics::now_millis(),
                account_id: account_id.to_string(),
                direction: diagnostics::MessageDirection::Sent,
                remote_addr: server_addr.to_string(),
                summary: diagnostics::summarize_sip(&msg),
                raw: msg,
            };
            {
                let s = state.read().await;
                s.diagnostic_store.push(diag.clone()).await;
            }
            let _ = event_tx.send(SipEvent::DiagnosticMessage(diag));
        }
        489 => {
            log::warn!("SUBSCRIBE rejected: Bad Event (489)");
            let mut s = state.write().await;
            if let Some(account) = s.get_account_mut(account_id) {
                account.subscriptions.retain(|s| s.call_id != call_id_header);
            }
        }
        _ if status >= 400 => {
            log::warn!("SUBSCRIBE failed: {}", status);
            let mut s = state.write().await;
            if let Some(account) = s.get_account_mut(account_id) {
                account.subscriptions.retain(|s| s.call_id != call_id_header);
            }
        }
        _ => {
            log::debug!("SUBSCRIBE response: {}", status);
        }
    }
}

/// Handle NOTIFY for presence/BLF updates.
pub async fn handle_notify_presence(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    remote: SocketAddr,
    event_type: &str,
    account_id: &str,
) {
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

    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    if body.is_empty() {
        return;
    }

    let presence_state = if event_type == "dialog" {
        presence::parse_dialog_info_xml(body)
    } else {
        presence::parse_pidf_xml(body)
    };

    let call_id_header = match extract_header(text, "Call-ID") {
        Some(c) => c,
        None => return,
    };

    let sub_state_hdr = extract_header(text, "Subscription-State").unwrap_or_default();
    let sub_state_val = sub_state_hdr
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();

    let target_uri = {
        let mut s = state.write().await;
        let account = match s.get_account_mut(account_id) {
            Some(a) => a,
            None => return,
        };
        let sub = match account
            .subscriptions
            .iter_mut()
            .find(|s| s.call_id == call_id_header)
        {
            Some(sub) => sub,
            None => {
                log::debug!(
                    "NOTIFY for unknown subscription (Call-ID: {})",
                    call_id_header
                );
                return;
            }
        };

        match sub_state_val.as_str() {
            "active" => sub.state = SubscriptionState::Active,
            "terminated" => sub.state = SubscriptionState::Terminated,
            "pending" => sub.state = SubscriptionState::Pending,
            _ => {}
        }

        sub.target_uri.clone()
    };

    let extension = presence::extract_extension_from_uri(&target_uri);

    let blf_entries = {
        let mut s = state.write().await;
        let account = match s.get_account_mut(account_id) {
            Some(a) => a,
            None => return,
        };
        account.blf_states.insert(
            extension.clone(),
            BlfEntry {
                extension,
                state: presence_state,
                display_name: None,
            },
        );
        account.blf_states.values().cloned().collect::<Vec<_>>()
    };

    let _ = event_tx.send(SipEvent::PresenceChanged(account_id.to_string(), blf_entries));

    if sub_state_val == "terminated" {
        let mut s = state.write().await;
        if let Some(account) = s.get_account_mut(account_id) {
            account.subscriptions.retain(|s| s.call_id != call_id_header);
        }
    }
}
