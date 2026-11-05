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

//! Statement stats of this instance, Java's Query Stats (plans/Tools-Database.md §7.1): per statement text, the hits,
//! errors, minimum, maximum, total and sum of squares of the durations, the first entity, and counts by the artifact
//! that ran it (a screen, service or REST path). On when the conf's `entity-facade@query-stats` is `true`
//! ([`configure`], read once when the facade opens); off, recording costs one relaxed load.
//!
//! The key is the statement text through [`statement_key`]: literals become `?` and whitespace is collapsed, so SQL with
//! literal values in it is one row per shape. Bounded: at most [`MAX_STATEMENTS`] rows and [`MAX_ARTIFACTS`] artifacts
//! each; past that new statements, and new artifacts of a row, count in an [`OTHER`] row or entry. The map is sharded by
//! the key's hash so threads rarely wait on each other.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

/// The most statements kept; past it a new statement counts in the [`OTHER`] row.
pub const MAX_STATEMENTS: usize = 5_000;
/// The most artifacts kept per statement; past it a new artifact counts as [`OTHER`].
pub const MAX_ARTIFACTS: usize = 20;
/// The row, and the artifact entry, that counts what is past the bounds.
pub const OTHER: &str = "(other)";
const SHARDS: usize = 16;

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATEMENTS: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct Stat {
    entity: Option<String>,
    hits: u64,
    errors: u64,
    min_nanos: u64,
    max_nanos: u64,
    total_nanos: u128,
    /// In microseconds squared, as an f64: nanoseconds squared overflow a u128 only after a very long time, but the
    /// standard deviation is shown in microseconds anyway.
    squared_micros: f64,
    artifacts: HashMap<String, u64>,
}

fn shards() -> &'static [Mutex<HashMap<String, Stat>>] {
    static SHARD_MAPS: OnceLock<Vec<Mutex<HashMap<String, Stat>>>> = OnceLock::new();
    SHARD_MAPS.get_or_init(|| (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect())
}

fn shard_of(key: &str) -> &'static Mutex<HashMap<String, Stat>> {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let index = usize::try_from(hasher.finish() % SHARDS as u64).unwrap_or(0);
    &shards()[index]
}

/// Whether statements are counted.
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Turn counting on or off; what was counted is kept.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Read `entity-facade@query-stats` (`${}` expanded) of the merged conf `conf` (the `moqui-conf` root, or the
/// `entity-facade` element itself) and turn counting on or off.
pub fn configure(conf: &MNode) {
    let facade = if conf.has_name("entity-facade") {
        Some(conf.clone())
    } else {
        conf.first_named("entity-facade")
    };
    let raw = facade
        .and_then(|node| node.attribute("query-stats"))
        .unwrap_or_default();
    let on = match moqui_lib::env::expand(&raw) {
        Ok(text) => text.trim() == "true",
        Err(err) => {
            tracing::error!(error = %err, "Could not expand entity-facade query-stats, statement stats stay off");
            false
        }
    };
    set_enabled(on);
}

/// The key of a statement: [`super::exec::redact_sql`] (quoted strings, numbers and dollar-quoted text become `?`,
/// comments go), then runs of whitespace become one space.
#[must_use]
pub fn statement_key(sql: &str) -> String {
    let redacted = super::exec::redact_sql(sql);
    let mut key = String::with_capacity(redacted.len());
    for word in redacted.split_whitespace() {
        if !key.is_empty() {
            key.push(' ');
        }
        key.push_str(word);
    }
    key
}

/// Count one statement that took `elapsed`, failed or not. Only called while [`enabled`].
pub(crate) fn record(sql: &str, elapsed: Duration, error: bool) {
    let key = statement_key(sql);
    let at = crate::hooks::artifact_name();
    let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
    let mut map = lock(shard_of(&key));
    let stat = if let Some(stat) = map.get_mut(&key) {
        stat
    } else {
        // Reserve a row; past the bound the statement counts in the other row, in its own shard.
        let reserved = STATEMENTS.fetch_add(1, Ordering::Relaxed) < MAX_STATEMENTS;
        if !reserved {
            STATEMENTS.fetch_sub(1, Ordering::Relaxed);
            drop(map);
            let mut other_map = lock(shard_of(OTHER));
            let other = other_map.entry(OTHER.to_string()).or_default();
            count(other, nanos, error, at.artifact);
            return;
        }
        map.entry(key).or_insert_with(|| Stat {
            entity: at.entity.clone(),
            ..Stat::default()
        })
    };
    if stat.entity.is_none() {
        stat.entity = at.entity;
    }
    count(stat, nanos, error, at.artifact);
}

fn count(stat: &mut Stat, nanos: u64, error: bool, artifact: Option<String>) {
    if stat.hits == 0 || nanos < stat.min_nanos {
        stat.min_nanos = nanos;
    }
    stat.max_nanos = stat.max_nanos.max(nanos);
    stat.hits += 1;
    if error {
        stat.errors += 1;
    }
    stat.total_nanos += u128::from(nanos);
    let micros = nanos as f64 / 1000.0;
    stat.squared_micros += micros * micros;
    if let Some(artifact) = artifact {
        if let Some(hits) = stat.artifacts.get_mut(&artifact) {
            *hits += 1;
        } else if stat.artifacts.len() < MAX_ARTIFACTS {
            stat.artifacts.insert(artifact, 1);
        } else {
            *stat.artifacts.entry(OTHER.to_string()).or_insert(0) += 1;
        }
    }
}

/// Forget every statement counted.
pub fn clear() {
    for shard in shards() {
        lock(shard).clear();
    }
    STATEMENTS.store(0, Ordering::Relaxed);
}

/// The statements counted, as Java's `getQueryStatsList`: maps of `entityName`, `sql`, `hitCount`, `errorCount`,
/// `minTime`, `maxTime`, `totalTime`, `average` and `stdDev` (microseconds), and `artifactCounts` (artifact to hits).
/// `entity_filter` and `sql_filter` keep the rows whose entity name or text holds them, ignoring case. `order_by` is a
/// field name, `-` in front for descending; numbers sort as numbers.
#[must_use]
pub fn list(order_by: &str, entity_filter: &str, sql_filter: &str) -> Vec<IndexMap<String, Value>> {
    let entity_filter = entity_filter.trim().to_lowercase();
    let sql_filter = sql_filter.trim().to_lowercase();
    let mut rows = Vec::new();
    for shard in shards() {
        let map = lock(shard);
        for (sql, stat) in map.iter() {
            let entity = stat.entity.clone().unwrap_or_default();
            if !entity_filter.is_empty() && !entity.to_lowercase().contains(&entity_filter) {
                continue;
            }
            if !sql_filter.is_empty() && !sql.to_lowercase().contains(&sql_filter) {
                continue;
            }
            rows.push(row(sql, &entity, stat));
        }
    }
    order(&mut rows, order_by);
    rows
}

fn micros(nanos: u128) -> f64 {
    nanos as f64 / 1000.0
}

fn row(sql: &str, entity: &str, stat: &Stat) -> IndexMap<String, Value> {
    let hits = stat.hits.max(1) as f64;
    let total = micros(stat.total_nanos);
    let average = total / hits;
    let std_dev = if stat.hits < 2 {
        0.0
    } else {
        ((stat.squared_micros - total * total / hits).abs() / (hits - 1.0)).sqrt()
    };
    let mut artifacts: Vec<(&String, &u64)> = stat.artifacts.iter().collect();
    artifacts.sort_by(|left, right| right.1.cmp(left.1).then_with(|| left.0.cmp(right.0)));
    let artifact_counts: IndexMap<String, Value> = artifacts
        .into_iter()
        .map(|(name, hits)| (name.clone(), Value::I64(i64::try_from(*hits).unwrap_or(i64::MAX))))
        .collect();
    let mut row = IndexMap::new();
    row.insert("entityName".to_string(), Value::string(entity));
    row.insert("sql".to_string(), Value::string(sql));
    row.insert(
        "hitCount".to_string(),
        Value::I64(i64::try_from(stat.hits).unwrap_or(i64::MAX)),
    );
    row.insert(
        "errorCount".to_string(),
        Value::I64(i64::try_from(stat.errors).unwrap_or(i64::MAX)),
    );
    row.insert("minTime".to_string(), Value::F64(micros(u128::from(stat.min_nanos))));
    row.insert("maxTime".to_string(), Value::F64(micros(u128::from(stat.max_nanos))));
    row.insert("totalTime".to_string(), Value::F64(total));
    row.insert("totalSquaredTime".to_string(), Value::F64(stat.squared_micros));
    row.insert("average".to_string(), Value::F64(average));
    row.insert("stdDev".to_string(), Value::F64(std_dev));
    row.insert("artifactCounts".to_string(), Value::Map(artifact_counts));
    row
}

fn order(rows: &mut [IndexMap<String, Value>], order_by: &str) {
    let field = order_by.trim();
    let descending = field.starts_with('-');
    let field = field.trim_start_matches(['-', '+', '^']);
    if field.is_empty() {
        return;
    }
    let number = |value: Option<&Value>| match value {
        Some(Value::I64(number)) => Some(*number as f64),
        Some(Value::F64(number)) => Some(*number),
        _ => None,
    };
    rows.sort_by(|left, right| {
        let ordering = match (number(left.get(field)), number(right.get(field))) {
            (Some(left), Some(right)) => left.total_cmp(&right),
            _ => {
                let text = |row: &IndexMap<String, Value>| {
                    row.get(field)
                        .map(moqui_lib::object::to_plain_string)
                        .unwrap_or_default()
                };
                text(left).cmp(&text(right))
            }
        };
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

#[cfg(test)]
#[path = "query_stats_tests.rs"]
mod tests;
