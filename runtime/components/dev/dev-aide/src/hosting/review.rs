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

//! The hosted repository of a working tree and its merge requests (plans/design-dev-aide.md 21.3, 21.4, D21): a
//! remote's URL that belongs to a connected server is resolved there (`/rest/s1/moqui/hosting/repository`) and kept on
//! the root; the merge requests are listed, opened, checked out (`refs/merge-requests/<n>/head`) and, for a user who
//! may not push, proposed by a push to `refs/for/<branch>`; Fork makes the user's fork on a server that offers forks.
//! Each call goes to the server as the user, and the server decides. A repository an application keeps (Smithy) is
//! reached through the application's REST, which the server offers (`application.restBase`); any other through the
//! framework's. Merging stays on the server's page (question 17).

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::client::{self, body, field, query_text, Application, Server};
use super::connect::current_user;
use crate::git::Mode;
use crate::roots::Root;
use crate::tier::Tier;
use crate::vcs::{audit_git, git_root, git_write_root, network_allowed, run_network_with};
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(
        moqui_core::service_location!(resolve_root_hosting),
        resolve_root_hosting,
    )?;
    registry.service_function(moqui_core::service_location!(get_merge_requests), get_merge_requests)?;
    registry.service_function(moqui_core::service_location!(get_merge_request), get_merge_request)?;
    registry.service_function(moqui_core::service_location!(get_issue), get_issue)?;
    registry.service_function(moqui_core::service_location!(open_merge_request), open_merge_request)?;
    registry.service_function(moqui_core::service_location!(propose_changes), propose_changes)?;
    registry.service_function(
        moqui_core::service_location!(checkout_merge_request),
        checkout_merge_request,
    )?;
    registry.service_function(moqui_core::service_location!(fork_repository), fork_repository)
}

/// Which REST a repository's calls go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Via {
    Framework,
    Application(Application),
}

/// The application's REST when the server offers one and the repository is the application's (`keptBy`), else the
/// framework's.
pub(crate) fn via(server: &Server, repository: &Json) -> Via {
    let kept = repository["keptBy"]["application"].as_str().unwrap_or("");
    match server.application() {
        Some(application) if !kept.is_empty() && application.name == kept => Via::Application(application),
        _ => Via::Framework,
    }
}

/// A REST call to make: what [`Server::request`] takes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Call {
    pub post: bool,
    pub path: String,
    pub query: Vec<(&'static str, String)>,
    pub body: Option<Json>,
}

impl Call {
    pub(crate) fn send(&self, server: &Server) -> Result<Json, LibError> {
        let query: Vec<(&str, &str)> = self.query.iter().map(|(key, value)| (*key, value.as_str())).collect();
        let method = if self.post {
            moqui_core::outbound::Method::Post
        } else {
            moqui_core::outbound::Method::Get
        };
        server.request(method, &self.path, &query, self.body.as_ref())
    }
}

/// The merge requests of a repository: `mine`, `review` (waiting for the user's review), `open` or `all`.
pub(crate) fn list_call(via: &Via, repositories: &str, repository_id: &str, filter: &str, username: &str) -> Call {
    let id = query_text(repository_id);
    match via {
        Via::Framework => Call {
            post: false,
            path: format!("{repositories}/{id}/mergeRequests"),
            query: vec![("filter", filter.to_string())],
            body: None,
        },
        Via::Application(application) => {
            let mut query = vec![("repositoryId", repository_id.to_string())];
            if filter != "all" {
                query.push(("state", "open".into()));
            }
            match filter {
                "mine" => query.push(("authorHandle", username.to_string())),
                "review" => query.push(("reviewerHandle", username.to_string())),
                _ => {}
            }
            Call {
                post: false,
                path: format!("{}/merge-requests", application.rest_base),
                query,
                body: None,
            }
        }
    }
}

/// What a merge request is opened with.
#[derive(Clone, Debug, Default)]
pub(crate) struct Opening {
    pub source: String,
    pub target: String,
    pub title: String,
    pub description: String,
    /// The fork the source branch is in, when it is not the repository's own.
    pub source_repository: String,
    /// The remote the source branch is pushed to, when it is not the branch's own (`branch.<name>.pushRemote`, then
    /// `.remote`): a fork's remote names the fork the merge request comes from.
    pub source_remote: String,
}

pub(crate) fn create_call(via: &Via, repositories: &str, repository_id: &str, opening: &Opening) -> Call {
    let id = query_text(repository_id);
    match via {
        Via::Framework => Call {
            post: true,
            path: format!("{repositories}/{id}/mergeRequests"),
            query: Vec::new(),
            body: Some(body(&[
                ("sourceRef", json!(opening.source)),
                ("targetRef", json!(opening.target)),
                ("title", json!(opening.title)),
                ("description", json!(opening.description)),
                ("sourceRepositoryId", json!(opening.source_repository)),
            ])),
        },
        Via::Application(application) => Call {
            post: true,
            path: format!("{}/repositories/{id}/merge-requests", application.rest_base),
            query: Vec::new(),
            body: Some(body(&[
                ("sourceRef", json!(opening.source)),
                ("targetRef", json!(opening.target)),
                ("title", json!(opening.title)),
                ("body", json!(opening.description)),
                ("sourceFormat", json!("markdown")),
                ("sourceRepositoryId", json!(opening.source_repository)),
                ("operationId", json!(client::operation_id())),
            ])),
        },
    }
}

/// A fork of the repository in the user's namespace: only where the server offers forks, through its application.
pub(crate) fn fork_call(via: &Via, repository_id: &str) -> Option<Call> {
    match via {
        Via::Framework => None,
        Via::Application(application) => Some(Call {
            post: true,
            path: format!(
                "{}/repositories/{}/forks",
                application.rest_base,
                query_text(repository_id)
            ),
            query: Vec::new(),
            body: Some(body(&[("operationId", json!(client::operation_id()))])),
        }),
    }
}

/// A page of the server the answer named, as a link the page may show: a path on the server, or an http(s) URL of its
/// own origin; anything else (another site, `javascript:`, a user part) is no link.
pub(crate) fn page_url(server: &Server, value: &Json) -> Json {
    let Some(url) = value.as_str().map(str::trim).filter(|url| !url.is_empty()) else {
        return Json::Null;
    };
    let checked = if url.starts_with('/') && !url.starts_with("//") && !url.contains('\\') {
        server.url(url).ok()
    } else {
        client::on_server(server, url).ok()
    };
    checked.map_or(Json::Null, Json::String)
}

/// A merge request as the list shows it, from the framework's shape or an application's; its link only when it is a
/// page of `server` ([`page_url`]).
pub(crate) fn merge_request_item(item: &Json, server: &Server) -> Json {
    let text = |keys: &[&str]| -> Json {
        keys.iter()
            .map(|key| &item[*key])
            .find(|value| !value.is_null())
            .cloned()
            .unwrap_or(Json::Null)
    };
    json!({
        "mergeRequestNum": text(&["mergeRequestNum", "number"]),
        "title": text(&["title"]),
        "state": text(&["statusId", "state"]),
        "sourceRef": text(&["sourceRef"]),
        "targetRef": text(&["targetRef"]),
        "author": text(&["authorName", "authorHandle", "author"]),
        "checkState": text(&["checkState", "checks"]),
        "pushedForReview": text(&["pushedForReview"]),
        "updated": text(&["lastUpdatedDate", "updatedAt", "lastUpdated"]),
        "url": page_url(server, &text(&["url", "pageUrl", "webUrl"])),
    })
}

// ---- the hosted repository of a root (21.3) ----

/// What is kept of a root's hosted repository: its server, the remote and the repository as the server answered.
pub(crate) struct Hosted {
    pub server: Server,
    pub remote: String,
    pub repository: Json,
}

impl Hosted {
    pub(crate) fn id(&self) -> String {
        self.repository["repositoryId"].as_str().unwrap_or("").to_string()
    }

    pub(crate) fn via(&self) -> Via {
        via(&self.server, &self.repository)
    }
}

/// The user preference of a root's hosted repository as this user may use it (`<prefix>.<rootId>`): the server's base,
/// the user's rights there and the agreement as it stands for the user. The repository is the root's; the rights, the
/// agreement and the connection are each user's own.
pub const HOSTED_ACCESS: &str = "DEV_AIDE_HOSTED_ACCESS";

/// What of a resolved repository is the user's alone, kept in their preference and never on the shared root.
const PER_USER: [&str; 2] = ["access", "agreement"];

/// The root's hosted repository, as last resolved, through the current user's connection to its server, with the
/// user's own rights when they resolved it.
pub(crate) fn hosted(context: &Context, root: &Root) -> Option<Hosted> {
    let root = main_tree(context, root);
    let row = crate::roots::root_row(&context.facade, &root.path)?;
    if field(&row, "hostedServerId").is_empty() {
        return None;
    }
    let mut repository: Json = serde_json::from_str(&field(&row, "hostedJson")).ok()?;
    let base = repository["serverBase"].as_str().unwrap_or("").to_string();
    let server = Server::all(&context.facade, &current_user())
        .into_iter()
        .find(|server| server.base == base)?;
    let own: Json = moqui_core::user::get_preference(&format!("{HOSTED_ACCESS}.{}", root.id))
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Json::Null);
    for name in PER_USER {
        repository[name] = if own["base"] == base {
            own[name].clone()
        } else {
            Json::Null
        };
    }
    Some(Hosted {
        server,
        remote: field(&row, "hostedRemote"),
        repository,
    })
}

/// The working tree whose hosted repository a root has: a worktree of an agent's session (21.8) shares its main
/// tree's repository and remotes, which keep it.
fn main_tree<'a>(context: &'a Context, root: &'a Root) -> &'a Root {
    root.worktree_of
        .as_deref()
        .and_then(|main| context.root(main).ok())
        .unwrap_or(root)
}

/// The root's remotes, origin first.
fn remotes(context: &Context, root: &Root) -> Vec<(String, String)> {
    let mut remotes = crate::git::parse_remotes(&context.git().read(&root.path, &["remote", "-v"]).unwrap_or_default());
    remotes.sort_by_key(|(name, _)| name != "origin");
    remotes
}

/// Resolve the root's remotes on the connected servers: the first a server knows is kept on the root (or nothing, when
/// none is). The repository as the server answered, with its server and remote.
pub(crate) fn resolve(context: &Context, root: &Root) -> Result<Option<Json>, LibError> {
    let root = main_tree(context, root);
    let servers = Server::all(&context.facade, &current_user());
    let mut tried = Vec::new();
    for (remote, url) in remotes(context, root) {
        // the remote's user part (a token pasted into a URL) is never sent to the server
        let asked = crate::vcs::without_userinfo(&url);
        for server in servers.iter().filter(|server| server.matches_remote(&url)) {
            let answer = server.get(
                &format!("{}/repository", server.rest_path("hosting")),
                &[("url", asked.as_str())],
            );
            match answer {
                Ok(answer) if answer["found"].as_bool() == Some(true) => {
                    let mut repository = answer["repository"].clone();
                    repository["serverBase"] = json!(server.base);
                    store(context, root, Some((&server.id, &remote, &repository)))?;
                    moqui_core::user::set_preference(
                        &format!("{HOSTED_ACCESS}.{}", root.id),
                        Some(
                            json!({ "base": server.base, "access": repository["access"],
                                "agreement": repository["agreement"] })
                            .to_string()
                            .as_str(),
                        ),
                    )?;
                    return Ok(Some(
                        json!({ "serverId": server.id, "serverLabel": server.label, "remote": remote,
                        "repository": repository }),
                    ));
                }
                Ok(_) => tried.push(format!("{} has no repository at {url} you may see", server.base)),
                Err(err) => tried.push(err.to_string()),
            }
        }
    }
    if !tried.is_empty() {
        tracing::info!(
            "Dev AIDE: {} has no hosted repository: {}",
            root.label,
            tried.join("; ")
        );
    }
    moqui_core::user::set_preference(&format!("{HOSTED_ACCESS}.{}", root.id), None)?;
    // on a shared root another user's resolution stays: this user's connections, or rights, are not theirs
    if resolved_by_current_user(context, root) {
        store(context, root, None)?;
    }
    Ok(None)
}

/// Whether the root's hosted repository was resolved through one of the current user's own connections.
fn resolved_by_current_user(context: &Context, root: &Root) -> bool {
    let Some(row) = crate::roots::root_row(&context.facade, &root.path) else {
        return false;
    };
    let server_id = field(&row, "hostedServerId");
    if server_id.is_empty() {
        return false;
    }
    context
        .facade
        .find(client::SERVER)
        .and_then(|find| find.eq("serverId", Value::string(&server_id)).one())
        .ok()
        .flatten()
        .is_none_or(|server| field(&server, "userId") == current_user())
}

/// Keep (or clear) a root's hosted repository on its `DevAideRoot` row.
fn store(context: &Context, root: &Root, hosted: Option<(&str, &str, &Json)>) -> Result<(), LibError> {
    let row = crate::roots::root_row(&context.facade, &root.path);
    if row.is_none() && hosted.is_none() {
        return Ok(());
    }
    let mut row = match row {
        Some(row) => row,
        None => {
            let mut row = context.facade.make_value(crate::roots::ROOT_ENTITY)?;
            row.set_sequenced_id_primary(&context.facade)?;
            row.set("location", Value::string(root.path.display().to_string()));
            row
        }
    };
    match hosted {
        Some((server, remote, repository)) => {
            row.set("hostedServerId", Value::string(server));
            row.set("hostedRemote", Value::string(remote));
            row.set(
                "hostedRepositoryId",
                Value::string(repository["repositoryId"].as_str().unwrap_or("")),
            );
            let mut shared = repository.clone();
            if let Json::Object(map) = &mut shared {
                for name in PER_USER {
                    map.remove(name);
                }
            }
            row.set("hostedJson", Value::string(shared.to_string()));
            row.set("hostedDate", moqui_core::user::now_value());
        }
        None => {
            for name in [
                "hostedServerId",
                "hostedRemote",
                "hostedRepositoryId",
                "hostedJson",
                "hostedDate",
            ] {
                row.set(name, Value::Null);
            }
        }
    }
    row.create_or_update(&context.facade)?;
    Ok(())
}

/// After a fetch: the root's hosted repository asked again (21.3), when it has one or a connected server may know it.
/// A failure is logged; the fetch stands.
pub(crate) fn refresh_after_fetch(context: &Context, root: &Root) {
    if super::client::Server::all(&context.facade, &current_user()).is_empty() {
        return;
    }
    if let Err(err) = resolve(context, root) {
        tracing::warn!(
            "Dev AIDE: the hosted repository of {} was not asked again: {err}",
            root.label
        );
    }
}

/// `resolve#RootHosting`: the root's hosted repository, asked of the connected servers now.
fn resolve_root_hosting(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = resolve(&context, root)?;
    let mut out = Parameters::new();
    out.insert("hosted".into(), hosted.as_ref().map_or(Value::Null, json_value));
    Ok(out)
}

pub(crate) fn need_hosted(context: &Context, root: &Root) -> Result<Hosted, LibError> {
    // known, with this user's rights: else asked again, as this user
    if let Some(hosted) = hosted(context, root).filter(|hosted| !hosted.repository["access"].is_null()) {
        return Ok(hosted);
    }
    // an agent's read (the hosting tool, under the read fence) writes nothing: the Workspace resolves it
    if moqui_core::entity::fence::active() {
        return Err(LibError::new(format!(
            "{} has no hosted repository known for you yet: the Workspace finds it (its Merge requests tab, or Fetch)",
            root.label
        )));
    }
    resolve(context, root)?;
    hosted(context, root).ok_or_else(|| {
        LibError::new(format!(
            "{} has no hosted repository on a connected group server (Settings: Group servers)",
            root.label
        ))
    })
}

/// `get#MergeRequests`: the hosted repository's merge requests, by `filter` (mine, review, open, all).
fn get_merge_requests(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let filter = Some(text(parameters, "filter"))
        .filter(|filter| !filter.is_empty())
        .unwrap_or_else(|| "open".into());
    let call = list_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        &filter,
        &hosted.server.username,
    );
    let answer = call.send(&hosted.server)?;
    let items: Vec<Json> = client::first_list(&answer)
        .iter()
        .map(|item| merge_request_item(item, &hosted.server))
        .collect();
    let mut out = Parameters::new();
    out.insert("mergeRequests".into(), json_value(&items));
    out.insert("repository".into(), json_value(&hosted.repository));
    Ok(out)
}

/// `get#MergeRequest`: one merge request of the hosted repository by its number: as the list shows it, with its
/// description, head, approvals, checks and what keeps it from merging, from the framework's REST or the application's.
fn get_merge_request(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let number = text(parameters, "mergeRequestNum");
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LibError::new("a merge request's number is a whole number"));
    }
    let path = super::threads::merge_request_path(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        &number,
    );
    let answer = hosted.server.get(&path, &[])?;
    let made = client::result_of(&answer);
    // the framework answers the merge request beside its approvals and checks; an application, the merge request
    let item = if made["mergeRequest"].is_object() {
        &made["mergeRequest"]
    } else {
        made
    };
    let mut detail = merge_request_item(item, &hosted.server);
    let pick = |value: &Json, keys: &[&str]| -> Json {
        keys.iter()
            .map(|key| &value[*key])
            .find(|value| !value.is_null())
            .cloned()
            .unwrap_or(Json::Null)
    };
    detail["description"] = pick(item, &["description", "body"]);
    detail["headCommitId"] = pick(item, &["headCommitId"]);
    detail["approvalCount"] = pick(made, &["approvalCount", "approvals"]);
    if detail["checkState"].is_null() {
        detail["checkState"] = pick(made, &["checkState"]);
    }
    detail["missing"] = pick(made, &["missingList", "missing"]);
    let mut out = Parameters::new();
    out.insert("mergeRequest".into(), json_value(&detail));
    Ok(out)
}

/// The REST path of an issue (a work item) on a group server, and whether it names the issue by its number: the offer's
/// `tasks.issuePath` with `{repositoryId}` and `{id}` (Mantle's `gitRepositories/{repositoryId}/issues/{id}`), else
/// Mantle's work effort (`/rest/s1/mantle/workEfforts/{id}`). A key `#12` is the repository's issue 12, asked with
/// `byNumber`; any other is the work item's id. `None` for an id that is not one.
#[must_use]
pub fn issue_path(template: &str, repository_id: &str, id: &str) -> Option<(String, bool)> {
    let by_number = id.trim().starts_with('#');
    let id = id.trim().trim_start_matches('#');
    if id.is_empty()
        || id.len() > 60
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return None;
    }
    if by_number && !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let template = Some(template)
        .filter(|template| template.starts_with('/') && !template.starts_with("//") && template.contains("{id}"))
        .unwrap_or("/rest/s1/mantle/workEfforts/{id}");
    Some((
        template
            .replace("{repositoryId}", &query_text(repository_id))
            .replace("{id}", &query_text(id)),
        by_number,
    ))
}

/// `get#Issue`: an issue (a work item) of the hosted repository's server by its id: its title, status, type and text.
fn get_issue(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let id = text(parameters, "issueId");
    let (path, by_number) = issue_path(&hosted.server.offered("tasks.issuePath"), &hosted.id(), &id)
        .ok_or_else(|| LibError::new(format!("{id} is not an issue's id")))?;
    let answer = hosted
        .server
        .get(&path, &[("byNumber", if by_number { "true" } else { "" })])?;
    // Mantle's get#RepositoryIssue answers it as issue; a work effort's REST, as itself
    let item = client::result_of(&answer);
    let item = if item["issue"].is_object() {
        &item["issue"]
    } else {
        item
    };
    let pick = |keys: &[&str]| -> Json {
        keys.iter()
            .map(|key| &item[*key])
            .find(|value| !value.is_null())
            .cloned()
            .unwrap_or(Json::Null)
    };
    let issue = json!({
        "issueId": pick(&["workEffortId", "issueId", "number", "id"]),
        "number": pick(&["issueNumber", "number"]),
        "title": pick(&["workEffortName", "title", "name"]),
        "status": pick(&["statusId", "state", "status"]),
        "type": pick(&["workEffortTypeEnumId", "type"]),
        "description": pick(&["description", "body", "text"]),
        "url": page_url(&hosted.server, &pick(&["url", "pageUrl"])),
    });
    let mut out = Parameters::new();
    out.insert("issue".into(), json_value(&issue));
    Ok(out)
}

/// The remote a branch is pushed to by its own config: `branch.<name>.pushRemote`, else `branch.<name>.remote`; empty
/// when it has none.
fn branch_remote(context: &Context, root: &Root, branch: &str) -> String {
    ["pushRemote", "remote"]
        .iter()
        .map(|key| {
            context
                .git()
                .read(&root.path, &["config", "--get", &format!("branch.{branch}.{key}")])
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .find(|remote| !remote.is_empty())
        .unwrap_or_default()
}

/// The repository of a merge request's source branch when it is on another remote than the hosted repository's (a
/// fork, 21.4): that remote's URL resolved on the same server. Empty for the hosted repository itself; an error for a
/// remote the server has no repository at, since the server would look for the branch in the wrong place.
fn source_repository(context: &Context, root: &Root, hosted: &Hosted, opening: &Opening) -> Result<String, LibError> {
    let remote = Some(opening.source_remote.trim().to_string())
        .filter(|remote| !remote.is_empty())
        .unwrap_or_else(|| branch_remote(context, root, &opening.source));
    if remote.is_empty() || remote == hosted.remote {
        return Ok(String::new());
    }
    let url = remotes(context, root)
        .into_iter()
        .find(|(name, _)| *name == remote)
        .map(|(_, url)| url)
        .ok_or_else(|| LibError::new(format!("{} has no remote {remote}", root.label)))?;
    if !hosted.server.matches_remote(&url) {
        return Err(LibError::new(format!(
            "{} is pushed to {remote}, which is not on {}: push it there to open a merge request",
            opening.source, hosted.server.base
        )));
    }
    let asked = crate::vcs::without_userinfo(&url);
    let answer = hosted.server.get(
        &format!("{}/repository", hosted.server.rest_path("hosting")),
        &[("url", asked.as_str())],
    )?;
    let id = answer["repository"]["repositoryId"].as_str().unwrap_or("");
    if answer["found"].as_bool() != Some(true) || id.is_empty() {
        return Err(LibError::new(format!(
            "{} has no repository at {remote} ({asked}) you may see",
            hosted.server.base
        )));
    }
    Ok(if id == hosted.id() {
        String::new()
    } else {
        id.to_string()
    })
}

/// Open a merge request on the hosted repository of `root`: the framework's, or the application's that keeps it. A
/// source branch pushed to a fork's remote comes from that fork ([`source_repository`]).
pub(crate) fn open(context: &Context, root: &Root, opening: &Opening) -> Result<Json, LibError> {
    let hosted = need_hosted(context, root)?;
    let mut opening = opening.clone();
    if opening.source_repository.is_empty() {
        opening.source_repository = source_repository(context, root, &hosted, &opening)?;
    }
    if opening.target.is_empty() {
        opening.target = hosted.repository["defaultBranch"]
            .as_str()
            .unwrap_or("main")
            .to_string();
    }
    if opening.source == opening.target && opening.source_repository.is_empty() {
        return Err(LibError::new(format!(
            "{} is the target: push a branch of its own to open a merge request",
            opening.source
        )));
    }
    let call = create_call(
        &hosted.via(),
        &hosted.server.rest_path("repositories"),
        &hosted.id(),
        &opening,
    );
    let answer = call.send(&hosted.server)?;
    let made = client::result_of(&answer);
    audit_git(
        context,
        root,
        "merge-request",
        json!({ "server": hosted.server.base, "repository": hosted.id(), "source": opening.source, "target": opening.target,
            "sourceRepository": opening.source_repository }),
    );
    Ok(json!({
        "mergeRequestNum": made["mergeRequestNum"], "repositoryId": hosted.id(), "target": opening.target,
        "sourceRepositoryId": if opening.source_repository.is_empty() { Json::Null } else { json!(opening.source_repository) },
        "url": page_url(&hosted.server, &made["url"]),
    }))
}

/// The summary of HEAD's commit, a merge request's default title.
fn head_summary(context: &Context, root: &Root) -> String {
    context
        .git()
        .read(&root.path, &["log", "-1", "--format=%s"])
        .map(|out| out.trim().to_string())
        .unwrap_or_default()
}

/// `open#MergeRequest`: a merge request of a pushed branch (the current one by default) into the target (the hosted
/// repository's default branch by default), titled by the head commit unless given.
fn open_merge_request(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_write_root(&context, parameters, "opens merge requests")?;
    let source = match text(parameters, "sourceBranch") {
        branch if branch.is_empty() => crate::vcs::current_branch(&context, root)?,
        branch => branch,
    };
    let title = Some(text(parameters, "title"))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| head_summary(&context, root));
    let made = open(
        &context,
        root,
        &Opening {
            source,
            target: text(parameters, "targetBranch"),
            title,
            description: text(parameters, "description"),
            source_repository: text(parameters, "sourceRepositoryId"),
            source_remote: text(parameters, "sourceRemote"),
        },
    )?;
    let mut out = Parameters::new();
    out.insert("mergeRequest".into(), json_value(&made));
    Ok(out)
}

/// The push of a proposal (Dogfood F5): HEAD to `refs/for/<target>`, with the topic and title as push options.
#[must_use]
pub fn propose_args(remote: &str, target: &str, topic: &str, title: &str) -> Vec<String> {
    let mut args = vec!["push".to_string()];
    if !topic.trim().is_empty() {
        args.push("-o".into());
        args.push(format!("topic={}", topic.trim()));
    }
    if !title.trim().is_empty() {
        args.push("-o".into());
        args.push(format!("title={}", title.trim().replace('\n', " ")));
    }
    args.push("--".into());
    args.push(remote.to_string());
    args.push(format!("HEAD:refs/for/{target}"));
    args
}

/// `propose#Changes` (push-to-review): for a user who may view the hosted repository and not push to it, HEAD's
/// commits pushed to `refs/for/<target>`; the server opens a merge request (or updates the user's of the same topic).
fn propose_changes(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_write_root(&context, parameters, "proposes changes")?;
    network_allowed(&context)?;
    let hosted = need_hosted(&context, root)?;
    if hosted.repository["mergeRequests"]["pushToReview"].as_bool() == Some(false) {
        return Err(LibError::new(format!(
            "{} takes no push for review",
            hosted.server.base
        )));
    }
    let target = Some(text(parameters, "targetBranch"))
        .filter(|target| !target.is_empty())
        .unwrap_or_else(|| {
            hosted.repository["defaultBranch"]
                .as_str()
                .unwrap_or("main")
                .to_string()
        });
    crate::vcs::valid_branch(&target)?;
    let topic = text(parameters, "topic");
    if !topic.is_empty() {
        crate::vcs::valid_branch(&topic)?;
    }
    let title = Some(text(parameters, "title"))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| head_summary(&context, root));
    let args = propose_args(&hosted.remote, &target, &topic, &title);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = run_network_with(&context, root, &argv, Some(&text(parameters, "operationId")))?;
    audit_git(
        &context,
        root,
        "propose",
        json!({ "ok": output.success(), "target": target, "topic": topic }),
    );
    let shown = format!("{}{}", output.stdout, output.stderr);
    if !output.success() {
        return Err(LibError::new(format!("git push for review: {}", shown.trim())));
    }
    let mut out = Parameters::new();
    out.insert("output".into(), Value::string(shown));
    out.insert("target".into(), Value::string(target));
    Ok(out)
}

/// The local branch a merge request is checked out to: the one named, else `mr/<n>`.
fn checkout_branch(parameters: &Parameters, number: &str) -> String {
    Some(text(parameters, "branch"))
        .filter(|branch| !branch.trim().is_empty())
        .unwrap_or_else(|| format!("mr/{number}"))
}

/// What checking out a merge request means here: its code runs in this instance.
pub const CHECKOUT_WARNING: &str = "This is someone else's code: the file watcher reloads it into this instance as it \
    is checked out, so its services, screens and scripts run here with your rights. Read the changes first.";

/// `checkout#MergeRequest`: `refs/merge-requests/<n>/head` of the hosted remote fetched into a local branch (`mr/<n>`
/// by default) and switched to. A branch is never overwritten by force: a new branch is made from the head; the
/// merge request's own `mr/<n>` takes a new head only when it has no commits of its own that the head lacks; any other
/// branch that exists is refused. The answer warns that the code now runs here (hot reload).
fn checkout_merge_request(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_write_root(&context, parameters, "checks out merge requests")?;
    network_allowed(&context)?;
    let hosted = need_hosted(&context, root)?;
    let number = text(parameters, "mergeRequestNum");
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(LibError::new("a merge request's number is a whole number"));
    }
    let branch = checkout_branch(parameters, &number);
    crate::vcs::valid_branch(&branch)?;
    let own = format!("mr/{number}");
    let source = format!("refs/merge-requests/{number}/head");
    let local = format!("refs/heads/{branch}");
    let exists = context
        .git()
        .read(&root.path, &["rev-parse", "-q", "--verify", &local])
        .is_ok_and(|out| !out.trim().is_empty());
    if exists && branch != own {
        return Err(LibError::new(format!(
            "{branch} is a branch already: check the merge request out as {own}, or name a new branch"
        )));
    }
    let current = crate::vcs::current_branch(&context, root).unwrap_or_default();
    let mut shown = String::new();
    let fetched =
        run_network_with(&context, root, &["fetch", "--", &hosted.remote, &source], None)?.checked("fetch")?;
    shown.push_str(&fetched.stderr);
    if exists {
        let ahead = context
            .git()
            .read(&root.path, &["rev-list", "--count", &format!("FETCH_HEAD..{local}")])
            .map(|out| out.trim().to_string())
            .unwrap_or_default();
        if ahead != "0" {
            return Err(LibError::new(format!(
                "{branch} has {} commit(s) of your own that merge request !{number} does not: push or move them, then \
                 check out again",
                if ahead.is_empty() { "unknown".to_string() } else { ahead }
            )));
        }
        let moved = if current == branch {
            // the working tree's own changes are kept; a change that the new head touches stops it
            context
                .git()
                .run(&root.path, Mode::Write, &["reset", "--keep", "FETCH_HEAD"], &[], None)?
                .checked("reset --keep")?
        } else {
            context
                .git()
                .run(
                    &root.path,
                    Mode::Write,
                    &["update-ref", &local, "FETCH_HEAD"],
                    &[],
                    None,
                )?
                .checked("update-ref")?
        };
        shown.push_str(&moved.stdout);
    } else {
        let made = context
            .git()
            .run(&root.path, Mode::Write, &["branch", &branch, "FETCH_HEAD"], &[], None)?
            .checked("branch")?;
        shown.push_str(&made.stderr);
    }
    if current != branch {
        let switched = context
            .git()
            .run(&root.path, Mode::Write, &["switch", &branch], &[], None)?
            .checked("switch")?;
        shown.push_str(&switched.stderr);
    }
    audit_git(
        &context,
        root,
        "checkout-merge-request",
        json!({ "number": number, "branch": branch }),
    );
    let mut out = Parameters::new();
    out.insert("branch".into(), Value::string(branch));
    out.insert("output".into(), Value::string(shown));
    out.insert("warning".into(), Value::string(CHECKOUT_WARNING));
    // the file watcher reloads what changed; a changed entity or conf file still takes a restart
    out.insert("restart".into(), Value::Bool(true));
    Ok(out)
}

/// `fork#Repository`: the user's fork of the hosted repository, made by the server where it offers forks, added as the
/// remote `fork` (its HTTPS URL from the offer's template), to push to and open merge requests from.
fn fork_repository(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    let _authz = ec.disable_authz();
    let root = git_write_root(&context, parameters, "forks repositories")?;
    let hosted = need_hosted(&context, root)?;
    let Some(call) = fork_call(&hosted.via(), &hosted.id()) else {
        return Err(LibError::new(format!(
            "{} makes no forks of {} through its REST: ask for a branch, or propose changes for review",
            hosted.server.base,
            hosted.id()
        )));
    };
    let answer = call.send(&hosted.server)?;
    let made = client::result_of(&answer);
    let fork = if made["repository"].is_object() {
        &made["repository"]
    } else {
        made
    };
    let path = fork["repositoryPath"].as_str().unwrap_or("").to_string();
    let url = fork["cloneUrl"]
        .as_str()
        .map(str::to_string)
        .or_else(|| fork["cloneUrls"]["httpsUrl"].as_str().map(str::to_string))
        .unwrap_or_else(|| hosted.server.offered("git.httpsUrlTemplate").replace("${name}", &path));
    if path.is_empty() && url.contains("${name}") {
        return Err(LibError::new(format!(
            "{} answered no path of the fork",
            hosted.server.base
        )));
    }
    // the fork's URL is the server's own, https, with no user part: git is given nothing else
    let url = client::on_server(&hosted.server, &url)?;
    let has_fork = remotes(&context, root).iter().any(|(name, _)| name == "fork");
    let args = if has_fork {
        vec!["remote", "set-url", "--", "fork", url.as_str()]
    } else {
        vec!["remote", "add", "--", "fork", url.as_str()]
    };
    context
        .git()
        .run(&root.path, Mode::Write, &args, &[], None)?
        .checked("remote")?;
    audit_git(
        &context,
        root,
        "fork",
        json!({ "repository": hosted.id(), "fork": path }),
    );
    let mut out = Parameters::new();
    out.insert("remote".into(), Value::string("fork"));
    out.insert("url".into(), Value::string(url));
    out.insert("repositoryPath".into(), Value::string(path));
    out.insert(
        "repositoryId".into(),
        fork["repositoryId"].as_str().map_or(Value::Null, Value::string),
    );
    Ok(out)
}
