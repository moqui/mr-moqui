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

//! Database jobs on the embedded cluster (plans/Tools-Database.md §6, §15 step 3): a `CREATE INDEX CONCURRENTLY` job
//! runs on a service worker to a valid index; a second, held up by an open transaction, is cancelled through
//! `pg_cancel_backend` as from another instance, leaves the invalid index the cancel leaves, and run again ends with a
//! valid index.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::object::{to_plain_string, Value};

use crate::common;

const SERVICES: &str = "org.moqui.impl.DatabaseServices";

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

fn statement(sql: &str) -> Value {
    let mut map = IndexMap::new();
    map.insert("statementText".to_string(), Value::string(sql));
    Value::Map(map)
}

fn text(map: &BTreeMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

/// The job's status and its statements' statuses.
fn status(job_id: &str) -> (String, Vec<IndexMap<String, Value>>) {
    let out = call("get#DatabaseJobStatus", &[("jobId", Value::string(job_id))]);
    let Some(Value::Map(job)) = out.get("job") else {
        panic!("{out:?}")
    };
    let Some(Value::List(statements)) = out.get("statements") else {
        panic!("{out:?}")
    };
    let statements = statements
        .iter()
        .map(|row| match row {
            Value::Map(row) => row.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    (to_plain_string(&job["statusId"]), statements)
}

fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < limit, "{what} within {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Whether the index `name` of the current schema is valid; `None` when there is no such index.
fn index_valid(facade: &moqui_core::entity::EntityFacade, name: &str) -> Option<bool> {
    let out = call("get#IndexHealth", &[]);
    let invalid = match out.get("invalidIndexes") {
        Some(Value::List(rows)) => rows.iter().any(|row| match row {
            Value::Map(row) => to_plain_string(&row["index_name"]) == name,
            _ => false,
        }),
        _ => false,
    };
    let exists = moqui_core::entity::info::run_sql(
        facade,
        "transactional",
        &format!(
            "select indexrelid from pg_index x join pg_class i on i.oid = x.indexrelid where i.relname = '{name}'"
        ),
        0,
    )
    .map(|result| !result.records.is_empty())
    .unwrap();
    exists.then_some(!invalid)
}

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

    // A job runs on a service worker to a valid index.
    let first = call(
        "create#DatabaseJob",
        &[
            ("description", Value::string("first index")),
            (
                "statements",
                Value::List(vec![statement(
                    "CREATE INDEX CONCURRENTLY IF NOT EXISTS test_job_one ON enumeration (enum_code)",
                )]),
            ),
        ],
    );
    let first_id = text(&first, "jobId");
    wait_for("the first job", Duration::from_secs(30), || {
        status(&first_id).0 == "DbjsDone"
    });
    let (_, statements) = status(&first_id);
    assert_eq!(to_plain_string(&statements[0]["statusId"]), "DbjsDone");
    assert!(
        statements[0].get("backendPid").is_some_and(|pid| !pid.is_null()),
        "{statements:?}"
    );
    assert_eq!(index_valid(&facade, "test_job_one"), Some(true));

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
    let second = call(
        "create#DatabaseJob",
        &[
            ("description", Value::string("second index")),
            (
                "statements",
                Value::List(vec![statement(
                    "CREATE INDEX CONCURRENTLY IF NOT EXISTS test_job_two ON enumeration (description)",
                )]),
            ),
        ],
    );
    let second_id = text(&second, "jobId");
    wait_for("the second build to wait", Duration::from_secs(20), || {
        let (job, statements) = status(&second_id);
        job == "DbjsRunning"
            && statements[0].get("backendPid").is_some_and(|pid| !pid.is_null())
            && index_valid(&facade, "test_job_two") == Some(false)
    });
    // As from another instance: the job's row names one, so the cancel goes by the backend pid.
    {
        let _authz = context.disable_authz();
        let mut job = facade
            .find("moqui.database.DatabaseJob")
            .unwrap()
            .eq("jobId", Value::string(&second_id))
            .one()
            .unwrap()
            .unwrap();
        job.set("instanceId", Value::string("another-instance"));
        job.update(&facade).unwrap();
    }
    let cancelled = call("cancel#DatabaseJob", &[("jobId", Value::string(&second_id))]);
    assert_eq!(text(&cancelled, "how"), "backend", "{cancelled:?}");
    wait_for("the cancel", Duration::from_secs(20), || {
        status(&second_id).0 == "DbjsCancelled"
    });
    let (_, statements) = status(&second_id);
    assert_eq!(
        to_plain_string(&statements[0]["statusId"]),
        "DbjsCancelled",
        "{statements:?}"
    );
    assert_eq!(
        index_valid(&facade, "test_job_two"),
        Some(false),
        "the cancel leaves an invalid index"
    );

    // Run again: the invalid index is dropped and built again, valid.
    held_tx.send(()).unwrap();
    holder.join().unwrap();
    let rerun = call("run#DatabaseJob", &[("jobId", Value::string(&second_id))]);
    assert_eq!(text(&rerun, "statusId"), "DbjsDone", "{rerun:?}");
    assert_eq!(index_valid(&facade, "test_job_two"), Some(true));
}

fn list(map: &BTreeMap<String, Value>, key: &str) -> Vec<IndexMap<String, Value>> {
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

/// A dropped index is planned back in its online form, and the plan as a job puts it back; a failed statement skips
/// the one that depends on it and not the one that does not; the table, activity and index health services answer.
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
    let entity = (
        "entityNames",
        Value::List(vec![Value::string("moqui.basic.Enumeration")]),
    );

    let clean = call("compare#Schema", std::slice::from_ref(&entity));
    assert!(list(&clean, "tables").is_empty(), "{clean:?}");
    let index = moqui_core::entity::info::run_sql(
        &facade,
        "transactional",
        "SELECT indexname FROM pg_indexes WHERE tablename = 'enumeration' AND indexname NOT LIKE '%pkey' ORDER BY 1",
        0,
    )
    .unwrap();
    let dropped = to_plain_string(&index.records[0][0]);
    moqui_core::entity::info::run_sql(&facade, "transactional", &format!("DROP INDEX \"{dropped}\""), 0).unwrap();
    let plan = call("plan#SchemaChanges", std::slice::from_ref(&entity));
    let statements = list(&plan, "statements");
    assert_eq!(statements.len(), 1, "{plan:?}");
    let sql = to_plain_string(&statements[0]["statementText"]);
    assert!(sql.starts_with("CREATE INDEX CONCURRENTLY IF NOT EXISTS"), "{sql}");
    let job = call(
        "create#DatabaseJob",
        &[
            (
                "statements",
                Value::List(statements.into_iter().map(Value::Map).collect()),
            ),
            ("run", Value::Bool(false)),
        ],
    );
    let ran = call("run#DatabaseJob", &[("jobId", Value::string(text(&job, "jobId")))]);
    assert_eq!(text(&ran, "statusId"), "DbjsDone", "{ran:?}");
    assert!(list(&call("compare#Schema", std::slice::from_ref(&entity)), "tables").is_empty());

    // a failed statement: the one that depends on it is skipped, the other runs
    let with_seq = |seq: &str, sql: &str, depends: Option<&str>| {
        let mut map = IndexMap::new();
        map.insert("statementSeqId".to_string(), Value::string(seq));
        map.insert("statementText".to_string(), Value::string(sql));
        if let Some(depends) = depends {
            map.insert("dependsOnSeqId".to_string(), Value::string(depends));
        }
        Value::Map(map)
    };
    let job = call(
        "create#DatabaseJob",
        &[
            (
                "statements",
                Value::List(vec![
                    with_seq("001", "ALTER TABLE no_such_table ADD COLUMN x INT", None),
                    with_seq("002", "ANALYZE enumeration", Some("001")),
                    with_seq("003", "ANALYZE enumeration_type", None),
                ]),
            ),
            ("run", Value::Bool(false)),
        ],
    );
    let job_id = text(&job, "jobId");
    let ran = call("run#DatabaseJob", &[("jobId", Value::string(&job_id))]);
    assert_eq!(text(&ran, "statusId"), "DbjsFailed", "{ran:?}");
    let (_, statements) = status(&job_id);
    let statuses: Vec<String> = statements.iter().map(|row| to_plain_string(&row["statusId"])).collect();
    assert_eq!(statuses, ["DbjsFailed", "DbjsSkipped", "DbjsDone"]);

    let ddl = call("get#SchemaDdl", std::slice::from_ref(&entity));
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
    let activity = call("get#DatabaseActivity", &[]);
    let sessions = list(&activity, "sessions");
    assert!(
        sessions.iter().any(|row| row["own_app"] == Value::Bool(true)),
        "{activity:?}"
    );
    let health = call("get#IndexHealth", &[]);
    let Some(Value::Map(reasons)) = health.get("reasons") else {
        panic!("{health:?}")
    };
    assert!(reasons.is_empty(), "{reasons:?}");
}
