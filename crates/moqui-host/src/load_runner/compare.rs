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

//! Load results side by side (plans/LoadRunner.md §7 `compare#LoadRuns`, L5), for the service and for
//! `moqui test-load --compare`: the summary's rows, then each request name's, every value after the first with its
//! change from the first in percent.

use serde_json::{json, Value};

/// The summary rows: the metric and where it is in a result.
const SUMMARY: [(&str, &[&str]); 7] = [
    ("requests", &["requests"]),
    ("rps", &["rps"]),
    ("errors", &["errors"]),
    ("errorRate", &["errorRate"]),
    ("p50", &["latency", "p50"]),
    ("p95", &["latency", "p95"]),
    ("p99", &["latency", "p99"]),
];

/// The rows of each request name: the metric, which is also its key under `names.<name>`.
const NAME_METRICS: [&str; 5] = ["count", "p50", "p95", "p99", "errors"];

/// One row of a comparison.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// `summary` or `name`.
    pub(crate) section: &'static str,
    /// The request name of a `name` row.
    pub(crate) name: Option<String>,
    pub(crate) metric: &'static str,
    /// One per result, none where the result has no value.
    pub(crate) values: Vec<Option<f64>>,
    /// None for the first result, and where either value is missing or the first is 0; else the percent change from
    /// the first, to a tenth.
    pub(crate) changes: Vec<Option<f64>>,
}

impl Row {
    pub(crate) fn to_json(&self) -> Value {
        let numbers = |list: &[Option<f64>]| list.iter().map(|value| number(*value)).collect::<Vec<Value>>();
        json!({ "section": self.section, "name": self.name, "metric": self.metric, "values": numbers(&self.values),
            "changes": numbers(&self.changes) })
    }
}

/// A whole value as an integer (a count reads as 40, not 40.0), any other as it is.
fn number(value: Option<f64>) -> Value {
    match value {
        Some(value) if value.fract() == 0.0 && value.abs() < 1e15 => json!(value as i64),
        Some(value) => json!(value),
        None => Value::Null,
    }
}

fn at(result: &Value, path: &[&str]) -> Option<f64> {
    path.iter()
        .try_fold(result, |value, key| value.get(*key))
        .and_then(Value::as_f64)
}

/// The change of `value` from `first` in percent, to a tenth.
fn change(first: Option<f64>, value: Option<f64>) -> Option<f64> {
    match (first, value) {
        (Some(first), Some(value)) if first != 0.0 => Some(((value - first) / first * 1000.0).round() / 10.0),
        _ => None,
    }
}

fn row(section: &'static str, name: Option<&str>, metric: &'static str, values: Vec<Option<f64>>) -> Row {
    let first = values.first().copied().flatten();
    let changes = values
        .iter()
        .enumerate()
        .map(|(index, value)| if index == 0 { None } else { change(first, *value) })
        .collect();
    Row {
        section,
        name: name.map(str::to_string),
        metric,
        values,
        changes,
    }
}

/// The request names of `results`: each result's `nameOrder` (the order first seen; a JSON object keeps none) then
/// its other names, the first result's first, then the names only a later one has.
pub(crate) fn names(results: &[Value]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for result in results {
        let ordered = result
            .get("nameOrder")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        let rest = result
            .get("names")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(name, _)| name.as_str());
        for name in ordered.chain(rest) {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// The comparison of `results`, the first the base: the summary's rows, then each name's.
pub(crate) fn rows(results: &[Value]) -> Vec<Row> {
    let mut out: Vec<Row> = SUMMARY
        .iter()
        .map(|(metric, path)| {
            let values = results.iter().map(|result| at(result, path)).collect();
            row("summary", None, metric, values)
        })
        .collect();
    for name in names(results) {
        for metric in NAME_METRICS {
            let values = results
                .iter()
                .map(|result| at(result, &["names", &name, metric]))
                .collect();
            out.push(row("name", Some(&name), metric, values));
        }
    }
    out
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
