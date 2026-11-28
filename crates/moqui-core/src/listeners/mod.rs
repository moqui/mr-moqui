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

//! The listener registry as core sees it (plans/Runtime-Config.md §3.1, §8.1): what each listener is doing now, and a
//! reconcile that makes the listeners match the running conf. The registry is the host's (`moqui_host::listeners`); it
//! installs its control on the factory when its listeners start, as the web protection view does
//! (`crate::web::protection_view`), so the services in core and devkit reach it without depending on the host. A
//! factory whose host started no listener has none. Per factory, not per process: tests run several servers in one
//! process.
//!
//! [`SWITCHES`] names each listener's switch and the settings that turned it on before it had one (§3.2), for the boot's
//! upgrade warning ([`upgrade_warnings`], §8.2), the test server's ports and the readers that list them.
//!
//! [`rows`] and [`change`] (`manage`) list the listeners with the settings behind each, and switch one as a person asks,
//! for `org.moqui.admin.ListenerServices` and the operator socket (§3.6).

use std::cell::OnceCell;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::node::MNode;

use crate::context::conf_env::{self, Override};
use crate::context::{ConfFile, ConfRole, ExecutionContextFactory};
use crate::startup::State;

mod manage;
pub use manage::{
    audit_refused, change, origin, rows, Actor, Answer, Applied, Change, OwnSetting, Refusal, RefusalKind, SettingSet,
    OWN_SETTINGS, WAYS_IN,
};

/// A listener's switch and the settings that turned it on before it had one (plans/Runtime-Config.md §3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerSwitch {
    /// Its row's name in the startup report.
    pub name: &'static str,
    /// Its switch's selector.
    pub enabled: &'static str,
    /// Its ports' selectors, with XMPP's domain, which counts with its ports: each turned the listener on while set.
    pub ports: &'static [&'static str],
}

impl ListenerSwitch {
    /// Its port's selector: the first of [`Self::ports`] that is a port and not a TLS one (XMPP's first is its domain).
    #[must_use]
    pub fn port(&self) -> Option<&'static str> {
        self.ports
            .iter()
            .copied()
            .find(|selector| selector.ends_with("port") && !selector.ends_with("tls-port"))
    }
}

/// The web listeners' entries of [`SWITCHES`], by name for the host's web listeners, whose rows and settings they name.
pub const HTTP_SWITCH: ListenerSwitch = ListenerSwitch {
    name: "HTTP",
    enabled: "webapp-list/webapp[webroot]@http-listen-enabled",
    ports: &["webapp-list/webapp[webroot]@http-listen-port"],
};
pub const HTTPS_SWITCH: ListenerSwitch = ListenerSwitch {
    name: "HTTPS",
    enabled: "webapp-list/webapp[webroot]@https-listen-enabled",
    ports: &["webapp-list/webapp[webroot]@https-listen-port"],
};
pub const GRPC_SWITCH: ListenerSwitch = ListenerSwitch {
    name: "gRPC",
    enabled: "webapp-list/webapp[webroot]@grpc-listen-enabled",
    ports: &["webapp-list/webapp[webroot]@grpc-listen-port"],
};
/// The SSH server's entry of [`SWITCHES`], by name for the host's SSH sessions, which say they came over it ([`origin`]).
pub const SSH_SWITCH: ListenerSwitch = ListenerSwitch {
    name: "SSH",
    enabled: "ssh-server@enabled",
    ports: &["ssh-server@port"],
};

/// The listeners with a switch, in the startup report's order.
pub const SWITCHES: [ListenerSwitch; 9] = [
    HTTP_SWITCH,
    HTTPS_SWITCH,
    GRPC_SWITCH,
    SSH_SWITCH,
    ListenerSwitch {
        name: "SMTP",
        enabled: "smtp-server@enabled",
        ports: &["smtp-server@listen-port"],
    },
    ListenerSwitch {
        name: "XMPP",
        enabled: "xmpp-server@enabled",
        ports: &["xmpp-server@domain", "xmpp-server@port", "xmpp-server@tls-port"],
    },
    ListenerSwitch {
        name: "SIP",
        enabled: "sip-server@enabled",
        ports: &["sip-server@port", "sip-server@tls-port"],
    },
    ListenerSwitch {
        name: "Call media",
        enabled: "media-server@enabled",
        ports: &["media-server@port"],
    },
    ListenerSwitch {
        name: "TURN",
        enabled: "media-server/turn-server@enabled",
        ports: &[
            "media-server/turn-server@port",
            "media-server/turn-server@tls-port",
            "media-server/turn-server@tls443-host",
        ],
    },
];

/// The entry of [`SWITCHES`] for the listener row `name`.
#[must_use]
pub fn switch(name: &str) -> Option<&'static ListenerSwitch> {
    SWITCHES.iter().find(|entry| entry.name == name)
}

/// A server's listeners, as the host's registry controls them.
pub trait ListenerControl: Send + Sync {
    /// Each listener in the registry's order, from its last published state: never waits on a reconcile in progress.
    fn listeners(&self) -> Vec<ListenerInfo>;
    /// Reconcile the listeners with the running conf and wait up to `wait` for what it did. The reconcile runs on the
    /// registry's own thread, merged with any other waiting there; Err when it did not finish within `wait` (it goes
    /// on) or the registry has stopped.
    fn reconcile(&self, request: ReconcileRequest, wait: Duration) -> Result<Vec<Outcome>, String>;
    /// Reconcile after `delay`, without waiting: a change that closes the connection its request came over is
    /// answered first and applied a little later (plans/Runtime-Config.md §3.6).
    fn reconcile_later(&self, request: ReconcileRequest, delay: Duration);
    /// Whether the listener `name` could bind `port` now, on `host` or, with none, where it binds now (its running
    /// address, else its conf's): the services' check before a change that binds a new port (§3.6). Err says why not:
    /// the port, the setting, the host, the error and the processes holding it.
    fn check_port(&self, name: &str, host: Option<&str>, port: u16) -> Result<(), String>;
    /// The settings a server started on an address holds at that address's until they are set (HTTP's switch, port and
    /// address): a change that sets one to the value the conf has still writes it, so the hold is released. None by
    /// default.
    fn held(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Why a reconcile ran: recorded on each listener row it changed, beside the time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// A staged pass of the server's start.
    Boot,
    /// The server is stopping.
    Shutdown,
    /// A conf reload changed a listener's settings.
    ConfReload,
    /// One setting set while running (`set_conf_value`), by its selector.
    SetValue(String),
    /// A signal other than the conf: a cluster member appeared, ACME wants the HTTPS listener.
    Signal(&'static str),
    /// A person, from the services or the operator socket, with the reason they gave and the settings their change set
    /// (which a server started on an address no longer holds at that address's, as a set's).
    Request {
        user: String,
        reason: String,
        selectors: Vec<String>,
    },
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boot => f.write_str("boot"),
            Self::Shutdown => f.write_str("shutdown"),
            Self::ConfReload => f.write_str("conf reload"),
            Self::SetValue(selector) => write!(f, "set {selector}"),
            Self::Signal(signal) => write!(f, "signal {signal}"),
            Self::Request { user, reason, .. } if reason.trim().is_empty() => write!(f, "request by {user}"),
            Self::Request { user, reason, .. } => write!(f, "request by {user}: {reason}"),
        }
    }
}

/// One listener as the registry last published it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerInfo {
    /// The name in the startup report (`SSH`, `HTTPS`).
    pub name: String,
    /// The state of its startup report row.
    pub state: State,
    /// When that state was recorded; none before the listener's first pass.
    pub since: Option<jiff::Timestamp>,
    /// What caused it; none for a state recorded with no cause.
    pub cause: Option<String>,
    /// The address it is bound to, while it runs.
    pub address: Option<String>,
    /// Whether a reconcile after the start changes it; until its phase makes it live its changes wait for a restart.
    pub live: bool,
    /// What the running conf asks of a listener that is not live, which waits for a restart.
    pub pending: Option<String>,
    /// Why its last start or change failed, until one succeeds.
    pub last_error: Option<String>,
    /// The connections it holds, when it counts them.
    pub connections: Option<usize>,
    /// What depends on it while it runs (the edge SSH tunnel, the cluster's SSH transport).
    pub dependents: Vec<String>,
    /// The conf settings it reads (`ssh-server@port`).
    pub selectors: Vec<String>,
}

impl ListenerInfo {
    /// The listener as the services answer it: `name`, `state` (`on`, `off`, `failed`, `skipped`, or null for what
    /// the build leaves out), `detail`, `since` (RFC 3339 or null), `cause`, `address`, `live`, `pending`,
    /// `lastError`, `connections`, `dependents` and `selectors`.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "state": self.state.word(),
            "detail": self.state.detail(),
            "since": self.since.map(|since| since.to_string()),
            "cause": self.cause,
            "address": self.address,
            "live": self.live,
            "pending": self.pending,
            "lastError": self.last_error,
            "connections": self.connections,
            "dependents": self.dependents,
            "selectors": self.selectors,
        })
    }
}

/// What a reconcile is asked to do beyond matching the conf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileRequest {
    pub cause: Cause,
    /// The listeners whose connections end at once when they stop, with no grace ("End connections now").
    pub end_now: Vec<String>,
}

impl ReconcileRequest {
    /// A request for `cause` that ends no connection early.
    #[must_use]
    pub fn new(cause: Cause) -> Self {
        Self {
            cause,
            end_now: Vec::new(),
        }
    }
}

/// What a reconcile did to one listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing: it already matched.
    Same,
    Started,
    Stopped,
    /// Bound again with new address settings.
    Rebound,
    /// Took new settings in place, without a new bind.
    Applied,
    /// Not live yet, or the server is stopping: the change waits for a restart.
    Held,
    /// A start or change failed; the detail says what kept running.
    Failed,
}

impl Action {
    /// `same`, `started`, `stopped`, `rebound`, `applied`, `held` or `failed`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Rebound => "rebound",
            Self::Applied => "applied",
            Self::Held => "held",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

/// One listener's part of a reconcile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub name: String,
    pub action: Action,
    /// The address, the reason or the error.
    pub detail: String,
}

#[derive(Default)]
struct Slot(Mutex<Option<Arc<dyn ListenerControl>>>);

/// Make `control` `factory`'s listener control.
pub fn install(factory: &ExecutionContextFactory, control: Arc<dyn ListenerControl>) {
    *moqui_lib::sync::lock(&factory.extension(|_| Slot::default()).0) = Some(control);
}

/// `factory`'s listener control, once its host's listeners started.
pub fn control(factory: &ExecutionContextFactory) -> Option<Arc<dyn ListenerControl>> {
    factory
        .extension_if_present::<Slot>()
        .and_then(|slot| moqui_lib::sync::lock(&slot.0).clone())
}

// The upgrade warning (plans/Runtime-Config.md §8.2). Remove it after one release: by then a deployment that set a
// port, or XMPP's domain, to turn a listener on has been told to set its switch.

/// One listener's upgrade warning: its row's name, and what was set where with the switch to set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpgradeWarning {
    pub name: &'static str,
    pub message: String,
}

/// The boot's upgrade warnings, on the factory ([`keep_upgrade_warnings`]); a switch set live takes its listener's out
/// ([`switch_set_live`]).
struct UpgradeWarnings(Mutex<Vec<UpgradeWarning>>);

/// The upgrade warnings of a boot's merged `conf`: for each listener of [`SWITCHES`] whose switch is off though the
/// deployment set one of its ports (or XMPP's domain) without setting the switch the same way, what was set where and
/// the switch to set. By source: a port an environment name set (a property, the OS environment or `.env`,
/// `overrides`) is excused only by a switch an environment name set too, since that is how a deployment turned a
/// listener on before; a port the conf file in effect writes (the last of `files` other than the default conf, in merge
/// order, that writes it at all) is excused by a switch any of those files or an environment name sets, as the shipped
/// topology confs write one beside their ports. So an edge node that turned mail on by `.env` with the shipped conf's
/// `enabled="false"` is warned, and a fresh one is not. An empty value turned nothing on, so it is not counted. A live
/// turn-off writes the switch false, so it never warns. A file's place is its path from `base`. A file that does not
/// parse is logged and left out.
#[must_use]
pub fn upgrade_warnings(conf: &MNode, overrides: &[Override], files: &[ConfFile], base: &Path) -> Vec<UpgradeWarning> {
    // every boot with a switch off reads its own conf files again here (not the default conf): a few small files, about
    // a millisecond or two
    let parsed: OnceCell<Vec<(String, MNode)>> = OnceCell::new();
    let own_files = || {
        parsed.get_or_init(|| {
            files
                .iter()
                .filter(|file| file.role != ConfRole::Default)
                .filter_map(|file| match MNode::parse_file(&file.path) {
                    Ok(node) => {
                        let place = file.path.strip_prefix(base).unwrap_or(&file.path);
                        Some((place.display().to_string(), node))
                    }
                    Err(err) => {
                        tracing::error!("The upgrade check could not read {}: {err}", file.path.display());
                        None
                    }
                })
                .collect()
        })
    };
    let named = |selector: &str| {
        let key = moqui_lib::node::path::AttrPath::parse(selector)
            .ok()
            .map(|path| conf_env::compare_key(&path));
        overrides.iter().find(|item| {
            item.applied()
                && item
                    .selector
                    .as_deref()
                    .and_then(|named| moqui_lib::node::path::AttrPath::parse(named).ok())
                    .is_some_and(|named| Some(conf_env::compare_key(&named)) == key)
        })
    };
    // the value the conf file in effect writes for `selector`: the last file that writes it at all, empty or not
    let in_files = |selector: &str| {
        own_files()
            .iter()
            .rev()
            .find_map(|(place, node)| attribute_in(node, selector).map(|value| (place.clone(), value)))
    };
    let mut warnings = Vec::new();
    for entry in &SWITCHES {
        if conf_env::switch_on(&conf_env::conf_attribute(conf, entry.enabled)) {
            continue;
        }
        let switch_by_name = named(entry.enabled).is_some();
        let switch_in_file = || in_files(entry.enabled).is_some();
        let set = entry.ports.iter().find_map(|selector| {
            if let Some(item) = named(selector) {
                return (!switch_by_name).then(|| (*selector, item.source.name().to_string()));
            }
            in_files(selector)
                .filter(|(_, value)| !value.trim().is_empty())
                .filter(|_| !switch_by_name && !switch_in_file())
                .map(|(place, _)| (*selector, place))
        });
        if let Some((selector, place)) = set {
            warnings.push(UpgradeWarning {
                name: entry.name,
                message: format!(
                    "{selector} {} is set ({place}) but {enabled} is false: {name} is off; set {env}=true, or {enabled} \
                     false to keep it off",
                    conf_env::conf_attribute(conf, selector),
                    enabled = entry.enabled,
                    name = entry.name,
                    env = conf_env::env_name_of(entry.enabled),
                ),
            });
        }
    }
    warnings
}

/// The attribute at `selector` in one conf file's `node`, written there or not.
fn attribute_in(node: &MNode, selector: &str) -> Option<String> {
    let path = moqui_lib::node::path::AttrPath::parse(selector).ok()?;
    let target = moqui_lib::node::path::resolve(node, &path, &crate::context::conf_schema::keying, false).ok()?;
    target.node.attribute(&target.attribute)
}

/// Log each of `warnings` and keep them on `factory` for the listeners' Off rows ([`upgrade_warning`]); the boot calls
/// it once.
pub(crate) fn keep_upgrade_warnings(factory: &ExecutionContextFactory, warnings: Vec<UpgradeWarning>) {
    for warning in &warnings {
        tracing::warn!("{}", warning.message);
    }
    factory.extension(move |_| UpgradeWarnings(Mutex::new(warnings)));
}

/// The boot's upgrade warning for the listener row `name`, if any: its Off row says it ([`crate::startup::listener`]).
#[must_use]
pub fn upgrade_warning(factory: &ExecutionContextFactory, name: &str) -> Option<String> {
    let kept = factory.extension_if_present::<UpgradeWarnings>()?;
    let warnings = moqui_lib::sync::lock(&kept.0);
    warnings
        .iter()
        .find(|warning| warning.name == name)
        .map(|warning| warning.message.clone())
}

/// The deployment set the attribute at `selector` live ([`crate::context::conf_reload::set`]): when it is a listener's
/// switch of [`SWITCHES`], the deployment has acted on that switch, so the boot's upgrade warning no longer applies to it
/// and its rows from now on leave it out.
pub(crate) fn switch_set_live(factory: &ExecutionContextFactory, selector: &str) {
    let key = |text: &str| {
        moqui_lib::node::path::AttrPath::parse(text)
            .ok()
            .map(|path| conf_env::compare_key(&path))
    };
    let Some(set) = key(selector) else {
        return;
    };
    let Some(entry) = SWITCHES.iter().find(|entry| key(entry.enabled).as_ref() == Some(&set)) else {
        return;
    };
    if let Some(kept) = factory.extension_if_present::<UpgradeWarnings>() {
        moqui_lib::sync::lock(&kept.0).retain(|warning| warning.name != entry.name);
    }
}

/// The Off reason of the listener row `name` with its upgrade warning: the warning in place of the reason that says only
/// its switch is false, else after the reason.
#[must_use]
pub fn off_reason(factory: &ExecutionContextFactory, name: &str, reason: String) -> String {
    match upgrade_warning(factory, name) {
        Some(warning) if switch(name).is_some_and(|entry| reason == format!("{} is false", entry.enabled)) => warning,
        Some(warning) => format!("{reason}; {warning}"),
        None => reason,
    }
}

#[cfg(test)]
mod tests;
