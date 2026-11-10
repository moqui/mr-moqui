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
//! A job with `errorHistoryOnly` Y removes that row when the run succeeds. Java keeps every run.
//!
//! A pass claims locks only for job workers that are idle, and a lock records the instance that
//! holds it. A lock whose instance is gone (not a live cluster member, or this instance with no run
//! of this process holding it) is taken over before `expireLockTime`, and a worker checks that its
//! run still holds the lock before it runs the service (`plans/High-Level-Review.md` HL-026).
//!
//! A cron expression is read in the machine's time zone ([`job_time_zone`]), the zone the operating
//! system reports. That is not the server zone `default_time_zone` names (UTC in MoquiDefaultConf).
//! The pass still skips a job only when the previous occurrence is strictly before the last run, so
//! a last run at that occurrence still runs, as Java's `!lastSchedule.isBefore(lastRunDt)`.
//!
//! A fire-and-forget call, or the tx rules of a committed call, that finds the pool queue full or
//! stopping is stored as a `ServiceJob` named with [`QUEUE_PREFIX`] and no cron expression, the call
//! as JSON in `queuedCall` ([`QueuedCall`]), so lists and maps keep their shape (HL-025). The pass
//! submits it when a slot is free and deletes it when it has run.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
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
/// The `ServiceJobParameter` names a queued call stored before `queuedCall` kept its caller in: the user it runs as and
/// the agent run it works for (the origin of its writes, which the agent triggers' loop guard reads). They are not
/// passed to the service.
const QUEUED_USERNAME: &str = "_moquiUsername";
const QUEUED_AGENT_ID: &str = "_originAgentId";
const QUEUED_RUN_ID: &str = "_originRunId";

const SERVICE_JOB: &str = "moqui.service.job.ServiceJob";
const JOB_PARAMETER: &str = "moqui.service.job.ServiceJobParameter";
const JOB_RUN: &str = "moqui.service.job.ServiceJobRun";
const JOB_LOCK: &str = "moqui.service.job.ServiceJobRunLock";
const DEFAULT_EXPIRE_MINUTES: i64 = 1440;
const DEFAULT_RETRY_MINUTES: i64 = 5;
const DEFAULT_JOB_TIMEOUT_SECONDS: u64 = 1800;
const TEXT_LIMIT: usize = 4000;

/// A job run in progress on this thread: its `jobRunId` and `jobName`.
type CurrentJobRun = Option<(String, String)>;

thread_local! {
    /// The `ServiceJobRun` whose service is running on this thread, while it runs.
    static CURRENT_JOB_RUN: std::cell::RefCell<CurrentJobRun> = const { std::cell::RefCell::new(None) };
}

/// Puts back the job run that was current before, when dropped.
struct JobRunScope(CurrentJobRun);

impl JobRunScope {
    fn enter(job_run_id: &str, job_name: &str) -> Self {
        Self(CURRENT_JOB_RUN.with(|slot| slot.replace(Some((job_run_id.to_string(), job_name.to_string())))))
    }
}

impl Drop for JobRunScope {
    fn drop(&mut self) {
        let previous = self.0.take();
        CURRENT_JOB_RUN.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// The `jobRunId` of the `ServiceJobRun` whose service is running on this thread: set while a job's service is called,
/// so a service can tell it runs as a job (an ETL run records it on its `EtlRun`). None outside a job.
#[must_use]
pub fn current_job_run_id() -> Option<String> {
    CURRENT_JOB_RUN.with(|slot| slot.borrow().as_ref().map(|(run_id, _)| run_id.clone()))
}

/// Run `body` as the service of the job run `job_run_id` of `job_name` would run, for [`current_job_run_id`] and
/// [`current_job_name`]: for a test that makes more than one call in one job's context.
#[doc(hidden)]
pub fn in_job_run<T>(job_run_id: &str, job_name: &str, body: impl FnOnce() -> T) -> T {
    let _job_run = JobRunScope::enter(job_run_id, job_name);
    body()
}

/// The `jobName` of the ServiceJob whose service is running on this thread, as [`current_job_run_id`].
#[must_use]
pub fn current_job_name() -> Option<String> {
    CURRENT_JOB_RUN.with(|slot| slot.borrow().as_ref().map(|(_, job_name)| job_name.clone()))
}

/// The time zone a ServiceJob cron expression is read in: the machine's zone, the one the operating
/// system reports. It is not the server zone `default_time_zone` names.
pub fn job_time_zone() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::system()
}

/// A job handed to a worker.
pub(crate) struct JobRun {
    job_name: String,
    service_name: String,
    parameters: BTreeMap<String, Value>,
    timeout: Duration,
    /// The run's scripts end when it is out ([`crate::script::DeadlineGuard`]), whatever the service's transaction
    /// attribute; `None` when the job says `noTimeLimit` (plans/High-Level-Review.md HL-035).
    script_limit: Option<Duration>,
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
    /// `ServiceJob.errorHistoryOnly`: a success deletes its `ServiceJobRun`.
    error_history_only: bool,
    /// The job has a schedule, so a run that fails is retried by a later one: its operation's rollback is `retry`.
    retries: bool,
    /// A queued run of tx rules: the rules of `service_name` run, with `parameters` and these results.
    rules: Option<QueuedRules>,
    /// The lock this run claimed, which the worker checks it still holds before it runs. None for an explicit run.
    claim: Option<LocalClaim>,
    factory: Arc<ExecutionContextFactory>,
}

/// The job runs this process claimed and has not finished, so a lock held under this instance's id but by none of
/// them (left by an earlier process of this instance) can be taken over.
#[derive(Default)]
struct LocalRuns(Mutex<HashSet<String>>);

/// One claimed run in [`LocalRuns`], until it drops.
pub(crate) struct LocalClaim {
    runs: Arc<LocalRuns>,
    run_id: String,
}

impl LocalClaim {
    fn new(factory: &ExecutionContextFactory, run_id: &str) -> Self {
        let runs = factory.extension(|_| LocalRuns::default());
        lock(&runs.0).insert(run_id.to_string());
        Self {
            runs,
            run_id: run_id.to_string(),
        }
    }
}

impl Drop for LocalClaim {
    fn drop(&mut self) {
        lock(&self.runs.0).remove(&self.run_id);
    }
}

/// This instance's id, which a lock it claims records: the cluster member id, else the instance id telemetry names.
fn own_instance(factory: &ExecutionContextFactory) -> String {
    crate::cluster::instance_id(factory).unwrap_or_else(|| crate::drain::instance_id(factory))
}

/// The instance holding `lock_row` for `run_id` when that instance is gone: another member whose heartbeat is outside
/// the liveness window (or that has no member row), or this instance when no run of this process holds it (it was left
/// by an earlier process). None while the holder may still be running it, and for a lock that names no instance.
fn gone_owner(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    lock_row: &EntityValue,
    run_id: &str,
) -> Option<String> {
    let owner = field_text(lock_row, "instanceId");
    if owner.is_empty() {
        return None;
    }
    if owner == own_instance(factory) {
        let local = factory
            .extension_if_present::<LocalRuns>()
            .is_some_and(|runs| lock(&runs.0).contains(run_id));
        return (!local).then_some(owner);
    }
    match crate::cluster::is_live_member(factory, facade, &owner) {
        Ok(true) => None,
        Ok(false) => Some(owner),
        Err(err) => {
            tracing::warn!(instance = %owner, error = %err, "could not read whether a job lock's holder is live");
            None
        }
    }
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
    /// Jobs not checked for readiness because no execution slot was left: their lock and history are not read.
    pub jobs_not_checked: usize,
    /// Jobs that could not be checked or started; each is logged and the pass goes on.
    pub jobs_failed: usize,
}

/// One scheduler pass for the pool's factory. Errors are logged, so a bad pass never ends the thread.
pub(crate) fn run_scheduled(shared: &Shared) -> PassReport {
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

fn pass(shared: &Shared, factory: &Arc<ExecutionContextFactory>) -> Result<PassReport, Error> {
    let mut report = PassReport::default();
    let Some(facade) = factory.entity() else {
        return Ok(report);
    };
    let mut slots = shared.job_slots();
    let now = jiff::Timestamp::now();
    let now_stamp = timestamp_value(now.as_millisecond());
    let plan = LockPlan::new(&facade)?;

    let jobs = discover(&facade, slots)?;
    for job in &jobs {
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
        // With no slot left nothing more can start, so the lock and history queries are not made: under a saturated
        // queue they would cost a query per queued call on every pass (`plans/High-Level-Review.md` HL-020). Jobs come
        // in priority order, so those left are the lower ones, checked on a later pass.
        if slots <= 0 {
            report.jobs_not_checked += 1;
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
        let ready = match readiness(factory, &facade, job, &job_name, &cron_text, now) {
            Ok(Some(ready)) => ready,
            Ok(None) => continue,
            Err(err) => {
                tracing::error!(job = %job_name, cron = %cron_text, error = %err, "Error checking scheduled job, skipping it");
                report.jobs_failed += 1;
                continue;
            }
        };
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

/// The jobs a pass looks at, in priority order: every scheduled job (one with a cron expression), and the queued calls
/// the free slots can take, read a bounded number at a time (plans/High-Level-Review.md HL-026): a full queue can leave
/// many `ASYNC_QUEUE_` rows, which a later pass reads when slots are free. Priority is lower first, a job with none after
/// every job with one, then by name: the same on every database (an `ORDER BY` puts nulls first on some and last on
/// others).
pub(crate) fn discover(facade: &EntityFacade, slots: i64) -> Result<Vec<EntityValue>, Error> {
    let mut jobs: Vec<EntityValue> = facade
        .find(SERVICE_JOB)?
        .condition(Condition::is_not_null("cronExpression"))
        .list()?
        .into_values();
    jobs.retain(|job| !field_text(job, "jobName").starts_with(QUEUE_PREFIX));
    let mut wanted = usize::try_from(slots).unwrap_or(0);
    let pattern = Value::string(format!("{QUEUE_PREFIX}%"));
    // Those with a priority first, then those with none.
    for (with_priority, field) in [(true, "priority"), (false, "jobName")] {
        if wanted == 0 {
            break;
        }
        let priority = if with_priority {
            Condition::is_not_null("priority")
        } else {
            Condition::is_null("priority")
        };
        let queued = facade
            .find(SERVICE_JOB)?
            .condition(Condition::like("jobName", pattern.clone()))
            .condition(priority)
            .order_by(field, false)
            .order_by("jobName", false)
            .limit(i64::try_from(wanted).unwrap_or(i64::MAX))
            .list()?
            .into_values();
        wanted = wanted.saturating_sub(queued.len());
        jobs.extend(
            queued
                .into_iter()
                .filter(|job| field_text(job, "jobName").starts_with(QUEUE_PREFIX)),
        );
    }
    jobs.sort_by(|left, right| {
        let key = |job: &EntityValue| {
            (
                number(job, "priority").map_or((1, 0), |priority| (0, priority)),
                field_text(job, "jobName"),
            )
        };
        key(left).cmp(&key(right))
    });
    Ok(jobs)
}

/// What the lock row allows this pass to do for a job that is otherwise due.
struct Ready {
    expect: Expect,
    last_run_time: Value,
    /// The run whose lock is taken over because its instance is gone.
    reclaimed: Option<String>,
}

/// `None` when the job is not due, is running, or failed too recently.
fn readiness(
    factory: &ExecutionContextFactory,
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
    let mut reclaimed = None;
    if let Some(lock) = &lock {
        let run_id = field_text(lock, "jobRunId");
        last_run_time = lock.get("lastRunTime").cloned().unwrap_or(Value::Null);
        last_run_ms = millis_of(&last_run_time);
        expect = if run_id.is_empty() {
            Expect::Free
        } else {
            Expect::Run(run_id)
        };
        if let Expect::Run(run_id) = &expect {
            // `expireLockTime` stays the upper bound; a lock whose instance is gone is taken over sooner (HL-026). A
            // run id with no time has nothing to say the lock is old, so only a gone instance frees it.
            let expire = number(job, "expireLockTime").unwrap_or(DEFAULT_EXPIRE_MINUTES);
            let expired = last_run_ms
                .is_some_and(|last| last < now.as_millisecond().saturating_sub(expire.saturating_mul(60_000)));
            if expired {
                tracing::warn!(job = job_name, run = %run_id, "lock expired, ignoring it");
            } else if let Some(owner) = gone_owner(factory, facade, lock, run_id) {
                tracing::warn!(job = job_name, run = %run_id, instance = %owner, "lock holder is gone, taking the lock over");
                reclaimed = Some(run_id.clone());
            } else {
                tracing::info!(job = job_name, run = %run_id, "lock held, not running");
                return Ok(None);
            }
        }
    }
    if !cron_text.is_empty() {
        let cron = super::schedule::cached_cron(cron_text)?;
        let zoned = now.to_zoned(job_time_zone());
        // A schedule with nothing before now has not come due yet.
        let Some(last_schedule) = cron.last_at_or_before(&zoned) else {
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
                // A success that was not kept (`errorHistoryOnly`) moves lastRunTime past this error, so the old
                // error row does not delay the next occurrence. An error restores lastRunTime to the time before
                // it, which is before the error, so the delay still holds until minRetryTime has passed.
                let ran_since = last_run_ms.is_some_and(|last_run| last_run >= stamp);
                if !ran_since && stamp > now.as_millisecond().saturating_sub(retry.saturating_mul(60_000)) {
                    tracing::info!(job = job_name, "not retrying after an error yet");
                    return Ok(None);
                }
            }
        }
    }
    Ok(Some(Ready {
        expect,
        last_run_time,
        reclaimed,
    }))
}

fn dispatch(
    shared: &Shared,
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    plan: &LockPlan,
    job: &EntityValue,
    ready: Ready,
    now_stamp: &Value,
) -> Result<bool, Error> {
    let job_name = field_text(job, "jobName");
    let history_on_error = error_history_only(job);
    let run_id = create_run(facade, &job_name, None, &BTreeMap::new())?;
    // Counted as this process's before the claim, so a pass never takes it for one an earlier process left.
    let claim = LocalClaim::new(factory, &run_id);
    if !plan.claim(&job_name, &run_id, now_stamp, &ready.expect, &own_instance(factory))? {
        // Another instance took it. Its run row would only confuse the history.
        delete_run(facade, &run_id);
        return Ok(false);
    }
    if let Some(old) = &ready.reclaimed {
        record(
            facade,
            old,
            &[
                ("endTime", now_stamp.clone()),
                ("hasError", Value::string("Y")),
                (
                    "errors",
                    Value::string("Its server instance stopped before it ended; the job's lock was taken over"),
                ),
            ],
        );
    }
    // Before the worker is queued, so the count includes this row. A submit that fails keeps the row as a failed
    // start, which counts too. A claim that lost the race returned above and does not count.
    if history_on_error {
        if let Err(err) = note_started(facade, &job_name) {
            tracing::warn!(job = %job_name, error = %err, "could not count a service job run");
        }
    }
    let queued = job_name.starts_with(QUEUE_PREFIX);
    let stored_call = field_text(job, "queuedCall");
    let submitted = if queued && !stored_call.is_empty() {
        QueuedCall::parse(&stored_call).map(|call| (call.parameters, call.user_name, call.origin, call.rules))
    } else {
        job_parameters(facade, &job_name).map(|mut parameters| {
            let (user_name, origin) = if queued {
                take_caller(&mut parameters)
            } else {
                (None, None)
            };
            (parameters, user_name, origin, None)
        })
    };
    let submitted = submitted.and_then(|(parameters, user_name, origin, rules)| {
        let run = JobRun {
            job_name: job_name.clone(),
            service_name: field_text(job, "serviceName"),
            parameters,
            timeout: job_timeout(job),
            script_limit: script_limit(job),
            job_run_id: run_id.clone(),
            last_run_time: ready.last_run_time,
            delete_when_done: queued,
            user_name,
            origin,
            topic: Some(field_text(job, "topic")).filter(|topic| !topic.is_empty()),
            description: Some(field_text(job, "description")).filter(|text| !text.is_empty()),
            error_history_only: history_on_error,
            retries: !queued && !field_text(job, "cronExpression").is_empty(),
            rules,
            claim: Some(claim),
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

/// A call stored as a queued job (`ServiceJob.queuedCall`) because the service queue was full or stopping: a
/// fire-and-forget call, or the tx rules of a call that committed. The parameters and results keep their lists and
/// maps, as JSON (HL-025).
pub(crate) struct QueuedCall {
    pub(crate) service: String,
    pub(crate) parameters: BTreeMap<String, Value>,
    /// The user it runs as and the agent run it works for (the agent triggers' loop guard).
    pub(crate) user_name: Option<String>,
    pub(crate) origin: Option<crate::context::AgentOrigin>,
    pub(crate) rules: Option<QueuedRules>,
}

/// The phase and results of queued tx rules, and the authorization they run under.
#[derive(Clone, Debug)]
pub(crate) struct QueuedRules {
    pub(crate) when: String,
    pub(crate) results: Option<BTreeMap<String, Value>>,
    pub(crate) authz: crate::user::RegisteredAuthz,
}

fn json_map(map: &BTreeMap<String, Value>) -> serde_json::Value {
    moqui_lib::json::to_json(&Value::Map(
        map.iter()
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    ))
}

fn map_json(json: Option<&serde_json::Value>) -> Option<BTreeMap<String, Value>> {
    match moqui_lib::json::from_json(json?) {
        Value::Map(entries) => Some(entries.into_iter().collect()),
        _ => None,
    }
}

impl QueuedCall {
    fn to_json(&self) -> String {
        let mut root = serde_json::Map::new();
        root.insert("parameters".into(), json_map(&self.parameters));
        if let Some(user_name) = self.user_name.as_deref().filter(|name| !name.is_empty()) {
            root.insert("userName".into(), user_name.into());
        }
        if let Some(origin) = &self.origin {
            root.insert("agentId".into(), origin.agent_id.clone().into());
            if let Some(run_id) = &origin.run_id {
                root.insert("agentRunId".into(), run_id.clone().into());
            }
        }
        if let Some(rules) = &self.rules {
            root.insert("when".into(), rules.when.clone().into());
            if let Some(results) = &rules.results {
                root.insert("results".into(), json_map(results));
            }
            root.insert("authz".into(), rules.authz.to_json());
        }
        serde_json::Value::Object(root).to_string()
    }

    /// The call stored in `text`. The service is the job's `serviceName`, so it is left empty here.
    fn parse(text: &str) -> Result<Self, Error> {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|err| Error::new(format!("queued call is not JSON: {err}")))?;
        let field = |name: &str| json.get(name).and_then(serde_json::Value::as_str).map(str::to_string);
        Ok(Self {
            service: String::new(),
            parameters: map_json(json.get("parameters")).unwrap_or_default(),
            user_name: field("userName"),
            origin: field("agentId").map(|agent_id| crate::context::AgentOrigin {
                agent_id,
                run_id: field("agentRunId"),
            }),
            rules: field("when").map(|when| QueuedRules {
                when,
                results: map_json(json.get("results")),
                authz: crate::user::RegisteredAuthz::from_json(json.get("authz")),
            }),
        })
    }
}

/// Store a call the pool queue had no room for, or did not run before it stopped, as a queued job with its caller.
/// The pass will submit it.
pub(crate) fn persist_queued(factory: &Arc<ExecutionContextFactory>, call: &QueuedCall) -> Result<(), Error> {
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("service worker queue is full and there is no database to queue in"))?;
    // A sequence number makes the name unique without inserting anything.
    let mut numbered = facade.make_value(JOB_RUN)?;
    numbered.set_sequenced_id_primary(&facade)?;
    let id = field_text(&numbered, "jobRunId");
    let name = format!("{QUEUE_PREFIX}{id}");
    let description = match &call.rules {
        Some(rules) => format!("Queued {} rules of {}", rules.when, call.service),
        None => format!("Queued call of {}", call.service),
    };
    let _authz = ExecutionContext::with_current(|ctx| ctx.disable_authz());
    let tx = facade.transactions_for(SERVICE_JOB)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut job = facade.make_value(SERVICE_JOB)?;
        job.set("jobName", Value::string(&name));
        job.set("description", Value::string(&description));
        job.set("serviceName", Value::string(&call.service));
        job.set("paused", Value::string("N"));
        // Queued calls go after the jobs people scheduled.
        job.set("priority", Value::I32(9));
        job.set("queuedCall", Value::string(call.to_json()));
        job.create(&facade)?;
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
    let history_on_error = error_history_only(&job);
    let run_id = create_run(
        &facade,
        job_name,
        crate::user::current_user_id().ok().flatten(),
        &parameters,
    )?;
    // Before the run is queued. An inline run can finish, and remove a success row, at the next drain. A submit
    // that fails deletes the row, so this count is one high on that path.
    if history_on_error {
        if let Err(err) = note_started(&facade, job_name) {
            tracing::warn!(job = %job_name, error = %err, "could not count a service job run");
        }
    }
    let pools = factory.service_pools();
    let shared = pools.shared.clone();
    let run = JobRun {
        job_name: job_name.to_string(),
        service_name: field_text(&job, "serviceName"),
        parameters,
        timeout: job_timeout(&job),
        script_limit: script_limit(&job),
        job_run_id: run_id.clone(),
        last_run_time: Value::Null,
        delete_when_done: false,
        user_name: crate::user::current_username().ok().flatten(),
        origin: super::pool::current_origin(),
        topic: Some(field_text(&job, "topic")).filter(|topic| !topic.is_empty()),
        description: Some(field_text(&job, "description")).filter(|text| !text.is_empty()),
        error_history_only: history_on_error,
        retries: !field_text(&job, "cronExpression").is_empty(),
        rules: None,
        claim: None,
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
    let telemetry = crate::telemetry::for_factory(&factory);
    let mut span = telemetry.start_span(&job.job_name, crate::telemetry::SpanKind::Internal, None);
    span.set_attribute("moqui.job.root", Value::Bool(true));
    span.set_attribute("moqui.job.name", Value::string(&job.job_name));
    span.set_attribute("moqui.job.run.id", Value::string(&job.job_run_id));
    let trace_context = span.context();
    let _trace = crate::telemetry::attach(trace_context.clone());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    // a run-as user whose account is disabled for good, past its end date or gone does not run (impl-admin F42): the run
    // is recorded with the refusal instead of running with no user
    let mut refused_user = None;
    if let Some(user) = &job.user_name {
        match crate::user::username_account_closed(user) {
            Ok(true) => refused_user = Some(format!("the job's user {user} cannot sign in; the job did not run")),
            Ok(false) => {}
            Err(err) => tracing::warn!(job = %job.job_name, error = %err, "could not check the job's user"),
        }
        if refused_user.is_none() {
            if let Err(err) = crate::user::internal_login_user(user) {
                tracing::warn!(job = %job.job_name, error = %err, "could not log in the job's user");
            }
        }
    }
    context.set_agent_origin(job.origin.clone());
    let Some(facade) = factory.entity() else {
        return;
    };
    // The run waited for a worker since its lock was claimed; an instance that took the lock over meanwhile runs the
    // job (HL-026). The lock is not this run's to release, nor a queued call's row to delete.
    if job.claim.is_some() && !holds_lock(&facade, &job) {
        tracing::warn!(job = %job.job_name, run = %job.job_run_id, "the job's lock was taken over before the run started; not running it");
        delete_run(&facade, &job.job_run_id);
        return;
    }
    // The thread's name is this port's (`moqui-job-0`, not Java's `MoquiJob-1`), an accepted difference (P7b-16).
    let thread = std::thread::current().name().unwrap_or("").to_string();
    let (host_address, host_name) = crate::context::identity::server_address();
    record(
        &facade,
        &job.job_run_id,
        &[
            (
                "traceId",
                trace_context
                    .as_ref()
                    .map_or(Value::Null, |context| Value::string(context.trace_id.to_string())),
            ),
            ("hostAddress", Value::string(host_address)),
            ("hostName", Value::string(host_name)),
            ("instanceId", Value::string(own_instance(&factory))),
            ("runThread", Value::string(thread)),
            ("startTime", now_stamp()),
        ],
    );
    let end = RunEnd {
        facade: &facade,
        job: &job,
        telemetry: &telemetry,
        done: false,
    };
    let mut parameters = job.parameters.clone();
    if !job.last_run_time.is_null() {
        parameters.insert("lastRunTime".to_string(), job.last_run_time.clone());
    }
    let options = CallOptions {
        disable_authz: true,
        timeout: Some(job.timeout),
        operation_retry: job.retries,
        ..CallOptions::default()
    };
    // The job's time bounds its scripts even when the service begins no transaction; a transaction inside keeps the
    // earlier of the two.
    let _deadline = crate::script::DeadlineGuard::new(job.script_limit);
    let _job_run = JobRunScope::enter(&job.job_run_id, &job.job_name);
    let called = match &job.rules {
        _ if refused_user.is_some() => Err(Error::new(refused_user.clone().unwrap_or_default())),
        Some(rules) => {
            let mut rules = super::pool::TxRules {
                service: job.service_name.clone(),
                when: rules.when.clone(),
                parameters: job.parameters.clone(),
                results: rules.results.clone(),
                // Under the authorization of the call that registered them, not the job's disabled authorization.
                authz: rules.authz.clone(),
            };
            super::call::run_tx_rules(&factory, &mut rules).map(|()| BTreeMap::new())
        }
        None => super::call::sync_with(&job.service_name, &parameters, &options),
    };
    let results = match called {
        Ok(results) => results,
        Err(err) => {
            tracing::error!(job = %job.job_name, error = %err, "error in service job call");
            context.message().add_error(err.to_string());
            BTreeMap::new()
        }
    };
    let (has_error, messages, errors, first_error) = {
        let messages = context.message();
        (
            !messages.errors().is_empty(),
            clip(&messages.messages().join("\n")),
            clip(&messages.errors().join("\n")),
            messages.errors().first().cloned(),
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
    count_job_run(&telemetry, if has_error { "error" } else { "success" });
    // The job's first error, so its error group is the failure's, not one group for every failed job.
    if let Some(error) = first_error {
        span.set_error(&error);
    }
    span.finish();
}

/// `moqui.job.runs` by outcome: success, error, or panic.
fn count_job_run(telemetry: &crate::telemetry::Telemetry, outcome: &str) {
    let mut attributes = crate::entity::document::Document::new();
    attributes.insert("outcome".into(), Value::string(outcome));
    telemetry.add_counter("moqui.job.runs", 1, attributes);
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
    telemetry: &'a crate::telemetry::Telemetry,
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
        // The job's span records the panic as it unwinds.
        count_job_run(self.telemetry, "panic");
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
    // `errorHistoryOnly`: a success is not history. The row existed so the lock could point at it and Job Detail
    // could open the active run; a success removes it. Java keeps every run. An error row stays, so the retry delay
    // and the job error screen still have it. The lock's lastRunTime, set above, is what says the job ran.
    let forget_run = job.error_history_only && !outcome.has_error;
    let end_fields = [
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
    ];
    // The run's end and a queued call's removal in one transaction-cache frame, one commit batch (plans/archive/TX-Cache.md
    // F2). Each is history or cleanup on its own: when the batch fails they are done one at a time, each failure logged.
    if let Err(err) = finish_records(facade, job, forget_run, &end_fields) {
        tracing::debug!(job = %job.job_name, run = %job.job_run_id, error = %err, "job run end batch failed; writing each");
        if forget_run {
            delete_run(facade, &job.job_run_id);
        } else {
            record(facade, &job.job_run_id, &end_fields);
        }
        if job.delete_when_done {
            delete_queued(facade, &job.job_name);
        }
    }
    if outcome.has_error {
        tracing::warn!(job = %job.job_name, run = %job.job_run_id, errors = %outcome.errors, "service job ended with errors");
    }
    send_job_notifications(facade, job, outcome);
    if job.delete_when_done {
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

/// Whether `job`'s run still holds its lock. A lock that cannot be read counts as held: it was this run's when claimed.
fn holds_lock(facade: &EntityFacade, job: &JobRun) -> bool {
    match facade
        .find(JOB_LOCK)
        .and_then(|find| find.eq("jobName", Value::string(&job.job_name)).one())
    {
        Ok(row) => row.is_some_and(|row| field_text(&row, "jobRunId") == job.job_run_id),
        Err(err) => {
            tracing::warn!(job = %job.job_name, run = %job.job_run_id, error = %err, "could not recheck the job lock");
            true
        }
    }
}

/// Clear this run's hold on the job lock and set `lastRunTime`. A failure is logged.
fn release_lock(facade: &EntityFacade, job: &JobRun, last_run: &Value) {
    if let Err(err) = LockPlan::new(facade).and_then(|plan| plan.release(&job.job_name, &job.job_run_id, last_run)) {
        tracing::warn!(job = %job.job_name, run = %job.job_run_id, error = %err, "could not clear the job lock");
    }
}

/// The run's end (its fields, or its row removed) and a queued call's job and parameters removed, in one
/// transaction-cache frame. The rows are removed by key, with no read first.
fn finish_records(
    facade: &EntityFacade,
    job: &JobRun,
    forget_run: bool,
    end_fields: &[(&str, Value)],
) -> Result<(), Error> {
    let tx = facade.transactions_for(JOB_RUN)?;
    tx.run(TransactionAttribute::ForceCache, None, || {
        let mut run = facade.make_value(JOB_RUN)?;
        run.set("jobRunId", Value::string(&job.job_run_id));
        if forget_run {
            run.delete(facade)?;
        } else {
            for (name, value) in end_fields {
                run.set(*name, value.clone());
            }
            run.update(facade)?;
        }
        if job.delete_when_done {
            facade
                .find(JOB_PARAMETER)?
                .eq("jobName", Value::string(&job.job_name))
                .delete_all()?;
            let mut queued = facade.make_value(SERVICE_JOB)?;
            queued.set("jobName", Value::string(&job.job_name));
            queued.delete(facade)?;
        }
        Ok(())
    })?;
    Ok(())
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

/// Take a queued call's caller out of its job parameters: the user it runs as and the agent run it works for.
fn take_caller(parameters: &mut BTreeMap<String, Value>) -> (Option<String>, Option<crate::context::AgentOrigin>) {
    let mut take = |name: &str| {
        parameters
            .remove(name)
            .map(|value| to_plain_string(&value))
            .filter(|text| !text.is_empty())
    };
    let user_name = take(QUEUED_USERNAME);
    let agent_id = take(QUEUED_AGENT_ID);
    let run_id = take(QUEUED_RUN_ID);
    let origin = agent_id.map(|agent_id| crate::context::AgentOrigin { agent_id, run_id });
    (user_name, origin)
}

/// True only when `errorHistoryOnly` is Y. Empty or N keeps every run.
fn error_history_only(job: &EntityValue) -> bool {
    field_text(job, "errorHistoryOnly") == "Y"
}

/// The lock's `runCount`, when the row and the count are there.
fn stored_run_count(facade: &EntityFacade, job_name: &str) -> Result<Option<i64>, Error> {
    let lock = facade.find(JOB_LOCK)?.eq("jobName", Value::string(job_name)).one()?;
    Ok(lock.as_ref().and_then(|row| number(row, "runCount")))
}

/// Count a run whose success row will not stay. Called after the run row exists and, for a scheduled run, after the
/// lock is held.
///
/// An empty `runCount` becomes the number of `ServiceJobRun` rows, which still includes this run and any history from
/// before the flag. After that it is one more than the stored count. A lost update retries. The caller logs a failure
/// and the run still goes on.
fn note_started(facade: &EntityFacade, job_name: &str) -> Result<(), Error> {
    let plan = LockPlan::new(facade)?;
    for _ in 0..8 {
        let rows = facade.find(JOB_RUN)?.eq("jobName", Value::string(job_name)).count()?;
        let lock = facade.find(JOB_LOCK)?.eq("jobName", Value::string(job_name)).one()?;
        let Some(lock) = lock else {
            // No lock yet, an explicit run. A duplicate primary key means another run inserted the row.
            if plan.insert_count(job_name, rows.max(1))? {
                return Ok(());
            }
            continue;
        };
        let current = number(&lock, "runCount");
        // The new row is in `rows`. A count of zero means it was not visible yet; this run is still one.
        let next = match current {
            Some(count) => count.saturating_add(1),
            None => rows.max(1),
        };
        if plan.advance_count(job_name, current, next)? {
            return Ok(());
        }
    }
    Err(Error::new(format!("could not count a run of {job_name}")))
}

/// Stop a repeating job that has run `repeatCount` times. True when it was stopped.
fn repeat_exhausted(facade: &EntityFacade, job: &EntityValue, job_name: &str, now: &Value) -> Result<bool, Error> {
    let Some(limit) = number(job, "repeatCount") else {
        return Ok(false);
    };
    // A job that drops its success rows cannot be counted from `ServiceJobRun`. An empty `runCount` means the flag
    // is new and the rows still there are the history, so they decide. This does not insert a lock: the run that is
    // accepted writes the count, after its row exists.
    let runs = if error_history_only(job) {
        match stored_run_count(facade, job_name)? {
            Some(count) => count,
            None => facade.find(JOB_RUN)?.eq("jobName", Value::string(job_name)).count()?,
        }
    } else {
        facade.find(JOB_RUN)?.eq("jobName", Value::string(job_name)).count()?
    };
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
    /// `SET runCount = ? WHERE jobName = ? AND runCount IS NULL`.
    count_seed_sql: String,
    /// `SET runCount = ? WHERE jobName = ? AND runCount = ?`.
    count_step_sql: String,
    /// `INSERT (jobName, runCount)`.
    count_insert_sql: String,
}

impl LockPlan {
    fn new(facade: &EntityFacade) -> Result<Self, Error> {
        facade
            .with_group(JOB_LOCK, |entity, dialect, _, tx| {
                let by_job = Condition::bound("jobName", CompareOp::Equals);
                let free = Condition::and(vec![by_job.clone(), Condition::is_null("jobRunId")]);
                let held = Condition::and(vec![by_job.clone(), Condition::bound("jobRunId", CompareOp::Equals)]);
                let set = ["jobRunId", "lastRunTime", "instanceId"];
                let seed = Condition::and(vec![by_job.clone(), Condition::is_null("runCount")]);
                let step = Condition::and(vec![by_job.clone(), Condition::bound("runCount", CompareOp::Equals)]);
                Ok(Self {
                    tx: tx.clone(),
                    insert_sql: insert(entity, dialect, &["jobName", "jobRunId", "lastRunTime", "instanceId"])?,
                    claim_free_sql: update(entity, dialect, &set, Some(&free))?.sql,
                    claim_run_sql: update(entity, dialect, &set, Some(&held))?.sql,
                    release_sql: update(entity, dialect, &set, Some(&held))?.sql,
                    delete_sql: moqui_db::sql::delete(entity, dialect, Some(&by_job))?.sql,
                    count_seed_sql: update(entity, dialect, &["runCount"], Some(&seed))?.sql,
                    count_step_sql: update(entity, dialect, &["runCount"], Some(&step))?.sql,
                    count_insert_sql: insert(entity, dialect, &["jobName", "runCount"])?,
                })
            })
            .map_err(Error::from)
    }

    /// True when this call took the lock for `instance`. Every path is a conditional write, so only one caller wins.
    fn claim(&self, job: &str, run_id: &str, now: &Value, expect: &Expect, instance: &str) -> Result<bool, Error> {
        let job_value = Value::string(job);
        let owner = Value::string(instance);
        reserve(&self.tx, |conn| match expect {
            Expect::Absent => {
                let params = [job_value, Value::string(run_id), now.clone(), owner];
                // A lost insert race is a held lock.
                Ok(conn.execute(&self.insert_sql, &params).is_ok())
            }
            Expect::Free => {
                let params = [Value::string(run_id), now.clone(), owner, job_value];
                Ok(conn.execute(&self.claim_free_sql, &params)? > 0)
            }
            Expect::Run(old) => {
                let params = [Value::string(run_id), now.clone(), owner, job_value, Value::string(old)];
                Ok(conn.execute(&self.claim_run_sql, &params)? > 0)
            }
        })
    }

    /// Clear `run_id`'s hold and set the run time. Does nothing if another run holds the lock now.
    fn release(&self, job: &str, run_id: &str, last_run: &Value) -> Result<(), Error> {
        let params = [
            Value::Null,
            last_run.clone(),
            Value::Null,
            Value::string(job),
            Value::string(run_id),
        ];
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

    /// Insert a lock that only carries `runCount`. False when the insert did not land, so the caller retries.
    fn insert_count(&self, job: &str, count: i64) -> Result<bool, Error> {
        let params = [Value::string(job), Value::I64(count)];
        reserve(&self.tx, |conn| match conn.execute(&self.count_insert_sql, &params) {
            Ok(updated) => Ok(updated > 0),
            Err(err) => {
                // A duplicate primary key is the other run's row, so the caller tries again.
                tracing::debug!(job, error = %err, "could not insert the job run count, retrying");
                Ok(false)
            }
        })
    }

    /// Set `runCount` to `next` only while it is still `current`. False when another run wrote it first.
    ///
    /// The count changes with a conditional update so two runs cannot both write the same number. The in-memory
    /// driver the tests use accepts `SET column = ?`.
    fn advance_count(&self, job: &str, current: Option<i64>, next: i64) -> Result<bool, Error> {
        reserve(&self.tx, |conn| {
            let updated = match current {
                None => conn.execute(&self.count_seed_sql, &[Value::I64(next), Value::string(job)])?,
                Some(old) => conn.execute(
                    &self.count_step_sql,
                    &[Value::I64(next), Value::string(job), Value::I64(old)],
                )?,
            };
            Ok(updated > 0)
        })
    }
}

// ---- values -------------------------------------------------------------------------------

/// `ServiceJob.transactionTimeout` in seconds, [`DEFAULT_JOB_TIMEOUT_SECONDS`] when empty or negative.
fn job_timeout(job: &EntityValue) -> Duration {
    Duration::from_secs(
        number(job, "transactionTimeout")
            .and_then(|secs| u64::try_from(secs).ok())
            .unwrap_or(DEFAULT_JOB_TIMEOUT_SECONDS),
    )
}

/// The time a job's scripts may run: its [`job_timeout`], or none when it says `noTimeLimit`.
pub(crate) fn script_limit(job: &EntityValue) -> Option<Duration> {
    (field_text(job, "noTimeLimit") != "Y").then(|| job_timeout(job))
}

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
    basic_convert(&Value::I64(millis), "Timestamp").value
}

fn clip(text: &str) -> String {
    text.chars().take(TEXT_LIMIT).collect()
}

impl Shared {
    /// The factory this pool serves. `None` before boot attaches it or after it is dropped.
    pub(crate) fn factory(&self) -> Option<Arc<ExecutionContextFactory>> {
        lock(&self.factory).upgrade()
    }

    /// Room for more jobs: the job workers that are idle, less the runs already waiting for one. A pass claims a job's
    /// lock only for a run a worker can take now, so this node does not hold the locks of jobs it cannot start while an
    /// idle member could run them (HL-026). Java counted `job-queue-max` plus `job-pool-max`, a queue too.
    pub(crate) fn job_slots(&self) -> i64 {
        let state = lock(&self.state);
        let total = i64::try_from(self.job_workers).unwrap_or(i64::MAX);
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
            script_limit: None,
            job_run_id: run_id.to_string(),
            last_run_time: timestamp_value(PREVIOUS),
            delete_when_done: false,
            user_name: None,
            origin: None,
            topic: None,
            description: None,
            error_history_only: false,
            retries: false,
            rules: None,
            claim: None,
            factory: Arc::clone(ctx.factory()),
        }
    }

    /// A run that waited for a worker while another instance took its lock over does not call its service (an unknown
    /// one here, whose call would end the run row with an error): its row goes and the other run keeps the lock.
    #[test]
    fn a_parked_run_whose_lock_was_taken_over_does_not_call_its_service() {
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let mut run = claimed(&ctx, &facade, "ParkedJob", "R-PARKED");
        run.claim = Some(super::LocalClaim::new(ctx.factory(), "R-PARKED"));
        let mut lock = one(&facade, JOB_LOCK, "jobName", "ParkedJob");
        lock.set("jobRunId", Value::string("ELSEWHERE"));
        lock.update(&facade).unwrap();
        super::run_job(run);
        assert_eq!(
            text(&one(&facade, JOB_LOCK, "jobName", "ParkedJob"), "jobRunId"),
            "ELSEWHERE"
        );
        let left = facade
            .find(JOB_RUN)
            .unwrap()
            .eq("jobRunId", Value::string("R-PARKED"))
            .one()
            .unwrap();
        assert!(left.is_none(), "the run did not start");
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
        let telemetry = crate::telemetry::for_factory(ctx.factory());
        #[cfg(feature = "telemetry")]
        let runs = {
            #[derive(Default)]
            struct Runs(std::sync::Mutex<Vec<String>>);
            impl crate::telemetry::Exporter for Runs {
                fn export(
                    &self,
                    index: &str,
                    documents: Vec<crate::entity::document::Document>,
                ) -> Result<usize, moqui_lib::error::Error> {
                    let count = documents.len();
                    for document in documents.iter().filter(|_| index == "otel_metrics") {
                        if document.get("name").and_then(Value::as_str) == Some("moqui.job.runs") {
                            self.0.lock().unwrap().push(text_of(document.get("outcome")));
                        }
                    }
                    Ok(count)
                }
            }
            fn text_of(value: Option<&Value>) -> String {
                value.map(to_plain_string).unwrap_or_default()
            }
            let runs = std::sync::Arc::new(Runs::default());
            telemetry.set_exporter(runs.clone());
            runs
        };
        let unwound = catch_unwind(AssertUnwindSafe(|| {
            let _end = RunEnd {
                facade: &facade,
                job: &run,
                telemetry: &telemetry,
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
        #[cfg(feature = "telemetry")]
        {
            telemetry.flush();
            assert_eq!(
                *runs.0.lock().unwrap(),
                ["panic"],
                "a panicked run counts in moqui.job.runs"
            );
            telemetry.shutdown();
        }
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
