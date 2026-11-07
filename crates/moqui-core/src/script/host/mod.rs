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

//! The engine-neutral host table (plans/Script-Engine-Oxide.md section 5): the `ec` facades a script calls, written once
//! over host values, and bound to each engine by its binder (`script/runtime/bind.rs` for QuickJS, `script/oxide/bind.rs`
//! for quickjs-oxide). Nothing here names an engine type.
//!
//! A [`HostClass`] is a prototype of [`Method`]s and getters over a [`HostObject`] (the receiver's state, `Mutex` where
//! it changes). A method's arguments are converted by the binder as its [`ArgKind`]s say, with the same refusals on
//! each engine; its body returns a [`Ret`] or a [`HostError`], which the binder throws as the facades always have
//! (`Exception::throw_message`, `throw_lib`, `throw_context`). The [`registry`] lists the `ec` paths the table installs;
//! [`path_allowed`] is what the static check of small scripts allows.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use moqui_lib::object::{HostObject, Value};

mod cache;
mod l10n;
mod logger;
mod message;
mod user;

pub(crate) use l10n::rounding_strategy_named;

/// How the binder converts one argument. A required kind counts toward the function's `length` and a call with fewer
/// arguments throws `Error calling function with N argument(s) while M where expected`, as QuickJS's typed functions
/// do; an optional kind (`Opt*`) is [`Arg::Missing`] when not passed. Required kinds come before optional ones.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArgKind {
    /// Any value as a host value; `undefined` is null.
    Value,
    /// [`ArgKind::Value`], optional: `undefined` is [`Arg::Undefined`], so a body can tell it from null.
    OptValue,
    /// A string (another type throws a `TypeError`).
    Text,
    /// [`ArgKind::Text`], optional.
    OptText,
    /// A string, or null or undefined ([`Arg::Undefined`]).
    NullableText,
    /// [`ArgKind::NullableText`], optional.
    OptNullableText,
    /// A boolean (another type throws).
    Bool,
    /// [`ArgKind::Bool`], optional.
    OptBool,
    /// A number as an `i32`, a fraction truncated.
    Int,
    /// [`ArgKind::Int`], optional.
    OptInt,
    /// [`ArgKind::OptInt`] as an `f64`.
    OptNumber,
    /// A function, called back through [`EngineOps::call_back`].
    Callback,
    /// An optional additional context: an object's entries (a map or an entity value's fields), else none.
    Context,
    /// The optional exception of a log call, as text: `name: message` of an error, its `message` alone, or a string.
    Thrown,
}

impl ArgKind {
    pub(crate) const fn required(self) -> bool {
        matches!(
            self,
            Self::Value | Self::Text | Self::NullableText | Self::Bool | Self::Int | Self::Callback
        )
    }

    /// The type name a QuickJS conversion error gives for this kind (`Error converting from js 'int' into type '...'`).
    pub(crate) const fn type_name(self) -> &'static str {
        match self {
            Self::Text | Self::OptText | Self::NullableText | Self::OptNullableText => "string",
            Self::Bool | Self::OptBool => "bool",
            Self::Int | Self::OptInt => "i32",
            Self::OptNumber => "f64",
            Self::Callback => "function",
            Self::Value | Self::OptValue | Self::Context | Self::Thrown => "value",
        }
    }
}

/// What the binder flushes before the body runs: the engine's tracked values, written back to the context stack.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlushBefore {
    /// The body does not read the context stack.
    None,
    /// It reads the stack: write the tracked values back and keep tracking them.
    Keep,
    /// It may change the stack: write them back and release them.
    Release,
}

/// One converted argument.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum Arg {
    /// Not passed (an optional kind only).
    Missing,
    /// `undefined` passed to [`ArgKind::OptValue`], or null or undefined to a nullable text.
    Undefined,
    Value(Value),
    Text(String),
    Bool(bool),
    Int(i32),
    Number(f64),
    /// The index of a function for [`EngineOps::call_back`].
    Callback(usize),
    /// The entries of an additional-context object.
    Context(Option<indexmap::IndexMap<String, Value>>),
}

/// A method or a getter of a [`HostClass`]. Made with [`method!`] so its census path is a literal.
pub(crate) struct Method {
    /// The JavaScript name.
    pub(crate) name: &'static str,
    /// The census row for a call, `ec.l10n.localize`, or `<Class>.<name>` for a class reached from a call's result.
    pub(crate) path: &'static str,
    pub(crate) args: &'static [ArgKind],
    pub(crate) flush: FlushBefore,
    pub(crate) f: fn(&mut Call<'_>) -> Result<Ret, HostError>,
}

impl Method {
    /// How many arguments a call needs: the function's `length`.
    pub(crate) fn required(&self) -> usize {
        self.args.iter().filter(|kind| kind.required()).count()
    }
}

/// A prototype the binders make once per engine.
pub(crate) struct HostClass {
    /// The name the engine caches the prototype under; unique among classes.
    pub(crate) name: &'static str,
    pub(crate) methods: &'static [Method],
    pub(crate) getters: &'static [Method],
    /// Functions given to [`HostClass::shim_js`] and not put on the prototype.
    pub(crate) hidden: &'static [Method],
    /// `(function (proto, hidden) { ... })`, run once when the prototype is made, for what is written in JavaScript
    /// (a method over a `RegExp`, a `Proxy`). `hidden` holds the [`HostClass::hidden`] functions by name.
    pub(crate) shim_js: Option<&'static str>,
    /// The members the shim adds to the prototype, which the static check allows as it allows the methods.
    pub(crate) shim_members: &'static [&'static str],
}

/// What a body returns.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
pub(crate) enum Ret {
    Value(Value),
    /// The receiver, for a builder's chained calls.
    This,
    Undefined,
    /// A new object of `class` over the state.
    Object(HostObject, &'static HostClass),
}

impl From<Value> for Ret {
    fn from(value: Value) -> Self {
        Ret::Value(value)
    }
}

impl From<bool> for Ret {
    fn from(flag: bool) -> Self {
        Ret::Value(Value::Bool(flag))
    }
}

impl From<String> for Ret {
    fn from(text: String) -> Self {
        Ret::Value(Value::String(text))
    }
}

impl From<Option<String>> for Ret {
    fn from(text: Option<String>) -> Self {
        Ret::Value(text.map_or(Value::Null, Value::String))
    }
}

/// A body's failure, thrown as the facades throw it on QuickJS.
#[derive(Debug)]
pub(crate) enum HostError {
    /// A plain `Error` with this message (`Exception::throw_message`).
    Message(String),
    /// A library error, with its diagnostic on the stack and its kind and retry seconds marked (`throw_lib`).
    Lib(moqui_lib::error::Error),
    /// A framework error, marked as [`HostError::Lib`] (`throw_context`).
    Context(crate::context::Error),
    /// A `TypeError` with this message.
    Type(String),
    /// The engine holds the exception already (a callback threw, a flush failed): it is thrown on.
    Thrown,
}

impl HostError {
    pub(crate) fn message(text: impl Into<String>) -> Self {
        HostError::Message(text.into())
    }
}

/// The marks a thrown error carries (`moquiErrorKind`, `moquiRetryAfter`), as runtime.rs's `mark_kind` puts them; the
/// oxide binder writes the same properties so the engine's error mapping reads them back.
pub(crate) const KIND_PROPERTY: &str = "moquiErrorKind";
pub(crate) const RETRY_AFTER_PROPERTY: &str = "moquiRetryAfter";

/// The name of a kind a thrown error carries, `None` for one it does not.
pub(crate) fn carried_kind_name(kind: crate::context::ErrorKind) -> Option<&'static str> {
    use crate::context::ErrorKind;
    Some(match kind {
        ErrorKind::AuthenticationRequired => "AuthenticationRequired",
        ErrorKind::Unauthorized => "Unauthorized",
        ErrorKind::Tarpit => "Tarpit",
        ErrorKind::NotFound => "NotFound",
        ErrorKind::Conflict => "Conflict",
        ErrorKind::Unprocessable => "Unprocessable",
        ErrorKind::Unavailable => "Unavailable",
        ErrorKind::MethodNotAllowed => "MethodNotAllowed",
        _ => return None,
    })
}

/// A [`HostError::Lib`] or [`HostError::Context`] as the parts a binder throws: message, diagnostic, kind, retry.
pub(crate) struct Thrown<'a> {
    pub(crate) message: &'a str,
    pub(crate) diagnostic: String,
    pub(crate) kind: Option<&'static str>,
    pub(crate) retry_after: Option<u64>,
}

impl HostError {
    /// The parts of a marked error; `None` for the others.
    pub(crate) fn marked(&self) -> Option<Thrown<'_>> {
        match self {
            HostError::Lib(err) => Some(Thrown {
                message: err.message(),
                diagnostic: err.diagnostic(),
                kind: carried_kind_name(crate::context::ErrorKind::of(err)),
                retry_after: crate::context::Error::retry_after_of(err),
            }),
            HostError::Context(err) => Some(Thrown {
                message: err.message(),
                diagnostic: err.diagnostic(),
                kind: carried_kind_name(err.kind()),
                retry_after: err.retry_after(),
            }),
            _ => None,
        }
    }
}

/// The message of a panic a binder caught, as `host_guard` gives it.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "host callback panicked".to_string()
}

/// What a body may ask of the engine it runs in.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
pub(crate) trait EngineOps {
    /// Call the [`Arg::Callback`] at `index` with `args`; its result as a host value. A throw is [`HostError::Thrown`].
    fn call_back(&mut self, index: usize, args: Vec<Value>) -> Result<Value, HostError>;
    /// Write the engine's tracked values back to the context stack.
    fn flush(&mut self, mode: FlushBefore) -> Result<(), HostError>;
    /// Run the engine's pending jobs until a promise settles (the spec engine's `settle_promise`, Phase 3).
    fn settle(&mut self) -> Result<(), HostError> {
        Err(HostError::message("settling a promise is not supported here"))
    }
}

/// How many converted arguments a call holds without allocating.
const INLINE_ARGS: usize = 4;

/// The converted arguments of a call: the first few inline, the rest (rare) in a vector.
pub(crate) struct Args {
    inline: [Arg; INLINE_ARGS],
    len: usize,
    spill: Vec<Arg>,
}

impl Args {
    pub(crate) fn new() -> Self {
        Args {
            inline: [const { Arg::Missing }; INLINE_ARGS],
            len: 0,
            spill: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, arg: Arg) {
        match self.inline.get_mut(self.len) {
            Some(slot) => *slot = arg,
            None => self.spill.push(arg),
        }
        self.len += 1;
    }

    pub(crate) fn get(&self, index: usize) -> Option<&Arg> {
        if index >= self.len {
            return None;
        }
        match self.inline.get(index) {
            Some(arg) => Some(arg),
            None => self.spill.get(index - INLINE_ARGS),
        }
    }
}

/// Lock a host object's state. A host object's state is shared with the script, which may call back into the same
/// object while a body holds the lock (a callback that calls the builder it is building): that is refused with an
/// error rather than a deadlock, and so is state an earlier panic left half-changed. Hold the guard only for the
/// state's own work, never across [`EngineOps::call_back`].
#[allow(dead_code)] // the builders B2 moves
pub(crate) fn lock_state<'a, T>(state: &'a Mutex<T>, class: &str) -> Result<MutexGuard<'a, T>, HostError> {
    match state.try_lock() {
        Ok(guard) => Ok(guard),
        Err(std::sync::TryLockError::WouldBlock) => Err(HostError::message(format!("re-entrant call on {class}"))),
        Err(std::sync::TryLockError::Poisoned(_)) => Err(HostError::message(format!(
            "the state of this {class} was left unfinished by an earlier failure"
        ))),
    }
}

/// One call of a [`Method`].
pub(crate) struct Call<'a> {
    /// The receiver's state, when the receiver is an object of a host class.
    pub(crate) this: Option<&'a HostObject>,
    pub(crate) args: Args,
    #[allow(dead_code)] // no migrated facade calls back yet
    pub(crate) ops: &'a mut dyn EngineOps,
}

impl Call<'_> {
    pub(crate) fn arg(&self, index: usize) -> &Arg {
        self.args.get(index).unwrap_or(&Arg::Missing)
    }

    /// Whether the argument was passed at all.
    pub(crate) fn passed(&self, index: usize) -> bool {
        !matches!(self.arg(index), Arg::Missing)
    }

    /// The host value of a value argument; `None` when missing or undefined.
    pub(crate) fn value(&self, index: usize) -> Option<&Value> {
        match self.arg(index) {
            Arg::Value(value) => Some(value),
            _ => None,
        }
    }

    /// [`Call::value`], null when missing or undefined.
    pub(crate) fn value_or_null(&self, index: usize) -> Value {
        self.value(index).cloned().unwrap_or(Value::Null)
    }

    /// The text of a text argument; `None` when missing, null or undefined.
    pub(crate) fn text(&self, index: usize) -> Option<&str> {
        match self.arg(index) {
            Arg::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The plain text of a value argument, `None` when missing, null or undefined (runtime/convert.rs `optional_text`).
    pub(crate) fn opt_text(&self, index: usize) -> Option<String> {
        match self.arg(index) {
            Arg::Value(Value::Null) => None,
            Arg::Value(value) => Some(moqui_lib::object::to_plain_string(value)),
            Arg::Text(text) => Some(text.clone()),
            _ => None,
        }
    }

    /// [`Call::opt_text`], empty when there is none (`text_or_empty`).
    pub(crate) fn text_or_empty(&self, index: usize) -> String {
        self.opt_text(index).unwrap_or_default()
    }

    #[allow(dead_code)] // no migrated facade takes a boolean yet
    pub(crate) fn bool(&self, index: usize) -> Option<bool> {
        match self.arg(index) {
            Arg::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub(crate) fn int(&self, index: usize) -> Option<i32> {
        match self.arg(index) {
            Arg::Int(number) => Some(*number),
            _ => None,
        }
    }

    pub(crate) fn number(&self, index: usize) -> Option<f64> {
        match self.arg(index) {
            Arg::Number(number) => Some(*number),
            _ => None,
        }
    }

    /// The receiver's state as a `T`; a `TypeError` when the receiver is not one.
    pub(crate) fn this<T: std::any::Any + Send + Sync>(&self) -> Result<Arc<T>, HostError> {
        self.this
            .and_then(HostObject::downcast::<T>)
            .ok_or_else(|| HostError::Type("the receiver is not the host object this method is for".to_string()))
    }
}

/// The JavaScript truthiness of a converted value (runtime/convert.rs `js_truthy`).
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::I32(number) => *number != 0,
        Value::I64(number) => *number != 0,
        Value::F64(number) => *number != 0.0 && !number.is_nan(),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// `body` on the entered execution context; the error the facades throw when none is entered.
pub(crate) fn with_ec<T>(body: impl FnOnce(&crate::context::ExecutionContext) -> T) -> Result<T, HostError> {
    crate::context::ExecutionContext::with_current(body)
        .ok_or_else(|| HostError::message("no execution context is entered"))
}

/// A [`Method`] whose census path is `$path.$name`.
macro_rules! method {
    ($path:literal, $name:literal, [$($kind:ident),* $(,)?], $f:expr) => {
        $crate::script::host::method!($path, $name, [$($kind),*], None, $f)
    };
    ($path:literal, $name:literal, [$($kind:ident),* $(,)?], $flush:ident, $f:expr) => {
        $crate::script::host::Method {
            name: $name,
            path: concat!($path, ".", $name),
            args: &[$($crate::script::host::ArgKind::$kind),*],
            flush: $crate::script::host::FlushBefore::$flush,
            f: $f,
        }
    };
}
pub(crate) use method;

/// An `ec` path the table installs and the object of `class` it holds.
pub(crate) struct Path {
    /// `ec.l10n`
    pub(crate) path: &'static str,
    pub(crate) class: &'static HostClass,
}

/// The paths of this crate. In ec member order.
static PATHS: &[Path] = &[
    Path {
        path: "ec.message",
        class: &message::MESSAGE,
    },
    Path {
        path: "ec.logger",
        class: &logger::LOGGER,
    },
    Path {
        path: "ec.user",
        class: &user::USER,
    },
    Path {
        path: "ec.cache",
        class: &cache::CACHE_FACADE,
    },
    Path {
        path: "ec.l10n",
        class: &l10n::L10N,
    },
];

/// The classes a method may return ([`Ret::Object`]), which a static check cannot see but must be complete on both
/// engines: every class here is, by being in the table.
static RETURNED: &[&HostClass] = &[&cache::CACHE];

/// Paths other crates contribute (moqui-llm's `ec.llm`), through [`register`], before the table is frozen.
static EXTRA: Mutex<Vec<&'static [Path]>> = Mutex::new(Vec::new());

/// The table as the binders install it and the static check reads it, frozen at its first use.
struct Table {
    paths: Vec<&'static Path>,
    /// Each path, and each member of its class (methods, getters and what the shim adds).
    exact: HashSet<String>,
    /// The members a shim adds: a JavaScript object on both engines, so anything below one is allowed too.
    open: HashSet<String>,
}

static TABLE: OnceLock<Table> = OnceLock::new();

fn table() -> &'static Table {
    TABLE.get_or_init(|| {
        let mut paths: Vec<&'static Path> = PATHS.iter().collect();
        for extra in moqui_lib::sync::lock(&EXTRA).iter() {
            paths.extend(extra.iter());
        }
        let mut exact = HashSet::new();
        let mut open = HashSet::new();
        for entry in &paths {
            exact.insert(entry.path.to_string());
            let class = entry.class;
            for method in class.methods.iter().chain(class.getters) {
                exact.insert(format!("{}.{}", entry.path, method.name));
            }
            for member in class.shim_members {
                let member = format!("{}.{member}", entry.path);
                exact.insert(member.clone());
                open.insert(member);
            }
        }
        Table { paths, exact, open }
    })
}

/// The `ec` paths of this crate's table.
#[allow(dead_code)] // the census and the static check (script/check.rs) read it at merge
pub(crate) fn registry() -> &'static [Path] {
    PATHS
}

/// Add another crate's paths. Only before the table is frozen (the first engine's start or the first
/// [`path_allowed`]), so a path is either bound on both engines or on neither.
#[allow(dead_code)] // the hook for moqui-llm and moqui-devkit
pub(crate) fn register(paths: &'static [Path]) -> Result<(), String> {
    if TABLE.get().is_some() {
        return Err("the host table is already in use; register its paths before the first script runs".to_string());
    }
    moqui_lib::sync::lock(&EXTRA).push(paths);
    Ok(())
}

/// Every path the binders install: this crate's, then the registered ones.
pub(crate) fn all_paths() -> &'static [&'static Path] {
    &table().paths
}

/// Whether a script may reach `path` (`ec.l10n`, `ec.l10n.localize`) on both engines: a path the binders install, a
/// method or getter of its class, a member its shim adds, or anything below such a member. What a method returns is a
/// class of the table, so it is complete on both engines too. Takes no lock and allocates nothing.
pub(crate) fn path_allowed(path: &str) -> bool {
    let table = table();
    if table.exact.contains(path) {
        return true;
    }
    path.match_indices('.')
        .any(|(index, _)| table.open.contains(&path[..index]))
}

/// Every class of the table, for a binder that makes its prototypes ahead.
#[allow(dead_code)] // the oxide binder makes them on first use
pub(crate) fn classes() -> impl Iterator<Item = &'static HostClass> {
    all_paths()
        .iter()
        .map(|path| path.class)
        .chain(RETURNED.iter().copied())
}

/// Record that a script reached `path`, into the running script's census entry ([`crate::script::census::note`]).
#[inline]
pub(crate) fn note_path(path: &'static str) {
    crate::script::census::note(path);
}

#[cfg(test)]
pub(crate) mod test_class;
#[cfg(test)]
mod tests;
