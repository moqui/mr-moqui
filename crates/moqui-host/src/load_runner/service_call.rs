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

//! `service.call(name, parameters, { name, expect })` of a load run in a server (plans/LoadRunner.md §6, D10): on the
//! user's thread, in the ExecutionContext the run made for it ([`super::run::in_user_context`]), logged in as the user
//! who started the run, through the service facade with authorization required and the transaction its definition
//! gives. Timed from the call to its return; the context's error messages are the sample's error, and every message
//! is cleared after the call, so none piles up over the run; a transaction left open is rolled back and is an error.
//! A call that leaves the context changed (authorization or entity ECA rules off, another user) is an error too, and
//! the context is replaced with a fresh one ([`super::run::in_user_context`]).
//!
//! The call goes straight to the service facade, so a service's `allow-remote="false"` does not refuse it: that
//! attribute is about calls from outside the server (REST, JSON-RPC), and this one runs in process, as the user who
//! started the run, with authorization required, as the services the user's own screens call do.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContext;
use moqui_lib::json::{from_json, to_json};
use serde_json::{json, Value};

use crate::test_client::load::{self, Kind, Sample};

/// What one call gave: its out-parameters, its errors, and its time.
struct Called {
    out: BTreeMap<String, moqui_lib::object::Value>,
    errors: Vec<String>,
    elapsed: Duration,
}

/// The service's out-parameters as plain values; an error ends the user's iteration unless `opts.expect` lists
/// `error`, when the answer is `{ error }`.
pub(crate) fn call(args: &Value) -> Result<Value, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Err("service.call needs a service name".to_string());
    }
    let parameters: BTreeMap<String, moqui_lib::object::Value> = args
        .get("parameters")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), from_json(value)))
        .collect();
    let opts = args.get("opts").unwrap_or(&Value::Null);
    let sample_name = opts.get("name").and_then(Value::as_str).unwrap_or(&name).to_string();
    let expected: Vec<&str> = opts
        .get("expect")
        .and_then(Value::as_array)
        .map(|outcomes| outcomes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    load::before_request()?;
    let (mut called, taint) = super::run::in_user_context(|| run_service(&name, &parameters))?;
    if let Some(reason) = taint {
        // the context was replaced; the call that changed it is an error
        called.errors.push(format!("{name}: {reason}"));
    }
    let error = (!called.errors.is_empty()).then(|| called.errors.join("; "));
    let outcome = if error.is_some() { "error" } else { "ok" };
    let counted = error.clone().filter(|_| !expected.contains(&outcome));
    let sample = Sample {
        kind: Kind::Service,
        outcome,
        status: None,
        elapsed: called.elapsed,
        error: counted.clone(),
    };
    load::record(|| sample_name, sample)?;
    if let Some(message) = counted {
        // outside the timed part (setup, teardown) nothing is recorded, and the error still throws
        return Err(format!("{name}: {message}"));
    }
    if let Some(message) = error {
        return Ok(json!({ "error": message }));
    }
    let plain: serde_json::Map<String, Value> = called
        .out
        .iter()
        .map(|(key, value)| (key.clone(), to_json(value)))
        .collect();
    Ok(Value::Object(plain))
}

/// The call, in the entered user context: messages before it set aside, its errors taken, then every message cleared.
fn run_service(name: &str, parameters: &BTreeMap<String, moqui_lib::object::Value>) -> Called {
    clear_messages();
    let started = Instant::now();
    let called = moqui_core::service::sync(name, parameters);
    let elapsed = started.elapsed();
    let mut errors: Vec<String> =
        ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    let out = match called {
        Ok(out) => out,
        Err(err) => {
            errors.push(err.to_string());
            BTreeMap::new()
        }
    };
    if moqui_core::transaction::thread_in_transaction() {
        rollback_open();
        errors.push(format!("{name} left a transaction open; it was rolled back"));
    }
    clear_messages();
    Called { out, errors, elapsed }
}

/// Every message of the context, errors, warnings and infos: a user's calls run for the whole run.
fn clear_messages() {
    ExecutionContext::with_current(|ec| ec.message().clear());
}

/// Roll back what a call left open on this thread: the transaction frames are the thread's, which the default
/// group's manager ends.
fn rollback_open() {
    let transactions =
        ExecutionContext::with_current(|ec| ec.factory().entity().map(|facade| facade.default_transactions()))
            .flatten();
    match transactions {
        Some(Ok(transactions)) => {
            if let Err(err) = transactions.rollback_all_in_thread() {
                tracing::error!(error = %err, "Could not roll back a load run's open transaction");
            }
        }
        Some(Err(err)) => tracing::error!(error = %err, "Could not roll back a load run's open transaction"),
        None => {}
    }
}
