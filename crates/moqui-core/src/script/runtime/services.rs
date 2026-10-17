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

//! The service call builder scripts get from `ec.service`.

use super::*;

pub(super) fn service_sync<'js>(ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostService>> {
    host_guard(&ctx, || fresh_service(&ctx, false))
}

pub(super) fn service_async<'js>(ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostService>> {
    host_guard(&ctx, || fresh_service(&ctx, true))
}

pub(super) fn fresh_service<'js>(ctx: &Ctx<'js>, is_async: bool) -> rquickjs::Result<Class<'js, HostService>> {
    Class::instance(
        ctx.clone(),
        HostService {
            call: std::cell::RefCell::new(ServiceCall {
                name: String::new(),
                parameters: std::collections::BTreeMap::new(),
                options: CallOptions::default(),
                is_async,
            }),
        },
    )
}

pub(super) struct ServiceCall {
    pub(super) name: String,
    pub(super) parameters: std::collections::BTreeMap<String, Value>,
    pub(super) options: CallOptions,
    pub(super) is_async: bool,
}

/// Service call builder. Each method changes the builder and returns it, as Java's. There is no `distribute` method.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostService {
    #[qjs(skip_trace)]
    pub(super) call: std::cell::RefCell<ServiceCall>,
}

/// Change the builder `this` and return it.
fn changed<'js>(
    this: This<Class<'js, HostService>>,
    ctx: &Ctx<'js>,
    change: impl FnOnce(&mut ServiceCall),
) -> rquickjs::Result<Class<'js, HostService>> {
    {
        let builder = this.0.borrow();
        let mut call = builder
            .call
            .try_borrow_mut()
            .map_err(|_| Exception::throw_message(ctx, "the service call is running"))?;
        change(&mut call);
    }
    Ok(this.0)
}

/// Whether `value` is `ec.context`, the whole context: `in-map="context"` passes it.
fn is_ec_context<'js>(ctx: &Ctx<'js>, value: &JsValue<'js>) -> rquickjs::Result<bool> {
    if !value.is_object() {
        return Ok(false);
    }
    let ec: JsValue = ctx.globals().get("ec")?;
    let Some(ec) = ec.as_object() else {
        return Ok(false);
    };
    let context: JsValue = ec.get("context")?;
    Ok(context == *value)
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostService {
    fn name<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        name: String,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || changed(this, &ctx, |call| call.name = name))
    }

    fn disable_authz<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || changed(this, &ctx, |call| call.options.disable_authz = true))
    }

    /// `multi(true)`: one call for each row of `name_N` parameters, in one transaction, as Java's multi form submit.
    fn multi<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        flag: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            changed(this, &ctx, |call| call.options.multi = flag.as_bool().unwrap_or(false))
        })
    }

    fn ignore_transaction<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        flag: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            changed(this, &ctx, |call| call.options.ignore_transaction = flag.as_bool())
        })
    }

    fn require_new_transaction<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        flag: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            changed(this, &ctx, |call| call.options.require_new_transaction = flag.as_bool())
        })
    }

    fn transaction_timeout<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        seconds: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            let seconds = js_i64(&ctx, &seconds)?;
            changed(this, &ctx, |call| {
                call.options.timeout = u64::try_from(seconds).ok().map(std::time::Duration::from_secs);
            })
        })
    }

    /// `distribute(true)`: Java sends the call to another server of a cluster. This port runs a call on this server's
    /// service workers, so the call is the same as without it.
    fn distribute<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        _flag: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || Ok(this.0))
    }

    /// `softValidate(true)`: input that fails validation is danger messages, not errors, and the call returns no results.
    fn soft_validate<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        flag: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            changed(this, &ctx, |call| {
                call.options.soft_validate = flag.as_bool().unwrap_or(true)
            })
        })
    }

    /// `ignorePreviousError(true)`: run even with errors already in the message facade; they come back after.
    fn ignore_previous_error<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        flag: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            let on = flag.0.as_ref().and_then(JsValue::as_bool).unwrap_or(true);
            changed(this, &ctx, |call| call.options.ignore_previous_error = on)
        })
    }

    /// `noRememberParameters()`: the artifact hit of this call does not keep its parameters.
    fn no_remember_parameters<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            changed(this, &ctx, |call| call.options.no_remember_parameters = true)
        })
    }

    fn use_transaction_cache<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        flag: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            changed(this, &ctx, |call| call.options.use_transaction_cache = flag.as_bool())
        })
    }

    /// Add the entries of a map. `ec.context` is the whole context, read from the stack as `context` reads it.
    fn parameters<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        map: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            let entries = if is_ec_context(&ctx, &map)? {
                if let Some(engine) = current_engine() {
                    readopt_all(&engine);
                }
                flush_current(&ctx, false)?;
                let mut visible = context_combined();
                visible.retain(|_, value| !matches!(value, Value::Bytes(_)));
                visible
            } else {
                // a plain object, or an entity value whose fields are the parameters
                match super::values::value_map(&js_to_host(&ctx, &map)?) {
                    Some(entries) => entries,
                    None => return Ok(this.0),
                }
            };
            changed(this, &ctx, |call| call.parameters.extend(entries))
        })
    }

    fn parameter<'js>(
        this: This<Class<'js, HostService>>,
        ctx: Ctx<'js>,
        name: String,
        value: JsValue<'js>,
    ) -> rquickjs::Result<Class<'js, HostService>> {
        host_guard(&ctx, || {
            let value = js_to_host(&ctx, &value)?;
            changed(this, &ctx, |call| {
                call.parameters.insert(name, value);
            })
        })
    }

    /// Start the service on a service worker and return a future for its out parameters.
    fn call_future<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            flush_current(&ctx, true)?;
            let call = self.call.borrow();
            match crate::service::call::start_future(&call.name, &call.parameters) {
                Ok(future) => Class::instance(
                    ctx.clone(),
                    HostFuture {
                        future: std::cell::RefCell::new(future),
                    },
                )
                .and_then(|class| class.into_js(&ctx)),
                Err(err) => {
                    let message = err.message().to_string();
                    with_ec(&ctx, |ec| ec.message().add_error(message))?;
                    Null.into_js(&ctx)
                }
            }
        })
    }

    /// Run the service. As Java's, a failure of its body is in the message facade and the result is the results as
    /// they were, empty. An authorization failure is thrown, with its kind, as Java's `ArtifactAuthorizationException`
    /// passes through the script. Another error of the call, which Java throws too (an unknown service), is added to the
    /// message facade and the result is null. Either way the generated code's `hasError()` check decides.
    fn call<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            // The callee may read or change the caller's names, so they are written back first.
            flush_current(&ctx, true)?;
            let call = self.call.borrow();
            if call.is_async {
                if let Err(err) = crate::service::call::async_call(&call.name, &call.parameters) {
                    let message = err.message().to_string();
                    with_ec(&ctx, |ec| ec.message().add_error(message))?;
                }
                return Null.into_js(&ctx);
            }
            let called = if call.options.multi {
                crate::service::call::sync_multi(&call.name, &call.parameters, &call.options)
            } else {
                crate::service::call::sync_with(&call.name, &call.parameters, &call.options)
            };
            match called {
                Ok(results) => host_to_js(&ctx, Value::Map(results.into_iter().collect())),
                Err(err) if err.kind() == ErrorKind::Unauthorized => Err(throw_context(&ctx, &err)),
                Err(err) => {
                    let message = err.diagnostic();
                    with_ec(&ctx, |ec| ec.message().add_error(message))?;
                    Null.into_js(&ctx)
                }
            }
        })
    }
}

/// The pending result of `callFuture`: `get()`, `get(timeoutMs)`, and `isDone()`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostFuture {
    #[qjs(skip_trace)]
    future: std::cell::RefCell<crate::service::ServiceFuture>,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostFuture {
    fn is_done(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || Ok(self.future.borrow_mut().is_done()))
    }

    /// The out parameters. With a timeout in milliseconds, or with none the time the script has left, a call that is
    /// not done in time throws.
    fn get<'js>(&self, ctx: Ctx<'js>, timeout_ms: Opt<JsValue<'js>>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let limit = match &timeout_ms.0 {
                Some(value) if !value.is_null() && !value.is_undefined() => {
                    let millis = js_i64(&ctx, value)?;
                    Some(std::time::Duration::from_millis(u64::try_from(millis).unwrap_or(0)))
                }
                // No limit given waits as long as the script may still run.
                _ => script_time_remaining(),
            };
            let outcome = self.future.borrow_mut().get(limit);
            match outcome {
                Ok(Some(results)) => host_to_js(&ctx, Value::Map(results.into_iter().collect())),
                Ok(None) => Err(Exception::throw_message(
                    &ctx,
                    "the service call did not finish in time",
                )),
                Err(err) => Err(throw_context(&ctx, &err)),
            }
        })
    }
}
