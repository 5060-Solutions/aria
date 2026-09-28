//! Media session re-exports from rtp-engine with serialization support.

use std::net::IpAddr;
use tokio::net::UdpSocket;

pub use rtp_engine::{CodecType, MediaSession, discover_public_address};

/// Allocate an RTP port and discover the public address via STUN.
///
/// Returns `(local_port, public_ip, public_port)`. The public half is `None`
/// when STUN fails or answers with an unspecified address — never a made-up
/// address. It used to fall back to the probe socket's own address, and that
/// socket is bound to `0.0.0.0`, so a failed STUN lookup put `c=IN IP4 0.0.0.0`
/// in the SDP. RFC 3264 reads that as "do not send me media", and a PBX that
/// follows it withholds audio for the whole call (#4).
pub async fn allocate_port_with_stun() -> Result<(u16, Option<IpAddr>, Option<u16>), String> {
    let rtp_port = MediaSession::allocate_port()
        .await
        .map_err(|e| format!("Failed to allocate RTP port: {}", e))?;

    // A throwaway socket on the same port, for the STUN probe only.
    let socket = UdpSocket::bind(format!("0.0.0.0:{}", rtp_port))
        .await
        .map_err(|e| format!("Failed to bind RTP socket for STUN: {}", e))?;

    match discover_public_address(&socket).await {
        Ok(result) if !result.public_ip.is_unspecified() => {
            log::info!(
                "STUN discovery: local {}:{} -> public {}:{}",
                result.local_ip, result.local_port, result.public_ip, result.public_port
            );
            // Symmetric NAT may change the port when the media socket binds
            // again; the public IP is what the SDP uses.
            Ok((rtp_port, Some(result.public_ip), Some(result.public_port)))
        }
        Ok(result) => {
            log::warn!("STUN answered with an unspecified address ({}); ignoring it", result.public_ip);
            Ok((rtp_port, None, None))
        }
        Err(e) => {
            log::warn!("STUN discovery failed; the SDP will carry the local interface address: {}", e);
            Ok((rtp_port, None, None))
        }
    }
}

/// The address to advertise for media (the SDP `c=` and `o=` lines).
///
/// `local` is the interface address that reaches the SIP server; `public` is
/// what STUN or the registrar reported we look like from outside.
///
/// A server on a private network is reached directly, so it must be given the
/// local address: a STUN-discovered public address would have it send RTP out
/// to the internet and back, which most NATs drop. Only a server on a public
/// address is given the public one. An unspecified address is never returned
/// while a real one is known — `0.0.0.0` means "hold" to the far end.
pub fn sdp_address(local: IpAddr, public: Option<IpAddr>, server: Option<IpAddr>) -> IpAddr {
    let public = public.filter(|ip| !ip.is_unspecified());
    if server.is_some_and(is_private_network) {
        return if local.is_unspecified() { public.unwrap_or(local) } else { local };
    }
    match public {
        Some(ip) => ip,
        None => local,
    }
}

/// Whether `ip` is only reachable inside a private network: RFC 1918, loopback,
/// link-local, carrier-grade NAT (100.64/10 — also Tailscale), or IPv6
/// unique-local / link-local.
pub fn is_private_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || (a == 100 && (64..=127).contains(&b))
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
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

#[cfg(test)]
mod sdp_address_tests {
    use super::{is_private_network, sdp_address};
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// The reported case (#4): a LAN PBX, and STUN failed. The SDP must carry
    /// the interface address, never 0.0.0.0.
    #[test]
    fn a_lan_server_gets_the_local_address_when_stun_failed() {
        let got = sdp_address(ip("10.2.0.124"), None, Some(ip("10.2.0.3")));
        assert_eq!(got, ip("10.2.0.124"));
    }

    /// Had STUN succeeded, a LAN PBX would have been told the WAN address and
    /// sent RTP out to the internet and back. It gets the local one.
    #[test]
    fn a_lan_server_gets_the_local_address_even_when_stun_succeeded() {
        let got = sdp_address(ip("192.168.1.20"), Some(ip("203.0.113.7")), Some(ip("192.168.1.1")));
        assert_eq!(got, ip("192.168.1.20"));
    }

    #[test]
    fn a_public_server_behind_nat_gets_the_public_address() {
        let got = sdp_address(ip("192.168.1.20"), Some(ip("203.0.113.7")), Some(ip("159.203.80.231")));
        assert_eq!(got, ip("203.0.113.7"));
    }

    #[test]
    fn a_public_server_with_no_public_address_known_gets_the_local_one() {
        let got = sdp_address(ip("192.168.1.20"), None, Some(ip("159.203.80.231")));
        assert_eq!(got, ip("192.168.1.20"));
    }

    /// An unspecified "public" address is discarded, not advertised.
    #[test]
    fn an_unspecified_public_address_is_never_advertised() {
        for server in [Some(ip("159.203.80.231")), Some(ip("10.0.0.1")), None] {
            let got = sdp_address(ip("192.168.1.20"), Some(ip("0.0.0.0")), server);
            assert_eq!(got, ip("192.168.1.20"), "server {server:?}");
        }
    }

    #[test]
    fn private_networks_are_recognised() {
        for p in ["10.2.0.3", "172.16.5.5", "192.168.0.1", "127.0.0.1", "169.254.1.1", "100.64.0.1", "100.127.255.1", "fd00::1", "fe80::1", "::1"] {
            assert!(is_private_network(ip(p)), "{p} should be private");
        }
        for p in ["159.203.80.231", "8.8.8.8", "100.128.0.1", "172.32.0.1", "2001:db8::1", "2606:4700::1111"] {
            assert!(!is_private_network(ip(p)), "{p} should be public");
        }
    }
}
