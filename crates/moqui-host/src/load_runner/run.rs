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

//! The loads running in this process (plans/LoadRunner.md §7, D5, D6): the registry by runId, a run's thread, its
//! stop, its watch on the starting user, and the stop of every run at shutdown. A running load is only here, in
//! memory; once every one of its users' threads has exited its thread writes `result.json` ([`super::results`]) and
//! takes it out of the registry. Nothing is written to the database.
//!
//! Each user's thread has an ExecutionContext of its own for the thread's life, logged in as the user who started the
//! run, with authorization required and the tarpit off (§6, D10). The context is entered only while a `service.call`
//! runs ([`in_user_context`]): the script itself runs with no context, and its thread may call only the client
//! harness's host functions (`moqui_core::script::restrict_host_calls`), so `ec` and the server's other globals do
//! nothing for it (§10). Its requests go through the client host of `moqui test-client`, made for the run
//! ([`ClientHost::for_server_run`]), which asks the run's [`TargetPolicy`] before every request and redirect.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_devkit::testkit::{SpecHost, ThreadBody};
use moqui_lib::sync::{lock, wait_timeout};
use serde_json::{json, Map, Value};

use super::scripts::LoadScript as ScriptFile;
use super::targets::{TargetKind, TargetPolicy};
use super::{results, server};
use crate::test_client::load::{self, Load};
use crate::test_client::{ClientHost, InServer};

/// How long a thread that only reads a plan may run its scripts: the top level only declares.
const PLAN_LIMIT: Duration = Duration::from_secs(30);
/// How long a user thread's `thread.begin` waits for the run to bind its load, which it does as soon as the users
/// have started.
const BIND_WAIT: Duration = Duration::from_secs(5);

/// How long shutdown waits for the running loads to write their results.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);
/// How often a run on this server samples the server ([`server::Samples`]).
const SAMPLE_EVERY: Duration = Duration::from_secs(5);
/// Every how many samples a run checks that the user who started it may still run loads: every 10 seconds.
const CHECK_EVERY_SAMPLES: u32 = 2;
/// How often a run's thread logs that it still waits for user threads that have not exited.
const LINGER_LOG_EVERY: Duration = Duration::from_secs(60);

/// A load in the registry.
pub(crate) struct Active {
    pub(crate) run_id: String,
    pub(crate) script: String,
    pub(crate) label: String,
    pub(crate) user: String,
    pub(crate) target: String,
    pub(crate) target_kind: &'static str,
    pub(crate) load: Arc<Load>,
    /// The user threads that have not exited yet: the run holds its slot until none is left.
    live: Arc<AtomicUsize>,
    /// Why the run was stopped by the server rather than by a user: the starting user lost the right to run loads.
    stopped_because: Mutex<Option<String>>,
    /// The starting user's id, for the run's Monitor annotations.
    user_id: String,
    /// The server's samples, for a run on this server (`this` or `service`); none for another server's.
    server: Option<Mutex<server::Samples>>,
    /// What the run's threads are bound to: its load and the flag that interrupts their scripts.
    bound: Arc<Bound>,
}

impl Active {
    /// The run as the list and the poll show it while it runs: its `run` fields and the snapshot's totals.
    pub(crate) fn summary(&self, snapshot: &Value) -> Value {
        let field = |key: &str| snapshot.get(key).cloned().unwrap_or(Value::Null);
        json!({
            "runId": self.run_id, "script": self.script, "label": self.label, "user": self.user,
            "status": "running", "target": self.target, "targetKind": self.target_kind,
            "start": field("start"), "end": null, "users": self.load.users(), "activeUsers": field("activeUsers"),
            "iterations": field("iterations"), "requests": field("requests"), "errors": field("errors"),
            "errorRate": field("errorRate"), "rps": field("rps"), "passed": null, "duration": field("elapsed"),
            "p95": snapshot.get("latency").and_then(|latency| latency.get("p95")).cloned().unwrap_or(Value::Null),
            "stopping": field("stopping"),
        })
    }

    /// Stop the run before each user's next request, and interrupt a script that runs on without one.
    pub(crate) fn stop_now(&self) {
        self.load.stop_now();
        self.bound.interrupt.store(true, Ordering::SeqCst);
    }

    /// The server's samples so far, for a run on this server.
    pub(crate) fn server_json(&self) -> Option<Value> {
        self.server.as_ref().map(|samples| lock(samples).to_json())
    }
}

/// The registry: the running loads by runId, and the condition a run's end and shutdown wait on.
struct Registry {
    runs: Mutex<HashMap<String, Arc<Active>>>,
    ended: Condvar,
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
    runs: Mutex::new(HashMap::new()),
    ended: Condvar::new(),
});

/// The factories that have begun to shut down, by [`factory_key`]: no run starts on one of them after it. Per factory,
/// since a process may shut one down and boot another (`moqui test --serve`), and cleared when a factory boots.
static SHUTTING_DOWN: LazyLock<Mutex<HashSet<(usize, i64)>>> = LazyLock::new(Mutex::default);

/// A factory's identity: its address and its start time, so a later factory at the same address is another.
fn factory_key(factory: &ExecutionContextFactory) -> (usize, i64) {
    (std::ptr::from_ref(factory) as usize, factory.started_millis())
}

fn shutting_down(factory: &ExecutionContextFactory) -> bool {
    lock(&SHUTTING_DOWN).contains(&factory_key(factory))
}

/// At a factory's boot: it is not shutting down, whatever an earlier factory at its address did.
pub(crate) fn booted(factory: &ExecutionContextFactory) {
    lock(&SHUTTING_DOWN).remove(&factory_key(factory));
}

/// A run's place in the registry, given up when this is dropped, however its thread ends.
struct Slot(String);

impl Drop for Slot {
    fn drop(&mut self) {
        lock(&REGISTRY.runs).remove(&self.0);
        REGISTRY.ended.notify_all();
    }
}

/// The running load `run_id`, if this process runs it.
pub(crate) fn find(run_id: &str) -> Option<Arc<Active>> {
    lock(&REGISTRY.runs).get(run_id).cloned()
}

/// The running loads, newest first.
pub(crate) fn running() -> Vec<Arc<Active>> {
    let mut runs: Vec<Arc<Active>> = lock(&REGISTRY.runs).values().cloned().collect();
    runs.sort_by(|a, b| b.run_id.cmp(&a.run_id));
    runs
}

/// The runIds in the registry.
pub(crate) fn running_ids() -> HashSet<String> {
    lock(&REGISTRY.runs).keys().cloned().collect()
}

/// Wait up to `wait` for run `run_id` to leave the registry; whether it has.
pub(crate) fn wait_for_end(run_id: &str, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    let mut runs = lock(&REGISTRY.runs);
    while runs.contains_key(run_id) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        runs = wait_timeout(&REGISTRY.ended, runs, left).0;
    }
    true
}

/// What the start of a run was asked, already checked by the service: the permission, the script's location.
pub(crate) struct Ask {
    pub(crate) script: ScriptFile,
    pub(crate) component_dir: PathBuf,
    pub(crate) target_kind: TargetKind,
    /// The target URL of `url`; empty for this server.
    pub(crate) target_url: String,
    pub(crate) params: Map<String, Value>,
    /// `{ name: { users, ... } }`.
    pub(crate) scenarios: Value,
    pub(crate) label: String,
    pub(crate) insecure: bool,
    /// The user who starts the run, whom its users' threads are logged in as.
    pub(crate) user: String,
    pub(crate) user_id: String,
}

/// The host calls a user thread makes only between its `thread.begin` and its `thread.end` (and `step.*`).
const BEGUN_ONLY: [&str; 7] = [
    "load.ramp",
    "load.next",
    "load.pause",
    "load.timed",
    "load.iterated",
    "load.error",
    "sleep",
];

/// What a run binds its threads to, in Rust and not through the script's own calls (plans/LoadRunner.md §10): its
/// load, set once its users have started, and the flag that interrupts their scripts on a stop now.
pub(crate) struct Bound {
    load: OnceLock<Arc<Load>>,
    interrupt: Arc<AtomicBool>,
}

impl Bound {
    fn new() -> Arc<Bound> {
        Arc::new(Bound {
            load: OnceLock::new(),
            interrupt: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The run's load, waiting at most [`BIND_WAIT`] for the run to bind it.
    fn load(&self) -> Option<&Arc<Load>> {
        let deadline = Instant::now() + BIND_WAIT;
        loop {
            if let Some(load) = self.load.get() {
                return Some(load);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// The run's side of the client host: its target rule, what its plan declares it uses, its threads' binding, and
/// `service.call`.
struct RunInServer {
    policy: TargetPolicy,
    /// The plan's `uses`, set once the plan is read; before that (the plan's own top level) nothing is used.
    uses: OnceLock<Vec<String>>,
    bound: Arc<Bound>,
}

impl RunInServer {
    fn uses(&self, kind: &str) -> bool {
        self.uses.get().is_some_and(|uses| uses.iter().any(|used| used == kind))
    }
}

impl InServer for RunInServer {
    fn allow(&self, url: &str) -> Result<(), String> {
        if !self.uses("http") {
            return Err(
                "the load script makes no HTTP request: its plan does not declare uses: [\"http\"]".to_string(),
            );
        }
        may_act()?;
        self.policy.allow(url)
    }

    fn service_call(&self, args: &Value) -> Result<Value, String> {
        if !self.uses("service") {
            return Err("the load script calls no service: its plan does not declare uses: [\"service\"]".to_string());
        }
        super::service_call::call(args)
    }

    /// A user thread's scripts end when the run is stopped now; a thread that only reads a plan (no user bound) has
    /// [`PLAN_LIMIT`] for its scripts, so a top level that spins does not stay behind.
    fn thread_start(&self) -> Option<String> {
        let token = USER.with(|cell| cell.borrow().as_ref().map(UserThread::issue_token));
        match token {
            Some(token) => {
                moqui_core::script::interrupt_thread_on(Some(Arc::clone(&self.bound.interrupt)));
                token
            }
            None => {
                moqui_core::script::limit_thread_scripts(Instant::now() + PLAN_LIMIT);
                None
            }
        }
    }

    /// `thread.begin` once, on a user thread, for the run's own load; `thread.end` once, after it. Any other is the
    /// script's own call, refused: another run's load, a second begin, or an end that would drop the recorder.
    fn check_call(&self, name: &str, args: &Value) -> Result<(), String> {
        match name {
            "thread.begin" => {
                let load = self.bound.load().ok_or("the run has not bound its load")?;
                let asked = args.get("load").and_then(Value::as_u64);
                if asked != Some(load.id()) {
                    return Err("thread.begin names a load that is not this thread's".to_string());
                }
                take_token(args.get("token").and_then(Value::as_str))?;
                move_phase(Phase::NotBegun, Phase::Begun)
            }
            "thread.end" => move_phase(Phase::Begun, Phase::Ended),
            // the user's waits and its recorder's calls, only between its begin and its end: before (the script's
            // top level) a wait would hold the run's slot with no recorder to see a stop
            name if BEGUN_ONLY.contains(&name) || name.starts_with("step.") => {
                // after an early end too: the recorder lives until the thread ends, and a wait still sees the stop
                if matches!(phase(), Some(Phase::Begun | Phase::Ended)) {
                    Ok(())
                } else {
                    Err(format!("{name} is only for a load user's scenario"))
                }
            }
            _ => Ok(()),
        }
    }
}

/// This server's own URL: https when the conf webroot has it, else http; `http://localhost` when the conf has none.
fn own_url(factory: &ExecutionContextFactory) -> String {
    let url = factory.webapp_root_url_for("webroot", None);
    if url.is_empty() {
        "http://localhost".to_string()
    } else {
        url
    }
}

/// The client host for a run of a script of `component_dir` against `server`.
fn host_for(
    factory: &ExecutionContextFactory,
    server: &str,
    component_dir: &Path,
    insecure: bool,
    bound: Arc<Bound>,
) -> Result<(Arc<dyn SpecHost>, Arc<RunInServer>), String> {
    let in_server = Arc::new(RunInServer {
        policy: TargetPolicy::of_factory(factory),
        uses: OnceLock::new(),
        bound,
    });
    let dyn_in_server: Arc<dyn InServer> = Arc::clone(&in_server) as Arc<dyn InServer>;
    let host = ClientHost::for_server_run(
        server,
        BTreeMap::new(),
        insecure,
        component_dir.to_path_buf(),
        dyn_in_server,
    )?;
    Ok((host, in_server))
}

/// The plans read for the script list, by file, with the file's time and length: a script is evaluated again only
/// when it changed, and one whose top level failed or timed out is not run again until then.
type PlanCache = HashMap<PathBuf, (SystemTime, u64, Result<load::script::Plan, String>)>;

static PLANS: LazyLock<Mutex<PlanCache>> = LazyLock::new(Mutex::default);

/// Read the plan of `script` for the script list: its top level evaluated with the defaults, against this server,
/// from [`PLANS`] while the file is the same.
pub(crate) fn read_plan_for_list(
    factory: &ExecutionContextFactory,
    script: &ScriptFile,
    component_dir: &Path,
) -> Result<load::script::Plan, String> {
    let meta = std::fs::metadata(&script.path).map_err(|err| format!("{}: {err}", script.location))?;
    let stamp = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
    if let Some((at, len, plan)) = lock(&PLANS).get(&script.path) {
        if (*at, *len) == stamp {
            return plan.clone();
        }
    }
    let read = (|| {
        let (host, _) = host_for(factory, &own_url(factory), component_dir, false, Bound::new())?;
        let file = load::LoadScript::from_file(&script.path, &script.location)?;
        load::script::read_plan_listed(&host, &file, &json!({}))
    })();
    lock(&PLANS).insert(script.path.clone(), (stamp.0, stamp.1, read.clone()));
    read
}

/// Where a user thread is in its run: before its `thread.begin` (the script file's top level), between it and its
/// `thread.end` (setup, the iterations, teardown), or after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    NotBegun,
    Begun,
    Ended,
}

/// A load user's thread in a server run: its ExecutionContext (entered only while a service call runs, and replaced
/// when a call leaves it changed), whom it is logged in as, its run's binding and its phase.
struct UserThread {
    context: RefCell<ExecutionContext>,
    factory: Arc<ExecutionContextFactory>,
    user: String,
    bound: Arc<Bound>,
    phase: Cell<Phase>,
    /// The one-time token of the prelude's `thread.begin`, given out by the thread's first `config` only, before
    /// any script of the thread runs: a script's own `thread.begin` has none.
    token: RefCell<Option<String>>,
    issued: Cell<bool>,
    /// A call left the context changed and a fresh one could not be logged in: the thread does nothing more.
    broken: Cell<bool>,
}

impl UserThread {
    /// The thread's token, at its first `config` only.
    fn issue_token(&self) -> Option<String> {
        if self.issued.replace(true) {
            return None;
        }
        let token = new_token();
        *self.token.borrow_mut() = Some(token.clone());
        Some(token)
    }
}

/// A token no script can guess: 128 bits from the process's random hasher keys, the time and the thread.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = |salt: u64| {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(salt);
        hasher.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |at| at.as_nanos()),
        );
        hasher.write(format!("{:?}", std::thread::current().id()).as_bytes());
        hasher.finish()
    };
    format!("{:016x}{:016x}", part(1), part(2))
}

/// Take this user thread's token, refused unless `given` is it: once only.
fn take_token(given: Option<&str>) -> Result<(), String> {
    USER.with(|cell| {
        let held = cell.borrow();
        let user = held.as_ref().ok_or("not a load user's thread")?;
        let expected = user.token.borrow_mut().take();
        match (expected, given) {
            (Some(expected), Some(given)) if expected == given => Ok(()),
            _ => Err("thread.begin is the load runner's own".to_string()),
        }
    })
}

/// This thread's phase; none when it is not a load user's thread.
fn phase() -> Option<Phase> {
    USER.with(|cell| cell.borrow().as_ref().map(|user| user.phase.get()))
}

thread_local! {
    static USER: RefCell<Option<UserThread>> = const { RefCell::new(None) };
}

/// Move this user thread from `from` to `to`, refused when it is not a user thread or not at `from`.
fn move_phase(from: Phase, to: Phase) -> Result<(), String> {
    USER.with(|cell| {
        let held = cell.borrow();
        let user = held.as_ref().ok_or("not a load user's thread")?;
        if user.phase.get() != from {
            return Err(format!("a load user's thread is {:?}, not {from:?}", user.phase.get()));
        }
        user.phase.set(to);
        Ok(())
    })
}

/// Whether this thread may send a request or call a service: a user thread between its `thread.begin` and its
/// `thread.end` (not the script file's top level, not after it ended), of a load not stopped now.
fn may_act() -> Result<(), String> {
    USER.with(|cell| {
        let held = cell.borrow();
        let user = held.as_ref().ok_or("a load script makes no request at its top level")?;
        if user.broken.get() {
            return Err("this load user's context could not be renewed; it makes no more calls".to_string());
        }
        match user.phase.get() {
            Phase::Begun => {}
            Phase::NotBegun => return Err("a load script makes no request at its top level".to_string()),
            Phase::Ended => return Err("this load user has ended".to_string()),
        }
        let stopped = user.bound.interrupt.load(Ordering::SeqCst)
            || user
                .bound
                .load
                .get()
                .is_none_or(|load| matches!(load.stopping(), Some("now" | "timed-out")));
        if stopped {
            return Err("the load was stopped".to_string());
        }
        Ok(())
    })
}

/// The user out of a run's own context, and nothing more: not a logout, which would mark the account logged out
/// (`hasLoggedOut`) and sign the starter's browser out with it.
fn forget_user(context: &ExecutionContext) {
    let _entered = context.enter();
    moqui_core::user::session::clear_login();
}

/// A context of its own for `user`, logged in, the tarpit off.
fn user_context(factory: &Arc<ExecutionContextFactory>, user: &str) -> Result<ExecutionContext, String> {
    let context = factory.new_context();
    {
        let _entered = context.enter();
        moqui_core::user::internal_login_user(user)
            .map_err(|err| format!("the run's user {user} could not be logged in: {err}"))?;
        context.disable_tarpit();
    }
    Ok(context)
}

/// What a call left changed in the context that a later call would inherit: authorization or entity ECA rules off,
/// or another user. Read with the context entered.
fn tainted(context: &ExecutionContext, user: &str) -> Option<String> {
    if context.authz_disabled() {
        return Some("it left authorization disabled".to_string());
    }
    if context.entity_eca_disabled() {
        return Some("it left entity ECA rules disabled".to_string());
    }
    let current = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    (current != user).then(|| format!("it left the context logged in as {current:?}, not the run's user"))
}

/// Run `body` with this load user's ExecutionContext entered and the host restriction lifted (a service's own
/// scripts are the server's), when [`may_act`] allows. Answers what `body` gave and, when it left the context
/// changed ([`tainted`]), why: the context is then thrown away and a fresh one logged in, so no later call
/// inherits it, and the caller counts the call as an error.
pub(crate) fn in_user_context<T>(body: impl FnOnce() -> T) -> Result<(T, Option<String>), String> {
    may_act()?;
    USER.with(|cell| {
        let held = cell.borrow();
        let user = held.as_ref().ok_or("not a load user's thread")?;
        let (out, taint) = {
            let context = user.context.borrow();
            let _entered = context.enter();
            let out = {
                let _lifted = moqui_core::script::HostCallsLifted::new();
                body()
            };
            // what a call may leave for the next one and that is no one's error: the shared map, an effective time
            context.shared_map().clear();
            context.set_effective_time(None);
            (out, tainted(&context, &user.user))
        };
        if let Some(reason) = &taint {
            tracing::warn!(
                user = user.user,
                reason,
                "A load run's service call changed its context; replacing it"
            );
            match user_context(&user.factory, &user.user) {
                Ok(fresh) => {
                    let old = user.context.replace(fresh);
                    forget_user(&old);
                }
                Err(err) => {
                    // the changed context loses its user and the thread refuses every later call
                    tracing::error!(user = user.user, error = %err, "A load user's context could not be renewed");
                    forget_user(&user.context.borrow());
                    user.broken.set(true);
                }
            }
        }
        Ok((out, taint))
    })
}

/// A user thread counted live from its making until its closure is dropped, run or not.
struct Live(Arc<AtomicUsize>);

impl Live {
    fn new(live: &Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(live))
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Each user's thread: an ExecutionContext of its own logged in as `user`, the tarpit off, kept for the thread's life
/// and entered only for its service calls; bound to the run ([`Bound`]) in Rust; counted in `live` until the thread
/// is done with it.
fn around(
    factory: Arc<ExecutionContextFactory>,
    user: String,
    live: Arc<AtomicUsize>,
    bound: Arc<Bound>,
) -> load::Around {
    Arc::new(move || {
        let factory = Arc::clone(&factory);
        let user = user.clone();
        let bound = Arc::clone(&bound);
        let live = Live::new(&live);
        Box::new(move |body: ThreadBody<'_>| {
            let _live = live;
            let context = user_context(&factory, &user)?;
            USER.with(|cell| {
                *cell.borrow_mut() = Some(UserThread {
                    context: RefCell::new(context),
                    factory,
                    user,
                    bound,
                    phase: Cell::new(Phase::NotBegun),
                    token: RefCell::new(None),
                    issued: Cell::new(false),
                    broken: Cell::new(false),
                })
            });
            let out = body();
            // the recorder merges into the load here, whatever the script called
            let _ = load::end();
            moqui_core::script::interrupt_thread_on(None);
            if let Some(ended) = USER.with(|cell| cell.borrow_mut().take()) {
                forget_user(&ended.context.into_inner());
            }
            out
        })
    })
}

/// Start a load and answer its runId: the plan read, `uses` checked against the target, the scenarios' users and the
/// runs at once checked against the limits (D6), the target checked (§10). The run goes on on a thread of its own.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>, ask: Ask) -> Result<String, String> {
    if shutting_down(factory) {
        return Err("this server is shutting down".to_string());
    }
    let policy = TargetPolicy::of_factory(factory);
    let target = match ask.target_kind {
        TargetKind::Url => ask.target_url.trim().trim_end_matches('/').to_string(),
        TargetKind::This | TargetKind::Service => own_url(factory),
    };
    policy.allow(&target)?;
    let bound = Bound::new();
    let (host, in_server) = host_for(factory, &target, &ask.component_dir, ask.insecure, Arc::clone(&bound))?;
    let file = load::LoadScript::from_file(&ask.script.path, &ask.script.location)?;
    let plan = load::script::read_plan(&host, &file, &Value::Object(ask.params.clone()), &json!({}))?;
    let uses = plan.uses();
    check_uses(&uses, ask.target_kind)?;
    let _ = in_server.uses.set(uses);
    // no overrides runs every scenario with its defaults; an object names the scenarios to run (script::settings)
    let overrides = match &ask.scenarios {
        Value::Object(map) if !map.is_empty() => ask.scenarios.clone(),
        _ => Value::Null,
    };
    let scenarios = load::script::settings(&plan, &json!({ "scenarios": overrides }))?;
    let total: u64 = scenarios.iter().map(|scenario| scenario.users).sum();
    super::targets::check_users(usize::try_from(total).unwrap_or(usize::MAX))?;

    let on_this_server = ask.target_kind != TargetKind::Url;
    // the CPU baseline of the server panel's first sample, a few seconds on ([`server::Samples`])
    let samples = on_this_server.then(|| Mutex::new(server::Samples::new()));
    let mut runs = lock(&REGISTRY.runs);
    if shutting_down(factory) {
        return Err("this server is shutting down".to_string());
    }
    let max = super::targets::max_runs();
    if runs.len() >= max {
        return Err(format!(
            "{} load(s) already running on this server, the most at once (load_runner_max_runs {max})",
            runs.len()
        ));
    }
    let root = results::root(factory);
    let base = results::run_id_base(jiff::Timestamp::now(), std::process::id());
    let (run_id, dir) = results::create_run_dir(&root, &base)?;
    let mut run = Map::new();
    run.insert("script".into(), json!(ask.script.location));
    run.insert("label".into(), json!(ask.label));
    run.insert("user".into(), json!(ask.user));
    run.insert("target".into(), json!(target));
    run.insert("targetKind".into(), json!(kind_name(ask.target_kind)));
    run.insert("params".into(), plan.public_params());
    run.insert("insecure".into(), json!(ask.insecure));
    run.insert("runId".into(), json!(run_id));
    // each scenario's settings as the run used them, for Run again: the overrides merged over the script's defaults
    let settings: Map<String, Value> = scenarios
        .iter()
        .map(|scenario| (scenario.name.clone(), scenario.to_json()))
        .collect();
    run.insert("scenarioSettings".into(), Value::Object(settings));
    let live = Arc::new(AtomicUsize::new(0));
    let start = load::Start {
        scenarios,
        source: load::Source::Script {
            script: Arc::new(file),
            params: plan.params.clone(),
        },
        timeout: None,
        thresholds: plan.thresholds().clone(),
        run: run.clone(),
        generator: "in-target",
        around: Some(around(
            Arc::clone(factory),
            ask.user.clone(),
            Arc::clone(&live),
            Arc::clone(&bound),
        )),
        stop_after: None,
        stop_now_after: None,
    };
    let running = match load::start(&host, start) {
        Ok(running) => running,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(err);
        }
    };
    // the users' thread.begin waits for this ([`Bound::load`])
    let _ = bound.load.set(Arc::clone(running.load()));
    let active = Arc::new(Active {
        run_id: run_id.clone(),
        script: ask.script.location.clone(),
        label: ask.label.clone(),
        user: ask.user.clone(),
        target: target.clone(),
        target_kind: kind_name(ask.target_kind),
        load: Arc::clone(running.load()),
        live,
        stopped_because: Mutex::new(None),
        user_id: ask.user_id.clone(),
        server: samples,
        bound,
    });
    runs.insert(run_id.clone(), Arc::clone(&active));
    drop(runs);
    if on_this_server {
        let text = server::annotation_text(&ask.label, &ask.script.location, "started");
        server::annotate(factory, &run_id, &text, &ask.user_id);
    }
    let slot = Slot(run_id.clone());
    let thread_active = Arc::clone(&active);
    let thread_factory = Arc::clone(factory);
    let spawned = std::thread::Builder::new()
        .name(format!("moqui-load-run-{run_id}"))
        .spawn(move || finish(&thread_factory, &root, slot, &thread_active, running, run, host));
    if let Err(err) = spawned {
        // the closure, with the Running (whose drop stops the users) and the slot, was dropped
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("the run's thread did not start: {err}"));
    }
    let watched = Arc::clone(&active);
    let factory = Arc::clone(factory);
    if let Err(err) = std::thread::Builder::new()
        .name(format!("moqui-load-watch-{run_id}"))
        .spawn(move || watch(&factory, &watched))
    {
        tracing::error!(run_id, error = %err, "A load run's watch on its user did not start; stopping it");
        active.stop_now();
    }
    Ok(run_id)
}

/// A run's thread: wait for its users and for every user thread to exit, write its result, and give up its slot
/// (on any end, a panic's too, by the slot's drop). A panic in the wait is a `failed` run, whose result says so.
fn finish(
    factory: &ExecutionContextFactory,
    root: &Path,
    slot: Slot,
    active: &Active,
    running: load::Running,
    run: Map<String, Value>,
    host: Arc<dyn SpecHost>,
) {
    let run_id = slot.0.clone();
    let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| running.wait()));
    let mut result = match waited {
        Ok((result, _order)) => result,
        Err(_) => {
            tracing::error!(run_id, "A load run's thread panicked; its result is written as failed");
            let mut run = run;
            run.insert("status".into(), json!("failed"));
            run.insert("end".into(), json!(jiff::Timestamp::now().to_string()));
            json!({ "users": 0, "iterations": 0, "requests": 0, "errors": 0, "latency": {}, "names": {}, "run": run })
        }
    };
    // the users were told to stop; a thread past its time may still be in a request or a loop of its own, which the
    // interrupt ends, and the run is not over until it has exited
    active.bound.interrupt.store(true, Ordering::SeqCst);
    let mut logged = Instant::now();
    while active.live.load(Ordering::SeqCst) > 0 {
        if logged.elapsed() >= LINGER_LOG_EVERY {
            tracing::warn!(
                run_id,
                threads = active.live.load(Ordering::SeqCst),
                "A load run waits for user threads that have not exited"
            );
            logged = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(because) = lock(&active.stopped_because).clone() {
        if let Some(run) = result.get_mut("run").and_then(Value::as_object_mut) {
            run.insert("stoppedBecause".into(), json!(because));
        }
    }
    if let Some(samples) = &active.server {
        let mut samples = lock(samples);
        samples.sample(factory);
        result["server"] = samples.to_json();
        if let Some(generator) = result.get_mut("generator").and_then(Value::as_object_mut) {
            generator.insert("note".into(), json!(server::IN_TARGET_NOTE));
        }
    }
    if let Err(err) = results::write_result(root, &run_id, &result) {
        tracing::error!(run_id, error = %err, "Could not write a load run's result");
    }
    // after the result, so a slow search index delays neither it nor the slot's release by much
    if active.server.is_some() {
        let status = result
            .get("run")
            .and_then(|run| run.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("ended");
        let text = server::annotation_text(&active.label, &active.script, status);
        server::annotate(factory, &run_id, &text, &active.user_id);
    }
    drop(host);
    drop(slot);
}

/// While the run is in the registry: every [`SAMPLE_EVERY`] a sample of the server for a run on this server, and every
/// [`CHECK_EVERY_SAMPLES`] of those a check that the user who started it can still log in and holds `LOAD_RUNNER`;
/// when not, the run is stopped now. A run lasts no longer than its starter's right to run it.
fn watch(factory: &Arc<ExecutionContextFactory>, active: &Active) {
    let mut ticks: u32 = 0;
    loop {
        let deadline = Instant::now() + SAMPLE_EVERY;
        {
            let mut runs = lock(&REGISTRY.runs);
            loop {
                if !runs.contains_key(&active.run_id) {
                    return;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                runs = wait_timeout(&REGISTRY.ended, runs, left).0;
            }
        }
        if let Some(samples) = &active.server {
            lock(samples).sample(factory);
        }
        ticks += 1;
        if !ticks.is_multiple_of(CHECK_EVERY_SAMPLES) {
            continue;
        }
        if let Err(reason) = may_still_run(factory, &active.user) {
            tracing::warn!(
                run_id = active.run_id,
                user = active.user,
                reason,
                "Stopping a load run"
            );
            *lock(&active.stopped_because) = Some(reason);
            active.stop_now();
            return;
        }
    }
}

/// Whether `user` can still log in and holds `LOAD_RUNNER`, read in a context of its own.
fn may_still_run(factory: &Arc<ExecutionContextFactory>, user: &str) -> Result<(), String> {
    let context = factory.new_context();
    let _entered = context.enter();
    if let Err(err) = moqui_core::user::internal_login_user(user) {
        return Err(format!("the user who started it can no longer log in: {err}"));
    }
    let holds = moqui_core::user::has_permission(super::PERMISSION).unwrap_or(false);
    forget_user(&context);
    if holds {
        Ok(())
    } else {
        Err("the user who started it no longer has the LOAD_RUNNER permission".to_string())
    }
}

fn kind_name(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Service => "service",
        TargetKind::This => "this",
        TargetKind::Url => "url",
    }
}

/// A plan's `uses` against the run's target: `service` needs the `service` target (this server's process), `http`
/// needs `this` or `url`.
pub(crate) fn check_uses(uses: &[String], kind: TargetKind) -> Result<(), String> {
    for kind_used in uses {
        match (kind_used.as_str(), kind) {
            ("service", TargetKind::Service) | ("http", TargetKind::This | TargetKind::Url) => {}
            ("service", _) => {
                return Err(
                    "the script calls services in process (uses service): run it with the service target".into(),
                )
            }
            ("http", _) => {
                return Err("the script makes HTTP requests (uses http): run it against this server or a URL".into())
            }
            (other, _) => return Err(format!("the script uses {other}, which a run in a server cannot make")),
        }
    }
    Ok(())
}

/// Stop run `run_id`: after each user's current iteration, or with `now` before each user's next request. None
/// when it is not running here.
pub(crate) fn stop(run_id: &str, now: bool) -> Option<()> {
    let active = find(run_id)?;
    if now {
        active.stop_now();
    } else {
        active.load.stop_after_iteration();
    }
    Some(())
}

/// At shutdown: no run starts from now, every running load is stopped now (under the registry's lock, so none is
/// added meanwhile), and its result written as stopped, waiting at most [`SHUTDOWN_WAIT`] for them.
pub(crate) fn stop_all(factory: &ExecutionContextFactory) {
    let mut held = lock(&REGISTRY.runs);
    lock(&SHUTTING_DOWN).insert(factory_key(factory));
    for active in held.values() {
        active.stop_now();
    }
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    while !held.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            tracing::error!(
                runs = held.len(),
                "Load runs still running at shutdown after {}s; their results may be missing",
                SHUTDOWN_WAIT.as_secs()
            );
            return;
        }
        held = wait_timeout(&REGISTRY.ended, held, left).0;
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
