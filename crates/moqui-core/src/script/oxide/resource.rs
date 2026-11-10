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

//! The members of `ec.resource` that run script text in the calling script's own run, as QuickJS's facades do
//! (`runtime/facades.rs`, `runtime/ec_extra.rs`): `script` (a script file's value, an inert one kept for the
//! outermost run), `expand` and `expandNoL10n` (a `${}` template), `expression` and `condition`. They are not on the
//! host table: what they give is a value of the running engine (a module's functions), and the text they run reads
//! and writes the caller's names, tracked values included.
//!
//! Each compiles through the runtime's compiled cache and runs while the host function lends the engine
//! (`Scope::lend`), so the text is a part of the run that called it: its bare names are the caller's, its errors the
//! caller's to catch. A text given an additional-context map runs in a frame of its own, the caller's tracked values
//! released first as on QuickJS.

use std::sync::Arc;

use quickjs_oxide::engine::api::core::{Context, FunctionOptions, JsResult, ObjectRef, RuntimeError, Scope, Value};

use super::convert::{key, to_host, truthy, Conv};
use super::engine::{current, OxideEngine};
use super::errors::runtime_error;
use super::tracked::Flush;
use crate::context::Error;

/// Put `script`, `expand`, `expandNoL10n`, `expression` and `condition` on `ec.resource`, made if need be.
pub(super) fn install(context: &Context, ec: &ObjectRef) -> Result<(), RuntimeError> {
    let resource_key = key("resource").map_err(|_| RuntimeError::Invariant("a key"))?;
    let resource = match context.get_property(ec, &resource_key)? {
        Value::Object(found) => found,
        _ => {
            let made = context.new_object()?;
            context.set_property(ec, &resource_key, Value::Object(made.clone()))?;
            made
        }
    };
    let define = |name: &'static str, length: u32, body: fn(&mut Scope<'_>) -> Result<Value, Conv>| {
        let function = context.new_function(
            FunctionOptions::new(name).length(length),
            move |scope: &mut Scope<'_>| -> JsResult<Value> { super::bind::host_body(scope, body) },
        )?;
        let name = key(name).map_err(|_| RuntimeError::Invariant("a key"))?;
        context.set_property(&resource, &name, Value::from(function))
    };
    define("script", 1, script)?;
    define("expand", 3, |scope| expand(scope, true))?;
    define("expandNoL10n", 3, |scope| expand(scope, false))?;
    define("expression", 3, expression)?;
    define("condition", 3, condition)?;
    Ok(())
}

/// The thread's runtime, which is running this call.
fn engine() -> Result<std::rc::Rc<OxideEngine>, Conv> {
    current().ok_or(Conv::Error("no quickjs-oxide runtime is running".to_string()))
}

/// Run `source` (named `file_name` in stack traces, kept by `hash`) in the running script: compiled through the
/// runtime's cache and executed while the engine is lent, so its names are the caller's. A `widget` text (an
/// expression or template, not a script file) runs with the widget check's wrapper of `ec` when the check is on, as
/// QuickJS's facades run it ([`super::widget`]).
fn run_here(
    scope: &mut Scope<'_>,
    source: &str,
    hash: u64,
    file_name: &str,
    widget: Option<&str>,
) -> Result<Value, RuntimeError> {
    let engine = engine().map_err(|_| RuntimeError::Invariant("no oxide engine"))?;
    super::bind::lend(scope, || {
        let _engine = super::bind::engine_call();
        let compiled = engine.compile(source, hash, file_name)?;
        match widget {
            Some(text) => super::widget::with_widget_ec(&engine, Some(text), || engine.context().execute(&compiled)),
            None => engine.context().execute(&compiled),
        }
    })
}

/// The error a text run here threw, as QuickJS's facades throw it on: a plain `Error` of the framework error's text.
/// The deadline's interrupt and the runtime's own failures go on as they are.
fn thrown_on(scope: &mut Scope<'_>, err: RuntimeError) -> Conv {
    match err {
        RuntimeError::Exception(_) => Conv::Error(runtime_error(scope, err).message().to_string()),
        other => Conv::Js(other),
    }
}

/// `ec.resource.script(location)`: the file's value (what it returns, or its last expression statement), run in the
/// caller's names. A file whose top level only declares gives the same value each time, so the value it gave earlier
/// in this outermost run is given again (`parse::module_is_inert`).
fn script(scope: &mut Scope<'_>) -> Result<Value, Conv> {
    let location = match scope.arg(0) {
        Value::String(text) => text.to_utf8_lossy(),
        other => scope.to_js_string(&other)?.to_utf8_lossy(),
    };
    let marked = |scope: &mut Scope<'_>, err: Error| {
        Conv::Js(super::bind::throw(
            scope,
            crate::script::host::HostError::Context(err),
            None,
        ))
    };
    let (text, inert) = match crate::script::runtime::read_module(&location) {
        Ok(read) => read,
        Err(err) => return Err(marked(scope, err)),
    };
    let inert = inert && crate::script::runtime::module_cache_on();
    let engine = engine()?;
    let source = text.shared_text();
    if inert {
        if let Some(kept) = engine.kept_module(&location, source) {
            return Ok(kept);
        }
    }
    let hash = crate::script::text::hash_of(text.hash(), text.file_name());
    let value = match run_here(scope, source, hash, text.file_name(), None) {
        Ok(value) => value,
        Err(RuntimeError::Exception(thrown)) => {
            let err = runtime_error(scope, RuntimeError::Exception(thrown));
            return Err(marked(scope, err));
        }
        Err(other) => return Err(Conv::Js(other)),
    };
    if inert {
        engine.keep_module(&location, Arc::clone(source), value.clone());
    }
    Ok(value)
}

/// The text of argument `index`, empty for `null` or `undefined` (a null template expands to nothing, as in Java).
fn text_arg(scope: &mut Scope<'_>, index: usize) -> Result<String, Conv> {
    Ok(match scope.arg(index) {
        Value::Undefined | Value::Null => String::new(),
        Value::String(text) => text.to_utf8_lossy(),
        other => scope.to_js_string(&other)?.to_utf8_lossy(),
    })
}

/// Run `body` with the fields of the additional-context argument (a map or an entity value, argument 2) in a frame of
/// their own, the caller's tracked values released before and after, as QuickJS's `with_additional`. With no map they
/// are written back first, kept, when `keep` (an expression reads the stack as it is).
fn with_additional<T>(
    scope: &mut Scope<'_>,
    keep: bool,
    body: impl FnOnce(&mut Scope<'_>) -> Result<T, Conv>,
) -> Result<T, Conv> {
    let map = scope.arg(2);
    let entries = match &map {
        Value::Object(_) => {
            let engine = engine()?;
            crate::script::runtime::value_map(&to_host(scope, &engine, &map)?)
        }
        _ => None,
    };
    let Some(entries) = entries else {
        if keep {
            super::tracked::flush(scope, Flush::Keep)?;
        }
        return body(scope);
    };
    super::tracked::flush(scope, Flush::Release)?;
    let frame = crate::script::runtime::ContextFrame::push();
    crate::script::runtime::put_context_all(&entries);
    let result = body(scope);
    // before the pop, so a list the body read under an additional name is not written over the caller's
    let flushed = super::tracked::flush(scope, Flush::Release);
    drop(frame);
    flushed?;
    result
}

/// On QuickJS a widget check goes on after a violation, with `none` for the value.
fn or_in_widget_check<T>(result: Result<T, Conv>, none: impl FnOnce() -> T) -> Result<T, Conv> {
    match result {
        Err(Conv::Error(_)) if crate::script::runtime::widget_wrap_on() => Ok(none()),
        other => other,
    }
}

/// `ec.resource.expand(template, location, map)`, localized first when `localize`, and `expandNoL10n`.
fn expand(scope: &mut Scope<'_>, localize: bool) -> Result<Value, Conv> {
    let template = text_arg(scope, 0)?;
    if template.is_empty() {
        return Ok(text_value(""));
    }
    let text = if localize {
        crate::script::runtime::localized(&template).map_err(|err| Conv::Error(err.message().to_string()))?
    } else {
        template
    };
    let expanded = with_additional(scope, false, |scope| {
        if !text.contains('$') || text.ends_with('$') {
            return Ok(text_value(&text));
        }
        let (literal, hash) = crate::script::runtime::template_source(&text);
        match run_here(scope, &literal, hash, super::compiled::FILE_NAME, Some(&text)) {
            Ok(Value::String(text)) => Ok(Value::String(text)),
            Ok(other) => Ok(Value::String(scope.to_js_string(&other)?)),
            Err(err) => Err(thrown_on(scope, err)),
        }
    });
    or_in_widget_check(expanded, || text_value(""))
}

/// `ec.resource.expression(expression, location, map)`: its value in the caller's names; null for an empty one.
fn expression(scope: &mut Scope<'_>) -> Result<Value, Conv> {
    let expression = text_arg(scope, 0)?;
    if expression.trim().is_empty() {
        return Ok(Value::Null);
    }
    let value = with_additional(scope, true, |scope| evaluate(scope, &expression));
    or_in_widget_check(value, || Value::Null)
}

/// `ec.resource.condition(expression, location, map)`: whether it holds, by JavaScript's truth; false for an empty one.
fn condition(scope: &mut Scope<'_>) -> Result<Value, Conv> {
    let expression = text_arg(scope, 0)?;
    if expression.trim().is_empty() {
        return Ok(Value::Bool(false));
    }
    let holds = with_additional(scope, true, |scope| {
        evaluate(scope, &expression).map(|value| truthy(&value))
    });
    or_in_widget_check(holds, || false).map(Value::Bool)
}

/// The expression strict, as QuickJS's `resource_expression` compiles it (`eval_compiled(.., true)`): the directive
/// on the same line, so its lines are the text's, and a strict text is kept apart from the same text run sloppy.
fn evaluate(scope: &mut Scope<'_>, expression: &str) -> Result<Value, Conv> {
    let (source, hash) = strict_source(expression);
    run_here(scope, &source, hash, super::compiled::FILE_NAME, Some(expression)).map_err(|err| thrown_on(scope, err))
}

/// The strict source of `expression` and its hash, kept by the thread for the texts it evaluated, so a call of the
/// same text formats and hashes no new source (plans/Profiling3.md W15). Up to [`STRICT_SOURCES`] texts; a full memo
/// starts again.
fn strict_source(expression: &str) -> (std::rc::Rc<str>, u64) {
    thread_local! {
        static SOURCES: std::cell::RefCell<StrictSources> = std::cell::RefCell::default();
    }
    SOURCES.with_borrow_mut(|sources| {
        if let Some((source, hash)) = sources.get(expression) {
            return (std::rc::Rc::clone(source), *hash);
        }
        let source: std::rc::Rc<str> =
            format!("\"use strict\"; {}", crate::script::runtime::as_expression(expression)).into();
        let hash = crate::script::text::hash_text(&source);
        if sources.len() >= STRICT_SOURCES {
            sources.clear();
        }
        sources.insert(expression.into(), (std::rc::Rc::clone(&source), hash));
        (source, hash)
    })
}

/// An expression's text to its strict source and that source's hash.
type StrictSources = std::collections::HashMap<Box<str>, (std::rc::Rc<str>, u64), foldhash::fast::RandomState>;

/// The expressions a thread keeps the strict source of.
const STRICT_SOURCES: usize = 1024;

fn text_value(text: &str) -> Value {
    match quickjs_oxide::engine::api::core::JsString::try_from_utf8(text) {
        Ok(text) => Value::String(text),
        Err(_) => Value::Undefined,
    }
}
