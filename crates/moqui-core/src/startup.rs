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

//! What each listener and integration did at startup, for the one report the host logs when the server is ready
//! (`moqui_host::startup_report`). A subsystem records its state here instead of logging its own "started" or "not
//! starting" line; its warnings and errors are still logged where they happen. The report is a factory extension, so
//! the second factory after an empty-database load starts with an empty one.
//!
//! Scripts read the same report as `ec.factory.getStartupReport()` ([`script_value`], plans/archive/Tools-Fancy.md
//! B1): the host records the routes and the ready time when it logs ([`StartupReport::record_ready`]).

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{Value, ValueMap};
use moqui_lib::sync::lock;

use crate::context::ExecutionContextFactory;

/// Where an entry goes in the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// A port this process opens.
    Listener,
    /// A tool, interface or integration that is on or off.
    Integration,
}

/// What became of a listener or integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Running; for a listener, the protocol and the bound address, then any detail.
    On(String),
    /// Not configured to run: the setting that keeps it off.
    Off(String),
    /// Configured but did not start: a short reason (the error itself is logged where it happened).
    Failed(String),
    /// Configured but not tried: its port was busy at the start check.
    Skipped(String),
    /// This build leaves it out.
    NotBuilt,
}

impl State {
    /// `on: <detail>` for an integration that is running.
    pub fn on(detail: impl Into<String>) -> Self {
        Self::On(detail.into())
    }

    /// `off: <reason>`.
    pub fn off(reason: impl Into<String>) -> Self {
        Self::Off(reason.into())
    }

    /// `failed: <reason>`.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::Failed(reason.into())
    }
}

impl State {
    /// The state as scripts read it: `on`, `off`, `failed` or `skipped`; `None` for what the build leaves out, which
    /// is listed apart.
    #[must_use]
    pub fn word(&self) -> Option<&'static str> {
        match self {
            Self::On(_) => Some("on"),
            Self::Off(_) => Some("off"),
            Self::Failed(_) => Some("failed"),
            Self::Skipped(_) => Some("skipped"),
            Self::NotBuilt => None,
        }
    }

    /// The address, detail or reason; empty for [`State::NotBuilt`].
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::On(detail) | Self::Off(detail) | Self::Failed(detail) | Self::Skipped(detail) => detail,
            Self::NotBuilt => "",
        }
    }
}

/// The listeners in the order the report shows them; one not named here comes after them.
pub const LISTENER_ORDER: [&str; 11] = [
    "HTTP",
    "HTTPS",
    "Cluster",
    "gRPC",
    "SSH",
    "SMTP",
    "Call media",
    "TURN",
    "SIP",
    "XMPP",
    "Edge tunnels",
];

/// What the host knew when it logged the report.
#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    /// Seconds from the process's start to the report.
    pub seconds: f64,
    /// The HTTP listener's routes, as the log names them (comma separated); empty when there is none.
    pub routes: String,
    /// False for a boot that opens no listener (`moqui tui`): the listener entries are then left out.
    pub listeners: bool,
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub section: Section,
    pub name: String,
    pub state: State,
}

/// The entries recorded so far, in the order they were first recorded.
#[derive(Default)]
pub struct StartupReport {
    entries: Mutex<Vec<Entry>>,
    ready: Mutex<Option<Ready>>,
}

impl StartupReport {
    /// Record `name`'s state. A second record of the same name in the same section replaces the first, so a
    /// subsystem can record `On` and later `Failed`.
    pub fn record(&self, section: Section, name: impl Into<String>, state: State) {
        let name = name.into();
        let mut entries = lock(&self.entries);
        match entries
            .iter_mut()
            .find(|entry| entry.section == section && entry.name == name)
        {
            Some(entry) => entry.state = state,
            None => entries.push(Entry { section, name, state }),
        }
    }

    /// Record `name` only when nothing has been recorded for it yet.
    pub fn record_default(&self, section: Section, name: impl Into<String>, state: State) {
        let name = name.into();
        let mut entries = lock(&self.entries);
        if !entries
            .iter()
            .any(|entry| entry.section == section && entry.name == name)
        {
            entries.push(Entry { section, name, state });
        }
    }

    /// A copy of the entries.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        lock(&self.entries).clone()
    }

    /// Record what the host knew when it logged the report. A second record replaces the first.
    pub fn record_ready(&self, ready: Ready) {
        *lock(&self.ready) = Some(ready);
    }

    /// What the host recorded when it logged the report; `None` before it has.
    #[must_use]
    pub fn ready(&self) -> Option<Ready> {
        lock(&self.ready).clone()
    }

    /// The listeners (in [`LISTENER_ORDER`]) and the integrations (by name) this build has, and the names of what
    /// it leaves out: the three lists of the log's report.
    #[must_use]
    pub fn sections(&self) -> Sections {
        let entries = self.entries();
        let listeners_shown = self.ready().is_none_or(|ready| ready.listeners);
        let of = |section: Section| -> Vec<Entry> {
            entries
                .iter()
                .filter(|entry| entry.section == section && entry.state != State::NotBuilt)
                .cloned()
                .collect()
        };
        let mut listeners = if listeners_shown { of(Section::Listener) } else { Vec::new() };
        listeners.sort_by_key(|entry| {
            LISTENER_ORDER
                .iter()
                .position(|name| *name == entry.name)
                .unwrap_or(LISTENER_ORDER.len())
        });
        let mut integrations = of(Section::Integration);
        integrations.sort_by_key(|entry| entry.name.to_lowercase());
        let mut not_built: Vec<String> = entries
            .iter()
            .filter(|entry| entry.state == State::NotBuilt)
            .filter(|entry| listeners_shown || entry.section != Section::Listener)
            .map(|entry| entry.name.clone())
            .collect();
        not_built.sort_unstable();
        not_built.dedup();
        Sections {
            listeners,
            integrations,
            not_built,
        }
    }

    /// The report as a script reads it (`ec.factory.getStartupReport()`): `version`, `package`, `purpose`, `role`,
    /// `readySeconds` (null until the host logged the report), `listeners` and `integrations` (each `{name, state,
    /// detail}`, state one of `on`, `off`, `failed`, `skipped`), `tools`, `notBuilt` and `routes` (a list).
    #[must_use]
    pub fn to_value(&self, identity: &Identity, tools: &[String]) -> ValueMap {
        let sections = self.sections();
        let rows = |entries: &[Entry]| {
            Value::List(
                entries
                    .iter()
                    .map(|entry| {
                        Value::Map(IndexMap::from_iter([
                            ("name".to_string(), Value::string(&entry.name)),
                            ("state".to_string(), entry.state.word().map_or(Value::Null, Value::string)),
                            ("detail".to_string(), Value::string(entry.state.detail())),
                        ]))
                    })
                    .collect(),
            )
        };
        let texts = |names: &[String]| Value::List(names.iter().map(Value::string).collect());
        let ready = self.ready();
        let routes: Vec<String> = ready
            .as_ref()
            .map(|ready| {
                ready
                    .routes
                    .split(',')
                    .map(|route| route.trim().to_string())
                    .filter(|route| !route.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        IndexMap::from_iter([
            ("version".to_string(), Value::string(&identity.version)),
            ("package".to_string(), Value::string(&identity.package)),
            ("purpose".to_string(), Value::string(&identity.purpose)),
            ("role".to_string(), Value::string(&identity.role)),
            (
                "readySeconds".to_string(),
                ready.map_or(Value::Null, |ready| Value::F64((ready.seconds * 1000.0).round() / 1000.0)),
            ),
            ("listeners".to_string(), rows(&sections.listeners)),
            ("integrations".to_string(), rows(&sections.integrations)),
            ("tools".to_string(), texts(tools)),
            ("notBuilt".to_string(), texts(&sections.not_built)),
            ("routes".to_string(), texts(&routes)),
        ])
    }
}

/// The report's entries as its three lists ([`StartupReport::sections`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sections {
    pub listeners: Vec<Entry>,
    pub integrations: Vec<Entry>,
    pub not_built: Vec<String>,
}

/// What the report's first line says of this process.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub version: String,
    /// The build package (`crate::build_info::package`); empty before the host registers.
    pub package: String,
    /// `instance_purpose`; empty when unset.
    pub purpose: String,
    /// `node_role`; empty when unset, which is every role.
    pub role: String,
}

impl Identity {
    /// This process's: the framework version, the host's package and the `instance_purpose` and `node_role`
    /// properties.
    #[must_use]
    pub fn current() -> Self {
        let property = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            package: crate::build_info::package().to_string(),
            purpose: property("instance_purpose"),
            role: property("node_role"),
        }
    }
}

/// `ec.factory.getStartupReport()`: the factory's report as a script value ([`StartupReport::to_value`]).
#[must_use]
pub fn script_value(factory: &ExecutionContextFactory) -> ValueMap {
    report(factory).to_value(&Identity::current(), &factory.tool_names())
}

/// The factory's report.
pub fn report(factory: &ExecutionContextFactory) -> Arc<StartupReport> {
    factory.extension(|_| StartupReport::default())
}

/// Record an integration's state in the factory's report.
pub fn integration(factory: &ExecutionContextFactory, name: impl Into<String>, state: State) {
    report(factory).record(Section::Integration, name, state);
}

/// Record a listener's state in the factory's report.
pub fn listener(factory: &ExecutionContextFactory, name: impl Into<String>, state: State) {
    report(factory).record(Section::Listener, name, state);
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
