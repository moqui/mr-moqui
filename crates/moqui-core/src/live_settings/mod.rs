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

//! Timed overrides of logging and telemetry settings (plans/archive/Monitor-Settings.md; guide docs/tools/Monitoring.md,
//! "Changing settings for a while"). An override is in memory only: it always expires, at most 24 hours on, and is gone
//! on restart. The on-disk value (the merged conf, or `moqui_log_level`) is always known beside it. A change applies
//! here and is published on the `runtime-settings` cluster topic, so it applies to every running instance; each
//! instance expires its own copy at the absolute time the message carries.

pub mod catalog;
pub mod log_filter;

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use moqui_lib::sync::{lock, wait_timeout};
use serde_json::{json, Value as Json};

use crate::context::ExecutionContextFactory;
use catalog::{Applies, Entry, Source};

/// The log target of every change and expiry.
pub const AUDIT: &str = "moqui.monitor.audit";
/// How long an override may be set for, in minutes.
pub const DURATIONS: [i64; 4] = [15, 60, 240, 1440];
const MINUTE: i64 = 60_000;
/// The latest an override may expire, from now.
pub const MAX_MILLIS: i64 = 1440 * MINUTE;
const MAX_REASON: usize = 1000;

/// One override.
#[derive(Clone, Debug, PartialEq)]
pub struct Override {
    pub key: String,
    /// The canonical value ([`catalog::Kind::parse`]).
    pub value: String,
    /// UTC milliseconds.
    pub expires_at: i64,
    pub set_by: String,
    pub set_at: i64,
    pub reason: String,
}

impl Override {
    pub fn to_json(&self) -> Json {
        json!({
            "key": self.key,
            "value": self.value,
            "expiresAt": self.expires_at,
            "setBy": self.set_by,
            "setAt": self.set_at,
            "reason": self.reason,
        })
    }

    fn from_json(value: &Json) -> Option<Self> {
        Some(Self {
            key: value.get("key")?.as_str()?.to_string(),
            value: value.get("value")?.as_str()?.to_string(),
            expires_at: value.get("expiresAt")?.as_i64()?,
            set_by: value.get("setBy").and_then(Json::as_str).unwrap_or("").to_string(),
            set_at: value.get("setAt").and_then(Json::as_i64).unwrap_or(0),
            reason: value.get("reason").and_then(Json::as_str).unwrap_or("").to_string(),
        })
    }
}

/// A request to set or extend an override, before it is checked.
pub struct Change<'a> {
    pub key: &'a str,
    pub value: &'a str,
    pub minutes: i64,
    pub reason: &'a str,
    pub user: &'a str,
}

/// The factory's overrides.
#[derive(Default)]
struct Registry {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    factory: Weak<ExecutionContextFactory>,
    overrides: BTreeMap<String, Override>,
    /// The expiry thread is running; it stops when no override is left.
    expiring: bool,
    /// Keeps the cluster listener.
    subscription: Option<crate::notify::Subscription>,
}

fn registry(factory: &ExecutionContextFactory) -> Arc<Registry> {
    factory.extension(|_| Registry::default())
}

/// Listen for other instances' changes. The factory's boot calls this once.
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>) {
    let registry = registry(factory);
    let weak = Arc::downgrade(factory);
    lock(&registry.state).factory = weak.clone();
    let listener: crate::notify::Listener = Arc::new(move |message| {
        // A change of this process was applied before it was published.
        if message.origin_id == crate::notify::origin_id() {
            return;
        }
        if let Some(factory) = weak.upgrade() {
            apply_message(&factory, &message.body);
        }
    });
    match crate::notify::subscribe(crate::notify::RUNTIME_SETTINGS, listener) {
        Ok(subscription) => lock(&registry.state).subscription = Some(subscription),
        Err(err) => tracing::error!(
            "runtime settings: the cluster topic was not subscribed: {}",
            err.message()
        ),
    }
}

pub fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// A setting as the screen shows it.
#[derive(Clone, Debug)]
pub struct Setting {
    pub entry: Entry,
    pub on_disk: String,
    pub source: Source,
    pub effective: String,
    pub active: Option<Override>,
}

impl Setting {
    pub fn to_json(&self) -> Json {
        let (path, attr) = catalog::split_key(&self.entry.key);
        let (_, subject) = catalog::split_selector(path);
        json!({
            "key": self.entry.key,
            "name": self.entry.name,
            "group": self.entry.group.as_str(),
            "subject": subject.unwrap_or(""),
            "attribute": attr,
            "choices": self.entry.kind.choices(),
            "onDisk": self.on_disk,
            "source": self.source.as_str(),
            "effective": self.effective,
            "override": self.active.as_ref().map(Override::to_json),
        })
    }
}

/// Every setting of the catalog, then each target rule, with its on-disk value and any active override.
pub fn list(factory: &ExecutionContextFactory) -> Vec<Setting> {
    let overrides = active(factory);
    let mut settings: Vec<Setting> = catalog::entries()
        .iter()
        .map(|entry| setting(factory, entry.clone(), overrides.get(&entry.key).cloned()))
        .collect();
    for (key, active) in &overrides {
        if catalog::target_of(key).is_some() {
            if let Ok(entry) = catalog::lookup(key) {
                settings.push(setting(factory, entry, Some(active.clone())));
            }
        }
    }
    settings
}

fn setting(factory: &ExecutionContextFactory, entry: Entry, active: Option<Override>) -> Setting {
    let (on_disk, source) = catalog::on_disk(factory, &entry);
    let effective = active
        .as_ref()
        .map_or_else(|| on_disk.clone(), |active| active.value.clone());
    Setting {
        entry,
        on_disk,
        source,
        effective,
        active,
    }
}

/// The overrides now in force, by key.
pub fn active(factory: &ExecutionContextFactory) -> BTreeMap<String, Override> {
    let now = now_millis();
    let registry = registry(factory);
    let state = lock(&registry.state);
    state
        .overrides
        .iter()
        .filter(|(_, active)| active.expires_at > now)
        .map(|(key, active)| (key.clone(), active.clone()))
        .collect()
}

/// The value of `key` now: its override while one is active, else the on-disk value.
pub fn effective(factory: &ExecutionContextFactory, key: &str) -> Result<String, String> {
    let entry = catalog::lookup(key)?;
    Ok(match active(factory).remove(key) {
        Some(active) => active.value,
        None => catalog::on_disk(factory, &entry).0,
    })
}

/// Check a change and make the override it asks for. Its expiry is `minutes` from `now`.
pub fn validate(change: &Change<'_>, now: i64) -> Result<Override, String> {
    let entry = catalog::lookup(change.key)?;
    if !DURATIONS.contains(&change.minutes) {
        return Err(format!(
            "an override is set for 15, 60, 240 or 1440 minutes, not {}",
            change.minutes
        ));
    }
    let reason = change.reason.trim();
    if reason.is_empty() {
        return Err("a reason is required".to_string());
    }
    if reason.len() > MAX_REASON {
        return Err(format!("the reason is longer than {MAX_REASON} bytes"));
    }
    let value = entry.kind.parse(change.value)?;
    if entry.key == "logging@console-level" && log_filter::disk().console == log_filter::Console::Off {
        return Err("this server writes no console".to_string());
    }
    Ok(Override {
        key: entry.key,
        value,
        expires_at: now + change.minutes * MINUTE,
        set_by: change.user.to_string(),
        set_at: now,
        reason: crate::telemetry::redact_text(reason),
    })
}

/// What [`set`] did.
#[derive(Clone, Debug)]
pub struct Applied {
    pub made: Override,
    pub on_disk: String,
    /// It replaced an active override of the same key with a new expiry.
    pub extended: bool,
}

/// Set or extend an override here and on every running instance.
pub fn set(factory: &ExecutionContextFactory, change: &Change<'_>) -> Result<Applied, String> {
    let made = validate(change, now_millis())?;
    let entry = catalog::lookup(&made.key)?;
    let (on_disk, _) = catalog::on_disk(factory, &entry);
    let extended = active(factory).contains_key(&made.key);
    put(factory, made.clone());
    tracing::warn!(
        target: AUDIT,
        user = %made.set_by,
        key = %made.key,
        on_disk = %on_disk,
        value = %made.value,
        expires_at = %stamp(made.expires_at),
        reason = %made.reason,
        "runtime setting {}",
        if extended { "extended" } else { "set" }
    );
    publish(factory, json!({"op": "set", "override": made.to_json()}));
    Ok(Applied {
        made,
        on_disk,
        extended,
    })
}

/// Revert `key` to its on-disk value here and on every running instance. Returns the override it ended, if any.
pub fn revert(factory: &ExecutionContextFactory, key: &str, user: &str) -> Result<Option<Override>, String> {
    let entry = catalog::lookup(key)?;
    let ended = remove(factory, std::slice::from_ref(&entry.key)).pop();
    if let Some(ended) = &ended {
        let (on_disk, _) = catalog::on_disk(factory, &entry);
        tracing::warn!(
            target: AUDIT,
            user,
            key = %ended.key,
            on_disk = %on_disk,
            value = %ended.value,
            "runtime setting reverted to on-disk"
        );
    }
    publish(factory, json!({"op": "revert", "key": entry.key}));
    Ok(ended)
}

/// Revert every override here and on every running instance. Returns the overrides it ended.
pub fn revert_all(factory: &ExecutionContextFactory, user: &str) -> Vec<Override> {
    let keys: Vec<String> = lock(&registry(factory).state).overrides.keys().cloned().collect();
    let ended = remove(factory, &keys);
    for ended in &ended {
        tracing::warn!(target: AUDIT, user, key = %ended.key, on_disk = %on_disk_of(factory, &ended.key),
            value = %ended.value, "runtime setting reverted to on-disk");
    }
    publish(factory, json!({"op": "revertAll"}));
    ended
}

/// The on-disk value of `key` for an audit line, empty for a key the catalog does not have.
fn on_disk_of(factory: &ExecutionContextFactory, key: &str) -> String {
    catalog::lookup(key)
        .map(|entry| catalog::on_disk(factory, &entry).0)
        .unwrap_or_default()
}

/// Apply a `runtime-settings` message from another instance. A key this instance does not know, or an override that
/// is past or too far out, is logged and skipped.
pub fn apply_message(factory: &ExecutionContextFactory, body: &Json) {
    match body.get("op").and_then(Json::as_str) {
        Some("set") => {
            let Some(made) = body.get("override").and_then(Override::from_json) else {
                tracing::error!("runtime settings: a set message without an override was skipped");
                return;
            };
            if let Err(error) = check_remote(&made, now_millis()) {
                tracing::error!(key = %made.key, "runtime settings: an override from another instance was skipped: {error}");
                return;
            }
            put(factory, made.clone());
            tracing::warn!(target: AUDIT, user = %made.set_by, key = %made.key,
                on_disk = %on_disk_of(factory, &made.key), value = %made.value,
                expires_at = %stamp(made.expires_at), reason = %made.reason,
                "runtime setting set from another instance");
        }
        Some("revert") => {
            let Some(key) = body.get("key").and_then(Json::as_str) else {
                tracing::error!("runtime settings: a revert message without a key was skipped");
                return;
            };
            if !remove(factory, &[key.to_string()]).is_empty() {
                tracing::warn!(target: AUDIT, key, "runtime setting reverted from another instance");
            }
        }
        Some("revertAll") => {
            let keys: Vec<String> = lock(&registry(factory).state).overrides.keys().cloned().collect();
            if !remove(factory, &keys).is_empty() {
                tracing::warn!(target: AUDIT, "runtime settings all reverted from another instance");
            }
        }
        other => tracing::error!(op = ?other, "runtime settings: a message of an unknown kind was skipped"),
    }
}

/// An override from another instance: a known key, a value this instance accepts, and an expiry within the cap.
fn check_remote(made: &Override, now: i64) -> Result<(), String> {
    let entry = catalog::lookup(&made.key)?;
    entry.kind.parse(&made.value)?;
    if made.expires_at <= now {
        return Err("it has already expired".to_string());
    }
    if made.expires_at > now + MAX_MILLIS + MINUTE {
        return Err("it expires more than 24 hours from now".to_string());
    }
    Ok(())
}

/// Store an override, rebuild what it applies to, and wake the expiry.
fn put(factory: &ExecutionContextFactory, made: Override) {
    let registry = registry(factory);
    let applies = catalog::applies(&made.key);
    {
        let mut state = lock(&registry.state);
        state.overrides.insert(made.key.clone(), made);
        start_expiry(&registry, &mut state);
    }
    registry.changed.notify_all();
    rebuild(factory, &[applies]);
}

/// Drop the overrides of `keys` and rebuild what they applied to. Returns those that were there.
fn remove(factory: &ExecutionContextFactory, keys: &[String]) -> Vec<Override> {
    let registry = registry(factory);
    let ended: Vec<Override> = {
        let mut state = lock(&registry.state);
        keys.iter().filter_map(|key| state.overrides.remove(key)).collect()
    };
    if !ended.is_empty() {
        registry.changed.notify_all();
        let applies: Vec<Applies> = ended.iter().map(|ended| catalog::applies(&ended.key)).collect();
        rebuild(factory, &applies);
    }
    ended
}

/// Rebuild each group in `applies` from its on-disk value plus the active overrides.
fn rebuild(factory: &ExecutionContextFactory, applies: &[Applies]) {
    let mut groups = applies.to_vec();
    groups.sort();
    groups.dedup();
    let overrides = active(factory);
    for group in groups {
        match group {
            Applies::Logging => log_filter::set(log_filter_of(&overrides)),
            Applies::Telemetry => {
                let telemetry = crate::telemetry::for_factory(factory);
                let config = telemetry_config(telemetry.config(), &overrides);
                telemetry.set_live(config);
            }
            Applies::Conf => factory.set_conf_overlay(
                overrides
                    .iter()
                    .filter(|(key, _)| catalog::applies(key) == Applies::Conf)
                    .map(|(key, active)| (key.clone(), active.value.clone()))
                    .collect(),
            ),
        }
    }
}

/// The on-disk log filter with the logging overrides applied.
pub fn log_filter_of(overrides: &BTreeMap<String, Override>) -> log_filter::LogFilter {
    let mut filter = log_filter::disk();
    for (key, active) in overrides {
        let Some(level) = log_filter::parse_level(&active.value) else {
            continue;
        };
        match key.as_str() {
            "logging@level" => filter.level = level,
            "logging@console-level" => {
                if filter.console != log_filter::Console::Off {
                    filter.console = log_filter::Console::AtLeast(level);
                }
            }
            key => {
                if let Some(target) = catalog::target_of(key) {
                    filter.targets.push((target.to_string(), level));
                }
            }
        }
    }
    filter.sort_targets();
    filter
}

/// The telemetry config the factory started with, with the telemetry overrides applied.
pub fn telemetry_config(
    disk: &crate::telemetry::Config,
    overrides: &BTreeMap<String, Override>,
) -> crate::telemetry::Config {
    let mut config = disk.clone();
    for (key, active) in overrides {
        if catalog::applies(key) != Applies::Telemetry {
            continue;
        }
        let (path, attr) = catalog::split_key(key);
        let value = active.value.as_str();
        let ratio = || value.parse::<f64>().ok();
        let whole = || value.parse::<u64>().ok();
        match (catalog::split_selector(path), attr) {
            (("telemetry", None), "sample-ratio") => {
                if let Some(ratio) = ratio() {
                    config.sample_ratio = ratio;
                }
            }
            (("telemetry", None), "slow-root-ms") => {
                if let Some(millis) = whole() {
                    config.slow_root = Duration::from_millis(millis);
                }
            }
            (("telemetry/artifact", Some(kind)), "spans" | "metrics") => {
                let default = config.artifact_types.get(kind).copied().unwrap_or((true, true));
                let on = value == "true";
                let pair = if attr == "spans" {
                    (on, default.1)
                } else {
                    (default.0, on)
                };
                config.artifact_types.insert(kind.to_string(), pair);
            }
            (("telemetry/artifact", Some(kind)), "sample-ratio") => {
                if let Some(ratio) = ratio() {
                    config.artifact_sample_ratios.insert(kind.to_string(), ratio);
                }
            }
            (("telemetry/metric", Some(name)), "attributes") => {
                let keys = value
                    .split(',')
                    .filter(|key| !key.is_empty())
                    .map(str::to_string)
                    .collect();
                config.metric_attributes.insert(name.to_string(), keys);
            }
            (("telemetry/operations", None), "record") => {
                if let Some(record) = crate::telemetry::operation::Record::parse(value) {
                    config.operations.record = record;
                }
            }
            (("telemetry/operations", None), "max-names") => {
                if let Some(max) = whole() {
                    config.operations.max_names = max as usize;
                }
            }
            _ => tracing::error!(key, "runtime settings: a telemetry override was not applied"),
        }
    }
    config
}

/// Publish a change to the other instances. Without an entity facade or a cluster it stays here.
fn publish(factory: &ExecutionContextFactory, body: Json) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let message = match crate::cluster::prepare_message(body) {
        Ok(message) => message,
        Err(err) => {
            tracing::error!("runtime settings: not published to the cluster: {}", err.message());
            return;
        }
    };
    if let Err(err) = crate::cluster::publish(&facade, crate::notify::RUNTIME_SETTINGS, &message) {
        tracing::error!("runtime settings: not published to the cluster: {}", err.message());
    }
}

/// Start the expiry thread unless it runs. It sleeps until the next expiry, and is woken by each change.
fn start_expiry(registry: &Arc<Registry>, state: &mut State) {
    if state.expiring {
        return;
    }
    let Some(factory) = state.factory.upgrade() else {
        // A factory that did not install (a test context): expiry is checked when the overrides are read.
        return;
    };
    let weak = state.factory.clone();
    let registry = Arc::clone(registry);
    let started = factory.spawn_background("moqui-live-settings", move || expire_loop(&registry, &weak));
    match started {
        Ok(()) => state.expiring = true,
        Err(err) => tracing::error!("runtime settings: the expiry thread did not start: {}", err.message()),
    }
}

/// The longest the expiry thread sleeps at once, so it sees the factory shut down.
const POLL: Duration = Duration::from_secs(1);

fn expire_loop(registry: &Registry, weak: &Weak<ExecutionContextFactory>) {
    loop {
        let Some(factory) = weak.upgrade().filter(|factory| !factory.is_shutting_down()) else {
            lock(&registry.state).expiring = false;
            return;
        };
        let now = now_millis();
        let (due, next) = {
            let mut state = lock(&registry.state);
            let due: Vec<String> = state
                .overrides
                .values()
                .filter(|active| active.expires_at <= now)
                .map(|active| active.key.clone())
                .collect();
            let due: Vec<Override> = due.iter().filter_map(|key| state.overrides.remove(key)).collect();
            let next = state.overrides.values().map(|active| active.expires_at).min();
            if due.is_empty() && next.is_none() {
                state.expiring = false;
                return;
            }
            (due, next)
        };
        if !due.is_empty() {
            for ended in &due {
                tracing::warn!(target: AUDIT, user = %ended.set_by, key = %ended.key,
                    on_disk = %on_disk_of(&factory, &ended.key), value = %ended.value,
                    "runtime setting expired; back to on-disk");
            }
            let applies: Vec<Applies> = due.iter().map(|ended| catalog::applies(&ended.key)).collect();
            rebuild(&factory, &applies);
        }
        drop(factory);
        let Some(next) = next else {
            continue;
        };
        let wait = Duration::from_millis((next - now_millis()).max(0) as u64).min(POLL);
        let state = lock(&registry.state);
        let _ = wait_timeout(&registry.changed, state, wait);
    }
}

/// A UTC timestamp for the audit lines.
fn stamp(millis: i64) -> String {
    jiff::Timestamp::from_millisecond(millis).map_or_else(|_| millis.to_string(), |at| at.to_string())
}

/// This instance's id and the live members of its cluster (1 without one).
pub fn instance(factory: &ExecutionContextFactory) -> (String, usize) {
    let id = crate::cluster::instance_id(factory)
        .unwrap_or_else(|| crate::telemetry::for_factory(factory).resource().instance_id);
    let members = factory
        .entity()
        .and_then(|facade| crate::cluster::live_member_count(factory, &facade).ok())
        .unwrap_or(0)
        .max(1);
    (id, members)
}

#[cfg(test)]
mod tests;
