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

//! Failed digests counted and blocked per source address (plans/Comms-Framework.md §5.2 "Fraud"). Scanners probe a
//! SIP listener for weak credentials; `limit` failures from one address within `window` block that address for
//! `block`, and everything it sends is then dropped without an answer. The account a scanner names is never locked,
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

/// The key a source counts under: an IPv4 address, or an IPv6 address's /64.
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

    /// A failed digest from `source` at `now`.
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
/// `sip_max_transactions`, `sip_max_connections`). 0 turns one off.
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
}

impl Default for RequestLimits {
    fn default() -> Self {
        RequestLimits {
            rate: 20.0,
            burst: 100,
            max_transactions: 2_000,
            max_connections: 500,
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
}

impl RequestGate {
    pub fn new(limits: RequestLimits) -> Self {
        RequestGate {
            limits,
            buckets: HashMap::new(),
            connections: 0,
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

    /// A stream connection opened: false when it is past the cap and must be closed (its close is still told to
    /// [`Self::connection_closed`]).
    pub fn connection_opened(&mut self) -> bool {
        self.connections += 1;
        self.limits.max_connections == 0 || self.connections <= self.limits.max_connections
    }

    pub fn connection_closed(&mut self) {
        self.connections = self.connections.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_from_one_source_is_held_to_its_rate_while_another_goes_on() {
        let mut gate = RequestGate::new(RequestLimits {
            rate: 2.0,
            burst: 5,
            max_transactions: 0,
            max_connections: 0,
        });
        let t0 = Instant::now();
        let flood: IpAddr = "203.0.113.9".parse().unwrap();
        let other: IpAddr = "198.51.100.4".parse().unwrap();
        let passed = (0..50).filter(|_| gate.admit(flood, 0, t0) == Admit::Pass).count();
        assert_eq!(passed, 5, "the burst, then nothing");
        assert_eq!(gate.admit(flood, 0, t0), Admit::RateLimited);
        assert_eq!(gate.admit(other, 0, t0), Admit::Pass, "another source is not held");
        // the same /64 is the same source
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2::2".parse().unwrap();
        for _ in 0..5 {
            assert_eq!(gate.admit(a, 0, t0), Admit::Pass);
        }
        assert_eq!(gate.admit(b, 0, t0), Admit::RateLimited);
        // the rate refills: one second is two more
        let t1 = t0 + Duration::from_secs(1);
        assert_eq!(gate.admit(flood, 0, t1), Admit::Pass);
        assert_eq!(gate.admit(flood, 0, t1), Admit::Pass);
        assert_eq!(gate.admit(flood, 0, t1), Admit::RateLimited);
    }

    #[test]
    fn the_transaction_and_connection_caps() {
        let mut gate = RequestGate::new(RequestLimits {
            rate: 0.0,
            burst: 0,
            max_transactions: 10,
            max_connections: 2,
        });
        let t0 = Instant::now();
        let source: IpAddr = "198.51.100.4".parse().unwrap();
        assert_eq!(gate.admit(source, 9, t0), Admit::Pass);
        assert_eq!(gate.admit(source, 10, t0), Admit::Busy);
        assert!(gate.connection_opened());
        assert!(gate.connection_opened());
        assert!(!gate.connection_opened(), "the third is closed");
        gate.connection_closed();
        assert!(
            !gate.connection_opened(),
            "still two open with the one past the cap counted until its close"
        );
        gate.connection_closed();
        gate.connection_closed();
        assert!(gate.connection_opened());
        assert_eq!(RequestLimits::default().max_connections, 500);
    }

    #[test]
    fn failures_block_a_source_for_a_while_and_not_another() {
        let policy = GuardPolicy {
            limit: 3,
            window: Duration::from_secs(60),
            block: Duration::from_secs(600),
        };
        let mut guard = SourceGuard::new(policy);
        let t0 = Instant::now();
        let scanner: IpAddr = "203.0.113.9".parse().unwrap();
        let trunk: IpAddr = "198.51.100.4".parse().unwrap();
        assert_eq!(guard.failure(scanner, t0), Failure::Counted(1));
        assert_eq!(guard.failure(scanner, t0 + Duration::from_secs(1)), Failure::Counted(2));
        assert_eq!(guard.failure(trunk, t0), Failure::Counted(1));
        assert_eq!(guard.failure(scanner, t0 + Duration::from_secs(2)), Failure::Blocked);
        assert!(guard.is_blocked(scanner, t0 + Duration::from_secs(3)));
        assert_eq!(
            guard.failure(scanner, t0 + Duration::from_secs(4)),
            Failure::StillBlocked
        );
        assert!(
            !guard.is_blocked(trunk, t0 + Duration::from_secs(3)),
            "another source goes on"
        );
        guard.success(trunk);
        assert_eq!(guard.failure(trunk, t0 + Duration::from_secs(5)), Failure::Counted(1));
        // the block ends
        assert!(!guard.is_blocked(scanner, t0 + Duration::from_secs(603)));
        // failures spread past the window do not add up
        assert_eq!(guard.failure(trunk, t0 + Duration::from_secs(120)), Failure::Counted(1));
        // a block from the services, and clearing it
        guard.set_block(trunk, Some(t0 + Duration::from_secs(900)));
        assert!(guard.is_blocked(trunk, t0 + Duration::from_secs(130)));
        guard.set_block(trunk, None);
        assert!(!guard.is_blocked(trunk, t0 + Duration::from_secs(130)));
    }

    #[test]
    fn an_ipv6_source_counts_as_its_64_and_a_full_table_keeps_its_blocks() {
        let policy = GuardPolicy {
            limit: 2,
            window: Duration::from_secs(60),
            block: Duration::from_secs(600),
        };
        let mut guard = SourceGuard::new(policy);
        let t0 = Instant::now();
        let a: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap();
        let other: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(guard.failure(a, t0), Failure::Counted(1));
        assert_eq!(guard.failure(b, t0), Failure::Blocked, "the same /64");
        assert!(!guard.is_blocked(other, t0));
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().unwrap();
        let v4: IpAddr = "203.0.113.9".parse().unwrap();
        guard.failure(mapped, t0);
        assert_eq!(
            guard.failure(v4, t0),
            Failure::Blocked,
            "an IPv4-mapped address is its IPv4 one"
        );
        // a scanner rotating through more sources than the table holds does not clear the blocks
        let mut blocked = 0u32;
        for i in 0..(MAX_SOURCES as u32 + 10) {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(0x0a00_0000 + i));
            guard.failure(ip, t0);
            if i < 5 {
                blocked += u32::from(guard.failure(ip, t0) == Failure::Blocked);
            }
        }
        assert_eq!(blocked, 5);
        assert!(guard.is_blocked(a, t0 + Duration::from_secs(1)));
        assert!(guard.entries.len() <= MAX_SOURCES);
    }
}
