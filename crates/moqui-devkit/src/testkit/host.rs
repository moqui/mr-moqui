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

//! `__moquiTestHost`, the calls the preludes make into the harness, as a class of the engine-neutral host table
//! (`moqui_core::script::host`), so each script engine's binder installs the same methods
//! ([`moqui_core::script::install_spec_global`]):
//!
//! ```text
//! __moquiTestHost.parallel(n, source, argsJson, transaction, timeoutSeconds) -> '{"results": [...], "errors": [...]}'
//! __moquiTestHost.uploadedFile(fileName, text, contentType?) -> an uploaded file
//! __moquiTestHost.setProperty(name, value | null) -> the property as it was, or null
//! __moquiTestHost.call(name, argsJson) -> the client host's answer (moqui test-client)
//! __moquiTestHost.settle(promise) -> {status: "fulfilled" | "rejected", value}
//! ```

use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContext;
use moqui_core::host_method;
use moqui_core::script::host::{Call, HostClass, HostError, Ret};

pub(super) static TEST_HOST: HostClass = HostClass {
    name: "host:MoquiTestHost",
    methods: &[
        // its threads run their own engines; this thread only waits
        host_method!(
            "__moquiTestHost",
            "parallel",
            [Int, Text, Text, Bool, Number],
            None,
            leaf,
            parallel
        ),
        host_method!(
            "__moquiTestHost",
            "uploadedFile",
            [Text, Text, OptNullableText],
            None,
            leaf,
            uploaded_file
        ),
        host_method!(
            "__moquiTestHost",
            "setProperty",
            [Text, OptNullableText],
            None,
            leaf,
            set_property
        ),
        host_method!("__moquiTestHost", "completionBody", [Text], None, leaf, completion_body),
        // a client host's call may reach anything
        host_method!("__moquiTestHost", "call", [Text, Text], None, call),
    ],
    getters: &[],
    hidden: &[host_method!("__moquiTestHost", "runJob", [], None, leaf, run_job)],
    shim_js: Some(SETTLE_JS),
    shim_members: &["settle"],
};

/// `settle(promise)`: run the engine's jobs until the promise fulfills or rejects; `{ status, value }`. A promise still
/// pending when no job is left, or after 10,000 jobs, throws. The engines run no job unless asked
/// (`assert.rejects` calls this).
const SETTLE_JS: &str = r#"(function (proto, hidden) {
    Object.defineProperty(proto, "settle", {
        value: function settle(promise) {
            if (!(promise instanceof Promise)) throw new Error("settle: not a promise");
            let settled = null;
            promise.then(
                (value) => { settled = { status: "fulfilled", value }; },
                (value) => { settled = { status: "rejected", value }; }
            );
            for (let jobs = 0; settled === null; jobs++) {
                if (jobs === 10000 || !hidden.runJob()) throw new Error("settle: the promise is still pending");
            }
            return settled;
        },
        writable: true, enumerable: false, configurable: true
    });
})"#;

fn run_job(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(call.ops.run_pending_job()?.into())
}

/// `parallel(n, source, argsJson, transaction, timeoutSeconds)`: the function source on `n` threads, each with its own
/// execution context and engine; `{results, errors}` as JSON.
fn parallel(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let count = u32::try_from(call.int(0).unwrap_or(0)).unwrap_or(0);
    let source = call.text(1).unwrap_or_default().to_string();
    let args = call.text(2).unwrap_or_default().to_string();
    let transaction = call.bool(3).unwrap_or(false);
    let timeout_seconds = call.number(4).unwrap_or(0.0);
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| HostError::message("parallel needs an execution context"))?;
    let timeout = Duration::from_secs_f64(timeout_seconds.max(1.0));
    Ok(super::run::run_parallel(&factory, count, &source, &args, transaction, timeout).into())
}

/// `uploadedFile(fileName, text, contentType)` of the prelude: an uploaded file holding the text.
fn uploaded_file(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let file_name = call.text(0).unwrap_or_default();
    let text = call.text(1).unwrap_or_default();
    let content_type = call.text(2).unwrap_or("text/plain");
    Ok(moqui_core::script::host::text_upload(
        "contentFile",
        file_name,
        content_type,
        text,
    ))
}

/// `setProperty(name, value)`: a process property (`null` clears it); the property as it was, from the process table
/// only, or `null`. Under `setProperty` and `withProperty` (server.js), which keep what to put back when the spec ends
/// (`__moquiTest.restoreProperties`, in cleanupSpec and in [`super::run::run_spec`]'s finish).
fn set_property(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default().to_string();
    let before = moqui_lib::env::get_property(&name);
    match call.text(1) {
        Some(value) => moqui_lib::env::set_property(&name, value.to_string()),
        None => moqui_lib::env::clear_property(&name),
    }
    Ok(before.into())
}

/// `completionBody(source)`: the script as a function body that returns its value (`runInContext` on an engine
/// without `with`, server.js).
fn completion_body(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(moqui_core::script::parse::completion_body(call.text(0).unwrap_or_default()).into())
}

/// `call(name, argsJson)`: the call of the client host set on this thread, its error thrown as the script's.
fn call(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default();
    let args = call.text(1).unwrap_or_default();
    let Some(host) = super::run::current_host() else {
        return Err(HostError::message(format!(
            "{name}: a client test host is not running (moqui test-client)"
        )));
    };
    host.call(name, args).map(Ret::from).map_err(HostError::Message)
}
