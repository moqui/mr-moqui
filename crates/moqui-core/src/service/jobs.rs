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

//! Service jobs: the scheduler pass and the job run. A port of `ScheduledJobRunner` and
//! `ServiceCallJobImpl`.
//!
//! The pass reads `ServiceJob` rows, takes the `ServiceJobRunLock` row with a conditional write so
//! two instances cannot both run a job, creates the `ServiceJobRun` history row, and hands the run
//! to the service pool. It never runs the service body. A worker does, and records the outcome.
//!
//! A cron expression is read in the machine's time zone ([`job_time_zone`]), the zone the operating
//! system reports. That is not the server zone `default_time_zone` names (UTC in MoquiDefaultConf).
//! The pass still skips a job only when the previous occurrence is strictly before the last run, so
//! a last run at that occurrence still runs, as Java's `!lastSchedule.isBefore(lastRunDt)`.
//!
//! A fire-and-forget call that finds the pool queue full is stored as a `ServiceJob` named with
//! [`QUEUE_PREFIX`] and no cron expression. The pass submits it when a slot is free and deletes it
//! when it has run. Parameters of such a call are stored as text, as for any job.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_db::sql::{insert, update, CompareOp, Condition};
use moqui_lib::object::{basic_convert, to_plain_string, Value};
use moqui_lib::sync::lock;

use super::call::CallOptions;
use super::pool::Shared;
use super::util::{field_text, now_stamp, reserve};
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::{TransactionAttribute, TransactionManager};

/// Marks a `ServiceJob` that is a queued fire-and-forget call, not a scheduled job.
pub(crate) const QUEUE_PREFIX: &str = "ASYNC_QUEUE_";

const SERVICE_JOB: &str = "moqui.service.job.ServiceJob";
const JOB_PARAMETER: &str = "moqui.service.job.ServiceJobParameter";
const JOB_RUN: &str = "moqui.service.job.ServiceJobRun";
const JOB_LOCK: &str = "moqui.service.job.ServiceJobRunLock";
const DEFAULT_EXPIRE_MINUTES: i64 = 1440;
const DEFAULT_RETRY_MINUTES: i64 = 5;
const DEFAULT_JOB_TIMEOUT_SECONDS: u64 = 1800;
const TEXT_LIMIT: usize = 4000;

/// The time zone a ServiceJob cron expression is read in: the machine's zone, the one the operating
/// system reports. It is not the server zone `default_time_zone` names.
pub(crate) fn job_time_zone() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::system()
}

/// A job handed to a worker.
pub(crate) struct JobRun {
    job_name: String,
    service_name: String,
    parameters: BTreeMap<String, Value>,
    timeout: Duration,
    job_run_id: String,
    /// The lock's `lastRunTime` before this run, restored when the run fails.
    last_run_time: Value,
    delete_when_done: bool,
    user_name: Option<String>,
    /// The agent run that started it, which the run works for too (the agent triggers' loop guard).
    origin: Option<crate::context::AgentOrigin>,
    /// `ServiceJob.topic`: a notification of this topic is sent when the run ends.
    topic: Option<String>,
    description: Option<String>,
    factory: Arc<ExecutionContextFactory>,
}

impl JobRun {
    /// The factory the run belongs to.
    pub(crate) fn factory(&self) -> &Arc<ExecutionContextFactory> {
        &self.factory
    }
}

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PassReport {
    pub jobs_run: usize,
    pub jobs_active: usize,
    pub jobs_paused: usize,
    pub jobs_ready_not_run: usize,
    /// Jobs that could not be checked or started; each is logged and the pass goes on.
    pub jobs_failed: usize,
}

/// One scheduler pass for the pool's factory. Errors are logged, so a bad pass never ends the thread.
pub(crate) fn run_scheduled(shared: &Arc<Shared>) -> PassReport {
    let Some(factory) = shared.factory() else {
        return PassReport::default();
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        pass(shared, &factory)
    }));
    let report = match outcome {
        Ok(Ok(report)) => report,
        Ok(Err(err)) => {
            tracing::error!(error = %err, "scheduled job pass failed");
            PassReport::default()
        }
        Err(_) => {
            tracing::error!("scheduled job pass panicked");
            PassReport::default()
        }
    };
    // As Java's `ScheduledJobRunner`, a pass that failed is still counted. A pass fails only before it looks at a job.
    shared.note_pass(&report);
    report
}

fn pass(shared: &Arc<Shared>, factory: &Arc<ExecutionContextFactory>) -> Result<PassReport, Error> {
    let mut report = PassReport::default();
    let Some(facade) = factory.entity() else {
        return Ok(report);
    };
    let mut slots = shared.job_slots();
    let now = jiff::Timestamp::now();
    let now_stamp = timestamp_value(now.as_millisecond());
    let plan = LockPlan::new(&facade)?;

    let jobs = facade
        .find(SERVICE_JOB)?
        .order_by("priority", false)
        .order_by("jobName", false)
        .list()?;
    for job in jobs.values() {
        let job_name = field_text(job, "jobName");
        let queued = job_name.starts_with(QUEUE_PREFIX);
        let cron_text = field_text(job, "cronExpression");
        if !queued && cron_text.is_empty() {
            continue;
        }
        if !within_dates(job, now.as_millisecond()) {
            continue;
        }
        if !paused_ok(job) {
            report.jobs_paused += 1;
            continue;
        }
        // As in Java's `ScheduledJobRunner`, a job that cannot be checked or started is logged and the pass goes on
        // with the next job.
        let exhausted = if queued {
            Ok(false)
        } else {
            repeat_exhausted(&facade, job, &job_name, &now_stamp)
        };
        match exhausted {
            Ok(true) => continue,
            Ok(false) => {}
            Err(err) => {
                tracing::error!(job = %job_name, error = %err, "Error checking scheduled job, skipping it");
                report.jobs_failed += 1;
                continue;
            }
        }
        report.jobs_active += 1;
        let ready = match readiness(&facade, job, &job_name, &cron_text, now) {
            Ok(Some(ready)) => ready,
            Ok(None) => continue,
            Err(err) => {
                tracing::error!(job = %job_name, cron = %cron_text, error = %err, "Error checking scheduled job, skipping it");
                report.jobs_failed += 1;
                continue;
            }
        };
        if slots <= 0 {
            report.jobs_ready_not_run += 1;
            continue;
        }
        match dispatch(shared, factory, &facade, &plan, job, ready, &now_stamp) {
            Ok(true) => {
                report.jobs_run += 1;
                slots -= 1;
            }
            Ok(false) => {}
            Err(err) => {
                tracing::error!(job = %job_name, error = %err, "Error starting scheduled job, skipping it");
                report.jobs_failed += 1;
            }
        }
    }
    Ok(report)
}

/// What the lock row allows this pass to do for a job that is otherwise due.
struct Ready {
    expect: Expect,
    last_run_time: Value,
}

/// `None` when the job is not due, is running, or failed too recently.
fn readiness(
    facade: &EntityFacade,
    job: &EntityValue,
    job_name: &str,
    cron_text: &str,
    now: jiff::Timestamp,
) -> Result<Option<Ready>, Error> {
    let lock = facade.find(JOB_LOCK)?.eq("jobName", Value::string(job_name)).one()?;
    let mut expect = Expect::Absent;
    let mut last_run_time = Value::Null;
    let mut last_run_ms = None;
    if let Some(lock) = &lock {
        let run_id = field_text(lock, "jobRunId");
        last_run_time = lock.get("lastRunTime").cloned().unwrap_or(Value::Null);
        last_run_ms = millis_of(&last_run_time);
        expect = if run_id.is_empty() {
            Expect::Free
        } else {
            Expect::Run(run_id)
        };
        if let (Expect::Run(run_id), Some(last)) = (&expect, last_run_ms) {
            let expire = number(job, "expireLockTime").unwrap_or(DEFAULT_EXPIRE_MINUTES);
            if last < now.as_millisecond().saturating_sub(expire.saturating_mul(60_000)) {
                tracing::warn!(job = job_name, run = %run_id, "lock expired, ignoring it");
            } else {
                tracing::info!(job = job_name, run = %run_id, "lock held, not running");
                return Ok(None);
            }
        } else if matches!(expect, Expect::Run(_)) {
            // A run id with no time: there is nothing to say the lock is old, so it holds.
            return Ok(None);
        }
    }
    if !cron_text.is_empty() {
        let cron = super::schedule::cached_cron(cron_text)?;
        let zoned = now.to_zoned(job_time_zone());
        // A schedule with nothing before now has not come due yet.
        let Ok(last_schedule) = cron.find_previous_occurrence(&zoned, true) else {
            return Ok(None);
        };
        if last_run_ms.is_some_and(|last_run| last_schedule.timestamp().as_millisecond() < last_run) {
            return Ok(None);
        }
        // After an error, wait the minimum retry time.
        let runs = facade
            .find(JOB_RUN)?
            .eq("jobName", Value::string(job_name))
            .order_by("startTime", true)
            .limit(1)
            .list()?;
        if let Some(last) = runs.values().first().filter(|run| field_text(run, "hasError") == "Y") {
            let stamp = millis_of(last.get("endTime").unwrap_or(&Value::Null))
                .or_else(|| millis_of(last.get("startTime").unwrap_or(&Value::Null)));
            let retry = number(job, "minRetryTime").unwrap_or(DEFAULT_RETRY_MINUTES);
            if let Some(stamp) = stamp {
                if stamp > now.as_millisecond().saturating_sub(retry.saturating_mul(60_000)) {
                    tracing::info!(job = job_name, "not retrying after an error yet");
                    return Ok(None);
                }
            }
        }
    }
    Ok(Some(Ready { expect, last_run_time }))
}

fn dispatch(
    shared: &Arc<Shared>,
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    plan: &LockPlan,
    job: &EntityValue,
    ready: Ready,
    now_stamp: &Value,
) -> Result<bool, Error> {
    let job_name = field_text(job, "jobName");
    let run_id = create_run(facade, &job_name, None, &BTreeMap::new())?;
    if !plan.claim(&job_name, &run_id, now_stamp, &ready.expect)? {
        // Another instance took it. Its run row would only confuse the history.
        delete_run(facade, &run_id);
        return Ok(false);
    }
    let queued = job_name.starts_with(QUEUE_PREFIX);
    let submitted = job_parameters(facade, &job_name).and_then(|parameters| {
        let run = JobRun {
            job_name: job_name.clone(),
            service_name: field_text(job, "serviceName"),
            parameters,
            timeout: Duration::from_secs(
                number(job, "transactionTimeout").map_or(DEFAULT_JOB_TIMEOUT_SECONDS, |secs| {
                    u64::try_from(secs).unwrap_or(DEFAULT_JOB_TIMEOUT_SECONDS)
                }),
            ),
            job_run_id: run_id.clone(),
            last_run_time: ready.last_run_time,
            delete_when_done: queued,
            user_name: None,
            origin: None,
            topic: Some(field_text(job, "topic")).filter(|topic| !topic.is_empty()),
            description: Some(field_text(job, "description")).filter(|text| !text.is_empty()),
            factory: Arc::clone(factory),
        };
        if shared.submit_job(run) {
            Ok(())
        } else {
            Err(Error::new("the service job queue is full or the pool stopped"))
        }
    });
    if let Err(err) = submitted {
        // As Java's `ScheduledJobRunner`, a run that could not be started is a failed run: the lock is cleared and
        // keeps this pass's run time, and the run row records the error.
        tracing::error!(job = %job_name, run = %run_id, error = %err, "Error running scheduled job");
        if let Err(err) = plan.release(&job_name, &run_id, now_stamp) {
            tracing::warn!(job = %job_name, run = %run_id, error = %err, "could not clear the job lock");
        }
        record(
            facade,
            &run_id,
            &[
                ("hasError", Value::string("Y")),
                ("errors", Value::string(err.to_string())),
                ("startTime", now_stamp.clone()),
                ("endTime", now_stamp.clone()),
            ],
        );
        return Ok(true);
    }
    tracing::info!(job = %job_name, run = %run_id, "submitted job");
    Ok(true)
}

/// Store a call the pool queue had no room for. The pass will submit it.
pub(crate) fn persist_queued(
    factory: &Arc<ExecutionContextFactory>,
    service: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<(), Error> {
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("service worker queue is full and there is no database to queue in"))?;
    // A sequence number makes the name unique without inserting anything.
    let mut numbered = facade.make_value(JOB_RUN)?;
    numbered.set_sequenced_id_primary(&facade)?;
    let id = field_text(&numbered, "jobRunId");
    let name = format!("{QUEUE_PREFIX}{id}");
    let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let tx = facade.transactions_for(SERVICE_JOB)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut job = facade.make_value(SERVICE_JOB)?;
        job.set("jobName", Value::string(&name));
        job.set("description", Value::string(format!("Queued call of {service}")));
        job.set("serviceName", Value::string(service));
        job.set("paused", Value::string("N"));
        // Queued calls go after the jobs people scheduled.
        job.set("priority", Value::I32(9));
        job.create(&facade)?;
        for (key, value) in parameters {
            if value.is_null() {
                continue;
            }
            let mut row = facade.make_value(JOB_PARAMETER)?;
            row.set("jobName", Value::string(&name));
            row.set("parameterName", Value::string(key));
            row.set("parameterValue", Value::string(to_plain_string(value)));
            row.create(&facade)?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Run a named job now, outside its schedule. Returns the run id.
///
/// # Errors
///
/// Returns an error when there is no such job, the pool is full, or a row cannot be written.
pub fn run_job_now(job_name: &str, extra: &BTreeMap<String, Value>) -> Result<String, Error> {
    let factory = super::pool::capture_factory()?;
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    let job = facade
        .find(SERVICE_JOB)?
        .eq("jobName", Value::string(job_name))
        .one()?
        .ok_or_else(|| Error::new(format!("No ServiceJob record found for jobName {job_name}")))?;
    let mut parameters = job_parameters(&facade, job_name)?;
    parameters.extend(extra.clone());
    let run_id = create_run(
        &facade,
        job_name,
        crate::user::current_user_id().ok().flatten(),
        &parameters,
    )?;
    let pools = factory.service_pools();
    let shared = pools.shared.clone();
    let run = JobRun {
        job_name: job_name.to_string(),
        service_name: field_text(&job, "serviceName"),
        parameters,
        timeout: Duration::from_secs(
            number(&job, "transactionTimeout")
                .and_then(|secs| u64::try_from(secs).ok())
                .unwrap_or(DEFAULT_JOB_TIMEOUT_SECONDS),
        ),
        job_run_id: run_id.clone(),
        last_run_time: Value::Null,
        delete_when_done: false,
        user_name: crate::user::current_username().ok().flatten(),
        origin: super::pool::current_origin(),
        topic: Some(field_text(&job, "topic")).filter(|topic| !topic.is_empty()),
        description: Some(field_text(&job, "description")).filter(|text| !text.is_empty()),
        factory: Arc::clone(&factory),
    };
    // The inline executor runs it at the next drain point.
    let run = match pools.submit_inline_job(Box::new(run)) {
        Ok(()) => return Ok(run_id),
        Err(run) => *run,
    };
    let Some(shared) = shared else {
        delete_run(&facade, &run_id);
        return Err(Error::new("service worker pool is not running"));
    };
    if !shared.submit_job(run) {
        delete_run(&facade, &run_id);
        return Err(Error::new("service worker queue is full"));
    }
    Ok(run_id)
}

/// Send the ServiceJobError notification. A failure is logged and does not change the job row.
#[cfg(test)]
pub(crate) fn send_job_error_notification(job_name: &str, job_run_id: &str, errors: &str) {
    send_job_error_to(job_name, job_run_id, errors, &[], indexmap::IndexMap::new());
}

fn send_job_error_to(
    job_name: &str,
    job_run_id: &str,
    errors: &str,
    user_ids: &[String],
    extra: indexmap::IndexMap<String, Value>,
) {
    if let Err(err) = crate::notify::service_job_error(job_name, job_run_id, errors, user_ids, extra) {
        tracing::warn!(
            job = %job_name,
            run = %job_run_id,
            error = %err,
            "could not send the service job error notification"
        );
    }
}

/// The worker side: run the service and record what happened.
///
/// A run that unwinds is still ended, by [`RunEnd`]: the lock goes back with the previous run time and the run is
/// recorded with an error, so the job does not wait out `expireLockTime`.
pub(crate) fn run_job(job: JobRun) {
    let factory = Arc::clone(&job.factory);
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    if let Some(user) = &job.user_name {
        if let Err(err) = crate::user::internal_login_user(user) {
            tracing::warn!(job = %job.job_name, error = %err, "could not log in the job's user");
        }
    }
    context.set_agent_origin(job.origin.clone());
    let Some(facade) = factory.entity() else {
        return;
    };
    // The thread's name is this port's (`moqui-job-0`, not Java's `MoquiJob-1`), an accepted difference (P7b-16).
    let thread = std::thread::current().name().unwrap_or("").to_string();
    let (host_address, host_name) = crate::context::hits::server_address();
    record(
        &facade,
        &job.job_run_id,
        &[
            ("hostAddress", Value::string(host_address)),
            ("hostName", Value::string(host_name)),
            ("runThread", Value::string(thread)),
            ("startTime", now_stamp()),
        ],
    );
    let end = RunEnd {
        facade: &facade,
        job: &job,
        done: false,
    };
    let mut parameters = job.parameters.clone();
    if !job.last_run_time.is_null() {
        parameters.insert("lastRunTime".to_string(), job.last_run_time.clone());
    }
    let options = CallOptions {
        disable_authz: true,
        timeout: Some(job.timeout),
        ..CallOptions::default()
    };
    let results = match super::call::sync_with(&job.service_name, &parameters, &options) {
        Ok(results) => results,
        Err(err) => {
            tracing::error!(job = %job.job_name, error = %err, "error in service job call");
            context.message().add_error(err.to_string());
            BTreeMap::new()
        }
    };
    let (has_error, messages, errors) = {
        let messages = context.message();
        (
            !messages.errors().is_empty(),
            clip(&messages.messages().join("\n")),
            clip(&messages.errors().join("\n")),
        )
    };
    let result_values: indexmap::IndexMap<String, Value> = results.into_iter().collect();
    let results = serde_json::to_string(&moqui_lib::json::to_json(&Value::Map(result_values.clone()))).ok();
    end.finish(&RunOutcome {
        has_error,
        results,
        result_values,
        messages,
        errors,
    });
}

/// What a job run produced, for [`finish_run`].
struct RunOutcome {
    has_error: bool,
    /// The results as JSON.
    results: Option<String>,
    /// The results, for the notifications.
    result_values: indexmap::IndexMap<String, Value>,
    messages: String,
    errors: String,
}

/// Ends a job run on every path. [`run_job`] hands it the outcome; a run that unwinds first ends as failed.
struct RunEnd<'a> {
    facade: &'a EntityFacade,
    job: &'a JobRun,
    done: bool,
}

impl RunEnd<'_> {
    fn finish(mut self, outcome: &RunOutcome) {
        self.done = true;
        finish_run(self.facade, self.job, outcome);
    }
}

impl Drop for RunEnd<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        tracing::error!(job = %self.job.job_name, run = %self.job.job_run_id, "service job panicked");
        let outcome = RunOutcome {
            has_error: true,
            results: None,
            result_values: indexmap::IndexMap::new(),
            messages: String::new(),
            errors: "The service job panicked".to_string(),
        };
        super::pool::cleanup_step("job run end", || finish_run(self.facade, self.job, &outcome));
    }
}

/// Release the lock, record the run's end, and report errors. A queued call is removed whether or not it worked.
fn finish_run(facade: &EntityFacade, job: &JobRun, outcome: &RunOutcome) {
    // Frames the service left open are rolled back first, so the records below do not join them.
    super::pool::rollback_frames(facade);
    // The lock goes first: a failure restores the previous run time so the job is due again.
    if !job.delete_when_done {
        let restored = if outcome.has_error {
            job.last_run_time.clone()
        } else {
            now_stamp()
        };
        release_lock(facade, job, &restored);
    }
    record(
        facade,
        &job.job_run_id,
        &[
            ("endTime", now_stamp()),
            ("results", outcome.results.clone().map_or(Value::Null, Value::String)),
            ("messages", Value::string(&outcome.messages)),
            ("hasError", Value::string(if outcome.has_error { "Y" } else { "N" })),
            (
                "errors",
                if outcome.has_error {
                    Value::string(&outcome.errors)
                } else {
                    Value::Null
                },
            ),
        ],
    );
    if outcome.has_error {
        tracing::warn!(job = %job.job_name, run = %job.job_run_id, errors = %outcome.errors, "service job ended with errors");
    }
    send_job_notifications(facade, job, outcome);
    if job.delete_when_done {
        delete_queued(facade, &job.job_name);
        if let Err(err) = LockPlan::new(facade).and_then(|plan| plan.delete_lock(&job.job_name)) {
            tracing::warn!(job = %job.job_name, error = %err, "could not remove a queued job's lock");
        }
    }
}

/// Java's job notifications: with a `topic`, a notification of that topic (`success`, or `danger` on an error), and on
/// an error a `ServiceJobError` notification. Both go to the user the job ran as and to each `ServiceJobUser` of the job
/// that does not turn notifications off. A failure is logged.
fn send_job_notifications(facade: &EntityFacade, job: &JobRun, outcome: &RunOutcome) {
    if job.topic.is_none() && !outcome.has_error {
        return;
    }
    let mut user_ids: Vec<String> = Vec::new();
    if let Some(user_id) = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
    {
        user_ids.push(user_id);
    }
    let job_users = facade.find("moqui.service.job.ServiceJobUser").and_then(|find| {
        find.eq("jobName", Value::string(&job.job_name))
            .use_cache(true)
            .disable_authz()
            .list()
    });
    match job_users {
        Ok(rows) => {
            for row in rows.iter() {
                let user_id = field_text(row, "userId");
                if !user_id.is_empty() && field_text(row, "receiveNotifications") != "N" && !user_ids.contains(&user_id)
                {
                    user_ids.push(user_id);
                }
            }
        }
        Err(err) => tracing::warn!(job = %job.job_name, error = %err, "could not read the job's notification users"),
    }
    let mut run = indexmap::IndexMap::new();
    run.insert("jobName".to_string(), Value::string(&job.job_name));
    run.insert(
        "description".to_string(),
        job.description.clone().map_or(Value::Null, Value::String),
    );
    run.insert("jobRunId".to_string(), Value::string(&job.job_run_id));
    run.insert("endTime".to_string(), now_stamp());
    run.insert("messages".to_string(), Value::string(&outcome.messages));
    run.insert("hasError".to_string(), Value::Bool(outcome.has_error));
    run.insert("errors".to_string(), Value::string(&outcome.errors));
    let mut message = indexmap::IndexMap::new();
    message.insert("serviceCallRun".to_string(), Value::Map(run));
    message.insert(
        "parameters".to_string(),
        Value::Map(
            job.parameters
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        ),
    );
    message.insert("results".to_string(), Value::Map(outcome.result_values.clone()));
    if let Some(topic) = &job.topic {
        let mut note = crate::notify::NotificationMessage::new();
        note.topic(topic)
            .notification_type(if outcome.has_error { "danger" } else { "success" })
            .message_map(message.clone())
            .user_ids(user_ids.iter().cloned());
        if let Err(err) = note.send() {
            tracing::warn!(job = %job.job_name, topic = %topic, error = %err, "could not send the job notification");
        }
    }
    if outcome.has_error {
        send_job_error_to(&job.job_name, &job.job_run_id, &outcome.errors, &user_ids, message);
    }
}

/// A run the pool dropped at shutdown before a worker took it. Its lock goes back with the previous run time and the
/// run is recorded as failed, so neither waits out the lock's expiry. A queued call stays, for the next start.
pub(crate) fn abandon(job: JobRun) {
    let factory = Arc::clone(&job.factory);
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let Some(facade) = factory.entity() else {
        return;
    };
    tracing::warn!(job = %job.job_name, run = %job.job_run_id, "service worker pool stopped before the job ran");
    release_lock(&facade, &job, &job.last_run_time);
    record(
        &facade,
        &job.job_run_id,
        &[
            ("endTime", now_stamp()),
            ("hasError", Value::string("Y")),
            (
                "errors",
                Value::string("The service worker pool stopped before the job ran"),
            ),
        ],
    );
}

/// Clear this run's hold on the job lock and set `lastRunTime`. A failure is logged.
fn release_lock(facade: &EntityFacade, job: &JobRun, last_run: &Value) {
    if let Err(err) = LockPlan::new(facade).and_then(|plan| plan.release(&job.job_name, &job.job_run_id, last_run)) {
        tracing::warn!(job = %job.job_name, run = %job.job_run_id, error = %err, "could not clear the job lock");
    }
}

fn delete_queued(facade: &EntityFacade, job_name: &str) {
    let outcome = (|| -> Result<(), Error> {
        let tx = facade.transactions_for(SERVICE_JOB)?;
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            let params = facade
                .find(JOB_PARAMETER)?
                .eq("jobName", Value::string(job_name))
                .list()?;
            for row in params.values() {
                row.delete(facade)?;
            }
            if let Some(job) = facade.find(SERVICE_JOB)?.eq("jobName", Value::string(job_name)).one()? {
                job.delete(facade)?;
            }
            Ok(())
        })?;
        Ok(())
    })();
    if let Err(err) = outcome {
        tracing::warn!(job = job_name, error = %err, "could not remove a queued job");
    }
}

fn create_run(
    facade: &EntityFacade,
    job_name: &str,
    user_id: Option<String>,
    parameters: &BTreeMap<String, Value>,
) -> Result<String, Error> {
    let tx = facade.transactions_for(JOB_RUN)?;
    let id = tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut run = facade.make_value(JOB_RUN)?;
        run.set_sequenced_id_primary(facade)?;
        run.set("jobName", Value::string(job_name));
        if let Some(user_id) = user_id {
            run.set("userId", Value::string(user_id));
        }
        if !parameters.is_empty() {
            let json = moqui_lib::json::to_json(&Value::Map(
                parameters.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ));
            run.set("parameters", Value::string(json.to_string()));
        }
        let id = field_text(&run, "jobRunId");
        run.create(facade)?;
        Ok(id)
    })?;
    Ok(id)
}

fn delete_run(facade: &EntityFacade, run_id: &str) {
    let outcome = (|| -> Result<(), Error> {
        if let Some(run) = facade.find(JOB_RUN)?.eq("jobRunId", Value::string(run_id)).one()? {
            run.delete(facade)?;
        }
        Ok(())
    })();
    if let Err(err) = outcome {
        tracing::warn!(run = run_id, error = %err, "could not remove an unused job run");
    }
}

/// Update fields of a run row. The run is history, so a failure is logged.
fn record(facade: &EntityFacade, run_id: &str, fields: &[(&str, Value)]) {
    let outcome = (|| -> Result<(), Error> {
        let tx = facade.transactions_for(JOB_RUN)?;
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            let mut run = facade.make_value(JOB_RUN)?;
            run.set("jobRunId", Value::string(run_id));
            for (name, value) in fields {
                run.set(*name, value.clone());
            }
            run.update(facade)?;
            Ok(())
        })?;
        Ok(())
    })();
    if let Err(err) = outcome {
        tracing::warn!(run = run_id, error = %err, "could not record a service job run");
    }
}

fn job_parameters(facade: &EntityFacade, job_name: &str) -> Result<BTreeMap<String, Value>, Error> {
    let rows = facade
        .find(JOB_PARAMETER)?
        .eq("jobName", Value::string(job_name))
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| {
            (
                field_text(row, "parameterName"),
                Value::string(field_text(row, "parameterValue")),
            )
        })
        .collect())
}

/// Stop a repeating job that has run `repeatCount` times. True when it was stopped.
fn repeat_exhausted(facade: &EntityFacade, job: &EntityValue, job_name: &str, now: &Value) -> Result<bool, Error> {
    let Some(limit) = number(job, "repeatCount") else {
        return Ok(false);
    };
    let runs = facade.find(JOB_RUN)?.eq("jobName", Value::string(job_name)).count()?;
    if runs < limit {
        return Ok(false);
    }
    let mut stop = facade.make_value(SERVICE_JOB)?;
    stop.set("jobName", Value::string(job_name));
    stop.set("paused", Value::string("Y"));
    stop.set("thruDate", now.clone());
    stop.update(facade)?;
    Ok(true)
}

fn within_dates(job: &EntityValue, now_ms: i64) -> bool {
    let from = millis_of(job.get("fromDate").unwrap_or(&Value::Null));
    let thru = millis_of(job.get("thruDate").unwrap_or(&Value::Null));
    // As Java's `filterByDate`, both ends are inclusive.
    from.is_none_or(|from| from <= now_ms) && thru.is_none_or(|thru| thru >= now_ms)
}

/// Active when `paused` is null or `N`; any other value, an empty one too, is paused, as Java's.
fn paused_ok(job: &EntityValue) -> bool {
    match job.get("paused") {
        None | Some(Value::Null) => true,
        Some(paused) => to_plain_string(paused) == "N",
    }
}

// ---- the lock row -------------------------------------------------------------------------

/// What the lock row looked like when the job was found due.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Expect {
    /// No row yet.
    Absent,
    /// A row nobody holds.
    Free,
    /// A row held by this run, whose hold has expired.
    Run(String),
}

struct LockPlan {
    tx: TransactionManager,
    insert_sql: String,
    claim_free_sql: String,
    claim_run_sql: String,
    release_sql: String,
    delete_sql: String,
}

impl LockPlan {
    fn new(facade: &EntityFacade) -> Result<Self, Error> {
        facade
            .with_group(JOB_LOCK, |entity, dialect, _, tx| {
                let by_job = Condition::bound("jobName", CompareOp::Equals);
                let free = Condition::and(vec![by_job.clone(), Condition::is_null("jobRunId")]);
                let held = Condition::and(vec![by_job.clone(), Condition::bound("jobRunId", CompareOp::Equals)]);
                let set = ["jobRunId", "lastRunTime"];
                Ok(Self {
                    tx: tx.clone(),
                    insert_sql: insert(entity, dialect, &["jobName", "jobRunId", "lastRunTime"])?,
                    claim_free_sql: update(entity, dialect, &set, Some(&free))?.sql,
                    claim_run_sql: update(entity, dialect, &set, Some(&held))?.sql,
                    release_sql: update(entity, dialect, &set, Some(&held))?.sql,
                    delete_sql: moqui_db::sql::delete(entity, dialect, Some(&by_job))?.sql,
                })
            })
            .map_err(Error::from)
    }

    /// True when this call took the lock. Every path is a conditional write, so only one caller wins.
    fn claim(&self, job: &str, run_id: &str, now: &Value, expect: &Expect) -> Result<bool, Error> {
        let job_value = Value::string(job);
        reserve(&self.tx, |conn| match expect {
            Expect::Absent => {
                let params = [job_value, Value::string(run_id), now.clone()];
                // A lost insert race is a held lock.
                Ok(conn.execute(&self.insert_sql, &params).is_ok())
            }
            Expect::Free => {
                let params = [Value::string(run_id), now.clone(), job_value];
                Ok(conn.execute(&self.claim_free_sql, &params)? > 0)
            }
            Expect::Run(old) => {
                let params = [Value::string(run_id), now.clone(), job_value, Value::string(old)];
                Ok(conn.execute(&self.claim_run_sql, &params)? > 0)
            }
        })
    }

    /// Clear `run_id`'s hold and set the run time. Does nothing if another run holds the lock now.
    fn release(&self, job: &str, run_id: &str, last_run: &Value) -> Result<(), Error> {
        let params = [Value::Null, last_run.clone(), Value::string(job), Value::string(run_id)];
        reserve(&self.tx, |conn| {
            conn.execute(&self.release_sql, &params)?;
            Ok(())
        })
    }

    fn delete_lock(&self, job: &str) -> Result<(), Error> {
        reserve(&self.tx, |conn| {
            conn.execute(&self.delete_sql, &[Value::string(job)])?;
            Ok(())
        })
    }
}

// ---- values -------------------------------------------------------------------------------

fn number(row: &EntityValue, field: &str) -> Option<i64> {
    match basic_convert(row.get(field)?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn millis_of(value: &Value) -> Option<i64> {
    match value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        Value::Null => None,
        other => to_plain_string(other).parse().ok(),
    }
}

fn timestamp_value(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

fn clip(text: &str) -> String {
    text.chars().take(TEXT_LIMIT).collect()
}

impl Shared {
    /// The factory this pool serves. `None` before boot attaches it or after it is dropped.
    pub(crate) fn factory(&self) -> Option<Arc<ExecutionContextFactory>> {
        lock(&self.factory).upgrade()
    }

    /// Room for more jobs, as Java's `ScheduledJobRunner`: `job-queue-max` plus `job-pool-max`, less the jobs running
    /// or waiting for a job worker.
    pub(crate) fn job_slots(&self) -> i64 {
        let state = lock(&self.state);
        let total = i64::try_from(self.job_queue_max + self.job_pool_max).unwrap_or(i64::MAX);
        let busy = i64::try_from(state.jobs_active + state.jobs.len()).unwrap_or(i64::MAX);
        total - busy
    }

    /// Queue a job run for the job workers. False when the job queue is full or the pool stopped.
    pub(crate) fn submit_job(&self, run: JobRun) -> bool {
        {
            let mut state = lock(&self.state);
            if state.stop || state.jobs.len() >= self.job_queue_max.max(self.job_pool_max) {
                return false;
            }
            state.jobs.push_back(Box::new(run));
        }
        self.job_wake.notify_all();
        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::Arc;
    use std::time::Duration;

    use moqui_lib::object::{to_plain_string, Value};

    use super::{abandon, timestamp_value, JobRun, RunEnd, JOB_LOCK, JOB_RUN};
    use crate::context::{ExecutionContext, ExecutionContextFactory};
    use crate::entity::{EntityFacade, EntityValue};

    const PREVIOUS: i64 = 1_700_000_000_000;

    /// A run whose lock row is claimed, as the pass leaves it before a worker takes the run.
    fn claimed(ctx: &ExecutionContext, facade: &EntityFacade, job: &str, run_id: &str) -> JobRun {
        let mut run = facade.make_value(JOB_RUN).unwrap();
        run.set("jobRunId", Value::string(run_id));
        run.set("jobName", Value::string(job));
        run.create(facade).unwrap();
        let mut lock = facade.make_value(JOB_LOCK).unwrap();
        lock.set("jobName", Value::string(job));
        lock.set("jobRunId", Value::string(run_id));
        lock.set("lastRunTime", super::now_stamp());
        lock.create(facade).unwrap();
        JobRun {
            job_name: job.to_string(),
            service_name: "never#Called".to_string(),
            parameters: BTreeMap::new(),
            timeout: Duration::from_secs(60),
            job_run_id: run_id.to_string(),
            last_run_time: timestamp_value(PREVIOUS),
            delete_when_done: false,
            user_name: None,
            origin: None,
            topic: None,
            description: None,
            factory: Arc::clone(ctx.factory()),
        }
    }

    fn one(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> EntityValue {
        facade
            .find(entity)
            .unwrap()
            .eq(field, Value::string(id))
            .one()
            .unwrap()
            .unwrap_or_else(|| panic!("no {entity} {id}"))
    }

    fn text(row: &EntityValue, field: &str) -> String {
        row.get(field).map(to_plain_string).unwrap_or_default()
    }

    /// The lock is free with the run time from before, and the run ended with an error.
    fn assert_released_and_failed(facade: &EntityFacade, job: &str, run_id: &str) {
        let lock = one(facade, JOB_LOCK, "jobName", job);
        assert_eq!(text(&lock, "jobRunId"), "");
        assert_eq!(
            super::millis_of(lock.get("lastRunTime").unwrap_or(&Value::Null)),
            Some(PREVIOUS)
        );
        let run = one(facade, JOB_RUN, "jobRunId", run_id);
        assert_eq!(text(&run, "hasError"), "Y");
        assert!(!text(&run, "endTime").is_empty());
    }

    #[test]
    fn a_run_that_unwinds_releases_its_lock_and_records_an_error() {
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let run = claimed(&ctx, &facade, "PanicJob", "R1");
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _end = RunEnd {
                facade: &facade,
                job: &run,
                done: false,
            };
            // A frame the service began and never ended: the records must not join it.
            assert!(facade.begin("moqui.test.TestEntity", None).unwrap());
            let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
            row.set("testId", Value::string("from-panicked-job"));
            row.create(&facade).unwrap();
            panic!("stand-in service panic");
        }));
        assert!(unwound.is_err());
        assert!(!crate::transaction::thread_in_transaction());
        assert_released_and_failed(&facade, "PanicJob", "R1");
        assert!(text(&one(&facade, JOB_RUN, "jobRunId", "R1"), "errors").contains("panicked"));
        let kept = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("from-panicked-job"))
            .one()
            .unwrap();
        assert!(kept.is_none(), "the panicked job's frame was committed");
    }

    /// As Java's `filterByDate` and its paused check: both date ends are inclusive, and a `paused` that is not null or
    /// `N`, an empty one too, pauses the job.
    #[test]
    fn job_dates_are_inclusive_and_an_empty_paused_pauses() {
        let facade = crate::testing::open();
        let mut job = facade.make_value(super::SERVICE_JOB).unwrap();
        assert!(super::paused_ok(&job));
        job.set("paused", Value::string("N"));
        assert!(super::paused_ok(&job));
        for paused in ["", "Y", "T"] {
            job.set("paused", Value::string(paused));
            assert!(!super::paused_ok(&job), "paused {paused:?}");
        }
        job.set("fromDate", timestamp_value(PREVIOUS));
        job.set("thruDate", timestamp_value(PREVIOUS + 1_000));
        assert!(super::within_dates(&job, PREVIOUS));
        assert!(super::within_dates(&job, PREVIOUS + 1_000));
        assert!(!super::within_dates(&job, PREVIOUS - 1));
        assert!(!super::within_dates(&job, PREVIOUS + 1_001));
    }

    #[test]
    fn a_run_dropped_at_shutdown_releases_its_lock() {
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let run = {
            let _entered = ctx.enter();
            let _authz = ctx.disable_authz();
            claimed(&ctx, &facade, "DroppedJob", "R2")
        };
        abandon(run);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        assert_released_and_failed(&facade, "DroppedJob", "R2");
    }
}
