//! REGISTER response handler.

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use crate::sip::auth::DigestAuth;
use crate::sip::builder::{extract_all_headers, extract_header, extract_via_received};
use crate::sip::diagnostics::{self, DiagnosticLog};
use crate::sip::state::RegistrationTransitionResult;
use crate::sip::{finish_unregister, send_register, ManagerState, RegistrationEvent, RegistrationState, SipEvent};

/// The lifetime the registrar granted our binding, from the 200 OK.
///
/// Per-binding `;expires=` on the Contact that is ours wins (other devices'
/// bindings are listed too); the `Expires` header is the fallback. None means
/// the response said nothing usable and the requested value stands.
fn granted_expires(text: &str, local_addr: Option<SocketAddr>, public_addr: Option<SocketAddr>) -> Option<u32> {
    let contacts = extract_all_headers(text, "Contact");
    let is_ours = |c: &String| {
        [local_addr, public_addr]
            .into_iter()
            .flatten()
            .any(|a| c.contains(&a.to_string()))
    };
    let contact_expires = |c: &String| {
        c.split(';')
            .skip(1)
            .find_map(|p| p.trim().strip_prefix("expires=")?.trim().parse::<u32>().ok())
    };
    contacts
        .iter()
        .find(|c| is_ours(c))
        .or(contacts.first())
        .and_then(contact_expires)
        .or_else(|| extract_header(text, "Expires")?.trim().parse().ok())
        .filter(|e| *e > 0)
}

/// Report a registration failure, if the account was in a state where it is one.
async fn fail_registration(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    account_id: &str,
    status: u16,
    reason: &str,
) {
    let (unregistering, transitioned) = {
        let mut s = state.write().await;
        let Some(account) = s.get_account_mut(account_id) else { return };
        if account.unregistering {
            (true, false)
        } else {
            account.refreshing = false;
            let result = account.registration.registration_failed(status, reason);
            (false, matches!(result, RegistrationTransitionResult::Ok))
        }
    };
    if unregistering {
        // Whatever went wrong, the user asked for this account to go.
        finish_unregister(state, event_tx, account_id).await;
        return;
    }
    // Only when the FSM actually moved: a late error for a registration that
    // already failed or was torn down must not flip the UI again.
    if transitioned {
        let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
            account_id: account_id.to_string(),
            state: RegistrationState::Error,
            error: Some(reason.to_string()),
        }));
    }
}

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
            let public = extract_via_received(text).map(|(ip, port)| {
                let addr: SocketAddr = format!("{}:{}", ip, port)
                    .parse()
                    .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], port)));
                log::info!("Discovered public address from Via: {}", addr);
                addr
            });

            let (unregistering, transitioned) = {
                let mut s = state.write().await;
                let Some(account) = s.get_account_mut(account_id) else { return };
                if account.unregistering {
                    (true, false)
                } else {
                    if let Some(granted) = granted_expires(text, account.local_addr, account.public_addr) {
                        if granted != account.granted_expires {
                            log::info!("Registrar granted Expires {} for {}", granted, account_id);
                        }
                        account.granted_expires = granted;
                    }
                    account.refreshing = false;
                    let result = account.registration.registration_success(public);
                    let ok = matches!(result, RegistrationTransitionResult::Ok);
                    if ok {
                        if let Some(addr) = public {
                            account.public_addr = Some(addr);
                        }
                    }
                    (false, ok)
                }
            };
            if unregistering {
                log::info!("Unregistered account {}", account_id);
                finish_unregister(state, event_tx, account_id).await;
                return;
            }
            if !transitioned {
                // A 200 for a registration that has since timed out or been
                // torn down. Announcing it would show a registered account
                // the backend is not actually running.
                log::debug!("Ignoring 200 OK to REGISTER for {} — not registering", account_id);
                return;
            }
            log::info!("Registration successful for account {}", account_id);
            let _ = event_tx.send(SipEvent::RegistrationChanged(RegistrationEvent {
                account_id: account_id.to_string(),
                state: RegistrationState::Registered,
                error: None,
            }));

            // Re-subscribe to BLF/presence if we have pending resubscriptions
            // (saved when transport died, before subscriptions were cleared).
            let resub_targets: Vec<(String, crate::sip::state::EventType)> = {
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    std::mem::take(&mut account.pending_resubscriptions)
                } else {
                    Vec::new()
                }
            };
            if !resub_targets.is_empty() {
                log::info!(
                    "Re-subscribing to {} BLF/presence targets after reconnect for {}",
                    resub_targets.len(),
                    account_id
                );
                // Emit event so frontend knows to re-subscribe
                // (subscribe_presence requires the Manager which we don't have here)
                for (uri, event_type) in &resub_targets {
                    log::info!("  → re-subscribe: {} ({:?})", uri, event_type);
                }
                // Store them back temporarily — the frontend will call subscribe_presence
                // for each via the Tauri command interface when it sees Registered state.
                {
                    let mut s = state.write().await;
                    if let Some(account) = s.get_account_mut(account_id) {
                        account.pending_resubscriptions = resub_targets;
                    }
                }
            }
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
                fail_registration(
                    state,
                    event_tx,
                    account_id,
                    status,
                    "Authentication failed - check username and password",
                )
                .await;
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
                    fail_registration(
                        state,
                        event_tx,
                        account_id,
                        status,
                        &format!("{} with no challenge header — check server config", status),
                    )
                    .await;
                    return;
                }
            };

            let (account_config, server_addr) = {
                let s = state.read().await;
                let account = match s.get_account(account_id) {
                    Some(a) => a,
                    None => return,
                };
                let server_addr = match account.server_addr {
                    Some(a) => a,
                    None => return,
                };
                (account.config.clone(), server_addr)
            };

            let registrar = account_config.registrar.as_deref().unwrap_or(&account_config.domain);
            let uri = format!("sip:{}", registrar);
            log::debug!(
                "Digest params: auth_user='{}', uri='{}', auth_realm_override={:?}",
                account_config.effective_auth_username(),
                uri,
                account_config.auth_realm
            );

            // Explicit override, then the 403 server-IP fallback, then the
            // announced realm — except that an override the server has just
            // rejected gives way to the announced realm. See
            // `auth::registration_realm`.
            let challenge_realm = crate::sip::auth::extract_challenge_realm(&www_auth);
            let realm_override = {
                let mut s = state.write().await;
                match s.get_account_mut(account_id) {
                    Some(account) => {
                        let (realm, override_rejected) = crate::sip::auth::registration_realm(
                            account.config.auth_realm.as_deref(),
                            account.realm_fallback.as_deref(),
                            challenge_realm.as_deref(),
                            attempts,
                        );
                        if override_rejected {
                            log::warn!(
                                "realm override {:?} was rejected; retrying with the server's realm {:?} \
                                 and using it for the rest of this session",
                                account.config.auth_realm,
                                challenge_realm
                            );
                            // Calls, presence and refreshes read the same
                            // config, so they follow what the server accepts.
                            account.config.auth_realm = None;
                            account.registration.forget_realm_override();
                        }
                        realm
                    }
                    None => account_config.auth_realm.clone(),
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
                    // Through the same redaction as every captured message:
                    // the digest `response=` is what an offline attack on the
                    // password needs, and this entry used to carry it into
                    // every exported log.
                    raw: diagnostics::redact_sip(&format!(
                        "{}\n\nChallenge header:\n{}\n\nAuth response header:\nAuthorization: {}",
                        debug_summary,
                        www_auth,
                        auth.as_ref().map(|a| a.to_header()).unwrap_or_else(|| "(parse failed)".to_string()),
                    )),
                };
                let s = state.read().await;
                s.diagnostic_store.push(diag.clone()).await;
                let _ = event_tx.send(SipEvent::DiagnosticMessage(diag));
            }

            let auth_header = match auth {
                Some(a) => a.to_header(),
                None => {
                    log::error!("Failed to parse auth challenge header: {}", www_auth);
                    fail_registration(state, event_tx, account_id, status, "Could not parse server auth challenge").await;
                    return;
                }
            };

            {
                let mut s = state.write().await;
                if let Some(account) = s.get_account_mut(account_id) {
                    account.registration.increment_auth_attempts();
                } else {
                    return;
                }
            }

            // Through the shared sender, so the authenticated REGISTER is
            // retransmitted and has a deadline like the first one, and so its
            // new CSeq stops the first one's retransmissions.
            log::debug!("Sending authenticated REGISTER to {}", server_addr);
            if let Err(e) = send_register(state, event_tx, account_id, Some(&auth_header)).await {
                log::error!("Failed to send auth REGISTER: {}", e);
            }
        }
        423 => {
            // Interval Too Brief: ask for at least what the registrar wants.
            // This used to be a terminal error, and the frontend's retry sent
            // the identical request.
            let min_expires = extract_header(text, "Min-Expires")
                .and_then(|v| v.trim().parse::<u32>().ok());
            let Some(min_expires) = min_expires else {
                fail_registration(state, event_tx, account_id, status, "Registrar rejected the registration interval").await;
                return;
            };
            {
                let mut s = state.write().await;
                let Some(account) = s.get_account_mut(account_id) else { return };
                if !account.registration.is_registering() {
                    return;
                }
                log::info!("Registrar wants Expires >= {} for {}; retrying", min_expires, account_id);
                account.requested_expires = account.requested_expires.max(min_expires);
            }
            if let Err(e) = send_register(state, event_tx, account_id, None).await {
                log::error!("Failed to resend REGISTER after 423: {}", e);
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
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

            if retry_info {
                if let Err(e) = send_register(state, event_tx, account_id, None).await {
                    log::error!("Failed to send fallback REGISTER: {}", e);
                }
            } else {
                log::error!("Registration forbidden (403) for account {}", account_id);
                fail_registration(state, event_tx, account_id, status, "Authentication failed").await;
            }
        }
        _ if status >= 400 => {
            log::warn!("Registration response: {} for account {}", status, account_id);
            // A refresh that fails because the server is struggling (timeout,
            // unavailable, internal error) is a connection problem: reconnect
            // with backoff, as a lost response would. It used to become a
            // terminal error that nothing in the backend recovered from.
            let refreshing = {
                let s = state.read().await;
                s.get_account(account_id).is_some_and(|a| a.refreshing && !a.unregistering)
            };
            if refreshing && matches!(status, 408 | 480 | 500..=599) {
                log::warn!("Refresh for {} failed with {}; reconnecting", account_id, status);
                crate::sip::SipManager::handle_transport_death(
                    state.clone(),
                    event_tx.clone(),
                    account_id.to_string(),
                    true,
                )
                .await;
                return;
            }
            fail_registration(state, event_tx, account_id, status, &format!("Registration failed: {}", status)).await;
        }
        _ => {
            log::debug!("Registration response: {} for account {}", status, account_id);
        }
    }
}
