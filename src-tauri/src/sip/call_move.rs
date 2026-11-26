//! Moving a call between the user's devices (Aria PBX `X-Aria-Move`).
//!
//! The app asks the PBX to move a call here and is told a `move_id`; the PBX
//! then INVITEs this app with `X-Aria-Move: <move_id>`. Only an INVITE whose
//! id this app is waiting for is answered without ringing; any other carrying
//! the header rings normally, so a stranger cannot make the phone pick up.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

pub const HEADER: &str = "X-Aria-Move";
/// How long a requested move waits for its INVITE.
pub const PENDING_TTL: Duration = Duration::from_secs(30);
/// How long an INVITE carrying `X-Aria-Move` waits for the 202 of a move
/// request still in flight: the PBX may send the INVITE before the API's
/// answer reaches this app.
pub const REQUEST_GRACE: Duration = Duration::from_secs(2);

/// Moves this app started and is waiting for.
#[derive(Default)]
pub struct PendingMoves {
    waiting: Mutex<HashMap<String, Instant>>,
    /// Move requests posted and not yet answered.
    in_flight: AtomicUsize,
    changed: Notify,
}

/// What an incoming INVITE's `X-Aria-Move` header means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveInvite {
    /// No (usable) header: an ordinary call.
    None,
    /// A move this app asked for: answer at once, no ring.
    AutoAnswer(String),
    /// A move someone else asked for: ring, labelled as a move.
    Ring(String),
}

/// A move id is a UUID; anything else is not trusted as one.
fn clean(id: &str) -> Option<String> {
    let id = id.trim();
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .then(|| id.to_ascii_lowercase())
}

impl PendingMoves {
    /// Remember a move; false for an id that is not a plausible one.
    pub fn register(&self, move_id: &str, now: Instant) -> bool {
        let Some(id) = clean(move_id) else { return false };
        let mut w = self.waiting.lock().unwrap_or_else(|p| p.into_inner());
        w.retain(|_, at| now.duration_since(*at) < PENDING_TTL);
        w.insert(id, now);
        drop(w);
        self.changed.notify_waiters();
        true
    }

    /// A move request is about to be posted.
    pub fn request_started(&self) {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
    }

    /// A move request was answered (or failed): release any held INVITE.
    pub fn request_finished(&self) {
        let _ = self.in_flight.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        self.changed.notify_waiters();
    }

    fn holds(&self, id: &str) -> bool {
        self.waiting.lock().unwrap_or_else(|p| p.into_inner()).contains_key(id)
    }

    /// [`classify`](Self::classify), but an INVITE with a move header whose id
    /// is not known yet is held for up to [`REQUEST_GRACE`] while this app has
    /// a move request in flight, since the 202 that names the id may be behind it.
    pub async fn classify_waiting(&self, header: Option<&str>, grace: Duration) -> MoveInvite {
        if let Some(id) = header.and_then(clean) {
            let deadline = tokio::time::Instant::now() + grace;
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.holds(&id) || self.in_flight.load(Ordering::SeqCst) == 0 {
                    break;
                }
                if tokio::time::timeout_at(deadline, notified).await.is_err() {
                    break;
                }
            }
        }
        self.classify(header, Instant::now())
    }

    /// Classify an INVITE's header. A pending id is used up: the PBX sends
    /// one INVITE per move, and a retried id must ring.
    pub fn classify(&self, header: Option<&str>, now: Instant) -> MoveInvite {
        let Some(id) = header.and_then(clean) else { return MoveInvite::None };
        let mut w = self.waiting.lock().unwrap_or_else(|p| p.into_inner());
        match w.remove(&id) {
            Some(at) if now.duration_since(at) < PENDING_TTL => MoveInvite::AutoAnswer(id),
            _ => MoveInvite::Ring(id),
        }
    }
}

/// The app's one set of pending moves.
pub fn pending() -> &'static PendingMoves {
    static P: OnceLock<PendingMoves> = OnceLock::new();
    P.get_or_init(PendingMoves::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "6f1c1d3e-0a4b-4c55-9f0e-2d3b8a7c9e10";

    #[test]
    fn a_move_this_app_started_is_auto_answered() {
        let p = PendingMoves::default();
        let t = Instant::now();
        assert!(p.register(ID, t));
        assert_eq!(p.classify(Some(ID), t + Duration::from_secs(2)), MoveInvite::AutoAnswer(ID.into()));
    }

    #[test]
    fn a_move_someone_else_started_rings() {
        let p = PendingMoves::default();
        assert_eq!(p.classify(Some(ID), Instant::now()), MoveInvite::Ring(ID.into()));
    }

    #[test]
    fn a_different_id_does_not_match() {
        let p = PendingMoves::default();
        let t = Instant::now();
        p.register(ID, t);
        assert!(matches!(p.classify(Some("other-id"), t), MoveInvite::Ring(_)));
    }

    #[test]
    fn a_move_expires_after_thirty_seconds() {
        let p = PendingMoves::default();
        let t = Instant::now();
        p.register(ID, t);
        assert_eq!(p.classify(Some(ID), t + PENDING_TTL), MoveInvite::Ring(ID.into()));
    }

    #[test]
    fn an_id_answers_once() {
        let p = PendingMoves::default();
        let t = Instant::now();
        p.register(ID, t);
        assert!(matches!(p.classify(Some(ID), t), MoveInvite::AutoAnswer(_)));
        assert!(matches!(p.classify(Some(ID), t), MoveInvite::Ring(_)));
    }

    #[test]
    fn no_header_or_junk_is_an_ordinary_call() {
        let p = PendingMoves::default();
        assert_eq!(p.classify(None, Instant::now()), MoveInvite::None);
        assert_eq!(p.classify(Some("  "), Instant::now()), MoveInvite::None);
        assert_eq!(p.classify(Some("a b;c"), Instant::now()), MoveInvite::None);
        assert!(!p.register("a b", Instant::now()));
    }

    #[tokio::test]
    async fn an_invite_ahead_of_the_202_waits_for_it() {
        let p = std::sync::Arc::new(PendingMoves::default());
        p.request_started();
        let q = p.clone();
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            q.register(ID, Instant::now());
            q.request_finished();
        });
        let got = p.classify_waiting(Some(ID), REQUEST_GRACE).await;
        late.await.unwrap();
        assert_eq!(got, MoveInvite::AutoAnswer(ID.into()));
    }

    #[tokio::test]
    async fn an_invite_with_no_request_in_flight_is_not_held() {
        let p = PendingMoves::default();
        let t = std::time::Instant::now();
        assert_eq!(p.classify_waiting(Some(ID), REQUEST_GRACE).await, MoveInvite::Ring(ID.into()));
        assert!(t.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn an_invite_waits_no_longer_than_the_grace_then_rings() {
        let p = PendingMoves::default();
        p.request_started();
        let t = std::time::Instant::now();
        assert_eq!(p.classify_waiting(Some(ID), Duration::from_millis(200)).await, MoveInvite::Ring(ID.into()));
        assert!(t.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn a_failed_request_releases_the_invite_to_ring() {
        let p = std::sync::Arc::new(PendingMoves::default());
        p.request_started();
        let q = p.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            q.request_finished();
        });
        assert_eq!(p.classify_waiting(Some(ID), REQUEST_GRACE).await, MoveInvite::Ring(ID.into()));
    }

    #[tokio::test]
    async fn an_ordinary_invite_is_never_held() {
        let p = PendingMoves::default();
        p.request_started();
        assert_eq!(p.classify_waiting(None, REQUEST_GRACE).await, MoveInvite::None);
    }

    #[test]
    fn ids_compare_without_case_or_padding() {
        let p = PendingMoves::default();
        let t = Instant::now();
        p.register(&ID.to_uppercase(), t);
        assert!(matches!(p.classify(Some(&format!(" {ID} ")), t), MoveInvite::AutoAnswer(_)));
    }
}
