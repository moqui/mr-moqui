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

//! A load's result (plans/LoadRunner.md section 5): the totals and the table by name from the histograms, the bins,
//! the thresholds and their verdict, the generator's share of the machine, and the console table.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use moqui_lib::histogram::Histogram;
use serde_json::{json, Map, Value};

use super::record::Shared;
use super::Kind;

/// The lines of error samples the console table shows under it.
const TABLE_SAMPLES: usize = 3;

fn ms(micros: f64) -> f64 {
    (micros / 1000.0 * 1000.0).round() / 1000.0
}

fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

/// `min`, `mean`, `p50`, `p90`, `p95`, `p99` and `max` in milliseconds. The percentiles are the histogram's, within
/// 1.6% of the exact ones.
pub(crate) fn latency(histogram: &Histogram) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("min".into(), json!(ms(histogram.min() as f64)));
    out.insert("mean".into(), json!(ms(histogram.mean())));
    for (key, p) in [("p50", 50.0), ("p90", 90.0), ("p95", 95.0), ("p99", 99.0)] {
        out.insert(key.into(), json!(ms(histogram.percentile(p) as f64)));
    }
    out.insert("max".into(), json!(ms(histogram.max() as f64)));
    out
}

/// The totals and the table by name: what a snapshot and the result share. `elapsed` is the load's time so far,
/// used when no user reached its scenario; `end` is now for a snapshot, and none for the result, whose duration ends
/// with the last user's timed part. With `with_bins`, each name has its bins.
pub(crate) fn summary(
    shared: &Shared,
    users: u64,
    elapsed: Duration,
    end: Option<Instant>,
    with_bins: bool,
) -> Map<String, Value> {
    let last = end.or(shared.last_timed);
    let duration = match (shared.first_timed, last) {
        (Some(first), Some(last)) if last > first => last - first,
        _ => elapsed,
    };
    let seconds = duration.as_secs_f64();
    let mut names = Map::new();
    let mut name_seconds = None;
    for name in &shared.order {
        let Some(stats) = shared.names.get(name) else {
            continue;
        };
        let mut entry = Map::new();
        entry.insert("kind".into(), json!(stats.kind.as_str()));
        entry.insert("count".into(), json!(stats.histogram.count()));
        entry.insert("errors".into(), json!(stats.errors));
        entry.extend(latency(&stats.histogram));
        let outcomes: Map<String, Value> = stats
            .outcomes
            .iter()
            .map(|(outcome, count)| (outcome.clone(), json!(count)))
            .collect();
        entry.insert("outcomes".into(), Value::Object(outcomes));
        if matches!(stats.kind, Kind::Http | Kind::Raw) {
            let statuses: Map<String, Value> = stats
                .statuses
                .iter()
                .map(|(status, count)| (status.to_string(), json!(count)))
                .collect();
            entry.insert("statuses".into(), Value::Object(statuses));
        }
        if with_bins {
            let (bins, seconds) = stats.series.to_json(None);
            name_seconds = Some(name_seconds.map_or(seconds, |known: u64| known.max(seconds)));
            entry.insert("bins".into(), Value::Array(bins));
            entry.insert("binSeconds".into(), json!(seconds));
        }
        names.insert(name.clone(), Value::Object(entry));
    }
    let requests = shared.all.count();
    // per attempt: each request, and each error that was not a request's (a throw, a user that failed), so the rate
    // is at most 1 and a fail() after a good request is one error in two attempts
    let attempts = requests + shared.errors.saturating_sub(shared.request_errors);
    let error_rate = if attempts > 0 {
        shared.errors as f64 / attempts as f64
    } else {
        0.0
    };
    let rps = if seconds > 0.0 { requests as f64 / seconds } else { 0.0 };
    let scenarios: Map<String, Value> = shared
        .scenarios
        .iter()
        .map(|counts| {
            let mut value = json!({ "users": counts.users, "iterations": counts.iterations, "errors": counts.errors });
            if counts.rate {
                value["dropped"] = json!(counts.dropped);
            }
            (counts.name.clone(), value)
        })
        .collect();
    let dropped: u64 = shared.scenarios.iter().map(|counts| counts.dropped).sum();
    let mut out = Map::new();
    out.insert("users".into(), json!(users));
    out.insert("duration".into(), json!(rounded(seconds, 3)));
    out.insert("iterations".into(), json!(shared.iterations));
    out.insert("dropped".into(), json!(dropped));
    out.insert("requests".into(), json!(requests));
    out.insert("errors".into(), json!(shared.errors));
    out.insert("errorRate".into(), json!(rounded(error_rate, 4)));
    out.insert("rps".into(), json!(rounded(rps, 1)));
    out.insert("latency".into(), Value::Object(latency(&shared.all)));
    out.insert("names".into(), Value::Object(names));
    if let Some(seconds) = name_seconds {
        out.insert("nameBinSeconds".into(), json!(seconds));
    }
    out.insert("scenarios".into(), Value::Object(scenarios));
    out.insert("errorSamples".into(), Value::Array(shared.error_samples.clone()));
    out
}

/// The parts of a result that are not the samples'.
pub(crate) struct Parts<'a> {
    pub(crate) users: u64,
    /// The load's whole time.
    pub(crate) wall: Duration,
    pub(crate) thresholds: &'a Value,
    pub(crate) generator: &'static str,
    /// The process's CPU time over the run, where it can be read.
    pub(crate) cpu: Option<Duration>,
    pub(crate) run: Value,
}

/// The result of a load (section 5), and the names in the order first seen, for the table.
pub(crate) fn result(shared: &Shared, parts: &Parts<'_>) -> (Value, Vec<String>) {
    let mut out = summary(shared, parts.users, parts.wall, None, true);
    let (bins, seconds) = shared.bins.to_json(None);
    out.insert("bins".into(), Value::Array(bins));
    out.insert("binSeconds".into(), json!(seconds));
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let wall = parts.wall.as_secs_f64();
    let cpu = parts
        .cpu
        .filter(|_| wall > 0.0)
        .map(|cpu| rounded(cpu.as_secs_f64() / (wall * cores as f64), 3));
    let user_time = shared.user_time.as_secs_f64();
    let script_share =
        (user_time > 0.0).then(|| rounded((1.0 - shared.wait_time.as_secs_f64() / user_time).clamp(0.0, 1.0), 3));
    out.insert(
        "generator".into(),
        json!({ "where": parts.generator, "cores": cores, "cpu": cpu, "scriptShare": script_share }),
    );
    let mut value = Value::Object(out);
    let (checked, passed) = thresholds(parts.thresholds, &value);
    if let Some(out) = value.as_object_mut() {
        out.insert("thresholds".into(), Value::Array(checked));
        out.insert("passed".into(), json!(passed));
        out.insert("run".into(), parts.run.clone());
        // the names in the order first seen, which a JSON object does not keep
        out.insert("nameOrder".into(), json!(shared.order));
    }
    (value, shared.order.clone())
}

/// The metrics a threshold can limit.
const METRICS: [&str; 8] = ["errorRate", "min", "mean", "p50", "p90", "p95", "p99", "max"];

/// Check `limits` (`{ errorRate, p95, p99, names: { "<name>": { p95 } } }`, times in milliseconds) against a result:
/// each limit with its value and whether it held, and whether all did. A name with no samples breaks its limits.
pub(crate) fn thresholds(limits: &Value, result: &Value) -> (Vec<Value>, bool) {
    let mut out = Vec::new();
    let mut check = |name: Option<&str>, limits: &Value, values: Option<&Value>| {
        for metric in METRICS {
            let Some(limit) = limits.get(metric).and_then(Value::as_f64) else {
                continue;
            };
            // with no request a run's own metrics have no value, as a name with no samples
            let empty =
                name.is_none() && values.and_then(|values| values.get("requests")).and_then(Value::as_u64) == Some(0);
            let value = values.filter(|_| !empty).and_then(|values| match metric {
                "errorRate" if name.is_some() => {
                    let count = values.get("count").and_then(Value::as_f64).unwrap_or(0.0);
                    let errors = values.get("errors").and_then(Value::as_f64).unwrap_or(0.0);
                    (count > 0.0).then(|| rounded(errors / count, 4))
                }
                "errorRate" => values.get("errorRate").and_then(Value::as_f64),
                _ if name.is_some() => values.get(metric).and_then(Value::as_f64),
                _ => values
                    .get("latency")
                    .and_then(|latency| latency.get(metric))
                    .and_then(Value::as_f64),
            });
            let passed = value.is_some_and(|value| value <= limit);
            out.push(json!({ "name": name, "metric": metric, "limit": limit, "value": value, "passed": passed }));
        }
    };
    check(None, limits, Some(result));
    if let Some(names) = limits.get("names").and_then(Value::as_object) {
        for (name, limits) in names {
            let values = result.get("names").and_then(|names| names.get(name));
            check(Some(name), limits, values);
        }
    }
    let passed = out.iter().all(|entry| entry["passed"] == json!(true));
    (out, passed)
}

/// Refuse thresholds with a metric this does not know or a limit that is not a number, before a run starts.
///
/// # Errors
///
/// Naming the key or the limit.
pub(crate) fn check_thresholds(limits: &Value) -> Result<(), String> {
    let check = |limits: &Map<String, Value>, at: &str, top: bool| -> Result<(), String> {
        for (key, limit) in limits {
            if top && key == "names" {
                continue;
            }
            if !METRICS.contains(&key.as_str()) {
                let known = if top { ", or names" } else { "" };
                return Err(format!(
                    "load: thresholds{at}: {key} is not a metric ({}{known})",
                    METRICS.join(", ")
                ));
            }
            if !limit.as_f64().is_some_and(f64::is_finite) {
                return Err(format!("load: thresholds{at}.{key} is a number, not {limit}"));
            }
        }
        Ok(())
    };
    let limits = match limits {
        Value::Null => return Ok(()),
        Value::Object(limits) => limits,
        other => {
            return Err(format!(
                "load: thresholds is {{ errorRate, p95, ..., names }}, not {other}"
            ))
        }
    };
    check(limits, "", true)?;
    match limits.get("names") {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(names)) => {
            for (name, own) in names {
                let Some(own) = own.as_object() else {
                    return Err(format!("load: thresholds.names.{name} is {{ p95, ... }}, not {own}"));
                };
                check(own, &format!(".names.{name}"), false)?;
            }
            Ok(())
        }
        Some(other) => Err(format!(
            "load: thresholds.names is {{ name: {{ p95, ... }} }}, not {other}"
        )),
    }
}

/// This process's CPU time (user and system, every thread), from `/proc/self/stat`; `None` off Linux.
pub(crate) fn process_cpu() -> Option<Duration> {
    // USER_HZ is 100 on Linux, so a tick is ten milliseconds.
    const TICK_MILLIS: u64 = 10;
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The fields after the name in parentheses: state is the first, utime and stime the 12th and 13th.
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(Duration::from_millis((utime + stime) * TICK_MILLIS))
}

/// A count with thousands separators: 2,412.
pub(crate) fn grouped(count: u64) -> String {
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
pub(crate) fn table_ms(value: &Value) -> String {
    let ms = value.as_f64().unwrap_or(0.0);
    if ms < 10.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{ms:.0}ms")
    }
}

/// The console table of a load result, its names in `order`; the first few errors under it, and a broken threshold.
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
    if number("dropped") > 0 {
        // the line's end, so that what reads its start is not moved
        out.pop();
        let _ = writeln!(out, ", {} dropped", grouped(number("dropped")));
    }
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
    let broken = stats
        .get("thresholds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry["passed"] != json!(true));
    for entry in broken {
        let metric = entry["metric"].as_str().unwrap_or("");
        let what = match entry["name"].as_str() {
            Some(name) => format!("{name} {metric}"),
            None => metric.to_string(),
        };
        let value = entry["value"]
            .as_f64()
            .map_or_else(|| "none".to_string(), |value| value.to_string());
        let _ = writeln!(out, "threshold broken: {what} {value} over {}", entry["limit"]);
    }
    out
}
