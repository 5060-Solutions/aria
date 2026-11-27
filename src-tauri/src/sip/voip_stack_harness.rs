//! End-to-end harness for issue #4: the real `SipManager` against a fake PBX
//! that behaves like the reporter's Home Assistant `voip_stack`.
//!
//! The fake PBX does what voip_stack does, taken from its source:
//!
//! - challenges REGISTER with `Digest realm="voip_stack", ..., qop="auth"`;
//! - verifies the digest over **its own** realm, and answers a wrong one with a
//!   fresh 401 (not a 403, so the client's 403 fallback never runs);
//! - reads the SDP of every INVITE, and a `0.0.0.0` connection address means it
//!   withholds audio (RFC 3264 hold) — which is what the reporter heard.
//!
//! Each test drives the actual registration and call code over real UDP on
//! loopback, so it exercises the same paths a user does; only the far end is
//! fake. The digest is checked here with an independent MD5, not the client's.

use super::account::{default_codec_preferences, AccountConfig};
use super::transport::TransportType;
use super::SipManager;
use md5::{Digest, Md5};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Codec negotiation, mute and hold, and media that moves mid-call.
mod media_tests;

const REALM: &str = "voip_stack";
const USER: &str = "ghostbook";
const PASSWORD: &str = "correct-horse";

/// What the fake PBX saw, for the tests to assert on.
#[derive(Default)]
struct Seen {
    /// The realm of every authenticated REGISTER, in order.
    register_realms: Vec<String>,
    /// Whether each authenticated REGISTER's digest verified.
    register_ok: Vec<bool>,
    /// The Expires header of every authenticated REGISTER, in order.
    register_expires: Vec<Option<String>>,
    /// How many REGISTERs arrived without credentials.
    register_unauth: usize,
    /// The Contact header of every REGISTER.
    register_contacts: Vec<String>,
    /// Every SUBSCRIBE, raw, except message-summary ones.
    subscribes: Vec<String>,
    /// Every message-summary (MWI) SUBSCRIBE, raw.
    mwi_subscribes: Vec<String>,
    /// Every INVITE, raw.
    invites: Vec<String>,
    /// How many ACKs arrived.
    acks: usize,
    /// Every ACK, raw.
    ack_msgs: Vec<String>,
    /// Every CANCEL, raw.
    cancels: Vec<String>,
    /// Every BYE, raw.
    byes: Vec<String>,
    /// Where the client registered from, so the PBX can call it.
    client: Option<SocketAddr>,
    /// Every response the client sent the PBX, raw.
    responses: Vec<String>,
    /// The method of every other request the client sent.
    methods: Vec<String>,
    /// Every request the PBX lost on purpose (see `PbxOptions::drop_once`).
    dropped: Vec<String>,
}

/// How the fake PBX answers an INVITE.
#[derive(Clone, Copy, PartialEq)]
enum InviteReply {
    /// 486, ending the attempt; for tests that only care about the INVITE.
    Busy,
    /// 200 OK, the same 200 again as a retransmission (as a peer does when it
    /// has not seen the ACK yet), then a BYE from the far end.
    AnswerTwiceThenHangUp,
    /// 407 for an INVITE without credentials — retransmitted twice, as a UAS
    /// does until it sees the ACK — then 200 OK for the authenticated one.
    ChallengeThenAnswer,
    /// 180 Ringing, and a 200 OK sent the moment the caller's CANCEL arrives
    /// (and resent once, as the ACK has not reached it yet): the callee
    /// picked up as the caller gave up.
    AnswerOnCancel,
    /// 100 Trying, then nothing for this long, then 180 and 200 OK: a PSTN
    /// call whose carrier takes its time.
    TryingThenLateAnswer(Duration),
    /// 100 Trying and 180 Ringing; a CANCEL gets its 200 and the INVITE a 487.
    RingUntilCancelled,
    /// No response to an INVITE until a CANCEL arrives; then the CANCEL gets
    /// its 200 and the INVITE a 487.
    SilentUntilCancelled,
    /// A 407 for an INVITE without credentials, but only after this long;
    /// 200 OK for the authenticated one.
    DelayedChallenge(Duration),
    /// No response to an INVITE at all.
    Silent,
}

/// Responses queued for the fake PBX to send: a status and extra headers.
type ScriptedReplies = Arc<Mutex<Vec<(u16, Vec<String>)>>>;

/// How the fake PBX behaves beyond its defaults.
#[derive(Clone)]
struct PbxOptions {
    invite_reply: InviteReply,
    /// The Expires it grants a registration.
    register_expires: u32,
    /// Set by a test to make the PBX swallow the next REGISTER transaction:
    /// every copy of the next REGISTER it sees (same CSeq) goes unanswered.
    drop_next_register: Arc<AtomicBool>,
    /// Challenge every SUBSCRIBE and never accept the answer.
    reject_subscribe_auth: bool,
    /// The Expires it grants a subscription.
    subscribe_expires: u32,
    /// Answer every REGISTER with `503` and this `Retry-After` (seconds).
    register_503: Option<u32>,
    /// Never answer OPTIONS.
    ignore_options: bool,
    /// ACK every 200 OK the client sends to an INVITE, as a PBX does.
    ack_invite_200: bool,
    /// Lose the first 200 OK the client sends to an INVITE on the wire: the
    /// PBX never sees it (#69).
    drop_first_invite_200: bool,
    /// Answer a message-summary SUBSCRIBE with a NOTIFY carrying counts, as
    /// the Aria PBX does.
    mwi_notify: bool,
    /// ACK every 4xx-6xx the client sends to an INVITE, as a PBX does.
    ack_invite_final: bool,
    /// Requests to lose on the wire, one copy per entry: a method name, or
    /// `INVITE+auth` for an INVITE carrying credentials. Shared, so a test can
    /// arm it mid-call.
    drop_once: Arc<Mutex<Vec<String>>>,
    /// Methods whose every transaction loses its first copy (by Via branch).
    drop_first_copy_of: Vec<&'static str>,
    /// Answers for the next message-summary SUBSCRIBEs, one per request
    /// copy, before the usual 200: a status and extra headers. Shared, so a
    /// test can arm it mid-subscription.
    mwi_replies: ScriptedReplies,
    /// While set, message-summary SUBSCRIBEs are recorded and not answered.
    ignore_mwi: Arc<AtomicBool>,
}

impl Default for PbxOptions {
    fn default() -> Self {
        Self {
            invite_reply: InviteReply::Busy,
            register_expires: 300,
            drop_next_register: Arc::new(AtomicBool::new(false)),
            reject_subscribe_auth: false,
            subscribe_expires: 600,
            register_503: None,
            ignore_options: false,
            ack_invite_200: false,
            drop_first_invite_200: false,
            mwi_notify: false,
            ack_invite_final: false,
            drop_once: Arc::new(Mutex::new(Vec::new())),
            drop_first_copy_of: Vec::new(),
            mwi_replies: Arc::new(Mutex::new(Vec::new())),
            ignore_mwi: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// A manager with its background loops running, as the app has one: the
/// subscription refresh tests depend on the refresh timer. Construction alone
/// starts nothing (see `SipManager::start_background_tasks`).
fn manager() -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
) {
    manager_with(super::Timing::default())
}

fn manager_with(
    timing: super::Timing,
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
) {
    let (mgr, rx) = SipManager::new_with_timing(timing);
    use_local_stun(&mgr);
    mgr.start_background_tasks();
    (mgr, rx)
}

/// Point `mgr`'s STUN lookups at the in-process stub. Every call the harness
/// places builds an SDP, and with the public servers a lookup that timed out
/// held the INVITE back three seconds per server and failed the tests that
/// time dialling (the emergency-call and failover tests).
fn use_local_stun(mgr: &SipManager) {
    let addr = local_stun_server().to_string();
    mgr.state
        .try_write()
        .expect("a new manager is not shared yet")
        .stun_servers = vec![addr].into();
}

/// A STUN server on loopback that answers every Binding Request with the
/// sender's own address (RFC 5389 XOR-MAPPED-ADDRESS), started once for the
/// whole test process. It runs on a plain thread, not a task, because every
/// `#[tokio::test]` has a runtime of its own that dies with the test.
fn local_stun_server() -> SocketAddr {
    static ADDR: std::sync::OnceLock<SocketAddr> = std::sync::OnceLock::new();
    *ADDR.get_or_init(|| {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind the STUN stub");
        let addr = socket.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            while let Ok((len, from)) = socket.recv_from(&mut buf) {
                if let Some(reply) = stun_binding_success(&buf[..len], from) {
                    let _ = socket.send_to(&reply, from);
                }
            }
        });
        addr
    })
}

/// The Binding Success Response to `request`, mapping it to `from`; `None`
/// for anything that is not an IPv4-answerable Binding Request.
fn stun_binding_success(request: &[u8], from: SocketAddr) -> Option<Vec<u8>> {
    const MAGIC: [u8; 4] = 0x2112_A442u32.to_be_bytes();
    if request.len() < 20 || request[0..2] != [0x00, 0x01] || request[4..8] != MAGIC {
        return None;
    }
    let SocketAddr::V4(from) = from else {
        return None;
    };
    let port = from.port() ^ 0x2112;
    let ip: Vec<u8> = from
        .ip()
        .octets()
        .iter()
        .zip(MAGIC)
        .map(|(b, m)| b ^ m)
        .collect();
    let mut reply = vec![0x01, 0x01, 0x00, 12];
    reply.extend_from_slice(&request[4..20]); // magic cookie and transaction ID
    reply.extend_from_slice(&[0x00, 0x20, 0x00, 8, 0x00, 0x01]); // XOR-MAPPED-ADDRESS, IPv4
    reply.extend_from_slice(&port.to_be_bytes());
    reply.extend_from_slice(&ip);
    Some(reply)
}

/// The harness never asks a public STUN server, and the stub answers at once
/// with the address the probe came from.
#[tokio::test]
async fn harness_managers_ask_only_the_local_stun_stub() {
    let (mgr, _rx) = manager();
    let servers = mgr.state.read().await.stun_servers.clone();
    assert_eq!(&*servers, &[local_stun_server().to_string()]);

    let started = tokio::time::Instant::now();
    let (_sockets, public_ip, public_port) = super::media::allocate_port_with_stun(&servers)
        .await
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(public_ip, Some("127.0.0.1".parse().unwrap()));
    assert!(public_port.is_some_and(|p| p != 0), "{public_port:?}");
}

/// Timers shrunk so a lost REGISTER, its deadline and the reconnect play out
/// in a couple of seconds.
fn fast_timing() -> super::Timing {
    super::Timing {
        t1: Duration::from_millis(50),
        timer_f: Duration::from_millis(400),
        min_refresh: Duration::from_secs(1),
        unregister_wait: Duration::from_secs(2),
        subscribe_tick: Duration::from_millis(200),
        options_interval: Duration::from_secs(15),
        options_wait: Duration::from_secs(5),
        max_wait: Duration::from_secs(2),
        no_answer: Duration::from_secs(30),
    }
}

fn md5_hex(s: &str) -> String {
    hex::encode(Md5::digest(s.as_bytes()))
}

fn header<'a>(msg: &'a str, name: &str) -> Option<&'a str> {
    let want = format!("{}:", name.to_ascii_lowercase());
    msg.lines()
        .find(|l| l.to_ascii_lowercase().starts_with(&want))
        .map(|l| l[want.len()..].trim())
}

fn param(value: &str, name: &str) -> Option<String> {
    let at = value
        .match_indices(&format!("{name}="))
        .find(|&(i, _)| i == 0 || !value.as_bytes()[i - 1].is_ascii_alphanumeric())?
        .0
        + name.len()
        + 1;
    let rest = &value[at..];
    Some(match rest.strip_prefix('"') {
        Some(q) => q[..q.find('"')?].to_owned(),
        None => rest[..rest.find(',').unwrap_or(rest.len())]
            .trim()
            .to_owned(),
    })
}

/// A response echoing the request's dialog headers, as a UAS must.
fn respond(req: &str, code: u16, reason: &str, extra: &[String]) -> String {
    let mut out = format!("SIP/2.0 {code} {reason}\r\n");
    for name in ["Via", "From", "Call-ID", "CSeq"] {
        if let Some(v) = header(req, name) {
            out.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    let to = header(req, "To").unwrap_or_default();
    let to = if to.contains("tag=") || code == 100 {
        to.to_owned()
    } else {
        format!("{to};tag=pbx1")
    };
    out.push_str(&format!("To: {to}\r\n"));
    for h in extra {
        out.push_str(h);
        out.push_str("\r\n");
    }
    out.push_str("Content-Length: 0\r\n\r\n");
    out
}

/// Start the fake PBX on loopback; returns its address and what it records.
async fn start_pbx() -> (SocketAddr, Arc<Mutex<Seen>>) {
    start_pbx_with(InviteReply::Busy).await
}

async fn start_pbx_with(invite_reply: InviteReply) -> (SocketAddr, Arc<Mutex<Seen>>) {
    let (addr, seen, _) = start_pbx_with_socket(invite_reply).await;
    (addr, seen)
}

/// As `start_pbx_with`, also returning the PBX's socket so a test can send the
/// client requests of its own: an incoming call, a CANCEL.
async fn start_pbx_with_socket(
    invite_reply: InviteReply,
) -> (SocketAddr, Arc<Mutex<Seen>>, Arc<UdpSocket>) {
    start_pbx_with_options(PbxOptions {
        invite_reply,
        ..PbxOptions::default()
    })
    .await
}

async fn start_pbx_with_options(
    opts: PbxOptions,
) -> (SocketAddr, Arc<Mutex<Seen>>, Arc<UdpSocket>) {
    let invite_reply = opts.invite_reply;
    let _ = env_logger::builder().is_test(true).try_init();
    let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.expect("bind fake PBX"));
    let addr = sock.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let rec = seen.clone();
    let pbx_sock = sock.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        let mut nonce_n = 0u32;
        // The CSeq of the REGISTER transaction being swallowed, if any.
        let mut dropping_cseq: Option<String> = None;
        // Whether the 200 OK `drop_first_invite_200` loses has gone by.
        let mut dropped_invite_200 = false;
        // The branches `drop_first_copy_of` has already lost a copy of.
        let mut lost_branches = std::collections::HashSet::new();
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                return;
            };
            let msg = String::from_utf8_lossy(&buf[..n]).into_owned();
            let method = msg.split_whitespace().next().unwrap_or("");
            if method == "SIP/2.0" {
                let invite_200 = msg.starts_with("SIP/2.0 200")
                    && header(&msg, "CSeq").is_some_and(|c| c.ends_with("INVITE"));
                if invite_200 && opts.drop_first_invite_200 && !dropped_invite_200 {
                    dropped_invite_200 = true;
                    continue;
                }
                rec.lock().unwrap().responses.push(msg.clone());
                if invite_200 && opts.ack_invite_200 {
                    let ack = ack_for_200(&msg, addr, from);
                    let _ = sock.send_to(ack.as_bytes(), from).await;
                }
                let invite_final = header(&msg, "CSeq").is_some_and(|c| c.ends_with("INVITE"))
                    && msg
                        .split_whitespace()
                        .nth(1)
                        .and_then(|c| c.parse::<u16>().ok())
                        .is_some_and(|c| c >= 300);
                if invite_final && opts.ack_invite_final {
                    let ack = ack_for_200(&msg, addr, from);
                    let _ = sock.send_to(ack.as_bytes(), from).await;
                }
                continue;
            }
            let key = if method == "INVITE" && header(&msg, "Proxy-Authorization").is_some() {
                "INVITE+auth"
            } else {
                method
            };
            let lose_once = {
                let mut once = opts.drop_once.lock().unwrap();
                once.iter()
                    .position(|k| k == key)
                    .map(|i| once.remove(i))
                    .is_some()
            };
            if lose_once
                || (opts.drop_first_copy_of.contains(&method)
                    && lost_branches.insert(via_branch(&msg)))
            {
                rec.lock().unwrap().dropped.push(msg.clone());
                continue;
            }
            if method != "REGISTER" {
                rec.lock().unwrap().methods.push(method.to_owned());
            }
            if method == "REGISTER" {
                let cseq = header(&msg, "CSeq").map(str::to_owned);
                rec.lock()
                    .unwrap()
                    .register_contacts
                    .push(header(&msg, "Contact").unwrap_or_default().to_owned());
                if opts.drop_next_register.swap(false, Ordering::SeqCst) {
                    dropping_cseq = cseq.clone();
                }
                if dropping_cseq.is_some() && dropping_cseq == cseq {
                    continue;
                }
            }
            if method == "OPTIONS" && opts.ignore_options {
                continue;
            }
            let is_mwi = method == "SUBSCRIBE" && header(&msg, "Event") == Some("message-summary");
            let reply = match method {
                // As the Aria PBX does: accept, and send the current counts at
                // once in an in-dialog NOTIFY.
                "SUBSCRIBE" if is_mwi => {
                    rec.lock().unwrap().mwi_subscribes.push(msg.clone());
                    if opts.ignore_mwi.load(Ordering::SeqCst) {
                        continue;
                    }
                    let queued = {
                        let mut replies = opts.mwi_replies.lock().unwrap();
                        (!replies.is_empty()).then(|| replies.remove(0))
                    };
                    if let Some((code, extra)) = queued {
                        let reply = respond(&msg, code, "Scripted", &extra);
                        let _ = sock.send_to(reply.as_bytes(), from).await;
                        continue;
                    }
                    let ok = respond(
                        &msg,
                        200,
                        "OK",
                        &[format!("Expires: {}", opts.subscribe_expires)],
                    );
                    let _ = sock.send_to(ok.as_bytes(), from).await;
                    // Off unless asked for: the client's 200 to the NOTIFY
                    // would land in `responses`, which other tests count.
                    if !opts.mwi_notify {
                        continue;
                    }
                    let body = "Messages-Waiting: yes\r\nVoice-Message: 2/1 (0/0)\r\n";
                    let to = header(&msg, "To").unwrap_or_default();
                    let to = if to.contains("tag=") {
                        to.to_owned()
                    } else {
                        format!("{to};tag=pbx1")
                    };
                    let notify = format!(
                        "NOTIFY sip:{USER}@{from} SIP/2.0\r\n\
                         Via: SIP/2.0/UDP {addr};branch=z9hG4bK-mwi-{n}\r\n\
                         From: {to}\r\n\
                         To: {from_hdr}\r\n\
                         Call-ID: {call_id}\r\n\
                         CSeq: {n} NOTIFY\r\n\
                         Event: message-summary\r\n\
                         Subscription-State: active;expires={expires}\r\n\
                         Content-Type: application/simple-message-summary\r\n\
                         Content-Length: {len}\r\n\r\n{body}",
                        n = nonce_n + 1,
                        from_hdr = header(&msg, "From").unwrap_or_default(),
                        call_id = header(&msg, "Call-ID").unwrap_or_default(),
                        expires = opts.subscribe_expires,
                        len = body.len(),
                    );
                    let _ = sock.send_to(notify.as_bytes(), from).await;
                    continue;
                }
                "SUBSCRIBE" if opts.reject_subscribe_auth => {
                    rec.lock().unwrap().subscribes.push(msg.clone());
                    nonce_n += 1;
                    respond(&msg, 401, "Unauthorized", &[format!(
                        "WWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"n{nonce_n}\", algorithm=MD5, qop=\"auth\""
                    )])
                }
                "SUBSCRIBE" => {
                    rec.lock().unwrap().subscribes.push(msg.clone());
                    respond(
                        &msg,
                        200,
                        "OK",
                        &[format!("Expires: {}", opts.subscribe_expires)],
                    )
                }
                "REGISTER" if opts.register_503.is_some() => {
                    rec.lock().unwrap().register_unauth += 1;
                    respond(
                        &msg,
                        503,
                        "Service Unavailable",
                        &[format!("Retry-After: {}", opts.register_503.unwrap())],
                    )
                }
                "REGISTER" => match header(&msg, "Authorization") {
                    None => {
                        rec.lock().unwrap().register_unauth += 1;
                        nonce_n += 1;
                        respond(&msg, 401, "Unauthorized", &[format!(
                            "WWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"n{nonce_n}\", algorithm=MD5, qop=\"auth\""
                        )])
                    }
                    Some(auth) => {
                        let realm = param(auth, "realm").unwrap_or_default();
                        let ha1 = md5_hex(&format!("{USER}:{REALM}:{PASSWORD}"));
                        let ha2 = md5_hex(&format!(
                            "REGISTER:{}",
                            param(auth, "uri").unwrap_or_default()
                        ));
                        let expected = match param(auth, "qop") {
                            Some(qop) => md5_hex(&format!(
                                "{ha1}:{}:{}:{}:{qop}:{ha2}",
                                param(auth, "nonce").unwrap_or_default(),
                                param(auth, "nc").unwrap_or_default(),
                                param(auth, "cnonce").unwrap_or_default()
                            )),
                            None => md5_hex(&format!(
                                "{ha1}:{}:{ha2}",
                                param(auth, "nonce").unwrap_or_default()
                            )),
                        };
                        let ok = param(auth, "username").as_deref() == Some(USER)
                            && param(auth, "response").as_deref() == Some(expected.as_str());
                        {
                            let mut s = rec.lock().unwrap();
                            s.register_realms.push(realm);
                            s.register_ok.push(ok);
                            s.register_expires
                                .push(header(&msg, "Expires").map(str::to_owned));
                        }
                        if ok {
                            rec.lock().unwrap().client = Some(from);
                            let contact = header(&msg, "Contact").unwrap_or_default().to_owned();
                            let expires = if header(&msg, "Expires") == Some("0") {
                                0
                            } else {
                                opts.register_expires
                            };
                            respond(
                                &msg,
                                200,
                                "OK",
                                &[
                                    format!("Contact: {contact};expires={expires}"),
                                    format!("Expires: {expires}"),
                                ],
                            )
                        } else {
                            // voip_stack answers a bad digest with a fresh
                            // challenge, not a 403.
                            nonce_n += 1;
                            respond(&msg, 401, "Unauthorized", &[format!(
                                "WWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"n{nonce_n}\", algorithm=MD5, qop=\"auth\""
                            )])
                        }
                    }
                },
                "INVITE" if invite_reply == InviteReply::AnswerTwiceThenHangUp => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    let _ = sock
                        .send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from)
                        .await;
                    // No SDP in the answer, so no audio device is opened.
                    let ok = respond(&msg, 200, "OK", &[format!("Contact: <sip:11@{addr}>")]);
                    let bye = format!(
                        "BYE sip:{USER}@{from} SIP/2.0\r\n\
                         Via: SIP/2.0/UDP {addr};branch=z9hG4bKbye1\r\n\
                         From: {};tag=pbx1\r\n\
                         To: {}\r\n\
                         Call-ID: {}\r\n\
                         CSeq: 1 BYE\r\n\
                         Content-Length: 0\r\n\r\n",
                        header(&msg, "To").unwrap_or_default(),
                        header(&msg, "From").unwrap_or_default(),
                        header(&msg, "Call-ID").unwrap_or_default(),
                    );
                    let sock = sock.clone();
                    tokio::spawn(async move {
                        // Spaced like real retransmissions (T1 = 500 ms).
                        for msg in [&ok, &ok, &bye] {
                            let _ = sock.send_to(msg.as_bytes(), from).await;
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                    });
                    continue;
                }
                "INVITE" if invite_reply == InviteReply::ChallengeThenAnswer => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    if header(&msg, "Proxy-Authorization").is_none() {
                        let challenge = respond(&msg, 407, "Proxy Authentication Required", &[format!(
                            "Proxy-Authenticate: Digest realm=\"{REALM}\", nonce=\"inv1\", algorithm=MD5, qop=\"auth\""
                        )]);
                        let sock = sock.clone();
                        tokio::spawn(async move {
                            // Timer G: the UAS resends the 407 until the ACK
                            // matches its transaction.
                            for _ in 0..3 {
                                let _ = sock.send_to(challenge.as_bytes(), from).await;
                                tokio::time::sleep(Duration::from_millis(300)).await;
                            }
                        });
                        continue;
                    }
                    let _ = sock
                        .send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from)
                        .await;
                    respond(&msg, 200, "OK", &[format!("Contact: <sip:11@{addr}>")])
                }
                "INVITE" if invite_reply == InviteReply::AnswerOnCancel => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    let _ = sock
                        .send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from)
                        .await;
                    respond(&msg, 180, "Ringing", &[])
                }
                "CANCEL" if invite_reply == InviteReply::AnswerOnCancel => {
                    let invite = rec.lock().unwrap().invites.last().cloned();
                    rec.lock().unwrap().cancels.push(msg.clone());
                    let _ = sock
                        .send_to(respond(&msg, 200, "OK", &[]).as_bytes(), from)
                        .await;
                    // The callee answered at the same moment: the 200 for the
                    // INVITE goes out after the 200 for the CANCEL, and again
                    // as a retransmission before the ACK can have arrived.
                    let Some(invite) = invite else { continue };
                    let ok = respond(&invite, 200, "OK", &[format!("Contact: <sip:11@{addr}>")]);
                    let sock = sock.clone();
                    tokio::spawn(async move {
                        for _ in 0..2 {
                            let _ = sock.send_to(ok.as_bytes(), from).await;
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    });
                    continue;
                }
                "INVITE" if matches!(invite_reply, InviteReply::TryingThenLateAnswer(_)) => {
                    let InviteReply::TryingThenLateAnswer(delay) = invite_reply else {
                        unreachable!()
                    };
                    rec.lock().unwrap().invites.push(msg.clone());
                    let _ = sock
                        .send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from)
                        .await;
                    let ringing = respond(&msg, 180, "Ringing", &[]);
                    let ok = respond(&msg, 200, "OK", &[format!("Contact: <sip:11@{addr}>")]);
                    let sock = sock.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = sock.send_to(ringing.as_bytes(), from).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let _ = sock.send_to(ok.as_bytes(), from).await;
                    });
                    continue;
                }
                "INVITE" if invite_reply == InviteReply::RingUntilCancelled => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    let _ = sock
                        .send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from)
                        .await;
                    respond(&msg, 180, "Ringing", &[])
                }
                "INVITE" if invite_reply == InviteReply::SilentUntilCancelled => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    continue;
                }
                "CANCEL"
                    if matches!(
                        invite_reply,
                        InviteReply::RingUntilCancelled | InviteReply::SilentUntilCancelled
                    ) =>
                {
                    let invite = rec.lock().unwrap().invites.last().cloned();
                    rec.lock().unwrap().cancels.push(msg.clone());
                    let _ = sock
                        .send_to(respond(&msg, 200, "OK", &[]).as_bytes(), from)
                        .await;
                    match invite {
                        Some(invite) => respond(&invite, 487, "Request Terminated", &[]),
                        None => continue,
                    }
                }
                "INVITE" if matches!(invite_reply, InviteReply::DelayedChallenge(_)) => {
                    let InviteReply::DelayedChallenge(delay) = invite_reply else {
                        unreachable!()
                    };
                    rec.lock().unwrap().invites.push(msg.clone());
                    if header(&msg, "Proxy-Authorization").is_some() {
                        let _ = sock
                            .send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from)
                            .await;
                        respond(&msg, 200, "OK", &[format!("Contact: <sip:11@{addr}>")])
                    } else {
                        let challenge = respond(&msg, 407, "Proxy Authentication Required", &[format!(
                            "Proxy-Authenticate: Digest realm=\"{REALM}\", nonce=\"inv1\", algorithm=MD5, qop=\"auth\""
                        )]);
                        let sock = sock.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(delay).await;
                            let _ = sock.send_to(challenge.as_bytes(), from).await;
                        });
                        continue;
                    }
                }
                "INVITE" if invite_reply == InviteReply::Silent => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    continue;
                }
                "INVITE" => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    // Enough to end the attempt cleanly; the SDP is the point.
                    respond(&msg, 486, "Busy Here", &[])
                }
                "ACK" => {
                    let mut s = rec.lock().unwrap();
                    s.acks += 1;
                    s.ack_msgs.push(msg.clone());
                    continue;
                }
                "BYE" => {
                    rec.lock().unwrap().byes.push(msg.clone());
                    respond(&msg, 200, "OK", &[])
                }
                "CANCEL" => {
                    rec.lock().unwrap().cancels.push(msg.clone());
                    respond(&msg, 200, "OK", &[])
                }
                _ => respond(&msg, 200, "OK", &[]),
            };
            let _ = sock.send_to(reply.as_bytes(), from).await;
        }
    });
    (addr, seen, pbx_sock)
}

fn account(pbx: SocketAddr, auth_realm: Option<&str>, password: &str) -> AccountConfig {
    AccountConfig {
        id: format!("harness-{}", uuid::Uuid::new_v4()),
        display_name: "ghostbook".into(),
        username: USER.into(),
        // The reporter's domain; the wizard used to save it as the realm.
        domain: "harness.lan".into(),
        registrar: Some(pbx.ip().to_string()),
        port: pbx.port(),
        password: password.into(),
        transport: TransportType::Udp,
        outbound_proxy: None,
        auth_username: None,
        auth_realm: auth_realm.map(str::to_owned),
        tls_server_name: None,
        enabled: true,
        auto_record: false,
        tls_insecure: false,
        srtp_mode: Default::default(),
        codecs: default_codec_preferences(),
    }
}

/// Register and wait for a settled outcome: `Ok` when registered, `Err` with
/// the reason when it failed, bounded so a retry loop cannot hang the test.
async fn register(mgr: &SipManager, cfg: AccountConfig) -> Result<(), String> {
    // `register` returns a status word, not the account id.
    let id = cfg.id.clone();
    mgr.register(cfg).await?;
    for _ in 0..100 {
        let (state, err) = mgr.registration_state_for_account(Some(&id)).await;
        match state {
            super::RegistrationState::Registered => return Ok(()),
            super::RegistrationState::Error => return Err(err.unwrap_or_default()),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    Err("registration did not settle in 10s".into())
}

/// The reported case: the wizard saved `harness.lan` as the realm override.
/// voip_stack rejects that digest with a fresh 401; the client must retry
/// with the realm the server announced, and register.
#[tokio::test]
async fn a_wizard_saved_domain_realm_recovers_to_the_servers_realm() {
    let (pbx, seen) = start_pbx().await;
    let (mgr, _rx) = manager();
    let result = register(&mgr, account(pbx, Some("harness.lan"), PASSWORD)).await;
    let s = seen.lock().unwrap();
    assert!(
        result.is_ok(),
        "did not register: {result:?}; realms tried {:?}",
        s.register_realms
    );
    assert_eq!(
        s.register_realms,
        ["harness.lan", REALM],
        "override first, then the server's realm"
    );
    assert_eq!(s.register_ok, [false, true]);
}

#[tokio::test]
async fn a_blank_realm_registers_on_the_first_attempt() {
    let (pbx, seen) = start_pbx().await;
    let (mgr, _rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let s = seen.lock().unwrap();
    assert_eq!(s.register_realms, [REALM]);
    assert_eq!(s.register_ok, [true]);
}

/// The recovery must not turn a wrong password into a registration, or into a
/// retry loop: it fails, after a bounded number of attempts.
#[tokio::test]
async fn a_wrong_password_still_fails_and_does_not_loop() {
    let (pbx, seen) = start_pbx().await;
    let (mgr, _rx) = manager();
    let result = register(&mgr, account(pbx, Some("harness.lan"), "wrong")).await;
    assert!(result.is_err(), "a wrong password registered");
    let s = seen.lock().unwrap();
    assert!(s.register_ok.iter().all(|ok| !ok));
    assert!(
        s.register_realms.len() <= 2,
        "retried {} times",
        s.register_realms.len()
    );
}

/// The audio bug: the INVITE's SDP must name a real address. voip_stack reads
/// `0.0.0.0` as "hold" and sends no media; a LAN PBX must be given the local
/// interface address, not a STUN-discovered public one it cannot reach
/// directly. This runs with whatever STUN does on the machine — failing (the
/// reported case) or succeeding — and both must produce the local address.
#[tokio::test]
async fn an_invite_to_a_lan_pbx_advertises_the_local_address() {
    let (pbx, seen) = start_pbx().await;
    let (mgr, _rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let _ = mgr.make_call("sip:11@harness.lan").await;

    let mut invite = None;
    for _ in 0..150 {
        if let Some(i) = seen.lock().unwrap().invites.first().cloned() {
            invite = Some(i);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let invite = invite.expect("the PBX received an INVITE");
    let body = invite.split("\r\n\r\n").nth(1).unwrap_or_default();
    let c_line = body
        .lines()
        .find(|l| l.starts_with("c="))
        .expect("an SDP c= line");
    let o_line = body
        .lines()
        .find(|l| l.starts_with("o="))
        .expect("an SDP o= line");

    assert!(
        !c_line.contains("0.0.0.0"),
        "voip_stack would withhold audio: {c_line}"
    );
    assert!(!o_line.contains("0.0.0.0"), "{o_line}");
    assert_eq!(
        c_line, "c=IN IP4 127.0.0.1",
        "a LAN PBX must get the local address"
    );
}

/// A retransmitted 200 OK must be acknowledged again and nothing more. Handling
/// it as a new answer used to restart media and auto-recording, splitting the
/// call's audio across two files and announcing the call connected twice. The
/// far end then hangs up, and the ended event must describe the call as it
/// was: this one is outbound, and the BYE path used to report every call as
/// inbound.
#[tokio::test]
async fn a_retransmitted_200_is_acked_but_not_answered_twice() {
    let (pbx, seen) = start_pbx_with(InviteReply::AnswerTwiceThenHangUp).await;
    let (mgr, mut rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .unwrap_or_else(|e| panic!("call placed: {e}"));

    let mut states = Vec::new();
    let mut ended_direction = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while ended_direction.is_none() {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("the call ended within 10s")
            .expect("event channel open");
        if let super::SipEvent::CallStateChanged(e) = event {
            if e.call_id == call_id {
                if e.state == "ended" {
                    ended_direction = Some(e.direction.clone());
                }
                states.push(e.state);
            }
        }
    }

    let connected = states.iter().filter(|s| *s == "connected").count();
    assert_eq!(
        connected, 1,
        "connected reported {connected} times: {states:?}"
    );
    assert_eq!(seen.lock().unwrap().acks, 2, "each 200 needs its own ACK");
    assert_eq!(ended_direction.as_deref(), Some("outbound"));
}

/// An INVITE from the PBX to the registered client, as an incoming call.
fn incoming_invite(
    pbx: SocketAddr,
    client: SocketAddr,
    call_id: &str,
    cseq_method: &str,
) -> String {
    let sdp = "v=0\r\no=pbx 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\n";
    let (method, body) = if cseq_method == "CANCEL" {
        ("CANCEL", "")
    } else {
        ("INVITE", sdp)
    };
    format!(
        "{method} sip:{USER}@{client} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {pbx};branch=z9hG4bKinv1\r\n\
         From: <sip:200@harness.lan>;tag=caller1\r\n\
         To: <sip:{USER}@harness.lan>\r\n\
         Call-ID: {call_id}\r\n\
         CSeq: 1 {method}\r\n\
         Contact: <sip:200@{pbx}>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len(),
    )
}

/// The ACK a PBX sends for a client's 200 OK to its INVITE (RFC 3261
/// §13.2.2.4): a transaction of its own, in the dialog the 200 set up.
fn ack_for_200(ok: &str, pbx: SocketAddr, client: SocketAddr) -> String {
    let cseq = header(ok, "CSeq")
        .and_then(|c| c.split_whitespace().next())
        .unwrap_or("1");
    format!(
        "ACK sip:{USER}@{client} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {pbx};branch=z9hG4bKack{cseq}\r\n\
         From: {}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {cseq} ACK\r\n\
         Content-Length: 0\r\n\r\n",
        header(ok, "From").unwrap_or_default(),
        header(ok, "To").unwrap_or_default(),
        header(ok, "Call-ID").unwrap_or_default(),
    )
}

/// Register, and return the manager, its events, and where the PBX can reach it.
async fn registered_client(
    pbx: SocketAddr,
    seen: &Arc<Mutex<Seen>>,
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    SocketAddr,
) {
    registered_client_with(pbx, seen, super::Timing::default()).await
}

async fn registered_client_with(
    pbx: SocketAddr,
    seen: &Arc<Mutex<Seen>>,
    timing: super::Timing,
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    SocketAddr,
) {
    let (mgr, rx) = manager_with(timing);
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let client = seen.lock().unwrap().client.expect("client address");
    (mgr, rx, client)
}

/// Wait for a call event in `state`, returning its call id.
async fn wait_for_call_state(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    state: &str,
) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("no {state} event within 5s"))
            .expect("event channel open");
        if let super::SipEvent::CallStateChanged(e) = event {
            if e.state == state {
                return e.call_id;
            }
        }
    }
}

fn status_lines(seen: &Arc<Mutex<Seen>>, cseq_method: &str) -> Vec<String> {
    seen.lock()
        .unwrap()
        .responses
        .iter()
        .filter(|r| header(r, "CSeq").is_some_and(|c| c.ends_with(cseq_method)))
        .map(|r| r.lines().next().unwrap_or_default().to_owned())
        .collect()
}

/// While a call rings, a resent INVITE must get the 180 again and nothing
/// else; answering it would pick the call up behind the user's back. When the
/// caller gives up, the CANCEL must end the call with a 487. Before, there was
/// no CANCEL handling at all, so Aria rang on after the caller had gone.
#[tokio::test]
async fn a_cancelled_incoming_call_stops_ringing_and_a_resent_invite_is_not_answered() {
    let (pbx, seen, sock) = start_pbx_with_socket(InviteReply::Busy).await;
    let (_mgr, mut rx, client) = registered_client(pbx, &seen).await;

    let invite = incoming_invite(pbx, client, "harness-cancel@pbx", "INVITE");
    sock.send_to(invite.as_bytes(), client).await.unwrap();
    wait_for_call_state(&mut rx, "incoming").await;

    sock.send_to(invite.as_bytes(), client).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let cancel = incoming_invite(pbx, client, "harness-cancel@pbx", "CANCEL");
    sock.send_to(cancel.as_bytes(), client).await.unwrap();
    wait_for_call_state(&mut rx, "ended").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let invite_replies = status_lines(&seen, "INVITE");
    assert!(
        invite_replies.iter().all(|l| !l.contains(" 200 ")),
        "a ringing call was answered: {invite_replies:?}"
    );
    assert!(
        invite_replies
            .iter()
            .filter(|l| l.contains(" 180 "))
            .count()
            >= 2,
        "the resent INVITE should get the 180 again: {invite_replies:?}"
    );
    assert!(
        invite_replies.iter().any(|l| l.contains(" 487 ")),
        "the cancelled INVITE needs a 487: {invite_replies:?}"
    );
    assert!(status_lines(&seen, "CANCEL")
        .iter()
        .any(|l| l.contains(" 200 ")));
}

/// Answer an incoming call from a PBX started with `opts`, on fast timers
/// (T1 = 50 ms, so Timer H is 3.2 s). Returns what the test needs to follow it.
async fn answer_incoming(
    opts: PbxOptions,
    sip_call_id: &str,
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    Arc<Mutex<Seen>>,
    String,
) {
    let (pbx, seen, sock) = start_pbx_with_options(opts).await;
    let (mgr, mut rx, client) = registered_client_with(pbx, &seen, fast_timing()).await;
    let invite = incoming_invite(pbx, client, sip_call_id, "INVITE");
    sock.send_to(invite.as_bytes(), client).await.unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;
    mgr.answer(&call_id).await.expect("answered");
    (mgr, rx, seen, call_id)
}

fn count_200s(seen: &Arc<Mutex<Seen>>) -> usize {
    status_lines(seen, "INVITE")
        .iter()
        .filter(|l| l.contains(" 200 "))
        .count()
}

/// #69: the 200 OK to an incoming INVITE is lost on the wire. The caller has
/// our 180 and has stopped resending its INVITE, so only we can recover it:
/// the 200 must be retransmitted until the PBX ACKs it, the retransmission
/// must stop at the ACK, and the call must stay up past Timer H. Before, the
/// 200 went out once and the caller rang until it gave up.
#[tokio::test]
async fn a_lost_200_ok_is_retransmitted_until_acked_and_the_call_stays_up() {
    let opts = PbxOptions {
        ack_invite_200: true,
        drop_first_invite_200: true,
        ..Default::default()
    };
    let (mgr, mut rx, seen, call_id) = answer_incoming(opts, "harness-lost-200@pbx").await;

    // The first retransmission is due at T1 (50 ms); the PBX ACKs it.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while seen
        .lock()
        .unwrap()
        .responses
        .iter()
        .all(|r| !r.starts_with("SIP/2.0 200"))
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the lost 200 OK was never sent again"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let after_ack = count_200s(&seen);

    // Well past Timer H (3.2 s): no more 200s, no BYE, still connected.
    tokio::time::sleep(Duration::from_millis(4000)).await;
    assert_eq!(
        count_200s(&seen),
        after_ack,
        "the 200 OK was retransmitted after its ACK"
    );
    let methods = seen.lock().unwrap().methods.clone();
    assert!(
        !methods.iter().any(|m| m == "BYE"),
        "an ACKed call was hung up: {methods:?}"
    );
    while let Ok(event) = rx.try_recv() {
        if let super::SipEvent::CallStateChanged(e) = event {
            assert_ne!(e.state, "ended", "an ACKed call ended");
        }
    }
    let established = mgr
        .state
        .read()
        .await
        .find_call(&call_id)
        .is_some_and(|(_, c)| c.is_established());
    assert!(established, "the call should still be up");
}

/// When no ACK ever comes, the 200 is resent at T1 doubling to T2 and the call
/// is given up with a BYE once 64*T1 has passed (RFC 3261 §13.3.1.4), rather
/// than showing connected to a caller who never was.
#[tokio::test]
async fn an_unacknowledged_200_ok_is_retransmitted_then_ended_with_a_bye() {
    let (_mgr, mut rx, seen, call_id) =
        answer_incoming(PbxOptions::default(), "harness-no-ack@pbx").await;

    let ended = wait_for_call_state(&mut rx, "ended").await;
    assert_eq!(ended, call_id);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 50, 100, 200, 400, 800, 1600 (capped by T2 = 4 s) within 3.2 s: the
    // original and about six retransmissions. Loopback is not lossy; more
    // than that would mean the interval is not doubling.
    let sent = count_200s(&seen);
    assert!(
        (5..=8).contains(&sent),
        "expected the 200 OK and ~6 retransmissions, got {sent}"
    );
    let methods = seen.lock().unwrap().methods.clone();
    assert!(
        methods.iter().any(|m| m == "BYE"),
        "the unacknowledged call needs a BYE: {methods:?}"
    );
}

/// Declining a ringing call is a final response to its INVITE, not a BYE: no
/// dialog exists yet to say goodbye to.
#[tokio::test]
async fn declining_an_incoming_call_answers_486_and_sends_no_bye() {
    let (pbx, seen, sock) = start_pbx_with_socket(InviteReply::Busy).await;
    let (mgr, mut rx, client) = registered_client(pbx, &seen).await;

    let invite = incoming_invite(pbx, client, "harness-decline@pbx", "INVITE");
    sock.send_to(invite.as_bytes(), client).await.unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;

    mgr.hangup(&call_id).await.expect("declined");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let invite_replies = status_lines(&seen, "INVITE");
    assert!(
        invite_replies.iter().any(|l| l.contains(" 486 ")),
        "declining should answer 486: {invite_replies:?}"
    );
    let methods = seen.lock().unwrap().methods.clone();
    assert!(
        !methods.iter().any(|m| m == "BYE"),
        "sent a BYE for an unanswered call: {methods:?}"
    );
}

/// Collect call events for `call_id` until `until` or the deadline.
async fn call_states_until(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    call_id: &str,
    until: &str,
    timeout: Duration,
) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut states = Vec::new();
    loop {
        let Ok(Some(event)) = tokio::time::timeout_at(deadline, rx.recv()).await else {
            return states;
        };
        if let super::SipEvent::CallStateChanged(e) = event {
            if e.call_id == call_id {
                states.push(e.state.clone());
                if e.state == until {
                    return states;
                }
            }
        }
    }
}

fn via_branch(msg: &str) -> String {
    header(msg, "Via")
        .and_then(|v| param(v, "branch"))
        .unwrap_or_default()
}

/// A PBX that challenges INVITEs retransmits its 407 until the ACK matches the
/// INVITE transaction, which needs the INVITE's Via branch (RFC 3261
/// §17.1.1.3). The challenge ACK used a fresh branch, so the PBX kept resending
/// the 407, and each copy was handled as a new challenge: the second one ended
/// the call as an "auth loop" half a second after dialling.
#[tokio::test]
async fn a_challenged_invite_is_acked_on_its_own_branch_and_resent_challenges_are_ignored() {
    let (pbx, seen) = start_pbx_with(InviteReply::ChallengeThenAnswer).await;
    let (mgr, mut rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .unwrap_or_else(|e| panic!("call placed: {e}"));

    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(
        states.contains(&"connected".to_owned()),
        "never connected: {states:?}"
    );

    // Outlive the PBX's 407 retransmissions (3 × 300 ms); none may end the call.
    let later = call_states_until(&mut rx, &call_id, "ended", Duration::from_millis(1500)).await;
    assert!(
        !later.contains(&"ended".to_owned()),
        "a retransmitted 407 ended the call: {later:?}"
    );

    {
        let s = seen.lock().unwrap();
        assert_eq!(s.invites.len(), 2, "one INVITE, one authenticated retry");
        assert!(header(&s.invites[1], "Proxy-Authorization").is_some());
        assert_eq!(header(&s.invites[1], "CSeq"), Some("2 INVITE"));
        let challenge_ack = s.ack_msgs.first().expect("the 407 was ACKed");
        assert_eq!(
            via_branch(challenge_ack),
            via_branch(&s.invites[0]),
            "the ACK for a non-2xx must reuse the INVITE's branch"
        );
        assert_eq!(header(challenge_ack, "CSeq"), Some("1 ACK"));
        assert_eq!(s.acks, 2, "one ACK per final response (407, 200)");
    }

    mgr.hangup(&call_id).await.expect("hung up");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = seen.lock().unwrap();
    let bye = s.byes.first().expect("an answered call ends with BYE");
    assert_eq!(
        header(bye, "CSeq"),
        Some("3 BYE"),
        "CSeq continues after the auth retry"
    );
    assert!(s.cancels.is_empty(), "no CANCEL for an answered call");
}

/// The SDP line starting with `prefix` in a message's body.
fn sdp_line<'a>(msg: &'a str, prefix: &str) -> Option<&'a str> {
    msg.split("\r\n\r\n")
        .nth(1)?
        .lines()
        .find(|l| l.starts_with(prefix))
}

/// The authenticated INVITE after a 407 must offer the same media address and
/// port as the INVITE it replaces. The first offer's address comes from a
/// per-call STUN probe; the retry used to rebuild it from the account's
/// registration-time address alone, so against a public PBX the `c=` line
/// flipped from the STUN-discovered public address to the LAN one between the
/// two INVITEs, and a server that sends RTP where the SDP says got one-way
/// audio. On loopback every address rule lands on 127.0.0.1, so this pins the
/// invariant rather than the NAT case; `pbx_e2e::inbound_call_through_the_pbx`
/// checks it against a real PBX.
#[tokio::test]
async fn an_authenticated_invite_offers_the_same_media_address_as_the_first() {
    let (pbx, seen) = start_pbx_with(InviteReply::ChallengeThenAnswer).await;
    let (mgr, mut rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .unwrap_or_else(|e| panic!("call placed: {e}"));
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(
        states.contains(&"connected".to_owned()),
        "never connected: {states:?}"
    );

    {
        let s = seen.lock().unwrap();
        assert_eq!(s.invites.len(), 2, "one INVITE, one authenticated retry");
        let (first, retry) = (&s.invites[0], &s.invites[1]);
        assert!(
            header(retry, "Proxy-Authorization").is_some(),
            "the second INVITE carries credentials"
        );
        for prefix in ["c=", "m="] {
            let a = sdp_line(first, prefix)
                .unwrap_or_else(|| panic!("first INVITE has no {prefix} line"));
            let b = sdp_line(retry, prefix)
                .unwrap_or_else(|| panic!("retried INVITE has no {prefix} line"));
            assert_eq!(a, b, "the authenticated INVITE changed its {prefix} line");
        }
        // The origin's session id is fresh per offer; its address is not.
        let origin_addr = |msg: &str| {
            sdp_line(msg, "o=")
                .and_then(|o| o.split_whitespace().last())
                .map(str::to_owned)
        };
        assert_eq!(
            origin_addr(first),
            origin_addr(retry),
            "the authenticated INVITE changed its o= address"
        );
        assert_eq!(sdp_line(first, "c="), Some("c=IN IP4 127.0.0.1"));
    }

    mgr.hangup(&call_id).await.expect("hung up");
}

/// The caller hangs up as the callee answers. Our CANCEL and their 200 OK
/// cross: the 200 confirms a dialog the CANCEL cannot touch, so after the ACK
/// a BYE must follow (RFC 3261 §9.1, §15). Before, the 200 was ACKed and left
/// alone, and the callee stayed connected to silence.
///
/// The 200 comes twice. Each copy is ACKed, with the INVITE's CSeq (the
/// CANCEL used to move the dialog's CSeq on, so the ACK said 2), and the
/// dialog gets one BYE, not one per copy.
#[tokio::test]
async fn a_200_that_crosses_our_cancel_is_acked_and_then_byed() {
    let (pbx, seen) = start_pbx_with(InviteReply::AnswerOnCancel).await;
    let (mgr, mut rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .unwrap_or_else(|e| panic!("call placed: {e}"));
    call_states_until(&mut rx, &call_id, "ringing", Duration::from_secs(5)).await;

    mgr.hangup(&call_id).await.expect("cancelled");
    let states = call_states_until(&mut rx, &call_id, "ended", Duration::from_secs(5)).await;
    assert!(states.contains(&"ended".to_owned()), "{states:?}");
    assert!(
        !states.contains(&"connected".to_owned()),
        "a cancelled call must not show connected: {states:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;

    let s = seen.lock().unwrap();
    let cancel = s.cancels.first().expect("a ringing call is cancelled");
    assert_eq!(
        header(cancel, "CSeq"),
        Some("1 CANCEL"),
        "CANCEL carries the INVITE's CSeq"
    );
    assert_eq!(
        via_branch(cancel),
        via_branch(&s.invites[0]),
        "CANCEL carries the INVITE's branch"
    );
    assert_eq!(s.acks, 2, "each copy of the crossed 200 is ACKed");
    for ack in &s.ack_msgs {
        assert_eq!(
            header(ack, "CSeq"),
            Some("1 ACK"),
            "the ACK carries the INVITE's CSeq: {ack}"
        );
    }
    assert_eq!(
        s.byes.len(),
        1,
        "one BYE per dialog, however often the 200 comes"
    );
    let bye = s
        .byes
        .first()
        .expect("the confirmed dialog is ended with BYE");
    assert!(
        bye.starts_with(&format!("BYE sip:11@{pbx} ")),
        "BYE goes to the 200's Contact: {bye}"
    );
    let invite_from_tag = header(&s.invites[0], "From")
        .and_then(|f| param(f, "tag"))
        .unwrap();
    assert_eq!(
        header(bye, "From").and_then(|f| param(f, "tag")),
        Some(invite_from_tag)
    );
    assert_eq!(
        header(bye, "To").and_then(|t| param(t, "tag")),
        Some("pbx1".to_owned())
    );
}

/// Wait for a registration event in `state` for `account_id`.
async fn wait_for_registration(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    account_id: &str,
    state: super::RegistrationState,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let Ok(Some(event)) = tokio::time::timeout_at(deadline, rx.recv()).await else {
            return false;
        };
        if let super::SipEvent::RegistrationChanged(e) = event {
            if e.account_id == account_id && e.state == state {
                return true;
            }
        }
    }
}

/// The refresh REGISTER follows what the registrar granted, not a fixed 120 s.
/// Each refresh is one unauthenticated request and one authenticated one: the
/// retransmission timer must stop once the challenge is answered, not keep
/// resending the first request (which the server would challenge again, and
/// the client would count against the credentials).
#[tokio::test]
async fn a_registration_refreshes_at_half_the_granted_expires_without_duplicate_requests() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions {
        register_expires: 2,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, _rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");

    // Granted 2 s → refresh every 1 s (the floor). Three should land in 3.5 s.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let s = seen.lock().unwrap();
    assert!(
        s.register_ok.len() >= 3,
        "only {} registrations in 3.5 s",
        s.register_ok.len()
    );
    assert!(s.register_ok.iter().all(|ok| *ok), "{:?}", s.register_ok);
    assert_eq!(
        s.register_unauth,
        s.register_ok.len(),
        "each cycle is one challenge and one answer; extra unauthenticated REGISTERs are Timer A \
         still running after the challenge was answered"
    );
}

/// A refresh whose request (or answer) is lost must not strand the account.
/// It used to: the refresh had no deadline, the account sat in "registering",
/// and the refresh timer skipped every tick because the state was not
/// Registered. Now the deadline moves it to reconnecting, the reconnect
/// registers again, and — the part that was missing even when reconnects
/// worked — the refresh timer runs on the new transport.
#[tokio::test]
async fn a_lost_refresh_reconnects_and_keeps_refreshing_afterwards() {
    let opts = PbxOptions {
        register_expires: 2,
        ..PbxOptions::default()
    };
    let drop_next = opts.drop_next_register.clone();
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, mut rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    let id = cfg.id.clone();
    register(&mgr, cfg).await.expect("registered");
    // Drain the events of the first registration.
    while rx.try_recv().is_ok() {}

    // The next REGISTER is the refresh, due in ~1 s. Swallow it.
    drop_next.store(true, Ordering::SeqCst);

    assert!(
        wait_for_registration(
            &mut rx,
            &id,
            super::RegistrationState::Reconnecting,
            Duration::from_secs(4)
        )
        .await,
        "an unanswered refresh must lead to reconnecting"
    );
    assert!(
        wait_for_registration(
            &mut rx,
            &id,
            super::RegistrationState::Registered,
            Duration::from_secs(4)
        )
        .await,
        "the reconnect must register again"
    );
    let registrations_after_reconnect = seen.lock().unwrap().register_ok.len();

    // The new transport must refresh too.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let registrations_later = seen.lock().unwrap().register_ok.len();
    assert!(
        registrations_later > registrations_after_reconnect,
        "no refresh after the reconnect: {} before, {} after",
        registrations_after_reconnect,
        registrations_later
    );
    let (state, _) = mgr.registration_state_for_account(Some(&id)).await;
    assert_eq!(state, super::RegistrationState::Registered);
}

/// Unregistering is a REGISTER with Expires: 0 and it is challenged like any
/// other. The account has to stay until the authenticated one is answered;
/// removing it on the first send left the server's binding in place and the
/// 401 unanswered, so a signed-out phone kept getting called.
#[tokio::test]
async fn unregistering_answers_the_challenge_and_clears_the_binding() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, mut rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    let id = cfg.id.clone();
    register(&mgr, cfg).await.expect("registered");
    while rx.try_recv().is_ok() {}

    mgr.unregister_account(Some(&id))
        .await
        .expect("unregistered");

    {
        let s = seen.lock().unwrap();
        assert_eq!(
            s.register_ok.len(),
            2,
            "one registration, one authenticated unregistration"
        );
        assert_eq!(
            s.register_expires[1].as_deref(),
            Some("0"),
            "the unregister carries Expires: 0"
        );
        assert!(s.register_ok[1], "the unregister was authenticated");
    }

    let (state, _) = mgr.registration_state_for_account(Some(&id)).await;
    assert_eq!(state, super::RegistrationState::Unregistered);
    assert!(
        wait_for_registration(
            &mut rx,
            &id,
            super::RegistrationState::Unregistered,
            Duration::from_secs(1)
        )
        .await,
        "the UI is told the account is unregistered"
    );
}

/// A subscription refresh is sent inside the subscription the server
/// accepted — with its tag — and when the server says so, not every 60 s.
#[tokio::test]
async fn a_subscription_refresh_stays_in_the_dialog_and_follows_the_granted_expires() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions {
        subscribe_expires: 2,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, _rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");

    mgr.subscribe_presence("sip:1002@harness.lan", super::state::EventType::Dialog)
        .await
        .expect("subscribed");

    // Granted 2 s → the refresh is due at 1 s; the timer looks every 200 ms.
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let s = seen.lock().unwrap();
    assert!(
        s.subscribes.len() >= 2,
        "no refresh within the granted lifetime: {}",
        s.subscribes.len()
    );
    let first = &s.subscribes[0];
    let refresh = &s.subscribes[1];
    assert!(
        header(first, "To").is_some_and(|t| !t.contains("tag=")),
        "the first SUBSCRIBE starts the dialog"
    );
    assert_eq!(
        header(refresh, "To").and_then(|t| param(t, "tag")),
        Some("pbx1".to_owned()),
        "the refresh must carry the server's tag: {refresh}"
    );
    assert_eq!(header(refresh, "Call-ID"), header(first, "Call-ID"));
}

/// A SUBSCRIBE the server keeps challenging is given up, like a REGISTER is.
/// It used to be answered forever, one round trip at a time, which on a
/// server with fail2ban got the whole phone banned.
#[tokio::test]
async fn a_subscribe_that_is_always_challenged_is_abandoned() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions {
        reject_subscribe_auth: true,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, _rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");

    mgr.subscribe_presence("sip:1003@harness.lan", super::state::EventType::Dialog)
        .await
        .expect("subscribe sent");
    tokio::time::sleep(Duration::from_millis(1500)).await;

    let sent = seen.lock().unwrap().subscribes.len();
    assert!(sent <= 3, "kept answering challenges: {sent} SUBSCRIBEs");
    assert!(
        mgr.find_subscription_by_extension("1003").await.is_none(),
        "a subscription the server will not accept must be dropped"
    );
}

/// After registering, the client subscribes to its own message summary, so a
/// voicemail left while it was offline shows at once (the PBX pushes MWI
/// unsolicited only on change). Registration refreshes do not pile up more
/// subscriptions, and the counts reach the frontend.
#[tokio::test]
async fn registering_subscribes_to_the_accounts_message_summary_once() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions {
        register_expires: 2,
        mwi_notify: true,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, mut rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    let id = cfg.id.clone();
    register(&mgr, cfg).await.expect("registered");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let status = loop {
        let Ok(Some(event)) = tokio::time::timeout_at(deadline, rx.recv()).await else {
            panic!("no voicemail status after registering");
        };
        if let super::SipEvent::VoicemailStatus(v) = event {
            break v;
        }
    };
    assert_eq!(status.account_id, id);
    assert!(status.messages_waiting);
    assert_eq!((status.new_messages, status.old_messages), (2, 1));

    // Granted 2 s → the registration refreshes every second; give it a few.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let s = seen.lock().unwrap();
    assert!(s.register_ok.len() >= 2, "no registration refresh happened");
    let first = s.mwi_subscribes.first().expect("an MWI SUBSCRIBE");
    assert!(
        first.starts_with(&format!("SUBSCRIBE sip:{USER}@harness.lan ")),
        "{first}"
    );
    assert!(header(first, "Accept") == Some("application/simple-message-summary"));
    let dialogs: std::collections::HashSet<_> = s
        .mwi_subscribes
        .iter()
        .filter_map(|m| header(m, "Call-ID"))
        .collect();
    assert_eq!(
        dialogs.len(),
        1,
        "refreshes must reuse the one subscription: {}",
        s.mwi_subscribes.len()
    );
    assert!(s.subscribes.is_empty(), "no BLF subscription was asked for");
}

/// The account's message-summary subscriptions as (state, Call-ID), and
/// whether MWI has been marked refused.
async fn mwi_state(
    mgr: &SipManager,
    account_id: &str,
) -> (Vec<(super::presence::SubscriptionState, String)>, bool) {
    let s = mgr.state.read().await;
    let a = s.accounts.get(account_id).expect("the account");
    let subs = a
        .subscriptions
        .iter()
        .filter(|x| x.event_type == super::presence::EventType::MessageSummary)
        .map(|x| (x.state, x.call_id.clone()))
        .collect();
    (subs, a.mwi_refused)
}

async fn wait_for_mwi(
    mgr: &SipManager,
    account_id: &str,
    what: &str,
    cond: impl Fn(&[(super::presence::SubscriptionState, String)]) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while !cond(&mwi_state(mgr, account_id).await.0) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{what}: {:?}",
            mwi_state(mgr, account_id).await
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn is_active(subs: &[(super::presence::SubscriptionState, String)]) -> bool {
    subs.len() == 1 && subs[0].0 == super::presence::SubscriptionState::Active
}

/// A 481 to an MWI refresh means the server lost the subscription (a
/// restart, or the refresh reached the other cluster node). It used to mark
/// MWI refused for the rest of the session; now a new subscription, in a new
/// dialog, replaces it.
#[tokio::test]
async fn an_mwi_refresh_answered_481_subscribes_afresh() {
    let opts = PbxOptions {
        subscribe_expires: 2,
        ..PbxOptions::default()
    };
    let replies = opts.mwi_replies.clone();
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    wait_for_mwi(&mgr, &cfg.id, "never subscribed", is_active).await;
    let first_dialog = mwi_state(&mgr, &cfg.id).await.0[0].1.clone();

    // The refresh (due at 1 s) gets a 481.
    replies.lock().unwrap().push((481, vec![]));
    wait_for_mwi(&mgr, &cfg.id, "never subscribed again", |subs| {
        is_active(subs) && subs[0].1 != first_dialog
    })
    .await;

    let (_, refused) = mwi_state(&mgr, &cfg.id).await;
    assert!(!refused, "a 481 is not a refusal");
    let s = seen.lock().unwrap();
    let fresh = s
        .mwi_subscribes
        .iter()
        .find(|m| header(m, "Call-ID") != Some(first_dialog.as_str()))
        .expect("a new SUBSCRIBE");
    assert!(
        header(fresh, "To").is_some_and(|t| !t.contains("tag=")),
        "a new dialog: {fresh}"
    );
}

/// A 423 says the Expires asked for is too short. The SUBSCRIBE is sent
/// again with the server's Min-Expires, and MWI is not marked refused.
#[tokio::test]
async fn an_mwi_subscribe_answered_423_is_retried_with_min_expires() {
    let opts = PbxOptions::default();
    opts.mwi_replies
        .lock()
        .unwrap()
        .push((423, vec!["Min-Expires: 7200".into()]));
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    wait_for_mwi(&mgr, &cfg.id, "never subscribed", is_active).await;

    assert!(!mwi_state(&mgr, &cfg.id).await.1, "a 423 is not a refusal");
    let s = seen.lock().unwrap();
    assert_eq!(s.mwi_subscribes.len(), 2, "{:?}", s.mwi_subscribes);
    let retry = &s.mwi_subscribes[1];
    assert_eq!(header(retry, "Expires"), Some("7200"), "{retry}");
    assert_eq!(header(retry, "CSeq"), Some("2 SUBSCRIBE"));
    assert_eq!(
        header(retry, "Call-ID"),
        header(&s.mwi_subscribes[0], "Call-ID")
    );
}

/// A refusal that stands (403) still turns MWI off for the session.
#[tokio::test]
async fn an_mwi_subscribe_answered_403_is_not_asked_again() {
    let opts = PbxOptions {
        register_expires: 2,
        ..PbxOptions::default()
    };
    opts.mwi_replies.lock().unwrap().push((403, vec![]));
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    // Past a registration refresh (every second here).
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let (subs, refused) = mwi_state(&mgr, &cfg.id).await;
    assert!(refused && subs.is_empty(), "{subs:?}");
    assert!(
        seen.lock().unwrap().register_ok.len() >= 2,
        "no registration refresh happened"
    );
    assert_eq!(seen.lock().unwrap().mwi_subscribes.len(), 1);
}

/// The MWI SUBSCRIBE goes through the transaction layer: a lost first copy
/// is resent, and the counts arrive. It used to go out once, and one lost
/// datagram meant no voicemail count until the app reconnected.
#[tokio::test]
async fn a_lost_mwi_subscribe_is_retransmitted() {
    let opts = PbxOptions {
        mwi_notify: true,
        drop_first_copy_of: vec!["SUBSCRIBE"],
        ..PbxOptions::default()
    };
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, mut rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let Ok(Some(event)) = tokio::time::timeout_at(deadline, rx.recv()).await else {
            panic!("no voicemail status: the lost SUBSCRIBE was never resent");
        };
        if matches!(event, super::SipEvent::VoicemailStatus(_)) {
            break;
        }
    }
    let s = seen.lock().unwrap();
    assert_eq!(s.dropped.len(), 1);
    assert_eq!(
        via_branch(&s.mwi_subscribes[0]),
        via_branch(&s.dropped[0]),
        "the copy is the same transaction"
    );
}

/// An MWI SUBSCRIBE that is never answered times out (Timer F) and is
/// dropped, so the next registration refresh subscribes again. Left
/// pending, it counted as live and blocked every later attempt.
#[tokio::test]
async fn an_unanswered_mwi_subscribe_is_retried_at_the_next_registration() {
    let opts = PbxOptions {
        register_expires: 2,
        ..PbxOptions::default()
    };
    let ignore = opts.ignore_mwi.clone();
    ignore.store(true, Ordering::SeqCst);
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(pbx, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");

    // Timer F is 400 ms here; the registration refreshes at 1 s.
    tokio::time::sleep(Duration::from_millis(650)).await;
    let (subs, refused) = mwi_state(&mgr, &cfg.id).await;
    assert!(
        subs.is_empty() && !refused,
        "the unanswered subscription is still there: {subs:?}"
    );
    let first_dialog = {
        let s = seen.lock().unwrap();
        assert!(
            s.mwi_subscribes.len() >= 2,
            "never resent: {}",
            s.mwi_subscribes.len()
        );
        header(&s.mwi_subscribes[0], "Call-ID").unwrap().to_owned()
    };

    ignore.store(false, Ordering::SeqCst);
    wait_for_mwi(&mgr, &cfg.id, "never subscribed again", |subs| {
        is_active(subs) && subs[0].1 != first_dialog
    })
    .await;
}

/// When an outbound proxy is configured, requests must be routed to that proxy
/// address and port (RFC 3261 §10.2 / §16.12) while preserving the target SIP
/// domain in Request-URI and headers.
#[tokio::test]
async fn an_outbound_proxy_receives_register_and_invite_while_preserving_domain() {
    let (proxy, seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, _rx) = manager_with(fast_timing());

    let mut cfg = account(proxy, None, PASSWORD);
    // Domain is set to an unresolvable domain without a registrar;
    // all traffic must reach `proxy` via `outbound_proxy`.
    cfg.domain = "unresolvable-ims.example.invalid".into();
    cfg.registrar = None;
    cfg.outbound_proxy = Some(format!("127.0.0.1:{}", proxy.port()));

    register(&mgr, cfg)
        .await
        .expect("registered via outbound proxy");

    let s = seen.lock().unwrap();
    assert!(!s.register_ok.is_empty(), "proxy received the REGISTER");
}

/// The outbound proxy (or registrar) may carry its own port, but a bare IPv6
/// literal must not lose its last group to that parsing.
#[test]
fn target_server_splits_only_an_unambiguous_port() {
    let pbx: SocketAddr = "127.0.0.1:5060".parse().unwrap();
    let mut cfg = account(pbx, None, PASSWORD);
    cfg.port = 5080;
    let cases = [
        ("proxy.example.com", ("proxy.example.com", 5080)),
        ("proxy.example.com:5061", ("proxy.example.com", 5061)),
        ("192.0.2.10:5061", ("192.0.2.10", 5061)),
        ("[2001:db8::1]:5061", ("2001:db8::1", 5061)),
        ("[2001:db8::1]", ("2001:db8::1", 5080)),
        ("2001:db8::1", ("2001:db8::1", 5080)),
    ];
    for (proxy, want) in cases {
        cfg.outbound_proxy = Some(proxy.into());
        assert_eq!(SipManager::target_server(&cfg), want, "{proxy}");
    }
}

/// A host that does not resolve fails the first step of the report, with
/// the DNS text the user can act on, and the report is what the query returns.
#[tokio::test]
async fn an_unresolvable_server_fails_the_resolve_step() {
    use super::connection::{FailureKind, Outcome, StepKind, StepStatus};
    let (mgr, mut rx) = manager_with(fast_timing());
    let mut cfg = account("127.0.0.1:5060".parse().unwrap(), None, PASSWORD);
    cfg.registrar = Some("no-such-host.invalid".into());
    let err = mgr
        .register(cfg.clone())
        .await
        .expect_err("cannot register");
    assert!(err.starts_with("DNS"), "{err}");
    let report = mgr.connection_report(&cfg.id).await.expect("report exists");
    assert_eq!(report.step(StepKind::Resolve).status, StepStatus::Failed);
    assert_eq!(
        report.step(StepKind::Resolve).failure,
        Some(FailureKind::Dns)
    );
    assert_eq!(report.step(StepKind::Connect).status, StepStatus::Pending);
    assert_eq!(report.outcome, Outcome::Error);
    assert_eq!(report.error.as_deref(), Some(err.as_str()));
    let mut saw = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, super::SipEvent::ConnectionReport(_)) {
            saw = true;
        }
    }
    assert!(saw, "a sip-connection event was emitted");
}

/// A literal proxy address needs no lookup; the step says so instead of
/// pretending one happened.
#[tokio::test]
async fn a_literal_address_skips_resolution() {
    use super::connection::{StepKind, StepStatus};
    let (proxy, _seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    let report = mgr.connection_report(&cfg.id).await.unwrap();
    assert_eq!(report.step(StepKind::Resolve).status, StepStatus::Skipped);
    assert_eq!(
        report.step(StepKind::Resolve).detail.as_deref(),
        Some("literal address")
    );
}

#[tokio::test]
async fn a_successful_registration_reports_every_step_ok() {
    use super::connection::{Outcome, StepKind, StepStatus};
    let (proxy, _seen, _) = start_pbx_with_options(PbxOptions {
        register_expires: 300,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    let r = mgr.connection_report(&cfg.id).await.unwrap();
    for k in [
        StepKind::Connect,
        StepKind::Register,
        StepKind::Authenticate,
        StepKind::Registered,
    ] {
        assert_eq!(r.step(k.clone()).status, StepStatus::Ok, "{k:?}");
    }
    assert!(r
        .step(StepKind::Register)
        .detail
        .as_deref()
        .unwrap()
        .starts_with("401 challenge in"));
    assert_eq!(
        r.step(StepKind::Registered).detail.as_deref(),
        Some("expires 300 s")
    );
    assert_eq!(r.outcome, Outcome::Registered);
    assert_eq!(r.registered.as_ref().unwrap().expires_in_s, 300);
}

#[tokio::test]
async fn a_wrong_password_fails_the_authenticate_step_as_auth() {
    use super::connection::{FailureKind, StepKind, StepStatus};
    let (proxy, _seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, "wrong");
    let err = register(&mgr, cfg.clone()).await.expect_err("rejected");
    let r = mgr.connection_report(&cfg.id).await.unwrap();
    assert_eq!(r.step(StepKind::Register).status, StepStatus::Ok);
    assert_eq!(r.step(StepKind::Authenticate).status, StepStatus::Failed);
    assert_eq!(
        r.step(StepKind::Authenticate).failure,
        Some(FailureKind::Auth)
    );
    assert!(r
        .step(StepKind::Authenticate)
        .detail
        .as_deref()
        .unwrap()
        .starts_with("401: Authentication failed"));
    assert_eq!(r.error.as_deref(), Some(err.as_str()));
}

#[tokio::test]
async fn an_unanswered_register_reports_the_timeout_with_the_send_count() {
    use super::connection::{FailureKind, StepKind, StepStatus};
    let opts = PbxOptions::default();
    opts.drop_next_register.store(true, Ordering::SeqCst);
    let (proxy, _seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    let _ = register(&mgr, cfg.clone()).await.expect_err("times out");
    let r = mgr.connection_report(&cfg.id).await.unwrap();
    let s = r.step(StepKind::Register);
    assert_eq!(s.status, StepStatus::Failed);
    assert_eq!(s.failure, Some(FailureKind::Timeout));
    let d = s.detail.as_deref().unwrap();
    assert!(
        d.starts_with("no reply in ") && d.contains("(sent ") && d.ends_with(" times)"),
        "{d}"
    );
}

/// Register, then make the PBX swallow the refresh; the backend goes to
/// Reconnecting and the report must show a fresh attempt, not the old ticks.
#[tokio::test]
async fn a_lost_refresh_restarts_the_report_as_reconnecting() {
    use super::connection::{Outcome, StepKind, StepStatus};
    let opts = PbxOptions {
        register_expires: 2,
        ..PbxOptions::default()
    };
    let drop = opts.drop_next_register.clone();
    let (proxy, _seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    drop.store(true, Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = mgr.connection_report(&cfg.id).await.unwrap();
        if r.outcome == Outcome::Reconnecting {
            assert_ne!(r.step(StepKind::Registered).status, StepStatus::Ok);
            assert!(r.reconnect_attempt >= 1, "attempt {}", r.reconnect_attempt);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "never went reconnecting"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll until the account is registered, or fail after five seconds.
async fn wait_registered(mgr: &SipManager, id: &str) {
    for _ in 0..50 {
        let (state, err) = mgr.registration_state_for_account(Some(id)).await;
        match state {
            super::RegistrationState::Registered => return,
            super::RegistrationState::Error => panic!("registration failed: {err:?}"),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    panic!("not registered within 5 s");
}

/// "Re-register" on a healthy account sends a refresh on the connection it
/// has. It must never sign the account out (no Expires: 0, no Unregistered
/// event) and must leave a call in progress alone.
#[tokio::test]
async fn register_now_on_a_registered_account_refreshes_without_signing_out() {
    let (proxy, seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, mut rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    let before = seen.lock().unwrap().register_ok.len();
    {
        let mut s = mgr.state.write().await;
        s.accounts
            .get_mut(&cfg.id)
            .unwrap()
            .calls
            .push(super::state::CallFSM::new_outbound(
                &cfg.id,
                "sip:200@harness.lan",
                "call-1".into(),
                "tag-1".into(),
                40000,
                "z9hG4bK-1".into(),
                "sip:ghostbook@harness.lan".into(),
            ));
    }
    while rx.try_recv().is_ok() {}
    assert_eq!(mgr.register_now(cfg.clone()).await.unwrap(), "refreshing");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().register_ok.len() == before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no refresh REGISTER arrived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_registered(&mgr, &cfg.id).await;
    {
        let s = mgr.state.read().await;
        let a = s.accounts.get(&cfg.id).unwrap();
        assert_eq!(a.calls.len(), 1, "the call survived");
        assert!(a.registration.is_registered());
    }
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert!(
        !events.iter().any(|e| matches!(e, super::SipEvent::RegistrationChanged(r) if r.state == super::RegistrationState::Unregistered)),
        "no sign-out event"
    );
    assert!(
        !seen
            .lock()
            .unwrap()
            .register_expires
            .iter()
            .any(|e| e.as_deref() == Some("0")),
        "no Expires: 0 REGISTER"
    );
    let r = mgr.connection_report(&cfg.id).await.unwrap();
    assert_eq!(r.outcome, super::connection::Outcome::Registered);
}

/// Retry on a failed account runs the whole path again, from DNS.
#[tokio::test]
async fn register_now_on_a_failed_account_runs_the_full_path_again() {
    let (proxy, _seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let mut cfg = account(proxy, None, "wrong");
    let _ = register(&mgr, cfg.clone()).await.expect_err("rejected");
    cfg.password = PASSWORD.into();
    assert_eq!(mgr.register_now(cfg.clone()).await.unwrap(), "registering");
    wait_registered(&mgr, &cfg.id).await;
    let report = mgr.connection_report(&cfg.id).await.unwrap();
    assert_eq!(report.outcome, super::connection::Outcome::Registered);
    assert_eq!(
        report.step(super::connection::StepKind::Resolve).status,
        super::connection::StepStatus::Skipped
    );
}

/// A retry for an account that never got as far as an `AccountState` (DNS
/// failed) still works, because the caller hands over the config.
#[tokio::test]
async fn register_now_after_a_dns_failure_registers_with_the_corrected_host() {
    let (proxy, _seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let mut cfg = account(proxy, None, PASSWORD);
    cfg.registrar = Some("no-such-host.invalid".into());
    let _ = mgr.register(cfg.clone()).await.expect_err("dns");
    cfg.registrar = Some(proxy.ip().to_string());
    assert_eq!(mgr.register_now(cfg.clone()).await.unwrap(), "registering");
    wait_registered(&mgr, &cfg.id).await;
}

/// The ordinary Wi-Fi-drop sequence: the refresh is lost, the backend
/// reconnects, and the reconnect attempt itself fails (here at DNS). The
/// panel must keep saying "reconnecting" with the failed step visible, not
/// flip to a dead-end error while the backend is still trying.
#[tokio::test]
async fn a_reconnect_attempt_that_fails_keeps_the_report_reconnecting() {
    use super::connection::{Outcome, StepKind, StepStatus};
    let opts = PbxOptions {
        register_expires: 2,
        ..PbxOptions::default()
    };
    let drop = opts.drop_next_register.clone();
    let (proxy, _seen, _) = start_pbx_with_options(opts).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    {
        let mut s = mgr.state.write().await;
        s.accounts.get_mut(&cfg.id).unwrap().config.registrar = Some("no-such-host.invalid".into());
    }
    drop.store(true, Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = mgr.connection_report(&cfg.id).await.unwrap();
        if r.step(StepKind::Resolve).status == StepStatus::Failed {
            assert_eq!(r.outcome, Outcome::Reconnecting, "{r:?}");
            assert!(
                r.error.as_deref().unwrap_or_default().starts_with("DNS"),
                "{:?}",
                r.error
            );
            let (state, _) = mgr.registration_state_for_account(Some(&cfg.id)).await;
            assert_eq!(state, super::RegistrationState::Reconnecting);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "reconnect never failed at DNS: {r:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A forced retry that fails before a REGISTER is sent must put the account
/// into Error, like a first registration does, so the chip and the panel
/// agree and the retry loop can start from it.
#[tokio::test]
async fn register_now_that_fails_at_dns_marks_the_account_error() {
    let (proxy, _seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, _rx) = manager_with(fast_timing());
    let mut cfg = account(proxy, None, "wrong");
    let _ = register(&mgr, cfg.clone()).await.expect_err("rejected");
    cfg.registrar = Some("no-such-host.invalid".into());
    let err = mgr.register_now(cfg.clone()).await.expect_err("dns");
    let (state, reason) = mgr.registration_state_for_account(Some(&cfg.id)).await;
    assert_eq!(state, super::RegistrationState::Error);
    assert_eq!(reason.as_deref(), Some(err.as_str()));
    let r = mgr.connection_report(&cfg.id).await.unwrap();
    assert_eq!(r.error.as_deref(), Some(err.as_str()));
}

// ---------------------------------------------------------------------------
// DNS SRV failover, against a fake resolver and fake servers on loopback
// ---------------------------------------------------------------------------

/// A manager whose DNS says `harness.lan` is served by `targets`, each
/// `(srv priority, port)` on loopback, in that priority order.
fn manager_with_srv(
    targets: &[(u16, u16)],
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
) {
    use super::srv::fake_dns::FakeDns;
    use super::srv::SrvRecord;
    let mut dns = FakeDns::default();
    let records = targets
        .iter()
        .enumerate()
        .map(|(i, (priority, port))| {
            let host = format!("s{i}.harness.lan");
            dns.ips
                .insert(host.clone(), vec!["127.0.0.1".parse().unwrap()]);
            SrvRecord {
                priority: *priority,
                weight: 1,
                port: *port,
                target: host,
            }
        })
        .collect();
    dns.srv.insert("_sip._udp.harness.lan".into(), records);
    let (mgr, rx) = SipManager::new_with_dns(fast_timing(), Arc::new(dns));
    use_local_stun(&mgr);
    mgr.start_background_tasks();
    (mgr, rx)
}

/// An account for `harness.lan` with no port: SRV decides.
fn srv_account() -> AccountConfig {
    let mut cfg = account("127.0.0.1:1".parse().unwrap(), None, PASSWORD);
    cfg.registrar = None;
    cfg.port = 0;
    cfg
}

/// The first target never answers: the REGISTER times out after the shortened
/// deadline and the next target is tried, with no error in between.
#[tokio::test]
async fn a_silent_srv_target_fails_over_to_the_next_one() {
    let (pbx, seen) = start_pbx().await;
    let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (mgr, _rx) =
        manager_with_srv(&[(10, silent.local_addr().unwrap().port()), (20, pbx.port())]);
    let cfg = srv_account();
    register(&mgr, cfg.clone())
        .await
        .expect("registered on the second target");
    let mut buf = [0u8; 2048];
    let (n, _) = tokio::time::timeout(Duration::from_secs(1), silent.recv_from(&mut buf))
        .await
        .expect("the first target was tried")
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("REGISTER "));
    let s = seen.lock().unwrap();
    assert_eq!(s.register_ok, [true]);
    // The Contact carries the RFC 5626 outbound parameters.
    let contact = s.register_contacts.first().cloned().unwrap_or_default();
    assert!(
        contact.contains(";ob>")
            && contact.contains("+sip.instance=")
            && contact.contains("reg-id=1"),
        "{contact}"
    );
}

/// A 503 is never final: the next target is tried at once, and the account
/// ends up registered.
#[tokio::test]
async fn a_503_fails_over_to_the_next_srv_target() {
    let (busy, busy_seen, _) = start_pbx_with_options(PbxOptions {
        register_503: Some(30),
        ..PbxOptions::default()
    })
    .await;
    let (pbx, _seen) = start_pbx().await;
    let (mgr, _rx) = manager_with_srv(&[(10, busy.port()), (20, pbx.port())]);
    register(&mgr, srv_account())
        .await
        .expect("registered on the second target");
    assert!(
        busy_seen.lock().unwrap().register_unauth >= 1,
        "the first target was asked"
    );
}

/// With nothing else to try, a 503 is retried (not reported as a failure): the
/// account is not left in `Error`.
#[tokio::test]
async fn a_lone_503_is_retried_not_reported_as_an_error() {
    let (busy, _seen, _) = start_pbx_with_options(PbxOptions {
        register_503: Some(1),
        ..PbxOptions::default()
    })
    .await;
    let (mgr, _rx) = manager_with_srv(&[(10, busy.port())]);
    let cfg = srv_account();
    mgr.register(cfg.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (state, err) = mgr.registration_state_for_account(Some(&cfg.id)).await;
    assert_ne!(state, super::RegistrationState::Error, "{err:?}");
}

/// Keepalives that go unanswered must not drop a call that is up: the server
/// here never answers OPTIONS, and the keepalive runs every 100 ms.
#[tokio::test]
async fn a_call_survives_a_keepalive_failure_and_the_account_fails_over_without_one() {
    let (pbx, _seen, _) = start_pbx_with_options(PbxOptions {
        invite_reply: InviteReply::ChallengeThenAnswer,
        ignore_options: true,
        ..PbxOptions::default()
    })
    .await;
    let mut timing = fast_timing();
    timing.options_interval = Duration::from_millis(100);
    timing.options_wait = Duration::from_millis(100);
    let (mgr, mut rx) = manager_with(timing);
    let cfg = account(pbx, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .expect("call placed");
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "{states:?}");

    // Many missed keepalives later the call is still there, and the account
    // has not been taken down.
    let later = call_states_until(&mut rx, &call_id, "ended", Duration::from_millis(1500)).await;
    assert!(
        !later.contains(&"ended".to_owned()),
        "a keepalive failure ended the call: {later:?}"
    );
    let (state, _) = mgr.registration_state_for_account(Some(&cfg.id)).await;
    assert_eq!(state, super::RegistrationState::Registered);
    mgr.hangup(&call_id).await.expect("hung up");

    // With no call up the same silence does fail the connection over.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (state, _) = mgr.registration_state_for_account(Some(&cfg.id)).await;
        if state != super::RegistrationState::Registered {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "silence never triggered a failover"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The only server answers every REGISTER with 503 and Retry-After 600. An
/// emergency call is placed at once all the same; an ordinary call is refused.
#[tokio::test]
async fn an_emergency_call_dials_through_a_503_retry_after() {
    let (busy, seen, _) = start_pbx_with_options(PbxOptions {
        register_503: Some(600),
        ..PbxOptions::default()
    })
    .await;
    let (mgr, _rx) = manager_with_srv(&[(10, busy.port())]);
    mgr.register(srv_account()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        mgr.make_call("sip:1001@harness.lan").await.is_err(),
        "an ordinary call needs a registration"
    );
    let started = tokio::time::Instant::now();
    let id = mgr
        .make_call("sip:911@harness.lan")
        .await
        .expect("the emergency call is placed");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "took {:?}",
        started.elapsed()
    );
    assert!(!id.is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = seen.lock().unwrap();
    assert!(
        s.invites[0].starts_with("INVITE sip:911@harness.lan"),
        "{:?}",
        s.invites
    );
    assert!(
        s.methods.iter().any(|m| m == "INVITE"),
        "the INVITE reached the server: {:?}",
        s.methods
    );
}

/// A 911 call placed while the account is reconnecting stays up, on the same
/// transport, while the reconnect loop fires over and over.
#[tokio::test]
async fn an_emergency_call_stays_up_while_the_reconnect_loop_fires() {
    let (busy, seen, _) = start_pbx_with_options(PbxOptions {
        register_503: Some(1),
        invite_reply: InviteReply::ChallengeThenAnswer,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, mut rx) = manager_with_srv(&[(10, busy.port())]);
    let cfg = srv_account();
    mgr.register(cfg.clone()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let call_id = mgr
        .make_call("sip:911@harness.lan")
        .await
        .expect("emergency call placed");
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "{states:?}");

    let local_before = mgr.state.read().await.accounts[&cfg.id].local_addr;
    // Several reconnect rounds (backoff capped at 2 s here).
    let later = call_states_until(&mut rx, &call_id, "ended", Duration::from_secs(5)).await;
    assert!(
        !later.contains(&"ended".to_owned()),
        "the call was ended: {later:?}"
    );
    {
        let s = mgr.state.read().await;
        let a = &s.accounts[&cfg.id];
        assert_eq!(
            a.local_addr, local_before,
            "the transport was replaced under the call"
        );
        assert!(a.transport.is_some());
        assert!(a.calls.iter().any(|c| !c.is_ended()));
    }
    // Every INVITE came from the one socket.
    assert_eq!(
        seen.lock().unwrap().invites.len(),
        2,
        "INVITE and its authenticated retry only"
    );

    // Once the call ends the reconnect loop is free again.
    mgr.hangup(&call_id).await.expect("hung up");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        seen.lock().unwrap().byes.len() == 1,
        "BYE went to the same server"
    );
}

/// Place a call on fast timers (T1 = 50 ms, Timer B 3.2 s) against a PBX
/// started with `opts`.
async fn dial(
    opts: PbxOptions,
) -> (
    SipManager,
    tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>,
    Arc<Mutex<Seen>>,
    String,
) {
    let (pbx, seen, _) = start_pbx_with_options(opts).await;
    let (mgr, rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .unwrap_or_else(|e| panic!("call placed: {e}"));
    (mgr, rx, seen, call_id)
}

/// Wait until `cond` holds for what the PBX saw, or fail with `what`.
async fn wait_for_seen(
    seen: &Arc<Mutex<Seen>>,
    within: Duration,
    what: &str,
    cond: impl Fn(&Seen) -> bool,
) {
    let deadline = tokio::time::Instant::now() + within;
    while !cond(&seen.lock().unwrap()) {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn drop_once(methods: &[&str]) -> Arc<Mutex<Vec<String>>> {
    Arc::new(Mutex::new(methods.iter().map(|m| m.to_string()).collect()))
}

/// S1: the first INVITE is lost on the wire. It is resent at T1 (Timer A),
/// the PBX challenges the copy, and the call connects. It used to go out
/// once, and the user heard nothing for 32 s and then "Request timeout".
#[tokio::test]
async fn a_lost_invite_is_retransmitted_and_the_call_connects() {
    let opts = PbxOptions {
        invite_reply: InviteReply::ChallengeThenAnswer,
        drop_once: drop_once(&["INVITE"]),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(
        states.contains(&"connected".to_owned()),
        "never connected: {states:?}"
    );
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.dropped.len(), 1);
        assert_eq!(
            via_branch(&s.dropped[0]),
            via_branch(&s.invites[0]),
            "the copy is the same transaction"
        );
        assert_eq!(header(&s.invites[0], "CSeq"), Some("1 INVITE"));
    }
    mgr.hangup(&call_id).await.expect("hung up");
}

/// S1: the authenticated INVITE is lost. It is resent the same way.
#[tokio::test]
async fn a_lost_authenticated_invite_is_retransmitted_and_the_call_connects() {
    let opts = PbxOptions {
        invite_reply: InviteReply::ChallengeThenAnswer,
        drop_once: drop_once(&["INVITE+auth"]),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(
        states.contains(&"connected".to_owned()),
        "never connected: {states:?}"
    );
    {
        let s = seen.lock().unwrap();
        assert_eq!(s.dropped.len(), 1);
        assert!(header(&s.dropped[0], "Proxy-Authorization").is_some());
        let retry = s
            .invites
            .iter()
            .find(|i| header(i, "Proxy-Authorization").is_some())
            .expect("retry arrived");
        assert_eq!(header(retry, "CSeq"), Some("2 INVITE"));
        assert_eq!(via_branch(retry), via_branch(&s.dropped[0]));
    }
    mgr.hangup(&call_id).await.expect("hung up");
}

/// S7: a 100 Trying puts the INVITE in Proceeding. Neither Timer B (3.2 s
/// here) nor anything else may end the call while it waits for the far end,
/// and the INVITE is not resent after the 100. The 32 s limit used to end a
/// call like this (scaled here to 4 s of silence on T1 = 50 ms) locally,
/// with no CANCEL.
#[tokio::test]
async fn a_call_that_has_had_100_trying_survives_a_long_silence() {
    let opts = PbxOptions {
        invite_reply: InviteReply::TryingThenLateAnswer(Duration::from_secs(4)),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(8)).await;
    assert!(
        states.contains(&"connected".to_owned()),
        "never connected: {states:?}"
    );
    assert!(!states.contains(&"ended".to_owned()), "{states:?}");
    assert_eq!(
        seen.lock().unwrap().invites.len(),
        1,
        "no retransmission after the 100"
    );
    mgr.hangup(&call_id).await.expect("hung up");
}

/// Timer B: an INVITE that gets no response at all is resent at T1 doubling
/// and the call fails at 64*T1.
#[tokio::test]
async fn an_invite_with_no_response_fails_at_timer_b() {
    let opts = PbxOptions {
        invite_reply: InviteReply::Silent,
        ..PbxOptions::default()
    };
    let started = tokio::time::Instant::now();
    let (_mgr, mut rx, seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "ended", Duration::from_secs(8)).await;
    assert!(
        states.contains(&"ended".to_owned()),
        "never gave up: {states:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(3200),
        "gave up early: {:?}",
        started.elapsed()
    );
    // Sent at 0, 50, 150, 350, 750, 1550 and 3150 ms.
    // The last lands 50 ms before Timer B; a loaded machine may miss it.
    let sent = seen.lock().unwrap().invites.len();
    assert!((6..=7).contains(&sent), "sent {sent} INVITEs");
}

/// S8: the user hangs up before the challenge arrives. The 407 is ACKed and
/// nothing more: answering it rang the callee for an abandoned call.
#[tokio::test]
async fn hanging_up_before_the_challenge_sends_no_authenticated_invite() {
    let (pbx, seen) =
        start_pbx_with(InviteReply::DelayedChallenge(Duration::from_millis(300))).await;
    let (mgr, mut rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD))
        .await
        .expect("registered");
    let call_id = mgr
        .make_call("sip:11@harness.lan")
        .await
        .expect("call placed");
    mgr.hangup(&call_id).await.expect("hung up");
    let states = call_states_until(&mut rx, &call_id, "ended", Duration::from_secs(2)).await;
    assert!(states.contains(&"ended".to_owned()), "{states:?}");

    wait_for_seen(
        &seen,
        Duration::from_secs(2),
        "the late 407 was never ACKed",
        |s| s.acks >= 1,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = seen.lock().unwrap();
    assert!(
        s.invites
            .iter()
            .all(|i| header(i, "Proxy-Authorization").is_none()),
        "an authenticated INVITE went out for a call already hung up"
    );
    assert_eq!(header(&s.ack_msgs[0], "CSeq"), Some("1 ACK"));
}

/// S2: the BYE is lost. It is resent until the PBX answers; it used to go
/// out once, leaving the far party connected to silence.
#[tokio::test]
async fn a_lost_bye_is_retransmitted() {
    let drop = drop_once(&[]);
    let opts = PbxOptions {
        invite_reply: InviteReply::ChallengeThenAnswer,
        drop_once: drop.clone(),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "{states:?}");

    drop.lock().unwrap().push("BYE".into());
    mgr.hangup(&call_id).await.expect("hung up");
    wait_for_seen(
        &seen,
        Duration::from_secs(2),
        "the lost BYE was never resent",
        |s| !s.byes.is_empty(),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = seen.lock().unwrap();
    assert_eq!(s.dropped.len(), 1);
    assert_eq!(s.byes.len(), 1, "answered, so not resent again");
    assert_eq!(via_branch(&s.byes[0]), via_branch(&s.dropped[0]));
}

/// S2: the CANCEL is lost. It is resent until answered, so the callee stops
/// ringing, and the 487 that follows is ACKed with the INVITE's CSeq.
#[tokio::test]
async fn a_lost_cancel_is_retransmitted() {
    let opts = PbxOptions {
        invite_reply: InviteReply::RingUntilCancelled,
        drop_once: drop_once(&["CANCEL"]),
        ..PbxOptions::default()
    };
    let (_mgr, mut rx, seen, call_id) = dial(opts).await;
    call_states_until(&mut rx, &call_id, "ringing", Duration::from_secs(5)).await;
    _mgr.hangup(&call_id).await.expect("cancelled");

    wait_for_seen(
        &seen,
        Duration::from_secs(2),
        "the 487 was never ACKed",
        |s| s.acks >= 1,
    )
    .await;
    let s = seen.lock().unwrap();
    assert_eq!(s.dropped.len(), 1);
    assert_eq!(s.cancels.len(), 1);
    assert_eq!(header(&s.cancels[0], "CSeq"), Some("1 CANCEL"));
    assert_eq!(
        header(&s.ack_msgs[0], "CSeq"),
        Some("1 ACK"),
        "the CANCEL must not move the ACK's CSeq"
    );
    assert_eq!(via_branch(&s.ack_msgs[0]), via_branch(&s.invites[0]));
}

/// The user hangs up before any provisional response, and the CANCEL's first
/// copy is lost. The CANCEL shares the INVITE's Via branch; with transactions
/// keyed by branch alone it took over the INVITE's entry, the INVITE's task
/// then removed it, and neither request was resent. The callee rang on, and
/// Timer B could end the call a second time. Now the CANCEL is resent, the
/// INVITE stops at the 487, which is ACKed, and the call ends exactly once.
#[tokio::test]
async fn a_cancel_before_any_provisional_is_retransmitted_and_ends_the_call_once() {
    let opts = PbxOptions {
        invite_reply: InviteReply::SilentUntilCancelled,
        drop_once: drop_once(&["CANCEL"]),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, seen, call_id) = dial(opts).await;
    mgr.hangup(&call_id).await.expect("cancelled");

    wait_for_seen(
        &seen,
        Duration::from_secs(2),
        "the lost CANCEL was never resent",
        |s| !s.cancels.is_empty(),
    )
    .await;
    wait_for_seen(
        &seen,
        Duration::from_secs(2),
        "the 487 was never ACKed",
        |s| s.acks >= 1,
    )
    .await;
    let invites_at_487 = seen.lock().unwrap().invites.len();

    // Past Timer B (64*T1 = 3.2 s): a second "ended" from it would show here.
    let states = call_states_until(&mut rx, &call_id, "never", Duration::from_secs(4)).await;
    assert_eq!(
        states.iter().filter(|s| *s == "ended").count(),
        1,
        "{states:?}"
    );

    let s = seen.lock().unwrap();
    assert_eq!(s.dropped.len(), 1);
    assert_eq!(s.cancels.len(), 1, "answered, so not resent again");
    assert_eq!(via_branch(&s.cancels[0]), via_branch(&s.dropped[0]));
    assert_eq!(via_branch(&s.cancels[0]), via_branch(&s.invites[0]));
    assert_eq!(header(&s.ack_msgs[0], "CSeq"), Some("1 ACK"));
    assert_eq!(
        s.invites.len(),
        invites_at_487,
        "the INVITE was resent after its 487"
    );
}

/// S2: the REFER is lost. It is resent until answered, and the transfer is
/// not reported as failed.
#[tokio::test]
async fn a_lost_refer_is_retransmitted() {
    let drop = drop_once(&[]);
    let opts = PbxOptions {
        invite_reply: InviteReply::ChallengeThenAnswer,
        drop_once: drop.clone(),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "{states:?}");

    drop.lock().unwrap().push("REFER".into());
    mgr.transfer_blind(&call_id, "sip:12@harness.lan")
        .await
        .expect("transfer sent");
    wait_for_seen(
        &seen,
        Duration::from_secs(2),
        "the lost REFER was never resent",
        |s| s.methods.iter().any(|m| m == "REFER"),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        seen.lock()
            .unwrap()
            .methods
            .iter()
            .filter(|m| *m == "REFER")
            .count(),
        1
    );
    while let Ok(event) = rx.try_recv() {
        if let super::SipEvent::TransferProgress(t) = event {
            assert!(
                t.status < 400,
                "an answered REFER was reported failed: {}",
                t.message
            );
        }
    }
    mgr.hangup(&call_id).await.expect("hung up");
}

/// S2: a REFER that is never answered ends at Timer F, and the UI is told
/// the transfer failed instead of showing "Transfer initiated" for good.
#[tokio::test]
async fn an_unanswered_refer_is_reported_as_a_failed_transfer() {
    let drop = drop_once(&[]);
    let opts = PbxOptions {
        invite_reply: InviteReply::ChallengeThenAnswer,
        drop_once: drop.clone(),
        ..PbxOptions::default()
    };
    let (mgr, mut rx, _seen, call_id) = dial(opts).await;
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "{states:?}");

    drop.lock()
        .unwrap()
        .extend(std::iter::repeat_n("REFER".to_string(), 50));
    mgr.transfer_blind(&call_id, "sip:12@harness.lan")
        .await
        .expect("transfer sent");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("no transfer failure reported within 3 s")
            .expect("event channel open");
        if let super::SipEvent::TransferProgress(t) = event {
            if t.status >= 400 {
                assert_eq!(t.call_id, call_id);
                assert_eq!(t.status, 408);
                break;
            }
        }
    }
    mgr.hangup(&call_id).await.expect("hung up");
}

/// S4: every keepalive OPTIONS loses its first copy. Each is resent within
/// the wait and answered, so the account stays registered. Each loss used to
/// count as a miss, and two in a row failed a healthy server over.
#[tokio::test]
async fn a_lost_options_keepalive_does_not_fail_over() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions {
        drop_first_copy_of: vec!["OPTIONS"],
        ..PbxOptions::default()
    })
    .await;
    let mut timing = fast_timing();
    timing.options_interval = Duration::from_millis(100);
    timing.options_wait = Duration::from_millis(300);
    let (mgr, _rx) = manager_with(timing);
    let cfg = account(pbx, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let (state, _) = mgr.registration_state_for_account(Some(&cfg.id)).await;
        assert_eq!(
            state,
            super::RegistrationState::Registered,
            "a lost OPTIONS failed the server over"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let s = seen.lock().unwrap();
    let lost = s
        .dropped
        .iter()
        .filter(|m| m.starts_with("OPTIONS"))
        .count();
    let delivered = s.methods.iter().filter(|m| *m == "OPTIONS").count();
    assert!(lost >= 3, "only {lost} keepalives went out");
    assert!(delivered >= lost - 1, "{delivered} resent of {lost} lost");
}

fn count_finals(seen: &Arc<Mutex<Seen>>, code: &str) -> usize {
    status_lines(seen, "INVITE")
        .iter()
        .filter(|l| l.contains(code))
        .count()
}

/// S6: a decline is a final response, and over UDP it is ours to deliver:
/// the 486 is resent (Timer G) until the PBX ACKs it, and not after. It used
/// to go out once; the PBX, which already had our 180, then waited for it
/// as long as it took, and the caller heard ringback.
#[tokio::test]
async fn a_decline_is_retransmitted_until_acked() {
    // No ACK: resent at T1 (50 ms) doubling to T2.
    let (pbx, seen, sock) = start_pbx_with_socket(InviteReply::Busy).await;
    let (mgr, mut rx, client) = registered_client_with(pbx, &seen, fast_timing()).await;
    let invite = incoming_invite(pbx, client, "harness-decline-lost@pbx", "INVITE");
    sock.send_to(invite.as_bytes(), client).await.unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;
    mgr.hangup(&call_id).await.expect("declined");
    tokio::time::sleep(Duration::from_millis(1000)).await;
    let sent = count_finals(&seen, " 486 ");
    assert!(sent >= 4, "the unacknowledged 486 was sent {sent} times");

    // ACKed: sent once.
    let (pbx, seen, sock) = start_pbx_with_options(PbxOptions {
        ack_invite_final: true,
        ..PbxOptions::default()
    })
    .await;
    let (mgr, mut rx, client) = registered_client_with(pbx, &seen, fast_timing()).await;
    let invite = incoming_invite(pbx, client, "harness-decline-acked@pbx", "INVITE");
    sock.send_to(invite.as_bytes(), client).await.unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;
    mgr.hangup(&call_id).await.expect("declined");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        count_finals(&seen, " 486 "),
        1,
        "the 486 was resent after its ACK"
    );
}

/// S6: the caller resends its INVITE because our 486 was lost. The copy is
/// answered with the same 486; it used to get nothing at all.
#[tokio::test]
async fn a_retransmitted_invite_for_a_declined_call_gets_the_decline_again() {
    let (pbx, seen, sock) = start_pbx_with_socket(InviteReply::Busy).await;
    // T1 of 500 ms: nothing is resent on its own within the test's window.
    let (mgr, mut rx, client) = registered_client(pbx, &seen).await;
    let invite = incoming_invite(pbx, client, "harness-decline-resent@pbx", "INVITE");
    sock.send_to(invite.as_bytes(), client).await.unwrap();
    let call_id = wait_for_call_state(&mut rx, "incoming").await;
    mgr.hangup(&call_id).await.expect("declined");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count_finals(&seen, " 486 "), 1);

    sock.send_to(invite.as_bytes(), client).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        count_finals(&seen, " 486 "),
        2,
        "the resent INVITE should get the 486 again"
    );
    let to_tags: Vec<_> = seen
        .lock()
        .unwrap()
        .responses
        .iter()
        .filter(|r| r.starts_with("SIP/2.0 486"))
        .map(|r| header(r, "To").and_then(|t| param(t, "tag")))
        .collect();
    assert_eq!(to_tags[0], to_tags[1], "the same response, To tag included");
}

/// The panel's "Re-register" on an idle, registered account is a real one:
/// the old binding is signed out (Expires: 0) and a fresh registration runs
/// the whole path again, so a stale binding or a changed network is replaced
/// rather than refreshed over a connection that may be dead.
#[tokio::test]
async fn reregister_on_an_idle_account_signs_out_and_back_in() {
    let (proxy, seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, mut rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    while rx.try_recv().is_ok() {}

    assert_eq!(mgr.reregister(cfg.clone()).await.unwrap(), "registering");
    wait_registered(&mgr, &cfg.id).await;

    let expires = seen.lock().unwrap().register_expires.clone();
    assert!(
        expires.iter().any(|e| e.as_deref() == Some("0")),
        "an Expires: 0 REGISTER signs the old binding out: {expires:?}"
    );
    assert_ne!(
        expires.last().and_then(|e| e.as_deref()),
        Some("0"),
        "and a fresh REGISTER follows: {expires:?}"
    );
    let mut saw_unregistered = false;
    let mut registered_after = false;
    while let Ok(e) = rx.try_recv() {
        if let super::SipEvent::RegistrationChanged(r) = e {
            if r.state == super::RegistrationState::Unregistered {
                saw_unregistered = true;
            } else if r.state == super::RegistrationState::Registered && saw_unregistered {
                registered_after = true;
            }
        }
    }
    assert!(
        saw_unregistered && registered_after,
        "the UI sees the sign-out and the new registration"
    );
}

/// With a call up, "Re-register" only refreshes: signing out would drop the
/// account and the call with it.
#[tokio::test]
async fn reregister_during_a_call_only_refreshes() {
    let (proxy, seen, _) = start_pbx_with_options(PbxOptions::default()).await;
    let (mgr, mut rx) = manager_with(fast_timing());
    let cfg = account(proxy, None, PASSWORD);
    register(&mgr, cfg.clone()).await.expect("registered");
    {
        let mut s = mgr.state.write().await;
        s.accounts
            .get_mut(&cfg.id)
            .unwrap()
            .calls
            .push(super::state::CallFSM::new_outbound(
                &cfg.id,
                "sip:200@harness.lan",
                "call-2".into(),
                "tag-2".into(),
                40000,
                "z9hG4bK-2".into(),
                "sip:ghostbook@harness.lan".into(),
            ));
    }
    while rx.try_recv().is_ok() {}
    let before = seen.lock().unwrap().register_ok.len();
    assert_eq!(mgr.reregister(cfg.clone()).await.unwrap(), "refreshing");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().register_ok.len() == before {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no refresh REGISTER arrived"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wait_registered(&mgr, &cfg.id).await;
    let s = mgr.state.read().await;
    let a = s.accounts.get(&cfg.id).unwrap();
    assert_eq!(a.calls.len(), 1, "the call survived");
    assert!(
        !seen
            .lock()
            .unwrap()
            .register_expires
            .iter()
            .any(|e| e.as_deref() == Some("0")),
        "no sign-out"
    );
}
