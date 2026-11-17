//! Locating and failing over between SIP servers (RFC 3263, RFC 2782).
//!
//! Three layers, none of which needs a network to be tested:
//!
//! * [`DnsResolver`] is the seam to DNS. [`SystemResolver`] is the real one
//!   (hickory, the system's resolver configuration); tests use a fake.
//! * [`resolve`] turns a host (and an optional port) into an ordered list of
//!   [`Target`]s: SRV when there is no port, A/AAAA when there is one, SRV
//!   priority respected and RFC 2782 weighted-random selection inside a
//!   priority.
//! * [`Failover`] is the state machine that walks that list: which target to
//!   try next, how long to hold one down, when to back off after the whole list
//!   has failed, and when a better target is worth probing again.
//!
//! The state machine takes the time as an argument and never sleeps, so its
//! behaviour over minutes is asserted in microseconds.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::transport::TransportType;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How long one TCP or TLS connect to a single target may take before the next
/// target is tried.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);

/// How long a REGISTER may go unanswered before the target is given up on
/// (Timer F is 32 s; waiting that long per target makes failover unusable).
pub const REGISTER_FAILOVER_TIMEOUT: Duration = Duration::from_secs(8);

/// Waits between rounds once every target has failed.
const BACKOFF_SECS: &[u64] = &[1, 2, 4, 8, 16, 30];

/// How long a target that just failed is not worth probing for a failback.
const HOLD_DOWN: Duration = Duration::from_secs(30);

/// The longest `Retry-After` that is honoured.
/// The longest any wait is: a `Retry-After` or a backoff. A phone that can be
/// made to sit idle for ten minutes is a phone that cannot be reached.
pub const MAX_WAIT: Duration = Duration::from_secs(30);

const MIN_TTL: Duration = Duration::from_secs(30);
const MAX_TTL: Duration = Duration::from_secs(300);

/// SRV targets and addresses kept per name. A name's owner chooses how many it
/// publishes.
const MAX_TARGETS: usize = 16;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const RESOLVE_DEADLINE: Duration = Duration::from_secs(6);

// ---------------------------------------------------------------------------
// DNS seam
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrvRecord {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct SrvAnswer {
    pub records: Vec<SrvRecord>,
    pub ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct IpAnswer {
    pub ips: Vec<IpAddr>,
    pub ttl: Duration,
}

pub trait DnsResolver: Send + Sync {
    /// SRV records for `name` (`_sip._udp.example.com`). No records is `Ok` and
    /// empty; only a failure to ask is an error.
    fn srv<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<SrvAnswer, String>>;
    /// A and AAAA records for `host`.
    fn ips<'a>(&'a self, host: &'a str) -> BoxFut<'a, Result<IpAnswer, String>>;
}

/// SRV through hickory on the system's resolver configuration, addresses
/// through the OS. Built on first use, so the manager can be constructed with
/// no Tokio runtime.
#[derive(Default)]
pub struct SystemResolver {
    inner: tokio::sync::OnceCell<Option<Arc<hickory_resolver::TokioResolver>>>,
}

impl SystemResolver {
    /// The system's own resolver configuration, or an error. There is no
    /// fallback to a public resolver: a name that only a corporate or VPN
    /// resolver knows must not be sent to one that does not.
    async fn resolver(&self) -> Result<Arc<hickory_resolver::TokioResolver>, String> {
        use hickory_resolver::{ResolverBuilder, TokioResolver};

        self.inner
            .get_or_init(|| async {
                TokioResolver::builder_tokio()
                    .and_then(ResolverBuilder::build)
                    .map(Arc::new)
                    .map_err(|e| log::error!("no usable system DNS configuration for SRV: {e}"))
                    .ok()
            })
            .await
            .clone()
            .ok_or_else(|| "no usable system DNS configuration for SRV lookups".to_string())
    }
}

fn ttl_of(valid_until: Instant) -> Duration {
    valid_until
        .checked_duration_since(Instant::now())
        .unwrap_or(Duration::ZERO)
}

impl DnsResolver for SystemResolver {
    fn srv<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<SrvAnswer, String>> {
        Box::pin(async move {
            let resolver = self.resolver().await?;
            let lookup = match tokio::time::timeout(LOOKUP_TIMEOUT, resolver.srv_lookup(name)).await
            {
                Err(_) => return Err(format!("SRV lookup for {name} timed out")),
                // NXDOMAIN and "no data" are an answer, not a failure.
                Ok(Err(e)) if e.is_no_records_found() => {
                    return Ok(SrvAnswer {
                        records: Vec::new(),
                        ttl: MIN_TTL,
                    });
                }
                Ok(Err(e)) => return Err(format!("SRV lookup for {name} failed: {e}")),
                Ok(Ok(l)) => l,
            };
            let records = lookup
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    hickory_resolver::proto::rr::RData::SRV(s) => Some(SrvRecord {
                        priority: s.priority,
                        weight: s.weight,
                        port: s.port,
                        target: s.target.to_utf8().trim_end_matches('.').to_string(),
                    }),
                    _ => None,
                })
                .collect();
            Ok(SrvAnswer {
                records,
                ttl: ttl_of(lookup.valid_until()),
            })
        })
    }

    /// A and AAAA through the operating system's resolver (`getaddrinfo`), so
    /// split-DNS, VPN and hosts-file configuration apply.
    fn ips<'a>(&'a self, host: &'a str) -> BoxFut<'a, Result<IpAnswer, String>> {
        Box::pin(async move {
            match tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, 0))).await {
                Err(_) => Err(format!("DNS lookup timed out for {host}")),
                Ok(Err(e)) => Err(format!("DNS resolve failed for {host}: {e}")),
                Ok(Ok(addrs)) => Ok(IpAnswer {
                    ips: addrs.map(|a| a.ip()).collect(),
                    // The OS does not report one.
                    ttl: MIN_TTL,
                }),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------

/// One address a SIP server can be reached at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The name the address came from: the SRV target, or the host itself.
    pub host: String,
    pub addr: SocketAddr,
    /// Never `Auto`: the transport this target is reached over.
    pub transport: TransportType,
    pub priority: u16,
    pub weight: u16,
    /// Position of the target's group: (transport preference, SRV priority).
    /// Lower is better. A failback only ever moves to a lower rank.
    rank: (u8, u16),
}

pub type TargetKey = (SocketAddr, &'static str);

impl Target {
    pub fn key(&self) -> TargetKey {
        (self.addr, self.transport.param_static())
    }
}

impl TransportType {
    /// `param`, but `auto` for `Auto`: the configured choice, not a wire value.
    pub fn param_static_or_auto(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            other => other.param_static(),
        }
    }

    /// `param`, with a `'static` lifetime, for use in map keys.
    pub fn param_static(&self) -> &'static str {
        match self {
            Self::Udp | Self::Auto => "udp",
            Self::Tcp => "tcp",
            Self::Tls => "tls",
        }
    }
}

/// What a lookup produced.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub targets: Vec<Target>,
    /// The targets came from SRV. A TLS certificate is then checked against
    /// the name that was looked up, not the target's host (RFC 5922).
    pub via_srv: bool,
    /// The name that was looked up.
    pub lookup_host: String,
    pub ttl: Duration,
}

/// The SRV names to ask, in preference order, with the transport each is for.
pub fn srv_names(host: &str, transport: &TransportType) -> Vec<(String, TransportType)> {
    let tls = (format!("_sips._tcp.{host}"), TransportType::Tls);
    let tcp = (format!("_sip._tcp.{host}"), TransportType::Tcp);
    let udp = (format!("_sip._udp.{host}"), TransportType::Udp);
    match transport {
        TransportType::Tls => vec![tls],
        TransportType::Tcp => vec![tcp],
        TransportType::Udp => vec![udp],
        TransportType::Auto => vec![tls, tcp, udp],
    }
}

fn fallback_transport(t: &TransportType) -> TransportType {
    match t {
        // RFC 3263 §4.1: with no other information a SIP URI is UDP.
        TransportType::Auto => TransportType::Udp,
        other => other.clone(),
    }
}

/// RFC 2782 ordering: ascending priority; within one priority, repeated
/// weighted-random selection. `rng(n)` returns a number in `0..n`.
pub fn order_records(
    mut records: Vec<SrvRecord>,
    rng: &mut dyn FnMut(u32) -> u32,
) -> Vec<SrvRecord> {
    records.sort_by_key(|r| r.priority); // stable: keeps the answer's order among equals
    let mut out = Vec::with_capacity(records.len());
    let mut i = 0;
    while i < records.len() {
        let priority = records[i].priority;
        let mut j = i;
        while j < records.len() && records[j].priority == priority {
            j += 1;
        }
        let mut group: Vec<SrvRecord> = records[i..j].to_vec();
        // Zero-weight records first, so they are chosen only when the draw is 0
        // (RFC 2782: "have a very small chance of being selected").
        group.sort_by_key(|r| r.weight != 0);
        while !group.is_empty() {
            let total: u32 = group.iter().map(|r| u32::from(r.weight)).sum();
            let draw = rng(total + 1).min(total);
            let mut running = 0u32;
            let mut chosen = group.len() - 1;
            for (k, r) in group.iter().enumerate() {
                running += u32::from(r.weight);
                if running >= draw {
                    chosen = k;
                    break;
                }
            }
            out.push(group.remove(chosen));
        }
        i = j;
    }
    out
}

/// IPv4 first: the UDP transport binds an IPv4 socket, and a dual-stack host
/// lists AAAA first.
fn v4_first(mut ips: Vec<IpAddr>) -> Vec<IpAddr> {
    ips.sort_by_key(|ip| !ip.is_ipv4());
    ips
}

pub fn default_port(t: &TransportType) -> u16 {
    match t {
        TransportType::Tls => 5061,
        _ => 5060,
    }
}

fn clamp_ttl(ttl: Duration) -> Duration {
    ttl.clamp(MIN_TTL, MAX_TTL)
}

/// Locate `host`.
///
/// `port` set (non-zero) means the target is fully specified: A/AAAA at that
/// port, no SRV (RFC 3263 §4.2). Otherwise SRV for the transport (all three, in
/// the order TLS, TCP, UDP, for `Auto`), falling back to A/AAAA at the
/// transport's default port when the name publishes none.
pub async fn resolve(
    dns: &dyn DnsResolver,
    host: &str,
    port: Option<u16>,
    transport: &TransportType,
    rng: &mut (dyn FnMut(u32) -> u32 + Send),
) -> Result<Resolved, String> {
    let port = port.filter(|p| *p != 0);
    let fb = fallback_transport(transport);
    let one = |ip: IpAddr, host: &str, port: u16| Target {
        host: host.to_string(),
        addr: SocketAddr::new(ip, port),
        transport: fb.clone(),
        priority: 0,
        weight: 0,
        rank: (0, 0),
    };

    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(Resolved {
            targets: vec![one(ip, host, port.unwrap_or_else(|| default_port(&fb)))],
            via_srv: false,
            lookup_host: host.to_string(),
            ttl: MAX_TTL,
        });
    }

    let deadline = Instant::now() + RESOLVE_DEADLINE;
    let mut ttl = MAX_TTL;
    let mut targets: Vec<Target> = Vec::new();

    if port.is_none() {
        for (pref, (name, t)) in srv_names(host, transport).into_iter().enumerate() {
            let answer = match dns.srv(&name).await {
                Ok(a) => a,
                Err(e) => {
                    log::debug!("SRV {name}: {e}");
                    continue;
                }
            };
            if answer.records.is_empty() {
                continue;
            }
            ttl = ttl.min(answer.ttl);
            let mut records = answer.records;
            // A target of "." means the service is decidedly not available.
            records.retain(|r| !r.target.is_empty() && r.target != ".");
            if records.is_empty() {
                continue;
            }
            // `auto` never falls from TLS to cleartext: once the domain
            // publishes a _sips service, TCP and UDP are not even asked about.
            let sips_published =
                t == TransportType::Tls && matches!(transport, TransportType::Auto);
            for r in order_records(records, rng) {
                if targets.len() >= MAX_TARGETS || Instant::now() >= deadline {
                    break;
                }
                let ips = match dns.ips(&r.target).await {
                    Ok(a) => {
                        ttl = ttl.min(a.ttl);
                        a.ips
                    }
                    Err(e) => {
                        log::debug!("SRV target {}: {e}", r.target);
                        continue;
                    }
                };
                for ip in v4_first(ips) {
                    if targets.len() >= MAX_TARGETS {
                        break;
                    }
                    targets.push(Target {
                        host: r.target.clone(),
                        addr: SocketAddr::new(ip, r.port),
                        transport: t.clone(),
                        priority: r.priority,
                        weight: r.weight,
                        rank: (u8::try_from(pref).unwrap_or(u8::MAX), r.priority),
                    });
                }
            }
            if sips_published {
                if targets.is_empty() {
                    return Err(format!(
                        "{host} publishes a TLS (_sips) service but none of its targets resolve; \
                         not falling back to an unencrypted transport"
                    ));
                }
                break;
            }
        }
    }

    if !targets.is_empty() {
        return Ok(Resolved {
            targets,
            via_srv: true,
            lookup_host: host.to_string(),
            ttl: clamp_ttl(ttl),
        });
    }

    let answer = dns.ips(host).await?;
    let p = port.unwrap_or_else(|| default_port(&fb));
    let targets: Vec<Target> = v4_first(answer.ips)
        .into_iter()
        .take(MAX_TARGETS)
        .map(|ip| one(ip, host, p))
        .collect();
    if targets.is_empty() {
        return Err(format!("No address found for {host}"));
    }
    Ok(Resolved {
        targets,
        via_srv: false,
        lookup_host: host.to_string(),
        ttl: clamp_ttl(ttl.min(answer.ttl)),
    })
}

/// A random draw for [`order_records`].
pub fn random_draw(n: u32) -> u32 {
    rand::random::<u32>() % n.max(1)
}

// ---------------------------------------------------------------------------
// Failover state machine
// ---------------------------------------------------------------------------

/// Why a target was given up on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The connection could not be made.
    Connect,
    /// A request got no response, or keepalives went unanswered.
    Timeout,
    /// A final response that means "not this server": 408, 5xx.
    Status {
        code: u16,
        retry_after: Option<Duration>,
    },
}

impl Failure {
    /// Whether a REGISTER's final response means moving to another target.
    pub fn is_failover_status(code: u16) -> bool {
        code == 408 || (500..=599).contains(&code)
    }
}

/// What the caller does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Try(Target),
    /// Every target has failed this round. Wait, then ask again.
    Wait(Duration),
}

#[derive(Debug, Clone, Default)]
pub struct Failover {
    /// What the list is for (host, port, transport as configured); a list
    /// built for another server is discarded.
    pub key: String,
    targets: Vec<Target>,
    via_srv: bool,
    lookup_host: String,
    expires: Option<Instant>,
    /// The target that last registered.
    current: Option<TargetKey>,
    /// Targets already offered in this round.
    tried: HashSet<TargetKey>,
    /// A server's own instruction (`Retry-After`): not before this.
    retry_until: HashMap<TargetKey, Instant>,
    /// Failed recently: not worth probing for a failback yet.
    held_down: HashMap<TargetKey, Instant>,
    round: usize,
}

impl Failover {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub fn needs_resolve(&self, now: Instant) -> bool {
        self.targets.is_empty() || self.expires.is_none_or(|e| now >= e)
    }

    pub fn via_srv(&self) -> bool {
        self.via_srv
    }

    pub fn lookup_host(&self) -> &str {
        &self.lookup_host
    }

    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    pub fn current(&self) -> Option<&Target> {
        let key = self.current?;
        self.targets.iter().find(|t| t.key() == key)
    }

    /// Replace the list with a fresh answer, keeping what is still true about
    /// the targets that survive.
    pub fn install(&mut self, resolved: Resolved, now: Instant) {
        self.targets = resolved.targets;
        self.via_srv = resolved.via_srv;
        self.lookup_host = resolved.lookup_host;
        self.expires = Some(now + resolved.ttl);
        let alive: HashSet<TargetKey> = self.targets.iter().map(Target::key).collect();
        self.tried.retain(|k| alive.contains(k));
        self.retry_until.retain(|k, _| alive.contains(k));
        self.held_down.retain(|k, _| alive.contains(k));
        if self.current.is_some_and(|k| !alive.contains(&k)) {
            self.current = None;
        }
    }

    fn blocked(&self, key: &TargetKey, now: Instant) -> bool {
        self.retry_until.get(key).is_some_and(|until| *until > now)
    }

    /// The order a round walks the list in: the target that worked first, then
    /// the rest by priority.
    fn round_order(&self) -> Vec<&Target> {
        let mut order: Vec<&Target> = Vec::with_capacity(self.targets.len());
        if let Some(cur) = self.current() {
            order.push(cur);
        }
        order.extend(
            self.targets
                .iter()
                .filter(|t| Some(t.key()) != self.current),
        );
        order
    }

    /// Whether this round still has a target to offer right now.
    pub fn has_alternative(&self, now: Instant) -> bool {
        self.round_order()
            .iter()
            .any(|t| !self.tried.contains(&t.key()) && !self.blocked(&t.key(), now))
    }

    /// The next target to try, or how long to wait before the list is walked
    /// again. Offering a target marks it tried for this round.
    pub fn next_action(&mut self, now: Instant) -> Action {
        let pick = self
            .round_order()
            .into_iter()
            .find(|t| !self.tried.contains(&t.key()) && !self.blocked(&t.key(), now))
            .cloned();
        if let Some(t) = pick {
            self.tried.insert(t.key());
            return Action::Try(t);
        }
        // The list is exhausted: back off, and no sooner than the earliest
        // `Retry-After` a server gave.
        let backoff = Duration::from_secs(BACKOFF_SECS[self.round.min(BACKOFF_SECS.len() - 1)]);
        let earliest = self
            .targets
            .iter()
            .map(|t| {
                self.retry_until
                    .get(&t.key())
                    .map_or(Duration::ZERO, |u| u.saturating_duration_since(now))
            })
            .min()
            .unwrap_or(Duration::ZERO);
        self.round += 1;
        self.tried.clear();
        Action::Wait(backoff.max(earliest).min(MAX_WAIT))
    }

    pub fn report_failure(&mut self, target: &Target, failure: &Failure, now: Instant) {
        let key = target.key();
        self.held_down.insert(key, now + HOLD_DOWN);
        // A failed target has had its turn this round, however it got there.
        self.tried.insert(key);
        if let Failure::Status {
            retry_after: Some(ra),
            ..
        } = failure
        {
            self.retry_until.insert(key, now + (*ra).min(MAX_WAIT));
        }
        if self.current == Some(key) {
            self.current = None;
        }
    }

    pub fn report_success(&mut self, target: &Target) {
        let key = target.key();
        self.current = Some(key);
        self.round = 0;
        self.tried.clear();
        self.retry_until.remove(&key);
        self.held_down.remove(&key);
    }

    /// A fresh attempt the user asked for: start the list from the top and set
    /// aside any `Retry-After`, which was for an earlier, automatic attempt.
    pub fn restart(&mut self) {
        self.tried.clear();
        self.retry_until.clear();
        self.round = 0;
    }

    /// Make `target` the first one the next round tries.
    pub fn prefer(&mut self, target: &Target) {
        self.current = Some(target.key());
        self.tried.clear();
    }

    /// A target of better rank than the current one, no longer held down:
    /// worth probing, and moving back to if it answers.
    pub fn failback_candidate(&self, now: Instant) -> Option<Target> {
        let cur = self.current()?;
        self.targets
            .iter()
            .find(|t| {
                t.rank < cur.rank
                    && !self.blocked(&t.key(), now)
                    && self
                        .held_down
                        .get(&t.key())
                        .is_none_or(|until| *until <= now)
            })
            .cloned()
    }
}

/// How long to wait between CRLF keep-alive pings (RFC 5626 section 4.4.1):
/// somewhere between 80 and 100 percent of the registrar's Flow-Timer, or 25 s
/// when it did not give one. `jitter` is 0..=20, the percent taken off.
pub fn keepalive_interval(flow_timer: Option<u32>, jitter: u32) -> Duration {
    let Some(timer) = flow_timer.filter(|t| *t > 0) else {
        return Duration::from_secs(25);
    };
    let percent = 100 - u64::from(jitter.min(20));
    Duration::from_millis(u64::from(timer) * 10 * percent).max(Duration::from_secs(1))
}

/// `Retry-After` in seconds from a response's header value (`120`,
/// `120 (maintenance)`, `120;duration=3600`). HTTP-dates are not SIP's.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    let digits: String = value
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse::<u64>().ok().map(Duration::from_secs)
}

/// A resolver that answers from a table, for tests.
#[cfg(test)]
pub(crate) mod fake_dns {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct FakeDns {
        pub srv: HashMap<String, Vec<SrvRecord>>,
        pub ips: HashMap<String, Vec<IpAddr>>,
        pub asked: Mutex<Vec<String>>,
    }

    impl DnsResolver for FakeDns {
        fn srv<'a>(&'a self, name: &'a str) -> BoxFut<'a, Result<SrvAnswer, String>> {
            self.asked.lock().unwrap().push(format!("SRV {name}"));
            let records = self.srv.get(name).cloned().unwrap_or_default();
            Box::pin(async move {
                Ok(SrvAnswer {
                    records,
                    ttl: Duration::from_secs(60),
                })
            })
        }
        fn ips<'a>(&'a self, host: &'a str) -> BoxFut<'a, Result<IpAnswer, String>> {
            self.asked.lock().unwrap().push(format!("A {host}"));
            let ips = self.ips.get(host).cloned();
            Box::pin(async move {
                ips.map(|ips| IpAnswer {
                    ips,
                    ttl: Duration::from_secs(60),
                })
                .ok_or_else(|| "NXDOMAIN".to_string())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake_dns::FakeDns;
    use super::*;

    fn rec(priority: u16, weight: u16, port: u16, target: &str) -> SrvRecord {
        SrvRecord {
            priority,
            weight,
            port,
            target: target.into(),
        }
    }

    fn names(v: &[SrvRecord]) -> Vec<&str> {
        v.iter().map(|r| r.target.as_str()).collect()
    }

    // --- ordering ---------------------------------------------------------

    #[test]
    fn lower_priority_always_comes_first() {
        let mut zero = |_| 0;
        let out = order_records(
            vec![
                rec(20, 5, 1, "c"),
                rec(10, 5, 1, "a"),
                rec(30, 5, 1, "d"),
                rec(10, 5, 1, "b"),
            ],
            &mut zero,
        );
        let n = names(&out);
        assert_eq!(&n[2..], ["c", "d"]);
        assert!(n[..2].contains(&"a") && n[..2].contains(&"b"));
    }

    #[test]
    fn a_draw_picks_the_record_whose_running_sum_covers_it() {
        // weights 10 and 30 (zero-weight none): a draw of 10 lands on the
        // first, 11 on the second.
        let recs = vec![rec(1, 10, 1, "a"), rec(1, 30, 1, "b")];
        let mut ten = |_| 10;
        assert_eq!(names(&order_records(recs.clone(), &mut ten))[0], "a");
        let mut eleven = |_| 11;
        assert_eq!(names(&order_records(recs, &mut eleven))[0], "b");
    }

    #[test]
    fn zero_weight_is_chosen_only_on_a_zero_draw() {
        let recs = vec![rec(1, 0, 1, "z"), rec(1, 5, 1, "w")];
        let mut zero = |_| 0;
        assert_eq!(names(&order_records(recs.clone(), &mut zero))[0], "z");
        let mut one = |_| 1;
        assert_eq!(names(&order_records(recs, &mut one))[0], "w");
    }

    #[test]
    fn weights_shape_the_first_choice() {
        // A fixed-seed LCG: the proportions are reproducible.
        let mut state = 12345u64;
        let mut rng = move |n: u32| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            u32::try_from((state >> 33) % u64::from(n)).unwrap()
        };
        let recs = vec![rec(1, 75, 1, "heavy"), rec(1, 25, 1, "light")];
        let mut heavy = 0;
        for _ in 0..4000 {
            if order_records(recs.clone(), &mut rng)[0].target == "heavy" {
                heavy += 1;
            }
        }
        // Expect about 3000.
        assert!(
            (2800..3200).contains(&heavy),
            "heavy was first {heavy} of 4000"
        );
    }

    #[test]
    fn every_record_is_kept_exactly_once() {
        let recs: Vec<_> = (0..9)
            .map(|i| rec(i % 3, i * 7 % 5, 1, &format!("t{i}")))
            .collect();
        let mut rng = random_draw;
        let out = order_records(recs.clone(), &mut rng);
        assert_eq!(out.len(), recs.len());
        let mut a = names(&out);
        a.sort_unstable();
        let mut b = names(&recs);
        b.sort_unstable();
        assert_eq!(a, b);
        assert!(out.windows(2).all(|w| w[0].priority <= w[1].priority));
    }

    // --- resolution -------------------------------------------------------

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn dns() -> FakeDns {
        let mut d = FakeDns::default();
        d.srv.insert(
            "_sips._tcp.pbx.example".into(),
            vec![
                rec(20, 1, 5061, "b.pbx.example"),
                rec(10, 1, 5061, "a.pbx.example"),
            ],
        );
        d.srv.insert(
            "_sip._udp.pbx.example".into(),
            vec![rec(0, 1, 5060, "u.pbx.example")],
        );
        d.ips.insert(
            "a.pbx.example".into(),
            vec![ip("2001:db8::1"), ip("192.0.2.1")],
        );
        d.ips.insert("b.pbx.example".into(), vec![ip("192.0.2.2")]);
        d.ips.insert("u.pbx.example".into(), vec![ip("192.0.2.3")]);
        d.ips.insert("plain.example".into(), vec![ip("192.0.2.9")]);
        d
    }

    #[tokio::test]
    async fn no_port_asks_srv_for_the_configured_transport_only() {
        let d = dns();
        let r = resolve(
            &d,
            "pbx.example",
            None,
            &TransportType::Tls,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert!(r.via_srv);
        let hosts: Vec<_> = r.targets.iter().map(|t| t.addr.to_string()).collect();
        // Priority 10 before 20, and a host's IPv4 before its IPv6.
        assert_eq!(
            hosts,
            ["192.0.2.1:5061", "[2001:db8::1]:5061", "192.0.2.2:5061"]
        );
        assert!(r.targets.iter().all(|t| t.transport == TransportType::Tls));
        let asked = d.asked.lock().unwrap();
        assert_eq!(asked[0], "SRV _sips._tcp.pbx.example");
        assert!(!asked.iter().any(|q| q.contains("_sip._udp")));
    }

    #[tokio::test]
    async fn auto_prefers_tls_then_tcp_then_udp() {
        let mut d = dns();
        d.srv.insert(
            "_sip._tcp.legacy.example".into(),
            vec![rec(0, 1, 5060, "t.pbx.example")],
        );
        d.srv.insert(
            "_sip._udp.legacy.example".into(),
            vec![rec(0, 1, 5060, "u.pbx.example")],
        );
        d.ips.insert("t.pbx.example".into(), vec![ip("192.0.2.4")]);
        let r = resolve(
            &d,
            "legacy.example",
            None,
            &TransportType::Auto,
            &mut random_draw,
        )
        .await
        .unwrap();
        let ts: Vec<_> = r
            .targets
            .iter()
            .map(|t| t.transport.param_static())
            .collect();
        assert_eq!(ts, ["tcp", "udp"]);
        let asked = d.asked.lock().unwrap();
        assert_eq!(asked[0], "SRV _sips._tcp.legacy.example");
    }

    #[tokio::test]
    async fn auto_never_downgrades_from_tls_when_a_sips_service_exists() {
        let d = dns(); // pbx.example publishes _sips._tcp AND _sip._udp
        let r = resolve(
            &d,
            "pbx.example",
            None,
            &TransportType::Auto,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert!(r.targets.iter().all(|t| t.transport == TransportType::Tls));
        // The cleartext services are not even asked about.
        assert!(!d.asked.lock().unwrap().iter().any(|q| q.contains("_sip._")));

        // A _sips service whose targets do not resolve is a failure, not a
        // reason to try UDP.
        let mut broken = FakeDns::default();
        broken.srv.insert(
            "_sips._tcp.x.example".into(),
            vec![rec(0, 1, 5061, "gone.x.example")],
        );
        broken.srv.insert(
            "_sip._udp.x.example".into(),
            vec![rec(0, 1, 5060, "u.x.example")],
        );
        broken
            .ips
            .insert("u.x.example".into(), vec![ip("192.0.2.50")]);
        broken
            .ips
            .insert("x.example".into(), vec![ip("192.0.2.51")]);
        let err = resolve(
            &broken,
            "x.example",
            None,
            &TransportType::Auto,
            &mut random_draw,
        )
        .await
        .unwrap_err();
        assert!(err.contains("not falling back"), "{err}");
    }

    #[test]
    fn no_wait_is_longer_than_30_seconds() {
        let (mut f, now, t) = fixture(1);
        assert_eq!(f.next_action(now), Action::Try(t[0].clone()));
        f.report_failure(
            &t[0],
            &Failure::Status {
                code: 503,
                retry_after: Some(secs(600)),
            },
            now,
        );
        assert_eq!(f.next_action(now), Action::Wait(secs(30)));
        assert_eq!(f.next_action(now + secs(30)), Action::Try(t[0].clone()));
    }

    #[tokio::test]
    async fn an_explicit_port_skips_srv() {
        let d = dns();
        let r = resolve(
            &d,
            "pbx.example",
            Some(5080),
            &TransportType::Tls,
            &mut random_draw,
        )
        .await;
        // The name has SRV records but no address of its own.
        assert!(r.is_err());
        assert!(d
            .asked
            .lock()
            .unwrap()
            .iter()
            .all(|q| !q.starts_with("SRV")));

        let r = resolve(
            &d,
            "plain.example",
            Some(5080),
            &TransportType::Udp,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert!(!r.via_srv);
        assert_eq!(r.targets[0].addr.to_string(), "192.0.2.9:5080");
    }

    #[tokio::test]
    async fn port_zero_means_srv() {
        let d = dns();
        let r = resolve(
            &d,
            "pbx.example",
            Some(0),
            &TransportType::Tls,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert!(r.via_srv);
    }

    #[tokio::test]
    async fn a_name_without_srv_falls_back_to_the_default_port() {
        let d = dns();
        let r = resolve(
            &d,
            "plain.example",
            None,
            &TransportType::Tls,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert!(!r.via_srv);
        assert_eq!(r.targets[0].addr.to_string(), "192.0.2.9:5061");
        let r = resolve(
            &d,
            "plain.example",
            None,
            &TransportType::Auto,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert_eq!(
            (r.targets[0].addr.port(), &r.targets[0].transport),
            (5060, &TransportType::Udp)
        );
    }

    #[tokio::test]
    async fn a_literal_address_needs_no_dns() {
        let d = FakeDns::default();
        let r = resolve(
            &d,
            "203.0.113.5",
            None,
            &TransportType::Tcp,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert_eq!(r.targets[0].addr.to_string(), "203.0.113.5:5060");
        assert!(d.asked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_dot_target_is_skipped() {
        let mut d = dns();
        d.srv
            .insert("_sip._udp.off.example".into(), vec![rec(0, 0, 0, ".")]);
        d.ips.insert("off.example".into(), vec![ip("192.0.2.77")]);
        let r = resolve(
            &d,
            "off.example",
            None,
            &TransportType::Udp,
            &mut random_draw,
        )
        .await
        .unwrap();
        assert!(!r.via_srv);
        assert_eq!(r.targets[0].addr.to_string(), "192.0.2.77:5060");
    }

    // --- failover state machine ------------------------------------------

    fn fixture(n: u16) -> (Failover, Instant, Vec<Target>) {
        let now = Instant::now();
        let targets: Vec<Target> = (0..n)
            .map(|i| Target {
                host: format!("h{i}"),
                addr: SocketAddr::new(ip(&format!("192.0.2.{}", i + 1)), 5061),
                transport: TransportType::Tls,
                priority: i * 10,
                weight: 1,
                rank: (0, i * 10),
            })
            .collect();
        let mut f = Failover::default();
        f.install(
            Resolved {
                targets: targets.clone(),
                via_srv: true,
                lookup_host: "pbx.example".into(),
                ttl: Duration::from_secs(60),
            },
            now,
        );
        (f, now, targets)
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// A fake server farm: what each address does with a REGISTER.
    #[derive(Clone, Copy, Debug)]
    enum Behaviour {
        Up,
        Dead,
        Unavailable(Option<u64>),
        ConnectRefused,
    }

    struct Farm(HashMap<SocketAddr, Behaviour>);

    impl Farm {
        fn register(&self, t: &Target) -> Result<(), Failure> {
            match self.0[&t.addr] {
                Behaviour::Up => Ok(()),
                Behaviour::Dead => Err(Failure::Timeout),
                Behaviour::ConnectRefused => Err(Failure::Connect),
                Behaviour::Unavailable(ra) => Err(Failure::Status {
                    code: 503,
                    retry_after: ra.map(secs),
                }),
            }
        }
    }

    /// The production loop's shape: ask for an action, try it, report.
    /// Returns the target that registered and the virtual time it took.
    fn run(
        f: &mut Failover,
        farm: &Farm,
        mut now: Instant,
        give_up_after: Duration,
    ) -> Result<(Target, Instant), Instant> {
        let start = now;
        loop {
            match f.next_action(now) {
                Action::Try(t) => match farm.register(&t) {
                    Ok(()) => {
                        f.report_success(&t);
                        return Ok((t, now));
                    }
                    Err(why) => f.report_failure(&t, &why, now),
                },
                Action::Wait(d) => {
                    now += d;
                    if now - start > give_up_after {
                        return Err(now);
                    }
                }
            }
        }
    }

    #[test]
    fn a_timeout_moves_to_the_next_target() {
        let (mut f, now, t) = fixture(3);
        let farm = Farm(HashMap::from([
            (t[0].addr, Behaviour::Dead),
            (t[1].addr, Behaviour::Up),
            (t[2].addr, Behaviour::Up),
        ]));
        let (won, _) = run(&mut f, &farm, now, secs(60)).unwrap();
        assert_eq!(won, t[1]);
        assert_eq!(f.current(), Some(&t[1]));
    }

    #[test]
    fn connect_failure_moves_on_and_the_working_target_is_remembered() {
        let (mut f, now, t) = fixture(2);
        let farm = Farm(HashMap::from([
            (t[0].addr, Behaviour::ConnectRefused),
            (t[1].addr, Behaviour::Up),
        ]));
        run(&mut f, &farm, now, secs(60)).unwrap();
        // A later reconnect (the transport dropped; nothing wrong with the
        // server) starts at the target that worked, not at the top of the list.
        assert_eq!(f.next_action(now), Action::Try(t[1].clone()));
    }

    #[test]
    fn a_503_with_retry_after_is_honoured_when_it_is_the_only_target() {
        let (mut f, now, t) = fixture(1);
        let t0 = &t[0];
        assert_eq!(f.next_action(now), Action::Try(t0.clone()));
        f.report_failure(
            t0,
            &Failure::Status {
                code: 503,
                retry_after: Some(secs(90)),
            },
            now,
        );
        // Not the 1 s backoff: the server asked for 90, but no wait is longer
        // than 30 s.
        assert_eq!(f.next_action(now), Action::Wait(secs(30)));
        // After waiting, it is offered again.
        assert_eq!(f.next_action(now + secs(30)), Action::Try(t0.clone()));
    }

    #[test]
    fn a_503_moves_to_the_next_target_and_skips_the_blocked_one_next_round() {
        let (mut f, now, t) = fixture(2);
        assert_eq!(f.next_action(now), Action::Try(t[0].clone()));
        f.report_failure(
            &t[0],
            &Failure::Status {
                code: 503,
                retry_after: Some(secs(120)),
            },
            now,
        );
        assert_eq!(f.next_action(now), Action::Try(t[1].clone()));
        f.report_failure(&t[1], &Failure::Timeout, now);
        // Round over. The second target is free after the 1 s backoff, the
        // first is not: only the second is offered.
        assert_eq!(f.next_action(now), Action::Wait(secs(1)));
        assert_eq!(f.next_action(now + secs(1)), Action::Try(t[1].clone()));
        assert!(!f.has_alternative(now + secs(1)));
    }

    #[test]
    fn a_503_without_retry_after_is_just_a_failure_never_a_permanent_one() {
        let (mut f, mut now, t) = fixture(1);
        for _ in 0..50 {
            match f.next_action(now) {
                Action::Try(x) => f.report_failure(
                    &x,
                    &Failure::Status {
                        code: 503,
                        retry_after: None,
                    },
                    now,
                ),
                Action::Wait(d) => now += d,
            }
        }
        // Still being offered after fifty failures.
        let farm = Farm(HashMap::from([(t[0].addr, Behaviour::Up)]));
        assert!(run(&mut f, &farm, now, secs(120)).is_ok());
    }

    #[test]
    fn backoff_grows_to_a_ceiling_then_the_whole_list_is_retried() {
        let (mut f, mut now, t) = fixture(2);
        let mut waits = Vec::new();
        for _ in 0..8 {
            loop {
                match f.next_action(now) {
                    Action::Try(x) => f.report_failure(&x, &Failure::Timeout, now),
                    Action::Wait(d) => {
                        waits.push(d.as_secs());
                        now += d;
                        break;
                    }
                }
            }
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
        // Both targets are walked again after a wait.
        assert_eq!(f.next_action(now), Action::Try(t[0].clone()));
        assert_eq!(f.next_action(now), Action::Try(t[1].clone()));
    }

    #[test]
    fn success_resets_the_backoff() {
        let (mut f, mut now, t) = fixture(1);
        for _ in 0..3 {
            if let Action::Try(x) = f.next_action(now) {
                f.report_failure(&x, &Failure::Timeout, now);
            }
            if let Action::Wait(d) = f.next_action(now) {
                now += d;
            }
        }
        assert_eq!(f.next_action(now), Action::Try(t[0].clone()));
        f.report_success(&t[0]);
        // Three rounds of failures behind it, yet the next wait is the first
        // step again, not the fourth.
        f.report_failure(&t[0], &Failure::Timeout, now);
        assert_eq!(f.next_action(now), Action::Wait(secs(1)));
        assert_eq!(f.next_action(now), Action::Try(t[0].clone()));
    }

    #[test]
    fn failback_waits_out_the_hold_down_then_offers_the_better_target() {
        let (mut f, now, t) = fixture(2);
        let farm = Farm(HashMap::from([
            (t[0].addr, Behaviour::Dead),
            (t[1].addr, Behaviour::Up),
        ]));
        let (won, at) = run(&mut f, &farm, now, secs(60)).unwrap();
        assert_eq!(won, t[1]);
        // Just failed: not worth probing.
        assert_eq!(f.failback_candidate(at), None);
        assert_eq!(f.failback_candidate(at + secs(29)), None);
        // Held down no longer.
        assert_eq!(f.failback_candidate(at + secs(31)), Some(t[0].clone()));

        // The probe fails: held down again, the current target is left alone.
        f.report_failure(&t[0], &Failure::Timeout, at + secs(31));
        assert_eq!(f.failback_candidate(at + secs(40)), None);
        assert_eq!(f.current(), Some(&t[1]));

        // The probe answers: prefer it, and the next round starts there.
        assert_eq!(f.failback_candidate(at + secs(70)), Some(t[0].clone()));
        f.prefer(&t[0]);
        assert_eq!(f.next_action(at + secs(70)), Action::Try(t[0].clone()));
        f.report_success(&t[0]);
        assert_eq!(f.failback_candidate(at + secs(200)), None);
    }

    #[test]
    fn failback_respects_retry_after_and_never_targets_an_equal_rank() {
        let (mut f, now, t) = fixture(2);
        f.report_success(&t[1]);
        f.report_failure(
            &t[0],
            &Failure::Status {
                code: 503,
                retry_after: Some(secs(25)),
            },
            now,
        );
        assert_eq!(f.failback_candidate(now + secs(10)), None);
        assert_eq!(f.failback_candidate(now + secs(31)), Some(t[0].clone()));

        // Two targets of one priority are peers: no failback between them.
        let now = Instant::now();
        let mut peers = Failover::default();
        let mut a = t[0].clone();
        let mut b = t[1].clone();
        a.rank = (0, 10);
        b.rank = (0, 10);
        peers.install(
            Resolved {
                targets: vec![a, b.clone()],
                via_srv: true,
                lookup_host: "x".into(),
                ttl: secs(60),
            },
            now,
        );
        peers.report_success(&b);
        assert_eq!(peers.failback_candidate(now + secs(500)), None);
    }

    #[test]
    fn a_new_answer_keeps_what_is_still_true() {
        let (mut f, now, t) = fixture(2);
        f.report_success(&t[1]);
        f.report_failure(&t[0], &Failure::Timeout, now + secs(60));
        assert!(f.needs_resolve(now + secs(61)));
        assert!(!f.needs_resolve(now + secs(59)));
        // Same targets again: still current, still held down.
        f.install(
            Resolved {
                targets: t.clone(),
                via_srv: true,
                lookup_host: "pbx.example".into(),
                ttl: secs(60),
            },
            now + secs(61),
        );
        assert_eq!(f.current(), Some(&t[1]));
        assert_eq!(f.failback_candidate(now + secs(62)), None);
        // The current target disappears from DNS: nothing is "current" any more.
        f.install(
            Resolved {
                targets: vec![t[0].clone()],
                via_srv: true,
                lookup_host: "pbx.example".into(),
                ttl: secs(60),
            },
            now + secs(120),
        );
        assert_eq!(f.current(), None);
    }

    #[tokio::test]
    async fn the_loop_over_a_fake_resolver_fails_over_and_back() {
        let d = dns();
        let now = Instant::now();
        let mut f = Failover::default();
        assert!(f.needs_resolve(now));
        let r = resolve(
            &d,
            "pbx.example",
            None,
            &TransportType::Tls,
            &mut random_draw,
        )
        .await
        .unwrap();
        f.install(r, now);
        let t = f.targets().to_vec();
        assert_eq!(t.len(), 3);
        let mut farm = Farm(t.iter().map(|x| (x.addr, Behaviour::Up)).collect());
        // Both addresses of the priority-10 host are out; the priority-20 one
        // takes over.
        farm.0.insert(t[0].addr, Behaviour::Unavailable(Some(20)));
        farm.0.insert(t[1].addr, Behaviour::Unavailable(Some(20)));
        let (won, at) = run(&mut f, &farm, now, secs(60)).unwrap();
        assert_eq!(won, t[2]);
        // It comes back: after the Retry-After and the hold-down it is probed.
        farm.0.insert(t[0].addr, Behaviour::Up);
        assert_eq!(f.failback_candidate(at + secs(10)), None);
        assert_eq!(f.failback_candidate(at + secs(31)), Some(t[0].clone()));
    }

    #[test]
    fn keepalive_interval_follows_the_flow_timer_or_defaults_to_25_s() {
        assert_eq!(keepalive_interval(None, 0), secs(25));
        assert_eq!(keepalive_interval(Some(0), 7), secs(25));
        assert_eq!(keepalive_interval(Some(120), 0), secs(120));
        assert_eq!(keepalive_interval(Some(120), 20), secs(96));
        // Never more than the timer allows, never a busy loop.
        assert_eq!(keepalive_interval(Some(120), 99), secs(96));
        assert_eq!(keepalive_interval(Some(1), 20), secs(1));
    }

    #[test]
    fn retry_after_parsing() {
        assert_eq!(parse_retry_after("120"), Some(secs(120)));
        assert_eq!(parse_retry_after(" 5 (Maintenance)"), Some(secs(5)));
        assert_eq!(parse_retry_after("18000;duration=3600"), Some(secs(18000)));
        assert_eq!(parse_retry_after("Fri, 01 Jan 2027"), None);
    }

    #[test]
    fn failover_statuses() {
        for c in [408, 500, 502, 503, 504, 599] {
            assert!(Failure::is_failover_status(c), "{c}");
        }
        for c in [200, 401, 403, 404, 423, 480, 600] {
            assert!(!Failure::is_failover_status(c), "{c}");
        }
    }
}
