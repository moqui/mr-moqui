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

//! `org.moqui.git.GitCiServices` (docs/interfaces/Pipelines.md "Services"): what the gitECA rules and the ServiceJobs
//! call to queue and move runs, and what an application reads and writes. Reading a run follows viewing its
//! repository, and a run of a hidden merge request's head is read only with authorization disabled and
//! `includeHidden`, or by the user it is for. Cancel, run, re-run, retry and approve need the update right on the
//! repository; so do a repository's secrets and environments, whose values no service reads back. An owner's
//! secrets and environments, and the runner pools, are the application's or an operator's: those services need
//! authorization disabled, or the caller's own service authorization.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
use moqui_core::rust_service;

use super::file::Event;
use super::queue::{self, RunRequest};
use super::{
    engine, field, field_value, int, is_yes, long, millis, opt, yes_no, CI_ENVIRONMENT, CI_SECRET, JOB_RUN, JOB_STEP,
    LOG_CHUNK, PIPELINE_RUN, RUNNER_POOL, RUNNER_USAGE, RUN_ARTIFACT, TEST_FAILURE,
};
use crate::values::map;
use crate::GitProvider;

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, LibError>;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, queue_push_runs)?;
    rust_service!(map, queue_merge_request_runs)?;
    rust_service!(map, queue_scheduled_runs)?;
    rust_service!(map, start_queued_job_runs)?;
    rust_service!(map, end_overdue_job_runs)?;
    rust_service!(map, expire_run_data)?;
    rust_service!(map, find_pipeline_runs)?;
    rust_service!(map, get_pipeline_run)?;
    rust_service!(map, get_job_run)?;
    rust_service!(map, get_job_log)?;
    rust_service!(map, get_run_artifact)?;
    rust_service!(map, run_pipeline)?;
    rust_service!(map, rerun_pipeline_run)?;
    rust_service!(map, cancel_pipeline_run)?;
    rust_service!(map, retry_job_run)?;
    rust_service!(map, approve_pipeline_run)?;
    rust_service!(map, find_runner_pools)?;
    rust_service!(map, store_runner_pool)?;
    rust_service!(map, delete_runner_pool)?;
    rust_service!(map, create_runner_pool_token)?;
    rust_service!(map, find_ci_secrets)?;
    rust_service!(map, store_ci_secret)?;
    rust_service!(map, delete_ci_secret)?;
    rust_service!(map, find_ci_environments)?;
    rust_service!(map, store_ci_environment)?;
    rust_service!(map, delete_ci_environment)?;
    rust_service!(map, get_runner_usage)?;
    rust_service!(map, delete_repository_runs)?;
    Ok(())
}

fn none() -> Out {
    Ok(BTreeMap::new())
}

fn out(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn text(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

fn flag(parameters: &Parameters, key: &str) -> bool {
    text(parameters, key).is_some_and(|value| matches!(value.as_str(), "true" | "Y" | "y"))
}

fn number(parameters: &Parameters, key: &str, default: i64) -> i64 {
    text(parameters, key)
        .and_then(|text| text.split('.').next().and_then(|whole| whole.parse().ok()))
        .unwrap_or(default)
}

fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

fn current_user() -> Option<String> {
    moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
}

fn authz_disabled() -> bool {
    ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
}

fn provider() -> Option<GitProvider> {
    let provider = ExecutionContext::with_current(|ec| crate::provider(ec.factory())).flatten();
    if provider.is_none() {
        add_error("git resources are not installed (resource-reference scheme=\"git\" in the conf)");
    }
    provider
}

/// Whether the caller may do `action` on the repository: with authorization disabled, or by its rights.
fn may(repository_id: &str, action: &str) -> bool {
    authz_disabled() || crate::server::permitted(repository_id, action)
}

/// The error of a repository the caller may not act on: not found when it may not view it either.
fn refuse(repository_id: &str, what: &str) {
    if may(repository_id, "view") {
        add_error(format!("{what} needs the update right on repository {repository_id}"));
    } else {
        add_error(format!("repository {repository_id} not found"));
    }
}

/// Whether the caller reads `run`: it may view the repository, and a hidden run only with authorization disabled and
/// `includeHidden`, or as the user it was made for.
fn readable(run: &EntityValue, parameters: &Parameters) -> bool {
    let repository = field(run, "repositoryId").unwrap_or_default();
    if !may(&repository, "view") {
        return false;
    }
    if !is_yes(run, "hidden") {
        return true;
    }
    (authz_disabled() && flag(parameters, "includeHidden"))
        || (current_user().is_some() && current_user() == field(run, "triggeredByUserId"))
}

/// The run a service is asked about, by `pipelineRunId` or by `repositoryId` and `runNum`, when the caller reads it;
/// otherwise the error recorded, the same for one that is not there.
fn run_of(facade: &EntityFacade, parameters: &Parameters) -> Result<Option<EntityValue>, LibError> {
    let found = {
        let _rows = crate::rows_guard();
        match (
            text(parameters, "pipelineRunId"),
            text(parameters, "repositoryId"),
            text(parameters, "runNum"),
        ) {
            (Some(id), _, _) => super::one(facade, PIPELINE_RUN, "pipelineRunId", &id)?,
            (None, Some(repository), Some(_)) => facade
                .find(PIPELINE_RUN)?
                .eq("repositoryId", Value::string(repository))
                .eq("runNum", int(number(parameters, "runNum", 0)))
                .one()?,
            _ => {
                add_error("pipelineRunId, or repositoryId and runNum, is required");
                return Ok(None);
            }
        }
    };
    match found.filter(|run| readable(run, parameters)) {
        Some(run) => Ok(Some(run)),
        None => {
            add_error("no such pipeline run");
            Ok(None)
        }
    }
}

/// The job run `jobRunId` and its run, when the caller reads the run.
fn job_of(facade: &EntityFacade, parameters: &Parameters) -> Result<Option<(EntityValue, EntityValue)>, LibError> {
    let Some(id) = text(parameters, "jobRunId") else {
        add_error("jobRunId is required");
        return Ok(None);
    };
    let found = {
        let _rows = crate::rows_guard();
        match super::one(facade, JOB_RUN, "jobRunId", &id)? {
            Some(job) => field(&job, "pipelineRunId")
                .map(|run| super::one(facade, PIPELINE_RUN, "pipelineRunId", &run))
                .transpose()?
                .flatten()
                .map(|run| (job, run)),
            None => None,
        }
    };
    match found.filter(|(_, run)| readable(run, parameters)) {
        Some(found) => Ok(Some(found)),
        None => {
            add_error("no such job run");
            Ok(None)
        }
    }
}

fn seconds_between(row: &EntityValue, from: &str, thru: &str) -> Value {
    match (millis(row, from), millis(row, thru)) {
        (Some(from), Some(thru)) => int(((thru - from) / 1000).max(0)),
        (Some(from), None) => int(((super::now_millis() - from) / 1000).max(0)),
        _ => Value::Null,
    }
}

fn run_value(run: &EntityValue) -> Value {
    let copy = |name: &'static str| (name, field_value(run, name));
    map([
        copy("pipelineRunId"),
        copy("repositoryId"),
        copy("runNum"),
        copy("pipelineName"),
        copy("definitionSource"),
        copy("event"),
        copy("refName"),
        copy("commitId"),
        copy("mergeRequestId"),
        copy("sourceRepositoryId"),
        copy("triggeredByUserId"),
        copy("ownerId"),
        ("trusted", Value::Bool(is_yes(run, "trusted"))),
        ("protectedRef", Value::Bool(is_yes(run, "protectedRef"))),
        ("hidden", Value::Bool(is_yes(run, "hidden"))),
        copy("approvalState"),
        copy("approvedByUserId"),
        copy("statusId"),
        copy("statusMessage"),
        copy("environmentId"),
        copy("retryOfRunId"),
        copy("createdDate"),
        copy("startedDate"),
        copy("finishedDate"),
        ("durationSeconds", seconds_between(run, "startedDate", "finishedDate")),
    ])
}

fn job_value(run: &EntityValue, job: &EntityValue) -> Value {
    let copy = |name: &'static str| (name, field_value(job, name));
    map([
        copy("jobRunId"),
        copy("pipelineRunId"),
        copy("jobName"),
        copy("matrixKey"),
        ("attempt", int(long(job, "attempt").unwrap_or(1))),
        ("superseded", Value::Bool(is_yes(job, "superseded"))),
        (
            "needs",
            Value::List(
                field(job, "needs")
                    .unwrap_or_default()
                    .split_whitespace()
                    .map(Value::string)
                    .collect(),
            ),
        ),
        (
            "context",
            Value::string(super::plan::context(
                &field(run, "pipelineName").unwrap_or_default(),
                &field(job, "jobName").unwrap_or_default(),
                &field(job, "matrixKey").unwrap_or_default(),
            )),
        ),
        copy("poolName"),
        copy("runnerPoolId"),
        copy("runnerId"),
        copy("image"),
        copy("statusId"),
        copy("statusMessage"),
        copy("exitCode"),
        copy("timeoutMinutes"),
        copy("logBytes"),
        ("hasLog", Value::Bool(long(job, "logBytes").unwrap_or(0) > 0)),
        copy("minutesUsed"),
        copy("testCount"),
        copy("testFailedCount"),
        copy("commitStatusId"),
        copy("queuedDate"),
        copy("startedDate"),
        copy("finishedDate"),
        ("durationSeconds", seconds_between(job, "startedDate", "finishedDate")),
    ])
}

// ---- what the rules and the jobs call ----

/// `queue#PushRuns`: the post-receive gitECA rule. Queue the push pipelines of each branch that moved and the tag
/// pipelines of each tag created, at their new commits, as the user who pushed, and keep the schedules of the
/// default branch. Out: `runList` (pipelineRunId, runNum, pipelineName, statusId).
fn queue_push_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        return none();
    };
    if !queue::enabled(facade) {
        return none();
    }
    let Some(provider) = provider() else { return none() };
    let user = text(parameters, "userId").or_else(current_user);
    let mut runs = Vec::new();
    let updates = match parameters.get("refUpdates") {
        Some(Value::List(updates)) => updates.clone(),
        _ => Vec::new(),
    };
    let mut default_moved = false;
    for update in &updates {
        let Value::Map(update) = update else { continue };
        let get = |name: &str| update.get(name).filter(|value| !is_empty(value)).map(to_plain_string);
        let (Some(ref_name), Some(new_id)) = (get("refName"), get("newId")) else {
            continue;
        };
        default_moved |= get("isDefaultBranch").as_deref() == Some("true");
        let event = if ref_name.starts_with("refs/heads/") {
            Event::Push
        } else if ref_name.starts_with("refs/tags/") && get("changeType").as_deref() == Some("create") {
            Event::Tag
        } else {
            continue;
        };
        // one ref that fails does not stop the others
        let queued = (|| -> Result<Vec<queue::Queued>, moqui_core::context::Error> {
            let Some(commit) = provider.resolve_commit(&repository, &new_id)? else {
                return Ok(Vec::new());
            };
            let mut request = RunRequest::new(&repository, event, &ref_name, &commit.to_string());
            request.user_id.clone_from(&user);
            queue::queue(&provider, facade, &request)
        })();
        match queued {
            Ok(queued) => runs.extend(queued),
            Err(err) => {
                tracing::error!(repository = %repository, ref_name = %ref_name, "pipeline runs were not queued: {err}");
            }
        }
    }
    if default_moved {
        if let Err(err) = queue::refresh_schedules(&provider, facade, &repository) {
            tracing::error!(repository = %repository, "the pipeline schedules were not refreshed: {err}");
        }
    }
    Ok(out([("runList", queued_list(&runs))]))
}

fn queued_list(runs: &[queue::Queued]) -> Value {
    Value::List(
        runs.iter()
            .map(|run| {
                map([
                    ("pipelineRunId", Value::string(&run.pipeline_run_id)),
                    ("runNum", int(run.run_num)),
                    ("pipelineName", Value::string(&run.pipeline_name)),
                    ("statusId", Value::string(&run.status)),
                ])
            })
            .collect(),
    )
}

/// `queue#MergeRequestRuns`: after `queue#MergeRequestEvent` (a service ECA rule). Queue the merge-request pipelines
/// of a merge request that was opened, reopened, made ready or whose head moved, at its head, for its target branch.
/// Out: `runList`.
fn queue_merge_request_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(id), Some(action)) = (text(parameters, "mergeRequestId"), text(parameters, "action")) else {
        return none();
    };
    if !matches!(action.as_str(), "opened" | "synchronize" | "reopened" | "ready") || !queue::enabled(facade) {
        return none();
    }
    let Some(provider) = provider() else { return none() };
    let Some(row) = queue::merge_request(facade, &id)? else {
        return none();
    };
    let Some(request) = queue::merge_request_run(&row, current_user()) else {
        return none();
    };
    match queue::queue(&provider, facade, &request) {
        Ok(runs) => Ok(out([("runList", queued_list(&runs))])),
        Err(err) => {
            // the merge request's own change stands
            tracing::error!(merge_request = %id, "pipeline runs were not queued: {err}");
            none()
        }
    }
}

/// `queue#ScheduledRuns`: the schedule job. Out: `queuedCount`.
fn queue_scheduled_runs(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(provider) = provider() else { return none() };
    let count = queue::queue_due(&provider, facade, super::now_millis())?;
    Ok(out([("queuedCount", int(i64::try_from(count).unwrap_or(0)))]))
}

/// `start#QueuedJobRuns`: the start job. Out: `startedCount`, `refusedCount`.
fn start_queued_job_runs(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let done = engine::start_queued(facade)?;
    Ok(out([
        ("startedCount", int(i64::try_from(done.started).unwrap_or(0))),
        ("refusedCount", int(i64::try_from(done.refused).unwrap_or(0))),
    ]))
}

/// `end#OverdueJobRuns`: the job that ends runs past their time and removes what they left. Out: `timedOutCount`,
/// `lostCount`, `unclaimedCount`, `removedCount`.
fn end_overdue_job_runs(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let ended = engine::end_overdue(facade, super::now_millis())?;
    Ok(out([
        ("timedOutCount", int(i64::try_from(ended.timed_out).unwrap_or(0))),
        ("lostCount", int(i64::try_from(ended.lost).unwrap_or(0))),
        ("unclaimedCount", int(i64::try_from(ended.unclaimed).unwrap_or(0))),
        ("removedCount", int(ended.removed)),
    ]))
}

/// `expire#RunData`: the daily retention. Out: `artifactCount`, `logCount`, `chunkJobCount`.
fn expire_run_data(_parameters: &Parameters, facade: &EntityFacade) -> Out {
    let expired = super::artifact::expire(facade, super::now_millis())?;
    Ok(out([
        ("artifactCount", int(expired.artifacts)),
        ("logCount", int(expired.logs)),
        ("chunkJobCount", int(expired.chunks)),
    ]))
}

// ---- reads ----

/// `find#PipelineRuns`: the runs of a repository, newest first. Out: `runList`, `runCount`.
fn find_pipeline_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&repository, "view") {
        add_error(format!("repository {repository} not found"));
        return none();
    }
    let hidden_too = authz_disabled() && flag(parameters, "includeHidden");
    let user = current_user();
    let _rows = crate::rows_guard();
    let mut find = facade
        .find(PIPELINE_RUN)?
        .eq("repositoryId", Value::string(&repository));
    for name in [
        "refName",
        "event",
        "statusId",
        "mergeRequestId",
        "commitId",
        "pipelineName",
    ] {
        if let Some(value) = text(parameters, name) {
            find = find.eq(name, Value::string(value));
        }
    }
    let rows = find.order_by("runNum", true).limit(5000).list()?;
    let visible: Vec<&EntityValue> = rows
        .iter()
        .filter(|run| {
            !is_yes(run, "hidden") || hidden_too || (user.is_some() && user == field(run, "triggeredByUserId"))
        })
        .collect();
    let size = usize::try_from(number(parameters, "pageSize", 20).clamp(1, 200)).unwrap_or(20);
    let index = usize::try_from(number(parameters, "pageIndex", 0).max(0)).unwrap_or(0);
    let page: Vec<Value> = visible
        .iter()
        .skip(index * size)
        .take(size)
        .map(|run| run_value(run))
        .collect();
    Ok(out([
        ("runList", Value::List(page)),
        ("runCount", int(i64::try_from(visible.len()).unwrap_or(0))),
    ]))
}

/// `get#PipelineRun`: a run with its jobs, in the order `needs` gives. Out: `run`, `jobList`, `canUpdate`.
fn get_pipeline_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(run) = run_of(facade, parameters)? else {
        return none();
    };
    let run_id = field(&run, "pipelineRunId").unwrap_or_default();
    let repository = field(&run, "repositoryId").unwrap_or_default();
    let can_update = may(&repository, "update");
    let _rows = crate::rows_guard();
    let rows = facade
        .find(JOB_RUN)?
        .eq("pipelineRunId", Value::string(&run_id))
        .order_by("jobRunId", false)
        .list()?;
    let all = flag(parameters, "includeSuperseded");
    let mut jobs: Vec<&EntityValue> = rows.iter().filter(|job| all || !is_yes(job, "superseded")).collect();
    // jobs that need nothing first, then those whose needs are placed
    let mut ordered: Vec<&EntityValue> = Vec::new();
    while !jobs.is_empty() {
        let placed: Vec<String> = ordered.iter().filter_map(|job| field(job, "jobName")).collect();
        let (ready, rest): (Vec<&EntityValue>, Vec<&EntityValue>) = jobs.into_iter().partition(|job| {
            field(job, "needs")
                .unwrap_or_default()
                .split_whitespace()
                .all(|need| placed.iter().any(|name| name == need))
        });
        if ready.is_empty() {
            ordered.extend(rest);
            break;
        }
        ordered.extend(ready);
        jobs = rest;
    }
    Ok(out([
        ("run", run_value(&run)),
        (
            "jobList",
            Value::List(ordered.iter().map(|job| job_value(&run, job)).collect()),
        ),
        ("canUpdate", Value::Bool(can_update)),
    ]))
}

/// `get#JobRun`: a job with its steps, its failed tests and its artifacts and reports (not its caches). Out: `run`,
/// `job`, `stepList`, `failedTestList`, `artifactList`.
fn get_job_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((job, run)) = job_of(facade, parameters)? else {
        return none();
    };
    let id = field(&job, "jobRunId").unwrap_or_default();
    let _rows = crate::rows_guard();
    let steps: Vec<Value> = facade
        .find(JOB_STEP)?
        .eq("jobRunId", Value::string(&id))
        .order_by("stepSeq", false)
        .list()?
        .iter()
        .map(|step| {
            let copy = |name: &'static str| (name, field_value(step, name));
            map([
                copy("stepSeq"),
                copy("stepName"),
                copy("command"),
                copy("statusId"),
                copy("exitCode"),
                copy("firstChunkSeq"),
                copy("startedDate"),
                copy("finishedDate"),
                ("durationSeconds", seconds_between(step, "startedDate", "finishedDate")),
            ])
        })
        .collect();
    let failures: Vec<Value> = facade
        .find(TEST_FAILURE)?
        .eq("jobRunId", Value::string(&id))
        .order_by("testSeq", false)
        .list()?
        .iter()
        .map(|failure| {
            let copy = |name: &'static str| (name, field_value(failure, name));
            map([
                copy("testSeq"),
                copy("suiteName"),
                copy("testName"),
                copy("filePath"),
                copy("lineNumber"),
                copy("message"),
                copy("durationMillis"),
            ])
        })
        .collect();
    let artifacts: Vec<Value> = facade
        .find(RUN_ARTIFACT)?
        .eq("jobRunId", Value::string(&id))
        .order_by("artifactId", false)
        .list()?
        .iter()
        .filter(|artifact| field(artifact, "artifactKind").as_deref() != Some("cache"))
        .map(artifact_value)
        .collect();
    Ok(out([
        ("run", run_value(&run)),
        ("job", job_value(&run, &job)),
        ("stepList", Value::List(steps)),
        ("failedTestList", Value::List(failures)),
        ("artifactList", Value::List(artifacts)),
    ]))
}

fn artifact_value(artifact: &EntityValue) -> Value {
    let copy = |name: &'static str| (name, field_value(artifact, name));
    map([
        copy("artifactId"),
        copy("jobRunId"),
        copy("artifactKind"),
        copy("name"),
        copy("path"),
        copy("sizeBytes"),
        copy("sha256"),
        ("keep", Value::Bool(is_yes(artifact, "keep"))),
        copy("expireDate"),
        copy("createdDate"),
        (
            "fileName",
            Value::string(match field(artifact, "artifactKind").as_deref() {
                Some("artifact") => format!("{}.tar.gz", field(artifact, "name").unwrap_or_default()),
                _ => field(artifact, "name").unwrap_or_default(),
            }),
        ),
    ])
}

/// `get#JobLog`: a piece of a job's log. While the job runs: the chunks after `afterChunkSeq`, at most `limit`
/// (100). Once it has ended (`finished`): the stored log from `offset`, at most `maxBytes` (256 KB), as `text`. A
/// page that tails passes back `nextChunkSeq` and `nextOffset` each time, and with `watch` is told of new lines on
/// the notification topic `GitCiJobLog`. Out: `chunkList` (chunkSeq, stepSeq, text), `nextChunkSeq`, `text`,
/// `nextOffset`, `logBytes`, `finished`, `statusId`.
fn get_job_log(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((job, _run)) = job_of(facade, parameters)? else {
        return none();
    };
    let limit = usize::try_from(number(parameters, "limit", 100).clamp(1, 1000)).unwrap_or(100);
    let max_bytes =
        usize::try_from(number(parameters, "maxBytes", 256 * 1024).clamp(1, 4 * 1024 * 1024)).unwrap_or(256 * 1024);
    let read = super::log::read(
        facade,
        &job,
        number(parameters, "afterChunkSeq", 0),
        limit,
        number(parameters, "offset", 0),
        max_bytes,
    )?;
    if flag(parameters, "watch") && !read.finished {
        super::log::watch(&field(&job, "jobRunId").unwrap_or_default());
    }
    let chunks: Vec<Value> = read
        .chunks
        .into_iter()
        .map(|(seq, step, text)| {
            map([
                ("chunkSeq", int(seq)),
                ("stepSeq", step.map_or(Value::Null, int)),
                ("text", Value::string(text)),
            ])
        })
        .collect();
    Ok(out([
        ("chunkList", Value::List(chunks)),
        ("nextChunkSeq", int(read.next_chunk_seq)),
        ("text", Value::string(read.text)),
        ("nextOffset", int(read.next_offset)),
        ("logBytes", int(read.log_bytes)),
        ("finished", Value::Bool(read.finished)),
        ("statusId", field_value(&job, "statusId")),
    ]))
}

/// `get#RunArtifact`: an artifact or a report of a run, and a piece of its content: `maxBytes` (1 MB, at most 8 MB)
/// from `offset`, so a download is read in pieces. A cache is not found. Out: `artifact`, `bytes`, `nextOffset`,
/// `done`.
fn get_run_artifact(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(id) = text(parameters, "artifactId") else {
        add_error("artifactId is required");
        return none();
    };
    let found = {
        let _rows = crate::rows_guard();
        let artifact = super::one(facade, RUN_ARTIFACT, "artifactId", &id)?
            .filter(|artifact| field(artifact, "artifactKind").as_deref() != Some("cache"));
        match artifact {
            Some(artifact) => field(&artifact, "pipelineRunId")
                .map(|run| super::one(facade, PIPELINE_RUN, "pipelineRunId", &run))
                .transpose()?
                .flatten()
                .map(|run| (artifact, run)),
            None => None,
        }
    };
    let Some((artifact, _run)) = found.filter(|(_, run)| readable(run, parameters)) else {
        add_error("no such artifact");
        return none();
    };
    let Some((blobs, info)) = super::artifact::content(facade, &artifact)? else {
        add_error("the artifact's content is gone");
        return none();
    };
    let offset = u64::try_from(number(parameters, "offset", 0).max(0))
        .unwrap_or(0)
        .min(info.size);
    let max =
        usize::try_from(number(parameters, "maxBytes", 1024 * 1024).clamp(1, 8 * 1024 * 1024)).unwrap_or(1024 * 1024);
    let bytes = {
        let _rows = crate::rows_guard();
        blobs.read_range(&info, offset, max)?
    };
    let next = offset + bytes.len() as u64;
    Ok(out([
        ("artifact", artifact_value(&artifact)),
        ("bytes", Value::Bytes(bytes)),
        ("nextOffset", int(i64::try_from(next).unwrap_or(i64::MAX))),
        ("done", Value::Bool(next >= info.size)),
    ]))
}

// ---- moves ----

/// `run#Pipeline`: run a pipeline that has the manual event, for a branch or a tag, as the caller. Needs the update
/// right. Out: `pipelineRunId`, `runNum`, `statusId`.
fn run_pipeline(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(repository), Some(pipeline)) = (text(parameters, "repositoryId"), text(parameters, "pipelineName"))
    else {
        add_error("repositoryId and pipelineName are required");
        return none();
    };
    if !may(&repository, "update") {
        refuse(&repository, "running a pipeline");
        return none();
    }
    let Some(provider) = provider() else { return none() };
    let repo = provider.repo(&repository)?;
    let ref_name = match text(parameters, "refName") {
        Some(name) if name.starts_with("refs/") => name,
        Some(name) => crate::ops::branch_ref(&name),
        None => crate::ops::branch_ref(&repo.definition.branch),
    };
    if !(ref_name.starts_with("refs/heads/") || ref_name.starts_with("refs/tags/")) {
        add_error("a pipeline runs for a branch or a tag");
        return none();
    }
    let Some(commit) = provider.resolve_commit(&repository, &ref_name)? else {
        add_error(format!("repository {repository} has no {ref_name}"));
        return none();
    };
    let mut request = RunRequest::new(&repository, Event::Manual, &ref_name, &commit.to_string());
    request.user_id = current_user();
    request.pipeline_name = Some(pipeline.clone());
    if !queue::enabled(facade) {
        add_error("pipelines are not on here: there is no enabled runner pool");
        return none();
    }
    let runs = queue::queue(&provider, facade, &request)?;
    let Some(run) = runs.first() else {
        add_error(format!("{ref_name} has no pipeline {pipeline} with the manual event"));
        return none();
    };
    Ok(out([
        ("pipelineRunId", Value::string(&run.pipeline_run_id)),
        ("runNum", int(run.run_num)),
        ("statusId", Value::string(&run.status)),
    ]))
}

/// The run a move is asked for, when the caller has the update right on its repository (or `own` and asked for the
/// run itself).
fn run_to_change(
    facade: &EntityFacade,
    parameters: &Parameters,
    what: &str,
    own: bool,
) -> Result<Option<EntityValue>, LibError> {
    let Some(run) = run_of(facade, parameters)? else {
        return Ok(None);
    };
    let repository = field(&run, "repositoryId").unwrap_or_default();
    let mine = own && current_user().is_some() && current_user() == field(&run, "triggeredByUserId");
    if !mine && !may(&repository, "update") {
        refuse(&repository, what);
        return Ok(None);
    }
    Ok(Some(run))
}

/// `rerun#PipelineRun`: a new run of the same pipeline for the same commit and event, as the caller: the file is
/// read again, and the run is trusted or not as the first was decided (a fork's merge request stays untrusted).
/// Needs the update right. Out: `pipelineRunId`, `runNum`, `statusId`.
fn rerun_pipeline_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(run) = run_to_change(facade, parameters, "running a pipeline again", false)? else {
        return none();
    };
    let Some(provider) = provider() else { return none() };
    let Some(event) = field(&run, "event").and_then(|event| Event::parse(&event)) else {
        add_error("the run has no event");
        return none();
    };
    let mut request = RunRequest::new(
        &field(&run, "repositoryId").unwrap_or_default(),
        event,
        &field(&run, "refName").unwrap_or_default(),
        &field(&run, "commitId").unwrap_or_default(),
    );
    request.user_id = current_user();
    request.pipeline_name = field(&run, "pipelineName");
    request.merge_request_id = field(&run, "mergeRequestId");
    request.source_repository_id = field(&run, "sourceRepositoryId");
    request.hidden = is_yes(&run, "hidden");
    request.environment_id = field(&run, "environmentId");
    request.retry_of_run_id = field(&run, "pipelineRunId");
    // a run that was not trusted for where its commits came from is not made trusted by who runs it again
    if let Some(row) = request
        .merge_request_id
        .as_deref()
        .map(|id| queue::merge_request(facade, id))
        .transpose()?
        .flatten()
    {
        request.pushed_for_review = field(&row, "pushedForReview").as_deref() == Some("Y");
    } else if request.merge_request_id.is_some() && !is_yes(&run, "trusted") {
        request.pushed_for_review = true;
    }
    let runs = queue::queue(&provider, facade, &request)?;
    let Some(next) = runs.first() else {
        add_error("the commit no longer has that pipeline");
        return none();
    };
    Ok(out([
        ("pipelineRunId", Value::string(&next.pipeline_run_id)),
        ("runNum", int(next.run_num)),
        ("statusId", Value::string(&next.status)),
    ]))
}

/// `cancel#PipelineRun`: cancel a run that has not ended. Needs the update right, or to be the user who asked for
/// the run. Out: `statusId`.
fn cancel_pipeline_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(run) = run_to_change(facade, parameters, "cancelling a pipeline run", true)? else {
        return none();
    };
    match engine::cancel_run(facade, &field(&run, "pipelineRunId").unwrap_or_default()) {
        Ok(()) => Ok(out([("statusId", Value::string("cancelled"))])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `retry#JobRun`: run an ended job again in its run. Needs the update right. Out: `jobRunId` (the new attempt),
/// `statusId`.
fn retry_job_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((job, run)) = job_of(facade, parameters)? else {
        return none();
    };
    let repository = field(&run, "repositoryId").unwrap_or_default();
    if !may(&repository, "update") {
        refuse(&repository, "running a job again");
        return none();
    }
    match engine::retry_job(facade, &field(&job, "jobRunId").unwrap_or_default()) {
        Ok(id) => Ok(out([
            ("jobRunId", Value::string(id)),
            ("statusId", Value::string("queued")),
        ])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `approve#PipelineRun`: let a run that waits for approval start. Needs the update right on the repository, or
/// authorization disabled (the application decides who approves). Out: `statusId`, `approvalState`.
fn approve_pipeline_run(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(run) = run_to_change(facade, parameters, "approving a pipeline run", false)? else {
        return none();
    };
    match engine::approve_run(
        facade,
        &field(&run, "pipelineRunId").unwrap_or_default(),
        current_user(),
    ) {
        Ok(()) => Ok(out([
            ("statusId", Value::string("queued")),
            ("approvalState", Value::string("approved")),
        ])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

// ---- pools ----

fn pool_value(pool: &EntityValue) -> Value {
    let copy = |name: &'static str| (name, field_value(pool, name));
    map([
        copy("runnerPoolId"),
        copy("poolName"),
        copy("description"),
        copy("ownerId"),
        copy("instanceHostId"),
        copy("poolKind"),
        ("trusted", Value::Bool(is_yes(pool, "trusted"))),
        ("enabled", Value::Bool(is_yes(pool, "enabled"))),
        copy("maxConcurrent"),
        copy("allowedImages"),
        copy("cpuLimit"),
        copy("memoryMb"),
        copy("diskMb"),
        copy("pidsLimit"),
        copy("maxMinutes"),
        copy("egressRule"),
        copy("serverUrl"),
        (
            "hasRegistrationToken",
            Value::Bool(field(pool, "registrationTokenHash").is_some()),
        ),
    ])
}

/// `find#RunnerPools`: the pools, with the jobs running and queued in each. Out: `poolList`.
fn find_runner_pools(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let _rows = crate::rows_guard();
    let mut find = facade.find(RUNNER_POOL)?;
    if let Some(owner) = text(parameters, "ownerId") {
        find = find.eq("ownerId", Value::string(owner));
    }
    let mut pools = Vec::new();
    for pool in find.order_by("poolName", false).list()?.iter() {
        let id = field(pool, "runnerPoolId").unwrap_or_default();
        let count = |status: &str| -> Result<i64, LibError> {
            facade
                .find(JOB_RUN)?
                .eq("statusId", Value::string(status))
                .eq("runnerPoolId", Value::string(&id))
                .count()
        };
        let Value::Map(mut entries) = pool_value(pool) else {
            continue;
        };
        entries.insert("runningCount".to_string(), int(count("running")?));
        entries.insert("queuedCount".to_string(), int(count("queued")?));
        pools.push(Value::Map(entries));
    }
    Ok(out([("poolList", Value::List(pools))]))
}

/// `store#RunnerPool`: make or change a pool. Out: `runnerPoolId`.
fn store_runner_pool(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let _rows = crate::rows_guard();
    let existing = match text(parameters, "runnerPoolId") {
        Some(id) => super::one(facade, RUNNER_POOL, "runnerPoolId", &id)?,
        None => None,
    };
    let creating = existing.is_none();
    let mut pool = match existing {
        Some(pool) => pool,
        None => {
            let mut pool = EntityValue::make(facade, RUNNER_POOL)?;
            match text(parameters, "runnerPoolId") {
                Some(id) => pool.set("runnerPoolId", Value::string(id)),
                None => pool.set_sequenced_id_primary(facade)?,
            }
            pool.set("poolName", Value::string("default"));
            pool.set("poolKind", Value::string("container"));
            pool.set("trusted", Value::string("Y"));
            pool.set("enabled", Value::string("Y"));
            pool.set("maxConcurrent", int(1));
            pool
        }
    };
    for name in [
        "poolName",
        "description",
        "ownerId",
        "instanceHostId",
        "allowedImages",
        "cpuLimit",
        "egressRule",
        "serverUrl",
    ] {
        if parameters.contains_key(name) {
            pool.set(name, opt(text(parameters, name)));
        }
    }
    for name in ["maxConcurrent", "memoryMb", "diskMb", "pidsLimit", "maxMinutes"] {
        if parameters.contains_key(name) {
            pool.set(
                name,
                text(parameters, name).map_or(Value::Null, |_| int(number(parameters, name, 0))),
            );
        }
    }
    for name in ["trusted", "enabled"] {
        if parameters.contains_key(name) {
            pool.set(name, yes_no(flag(parameters, name)));
        }
    }
    if let Some(kind) = text(parameters, "poolKind") {
        if !matches!(kind.as_str(), "container" | "registered") {
            add_error("poolKind is container or registered");
            return none();
        }
        pool.set("poolKind", Value::string(kind));
    }
    if field(&pool, "poolName").is_none_or(|name| !super::file::valid_name(&name)) {
        add_error("poolName must be letters, digits, dot, dash and underscore");
        return none();
    }
    if field(&pool, "poolKind").as_deref() == Some("container") && field(&pool, "instanceHostId").is_none() {
        add_error("a pool of containers needs an instanceHostId");
        return none();
    }
    if creating {
        pool.create(facade)?;
    } else {
        pool.update(facade)?;
    }
    Ok(out([("runnerPoolId", field_value(&pool, "runnerPoolId"))]))
}

/// `delete#RunnerPool`: remove a pool with no running job; its queued jobs are then errors when they would start.
fn delete_runner_pool(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(id) = text(parameters, "runnerPoolId") else {
        add_error("runnerPoolId is required");
        return none();
    };
    let _rows = crate::rows_guard();
    let running = facade
        .find(JOB_RUN)?
        .eq("statusId", Value::string("running"))
        .eq("runnerPoolId", Value::string(&id))
        .count()?;
    if running > 0 {
        add_error(format!("runner pool {id} has {running} running jobs"));
        return none();
    }
    facade
        .find(super::RUNNER)?
        .eq("runnerPoolId", Value::string(&id))
        .delete_all()?;
    facade
        .find(RUNNER_POOL)?
        .eq("runnerPoolId", Value::string(&id))
        .delete_all()?;
    none()
}

/// `create#RunnerPoolToken`: a new registration token for a pool of registered runners, replacing the one it had.
/// Out: `token`, shown once.
fn create_runner_pool_token(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(id) = text(parameters, "runnerPoolId") else {
        add_error("runnerPoolId is required");
        return none();
    };
    match super::registered::new_pool_token(facade, &id) {
        Ok(token) => Ok(out([("token", Value::string(token))])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

// ---- secrets and environments ----

/// Whose rows a secret or environment service is for: a repository's (the caller needs the update right), or an
/// owner's (the caller's authorization is disabled). `None` with the error recorded.
fn scope_of(parameters: &Parameters, what: &str) -> Option<(Option<String>, Option<String>)> {
    match (text(parameters, "repositoryId"), text(parameters, "ownerId")) {
        (Some(repository), None) => {
            if may(&repository, "update") {
                Some((Some(repository), None))
            } else {
                refuse(&repository, what);
                None
            }
        }
        (None, Some(owner)) => {
            if authz_disabled() {
                Some((None, Some(owner)))
            } else {
                add_error(format!(
                    "{what} of an owner is the application's (authorization disabled)"
                ));
                None
            }
        }
        _ => {
            add_error("one of repositoryId and ownerId is required");
            None
        }
    }
}

fn scoped(
    facade: &EntityFacade,
    entity_name: &str,
    scope: &(Option<String>, Option<String>),
) -> Result<moqui_core::entity::EntityFind, LibError> {
    let find = facade.find(entity_name)?;
    Ok(match scope {
        (Some(repository), _) => find.eq("repositoryId", Value::string(repository)),
        (None, Some(owner)) => find
            .eq("ownerId", Value::string(owner))
            .condition(Condition::is_null("repositoryId")),
        (None, None) => find,
    })
}

/// `find#CiSecrets`: the secrets of a repository or an owner, without their values. Out: `secretList`.
fn find_ci_secrets(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "reading the secrets") else {
        return none();
    };
    let _rows = crate::rows_guard();
    let environments: BTreeMap<String, String> = scoped(facade, CI_ENVIRONMENT, &scope)?
        .list()?
        .iter()
        .filter_map(|row| Some((field(row, "environmentId")?, field(row, "name")?)))
        .collect();
    let secrets: Vec<Value> = scoped(facade, CI_SECRET, &scope)?
        .order_by("name", false)
        .list()?
        .iter()
        .map(|secret| {
            let copy = |name: &'static str| (name, field_value(secret, name));
            map([
                copy("secretId"),
                copy("name"),
                copy("scope"),
                copy("environmentId"),
                (
                    "environmentName",
                    opt(field(secret, "environmentId").and_then(|id| environments.get(&id).cloned())),
                ),
                copy("createdByUserId"),
                copy("createdDate"),
                copy("lastUsedDate"),
            ])
        })
        .collect();
    Ok(out([("secretList", Value::List(secrets))]))
}

/// `store#CiSecret`: make a secret, or change one (`secretId`): its `value` is written and never read back; left
/// out on a change, the value stays. `scope` is protected (the default), tags or environment (with
/// `environmentId`). Out: `secretId`.
fn store_ci_secret(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "writing a secret") else {
        return none();
    };
    let _rows = crate::rows_guard();
    let existing = match text(parameters, "secretId") {
        Some(id) => {
            let found = scoped(facade, CI_SECRET, &scope)?
                .eq("secretId", Value::string(&id))
                .one()?;
            if found.is_none() {
                add_error(format!("no secret {id}"));
                return none();
            }
            found
        }
        None => None,
    };
    let name = text(parameters, "name").or_else(|| existing.as_ref().and_then(|row| field(row, "name")));
    let Some(name) = name.filter(|name| {
        name.len() <= 128
            && name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            && !name.starts_with(|ch: char| ch.is_ascii_digit())
    }) else {
        add_error("name is required: letters, digits and underscore, as an environment variable's");
        return none();
    };
    let secret_scope = text(parameters, "scope")
        .or_else(|| existing.as_ref().and_then(|row| field(row, "scope")))
        .unwrap_or_else(|| "protected".to_string());
    if !matches!(secret_scope.as_str(), "protected" | "tags" | "environment") {
        add_error("scope is protected, tags or environment");
        return none();
    }
    let environment = if secret_scope == "environment" {
        let id =
            text(parameters, "environmentId").or_else(|| existing.as_ref().and_then(|row| field(row, "environmentId")));
        let known = match &id {
            Some(id) => scoped(facade, CI_ENVIRONMENT, &scope)?
                .eq("environmentId", Value::string(id))
                .one()?
                .is_some(),
            None => false,
        };
        if !known {
            add_error("a secret of the environment scope needs an environmentId of the same repository or owner");
            return none();
        }
        id
    } else {
        None
    };
    let own_id = existing.as_ref().and_then(|row| field(row, "secretId"));
    let taken = scoped(facade, CI_SECRET, &scope)?
        .eq("name", Value::string(&name))
        .list()?
        .iter()
        .any(|row| field(row, "secretId") != own_id && field(row, "environmentId") == environment);
    if taken {
        add_error(format!("a secret named {name} is already there"));
        return none();
    }
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let Some(_) = text(parameters, "value") else {
                add_error("value is required");
                return none();
            };
            let mut row = EntityValue::make(facade, CI_SECRET)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("repositoryId", opt(scope.0.clone()));
            row.set("ownerId", opt(scope.1.clone()));
            row.set("createdByUserId", opt(current_user()));
            row.set("createdDate", moqui_core::user::now_value());
            row
        }
    };
    row.set("name", Value::string(&name));
    row.set("scope", Value::string(&secret_scope));
    row.set("environmentId", opt(environment));
    if let Some(value) = parameters
        .get("value")
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
    {
        if value.len() > 64 * 1024 {
            add_error("a secret's value is at most 64 KB");
            return none();
        }
        row.set("secretValue", Value::string(value));
    }
    if creating {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    Ok(out([("secretId", field_value(&row, "secretId"))]))
}

/// `delete#CiSecret`. Out: `deleted`.
fn delete_ci_secret(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "deleting a secret") else {
        return none();
    };
    let Some(id) = text(parameters, "secretId") else {
        add_error("secretId is required");
        return none();
    };
    let _rows = crate::rows_guard();
    let deleted = scoped(facade, CI_SECRET, &scope)?
        .eq("secretId", Value::string(id))
        .delete_all()?;
    Ok(out([("deleted", Value::Bool(deleted > 0))]))
}

/// `find#CiEnvironments`: the environments of a repository (the caller views it) or an owner. Out:
/// `environmentList`, without the target's token.
fn find_ci_environments(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let scope = match (text(parameters, "repositoryId"), text(parameters, "ownerId")) {
        (Some(repository), None) if may(&repository, "view") => (Some(repository), None),
        (Some(repository), None) => {
            add_error(format!("repository {repository} not found"));
            return none();
        }
        _ => match scope_of(parameters, "reading the environments") {
            Some(scope) => scope,
            None => return none(),
        },
    };
    let _rows = crate::rows_guard();
    let environments: Vec<Value> = scoped(facade, CI_ENVIRONMENT, &scope)?
        .order_by("name", false)
        .list()?
        .iter()
        .map(|row| {
            let copy = |name: &'static str| (name, field_value(row, name));
            map([
                copy("environmentId"),
                copy("name"),
                copy("targetUrl"),
                copy("requiredPipeline"),
                ("approvalRequired", Value::Bool(is_yes(row, "approvalRequired"))),
                ("hasToken", Value::Bool(field(row, "targetToken").is_some())),
                copy("lastCommitId"),
                copy("lastDeployDate"),
            ])
        })
        .collect();
    Ok(out([("environmentList", Value::List(environments))]))
}

/// `store#CiEnvironment`: make or change an environment. The target's `token` is written and never read back. Out:
/// `environmentId`.
fn store_ci_environment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "writing an environment") else {
        return none();
    };
    let _rows = crate::rows_guard();
    let existing = match text(parameters, "environmentId") {
        Some(id) => {
            let found = scoped(facade, CI_ENVIRONMENT, &scope)?
                .eq("environmentId", Value::string(&id))
                .one()?;
            if found.is_none() {
                add_error(format!("no environment {id}"));
                return none();
            }
            found
        }
        None => None,
    };
    let name = text(parameters, "name").or_else(|| existing.as_ref().and_then(|row| field(row, "name")));
    let Some(name) = name.filter(|name| super::file::valid_name(name)) else {
        add_error("name is required: letters, digits, dot, dash and underscore");
        return none();
    };
    let own_id = existing.as_ref().and_then(|row| field(row, "environmentId"));
    let taken = scoped(facade, CI_ENVIRONMENT, &scope)?
        .eq("name", Value::string(&name))
        .list()?
        .iter()
        .any(|row| field(row, "environmentId") != own_id);
    if taken {
        add_error(format!("an environment named {name} is already there"));
        return none();
    }
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = EntityValue::make(facade, CI_ENVIRONMENT)?;
            row.set_sequenced_id_primary(facade)?;
            row.set("repositoryId", opt(scope.0.clone()));
            row.set("ownerId", opt(scope.1.clone()));
            row
        }
    };
    row.set("name", Value::string(&name));
    for field_name in ["targetUrl", "requiredPipeline"] {
        if parameters.contains_key(field_name) {
            row.set(field_name, opt(text(parameters, field_name)));
        }
    }
    if parameters.contains_key("approvalRequired") {
        row.set("approvalRequired", yes_no(flag(parameters, "approvalRequired")));
    }
    if let Some(token) = text(parameters, "token") {
        row.set("targetToken", Value::string(token));
    }
    if creating {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    Ok(out([("environmentId", field_value(&row, "environmentId"))]))
}

/// `delete#CiEnvironment`: refused while a secret is scoped to it. Out: `deleted`.
fn delete_ci_environment(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(scope) = scope_of(parameters, "deleting an environment") else {
        return none();
    };
    let Some(id) = text(parameters, "environmentId") else {
        add_error("environmentId is required");
        return none();
    };
    let _rows = crate::rows_guard();
    let secrets = facade
        .find(CI_SECRET)?
        .eq("environmentId", Value::string(&id))
        .count()?;
    if secrets > 0 {
        add_error(format!("environment {id} has {secrets} secrets scoped to it"));
        return none();
    }
    let deleted = scoped(facade, CI_ENVIRONMENT, &scope)?
        .eq("environmentId", Value::string(id))
        .delete_all()?;
    Ok(out([("deleted", Value::Bool(deleted > 0))]))
}

// ---- usage, and a repository that goes ----

/// `get#RunnerUsage`: the runner time of a repository (the caller views it) or of an owner (authorization disabled)
/// in a month (`periodKey`, yyyy-MM; this month when empty), with what is running now. Out: `periodKey`,
/// `minutesUsed`, `secondsUsed`, `jobCount`, `untrustedMinutesUsed`, `runningCount`.
fn get_runner_usage(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let scope = match (text(parameters, "repositoryId"), text(parameters, "ownerId")) {
        (Some(repository), None) if may(&repository, "view") => (Some(repository), None),
        (None, Some(owner)) if authz_disabled() => (None, Some(owner)),
        (None, Some(_)) => {
            add_error("the usage of an owner is the application's (authorization disabled)");
            return none();
        }
        (Some(repository), None) => {
            add_error(format!("repository {repository} not found"));
            return none();
        }
        _ => {
            add_error("one of repositoryId and ownerId is required");
            return none();
        }
    };
    let period = text(parameters, "periodKey").unwrap_or_else(|| jiff::Timestamp::now().strftime("%Y-%m").to_string());
    let _rows = crate::rows_guard();
    let mut find = facade.find(RUNNER_USAGE)?.eq("periodKey", Value::string(&period));
    find = match &scope {
        (Some(repository), _) => find.eq("repositoryId", Value::string(repository)),
        (None, Some(owner)) => find.eq("ownerId", Value::string(owner)),
        (None, None) => find,
    };
    let rows = find.list()?;
    let (mut minutes, mut seconds, mut untrusted) = (0, 0, 0);
    for row in rows.iter() {
        minutes += long(row, "minutesUsed").unwrap_or(0);
        seconds += long(row, "secondsUsed").unwrap_or(0);
        if !is_yes(row, "trusted") {
            untrusted += long(row, "minutesUsed").unwrap_or(0);
        }
    }
    let running = match &scope {
        (Some(repository), _) => facade
            .find(JOB_RUN)?
            .eq("statusId", Value::string("running"))
            .eq("repositoryId", Value::string(repository))
            .count()?,
        _ => 0,
    };
    Ok(out([
        ("periodKey", Value::string(period)),
        ("minutesUsed", int(minutes)),
        ("secondsUsed", int(seconds)),
        ("untrustedMinutesUsed", int(untrusted)),
        ("jobCount", int(i64::try_from(rows.len()).unwrap_or(0))),
        ("runningCount", int(running)),
    ]))
}

/// `delete#RepositoryRuns`: remove every run of a repository with its jobs, steps, logs, artifacts, failures,
/// secrets, environments and schedules, for an application that deletes the repository. Running jobs are cancelled
/// first. Needs the delete right, or authorization disabled. Out: `runCount`.
fn delete_repository_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&repository, "delete") {
        add_error(format!("repository {repository} not found"));
        return none();
    }
    let _rows = crate::rows_guard();
    let runs = facade
        .find(PIPELINE_RUN)?
        .eq("repositoryId", Value::string(&repository))
        .list()?;
    for run in runs.iter() {
        let Some(run_id) = field(run, "pipelineRunId") else {
            continue;
        };
        if field(run, "statusId").is_some_and(|status| !super::ENDED.contains(&status.as_str())) {
            if let Err(err) = engine::cancel_run(facade, &run_id) {
                tracing::warn!(run = %run_id, "a run of a repository that goes was not cancelled: {err}");
            }
        }
        for job in facade
            .find(JOB_RUN)?
            .eq("pipelineRunId", Value::string(&run_id))
            .list()?
            .iter()
        {
            let Some(job_id) = field(job, "jobRunId") else { continue };
            for entity_name in [JOB_STEP, LOG_CHUNK, TEST_FAILURE, RUN_ARTIFACT, RUNNER_USAGE] {
                facade
                    .find(entity_name)?
                    .eq("jobRunId", Value::string(&job_id))
                    .delete_all()?;
            }
            job.delete(facade)?;
        }
        run.delete(facade)?;
    }
    for entity_name in [
        RUN_ARTIFACT,
        CI_SECRET,
        super::DEPLOYMENT,
        CI_ENVIRONMENT,
        queue::SCHEDULE,
    ] {
        facade
            .find(entity_name)?
            .eq("repositoryId", Value::string(&repository))
            .delete_all()?;
    }
    Ok(out([("runCount", int(i64::try_from(runs.len()).unwrap_or(0)))]))
}
