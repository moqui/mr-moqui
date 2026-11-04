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
    let junk =
        super::super::start_pools(r#"<moqui-conf><tools worker-pool-max="nope" worker-queue="nope"/></moqui-conf>"#)
            .unwrap();
    assert_eq!(junk.sizes(), (30, 65_536));
    drop(junk);
    let none = super::super::start_pools(r#"<moqui-conf><tools worker-pool-max="0"/></moqui-conf>"#);
    assert!(none.is_err(), "a pool with no workers can never run anything");
}

/// As Java, `scheduled-job-check-time="0"` starts no job runner; the tick hooks keep the default interval.
#[test]
fn a_check_time_of_zero_runs_no_job_passes() {
    let off = super::super::start_pools(r#"<moqui-conf><service-facade scheduled-job-check-time="0"/></moqui-conf>"#)
        .unwrap();
    assert_eq!(off.runner_stats(), None, "no job runner");
    assert_eq!(off.check_interval(), Duration::from_secs(60));
    assert_eq!(off.tick().jobs_run, 0);
    assert_eq!(off.runner_stats(), None, "a tick runs no job pass");
    drop(off);
    let on = super::super::start_pools(r#"<moqui-conf><service-facade scheduled-job-check-time="5"/></moqui-conf>"#)
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
    jobs_world_with(workers, queue, None, |_| {})
}

/// [`jobs_world`], with the `extra` services file and `register` run on the factory before any service definition is
/// read (for rust services).
fn jobs_world_with(
    workers: usize,
    queue: usize,
    extra: Option<&str>,
    register: impl FnOnce(&ExecutionContextFactory),
) -> Jobs {
    // a file for each harness: tests in parallel would otherwise write and lint one shared file
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("moqui-jobs-{}-{seq}.xml", std::process::id()));
    std::fs::write(&path, SERVICES).unwrap();
    let pools = ServicePools::bounded(workers, queue);
    let ctx = ExecutionContextFactory::test_context_with("", pools);
    register(ctx.factory());
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    {
        let _entered = ctx.enter();
        let mut paths = vec![path.clone()];
        if let Some(extra) = extra {
            let extra_path = path.with_extension("extra.xml");
            std::fs::write(&extra_path, extra).unwrap();
            paths.push(extra_path);
        }
        let failures = crate::service::lint(&paths).unwrap();
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

/// With every execution slot taken, a pass reads no job's lock or history: the bad cron, which fails only once its
/// readiness is checked, is not reached. With a slot free again the same pass checks and runs them.
#[test]
fn a_pass_with_no_free_slot_checks_no_job() {
    let world = jobs_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(&world, "A0", "echo#Note", "not a cron", &[]);
    add_job(&world, "J1", "echo#Note", "* * * * * *", &[]);
    let pools = world.ctx.factory().service_pools();
    let shared = pools.shared_handle();
    let busy = usize::try_from(shared.job_slots()).unwrap();
    moqui_lib::sync::lock(&shared.state).jobs_active += busy;
    let report = pools.tick();
    moqui_lib::sync::lock(&shared.state).jobs_active -= busy;
    assert_eq!(
        report,
        super::super::jobs::PassReport {
            jobs_not_checked: 2,
            ..Default::default()
        }
    );
    assert!(runs(&world, "J1").is_empty());
    let report = pools.tick();
    assert_eq!(
        (report.jobs_failed, report.jobs_run, report.jobs_not_checked),
        (1, 1, 0),
        "{report:?}"
    );
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

fn lock_row(world: &Jobs, name: &str) -> Option<crate::entity::EntityValue> {
    world
        .facade
        .find("moqui.service.job.ServiceJobRunLock")
        .unwrap()
        .eq("jobName", Value::string(name))
        .one()
        .unwrap()
}

/// `errorHistoryOnly` Y: a success removes its run row. The lock still records that the job ran.
#[test]
fn a_success_with_error_history_only_leaves_no_run_row() {
    let world = jobs_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(
        &world,
        "QUIET",
        "echo#Note",
        "* * * * * *",
        &[("errorHistoryOnly", "Y")],
    );
    assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
    wait_for("the success to be removed", || {
        runs(&world, "QUIET").is_empty()
            && lock_row(&world, "QUIET").is_some_and(|lock| text(&lock, "jobRunId").is_empty())
    });
    let lock = lock_row(&world, "QUIET").expect("the lock row");
    assert!(!text(&lock, "lastRunTime").is_empty(), "last run still updates");
}

/// `errorHistoryOnly` Y: a run that errors keeps its row.
#[test]
fn an_error_with_error_history_only_keeps_the_run() {
    let world = jobs_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(
        &world,
        "LOUD",
        "fail#Always",
        "* * * * * *",
        &[("errorHistoryOnly", "Y")],
    );
    assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
    wait_for("the failed run", || {
        runs(&world, "LOUD").iter().any(|run| text(run, "hasError") == "Y")
    });
    let history = runs(&world, "LOUD");
    assert_eq!(history.len(), 1);
    assert!(
        text(&history[0], "errors").contains("job failed"),
        "{}",
        text(&history[0], "errors")
    );
}

/// A later success moves `lastRunTime` past the error, so the old error row does not hold the job
/// for `minRetryTime`.
#[test]
fn a_success_after_an_error_does_not_keep_the_retry_delay() {
    let world = jobs_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(
        &world,
        "RETRY",
        "fail#Always",
        "* * * * * *",
        &[("minRetryTime", "60"), ("errorHistoryOnly", "Y")],
    );
    assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
    wait_for("the failed run", || {
        runs(&world, "RETRY").iter().any(|run| text(run, "hasError") == "Y")
    });
    std::thread::sleep(Duration::from_millis(1100));
    let held = world.ctx.factory().service_pools().tick();
    assert_eq!(held.jobs_run, 0, "{held:?}");
    // What a success records: lastRunTime moves to now, past the error, and the success row is gone.
    let mut lock = lock_row(&world, "RETRY").expect("the lock row");
    lock.set(
        "lastRunTime",
        moqui_lib::object::basic_convert(
            &Value::I64(jiff::Timestamp::now().as_millisecond()),
            "java.sql.Timestamp",
        )
        .value,
    );
    lock.update(&world.facade).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    let again = world.ctx.factory().service_pools().tick();
    assert_eq!(again.jobs_run, 1, "{again:?}");
}

/// `repeatCount` still pauses a job that does not keep its success rows. The count is on the lock.
#[test]
fn a_repeat_count_with_error_history_only_pauses_after_that_many_runs() {
    let world = jobs_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(
        &world,
        "ONCEH",
        "echo#Note",
        "* * * * * *",
        &[("repeatCount", "1"), ("errorHistoryOnly", "Y")],
    );
    assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
    wait_for("the success to be removed", || {
        runs(&world, "ONCEH").is_empty()
            && lock_row(&world, "ONCEH").is_some_and(|lock| text(&lock, "jobRunId").is_empty())
    });
    std::thread::sleep(Duration::from_millis(1100));
    let report = world.ctx.factory().service_pools().tick();
    assert_eq!(report.jobs_run, 0, "{report:?}");
    let job = world
        .facade
        .find("moqui.service.job.ServiceJob")
        .unwrap()
        .eq("jobName", Value::string("ONCEH"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(text(&job, "paused"), "Y");
    let lock = lock_row(&world, "ONCEH").expect("the lock row");
    assert_eq!(text(&lock, "runCount"), "1");
    assert!(runs(&world, "ONCEH").is_empty());
}

/// The stored call runs as its caller, the user and the agent run it was made in (the origin of its writes), as a
/// call the queue had room for does; what keeps them is not a parameter of the service.
#[test]
fn a_call_that_finds_the_queue_full_is_stored_as_a_job_and_run_later() {
    type Seen = (Option<String>, Option<crate::context::AgentOrigin>, Vec<String>);
    let seen: Arc<Mutex<Option<Seen>>> = Arc::new(Mutex::new(None));
    let heard = Arc::clone(&seen);
    let caller_service = r#"<services>
                <service verb="see" noun="Caller" type="rust" location="test::queued_caller">
                    <in-parameters><parameter name="note"/></in-parameters>
                </service>
            </services>"#;
    let world = jobs_world_with(1, 1, Some(caller_service), move |factory| {
        factory
            .register_service("test::queued_caller", move |ec, _info, parameters| {
                let user = crate::user::current_username().ok().flatten();
                let names = parameters.keys().cloned().collect();
                *heard.lock().unwrap() = Some((user, ec.agent_origin(), names));
                Ok(BTreeMap::new())
            })
            .unwrap();
    });
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    let mut user = world.facade.make_value("moqui.security.UserAccount").unwrap();
    user.set("userId", Value::string("QUEUED_CALLER"));
    user.set("username", Value::string("queued-caller"));
    user.create(&world.facade).unwrap();
    crate::user::internal_login_user("queued-caller").unwrap();
    let origin = crate::context::AgentOrigin {
        agent_id: "queue-agent".to_string(),
        run_id: Some("QUEUE_RUN_1".to_string()),
    };
    world.ctx.set_agent_origin(Some(origin.clone()));
    let pools = world.ctx.factory().service_pools();
    let hold = Hold::new();
    fill_queue(pools, &hold);
    let mut params = BTreeMap::new();
    params.insert("note".into(), Value::string("queued-call"));
    pools.submit("mark#Row", &params).expect("stored, not lost");
    pools.submit("see#Caller", &params).expect("stored, not lost");
    world.ctx.set_agent_origin(None);
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
    assert_eq!(stored.len(), 2);
    let services: Vec<String> = stored.iter().map(|job| text(job, "serviceName")).collect();
    assert!(services.contains(&"mark#Row".to_string()), "{services:?}");
    let caller_rows = world
        .facade
        .find("moqui.service.job.ServiceJobParameter")
        .unwrap()
        .eq("jobName", Value::string(text(&stored[0], "jobName")))
        .list()
        .unwrap();
    let caller: BTreeMap<String, String> = caller_rows
        .values()
        .iter()
        .map(|row| (text(row, "parameterName"), text(row, "parameterValue")))
        .collect();
    assert_eq!(caller.get("_moquiUsername").map(String::as_str), Some("queued-caller"));
    assert_eq!(caller.get("_originAgentId").map(String::as_str), Some("queue-agent"));
    assert_eq!(caller.get("_originRunId").map(String::as_str), Some("QUEUE_RUN_1"));
    // Jobs run on their own workers, as Java's `jobWorkerPool`, so the stored call runs while the service queue
    // is still full.
    let mut submitted = 0;
    wait_for("the queued calls to run", || {
        if submitted < 2 {
            submitted += pools.tick().jobs_run;
        }
        seen.lock().unwrap().is_some()
            && world
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
    let (user, run_origin, names) = seen.lock().unwrap().take().unwrap();
    assert_eq!(user.as_deref(), Some("queued-caller"), "runs as the caller");
    assert_eq!(run_origin, Some(origin), "works for the caller's agent run");
    assert!(
        names
            .iter()
            .all(|name| !name.starts_with("_moqui") && !name.starts_with("_origin")),
        "{names:?}"
    );
    assert!(names.contains(&"note".to_string()), "{names:?}");
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
