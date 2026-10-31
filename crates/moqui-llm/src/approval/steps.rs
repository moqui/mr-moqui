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

//! `org.moqui.impl.SimServices.run#Steps` (docs/ai/Assist.md, Sim steps): service calls run one after another in one
//! sim run, as the current user and under the sim fence, as an approval's preview runs its write ([`super::preview`]).
//! Each step runs under its own savepoint ([`moqui_core::entity::sim_step`]): a step that fails (an error, a refused
//! outside effect, a transaction it rolled back) is rolled back to it and the next step runs on what the steps before
//! it left. Each step answers the rows it changed, against the values before it, and the keys it created; an optional
//! read service runs after each step and after all of them, under a savepoint that is always rolled back, so a read
//! changes nothing the steps see.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::{json, Value as Json};

use super::preview::{compared_to, key_json, production_row, MAX_ROWS};
use crate::run;
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::cache::{self, SimChange};
use moqui_core::entity::{fence, sim_step, EntityFacade};
use moqui_core::llm_hooks::RustServiceFn;
use moqui_core::rust_service;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, run_steps)
}

/// A service call of a sim run: a step or the read.
#[derive(Clone, Debug, Default)]
pub struct SimCall {
    pub service_name: String,
    pub parameters: BTreeMap<String, Value>,
}

/// What one step came to.
#[derive(Debug)]
pub struct StepOutcome {
    pub ok: bool,
    pub error: Option<String>,
    /// The rows the step changed, at most [`MAX_ROWS`], in the preview row shape.
    pub rows: Vec<Json>,
    /// The step changed more rows than `rows` lists.
    pub more: bool,
    /// `{entity, key}` of each row the step created.
    pub created_keys: Vec<Json>,
    /// The read after this step, when asked for.
    pub read: Option<Result<BTreeMap<String, Value>, String>>,
}

/// What a sim run of steps came to. Not `available` when this process has no sim overlay or it could not be entered;
/// then nothing ran.
#[derive(Debug, Default)]
pub struct StepsRun {
    pub available: bool,
    pub steps: Vec<StepOutcome>,
    pub read: Option<Result<BTreeMap<String, Value>, String>>,
}

/// What the run asks for.
pub struct StepsRequest<'a> {
    pub username: &'a str,
    pub label: &'a str,
    pub steps: &'a [SimCall],
    pub read: Option<&'a SimCall>,
    pub read_after_each_step: bool,
}

/// Run `request`'s steps in one sim run on a thread of its own, logged in as its user.
pub fn run(factory: &Arc<ExecutionContextFactory>, request: &StepsRequest<'_>) -> StepsRun {
    std::thread::scope(|scope| {
        scope.spawn(|| in_sim(factory, request)).join().unwrap_or_else(|_| {
            tracing::error!("the sim run of {} failed unexpectedly", request.label);
            StepsRun::default()
        })
    })
}

/// The values a step started from for a row it changed.
enum Before {
    /// The sim had changed the row already: its values there, none when deleted.
    Sim(Option<BTreeMap<String, Value>>),
    /// The sim had not touched it, so it is production's row, read after the sim is left.
    Production,
}

/// A row a step changed, as found in the sim.
struct Changed {
    change: SimChange,
    before: Before,
    visible: bool,
}

/// A step as found in the sim, before its rows are compared outside it.
struct Pending {
    error: Option<String>,
    changed: Vec<Changed>,
    read: Option<Result<BTreeMap<String, Value>, String>>,
}

type RowKey = (String, Vec<(String, String)>);

fn in_sim(factory: &Arc<ExecutionContextFactory>, request: &StepsRequest<'_>) -> StepsRun {
    let facade = match run::facade_of(factory) {
        Ok(facade) => facade,
        Err(error) => {
            tracing::warn!("the sim run of {} has no entity facade: {error}", request.label);
            return StepsRun::default();
        }
    };
    if !facade.sim_available() {
        return StepsRun::default();
    }
    let context = factory.new_context();
    let _entered = context.enter();
    {
        let _authz = context.disable_authz();
        if let Err(error) = moqui_core::user::internal_login_user(request.username) {
            tracing::warn!("the sim run of {} could not log in: {error}", request.label);
            return StepsRun::default();
        }
    }
    if let Err(error) = cache::enter_sim(&facade) {
        tracing::warn!("the sim run of {} could not enter the sim: {error}", request.label);
        return StepsRun::default();
    }
    let ran = held_steps(&facade, request);
    if let Err(error) = cache::exit_sim(&facade) {
        tracing::error!("the sim of {} was not dropped: {error}", request.label);
    }
    let Some((pending, read)) = ran else {
        return StepsRun::default();
    };
    // Production values are read now, outside the sim, as the preview reads them.
    let steps = pending.into_iter().map(|step| compare_step(&facade, step)).collect();
    StepsRun {
        available: true,
        steps,
        read,
    }
}

/// The steps and reads inside the held overlay transaction; none when it could not be held.
#[allow(clippy::type_complexity)]
fn held_steps(
    facade: &EntityFacade,
    request: &StepsRequest<'_>,
) -> Option<(Vec<Pending>, Option<Result<BTreeMap<String, Value>, String>>)> {
    let hold = match sim_step::hold(facade) {
        Ok(hold) => hold,
        Err(error) => {
            tracing::warn!("the sim run of {} could not hold the overlay: {error}", request.label);
            return None;
        }
    };
    let _fence = fence::enter_sim(request.label.to_string());
    let mut invisible: HashSet<RowKey> = HashSet::new();
    let mut pending = Vec::with_capacity(request.steps.len());
    for (index, call) in request.steps.iter().enumerate() {
        let label = format!("{} step {}", request.label, index + 1);
        let mut step = run_step(facade, call, &label, &mut invisible);
        if request.read_after_each_step {
            step.read = request.read.map(|read| run_read(facade, read, &label));
        }
        pending.push(step);
    }
    let read = request
        .read
        .map(|read| run_read(facade, read, &format!("{} read", request.label)));
    if let Err(error) = hold.end() {
        tracing::warn!(
            "the sim run of {} did not end its overlay transaction: {error}",
            request.label
        );
    }
    Some((pending, read))
}

fn run_step(facade: &EntityFacade, call: &SimCall, label: &str, invisible: &mut HashSet<RowKey>) -> Pending {
    let failed = |error: String| Pending {
        error: Some(error),
        changed: Vec::new(),
        read: None,
    };
    let before = match cache::sim_changes(facade) {
        Ok(changes) => by_key(changes),
        Err(error) => return failed(format!("the sim could not be read: {error}")),
    };
    let savepoint = match sim_step::savepoint(facade) {
        Ok(savepoint) => savepoint,
        Err(error) => return failed(format!("the step could not be started: {error}")),
    };
    let outcome = fenced_call(call, label).and_then(|_| {
        if let Some(reason) = cache::sim_poisoned() {
            return Err(reason);
        }
        let after = cache::sim_changes(facade).map_err(|error| format!("the sim could not be read: {error}"))?;
        Ok(step_changes(facade, &before, after, invisible))
    });
    match outcome {
        Ok(changed) => match savepoint.release() {
            Ok(()) => Pending {
                error: None,
                changed,
                read: None,
            },
            Err(error) => failed(format!("the step could not be kept: {error}")),
        },
        Err(error) => {
            if let Err(rollback) = savepoint.rollback() {
                tracing::error!("{label} was not rolled back: {rollback}");
            }
            failed(error)
        }
    }
}

/// Call `call` under a fence of its own, so its refused outside effects are its own; the errors it left in the
/// message facade are its error and are cleared, so the next call runs.
fn fenced_call(call: &SimCall, label: &str) -> Result<BTreeMap<String, Value>, String> {
    if call.service_name.trim().is_empty() {
        return Err("a step needs a serviceName".to_string());
    }
    let (ran, effects) = {
        let fence = fence::enter_sim(label.to_string());
        let ran = moqui_core::service::call::sync_checked(
            &call.service_name,
            &call.parameters,
            &moqui_core::service::call::CallOptions::default(),
        );
        let effects: Vec<String> = fence.violations().iter().map(fence::Violation::describe).collect();
        (ran, effects)
    };
    ExecutionContext::with_current(|context| context.message().clear_errors());
    if !effects.is_empty() {
        return Err(format!(
            "it would do what a sim cannot simulate: {}",
            effects.join(", ")
        ));
    }
    ran.map_err(|error| error.to_string())
}

/// The read service under a savepoint that is always rolled back: its out-parameters, plain values only.
fn run_read(facade: &EntityFacade, call: &SimCall, label: &str) -> Result<BTreeMap<String, Value>, String> {
    let savepoint = sim_step::savepoint(facade).map_err(|error| format!("the read could not be started: {error}"))?;
    let ran = fenced_call(call, label);
    if let Err(error) = savepoint.rollback() {
        tracing::error!("the read of {label} was not rolled back: {error}");
    }
    ran.map(|out| out.into_iter().map(|(name, value)| (name, plain(value))).collect())
}

/// A value without host objects, read while still in the sim: an entity value becomes a map of its fields and an
/// entity list a list of them; any other object (a find, a handle) would read production once the sim is left, so it
/// is dropped.
fn plain(value: Value) -> Value {
    match value {
        Value::Object(_) => {
            if let Some(fields) = moqui_core::script::value_map(&value) {
                Value::Map(fields.into_iter().map(|(key, item)| (key, plain(item))).collect())
            } else if let Some(items) = moqui_core::script::value_items(&value) {
                Value::List(items.into_iter().map(plain).collect())
            } else {
                Value::Null
            }
        }
        Value::List(items) => Value::List(items.into_iter().map(plain).collect()),
        Value::Map(entries) => Value::Map(entries.into_iter().map(|(key, item)| (key, plain(item))).collect()),
        other => other,
    }
}

fn by_key(changes: Vec<SimChange>) -> HashMap<RowKey, Option<BTreeMap<String, Value>>> {
    changes
        .into_iter()
        .map(|change| ((change.entity, change.key), change.values))
        .collect()
}

/// The rows whose values in the sim differ from those before the step, each checked for the user's view in the sim,
/// as the preview checks its after-values.
fn step_changes(
    facade: &EntityFacade,
    before: &HashMap<RowKey, Option<BTreeMap<String, Value>>>,
    after: Vec<SimChange>,
    invisible: &mut HashSet<RowKey>,
) -> Vec<Changed> {
    let mut changed = Vec::new();
    for change in after {
        let key: RowKey = (change.entity.clone(), change.key.clone());
        let before = match before.get(&key) {
            Some(values) if *values == change.values => continue,
            Some(values) => Before::Sim(values.clone()),
            None => Before::Production,
        };
        let mut visible = change.values.is_none()
            || match production_row(facade, &change) {
                Ok(row) => row.is_some(),
                Err(error) => {
                    tracing::warn!(entity = %change.entity, error = %error, "sim step values could not be authorized");
                    false
                }
            };
        // values the user could not see after an earlier step are not shown as this step's before
        if matches!(before, Before::Sim(_)) && invisible.contains(&key) {
            visible = false;
        }
        if !visible {
            invisible.insert(key);
        }
        changed.push(Changed {
            change,
            before,
            visible,
        });
    }
    changed
}

fn compare_step(facade: &EntityFacade, step: Pending) -> StepOutcome {
    let mut rows = Vec::new();
    let mut created_keys = Vec::new();
    for changed in &step.changed {
        let row = if changed.visible {
            match &changed.before {
                Before::Sim(values) => compared_to(facade, &changed.change, values.as_ref()),
                Before::Production => match production_row(facade, &changed.change) {
                    Ok(production) => compared_to(facade, &changed.change, production.as_ref()),
                    Err(error) => {
                        tracing::warn!(entity = %changed.change.entity, error = %error,
                            "sim step production values could not be authorized");
                        json!({"entity": changed.change.entity, "action": "unavailable",
                            "error": "production values could not be read"})
                    }
                },
            }
        } else {
            json!({"entity": changed.change.entity, "action": "unavailable",
                "error": "values are not visible to the user"})
        };
        // a write that left every field as it was is no change
        if row["action"] == "update" && row["fields"].as_array().is_some_and(Vec::is_empty) {
            continue;
        }
        if row["action"] == "create" {
            created_keys.push(json!({"entity": changed.change.entity, "key": key_json(&changed.change)}));
        }
        rows.push(row);
    }
    let more = rows.len() > MAX_ROWS;
    rows.truncate(MAX_ROWS);
    StepOutcome {
        ok: step.error.is_none(),
        error: step.error,
        rows,
        more,
        created_keys,
        read: step.read,
    }
}

/// A service call from a `{serviceName, parameters}` map; none when the value is not a map.
fn sim_call(value: &Value) -> Option<SimCall> {
    let Value::Map(map) = value else {
        return None;
    };
    let service_name = map.get("serviceName").map(to_plain_string).unwrap_or_default();
    let parameters = match map.get("parameters") {
        Some(Value::Map(parameters)) => parameters
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
        Some(Value::String(text)) if !text.trim().is_empty() => match serde_json::from_str::<Json>(text) {
            Ok(json) => match moqui_lib::json::from_json(&json) {
                Value::Map(parameters) => parameters.into_iter().collect(),
                _ => BTreeMap::new(),
            },
            Err(_) => BTreeMap::new(),
        },
        _ => BTreeMap::new(),
    };
    Some(SimCall {
        service_name: service_name.trim().to_string(),
        parameters,
    })
}

/// A list parameter, or a JSON list in text as a form or REST call sends it.
fn list_of(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::List(items)) => items.clone(),
        Some(Value::String(text)) if !text.trim().is_empty() => match serde_json::from_str::<Json>(text) {
            Ok(json) => match moqui_lib::json::from_json(&json) {
                Value::List(items) => items,
                _ => Vec::new(),
            },
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn map_of(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(map @ Value::Map(_)) => Some(map.clone()),
        Some(Value::String(text)) if !text.trim().is_empty() => serde_json::from_str::<Json>(text)
            .ok()
            .map(|json| moqui_lib::json::from_json(&json))
            .filter(|value| matches!(value, Value::Map(_))),
        _ => None,
    }
}

fn read_value(read: &Result<BTreeMap<String, Value>, String>) -> Value {
    match read {
        Ok(out) => Value::Map(out.iter().map(|(name, value)| (name.clone(), value.clone())).collect()),
        Err(_) => Value::Map(Default::default()),
    }
}

/// `run#Steps`: the steps in one sim run as the current user. A step that is not a `{serviceName, parameters}` map
/// fails alone.
fn run_steps(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Result<BTreeMap<String, Value>, Error> {
    let factory = ExecutionContext::with_current(|context| Arc::clone(context.factory()))
        .ok_or_else(|| Error::new("run#Steps needs an execution context"))?;
    let username = moqui_core::user::current_username()
        .ok()
        .flatten()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::new("run#Steps needs a logged-in user"))?;
    let inputs = list_of(parameters.get("steps"));
    let steps: Vec<SimCall> = inputs.iter().map(|input| sim_call(input).unwrap_or_default()).collect();
    let read = map_of(parameters.get("read")).as_ref().and_then(sim_call);
    let read_after_each_step = parameters
        .get("readAfterEachStep")
        .filter(|value| !is_empty(value))
        .is_some_and(|value| matches!(value, Value::Bool(true)) || to_plain_string(value) == "true");
    let label = parameters
        .get("label")
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_else(|| "a sim run of steps".to_string());
    let ran = run(
        &factory,
        &StepsRequest {
            username: &username,
            label: &label,
            steps: &steps,
            read: read.as_ref(),
            read_after_each_step,
        },
    );
    let mut out = BTreeMap::new();
    out.insert("available".to_string(), Value::Bool(ran.available));
    if !ran.available {
        out.insert("steps".to_string(), Value::List(Vec::new()));
        return Ok(out);
    }
    let steps: Vec<Value> = ran
        .steps
        .iter()
        .map(|step| step_value(step, read_after_each_step))
        .collect();
    out.insert("steps".to_string(), Value::List(steps));
    if let Some(read) = &ran.read {
        out.insert("readResult".to_string(), read_value(read));
        if let Err(error) = read {
            out.insert("readError".to_string(), Value::string(error));
        }
    }
    Ok(out)
}

fn step_value(step: &StepOutcome, read_after_each_step: bool) -> Value {
    let mut map = indexmap::IndexMap::new();
    map.insert("ok".to_string(), Value::Bool(step.ok));
    if let Some(error) = &step.error {
        map.insert("error".to_string(), Value::string(error));
    }
    map.insert(
        "rows".to_string(),
        moqui_lib::json::from_json(&Json::Array(step.rows.clone())),
    );
    if step.more {
        map.insert("moreRows".to_string(), Value::Bool(true));
    }
    map.insert(
        "createdKeys".to_string(),
        moqui_lib::json::from_json(&Json::Array(step.created_keys.clone())),
    );
    if read_after_each_step {
        let read = step.read.as_ref();
        map.insert(
            "readResult".to_string(),
            read.map_or_else(|| Value::Map(Default::default()), read_value),
        );
        if let Some(Err(error)) = read {
            map.insert("readError".to_string(), Value::string(error));
        }
    }
    Value::Map(map)
}

#[cfg(test)]
#[path = "steps_tests.rs"]
mod tests;
