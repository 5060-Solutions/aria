use super::transport::TransportType;
use rtp_engine::CodecType;

/// SRTP mode for media encryption
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SrtpMode {
    /// No SRTP - plain RTP only
    #[default]
    Disabled,
    /// SDES-SRTP key exchange (RFC 4568)
    Sdes,
}

impl SrtpMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "sdes" | "optional" | "required" => SrtpMode::Sdes,
            // DTLS-SRTP was offered as a setting but never implemented: it sent
            // plain RTP. Stored accounts that still say "dtls" keep that
            // behaviour rather than silently changing what goes on the wire.
            "dtls" => {
                log::warn!("srtp mode \"dtls\" is no longer offered; treating as disabled");
                SrtpMode::Disabled
            }
            _ => SrtpMode::Disabled,
        }
    }

}

/// Codec preference with enable/disable and priority
#[derive(Debug, Clone)]
pub struct CodecPreference {
    pub codec: CodecType,
    pub enabled: bool,
    pub priority: u8,
}

impl Default for CodecPreference {
    fn default() -> Self {
        Self {
            codec: CodecType::Pcmu,
            enabled: true,
            priority: 1,
        }
    }
}

/// Default codec preferences (Opus > PCMU > PCMA).
///
/// No G.729: rtp-engine's G.729 is not ITU-T G.729 compatible, so it is not
/// offered (see `SUPPORTED_CODECS` in `sip::builder`).
pub fn default_codec_preferences() -> Vec<CodecPreference> {
    vec![
        CodecPreference { codec: CodecType::Opus, enabled: true, priority: 1 },
        CodecPreference { codec: CodecType::Pcmu, enabled: true, priority: 2 },
        CodecPreference { codec: CodecType::Pcma, enabled: true, priority: 3 },
    ]
}

#[derive(Debug, Clone)]
pub struct AccountConfig {
    #[allow(dead_code)]
    pub id: String,
    pub display_name: String,
    pub username: String,
    pub domain: String,
    pub password: String,
    pub transport: TransportType,
    pub port: u16,
    pub registrar: Option<String>,
    #[allow(dead_code)]
    pub outbound_proxy: Option<String>,
    pub auth_username: Option<String>,
    /// Override realm for digest authentication.
    /// When set, this realm is used instead of the one from the server's challenge.
    /// Required for some FreeSwitch deployments where the challenge realm
    /// doesn't match the realm used for credential verification.
    pub auth_realm: Option<String>,
    /// The name the TLS certificate is checked against, when it is neither the
    /// registrar nor the domain — typically an outbound proxy's host. Set by
    /// setup links; `None` for accounts set up by hand.
    pub tls_server_name: Option<String>,
    #[allow(dead_code)]
    pub enabled: bool,
    /// Whether to automatically record calls for this account
    pub auto_record: bool,
    /// Disable TLS certificate/hostname verification for this account.
    ///
    /// Defaults to `false`. Only for deployments with a private PBX certificate
    /// that cannot be installed into the OS trust store — with this enabled an
    /// on-path attacker can read the digest credentials and SDES-SRTP keys.
    /// Never settable via QR provisioning.
    pub tls_insecure: bool,
    /// SRTP mode for media encryption
    pub srtp_mode: SrtpMode,
    /// Codec preferences in priority order
    pub codecs: Vec<CodecPreference>,
}

impl AccountConfig {
    #[allow(dead_code)]
    pub fn sip_uri(&self) -> String {
        format!("sip:{}@{}", self.username, self.domain)
    }

    /// The TLS server name: `tls_server_name`, else the registrar, else the
    /// domain.
    pub fn tls_name(&self) -> &str {
        [self.tls_server_name.as_deref(), self.registrar.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|s| !s.is_empty())
            .unwrap_or(&self.domain)
    }

    /// The name the server's certificate is checked against when connecting to
    /// a target found by DNS.
    ///
    /// An explicit `tls_server_name` always wins. When the target came from SRV
    /// the certificate is for the name that was looked up, not for whatever
    /// host the SRV record points at (RFC 5922 §7.2), so that name is used
    /// while the connection goes to the target's IP. Otherwise the usual:
    /// registrar, then domain.
    pub fn tls_name_for<'a>(&'a self, via_srv: bool, lookup_host: &'a str) -> &'a str {
        if let Some(explicit) = self.tls_server_name.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            return explicit;
        }
        if via_srv && !lookup_host.is_empty() {
            return lookup_host;
        }
        self.tls_name()
    }

    pub fn effective_auth_username(&self) -> &str {
        self.auth_username
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.username)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AccountConfig {
        AccountConfig {
            id: "a".into(),
            display_name: String::new(),
            username: "1001".into(),
            domain: "tenant.example.com".into(),
            password: String::new(),
            transport: TransportType::Tls,
            port: 5061,
            registrar: None,
            outbound_proxy: None,
            auth_username: None,
            auth_realm: None,
            tls_server_name: None,
            enabled: true,
            auto_record: false,
            tls_insecure: false,
            srtp_mode: SrtpMode::Disabled,
            codecs: default_codec_preferences(),
        }
    }

    #[test]
    fn tls_name_prefers_the_explicit_name_then_registrar_then_domain() {
        let mut c = config();
        assert_eq!(c.tls_name(), "tenant.example.com");
        c.registrar = Some("pbx.example.com".into());
        assert_eq!(c.tls_name(), "pbx.example.com");
        c.tls_server_name = Some("sip.example.com".into());
        assert_eq!(c.tls_name(), "sip.example.com");
        // Blank values are skipped, as an unset registrar always has been.
        c.tls_server_name = Some("  ".into());
        assert_eq!(c.tls_name(), "pbx.example.com");
        c.registrar = None;
        assert_eq!(c.tls_name(), "tenant.example.com");
    }

    #[test]
    fn tls_name_for_srv_targets_is_the_looked_up_name() {
        let mut c = config();
        c.registrar = Some("pbx.example.com".into());
        // Without SRV nothing changes.
        assert_eq!(c.tls_name_for(false, "pbx.example.com"), "pbx.example.com");
        // With SRV the certificate is checked against the SIP domain that was
        // looked up, never the SRV target's own host name.
        assert_eq!(c.tls_name_for(true, "tenant.example.com"), "tenant.example.com");
        // An explicit name still wins, with or without SRV.
        c.tls_server_name = Some("sip.example.com".into());
        assert_eq!(c.tls_name_for(true, "tenant.example.com"), "sip.example.com");
        assert_eq!(c.tls_name_for(false, "tenant.example.com"), "sip.example.com");
        // A blank explicit name is no name.
        c.tls_server_name = Some(" ".into());
        assert_eq!(c.tls_name_for(true, "tenant.example.com"), "tenant.example.com");
    }
}
