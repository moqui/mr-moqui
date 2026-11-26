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

//! `org.moqui.git.GitRestServices`, the framework's REST for hosting (`/rest/s1/moqui/git` and `/rest/s1/moqui/hosting`
//! in `tools/service/moqui.rest.xml`; plans/Dogfood.md F9, plans/design-dev-aide.md F18): what the server offers its
//! clients, the repository behind a clone or page URL with the caller's rights and settings, whether an author signed a
//! repository's agreement, and the merge request, review and pipeline resources over the framework's services, by the
//! repository's own numbers. Each service under them checks the repository's rights as without REST. Before each write
//! on a repository the application that keeps it is asked (`git-server rest-write-check-service`, interface
//! `check#GitRestWrite`): a repository Smithy keeps is changed only through Smithy's own REST, which the refusal names.
//! Guide: `docs/interfaces/Git-Hosting.md`, REST.

use std::collections::BTreeMap;

use base64::Engine;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::hosting::{may, none, Out, Parameters};
use super::{add_error, out, text};
use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::rust_service;

use crate::values::{map, opt_text};
use crate::{rows_guard, GIT_REPOSITORY};

const GIT: &str = "org.moqui.git.GitServices.";
const REVIEW: &str = "org.moqui.git.GitReviewServices.";
const CI: &str = "org.moqui.git.GitCiServices.";
const MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";
const JOB_RUN: &str = "moqui.resource.GitJobRun";
const PIPELINE_RUN: &str = "moqui.resource.GitPipelineRun";

/// Where the framework's REST is, as the offer names it.
const REST_BASE: &str = "/rest/s1/moqui";

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_hosting_offer)?;
    rust_service!(map, resolve_repository_url)?;
    rust_service!(map, get_repository)?;
    rust_service!(map, get_author_signed)?;
    rust_service!(map, verify_write)?;
    rust_service!(map, get_blob)?;
    rust_service!(map, create_ref)?;
    rust_service!(map, delete_ref)?;
    rust_service!(map, create_commit_status)?;
    rust_service!(map, find_merge_requests)?;
    rust_service!(map, create_merge_request)?;
    rust_service!(map, get_merge_request)?;
    rust_service!(map, update_merge_request)?;
    rust_service!(map, close_merge_request)?;
    rust_service!(map, reopen_merge_request)?;
    rust_service!(map, set_merge_request_reviewers)?;
    rust_service!(map, submit_merge_request_review)?;
    rust_service!(map, get_merge_request_diff)?;
    rust_service!(map, get_merge_request_approvals)?;
    rust_service!(map, merge_merge_request)?;
    rust_service!(map, find_review_threads)?;
    rust_service!(map, create_review_comment)?;
    rust_service!(map, resolve_review_thread)?;
    rust_service!(map, run_pipeline)?;
    rust_service!(map, rerun_pipeline_run)?;
    rust_service!(map, cancel_pipeline_run)?;
    rust_service!(map, approve_pipeline_run)?;
    rust_service!(map, retry_job_run)?;
    rust_service!(map, get_run_artifact)?;
    Ok(())
}

// ---- the application that keeps a repository ----

/// Call the service the conf's `git-server` attribute `conf_name` names, with authorization disabled; `None` when it
/// names none. Its errors are its answer, not left in the message facade.
fn call_hook(
    conf_name: &str,
    parameters: BTreeMap<String, Value>,
) -> Option<Result<BTreeMap<String, Value>, moqui_core::context::Error>> {
    let name = crate::setting(conf_name);
    if name.is_empty() {
        return None;
    }
    let options = moqui_core::service::call::CallOptions {
        disable_authz: true,
        ..moqui_core::service::call::CallOptions::default()
    };
    Some(crate::ci::with_errors_taken(|| {
        moqui_core::service::call::sync_checked(&name, &parameters, &options)
    }))
}

/// What the application that keeps a repository says of an operation on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Keeper {
    pub allowed: bool,
    pub message: String,
    /// The application's name (`smithy`); empty when none keeps it.
    pub application: String,
    /// Its own REST for the repository.
    pub rest_url: String,
}

/// Ask `git-server rest-write-check-service` about `operation` on `repository_id`, with authorization off. With no
/// service, allowed; with one, only an explicit `allowed` true (or `Y`) allows: an answer that leaves it empty refuses,
/// as a service that fails does, with its errors as the message.
pub(crate) fn keeper(repository_id: &str, operation: &str, merge_request_id: Option<&str>) -> Keeper {
    let user = moqui_core::user::current_user_id().ok().flatten();
    let parameters = crate::ci::parameters([
        ("repositoryId", Value::string(repository_id)),
        ("operation", Value::string(operation)),
        ("userId", user.map_or(Value::Null, Value::string)),
        ("mergeRequestId", merge_request_id.map_or(Value::Null, Value::string)),
    ]);
    match call_hook("rest-write-check-service", parameters) {
        None => Keeper {
            allowed: true,
            ..Keeper::default()
        },
        Some(Ok(answer)) => {
            let get = |key: &str| {
                answer
                    .get(key)
                    .filter(|value| !is_empty(value))
                    .map(to_plain_string)
                    .unwrap_or_default()
            };
            Keeper {
                allowed: matches!(get("allowed").as_str(), "true" | "Y"),
                message: get("message"),
                application: get("application"),
                rest_url: get("restUrl"),
            }
        }
        Some(Err(err)) => Keeper {
            allowed: false,
            message: format!("the application's check failed: {}", err.message()),
            ..Keeper::default()
        },
    }
}

/// The refusal of a write the application keeps to itself: `GitKeptByApplication`, its message and where its REST is.
fn kept_message(repository_id: &str, keeper: &Keeper) -> String {
    let message = if keeper.message.is_empty() {
        let by = if keeper.application.is_empty() {
            "an application"
        } else {
            keeper.application.as_str()
        };
        format!("repository {repository_id} is kept by {by}, which takes this change through its own REST")
    } else {
        keeper.message.clone()
    };
    if keeper.rest_url.is_empty() {
        format!("GitKeptByApplication: {message}")
    } else {
        format!("GitKeptByApplication: {message} (use {})", keeper.rest_url)
    }
}

/// Whether a write may go through the framework's REST; the refusal recorded when not.
fn may_write(repository_id: &str, operation: &str, merge_request_id: Option<&str>) -> bool {
    let keeper = keeper(repository_id, operation, merge_request_id);
    if !keeper.allowed {
        add_error(kept_message(repository_id, &keeper));
    }
    keeper.allowed
}

/// `verify#Write`: what the application that keeps `repositoryId` says of `operation`, for a client that asks before
/// it offers a control. With `refuse`, a refusal is also the call's error. Out: `allowed`, `message`, `application`,
/// `restUrl`.
fn verify_write(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let (Some(repository_id), Some(operation)) = (text(parameters, "repositoryId"), text(parameters, "operation"))
    else {
        add_error("repositoryId and operation are required");
        return none();
    };
    if !may(&repository_id, "view") {
        add_error(format!("Repository {repository_id} not found"));
        return none();
    }
    let keeper = keeper(&repository_id, &operation, None);
    if !keeper.allowed && super::flag(parameters, "refuse") {
        add_error(kept_message(&repository_id, &keeper));
    }
    Ok(out([
        ("allowed", Value::Bool(keeper.allowed)),
        ("message", opt_text(&keeper.message)),
        ("application", opt_text(&keeper.application)),
        ("restUrl", opt_text(&keeper.rest_url)),
    ]))
}

// ---- what the server offers (F18) ----

/// A conf attribute by its selector (it has an `@`), else a property.
fn property(name: &str) -> String {
    moqui_core::context::conf_env::setting_or_property(name)
        .trim()
        .to_string()
}

fn conf(path: &str, name: &str) -> String {
    ExecutionContext::with_current(|ec| ec.factory().conf_setting(path, name)).unwrap_or_default()
}

/// `get#HostingOffer`: what this server offers a client that connects to it (a dev instance's Dev AIDE, a tool), for
/// a caller with or without credentials: where git, the REST and the sign-in are, push to review, the LLM gateway, the
/// agreement check and, from `git-server offer-service`, what an application adds (its REST, its task source, its
/// forks). `user` says whether the caller's credentials were good. Out: `offer`.
fn get_hosting_offer(_parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let base = crate::server::public_base_url();
    let user_id = moqui_core::user::current_user_id().ok().flatten();
    let username = moqui_core::user::current_username().ok().flatten();
    let ssh = super::ssh_host_port();
    // the SSH user is the account, as in get#GitCloneUrls
    let ssh_user = username.clone().unwrap_or_else(|| "username".to_string());
    let ssh_template = ssh.as_ref().map(|(host, port)| {
        if port == "22" {
            format!("{ssh_user}@{host}:${{name}}.git")
        } else {
            format!("ssh://{ssh_user}@{host}:{port}/${{name}}.git")
        }
    });
    let gateway = conf("llm-facade", "enabled") != "false" && conf("llm-facade/gateway", "enabled") == "true";
    // a login key needs an LlmApiClient naming it unless require-client is off (docs/ai/Gateway.md)
    let require_client = conf("llm-facade/gateway", "require-client") != "false";
    let loopback = property("webapp-list/webapp[webroot]@app-loopback-redirect") != "false";
    let agreement_service = !crate::setting("agreement-check-service").is_empty();
    let offer = map([
        ("baseUrl", Value::string(&base)),
        (
            "user",
            map([
                ("signedIn", Value::Bool(user_id.is_some())),
                ("username", username.map_or(Value::Null, Value::string)),
            ]),
        ),
        (
            "git",
            map([
                ("httpsBase", Value::string(format!("{base}/git/"))),
                ("httpsUrlTemplate", Value::string(format!("{base}/git/${{name}}.git"))),
                (
                    "sshHost",
                    ssh.as_ref().map_or(Value::Null, |(host, _)| Value::string(host)),
                ),
                (
                    "sshPort",
                    ssh.as_ref().map_or(Value::Null, |(_, port)| Value::string(port)),
                ),
                ("sshUrlTemplate", ssh_template.map_or(Value::Null, Value::string)),
                ("pushToReview", Value::Bool(crate::setting("review-push") != "false")),
                (
                    "mergeRequestRefs",
                    Value::string("refs/merge-requests/{mergeRequestNum}/head"),
                ),
            ]),
        ),
        (
            "rest",
            map([
                ("base", Value::string(REST_BASE)),
                ("repositories", Value::string(format!("{REST_BASE}/git"))),
                ("hosting", Value::string(format!("{REST_BASE}/hosting"))),
                ("openapi", Value::string("/rest/service.swagger/moqui.json")),
                // the merge requests' review threads, where an application keeps them (review-thread-service)
                (
                    "reviewThreads",
                    Value::Bool(!crate::setting("review-thread-service").is_empty()),
                ),
            ]),
        ),
        (
            "signIn",
            map([
                ("appLogin", Value::string("/sso/appLogin")),
                ("appToken", Value::string("/sso/appToken")),
                ("appProviders", Value::string("/sso/appProviders")),
                ("loopbackRedirect", Value::Bool(loopback)),
                ("keyRevoke", Value::string(format!("{REST_BASE}/hosting/keyRevoke"))),
            ]),
        ),
        (
            "gateway",
            map([
                ("enabled", Value::Bool(gateway)),
                (
                    "url",
                    if gateway {
                        Value::string(format!("{base}/llm"))
                    } else {
                        Value::Null
                    },
                ),
                // a key for the gateway alone, so a client never writes its account's key where a model client reads it
                ("keyPath", Value::string(format!("{REST_BASE}/hosting/gatewayKey"))),
                ("requireClient", Value::Bool(require_client)),
            ]),
        ),
        (
            "agreement",
            map([
                ("path", Value::string(format!("{REST_BASE}/hosting/agreement"))),
                ("decided", Value::Bool(agreement_service)),
            ]),
        ),
        ("forks", Value::Bool(false)),
        // a repository's pages for a person (a file at a line, a commit, a branch), as templates: the framework has
        // none to offer; an application that shows repositories offers its own (application.pages for those it keeps,
        // pages for this hosting's, as IronStone Projects' Code does)
        ("pages", Value::Null),
    ]);
    let parameters = crate::ci::parameters([
        ("offer", offer.clone()),
        ("userId", user_id.map_or(Value::Null, Value::string)),
    ]);
    let offer = match call_hook("offer-service", parameters) {
        None => offer,
        Some(Ok(answer)) => match answer.get("offer") {
            Some(added @ Value::Map(_)) => added.clone(),
            _ => offer,
        },
        // one hook that fails leaves the framework's offer, which still serves git and its REST
        Some(Err(err)) => {
            tracing::error!("the git-server offer-service failed: {err}");
            offer
        }
    };
    Ok(out([("offer", offer)]))
}

// ---- the repository behind a URL (F18) ----

/// The repository path a clone or page URL names, and the shorter paths to try after it (a page under a mount):
/// `https://host/git/a/b.git`, `https://host/a/b/-/tree/main`, `ssh://user@host:2222/a/b.git` and `user@host:a/b.git`.
pub(crate) fn url_paths(url: &str) -> Vec<String> {
    let url = url.trim();
    let path = if let Some((_, rest)) = url.split_once("://") {
        rest.split_once('/').map_or("", |(_, path)| path)
    } else if let Some((authority, path)) = url.split_once(':') {
        // scp-like user@host:path; a Windows drive or a bare word is no URL
        if authority.contains('/') || authority.len() < 2 {
            return Vec::new();
        }
        path
    } else {
        return Vec::new();
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    // a page's section, and what a smart HTTP request adds
    let path = path.split_once("/-/").map_or(path, |(repository, _)| repository);
    let path = crate::server::path::split_request(path).map_or(path, |(repository, _)| repository);
    let path = path.trim_matches('/');
    let mut paths = Vec::new();
    let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
    for start in 0..segments.len() {
        if let Some(found) = crate::server::path::repository_of(&segments[start..].join("/")) {
            if !paths.contains(&found) {
                paths.push(found);
            }
        }
    }
    paths
}

/// What a client asks of a repository it holds a working tree of, for a caller who may view it: its id, path,
/// description and default branch, the caller's rights, whether it is archived or a fork, its clone URLs, its merge
/// request settings (the default branch's rules, push to review), who keeps it (an application and its REST), and the
/// agreement as it stands for the caller. `None` when the caller may not view it.
fn repository_info(id: &str) -> Option<Value> {
    if !may(id, "view") {
        return None;
    }
    let entity = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()?;
    let row = {
        let _rows = rows_guard();
        entity
            .find(GIT_REPOSITORY)
            .ok()?
            .eq("repositoryId", Value::string(id))
            .one()
            .ok()
            .flatten()?
    };
    let field = |name: &str| {
        row.get(name)
            .filter(|value| !is_empty(value))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let branch = Some(field("branch"))
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| "main".into());
    let repository = crate::ci::parameters([("repositoryId", Value::string(id))]);
    let clone = moqui_core::service::sync(&format!("{GIT}get#GitCloneUrls"), &repository).unwrap_or_default();
    let mut ruled = repository.clone();
    ruled.insert("refName".into(), Value::string(format!("refs/heads/{branch}")));
    let rules = moqui_core::service::sync(&format!("{GIT}get#GitBranchRules"), &ruled).unwrap_or_default();
    let rule = |name: &str| rules.get(name).cloned().unwrap_or(Value::Null);
    let keeper = keeper(id, "read", None);
    let agreement = author_signed(id, moqui_core::user::current_user_id().ok().flatten().as_deref(), None);
    let fork_of = field("forkOfRepositoryId");
    Some(map([
        ("repositoryId", Value::string(id)),
        ("repositoryPath", Value::string(crate::server::path::path_of(id))),
        ("description", opt_text(&field("description"))),
        ("repositoryKind", opt_text(&field("repositoryKind"))),
        ("defaultBranch", Value::string(&branch)),
        ("headCommitId", opt_text(&field("headCommitId"))),
        ("archived", Value::Bool(!field("archivedDate").is_empty())),
        (
            "forkOf",
            if fork_of.is_empty() || !may(&fork_of, "view") {
                Value::Null
            } else {
                map([
                    ("repositoryId", Value::string(&fork_of)),
                    ("repositoryPath", Value::string(crate::server::path::path_of(&fork_of))),
                ])
            },
        ),
        (
            "access",
            map([
                ("canView", Value::Bool(true)),
                ("canCreate", Value::Bool(may(id, "create"))),
                ("canUpdate", Value::Bool(may(id, "update"))),
                ("canDelete", Value::Bool(may(id, "delete"))),
            ]),
        ),
        (
            "cloneUrls",
            map([
                ("httpsUrl", clone.get("httpsUrl").cloned().unwrap_or(Value::Null)),
                ("sshUrl", clone.get("sshUrl").cloned().unwrap_or(Value::Null)),
            ]),
        ),
        (
            "mergeRequests",
            map([
                ("defaultMergeMethodEnumId", opt_text(&field("defaultMergeMethodEnumId"))),
                ("pushToReview", Value::Bool(crate::setting("review-push") != "false")),
                ("protected", rule("protected")),
                ("requireMergeRequest", rule("requireMergeRequest")),
                ("requiredApprovals", rule("requiredApprovals")),
                ("requiredStatusContexts", rule("requiredStatusContexts")),
                ("allowedMergeMethods", rule("allowedMergeMethods")),
                ("mayPushDirectly", rule("mayPushDirectly")),
            ]),
        ),
        (
            "keptBy",
            if keeper.application.is_empty() && keeper.rest_url.is_empty() {
                Value::Null
            } else {
                map([
                    ("application", opt_text(&keeper.application)),
                    ("restUrl", opt_text(&keeper.rest_url)),
                ])
            },
        ),
        ("agreement", agreement),
    ]))
}

/// `resolve#RepositoryUrl`: the repository a clone URL (HTTPS or SSH) or a page URL of this server names, as
/// [`repository_info`] gives it. Out: `found`, and `repository` when found and the caller may view it.
fn resolve_repository_url(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(url) = text(parameters, "url") else {
        add_error("url is required");
        return none();
    };
    let found = url_paths(&url)
        .iter()
        .find_map(|path| crate::server::path::resolve(path))
        .and_then(|id| repository_info(&id));
    Ok(out([
        ("found", Value::Bool(found.is_some())),
        ("repository", found.unwrap_or(Value::Null)),
    ]))
}

/// `get#Repository`: [`repository_info`] of `repositoryId`. Out: `repository`.
fn get_repository(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    match repository_info(&id) {
        Some(repository) => Ok(out([("repository", repository)])),
        None => {
            add_error(format!("Repository {id} not found"));
            none()
        }
    }
}

// ---- the contributor agreement (F18) ----

/// Whether an author signed repository `id`'s agreement, from `git-server agreement-check-service`: `known` false
/// when no service decides it here, with the keeping application and its REST, where a client asks next.
fn author_signed(id: &str, user_id: Option<&str>, email: Option<&str>) -> Value {
    let parameters = crate::ci::parameters([
        ("repositoryId", Value::string(id)),
        ("authorUserId", user_id.map_or(Value::Null, Value::string)),
        ("authorEmail", email.map_or(Value::Null, Value::string)),
    ]);
    let answered = call_hook("agreement-check-service", parameters).and_then(|answer| match answer {
        Ok(answer) => Some(answer),
        Err(err) => {
            tracing::error!(repository = id, "the git-server agreement-check-service failed: {err}");
            None
        }
    });
    let get = |answer: &BTreeMap<String, Value>, key: &str| answer.get(key).filter(|value| !is_empty(value)).cloned();
    let flag = |answer: &BTreeMap<String, Value>, key: &str| {
        get(answer, key).is_some_and(|value| matches!(to_plain_string(&value).as_str(), "true" | "Y"))
    };
    match answered {
        Some(answer) if flag(&answer, "known") => map([
            ("known", Value::Bool(true)),
            ("required", Value::Bool(flag(&answer, "required"))),
            ("signed", Value::Bool(flag(&answer, "signed"))),
            ("message", get(&answer, "message").unwrap_or(Value::Null)),
            ("signUrl", get(&answer, "signUrl").unwrap_or(Value::Null)),
        ]),
        _ => {
            let keeper = keeper(id, "read", None);
            map([
                ("known", Value::Bool(false)),
                ("application", opt_text(&keeper.application)),
                ("applicationRestUrl", opt_text(&keeper.rest_url)),
            ])
        }
    }
}

/// `get#AuthorSigned`: [`author_signed`] for a viewer of the repository, of `authorUserId` or `authorEmail` (the caller
/// when both are empty). Out: `known`, `required`, `signed`, `message`, `signUrl`, `application`,
/// `applicationRestUrl`.
fn get_author_signed(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&id, "view") {
        add_error(format!("Repository {id} not found"));
        return none();
    }
    let email = text(parameters, "authorEmail");
    let user = text(parameters, "authorUserId").or_else(|| {
        email
            .is_none()
            .then(|| moqui_core::user::current_user_id().ok().flatten())
            .flatten()
    });
    match author_signed(&id, user.as_deref(), email.as_deref()) {
        Value::Map(entries) => Ok(entries.into_iter().collect()),
        _ => none(),
    }
}

// ---- reads that need a shape of their own ----

/// `bytes` of an answer as base64 text in `base64`, as JSON has no bytes.
fn bytes_as_base64(mut answer: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    if let Some(Value::Bytes(bytes)) = answer.remove("bytes") {
        answer.insert(
            "base64".into(),
            Value::string(base64::engine::general_purpose::STANDARD.encode(bytes)),
        );
    }
    answer
}

/// `get#Blob`: `get#GitBlob`, a file that is not text as `base64`.
fn get_blob(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    Ok(bytes_as_base64(moqui_core::service::sync(
        &format!("{GIT}get#GitBlob"),
        parameters,
    )?))
}

/// `get#RunArtifact`: `get#RunArtifact` of the engine, its piece as `base64`.
fn get_run_artifact(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    Ok(bytes_as_base64(moqui_core::service::sync(
        &format!("{CI}get#RunArtifact"),
        parameters,
    )?))
}

// ---- writes on a repository ----

/// Call `name` with `parameters` once the keeper allowed `operation` on the repository they name. A caller who may not
/// view the repository hears that it is not there before the keeper is asked, so the answer names no keeper of a
/// repository the caller may not see.
fn forward_write(name: &str, operation: &str, parameters: &Parameters) -> Out {
    let Some(id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&id, "view") {
        add_error(format!("Repository {id} not found"));
        return none();
    }
    if !may_write(&id, operation, None) {
        return none();
    }
    Ok(moqui_core::service::sync(name, parameters)?)
}

fn create_ref(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(&format!("{GIT}create#GitRef"), "ref.create", parameters)
}

fn delete_ref(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(&format!("{GIT}delete#GitRef"), "ref.delete", parameters)
}

fn create_commit_status(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(
        &format!("{GIT}create#GitCommitStatus"),
        "commitStatus.create",
        parameters,
    )
}

fn run_pipeline(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(&format!("{CI}run#Pipeline"), "pipeline.run", parameters)
}

fn rerun_pipeline_run(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(&format!("{CI}rerun#PipelineRun"), "pipeline.rerun", parameters)
}

fn cancel_pipeline_run(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(&format!("{CI}cancel#PipelineRun"), "pipeline.cancel", parameters)
}

fn approve_pipeline_run(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(&format!("{CI}approve#PipelineRun"), "pipeline.approve", parameters)
}

/// `retry#JobRun`: the job of a run of `repositoryId` only (its path), asked of the keeper as the run's repository.
fn retry_job_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(id), Some(job)) = (text(parameters, "repositoryId"), text(parameters, "jobRunId")) else {
        add_error("repositoryId and jobRunId are required");
        return none();
    };
    let of_repository = (|| -> Result<bool, moqui_core::context::Error> {
        let _rows = rows_guard();
        let Some(job_row) = facade.find(JOB_RUN)?.eq("jobRunId", Value::string(&job)).one()? else {
            return Ok(false);
        };
        let run = job_row.get("pipelineRunId").map(to_plain_string).unwrap_or_default();
        let run_row = facade
            .find(PIPELINE_RUN)?
            .eq("pipelineRunId", Value::string(run))
            .one()?;
        Ok(run_row.is_some_and(|row| row.get("repositoryId").map(to_plain_string).as_deref() == Some(id.as_str())))
    })()
    .unwrap_or(false);
    if !of_repository || !may(&id, "view") {
        add_error(format!("No job run {job} in {id}"));
        return none();
    }
    forward_write(&format!("{CI}retry#JobRun"), "job.retry", parameters)
}

// ---- merge requests by their numbers ----

/// The id of merge request `mergeRequestNum` of `repositoryId`, for a caller who may view the repository, and, while
/// the application holds it hidden (`hiddenDate`), only for its author; else the error recorded, which says the same
/// whether it is not there or not to be seen. With the number as the path has it: what an error names.
fn merge_request_id(parameters: &Parameters) -> Option<MergeRequestRef> {
    let Some(id) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return None;
    };
    let num = text(parameters, "mergeRequestNum").unwrap_or_default();
    let not_found = || {
        add_error(format!("No merge request !{num} in {id}"));
        None
    };
    let Ok(number) = num.trim().parse::<i64>() else {
        return not_found();
    };
    if !may(&id, "view") {
        return not_found();
    }
    let entity = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()?;
    let found = {
        let _rows = rows_guard();
        entity.find(MERGE_REQUEST).and_then(|find| {
            find.eq("repositoryId", Value::string(&id))
                .eq("mergeRequestNum", Value::I64(number))
                .one()
        })
    };
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    match found {
        Ok(Some(row)) if visible(&row, &user) => Some(MergeRequestRef {
            repository_id: id,
            merge_request_id: row.get("mergeRequestId").map(to_plain_string).unwrap_or_default(),
            num,
        }),
        Ok(_) => not_found(),
        Err(err) => {
            add_error(format!("merge request !{num} of {id} not read: {err}"));
            None
        }
    }
}

/// A merge request the path names: its repository, its id and its number as given.
pub(crate) struct MergeRequestRef {
    pub repository_id: String,
    pub merge_request_id: String,
    pub num: String,
}

/// Whether a merge request row may be seen by `user`: not hidden, or hidden and the user's own.
pub(crate) fn visible(row: &moqui_core::entity::EntityValue, user: &str) -> bool {
    let hidden = row.get("hiddenDate").is_some_and(|value| !is_empty(value));
    !hidden || (!user.is_empty() && row.get("authorUserId").map(to_plain_string).as_deref() == Some(user))
}

/// An error of a framework service about the merge request, in the words of the path: `No merge request <id>` (which
/// names the internal id, and says what a hidden one says) becomes the answer of a number that is not there.
pub(crate) fn path_error(error: &str, merge_request: &MergeRequestRef) -> String {
    let internal = format!("No merge request {}", merge_request.merge_request_id);
    if error.trim() == internal || error.contains(&format!("{internal} ")) || error.ends_with(&internal) {
        format!(
            "No merge request !{} in {}",
            merge_request.num, merge_request.repository_id
        )
    } else {
        error.to_string()
    }
}

/// Call `name` for the merge request the path names, with its id in place of its number; a write (`operation`)
/// once the keeper allowed it. The service's errors are said in the path's words ([`path_error`]).
fn forward_merge_request(name: &str, operation: Option<&str>, parameters: &Parameters) -> Out {
    let Some(merge_request) = merge_request_id(parameters) else {
        return none();
    };
    if let Some(operation) = operation {
        if !may_write(
            &merge_request.repository_id,
            operation,
            Some(&merge_request.merge_request_id),
        ) {
            return none();
        }
    }
    let mut parameters = parameters.clone();
    parameters.insert("mergeRequestId".into(), Value::string(&merge_request.merge_request_id));
    match crate::ci::with_errors_taken(|| moqui_core::service::sync(name, &parameters)) {
        Ok(answer) => Ok(answer),
        Err(err) => {
            for line in err.message().lines().filter(|line| !line.trim().is_empty()) {
                add_error(path_error(line, &merge_request));
            }
            none()
        }
    }
}

/// The most merge requests one page of `find#MergeRequests` gives through REST, when `git-server`
/// `merge-requests-max-page` does not say.
pub(crate) const MAX_PAGE_SIZE: usize = 100;

/// The most repositories a REST `find#MergeRequests` that names none looks in, when `git-server`
/// `merge-requests-max-repositories` does not say.
pub(crate) const MAX_REPOSITORIES: usize = 500;

/// A `git-server` setting that is a positive number, else `default`.
fn positive_setting(name: &str, default: usize) -> usize {
    crate::setting(name)
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// The first `most` repositories the caller may view, by id, archived ones too (as the framework service lists them):
/// the walk stops there, so one call checks the view right of no more repositories than it must (SEC-GIT-18).
fn first_visible(facade: &EntityFacade, most: usize) -> Result<Vec<String>, moqui_core::context::Error> {
    let ids: Vec<String> = {
        let _rows = rows_guard();
        facade
            .find(GIT_REPOSITORY)?
            .select_field("repositoryId")
            .order_by("repositoryId", false)
            .list()?
            .iter()
            .filter_map(|row| row.get("repositoryId").map(to_plain_string))
            .collect()
    };
    Ok(ids.into_iter().filter(|id| may(id, "view")).take(most).collect())
}

/// `find#MergeRequests`: those of a repository, or of every repository the caller may view, by `filter`: `mine` (the
/// caller's), `review` (waiting for the caller's review), `open` (open and draft; the default) or `all`, with the
/// framework service's other filters, at most `git-server` `merge-requests-max-page` (100) a page. A call that names
/// no repository looks in the first `merge-requests-max-repositories` (500) the caller may view, by id (SEC-GIT-18).
fn find_merge_requests(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let mut parameters = parameters.clone();
    // the hidden ones are the application's to show, never a REST caller's to ask for
    parameters.remove("includeHidden");
    let size = text(&parameters, "pageSize")
        .and_then(|size| size.trim().parse::<i64>().ok())
        .unwrap_or(20);
    let most = i64::try_from(positive_setting("merge-requests-max-page", MAX_PAGE_SIZE)).unwrap_or(i64::MAX);
    parameters.insert("pageSize".into(), Value::I64(size.clamp(1, most)));
    let named = text(&parameters, "repositoryId").is_some()
        || !super::hosting::list_param(&parameters, "repositoryIdList").is_empty();
    if !named {
        let visible = first_visible(
            facade,
            positive_setting("merge-requests-max-repositories", MAX_REPOSITORIES),
        )?;
        if visible.is_empty() {
            return Ok(out([
                ("mergeRequestList", Value::List(Vec::new())),
                ("mergeRequestListCount", Value::I64(0)),
            ]));
        }
        parameters.insert(
            "repositoryIdList".into(),
            Value::List(visible.into_iter().map(Value::string).collect()),
        );
    }
    let user = moqui_core::user::current_user_id().ok().flatten();
    let filter = text(&parameters, "filter").unwrap_or_else(|| "open".into());
    let open = || Value::List(vec![Value::string("GmrDraft"), Value::string("GmrOpen")]);
    match filter.as_str() {
        "mine" => {
            parameters.insert("authorUserId".into(), user.map_or(Value::Null, Value::string));
        }
        "review" => {
            parameters.insert("waitingForUserId".into(), user.map_or(Value::Null, Value::string));
        }
        "open" => {}
        "all" => {}
        other => {
            add_error(format!("filter is mine, review, open or all, not {other}"));
            return none();
        }
    }
    if filter != "all" && text(&parameters, "statusId").is_none() && !parameters.contains_key("statusIdList") {
        parameters.insert("statusIdList".into(), open());
    }
    Ok(moqui_core::service::sync(
        &format!("{REVIEW}find#MergeRequests"),
        &parameters,
    )?)
}

fn create_merge_request(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_write(
        &format!("{REVIEW}create#MergeRequest"),
        "mergeRequest.create",
        parameters,
    )
}

fn get_merge_request(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(&format!("{REVIEW}get#MergeRequest"), None, parameters)
}

fn update_merge_request(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(
        &format!("{REVIEW}update#MergeRequest"),
        Some("mergeRequest.update"),
        parameters,
    )
}

fn close_merge_request(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(
        &format!("{REVIEW}close#MergeRequest"),
        Some("mergeRequest.close"),
        parameters,
    )
}

fn reopen_merge_request(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(
        &format!("{REVIEW}reopen#MergeRequest"),
        Some("mergeRequest.reopen"),
        parameters,
    )
}

fn set_merge_request_reviewers(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(
        &format!("{REVIEW}set#MergeRequestReviewers"),
        Some("mergeRequest.reviewers"),
        parameters,
    )
}

fn submit_merge_request_review(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(
        &format!("{REVIEW}submit#MergeRequestReview"),
        Some("mergeRequest.review"),
        parameters,
    )
}

fn get_merge_request_diff(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(&format!("{REVIEW}get#MergeRequestDiff"), None, parameters)
}

fn get_merge_request_approvals(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(&format!("{REVIEW}get#MergeRequestApprovals"), None, parameters)
}

fn merge_merge_request(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    forward_merge_request(
        &format!("{REVIEW}merge#MergeRequest"),
        Some("mergeRequest.merge"),
        parameters,
    )
}

// ---- review threads: an application's, through `git-server review-thread-service` ----

/// What a call of the review-thread service came to.
enum Thread {
    /// Its answer.
    Answer(BTreeMap<String, Value>),
    /// The server names no service.
    NoService,
    /// Refused or failed, with the errors recorded.
    Refused,
}

/// Call `git-server review-thread-service` (`handle#GitReviewThread`) for the merge request the path names, as the
/// caller: `operation` with the `keys` of `parameters`. A write is asked of the keeper first (`keeper_operation`). The
/// service's errors are said in the path's words.
fn thread_call(operation: &str, keeper_operation: Option<&str>, parameters: &Parameters, keys: &[&str]) -> Thread {
    let Some(merge_request) = merge_request_id(parameters) else {
        return Thread::Refused;
    };
    let name = crate::setting("review-thread-service");
    if name.is_empty() {
        return Thread::NoService;
    }
    if let Some(keeper_operation) = keeper_operation {
        if !may_write(
            &merge_request.repository_id,
            keeper_operation,
            Some(&merge_request.merge_request_id),
        ) {
            return Thread::Refused;
        }
    }
    let mut call = crate::ci::parameters([
        ("operation", Value::string(operation)),
        ("repositoryId", Value::string(&merge_request.repository_id)),
        ("mergeRequestId", Value::string(&merge_request.merge_request_id)),
    ]);
    for key in keys {
        if let Some(value) = parameters.get(*key).filter(|value| !is_empty(value)) {
            call.insert((*key).to_string(), value.clone());
        }
    }
    match crate::ci::with_errors_taken(|| moqui_core::service::sync(&name, &call)) {
        Ok(answer) => Thread::Answer(answer),
        Err(err) => {
            for line in err.message().lines().filter(|line| !line.trim().is_empty()) {
                add_error(path_error(line, &merge_request));
            }
            Thread::Refused
        }
    }
}

/// The refusal of a thread write on a server that keeps no threads.
const NO_THREADS: &str = "this server keeps no review threads (git-server review-thread-service names none)";

/// `find#ReviewThreads`: the merge request's threads (of `path`), from the application that keeps them; with none,
/// no threads and `threadsAvailable` false. Out: `threadList`, `threadsAvailable`.
fn find_review_threads(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    match thread_call("find", None, parameters, &["path"]) {
        Thread::Answer(answer) => Ok(out([
            (
                "threadList",
                answer.get("threadList").cloned().unwrap_or(Value::List(Vec::new())),
            ),
            ("threadsAvailable", Value::Bool(true)),
        ])),
        Thread::NoService => Ok(out([
            ("threadList", Value::List(Vec::new())),
            ("threadsAvailable", Value::Bool(false)),
        ])),
        Thread::Refused => none(),
    }
}

/// `create#ReviewComment`: a comment as the caller, a new thread or a reply. Out: `threadId`, `held`.
fn create_review_comment(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    if text(parameters, "body").is_none() {
        add_error("body is required");
        return none();
    }
    let keys = ["body", "threadId", "path", "commitId", "oldLine", "newLine", "lineText"];
    match thread_call("comment", Some("mergeRequest.comment"), parameters, &keys) {
        Thread::Answer(answer) => Ok(out([
            ("threadId", answer.get("threadId").cloned().unwrap_or(Value::Null)),
            ("held", answer.get("held").cloned().unwrap_or(Value::Bool(false))),
        ])),
        Thread::NoService => {
            add_error(NO_THREADS);
            none()
        }
        Thread::Refused => none(),
    }
}

/// `resolve#ReviewThread`: a thread resolved, or reopened with `resolved` false. Out: `threadId`.
fn resolve_review_thread(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let mut parameters = parameters.clone();
    parameters.entry("resolved".into()).or_insert(Value::Bool(true));
    match thread_call(
        "resolve",
        Some("mergeRequest.resolveThread"),
        &parameters,
        &["threadId", "resolved"],
    ) {
        Thread::Answer(answer) => Ok(out([(
            "threadId",
            answer
                .get("threadId")
                .filter(|value| !is_empty(value))
                .or_else(|| parameters.get("threadId"))
                .cloned()
                .unwrap_or(Value::Null),
        )])),
        Thread::NoService => {
            add_error(NO_THREADS);
            none()
        }
        Thread::Refused => none(),
    }
}

#[cfg(test)]
#[path = "rest_tests.rs"]
mod tests;
