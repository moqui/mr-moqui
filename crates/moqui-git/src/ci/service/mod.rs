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
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::rust_service;

use super::file::Event;
use super::queue::{self, RunRequest};
use super::{
    engine, field, field_value, int, is_yes, long, millis, JOB_RUN, JOB_STEP, PIPELINE_RUN, RUNNER_USAGE, RUN_ARTIFACT,
    TEST_FAILURE,
};
use crate::values::map;
use crate::GitProvider;

mod pools;
mod secrets;

#[cfg(test)]
pub(crate) use pools::OPERATOR_PERMISSION;

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
    rust_service!(map, send_run_artifact)?;
    rust_service!(map, get_pipeline_definitions)?;
    rust_service!(map, run_pipeline)?;
    rust_service!(map, rerun_pipeline_run)?;
    rust_service!(map, cancel_pipeline_run)?;
    rust_service!(map, retry_job_run)?;
    rust_service!(map, approve_pipeline_run)?;
    pools::register(map)?;
    secrets::register(map)?;
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
///
/// With authorization disabled only, as a rule's actions run: the user and the commits are the event's, and a caller
/// that named them would run as someone else, at a commit of its choosing. A push to `refs/for/<branch>` and the
/// head ref of a merge request are neither a branch nor a tag: their runs are the merge request's.
fn queue_push_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        return none();
    };
    if !authz_disabled() {
        add_error("queue#PushRuns is called by the framework's gitECA rule only");
        return none();
    }
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
        let event = if ref_name.starts_with("refs/heads/") {
            default_moved |= get("isDefaultBranch").as_deref() == Some("true");
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
///
/// With authorization disabled only, as the rule calls it. A fork's, a pushed-for-review and a detached merge
/// request's run is untrusted, and a hidden one's is hidden. `opened` for a head that has had its run queues none:
/// a merge request that is shown is told as opened a second time (`set#MergeRequestHidden`).
fn queue_merge_request_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let (Some(id), Some(action)) = (text(parameters, "mergeRequestId"), text(parameters, "action")) else {
        return none();
    };
    if !authz_disabled() {
        add_error("queue#MergeRequestRuns is called by the rule on queue#MergeRequestEvent only");
        return none();
    }
    if !matches!(action.as_str(), "opened" | "synchronize" | "reopened" | "ready") || !queue::enabled(facade) {
        return none();
    }
    let Some(provider) = provider() else { return none() };
    let Some(row) = queue::merge_request(facade, &id)? else {
        return none();
    };
    let Some(mut request) = queue::merge_request_run(&row, current_user()) else {
        return none();
    };
    request.once_per_head = action == "opened";
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
    let Some((artifact, blobs, info)) = readable_artifact(parameters, facade)? else {
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

/// `send#RunArtifact`: send an artifact or a report of a run as this request's response, streamed from its content
/// blob a window at a time (with `Range`, `ETag` and `If-Range`), as `ec.web.sendResourceResponse` sends a file; for a
/// screen's download transition, so the application copies nothing to a file. Read as `get#RunArtifact` is (the
/// view right on the repository). Out: `artifact`.
fn send_run_artifact(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some((artifact, blobs, info)) = readable_artifact(parameters, facade)? else {
        return none();
    };
    let value = artifact_value(&artifact);
    let file_name = match &value {
        Value::Map(entries) => entries.get("fileName").map(moqui_lib::object::to_plain_string),
        _ => None,
    }
    .unwrap_or_else(|| "artifact".to_string());
    let opened = moqui_core::context::Opened {
        stat: moqui_core::context::ResourceStat {
            is_file: true,
            size: info.size,
            last_modified: None,
        },
        source: blobs.source(&info),
    };
    if let Err(err) = moqui_core::web::send_file_response(&opened, &file_name, flag(parameters, "inline")) {
        add_error(format!("the artifact's content cannot be read: {}", err.message()));
        return none();
    }
    Ok(out([("artifact", value)]))
}

/// The artifact or report `artifactId` names, with its content, when the caller reads its run; else the error
/// recorded (the same for one that is not there) and `None`. A cache is not found.
fn readable_artifact(
    parameters: &Parameters,
    facade: &EntityFacade,
) -> Result<
    Option<(
        EntityValue,
        moqui_core::resource::blob::BlobStore,
        moqui_core::resource::blob::BlobInfo,
    )>,
    LibError,
> {
    let Some(id) = text(parameters, "artifactId") else {
        add_error("artifactId is required");
        return Ok(None);
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
        return Ok(None);
    };
    let Some((blobs, info)) = super::artifact::content(facade, &artifact)? else {
        add_error("the artifact's content is gone");
        return Ok(None);
    };
    Ok(Some((artifact, blobs, info)))
}

/// `get#PipelineDefinitions`: the pipelines a commit defines, as the engine reads them (its `ci.xml`, else the built-in
/// pipeline of a component), so an application need not parse the file: for each `pipelineName`, `events`, `manual`
/// (it has the manual event, so `run#Pipeline` runs it), `branches`, `tags`, `cron` and `jobNames`. `revision` is a
/// branch, a tag, a ref or a commit (the default branch when empty). Needs the view right. Out: `commitId`, `source`
/// (file, builtin or none; invalid with `errorMessage` for a file that is not valid), `pipelineList`.
fn get_pipeline_definitions(parameters: &Parameters, _facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&repository, "view") {
        refuse(&repository, "reading its pipelines");
        return none();
    }
    let Some(provider) = provider() else { return none() };
    let revision = text(parameters, "revision").unwrap_or_default();
    let Some(commit) = provider.resolve_commit(&repository, &revision)? else {
        add_error(format!("repository {repository} has no revision {revision}"));
        return none();
    };
    let commit = commit.to_string();
    let (source, error, pipelines) = match queue::definition(&provider, &repository, &commit)? {
        queue::Definition::File(file) => ("file", None, file.pipelines),
        queue::Definition::Builtin(pipeline) => ("builtin", None, vec![pipeline]),
        queue::Definition::Invalid(message) => ("invalid", Some(message), Vec::new()),
        queue::Definition::None => ("none", None, Vec::new()),
    };
    let strings = |items: &[String]| Value::List(items.iter().map(Value::string).collect());
    let list = pipelines
        .iter()
        .map(|pipeline| {
            let mut job_names: Vec<String> = Vec::new();
            for job in &pipeline.jobs {
                if !job_names.contains(&job.name) {
                    job_names.push(job.name.clone());
                }
            }
            map([
                ("pipelineName", Value::string(&pipeline.name)),
                (
                    "events",
                    Value::List(
                        pipeline
                            .events
                            .iter()
                            .map(|event| Value::string(event.name()))
                            .collect(),
                    ),
                ),
                ("manual", Value::Bool(pipeline.events.contains(&Event::Manual))),
                ("branches", strings(&pipeline.branches)),
                ("tags", strings(&pipeline.tags)),
                ("cron", pipeline.cron.as_deref().map_or(Value::Null, Value::string)),
                ("jobNames", strings(&job_names)),
            ])
        })
        .collect();
    Ok(out([
        ("commitId", Value::string(&commit)),
        ("source", Value::string(source)),
        ("errorMessage", error.map_or(Value::Null, Value::string)),
        ("pipelineList", Value::List(list)),
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
    // a run that was not trusted for where its commits came from is not made trusted by who runs it again, and a
    // merge request's run is hidden while its merge request is
    if let Some(row) = request
        .merge_request_id
        .as_deref()
        .map(|id| queue::merge_request(facade, id))
        .transpose()?
        .flatten()
    {
        request.pushed_for_review = is_yes(&row, "pushedForReview");
        request.source_detached = field(&row, "sourceDetachedDate").is_some();
        request.hidden = field(&row, "hiddenDate").is_some();
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
    // the application's check is asked again: a run it would refuse or hold now is not run again
    if let Err(why) = super::queue::check_again(&run) {
        add_error(why);
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
    // a run with a job in an environment that requires approval: a maintainer's (the delete right), and never the
    // approval of who triggered it, the pusher of its commit (plans/Dogfood.md §8.4, §8.6)
    let repository = field(&run, "repositoryId").unwrap_or_default();
    let specs: Vec<serde_json::Value> =
        engine::current_jobs(facade, &field(&run, "pipelineRunId").unwrap_or_default())?
            .iter()
            .filter_map(|job| serde_json::from_str(&field(job, "jobSpec").unwrap_or_default()).ok())
            .collect();
    let environments =
        super::trust::approval_environments(facade, &repository, field(&run, "ownerId").as_deref(), specs.iter())?;
    if !environments.is_empty() {
        if !may(&repository, "delete") {
            add_error(format!(
                "approving a run in environment {} needs the delete right on repository {repository} (a maintainer)",
                environments.join(", ")
            ));
            return none();
        }
        if current_user().is_some() && current_user() == field(&run, "triggeredByUserId") {
            add_error(format!(
                "a run in environment {} is approved by another than who triggered it",
                environments.join(", ")
            ));
            return none();
        }
    }
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
    // what is stored now, for a quota: artifacts and reports, and caches
    let (artifact_bytes, cache_bytes) = match &scope {
        (Some(repository), _) => super::artifact::stored_by_kind(facade, "repositoryId", repository)?,
        (None, Some(owner)) => super::artifact::stored_by_kind(facade, "ownerId", owner)?,
        (None, None) => (0, 0),
    };
    Ok(out([
        ("periodKey", Value::string(period)),
        ("minutesUsed", int(minutes)),
        ("secondsUsed", int(seconds)),
        ("untrustedMinutesUsed", int(untrusted)),
        ("jobCount", int(i64::try_from(rows.len()).unwrap_or(0))),
        ("runningCount", int(running)),
        ("artifactBytes", int(artifact_bytes)),
        ("cacheBytes", int(cache_bytes)),
    ]))
}

/// `delete#RepositoryRuns`: remove every run of a repository with its jobs, steps, logs, artifacts, failures,
/// secrets, environments and schedules, and stop its running jobs: what `delete#GitRepository` does of itself, for
/// an application that clears a repository's pipelines and keeps the repository. Needs the delete right, or
/// authorization disabled. Out: `runCount`.
fn delete_repository_runs(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let Some(repository) = text(parameters, "repositoryId") else {
        add_error("repositoryId is required");
        return none();
    };
    if !may(&repository, "delete") {
        add_error(format!("repository {repository} not found"));
        return none();
    }
    let runs = super::delete_repository_rows(facade, &repository)?;
    Ok(out([("runCount", int(i64::try_from(runs).unwrap_or(0)))]))
}
