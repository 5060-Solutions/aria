//! ICE-lite implementation (RFC 8445 simplified)
//!
//! Aria acts as an ICE-lite agent — it gathers host candidates and optionally
//! a server-reflexive candidate via STUN binding request, then includes them
//! in SDP. It responds to connectivity checks but does not drive the full
//! ICE state machine. This is sufficient for interop with most VoIP systems.

use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// STUN magic cookie (RFC 5389)
const MAGIC_COOKIE: u32 = 0x2112A442;

/// A single ICE candidate
#[derive(Debug, Clone, serde::Serialize)]
pub struct IceCandidate {
    pub foundation: String,
    pub component: u32,
    pub transport: String,
    pub priority: u32,
    pub address: String,
    pub port: u16,
    pub cand_type: String, // "host" or "srflx"
    pub rel_addr: Option<String>,
    pub rel_port: Option<u16>,
}

impl IceCandidate {
    /// Format as an SDP a=candidate line
    pub fn to_sdp_line(&self) -> String {
        let mut line = format!(
            "a=candidate:{} {} {} {} {} {} typ {}",
            self.foundation,
            self.component,
            self.transport,
            self.priority,
            self.address,
            self.port,
            self.cand_type,
        );
        if let (Some(ref ra), Some(rp)) = (&self.rel_addr, self.rel_port) {
            line.push_str(&format!(" raddr {} rport {}", ra, rp));
        }
        line
    }
}

/// Compute candidate priority per RFC 8445 section 5.1.2.1
fn compute_priority(type_pref: u32, local_pref: u32, component: u32) -> u32 {
    (type_pref << 24) | (local_pref << 8) | (256 - component)
}

/// Gather host candidate from local RTP socket
pub fn gather_host_candidate(local_addr: SocketAddr, component: u32) -> IceCandidate {
    IceCandidate {
        foundation: "1".to_string(),
        component,
        transport: "UDP".to_string(),
        priority: compute_priority(126, 65535, component),
        address: local_addr.ip().to_string(),
        port: local_addr.port(),
        cand_type: "host".to_string(),
        rel_addr: None,
        rel_port: None,
    }
}

/// Build a STUN Binding Request (RFC 5389)
fn build_stun_binding_request(transaction_id: &[u8; 12]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(20);
    // Message Type: Binding Request (0x0001)
    buf.extend_from_slice(&0x0001u16.to_be_bytes());
    // Message Length: 0 (no attributes)
    buf.extend_from_slice(&0x0000u16.to_be_bytes());
    // Magic Cookie
    buf.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    // Transaction ID (12 bytes)
    buf.extend_from_slice(transaction_id);
    buf
}

/// Parse a STUN Binding Response to extract XOR-MAPPED-ADDRESS
fn parse_stun_binding_response(data: &[u8], transaction_id: &[u8; 12]) -> Option<SocketAddr> {
    if data.len() < 20 {
        return None;
    }

    // Check message type: Binding Success Response (0x0101)
    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    if msg_type != 0x0101 {
        return None;
    }

    // Verify magic cookie
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    if cookie != MAGIC_COOKIE {
        return None;
    }

    // Verify transaction ID
    if data[8..20] != *transaction_id {
        return None;
    }

    let msg_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    let mut offset = 20;
    let end = 20 + msg_len;

    while offset + 4 <= end && offset + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let attr_start = offset + 4;

        // XOR-MAPPED-ADDRESS (0x0020) or MAPPED-ADDRESS (0x0001)
        if (attr_type == 0x0020 || attr_type == 0x0001) && attr_len >= 8 {
            let family = data[attr_start + 1];
            if family == 0x01 {
                // IPv4
                let port_raw = u16::from_be_bytes([data[attr_start + 2], data[attr_start + 3]]);
                let ip_raw = [
                    data[attr_start + 4],
                    data[attr_start + 5],
                    data[attr_start + 6],
                    data[attr_start + 7],
                ];

                let (port, ip) = if attr_type == 0x0020 {
                    // XOR with magic cookie
                    let xor_port = port_raw ^ (MAGIC_COOKIE >> 16) as u16;
                    let cookie_bytes = MAGIC_COOKIE.to_be_bytes();
                    let xor_ip = [
                        ip_raw[0] ^ cookie_bytes[0],
                        ip_raw[1] ^ cookie_bytes[1],
                        ip_raw[2] ^ cookie_bytes[2],
                        ip_raw[3] ^ cookie_bytes[3],
                    ];
                    (
                        xor_port,
                        std::net::Ipv4Addr::new(xor_ip[0], xor_ip[1], xor_ip[2], xor_ip[3]),
                    )
                } else {
                    (
                        port_raw,
                        std::net::Ipv4Addr::new(ip_raw[0], ip_raw[1], ip_raw[2], ip_raw[3]),
                    )
                };

                return Some(SocketAddr::new(std::net::IpAddr::V4(ip), port));
            }
        }

        // Pad to 4-byte boundary
        offset = attr_start + ((attr_len + 3) & !3);
    }

    None
}

/// Perform a STUN binding request to discover the server-reflexive address.
/// Returns None if the STUN server is unreachable or times out.
pub async fn stun_binding(
    socket: &UdpSocket,
    stun_server: SocketAddr,
) -> Option<SocketAddr> {
    let transaction_id: [u8; 12] = rand::random();
    let request = build_stun_binding_request(&transaction_id);

    // Send up to 3 retries
    for attempt in 0..3 {
        if socket.send_to(&request, stun_server).await.is_err() {
            continue;
        }

        let timeout = std::time::Duration::from_millis(500 * (1 << attempt));
        let mut buf = [0u8; 512];

        match tokio::time::timeout(timeout, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, _))) => {
                if let Some(addr) = parse_stun_binding_response(&buf[..len], &transaction_id) {
                    log::info!("STUN srflx address: {}", addr);
                    return Some(addr);
                }
            }
            _ => continue,
        }
    }

    log::debug!("STUN binding to {} failed after 3 attempts", stun_server);
    None
}

/// Gather all ICE candidates for a media session.
/// Returns (host_candidates, srflx_candidates).
pub async fn gather_candidates(
    local_addr: SocketAddr,
    stun_server: Option<SocketAddr>,
) -> Vec<IceCandidate> {
    let mut candidates = Vec::new();

    // Host candidate (RTP)
    candidates.push(gather_host_candidate(local_addr, 1));

    // Server-reflexive candidate via STUN
    if let Some(stun_addr) = stun_server {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await {
            if let Some(srflx) = stun_binding(&sock, stun_addr).await {
                candidates.push(IceCandidate {
                    foundation: "2".to_string(),
                    component: 1,
                    transport: "UDP".to_string(),
                    priority: compute_priority(100, 65535, 1),
                    address: srflx.ip().to_string(),
                    port: srflx.port(),
                    cand_type: "srflx".to_string(),
                    rel_addr: Some(local_addr.ip().to_string()),
                    rel_port: Some(local_addr.port()),
                });
            }
        }
    }

    candidates
}

/// Generate ICE SDP attributes for inclusion in an offer/answer
pub fn ice_sdp_attributes(candidates: &[IceCandidate], ufrag: &str, pwd: &str) -> String {
    let mut sdp = String::new();
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", pwd));
    sdp.push_str("a=ice-lite\r\n");
    for c in candidates {
        sdp.push_str(&c.to_sdp_line());
        sdp.push_str("\r\n");
    }
    sdp
}

/// Generate random ICE credentials
pub fn generate_ice_credentials() -> (String, String) {
    let ufrag = format!("{:08x}", rand::random::<u32>());
    let pwd = format!("{:024x}", rand::random::<u128>());
    (ufrag, pwd)
}

/// Handle incoming STUN binding request (connectivity check) on the RTP socket.
/// Returns a STUN Binding Success Response if the data is a valid STUN request.
pub fn handle_stun_request(data: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    if data.len() < 20 {
        return None;
    }

    let msg_type = u16::from_be_bytes([data[0], data[1]]);
    let cookie = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);

    // Must be Binding Request with magic cookie
    if msg_type != 0x0001 || cookie != MAGIC_COOKIE {
        return None;
    }

    let mut transaction_id = [0u8; 12];
    transaction_id.copy_from_slice(&data[8..20]);

    // Build Binding Success Response with XOR-MAPPED-ADDRESS
    let mut resp = Vec::with_capacity(32);
    // Message Type: Binding Success Response
    resp.extend_from_slice(&0x0101u16.to_be_bytes());

    let cookie_bytes = MAGIC_COOKIE.to_be_bytes();

    // Build XOR-MAPPED-ADDRESS attribute
    let mut attr = Vec::with_capacity(12);
    attr.push(0x00); // reserved
    attr.push(0x01); // IPv4

    let port = from.port();
    let xor_port = port ^ (MAGIC_COOKIE >> 16) as u16;
    attr.extend_from_slice(&xor_port.to_be_bytes());

    if let std::net::IpAddr::V4(ipv4) = from.ip() {
        let octets = ipv4.octets();
        attr.push(octets[0] ^ cookie_bytes[0]);
        attr.push(octets[1] ^ cookie_bytes[1]);
        attr.push(octets[2] ^ cookie_bytes[2]);
        attr.push(octets[3] ^ cookie_bytes[3]);
    } else {
        return None; // IPv6 not supported in this implementation
    }

    // Message Length = 4 (attr header) + 8 (attr value) = 12
    resp.extend_from_slice(&12u16.to_be_bytes());
    resp.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    resp.extend_from_slice(&transaction_id);

    // XOR-MAPPED-ADDRESS attribute (type 0x0020)
    resp.extend_from_slice(&0x0020u16.to_be_bytes());
    resp.extend_from_slice(&(attr.len() as u16).to_be_bytes());
    resp.extend_from_slice(&attr);

    log::debug!("Responded to STUN binding request from {}", from);
    Some(resp)
}
