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
//! user's thread, whose ExecutionContext the run entered and logged in as the user who started it, through the
//! service facade with authorization required and the transaction its definition gives. Timed from the call to its
//! return; the context's error messages are the sample's error and are then cleared; a transaction left open is
//! rolled back and is an error.

use std::collections::BTreeMap;
use std::time::Instant;

use moqui_core::context::ExecutionContext;
use moqui_lib::json::{from_json, to_json};
use serde_json::{json, Value};

use crate::test_client::load::{self, Kind, Sample};

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
    if ExecutionContext::with_current(|_| ()).is_none() {
        return Err("service.call needs the run's execution context on this thread".to_string());
    }
    load::before_request()?;
    // errors left by an earlier call or the script are not this call's
    clear_errors();
    let started = Instant::now();
    let called = moqui_core::service::sync(&name, &parameters);
    let elapsed = started.elapsed();
    let mut errors: Vec<String> =
        ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default();
    if let Err(err) = &called {
        errors.push(err.to_string());
    }
    if moqui_core::transaction::thread_in_transaction() {
        rollback_open();
        errors.push(format!("{name} left a transaction open; it was rolled back"));
    }
    clear_errors();
    let error = (!errors.is_empty()).then(|| errors.join("; "));
    let outcome = if error.is_some() { "error" } else { "ok" };
    let counted = error.clone().filter(|_| !expected.contains(&outcome));
    let sample = Sample {
        kind: Kind::Service,
        outcome,
        status: None,
        elapsed,
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
    let out = called.unwrap_or_default();
    let plain: serde_json::Map<String, Value> = out.iter().map(|(key, value)| (key.clone(), to_json(value))).collect();
    Ok(Value::Object(plain))
}

fn clear_errors() {
    ExecutionContext::with_current(|ec| ec.message().clear_errors());
}

/// Roll back what a call left open on this thread: the transaction frames are the thread's, which the default
/// group's manager ends.
fn rollback_open() {
    let transactions =
        ExecutionContext::with_current(|ec| ec.factory().entity().map(|facade| facade.default_transactions()))
            .flatten();
    let Some(transactions) = transactions else {
        return;
    };
    match transactions {
        Ok(transactions) => {
            if let Err(err) = transactions.rollback_all_in_thread() {
                tracing::error!(error = %err, "Could not roll back a load run's open transaction");
            }
        }
        Err(err) => tracing::error!(error = %err, "Could not roll back a load run's open transaction"),
    }
}
