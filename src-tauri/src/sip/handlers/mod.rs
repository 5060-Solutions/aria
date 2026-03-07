//! SIP message handlers.
//!
//! This module contains handlers for incoming SIP requests and responses,
//! organized by message type.

mod invite;
mod presence;
mod registration;
mod request;

pub use invite::handle_invite_response;
pub use presence::handle_subscribe_response;
pub use registration::handle_register_response;
pub use request::handle_incoming_request;

use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::builder::{extract_header, parse_status_code};
use super::{ManagerState, SipEvent, TransferEvent};

/// Route a SIP response to the appropriate handler based on CSeq method.
pub async fn handle_response(
    state: &Arc<RwLock<ManagerState>>,
    event_tx: &mpsc::UnboundedSender<SipEvent>,
    text: &str,
    account_id: &str,
) {
    let status = match parse_status_code(text) {
        Some(c) => c,
        None => return,
    };
    let cseq_header = extract_header(text, "CSeq").unwrap_or_default();
    let method = cseq_header
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string();

    match method.as_str() {
        "REGISTER" => handle_register_response(state, event_tx, text, status, account_id).await,
        "INVITE" => handle_invite_response(state, event_tx, text, status).await,
        "BYE" | "CANCEL" => {
            log::info!("{} response: {}", method, status);
        }
        "REFER" => {
            log::info!("REFER response: {}", status);
            if status == 202 {
                log::info!("REFER accepted (202)");
            } else if status >= 400 {
                log::warn!("REFER rejected ({})", status);
                let call_id_header = extract_header(text, "Call-ID").unwrap_or_default();
                let s = state.read().await;
                if let Some(account) = s.get_account(account_id) {
                    if let Some(call) = account.calls.iter().find(|c| c.call_id_header == call_id_header) {
                        let _ = event_tx.send(SipEvent::TransferProgress(TransferEvent {
                            account_id: account_id.to_string(),
                            call_id: call.id.clone(),
                            status,
                            message: format!("Transfer rejected: {}", status),
                        }));
                    }
                }
            }
        }
        "SUBSCRIBE" => handle_subscribe_response(state, event_tx, text, status, account_id).await,
        "OPTIONS" => {
            log::debug!("OPTIONS response: {}", status);
        }
        _ => {
            log::debug!("Unhandled response for {}: {}", method, status);
        }
    }
}
