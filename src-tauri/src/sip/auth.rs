// Re-export digest auth from the SIP library
#[allow(unused_imports)]
pub use rsip::sip_auth::{extract_challenge_realm, extract_param, DigestAuth};

/// The realm to put in a REGISTER's digest, and whether the configured
/// override has just been proven wrong.
///
/// `configured` is the account's realm override, `fallback` the server-IP
/// realm tried after a 403, `challenge` the realm the server announced, and
/// `prior_attempts` how many authenticated REGISTERs this registration has
/// already sent.
///
/// An override is honoured first — some providers verify against a realm other
/// than the one they announce. But a second challenge after an attempt made
/// with an override that differs from the announced realm means the server
/// rejected it, so the last attempt uses the server's own realm. Without this
/// an override that is simply wrong — the setup wizard used to save the SIP
/// domain as one whenever the field was left blank — failed every attempt,
/// and the 403 fallback never ran because an override was set (#4).
pub fn registration_realm(
    configured: Option<&str>,
    fallback: Option<&str>,
    challenge: Option<&str>,
    prior_attempts: u32,
) -> (Option<String>, bool) {
    let configured = configured.filter(|r| !r.trim().is_empty());
    if let (Some(set), Some(announced)) = (configured, challenge) {
        if prior_attempts >= 1 && set != announced {
            return (None, true);
        }
    }
    (configured.or(fallback).map(str::to_owned), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_digest_auth() {
        let auth = DigestAuth {
            username: "bob".to_string(),
            password: "zanzibar".to_string(),
            realm: "biloxi.com".to_string(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_string(),
            uri: "sip:bob@biloxi.com".to_string(),
            method: "REGISTER".to_string(),
            algorithm: "MD5".to_string(),
            qop: None,
            nc: 1,
            cnonce: "0a4f113b".to_string(),
        };
        // Just verify it produces a 32-char hex string
        assert_eq!(auth.response().len(), 32);
    }

    #[test]
    fn test_extract_param() {
        let header = r#"Digest realm="biloxi.com", nonce="abc123", algorithm=MD5"#;
        assert_eq!(extract_param(header, "realm"), Some("biloxi.com".into()));
        assert_eq!(extract_param(header, "nonce"), Some("abc123".into()));
        assert_eq!(extract_param(header, "algorithm"), Some("MD5".into()));
    }
}

#[cfg(test)]
mod registration_realm_tests {
    use super::registration_realm;

    #[test]
    fn with_no_override_the_announced_realm_is_used() {
        assert_eq!(registration_realm(None, None, Some("voip_stack"), 0), (None, false));
        assert_eq!(registration_realm(Some(""), None, Some("voip_stack"), 0), (None, false));
    }

    /// Providers that need an override still get it: it is always tried first.
    #[test]
    fn an_override_is_tried_first() {
        let got = registration_realm(Some("teliax.com"), None, Some("sip.teliax.net"), 0);
        assert_eq!(got, (Some("teliax.com".to_owned()), false));
    }

    /// The reported case (#4): the wizard saved the domain as an override, the
    /// server rejected it, and the retry uses the server's own realm.
    #[test]
    fn a_rejected_override_gives_way_to_the_announced_realm() {
        let got = registration_realm(Some("homeassistant.lan"), None, Some("voip_stack"), 1);
        assert_eq!(got, (None, true));
    }

    /// Rejected, but it already *was* the announced realm: switching would try
    /// the same thing again, so the override stays and the password is wrong.
    #[test]
    fn an_override_matching_the_challenge_is_not_second_guessed() {
        let got = registration_realm(Some("voip_stack"), None, Some("voip_stack"), 1);
        assert_eq!(got, (Some("voip_stack".to_owned()), false));
    }

    #[test]
    fn the_403_fallback_still_applies_when_no_override_is_set() {
        let got = registration_realm(None, Some("10.2.0.3"), Some("voip_stack"), 1);
        assert_eq!(got, (Some("10.2.0.3".to_owned()), false));
    }
}
