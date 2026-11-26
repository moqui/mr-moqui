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
//! The values are bounded (each at most [`MAX_VALUE`] characters, a list at its first items) and masked as the logs
//! mask them: a name that names a secret shows `(hidden)`, so does such a key of a map, and credential assignments in
//! text are redacted (`telemetry::redact_text`).

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

/// A JSON value with each key that names a secret hidden, at any depth.
fn masked(value: Json) -> Json {
    match value {
        Json::Object(map) => Json::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if crate::context::secret_parameter(&key) {
                        (key, json!("(hidden)"))
                    } else {
                        (key, masked(value))
                    }
                })
                .collect(),
        ),
        Json::Array(items) => Json::Array(items.into_iter().map(masked).collect()),
        other => other,
    }
}

/// How the value an element set under `name` is shown: bounded and masked (the module's doc).
fn shown(name: &str, value: &Value) -> String {
    if crate::context::secret_parameter(name) {
        return "(hidden)".to_string();
    }
    let text = match masked(moqui_lib::json::to_json(value)) {
        Json::String(text) => text,
        other => other.to_string(),
    };
    bounded(&crate::telemetry::redact_text(&text))
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
                element.values.insert(name.clone(), shown(&name, value));
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
