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

//! SQL runs (plans/Tools-Database.md §15 step 4): read only by default and refused by the database, the one-time token
//! of a run with changes, an audit row for every statement run, and cancel, here and from another instance.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::driver::DbDriver;
use moqui_core::entity::exec::SqlSyntax;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_drivers::embedded::EmbeddedCluster;
use moqui_lib::object::{to_plain_string, Value};

use super::{
    bind_parameters, cancel, classify, explain, no_transaction, parameter_names, run, transaction_control,
    CancelOutcome, ExplainRequest, RunRequest,
};
use crate::audit::Actor;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parameters_are_bound_as_values_outside_text_casts_slices_and_comments() {
    let mut params = BTreeMap::new();
    params.insert("id".to_string(), Value::string("A1"));
    params.insert("n".to_string(), Value::I64(3));
    let (sql, values) = bind_parameters(
        "SELECT x::text, ':id', \"a:id\", a[:n], a[1 : n] FROM t -- :n\n WHERE id = :id AND n > :n AND s[1:2] = 'x'",
        SqlSyntax::Postgres,
        &params,
        false,
    )
    .unwrap();
    assert_eq!(
        sql,
        "SELECT x::text, ':id', \"a:id\", a[:n], a[1 : n] FROM t -- :n\n WHERE id = ? AND n > ? AND s[1:2] = 'x'"
    );
    assert_eq!(values, vec![Value::string("A1"), Value::I64(3)]);
    assert!(bind_parameters("SELECT :missing", SqlSyntax::Postgres, &params, false).is_err());
    assert!(bind_parameters("SELECT :id, ?", SqlSyntax::Postgres, &params, false).is_err());
    assert_eq!(
        bind_parameters("SELECT '?' ", SqlSyntax::Postgres, &params, false)
            .unwrap()
            .0,
        "SELECT '?' "
    );
    assert_eq!(
        parameter_names("SELECT :a, :b; SELECT :a, @v := 1", SqlSyntax::Mysql),
        ["a", "b"]
    );
}

#[test]
fn statements_that_cannot_run_in_a_transaction_or_control_one_are_known_by_their_first_words() {
    for sql in [
        "VACUUM ANALYZE t",
        "create index concurrently i on t (a)",
        "DROP INDEX CONCURRENTLY IF EXISTS i",
        "REINDEX INDEX CONCURRENTLY i",
        "CREATE DATABASE other",
        "ALTER SYSTEM SET work_mem = '8MB'",
    ] {
        assert!(no_transaction(sql, SqlSyntax::Postgres), "{sql}");
    }
    for sql in ["CREATE INDEX i ON t (a)", "SELECT 'vacuum'", "-- VACUUM\nSELECT 1"] {
        assert!(!no_transaction(sql, SqlSyntax::Postgres), "{sql}");
    }
    for sql in [
        "COMMIT",
        "rollback to savepoint a",
        "BEGIN",
        "START TRANSACTION READ WRITE",
        "SAVEPOINT a",
        "RELEASE SAVEPOINT a",
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
        "END",
        "ABORT",
    ] {
        assert!(transaction_control(sql, SqlSyntax::Postgres), "{sql}");
    }
    for sql in [
        "LOCK TABLES t WRITE",
        "UNLOCK TABLES",
        "SET autocommit = 0",
        "SET SESSION autocommit = 1",
    ] {
        assert!(transaction_control(sql, SqlSyntax::Mysql), "{sql}");
    }
    for sql in ["SELECT 'commit'", "SET search_path = x", "LOCK TABLE t IN SHARE MODE"] {
        assert!(!transaction_control(sql, SqlSyntax::Postgres), "{sql}");
    }
}

fn facade_on(driver: Arc<dyn DbDriver>) -> EntityFacade {
    let facade = moqui_drivers::entity_facade::open_entity_facade(
        &moqui_core::context::framework_dir(),
        driver,
        PoolConfig {
            min: 0,
            max: 6,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    facade
        .check_tables(&["moqui.database.DatabaseAudit", "moqui.database.SqlRunToken", "moqui.entity.SequenceValueItem"])
        .unwrap();
    facade
}

fn request(run_id: &str, sql: &str) -> RunRequest {
    RunRequest {
        run_id: run_id.to_string(),
        group: "transactional".to_string(),
        sql_text: sql.to_string(),
        ..RunRequest::default()
    }
}

fn changes(run_id: &str, sql: &str, token: String) -> RunRequest {
    RunRequest {
        allow_changes: true,
        token,
        ..request(run_id, sql)
    }
}

fn actor(user: &str) -> Actor {
    Actor {
        user_id: Some(user.to_string()),
        instance_id: Some("this-instance".to_string()),
        ..Actor::default()
    }
}

fn audit_rows(facade: &EntityFacade, run_id: &str) -> Vec<moqui_core::entity::EntityValue> {
    facade
        .find(crate::audit::DATABASE_AUDIT)
        .unwrap()
        .eq("runId", Value::string(run_id))
        .order_by("statementSeq", false)
        .list()
        .unwrap()
        .iter()
        .cloned()
        .collect()
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn count(facade: &EntityFacade, sql: &str) -> i64 {
    let ran = run(facade, &request("", sql), &actor("ANA"), false).unwrap();
    match &ran.results[0].records[0][0] {
        Value::I64(count) => *count,
        other => to_plain_string(other).parse().unwrap(),
    }
}

/// Wait for the running audit row of `run_id`'s statement `seq`.
fn wait_running(facade: &EntityFacade, run_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while audit_rows(facade, run_id).is_empty() {
        assert!(Instant::now() < deadline, "{run_id} did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
}

/// The exit tests of §15 step 4 and the review's, on one embedded cluster.
#[test]
fn sql_runs_are_read_only_by_default_confirmed_audited_and_cancelled() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = PathBuf::from("/tmp").join(format!("moqui-pg-sql-{}-{nanos}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 30).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = facade_on(Arc::clone(&driver));
    driver
        .connect()
        .unwrap()
        .execute("CREATE TABLE sql_note (id INT PRIMARY KEY, body TEXT)", &[])
        .unwrap();
    let ana = actor("ANA");
    let bo = actor("BO");
    let token = |sql: &str, who: &Actor| {
        classify(&facade, "transactional", sql, who.user_id.as_deref(), false)
            .unwrap()
            .token
    };

    // a write in the default read-only mode is refused by the database, and audited with its error
    let refused = run(
        &facade,
        &request("readonly-run-1", "INSERT INTO sql_note VALUES (1, 'one')"),
        &ana,
        false,
    )
    .unwrap();
    let error = refused.results[0].error.clone().unwrap_or_default();
    assert!(error.contains("read-only"), "{error}");
    let rows = audit_rows(&facade, "readonly-run-1");
    assert_eq!(rows.len(), 1);
    assert!(field(&rows[0], "errorText").contains("read-only"));
    assert_eq!(field(&rows[0], "isChange"), "N");
    assert_eq!(field(&rows[0], "userId"), "ANA");

    // with changes allowed, the run needs the token of the classification, for the same user, once
    let script = "INSERT INTO sql_note VALUES (:id, :body); SELECT body FROM sql_note WHERE id = :id";
    let mut with_params = changes("changes-run-1", script, String::new());
    with_params.params.insert("id".to_string(), Value::I64(2));
    with_params
        .params
        .insert("body".to_string(), Value::string("two'); DROP TABLE sql_note; --"));
    assert!(run(&facade, &with_params, &ana, false).is_err(), "no token");
    let refusals = audit_rows(&facade, "changes-run-1");
    assert_eq!(refusals.len(), 1, "the refused attempt is audited");
    assert!(field(&refusals[0], "errorText").contains("confirmation"));
    let classified = classify(&facade, "transactional", script, Some("ANA"), false).unwrap();
    assert_eq!(classified.change_count, 1);
    assert!(classified.statements[0].changes && !classified.statements[1].changes);
    assert_eq!(classified.parameter_names, ["id", "body"]);
    with_params.token = classified.token.clone();
    with_params.run_id = "changes-run-2".to_string();
    assert!(run(&facade, &with_params, &bo, false).is_err(), "another user's token");
    with_params.token = token(script, &ana);
    let ran = run(&facade, &with_params, &ana, false).unwrap();
    assert!(
        ran.results.iter().all(|result| result.error.is_none()),
        "{:?}",
        ran.results
    );
    assert_eq!(ran.results[0].rows_affected, Some(1));
    assert_eq!(
        ran.results[1].records,
        vec![vec![Value::string("two'); DROP TABLE sql_note; --")]],
        "the parameter was bound as a value"
    );
    with_params.run_id = "changes-run-3".to_string();
    assert!(run(&facade, &with_params, &ana, false).is_err(), "the token is spent");

    // a run id used again replaces no audit row
    let again = run(&facade, &request("readonly-run-1", "SELECT 1"), &bo, false).unwrap();
    assert!(again.results[0].error.is_none());
    let rows = audit_rows(&facade, "readonly-run-1");
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .any(|row| field(row, "userId") == "ANA" && field(row, "errorText").contains("read-only")));

    // transaction control is refused
    assert!(classify(
        &facade,
        "transactional",
        "INSERT INTO sql_note VALUES (9, 'x'); COMMIT",
        Some("ANA"),
        false
    )
    .is_err());
    assert!(run(&facade, &request("control-run-1", "SELECT 1; ROLLBACK"), &ana, false).is_err());

    // on production the database name must be typed
    let delete = "DELETE FROM sql_note WHERE id = 99";
    let classified = classify(&facade, "transactional", delete, Some("ANA"), true).unwrap();
    assert_eq!(classified.confirm_database_name.as_deref(), Some("moqui"));
    assert!(run(&facade, &changes("prod-run-1", delete, classified.token), &ana, true).is_err());
    let mut named = changes("prod-run-2", delete, token(delete, &ana));
    named.confirm_database_name = "moqui".to_string();
    assert!(run(&facade, &named, &ana, true).is_ok());

    // one transaction: a failure rolls the script back; each statement run has its audit row, marked rolled back
    let script = "INSERT INTO sql_note VALUES (3, 'three'); INSERT INTO sql_note VALUES (2, 'duplicate'); SELECT 1";
    let ran = run(
        &facade,
        &changes("atomic-run-1", script, token(script, &ana)),
        &ana,
        false,
    )
    .unwrap();
    assert!(ran.rolled_back);
    assert!(ran.results[0].rolled_back && ran.results[1].error.is_some() && ran.results[2].skipped);
    let rows = audit_rows(&facade, "atomic-run-1");
    assert_eq!(rows.len(), 2);
    assert!(field(&rows[0], "detailJson").contains("\"rolledBack\":true"));
    assert!(!field(&rows[1], "errorText").is_empty());
    assert_eq!(
        count(&facade, "SELECT count(*) FROM sql_note WHERE id = 3"),
        0,
        "the first insert was rolled back"
    );

    // the session a run with changes used is closed after it: its settings reach no later borrower
    let set = "SET application_name = 'sql-run-leak'";
    let ran = run(&facade, &changes("session-run-1", set, token(set, &ana)), &ana, false).unwrap();
    assert!(ran.results[0].error.is_none(), "{:?}", ran.results);
    let deadline = Instant::now() + Duration::from_secs(5);
    while count(
        &facade,
        "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'sql-run-leak'",
    ) > 0
    {
        assert!(
            Instant::now() < deadline,
            "the session of the run went back to the pool"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // explain plans without running; Explain Analyze of a write needs the token and is rolled back
    let plan = explain(
        &facade,
        &ExplainRequest {
            group: "transactional".to_string(),
            sql_text: "SELECT * FROM sql_note".to_string(),
            ..ExplainRequest::default()
        },
        &ana,
        false,
    )
    .unwrap();
    assert_eq!(plan.format, "json");
    assert!(plan.plan.trim_start().starts_with('['), "{}", plan.plan);
    let write = "INSERT INTO sql_note VALUES (50, 'analyzed')";
    let mut analyze = ExplainRequest {
        group: "transactional".to_string(),
        sql_text: write.to_string(),
        analyze: true,
        ..ExplainRequest::default()
    };
    assert!(explain(&facade, &analyze, &ana, false).is_err());
    analyze.token = token(write, &ana);
    assert!(
        explain(&facade, &analyze, &ana, true).is_err(),
        "production needs the name too"
    );
    analyze.token = token(write, &ana);
    let analyzed = explain(&facade, &analyze, &ana, false).unwrap();
    assert!(analyzed.analyzed && analyzed.rolled_back);
    assert!(analyzed.plan.contains("Actual"), "{}", analyzed.plan);
    assert_eq!(
        count(&facade, "SELECT count(*) FROM sql_note WHERE id = 50"),
        0,
        "the analyzed insert was rolled back"
    );

    // cancel stops a pg_sleep(30); another user may not cancel it
    let started = Instant::now();
    let sleeper = {
        let facade = facade.clone();
        let ana = ana.clone();
        std::thread::spawn(move || run(&facade, &request("sleep-run-1", "SELECT pg_sleep(30)"), &ana, false))
    };
    wait_running(&facade, "sleep-run-1");
    assert!(cancel(&facade, "sleep-run-1", Some("BO"), false, Some("this-instance")).is_err());
    assert_eq!(
        cancel(&facade, "sleep-run-1", Some("ANA"), false, Some("this-instance")),
        Ok(CancelOutcome::Cancelled)
    );
    let slept = sleeper.join().unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
    assert!(
        slept.cancelled && slept.results[0].error.is_some(),
        "{:?}",
        slept.results
    );
    assert_ne!(field(&audit_rows(&facade, "sleep-run-1")[0], "durationMs"), "");

    // a cancel with statements left rolls the whole transaction back
    let script =
        "INSERT INTO sql_note VALUES (60, 'sixty'); SELECT pg_sleep(30); INSERT INTO sql_note VALUES (61, 'x')";
    let sleeper = {
        let facade = facade.clone();
        let ana = ana.clone();
        let request = changes("sleep-run-2", script, token(script, &ana));
        std::thread::spawn(move || run(&facade, &request, &ana, false))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while audit_rows(&facade, "sleep-run-2").len() < 2 {
        assert!(Instant::now() < deadline, "the sleep did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        cancel(&facade, "sleep-run-2", Some("ANA"), false, Some("this-instance")),
        Ok(CancelOutcome::Cancelled)
    );
    let slept = sleeper.join().unwrap().unwrap();
    assert!(slept.rolled_back && slept.cancelled, "{slept:?}");
    assert!(slept.results[0].rolled_back && slept.results[2].skipped);
    assert_eq!(count(&facade, "SELECT count(*) FROM sql_note WHERE id = 60"), 0);

    // from another instance: the running audit row names the session, which is cancelled while it is the same one
    let remote = {
        let driver = Arc::clone(&driver);
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = driver
                .connect()
                .unwrap()
                .query("SELECT pg_sleep(30), 'remote-sleep'", &[]);
            (result.is_err(), started.elapsed())
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let (pid, backend_start) = loop {
        let ran = run(
            &facade,
            &request(
                "",
                "SELECT pid, CAST(backend_start AS TEXT) FROM pg_stat_activity WHERE query LIKE '%''remote-sleep''%' \
                 AND pid <> pg_backend_pid()",
            ),
            &ana,
            false,
        )
        .unwrap();
        if let Some(record) = ran.results[0].records.first() {
            break (
                to_plain_string(&record[0]).parse::<i64>().unwrap(),
                to_plain_string(&record[1]),
            );
        }
        assert!(Instant::now() < deadline, "the remote sleep did not start");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mark = |run_id: &str, start: &str| {
        crate::audit::mark_running(
            &facade,
            &crate::audit::AuditEntry {
                audit_id: Some(crate::audit::new_id(&facade).unwrap()),
                action: "DbaSql",
                db: Some("group:transactional".to_string()),
                run_id: Some(run_id.to_string()),
                statement_seq: Some(1),
                detail_json: Some(serde_json::json!({"backendId": pid, "backendStart": start}).to_string()),
                ..crate::audit::AuditEntry::default()
            },
            &Actor {
                instance_id: Some("other-instance".to_string()),
                ..actor("ANA")
            },
        );
    };
    mark("remote-stale-1", "2000-01-01 00:00:00+00");
    assert_eq!(
        cancel(&facade, "remote-stale-1", Some("ANA"), false, Some("this-instance")),
        Ok(CancelOutcome::NotRunning),
        "a row whose session is not the one now is not cancelled"
    );
    mark("remote-run-1", &backend_start);
    assert!(cancel(&facade, "remote-run-1", Some("BO"), false, Some("this-instance")).is_err());
    assert_eq!(
        cancel(&facade, "remote-run-1", Some("ANA"), false, Some("this-instance")),
        Ok(CancelOutcome::CancelledRemote {
            instance_id: "other-instance".to_string(),
            backend_id: pid
        })
    );
    let (failed, took) = remote.join().unwrap();
    assert!(failed && took < Duration::from_secs(15), "{took:?}");

    drop(facade);
    drop(driver);
    drop(cluster);
}
