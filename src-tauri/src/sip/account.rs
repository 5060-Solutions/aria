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
    /// DTLS-SRTP key exchange (RFC 5764)
    Dtls,
}

impl SrtpMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "sdes" | "optional" | "required" => SrtpMode::Sdes,
            "dtls" => SrtpMode::Dtls,
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

/// Default codec preferences (Opus > G.729 > PCMU > PCMA)
pub fn default_codec_preferences() -> Vec<CodecPreference> {
    vec![
        CodecPreference { codec: CodecType::Opus, enabled: true, priority: 1 },
        CodecPreference { codec: CodecType::G729, enabled: true, priority: 2 },
        CodecPreference { codec: CodecType::Pcmu, enabled: true, priority: 3 },
        CodecPreference { codec: CodecType::Pcma, enabled: true, priority: 4 },
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
    #[allow(dead_code)]
    pub enabled: bool,
    /// Whether to automatically record calls for this account
    pub auto_record: bool,
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

    pub fn effective_auth_username(&self) -> &str {
        self.auth_username
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.username)
    }
}
