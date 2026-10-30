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

//! `load(opts)` and the client's `parallel(n, fn, args)` (plans/archive/ClientTestHarness.md 5.6): virtual users, each a
//! thread of [`moqui_devkit::testkit::run_on_thread`] with its own engine and default session, the samples of each
//! request, and the statistics, table and JSON of a load.
//!
//! A user's thread holds its state in a thread local from `thread.begin` to `thread.end`: the sessions it made, and
//! for a load the [`Recorder`] its requests are timed into. The recorder merges into the load's [`Collected`] when it
//! is dropped, at `thread.end` or when the thread exits, so a user whose engine is stopped still counts.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;
use serde_json::{json, Map, Value};

/// The errors a load result keeps whole, the first.
pub(crate) const ERROR_SAMPLES: usize = 20;

/// The lines of error samples the console table shows under it.
const TABLE_SAMPLES: usize = 3;

/// One name's samples: each latency in microseconds, its errors, and its statuses (0 for a transport error).
#[derive(Default)]
struct NameSamples {
    micros: Vec<u64>,
    errors: u64,
    statuses: BTreeMap<u16, u64>,
}

/// What a load's users collected, merged as each user ends.
#[derive(Default)]
pub(crate) struct Collected {
    /// The names in the order first seen, which is the scenario's.
    order: Vec<String>,
    names: HashMap<String, NameSamples>,
    iterations: u64,
    /// Every error: of a request, a throw in the scenario, a user whose setup or thread failed.
    errors: u64,
    error_samples: Vec<Value>,
    /// The first user's start of its timed iterations and the last user's end of them.
    first_timed: Option<Instant>,
    last_timed: Option<Instant>,
}

impl Collected {
    fn sample(&mut self, sample: Value) {
        if self.error_samples.len() < ERROR_SAMPLES {
            self.error_samples.push(sample);
        }
    }

    /// An error outside any request, such as a user whose setup threw.
    pub(crate) fn user_error(&mut self, user: u64, message: &str) {
        self.errors += 1;
        self.sample(json!({ "name": null, "status": null, "user": user, "message": message }));
    }

    fn merge(&mut self, recorder: &mut Recorder) {
        for name in recorder.order.drain(..) {
            let Some(mut samples) = recorder.names.remove(&name) else {
                continue;
            };
            let into = match self.names.get_mut(&name) {
                Some(into) => into,
                None => {
                    self.order.push(name.clone());
                    self.names.entry(name).or_default()
                }
            };
            into.micros.append(&mut samples.micros);
            into.errors += samples.errors;
            for (status, count) in samples.statuses {
                *into.statuses.entry(status).or_default() += count;
            }
        }
        self.iterations += recorder.iterations;
        self.errors += recorder.errors;
        if let Some(started) = recorder.started {
            self.first_timed = Some(self.first_timed.map_or(started, |first| first.min(started)));
        }
        if let Some(ended) = recorder.ended.or(recorder.started.map(|_| Instant::now())) {
            self.last_timed = Some(self.last_timed.map_or(ended, |last| last.max(ended)));
        }
    }
}

/// One `load` running: what its users collect, and the stop asked of them when it runs past its time.
#[derive(Default)]
pub(crate) struct Load {
    pub(crate) collected: Mutex<Collected>,
    pub(crate) stop: AtomicBool,
}

/// A load user's samples, kept on its own thread so that no request takes a lock to be counted.
struct Recorder {
    load: Arc<Load>,
    user: u64,
    iteration: u64,
    /// The scenario is running: setup and teardown are not timed.
    timed: bool,
    /// The last request's error was counted and thrown, so the scenario's throw that it causes is not counted again.
    counted: bool,
    order: Vec<String>,
    names: HashMap<String, NameSamples>,
    iterations: u64,
    errors: u64,
    started: Option<Instant>,
    ended: Option<Instant>,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let load = Arc::clone(&self.load);
        lock(&load.collected).merge(self);
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

/// `thread.begin`: this thread is a `parallel` thread, or user `user` of `load`.
pub(crate) fn begin(load: Option<Arc<Load>>, user: u64) {
    let recorder = load.map(|load| Recorder {
        load,
        user,
        iteration: 0,
        timed: false,
        counted: false,
        order: Vec::new(),
        names: HashMap::new(),
        iterations: 0,
        errors: 0,
        started: None,
        ended: None,
    });
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

/// `load.timed`: the scenario's iterations start (`on`) or end, with the count the user ran.
pub(crate) fn timed(on: bool, iterations: u64) {
    with_recorder(|recorder| {
        recorder.timed = on;
        if on {
            recorder.started.get_or_insert_with(Instant::now);
        } else {
            recorder.ended = Some(Instant::now());
            recorder.iterations = iterations;
        }
    });
}

/// `load.error`: the scenario threw in `iteration`. Counted unless the throw is a request's error already counted.
/// Answers whether the load asks its users to stop.
pub(crate) fn scenario_error(message: &str, iteration: u64) -> bool {
    with_recorder(|recorder| {
        recorder.iteration = iteration;
        if std::mem::take(&mut recorder.counted) {
            return recorder.load.stop.load(Ordering::Relaxed);
        }
        recorder.errors += 1;
        let sample = json!({ "name": null, "status": null, "user": recorder.user, "iteration": iteration,
            "message": message });
        lock(&recorder.load.collected).sample(sample);
        recorder.load.stop.load(Ordering::Relaxed)
    })
    .unwrap_or(false)
}

/// Before a request: an error when the load this thread belongs to has asked its users to stop.
pub(crate) fn check_stopped() -> Result<(), String> {
    let stopped = with_recorder(|recorder| recorder.timed && recorder.load.stop.load(Ordering::Relaxed));
    if stopped == Some(true) {
        return Err("the load stopped: it ran past its time".to_string());
    }
    Ok(())
}

/// What one request answered, for [`observe`].
pub(crate) struct Observed<'a> {
    /// The status, or `None` for a transport error.
    pub(crate) status: Option<u16>,
    /// The transport error, or the start of the body of an answer.
    pub(crate) text: &'a str,
    pub(crate) elapsed: Duration,
    /// The request's `expect`: the statuses that are not errors. Without it, 400 and up are.
    pub(crate) expect: Option<Vec<u16>>,
}

/// The default name of a request: its method and its path without the query.
pub(crate) fn default_name(method: &str, path: &str) -> String {
    let path = match reqwest::Url::parse(path) {
        Ok(url) if path.starts_with("http://") || path.starts_with("https://") => url.path().to_string(),
        _ => path.split(['?', '#']).next().unwrap_or("").to_string(),
    };
    format!("{} {path}", method.to_ascii_uppercase())
}

/// Count one request of a load user's scenario under its name. A response of an error status (see
/// [`Observed::expect`]) and a transport error are errors: they are counted here and come back as `Err`, which the
/// host throws so that the user's iteration ends. Outside a load's scenario this does nothing.
pub(crate) fn observe(name: impl FnOnce() -> String, observed: &Observed<'_>) -> Result<(), String> {
    with_recorder(|recorder| {
        if !recorder.timed {
            return Ok(());
        }
        recorder.counted = false;
        let name = name();
        if !recorder.names.contains_key(&name) {
            recorder.order.push(name.clone());
        }
        let samples = recorder.names.entry(name.clone()).or_default();
        samples
            .micros
            .push(u64::try_from(observed.elapsed.as_micros()).unwrap_or(u64::MAX));
        *samples.statuses.entry(observed.status.unwrap_or(0)).or_default() += 1;
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
        let Some(message) = error else {
            return Ok(());
        };
        samples.errors += 1;
        recorder.errors += 1;
        recorder.counted = true;
        let sample = json!({ "name": name, "status": observed.status, "user": recorder.user,
            "iteration": recorder.iteration, "message": message });
        lock(&recorder.load.collected).sample(sample);
        Err(format!("load: {name}: {message}"))
    })
    .unwrap_or(Ok(()))
}

/// The value at percentile `p` (0 to 100) of sorted samples, by nearest rank; 0 for none.
pub(crate) fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn ms(micros: f64) -> f64 {
    (micros / 1000.0 * 1000.0).round() / 1000.0
}

/// `min`, `mean`, `p50`, `p90`, `p95`, `p99` and `max` in milliseconds, of samples sorted here.
pub(crate) fn latency(micros: &mut [u64]) -> Map<String, Value> {
    micros.sort_unstable();
    let mean = if micros.is_empty() {
        0.0
    } else {
        micros.iter().map(|value| *value as f64).sum::<f64>() / micros.len() as f64
    };
    let mut out = Map::new();
    out.insert("min".into(), json!(ms(micros.first().copied().unwrap_or(0) as f64)));
    out.insert("mean".into(), json!(ms(mean)));
    for (key, p) in [("p50", 50.0), ("p90", 90.0), ("p95", 95.0), ("p99", 99.0)] {
        out.insert(key.into(), json!(ms(percentile(micros, p) as f64)));
    }
    out.insert("max".into(), json!(ms(micros.last().copied().unwrap_or(0) as f64)));
    out
}

/// The result of a load (5.6), from what its users collected over `wall` (the load's whole time, used when no user
/// reached its scenario). Answers the names in the scenario's order too, for the table.
pub(crate) fn stats(users: u64, collected: &mut Collected, wall: Duration) -> (Value, Vec<String>) {
    let duration = match (collected.first_timed, collected.last_timed) {
        (Some(first), Some(last)) if last > first => last - first,
        _ => wall,
    };
    let seconds = duration.as_secs_f64();
    let mut all = Vec::new();
    let mut names = Map::new();
    for name in &collected.order {
        let Some(samples) = collected.names.get_mut(name) else {
            continue;
        };
        all.extend_from_slice(&samples.micros);
        let mut entry = latency(&mut samples.micros);
        entry.insert("count".into(), json!(samples.micros.len()));
        entry.insert("errors".into(), json!(samples.errors));
        let statuses: Map<String, Value> = samples
            .statuses
            .iter()
            .map(|(status, count)| (status.to_string(), json!(count)))
            .collect();
        entry.insert("statuses".into(), Value::Object(statuses));
        names.insert(name.clone(), Value::Object(entry));
    }
    let requests = all.len() as u64;
    let error_rate = if requests > 0 {
        collected.errors as f64 / requests as f64
    } else if collected.errors > 0 {
        1.0
    } else {
        0.0
    };
    let rps = if seconds > 0.0 { requests as f64 / seconds } else { 0.0 };
    let value = json!({
        "users": users,
        "duration": (seconds * 1000.0).round() / 1000.0,
        "iterations": collected.iterations,
        "requests": requests,
        "errors": collected.errors,
        "errorRate": (error_rate * 10000.0).round() / 10000.0,
        "rps": (rps * 10.0).round() / 10.0,
        "latency": Value::Object(latency(&mut all)),
        "names": Value::Object(names),
        "errorSamples": collected.error_samples.clone(),
    });
    (value, collected.order.clone())
}

/// A count with thousands separators: 2,412.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Milliseconds for the table: one decimal under 10 ms, whole above.
fn table_ms(value: &Value) -> String {
    let ms = value.as_f64().unwrap_or(0.0);
    if ms < 10.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{ms:.0}ms")
    }
}

/// The console table of a load result (1), its names in `order`; the first few errors under it.
pub(crate) fn table(stats: &Value, order: &[String]) -> String {
    let number = |key: &str| stats.get(key).and_then(Value::as_u64).unwrap_or(0);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} users, {:.1} s: {} requests, {:.1}/s, {} errors",
        number("users"),
        stats.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
        grouped(number("requests")),
        stats.get("rps").and_then(Value::as_f64).unwrap_or(0.0),
        grouped(number("errors"))
    );
    let width = order.iter().map(|name| name.chars().count()).max().unwrap_or(0).max(4);
    let _ = writeln!(
        out,
        "{:<width$}  {:>7}  {:>6}  {:>5}  {:>5}  {:>5}  {:>5}",
        "name", "count", "errors", "p50", "p95", "p99", "max"
    );
    for name in order {
        let Some(entry) = stats.get("names").and_then(|names| names.get(name)) else {
            continue;
        };
        let count = |key: &str| grouped(entry.get(key).and_then(Value::as_u64).unwrap_or(0));
        let at = |key: &str| table_ms(entry.get(key).unwrap_or(&Value::Null));
        let _ = writeln!(
            out,
            "{name:<width$}  {:>7}  {:>6}  {:>5}  {:>5}  {:>5}  {:>5}",
            count("count"),
            count("errors"),
            at("p50"),
            at("p95"),
            at("p99"),
            at("max")
        );
    }
    let samples = stats
        .get("errorSamples")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for sample in samples.iter().take(TABLE_SAMPLES) {
        let name = sample.get("name").and_then(Value::as_str).unwrap_or("(scenario)");
        let message = sample.get("message").and_then(Value::as_str).unwrap_or("");
        let first = message.lines().next().unwrap_or("");
        let _ = writeln!(out, "error: {name}: {}", first.chars().take(160).collect::<String>());
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_are_by_nearest_rank_over_every_sample() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&sorted, 50.0), 50);
        assert_eq!(percentile(&sorted, 95.0), 95);
        assert_eq!(percentile(&sorted, 99.0), 99);
        assert_eq!(percentile(&sorted, 100.0), 100);
        assert_eq!(percentile(&sorted, 0.0), 1);
        assert_eq!(percentile(&[7], 99.0), 7);
        assert_eq!(percentile(&[], 50.0), 0);
        let mut micros = vec![4000, 1000, 3000, 2000];
        let out = latency(&mut micros);
        assert_eq!(out["min"], json!(1.0));
        assert_eq!(out["mean"], json!(2.5));
        assert_eq!(out["p50"], json!(2.0));
        assert_eq!(out["p90"], json!(4.0));
        assert_eq!(out["max"], json!(4.0));
    }

    #[test]
    fn a_load_user_counts_its_requests_errors_and_throws_into_the_stats() {
        let load = Arc::new(Load::default());
        let user = {
            let load = Arc::clone(&load);
            std::thread::spawn(move || {
                begin(Some(load), 0);
                let ok = |ms: u64| Observed {
                    status: Some(200),
                    text: "",
                    elapsed: Duration::from_millis(ms),
                    expect: None,
                };
                // setup is not timed
                assert!(observe(|| "GET /setup".to_string(), &ok(5)).is_ok());
                timed(true, 0);
                assert!(observe(|| default_name("get", "/rest/s1/a?x=1"), &ok(2)).is_ok());
                assert!(observe(|| "b".to_string(), &ok(4)).is_ok());
                let refused = Observed {
                    status: Some(429),
                    text: "too many",
                    elapsed: Duration::from_millis(6),
                    expect: None,
                };
                let err = observe(|| "b".to_string(), &refused).unwrap_err();
                assert_eq!(err, "load: b: HTTP 429: too many");
                // the throw the counted error causes is not counted again; a throw of the scenario's own is
                assert!(!scenario_error(&err, 0));
                assert!(!scenario_error("Error: boom", 1));
                let expected = Observed {
                    status: Some(401),
                    text: "",
                    elapsed: Duration::from_millis(8),
                    expect: Some(vec![401]),
                };
                assert!(observe(|| "b".to_string(), &expected).is_ok());
                let transport = Observed {
                    status: None,
                    text: "connection refused",
                    elapsed: Duration::from_millis(1),
                    expect: None,
                };
                assert!(observe(|| "c".to_string(), &transport).is_err());
                timed(false, 2);
                assert_eq!(end(), Vec::<u64>::new());
            })
        };
        user.join().unwrap();
        lock(&load.collected).user_error(1, "Error: login of test.load failed: HTTP 401");
        let (stats, order) = stats(2, &mut lock(&load.collected), Duration::from_secs(1));
        assert_eq!(order, vec!["GET /rest/s1/a", "b", "c"]);
        assert_eq!(stats["users"], json!(2));
        assert_eq!(stats["iterations"], json!(2));
        assert_eq!(stats["requests"], json!(5));
        // the 429, the throw, the transport error and the user whose setup failed
        assert_eq!(stats["errors"], json!(4));
        assert_eq!(stats["errorRate"], json!(0.8));
        assert_eq!(stats["latency"]["max"], json!(8.0));
        let b = &stats["names"]["b"];
        assert_eq!(b["count"], json!(3));
        assert_eq!(b["errors"], json!(1));
        assert_eq!(b["statuses"], json!({ "200": 1, "401": 1, "429": 1 }));
        assert_eq!(b["p50"], json!(6.0));
        assert_eq!(stats["names"]["c"]["statuses"], json!({ "0": 1 }));
        let samples = stats["errorSamples"].as_array().unwrap();
        assert_eq!(samples.len(), 4);
        assert_eq!(samples[0]["name"], json!("b"));
        assert_eq!(samples[0]["status"], json!(429));
        assert_eq!(samples[1]["message"], json!("Error: boom"));
        assert_eq!(samples[3]["user"], json!(1));
        let text = table(&stats, &order);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("2 users, "), "{text}");
        assert!(lines[0].contains(": 5 requests, "), "{text}");
        assert!(lines[0].ends_with(", 4 errors"), "{text}");
        assert_eq!(lines[1], "name              count  errors    p50    p95    p99    max");
        assert_eq!(lines[3], "b                     3       1  6.0ms  8.0ms  8.0ms  8.0ms");
        assert_eq!(lines[5], "error: b: HTTP 429: too many");
        assert_eq!(lines.len(), 8, "{text}");
    }

    #[test]
    fn counts_names_and_file_parts() {
        assert_eq!(grouped(2412), "2,412");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000_000), "1,000,000");
        assert_eq!(default_name("post", "/apps/setPreference"), "POST /apps/setPreference");
        assert_eq!(default_name("GET", "http://localhost:8080/rest/x?y=1"), "GET /rest/x");
        assert_eq!(file_part("mixed reads and writes"), "mixed-reads-and-writes");
        assert_eq!(file_part("a/b: \"c\""), "a_b_-_c_");
    }
}
