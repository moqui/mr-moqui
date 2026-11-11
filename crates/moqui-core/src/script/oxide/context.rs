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

//! The bare name `context` on quickjs-oxide: the stack itself, as Java's `ContextStack`, an exotic object
//! ([`ContextStack`], the fork's F2) with the hooks of QuickJS's `HostContextStack` proxy (`runtime.rs`,
//! `CONTEXT_PROXY`). `context.x` reads as a bare `x`, `context.x = v` is a bare `x = v`, `'x' in context` holds for
//! a visible name (and `context`), `Object.keys(context)` and a spread list the visible names, and `delete context.x`
//! removes the name from the top frame. The methods of Java's `ContextStack` ([`METHODS`]: `get`, `put`, `putAll`,
//! `remove`, `containsKey`, `keySet`, `size`, `push`, `pop`, ...) are there for a name no frame holds, so a context
//! name wins over a method of the same name. `ec.context` is this same object. Passed to a host call, `context` is
//! the combined map of the visible names, with the run's tracked values written first ([`combined_map`]).

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{
    CompleteOrdinaryPropertyDescriptor, Context, ExoticClass, ExoticScope, FunctionOptions, Hook, JsResult, JsString,
    ObjectRef, PropertyKey, RuntimeError, Scope, Value,
};

use super::convert::{key as property_key, Conv};
use super::engine::{current, guarded, guarded_hook};
use super::tracked::Flush;
use crate::script::run::STACK;

/// The methods of the stack, answered for a name no frame holds.
const METHODS: &[&str] = &[
    "get",
    "put",
    "putAll",
    "remove",
    "containsKey",
    "keySet",
    "getCombinedMap",
    "size",
    "isEmpty",
    "containsValue",
    "values",
    "entrySet",
    "getByString",
    "toString",
    // the host table's (`host/context.rs`, `host/tools.rs`)
    "push",
    "pop",
    "pushContext",
    "popContext",
    "getRootMap",
    "addRootMap",
    "clear",
    "sharedMap",
    "getSharedMap",
];

/// The methods, over the stack object and a direct read of a name (so `context.get('put')` reads the name `put`).
const METHODS_JS: &str = r#"(function (context, read) {
    return {
        get: (name) => read(String(name)),
        put: (name, value) => { const key = String(name); const old = read(key); context[key] = value; return old; },
        putAll: (map) => { for (const [key, value] of Object.entries(map || {})) context[key] = value; },
        remove: (name) => { const key = String(name); const old = read(key); delete context[key]; return old; },
        containsKey: (name) => String(name) !== "context" && String(name) in context,
        keySet: () => Object.keys(context),
        getCombinedMap: () => { const out = {}; for (const key of Object.keys(context)) out[key] = read(key); return out; },
        size: () => Object.keys(context).length,
        isEmpty: () => Object.keys(context).length === 0,
        containsValue: (value) => Object.keys(context).some((key) => read(key) === value),
        values: () => Object.keys(context).map(read),
        entrySet: () => Object.keys(context).map((key) => {
            const value = read(key);
            return { key: key, value: value, getKey: () => key, getValue: () => value };
        }),
        getByString: (name) => read(String(name)),
        toString: () => "ContextStack[" + Object.keys(context).join(", ") + "]"
    };
})"#;

/// The datum of the stack object: none, the stack is the thread's.
pub(super) struct ContextTarget;

/// The class of the stack object.
pub(super) struct ContextStack;

fn name_of(key: &PropertyKey) -> Option<super::convert::KeyText> {
    key.to_js_string().map(|name| super::convert::text_of(&name))
}

fn holds(name: &str) -> bool {
    STACK.with(|stack| stack.borrow().contains_key(name))
}

impl ExoticClass for ContextStack {
    type Data = ContextTarget;

    fn get(&self, scope: &mut ExoticScope<'_>, _: &ContextTarget, key: &PropertyKey) -> JsResult<Hook<Value>> {
        let Some(name) = name_of(key) else {
            return Ok(Hook::Handled(Value::Undefined));
        };
        if !holds(&name) && METHODS.contains(&&*name) {
            return guarded_hook(scope, |scope| {
                let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
                Ok(scope.get_property(&engine.protos.context_methods, &property_key(&name)?)?)
            });
        }
        guarded_hook(scope, |scope| super::tracked::read_name(scope, &name))
    }

    fn set(
        &self,
        scope: &mut ExoticScope<'_>,
        _: &ContextTarget,
        key: &PropertyKey,
        value: Value,
    ) -> JsResult<Hook<bool>> {
        let Some(name) = name_of(key) else {
            return Ok(Hook::Handled(true));
        };
        guarded_hook(scope, |scope| {
            super::tracked::write_name(scope, &name, &value)?;
            Ok(true)
        })
    }

    fn has(&self, _: &mut ExoticScope<'_>, _: &ContextTarget, key: &PropertyKey) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(
            name_of(key).is_some_and(|name| &*name == "context" || holds(&name)),
        ))
    }

    /// The name goes from the top frame, as Java's `ContextStack.remove`, and the run's object of it is forgotten.
    fn delete(&self, scope: &mut ExoticScope<'_>, _: &ContextTarget, key: &PropertyKey) -> JsResult<Hook<bool>> {
        let Some(name) = name_of(key) else {
            return Ok(Hook::Handled(true));
        };
        guarded_hook(scope, |scope| {
            super::tracked::forget(scope, &name)?;
            STACK.with(|stack| {
                stack.borrow_mut().remove(&name);
            });
            Ok(true)
        })
    }

    fn get_own_property(
        &self,
        scope: &mut ExoticScope<'_>,
        _: &ContextTarget,
        key: &PropertyKey,
    ) -> JsResult<Hook<Option<CompleteOrdinaryPropertyDescriptor>>> {
        let Some(name) = name_of(key).filter(|name| holds(name)) else {
            return Ok(Hook::Handled(None));
        };
        let value = guarded(scope, |scope| super::tracked::read_name(scope, &name))?;
        Ok(Hook::Handled(Some(CompleteOrdinaryPropertyDescriptor::Data {
            value,
            writable: true,
            enumerable: true,
            configurable: true,
        })))
    }

    fn own_keys(&self, _: &mut ExoticScope<'_>, _: &ContextTarget) -> JsResult<Hook<Vec<PropertyKey>>> {
        let names = STACK.with(|stack| stack.borrow().combined_keys());
        let mut keys = Vec::with_capacity(names.len());
        for name in names {
            keys.push(PropertyKey::from_js_string(JsString::try_from_utf8(&name)?));
        }
        Ok(Hook::Handled(keys))
    }
}

/// The stack object of a runtime and its methods, made when it starts.
pub(super) fn make(context: &Context) -> Result<(ObjectRef, ObjectRef), RuntimeError> {
    let class = context.register_exotic_class(ContextStack)?;
    let object = context.new_host_exotic(ContextTarget, &class, None)?;
    let read = context.new_function(
        FunctionOptions::new("read").length(1),
        |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |scope| {
                let name = scope.arg(0);
                let name = scope.to_js_string(&name)?.to_utf8_lossy();
                super::tracked::read_name(scope, &name)
            })
        },
    )?;
    let make = super::compiled::eval_start(&mut context.clone(), METHODS_JS)?;
    let make = match &make {
        Value::Object(made) => context.as_callable(made)?,
        _ => None,
    };
    let Some(make) = make else {
        return Err(RuntimeError::Invariant("the context methods"));
    };
    let methods = context.call(
        &make,
        Value::Undefined,
        &[Value::Object(object.clone()), Value::from(read)],
    )?;
    let Value::Object(methods) = methods else {
        return Err(RuntimeError::Invariant("the context methods"));
    };
    Ok((object, methods))
}

/// Whether `object` is the stack object.
pub(super) fn is_stack<J: quickjs_oxide::engine::api::core::Js>(js: &mut J, object: &ObjectRef) -> JsResult<bool> {
    Ok(js.host_data::<ContextTarget>(object)?.is_some())
}

/// `context` as a host value: the combined map of the visible names, after the run's tracked values are written back
/// (kept tracked), as QuickJS's `context_combined_map` passes the stack to a host call.
pub(super) fn combined_map<J: quickjs_oxide::engine::api::core::Js>(js: &mut J) -> Result<HostValue, Conv> {
    super::tracked::flush(js, Flush::Keep)?;
    Ok(HostValue::Map(STACK.with(|stack| stack.borrow().combined_map())))
}
