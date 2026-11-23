//! Media session re-exports from rtp-engine with serialization support.

use std::net::{IpAddr, SocketAddr};
use tokio::net::UdpSocket;

use crate::sip::builder::{self, NegotiatedAudio};
pub use rtp_engine::{CodecType, MediaSession};
use rtp_engine::StunResult;

/// How long each STUN server gets to answer before the next is tried.
const STUN_TIMEOUT_MS: u64 = 3000;

/// The STUN servers a manager asks by default: `rtp_engine`'s public list.
pub(crate) fn default_stun_servers() -> Vec<String> {
    rtp_engine::DEFAULT_STUN_SERVERS.iter().map(|s| s.to_string()).collect()
}

/// Ask each of `servers` in turn for `socket`'s public address.
///
/// The same walk as `rtp_engine::discover_public_address`, over a list the
/// manager holds rather than the crate's constant, so the harness can point
/// its managers at a local stub instead of the public servers — a lookup
/// that timed out there cost a call three seconds and failed the tests that
/// time dialling.
async fn discover_public_address(socket: &UdpSocket, servers: &[String]) -> Result<StunResult, String> {
    let mut last_error = None;
    for server in servers {
        match rtp_engine::stun::discover_with_socket(socket, server, STUN_TIMEOUT_MS).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                log::warn!("STUN server {} failed: {}", server, e);
                last_error = Some(e.to_string());
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "All STUN servers failed".to_owned()))
}

/// Point a freshly started session at the payload types the peer's SDP
/// named, for both the audio codec and DTMF.
///
/// Audio goes on the number the peer carries the codec on — Opus on a
/// dynamic payload type other than 111 would otherwise be sent as 111 and
/// dropped. DTMF goes on the telephone-event type whose clock matches the
/// codec's (RFC 4733 §2.1), with the engine told that clock so a digit's
/// duration is scaled to it: paired with Opus, `telephone-event/48000`
/// expects 960 units per 20 ms, not 160.
pub fn apply_negotiated_payload_types(session: &MediaSession, audio: NegotiatedAudio, sdp: &str) {
    session.set_audio_payload_type(audio.pt);
    if let Some((pt, rate)) = builder::parse_telephone_event(sdp, session.codec().clock_rate()) {
        log::info!("DTMF on telephone-event payload type {} at {} Hz", pt, rate);
        session.set_dtmf_payload_type(pt);
        session.set_dtmf_clock_rate(rate);
    }
}

/// What a call's running media session was started against: the far end's
/// address, the codec, and the far end's SRTP key. A later SDP from the far
/// end that differs in any of these needs a new session — rtp-engine cannot
/// change a running one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMedia {
    pub addr: SocketAddr,
    pub audio: NegotiatedAudio,
    pub srtp_key: Option<String>,
}

impl RemoteMedia {
    /// Whether a session running against `self` must be restarted to carry
    /// media described by `next`.
    ///
    /// An unspecified address (`c=0.0.0.0`, the old way to hold) is not a
    /// new destination: the session stays where it is.
    pub fn needs_restart_for(&self, next: &RemoteMedia) -> bool {
        let moved = next.addr != self.addr && !next.addr.ip().is_unspecified() && next.addr.port() != 0;
        moved || next.audio != self.audio || next.srtp_key != self.srtp_key
    }

    /// This media as a session that came up with `srtp_active` uses it: the
    /// far end's key only when the session is encrypted.
    ///
    /// What a call records as its `remote_media` goes through this, as a
    /// re-INVITE's offer has its key cleared when it will not be answered in
    /// SRTP. Recording an offer's key for a plain-RTP answer made the first,
    /// identical, session refresh look like a new key and restart the media.
    pub fn for_session(mut self, srtp_active: bool) -> Self {
        if !srtp_active {
            self.srtp_key = None;
        }
        self
    }
}

/// Everything needed to start a call's media session.
pub struct MediaPlan {
    pub local_port: u16,
    pub remote: RemoteMedia,
    /// Our SDES key. SRTP runs only when both this and the far end's are set.
    pub local_srtp_key: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
}

/// A started session, and whether it runs encrypted.
pub struct StartedMedia {
    pub session: MediaSession,
    pub srtp_active: bool,
}

/// Start a call's media session: SRTP when both keys are there and both
/// contexts build (plain RTP otherwise, with the outcome reported in
/// `srtp_active`), audio on the negotiated payload type, and DTMF on the
/// telephone-event type `sdp` (the far end's) named.
///
/// The one place sessions are started, so the codec and payload type always
/// come from [`builder::negotiate_audio`] and the SRTP decision is the same
/// for an answer, a 183, a 200 and a re-INVITE.
pub async fn start_call_media(plan: &MediaPlan, sdp: &str) -> Result<StartedMedia, String> {
    let RemoteMedia { addr, audio, srtp_key: ref remote_key } = plan.remote;
    let contexts = match (&plan.local_srtp_key, remote_key) {
        (Some(ours), Some(theirs)) => {
            match (
                rtp_engine::srtp::SrtpContext::from_base64(ours),
                rtp_engine::srtp::SrtpContext::from_base64(theirs),
            ) {
                (Ok(tx), Ok(rx)) => Some((tx, rx)),
                (Err(e), _) | (_, Err(e)) => {
                    // Never log the keys themselves: they decrypt the call.
                    log::error!("Could not build SRTP contexts ({:?}); this call is NOT encrypted", e);
                    None
                }
            }
        }
        _ => None,
    };
    let srtp_active = contexts.is_some();
    let (input, output) = (plan.input_device.clone(), plan.output_device.clone());
    let session = match contexts {
        Some((tx, rx)) => {
            MediaSession::start_with_srtp_keys_and_devices(
                plan.local_port, addr, audio.codec, tx, rx, input, output,
            )
            .await
        }
        None => MediaSession::start_with_devices(plan.local_port, addr, audio.codec, input, output).await,
    }
    .map_err(|e| e.to_string())?;
    apply_negotiated_payload_types(&session, audio, sdp);
    Ok(StartedMedia { session, srtp_active })
}

/// The far end's media for a call, read from its SDP: where to send, what
/// codec on which payload type, and its SRTP key. `None` when the SDP has no
/// usable address or no codec in common.
///
/// `prefs` are the account's codec preferences when `sdp` is an offer we
/// answer, and `None` when it is the answer to our offer.
pub fn remote_media(
    sdp: &str,
    prefs: Option<&[crate::sip::account::CodecPreference]>,
) -> Option<RemoteMedia> {
    let (ip, port) = builder::parse_sdp_connection(sdp)?;
    let addr: SocketAddr = format!("{}:{}", ip, port).parse().ok()?;
    let audio = builder::negotiate_audio(sdp, prefs)?;
    Some(RemoteMedia { addr, audio, srtp_key: builder::parse_sdp_crypto(sdp) })
}

/// An even RTP port with its RTCP neighbour free, released for the media
/// session to bind. For a session that must move off the port the call's
/// current one holds: rtp-engine's RTCP task keeps that bound for up to five
/// seconds after the session stops.
pub async fn allocate_free_rtp_port() -> Result<u16, String> {
    allocate_rtp_socket().await.map(|(_, port)| port)
}

/// Allocate an RTP port and discover the public address via STUN.
///
/// Returns `(local_port, public_ip, public_port)`. The public half is `None`
/// when STUN fails or answers with an unspecified address — never a made-up
/// address. It used to fall back to the probe socket's own address, and that
/// socket is bound to `0.0.0.0`, so a failed STUN lookup put `c=IN IP4 0.0.0.0`
/// in the SDP. RFC 3264 reads that as "do not send me media", and a PBX that
/// follows it withholds audio for the whole call (#4).
///
/// The port is held for the whole probe. This used to be
/// `MediaSession::allocate_port()` — which binds the port, checks the RTCP
/// port and closes both — followed by a second bind of the same port for the
/// STUN probe. Anything that bound an ephemeral port in between could take
/// it, and the second bind then failed the call with "address in use" before
/// the INVITE went out (#15: the harness tests bind sockets in parallel, and
/// Windows is where the freed port came straight back). `MediaSession::start`
/// still rebinds the port when the call connects; that window is the
/// engine's, and a failure there lands on a connected call's media, not on
/// dialling.
pub async fn allocate_port_with_stun(
    stun_servers: &[String],
) -> Result<(u16, Option<IpAddr>, Option<u16>), String> {
    let (socket, rtp_port) = allocate_rtp_socket().await?;

    match discover_public_address(&socket, stun_servers).await {
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

/// How many ephemeral ports to try for an even RTP port with a free RTCP
/// port beside it before giving up. Each miss costs one bind.
const RTP_PORT_ATTEMPTS: usize = 32;

/// Bind an even ephemeral port whose odd neighbour (RTCP, RFC 3550 section
/// 11) is also free, and return the socket still bound to it with the port.
///
/// The same rules as `MediaSession::allocate_port`, but the RTP socket is
/// returned rather than closed, so the caller's use of the port cannot lose
/// it to another bind in the meantime. The RTCP check socket is closed: it
/// is only a probe, and the engine binds it itself when media starts.
async fn allocate_rtp_socket() -> Result<(UdpSocket, u16), String> {
    let mut last_error = None;
    for _ in 0..RTP_PORT_ATTEMPTS {
        let rtp_socket = UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| format!("Failed to allocate RTP port: {}", e))?;
        let rtp_port = rtp_socket
            .local_addr()
            .map_err(|e| format!("Failed to allocate RTP port: {}", e))?
            .port();
        if rtp_port % 2 != 0 {
            last_error = Some(format!("port {} is not usable for RTP", rtp_port));
            continue;
        }
        match UdpSocket::bind(format!("0.0.0.0:{}", rtp_port + 1)).await {
            Ok(_rtcp_probe) => return Ok((rtp_socket, rtp_port)),
            Err(e) => last_error = Some(format!("RTCP port {} is taken: {}", rtp_port + 1, e)),
        }
    }
    Err(format!(
        "Failed to allocate RTP port: no even port with a free RTCP port in {} attempts (last: {})",
        RTP_PORT_ATTEMPTS,
        last_error.unwrap_or_default()
    ))
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
pub async fn discover_public_ip(stun_servers: &[String]) -> Result<IpAddr, String> {
    // Use a random ephemeral port for STUN discovery
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| format!("Failed to bind socket for STUN: {}", e))?;
    
    match discover_public_address(&socket, stun_servers).await {
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
mod remote_media_tests {
    use super::{remote_media, CodecType, NegotiatedAudio, RemoteMedia};

    fn sdp(addr: &str, port: u16, pts: &str, extra: &str) -> String {
        format!("v=0\r\nc=IN IP4 {addr}\r\nm=audio {port} RTP/AVP {pts}\r\n{extra}")
    }

    /// What decides a restart: the address or port, the codec or its
    /// payload type, the far end's key. A hold to 0.0.0.0 is not a move.
    #[test]
    fn a_new_address_codec_or_key_needs_a_new_session_and_nothing_else_does() {
        let current = remote_media(&sdp("10.0.0.1", 40000, "0 8", ""), None).unwrap();
        assert_eq!(current.audio, NegotiatedAudio { codec: CodecType::Pcmu, pt: 0 });
        let same = remote_media(&sdp("10.0.0.1", 40000, "0 8", "a=sendonly\r\n"), None).unwrap();
        assert!(!current.needs_restart_for(&same));
        for moved in [sdp("10.0.0.1", 40002, "0 8", ""), sdp("10.0.0.9", 40000, "0 8", ""), sdp("10.0.0.1", 40000, "8 0", "")] {
            assert!(current.needs_restart_for(&remote_media(&moved, None).unwrap()), "{moved}");
        }
        let held = remote_media(&sdp("0.0.0.0", 40000, "0 8", ""), None).unwrap();
        assert!(!current.needs_restart_for(&held));
        let rekeyed = RemoteMedia { srtp_key: Some("k".into()), ..current.clone() };
        assert!(current.needs_restart_for(&rekeyed));
    }

    /// The far end's media from its SDP uses the same negotiation as the
    /// answer: G.722 first runs PCMU, and nothing in common is `None`.
    #[test]
    fn remote_media_negotiates_like_the_answer() {
        let yealink = sdp("10.0.0.1", 40000, "9 0 8 101", "a=rtpmap:9 G722/8000\r\n");
        let got = remote_media(&yealink, Some(&crate::sip::account::default_codec_preferences())).unwrap();
        assert_eq!(got.audio, NegotiatedAudio { codec: CodecType::Pcmu, pt: 0 });
        assert!(remote_media(&sdp("10.0.0.1", 40000, "9 18", ""), None).is_none());
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
