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

//! Queueing runs (plans/Dogfood.md §8.2, docs/interfaces/Pipelines.md "Events" and "Trust and approval"). An event of a
//! repository (a push, a tag, a merge request opened or with a new head, a schedule, a request by hand, a deploy)
//! reads the pipeline file at the event's commit, and each pipeline that applies becomes a `GitPipelineRun` with its
//! `GitJobRun` rows and their pending statuses. Before a run is queued the application's check is asked (the conf's
//! `git-ci` `run-check-service`): it may refuse the run or hold it (a quota), say it may start without approval, lower
//! its trust, and name the owner its secrets and usage count under. With no answer a trusted run is queued and an
//! untrusted one waits for `approve#PipelineRun`.

use std::sync::Mutex;

use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};

use super::file::{CiFile, Event, Pipeline};
use super::plan::PlannedJob;
use super::{
    builtin, field, int, opt, parameters, plan, status, trust, yes_no, JOB_RUN, JOB_STEP, MERGE_REQUEST, PIPELINE_RUN,
};
use crate::GitProvider;

pub(crate) const SCHEDULE: &str = "moqui.resource.GitPipelineSchedule";

/// What to run for.
#[derive(Clone, Debug)]
pub struct RunRequest {
    pub repository_id: String,
    pub event: Event,
    /// The full ref: the branch of a push, a schedule or a request by hand, the tag, the target branch of a merge request.
    pub ref_name: String,
    pub commit_id: String,
    pub user_id: Option<String>,
    pub merge_request_id: Option<String>,
    pub source_repository_id: Option<String>,
    pub pushed_for_review: bool,
    pub hidden: bool,
    /// Only this pipeline (a schedule, a request by hand, a deploy); every pipeline that applies when `None`.
    pub pipeline_name: Option<String>,
    pub environment_id: Option<String>,
    pub retry_of_run_id: Option<String>,
}

impl RunRequest {
    pub fn new(repository_id: &str, event: Event, ref_name: &str, commit_id: &str) -> Self {
        Self {
            repository_id: repository_id.to_string(),
            event,
            ref_name: ref_name.to_string(),
            commit_id: commit_id.to_string(),
            user_id: None,
            merge_request_id: None,
            source_repository_id: None,
            pushed_for_review: false,
            hidden: false,
            pipeline_name: None,
            environment_id: None,
            retry_of_run_id: None,
        }
    }
}

/// A run that was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub pipeline_run_id: String,
    pub run_num: i64,
    pub pipeline_name: String,
    /// queued, waiting or error.
    pub status: String,
}

/// The pipelines of a commit.
pub enum Definition {
    File(CiFile),
    /// The built-in pipeline of a component repository.
    Builtin(Pipeline),
    /// A `ci.xml` that is not valid.
    Invalid(String),
    None,
}

/// Whether the engine queues runs here: the conf's `git-ci` `enabled`, and when that is empty, whether there is an
/// enabled runner pool, so a server that hosts repositories and runs no CI leaves no pending statuses.
pub fn enabled(entity: &EntityFacade) -> bool {
    match super::setting("enabled").as_str() {
        "true" => true,
        "false" => false,
        _ => {
            let _rows = crate::rows_guard();
            entity
                .find(super::RUNNER_POOL)
                .and_then(|find| find.eq("enabled", Value::string("Y")).limit(1).list())
                .is_ok_and(|rows| !rows.is_empty())
        }
    }
}

/// A file of the repository's root at `commit_id`, as text.
///
/// The commit is the event's own (a pushed ref's, a merge request's head), so the read is the engine's and not a
/// user's by commit id.
fn read_at(provider: &GitProvider, repository_id: &str, commit_id: &str, name: &str) -> Result<Option<String>, Error> {
    Ok(provider
        .blob_at(repository_id, commit_id, &[name.to_string()])?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
}

/// The pipelines of `repository_id` at `commit_id`: its `ci.xml`, else the built-in pipeline when its root has a
/// `component.xml`.
pub fn definition(provider: &GitProvider, repository_id: &str, commit_id: &str) -> Result<Definition, Error> {
    let limits = super::limits();
    if let Some(text) = read_at(provider, repository_id, commit_id, "ci.xml")? {
        return Ok(match super::file::parse(&text, &limits) {
            Ok(file) => Definition::File(file),
            Err(err) => Definition::Invalid(err.to_string()),
        });
    }
    let Some(component) =
        read_at(provider, repository_id, commit_id, "component.xml")?.and_then(|text| builtin::component(&text))
    else {
        return Ok(Definition::None);
    };
    if super::setting("builtin-enabled") == "false" {
        return Ok(Definition::None);
    }
    let image = Some(super::setting("builtin-image"))
        .filter(|image| !image.is_empty())
        .unwrap_or_else(|| "moqui-build".to_string());
    let pool = Some(super::setting("builtin-pool")).filter(|pool| !pool.is_empty());
    let workers = u32::try_from(super::number_setting("builtin-workers", 2)).unwrap_or(2);
    Ok(Definition::Builtin(builtin::pipeline(
        &component, &image, pool, workers, &limits,
    )))
}

/// One at a time in this process makes a run of a repository, so two events do not take one number. Another node's
/// run is told apart by the unique index, and the number is taken again.
static NUMBERS: Mutex<()> = Mutex::new(());

fn next_run_num(entity: &EntityFacade, repository_id: &str) -> Result<i64, Error> {
    Ok(entity
        .find(PIPELINE_RUN)?
        .eq("repositoryId", Value::string(repository_id))
        .order_by("runNum", true)
        .select_field("runNum")
        .limit(1)
        .list()?
        .iter()
        .find_map(|row| super::long(row, "runNum"))
        .unwrap_or(0)
        + 1)
}

/// What the application's check said of a run.
struct Decision {
    /// allow, wait or refuse.
    decision: String,
    trusted: bool,
    message: Option<String>,
    owner_id: Option<String>,
    runner_pool_id: Option<String>,
}

/// Ask the conf's `git-ci` `run-check-service` about a run, as the LFS upload check is asked: in `repositoryId`,
/// `pipelineName`, `event`, `refName`, `commitId`, `mergeRequestId`, `sourceRepositoryId`, `pushedForReview`,
/// `triggeredByUserId`, `trusted` (the engine's answer) and `jobCount`; out `decision` (allow, wait or refuse),
/// `trusted` (false lowers the engine's; true never raises it), `message`, `ownerId` and `runnerPoolId`. With no
/// service, or no decision, a trusted run is allowed and an untrusted one waits. A service that fails refuses.
fn decide(request: &RunRequest, pipeline: &str, trusted: bool, job_count: usize) -> Decision {
    let default = |trusted: bool| if trusted { "allow" } else { "wait" }.to_string();
    let asked = super::call_hook(
        "run-check-service",
        parameters([
            ("repositoryId", Value::string(&request.repository_id)),
            ("pipelineName", Value::string(pipeline)),
            ("event", Value::string(request.event.name())),
            ("refName", Value::string(&request.ref_name)),
            ("commitId", Value::string(&request.commit_id)),
            ("mergeRequestId", opt(request.merge_request_id.clone())),
            ("sourceRepositoryId", opt(request.source_repository_id.clone())),
            ("pushedForReview", Value::Bool(request.pushed_for_review)),
            ("triggeredByUserId", opt(request.user_id.clone())),
            ("trusted", Value::Bool(trusted)),
            ("jobCount", int(i64::try_from(job_count).unwrap_or(i64::MAX))),
        ]),
    );
    match asked {
        None => Decision {
            decision: default(trusted),
            trusted,
            message: None,
            owner_id: None,
            runner_pool_id: None,
        },
        Some(Err(err)) => {
            // the errors are the check's answer, not the event's failure
            ExecutionContext::with_current(|ec| ec.message().clear_errors());
            Decision {
                decision: "refuse".to_string(),
                trusted: false,
                message: Some(err.message().to_string()),
                owner_id: None,
                runner_pool_id: None,
            }
        }
        Some(Ok(out)) => {
            let text = |name: &str| {
                out.get(name)
                    .filter(|value| !moqui_lib::object::is_empty(value))
                    .map(to_plain_string)
            };
            let lowered = text("trusted").is_some_and(|value| matches!(value.as_str(), "false" | "N"));
            let trusted = trusted && !lowered;
            let decision = match text("decision").as_deref() {
                Some("allow") => "allow".to_string(),
                Some("wait") => "wait".to_string(),
                Some("refuse") => "refuse".to_string(),
                _ => default(trusted),
            };
            Decision {
                decision,
                trusted,
                message: text("message"),
                owner_id: text("ownerId"),
                runner_pool_id: text("runnerPoolId"),
            }
        }
    }
}

/// A run of the same pipeline for the same commit, event, ref and merge request that has not ended: an event
/// delivered twice makes one run.
fn duplicate(entity: &EntityFacade, request: &RunRequest, pipeline: &str) -> Result<bool, Error> {
    if request.retry_of_run_id.is_some() || matches!(request.event, Event::Manual | Event::Schedule | Event::Deploy) {
        return Ok(false);
    }
    Ok(entity
        .find(PIPELINE_RUN)?
        .eq("repositoryId", Value::string(&request.repository_id))
        .eq("commitId", Value::string(&request.commit_id))
        .eq("pipelineName", Value::string(pipeline))
        .eq("event", Value::string(request.event.name()))
        .list()?
        .iter()
        .any(|row| {
            field(row, "refName").as_deref() == Some(request.ref_name.as_str())
                && field(row, "mergeRequestId") == request.merge_request_id
                && field(row, "statusId").is_some_and(|status| !super::ENDED.contains(&status.as_str()))
        }))
}

struct NewRun<'a> {
    pipeline: &'a str,
    source: &'a str,
    trusted: bool,
    protected: bool,
    status: &'a str,
    approval: &'a str,
    message: Option<String>,
    owner_id: Option<String>,
}

fn create_run(entity: &EntityFacade, request: &RunRequest, new: &NewRun<'_>) -> Result<EntityValue, Error> {
    let _turn = moqui_lib::sync::lock(&NUMBERS);
    let mut last = None;
    for _ in 0..5 {
        let mut run = EntityValue::make(entity, PIPELINE_RUN)?;
        run.set_sequenced_id_primary(entity)?;
        run.set("repositoryId", Value::string(&request.repository_id));
        run.set("runNum", int(next_run_num(entity, &request.repository_id)?));
        run.set("pipelineName", Value::string(new.pipeline));
        run.set("definitionSource", Value::string(new.source));
        run.set("event", Value::string(request.event.name()));
        run.set("refName", Value::string(&request.ref_name));
        run.set("commitId", Value::string(&request.commit_id));
        run.set("mergeRequestId", opt(request.merge_request_id.clone()));
        run.set("sourceRepositoryId", opt(request.source_repository_id.clone()));
        run.set("triggeredByUserId", opt(request.user_id.clone()));
        run.set("ownerId", opt(new.owner_id.clone()));
        run.set("trusted", yes_no(new.trusted));
        run.set("protectedRef", yes_no(new.protected));
        run.set("hidden", yes_no(request.hidden));
        run.set("approvalState", Value::string(new.approval));
        run.set("statusId", Value::string(new.status));
        run.set("statusMessage", opt(new.message.clone()));
        run.set("environmentId", opt(request.environment_id.clone()));
        run.set("retryOfRunId", opt(request.retry_of_run_id.clone()));
        run.set("createdDate", moqui_core::user::now_value());
        if super::ENDED.contains(&new.status) {
            run.set("finishedDate", moqui_core::user::now_value());
        }
        match run.create(entity) {
            Ok(()) => return Ok(run),
            // another node took the number
            Err(err) => last = Some(err),
        }
    }
    Err(last.map_or_else(|| Error::new("a pipeline run was not created"), Error::from))
}

/// Write a planned job of `run` in `status`, with its steps. The row.
pub(crate) fn create_job(
    entity: &EntityFacade,
    run: &EntityValue,
    job: &PlannedJob,
    status: &str,
    message: Option<&str>,
    runner_pool_id: Option<&str>,
) -> Result<EntityValue, Error> {
    let mut row = EntityValue::make(entity, JOB_RUN)?;
    row.set_sequenced_id_primary(entity)?;
    let job_run_id = field(&row, "jobRunId").unwrap_or_default();
    row.set(
        "pipelineRunId",
        field(run, "pipelineRunId").map_or(Value::Null, Value::string),
    );
    row.set(
        "repositoryId",
        field(run, "repositoryId").map_or(Value::Null, Value::string),
    );
    row.set("jobName", Value::string(&job.name));
    row.set("matrixKey", Value::string(&job.matrix_key));
    row.set("needs", Value::string(job.needs.join(" ")));
    row.set("poolName", opt(job.pool.clone()));
    row.set("runnerPoolId", opt(runner_pool_id.map(str::to_string)));
    row.set("image", Value::string(&job.image));
    row.set("statusId", Value::string(status));
    row.set("statusMessage", opt(message.map(str::to_string)));
    row.set("timeoutMinutes", int(i64::from(job.timeout_minutes)));
    row.set("jobSpec", Value::string(job.spec.to_string()));
    row.set("nextChunkSeq", int(1));
    row.set("logBytes", int(0));
    if status == "queued" {
        row.set("queuedDate", moqui_core::user::now_value());
    }
    if super::ENDED.contains(&status) {
        row.set("finishedDate", moqui_core::user::now_value());
    }
    row.create(entity)?;
    for (index, (name, command)) in job.steps.iter().enumerate() {
        let mut step = EntityValue::make(entity, JOB_STEP)?;
        step.set("jobRunId", Value::string(&job_run_id));
        step.set("stepSeq", int(i64::try_from(index + 1).unwrap_or(i64::MAX)));
        step.set("stepName", opt(name.clone()));
        step.set("command", Value::string(command));
        step.set(
            "statusId",
            Value::string(if status == "error" { "skipped" } else { "pending" }),
        );
        step.create(entity)?;
    }
    Ok(row)
}

/// A run that could not be planned (a file that is not valid): an `error` run with no jobs, and one status that says
/// why, under `ci/<name>`.
fn error_run(entity: &EntityFacade, request: &RunRequest, name: &str, message: &str) -> Result<Queued, Error> {
    let run = create_run(
        entity,
        request,
        &NewRun {
            pipeline: name,
            source: "file",
            trusted: false,
            protected: false,
            status: "error",
            approval: "none",
            message: Some(message.to_string()),
            owner_id: None,
        },
    )?;
    if let Err(err) = status::write(
        entity,
        &request.repository_id,
        &request.commit_id,
        &format!("ci/{name}"),
        "error",
        message,
        status::run_url(&run, None),
    ) {
        tracing::error!(repository = %request.repository_id, "the status of a pipeline file that is not valid was not written: {err}");
    }
    Ok(Queued {
        pipeline_run_id: field(&run, "pipelineRunId").unwrap_or_default(),
        run_num: super::long(&run, "runNum").unwrap_or(0),
        pipeline_name: name.to_string(),
        status: "error".to_string(),
    })
}

fn queue_pipeline(
    entity: &EntityFacade,
    request: &RunRequest,
    pipeline: &Pipeline,
    source: &str,
) -> Result<Option<Queued>, Error> {
    if duplicate(entity, request, &pipeline.name)? {
        return Ok(None);
    }
    let jobs = plan::plan(pipeline);
    let origin = trust::Origin {
        repository_id: &request.repository_id,
        user_id: request.user_id.as_deref(),
        source_repository_id: request.source_repository_id.as_deref(),
        pushed_for_review: request.pushed_for_review,
    };
    let decided = decide(
        request,
        &pipeline.name,
        trust::is_trusted(request.event, &origin),
        jobs.len(),
    );
    let protected =
        decided.trusted && trust::is_protected(entity, &request.repository_id, request.event, &request.ref_name)?;
    let (run_status, approval, description) = match decided.decision.as_str() {
        "allow" => ("queued", "none", "Queued".to_string()),
        "wait" => (
            "waiting",
            "waiting",
            decided
                .message
                .clone()
                .unwrap_or_else(|| "Waiting for approval".to_string()),
        ),
        _ => (
            "error",
            "none",
            decided
                .message
                .clone()
                .unwrap_or_else(|| "The run was refused".to_string()),
        ),
    };
    let run = create_run(
        entity,
        request,
        &NewRun {
            pipeline: &pipeline.name,
            source,
            trusted: decided.trusted,
            protected,
            status: run_status,
            approval,
            message: decided.message.clone().filter(|_| run_status != "queued"),
            owner_id: decided.owner_id.clone(),
        },
    )?;
    for job in &jobs {
        let job_status = match run_status {
            "queued" if job.needs.is_empty() => "queued",
            "error" => "error",
            _ => "waiting",
        };
        let message = (job_status == "error").then_some(description.as_str());
        let row = create_job(
            entity,
            &run,
            job,
            job_status,
            message,
            decided.runner_pool_id.as_deref(),
        )?;
        status::write_job(entity, &run, &row, &description);
    }
    Ok(Some(Queued {
        pipeline_run_id: field(&run, "pipelineRunId").unwrap_or_default(),
        run_num: super::long(&run, "runNum").unwrap_or(0),
        pipeline_name: pipeline.name.clone(),
        status: run_status.to_string(),
    }))
}

/// Queue the runs of an event: each pipeline of the commit's file that applies. Nothing when the engine is off here,
/// or the commit has no pipelines. A file that is not valid makes one `error` run that says why.
pub fn queue(provider: &GitProvider, entity: &EntityFacade, request: &RunRequest) -> Result<Vec<Queued>, Error> {
    if !enabled(entity) {
        return Ok(Vec::new());
    }
    let _rows = crate::rows_guard();
    let (pipelines, source) = match definition(provider, &request.repository_id, &request.commit_id)? {
        Definition::None => return Ok(Vec::new()),
        Definition::Invalid(message) => {
            // an invalid file is told once per commit and event
            if duplicate_error(entity, request)? {
                return Ok(Vec::new());
            }
            return Ok(vec![error_run(entity, request, "ci.xml", &message)?]);
        }
        Definition::File(file) => (file.pipelines, "file"),
        Definition::Builtin(pipeline) => (vec![pipeline], "builtin"),
    };
    let mut queued = Vec::new();
    for pipeline in &pipelines {
        let wanted = match &request.pipeline_name {
            Some(name) => name == &pipeline.name && pipeline.events.contains(&request.event),
            None => plan::applies(pipeline, request.event, &request.ref_name),
        };
        if !wanted {
            continue;
        }
        // one pipeline that fails to queue does not stop the others
        match queue_pipeline(entity, request, pipeline, source) {
            Ok(Some(run)) => queued.push(run),
            Ok(None) => {}
            Err(err) => {
                tracing::error!(repository = %request.repository_id, pipeline = %pipeline.name, "a pipeline run was not queued: {err}");
            }
        }
    }
    if queued.iter().any(|run| run.status == "queued") {
        super::engine::kick();
    }
    Ok(queued)
}

fn duplicate_error(entity: &EntityFacade, request: &RunRequest) -> Result<bool, Error> {
    Ok(entity
        .find(PIPELINE_RUN)?
        .eq("repositoryId", Value::string(&request.repository_id))
        .eq("commitId", Value::string(&request.commit_id))
        .eq("pipelineName", Value::string("ci.xml"))
        .eq("event", Value::string(request.event.name()))
        .list()?
        .iter()
        .any(|row| field(row, "mergeRequestId") == request.merge_request_id))
}

/// The pipelines of a repository's default branch that run on schedule, kept as `GitPipelineSchedule` rows so the
/// schedule job reads no file: written again whenever the default branch moves.
pub fn refresh_schedules(provider: &GitProvider, entity: &EntityFacade, repository_id: &str) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let repo = provider.repo(repository_id)?;
    let branch_ref = crate::ops::branch_ref(&repo.definition.branch);
    let scheduled: Vec<(String, String, String)> = match repo.store.read_ref(&branch_ref)? {
        Some(head) => match definition(provider, repository_id, &head.to_string())? {
            Definition::File(file) => file
                .pipelines
                .iter()
                .filter_map(|pipeline| {
                    let cron = pipeline.cron.clone()?;
                    // the first branch it names that is one, else the default branch
                    let branch = pipeline
                        .branches
                        .iter()
                        .find(|branch| !branch.contains('*'))
                        .map_or_else(|| branch_ref.clone(), |branch| crate::ops::branch_ref(branch));
                    Some((pipeline.name.clone(), cron, branch))
                })
                .collect(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let existing = entity
        .find(SCHEDULE)?
        .eq("repositoryId", Value::string(repository_id))
        .list()?;
    for row in existing.iter() {
        let name = field(row, "pipelineName").unwrap_or_default();
        if !scheduled.iter().any(|(pipeline, _, _)| pipeline == &name) {
            row.delete(entity)?;
        }
    }
    for (name, cron, branch) in scheduled {
        let known = existing
            .iter()
            .find(|row| field(row, "pipelineName").as_deref() == Some(name.as_str()));
        match known {
            Some(row) if field(row, "cron").as_deref() == Some(cron.as_str()) => {
                if field(row, "refName").as_deref() != Some(branch.as_str()) {
                    let mut changed = row.clone();
                    changed.set("refName", Value::string(&branch));
                    changed.update(entity)?;
                }
            }
            Some(row) => {
                let mut changed = row.clone();
                changed.set("cron", Value::string(&cron));
                changed.set("refName", Value::string(&branch));
                changed.set("lastCheckDate", moqui_core::user::now_value());
                changed.update(entity)?;
            }
            None => {
                let mut row = EntityValue::make(entity, SCHEDULE)?;
                row.set("repositoryId", Value::string(repository_id));
                row.set("pipelineName", Value::string(&name));
                row.set("cron", Value::string(&cron));
                row.set("refName", Value::string(&branch));
                row.set("lastCheckDate", moqui_core::user::now_value());
                row.create(entity)?;
            }
        }
    }
    Ok(())
}

/// Queue the schedule pipelines that came due since they were last checked, as of `now` (milliseconds). The count
/// queued. One that fails is logged and the others still run.
pub fn queue_due(provider: &GitProvider, entity: &EntityFacade, now: i64) -> Result<usize, Error> {
    if !enabled(entity) {
        return Ok(0);
    }
    let _rows = crate::rows_guard();
    let mut count = 0;
    for row in entity.find(SCHEDULE)?.list()?.iter() {
        let (Some(repository), Some(pipeline), Some(cron), Some(ref_name)) = (
            field(row, "repositoryId"),
            field(row, "pipelineName"),
            field(row, "cron"),
            field(row, "refName"),
        ) else {
            continue;
        };
        let last = super::millis(row, "lastCheckDate").unwrap_or(now);
        let due = match moqui_core::service::info::next_cron_execution(&cron, last) {
            Ok(next) => next.is_some_and(|next| next <= now),
            Err(err) => {
                tracing::error!(repository = %repository, pipeline = %pipeline, "a pipeline's cron does not parse: {err}");
                false
            }
        };
        if !due {
            continue;
        }
        let mut checked = row.clone();
        checked.set("lastCheckDate", moqui_core::user::stamp_value(now));
        if let Err(err) = checked.update(entity) {
            tracing::error!(repository = %repository, pipeline = %pipeline, "a pipeline schedule was not marked: {err}");
            continue;
        }
        let outcome = (|| -> Result<usize, Error> {
            let repo = provider.repo(&repository)?;
            let Some(head) = repo.store.read_ref(&ref_name)? else {
                return Ok(0);
            };
            let mut request = RunRequest::new(&repository, Event::Schedule, &ref_name, &head.to_string());
            request.pipeline_name = Some(pipeline.clone());
            Ok(queue(provider, entity, &request)?.len())
        })();
        match outcome {
            Ok(made) => count += made,
            Err(err) => {
                tracing::error!(repository = %repository, pipeline = %pipeline, "a scheduled pipeline was not queued: {err}");
            }
        }
    }
    Ok(count)
}

/// The request of a merge request's run, from its row: the head commit, for the target branch. `None` when it has no
/// head or is not open.
pub fn merge_request_run(row: &EntityValue, user_id: Option<String>) -> Option<RunRequest> {
    if !matches!(field(row, "statusId").as_deref(), Some("GmrDraft" | "GmrOpen")) {
        return None;
    }
    let repository = field(row, "repositoryId")?;
    let head = field(row, "headCommitId")?;
    let target = field(row, "targetRef")?;
    let mut request = RunRequest::new(
        &repository,
        Event::MergeRequest,
        &crate::ops::branch_ref(&target),
        &head,
    );
    request.merge_request_id = field(row, "mergeRequestId");
    // the fields of forks and push to review (plans/Dogfood.md F4, F5), where the merge request has them
    request.source_repository_id = field(row, "sourceRepositoryId").filter(|source| source != &repository);
    request.pushed_for_review = field(row, "pushedForReview").as_deref() == Some("Y");
    request.hidden = field(row, "hiddenDate").is_some();
    request.user_id = user_id.or_else(|| field(row, "authorUserId"));
    Some(request)
}

/// The merge request `id`.
pub(crate) fn merge_request(entity: &EntityFacade, id: &str) -> Result<Option<EntityValue>, Error> {
    let _rows = crate::rows_guard();
    super::one(entity, MERGE_REQUEST, "mergeRequestId", id)
}
