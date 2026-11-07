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

//! The conf's script servlets, filters and listeners on quickjs-oxide under `script_engine=oxide`, the oxide side of
//! `runtime/web_host.rs`: the handler file's value called through the same invoker (`web_host::INVOKE_JS`), in a run
//! of its own, with `chain.doFilter()` a host function that lends the engine to the rest of the chain.

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{FunctionOptions, JsResult, Scope, Value};

use super::convert::{key, to_js};
use super::engine::{guarded, with_run, Outcome};
use super::errors::runtime_error;
use crate::context::Error;

/// [`crate::script::invoke_web_handler`] on quickjs-oxide.
pub(crate) fn invoke(location: &str, kind: &str, data: HostValue) -> Result<(), Error> {
    let (script, _) = crate::script::runtime::read_module(location)?;
    let _frame = crate::script::runtime::ContextFrame::push();
    let ran = with_run(true, location, |engine, context, note| {
        let failed =
            |context: &mut quickjs_oxide::engine::api::core::Context, err| Outcome::Error(runtime_error(context, err));
        let hash = crate::script::text::hash_of(script.hash(), script.file_name());
        let handler = match engine
            .compile(script.shared_text(), hash, script.file_name())
            .and_then(|compiled| context.execute(&compiled))
        {
            Ok(handler) => handler,
            Err(err) => {
                note(&err);
                return Err(failed(context, err));
            }
        };
        let made = (|| {
            let invoker = context.eval(crate::script::runtime::WEB_INVOKE_JS)?;
            let invoker = match &invoker {
                Value::Object(object) => context.as_callable(object)?,
                _ => None,
            }
            .ok_or(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
                "the web invoker",
            ))?;
            let global = context.global_object()?;
            let ec = context.get_property(
                &global,
                &key("ec").map_err(|_| quickjs_oxide::engine::api::core::RuntimeError::Invariant("a key"))?,
            )?;
            let next = context.new_function(
                FunctionOptions::new("chainNext"),
                |scope: &mut Scope<'_>| -> JsResult<Value> {
                    guarded(scope, |scope| {
                        let next = scope.lend(crate::web::servlet::chain_next);
                        next.map(|()| Value::Undefined).map_err(|err| {
                            super::convert::Conv::Js(super::bind::throw(
                                scope,
                                crate::script::host::HostError::Context(err),
                                None,
                            ))
                        })
                    })
                },
            )?;
            let made = context.call(&invoker, Value::Undefined, &[ec, Value::from(next)])?;
            let Value::Object(made) = made else {
                return Err(quickjs_oxide::engine::api::core::RuntimeError::Invariant(
                    "the web invoker",
                ));
            };
            match context.get_property(
                &made,
                &key("invoke").map_err(|_| quickjs_oxide::engine::api::core::RuntimeError::Invariant("a key"))?,
            )? {
                Value::Object(call) => context
                    .as_callable(&call)?
                    .ok_or(quickjs_oxide::engine::api::core::RuntimeError::Invariant("invoke")),
                _ => Err(quickjs_oxide::engine::api::core::RuntimeError::Invariant("invoke")),
            }
        })();
        let call = match made {
            Ok(call) => call,
            Err(err) => {
                note(&err);
                return Err(failed(context, err));
            }
        };
        let data = to_js(context, engine, &data).map_err(|conv| match conv {
            super::convert::Conv::Js(err) => failed(context, err),
            super::convert::Conv::Error(message) => Outcome::Error(Error::new(message)),
            super::convert::Conv::Unsupported(reason) => Outcome::Fallback(reason),
        })?;
        let kind = Value::String(
            quickjs_oxide::engine::api::core::JsString::try_from_utf8(kind)
                .map_err(|_| Outcome::Error(Error::new("a handler kind")))?,
        );
        match context.call(&call, Value::Undefined, &[handler, kind, data]) {
            Ok(_) => Ok(()),
            Err(err) => {
                note(&err);
                Err(failed(context, err))
            }
        }
    });
    ran.map_err(|outcome| match outcome {
        Outcome::Error(err) => err,
        Outcome::Fallback(reason) => Error::new(format!(
            "script_engine=oxide: this web handler needs {reason}, which the quickjs-oxide bindings do not have yet"
        )),
    })
}
