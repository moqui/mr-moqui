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

//! Deployments (plans/Dogfood.md §8.6, DG10; docs/interfaces/Pipelines.md "Deployments"). A `GitDeployment` is a
//! branch's head asked to be deployed to an environment of its repository. The environment's pipeline runs for the
//! commit with the `deploy` event, through the engine's queue, so the run check still decides trust; when that run
//! passes, the deployment waits for an approver if the environment requires one (never the requester); then the deploy
//! step posts an attestation of the passed run ([`super::attest`]), signed with the token the target issued, to the
//! target instance's Component-Deploy REST, which fetches the branch, checks it is still at that commit, and deploys
//! it. A repository that is not a Moqui component (the framework's own) is not deployed here: its change ships as a
//! new archive or image. Each move is written as the commit status `deploy/<environment>` and told to the requester
//! (and to the approvers while one is awaited) on the notification topic [`TOPIC`].

use std::sync::Arc;

use moqui_lib::object::Value;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{Condition, EntityFacade, EntityValue};

use super::file::Event;
use super::queue::{self, RunRequest};
use super::{field, millis, now_millis, one, opt, set_fields, status, CI_ENVIRONMENT, DEPLOYMENT, PIPELINE_RUN};
use crate::GitProvider;

pub const REQUESTED: &str = "requested";
pub const WAITING_PIPELINE: &str = "waiting-pipeline";
pub const WAITING_APPROVAL: &str = "waiting-approval";
pub const DEPLOYING: &str = "deploying";
pub const DEPLOYED: &str = "deployed";
pub const FAILED: &str = "failed";
pub const REFUSED: &str = "refused";
pub const CANCELLED: &str = "cancelled";

/// The states a deployment ends in.
pub const ENDED: [&str; 4] = [DEPLOYED, FAILED, REFUSED, CANCELLED];

/// The notification topic of a deployment's moves: `{deploymentId, repositoryId, environment, commitId, statusId}`.
pub const TOPIC: &str = "GitDeployment";

/// The target's Component-Deploy REST that takes an attested deploy (`deploy#AttestedComponent`).
pub const REST_PATH: &str = "/rest/s1/moqui/components/deploy";

/// The commit status context of an environment's deployments.
pub fn context(environment: &str) -> String {
    format!("deploy/{environment}")
}

/// A deployment that was asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requested {
    pub deployment_id: String,
    pub status: String,
    pub pipeline_run_id: Option<String>,
    pub message: Option<String>,
}

/// Ask for the deployment of `commit_id`, the head of the branch `ref_name`, to `environment`, as `user_id`, and
/// queue the environment's pipeline for it with the `deploy` event. The caller checked the rights, the branch and the
/// environment.
///
/// # Errors
///
/// When a row is not written.
pub fn request(
    provider: &GitProvider,
    entity: &EntityFacade,
    environment: &EntityValue,
    ref_name: &str,
    commit_id: &str,
    user_id: Option<String>,
) -> Result<Requested, Error> {
    let _rows = crate::rows_guard();
    let repository = field(environment, "repositoryId").unwrap_or_default();
    let pipeline = field(environment, "requiredPipeline").unwrap_or_default();
    let mut row = EntityValue::make(entity, DEPLOYMENT)?;
    row.set_sequenced_id_primary(entity)?;
    let id = field(&row, "deploymentId").unwrap_or_default();
    row.set("repositoryId", Value::string(&repository));
    row.set("environmentId", opt(field(environment, "environmentId")));
    row.set("environmentName", opt(field(environment, "name")));
    row.set("commitId", Value::string(commit_id));
    row.set("refName", Value::string(ref_name));
    row.set("statusId", Value::string(REQUESTED));
    row.set("requestedByUserId", opt(user_id.clone()));
    row.set("createdDate", moqui_core::user::now_value());
    row.create(entity)?;
    write_status(entity, &row, "pending", "Requested");
    let refused = |message: String| -> Result<Requested, Error> {
        end(entity, &id, REQUESTED, FAILED, &message, vec![])?;
        Ok(Requested {
            deployment_id: id.clone(),
            status: FAILED.to_string(),
            pipeline_run_id: None,
            message: Some(message),
        })
    };
    if !queue::enabled(entity) {
        return refused("pipelines are not on here: there is no enabled runner pool".to_string());
    }
    let mut run_request = RunRequest::new(&repository, Event::Deploy, ref_name, commit_id);
    run_request.user_id = user_id;
    run_request.pipeline_name = Some(pipeline.clone());
    run_request.environment_id = field(environment, "environmentId");
    let runs = queue::queue(provider, entity, &run_request)?;
    let Some(run) = runs.first() else {
        return refused(format!(
            "{ref_name} at {} has no pipeline {pipeline} with the deploy event (or the run check skips it)",
            short(commit_id)
        ));
    };
    if run.status == "error" {
        let why = one(entity, PIPELINE_RUN, "pipelineRunId", &run.pipeline_run_id)?
            .and_then(|run| field(&run, "statusMessage"))
            .unwrap_or_else(|| "the run was refused".to_string());
        set_fields(
            entity,
            DEPLOYMENT,
            "deploymentId",
            &id,
            None,
            vec![("pipelineRunId", Value::string(&run.pipeline_run_id))],
        )?;
        let mut answer = refused(format!("pipeline {pipeline} run {}: {why}", run.run_num))?;
        answer.pipeline_run_id = Some(run.pipeline_run_id.clone());
        return Ok(answer);
    }
    set_fields(
        entity,
        DEPLOYMENT,
        "deploymentId",
        &id,
        Some(REQUESTED),
        vec![
            ("statusId", Value::string(WAITING_PIPELINE)),
            ("pipelineRunId", Value::string(&run.pipeline_run_id)),
        ],
    )?;
    if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", &id)? {
        write_status(entity, &row, "pending", &format!("Waiting for pipeline {pipeline}"));
    }
    Ok(Requested {
        deployment_id: id,
        status: WAITING_PIPELINE.to_string(),
        pipeline_run_id: Some(run.pipeline_run_id.clone()),
        message: None,
    })
}

/// A run ended (the engine, as a run's last job ends or the run is cancelled): each deployment that waited for it
/// fails when it did not pass, and otherwise waits for an approver or deploys. A failure is logged: the run's own end
/// stands.
pub(crate) fn run_ended(entity: &EntityFacade, pipeline_run_id: &str) {
    let moved = (|| -> Result<(), Error> {
        let _rows = crate::rows_guard();
        if !entity.table_ready(DEPLOYMENT) {
            return Ok(());
        }
        let Some(run) = one(entity, PIPELINE_RUN, "pipelineRunId", pipeline_run_id)? else {
            return Ok(());
        };
        if field(&run, "event").as_deref() != Some(Event::Deploy.name()) {
            return Ok(());
        }
        let waiting = entity
            .find(DEPLOYMENT)?
            .eq("pipelineRunId", Value::string(pipeline_run_id))
            .eq("statusId", Value::string(WAITING_PIPELINE))
            .list()?;
        for deployment in waiting.iter() {
            if let Err(err) = after_run(entity, &run, deployment) {
                tracing::error!(deployment = ?field(deployment, "deploymentId"), "a deployment was not moved on after its run: {err}");
            }
        }
        Ok(())
    })();
    if let Err(err) = moved {
        tracing::error!(
            run = pipeline_run_id,
            "the deployments of a run were not moved on: {err}"
        );
    }
}

fn after_run(entity: &EntityFacade, run: &EntityValue, deployment: &EntityValue) -> Result<(), Error> {
    let id = field(deployment, "deploymentId").unwrap_or_default();
    let run_status = field(run, "statusId").unwrap_or_default();
    if run_status != "success" {
        let message = format!(
            "pipeline {} run {} ended {run_status}{}",
            field(run, "pipelineName").unwrap_or_default(),
            field(run, "runNum").unwrap_or_default(),
            field(run, "statusMessage")
                .map(|why| format!(": {why}"))
                .unwrap_or_default()
        );
        return end(entity, &id, WAITING_PIPELINE, FAILED, &message, vec![]);
    }
    let environment = field(deployment, "environmentId")
        .map(|environment| one(entity, CI_ENVIRONMENT, "environmentId", &environment))
        .transpose()?
        .flatten();
    let Some(environment) = environment else {
        return end(entity, &id, WAITING_PIPELINE, FAILED, "its environment is gone", vec![]);
    };
    if super::is_yes(&environment, "approvalRequired") {
        // the run itself waited for an approver (a job in the environment, for its secrets): that approval counts
        // once, when it was an approver's other than the requester's
        let repository = field(deployment, "repositoryId").unwrap_or_default();
        let approved_by = field(run, "approvedByUserId").filter(|approver| {
            field(run, "approvalState").as_deref() == Some("approved")
                && Some(approver) != field(deployment, "requestedByUserId").as_ref()
                && is_approver(&environment, &repository, approver)
        });
        let Some(approver) = approved_by else {
            if set_fields(
                entity,
                DEPLOYMENT,
                "deploymentId",
                &id,
                Some(WAITING_PIPELINE),
                vec![("statusId", Value::string(WAITING_APPROVAL))],
            )? {
                if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", &id)? {
                    write_status(entity, &row, "pending", "Waiting for approval");
                    notify(&row, &environment, "waits for approval", true);
                }
            }
            return Ok(());
        };
        return ready(
            entity,
            &id,
            WAITING_PIPELINE,
            vec![
                ("approvedByUserId", Value::string(approver)),
                ("approvedDate", moqui_core::user::now_value()),
            ],
        );
    }
    ready(entity, &id, WAITING_PIPELINE, vec![])
}

/// Whether `user_id` approves deployments to `environment`, decided as that user: a member of its approver group
/// who views the repository, else a maintainer (the delete right).
pub(crate) fn is_approver(environment: &EntityValue, repository_id: &str, user_id: &str) -> bool {
    let group = field(environment, "approverGroupId");
    let repository = repository_id.to_string();
    moqui_core::user::as_user(user_id, move || approver_now(group.as_deref(), &repository)).unwrap_or(false)
}

/// Whether the current user approves for an environment of `repository_id` whose approver group is `group`.
pub(crate) fn approver_now(group: Option<&str>, repository_id: &str) -> bool {
    match group {
        Some(group) => {
            moqui_core::user::is_in_group(group).unwrap_or(false) && crate::server::permitted(repository_id, "view")
        }
        None => crate::server::permitted(repository_id, "delete"),
    }
}

/// Move a deployment from `from` to deploying (with `more` fields), and start its deploy step once the caller's
/// transaction commits.
fn ready(entity: &EntityFacade, id: &str, from: &str, more: Vec<(&str, Value)>) -> Result<(), Error> {
    let mut fields = vec![("statusId", Value::string(DEPLOYING))];
    fields.extend(more);
    if set_fields(entity, DEPLOYMENT, "deploymentId", id, Some(from), fields)? {
        if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", id)? {
            write_status(entity, &row, "pending", "Deploying");
        }
        kick(id);
    }
    Ok(())
}

/// Approve or refuse a deployment that waits for approval, as `user_id`; the caller checked that the user is an
/// approver and not the requester. Its new state.
///
/// # Errors
///
/// When it does not wait for approval.
pub fn decide(
    entity: &EntityFacade,
    deployment: &EntityValue,
    user_id: Option<String>,
    approve: bool,
    reason: Option<&str>,
) -> Result<String, Error> {
    let _rows = crate::rows_guard();
    let id = field(deployment, "deploymentId").unwrap_or_default();
    if field(deployment, "statusId").as_deref() != Some(WAITING_APPROVAL) {
        return Err(Error::new(format!("deployment {id} does not wait for approval")));
    }
    if approve {
        ready(
            entity,
            &id,
            WAITING_APPROVAL,
            vec![
                ("approvedByUserId", opt(user_id)),
                ("approvedDate", moqui_core::user::now_value()),
            ],
        )?;
        return Ok(DEPLOYING.to_string());
    }
    let message = format!(
        "Refused by {}{}",
        user_id.clone().unwrap_or_else(|| "the application".to_string()),
        reason.map(|why| format!(": {why}")).unwrap_or_default()
    );
    end(
        entity,
        &id,
        WAITING_APPROVAL,
        REFUSED,
        &message,
        vec![("refusedByUserId", opt(user_id))],
    )?;
    Ok(REFUSED.to_string())
}

/// Cancel a deployment whose deploy step has not begun; a run of its pipeline that has not ended is cancelled too.
///
/// # Errors
///
/// When it has ended or its deploy step has begun.
pub fn cancel(entity: &EntityFacade, deployment: &EntityValue, user_id: Option<&str>) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let id = field(deployment, "deploymentId").unwrap_or_default();
    let state = field(deployment, "statusId").unwrap_or_default();
    let message = format!("Cancelled by {}", user_id.unwrap_or("the application"));
    let cancelled = match state.as_str() {
        REQUESTED | WAITING_PIPELINE | WAITING_APPROVAL => {
            end(entity, &id, &state, CANCELLED, &message, vec![]).map(|()| {
                one(entity, DEPLOYMENT, "deploymentId", &id)
                    .ok()
                    .flatten()
                    .is_some_and(|row| field(&row, "statusId").as_deref() == Some(CANCELLED))
            })?
        }
        // not yet begun: the step's claim and this cancel take the same row by one conditional update each
        DEPLOYING => {
            let changed = crate::conditional::update_where(
                entity,
                DEPLOYMENT,
                &[
                    ("statusId", Value::string(CANCELLED)),
                    ("errorText", Value::string(&message)),
                    ("finishedDate", moqui_core::user::now_value()),
                ],
                &not_started(&id),
                super::Commit::Caller,
            )? > 0;
            if changed {
                super::audit_status(
                    entity,
                    DEPLOYMENT,
                    &id,
                    DEPLOYING,
                    &[("statusId", Value::string(CANCELLED))],
                );
                if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", &id)? {
                    write_status(entity, &row, "error", &message);
                    notify_end(entity, &row);
                }
            }
            changed
        }
        _ => false,
    };
    if !cancelled {
        return Err(Error::new(format!(
            "deployment {id} has ended or its deploy step has begun ({state})"
        )));
    }
    if state == WAITING_PIPELINE {
        if let Some(run_id) = field(deployment, "pipelineRunId") {
            // a run that has ended meanwhile is left as it is
            let _ = super::engine::cancel_run(entity, &run_id);
        }
    }
    Ok(())
}

fn not_started(id: &str) -> Condition {
    Condition::and(vec![
        Condition::equals("deploymentId", Value::string(id)),
        Condition::equals("statusId", Value::string(DEPLOYING)),
        Condition::is_null("startedDate"),
    ])
}

/// End a deployment that is still in `from`: its `errorText`, the status and the notices.
fn end(
    entity: &EntityFacade,
    id: &str,
    from: &str,
    to: &str,
    message: &str,
    more: Vec<(&str, Value)>,
) -> Result<(), Error> {
    let mut fields = vec![
        ("statusId", Value::string(to)),
        ("errorText", Value::string(message)),
        ("finishedDate", moqui_core::user::now_value()),
    ];
    fields.extend(more);
    if set_fields(entity, DEPLOYMENT, "deploymentId", id, Some(from), fields)? {
        if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", id)? {
            let state = if to == FAILED { "failure" } else { "error" };
            write_status(entity, &row, state, message);
            notify_end(entity, &row);
        }
    }
    Ok(())
}

// ---- the deploy step ----

/// Start the deploy step of `id` soon, on a thread of its own, once the caller's transaction commits (the step reads
/// what it wrote). Not with `git-ci` `start-inline` false, as the job starts are not: a test runs [`step`] itself,
/// and `end#OverdueJobRuns` starts a step left waiting.
fn kick(id: &str) {
    let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
        return;
    };
    if super::setting("start-inline") == "false" {
        return;
    }
    let id = id.to_string();
    let deferred = (|| -> Result<bool, Error> {
        let Some(entity) = factory.entity() else {
            return Ok(false);
        };
        let transactions = entity.transactions_for(DEPLOYMENT)?;
        if !transactions.in_transaction() {
            return Ok(false);
        }
        let factory = Arc::clone(&factory);
        let id = id.clone();
        transactions.on_commit(move || spawn_step(&factory, id))?;
        Ok(true)
    })()
    .unwrap_or(false);
    if !deferred {
        spawn_step(&factory, id);
    }
}

fn spawn_step(factory: &Arc<moqui_core::context::ExecutionContextFactory>, id: String) {
    let weak = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new().name("git-ci-deploy".into()).spawn(move || {
        let Some(factory) = weak.upgrade() else { return };
        let passed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let context = factory.new_context();
            let _entered = context.enter();
            if let Some(entity) = factory.entity() {
                if let Err(err) = step(&entity, &id) {
                    tracing::error!(deployment = %id, "a deploy step failed: {err}");
                }
            }
        }));
        if passed.is_err() {
            tracing::error!(deployment = %id, "a deploy step panicked");
        }
    });
    if let Err(err) = spawned {
        tracing::warn!("a deploy step was left for the overdue job: {err}");
    }
}

/// Deployments the deploy step did not take (a server that stopped before the thread ran) are started again, and
/// those whose step began more than its time limit ago and never ended (a server that stopped while it ran) fail, as
/// of `now`. For `end#OverdueJobRuns`. The count of each.
pub fn resume(entity: &EntityFacade, now: i64) -> Result<(usize, usize), Error> {
    let _rows = crate::rows_guard();
    if !entity.table_ready(DEPLOYMENT) {
        return Ok((0, 0));
    }
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()));
    let limit = i64::try_from(timeout().as_millis()).unwrap_or(i64::MAX) + 5 * 60_000;
    let (mut started, mut ended) = (0, 0);
    for row in entity
        .find(DEPLOYMENT)?
        .eq("statusId", Value::string(DEPLOYING))
        .list()?
        .iter()
    {
        let id = field(row, "deploymentId").unwrap_or_default();
        match millis(row, "startedDate") {
            None => {
                let quiet = millis(row, "lastUpdatedStamp").is_none_or(|stamp| now - stamp > 60_000);
                if let (true, Some(factory)) = (quiet, factory.as_ref()) {
                    spawn_step(factory, id);
                    started += 1;
                }
            }
            Some(began) if now - began > limit => {
                let fields = vec![
                    ("statusId", Value::string(FAILED)),
                    (
                        "errorText",
                        Value::string("the deploy step never ended (the server stopped while it ran?): deploy again"),
                    ),
                    ("finishedDate", moqui_core::user::now_value()),
                ];
                if set_fields(entity, DEPLOYMENT, "deploymentId", &id, Some(DEPLOYING), fields)? {
                    if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", &id)? {
                        write_status(entity, &row, "failure", "The deploy step never ended");
                        notify_end(entity, &row);
                    }
                    ended += 1;
                }
            }
            Some(_) => {}
        }
    }
    Ok((started, ended))
}

/// How long the target is given to fetch and deploy: `git-ci` `deploy-timeout-seconds` (600).
fn timeout() -> std::time::Duration {
    std::time::Duration::from_secs(super::number_setting("deploy-timeout-seconds", 600))
}

/// What the target answered.
struct Answer {
    component: Option<String>,
    outcome: Result<String, String>,
}

/// Run the deploy step of a deployment that is ready: claimed by one conditional update (`startedDate` set), so of
/// two threads or members only one calls the target; then the call, and the outcome on the row, the environment, the
/// status and the notices.
///
/// # Errors
///
/// When a row is not read or written.
pub fn step(entity: &EntityFacade, id: &str) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let claimed = crate::conditional::update_where(
        entity,
        DEPLOYMENT,
        &[("startedDate", moqui_core::user::now_value())],
        &not_started(id),
        super::Commit::Caller,
    )? > 0;
    if !claimed {
        return Ok(());
    }
    let Some(deployment) = one(entity, DEPLOYMENT, "deploymentId", id)? else {
        return Ok(());
    };
    let answer = call_target(entity, &deployment);
    let mut fields = vec![("componentName", opt(answer.component.clone()))];
    let (to, state, description) = match &answer.outcome {
        Ok(result) => {
            fields.push(("deployResult", Value::string(result)));
            (DEPLOYED, "success", "Deployed".to_string())
        }
        Err(why) => {
            fields.push(("errorText", Value::string(why)));
            (FAILED, "failure", why.clone())
        }
    };
    fields.push(("statusId", Value::string(to)));
    fields.push(("finishedDate", moqui_core::user::now_value()));
    if !set_fields(entity, DEPLOYMENT, "deploymentId", id, Some(DEPLOYING), fields)? {
        return Ok(());
    }
    if to == DEPLOYED {
        if let Some(environment) = field(&deployment, "environmentId") {
            set_fields(
                entity,
                CI_ENVIRONMENT,
                "environmentId",
                &environment,
                None,
                vec![
                    ("lastCommitId", opt(field(&deployment, "commitId"))),
                    ("lastDeploymentId", Value::string(id)),
                    ("lastDeployDate", moqui_core::user::now_value()),
                ],
            )?;
        }
    }
    if let Some(row) = one(entity, DEPLOYMENT, "deploymentId", id)? {
        write_status(entity, &row, state, &description);
        notify_end(entity, &row);
    }
    Ok(())
}

/// The deploy call: the component at the commit, the target and its token, the outbound check, then the signed
/// attestation posted to the target. Never logs or answers the token, which is not sent either.
fn call_target(entity: &EntityFacade, deployment: &EntityValue) -> Answer {
    let repository = field(deployment, "repositoryId").unwrap_or_default();
    let commit = field(deployment, "commitId").unwrap_or_default();
    let component = match component_at(&repository, &commit) {
        Ok(Some(name)) => name,
        Ok(None) => {
            return Answer {
                component: None,
                outcome: Err(format!(
                    "not deployable here: repository {repository} is not a Moqui component (no component.xml at its \
                     top); a framework change deploys as a new archive or image"
                )),
            }
        }
        Err(err) => {
            return Answer {
                component: None,
                outcome: Err(format!("its component.xml was not read: {err}")),
            }
        }
    };
    let outcome = attest_and_post(entity, deployment, &component);
    Answer {
        component: Some(component),
        outcome,
    }
}

/// The name in `component.xml` at the commit, read as the engine reads a pipeline file.
fn component_at(repository: &str, commit: &str) -> Result<Option<String>, Error> {
    let Some(provider) = ExecutionContext::with_current(|ec| crate::provider(ec.factory())).flatten() else {
        return Err(Error::new("git resources are not installed"));
    };
    let _rows = crate::rows_guard();
    let _hidden = crate::reach::with_hidden(true);
    Ok(provider
        .blob_at(repository, commit, &["component.xml".to_string()])?
        .and_then(|bytes| super::builtin::component(&String::from_utf8_lossy(&bytes)))
        .map(|component| component.name))
}

/// The token an environment's target issued, which signs its attestations. The one place it is read: it is to become
/// a vault item (plans/Vault-Instances.md §2, InstanceEnvironment), and only this changes then.
pub(crate) fn target_token(environment: &EntityValue) -> Option<String> {
    field(environment, "targetToken")
}

/// The endpoint of an environment's target: `https`, or `http` to a loopback IP literal only (the instance and
/// itself), and no credentials in the URL. Why not.
pub(crate) fn endpoint(target_url: &str) -> Result<String, String> {
    let base = target_url.trim().trim_end_matches('/');
    let Some((scheme, rest)) = base.split_once("://") else {
        return Err("the environment's target URL is not an http(s) URL".to_string());
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err("the environment's target URL has no host, or holds credentials".to_string());
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => {}
        "http" if moqui_lib::http::host_address(base).is_some_and(|address| address.is_loopback()) => {}
        _ => {
            return Err("the environment's target URL must be https (http only to a loopback IP address)".to_string());
        }
    }
    Ok(format!("{base}{REST_PATH}"))
}

fn attest_and_post(entity: &EntityFacade, deployment: &EntityValue, component: &str) -> Result<String, String> {
    let repository = field(deployment, "repositoryId").unwrap_or_default();
    let environment = field(deployment, "environmentId")
        .and_then(|id| one(entity, CI_ENVIRONMENT, "environmentId", &id).ok().flatten())
        .ok_or_else(|| "its environment is gone".to_string())?;
    let name = field(&environment, "name").unwrap_or_default();
    let url = endpoint(&field(&environment, "targetUrl").unwrap_or_default())?;
    let Some(token) = target_token(&environment) else {
        return Err(format!("environment {name} has no token from its target"));
    };
    crate::outbound_allowed(&repository, crate::Outbound::Deploy, &name)?;
    let run = field(deployment, "pipelineRunId")
        .and_then(|id| one(entity, PIPELINE_RUN, "pipelineRunId", &id).ok().flatten())
        .ok_or_else(|| "its pipeline run is gone".to_string())?;
    if field(&run, "statusId").as_deref() != Some("success") {
        return Err("its pipeline run did not pass".to_string());
    }
    let base = crate::server::public_base_url();
    let repository_url = field(&environment, "cloneUrl")
        .unwrap_or_else(|| format!("{base}/git/{}.git", crate::server::path::path_of(&repository)));
    let attestation = super::attest::Attestation {
        kind: super::attest::KIND.to_string(),
        v: 1,
        repository_url,
        ref_name: field(deployment, "refName").unwrap_or_default(),
        commit_id: field(deployment, "commitId").unwrap_or_default(),
        component: component.to_string(),
        pipeline: field(&run, "pipelineName").unwrap_or_default(),
        pipeline_run_id: field(&run, "pipelineRunId").unwrap_or_default(),
        run_num: super::long(&run, "runNum").unwrap_or(0),
        run_status: "success".to_string(),
        environment: name.clone(),
        deployment_id: field(deployment, "deploymentId").unwrap_or_default(),
        issued_at: now_millis(),
        source: base,
    };
    let signed = super::attest::sign(&token, &attestation)?;
    let answered = post(&url, &attestation.deployment_id, &signed)?;
    // a 2xx that is not a deploy's answer for this commit (a page at that URL, another service) deployed nothing
    let status = answered["deployStatus"].as_str().unwrap_or_default();
    if !matches!(status, "deployed" | "up-to-date")
        || answered["commitId"].as_str() != Some(attestation.commit_id.as_str())
    {
        return Err(format!(
            "the target {} did not answer as a deploy of this commit does",
            moqui_lib::http::redacted(&url)
        ));
    }
    let result = serde_json::json!({
        "status": status,
        "component": answered["componentName"],
        "commitId": answered["commitId"],
        "problems": answered["report"]["problems"],
    });
    Ok(result.to_string())
}

/// POST the signed attestation, as webhooks are sent: no redirect, a time limit, internal addresses refused unless
/// `git-ci` `deploy-private-network` is true. What the target answered, or why it failed.
fn post(url: &str, deployment_id: &str, signed: &super::attest::Signed) -> Result<serde_json::Value, String> {
    let private = super::setting("deploy-private-network") == "true";
    let sim = ExecutionContext::with_current(|ec| ec.factory().entity().is_some_and(|entity| entity.sim_active()))
        .unwrap_or(false);
    let client = moqui_lib::http::RestClient::new()
        .deny_internal_addresses(!private)
        .timeout(timeout())
        .max_response_bytes(1024 * 1024)
        .truncate_body(true)
        .redirects(0, false);
    let body = serde_json::json!({"attestation": signed.text, "signature": signed.signature});
    let mut call = client
        .post(url)
        .header("user-agent", "Moqui-Deploy/1.0")
        .header("x-moqui-deployment", deployment_id)
        .json(&body);
    let mut traced = moqui_core::outbound::ClientCall::start("POST", url);
    for (name, value) in traced.headers() {
        call = call.header(name, value);
    }
    let shown = moqui_lib::http::redacted(url);
    let response = match call.send(sim) {
        Ok(response) => {
            traced.status(response.status());
            response
        }
        Err(err) => {
            traced.error(err.message());
            return Err(format!("the target {shown} was not reached: {}", err.message()));
        }
    };
    let answered: serde_json::Value = serde_json::from_str(response.body()).unwrap_or(serde_json::Value::Null);
    if !(200..300).contains(&response.status()) {
        return Err(format!(
            "the target {shown} answered {}: {}",
            response.status(),
            errors_of(&answered, response.body())
        ));
    }
    Ok(answered)
}

/// The errors of a REST answer, else the start of its text.
fn errors_of(answered: &serde_json::Value, body: &str) -> String {
    let text = match &answered["errors"] {
        serde_json::Value::String(errors) => errors.clone(),
        serde_json::Value::Array(errors) => errors
            .iter()
            .map(|error| error.as_str().map_or_else(|| error.to_string(), str::to_string))
            .collect::<Vec<_>>()
            .join("; "),
        _ => answered["error"]
            .as_str()
            .map_or_else(|| body.chars().take(300).collect(), str::to_string),
    };
    text.chars().take(2000).collect()
}

// ---- statuses and notices ----

/// Write the `deploy/<environment>` status of a deployment's commit, as the system. A failure is logged.
fn write_status(entity: &EntityFacade, deployment: &EntityValue, state: &str, description: &str) {
    let (Some(repository), Some(commit), Some(environment)) = (
        field(deployment, "repositoryId"),
        field(deployment, "commitId"),
        field(deployment, "environmentName"),
    ) else {
        return;
    };
    let target = field(deployment, "pipelineRunId")
        .and_then(|id| one(entity, PIPELINE_RUN, "pipelineRunId", &id).ok().flatten())
        .and_then(|run| status::run_url(&run, None));
    if let Err(err) = status::write(&repository, &commit, &context(&environment), state, description, target) {
        tracing::warn!(deployment = ?field(deployment, "deploymentId"), "the status of a deployment was not written: {err}");
    }
}

/// Tell the requester how a deployment ended (and the approvers, when one was awaited or decided).
fn notify_end(entity: &EntityFacade, deployment: &EntityValue) {
    let environment = field(deployment, "environmentId")
        .and_then(|id| one(entity, CI_ENVIRONMENT, "environmentId", &id).ok().flatten());
    let state = field(deployment, "statusId").unwrap_or_default();
    let what = match state.as_str() {
        DEPLOYED => "was deployed",
        REFUSED => "was refused",
        CANCELLED => "was cancelled",
        _ => "failed",
    };
    let to_approvers = field(deployment, "approvedByUserId").is_some() || state == REFUSED;
    match environment {
        Some(environment) => notify(deployment, &environment, what, to_approvers),
        None => notify_users(deployment, None, what),
    }
}

fn notify(deployment: &EntityValue, environment: &EntityValue, what: &str, to_approvers: bool) {
    let group = field(environment, "approverGroupId").filter(|_| to_approvers);
    notify_users(deployment, group.as_deref(), what);
}

fn notify_users(deployment: &EntityValue, approver_group: Option<&str>, what: &str) {
    let id = field(deployment, "deploymentId").unwrap_or_default();
    let repository = field(deployment, "repositoryId").unwrap_or_default();
    let environment = field(deployment, "environmentName").unwrap_or_default();
    let commit = field(deployment, "commitId").unwrap_or_default();
    let title = format!(
        "The deployment of {repository} {} to {environment} {what}",
        short(&commit)
    );
    let message = || {
        let mut message = moqui_lib::object::ValueMap::default();
        for (key, value) in [
            ("deploymentId", id.as_str()),
            ("repositoryId", repository.as_str()),
            ("environment", environment.as_str()),
            ("commitId", commit.as_str()),
        ] {
            message.insert(key.to_string(), Value::string(value));
        }
        message.insert("statusId".to_string(), opt(field(deployment, "statusId")));
        message
    };
    let mut targets: Vec<(Option<String>, Option<&str>)> = Vec::new();
    if let Some(user) = field(deployment, "requestedByUserId") {
        targets.push((Some(user), None));
    }
    if let Some(group) = approver_group {
        targets.push((None, Some(group)));
    }
    for (user, group) in targets {
        let mut notice = moqui_core::notify::NotificationMessage::new();
        notice
            .topic(TOPIC)
            .sub_topic(&repository)
            .title(title.clone())
            .message_map(message())
            .persist(true)
            .show_alert(true);
        if let Some(user) = &user {
            notice.user_id(user);
        }
        if let Some(group) = group {
            notice.user_group_id(group);
        }
        if let Err(err) = notice.send() {
            tracing::warn!(deployment = %id, "a deployment notice was not sent: {err}");
        }
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

#[cfg(test)]
#[path = "deploy_tests.rs"]
mod tests;
