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
    /// Every SUBSCRIBE, raw.
    subscribes: Vec<String>,
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
    /// 180 Ringing, and a 200 OK sent the moment the caller's CANCEL arrives:
    /// the callee picked up as the caller gave up.
    AnswerOnCancel,
}

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
}

impl Default for PbxOptions {
    fn default() -> Self {
        Self {
            invite_reply: InviteReply::Busy,
            register_expires: 300,
            drop_next_register: Arc::new(AtomicBool::new(false)),
            reject_subscribe_auth: false,
            subscribe_expires: 600,
        }
    }
}

/// A manager with its background loops running, as the app has one: the
/// subscription refresh tests depend on the refresh timer. Construction alone
/// starts nothing (see `SipManager::start_background_tasks`).
fn manager() -> (SipManager, tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>) {
    manager_with(super::Timing::default())
}

fn manager_with(timing: super::Timing) -> (SipManager, tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>) {
    let (mgr, rx) = SipManager::new_with_timing(timing);
    mgr.start_background_tasks();
    (mgr, rx)
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
        None => rest[..rest.find(',').unwrap_or(rest.len())].trim().to_owned(),
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
    let to = if to.contains("tag=") || code == 100 { to.to_owned() } else { format!("{to};tag=pbx1") };
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
    start_pbx_with_options(PbxOptions { invite_reply, ..PbxOptions::default() }).await
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
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
            let msg = String::from_utf8_lossy(&buf[..n]).into_owned();
            let method = msg.split_whitespace().next().unwrap_or("");
            if method == "SIP/2.0" {
                rec.lock().unwrap().responses.push(msg.clone());
                continue;
            }
            if method != "REGISTER" {
                rec.lock().unwrap().methods.push(method.to_owned());
            }
            if method == "REGISTER" {
                let cseq = header(&msg, "CSeq").map(str::to_owned);
                if opts.drop_next_register.swap(false, Ordering::SeqCst) {
                    dropping_cseq = cseq.clone();
                }
                if dropping_cseq.is_some() && dropping_cseq == cseq {
                    continue;
                }
            }
            let reply = match method {
                "SUBSCRIBE" if opts.reject_subscribe_auth => {
                    rec.lock().unwrap().subscribes.push(msg.clone());
                    nonce_n += 1;
                    respond(&msg, 401, "Unauthorized", &[format!(
                        "WWW-Authenticate: Digest realm=\"{REALM}\", nonce=\"n{nonce_n}\", algorithm=MD5, qop=\"auth\""
                    )])
                }
                "SUBSCRIBE" => {
                    rec.lock().unwrap().subscribes.push(msg.clone());
                    respond(&msg, 200, "OK", &[format!("Expires: {}", opts.subscribe_expires)])
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
                        let ha2 = md5_hex(&format!("REGISTER:{}", param(auth, "uri").unwrap_or_default()));
                        let expected = match param(auth, "qop") {
                            Some(qop) => md5_hex(&format!(
                                "{ha1}:{}:{}:{}:{qop}:{ha2}",
                                param(auth, "nonce").unwrap_or_default(),
                                param(auth, "nc").unwrap_or_default(),
                                param(auth, "cnonce").unwrap_or_default()
                            )),
                            None => md5_hex(&format!("{ha1}:{}:{ha2}", param(auth, "nonce").unwrap_or_default())),
                        };
                        let ok = param(auth, "username").as_deref() == Some(USER)
                            && param(auth, "response").as_deref() == Some(expected.as_str());
                        {
                            let mut s = rec.lock().unwrap();
                            s.register_realms.push(realm);
                            s.register_ok.push(ok);
                            s.register_expires.push(header(&msg, "Expires").map(str::to_owned));
                        }
                        if ok {
                            rec.lock().unwrap().client = Some(from);
                            let contact = header(&msg, "Contact").unwrap_or_default().to_owned();
                            let expires = if header(&msg, "Expires") == Some("0") { 0 } else { opts.register_expires };
                            respond(&msg, 200, "OK", &[format!("Contact: {contact};expires={expires}"), format!("Expires: {expires}")])
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
                    let _ = sock.send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from).await;
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
                    let _ = sock.send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from).await;
                    respond(&msg, 200, "OK", &[format!("Contact: <sip:11@{addr}>")])
                }
                "INVITE" if invite_reply == InviteReply::AnswerOnCancel => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    let _ = sock.send_to(respond(&msg, 100, "Trying", &[]).as_bytes(), from).await;
                    respond(&msg, 180, "Ringing", &[])
                }
                "CANCEL" if invite_reply == InviteReply::AnswerOnCancel => {
                    let invite = rec.lock().unwrap().invites.last().cloned();
                    rec.lock().unwrap().cancels.push(msg.clone());
                    let _ = sock.send_to(respond(&msg, 200, "OK", &[]).as_bytes(), from).await;
                    // The callee answered at the same moment: the 200 for the
                    // INVITE goes out after the 200 for the CANCEL.
                    match invite {
                        Some(invite) => respond(&invite, 200, "OK", &[format!("Contact: <sip:11@{addr}>")]),
                        None => continue,
                    }
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
    assert!(result.is_ok(), "did not register: {result:?}; realms tried {:?}", s.register_realms);
    assert_eq!(s.register_realms, ["harness.lan", REALM], "override first, then the server's realm");
    assert_eq!(s.register_ok, [false, true]);
}

#[tokio::test]
async fn a_blank_realm_registers_on_the_first_attempt() {
    let (pbx, seen) = start_pbx().await;
    let (mgr, _rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
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
    assert!(s.register_realms.len() <= 2, "retried {} times", s.register_realms.len());
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
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
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
    let c_line = body.lines().find(|l| l.starts_with("c=")).expect("an SDP c= line");
    let o_line = body.lines().find(|l| l.starts_with("o=")).expect("an SDP o= line");

    assert!(!c_line.contains("0.0.0.0"), "voip_stack would withhold audio: {c_line}");
    assert!(!o_line.contains("0.0.0.0"), "{o_line}");
    assert_eq!(c_line, "c=IN IP4 127.0.0.1", "a LAN PBX must get the local address");
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
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
    let call_id = mgr.make_call("sip:11@harness.lan").await.unwrap_or_else(|e| panic!("call placed: {e}"));

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
    assert_eq!(connected, 1, "connected reported {connected} times: {states:?}");
    assert_eq!(seen.lock().unwrap().acks, 2, "each 200 needs its own ACK");
    assert_eq!(ended_direction.as_deref(), Some("outbound"));
}

/// An INVITE from the PBX to the registered client, as an incoming call.
fn incoming_invite(pbx: SocketAddr, client: SocketAddr, call_id: &str, cseq_method: &str) -> String {
    let sdp = "v=0\r\no=pbx 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0\r\n";
    let (method, body) = if cseq_method == "CANCEL" { ("CANCEL", "") } else { ("INVITE", sdp) };
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

/// Register, and return the manager, its events, and where the PBX can reach it.
async fn registered_client(
    pbx: SocketAddr,
    seen: &Arc<Mutex<Seen>>,
) -> (SipManager, tokio::sync::mpsc::UnboundedReceiver<super::SipEvent>, SocketAddr) {
    let (mgr, rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
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
        invite_replies.iter().filter(|l| l.contains(" 180 ")).count() >= 2,
        "the resent INVITE should get the 180 again: {invite_replies:?}"
    );
    assert!(
        invite_replies.iter().any(|l| l.contains(" 487 ")),
        "the cancelled INVITE needs a 487: {invite_replies:?}"
    );
    assert!(status_lines(&seen, "CANCEL").iter().any(|l| l.contains(" 200 ")));
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
    assert!(!methods.iter().any(|m| m == "BYE"), "sent a BYE for an unanswered call: {methods:?}");
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
    header(msg, "Via").and_then(|v| param(v, "branch")).unwrap_or_default()
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
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
    let call_id = mgr.make_call("sip:11@harness.lan").await.unwrap_or_else(|e| panic!("call placed: {e}"));

    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "never connected: {states:?}");

    // Outlive the PBX's 407 retransmissions (3 × 300 ms); none may end the call.
    let later = call_states_until(&mut rx, &call_id, "ended", Duration::from_millis(1500)).await;
    assert!(!later.contains(&"ended".to_owned()), "a retransmitted 407 ended the call: {later:?}");

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
    assert_eq!(header(bye, "CSeq"), Some("3 BYE"), "CSeq continues after the auth retry");
    assert!(s.cancels.is_empty(), "no CANCEL for an answered call");
}

/// The SDP line starting with `prefix` in a message's body.
fn sdp_line<'a>(msg: &'a str, prefix: &str) -> Option<&'a str> {
    msg.split("\r\n\r\n").nth(1)?.lines().find(|l| l.starts_with(prefix))
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
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
    let call_id = mgr.make_call("sip:11@harness.lan").await.unwrap_or_else(|e| panic!("call placed: {e}"));
    let states = call_states_until(&mut rx, &call_id, "connected", Duration::from_secs(5)).await;
    assert!(states.contains(&"connected".to_owned()), "never connected: {states:?}");

    {
        let s = seen.lock().unwrap();
        assert_eq!(s.invites.len(), 2, "one INVITE, one authenticated retry");
        let (first, retry) = (&s.invites[0], &s.invites[1]);
        assert!(header(retry, "Proxy-Authorization").is_some(), "the second INVITE carries credentials");
        for prefix in ["c=", "m="] {
            let a = sdp_line(first, prefix).unwrap_or_else(|| panic!("first INVITE has no {prefix} line"));
            let b = sdp_line(retry, prefix).unwrap_or_else(|| panic!("retried INVITE has no {prefix} line"));
            assert_eq!(a, b, "the authenticated INVITE changed its {prefix} line");
        }
        // The origin's session id is fresh per offer; its address is not.
        let origin_addr = |msg: &str| sdp_line(msg, "o=").and_then(|o| o.split_whitespace().last()).map(str::to_owned);
        assert_eq!(origin_addr(first), origin_addr(retry), "the authenticated INVITE changed its o= address");
        assert_eq!(sdp_line(first, "c="), Some("c=IN IP4 127.0.0.1"));
    }

    mgr.hangup(&call_id).await.expect("hung up");
}

/// The caller hangs up as the callee answers. Our CANCEL and their 200 OK
/// cross: the 200 confirms a dialog the CANCEL cannot touch, so after the ACK
/// a BYE must follow (RFC 3261 §9.1, §15). Before, the 200 was ACKed and left
/// alone, and the callee stayed connected to silence.
#[tokio::test]
async fn a_200_that_crosses_our_cancel_is_acked_and_then_byed() {
    let (pbx, seen) = start_pbx_with(InviteReply::AnswerOnCancel).await;
    let (mgr, mut rx) = manager();
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");
    let call_id = mgr.make_call("sip:11@harness.lan").await.unwrap_or_else(|e| panic!("call placed: {e}"));
    call_states_until(&mut rx, &call_id, "ringing", Duration::from_secs(5)).await;

    mgr.hangup(&call_id).await.expect("cancelled");
    let states = call_states_until(&mut rx, &call_id, "ended", Duration::from_secs(5)).await;
    assert!(states.contains(&"ended".to_owned()), "{states:?}");
    assert!(!states.contains(&"connected".to_owned()), "a cancelled call must not show connected: {states:?}");
    tokio::time::sleep(Duration::from_millis(500)).await;

    let s = seen.lock().unwrap();
    let cancel = s.cancels.first().expect("a ringing call is cancelled");
    assert_eq!(header(cancel, "CSeq"), Some("1 CANCEL"), "CANCEL carries the INVITE's CSeq");
    assert_eq!(via_branch(cancel), via_branch(&s.invites[0]), "CANCEL carries the INVITE's branch");
    assert_eq!(s.acks, 1, "the crossed 200 is ACKed");
    let bye = s.byes.first().expect("the confirmed dialog is ended with BYE");
    assert!(bye.starts_with(&format!("BYE sip:11@{pbx} ")), "BYE goes to the 200's Contact: {bye}");
    let invite_from_tag = header(&s.invites[0], "From").and_then(|f| param(f, "tag")).unwrap();
    assert_eq!(header(bye, "From").and_then(|f| param(f, "tag")), Some(invite_from_tag));
    assert_eq!(header(bye, "To").and_then(|t| param(t, "tag")), Some("pbx1".to_owned()));
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
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions { register_expires: 2, ..PbxOptions::default() }).await;
    let (mgr, _rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");

    // Granted 2 s → refresh every 1 s (the floor). Three should land in 3.5 s.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    let s = seen.lock().unwrap();
    assert!(s.register_ok.len() >= 3, "only {} registrations in 3.5 s", s.register_ok.len());
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
    let opts = PbxOptions { register_expires: 2, ..PbxOptions::default() };
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
        wait_for_registration(&mut rx, &id, super::RegistrationState::Reconnecting, Duration::from_secs(4)).await,
        "an unanswered refresh must lead to reconnecting"
    );
    assert!(
        wait_for_registration(&mut rx, &id, super::RegistrationState::Registered, Duration::from_secs(4)).await,
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

    mgr.unregister_account(Some(&id)).await.expect("unregistered");

    {
        let s = seen.lock().unwrap();
        assert_eq!(s.register_ok.len(), 2, "one registration, one authenticated unregistration");
        assert_eq!(s.register_expires[1].as_deref(), Some("0"), "the unregister carries Expires: 0");
        assert!(s.register_ok[1], "the unregister was authenticated");
    }

    let (state, _) = mgr.registration_state_for_account(Some(&id)).await;
    assert_eq!(state, super::RegistrationState::Unregistered);
    assert!(
        wait_for_registration(&mut rx, &id, super::RegistrationState::Unregistered, Duration::from_secs(1)).await,
        "the UI is told the account is unregistered"
    );
}

/// A subscription refresh is sent inside the subscription the server
/// accepted — with its tag — and when the server says so, not every 60 s.
#[tokio::test]
async fn a_subscription_refresh_stays_in_the_dialog_and_follows_the_granted_expires() {
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions { subscribe_expires: 2, ..PbxOptions::default() }).await;
    let (mgr, _rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");

    mgr.subscribe_presence("sip:1002@harness.lan", super::state::EventType::Dialog)
        .await
        .expect("subscribed");

    // Granted 2 s → the refresh is due at 1 s; the timer looks every 200 ms.
    tokio::time::sleep(Duration::from_millis(1800)).await;
    let s = seen.lock().unwrap();
    assert!(s.subscribes.len() >= 2, "no refresh within the granted lifetime: {}", s.subscribes.len());
    let first = &s.subscribes[0];
    let refresh = &s.subscribes[1];
    assert!(header(first, "To").is_some_and(|t| !t.contains("tag=")), "the first SUBSCRIBE starts the dialog");
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
    let (pbx, seen, _) = start_pbx_with_options(PbxOptions { reject_subscribe_auth: true, ..PbxOptions::default() }).await;
    let (mgr, _rx) = manager_with(fast_timing());
    register(&mgr, account(pbx, None, PASSWORD)).await.expect("registered");

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
