use crate::sip::account::{AccountConfig, SrtpMode};
use crate::sip::presence::EventType;
use rsip::headers::UntypedHeader;
use std::net::SocketAddr;

// Re-export shared utilities so existing callers keep working
pub use aria_sip_core::{generate_branch, generate_call_id, generate_tag};
pub use aria_sip_core::parser::{
    extract_all_headers, extract_from_tag, extract_header, extract_method, extract_to_tag,
    extract_via_branch, extract_via_received, is_request, parse_replaces_header,
    parse_sdp_connection, parse_sipfrag_status, parse_status_code,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const USER_AGENT: &str = "Aria/0.2.0";
const ALLOW: &str = "INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, REFER, INFO";

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
    let sdp_ip = public_ip.map(|s| s.to_string()).unwrap_or_else(|| local_addr.ip().to_string());

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
        SrtpMode::Dtls => {
            log::warn!("DTLS-SRTP not yet implemented, falling back to plain RTP");
            None
        }
        SrtpMode::Disabled => {
            log::info!("SRTP disabled for this account");
            None
        }
    };
    let sdp = build_sdp_offer_with_codecs(sdp_ip, rtp_port, crypto_key.as_deref(), &account.codecs);

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
    let sdp_ip = public_ip.map(|s| s.to_string()).unwrap_or_else(|| local_addr.ip().to_string());

    // If the request has SDP, generate an answer; otherwise generate an offer
    let remote_sdp = request_raw.split("\r\n\r\n").nth(1).unwrap_or("");
    let sdp = if remote_sdp.contains("m=audio") {
        build_sdp_answer(remote_sdp, sdp_ip, rtp_port)
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

    // Our supported codecs: 0=PCMU, 8=PCMA, 18=G729, 111=Opus, 101=telephone-event
    let supported = [0u8, 8, 18, 111, 101];
    let mut common: Vec<u8> = offered_pts
        .iter()
        .filter(|pt| supported.contains(pt))
        .copied()
        .collect();
    if common.is_empty() {
        common.push(0); // fallback to PCMU
    }

    let pt_list: String = common
        .iter()
        .map(|pt| pt.to_string())
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

    for pt in &common {
        match pt {
            0 => sdp.push_str("a=rtpmap:0 PCMU/8000\r\n"),
            8 => sdp.push_str("a=rtpmap:8 PCMA/8000\r\n"),
            18 => {
                sdp.push_str("a=rtpmap:18 G729/8000\r\n");
                sdp.push_str("a=fmtp:18 annexb=no\r\n");
            }
            111 => {
                sdp.push_str("a=rtpmap:111 opus/48000/2\r\n");
                sdp.push_str("a=fmtp:111 minptime=10;useinbandfec=1\r\n");
            }
            101 => {
                sdp.push_str("a=rtpmap:101 telephone-event/8000\r\n");
                sdp.push_str("a=fmtp:101 0-16\r\n");
            }
            _ => {}
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

    // Build payload type list from enabled codecs in priority order
    let mut pts: Vec<u8> = codecs
        .iter()
        .filter(|c| c.enabled)
        .map(|c| c.codec.payload_type())
        .collect();

    // Always include telephone-event for DTMF
    pts.push(101);

    // Fallback to PCMU if no codecs enabled
    if pts.len() == 1 {
        pts.insert(0, 0);
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
    for codec_pref in codecs.iter().filter(|c| c.enabled) {
        match codec_pref.codec {
            CodecType::Pcmu => {
                sdp.push_str("a=rtpmap:0 PCMU/8000\r\n");
            }
            CodecType::Pcma => {
                sdp.push_str("a=rtpmap:8 PCMA/8000\r\n");
            }
            CodecType::G729 => {
                sdp.push_str("a=rtpmap:18 G729/8000\r\n");
                sdp.push_str("a=fmtp:18 annexb=no\r\n");
            }
            CodecType::Opus => {
                sdp.push_str("a=rtpmap:111 opus/48000/2\r\n");
                sdp.push_str("a=fmtp:111 minptime=10;useinbandfec=1\r\n");
            }
        }
    }

    // Telephone event for DTMF
    sdp.push_str("a=rtpmap:101 telephone-event/8000\r\n");
    sdp.push_str("a=fmtp:101 0-16\r\n");
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
    let from_hdr = format!("<sip:user@host>;tag={}", from_tag);
    let to_hdr = format!("<{}>;tag={}", target_uri, to_tag);
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
        SrtpMode::Dtls => None,
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
    event_type: &EventType,
    expires: u32,
    auth_header: Option<&str>,
) -> String {
    let transport_param = account.transport.param();
    let branch = generate_branch();

    let request_uri = parse_uri(target_uri);

    let via = via_value(transport_param, local_addr, &branch);
    let from_hdr = format!("<sip:{}@{}>;tag={}", account.username, account.domain, from_tag);
    let to_hdr = format!("<{}>", target_uri);
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
    use super::*;

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
}
