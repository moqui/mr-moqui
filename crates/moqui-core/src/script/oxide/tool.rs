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

//! A native tool (`ec.getTool(name)`, [`crate::native::Tool`]) on quickjs-oxide: a host exotic object (F2) whose hooks
//! answer as `HostTool`'s exotic methods do on QuickJS (`script/runtime/native_tools.rs`). A method of the tool reads
//! as a function that calls it, first and as an own non-enumerable property; then the prototype; then the tool's own
//! property (`Tool::property`), `undefined` when it has none. A set or delete is refused (a `TypeError` in strict code).
//!
//! The engine registers the class once per runtime ([`register`]) and makes an object of it for a tool value
//! ([`tool_object`]), from its conversion of a host value.

use std::rc::Rc;
use std::sync::Arc;

use moqui_lib::object::Value;
use quickjs_oxide::engine::api::core::{
    CompleteOrdinaryPropertyDescriptor, ExoticClass, ExoticScope, FunctionOptions, Hook, HostClass, Js, JsResult,
    ObjectRef, PropertyKey, Scope, Value as JsValue,
};

use super::OxideHooks;
use crate::native::ToolHandle;

/// The exotic class of tools: the engine's conversions, for a method's arguments and result.
pub(crate) struct ToolClass {
    hooks: Rc<dyn OxideHooks>,
}

/// Register the tool class in a runtime.
pub(crate) fn register(js: &mut impl Js, hooks: Rc<dyn OxideHooks>) -> JsResult<HostClass<ToolClass>> {
    js.register_exotic_class(ToolClass { hooks })
}

/// An object of the tool class over `tool`, with `prototype` (`Object.prototype` for a plain tool) behind it.
pub(crate) fn tool_object(
    js: &mut impl Js,
    class: &HostClass<ToolClass>,
    tool: Arc<ToolHandle>,
    prototype: Option<&ObjectRef>,
) -> JsResult<ObjectRef> {
    js.new_host_exotic(tool, class, prototype)
}

fn name_of(key: &PropertyKey) -> Option<String> {
    key.to_js_string().map(|text| text.to_utf8_lossy())
}

fn is_method(tool: &ToolHandle, name: &str) -> bool {
    tool.0.methods().contains(&name)
}

impl ToolClass {
    /// A function that calls `method` on the tool, in the calling script's execution context, the engine lent to it.
    fn method(&self, scope: &mut ExoticScope<'_>, tool: &Arc<ToolHandle>, method: String) -> JsResult<JsValue> {
        let tool = Arc::clone(tool);
        let hooks = Rc::clone(&self.hooks);
        let name = method.clone();
        let function = scope.new_function(FunctionOptions::new(&name), move |scope: &mut Scope<'_>| {
            if crate::script::oxide::engine::fallback_pending() {
                let conv = crate::script::oxide::convert::Conv::Unsupported(super::FELL_BACK);
                return Err(crate::script::oxide::engine::host_error(scope, conv));
            }
            // a panic is not in flight when a call starts, so a mark an earlier one left at the top is stale
            super::UNWOUND.with(|unwound| unwound.set(false));
            let mut args = Vec::with_capacity(scope.argc());
            {
                // a panic in a conversion came through the engine
                let _engine = super::EngineCall;
                for index in 0..scope.argc() {
                    let value = scope.arg(index);
                    args.push(hooks.to_host(scope, &value)?);
                }
            }
            // a panic in the tool's own code is an error the script catches, as the trampoline's
            let called = scope.lend(|| {
                super::caught_panic(|| crate::script::host::with_ec(|ec| tool.0.call(ec, &method, &args)))
            });
            match called {
                Ok(Ok(Ok(result))) => {
                    let _engine = super::EngineCall;
                    hooks.to_js(scope, result)
                }
                Ok(Ok(Err(err))) => Err(super::plain_error(scope, err.message())),
                Ok(Err(_)) => Err(super::plain_error(scope, "no execution context is entered")),
                Err(message) => Err(super::plain_error(scope, &message)),
            }
        })?;
        Ok(JsValue::Object(function.into_object()))
    }

    /// A host value as a script value from a hook, which has no [`Scope`] for the engine's conversion: through a
    /// function that converts it.
    fn to_js(&self, scope: &mut ExoticScope<'_>, value: Value) -> JsResult<JsValue> {
        let hooks = Rc::clone(&self.hooks);
        let held = std::cell::RefCell::new(Some(value));
        let convert = scope.new_function(FunctionOptions::new(""), move |scope: &mut Scope<'_>| {
            let value = held.borrow_mut().take().unwrap_or(Value::Null);
            hooks.to_js(scope, value)
        })?;
        scope.call(&convert, JsValue::Undefined, &[])
    }
}

impl ExoticClass for ToolClass {
    type Data = Arc<ToolHandle>;

    fn get(&self, scope: &mut ExoticScope<'_>, tool: &Self::Data, key: &PropertyKey) -> JsResult<Hook<JsValue>> {
        let Some(name) = name_of(key) else {
            return Ok(Hook::Ordinary);
        };
        if is_method(tool, &name) {
            return self.method(scope, tool, name).map(Hook::Handled);
        }
        // the prototype (and `Object.prototype`) before the tool's own properties
        let this = scope.this();
        if let Some(prototype) = scope.get_prototype_of(&this)? {
            if scope.has_property(&prototype, key)? {
                return Ok(Hook::Ordinary);
            }
        }
        if crate::script::oxide::engine::fallback_pending() {
            let conv = crate::script::oxide::convert::Conv::Unsupported(super::FELL_BACK);
            return Err(crate::script::oxide::engine::host_error(scope, conv));
        }
        let read = scope.lend(|| {
            super::caught_panic(|| crate::script::host::with_ec(|ec| tool.0.property(ec, &name)))
        });
        match read {
            Ok(Ok(Ok(Some(value)))) => self.to_js(scope, value).map(Hook::Handled),
            Ok(Ok(Ok(None))) => Ok(Hook::Handled(JsValue::Undefined)),
            Ok(Ok(Err(err))) => Err(super::plain_error(scope, err.message())),
            Ok(Err(_)) => Err(super::plain_error(scope, "no execution context is entered")),
            Err(message) => Err(super::plain_error(scope, &message)),
        }
    }

    fn get_own_property(
        &self,
        scope: &mut ExoticScope<'_>,
        tool: &Self::Data,
        key: &PropertyKey,
    ) -> JsResult<Hook<Option<CompleteOrdinaryPropertyDescriptor>>> {
        match name_of(key) {
            Some(name) if is_method(tool, &name) => {
                let value = self.method(scope, tool, name)?;
                Ok(Hook::Handled(Some(CompleteOrdinaryPropertyDescriptor::Data {
                    value,
                    writable: false,
                    enumerable: false,
                    configurable: false,
                })))
            }
            _ => Ok(Hook::Handled(None)),
        }
    }

    fn has(&self, _scope: &mut ExoticScope<'_>, tool: &Self::Data, key: &PropertyKey) -> JsResult<Hook<bool>> {
        if name_of(key).is_some_and(|name| is_method(tool, &name)) {
            return Ok(Hook::Handled(true));
        }
        // the prototype answers
        Ok(Hook::Ordinary)
    }

    fn set(
        &self,
        _scope: &mut ExoticScope<'_>,
        _tool: &Self::Data,
        _key: &PropertyKey,
        _value: JsValue,
    ) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(false))
    }

    fn define_own_property(
        &self,
        _scope: &mut ExoticScope<'_>,
        _tool: &Self::Data,
        _key: &PropertyKey,
        _descriptor: &quickjs_oxide::engine::api::core::OrdinaryPropertyDescriptor,
    ) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(false))
    }

    fn delete(&self, _scope: &mut ExoticScope<'_>, _tool: &Self::Data, _key: &PropertyKey) -> JsResult<Hook<bool>> {
        Ok(Hook::Handled(false))
    }

    fn own_keys(&self, _scope: &mut ExoticScope<'_>, tool: &Self::Data) -> JsResult<Hook<Vec<PropertyKey>>> {
        let mut keys = Vec::new();
        for name in tool.0.methods() {
            keys.push(super::key(name)?);
        }
        Ok(Hook::Handled(keys))
    }
}
