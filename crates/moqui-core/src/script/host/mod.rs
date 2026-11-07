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

mod archive;
mod cache;
pub(crate) mod conditions;
mod ec_web;
mod edi;
pub(crate) mod error_info;
pub(crate) mod entities;
mod files;
mod l10n;
mod logger;
mod message;
mod native_tools;
mod notification;
mod pass;
mod screen;
mod search;
mod service;
pub(crate) mod globals;
mod sftp;
mod sri;
mod theme;
mod tools;
pub(crate) mod upload;
mod urls;
mod user;
mod web;

pub(crate) use l10n::rounding_strategy_named;
pub use upload::text_upload;
#[cfg(test)]
pub(crate) use sri::{HOLD_EXPECTED as SRI_HOLD_EXPECTED, HOLD_SCRIPT as SRI_HOLD_SCRIPT};

/// How the binder converts one argument. A required kind counts toward the function's `length` and a call with fewer
/// arguments throws `Error calling function with N argument(s) while M where expected`, as QuickJS's typed functions
/// do; an optional kind (`Opt*`) is [`Arg::Missing`] when not passed. Required kinds come before optional ones.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
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
    /// A number as an `f64` (another type throws).
    Number,
    /// An array of strings ([`Arg::Texts`]); another value, or an item that is not a string, throws a `TypeError`.
    Texts,
    /// [`ArgKind::Texts`], optional; passed, it is converted, so `undefined` throws as QuickJS's `Opt<Vec<String>>`.
    OptTexts,
    /// A function, called back through [`EngineOps::call_back`].
    Callback,
    /// An optional additional context: an object's entries (a map or an entity value's fields), else none.
    Context,
    /// A whole number (QuickJS's `js_i64`): an integer, else `limit is not an integer` for a fraction and `limit is not a
    /// number` for anything else.
    Whole,
    /// [`ArgKind::Whole`], optional; null or undefined is none.
    OptWhole,
    /// A parameters map: `ec.context` itself is [`Arg::WholeContext`] (the whole context, read from the stack), anything
    /// else a value.
    Parameters,
    /// The optional exception of a log call, as text: `name: message` of an error, its `message` alone, or a string.
    Thrown,
    /// An object of a host class ([`Arg::Host`], its state); anything else throws a `TypeError`.
    Host,
    /// A screen unit's element literal ([`Arg::Node`]): the conversion `sri.holdNodeLits` attached to it, else converted
    /// now; a value that is not an object throws `expected an element object`.
    Node,
    /// [`ArgKind::Node`], optional; a value that is not an object is [`Arg::Undefined`].
    OptNode,
}

impl ArgKind {
    pub const fn required(self) -> bool {
        matches!(
            self,
            Self::Value
                | Self::Text
                | Self::NullableText
                | Self::Bool
                | Self::Int
                | Self::Number
                | Self::Texts
                | Self::Callback
                | Self::Whole
                | Self::Parameters
                | Self::Host
                | Self::Node
        )
    }

    /// The type name a QuickJS conversion error gives for this kind (`Error converting from js 'int' into type '...'`).
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Text | Self::OptText | Self::NullableText | Self::OptNullableText => "string",
            Self::Bool | Self::OptBool => "bool",
            Self::Int | Self::OptInt => "i32",
            Self::OptNumber | Self::Number => "f64",
            Self::Callback => "function",
            Self::Whole | Self::OptWhole => "i64",
            Self::Value
            | Self::OptValue
            | Self::Context
            | Self::Thrown
            | Self::Parameters
            | Self::Node
            | Self::OptNode => "value",
            Self::Host => "object",
            Self::Texts | Self::OptTexts => "array",
        }
    }
}

/// What the binder flushes before the body runs: the engine's tracked values, written back to the context stack.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushBefore {
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
pub enum Arg {
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
    /// `ec.context` passed as a parameters map ([`ArgKind::Parameters`]).
    WholeContext,
    /// The entries of an additional-context object.
    Context(Option<indexmap::IndexMap<String, Value>>),
    /// The state of a host object ([`ArgKind::Host`]).
    Host(HostObject),
    /// An element literal ([`ArgKind::Node`], [`ArgKind::OptNode`]).
    Node(Arc<crate::screen::sri::NodeLit>),
    /// The strings of an array ([`ArgKind::Texts`]).
    Texts(Vec<String>),
}

/// A method or a getter of a [`HostClass`]. Made with [`method!`] so its census path is a literal.
pub struct Method {
    /// The JavaScript name.
    pub name: &'static str,
    /// The census row for a call, `ec.l10n.localize`, or `<Class>.<name>` for a class reached from a call's result.
    pub path: &'static str,
    pub args: &'static [ArgKind],
    pub flush: FlushBefore,
    /// Whether the body provably never runs a nested script. By default a body may (a service call, an entity find with
    /// its authorization filters, a localized lookup, a screen render): the oxide binder lends the engine to it, so the
    /// nested script runs on the same runtime, and the body's [`EngineOps`] are what the binder did before the lend (the
    /// flush, an [`Arg::WholeContext`] read) and nothing else. A leaf body (`host_method!(.., [kinds], Flush, leaf, f)`)
    /// runs without the lend and has the engine's operations: one that calls back, evaluates or flushes must be a leaf.
    /// It must not reach entities, services, screens, scripts or the user and l10n lookups that read entities.
    pub leaf: bool,
    pub f: fn(&mut Call<'_>) -> Result<Ret, HostError>,
}

impl Method {
    /// How many arguments a call needs: the function's `length`.
    pub fn required(&self) -> usize {
        self.args.iter().filter(|kind| kind.required()).count()
    }
}

/// A prototype the binders make once per engine.
pub struct HostClass {
    /// The name the engine caches the prototype under; unique among classes.
    pub name: &'static str,
    pub methods: &'static [Method],
    pub getters: &'static [Method],
    /// Functions given to [`HostClass::shim_js`] and not put on the prototype.
    pub hidden: &'static [Method],
    /// `(function (proto, hidden) { ... })`, run once when the prototype is made, for what is written in JavaScript
    /// (a method over a `RegExp`, a `Proxy`). `hidden` holds the [`HostClass::hidden`] functions by name.
    pub shim_js: Option<&'static str>,
    /// The members the shim adds to the prototype, which the static check allows as it allows the methods.
    pub shim_members: &'static [&'static str],
}

/// What a body returns.
// the kinds, returns and operations the facades B2 moves (services, entities, the spec engine) use
#[allow(dead_code)]
pub enum Ret {
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

impl From<()> for Ret {
    fn from((): ()) -> Self {
        Ret::Undefined
    }
}

impl From<i32> for Ret {
    fn from(number: i32) -> Self {
        Ret::Value(Value::I32(number))
    }
}

impl From<f64> for Ret {
    fn from(number: f64) -> Self {
        Ret::Value(Value::F64(number))
    }
}

impl From<Vec<String>> for Ret {
    fn from(texts: Vec<String>) -> Self {
        Ret::Value(Value::List(texts.into_iter().map(Value::String).collect()))
    }
}

impl From<Option<String>> for Ret {
    fn from(text: Option<String>) -> Self {
        Ret::Value(text.map_or(Value::Null, Value::String))
    }
}

/// A body's failure, thrown as the facades throw it on QuickJS.
#[derive(Debug)]
pub enum HostError {
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
    pub fn message(text: impl Into<String>) -> Self {
        HostError::Message(text.into())
    }
}

/// The marks a thrown error carries (`moquiErrorKind`, `moquiRetryAfter`), as runtime.rs's `mark_kind` puts them; the
/// oxide binder writes the same properties so the engine's error mapping reads them back.
pub const KIND_PROPERTY: &str = "moquiErrorKind";
pub const RETRY_AFTER_PROPERTY: &str = "moquiRetryAfter";

/// The name of a kind a thrown error carries, `None` for one it does not.
pub fn carried_kind_name(kind: crate::context::ErrorKind) -> Option<&'static str> {
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
pub struct Thrown<'a> {
    pub message: &'a str,
    pub diagnostic: String,
    pub kind: Option<&'static str>,
    pub retry_after: Option<u64>,
}

impl HostError {
    /// The parts of a marked error; `None` for the others.
    pub fn marked(&self) -> Option<Thrown<'_>> {
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
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
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
pub trait EngineOps {
    /// Call the [`Arg::Callback`] at `index` with `args`; its result as a host value. A throw is [`HostError::Thrown`].
    fn call_back(&mut self, index: usize, args: Vec<Value>) -> Result<Value, HostError>;
    /// Write the engine's tracked values back to the context stack.
    fn flush(&mut self, mode: FlushBefore) -> Result<(), HostError>;
    /// The whole context as a script sees it, its tracked values written back first ([`Arg::WholeContext`]).
    fn whole_context(&mut self) -> Result<indexmap::IndexMap<String, Value>, HostError>;
    /// Evaluate `source` (a script whose completion value is the result, `name` its file name in errors) in the running
    /// engine; its value as a host value. A script that throws is [`HostError::Message`] of the error's text.
    fn evaluate(&mut self, source: &str, name: &str) -> Result<Value, HostError>;
    /// Run `body` with the engine lent to it, so a nested script it runs (a service, an entity ECA rule) runs on this
    /// engine: for a leaf method that calls back but has one step that may nest. A panic in `body` is an error.
    fn lend(&mut self, body: &mut dyn FnMut()) -> Result<(), HostError> {
        body();
        Ok(())
    }
    /// The screen runner of the running engine, for a body that renders (`sri.renderSection`, `sri.renderInlineText`):
    /// the units and templates it runs are in this engine, in the script that made the call (its unit depth and its
    /// tracked values), as the call itself is.
    fn screen_runner(&mut self) -> Result<&dyn crate::screen::sri::Runner, HostError> {
        Err(HostError::message("screen units do not run on this engine yet"))
    }
    /// Run the engine's oldest pending job (a promise reaction); `false` when none is pending. The engines run no job
    /// unless asked, so the test harness's `settle` drives a promise to its end with this, one job at a time.
    fn run_pending_job(&mut self) -> Result<bool, HostError> {
        Err(HostError::message("running pending jobs is not supported here"))
    }
}

/// How many converted arguments a call holds without allocating.
const INLINE_ARGS: usize = 4;

/// The converted arguments of a call: the first few inline, the rest (rare) in a vector.
pub struct Args {
    inline: [Arg; INLINE_ARGS],
    len: usize,
    spill: Vec<Arg>,
}

impl Default for Args {
    fn default() -> Self {
        Self::new()
    }
}

impl Args {
    pub fn new() -> Self {
        Args {
            inline: [const { Arg::Missing }; INLINE_ARGS],
            len: 0,
            spill: Vec::new(),
        }
    }

    pub fn push(&mut self, arg: Arg) {
        match self.inline.get_mut(self.len) {
            Some(slot) => *slot = arg,
            None => self.spill.push(arg),
        }
        self.len += 1;
    }

    pub fn get(&self, index: usize) -> Option<&Arg> {
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
pub fn lock_state<'a, T>(state: &'a Mutex<T>, class: &str) -> Result<MutexGuard<'a, T>, HostError> {
    match state.try_lock() {
        Ok(guard) => Ok(guard),
        Err(std::sync::TryLockError::WouldBlock) => Err(HostError::message(format!("re-entrant call on {class}"))),
        Err(std::sync::TryLockError::Poisoned(_)) => Err(HostError::message(format!(
            "the state of this {class} was left unfinished by an earlier failure"
        ))),
    }
}

/// One call of a [`Method`].
pub struct Call<'a> {
    /// The receiver's state, when the receiver is an object of a host class.
    pub this: Option<&'a HostObject>,
    pub args: Args,
    #[allow(dead_code)] // no migrated facade calls back yet
    pub ops: &'a mut dyn EngineOps,
}

impl Call<'_> {
    pub fn arg(&self, index: usize) -> &Arg {
        self.args.get(index).unwrap_or(&Arg::Missing)
    }

    /// Whether the argument was passed at all.
    pub fn passed(&self, index: usize) -> bool {
        !matches!(self.arg(index), Arg::Missing)
    }

    /// The host value of a value argument; `None` when missing or undefined.
    pub fn value(&self, index: usize) -> Option<&Value> {
        match self.arg(index) {
            Arg::Value(value) => Some(value),
            _ => None,
        }
    }

    /// [`Call::value`], null when missing or undefined.
    pub fn value_or_null(&self, index: usize) -> Value {
        self.value(index).cloned().unwrap_or(Value::Null)
    }

    /// The text of a text argument; `None` when missing, null or undefined.
    pub fn text(&self, index: usize) -> Option<&str> {
        match self.arg(index) {
            Arg::Text(text) => Some(text),
            _ => None,
        }
    }

    /// The plain text of a value argument, `None` when missing, null or undefined (runtime/convert.rs `optional_text`).
    pub fn opt_text(&self, index: usize) -> Option<String> {
        match self.arg(index) {
            Arg::Value(Value::Null) => None,
            Arg::Value(value) => Some(moqui_lib::object::to_plain_string(value)),
            Arg::Text(text) => Some(text.clone()),
            _ => None,
        }
    }

    /// [`Call::opt_text`], empty when there is none (`text_or_empty`).
    pub fn text_or_empty(&self, index: usize) -> String {
        self.opt_text(index).unwrap_or_default()
    }

    /// The state of a host-object argument as a `T`; `None` when it is not one.
    pub fn host<T: std::any::Any + Send + Sync>(&self, index: usize) -> Option<Arc<T>> {
        match self.arg(index) {
            Arg::Host(obj) => obj.downcast::<T>(),
            _ => None,
        }
    }

    /// The strings of an [`ArgKind::Texts`] argument; `None` when it was not passed.
    pub fn texts(&self, index: usize) -> Option<&[String]> {
        match self.arg(index) {
            Arg::Texts(texts) => Some(texts),
            _ => None,
        }
    }

    /// The element literal of an [`ArgKind::Node`] or [`ArgKind::OptNode`] argument.
    pub fn node(&self, index: usize) -> Option<&Arc<crate::screen::sri::NodeLit>> {
        match self.arg(index) {
            Arg::Node(node) => Some(node),
            _ => None,
        }
    }

    #[allow(dead_code)] // no migrated facade takes a boolean yet
    pub fn bool(&self, index: usize) -> Option<bool> {
        match self.arg(index) {
            Arg::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn int(&self, index: usize) -> Option<i32> {
        match self.arg(index) {
            Arg::Int(number) => Some(*number),
            _ => None,
        }
    }

    pub fn number(&self, index: usize) -> Option<f64> {
        match self.arg(index) {
            Arg::Number(number) => Some(*number),
            _ => None,
        }
    }

    /// The receiver's state as a `T`; a `TypeError` when the receiver is not one.
    pub fn this<T: std::any::Any + Send + Sync>(&self) -> Result<Arc<T>, HostError> {
        self.this
            .and_then(HostObject::downcast::<T>)
            .ok_or_else(|| HostError::Type("the receiver is not the host object this method is for".to_string()))
    }
}

/// The bytes behind a `HostStream` (QuickJS), as a host value: a stream passed to a service (`upload#Resource` with
/// `fileList: [ref.openStream()]`) or kept in the context stays the stream, as a Java `InputStream` would, instead of
/// becoming an empty map that a write would store as the two bytes `{}`.
pub(crate) struct StreamBytes {
    pub(crate) source: Arc<dyn crate::resource::source::ByteSource>,
}

impl StreamBytes {
    /// All of the content, read from its source.
    pub(crate) fn bytes(&self) -> Result<Vec<u8>, moqui_lib::error::Error> {
        crate::resource::source::read_all(self.source.as_ref())
    }
}

/// Bytes as text in a Java charset name, as `new String(bytes, charset)`: UTF-8 when none is given (malformed sequences
/// become U+FFFD), ISO-8859-1 byte for byte, UTF-16 by its byte order mark (big-endian without one, as Java), and the other
/// names of the WHATWG Encoding Standard (windows-1252, UTF-16LE, Shift_JIS, ...). A byte order mark of the charset
/// itself is not removed, as Java keeps it. An unknown name is an error, as Java's `UnsupportedEncodingException`.
pub(crate) fn decode_text(bytes: &[u8], charset: Option<&str>) -> Result<String, crate::context::Error> {
    let Some(charset) = charset.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    };
    let upper = charset.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "ISO-8859-1" | "ISO8859-1" | "ISO8859_1" | "LATIN1" | "ISO-LATIN-1"
    ) {
        return Ok(bytes.iter().map(|byte| char::from(*byte)).collect());
    }
    if upper == "UTF-16" || upper == "UTF16" {
        let little = bytes.starts_with(&[0xFF, 0xFE]);
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|pair| {
                let (first, second) = (pair[0], pair.get(1).copied().unwrap_or(0));
                if little {
                    u16::from_le_bytes([first, second])
                } else {
                    u16::from_be_bytes([first, second])
                }
            })
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    let encoding = crate::sftp::encoding_for(charset)?;
    Ok(crate::sftp::decode(encoding, bytes))
}

/// The key a script `Set` converts under: a map of it to the set's items (runtime/convert.rs `SET_KEY`).
pub const SET_KEY: &str = "__set__";

/// The texts of a list, of a converted `Set`, or of one value; none for null (runtime/convert.rs `host_texts`).
pub fn host_texts(value: Value) -> Vec<String> {
    match value {
        Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Value::Map(mut map) if map.contains_key(SET_KEY) => host_texts(map.swap_remove(SET_KEY).unwrap_or(Value::Null)),
        Value::Null => Vec::new(),
        other => vec![moqui_lib::object::to_plain_string(&other)],
    }
}

/// The JavaScript truthiness of a converted value (runtime/convert.rs `js_truthy`).
pub fn truthy(value: &Value) -> bool {
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
pub fn with_ec<T>(body: impl FnOnce(&crate::context::ExecutionContext) -> T) -> Result<T, HostError> {
    crate::context::ExecutionContext::with_current(body)
        .ok_or_else(|| HostError::message("no execution context is entered"))
}

/// The factory of the entered execution context.
pub fn factory() -> Result<Arc<crate::context::ExecutionContextFactory>, HostError> {
    with_ec(|ec| Arc::clone(ec.factory()))
}

/// The entity facade of the entered execution context; the error the facades throw when none is installed.
pub fn entity_facade() -> Result<crate::entity::EntityFacade, HostError> {
    with_ec(|ec| ec.factory().entity())?.ok_or_else(|| HostError::message("no entity facade is installed"))
}

/// A [`Method`] whose census path is `$path.$name`: `host_method!("ec.l10n", "localize", [Value, OptValue], localize)`,
/// with an optional [`FlushBefore`] after the kinds. Another crate uses it as `moqui_core::host_method!`.
#[macro_export]
macro_rules! host_method {
    ($path:literal, $name:literal, [$($kind:ident),* $(,)?], $flush:ident, leaf, $f:expr) => {
        $crate::script::host::Method {
            name: $name,
            path: concat!($path, ".", $name),
            args: &[$($crate::script::host::ArgKind::$kind),*],
            flush: $crate::script::host::FlushBefore::$flush,
            leaf: true,
            f: $f,
        }
    };
    ($path:literal, $name:literal, [$($kind:ident),* $(,)?], $f:expr) => {
        $crate::host_method!($path, $name, [$($kind),*], None, $f)
    };
    ($path:literal, $name:literal, [$($kind:ident),* $(,)?], $flush:ident, $f:expr) => {
        $crate::script::host::Method {
            name: $name,
            path: concat!($path, ".", $name),
            args: &[$($crate::script::host::ArgKind::$kind),*],
            flush: $crate::script::host::FlushBefore::$flush,
            leaf: false,
            f: $f,
        }
    };
}
pub(crate) use crate::host_method as method;

/// An `ec` path the table installs and what it holds.
pub struct Path {
    /// `ec.l10n`
    pub path: &'static str,
    pub bound: Bound,
}

/// What a [`Path`] holds.
pub enum Bound {
    /// An object of the class.
    Object(&'static HostClass),
    /// A function (`ec.makeNotificationMessage`).
    Function(&'static Method),
    /// JavaScript run once when the engine starts, after `ec` and the helper objects are made, for a global outside
    /// `ec` (`EdiHandler`, `sri`'s URL functions, the tools methods across `ec` and the helpers).
    Shim(&'static Shim),
}

/// A [`Bound::Shim`]: `(function (global, hidden, helpers) { ... })`, called with the global object, an object of the
/// [`Shim::hidden`] functions by name, and an object of the engine's helper objects (`CollectionUtilities`,
/// `StringUtilities`, `WebUtilities`). What it builds (constructors, functions on `ec` or on the helpers) calls the
/// hidden functions; the functions themselves are not reachable by name.
pub struct Shim {
    pub js: &'static str,
    /// The hidden functions, in groups (a shim over several files takes each file's table).
    pub hidden: &'static [&'static [Method]],
}

impl Path {
    pub const fn object(path: &'static str, class: &'static HostClass) -> Self {
        Path {
            path,
            bound: Bound::Object(class),
        }
    }

    pub const fn function(path: &'static str, method: &'static Method) -> Self {
        Path {
            path,
            bound: Bound::Function(method),
        }
    }

    /// A shim, named by the global it makes (`EdiHandler`); not an `ec` path, so the static check does not allow it.
    pub const fn shim(name: &'static str, shim: &'static Shim) -> Self {
        Path {
            path: name,
            bound: Bound::Shim(shim),
        }
    }
}

/// The paths of this crate. In ec member order.
static PATHS: &[Path] = &[
    Path::object("ec.message", &message::MESSAGE),
    Path::object("ec.logger", &logger::LOGGER),
    Path::object("ec.service", &service::SERVICE_FACADE),
    Path::object("ec.user", &user::USER),
    Path::object("ec.cache", &cache::CACHE_FACADE),
    Path::object("ec.search", &search::SEARCH),
    Path::object("ec.l10n", &l10n::L10N),
    Path::function("ec.makeNotificationMessage", &notification::MAKE),
    Path::object("ec.web", &ec_web::WEB),
    Path::object("ec.screen", &screen::SCREEN),
    Path::function("ec.resource.xslFoTransform", &screen::XSL_FO_TRANSFORM),
    Path::object("ec.resource.theme", &theme::THEME),
    Path::shim("tools", &tools::SHIM),
    Path::shim("EdiHandler", &edi::SHIM),
    Path::shim("sri", &sri::SHIM),
    Path::shim("SftpClient", &sftp::SHIM),
];

/// The classes a method may return ([`Ret::Object`]), which a static check cannot see but must be complete on both
/// engines: every class here is, by being in the table.
static RETURNED: &[&HostClass] = &[
    &cache::CACHE,
    &service::SERVICE_CALL,
    &service::SERVICE_JOB,
    &service::SERVICE_FUTURE,
    &notification::NOTIFICATION,
    &edi::EDI_HANDLER,
    &urls::URL,
    &sri::NODE_LIT,
    &sftp::SFTP_CLIENT,
    &entities::FIND,
    &entities::LIST,
    &entities::ENTITY_VALUE,
    &upload::UPLOAD,
    &upload::STREAM,
    &conditions::CONDITION,
    &error_info::ERROR_INFO,
    &native_tools::TOOL_FACTORY,
];

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
        let mut paths: Vec<&'static Path> = PATHS.iter().chain(entities::PATHS).chain(native_tools::PATHS).collect();
        // ec.llm, when the binary links the agent runtime
        if let Some(hooks) = crate::llm_hooks::get() {
            paths.extend(hooks.script_paths.iter());
        }
        for extra in moqui_lib::sync::lock(&EXTRA).iter() {
            paths.extend(extra.iter());
        }
        let mut exact = HashSet::new();
        let mut open = HashSet::new();
        for entry in &paths {
            if let Bound::Shim(_) = entry.bound {
                continue;
            }
            exact.insert(entry.path.to_string());
            let Bound::Object(class) = entry.bound else {
                continue;
            };
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
pub fn registry() -> &'static [Path] {
    PATHS
}

/// Add another crate's paths. Only before the table is frozen (the first engine's start or the first
/// [`path_allowed`]), so a path is either bound on both engines or on neither.
#[allow(dead_code)] // the hook for moqui-llm and moqui-devkit
pub fn register(paths: &'static [Path]) -> Result<(), String> {
    if TABLE.get().is_some() {
        return Err("the host table is already in use; register its paths before the first script runs".to_string());
    }
    moqui_lib::sync::lock(&EXTRA).push(paths);
    Ok(())
}

/// Every path the binders install: this crate's, then the registered ones.
pub fn all_paths() -> &'static [&'static Path] {
    &table().paths
}

/// Whether a script may reach `path` (`ec.l10n`, `ec.l10n.localize`) on both engines: a path the binders install, a
/// method or getter of its class, a member its shim adds, or anything below such a member. What a method returns is a
/// class of the table, so it is complete on both engines too. Takes no lock and allocates nothing.
pub fn path_allowed(path: &str) -> bool {
    let table = table();
    if table.exact.contains(path) {
        return true;
    }
    path.match_indices('.')
        .any(|(index, _)| table.open.contains(&path[..index]))
}

/// Every class of the table, for a binder that makes its prototypes ahead.
#[allow(dead_code)] // the oxide binder makes them on first use
pub fn classes() -> impl Iterator<Item = &'static HostClass> {
    all_paths()
        .iter()
        .filter_map(|path| match path.bound {
            Bound::Object(class) => Some(class),
            Bound::Function(_) | Bound::Shim(_) => None,
        })
        .chain(RETURNED.iter().copied())
}

/// The shims of the table, in table order.
pub fn shims() -> impl Iterator<Item = &'static Shim> {
    all_paths().iter().filter_map(|path| match path.bound {
        Bound::Shim(shim) => Some(shim),
        _ => None,
    })
}

/// Record that a script reached `path`, into the running script's census entry ([`crate::script::census::note`]).
#[inline]
pub fn note_path(path: &'static str) {
    crate::script::census::note(path);
}

#[cfg(test)]
pub(crate) mod test_class;
#[cfg(test)]
mod tests;
