//! Media negotiation end to end: the codec a call runs on, mute against
//! hold, a re-INVITE that moves media, and early media that the 200 OK
//! replaces. The real `SipManager` against the fake PBX, over loopback.

use super::*;
use crate::sip::account::SrtpMode;
use crate::sip::media::CodecType;

/// An SDP from the PBX: `m=audio {port} {profile} {pts}` on loopback, with
/// whatever extra lines the test needs.
fn pbx_sdp(port: u16, profile: &str, pts: &str, extra: &str) -> String {
    format!(
        "v=0\r\no=pbx 7 7 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {port} {profile} {pts}\r\n{extra}"
    )
}

/// An INVITE from the PBX to the client: a new call without `to_tag`, a
/// re-INVITE in the dialog with it.
fn invite(
    pbx: SocketAddr,
    client: SocketAddr,
    call_id: &str,
    cseq: u32,
    to_tag: Option<&str>,
    sdp: &str,
) -> String {
    let to = match to_tag {
        Some(tag) => format!("<sip:{USER}@harness.lan>;tag={tag}"),
        None => format!("<sip:{USER}@harness.lan>"),
    };
    format!(
        "INVITE sip:{USER}@{client} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {pbx};branch=z9hG4bKinv{cseq}\r\n\
         From: <sip:200@harness.lan>;tag=caller1\r\n\
         To: {to}\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: {cseq} INVITE\r\n\
         Contact: <sip:200@{pbx}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n{sdp}",
        sdp.len(),
    )
}

/// A response to `req` (an INVITE we sent) carrying `sdp`.
fn respond_sdp(req: &str, code: u16, reason: &str, contact: &str, sdp: &str) -> String {
    let head = respond(
        req,
        code,
        reason,
        &[
            contact.to_owned(),
            "Content-Type: application/sdp".to_owned(),
        ],
    );
    let head = head.trim_end_matches("Content-Length: 0\r\n\r\n");
    format!("{head}Content-Length: {}\r\n\r\n{sdp}", sdp.len())
}

/// The client's final response to the INVITE with this CSeq, waited for.
async fn final_response(seen: &Arc<Mutex<Seen>>, cseq: u32) -> String {
    let want = format!("{cseq} INVITE");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let found = seen
            .lock()
            .unwrap()
            .responses
            .iter()
            .rev()
            .find(|r| header(r, "CSeq") == Some(want.as_str()) && !r.starts_with("SIP/2.0 1"))
            .cloned();
        if let Some(r) = found {
            return r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no final response to INVITE {cseq}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn body(msg: &str) -> &str {
    msg.split("\r\n\r\n").nth(1).unwrap_or("")
}

/// The port and payload types of an SDP's audio stream.
fn m_line(sdp: &str) -> (u16, Vec<u8>) {
    let line = sdp
        .lines()
        .find(|l| l.starts_with("m=audio "))
        .expect("an audio m= line");
    let mut parts = line.split_whitespace().skip(1);
    let port = parts.next().unwrap().parse().unwrap();
    let pts = parts.skip(1).map(|p| p.parse().unwrap()).collect();
    (port, pts)
}

/// The `o=` line's session id and version.
fn origin(sdp: &str) -> (String, String) {
    let line = sdp
        .lines()
        .find(|l| l.starts_with("o="))
        .expect("an o= line");
    let f: Vec<&str> = line.split_whitespace().collect();
    (f[1].to_owned(), f[2].to_owned())
}

/// What the call's running session looks like now.
#[derive(Debug, PartialEq)]
struct Session {
    ssrc: u32,
    remote: SocketAddr,
    codec: CodecType,
    muted: bool,
}

async fn session(mgr: &SipManager, call_id: &str) -> Option<Session> {
    let s = mgr.state.read().await;
    let (_, call) = s.find_call(call_id)?;
    call.media_any().map(|m| Session {
        ssrc: m.ssrc(),
        remote: m.remote_addr(),
        codec: m.codec(),
        muted: m.is_muted(),
    })
}

/// A registered client on `srtp`, with the PBX ACKing every 200 OK.
async fn client(
    srtp: SrtpMode,
    invite_reply: InviteReply,
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<crate::sip::SipEvent>,
    Arc<Mutex<Seen>>,
    Arc<UdpSocket>,
    SocketAddr,
    SocketAddr,
) {
    let opts = PbxOptions {
        ack_invite_200: true,
        invite_reply,
        ..Default::default()
    };
    let (pbx, seen, sock) = start_pbx_with_options(opts).await;
    let (mgr, rx) = manager_with(fast_timing());
    let mut cfg = account(pbx, None, PASSWORD);
    cfg.srtp_mode = srtp;
    register(&mgr, cfg).await.expect("registered");
    let client = seen.lock().unwrap().client.expect("client address");
    (mgr, rx, seen, sock, pbx, client)
}

/// Ring the client with `sdp` and answer; returns the call id and our 200.
async fn answered(
    mgr: &SipManager,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sip::SipEvent>,
    seen: &Arc<Mutex<Seen>>,
    sock: &UdpSocket,
    (pbx, client): (SocketAddr, SocketAddr),
    sip_call_id: &str,
    sdp: &str,
) -> (String, String) {
    sock.send_to(
        invite(pbx, client, sip_call_id, 1, None, sdp).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let call_id = wait_for_call_state(rx, "incoming").await;
    mgr.answer(&call_id).await.expect("answered");
    (call_id, final_response(seen, 1).await)
}

/// M1: a Yealink offers G.722 first. The answer said PCMU, and the session
/// used to run G.722: no audio either way. Both now come from one choice.
#[tokio::test]
async fn a_g722_first_offer_is_answered_and_run_on_the_same_codec() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(
        40000,
        "RTP/AVP",
        "9 0 8 101",
        "a=rtpmap:9 G722/8000\r\na=rtpmap:101 telephone-event/8000\r\n",
    );
    let (call_id, ok) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        "media-g722@pbx",
        &offer,
    )
    .await;

    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    let (_, pts) = m_line(body(&ok));
    // The default preferences rank Opus, then PCMU: PCMU is the best offered.
    assert_eq!(pts, vec![0, 8, 101], "{ok}");
    assert_eq!(
        session(&mgr, &call_id).await.expect("media").codec,
        CodecType::Pcmu
    );
}

/// A G.729-only offer, as a bandwidth-limited PBX or trunk sends, is answered
/// on G.729 and the session runs on it.
#[tokio::test]
async fn a_g729_only_offer_is_answered_and_run_on_g729() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(
        40000,
        "RTP/AVP",
        "18 101",
        "a=rtpmap:18 G729/8000\r\na=fmtp:18 annexb=no\r\na=rtpmap:101 telephone-event/8000\r\n",
    );
    let (call_id, ok) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        "media-g729@pbx",
        &offer,
    )
    .await;

    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    let (_, pts) = m_line(body(&ok));
    assert_eq!(pts, vec![18, 101], "{ok}");
    assert_eq!(
        session(&mgr, &call_id).await.expect("media").codec,
        CodecType::G729
    );
}

/// An offer of nothing this phone carries is declined 488, not answered
/// with a PCMU nobody offered.
#[tokio::test]
async fn an_offer_with_no_codec_in_common_is_declined_488() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(40000, "RTP/AVP", "9", "a=rtpmap:9 G722/8000\r\n");
    sock.send_to(
        invite(pbx, client, "media-none@pbx", 1, None, &offer).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;
    assert!(mgr.answer(&call_id).await.is_err());
    let reply = final_response(&seen, 1).await;
    assert!(reply.starts_with("SIP/2.0 488"), "{reply}");
}

/// S5/M2/M7: mute and hold are separate. A mute pressed before the call is
/// answered applies once it is; resuming from hold keeps it; unmuting while
/// held does not open the microphone.
#[tokio::test]
async fn mute_survives_hold_and_applies_to_media_that_starts_later() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(40000, "RTP/AVP", "0", "");
    sock.send_to(
        invite(pbx, client, "media-mute@pbx", 1, None, &offer).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;

    // Muted while it rang: no session yet.
    mgr.mute(&call_id, true).await.unwrap();
    mgr.answer(&call_id).await.expect("answered");
    final_response(&seen, 1).await;
    assert!(
        session(&mgr, &call_id).await.unwrap().muted,
        "the mute pressed while ringing was lost"
    );

    // Mute, hold, resume: still muted.
    mgr.hold(&call_id, true).await.unwrap();
    mgr.hold(&call_id, false).await.unwrap();
    assert!(
        session(&mgr, &call_id).await.unwrap().muted,
        "resuming opened a muted microphone"
    );

    // Unmute while held: the microphone stays off until the call resumes.
    mgr.hold(&call_id, true).await.unwrap();
    mgr.mute(&call_id, false).await.unwrap();
    assert!(
        session(&mgr, &call_id).await.unwrap().muted,
        "unmuting a held call opened the microphone"
    );
    mgr.hold(&call_id, false).await.unwrap();
    assert!(
        !session(&mgr, &call_id).await.unwrap().muted,
        "resumed and unmuted, the microphone is on"
    );
}

/// #93: an incoming call's RTP and RTCP ports stay bound from the INVITE
/// to the answer. They used to be chosen, released, and bound again when the
/// call was answered; on Windows, which hands a freed ephemeral port straight
/// back, a socket bound in parallel took one in between and the answer failed
/// with "address in use" (os error 10048). The test takes that socket's part
/// itself: while the call rings, nothing else can bind either port, and the
/// answer starts media on them.
#[tokio::test]
async fn an_incoming_calls_media_ports_stay_bound_until_it_is_answered() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(40000, "RTP/AVP", "0", "");
    sock.send_to(
        invite(pbx, client, "media-held-port@pbx", 1, None, &offer).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;
    let port = mgr
        .state
        .read()
        .await
        .find_call(&call_id)
        .map(|(_, c)| c.local_rtp_port)
        .expect("the ringing call");

    let rtp_thief = std::net::UdpSocket::bind(("0.0.0.0", port));
    let rtcp_thief = std::net::UdpSocket::bind(("0.0.0.0", port + 1));
    assert!(
        rtp_thief.is_err(),
        "the ringing call's RTP port {port} was free for anything to take"
    );
    assert!(
        rtcp_thief.is_err(),
        "the ringing call's RTCP port {} was free for anything to take",
        port + 1
    );

    mgr.answer(&call_id).await.expect("answered");
    let ok = final_response(&seen, 1).await;
    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    assert!(
        ok.contains(&format!("m=audio {port} ")),
        "the answer names the port the call held: {ok}"
    );
    assert!(session(&mgr, &call_id).await.is_some(), "no media session");
}

/// S3/M5: a re-INVITE that moves media gets a new session pointed at the
/// new address, on a new local port that the 200 OK names. A refresh with the
/// same SDP keeps the running session and the same answer, and the `o=` id
/// stays the dialog's throughout.
#[tokio::test]
async fn a_reinvite_that_moves_media_restarts_the_session_and_a_refresh_does_not() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(
        40000,
        "RTP/AVP",
        "0 101",
        "a=rtpmap:101 telephone-event/8000\r\n",
    );
    let sip_call_id = "media-move@pbx";
    let (call_id, ok1) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        sip_call_id,
        &offer,
    )
    .await;
    let to_tag = header(&ok1, "To")
        .and_then(|t| param(t, "tag"))
        .expect("our tag");
    let before = session(&mgr, &call_id).await.expect("media");
    assert_eq!(before.remote, "127.0.0.1:40000".parse().unwrap());
    let (port1, _) = m_line(body(&ok1));
    let (id1, v1) = origin(body(&ok1));

    // A session refresh: the same offer again.
    sock.send_to(
        invite(pbx, client, sip_call_id, 2, Some(&to_tag), &offer).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok2 = final_response(&seen, 2).await;
    assert!(ok2.starts_with("SIP/2.0 200"), "{ok2}");
    assert_eq!(m_line(body(&ok2)).0, port1, "a refresh moved our port");
    assert_eq!(
        origin(body(&ok2)),
        (id1.clone(), v1.clone()),
        "an unchanged answer keeps its o= version"
    );
    assert_eq!(
        session(&mgr, &call_id).await.unwrap(),
        before,
        "a refresh restarted the session"
    );

    // The PBX moves the call to another relay port.
    let moved = pbx_sdp(
        40100,
        "RTP/AVP",
        "0 101",
        "a=rtpmap:101 telephone-event/8000\r\n",
    );
    sock.send_to(
        invite(pbx, client, sip_call_id, 3, Some(&to_tag), &moved).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok3 = final_response(&seen, 3).await;
    assert!(ok3.starts_with("SIP/2.0 200"), "{ok3}");
    let (port3, pts3) = m_line(body(&ok3));
    assert_ne!(
        port3, port1,
        "the new session needs a port the old one has let go of"
    );
    assert_eq!(pts3, vec![0, 101]);
    let (id3, v3) = origin(body(&ok3));
    assert_eq!(id3, id1, "the o= session id is the dialog's");
    assert_eq!(
        v3.parse::<u64>().unwrap(),
        v1.parse::<u64>().unwrap() + 1,
        "a changed answer bumps the version"
    );
    let after = session(&mgr, &call_id).await.expect("media");
    assert_eq!(
        after.remote,
        "127.0.0.1:40100".parse().unwrap(),
        "media still goes to the old relay port"
    );
    assert_ne!(after.ssrc, before.ssrc, "the session was not replaced");
    let port_on_call = mgr
        .state
        .read()
        .await
        .find_call(&call_id)
        .map(|(_, c)| c.local_rtp_port);
    assert_eq!(port_on_call, Some(port3));
}

/// A codec change and a hold. The far end holding us (`sendonly`) is
/// answered `recvonly` and we stop sending; resuming (`sendrecv`) is
/// answered in kind and the microphone comes back. Neither moves media.
#[tokio::test]
async fn a_hold_reinvite_is_mirrored_and_a_codec_change_restarts() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::Busy).await;
    let offer = pbx_sdp(40000, "RTP/AVP", "0 8", "");
    let sip_call_id = "media-hold@pbx";
    let (call_id, ok1) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        sip_call_id,
        &offer,
    )
    .await;
    let to_tag = header(&ok1, "To").and_then(|t| param(t, "tag")).unwrap();
    let before = session(&mgr, &call_id).await.unwrap();
    assert!(!before.muted);

    let held = pbx_sdp(40000, "RTP/AVP", "0 8", "a=sendonly\r\n");
    sock.send_to(
        invite(pbx, client, sip_call_id, 2, Some(&to_tag), &held).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok2 = final_response(&seen, 2).await;
    assert!(body(&ok2).contains("a=recvonly\r\n"), "{ok2}");
    let during = session(&mgr, &call_id).await.unwrap();
    assert_eq!(during.ssrc, before.ssrc, "a hold restarted the session");
    assert!(during.muted, "held by the far end, we must not send");

    let resumed = pbx_sdp(40000, "RTP/AVP", "0 8", "a=sendrecv\r\n");
    sock.send_to(
        invite(pbx, client, sip_call_id, 3, Some(&to_tag), &resumed).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok3 = final_response(&seen, 3).await;
    assert!(body(&ok3).contains("a=sendrecv\r\n"), "{ok3}");
    assert!(!session(&mgr, &call_id).await.unwrap().muted);

    // PCMA only now: the session must run PCMA.
    let pcma = pbx_sdp(40000, "RTP/AVP", "8", "");
    sock.send_to(
        invite(pbx, client, sip_call_id, 4, Some(&to_tag), &pcma).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok4 = final_response(&seen, 4).await;
    assert_eq!(m_line(body(&ok4)).1, vec![8], "{ok4}");
    let after = session(&mgr, &call_id).await.unwrap();
    assert_eq!(after.codec, CodecType::Pcma);
    assert_ne!(after.ssrc, before.ssrc);

    // Nothing in common: 488, and the call carries on as it was.
    let g722 = pbx_sdp(40000, "RTP/AVP", "9", "a=rtpmap:9 G722/8000\r\n");
    sock.send_to(
        invite(pbx, client, sip_call_id, 5, Some(&to_tag), &g722).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok5 = final_response(&seen, 5).await;
    assert!(ok5.starts_with("SIP/2.0 488"), "{ok5}");
    assert_eq!(session(&mgr, &call_id).await.unwrap().ssrc, after.ssrc);
}

fn srtp_key() -> String {
    rtp_engine::srtp::SrtpContext::generate()
        .expect("an SRTP key")
        .1
}

fn crypto(key: &str) -> String {
    format!("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:{key}\r\n")
}

/// The attended-transfer re-INVITE on an SRTP call: same relay address, a
/// new key. The old session would fail authentication on everything that
/// came back; the answer stays RTP/SAVP with our crypto line.
#[tokio::test]
async fn an_srtp_rekey_reinvite_restarts_and_stays_encrypted() {
    let (mgr, mut rx, seen, sock, pbx, client) = client(SrtpMode::Sdes, InviteReply::Busy).await;
    let offer = pbx_sdp(40000, "RTP/SAVP", "0", &crypto(&srtp_key()));
    let sip_call_id = "media-rekey@pbx";
    let (call_id, ok1) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        sip_call_id,
        &offer,
    )
    .await;
    assert!(body(&ok1).contains("RTP/SAVP"), "{ok1}");
    let to_tag = header(&ok1, "To").and_then(|t| param(t, "tag")).unwrap();
    let before = session(&mgr, &call_id).await.unwrap();

    let rekeyed = pbx_sdp(40000, "RTP/SAVP", "0", &crypto(&srtp_key()));
    sock.send_to(
        invite(pbx, client, sip_call_id, 2, Some(&to_tag), &rekeyed).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok2 = final_response(&seen, 2).await;
    assert!(
        body(&ok2).contains("m=audio ") && body(&ok2).contains("RTP/SAVP"),
        "{ok2}"
    );
    assert!(
        body(&ok2).contains("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:"),
        "{ok2}"
    );
    assert_ne!(
        session(&mgr, &call_id).await.unwrap().ssrc,
        before.ssrc,
        "a new key needs a new session"
    );
    let srtp = mgr
        .state
        .read()
        .await
        .find_call(&call_id)
        .map(|(_, c)| c.srtp_active);
    assert_eq!(srtp, Some(true));
}

/// Every `srtp_active` the call events for `call_id` carried, until none has
/// arrived for a while.
async fn srtp_updates(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sip::SipEvent>,
    call_id: &str,
) -> Vec<(String, bool)> {
    let mut seen = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
        if let crate::sip::SipEvent::CallStateChanged(e) = event {
            if let (true, Some(srtp)) = (e.call_id == call_id, e.srtp_active) {
                seen.push((e.state, srtp));
            }
        }
    }
    seen
}

/// FreeSWITCH-style optional SRTP: an `RTP/AVP` offer with a crypto line,
/// answered in plain RTP. The offer's key was recorded as what the session
/// ran against, so the first session refresh, with the very same SDP,
/// looked like a re-key and restarted the media on a new port.
#[tokio::test]
async fn a_refresh_of_a_plain_answer_to_an_offer_with_crypto_keeps_the_session() {
    let (mgr, mut rx, seen, sock, pbx, client) = client(SrtpMode::Sdes, InviteReply::Busy).await;
    let offer = pbx_sdp(40000, "RTP/AVP", "0", &crypto(&srtp_key()));
    let sip_call_id = "media-optional-srtp@pbx";
    let (call_id, ok1) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        sip_call_id,
        &offer,
    )
    .await;
    assert!(
        body(&ok1).contains("RTP/AVP") && !body(&ok1).contains("a=crypto"),
        "{ok1}"
    );
    let to_tag = header(&ok1, "To").and_then(|t| param(t, "tag")).unwrap();
    let before = session(&mgr, &call_id).await.unwrap();

    sock.send_to(
        invite(pbx, client, sip_call_id, 2, Some(&to_tag), &offer).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok2 = final_response(&seen, 2).await;
    assert!(ok2.starts_with("SIP/2.0 200"), "{ok2}");
    assert_eq!(
        m_line(body(&ok2)).0,
        m_line(body(&ok1)).0,
        "a refresh moved our port"
    );
    assert_eq!(
        origin(body(&ok2)),
        origin(body(&ok1)),
        "an unchanged answer keeps its o= version"
    );
    assert_eq!(
        session(&mgr, &call_id).await.unwrap(),
        before,
        "a refresh restarted the session"
    );
}

/// A re-INVITE that turns encryption on, then off again, moves the call
/// screen's padlock each time: the call event the padlock reads is sent
/// with the new `srtp_active`, in the call's own state. It used to keep
/// showing what the first answer reported. A refresh that changes nothing
/// sends nothing.
#[tokio::test]
async fn a_reinvite_that_changes_encryption_updates_the_padlock() {
    let (mgr, mut rx, seen, sock, pbx, client) = client(SrtpMode::Sdes, InviteReply::Busy).await;
    let plain = pbx_sdp(40000, "RTP/AVP", "0", "");
    let sip_call_id = "media-padlock@pbx";
    let (call_id, ok1) = answered(
        &mgr,
        &mut rx,
        &seen,
        &sock,
        (pbx, client),
        sip_call_id,
        &plain,
    )
    .await;
    let to_tag = header(&ok1, "To").and_then(|t| param(t, "tag")).unwrap();
    // The "connected" event reports the answer's state.
    assert_eq!(
        srtp_updates(&mut rx, &call_id).await,
        vec![("connected".to_owned(), false)]
    );

    let encrypted = pbx_sdp(40000, "RTP/SAVP", "0", &crypto(&srtp_key()));
    sock.send_to(
        invite(pbx, client, sip_call_id, 2, Some(&to_tag), &encrypted).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let ok2 = final_response(&seen, 2).await;
    assert!(body(&ok2).contains("a=crypto"), "{ok2}");
    assert_eq!(
        srtp_updates(&mut rx, &call_id).await,
        vec![("connected".to_owned(), true)]
    );

    sock.send_to(
        invite(pbx, client, sip_call_id, 3, Some(&to_tag), &encrypted).as_bytes(),
        client,
    )
    .await
    .unwrap();
    final_response(&seen, 3).await;
    assert_eq!(
        srtp_updates(&mut rx, &call_id).await,
        vec![],
        "a refresh changed nothing"
    );

    sock.send_to(
        invite(pbx, client, sip_call_id, 4, Some(&to_tag), &plain).as_bytes(),
        client,
    )
    .await
    .unwrap();
    final_response(&seen, 4).await;
    assert_eq!(
        srtp_updates(&mut rx, &call_id).await,
        vec![("connected".to_owned(), false)]
    );
}

/// Place a call that the PBX rings (180) and leaves to the test; returns
/// the call id and the INVITE it saw.
async fn ringing_outbound(
    mgr: &SipManager,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sip::SipEvent>,
    seen: &Arc<Mutex<Seen>>,
) -> (String, String) {
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .expect("call placed");
    call_states_until(rx, &call_id, "ringing", Duration::from_secs(5)).await;
    let invite = seen
        .lock()
        .unwrap()
        .invites
        .last()
        .cloned()
        .expect("the INVITE");
    (call_id, invite)
}

/// The "connected" event's `srtp_active` for `call_id`.
async fn connected_srtp(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::sip::SipEvent>,
    call_id: &str,
) -> Option<bool> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("connected within 5s")
            .expect("events");
        if let crate::sip::SipEvent::CallStateChanged(e) = event {
            if e.call_id == call_id && e.state == "connected" {
                return e.srtp_active;
            }
        }
    }
}

/// #93, the dialling side: the RTP and RTCP ports our INVITE offers stay
/// bound while the call rings, and the 200 OK's media starts on them. They
/// used to be released after the STUN probe and bound again on the answer.
#[tokio::test]
async fn an_outbound_calls_offered_ports_stay_bound_until_it_is_answered() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Disabled, InviteReply::AnswerOnCancel).await;
    let (call_id, inv) = ringing_outbound(&mgr, &mut rx, &seen).await;
    let port = mgr
        .state
        .read()
        .await
        .find_call(&call_id)
        .map(|(_, c)| c.local_rtp_port)
        .expect("the ringing call");
    assert!(
        body(&inv).contains(&format!("m=audio {port} ")),
        "the INVITE offers the call's port: {inv}"
    );
    assert!(
        std::net::UdpSocket::bind(("0.0.0.0", port)).is_err(),
        "the offered RTP port {port} was free while the call rang"
    );
    assert!(
        std::net::UdpSocket::bind(("0.0.0.0", port + 1)).is_err(),
        "the offered RTCP port {} was free while the call rang",
        port + 1
    );

    let contact = format!("Contact: <sip:11@{pbx}>");
    let sdp = pbx_sdp(40300, "RTP/AVP", "0", "");
    sock.send_to(
        respond_sdp(&inv, 200, "OK", &contact, &sdp).as_bytes(),
        client,
    )
    .await
    .unwrap();
    assert_eq!(connected_srtp(&mut rx, &call_id).await, Some(false));
    assert!(session(&mgr, &call_id).await.is_some(), "no media session");
    mgr.hangup(&call_id).await.unwrap();
}

/// M6/M7: an SRTP call that rings with early media (183 with SDP) and is
/// answered with the same SDP connects on that session, shows encrypted, and
/// a mute and DTMF during the early media reach it.
#[tokio::test]
async fn an_srtp_call_answered_on_its_early_media_shows_encrypted() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Sdes, InviteReply::AnswerOnCancel).await;
    let (call_id, inv) = ringing_outbound(&mgr, &mut rx, &seen).await;
    assert!(body(&inv).contains("RTP/SAVP"), "{inv}");

    let contact = format!("Contact: <sip:11@{pbx}>");
    let sdp = pbx_sdp(
        40200,
        "RTP/SAVP",
        "0 101",
        &format!(
            "a=rtpmap:101 telephone-event/8000\r\n{}",
            crypto(&srtp_key())
        ),
    );
    sock.send_to(
        respond_sdp(&inv, 183, "Session Progress", &contact, &sdp).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let early = loop {
        if let Some(s) = session(&mgr, &call_id).await {
            break s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no early media session"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    // Mute and the keypad reach early media (an IVR in a 183).
    mgr.mute(&call_id, true).await.unwrap();
    assert!(session(&mgr, &call_id).await.unwrap().muted);
    mgr.send_dtmf(&call_id, "5")
        .await
        .expect("DTMF during early media");

    sock.send_to(
        respond_sdp(&inv, 200, "OK", &contact, &sdp).as_bytes(),
        client,
    )
    .await
    .unwrap();
    assert_eq!(
        connected_srtp(&mut rx, &call_id).await,
        Some(true),
        "an SRTP call on early media showed unencrypted"
    );
    let after = session(&mgr, &call_id).await.unwrap();
    assert_eq!(
        after.ssrc, early.ssrc,
        "the same SDP must not restart the session"
    );
    assert!(
        after.muted,
        "the mute pressed during early media was lost on connect"
    );
    mgr.hangup(&call_id).await.unwrap();
}

/// M6: the 200 OK points somewhere else than the 183 did (another port and
/// key). Media follows the 200, on the port our offer named.
#[tokio::test]
async fn a_200_that_moves_early_media_restarts_on_the_offered_port() {
    let (mgr, mut rx, seen, sock, pbx, client) =
        client(SrtpMode::Sdes, InviteReply::AnswerOnCancel).await;
    let (call_id, inv) = ringing_outbound(&mgr, &mut rx, &seen).await;
    let offered_port = m_line(body(&inv)).0;

    let contact = format!("Contact: <sip:11@{pbx}>");
    let early_sdp = pbx_sdp(40300, "RTP/SAVP", "0", &crypto(&srtp_key()));
    sock.send_to(
        respond_sdp(&inv, 183, "Session Progress", &contact, &early_sdp).as_bytes(),
        client,
    )
    .await
    .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let early = loop {
        if let Some(s) = session(&mgr, &call_id).await {
            break s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no early media session"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(early.remote, "127.0.0.1:40300".parse().unwrap());

    let answer_sdp = pbx_sdp(40302, "RTP/SAVP", "0", &crypto(&srtp_key()));
    sock.send_to(
        respond_sdp(&inv, 200, "OK", &contact, &answer_sdp).as_bytes(),
        client,
    )
    .await
    .unwrap();
    assert_eq!(connected_srtp(&mut rx, &call_id).await, Some(true));

    // The early session's RTCP task holds the port for up to five seconds.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(9);
    let moved = loop {
        if let Some(s) = session(&mgr, &call_id)
            .await
            .filter(|s| s.ssrc != early.ssrc)
        {
            break s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "media never moved to the 200's address"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(moved.remote, "127.0.0.1:40302".parse().unwrap());
    let call = mgr
        .state
        .read()
        .await
        .find_call(&call_id)
        .map(|(_, c)| (c.local_rtp_port, c.srtp_active));
    assert_eq!(call, Some((offered_port, true)));
    mgr.hangup(&call_id).await.unwrap();
}
