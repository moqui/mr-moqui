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

//! The listener registry (plans/Runtime-Config.md §3.1, §8.1): one entry for each listener, in a fixed order, and one
//! reconcile that compares what the running conf wants of each with what runs and starts, stops, binds again or applies
//! in place to match. The boot runs it in four staged passes ([`Stage`]); after the start a conf reload, a single
//! setting, a signal or a person asks for a pass through [`Registry::request`], which the `moqui-listeners` thread runs
//! ([`worker`]); the process stop stops each entry in its [`StopPhase`], in its turn ([`Listener::stop_rank`]). Each
//! action records the listener's startup report row with the time and the cause, so System Home shows the state now.
//!
//! A pass runs only on a plain thread with no ExecutionContext entered (the boot thread, a `spawn_blocking` thread of
//! `http::listen`, the worker): a listener's start may bind on a runtime ([`worker::on_runtime`]) and some listeners'
//! drops block or drop their own runtimes, which stalls or panics on an async thread. Core reaches the registry through
//! its `moqui_core::listeners` slot ([`control`]).

mod cluster;
mod comms;
pub(crate) mod control;
mod entry;
#[cfg(feature = "ssh")]
mod ssh;
mod web;
pub(crate) mod worker;

use std::fmt::Debug;
use std::net::SocketAddr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::listeners::{Action, Cause, ListenerInfo, Outcome, ReconcileRequest};
use moqui_core::startup::{Section, State};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;

use crate::port_check::Target;
use crate::ListenBind;

pub(crate) use cluster::NAME as CLUSTER;
use entry::{Entry, Slot, Stopping};
pub(crate) use web::{HTTP, HTTPS};

/// How long a listener turned off while the server runs gives the requests in flight before its connections end; the
/// process stop keeps `web-protection/connections@shutdown-timeout` (plans/Runtime-Config.md §6).
pub(crate) const LIVE_STOP_GRACE: Duration = Duration::from_secs(5);

/// The boot pass a listener starts in: other boot work sits between them (plans/Runtime-Config.md §3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Before the cluster starts: SSH, whose address and fingerprint this member's row carries.
    BeforeCluster,
    /// After `cluster::start`: the cluster port.
    ClusterPort,
    /// Call media, SIP, XMPP and SMTP, after the cluster so they register under its instance id.
    Comms,
    /// HTTP, HTTPS and gRPC, once `http::listen` has built the router.
    Web,
}

/// Where in the process stop a listener stops, in today's order: the web listeners first, then the ones that must stop
/// before the factory, then the rest after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopPhase {
    /// HTTP, HTTPS and gRPC, before the axum runtime stops.
    Web,
    /// XMPP, SIP and call media (its last act deregisters the node).
    BeforeFactory,
    /// SMTP, SSH and the cluster port, after the factory stops.
    AfterFactory,
}

/// The entries a pass covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The entries of one boot stage.
    Stage(Stage),
    /// Every entry.
    All,
}

impl Scope {
    fn covers(self, stage: Stage) -> bool {
        match self {
            Self::Stage(only) => only == stage,
            Self::All => true,
        }
    }
}

/// How a running listener stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopHow {
    /// The process is stopping: the listener's own shutdown, as today.
    Process,
    /// Turned off while running: no new connections, `Duration` for the requests in flight, then its connections end.
    Grace(Duration),
    /// Turned off with "End connections now": its connections end at once.
    Now,
}

/// What the running conf wants of a listener.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Wanted<B, P> {
    /// Running with these settings: `bind` needs a new bind when it changes, `in_place` applies to what runs.
    On { bind: B, in_place: P },
    /// Off, with the setting that keeps it off.
    Off(String),
    /// Configured but not tried: its port was busy at the start check. RC0's wrapped starts record it themselves.
    #[cfg_attr(not(test), allow(dead_code))]
    Skipped(String),
    /// Not yet: a web listener before the router exists.
    NotYet,
}

/// A listener the registry starts and stops (plans/Runtime-Config.md §8.1). Its settings are split into what needs a
/// new bind ([`Listener::Bind`]: address, port, TLS files, protocol) and what applies in place ([`Listener::InPlace`]:
/// limits, access policy), each one comparable value.
pub(crate) trait Listener: Send + 'static {
    type Bind: Clone + PartialEq + Debug + Send;
    type InPlace: Clone + PartialEq + Debug + Send;
    type Run: Running<Self::InPlace> + 'static;

    /// Its name in the startup report (`SSH`, `HTTPS`).
    fn name(&self) -> &'static str;
    fn stage(&self) -> Stage;
    fn stop_phase(&self) -> StopPhase;
    /// Whether a pass after the start changes it. Until its phase makes it live its changes wait for a restart: a pass
    /// only notes them as pending.
    fn live(&self) -> bool {
        false
    }
    /// Whether its failed start at boot stops the boot (SSH and the cluster port until RC3 and RC6).
    fn fatal_at_boot(&self) -> bool {
        false
    }
    /// Its turn within its stop phase: a lower rank finishes stopping before a higher rank begins, and the listeners
    /// of one rank stop together, in the order they were added. Today's order is XMPP, SIP, then call media (SIP's
    /// legs are in call media) before the factory, and SMTP, SSH, then the cluster port after it; the web listeners
    /// stop together.
    fn stop_rank(&self) -> u8 {
        0
    }
    /// The startup report rows its own start records beside its row (call media records `TURN`): a stop records them
    /// off with its own, and each change sets their cause.
    fn extra_rows(&self) -> &'static [&'static str] {
        &[]
    }
    /// The conf settings it reads.
    fn selectors(&self) -> &'static [&'static str];
    /// What the running conf wants of it now.
    fn wanted(&self, ctx: &Ctx) -> Wanted<Self::Bind, Self::InPlace>;
    /// Bind and start serving. `Ok(None)`: a wrapped start of today found it off and recorded why itself (the RC0
    /// wrappers only). A failed bind is explained with the port check's text (`port_check::explain`); the error of one
    /// that is fatal at boot is the boot's own.
    fn start(&self, ctx: &Ctx, bind: &Self::Bind, in_place: &Self::InPlace) -> Result<Option<Self::Run>, Error>;
    /// Whether `new` would collide with `old` while both are bound (the same port, a wildcard and a specific address),
    /// so the old must stop before the new binds.
    fn overlaps(&self, old: &Self::Bind, new: &Self::Bind) -> bool;
    /// What depends on it while it runs (the edge SSH tunnel, the cluster's SSH transport).
    fn dependents(&self, _ctx: &Ctx) -> Vec<String> {
        Vec::new()
    }
    /// The port `bind` takes, for the pre-check of a change and to name a pending bind.
    fn target(&self, bind: &Self::Bind) -> Option<Target>;
}

/// A started listener.
pub(crate) trait Running<P>: Send {
    /// Its state for the startup report row: `On` with the protocol and the bound address.
    fn state(&self) -> State;
    fn address(&self) -> Option<SocketAddr>;
    /// Take new in-place settings; new connections use them.
    fn apply(&mut self, _in_place: &P) {}
    /// Begin to stop: no new connections, and the connections end as `how` says. Never waits out the grace.
    fn begin_stop(&mut self, how: StopHow);
    /// Wait up to `wait` for the stop to finish; false when it did not, and dropping it ends what is left.
    fn finished(&mut self, wait: Duration) -> bool;
    /// The connections it holds, when it counts them.
    fn connections(&self) -> Option<usize> {
        None
    }
    /// Whether it records its own startup report row as its state changes (HTTPS standing by, which binds and closes
    /// as ACME wants it): the registry then sets only the row's cause and never records [`Running::state`] over it.
    fn records_own_row(&self) -> bool {
        false
    }
}

/// A listener of today wrapped for the registry (RC0, plans/Runtime-Config.md §8.1): the handle its start returned,
/// whose drop stops it as it did before the registry, the state its row says, and the address it bound. Its stop
/// begins and finishes in that drop, so the registry's phase stop drops it in its turn.
pub(crate) struct Wrapped<T: Send> {
    _handle: T,
    state: State,
    address: Option<SocketAddr>,
}

impl<T: Send> Wrapped<T> {
    pub(crate) fn new(handle: T, state: State, address: Option<SocketAddr>) -> Self {
        Self {
            _handle: handle,
            state,
            address,
        }
    }
}

impl<T: Send, P> Running<P> for Wrapped<T> {
    fn state(&self) -> State {
        self.state.clone()
    }

    fn address(&self) -> Option<SocketAddr> {
        self.address
    }

    fn begin_stop(&mut self, _how: StopHow) {}

    fn finished(&mut self, _wait: Duration) -> bool {
        true
    }
}

/// The entries the boot starts through the registry, in their fixed order: SSH, the cluster port, call media (with
/// TURN), SIP, XMPP and SMTP in the order they started before the registry, then HTTP, HTTPS and gRPC. The edge conf
/// tunnels stay inside `edge_link::start` until RC8.
pub(crate) fn add_boot_entries(registry: &Registry) {
    #[cfg(feature = "ssh")]
    registry.add(ssh::Ssh);
    registry.add(cluster::Cluster);
    registry.add(comms::CallMedia);
    registry.add(comms::Sip);
    registry.add(comms::Xmpp);
    registry.add(comms::Smtp);
    registry.add(web::Http);
    registry.add(web::Https);
    #[cfg(feature = "grpc")]
    registry.add(web::Grpc);
}

/// What a wrapped start of today left on `name`'s row: the state the registry records for it once it runs, so the
/// row reads as it did before the registry.
fn recorded_state(factory: &ExecutionContextFactory, name: &str) -> State {
    let row = moqui_core::startup::report(factory)
        .entries()
        .into_iter()
        .find(|row| row.section == Section::Listener && row.name == name);
    match row {
        Some(row) => row.state,
        None => {
            // each wrapped start records its row before it answers; one that did not is a bug in it
            tracing::error!("The {name} listener started but its start recorded no startup report row");
            State::on("")
        }
    }
}

/// What the boot hands every entry.
pub(crate) struct BootInputs {
    pub(crate) bind: ListenBind,
    /// Read by the SSH server's start, for its SFTP root.
    #[cfg_attr(not(feature = "ssh"), allow(dead_code))]
    pub(crate) runtime_dir: PathBuf,
    /// Topics accepted from cluster peers, which the SSH server and the cluster port both take
    /// (`Server::cluster_posts`).
    pub(crate) cluster_posts: Arc<Mutex<Vec<String>>>,
}

/// What an entry reads while a pass runs. RC0's wrapped starts read their settings as they did before the registry,
/// the process-wide `conf_env::setting` among them; from RC1 each entry reads the factory's running conf (its keyed
/// selector) instead, so two servers in one process do not share a value.
pub(crate) struct Ctx<'a> {
    pub(crate) factory: &'a Arc<ExecutionContextFactory>,
    pub(crate) boot: &'a BootInputs,
    /// What `http::listen` built for the web listeners, once it has ([`Registry::install_web`]).
    pub(crate) web: Option<&'a crate::http::WebContext>,
    /// The first cause of the pass; a pass of the worker may merge several ([`Pass::cause`]). gRPC logs its h2c
    /// refusal only at boot; from RC1 an entry reads the start check's busy list only for [`Cause::Boot`].
    #[cfg_attr(not(feature = "grpc"), allow(dead_code))]
    pub(crate) cause: &'a Cause,
}

/// How one pass acts, the same for each entry.
pub(crate) struct Pass<'a> {
    /// A boot pass: every entry acts, live or not, and every row is recorded.
    boot: bool,
    /// False once the registry is shutting down: nothing starts.
    may_start: bool,
    /// The listeners whose connections end at once when they stop.
    end_now: &'a [String],
    /// The text recorded as each changed row's cause: the pass's causes, joined.
    cause: String,
}

/// One entry as last published.
#[derive(Clone)]
struct Published {
    info: ListenerInfo,
    address: Option<SocketAddr>,
}

/// The registry: a handle that clones share. [`Registry::close`] ends it; until then its entries own what runs.
#[derive(Clone)]
pub(crate) struct Registry(Arc<Inner>);

struct Inner {
    factory: Arc<ExecutionContextFactory>,
    boot: BootInputs,
    /// The one lock, held for a whole pass or stop: nothing starts or stops while another pass runs.
    entries: Mutex<Vec<Box<dyn Entry>>>,
    /// Each entry's state after its last action, index for index with `entries`, so readers never wait on a pass.
    published: Mutex<Vec<Published>>,
    /// Set by [`Registry::stop`] of the web phase under the lock: no pass starts anything after it, so a reload during
    /// the drain cannot start HTTP again.
    shutting_down: AtomicBool,
    worker: Mutex<worker::State>,
    /// What `http::listen` built for the web listeners, set once before the web pass and taken out when the web
    /// phase stops, so it goes with `http::listen` as it did before the registry.
    web: Mutex<Option<Arc<crate::http::WebContext>>>,
}

impl Registry {
    /// An empty registry for `factory`'s listeners; [`Registry::add`] gives it its entries, in order.
    pub(crate) fn new(factory: Arc<ExecutionContextFactory>, boot: BootInputs) -> Self {
        Self(Arc::new(Inner {
            factory,
            boot,
            entries: Mutex::new(Vec::new()),
            published: Mutex::new(Vec::new()),
            shutting_down: AtomicBool::new(false),
            worker: Mutex::new(worker::State::Idle),
            web: Mutex::new(None),
        }))
    }

    /// Give the web entries what `http::listen` built for them, before the web pass; a second is ignored.
    pub(crate) fn install_web(&self, web: crate::http::WebContext) {
        let mut slot = lock(&self.0.web);
        if slot.is_some() {
            tracing::warn!("The web listeners' context was given twice; the first is kept");
        } else {
            *slot = Some(Arc::new(web));
        }
    }

    /// Add `listener` after the entries already added: passes act and stops count from this order.
    pub(crate) fn add<L: Listener>(&self, listener: L) {
        let slot: Box<dyn Entry> = Box::new(Slot::new(listener));
        let mut entries = lock(&self.0.entries);
        let cause = Cause::Boot;
        let web = self.0.web();
        let ctx = self.0.ctx(&cause, web.as_deref());
        lock(&self.0.published).push(Published {
            info: slot.info(&ctx),
            address: None,
        });
        entries.push(slot);
    }

    /// Reconcile the entries `scope` covers, on this plain thread. With [`Cause::Boot`] it is a staged boot pass,
    /// the only kind: it acts on each entry of the stage, live or not, and fails when one that is fatal at boot does
    /// not start (the others still act). Any other pass skips the entries that are not live but for noting what waits
    /// for a restart, and leaves alone those whose boot stage has not run.
    ///
    /// # Errors
    ///
    /// The start error of the first entry that is fatal at boot and did not start, in a boot pass.
    pub(crate) fn run(&self, scope: Scope, cause: Cause) -> Result<Vec<Outcome>, Error> {
        let boot = cause == Cause::Boot;
        let mut entries = lock(&self.0.entries);
        self.0
            .pass(&mut entries, scope, std::slice::from_ref(&cause), &[], boot)
    }

    /// Ask the worker for a pass over every entry after `delay`, merged with whatever else waits; the receiver gets
    /// its outcomes. A boot or shutdown cause is refused (those passes are the server's own); the receiver reads a
    /// disconnect when the worker has stopped or could not start.
    pub(crate) fn request(
        &self,
        request: ReconcileRequest,
        delay: Duration,
    ) -> mpsc::Receiver<Result<Vec<Outcome>, String>> {
        worker::submit(&self.0, request, delay)
    }

    /// Stop the running entries of `phase` in their turns ([`Listener::stop_rank`]): the entries of one rank each
    /// begin to stop before any is waited on, so their graces run together, and are then dropped on this thread in
    /// the order they were added; the next rank begins once they are. `wait` is the whole phase's, one deadline for
    /// every rank: a listener not finished by then is dropped, which ends what is left. The web phase first marks the
    /// registry shutting down, and last lets go of the web listeners' context.
    pub(crate) fn stop(&self, phase: StopPhase, wait: Duration) {
        debug_assert!(!ExecutionContext::is_entered(), "{PLAIN_THREAD}");
        let mut entries = lock(&self.0.entries);
        if phase == StopPhase::Web {
            self.0.shutting_down.store(true, Ordering::SeqCst);
        }
        let deadline = deadline_after(wait);
        let cause = Cause::Shutdown;
        let web = self.0.web();
        let ctx = self.0.ctx(&cause, web.as_deref());
        let pass = Pass {
            boot: false,
            may_start: false,
            end_now: &[],
            cause: cause.to_string(),
        };
        let mut ranked: Vec<(u8, usize)> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.stop_phase() == phase)
            .map(|(index, entry)| (entry.stop_rank(), index))
            .collect();
        ranked.sort_unstable();
        for turn in ranked.chunk_by(|(first, _), (next, _)| first == next) {
            for &(_, index) in turn {
                entries[index].begin_stop(StopHow::Process);
            }
            for &(_, index) in turn {
                let entry = &mut entries[index];
                let finished = catch_unwind(AssertUnwindSafe(|| entry.finish_stop(&ctx, &pass, deadline)));
                if let Err(panic) = finished {
                    tracing::error!(
                        "The {} listener's stop failed, the others go on: {}",
                        entry.name(),
                        panic_text(panic.as_ref())
                    );
                }
                self.0.publish(index, entry.as_ref(), &ctx);
            }
        }
        if phase == StopPhase::Web {
            // the web listeners' context (the app and its factory, the edge and manager acceptors, the runtime's
            // handle) goes now, with `http::listen`, rather than with the registry after the factory's shutdown
            drop(web);
            drop(lock(&self.0.web).take());
        }
    }

    /// Stop the worker, waiting for a pass it is running, so no pass starts after it. The process stop calls it first.
    pub(crate) fn join_worker(&self) {
        worker::join(&self.0);
    }

    /// End the registry: stop the worker, then drop what still runs, last added first, on this thread (a boot that
    /// failed part way, or what the stops left). Nothing starts after it.
    pub(crate) fn close(&self) {
        debug_assert!(!ExecutionContext::is_entered(), "{PLAIN_THREAD}");
        self.join_worker();
        let mut entries = lock(&self.0.entries);
        self.0.shutting_down.store(true, Ordering::SeqCst);
        let cause = Cause::Shutdown;
        let web = self.0.web();
        let ctx = self.0.ctx(&cause, web.as_deref());
        for (index, entry) in entries.iter_mut().enumerate().rev() {
            let dropped = catch_unwind(AssertUnwindSafe(|| drop(entry.take_running())));
            if let Err(panic) = dropped {
                tracing::error!(
                    "The {} listener panicked as it was dropped, the others go on: {}",
                    entry.name(),
                    panic_text(panic.as_ref())
                );
            }
            self.0.publish(index, entry.as_ref(), &ctx);
        }
    }

    /// The address `name` is bound to while it runs, as last published.
    pub(crate) fn address(&self, name: &str) -> Option<SocketAddr> {
        lock(&self.0.published)
            .iter()
            .find(|published| published.info.name == name)
            .and_then(|published| published.address)
    }

    /// Each entry as last published, in order.
    pub(crate) fn snapshot(&self) -> Vec<ListenerInfo> {
        lock(&self.0.published)
            .iter()
            .map(|published| published.info.clone())
            .collect()
    }
}

/// A registry dropped without [`Registry::close`] (a boot that failed part way) drops what still runs last added first,
/// as the locals of the boot it replaces unwound.
impl Drop for Inner {
    fn drop(&mut self) {
        let entries = self
            .entries
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries.iter_mut().rev() {
            let dropped = catch_unwind(AssertUnwindSafe(|| drop(entry.take_running())));
            if let Err(panic) = dropped {
                tracing::error!(
                    "The {} listener panicked as it was dropped, the others go on: {}",
                    entry.name(),
                    panic_text(panic.as_ref())
                );
            }
        }
    }
}

impl Inner {
    fn ctx<'a>(&'a self, cause: &'a Cause, web: Option<&'a crate::http::WebContext>) -> Ctx<'a> {
        Ctx {
            factory: &self.factory,
            boot: &self.boot,
            web,
            cause,
        }
    }

    /// The web listeners' context, while it is installed.
    fn web(&self) -> Option<Arc<crate::http::WebContext>> {
        lock(&self.web).clone()
    }

    /// One pass over `entries` (the lock held by the caller) for `causes`, the first of which the entries read; `boot`
    /// for a staged boot pass ([`Registry::run`]), never for the worker's.
    fn pass(
        &self,
        entries: &mut [Box<dyn Entry>],
        scope: Scope,
        causes: &[Cause],
        end_now: &[String],
        boot: bool,
    ) -> Result<Vec<Outcome>, Error> {
        debug_assert!(!ExecutionContext::is_entered(), "{PLAIN_THREAD}");
        let Some(cause) = causes.first() else {
            return Ok(Vec::new());
        };
        let pass = Pass {
            boot,
            may_start: !self.shutting_down.load(Ordering::SeqCst),
            end_now,
            cause: cause_text(causes),
        };
        let web = self.web();
        let ctx = self.ctx(cause, web.as_deref());
        let mut outcomes = Vec::new();
        let mut fatal = None;
        let mut started = Vec::new();
        let mut stopping: Vec<Stopping> = Vec::new();
        for (index, entry) in entries.iter_mut().enumerate() {
            if !scope.covers(entry.stage()) {
                continue;
            }
            let name = entry.name();
            if !boot && !entry.booted() {
                // a request during the boot: its stage has not run, so it neither starts nor waits for a restart
                outcomes.push(Outcome {
                    name: name.to_string(),
                    action: Action::Same,
                    detail: "waits for its boot stage".to_string(),
                });
                continue;
            }
            let acts = boot || entry.live();
            // the slot catches a panic in each call into its listener; this is the last guard, for the rest of it
            let acted = catch_unwind(AssertUnwindSafe(|| {
                if acts {
                    entry.reconcile(&ctx, &pass, &mut stopping)
                } else {
                    entry.hold(&ctx)
                }
            }));
            let outcome = match acted {
                Ok(outcome) => outcome,
                Err(panic) => {
                    let detail = format!("the registry's handling of it panicked: {}", panic_text(panic.as_ref()));
                    tracing::error!("The {name} listener failed, the others go on: {detail}");
                    // what ran may have gone with the panic: the row, and those it owns, say so rather than on
                    if !entry.is_running() {
                        moqui_core::startup::listener(&self.factory, name, State::Failed(detail.clone()));
                        moqui_core::startup::set_cause(&self.factory, Section::Listener, name, pass.cause.as_str());
                        for extra in entry.extra_rows() {
                            moqui_core::startup::listener(
                                &self.factory,
                                *extra,
                                State::off(format!("{name} is not running")),
                            );
                            moqui_core::startup::set_cause(
                                &self.factory,
                                Section::Listener,
                                extra,
                                pass.cause.as_str(),
                            );
                        }
                    }
                    Outcome {
                        name: name.to_string(),
                        action: Action::Failed,
                        detail,
                    }
                }
            };
            if boot && entry.fatal_at_boot() && outcome.action == Action::Failed {
                fatal = Some(
                    entry
                        .take_boot_error()
                        .unwrap_or_else(|| Error::new(outcome.detail.clone())),
                );
            } else if outcome.action == Action::Started {
                started.push(index);
            }
            self.publish(index, entry.as_ref(), &ctx);
            outcomes.push(outcome);
            if fatal.is_some() {
                // the boot stops here, as its `?` did before the registry: no later entry of the stage starts
                break;
            }
        }
        // every stop of the pass has begun, so their graces run together rather than one after another
        for stop in stopping {
            stop.finish();
        }
        let Some(err) = fatal else {
            return Ok(outcomes);
        };
        // what this pass started stops again before the boot's error goes back (HTTP when HTTPS then failed)
        let deadline = deadline_after(FAILED_BOOT_WAIT);
        for &index in started.iter().rev() {
            entries[index].begin_stop(StopHow::Process);
        }
        for &index in started.iter().rev() {
            let entry = &mut entries[index];
            let finished = catch_unwind(AssertUnwindSafe(|| entry.finish_stop(&ctx, &pass, deadline)));
            if let Err(panic) = finished {
                tracing::error!(
                    "The {} listener's stop failed, the others go on: {}",
                    entry.name(),
                    panic_text(panic.as_ref())
                );
            }
            self.publish(index, entry.as_ref(), &ctx);
        }
        Err(err)
    }

    fn publish(&self, index: usize, entry: &dyn Entry, ctx: &Ctx) {
        let published = Published {
            info: entry.info(ctx),
            address: entry.address(),
        };
        if let Some(slot) = lock(&self.published).get_mut(index) {
            *slot = published;
        }
    }
}

/// How long a boot pass that failed waits for what it started to stop: nothing has taken a connection yet.
const FAILED_BOOT_WAIT: Duration = Duration::from_secs(5);

/// What a pass or stop asserts of its thread (plans/Runtime-Config.md §8.1).
const PLAIN_THREAD: &str =
    "a listener pass or stop runs on a plain thread, and this one has an ExecutionContext entered";

/// `wait` from now, or as far as an instant goes for a wait too long to add.
fn deadline_after(wait: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(wait)
        .or_else(|| now.checked_add(Duration::from_secs(60 * 60 * 24 * 365)))
        .unwrap_or(now)
}

/// The causes of a merged pass as one text, each named once, in the order they came.
fn cause_text(causes: &[Cause]) -> String {
    let mut texts: Vec<String> = Vec::new();
    for cause in causes {
        let text = cause.to_string();
        if !texts.contains(&text) {
            texts.push(text);
        }
    }
    texts.join("; ")
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(ToString::to_string)
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string())
}

#[cfg(test)]
mod tests;
