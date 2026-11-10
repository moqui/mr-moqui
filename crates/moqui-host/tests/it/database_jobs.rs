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

//! Database jobs on the embedded cluster (plans/archive/Tools-Database.md §6, §15 step 3): a `CREATE INDEX
//! CONCURRENTLY` job runs on a service worker to a valid index; a second, held up by an open transaction, is cancelled
//! through `pg_cancel_backend` as from another instance, leaves the invalid index the cancel leaves, and run again ends
//! with a valid index.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_core::transaction::TransactionAttribute;
use moqui_lib::object::{to_plain_string, Value};

use crate::common;

const SERVICES: &str = "org.moqui.dev.DatabaseServices";

fn call(verb_noun: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    moqui_core::service::call::sync_checked(
        &format!("{SERVICES}.{verb_noun}"),
        &parameters,
        // the Tools screens grant the services by inheritance; called alone, each checks the Tools app and
        // DATABASE_ADMIN itself
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    )
    .unwrap_or_else(|err| panic!("{verb_noun}: {err}"))
}

fn text(map: &BTreeMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

/// A job as get#DatabaseJobStatus gives it: its status, whether its runner ended, its statements, and the whole out.
type JobStatus = (String, bool, Vec<moqui_lib::object::ValueMap>, BTreeMap<String, Value>);

fn status_of(job_id: &str) -> JobStatus {
    let out = call("get#DatabaseJobStatus", &[("jobId", Value::string(job_id))]);
    let Some(Value::Map(job)) = out.get("job") else {
        panic!("{out:?}")
    };
    let statements = list(&out, "statements");
    let ended = job.get("endTime").is_some_and(|end| !end.is_null());
    (to_plain_string(&job["statusId"]), ended, statements, out)
}

fn status(job_id: &str) -> (String, Vec<moqui_lib::object::ValueMap>) {
    let (status, _, statements, _) = status_of(job_id);
    (status, statements)
}

fn list(map: &BTreeMap<String, Value>, key: &str) -> Vec<moqui_lib::object::ValueMap> {
    match map.get(key) {
        Some(Value::List(rows)) => rows
            .iter()
            .filter_map(|row| match row {
                Value::Map(row) => Some(row.clone()),
                _ => None,
            })
            .collect(),
        other => panic!("{key}: {other:?}"),
    }
}

fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < limit, "{what} within {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until the job's runner has finished: it writes the end time last.
fn wait_ended(job_id: &str) -> String {
    let mut status = String::new();
    wait_for("the job's runner to finish", Duration::from_secs(30), || {
        let (now, ended, _, _) = status_of(job_id);
        status = now;
        ended
    });
    status
}

fn sql(facade: &moqui_core::entity::EntityFacade, statement: &str) -> moqui_core::entity::info::SqlResult {
    moqui_core::entity::info::run_sql(facade, "transactional", statement, 0)
        .unwrap_or_else(|err| panic!("{statement}: {err}"))
}

/// The non-key indexes of `table`, sorted.
fn indexes_of(facade: &moqui_core::entity::EntityFacade, table: &str) -> Vec<String> {
    sql(
        facade,
        &format!(
            "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema() AND tablename = '{table}' \
             AND indexname NOT LIKE '%pkey' ORDER BY 1"
        ),
    )
    .records
    .iter()
    .map(|row| to_plain_string(&row[0]))
    .collect()
}

/// Whether the index `name` of the current schema is valid; `None` when there is no such index.
fn index_valid(facade: &moqui_core::entity::EntityFacade, name: &str) -> Option<bool> {
    let rows = sql(
        facade,
        &format!(
            "SELECT x.indisvalid FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid WHERE i.relname = '{name}'"
        ),
    );
    rows.records.first().map(|row| row[0] == Value::Bool(true))
}

/// The plan of the missing index `name` of Enumeration, stored as a job: its id.
fn plan_index(name: &str) -> String {
    let plan = call(
        "plan#SchemaChanges",
        &[
            (
                "entityNames",
                Value::List(vec![Value::string("moqui.basic.Enumeration")]),
            ),
            ("changes", Value::List(vec![Value::string("missingIndexes")])),
            ("names", Value::List(vec![Value::string(name)])),
            ("description", Value::string(format!("index {name}"))),
        ],
    );
    let statements = list(&plan, "statements");
    assert_eq!(statements.len(), 1, "{plan:?}");
    assert!(
        to_plain_string(&statements[0]["statementText"]).starts_with("CREATE INDEX CONCURRENTLY IF NOT EXISTS"),
        "{plan:?}"
    );
    let job_id = text(&plan, "jobId");
    assert!(!job_id.is_empty(), "{plan:?}");
    job_id
}

/// A `CREATE INDEX CONCURRENTLY` plan job runs on a service worker to a valid index; a second, held by an open
/// transaction, is cancelled through `pg_cancel_backend` as from another instance and leaves an invalid index; run
/// again, it drops that index and builds it valid.
#[test]
fn a_concurrent_index_job_runs_is_cancelled_through_the_backend_and_runs_again() {
    if !common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start("database-jobs");
    let factory = gate.server.factory();
    let facade = factory.entity().expect("entity facade");
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let indexes = indexes_of(&facade, "enumeration");
    assert!(indexes.len() >= 2, "{indexes:?}");
    let (one, two) = (indexes[0].clone(), indexes[1].clone());
    sql(&facade, &format!("DROP INDEX \"{one}\""));
    sql(&facade, &format!("DROP INDEX \"{two}\""));

    // A plan job runs on a service worker to a valid index.
    let first = plan_index(&one);
    let queued = call("run#DatabaseJob", &[("jobId", Value::string(&first))]);
    assert_eq!(queued.get("queued"), Some(&Value::Bool(true)), "{queued:?}");
    assert_eq!(wait_ended(&first), "DbjsDone");
    let (_, statements) = status(&first);
    assert_eq!(to_plain_string(&statements[0]["statusId"]), "DbjsDone");
    assert!(
        statements[0].get("backendStart").is_some_and(|start| !start.is_null()),
        "{statements:?}"
    );
    assert_eq!(index_valid(&facade, &one), Some(true));

    // An open transaction holds the second build in its wait for older snapshots.
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let holder_facade = facade.clone();
    let holder = std::thread::spawn(move || {
        let tx = holder_facade.default_transactions().unwrap();
        tx.run(TransactionAttribute::UseOrBegin, Some(Duration::from_secs(60)), || {
            tx.with_connection(|conn| conn.query_once("select txid_current()", &[]))?;
            ready_tx.send(()).unwrap();
            let _ = held_rx.recv_timeout(Duration::from_secs(50));
            Ok(())
        })
        .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let second = plan_index(&two);
    call("run#DatabaseJob", &[("jobId", Value::string(&second))]);
    wait_for("the second build to wait", Duration::from_secs(20), || {
        let (job, _, statements, _) = status_of(&second);
        job == "DbjsRunning"
            && statements[0].get("backendPid").is_some_and(|pid| !pid.is_null())
            && index_valid(&facade, &two) == Some(false)
    });
    // while it waits, its progress and what blocks it read without error
    let (_, _, _, out) = status_of(&second);
    let Some(Value::Map(reasons)) = out.get("reasons") else {
        panic!("{out:?}")
    };
    assert!(reasons.is_empty(), "{reasons:?}");
    assert!(!list(&out, "progress").is_empty(), "{out:?}");
    // A second run of a running job is refused.
    let parameters: BTreeMap<String, Value> = [("jobId".to_string(), Value::string(&second))].into_iter().collect();
    let refused = moqui_core::service::call::sync_checked(
        &format!("{SERVICES}.run#DatabaseJob"),
        &parameters,
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    );
    assert!(refused.is_err_and(|err| err.to_string().contains("is running")));
    moqui_core::context::ExecutionContext::with_current(|ec| ec.message().clear_errors());
    // As from another instance: the job's row names one, so the cancel goes by the backend pid and start.
    {
        let _authz = context.disable_authz();
        let mut job = facade
            .find("moqui.database.DatabaseJob")
            .unwrap()
            .eq("jobId", Value::string(&second))
            .one()
            .unwrap()
            .unwrap();
        job.set("instanceId", Value::string("another-instance"));
        job.update(&facade).unwrap();
    }
    let cancelled = call("cancel#DatabaseJob", &[("jobId", Value::string(&second))]);
    assert_eq!(text(&cancelled, "how"), "backend", "{cancelled:?}");
    assert_eq!(wait_ended(&second), "DbjsCancelled");
    let (_, statements) = status(&second);
    assert_eq!(
        to_plain_string(&statements[0]["statusId"]),
        "DbjsCancelled",
        "{statements:?}"
    );
    assert_eq!(
        index_valid(&facade, &two),
        Some(false),
        "the cancel leaves an invalid index"
    );

    // Run again: the invalid index is dropped and built again, valid.
    held_tx.send(()).unwrap();
    holder.join().unwrap();
    let rerun = call(
        "run#DatabaseJob",
        &[("jobId", Value::string(&second)), ("wait", Value::Bool(true))],
    );
    assert_eq!(text(&rerun, "statusId"), "DbjsDone", "{rerun:?}");
    assert_eq!(index_valid(&facade, &two), Some(true));
}

/// A plan's key that times out on its lock fails, the VALIDATE that depends on it is skipped, and an index of another
/// table in the same job is built; the table, activity and index health services answer, and the app's own session is
/// not cancelled without force.
#[test]
fn plans_jobs_tables_and_activity_on_postgres() {
    if !common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start("database-plans");
    let factory = gate.server.factory();
    let facade = factory.entity().expect("entity facade");
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let entities = Value::List(vec![
        Value::string("moqui.basic.Uom"),
        Value::string("moqui.basic.StatusItem"),
    ]);
    let clean = call("compare#Schema", &[("entityNames", entities.clone())]);
    assert!(list(&clean, "tables").is_empty(), "{clean:?}");

    let key = sql(
        &facade,
        "SELECT c.conname FROM pg_constraint c JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_class r ON r.oid = c.confrelid WHERE c.contype = 'f' AND t.relname = 'uom' \
         AND r.relname = 'enumeration'",
    );
    let key = to_plain_string(&key.records[0][0]);
    sql(&facade, &format!("ALTER TABLE uom DROP CONSTRAINT \"{key}\""));
    let status_index = indexes_of(&facade, "status_item")[0].clone();
    sql(&facade, &format!("DROP INDEX \"{status_index}\""));
    let plan = call(
        "plan#SchemaChanges",
        &[("entityNames", entities.clone()), ("lockTimeoutMs", Value::I64(200))],
    );
    let texts: Vec<String> = list(&plan, "statements")
        .iter()
        .map(|row| to_plain_string(&row["statementText"]))
        .collect();
    assert_eq!(texts.len(), 3, "{plan:?}");
    assert!(texts[0].ends_with("NOT VALID"), "{texts:?}");
    assert!(texts[1].contains("VALIDATE CONSTRAINT"), "{texts:?}");
    assert!(texts[2].starts_with("CREATE INDEX CONCURRENTLY"), "{texts:?}");
    let job_id = text(&plan, "jobId");

    // the key's table is locked (one the jobs and the audit do not use): the ADD waits its 200 ms lock timeout
    // out, four times
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let holder_facade = facade.clone();
    let holder = std::thread::spawn(move || {
        let tx = holder_facade.default_transactions().unwrap();
        tx.run(TransactionAttribute::UseOrBegin, Some(Duration::from_secs(60)), || {
            tx.with_connection(|conn| conn.execute("LOCK TABLE uom IN ACCESS EXCLUSIVE MODE", &[]))?;
            ready_tx.send(()).unwrap();
            let _ = held_rx.recv_timeout(Duration::from_secs(50));
            Ok(())
        })
        .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let ran = call(
        "run#DatabaseJob",
        &[("jobId", Value::string(&job_id)), ("wait", Value::Bool(true))],
    );
    held_tx.send(()).unwrap();
    holder.join().unwrap();
    assert_eq!(text(&ran, "statusId"), "DbjsFailed", "{ran:?}");
    let (_, statements) = status(&job_id);
    let statuses: Vec<String> = statements.iter().map(|row| to_plain_string(&row["statusId"])).collect();
    assert_eq!(statuses, ["DbjsFailed", "DbjsSkipped", "DbjsDone"], "{statements:?}");
    assert!(
        to_plain_string(&statements[0]["errorText"]).contains("lock timeout"),
        "{statements:?}"
    );
    // run again with the lock gone: the key is added and validated
    let rerun = call(
        "run#DatabaseJob",
        &[("jobId", Value::string(&job_id)), ("wait", Value::Bool(true))],
    );
    assert_eq!(
        text(&rerun, "statusId"),
        "DbjsDone",
        "{rerun:?} {:?}",
        status(&job_id).1
    );
    assert!(list(&call("compare#Schema", &[("entityNames", entities)]), "tables").is_empty());

    let ddl = call(
        "get#SchemaDdl",
        &[(
            "entityNames",
            Value::List(vec![Value::string("moqui.basic.Enumeration")]),
        )],
    );
    assert!(text(&ddl, "ddlText").contains("CREATE TABLE"), "{ddl:?}");
    let tables = call("get#TableStats", &[]);
    assert!(
        list(&tables, "tables")
            .iter()
            .any(|row| to_plain_string(&row["entity_name"]) == "moqui.basic.Enumeration"),
        "{tables:?}"
    );
    let count = call("count#TableRows", &[("tableName", Value::string("enumeration"))]);
    assert!(text(&count, "rowCount").parse::<i64>().unwrap() > 0, "{count:?}");
    let maintenance = call(
        "plan#TableMaintenance",
        &[
            ("tableNames", Value::string("enumeration")),
            ("action", Value::string("vacuumAnalyze")),
        ],
    );
    assert_eq!(
        to_plain_string(&list(&maintenance, "statements")[0]["statementText"]),
        "VACUUM (ANALYZE) \"enumeration\""
    );
    let maintenance_job = text(&maintenance, "jobId");
    let ran = call(
        "run#DatabaseJob",
        &[("jobId", Value::string(&maintenance_job)), ("wait", Value::Bool(true))],
    );
    assert_eq!(text(&ran, "statusId"), "DbjsDone", "{ran:?}");
    let wrap = call(
        "plan#LoadWrap",
        &[(
            "entityNames",
            Value::List(vec![Value::string("moqui.basic.Enumeration")]),
        )],
    );
    assert!(!list(&wrap, "dropStatements").is_empty(), "{wrap:?}");
    assert!(
        list(&wrap, "restoreStatements")
            .iter()
            .any(|row| to_plain_string(&row["statementText"]).ends_with("NOT VALID")),
        "{wrap:?}"
    );
    assert!(!text(&wrap, "dropJobId").is_empty() && !text(&wrap, "restoreJobId").is_empty());

    let activity = call("get#DatabaseActivity", &[]);
    let sessions = list(&activity, "sessions");
    let own = sessions
        .iter()
        .find(|row| row["own_app"] == Value::Bool(true))
        .unwrap_or_else(|| panic!("{activity:?}"));
    // the app's own session is not cancelled without force
    let parameters: BTreeMap<String, Value> = [("pid".to_string(), own["pid"].clone())].into_iter().collect();
    let refused = moqui_core::service::call::sync_checked(
        &format!("{SERVICES}.cancel#DatabaseSession"),
        &parameters,
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    );
    assert!(refused.is_err_and(|err| err.to_string().contains("force")));
    moqui_core::context::ExecutionContext::with_current(|ec| ec.message().clear_errors());
    let health = call("get#IndexHealth", &[]);
    let Some(Value::Map(reasons)) = health.get("reasons") else {
        panic!("{health:?}")
    };
    assert!(reasons.is_empty(), "{reasons:?}");
}
