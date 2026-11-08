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

//! Element literals on quickjs-oxide: the conversion of an [`ArgKind::Node`](crate::script::host::ArgKind::Node)
//! argument, as `script/runtime/screen_host.rs` makes it on QuickJS. A screen unit's literal holds its conversion as
//! `$nodeLit`, a host object of the table's `host:NodeLit` class that `sri.holdNodeLits` (the `sri` shim,
//! `script/host/sri.rs`) attached; any other element object is converted now.

use std::sync::Arc;

use quickjs_oxide::engine::api::core::{Js, JsResult, NativeErrorKind, ObjectRef, PropertyKey, Value as JsValue};

use super::{plain_error, OxideHost};
use crate::screen::sri::NodeLit;
use crate::script::runtime::{MAX_NESTING, TOO_DEEP};

/// The property of a unit's element literal that holds its conversion.
const HELD_LIT: &str = "$nodeLit";

/// The element `value` is: what a unit's literal holds, else converted now. A value that is not an object throws
/// `expected an element object`.
pub(crate) fn lit_of(js: &mut impl Js, value: &JsValue) -> JsResult<Arc<NodeLit>> {
    if let JsValue::Object(object) = value {
        let held = js.property_host_data::<OxideHost>(object, &super::super::convert::static_key(HELD_LIT)?)?;
        if let Some(node) = held_lit(held) {
            return Ok(node);
        }
    }
    lit_in(js, value, &mut Vec::new()).map(Arc::new)
}

/// [`lit_of`] of argument `index` when its literal holds its conversion, read with no handle for the argument or the
/// held object; `None` for any other argument (converted by [`lit_of`]).
pub(crate) fn held_lit_arg(scope: &mut quickjs_oxide::engine::api::core::Scope<'_>, index: usize) -> JsResult<Option<Arc<NodeLit>>> {
    let held = scope.arg_property_host_data::<OxideHost>(index, &super::super::convert::static_key(HELD_LIT)?)?;
    Ok(held_lit(held))
}

fn held_lit(held: Option<std::rc::Rc<OxideHost>>) -> Option<Arc<NodeLit>> {
    held.and_then(|held| held.obj().and_then(|obj| obj.downcast::<NodeLit>()))
}

/// `path` holds the element objects above this one, through `children` and `parent`.
fn lit_in(js: &mut impl Js, value: &JsValue, path: &mut Vec<ObjectRef>) -> JsResult<NodeLit> {
    let JsValue::Object(object) = value else {
        return Err(plain_error(js, "expected an element object"));
    };
    if path.len() >= MAX_NESTING || path.contains(object) {
        return Err(plain_error(js, TOO_DEEP));
    }
    path.push(object.clone());
    let mut node = NodeLit::default();
    if let JsValue::String(name) = js.get_property(object, &super::super::convert::static_key("name")?)? {
        node.name = name.to_utf8_lossy();
    }
    if let JsValue::String(text) = js.get_property(object, &super::super::convert::static_key("text")?)? {
        node.text = text.to_utf8_lossy();
    }
    if let JsValue::Object(attrs) = js.get_property(object, &super::super::convert::static_key("attrs")?)? {
        for property in js.own_property_keys(&attrs)? {
            if property.as_symbol().is_some() {
                continue;
            }
            let enumerable = js
                .get_own_property(&attrs, &property)?
                .is_some_and(|descriptor| descriptor.enumerable());
            let Some(name) = property.to_js_string().filter(|_| enumerable) else {
                continue;
            };
            match js.get_property(&attrs, &property)? {
                JsValue::String(item) => {
                    node.attrs.insert(name.to_utf8_lossy(), item.to_utf8_lossy());
                }
                other => return Err(not_a_string(js, &other)),
            }
        }
    }
    if let JsValue::Object(children) = js.get_property(object, &super::super::convert::static_key("children")?)? {
        if js.is_array_object(&children)? {
            let length = js.get_property(&children, &super::super::convert::static_key("length")?)?;
            let length = js.to_number(&length)?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let length = length as u32;
            for index in 0..length {
                let item = js.get_property(&children, &PropertyKey::index(index))?;
                node.children.push(lit_in(js, &item, path)?);
            }
        }
    }
    let parent = js.get_property(object, &super::super::convert::static_key("parent")?)?;
    if let JsValue::Object(_) = parent {
        node.parent = Some(Box::new(lit_in(js, &parent, path)?));
    }
    path.pop();
    Ok(node)
}

/// The `TypeError` QuickJS's string conversion of an attribute value throws, the value's type named as rquickjs names
/// it (a Proxy is `object` here: the bindings cannot tell one, where rquickjs says `proxy`).
fn not_a_string(js: &mut impl Js, value: &JsValue) -> quickjs_oxide::engine::api::core::RuntimeError {
    let from = super::super::errors::rquickjs_type_name(js, value, false);
    let message = format!("Error converting from js '{from}' into type 'string'");
    js.throw_error(NativeErrorKind::Type, &message)
}

#[cfg(test)]
mod tests {
    use quickjs_oxide::engine::api::core::RuntimeError;

    use super::super::tests::shimmed_runtime;

    /// The `sri` shim on a bare runtime with the host table's shims holds element literals and converts them as the
    /// QuickJS binder does (`script/runtime/bind_tests.rs` runs the same script).
    #[test]
    fn element_literals_are_held_and_converted_as_on_quickjs() -> Result<(), RuntimeError> {
        let (_runtime, context) = shimmed_runtime()?;
        let value = context.eval(crate::script::host::SRI_HOLD_SCRIPT)?;
        let got = context.to_js_string(&value)?.to_utf8_lossy();
        assert_eq!(got, crate::script::host::SRI_HOLD_EXPECTED);
        Ok(())
    }

    /// An attribute value that is not a string is refused with rquickjs's conversion error, its type named as rquickjs
    /// names it.
    #[test]
    fn an_attribute_that_is_no_string_is_named_as_rquickjs_names_it() -> Result<(), RuntimeError> {
        let (_runtime, mut context) = shimmed_runtime()?;
        for (value, type_name) in [
            ("[]", "array"),
            ("() => 1", "function"),
            ("function () {}", "constructor"),
            ("new Error('x')", "exception"),
            ("({ [Symbol.toStringTag]: 'Error' })", "object"),
            ("Promise.resolve(1)", "promise"),
            ("7", "int"),
            ("1.5", "float"),
            ("true", "bool"),
            ("null", "null"),
        ] {
            let element = context.eval(&format!("({{ name: 'x', attrs: {{ a: {value} }} }})"))?;
            let Err(RuntimeError::Exception(thrown)) = super::lit_of(&mut context, &element) else {
                panic!("{value} converted");
            };
            let message = context.to_js_string(&thrown)?.to_utf8_lossy();
            assert_eq!(
                message,
                format!("TypeError: Error converting from js '{type_name}' into type 'string'"),
                "{value}"
            );
        }
        Ok(())
    }
}
