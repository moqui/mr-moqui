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

//! Bare names and entity fields on quickjs-oxide, through Proxies with host-function traps until the fork's exotic
//! objects (F2) land (plans/Script-Engine-Oxide.md section 5.2).
//!
//! The global object's prototype is a Proxy: a name the global object does not have reaches its `has` (every string
//! name is there, as on QuickJS, so an unset name reads as null) and its `get`, which reads the context stack as
//! QuickJS's `read_name` does. A list, map, decimal or entity value read once in a run is kept for the run, so two reads
//! of a name are the same object (`big == big`). Phase 1 small scripts are read-only: a write is a fallback (the static
//! check refuses assignments, so none reaches here).
//!
//! An entity value is a Proxy over a host object of its cell: `get` reads a field (localized, a view's alias, null for
//! a field it does not hold), as the QuickJS `HostEntity` does; a relationship, a method, and every other trap fall
//! back to QuickJS for now.

use std::sync::Arc;

use moqui_lib::context_stack::Lookup;
use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{CallableRef, Context, JsResult, Scope, Value};

use super::convert::{to_js, Conv};
use super::engine::{current, guarded, OxideEngine};
use crate::script::run::STACK;
use crate::script::runtime::{entity_field, EntityCell, EntityField};

/// The target of an entity value's Proxy: the cell the context stack shares.
pub(super) struct EntityTarget(pub(super) Arc<EntityCell>);

/// The global names Proxy, made by `make(has, get, set)`.
const NAMES_JS: &str = "(function (has, get, set) { return new Proxy(Object.create(null), { has, get, set }); })";

/// The maker of an entity value's Proxy, over one handler for the runtime.
const ENTITY_JS: &str = "(function (handler) { return function (target) { return new Proxy(target, handler); }; })";

/// The entity traps that fall back: every one but `get`.
const ENTITY_FALLBACK_TRAPS: &[&str] = &[
    "has",
    "set",
    "deleteProperty",
    "defineProperty",
    "getOwnPropertyDescriptor",
    "ownKeys",
    "getPrototypeOf",
    "setPrototypeOf",
    "isExtensible",
    "preventExtensions",
];

/// The names the QuickJS entity value's prototype has (its methods): on QuickJS a method wins over a field of the
/// same name, so such a name falls back here. A test keeps the list in step with QuickJS.
pub(super) const ENTITY_METHODS: &[&str] = &[
    "cloneValue",
    "create",
    "createOrUpdate",
    "delete",
    "deleteRelated",
    "deleteWithRelated",
    "entrySet",
    "equals",
    "findRelated",
    "findRelatedFk",
    "findRelatedOne",
    "get",
    "getEntityName",
    "getEntityNamePretty",
    "getMap",
    "getPlainValueMap",
    "getPrimaryKeys",
    "refresh",
    "resolveEntityName",
    "resolveEntityNamePretty",
    "set",
    "setFields",
    "setSequencedIdPrimary",
    "setSequencedIdSecondary",
    "update",
];

/// Make the global object's prototype the names Proxy.
pub(super) fn install_names(context: &Context) -> Result<(), quickjs_oxide::engine::api::core::RuntimeError> {
    let has = context.new_function(
        quickjs_oxide::engine::api::core::FunctionOptions::new("has").length(2),
        |scope: &mut Scope<'_>| -> JsResult<Value> { Ok(Value::Bool(matches!(scope.arg(1), Value::String(_)))) },
    )?;
    let get = context.new_function(
        quickjs_oxide::engine::api::core::FunctionOptions::new("get").length(3),
        |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |scope| match scope.arg(1) {
                Value::String(name) => read_name(scope, &name.to_utf8_lossy()),
                _ => Ok(Value::Undefined),
            })
        },
    )?;
    let set = context.new_function(
        quickjs_oxide::engine::api::core::FunctionOptions::new("set").length(4),
        |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |_| Err(Conv::Unsupported("an assignment to a bare name")))
        },
    )?;
    let make = callable(context, NAMES_JS)?;
    let proxy = context.call(
        &make,
        Value::Undefined,
        &[Value::from(has), Value::from(get), Value::from(set)],
    )?;
    let Value::Object(proxy) = proxy else {
        return Err(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
            "the names proxy",
        ));
    };
    let global = context.global_object()?;
    context.set_prototype_of(&global, Some(&proxy))?;
    Ok(())
}

/// The maker of entity value Proxies for the runtime.
pub(super) fn entity_maker(context: &Context) -> Result<CallableRef, quickjs_oxide::engine::api::core::RuntimeError> {
    let handler = context.new_object()?;
    context.define_function(&handler, "get", 3, |scope: &mut Scope<'_>| -> JsResult<Value> {
        guarded(scope, |scope| {
            let target = scope.arg(0);
            let Value::Object(target) = target else {
                return Err(Conv::Unsupported("an entity value"));
            };
            let Some(cell) = scope.host_data::<EntityTarget>(&target)? else {
                return Err(Conv::Unsupported("an entity value"));
            };
            match scope.arg(1) {
                Value::String(name) => read_field(scope, &cell.0, &name.to_utf8_lossy()),
                _ => Ok(Value::Undefined),
            }
        })
    })?;
    for trap in ENTITY_FALLBACK_TRAPS {
        context.define_function(&handler, trap, 2, |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |_| {
                Err(Conv::Unsupported("an entity value operation other than a field read"))
            })
        })?;
    }
    let make = callable(context, ENTITY_JS)?;
    match context.call(&make, Value::Undefined, &[Value::Object(handler)])? {
        Value::Object(maker) => {
            context
                .as_callable(&maker)?
                .ok_or(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
                    "the entity maker",
                ))
        }
        _ => Err(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
            "the entity maker",
        )),
    }
}

/// The function a helper's source evaluates to.
pub(super) fn callable(
    context: &Context,
    source: &str,
) -> Result<CallableRef, quickjs_oxide::engine::api::core::RuntimeError> {
    match context.eval(source)? {
        Value::Object(object) => {
            context
                .as_callable(&object)?
                .ok_or(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
                    "a helper function",
                ))
        }
        _ => Err(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
            "a helper function",
        )),
    }
}

/// A bare `name` read: the object this run already made for it, else the stack's value converted. `context` and the
/// helper objects QuickJS answers by name fall back.
fn read_name(scope: &mut Scope<'_>, name: &str) -> Result<Value, Conv> {
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    if let Some(kept) = engine.kept_name(name) {
        return Ok(kept);
    }
    // In place, without a copy of the stack's value: the conversion runs no script and does not reach the stack.
    let found = STACK.with(|stack| {
        let stack = stack.borrow();
        stack.with_value(name, |value| {
            let keep = matches!(
                value,
                HostValue::List(_) | HostValue::Map(_) | HostValue::Decimal(_) | HostValue::Object(_)
            );
            to_js(scope, &engine, value).map(|converted| (converted, keep))
        })
    });
    match found {
        Some(converted) => {
            let (value, keep) = converted?;
            if keep {
                engine.keep_name(name, value.clone());
            }
            Ok(value)
        }
        None => missing(&engine, name),
    }
}

fn missing(engine: &OxideEngine, name: &str) -> Result<Value, Conv> {
    let _ = engine;
    match STACK.with(|stack| stack.borrow().get(name)) {
        Lookup::Context => Err(Conv::Unsupported("the stack as context")),
        Lookup::Value(_) => Err(Conv::Unsupported("a stack value read twice")),
        Lookup::Missing => {
            if crate::script::check::QUICKJS_ONLY_GLOBALS.contains(&name) {
                return Err(Conv::Unsupported("a QuickJS global"));
            }
            Ok(Value::Null)
        }
    }
}

/// A field of an entity value, as the QuickJS `HostEntity` reads it; anything else falls back.
fn read_field(scope: &mut Scope<'_>, cell: &Arc<EntityCell>, name: &str) -> Result<Value, Conv> {
    if ENTITY_METHODS.contains(&name) {
        return Err(Conv::Unsupported("an entity value method"));
    }
    let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
    match entity_field(cell, name) {
        EntityField::Value(value) => to_js(scope, &engine, &value),
        EntityField::Null => Ok(Value::Null),
        EntityField::Other => Err(Conv::Unsupported(
            "an entity relationship or a name that is not a field",
        )),
    }
}
