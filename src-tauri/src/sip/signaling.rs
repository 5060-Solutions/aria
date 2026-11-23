//! What the call screen may claim about a call's protection.
//!
//! SDES-SRTP (RFC 4568) carries the media key in the SDP, so the media is only
//! as private as the signalling: over UDP or TCP anyone on the path can read
//! the `a=crypto` line and decrypt the audio. The padlock used to read
//! `srtp_active` alone and showed "Encrypted" for exactly that case.

use super::transport::TransportType;
use super::SipManager;

impl SipManager {
    /// Whether the account's signalling currently runs over TLS. For an
    /// `Auto` account this is the transport DNS settled on, not the setting.
    pub async fn signaling_is_tls(&self, account_id: &str) -> bool {
        let s = self.state.read().await;
        s.get_account(account_id)
            .is_some_and(|a| is_tls(&a.config.transport))
    }
}

/// TLS is the only transport that keeps an SDES key out of view.
pub fn is_tls(transport: &TransportType) -> bool {
    *transport == TransportType::Tls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tls_protects_sdes_keys() {
        assert!(is_tls(&TransportType::Tls));
        assert!(!is_tls(&TransportType::Udp));
        assert!(!is_tls(&TransportType::Tcp));
        // Unresolved Auto is not a promise of TLS.
        assert!(!is_tls(&TransportType::Auto));
    }
}
