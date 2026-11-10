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

//! Load runs in a server (plans/LoadRunner.md §6, §7, §10), the server side of the Tools load runner under the
//! `test-client` feature: `org.moqui.dev.LoadServices`, Rust services registered through this module's extension
//! entry. The load scripts of the loaded components ([`scripts`]), the running loads and their threads ([`run`]),
//! `service.call` in process ([`service_call`]), the result files of finished runs ([`results`]) and what a run may
//! reach ([`targets`]). Each service checks [`may_run_loads`] whatever calls it, as the SQL services do.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::json;

pub(crate) mod results;
pub(crate) mod run;
pub(crate) mod scripts;
pub(crate) mod service_call;
pub(crate) mod targets;

/// The Tools app, whose all-actions authorization a load run needs beside the permission.
const TOOLS_SCREEN: &str = "component://tools/screen/Tools.xml";
/// The permission of load runs (D9), sealed and on `ADMIN_ADV`.
pub(crate) const PERMISSION: &str = "LOAD_RUNNER";
/// The longest `waitSeconds` of `get#LoadRunStatus`.
const MAX_WAIT_SECONDS: f64 = 30.0;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["test-client"],
        register_services,
        before_shutdown: |_| run::stop_all(),
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, find_load_scripts)?;
    moqui_core::rust_service!(map, start_load_run)?;
    moqui_core::rust_service!(map, stop_load_run)?;
    moqui_core::rust_service!(map, get_load_run_status)?;
    moqui_core::rust_service!(map, find_load_runs)?;
    moqui_core::rust_service!(map, get_load_run_result)?;
    moqui_core::rust_service!(map, delete_load_run)?;
    moqui_core::rust_service!(map, import_load_run_result)?;
    moqui_core::rust_service!(map, compare_load_runs)?;
    Ok(())
}

type Out = Result<BTreeMap<String, Value>, LibError>;

/// Load runs need `LOAD_RUNNER` and authorization for all actions on the Tools app (§10), as the screens check:
/// asked here whatever calls the service. When not, an error is added.
pub(crate) fn may_run_loads() -> bool {
    if !moqui_core::user::has_permission(PERMISSION).unwrap_or(false) {
        add_error("Load runs need the LOAD_RUNNER permission");
        return false;
    }
    match moqui_core::user::authorize_own("AT_XML_SCREEN", TOOLS_SCREEN, "all") {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(error = %err, "Refused a load run service");
            add_error("Load runs need authorization for all actions on the Tools app");
            false
        }
    }
}

fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("a load run service needs an execution context"))
}

fn none() -> Out {
    Ok(BTreeMap::new())
}

/// The loaded components as the script lookups take them, name and directory.
fn component_dirs(factory: &ExecutionContextFactory) -> Vec<(String, std::path::PathBuf)> {
    let components = factory.components();
    components
        .iter()
        .map(|c| (c.name.clone(), c.location.clone()))
        .collect()
}

/// The runId parameter, refused with an error unless it is a runId.
fn run_id(parameters: &BTreeMap<String, Value>) -> Option<String> {
    let id = text(parameters, "runId");
    if results::valid_run_id(&id) {
        Some(id)
    } else {
        add_error(format!("{id} is not a load run id"));
        None
    }
}

/// A Map parameter as JSON; anything else as an empty object.
fn json_map(parameters: &BTreeMap<String, Value>, key: &str) -> serde_json::Map<String, serde_json::Value> {
    match parameters.get(key) {
        Some(value @ Value::Map(_)) => match to_json(value) {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        },
        _ => serde_json::Map::new(),
    }
}

fn find_load_scripts(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let factory = factory()?;
    let dirs = component_dirs(&factory);
    let found = scripts::scripts_in(dirs.iter().map(|(name, dir)| (name.as_str(), dir.as_path())));
    let mut list = Vec::new();
    for script in found {
        let mut entry = json!({
            "location": script.location,
            "componentName": script.component,
            "name": script.name,
            "fileName": script.file_name,
        });
        let component_dir = dirs
            .iter()
            .find(|(name, _)| *name == script.component)
            .map(|(_, dir)| dir.clone())
            .unwrap_or_default();
        // a file that does not evaluate is listed with its error, and the others still list
        match run::read_plan_for_list(&factory, &script, &component_dir) {
            Ok(plan) => {
                let field = |key: &str| plan.plan.get(key).cloned().unwrap_or(serde_json::Value::Null);
                entry["title"] = field("title");
                entry["description"] = field("description");
                entry["uses"] = json!(plan.uses());
                entry["params"] = field("params");
                let scenarios: Vec<serde_json::Value> = plan
                    .scenarios
                    .iter()
                    .map(|(name, defaults)| {
                        json!({ "name": name, "users": defaults.get("users"), "duration": defaults.get("duration"),
                            "iterations": defaults.get("iterations") })
                    })
                    .collect();
                entry["scenarios"] = json!(scenarios);
            }
            Err(err) => {
                tracing::warn!(script = script.location, error = %err, "A load script's plan did not read");
                entry["error"] = json!(err);
            }
        }
        list.push(from_json(&entry));
    }
    Ok(BTreeMap::from([("scripts".to_string(), Value::List(list))]))
}

fn start_load_run(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let factory = factory()?;
    let dirs = component_dirs(&factory);
    let location = text(parameters, "scriptLocation");
    let script = match scripts::resolve(&location, dirs.iter().map(|(name, dir)| (name.as_str(), dir.as_path()))) {
        Ok(script) => script,
        Err(message) => {
            add_error(message);
            return none();
        }
    };
    // The three targets of the New run form (§8): this server's services in process (`service`), this server over
    // HTTP (`this`, the default for an http script), another server by URL (`url`). The plan's `uses` is checked
    // against it once the plan is read (run::check_uses).
    let target_kind = match targets::TargetKind::parse(&text(parameters, "targetKind")) {
        Ok(kind) => kind,
        Err(message) => {
            add_error(message);
            return none();
        }
    };
    // each override's users is checked here; their total with the plan's defaults when the plan is read
    if let Err(message) = targets::override_users(parameters.get("scenarios")) {
        add_error(message);
        return none();
    }
    let component_dir = dirs
        .iter()
        .find(|(name, _)| *name == script.component)
        .map(|(_, dir)| dir.clone())
        .unwrap_or_default();
    let user = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let ask = run::Ask {
        script,
        component_dir,
        target_kind,
        target_url: text(parameters, "targetUrl"),
        params: json_map(parameters, "parameters"),
        scenarios: serde_json::Value::Object(json_map(parameters, "scenarios")),
        label: text(parameters, "label"),
        insecure: matches!(parameters.get("insecure"), Some(Value::Bool(true))),
        user,
    };
    match run::start(&factory, ask) {
        Ok(id) => Ok(BTreeMap::from([("runId".to_string(), Value::String(id))])),
        Err(message) => {
            add_error(message);
            none()
        }
    }
}

fn stop_load_run(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let Some(id) = run_id(parameters) else {
        return none();
    };
    let now = matches!(parameters.get("now"), Some(Value::Bool(true)));
    if run::stop(&id, now).is_some() {
        return none();
    }
    let root = results::root(&*factory()?);
    match results::read_result(&root, &id) {
        Ok(Some(_)) => add_error(format!("Load run {id} has already finished")),
        Ok(None) => add_error(format!("Load run {id} is not running on this server")),
        Err(message) => add_error(message),
    }
    none()
}

fn get_load_run_status(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let Some(id) = run_id(parameters) else {
        return none();
    };
    // a poll may wait for the run's end (a spec, or a page that wants the result as soon as it is written)
    let wait = text(parameters, "waitSeconds").parse::<f64>().unwrap_or(0.0);
    if wait.is_finite() && wait > 0.0 {
        run::wait_for_end(&id, Duration::from_secs_f64(wait.min(MAX_WAIT_SECONDS)));
    }
    if let Some(active) = run::find(&id) {
        let snapshot = active.load.snapshot();
        let summary = active.summary(&snapshot);
        return Ok(BTreeMap::from([
            ("runId".to_string(), Value::String(id)),
            ("running".to_string(), Value::Bool(true)),
            ("status".to_string(), Value::String("running".to_string())),
            ("snapshot".to_string(), from_json(&snapshot)),
            ("summary".to_string(), from_json(&summary)),
        ]));
    }
    let root = results::root(&*factory()?);
    let result = match results::read_result(&root, &id) {
        Ok(Some(result)) => result,
        Ok(None) => {
            add_error(format!("Load run {id} is not on this server"));
            return none();
        }
        Err(message) => {
            add_error(message);
            return none();
        }
    };
    let summary = results::summary(&id, &result);
    let status = summary
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(BTreeMap::from([
        ("runId".to_string(), Value::String(id)),
        ("running".to_string(), Value::Bool(false)),
        ("status".to_string(), Value::String(status)),
        ("summary".to_string(), from_json(&summary)),
    ]))
}

fn find_load_runs(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let root = results::root(&*factory()?);
    let (script, status) = (text(parameters, "scriptLocation"), text(parameters, "status"));
    let find = results::Find {
        script: Some(script.as_str()).filter(|s| !s.is_empty()),
        status: Some(status.as_str()).filter(|s| !s.is_empty()),
    };
    let running = run::running();
    let mut runs: Vec<Value> = Vec::new();
    for active in &running {
        let summary = active.summary(&active.load.snapshot());
        if find.keeps(&summary) {
            runs.push(from_json(&summary));
        }
    }
    // a run still in the registry is listed once, as running, even after its result is written and before it leaves
    let ids = run::running_ids();
    let finished = results::list(&root, &find);
    let finished = finished.iter().filter(|summary| {
        let id = summary
            .get("runId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        !ids.contains(id)
    });
    runs.extend(finished.map(from_json));
    Ok(BTreeMap::from([("runs".to_string(), Value::List(runs))]))
}

fn get_load_run_result(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let Some(id) = run_id(parameters) else {
        return none();
    };
    match results::read_result(&results::root(&*factory()?), &id) {
        Ok(Some(result)) => Ok(BTreeMap::from([("result".to_string(), from_json(&result))])),
        Ok(None) => {
            add_error(format!("No finished load run {id} on this server"));
            none()
        }
        Err(message) => {
            add_error(message);
            none()
        }
    }
}

fn delete_load_run(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let Some(id) = run_id(parameters) else {
        return none();
    };
    if run::find(&id).is_some() {
        add_error(format!("Load run {id} is running; stop it first"));
        return none();
    }
    match results::delete(&results::root(&*factory()?), &id) {
        Ok(deleted) => Ok(BTreeMap::from([("deleted".to_string(), Value::Bool(deleted))])),
        Err(message) => {
            add_error(message);
            none()
        }
    }
}

fn import_load_run_result(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let text_of = match parameters.get("result") {
        Some(Value::String(text)) => text.clone(),
        Some(value @ Value::Map(_)) => to_json(value).to_string(),
        _ => String::new(),
    };
    let importer = moqui_core::user::current_username().ok().flatten().unwrap_or_default();
    let result = match results::imported_result(&text_of, &importer, &text(parameters, "label")) {
        Ok(result) => result,
        Err(message) => {
            add_error(message);
            return none();
        }
    };
    let root = results::root(&*factory()?);
    let base = results::run_id_base(jiff::Timestamp::now(), std::process::id());
    let written = results::create_run_dir(&root, &base).and_then(|(id, _)| {
        results::write_result(&root, &id, &result)?;
        Ok(id)
    });
    match written {
        Ok(id) => Ok(BTreeMap::from([("runId".to_string(), Value::String(id))])),
        Err(message) => {
            tracing::error!(error = %message, "Could not keep an imported load result");
            add_error(format!("Could not keep the result: {message}"));
            none()
        }
    }
}

fn compare_load_runs(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    // plans/LoadRunner.md L5
    add_error("Comparing load runs in the server is not available yet in this build");
    none()
}
