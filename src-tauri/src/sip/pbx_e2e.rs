//! End-to-end tests against a real PBX.
//!
//! Nothing here is faked: two `SipManager`s from this crate register on two
//! throwaway extensions of a live PBX, and one calls the other *through* it.
//! The PBX has no API to originate a call into a phone, so an inbound call —
//! the INVITE arriving, being answered, carrying media, taking DTMF, being
//! ended by the far side — could never be verified before. Here endpoint B
//! is the far side.
//!
//! Every test is `#[ignore]`: it needs the network, audio devices and two
//! extensions, so it runs on a developer machine on demand, not in CI. See
//! `scripts/pbx-e2e.sh` and the README. The credentials come from the
//! environment:
//!
//! ```text
//! ARIA_E2E_PBX_DOMAIN   SIP AOR domain, e.g. test.ariaroute.com
//! ARIA_E2E_PBX_SERVER   registrar host or host:port, e.g. sip.ariaroute.com
//! ARIA_E2E_EXT_A / ARIA_E2E_PASS_A   the extension that receives the call
//! ARIA_E2E_EXT_B / ARIA_E2E_PASS_B   the extension that places it
//! ARIA_E2E_EXT_C / ARIA_E2E_PASS_C   optional: a third, for the transfer test
//! ```
//!
//! With any of them unset the tests print why and pass, so a bare
//! `cargo test -- --ignored` on a machine without a PBX does not fail.
//!
//! Media is plain RTP unless `ARIA_E2E_SRTP_A` / `ARIA_E2E_SRTP_B` say
//! `sdes`. The test PBX accepts an SDES offer — it answers `RTP/SAVP` with a
//! key of its own — but at the time of writing it hands the *same* key to
//! both legs and the relayed packets fail SRTP authentication on both ends,
//! so no audio gets through. `sdes_offer_is_answered_by_the_pbx` covers what
//! can be asserted of that: the offer is answered and the call connects. The
//! media assertions in the other tests stay on plain RTP so they test the
//! softphone rather than the PBX's relay.

use super::account::{default_codec_preferences, AccountConfig, SrtpMode};
use super::diagnostics::MessageDirection;
use super::transport::TransportType;
use super::{RegistrationState, SipEvent, SipManager};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

/// The whole test; generous, because a real PBX is on the other end.
const TEST_BUDGET: Duration = Duration::from_secs(90);

/// Where the tests read their PBX from.
struct Env {
    domain: String,
    server: String,
    ext_a: String,
    pass_a: String,
    ext_b: String,
    pass_b: String,
    /// What each endpoint offers, from `ARIA_E2E_SRTP_A` / `ARIA_E2E_SRTP_B`
    /// (`sdes` or `disabled`; default disabled, see the module docs).
    srtp_a: SrtpMode,
    srtp_b: SrtpMode,
}

impl Env {
    /// `None`, with the reason printed, when the environment is not set up.
    fn load() -> Option<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let names = [
            "ARIA_E2E_PBX_DOMAIN",
            "ARIA_E2E_PBX_SERVER",
            "ARIA_E2E_EXT_A",
            "ARIA_E2E_PASS_A",
            "ARIA_E2E_EXT_B",
            "ARIA_E2E_PASS_B",
        ];
        let missing: Vec<&str> = names.iter().copied().filter(|n| var(n).is_none()).collect();
        if !missing.is_empty() {
            eprintln!(
                "pbx_e2e: skipped — not set: {}. See scripts/pbx-e2e.sh.",
                missing.join(", ")
            );
            return None;
        }
        Some(Self {
            domain: var(names[0])?,
            server: var(names[1])?,
            ext_a: var(names[2])?,
            pass_a: var(names[3])?,
            ext_b: var(names[4])?,
            pass_b: var(names[5])?,
            srtp_a: var("ARIA_E2E_SRTP_A").map(|v| SrtpMode::from_str(&v)).unwrap_or_default(),
            srtp_b: var("ARIA_E2E_SRTP_B").map(|v| SrtpMode::from_str(&v)).unwrap_or_default(),
        })
    }

    /// The same PBX, with both endpoints offering SDES-SRTP.
    fn with_sdes(mut self) -> Self {
        self.srtp_a = SrtpMode::Sdes;
        self.srtp_b = SrtpMode::Sdes;
        self
    }

    fn account(&self, label: &str, ext: &str, pass: &str, srtp_mode: SrtpMode) -> AccountConfig {
        let (host, port) = match self.server.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
            _ => (self.server.clone(), 5060),
        };
        AccountConfig {
            id: format!("e2e-{label}-{}", uuid::Uuid::new_v4()),
            display_name: format!("E2E harness {label}"),
            username: ext.to_string(),
            domain: self.domain.clone(),
            password: pass.to_string(),
            transport: TransportType::Udp,
            port,
            registrar: Some(host),
            outbound_proxy: None,
            auth_username: None,
            auth_realm: None,
            enabled: true,
            auto_record: false,
            tls_insecure: false,
            srtp_mode,
            codecs: default_codec_preferences(),
        }
    }

    fn uri(&self, user: &str) -> String {
        format!("sip:{user}@{}", self.domain)
    }
}

/// One registered endpoint: its manager, its events and its account id.
struct Endpoint {
    label: &'static str,
    mgr: SipManager,
    rx: UnboundedReceiver<SipEvent>,
    account_id: String,
}

/// A failed step. Carries the message; the SIP traces are printed by the
/// test wrapper, which still has both endpoints.
type Step<T> = Result<T, String>;

macro_rules! check {
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!($($arg)+));
        }
    };
}

impl Endpoint {
    /// Register `ext` and wait until the registrar has accepted it.
    async fn register(
        label: &'static str,
        env: &Env,
        ext: &str,
        pass: &str,
        srtp_mode: SrtpMode,
    ) -> Step<Self> {
        let cfg = env.account(label, ext, pass, srtp_mode);
        let account_id = cfg.id.clone();
        let (mgr, rx) = SipManager::new_with_receiver();
        mgr.register(cfg)
            .await
            .map_err(|e| format!("{label}: register() refused: {e}"))?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let (state, err) = mgr.registration_state_for_account(Some(&account_id)).await;
            match state {
                RegistrationState::Registered => break,
                RegistrationState::Error => {
                    return Err(format!("{label}: registration failed: {}", err.unwrap_or_default()))
                }
                _ if tokio::time::Instant::now() >= deadline => {
                    return Err(format!("{label}: registration did not settle in 15 s ({state:?})"))
                }
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        eprintln!(
            "[{label}] registered {ext}@{} via {} (srtp {srtp_mode:?})",
            env.domain, env.server
        );
        Ok(Self { label, mgr, rx, account_id })
    }

    /// The next call event in one of `states`, for `call_id` when given.
    ///
    /// Everything else on the channel (registration, diagnostics, presence)
    /// is dropped on the way.
    async fn next_call_event(
        &mut self,
        call_id: Option<&str>,
        states: &[&str],
        timeout: Duration,
    ) -> Step<super::CallEventPayload> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let event = match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => return Err(format!("{}: event channel closed", self.label)),
                Err(_) => {
                    return Err(format!(
                        "{}: no {} event{} within {timeout:?}",
                        self.label,
                        states.join("/"),
                        call_id.map(|c| format!(" for call {c}")).unwrap_or_default()
                    ))
                }
            };
            match event {
                SipEvent::CallStateChanged(e)
                    if states.contains(&e.state.as_str())
                        && call_id.is_none_or(|id| id == e.call_id) =>
                {
                    eprintln!(
                        "[{}] call {} -> {} ({}, remote {})",
                        self.label, e.call_id, e.state, e.direction, e.remote_uri
                    );
                    return Ok(e);
                }
                SipEvent::CallStateChanged(e) => {
                    eprintln!("[{}] call {} -> {} (passing)", self.label, e.call_id, e.state);
                }
                SipEvent::TransferProgress(t) => {
                    eprintln!("[{}] transfer {}: {} {}", self.label, t.call_id, t.status, t.message);
                }
                _ => {}
            }
        }
    }

    /// Wait for `call_id` to reach `state`.
    async fn wait_state(&mut self, call_id: &str, state: &str, timeout: Duration) -> Step<()> {
        self.next_call_event(Some(call_id), &[state], timeout).await.map(|_| ())
    }

    /// The manager's view of `call_id`, as the diagnostics panel sees it.
    async fn status(&self, call_id: &str) -> Step<super::CallStatusInfo> {
        self.mgr
            .get_system_status()
            .await
            .accounts
            .into_iter()
            .flat_map(|a| a.active_calls)
            .find(|c| c.id == call_id)
            .ok_or_else(|| format!("{}: call {call_id} is not in the status report", self.label))
    }

    /// Assert the call is carrying RTP in both directions.
    async fn assert_media_flowing(&self, call_id: &str) -> Step<super::media::RtpStats> {
        let status = self.status(call_id).await?;
        let stats = status
            .rtp_stats
            .ok_or_else(|| format!("{}: call {call_id} has no media session", self.label))?;
        eprintln!(
            "[{}] rtp: sent {} pkts / {} B, received {} pkts / {} B, lost {}, jitter {:.1} ms, codec {}, srtp {}",
            self.label,
            stats.packets_sent,
            stats.bytes_sent,
            stats.packets_received,
            stats.bytes_received,
            stats.packets_lost,
            stats.jitter_ms,
            stats.codec_name,
            status.srtp_active
        );
        check!(stats.packets_sent > 0, "{}: no RTP sent on call {call_id}", self.label);
        check!(stats.packets_received > 0, "{}: no RTP received on call {call_id}", self.label);
        Ok(stats)
    }

    /// Every SDP `c=` line and `m=` port this side put in the messages it
    /// sent whose start line begins with `start` (a method, or `SIP/2.0 200`)
    /// in the dialog `sip_call_id` must be the same: the media address decided
    /// for a call is not allowed to change between the INVITE and its
    /// authenticated retry, or between the first answer and a re-INVITE's.
    /// Prints what it found, so a passing run shows the addresses too.
    async fn assert_consistent_sdp(&self, sip_call_id: &str, start: &str) -> Step<Vec<(String, String)>> {
        let found: Vec<(String, String)> = self
            .mgr
            .get_diagnostics()
            .await
            .into_iter()
            .filter(|log| {
                matches!(log.direction, MessageDirection::Sent)
                    && log.call_id.as_deref() == Some(sip_call_id)
                    && log.raw.starts_with(start)
                    && log.raw.contains("\r\nm=audio ")
            })
            .map(|log| {
                let body = log.raw.split("\r\n\r\n").nth(1).unwrap_or_default();
                let line = |prefix: &str| {
                    body.lines().find(|l| l.starts_with(prefix)).unwrap_or_default().to_owned()
                };
                (line("c="), line("m="))
            })
            .collect();
        for (c, m) in &found {
            eprintln!("[{}] sent {start}: {c}  {m}", self.label);
        }
        check!(!found.is_empty(), "{}: sent no {start} with SDP for {sip_call_id}", self.label);
        let (c0, m0) = &found[0];
        let port = |m: &str| m.split_whitespace().nth(1).unwrap_or_default().to_owned();
        for (c, m) in &found[1..] {
            check!(c == c0, "{}: {start} changed its media address: {c0} then {c}", self.label);
            check!(port(m) == port(m0), "{}: {start} changed its media port: {m0} then {m}", self.label);
        }
        Ok(found)
    }

    /// Print this side's SIP trace, for a failed run.
    async fn dump_trace(&self) {
        eprintln!("---- SIP trace, endpoint {} ----", self.label);
        for log in self.mgr.get_diagnostics().await {
            eprintln!(
                "{} {:?} {} {}\n{}",
                log.timestamp,
                log.direction,
                log.remote_addr,
                log.summary,
                log.raw.trim_end()
            );
        }
        eprintln!("---- end of trace, endpoint {} ----", self.label);
    }

    async fn unregister(&self) -> Step<()> {
        self.mgr
            .unregister_account(Some(&self.account_id))
            .await
            .map_err(|e| format!("{}: unregister failed: {e}", self.label))?;
        let (state, _) = self.mgr.registration_state_for_account(Some(&self.account_id)).await;
        check!(
            !matches!(state, RegistrationState::Registered),
            "{}: still registered after unregister: {state:?}",
            self.label
        );
        eprintln!("[{}] unregistered", self.label);
        Ok(())
    }
}

/// Run `body` with both endpoints registered; on failure print both SIP
/// traces and panic with the step that failed.
async fn with_endpoints<F, Fut>(env: &Env, body: F)
where
    F: FnOnce(Endpoint, Endpoint) -> Fut,
    Fut: std::future::Future<Output = (Endpoint, Endpoint, Step<()>)>,
{
    let _ = env_logger::builder().is_test(true).try_init();
    let a = Endpoint::register("A", env, &env.ext_a, &env.pass_a, env.srtp_a).await;
    let b = Endpoint::register("B", env, &env.ext_b, &env.pass_b, env.srtp_b).await;
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (a, b) => {
            for e in [&a, &b].into_iter().flatten() {
                e.dump_trace().await;
            }
            panic!("{}", a.err().or(b.err()).unwrap_or_default());
        }
    };

    let (a, b, outcome) = match tokio::time::timeout(TEST_BUDGET, body(a, b)).await {
        Ok(done) => done,
        Err(_) => panic!("test did not finish within {TEST_BUDGET:?}"),
    };

    // Always leave the registrar clean, and report the step failure after.
    let cleanup = async {
        a.unregister().await?;
        b.unregister().await
    }
    .await;

    if let Err(msg) = outcome.and(cleanup) {
        a.dump_trace().await;
        b.dump_trace().await;
        panic!("{msg}");
    }
}

/// B calls A through the PBX. A sees the INVITE as an inbound call and
/// answers; audio flows both ways; B sends DTMF; A holds and resumes; B hangs
/// up and A sees the BYE.
#[ignore = "needs a live PBX, two extensions and audio devices; see scripts/pbx-e2e.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn inbound_call_through_the_pbx() {
    let Some(env) = Env::load() else { return };
    let env = &env;
    with_endpoints(env, |mut a, mut b| async move {
        let outcome: Step<()> = async {
            // B dials A's extension; the PBX routes it to A's registration.
            let b_call = b
                .mgr
                .make_call(&env.uri(&env.ext_a))
                .await
                .map_err(|e| format!("B: make_call failed: {e}"))?;
            eprintln!("[B] dialing {} as call {b_call}", env.uri(&env.ext_a));

            // A sees the inbound INVITE.
            let incoming = a.next_call_event(None, &["incoming"], Duration::from_secs(10)).await?;
            let a_call = incoming.call_id.clone();
            check!(incoming.direction == "inbound", "A: direction was {:?}, not inbound", incoming.direction);
            check!(
                incoming.remote_uri.contains(&env.ext_b),
                "A: inbound call is from {:?}, expected extension {}",
                incoming.remote_uri,
                env.ext_b
            );
            let a_status = a.status(&a_call).await?;
            check!(a_status.direction == "inbound", "A: status direction {:?}", a_status.direction);
            eprintln!("[A] inbound INVITE from {} (SIP Call-ID {})", incoming.remote_uri, a_status.sip_call_id);

            // A answers; both legs connect.
            a.mgr.answer(&a_call).await.map_err(|e| format!("A: answer failed: {e}"))?;
            let a_connected = a.next_call_event(Some(&a_call), &["connected"], Duration::from_secs(10)).await?;
            let b_connected = b.next_call_event(Some(&b_call), &["connected"], Duration::from_secs(10)).await?;
            eprintln!(
                "[A] answered; srtp_active={:?}  [B] connected; srtp_active={:?}",
                a_connected.srtp_active, b_connected.srtp_active
            );

            // Media: give the RTP streams a moment, then both must have
            // sent and received.
            tokio::time::sleep(Duration::from_secs(3)).await;
            a.assert_media_flowing(&a_call).await?;
            b.assert_media_flowing(&b_call).await?;

            // The PBX challenges the INVITE, so B sent it twice. Both must
            // offer the same media address: the retry used to rebuild its
            // SDP from the registration-time address and flip `c=` from the
            // STUN-discovered public address to the LAN one.
            let b_sip_call_id = b.status(&b_call).await?.sip_call_id;
            b.assert_consistent_sdp(&b_sip_call_id, "INVITE").await?;
            // Likewise every 200 OK A sent for an INVITE in its dialog: the
            // first answer and any re-INVITE refresh the PBX sent meanwhile.
            let a_sip_call_id = a.status(&a_call).await?.sip_call_id;
            a.assert_consistent_sdp(&a_sip_call_id, "SIP/2.0 200").await?;

            // DTMF from the caller. rtp-engine has no receive-side DTMF
            // counter, so this is only asserted not to fail.
            b.mgr.send_dtmf(&b_call, "1").await.map_err(|e| format!("B: send_dtmf failed: {e}"))?;
            eprintln!("[B] sent DTMF 1");

            // Hold and resume on the callee. Hold is local (the FSM state and
            // muted media), so it is asserted from the status report.
            a.mgr.hold(&a_call, true).await.map_err(|e| format!("A: hold failed: {e}"))?;
            let held = a.status(&a_call).await?;
            check!(held.state == "held", "A: state after hold was {:?}", held.state);
            eprintln!("[A] held");
            tokio::time::sleep(Duration::from_secs(1)).await;
            a.mgr.hold(&a_call, false).await.map_err(|e| format!("A: resume failed: {e}"))?;
            let resumed = a.status(&a_call).await?;
            check!(resumed.state == "connected", "A: state after resume was {:?}", resumed.state);
            eprintln!("[A] resumed");

            // The call still carries audio after the hold.
            tokio::time::sleep(Duration::from_secs(1)).await;
            let before = a.status(&a_call).await?.rtp_stats.map(|s| s.packets_received).unwrap_or(0);
            tokio::time::sleep(Duration::from_secs(1)).await;
            let after = a.status(&a_call).await?.rtp_stats.map(|s| s.packets_received).unwrap_or(0);
            check!(after > before, "A: no RTP received after resume ({before} -> {after})");

            // B hangs up; A sees the BYE.
            b.mgr.hangup(&b_call).await.map_err(|e| format!("B: hangup failed: {e}"))?;
            eprintln!("[B] hung up");
            let ended = a.next_call_event(Some(&a_call), &["ended"], Duration::from_secs(5)).await?;
            check!(ended.direction == "inbound", "A: ended event direction {:?}", ended.direction);
            eprintln!("[A] saw BYE; call ended");
            Ok(())
        }
        .await;
        (a, b, outcome)
    })
    .await;
}

/// Both endpoints offer SDES-SRTP. Asserted: the PBX answers the offer and
/// the call connects on both legs. Whether the relayed media then
/// authenticates is reported, not asserted — see the module docs.
#[ignore = "needs a live PBX, two extensions and audio devices; see scripts/pbx-e2e.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn sdes_offer_is_answered_by_the_pbx() {
    let Some(env) = Env::load() else { return };
    let env = &env.with_sdes();
    with_endpoints(env, |mut a, mut b| async move {
        let outcome: Step<()> = async {
            let b_call = b
                .mgr
                .make_call(&env.uri(&env.ext_a))
                .await
                .map_err(|e| format!("B: make_call failed: {e}"))?;
            let incoming = a.next_call_event(None, &["incoming"], Duration::from_secs(10)).await?;
            let a_call = incoming.call_id.clone();
            a.mgr.answer(&a_call).await.map_err(|e| format!("A: answer failed: {e}"))?;
            let a_connected = a.next_call_event(Some(&a_call), &["connected"], Duration::from_secs(10)).await?;
            let b_connected = b.next_call_event(Some(&b_call), &["connected"], Duration::from_secs(10)).await?;
            eprintln!(
                "[A] answered with srtp_active={:?}; [B] connected with srtp_active={:?}",
                a_connected.srtp_active, b_connected.srtp_active
            );

            tokio::time::sleep(Duration::from_secs(3)).await;
            for (ep, call) in [(&a, &a_call), (&b, &b_call)] {
                let status = ep.status(call).await?;
                let stats = status
                    .rtp_stats
                    .ok_or_else(|| format!("{}: call {call} has no media session", ep.label))?;
                check!(stats.packets_sent > 0, "{}: no RTP sent on call {call}", ep.label);
                eprintln!(
                    "[{}] srtp_active={} sent {} received {} (0 received with srtp_active means \
                     the PBX's relayed SRTP did not authenticate)",
                    ep.label, status.srtp_active, stats.packets_sent, stats.packets_received
                );
            }

            b.mgr.hangup(&b_call).await.map_err(|e| format!("B: hangup failed: {e}"))?;
            a.wait_state(&a_call, "ended", Duration::from_secs(5)).await?;
            Ok(())
        }
        .await;
        (a, b, outcome)
    })
    .await;
}

/// A calls an extension that has voicemail and no phone registered, so its
/// greeting answers: an outbound call that connects and carries media.
///
/// The default is 1001 rather than `*97`: `*97` answers but plays nothing for
/// an extension without a mailbox, and the throwaway extensions have none.
#[ignore = "needs a live PBX, two extensions and audio devices; see scripts/pbx-e2e.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn outbound_call_to_voicemail() {
    let Some(env) = Env::load() else { return };
    let env = &env;
    with_endpoints(env, |mut a, b| async move {
        let outcome: Step<()> = async {
            let target = std::env::var("ARIA_E2E_VOICEMAIL").unwrap_or_else(|_| "1001".into());
            let call = a
                .mgr
                .make_call(&env.uri(&target))
                .await
                .map_err(|e| format!("A: make_call failed: {e}"))?;
            eprintln!("[A] dialing {} as call {call}", env.uri(&target));
            let connected = a.next_call_event(Some(&call), &["connected"], Duration::from_secs(15)).await?;
            check!(connected.direction == "outbound", "A: direction {:?}", connected.direction);
            tokio::time::sleep(Duration::from_secs(3)).await;
            a.assert_media_flowing(&call).await?;
            a.mgr.hangup(&call).await.map_err(|e| format!("A: hangup failed: {e}"))?;
            a.wait_state(&call, "ended", Duration::from_secs(5)).await?;
            eprintln!("[A] hung up; call ended");
            Ok(())
        }
        .await;
        (a, b, outcome)
    })
    .await;
}

/// B calls A; A answers and blind-transfers B to a third extension, C.
///
/// This PBX honours a REFER only towards a local extension with a registered
/// phone — never towards voicemail or an outside number — so the transfer
/// target has to be a third registered endpoint. It comes from
/// `ARIA_E2E_EXT_C` / `ARIA_E2E_PASS_C`; without them the test says so and
/// passes. Asserted: C gets an inbound call from B, A's leg ends once the
/// PBX reports the transfer complete, and B is still in a call afterwards.
#[ignore = "needs a live PBX, three extensions and audio devices; see scripts/pbx-e2e.sh"]
#[tokio::test(flavor = "multi_thread")]
async fn blind_transfer_through_the_pbx() {
    let Some(env) = Env::load() else { return };
    let env = &env;
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let (Some(ext_c), Some(pass_c)) = (var("ARIA_E2E_EXT_C"), var("ARIA_E2E_PASS_C")) else {
        eprintln!(
            "pbx_e2e: blind_transfer_through_the_pbx skipped — ARIA_E2E_EXT_C / ARIA_E2E_PASS_C \
             not set. The PBX only transfers to an extension with a registered phone, so the \
             test needs a third one."
        );
        return;
    };
    with_endpoints(env, |mut a, mut b| async move {
        let mut c = match Endpoint::register("C", env, &ext_c, &pass_c, env.srtp_a).await {
            Ok(c) => c,
            Err(e) => return (a, b, Err(e)),
        };
        let outcome: Step<()> = async {
            let b_call = b
                .mgr
                .make_call(&env.uri(&env.ext_a))
                .await
                .map_err(|e| format!("B: make_call failed: {e}"))?;
            let incoming = a.next_call_event(None, &["incoming"], Duration::from_secs(10)).await?;
            let a_call = incoming.call_id.clone();
            a.mgr.answer(&a_call).await.map_err(|e| format!("A: answer failed: {e}"))?;
            a.wait_state(&a_call, "connected", Duration::from_secs(10)).await?;
            b.wait_state(&b_call, "connected", Duration::from_secs(10)).await?;
            tokio::time::sleep(Duration::from_secs(2)).await;

            a.mgr
                .transfer_blind(&a_call, &env.uri(&ext_c))
                .await
                .map_err(|e| format!("A: transfer_blind failed: {e}"))?;
            eprintln!("[A] sent REFER to {}", env.uri(&ext_c));

            // The PBX rings C with B's call.
            let c_incoming = c.next_call_event(None, &["incoming"], Duration::from_secs(15)).await?;
            let c_call = c_incoming.call_id.clone();
            check!(
                c_incoming.remote_uri.contains(&env.ext_b),
                "C: transferred call is from {:?}, expected extension {}",
                c_incoming.remote_uri,
                env.ext_b
            );
            c.mgr.answer(&c_call).await.map_err(|e| format!("C: answer failed: {e}"))?;
            c.wait_state(&c_call, "connected", Duration::from_secs(10)).await?;
            eprintln!("[C] answered the transferred call");

            // A's leg ends once the PBX reports the transfer complete (a
            // NOTIFY with a 200 sipfrag, or a BYE). A refusal is a failure.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let event = match tokio::time::timeout_at(deadline, a.rx.recv()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => return Err("A: event channel closed".into()),
                    Err(_) => return Err("A: leg did not end within 20 s of the transfer".into()),
                };
                match event {
                    SipEvent::TransferProgress(t) if t.call_id == a_call => {
                        eprintln!("[A] transfer: {} {}", t.status, t.message);
                        check!(t.status < 400, "A: PBX refused the REFER: {} {}", t.status, t.message);
                    }
                    SipEvent::CallStateChanged(e) if e.call_id == a_call && e.state == "ended" => {
                        eprintln!("[A] leg ended after the transfer");
                        break;
                    }
                    _ => {}
                }
            }

            // B is still in its call, now with C. Both sides' RTP counters
            // are reported, not asserted: the PBX hands C the relay port A's
            // leg was on, and whether its relay re-learns the new peer's
            // address after a transfer is the PBX's business, not the
            // softphone's (at the time of writing it does not, and C hears
            // nothing).
            tokio::time::sleep(Duration::from_secs(3)).await;
            let b_status = b.status(&b_call).await?;
            check!(
                b_status.state == "connected",
                "B: expected to still be in the call after the transfer, state {:?}",
                b_status.state
            );
            for (ep, call) in [(&b, &b_call), (&c, &c_call)] {
                if let Some(stats) = ep.status(call).await?.rtp_stats {
                    eprintln!(
                        "[{}] after the transfer: sent {} pkts, received {} pkts",
                        ep.label, stats.packets_sent, stats.packets_received
                    );
                }
            }

            b.mgr.hangup(&b_call).await.map_err(|e| format!("B: hangup failed: {e}"))?;
            b.wait_state(&b_call, "ended", Duration::from_secs(5)).await?;
            c.wait_state(&c_call, "ended", Duration::from_secs(5)).await?;
            eprintln!("[C] saw BYE; transferred call ended");
            Ok(())
        }
        .await;
        let outcome = outcome.and(c.unregister().await);
        if outcome.is_err() {
            c.dump_trace().await;
        }
        (a, b, outcome)
    })
    .await;
}
