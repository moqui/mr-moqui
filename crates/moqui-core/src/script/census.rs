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

//! The census of small scripts (plans/Script-Engine-Oxide.md section 5.3): with `MOQUI_SCRIPT_CENSUS` on (or
//! `MOQUI_SCRIPT_CENSUS_FILE` set), each distinct small-script text is recorded once with its entry kind, text hash,
//! the routes it took and how often, the static check's verdict and reason, the `ec` paths the text names and the host
//! paths its runs reached (the binders call [`note`]). The test command writes it as JSON lines at the end of a run
//! ([`write_census`]). Off, a note is one thread-local count, which the router also reads to know whether an oxide
//! run reached a host call.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::script::check::Verdict;
use crate::script::engine::{EntryKind, Route};

/// Distinct texts the census keeps at most; past it new texts are counted only.
const MAX_ROWS: usize = 200_000;
/// The text kept with a row, at most.
const MAX_TEXT: usize = 400;

/// One distinct text of one entry kind.
#[derive(Default)]
struct Row {
    text: String,
    runs: u64,
    routes: BTreeMap<&'static str, u64>,
    passes: bool,
    reason: Option<String>,
    why_quickjs: BTreeMap<&'static str, u64>,
    ec_paths: Vec<String>,
    reached: BTreeSet<&'static str>,
    fallbacks: BTreeMap<String, u64>,
}

#[derive(Default)]
struct Census {
    rows: HashMap<(EntryKind, u64), Row>,
    dropped: u64,
}

static CENSUS: LazyLock<Mutex<Census>> = LazyLock::new(|| Mutex::new(Census::default()));

thread_local! {
    /// Host calls (binder trampolines) made on this thread since it started; see [`host_calls`].
    static HOST_CALLS: Cell<u64> = const { Cell::new(0) };
    /// The row the running small script on this thread records into, while one runs ([`begin`]).
    static CURRENT: RefCell<Vec<(EntryKind, u64)>> = const { RefCell::new(Vec::new()) };
}

/// Whether the census records, read once: `MOQUI_SCRIPT_CENSUS` is `true` or `1`, or `MOQUI_SCRIPT_CENSUS_FILE` is set.
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let flag = moqui_lib::env::get_prop_or_env("MOQUI_SCRIPT_CENSUS");
        matches!(flag.trim(), "true" | "1") || !census_file().is_empty()
    })
}

fn census_file() -> String {
    moqui_lib::env::get_prop_or_env("MOQUI_SCRIPT_CENSUS_FILE")
}

/// A host function or `ec` path reached by the running script, from a binder's trampoline (both engines). Counted
/// always, recorded only when the census is on.
pub(crate) fn note(path: &'static str) {
    HOST_CALLS.with(|calls| calls.set(calls.get() + 1));
    if !enabled() {
        return;
    }
    let Some(current) = CURRENT.with(|current| current.borrow().last().copied()) else {
        return;
    };
    if let Some(row) = moqui_lib::sync::lock(&CENSUS).rows.get_mut(&current) {
        row.reached.insert(path);
    }
}

/// Host calls made on this thread so far: an oxide run that made none may run again on QuickJS.
pub(crate) fn host_calls() -> u64 {
    HOST_CALLS.with(Cell::get)
}

/// The running script's census entry, from [`begin`] until dropped.
pub(crate) struct Entry {
    active: bool,
}

/// Start the census entry of a small script of `kind` with the text hash `hash`, for the paths its run reaches.
pub(crate) fn begin(kind: EntryKind, hash: u64) -> Entry {
    let active = enabled() && kind.is_small();
    if active {
        CURRENT.with(|current| current.borrow_mut().push((kind, hash)));
    }
    Entry { active }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if self.active {
            CURRENT.with(|current| {
                current.borrow_mut().pop();
            });
        }
    }
}

/// Record one run of `text` of `kind`: the route the router took and, for a QuickJS route of a kind oxide covers, why.
pub(crate) fn record(
    kind: EntryKind,
    text: &str,
    hash: u64,
    route: Route,
    verdict: &Verdict,
    why_quickjs: Option<&'static str>,
) {
    let mut census = moqui_lib::sync::lock(&CENSUS);
    let full = census.rows.len() >= MAX_ROWS;
    if full && !census.rows.contains_key(&(kind, hash)) {
        census.dropped += 1;
        return;
    }
    let row = census.rows.entry((kind, hash)).or_insert_with(|| Row {
        text: clip(text),
        passes: verdict.passes(),
        reason: verdict.reason(),
        ec_paths: verdict.ec_paths().to_vec(),
        ..Row::default()
    });
    row.runs += 1;
    *row.routes.entry(route.name()).or_default() += 1;
    if let Some(why) = why_quickjs {
        *row.why_quickjs.entry(why).or_default() += 1;
    }
}

/// Record an oxide run of the text of `kind` and `hash` that went to QuickJS before any host call, and why.
pub(crate) fn fallback(kind: EntryKind, hash: u64, reason: &str) {
    if !enabled() {
        return;
    }
    if let Some(row) = moqui_lib::sync::lock(&CENSUS).rows.get_mut(&(kind, hash)) {
        *row.fallbacks.entry(reason.to_string()).or_default() += 1;
    }
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_string();
    }
    let mut end = MAX_TEXT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// The census as JSON lines, one per row, the most run first.
pub fn census_lines() -> Vec<String> {
    let census = moqui_lib::sync::lock(&CENSUS);
    let mut rows: Vec<(&(EntryKind, u64), &Row)> = census.rows.iter().collect();
    rows.sort_by(|a, b| b.1.runs.cmp(&a.1.runs).then(a.0.cmp(b.0)));
    let mut lines: Vec<String> = rows
        .into_iter()
        .map(|((kind, hash), row)| {
            serde_json::json!({
                "kind": kind.name(),
                "hash": format!("{hash:016x}"),
                "runs": row.runs,
                "routes": row.routes,
                "passes": row.passes,
                "reason": row.reason,
                "why_quickjs": row.why_quickjs,
                "ec_paths": row.ec_paths,
                "reached": row.reached,
                "fallbacks": row.fallbacks,
                "text": row.text,
            })
            .to_string()
        })
        .collect();
    if census.dropped > 0 {
        lines.push(serde_json::json!({"dropped_runs": census.dropped}).to_string());
    }
    lines
}

/// With `MOQUI_SCRIPT_CENSUS_FILE` set, the census appended to that file as JSON lines, each with the process id (a
/// worker of a run writes its own). Nothing when the census is off or empty.
pub fn write_census() {
    let path = census_file();
    if path.is_empty() || !enabled() {
        return;
    }
    let pid = std::process::id();
    let mut text = String::new();
    for line in census_lines() {
        // each line is an object: the pid goes first
        text.push_str(&format!("{{\"pid\":{pid},{}\n", line.trim_start_matches('{')));
    }
    if text.is_empty() {
        return;
    }
    use std::io::Write as _;
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(err) = file.write_all(text.as_bytes()) {
                tracing::warn!(error = %err, path, "could not write the script census");
            }
        }
        Err(err) => tracing::warn!(error = %err, path, "could not open the script census"),
    }
}
