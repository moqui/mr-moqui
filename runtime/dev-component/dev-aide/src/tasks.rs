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

/// A GET of another instance's REST API with the source's token, its JSON answer.
fn rest_get(row: &moqui_core::entity::EntityValue, path: &str) -> Result<Json, LibError> {
    let base = field(row, "baseUrl").trim_end_matches('/').to_string();
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return Err(LibError::new(
            "a task source's base URL starts with https:// or http://",
        ));
    }
    let request = moqui_core::outbound::Request {
        method: moqui_core::outbound::Method::Get,
        url: format!("{base}{path}"),
        headers: vec![
            ("api_key".into(), field(row, "token")),
            ("accept".into(), "application/json".into()),
        ],
        body: None,
        timeout: Some(std::time::Duration::from_secs(20)),
        deny_internal: false,
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
    let found = rest_get(row, &format!("/rest/s1/mantle/my/tasks?pageSize={LIMIT}"))?;
    let filter: Vec<String> = field(row, "projectFilter")
        .split(',')
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    let mut tasks: Vec<Json> = Vec::new();
    for found in found["taskList"].as_array().into_iter().flatten() {
        let text = |name: &str| found[name].as_str().unwrap_or("").to_string();
        let (id, status, project) = (text("workEffortId"), text("statusId"), text("rootWorkEffortId"));
        if id.is_empty() || DONE.contains(&status.as_str()) || (!filter.is_empty() && !filter.contains(&project)) {
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

/// The apps a task opens in: the conf's `task-app` elements, then the `DevAideTaskApp` rows (a row of the same name
/// replaces the conf's).
fn apps(context: &Context) -> Vec<Json> {
    let mut apps: Vec<Json> = Vec::new();
    if let Ok(conf) = MNode::parse_shared("moqui-conf", context.factory.conf_text()) {
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
    apps
}

/// `get#Tasks`: the user's open tasks from their source, the apps (the chosen one first) and each task's link in it.
fn get_tasks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
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
    let link = |path: &str, task: &Json| -> Option<String> {
        let app = app.as_ref()?;
        let template = app[path].as_str().filter(|template| !template.is_empty())?;
        let app_base = app["baseUrl"]
            .as_str()
            .filter(|base| !base.is_empty())
            .map_or(base.as_str(), |base| base);
        Some(format!(
            "{}{}",
            app_base.trim_end_matches('/'),
            template
                .replace("{taskId}", &query(task["taskId"].as_str().unwrap_or("")))
                .replace("{projectId}", &query(task["projectId"].as_str().unwrap_or("")))
        ))
    };
    let mut out = Parameters::new();
    match found {
        Ok(mut tasks) => {
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

/// `store#TaskApp`: an app tasks open in, as the conf's `task-app` (one of the same name replaced). The full tier's.
fn store_task_app(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Full)?;
    let _authz = ec.disable_authz();
    let name = text(parameters, "taskAppName");
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(LibError::new("a task app's name is letters, digits, - and _"));
    }
    let mut row = context.facade.make_value(APP)?;
    row.set("taskAppName", Value::string(&name));
    for key in ["title", "projectPath", "taskPath", "newTaskPath", "baseUrl"] {
        row.set(key, Value::string(text(parameters, key).trim()));
    }
    row.create_or_update(&context.facade)?;
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
    let root = crate::vcs::git_root(&context, parameters)?;
    let key = text(parameters, "taskId");
    let name = branch_name(&key, &text(parameters, "title"));
    crate::vcs::valid_branch(&name)?;
    let output = context
        .git()
        .run(&root.path, Mode::Write, &["switch", "-c", &name], &[], None)?
        .checked("switch -c")?;
    crate::vcs::audit_git(&context, root, "branch", json!({ "branch": name, "task": key }));
    moqui_core::user::set_preference(&format!("{}.{}", crate::vcs::CURRENT_TASK, root.id), Some(key.as_str()))?;
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
    fn a_task_branch_is_its_key_and_first_words() {
        assert_eq!(
            super::branch_name("PROJ-12", "Add a due date to orders!"),
            "PROJ-12-add-a-due-date-to"
        );
        assert_eq!(super::branch_name("100234", ""), "100234");
        assert_eq!(super::branch_name("A B", "Fix: it"), "A-B-fix-it");
    }
}
