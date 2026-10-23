//! Consent for call control requested over MCP.
//!
//! An assistant can only place a call after the user has said yes to that
//! specific call in the app. The tool registers a request here, the frontend
//! shows a dialog and reports the decision through the `mcp_call_decision`
//! command, and the tool is released. A request nobody answers is denied
//! when its timeout runs out, so a tool call can never hang on a dialog the
//! user did not see.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::oneshot;

/// How long the consent dialog stays up before the request is denied.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(30);

/// What the user (or the timeout) decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    /// The id of the call the frontend placed, when allowed.
    pub call_id: Option<String>,
    /// Why it was not placed: "denied", "timeout", or the dial error.
    pub reason: Option<String>,
}

impl Decision {
    pub fn denied(reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            call_id: None,
            reason: Some(reason.into()),
        }
    }

    pub fn allowed(call_id: impl Into<String>) -> Self {
        Self {
            allowed: true,
            call_id: Some(call_id.into()),
            reason: None,
        }
    }
}

/// The call requests waiting for an answer.
#[derive(Default)]
pub struct ConsentRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
}

impl ConsentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open a request. Returns its id, to show to the user, and the receiver
    /// for `wait`.
    pub fn begin(&self) -> (String, oneshot::Receiver<Decision>) {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), tx);
        (id, rx)
    }

    /// Answer a request. Fails if the id is unknown, which includes a request
    /// that already timed out.
    pub fn decide(&self, request_id: &str, decision: Decision) -> Result<(), String> {
        let tx = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id)
            .ok_or_else(|| format!("No pending call request {request_id}"))?;
        tx.send(decision)
            .map_err(|_| "The call request is no longer waiting".to_string())
    }

    /// Wait for the answer to `request_id`, denying it after `timeout`.
    pub async fn wait(
        &self,
        request_id: &str,
        rx: oneshot::Receiver<Decision>,
        timeout: Duration,
    ) -> Decision {
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(decision)) => decision,
            // Sender dropped without an answer: treat like a denial.
            Ok(Err(_)) => {
                self.forget(request_id);
                Decision::denied("denied")
            }
            Err(_) => {
                self.forget(request_id);
                Decision::denied("timeout")
            }
        }
    }

    /// How many requests are waiting.
    #[cfg(test)]
    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn forget(&self, request_id: &str) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unanswered_request_is_denied_on_timeout() {
        let registry = ConsentRegistry::new();
        let (id, rx) = registry.begin();
        assert_eq!(registry.pending_count(), 1);

        let decision = registry.wait(&id, rx, Duration::from_millis(20)).await;
        assert_eq!(decision, Decision::denied("timeout"));
        // Nothing is left behind for a late answer to find.
        assert_eq!(registry.pending_count(), 0);
        assert!(registry.decide(&id, Decision::allowed("x")).is_err());
    }

    #[tokio::test]
    async fn answer_releases_the_waiter() {
        let registry = std::sync::Arc::new(ConsentRegistry::new());
        let (id, rx) = registry.begin();
        let r2 = registry.clone();
        let id2 = id.clone();
        tokio::spawn(async move {
            r2.decide(&id2, Decision::allowed("call-1")).unwrap();
        });
        let decision = registry.wait(&id, rx, Duration::from_secs(5)).await;
        assert_eq!(decision, Decision::allowed("call-1"));
        assert_eq!(registry.pending_count(), 0);
    }

    #[tokio::test]
    async fn denial_carries_its_reason() {
        let registry = ConsentRegistry::new();
        let (id, rx) = registry.begin();
        registry.decide(&id, Decision::denied("denied")).unwrap();
        let decision = registry.wait(&id, rx, Duration::from_secs(1)).await;
        assert!(!decision.allowed);
        assert_eq!(decision.reason.as_deref(), Some("denied"));
    }

    #[test]
    fn deciding_an_unknown_request_fails() {
        let registry = ConsentRegistry::new();
        assert!(registry.decide("nope", Decision::denied("denied")).is_err());
    }
}
