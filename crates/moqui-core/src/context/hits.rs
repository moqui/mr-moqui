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

//! Artifact hits and hit bins. A popped artifact frame is counted here in memory, as Java's
//! `countArtifactHit`. The scheduler tick writes `moqui.server.ArtifactHit` and
//! `moqui.server.ArtifactHitBin` on a reserve connection, so a request never writes them.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use moqui_db::sql::insert as sql_insert;
use moqui_lib::node::MNode;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::error::Error;
use super::factory::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::user::{now_millis, stamp_value};

const HIT: &str = "moqui.server.ArtifactHit";
const HIT_BIN: &str = "moqui.server.ArtifactHitBin";
/// Hits before a running time can be called slow.
const CHECK_SLOW_THRESHOLD: u64 = 50;
/// A slow hit longer than this is always written, whatever `persist-hit` says.
const USER_IMPACT_MIN_MILLIS: f64 = 1000.0;
/// Rows per transaction in a flush, Java's `maxCreates`.
const MAX_CREATES: usize = 1000;
const FLUSH_RETRIES: u32 = 5;
/// Hits held for the flush. Past this, until a tick takes them, a new hit is dropped and counted, so memory stays
/// bounded when no tick runs.
const MAX_QUEUED_HITS: usize = 10_000;
/// Queued hits at which [`ArtifactHits::count`] asks for a flush now, as the scheduler tick may not run for a long
/// time, such as during one long call. Half the limit, so the flush has room to run while hits keep coming.
const FLUSH_QUEUED_HITS: usize = MAX_QUEUED_HITS / 2;
/// Closed bins at which a flush is asked for, half the limit as for hits.
const FLUSH_CLOSED_BINS: usize = MAX_CLOSED_BINS / 2;
/// Closed bins held for the flush, dropped and counted past this as hits are.
const MAX_CLOSED_BINS: usize = 20_000;
/// Distinct artifacts with counts. A new artifact past this is not binned, and is counted as dropped.
const MAX_STATS: usize = 20_000;
/// Exact parameter names, in lower case, left out of a hit's `parameterString` besides the web credential names.
const SECRET_PARAMETER_NAMES: &[&str] = &["code", "moquisessiontoken", "sessiontoken", "sql", "sharedsecret"];

/// Entities whose operations are not counted, so counting does not count itself.
const ENTITIES_TO_SKIP: &[&str] = &[
    "moqui.server.ArtifactHit",
    "create#moqui.server.ArtifactHit",
    "moqui.server.ArtifactHitBin",
    "create#moqui.server.ArtifactHitBin",
    "moqui.entity.SequenceValueItem",
    "moqui.security.UserAccount",
    "moqui.entity.document.DataDocument",
    "moqui.entity.document.DataDocumentField",
    "moqui.entity.document.DataDocumentCondition",
    "moqui.entity.feed.DataFeedAndDocument",
    "moqui.entity.view.DbViewEntity",
    "moqui.entity.view.DbViewEntityMember",
    "moqui.entity.view.DbViewEntityKeyMap",
    "moqui.entity.view.DbViewEntityAlias",
];

/// One hit as the artifact stack or a screen reports it.
pub(crate) struct Hit<'a> {
    pub artifact_type: &'a str,
    pub sub_type: Option<&'a str>,
    pub name: &'a str,
    pub parameters: Option<&'a BTreeMap<String, Value>>,
    pub start_ms: i64,
    pub running_ms: f64,
    pub output_size: Option<i64>,
}

/// What a written hit takes from the context: read only when a hit row is made.
#[derive(Default)]
pub(crate) struct HitContext {
    pub visit_id: Option<String>,
    pub user_id: Option<String>,
    pub errors: Vec<String>,
    pub request_url: Option<String>,
    pub referrer_url: Option<String>,
}

/// The `server-stats` settings and the counts since start. One per factory.
pub(crate) struct ArtifactHits {
    bin_length_ms: i64,
    /// `artifact-stats` per type: `(persist-hit, persist-bin)`.
    by_type: HashMap<String, (bool, bool)>,
    state: Mutex<State>,
    /// True while a background flush started by [`spawn_flush`] runs, so only one runs at a time.
    flushing: AtomicBool,
}

#[derive(Default)]
struct State {
    stats: HashMap<String, StatsInfo>,
    /// Insertion number of the next new artifact, for [`java_hash_map_order`].
    next_order: u64,
    /// Bins that ended, waiting for the flush.
    closed: Vec<BinRow>,
    /// Hits waiting for the flush, Java's `deferredHitInfoQueue`.
    hits: Vec<HitRow>,
    /// Hits, closed bins, and new artifacts dropped by the limits since the last flush.
    dropped: u64,
}

impl State {
    /// Count one thing dropped by a limit. The first, and every thousandth after, is logged.
    fn drop_one(&mut self, what: &str) {
        self.dropped += 1;
        if self.dropped == 1 || self.dropped.is_multiple_of(1000) {
            tracing::warn!(
                dropped = self.dropped,
                "Artifact hit {what} dropped, the in-memory limit is reached and no flush has run"
            );
        }
    }
}

/// Java's `ArtifactStatsInfo`: counts since start for one artifact, and its current bin.
struct StatsInfo {
    /// When the artifact was first counted since start, see [`java_hash_map_order`].
    order: u64,
    artifact_type: String,
    sub_type: Option<String>,
    name: String,
    cur_bin: Option<BinInfo>,
    hit_count: u64,
    total_time: f64,
    total_squared: f64,
}

/// Java's `ArtifactBinInfo`.
struct BinInfo {
    start_ms: i64,
    hit_count: u64,
    slow_hit_count: u64,
    total_time: f64,
    total_squared: f64,
    min_time: f64,
    max_time: f64,
}

struct BinRow {
    artifact_type: String,
    sub_type: Option<String>,
    name: String,
    end_ms: i64,
    bin: BinInfo,
}

struct HitRow {
    context: HitContext,
    is_slow: bool,
    artifact_type: String,
    sub_type: Option<String>,
    name: String,
    start_ms: i64,
    running_ms: f64,
    parameter_string: Option<String>,
    output_size: Option<i64>,
}

impl ArtifactHits {
    /// Read `server-stats` from the merged conf. No conf counts nothing.
    pub(crate) fn from_conf(conf_text: &str) -> Self {
        let mut bin_length_ms = 900_000;
        let mut by_type = HashMap::new();
        let stats = (!conf_text.trim().is_empty())
            .then(|| MNode::parse_text("moqui-conf", conf_text).ok())
            .flatten()
            .and_then(|root| root.first_named("server-stats"));
        if let Some(stats) = stats {
            if let Some(seconds) = stats
                .attribute("bin-length-seconds")
                .and_then(|text| text.trim().parse::<i64>().ok())
            {
                bin_length_ms = seconds.saturating_mul(1000);
            }
            for node in stats.children_named("artifact-stats") {
                let Some(artifact_type) = node.attribute("type") else {
                    continue;
                };
                // The first entry for a type wins, as Java's getArtifactStatsNode with no sub-type.
                by_type.entry(artifact_type).or_insert((
                    node.attribute("persist-hit").as_deref() == Some("true"),
                    node.attribute("persist-bin").as_deref() == Some("true"),
                ));
            }
        }
        Self {
            bin_length_ms,
            by_type,
            state: Mutex::new(State::default()),
            flushing: AtomicBool::new(false),
        }
    }

    fn persist(&self, artifact_type: &str) -> (bool, bool) {
        self.by_type.get(artifact_type).copied().unwrap_or((false, false))
    }

    /// Whether a hit of this type is counted at all, so a caller need not keep its parameters.
    pub(crate) fn counts(&self, artifact_type: &str) -> bool {
        let (hit, bin) = self.persist(artifact_type);
        hit || bin
    }

    /// Java's `countArtifactHit`. `context` is read only when the hit is written. Returns true when the queued hits
    /// or closed bins have reached the flush threshold, so the caller should start a flush rather than wait for the
    /// scheduler tick, see [`spawn_flush`]. The limits stay as the safety net when no flush runs.
    pub(crate) fn count(&self, hit: &Hit<'_>, context: impl FnOnce() -> HitContext) -> bool {
        let (persist_hit, persist_bin) = self.persist(hit.artifact_type);
        if !persist_hit && !persist_bin {
            return false;
        }
        self.count_counted(hit, persist_hit, persist_bin, context);
        let state = lock(&self.state);
        state.hits.len() >= FLUSH_QUEUED_HITS || state.closed.len() >= FLUSH_CLOSED_BINS
    }

    fn count_counted(&self, hit: &Hit<'_>, persist_hit: bool, persist_bin: bool, context: impl FnOnce() -> HitContext) {
        let is_entity =
            hit.artifact_type == "AT_ENTITY" || hit.sub_type.is_some_and(|sub_type| sub_type.starts_with("entity"));
        if is_entity && ENTITIES_TO_SKIP.contains(&hit.name) {
            return;
        }
        let mut state = lock(&self.state);
        let mut is_slow = false;
        if persist_bin {
            // Artifact names are unique enough that the type is not in the key.
            let key = match hit.sub_type {
                Some(sub_type) if hit.artifact_type != "AT_SERVICE" => format!("{}{sub_type}", hit.name),
                _ => hit.name.to_string(),
            };
            if state.stats.len() >= MAX_STATS && !state.stats.contains_key(&key) {
                state.drop_one("artifact");
                return;
            }
            let State {
                stats,
                closed,
                next_order,
                ..
            } = &mut *state;
            let full = closed.len() >= MAX_CLOSED_BINS;
            let info = stats.entry(key).or_insert_with(|| {
                *next_order += 1;
                StatsInfo {
                    order: *next_order,
                    artifact_type: hit.artifact_type.to_string(),
                    sub_type: hit.sub_type.map(str::to_string),
                    name: hit.name.to_string(),
                    cur_bin: None,
                    hit_count: 0,
                    total_time: 0.0,
                    total_squared: 0.0,
                }
            });
            // The current bin ended since the last hit: it waits for the flush, and a new one starts.
            let expired = info
                .cur_bin
                .as_ref()
                .is_some_and(|bin| hit.start_ms > bin.start_ms + self.bin_length_ms);
            let mut dropped_bin = false;
            if expired {
                if let Some(row) = info.close_bin(self.bin_length_ms, None) {
                    if full {
                        dropped_bin = true;
                    } else {
                        closed.push(row);
                    }
                }
                info.cur_bin = Some(BinInfo::new(hit.start_ms));
            }
            is_slow = info.count_hit(hit.start_ms, hit.running_ms);
            if dropped_bin {
                state.drop_one("bin");
            }
        }
        // Single hits of an entity are never written: too many, and they would count themselves.
        if is_entity || !((is_slow && hit.running_ms > USER_IMPACT_MIN_MILLIS) || persist_hit) {
            return;
        }
        drop(state);
        let row = HitRow {
            context: context(),
            is_slow,
            artifact_type: hit.artifact_type.to_string(),
            sub_type: hit.sub_type.map(str::to_string),
            name: hit.name.to_string(),
            start_ms: hit.start_ms,
            running_ms: hit.running_ms,
            parameter_string: hit.parameters.and_then(parameter_string),
            output_size: hit.output_size,
        };
        let mut state = lock(&self.state);
        if state.hits.len() >= MAX_QUEUED_HITS {
            state.drop_one("row");
        } else {
            state.hits.push(row);
        }
    }

    /// Claim the one background flush. False when one is already running.
    fn begin_flush(&self) -> bool {
        self.flushing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Take what the flush writes: the waiting hits, the closed bins, and each current bin whose
    /// time is over. `all` also takes the current bins that are still open, as Java's `destroy`.
    ///
    /// Current bins are taken in the iteration order of Java's `artifactStatsInfoByType` `HashMap`, which is the
    /// order Java's `destroy` gives them their sequenced ids. Bins of Rust-only sub types (see
    /// [`crate::entity::RustOnlyEntityHits`]) have no place in Java's map: they are left out of its order and taken
    /// after it, so the bins Java also has get the same ids.
    fn take(&self, all: bool) -> (Vec<HitRow>, Vec<BinRow>) {
        let now = now_millis();
        let mut state = lock(&self.state);
        let State {
            stats,
            closed,
            hits,
            dropped,
            ..
        } = &mut *state;
        let (rust_only, java): (Vec<_>, Vec<_>) = stats
            .iter()
            .map(|(key, info)| (key.as_str(), info.order, info.sub_type.as_deref()))
            .partition(|(_, _, sub_type)| crate::entity::is_rust_only_sub_type(*sub_type));
        let mut keys = java_hash_map_order(java.into_iter().map(|(key, order, _)| (key, order)));
        let mut rust_only: Vec<(&str, u64)> = rust_only.into_iter().map(|(key, order, _)| (key, order)).collect();
        rust_only.sort_by_key(|(_, order)| *order);
        keys.extend(rust_only.into_iter().map(|(key, _)| key.to_string()));
        for key in keys {
            let Some(info) = stats.get_mut(&key) else {
                continue;
            };
            let Some(start) = info.cur_bin.as_ref().map(|bin| bin.start_ms) else {
                continue;
            };
            if now > start + self.bin_length_ms {
                closed.extend(info.close_bin(self.bin_length_ms, None));
            } else if all {
                closed.extend(info.close_bin(self.bin_length_ms, Some(now)));
            }
        }
        if *dropped > 0 {
            tracing::warn!(
                dropped = *dropped,
                "Artifact hits, bins, or artifacts were dropped since the last flush"
            );
            *dropped = 0;
        }
        (std::mem::take(hits), std::mem::take(closed))
    }
}

/// Keys in the iteration order of a Java `HashMap<String, _>` the keys were put into in `order`, with nothing
/// removed: by bucket, `(h ^ (h >>> 16)) & (capacity - 1)` of the Java `String.hashCode`, and in each bucket by
/// insertion, which a resize keeps. The capacity starts at 16 and doubles while the size is over 3/4 of it. A bucket
/// of 8 or more keys, which Java turns into a tree once the capacity is 64, is not modelled.
fn java_hash_map_order<'a>(keys: impl Iterator<Item = (&'a str, u64)>) -> Vec<String> {
    let mut keyed: Vec<(u32, u64, &str)> = Vec::new();
    for (key, order) in keys {
        let hash = key
            .encode_utf16()
            .fold(0_u32, |hash, unit| hash.wrapping_mul(31).wrapping_add(u32::from(unit)));
        keyed.push((hash ^ (hash >> 16), order, key));
    }
    let mut capacity = 16_usize;
    while keyed.len() > capacity / 4 * 3 {
        capacity *= 2;
    }
    let mask = u32::try_from(capacity - 1).unwrap_or(u32::MAX);
    keyed.sort_by_key(|(spread, order, _)| (spread & mask, *order));
    keyed.into_iter().map(|(_, _, key)| key.to_string()).collect()
}

impl StatsInfo {
    /// Count a hit and say whether it was slow: more than 2.6 standard deviations above the average.
    fn count_hit(&mut self, start_ms: i64, running: f64) -> bool {
        self.hit_count += 1;
        let is_slow = self.is_hit_slow(running);
        // The first hit is almost always much slower, so when it is over 3 times the second the second counts twice.
        if self.hit_count == 2 && self.total_time > running * 3.0 {
            self.total_time = running * 2.0;
            self.total_squared = running * running * 2.0;
        } else {
            self.total_time += running;
            self.total_squared += running * running;
        }
        self.cur_bin
            .get_or_insert_with(|| BinInfo::new(start_ms))
            .count_hit(running, is_slow);
        is_slow
    }

    fn is_hit_slow(&self, running: f64) -> bool {
        if self.hit_count < CHECK_SLOW_THRESHOLD {
            return false;
        }
        let count = self.hit_count as f64;
        let average = self.total_time / count;
        let std_dev = ((self.total_squared - (self.total_time * self.total_time) / count).abs() / (count - 1.0)).sqrt();
        let slow_time = average + std_dev * 2.6;
        if slow_time != 0.0 && running > slow_time {
            if running > USER_IMPACT_MIN_MILLIS {
                tracing::warn!(
                    "Slow hit to {}:{}:{} running time {running} is greater than average {average} plus 2.6 standard deviations {std_dev}",
                    self.artifact_type,
                    self.sub_type.as_deref().unwrap_or("null"),
                    self.name
                );
            }
            return true;
        }
        false
    }

    /// End the current bin at its length, or at `end_ms` when given.
    fn close_bin(&mut self, bin_length_ms: i64, end_ms: Option<i64>) -> Option<BinRow> {
        let bin = self.cur_bin.take()?;
        Some(BinRow {
            artifact_type: self.artifact_type.clone(),
            sub_type: self.sub_type.clone(),
            name: self.name.clone(),
            end_ms: end_ms.unwrap_or(bin.start_ms + bin_length_ms),
            bin,
        })
    }
}

impl BinInfo {
    fn new(start_ms: i64) -> Self {
        Self {
            start_ms,
            hit_count: 0,
            slow_hit_count: 0,
            total_time: 0.0,
            total_squared: 0.0,
            min_time: f64::MAX,
            max_time: 0.0,
        }
    }

    fn count_hit(&mut self, running: f64, is_slow: bool) {
        self.hit_count += 1;
        if is_slow {
            self.slow_hit_count += 1;
        }
        if self.hit_count == 2 && self.total_time > running * 3.0 {
            self.total_time = running * 2.0;
            self.total_squared = running * running * 2.0;
        } else {
            self.total_time += running;
            self.total_squared += running * running;
        }
        self.min_time = self.min_time.min(running);
        self.max_time = self.max_time.max(running);
    }
}

/// `key=value` pairs, without maps, lists, empty values and [`secret_parameter`] names.
/// Values are cut at 80 characters and the whole at 1000.
fn parameter_string(parameters: &BTreeMap<String, Value>) -> Option<String> {
    if parameters.is_empty() {
        return None;
    }
    let mut out = String::new();
    for (key, value) in parameters {
        if matches!(value, Value::Null | Value::Map(_) | Value::List(_) | Value::Object(_)) || secret_parameter(key) {
            continue;
        }
        let text = to_plain_string(value);
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(key);
        out.push('=');
        out.push_str(&truncate(&text, 80));
    }
    Some(truncate(&out, 1000))
}

/// A parameter a hit row must not keep, compared without case: the web credential names, a second-factor `code`,
/// session tokens, SQL text, a shared secret, and any name with `password`, `secret`, or `token` in it or ending in
/// `_key`, `apikey`, or `loginkey`. Ids such as `primaryKeyId` or `monkeyId` are kept.
/// Java left out only names with `password`.
pub(crate) fn secret_parameter(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    crate::web::CREDENTIAL_PARAMETER_NAMES.contains(&lower.as_str())
        || SECRET_PARAMETER_NAMES.contains(&lower.as_str())
        || ["password", "secret", "token"].iter().any(|part| lower.contains(part))
        || ["_key", "apikey", "loginkey"].iter().any(|end| lower.ends_with(end))
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// The host address and name, as Java's `InetAddress.getLocalHost()`, found once.
pub(crate) fn server_address() -> &'static (String, String) {
    static ADDRESS: OnceLock<(String, String)> = OnceLock::new();
    ADDRESS.get_or_init(|| {
        let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .or_else(|| std::env::var("HOSTNAME").ok())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty());
        let Some(name) = name else {
            return ("127.0.0.1".to_string(), "localhost".to_string());
        };
        // Java orders the resolved addresses IPv4 first (java.net.preferIPv6Addresses defaults to false), keeping
        // the resolver's order within each family, and takes the first.
        let addresses: Vec<std::net::IpAddr> = std::net::ToSocketAddrs::to_socket_addrs(&(name.as_str(), 0))
            .map(|found| found.map(|address| address.ip()).collect())
            .unwrap_or_default();
        let ip = addresses
            .iter()
            .find(|address| address.is_ipv4())
            .or_else(|| addresses.first())
            .map_or_else(|| "127.0.0.1".to_string(), ToString::to_string);
        (ip, name)
    })
}

/// Write the waiting hits and the bins whose time is over. The scheduler tick calls this with
/// `all` false. Shutdown calls it with `all` true, which also writes the bins still open.
///
/// # Errors
///
/// Returns an error when a chunk of rows cannot be written after the retries.
pub(crate) fn flush(factory: &ExecutionContextFactory, all: bool) -> Result<(), Error> {
    let (hits, bins) = factory.artifact_hits().take(all);
    if hits.is_empty() && bins.is_empty() {
        return Ok(());
    }
    let Some(facade) = factory.entity() else {
        return Ok(());
    };
    let (ip, host) = server_address();
    let mut rows: Vec<(&str, BTreeMap<String, Value>)> = Vec::with_capacity(hits.len() + bins.len());
    for bin in bins {
        rows.push((HIT_BIN, bin_fields(bin, ip, host)));
    }
    for hit in hits {
        rows.push((HIT, hit_fields(hit, ip, host)));
    }
    let mut failed = None;
    for chunk in rows.chunks(MAX_CREATES) {
        let mut last = None;
        for _ in 0..FLUSH_RETRIES {
            match write_chunk(&facade, chunk) {
                Ok(()) => {
                    last = None;
                    break;
                }
                Err(err) => {
                    tracing::error!("Error saving ArtifactHits, retrying: {err}");
                    last = Some(err);
                }
            }
        }
        failed = failed.or(last);
    }
    failed.map_or(Ok(()), Err)
}

/// One transaction on a reserve connection for a chunk of rows. Ids come from the sequence first,
/// which takes its own reserve connection.
fn write_chunk(facade: &EntityFacade, chunk: &[(&str, BTreeMap<String, Value>)]) -> Result<(), Error> {
    let mut statements = Vec::with_capacity(chunk.len());
    for (entity_name, fields) in chunk {
        let mut value = facade.make_value(entity_name)?;
        for (name, field) in fields {
            value.set(name.clone(), field.clone());
        }
        value.set_sequenced_id_primary(facade)?;
        let plain = facade.normalize_values(entity_name, value.fields().clone(), false)?;
        let names: Vec<&str> = plain.keys().map(|name| &**name).collect();
        let sql = facade.with_entity(entity_name, |entity, dialect, _| sql_insert(entity, dialect, &names))?;
        statements.push((sql, plain.into_values().collect::<Vec<_>>()));
    }
    let tx = facade.transactions_for(HIT_BIN)?;
    crate::entity::exec::with_reserve_tx(&tx, |conn| {
        for (sql, params) in &statements {
            conn.execute(sql, params)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// A decimal with 6 digits after the point, as Java's `setScale(6, HALF_UP)`.
fn decimal(value: f64) -> Value {
    moqui_lib::object::basic_convert(&Value::string(format!("{value:.6}")), "java.math.BigDecimal").value
}

fn optional(text: Option<String>) -> Value {
    text.map_or(Value::Null, Value::string)
}

fn bin_fields(row: BinRow, ip: &str, host: &str) -> BTreeMap<String, Value> {
    let bin = row.bin;
    let mut fields = BTreeMap::new();
    fields.insert("artifactType".to_string(), Value::string(row.artifact_type));
    fields.insert("artifactSubType".to_string(), optional(row.sub_type));
    fields.insert("artifactName".to_string(), Value::string(row.name));
    fields.insert("binStartDateTime".to_string(), stamp_value(bin.start_ms));
    fields.insert("binEndDateTime".to_string(), stamp_value(row.end_ms));
    fields.insert("hitCount".to_string(), Value::I64(bin.hit_count as i64));
    fields.insert("totalTimeMillis".to_string(), decimal(bin.total_time));
    fields.insert("totalSquaredTime".to_string(), decimal(bin.total_squared));
    fields.insert("minTimeMillis".to_string(), decimal(bin.min_time));
    fields.insert("maxTimeMillis".to_string(), decimal(bin.max_time));
    fields.insert("slowHitCount".to_string(), Value::I64(bin.slow_hit_count as i64));
    fields.insert("serverIpAddress".to_string(), Value::string(ip));
    fields.insert("serverHostName".to_string(), Value::string(host));
    fields
}

fn hit_fields(row: HitRow, ip: &str, host: &str) -> BTreeMap<String, Value> {
    let HitContext {
        visit_id,
        user_id,
        errors,
        request_url,
        referrer_url,
    } = row.context;
    let mut fields = BTreeMap::new();
    fields.insert("visitId".to_string(), optional(visit_id));
    fields.insert("userId".to_string(), optional(user_id));
    fields.insert(
        "isSlowHit".to_string(),
        Value::string(if row.is_slow { "Y" } else { "N" }),
    );
    fields.insert("artifactType".to_string(), Value::string(row.artifact_type));
    fields.insert("artifactSubType".to_string(), optional(row.sub_type));
    fields.insert("artifactName".to_string(), Value::string(row.name));
    fields.insert("startDateTime".to_string(), stamp_value(row.start_ms));
    fields.insert("runningTimeMillis".to_string(), decimal(row.running_ms));
    if let Some(parameters) = row.parameter_string {
        fields.insert("parameterString".to_string(), Value::string(parameters));
    }
    if let Some(size) = row.output_size {
        fields.insert("outputSize".to_string(), Value::I64(size));
    }
    if errors.is_empty() {
        fields.insert("wasError".to_string(), Value::string("N"));
    } else {
        let joined: String = errors.iter().map(|error| format!("{error};")).collect();
        fields.insert("wasError".to_string(), Value::string("Y"));
        fields.insert("errorMessage".to_string(), Value::string(truncate(&joined, 255)));
    }
    if let Some(url) = request_url.filter(|url| !url.is_empty()) {
        fields.insert("requestUrl".to_string(), Value::string(truncate(&url, 255)));
    }
    if let Some(url) = referrer_url.filter(|url| !url.is_empty()) {
        fields.insert("referrerUrl".to_string(), Value::string(url));
    }
    fields.insert("serverIpAddress".to_string(), Value::string(ip));
    fields.insert("serverHostName".to_string(), Value::string(host));
    fields
}

/// Releases the single-flight claim when the flush thread ends, also by a panic.
struct FlushClaim(Arc<ExecutionContextFactory>);

impl Drop for FlushClaim {
    fn drop(&mut self) {
        self.0.artifact_hits().flushing.store(false, Ordering::Release);
    }
}

/// Start a flush on a background thread when none is running, for [`ArtifactHits::count`] reporting a full queue.
/// It writes as the scheduler tick does, on the reserve connection. The thread holds the factory only while it
/// flushes, and a failure or panic is logged, not raised.
pub(crate) fn spawn_flush(factory: &Arc<ExecutionContextFactory>) {
    if !factory.artifact_hits().begin_flush() {
        return;
    }
    let weak = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("artifact-hit-flush".to_string())
        .spawn(move || {
            let Some(factory) = weak.upgrade() else {
                return;
            };
            let claim = FlushClaim(factory);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| flush(&claim.0, false)));
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!("artifact hit flush failed: {err}"),
                Err(_) => tracing::warn!("artifact hit flush panicked"),
            }
        });
    if let Err(err) = spawned {
        // The closure never ran, so its claim was never made: release ours.
        factory.artifact_hits().flushing.store(false, Ordering::Release);
        tracing::warn!("artifact hit flush thread did not start: {err}");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use moqui_lib::object::Value;

    use super::{parameter_string, ArtifactHits, Hit, HitContext};

    fn conf() -> &'static str {
        r#"<moqui-conf><server-stats bin-length-seconds="60">
            <artifact-stats type="AT_XML_SCREEN" persist-bin="true" persist-hit="true"/>
            <artifact-stats type="AT_SERVICE" persist-bin="true" persist-hit="false"/>
            <artifact-stats type="AT_ENTITY" persist-bin="false"/>
        </server-stats></moqui-conf>"#
    }

    fn hit<'a>(artifact_type: &'a str, name: &'a str, start_ms: i64, running_ms: f64) -> Hit<'a> {
        Hit {
            artifact_type,
            sub_type: None,
            name,
            parameters: None,
            start_ms,
            running_ms,
            output_size: None,
        }
    }

    #[test]
    fn bins_follow_the_conf_and_advance_after_the_bin_length() {
        let hits = ArtifactHits::from_conf(conf());
        assert_eq!(hits.bin_length_ms, 60_000);
        assert!(hits.counts("AT_SERVICE") && !hits.counts("AT_ENTITY") && !hits.counts("AT_LLM"));
        hits.count(&hit("AT_SERVICE", "a.b#c", 1_000, 4.0), HitContext::default);
        hits.count(&hit("AT_SERVICE", "a.b#c", 2_000, 2.0), HitContext::default);
        hits.count(
            &hit("AT_ENTITY", "moqui.basic.Enumeration", 2_000, 2.0),
            HitContext::default,
        );
        hits.count(
            &hit("AT_XML_SCREEN", "component://x.xml", 3_000, 5.0),
            HitContext::default,
        );
        // A hit after the bin length closes the old bin at its end and starts another.
        hits.count(&hit("AT_SERVICE", "a.b#c", 70_000, 1.0), HitContext::default);
        let state = hits.state.lock().unwrap();
        assert_eq!(state.closed.len(), 1);
        let closed = &state.closed[0];
        assert_eq!(
            (closed.bin.start_ms, closed.end_ms, closed.bin.hit_count),
            (1_000, 61_000, 2)
        );
        assert_eq!(
            (closed.bin.min_time, closed.bin.max_time, closed.bin.total_time),
            (2.0, 4.0, 6.0)
        );
        assert_eq!(state.stats.len(), 2);
        let service = &state.stats["a.b#c"];
        assert_eq!(
            (service.hit_count, service.cur_bin.as_ref().unwrap().start_ms),
            (3, 70_000)
        );
        // Only the screen has persist-hit.
        assert_eq!(state.hits.len(), 1);
        assert_eq!(state.hits[0].name, "component://x.xml");
    }

    #[test]
    fn count_asks_for_a_flush_at_the_threshold_and_a_flush_in_time_drops_nothing() {
        use super::{FLUSH_QUEUED_HITS, MAX_QUEUED_HITS};

        let hits = ArtifactHits::from_conf(conf());
        let screen = |index: i64| hit("AT_XML_SCREEN", "component://x.xml", index, 1.0);
        for index in 0..(FLUSH_QUEUED_HITS - 1) {
            assert!(!hits.count(&screen(index as i64), HitContext::default));
        }
        assert!(hits.count(&screen(0), HitContext::default), "threshold reached");
        // A type that is not counted never asks.
        assert!(!hits.count(&hit("AT_LLM", "m", 0, 1.0), HitContext::default));

        // The caller flushes when asked, as `spawn_flush` does with one flush at a time: nothing is dropped
        // though three times the limit is counted with no scheduler tick.
        let mut flushed = 0;
        for index in 0..(MAX_QUEUED_HITS * 3) {
            if hits.count(&screen(index as i64), HitContext::default) && hits.begin_flush() {
                flushed += hits.take(false).0.len();
                hits.flushing.store(false, std::sync::atomic::Ordering::Release);
            }
        }
        let state = hits.state.lock().unwrap();
        assert_eq!(state.dropped, 0);
        assert_eq!(flushed + state.hits.len(), FLUSH_QUEUED_HITS + MAX_QUEUED_HITS * 3);
    }

    #[test]
    fn current_bins_are_taken_in_java_hash_map_order() {
        use super::java_hash_map_order;

        let order =
            |keys: &[&str]| java_hash_map_order(keys.iter().enumerate().map(|(index, key)| (*key, index as u64)));
        // Buckets by String.hashCode: "c" 99, "a" 97, "b" 98, whatever the insertion order.
        assert_eq!(order(&["c", "a", "b"]), ["a", "b", "c"]);
        // "BB" and "Aa" share hash 2112: one bucket, in insertion order.
        assert_eq!(order(&["BB", "Aa"]), ["BB", "Aa"]);
        assert_eq!(order(&["Aa", "BB"]), ["Aa", "BB"]);
        // 13 keys pass the 3/4 threshold of 16, so 32 buckets: "q" (113) is bucket 1 of 16 but 17 of 32.
        let letters = ["q", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"];
        assert_eq!(order(&letters)[0], "q");
        let mut thirteen = letters.to_vec();
        thirteen.push("l");
        assert_eq!(order(&thirteen).last().map(String::as_str), Some("q"));
        // Java wrote these first, in this order, at the end of an empty-db load of 85 bins (128 buckets). Inserted
        // in Java's order, EnumerationType and UomConversion share bucket 4.
        let mut keys = vec![
            "moqui.basic.EnumerationTyperefresh".to_string(),
            "moqui.security.ArtifactTarpitrefresh".to_string(),
            "moqui.basic.UomConversionrefresh".to_string(),
        ];
        keys.extend((0..82).map(|index| format!("pad{index}")));
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let ordered: Vec<String> = order(&keys)
            .into_iter()
            .filter(|key| key.starts_with("moqui."))
            .collect();
        assert_eq!(
            ordered,
            [
                "moqui.security.ArtifactTarpitrefresh",
                "moqui.basic.EnumerationTyperefresh",
                "moqui.basic.UomConversionrefresh",
            ]
        );
    }

    #[test]
    fn rust_only_bins_are_taken_after_the_java_order() {
        let hits = ArtifactHits::from_conf(conf());
        let fk_check = Hit {
            sub_type: Some(crate::entity::RustOnlyEntityHits::FK_CHECK),
            ..hit("AT_XML_SCREEN", "a", 1_000, 1.0)
        };
        hits.count(&fk_check, HitContext::default);
        for name in ["c", "a", "b"] {
            hits.count(&hit("AT_XML_SCREEN", name, 1_000, 1.0), HitContext::default);
        }
        let (_, bins) = hits.take(true);
        let taken: Vec<(&str, Option<&str>)> = bins
            .iter()
            .map(|bin| (bin.name.as_str(), bin.sub_type.as_deref()))
            .collect();
        assert_eq!(taken, [("a", None), ("b", None), ("c", None), ("a", Some("fk-check"))]);
    }

    #[test]
    fn only_one_background_flush_is_claimed_at_a_time() {
        let hits = ArtifactHits::from_conf(conf());
        assert!(hits.begin_flush());
        assert!(!hits.begin_flush());
        hits.flushing.store(false, std::sync::atomic::Ordering::Release);
        assert!(hits.begin_flush());
    }

    #[test]
    fn without_a_flush_the_limit_still_drops_and_counts() {
        use super::MAX_QUEUED_HITS;

        let hits = ArtifactHits::from_conf(conf());
        for index in 0..(MAX_QUEUED_HITS + 5) {
            hits.count(
                &hit("AT_XML_SCREEN", "component://x.xml", index as i64, 1.0),
                HitContext::default,
            );
        }
        let state = hits.state.lock().unwrap();
        assert_eq!((state.hits.len(), state.dropped), (MAX_QUEUED_HITS, 5));
    }

    #[test]
    fn a_hit_far_above_the_average_is_slow_after_fifty() {
        let hits = ArtifactHits::from_conf(conf());
        for index in 0..60 {
            hits.count(
                &hit("AT_SERVICE", "s", index, 10.0 + (index % 3) as f64),
                HitContext::default,
            );
        }
        // Slow and over the user impact time: written though AT_SERVICE does not persist hits.
        hits.count(&hit("AT_SERVICE", "s", 61, 1500.0), HitContext::default);
        let state = hits.state.lock().unwrap();
        assert_eq!(state.stats["s"].cur_bin.as_ref().unwrap().slow_hit_count, 1);
        assert_eq!(state.hits.len(), 1);
        assert!(state.hits[0].is_slow);
    }

    #[test]
    fn the_parameter_string_skips_passwords_maps_and_empty_values() {
        let mut parameters = BTreeMap::new();
        parameters.insert("a".to_string(), Value::string("x".repeat(100)));
        parameters.insert("newPassword".to_string(), Value::string("no"));
        parameters.insert("password".to_string(), Value::string("no"));
        parameters.insert("empty".to_string(), Value::string(""));
        parameters.insert("list".to_string(), Value::List(vec![Value::I64(1)]));
        parameters.insert("n".to_string(), Value::I64(5));
        for secret in [
            "code",
            "moquiSessionToken",
            "sql",
            "sharedSecret",
            "api_key",
            "login_key",
            "clientSecret",
            "refreshToken",
            "AUTH_TOKEN",
            "privateApiKey",
        ] {
            parameters.insert(secret.to_string(), Value::string("no"));
        }
        // Ordinary ids are kept.
        parameters.insert("primaryKeyId".to_string(), Value::string("k"));
        parameters.insert("monkeyId".to_string(), Value::string("m"));
        parameters.insert("codeId".to_string(), Value::string("c"));
        let text = parameter_string(&parameters).unwrap();
        assert_eq!(
            text,
            format!("a={}, codeId=c, monkeyId=m, n=5, primaryKeyId=k", "x".repeat(80))
        );
    }

    #[test]
    fn queued_hits_and_new_artifacts_stop_at_the_limits_until_a_flush_takes_them() {
        let hits = ArtifactHits::from_conf(conf());
        for index in 0..super::MAX_QUEUED_HITS + 5 {
            hits.count(
                &hit("AT_XML_SCREEN", "component://a/screen/A.xml", index as i64, 1.0),
                HitContext::default,
            );
        }
        {
            let state = hits.state.lock().unwrap();
            assert_eq!(state.hits.len(), super::MAX_QUEUED_HITS);
            assert_eq!(state.dropped, 5);
        }
        let (taken, _) = hits.take(false);
        assert_eq!(taken.len(), super::MAX_QUEUED_HITS);
        assert_eq!(hits.state.lock().unwrap().dropped, 0);
        hits.count(
            &hit("AT_XML_SCREEN", "component://a/screen/A.xml", 0, 1.0),
            HitContext::default,
        );
        assert_eq!(hits.state.lock().unwrap().hits.len(), 1);

        let names: Vec<String> = (0..super::MAX_STATS + 3).map(|index| format!("svc.{index}")).collect();
        for name in &names {
            hits.count(&hit("AT_SERVICE", name, 0, 1.0), HitContext::default);
        }
        let state = hits.state.lock().unwrap();
        // The screen artifact, then services up to the limit.
        assert_eq!(state.stats.len(), super::MAX_STATS);
        assert_eq!(state.dropped, 4);
    }

    /// A world with the conf's server stats, a pool to tick, screens, and a service.
    struct World {
        context: crate::context::ExecutionContext,
        facade: crate::entity::EntityFacade,
        tape: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        dir: std::path::PathBuf,
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const SCREEN: &str = r#"<screen require-authentication="false">
        <transition name="doIt"><default-response url="."/></transition>
        <widgets><label text="tracked"/></widgets></screen>"#;
    const UNTRACKED: &str = r#"<screen require-authentication="false" track-artifact-hit="false">
        <widgets><label text="untracked"/></widgets></screen>"#;
    const SERVICES: &str = r#"<services>
        <service verb="find" noun="Hits" authenticate="false">
            <in-parameters><parameter name="enumTypeId"/><parameter name="password"/></in-parameters>
            <actions><entity-find entity-name="moqui.basic.Enumeration" list="found">
                <econdition field-name="enumTypeId"/></entity-find></actions>
        </service></services>"#;

    fn world() -> World {
        let conf = r#"<moqui-conf><server-stats bin-length-seconds="1">
            <artifact-stats type="AT_XML_SCREEN" persist-bin="true" persist-hit="true"/>
            <artifact-stats type="AT_XML_SCREEN_TRANS" persist-bin="true" persist-hit="true"/>
            <artifact-stats type="AT_SERVICE" persist-bin="true" persist-hit="true"/>
            <artifact-stats type="AT_ENTITY" persist-bin="true"/>
        </server-stats></moqui-conf>"#;
        world_with_conf(conf)
    }

    fn world_with_conf(conf: &str) -> World {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("moqui-hits-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("HitScreen.xml"), SCREEN).unwrap();
        std::fs::write(dir.join("Untracked.xml"), UNTRACKED).unwrap();
        std::fs::write(dir.join("HitServices.xml"), SERVICES).unwrap();
        let pools = crate::service::ServicePools::bounded(1, 4);
        let context = crate::context::ExecutionContextFactory::test_context_with(conf, pools);
        let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let facade = crate::testing::open_with_tape(std::sync::Arc::clone(&tape));
        context.factory().set_entity(facade.clone());
        {
            let _entered = context.enter();
            let failures = crate::service::lint(&[dir.join("HitServices.xml")]).unwrap();
            assert!(failures.is_empty(), "{failures:?}");
        }
        World {
            context,
            facade,
            tape,
            dir,
        }
    }

    fn rows(world: &World, entity: &str, artifact_type: &str) -> Vec<crate::entity::EntityValue> {
        world
            .facade
            .find(entity)
            .unwrap()
            .eq("artifactType", Value::string(artifact_type))
            .list()
            .unwrap()
            .values()
            .to_vec()
    }

    fn text(value: &crate::entity::EntityValue, field: &str) -> String {
        value
            .get(field)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    }

    fn number(value: &crate::entity::EntityValue, field: &str) -> f64 {
        text(value, field).parse().unwrap_or(-1.0)
    }

    fn render(world: &World, screen: &str, path: &str) -> String {
        crate::screen::make_render(world.context.factory())
            .root_screen(world.dir.join(screen).to_string_lossy().into_owned())
            .screen_path(path)
            .render()
            .unwrap_or_else(|err| panic!("{screen}{path}: {err}"))
            .body_text()
    }

    #[test]
    fn shipped_settings_and_disabled_overrides_survive_merge_and_shutdown_flush() {
        use moqui_lib::node::MNode;

        let runtime = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
        for mode in ["default", "dev", "disabled"] {
            let conf = MNode::parse_file(runtime.join("base-component/framework/conf/MoquiDefaultConf.xml")).unwrap();
            if mode != "default" {
                super::super::conf::merge_conf(
                    &conf,
                    &MNode::parse_file(runtime.join("conf/MoquiDevConf.xml")).unwrap(),
                );
            }
            if mode == "disabled" {
                super::super::conf::merge_conf(
                    &conf,
                    &MNode::parse_text(
                        "disabled",
                        r#"<moqui-conf><server-stats>
                        <artifact-stats type="AT_XML_SCREEN" persist-hit="false" persist-bin="false"/>
                        <artifact-stats type="AT_SERVICE" persist-hit="false" persist-bin="false"/>
                        <artifact-stats type="AT_ENTITY" persist-bin="false"/>
                    </server-stats></moqui-conf>"#,
                    )
                    .unwrap(),
                );
            }
            let world = world_with_conf(&conf.write_string());
            let configured = world.context.factory().artifact_hits();
            assert_eq!(configured.bin_length_ms, 900_000);
            assert_eq!(configured.persist("AT_XML_SCREEN_CONTENT"), (mode != "default", true));
            assert_eq!(configured.persist("AT_LLM"), (false, true));
            assert_eq!(configured.persist("AT_ENTITY"), (false, mode == "dev"));
            let _entered = world.context.enter();
            let _authz = world.context.disable_authz();
            assert!(render(&world, "HitScreen.xml", "").contains("tracked"));
            render(&world, "Untracked.xml", "");
            crate::service::sync("find#Hits", &BTreeMap::new()).unwrap();
            // No tick and no bin expiry: shutdown must persist the still-open bins and queued hits.
            assert!(rows(&world, super::HIT_BIN, "AT_SERVICE").is_empty());
            world.context.factory().flush_artifact_hits().unwrap();
            for (kind, hit_enabled, bin_enabled) in [
                ("AT_XML_SCREEN", mode != "disabled", mode != "disabled"),
                ("AT_SERVICE", mode == "dev", mode != "disabled"),
                ("AT_ENTITY", false, mode == "dev"),
            ] {
                assert_eq!(
                    !rows(&world, super::HIT, kind).is_empty(),
                    hit_enabled,
                    "{mode}: {kind} hits"
                );
                let bins = rows(&world, super::HIT_BIN, kind);
                assert_eq!(!bins.is_empty(), bin_enabled, "{mode}: {kind} bins");
                assert!(bins.iter().all(|bin| number(bin, "hitCount") > 0.0));
                assert!(bins
                    .iter()
                    .all(|bin| !text(bin, "artifactName").ends_with("Untracked.xml")));
            }
            let before = rows(&world, super::HIT_BIN, "AT_XML_SCREEN").len();
            world.context.factory().flush_artifact_hits().unwrap();
            assert_eq!(rows(&world, super::HIT_BIN, "AT_XML_SCREEN").len(), before);
        }
    }

    #[test]
    fn create_or_update_counts_what_it_runs_and_an_unmatched_probe_apart() {
        let world = world();
        let _entered = world.context.enter();
        let _authz = world.context.disable_authz();
        let entity = "moqui.basic.EnumerationType";
        let mut value = world.facade.make_value(entity).unwrap();
        value.set("enumTypeId", Value::string("HitType"));
        value.set("description", Value::string("first"));
        // A new row: the UPDATE probe matches nothing (Rust-only `update-probe`), then create.
        value.create_or_update(&world.facade).unwrap();
        value.set("description", Value::string("second"));
        // An existing row: the probe is the update, with no refresh.
        value.create_or_update(&world.facade).unwrap();
        value.refresh(&world.facade).unwrap();
        world.facade.find(entity).unwrap().one().unwrap();
        world.facade.find(entity).unwrap().list().unwrap();
        world.context.factory().flush_artifact_hits().unwrap();

        let bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_ENTITY");
        let counts: BTreeMap<_, _> = bins
            .iter()
            .filter(|bin| text(bin, "artifactName") == entity)
            .map(|bin| (text(bin, "artifactSubType"), number(bin, "hitCount")))
            .collect();
        assert_eq!(bins.iter().filter(|bin| text(bin, "artifactName") == entity).count(), 6);
        assert_eq!(
            counts,
            BTreeMap::from([
                ("create".to_string(), 1.0),
                ("update".to_string(), 1.0),
                ("update-probe".to_string(), 1.0),
                ("refresh".to_string(), 1.0),
                ("one".to_string(), 1.0),
                ("list".to_string(), 1.0),
            ])
        );
    }

    #[test]
    fn audit_services_reuse_the_users_authorization_list_across_seed_writes() {
        let world = world_with_conf(
            r#"<moqui-conf>
            <artifact-execution-facade>
                <artifact-execution type="AT_SERVICE" authz-enabled="true"/>
            </artifact-execution-facade>
            <server-stats bin-length-seconds="600">
                <artifact-stats type="AT_SERVICE" persist-bin="true"/>
                <artifact-stats type="AT_ENTITY" persist-bin="true"/>
            </server-stats>
        </moqui-conf>"#,
        );
        let _entered = world.context.enter();
        let _authz = world.context.disable_authz();
        world.facade.set_disable_audit_log(false);
        let mut user = world.facade.make_value("moqui.security.UserAccount").unwrap();
        user.set("userId", Value::string("AUDIT_CACHE"));
        user.set("username", Value::string("first"));
        user.create(&world.facade).unwrap();
        let mut authz = world.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
        authz.set("artifactAuthzId", Value::string("SEED_CACHE"));
        authz.set("userGroupId", Value::string("ALL_USERS"));
        authz.create(&world.facade).unwrap();
        user.set("username", Value::string("second"));
        user.update(&world.facade).unwrap();
        world.context.factory().flush_artifact_hits().unwrap();
        let bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_ENTITY");
        let checks: Vec<_> = bins
            .iter()
            .filter(|bin| {
                text(bin, "artifactName") == "moqui.security.ArtifactAuthzCheckView"
                    && text(bin, "artifactSubType") == "list"
            })
            .collect();
        assert_eq!(checks.len(), 1);
        assert_eq!(text(checks[0], "hitCount"), "1");
        let bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_SERVICE");
        let audit: Vec<_> = bins
            .iter()
            .filter(|bin| text(bin, "artifactName") == "create#moqui.entity.EntityAuditLog")
            .collect();
        assert_eq!(audit.len(), 1);
        assert_eq!(text(audit[0], "artifactSubType"), "entity-implicit");
        assert_eq!(text(audit[0], "hitCount"), "2");
    }

    #[test]
    fn fk_reads_are_counted_apart_when_a_load_makes_dummy_parents() {
        let world = world();
        let _entered = world.context.enter();
        let _authz = world.context.disable_authz();
        world.facade.set_disable_fk_create(false);
        let mut flow = world.facade.make_value("moqui.basic.StatusFlow").unwrap();
        flow.set("statusFlowId", Value::string("Default"));
        flow.create(&world.facade).unwrap();
        let mut transition = world.facade.make_value("moqui.basic.StatusFlowTransition").unwrap();
        transition.set("statusFlowId", Value::string("Default"));
        transition.set("statusId", Value::string("Missing"));
        transition.set("toStatusId", Value::string("AlsoMissing"));
        // Foreign keys are only checked with dummy foreign keys, as Java's data loader does.
        world.facade.set_dummy_fks(true);
        transition.create(&world.facade).unwrap();
        world.facade.set_dummy_fks(false);
        // A read made after the FK check is counted under its own sub type again.
        world.facade.find("moqui.basic.StatusFlow").unwrap().list().unwrap();
        world.context.factory().flush_artifact_hits().unwrap();
        let bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_ENTITY");
        let status_items: Vec<String> = bins
            .iter()
            .filter(|bin| text(bin, "artifactName") == "moqui.basic.StatusItem")
            .map(|bin| text(bin, "artifactSubType"))
            .collect();
        // The check reads are `fk-check`, not `one`; the dummy row is a create.
        assert_eq!(status_items, ["create", "fk-check"]);
        assert!(
            bins.iter()
                .any(|bin| text(bin, "artifactName") == "moqui.basic.StatusFlow"
                    && text(bin, "artifactSubType") == "list")
        );
    }

    #[test]
    fn existence_reads_do_not_localize_unused_fields_or_disable_later_localization() {
        let world = world();
        let _entered = world.context.enter();
        let _authz = world.context.disable_authz();
        world.facade.set_disable_fk_create(false);
        for id in ["First", "Second"] {
            let mut status = world.facade.make_value("moqui.basic.StatusItem").unwrap();
            status.set("statusId", Value::string(id));
            status.set("description", Value::string(id));
            status.create(&world.facade).unwrap();
            status.create_or_update(&world.facade).unwrap();
        }
        let mut flow = world.facade.make_value("moqui.basic.StatusFlow").unwrap();
        flow.set("statusFlowId", Value::string("Default"));
        flow.create(&world.facade).unwrap();
        let mut transition = world.facade.make_value("moqui.basic.StatusFlowTransition").unwrap();
        transition.set("statusFlowId", Value::string("Default"));
        transition.set("statusId", Value::string("First"));
        transition.set("toStatusId", Value::string("Second"));
        transition.create(&world.facade).unwrap();
        world.context.factory().flush_artifact_hits().unwrap();
        let bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_ENTITY");
        assert!(bins
            .iter()
            .all(|bin| text(bin, "artifactName") != "moqui.basic.LocalizedEntityField"));
        // The two FK existence reads of the transition ran, but Java makes no such read, so they are not hits.
        assert!(bins.iter().all(
            |bin| !(text(bin, "artifactName") == "moqui.basic.StatusItem" && text(bin, "artifactSubType") == "one")
        ));

        let mut translation = world.facade.make_value("moqui.basic.LocalizedEntityField").unwrap();
        for (field, value) in [
            ("entityName", "moqui.basic.StatusItem"),
            ("fieldName", "description"),
            ("pkValue", "First"),
            ("locale", "en"),
            ("localized", "Translated"),
        ] {
            translation.set(field, Value::string(value));
        }
        translation.create(&world.facade).unwrap();
        let found = world
            .facade
            .find("moqui.basic.StatusItem")
            .unwrap()
            .eq("statusId", Value::string("First"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(
            found.get_localized(&world.facade, "description"),
            Some(Value::string("Translated"))
        );
        assert_eq!(found.get("description"), Some(&Value::string("First")));
    }

    #[test]
    fn plain_map_loader_upserts_each_row_with_its_whole_key() {
        let world = world();
        let _entered = world.context.enter();
        let _authz = world.context.disable_authz();
        world.facade.set_disable_fk_create(true);
        crate::entity::load_feed::load_entity_xml_text(
            &world.facade,
            "statistics-loader",
            r#"<entity-facade-xml>
                <moqui.basic.EnumerationType enumTypeId="HitType" description="first"/>
                <moqui.basic.EnumerationType enumTypeId="HitType" description="second"/>
                <moqui.basic.StatusItem statusId="HitStatus"/>
            </entity-facade-xml>"#,
            &crate::entity::load_feed::LoadOptions::default(),
        )
        .unwrap();
        world.context.factory().flush_artifact_hits().unwrap();
        let bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_ENTITY");
        let counts: BTreeMap<_, _> = bins
            .iter()
            .filter(|bin| {
                ["moqui.basic.EnumerationType", "moqui.basic.StatusItem"].contains(&text(bin, "artifactName").as_str())
            })
            .map(|bin| {
                (
                    (text(bin, "artifactName"), text(bin, "artifactSubType")),
                    number(bin, "hitCount"),
                )
            })
            .collect();
        // Each row is one upsert, a create's hit whether the row was there or not. The StatusItem's `statusId` is its
        // primary key, which cannot change, so it has no status change to check against a stored row.
        let expected = [
            ("moqui.basic.EnumerationType", "create", 2.0),
            ("moqui.basic.StatusItem", "create", 1.0),
        ]
        .into_iter()
        .map(|(entity, operation, count)| ((entity.to_string(), operation.to_string()), count))
        .collect();
        assert_eq!(counts, expected);
    }

    #[test]
    fn screen_service_and_entity_hits_are_written_by_the_tick_and_shown_by_the_tools_screens() {
        let world = world();
        let _entered = world.context.enter();
        let _authz = world.context.disable_authz();
        let mut account = world.facade.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", Value::string("HitUser"));
        account.set("username", Value::string("hituser"));
        account.create(&world.facade).unwrap();
        crate::user::internal_login_user("hituser").unwrap();
        let screen = world.dir.join("HitScreen.xml").to_string_lossy().into_owned();
        let untracked = world.dir.join("Untracked.xml").to_string_lossy().into_owned();
        assert!(render(&world, "HitScreen.xml", "").contains("tracked"));
        assert!(render(&world, "HitScreen.xml", "").contains("tracked"));
        assert!(render(&world, "Untracked.xml", "").contains("untracked"));
        render(&world, "HitScreen.xml", "/doIt");
        let mut parameters = BTreeMap::new();
        parameters.insert("enumTypeId".to_string(), Value::string("HitType"));
        parameters.insert("password".to_string(), Value::string("secret"));
        crate::service::sync("find#Hits", &parameters).unwrap();
        world.facade.find("moqui.basic.Enumeration").unwrap().list().unwrap();
        world.facade.find("moqui.basic.Enumeration").unwrap().one().unwrap();

        // Nothing is written on the request path.
        assert!(rows(&world, "moqui.server.ArtifactHit", "AT_XML_SCREEN").is_empty());
        assert!(rows(&world, "moqui.server.ArtifactHitBin", "AT_SERVICE").is_empty());

        // After the bin length, one tick writes the hits and the bins that ended.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        world.context.factory().service_pools().tick();

        let screen_bins = rows(&world, "moqui.server.ArtifactHitBin", "AT_XML_SCREEN");
        // The two renders. The transition's path frame is a separate bin with no sub-type, as in Java.
        let renders: Vec<_> = screen_bins
            .iter()
            .filter(|bin| text(bin, "artifactName") == screen && text(bin, "artifactSubType").starts_with("text/html"))
            .collect();
        assert_eq!(renders.len(), 1, "{screen_bins:?}");
        assert_eq!(number(renders[0], "hitCount"), 2.0);
        assert!(number(renders[0], "minTimeMillis") <= number(renders[0], "maxTimeMillis"));
        assert!(number(renders[0], "totalTimeMillis") > 0.0);
        assert!(!text(renders[0], "binStartDateTime").is_empty());
        assert!(!text(renders[0], "serverHostName").is_empty());
        assert!(screen_bins.iter().all(|bin| text(bin, "artifactName") != untracked));

        let transitions = rows(&world, "moqui.server.ArtifactHitBin", "AT_XML_SCREEN_TRANS");
        let names: Vec<String> = transitions.iter().map(|bin| text(bin, "artifactName")).collect();
        assert!(names.contains(&format!("{screen}#doIt")), "{names:?}");
        assert!(names.contains(&format!("{screen}/doIt")), "{names:?}");

        let services = rows(&world, "moqui.server.ArtifactHitBin", "AT_SERVICE");
        assert_eq!(services.len(), 1, "{services:?}");
        assert_eq!(text(&services[0], "artifactName"), "find#Hits");
        assert_eq!(text(&services[0], "artifactSubType"), "inline");
        assert_eq!(number(&services[0], "hitCount"), 1.0);

        let entities = rows(&world, "moqui.server.ArtifactHitBin", "AT_ENTITY");
        let lists: Vec<_> = entities
            .iter()
            .filter(|bin| {
                text(bin, "artifactName") == "moqui.basic.Enumeration" && text(bin, "artifactSubType") == "list"
            })
            .collect();
        assert_eq!(lists.len(), 1, "{entities:?}");
        assert_eq!(number(lists[0], "hitCount"), 2.0);
        let ones: Vec<_> = entities
            .iter()
            .filter(|bin| {
                text(bin, "artifactName") == "moqui.basic.Enumeration" && text(bin, "artifactSubType") == "one"
            })
            .collect();
        assert_eq!(ones.len(), 1, "{entities:?}");
        assert_eq!(number(ones[0], "hitCount"), 1.0);
        // The hit tables themselves are not counted.
        assert!(entities
            .iter()
            .all(|bin| !text(bin, "artifactName").starts_with("moqui.server.ArtifactHit")));

        let screen_hits: Vec<_> = rows(&world, "moqui.server.ArtifactHit", "AT_XML_SCREEN")
            .into_iter()
            .filter(|hit| text(hit, "artifactName") == screen && text(hit, "artifactSubType").starts_with("text/html"))
            .collect();
        assert_eq!(screen_hits.len(), 2);
        assert_eq!(text(&screen_hits[0], "wasError"), "N");
        assert_eq!(text(&screen_hits[0], "isSlowHit"), "N");
        assert_eq!(text(&screen_hits[0], "userId"), "HitUser");
        assert!(number(&screen_hits[0], "runningTimeMillis") >= 0.0);
        assert!(rows(&world, "moqui.server.ArtifactHit", "AT_XML_SCREEN")
            .iter()
            .all(|hit| text(hit, "artifactName") != untracked));
        let service_hits = rows(&world, "moqui.server.ArtifactHit", "AT_SERVICE");
        assert_eq!(service_hits.len(), 1);
        assert_eq!(text(&service_hits[0], "parameterString"), "enumTypeId=HitType");
        assert!(rows(&world, "moqui.server.ArtifactHit", "AT_ENTITY").is_empty());

        // The tools screens read the tables.
        let tools = format!(
            "{}/../../runtime/base-component/tools/screen/System",
            env!("CARGO_MANIFEST_DIR")
        );
        let mut find = BTreeMap::new();
        find.insert("artifactName".to_string(), Value::string("find#Hits"));
        let bins = crate::screen::make_render(world.context.factory())
            .root_screen(format!("{tools}/ArtifactHitBins.xml"))
            .parameters(find.clone())
            .render()
            .unwrap()
            .body_text();
        assert!(!bins.contains("Error rendering"), "{bins}");
        assert!(bins.contains("find#Hits") && bins.contains("AT_SERVICE"), "{bins}");
        let summary = crate::screen::make_render(world.context.factory())
            .root_screen(format!("{tools}/ArtifactHitSummary.xml"))
            .render()
            .unwrap()
            .body_text();
        assert!(!summary.contains("Error rendering"), "{summary}");
        assert!(summary.contains("ArtifactHitSummaryList"), "{summary}");
        // The in-memory database has no aggregates, so the summary's query is checked as it is sent: the
        // displayed aliases are grouped.
        let _ = crate::screen::make_render(world.context.factory())
            .root_screen(format!("{tools}/ArtifactHitSummary.xml"))
            .parameters(find)
            .render();
        let sql = world
            .tape
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|sql| sql.contains("SUM("))
            .cloned()
            .unwrap_or_default();
        assert!(sql.contains("GROUP BY") && sql.contains("\"artifactName\""), "{sql}");
        crate::user::session::reset_session();
    }
}
