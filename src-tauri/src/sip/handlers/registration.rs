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

            log::info!("Auth challenge header: {}", www_auth);

            let registrar = account_config.registrar.as_deref().unwrap_or(&account_config.domain);
            let uri = format!("sip:{}", registrar);
            log::info!(
                "Digest params: auth_user='{}', uri='{}', auth_realm_override={:?}",
                account_config.effective_auth_username(),
                uri,
                account_config.auth_realm
            );

            // Use realm override: explicit config > fallback from 403 retry > challenge realm
            let realm_override = {
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    account.config.auth_realm.clone()
                        .or_else(|| account.realm_fallback.clone())
                } else {
                    account_config.auth_realm.clone()
                }
            };

            let auth = DigestAuth::from_challenge_with_realm(
                &www_auth,
                account_config.effective_auth_username(),
                &account_config.password,
                &uri,
                "REGISTER",
                realm_override.as_deref(),
            );

            // Emit auth debug info as a diagnostic message visible in the UI
            {
                let challenge_realm = crate::sip::auth::extract_challenge_realm(&www_auth);
                let effective_realm = realm_override.as_deref()
                    .unwrap_or(challenge_realm.as_deref().unwrap_or("(unknown)"));
                let debug_summary = format!(
                    "[AUTH DEBUG] user='{}', challenge_realm='{}', effective_realm='{}', uri='{}', override={:?}",
                    account_config.effective_auth_username(),
                    challenge_realm.as_deref().unwrap_or("(none)"),
                    effective_realm,
                    uri,
                    realm_override,
                );
                let diag = DiagnosticLog {
                    timestamp: diagnostics::now_millis(),
                    account_id: account_id.to_string(),
                    direction: diagnostics::MessageDirection::Sent,
                    remote_addr: server_addr.to_string(),
                    summary: debug_summary.clone(),
                    call_id: None, // Auth debug entry, not a SIP message
                    raw: format!(
                        "{}\n\nChallenge header:\n{}\n\nAuth response header:\n{}",
                        debug_summary,
                        www_auth,
                        auth.as_ref().map(|a| a.to_header()).unwrap_or_else(|| "(parse failed)".to_string()),
                    ),
                };
                let s = state.read().await;
                s.diagnostic_store.push(diag.clone()).await;
                let _ = event_tx.send(SipEvent::DiagnosticMessage(diag));
            }

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

            if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                log::error!("Failed to send auth REGISTER: {}", e);
            }
        }
        403 => {
            // When we get 403 after auth, try realm fallback with server IP.
            // FreeSwitch sometimes sends a challenge realm that differs from
            // the realm it uses internally for HA1 verification.
            let retry_info = {
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    if account.config.auth_realm.is_none()
                        && !account.realm_fallback_exhausted
                    {
                        if let Some(sa) = account.server_addr {
                            let server_ip = sa.ip().to_string();
                            log::info!(
                                "403 after auth — retrying with server IP '{}' as realm",
                                server_ip
                            );
                            account.realm_fallback = Some(server_ip);
                            account.realm_fallback_exhausted = true;
                            // Reset FSM so we can re-register
                            account.registration.registration_failed(403, "retrying with realm fallback");
                            account.registration.start_registration(account.config.clone());
                            let cseq = account.registration.next_cseq();
                            let config = account.config.clone();
                            let local_addr = account.local_addr;
                            let server_addr = account.server_addr;
                            let transport = account.transport.clone();
                            let call_id = account.registration.call_id().to_string();
                            let from_tag = account.registration.local_tag().to_string();
                            Some((config, local_addr, server_addr, transport, call_id, from_tag, cseq))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((config, Some(local_addr), Some(server_addr), Some(transport), call_id, from_tag, cseq)) = retry_info {
                let msg = build_register(&config, local_addr, &call_id, cseq, &from_tag, None, 3600);

                if let Err(e) = transport.send_to(msg.as_bytes(), server_addr).await {
                    log::error!("Failed to send fallback REGISTER: {}", e);
                }
            } else {
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
