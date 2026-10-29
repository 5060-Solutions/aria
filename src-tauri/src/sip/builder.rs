use crate::sip::account::{AccountConfig, SrtpMode};
use crate::sip::presence::EventType;
use rsip::headers::UntypedHeader;
use std::net::SocketAddr;

// Re-export shared utilities so existing callers keep working
pub use rsip::generators::{generate_branch, generate_call_id, generate_tag};
pub use rsip::sip_parser::{
    extract_all_headers, extract_from_tag, extract_header, extract_method, extract_to_tag,
    extract_via_branch, extract_via_received, is_request, parse_replaces_header,
    parse_sdp_connection, parse_sipfrag_status, parse_status_code,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// From Cargo.toml, so it cannot fall behind the release the way the literal
// "Aria/0.2.0" did.
pub const USER_AGENT: &str = concat!("Aria/", env!("CARGO_PKG_VERSION"));
// Only methods the request handler actually answers. MESSAGE is not one.
const ALLOW: &str = "INVITE, ACK, CANCEL, BYE, NOTIFY, REFER, OPTIONS, INFO, SUBSCRIBE";

/// The URI from a Contact header value: the part inside `<...>` when it has
/// one, otherwise the whole value. This is the dialog's remote target
/// (RFC 3261 §12.1), where in-dialog requests are addressed.
pub fn contact_uri(contact: &str) -> String {
    let contact = contact.trim();
    contact
        .find('<')
        .and_then(|start| {
            let rest = &contact[start + 1..];
            rest.find('>').map(|end| rest[..end].to_string())
        })
        .unwrap_or_else(|| contact.to_string())
}

/// Build an `rsip::Request` from parts and convert to `String`.
fn request_to_string(
    method: rsip::Method,
    uri: rsip::Uri,
    headers: rsip::Headers,
    body: Vec<u8>,
) -> String {
    let req = rsip::Request {
        method,
        uri,
        version: rsip::Version::V2,
        headers,
        body,
    };
    req.to_string()
}

/// Build an `rsip::Response` from parts and convert to `String`.
fn response_to_string(
    status_code: rsip::StatusCode,
    headers: rsip::Headers,
    body: Vec<u8>,
) -> String {
    let resp = rsip::Response {
        status_code,
        version: rsip::Version::V2,
        headers,
        body,
    };
    resp.to_string()
}

/// Parse a SIP URI string like `sip:user@host:port` into an `rsip::Uri`.
/// Falls back to treating the whole string as a domain if parsing fails.
fn parse_uri(s: &str) -> rsip::Uri {
    use std::convert::TryFrom;
    rsip::Uri::try_from(s).unwrap_or_else(|_| {
        rsip::Uri {
            scheme: Some(rsip::Scheme::Sip),
            host_with_port: rsip::Domain::from(s).into(),
            ..Default::default()
        }
    })
}

/// Build a Via header value string for our outgoing messages.
fn via_value(transport: &str, local_addr: SocketAddr, branch: &str) -> String {
    format!(
        "SIP/2.0/{} {}:{};branch={};rport",
        transport.to_uppercase(),
        local_addr.ip(),
        local_addr.port(),
        branch,
    )
}

/// Create common headers shared by many outgoing requests.
fn base_request_headers(
    via: &str,
    from: &str,
    to: &str,
    call_id: &str,
    cseq_val: &str,
) -> rsip::Headers {
    let mut headers = rsip::Headers::default();
    headers.push(rsip::headers::Via::new(via).into());
    headers.push(rsip::Header::MaxForwards(rsip::headers::MaxForwards::new("70")));
    headers.push(rsip::headers::From::new(from).into());
    headers.push(rsip::headers::To::new(to).into());
    headers.push(rsip::headers::CallId::new(call_id).into());
    headers.push(rsip::headers::CSeq::new(cseq_val).into());
    headers
}

// ---------------------------------------------------------------------------
// REGISTER
// ---------------------------------------------------------------------------

/// Build a REGISTER request
#[allow(clippy::too_many_arguments)]
pub fn build_register(
    account: &AccountConfig,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    auth_header: Option<&str>,
    expires: u32,
) -> String {
    let registrar = account.registrar.as_deref().unwrap_or(&account.domain);
    let transport_param = account.transport.param();
    let branch = generate_branch();

    let request_uri = rsip::Uri {
        scheme: Some(rsip::Scheme::Sip),
        host_with_port: rsip::Domain::from(registrar).into(),
        ..Default::default()
    };

    let via = via_value(transport_param, local_addr, &branch);
    let from_hdr = format!("<sip:{}@{}>;tag={}", account.username, account.domain, from_tag);
    let to_hdr = format!("<sip:{}@{}>", account.username, account.domain);
    let cseq_hdr = format!("{} REGISTER", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);

    let contact = format!(
        "<sip:{}@{}:{};transport={}>",
        account.username, local_addr.ip(), local_addr.port(), transport_param,
    );
    headers.push(rsip::headers::Contact::new(contact).into());
    headers.push(rsip::headers::Expires::new(expires.to_string()).into());
    headers.push(rsip::headers::Allow::new(ALLOW).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());

    if let Some(auth) = auth_header {
        headers.push(rsip::headers::Authorization::new(auth).into());
    }

    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Register, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// Authorization header type
// ---------------------------------------------------------------------------

/// Authorization header type for SIP requests
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AuthHeaderType {
    /// Authorization header (for 401 WWW-Authenticate challenges)
    Authorization,
    /// Proxy-Authorization header (for 407 Proxy-Authenticate challenges)
    ProxyAuthorization,
}

// ---------------------------------------------------------------------------
// INVITE
// ---------------------------------------------------------------------------

/// The address an offer or answer advertises: `public_ip` when it is a real
/// address, else the local one.
///
/// Last line of defence for #4: an unspecified address in the SDP tells the far
/// end not to send media (RFC 3264), so one arriving here is discarded and
/// logged rather than advertised.
fn sdp_ip(local_addr: SocketAddr, public_ip: Option<&str>) -> String {
    match public_ip.map(str::parse::<std::net::IpAddr>) {
        Some(Ok(ip)) if !ip.is_unspecified() => ip.to_string(),
        Some(Ok(ip)) => {
            log::error!("refusing to advertise {} for media; using the local address", ip);
            local_addr.ip().to_string()
        }
        Some(Err(_)) => public_ip.unwrap_or_default().to_string(),
        None => local_addr.ip().to_string(),
    }
}

/// Build an INVITE request with SDP, optionally using a public IP for NAT traversal.
/// Returns (invite_message, local_srtp_key) where local_srtp_key is Some if SRTP is enabled.
#[allow(clippy::too_many_arguments)]
pub fn build_invite_with_public_ip(
    account: &AccountConfig,
    target_uri: &str,
    local_addr: SocketAddr,
    rtp_port: u16,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    auth: Option<(&str, AuthHeaderType)>,
    public_ip: Option<&str>,
) -> (String, Option<String>) {
    let transport_param = account.transport.param();
    let branch = generate_branch();

    // Use public IP for SDP if available (for NAT traversal), otherwise use local IP
    let sdp_ip = sdp_ip(local_addr, public_ip);

    // Generate SRTP key if account has SRTP enabled
    log::info!("Building INVITE for account {} with SRTP mode: {:?}", account.username, account.srtp_mode);
    let crypto_key = match account.srtp_mode {
        SrtpMode::Sdes => {
            match rtp_engine::srtp::SrtpContext::generate() {
                Ok((_, key)) => {
                    log::info!("Generated SRTP key for call");
                    Some(key)
                }
                Err(e) => {
                    log::error!("Failed to generate SRTP key: {:?}", e);
                    None
                }
            }
        }
        SrtpMode::Disabled => {
            log::info!("SRTP disabled for this account");
            None
        }
    };
    let sdp = build_sdp_offer_with_codecs(sdp_ip, rtp_port, crypto_key.as_deref(), &account.codecs);

    let request_uri = parse_uri(target_uri);

    let via = via_value(transport_param, local_addr, &branch);
    let from_hdr = if account.display_name.trim().is_empty() {
        format!("<sip:{}@{}>;tag={}", account.username, account.domain, from_tag)
    } else {
        format!("\"{}\" <sip:{}@{}>;tag={}", account.display_name, account.username, account.domain, from_tag)
    };
    let to_hdr = format!("<{}>", target_uri);
    let cseq_hdr = format!("{} INVITE", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);

    let contact = format!(
        "<sip:{}@{}:{};transport={}>",
        account.username, local_addr.ip(), local_addr.port(), transport_param,
    );
    headers.push(rsip::headers::Contact::new(contact).into());
    headers.push(rsip::headers::ContentType::new("application/sdp").into());
    headers.push(rsip::headers::Allow::new(ALLOW).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());

    if let Some((auth_value, auth_type)) = auth {
        match auth_type {
            AuthHeaderType::Authorization => {
                headers.push(rsip::headers::Authorization::new(auth_value).into());
            }
            AuthHeaderType::ProxyAuthorization => {
                headers.push(rsip::headers::ProxyAuthorization::new(auth_value).into());
            }
        }
    }

    headers.push(rsip::headers::ContentLength::new(sdp.len().to_string()).into());

    let msg = request_to_string(rsip::Method::Invite, request_uri, headers, sdp.into_bytes());
    (msg, crypto_key)
}

// ---------------------------------------------------------------------------
// ACK
// ---------------------------------------------------------------------------

/// Build an ACK request
#[allow(clippy::too_many_arguments)]
pub fn build_ack(
    target_uri: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: &str,
    transport: &str,
    via_branch: &str,
    from_uri: &str,
    to_uri: &str,
) -> String {
    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, via_branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>;tag={}", to_uri, to_tag);
    let cseq_hdr = format!("{} ACK", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Ack, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// BYE
// ---------------------------------------------------------------------------

/// Build a BYE request
#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_bye(
    target_uri: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: &str,
    transport: &str,
    from_uri: &str,
    to_uri: &str,
) -> String {
    let branch = generate_branch();
    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, &branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>;tag={}", to_uri, to_tag);
    let cseq_hdr = format!("{} BYE", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);
    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Bye, request_uri, headers, vec![])
}

/// Build a BYE request with Route headers
#[allow(clippy::too_many_arguments)]
pub fn build_bye_with_routes(
    target_uri: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: &str,
    transport: &str,
    route_set: &[String],
    from_uri: &str,
    to_uri: &str,
) -> String {
    let branch = generate_branch();
    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, &branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>;tag={}", to_uri, to_tag);
    let cseq_hdr = format!("{} BYE", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());

    for route in route_set {
        headers.push(rsip::headers::Route::new(route.as_str()).into());
    }

    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Bye, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// CANCEL
// ---------------------------------------------------------------------------

/// Build a CANCEL request
#[allow(clippy::too_many_arguments)]
pub fn build_cancel(
    target_uri: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    transport: &str,
    via_branch: &str,
    from_uri: &str,
    to_uri: &str,
) -> String {
    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, via_branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>", to_uri);
    let cseq_hdr = format!("{} CANCEL", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);
    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Cancel, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// 200 OK (INVITE)
// ---------------------------------------------------------------------------

/// Build a 200 OK response for an incoming INVITE
#[allow(dead_code)]
pub fn build_200_ok_invite(
    request_raw: &str,
    local_addr: SocketAddr,
    rtp_port: u16,
    to_tag: &str,
) -> Option<String> {
    build_200_ok_invite_with_user(request_raw, local_addr, rtp_port, to_tag, None)
}

/// Build a 200 OK response for an incoming INVITE with custom username in Contact
#[allow(clippy::too_many_arguments)]
pub fn build_200_ok_invite_with_user(
    request_raw: &str,
    local_addr: SocketAddr,
    rtp_port: u16,
    to_tag: &str,
    username: Option<&str>,
) -> Option<String> {
    build_200_ok_invite_with_public_ip(request_raw, local_addr, rtp_port, to_tag, username, None)
}

/// Build a 200 OK response for an incoming INVITE with optional public IP for SDP
#[allow(clippy::too_many_arguments)]
pub fn build_200_ok_invite_with_public_ip(
    request_raw: &str,
    local_addr: SocketAddr,
    rtp_port: u16,
    to_tag: &str,
    username: Option<&str>,
    public_ip: Option<&str>,
) -> Option<String> {
    build_200_ok_invite_srtp(request_raw, local_addr, rtp_port, to_tag, username, public_ip, None)
}

/// Build a 200 OK for an incoming INVITE, answering SRTP when `crypto_b64_key`
/// is given and the offer was RTP/SAVP.
///
/// The offer-less case still produces an offer. The plain-RTP case is this
/// with `crypto_b64_key = None`.
#[allow(clippy::too_many_arguments)]
pub fn build_200_ok_invite_srtp(
    request_raw: &str,
    local_addr: SocketAddr,
    rtp_port: u16,
    to_tag: &str,
    username: Option<&str>,
    public_ip: Option<&str>,
    crypto_b64_key: Option<&str>,
) -> Option<String> {
    let via = extract_header(request_raw, "Via")?;
    let from = extract_header(request_raw, "From")?;
    let to_base = extract_header(request_raw, "To")?;
    let call_id = extract_header(request_raw, "Call-ID")?;
    let cseq = extract_header(request_raw, "CSeq")?;
    let user = username.unwrap_or("aria");
    let contact_uri = format!("sip:{}@{}:{}", user, local_addr.ip(), local_addr.port());

    let to = if to_base.contains("tag=") {
        to_base.clone()
    } else {
        format!("{};tag={}", to_base, to_tag)
    };

    // Use public IP for SDP if available (for NAT traversal), otherwise use local IP
    let sdp_ip = sdp_ip(local_addr, public_ip);

    // If the request has SDP, generate an answer; otherwise generate an offer
    let remote_sdp = request_raw.split("\r\n\r\n").nth(1).unwrap_or("");
    let sdp = if remote_sdp.contains("m=audio") {
        build_sdp_answer_srtp(remote_sdp, sdp_ip, rtp_port, crypto_b64_key)
    } else {
        build_sdp_offer(sdp_ip, rtp_port)
    };

    let mut headers = rsip::Headers::default();
    headers.push(rsip::headers::Via::new(via).into());
    headers.push(rsip::headers::From::new(from).into());
    headers.push(rsip::headers::To::new(to).into());
    headers.push(rsip::headers::CallId::new(call_id).into());
    headers.push(rsip::headers::CSeq::new(cseq).into());
    headers.push(rsip::headers::Contact::new(format!("<{}>", contact_uri)).into());
    headers.push(rsip::headers::ContentType::new("application/sdp").into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new(sdp.len().to_string()).into());

    Some(response_to_string(200.into(), headers, sdp.into_bytes()))
}

// ---------------------------------------------------------------------------
// SDP helpers (not SIP -- kept as-is)
// ---------------------------------------------------------------------------

/// The payload type this side offers `telephone-event/8000` on (RFC 4733),
/// the near-universal convention for DTMF alongside the 8 kHz codecs.
pub const TELEPHONE_EVENT_PT_8K: u8 = 101;

/// The payload type this side offers `telephone-event/48000` on. RFC 4733
/// §2.1 has the event's clock match the audio codec's, so an Opus call (a
/// 48 kHz clock) is offered DTMF at 48 kHz as well as the classic 8 kHz.
pub const TELEPHONE_EVENT_PT_48K: u8 = 102;

/// One `a=rtpmap` entry from an SDP: payload type, lower-cased encoding name
/// and clock rate.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RtpMap {
    pt: u8,
    name: String,
    rate: u32,
}

/// Every `a=rtpmap` line in the SDP, in order.
fn parse_rtpmaps(sdp: &str) -> Vec<RtpMap> {
    sdp.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("a=rtpmap:")?;
            let (pt, encoding) = rest.split_once(char::is_whitespace)?;
            let pt = pt.trim().parse().ok()?;
            let mut parts = encoding.trim().split('/');
            let name = parts.next()?.to_ascii_lowercase();
            let rate = parts.next()?.trim().parse().ok()?;
            Some(RtpMap { pt, name, rate })
        })
        .collect()
}

/// What an offered payload type means, once its rtpmap (or, lacking one,
/// the static assignment) has been read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfferedPayload {
    Codec(rtp_engine::CodecType),
    TelephoneEvent(u32),
}

/// Classify one offered payload type. An `a=rtpmap` line is authoritative;
/// without one the static numbers apply, plus the conventional dynamic ones
/// (111 for Opus, 101 for telephone-event/8000).
fn classify_payload(pt: u8, rtpmaps: &[RtpMap]) -> Option<OfferedPayload> {
    use rtp_engine::CodecType;
    match rtpmaps.iter().find(|m| m.pt == pt) {
        Some(map) if map.name == "telephone-event" => Some(OfferedPayload::TelephoneEvent(map.rate)),
        Some(map) => CodecType::from_name(&map.name)
            .filter(|codec| codec.clock_rate() == map.rate)
            .filter(|codec| SUPPORTED_CODECS.contains(codec))
            .map(OfferedPayload::Codec),
        None if pt == TELEPHONE_EVENT_PT_8K => Some(OfferedPayload::TelephoneEvent(8000)),
        None => CodecType::from_payload_type(pt)
            .filter(|codec| SUPPORTED_CODECS.contains(codec))
            .map(OfferedPayload::Codec),
    }
}

/// The codecs this softphone offers and answers with. Named rather than
/// "whatever rtp-engine knows", so a codec added to rtp-engine is not
/// negotiated until the softphone chooses to carry it.
///
/// No G.729: rtp-engine's G.729 is not ITU-T G.729 compatible (its own
/// bitstream on G.729's framing), so a real G.729 peer would hear noise both
/// ways. It comes back only with a conformant implementation.
const SUPPORTED_CODECS: [rtp_engine::CodecType; 3] = [
    rtp_engine::CodecType::Pcmu,
    rtp_engine::CodecType::Pcma,
    rtp_engine::CodecType::Opus,
];

/// The `a=rtpmap` (and `a=fmtp`) lines for one codec on one payload type.
///
/// Keyed on the codec's SDP name rather than an exhaustive match on
/// `CodecType`, so a codec added to rtp-engine gets a correct `name/clock`
/// line instead of breaking the build.
fn codec_rtpmap(pt: u8, codec: rtp_engine::CodecType) -> String {
    match codec.name() {
        "opus" => {
            format!("a=rtpmap:{pt} opus/48000/2\r\na=fmtp:{pt} minptime=10;useinbandfec=1\r\n")
        }
        "G729" => format!("a=rtpmap:{pt} G729/8000\r\na=fmtp:{pt} annexb=no\r\n"),
        name => format!("a=rtpmap:{pt} {name}/{}\r\n", codec.clock_rate()),
    }
}

/// The `a=rtpmap` and `a=fmtp` lines for telephone-event at `rate` on `pt`.
fn telephone_event_rtpmap(pt: u8, rate: u32) -> String {
    format!("a=rtpmap:{pt} telephone-event/{rate}\r\na=fmtp:{pt} 0-16\r\n")
}

/// Build an SDP answer that only includes codecs from the offer that we support
#[allow(dead_code)]
pub fn build_sdp_answer(remote_sdp: &str, ip: String, rtp_port: u16) -> String {
    build_sdp_answer_srtp(remote_sdp, ip, rtp_port, None)
}

/// Build an SDP answer with optional SRTP crypto attribute.
///
/// If the remote offer contains an `a=crypto` line with AES_CM_128_HMAC_SHA1_80
/// and `crypto_b64_key` is provided, the answer echoes back the crypto suite
/// with our own keying material and uses RTP/SAVP.
pub fn build_sdp_answer_srtp(
    remote_sdp: &str,
    ip: String,
    rtp_port: u16,
    crypto_b64_key: Option<&str>,
) -> String {
    let session_id = rand::random::<u32>();
    // Parse offered payload types from m= line
    let mut offered_pts: Vec<u8> = Vec::new();
    let mut remote_uses_savp = false;
    for line in remote_sdp.lines() {
        let line = line.trim();
        if line.starts_with("m=audio ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[2] == "RTP/SAVP" {
                remote_uses_savp = true;
            }
            for pt_str in parts.iter().skip(3) {
                if let Ok(pt) = pt_str.parse::<u8>() {
                    offered_pts.push(pt);
                }
            }
        }
    }

    // Use SAVP if the remote offered it and we have a crypto key
    let use_srtp = remote_uses_savp && crypto_b64_key.is_some();
    let profile = if use_srtp { "RTP/SAVP" } else { "RTP/AVP" };

    // Keep every offered payload type we can carry, on the number the
    // offer gave it: the static codecs, Opus on whatever dynamic type the
    // peer put it on (not only 111), and telephone-event at every rate the
    // peer offered, so DTMF can run on the clock that matches the codec.
    let rtpmaps = parse_rtpmaps(remote_sdp);
    let mut common: Vec<(u8, OfferedPayload)> = offered_pts
        .iter()
        .filter_map(|&pt| classify_payload(pt, &rtpmaps).map(|p| (pt, p)))
        .collect();
    if !common.iter().any(|(_, p)| matches!(p, OfferedPayload::Codec(_))) {
        common.insert(0, (0, OfferedPayload::Codec(rtp_engine::CodecType::Pcmu))); // fallback to PCMU
    }

    let pt_list: String = common
        .iter()
        .map(|(pt, _)| pt.to_string())
        .collect::<Vec<_>>()
        .join(" ");

    let mut sdp = format!(
        "v=0\r\n\
         o=aria {sid} {sid} IN IP4 {ip}\r\n\
         s=Aria Call\r\n\
         c=IN IP4 {ip}\r\n\
         t=0 0\r\n\
         m=audio {port} {profile} {pts}\r\n",
        sid = session_id,
        ip = ip,
        port = rtp_port,
        profile = profile,
        pts = pt_list,
    );

    for (pt, payload) in &common {
        match payload {
            OfferedPayload::Codec(codec) => sdp.push_str(&codec_rtpmap(*pt, *codec)),
            OfferedPayload::TelephoneEvent(rate) => sdp.push_str(&telephone_event_rtpmap(*pt, *rate)),
        }
    }

    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");

    if use_srtp {
        if let Some(key) = crypto_b64_key {
            sdp.push_str(&super::srtp::build_sdp_crypto_line(key));
        }
    }

    sdp
}

/// Parse the `a=crypto` attribute from remote SDP.
///
/// Returns the base64-encoded keying material if found with
/// the AES_CM_128_HMAC_SHA1_80 crypto suite.
#[allow(dead_code)]
pub fn parse_sdp_crypto(sdp: &str) -> Option<String> {
    super::srtp::parse_sdp_crypto(sdp)
}

/// The payload type and clock rate of the peer's `telephone-event` (DTMF,
/// RFC 4733), if any. DTMF must be sent on the type the peer offered; 101 is
/// only the usual default. When the peer named several, this prefers the entry
/// whose clock matches `audio_clock_rate` — the codec's RTP clock — since RFC
/// 4733 §2.1 measures a digit's duration and timestamp on the event's clock
/// and the two are meant to agree. A peer that offered only one rate gets
/// that one, whatever the codec.
pub fn parse_telephone_event(sdp: &str, audio_clock_rate: u32) -> Option<(u8, u32)> {
    let events: Vec<(u8, u32)> = parse_rtpmaps(sdp)
        .into_iter()
        .filter(|m| m.name == "telephone-event")
        .map(|m| (m.pt, m.rate))
        .collect();
    events
        .iter()
        .find(|(_, rate)| *rate == audio_clock_rate)
        .or_else(|| events.first())
        .copied()
}

/// Whether an SDP offer's audio media uses an SRTP profile (RTP/SAVP).
pub fn sdp_offers_savp(sdp: &str) -> bool {
    sdp.lines().any(|line| {
        let line = line.trim();
        line.starts_with("m=audio ")
            && line.split_whitespace().nth(2).is_some_and(|p| p.contains("SAVP"))
    })
}

/// Build SDP offer/answer for audio (PCMU default, PCMA, Opus, telephone-event)
///
/// When `crypto_b64_key` is provided, the offer uses RTP/SAVP and includes an
/// `a=crypto` line for SDES key exchange (RFC 4568).
#[allow(dead_code)]
fn build_sdp_offer(ip: String, rtp_port: u16) -> String {
    build_sdp_offer_srtp(ip, rtp_port, None)
}

/// Build SDP offer with optional SRTP crypto attribute (legacy, uses default codecs).
pub fn build_sdp_offer_srtp(ip: String, rtp_port: u16, crypto_b64_key: Option<&str>) -> String {
    use super::account::default_codec_preferences;
    build_sdp_offer_with_codecs(ip, rtp_port, crypto_b64_key, &default_codec_preferences())
}

/// Build SDP offer with configurable codec preferences.
///
/// Codecs are ordered by priority (lowest priority number = first in offer).
/// Only enabled codecs are included in the offer.
pub fn build_sdp_offer_with_codecs(
    ip: String,
    rtp_port: u16,
    crypto_b64_key: Option<&str>,
    codecs: &[super::account::CodecPreference],
) -> String {
    use rtp_engine::CodecType;

    let session_id = rand::random::<u32>();
    let profile = if crypto_b64_key.is_some() {
        "RTP/SAVP"
    } else {
        "RTP/AVP"
    };

    // Build payload type list from enabled codecs in priority order. Only the
    // codecs the softphone carries: a saved preference may still name G.729.
    let offered: Vec<&super::account::CodecPreference> = codecs
        .iter()
        .filter(|c| c.enabled && SUPPORTED_CODECS.contains(&c.codec))
        .collect();
    let mut pts: Vec<u8> = offered.iter().map(|c| c.codec.payload_type()).collect();

    // Fallback to PCMU if no codecs enabled
    if pts.is_empty() {
        pts.push(CodecType::Pcmu.payload_type());
    }

    // Always include telephone-event for DTMF, at 8 kHz for the telephone
    // codecs and also at 48 kHz when Opus is on offer, so the peer can pair
    // DTMF with the codec's own clock.
    let offers_opus = offered.iter().any(|c| c.codec == CodecType::Opus);
    pts.push(TELEPHONE_EVENT_PT_8K);
    if offers_opus {
        pts.push(TELEPHONE_EVENT_PT_48K);
    }

    let pt_list = pts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(" ");

    let mut sdp = format!(
        "v=0\r\n\
         o=aria {sid} {sid} IN IP4 {ip}\r\n\
         s=Aria Call\r\n\
         c=IN IP4 {ip}\r\n\
         t=0 0\r\n\
         m=audio {port} {profile} {pts}\r\n",
        sid = session_id,
        ip = ip,
        port = rtp_port,
        profile = profile,
        pts = pt_list,
    );

    // Add rtpmap for each enabled codec in priority order
    for codec_pref in &offered {
        sdp.push_str(&codec_rtpmap(codec_pref.codec.payload_type(), codec_pref.codec));
    }

    // Telephone event for DTMF
    sdp.push_str(&telephone_event_rtpmap(TELEPHONE_EVENT_PT_8K, 8000));
    if offers_opus {
        sdp.push_str(&telephone_event_rtpmap(TELEPHONE_EVENT_PT_48K, 48000));
    }
    sdp.push_str("a=ptime:20\r\n");
    sdp.push_str("a=sendrecv\r\n");

    if let Some(key) = crypto_b64_key {
        sdp.push_str(&super::srtp::build_sdp_crypto_line(key));
    }
    sdp
}

// ---------------------------------------------------------------------------
// REFER (blind transfer, RFC 3515)
// ---------------------------------------------------------------------------

/// Build a REFER request for blind transfer (RFC 3515)
#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_refer(
    target_uri: &str,
    refer_to: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: &str,
    transport: &str,
    route_set: &[String],
    from_uri: &str,
    to_uri: &str,
) -> String {
    let branch = generate_branch();
    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, &branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>;tag={}", to_uri, to_tag);
    let cseq_hdr = format!("{} REFER", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);

    for route in route_set {
        headers.push(rsip::headers::Route::new(route.as_str()).into());
    }

    headers.push(rsip::Header::Other("Refer-To".into(), format!("<{}>", refer_to)));
    headers.push(rsip::Header::Other(
        "Referred-By".into(),
        format!("<sip:user@{}:{}>", local_addr.ip(), local_addr.port()),
    ));
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Refer, request_uri, headers, vec![])
}

/// Build a REFER with Replaces for attended transfer (RFC 3515 + RFC 3891)
#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_refer_with_replaces(
    target_uri: &str,
    refer_to: &str,
    replaces_call_id: &str,
    replaces_to_tag: &str,
    replaces_from_tag: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: &str,
    transport: &str,
    route_set: &[String],
    from_uri: &str,
    to_uri: &str,
) -> String {
    let branch = generate_branch();
    // URL-encode the Replaces header value inside Refer-To URI
    let replaces_param = format!(
        "{}%3Bto-tag%3D{}%3Bfrom-tag%3D{}",
        replaces_call_id, replaces_to_tag, replaces_from_tag
    );
    let refer_to_uri = format!("{}?Replaces={}", refer_to, replaces_param);

    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, &branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>;tag={}", to_uri, to_tag);
    let cseq_hdr = format!("{} REFER", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);

    for route in route_set {
        headers.push(rsip::headers::Route::new(route.as_str()).into());
    }

    headers.push(rsip::Header::Other("Refer-To".into(), format!("<{}>", refer_to_uri)));
    headers.push(rsip::Header::Other(
        "Referred-By".into(),
        format!("<sip:user@{}:{}>", local_addr.ip(), local_addr.port()),
    ));
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Refer, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// 202 Accepted (for REFER)
// ---------------------------------------------------------------------------

/// Build a 202 Accepted response for an incoming REFER
#[allow(dead_code)]
pub fn build_202_accepted(request: &str, to_tag: &str) -> Option<String> {
    let via = extract_header(request, "Via")?;
    let from = extract_header(request, "From")?;
    let to_raw = extract_header(request, "To")?;
    let call_id = extract_header(request, "Call-ID")?;
    let cseq = extract_header(request, "CSeq")?;

    let to = if to_raw.contains("tag=") {
        to_raw
    } else {
        format!("{};tag={}", to_raw, to_tag)
    };

    let mut headers = rsip::Headers::default();
    headers.push(rsip::headers::Via::new(via).into());
    headers.push(rsip::headers::From::new(from).into());
    headers.push(rsip::headers::To::new(to).into());
    headers.push(rsip::headers::CallId::new(call_id).into());
    headers.push(rsip::headers::CSeq::new(cseq).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    Some(response_to_string(202.into(), headers, vec![]))
}

// ---------------------------------------------------------------------------
// NOTIFY (REFER progress)
// ---------------------------------------------------------------------------

/// Build a NOTIFY with message/sipfrag body for REFER progress (RFC 3515 section 2.4.5)
#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_notify_refer(
    target_uri: &str,
    from_uri: &str,
    to_uri: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: &str,
    transport: &str,
    sipfrag_status: u16,
    sipfrag_reason: &str,
    subscription_state: &str,
) -> String {
    let branch = generate_branch();
    let body = format!("SIP/2.0 {} {}\r\n", sipfrag_status, sipfrag_reason);

    let request_uri = parse_uri(target_uri);

    let via = via_value(transport, local_addr, &branch);
    let from_hdr = format!("<{}>;tag={}", from_uri, from_tag);
    let to_hdr = format!("<{}>;tag={}", to_uri, to_tag);
    let cseq_hdr = format!("{} NOTIFY", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);
    headers.push(rsip::headers::Event::new("refer").into());
    headers.push(rsip::headers::SubscriptionState::new(subscription_state).into());
    headers.push(rsip::headers::ContentType::new("message/sipfrag;version=2.0").into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new(body.len().to_string()).into());

    request_to_string(rsip::Method::Notify, request_uri, headers, body.into_bytes())
}

// ---------------------------------------------------------------------------
// INVITE with Replaces (RFC 3891)
// ---------------------------------------------------------------------------

/// Build an INVITE with Replaces header (RFC 3891)
#[allow(clippy::too_many_arguments, dead_code)]
pub fn build_invite_with_replaces(
    account: &AccountConfig,
    target_uri: &str,
    local_addr: SocketAddr,
    rtp_port: u16,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    replaces_call_id: &str,
    replaces_to_tag: &str,
    replaces_from_tag: &str,
    auth_header: Option<&str>,
) -> String {
    let transport_param = account.transport.param();
    let branch = generate_branch();

    // Generate SRTP key if account has SRTP enabled
    let crypto_key = match account.srtp_mode {
        SrtpMode::Sdes => {
            rtp_engine::srtp::SrtpContext::generate()
                .ok()
                .map(|(_, key)| key)
        }
        SrtpMode::Disabled => None,
    };
    let sdp = build_sdp_offer_with_codecs(local_addr.ip().to_string(), rtp_port, crypto_key.as_deref(), &account.codecs);

    let request_uri = parse_uri(target_uri);

    let via = via_value(transport_param, local_addr, &branch);
    let from_hdr = format!(
        "\"{}\" <sip:{}@{}>;tag={}",
        account.display_name, account.username, account.domain, from_tag,
    );
    let to_hdr = format!("<{}>", target_uri);
    let cseq_hdr = format!("{} INVITE", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);

    let contact = format!(
        "<sip:{}@{}:{};transport={}>",
        account.username, local_addr.ip(), local_addr.port(), transport_param,
    );
    headers.push(rsip::headers::Contact::new(contact).into());
    headers.push(rsip::Header::Other(
        "Replaces".into(),
        format!("{};to-tag={};from-tag={}", replaces_call_id, replaces_to_tag, replaces_from_tag),
    ));
    headers.push(rsip::headers::ContentType::new("application/sdp").into());
    headers.push(rsip::headers::Allow::new(ALLOW).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());

    if let Some(auth) = auth_header {
        headers.push(rsip::headers::ProxyAuthorization::new(auth).into());
    }

    headers.push(rsip::headers::ContentLength::new(sdp.len().to_string()).into());

    request_to_string(rsip::Method::Invite, request_uri, headers, sdp.into_bytes())
}

// ---------------------------------------------------------------------------
// OPTIONS
// ---------------------------------------------------------------------------

/// Build a SIP OPTIONS request (used as keepalive)
#[allow(clippy::too_many_arguments)]
pub fn build_options(
    domain: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    username: &str,
    transport: &str,
) -> String {
    let branch = generate_branch();

    let request_uri = rsip::Uri {
        scheme: Some(rsip::Scheme::Sip),
        host_with_port: rsip::Domain::from(domain).into(),
        ..Default::default()
    };

    let via = via_value(transport, local_addr, &branch);
    let from_hdr = format!("<sip:{}@{}>;tag={}", username, domain, from_tag);
    let to_hdr = format!("<sip:{}>", domain);
    let cseq_hdr = format!("{} OPTIONS", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);
    headers.push(rsip::headers::Accept::new("application/sdp").into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Options, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// SUBSCRIBE
// ---------------------------------------------------------------------------

/// Build a SUBSCRIBE request (RFC 6665)
#[allow(clippy::too_many_arguments)]
pub fn build_subscribe(
    account: &AccountConfig,
    target_uri: &str,
    local_addr: SocketAddr,
    call_id: &str,
    cseq: u32,
    from_tag: &str,
    to_tag: Option<&str>,
    event_type: &EventType,
    expires: u32,
    auth_header: Option<&str>,
) -> String {
    let transport_param = account.transport.param();
    let branch = generate_branch();

    let request_uri = parse_uri(target_uri);

    let via = via_value(transport_param, local_addr, &branch);
    let from_hdr = format!("<sip:{}@{}>;tag={}", account.username, account.domain, from_tag);
    // With the server's tag this is a refresh inside the existing
    // subscription (RFC 6665 §4.1.2.2). Without it, it is a brand-new
    // subscription: refreshes used to leave the tag off, so the server
    // created a new subscription every time and let the original expire,
    // which ended BLF updates for that extension after ten minutes.
    let to_hdr = match to_tag {
        Some(tag) => format!("<{}>;tag={}", target_uri, tag),
        None => format!("<{}>", target_uri),
    };
    let cseq_hdr = format!("{} SUBSCRIBE", cseq);

    let mut headers = base_request_headers(&via, &from_hdr, &to_hdr, call_id, &cseq_hdr);

    let contact = format!(
        "<sip:{}@{}:{};transport={}>",
        account.username, local_addr.ip(), local_addr.port(), transport_param,
    );
    headers.push(rsip::headers::Contact::new(contact).into());
    headers.push(rsip::headers::Event::new(event_type.event_header()).into());
    headers.push(rsip::headers::Accept::new(event_type.accept_header()).into());
    headers.push(rsip::headers::Expires::new(expires.to_string()).into());
    headers.push(rsip::headers::Allow::new(
        "INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, SUBSCRIBE, REFER, INFO",
    ).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());

    if let Some(auth) = auth_header {
        headers.push(rsip::headers::Authorization::new(auth).into());
    }

    headers.push(rsip::headers::ContentLength::new("0").into());

    request_to_string(rsip::Method::Subscribe, request_uri, headers, vec![])
}

// ---------------------------------------------------------------------------
// 200 OK (SUBSCRIBE)
// ---------------------------------------------------------------------------

/// Build a 200 OK response for an incoming SUBSCRIBE
pub fn build_200_ok_subscribe(request_raw: &str, expires: u32) -> Option<String> {
    let via = extract_header(request_raw, "Via")?;
    let from = extract_header(request_raw, "From")?;
    let to_base = extract_header(request_raw, "To")?;
    let call_id = extract_header(request_raw, "Call-ID")?;
    let cseq = extract_header(request_raw, "CSeq")?;

    let to = if to_base.contains("tag=") {
        to_base
    } else {
        let tag = generate_tag();
        format!("{};tag={}", to_base, tag)
    };

    let mut headers = rsip::Headers::default();
    headers.push(rsip::headers::Via::new(via).into());
    headers.push(rsip::headers::From::new(from).into());
    headers.push(rsip::headers::To::new(to).into());
    headers.push(rsip::headers::CallId::new(call_id).into());
    headers.push(rsip::headers::CSeq::new(cseq).into());
    headers.push(rsip::headers::Expires::new(expires.to_string()).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    Some(response_to_string(200.into(), headers, vec![]))
}

// ---------------------------------------------------------------------------
// Simple response
// ---------------------------------------------------------------------------

/// Build a bodiless final response to an INVITE, with our tag on To.
///
/// A final response creates (or ends) a dialog, so it needs a To tag (RFC
/// 3261 §12.1.1); `build_simple_response` copies To as it was in the request.
/// An existing tag — a re-sent INVITE already carries ours — is kept.
pub fn build_final_response(request: &str, code: u16, reason: &str, to_tag: &str) -> Option<String> {
    let response = build_simple_response(request, code, reason)?;
    let to = extract_header(request, "To")?;
    if to.contains("tag=") {
        return Some(response);
    }
    Some(response.replace(&format!("To: {to}\r\n"), &format!("To: {to};tag={to_tag}\r\n")))
}

/// Build a simple SIP response (no body) from a request
pub fn build_simple_response(request: &str, code: u16, reason: &str) -> Option<String> {
    let via = extract_header(request, "Via")?;
    let from = extract_header(request, "From")?;
    let to = extract_header(request, "To")?;
    let call_id = extract_header(request, "Call-ID")?;
    let cseq = extract_header(request, "CSeq")?;

    // rsip::StatusCode only stores the numeric code; the reason phrase is appended
    // in its Display impl. Since we need a custom reason phrase, we build the status
    // line manually and let rsip handle the headers.
    let mut headers = rsip::Headers::default();
    headers.push(rsip::headers::Via::new(via).into());
    headers.push(rsip::headers::From::new(from).into());
    headers.push(rsip::headers::To::new(to).into());
    headers.push(rsip::headers::CallId::new(call_id).into());
    headers.push(rsip::headers::CSeq::new(cseq).into());
    headers.push(rsip::headers::UserAgent::new(USER_AGENT).into());
    headers.push(rsip::headers::ContentLength::new("0").into());

    // Use format! for the status line so we keep the exact reason phrase the caller wants.
    // rsip::StatusCode maps code -> canonical reason, which may differ from what the caller
    // intends (e.g., 481 "Call/Transaction Does Not Exist" vs rsip's default).
    Some(format!(
        "SIP/2.0 {} {}\r\n{}\r\n",
        code,
        reason,
        headers,
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    #[test]
    fn contact_uri_takes_the_bracketed_uri_and_drops_header_params() {
        assert_eq!(
            contact_uri("\"Bob\" <sip:bob@10.0.0.5:5060;transport=udp>;expires=300"),
            "sip:bob@10.0.0.5:5060;transport=udp"
        );
        assert_eq!(contact_uri(" sip:bob@10.0.0.5 "), "sip:bob@10.0.0.5");
    }

    use super::*;

    /// rtp-engine's G.729 is not ITU-T G.729, so the softphone neither offers
    /// it (by default or from a saved preference) nor answers with it.
    #[test]
    fn g729_is_never_offered_or_answered() {
        use super::super::account::{CodecPreference, default_codec_preferences};
        use rtp_engine::CodecType;

        let defaults = build_sdp_offer_with_codecs("10.0.0.2".to_string(), 40000, None, &default_codec_preferences());
        assert!(defaults.contains("m=audio 40000 RTP/AVP 111 0 8 101 102\r\n"), "{defaults}");
        assert!(!defaults.contains("G729"));

        let saved = build_sdp_offer_with_codecs(
            "10.0.0.2".to_string(),
            40000,
            None,
            &[
                CodecPreference { codec: CodecType::G729, enabled: true, priority: 1 },
                CodecPreference { codec: CodecType::Pcmu, enabled: true, priority: 2 },
            ],
        );
        assert!(saved.contains("m=audio 40000 RTP/AVP 0 101\r\n"), "{saved}");
        assert!(!saved.contains("G729"));

        let answer = build_sdp_answer("m=audio 20000 RTP/AVP 18 0\r\n", "10.0.0.2".to_string(), 30000);
        assert!(answer.contains("m=audio 30000 RTP/AVP 0\r\n"), "{answer}");
    }

    /// Every codec's rtpmap line, pinned: what the answer and offer carry.
    #[test]
    fn codec_rtpmap_lines() {
        use rtp_engine::CodecType;
        assert_eq!(codec_rtpmap(0, CodecType::Pcmu), "a=rtpmap:0 PCMU/8000\r\n");
        assert_eq!(codec_rtpmap(8, CodecType::Pcma), "a=rtpmap:8 PCMA/8000\r\n");
        assert_eq!(
            codec_rtpmap(18, CodecType::G729),
            "a=rtpmap:18 G729/8000\r\na=fmtp:18 annexb=no\r\n"
        );
        assert_eq!(
            codec_rtpmap(111, CodecType::Opus),
            "a=rtpmap:111 opus/48000/2\r\na=fmtp:111 minptime=10;useinbandfec=1\r\n"
        );
    }

    fn addr() -> SocketAddr {
        "192.168.1.100:5060".parse().unwrap()
    }

    #[test]
    fn bye_has_correct_from_to_uris() {
        let msg = build_bye(
            "sip:pbx@159.203.80.231", addr(), "call-123", 2,
            "from-tag", "to-tag", "UDP",
            "sip:1001@example.com", "sip:1002@example.com",
        );
        assert!(msg.starts_with("BYE sip:pbx@159.203.80.231 SIP/2.0\r\n"));
        assert!(msg.contains("From: <sip:1001@example.com>;tag=from-tag"));
        assert!(msg.contains("To: <sip:1002@example.com>;tag=to-tag"));
        assert!(msg.contains("Call-ID: call-123"));
        assert!(msg.contains("CSeq: 2 BYE"));
        assert!(!msg.contains("user@host"));
        assert!(!msg.contains("unknown"));
    }

    #[test]
    fn bye_with_routes_includes_route_headers() {
        let routes = vec!["<sip:proxy.example.com;lr>".to_string()];
        let msg = build_bye_with_routes(
            "sip:pbx@159.203.80.231", addr(), "call-123", 2,
            "ft", "tt", "UDP", &routes,
            "sip:1001@example.com", "sip:1002@example.com",
        );
        assert!(msg.contains("Route: <sip:proxy.example.com;lr>"));
        assert!(msg.contains("From: <sip:1001@example.com>;tag=ft"));
        assert!(!msg.contains("user@host"));
    }

    #[test]
    fn ack_has_correct_from_to_uris() {
        let msg = build_ack(
            "sip:1002@example.com", addr(), "call-123", 1,
            "ft", "tt", "UDP", "z9hG4bK-test",
            "sip:1001@example.com", "sip:1002@example.com",
        );
        assert!(msg.starts_with("ACK sip:1002@example.com SIP/2.0\r\n"));
        assert!(msg.contains("From: <sip:1001@example.com>;tag=ft"));
        assert!(msg.contains("To: <sip:1002@example.com>;tag=tt"));
        assert!(!msg.contains("user@host"));
    }

    #[test]
    fn cancel_has_correct_from_to_uris() {
        let msg = build_cancel(
            "sip:1002@example.com", addr(), "call-123", 1,
            "ft", "UDP", "z9hG4bK-branch", "sip:1001@example.com", "sip:1002@example.com",
        );
        assert!(msg.starts_with("CANCEL sip:1002@example.com SIP/2.0\r\n"));
        assert!(msg.contains("From: <sip:1001@example.com>;tag=ft"));
        assert!(msg.contains("To: <sip:1002@example.com>"));
        assert!(msg.contains("branch=z9hG4bK-branch"));
        assert!(!msg.contains("user@host"));
    }

    #[test]
    fn all_messages_end_with_double_crlf() {
        let bye = build_bye("sip:x@y", addr(), "c", 1, "f", "t", "UDP", "sip:a@b", "sip:x@y");
        assert!(bye.ends_with("\r\n\r\n"), "BYE must end with \\r\\n\\r\\n");

        let ack = build_ack("sip:x@y", addr(), "c", 1, "f", "t", "UDP", "z9hG4bK-b", "sip:a@b", "sip:x@y");
        assert!(ack.ends_with("\r\n\r\n"), "ACK must end with \\r\\n\\r\\n");

        let cancel = build_cancel("sip:x@y", addr(), "c", 1, "f", "UDP", "z9hG4bK-b", "sip:a@b", "sip:x@y");
        assert!(cancel.ends_with("\r\n\r\n"), "CANCEL must end with \\r\\n\\r\\n");
    }

    #[test]
    fn options_is_valid_sip() {
        let msg = build_options("example.com", addr(), "call-1", 1, "ft", "1001", "UDP");
        assert!(msg.starts_with("OPTIONS sip:example.com SIP/2.0\r\n"));
        assert!(msg.contains("Accept: application/sdp"));
        assert!(msg.contains("From: <sip:1001@example.com>;tag=ft"));
    }

    #[test]
    fn sdp_answer_has_correct_format() {
        let remote_sdp = "v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\ns=-\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\nm=audio 20000 RTP/AVP 0 8 101\r\na=rtpmap:0 PCMU/8000\r\n";
        let answer = build_sdp_answer(remote_sdp, "192.168.1.100".to_string(), 30000);
        assert!(answer.contains("v=0\r\n"));
        assert!(answer.contains("c=IN IP4 192.168.1.100"));
        assert!(answer.contains("m=audio 30000 RTP/AVP"));
    }

    #[test]
    fn no_message_contains_hardcoded_user_at_host() {
        let bye = build_bye("sip:x@y", addr(), "c", 1, "f", "t", "UDP", "sip:a@b", "sip:x@y");
        assert!(!bye.contains("user@host"));

        let ack = build_ack("sip:x@y", addr(), "c", 1, "f", "t", "UDP", "z9hG4bK-b", "sip:a@b", "sip:x@y");
        assert!(!ack.contains("user@host"));

        let cancel = build_cancel("sip:x@y", addr(), "c", 1, "f", "UDP", "z9hG4bK-b", "sip:a@b", "sip:x@y");
        assert!(!cancel.contains("user@host"));
    }

    const SAVP_OFFER: &str = "v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\ns=-\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\n\
        m=audio 20000 RTP/SAVP 0 8 96\r\na=rtpmap:0 PCMU/8000\r\n\
        a=rtpmap:96 telephone-event/8000\r\n\
        a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:WVyS3cm9vZ3J0ZXN0aW5na2V5bWF0ZXJpYWwxMjM0\r\n";

    #[test]
    fn savp_offer_is_recognised_and_its_crypto_key_parsed() {
        assert!(sdp_offers_savp(SAVP_OFFER));
        assert!(!sdp_offers_savp(
            "m=audio 20000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"
        ));
        assert!(parse_sdp_crypto(SAVP_OFFER).is_some());
    }

    #[test]
    fn telephone_event_pt_is_read_from_the_offer() {
        assert_eq!(parse_telephone_event(SAVP_OFFER, 8000), Some((96, 8000)));
        assert_eq!(
            parse_telephone_event("a=rtpmap:0 PCMU/8000\r\n", 8000),
            None
        );
    }

    /// A peer that named telephone-event at both rates gets DTMF on the one
    /// matching the codec's clock; one that named a single rate gets that
    /// one whatever the codec, since it is the only thing it can decode.
    #[test]
    fn telephone_event_is_picked_by_the_codecs_clock() {
        let both = "m=audio 20000 RTP/AVP 111 0 101 102\r\n\
                    a=rtpmap:111 opus/48000/2\r\n\
                    a=rtpmap:0 PCMU/8000\r\n\
                    a=rtpmap:101 telephone-event/8000\r\n\
                    a=rtpmap:102 telephone-event/48000\r\n";
        assert_eq!(parse_telephone_event(both, 48000), Some((102, 48000)));
        assert_eq!(parse_telephone_event(both, 8000), Some((101, 8000)));

        let only_8k = "m=audio 20000 RTP/AVP 111 96\r\n\
                       a=rtpmap:111 opus/48000/2\r\n\
                       a=rtpmap:96 telephone-event/8000\r\n";
        assert_eq!(parse_telephone_event(only_8k, 48000), Some((96, 8000)));
        assert_eq!(parse_telephone_event("a=rtpmap:0 PCMU/8000\r\n", 8000), None);
    }

    /// Offering Opus also offers telephone-event at 48 kHz, on its own
    /// payload type, beside the classic 8 kHz one. Without Opus only the
    /// 8 kHz entry goes out.
    #[test]
    fn offer_carries_telephone_event_at_48k_only_with_opus() {
        use super::super::account::CodecPreference;
        use rtp_engine::CodecType;

        let with_opus = build_sdp_offer_with_codecs(
            "10.0.0.2".to_string(),
            40000,
            None,
            &[
                CodecPreference { codec: CodecType::Opus, enabled: true, priority: 1 },
                CodecPreference { codec: CodecType::Pcmu, enabled: true, priority: 2 },
            ],
        );
        assert!(with_opus.contains("m=audio 40000 RTP/AVP 111 0 101 102\r\n"), "{with_opus}");
        assert!(with_opus.contains("a=rtpmap:111 opus/48000/2\r\n"));
        assert!(with_opus.contains("a=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\n"));
        assert!(with_opus.contains("a=rtpmap:102 telephone-event/48000\r\na=fmtp:102 0-16\r\n"));

        let without_opus = build_sdp_offer_with_codecs(
            "10.0.0.2".to_string(),
            40000,
            None,
            &[
                CodecPreference { codec: CodecType::Opus, enabled: false, priority: 1 },
                CodecPreference { codec: CodecType::Pcmu, enabled: true, priority: 2 },
            ],
        );
        assert!(without_opus.contains("m=audio 40000 RTP/AVP 0 101\r\n"), "{without_opus}");
        assert!(!without_opus.contains("telephone-event/48000"));
        assert!(!without_opus.contains("opus"));
    }

    /// The answer echoes Opus on the payload type the offer carried it on
    /// (not a fixed 111), and every telephone-event rate the offer named, each
    /// on its own number.
    #[test]
    fn answer_echoes_opus_and_telephone_event_on_the_offered_payload_types() {
        let offer = "v=0\r\no=- 1 1 IN IP4 10.0.0.1\r\ns=-\r\nc=IN IP4 10.0.0.1\r\nt=0 0\r\n\
                     m=audio 20000 RTP/AVP 96 0 97 98\r\n\
                     a=rtpmap:96 opus/48000/2\r\n\
                     a=fmtp:96 useinbandfec=1\r\n\
                     a=rtpmap:0 PCMU/8000\r\n\
                     a=rtpmap:97 telephone-event/48000\r\n\
                     a=rtpmap:98 telephone-event/8000\r\n";
        let answer = build_sdp_answer(offer, "10.0.0.2".to_string(), 30000);
        assert!(answer.contains("m=audio 30000 RTP/AVP 96 0 97 98\r\n"), "{answer}");
        assert!(answer.contains("a=rtpmap:96 opus/48000/2\r\na=fmtp:96 minptime=10;useinbandfec=1\r\n"));
        assert!(answer.contains("a=rtpmap:0 PCMU/8000\r\n"));
        assert!(answer.contains("a=rtpmap:97 telephone-event/48000\r\na=fmtp:97 0-16\r\n"));
        assert!(answer.contains("a=rtpmap:98 telephone-event/8000\r\na=fmtp:98 0-16\r\n"));
        assert!(!answer.contains("111"), "{answer}");
    }

    /// An rtpmap is authoritative: 111 mapped to something else is not Opus,
    /// an unknown codec is left out, and the static numbers still work bare.
    #[test]
    fn answer_drops_what_it_cannot_carry_and_keeps_static_types_bare() {
        let offer = "m=audio 20000 RTP/AVP 111 9 18 8 101\r\n\
                     a=rtpmap:111 speex/16000\r\n\
                     a=rtpmap:9 G722/8000\r\n";
        let answer = build_sdp_answer(offer, "10.0.0.2".to_string(), 30000);
        assert!(answer.contains("m=audio 30000 RTP/AVP 8 101\r\n"), "{answer}");
        assert!(!answer.contains("G729"), "G.729 is not carried: {answer}");
        assert!(answer.contains("a=rtpmap:8 PCMA/8000\r\n"));
        assert!(answer.contains("a=rtpmap:101 telephone-event/8000\r\n"));
        assert!(!answer.contains("speex") && !answer.contains("G722"));

        // Nothing usable at all: PCMU is the fallback.
        let none = build_sdp_answer("m=audio 20000 RTP/AVP 9\r\na=rtpmap:9 G722/8000\r\n", "10.0.0.2".to_string(), 30000);
        assert!(none.contains("m=audio 30000 RTP/AVP 0\r\n"), "{none}");
    }

    /// Given a key and a SAVP offer, the 200 OK answers RTP/SAVP with a crypto
    /// line. Without a key it stays plain RTP/AVP even for a SAVP offer, so the
    /// answer never promises encryption the session is not running.
    #[test]
    fn srtp_200_ok_answers_savp_with_a_crypto_line() {
        let invite = format!(
            "INVITE sip:1001@pbx SIP/2.0\r\nVia: SIP/2.0/UDP 10.0.0.1;branch=z9hG4bK1\r\n\
             From: <sip:200@pbx>;tag=caller\r\nTo: <sip:1001@pbx>\r\nCall-ID: c1\r\nCSeq: 1 INVITE\r\n\
             Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            SAVP_OFFER.len(),
            SAVP_OFFER
        );
        let key = "WVyS3cm9vZ3J0ZXN0aW5na2V5bWF0ZXJpYWwxMjM0";
        let enc = build_200_ok_invite_srtp(&invite, addr(), 30000, "t", Some("1001"), None, Some(key))
            .expect("200 OK");
        assert!(enc.contains("m=audio 30000 RTP/SAVP"), "{enc}");
        assert!(enc.contains("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:"), "{enc}");

        let plain = build_200_ok_invite_srtp(&invite, addr(), 30000, "t", Some("1001"), None, None)
            .expect("200 OK");
        assert!(plain.contains("m=audio 30000 RTP/AVP"), "{plain}");
        assert!(!plain.contains("a=crypto"), "{plain}");
    }
}
