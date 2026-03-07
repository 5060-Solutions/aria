//! SRTP re-exports from rtp-engine.

pub use rtp_engine::srtp::{build_sdp_crypto_line, parse_sdp_crypto};

// Re-export SrtpContext for potential future use
#[allow(unused_imports)]
pub use rtp_engine::srtp::SrtpContext;
