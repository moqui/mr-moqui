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

use std::rc::Rc;

use quickjs_oxide::engine::api::core::{Context, Runtime, RuntimeError, Scope, Value as JsValue};

use moqui_lib::object::Value;
use quickjs_oxide::engine::api::core::{JsResult, PropertyKey};

use super::{install_ec, install_paths, key, plain_error, text, OxideHooks};
use crate::script::host::test_class::{EXPECTED, TEST_PATHS};

/// A bare runtime with a global `ec` and the given paths installed under it.
fn runtime_with(paths: &[&'static crate::script::host::Path]) -> Result<(Runtime, Context), RuntimeError> {
    let runtime = Runtime::new();
    let mut context = runtime.context();
    let ec = context.new_object()?;
    let global = context.global_object()?;
    context.set_property(&global, &key("ec")?, JsValue::Object(ec.clone()))?;
    install_paths(&mut context, &ec, paths, Rc::new(BasicConvert))?;
    Ok((runtime, context))
}

/// The text of `script`'s value, or of what it threw.
fn eval_text(context: &Context, script: &str) -> Result<String, RuntimeError> {
    let value = match context.eval(&format!("String({script})")) {
        Ok(value) => value,
        Err(RuntimeError::Exception(thrown)) => {
            let thrown = context.to_js_string(&thrown)?.to_utf8_lossy();
            return Ok(format!("threw {thrown}"));
        }
        Err(err) => return Err(err),
    };
    Ok(context.to_js_string(&value)?.to_utf8_lossy())
}

/// The test class answers each script as the QuickJS binder does (`script/runtime/bind_tests.rs`): conversions, names
/// and lengths, getters, the shim, builders, callbacks, and the errors thrown, a panic among them, after which the
/// runtime still runs scripts.
#[test]
fn the_oxide_binder_answers_the_host_table_scripts() -> Result<(), RuntimeError> {
    let paths: Vec<&'static crate::script::host::Path> = TEST_PATHS.iter().collect();
    let (_runtime, context) = runtime_with(&paths)?;
    for (script, expected) in EXPECTED {
        let got = eval_text(&context, script)?;
        assert_eq!(&got, expected, "{script}");
    }
    assert_eq!(
        eval_text(&context, "1 + 1")?,
        "2",
        "a panic does not poison the runtime"
    );
    Ok(())
}

/// The table's own paths install, each an object whose methods are on its prototype, and a method that needs no
/// execution context answers.
#[test]
fn the_host_table_installs_on_a_bare_runtime() -> Result<(), RuntimeError> {
    let runtime = Runtime::new();
    let mut context = runtime.context();
    let ec = context.new_object()?;
    let global = context.global_object()?;
    context.set_property(&global, &key("ec")?, JsValue::Object(ec.clone()))?;
    install_ec(&mut context, &ec, Rc::new(BasicConvert))?;
    assert_eq!(
        eval_text(
            &context,
            "[typeof ec.l10n.localize, typeof ec.message.addError, typeof ec.user.getPreferences, \
             typeof ec.user.context.get, typeof ec.cache.getCache, ec.logger.info.length].join()"
        )?,
        "function,function,function,function,function,1"
    );
    assert_eq!(
        eval_text(
            &context,
            "(() => { try { ec.message.addError('x') } catch (e) { return e.message } })()"
        )?,
        "no execution context is entered"
    );
    Ok(())
}

/// A panic that unwinds through the engine under a binder call (here a host function outside the table, called back)
/// goes on to the embedder as a panic and leaves the runtime poisoned: the binder does not turn it into an error the
/// script catches.
#[test]
fn a_panic_through_the_engine_is_not_caught_as_an_error() -> Result<(), RuntimeError> {
    let paths: Vec<&'static crate::script::host::Path> = TEST_PATHS.iter().collect();
    let (_runtime, context) = runtime_with(&paths)?;
    let global = context.global_object()?;
    context.define_function(
        &global,
        "explode",
        0,
        |_: &mut Scope<'_>| -> Result<JsValue, RuntimeError> { panic!("engine-side panic") },
    )?;
    let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        context.eval("try { ec.test.twice(explode) } catch (e) { 'caught' }")
    }));
    assert!(first.is_err(), "the panic reaches the embedder");
    assert!(matches!(context.eval("1"), Err(RuntimeError::Poisoned)));
    Ok(())
}

/// Minimal conversions for the binder's tests on a bare runtime, which has no oxide engine for `oxide::convert`:
/// null, booleans, numbers, text, lists and maps; a decimal and a time stamp as their text.
pub(super) struct BasicConvert;

impl OxideHooks for BasicConvert {
    fn to_host(&self, scope: &mut Scope<'_>, value: &JsValue) -> JsResult<Value> {
        basic_to_host(scope, value, 0)
    }

    fn to_js(&self, scope: &mut Scope<'_>, value: Value) -> JsResult<JsValue> {
        basic_to_js(scope, value)
    }
}

/// How deep [`BasicConvert`] follows nested lists and maps, as runtime/convert.rs's `MAX_NESTING`.
const MAX_NESTING: usize = 200;

fn basic_to_host(scope: &mut Scope<'_>, value: &JsValue, depth: usize) -> JsResult<Value> {
    const SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    if depth > MAX_NESTING {
        return Err(plain_error(scope, "value is cyclic or nested too deeply"));
    }
    Ok(match value {
        JsValue::Undefined | JsValue::Null => Value::Null,
        JsValue::Bool(flag) => Value::Bool(*flag),
        JsValue::Int(number) => Value::I32(*number),
        #[allow(clippy::cast_possible_truncation)] // a whole number in the safe range
        JsValue::Float(number) if number.fract() == 0.0 && number.abs() <= SAFE_INTEGER => Value::I64(*number as i64),
        JsValue::Float(number) => Value::F64(*number),
        JsValue::String(text) => Value::String(text.to_utf8_lossy()),
        JsValue::Object(object) if scope.is_array_object(object)? => {
            let length = scope.get_property(object, &key("length")?)?;
            let length = scope.to_number(&length)?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // an array length
            let length = length as u32;
            let mut items = Vec::with_capacity(length as usize);
            for index in 0..length {
                let item = scope.get_property(object, &PropertyKey::index(index))?;
                items.push(basic_to_host(scope, &item, depth + 1)?);
            }
            Value::List(items)
        }
        JsValue::Object(object) => {
            let mut map = indexmap::IndexMap::new();
            for key in scope.own_property_keys(object)? {
                let Some(name) = key.to_js_string() else {
                    continue;
                };
                let item = scope.get_property(object, &key)?;
                if let JsValue::Object(item_object) = &item {
                    if scope.as_callable(item_object)?.is_some() {
                        continue;
                    }
                }
                map.insert(name.to_utf8_lossy(), basic_to_host(scope, &item, depth + 1)?);
            }
            Value::Map(map)
        }
        JsValue::BigInt(_) | JsValue::Symbol(_) => return Err(plain_error(scope, "cannot convert this value")),
    })
}

fn basic_to_js(scope: &mut Scope<'_>, value: Value) -> JsResult<JsValue> {
    Ok(match value {
        Value::Null => JsValue::Null,
        Value::Bool(flag) => JsValue::Bool(flag),
        Value::I32(number) => JsValue::Int(number),
        #[allow(clippy::cast_precision_loss)] // as runtime/convert.rs's `i64_to_js`, which refuses past 2^53
        Value::I64(number) => JsValue::number(number as f64),
        Value::F64(number) => JsValue::Float(number),
        Value::String(text) => self::text(&text)?,
        Value::List(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(basic_to_js(scope, item)?);
            }
            JsValue::Object(scope.new_array_from_values(values)?)
        }
        Value::Map(entries) => {
            let object = scope.new_object()?;
            for (name, item) in entries {
                let item = basic_to_js(scope, item)?;
                scope.set_property(&object, &key(&name)?, item)?;
            }
            JsValue::Object(object)
        }
        Value::Decimal(_) => self::text(&moqui_lib::object::to_plain_string(&value))?,
        Value::Timestamp(_) | Value::Date(_) | Value::Time(_) => {
            let converted = moqui_lib::object::basic_convert(&value, "String").value;
            match converted.as_str() {
                Some(text) => self::text(text)?,
                None => self::text(&moqui_lib::object::to_plain_string(&value))?,
            }
        }
        Value::Bytes(_) | Value::Object(_) => return Err(plain_error(scope, "cannot convert this object")),
    })
}

/// A tool for [`a_tool_is_an_exotic_object_of_its_methods`].
struct Greeter;

impl crate::native::Tool for Greeter {
    fn methods(&self) -> &'static [&'static str] {
        &["greet", "wave"]
    }
}

/// A native tool on oxide answers as `HostTool` does on QuickJS: its methods are own, non-enumerable functions, `in`
/// finds them and the prototype, a set and a delete are refused, and its other properties are read from the tool (in
/// an execution context, which a bare runtime has none of).
#[test]
fn a_tool_is_an_exotic_object_of_its_methods() -> Result<(), RuntimeError> {
    let runtime = Runtime::new();
    let mut context = runtime.context();
    let class = super::tool::register(&mut context, Rc::new(BasicConvert))?;
    let prototype = context.object_prototype()?;
    let tool = std::sync::Arc::new(crate::native::ToolHandle(std::sync::Arc::new(Greeter)));
    let object = super::tool::tool_object(&mut context, &class, tool, Some(&prototype))?;
    let global = context.global_object()?;
    context.set_property(&global, &key("tool")?, JsValue::Object(object))?;
    for (script, expected) in [
        ("typeof tool.greet + ' ' + tool.greet.name", "function greet"),
        ("Object.getOwnPropertyNames(tool).join()", "greet,wave"),
        ("Object.keys(tool).length", "0"),
        ("'wave' in tool && 'toString' in tool && !('other' in tool)", "true"),
        ("typeof tool.hasOwnProperty", "function"),
        ("(() => { 'use strict'; try { tool.greet = 1; return 'set'; } catch (e) { return e.name; } })()", "TypeError"),
        ("delete tool.greet", "false"),
        ("(() => { try { return tool.other; } catch (e) { return e.message; } })()", "no execution context is entered"),
    ] {
        assert_eq!(eval_text(&context, script)?, expected, "{script}");
    }
    Ok(())
}

/// A bare runtime with the table's `ec`, what the engine makes outside the table and empty helper objects, and every
/// shim of the host table installed over them; a shim that fails panics with what it threw.
pub(super) fn shimmed_runtime() -> Result<(Runtime, Context), RuntimeError> {
    let runtime = Runtime::new();
    let mut context = runtime.context();
    let ec = context.new_object()?;
    let global = context.global_object()?;
    context.set_property(&global, &key("ec")?, JsValue::Object(ec.clone()))?;
    install_ec(&mut context, &ec, Rc::new(BasicConvert))?;
    // what the engine makes outside the table, which the tools shim adds members to
    for name in ["context", "entity", "resource", "factory"] {
        if !matches!(context.get_property(&ec, &key(name)?)?, JsValue::Object(_)) {
            let member = context.new_object()?;
            context.set_property(&ec, &key(name)?, JsValue::Object(member))?;
        }
    }
    let sri = context.new_object()?;
    context.set_property(&global, &key("sri")?, JsValue::Object(sri))?;
    let helpers = context.new_object()?;
    for name in ["CollectionUtilities", "StringUtilities", "WebUtilities"] {
        let helper = context.new_object()?;
        context.set_property(&helpers, &key(name)?, JsValue::Object(helper))?;
    }
    if let Err(RuntimeError::Exception(thrown)) = super::install_shims(&mut context, &helpers, Rc::new(BasicConvert)) {
        let thrown = context.to_js_string(&thrown)?.to_utf8_lossy();
        panic!("a shim failed: {thrown}");
    }
    Ok((runtime, context))
}

/// Every shim of the host table installs on oxide too, over the table's `ec` and empty helper objects: none fails.
#[test]
fn every_host_shim_installs_on_oxide() -> Result<(), RuntimeError> {
    let (_runtime, context) = shimmed_runtime()?;
    assert_eq!(
        eval_text(
            &context,
            "[typeof ec.artifactExecution.runAsUser, typeof EdiHandler, typeof sri, typeof SftpClient].join()"
        )?,
        "function,function,object,function"
    );
    Ok(())
}
