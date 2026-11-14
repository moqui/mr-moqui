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

//! The deployments of `org.moqui.git.GitCiServices` (docs/interfaces/Pipelines.md "Deployments"): asked for with the
//! update right on the repository, approved or refused by an approver who is not the requester, cancelled by the
//! requester or with the update right, read with the view right. A repository the caller does not view is not found,
//! and so is its deployment. The moves are [`crate::ci::deploy`]'s.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::rust_service;
use moqui_lib::object::Value;

use super::{
    add_error, authz_disabled, current_user, field_value, int, may, none, number, out, provider, refuse, text,
};
use super::{Out, Parameters};
use crate::ci::file::Event;
use crate::ci::{deploy, field, one, CI_ENVIRONMENT, DEPLOYMENT, PIPELINE_RUN};
use crate::values::map;

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, request_git_deployment)?;
    rust_service!(map, decide_git_deployment)?;
    rust_service!(map, cancel_git_deployment)?;
    rust_service!(map, find_git_deployments)?;
    rust_service!(map, get_git_deployment)?;
    Ok(())
}

/// The environment a deployment is asked for: by `environmentId`, else by its `environment` name, of the repository.
fn environment_of(facade: &EntityFacade, parameters: &Parameters, repository: &str) -> Option<EntityValue> {
    let _rows = crate::rows_guard();
    let found = match (text(parameters, "environmentId"), text(parameters, "environment")) {
        (Some(id), _) => one(facade, CI_ENVIRONMENT, "environmentId", &id)
            .ok()
            .flatten()
            .filter(|row| field(row, "repositoryId").as_deref() == Some(repository)),
        (None, Some(name)) => crate::ci::trust::environment_named(facade, repository, None, &name)
            .ok()
            .flatten(),
        (None, None) => {
            add_error("environment or environmentId is required");
            return None;
        }
    };
    if found.is_none() {
        add_error(format!("repository {repository} has no such environment"));
    }
    found
}

/// `request#GitDeployment`: deploy the head of a branch (`refName`, the default branch when empty; `commitId`, when
/// given, must be its head) to an environment of the repository, as the caller. Needs the update right; the branch
/// must be protected or one the environment's `allowedRefs` names; the environment must name its pipeline, its
/// target and its token. Out: `deploymentId`, `statusId`, `pipelineRunId`, `message`.
fn request_git_deployment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&repository, "update") {
        refuse(&repository, "deploying");
        return none();
    }
    let Some(environment) = environment_of(facade, parameters, &repository) else {
        return none();
    };
    let name = field(&environment, "name").unwrap_or_default();
    if field(&environment, "requiredPipeline").is_none() {
        add_error(format!(
            "environment {name} names no pipeline that must pass (requiredPipeline)"
        ));
        return none();
    }
    if field(&environment, "targetUrl").is_none() || deploy::target_token(&environment).is_none() {
        add_error(format!(
            "environment {name} has no target URL or no token from its target"
        ));
        return none();
    }
    let Some(provider) = provider() else { return none() };
    let repo = provider.repo(&repository)?;
    let ref_name = match text(parameters, "refName") {
        Some(name) if name.starts_with("refs/") => name,
        Some(name) => crate::ops::branch_ref(&name),
        None => crate::ops::branch_ref(&repo.definition.branch),
    };
    if !ref_name.starts_with("refs/heads/") {
        add_error("a deployment is of a branch: the target fetches the branch and deploys its head");
        return none();
    }
    let Some(head) = provider.resolve_commit(&repository, &ref_name)? else {
        add_error(format!("repository {repository} has no {ref_name}"));
        return none();
    };
    let head = head.to_string();
    if let Some(asked) = text(parameters, "commitId") {
        let resolved = provider.resolve_commit(&repository, &asked)?.map(|id| id.to_string());
        if resolved.as_deref() != Some(head.as_str()) {
            add_error(format!(
                "commit {asked} is not the head of {ref_name}: the target deploys a branch's head, so deploy the head"
            ));
            return none();
        }
    }
    let allowed = {
        let _rows = crate::rows_guard();
        crate::ci::trust::is_protected(facade, &repository, Event::Deploy, &ref_name)?
            || crate::ci::trust::allowed_ref(&environment, &ref_name)
    };
    if !allowed {
        add_error(format!(
            "environment {name} takes protected branches and those its allowedRefs names; {ref_name} is neither"
        ));
        return none();
    }
    let requested = deploy::request(&provider, facade, &environment, &ref_name, &head, current_user())?;
    Ok(out([
        ("deploymentId", Value::string(&requested.deployment_id)),
        ("statusId", Value::string(&requested.status)),
        (
            "pipelineRunId",
            requested.pipeline_run_id.map_or(Value::Null, Value::string),
        ),
        ("message", requested.message.map_or(Value::Null, Value::string)),
    ]))
}

/// The deployment `deploymentId` when the caller views its repository; otherwise not found, the same for one that is
/// not there.
fn deployment_of(
    facade: &EntityFacade,
    parameters: &Parameters,
) -> Result<Option<EntityValue>, moqui_lib::error::Error> {
    let Some(id) = text(parameters, "deploymentId") else {
        add_error("deploymentId is required");
        return Ok(None);
    };
    let found = {
        let _rows = crate::rows_guard();
        one(facade, DEPLOYMENT, "deploymentId", &id)?
    };
    match found.filter(|row| may(&field(row, "repositoryId").unwrap_or_default(), "view")) {
        Some(row) => Ok(Some(row)),
        None => {
            add_error(format!("no deployment {id}"));
            Ok(None)
        }
    }
}

fn environment_row(facade: &EntityFacade, deployment: &EntityValue) -> Option<EntityValue> {
    let _rows = crate::rows_guard();
    field(deployment, "environmentId").and_then(|id| one(facade, CI_ENVIRONMENT, "environmentId", &id).ok().flatten())
}

/// Whether the caller approves `deployment`: an approver of its environment who did not ask for it, or the application
/// (authorization disabled) for a requester other than its own user.
fn may_decide(facade: &EntityFacade, deployment: &EntityValue) -> Result<(), String> {
    let repository = field(deployment, "repositoryId").unwrap_or_default();
    let user = current_user();
    if user.is_some() && user == field(deployment, "requestedByUserId") {
        return Err("a deployment is approved by another than who asked for it".to_string());
    }
    if authz_disabled() {
        return Ok(());
    }
    let group = environment_row(facade, deployment).and_then(|environment| field(&environment, "approverGroupId"));
    if deploy::approver_now(group.as_deref(), &repository) {
        Ok(())
    } else {
        Err(match group {
            Some(group) => format!("deciding a deployment needs the approver group {group}"),
            None => format!("deciding a deployment needs the delete right on repository {repository} (a maintainer)"),
        })
    }
}

/// `decide#GitDeployment`: approve or refuse (`decision`, with a `reason`) a deployment whose pipeline passed and that
/// waits for approval. By an approver (the environment's approver group, else a maintainer), never the requester.
/// Out: `statusId`.
fn decide_git_deployment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(deployment) = deployment_of(facade, parameters)? else {
        return none();
    };
    let approve = match text(parameters, "decision").as_deref() {
        Some("approve") => true,
        Some("refuse") => false,
        _ => {
            add_error("decision is approve or refuse");
            return none();
        }
    };
    if let Err(why) = may_decide(facade, &deployment) {
        add_error(why);
        return none();
    }
    match deploy::decide(
        facade,
        &deployment,
        current_user(),
        approve,
        text(parameters, "reason").as_deref(),
    ) {
        Ok(status) => Ok(out([("statusId", Value::string(status))])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `cancel#GitDeployment`: cancel a deployment whose deploy step has not begun, and its pipeline's run when it has not
/// ended. By the requester, or with the update right. Out: `statusId`.
fn cancel_git_deployment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(deployment) = deployment_of(facade, parameters)? else {
        return none();
    };
    let repository = field(&deployment, "repositoryId").unwrap_or_default();
    let mine = current_user().is_some() && current_user() == field(&deployment, "requestedByUserId");
    if !mine && !may(&repository, "update") {
        refuse(&repository, "cancelling a deployment");
        return none();
    }
    match deploy::cancel(facade, &deployment, current_user().as_deref()) {
        Ok(()) => Ok(out([("statusId", Value::string(deploy::CANCELLED))])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

fn deployment_value(facade: &EntityFacade, row: &EntityValue) -> Value {
    let copy = |name: &'static str| (name, field_value(row, name));
    let run_num = field(row, "pipelineRunId")
        .and_then(|id| one(facade, PIPELINE_RUN, "pipelineRunId", &id).ok().flatten())
        .and_then(|run| crate::ci::long(&run, "runNum"));
    map([
        copy("deploymentId"),
        copy("repositoryId"),
        copy("environmentId"),
        ("environment", field_value(row, "environmentName")),
        copy("commitId"),
        copy("refName"),
        copy("componentName"),
        copy("statusId"),
        copy("pipelineRunId"),
        ("runNum", run_num.map_or(Value::Null, int)),
        copy("requestedByUserId"),
        copy("approvedByUserId"),
        copy("approvedDate"),
        copy("refusedByUserId"),
        copy("deployResult"),
        copy("errorText"),
        copy("createdDate"),
        copy("startedDate"),
        copy("finishedDate"),
    ])
}

/// `find#GitDeployments`: the deployments of a repository the caller views, newest first, by `environment` (name),
/// `statusId` and `commitId`; paged (`pageIndex`, `pageSize` 20, at most 100). Out: `deploymentList`,
/// `deploymentCount`.
fn find_git_deployments(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&repository, "view") {
        add_error(format!("repository {repository} not found"));
        return none();
    }
    let _rows = crate::rows_guard();
    let mut find = facade.find(DEPLOYMENT)?.eq("repositoryId", Value::string(&repository));
    for (parameter, field_name) in [
        ("environment", "environmentName"),
        ("statusId", "statusId"),
        ("commitId", "commitId"),
    ] {
        if let Some(value) = text(parameters, parameter) {
            find = find.eq(field_name, Value::string(value));
        }
    }
    let rows = find.order_by("createdDate", true).limit(5000).list()?;
    let size = usize::try_from(number(parameters, "pageSize", 20).clamp(1, 100)).unwrap_or(20);
    let index = usize::try_from(number(parameters, "pageIndex", 0).max(0)).unwrap_or(0);
    let page: Vec<Value> = rows
        .iter()
        .skip(index * size)
        .take(size)
        .map(|row| deployment_value(facade, row))
        .collect();
    Ok(out([
        ("deploymentList", Value::List(page)),
        ("deploymentCount", int(i64::try_from(rows.len()).unwrap_or(0))),
    ]))
}

/// `get#GitDeployment`: one deployment of a repository the caller views. Out: `deployment`, `canDecide` (it waits
/// for approval and the caller may decide it), `canCancel`.
fn get_git_deployment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(deployment) = deployment_of(facade, parameters)? else {
        return none();
    };
    let repository = field(&deployment, "repositoryId").unwrap_or_default();
    let state = field(&deployment, "statusId").unwrap_or_default();
    let can_decide = state == deploy::WAITING_APPROVAL && may_decide(facade, &deployment).is_ok();
    let mine = current_user().is_some() && current_user() == field(&deployment, "requestedByUserId");
    let can_cancel = (mine || may(&repository, "update"))
        && (matches!(
            state.as_str(),
            deploy::REQUESTED | deploy::WAITING_PIPELINE | deploy::WAITING_APPROVAL
        ) || (state == deploy::DEPLOYING && field(&deployment, "startedDate").is_none()));
    let value = {
        let _rows = crate::rows_guard();
        deployment_value(facade, &deployment)
    };
    Ok(out([
        ("deployment", value),
        ("canDecide", Value::Bool(can_decide)),
        ("canCancel", Value::Bool(can_cancel)),
    ]))
}
