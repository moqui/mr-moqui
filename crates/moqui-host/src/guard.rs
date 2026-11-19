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

//! Web protection at the edge, the `web-protection` element of the `webroot` webapp: per-client connection caps,
//! rate limits and bans, and client, method, path and user-agent rules, checked on the connection's task before a
//! session, a request worker or the database is spent. In memory and per node. See
//! `plans/proposals/Web-Protection.md`.
//!
//! This is not the ArtifactTarpit, which counts a user's use of screens and services inside a worker, after login
//! and authorization, for business quotas. The two apply together, this one first.
//!
//! The per-client parts are on when `enabled` is `true`, or when it is empty and `trusted-proxies` names a proxy;
//! without a proxy list a deployment behind a proxy would see every client as the proxy. The global limits of
//! [`crate::http::ListenLimits`] and the overload answers apply either way.
//!
//! **Per host** (plans/Edge-Node2.md §4.3): a `rate-limit` or a client, path or user-agent rule with `host` applies only
//! to the requests for those host names, on top of the rules without one, so one site can be stricter than another. A
//! connection's checks, before its first request names a host, use only the client rules without `host`.
//!
//! **CI runners** ([`ci_credential`], [`Guard::ci_token_verified`]): a request to `/ci/runner/` or `/git/` with a CI
//! token that the CI endpoints or the git server found good a short time ago is counted by its token, by the CI
//! endpoints, so no per-client rate limit applies to it. The answer that found it good carries [`CI_TOKEN_HEADER`],
//! which the node serving the request records, and an edge node in front of it records too as the answer passes
//! (`http::serve`), so the exemption holds on every node of every topology. A made-up token is counted by its address
//! as any request is, and a failed lookup of one counts toward the `ci-token-failure` ban, not `auth-failure`'s.
//!
//! **Shared bans** (§4.3): a ban started here is told to the other members of the cluster and up to the edge node
//! this node is paired with ([`Guard::set_notifier`], `crate::guard_share`), and one told by them is applied here
//! ([`Guard::apply_shared`]). A ban a paired node told an edge node holds only for that node's hosts
//! ([`Guard::banned_for`]). Each rule counts what it refused, and [`Guard::report`] lists the counts and the bans for
//! System › Security › Web Protection, which can lift a ban ([`Guard::lift`]).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use axum::response::Response;
use moqui_core::web::address::masked;

#[path = "guard_conf.rs"]
mod conf;

use conf::*;
pub(crate) use conf::{ci_credential, protection_node, request_host, setting, Event, GuardConf, CI_TOKEN_HEADER};

const SHARDS: usize = 32;
/// How long a scan of a full client table that found no idle client is believed ([`Guard::evict_idle`]).
const NO_IDLE_FOR: Duration = Duration::from_secs(1);
/// How often a full client table logs that it refuses new clients.
const UNTRACKED_LOG_EVERY: Duration = Duration::from_secs(60);

/// One client in the table.
struct Client {
    connections: usize,
    /// GCRA arrival time of each rate limit, nanoseconds after [`Guard::epoch`].
    arrivals: Vec<u64>,
    /// Start and count of each ban's window.
    windows: Vec<(u64, u32)>,
    banned_until: u64,
    /// What banned it: a ban rule here, another member, the node at the other end of a link.
    ban_source: String,
    seen: u64,
}

/// The longest a ban told by another node lasts here, whatever it says: a week.
const MAX_SHARED_BAN: Duration = Duration::from_secs(7 * 24 * 3600);
/// The bans one paired node may have told an edge node at once; past it a new one is not kept.
const MAX_BANS_PER_NODE: usize = 10_000;
/// The count of the requests refused for a ban here ([`Guard::report`]).
const BANNED: &str = "banned";
/// The count of the requests to a pairing's hosts refused for a ban that pairing told an edge node.
const BANNED_BY_PAIRING: &str = "banned by a pairing";
/// The bans [`Guard::report`] lists at most.
const MAX_REPORTED_BANS: usize = 1_000;

/// The bans one paired node told an edge node: by client key, until when and the ban's name.
type NodeBans = HashMap<IpAddr, (u64, String)>;

/// A ban started or lifted here, for the other members and the links ([`Guard::set_notifier`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BanNotice {
    /// The client's table key: its address, or its IPv6 prefix.
    pub(crate) address: IpAddr,
    /// How long it lasts; 0 for a lift.
    pub(crate) seconds: u64,
    /// The ban rule's name, or why it was lifted.
    pub(crate) ban: String,
    pub(crate) lift: bool,
}

/// Tells the others of a [`BanNotice`].
pub(crate) type Notifier = Arc<dyn Fn(&BanNotice) + Send + Sync>;

/// Refusal counts and overload counts, for `/metrics`.
#[derive(Default)]
pub(crate) struct Counts {
    pub(crate) connections_refused: AtomicU64,
    pub(crate) denied: AtomicU64,
    pub(crate) rate_limited: AtomicU64,
    pub(crate) untracked: AtomicU64,
    pub(crate) queue_full: AtomicU64,
    pub(crate) queue_expired: AtomicU64,
    pub(crate) abandoned: AtomicU64,
}

/// The web protection state of one listener set: the conf and the clients seen.
pub(crate) struct Guard {
    /// A conf reload replaces it in place ([`Self::replace_conf`]); the clients seen stay.
    conf: arc_swap::ArcSwap<GuardConf>,
    shards: Vec<Mutex<HashMap<IpAddr, Client>>>,
    tracked: AtomicUsize,
    epoch: Instant,
    pub(crate) counts: Counts,
    /// What each rule refused, by its label: a rate limit's 429s, a rule's denials, the bans a ban rule started.
    rule_counts: Mutex<HashMap<String, u64>>,
    /// The bans the nodes at the other end of this edge node's links told it, by node and client key: until when, and
    /// the ban's name. Each holds only for its node's hosts ([`Self::banned_for`]).
    link_bans: Mutex<HashMap<String, NodeBans>>,
    notifier: std::sync::OnceLock<Notifier>,
    /// The CI tokens found good a short time ago, by a keyed hash of the host and the token: until when each is
    /// exempt from the per-address rate limits ([`Self::ci_token_verified`]).
    ci_tokens: Mutex<HashMap<u64, Instant>>,
    ci_keys: std::hash::RandomState,
    /// The shard [`Self::evict_idle`] looks in first, turned by one each time.
    evict_from: AtomicUsize,
    /// Until when, nanoseconds after [`Self::epoch`], a scan found no idle client ([`Self::evict_idle`]).
    no_idle_until: AtomicU64,
    /// When a full table last logged a refused client, nanoseconds after [`Self::epoch`]; 0 for never.
    untracked_logged: AtomicU64,
}

/// A connection counted against its client's `max-per-client` until it is dropped.
pub(crate) struct ConnectionSlot {
    counted: Option<(Arc<Guard>, IpAddr)>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        if let Some((guard, key)) = self.counted.take() {
            guard.with_known(key, |client| client.connections = client.connections.saturating_sub(1));
        }
    }
}

/// The answer to a request [`Guard::check`] refused; a `Connection: close` header in it closes the connection.
pub(crate) struct Refused {
    pub(crate) response: Box<Response>,
}

impl Guard {
    pub(crate) fn new(conf: GuardConf) -> Self {
        Self {
            conf: arc_swap::ArcSwap::from_pointee(conf),
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            tracked: AtomicUsize::new(0),
            epoch: Instant::now(),
            counts: Counts::default(),
            rule_counts: Mutex::new(HashMap::new()),
            link_bans: Mutex::new(HashMap::new()),
            notifier: std::sync::OnceLock::new(),
            ci_tokens: Mutex::new(HashMap::new()),
            ci_keys: std::hash::RandomState::new(),
            evict_from: AtomicUsize::new(0),
            untracked_logged: AtomicU64::new(0),
            no_idle_until: AtomicU64::new(0),
        }
    }

    fn ci_key(&self, host: &str, token: &str) -> u64 {
        use std::hash::BuildHasher as _;
        self.ci_keys.hash_one((host, token))
    }

    /// The CI endpoints or the git server found `token` good for a request to `host`: its requests there are exempt
    /// from the per-address rate limits for [`CI_TOKEN_TTL`] after this. Keyed by host too, so a paired node an edge
    /// node forwards to vouches only for its own hosts.
    pub(crate) fn ci_token_verified(&self, host: &str, token: &str) {
        let key = self.ci_key(host, token);
        let now = Instant::now();
        let mut tokens = moqui_lib::sync::lock(&self.ci_tokens);
        if tokens.len() >= MAX_CI_TOKENS && !tokens.contains_key(&key) {
            tokens.retain(|_, until| *until > now);
            if tokens.len() >= MAX_CI_TOKENS {
                return;
            }
        }
        tokens.insert(key, now + CI_TOKEN_TTL);
    }

    /// `token` was refused for a request to `host` (unknown, or its job ended): it is counted by address again.
    pub(crate) fn ci_token_failed(&self, host: &str, token: &str) {
        let key = self.ci_key(host, token);
        moqui_lib::sync::lock(&self.ci_tokens).remove(&key);
    }

    /// Whether a request to `host` at `path` (raw) carries a CI token found good a short time ago ([`ci_credential`]).
    fn ci_token_exempt(&self, host: &str, path: &str, headers: &HeaderMap) -> bool {
        let Some(token) = ci_credential(path, headers) else {
            return false;
        };
        let key = self.ci_key(host, &token);
        moqui_lib::sync::lock(&self.ci_tokens)
            .get(&key)
            .is_some_and(|until| *until > Instant::now())
    }

    /// Tell `notifier` of each ban started here and each lift ([`BanNotice`]); set once.
    pub(crate) fn set_notifier(&self, notifier: Notifier) {
        let _ = self.notifier.set(notifier);
    }

    fn notify(&self, notice: &BanNotice) {
        if let Some(notifier) = self.notifier.get() {
            notifier(notice);
        }
    }

    fn count_rule(&self, label: &str) {
        *moqui_lib::sync::lock(&self.rule_counts)
            .entry(label.to_string())
            .or_insert(0) += 1;
    }

    /// Everything per client off: the global limits only.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self::new(GuardConf::from_node(None, Arc::new(moqui_core::web::address::ClientAddressing::default())))
    }

    /// The conf in place now.
    pub(crate) fn conf(&self) -> arc_swap::Guard<Arc<GuardConf>> {
        self.conf.load()
    }

    /// Take `conf` after a conf reload changed the webapp's `web-protection` (plans/archive/Component-Deploy.md, Phase 1):
    /// the clients seen keep their connections and bans, and their rate and ban windows start again, as the rules
    /// they count against may have changed.
    pub(crate) fn replace_conf(&self, conf: GuardConf) {
        let (limits, bans) = (conf.rate_limits.len(), conf.bans.len());
        self.conf.store(Arc::new(conf));
        for shard in &self.shards {
            for client in moqui_lib::sync::lock(shard).values_mut() {
                client.arrivals = vec![0; limits];
                client.windows = vec![(0, 0); bans];
            }
        }
    }

    fn now(&self) -> u64 {
        nanos(self.epoch.elapsed()).max(1)
    }

    /// The table key of `address`: itself for IPv4, its `client-ipv6-prefix` for IPv6.
    fn key(&self, address: IpAddr) -> IpAddr {
        let address = address.to_canonical();
        if address.is_ipv6() {
            masked(address, self.conf().ipv6_prefix)
        } else {
            address
        }
    }

    fn shard(&self, key: IpAddr) -> &Mutex<HashMap<IpAddr, Client>> {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match key {
            IpAddr::V4(v4) => hasher.write(&v4.octets()),
            IpAddr::V6(v6) => hasher.write(&v6.octets()),
        }
        let index = usize::try_from(hasher.finish() % SHARDS as u64).unwrap_or(0);
        &self.shards[index]
    }

    /// Run `body` on the client of `key`, adding it. When the table is full a new client takes the place of an idle
    /// one ([`Self::evict_idle`]); with none to drop it gets `None`, which is counted and logged and which the callers
    /// answer as a refusal, so a full table never lets a client pass the per-client rules uncounted (SEC-WEB-83).
    fn with_client<R>(&self, key: IpAddr, body: impl FnOnce(&mut Client, u64) -> R) -> Option<R> {
        let now = self.now();
        let full = || self.tracked.load(Ordering::Relaxed) >= self.conf().max_tracked;
        if full() && !moqui_lib::sync::lock(self.shard(key)).contains_key(&key) && !self.evict_idle(now) {
            self.counts.untracked.fetch_add(1, Ordering::Relaxed);
            let last = self.untracked_logged.load(Ordering::Relaxed);
            if (last == 0 || now.saturating_sub(last) >= nanos(UNTRACKED_LOG_EVERY))
                && self
                    .untracked_logged
                    .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                tracing::warn!(
                    "web protection: the client table is full ({} clients, each holding a connection) and new clients are refused; \
                     {} so far (max-tracked-clients)",
                    self.conf().max_tracked,
                    self.counts.untracked.load(Ordering::Relaxed)
                );
            }
            return None;
        }
        let mut shard = moqui_lib::sync::lock(self.shard(key));
        let client = match shard.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                self.tracked.fetch_add(1, Ordering::Relaxed);
                entry.insert(Client {
                    connections: 0,
                    arrivals: vec![0; self.conf().rate_limits.len()],
                    windows: vec![(0, 0); self.conf().bans.len()],
                    banned_until: 0,
                    ban_source: String::new(),
                    seen: now,
                })
            }
        };
        client.seen = now;
        Some(body(client, now))
    }

    /// Make room for a new client in a full table: drop the client seen longest ago that has no connection and no ban,
    /// from the first shard after the last one that gave one up which has such a client; when none is idle, the banned
    /// one without a connection whose ban ends soonest, from the first such shard, so a table filled with bans (a /47
    /// of IPv6 /64s, each banned) does not refuse every new client (SEC-WEB-83). A scan that found no idle client is
    /// remembered for [`NO_IDLE_FOR`], in which a new client goes straight to the bans: one shard's scan, not the
    /// table's, for each. False when every client holds a connection.
    fn evict_idle(&self, now: u64) -> bool {
        let start = self.evict_from.fetch_add(1, Ordering::Relaxed);
        if now >= self.no_idle_until.load(Ordering::Relaxed) {
            for offset in 0..SHARDS {
                let mut shard = moqui_lib::sync::lock(&self.shards[(start + offset) % SHARDS]);
                let oldest = shard
                    .iter()
                    .filter(|(_, client)| client.connections == 0 && client.banned_until <= now)
                    .min_by_key(|(_, client)| client.seen)
                    .map(|(key, _)| *key);
                if let Some(oldest) = oldest {
                    shard.remove(&oldest);
                    self.tracked.fetch_sub(1, Ordering::Relaxed);
                    return true;
                }
            }
            self.no_idle_until
                .store(now.saturating_add(nanos(NO_IDLE_FOR)), Ordering::Relaxed);
        }
        for offset in 0..SHARDS {
            let mut shard = moqui_lib::sync::lock(&self.shards[(start + offset) % SHARDS]);
            let soonest = shard
                .iter()
                .filter(|(_, client)| client.connections == 0)
                .min_by_key(|(_, client)| client.banned_until)
                .map(|(key, _)| *key);
            if let Some(soonest) = soonest {
                shard.remove(&soonest);
                self.tracked.fetch_sub(1, Ordering::Relaxed);
                tracing::debug!("web protection: the client table is full of bans; the ban of {soonest} that ends soonest made room");
                return true;
            }
        }
        false
    }

    /// Run `body` on the client of `key` when it is in the table.
    fn with_known<R>(&self, key: IpAddr, body: impl FnOnce(&mut Client) -> R) -> Option<R> {
        moqui_lib::sync::lock(self.shard(key)).get_mut(&key).map(body)
    }

    /// Whether a client rule exempts `address`: for a request to `host`, the rules for it and those without `host`;
    /// with no host (a connection, a ban), only those without.
    fn exempt(&self, address: IpAddr, host: Option<&str>) -> bool {
        self.client_rule(address, host, false).is_some()
    }

    /// The label of a client rule that denies `address`, as [`Self::exempt`] reads the rules.
    fn denied(&self, address: IpAddr, host: Option<&str>) -> Option<String> {
        self.client_rule(address, host, true)
    }

    fn client_rule(&self, address: IpAddr, host: Option<&str>, deny: bool) -> Option<String> {
        self.conf()
            .client_rules
            .iter()
            .filter(|rule| rule.deny == deny)
            .filter(|rule| host.map_or(rule.hosts.all(), |host| rule.hosts.matches(host)))
            .find(|rule| rule.cidrs.iter().any(|range| range.contains(address)))
            .map(|rule| rule.label.clone())
    }

    fn banned(&self, key: IpAddr) -> bool {
        let now = self.now();
        self.with_known(key, |client| client.banned_until > now)
            .unwrap_or(false)
    }

    /// A connection from `peer`, before the TLS handshake: `None` closes it, for a denied or banned client or one
    /// over `max-per-client`. A trusted proxy and an exempt client are not counted.
    pub(crate) fn admit(self: &Arc<Self>, peer: IpAddr) -> Option<ConnectionSlot> {
        let peer = peer.to_canonical();
        let uncounted = ConnectionSlot { counted: None };
        if !self.conf().enabled || self.conf().addressing.is_trusted(peer) || self.exempt(peer, None) {
            return Some(uncounted);
        }
        let key = self.key(peer);
        if let Some(rule) = self.denied(peer, None) {
            self.counts.connections_refused.fetch_add(1, Ordering::Relaxed);
            self.count_rule(&rule);
            return None;
        }
        let max = self.conf().max_per_client;
        let admitted = self.with_client(key, |client, now| {
            if client.banned_until > now || client.connections >= max {
                false
            } else {
                client.connections += 1;
                true
            }
        });
        match admitted {
            Some(true) => Some(ConnectionSlot {
                counted: Some((Arc::clone(self), key)),
            }),
            None | Some(false) => {
                self.counts.connections_refused.fetch_add(1, Ordering::Relaxed);
                tracing::debug!("web protection: a connection from {peer} was refused");
                None
            }
        }
    }

    /// The client of a request from `peer` with `headers`.
    pub(crate) fn client_of(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let conf = self.conf();
        let header = conf.addressing.header();
        let value = (!header.is_empty())
            .then(|| headers.get(header).and_then(|value| value.to_str().ok()))
            .flatten();
        conf.addressing.resolve(peer, value)
    }

    /// The rules and rate limits for a request from `client` (through `peer`) to `host` (its `Host`, as
    /// [`request_host`] gives it; empty when it named none), once its head is read. A refusal of a client that came
    /// straight to this listener closes the connection; through a trusted proxy it does not, since the proxy's
    /// connection carries other clients.
    pub(crate) fn check(
        &self,
        peer: IpAddr,
        client: IpAddr,
        host: &str,
        method: &str,
        path: &str,
        headers: &HeaderMap,
    ) -> Result<(), Refused> {
        if !self.conf().enabled || self.exempt(client, Some(host)) {
            return Ok(());
        }
        let close = !self.conf().addressing.is_trusted(peer.to_canonical());
        let key = self.key(client);
        let denied = self.denied(client, Some(host));
        if denied.is_some() || self.banned(key) {
            self.counts.denied.fetch_add(1, Ordering::Relaxed);
            self.count_rule(denied.as_deref().unwrap_or(BANNED));
            return Err(Refused {
                response: Box::new(refusal(403, "forbidden", close, None)),
            });
        }
        let agent = headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !agent.is_empty() {
            let rule = self
                .conf()
                .agent_rules
                .iter()
                .find(|rule| rule.hosts.matches(host) && rule.parts.iter().any(|part| agent.contains(part.as_str())))
                .map(|rule| rule.label.clone());
            if let Some(rule) = rule {
                return Err(self.deny(key, client, &rule, refusal(403, "forbidden", close, None)));
            }
        }
        if !self.conf().methods.is_empty() && !self.conf().methods.iter().any(|allowed| allowed == method) {
            let mut response = refusal(405, "method not allowed", false, None);
            if let Ok(allow) = self.conf().methods.join(", ").parse() {
                response.headers_mut().insert(axum::http::header::ALLOW, allow);
            }
            return Err(self.deny(key, client, "requests methods", response));
        }
        let decoded = decode_path(path);
        // the path as sent and the screen path it reaches: a rule or a limit that names either applies
        let resolved = screen_path(path);
        let resolved = Some(resolved).filter(|resolved| *resolved != decoded);
        if let Some(rule) = self.path_rule(host, &decoded, resolved.as_deref()) {
            return Err(self.deny(key, client, &rule, refusal(404, "not found", false, None)));
        }
        // a CI token found good is limited per token by the CI endpoints, not per address here; the raw path, as the
        // router routes it, so no path a screen resolves elsewhere passes as the runner REST
        if self.conf().rate_limits.is_empty() || self.ci_token_exempt(host, path, headers) {
            return Ok(());
        }
        let credentials = has_credentials(headers);
        self.limit(key, client, |limit| {
            limit.applies(host, method, &decoded, credentials)
                || resolved
                    .as_deref()
                    .is_some_and(|resolved| limit.applies(host, method, resolved, credentials))
        })
    }

    /// The path rules and rate limits again, in the request's worker, for `reached`: the screen paths the request
    /// resolved to (`ScreenUrlInfo::reached_path` under a token route's base, and with the extra path after a
    /// transition), which the path as sent does not show for the FOP servlet's prefix,
    /// a `//name` search path, a `ScreenPathAlias` row, the extra path after a transition or a render-mode extension
    /// (SEC-WEB-81). Only a rule or a limit that matched neither form [`Self::check`] read applies here, so no limit
    /// counts a request twice; the client rules, the methods and the bans were applied there.
    pub(crate) fn check_reached(
        &self,
        client: IpAddr,
        host: &str,
        method: &str,
        path: &str,
        reached: &[String],
        headers: &HeaderMap,
    ) -> Result<(), Refused> {
        if !self.conf().enabled || self.exempt(client, Some(host)) {
            return Ok(());
        }
        let decoded = decode_path(path);
        let resolved = screen_path(path);
        let reached: Vec<String> = reached
            .iter()
            .map(|reached| decode_path(reached))
            .filter(|reached| !reached.eq_ignore_ascii_case(&decoded) && !reached.eq_ignore_ascii_case(&resolved))
            .collect();
        if reached.is_empty() {
            return Ok(());
        }
        let key = self.key(client);
        let rule = self.conf().path_rules.iter().find_map(|rule| {
            let only_reached = rule.hosts.matches(host)
                && rule.globs.iter().any(|glob| {
                    reached.iter().any(|reached| glob.matches(reached))
                        && !glob.matches(&decoded)
                        && !glob.matches(&resolved)
                });
            only_reached.then(|| rule.label.clone())
        });
        if let Some(rule) = rule {
            return Err(self.deny(key, client, &rule, refusal(404, "not found", false, None)));
        }
        if self.conf().rate_limits.is_empty() || self.ci_token_exempt(host, path, headers) {
            return Ok(());
        }
        let credentials = has_credentials(headers);
        self.limit(key, client, |limit| {
            reached.iter().any(|reached| limit.applies(host, method, reached, credentials))
                && !limit.applies(host, method, &decoded, credentials)
                && !limit.applies(host, method, &resolved, credentials)
        })
    }

    /// Count a request of `client` against each rate limit `applies` picks: a 429 past one's burst (which then takes
    /// nothing from the others), and a 503 when the table is full with every client holding a connection.
    fn limit(&self, key: IpAddr, client: IpAddr, applies: impl Fn(&RateLimit) -> bool) -> Result<(), Refused> {
        let limits = &self.conf().rate_limits;
        let waited = self.with_client(key, |client_state, now| {
            // a conf reload between reading the rules and this client (Self::replace_conf) changes their count
            client_state.arrivals.resize(limits.len(), 0);
            let mut next = Vec::with_capacity(limits.len());
            for (index, limit) in limits.iter().enumerate() {
                if !applies(limit) {
                    next.push(None);
                    continue;
                }
                let arrival = client_state.arrivals[index].max(now).saturating_add(limit.interval);
                let ahead = arrival - now;
                if ahead > limit.tolerance {
                    return Some((ahead - limit.tolerance, limit.name.clone()));
                }
                next.push(Some(arrival));
            }
            for (index, arrival) in next.into_iter().enumerate() {
                if let Some(arrival) = arrival {
                    client_state.arrivals[index] = arrival;
                }
            }
            None
        });
        let Some(waited) = waited else {
            // a full table with every client holding a connection: refused, not served without the limits
            return Err(Refused {
                response: Box::new(self.unavailable("the server is busy; try again")),
            });
        };
        let Some((wait, name)) = waited else {
            return Ok(());
        };
        self.counts.rate_limited.fetch_add(1, Ordering::Relaxed);
        self.count_rule(&format!("rate-limit {name}"));
        tracing::debug!("web protection: rate limit {name} refused a request from {client}");
        self.record_key(key, client, Event::RateLimited);
        let seconds = wait.div_ceil(1_000_000_000).max(1);
        Err(Refused {
            response: Box::new(refusal(429, "too many requests", false, Some(seconds))),
        })
    }

    /// The path rules again, for the path a request is served at when that is not the one it came with (git at a
    /// page's URL, served under `/git/`): a `path-rule` that denies the served path stops the request too. The client
    /// rules, the methods and the rate limits were applied by [`Self::check`], to the path it came with.
    #[cfg(any(feature = "git", test))]
    pub(crate) fn check_served_path(&self, client: IpAddr, host: &str, path: &str) -> Result<(), Refused> {
        if !self.conf().enabled || self.exempt(client, Some(host)) {
            return Ok(());
        }
        let decoded = decode_path(path);
        let resolved = screen_path(path);
        if let Some(rule) = self.path_rule(host, &decoded, Some(resolved.as_str())) {
            return Err(self.deny(self.key(client), client, &rule, refusal(404, "not found", false, None)));
        }
        Ok(())
    }

    /// The label of a path rule for `host` that denies `decoded`, or `resolved` (the screen path it reaches).
    fn path_rule(&self, host: &str, decoded: &str, resolved: Option<&str>) -> Option<String> {
        let denies = |glob: &Glob| glob.matches(decoded) || resolved.is_some_and(|resolved| glob.matches(resolved));
        self.conf()
            .path_rules
            .iter()
            .find(|rule| rule.hosts.matches(host) && rule.globs.iter().any(denies))
            .map(|rule| rule.label.clone())
    }

    fn deny(&self, key: IpAddr, client: IpAddr, rule: &str, response: Response) -> Refused {
        self.counts.denied.fetch_add(1, Ordering::Relaxed);
        self.count_rule(rule);
        tracing::debug!("web protection: a request from {client} was denied by {rule}");
        self.record_key(key, client, Event::Denied);
        Refused {
            response: Box::new(response),
        }
    }

    /// Count `event` for `client` toward the bans that count it. Called from the connection task and from request
    /// workers (an auth failure).
    pub(crate) fn record(&self, client: IpAddr, event: Event) {
        if !self.conf().enabled || self.exempt(client, None) {
            return;
        }
        self.record_key(self.key(client), client, event);
    }

    fn record_key(&self, key: IpAddr, client: IpAddr, event: Event) {
        let bans = &self.conf().bans;
        if !bans.iter().any(|ban| ban.events.contains(&event)) {
            return;
        }
        let started = self.with_client(key, |state, now| {
            // a conf reload between reading the rules and this client (Self::replace_conf) changes their count
            state.windows.resize(bans.len(), (0, 0));
            let mut started = None;
            for (index, ban) in bans.iter().enumerate() {
                if !ban.events.contains(&event) {
                    continue;
                }
                let window = &mut state.windows[index];
                if window.1 == 0 || now.saturating_sub(window.0) > ban.window {
                    *window = (now, 0);
                }
                window.1 += 1;
                if window.1 >= ban.threshold {
                    *window = (now, 0);
                    let until = now.saturating_add(ban.duration);
                    if state.banned_until <= now {
                        started = Some((ban.name.clone(), ban.duration));
                        state.ban_source = format!("ban {}", ban.name);
                    }
                    state.banned_until = state.banned_until.max(until);
                }
            }
            started
        });
        if let Some(Some((name, duration))) = started {
            tracing::warn!(
                "web protection: {client} is banned for {} seconds by ban {name}",
                duration / 1_000_000_000
            );
            self.count_rule(&format!("ban {name}"));
            self.notify(&BanNotice {
                address: key,
                seconds: duration / 1_000_000_000,
                ban: name,
                lift: false,
            });
        }
    }

    /// Apply a ban another node told this one of: `address` for `seconds` (at most [`MAX_SHARED_BAN`]), from `source`;
    /// with `node`, a node at the other end of this edge node's links, only for that node's hosts. Never a trusted
    /// proxy, nor a client a rule without `host` exempts. Not told on. True when it was applied.
    pub(crate) fn apply_shared(&self, address: IpAddr, seconds: u64, source: &str, node: Option<&str>) -> bool {
        let address = address.to_canonical();
        if seconds == 0 || self.conf().addressing.is_trusted(address) || self.exempt(address, None) {
            return false;
        }
        let key = self.key(address);
        let duration = nanos(Duration::from_secs(seconds).min(MAX_SHARED_BAN));
        match node {
            Some(node) => {
                let until = self.now().saturating_add(duration);
                let mut bans = moqui_lib::sync::lock(&self.link_bans);
                let of_node = bans.entry(node.to_string()).or_default();
                if !of_node.contains_key(&key) && of_node.len() >= MAX_BANS_PER_NODE {
                    tracing::warn!(
                        "web protection: node {node} has {MAX_BANS_PER_NODE} bans here; one more is not kept"
                    );
                    return false;
                }
                let held = of_node.entry(key).or_insert((0, String::new()));
                held.0 = held.0.max(until);
                held.1 = source.to_string();
                true
            }
            None => self
                .with_client(key, |client, now| {
                    client.banned_until = client.banned_until.max(now.saturating_add(duration));
                    client.ban_source = source.to_string();
                })
                .is_some(),
        }
    }

    /// Whether a ban that `node` (at the other end of this edge node's links) told holds for `client`: a request to one
    /// of its hosts is then refused, and counted.
    pub(crate) fn banned_for(&self, node: &str, client: IpAddr) -> bool {
        if !self.conf().enabled {
            return false;
        }
        let key = self.key(client);
        let now = self.now();
        let banned = moqui_lib::sync::lock(&self.link_bans)
            .get(node)
            .and_then(|of_node| of_node.get(&key))
            .is_some_and(|(until, _)| *until > now);
        if banned {
            self.counts.denied.fetch_add(1, Ordering::Relaxed);
            self.count_rule(BANNED_BY_PAIRING);
        }
        banned
    }

    /// Lift the ban of `address`: with `node`, only the one that node told; else every ban of it here. With `tell`
    /// (an operator's lift), the others are told too. True when there was one.
    pub(crate) fn lift(&self, address: IpAddr, node: Option<&str>, tell: bool) -> bool {
        let key = self.key(address.to_canonical());
        let now = self.now();
        let mut lifted = false;
        {
            let mut bans = moqui_lib::sync::lock(&self.link_bans);
            for (from, of_node) in bans.iter_mut() {
                if node.is_none_or(|node| node == from) {
                    if let Some((until, _)) = of_node.remove(&key) {
                        lifted |= until > now;
                    }
                }
            }
            bans.retain(|_, of_node| !of_node.is_empty());
        }
        if node.is_none() {
            lifted |= self
                .with_known(key, |client| {
                    let was = client.banned_until > now;
                    client.banned_until = 0;
                    client.ban_source.clear();
                    client.windows.iter_mut().for_each(|window| *window = (0, 0));
                    was
                })
                .unwrap_or(false);
        }
        if tell {
            self.notify(&BanNotice {
                address: key,
                seconds: 0,
                ban: "lifted".to_string(),
                lift: true,
            });
        }
        lifted
    }

    /// What web protection is doing, for System › Security › Web Protection: whether the per-client parts are on, the
    /// counts, each rule with what it refused, and the clients banned now (at most [`MAX_REPORTED_BANS`]).
    pub(crate) fn report(&self) -> serde_json::Value {
        use serde_json::json;
        let conf = self.conf();
        let counted = moqui_lib::sync::lock(&self.rule_counts).clone();
        let count = |label: &str| counted.get(label).copied().unwrap_or(0);
        let mut rules = Vec::new();
        for limit in &conf.rate_limits {
            let label = format!("rate-limit {}", limit.name);
            let per = limit.interval.max(1);
            rules.push(
                json!({"kind": "rate-limit", "name": limit.name, "hosts": limit.hosts.text(),
                "detail": format!("{:.3} requests a second, {} at once", 1e9 / per as f64, limit.tolerance / per),
                "count": count(&label)}),
            );
        }
        for ban in &conf.bans {
            let label = format!("ban {}", ban.name);
            rules.push(json!({"kind": "ban", "name": ban.name, "hosts": "",
                "detail": format!("{} events in {} seconds bans for {} seconds", ban.threshold, ban.window / 1_000_000_000,
                    ban.duration / 1_000_000_000),
                "count": count(&label)}));
        }
        for rule in &conf.client_rules {
            rules.push(
                json!({"kind": "client-rule", "name": rule.label, "hosts": rule.hosts.text(),
                "detail": format!("{} {} ranges", if rule.deny { "deny" } else { "exempt" }, rule.cidrs.len()),
                "count": count(&rule.label)}),
            );
        }
        for rule in &conf.path_rules {
            rules.push(
                json!({"kind": "path-rule", "name": rule.label, "hosts": rule.hosts.text(),
                "detail": format!("deny {} paths", rule.globs.len()), "count": count(&rule.label)}),
            );
        }
        for rule in &conf.agent_rules {
            rules.push(
                json!({"kind": "user-agent-rule", "name": rule.label, "hosts": rule.hosts.text(),
                "detail": format!("deny {} user agents", rule.parts.len()), "count": count(&rule.label)}),
            );
        }
        // the requests refused for a ban rather than a rule: this server's own and its members', and on an edge node
        // a pairing's for that pairing's hosts
        rules.push(json!({"kind": "ban", "name": BANNED, "hosts": "",
            "detail": "requests of a banned client", "count": count(BANNED)}));
        rules.push(
            json!({"kind": "ban", "name": BANNED_BY_PAIRING, "hosts": "its own hosts",
            "detail": "requests to a pairing's hosts of a client it banned", "count": count(BANNED_BY_PAIRING)}),
        );
        let now = self.now();
        let seconds = |until: u64| until.saturating_sub(now).div_ceil(1_000_000_000);
        let mut bans = Vec::new();
        for shard in &self.shards {
            for (key, client) in moqui_lib::sync::lock(shard).iter() {
                if client.banned_until > now && bans.len() < MAX_REPORTED_BANS {
                    bans.push(
                        json!({"address": key.to_string(), "seconds": seconds(client.banned_until),
                        "source": client.ban_source, "node": ""}),
                    );
                }
            }
        }
        let link_bans = moqui_lib::sync::lock(&self.link_bans);
        for (node, key, (until, source)) in link_bans
            .iter()
            .flat_map(|(node, of_node)| of_node.iter().map(move |(key, held)| (node, key, held)))
        {
            if *until > now && bans.len() < MAX_REPORTED_BANS {
                bans.push(
                    json!({"address": key.to_string(), "seconds": seconds(*until), "source": source,
                    "node": node}),
                );
            }
        }
        drop(link_bans);
        let values: serde_json::Map<String, serde_json::Value> = self
            .values()
            .iter()
            .map(|(name, value)| ((*name).to_string(), json!(value)))
            .collect();
        json!({"enabled": conf.enabled, "counts": values, "rules": rules, "bans": bans})
    }

    /// Drop the clients idle past every window, with no open connection and no ban.
    pub(crate) fn sweep(&self) {
        let now = self.now();
        let idle = self.conf().idle_after();
        for shard in &self.shards {
            let mut shard = moqui_lib::sync::lock(shard);
            let before = shard.len();
            shard.retain(|_, client| {
                client.connections > 0 || client.banned_until > now || now.saturating_sub(client.seen) <= idle
            });
            self.tracked.fetch_sub(before - shard.len(), Ordering::Relaxed);
        }
        let mut link_bans = moqui_lib::sync::lock(&self.link_bans);
        for of_node in link_bans.values_mut() {
            of_node.retain(|_, (until, _)| *until > now);
        }
        link_bans.retain(|_, of_node| !of_node.is_empty());
    }

    /// A 503 for a busy server, with `Retry-After`.
    pub(crate) fn unavailable(&self, text: &str) -> Response {
        refusal(503, text, false, Some(self.conf().retry_after))
    }

    /// The `/metrics` lines.
    pub(crate) fn metrics(&self) -> String {
        self.values()
            .iter()
            .map(|(name, value)| format!("{name} {value}\n"))
            .collect()
    }

    /// The `/metrics` line names and values, in their order; the telemetry gauges observe the same values.
    pub(crate) fn values(&self) -> [(&'static str, u64); 9] {
        let now = self.now();
        let banned: usize = self
            .shards
            .iter()
            .map(|shard| {
                moqui_lib::sync::lock(shard)
                    .values()
                    .filter(|client| client.banned_until > now)
                    .count()
            })
            .sum();
        let count = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        [
            ("edge_connections_refused", count(&self.counts.connections_refused)),
            ("edge_denied", count(&self.counts.denied)),
            ("edge_rate_limited", count(&self.counts.rate_limited)),
            ("edge_banned_clients", banned as u64),
            ("edge_tracked_clients", self.tracked.load(Ordering::Relaxed) as u64),
            ("edge_untracked", count(&self.counts.untracked)),
            ("request_queue_full", count(&self.counts.queue_full)),
            ("request_queue_expired", count(&self.counts.queue_expired)),
            ("request_abandoned", count(&self.counts.abandoned)),
        ]
    }

    /// Record in the startup report what is on, and warn of a forwarding header that is believed from anyone.
    pub(crate) fn log_start(&self, factory: &moqui_core::context::ExecutionContextFactory) {
        let addressing = &self.conf().addressing;
        if !addressing.header().is_empty() && !addressing.has_trusted() {
            tracing::warn!(
                "client-ip-header {} is believed from any peer: set the webroot webapp's trusted-proxies to the \
                 proxies in front so a client cannot name its own address",
                addressing.header()
            );
        }
        use moqui_core::startup::{integration, State};
        if self.conf().enabled {
            integration(
                factory,
                "Web protection",
                State::on(format!(
                    "{} rate limits, {} bans, {} connections per client",
                    self.conf().rate_limits.len(),
                    self.conf().bans.len(),
                    self.conf().max_per_client
                )),
            );
        } else {
            integration(
                factory,
                "Web protection",
                State::off("global limits only; per client needs trusted-proxies or enabled=true"),
            );
        }
    }
}

/// A short plain response for a refusal: `Retry-After` when `retry` is given, `Connection: close` when `close`.
fn refusal(status: u16, text: &str, close: bool, retry: Option<u64>) -> Response {
    let mut response = crate::http::plain(status, text);
    let headers = response.headers_mut();
    if let Some(seconds) = retry {
        headers.insert(axum::http::header::RETRY_AFTER, axum::http::HeaderValue::from(seconds));
    }
    if close {
        headers.insert(
            axum::http::header::CONNECTION,
            axum::http::HeaderValue::from_static("close"),
        );
    }
    response
}

#[cfg(test)]
#[path = "guard_tests.rs"]
pub(crate) mod tests;
