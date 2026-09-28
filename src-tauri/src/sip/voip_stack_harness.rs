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
    /// Every INVITE, raw.
    invites: Vec<String>,
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
    let _ = env_logger::builder().is_test(true).try_init();
    let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake PBX");
    let addr = sock.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let rec = seen.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        let mut nonce_n = 0u32;
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else { return };
            let msg = String::from_utf8_lossy(&buf[..n]).into_owned();
            let method = msg.split_whitespace().next().unwrap_or("");
            let reply = match method {
                "REGISTER" => match header(&msg, "Authorization") {
                    None => {
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
                        }
                        if ok {
                            let contact = header(&msg, "Contact").unwrap_or_default().to_owned();
                            respond(&msg, 200, "OK", &[format!("Contact: {contact};expires=300"), "Expires: 300".to_owned()])
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
                "INVITE" => {
                    rec.lock().unwrap().invites.push(msg.clone());
                    // Enough to end the attempt cleanly; the SDP is the point.
                    respond(&msg, 486, "Busy Here", &[])
                }
                "ACK" => continue,
                _ => respond(&msg, 200, "OK", &[]),
            };
            let _ = sock.send_to(reply.as_bytes(), from).await;
        }
    });
    (addr, seen)
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
    let (mgr, _rx) = SipManager::new_with_receiver();
    let result = register(&mgr, account(pbx, Some("harness.lan"), PASSWORD)).await;
    let s = seen.lock().unwrap();
    assert!(result.is_ok(), "did not register: {result:?}; realms tried {:?}", s.register_realms);
    assert_eq!(s.register_realms, ["harness.lan", REALM], "override first, then the server's realm");
    assert_eq!(s.register_ok, [false, true]);
}

#[tokio::test]
async fn a_blank_realm_registers_on_the_first_attempt() {
    let (pbx, seen) = start_pbx().await;
    let (mgr, _rx) = SipManager::new_with_receiver();
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
    let (mgr, _rx) = SipManager::new_with_receiver();
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
    let (mgr, _rx) = SipManager::new_with_receiver();
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
