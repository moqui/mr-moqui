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

//! Balancing a pool's traffic by protocol (plans/Edge-Node2.md §4.4): which member of a pairing takes a request, a
//! connection or a call, and each member's health. The edge node's registry (`accept.rs`) gives the members that take
//! work; this chooses among them.
//!
//! - **Sticky by a key** ([`Affinity::Key`]): a terminate host's session cookie, a PASS sign-in's session handle, a
//!   delegated call's conversation. The member is chosen by rendezvous hashing (the highest [`weight`] of the key with
//!   each member's id) over the members that take work. There is no table: a restart changes nothing, every member of
//!   an edge cluster chooses the same one from the same list in any order, and a member that goes moves only its own
//!   keys. The key is the client's, but it only picks among the pairing's own members.
//! - **Sticky by address** ([`Affinity::Address`]): a passthrough host's connections and the tunnels, by the client's
//!   address, an IPv6 one by its /64.
//! - **Round robin** ([`Affinity::Any`]): a request with no cookie. **Fewest in flight** ([`Affinity::Fewest`]): a PASS
//!   call outside a sign-in, a delegated call with no conversation.
//!
//! **Health.** A member takes new work while its link is up and it says it is live. One whose work fails or times out
//! [`Rules::failures`] times in a row is set aside for [`Rules::aside_for`]: it gets no new work while another member
//! takes it, then is tried again; a success clears its count. With every member set aside, they are all tried. All of
//! it is in memory ([`Health`]), from the edge node's start.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;
use sha2::{Digest as _, Sha256};

/// How a piece of work is spread over a pairing's members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Affinity<'a> {
    /// To the member a key hashes to ([`weight`]).
    Key(&'a str),
    /// To the member the client's address hashes to, an IPv6 address by its /64.
    Address(IpAddr),
    /// In turn.
    Any,
    /// To the member with the fewest in flight, in turn among equals.
    Fewest,
}

/// Balancing's settings, from the `<edge>` conf.
#[derive(Clone, Debug)]
pub(crate) struct Rules {
    /// The cookie a terminate host's requests are sticky by where its row names none.
    pub sticky_cookie: Arc<str>,
    /// Failures or time-outs in a row that set a member aside, at least 1.
    pub failures: u32,
    /// How long a member is set aside.
    pub aside_for: Duration,
}

impl Rules {
    pub(crate) fn of(settings: &moqui_core::edge::conf::EdgeSettings) -> Self {
        Self {
            sticky_cookie: Arc::from(settings.sticky_cookie.as_str()),
            failures: settings.set_aside_failures.max(1),
            aside_for: settings.set_aside_for,
        }
    }
}

impl Default for Rules {
    fn default() -> Self {
        Self::of(&moqui_core::edge::conf::EdgeSettings::default())
    }
}

/// One member's traffic and health, kept per pairing across its links.
#[derive(Debug, Default)]
pub(crate) struct Health {
    in_flight: AtomicU64,
    requests: AtomicU64,
    failures_in_row: AtomicU32,
    set_asides: AtomicU64,
    aside_until: Mutex<Option<Instant>>,
}

/// A member's [`Health`] as read at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HealthNow {
    pub in_flight: u64,
    pub requests: u64,
    pub failures_in_row: u32,
    pub set_asides: u64,
    /// Until when it is set aside, when it is at the moment read.
    pub aside_until: Option<Instant>,
}

impl Health {
    /// Whether it is set aside at `now`.
    pub(crate) fn aside_at(&self, now: Instant) -> bool {
        lock(&self.aside_until).is_some_and(|until| now < until)
    }

    pub(crate) fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::Acquire)
    }

    pub(crate) fn now(&self, now: Instant) -> HealthNow {
        HealthNow {
            in_flight: self.in_flight.load(Ordering::Acquire),
            requests: self.requests.load(Ordering::Acquire),
            failures_in_row: self.failures_in_row.load(Ordering::Acquire),
            set_asides: self.set_asides.load(Ordering::Acquire),
            aside_until: lock(&self.aside_until).filter(|until| now < *until),
        }
    }

    /// A success: its failures in a row start again.
    fn succeeded(&self) {
        self.failures_in_row.store(0, Ordering::Release);
    }

    /// A failure or time-out at `now`: true when it sets the member aside, at `rules.failures` in a row and again at
    /// each failure after while it is not set aside (a member tried again after its time that fails again).
    fn failed(&self, rules: &Rules, now: Instant) -> bool {
        let in_row = self.failures_in_row.fetch_add(1, Ordering::AcqRel).saturating_add(1);
        if in_row < rules.failures {
            return false;
        }
        let mut until = lock(&self.aside_until);
        if until.is_some_and(|until| now < until) {
            return false;
        }
        *until = Some(now + rules.aside_for);
        self.set_asides.fetch_add(1, Ordering::AcqRel);
        true
    }
}

/// A piece of work sent to a member: counted as in flight until it drops, and its outcome counted toward the member's
/// health.
#[derive(Debug)]
pub(crate) struct Flight {
    health: Arc<Health>,
    rules: Rules,
}

impl Flight {
    /// Count a piece of work sent to the member of `health`.
    pub(crate) fn start(health: Arc<Health>, rules: Rules) -> Self {
        health.in_flight.fetch_add(1, Ordering::AcqRel);
        health.requests.fetch_add(1, Ordering::AcqRel);
        Self { health, rules }
    }

    pub(crate) fn succeeded(&self) {
        self.health.succeeded();
    }

    /// It failed or timed out: true when that set the member aside.
    pub(crate) fn failed(&self) -> bool {
        self.health.failed(&self.rules, Instant::now())
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.health.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A member a choice may fall on: its id and health.
pub(crate) struct Candidate<'a> {
    pub member: &'a str,
    pub health: &'a Health,
}

/// The weight of `member` for `key` in rendezvous hashing: the first eight bytes of SHA-256 over the member's id, a
/// zero byte and the key, so every edge node and every build gives the same.
pub(crate) fn weight(member: &str, key: &str) -> u64 {
    let mut sha = Sha256::new();
    sha.update(member.as_bytes());
    sha.update([0]);
    sha.update(key.as_bytes());
    let digest = sha.finalize();
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(first)
}

/// The key a client's address is sticky by: an IPv4 address (an IPv4-mapped one as IPv4), an IPv6 address's /64.
pub(crate) fn address_key(ip: IpAddr) -> String {
    moqui_lib::source_guard::source_key(ip).to_string()
}

/// The conversation a delegated call belongs to (plans/Edge-Node2.md §4.4): its call session's id, else its phone call's
/// (`sipCallId`, before a session is made), else its leg's; `None` for a call of none, which goes to the member with
/// the fewest in flight. The two calls that name a `sipCallId` (`receive#PhoneCall`, `receive#DeviceCall`) name no
/// session, and the calls after name the session, so a call's first event and the rest may reach two members. That is
/// fine: a local node keeps a call's state in its rows, not in a member's memory, and the edge node's engines wait for
/// each delegated call's answer before the next event of the call.
pub(crate) fn conversation(parameters: &serde_json::Value) -> Option<String> {
    ["callSessionId", "sipCallId", "callLegId"].iter().find_map(|name| {
        let value = match &parameters[*name] {
            serde_json::Value::String(text) => text.trim().to_string(),
            serde_json::Value::Number(number) => number.to_string(),
            _ => String::new(),
        };
        (!value.is_empty()).then(|| format!("{name}:{value}"))
    })
}

/// The longest sticky key hashed; a longer one (a client's oversized cookie) goes in turn.
pub(crate) const MAX_STICKY_KEY: usize = 4096;

/// The index in `candidates` that `affinity` chooses at `now`, `turn` being the pairing's round robin's: among those not
/// set aside, else among all; `None` when there are none.
pub(crate) fn choose(candidates: &[Candidate<'_>], affinity: Affinity<'_>, turn: usize, now: Instant) -> Option<usize> {
    let mut usable: Vec<usize> = (0..candidates.len())
        .filter(|index| !candidates[*index].health.aside_at(now))
        .collect();
    if usable.is_empty() {
        usable = (0..candidates.len()).collect();
    }
    if usable.is_empty() {
        return None;
    }
    let by_key = |key: &str| {
        usable.iter().copied().max_by(|left, right| {
            let (left, right) = (candidates[*left].member, candidates[*right].member);
            // equal weights (never in practice) go by id, so the order of the list never decides
            (weight(left, key), right).cmp(&(weight(right, key), left))
        })
    };
    match affinity {
        // a key longer than any session id or call id is not hashed: such a request goes in turn
        Affinity::Key(key) if key.len() > MAX_STICKY_KEY => Some(usable[turn % usable.len()]),
        Affinity::Key(key) => by_key(key),
        Affinity::Address(ip) => by_key(&address_key(ip)),
        Affinity::Any => Some(usable[turn % usable.len()]),
        Affinity::Fewest => {
            let start = turn % usable.len();
            usable[start..]
                .iter()
                .chain(&usable[..start])
                .copied()
                .min_by_key(|index| candidates[*index].health.in_flight())
        }
    }
}

#[cfg(test)]
#[path = "balance_tests.rs"]
mod tests;
