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

//! Bare names and entity fields on quickjs-oxide, as host exotic objects (the fork's F2;
//! plans/archive/Script-Engine-Oxide.md section 5.2), the same hooks the QuickJS bindings' `HostGlobalNames` and `HostEntity`
//! are.
//!
//! The global object's prototype is a [`GlobalNames`] object: a name the global object does not have reaches its `has`
//! (every string name is there, as on QuickJS, so an unset name reads as null) and its `get`, which reads the context
//! stack as QuickJS's `read_name` does; a bare assignment reaches its `set` ([`super::tracked`]).
//!
//! An entity value is an [`EntityValues`] object over its cell: a field reads as the QuickJS `HostEntity` reads it
//! (localized, a view's alias, null for a field it does not hold), a field set writes the cell (an assignment whose
//! receiver is another object, the value on its prototype chain, defines on that object), `in`, `Object.keys` and own
//! property reads see the fields, a relationship reads the related value or list (the engine lent to the find), and
//! other names are those of its prototype: the host table's entity value class (`oxide/entities.rs`), whose methods
//! win over a field of the same name, as on QuickJS.

use std::sync::Arc;

use moqui_lib::context_stack::Lookup;
use quickjs_oxide::engine::api::core::{
    CompleteOrdinaryPropertyDescriptor, Context, ExoticClass, ExoticScope, Hook, HostClass, JsResult, ObjectRef,
    PropertyKey, RuntimeError, Value,
};

use super::convert::{to_host, to_js, Conv};
use super::engine::{current, guarded, guarded_hook};
use crate::script::run::STACK;
use crate::script::runtime::{
    entity_field_names, entity_is_field, entity_set_field, EntityCell, EntityField,
};

/// The names the entity value's prototype has (its methods, the host table's on both engines): a method wins over a
/// field of the same name. A test keeps the list in step with QuickJS.
#[allow(dead_code)] // a test keeps it in step; the route's read check (reads.rs) names it
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

/// The class of the global object's prototype: bare names over the context stack.
pub(super) struct GlobalNames;

impl ExoticClass for GlobalNames {
    type Data = ();

    fn has(&self, _: &mut ExoticScope<'_>, _: &(), key: &PropertyKey) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(key.as_symbol().is_none()))
    }

    fn get(&self, scope: &mut ExoticScope<'_>, _: &(), key: &PropertyKey) -> JsResult<Hook<Value>> {
        let Some(name) = key.to_js_string() else {
            return Ok(Hook::Handled(Value::Undefined));
        };
        let name = super::convert::text_of(&name);
        crate::telemetry::profile::count("name.read");
        guarded_hook(scope, |scope| super::tracked::read_name(scope, &name))
    }

    fn set(&self, scope: &mut ExoticScope<'_>, _: &(), key: &PropertyKey, value: Value) -> JsResult<Hook<bool>> {
        let Some(name) = key.to_js_string() else {
            return Ok(Hook::Handled(true));
        };
        let name = super::convert::text_of(&name);
        crate::telemetry::profile::count("name.write");
        guarded_hook(scope, |scope| {
            super::tracked::write_name(scope, &name, &value)?;
            Ok(true)
        })
    }

    fn delete(&self, _: &mut ExoticScope<'_>, _: &(), _: &PropertyKey) -> JsResult<Hook<bool>> {
        // as QuickJS's exotic answers
        Ok(Hook::Handled(true))
    }

    fn get_own_property(
        &self,
        _: &mut ExoticScope<'_>,
        _: &(),
        _: &PropertyKey,
    ) -> JsResult<Hook<Option<CompleteOrdinaryPropertyDescriptor>>> {
        Ok(Hook::Handled(None))
    }

    fn own_keys(&self, _: &mut ExoticScope<'_>, _: &()) -> JsResult<Hook<Vec<PropertyKey>>> {
        Ok(Hook::Handled(Vec::new()))
    }
}

/// Make the global object's prototype a [`GlobalNames`] object.
pub(super) fn install_names(context: &Context) -> Result<(), RuntimeError> {
    let class = context.register_exotic_class(GlobalNames)?;
    let names = context.new_host_exotic((), &class, None)?;
    let global = context.global_object()?;
    context.set_prototype_of(&global, Some(&names))?;
    Ok(())
}

/// An entity value's datum: the cell the context stack shares.
pub(super) struct EntityTarget(pub(super) Arc<EntityCell>);

/// The class of entity values.
pub(super) struct EntityValues;

impl ExoticClass for EntityValues {
    type Data = EntityTarget;

    fn get(&self, scope: &mut ExoticScope<'_>, data: &EntityTarget, key: &PropertyKey) -> JsResult<Hook<Value>> {
        let Some(name) = key.to_js_string() else {
            return Ok(Hook::Ordinary);
        };
        let name = super::convert::text_of(&name);
        // a method of the prototype (the host table's) wins over a field of the same name, as on QuickJS
        if super::bind::entities::is_value_method(&name) {
            return Ok(Hook::Ordinary);
        }
        crate::telemetry::profile::count("field.read");
        let cell = Arc::clone(&data.0);
        match super::bind::entities::field(scope, &cell, &name)? {
            EntityField::Value(value) => guarded_hook(scope, |scope| {
                let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
                to_js(scope, &engine, &value)
            }),
            EntityField::Null => Ok(Hook::Handled(Value::Null)),
            EntityField::Relationship => super::bind::entities::relationship(scope, &cell, &name),
            // `Object.prototype`'s members, else undefined, as QuickJS reads such a name
            EntityField::Unknown => Ok(Hook::Ordinary),
        }
    }

    fn set(
        &self,
        scope: &mut ExoticScope<'_>,
        data: &EntityTarget,
        key: &PropertyKey,
        value: Value,
    ) -> JsResult<Hook<bool>> {
        // An assignment whose receiver is another object (this value on its prototype chain) defines on that object.
        if !scope.receiver_is_this() {
            return Ok(Hook::Ordinary);
        }
        let Some(name) = key.to_js_string() else {
            return guarded_hook(scope, |_| {
                Err(Conv::Error("an entity value field name is not a symbol".to_string()))
            });
        };
        let name = name.to_utf8_lossy();
        crate::telemetry::profile::count("field.write");
        let cell = Arc::clone(&data.0);
        guarded_hook(scope, |scope| {
            if super::engine::fallback_pending() {
                return Err(Conv::Unsupported("a run that fell back"));
            }
            let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
            let host = to_host(scope, &engine, &value)?;
            entity_set_field(&cell, name, host).map_err(Conv::Error)?;
            Ok(true)
        })
    }

    fn has(&self, _: &mut ExoticScope<'_>, data: &EntityTarget, key: &PropertyKey) -> JsResult<Hook<bool>> {
        let Some(name) = key.to_js_string() else {
            return Ok(Hook::Ordinary);
        };
        let name = super::convert::text_of(&name);
        if super::bind::entities::is_value_method(&name) {
            return Ok(Hook::Ordinary);
        }
        if entity_is_field(&data.0, &name) {
            return Ok(Hook::Handled(true));
        }
        Ok(Hook::Ordinary)
    }

    /// A field is not removed from a value, as Java `EntityValueBase` has no remove.
    fn delete(&self, _: &mut ExoticScope<'_>, _: &EntityTarget, _: &PropertyKey) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(false))
    }

    fn get_own_property(
        &self,
        scope: &mut ExoticScope<'_>,
        data: &EntityTarget,
        key: &PropertyKey,
    ) -> JsResult<Hook<Option<CompleteOrdinaryPropertyDescriptor>>> {
        let Some(name) = key.to_js_string() else {
            return Ok(Hook::Handled(None));
        };
        let name = super::convert::text_of(&name);
        if !entity_is_field(&data.0, &name) {
            return Ok(Hook::Handled(None));
        }
        let held = match super::bind::entities::field(scope, &data.0, &name)? {
            EntityField::Value(value) => value,
            _ => moqui_lib::object::Value::Null,
        };
        let value = guarded(scope, |scope| {
            let engine = current().ok_or(Conv::Unsupported("no oxide engine"))?;
            to_js(scope, &engine, &held)
        })?;
        Ok(Hook::Handled(Some(CompleteOrdinaryPropertyDescriptor::Data {
            value,
            writable: true,
            enumerable: true,
            configurable: true,
        })))
    }

    /// The fields the value holds, so `Object.keys`, `for...in`, a spread and `JSON.stringify` see the field map.
    fn own_keys(&self, _: &mut ExoticScope<'_>, data: &EntityTarget) -> JsResult<Hook<Vec<PropertyKey>>> {
        let mut keys = Vec::new();
        for name in entity_field_names(&data.0) {
            keys.push(PropertyKey::from_js_string(
                quickjs_oxide::engine::api::core::JsString::try_from_utf8(&name)?,
            ));
        }
        Ok(Hook::Handled(keys))
    }
}

/// A host object the oxide bindings pass through as itself but do not bind yet (an upload, a stream, an entity list,
/// a tools map held as its host value): any read of a member falls back to QuickJS, so a script that only hands it on
/// to a host call runs on oxide.
pub(super) struct OpaqueTarget(pub(super) moqui_lib::object::HostObject);

pub(super) struct OpaqueValues;

impl ExoticClass for OpaqueValues {
    type Data = OpaqueTarget;

    fn get(&self, scope: &mut ExoticScope<'_>, _: &OpaqueTarget, key: &PropertyKey) -> JsResult<Hook<Value>> {
        if key.as_symbol().is_some() {
            return Ok(Hook::Ordinary);
        }
        guarded_hook(scope, |_| Err(Conv::Unsupported("a member of a host object the oxide bindings do not have yet")))
    }

    fn has(&self, scope: &mut ExoticScope<'_>, _: &OpaqueTarget, _: &PropertyKey) -> JsResult<Hook<bool>> {
        guarded_hook(scope, |_| Err(Conv::Unsupported("a member of a host object the oxide bindings do not have yet")))
    }

    fn own_keys(&self, scope: &mut ExoticScope<'_>, _: &OpaqueTarget) -> JsResult<Hook<Vec<PropertyKey>>> {
        guarded_hook(scope, |_| Err(Conv::Unsupported("a member of a host object the oxide bindings do not have yet")))
    }
}

/// The entity value class of a runtime, registered when it starts.
pub(super) fn register_entities(context: &Context) -> Result<HostClass<EntityValues>, RuntimeError> {
    context.register_exotic_class(EntityValues)
}

/// The cell of an entity value object, or `None` for any other object.
pub(super) fn entity_cell<J: quickjs_oxide::engine::api::core::Js>(
    js: &mut J,
    object: &ObjectRef,
) -> JsResult<Option<Arc<EntityCell>>> {
    Ok(js
        .host_data::<EntityTarget>(object)?
        .map(|target| Arc::clone(&target.0)))
}

/// The missing name `name`: `context` is the stack object ([`super::context`]), a QuickJS global falls back, any
/// other is null.
pub(super) fn missing(name: &str) -> Result<Value, Conv> {
    match STACK.with(|stack| stack.borrow().get(name)) {
        Lookup::Context => Ok(Value::Object(
            current()
                .ok_or(Conv::Unsupported("no oxide engine"))?
                .protos
                .context
                .clone(),
        )),
        Lookup::Value(_) => Err(Conv::Unsupported("a stack value read twice")),
        Lookup::Missing => {
            // the helper objects QuickJS's bare-name object answers itself, as it answers them
            if let Some(engine) = current() {
                if let Some((_, helper)) = engine.protos.helper_objects.iter().find(|(helper, _)| *helper == name) {
                    return Ok(Value::Object(helper.clone()));
                }
            }
            if QUICKJS_ONLY_GLOBALS.contains(&name) {
                return Err(Conv::Unsupported("a QuickJS global"));
            }
            Ok(Value::Null)
        }
    }
}

/// Global names a script written for QuickJS may use that quickjs-oxide does not bind: a bare one is an error naming
/// it, not a context name that is not set.
pub(crate) const QUICKJS_ONLY_GLOBALS: &[&str] = &[
    // QuickJS builtins quickjs-oxide does not have (the fork has explicit resource management's own)
    "DOMException",
    "atob",
    "btoa",
    "performance",
    "queueMicrotask",
    // the host's globals (`install_ec`, `install_helpers`, `tools_host`, `edi`, `sftp`)
    "EdiHandler",
    "ResourceReference",
    "SftpClient",
    "System",
    "__compareOrder",
    "__inDateRange",
    "__moquiWrap",
    "compare",
    "elasticQueryAutoWildcard",
    "filterMapList",
    "filterMapListByDate",
    "numberToWords",
    "numberToWordsWithDecimal",
    "orderMapList",
    "sri",
    // names the QuickJS bare-name object answers itself (`read_name`'s helper objects, the stack as `context`)
    "ObjectUtilities",
    "CollectionUtilities",
    "StringUtilities",
    "WebUtilities",
    "UserSecurityChecks",
];
