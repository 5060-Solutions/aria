//! REGISTER response handler.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::sip::auth::DigestAuth;
use crate::sip::builder::{build_register, extract_header, extract_via_received};
use crate::sip::diagnostics::{self, DiagnosticLog};
use crate::sip::{ManagerState, RegistrationEvent, RegistrationState, SipEvent};

/// Handle REGISTER response (200 OK, 401/407 challenge, errors).
pub async fn handle_register_response(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    status: u16,
    account_id: &str,
) {
    match status {
        200 => {
            log::info!("Registration successful for account {}", account_id);

            let public = extract_via_received(text).map(|(ip, port)| {
                let addr: SocketAddr = format!("{}:{}", ip, port)
                    .parse()
                    .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));
                log::info!("Discovered public address from Via: {}", addr);
                addr
            });

            {
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    account.registration.registration_success(public);
                    if let Some(addr) = public {
                        account.public_addr = Some(addr);
                    }
                }
            }
            let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                account_id: account_id.to_string(),
                state: RegistrationState::Registered,
                error: None,
            }));
        }
        401 | 407 => {
            // Check current state and auth attempts to prevent infinite loops
            let (attempts, is_registering) = {
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    (account.registration.auth_attempts(), account.registration.is_registering())
                } else {
                    (0, false)
                }
            };
            
            // Ignore 401/407 if we're not actively registering (already succeeded, failed, or not started)
            if !is_registering {
                log::debug!("Ignoring {} for account {} - not in registering state", status, account_id);
                return;
            }
            
            log::info!("Registration challenged ({}) for account {}, auth attempt #{}", status, account_id, attempts);
            
            if attempts >= 2 {
                log::error!("Too many auth attempts ({}) for account {} - credentials likely wrong", attempts, account_id);
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    account.registration.registration_failed(
                        status,
                        "Authentication failed - check username and password",
                    );
                }
                let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                    account_id: account_id.to_string(),
                    state: RegistrationState::Error,
                    error: Some("Authentication failed - check username and password".into()),
                }));
                return;
            }
            
            let www_auth = extract_header(
                text,
                if status == 401 {
                    "WWW-Authenticate"
                } else {
                    "Proxy-Authenticate"
                },
            );

            let www_auth = match www_auth {
                Some(h) => h,
                None => {
                    log::error!("No auth challenge header in {} — cannot authenticate", status);
                    let mut s = state.write().await;
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.registration.registration_failed(
                            status,
                            &format!("{} with no challenge header — check server config", status),
                        );
                    }
                    let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                        account_id: account_id.to_string(),
                        state: RegistrationState::Error,
                        error: Some(format!("{} with no challenge header", status)),
                    }));
                    return;
                }
            };

            let (account_config, local_addr, server_addr, reg_call_id, reg_from_tag, transport) = {
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
                (
                    account.config.clone(),
                    local_addr,
                    server_addr,
                    account.registration.call_id().to_string(),
                    account.registration.local_tag().to_string(),
                    transport,
                )
            };

            let registrar = account_config.registrar.as_deref().unwrap_or(&account_config.domain);
            let uri = format!("sip:{}", registrar);

            let auth = DigestAuth::from_challenge(
                &www_auth,
                account_config.effective_auth_username(),
                &account_config.password,
                &uri,
                "REGISTER",
            );

            let auth_header = match auth {
                Some(a) => a.to_header(),
                None => {
                    log::error!("Failed to parse auth challenge header: {}", www_auth);
                    let mut s = state.write().await;
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.registration
                            .registration_failed(status, "Could not parse server auth challenge");
                    }
                    let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                        account_id: account_id.to_string(),
                        state: RegistrationState::Error,
                        error: Some("Could not parse server auth challenge".into()),
                    }));
                    return;
                }
            };

            let cseq = {
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    account.registration.increment_auth_attempts();
                    account.registration.next_cseq()
                } else {
                    return;
                }
            };

            let msg = build_register(
                &account_config,
                local_addr,
                &reg_call_id,
                cseq,
                &reg_from_tag,
                Some(&auth_header),
                3600,
            );

            log::debug!("Sending authenticated REGISTER:\n{}", msg);

            let diag = DiagnosticLog {
                timestamp: diagnostics::now_millis(),
                account_id: account_id.to_string(),
                direction: diagnostics::MessageDirection::Sent,
                remote_addr: server_addr.to_string(),
                summary: diagnostics::summarize_sip(&msg),
                raw: msg.clone(),
            };
            {
                let s = state.read().await;
                s.diagnostic_store.push(diag.clone()).await;
            }
            let _ = event_tx.send(SipEvent::DiagnosticMessage(diag));

            if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                log::error!("Failed to send auth REGISTER: {}", e);
            }
        }
        403 => {
            log::error!("Registration forbidden (403) for account {}", account_id);
            {
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    account.registration
                        .registration_failed(status, "403 Forbidden — check username/password");
                }
            }
            let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                account_id: account_id.to_string(),
                state: RegistrationState::Error,
                error: Some("Authentication failed".into()),
            }));
        }
        _ => {
            log::warn!("Registration response: {} for account {}", status, account_id);
            if status >= 400 {
                {
                    let mut s = state.write().await;
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.registration
                            .registration_failed(status, &format!("Registration failed: {}", status));
                    }
                }
                let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                    account_id: account_id.to_string(),
                    state: RegistrationState::Error,
                    error: Some(format!("Registration failed: {}", status)),
                }));
            }
        }
    }
}
