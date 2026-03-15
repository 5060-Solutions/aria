// Re-export digest auth from the shared crate
#[allow(unused_imports)]
pub use aria_sip_core::auth::{extract_challenge_realm, extract_param, DigestAuth};

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
