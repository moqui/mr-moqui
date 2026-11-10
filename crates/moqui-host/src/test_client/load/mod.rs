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

//! The load engine (plans/LoadRunner.md section 5; plans/archive/ClientTestHarness.md 5.6): a [`Load`]'s virtual
//! users, each a thread of [`moqui_devkit::testkit::run_script_on_thread`] with its own engine and default session,
//! the samples they record, the live [`Load::snapshot`] and the two stops, and the result. `load(opts)` of a client
//! spec, `moqui test-load` and a run in a server all start users with [`start`].
//!
//! A user's thread holds its state in a thread local from `thread.begin` to `thread.end`: the sessions it made, and
//! for a load the [`record::Recorder`] its samples are kept in. The recorder merges into the load's shared state at
//! most once a second and when it is dropped, at `thread.end` or when the thread exits, so a user whose engine is
//! stopped still counts.

mod bins;
mod pace;
mod record;
pub(crate) mod script;
mod stats;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_devkit::testkit::{run_script_on_thread, AroundThread, ScriptFile, ScriptThread, SpecHost};
use moqui_lib::sync::lock;
use serde_json::{json, Map, Value};

pub(crate) use pace::{seconds, Pause};
use record::{Pending, Recorder, ScenarioCounts, Shared};
pub(crate) use script::{LoadScript, ScenarioSettings};
pub(crate) use stats::{grouped, table, table_ms};

/// The errors a load result keeps whole, the first.
pub(crate) const ERROR_SAMPLES: usize = 20;

/// The `User-Agent` of a load user's requests.
pub(crate) const LOAD_AGENT: &str = concat!("moqui-load/", env!("CARGO_PKG_VERSION"));

/// The most users a load runs.
pub(crate) const MAX_USERS: u64 = 10_000;

/// What a sample timed: the statistics are the same for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Http,
    /// `http.raw`.
    Raw,
    /// `service.call` in a server's process.
    #[allow(dead_code)] // for `moqui test-load` and a run in a server (plans/LoadRunner.md L2, L3)
    Service,
    /// `step(name, fn)`: the whole of `fn`, whose requests are counted under their own names too.
    Step,
}

impl Kind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Kind::Http => "http",
            Kind::Raw => "raw",
            Kind::Service => "service",
            Kind::Step => "step",
        }
    }
}

/// A load's stop, which only goes up.
const RUNNING: u8 = 0;
/// Each user ends after its current iteration.
const AFTER_ITERATION: u8 = 1;
/// Each user ends before its next request.
const NOW: u8 = 2;
/// The users ran past the load's timeout: as `NOW`, and the refused requests are errors.
const TIMED_OUT: u8 = 3;

/// One load running: what its users merged, and the stop asked of them.
pub(crate) struct Load {
    id: u64,
    pub(crate) started: Instant,
    started_at: jiff::Timestamp,
    pub(crate) shared: Mutex<Shared>,
    stop: AtomicU8,
    /// The users in their timed part.
    pub(crate) active: AtomicU64,
    /// The error samples taken, so that an error past the first few takes no lock for its sample.
    error_samples: AtomicUsize,
    users: u64,
    /// Each user's oldest second not merged yet ([`record::Recorder`]): a second stays open while one holds it.
    pub(crate) holds: Mutex<Vec<Arc<AtomicU64>>>,
}

/// The loads running in this process, by id, for their users' `thread.begin`.
static LOADS: LazyLock<Mutex<HashMap<u64, Arc<Load>>>> = LazyLock::new(Mutex::default);
static NEXT_LOAD: AtomicU64 = AtomicU64::new(1);

impl Load {
    /// A load of `scenarios`, in the registry until its run ends.
    pub(crate) fn new(scenarios: &[ScenarioSettings]) -> Arc<Load> {
        let shared = Shared {
            scenarios: scenarios
                .iter()
                .map(|scenario| ScenarioCounts {
                    name: scenario.name.clone(),
                    users: scenario.users,
                    ..ScenarioCounts::default()
                })
                .collect(),
            ..Shared::default()
        };
        let load = Arc::new(Load {
            id: NEXT_LOAD.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            started_at: jiff::Timestamp::now(),
            shared: Mutex::new(shared),
            stop: AtomicU8::new(RUNNING),
            active: AtomicU64::new(0),
            error_samples: AtomicUsize::new(0),
            users: scenarios.iter().map(|scenario| scenario.users).sum(),
            holds: Mutex::new(Vec::new()),
        });
        lock(&LOADS).insert(load.id, Arc::clone(&load));
        load
    }

    /// A load running in this process.
    pub(crate) fn find(id: u64) -> Option<Arc<Load>> {
        lock(&LOADS).get(&id).cloned()
    }

    fn unregister(&self) {
        lock(&LOADS).remove(&self.id);
    }

    #[allow(dead_code)] // for `moqui test-load` and a run in a server (plans/LoadRunner.md L2, L3)
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    #[allow(dead_code)] // for `moqui test-load` and a run in a server (plans/LoadRunner.md L2, L3)
    pub(crate) fn users(&self) -> u64 {
        self.users
    }

    pub(crate) fn active_users(&self) -> u64 {
        self.active.load(Ordering::Relaxed)
    }

    /// Each user ends after its current iteration (and its teardown runs).
    pub(crate) fn stop_after_iteration(&self) {
        self.stop.fetch_max(AFTER_ITERATION, Ordering::Relaxed);
    }

    /// Each user ends before its next request: a request then throws, and is not counted as an error.
    pub(crate) fn stop_now(&self) {
        self.stop.fetch_max(NOW, Ordering::Relaxed);
    }

    fn stop_timed_out(&self) {
        self.stop.fetch_max(TIMED_OUT, Ordering::Relaxed);
    }

    fn stop_state(&self) -> u8 {
        self.stop.load(Ordering::Relaxed)
    }

    /// Why the run stopped, for its `run.stopReason`: `stop`, `stop-now` or `timeout`; none when it was not stopped.
    fn stop_reason(&self) -> Option<&'static str> {
        match self.stop_state() {
            AFTER_ITERATION => Some("stop"),
            NOW => Some("stop-now"),
            TIMED_OUT => Some("timeout"),
            _ => None,
        }
    }

    /// The stop asked, if any: `after-iteration`, `now` or `timed-out`.
    pub(crate) fn stopping(&self) -> Option<&'static str> {
        match self.stop_state() {
            AFTER_ITERATION => Some("after-iteration"),
            NOW => Some("now"),
            TIMED_OUT => Some("timed-out"),
            _ => None,
        }
    }

    /// Note the active users of this second, and close the seconds that every user has merged.
    pub(crate) fn tick(&self) {
        self.tick_at(self.started.elapsed().as_secs());
    }

    /// [`Load::tick`] at `second` from the start. A second a user still holds samples of stays open.
    pub(crate) fn tick_at(&self, second: u64) {
        let active = self.active_users();
        let held = lock(&self.holds)
            .iter()
            .map(|hold| hold.load(Ordering::Relaxed))
            .min()
            .unwrap_or(u64::MAX);
        let mut shared = lock(&self.shared);
        shared.bins.note_users(second, active);
        shared
            .bins
            .close_before(second.saturating_sub(bins::CLOSE_LAG).min(held));
    }

    /// The load as it is now, from what its users have merged (each at most a second ago): the totals, the table by
    /// name, the bins thinned to at most 300 points, and the active users. Readable at any time.
    pub(crate) fn snapshot(&self) -> Value {
        self.tick();
        let shared = lock(&self.shared);
        let mut out = stats::summary(&shared, self.users, self.started.elapsed(), Some(Instant::now()), false);
        let (bins, seconds) = shared.bins.to_json(Some(bins::SNAPSHOT_POINTS));
        out.insert("bins".into(), Value::Array(bins));
        out.insert("binSeconds".into(), json!(seconds));
        out.insert("activeUsers".into(), json!(self.active_users()));
        out.insert("elapsed".into(), json!(self.started.elapsed().as_secs_f64()));
        out.insert("stopping".into(), json!(self.stopping()));
        out.insert("start".into(), json!(self.started_at.to_string()));
        Value::Object(out)
    }
}

/// A user thread's state from `thread.begin` to `thread.end`.
#[derive(Default)]
struct ThreadState {
    /// The sessions the thread made, dropped when it ends.
    sessions: Vec<u64>,
    recorder: Option<Recorder>,
}

thread_local! {
    static THREAD: RefCell<Option<ThreadState>> = const { RefCell::new(None) };
}

/// `thread.begin`: this thread is a `parallel` thread, or user `user` of scenario `scenario` of `load`, with that
/// scenario's `pause` before each request.
pub(crate) fn begin(load: Option<Arc<Load>>, scenario: usize, user: u64, pause: Option<Pause>) {
    let recorder = load.map(|load| Recorder::new(load, scenario, user, pause));
    THREAD.with(|cell| {
        *cell.borrow_mut() = Some(ThreadState {
            sessions: Vec::new(),
            recorder,
        })
    });
}

/// `thread.end`: merge the samples into the load, and answer the sessions the thread made.
pub(crate) fn end() -> Vec<u64> {
    THREAD
        .with(|cell| cell.borrow_mut().take())
        .map(|state| state.sessions)
        .unwrap_or_default()
}

/// A session made on this thread, when it is a `parallel` or load thread.
pub(crate) fn track_session(id: u64) {
    THREAD.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            state.sessions.push(id);
        }
    });
}

fn with_recorder<T>(f: impl FnOnce(&mut Recorder) -> T) -> Option<T> {
    THREAD.with(|cell| {
        cell.borrow_mut()
            .as_mut()
            .and_then(|state| state.recorder.as_mut())
            .map(f)
    })
}

/// This thread is a load's user.
pub(crate) fn is_user() -> bool {
    with_recorder(|_| ()).is_some()
}

/// `load.timed`: the scenario's iterations start (`on`) or end. Answers whether the load asks its users to stop.
pub(crate) fn timed(on: bool) -> bool {
    with_recorder(|recorder| {
        recorder.set_timed(on);
        recorder.load.stop_state() != RUNNING
    })
    .unwrap_or(false)
}

/// `load.error`: the scenario threw in `iteration`. Counted unless the throw is a request's error already counted,
/// or the refusal of a stopped load. Answers whether the load asks its users to stop.
pub(crate) fn scenario_error(message: &str, iteration: u64) -> bool {
    with_recorder(|recorder| {
        recorder.iteration = iteration;
        let stop = recorder.load.stop_state() != RUNNING;
        if recorder
            .counted
            .take()
            .is_some_and(|counted| message.contains(&counted))
        {
            return stop;
        }
        recorder.count_error();
        take_sample(recorder, || {
            let scenario = scenario_name(recorder);
            json!({ "name": null, "status": null, "scenario": scenario, "user": recorder.user,
                "iteration": iteration, "message": message })
        });
        stop
    })
    .unwrap_or(false)
}

/// `load.iterated`: an iteration ended. Answers whether the load asks its users to stop.
pub(crate) fn iterated(iteration: u64) -> bool {
    with_recorder(|recorder| {
        recorder.iteration = iteration;
        recorder.counted = None;
        recorder.count_iteration();
        recorder.load.stop_state() != RUNNING
    })
    .unwrap_or(false)
}

fn scenario_name(recorder: &Recorder) -> Option<String> {
    lock(&recorder.load.shared)
        .scenarios
        .get(recorder.scenario)
        .map(|counts| counts.name.clone())
}

/// An error sample into the load, while it has fewer than [`ERROR_SAMPLES`].
fn take_sample(recorder: &Recorder, sample: impl FnOnce() -> Value) {
    if recorder.load.error_samples.fetch_add(1, Ordering::Relaxed) < ERROR_SAMPLES {
        let sample = sample();
        lock(&recorder.load.shared).sample(sample);
    }
}

/// The refusal of a request after a stop now.
const STOPPED: &str = "the load was stopped";

/// Before a request: an error when the load this thread belongs to has asked its users to stop now.
pub(crate) fn check_stopped() -> Result<(), String> {
    let refused = with_recorder(|recorder| {
        if !recorder.timed {
            return None;
        }
        match recorder.load.stop_state() {
            NOW => {
                // a stop asked for is not the server's error
                recorder.counted = Some(STOPPED.to_string());
                Some(STOPPED)
            }
            TIMED_OUT => Some("the load stopped: it ran past its time"),
            _ => None,
        }
    })
    .flatten();
    match refused {
        Some(message) => Err(message.to_string()),
        None => Ok(()),
    }
}

/// Before a request of a load user's scenario: the scenario's `pause`, then [`check_stopped`].
pub(crate) fn before_request() -> Result<(), String> {
    let pause = with_recorder(|recorder| {
        recorder.merge_when_due(Duration::ZERO);
        recorder.timed.then(|| recorder.pause.clone()).flatten()
    })
    .flatten();
    if let Some(pause) = pause {
        wait(&pace::draw(&pause), None);
    }
    check_stopped()
}

/// `pause(...)` and the think time: a wait that is not timed, until `until` at the latest. In a load it ends when
/// the load stops now.
pub(crate) fn pause(pause: &Pause, until: Option<Instant>) {
    wait(&pace::draw(pause), until);
}

fn wait(duration: &Duration, until: Option<Instant>) {
    let load = with_recorder(|recorder| {
        recorder.merge_when_due(*duration);
        Arc::clone(&recorder.load)
    });
    let slept = match &load {
        Some(load) => pace::sleep(*duration, until, &|| load.stop_state() >= NOW),
        None => pace::sleep(*duration, until, &|| false),
    };
    with_recorder(|recorder| {
        recorder.paused += slept;
        if recorder.timed {
            recorder.waited += slept;
        }
    });
}

/// `load.ramp`: wait for this user's start, at `until`, ending early on any stop. Answers whether the load asks
/// its users to stop, in which case the user runs neither its setup nor its scenario.
pub(crate) fn ramp(until: Instant) -> bool {
    let Some(load) = with_recorder(|recorder| Arc::clone(&recorder.load)) else {
        pace::sleep(until.saturating_duration_since(Instant::now()), None, &|| false);
        return false;
    };
    let stopped = || load.stop_state() != RUNNING;
    pace::sleep(until.saturating_duration_since(Instant::now()), None, &stopped);
    stopped()
}

/// `step.begin`: a step starts.
pub(crate) fn step_begin() {
    with_recorder(|recorder| recorder.steps.push((Instant::now(), recorder.paused)));
}

/// `step.end`: the step that began last ends, an error with `error` when its function threw. Its sample is kind
/// `step`; its error is not counted in the load's errors, since the request or throw inside it was.
pub(crate) fn step_end(name: &str, error: Option<&str>) {
    // the pauses inside the step are not its time
    let Some(Some(elapsed)) = with_recorder(|recorder| {
        recorder
            .steps
            .pop()
            .map(|(started, paused)| started.elapsed().saturating_sub(recorder.paused.saturating_sub(paused)))
    }) else {
        return;
    };
    let sample = Sample {
        kind: Kind::Step,
        outcome: if error.is_some() { "error" } else { "ok" },
        status: None,
        elapsed,
        error: error.map(str::to_string),
    };
    let _ = record(|| name.to_string(), sample);
}

/// One sample, of any kind, for [`record`].
pub(crate) struct Sample<'a> {
    pub(crate) kind: Kind,
    /// The outcome label: an HTTP status, `ok`, `error`.
    pub(crate) outcome: &'a str,
    /// For HTTP, the status for `statuses`: 0 for a transport error.
    pub(crate) status: Option<u16>,
    pub(crate) elapsed: Duration,
    /// The error's message; none when the sample is not an error.
    pub(crate) error: Option<String>,
}

/// Count one sample of a load user's scenario under its name. An error (except a step's) is counted here and comes
/// back as `Err`, which the host throws so that the user's iteration ends. Outside a load's scenario this does nothing.
pub(crate) fn record(name: impl FnOnce() -> String, sample: Sample<'_>) -> Result<(), String> {
    with_recorder(|recorder| {
        if !recorder.timed {
            return Ok(());
        }
        let name = name();
        let micros = u64::try_from(sample.elapsed.as_micros()).unwrap_or(u64::MAX);
        let second = recorder.load.started.elapsed().as_secs();
        if sample.kind != Kind::Step {
            recorder.waited += sample.elapsed;
        }
        let error = sample.error.is_some();
        recorder.push(Pending {
            name: name.clone(),
            kind: sample.kind,
            outcome: sample.outcome.to_string(),
            status: sample.status,
            micros,
            second,
            error,
        });
        let Some(message) = sample.error else {
            return Ok(());
        };
        if sample.kind == Kind::Step {
            return Ok(());
        }
        recorder.count_error();
        let thrown = format!("load: {name}: {message}");
        recorder.counted = Some(thrown.clone());
        take_sample(recorder, || {
            let scenario = scenario_name(recorder);
            json!({ "name": name, "status": sample.status.filter(|status| *status > 0), "scenario": scenario,
                "user": recorder.user, "iteration": recorder.iteration, "message": message })
        });
        Err(thrown)
    })
    .unwrap_or(Ok(()))
}

/// What one HTTP request answered, for [`observe`].
pub(crate) struct Observed<'a> {
    /// The status, or `None` for a transport error.
    pub(crate) status: Option<u16>,
    /// The transport error, or the start of the body of an answer.
    pub(crate) text: &'a str,
    pub(crate) elapsed: Duration,
    /// The request's `expect`: the statuses that are not errors. Without it, 400 and up are.
    pub(crate) expect: Option<Vec<u16>>,
}

/// Count one HTTP request (`kind` [`Kind::Http`] or [`Kind::Raw`]) of a load user's scenario under its name: a
/// response of an error status (see [`Observed::expect`]) and a transport error are errors.
pub(crate) fn observe(name: impl FnOnce() -> String, kind: Kind, observed: &Observed<'_>) -> Result<(), String> {
    let error = match observed.status {
        None => Some(observed.text.to_string()),
        Some(status) => {
            let refused = match &observed.expect {
                Some(expect) => !expect.contains(&status),
                None => status >= 400,
            };
            refused.then(|| {
                let start: String = observed.text.chars().take(200).collect();
                format!("HTTP {status}: {}", start.trim())
            })
        }
    };
    let outcome = observed
        .status
        .map_or_else(|| "error".to_string(), |status| status.to_string());
    let sample = Sample {
        kind,
        outcome: &outcome,
        status: Some(observed.status.unwrap_or(0)),
        elapsed: observed.elapsed,
        error,
    };
    record(name, sample)
}

/// Milliseconds since the Unix epoch as a time on this clock: `endAt` of a user's spec.
fn instant_of_epoch_ms(ms: f64) -> Instant {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
    let ahead = (ms - now_ms).max(0.0);
    Instant::now() + pace::seconds(ahead / 1000.0)
}

/// The host calls of the load's users, which need no host state; `None` for any other call.
pub(crate) fn host_call(name: &str, args: &Value) -> Option<Result<Value, String>> {
    let field = |key: &str| args.get(key).unwrap_or(&Value::Null);
    let out = match name {
        "thread.begin" => Pause::parse(field("pause"), "pause").and_then(|pause| {
            let load = match field("load").as_u64() {
                // a user that starts after its load's run ended would run unrecorded and unstopped
                Some(id) => Some(Load::find(id).ok_or("the load this user belongs to has ended")?),
                None => None,
            };
            let scenario = field("scenario").as_u64().unwrap_or(0) as usize;
            begin(load, scenario, field("user").as_u64().unwrap_or(0), pause);
            Ok(Value::Null)
        }),
        "load.ramp" => {
            let until = field("until").as_f64().map_or_else(Instant::now, instant_of_epoch_ms);
            Ok(json!({ "stop": ramp(until) }))
        }
        "load.timed" => Ok(json!({ "stop": timed(field("on").as_bool().unwrap_or(false)) })),
        "load.error" => {
            let message = field("message").as_str().unwrap_or("");
            let stop = scenario_error(message, field("iteration").as_u64().unwrap_or(0));
            Ok(json!({ "stop": stop }))
        }
        "load.iterated" => Ok(json!({ "stop": iterated(field("iteration").as_u64().unwrap_or(0)) })),
        "load.pause" => Pause::parse(field("pause"), "pause").map(|parsed| {
            if let Some(parsed) = parsed {
                pause(&parsed, field("until").as_f64().map(instant_of_epoch_ms));
            }
            Value::Null
        }),
        "step.begin" => {
            step_begin();
            Ok(Value::Null)
        }
        "step.end" => {
            step_end(field("name").as_str().unwrap_or("step"), field("error").as_str());
            Ok(Value::Null)
        }
        _ => return None,
    };
    Some(out)
}

/// The default name of a request: its method and its path without the query.
pub(crate) fn default_name(method: &str, path: &str) -> String {
    let path = match reqwest::Url::parse(path) {
        Ok(url) if path.starts_with("http://") || path.starts_with("https://") => url.path().to_string(),
        _ => path.split(['?', '#']).next().unwrap_or("").to_string(),
    };
    format!("{} {path}", method.to_ascii_uppercase())
}

/// A file name part from a spec or feature name: letters, digits, `.`, `_` and `-`, with spaces as `-`.
pub(crate) fn file_part(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            ch if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') => ch,
            ch if ch.is_whitespace() => '-',
            _ => '_',
        })
        .collect()
}

type Handle = JoinHandle<Result<String, String>>;

/// Wait for `running` threads until `timeout`, calling `poll` every 10 ms; then `on_timeout`, and 10 seconds more.
/// Each thread's answer by its index, or why it has none.
pub(crate) fn join_all(
    mut running: Vec<(u64, Handle)>,
    timeout: Duration,
    on_timeout: &dyn Fn(),
    poll: &dyn Fn(),
) -> Vec<(u64, Result<String, String>)> {
    let deadline = Instant::now() + timeout;
    let mut stopping = false;
    let mut out = Vec::new();
    while !running.is_empty() {
        let (done, left): (Vec<_>, Vec<_>) = running.into_iter().partition(|(_, handle)| handle.is_finished());
        running = left;
        for (index, handle) in done {
            let answer = handle.join().unwrap_or_else(|_| Err("the thread panicked".to_string()));
            out.push((index, answer));
        }
        if running.is_empty() {
            break;
        }
        poll();
        let now = Instant::now();
        if now >= deadline && !stopping {
            stopping = true;
            on_timeout();
        }
        if now >= deadline + Duration::from_secs(10) {
            // Left to end on their own: the spec's sessions close after it, so their requests fail.
            for (index, _) in running.drain(..) {
                out.push((
                    index,
                    Err(format!("still running {}s after the start", timeout.as_secs())),
                ));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    out.sort_by_key(|(index, _)| *index);
    out
}

/// What a load's users run.
pub(crate) enum Source {
    /// `load({ setup, scenario, teardown, args })`: the functions' text, called with the user's spec.
    Functions { source: String, args: Value },
    /// A load script, evaluated whole in each user's engine with the run's params (section 4).
    Script { script: Arc<LoadScript>, params: Value },
}

/// Makes the [`AroundThread`] of each user's thread: what a run in a server uses to enter an ExecutionContext there.
pub(crate) type Around = Arc<dyn Fn() -> AroundThread + Send + Sync>;

/// A load to [`start`].
pub(crate) struct Start {
    pub(crate) scenarios: Vec<ScenarioSettings>,
    pub(crate) source: Source,
    /// Seconds before the users are told to stop; by default the longest scenario's duration and ramp and 120
    /// seconds, or 600 with only iterations.
    pub(crate) timeout: Option<f64>,
    /// `{ errorRate, p95, p99, ..., names: { "<name>": { p95, ... } } }`, checked at the end.
    pub(crate) thresholds: Value,
    /// The result's `run` block: `script`, `target`, `label`, `params` (without secrets). The scenarios, start, end and
    /// status are added at the end.
    pub(crate) run: Map<String, Value>,
    /// Where the generator runs, for the result: `same-machine` (the client) or `in-target` (a run in the server).
    pub(crate) generator: &'static str,
    /// Wraps each user's thread, as a run in a server enters an ExecutionContext on it; none for a client.
    pub(crate) around: Option<Around>,
    /// After this long the users are asked to stop after their current iteration (`load({ stopAfter })`).
    pub(crate) stop_after: Option<Duration>,
    /// After this long the users are asked to stop before their next request (`load({ stopNowAfter })`).
    pub(crate) stop_now_after: Option<Duration>,
}

/// A load whose users are running: its [`Load`] for the snapshot and stops, and [`Running::wait`] for its result.
pub(crate) struct Running {
    load: Arc<Load>,
    handles: Vec<(u64, Handle)>,
    /// The scenario and user number of each thread, by index.
    users: Vec<(usize, u64)>,
    /// The threads that did not start.
    failed: Vec<(u64, String)>,
    timeout: Duration,
    thresholds: Value,
    run: Map<String, Value>,
    generator: &'static str,
    scenarios: Vec<ScenarioSettings>,
    cpu: Option<Duration>,
    stop_after: Option<Duration>,
    stop_now_after: Option<Duration>,
}

/// Start the users of every scenario of `start` together, each scenario with its own ramp and its users numbered
/// from 0, and answer at once.
///
/// # Errors
///
/// When there is no scenario, or more than [`MAX_USERS`] users.
pub(crate) fn start(host: &Arc<dyn SpecHost>, start: Start) -> Result<Running, String> {
    let Start {
        scenarios,
        source,
        timeout,
        thresholds,
        run,
        generator,
        around,
        stop_after,
        stop_now_after,
    } = start;
    if scenarios.is_empty() {
        return Err("load: there is no scenario to run".to_string());
    }
    stats::check_thresholds(&thresholds)?;
    let total: u64 = scenarios.iter().map(|scenario| scenario.users).sum();
    if total > MAX_USERS {
        return Err(format!("load: {total} users is more than {MAX_USERS}"));
    }
    let timeout = timeout
        .filter(|seconds| *seconds > 0.0)
        .unwrap_or_else(|| scenarios.iter().map(ScenarioSettings::timeout).fold(0.0, f64::max));
    let load = Load::new(&scenarios);
    let cpu = stats::process_cpu();
    let start_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
    let mut running = Running {
        load: Arc::clone(&load),
        handles: Vec::new(),
        users: Vec::new(),
        failed: Vec::new(),
        timeout: pace::seconds(timeout),
        thresholds,
        run,
        generator,
        scenarios: scenarios.clone(),
        cpu,
        stop_after,
        stop_now_after,
    };
    for (index, scenario) in scenarios.iter().enumerate() {
        for user in 0..scenario.users {
            let start_at = start_ms + scenario.ramp_up * 1000.0 * user as f64 / scenario.users as f64;
            let mut spec = json!({
                "load": load.id, "scenario": index, "scenarioName": scenario.name, "user": user, "startAt": start_at,
                "endAt": scenario.duration.map(|seconds| start_ms + seconds * 1000.0),
                "iterations": scenario.iterations, "think": scenario.think.as_ref().map(Pause::to_json),
                "pause": scenario.pause.as_ref().map(Pause::to_json), "newSession": scenario.new_session,
            });
            let thread = match &source {
                Source::Functions { source, args } => {
                    spec["args"] = args.clone();
                    ScriptThread {
                        call: source.clone(),
                        ..ScriptThread::default()
                    }
                }
                Source::Script { script, params } => ScriptThread {
                    before: script::begin_code(params, &Value::Null),
                    file: Some(ScriptFile {
                        display: script.display.clone(),
                        source: script.source.clone(),
                    }),
                    call: "(spec) => __moquiThread.script(spec)".to_string(),
                    ..ScriptThread::default()
                },
            };
            let thread = ScriptThread {
                name: format!("moqui-load-{index}-{user}"),
                args_json: json!([spec]).to_string(),
                after: script::CLOSE_CODE.to_string(),
                around: around.as_ref().map(|around| around()),
                ..thread
            };
            let at = running.users.len() as u64;
            running.users.push((index, user));
            match run_script_on_thread(Arc::clone(host), thread) {
                Ok(handle) => running.handles.push((at, handle)),
                Err(err) => running.failed.push((at, format!("the thread did not start: {err}"))),
            }
        }
    }
    Ok(running)
}

impl Running {
    pub(crate) fn load(&self) -> &Arc<Load> {
        &self.load
    }

    /// Wait for the users, telling them to stop at the timeout, and answer the result (section 5) with the names in
    /// the order first seen, for the table.
    pub(crate) fn wait(mut self) -> (Value, Vec<String>) {
        let handles = std::mem::take(&mut self.handles);
        let load = Arc::clone(&self.load);
        let (stop_after, stop_now_after) = (self.stop_after, self.stop_now_after);
        let poll = || {
            let elapsed = load.started.elapsed();
            if stop_after.is_some_and(|after| elapsed >= after) {
                load.stop_after_iteration();
            }
            if stop_now_after.is_some_and(|after| elapsed >= after) {
                load.stop_now();
            }
            load.tick();
        };
        let answers = join_all(handles, self.timeout, &|| load.stop_timed_out(), &poll);
        let elapsed = load.started.elapsed();
        let mut shared = lock(&load.shared);
        let failed = std::mem::take(&mut self.failed);
        let errors = answers
            .into_iter()
            .filter_map(|(index, answer)| answer.err().map(|err| (index, err)))
            .chain(failed);
        for (index, err) in errors {
            let (scenario, user) = self.users.get(index as usize).copied().unwrap_or_default();
            tracing::warn!("load user {user}: {err}");
            shared.user_error(scenario, user, &err);
        }
        let second = elapsed.as_secs();
        shared.bins.note_users(second, load.active_users());
        shared.bins.close_before(second + 1);
        let cpu = match (self.cpu, stats::process_cpu()) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        let mut run = std::mem::take(&mut self.run);
        let scenarios: Map<String, Value> = self
            .scenarios
            .iter()
            .map(|scenario| (scenario.name.clone(), scenario.to_json()))
            .collect();
        run.insert("scenarios".into(), Value::Object(scenarios));
        run.insert("start".into(), json!(load.started_at.to_string()));
        run.insert("end".into(), json!(jiff::Timestamp::now().to_string()));
        let reason = load.stop_reason();
        run.insert(
            "status".into(),
            json!(if reason.is_some() { "stopped" } else { "completed" }),
        );
        run.insert("stopReason".into(), json!(reason));
        let parts = stats::Parts {
            users: load.users,
            wall: elapsed,
            thresholds: &self.thresholds,
            generator: self.generator,
            cpu,
            run: Value::Object(run),
        };
        let out = stats::result(&shared, &parts);
        drop(shared);
        load.unregister();
        out
    }
}

impl Drop for Running {
    /// A run dropped without [`Running::wait`] stops its users: those already begun see the stop, and one that has
    /// not begun is refused, since the load is out of the registry.
    fn drop(&mut self) {
        if !self.handles.is_empty() {
            self.load.stop_now();
        }
        self.load.unregister();
    }
}

#[cfg(test)]
mod tests;
