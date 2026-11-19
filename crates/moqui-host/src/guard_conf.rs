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

//! The `web-protection` conf the guard runs with (`crate::guard`): its rate limits, bans and client, path and
//! user-agent rules as read from the `webroot` webapp's element, the path forms a rule or a limit matches, and the
//! CI tokens counted by themselves.

use std::sync::Arc;
use std::time::Duration;

use axum::http::HeaderMap;
use moqui_core::web::address::{parse_cidrs, Cidr, ClientAddressing};
use moqui_lib::node::MNode;

pub(super) const DEFAULT_MAX_PER_CLIENT: usize = 64;
pub(super) const DEFAULT_MAX_TRACKED: usize = 100_000;
pub(super) const DEFAULT_RETRY_AFTER: u64 = 5;
pub(super) const DEFAULT_QUEUE_TIMEOUT: u64 = 10;
/// The shortest time an idle client stays in the table.
pub(super) const MIN_IDLE: Duration = Duration::from_secs(60);

/// What can count toward a [`Ban`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// Credentials that were not valid: a login form, Basic auth, an API or login key, git.
    AuthFailure,
    /// A 429 from a rate limit.
    RateLimited,
    /// A client, method, path or user-agent rule refused the request.
    Denied,
    /// A well-formed CI token (job, runner or pool) that the CI endpoints or the git server did not know: runners
    /// share addresses, so this counts toward a ban of its own with a higher threshold than `auth-failure`.
    CiTokenFailure,
}

impl Event {
    pub(super) fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "auth-failure" => Some(Self::AuthFailure),
            "rate-limited" => Some(Self::RateLimited),
            "denied" => Some(Self::Denied),
            "ci-token-failure" => Some(Self::CiTokenFailure),
            _ => None,
        }
    }
}

/// A `rate-limit`: `requests` per `seconds` sustained, `burst` at once, for each client, over the requests its
/// `path` and `methods` match (all of them when left out), and with `anonymous` only those that carry no credentials
/// ([`has_credentials`]). GCRA: one arrival time per client and limit.
#[derive(Debug)]
pub(super) struct RateLimit {
    pub(super) name: String,
    pub(super) hosts: Hosts,
    pub(super) paths: Vec<Glob>,
    pub(super) methods: Vec<String>,
    /// `anonymous="true"`: only requests without credentials count, such as a clone of a public repository.
    pub(super) anonymous: bool,
    /// Nanoseconds between requests at the sustained rate.
    pub(super) interval: u64,
    /// How far ahead of now the arrival time may run: `interval * burst`.
    pub(super) tolerance: u64,
}

impl RateLimit {
    pub(super) fn applies(&self, host: &str, method: &str, path: &str, credentials: bool) -> bool {
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
    pub(super) fn parse(text: &str) -> Self {
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
    pub(super) fn all(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `host` (as [`request_host`] gives it; empty when the request named none) is one of these.
    pub(super) fn matches(&self, host: &str) -> bool {
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

    pub(super) fn text(&self) -> String {
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
pub(super) struct ClientRule {
    pub(super) label: String,
    pub(super) hosts: Hosts,
    pub(super) cidrs: Vec<Cidr>,
    pub(super) deny: bool,
}

/// A `path-rule`: the paths it denies, for its hosts.
#[derive(Debug)]
pub(super) struct PathRule {
    pub(super) label: String,
    pub(super) hosts: Hosts,
    pub(super) globs: Vec<Glob>,
}

/// A `user-agent-rule`: the user-agent text it denies, for its hosts.
#[derive(Debug)]
pub(super) struct AgentRule {
    pub(super) label: String,
    pub(super) hosts: Hosts,
    pub(super) parts: Vec<String>,
}

/// The label of a rule in the counts: its kind and its name, or its place among the rules of its kind.
pub(super) fn rule_label(kind: &str, name: &str, index: usize) -> String {
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
pub(super) fn has_credentials(headers: &HeaderMap) -> bool {
    headers.contains_key(axum::http::header::AUTHORIZATION)
        || headers.contains_key("api_key")
        || headers.contains_key("login_key")
}

/// Where a CI token is taken: the runner REST (docs/interfaces/Pipelines.md "The runner REST") and a job's fetch over
/// git HTTP. Matched against the raw path, as the router routes it to those handlers.
pub(super) const CI_TOKEN_PATHS: [&str; 2] = ["/ci/runner/", "/git/"];
/// The CI tokens' prefixes, job, runner and pool (`moqui_git::ci::token`), each before 32 random bytes in hex.
pub(super) const CI_TOKEN_PREFIXES: [&str; 3] = ["mqcj_", "mqcr_", "mqcp_"];
pub(super) const CI_TOKEN_HEX: usize = 64;
/// The answer header of a request whose CI token was found good (set by `git_http`'s responder), which the node
/// serving it and an edge node in front of it record ([`Guard::ci_token_verified`]) and the outermost node removes.
pub(crate) const CI_TOKEN_HEADER: &str = "x-moqui-ci-token";
/// How long a CI token found good is exempt from the per-address limits after its last good request.
pub(super) const CI_TOKEN_TTL: Duration = Duration::from_secs(600);
/// The most CI tokens kept as found good; past it a new one is not kept (its requests are counted by address).
pub(super) const MAX_CI_TOKENS: usize = 16_384;

/// The CI token of a request to `path` (raw, as the router routes it): a well-formed job, runner or pool token in
/// `Authorization: Bearer`, or Basic's password as git sends it, to the runner REST or git HTTP. A path with a `%`
/// escape or a `.` or `..` segment has none, so no path that a screen or another route resolves differently (`/ci%2F
/// runner/../Login/login`) is taken for one. Whether the token is good is not checked here.
pub(crate) fn ci_credential(path: &str, headers: &HeaderMap) -> Option<String> {
    if !CI_TOKEN_PATHS.iter().any(|prefix| path.starts_with(prefix))
        || path.contains('%')
        || path.split('/').any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())?
        .trim();
    let token = match header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    {
        Some(token) => token.trim().to_string(),
        None => moqui_core::web::basic_credentials(header)?.1,
    };
    let well_formed = CI_TOKEN_PREFIXES.iter().any(|prefix| {
        token.strip_prefix(prefix).is_some_and(|hex| {
            hex.len() == CI_TOKEN_HEX && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    });
    well_formed.then_some(token)
}

/// A `ban`: `threshold` of its `events` within `window` bans the client for `duration`.
#[derive(Debug)]
pub(super) struct Ban {
    pub(super) name: String,
    pub(super) events: Vec<Event>,
    pub(super) threshold: u32,
    pub(super) window: u64,
    pub(super) duration: u64,
}

/// A `url-pattern` style path: `*` within a segment, `**` across any number of them. Matched without regard to ASCII
/// case, against the path with its `%XX` escapes decoded.
#[derive(Clone, Debug)]
pub(super) struct Glob {
    pub(super) segments: Vec<String>,
}

impl Glob {
    pub(super) fn new(pattern: &str) -> Option<Self> {
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

    pub(super) fn matches(&self, path: &str) -> bool {
        let path = path.to_ascii_lowercase();
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let segments: Vec<&str> = self.segments.iter().map(String::as_str).collect();
        segments_match(&segments, &parts)
    }
}

pub(super) fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(part, others)| wildcard_match(first, part) && segments_match(rest, others)),
    }
}

/// `*` for any run of characters within one segment.
pub(super) fn wildcard_match(pattern: &str, text: &str) -> bool {
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

/// The path a screen is found at, as the screen walk reads a request path (`moqui_core::screen::screen_path_names`):
/// empty names, `.` and `~name=value` drop, `..` pops, and each other name is percent-decoded. A path rule or a rate
/// limit matches this as well as the path as sent, so `/Login/./login`, `/x/../Login/login` and `/~a=b/Login/login`
/// are the `/Login/login` they reach (SEC-WEB-07).
pub(super) fn screen_path(path: &str) -> String {
    format!("/{}", moqui_core::screen::screen_path_names(path).join("/"))
}

/// `%XX` escapes decoded, a bad one kept as written; `+` is a plus in a path.
pub(super) fn decode_path(path: &str) -> String {
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
    pub(super) enabled: bool,
    pub(super) addressing: Arc<ClientAddressing>,
    pub(super) ipv6_prefix: u8,
    pub(super) max_tracked: usize,
    pub(super) max_per_client: usize,
    pub(super) methods: Vec<String>,
    /// How long a request may wait in the request pool's queue before it is answered 503 instead of run.
    pub(crate) queue_timeout: Option<Duration>,
    /// The `Retry-After` seconds of a 503.
    pub(crate) retry_after: u64,
    pub(super) rate_limits: Vec<RateLimit>,
    pub(super) bans: Vec<Ban>,
    pub(super) client_rules: Vec<ClientRule>,
    pub(super) path_rules: Vec<PathRule>,
    pub(super) agent_rules: Vec<AgentRule>,
}

/// An attribute of `node`'s child `child` (of `node` itself when `child` is empty), trimmed; empty
/// when it is left out, as `moqui_core::web::protection_setting`.
pub(crate) fn setting(node: Option<&MNode>, child: &str, name: &str) -> String {
    let element = match node {
        Some(node) if child.is_empty() => Some(node.clone()),
        Some(node) => node.first_named(child),
        None => None,
    };
    element
        .and_then(|element| element.attribute(name))
        .map(|value| value.trim().to_string())
        .unwrap_or_default()
}

/// A positive number from `text`, else `default`; a value that is there and not a number is logged with `what`.
pub(super) fn number<T: std::str::FromStr + PartialOrd + Default + Copy>(text: &str, default: T, what: &str) -> T {
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

pub(super) fn list(text: &str) -> impl Iterator<Item = &str> {
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
        let enabled = match setting(node, "", "enabled").as_str() {
            "true" => true,
            "false" => false,
            "" => addressing.has_trusted(),
            other => {
                tracing::error!("web-protection enabled: '{other}' is not true or false; it follows trusted-proxies");
                addressing.has_trusted()
            }
        };
        let ipv6_prefix = number(&setting(node, "", "client-ipv6-prefix"), 64u8, "client-ipv6-prefix").min(128);
        let max_tracked = number(
            &setting(node, "", "max-tracked-clients"),
            DEFAULT_MAX_TRACKED,
            "max-tracked-clients",
        );
        let max_per_client = number(
            &setting(node, "connections", "max-per-client"),
            DEFAULT_MAX_PER_CLIENT,
            "connections max-per-client",
        );
        let methods = list(&setting(node, "requests", "methods"))
            .map(str::to_ascii_uppercase)
            .collect();
        let queue_timeout = match setting(node, "overload", "queue-timeout").as_str() {
            "0" => None,
            text => Some(Duration::from_secs(number(
                text,
                DEFAULT_QUEUE_TIMEOUT,
                "overload queue-timeout",
            ))),
        };
        let retry_after = number(
            &setting(node, "overload", "retry-after"),
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

    pub(super) fn read_lists(&mut self, node: &MNode) {
        let attr = |element: &MNode, name: &str| {
            element
                .attribute(name)
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
    pub(super) fn idle_after(&self) -> u64 {
        let longest_ban = self.bans.iter().map(|ban| ban.window).max().unwrap_or(0);
        let longest_refill = self.rate_limits.iter().map(|limit| limit.tolerance).max().unwrap_or(0);
        longest_ban.max(longest_refill).max(nanos(MIN_IDLE))
    }
}

pub(super) fn rate_limit(element: &MNode, attr: &impl Fn(&MNode, &str) -> String) -> Option<RateLimit> {
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

pub(super) fn ban(element: &MNode, attr: &impl Fn(&MNode, &str) -> String) -> Option<Ban> {
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

pub(super) fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

