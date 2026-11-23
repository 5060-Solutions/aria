//! Client transactions (RFC 3261 §17.1): retransmission over UDP and the
//! deadline that says a request went unanswered.
//!
//! sip-5060 has no transaction layer, so every request Aria sent over UDP went
//! out once, and a single lost datagram lost the INVITE, BYE, CANCEL or REFER
//! with it. Requests now go through `send_request`, which registers the
//! request's Via branch, sends it, and keeps resending it until a response on
//! that branch arrives or the deadline passes:
//!
//! - INVITE (Timer A/B): resent at T1, doubling, until any response at all
//!   (a 1xx moves the transaction to Proceeding, where the UAC no longer
//!   retransmits). With no response within 64*T1 it has timed out.
//! - Everything else (Timer E/F): resent at T1 doubling up to T2, and at T2
//!   once a provisional response has come, until a final response. With no
//!   final response within Timer F it has timed out.
//!
//! Over TCP and TLS the transport delivers the request, so nothing is resent;
//! only the deadline applies.
//!
//! The response side is `Transactions::on_response`, which the response
//! dispatcher calls for every response before handing it to its handler.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use super::builder::extract_via_branch;
use super::transport::SipTransport;
use super::Timing;

/// RFC 3261 T2: the longest interval between retransmissions of a non-INVITE
/// request (and of a 2xx or non-2xx response to an INVITE).
pub(crate) const T2: Duration = Duration::from_secs(4);

/// Which client transaction state machine a request runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// INVITE: Timer A/B, stops at the first response of any kind.
    Invite,
    /// Everything else: Timer E/F, stops at a final response.
    NonInvite,
}

/// How a client transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A response arrived that ends the retransmissions: any response for an
    /// INVITE, a final one for anything else. Carries its status.
    Answered(u16),
    /// Timer B or F fired: no (final) response in time.
    TimedOut,
}

/// The client transactions in flight, by the Via branch of their request.
///
/// Each entry holds the highest status seen so far on that branch (0 before
/// any response), which the retransmission task watches.
#[derive(Default)]
pub(crate) struct Transactions {
    inner: Mutex<HashMap<String, watch::Sender<u16>>>,
}

impl Transactions {
    fn begin(&self, branch: &str) -> watch::Receiver<u16> {
        let (tx, rx) = watch::channel(0);
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).insert(branch.to_owned(), tx);
        rx
    }

    fn end(&self, branch: &str) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).remove(branch);
    }

    /// Record a response to one of our requests. A response for a branch
    /// with no transaction in flight (a retransmitted final response, one for
    /// a request sent without `send_request`) is ignored here; its handler
    /// still sees it.
    pub(crate) fn on_response(&self, response: &str, status: u16) {
        let Some(branch) = extract_via_branch(response) else { return };
        let map = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = map.get(&branch) {
            tx.send_if_modified(|seen| {
                let higher = status > *seen;
                if higher {
                    *seen = status;
                }
                higher
            });
        }
    }

    /// Whether a transaction is in flight on `branch`. For tests.
    #[cfg(test)]
    pub(crate) fn in_flight(&self, branch: &str) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).contains_key(branch)
    }
}

/// Send `msg` to `dest` and see its client transaction through: resend it over
/// UDP until it is answered, and report when it never is.
///
/// The first copy is sent before this returns, so a transport error comes back
/// to the caller as it did when requests were sent directly. The rest runs in
/// a task whose handle is returned; drop it if the outcome does not matter.
/// `deadline` overrides Timer B/F (the OPTIONS keepalive waits less).
pub(crate) async fn send_request(
    transactions: &Arc<Transactions>,
    transport: &SipTransport,
    dest: SocketAddr,
    msg: String,
    kind: Kind,
    timing: Timing,
    deadline: Option<Duration>,
) -> Result<tokio::task::JoinHandle<Outcome>, String> {
    let branch = extract_via_branch(&msg).unwrap_or_default();
    let seen = transactions.begin(&branch);
    if let Err(e) = transport.send_to(msg.as_bytes(), dest).await {
        transactions.end(&branch);
        return Err(e);
    }
    let deadline = deadline.unwrap_or(match kind {
        Kind::Invite => timing.t1 * 64,
        Kind::NonInvite => timing.timer_f,
    });
    let transactions = transactions.clone();
    let transport = transport.clone();
    Ok(tokio::spawn(async move {
        let outcome = retransmit(&transport, dest, &msg, kind, timing.t1, deadline, seen).await;
        transactions.end(&branch);
        outcome
    }))
}

async fn retransmit(
    transport: &SipTransport,
    dest: SocketAddr,
    msg: &str,
    kind: Kind,
    t1: Duration,
    deadline: Duration,
    mut seen: watch::Receiver<u16>,
) -> Outcome {
    let reliable = !matches!(transport, SipTransport::Udp(_));
    let done = |status: u16| match kind {
        Kind::Invite => status >= 100,
        Kind::NonInvite => status >= 200,
    };
    // Retransmissions are scheduled from when the request went out, not
    // from when the last one did, so the timers do not drift.
    let deadline = tokio::time::Instant::now() + deadline;
    let mut interval = t1;
    let mut next_send = tokio::time::Instant::now() + interval;
    loop {
        let status = *seen.borrow_and_update();
        if done(status) {
            return Outcome::Answered(status);
        }
        if tokio::time::Instant::now() >= deadline {
            return Outcome::TimedOut;
        }
        let wake = if reliable { deadline } else { next_send.min(deadline) };
        tokio::select! {
            changed = seen.changed() => {
                if changed.is_err() {
                    // The registry entry went away: nothing can answer now.
                    return Outcome::TimedOut;
                }
                // A non-INVITE in Proceeding is resent at T2 from here on
                // (RFC 3261 §17.1.2.2).
                if kind == Kind::NonInvite && *seen.borrow() >= 100 && interval < T2 {
                    interval = T2;
                    next_send = tokio::time::Instant::now() + T2;
                }
                continue;
            }
            _ = tokio::time::sleep_until(wake) => {}
        }
        if reliable || tokio::time::Instant::now() >= deadline {
            continue;
        }
        log::debug!("Retransmitting {}", msg.split_whitespace().next().unwrap_or("request"));
        if let Err(e) = transport.send_to(msg.as_bytes(), dest).await {
            log::warn!("Could not retransmit a request: {}", e);
        }
        interval = match kind {
            // Timer A doubles without a cap; Timer B ends it at 64*T1.
            Kind::Invite => interval * 2,
            Kind::NonInvite => (interval * 2).min(T2),
        };
        next_send += interval;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    fn request(branch: &str, method: &str) -> String {
        format!(
            "{method} sip:x@127.0.0.1 SIP/2.0\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch={branch}\r\nCSeq: 1 {method}\r\n\r\n"
        )
    }

    fn response(branch: &str, code: u16) -> String {
        format!("SIP/2.0 {code} X\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch={branch}\r\n\r\n")
    }

    async fn pair() -> (SipTransport, UdpSocket, SocketAddr) {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dest = peer.local_addr().unwrap();
        let (udp, _rx) = super::super::transport::UdpTransport::bind("127.0.0.1:0").await.unwrap();
        (SipTransport::Udp(udp), peer, dest)
    }

    fn timing(t1_ms: u64) -> Timing {
        Timing { t1: Duration::from_millis(t1_ms), timer_f: Duration::from_millis(t1_ms * 64), ..Timing::default() }
    }

    async fn count_received(peer: &UdpSocket, within: Duration) -> usize {
        let mut buf = [0u8; 2048];
        let mut n = 0;
        let end = tokio::time::Instant::now() + within;
        while let Ok(Ok(_)) = tokio::time::timeout_at(end, peer.recv_from(&mut buf)).await {
            n += 1;
        }
        n
    }

    #[tokio::test]
    async fn an_unanswered_non_invite_doubles_to_t2_then_times_out() {
        let (transport, peer, dest) = pair().await;
        let txs = Arc::new(Transactions::default());
        let mut t = timing(10);
        t.timer_f = Duration::from_millis(300);
        let handle = send_request(&txs, &transport, dest, request("z9hG4bKa", "BYE"), Kind::NonInvite, t, None)
            .await
            .unwrap();
        let sent = count_received(&peer, Duration::from_millis(400)).await;
        assert_eq!(handle.await.unwrap(), Outcome::TimedOut);
        // 0, 10, 30, 70, 150 (then 310 is past the 300 ms deadline).
        assert_eq!(sent, 5);
        assert!(!txs.in_flight("z9hG4bKa"));
    }

    #[tokio::test]
    async fn a_provisional_stops_an_invite_but_not_a_non_invite() {
        let (transport, peer, dest) = pair().await;
        let txs = Arc::new(Transactions::default());
        let invite = send_request(&txs, &transport, dest, request("z9hG4bKi", "INVITE"), Kind::Invite, timing(20), None)
            .await
            .unwrap();
        let bye = send_request(&txs, &transport, dest, request("z9hG4bKb", "BYE"), Kind::NonInvite, timing(20), None)
            .await
            .unwrap();
        txs.on_response(&response("z9hG4bKi", 100), 100);
        txs.on_response(&response("z9hG4bKb", 100), 100);
        assert_eq!(invite.await.unwrap(), Outcome::Answered(100));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!bye.is_finished(), "a 100 does not end a non-INVITE transaction");
        txs.on_response(&response("z9hG4bKb", 200), 200);
        assert_eq!(bye.await.unwrap(), Outcome::Answered(200));
        drop(peer);
    }

    #[tokio::test]
    async fn an_unanswered_invite_gives_up_at_timer_b() {
        let (transport, peer, dest) = pair().await;
        let txs = Arc::new(Transactions::default());
        let handle = send_request(&txs, &transport, dest, request("z9hG4bKt", "INVITE"), Kind::Invite, timing(5), None)
            .await
            .unwrap();
        // Timer B is 64*T1 = 320 ms: sent at 0, 5, 15, 35, 75, 155 and 315.
        let sent = count_received(&peer, Duration::from_millis(450)).await;
        assert_eq!(handle.await.unwrap(), Outcome::TimedOut);
        // The last is 5 ms before the deadline; a loaded machine may miss it.
        assert!((6..=7).contains(&sent), "sent {sent} times");
    }
}
