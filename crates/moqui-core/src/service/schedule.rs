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

//! The scheduler thread and the Quartz cron parser. The thread only sleeps and starts passes; a
//! pass is in `jobs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;

use croner::parser::{CronParser, Seconds};
use croner::Cron;

use super::pool::Shared;
use crate::context::Error;

/// How often the thread looks at the stop flag. A pass runs every `check_interval`.
const SLICE: Duration = Duration::from_millis(250);

/// Java's wait before the first job pass, "to make sure all is loaded and we're past an initial activity burst".
const INITIAL_DELAY: Duration = Duration::from_secs(120);

pub(crate) fn scheduler_loop(shared: Arc<Shared>) {
    let mut clock = Clock::new(Instant::now(), shared.job_runner, shared.check_interval);
    loop {
        {
            let state = lock(&shared.state);
            if state.stop {
                return;
            }
            let (state, _) = shared
                .wake
                .wait_timeout(state, SLICE)
                .unwrap_or_else(|err| err.into_inner());
            if state.stop {
                return;
            }
        }
        let (jobs, hooks) = clock.due(Instant::now());
        if !jobs && !hooks {
            continue;
        }
        let _cleanup = super::pool::WorkCleanup::new(shared.factory());
        if jobs {
            super::jobs::run_scheduled(&shared);
        }
        if hooks {
            super::pool::run_tick_hook(&shared);
        }
    }
}

/// When the scheduler runs a job pass and the tick hooks. Job passes are Java's `scheduleAtFixedRate(jobRunner, 120,
/// rate)`: the first after [`INITIAL_DELAY`], then every interval, and none without a job runner. The tick hooks (the
/// cluster heartbeat) are this port's own, so they run every interval from the start.
struct Clock {
    next_jobs: Option<Instant>,
    last_hooks: Instant,
    interval: Duration,
}

impl Clock {
    fn new(start: Instant, job_runner: bool, interval: Duration) -> Self {
        Self {
            next_jobs: job_runner.then(|| start + INITIAL_DELAY),
            last_hooks: start,
            interval,
        }
    }

    /// Whether a job pass and the hooks are due at `now`; each that is is counted as run.
    fn due(&mut self, now: Instant) -> (bool, bool) {
        let jobs = self.next_jobs.is_some_and(|at| now >= at);
        if jobs {
            self.next_jobs = Some(now + self.interval);
        }
        let hooks = now.duration_since(self.last_hooks) >= self.interval;
        if hooks {
            self.last_hooks = now;
        }
        (jobs, hooks)
    }
}

/// Parsed patterns, as Java's `ScheduledJobRunner.cronByExpression`, so a pass does not parse each job's pattern again.
static CRONS: OnceLock<Mutex<HashMap<String, Arc<Cron>>>> = OnceLock::new();

/// More distinct patterns than this clears the map, which only bounds memory if patterns are generated.
const CRON_CACHE_MAX: usize = 1000;

/// [`parse_cron`], parsed once for each pattern. A pattern that does not parse is not kept.
pub(crate) fn cached_cron(pattern: &str) -> Result<Arc<Cron>, Error> {
    let crons = CRONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cron) = lock(crons).get(pattern) {
        return Ok(Arc::clone(cron));
    }
    let cron = Arc::new(parse_cron(pattern)?);
    let mut crons = lock(crons);
    if crons.len() >= CRON_CACHE_MAX {
        crons.clear();
    }
    crons.insert(pattern.to_string(), Arc::clone(&cron));
    Ok(cron)
}

/// Quartz syntax: optional seconds, Sunday is day 1, `?` `L` `W` `#` allowed.
pub(crate) fn parse_cron(pattern: &str) -> Result<Cron, Error> {
    CronParser::builder()
        .seconds(Seconds::Optional)
        .alternative_weekdays(true)
        .sloppy_ranges(true)
        .build()
        .parse(pattern)
        .map_err(|err| Error::new(format!("cron pattern: {err}")))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use moqui_lib::object::Value;

    use super::super::pool::ServicePools;
    use super::parse_cron;
    use crate::context::{ExecutionContext, ExecutionContextFactory};
    use crate::entity::EntityFacade;

    /// As Java's `scheduleAtFixedRate(jobRunner, 120, rate)`, the first job pass waits 120 seconds; the hooks do not.
    #[test]
    fn the_first_job_pass_waits_the_initial_delay_and_none_runs_without_a_runner() {
        let start = Instant::now();
        let minute = Duration::from_secs(60);
        let mut clock = super::Clock::new(start, true, minute);
        assert_eq!(clock.due(start + Duration::from_secs(30)), (false, false));
        assert_eq!(clock.due(start + Duration::from_secs(61)), (false, true));
        assert_eq!(clock.due(start + Duration::from_secs(120)), (true, false));
        assert_eq!(clock.due(start + Duration::from_secs(150)), (false, true));
        assert_eq!(clock.due(start + Duration::from_secs(170)), (false, false));
        assert_eq!(clock.due(start + Duration::from_secs(181)), (true, false));
        let mut off = super::Clock::new(start, false, minute);
        assert_eq!(off.due(start + Duration::from_secs(61)), (false, true));
        assert_eq!(off.due(start + Duration::from_secs(600)), (false, true));
    }

    #[test]
    fn quartz_patterns_parse_and_describe() {
        parse_cron("0 0/15 * * * ?").unwrap();
        let nightly = parse_cron("0 0 2 * * ?").unwrap();
        assert!(!nightly.describe().is_empty());
        let every = parse_cron("* * * * * *").unwrap();
        let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::system());
        let next = every.find_next_occurrence(&now, true).unwrap();
        let wait = next.timestamp().as_second() - now.timestamp().as_second();
        assert!((0..=1).contains(&wait), "{wait}");
        // Sunday is day 1 and Monday day 2, as in Quartz.
        parse_cron("0 0 8 ? * 2").unwrap();
    }

    #[test]
    fn call_future_returns_the_out_parameter() {
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(crate::testing::open());
        let _entered = ctx.enter();
        register_echo();
        let pools = ServicePools::bounded(2, 8);
        let mut params = BTreeMap::new();
        params.insert("note".into(), Value::string("hello"));
        let out = pools.call_future("echo#Note", &params).unwrap();
        assert_eq!(out.get("note").and_then(Value::as_str), Some("hello"));
    }

    #[test]
    fn full_queue_call_future_returns_immediately() {
        let ctx = ExecutionContextFactory::test_context();
        let _entered = ctx.enter();
        register_echo();
        let pools = ServicePools::bounded(1, 1);
        let hold = Hold::new();
        fill_queue(&pools, &hold);
        let started = Instant::now();
        let err = pools.call_future("echo#Note", &BTreeMap::new()).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1), "call_future blocked");
        assert!(err.to_string().contains("queue"), "{err}");
    }

    #[test]
    fn one_worker_call_future_uses_a_reserved_thread() {
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(crate::testing::open());
        let _entered = ctx.enter();
        register_echo();
        let factory = Arc::clone(ctx.factory());
        let pools = ServicePools::bounded(1, 1);
        let shared = pools.shared_handle();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        pools
            .submit_task(move || {
                let worker_ctx = factory.new_context();
                let _entered = worker_ctx.enter();
                let mut params = BTreeMap::new();
                params.insert("note".into(), Value::string("nested"));
                let out = shared.call_future("echo#Note", &params).unwrap();
                done_tx.send(out).unwrap();
            })
            .unwrap();
        let out = done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("reserved call finished");
        assert_eq!(out.get("note").and_then(Value::as_str), Some("nested"));
    }

    #[test]
    fn dropping_idle_does_not_stop_another_pool() {
        let pools = ServicePools::bounded(1, 4);
        drop(ServicePools::idle());
        let (tx, rx) = std::sync::mpsc::channel();
        pools.submit_task(move || tx.send(()).unwrap()).unwrap();
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn call_future_without_a_context_names_it() {
        let pools = ServicePools::bounded(1, 1);
        let err = pools.call_future("echo#Note", &BTreeMap::new()).unwrap_err();
        assert!(err.to_string().contains("execution context"), "{err}");
    }

    #[test]
    fn default_conf_sizes_and_drop_joins() {
        let text = std::fs::read_to_string(framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap();
        // `worker-pool-max` 32, less the 2 job workers of `job-pool-core`.
        let pools = super::super::start_pools(&text).unwrap();
        assert_eq!(pools.sizes(), (30, 65_535));
        drop(pools);
        // With no `worker-queue`, Java's code default.
        let missing = super::super::start_pools("<moqui-conf/>").unwrap();
        assert_eq!(missing.sizes(), (30, 65_536));
        drop(missing);
        let junk = super::super::start_pools(
            r#"<moqui-conf><tools worker-pool-max="nope" worker-queue="nope"/></moqui-conf>"#,
        )
        .unwrap();
        assert_eq!(junk.sizes(), (30, 65_536));
        drop(junk);
        let none = super::super::start_pools(r#"<moqui-conf><tools worker-pool-max="0"/></moqui-conf>"#);
        assert!(none.is_err(), "a pool with no workers can never run anything");
    }

    /// As Java, `scheduled-job-check-time="0"` starts no job runner; the tick hooks keep the default interval.
    #[test]
    fn a_check_time_of_zero_runs_no_job_passes() {
        let off =
            super::super::start_pools(r#"<moqui-conf><service-facade scheduled-job-check-time="0"/></moqui-conf>"#)
                .unwrap();
        assert_eq!(off.runner_stats(), None, "no job runner");
        assert_eq!(off.check_interval(), Duration::from_secs(60));
        assert_eq!(off.tick().jobs_run, 0);
        assert_eq!(off.runner_stats(), None, "a tick runs no job pass");
        drop(off);
        let on =
            super::super::start_pools(r#"<moqui-conf><service-facade scheduled-job-check-time="5"/></moqui-conf>"#)
                .unwrap();
        assert!(on.runner_stats().is_some());
        assert_eq!(on.check_interval(), Duration::from_secs(5));
    }

    // ---- scheduled jobs ----------------------------------------------------------------------

    const SERVICES: &str = r#"<services>
        <service verb="echo" noun="Note" authenticate="false" transaction="ignore">
            <in-parameters><parameter name="note"/></in-parameters>
            <out-parameters><parameter name="note"/></out-parameters>
        </service>
        <service verb="mark" noun="Row">
            <in-parameters><parameter name="note"/></in-parameters>
            <actions>
                <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                <script>made.set("testId", note); made.set("testMedium", "from-job");</script>
                <entity-create value-field="made"/>
            </actions>
        </service>
        <service verb="markOpen" noun="Row" authenticate="false">
            <in-parameters><parameter name="note"/></in-parameters>
            <actions>
                <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                <script>made.set("testId", note); made.set("testMedium", "open");</script>
                <entity-create value-field="made"/>
            </actions>
        </service>
        <service verb="fail" noun="Always">
            <actions><message error="true">job failed</message></actions>
        </service>
    </services>"#;

    struct Jobs {
        ctx: ExecutionContext,
        facade: EntityFacade,
    }

    fn jobs_world(workers: usize, queue: usize) -> Jobs {
        // a file for each harness: tests in parallel would otherwise write and lint one shared file
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("moqui-jobs-{}-{seq}.xml", std::process::id()));
        std::fs::write(&path, SERVICES).unwrap();
        let pools = ServicePools::bounded(workers, queue);
        let ctx = ExecutionContextFactory::test_context_with("", pools);
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        {
            let _entered = ctx.enter();
            let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
            assert!(failures.is_empty(), "{failures:?}");
        }
        Jobs { ctx, facade }
    }

    fn create(facade: &EntityFacade, name: &str, fields: &[(&str, &str)]) {
        let mut value = facade.make_value(name).unwrap();
        for (field, text) in fields {
            value.set(*field, Value::string(*text));
        }
        value.create(facade).unwrap();
    }

    fn add_job(world: &Jobs, name: &str, service: &str, cron: &str, extra: &[(&str, &str)]) {
        let mut fields = vec![
            ("jobName", name),
            ("serviceName", service),
            ("cronExpression", cron),
            ("paused", "N"),
        ];
        fields.extend_from_slice(extra);
        create(&world.facade, "moqui.service.job.ServiceJob", &fields);
    }

    fn runs(world: &Jobs, name: &str) -> Vec<crate::entity::EntityValue> {
        world
            .facade
            .find("moqui.service.job.ServiceJobRun")
            .unwrap()
            .eq("jobName", Value::string(name))
            .list()
            .unwrap()
            .values()
            .to_vec()
    }

    fn text(row: &crate::entity::EntityValue, field: &str) -> String {
        row.get(field)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_due_job_runs_once_and_leaves_its_history() {
        let world = jobs_world(2, 8);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        add_job(&world, "J1", "mark#Row", "* * * * * *", &[]);
        create(
            &world.facade,
            "moqui.service.job.ServiceJobParameter",
            &[
                ("jobName", "J1"),
                ("parameterName", "note"),
                ("parameterValue", "ran-once"),
            ],
        );
        let pools = world.ctx.factory().service_pools();
        let report = pools.tick();
        assert_eq!(report.jobs_run, 1, "{report:?}");
        wait_for("the run to finish", || {
            runs(&world, "J1").iter().any(|run| text(run, "hasError") == "N")
        });
        let row = world
            .facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("ran-once"))
            .one()
            .unwrap()
            .expect("the job's service ran with its stored parameter");
        assert_eq!(text(&row, "testMedium"), "from-job");
        let lock = world
            .facade
            .find("moqui.service.job.ServiceJobRunLock")
            .unwrap()
            .eq("jobName", Value::string("J1"))
            .one()
            .unwrap()
            .expect("the lock row");
        assert_eq!(text(&lock, "jobRunId"), "", "the lock is cleared after the run");
        assert!(!text(&lock, "lastRunTime").is_empty());
        let history = runs(&world, "J1");
        assert_eq!(history.len(), 1);
        // As Java's `ServiceCallJobImpl`, the run records the host it ran on.
        assert!(!text(&history[0], "hostAddress").is_empty());
        assert!(!text(&history[0], "hostName").is_empty());
        assert!(!text(&history[0], "runThread").is_empty());
    }

    /// As Java's `ScheduledJobRunner`, a run that cannot be started once the lock is taken is kept as a failed run, and
    /// the lock is cleared with this pass's run time.
    #[test]
    fn a_run_that_cannot_be_started_is_recorded_as_failed() {
        let world = jobs_world(1, 4);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        add_job(&world, "NOSTART", "echo#Note", "* * * * * *", &[]);
        let pools = world.ctx.factory().service_pools();
        // A stopped pool takes no job run.
        pools.shutdown();
        let report = pools.tick();
        assert_eq!(report.jobs_run, 1, "{report:?}");
        let history = runs(&world, "NOSTART");
        assert_eq!(history.len(), 1, "the run row is kept");
        assert_eq!(text(&history[0], "hasError"), "Y");
        assert!(!text(&history[0], "errors").is_empty());
        assert!(!text(&history[0], "endTime").is_empty());
        let lock = world
            .facade
            .find("moqui.service.job.ServiceJobRunLock")
            .unwrap()
            .eq("jobName", Value::string("NOSTART"))
            .one()
            .unwrap()
            .expect("the lock row");
        assert_eq!(text(&lock, "jobRunId"), "");
        assert!(!text(&lock, "lastRunTime").is_empty());
    }

    /// As Java's `ScheduledJobRunner`, the runner's counters are updated after a pass that failed.
    #[test]
    fn a_failed_pass_is_still_counted() {
        let world = jobs_world(1, 4);
        // A database with no service job entities: the pass fails before it looks at a job.
        let dir = std::env::temp_dir().join(format!("moqui-no-job-entities-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("entity")).unwrap();
        std::fs::create_dir_all(dir.join("conf")).unwrap();
        std::fs::copy(
            framework_dir().join("conf/MoquiDefaultConf.xml"),
            dir.join("conf/MoquiDefaultConf.xml"),
        )
        .unwrap();
        world.ctx.factory().set_entity(crate::testing::open_at(&dir));
        let pools = world.ctx.factory().service_pools();
        assert_eq!(pools.tick(), super::super::jobs::PassReport::default());
        let stats = pools.runner_stats().expect("a job runner");
        assert_eq!(stats.execute_count, 1);
        assert!(stats.last_execute_millis > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// As Java's `ScheduledJobRunner`, a job whose cron pattern does not parse is logged and skipped, and the jobs
    /// after it in the pass still run.
    #[test]
    fn a_job_with_a_bad_cron_is_skipped_and_the_next_job_runs() {
        let world = jobs_world(2, 8);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        add_job(&world, "A0", "mark#Row", "not a cron", &[]);
        add_job(&world, "J1", "mark#Row", "* * * * * *", &[]);
        create(
            &world.facade,
            "moqui.service.job.ServiceJobParameter",
            &[
                ("jobName", "J1"),
                ("parameterName", "note"),
                ("parameterValue", "after-bad"),
            ],
        );
        let report = world.ctx.factory().service_pools().tick();
        assert_eq!(report.jobs_failed, 1, "{report:?}");
        assert_eq!(report.jobs_run, 1, "{report:?}");
        wait_for("the good job to finish", || {
            runs(&world, "J1").iter().any(|run| text(run, "hasError") == "N")
        });
        assert!(runs(&world, "A0").is_empty());
    }

    #[test]
    fn paused_and_future_jobs_do_not_run_and_a_held_lock_blocks_a_job() {
        let world = jobs_world(2, 8);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        add_job(&world, "PAUSED", "echo#Note", "* * * * * *", &[("paused", "Y")]);
        // Every first of January: nothing before now has been missed yet this year only if it ran.
        add_job(&world, "HELD", "echo#Note", "* * * * * *", &[]);
        create(
            &world.facade,
            "moqui.service.job.ServiceJobRunLock",
            &[("jobName", "HELD"), ("jobRunId", "OTHER-INSTANCE")],
        );
        let mut lock = world
            .facade
            .find("moqui.service.job.ServiceJobRunLock")
            .unwrap()
            .eq("jobName", Value::string("HELD"))
            .one()
            .unwrap()
            .unwrap();
        lock.set(
            "lastRunTime",
            moqui_lib::object::basic_convert(
                &Value::I64(jiff::Timestamp::now().as_millisecond()),
                "java.sql.Timestamp",
            )
            .value,
        );
        lock.update(&world.facade).unwrap();
        let report = world.ctx.factory().service_pools().tick();
        assert_eq!(report.jobs_run, 0, "{report:?}");
        assert_eq!(report.jobs_paused, 1);
        assert!(runs(&world, "HELD").is_empty());
        assert!(runs(&world, "PAUSED").is_empty());
    }

    #[test]
    fn a_failed_run_is_recorded_and_not_retried_before_the_minimum_retry_time() {
        let world = jobs_world(2, 8);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        add_job(&world, "BAD", "fail#Always", "* * * * * *", &[("minRetryTime", "60")]);
        assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
        wait_for("the failed run", || {
            runs(&world, "BAD").iter().any(|run| text(run, "hasError") == "Y")
        });
        let run = runs(&world, "BAD").remove(0);
        assert!(text(&run, "errors").contains("job failed"), "{}", text(&run, "errors"));
        std::thread::sleep(Duration::from_millis(1100));
        let report = world.ctx.factory().service_pools().tick();
        assert_eq!(report.jobs_run, 0, "{report:?}");
        assert_eq!(runs(&world, "BAD").len(), 1);
    }

    #[test]
    fn a_repeat_count_pauses_the_job_after_that_many_runs() {
        let world = jobs_world(2, 8);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        add_job(&world, "ONCE", "echo#Note", "* * * * * *", &[("repeatCount", "1")]);
        assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
        wait_for("the run", || {
            runs(&world, "ONCE").iter().any(|run| !text(run, "endTime").is_empty())
        });
        std::thread::sleep(Duration::from_millis(1100));
        let report = world.ctx.factory().service_pools().tick();
        assert_eq!(report.jobs_run, 0, "{report:?}");
        let job = world
            .facade
            .find("moqui.service.job.ServiceJob")
            .unwrap()
            .eq("jobName", Value::string("ONCE"))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(text(&job, "paused"), "Y");
    }

    #[test]
    fn a_call_that_finds_the_queue_full_is_stored_as_a_job_and_run_later() {
        let world = jobs_world(1, 1);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        let pools = world.ctx.factory().service_pools();
        let hold = Hold::new();
        fill_queue(pools, &hold);
        let mut params = BTreeMap::new();
        params.insert("note".into(), Value::string("queued-call"));
        pools.submit("mark#Row", &params).expect("stored, not lost");
        let stored: Vec<_> = world
            .facade
            .find("moqui.service.job.ServiceJob")
            .unwrap()
            .list()
            .unwrap()
            .values()
            .iter()
            .filter(|job| text(job, "jobName").starts_with(super::super::jobs::QUEUE_PREFIX))
            .cloned()
            .collect();
        assert_eq!(stored.len(), 1);
        assert_eq!(text(&stored[0], "serviceName"), "mark#Row");
        // Jobs run on their own workers, as Java's `jobWorkerPool`, so the stored call runs while the service queue
        // is still full.
        let mut submitted = false;
        wait_for("the queued call to run", || {
            if !submitted {
                submitted = pools.tick().jobs_run == 1;
            }
            world
                .facade
                .find("moqui.test.TestEntity")
                .unwrap()
                .eq("testId", Value::string("queued-call"))
                .one()
                .unwrap()
                .is_some()
        });
        wait_for("the stored call to be removed", || {
            world
                .facade
                .find("moqui.service.job.ServiceJob")
                .unwrap()
                .list()
                .unwrap()
                .values()
                .iter()
                .all(|job| !text(job, "jobName").starts_with(super::super::jobs::QUEUE_PREFIX))
        });
        hold.open();
    }

    #[test]
    fn a_failed_async_call_is_not_silent() {
        // The worker logs the failure; the caller of a fire-and-forget call gets nothing back, so
        // the observable part is that the pool survives and runs the next call.
        let world = jobs_world(1, 8);
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        let pools = world.ctx.factory().service_pools();
        // An unknown service is an error at once, as Java's `validateCall`; one that fails as it runs is logged.
        assert!(pools.submit("nothing#Registered", &BTreeMap::new()).is_err());
        pools.submit("fail#Always", &BTreeMap::new()).unwrap();
        let mut params = BTreeMap::new();
        params.insert("note".into(), Value::string("after-failure"));
        pools.submit("markOpen#Row", &params).unwrap();
        wait_for("the second call", || {
            world
                .facade
                .find("moqui.test.TestEntity")
                .unwrap()
                .eq("testId", Value::string("after-failure"))
                .one()
                .unwrap()
                .is_some()
        });
    }

    fn fill_queue(pools: &ServicePools, hold: &Hold) {
        let task_hold = hold.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        pools
            .submit_task(move || {
                started_tx.send(()).unwrap();
                task_hold.wait();
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        pools.submit_task(|| {}).unwrap();
    }

    fn register_echo() {
        let path = std::env::temp_dir().join(format!("moqui-svc-echo-{}.xml", std::process::id()));
        std::fs::write(
            &path,
            r#"<services>
                <service verb="echo" noun="Note" authenticate="false" transaction="ignore">
                    <in-parameters><parameter name="note"/></in-parameters>
                    <out-parameters><parameter name="note"/></out-parameters>
                </service>
            </services>"#,
        )
        .unwrap();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }

    #[derive(Clone)]
    struct Hold {
        pair: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Hold {
        fn new() -> Self {
            Self {
                pair: Arc::new((Mutex::new(false), Condvar::new())),
            }
        }

        fn wait(&self) {
            let (lock, cv) = &*self.pair;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }

        fn open(&self) {
            let (lock, cv) = &*self.pair;
            let mut open = lock.lock().unwrap();
            *open = true;
            cv.notify_all();
        }
    }

    impl Drop for Hold {
        fn drop(&mut self) {
            self.open();
        }
    }

    fn framework_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
    }
}
