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
//! of a run with changes, an audit row for every statement run, and cancel.

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

use super::{bind_parameters, classify, no_transaction, parameter_names, run, RunRequest};

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn parameters_are_bound_as_values_outside_text_casts_and_comments() {
    let mut params = BTreeMap::new();
    params.insert("id".to_string(), Value::string("A1"));
    params.insert("n".to_string(), Value::I64(3));
    let (sql, values) = bind_parameters(
        "SELECT x::text, ':id', \"a:id\" FROM t -- :n\n WHERE id = :id AND n > :n AND s[1:2] = 'x'",
        SqlSyntax::Postgres,
        &params,
        false,
    )
    .unwrap();
    assert_eq!(
        sql,
        "SELECT x::text, ':id', \"a:id\" FROM t -- :n\n WHERE id = ? AND n > ? AND s[1:2] = 'x'"
    );
    assert_eq!(values, vec![Value::string("A1"), Value::I64(3)]);
    assert!(bind_parameters("SELECT :missing", SqlSyntax::Postgres, &params, false).is_err());
    assert!(bind_parameters("SELECT :id, ?", SqlSyntax::Postgres, &params, false).is_err());
    // no parameters: the text as it is, a ? of its own kept
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
fn statements_that_cannot_run_in_a_transaction_are_known_by_their_first_words() {
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
    facade.check_tables(&["moqui.database.DatabaseAudit"]).unwrap();
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

fn audit_row(facade: &EntityFacade, audit_id: &str) -> Option<moqui_core::entity::EntityValue> {
    facade
        .find(crate::audit::DATABASE_AUDIT)
        .unwrap()
        .eq("auditId", Value::string(audit_id))
        .one()
        .unwrap()
}

/// The exit tests of §15 step 4, on one embedded cluster.
#[test]
fn sql_runs_are_read_only_by_default_confirmed_audited_and_cancelled() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = PathBuf::from("/tmp").join(format!("moqui-pg-sql-{}-{nanos}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = facade_on(Arc::clone(&driver));
    driver
        .connect()
        .unwrap()
        .execute("CREATE TABLE sql_note (id INT PRIMARY KEY, body TEXT)", &[])
        .unwrap();

    // a write in the default read-only mode is refused by the database, and audited with its error
    let refused = run(
        &facade,
        &request("readonly-run-1", "INSERT INTO sql_note VALUES (1, 'one')"),
        false,
    )
    .unwrap();
    let error = refused.results[0].error.clone().unwrap_or_default();
    assert!(error.contains("read-only"), "{error}");
    let row = audit_row(&facade, "readonly-run-1_1").expect("the failed statement's audit row");
    assert!(row
        .get("errorText")
        .map(to_plain_string)
        .is_some_and(|text| text.contains("read-only")));
    assert_eq!(row.get("isChange").map(to_plain_string).as_deref(), Some("N"));

    // with changes allowed, the run needs the token of the classification, once
    let script = "INSERT INTO sql_note VALUES (:id, :body); SELECT body FROM sql_note WHERE id = :id";
    let mut changes = request("changes-run-1", script);
    changes.allow_changes = true;
    changes.params.insert("id".to_string(), Value::I64(2));
    changes
        .params
        .insert("body".to_string(), Value::string("two'); DROP TABLE sql_note; --"));
    assert!(run(&facade, &changes, false).is_err(), "no token");
    let classified = classify(&facade, "transactional", script, None, false).unwrap();
    assert_eq!(classified.change_count, 1);
    assert!(classified.statements[0].changes && !classified.statements[1].changes);
    assert_eq!(classified.parameter_names, ["id", "body"]);
    changes.token = classified.token.clone();
    let ran = run(&facade, &changes, false).unwrap();
    assert!(
        ran.results.iter().all(|result| result.error.is_none()),
        "{:?}",
        ran.results
    );
    assert_eq!(ran.results[0].rows_affected, Some(1));
    // the parameter was bound as a value
    assert_eq!(
        ran.results[1].records,
        vec![vec![Value::string("two'); DROP TABLE sql_note; --")]]
    );
    changes.run_id = "changes-run-2".to_string();
    assert!(run(&facade, &changes, false).is_err(), "the token is spent");
    // on production the database name must be typed
    let mut production = request("changes-run-3", "DELETE FROM sql_note WHERE id = 99");
    production.allow_changes = true;
    let classified = classify(&facade, "transactional", &production.sql_text, None, true).unwrap();
    assert_eq!(classified.confirm_database_name.as_deref(), Some("moqui"));
    production.token = classified.token.clone();
    assert!(run(&facade, &production, true).is_err(), "no database name");
    let classified = classify(&facade, "transactional", &production.sql_text, None, true).unwrap();
    production.token = classified.token;
    production.confirm_database_name = "moqui".to_string();
    assert!(run(&facade, &production, true).is_ok());

    // one transaction: a failure rolls the script back, and each statement run has its audit row
    let script = "INSERT INTO sql_note VALUES (3, 'three'); INSERT INTO sql_note VALUES (2, 'duplicate'); SELECT 1";
    let mut atomic = request("atomic-run-1", script);
    atomic.allow_changes = true;
    atomic.token = classify(&facade, "transactional", script, None, false).unwrap().token;
    let ran = run(&facade, &atomic, false).unwrap();
    assert!(ran.rolled_back);
    assert!(ran.results[1].error.is_some() && ran.results[2].skipped);
    assert!(audit_row(&facade, "atomic-run-1_1").is_some());
    assert!(audit_row(&facade, "atomic-run-1_2")
        .and_then(|row| row.get("errorText").map(to_plain_string))
        .is_some_and(|text| !text.is_empty()));
    let kept = run(
        &facade,
        &request("readonly-run-2", "SELECT count(*) FROM sql_note WHERE id = 3"),
        false,
    )
    .unwrap();
    assert_eq!(
        kept.results[0].records[0][0],
        Value::I64(0),
        "the first insert was rolled back"
    );

    // explain plans without running; Explain Analyze of a write needs the token and is rolled back
    let no_params = BTreeMap::new();
    let plan = super::explain(
        &facade,
        "transactional",
        "SELECT * FROM sql_note",
        &no_params,
        false,
        "",
        false,
    )
    .unwrap();
    assert_eq!(plan.format, "json");
    assert!(plan.plan.trim_start().starts_with('['), "{}", plan.plan);
    let write = "INSERT INTO sql_note VALUES (50, 'analyzed')";
    assert!(super::explain(&facade, "transactional", write, &no_params, true, "", false).is_err());
    let token = classify(&facade, "transactional", write, None, false).unwrap().token;
    let analyzed = super::explain(&facade, "transactional", write, &no_params, true, &token, false).unwrap();
    assert!(analyzed.analyzed && analyzed.rolled_back);
    assert!(analyzed.plan.contains("Actual"), "{}", analyzed.plan);
    let after = run(
        &facade,
        &request("readonly-run-3", "SELECT count(*) FROM sql_note WHERE id = 50"),
        false,
    )
    .unwrap();
    assert_eq!(
        after.results[0].records[0][0],
        Value::I64(0),
        "the analyzed insert was rolled back"
    );

    // cancel stops a pg_sleep(30)
    let started = Instant::now();
    let sleeper = {
        let facade = facade.clone();
        std::thread::spawn(move || run(&facade, &request("sleep-run-1", "SELECT pg_sleep(30)"), false))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let cancelled = loop {
        // the statement's running mark is written once it has started
        if audit_row(&facade, "sleep-run-1_1").is_some() {
            std::thread::sleep(Duration::from_millis(200));
            break super::cancel(&facade, "sleep-run-1", None, false, None) == Ok(super::CancelOutcome::Cancelled);
        }
        if Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(cancelled, "the run was not found to cancel");
    let slept = sleeper.join().unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
    assert!(slept.cancelled);
    assert!(slept.results[0].error.is_some(), "{:?}", slept.results);
    assert!(audit_row(&facade, "sleep-run-1_1")
        .and_then(|row| row.get("durationMs").cloned())
        .is_some_and(|duration| !matches!(duration, Value::Null)));
    drop(facade);
    drop(driver);
    drop(cluster);
}
