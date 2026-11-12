/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Failed sign-ins counted and blocked per source address (plans/Comms-Framework.md §5.2 "Fraud", §4.3 XMPP), shared by
//! the SIP engine (failed digests, moqui-sip) and the XMPP server (failed SASL, moqui-xmpp). Scanners probe a listener
//! for weak credentials; `limit` failures from one address within `window` block that address for `block`, and
//! everything it sends is then dropped without an answer. The account a scanner names is never locked,
//! which would let it lock users out. Sans-IO: the caller passes the time.
//!
//! An IPv6 source counts as its /64 (one subscriber's network, which a scanner can rotate through freely), and an
//! IPv4-mapped one as its IPv4 address. The table is bounded: when it is full, sources not blocked go first, then the
//! blocks that end soonest, so rotating addresses cannot clear the blocks already made.
//!
//! [`RequestGate`] caps what a flood can make, whatever its credentials: new requests (outside a dialog) per source, as
//! a token bucket of [`RequestLimits::rate`] a second up to [`RequestLimits::burst`]; new requests while the endpoint
//! already runs [`RequestLimits::max_transactions`]; and stream connections (TCP, TLS) open at once. What is over a
//! limit is dropped unanswered, as a blocked source's is; requests in a dialog (a call's BYE, its re-INVITE) and
//! responses always pass, so the calls going on are not cut.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{Duration, Instant};

/// How many failures block a source, and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuardPolicy {
    pub limit: u32,
    pub window: Duration,
    pub block: Duration,
}

impl Default for GuardPolicy {
    fn default() -> Self {
        GuardPolicy {
            limit: 5,
            window: Duration::from_secs(600),
            block: Duration::from_secs(3600),
        }
    }
}

/// What a failure did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Counted; this many in the window so far.
    Counted(u32),
    /// The source is now blocked, for the first time in this block.
    Blocked,
    /// It was already blocked.
    StillBlocked,
}

struct Entry {
    count: u32,
    first: Instant,
    blocked_until: Option<Instant>,
}

/// The failures of each source address.
pub struct SourceGuard {
    policy: GuardPolicy,
    entries: HashMap<IpAddr, Entry>,
}

/// The most sources tracked; past this the ones not blocked go first, then the blocks that end soonest.
const MAX_SOURCES: usize = 50_000;

/// The key a source counts under: an IPv4 address (an IPv4-mapped IPv6 one as its IPv4 address), or an IPv6
/// address's /64. Every per-source limit of the listeners counts by it, so one host cannot spread over its /64.
pub fn source_key(source: IpAddr) -> IpAddr {
    key_of(source)
}

fn key_of(source: IpAddr) -> IpAddr {
    match source {
        IpAddr::V4(_) => source,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        },
    }
}

impl SourceGuard {
    pub fn new(policy: GuardPolicy) -> Self {
        SourceGuard {
            policy,
            entries: HashMap::new(),
        }
    }

    pub fn set_policy(&mut self, policy: GuardPolicy) {
        self.policy = policy;
    }

    /// Whether `source` is blocked at `now`.
    pub fn is_blocked(&mut self, source: IpAddr, now: Instant) -> bool {
        let source = key_of(source);
        match self.entries.get(&source).and_then(|e| e.blocked_until) {
            Some(until) if until > now => true,
            Some(_) => {
                self.entries.remove(&source);
                false
            }
            None => false,
        }
    }

    /// A failed sign-in (a digest, a SASL exchange) from `source` at `now`.
    pub fn failure(&mut self, source: IpAddr, now: Instant) -> Failure {
        let source = key_of(source);
        if self.is_blocked(source, now) {
            return Failure::StillBlocked;
        }
        if self.entries.len() >= MAX_SOURCES && !self.entries.contains_key(&source) {
            self.evict(now);
        }
        let policy = self.policy;
        let entry = self.entries.entry(source).or_insert(Entry {
            count: 0,
            first: now,
            blocked_until: None,
        });
        if now.duration_since(entry.first) > policy.window {
            entry.count = 0;
            entry.first = now;
        }
        entry.count += 1;
        if entry.count >= policy.limit.max(1) {
            entry.blocked_until = Some(now + policy.block);
            Failure::Blocked
        } else {
            Failure::Counted(entry.count)
        }
    }

    /// A request from `source` passed: its count starts over.
    pub fn success(&mut self, source: IpAddr) {
        let source = key_of(source);
        if self.entries.get(&source).is_some_and(|e| e.blocked_until.is_none()) {
            self.entries.remove(&source);
        }
    }

    /// A block known from before (the voice services keep them across restarts), or `None` to clear one.
    pub fn set_block(&mut self, source: IpAddr, until: Option<Instant>) {
        let source = key_of(source);
        match until {
            Some(until) => {
                if self.entries.len() >= MAX_SOURCES && !self.entries.contains_key(&source) {
                    self.evict(Instant::now());
                }
                let entry = self.entries.entry(source).or_insert(Entry {
                    count: 0,
                    first: until,
                    blocked_until: None,
                });
                entry.blocked_until = Some(until);
            }
            None => {
                self.entries.remove(&source);
            }
        }
    }

    fn evict(&mut self, now: Instant) {
        self.entries.retain(|_, e| e.blocked_until.is_some_and(|u| u > now));
        if self.entries.len() >= MAX_SOURCES {
            // every one is blocked: the tenth whose blocks end soonest go, the others stay blocked
            let mut ends: Vec<Instant> = self.entries.values().filter_map(|e| e.blocked_until).collect();
            let cut = MAX_SOURCES / 10;
            ends.select_nth_unstable(cut);
            let threshold = ends[cut];
            self.entries
                .retain(|_, e| e.blocked_until.is_some_and(|u| u > threshold));
        }
    }
}

/// The caps of [`RequestGate`], from the host's properties (`sip_request_rate`, `sip_request_burst`,
/// `sip_max_transactions`, `sip_max_connections`, `sip_max_connections_per_source`). 0 turns one off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RequestLimits {
    /// New requests a second from one source, refilled continuously.
    pub rate: f64,
    /// The most one source may send at once before the rate holds it.
    pub burst: u32,
    /// New requests are dropped while the endpoint runs this many transactions.
    pub max_transactions: usize,
    /// TCP and TLS connections open at once; one past it is closed.
    pub max_connections: usize,
    /// TCP and TLS connections open at once from one source (an IPv6 /64 is one), so one source cannot hold every
    /// connection the registrar's phones keep open; one past it is closed.
    pub max_connections_per_source: usize,
}

impl Default for RequestLimits {
    fn default() -> Self {
        RequestLimits {
            rate: 20.0,
            burst: 100,
            max_transactions: 2_000,
            max_connections: 500,
            max_connections_per_source: 100,
        }
    }
}

/// What the gate says of a new request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    Pass,
    /// Its source sent more than its rate.
    RateLimited,
    /// The endpoint runs its most transactions.
    Busy,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

/// The request and connection caps. Sans-IO: the caller passes the time and the running transactions.
pub struct RequestGate {
    limits: RequestLimits,
    buckets: HashMap<IpAddr, Bucket>,
    connections: usize,
    /// The connections open from each source, while it has any.
    by_source: HashMap<IpAddr, usize>,
}

impl RequestGate {
    pub fn new(limits: RequestLimits) -> Self {
        RequestGate {
            limits,
            buckets: HashMap::new(),
            connections: 0,
            by_source: HashMap::new(),
        }
    }

    pub fn limits(&self) -> RequestLimits {
        self.limits
    }

    /// A new request (outside a dialog) from `source` at `now`, with `running` transactions in the endpoint.
    pub fn admit(&mut self, source: IpAddr, running: usize, now: Instant) -> Admit {
        let limits = self.limits;
        if limits.rate > 0.0 && limits.burst > 0 {
            let key = key_of(source);
            if self.buckets.len() >= MAX_SOURCES && !self.buckets.contains_key(&key) {
                // the full buckets are idle sources; a flood from more sources than the table holds is left to the
                // transaction cap
                let full = f64::from(limits.burst);
                self.buckets
                    .retain(|_, b| b.tokens + now.duration_since(b.last).as_secs_f64() * limits.rate < full);
                if self.buckets.len() >= MAX_SOURCES {
                    self.buckets.clear();
                }
            }
            let bucket = self.buckets.entry(key).or_insert(Bucket {
                tokens: f64::from(limits.burst),
                last: now,
            });
            let refill = now.saturating_duration_since(bucket.last).as_secs_f64() * limits.rate;
            bucket.tokens = (bucket.tokens + refill).min(f64::from(limits.burst));
            bucket.last = now;
            if bucket.tokens < 1.0 {
                return Admit::RateLimited;
            }
            bucket.tokens -= 1.0;
        }
        if limits.max_transactions > 0 && running >= limits.max_transactions {
            return Admit::Busy;
        }
        Admit::Pass
    }

    /// A stream connection opened from `source` (when known): false when it is past a cap, overall or for its
    /// source, and must be closed (its close is still told to [`Self::connection_closed`]).
    pub fn connection_opened(&mut self, source: Option<IpAddr>) -> bool {
        self.connections += 1;
        let from_source = match source {
            Some(source) => {
                let count = self.by_source.entry(key_of(source)).or_insert(0);
                *count += 1;
                *count
            }
            None => 0,
        };
        let per_source = self.limits.max_connections_per_source;
        (self.limits.max_connections == 0 || self.connections <= self.limits.max_connections)
            && (per_source == 0 || from_source <= per_source)
    }

    pub fn connection_closed(&mut self, source: Option<IpAddr>) {
        self.connections = self.connections.saturating_sub(1);
        if let Some(source) = source {
            let key = key_of(source);
            if let Some(count) = self.by_source.get_mut(&key) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.by_source.remove(&key);
                }
            }
        }
    }
}

/// A listener's connection slots with a share per source (plans/archive/High-Level-Review.md HL-032): at most `total` held at
/// once, and at most `per_source` by one source ([`source_key`]), so one host cannot hold every slot of a pool that
/// connections wait in before they authenticate. Only sources holding a slot are in the table, so it is bounded by
/// `total` and nothing is scanned per call. Cheap to clone; the clones share the slots.
#[derive(Clone)]
pub struct SourceSlots {
    inner: std::sync::Arc<SlotsInner>,
}

struct SlotsInner {
    total: usize,
    per_source: usize,
    state: std::sync::Mutex<SlotsState>,
}

#[derive(Default)]
struct SlotsState {
    held: usize,
    by_source: HashMap<IpAddr, usize>,
}

/// Why [`SourceSlots::take`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRefused {
    /// Every slot is held.
    Full,
    /// The source holds its share.
    Source,
}

/// One held slot, given back when dropped.
pub struct SourceSlot {
    inner: std::sync::Arc<SlotsInner>,
    key: IpAddr,
}

impl SourceSlots {
    /// `total` slots, `per_source` of them for one source; 0 for `per_source` is no share.
    pub fn new(total: usize, per_source: usize) -> Self {
        SourceSlots {
            inner: std::sync::Arc::new(SlotsInner {
                total,
                per_source,
                state: std::sync::Mutex::default(),
            }),
        }
    }

    /// A slot for a connection from `source`.
    ///
    /// # Errors
    ///
    /// [`SlotRefused`] when every slot is held, or the source holds its share.
    pub fn take(&self, source: IpAddr) -> Result<SourceSlot, SlotRefused> {
        let key = key_of(source);
        let mut state = crate::sync::lock(&self.inner.state);
        if state.held >= self.inner.total {
            return Err(SlotRefused::Full);
        }
        let from_source = state.by_source.get(&key).copied().unwrap_or(0);
        if self.inner.per_source > 0 && from_source >= self.inner.per_source {
            return Err(SlotRefused::Source);
        }
        state.held += 1;
        state.by_source.insert(key, from_source + 1);
        Ok(SourceSlot {
            inner: std::sync::Arc::clone(&self.inner),
            key,
        })
    }

    /// The slots held now.
    pub fn held(&self) -> usize {
        crate::sync::lock(&self.inner.state).held
    }

    /// The slots `source` holds now.
    pub fn held_by(&self, source: IpAddr) -> usize {
        crate::sync::lock(&self.inner.state)
            .by_source
            .get(&key_of(source))
            .copied()
            .unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.inner.total
    }

    pub fn per_source(&self) -> usize {
        self.inner.per_source
    }
}

impl Drop for SourceSlot {
    fn drop(&mut self) {
        let mut state = crate::sync::lock(&self.inner.state);
        state.held = state.held.saturating_sub(1);
        if let Some(count) = state.by_source.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.by_source.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
#[path = "source_guard_tests.rs"]
mod tests;
