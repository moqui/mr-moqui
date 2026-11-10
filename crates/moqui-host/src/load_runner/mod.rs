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

//! Load runs in a server (plans/LoadRunner.md §7, §10), the server side of the Tools load runner under the
//! `test-client` feature: `org.moqui.dev.LoadServices`, Rust services registered through this module's extension
//! entry. The load scripts of the loaded components ([`scripts`]), the result files of finished runs ([`results`]) and
//! what a run may reach ([`targets`]). Starting, stopping and polling a running load need the load engine's registry
//! and run thread, which come with it; until then `start#LoadRun` checks its input and refuses, and no run is ever
//! running here. Each service checks [`may_run_loads`] whatever calls it, as the SQL services do.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::{is_empty, to_plain_string, Value};

pub(crate) mod results;
pub(crate) mod scripts;
pub(crate) mod targets;

/// The Tools app, whose all-actions authorization a load run needs beside the permission.
const TOOLS_SCREEN: &str = "component://tools/screen/Tools.xml";
/// The permission of load runs (D9), sealed and on `ADMIN_ADV`.
pub(crate) const PERMISSION: &str = "LOAD_RUNNER";
/// What a service that needs the load engine answers until it is linked here.
const NO_ENGINE: &str = "Running a load in the server is not available yet in this build";

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["test-client"],
        register_services,
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

fn find_load_scripts(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let dirs = component_dirs(&*factory()?);
    let found = scripts::scripts_in(dirs.iter().map(|(name, dir)| (name.as_str(), dir.as_path())));
    let list = found
        .into_iter()
        .map(|script| {
            from_json(&serde_json::json!({
                "location": script.location,
                "componentName": script.component,
                "name": script.name,
                "fileName": script.file_name,
            }))
        })
        .collect();
    Ok(BTreeMap::from([("scripts".to_string(), Value::List(list))]))
}

fn start_load_run(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let factory = factory()?;
    let dirs = component_dirs(&factory);
    let location = text(parameters, "scriptLocation");
    if let Err(message) = scripts::resolve(&location, dirs.iter().map(|(name, dir)| (name.as_str(), dir.as_path()))) {
        add_error(message);
        return none();
    }
    // The three targets of the New run form (§8): this server's services in process (`service`), this server over
    // HTTP (`this`, the default for an http script), another server by URL (`url`). A plan whose `uses` has
    // `service` needs the `service` target; that is checked when the plan is read, with the engine.
    match targets::TargetKind::parse(&text(parameters, "targetKind")) {
        Ok(targets::TargetKind::Url) => {
            let url = text(parameters, "targetUrl");
            if let Err(message) = targets::TargetPolicy::of_factory(&factory).allow(&url) {
                add_error(message);
                return none();
            }
        }
        Ok(targets::TargetKind::Service | targets::TargetKind::This) => {}
        Err(message) => {
            add_error(message);
            return none();
        }
    }
    let overrides = match targets::override_users(parameters.get("scenarios")) {
        Ok(overrides) => overrides,
        Err(message) => {
            add_error(message);
            return none();
        }
    };
    // The total checked here is the overrides' alone. When the plan is read, each scenario's users is its override
    // or else the plan's default, and that merged total is what check_users is given.
    let total = overrides.values().copied().fold(0, usize::saturating_add);
    if let Err(message) = targets::check_users(total) {
        add_error(message);
        return none();
    }
    add_error(NO_ENGINE);
    none()
}

fn stop_load_run(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !may_run_loads() {
        return none();
    }
    let Some(id) = run_id(parameters) else {
        return none();
    };
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
    let runs = results::list(&root, &find).iter().map(from_json).collect();
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
