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

/// Redact credential material from a raw SIP message.
///
/// Diagnostic traces are rendered in the debug window and written verbatim to
/// disk by the export commands, and users are encouraged to send them to
/// support. Two things in a SIP trace are credential-equivalent:
///
/// * the `response="…"` field of an `Authorization` / `Proxy-Authorization`
///   header — a valid digest proof for that nonce, and the input to an offline
///   dictionary attack on the SIP password;
/// * the `inline:` key material of an SDP `a=crypto` line — the SDES-SRTP
///   master key, which decrypts the call's media.
///
/// Everything else (realm, nonce, URIs, tags, codecs) is preserved so the trace
/// stays useful for debugging.
pub fn redact_sip(msg: &str) -> String {
    // Preserve the original line endings: SIP traces are CRLF and consumers
    // (Wireshark via the PCAP export) care.
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;

    loop {
        match rest.find('\n') {
            Some(idx) => {
                out.push_str(&redact_sip_line(&rest[..=idx]));
                rest = &rest[idx + 1..];
            }
            None => {
                out.push_str(&redact_sip_line(rest));
                break;
            }
        }
    }

    out
}

/// Redact a single line (which may still carry its trailing CR/LF).
fn redact_sip_line(line: &str) -> String {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let eol = &line[trimmed.len()..];
    let lower = trimmed.to_ascii_lowercase();

    if lower.starts_with("authorization:") || lower.starts_with("proxy-authorization:") {
        return format!("{}{}", redact_digest_response(trimmed), eol);
    }

    if lower.starts_with("a=crypto:") {
        return format!("{}{}", redact_crypto_key(trimmed), eol);
    }

    line.to_string()
}

/// Replace the value of `response="…"` (or bare `response=…`) with `REDACTED`.
fn redact_digest_response(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let mut out = String::with_capacity(value.len());
    let mut cursor = 0usize;

    while let Some(rel) = lower[cursor..].find("response=") {
        let start = cursor + rel;
        // Must be a parameter name, not the tail of another token.
        let is_boundary = start == 0
            || !value.as_bytes()[start - 1].is_ascii_alphanumeric()
                && value.as_bytes()[start - 1] != b'-';
        let after = start + "response=".len();
        if !is_boundary {
            out.push_str(&value[cursor..after]);
            cursor = after;
            continue;
        }

        out.push_str(&value[cursor..after]);

        let tail = &value[after..];
        let end_rel = if let Some(stripped) = tail.strip_prefix('"') {
            out.push_str("\"REDACTED\"");
            // Skip past the closing quote.
            match stripped.find('"') {
                Some(q) => 1 + q + 1,
                None => tail.len(),
            }
        } else {
            out.push_str("REDACTED");
            tail.find(',').unwrap_or(tail.len())
        };
        cursor = after + end_rel;
    }

    out.push_str(&value[cursor..]);
    out
}

/// Replace the base64 key material in an SDP `a=crypto` line.
///
/// Format: `a=crypto:<tag> <suite> inline:<key>|<lifetime>|<mki> [params]`
fn redact_crypto_key(line: &str) -> String {
    match line.find("inline:") {
        Some(pos) => {
            let after = pos + "inline:".len();
            // The key runs to the next `|`, whitespace, or end of line.
            let end = line[after..]
                .find(['|', ' ', ';'])
                .map(|p| after + p)
                .unwrap_or(line.len());
            format!("{}REDACTED{}", &line[..after], &line[end..])
        }
        None => line.to_string(),
    }
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
    fn test_redact_authorization_response() {
        let msg = "REGISTER sip:example.com SIP/2.0\r\n\
                   Authorization: Digest username=\"1001\", realm=\"example.com\", \
                   nonce=\"abc123\", uri=\"sip:example.com\", response=\"deadbeefdeadbeefdeadbeefdeadbeef\"\r\n\
                   \r\n";
        let out = redact_sip(msg);
        assert!(!out.contains("deadbeefdeadbeefdeadbeefdeadbeef"));
        assert!(out.contains("response=\"REDACTED\""));
        // Non-secret params survive so the trace stays useful.
        assert!(out.contains("realm=\"example.com\""));
        assert!(out.contains("nonce=\"abc123\""));
        assert!(out.contains("username=\"1001\""));
    }

    #[test]
    fn test_redact_proxy_authorization_response() {
        let msg = "INVITE sip:bob@example.com SIP/2.0\r\n\
                   Proxy-Authorization: Digest username=\"1001\", response=\"cafebabe\", qop=auth\r\n\
                   \r\n";
        let out = redact_sip(msg);
        assert!(!out.contains("cafebabe"));
        assert!(out.contains("qop=auth"));
    }

    #[test]
    fn test_redact_does_not_touch_cnonce_or_challenge() {
        // `cnonce=` must not be mistaken for a response, and the server's
        // WWW-Authenticate challenge carries no secret of ours.
        let msg = "SIP/2.0 401 Unauthorized\r\n\
                   WWW-Authenticate: Digest realm=\"example.com\", nonce=\"xyz\"\r\n\
                   \r\n";
        assert_eq!(redact_sip(msg), msg);
    }

    #[test]
    fn test_redact_sdp_crypto_key() {
        let msg = "INVITE sip:bob@example.com SIP/2.0\r\n\
                   Content-Type: application/sdp\r\n\
                   \r\n\
                   m=audio 30000 RTP/SAVP 0\r\n\
                   a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj|2^20|1:32\r\n";
        let out = redact_sip(msg);
        assert!(!out.contains("d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj"));
        assert!(out.contains("inline:REDACTED|2^20|1:32"));
        // Suite name preserved so we can still see whether SRTP was negotiated.
        assert!(out.contains("AES_CM_128_HMAC_SHA1_80"));
    }

    #[test]
    fn test_redact_preserves_line_endings_and_length_semantics() {
        let msg = "OPTIONS sip:example.com SIP/2.0\r\nVia: foo\r\n\r\n";
        assert_eq!(redact_sip(msg), msg);
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
            // Outbound REGISTER/INVITE carry the digest response and, with
            // SRTP, the SDES key. Never store them verbatim.
            raw: redact_sip(msg),
            call_id: extract_sip_call_id(msg),
        };
        self.store.push(diag.clone()).await;
        let _ = self.event_tx.send(SipEvent::DiagnosticMessage(diag));
    }
}
