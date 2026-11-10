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

//! `org.moqui.impl.ComponentServices`: the conf reload and, later, the components loaded on a running server, as
//! services for the System app (plans/archive/Component-Deploy.md). They need the `ADMIN` group on any instance.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::context::{conf_reload, ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, preview_conf_reload)?;
    rust_service!(map, reload_conf)?;
    rust_service!(map, get_pending_restart)?;
    super::conf_edit::register(map)?;
    rust_service!(map, scan_components)?;
    rust_service!(map, preview_component_load)?;
    rust_service!(map, load_component)?;
    rust_service!(map, load_component_data)?;
    rust_service!(map, remove_component)?;
    rust_service!(map, preview_component_deploy)?;
    rust_service!(map, deploy_component)?;
    rust_service!(map, discard_component_deploy)?;
    rust_service!(map, preview_component_update)?;
    rust_service!(map, update_component)?;
    Ok(())
}

/// The factory of the entered context when its user is in the `ADMIN` group; otherwise the error is added and the
/// answer is `None`.
pub(super) fn admin(what: &str) -> Option<Arc<ExecutionContextFactory>> {
    if !moqui_core::user::is_in_group("ADMIN").unwrap_or(false) {
        ExecutionContext::with_current(|ec| ec.message().add_error(format!("{what} needs the ADMIN group")));
        return None;
    }
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

pub(super) fn add_error(text: String) {
    ExecutionContext::with_current(|ec| ec.message().add_error(text)).unwrap_or_default();
}

/// The text of the parameter `name`.
pub(super) fn parameter(parameters: &BTreeMap<String, Value>, name: &str) -> String {
    parameters
        .get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// A list parameter given as a list or as comma separated text.
fn list_parameter(parameters: &BTreeMap<String, Value>, name: &str) -> Vec<String> {
    match parameters.get(name) {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Some(other) => moqui_lib::object::to_plain_string(other)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    }
}

pub(super) fn json_value<T: serde::Serialize>(value: &T) -> Value {
    moqui_lib::json::from_json(&serde_json::to_value(value).unwrap_or(serde_json::Value::Null))
}

/// `preview#ConfReload`: what reloading the conf files would change, each change with its class (live, apply,
/// restart) and the outcome it would have; nothing changes. Out: `report`.
fn preview_conf_reload(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Previewing a conf reload") else {
        return Ok(out);
    };
    match conf_reload::preview(&factory) {
        Ok(report) => {
            out.insert("report".to_string(), json_value(&report));
        }
        Err(err) => ExecutionContext::with_current(|ec| {
            ec.message().add_error(format!("The conf files do not merge: {err}"));
        })
        .unwrap_or_default(),
    }
    Ok(out)
}

/// `reload#Conf`: reload the conf files, applying what can be live and naming the rest as waiting for a restart.
/// Out: `report`.
fn reload_conf(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Reloading the conf") else {
        return Ok(out);
    };
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    tracing::warn!(target: moqui_core::live_settings::AUDIT, "Conf reload requested by user {user}");
    match conf_reload::apply(&factory) {
        Ok(report) => {
            out.insert("report".to_string(), json_value(&report));
        }
        Err(err) => ExecutionContext::with_current(|ec| {
            ec.message()
                .add_error(format!("The conf files do not merge, so the running conf stays: {err}"));
        })
        .unwrap_or_default(),
    }
    Ok(out)
}

/// `get#PendingRestart`: the conf changes the last reload left waiting for a restart. Out: `changes`.
fn get_pending_restart(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("The pending restart list") else {
        return Ok(out);
    };
    out.insert(
        "changes".to_string(),
        json_value(&conf_reload::pending_restart(&factory)),
    );
    out.insert(
        "restartAllowed".to_string(),
        Value::Bool(crate::service::restart_allowed()),
    );
    Ok(out)
}

/// `scan#Components`: the components found in the component directories that are not loaded. Out: `components`.
fn scan_components(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Finding components") else {
        return Ok(out);
    };
    match crate::deploy::load::scan(&factory) {
        Ok(found) => {
            out.insert("components".to_string(), json_value(&found));
        }
        Err(err) => add_error(format!("The component directories could not be read: {err}")),
    }
    Ok(out)
}

/// `preview#ComponentLoad`: what loading the component `componentName` would do: why it cannot be loaded now, its
/// conf changes with their class, its entities, and its data files by type. Out: `preview`.
fn preview_component_load(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Previewing a component load") else {
        return Ok(out);
    };
    match crate::deploy::load::preview(&factory, &parameter(parameters, "componentName")) {
        Ok(preview) => {
            out.insert("preview".to_string(), json_value(&preview));
        }
        Err(err) => add_error(err),
    }
    Ok(out)
}

/// The `mode` of a load or deploy: now (the default), restart (at the next start), or restart-now (at the next start,
/// and the server restarted right after). The second is whether to restart; `None`, with the error added, when it asks
/// for a restart this user or server cannot make.
fn mode_of(parameters: &BTreeMap<String, Value>, what: &str) -> Option<(crate::deploy::load::Mode, bool)> {
    use crate::deploy::load::Mode;
    match parameter(parameters, "mode").as_str() {
        "restart" => Some((Mode::AtRestart, false)),
        "restart-now" if crate::service::restart_allowed() => Some((Mode::AtRestart, true)),
        "restart-now" => {
            add_error(format!(
                "{what} and restart: the restart is only for a dev instance, a server that can restart and a user \
                 with DEV_AIDE or in ADMIN"
            ));
            None
        }
        _ => Some((Mode::Now, false)),
    }
}

/// After a load, deploy or update that asked for it and did not fail, restart the server.
fn restart_after(report: &crate::deploy::load::LoadReport, restart: bool, out: &mut BTreeMap<String, Value>) {
    if restart && report.status != "failed" && crate::service::restart_soon("Restart after the component change") {
        out.insert("restarting".to_string(), Value::Bool(true));
    }
}

/// `load#Component`: load the component `componentName` on the running server (`mode` now, the default), at the next
/// start (`mode` restart) or with a restart now (`mode` restart-now), and its data of `dataTypes` (seed, seed-initial
/// and install when empty). Out: `report`, and `restarting`.
fn load_component(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Loading a component") else {
        return Ok(out);
    };
    let name = parameter(parameters, "componentName");
    let Some((mode, restart)) = mode_of(parameters, "Load") else {
        return Ok(out);
    };
    let report = crate::deploy::load::load(&factory, &name, mode, &list_parameter(parameters, "dataTypes"));
    if report.status == "failed" {
        add_error(format!("{name} was not loaded: {}", report.problems.join("; ")));
    }
    restart_after(&report, restart, &mut out);
    out.insert("report".to_string(), json_value(&report));
    Ok(out)
}

/// `load#ComponentData`: load the data files of `dataTypes` (seed, seed-initial and install when empty) of the loaded
/// component `componentName` again, each file on its own. Out: `report`.
fn load_component_data(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Loading a component's data") else {
        return Ok(out);
    };
    let name = parameter(parameters, "componentName");
    match crate::deploy::load::load_component_data(&factory, &name, &list_parameter(parameters, "dataTypes")) {
        Ok(report) => {
            out.insert("report".to_string(), json_value(&report));
        }
        Err(err) => add_error(err),
    }
    Ok(out)
}

/// `remove#Component`: leave out at the next start a component a deploy loaded; its directory is moved aside. A working
/// clone with changes that are not committed (or not pushed) is left as it is unless `confirmDirty`, with the changes
/// in `dirty`. Out: `movedTo` or `dirty`.
fn remove_component(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Removing a component") else {
        return Ok(out);
    };
    let confirm_dirty = matches!(parameter(parameters, "confirmDirty").as_str(), "true" | "Y" | "y");
    match crate::deploy::load::remove(&factory, &parameter(parameters, "componentName"), confirm_dirty) {
        Ok(crate::deploy::load::Removed::MovedTo(moved)) => {
            out.insert("movedTo".to_string(), Value::string(moved));
        }
        Ok(crate::deploy::load::Removed::Dirty(changes)) => {
            out.insert("dirty".to_string(), json_value(&changes));
        }
        Err(err) => add_error(err),
    }
    Ok(out)
}

/// The factory of the entered context; `None` (with the error added) when there is none.
fn entered_factory() -> Option<Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

/// `preview#ComponentDeploy`: fetch `branch` of `url` into a repository kept as `storage` (database, bare or clone,
/// in `directory` for a clone), stage the files, and answer what deploying would do. The gate (the `COMPONENT_DEPLOY`
/// permission, and `component_deploy_enabled` in production) is the deploy's own. Out: `preview`.
fn preview_component_deploy(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    use crate::deploy::git::{DeployRequest, Storage};
    let mut out = BTreeMap::new();
    let Some(factory) = entered_factory() else {
        return Ok(out);
    };
    let Some(storage) = Storage::parse(&parameter(parameters, "storage")) else {
        add_error("storage is database, bare or clone".to_string());
        return Ok(out);
    };
    let optional = |name: &str| Some(parameter(parameters, name)).filter(|value| !value.is_empty());
    let request = DeployRequest {
        url: parameter(parameters, "url"),
        branch: parameter(parameters, "branch"),
        storage,
        directory: parameter(parameters, "directory"),
        username: optional("username"),
        token: optional("accessToken"),
    };
    match crate::deploy::git::preview(&factory, &request) {
        Ok(preview) => {
            out.insert("preview".to_string(), json_value(&preview));
        }
        Err(err) => add_error(err),
    }
    Ok(out)
}

/// `deploy#Component`: move the files a preview staged for `componentName` into place and load the component (`mode`
/// now, restart or restart-now as `load#Component`, `dataTypes`). Out: `report`, and `restarting`.
fn deploy_component(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = entered_factory() else {
        return Ok(out);
    };
    let name = parameter(parameters, "componentName");
    let Some((mode, restart)) = mode_of(parameters, "Deploy") else {
        return Ok(out);
    };
    let report = crate::deploy::git::deploy(&factory, &name, mode, &list_parameter(parameters, "dataTypes"));
    if report.status == "failed" {
        add_error(format!("{name} was not deployed: {}", report.problems.join("; ")));
    }
    restart_after(&report, restart, &mut out);
    out.insert("report".to_string(), json_value(&report));
    Ok(out)
}

/// `discard#ComponentDeploy`: drop what a preview staged for `componentName`.
fn discard_component_deploy(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = BTreeMap::new();
    let Some(factory) = entered_factory() else {
        return Ok(out);
    };
    if let Err(err) = crate::deploy::git::discard(&factory, &parameter(parameters, "componentName")) {
        add_error(err);
    }
    Ok(out)
}

/// `preview#ComponentUpdate`: fetch the branch of a component deployed from git and answer what updating it would do:
/// the commits, the paths that changed, its conf changes, and why it cannot be updated now. Out: `preview`.
fn preview_component_update(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = entered_factory() else {
        return Ok(out);
    };
    match crate::deploy::git::preview_update(&factory, &parameter(parameters, "componentName")) {
        Ok(preview) => {
            out.insert("preview".to_string(), json_value(&preview));
        }
        Err(err) => add_error(err),
    }
    Ok(out)
}

/// `update#Component`: update a component deployed from git to its branch's head, and load its changed data files of
/// `dataTypes` (seed when empty); with `mode` restart-now the server restarts after, for the parts that wait for a
/// restart. Out: `report`, and `restarting`.
fn update_component(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = entered_factory() else {
        return Ok(out);
    };
    let name = parameter(parameters, "componentName");
    let Some((_, restart)) = mode_of(parameters, "Update") else {
        return Ok(out);
    };
    let report = crate::deploy::git::update(&factory, &name, &list_parameter(parameters, "dataTypes"));
    if report.status == "failed" {
        add_error(format!("{name} was not updated: {}", report.problems.join("; ")));
    }
    restart_after(&report, restart, &mut out);
    out.insert("report".to_string(), json_value(&report));
    Ok(out)
}
