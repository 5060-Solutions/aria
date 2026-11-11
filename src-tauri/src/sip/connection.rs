//! What the status chip's panel shows: registration as a list of steps.
//!
//! The registration code fills one `ConnectionReport` per account in as it
//! goes (resolve, connect, REGISTER, authenticate, registered), so the UI can
//! show which step broke and the exact failure text instead of "Connection
//! error". Every change is pushed to the webview as a `sip-connection` event
//! and can be fetched with `get_connection_report`.

use std::time::Duration;

use serde::Serialize;

use super::account::AccountConfig;
use super::SipManager;

/// One stage of bringing an account up, in the order they happen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Resolve,
    Connect,
    Register,
    Authenticate,
    Registered,
}

impl StepKind {
    const ALL: [StepKind; 5] = [
        StepKind::Resolve,
        StepKind::Connect,
        StepKind::Register,
        StepKind::Authenticate,
        StepKind::Registered,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Ok,
    Failed,
    Running,
    Pending,
    Skipped,
}

/// Why a step failed, coarse enough for the UI to pick a hint from. The exact
/// text stays in `Step::detail`; this never replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Dns,
    DnsTimeout,
    Unreachable,
    TlsUntrusted,
    TlsName,
    TlsHandshake,
    Timeout,
    Auth,
    ForbiddenIp,
    ServerError,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub kind: StepKind,
    pub status: StepStatus,
    pub detail: Option<String>,
    pub elapsed_ms: Option<u32>,
    pub failure: Option<FailureKind>,
    /// When the step went `Running`, so a later `ok`/`failed` without an
    /// explicit duration can still report how long it took.
    #[serde(skip)]
    pub started: Option<tokio::time::Instant>,
}

impl Step {
    fn pending(kind: StepKind) -> Self {
        Self { kind, status: StepStatus::Pending, detail: None, elapsed_ms: None, failure: None, started: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Registering,
    Registered,
    Reconnecting,
    Error,
    Unregistered,
}

/// Where packets go: the outbound proxy when one is set, else the registrar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub via_proxy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredInfo {
    pub registered_at_ms: u64,
    pub expires_in_s: u32,
    pub last_refresh_ms: u64,
    pub public_addr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionReport {
    pub account_id: String,
    pub username: String,
    pub domain: String,
    /// `udp`, `tcp` or `tls`.
    pub transport: String,
    pub target: Target,
    /// Always the five `StepKind`s, in order.
    pub steps: Vec<Step>,
    pub outcome: Outcome,
    /// The same text the account's registration state carries.
    pub error: Option<String>,
    pub reconnect_attempt: u32,
    pub registered: Option<RegisteredInfo>,
    pub attempt_started_ms: u64,
    pub attempt_finished_ms: Option<u64>,
}

pub fn now_ms() -> u64 {
    super::diagnostics::now_millis()
}

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

impl ConnectionReport {
    /// A fresh attempt for `account`, every step pending.
    pub fn start(account: &AccountConfig) -> Self {
        let (host, port) = SipManager::target_server(account);
        let via_proxy = account
            .outbound_proxy
            .as_deref()
            .is_some_and(|p| !p.trim().is_empty());
        Self {
            account_id: account.id.clone(),
            username: account.username.clone(),
            domain: account.domain.clone(),
            transport: format!("{:?}", account.transport).to_lowercase(),
            target: Target { host: host.to_string(), port, via_proxy },
            steps: StepKind::ALL.iter().cloned().map(Step::pending).collect(),
            outcome: Outcome::Registering,
            error: None,
            reconnect_attempt: 0,
            registered: None,
            attempt_started_ms: now_ms(),
            attempt_finished_ms: None,
        }
    }

    /// Begin another attempt for the same account: steps back to pending,
    /// the old failure gone, identity kept.
    pub fn restart(&mut self) {
        self.steps = StepKind::ALL.iter().cloned().map(Step::pending).collect();
        self.outcome = Outcome::Registering;
        self.error = None;
        self.registered = None;
        self.attempt_started_ms = now_ms();
        self.attempt_finished_ms = None;
    }

    pub fn step(&self, kind: StepKind) -> &Step {
        self.steps.iter().find(|s| s.kind == kind).expect("every kind is present")
    }

    fn step_mut(&mut self, kind: StepKind) -> &mut Step {
        self.steps.iter_mut().find(|s| s.kind == kind).expect("every kind is present")
    }

    /// The elapsed time for a step: what the caller measured, else the time
    /// since the step went running.
    fn elapsed_for(step: &Step, elapsed: Option<Duration>) -> Option<u32> {
        elapsed.or_else(|| step.started.map(|t| t.elapsed())).map(millis)
    }

    pub fn running(&mut self, kind: StepKind) {
        let step = self.step_mut(kind);
        step.status = StepStatus::Running;
        step.detail = None;
        step.elapsed_ms = None;
        step.failure = None;
        step.started = Some(tokio::time::Instant::now());
    }

    pub fn ok(&mut self, kind: StepKind, detail: impl Into<String>, elapsed: Option<Duration>) {
        let step = self.step_mut(kind);
        step.elapsed_ms = Self::elapsed_for(step, elapsed);
        step.status = StepStatus::Ok;
        step.detail = Some(detail.into());
        step.failure = None;
    }

    pub fn skipped(&mut self, kind: StepKind, detail: impl Into<String>) {
        let step = self.step_mut(kind);
        step.status = StepStatus::Skipped;
        step.detail = Some(detail.into());
        step.elapsed_ms = None;
        step.failure = None;
    }

    /// The step broke. This is also the attempt's outcome: `error` carries the
    /// same text, so the chip and the panel never disagree.
    pub fn failed(
        &mut self,
        kind: StepKind,
        failure: FailureKind,
        detail: impl Into<String>,
        elapsed: Option<Duration>,
    ) {
        let detail = detail.into();
        let step = self.step_mut(kind);
        step.elapsed_ms = Self::elapsed_for(step, elapsed);
        step.status = StepStatus::Failed;
        step.detail = Some(detail.clone());
        step.failure = Some(failure);
        self.error = Some(detail);
        self.outcome = Outcome::Error;
        self.attempt_finished_ms = Some(now_ms());
    }

    pub fn finish(&mut self, outcome: Outcome) {
        self.outcome = outcome;
        self.attempt_finished_ms = Some(now_ms());
    }
}

/// Which hint fits a failed name lookup. The message is `resolve_server`'s.
pub fn classify_resolve(err: &str) -> FailureKind {
    if err.starts_with("DNS lookup timed out") {
        FailureKind::DnsTimeout
    } else {
        FailureKind::Dns
    }
}

/// Which hint fits a failed bind, connect or TLS handshake. The message is
/// the transport's; the TLS wording it keys on is produced by
/// `transport::describe_tls_error`.
pub fn classify_connect(transport: &super::transport::TransportType, err: &str) -> FailureKind {
    let _ = transport;
    if err.contains("certificate not trusted") {
        FailureKind::TlsUntrusted
    } else if err.contains("certificate name mismatch") {
        FailureKind::TlsName
    } else if err.starts_with("TLS handshake")
        || err.starts_with("TLS config")
        || err.starts_with("Invalid server name")
    {
        FailureKind::TlsHandshake
    } else {
        FailureKind::Unreachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sip::account::{default_codec_preferences, AccountConfig};
    use crate::sip::transport::TransportType;

    fn sample_account(domain: &str, proxy: Option<&str>) -> AccountConfig {
        AccountConfig {
            id: "acct-1".into(),
            display_name: "ghostbook".into(),
            username: "ghostbook".into(),
            domain: domain.into(),
            registrar: None,
            port: 5060,
            password: "secret".into(),
            transport: TransportType::Udp,
            outbound_proxy: proxy.map(str::to_owned),
            auth_username: None,
            auth_realm: None,
            enabled: true,
            auto_record: false,
            tls_insecure: false,
            tls_server_name: None,
            srtp_mode: Default::default(),
            codecs: default_codec_preferences(),
        }
    }

    #[test]
    fn a_new_report_has_five_pending_steps_in_order() {
        let r = ConnectionReport::start(&sample_account("sip.example.com", None));
        assert_eq!(
            r.steps.iter().map(|s| s.kind.clone()).collect::<Vec<_>>(),
            vec![
                StepKind::Resolve,
                StepKind::Connect,
                StepKind::Register,
                StepKind::Authenticate,
                StepKind::Registered
            ]
        );
        assert!(r.steps.iter().all(|s| s.status == StepStatus::Pending));
        assert_eq!(r.outcome, Outcome::Registering);
        assert_eq!(
            r.target,
            Target { host: "sip.example.com".into(), port: 5060, via_proxy: false }
        );
    }

    #[test]
    fn a_proxy_account_targets_the_proxy() {
        let r = ConnectionReport::start(&sample_account("pbx.lan", Some("203.0.113.9:5061")));
        assert_eq!(
            r.target,
            Target { host: "203.0.113.9".into(), port: 5061, via_proxy: true }
        );
    }

    #[test]
    fn a_failed_step_sets_the_error_and_outcome() {
        let mut r = ConnectionReport::start(&sample_account("sip.example.com", None));
        r.running(StepKind::Resolve);
        r.failed(
            StepKind::Resolve,
            FailureKind::Dns,
            "DNS resolve failed for sip.example.com:5060: no such host",
            None,
        );
        assert_eq!(r.step(StepKind::Resolve).status, StepStatus::Failed);
        assert_eq!(r.step(StepKind::Resolve).failure, Some(FailureKind::Dns));
        assert_eq!(
            r.error.as_deref(),
            Some("DNS resolve failed for sip.example.com:5060: no such host")
        );
        assert_eq!(r.outcome, Outcome::Error);
        assert!(r.attempt_finished_ms.is_some());
    }

    #[test]
    fn restart_keeps_identity_and_clears_the_attempt() {
        let mut r = ConnectionReport::start(&sample_account("sip.example.com", None));
        r.failed(
            StepKind::Connect,
            FailureKind::Unreachable,
            "TCP connect to 1.2.3.4:5060 failed: connection refused",
            None,
        );
        let started = r.attempt_started_ms;
        std::thread::sleep(std::time::Duration::from_millis(2));
        r.restart();
        assert!(r.steps.iter().all(|s| s.status == StepStatus::Pending));
        assert_eq!(r.error, None);
        assert!(r.attempt_started_ms > started);
        assert_eq!(r.username, "ghostbook");
    }

    #[test]
    fn the_report_serializes_in_camel_case_with_snake_case_enums() {
        let r = ConnectionReport::start(&sample_account("sip.example.com", None));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["accountId"], "acct-1");
        assert_eq!(v["steps"][0]["kind"], "resolve");
        assert_eq!(v["steps"][0]["status"], "pending");
        assert_eq!(v["target"]["viaProxy"], false);
    }

    #[test]
    fn resolve_failures_are_classified() {
        assert_eq!(classify_resolve("DNS lookup timed out for x:5060"), FailureKind::DnsTimeout);
        assert_eq!(classify_resolve("DNS resolve failed for x:5060: no such host"), FailureKind::Dns);
    }

    #[test]
    fn connect_failures_are_classified() {
        use TransportType::*;
        assert_eq!(
            classify_connect(&Tls, "TLS handshake with 1.2.3.4:5061 failed: certificate not trusted: invalid peer certificate: UnknownIssuer"),
            FailureKind::TlsUntrusted
        );
        assert_eq!(
            classify_connect(&Tls, "TLS handshake with 1.2.3.4:5061 failed: certificate name mismatch: NotValidForName"),
            FailureKind::TlsName
        );
        assert_eq!(classify_connect(&Tls, "TLS handshake with 1.2.3.4:5061 timed out"), FailureKind::TlsHandshake);
        assert_eq!(
            classify_connect(&Tls, "TLS/TCP connect to 1.2.3.4:5061 failed: connection refused"),
            FailureKind::Unreachable
        );
        assert_eq!(classify_connect(&Tcp, "TCP connect to 1.2.3.4:5060 timed out"), FailureKind::Unreachable);
    }
}
