//! Media session re-exports from rtp-engine with serialization support.

use std::net::IpAddr;
use tokio::net::UdpSocket;

pub use rtp_engine::{CodecType, MediaSession, discover_public_address};

/// Allocate an RTP port and discover the public address via STUN.
///
/// Returns (local_port, public_ip, public_port) where public_ip/port are from STUN discovery.
/// If STUN fails, falls back to local address.
pub async fn allocate_port_with_stun() -> Result<(u16, IpAddr, u16), String> {
    // First allocate a port
    let rtp_port = MediaSession::allocate_port()
        .await
        .map_err(|e| format!("Failed to allocate RTP port: {}", e))?;
    
    // Create a socket to use for STUN discovery (we'll drop it after discovery)
    let socket = UdpSocket::bind(format!("0.0.0.0:{}", rtp_port))
        .await
        .map_err(|e| format!("Failed to bind RTP socket for STUN: {}", e))?;
    
    // Try STUN discovery
    match discover_public_address(&socket).await {
        Ok(result) => {
            log::info!(
                "STUN discovery: local {}:{} -> public {}:{}",
                result.local_ip, result.local_port, result.public_ip, result.public_port
            );
            // Note: Due to symmetric NAT, the public port may change when we bind again.
            // However, we'll use the public IP which should be consistent.
            Ok((rtp_port, result.public_ip, result.public_port))
        }
        Err(e) => {
            log::warn!("STUN discovery failed (will use local IP): {}", e);
            let local_addr = socket.local_addr()
                .map_err(|e| format!("Failed to get local address: {}", e))?;
            Ok((rtp_port, local_addr.ip(), rtp_port))
        }
    }
}

/// Discover just the public IP address via STUN (without allocating a specific port).
///
/// This is useful when you already have an RTP port allocated and just need the public IP.
/// The public IP should be the same regardless of which port is used for STUN.
pub async fn discover_public_ip() -> Result<IpAddr, String> {
    // Use a random ephemeral port for STUN discovery
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Failed to bind socket for STUN: {}", e))?;
    
    match discover_public_address(&socket).await {
        Ok(result) => {
            log::info!("STUN discovered public IP: {}", result.public_ip);
            Ok(result.public_ip)
        }
        Err(e) => {
            Err(format!("STUN discovery failed: {}", e))
        }
    }
}

/// RTP/RTCP statistics with serde serialization support.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RtpStats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub packets_lost: u64,
    pub jitter_ms: f64,
    pub codec_name: String,
}

impl From<rtp_engine::RtpStats> for RtpStats {
    fn from(s: rtp_engine::RtpStats) -> Self {
        Self {
            packets_sent: s.packets_sent,
            packets_received: s.packets_received,
            bytes_sent: s.bytes_sent,
            bytes_received: s.bytes_received,
            packets_lost: s.packets_lost,
            jitter_ms: s.jitter_ms,
            codec_name: s.codec_name,
        }
    }
}

/// Extension trait to provide convenient accessors with serde-compatible types.
pub trait MediaSessionExt {
    fn get_stats(&self) -> RtpStats;
    fn get_codec(&self) -> CodecType;
}

impl MediaSessionExt for MediaSession {
    fn get_stats(&self) -> RtpStats {
        self.stats().into()
    }

    fn get_codec(&self) -> CodecType {
        self.codec()
    }
}
