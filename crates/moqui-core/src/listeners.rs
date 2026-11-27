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

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::context::ExecutionContextFactory;
use crate::startup::State;

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
    /// A person, from the services or the operator socket, with the reason they gave.
    Request { user: String, reason: String },
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boot => f.write_str("boot"),
            Self::Shutdown => f.write_str("shutdown"),
            Self::ConfReload => f.write_str("conf reload"),
            Self::SetValue(selector) => write!(f, "set {selector}"),
            Self::Signal(signal) => write!(f, "signal {signal}"),
            Self::Request { user, reason } if reason.trim().is_empty() => write!(f, "request by {user}"),
            Self::Request { user, reason } => write!(f, "request by {user}: {reason}"),
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

#[cfg(test)]
#[path = "listeners_tests.rs"]
mod tests;
