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

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_devkit::testkit::{SpecHost, ThreadBody};
use moqui_lib::sync::{lock, wait_timeout};
use serde_json::{json, Map, Value};

use super::results;
use super::scripts::LoadScript as ScriptFile;
use super::targets::{TargetKind, TargetPolicy};
use crate::test_client::load::{self, Load};
use crate::test_client::{ClientHost, InServer};

/// How long shutdown waits for the running loads to write their results.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);
/// How often a run checks that the user who started it may still run loads.
const WATCH_EVERY: Duration = Duration::from_secs(10);
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

/// Set at shutdown: no run starts after it.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

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
}

/// The run's side of the client host: its target rule, what its plan declares it uses, and `service.call`.
struct RunInServer {
    policy: TargetPolicy,
    /// The plan's `uses`, set once the plan is read; before that (the plan's own top level) nothing is used.
    uses: OnceLock<Vec<String>>,
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
        self.policy.allow(url)
    }

    fn service_call(&self, args: &Value) -> Result<Value, String> {
        if !self.uses("service") {
            return Err("the load script calls no service: its plan does not declare uses: [\"service\"]".to_string());
        }
        super::service_call::call(args)
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
) -> Result<(Arc<dyn SpecHost>, Arc<RunInServer>), String> {
    let in_server = Arc::new(RunInServer {
        policy: TargetPolicy::of_factory(factory),
        uses: OnceLock::new(),
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
        let (host, _) = host_for(factory, &own_url(factory), component_dir, false)?;
        let file = load::LoadScript::from_file(&script.path, &script.location)?;
        load::script::read_plan(&host, &file, &json!({}))
    })();
    lock(&PLANS).insert(script.path.clone(), (stamp.0, stamp.1, read.clone()));
    read
}

thread_local! {
    /// The load user's ExecutionContext on this thread, entered only while a service call runs.
    static USER_CONTEXT: RefCell<Option<ExecutionContext>> = const { RefCell::new(None) };
}

/// Run `body` with this load user's ExecutionContext entered and the host restriction lifted (a service's own
/// scripts are the server's); None on a thread that is not a load user's in a server run.
pub(crate) fn in_user_context<T>(body: impl FnOnce() -> T) -> Option<T> {
    USER_CONTEXT.with(|cell| {
        let held = cell.borrow();
        let context = held.as_ref()?;
        let _entered = context.enter();
        let _lifted = moqui_core::script::HostCallsLifted::new();
        Some(body())
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
/// and entered only for its service calls; counted in `live` until the thread is done with it.
fn around(factory: Arc<ExecutionContextFactory>, user: String, live: Arc<AtomicUsize>) -> load::Around {
    Arc::new(move || {
        let factory = Arc::clone(&factory);
        let user = user.clone();
        let live = Live::new(&live);
        Box::new(move |body: ThreadBody<'_>| {
            let _live = live;
            let context = factory.new_context();
            {
                let _entered = context.enter();
                if let Err(err) = moqui_core::user::internal_login_user(&user) {
                    return Err(format!("the run's user {user} could not be logged in: {err}"));
                }
                context.disable_tarpit();
            }
            USER_CONTEXT.with(|cell| *cell.borrow_mut() = Some(context));
            let out = body();
            if let Some(context) = USER_CONTEXT.with(|cell| cell.borrow_mut().take()) {
                let _entered = context.enter();
                if let Err(err) = moqui_core::user::logout() {
                    tracing::debug!("logout after a load user: {err}");
                }
            }
            out
        })
    })
}

/// Start a load and answer its runId: the plan read, `uses` checked against the target, the scenarios' users and the
/// runs at once checked against the limits (D6), the target checked (§10). The run goes on on a thread of its own.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>, ask: Ask) -> Result<String, String> {
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
        return Err("this server is shutting down".to_string());
    }
    let policy = TargetPolicy::of_factory(factory);
    let target = match ask.target_kind {
        TargetKind::Url => ask.target_url.trim().trim_end_matches('/').to_string(),
        TargetKind::This | TargetKind::Service => own_url(factory),
    };
    policy.allow(&target)?;
    let (host, in_server) = host_for(factory, &target, &ask.component_dir, ask.insecure)?;
    let file = load::LoadScript::from_file(&ask.script.path, &ask.script.location)?;
    let plan = load::script::read_plan(&host, &file, &Value::Object(ask.params.clone()))?;
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

    let mut runs = lock(&REGISTRY.runs);
    if SHUTTING_DOWN.load(Ordering::SeqCst) {
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
        around: Some(around(Arc::clone(factory), ask.user.clone(), Arc::clone(&live))),
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
    });
    runs.insert(run_id.clone(), Arc::clone(&active));
    drop(runs);
    let slot = Slot(run_id.clone());
    let thread_active = Arc::clone(&active);
    let spawned = std::thread::Builder::new()
        .name(format!("moqui-load-run-{run_id}"))
        .spawn(move || finish(&root, slot, &thread_active, running, run, host));
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
        active.load.stop_now();
    }
    Ok(run_id)
}

/// A run's thread: wait for its users and for every user thread to exit, write its result, and give up its slot
/// (on any end, a panic's too, by the slot's drop). A panic in the wait is a `failed` run, whose result says so.
fn finish(
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
    // the users were told to stop; a thread past its time may still be in a request, and the run is not over until
    // it has exited
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
    if let Err(err) = results::write_result(root, &run_id, &result) {
        tracing::error!(run_id, error = %err, "Could not write a load run's result");
    }
    drop(host);
    drop(slot);
}

/// While the run is in the registry, check every [`WATCH_EVERY`] that the user who started it can still log in and
/// holds `LOAD_RUNNER`; when not, stop the run now. A run lasts no longer than its starter's right to run it.
fn watch(factory: &Arc<ExecutionContextFactory>, active: &Active) {
    loop {
        let deadline = Instant::now() + WATCH_EVERY;
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
        if let Err(reason) = may_still_run(factory, &active.user) {
            tracing::warn!(
                run_id = active.run_id,
                user = active.user,
                reason,
                "Stopping a load run"
            );
            *lock(&active.stopped_because) = Some(reason);
            active.load.stop_now();
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
    if let Err(err) = moqui_core::user::logout() {
        tracing::debug!("logout after a load run's check: {err}");
    }
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
        active.load.stop_now();
    } else {
        active.load.stop_after_iteration();
    }
    Some(())
}

/// At shutdown: no run starts from now, every running load is stopped now (under the registry's lock, so none is
/// added meanwhile), and its result written as stopped, waiting at most [`SHUTDOWN_WAIT`] for them.
pub(crate) fn stop_all() {
    let mut held = lock(&REGISTRY.runs);
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    for active in held.values() {
        active.load.stop_now();
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
