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

/// The parser itself is tested in moqui-lib (`cron/tests.rs`); this is the framework's use of it.
#[test]
fn quartz_patterns_parse_and_describe() {
    parse_cron("0 0/15 * * * ?").unwrap();
    assert_eq!(parse_cron("0 0 2 * * ?").unwrap().describe(), "At 02:00.");
    let every = parse_cron("* * * * * *").unwrap();
    let now = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::system());
    let next = every.next_after(&now).unwrap();
    let wait = next.timestamp().as_second() - now.timestamp().as_second();
    assert!((0..=1).contains(&wait), "{wait}");
    // Sunday is day 1 and Monday day 2, as in Quartz.
    parse_cron("0 0 8 ? * 2").unwrap();
    assert!(parse_cron("*/5 * * * *")
        .unwrap_err()
        .to_string()
        .contains("6 or 7 fields"));
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
    // `worker-pool-max` 22, less the 2 job workers of `job-pool-core`.
    let pools = super::super::start_pools(&text).unwrap();
    assert_eq!(pools.sizes(), (20, 65_535));
    drop(pools);
    // With no `worker-queue`, Java's code default.
    let missing = super::super::start_pools("<moqui-conf/>").unwrap();
    assert_eq!(missing.sizes(), (20, 65_536));
    drop(missing);
    let junk =
        super::super::start_pools(r#"<moqui-conf><tools worker-pool-max="nope" worker-queue="nope"/></moqui-conf>"#)
            .unwrap();
    assert_eq!(junk.sizes(), (20, 65_536));
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
    jobs_world_on(workers, queue, extra, register, crate::testing::open())
}

/// [`jobs_world_with`] over `facade` (one whose statements go on a tape, say).
fn jobs_world_on(
    workers: usize,
    queue: usize,
    extra: Option<&str>,
    register: impl FnOnce(&ExecutionContextFactory),
    facade: EntityFacade,
) -> Jobs {
    // a file for each harness: tests in parallel would otherwise write and lint one shared file
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("moqui-jobs-{}-{seq}.xml", std::process::id()));
    std::fs::write(&path, SERVICES).unwrap();
    let pools = ServicePools::bounded(workers, queue);
    let ctx = ExecutionContextFactory::test_context_with("", pools);
    register(ctx.factory());
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
        moqui_lib::object::basic_convert(&Value::I64(jiff::Timestamp::now().as_millisecond()), "Timestamp").value,
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
        moqui_lib::object::basic_convert(&Value::I64(jiff::Timestamp::now().as_millisecond()), "Timestamp").value,
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
    let call: serde_json::Value = serde_json::from_str(&text(&stored[0], "queuedCall")).unwrap();
    assert_eq!(call["userName"], "queued-caller");
    assert_eq!(call["agentId"], "queue-agent");
    assert_eq!(call["agentRunId"], "QUEUE_RUN_1");
    assert_eq!(call["parameters"]["note"], "queued-call");
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework")
}

// ---- queued tx rules and job locks (HL-025, HL-026) ---------------------------------------

const MAP_SERVICE: &str = r#"<services>
        <service verb="commit" noun="Map">
            <in-parameters>
                <parameter name="items" type="Map"/>
                <parameter name="tags" type="List"/>
            </in-parameters>
            <out-parameters><parameter name="note"/></out-parameters>
            <actions><set field="note" value="done"/></actions>
        </service>
    </services>"#;

/// The tx-commit rule of `commit#Map` writes a row from a map entry, a list item and a result.
const MAP_RULE: &str = r#"<secas>
        <seca service="commit#Map" when="tx-commit">
            <actions>
                <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                <script>if (ec.entity.find("moqui.test.TestEntity").condition("testId", "BLOCK_RULE").one() != null) throw new Error("the rule is blocked"); made.set("testId", "MAP_RULE"); made.set("testMedium", parameters.items.color + "-" + parameters.tags[1] + "-" + results.note)</script>
                <entity-create value-field="made"/>
            </actions>
        </seca>
    </secas>"#;

fn map_rule_world(workers: usize, queue: usize) -> Jobs {
    jobs_world_with(workers, queue, Some(MAP_SERVICE), |factory| {
        crate::service::eca_rules::install(factory, crate::service::eca_rules::from_xml(MAP_RULE, ""));
    })
}

fn commit_map() {
    let mut items = indexmap::IndexMap::default();
    items.insert("color".to_string(), Value::string("blue"));
    let mut params = BTreeMap::new();
    params.insert("items".to_string(), Value::Map(items));
    params.insert(
        "tags".to_string(),
        Value::List(vec![Value::string("a"), Value::string("b")]),
    );
    crate::service::call::sync("commit#Map", &params).unwrap();
}

fn queued_jobs(facade: &EntityFacade) -> Vec<crate::entity::EntityValue> {
    facade
        .find("moqui.service.job.ServiceJob")
        .unwrap()
        .list()
        .unwrap()
        .values()
        .iter()
        .filter(|job| text(job, "jobName").starts_with(super::super::jobs::QUEUE_PREFIX))
        .cloned()
        .collect()
}

fn rule_row(facade: &EntityFacade) -> Option<String> {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("MAP_RULE"))
        .one()
        .unwrap()
        .map(|row| text(&row, "testMedium"))
}

fn outbox_rows(facade: &EntityFacade) -> Vec<crate::entity::EntityValue> {
    super::super::outbox::rows(facade)
}

/// What a minute after the commit looks like to the scheduler: every outbox row is available.
fn make_outbox_available(facade: &EntityFacade) {
    facade
        .find(super::super::outbox::OUTBOX)
        .unwrap()
        .disable_authz()
        .update_all([("availableDate".to_string(), stamp_ago(1_000))])
        .unwrap();
}

/// plans/archive/High-Level-Review.md HL-025: tx-commit rules that ran after their commit leave no outbox row: the row was
/// written in the committing transaction and the run deleted it in its own.
#[test]
fn tx_commit_rules_that_ran_leave_no_outbox_row() {
    let world = map_rule_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    commit_map();
    wait_for("the rules to run", || rule_row(&world.facade).is_some());
    assert_eq!(rule_row(&world.facade).as_deref(), Some("blue-b-done"));
    wait_for("the outbox row to be deleted", || outbox_rows(&world.facade).is_empty());
    assert!(queued_jobs(&world.facade).is_empty(), "nothing stored as a job");
}

/// HL-025: a call whose transaction rolls back leaves no outbox row, and its tx-commit rules do not run.
#[test]
fn a_rolled_back_call_leaves_no_outbox_row() {
    let world = map_rule_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    let tx = world.facade.default_transactions().unwrap();
    let outcome: Result<(), _> = tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
        commit_map();
        assert_eq!(outbox_rows(&world.facade).len(), 1, "written in the transaction");
        Err(moqui_lib::error::Error::new("stand-in failure after the call"))
    });
    assert!(outcome.is_err());
    assert!(outbox_rows(&world.facade).is_empty(), "rolled back with the call");
    world.ctx.factory().service_pools().tick();
    assert!(rule_row(&world.facade).is_none());
}

/// HL-025: the rules of a call that committed are run from the outbox row when their commit hook never ran (the process
/// stopped first), by the scheduler once the row is available, on a job worker, with the phase, the results and the
/// map and list parameters as they were.
#[test]
fn tx_commit_rules_whose_hook_never_ran_run_from_the_outbox_with_their_maps() {
    let world = map_rule_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    // What register_tx_rules writes, committed, with no hook to run it: as after a crash.
    let mut items = indexmap::IndexMap::default();
    items.insert("color".to_string(), Value::string("blue"));
    let mut results = BTreeMap::new();
    results.insert("note".to_string(), Value::string("done"));
    let rules = super::super::pool::TxRules {
        service: "commit#Map".to_string(),
        when: "tx-commit".to_string(),
        parameters: BTreeMap::from([
            ("items".to_string(), Value::Map(items)),
            (
                "tags".to_string(),
                Value::List(vec![Value::string("a"), Value::string("b")]),
            ),
        ]),
        results: Some(results),
        authz: world.ctx.registered_authz(),
    };
    let tx = world.facade.default_transactions().unwrap();
    tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
        super::super::outbox::write(&world.facade, &tx, &rules, Some("john.doe"), None).map_err(Into::into)
    })
    .unwrap()
    .expect("the default group's transaction keeps an outbox row");
    let stored = outbox_rows(&world.facade);
    assert_eq!(stored.len(), 1);
    let call: serde_json::Value = serde_json::from_str(&text(&stored[0], "queuedCall")).unwrap();
    assert_eq!(call["when"], "tx-commit");
    assert_eq!(call["parameters"]["items"]["color"], "blue");
    assert_eq!(call["results"]["note"], "done");
    assert_eq!(call["userName"], "john.doe");
    assert_eq!(text(&stored[0], "phase"), "tx-commit");
    let pools = world.ctx.factory().service_pools();
    assert_eq!(pools.tick().outbox_runs, 0, "not before its grace");
    make_outbox_available(&world.facade);
    assert_eq!(pools.tick().outbox_runs, 1);
    wait_for("the rules to run from the outbox", || rule_row(&world.facade).is_some());
    assert_eq!(rule_row(&world.facade).as_deref(), Some("blue-b-done"));
    wait_for("the outbox row to be deleted", || outbox_rows(&world.facade).is_empty());
}

/// HL-025: tx-commit rules that find the service queue full stay in their outbox row (no job is stored), and the
/// scheduler runs them on a job worker while the service queue is still full.
#[test]
fn tx_commit_rules_that_find_the_queue_full_stay_in_the_outbox_and_run_later() {
    let world = map_rule_world(1, 1);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    let pools = world.ctx.factory().service_pools();
    let hold = Hold::new();
    fill_queue(pools, &hold);
    commit_map();
    assert!(queued_jobs(&world.facade).is_empty(), "no job is stored");
    let stored = outbox_rows(&world.facade);
    assert_eq!(stored.len(), 1);
    assert_eq!(text(&stored[0], "serviceName"), "commit#Map");
    let call: serde_json::Value = serde_json::from_str(&text(&stored[0], "queuedCall")).unwrap();
    assert_eq!(call["parameters"]["tags"][1], "b");
    // Registered with authorization disabled, the rules run so.
    assert_eq!(call["authz"]["disabled"], true);
    assert!(rule_row(&world.facade).is_none());
    make_outbox_available(&world.facade);
    wait_for("the rules to run from the outbox", || {
        pools.tick();
        rule_row(&world.facade).is_some()
    });
    assert_eq!(rule_row(&world.facade).as_deref(), Some("blue-b-done"));
    wait_for("the outbox row to be deleted", || outbox_rows(&world.facade).is_empty());
    hold.open();
}

/// HL-025: rules still in the service queue when the pool stops with no wait stay in the outbox, which a later pool's
/// scheduler runs.
#[test]
fn tx_commit_rules_queued_at_a_zero_wait_stop_stay_in_the_outbox() {
    let world = map_rule_world(1, 4);
    {
        let _entered = world.ctx.enter();
        let _authz = world.ctx.disable_authz();
        let pools = world.ctx.factory().service_pools();
        let shared = pools.shared_handle();
        let hold = Hold::new();
        let task_hold = hold.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        pools
            .submit_task(move || {
                started_tx.send(()).unwrap();
                task_hold.wait();
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        commit_map();
        assert_eq!(
            moqui_lib::sync::lock(&shared.state).queue.len(),
            1,
            "the queue had room"
        );
        let stopping = {
            let factory = Arc::clone(world.ctx.factory());
            std::thread::spawn(move || factory.service_pools().shutdown_now())
        };
        wait_for("the stop to take the queue", || {
            moqui_lib::sync::lock(&shared.state).queue.is_empty()
        });
        hold.open();
        stopping.join().unwrap();
    }
    assert!(rule_row(&world.facade).is_none());
    assert_eq!(outbox_rows(&world.facade).len(), 1, "kept in the outbox");
    assert!(queued_jobs(&world.facade).is_empty(), "no job is stored");
    let later = map_rule_world(1, 4);
    later.ctx.factory().set_entity(world.facade.clone());
    let _entered = later.ctx.enter();
    make_outbox_available(&world.facade);
    let pools = later.ctx.factory().service_pools();
    wait_for("the kept rules to run", || {
        pools.tick();
        rule_row(&world.facade).is_some()
    });
    assert_eq!(rule_row(&world.facade).as_deref(), Some("blue-b-done"));
}

/// HL-025: a run of the rules that fails rolls back with its outbox row's delete; the row counts the attempt, keeps the
/// error and waits, and a later run from the outbox that succeeds removes it.
#[test]
fn a_failed_tx_rules_run_is_counted_in_the_outbox_and_tried_again() {
    let world = map_rule_world(2, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    // A BLOCK_RULE row makes the rule fail.
    create(&world.facade, "moqui.test.TestEntity", &[("testId", "BLOCK_RULE")]);
    commit_map();
    wait_for("the failure to be recorded", || {
        outbox_rows(&world.facade)
            .first()
            .is_some_and(|row| text(row, "attemptCount") == "1")
    });
    let row = outbox_rows(&world.facade).remove(0);
    assert!(!text(&row, "lastError").is_empty());
    assert!(!text(&row, "lastAttemptDate").is_empty());
    let pools = world.ctx.factory().service_pools();
    assert_eq!(pools.tick().outbox_runs, 0, "it waits after the failure");
    let mut blocking = world.facade.make_value("moqui.test.TestEntity").unwrap();
    blocking.set("testId", Value::string("BLOCK_RULE"));
    blocking.delete(&world.facade).unwrap();
    make_outbox_available(&world.facade);
    assert_eq!(pools.tick().outbox_runs, 1);
    wait_for("the second run to commit", || outbox_rows(&world.facade).is_empty());
    assert_eq!(rule_row(&world.facade).as_deref(), Some("blue-b-done"));
}

fn lock_row_of(facade: &EntityFacade, name: &str) -> Option<crate::entity::EntityValue> {
    facade
        .find("moqui.service.job.ServiceJobRunLock")
        .unwrap()
        .eq("jobName", Value::string(name))
        .one()
        .unwrap()
}

/// A pass claims a lock only for a job worker that is idle: with both busy, the third due job's lock is left for
/// another member.
#[test]
fn with_both_job_workers_busy_a_third_due_job_is_not_claimed() {
    let hold = Hold::new();
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let blocking = r#"<services>
            <service verb="block" noun="Job" type="rust" location="test::blocking_job"/>
        </services>"#;
    let (job_hold, job_started) = (hold.clone(), Arc::clone(&started));
    let world = jobs_world_with(1, 8, Some(blocking), move |factory| {
        factory
            .register_service("test::blocking_job", move |_ec, _info, _parameters| {
                job_started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                job_hold.wait();
                Ok(BTreeMap::new())
            })
            .unwrap();
    });
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(&world, "B1", "block#Job", "* * * * * *", &[]);
    add_job(&world, "B2", "block#Job", "* * * * * *", &[]);
    add_job(&world, "C3", "echo#Note", "* * * * * *", &[]);
    let pools = world.ctx.factory().service_pools();
    assert_eq!(pools.tick().jobs_run, 2);
    wait_for("both job workers to be busy", || {
        started.load(std::sync::atomic::Ordering::SeqCst) == 2
    });
    let report = pools.tick();
    assert_eq!((report.jobs_run, report.jobs_not_checked), (0, 3), "{report:?}");
    assert!(
        lock_row_of(&world.facade, "C3").is_none(),
        "the third job's lock is not claimed"
    );
    hold.open();
    wait_for("a worker to be free and the third job to run", || {
        pools.tick();
        runs(&world, "C3").iter().any(|run| text(run, "hasError") == "N")
    });
}

/// A lock held by a member that is not live is taken over before `expireLockTime`, and the run it named is ended as
/// failed; a lock held by a live member is not.
#[test]
fn a_lock_held_by_a_gone_member_is_taken_over_before_it_expires() {
    let world = jobs_world(1, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    let stamp = |ago: i64| {
        moqui_lib::object::basic_convert(&Value::I64(jiff::Timestamp::now().as_millisecond() - ago), "Timestamp").value
    };
    let now = stamp(0);
    // Ten seconds ago: far from expiring, and the every-second schedule is due again.
    let started = stamp(10_000);
    for (job, run, instance) in [("GONE", "GONE-RUN", "gone-member"), ("LIVE", "LIVE-RUN", "live-member")] {
        add_job(&world, job, "echo#Note", "* * * * * *", &[]);
        create(
            &world.facade,
            "moqui.service.job.ServiceJobRun",
            &[("jobRunId", run), ("jobName", job)],
        );
        let mut lock = world.facade.make_value("moqui.service.job.ServiceJobRunLock").unwrap();
        lock.set("jobName", Value::string(job));
        lock.set("jobRunId", Value::string(run));
        lock.set("instanceId", Value::string(instance));
        lock.set("lastRunTime", started.clone());
        lock.create(&world.facade).unwrap();
    }
    let mut member = world.facade.make_value("moqui.server.ClusterMember").unwrap();
    member.set("instanceId", Value::string("live-member"));
    member.set("heartbeat", now.clone());
    member.create(&world.facade).unwrap();
    let report = world.ctx.factory().service_pools().tick();
    assert_eq!(report.jobs_run, 1, "{report:?}");
    let gone = lock_row_of(&world.facade, "GONE").unwrap();
    assert_ne!(text(&gone, "jobRunId"), "GONE-RUN", "taken over");
    let old = runs(&world, "GONE")
        .into_iter()
        .find(|run| text(run, "jobRunId") == "GONE-RUN")
        .unwrap();
    assert_eq!(text(&old, "hasError"), "Y");
    let live = lock_row_of(&world.facade, "LIVE").unwrap();
    assert_eq!(text(&live, "jobRunId"), "LIVE-RUN", "a live member keeps its lock");
    wait_for("the taken-over job to run", || {
        runs(&world, "GONE").iter().any(|run| text(run, "hasError") == "N")
    });
}

/// plans/archive/High-Level-Review.md HL-035: a job's `transactionTimeout` bounds its scripts though the service begins no
/// transaction: a looping script ends interrupted, and the job's lock is released, within seconds.
#[test]
fn a_looping_job_script_ends_at_the_jobs_timeout_and_releases_its_lock() {
    let world = jobs_world_with(
        2,
        8,
        Some(
            r#"<services>
            <service verb="loop" noun="Forever" authenticate="false" transaction="ignore">
                <actions><script>while (true) {}</script></actions>
            </service>
            <service verb="loop" noun="Declared" authenticate="false" transaction="ignore" transaction-timeout="1">
                <actions><script>while (true) {}</script></actions>
            </service>
        </services>"#,
        ),
        |_| {},
    );
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(
        &world,
        "LOOPS",
        "loop#Forever",
        "* * * * * *",
        &[("transactionTimeout", "1")],
    );
    let started = Instant::now();
    let report = world.ctx.factory().service_pools().tick();
    assert_eq!(report.jobs_run, 1, "{report:?}");
    wait_for("the run to end", || {
        runs(&world, "LOOPS").iter().any(|run| !text(run, "endTime").is_empty())
    });
    let run = &runs(&world, "LOOPS")[0];
    assert_eq!(text(run, "hasError"), "Y");
    assert!(text(run, "errors").contains("interrupted"), "{}", text(run, "errors"));
    let lock = world
        .facade
        .find("moqui.service.job.ServiceJobRunLock")
        .unwrap()
        .eq("jobName", Value::string("LOOPS"))
        .one()
        .unwrap()
        .expect("the lock row");
    assert_eq!(text(&lock, "jobRunId"), "", "the lock is released");
    assert!(started.elapsed() < Duration::from_secs(6), "{:?}", started.elapsed());
    // Called directly, as an async call's worker calls it, a service that begins no transaction is bounded by the
    // transaction-timeout it declares.
    let started = Instant::now();
    // The script's error is the call's message, as a service's error is.
    let _ = super::super::call::sync("loop#Declared", &BTreeMap::new());
    let errors = world.ctx.message().errors().join("\n");
    assert!(errors.contains("interrupted"), "{errors}");
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
}

/// A long batch job opts out of the job's time limit with `noTimeLimit`; otherwise the limit is `transactionTimeout`,
/// 1800 seconds when it is empty.
#[test]
fn a_job_says_no_time_limit_to_run_its_scripts_without_one() {
    let facade = crate::testing::open();
    let job = |fields: &[(&str, &str)]| {
        let mut row = facade.make_value("moqui.service.job.ServiceJob").unwrap();
        for (name, text) in fields {
            row.set(*name, Value::string(*text));
        }
        row
    };
    let limit = super::super::jobs::script_limit;
    assert_eq!(limit(&job(&[])), Some(Duration::from_secs(1800)));
    assert_eq!(
        limit(&job(&[("transactionTimeout", "5")])),
        Some(Duration::from_secs(5))
    );
    assert_eq!(limit(&job(&[("transactionTimeout", "5"), ("noTimeLimit", "Y")])), None);
}

/// plans/archive/High-Level-Review.md HL-026: a pass reads every scheduled job but only as many queued calls as it has free
/// slots, in priority order; a job with no priority comes after every job with one, on every database.
#[test]
fn a_pass_reads_only_the_queued_calls_its_free_slots_can_take() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    let job = |name: &str, fields: &[(&str, &str)]| {
        let mut row = facade.make_value("moqui.service.job.ServiceJob").unwrap();
        row.set("jobName", Value::string(name));
        row.set("serviceName", Value::string("echo#Note"));
        for (field, text) in fields {
            row.set(*field, Value::string(*text));
        }
        row.create(&facade).unwrap();
    };
    job("S_NONE", &[("cronExpression", "* * * * * *")]);
    job("S_FIRST", &[("cronExpression", "* * * * * *"), ("priority", "1")]);
    job("ADHOC", &[]);
    for index in 0..40 {
        job(&format!("ASYNC_QUEUE_{index:03}"), &[("priority", "9")]);
    }
    job("ASYNC_QUEUE_NULL", &[]);
    let names = |slots: i64| -> Vec<String> {
        super::super::jobs::discover(&facade, slots)
            .unwrap()
            .iter()
            .map(|job| text(job, "jobName"))
            .collect()
    };
    let before = tape.lock().unwrap().len();
    assert_eq!(
        names(3),
        [
            "S_FIRST",
            "ASYNC_QUEUE_000",
            "ASYNC_QUEUE_001",
            "ASYNC_QUEUE_002",
            "S_NONE"
        ]
    );
    let reads: Vec<String> = tape.lock().unwrap()[before..]
        .iter()
        .filter(|sql| sql.starts_with("SELECT") && sql.contains("\"service_job\""))
        .cloned()
        .collect();
    assert_eq!(reads.len(), 2, "the scheduled jobs, then three queued calls: {reads:?}");
    assert!(reads[1].contains("LIMIT"), "{reads:?}");
    assert_eq!(names(0), ["S_FIRST", "S_NONE"], "no free slot reads no queued call");
    let all = names(100);
    assert_eq!(all.len(), 43);
    assert_eq!(all[..2], ["S_FIRST".to_string(), "ASYNC_QUEUE_000".to_string()]);
    assert_eq!(all[41..], ["ASYNC_QUEUE_NULL".to_string(), "S_NONE".to_string()]);
}

/// The statements on `tape` from `from` on that read `table` (its quoted name).
fn reads_of(tape: &Mutex<Vec<String>>, from: usize, table: &str) -> Vec<String> {
    let quoted = format!("\"{table}\"");
    tape.lock().unwrap()[from..]
        .iter()
        .filter(|sql| sql.starts_with("SELECT") && sql.contains(&quoted))
        .cloned()
        .collect()
}

/// plans/archive/High-Level-Review.md HL-020: a pass reads the lock rows of the jobs it may start in one query. A job that is
/// not due (it ran after its schedule's last occurrence), one whose lock a live run holds, and one whose schedule names
/// nothing before now cost no query of their own: no run history, no repeat count, no member liveness.
#[test]
fn a_pass_reads_locks_in_one_query_and_a_job_not_due_costs_none_of_its_own() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    let world = jobs_world_on(2, 8, None, |_| {}, facade);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    let now = moqui_lib::object::basic_convert(&Value::I64(jiff::Timestamp::now().as_millisecond()), "Timestamp").value;
    // Every first of January: each ran just now, after that occurrence.
    let yearly = "0 0 0 1 1 ?";
    add_job(&world, "RAN", "echo#Note", yearly, &[]);
    add_job(&world, "RAN_COUNTED", "echo#Note", yearly, &[("repeatCount", "5")]);
    add_job(&world, "RUNNING", "echo#Note", yearly, &[]);
    add_job(&world, "LATER", "echo#Note", "0 0 0 1 1 ? 2099", &[]);
    for job in ["RAN", "RAN_COUNTED", "RUNNING"] {
        let mut lock = world.facade.make_value("moqui.service.job.ServiceJobRunLock").unwrap();
        lock.set("jobName", Value::string(job));
        lock.set("lastRunTime", now.clone());
        if job == "RUNNING" {
            lock.set("jobRunId", Value::string("RUNNING_RUN"));
            lock.set("instanceId", Value::string("another-member"));
        }
        lock.create(&world.facade).unwrap();
    }
    let before = tape.lock().unwrap().len();
    let report = world.ctx.factory().service_pools().tick();
    assert_eq!(
        (report.jobs_run, report.jobs_active, report.jobs_failed),
        (0, 4, 0),
        "{report:?}"
    );
    let locks = reads_of(&tape, before, "service_job_run_lock");
    assert_eq!(locks.len(), 1, "one read of the lock rows: {locks:?}");
    assert!(locks[0].contains(" IN "), "{locks:?}");
    assert_eq!(
        reads_of(&tape, before, "service_job_run"),
        Vec::<String>::new(),
        "no run history or count"
    );
    assert_eq!(
        reads_of(&tape, before, "cluster_member"),
        Vec::<String>::new(),
        "no liveness read"
    );
}

// ---- the job lease (HL-026) ---------------------------------------------------------------

/// A timestamp `ago` milliseconds before now (after it, for a negative `ago`).
fn stamp_ago(ago: i64) -> Value {
    moqui_lib::object::basic_convert(&Value::I64(jiff::Timestamp::now().as_millisecond() - ago), "Timestamp").value
}

fn lease_millis_of(facade: &EntityFacade, job: &str) -> Option<i64> {
    match lock_row_of(facade, job)?.get("leaseExpireDate") {
        Some(Value::Timestamp(at)) => Some(at.as_millisecond()),
        _ => None,
    }
}

/// plans/archive/High-Level-Review.md HL-026: a lock whose lease lapsed is taken over though its holder is a live member, and
/// the run it named is ended as failed; a live member's lock whose lease has not lapsed is kept. The new run's claim
/// gives it a lease, and its end clears it.
#[test]
fn a_lock_whose_lease_lapsed_on_a_live_member_is_taken_over() {
    let world = jobs_world(1, 8);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    // Ten seconds ago: far from expiring, and the every-second schedule is due again.
    let started = stamp_ago(10_000);
    for (job, run, lease) in [
        ("LAPSED", "LAPSED-RUN", stamp_ago(60_000)),
        ("LEASED", "LEASED-RUN", stamp_ago(-60_000)),
    ] {
        add_job(&world, job, "echo#Note", "* * * * * *", &[]);
        create(
            &world.facade,
            "moqui.service.job.ServiceJobRun",
            &[("jobRunId", run), ("jobName", job)],
        );
        let mut lock = world.facade.make_value("moqui.service.job.ServiceJobRunLock").unwrap();
        lock.set("jobName", Value::string(job));
        lock.set("jobRunId", Value::string(run));
        lock.set("instanceId", Value::string("live-member"));
        lock.set("lastRunTime", started.clone());
        lock.set("leaseExpireDate", lease);
        lock.create(&world.facade).unwrap();
    }
    let mut member = world.facade.make_value("moqui.server.ClusterMember").unwrap();
    member.set("instanceId", Value::string("live-member"));
    member.set("heartbeat", stamp_ago(0));
    member.create(&world.facade).unwrap();
    let report = world.ctx.factory().service_pools().tick();
    assert_eq!(report.jobs_run, 1, "{report:?}");
    let lapsed = lock_row_of(&world.facade, "LAPSED").unwrap();
    assert_ne!(text(&lapsed, "jobRunId"), "LAPSED-RUN", "taken over");
    let old = runs(&world, "LAPSED")
        .into_iter()
        .find(|run| text(run, "jobRunId") == "LAPSED-RUN")
        .unwrap();
    assert_eq!(text(&old, "hasError"), "Y");
    assert_eq!(
        text(&lock_row_of(&world.facade, "LEASED").unwrap(), "jobRunId"),
        "LEASED-RUN",
        "a lease that has not lapsed keeps its lock"
    );
    wait_for("the taken-over job to run and release its lock", || {
        runs(&world, "LAPSED").iter().any(|run| text(run, "hasError") == "N")
            && lock_row_of(&world.facade, "LAPSED").is_some_and(|lock| text(&lock, "jobRunId").is_empty())
    });
    assert_eq!(
        lease_millis_of(&world.facade, "LAPSED"),
        None,
        "the release clears the lease"
    );
}

/// A world with `block#Job`, a rust service that waits on `hold`, counting its starts in `started`.
fn blocking_world(hold: &Hold, started: &Arc<std::sync::atomic::AtomicUsize>) -> Jobs {
    let blocking = r#"<services>
            <service verb="block" noun="Job" type="rust" location="test::blocking_job"/>
        </services>"#;
    let (job_hold, job_started) = (hold.clone(), Arc::clone(started));
    jobs_world_with(1, 8, Some(blocking), move |factory| {
        factory
            .register_service("test::blocking_job", move |_ec, _info, _parameters| {
                job_started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                job_hold.wait();
                Ok(BTreeMap::new())
            })
            .unwrap();
    })
}

/// HL-026: the claim gives a running job's lock a lease, and a renewal moves it on while the job runs.
#[test]
fn a_running_job_renews_its_lease() {
    let hold = Hold::new();
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let world = blocking_world(&hold, &started);
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    add_job(&world, "LEASE1", "block#Job", "* * * * * *", &[]);
    let pools = world.ctx.factory().service_pools();
    assert_eq!(pools.tick().jobs_run, 1);
    wait_for("the job to start", || {
        started.load(std::sync::atomic::Ordering::SeqCst) == 1
    });
    let claimed = lease_millis_of(&world.facade, "LEASE1").expect("the claim's lease");
    assert!(
        claimed > jiff::Timestamp::now().as_millisecond(),
        "a lease ahead of now"
    );
    std::thread::sleep(Duration::from_millis(5));
    let renewal = super::super::job_lease::renew(world.ctx.factory()).unwrap();
    assert_eq!(
        (renewal.renewed, renewal.lost.len(), renewal.overdue.len()),
        (1, 0, 0),
        "{renewal:?}"
    );
    let renewed = lease_millis_of(&world.facade, "LEASE1").unwrap();
    assert!(renewed > claimed, "{renewed} after {claimed}");
    hold.open();
    wait_for("the run to end", || {
        runs(&world, "LEASE1").iter().any(|run| text(run, "hasError") == "N")
    });
}

/// HL-026: a run whose lock another run took over has its scripts ended at the next renewal, rather than going on beside
/// the run that holds the lock now, which keeps it.
#[test]
fn a_run_that_lost_its_lock_is_cut_short() {
    let looping = r#"<services>
            <service verb="loop" noun="Long" authenticate="false" transaction="ignore">
                <actions><script>while (true) {}</script></actions>
            </service>
        </services>"#;
    let world = jobs_world_with(1, 8, Some(looping), |_| {});
    let _entered = world.ctx.enter();
    let _authz = world.ctx.disable_authz();
    // A timeout far off, so only the lost lock ends it.
    add_job(
        &world,
        "LOSER",
        "loop#Long",
        "* * * * * *",
        &[("transactionTimeout", "600")],
    );
    let started = Instant::now();
    assert_eq!(world.ctx.factory().service_pools().tick().jobs_run, 1);
    wait_for("the run to start", || {
        runs(&world, "LOSER")
            .iter()
            .any(|run| !text(run, "startTime").is_empty())
    });
    let run_id = text(&runs(&world, "LOSER")[0], "jobRunId");
    let mut lock = lock_row_of(&world.facade, "LOSER").unwrap();
    lock.set("jobRunId", Value::string("ELSEWHERE"));
    lock.update(&world.facade).unwrap();
    let renewal = super::super::job_lease::renew(world.ctx.factory()).unwrap();
    assert_eq!(renewal.lost, [run_id], "{renewal:?}");
    wait_for("the run to end", || {
        runs(&world, "LOSER").iter().any(|run| !text(run, "endTime").is_empty())
    });
    let run = runs(&world, "LOSER").remove(0);
    assert_eq!(text(&run, "hasError"), "Y");
    assert!(text(&run, "errors").contains("interrupted"), "{}", text(&run, "errors"));
    assert_eq!(
        text(&lock_row_of(&world.facade, "LOSER").unwrap(), "jobRunId"),
        "ELSEWHERE",
        "the other run keeps the lock"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}
