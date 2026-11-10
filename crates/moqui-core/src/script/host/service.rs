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

//! `ec.service`: the service call builder (`sync()`, `async()`), the job run builder (`job(name)`), and the future
//! `callFuture()` gives. Each builder method changes the builder and returns it, as Java's. There is no `distribute`
//! effect: a call runs on this server.

use std::collections::BTreeMap;
use std::sync::{Mutex, RwLock};

use moqui_lib::object::{HostObject, Value};

use super::{lock_state, method, with_ec, Arg, Call, HostClass, HostError, Ret};
use crate::context::ErrorKind;
use crate::service::call::CallOptions;

pub(super) static SERVICE_FACADE: HostClass = HostClass {
    name: "host:ServiceFacade",
    methods: &[
        method!("ec.service", "sync", [], sync),
        method!("ec.service", "async", [], async_call),
        method!("ec.service", "job", [Text], job),
        // a generated <service-call>, one crossing for the builder chain (plans/Profiling3.md S3a)
        method!(
            "ec.service",
            "callXml",
            [Text, Int, Value, Parameters, Value],
            Release,
            call_xml
        ),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

pub(super) static SERVICE_CALL: HostClass = HostClass {
    name: "host:ServiceCall",
    methods: &[
        method!("ec.service.sync()", "name", [Text], name),
        method!("ec.service.sync()", "disableAuthz", [], disable_authz),
        method!("ec.service.sync()", "multi", [Value], multi),
        method!("ec.service.sync()", "throwOnError", [Bool], throw_on_error),
        method!("ec.service.sync()", "ignoreTransaction", [Value], ignore_transaction),
        method!(
            "ec.service.sync()",
            "requireNewTransaction",
            [Value],
            require_new_transaction
        ),
        method!("ec.service.sync()", "transactionTimeout", [Whole], transaction_timeout),
        method!("ec.service.sync()", "distribute", [OptValue], distribute),
        method!("ec.service.sync()", "softValidate", [Value], soft_validate),
        method!(
            "ec.service.sync()",
            "ignorePreviousError",
            [OptValue],
            ignore_previous_error
        ),
        method!("ec.service.sync()", "noRememberParameters", [], no_remember_parameters),
        method!(
            "ec.service.sync()",
            "useTransactionCache",
            [Value],
            use_transaction_cache
        ),
        method!("ec.service.sync()", "parameters", [Parameters], parameters),
        method!("ec.service.sync()", "parameter", [Text, Value], parameter),
        method!("ec.service.sync()", "callFuture", [], Release, call_future),
        method!("ec.service.sync()", "call", [], Release, call),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

pub(super) static SERVICE_JOB: HostClass = HostClass {
    name: "host:ServiceJob",
    methods: &[
        method!("ec.service.job()", "parameters", [Value], job_parameters),
        method!("ec.service.job()", "parameter", [Text, Value], job_parameter),
        method!("ec.service.job()", "localOnly", [OptValue], local_only),
        method!("ec.service.job()", "run", [], Release, run_job),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

pub(super) static SERVICE_FUTURE: HostClass = HostClass {
    name: "host:ServiceFuture",
    methods: &[
        method!("ec.service.sync().callFuture()", "isDone", [], is_done),
        method!("ec.service.sync().callFuture()", "get", [OptWhole], future_get),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

struct ServiceCall {
    name: String,
    parameters: BTreeMap<String, Value>,
    options: CallOptions,
    is_async: bool,
}

/// A call builder's state. A read lock is held while the call runs, so a change from a nested script is refused as
/// "the service call is running" (the `RefCell` borrow of before), and never waits.
type Builder = RwLock<ServiceCall>;

fn fresh(is_async: bool) -> Ret {
    let call = ServiceCall {
        name: String::new(),
        parameters: BTreeMap::new(),
        options: CallOptions::default(),
        is_async,
    };
    Ret::Object(HostObject::new(Builder::new(call)), &SERVICE_CALL)
}

fn sync(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(fresh(false))
}

fn async_call(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(fresh(true))
}

/// Change the builder and return it.
fn changed(call: &Call<'_>, change: impl FnOnce(&mut ServiceCall)) -> Result<Ret, HostError> {
    let builder = call.this::<Builder>()?;
    let mut state = builder
        .try_write()
        .map_err(|_| HostError::message("the service call is running"))?;
    change(&mut state);
    Ok(Ret::This)
}

/// A JavaScript boolean argument, `None` for any other value.
fn flag(call: &Call<'_>, index: usize) -> Option<bool> {
    match call.value(index) {
        Some(Value::Bool(flag)) => Some(*flag),
        _ => None,
    }
}

fn name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default().to_string();
    changed(call, |state| state.name = name)
}

fn disable_authz(call: &mut Call<'_>) -> Result<Ret, HostError> {
    changed(call, |state| state.options.disable_authz = true)
}

/// `multi(true)`: one call for each row of `name_N` parameters, in one transaction, as Java's multi form submit.
fn multi(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0).unwrap_or(false);
    changed(call, |state| state.options.multi = on)
}

/// Preserve typed errors for protocol adapters; legacy message/result behavior remains the default.
fn throw_on_error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = call.bool(0).unwrap_or(false);
    changed(call, |state| state.options.throw_on_error = on)
}

fn ignore_transaction(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |state| state.options.ignore_transaction = on)
}

fn require_new_transaction(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |state| state.options.require_new_transaction = on)
}

fn transaction_timeout(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let seconds = i64::from(call.int(0).unwrap_or_default());
    changed(call, |state| {
        state.options.timeout = u64::try_from(seconds).ok().map(std::time::Duration::from_secs);
    })
}

/// `distribute(true)`: Java sends the call to another server of a cluster. This port runs a call on this server's
/// service workers, so the call is the same as without it.
fn distribute(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::This)
}

/// `softValidate(true)`: input that fails validation is danger messages, not errors, and the call returns no results.
fn soft_validate(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0).unwrap_or(true);
    changed(call, |state| state.options.soft_validate = on)
}

/// `ignorePreviousError(true)`: run even with errors already in the message facade; they come back after.
fn ignore_previous_error(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0).unwrap_or(true);
    changed(call, |state| state.options.ignore_previous_error = on)
}

/// `noRememberParameters()`: the artifact hit of this call does not keep its parameters.
fn no_remember_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    changed(call, |state| state.options.no_remember_parameters = true)
}

fn use_transaction_cache(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let on = flag(call, 0);
    changed(call, |state| state.options.use_transaction_cache = on)
}

/// Add the entries of a map. `ec.context` is the whole context, read from the stack as `context` reads it.
fn parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entries = match call.arg(0) {
        Arg::WholeContext => match declared_from_context(call)? {
            Some(entries) => entries,
            None => {
                let mut visible = call.ops.whole_context()?;
                visible.retain(|_, value| !matches!(value, Value::Bytes(_)));
                visible
            }
        },
        // a plain object, or an entity value whose fields are the parameters
        _ => match crate::script::value_map(&call.value_or_null(0)) {
            Some(entries) => entries,
            None => return Ok(Ret::This),
        },
    };
    changed(call, |state| state.parameters.extend(entries))
}

/// The in-parameters of the named service read from the context, when they are all the call can use
/// ([`crate::service::call::context_in_params`]); None when the whole context is needed.
fn declared_from_context(call: &mut Call<'_>) -> Result<Option<moqui_lib::object::ValueMap>, HostError> {
    let (name, multi) = {
        let builder = call.this::<Builder>()?;
        let state = builder
            .try_read()
            .map_err(|_| HostError::message("the service call is running"))?;
        (state.name.clone(), state.options.multi)
    };
    declared_for(call, &name, multi)
}

/// [`declared_from_context`] of the service `name`: a multi call reads each row's `name_N`, which only the whole
/// context has.
fn declared_for(
    call: &mut Call<'_>,
    name: &str,
    multi: bool,
) -> Result<Option<moqui_lib::object::ValueMap>, HostError> {
    if multi {
        return Ok(None);
    }
    let Some(params) = crate::service::call::context_in_params(name) else {
        return Ok(None);
    };
    // the values the script holds are written back first, as the whole context's read does
    if call.ops.flush(super::FlushBefore::Keep).is_err() {
        return Ok(None);
    }
    let names = params
        .iter()
        .map(|param| param.name.as_str())
        .filter(|name| !name.is_empty())
        .chain(crate::service::call::CALL_INPUT_NAMES);
    let mut entries = moqui_lib::object::ValueMap::default();
    for name in names {
        if entries.contains_key(name) {
            continue;
        }
        match crate::script::context_value(name) {
            Some(Value::Bytes(_)) | None => {}
            Some(value) => {
                entries.insert(name.to_string(), value);
            }
        }
    }
    Ok(Some(entries))
}

fn parameter(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default().to_string();
    let value = call.value_or_null(1);
    changed(call, |state| {
        state.parameters.insert(name, value);
    })
}

/// The builder's state for the call, read-locked; a change under way is refused.
fn running<'a>(builder: &'a Builder) -> Result<std::sync::RwLockReadGuard<'a, ServiceCall>, HostError> {
    builder
        .try_read()
        .map_err(|_| HostError::message("the service call is running"))
}

/// Start the service on a service worker and return a future for its out parameters.
fn call_future(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = call.this::<Builder>()?;
    let state = running(&builder)?;
    match crate::service::call::start_future(&state.name, &state.parameters) {
        Ok(future) => Ok(Ret::Object(HostObject::new(Mutex::new(future)), &SERVICE_FUTURE)),
        Err(err) => {
            let message = err.message().to_string();
            with_ec(|ec| ec.message().add_error(message))?;
            Ok(Ret::Value(Value::Null))
        }
    }
}

/// Run the service. As Java's, a failure of its body is in the message facade and the result is the results as they
/// were, empty. An authorization failure is thrown, with its kind, as Java's `ArtifactAuthorizationException` passes
/// through the script. Another error of the call, which Java throws too (an unknown service), is added to the message
/// facade and the result is null. Either way the generated code's `hasError()` check decides.
fn call(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let builder = call.this::<Builder>()?;
    // the parameters of a plain call go with it, validated in place, not copied: a builder is called once
    let owned = {
        let mut state = builder
            .try_write()
            .map_err(|_| HostError::message("the service call is running"))?;
        let plain = !state.is_async && !state.options.multi && !state.options.throw_on_error;
        plain.then(|| std::mem::take(&mut state.parameters))
    };
    let state = running(&builder)?;
    if state.is_async {
        if let Err(err) = crate::service::call::async_call(&state.name, &state.parameters) {
            let message = err.message().to_string();
            with_ec(|ec| ec.message().add_error(message))?;
        }
        return Ok(Ret::Value(Value::Null));
    }
    let called = if state.options.multi {
        crate::service::call::sync_multi(&state.name, &state.parameters, &state.options)
    } else if state.options.throw_on_error {
        crate::service::call::sync_checked(&state.name, &state.parameters, &state.options)
    } else {
        crate::service::call::sync_with_owned(&state.name, owned.unwrap_or_default(), &state.options)
    };
    match called {
        Ok(results) => Ok(Ret::Value(Value::Map(results.into_iter().collect()))),
        Err(err) if state.options.throw_on_error || err.kind() == ErrorKind::Unauthorized => {
            Err(HostError::Context(err))
        }
        Err(err) => {
            // the client sees the message; the server log has where it came from
            tracing::error!(target: "moqui_core::script::runtime::services", "Service {} failed: {}", state.name, err.diagnostic());
            let message = crate::context::client_text(&err.to_string());
            with_ec(|ec| ec.message().add_error(message))?;
            Ok(Ret::Value(Value::Null))
        }
    }
}

/// The bits of [`call_xml`]'s options, as XmlActions.js's service-call macro sets them.
const XML_DISABLE_AUTHZ: i32 = 1;
const XML_MULTI: i32 = 2;
const XML_IGNORE_TRANSACTION: i32 = 4;
const XML_REQUIRE_NEW: i32 = 8;
const XML_SETS_CACHE: i32 = 16;
const XML_CACHE: i32 = 32;

/// `ec.service.callXml(name, flags, timeout, inMap, fields)`: a generated `<service-call>` in one crossing, where the
/// builder chain (`sync().name(..)`, an option each, `.parameters(..)`, a `.parameter(..)` each, `.call()`) was five or
/// more (plans/Profiling3.md S3a). `flags` are the options' bits, `timeout` the transaction timeout in seconds or null,
/// `inMap` the in-map (`ec.context` is the whole context, read as `parameters` reads it) or null, and `fields` the
/// field-maps as an object or null, over the in-map. The result is `call()`'s.
fn call_xml(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let flags = call.int(1).unwrap_or(0);
    let on = |bit: i32| flags & bit != 0;
    let mut options = CallOptions {
        disable_authz: on(XML_DISABLE_AUTHZ),
        multi: on(XML_MULTI),
        ..CallOptions::default()
    };
    if on(XML_IGNORE_TRANSACTION) {
        options.ignore_transaction = Some(true);
    }
    if on(XML_REQUIRE_NEW) {
        options.require_new_transaction = Some(true);
    }
    if on(XML_SETS_CACHE) {
        options.use_transaction_cache = Some(on(XML_CACHE));
    }
    let seconds = match call.value(2) {
        Some(Value::I32(seconds)) => Some(i64::from(*seconds)),
        Some(Value::I64(seconds)) => Some(*seconds),
        // a whole number of seconds, as transactionTimeout takes it
        #[allow(clippy::cast_possible_truncation)]
        Some(Value::F64(seconds)) if seconds.is_finite() => Some(*seconds as i64),
        _ => None,
    };
    if let Some(seconds) = seconds {
        options.timeout = u64::try_from(seconds).ok().map(std::time::Duration::from_secs);
    }
    let mut parameters: BTreeMap<String, Value> = BTreeMap::new();
    match call.arg(3) {
        Arg::WholeContext => {
            let entries = match declared_for(call, &name, options.multi)? {
                Some(entries) => entries,
                None => {
                    let mut visible = call.ops.whole_context()?;
                    visible.retain(|_, value| !matches!(value, Value::Bytes(_)));
                    visible
                }
            };
            parameters.extend(entries);
        }
        _ => {
            if let Some(entries) = crate::script::value_map(&call.value_or_null(3)) {
                parameters.extend(entries);
            }
        }
    }
    if let Some(fields) = crate::script::value_map(&call.value_or_null(4)) {
        parameters.extend(fields);
    }
    let called = if options.multi {
        crate::service::call::sync_multi(&name, &parameters, &options)
    } else {
        crate::service::call::sync_with_owned(&name, parameters, &options)
    };
    match called {
        Ok(results) => Ok(Ret::Value(Value::Map(results.into_iter().collect()))),
        Err(err) if err.kind() == ErrorKind::Unauthorized => Err(HostError::Context(err)),
        Err(err) => {
            tracing::error!(target: "moqui_core::script::runtime::services", "Service {name} failed: {}", err.diagnostic());
            let message = crate::context::client_text(&err.to_string());
            with_ec(|ec| ec.message().add_error(message))?;
            Ok(Ret::Value(Value::Null))
        }
    }
}

/// A job run builder's state: the job and the parameters over its own.
struct JobRun {
    job_name: String,
    parameters: Mutex<BTreeMap<String, Value>>,
}

/// `ec.service.job(jobName)`: a builder for running a `ServiceJob` now, outside its schedule, as Java's `ServiceCallJob`.
fn job(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let run = JobRun {
        job_name: call.text(0).unwrap_or_default().to_string(),
        parameters: Mutex::new(BTreeMap::new()),
    };
    Ok(Ret::Object(HostObject::new(run), &SERVICE_JOB))
}

fn job_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let run = call.this::<JobRun>()?;
    if let Some(entries) = crate::script::value_map(&call.value_or_null(0)) {
        lock_state(&run.parameters, "service job")?.extend(entries);
    }
    Ok(Ret::This)
}

fn job_parameter(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let run = call.this::<JobRun>()?;
    let name = call.text(0).unwrap_or_default().to_string();
    lock_state(&run.parameters, "service job")?.insert(name, call.value_or_null(1));
    Ok(Ret::This)
}

/// Java's: run on this server, not on another of the cluster. A job here always runs on this server.
fn local_only(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::This)
}

/// Start the job with the job's own parameters and these over them; its run id. A job that cannot start (no such job,
/// a full pool) is an error in the message facade and the result is null, as a failed service call is.
fn run_job(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let run = call.this::<JobRun>()?;
    let parameters = lock_state(&run.parameters, "service job")?.clone();
    match crate::service::run_job_now(&run.job_name, &parameters) {
        Ok(run_id) => Ok(run_id.into()),
        Err(err) => {
            tracing::error!(target: "moqui_core::script::runtime::services", "Service job {} did not start: {}", run.job_name, err.diagnostic());
            let message = crate::context::client_text(&err.to_string());
            with_ec(|ec| ec.message().add_error(message))?;
            Ok(Ret::Value(Value::Null))
        }
    }
}

type Future = Mutex<crate::service::ServiceFuture>;

fn is_done(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let future = call.this::<Future>()?;
    let done = lock_state(&future, "service future")?.is_done();
    Ok(done.into())
}

/// The out parameters. With a timeout in milliseconds, or with none the time the script has left, a call that is not
/// done in time throws.
fn future_get(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let future = call.this::<Future>()?;
    let limit = match call.int(0) {
        Some(millis) => Some(std::time::Duration::from_millis(u64::try_from(millis).unwrap_or(0))),
        // No limit given waits as long as the script may still run.
        None => crate::script::runtime::script_time_remaining(),
    };
    let outcome = lock_state(&future, "service future")?.get(limit);
    match outcome {
        Ok(Some(results)) => Ok(Ret::Value(Value::Map(results.into_iter().collect()))),
        Ok(None) => Err(HostError::message("the service call did not finish in time")),
        Err(err) => Err(HostError::Context(err)),
    }
}
