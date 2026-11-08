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

//! Entity values, lists and finds on quickjs-oxide, as the QuickJS bindings make them (`script/runtime/entities.rs`):
//! a value is an exotic object of its fields ([`super::super::names::EntityValues`]) and a list one of its rows and
//! `length` ([`ListValues`]), each with the host table's class (`script/host/entities.rs`) as its prototype, so their
//! methods are the table's on both engines; a find, an uploaded file and a stream are objects of the table's
//! classes.
//!
//! The engine's conversion calls [`object_to_js`] and [`object_to_host`]; the binder calls [`receiver_state`] for a
//! method's receiver.

use std::sync::Arc;

use moqui_lib::object::{HostObject, Value as HostValue};
use quickjs_oxide::engine::api::core::{
    CompleteOrdinaryPropertyDescriptor, ExoticClass, ExoticScope, Hook, Js, JsResult, ObjectRef,
    OrdinaryPropertyDescriptor, PropertyKey, Value,
};

use super::super::convert::{to_js, Conv};
use super::super::engine::{current, guarded_hook, OxideEngine};
use crate::entity::{EntityFind, EntityList};
use crate::script::host::entities::{self as host_entities, EntityCell, EntityField, FindState, ListState, ScriptList};
use crate::script::host::{conditions, error_info, upload};

/// A list's datum: its rows and this object's cursor.
pub(crate) struct ListTarget(pub(crate) Arc<ListState>);

/// The class of entity lists: `length`, the rows by index, and the table's methods on the prototype.
pub(crate) struct ListValues;

/// The index a property name stands for, when it is one of the list's.
fn index_of(list: &ListState, key: &PropertyKey) -> Option<usize> {
    // a key in an index's canonical form: digits, no leading zero (the engine makes such a key an index already)
    let name = key.to_js_string()?;
    let mut index: usize = 0;
    for (at, unit) in name.utf16_units().enumerate() {
        let digit = unit.checked_sub(u16::from(b'0')).filter(|digit| *digit < 10)?;
        if at > 0 && index == 0 {
            return None;
        }
        index = index.checked_mul(10)?.checked_add(usize::from(digit))?;
    }
    (!name.is_empty() && index < list.list.len()).then_some(index)
}

fn is_length(key: &PropertyKey) -> bool {
    super::super::convert::static_key("length").is_ok_and(|length| *key == length)
}

fn length(list: &ListState) -> Value {
    Value::Int(i32::try_from(list.list.len()).unwrap_or(i32::MAX))
}

/// A host value as a script value, from a hook.
fn hook_to_js(scope: &mut ExoticScope<'_>, value: HostValue) -> JsResult<Hook<Value>> {
    guarded_hook(scope, |scope| {
        let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
        to_js(scope, &engine, &value)
    })
}

impl ExoticClass for ListValues {
    type Data = ListTarget;

    fn get(&self, scope: &mut ExoticScope<'_>, data: &ListTarget, key: &PropertyKey) -> JsResult<Hook<Value>> {
        if is_length(key) {
            return Ok(Hook::Handled(length(&data.0)));
        }
        match index_of(&data.0, key) {
            Some(index) => hook_to_js(scope, data.0.item(index)),
            // the prototype's members (the table's methods, the shim's iteration and array methods), else undefined
            None => Ok(Hook::Ordinary),
        }
    }

    /// A list is read-only: an assignment is ignored, as on a frozen array. One whose receiver is another object (the
    /// list on its prototype chain, `Object.create(list)`) defines on that object, as on QuickJS.
    fn set(&self, scope: &mut ExoticScope<'_>, _: &ListTarget, _: &PropertyKey, _: Value) -> JsResult<Hook<bool>> {
        if !scope.receiver_is_this() {
            return Ok(Hook::Ordinary);
        }
        Ok(Hook::Handled(false))
    }

    fn define_own_property(
        &self,
        _: &mut ExoticScope<'_>,
        _: &ListTarget,
        _: &PropertyKey,
        _: &OrdinaryPropertyDescriptor,
    ) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(false))
    }

    fn delete(&self, _: &mut ExoticScope<'_>, _: &ListTarget, _: &PropertyKey) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(false))
    }

    fn has(&self, _: &mut ExoticScope<'_>, data: &ListTarget, key: &PropertyKey) -> JsResult<Hook<bool>> {
        if is_length(key) || index_of(&data.0, key).is_some() {
            return Ok(Hook::Handled(true));
        }
        Ok(Hook::Ordinary)
    }

    fn get_own_property(
        &self,
        scope: &mut ExoticScope<'_>,
        data: &ListTarget,
        key: &PropertyKey,
    ) -> JsResult<Hook<Option<CompleteOrdinaryPropertyDescriptor>>> {
        if is_length(key) {
            return Ok(Hook::Handled(Some(CompleteOrdinaryPropertyDescriptor::Data {
                value: length(&data.0),
                writable: false,
                enumerable: false,
                configurable: false,
            })));
        }
        let Some(index) = index_of(&data.0, key) else {
            return Ok(Hook::Handled(None));
        };
        let value = match hook_to_js(scope, data.0.item(index))? {
            Hook::Handled(value) => value,
            Hook::Ordinary => Value::Null,
        };
        Ok(Hook::Handled(Some(CompleteOrdinaryPropertyDescriptor::Data {
            value,
            writable: true,
            enumerable: true,
            configurable: false,
        })))
    }

    fn own_keys(&self, _: &mut ExoticScope<'_>, data: &ListTarget) -> JsResult<Hook<Vec<PropertyKey>>> {
        let count = u32::try_from(data.0.list.len()).unwrap_or(u32::MAX);
        Ok(Hook::Handled((0..count).map(PropertyKey::index).collect()))
    }
}

/// A host object the entity bindings make a script value of: a value, a list (of a find, or the entity layer's), or a
/// find kept in the context. `None` for another object.
pub(in crate::script::oxide) fn object_to_js<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    object: &HostObject,
) -> Result<Option<Value>, Conv> {
    if let Some(cell) = object.downcast::<EntityCell>() {
        return entity_to_js(js, engine, cell).map(Some);
    }
    if let Some(row) = object.downcast::<host_entities::ListRow>() {
        return Ok(Some(Value::Object(entity_object(js, engine, Arc::clone(&row.0))?)));
    }
    let list = if let Some(list) = object.downcast::<ScriptList>() {
        list
    } else if let Some(list) = object.downcast::<EntityList>() {
        Arc::new(ScriptList::of_list(EntityList::clone(&list)))
    } else if object.downcast::<crate::web::multipart::UploadedFile>().is_some() {
        // an upload or a stream passes through a script to a service as itself, as on QuickJS, with its methods
        return Ok(Some(Value::Object(super::class_instance(
            js,
            object.clone(),
            &upload::UPLOAD,
        )?)));
    } else if object.downcast::<moqui_db::sql::Condition>().is_some() {
        return Ok(Some(Value::Object(super::class_instance(
            js,
            object.clone(),
            &conditions::CONDITION,
        )?)));
    } else if object.downcast::<crate::web::error_info::ErrorInfo>().is_some() {
        return Ok(Some(Value::Object(super::class_instance(
            js,
            object.clone(),
            &error_info::ERROR_INFO,
        )?)));
    } else if object.downcast::<crate::script::host::StreamBytes>().is_some() {
        return Ok(Some(Value::Object(super::class_instance(
            js,
            object.clone(),
            &upload::STREAM,
        )?)));
    } else if let Some(find) = object.downcast::<EntityFind>() {
        let state = HostObject::new(FindState::of(EntityFind::clone(&find)));
        return Ok(Some(Value::Object(super::class_instance(
            js,
            state,
            &host_entities::FIND,
        )?)));
    } else {
        return Ok(None);
    };
    let prototype = engine.entity_prototype(js, &host_entities::LIST)?;
    let made = js.new_host_exotic(
        ListTarget(Arc::new(ListState::of(list))),
        &engine.protos.lists,
        Some(&prototype),
    )?;
    Ok(Some(Value::Object(made)))
}

/// An entity value: an exotic object over its cell, one per cell in a run, so a value read twice is the same object.
fn entity_to_js<J: Js>(js: &mut J, engine: &OxideEngine, cell: Arc<EntityCell>) -> Result<Value, Conv> {
    if let Some(proxy) = engine.entity_proxy(&cell) {
        return Ok(Value::Object(proxy));
    }
    let object = entity_object(js, engine, Arc::clone(&cell))?;
    engine.keep_entity(object.clone(), cell);
    Ok(Value::Object(object))
}

/// A new entity value object over `cell`, with the table's prototype.
fn entity_object<J: Js>(js: &mut J, engine: &OxideEngine, cell: Arc<EntityCell>) -> JsResult<ObjectRef> {
    let prototype = engine.entity_prototype(js, &host_entities::ENTITY_VALUE)?;
    js.new_host_exotic(
        super::super::names::EntityTarget(cell),
        &engine.protos.entities,
        Some(&prototype),
    )
}

/// [`crate::script::host::entities::entity_field`] of `name`, with the engine lent when the read may run a find or an
/// expression (a localized field, a `pq-expression` alias).
pub(crate) fn field(scope: &mut ExoticScope<'_>, cell: &Arc<EntityCell>, name: &str) -> JsResult<EntityField> {
    if !host_entities::entity_field_reads(cell, name) {
        return Ok(host_entities::entity_field(cell, name));
    }
    // a localized field the memos hold needs no find, so no lend
    if let Some(found) = host_entities::entity_field_memoized(cell, name) {
        return Ok(found);
    }
    super::release_before_lend(scope)?;
    let read = scope.lend(|| super::caught_panic(|| host_entities::entity_field(cell, name)));
    read.map_err(|message| super::plain_error(scope, &message))
}

/// The host value of an entity value, a list or a find; `None` for another object.
pub(crate) fn object_to_host<J: Js>(js: &mut J, object: &ObjectRef) -> JsResult<Option<HostValue>> {
    if let Some(cell) = super::super::names::entity_cell(js, object)? {
        return Ok(Some(HostValue::Object(HostObject(cell))));
    }
    if let Some(list) = js.host_data::<ListTarget>(object)? {
        return Ok(Some(HostValue::Object(HostObject(Arc::clone(&list.0.list) as _))));
    }
    // A find stays the find, so a later screen section can read the SQL it ran.
    if let Some(held) = js.host_data::<super::OxideHost>(object)? {
        // an object whose state went when its script ended is null here, so a value holding one (the end of a run's
        // write-back) still converts; a call of its methods, or one passing it as the object an argument takes, throws
        let Some(held) = held.obj() else {
            return Ok(Some(HostValue::Null));
        };
        if held.downcast::<crate::web::multipart::UploadedFile>().is_some()
            || held.downcast::<crate::script::host::StreamBytes>().is_some()
            || held.downcast::<moqui_db::sql::Condition>().is_some()
        {
            return Ok(Some(HostValue::Object(held)));
        }
        if let Some(find) = held.downcast::<FindState>().and_then(|state| state.peek()) {
            return Ok(Some(HostValue::Object(HostObject::new(find))));
        }
    }
    Ok(None)
}

/// The receiver state a table method reads from an entity value (its cell) or a list (its rows and cursor).
pub(crate) fn receiver_state<J: Js>(js: &mut J, this: &Value) -> JsResult<Option<HostObject>> {
    let Value::Object(object) = this else {
        return Ok(None);
    };
    if let Some(cell) = super::super::names::entity_cell(js, object)? {
        return Ok(Some(HostObject(cell)));
    }
    Ok(js
        .host_data::<ListTarget>(object)?
        .map(|list| HostObject(Arc::clone(&list.0) as _)))
}

/// Whether `name` is a member of the entity value prototype (a table method or one the shim adds): on QuickJS a method
/// wins over a field of the same name, so such a name reads from the prototype.
pub(crate) fn is_value_method(name: &str) -> bool {
    value_methods().contains(name)
}

/// The names of [`is_value_method`], made once.
fn value_methods() -> &'static std::collections::HashSet<&'static str, foldhash::fast::FixedState> {
    static NAMES: std::sync::OnceLock<std::collections::HashSet<&'static str, foldhash::fast::FixedState>> =
        std::sync::OnceLock::new();
    NAMES.get_or_init(|| {
        let class = &host_entities::ENTITY_VALUE;
        class
            .methods
            .iter()
            .map(|method| method.name)
            .chain(class.shim_members.iter().copied())
            .collect()
    })
}

/// The related value or list of a relationship name, read with the engine lent (a find may run nested scripts).
pub(crate) fn relationship(scope: &mut ExoticScope<'_>, cell: &Arc<EntityCell>, name: &str) -> JsResult<Hook<Value>> {
    super::release_before_lend(scope)?;
    let read = scope.lend(|| {
        super::caught_panic(|| host_entities::relationship_value(cell, name))
            .unwrap_or_else(|message| Err(moqui_lib::error::Error::new(message)))
    });
    match read {
        Ok(Some(value)) => hook_to_js(scope, value),
        Ok(None) => Ok(Hook::Handled(Value::Undefined)),
        // thrown as the binder throws a library error: its diagnostic, kind and retry seconds marked
        Err(err) => Err(super::throw(scope, crate::script::host::HostError::Lib(err), None)),
    }
}

#[cfg(test)]
#[path = "entities_tests.rs"]
mod tests;
