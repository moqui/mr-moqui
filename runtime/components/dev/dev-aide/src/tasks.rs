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

//! `org.moqui.devaide.TaskServices`, the task panel (plans/design-dev-aide.md 14, P8): the user's open tasks from a task
//! source (`mantle`, this instance's `WorkEffort` tasks assigned to the user when Mantle is loaded; or `moqui-rest`,
//! another Moqui instance such as the team's Projects server, read through Mantle's REST API, `my/tasks`, with the
//! user's own access token), the apps a task opens in (the conf's `task-app` elements and `DevAideTaskApp` rows), Quick create, and a
//! branch named for a task. Planning, boards and time are the project app's. A source that cannot be reached answers
//! its error with no tasks, and the rest of the dev-aide goes on.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::git::Mode;
use crate::tier::Tier;
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_tasks), get_tasks)?;
    registry.service_function(moqui_core::service_location!(get_task_sources), get_task_sources)?;
    registry.service_function(moqui_core::service_location!(set_task_source), set_task_source)?;
    registry.service_function(moqui_core::service_location!(store_task_source), store_task_source)?;
    registry.service_function(moqui_core::service_location!(delete_task_source), delete_task_source)?;
    registry.service_function(moqui_core::service_location!(store_task_app), store_task_app)?;
    registry.service_function(moqui_core::service_location!(get_task_apps), get_task_apps)?;
    registry.service_function(moqui_core::service_location!(delete_task_app), delete_task_app)?;
    registry.service_function(moqui_core::service_location!(create_task), create_task)?;
    registry.service_function(moqui_core::service_location!(create_task_branch), create_task_branch)
}

/// The preference naming the user's task source: `mantle`, or a `DevAideTaskSource` id.
pub const TASK_SOURCE: &str = "DEV_AIDE_TASK_SOURCE";
/// The preference naming the app a task opens in.
pub const TASK_APP: &str = "DEV_AIDE_TASK_APP";

const SOURCE: &str = "moqui.dev.DevAideTaskSource";
const APP: &str = "moqui.dev.DevAideTaskApp";
const TASKS_VIEW: &str = "mantle.work.effort.WorkEffortAndParty";
/// The task statuses that are done: the rest are open.
const DONE: &[&str] = &["WeComplete", "WeClosed", "WeCancelled"];
/// The most tasks a source answers.
const LIMIT: usize = 200;

/// One task as the panel shows it.
fn task(id: &str, title: &str, status: &str, project_id: &str, priority: Option<i64>) -> Json {
    json!({ "taskId": id, "key": id, "title": title, "statusId": status, "projectId": project_id, "priority": priority })
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// Whether this instance has Mantle's tasks.
fn mantle_loaded(context: &Context) -> bool {
    context.facade.find(TASKS_VIEW).is_ok()
}

/// The user's source: the preference, else `mantle` when Mantle is loaded, else the user's first `moqui-rest` row.
fn current_source(context: &Context, user: &str) -> Option<String> {
    if let Some(chosen) = moqui_core::user::get_preference(TASK_SOURCE)
        .ok()
        .flatten()
        .filter(|chosen| !chosen.is_empty())
    {
        return Some(chosen);
    }
    if mantle_loaded(context) {
        return Some("mantle".into());
    }
    context
        .facade
        .find(SOURCE)
        .ok()?
        .eq("userId", Value::string(user))
        .list()
        .ok()?
        .iter()
        .next()
        .map(|row| field(row, "taskSourceId"))
}

/// The user's open tasks on this instance, with their projects' names.
fn mantle_tasks(context: &Context, user: &str) -> Result<Vec<Json>, LibError> {
    let party = context
        .facade
        .find("moqui.security.UserAccount")?
        .eq("userId", Value::string(user))
        .one()?
        .map(|row| field(&row, "partyId"))
        .filter(|party| !party.is_empty())
        .ok_or_else(|| LibError::new("your user account has no party, so no task is assigned to it"))?;
    let rows = context
        .facade
        .find(TASKS_VIEW)?
        .eq("partyId", Value::string(&party))
        .eq("workEffortTypeEnumId", Value::string("WetTask"))
        .eq("thruDate", Value::Null)
        .order_by("priority", false)
        .order_by("workEffortId", false)
        .list()?;
    let mut tasks = Vec::new();
    for row in rows.iter() {
        let status = field(row, "statusId");
        if DONE.contains(&status.as_str())
            || tasks
                .iter()
                .any(|have: &Json| have["taskId"] == field(row, "workEffortId"))
        {
            continue;
        }
        let priority = row
            .get("priority")
            .and_then(|value| moqui_lib::object::to_plain_string(value).parse().ok());
        tasks.push(task(
            &field(row, "workEffortId"),
            &field(row, "workEffortName"),
            &status,
            &field(row, "rootWorkEffortId"),
            priority,
        ));
        if tasks.len() >= LIMIT {
            break;
        }
    }
    name_projects(context, &mut tasks);
    Ok(tasks)
}

/// Each task's project name, from this instance's work efforts.
fn name_projects(context: &Context, tasks: &mut [Json]) {
    let mut names: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for task in tasks.iter_mut() {
        let project = task["projectId"].as_str().unwrap_or("").to_string();
        if project.is_empty() {
            continue;
        }
        let name = names.entry(project.clone()).or_insert_with(|| {
            context
                .facade
                .find("mantle.work.effort.WorkEffort")
                .and_then(|find| find.eq("workEffortId", Value::string(&project)).one())
                .ok()
                .flatten()
                .map(|row| field(&row, "workEffortName"))
                .unwrap_or_default()
        });
        task["projectName"] = json!(name);
    }
}

/// A `DevAideTaskSource` row of the user.
fn source_row(context: &Context, id: &str, user: &str) -> Result<moqui_core::entity::EntityValue, LibError> {
    context
        .facade
        .find(SOURCE)?
        .eq("taskSourceId", Value::string(id))
        .one()?
        .filter(|row| field(row, "userId") == user)
        .ok_or_else(|| LibError::new(format!("no task source {id} of yours")))
}

/// A GET of another instance's REST API with the source's token, its JSON answer. A source Connect made
/// (`serverId`) reads with the group server's token, sent only to the server's own origin, whatever its `baseUrl` says
/// (plans/design-dev-aide.md 21.6).
fn rest_get(row: &moqui_core::entity::EntityValue, path: &str) -> Result<Json, LibError> {
    let server_id = field(row, "serverId");
    if !server_id.is_empty() {
        let server = crate::hosting::client::Server::load(
            &row_facade()?,
            &server_id,
            &moqui_core::user::current_user_id().ok().flatten().unwrap_or_default(),
        )?;
        // the task panel's read, with the key for tasks alone after an app sign-in (question 18)
        return server.tasks_get(path, &[], false);
    }
    let base = field(row, "baseUrl").trim_end_matches('/').to_string();
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err(LibError::new(
            "a task source's base URL starts with https:// or http://",
        ));
    }
    let token = field(row, "token");
    let request = moqui_core::outbound::Request {
        method: moqui_core::outbound::Method::Get,
        url: format!("{base}{path}"),
        headers: vec![("api_key".into(), token), ("accept".into(), "application/json".into())],
        body: None,
        timeout: Some(std::time::Duration::from_secs(20)),
        // the view tier, which runs no code of its own here, reaches no internal address through a source it stored;
        // the extend tier can already (a site service), and a team's server is often on the local network
        deny_internal: crate::tier::current().is_none_or(|tier| tier < crate::tier::Tier::Extend),
    };
    let response = moqui_core::outbound::send(request)?;
    if !(200..300).contains(&response.status) {
        let said: String = serde_json::from_str::<Json>(&response.body)
            .ok()
            .and_then(|answer| answer["errors"].as_str().map(str::to_string))
            .unwrap_or_default()
            .chars()
            .take(200)
            .collect();
        return Err(LibError::new(format!(
            "{base} answered {} {}{}",
            response.status,
            moqui_core::outbound::reason_phrase(response.status),
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        )));
    }
    serde_json::from_str(&response.body).map_err(|err| LibError::new(format!("{base} answered no JSON: {err}")))
}

/// The current context's entity facade.
fn row_facade() -> Result<moqui_core::entity::EntityFacade, LibError> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| LibError::new("no entity facade"))
}

/// `value` as a query parameter.
fn query(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The user's open tasks on another instance: Mantle's `my/tasks` (`get#MyTasks`, the tasks assigned to the token's
/// user that are not done), narrowed to the source's projects.
fn rest_tasks(row: &moqui_core::entity::EntityValue) -> Result<Vec<Json>, LibError> {
    let path = Some(field(row, "tasksPath"))
        .filter(|path| path.starts_with('/'))
        .unwrap_or_else(|| "/rest/s1/mantle/my/tasks".into());
    let separator = if path.contains('?') { '&' } else { '?' };
    let found = rest_get(row, &format!("{path}{separator}pageSize={LIMIT}"))?;
    let filter: Vec<String> = field(row, "projectFilter")
        .split(',')
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    let mut tasks: Vec<Json> = Vec::new();
    for found in found["taskList"].as_array().into_iter().flatten() {
        let text = |name: &str| found[name].as_str().unwrap_or("").to_string();
        let (id, status, project) = (text("workEffortId"), text("statusId"), text("rootWorkEffortId"));
        // the server's id becomes a commit's trailer: one short line, or the task is left out
        if !crate::git::task_key_valid(&id)
            || DONE.contains(&status.as_str())
            || (!filter.is_empty() && !filter.contains(&project))
        {
            continue;
        }
        let mut task = task(
            &id,
            &text("workEffortName"),
            &status,
            &project,
            found["priority"].as_i64(),
        );
        task["projectName"] = json!(text("projectName"));
        tasks.push(task);
    }
    Ok(tasks)
}

/// The name of the app a group server offers for its tasks (`tasks.app` of its offer), as the user's preference names it.
#[must_use]
pub fn offered_app_name(server_id: &str) -> String {
    format!("server-{server_id}")
}

/// A path of an offered app: on the server, with `{taskId}` or `{projectId}` in it, never another origin (no
/// backslash, white space or control character, which a browser reads as another host: `/\t/evil.example`).
fn offered_path(app: &Json, key: &str) -> String {
    app[key]
        .as_str()
        .map(str::trim)
        .filter(|path| moqui_lib::string::local_path(path))
        .unwrap_or_default()
        .to_string()
}

/// The app a connected server offers for its tasks (`tasks.app`: a title and the paths of a project, a task and a new
/// task), at the server's own address. `None` when it offers none, or no path of one.
pub(crate) fn offered_app(server: &crate::hosting::client::Server) -> Option<Json> {
    let app = &server.offer["tasks"]["app"];
    let (project, task, new_task) = (
        offered_path(app, "projectPath"),
        offered_path(app, "taskPath"),
        offered_path(app, "newTaskPath"),
    );
    if project.is_empty() && task.is_empty() {
        return None;
    }
    let title = app["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .map_or_else(|| server.label.clone(), |title| title.chars().take(100).collect());
    Some(json!({
        "name": offered_app_name(&server.id), "title": format!("{title} ({})", server.label), "projectPath": project,
        "taskPath": task, "newTaskPath": new_task, "baseUrl": server.base, "offeredBy": server.id,
    }))
}

/// A task's link in an app: the app's `path` (`taskPath`, `projectPath`) with the task's ids, at the app's base URL,
/// else at `base` (the task source's).
fn app_link(app: &Json, base: &str, path: &str, task: &Json) -> Option<String> {
    let template = app[path].as_str().filter(|template| !template.is_empty())?;
    let app_base = app["baseUrl"].as_str().filter(|base| !base.is_empty()).unwrap_or(base);
    Some(format!(
        "{}{}",
        app_base.trim_end_matches('/'),
        template
            .replace("{taskId}", &query(task["taskId"].as_str().unwrap_or("")))
            .replace("{projectId}", &query(task["projectId"].as_str().unwrap_or("")))
    ))
}

/// The tasks of the projects linked to the repository first, each group in its order.
#[must_use]
pub fn linked_first(tasks: Vec<Json>, project_ids: &[String]) -> Vec<Json> {
    let (mut linked, other): (Vec<Json>, Vec<Json>) = tasks.into_iter().partition(|task| {
        task["projectId"]
            .as_str()
            .is_some_and(|project| project_ids.iter().any(|id| id == project))
    });
    linked.extend(other);
    linked
}

/// The apps a task opens in: the conf's `task-app` elements, then the `DevAideTaskApp` rows (a row of the same name
/// replaces the conf's), then the apps the user's group servers offer.
fn apps(context: &Context) -> Vec<Json> {
    let mut apps: Vec<Json> = Vec::new();
    if let Ok(conf) = MNode::parse_shared("moqui-conf", &context.factory.conf_text()) {
        for element in conf.named_iter("dev-aide") {
            for app in element.named_iter("task-app") {
                let attr = |name: &str| app.attribute(name).unwrap_or_default();
                apps.push(json!({
                    "name": attr("name"), "title": attr("title"), "projectPath": attr("project-path"),
                    "taskPath": attr("task-path"), "newTaskPath": attr("new-task-path"), "baseUrl": attr("base-url"),
                }));
            }
        }
    }
    if let Ok(rows) = context.facade.find(APP).and_then(|find| find.list()) {
        for row in rows.iter() {
            let name = field(row, "taskAppName");
            apps.retain(|app| app["name"] != name.as_str());
            apps.push(json!({
                "name": name, "title": field(row, "title"), "projectPath": field(row, "projectPath"),
                "taskPath": field(row, "taskPath"), "newTaskPath": field(row, "newTaskPath"), "baseUrl": field(row, "baseUrl"),
            }));
        }
    }
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let _authz = ExecutionContext::with_current(|ec| ec.disable_authz());
    for server in crate::hosting::client::Server::all(&context.facade, &user) {
        if let Some(app) = offered_app(&server) {
            apps.push(app);
        }
    }
    apps
}

/// `get#Tasks`: the user's open tasks from their source, the apps (the chosen one first) and each task's link in it.
/// With `rootId` of a working tree whose hosted repository's server offers its issues (plans/design-dev-aide.md 21.6):
/// the repository's open issues too, and the tasks of the projects linked to it first.
fn get_tasks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let source = current_source(&context, &user);
    let (label, base, found) = match source.as_deref() {
        None => ("No task source".to_string(), String::new(), Ok(Vec::new())),
        Some("mantle") => (
            "This instance".to_string(),
            String::new(),
            mantle_tasks(&context, &user),
        ),
        Some(id) => match source_row(&context, id, &user) {
            Ok(row) => {
                let label = Some(field(&row, "label"))
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| field(&row, "baseUrl"));
                let base = field(&row, "baseUrl").trim_end_matches('/').to_string();
                (label, base, rest_tasks(&row))
            }
            Err(err) => (id.to_string(), String::new(), Err(err)),
        },
    };
    let mut apps = apps(&context);
    let chosen = moqui_core::user::get_preference(TASK_APP)
        .ok()
        .flatten()
        .unwrap_or_default();
    if let Some(at) = apps.iter().position(|app| app["name"] == chosen.as_str()) {
        let app = apps.remove(at);
        apps.insert(0, app);
    }
    let app = apps.first().cloned();
    let link = |path: &str, task: &Json| -> Option<String> { app_link(app.as_ref()?, &base, path, task) };
    let mut out = Parameters::new();
    // the issues of the working tree's hosted repository, and its projects (21.6); a server that fails them leaves
    // the tasks
    let mut linked: Vec<String> = Vec::new();
    let root_id = text(parameters, "rootId");
    if let Some(root) = Some(root_id.as_str())
        .filter(|id| !id.is_empty())
        .and_then(|id| context.root(id).ok())
    {
        let _authz = ec.disable_authz();
        match crate::hosting::issues::root_issues(&context, root) {
            Ok(Some((hosted, found))) => {
                let server_app = offered_app(&hosted.server);
                let issues: Vec<Json> = found
                    .issues
                    .into_iter()
                    .map(|mut issue| {
                        let page = issue["pageUrl"].take();
                        issue["url"] = server_app
                            .as_ref()
                            .and_then(|app| app_link(app, &hosted.server.base, "taskPath", &issue))
                            .map_or(page, Json::String);
                        issue
                    })
                    .collect();
                linked = found
                    .projects
                    .iter()
                    .filter_map(|project| project["projectId"].as_str().map(str::to_string))
                    .collect();
                out.insert("issues".into(), json_value(&issues));
                out.insert("issueProjects".into(), json_value(&found.projects));
                out.insert("numbered".into(), Value::Bool(found.numbered));
                out.insert(
                    "repositoryPath".into(),
                    json_value(&hosted.repository["repositoryPath"]),
                );
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!("Dev AIDE: the issues of {} failed: {err}", root.label);
                out.insert("issuesError".into(), Value::string(err.to_string()));
            }
        }
    }
    match found {
        Ok(tasks) => {
            let mut tasks = linked_first(tasks, &linked);
            for task in &mut tasks {
                task["url"] = json!(link("taskPath", task));
                task["projectUrl"] = json!(link("projectPath", task));
            }
            out.insert("tasks".into(), json_value(&tasks));
        }
        Err(err) => {
            tracing::warn!("Dev AIDE: the task source {label} failed: {err}");
            out.insert("tasks".into(), Value::List(Vec::new()));
            out.insert("error".into(), Value::string(err.to_string()));
        }
    }
    out.insert("source".into(), source.map_or(Value::Null, Value::string));
    out.insert("sourceLabel".into(), Value::string(label));
    out.insert("apps".into(), json_value(&apps));
    Ok(out)
}

/// `get#TaskSources`: what the user may pick: this instance (when Mantle is loaded) and their own `moqui-rest` rows,
/// without their tokens.
fn get_task_sources(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let mut sources = Vec::new();
    if mantle_loaded(&context) {
        sources.push(json!({ "taskSourceId": "mantle", "kind": "mantle", "label": "This instance's tasks" }));
    }
    for row in context
        .facade
        .find(SOURCE)?
        .eq("userId", Value::string(&user))
        .list()?
        .iter()
    {
        sources.push(json!({
            "taskSourceId": field(row, "taskSourceId"), "kind": field(row, "kind"), "label": field(row, "label"),
            "baseUrl": field(row, "baseUrl"), "username": field(row, "username"), "projectFilter": field(row, "projectFilter"),
        }));
    }
    let mut out = Parameters::new();
    out.insert("sources".into(), json_value(&sources));
    out.insert(
        "current".into(),
        current_source(&context, &user).map_or(Value::Null, Value::string),
    );
    out.insert("apps".into(), json_value(&apps(&context)));
    Ok(out)
}

/// `set#TaskSource`: the user's source and, with `taskAppName`, the app a task opens in.
fn set_task_source(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let id = text(parameters, "taskSourceId");
    if !id.is_empty() && id != "mantle" {
        source_row(&context, &id, &user)?;
    }
    if parameters.contains_key("taskSourceId") {
        moqui_core::user::set_preference(TASK_SOURCE, Some(id.as_str()).filter(|id| !id.is_empty()))?;
    }
    let app = text(parameters, "taskAppName");
    if !app.is_empty() {
        moqui_core::user::set_preference(TASK_APP, Some(app.as_str()))?;
    }
    Ok(Parameters::new())
}

/// `store#TaskSource`: the user's own `moqui-rest` source, new or (with `taskSourceId`) changed; a token left empty on
/// a change keeps the one stored.
fn store_task_source(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let base = text(parameters, "baseUrl").trim().to_string();
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err(LibError::new("the base URL starts with https:// or http://"));
    }
    let id = text(parameters, "taskSourceId");
    let mut row = if id.is_empty() {
        let mut row = context.facade.make_value(SOURCE)?;
        row.set_sequenced_id_primary(&context.facade)?;
        row.set("userId", Value::string(&user));
        row.set("kind", Value::string("moqui-rest"));
        row
    } else {
        source_row(&context, &id, &user)?
    };
    // a source Connect made reads with the group server's token; at another address it is a source of its own, which
    // needs its own token
    if !field(&row, "serverId").is_empty() && field(&row, "baseUrl").trim_end_matches('/') != base.trim_end_matches('/')
    {
        row.set("serverId", Value::Null);
        row.set("token", Value::Null);
        if text(parameters, "token").trim().is_empty() {
            return Err(LibError::new(
                "this source read with the group server's token; at another address it needs a token of its own",
            ));
        }
    }
    row.set("label", Value::string(text(parameters, "label").trim()));
    row.set("baseUrl", Value::string(&base));
    row.set("username", Value::string(text(parameters, "username").trim()));
    row.set("projectFilter", Value::string(text(parameters, "projectFilter").trim()));
    let token = text(parameters, "token");
    if !token.trim().is_empty() {
        row.set("token", Value::string(token.trim()));
    } else if id.is_empty() {
        return Err(LibError::new("an access token is required"));
    }
    row.create_or_update(&context.facade)?;
    let mut out = Parameters::new();
    out.insert("taskSourceId".into(), Value::string(field(&row, "taskSourceId")));
    Ok(out)
}

/// `delete#TaskSource`: one of the user's own sources.
fn delete_task_source(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    source_row(&context, &text(parameters, "taskSourceId"), &user)?.delete(&context.facade)?;
    Ok(Parameters::new())
}

/// Why `path` is no path template of a task app, or None: a path on the app's server (as an offered app's paths are,
/// [`offered_path`]: from one `/`, with no backslash, white space or control character), with `placeholder`
/// (`{taskId}`, `{projectId}`) in it. Empty is none given.
#[must_use]
pub fn app_path_problem(path: &str, placeholder: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    if !moqui_lib::string::local_path(path) {
        return Some(format!(
            "{} is not a path on the server: it starts with one /, with no backslash, space or control character",
            path.escape_debug()
        ));
    }
    if !path.contains(placeholder) {
        return Some(format!("{path} has no {placeholder}"));
    }
    None
}

/// A task app's name from its title: its letters and digits, lower case, other runs as `-`.
#[must_use]
pub fn app_name_of(title: &str) -> String {
    let mut name = String::new();
    for ch in title.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else if !name.ends_with('-') && !name.is_empty() {
            name.push('-');
        }
    }
    name.trim_end_matches('-').chars().take(40).collect()
}

/// The names of the conf's `task-app` elements.
/// The names and titles of the conf's `task-app` elements.
fn conf_apps(context: &Context) -> Vec<(String, String)> {
    let Ok(conf) = MNode::parse_shared("moqui-conf", &context.factory.conf_text()) else {
        return Vec::new();
    };
    conf.named_iter("dev-aide")
        .flat_map(|element| {
            element
                .named_iter("task-app")
                .map(|app| {
                    (
                        app.attribute("name").unwrap_or_default(),
                        app.attribute("title").unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `store#TaskApp`: an app tasks open in, as the conf's `task-app`, added in Settings: its title, its project and task
/// path templates (each a path on the server with `{projectId}` or `{taskId}`, [`app_path_problem`]), a new task's,
/// and a base URL when it is another instance (the full tier's). Its name is the title's when not given. The extend
/// tier adds one; one already there is changed by who added it or the full tier, and one replacing a conf `task-app`
/// only by the full tier.
fn store_task_app(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let title = text(parameters, "title").trim().to_string();
    if title.is_empty() || title.chars().count() > 100 {
        return Err(LibError::new("a task app has a title of at most 100 characters"));
    }
    let mut name = text(parameters, "taskAppName").trim().to_string();
    if name.is_empty() {
        name = app_name_of(&title);
    }
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(LibError::new("a task app's name is letters, digits, - and _"));
    }
    let path = |key: &str| text(parameters, key).trim().to_string();
    let (project, task, new_task) = (path("projectPath"), path("taskPath"), path("newTaskPath"));
    if project.is_empty() && task.is_empty() {
        return Err(LibError::new("a task app has a project path or a task path"));
    }
    let problems: Vec<String> = [
        app_path_problem(&project, "{projectId}"),
        app_path_problem(&task, "{taskId}"),
        app_path_problem(&new_task, ""),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !problems.is_empty() {
        return Err(LibError::new(problems.join("; ")));
    }
    let base = path("baseUrl");
    if !base.is_empty() && !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err(LibError::new("the base URL starts with https:// or http://"));
    }
    // every user's Open in offers it: an app at another address only by the full tier
    if !base.is_empty() && context.tier != Tier::Full {
        return Err(LibError::new(
            "a task app at another instance (a base URL) is the full tier's",
        ));
    }
    let existing = context
        .facade
        .find(APP)?
        .eq("taskAppName", Value::string(&name))
        .one()?;
    if context.tier != Tier::Full {
        if let Some(row) = &existing {
            if field(row, "createdByUserId") != user {
                return Err(LibError::new(format!(
                    "the task app {name} was added by another: only they or the full tier change it"
                )));
            }
        } else if conf_apps(&context).iter().any(|(conf_name, _)| *conf_name == name) {
            return Err(LibError::new(format!(
                "{name} is a task app of the conf: only the full tier replaces it"
            )));
        }
    }
    // another app under a conf app's title would pass for it in every user's Open in
    let taken = conf_apps(&context)
        .into_iter()
        .any(|(conf_name, conf_title)| conf_name != name && conf_title.trim().eq_ignore_ascii_case(&title));
    if taken {
        return Err(LibError::new(format!("{title} is the title of a task app of the conf")));
    }
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = context.facade.make_value(APP)?;
            row.set("taskAppName", Value::string(&name));
            row.set("createdByUserId", Value::string(&user));
            row
        }
    };
    row.set("title", Value::string(&title));
    row.set("projectPath", Value::string(&project));
    row.set("taskPath", Value::string(&task));
    row.set("newTaskPath", Value::string(&new_task));
    row.set("baseUrl", Value::string(&base));
    row.create_or_update(&context.facade)?;
    let mut out = Parameters::new();
    out.insert("taskAppName".into(), Value::string(name));
    Ok(out)
}

/// `get#TaskApps`: the task apps added in Settings, each with who added it and whether the user may remove it.
fn get_task_apps(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let may_add = context.tier >= Tier::Extend;
    let mut apps = Vec::new();
    let rows = context.facade.find(APP)?.order_by("title", false).list()?;
    for row in rows.iter() {
        let by = field(row, "createdByUserId");
        apps.push(json!({
            "taskAppName": field(row, "taskAppName"), "title": field(row, "title"),
            "projectPath": field(row, "projectPath"), "taskPath": field(row, "taskPath"),
            "baseUrl": field(row, "baseUrl"), "createdByUserId": by,
            "mayRemove": may_add && (context.tier == Tier::Full || by == user),
        }));
    }
    let mut out = Parameters::new();
    out.insert("apps".into(), json_value(&apps));
    out.insert("mayAdd".into(), Value::Bool(may_add));
    Ok(out)
}

/// `delete#TaskApp`: a task app added in Settings, by who added it or the full tier.
fn delete_task_app(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let name = text(parameters, "taskAppName");
    let row = context
        .facade
        .find(APP)?
        .eq("taskAppName", Value::string(&name))
        .one()?
        .ok_or_else(|| LibError::new(format!("no task app {name}")))?;
    if context.tier != Tier::Full && field(&row, "createdByUserId") != user {
        return Err(LibError::new(format!(
            "the task app {name} was added by another: only they or the full tier remove it"
        )));
    }
    row.delete(&context.facade)?;
    Ok(Parameters::new())
}

/// `create#Task`: Quick create, a task with a title in a project, assigned to the user, on this instance (Mantle's
/// `create#Task`). A `moqui-rest` source's tasks are made in its own app, which the panel opens.
fn create_task(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    if current_source(&context, &user).as_deref() != Some("mantle") {
        return Err(LibError::new(
            "Quick create makes a task on this instance; for another instance's, open its app",
        ));
    }
    let title = text(parameters, "title");
    if title.trim().is_empty() {
        return Err(LibError::new("a task needs a title"));
    }
    let party = context
        .facade
        .find("moqui.security.UserAccount")?
        .eq("userId", Value::string(&user))
        .one()?
        .map(|row| field(&row, "partyId"))
        .unwrap_or_default();
    let mut call = std::collections::BTreeMap::new();
    call.insert("workEffortName".to_string(), Value::string(title.trim()));
    let project = text(parameters, "projectId");
    if !project.is_empty() {
        call.insert("rootWorkEffortId".to_string(), Value::string(&project));
    }
    if !party.is_empty() {
        call.insert("assignToPartyId".to_string(), Value::string(&party));
    }
    let made = moqui_core::service::sync("mantle.work.TaskServices.create#Task", &call)?;
    let mut out = Parameters::new();
    out.insert(
        "taskId".into(),
        made.get("workEffortId").cloned().unwrap_or(Value::Null),
    );
    Ok(out)
}

/// A branch name for a task: its key, then the title's first words, lower case, joined by `-`.
#[must_use]
pub fn branch_name(key: &str, title: &str) -> String {
    let words: Vec<String> = title
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(5)
        .map(str::to_ascii_lowercase)
        .collect();
    let key: String = key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if words.is_empty() {
        key
    } else {
        format!("{key}-{}", words.join("-"))
    }
}

/// `create#TaskBranch`: a local branch named for the task, switched to, and the task made the root's current one.
fn create_task_branch(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = crate::vcs::git_write_root(&context, parameters, "makes branches")?;
    let key = text(parameters, "taskId").trim().to_string();
    // the trailer its commits get: the task's key, or what names an issue in its repository (`#12`); each one short
    // line, or a commit would get lines of its own
    let trailer = Some(text(parameters, "trailer").trim().to_string())
        .filter(|trailer| !trailer.is_empty())
        .unwrap_or_else(|| key.clone());
    if !crate::git::task_key_valid(&key) || !crate::git::task_key_valid(&trailer) {
        return Err(LibError::new("a task key is one short line"));
    }
    let name = branch_name(&key, &text(parameters, "title"));
    crate::vcs::valid_branch(&name)?;
    let output = context
        .git()
        .run(&root.path, Mode::Write, &["switch", "-c", &name], &[], None)?
        .checked("switch -c")?;
    crate::vcs::audit_git(&context, root, "branch", json!({ "branch": name, "task": key }));
    moqui_core::user::set_preference(
        &format!("{}.{}", crate::vcs::CURRENT_TASK, root.id),
        Some(trailer.as_str()),
    )?;
    let mut out = Parameters::new();
    out.insert("branch".into(), Value::string(name));
    out.insert(
        "output".into(),
        Value::string(format!("{}{}", output.stdout, output.stderr)),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_tasks_of_linked_projects_come_first() {
        let task = |id: &str, project: &str| serde_json::json!({ "taskId": id, "projectId": project });
        let sorted = super::linked_first(
            vec![task("1", "A"), task("2", "B"), task("3", ""), task("4", "B")],
            &["B".to_string()],
        );
        let ids: Vec<&str> = sorted.iter().map(|task| task["taskId"].as_str().unwrap()).collect();
        assert_eq!(ids, ["2", "4", "1", "3"]);
        assert_eq!(super::linked_first(vec![task("1", "A")], &[]).len(), 1);
    }

    #[test]
    fn a_task_branch_is_its_key_and_first_words() {
        assert_eq!(
            super::branch_name("PROJ-12", "Add a due date to orders!"),
            "PROJ-12-add-a-due-date-to"
        );
        assert_eq!(super::branch_name("100234", ""), "100234");
        assert_eq!(super::branch_name("A B", "Fix: it"), "A-B-fix-it");
    }

    /// A task app's paths are paths on its server with their placeholder, as an offered app's are; its name is its
    /// title's.
    #[test]
    fn a_task_apps_paths_are_on_the_server_with_their_placeholder() {
        use super::app_path_problem as problem;
        assert_eq!(
            problem(
                "/qapps/Projects/WorkItems/WorkItemDetail?workEffortId={taskId}",
                "{taskId}"
            ),
            None
        );
        assert_eq!(problem("", "{taskId}"), None);
        for off in [
            "https://elsewhere.example.com/{taskId}",
            "//elsewhere.example.com/{taskId}",
            "/a\\b/{taskId}",
            "x/{taskId}",
            // a browser drops a tab, a newline or a carriage return in a URL: /<tab>/evil.example is that host
            "/\t/evil.example/{taskId}",
            "/\n/evil.example/{taskId}",
            "/\r/evil.example/{taskId}",
            "/a b/{taskId}",
            "/a\u{7}b/{taskId}",
        ] {
            assert!(
                problem(off, "{taskId}").unwrap().contains("not a path on the server"),
                "{off}"
            );
        }
        assert!(problem("/qapps/Projects/{projectId}", "{taskId}")
            .unwrap()
            .contains("has no {taskId}"));
        assert_eq!(super::app_name_of(" Team Projects (EU) "), "team-projects-eu");
        assert_eq!(super::app_name_of("!!"), "");
    }
}
