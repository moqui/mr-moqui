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

use std::collections::HashMap;
use std::net::IpAddr;
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

/// The most sources tracked; past this the ones not blocked and oldest go first.
const MAX_SOURCES: usize = 50_000;

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
        if self.entries.get(&source).is_some_and(|e| e.blocked_until.is_none()) {
            self.entries.remove(&source);
        }
    }

    /// A block known from before (the voice services keep them across restarts), or `None` to clear one.
    pub fn set_block(&mut self, source: IpAddr, until: Option<Instant>) {
        match until {
            Some(until) => {
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
            self.entries.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
