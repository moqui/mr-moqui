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

//! The finished runs (plans/LoadRunner.md §7, D5): each is `runtime/tmp/test-load/<runId>/result.json`, the result of
//! §5 with its `run` block (script, label, user, status, target, start, end). No entity and no database rows. The
//! list of runs is those files, read for their `run` block and summary and cached by the file's time. A run is read,
//! written and deleted only by a [`valid_run_id`], a directory name with no path in it (§10 Result files).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use serde_json::{json, Map, Value};

use moqui_core::context::ExecutionContextFactory;

/// The result file in a run's directory.
pub(crate) const RESULT_FILE: &str = "result.json";
/// The statuses of a finished run.
pub(crate) const STATUSES: [&str; 3] = ["completed", "stopped", "failed"];
/// The format of a runId's time, in UTC.
const RUN_ID_TIME: &str = "%Y%m%d-%H%M%S";
/// The most `-<n>` suffixes tried for runs started in the same second by the same process.
const MAX_SUFFIX: u32 = 999;

/// The directory of the runs: `runtime/tmp/test-load`, which the command writes to by default too (§9).
pub(crate) fn root(factory: &ExecutionContextFactory) -> PathBuf {
    factory.runtime_dir().join("tmp").join("test-load")
}

/// Whether `id` is a runId: `<yyyyMMdd-HHmmss>-<pid>` with an optional `-<n>`, digits only, so it can name nothing but
/// a directory under the root.
pub(crate) fn valid_run_id(id: &str) -> bool {
    let digits = |part: &str, min: usize, max: usize| {
        (min..=max).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    let parts: Vec<&str> = id.split('-').collect();
    match parts.as_slice() {
        [date, time, pid] => digits(date, 8, 8) && digits(time, 6, 6) && digits(pid, 1, 10),
        [date, time, pid, n] => digits(date, 8, 8) && digits(time, 6, 6) && digits(pid, 1, 10) && digits(n, 1, 3),
        _ => false,
    }
}

/// The runId of a run started at `now` by process `pid`, before any `-<n>` suffix.
pub(crate) fn run_id_base(now: jiff::Timestamp, pid: u32) -> String {
    format!("{}-{pid}", now.strftime(RUN_ID_TIME))
}

/// The start time a runId names, in milliseconds since the epoch.
fn run_id_millis(id: &str) -> Option<i64> {
    let stamp = id.get(..15)?;
    let civil = jiff::civil::DateTime::strptime(RUN_ID_TIME, stamp).ok()?;
    let zoned = civil.to_zoned(jiff::tz::TimeZone::UTC).ok()?;
    Some(zoned.timestamp().as_millisecond())
}

/// A run's directory, made new: `base`, else `base-2`, `base-3` and on when another run took it in the same second.
/// Answers the runId and the directory.
pub(crate) fn create_run_dir(root: &Path, base: &str) -> Result<(String, PathBuf), String> {
    std::fs::create_dir_all(root).map_err(|err| format!("{}: {err}", root.display()))?;
    for n in 1..=MAX_SUFFIX {
        let id = if n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        if !valid_run_id(&id) {
            return Err(format!("{id} is not a runId"));
        }
        let dir = root.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        }
    }
    Err(format!("no free runId after {base}"))
}

/// The directory of run `id`; None when `id` is not a runId.
pub(crate) fn run_dir(root: &Path, id: &str) -> Option<PathBuf> {
    valid_run_id(id).then(|| root.join(id))
}

/// Write run `id`'s result, through a temporary file renamed into place so a reader never sees half a file.
pub(crate) fn write_result(root: &Path, id: &str, result: &Value) -> Result<(), String> {
    let Some(dir) = run_dir(root, id) else {
        return Err(format!("{id} is not a runId"));
    };
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let text = serde_json::to_vec_pretty(result).map_err(|err| err.to_string())?;
    let partial = dir.join(format!("{RESULT_FILE}.partial"));
    std::fs::write(&partial, text).map_err(|err| format!("{}: {err}", partial.display()))?;
    let file = dir.join(RESULT_FILE);
    std::fs::rename(&partial, &file).map_err(|err| format!("{}: {err}", file.display()))
}

/// Run `id`'s whole result; None when there is no such finished run.
pub(crate) fn read_result(root: &Path, id: &str) -> Result<Option<Value>, String> {
    let Some(dir) = run_dir(root, id) else {
        return Err(format!("{id} is not a runId"));
    };
    let file = dir.join(RESULT_FILE);
    let text = match std::fs::read(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", file.display())),
    };
    serde_json::from_slice(&text)
        .map(Some)
        .map_err(|err| format!("{}: {err}", file.display()))
}

/// Delete run `id`'s directory; false when there was none.
pub(crate) fn delete(root: &Path, id: &str) -> Result<bool, String> {
    let Some(dir) = run_dir(root, id) else {
        return Err(format!("{id} is not a runId"));
    };
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {
            lock_cache().remove(&dir.join(RESULT_FILE));
            Ok(true)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("{}: {err}", dir.display())),
    }
}

/// Whether `value` is a load result (§5), as an import checks before it keeps one: an object with the totals, the
/// latency and the names, and a `run` block whose status, when it has one, is a finished run's.
pub(crate) fn is_result(value: &Value) -> bool {
    let Some(result) = value.as_object() else {
        return false;
    };
    let run_ok = match result.get("run") {
        Some(Value::Object(run)) => match run.get("status") {
            None | Some(Value::Null) => true,
            Some(Value::String(status)) => STATUSES.contains(&status.as_str()),
            Some(_) => false,
        },
        _ => false,
    };
    run_ok
        && result.get("requests").is_some_and(Value::is_number)
        && result.get("errors").is_some_and(Value::is_number)
        && result.get("latency").is_some_and(Value::is_object)
        && result.get("names").is_some_and(Value::is_object)
}

/// What [`list`] keeps: runs of one script, or of one status.
#[derive(Clone, Debug, Default)]
pub(crate) struct Find<'a> {
    pub(crate) script: Option<&'a str>,
    pub(crate) status: Option<&'a str>,
}

impl Find<'_> {
    pub(crate) fn keeps(&self, summary: &Value) -> bool {
        let field = |name: &str| summary.get(name).and_then(Value::as_str).unwrap_or_default();
        self.script.is_none_or(|script| field("script") == script)
            && self.status.is_none_or(|status| field("status") == status)
    }
}

/// A run's summary for the list and the poll: its runId, its `run` block's fields, and the totals of its result.
pub(crate) fn summary(id: &str, result: &Value) -> Value {
    let run = result.get("run").cloned().unwrap_or_else(|| json!({}));
    let mut out = Map::new();
    out.insert("runId".into(), json!(id));
    for key in ["script", "label", "user", "status", "target", "start", "end"] {
        out.insert(key.into(), run.get(key).cloned().unwrap_or(Value::Null));
    }
    for key in [
        "users",
        "iterations",
        "requests",
        "errors",
        "errorRate",
        "rps",
        "passed",
        "duration",
    ] {
        out.insert(key.into(), result.get(key).cloned().unwrap_or(Value::Null));
    }
    let p95 = result.get("latency").and_then(|latency| latency.get("p95")).cloned();
    out.insert("p95".into(), p95.unwrap_or(Value::Null));
    Value::Object(out)
}

/// A summary's start in milliseconds since the epoch, for the order of the list: `run.start` as a number or an
/// RFC 3339 time, else the time in the runId.
fn start_millis(summary: &Value) -> i64 {
    let from_start = match summary.get("start") {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => text.parse::<jiff::Timestamp>().ok().map(|at| at.as_millisecond()),
        _ => None,
    };
    let id = summary.get("runId").and_then(Value::as_str).unwrap_or_default();
    from_start.or_else(|| run_id_millis(id)).unwrap_or(0)
}

/// What the cache holds for a result file: its summary, or None when it did not read or is not a result (logged
/// once, while the file's time and length stay the same).
type SummaryCache = HashMap<PathBuf, (SystemTime, u64, Option<Value>)>;

static SUMMARIES: LazyLock<Mutex<SummaryCache>> = LazyLock::new(Mutex::default);

fn lock_cache() -> std::sync::MutexGuard<'static, SummaryCache> {
    moqui_lib::sync::lock(&SUMMARIES)
}

/// The finished runs under `root` that `find` keeps, newest first. A directory without a result (a run still
/// running, or one cut off) is left out; a result that does not read is logged and left out, and the rest still list.
pub(crate) fn list(root: &Path, find: &Find) -> Vec<Value> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::error!(dir = %root.display(), error = %err, "Could not list the load runs");
            return Vec::new();
        }
    };
    let mut seen = Vec::new();
    let mut summaries = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::error!(dir = %root.display(), error = %err, "Could not read an entry of the load runs");
                continue;
            }
        };
        let name = entry.file_name();
        let Some(id) = name.to_str().filter(|id| valid_run_id(id)) else {
            continue;
        };
        let file = entry.path().join(RESULT_FILE);
        if let Some(summary) = cached_summary(&file, id) {
            if find.keeps(&summary) {
                summaries.push(summary);
            }
        }
        seen.push(file);
    }
    // forget the files of this root whose run directories are gone
    lock_cache().retain(|file, _| !file.starts_with(root) || seen.contains(file));
    let mut keyed: Vec<(i64, String, Value)> = summaries
        .into_iter()
        .map(|summary| {
            let id = summary
                .get("runId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            (start_millis(&summary), id, summary)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    keyed.into_iter().map(|(_, _, summary)| summary).collect()
}

/// The summary of the result file `file`, from the cache while the file's time and length are the same. A file
/// that does not read is cached as such too, so it is logged once and not on every listing.
fn cached_summary(file: &Path, id: &str) -> Option<Value> {
    let meta = std::fs::metadata(file).ok()?;
    let (modified, len) = (meta.modified().ok()?, meta.len());
    if let Some((at, at_len, summary)) = lock_cache().get(file) {
        if *at == modified && *at_len == len {
            return summary.clone();
        }
    }
    let read = std::fs::read(file)
        .map_err(|err| err.to_string())
        .and_then(|text| serde_json::from_slice::<Value>(&text).map_err(|err| err.to_string()));
    let summary = match read {
        Ok(result) if is_result(&result) => Some(summary(id, &result)),
        Ok(_) => {
            tracing::error!(file = %file.display(), "Not a load result; left out of the runs");
            None
        }
        Err(err) => {
            tracing::error!(file = %file.display(), error = %err, "Could not read a load result; left out of the runs");
            None
        }
    };
    lock_cache().insert(file.to_path_buf(), (modified, len, summary.clone()));
    summary
}

/// The most text an imported result may have.
pub(crate) const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;

/// A result from another machine made ready to keep as a run (§7, §10 Result files): checked to be a result and at
/// most [`MAX_IMPORT_BYTES`], its `run.user` the importer (the file's is kept as `run.importedUser`), its label
/// `label` when given, its status `completed` when it has none, and `run.imported` true.
///
/// Nothing in an imported result is trusted. Its `run.script` and `run.target` are what another machine wrote: Run
/// again on an imported run must resolve the script again through `scripts::resolve` among the loaded components and
/// check the target again through `TargetPolicy::allow`, never start from the file's values as given.
pub(crate) fn imported_result(text: &str, importer: &str, label: &str) -> Result<Value, String> {
    if text.len() > MAX_IMPORT_BYTES {
        return Err(format!(
            "The result is {} bytes; an import takes at most {MAX_IMPORT_BYTES}",
            text.len()
        ));
    }
    let mut result: Value = serde_json::from_str(text).map_err(|err| format!("The result is not JSON: {err}"))?;
    if !is_result(&result) {
        return Err("That is not a load result".to_string());
    }
    let Some(run) = result.get_mut("run").and_then(Value::as_object_mut) else {
        return Err("That is not a load result".to_string());
    };
    let file_user = run.remove("user").unwrap_or(Value::Null);
    run.insert("importedUser".into(), file_user);
    run.insert("user".into(), json!(importer));
    if !label.is_empty() {
        run.insert("label".into(), json!(label));
    }
    if run.get("status").is_none_or(Value::is_null) {
        run.insert("status".into(), json!("completed"));
    }
    run.insert("imported".into(), json!(true));
    Ok(result)
}

#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;
