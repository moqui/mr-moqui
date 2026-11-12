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

//! The job lease (plans/archive/High-Level-Review.md HL-026): the job runs this process claimed ([`LocalRuns`]), and a keeper
//! thread of their own, apart from the scheduler and the cluster heartbeat, that renews the `leaseExpireDate` of their
//! `ServiceJobRunLock` rows in one update. A lock whose lease lapsed is taken over by the next pass on any member,
//! before the holder is gone and before `expireLockTime`, which stays the upper bound.
//!
//! The keeper stops renewing a run that has gone on past its job timeout (`transactionTimeout`, 1800 seconds when
//! empty; `noTimeLimit` has none) and [`LeaseTiming::grace`], and ends its scripts. It reads the locks of the runs that
//! started back after each renewal: a run whose lock another run holds now has its scripts ended too, so it stops
//! rather than runs beside the one that took the lock over. A run's scripts end through the thread's interrupt flag
//! ([`crate::script::interrupt_thread_on`]), as at a passed deadline; Rust code a run calls is not stopped by it.
//!
//! Lease times are the database's clock, as member liveness is ([`crate::cluster::db_now`]), so members whose clocks
//! differ agree on when a lease lapsed.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_db::sql::{update, Condition};
use moqui_lib::object::{basic_convert, Value};
use moqui_lib::sync::lock;

use super::util::{field_text, reserve};
use crate::context::{Error, ExecutionContextFactory};

const JOB_LOCK: &str = "moqui.service.job.ServiceJobRunLock";

/// How the job lease is timed. A factory extension: [`timing`] gives the defaults, and a test that wants others inserts
/// its own before the first claim.
#[derive(Clone, Debug)]
pub(crate) struct LeaseTiming {
    /// How often the keeper renews.
    pub(crate) renew_every: Duration,
    /// How long a lease lasts from its claim or renewal: three renewals.
    pub(crate) lease: Duration,
    /// How long past its job timeout a run is still renewed, for its own deadline to end it first.
    pub(crate) grace: Duration,
}

impl Default for LeaseTiming {
    fn default() -> Self {
        Self {
            renew_every: Duration::from_secs(60),
            lease: Duration::from_secs(180),
            grace: Duration::from_secs(60),
        }
    }
}

/// The factory's lease timing.
pub(crate) fn timing(factory: &ExecutionContextFactory) -> Arc<LeaseTiming> {
    factory.extension(|_| LeaseTiming::default())
}

/// The lease's milliseconds.
pub(crate) fn lease_millis(factory: &ExecutionContextFactory) -> i64 {
    i64::try_from(timing(factory).lease.as_millis()).unwrap_or(i64::MAX)
}

/// The database time `millis` as a timestamp value.
pub(crate) fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "Timestamp").value
}

/// The job runs this process claimed and has not finished, by run id: a lock held under this instance's id but by none
/// of them (left by an earlier process of this instance) can be taken over, and the keeper renews the leases of these.
#[derive(Default)]
pub(crate) struct LocalRuns(Mutex<HashMap<String, LocalRun>>);

/// One claimed run: parked until a worker starts it.
#[derive(Default)]
pub(crate) struct LocalRun {
    started: Option<Started>,
}

/// A run a worker is running.
struct Started {
    job_name: String,
    at: Instant,
    /// Its job timeout; None for `noTimeLimit`.
    limit: Option<Duration>,
    /// Set to end the run's scripts.
    interrupt: Arc<AtomicBool>,
}

impl LocalRuns {
    /// Whether `run_id` is a run of this process.
    pub(crate) fn contains(&self, run_id: &str) -> bool {
        lock(&self.0).contains_key(run_id)
    }

    #[cfg(test)]
    pub(crate) fn insert_started(
        &self,
        run_id: &str,
        job_name: &str,
        ago: Duration,
        limit: Option<Duration>,
    ) -> Arc<AtomicBool> {
        let interrupt = Arc::new(AtomicBool::new(false));
        let started = Started {
            job_name: job_name.to_string(),
            at: Instant::now().checked_sub(ago).unwrap_or_else(Instant::now),
            limit,
            interrupt: Arc::clone(&interrupt),
        };
        lock(&self.0).insert(run_id.to_string(), LocalRun { started: Some(started) });
        interrupt
    }
}

/// One claimed run in [`LocalRuns`], until it drops.
pub(crate) struct LocalClaim {
    runs: Arc<LocalRuns>,
    run_id: String,
}

impl LocalClaim {
    pub(crate) fn new(factory: &ExecutionContextFactory, run_id: &str) -> Self {
        let runs = factory.extension(|_| LocalRuns::default());
        lock(&runs.0).insert(run_id.to_string(), LocalRun::default());
        Self {
            runs,
            run_id: run_id.to_string(),
        }
    }

    /// The run starts on this thread: until the guard drops, the keeper checks that it still holds its lock, and ends
    /// its scripts when it lost it or ran past `limit` and the grace.
    #[must_use]
    pub(crate) fn start(&self, job_name: &str, limit: Option<Duration>) -> Running<'_> {
        let interrupt = Arc::new(AtomicBool::new(false));
        crate::script::interrupt_thread_on(Some(Arc::clone(&interrupt)));
        if let Some(run) = lock(&self.runs.0).get_mut(&self.run_id) {
            run.started = Some(Started {
                job_name: job_name.to_string(),
                at: Instant::now(),
                limit,
                interrupt,
            });
        }
        Running { claim: self }
    }
}

impl Drop for LocalClaim {
    fn drop(&mut self) {
        lock(&self.runs.0).remove(&self.run_id);
    }
}

/// A started run, from [`LocalClaim::start`]. Dropped before the run's end is written, so the release of its lock is
/// not read as a lost lock, and the end's own scripts are not ended.
pub(crate) struct Running<'a> {
    claim: &'a LocalClaim,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        crate::script::interrupt_thread_on(None);
        if let Some(run) = lock(&self.claim.runs.0).get_mut(&self.claim.run_id) {
            run.started = None;
        }
    }
}

/// What one [`renew`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Renewal {
    /// Lock rows whose lease was renewed.
    pub(crate) renewed: usize,
    /// Started runs whose lock another run holds now: their scripts are ended.
    pub(crate) lost: Vec<String>,
    /// Started runs past their job timeout and the grace: not renewed, and their scripts are ended.
    pub(crate) overdue: Vec<String>,
}

/// Renew the leases of this process's job runs in one update (`jobRunId IN (...)` and this instance), then read back
/// the locks of the started ones; a run past its timeout and the grace is left out, and its scripts end, as do those of
/// a run whose lock is held by another run now. Each such run is logged.
///
/// # Errors
///
/// Returns an error when the locks cannot be written or read.
pub(crate) fn renew(factory: &ExecutionContextFactory) -> Result<Renewal, Error> {
    let mut renewal = Renewal::default();
    let Some(runs) = factory.extension_if_present::<LocalRuns>() else {
        return Ok(renewal);
    };
    let grace = timing(factory).grace;
    let (renewing, started) = {
        let mut table = lock(&runs.0);
        let mut renewing = Vec::new();
        let mut started = Vec::new();
        for (run_id, run) in table.iter_mut() {
            match &run.started {
                Some(running) if running.limit.is_some_and(|limit| running.at.elapsed() > limit + grace) => {
                    if !running.interrupt.swap(true, Ordering::SeqCst) {
                        tracing::warn!(job = %running.job_name, run = %run_id, "the job run went on past its timeout; its lease is not renewed and its scripts end");
                    }
                    renewal.overdue.push(run_id.clone());
                }
                Some(_) => {
                    renewing.push(run_id.clone());
                    started.push(run_id.clone());
                }
                None => renewing.push(run_id.clone()),
            }
        }
        (renewing, started)
    };
    if renewing.is_empty() {
        return Ok(renewal);
    }
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    let until = crate::cluster::db_now(&facade).saturating_add(lease_millis(factory));
    let instance = super::jobs::own_instance(factory);
    let ids: Vec<Value> = renewing.iter().map(Value::string).collect();
    let condition = Condition::and(vec![
        Condition::is_in("jobRunId", ids),
        Condition::equals("instanceId", Value::string(&instance)),
    ]);
    let (statement, tx) = facade
        .with_group(JOB_LOCK, |entity, dialect, _, tx| {
            Ok((
                update(entity, dialect, &["leaseExpireDate"], Some(&condition))?,
                tx.clone(),
            ))
        })
        .map_err(Error::from)?;
    let mut params = vec![stamp(until)];
    params.extend(statement.params);
    let renewed = reserve(&tx, |conn| conn.execute(&statement.sql, &params))?;
    renewal.renewed = usize::try_from(renewed).unwrap_or(usize::MAX);
    if started.is_empty() {
        return Ok(renewal);
    }
    let held: HashSet<String> = facade
        .find(JOB_LOCK)?
        .condition(Condition::is_in(
            "jobRunId",
            started.iter().map(Value::string).collect(),
        ))
        .list()?
        .values()
        .iter()
        .map(|row| field_text(row, "jobRunId"))
        .collect();
    let table = lock(&runs.0);
    for run_id in started.into_iter().filter(|run_id| !held.contains(run_id)) {
        // Still started: one that ended since released its lock itself.
        let Some(running) = table.get(&run_id).and_then(|run| run.started.as_ref()) else {
            continue;
        };
        if !running.interrupt.swap(true, Ordering::SeqCst) {
            tracing::warn!(job = %running.job_name, run = %run_id, "the job run's lock was taken over; its scripts end");
        }
        renewal.lost.push(run_id);
    }
    Ok(renewal)
}

/// Start the keeper thread for `factory`, once: [`renew`] every [`LeaseTiming::renew_every`] until the factory is
/// dropped or its shutdown begins, which waits for it. A failure, or a panic, is logged and the next renewal is tried.
pub(crate) fn start_keeper(factory: &Arc<ExecutionContextFactory>) {
    struct Keeper;
    let mut first = false;
    factory.extension(|_| {
        first = true;
        Keeper
    });
    if !first {
        return;
    }
    let every = timing(factory).renew_every;
    let weak = Arc::downgrade(factory);
    let spawned = factory.spawn_background("moqui-job-lease", move || loop {
        let Some(factory) = ExecutionContextFactory::sleep_while_running(&weak, every) else {
            return;
        };
        let renewed = catch_unwind(AssertUnwindSafe(|| {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            renew(&factory)
        }));
        match renewed {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => tracing::error!(error = %err, "the job leases could not be renewed"),
            Err(_) => tracing::error!("the job lease renewal panicked"),
        }
    });
    if let Err(err) = spawned {
        tracing::error!(error = %err, "the job lease keeper did not start");
    }
}

#[cfg(test)]
#[path = "job_lease_tests.rs"]
mod tests;
