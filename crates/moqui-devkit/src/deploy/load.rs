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

//! Load a component on a running server from the directory it was placed in (plans/Component-Deploy.md, Phase 2):
//! found as boot finds components, checked, then loaded in an order that keeps the server consistent: its entities'
//! tables made against the new model before the model is swapped in, the component registered, the conf merged
//! again and applied, its files reloaded through the reload pipeline, the crates' caches read again, and its data
//! loaded. What it needs that cannot be live (a restart-class conf change, Rust code) is named in the preview; the
//! load can then put it in place for the next start instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::context::conf_reload::{self, ConfReport};
use moqui_core::context::{ComponentInfo, ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::load_feed::{component_data_locations, data_types_of, load_entity_data, LoadOptions};
use moqui_core::entity::{RelinkPreview, RelinkReport};
use moqui_db::sql::Condition;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use serde::Serialize;

use crate::reload::{self, ReloadReport};

/// The data types loaded when none are asked for.
pub const DEFAULT_DATA_TYPES: [&str; 3] = ["seed", "seed-initial", "install"];
/// The key of the data files that declare no type, in a preview's counts: they load whatever types are asked for.
pub const UNTYPED: &str = "(any)";
const DEPLOYED: &str = "moqui.server.DeployedComponent";
const HISTORY: &str = "moqui.server.DeployedComponentHistory";

/// A component found in a component directory that is not loaded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub name: String,
    pub version: String,
    pub location: String,
    pub depends_on: Vec<String>,
}

/// What loading a component would do, changing nothing.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPreview {
    pub component: String,
    pub version: String,
    pub location: String,
    pub depends_on: Vec<String>,
    /// Why it cannot be loaded now: none when it can.
    pub refusals: Vec<String>,
    /// The conf changes its `MoquiConf.xml` makes, each with its class.
    pub conf: ConfReport,
    /// Whether a conf change waits for a restart, so loading it now leaves that part for the restart.
    pub restart_needed: bool,
    pub entities: RelinkPreview,
    /// The data files by the type they declare ([`UNTYPED`] for none).
    pub data_types: BTreeMap<String, usize>,
}

/// What a load does besides what every load does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Shared {
    /// Load the component's data; a cluster member applying another member's deploy does not, as the data is in the
    /// shared database already.
    pub data: bool,
    /// The component's files reach the cluster's other members (a database repository, plans/Component-Deploy.md
    /// Phase 4), so a cluster does not refuse it.
    pub cluster_ok: bool,
}

impl Shared {
    /// A load on this server alone, with its data.
    pub(crate) const ALONE: Self = Self {
        data: true,
        cluster_ok: false,
    };
}

/// When a load applies the component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Now: what can be live, the rest at the next start.
    Now,
    /// At the next start: recorded only; the boot loads it and its data.
    AtRestart,
}

/// One data file a load loaded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataFile {
    pub location: String,
    pub records: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a load did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadReport {
    pub component: String,
    /// `deployed`, `pending-restart` or `failed`.
    pub status: String,
    pub preview: LoadPreview,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entities: Option<RelinkReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conf: Option<ConfReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reload: Option<ReloadReport>,
    pub data: Vec<DataFile>,
    pub records: u64,
    pub problems: Vec<String>,
    pub millis: u64,
}

/// What a load needs besides its preview: the components with it, and the conf they make.
struct Plan {
    preview: LoadPreview,
    component: ComponentInfo,
    components: Vec<ComponentInfo>,
    conf: Option<MNode>,
}

/// The components found in the component directories, as boot finds them, that are not loaded.
///
/// # Errors
///
/// When the conf does not merge or a `component.xml` does not parse.
pub fn scan(factory: &ExecutionContextFactory) -> Result<Vec<Found>, String> {
    let loaded = factory.components();
    let found = moqui_core::context::discover_again(factory).map_err(|err| err.to_string())?;
    Ok(found
        .into_iter()
        .filter(|info| !loaded.iter().any(|have| have.name == info.name))
        .map(|info| Found {
            name: info.name,
            version: info.version,
            location: info.location.display().to_string(),
            depends_on: info.depends_on,
        })
        .collect())
}

/// What loading the component `name` would do, changing nothing.
///
/// # Errors
///
/// When it is not found, or the conf does not merge.
pub fn preview(factory: &ExecutionContextFactory, name: &str) -> Result<LoadPreview, String> {
    plan(factory, name, None, false).map(|plan| plan.preview)
}

/// [`preview`] of `staged`, a component fetched for a deploy into a directory that is not a component directory
/// (`.staging-<name>`): it goes after the loaded components, where its directory will be.
///
/// # Errors
///
/// As [`preview`].
pub fn preview_staged(factory: &ExecutionContextFactory, staged: ComponentInfo) -> Result<LoadPreview, String> {
    preview_staged_shared(factory, staged, false)
}

/// [`preview_staged`], not refused in a cluster when `cluster_ok` (a database repository's files reach every member).
pub(crate) fn preview_staged_shared(
    factory: &ExecutionContextFactory,
    staged: ComponentInfo,
    cluster_ok: bool,
) -> Result<LoadPreview, String> {
    let name = staged.name.clone();
    plan(factory, &name, Some(staged), cluster_ok).map(|plan| plan.preview)
}

fn plan(
    factory: &ExecutionContextFactory,
    name: &str,
    staged: Option<ComponentInfo>,
    cluster_ok: bool,
) -> Result<Plan, String> {
    let loaded = factory.components();
    if loaded.iter().any(|info| info.name == name) {
        return Err(format!("{name} is loaded already"));
    }
    let (component, components) = match staged {
        Some(component) => {
            let mut components = loaded.to_vec();
            components.push(component.clone());
            (component, components)
        }
        None => {
            let found = moqui_core::context::discover_again(factory).map_err(|err| err.to_string())?;
            let Some(component) = found.iter().find(|info| info.name == name).cloned() else {
                return Err(format!(
                    "{name} is not found: it is not in a component directory, it is disabled or not for this \
                     instance's purpose, or a component it depends on is missing"
                ));
            };
            let components: Vec<ComponentInfo> = found
                .into_iter()
                .filter(|info| info.name == name || loaded.iter().any(|have| have.name == info.name))
                .collect();
            (component, components)
        }
    };
    let mut preview = LoadPreview {
        component: component.name.clone(),
        version: component.version.clone(),
        location: component.location.display().to_string(),
        depends_on: component.depends_on.clone(),
        ..LoadPreview::default()
    };
    let refusals = &mut preview.refusals;
    // the loaded components keep their order, so the conf merges and the artifacts override as at the next start
    let others: Vec<&str> = components
        .iter()
        .filter(|info| info.name != name)
        .map(|info| info.name.as_str())
        .collect();
    let names: Vec<&str> = loaded.iter().map(|info| info.name.as_str()).collect();
    if others != names {
        refusals.push(
            "the loaded components are not what the component directories hold now (one moved, was removed or is \
             ordered differently); restart to load it"
                .to_string(),
        );
    }
    for dependency in &component.depends_on {
        if !names.contains(&dependency.as_str()) {
            refusals.push(format!(
                "it depends on {dependency}, which is not loaded; load that first"
            ));
        }
    }
    if component.location.join("Cargo.toml").is_file() {
        refusals.push(if moqui_core::native::component_linked(name) {
            "it has Rust code, which the binary links and a restart loads".to_string()
        } else {
            "it has Rust code: build it into the binary (./moq build) and restart".to_string()
        });
    }
    for problem in xml_problems(&component.location) {
        refusals.push(problem);
    }
    if let Some(facade) = factory.entity() {
        if !cluster_ok && moqui_core::cluster::live_member_count(factory, &facade).unwrap_or(1) > 1 {
            refusals.push(
                "this server is in a cluster, whose other members would not have the component; restart each member \
                 instead"
                    .to_string(),
            );
        }
        let dirs = moqui_core::context::component_entity_dirs_of(factory.framework_component_dir(), &components);
        match facade.preview_relink(&dirs) {
            Ok(entities) => {
                for incompatible in &entities.incompatible {
                    refusals.push(format!("an entity change the table check cannot make: {incompatible}"));
                }
                preview.entities = entities;
            }
            Err(err) => preview.refusals.push(err.to_string()),
        }
    }
    let conf = match conf_reload::merged_from_files(factory, &components) {
        Ok(conf) => {
            match conf_reload::preview_conf(factory, &conf) {
                Ok(report) => {
                    preview.restart_needed = report.pending_restart > 0;
                    preview.conf = report;
                }
                Err(err) => preview.refusals.push(format!("the conf: {err}")),
            }
            Some(conf)
        }
        Err(err) => {
            preview.refusals.push(format!("the conf does not merge with it: {err}"));
            None
        }
    };
    for location in component_data_locations(name, &component.location, true) {
        let path = file_of(&component, &location);
        let types = data_types_of(&path);
        if types.is_empty() {
            *preview.data_types.entry(UNTYPED.to_string()).or_default() += 1;
        }
        for kind in types {
            *preview.data_types.entry(kind).or_default() += 1;
        }
    }
    Ok(Plan {
        preview,
        component,
        components,
        conf,
    })
}

/// Load the component `name`: with [`Mode::Now`] what can be live is applied and its data loaded, with
/// [`Mode::AtRestart`] it is recorded and the next start loads it and its data. `data_types` empty is
/// [`DEFAULT_DATA_TYPES`]. Runs holding the reload lock, so no other reload runs beside it.
pub fn load(factory: &Arc<ExecutionContextFactory>, name: &str, mode: Mode, data_types: &[String]) -> LoadReport {
    load_shared(factory, name, mode, data_types, Shared::ALONE)
}

/// [`load`] with what `shared` says: a database repository's deploy, or a cluster member applying another's.
pub(crate) fn load_shared(
    factory: &Arc<ExecutionContextFactory>,
    name: &str,
    mode: Mode,
    data_types: &[String],
    shared: Shared,
) -> LoadReport {
    let started = std::time::Instant::now();
    let data_types: Vec<String> = if data_types.is_empty() {
        DEFAULT_DATA_TYPES.iter().map(|kind| (*kind).to_string()).collect()
    } else {
        data_types.to_vec()
    };
    let mut report = reload::with_reload_lock(factory, || load_held(factory, name, mode, &data_types, shared));
    report.millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    // a cluster member applying another member's deploy leaves the shared row as that member wrote it
    if shared.data {
        record(factory, &report, "load", &data_types);
    }
    report
}

fn load_held(
    factory: &Arc<ExecutionContextFactory>,
    name: &str,
    mode: Mode,
    data_types: &[String],
    shared: Shared,
) -> LoadReport {
    let mut report = LoadReport {
        component: name.to_string(),
        status: "failed".to_string(),
        ..LoadReport::default()
    };
    let plan = match plan(factory, name, None, shared.cluster_ok) {
        Ok(plan) => plan,
        Err(err) => {
            report.problems.push(err);
            return report;
        }
    };
    report.preview = plan.preview.clone();
    if !plan.preview.refusals.is_empty() {
        report.problems.extend(plan.preview.refusals.iter().cloned());
        return report;
    }
    if mode == Mode::AtRestart {
        report.status = "pending-restart".to_string();
        return report;
    }
    let Some(conf) = plan.conf else {
        report.problems.push("the conf does not merge with it".to_string());
        return report;
    };
    let previous = factory.components();
    // 1. its entities: the tables made against the new model, then the model swapped in; a failure changes nothing
    if let Some(facade) = factory.entity() {
        let dirs = moqui_core::context::component_entity_dirs_of(factory.framework_component_dir(), &plan.components);
        let entity_files = entity_files(&plan.component.location);
        if !plan.preview.entities.added.is_empty() || !plan.preview.entities.changed.is_empty() {
            match facade.relink_components(Some(dirs), &entity_files) {
                Ok(relinked) => report.entities = Some(relinked),
                Err(err) => {
                    report.problems.push(format!("its entities: {err}"));
                    return report;
                }
            }
        }
    }
    // 2. the component registered, its `component://` root first
    factory.replace_components(plan.components.clone());
    // 3. the conf merged again with it, what can be live applied
    match conf_reload::apply_conf(factory, &conf) {
        Ok(applied) => {
            report.problems.extend(applied.problems.iter().cloned());
            report.conf = Some(applied);
        }
        Err(err) => {
            report.problems.push(format!("the conf: {err}"));
            roll_back(factory, &previous);
            return report;
        }
    }
    // 4. its files through the reload pipeline: services, ECA rules, REST, screens and what they read, skills
    let paths = reload_files(&plan.component.location);
    let reloaded = reload::reload_paths_held(factory, &paths);
    for file in &reloaded.files {
        for problem in &file.problems {
            report.problems.push(format!("{}: {}", problem.path, problem.message));
        }
    }
    report.reload = Some(reloaded);
    #[cfg(feature = "watch")]
    crate::watch::watch_component(factory, &plan.component.location);
    moqui_core::extension::artifacts_changed(factory);
    // 5. its data, each file on its own (a cluster member applying another member's deploy has it already)
    let (data, records) = if shared.data {
        load_data(factory, &plan.component, data_types)
    } else {
        (Vec::new(), 0)
    };
    report.problems.extend(
        data.iter()
            .filter_map(|file| file.error.as_ref().map(|error| format!("{}: {error}", file.location))),
    );
    report.data = data;
    report.records = records;
    report.status = "deployed".to_string();
    tracing::warn!(
        target: moqui_core::live_settings::AUDIT,
        "Component {name} loaded on the running server: {} records loaded, {} problems",
        report.records,
        report.problems.len()
    );
    report
}

/// Put back the components and the entity model there were before a load that failed after it registered the
/// component. Tables it made stay; they are additive.
fn roll_back(factory: &ExecutionContextFactory, previous: &[ComponentInfo]) {
    factory.replace_components(previous.to_vec());
    if let Some(facade) = factory.entity() {
        let dirs = moqui_core::context::component_entity_dirs_of(factory.framework_component_dir(), previous);
        if let Err(err) = facade.relink_components(Some(dirs), &[]) {
            tracing::error!("Could not put the entity model back after a failed component load: {err}");
        }
    }
}

/// Load the data files of `component` of `data_types`, each in its own transaction; a file that fails is reported and
/// the rest go on. Then the search indexes of new documents are built.
fn load_data(
    factory: &Arc<ExecutionContextFactory>,
    component: &ComponentInfo,
    data_types: &[String],
) -> (Vec<DataFile>, u64) {
    let mut out = Vec::new();
    let mut records = 0;
    let Some(facade) = factory.entity() else {
        return (out, records);
    };
    let options = LoadOptions {
        data_types: data_types.to_vec(),
        ..LoadOptions::default()
    };
    let seed = data_types.iter().any(|kind| kind == "seed");
    in_context(factory, || {
        for location in component_data_locations(&component.name, &component.location, seed) {
            match load_entity_data(&facade, &location, &options) {
                Ok(result) => {
                    records += result.records;
                    let error =
                        (result.failed_files > 0).then(|| format!("{} files failed to load", result.failed_files));
                    out.push(DataFile {
                        location,
                        records: result.records,
                        error,
                    });
                }
                Err(err) => {
                    let text = moqui_core::entity::load_feed::error_chain(&err);
                    tracing::error!(
                        "Component data: skipping to the next file after an error loading {location}: {text}"
                    );
                    out.push(DataFile {
                        location,
                        records: 0,
                        error: Some(text),
                    });
                }
            }
        }
        moqui_core::search::index_feeds_on_start_empty();
    });
    (out, records)
}

/// Run `body` in the entered context, or a new one, with authorization off.
fn in_context<R>(factory: &Arc<ExecutionContextFactory>, body: impl FnOnce() -> R) -> R {
    let entered_here = ExecutionContext::with_current(|_| ())
        .is_none()
        .then(|| factory.new_context());
    let _entered = entered_here.as_ref().map(ExecutionContext::enter);
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    body()
}

/// At the start, load the data of the components a load left for the next start, now that the boot loaded them.
pub fn load_pending(factory: &Arc<ExecutionContextFactory>) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let pending = in_context(factory, || {
        facade
            .find(DEPLOYED)
            .and_then(|find| {
                find.condition(Condition::equals("status", Value::string("pending-restart")))
                    .disable_authz()
                    .list()
            })
            .map(|list| list.into_values())
    });
    let pending = match pending {
        Ok(pending) => pending,
        Err(err) => {
            tracing::warn!("Could not read the components waiting for this start: {err}");
            return;
        }
    };
    let loaded = factory.components();
    for row in pending {
        let name = text(&row, "componentName");
        let Some(component) = loaded.iter().find(|info| info.name == name) else {
            tracing::warn!("Component {name} was left for this start, but it is not loaded; see System › Components");
            continue;
        };
        let types: Vec<String> = text(&row, "dataTypes")
            .split(',')
            .map(str::trim)
            .filter(|kind| !kind.is_empty())
            .map(str::to_string)
            .collect();
        let (data, records) = load_data(factory, component, &types);
        let failed = data.iter().filter(|file| file.error.is_some()).count();
        tracing::info!("Component {name}: loaded its data at this start, {records} records, {failed} files failed");
        let report = LoadReport {
            component: name,
            status: "deployed".to_string(),
            data,
            records,
            ..LoadReport::default()
        };
        record(factory, &report, "data", &types);
    }
}

/// Mark the component `name` loaded by a deploy as removed and move its directory aside (`.removed/` beside it), so
/// the next start leaves it out. What it loaded stays until then; its tables and data stay after.
///
/// # Errors
///
/// When it was not loaded by a deploy, or its directory cannot be moved.
pub fn remove(factory: &Arc<ExecutionContextFactory>, name: &str) -> Result<String, String> {
    let Some(facade) = factory.entity() else {
        return Err("no entity facade".to_string());
    };
    let row = in_context(factory, || {
        facade.find(DEPLOYED).and_then(|find| {
            find.condition(Condition::equals("componentName", Value::string(name)))
                .disable_authz()
                .one()
        })
    })
    .map_err(|err| err.to_string())?
    .ok_or_else(|| format!("{name} was not loaded by a deploy; take it out of its component directory instead"))?;
    let location = PathBuf::from(text(&row, "location"));
    let moved = if location.is_dir() {
        let parent = location.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let aside = parent
            .join(".removed")
            .join(format!("{name}-{}", moqui_core::service::util::now_millis()));
        std::fs::create_dir_all(aside.parent().unwrap_or(&parent)).map_err(|err| err.to_string())?;
        std::fs::rename(&location, &aside).map_err(|err| format!("could not move {}: {err}", location.display()))?;
        aside.display().to_string()
    } else {
        String::new()
    };
    let report = LoadReport {
        component: name.to_string(),
        status: "removed".to_string(),
        ..LoadReport::default()
    };
    record(factory, &report, "remove", &[]);
    tracing::warn!(target: moqui_core::live_settings::AUDIT, "Component {name} removed; it is left out at the next start");
    Ok(moved)
}

/// Keep what a load did: the component's row and a history row. A failure is logged.
fn record(factory: &Arc<ExecutionContextFactory>, report: &LoadReport, action: &str, data_types: &[String]) {
    let Some(facade) = factory.entity() else {
        return;
    };
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    let text = serde_json::to_string(report).unwrap_or_default();
    let now = moqui_core::service::util::now_stamp();
    let written = in_context(factory, || -> Result<(), moqui_lib::error::Error> {
        let existing = facade
            .find(DEPLOYED)?
            .condition(Condition::equals(
                "componentName",
                Value::string(report.component.as_str()),
            ))
            .disable_authz()
            .one()?;
        // a load refused before it changed anything leaves no row of its own
        if existing.is_none() && report.status == "failed" {
            return Ok(());
        }
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(DEPLOYED)?;
                row.set("componentName", Value::string(report.component.as_str()));
                row.set("source", Value::string("directory"));
                row.set("location", Value::string(report.preview.location.as_str()));
                row
            }
        };
        if action != "data" && action != "remove" {
            row.set("dataTypes", Value::string(data_types.join(",")));
        }
        row.set("status", Value::string(report.status.as_str()));
        row.set("lastReport", Value::string(text.as_str()));
        row.set("changedDate", now.clone());
        row.set("changedByUserId", Value::string(user.as_str()));
        row.create_or_update(&facade)?;
        let mut history = facade.make_value(HISTORY)?;
        history.set("componentName", Value::string(report.component.as_str()));
        history.set("historyDate", now.clone());
        history.set("action", Value::string(action));
        history.set("status", Value::string(report.status.as_str()));
        history.set("userId", Value::string(user.as_str()));
        history.set("report", Value::string(text.as_str()));
        history.create(&facade)?;
        Ok(())
    });
    if let Err(err) = written {
        tracing::error!("Could not record the {action} of component {}: {err}", report.component);
    }
}

fn text(row: &moqui_core::entity::EntityValue, field: &str) -> String {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// The file a `component://name/...` location of `component` names, before the component is registered.
fn file_of(component: &ComponentInfo, location: &str) -> PathBuf {
    let prefix = format!("component://{}/", component.name);
    location
        .strip_prefix(&prefix)
        .map_or_else(|| PathBuf::from(location), |relative| component.location.join(relative))
}

/// The component's entity definition files, for the relink's table check.
fn entity_files(location: &Path) -> Vec<PathBuf> {
    let mut files = files_under(&location.join("entity"));
    files.retain(|file| {
        let name = file.to_string_lossy();
        name.ends_with(".xml") && !name.ends_with(".eecas.xml")
    });
    files
}

/// The component's files the reload pipeline takes: all but its conf and `component.xml` (the load applies those),
/// its entity definitions (relinked first), its data (loaded last), its Rust code and its tests.
fn reload_files(location: &Path) -> Vec<PathBuf> {
    let mut files = files_under(location);
    files.retain(|file| {
        let Ok(relative) = file.strip_prefix(location) else {
            return false;
        };
        let text = relative.to_string_lossy().replace('\\', "/");
        let top = text.split('/').next().unwrap_or("");
        let entity_definition = top == "entity" && text.ends_with(".xml") && !text.ends_with(".eecas.xml");
        !(text == "component.xml"
            || text == "MoquiConf.xml"
            || text == "Cargo.toml"
            || text.ends_with(".rs")
            || entity_definition
            || matches!(top, "data" | "test" | "src" | "target"))
    });
    files
}

/// Every file under `dir`, but in hidden directories (`.git`) and `node_modules`.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                if !name.starts_with('.') && name != "node_modules" {
                    pending.push(path);
                }
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The XML files of the component's artifacts that do not parse, each as a refusal.
fn xml_problems(location: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let mut files = files_under(location);
    files.retain(|file| {
        file.extension().is_some_and(|ext| ext == "xml")
            && file.strip_prefix(location).is_ok_and(|relative| {
                let top = relative
                    .iter()
                    .next()
                    .map(|part| part.to_string_lossy().to_string())
                    .unwrap_or_default();
                !matches!(top.as_str(), "data" | "test" | "target")
            })
    });
    for file in files {
        if let Err(err) = MNode::parse_file(&file) {
            problems.push(format!("{} does not parse: {err}", file.display()));
        }
    }
    problems
}

#[cfg(test)]
#[path = "load_tests.rs"]
mod tests;
