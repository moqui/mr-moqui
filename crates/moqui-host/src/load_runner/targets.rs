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

//! What a load run from Tools may reach and how big it may be (plans/LoadRunner.md §10 Targets and Limits, D6). A
//! run reaches this server, loopback and the URL prefixes of `load_runner_targets`; a dev instance any URL. Without
//! the rule a holder could aim the server at another site, a request forgery and a denial of service tool. Plain
//! http to another machine is refused unless the list names it, since a login crosses in the clear: a prefix carries
//! its scheme, so an https entry does not allow http, and this server's own http URL counts only on a loopback host
//! (behind a load balancer its public name is another machine's). URLs are parsed as reqwest parses them (WHATWG, the
//! `url` crate), so a prefix is matched on the path a request is really sent to, after `..`, `%2e%2e` and `.`
//! segments are resolved.

use std::collections::BTreeMap;
use std::net::IpAddr;

use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::web::origin::{configured_origins, SiteOrigin};

/// The loads that may run at once on an instance when `load_runner_max_runs` is not set (D6).
pub(crate) const DEFAULT_MAX_RUNS: usize = 1;
/// The most users a run may have when `load_runner_max_users` is not set (D6).
pub(crate) const DEFAULT_MAX_USERS: usize = 1000;

/// The targets a run may reach.
#[derive(Clone, Debug, Default)]
pub(crate) struct TargetPolicy {
    /// This server's own origins (the conf webroot's http and https URLs); an http one counts only on a loopback host.
    pub(crate) own: Vec<SiteOrigin>,
    /// The `load_runner_targets` prefixes.
    pub(crate) prefixes: Vec<Prefix>,
    /// A dev instance reaches any URL.
    pub(crate) any: bool,
}

/// A `load_runner_targets` entry: an origin, and the path a target must be at or under (empty for any path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Prefix {
    origin: SiteOrigin,
    path: String,
}

impl TargetPolicy {
    /// The policy of this instance: its own origins, `load_runner_targets`, and whether it is a dev instance.
    pub(crate) fn of_factory(factory: &ExecutionContextFactory) -> Self {
        Self {
            own: configured_origins(factory),
            prefixes: parse_prefixes(&moqui_lib::env::get_prop_or_env("load_runner_targets")),
            any: moqui_core::devtools::is_dev_instance(),
        }
    }

    /// Whether a run may reach `url`; the error says why not.
    pub(crate) fn allow(&self, url: &str) -> Result<(), String> {
        let url = url.trim();
        let Some((origin, path)) = parse_url(url) else {
            return Err(format!("{url} is not an http or https URL"));
        };
        if self.any || is_loopback(&origin.host) {
            return Ok(());
        }
        if origin.scheme == "https" && self.own.contains(&origin) {
            return Ok(());
        }
        if self.prefixes.iter().any(|prefix| prefix.matches(&origin, &path)) {
            return Ok(());
        }
        if origin.scheme == "http" {
            return Err(format!(
                "{url} is plain http to another machine, which a run reaches only when load_runner_targets names it: \
                 a login would cross in the clear"
            ));
        }
        Err(format!(
            "{url} is not this server, loopback, or a prefix in load_runner_targets"
        ))
    }
}

impl Prefix {
    fn matches(&self, origin: &SiteOrigin, path: &str) -> bool {
        if self.origin != *origin {
            return false;
        }
        let prefix = self.path.trim_end_matches('/');
        // at the prefix or under it on a segment boundary: /app does not allow /application
        match path.strip_prefix(prefix) {
            Some(rest) => rest.is_empty() || rest.starts_with(['/', '?', '#']) || prefix.is_empty(),
            None => false,
        }
    }
}

/// The comma- or space-separated prefixes of `load_runner_targets`; an entry that is not an http or https URL is
/// logged and left out.
pub(crate) fn parse_prefixes(text: &str) -> Vec<Prefix> {
    let mut prefixes = Vec::new();
    let entries = text
        .split([',', ' ', '\n', '\t'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty());
    for entry in entries {
        match parse_url(entry) {
            Some((origin, path)) => prefixes.push(Prefix { origin, path }),
            None => tracing::error!(entry, "load_runner_targets: not an http or https URL; left out"),
        }
    }
    prefixes
}

/// The origin and the normalized path of an http or https URL, as reqwest sends it: dot segments (`..`, `%2e%2e`,
/// `.`) resolved and a backslash read as a slash. None for another scheme, no host, or userinfo
/// (`http://127.0.0.1@example.com` reaches example.com, which a reader may not see).
fn parse_url(text: &str) -> Option<(SiteOrigin, String)> {
    let url = url::Url::parse(text.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str().filter(|host| !host.is_empty())?.to_ascii_lowercase();
    let origin = SiteOrigin {
        scheme: url.scheme().to_string(),
        host,
        port: url.port_or_known_default()?,
    };
    Some((origin, url.path().to_string()))
}

/// The literal `localhost`, 127.0.0.0/8 or ::1. Not a name under `.localhost`, which a resolver may answer with any
/// address.
fn is_loopback(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    bare.parse::<IpAddr>().is_ok_and(|address| address.is_loopback())
}

/// What a run is aimed at (§8 New run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetKind {
    /// This server's services, called in process (a plan whose `uses` has `service`).
    Service,
    /// This server over HTTP, by its own URL; the default.
    This,
    /// Another server by URL, which [`TargetPolicy::allow`] checks.
    Url,
}

impl TargetKind {
    /// The `targetKind` parameter; empty is `this`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "service" => Ok(Self::Service),
            "" | "this" => Ok(Self::This),
            "url" => Ok(Self::Url),
            other => Err(format!("{other} is not a target kind (service, this or url)")),
        }
    }
}

/// The `users` of each scenario the `scenarios` overrides set, by scenario name. A `users` that is not a whole number
/// of at least 1 is refused.
pub(crate) fn override_users(scenarios: Option<&Value>) -> Result<BTreeMap<String, usize>, String> {
    let mut users = BTreeMap::new();
    let Some(Value::Map(scenarios)) = scenarios else {
        return Ok(users);
    };
    for (name, scenario) in scenarios {
        let Value::Map(settings) = scenario else {
            continue;
        };
        let Some(value) = settings.get("users").filter(|value| !is_empty(value)) else {
            continue;
        };
        let text = to_plain_string(value);
        match whole_number(&text) {
            Some(count) if count >= 1 => {
                users.insert(name.clone(), count);
            }
            _ => {
                return Err(format!(
                    "Scenario {name}: users must be a whole number of at least 1, not {text}"
                ))
            }
        }
    }
    Ok(users)
}

/// `text` as a whole number: digits, or a decimal with no fraction (`3.0`).
fn whole_number(text: &str) -> Option<usize> {
    let text = text.trim();
    if let Ok(count) = text.parse::<usize>() {
        return Some(count);
    }
    let (whole, fraction) = text.split_once('.')?;
    (fraction.bytes().all(|byte| byte == b'0'))
        .then(|| whole.parse::<usize>().ok())
        .flatten()
}

/// Refuses a run of more than `load_runner_max_users` users in all.
pub(crate) fn check_users(total: usize) -> Result<(), String> {
    let max = max_users();
    if total > max {
        return Err(format!(
            "A run may have at most {max} users (load_runner_max_users); the scenarios ask for {total}"
        ));
    }
    Ok(())
}

/// The loads that may run at once on this instance (`load_runner_max_runs`, D6).
#[expect(
    dead_code,
    reason = "the run registry checks it when it starts a load, which comes with the load engine"
)]
pub(crate) fn max_runs() -> usize {
    limit_of(
        &moqui_lib::env::get_prop_or_env("load_runner_max_runs"),
        DEFAULT_MAX_RUNS,
    )
}

/// The most users a run may have (`load_runner_max_users`, D6).
pub(crate) fn max_users() -> usize {
    limit_of(
        &moqui_lib::env::get_prop_or_env("load_runner_max_users"),
        DEFAULT_MAX_USERS,
    )
}

/// A limit property's value: a whole number of at least 1, else `default` (an unparseable value is logged).
pub(crate) fn limit_of(text: &str, default: usize) -> usize {
    let text = text.trim();
    if text.is_empty() {
        return default;
    }
    match text.parse::<usize>() {
        Ok(limit) if limit >= 1 => limit,
        _ => {
            tracing::error!(
                value = text,
                default,
                "a load runner limit is not a whole number of at least 1"
            );
            default
        }
    }
}

#[cfg(test)]
#[path = "targets_tests.rs"]
mod tests;
