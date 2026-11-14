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

//! The engine's moves (plans/Dogfood.md §8.2, §8.3; docs/interfaces/Pipelines.md "The engine"): start queued job runs up
//! to each pool's limit, end a job with its result, move a run on as its jobs end (`needs`), end the jobs past their
//! time and the ones whose runner is gone, cancel, approve and retry. A step never runs here: a job is a container
//! the pool's host type starts ([`super::driver`]) or a registered runner's ([`super::registered`]).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use moqui_lib::object::Value;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{Condition, EntityFacade, EntityValue};

use super::driver::{self, InstanceState, RunSpec, ServiceSpec};
use super::{
    field, int, is_yes, long, millis, now_millis, opt, set_fields, status, token, yes_no, ENDED, JOB_RUN, JOB_STEP,
    PIPELINE_RUN, RUNNER, RUNNER_POOL, RUNNER_USAGE,
};

const MINUTE: i64 = 60 * 1000;

/// One at a time in this process starts jobs, so a pool's limit is counted once.
static STARTS: Mutex<()> = Mutex::new(());

/// The name of a job run's container.
pub fn container_name(job_run_id: &str) -> String {
    format!("moqui-ci-{job_run_id}")
}

/// Start queued jobs soon, on a job of the git pool: once the caller's transaction commits, which holds the rows a
/// start reads, or at once when there is none. When the pool cannot take it, the start job does
/// (`start_QueuedJobRuns_frequent`).
pub(crate) fn kick() {
    let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) else {
        return;
    };
    if super::setting("start-inline") == "false" {
        return;
    }
    let deferred = (|| -> Result<bool, Error> {
        let Some(entity) = factory.entity() else {
            return Ok(false);
        };
        let transactions = entity.transactions_for(JOB_RUN)?;
        if !transactions.in_transaction() {
            return Ok(false);
        }
        let factory = Arc::clone(&factory);
        transactions.on_commit(move || submit_start(&factory))?;
        Ok(true)
    })()
    .unwrap_or(false);
    if !deferred {
        submit_start(&factory);
    }
}

fn submit_start(factory: &Arc<moqui_core::context::ExecutionContextFactory>) {
    let submitted = crate::server::pool(factory).try_submit(Arc::clone(factory), move || {
        let entity = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten();
        if let Some(entity) = entity {
            if let Err(err) = start_queued(&entity) {
                tracing::error!("queued job runs were not started: {err}");
            }
        }
        Ok(())
    });
    if let Err(err) = submitted {
        tracing::debug!("queued job runs left for the start job: {err}");
    }
}

/// The jobs of a run that count: the newest attempt of each.
pub(crate) fn current_jobs(entity: &EntityFacade, pipeline_run_id: &str) -> Result<Vec<EntityValue>, Error> {
    Ok(entity
        .find(JOB_RUN)?
        .eq("pipelineRunId", Value::string(pipeline_run_id))
        .order_by("jobRunId", false)
        .list()?
        .iter()
        .filter(|job| !is_yes(job, "superseded"))
        .cloned()
        .collect())
}

fn is_ended(row: &EntityValue) -> bool {
    field(row, "statusId").is_some_and(|status| ENDED.contains(&status.as_str()))
}

/// Why a job has no pool, or its pool.
enum PoolChoice {
    Pool(Box<EntityValue>),
    None(String),
}

/// The pool a job runs in: the one the application's check named, else an enabled pool of the name the job asks for
/// (`default` when it asks for none), one of the run's owner before a shared one. An untrusted run takes only a pool
/// that takes untrusted runs (`trusted` N), and a trusted run only one that does not.
fn pool_for(entity: &EntityFacade, run: &EntityValue, job: &EntityValue) -> Result<PoolChoice, Error> {
    let trusted = is_yes(run, "trusted");
    let kind = if trusted { "trusted" } else { "untrusted" };
    let fits = |pool: &EntityValue| is_yes(pool, "enabled") && is_yes(pool, "trusted") == trusted;
    if let Some(id) = field(job, "runnerPoolId") {
        return Ok(match super::one(entity, RUNNER_POOL, "runnerPoolId", &id)? {
            Some(pool) if fits(&pool) => PoolChoice::Pool(Box::new(pool)),
            Some(_) => PoolChoice::None(format!("runner pool {id} does not take {kind} runs")),
            None => PoolChoice::None(format!("no runner pool {id}")),
        });
    }
    let name = field(job, "poolName").unwrap_or_else(|| "default".to_string());
    let owner = field(run, "ownerId");
    let pools = entity
        .find(RUNNER_POOL)?
        .eq("poolName", Value::string(&name))
        .order_by("runnerPoolId", false)
        .list()?;
    let mut candidates: Vec<&EntityValue> = pools
        .iter()
        .filter(|pool| fits(pool))
        .filter(|pool| field(pool, "ownerId").is_none_or(|pool_owner| Some(&pool_owner) == owner.as_ref()))
        .collect();
    candidates.sort_by_key(|pool| field(pool, "ownerId").is_none());
    Ok(match candidates.first() {
        Some(pool) => PoolChoice::Pool(Box::new((*pool).clone())),
        None => PoolChoice::None(format!("no enabled runner pool named {name} takes {kind} runs")),
    })
}

/// Whether a pool allows `image`: one of its patterns matches. A pool with no patterns allows none.
pub fn image_allowed(pool: &EntityValue, image: &str) -> bool {
    field(pool, "allowedImages")
        .unwrap_or_default()
        .split_whitespace()
        .any(|pattern| crate::rules::glob(pattern, image))
}

/// The URL a pool's runners reach this server by: the pool's, else the conf's `git-ci` `server-url`, else the public
/// URL.
pub fn server_url(pool: &EntityValue) -> String {
    field(pool, "serverUrl")
        .or_else(|| Some(super::setting("server-url")).filter(|url| !url.is_empty()))
        .unwrap_or_else(crate::server::public_base_url)
        .trim_end_matches('/')
        .to_string()
}

/// The minutes a job may run: its own, at most its pool's and the server's.
fn time_limit(job: &EntityValue, pool: &EntityValue) -> i64 {
    let limits = super::limits();
    let mut minutes = long(job, "timeoutMinutes")
        .filter(|minutes| *minutes > 0)
        .unwrap_or(i64::from(limits.default_timeout_minutes))
        .min(i64::from(limits.max_timeout_minutes));
    if let Some(pool_max) = long(pool, "maxMinutes").filter(|max| *max > 0) {
        minutes = minutes.min(pool_max);
    }
    minutes.max(1)
}

/// Make a queued job run the claimer's: it is `running` with a new job token. The token, or `None` when another
/// worker took the job first.
pub(crate) fn claim_job(
    entity: &EntityFacade,
    run: &EntityValue,
    job: &EntityValue,
    pool: &EntityValue,
    runner_id: Option<&str>,
) -> Result<Option<String>, Error> {
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    let (plain, hashed) = token::new_token(token::JOB_PREFIX)?;
    let now = now_millis();
    let minutes = time_limit(job, pool);
    let claimed = set_fields(
        entity,
        JOB_RUN,
        "jobRunId",
        &job_run_id,
        Some("queued"),
        vec![
            ("statusId", Value::string("running")),
            ("runnerPoolId", opt(field(pool, "runnerPoolId"))),
            ("runnerId", opt(runner_id.map(str::to_string))),
            (
                "instanceHostId",
                opt(field(pool, "instanceHostId").filter(|_| runner_id.is_none())),
            ),
            ("timeoutMinutes", int(minutes)),
            ("tokenHash", Value::string(hashed)),
            ("tokenExpireDate", moqui_core::user::stamp_value(now + minutes * MINUTE)),
            ("startedDate", moqui_core::user::stamp_value(now)),
            ("lastContactDate", moqui_core::user::stamp_value(now)),
        ],
    )?;
    if !claimed {
        return Ok(None);
    }
    if let Some(run_id) = field(run, "pipelineRunId") {
        set_fields(
            entity,
            PIPELINE_RUN,
            "pipelineRunId",
            &run_id,
            Some("queued"),
            vec![
                ("statusId", Value::string("running")),
                ("startedDate", moqui_core::user::stamp_value(now)),
            ],
        )?;
    }
    if let Some(job) = super::one(entity, JOB_RUN, "jobRunId", &job_run_id)? {
        status::write_job(entity, run, &job, "Running");
    }
    Ok(Some(plain))
}

fn pairs(value: &serde_json::Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// The container of a job: the image, the server's URL and the job token in its environment, the pool's limits, and
/// the labels that tie it to the job run. The job's own variables and its secrets are not here: the runner asks for
/// them with the token.
pub fn run_spec(job: &EntityValue, pool: &EntityValue, job_token: &str) -> RunSpec {
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    let spec: serde_json::Value =
        serde_json::from_str(&field(job, "jobSpec").unwrap_or_default()).unwrap_or(serde_json::Value::Null);
    let command = Some(super::setting("runner-command"))
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| "moqui ci-run".to_string());
    RunSpec {
        name: container_name(&job_run_id),
        image: field(job, "image").unwrap_or_default(),
        command: command.split_whitespace().map(str::to_string).collect(),
        env: vec![
            ("MOQUI_CI_SERVER_URL".to_string(), server_url(pool)),
            ("MOQUI_CI_TOKEN".to_string(), job_token.to_string()),
            ("MOQUI_CI_JOB_RUN_ID".to_string(), job_run_id.clone()),
            ("CI".to_string(), "true".to_string()),
        ],
        labels: vec![
            (driver::LABEL_JOB.to_string(), job_run_id),
            (
                driver::LABEL_RUN.to_string(),
                field(job, "pipelineRunId").unwrap_or_default(),
            ),
            (driver::LABEL_SERVER.to_string(), driver::server_id()),
        ],
        services: spec["services"]
            .as_array()
            .map(|services| {
                services
                    .iter()
                    .map(|service| ServiceSpec {
                        name: service["name"].as_str().unwrap_or_default().to_string(),
                        image: service["image"].as_str().unwrap_or_default().to_string(),
                        env: pairs(&service["env"]),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        cpu_limit: field(pool, "cpuLimit").and_then(|cpus| cpus.parse().ok()),
        memory_mb: long(pool, "memoryMb"),
        disk_mb: long(pool, "diskMb"),
        pids_limit: long(pool, "pidsLimit"),
        network_mode: field(pool, "egressRule"),
    }
}

/// What a pass of [`start_queued`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Started {
    pub started: usize,
    /// Jobs ended as errors: no pool, an image the pool does not allow, a host that did not start the container.
    pub refused: usize,
}

/// Start queued job runs, oldest first, up to each pool's `maxConcurrent`. A job with no pool or an image its pool
/// does not allow is an error; a job of a pool of registered runners waits for one to ask.
pub fn start_queued(entity: &EntityFacade) -> Result<Started, Error> {
    let _turn = moqui_lib::sync::lock(&STARTS);
    let _rows = crate::rows_guard();
    let mut done = Started::default();
    let queued = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("queued"))
        .order_by("queuedDate", false)
        .order_by("jobRunId", false)
        .limit(500)
        .list()?;
    let mut running: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for job in queued.iter() {
        let job_run_id = field(job, "jobRunId").unwrap_or_default();
        // one job that fails to start does not stop the others
        let outcome = (|| -> Result<(), Error> {
            let Some(run) = field(job, "pipelineRunId")
                .map(|id| super::one(entity, PIPELINE_RUN, "pipelineRunId", &id))
                .transpose()?
                .flatten()
            else {
                return Ok(());
            };
            let pool = match pool_for(entity, &run, job)? {
                PoolChoice::Pool(pool) => *pool,
                PoolChoice::None(why) => {
                    done.refused += 1;
                    finish_job(entity, &job_run_id, &Outcome::error(why))?;
                    return Ok(());
                }
            };
            let image = field(job, "image").unwrap_or_default();
            if !image_allowed(&pool, &image) {
                done.refused += 1;
                let name = field(&pool, "poolName").unwrap_or_default();
                finish_job(
                    entity,
                    &job_run_id,
                    &Outcome::error(format!("runner pool {name} does not allow the image {image}")),
                )?;
                return Ok(());
            }
            if field(&pool, "poolKind").as_deref() == Some("registered") {
                return Ok(());
            }
            let pool_id = field(&pool, "runnerPoolId").unwrap_or_default();
            let count = match running.get(&pool_id) {
                Some(count) => *count,
                None => entity
                    .find(JOB_RUN)?
                    .eq("statusId", Value::string("running"))
                    .eq("runnerPoolId", Value::string(&pool_id))
                    .count()?,
            };
            if count >= long(&pool, "maxConcurrent").unwrap_or(1).max(1) {
                running.insert(pool_id, count);
                return Ok(());
            }
            let Some(host_id) = field(&pool, "instanceHostId") else {
                done.refused += 1;
                finish_job(
                    entity,
                    &job_run_id,
                    &Outcome::error(format!("runner pool {pool_id} has no instance host")),
                )?;
                return Ok(());
            };
            let Some(job_token) = claim_job(entity, &run, job, &pool, None)? else {
                return Ok(());
            };
            running.insert(pool_id, count + 1);
            match driver::start(entity, &host_id, &run_spec(job, &pool, &job_token)) {
                Ok(instance_ref) => {
                    set_fields(
                        entity,
                        JOB_RUN,
                        "jobRunId",
                        &job_run_id,
                        None,
                        vec![("instanceRef", Value::string(instance_ref))],
                    )?;
                    done.started += 1;
                }
                Err(err) => {
                    // the errors are the host's answer, kept on the job
                    ExecutionContext::with_current(|ec| ec.message().clear_errors());
                    done.refused += 1;
                    finish_job(
                        entity,
                        &job_run_id,
                        &Outcome::error(format!("the runner did not start: {}", err.message())),
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(err) = outcome {
            tracing::error!(job = %job_run_id, "a queued job run was not started: {err}");
        }
    }
    Ok(done)
}

/// How a job run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// success, failure, error or cancelled.
    pub status: &'static str,
    pub exit_code: Option<i64>,
    pub message: Option<String>,
}

impl Outcome {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            status: "error",
            exit_code: None,
            message: Some(message.into()),
        }
    }
}

fn duration_text(millis: i64) -> String {
    let seconds = (millis / 1000).max(0);
    if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Let go of what a job run that is over held: its container is removed and its registered runner freed. A failure
/// is logged (the cleanup pass removes a container that stays).
pub(crate) fn release(entity: &EntityFacade, job: &EntityValue) {
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    if let (Some(host), Some(instance)) = (field(job, "instanceHostId"), field(job, "instanceRef")) {
        if let Err(err) = driver::remove(entity, &host, &instance, &job_run_id) {
            ExecutionContext::with_current(|ec| ec.message().clear_errors());
            tracing::warn!(job = %job_run_id, "the container of an ended job run was not removed (the cleanup will): {err}");
        }
    }
    if let Some(runner) = field(job, "runnerId") {
        let freed = entity.find(RUNNER).and_then(|find| {
            find.eq("runnerId", Value::string(&runner))
                .eq("currentJobRunId", Value::string(&job_run_id))
                .update_all([("currentJobRunId".to_string(), Value::Null)])
        });
        if let Err(err) = freed {
            tracing::warn!(job = %job_run_id, "the runner of an ended job run was not freed: {err}");
        }
    }
}

/// End a job run that has not ended: its state, its steps left, its log as one blob, its usage, its commit status,
/// its token dead, its container removed, and its run moved on. False when it had already ended.
pub fn finish_job(entity: &EntityFacade, job_run_id: &str, outcome: &Outcome) -> Result<bool, Error> {
    let _rows = crate::rows_guard();
    let Some(job) = super::one(entity, JOB_RUN, "jobRunId", job_run_id)? else {
        return Ok(false);
    };
    let Some(was) = field(&job, "statusId").filter(|status| !ENDED.contains(&status.as_str())) else {
        return Ok(false);
    };
    let now = now_millis();
    let started = millis(&job, "startedDate");
    let used_millis = started.map_or(0, |started| (now - started).max(0));
    let minutes = if started.is_some() {
        (used_millis + MINUTE - 1) / MINUTE
    } else {
        0
    };
    let claimed = set_fields(
        entity,
        JOB_RUN,
        "jobRunId",
        job_run_id,
        Some(&was),
        vec![
            ("statusId", Value::string(outcome.status)),
            ("exitCode", outcome.exit_code.map_or(Value::Null, int)),
            ("statusMessage", opt(outcome.message.clone())),
            ("finishedDate", moqui_core::user::stamp_value(now)),
            ("tokenHash", Value::Null),
            ("tokenExpireDate", Value::Null),
            ("minutesUsed", int(minutes)),
        ],
    )?;
    if !claimed {
        return Ok(false);
    }
    // the steps it did not get to, and the one it was in
    for step in entity
        .find(JOB_STEP)?
        .eq("jobRunId", Value::string(job_run_id))
        .list()?
        .iter()
    {
        let left = match field(step, "statusId").as_deref() {
            Some("running") => {
                if outcome.status == "success" {
                    "success"
                } else {
                    "failure"
                }
            }
            Some("pending") => "skipped",
            _ => continue,
        };
        let mut changed = step.clone();
        changed.set("statusId", Value::string(left));
        if left != "skipped" {
            changed.set("finishedDate", moqui_core::user::stamp_value(now));
        }
        if let Err(err) = changed.update(entity) {
            tracing::error!(job = %job_run_id, "a step of an ended job run was not closed: {err}");
        }
    }
    if let Err(err) = super::log::finalize(entity, job_run_id) {
        tracing::error!(job = %job_run_id, "the log of an ended job run was not stored: {err}");
    }
    let run = field(&job, "pipelineRunId")
        .map(|id| super::one(entity, PIPELINE_RUN, "pipelineRunId", &id))
        .transpose()?
        .flatten();
    if started.is_some() {
        let written = (|| -> Result<(), Error> {
            let mut usage = EntityValue::make(entity, RUNNER_USAGE)?;
            usage.set("jobRunId", Value::string(job_run_id));
            usage.set("pipelineRunId", opt(field(&job, "pipelineRunId")));
            usage.set("repositoryId", opt(field(&job, "repositoryId")));
            usage.set("ownerId", opt(run.as_ref().and_then(|run| field(run, "ownerId"))));
            usage.set("runnerPoolId", opt(field(&job, "runnerPoolId")));
            usage.set(
                "trusted",
                yes_no(run.as_ref().is_some_and(|run| is_yes(run, "trusted"))),
            );
            let month = jiff::Timestamp::from_millisecond(now)
                .map(|stamp| stamp.strftime("%Y-%m").to_string())
                .unwrap_or_default();
            usage.set("periodKey", Value::string(month));
            usage.set("secondsUsed", int(used_millis / 1000));
            usage.set("minutesUsed", int(minutes));
            usage.set("finishedDate", moqui_core::user::stamp_value(now));
            Ok(usage.create_or_update(entity)?)
        })();
        if let Err(err) = written {
            tracing::error!(job = %job_run_id, "the usage of a job run was not recorded: {err}");
        }
    }
    if let (Some(run), Some(job)) = (&run, super::one(entity, JOB_RUN, "jobRunId", job_run_id)?) {
        let description = match (outcome.status, &outcome.message) {
            ("success", _) => format!("Succeeded in {}", duration_text(used_millis)),
            (_, Some(message)) => message.clone(),
            ("failure", None) => format!("Failed in {}", duration_text(used_millis)),
            ("cancelled", None) => "Cancelled".to_string(),
            _ => "Error".to_string(),
        };
        status::write_job(entity, run, &job, &description);
    }
    release(entity, &job);
    if let Some(run_id) = field(&job, "pipelineRunId") {
        advance(entity, &run_id)?;
    }
    Ok(true)
}

/// Move a run on: a waiting job whose needed jobs all succeeded is queued, one whose needed job did not succeed is
/// skipped (and so are the jobs that need it), and the run ends when every job has: success when all succeeded,
/// else error, failure or cancelled, in that order.
pub fn advance(entity: &EntityFacade, pipeline_run_id: &str) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let Some(run) = super::one(entity, PIPELINE_RUN, "pipelineRunId", pipeline_run_id)? else {
        return Ok(());
    };
    if is_ended(&run) || field(&run, "statusId").as_deref() == Some("waiting") {
        return Ok(());
    }
    let mut queued_any = false;
    loop {
        let jobs = current_jobs(entity, pipeline_run_id)?;
        let mut changed = false;
        for job in jobs
            .iter()
            .filter(|job| field(job, "statusId").as_deref() == Some("waiting"))
        {
            let job_run_id = field(job, "jobRunId").unwrap_or_default();
            let needs: Vec<String> = field(job, "needs")
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let needed: Vec<&EntityValue> = jobs
                .iter()
                .filter(|other| field(other, "jobName").is_some_and(|name| needs.contains(&name)))
                .collect();
            let failed = needed
                .iter()
                .find(|other| is_ended(other) && field(other, "statusId").as_deref() != Some("success"));
            if let Some(failed) = failed {
                let message = format!(
                    "Skipped: job {} did not succeed",
                    field(failed, "jobName").unwrap_or_default()
                );
                if set_fields(
                    entity,
                    JOB_RUN,
                    "jobRunId",
                    &job_run_id,
                    Some("waiting"),
                    vec![
                        ("statusId", Value::string("skipped")),
                        ("statusMessage", Value::string(&message)),
                        ("finishedDate", moqui_core::user::now_value()),
                    ],
                )? {
                    changed = true;
                    if let Some(job) = super::one(entity, JOB_RUN, "jobRunId", &job_run_id)? {
                        status::write_job(entity, &run, &job, &message);
                    }
                }
            } else if needed
                .iter()
                .all(|other| field(other, "statusId").as_deref() == Some("success"))
                && set_fields(
                    entity,
                    JOB_RUN,
                    "jobRunId",
                    &job_run_id,
                    Some("waiting"),
                    vec![
                        ("statusId", Value::string("queued")),
                        ("queuedDate", moqui_core::user::now_value()),
                    ],
                )?
            {
                changed = true;
                queued_any = true;
            }
        }
        if !changed {
            break;
        }
    }
    let jobs = current_jobs(entity, pipeline_run_id)?;
    if !jobs.is_empty() && jobs.iter().all(is_ended) {
        let has = |status: &str| jobs.iter().any(|job| field(job, "statusId").as_deref() == Some(status));
        let ended = if has("error") {
            "error"
        } else if has("failure") || has("skipped") {
            "failure"
        } else if has("cancelled") {
            "cancelled"
        } else {
            "success"
        };
        let was = field(&run, "statusId").unwrap_or_default();
        set_fields(
            entity,
            PIPELINE_RUN,
            "pipelineRunId",
            pipeline_run_id,
            Some(&was),
            vec![
                ("statusId", Value::string(ended)),
                ("finishedDate", moqui_core::user::now_value()),
            ],
        )?;
    } else if queued_any {
        kick();
    }
    Ok(())
}

/// What a pass of [`end_overdue`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ended {
    pub timed_out: usize,
    pub lost: usize,
    pub unclaimed: usize,
    /// Containers of ended or lost job runs removed from the hosts.
    pub removed: i64,
}

/// End the job runs past their time and the ones whose runner is gone, and remove what they left, as of `now`
/// (milliseconds): a running job past its time limit; one whose runner has not called for the conf's `git-ci`
/// `lost-seconds` (180) and whose container the host no longer has, or has as exited; a queued job no runner took in
/// `queue-timeout-minutes` (1440). Then each host's containers of this server whose job run is not running are
/// removed, by label.
pub fn end_overdue(entity: &EntityFacade, now: i64) -> Result<Ended, Error> {
    let _rows = crate::rows_guard();
    let mut ended = Ended::default();
    let lost_millis = i64::try_from(super::number_setting("lost-seconds", 180)).unwrap_or(180) * 1000;
    let running = entity.find(JOB_RUN)?.eq("statusId", Value::string("running")).list()?;
    for job in running.iter() {
        let job_run_id = field(job, "jobRunId").unwrap_or_default();
        // one job that cannot be checked does not stop the others
        let outcome = (|| -> Result<(), Error> {
            if millis(job, "tokenExpireDate").is_none_or(|expires| expires <= now) {
                let minutes = long(job, "timeoutMinutes").unwrap_or(0);
                if finish_job(
                    entity,
                    &job_run_id,
                    &Outcome::error(format!("Timed out after {minutes} minutes")),
                )? {
                    ended.timed_out += 1;
                }
                return Ok(());
            }
            let silent = millis(job, "lastContactDate").is_none_or(|last| now - last > lost_millis);
            if !silent {
                return Ok(());
            }
            let gone = match (field(job, "instanceHostId"), field(job, "instanceRef")) {
                (Some(host), Some(instance)) => match driver::inspect(entity, &host, &instance) {
                    Ok(InstanceState::Running) => None,
                    Ok(InstanceState::Missing) => Some("The runner's container was lost".to_string()),
                    Ok(InstanceState::Exited(code)) => Some(format!(
                        "The runner exited{} without a result",
                        code.map_or(String::new(), |code| format!(" (code {code})"))
                    )),
                    Err(err) => {
                        ExecutionContext::with_current(|ec| ec.message().clear_errors());
                        tracing::warn!(job = %job_run_id, "the container of a job run was not inspected: {err}");
                        None
                    }
                },
                // a registered runner that stopped answering; a container still being started is left to its time limit
                (None, _) if field(job, "runnerId").is_some() => Some("The runner stopped answering".to_string()),
                _ => None,
            };
            if let Some(why) = gone {
                if finish_job(entity, &job_run_id, &Outcome::error(why))? {
                    ended.lost += 1;
                }
            }
            Ok(())
        })();
        if let Err(err) = outcome {
            tracing::error!(job = %job_run_id, "a running job run was not checked: {err}");
        }
    }
    let queue_minutes = i64::try_from(super::number_setting("queue-timeout-minutes", 1440)).unwrap_or(1440);
    let stale = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("queued"))
        .condition(Condition::less(
            "queuedDate",
            moqui_core::user::stamp_value(now - queue_minutes * MINUTE),
        ))
        .list()?;
    for job in stale.iter() {
        let Some(job_run_id) = field(job, "jobRunId") else {
            continue;
        };
        match finish_job(
            entity,
            &job_run_id,
            &Outcome::error(format!("No runner took the job in {queue_minutes} minutes")),
        ) {
            Ok(true) => ended.unclaimed += 1,
            Ok(false) => {}
            Err(err) => tracing::error!(job = %job_run_id, "a stale queued job run was not ended: {err}"),
        }
    }
    // what ended or lost runs left on the hosts
    let hosts: BTreeSet<String> = entity
        .find(RUNNER_POOL)?
        .list()?
        .iter()
        .filter(|pool| field(pool, "poolKind").as_deref() != Some("registered"))
        .filter_map(|pool| field(pool, "instanceHostId"))
        .collect();
    let keep: Vec<(String, String)> = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("running"))
        .list()?
        .iter()
        .filter_map(|job| Some((field(job, "instanceHostId")?, field(job, "jobRunId")?)))
        .collect();
    for host in hosts {
        let of_host: Vec<String> = keep
            .iter()
            .filter(|(job_host, _)| job_host == &host)
            .map(|(_, job)| job.clone())
            .collect();
        match driver::cleanup(entity, &host, &of_host) {
            Ok(removed) => ended.removed += removed,
            Err(err) => {
                ExecutionContext::with_current(|ec| ec.message().clear_errors());
                tracing::warn!(host = %host, "the containers of ended job runs were not cleaned up: {err}");
            }
        }
    }
    Ok(ended)
}

/// Cancel a run that has not ended: its waiting and queued jobs are cancelled, its running jobs ended and their
/// containers removed.
///
/// # Errors
///
/// When there is no such run, or it has ended.
pub fn cancel_run(entity: &EntityFacade, pipeline_run_id: &str) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let run = super::one(entity, PIPELINE_RUN, "pipelineRunId", pipeline_run_id)?
        .ok_or_else(|| Error::new(format!("no pipeline run {pipeline_run_id}")))?;
    if is_ended(&run) {
        return Err(Error::new(format!(
            "pipeline run {pipeline_run_id} has ended ({})",
            field(&run, "statusId").unwrap_or_default()
        )));
    }
    let was = field(&run, "statusId").unwrap_or_default();
    let cancelled = Outcome {
        status: "cancelled",
        exit_code: None,
        message: Some("Cancelled".to_string()),
    };
    // first the run, so ending its jobs does not queue the ones that wait for them
    set_fields(
        entity,
        PIPELINE_RUN,
        "pipelineRunId",
        pipeline_run_id,
        Some(&was),
        vec![
            ("statusId", Value::string("cancelled")),
            ("finishedDate", moqui_core::user::now_value()),
        ],
    )?;
    for job in current_jobs(entity, pipeline_run_id)? {
        if let Some(job_run_id) = field(&job, "jobRunId").filter(|_| !is_ended(&job)) {
            finish_job(entity, &job_run_id, &cancelled)?;
        }
    }
    Ok(())
}

/// Let a run that waits for approval start: its jobs that need no other are queued.
///
/// # Errors
///
/// When there is no such run, or it does not wait.
pub fn approve_run(entity: &EntityFacade, pipeline_run_id: &str, user_id: Option<String>) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let run = super::one(entity, PIPELINE_RUN, "pipelineRunId", pipeline_run_id)?
        .ok_or_else(|| Error::new(format!("no pipeline run {pipeline_run_id}")))?;
    let approved = field(&run, "approvalState").as_deref() == Some("waiting")
        && set_fields(
            entity,
            PIPELINE_RUN,
            "pipelineRunId",
            pipeline_run_id,
            Some("waiting"),
            vec![
                ("statusId", Value::string("queued")),
                ("approvalState", Value::string("approved")),
                ("approvedByUserId", opt(user_id)),
                ("statusMessage", Value::Null),
            ],
        )?;
    if !approved {
        return Err(Error::new(format!(
            "pipeline run {pipeline_run_id} does not wait for approval"
        )));
    }
    for job in current_jobs(entity, pipeline_run_id)? {
        if field(&job, "needs").is_some() || field(&job, "statusId").as_deref() != Some("waiting") {
            continue;
        }
        let job_run_id = field(&job, "jobRunId").unwrap_or_default();
        if set_fields(
            entity,
            JOB_RUN,
            "jobRunId",
            &job_run_id,
            Some("waiting"),
            vec![
                ("statusId", Value::string("queued")),
                ("queuedDate", moqui_core::user::now_value()),
            ],
        )? {
            if let Some(job) = super::one(entity, JOB_RUN, "jobRunId", &job_run_id)? {
                status::write_job(entity, &run, &job, "Queued");
            }
        }
    }
    kick();
    Ok(())
}

/// Run an ended job again: a new attempt of it is queued in the same run, with the same trust, and the jobs skipped
/// for it wait again. The new job run's id.
///
/// # Errors
///
/// When there is no such job run, it has not ended, or a newer attempt replaced it.
pub fn retry_job(entity: &EntityFacade, job_run_id: &str) -> Result<String, Error> {
    let _rows = crate::rows_guard();
    let job = super::one(entity, JOB_RUN, "jobRunId", job_run_id)?
        .ok_or_else(|| Error::new(format!("no job run {job_run_id}")))?;
    if !is_ended(&job) {
        return Err(Error::new(format!("job run {job_run_id} has not ended")));
    }
    if is_yes(&job, "superseded") {
        return Err(Error::new(format!("job run {job_run_id} was already run again")));
    }
    let run_id = field(&job, "pipelineRunId").unwrap_or_default();
    let run = super::one(entity, PIPELINE_RUN, "pipelineRunId", &run_id)?
        .ok_or_else(|| Error::new(format!("no pipeline run {run_id}")))?;
    if field(&run, "statusId").as_deref() == Some("waiting") {
        return Err(Error::new(format!("pipeline run {run_id} waits for approval")));
    }
    let mut next = EntityValue::make(entity, JOB_RUN)?;
    next.set_sequenced_id_primary(entity)?;
    for name in [
        "pipelineRunId",
        "repositoryId",
        "jobName",
        "matrixKey",
        "needs",
        "poolName",
        "image",
        "jobSpec",
    ] {
        next.set(name, super::field_value(&job, name));
    }
    let spec: serde_json::Value =
        serde_json::from_str(&field(&job, "jobSpec").unwrap_or_default()).unwrap_or(serde_json::Value::Null);
    next.set(
        "timeoutMinutes",
        spec["timeoutMinutes"]
            .as_i64()
            .map_or_else(|| super::field_value(&job, "timeoutMinutes"), int),
    );
    next.set("attempt", int(long(&job, "attempt").unwrap_or(1) + 1));
    next.set("statusId", Value::string("queued"));
    next.set("queuedDate", moqui_core::user::now_value());
    next.set("nextChunkSeq", int(1));
    next.set("logBytes", int(0));
    next.create(entity)?;
    let next_id = field(&next, "jobRunId").unwrap_or_default();
    for step in entity
        .find(JOB_STEP)?
        .eq("jobRunId", Value::string(job_run_id))
        .order_by("stepSeq", false)
        .list()?
        .iter()
    {
        let mut copy = EntityValue::make(entity, JOB_STEP)?;
        copy.set("jobRunId", Value::string(&next_id));
        for name in ["stepSeq", "stepName", "command"] {
            copy.set(name, super::field_value(step, name));
        }
        copy.set("statusId", Value::string("pending"));
        copy.create(entity)?;
    }
    set_fields(
        entity,
        JOB_RUN,
        "jobRunId",
        job_run_id,
        None,
        vec![("superseded", Value::string("Y"))],
    )?;
    // the jobs skipped because this one did not succeed wait for the new attempt
    let name = field(&job, "jobName").unwrap_or_default();
    let mut waiting_for: Vec<String> = vec![name];
    loop {
        let mut more = false;
        for other in current_jobs(entity, &run_id)? {
            if field(&other, "statusId").as_deref() != Some("skipped") {
                continue;
            }
            let needs = field(&other, "needs").unwrap_or_default();
            if !needs
                .split_whitespace()
                .any(|need| waiting_for.iter().any(|name| name == need))
            {
                continue;
            }
            let other_id = field(&other, "jobRunId").unwrap_or_default();
            set_fields(
                entity,
                JOB_RUN,
                "jobRunId",
                &other_id,
                Some("skipped"),
                vec![
                    ("statusId", Value::string("waiting")),
                    ("statusMessage", Value::Null),
                    ("finishedDate", Value::Null),
                ],
            )?;
            if let Some(other_name) = field(&other, "jobName").filter(|name| !waiting_for.contains(name)) {
                waiting_for.push(other_name);
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    set_fields(
        entity,
        PIPELINE_RUN,
        "pipelineRunId",
        &run_id,
        None,
        vec![("statusId", Value::string("queued")), ("finishedDate", Value::Null)],
    )?;
    status::write_job(entity, &run, &next, "Queued");
    kick();
    Ok(next_id)
}
