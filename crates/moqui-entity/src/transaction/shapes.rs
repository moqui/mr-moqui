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

//! Statement shapes for the transaction census (plans/Profiling3.md M2): what a transaction's round trips were.
//!
//! With `MOQUI_TX_CACHE_CENSUS_FILE` set, each statement a pooled connection runs inside a transaction frame is noted
//! against the frame begun last on the thread: its shape (a hash of the SQL text, which holds no literals on the
//! entity paths, and its first words), its kind, whether it waited for a round trip, and how many rows it returned. When
//! the frame ends, its census line gets the shapes and three counts that say what batching or caching could save: the
//! statements of a shape run again (`repeats`), those run again with other parameters (`repeats_other_params`, a find
//! in a loop), and waited reads that came right after another waited read with no other statement between
//! (`read_after_read`).

use std::cell::RefCell;
use std::hash::{BuildHasher, Hash, Hasher};

use indexmap::IndexMap;

use moqui_lib::object::Value;

/// What a statement is, from its text and how it was sent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Read,
    Count,
    Lock,
    Sequence,
    Write,
    WriteSent,
    Stream,
    Begin,
    Commit,
    Rollback,
    Savepoint,
    Other,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Count => "count",
            Self::Lock => "lock",
            Self::Sequence => "sequence",
            Self::Write => "write",
            Self::WriteSent => "write-sent",
            Self::Stream => "stream",
            Self::Begin => "begin",
            Self::Commit => "commit",
            Self::Rollback => "rollback",
            Self::Savepoint => "savepoint",
            Self::Other => "other",
        }
    }

    fn is_read(self) -> bool {
        matches!(self, Self::Read | Self::Count | Self::Lock)
    }

    /// The kind of a statement run and waited for, from its text.
    pub(crate) fn of_sql(sql: &str) -> Self {
        let head = sql.trim_start();
        let lower_head: String = head.chars().take(16).flat_map(char::to_lowercase).collect();
        let lower = || sql.to_ascii_lowercase();
        if lower_head.starts_with("select") || lower_head.starts_with("with") {
            let text = lower();
            if text.contains("sequence_value_item") {
                Self::Sequence
            } else if text.contains(" for update") || text.contains(" for share") || text.contains(" for no key") {
                Self::Lock
            } else if lower_head.starts_with("select count(") || text.contains("select count(") {
                Self::Count
            } else {
                Self::Read
            }
        } else if lower_head.starts_with("insert")
            || lower_head.starts_with("update")
            || lower_head.starts_with("delete")
            || lower_head.starts_with("merge")
        {
            if lower().contains("sequence_value_item") {
                Self::Sequence
            } else {
                Self::Write
            }
        } else {
            Self::Other
        }
    }
}

#[derive(Default)]
struct Shape {
    kind: Option<Kind>,
    head: String,
    count: u64,
    waited: u64,
    rows: u64,
    params: Vec<u64>,
}

#[derive(Default)]
struct Log {
    shapes: IndexMap<u64, Shape, foldhash::fast::FixedState>,
    repeats: u64,
    repeats_other_params: u64,
    read_after_read: u64,
    waited: u64,
    last_waited_read: bool,
}

/// The logs of the frames open on this thread, the one begun last at the end.
#[derive(Default)]
struct Logs {
    open: Vec<(u64, Log)>,
}

thread_local! {
    static LOGS: RefCell<Logs> = RefCell::new(Logs::default());
}

/// At most this many frames open on one thread are logged; a frame that never ended goes first.
const MAX_OPEN: usize = 64;

/// Whether shapes are logged: the census file is set.
pub(crate) fn enabled() -> bool {
    crate::entity::cache::census_measures()
}

/// A frame began: statements go to its log until a later frame begins or it ends.
pub(crate) fn begin(token: u64) {
    if !enabled() {
        return;
    }
    LOGS.with_borrow_mut(|logs| {
        if logs.open.len() >= MAX_OPEN {
            logs.open.remove(0);
        }
        logs.open.push((token, Log::default()));
    });
}

/// A statement ran: `waited` when the thread's waited round trips moved over it.
pub(crate) fn note(sql: &str, params: &[Value], kind: Kind, waited: bool, rows: u64) {
    if !enabled() {
        return;
    }
    LOGS.with_borrow_mut(|logs| {
        let Some((_, log)) = logs.open.last_mut() else { return };
        let hasher = foldhash::fast::FixedState::default();
        let shape_hash = hasher.hash_one(sql);
        let mut params_hasher = hasher.build_hasher();
        for param in params {
            format!("{param:?}").hash(&mut params_hasher);
        }
        let params_hash = params_hasher.finish();
        let shape = log.shapes.entry(shape_hash).or_insert_with(|| Shape {
            kind: Some(kind),
            head: head_of(sql),
            ..Shape::default()
        });
        if shape.count > 0 {
            log.repeats += 1;
            if !params.is_empty() && !shape.params.contains(&params_hash) {
                log.repeats_other_params += 1;
            }
        }
        shape.count += 1;
        shape.rows += rows;
        if shape.params.len() < 32 && !shape.params.contains(&params_hash) {
            shape.params.push(params_hash);
        }
        if waited {
            shape.waited += 1;
            log.waited += 1;
            if kind.is_read() && log.last_waited_read {
                log.read_after_read += 1;
            }
        }
        log.last_waited_read = waited && kind.is_read();
    });
}

/// The first words of a statement and the table it reads or writes: enough to name it, with no literal.
fn head_of(sql: &str) -> String {
    let words: Vec<&str> = sql.split_whitespace().collect();
    let lower: Vec<String> = words.iter().map(|word| word.to_ascii_lowercase()).collect();
    let table = lower
        .iter()
        .position(|word| word == "from" || word == "into" || word == "update")
        .and_then(|at| words.get(at + 1))
        .map(|table| table.trim_matches(|c: char| c == '"' || c == '(' || c == ','));
    let first = words.first().copied().unwrap_or_default().to_ascii_uppercase();
    match table {
        Some(table) => format!("{first} {table}"),
        None => words.iter().take(3).copied().collect::<Vec<_>>().join(" "),
    }
}

/// The frame `token` ended: its shapes as the census line's JSON members (with a leading comma), or empty.
pub(crate) fn end(token: u64) -> String {
    if !enabled() {
        return String::new();
    }
    let log = LOGS.with_borrow_mut(|logs| {
        let at = logs.open.iter().rposition(|(open, _)| *open == token)?;
        Some(logs.open.remove(at).1)
    });
    let Some(log) = log else { return String::new() };
    let mut by_kind: IndexMap<&'static str, (u64, u64)> = IndexMap::new();
    let mut shapes: Vec<serde_json::Value> = Vec::with_capacity(log.shapes.len());
    for (hash, shape) in &log.shapes {
        let kind = shape.kind.unwrap_or(Kind::Other).as_str();
        let entry = by_kind.entry(kind).or_default();
        entry.0 += shape.count;
        entry.1 += shape.waited;
        shapes.push(serde_json::json!({
            "shape": format!("{hash:016x}"),
            "head": shape.head,
            "kind": kind,
            "n": shape.count,
            "waited": shape.waited,
            "rows": shape.rows,
            "params": shape.params.len(),
        }));
    }
    let kinds: serde_json::Map<String, serde_json::Value> = by_kind
        .into_iter()
        .map(|(kind, (n, waited))| (kind.to_string(), serde_json::json!({"n": n, "waited": waited})))
        .collect();
    format!(
        ",\"statements_waited\":{},\"repeats\":{},\"repeats_other_params\":{},\"read_after_read\":{},\"kinds\":{},\"shapes\":{}",
        log.waited,
        log.repeats,
        log.repeats_other_params,
        log.read_after_read,
        serde_json::Value::Object(kinds),
        serde_json::Value::Array(shapes),
    )
}

#[cfg(test)]
#[path = "shapes_tests.rs"]
mod tests;
