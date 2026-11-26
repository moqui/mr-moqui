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

//! A trace of the XML actions a Try runs (plans/design-dev-aide.md 9.2, 9.4): while [`traced`] holds the thread, each
//! actions block runs a script generated with trace calls (`TemplateSupport.js`'s `options.trace`), not the one kept
//! for it, and those calls (`__moquiTrace`, a global of the host's) count each statement element as it starts, by its
//! path under the `actions` element, read the names it set as it ends, and leave the element that failed marked. The
//! blocks are told apart by their source position (the file, line and column of the `actions` element), so the logic
//! composer can mark its blocks as run, not run or failed. When nothing traces, nothing of this runs and the generated
//! scripts are the ones they always were.
//!
//! The values are bounded (each at most [`MAX_VALUE`] characters, a map at its first keys, a list at its first items,
//! a few levels deep) and masked as the logs and a sim's rows mask them: a name that names a secret shows `(hidden)`,
//! so does such a key of a map and an entity value's encrypted field, and credential assignments in text are redacted
//! (`telemetry::redact_text`). A trace shows what a service builds inside, its finds with authorization off too, so the
//! Dev AIDE offers it at the extend tier only.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

/// The longest text a value set is shown as.
const MAX_VALUE: usize = 200;
/// The most elements traced in one run, over all its blocks; past it the elements are not counted.
const MAX_ELEMENTS: usize = 4000;
/// The most blocks traced in one run.
const MAX_BLOCKS: usize = 200;
/// The most names an element's values keep.
const MAX_NAMES: usize = 8;
/// The most keys of a map, items of a list and levels of either a value shows.
const MAX_KEYS: usize = 30;
const MAX_ITEMS: usize = 10;
const MAX_DEPTH: usize = 3;

/// An actions block's source position: its file, line and column, or for one with none the hash of its text.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BlockKey {
    file: Option<String>,
    line: u32,
    column: u32,
    text: u64,
}

/// What a run of one element came to.
#[derive(Debug, Default)]
struct ElementTrace {
    count: u64,
    /// Each name it set, as its last run left it.
    values: BTreeMap<String, String>,
    /// Why it failed: what it threw, or the error it added.
    failed: Option<String>,
}

#[derive(Debug)]
struct BlockTrace {
    key: BlockKey,
    runs: u64,
    elements: BTreeMap<String, ElementTrace>,
    /// How many of its elements were marked failed, so an element marks itself only when none inside it did.
    marks: u64,
}

/// An element that started and has not ended.
struct Open {
    block: usize,
    path: String,
    /// The errors in the message facade when it started.
    errors: usize,
    /// Its block's marks when it started.
    marks: u64,
}

#[derive(Default)]
struct State {
    blocks: Vec<BlockTrace>,
    index: HashMap<BlockKey, usize>,
    /// The blocks being run, innermost last; `None` for one past [`MAX_BLOCKS`].
    running: Vec<Option<usize>>,
    open: Vec<Open>,
    elements: usize,
    /// The first element marked failed: the innermost, as it is marked first.
    first: Option<(usize, String)>,
}

impl State {
    /// Mark the element at `path` of `block` failed with `why`, unless it is already.
    fn mark(&mut self, block: usize, path: &str, why: String) {
        let trace = &mut self.blocks[block];
        let Some(element) = trace.elements.get_mut(path) else {
            return;
        };
        if element.failed.is_none() {
            element.failed = Some(why);
            trace.marks += 1;
            self.first.get_or_insert_with(|| (block, path.to_string()));
        }
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Whether a trace holds this thread: actions run their traced scripts.
#[must_use]
pub(crate) fn active() -> bool {
    STATE.with(|state| state.borrow().is_some())
}

/// Run `body` with a trace of the actions it runs on this thread, and answer the trace ([`answer`]'s shape). Inside
/// another trace, the outer one keeps the blocks and this answers `null`.
pub fn traced<R>(body: impl FnOnce() -> R) -> (R, Json) {
    if active() {
        return (body(), Json::Null);
    }
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            STATE.with(|state| state.borrow_mut().take());
        }
    }
    STATE.with(|state| *state.borrow_mut() = Some(State::default()));
    let clear = Clear;
    let out = body();
    let trace = STATE
        .with(|state| state.borrow().as_ref().map(answer))
        .unwrap_or(Json::Null);
    drop(clear);
    (out, trace)
}

/// The trace as JSON: `{blocks: [{file, line, column, runs, elements: {path: {count, values, failed}}}], failed}`,
/// `failed` the first element marked (the innermost, as it is marked first): its block's position, its `element` path
/// and why.
fn answer(state: &State) -> Json {
    let blocks: Vec<Json> = state
        .blocks
        .iter()
        .map(|block| {
            let elements: serde_json::Map<String, Json> = block
                .elements
                .iter()
                .map(|(path, element)| {
                    let mut out = json!({ "count": element.count, "values": element.values });
                    if let Some(why) = &element.failed {
                        out["failed"] = json!(why);
                    }
                    (path.clone(), out)
                })
                .collect();
            json!({ "file": block.key.file, "line": block.key.line, "column": block.key.column, "runs": block.runs,
                "elements": elements })
        })
        .collect();
    let failed = state.first.as_ref().map(|(at, path)| {
        let block = &state.blocks[*at];
        let why = block.elements.get(path).and_then(|element| element.failed.clone());
        json!({ "file": block.key.file, "line": block.key.line, "column": block.key.column, "element": path,
            "message": why })
    });
    json!({ "blocks": blocks, "failed": failed })
}

fn error_count() -> usize {
    crate::context::ExecutionContext::with_current(|ec| ec.message().errors().len()).unwrap_or(0)
}

fn new_errors(since: usize) -> Option<String> {
    crate::context::ExecutionContext::with_current(|ec| {
        let message = ec.message();
        let errors = message.errors();
        (errors.len() > since).then(|| bounded(&errors[since..].join("; ")))
    })
    .flatten()
}

/// `text` cut at [`MAX_VALUE`] characters.
fn bounded(text: &str) -> String {
    match text.char_indices().nth(MAX_VALUE) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// The fields of `entity` whose values a trace never shows: those the entity encrypts, as a sim's rows hide them
/// (`approval::preview`), and those whose names name a secret.
fn hidden_fields(entity: &str) -> Vec<String> {
    let facade = crate::context::ExecutionContext::with_current(|ec| ec.factory().entity()).flatten();
    facade
        .and_then(|facade| {
            facade
                .with_entity(entity, |definition, _, _| {
                    Ok(definition
                        .fields
                        .iter()
                        .filter(|field| field.encrypt)
                        .map(|field| field.name.clone())
                        .collect())
                })
                .ok()
        })
        .unwrap_or_default()
}

/// A value as a trace shows it, before it becomes text: a map at its first [`MAX_KEYS`] keys and a list (an entity
/// list too) at its first [`MAX_ITEMS`] items, each with how many more, [`MAX_DEPTH`] levels down; a key that names a
/// secret, and an entity value's encrypted field, `(hidden)`. Only what is shown is converted.
fn plain(value: &Value, depth: usize) -> Json {
    let more = |count: usize| json!(format!("… {count} more"));
    if depth >= MAX_DEPTH && matches!(value, Value::Map(_) | Value::List(_) | Value::Object(_)) {
        return json!("…");
    }
    let field = |key: &str, value: &Value, hidden: &[String]| {
        if crate::context::secret_parameter(key) || hidden.iter().any(|name| name == key) {
            json!("(hidden)")
        } else {
            plain(value, depth + 1)
        }
    };
    match value {
        Value::Map(map) => {
            let mut out: serde_json::Map<String, Json> = map
                .iter()
                .take(MAX_KEYS)
                .map(|(key, value)| (key.clone(), field(key, value, &[])))
                .collect();
            if map.len() > MAX_KEYS {
                out.insert("…".into(), more(map.len() - MAX_KEYS));
            }
            Json::Object(out)
        }
        Value::List(items) => {
            let mut out: Vec<Json> = items
                .iter()
                .take(MAX_ITEMS)
                .map(|item| plain(item, depth + 1))
                .collect();
            if items.len() > MAX_ITEMS {
                out.push(more(items.len() - MAX_ITEMS));
            }
            Json::Array(out)
        }
        Value::Bytes(bytes) => json!(format!("({} bytes)", bytes.len())),
        Value::Object(_) => match super::runtime::host_parts(value, MAX_ITEMS) {
            Some(super::runtime::HostParts::Entity(entity, fields)) => {
                let hidden = hidden_fields(&entity);
                let mut out: serde_json::Map<String, Json> = fields
                    .iter()
                    .filter(|(_, value)| !value.is_null())
                    .take(MAX_KEYS)
                    .map(|(key, value)| (key.clone(), field(key, value, &hidden)))
                    .collect();
                out.insert("_entity".into(), json!(entity));
                Json::Object(out)
            }
            Some(super::runtime::HostParts::List(len, rows)) => {
                let mut out: Vec<Json> = rows.iter().map(|row| plain(row, depth + 1)).collect();
                if len > rows.len() {
                    out.push(more(len - rows.len()));
                }
                Json::Array(out)
            }
            None => json!("(object)"),
        },
        other => moqui_lib::json::to_json(other),
    }
}

/// How the value an element set under `name` is shown: bounded and masked (the module's doc).
fn shown(name: &str, value: &Value) -> String {
    if crate::context::secret_parameter(name) {
        return "(hidden)".to_string();
    }
    let text = match plain(value, 0) {
        Json::String(text) => text,
        other => other.to_string(),
    };
    bounded(&crate::telemetry::redact_text(&text))
}

/// [`shown`] of what a trace call read of `name`: a field of a host object comes as `{__traceEntity, __traceKey}`
/// (`TemplateSupport.js`), shown `(hidden)` when its entity encrypts it.
fn shown_set(name: &str, value: &Value) -> String {
    if let Value::Map(read) = value {
        if let (Some(object), Some(key)) = (read.get("__traceEntity"), read.get("__traceKey")) {
            let key = moqui_lib::object::to_plain_string(key);
            let hidden = match super::runtime::host_parts(object, 0) {
                Some(super::runtime::HostParts::Entity(entity, _)) => hidden_fields(&entity).contains(&key),
                _ => false,
            };
            if hidden || crate::context::secret_parameter(&key) {
                return "(hidden)".to_string();
            }
            let field = super::runtime::value_field(object, &key).unwrap_or(Value::Null);
            return shown(name, &field);
        }
    }
    shown(name, value)
}

/// One running block, entered for a traced run of an actions text and left by [`Self::finish`], or on a panic by its
/// drop.
pub(crate) struct Block {
    entered: bool,
}

impl Block {
    /// Enter the block of `source` (or of `text`, when it has no source) for a traced run.
    pub(crate) fn enter(source: Option<&moqui_lib::error::SourceLocation>, text: &str) -> Self {
        let key = match source {
            Some(source) => BlockKey {
                file: source.file.clone(),
                line: source.line,
                column: source.column,
                text: 0,
            },
            None => BlockKey {
                file: None,
                line: 0,
                column: 0,
                text: crate::script::text::hash_text(text),
            },
        };
        let entered = STATE.with(|state| {
            let mut state = state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return false;
            };
            let at = match state.index.get(&key) {
                Some(at) => Some(*at),
                None if state.blocks.len() < MAX_BLOCKS => {
                    state.blocks.push(BlockTrace {
                        key: key.clone(),
                        runs: 0,
                        elements: BTreeMap::new(),
                        marks: 0,
                    });
                    let at = state.blocks.len() - 1;
                    state.index.insert(key, at);
                    Some(at)
                }
                None => None,
            };
            if let Some(at) = at {
                state.blocks[at].runs += 1;
            }
            state.running.push(at);
            true
        });
        Self { entered }
    }

    /// Leave the block after its run: on an error the innermost of its elements still open is marked failed with it,
    /// and on success one that added an error and ended the block (a `return` with an error) is.
    pub(crate) fn finish(mut self, result: &Result<(), crate::context::Error>) {
        let failure = match result {
            Err(err) => Some(bounded(err.message().lines().next().unwrap_or_default())),
            Ok(()) => None,
        };
        self.leave(failure, true);
    }

    fn leave(&mut self, failure: Option<String>, finished: bool) {
        if !self.entered {
            return;
        }
        self.entered = false;
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            let Some(state) = state.as_mut() else {
                return;
            };
            let Some(at) = state.running.pop() else {
                return;
            };
            let Some(at) = at else {
                return;
            };
            // its elements still open: those since the innermost open element of another block
            let mut keep = state.open.len();
            while keep > 0 && state.open[keep - 1].block == at {
                keep -= 1;
            }
            let innermost = state.open.drain(keep..).next_back();
            if !finished {
                return;
            }
            if let Some(open) = innermost {
                let why = failure.or_else(|| new_errors(open.errors));
                if let (Some(why), true) = (why, state.blocks[at].marks == open.marks) {
                    state.mark(at, &open.path, why);
                }
            }
        });
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        self.leave(None, false);
    }
}

/// How many elements are open on this thread's trace, for tests.
#[cfg(test)]
pub(crate) fn open_depth() -> usize {
    STATE.with(|state| state.borrow().as_ref().map_or(0, |state| state.open.len()))
}

/// Whether the element at `path` is inside the one at `outer` (`1.0.2` is inside `1` and `1.0`).
fn is_inside(path: &str, outer: &str) -> bool {
    path.strip_prefix(outer).is_some_and(|rest| rest.starts_with('.'))
}

/// A trace call of a traced script (`__moquiTrace(op, path, names, values)`): `op` 0 when the element at `path` starts,
/// 1 when it ends, with the names it set and their values. Nothing when no trace holds the thread.
pub(crate) fn record(op: i32, path: &str, names: Option<&Value>, values: Option<&Value>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return;
        };
        let Some(Some(block)) = state.running.last().copied() else {
            return;
        };
        if op == 0 {
            // an element starts after its earlier siblings ended, inside its open ancestors: what of its block is open
            // and is not one of them was left by a break, a continue or a return, so the stack stays as deep as the
            // actions however many times a loop goes round
            while state
                .open
                .last()
                .is_some_and(|open| open.block == block && !is_inside(path, &open.path))
            {
                state.open.pop();
            }
            let known = state.blocks[block].elements.contains_key(path);
            if !known && state.elements >= MAX_ELEMENTS {
                return;
            }
            if !known {
                state.elements += 1;
            }
            let marks = state.blocks[block].marks;
            state.blocks[block].elements.entry(path.to_string()).or_default().count += 1;
            state.open.push(Open {
                block,
                path: path.to_string(),
                errors: error_count(),
                marks,
            });
            return;
        }
        // its start, and those started inside it and left by a break, a continue or a return
        let Some(at) = state
            .open
            .iter()
            .rposition(|open| open.block == block && open.path == path)
        else {
            return;
        };
        let Some(open) = state.open.drain(at..).next() else {
            return;
        };
        let added = new_errors(open.errors);
        let unmarked = state.blocks[block].marks == open.marks;
        if let (Some(Value::List(names)), Some(Value::List(values)), Some(element)) =
            (names, values, state.blocks[block].elements.get_mut(path))
        {
            for (name, value) in names.iter().zip(values.iter()).take(MAX_NAMES) {
                let name = moqui_lib::object::to_plain_string(name);
                element.values.insert(name.clone(), shown_set(&name, value));
            }
        }
        if let (Some(why), true) = (added, unmarked) {
            state.mark(block, path, why);
        }
    });
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
