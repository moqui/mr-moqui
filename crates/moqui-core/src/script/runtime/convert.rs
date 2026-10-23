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

//! Conversion between context-stack values and JavaScript values.

use super::*;

/// How deep a script value may nest, in arrays, objects and Sets, when it becomes a host value.
/// Context data rarely nests past a few levels and parsed JSON stops at 128; the limit keeps a
/// deep or cyclic value from overflowing the native stack.
pub(super) const MAX_NESTING: usize = 200;

/// How deep a host value may nest when it becomes a script value. A host value cannot be cyclic;
/// this is a little above [`MAX_NESTING`] so a value a script stored still reads back inside the
/// maps host code wraps it in.
const MAX_HOST_NESTING: usize = MAX_NESTING + 56;

pub(super) fn host_to_js<'js>(ctx: &Ctx<'js>, value: Value) -> rquickjs::Result<JsValue<'js>> {
    host_to_js_at(ctx, value, 0)
}

fn host_to_js_at<'js>(ctx: &Ctx<'js>, value: Value, depth: usize) -> rquickjs::Result<JsValue<'js>> {
    match value {
        Value::Null => Null.into_js(ctx),
        Value::Bool(flag) => flag.into_js(ctx),
        Value::String(text) => text.into_js(ctx),
        Value::I32(number) => number.into_js(ctx),
        Value::I64(number) => i64_to_js(ctx, number),
        Value::F64(number) => number.into_js(ctx),
        Value::Decimal(_) => {
            let class = Class::instance(ctx.clone(), HostDecimal { inner: value })?;
            class.into_js(ctx)
        }
        Value::Timestamp(_) | Value::Date(_) | Value::Time(_) => temporal_text(&value).into_js(ctx),
        Value::List(_) | Value::Map(_) => collection_to_js_at(ctx, value, depth),
        Value::Bytes(bytes) => {
            rquickjs::TypedArray::<u8>::new(ctx.clone(), bytes).map(rquickjs::TypedArray::into_value)
        }
        Value::Object(object) => object_to_js(ctx, &object),
    }
}

/// Dates reach a script as text, as Java `toString` would give. Assigning one back stores the text,
/// and the entity layer converts it to the field type on write.
pub(super) fn temporal_text(value: &Value) -> String {
    let text = moqui_lib::object::basic_convert(value, "String").value;
    match text.as_str() {
        Some(text) => text.to_string(),
        None => moqui_lib::object::to_plain_string(value),
    }
}

// Keep the large native builder out of the recursive conversion frames.
#[inline(never)]
fn find_to_js<'js>(ctx: &Ctx<'js>, find: &EntityFind) -> rquickjs::Result<JsValue<'js>> {
    wrap_find(ctx, find.clone()).and_then(|class| class.into_js(ctx))
}

#[inline(never)]
fn find_to_host(ctx: &Ctx<'_>, class: &Class<'_, HostFind>) -> Result<Value, Error> {
    let find = class.borrow().peek(ctx).map_err(|err| js_caught(ctx, err))?;
    Ok(Value::Object(HostObject::new(find)))
}

pub(super) fn object_to_js<'js>(ctx: &Ctx<'js>, object: &HostObject) -> rquickjs::Result<JsValue<'js>> {
    if let Some(find) = object.downcast::<EntityFind>() {
        return find_to_js(ctx, &find);
    }
    if let Some(inner) = object.downcast::<EntityCell>() {
        return entity_to_js(ctx, HostEntity::of(inner));
    }
    if let Some(inner) = object.downcast::<ScriptList>() {
        return Class::instance(ctx.clone(), HostList::of_shared(inner)).and_then(|class| class.into_js(ctx));
    }
    if let Some(list) = object.downcast::<EntityList>() {
        return Class::instance(ctx.clone(), HostList::of_list(EntityList::clone(&list)))
            .and_then(|class| class.into_js(ctx));
    }
    if let Some(inner) = object.downcast::<crate::web::error_info::ErrorInfo>() {
        return Class::instance(ctx.clone(), HostErrorInfo { inner }).and_then(|class| class.into_js(ctx));
    }
    if let Some(inner) = object.downcast::<crate::web::multipart::UploadedFile>() {
        return Class::instance(ctx.clone(), HostUpload { inner }).and_then(|class| class.into_js(ctx));
    }
    if let Some(inner) = object.downcast::<StreamBytes>() {
        return Class::instance(ctx.clone(), HostStream { inner }).and_then(|class| class.into_js(ctx));
    }
    if let Some(inner) = object.downcast::<crate::native::ToolHandle>() {
        return tool_to_js(ctx, inner);
    }
    if let Some(inner) = object.downcast::<Condition>() {
        return condition_js(ctx, Some(Condition::clone(&inner)));
    }
    Err(Exception::throw_message(ctx, "cannot convert this object"))
}

/// Largest integer a JavaScript number holds exactly.
pub(super) const SAFE_INTEGER: i64 = 9_007_199_254_740_991;

pub(super) fn i64_to_js<'js>(ctx: &Ctx<'js>, number: i64) -> rquickjs::Result<JsValue<'js>> {
    if let Ok(small) = i32::try_from(number) {
        return small.into_js(ctx);
    }
    if number.unsigned_abs() > SAFE_INTEGER.unsigned_abs() {
        return Err(Exception::throw_message(
            ctx,
            "integer is too large for a script number; it would lose precision",
        ));
    }
    #[allow(clippy::cast_precision_loss)]
    (number as f64).into_js(ctx)
}

pub(super) fn js_to_host(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<Value> {
    js_value_to_host(ctx, value).map_err(|err| Exception::throw_message(ctx, err.message()))
}

/// The plain text of a script argument. `None` when it is missing, `null` or `undefined`.
pub(super) fn optional_text(ctx: &Ctx<'_>, value: Option<&JsValue<'_>>) -> rquickjs::Result<Option<String>> {
    match value {
        Some(value) if !value.is_null() && !value.is_undefined() => {
            Ok(Some(moqui_lib::object::to_plain_string(&js_to_host(ctx, value)?)))
        }
        _ => Ok(None),
    }
}

/// [`optional_text`], empty when the argument is missing, `null` or `undefined`.
pub(super) fn text_or_empty(ctx: &Ctx<'_>, value: Option<&JsValue<'_>>) -> rquickjs::Result<String> {
    Ok(optional_text(ctx, value)?.unwrap_or_default())
}

pub(super) fn js_truthy(value: &JsValue<'_>) -> bool {
    if value.is_null() || value.is_undefined() {
        return false;
    }
    if let Some(flag) = value.as_bool() {
        return flag;
    }
    if let Some(number) = value.as_number() {
        return number != 0.0 && !number.is_nan();
    }
    if let Some(text) = value.as_string() {
        return text.to_string().ok().is_some_and(|text| !text.is_empty());
    }
    true
}

pub(super) fn value_to_js<'js>(ctx: &Ctx<'js>, value: &Value) -> rquickjs::Result<JsValue<'js>> {
    host_to_js(ctx, value.clone())
}

pub(super) fn js_string(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<String> {
    let Some(text) = value.as_string() else {
        return Err(Exception::throw_message(ctx, "expected a string"));
    };
    text.to_string()
}

pub(super) fn js_i64(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<i64> {
    if let Some(number) = value.as_int() {
        return Ok(i64::from(number));
    }
    if value.as_float().is_some() {
        return Err(Exception::throw_message(ctx, "limit is not an integer"));
    }
    Err(Exception::throw_message(ctx, "limit is not a number"))
}

pub(super) fn collection_to_js<'js>(ctx: &Ctx<'js>, value: Value) -> rquickjs::Result<JsValue<'js>> {
    collection_to_js_at(ctx, value, 0)
}

fn collection_to_js_at<'js>(ctx: &Ctx<'js>, value: Value, depth: usize) -> rquickjs::Result<JsValue<'js>> {
    if depth >= MAX_HOST_NESTING && matches!(value, Value::List(_) | Value::Map(_)) {
        return Err(Exception::throw_message(ctx, TOO_DEEP));
    }
    let depth = depth + 1;
    match value {
        Value::List(items) => {
            let array = rquickjs::Array::new(ctx.clone())?;
            for (index, item) in items.into_iter().enumerate() {
                array.set(index, host_to_js_at(ctx, item, depth)?)?;
            }
            Ok(array.into_value())
        }
        Value::Map(mut entries) if entries.len() == 1 && entries.contains_key(SET_KEY) => {
            let items = entries.swap_remove(SET_KEY).unwrap_or(Value::Null);
            let array = collection_to_js_at(ctx, items, depth)?;
            let constructor: rquickjs::Constructor = ctx.globals().get("Set")?;
            constructor.construct((array,))
        }
        Value::Map(entries) => {
            let wrap = match entries.get(WRAP_KEY) {
                Some(Value::String(name)) => Some(name.clone()),
                _ => None,
            };
            let object = Object::new(ctx.clone())?;
            for (key, item) in entries {
                object.set(key, host_to_js_at(ctx, item, depth)?)?;
            }
            // A tools object keeps its data on the stack and gets its methods back when it is read (`tools_host`).
            if let Some(name) = wrap {
                let wrapper: Option<rquickjs::Function> = ctx.globals().get("__moquiWrap").ok();
                if let Some(wrapper) = wrapper {
                    return wrapper.call((name, object));
                }
            }
            Ok(object.into_value())
        }
        other => host_to_js_at(ctx, other, depth),
    }
}

pub(super) fn object_map(
    ctx: &Ctx<'_>,
    value: &JsValue<'_>,
) -> Result<std::collections::BTreeMap<String, Value>, Error> {
    if !value.is_object() || value.is_array() || value.is_function() {
        return Err(Error::new("script value is not an object"));
    }
    match js_value_to_host(ctx, value)? {
        Value::Map(map) => Ok(map.into_iter().collect()),
        _ => Err(Error::new("script value is not an object")),
    }
}

/// A `Uint8Array` or an `ArrayBuffer` is binary: a copy of its bytes.
pub(super) fn js_bytes(object: &Object<'_>) -> Option<Vec<u8>> {
    if let Ok(array) = rquickjs::TypedArray::<u8>::from_object(object.clone()) {
        // SAFETY: the slice is copied before any JavaScript runs again.
        return unsafe { array.as_bytes() }.map(<[u8]>::to_vec);
    }
    let buffer = rquickjs::ArrayBuffer::from_object(object.clone())?;
    // SAFETY: as above.
    unsafe { buffer.as_bytes() }.map(<[u8]>::to_vec)
}

/// The error for a value deeper than [`MAX_NESTING`], which a cyclic value always is.
pub(super) const TOO_DEEP: &str = "value is cyclic or nested too deeply";

/// Step into `value` on the way down a walk of nested script values. `path` holds the arrays,
/// objects and Sets above it: a value already there is a cycle. A value shared by two branches
/// is not on the path of either, so it converts once for each.
pub(super) fn enter_nested<'js>(path: &mut Vec<JsValue<'js>>, value: &JsValue<'js>) -> Result<(), Error> {
    if path.len() >= MAX_NESTING || path.contains(value) {
        return Err(Error::new(TOO_DEEP));
    }
    path.push(value.clone());
    Ok(())
}

pub(super) fn js_value_to_host(ctx: &Ctx<'_>, value: &JsValue<'_>) -> Result<Value, Error> {
    js_value_to_host_in(ctx, value, &mut Vec::new())
}

fn js_value_to_host_in<'js>(ctx: &Ctx<'_>, value: &JsValue<'js>, path: &mut Vec<JsValue<'js>>) -> Result<Value, Error> {
    if value.is_null() || value.is_undefined() {
        return Ok(Value::Null);
    }
    if let Some(flag) = value.as_bool() {
        return Ok(Value::Bool(flag));
    }
    if let Some(number) = value.as_int() {
        return Ok(Value::I32(number));
    }
    if let Some(number) = value.as_float() {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        if number.fract() == 0.0 && number.abs() <= SAFE_INTEGER as f64 {
            return Ok(Value::I64(number as i64));
        }
        return Ok(Value::F64(number));
    }
    if let Some(text) = value.as_string() {
        let text = text.to_string().map_err(|err| js_caught(ctx, err))?;
        return Ok(Value::String(text));
    }
    if let Some(array) = value.as_array() {
        enter_nested(path, value)?;
        let mut items = Vec::with_capacity(array.len());
        for item in array.iter::<JsValue>() {
            let item = item.map_err(|err| js_caught(ctx, err))?;
            items.push(js_value_to_host_in(ctx, &item, path)?);
        }
        path.pop();
        return Ok(Value::List(items));
    }
    if value.is_function() {
        return Err(Error::new("cannot convert this value"));
    }
    // An entity value reaches a script behind its field proxy; the host object is the proxy's target.
    if let Some(proxy) = value.as_proxy() {
        if let Ok(target) = proxy.target() {
            if let Some(class) = target.as_class::<HostEntity>() {
                return Ok(Value::Object(HostObject(class.borrow().inner.clone())));
            }
            // `context` is the stack: a call that takes it as a map gets the visible names, as Java passes the stack.
            if target.as_class::<HostContextStack>().is_some() {
                return context_combined_map(ctx);
            }
        }
    }
    if let Some(object) = value.as_object() {
        if let Some(bytes) = js_bytes(object) {
            return Ok(Value::Bytes(bytes));
        }
        if let Some(class) = object.as_class::<HostDecimal>() {
            return Ok(class.borrow().inner.clone());
        }
        if let Some(class) = object.as_class::<HostEntity>() {
            return Ok(Value::Object(HostObject(class.borrow().inner.clone())));
        }
        // XML actions keep the find in context for later sections such as Show SQL.
        if let Some(class) = object.as_class::<HostFind>() {
            return find_to_host(ctx, class);
        }
        if let Some(class) = object.as_class::<HostList>() {
            return Ok(Value::Object(HostObject(class.borrow().inner.clone())));
        }
        // An uploaded file passed to a service stays the file, as the Java FileItem in `ec.web.parameters` does.
        if let Some(class) = object.as_class::<HostUpload>() {
            return Ok(Value::Object(HostObject(class.borrow().inner.clone())));
        }
        // A stream (`openStream()`, `getInputStream()`) stays the stream too: it has no own properties, so it would be an
        // empty map, which a write would store as the text `{}`.
        if let Some(class) = object.as_class::<HostStream>() {
            return Ok(Value::Object(HostObject(class.borrow().inner.clone())));
        }
        // A tool stays the tool, so its methods are there again when the value is read back.
        if let Some(class) = object.as_class::<HostTool>() {
            return Ok(Value::Object(HostObject(class.borrow().inner.clone())));
        }
        // An entity condition stays the condition, as the Java EntityCondition in the context does, so a later
        // `<entity-find>` applies it through `<econdition-object>`; it has no own properties and would be an empty map.
        if let Some(class) = object.as_class::<HostCondition>() {
            return Ok(Value::Object(HostObject::new(class.borrow().inner.clone())));
        }
        // A `Date` has no own properties; it is a time stamp, as a Java `Date` passed to a service is.
        if let Some(millis) = js_date_millis(object) {
            return Ok(moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value);
        }
        enter_nested(path, value)?;
        if is_js_set(object) {
            // The value's own context, so the lifetimes of the call agree.
            let own = value.ctx().clone();
            let array_constructor: Object = own.globals().get("Array").map_err(|err| js_caught(ctx, err))?;
            let from: rquickjs::Function = array_constructor.get("from").map_err(|err| js_caught(ctx, err))?;
            let items: JsValue = from.call((value.clone(),)).map_err(|err| js_caught(ctx, err))?;
            let mut map = indexmap::IndexMap::new();
            map.insert(SET_KEY.to_string(), js_value_to_host_in(ctx, &items, path)?);
            path.pop();
            return Ok(Value::Map(map));
        }
        // A method of the object is not data, so it is left out, as the unwrapped projection leaves out a function.
        let mut map = indexmap::IndexMap::new();
        for entry in object.props::<String, JsValue>() {
            let (key, item) = entry.map_err(|err| js_caught(ctx, err))?;
            if item.is_function() {
                continue;
            }
            map.insert(key, js_value_to_host_in(ctx, &item, path)?);
        }
        path.pop();
        return Ok(Value::Map(map));
    }
    Err(Error::new("cannot convert this value"))
}

/// A script `Set` is held on the context stack as a one-key map holding its items, so a Set one
/// action block makes in a context name is a Set again in the next block.
pub(super) const SET_KEY: &str = "__set__";

/// The key of a map whose JavaScript form is a tools object: the name of the wrapper that adds its methods.
pub(super) const WRAP_KEY: &str = "__wrap";

/// A script `Set`, or a subclass of one: an instance of this context's own `Set`, not an object that only names
/// itself one through `constructor.name`.
fn is_js_set(object: &Object<'_>) -> bool {
    let ctx = object.ctx();
    ctx.globals()
        .get::<_, Object>("Set")
        .is_ok_and(|set| object.is_instance_of(&set))
}

/// The epoch millis of a script `Date`, or `None` for any other object. An invalid date is `None` too, so it converts
/// as the empty object it is. The intrinsic `Date.prototype.getTime` reads the time, not a `getTime` the object brings.
fn js_date_millis(object: &Object<'_>) -> Option<i64> {
    let ctx = object.ctx();
    let date: Object = ctx.globals().get("Date").ok()?;
    if !object.is_instance_of(&date) {
        return None;
    }
    let prototype: Object = date.get("prototype").ok()?;
    let get_time: rquickjs::Function = prototype.get("getTime").ok()?;
    let millis: f64 = match get_time.call((rquickjs::function::This(object.clone()),)) {
        Ok(millis) => millis,
        Err(_) => {
            // an object that only inherits from `Date.prototype` has no time: the call threw, so the throw is cleared
            let _ = ctx.catch();
            return None;
        }
    };
    #[allow(clippy::cast_possible_truncation)]
    millis.is_finite().then_some(millis as i64)
}

pub(super) fn js_caught(ctx: &Ctx<'_>, err: JsError) -> Error {
    Error::new(rquickjs::CaughtError::from_error(ctx, err).to_string())
}

/// The texts of an array or a `Set` as the host holds it, or of one value.
pub(super) fn host_texts(value: Value) -> Vec<String> {
    match value {
        Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Value::Map(mut map) if map.contains_key(SET_KEY) => host_texts(map.swap_remove(SET_KEY).unwrap_or(Value::Null)),
        Value::Null => Vec::new(),
        other => vec![moqui_lib::object::to_plain_string(&other)],
    }
}
