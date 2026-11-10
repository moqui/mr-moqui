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
//! stop, and the stop of every run at shutdown. A running load is only here, in memory; at its end its thread writes
//! `result.json` ([`super::results`]) and takes it out of the registry. Nothing is written to the database.
//!
//! Each user's thread enters an ExecutionContext of its own for the thread's life, logged in as the user who started
//! the run, with authorization required and the tarpit off (§6, D10). Its requests go through the client host of
//! `moqui test-client`, made for the run ([`ClientHost::for_server_run`]), which asks the run's [`TargetPolicy`]
//! before every request and redirect.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
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

/// A load in the registry.
pub(crate) struct Active {
    pub(crate) run_id: String,
    pub(crate) script: String,
    pub(crate) label: String,
    pub(crate) user: String,
    pub(crate) target: String,
    pub(crate) target_kind: &'static str,
    pub(crate) load: Arc<Load>,
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

/// The run's side of the client host: its target rule and `service.call` in process.
struct RunInServer {
    policy: TargetPolicy,
}

impl InServer for RunInServer {
    fn allow(&self, url: &str) -> Result<(), String> {
        self.policy.allow(url)
    }

    fn service_call(&self, args: &Value) -> Result<Value, String> {
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
) -> Result<Arc<dyn SpecHost>, String> {
    let in_server: Arc<dyn InServer> = Arc::new(RunInServer {
        policy: TargetPolicy::of_factory(factory),
    });
    let host = ClientHost::for_server_run(
        server,
        BTreeMap::new(),
        insecure,
        component_dir.to_path_buf(),
        in_server,
    )?;
    Ok(host)
}

/// Read the plan of `script` for the script list: its top level evaluated with the defaults, against this server.
pub(crate) fn read_plan_for_list(
    factory: &ExecutionContextFactory,
    script: &ScriptFile,
    component_dir: &Path,
) -> Result<load::script::Plan, String> {
    let host = host_for(factory, &own_url(factory), component_dir, false)?;
    let file = load::LoadScript::from_file(&script.path, &script.location)?;
    load::script::read_plan(&host, &file, &json!({}))
}

/// Each user's thread: an ExecutionContext of its own, logged in as `user`, the tarpit off, for the thread's life.
fn around(factory: Arc<ExecutionContextFactory>, user: String) -> load::Around {
    Arc::new(move || {
        let factory = Arc::clone(&factory);
        let user = user.clone();
        Box::new(move |body: ThreadBody<'_>| {
            let context = factory.new_context();
            let _entered = context.enter();
            if let Err(err) = moqui_core::user::internal_login_user(&user) {
                return Err(format!("the run's user {user} could not be logged in: {err}"));
            }
            context.disable_tarpit();
            let out = body();
            if let Err(err) = moqui_core::user::logout() {
                tracing::debug!("logout after a load user: {err}");
            }
            out
        })
    })
}

/// Start a load and answer its runId: the plan read, `uses` checked against the target, the scenarios' users and the
/// runs at once checked against the limits (D6), the target checked (§10). The run goes on on a thread of its own.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>, ask: Ask) -> Result<String, String> {
    let policy = TargetPolicy::of_factory(factory);
    let target = match ask.target_kind {
        TargetKind::Url => ask.target_url.trim().trim_end_matches('/').to_string(),
        TargetKind::This | TargetKind::Service => own_url(factory),
    };
    if ask.target_kind != TargetKind::Service {
        policy.allow(&target)?;
    }
    let host = host_for(factory, &target, &ask.component_dir, ask.insecure)?;
    let file = load::LoadScript::from_file(&ask.script.path, &ask.script.location)?;
    let plan = load::script::read_plan(&host, &file, &Value::Object(ask.params.clone()))?;
    let uses = plan.uses();
    check_uses(&uses, ask.target_kind)?;
    // no overrides runs every scenario with its defaults; an object names the scenarios to run (script::settings)
    let overrides = match &ask.scenarios {
        Value::Object(map) if !map.is_empty() => ask.scenarios.clone(),
        _ => Value::Null,
    };
    let scenarios = load::script::settings(&plan, &json!({ "scenarios": overrides }))?;
    let total: u64 = scenarios.iter().map(|scenario| scenario.users).sum();
    super::targets::check_users(usize::try_from(total).unwrap_or(usize::MAX))?;

    let mut runs = lock(&REGISTRY.runs);
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
        around: Some(around(Arc::clone(factory), ask.user.clone())),
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
    });
    runs.insert(run_id.clone(), active);
    drop(runs);
    let thread_id = run_id.clone();
    let spawned = std::thread::Builder::new()
        .name(format!("moqui-load-run-{run_id}"))
        .spawn(move || finish(&root, &thread_id, running, run, host));
    if let Err(err) = spawned {
        // the Running was moved into the closure, which dropping stops
        lock(&REGISTRY.runs).remove(&run_id);
        REGISTRY.ended.notify_all();
        return Err(format!("the run's thread did not start: {err}"));
    }
    Ok(run_id)
}

/// A run's thread: wait for its users, write its result, and take it out of the registry. A panic is a `failed`
/// run, whose result still says so.
fn finish(root: &Path, run_id: &str, running: load::Running, run: Map<String, Value>, host: Arc<dyn SpecHost>) {
    let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| running.wait()));
    let result = match waited {
        Ok((result, _order)) => result,
        Err(_) => {
            tracing::error!(run_id, "A load run's thread panicked; its result is written as failed");
            let mut run = run;
            run.insert("status".into(), json!("failed"));
            run.insert("end".into(), json!(jiff::Timestamp::now().to_string()));
            json!({ "users": 0, "iterations": 0, "requests": 0, "errors": 0, "latency": {}, "names": {}, "run": run })
        }
    };
    if let Err(err) = results::write_result(root, run_id, &result) {
        tracing::error!(run_id, error = %err, "Could not write a load run's result");
    }
    drop(host);
    lock(&REGISTRY.runs).remove(run_id);
    REGISTRY.ended.notify_all();
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

/// At shutdown: every running load stopped now, and its result written as stopped, waiting at most
/// [`SHUTDOWN_WAIT`] for them.
pub(crate) fn stop_all() {
    let runs = running();
    if runs.is_empty() {
        return;
    }
    for active in &runs {
        active.load.stop_now();
    }
    let deadline = Instant::now() + SHUTDOWN_WAIT;
    let mut held = lock(&REGISTRY.runs);
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
