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

//! Conversion between context-stack values and quickjs-oxide values, as the QuickJS bindings convert
//! (`script/runtime/convert.rs`): null, booleans, numbers (an `I64` an integer where it fits, an error past the safe
//! range), text, timestamps and dates as their text, lists and maps converted on read, a Set as the stack holds one,
//! bytes as a `Uint8Array`, decimals and entity values as host objects. A host object the oxide bindings do not have
//! yet (a find, an entity list, an upload) and a tools map pass through as opaque objects whose members fall back; a
//! Set from the stack is [`Conv::Unsupported`]: the run falls back to QuickJS.


use moqui_lib::object::{HostObject, Value as HostValue};
use quickjs_oxide::engine::api::core::{Js, JsString, ObjectRef, PropertyKey, RuntimeError, TypedArrayKind, Value};

use super::decimal::OxDecimal;
use super::engine::OxideEngine;
use crate::script::runtime::{SAFE_INTEGER, SET_KEY, TOO_DEEP, WRAP_KEY};

/// How deep a host value may nest when it becomes a script value, and a script value when it becomes a host value
/// (the QuickJS bindings' limits).
const MAX_HOST_NESTING: usize = crate::script::runtime::MAX_NESTING + 56;
const MAX_NESTING: usize = crate::script::runtime::MAX_NESTING;

/// Why a conversion did not give a value.
pub(super) enum Conv {
    /// The engine failed or a script it ran threw.
    Js(RuntimeError),
    /// Something the oxide bindings do not have yet: the run falls back to QuickJS.
    Unsupported(&'static str),
    /// A value that does not convert, with the QuickJS bindings' message.
    Error(String),
}

impl From<RuntimeError> for Conv {
    fn from(err: RuntimeError) -> Self {
        Conv::Js(err)
    }
}

impl From<quickjs_oxide::engine::api::core::JsStringError> for Conv {
    fn from(err: quickjs_oxide::engine::api::core::JsStringError) -> Self {
        Conv::Js(RuntimeError::from(err))
    }
}

/// A property key for `name`.
pub(super) fn key(name: &str) -> Result<PropertyKey, Conv> {
    Ok(PropertyKey::from_js_string(JsString::try_from_utf8(name)?))
}

/// The property key of a literal name the bindings read often (`length`, a node literal's fields), made once per
/// thread: a key is the engine API's own string, not a runtime's.
pub(super) fn static_key(name: &'static str) -> Result<PropertyKey, quickjs_oxide::engine::api::core::JsStringError> {
    thread_local! {
        static KEYS: std::cell::RefCell<Vec<(&'static str, PropertyKey)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    KEYS.with(|keys| {
        if let Some((_, found)) = keys.borrow().iter().find(|(kept, _)| *kept == name) {
            return Ok(found.clone());
        }
        let made = PropertyKey::from_js_string(JsString::try_from_utf8(name)?);
        keys.borrow_mut().push((name, made.clone()));
        Ok(made)
    })
}

/// Whether `text` is `name`, compared by code unit with no copy.
pub(super) fn text_is(text: &JsString, name: &str) -> bool {
    text.utf16_units().eq(name.encode_utf16())
}

/// The text of a string, for a bare name or a field name an exotic hook reads: a short one without an allocation.
pub(super) struct KeyText {
    inline: [u8; 64],
    len: usize,
    heap: Option<String>,
}

impl std::ops::Deref for KeyText {
    type Target = str;

    fn deref(&self) -> &str {
        match &self.heap {
            Some(text) => text,
            None => std::str::from_utf8(&self.inline[..self.len]).unwrap_or_default(),
        }
    }
}

/// [`KeyText`] of `text`, as `to_utf8_lossy` gives it.
pub(super) fn text_of(text: &JsString) -> KeyText {
    let mut out = KeyText {
        inline: [0; 64],
        len: 0,
        heap: None,
    };
    for decoded in char::decode_utf16(text.utf16_units()) {
        let ch = decoded.unwrap_or(char::REPLACEMENT_CHARACTER);
        if let Some(heap) = &mut out.heap {
            heap.push(ch);
            continue;
        }
        let width = ch.len_utf8();
        if out.len + width <= out.inline.len() {
            ch.encode_utf8(&mut out.inline[out.len..]);
            out.len += width;
        } else {
            let mut heap = String::with_capacity(out.len * 2 + width);
            heap.push_str(std::str::from_utf8(&out.inline[..out.len]).unwrap_or_default());
            heap.push(ch);
            out.heap = Some(heap);
        }
    }
    out
}

pub(super) fn text(value: &str) -> Result<Value, Conv> {
    Ok(Value::String(JsString::try_from_utf8(value)?))
}

/// A host value as an oxide value.
pub(super) fn to_js<J: Js>(js: &mut J, engine: &OxideEngine, value: &HostValue) -> Result<Value, Conv> {
    to_js_at(js, engine, value, 0)
}

fn to_js_at<J: Js>(js: &mut J, engine: &OxideEngine, value: &HostValue, depth: usize) -> Result<Value, Conv> {
    Ok(match value {
        HostValue::Null => Value::Null,
        HostValue::Bool(flag) => Value::Bool(*flag),
        HostValue::String(text) => Value::String(JsString::try_from_utf8(text)?),
        HostValue::I32(number) => Value::Int(*number),
        HostValue::I64(number) => match i32::try_from(*number) {
            Ok(small) => Value::Int(small),
            Err(_) if number.unsigned_abs() > SAFE_INTEGER.unsigned_abs() => {
                return Err(Conv::Error(
                    "integer is too large for a script number; it would lose precision".to_string(),
                ))
            }
            #[allow(clippy::cast_precision_loss)]
            Err(_) => Value::Float(*number as f64),
        },
        HostValue::F64(number) => Value::Float(*number),
        HostValue::Decimal(_) => {
            Value::Object(js.new_host_object(OxDecimal { inner: value.clone() }, Some(&engine.protos.decimal))?)
        }
        HostValue::Timestamp(_) | HostValue::Date(_) | HostValue::Time(_) => {
            text(&crate::script::runtime::temporal_text(value))?
        }
        HostValue::List(items) => {
            if depth >= MAX_HOST_NESTING {
                return Err(Conv::Error(TOO_DEEP.to_string()));
            }
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(to_js_at(js, engine, item, depth + 1)?);
            }
            Value::Object(js.new_array_from_values(values)?)
        }
        HostValue::Map(entries) => {
            if depth >= MAX_HOST_NESTING {
                return Err(Conv::Error(TOO_DEEP.to_string()));
            }
            // A tools object keeps its data on the stack and gets its methods back from the tools shim's wrapper, as on
            // QuickJS (`runtime/convert.rs`): its entries converted, then `__moquiWrap(name, object)`.
            if let (Some(HostValue::String(name)), Some(wrap)) = (entries.get(WRAP_KEY), engine.tools_wrap()) {
                let object = js.new_object()?;
                for (key, item) in entries {
                    let item = to_js_at(js, engine, item, depth + 1)?;
                    js.set_property(&object, &engine.key(key)?, item)?;
                }
                return Ok(js.call(wrap, Value::Undefined, &[text(name)?, Value::Object(object)])?);
            }
            // a `__wrap` that is not a name, or a runtime still starting: it passes through as itself, and a read of
            // its members falls back
            if entries.contains_key(WRAP_KEY) {
                return Ok(Value::Object(js.new_host_exotic(
                    super::names::OpaqueTarget(HostObject::new(value.clone())),
                    &engine.protos.opaque,
                    Some(&engine.protos.object_prototype),
                )?));
            }
            // A Set the stack holds stays QuickJS's for now: its `add` and `delete` change it in place, and an oxide
            // copy would not be written back (the static check lets `add` through, for decimals).
            if entries.len() == 1 && entries.contains_key(SET_KEY) {
                // under `oxide` a Set read by name is tracked as a list or map is, and written back at a flush
                if !engine.whole() {
                    return Err(Conv::Unsupported("a Set from the stack"));
                }
                let items = match entries.get(SET_KEY) {
                    Some(HostValue::List(items)) => items.as_slice(),
                    _ => &[],
                };
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(to_js_at(js, engine, item, depth + 1)?);
                }
                let array = js.new_array_from_values(values)?;
                return Ok(js.construct(&engine.protos.set, &[Value::Object(array)])?);
            }
            let object = js.new_object()?;
            for (name, item) in entries {
                let item = to_js_at(js, engine, item, depth + 1)?;
                js.set_property(&object, &engine.key(name)?, item)?;
            }
            Value::Object(object)
        }
        HostValue::Bytes(bytes) => {
            let buffer = js.new_array_buffer(bytes)?;
            Value::Object(js.new_typed_array(TypedArrayKind::Uint8, &buffer, 0, Some(bytes.len()))?)
        }
        HostValue::Object(object) => object_to_js(js, engine, object)?,
    })
}

fn object_to_js<J: Js>(js: &mut J, engine: &OxideEngine, object: &HostObject) -> Result<Value, Conv> {
    if let Some(value) = super::bind::entities::object_to_js(js, engine, object)? {
        return Ok(value);
    }
    if let Some(tool) = object.downcast::<crate::native::ToolHandle>() {
        let prototype = engine.protos.object_prototype.clone();
        return Ok(Value::Object(super::bind::tool::tool_object(
            js,
            &engine.protos.tools,
            tool,
            Some(&prototype),
        )?));
    }
    // Any other host object (an upload, a stream, an entity list, a find) passes through a script as itself, as on
    // QuickJS: a host call's result that holds one converts, and goes back to the host as it came. Only a read of its
    // members falls back (`names::OpaqueValues`), so a run fails only when it uses one.
    Ok(Value::Object(js.new_host_exotic(
        super::names::OpaqueTarget(object.clone()),
        &engine.protos.opaque,
        Some(&engine.protos.object_prototype),
    )?))
}

/// An oxide value as a host value; an error for one that cannot be held (a function, a symbol, a cyclic value).
pub(super) fn to_host<J: Js>(js: &mut J, engine: &OxideEngine, value: &Value) -> Result<HostValue, Conv> {
    to_host_in(js, engine, value, &mut Vec::new())
}

fn to_host_in<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    value: &Value,
    path: &mut Vec<ObjectRef>,
) -> Result<HostValue, Conv> {
    let object = match value {
        Value::Undefined | Value::Null => return Ok(HostValue::Null),
        Value::Bool(flag) => return Ok(HostValue::Bool(*flag)),
        Value::Int(number) => return Ok(HostValue::I32(*number)),
        Value::Float(number) => {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            if number.fract() == 0.0 && number.abs() <= SAFE_INTEGER as f64 {
                return Ok(HostValue::I64(*number as i64));
            }
            return Ok(HostValue::F64(*number));
        }
        Value::String(text) => return Ok(HostValue::String(text.to_utf8_lossy())),
        Value::BigInt(_) | Value::Symbol(_) => return Err(Conv::Error("cannot convert this value".to_string())),
        Value::Object(object) => object,
    };
    if js.as_callable(object)?.is_some() {
        return Err(Conv::Error("cannot convert this value".to_string()));
    }
    if let Some(decimal) = js.host_data::<OxDecimal>(object)? {
        return Ok(decimal.inner.clone());
    }
    if super::context::is_stack(js, object)? {
        return super::context::combined_map(js);
    }
    if let Some(tool) = js.host_data::<std::sync::Arc<crate::native::ToolHandle>>(object)? {
        return Ok(HostValue::Object(HostObject(std::sync::Arc::clone(&tool) as _)));
    }
    if let Some(opaque) = js.host_data::<super::names::OpaqueTarget>(object)? {
        // a tools object passed through is the stack's value it was made from
        if let Some(held) = opaque.0.downcast::<HostValue>() {
            return Ok((*held).clone());
        }
        return Ok(HostValue::Object(opaque.0.clone()));
    }
    if let Some(host) = super::bind::entities::object_to_host(js, object)? {
        return Ok(host);
    }
    if js.is_array_object(object)? {
        enter(path, object)?;
        let length = js.get_property(object, &static_key("length")?)?;
        let length = js.to_number(&length)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let length = length as u32;
        let mut items = Vec::with_capacity(length as usize);
        for index in 0..length {
            let item = js.get_property(object, &PropertyKey::index(index))?;
            items.push(to_host_in(js, engine, &item, path)?);
        }
        path.pop();
        return Ok(HostValue::List(items));
    }
    let prototype = js.get_prototype_of(object)?;
    let plain = prototype.as_ref() == Some(&engine.protos.object_prototype);
    if !plain {
        if let Some(converted) = class_to_host(js, engine, object, prototype.as_ref())? {
            return Ok(converted);
        }
    }
    enter(path, object)?;
    if !plain && is_set(js, engine, prototype)? {
        let items = js.call(
            &engine.protos.array_from,
            Value::Undefined,
            &[Value::Object(object.clone())],
        )?;
        let mut map = indexmap::IndexMap::new();
        map.insert(SET_KEY.to_string(), to_host_in(js, engine, &items, path)?);
        path.pop();
        return Ok(HostValue::Map(map));
    }
    // A method of the object is not data, so it is left out, as the QuickJS conversion leaves it out; the own
    // enumerable string keys in their order, as QuickJS lists them.
    let keys = js.own_property_keys(object)?;
    let mut map = indexmap::IndexMap::with_capacity(keys.len());
    for property in keys {
        if property.as_symbol().is_some() {
            continue;
        }
        let enumerable = js
            .get_own_property(object, &property)?
            .is_some_and(|descriptor| descriptor.enumerable());
        if !enumerable {
            continue;
        }
        let Some(name) = property.to_js_string() else {
            continue;
        };
        let item = js.get_property(object, &property)?;
        if let Value::Object(inner) = &item {
            if js.as_callable(inner)?.is_some() {
                continue;
            }
        }
        map.insert(name.to_utf8_lossy(), to_host_in(js, engine, &item, path)?);
    }
    path.pop();
    Ok(HostValue::Map(map))
}

/// A cycle or a value nested past [`MAX_NESTING`] is an error, as on QuickJS.
fn enter(path: &mut Vec<ObjectRef>, object: &ObjectRef) -> Result<(), Conv> {
    if path.len() >= MAX_NESTING || path.contains(object) {
        return Err(Conv::Error(TOO_DEEP.to_string()));
    }
    path.push(object.clone());
    Ok(())
}

/// Bytes of a `Uint8Array` or an `ArrayBuffer`, or a `Date`'s time stamp; `None` for an object that converts by its
/// properties.
fn class_to_host<J: Js>(
    js: &mut J,
    engine: &OxideEngine,
    object: &ObjectRef,
    prototype: Option<&ObjectRef>,
) -> Result<Option<HostValue>, Conv> {
    if let Some(view) = js.typed_array_view(object)? {
        if view.kind != TypedArrayKind::Uint8 {
            return Ok(None);
        }
        let Some(length) = view.length else {
            return Ok(None);
        };
        let bytes = js.array_buffer_to_vec(&view.buffer)?;
        let end = (view.byte_offset + length).min(bytes.len());
        let start = view.byte_offset.min(end);
        return Ok(Some(HostValue::Bytes(bytes[start..end].to_vec())));
    }
    if js.array_buffer_len(object)?.is_some() {
        return Ok(Some(HostValue::Bytes(js.array_buffer_to_vec(object)?)));
    }
    if prototype.is_some() && reaches(js, prototype.cloned(), &engine.protos.date_prototype)? {
        // the intrinsic getTime reads the time; an object that only inherits from Date.prototype has none
        if let Ok(millis) = js.call(&engine.protos.date_get_time, Value::Object(object.clone()), &[]) {
            if let Some(millis) = millis.as_number().filter(|millis| millis.is_finite()) {
                #[allow(clippy::cast_possible_truncation)]
                let millis = HostValue::I64(millis as i64);
                return Ok(Some(
                    moqui_lib::object::basic_convert(&millis, "java.sql.Timestamp").value,
                ));
            }
        }
    }
    Ok(None)
}

fn is_set<J: Js>(js: &mut J, engine: &OxideEngine, prototype: Option<ObjectRef>) -> Result<bool, Conv> {
    reaches(js, prototype, &engine.protos.set_prototype)
}

/// Whether `target` is on the prototype chain from `prototype`.
fn reaches<J: Js>(js: &mut J, mut prototype: Option<ObjectRef>, target: &ObjectRef) -> Result<bool, Conv> {
    let mut steps = 0;
    while let Some(current) = prototype {
        if current == *target {
            return Ok(true);
        }
        steps += 1;
        if steps > 32 {
            return Ok(false);
        }
        prototype = js.get_prototype_of(&current)?;
    }
    Ok(false)
}

/// Whether an oxide value is true for a condition: JavaScript's truth, as the QuickJS path's `js_truthy`.
pub(super) fn truthy(value: &Value) -> bool {
    value.to_boolean()
}
