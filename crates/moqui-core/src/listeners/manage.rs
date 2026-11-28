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

//! Listing and switching the listeners as a person asks (plans/Runtime-Config.md §3.6, RC4): what
//! `org.moqui.admin.ListenerServices` and the operator socket share. [`rows`] lists each listener with its state and the
//! settings behind it, each with where its running value comes from. [`change`] checks a change of one listener (its
//! switch, port, host and own settings) and refuses one that would leave no way in over the network, open plain HTTP
//! to every network, or end what depends on SSH, unless it is confirmed; then it keeps the settings that change
//! ([`conf_reload::set_many_holding`]), logs the change on the audit log and the Monitor charts, and asks the registry
//! for one pass that carries who, why and what was set. A way in counts only where clients reach it: SSH not on
//! loopback, HTTP and HTTPS not on loopback unless a proxy on this machine is trusted. Who may ask, and whether they signed in
//! recently, the caller checks ([`Actor`]); the operator socket is itself a way back, so no lockout guard applies to it.
//! A switch has no timer: what is turned off stays off until someone turns it on (§6).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::node::path::AttrPath;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::{
    control, switch, Cause, ListenerControl, ListenerInfo, ListenerSwitch, ReconcileRequest, HTTPS_SWITCH, HTTP_SWITCH,
    SSH_SWITCH, SWITCHES,
};
use crate::context::conf_env::{self, switch_on, Source};
use crate::context::conf_reload::{self, view, Kept, SetOutcome};
use crate::context::conf_schema::{Class, Group};
use crate::context::{framework_thread, ExecutionContext, ExecutionContextFactory};
use crate::startup::State;
use crate::web::address::Cidr;

/// The ways in over the network (§3.6): with all three off only the ways back are left, which need no listener.
pub const WAYS_IN: [&str; 3] = [HTTPS_SWITCH.name, HTTP_SWITCH.name, SSH_SWITCH.name];

/// How long a change waits for the pass it asks for: a bind may take up to 30 s.
const RECONCILE_WAIT: Duration = Duration::from_secs(40);
/// How long a change that closes the connection its request came over waits before it applies, so that its answer
/// goes first (§3.6).
const DEFER: Duration = Duration::from_secs(1);

/// The address the HTTP and HTTPS listeners bind, and gRPC's while its own is empty.
const HTTP_BIND: &str = "webapp-list/webapp[webroot]@http-bind";
/// The reverse proxies believed in front: with none, plain HTTP on a public address reaches clients unencrypted.
const TRUSTED_PROXIES: &str = "webapp-list/webapp[webroot]@trusted-proxies";

/// The address setting of each listener that has one, by its row's name; the web listeners' are IP addresses.
const HOSTS: [(&str, &str); 5] = [
    ("HTTP", HTTP_BIND),
    ("HTTPS", HTTP_BIND),
    ("gRPC", GRPC_BIND),
    ("SSH", "ssh-server@host"),
    ("SMTP", "smtp-server@listen-host"),
];

/// gRPC's own address; empty, it binds `http-bind` and follows it.
const GRPC_BIND: &str = "webapp-list/webapp[webroot]@grpc-listen-bind";
/// The longest reason a change takes: it goes on the audit log, the listener rows and the Monitor charts.
const REASON_MAX: usize = 500;

/// The state of a row while no listeners run in this process (the spec runner, a command line).
const NOT_RUNNING: &str = "not-running";
/// Where a value comes from that a process property set after the boot: a setting set without being kept, or one the
/// OS environment names, which is held until the restart (§3.6, "Kept where").
const SET_LIVE: &str = "set while running, not kept past a restart";

/// A listener's own setting that [`change`] takes by name, beside its switch, port and host: a fixed list, never a free
/// selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnSetting {
    /// The listener's row name.
    pub listener: &'static str,
    /// The name a change gives it by (`access`).
    pub name: &'static str,
    pub selector: &'static str,
    /// The values it takes.
    pub values: &'static [&'static str],
}

/// The listeners' own settings: SSH's access mode (§3.4).
pub const OWN_SETTINGS: [OwnSetting; 1] = [OwnSetting {
    listener: "SSH",
    name: "access",
    selector: "ssh-server@access",
    values: &["open", "login", "key"],
}];

/// One listener switched as a person asks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// The listener's row name, one of [`SWITCHES`] (in any case).
    pub name: String,
    pub enabled: bool,
    /// A new port, 1 to 65535; none keeps the written one.
    pub port: Option<String>,
    /// A new address; none keeps the written one, and an empty one clears it (every address for the web listeners,
    /// 127.0.0.1 for SSH, `http-bind` for gRPC).
    pub host: Option<String>,
    /// The listener's own settings by name ([`OWN_SETTINGS`]).
    pub settings: BTreeMap<String, String>,
    /// Why: required to turn a listener off, and on the audit log either way; one line of at most 500 characters.
    pub reason: String,
    /// Keep the settings in `runtime/.env` (or until the restart, when the environment names one); false sets them as
    /// process properties until the process ends.
    pub persist: bool,
    /// Turning off ends the listener's connections at once, with no grace.
    pub end_now: bool,
    /// Turn off the last way in anyway.
    pub confirm_lockout: bool,
    /// Turn plain HTTP on at a public address with no proxy believed in front anyway.
    pub confirm_public_http: bool,
    /// Turn SSH off anyway while the edge tunnel or the cluster's SSH transport depends on it.
    pub confirm_dependents: bool,
}

impl Change {
    /// The listener `name` turned on or off, kept in `runtime/.env`, with nothing else changed or confirmed.
    #[must_use]
    pub fn new(name: impl Into<String>, enabled: bool) -> Self {
        Self {
            name: name.into(),
            enabled,
            persist: true,
            ..Self::default()
        }
    }
}

/// Who asks for a change: on the audit log, the listener rows' cause and the Monitor annotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Actor {
    /// A signed-in user, whose permission and recent sign-in the caller checked, with the listener their request came
    /// over ([`origin`]).
    User {
        username: String,
        user_id: Option<String>,
        origin: Option<String>,
    },
    /// A peer of the operator socket, by its user id: the socket's own access is the authorization, and it needs no
    /// network listener, so it is itself a way back.
    Operator { uid: u32 },
}

impl Actor {
    /// The user's name, or `operator socket (uid N)`.
    #[must_use]
    pub fn who(&self) -> String {
        match self {
            Self::User { username, .. } => username.clone(),
            Self::Operator { uid } => format!("operator socket (uid {uid})"),
        }
    }

    /// The listener the request came over, when it came over one.
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        match self {
            Self::User { origin, .. } => origin.as_deref(),
            Self::Operator { .. } => None,
        }
    }

    fn user_id(&self) -> Option<&str> {
        match self {
            Self::User { user_id, .. } => user_id.as_deref(),
            Self::Operator { .. } => None,
        }
    }

    /// Whether the lockout guard applies: not to the operator socket, which is itself a way back.
    fn lockout_applies(&self) -> bool {
        matches!(self, Self::User { .. })
    }
}

/// Why a change was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalKind {
    /// Not a listener, a port, a host or a value of its setting; no reason to turn it off.
    Invalid,
    /// The port it would bind is held.
    PortBusy,
    /// It turns off the last way in over the network.
    Lockout,
    /// It opens plain HTTP at a public address with no proxy believed in front.
    PublicHttp,
    /// It turns SSH off while something depends on it.
    Dependents,
    /// A setting could not be kept.
    Failed,
}

impl RefusalKind {
    /// The service's `reasonCode`.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::PortBusy => "portBusy",
            Self::Lockout => "lockout",
            Self::PublicHttp => "publicHttp",
            Self::Dependents => "dependents",
            Self::Failed => "failed",
        }
    }

    /// The parameter that lets the change through, for a refusal a confirmation answers.
    #[must_use]
    pub fn confirmation(self) -> Option<&'static str> {
        match self {
            Self::Lockout => Some("confirmLockout"),
            Self::PublicHttp => Some("confirmPublicHttp"),
            Self::Dependents => Some("confirmDependents"),
            Self::Invalid | Self::PortBusy | Self::Failed => None,
        }
    }
}

/// A change refused, before anything was kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub kind: RefusalKind,
    pub message: String,
}

impl Refusal {
    fn new(kind: RefusalKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal::new(RefusalKind::Invalid, message)
}

/// What became of the pass a change asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// It ran: what it did to each listener.
    Now(Vec<super::Outcome>),
    /// The change closes the connection its request came over: answered first, applied a second later.
    Later,
    /// No listeners run in this process: they read the settings when they start.
    NoListeners,
    /// The registry did not answer in time (the pass goes on) or has stopped.
    Failed(String),
}

impl Applied {
    /// `now`, `deferred`, `no-listeners` or `failed`.
    #[must_use]
    pub fn word(&self) -> &'static str {
        match self {
            Self::Now(_) => "now",
            Self::Later => "deferred",
            Self::NoListeners => "no-listeners",
            Self::Failed(_) => "failed",
        }
    }
}

/// One setting a change kept: its new value and what keeping it did.
#[derive(Clone, Debug)]
pub struct SettingSet {
    pub value: String,
    pub outcome: SetOutcome,
}

/// What a change did.
#[derive(Clone, Debug)]
pub struct Answer {
    /// The listener's row name.
    pub name: &'static str,
    pub enabled: bool,
    /// Each setting kept, its switch first.
    pub settings: Vec<SettingSet>,
    pub applied: Applied,
    /// What else the caller should know: a shared address, a part that waits for a restart.
    pub notes: Vec<String>,
}

impl Answer {
    /// One sentence: the listener, its state and what the pass did to it.
    #[must_use]
    pub fn message(&self) -> String {
        let state = if self.enabled { "on" } else { "off" };
        let applied = match &self.applied {
            Applied::Now(outcomes) => match outcomes.iter().find(|outcome| outcome.name == self.name) {
                Some(outcome) if outcome.detail.is_empty() => outcome.action.word().to_string(),
                Some(outcome) => format!("{}: {}", outcome.action, outcome.detail),
                None => "the listeners did not change".to_string(),
            },
            Applied::Later => {
                "it applies in a second, after this answer, since it closes the connection this request came over"
                    .to_string()
            }
            Applied::NoListeners => "no listeners run in this process: they read it when they start".to_string(),
            Applied::Failed(err) => format!("the listeners did not answer: {err}"),
        };
        format!("{} set {state}; {applied}", self.name)
    }

    /// The answer as `set#Listener` gives it: `name`, `enabled`, `applied` ([`Applied::word`]), `message`,
    /// `settingList` (each `selector`, `envName`, `value`, `previous`, `kept`, `outrankedBy`, `applies` and `note`),
    /// `outcomeList` (each listener's `name`, `action` and `detail`) and `noteList`.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let settings: Vec<Json> = self
            .settings
            .iter()
            .map(|set| {
                let outcome = &set.outcome;
                let shown = |value: Option<&str>| {
                    value.map(|value| {
                        if outcome.secret && !value.is_empty() {
                            view::MASKED.to_string()
                        } else {
                            value.to_string()
                        }
                    })
                };
                let applies = applies(outcome, &self.applied);
                json!({
                    "selector": outcome.selector,
                    "envName": outcome.env_name,
                    "value": shown(Some(&set.value)),
                    "previous": shown(outcome.previous.as_deref()),
                    "kept": outcome.kept.name(),
                    "outrankedBy": outcome.outranked_by.map(Source::name),
                    "applies": applies,
                    "note": format!("{}; {applies}", outcome.kept_note()),
                })
            })
            .collect();
        let outcomes: Vec<Json> = match &self.applied {
            Applied::Now(outcomes) => outcomes
                .iter()
                .map(|outcome| json!({"name": outcome.name, "action": outcome.action.word(), "detail": outcome.detail}))
                .collect(),
            Applied::Later | Applied::NoListeners | Applied::Failed(_) => Vec::new(),
        };
        json!({
            "name": self.name,
            "enabled": self.enabled,
            "applied": self.applied.word(),
            "message": self.message(),
            "settingList": settings,
            "outcomeList": outcomes,
            "noteList": self.notes,
        })
    }
}

/// When one kept setting applies, as the change's pass went.
fn applies(outcome: &SetOutcome, applied: &Applied) -> String {
    if !outcome.problems.is_empty() {
        return format!("applying it failed: {}", outcome.problems.join("; "));
    }
    match (outcome.class, applied) {
        (Class::Restart(reason), _) => format!("it applies at the next start ({reason})"),
        (_, Applied::Now(_)) => "it applies now".to_string(),
        (_, Applied::Later) => "it applies in a second, after this answer".to_string(),
        (_, Applied::NoListeners) => "no listeners run in this process: they read it when they start".to_string(),
        (_, Applied::Failed(err)) => format!("the listeners did not answer: {err}"),
    }
}

/// The listener the work of `ec` came over: its request's (`HTTP`, `HTTPS`), else the one its host marked it with
/// (`SSH` for the SSH server's terminal client, [`ExecutionContext::set_listener_origin`]); none for work that came over
/// none (a job, the operator socket, a request down the edge link).
#[must_use]
pub fn origin(ec: &ExecutionContext) -> Option<String> {
    crate::web::current_request()
        .and_then(|request| request.listener.clone())
        .or_else(|| ec.listener_origin())
}

/// The running conf's value of `selector`, trimmed; empty when it has none. Listener readers read the factory's running
/// conf, never the process-wide setting (§3.6).
fn conf(factory: &ExecutionContextFactory, selector: &str) -> String {
    factory
        .conf_value(selector)
        .map(|found| found.value.trim().to_string())
        .unwrap_or_default()
}

/// The address setting of the listener `name`, when it has one.
fn host_selector(name: &str) -> Option<&'static str> {
    HOSTS
        .iter()
        .find(|(listener, _)| *listener == name)
        .map(|(_, selector)| *selector)
}

/// A port of 1 to 65535, written in digits.
fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let port: u16 = text.parse().ok()?;
    (port != 0).then_some(port)
}

/// Whether `text` is a host a listener may bind: an IP address, or for a listener that is not a web one a DNS name.
fn valid_host(text: &str, web: bool) -> bool {
    if text.parse::<IpAddr>().is_ok() || text.eq_ignore_ascii_case("localhost") {
        return true;
    }
    !web && text.len() <= 253
        && text.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

/// A change checked: its listener and the settings it writes, its switch first.
#[derive(Debug)]
struct Plan {
    entry: &'static ListenerSwitch,
    settings: Vec<(&'static str, String)>,
}

impl Plan {
    /// The value the plan writes at `selector`, when it writes one.
    fn value(&self, selector: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(set, _)| conf_env::same_selector(set, selector))
            .map(|(_, value)| value.as_str())
    }

    /// Whether the plan writes the listener's switch.
    fn switches(&self) -> bool {
        self.value(self.entry.enabled).is_some()
    }

    /// Only the settings that change: their value differs from the running conf (a switch compared as a switch reads,
    /// a host in any case); or, kept (`persist`), where they are kept would change, as a live process property holds
    /// the value (set without keeping it, or until the restart) or `runtime/.env` holds another; or a server started on
    /// an address holds them at its address's (`held`), which a set releases. A setting written at the value it has
    /// would be kept in `runtime/.env` all the same, where it outranks later edits of the conf files.
    fn changed(self, factory: &ExecutionContextFactory, persist: bool, held: &[String]) -> Self {
        let entry = self.entry;
        let settings = self
            .settings
            .into_iter()
            .filter(|(selector, value)| {
                if held.iter().any(|held| conf_env::same_selector(held, selector)) {
                    return true;
                }
                let running = conf(factory, selector);
                let differs = if *selector == entry.enabled {
                    switch_on(&running) != switch_on(value)
                } else if HOSTS.iter().any(|(_, host)| host == selector) {
                    !running.eq_ignore_ascii_case(value)
                } else {
                    running != *value
                };
                differs || (persist && kept_elsewhere(factory, selector, value))
            })
            .collect();
        Self { entry, settings }
    }
}

/// Whether keeping `value` at `selector` would change where it is kept though the running conf has it: a live process
/// property of its environment name holds it (which a kept change clears, keeping the value in `runtime/.env`, or sets
/// again until the restart), or `runtime/.env` holds another value under that name.
fn kept_elsewhere(factory: &ExecutionContextFactory, selector: &str, value: &str) -> bool {
    let name = conf_env::env_name_of(selector);
    if name.is_empty() {
        return false;
    }
    if moqui_lib::env::get_property(&name).is_some() {
        return true;
    }
    let in_file = std::fs::read_to_string(factory.runtime_dir().join(".env"))
        .map(|text| moqui_lib::env::parse_dotenv(&text))
        .unwrap_or_default();
    in_file
        .iter()
        .rev()
        .find(|(held, _)| *held == name)
        .is_some_and(|(_, kept)| kept.trim() != value)
}

/// `change` checked against the listeners and their settings: the listener, a reason to turn it off, a port, a host the
/// listener binds (an IP address for a web listener; kept in lower case, as the listeners read it) and each own
/// setting, which only its own listener takes, at one of its values. A retired spelling cannot pass, as each value is
/// checked against what it may be. Every setting given is in the plan, its switch first ([`Plan::changed`] keeps those
/// that change).
fn validate(change: &Change) -> Result<Plan, Refusal> {
    let given = change.name.trim();
    let Some(entry) = SWITCHES.iter().find(|entry| entry.name.eq_ignore_ascii_case(given)) else {
        let names: Vec<&str> = SWITCHES.iter().map(|entry| entry.name).collect();
        return Err(invalid(format!(
            "No listener is named \"{given}\": one of {}",
            names.join(", ")
        )));
    };
    let name = entry.name;
    let reason = change.reason.trim();
    if !change.enabled && reason.is_empty() {
        return Err(invalid(format!("A reason is required to turn {name} off")));
    }
    // it goes on the audit log, the rows' cause, the Monitor charts and an operator's terminal: one line, no escapes
    if reason.chars().any(char::is_control) {
        return Err(invalid(
            "A reason is one line of text: no control characters or terminal escapes",
        ));
    }
    if reason.chars().count() > REASON_MAX {
        return Err(invalid(format!("A reason is at most {REASON_MAX} characters")));
    }
    let mut settings = vec![(entry.enabled, change.enabled.to_string())];
    if let Some(text) = change.port.as_deref().map(str::trim).filter(|text| !text.is_empty()) {
        let selector = entry
            .port()
            .ok_or_else(|| invalid(format!("{name} has no port setting")))?;
        let port =
            parse_port(text).ok_or_else(|| invalid(format!("\"{text}\" is not a port: a number from 1 to 65535")))?;
        settings.push((selector, port.to_string()));
    }
    if let Some(text) = change.host.as_deref().map(str::trim) {
        let selector = host_selector(name).ok_or_else(|| invalid(format!("{name} has no address setting")))?;
        let web = selector.starts_with("webapp-list/");
        if !text.is_empty() && !valid_host(text, web) {
            return Err(invalid(if web {
                format!("\"{text}\" is not an IP address for {selector}")
            } else {
                format!("\"{text}\" is not a host name or an IP address for {selector}")
            }));
        }
        settings.push((selector, text.to_ascii_lowercase()));
    }
    for (setting, value) in &change.settings {
        let setting = setting.trim();
        let Some(own) = OWN_SETTINGS
            .iter()
            .find(|own| own.listener == name && own.name == setting)
        else {
            return Err(invalid(format!("{setting} is not a setting of {name}")));
        };
        let value = value.trim();
        if !own.values.contains(&value) {
            return Err(invalid(format!(
                "\"{value}\" is not a {name} {setting}: one of {}",
                own.values.join(", ")
            )));
        }
        settings.push((own.selector, value.to_string()));
    }
    Ok(Plan { entry, settings })
}

/// The settings a listener reads, by its name: its switch, ports, address and own settings, with those its row says
/// it reads (HTTPS reads `http-bind` too).
fn selectors_of(entry: &ListenerSwitch, row: Option<&ListenerInfo>) -> Vec<String> {
    let mut selectors: Vec<String> = std::iter::once(entry.enabled)
        .chain(entry.ports.iter().copied())
        .chain(host_selector(entry.name))
        .chain(
            OWN_SETTINGS
                .iter()
                .filter(|own| own.listener == entry.name)
                .map(|own| own.selector),
        )
        .map(str::to_string)
        .collect();
    for selector in row.map(|row| row.selectors.as_slice()).unwrap_or_default() {
        if !selectors.iter().any(|known| conf_env::same_selector(known, selector)) {
            selectors.push(selector.clone());
        }
    }
    selectors
}

/// The address setting the listener `name` binds after `plan`: its own, except gRPC's while `grpc-listen-bind` is empty
/// (or the plan empties it), when it follows `http-bind`.
fn bind_selector(factory: &ExecutionContextFactory, plan: &Plan, name: &str) -> Option<&'static str> {
    let own = host_selector(name)?;
    let follows = own == GRPC_BIND
        && plan
            .value(own)
            .map_or_else(|| conf(factory, own), str::to_string)
            .is_empty();
    Some(if follows { HTTP_BIND } else { own })
}

/// The listeners a change of `plan`'s settings affects: each whose settings include one of them, with the address it
/// binds ([`bind_selector`]), by name in the startup report's order (HTTP and HTTPS both for `http-bind`, and gRPC
/// while it follows it).
fn affected(factory: &ExecutionContextFactory, plan: &Plan, rows: Option<&[ListenerInfo]>) -> Vec<&'static str> {
    SWITCHES
        .iter()
        .filter(|entry| {
            let row = rows.and_then(|rows| rows.iter().find(|row| row.name == entry.name));
            let mut reads = selectors_of(entry, row);
            reads.extend(bind_selector(factory, plan, entry.name).map(str::to_string));
            plan.settings
                .iter()
                .any(|(set, _)| reads.iter().any(|read| conf_env::same_selector(read, set)))
        })
        .map(|entry| entry.name)
        .collect()
}

/// The listeners as a change finds them: their rows while listeners run in this process, else the running conf.
struct Now<'a> {
    factory: &'a ExecutionContextFactory,
    rows: Option<&'a [ListenerInfo]>,
}

impl Now<'_> {
    /// Whether the listener `name`'s switch is on in the running conf.
    fn switched_on(&self, name: &str) -> bool {
        switch(name).is_some_and(|entry| switch_on(&conf(self.factory, entry.enabled)))
    }

    /// Whether the listener `name` is on: its switch is, and while listeners run here its row says it runs too; so a
    /// listener turned off whose pass has not run yet (a deferred change) is off.
    fn on(&self, name: &str) -> bool {
        self.switched_on(name)
            && self.rows.is_none_or(|rows| {
                rows.iter()
                    .any(|row| row.name == name && matches!(row.state, State::On(_)))
            })
    }

    fn row(&self, name: &str) -> Option<&ListenerInfo> {
        self.rows?.iter().find(|row| row.name == name)
    }

    /// The address the listener `name` is bound to while it runs, else its conf's: an IP, a host name, or empty for
    /// every address (SSH's empty host is 127.0.0.1, as it binds it).
    fn host(&self, name: &str) -> String {
        let bound = self
            .row(name)
            .filter(|row| matches!(row.state, State::On(_)))
            .and_then(|row| row.address.as_deref()?.parse::<SocketAddr>().ok());
        if let Some(address) = bound {
            return address.ip().to_string();
        }
        let configured = host_selector(name)
            .map(|selector| conf(self.factory, selector))
            .unwrap_or_default();
        if configured.is_empty() && name == SSH_SWITCH.name {
            "127.0.0.1".to_string()
        } else {
            configured
        }
    }
}

/// One way in as the lockout guard sees it, before or after a change: whether it is on, and where it binds (an IP, a
/// host name, or empty for every address).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Way {
    name: &'static str,
    on: bool,
    host: String,
}

/// Whether `host` is the loopback interface only: `localhost`, or a loopback address (`::ffff:127.0.0.1` too). Another
/// host name is not looked up and counts as reachable.
fn on_loopback(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// Whether `trusted-proxies` (`text`) names a proxy on this machine: an entry that is a loopback address or a range
/// holding one (127.0.0.0/8, ::1, ::ffff:127.0.0.0/104, an IPv4-mapped range taken as its IPv4 range). A load balancer
/// elsewhere does not reach a listener on loopback; an entry that is no range counts for nothing.
fn proxy_on_loopback(text: &str) -> bool {
    text.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let (address, prefix) = match entry.split_once('/') {
                Some((address, prefix)) => match prefix.trim().parse::<u8>() {
                    Ok(prefix) => (address.trim(), Some(prefix)),
                    Err(_) => return false,
                },
                None => (entry, None),
            };
            let Ok(ip) = address.parse::<IpAddr>() else {
                return false;
            };
            let (ip, prefix) = match ip {
                IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                    Some(v4) => (IpAddr::V4(v4), prefix.map(|prefix| prefix.saturating_sub(96))),
                    None => (ip, prefix),
                },
                IpAddr::V4(_) => (ip, prefix),
            };
            let range = match prefix {
                Some(prefix) => format!("{ip}/{prefix}"),
                None => ip.to_string(),
            };
            Cidr::parse(&range).is_some_and(|range| {
                range.contains(IpAddr::V4(Ipv4Addr::LOCALHOST)) || range.contains(IpAddr::V6(Ipv6Addr::LOCALHOST))
            })
        })
}

/// Whether a way in reaches the network: on, and bound where clients reach it. SSH on loopback does not; HTTP and
/// HTTPS on loopback do only behind a proxy on this machine the conf believes (`local_proxy`, [`proxy_on_loopback`]).
fn reachable(way: &Way, local_proxy: bool) -> bool {
    way.on && (!on_loopback(&way.host) || (local_proxy && way.name != SSH_SWITCH.name))
}

/// The ways in before and after a change of `plan`'s settings: the listener the change turns on or off, and the address
/// each of them binds after a change of its address setting (HTTP and HTTPS share `http-bind`).
fn ways(now: &Now, plan: &Plan, enabled: bool) -> (Vec<Way>, Vec<Way>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for name in WAYS_IN {
        let was = Way {
            name,
            on: now.on(name),
            host: now.host(name),
        };
        let on = if name == plan.entry.name && plan.switches() {
            enabled
        } else {
            was.on
        };
        let host = host_selector(name)
            .and_then(|selector| plan.value(selector))
            .map_or_else(|| was.host.clone(), str::to_string);
        after.push(Way { name, on, host });
        before.push(was);
    }
    (before, after)
}

/// The lockout guard (§3.6): after the change no way in reaches the network, while one it affects did before or carried
/// the caller's request. Turning off the last way in and moving it to loopback are both lockouts.
fn locks_out(before: &[Way], after: &[Way], affected: &[&str], local_proxy: bool, origin: Option<&str>) -> bool {
    if after.iter().any(|way| reachable(way, local_proxy)) {
        return false;
    }
    before
        .iter()
        .any(|way| affected.contains(&way.name) && (reachable(way, local_proxy) || origin == Some(way.name)))
}

/// The ways back to a server with no way in over the network, from where the change keeps its first setting
/// (`kept`) under the environment names `names`, the operator socket first: it needs no listener. A value kept in
/// `runtime/.env` outranks the conf files, and a process property outranks `.env` and the conf files in a reload, so
/// editing the conf alone does not bring a listener back.
fn ways_back(name: &str, names: &[String], kept: Option<Kept>) -> String {
    let socket = format!(
        "The ways back: `moqui operator listener {name} on` over the operator socket on this server, which needs no \
         network listener (`moqui operator listeners` lists them; --host and --port move one back)"
    );
    let names = names.join(", ");
    let then = match kept {
        Some(Kept::DotEnv) => format!(
            "; or edit {names} in runtime/.env, where this change keeps it and which outranks the conf files, then \
             `systemctl reload moqui` (SIGHUP) or a restart"
        ),
        Some(Kept::Property) => "; or a restart: this change is held as a process property until the process ends, \
                                 which outranks runtime/.env and the conf files in a reload"
            .to_string(),
        Some(Kept::UntilRestart) => format!(
            "; or a restart: the environment names {names}, so this change is held by this process until a restart, \
             which takes the environment's value again"
        ),
        Some(Kept::SealStore) | None => "; or a restart".to_string(),
    };
    format!("{socket}{then}")
}

/// [`ways_back`] for `plan`'s settings as they would be kept with `persist`.
fn ways_back_of(factory: &ExecutionContextFactory, plan: &Plan, persist: bool) -> String {
    let names: Vec<String> = plan
        .settings
        .iter()
        .map(|(selector, _)| conf_env::env_name_of(selector))
        .collect();
    let kept = plan
        .settings
        .first()
        .and_then(|(selector, _)| conf_reload::keeps(factory, selector, persist).ok());
    ways_back(plan.entry.name, &names, kept)
}

/// Whether plain HTTP bound at `bind` reaches every network that reaches the server (§3.3): every address (empty,
/// `0.0.0.0`, `::`) or one that is not loopback; text that is no address is taken to be one.
fn public_bind(bind: &str) -> bool {
    bind.trim().is_empty() || !on_loopback(bind)
}

/// The public HTTP guard (§3.3), keyed on HTTP's switch: on after the change, turned on by it or bound elsewhere by it,
/// at a public address (`bind`) with no proxy believed in front (`proxies` empty).
fn public_http(on_after: bool, turned_on: bool, bind_moved: bool, bind: &str, proxies: &str) -> Option<String> {
    if !on_after || !(turned_on || bind_moved) || !public_bind(bind) || !proxies.trim().is_empty() {
        return None;
    }
    let shown = match bind.trim() {
        "" => "every address".to_string(),
        text => text.to_string(),
    };
    Some(format!(
        "HTTP at {shown} ({HTTP_BIND}) with no proxy believed in front ({TRUSTED_PROXIES} is empty) serves plain HTTP to \
         every network that reaches this server: sign-ins, sessions and data go unencrypted. Send confirmPublicHttp to \
         turn it on anyway; or bind it to 127.0.0.1 for a proxy on this machine, or name the proxy in trusted-proxies"
    ))
}

/// The dependents guard (§3.4): turning SSH off while the edge tunnel or the cluster's SSH transport uses it.
fn dependents(name: &str, enabled: bool, row: Option<&ListenerInfo>) -> Option<String> {
    if enabled || name != SSH_SWITCH.name {
        return None;
    }
    let row = row.filter(|row| !row.dependents.is_empty())?;
    Some(format!(
        "Turning SSH off ends what depends on it: {}. Send confirmDependents to turn it off anyway",
        row.dependents.join("; ")
    ))
}

/// The guards in order, each unless confirmed: the last way in (not for the operator socket, itself a way back), plain
/// HTTP in public, SSH's dependents.
fn guard(now: &Now, plan: &Plan, change: &Change, affected: &[&str], actor: &Actor) -> Result<(), Refusal> {
    let name = plan.entry.name;
    if !change.confirm_lockout && actor.lockout_applies() {
        let (before, after) = ways(now, plan, change.enabled);
        let local_proxy = proxy_on_loopback(&conf(now.factory, TRUSTED_PROXIES));
        if locks_out(&before, &after, affected, local_proxy, actor.origin()) {
            return Err(Refusal::new(
                RefusalKind::Lockout,
                format!(
                    "This change leaves no way in over the network: none of HTTPS, HTTP and SSH would be on at an address \
                     clients reach (SSH on loopback does not count, nor HTTP or HTTPS on loopback with no trusted proxy). \
                     Send confirmLockout to make it anyway. {}",
                    ways_back_of(now.factory, plan, change.persist)
                ),
            ));
        }
    }
    if !change.confirm_public_http {
        let is_http = name == HTTP_SWITCH.name;
        let switched_on = now.switched_on(HTTP_SWITCH.name);
        let on_after = if is_http { change.enabled } else { switched_on };
        let moved_bind = plan.value(HTTP_BIND);
        let bind = moved_bind.map_or_else(|| conf(now.factory, HTTP_BIND), str::to_string);
        let turned_on = is_http && change.enabled && !switched_on;
        let proxies = conf(now.factory, TRUSTED_PROXIES);
        if let Some(message) = public_http(on_after, turned_on, moved_bind.is_some(), &bind, &proxies) {
            return Err(Refusal::new(RefusalKind::PublicHttp, message));
        }
    }
    if !change.confirm_dependents {
        if let Some(message) = dependents(name, change.enabled, now.row(name)) {
            return Err(Refusal::new(RefusalKind::Dependents, message));
        }
    }
    Ok(())
}

/// Whether binding `host` collides with a bind at `old` on the same port: the same address, or either on every address;
/// a host that is a name is taken to collide. The registry then closes the old before it binds the new.
fn overlaps(old: IpAddr, host: &str) -> bool {
    host.parse::<IpAddr>()
        .map_or(true, |new| new == old || new.is_unspecified() || old.is_unspecified())
}

/// The port check before a change that binds (§3.6), for each live listener the change affects that is on after it
/// and would bind something new: it is not bound now, its port changes, or its address moves somewhere that does not
/// collide with where it is bound. A listener not live yet waits for a restart, whose start checks it.
fn check_ports(
    control: &dyn ListenerControl,
    now: &Now,
    plan: &Plan,
    change: &Change,
    affected: &[&str],
) -> Result<(), Refusal> {
    for name in affected {
        let Some(entry) = switch(name) else {
            continue;
        };
        let Some(row) = now.row(name).filter(|row| row.live) else {
            continue;
        };
        let on_after = if *name == plan.entry.name {
            change.enabled
        } else {
            now.switched_on(name)
        };
        let Some(selector) = entry.port().filter(|_| on_after) else {
            continue;
        };
        let port: u16 = match plan
            .value(selector)
            .map_or_else(|| conf(now.factory, selector), str::to_string)
            .parse()
        {
            Ok(port) => port,
            Err(_) => continue,
        };
        // an empty address is every address, or SSH's 127.0.0.1; gRPC's empty one follows http-bind
        let host = bind_selector(now.factory, plan, name)
            .and_then(|selector| plan.value(selector))
            .map(|host| match host {
                "" if *name == SSH_SWITCH.name => "127.0.0.1",
                "" => "0.0.0.0",
                host => host,
            });
        let bound = row
            .address
            .as_deref()
            .and_then(|address| address.parse::<SocketAddr>().ok())
            .filter(|_| matches!(row.state, State::On(_)));
        let binds_new = match bound {
            None => true,
            Some(old) if old.port() != port => true,
            Some(old) => host.is_some_and(|host| !overlaps(old.ip(), host)),
        };
        if binds_new {
            control
                .check_port(name, host, port)
                .map_err(|err| Refusal::new(RefusalKind::PortBusy, format!("{name} cannot bind {err}")))?;
        }
    }
    Ok(())
}

/// Whether a change closes the connection its request came over, so it is answered first and applied a second later
/// (§3.6): the caller's own listener is among those it affects.
fn defers(origin: Option<&str>, affected: &[&str]) -> bool {
    origin.is_some_and(|origin| affected.contains(&origin))
}

/// What else the caller should know of a change: that `http-bind` is shared, that nothing changed, that `endNow` spared
/// the caller's own listener, the ways back once no way in is left, and of the listener's row after the pass what
/// waits for a restart (an edge node's links on HTTPS turned on live).
fn notes(plan: &Plan, spared: Option<&str>, ways_back: Option<String>, after: Option<&ListenerInfo>) -> Vec<String> {
    let mut notes = Vec::new();
    if plan.settings.is_empty() {
        notes.push("Nothing changed in the conf: each setting already had that value".to_string());
    }
    if plan.value(HTTP_BIND).is_some() {
        notes.push(format!(
            "{HTTP_BIND} is shared: HTTP and HTTPS both bind it, and gRPC too while grpc-listen-bind is empty"
        ));
    }
    if let Some(spared) = spared {
        notes.push(format!(
            "endNow does not end the connections of {spared}, which this request came over, so that this answer gets \
             out: they get the usual grace"
        ));
    }
    if let Some(ways_back) = ways_back {
        notes.push(format!("No way in over the network is left. {ways_back}"));
    }
    if let Some(row) = after {
        if let Some(pending) = &row.pending {
            notes.push(format!("{} waits for a restart: {pending}", row.name));
        }
        for part in row.state.detail().split("; ") {
            if part.contains("after a restart") {
                notes.push(format!("{}: {part}", row.name));
            }
        }
    }
    notes
}

/// One change at a time on a factory: two people switching listeners at once each check the guards against what the
/// other has not applied yet.
#[derive(Default)]
struct Changing(Mutex<()>);

/// Check `change` of one listener, keep its settings and apply them through the listener registry, as `actor` asks
/// (plans/Runtime-Config.md §3.6), one change at a time on a factory. In order: the listener and its values
/// ([`validate`]), of which only those that change are written; a port a listener it affects would bind is free
/// ([`ListenerControl::check_port`]); then the guards, each unless confirmed: no way in over the network left (not for
/// the operator socket), plain HTTP at a public address with no proxy believed in front, SSH's dependents. The settings
/// are kept as one ([`conf_reload::set_many_holding`], holding the listeners' hook), the change is logged on the audit
/// log (a refusal too) and as a `config` annotation on the Monitor charts (written off this thread; one that cannot be
/// written is logged and the change stands), and then one pass is asked for with the actor, the reason and the settings
/// as its cause. A change that affects the listener the request came over is applied a second after its answer, and
/// `end_now` spares that listener's connections so the answer gets out. With no listeners in this process the
/// settings are kept for their start.
///
/// # Errors
///
/// A [`Refusal`] before anything is kept: the change is not valid, a port is held, a guard refuses it, or a setting
/// could not be kept.
pub fn change(factory: &Arc<ExecutionContextFactory>, change: &Change, actor: &Actor) -> Result<Answer, Refusal> {
    let changing = factory.extension(|_| Changing::default());
    let _one = lock(&changing.0);
    let answered = apply(factory, change, actor);
    if let Err(refusal) = &answered {
        audit_refused(
            &change.name,
            actor,
            Some(change.enabled),
            refusal.kind.code(),
            &change.reason,
        );
    }
    answered
}

/// [`change`] once it holds the factory's lock.
fn apply(factory: &Arc<ExecutionContextFactory>, change: &Change, actor: &Actor) -> Result<Answer, Refusal> {
    let control = control(factory);
    let held = control.as_ref().map(|control| control.held()).unwrap_or_default();
    let plan = validate(change)?.changed(factory, change.persist, &held);
    let name = plan.entry.name;
    let rows = control.as_ref().map(|control| control.listeners());
    let now = Now {
        factory,
        rows: rows.as_deref(),
    };
    let affected = affected(factory, &plan, now.rows);
    if let Some(control) = &control {
        check_ports(control.as_ref(), &now, &plan, change, &affected)?;
    }
    guard(&now, &plan, change, &affected, actor)?;
    let was_on = now.on(name);
    let left_none = {
        let (before, after) = ways(&now, &plan, change.enabled);
        let local_proxy = proxy_on_loopback(&conf(factory, TRUSTED_PROXIES));
        locks_out(&before, &after, &affected, local_proxy, actor.origin())
    };

    let written: Vec<(&str, Option<&str>)> = plan
        .settings
        .iter()
        .map(|(selector, value)| (*selector, Some(value.as_str())))
        .collect();
    let outcomes = if written.is_empty() {
        Vec::new()
    } else {
        conf_reload::set_many_holding(factory, &written, change.persist, Group::Listeners)
            .map_err(|err| Refusal::new(RefusalKind::Failed, format!("{name} was not changed: {err}")))?
    };
    let settings: Vec<SettingSet> = plan
        .settings
        .iter()
        .zip(outcomes)
        .map(|((_, value), outcome)| SettingSet {
            value: value.clone(),
            outcome,
        })
        .collect();
    record(factory, name, &settings, change, actor, was_on);

    // the listeners whose connections end now as they stop: the one named, and those the change affects (a stop
    // already draining is cut short too), but never the one this request came over, so that its answer gets out
    let origin = actor.origin();
    let mut end_now: Vec<String> = Vec::new();
    let mut spared = None;
    if change.end_now {
        for listener in std::iter::once(name).chain(affected.iter().copied()) {
            if Some(listener) == origin {
                spared = Some(listener);
            } else if !end_now.iter().any(|ending| ending == listener) {
                end_now.push(listener.to_string());
            }
        }
    }
    let request = ReconcileRequest {
        cause: Cause::Request {
            user: actor.who(),
            reason: change.reason.trim().to_string(),
            selectors: plan
                .settings
                .iter()
                .map(|(selector, _)| (*selector).to_string())
                .collect(),
        },
        end_now,
    };
    let applied = match &control {
        None => Applied::NoListeners,
        Some(control) if defers(origin, &affected) => {
            control.reconcile_later(request, DEFER);
            Applied::Later
        }
        Some(control) => match control.reconcile(request, RECONCILE_WAIT) {
            Ok(outcomes) => Applied::Now(outcomes),
            Err(err) => Applied::Failed(err),
        },
    };
    let after = match (&control, &applied) {
        (Some(control), Applied::Now(_)) => control.listeners().into_iter().find(|row| row.name == name),
        _ => None,
    };
    let ways_back = left_none.then(|| {
        let names: Vec<String> = settings.iter().map(|set| set.outcome.env_name.clone()).collect();
        ways_back(name, &names, settings.first().map(|set| set.outcome.kept))
    });
    Ok(Answer {
        name,
        enabled: change.enabled,
        notes: notes(&plan, spared, ways_back, after.as_ref()),
        settings,
        applied,
    })
}

/// A listener's state for the audit log: on or off, and its settings beside the switch (`port 2222, access open`).
fn state_text<'a>(on: bool, settings: impl Iterator<Item = (&'a str, Option<&'a str>)>) -> String {
    let parts: Vec<String> = settings
        .map(|(selector, value)| {
            let attribute = selector.rsplit_once('@').map_or(selector, |(_, attribute)| attribute);
            format!(
                "{attribute} {}",
                value.filter(|value| !value.is_empty()).unwrap_or("none")
            )
        })
        .collect();
    let state = if on { "on" } else { "off" };
    if parts.is_empty() {
        state.to_string()
    } else {
        format!("{state} ({})", parts.join(", "))
    }
}

/// The audit line of a change: who, the listener, from, to, the reason, end now and persist.
fn audit_line(name: &str, who: &str, from: &str, to: &str, change: &Change) -> String {
    format!(
        "Listener {name} changed by {who}: {from} to {to}; reason: {:?}; end now: {}; persist: {}",
        change.reason.trim(),
        change.end_now,
        change.persist
    )
}

/// Text a person gave, for the audit log: trimmed and at most [`REASON_MAX`] characters, which the log quotes with its
/// control characters escaped.
fn for_log(text: &str) -> String {
    text.trim().chars().take(REASON_MAX).collect()
}

/// Log a refused change on the audit log, as a change is: who, the listener as named, the state asked for (none when
/// the request named none), why it was refused (`code`: `permission`, `reauthRequired`, `lockout`, `publicHttp`,
/// `statusOnly`, `notServing` and the others) and the reason given. The listener services and the operator socket log
/// their own refusals here too.
pub fn audit_refused(name: &str, actor: &Actor, enabled: Option<bool>, code: &str, reason: &str) {
    let state = match enabled {
        Some(true) => "on",
        Some(false) => "off",
        None => "an unnamed state",
    };
    tracing::warn!(
        target: crate::live_settings::AUDIT,
        "Listener {:?} change to {state} by {} refused ({code}); reason: {:?}",
        for_log(name),
        actor.who(),
        for_log(reason)
    );
}

/// Log a change on the audit log (who, the listener, from, to, the reason, end now, persist), as conf saves are, and
/// as a `config` annotation on the Monitor charts, as Monitor › Settings writes one, the annotation on a thread of its
/// own so the change's pass is not held up by it.
fn record(
    factory: &Arc<ExecutionContextFactory>,
    name: &'static str,
    settings: &[SettingSet],
    change: &Change,
    actor: &Actor,
    was_on: bool,
) {
    let who = actor.who();
    // nothing changed in the conf: the pass is still asked for (it tries a failed listener again), and no chart marks it
    if settings.is_empty() {
        let state = if change.enabled { "on" } else { "off" };
        tracing::warn!(
            target: crate::live_settings::AUDIT,
            "Listener {name} unchanged (pass asked) by {who}: asked {state}; reason: {:?}; end now: {}; persist: {}",
            for_log(&change.reason),
            change.end_now,
            change.persist
        );
        return;
    }
    let enabled = switch(name).map_or("", |entry| entry.enabled);
    let others = || {
        settings
            .iter()
            .filter(|set| !conf_env::same_selector(&set.outcome.selector, enabled))
    };
    let from = state_text(
        was_on,
        others().map(|set| (set.outcome.selector.as_str(), set.outcome.previous.as_deref())),
    );
    let to = state_text(
        change.enabled,
        others().map(|set| (set.outcome.selector.as_str(), Some(set.value.as_str()))),
    );
    tracing::warn!(target: crate::live_settings::AUDIT, "{}", audit_line(name, &who, &from, &to, change));
    let text = format!("{name} {from} → {to} by {who}: {}", change.reason.trim());
    let user_id = actor.user_id().map(str::to_string);
    let factory = Arc::clone(factory);
    let spawned = std::thread::Builder::new()
        .name("listener-annotation".to_string())
        .spawn(framework_thread(move || {
            annotate(&factory, name, &text, user_id.as_deref())
        }));
    if let Err(err) = spawned {
        tracing::error!(
            listener = name,
            "the listener change's Monitor annotation was not written: {err}"
        );
    }
}

/// A `config` annotation on the Monitor charts (the document `moqui-report` writes for a runtime override), tagged
/// `listener` and the listener's name. Without search (a test), or when it cannot be written, the change stands and
/// the failure is logged.
fn annotate(factory: &ExecutionContextFactory, name: &str, text: &str, user_id: Option<&str>) {
    let Some(search) = crate::search::installed(factory) else {
        return;
    };
    let written = (|| -> Result<(), String> {
        let id = format!(
            "marker-{}",
            crate::user::random_string(24).map_err(|err| err.to_string())?
        );
        let resource = crate::telemetry::for_factory(factory).resource();
        let annotation = json!({
            "_id": id,
            "time": jiff::Timestamp::now().to_string(),
            "source": "config",
            "scope": "service",
            "service.name": resource.service_name,
            "text": crate::telemetry::redact_text(text),
            "tags": ["listener", name],
            "created_by": user_id,
        });
        let Value::Map(document) = moqui_lib::json::from_json(&annotation) else {
            return Err("the annotation is not a map".to_string());
        };
        match search.index_entries("monitor_annotations", vec![document]) {
            Ok(1) => Ok(()),
            Ok(_) => Err("the annotation could not be indexed".to_string()),
            Err(err) => Err(err.to_string()),
        }
    })();
    if let Err(error) = written {
        tracing::error!(listener = name, %error, "the listener change's Monitor annotation was not written");
    }
}

/// Each listener with a switch in the startup report's order, then the registry's others (the cluster port), as
/// `get#Listeners` answers them: its row ([`ListenerInfo::to_json`]) while listeners run in this process, else the
/// conf's alone with `state` `not-running`; with `running` (listeners run in this process), `switchable`, `wayIn`,
/// `enabled` (its switch in the running conf), `ownRequest` (the caller's request came over it, by `origin`) and
/// `settingList`: each setting behind it with its running value and where that comes from ([`setting_row`]).
#[must_use]
pub fn rows(factory: &ExecutionContextFactory, origin: Option<&str>) -> Vec<Json> {
    let infos = control(factory).map(|control| control.listeners());
    let view = match view::view(factory) {
        Ok(found) => Some(found),
        Err(err) => {
            tracing::debug!("The listeners' settings are listed without their files: {err}");
            None
        }
    };
    let running = infos.is_some();
    let mut rows = Vec::new();
    for entry in &SWITCHES {
        let info = infos
            .as_deref()
            .and_then(|infos| infos.iter().find(|info| info.name == entry.name));
        let enabled = switch_on(&conf(factory, entry.enabled));
        let mut row = match info {
            Some(info) => info.to_json(),
            None => {
                let detail = if running {
                    "this server's registry has no such listener (not in this build)".to_string()
                } else {
                    let state = if enabled { "on" } else { "off" };
                    format!("this process runs no listeners; the conf turns it {state}")
                };
                json!({"name": entry.name, "state": NOT_RUNNING, "detail": detail})
            }
        };
        let settings: Vec<Json> = selectors_of(entry, info)
            .iter()
            .map(|selector| setting_row(factory, view.as_ref(), selector))
            .collect();
        if let Json::Object(fields) = &mut row {
            fields.insert("running".to_string(), json!(running));
            fields.insert("switchable".to_string(), json!(true));
            fields.insert("wayIn".to_string(), json!(WAYS_IN.contains(&entry.name)));
            fields.insert("enabled".to_string(), json!(enabled));
            fields.insert("ownRequest".to_string(), json!(origin == Some(entry.name)));
            fields.insert("settingList".to_string(), json!(settings));
        }
        rows.push(row);
    }
    let others = infos.iter().flatten().filter(|info| switch(&info.name).is_none());
    for info in others {
        let mut row = info.to_json();
        let settings: Vec<Json> = info
            .selectors
            .iter()
            .map(|selector| setting_row(factory, view.as_ref(), selector))
            .collect();
        if let Json::Object(fields) = &mut row {
            fields.insert("running".to_string(), json!(true));
            fields.insert("switchable".to_string(), json!(false));
            fields.insert("wayIn".to_string(), json!(false));
            fields.insert("enabled".to_string(), Json::Null);
            fields.insert("ownRequest".to_string(), json!(origin == Some(info.name.as_str())));
            fields.insert("settingList".to_string(), json!(settings));
        }
        rows.push(row);
    }
    rows
}

/// The attribute at `path` in the conf view, with its element: looked up in the view's tree, which the view's walk
/// built with each value's file and line and the environment name that set it.
fn attr_in_view<'a>(view: &'a view::ConfView, path: &AttrPath) -> Option<(&'a view::ViewNode, &'a view::ViewAttr)> {
    let mut node = &view.root;
    for step in &path.steps {
        let key = (!step.keys.is_empty()).then(|| step.keys.join(","));
        node = node
            .children
            .iter()
            .find(|child| child.name == step.element && (key.is_none() || child.key == key))?;
    }
    let attr = node.attrs.iter().find(|attr| attr.name == path.attribute)?;
    Some((node, attr))
}

/// Where a value an environment name set comes from: a `-D` the boot did not take was set while running.
fn named_source(source: &'static str, boot: &[Source]) -> &'static str {
    if source == Source::Property.name() && !boot.contains(&Source::Property) {
        SET_LIVE
    } else {
        source
    }
}

/// One setting of a listener row: `selector`, `envName`, `value` (the running conf's, masked for a secret), `source`
/// (`conf file`, `runtime/.env`, `the environment`, `-D`, or set while running and not kept) and for a conf file its
/// `file`, `fileLabel` and `line`. Where a value comes from is the conf view's ([`view::view`]), with the boot's
/// environment names ([`conf_env::boot_sources`]) telling a `-D` of the start from a property set since; without the
/// view, the boot's names alone.
fn setting_row(factory: &ExecutionContextFactory, view: Option<&view::ConfView>, selector: &str) -> Json {
    let path = AttrPath::parse(selector).ok();
    let running = factory.conf_value(selector);
    let secret = running
        .as_ref()
        .map_or_else(|| conf_env::selector_secret(selector), |found| found.secret);
    let value = running.map(|found| {
        if secret && !found.value.is_empty() {
            view::MASKED.to_string()
        } else {
            found.value
        }
    });
    let boot = path
        .as_ref()
        .map(|path| conf_env::boot_sources(factory, path))
        .unwrap_or_default();
    let found = view
        .zip(path.as_ref())
        .and_then(|(view, path)| attr_in_view(view, path).map(|(node, attr)| (view, node, attr)));
    let (source, file, line) = match found {
        Some((_, _, attr)) if attr.set_by.is_some() => (
            attr.set_by
                .as_ref()
                .map_or("conf file", |set_by| named_source(set_by.source, &boot)),
            None,
            None,
        ),
        Some((view, node, attr)) => {
            let at = attr.source.or_else(|| node.sources.first().copied());
            (
                "conf file",
                at.and_then(|at| view.files.get(at.file)),
                at.map(|at| at.line),
            )
        }
        None => (boot.first().map_or("conf file", |source| source.name()), None, None),
    };
    json!({
        "selector": selector,
        "envName": conf_env::env_name_of(selector),
        "value": value,
        "source": source,
        "file": file.map(|file| file.path.clone()),
        "fileLabel": file.map(|file| file.label.clone()),
        "line": line,
    })
}

#[cfg(test)]
#[path = "manage_tests.rs"]
mod tests;
