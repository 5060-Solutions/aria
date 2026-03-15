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
    /// SIP Call-ID header value, if present (links SIP messages to calls)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

/// Extract the Call-ID header from a raw SIP message.
pub fn extract_sip_call_id(msg: &str) -> Option<String> {
    for line in msg.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("call-id:") || lower.starts_with("i:") {
            if let Some(val) = line.split_once(':').map(|(_, v)| v.trim().to_string()) {
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_sip_call_id_standard() {
        let msg = "INVITE sip:bob@example.com SIP/2.0\r\n\
                   Via: SIP/2.0/UDP 10.0.0.1:5060\r\n\
                   Call-ID: abc123@10.0.0.1\r\n\
                   From: <sip:alice@example.com>;tag=xyz\r\n\
                   \r\n";
        assert_eq!(extract_sip_call_id(msg), Some("abc123@10.0.0.1".to_string()));
    }

    #[test]
    fn test_extract_sip_call_id_compact_form() {
        let msg = "SIP/2.0 200 OK\r\n\
                   i: compact-call-id@host\r\n\
                   \r\n";
        assert_eq!(extract_sip_call_id(msg), Some("compact-call-id@host".to_string()));
    }

    #[test]
    fn test_extract_sip_call_id_missing() {
        let msg = "SIP/2.0 200 OK\r\n\
                   Via: SIP/2.0/UDP 10.0.0.1:5060\r\n\
                   \r\n";
        assert_eq!(extract_sip_call_id(msg), None);
    }

    #[test]
    fn test_extract_sip_call_id_case_insensitive() {
        let msg = "REGISTER sip:example.com SIP/2.0\r\n\
                   call-id: lowercase-id@host\r\n\
                   \r\n";
        assert_eq!(extract_sip_call_id(msg), Some("lowercase-id@host".to_string()));
    }

    #[test]
    fn test_summarize_sip() {
        let msg = "SIP/2.0 200 OK\r\nVia: foo\r\n";
        assert_eq!(summarize_sip(msg), "SIP/2.0 200 OK");
    }

    #[test]
    fn test_summarize_sip_empty() {
        assert_eq!(summarize_sip(""), "(empty)");
    }

    #[test]
    fn test_diagnostic_log_serializes_call_id() {
        let log = DiagnosticLog {
            timestamp: 1000,
            account_id: "acc1".to_string(),
            direction: MessageDirection::Sent,
            remote_addr: "10.0.0.1:5060".to_string(),
            summary: "INVITE".to_string(),
            raw: "INVITE sip:bob@example.com SIP/2.0".to_string(),
            call_id: Some("test-call-id@host".to_string()),
        };
        let json = serde_json::to_string(&log).unwrap();
        assert!(json.contains("\"callId\":\"test-call-id@host\""));
    }

    #[test]
    fn test_diagnostic_log_omits_null_call_id() {
        let log = DiagnosticLog {
            timestamp: 1000,
            account_id: "acc1".to_string(),
            direction: MessageDirection::Sent,
            remote_addr: "10.0.0.1:5060".to_string(),
            summary: "OPTIONS".to_string(),
            raw: "OPTIONS sip:example.com SIP/2.0".to_string(),
            call_id: None,
        };
        let json = serde_json::to_string(&log).unwrap();
        assert!(!json.contains("callId"));
    }
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
            call_id: extract_sip_call_id(msg),
        };
        self.store.push(diag.clone()).await;
        let _ = self.event_tx.send(SipEvent::DiagnosticMessage(diag));
    }
}
