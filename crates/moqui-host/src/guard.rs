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
use moqui_core::web::address::{masked, parse_cidrs, Cidr, ClientAddressing};
use moqui_lib::node::MNode;

const SHARDS: usize = 32;
const DEFAULT_MAX_PER_CLIENT: usize = 64;
const DEFAULT_MAX_TRACKED: usize = 100_000;
const DEFAULT_RETRY_AFTER: u64 = 5;
const DEFAULT_QUEUE_TIMEOUT: u64 = 10;
/// The shortest time an idle client stays in the table.
const MIN_IDLE: Duration = Duration::from_secs(60);

/// What can count toward a [`Ban`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Credentials that were not valid: a login form, Basic auth, an API or login key, git.
    AuthFailure,
    /// A 429 from a rate limit.
    RateLimited,
    /// A client, method, path or user-agent rule refused the request.
    Denied,
}

impl Event {
    fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "auth-failure" => Some(Self::AuthFailure),
            "rate-limited" => Some(Self::RateLimited),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }
}

/// A `rate-limit`: `requests` per `seconds` sustained, `burst` at once, for each client, over the requests its
/// `path` and `methods` match (all of them when left out), and with `anonymous` only those that carry no credentials
/// ([`has_credentials`]). GCRA: one arrival time per client and limit.
#[derive(Debug)]
struct RateLimit {
    name: String,
    hosts: Hosts,
    paths: Vec<Glob>,
    methods: Vec<String>,
    /// `anonymous="true"`: only requests without credentials count, such as a clone of a public repository.
    anonymous: bool,
    /// Nanoseconds between requests at the sustained rate.
    interval: u64,
    /// How far ahead of now the arrival time may run: `interval * burst`.
    tolerance: u64,
}

impl RateLimit {
    fn applies(&self, host: &str, method: &str, path: &str, credentials: bool) -> bool {
        !(self.anonymous && credentials)
            && self.hosts.matches(host)
            && (self.methods.is_empty() || self.methods.iter().any(|allowed| allowed.eq_ignore_ascii_case(method)))
            && (self.paths.is_empty() || self.paths.iter().any(|glob| glob.matches(path)))
    }
}

/// The `host` of a rule: the host names it applies to, `*.example.com` for any name under one; empty for every host.
/// Matched without regard to ASCII case against the request's host without its port.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hosts(Vec<String>);

impl Hosts {
    fn parse(text: &str) -> Self {
        Self(
            list(text)
                .map(|name| {
                    // a request's host is matched without its port, so a name with one never matches
                    if name.rsplit_once(':').is_some_and(|(_, port)| port.bytes().all(|b| b.is_ascii_digit()))
                        && !name.ends_with(']')
                    {
                        tracing::warn!("web-protection host '{name}' has a port; a request's host is matched without one, so it never applies");
                    }
                    name.trim_end_matches('.').to_ascii_lowercase()
                })
                .collect(),
        )
    }

    /// Whether the rule applies to every host.
    fn all(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `host` (as [`request_host`] gives it; empty when the request named none) is one of these.
    fn matches(&self, host: &str) -> bool {
        self.all()
            || self.0.iter().any(|name| match name.strip_prefix("*.") {
                Some(parent) => {
                    host.len() > parent.len() + 1
                        && host.ends_with(parent)
                        && host.as_bytes()[host.len() - parent.len() - 1] == b'.'
                }
                None => host == name,
            })
    }

    fn text(&self) -> String {
        self.0.join(", ")
    }
}

/// The host name of a request's `Host` header or authority, for the rules' `host`: lower case, without its port or a
/// trailing dot; an IPv6 literal keeps its brackets.
pub(crate) fn request_host(host: &str) -> String {
    let host = host.trim();
    let name = if host.starts_with('[') {
        host.split_once(']').map_or(host, |(literal, _)| literal).to_string() + "]"
    } else {
        host.rsplit_once(':')
            .filter(|(_, port)| port.bytes().all(|byte| byte.is_ascii_digit()))
            .map_or(host, |(name, _)| name)
            .to_string()
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// A `client-rule`: `deny` or exempt the clients of its ranges, for its hosts.
#[derive(Debug)]
struct ClientRule {
    label: String,
    hosts: Hosts,
    cidrs: Vec<Cidr>,
    deny: bool,
}

/// A `path-rule`: the paths it denies, for its hosts.
#[derive(Debug)]
struct PathRule {
    label: String,
    hosts: Hosts,
    globs: Vec<Glob>,
}

/// A `user-agent-rule`: the user-agent text it denies, for its hosts.
#[derive(Debug)]
struct AgentRule {
    label: String,
    hosts: Hosts,
    parts: Vec<String>,
}

/// The label of a rule in the counts: its kind and its name, or its place among the rules of its kind.
fn rule_label(kind: &str, name: &str, index: usize) -> String {
    if name.is_empty() {
        format!("{kind} #{}", index + 1)
    } else {
        format!("{kind} {name}")
    }
}

/// Whether a request names who it is from, by what its head shows: an `Authorization` header (Basic, Bearer, or a
/// token of another scheme) or an `api_key` or `login_key` header. They are not checked here, and a session cookie
/// does not count: the session is read later, in a request worker. A client that sends made-up credentials to pass an
/// anonymous limit meets the `auth-failure` ban instead.
fn has_credentials(headers: &HeaderMap) -> bool {
    headers.contains_key(axum::http::header::AUTHORIZATION)
        || headers.contains_key("api_key")
        || headers.contains_key("login_key")
}

/// A `ban`: `threshold` of its `events` within `window` bans the client for `duration`.
#[derive(Debug)]
struct Ban {
    name: String,
    events: Vec<Event>,
    threshold: u32,
    window: u64,
    duration: u64,
}

/// A `url-pattern` style path: `*` within a segment, `**` across any number of them. Matched without regard to ASCII
/// case, against the path with its `%XX` escapes decoded.
#[derive(Clone, Debug)]
struct Glob {
    segments: Vec<String>,
}

impl Glob {
    fn new(pattern: &str) -> Option<Self> {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return None;
        }
        Some(Self {
            segments: pattern
                .trim_start_matches('/')
                .split('/')
                .map(str::to_ascii_lowercase)
                .collect(),
        })
    }

    fn matches(&self, path: &str) -> bool {
        let path = path.to_ascii_lowercase();
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let segments: Vec<&str> = self.segments.iter().map(String::as_str).collect();
        segments_match(&segments, &parts)
    }
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(part, others)| wildcard_match(first, part) && segments_match(rest, others)),
    }
}

/// `*` for any run of characters within one segment.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while t < text.len() {
        if p < pattern.len() && pattern[p] != b'*' && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|byte| *byte == b'*')
}

/// `%XX` escapes decoded, a bad one kept as written; `+` is a plus in a path.
fn decode_path(path: &str) -> String {
    if !path.contains('%') {
        return path.to_string();
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|byte| char::from(*byte).to_digit(16));
        match (bytes[index], hex(index + 1), hex(index + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push(u8::try_from(high * 16 + low).unwrap_or(b'%'));
                index += 3;
            }
            (byte, _, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `web-protection` conf, read when the listener starts and again after a conf reload changes it.
#[derive(Debug)]
pub(crate) struct GuardConf {
    /// The per-client parts: rules, rate limits, bans, `max-per-client`.
    enabled: bool,
    addressing: Arc<ClientAddressing>,
    ipv6_prefix: u8,
    max_tracked: usize,
    max_per_client: usize,
    methods: Vec<String>,
    /// How long a request may wait in the request pool's queue before it is answered 503 instead of run.
    pub(crate) queue_timeout: Option<Duration>,
    /// The `Retry-After` seconds of a 503.
    pub(crate) retry_after: u64,
    rate_limits: Vec<RateLimit>,
    bans: Vec<Ban>,
    client_rules: Vec<ClientRule>,
    path_rules: Vec<PathRule>,
    agent_rules: Vec<AgentRule>,
}

/// An attribute of `node`'s child `child` (of `node` itself when `child` is empty), `${}` expanded and trimmed, or
/// the property `property` when it is left out or expands to nothing, as `moqui_core::web::protection_setting`.
pub(crate) fn setting(node: Option<&MNode>, child: &str, name: &str, property: &str) -> String {
    let element = match node {
        Some(node) if child.is_empty() => Some(node.clone()),
        Some(node) => node.first_named(child),
        None => None,
    };
    let configured = element
        .and_then(|element| element.attribute(name))
        .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if !configured.is_empty() || property.is_empty() {
        return configured;
    }
    moqui_lib::env::get_prop_or_env(property).trim().to_string()
}

/// A positive number from `text`, else `default`; a value that is there and not a number is logged with `what`.
fn number<T: std::str::FromStr + PartialOrd + Default + Copy>(text: &str, default: T, what: &str) -> T {
    if text.is_empty() {
        return default;
    }
    match text.parse::<T>() {
        Ok(value) if value > T::default() => value,
        _ => {
            tracing::error!("web-protection {what}: '{text}' is not a positive number; it is left out");
            default
        }
    }
}

fn list(text: &str) -> impl Iterator<Item = &str> {
    text.split(',').map(str::trim).filter(|item| !item.is_empty())
}

/// `<web-protection>` of the `webroot` webapp in `conf_text`.
pub(crate) fn protection_node(conf_text: &str) -> Option<MNode> {
    let conf = MNode::parse_shared("moqui-conf", conf_text).ok()?;
    conf.first_named("webapp-list")?
        .children_named("webapp")
        .into_iter()
        .find(|webapp| webapp.attribute_is("name", "webroot"))?
        .first_named("web-protection")
}

impl GuardConf {
    /// From a `web-protection` element; with none, the properties and defaults (the per-client parts off unless
    /// `addressing` has trusted proxies).
    pub(crate) fn from_node(node: Option<&MNode>, addressing: Arc<ClientAddressing>) -> Self {
        let enabled = match setting(node, "", "enabled", "webapp_web_protection").as_str() {
            "true" => true,
            "false" => false,
            "" => addressing.has_trusted(),
            other => {
                tracing::error!("web-protection enabled: '{other}' is not true or false; it follows trusted-proxies");
                addressing.has_trusted()
            }
        };
        let ipv6_prefix = number(&setting(node, "", "client-ipv6-prefix", ""), 64u8, "client-ipv6-prefix").min(128);
        let max_tracked = number(
            &setting(node, "", "max-tracked-clients", ""),
            DEFAULT_MAX_TRACKED,
            "max-tracked-clients",
        );
        let max_per_client = number(
            &setting(node, "connections", "max-per-client", ""),
            DEFAULT_MAX_PER_CLIENT,
            "connections max-per-client",
        );
        let methods = list(&setting(node, "requests", "methods", ""))
            .map(str::to_ascii_uppercase)
            .collect();
        let queue_timeout = match setting(node, "overload", "queue-timeout", "").as_str() {
            "0" => None,
            text => Some(Duration::from_secs(number(
                text,
                DEFAULT_QUEUE_TIMEOUT,
                "overload queue-timeout",
            ))),
        };
        let retry_after = number(
            &setting(node, "overload", "retry-after", ""),
            DEFAULT_RETRY_AFTER,
            "overload retry-after",
        );
        let mut conf = Self {
            enabled,
            addressing,
            ipv6_prefix,
            max_tracked,
            max_per_client,
            methods,
            queue_timeout,
            retry_after,
            rate_limits: Vec::new(),
            bans: Vec::new(),
            client_rules: Vec::new(),
            path_rules: Vec::new(),
            agent_rules: Vec::new(),
        };
        if let Some(node) = node {
            conf.read_lists(node);
        }
        conf
    }

    fn read_lists(&mut self, node: &MNode) {
        let attr = |element: &MNode, name: &str| {
            element
                .attribute(name)
                .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        for element in node.named_iter("rate-limit") {
            let name = attr(&element, "name");
            if attr(&element, "enabled") == "false" {
                continue;
            }
            match rate_limit(&element, &attr) {
                Some(limit) => self.rate_limits.push(limit),
                None => tracing::error!("web-protection rate-limit '{name}' is not valid; it is left out"),
            }
        }
        for element in node.named_iter("ban") {
            let name = attr(&element, "name");
            if attr(&element, "enabled") == "false" {
                continue;
            }
            match ban(&element, &attr) {
                Some(ban) => self.bans.push(ban),
                None => tracing::error!("web-protection ban '{name}' is not valid; it is left out"),
            }
        }
        let rules = |name: &str| {
            node.children_named(name)
                .into_iter()
                .filter(|element| attr(element, "enabled") != "false")
                .collect::<Vec<_>>()
        };
        for (index, element) in rules("client-rule").into_iter().enumerate() {
            let cidrs = parse_cidrs(&attr(&element, "cidr"), "web-protection client-rule");
            let deny = match attr(&element, "action").as_str() {
                "exempt" => false,
                "deny" => true,
                other => {
                    tracing::error!("web-protection client-rule action '{other}' is not deny or exempt");
                    continue;
                }
            };
            self.client_rules.push(ClientRule {
                label: rule_label("client-rule", &attr(&element, "name"), index),
                hosts: Hosts::parse(&attr(&element, "host")),
                cidrs,
                deny,
            });
        }
        for (index, element) in rules("path-rule").into_iter().enumerate() {
            match attr(&element, "action").as_str() {
                "deny" => self.path_rules.push(PathRule {
                    label: rule_label("path-rule", &attr(&element, "name"), index),
                    hosts: Hosts::parse(&attr(&element, "host")),
                    globs: list(&attr(&element, "path")).filter_map(Glob::new).collect(),
                }),
                other => tracing::error!("web-protection path-rule action '{other}' is not deny"),
            }
        }
        for (index, element) in rules("user-agent-rule").into_iter().enumerate() {
            match attr(&element, "action").as_str() {
                "deny" => self.agent_rules.push(AgentRule {
                    label: rule_label("user-agent-rule", &attr(&element, "name"), index),
                    hosts: Hosts::parse(&attr(&element, "host")),
                    parts: list(&attr(&element, "contains")).map(str::to_ascii_lowercase).collect(),
                }),
                other => tracing::error!("web-protection user-agent-rule action '{other}' is not deny"),
            }
        }
    }

    pub(crate) fn addressing(&self) -> &ClientAddressing {
        &self.addressing
    }

    /// The idle time after which a client is dropped from the table: its longest ban window or rate-limit refill.
    fn idle_after(&self) -> u64 {
        let longest_ban = self.bans.iter().map(|ban| ban.window).max().unwrap_or(0);
        let longest_refill = self.rate_limits.iter().map(|limit| limit.tolerance).max().unwrap_or(0);
        longest_ban.max(longest_refill).max(nanos(MIN_IDLE))
    }
}

fn rate_limit(element: &MNode, attr: &impl Fn(&MNode, &str) -> String) -> Option<RateLimit> {
    let requests = attr(element, "requests")
        .parse::<f64>()
        .ok()
        .filter(|count| *count > 0.0)?;
    let seconds = match attr(element, "seconds").as_str() {
        "" => 1.0,
        text => text.parse::<f64>().ok().filter(|seconds| *seconds > 0.0)?,
    };
    let burst = match attr(element, "burst").as_str() {
        "" => requests.ceil(),
        text => text.parse::<f64>().ok().filter(|burst| *burst >= 1.0)?,
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // positive and far below u64::MAX nanoseconds
    let interval = ((seconds * 1e9) / requests).max(1.0) as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let tolerance = (interval as f64 * burst.floor()) as u64;
    Some(RateLimit {
        name: attr(element, "name"),
        hosts: Hosts::parse(&attr(element, "host")),
        paths: list(&attr(element, "path")).filter_map(Glob::new).collect(),
        methods: list(&attr(element, "methods")).map(str::to_ascii_uppercase).collect(),
        anonymous: attr(element, "anonymous") == "true",
        interval,
        tolerance,
    })
}

fn ban(element: &MNode, attr: &impl Fn(&MNode, &str) -> String) -> Option<Ban> {
    let name = attr(element, "name");
    let mut events = Vec::new();
    for text in list(&attr(element, "events")) {
        match Event::parse(text) {
            Some(event) => events.push(event),
            None => tracing::error!("web-protection ban '{name}': '{text}' is not an event; it is left out"),
        }
    }
    if events.is_empty() {
        return None;
    }
    let threshold = attr(element, "threshold")
        .parse::<u32>()
        .ok()
        .filter(|count| *count > 0)?;
    let window = attr(element, "seconds")
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)?;
    let duration = attr(element, "duration")
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)?;
    Some(Ban {
        name,
        events,
        threshold,
        window: nanos(Duration::from_secs(window)),
        duration: nanos(Duration::from_secs(duration)),
    })
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

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
        }
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
        Self::new(GuardConf::from_node(None, Arc::new(ClientAddressing::default())))
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

    /// Run `body` on the client of `key`, adding it unless the table is full (then `None`, and it is counted).
    fn with_client<R>(&self, key: IpAddr, body: impl FnOnce(&mut Client, u64) -> R) -> Option<R> {
        let now = self.now();
        let mut shard = moqui_lib::sync::lock(self.shard(key));
        let client = match shard.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                if self.tracked.load(Ordering::Relaxed) >= self.conf().max_tracked {
                    self.counts.untracked.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
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
            None => Some(uncounted),
            Some(true) => Some(ConnectionSlot {
                counted: Some((Arc::clone(self), key)),
            }),
            Some(false) => {
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
        if let Some(rule) = self.path_rule(host, &decoded) {
            return Err(self.deny(key, client, &rule, refusal(404, "not found", false, None)));
        }
        if self.conf().rate_limits.is_empty() {
            return Ok(());
        }
        let limits = &self.conf().rate_limits;
        let credentials = has_credentials(headers);
        let waited = self.with_client(key, |client_state, now| {
            // a conf reload between reading the rules and this client (Self::replace_conf) changes their count
            client_state.arrivals.resize(limits.len(), 0);
            let mut next = Vec::with_capacity(limits.len());
            for (index, limit) in limits.iter().enumerate() {
                if !limit.applies(host, method, &decoded, credentials) {
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
        let Some(Some((wait, name))) = waited else {
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
    pub(crate) fn check_served_path(&self, client: IpAddr, host: &str, path: &str) -> Result<(), Refused> {
        if !self.conf().enabled || self.exempt(client, Some(host)) {
            return Ok(());
        }
        let decoded = decode_path(path);
        if let Some(rule) = self.path_rule(host, &decoded) {
            return Err(self.deny(self.key(client), client, &rule, refusal(404, "not found", false, None)));
        }
        Ok(())
    }

    /// The label of a path rule for `host` that denies `decoded`.
    fn path_rule(&self, host: &str, decoded: &str) -> Option<String> {
        self.conf()
            .path_rules
            .iter()
            .find(|rule| rule.hosts.matches(host) && rule.globs.iter().any(|glob| glob.matches(decoded)))
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
                "client-ip-header {} is believed from any peer: set trusted-proxies (webapp_trusted_proxies) to the \
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
