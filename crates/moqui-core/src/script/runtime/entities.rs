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

//! Entity lists and values as QuickJS scripts see them: exotic objects whose rows and fields read as properties, with
//! the host table's classes (`script/host/entities.rs`) as their prototypes. `ec.entity`'s functions and the find
//! builder are the table's.

use rquickjs::class::{PropertyDescriptor, PropertyName};
use rquickjs::Atom;

use super::*;
use crate::script::host::entities::{self as host_entities, ListState};
pub(crate) use crate::script::host::entities::{
    entity_field_names, entity_is_field, entity_set_field, EntityCell, EntityField, ScriptList,
};

/// An entity value as a script sees it.
pub(super) fn entity_to_js<'js>(ctx: &Ctx<'js>, host: HostEntity) -> rquickjs::Result<JsValue<'js>> {
    Class::instance(ctx.clone(), host).and_then(|class| class.into_js(ctx))
}

/// A find builder over `find`, an object of the host table's class.
pub(super) fn wrap_find<'js>(ctx: &Ctx<'js>, find: EntityFind) -> rquickjs::Result<JsValue<'js>> {
    bind::running_instance(
        ctx,
        HostObject::new(host_entities::FindState::of(find)),
        &host_entities::FIND,
    )
    .map(Object::into_value)
}

/// The exotic methods `#[rquickjs::class(exotic)]` calls, in the module it names `__impl_exotic_<Class>__`, written
/// here rather than by `#[rquickjs::exotic]` because that macro drops the receiver of a set. A set whose receiver is
/// another object, as `Object.create(value).x = 1` reaches the value through the prototype chain, defines the
/// property on the receiver, as an ordinary object's set does, rather than writing the field. Re-check when rquickjs
/// is upgraded.
macro_rules! exotic_module {
    ($module:ident, $class:ident) => {
        #[allow(non_snake_case)]
        mod $module {
            use super::*;
            use rquickjs::class::JsCell;

            pub(crate) struct ExoticImpl;

            impl ExoticImpl {
                pub fn exotic_get_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                    _receiver: JsValue<'js>,
                ) -> rquickjs::Result<JsValue<'js>> {
                    this.borrow().property(ctx, atom)
                }

                pub fn exotic_set_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                    receiver: JsValue<'js>,
                    value: JsValue<'js>,
                ) -> rquickjs::Result<bool> {
                    if is_receiver(this, &receiver) {
                        this.borrow().assign(ctx, atom, value)
                    } else {
                        define_on_receiver(atom, &receiver, value)
                    }
                }

                pub fn exotic_delete_property<'js>(
                    this: &JsCell<'js, $class>,
                    _ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                ) -> rquickjs::Result<bool> {
                    Ok(this.borrow().remove(atom))
                }

                pub fn exotic_has_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                ) -> rquickjs::Result<bool> {
                    this.borrow().has_member(ctx, atom)
                }

                pub fn exotic_get_own_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                ) -> rquickjs::Result<Option<PropertyDescriptor<'js>>> {
                    this.borrow().own_property(ctx, atom)
                }

                pub fn exotic_get_own_property_names<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                ) -> rquickjs::Result<Vec<PropertyName<'js>>> {
                    this.borrow().own_names(ctx)
                }
            }
        }
    };
}

/// True when a set's receiver is the exotic object itself, not an object that has it on its prototype chain.
fn is_receiver<'js, C: rquickjs::class::JsClass<'js>>(
    this: &rquickjs::class::JsCell<'js, C>,
    receiver: &JsValue<'js>,
) -> bool {
    receiver
        .as_object()
        .and_then(Class::<C>::from_object)
        .is_some_and(|class| std::ptr::eq(class.get_cell(), this))
}

/// The ordinary set of a property that is not on the receiver: a new own data property of the receiver. A receiver
/// that is not an object takes no property.
fn define_on_receiver<'js>(atom: Atom<'js>, receiver: &JsValue<'js>, value: JsValue<'js>) -> rquickjs::Result<bool> {
    let result = match receiver.as_object() {
        Some(object) => object
            .prop(
                atom,
                rquickjs::object::Property::from(value.clone())
                    .writable()
                    .enumerable()
                    .configurable(),
            )
            .map(|()| true),
        None => Ok(false),
    };
    keep_assigned(value);
    result
}

/// A find result. `length`, an index and `get` are what iterate and inline scripts use, and it is iterable as a
/// Java `List` is to Groovy. Its methods are the host table's ([`install_entity_prototypes`]).
#[derive(Trace, JsLifetime)]
#[rquickjs::class(exotic)]
pub(super) struct HostList {
    /// The rows, and this object's own `EntityListIterator` cursor.
    #[qjs(skip_trace)]
    pub(super) state: Arc<ListState>,
}

impl HostList {
    pub(super) fn of_list(list: EntityList) -> Self {
        Self::of_shared(Arc::new(ScriptList::of_list(list)))
    }

    /// A list over rows another list holds too.
    pub(super) fn of_shared(inner: Arc<ScriptList>) -> Self {
        Self {
            state: Arc::new(ListState::of(inner)),
        }
    }

    /// The rows as a host value.
    pub(super) fn rows(&self) -> HostObject {
        HostObject(Arc::clone(&self.state.list) as _)
    }

    /// The receiver state a host table method reads.
    pub(super) fn host_state(&self) -> HostObject {
        HostObject(Arc::clone(&self.state) as _)
    }

    fn length(&self, ctx: &Ctx<'_>) -> rquickjs::Result<i32> {
        i32::try_from(self.state.list.len()).map_err(|_| Exception::throw_message(ctx, "list is too long"))
    }

    /// The position a property name stands for, when it is an index in the list.
    fn index_of(&self, name: &str) -> Option<usize> {
        let index: usize = name.parse().ok()?;
        (index < self.state.list.len() && index.to_string() == name).then_some(index)
    }

    /// The row itself, not a copy: a field set on it is kept in the list.
    fn item<'js>(&self, ctx: &Ctx<'js>, index: usize) -> rquickjs::Result<JsValue<'js>> {
        match self.state.list.cells().get(index) {
            Some(cell) => entity_to_js(ctx, HostEntity::of(Arc::clone(cell))),
            None => Null.into_js(ctx),
        }
    }
}

exotic_module!(__impl_exotic_HostList__, HostList);

/// Property access on a list: a prototype method first, then `length` and the index of a value.
impl HostList {
    fn property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(ctx, || {
            if let Some(found) = prototype_member::<HostList>(ctx, &atom)? {
                return Ok(found);
            }
            let Some(name) = atom_name(&atom)? else {
                return Undefined.into_js(ctx);
            };
            if name == "length" {
                return self.length(ctx)?.into_js(ctx);
            }
            match self.index_of(&name) {
                Some(index) => self.item(ctx, index),
                None => Undefined.into_js(ctx),
            }
        })
    }

    /// A list is read-only: an assignment is ignored, as on a frozen array.
    fn assign<'js>(&self, _ctx: &Ctx<'js>, _atom: Atom<'js>, value: JsValue<'js>) -> Result<bool, JsError> {
        keep_assigned(value);
        Ok(false)
    }

    fn remove<'js>(&self, _atom: Atom<'js>) -> bool {
        false
    }

    fn has_member<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<bool, JsError> {
        if prototype_has::<HostList>(ctx, &atom)? {
            return Ok(true);
        }
        Ok(atom_name(&atom)?.is_some_and(|name| name == "length" || self.index_of(&name).is_some()))
    }

    fn own_property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<Option<PropertyDescriptor<'js>>, JsError> {
        let Some(name) = atom_name(&atom)? else {
            return Ok(None);
        };
        if name == "length" {
            let length = self.length(ctx)?.into_js(ctx)?;
            return Ok(Some(PropertyDescriptor::new_value(length, false, false, false)));
        }
        match self.index_of(&name) {
            Some(index) => Ok(Some(PropertyDescriptor::new_value(
                self.item(ctx, index)?,
                true,
                true,
                false,
            ))),
            None => Ok(None),
        }
    }

    fn own_names<'js>(&self, ctx: &Ctx<'js>) -> Result<Vec<PropertyName<'js>>, JsError> {
        (0..self.state.list.len())
            .map(|index| {
                let index = u32::try_from(index).map_err(|_| Exception::throw_message(ctx, "list is too long"))?;
                Ok(PropertyName {
                    atom: Atom::from_u32(ctx.clone(), index)?,
                    is_enumerable: true,
                })
            })
            .collect()
    }
}

/// The class's prototype, which [`install_entity_prototypes`] fills with the host table's methods.
#[rquickjs::methods]
impl HostList {}

/// One entity value. Field storage stays on this object, not on the context stack.
///
/// Fields read and write as properties too, as Groovy's map property syntax does over Java `EntityValueBase`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(exotic)]
pub(super) struct HostEntity {
    #[qjs(skip_trace)]
    pub(super) inner: Arc<EntityCell>,
}

impl HostEntity {
    pub(super) fn of(inner: Arc<EntityCell>) -> Self {
        Self { inner }
    }

    fn set_field(&self, ctx: &Ctx<'_>, field: String, value: &JsValue<'_>) -> rquickjs::Result<()> {
        let host = js_to_host(ctx, value)?;
        entity_set_field(&self.inner, field, host).map_err(|message| Exception::throw_message(ctx, &message))
    }

    /// A field value, null for a field the value does not hold, or the related value or list for a relationship
    /// name, as Java `EntityValueBase.get` resolves a name. Undefined for any other name.
    fn read_member<'js>(&self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<JsValue<'js>> {
        match host_entities::read_member(&self.inner, name).map_err(|err| throw_lib(ctx, &err))? {
            Some(value) => host_to_js(ctx, value),
            None => Undefined.into_js(ctx),
        }
    }
}

exotic_module!(__impl_exotic_HostEntity__, HostEntity);

/// Property access on an entity value. A prototype method wins over a field of the same name; a field name reads and
/// writes the field; a relationship name reads the related value, or list for a `many` relationship. Any other name
/// reads as undefined, where Java throws, because the engine and scripts look up names a value does not have
/// (`toJSON`, `then`, `Symbol.toPrimitive`); writing one is an error, as in Java.
impl HostEntity {
    fn property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(ctx, || {
            if let Some(found) = prototype_member::<HostEntity>(ctx, &atom)? {
                return Ok(found);
            }
            match atom_name(&atom)? {
                Some(name) => self.read_member(ctx, &name),
                None => Undefined.into_js(ctx),
            }
        })
    }

    fn assign<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>, value: JsValue<'js>) -> Result<bool, JsError> {
        let result = host_guard(ctx, || {
            let Some(name) = atom_name(&atom)? else {
                return Err(Exception::throw_message(
                    ctx,
                    "an entity value field name is not a symbol",
                ));
            };
            self.set_field(ctx, name.to_string(), &value)?;
            Ok(true)
        });
        keep_assigned(value);
        result
    }

    /// A field is not removed from a value, as Java `EntityValueBase` has no remove.
    fn remove<'js>(&self, _atom: Atom<'js>) -> bool {
        false
    }

    /// `name in value`: a prototype member or a field of the entity.
    fn has_member<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<bool, JsError> {
        if prototype_has::<HostEntity>(ctx, &atom)? {
            return Ok(true);
        }
        Ok(atom_name(&atom)?.is_some_and(|name| entity_is_field(&self.inner, &name)))
    }

    fn own_property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<Option<PropertyDescriptor<'js>>, JsError> {
        let Some(name) = atom_name(&atom)? else {
            return Ok(None);
        };
        if !entity_is_field(&self.inner, &name) {
            return Ok(None);
        }
        let held = host_entities::field_value(&self.inner, &name).unwrap_or(Value::Null);
        let value = host_to_js(ctx, held)?;
        Ok(Some(PropertyDescriptor::new_value(value, true, true, true)))
    }

    /// The fields the value holds, so `Object.keys`, `for...in`, spread and `JSON.stringify` see the field map.
    fn own_names<'js>(&self, ctx: &Ctx<'js>) -> Result<Vec<PropertyName<'js>>, JsError> {
        entity_field_names(&self.inner)
            .iter()
            .map(|name| {
                Ok(PropertyName {
                    atom: Atom::from_str(ctx.clone(), name)?,
                    is_enumerable: true,
                })
            })
            .collect()
    }
}

/// The class's prototype, which [`install_entity_prototypes`] fills with the host table's methods.
#[rquickjs::methods]
impl HostEntity {}

/// A member of the class prototype or of `Object.prototype`. An exotic class's own get runs instead of the
/// prototype lookup, so it looks there itself first.
pub(super) fn prototype_member<'js, C: rquickjs::class::JsClass<'js>>(
    ctx: &Ctx<'js>,
    atom: &Atom<'js>,
) -> rquickjs::Result<Option<JsValue<'js>>> {
    let Some(prototype) = Class::<C>::prototype(ctx)? else {
        return Ok(None);
    };
    let found: JsValue = prototype.get(atom.clone())?;
    Ok((!found.is_undefined()).then_some(found))
}

pub(super) fn prototype_has<'js, C: rquickjs::class::JsClass<'js>>(
    ctx: &Ctx<'js>,
    atom: &Atom<'js>,
) -> Result<bool, JsError> {
    match Class::<C>::prototype(ctx)? {
        Some(prototype) => prototype.contains_key(atom.clone()),
        None => Ok(false),
    }
}

/// Hand back the reference to an assigned value that the exotic set took. rquickjs 0.14 wraps the value QuickJS
/// passes to `set_property` as an owned value, and QuickJS frees it again after the call, so without this each
/// assignment would free the value twice. Re-check when rquickjs is upgraded: once the wrapper borrows the value,
/// this would keep one reference too many (a leak, not a crash). The value is not forgotten whole: it holds a
/// reference to the context too.
pub(super) fn keep_assigned(value: JsValue<'_>) {
    // SAFETY: the context and the value are live for this call; this adds one reference to the value, which the
    // free QuickJS does after the exotic set takes back.
    unsafe {
        rquickjs::qjs::JS_DupValue(value.ctx().as_raw().as_ptr(), value.as_raw());
    }
}

/// The text of a property name, or None for a symbol.
pub(super) fn atom_name<'js>(atom: &Atom<'js>) -> rquickjs::Result<Option<super::AtomText<'js>>> {
    super::AtomText::of(&atom.to_value()?)
}

/// Fill the entity list and value prototypes with the host table's classes (`script/host/entities.rs`): their methods,
/// and what their shims add. Lists and values made after this share them.
pub(super) fn install_entity_prototypes(ctx: &Ctx<'_>) -> Result<(), Error> {
    let list = caught(ctx, "entity list prototype", Class::<HostList>::prototype(ctx))?
        .ok_or_else(|| Error::new("entity list has no prototype"))?;
    bind::fill_prototype(ctx, &list, &host_entities::LIST)?;
    let value = caught(ctx, "entity value prototype", Class::<HostEntity>::prototype(ctx))?
        .ok_or_else(|| Error::new("entity value has no prototype"))?;
    bind::fill_prototype(ctx, &value, &host_entities::ENTITY_VALUE)
}

#[cfg(test)]
#[path = "entities_tests.rs"]
mod tests;
