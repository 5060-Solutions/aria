use crate::sip::account::{AccountConfig, SrtpMode};
use crate::sip::presence::EventType;
use std::net::SocketAddr;

/// Generate a unique branch parameter for Via headers
pub fn generate_branch() -> String {
    format!("z9hG4bK-{}", uuid::Uuid::new_v4().as_simple())
}

/// Generate a unique tag for From/To headers
pub fn generate_tag() -> String {
    format!("{:08x}", rand::random::<u32>())
}

/// Generate a unique Call-ID
pub fn generate_call_id() -> String {
    format!("{}", uuid::Uuid::new_v4().as_simple())
}

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

    let mut msg = format!(
        "REGISTER sip:{registrar} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:{user}@{domain}>;tag={from_tag}\r\n\
         To: <sip:{user}@{domain}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} REGISTER\r\n\
         Contact: <sip:{user}@{local_ip}:{local_port};transport={tp}>\r\n\
         Expires: {expires}\r\n\
         Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, REFER, INFO\r\n\
         User-Agent: Aria/0.2.0\r\n",
        registrar = registrar,
        transport = transport_param.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        user = account.username,
        domain = account.domain,
        from_tag = from_tag,
        call_id = call_id,
        cseq = cseq,
        tp = transport_param,
        expires = expires,
    );

    if let Some(auth) = auth_header {
        msg.push_str(&format!("Authorization: {}\r\n", auth));
    }

    msg.push_str("Content-Length: 0\r\n\r\n");
    msg
}

/// Authorization header type for SIP requests
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AuthHeaderType {
    /// Authorization header (for 401 WWW-Authenticate challenges)
    Authorization,
    /// Proxy-Authorization header (for 407 Proxy-Authenticate challenges)
    ProxyAuthorization,
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

    let mut msg = format!(
        "INVITE {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: \"{display}\" <sip:{user}@{domain}>;tag={from_tag}\r\n\
         To: <{target_uri}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} INVITE\r\n\
         Contact: <sip:{user}@{local_ip}:{local_port};transport={tp}>\r\n\
         Content-Type: application/sdp\r\n\
         Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, REFER, INFO\r\n\
         User-Agent: Aria/0.2.0\r\n",
        target_uri = target_uri,
        transport = transport_param.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        display = account.display_name,
        user = account.username,
        domain = account.domain,
        from_tag = from_tag,
        call_id = call_id,
        cseq = cseq,
        tp = transport_param,
    );

    if let Some((auth_value, auth_type)) = auth {
        let header_name = match auth_type {
            AuthHeaderType::Authorization => "Authorization",
            AuthHeaderType::ProxyAuthorization => "Proxy-Authorization",
        };
        msg.push_str(&format!("{}: {}\r\n", header_name, auth_value));
    }

    msg.push_str(&format!("Content-Length: {}\r\n\r\n{}", sdp.len(), sdp));
    (msg, crypto_key)
}

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
) -> String {
    format!(
        "ACK {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>;tag={to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} ACK\r\n\
         Content-Length: 0\r\n\r\n",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = via_branch,
        from_tag = from_tag,
        to_tag = to_tag,
        call_id = call_id,
        cseq = cseq,
    )
}

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
) -> String {
    let branch = generate_branch();
    format!(
        "BYE {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>;tag={to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} BYE\r\n\
         Content-Length: 0\r\n\r\n",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        from_tag = from_tag,
        to_tag = to_tag,
        call_id = call_id,
        cseq = cseq,
    )
}

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
) -> String {
    format!(
        "CANCEL {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} CANCEL\r\n\
         Content-Length: 0\r\n\r\n",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = via_branch,
        from_tag = from_tag,
        call_id = call_id,
        cseq = cseq,
    )
}

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

    Some(format!(
        "SIP/2.0 200 OK\r\n\
         Via: {via}\r\n\
         From: {from}\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq}\r\n\
         Contact: <{contact}>\r\n\
         Content-Type: application/sdp\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: {len}\r\n\r\n{sdp}",
        via = via,
        from = from,
        to = to,
        call_id = call_id,
        cseq = cseq,
        contact = contact_uri,
        len = sdp.len(),
        sdp = sdp,
    ))
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

/// Extract a header value from raw SIP message
pub fn extract_header(msg: &str, name: &str) -> Option<String> {
    for line in msg.lines() {
        // Case-insensitive header match
        let lower = line.to_lowercase();
        let search = format!("{}:", name.to_lowercase());
        if lower.starts_with(&search) {
            let value = &line[name.len() + 1..];
            return Some(value.trim().to_string());
        }
    }
    None
}

/// Extract the To tag from a SIP response
pub fn extract_to_tag(msg: &str) -> Option<String> {
    let to = extract_header(msg, "To")?;
    let tag_pos = to.find("tag=")?;
    let tag_start = tag_pos + 4;
    let tag_end = to[tag_start..]
        .find([';', '>', ' '])
        .map(|p| tag_start + p)
        .unwrap_or(to.len());
    Some(to[tag_start..tag_end].to_string())
}

/// Extract the Via branch from a SIP message
pub fn extract_via_branch(msg: &str) -> Option<String> {
    let via = extract_header(msg, "Via")?;
    let branch_pos = via.find("branch=")?;
    let start = branch_pos + 7;
    let end = via[start..]
        .find([';', ',', ' '])
        .map(|p| start + p)
        .unwrap_or(via.len());
    Some(via[start..end].to_string())
}

/// Parse SDP to extract remote RTP address and port
pub fn parse_sdp_connection(sdp: &str) -> Option<(String, u16)> {
    let mut ip = None;
    let mut port = None;

    for line in sdp.lines() {
        if let Some(addr) = line.strip_prefix("c=IN IP4 ") {
            ip = Some(addr.trim().to_string());
        }
        if line.starts_with("m=audio ") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                port = parts[1].parse().ok();
            }
        }
    }

    match (ip, port) {
        (Some(i), Some(p)) => Some((i, p)),
        _ => None,
    }
}

/// Get the status code from a SIP response line
pub fn parse_status_code(msg: &str) -> Option<u16> {
    let first_line = msg.lines().next()?;
    if first_line.starts_with("SIP/2.0 ") {
        let parts: Vec<&str> = first_line.split_whitespace().collect();
        if parts.len() >= 2 {
            return parts[1].parse().ok();
        }
    }
    None
}

/// Check if a raw message is a SIP request (not a response)
pub fn is_request(msg: &str) -> bool {
    let first_line = msg.lines().next().unwrap_or("");
    !first_line.starts_with("SIP/2.0")
}

/// Extract the method from a SIP request
pub fn extract_method(msg: &str) -> Option<String> {
    let first_line = msg.lines().next()?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() >= 2 && !first_line.starts_with("SIP/2.0") {
        Some(parts[0].to_string())
    } else {
        None
    }
}

/// Extract `received` and `rport` parameters from the Via header of a SIP response
pub fn extract_via_received(msg: &str) -> Option<(String, u16)> {
    let via = extract_header(msg, "Via")?;
    let received = via.find("received=").map(|p| {
        let start = p + 9;
        let end = via[start..]
            .find([';', ',', ' '])
            .map(|e| start + e)
            .unwrap_or(via.len());
        via[start..end].to_string()
    })?;
    let rport = via.find("rport=").and_then(|p| {
        let start = p + 6;
        let end = via[start..]
            .find([';', ',', ' '])
            .map(|e| start + e)
            .unwrap_or(via.len());
        via[start..end].parse::<u16>().ok()
    })?;
    Some((received, rport))
}

/// Extract all values for a given header (e.g., multiple Record-Route lines)
pub fn extract_all_headers(msg: &str, name: &str) -> Vec<String> {
    let search = format!("{}:", name.to_lowercase());
    let mut results = Vec::new();
    for line in msg.lines() {
        let lower = line.to_lowercase();
        if lower.starts_with(&search) {
            let value = &line[name.len() + 1..];
            results.push(value.trim().to_string());
        }
    }
    results
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
) -> String {
    let branch = generate_branch();
    let mut msg = format!(
        "BYE {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
    );

    for route in route_set {
        msg.push_str(&format!("Route: {}\r\n", route));
    }

    msg.push_str(&format!(
        "From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>;tag={to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} BYE\r\n\
         Content-Length: 0\r\n\r\n",
        from_tag = from_tag,
        target_uri = target_uri,
        to_tag = to_tag,
        call_id = call_id,
        cseq = cseq,
    ));
    msg
}

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
) -> String {
    let branch = generate_branch();
    let mut msg = format!(
        "REFER {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
    );

    for route in route_set {
        msg.push_str(&format!("Route: {}\r\n", route));
    }

    msg.push_str(&format!(
        "From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>;tag={to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} REFER\r\n\
         Refer-To: <{refer_to}>\r\n\
         Referred-By: <sip:user@{local_ip}:{local_port}>\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
        from_tag = from_tag,
        target_uri = target_uri,
        to_tag = to_tag,
        call_id = call_id,
        cseq = cseq,
        refer_to = refer_to,
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
    ));
    msg
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
) -> String {
    let branch = generate_branch();
    // URL-encode the Replaces header value inside Refer-To URI
    let replaces_param = format!(
        "{}%3Bto-tag%3D{}%3Bfrom-tag%3D{}",
        replaces_call_id, replaces_to_tag, replaces_from_tag
    );
    let refer_to_uri = format!("{}?Replaces={}", refer_to, replaces_param);

    let mut msg = format!(
        "REFER {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
    );

    for route in route_set {
        msg.push_str(&format!("Route: {}\r\n", route));
    }

    msg.push_str(&format!(
        "From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>;tag={to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} REFER\r\n\
         Refer-To: <{refer_to_uri}>\r\n\
         Referred-By: <sip:user@{local_ip}:{local_port}>\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
        from_tag = from_tag,
        target_uri = target_uri,
        to_tag = to_tag,
        call_id = call_id,
        cseq = cseq,
        refer_to_uri = refer_to_uri,
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
    ));
    msg
}

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

    Some(format!(
        "SIP/2.0 202 Accepted\r\n\
         Via: {via}\r\n\
         From: {from}\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq}\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
    ))
}

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
    format!(
        "NOTIFY {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:user@host>;tag={from_tag}\r\n\
         To: <{target_uri}>;tag={to_tag}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} NOTIFY\r\n\
         Event: refer\r\n\
         Subscription-State: {subscription_state}\r\n\
         Content-Type: message/sipfrag;version=2.0\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: {len}\r\n\r\n{body}",
        target_uri = target_uri,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        from_tag = from_tag,
        to_tag = to_tag,
        call_id = call_id,
        cseq = cseq,
        subscription_state = subscription_state,
        len = body.len(),
        body = body,
    )
}

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

    let mut msg = format!(
        "INVITE {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: \"{display}\" <sip:{user}@{domain}>;tag={from_tag}\r\n\
         To: <{target_uri}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} INVITE\r\n\
         Contact: <sip:{user}@{local_ip}:{local_port};transport={tp}>\r\n\
         Replaces: {replaces_call_id};to-tag={replaces_to_tag};from-tag={replaces_from_tag}\r\n\
         Content-Type: application/sdp\r\n\
         Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, REFER, INFO\r\n\
         User-Agent: Aria/0.2.0\r\n",
        target_uri = target_uri,
        transport = transport_param.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        display = account.display_name,
        user = account.username,
        domain = account.domain,
        from_tag = from_tag,
        call_id = call_id,
        cseq = cseq,
        tp = transport_param,
        replaces_call_id = replaces_call_id,
        replaces_to_tag = replaces_to_tag,
        replaces_from_tag = replaces_from_tag,
    );

    if let Some(auth) = auth_header {
        msg.push_str(&format!("Proxy-Authorization: {}\r\n", auth));
    }

    msg.push_str(&format!("Content-Length: {}\r\n\r\n{}", sdp.len(), sdp));
    msg
}

/// Extract the From tag from a SIP message
#[allow(dead_code)]
pub fn extract_from_tag(msg: &str) -> Option<String> {
    let from = extract_header(msg, "From")?;
    let tag_pos = from.find("tag=")?;
    let tag_start = tag_pos + 4;
    let tag_end = from[tag_start..]
        .find([';', '>', ' '])
        .map(|p| tag_start + p)
        .unwrap_or(from.len());
    Some(from[tag_start..tag_end].to_string())
}

/// Parse a sipfrag body to extract the status code (e.g. "SIP/2.0 200 OK" -> 200)
#[allow(dead_code)]
pub fn parse_sipfrag_status(body: &str) -> Option<u16> {
    let line = body.lines().next()?;
    if line.starts_with("SIP/2.0 ") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            return parts[1].parse().ok();
        }
    }
    None
}

/// Parse Replaces header: "call-id;to-tag=xxx;from-tag=yyy"
#[allow(dead_code)]
pub fn parse_replaces_header(header_value: &str) -> Option<(String, String, String)> {
    let parts: Vec<&str> = header_value.splitn(2, ';').collect();
    if parts.len() < 2 {
        return None;
    }
    let replaces_call_id = parts[0].trim().to_string();
    let params = parts[1];

    let mut to_tag = String::new();
    let mut from_tag = String::new();

    for param in params.split(';') {
        let param = param.trim();
        if let Some(val) = param.strip_prefix("to-tag=") {
            to_tag = val.to_string();
        } else if let Some(val) = param.strip_prefix("from-tag=") {
            from_tag = val.to_string();
        }
    }

    if to_tag.is_empty() || from_tag.is_empty() {
        return None;
    }

    Some((replaces_call_id, to_tag, from_tag))
}

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
    format!(
        "OPTIONS sip:{domain} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:{user}@{domain}>;tag={from_tag}\r\n\
         To: <sip:{domain}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} OPTIONS\r\n\
         Accept: application/sdp\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
        domain = domain,
        transport = transport.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        user = username,
        from_tag = from_tag,
        call_id = call_id,
        cseq = cseq,
    )
}

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

    let mut msg = format!(
        "SUBSCRIBE {target_uri} SIP/2.0\r\n\
         Via: SIP/2.0/{transport} {local_ip}:{local_port};branch={branch};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:{user}@{domain}>;tag={from_tag}\r\n\
         To: <{target_uri}>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} SUBSCRIBE\r\n\
         Contact: <sip:{user}@{local_ip}:{local_port};transport={tp}>\r\n\
         Event: {event}\r\n\
         Accept: {accept}\r\n\
         Expires: {expires}\r\n\
         Allow: INVITE, ACK, CANCEL, BYE, OPTIONS, NOTIFY, SUBSCRIBE, REFER, INFO\r\n\
         User-Agent: Aria/0.2.0\r\n",
        target_uri = target_uri,
        transport = transport_param.to_uppercase(),
        local_ip = local_addr.ip(),
        local_port = local_addr.port(),
        branch = branch,
        user = account.username,
        domain = account.domain,
        from_tag = from_tag,
        call_id = call_id,
        cseq = cseq,
        tp = transport_param,
        event = event_type.event_header(),
        accept = event_type.accept_header(),
        expires = expires,
    );

    if let Some(auth) = auth_header {
        msg.push_str(&format!("Authorization: {}\r\n", auth));
    }

    msg.push_str("Content-Length: 0\r\n\r\n");
    msg
}

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

    Some(format!(
        "SIP/2.0 200 OK\r\n\
         Via: {via}\r\n\
         From: {from}\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq}\r\n\
         Expires: {expires}\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
        via = via,
        from = from,
        to = to,
        call_id = call_id,
        cseq = cseq,
        expires = expires,
    ))
}

/// Build a simple SIP response (no body) from a request
pub fn build_simple_response(request: &str, code: u16, reason: &str) -> Option<String> {
    let via = extract_header(request, "Via")?;
    let from = extract_header(request, "From")?;
    let to = extract_header(request, "To")?;
    let call_id = extract_header(request, "Call-ID")?;
    let cseq = extract_header(request, "CSeq")?;

    Some(format!(
        "SIP/2.0 {} {}\r\n\
         Via: {}\r\n\
         From: {}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {}\r\n\
         User-Agent: Aria/0.2.0\r\n\
         Content-Length: 0\r\n\r\n",
        code, reason, via, from, to, call_id, cseq,
    ))
}
