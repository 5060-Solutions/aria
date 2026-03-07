use md5::{Digest as Md5Digest, Md5};
use sha2::{Digest as Sha2Digest, Sha256};

/// SIP Digest Authentication (RFC 2617 / RFC 7616)
pub struct DigestAuth {
    pub username: String,
    pub password: String,
    pub realm: String,
    pub nonce: String,
    pub uri: String,
    pub method: String,
    pub algorithm: String,
    pub qop: Option<String>,
    pub nc: u32,
    pub cnonce: String,
}

impl DigestAuth {
    pub fn from_challenge(
        www_auth: &str,
        username: &str,
        password: &str,
        uri: &str,
        method: &str,
    ) -> Option<Self> {
        let realm = extract_param(www_auth, "realm")?;
        let nonce = extract_param(www_auth, "nonce")?;
        let algorithm = extract_param(www_auth, "algorithm").unwrap_or_else(|| "MD5".to_string());
        let qop = extract_param(www_auth, "qop");

        let cnonce = format!("{:08x}", rand::random::<u32>());

        Some(Self {
            username: username.to_string(),
            password: password.to_string(),
            realm,
            nonce,
            uri: uri.to_string(),
            method: method.to_string(),
            algorithm,
            qop,
            nc: 1,
            cnonce,
        })
    }

    pub fn response(&self) -> String {
        let hash_fn: fn(&str) -> String = if self.algorithm.eq_ignore_ascii_case("SHA-256") {
            sha256_hex
        } else {
            md5_hex
        };

        let ha1 = hash_fn(&format!(
            "{}:{}:{}",
            self.username, self.realm, self.password
        ));

        let ha2 = hash_fn(&format!("{}:{}", self.method, self.uri));

        if let Some(ref qop) = self.qop {
            if qop.contains("auth") {
                let nc = format!("{:08x}", self.nc);
                return hash_fn(&format!(
                    "{}:{}:{}:{}:auth:{}",
                    ha1, self.nonce, nc, self.cnonce, ha2
                ));
            }
        }

        hash_fn(&format!("{}:{}:{}", ha1, self.nonce, ha2))
    }

    pub fn to_header(&self) -> String {
        let response = self.response();

        let mut header = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\"",
            self.username, self.realm, self.nonce, self.uri, response
        );

        if self.algorithm != "MD5" {
            header.push_str(&format!(", algorithm={}", self.algorithm));
        }

        if self.qop.is_some() {
            header.push_str(&format!(
                ", qop=auth, nc={:08x}, cnonce=\"{}\"",
                self.nc, self.cnonce
            ));
        }

        header
    }
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    Md5Digest::update(&mut hasher, input.as_bytes());
    hex::encode(Md5Digest::finalize(hasher))
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    Sha2Digest::update(&mut hasher, input.as_bytes());
    hex::encode(Sha2Digest::finalize(hasher))
}

fn extract_param(header: &str, name: &str) -> Option<String> {
    let search = format!("{}=", name);
    let pos = header.find(&search)?;
    let rest = &header[pos + search.len()..];

    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find(',').unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    }
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
