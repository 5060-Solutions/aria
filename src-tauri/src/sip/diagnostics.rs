use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

use super::SipEvent;

/// Direction of a SIP message
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageDirection {
    Sent,
    Received,
}

/// A single SIP message log entry
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLog {
    pub timestamp: u64,
    pub account_id: String,
    pub direction: MessageDirection,
    pub remote_addr: String,
    pub summary: String,
    pub raw: String,
}

/// Stores SIP diagnostic messages (ring buffer)
pub struct DiagnosticStore {
    messages: Arc<RwLock<Vec<DiagnosticLog>>>,
    max_entries: usize,
}

impl DiagnosticStore {
    pub fn new(max_entries: usize) -> Self {
        Self {
            messages: Arc::new(RwLock::new(Vec::with_capacity(max_entries))),
            max_entries,
        }
    }

    pub async fn push(&self, log: DiagnosticLog) {
        let mut msgs = self.messages.write().await;
        if msgs.len() >= self.max_entries {
            msgs.remove(0);
        }
        msgs.push(log);
    }

    pub async fn get_all(&self) -> Vec<DiagnosticLog> {
        self.messages.read().await.clone()
    }

    pub async fn clear(&self) {
        self.messages.write().await.clear();
    }
}

/// Create a summary line from a SIP message (first line)
pub fn summarize_sip(msg: &str) -> String {
    msg.lines().next().unwrap_or("(empty)").to_string()
}

/// Get current timestamp in millis
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Lightweight handle that the transport layer uses to log outbound messages.
/// Cloneable and cheap — stores only an Arc to the diagnostic store, the event
/// sender, and the account ID.
#[derive(Clone)]
pub struct DiagnosticSender {
    store: Arc<DiagnosticStore>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
    account_id: String,
}

impl DiagnosticSender {
    pub fn new(
        store: Arc<DiagnosticStore>,
        event_tx: mpsc::UnboundedSender<SipEvent>,
        account_id: String,
    ) -> Self {
        Self {
            store,
            event_tx,
            account_id,
        }
    }

    pub async fn log_sent(&self, msg: &str, remote: SocketAddr) {
        let diag = DiagnosticLog {
            timestamp: now_millis(),
            account_id: self.account_id.clone(),
            direction: MessageDirection::Sent,
            remote_addr: remote.to_string(),
            summary: summarize_sip(msg),
            raw: msg.to_string(),
        };
        self.store.push(diag.clone()).await;
        let _ = self.event_tx.send(SipEvent::DiagnosticMessage(diag));
    }
}
