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

//! The issues of a working tree's hosted repository (plans/design-dev-aide.md 21.6): its open issues and the projects
//! it is linked to, read through the offer's `tasks.issuesPath` (Mantle's `gitRepositories/{repositoryId}/issues`) as
//! the connected user, for the task panel; and Quick create, which files one there. A server that offers no such path
//! has no issues here, and the panel shows the user's tasks alone.

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::client::{self, query_text, Server};
use super::review::{hosted, need_hosted, page_url, Hosted};
use crate::roots::Root;
use crate::tier::Tier;
use crate::vcs::git_root;
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(create_root_issue), create_root_issue)
}

/// The REST path of a repository's issues on a server: the offer's `tasks.issuesPath` with `{repositoryId}`, a path
/// on the server. `None` when the server offers none.
#[must_use]
pub fn issues_path(template: &str, repository_id: &str) -> Option<String> {
    let template = template.trim();
    if !template.starts_with('/') || template.starts_with("//") || !template.contains("{repositoryId}") {
        return None;
    }
    let id = repository_id.trim();
    if id.is_empty() {
        return None;
    }
    Some(template.replace("{repositoryId}", &query_text(id)))
}

/// One issue of a server's answer, as the task panel shows it: its id (the work item's), its number in the repository
/// when it has one, the key a commit names it by (`#12` with a number, else the id), its title, status and project.
#[must_use]
pub fn issue_item(item: &Json) -> Option<Json> {
    let pick = |keys: &[&str]| -> Json {
        keys.iter()
            .map(|key| &item[*key])
            .find(|value| !value.is_null())
            .cloned()
            .unwrap_or(Json::Null)
    };
    let as_text = |value: Json| match value {
        Json::String(text) => text,
        Json::Number(number) => number.to_string(),
        _ => String::new(),
    };
    let id = as_text(pick(&["workEffortId", "issueId", "id"]));
    let number = pick(&["issueNumber", "number"]);
    let number = number
        .as_i64()
        .or_else(|| number.as_str().and_then(|text| text.trim().parse().ok()))
        .filter(|number| *number > 0);
    if id.is_empty() && number.is_none() {
        return None;
    }
    let key = number.map_or_else(|| id.clone(), |number| format!("#{number}"));
    Some(json!({
        "taskId": if id.is_empty() { key.clone() } else { id },
        "number": number,
        "key": key,
        "title": as_text(pick(&["workEffortName", "title", "name"])),
        "statusId": as_text(pick(&["statusId", "state", "status"])),
        "projectId": as_text(pick(&["rootWorkEffortId", "projectId"])),
        "projectName": as_text(pick(&["projectName"])),
        "priority": pick(&["priority"]),
        "pageUrl": pick(&["url", "pageUrl"]),
    }))
}

/// A project of a server's answer: its id and name.
fn project_item(item: &Json) -> Option<Json> {
    let id = item["workEffortId"]
        .as_str()
        .or_else(|| item["projectId"].as_str())
        .filter(|id| !id.is_empty())?;
    let name = item["workEffortName"]
        .as_str()
        .or_else(|| item["projectName"].as_str())
        .unwrap_or("");
    Some(json!({ "projectId": id, "projectName": name }))
}

/// The open issues of a repository as the server answered: the issues, the linked projects and whether issues have
/// numbers.
#[derive(Debug, Default, PartialEq)]
pub struct Issues {
    pub issues: Vec<Json>,
    pub projects: Vec<Json>,
    pub numbered: bool,
}

/// The issues of an answer of the issues path (Mantle's `get#RepositoryIssues`: `issueList`, `projectList`,
/// `numbered`; another server's list of the same fields), each link a page of `server` only.
#[must_use]
pub fn read_issues(answer: &Json, server: Option<&Server>) -> Issues {
    let answer = client::result_of(answer);
    let list = |names: &[&str]| -> Vec<Json> {
        names
            .iter()
            .find_map(|name| answer[*name].as_array())
            .cloned()
            .unwrap_or_default()
    };
    let issues: Vec<Json> = list(&["issueList", "issues"])
        .iter()
        .filter_map(issue_item)
        .map(|mut issue| {
            let page = issue["pageUrl"].take();
            issue["pageUrl"] = server.map_or(Json::Null, |server| page_url(server, &page));
            issue
        })
        .collect();
    let numbered = answer["numbered"]
        .as_bool()
        .unwrap_or_else(|| issues.iter().any(|issue| !issue["number"].is_null()));
    Issues {
        issues,
        projects: list(&["projectList", "projects"])
            .iter()
            .filter_map(project_item)
            .collect(),
        numbered,
    }
}

/// The open issues of `root`'s hosted repository, when it has one known for the user and its server offers an issues
/// path; `Ok(None)` otherwise. The repository is not resolved here: the Workspace does that on its own.
pub(crate) fn root_issues(context: &Context, root: &Root) -> Result<Option<(Hosted, Issues)>, LibError> {
    let Some(hosted) = hosted(context, root) else {
        return Ok(None);
    };
    let Some(path) = issues_path(&hosted.server.offered("tasks.issuesPath"), &hosted.id()) else {
        return Ok(None);
    };
    let answer = hosted.server.get(&path, &[("pageSize", "200")])?;
    let issues = read_issues(&answer, Some(&hosted.server));
    Ok(Some((hosted, issues)))
}

/// `create#RootIssue`: Quick create on a hosted repository, an issue filed in its tracker on the server as the user (a
/// task of a project the repository is linked to, `projectId` or the first). A write on the server, from a session only.
fn create_root_issue(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "An issue is filed from a session, not a request logged in by a header",
        ));
    }
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let title = text(parameters, "title").trim().to_string();
    if title.is_empty() {
        return Err(LibError::new("an issue needs a title"));
    }
    let hosted = need_hosted(&context, root)?;
    let path = issues_path(&hosted.server.offered("tasks.issuesPath"), &hosted.id()).ok_or_else(|| {
        LibError::new(format!(
            "{} offers no issues of its repositories: make the task in its app",
            hosted.server.base
        ))
    })?;
    let project = text(parameters, "projectId");
    let mut body = json!({ "workEffortName": title, "title": title });
    if !project.is_empty() {
        body["rootWorkEffortId"] = json!(project);
    }
    let answer = hosted.server.post(&path, &body)?;
    let made = client::result_of(&answer);
    let issue = issue_item(&json!({
        "workEffortId": made["workEffortId"].clone(), "issueId": made["issueId"].clone(), "id": made["id"].clone(),
        "issueNumber": made["issueNumber"].clone(), "number": made["number"].clone(), "workEffortName": title,
        "rootWorkEffortId": made["rootWorkEffortId"].clone(),
    }))
    .ok_or_else(|| LibError::new(format!("{} answered no issue", hosted.server.base)))?;
    crate::vcs::audit_git(
        &context,
        root,
        "issue",
        json!({ "server": hosted.server.base, "repository": hosted.id(), "issue": issue["key"] }),
    );
    let mut out = Parameters::new();
    out.insert("issue".into(), json_value(&issue));
    out.insert("taskId".into(), Value::string(issue["taskId"].as_str().unwrap_or("")));
    Ok(out)
}
