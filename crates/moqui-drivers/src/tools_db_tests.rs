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

//! The entity layer's pieces of the Tools app's database sections on an embedded cluster (plans/Tools-Database.md):
//! the schema comparison's catalog queries and names (5.1), the SQL runner's read-only transaction and cancel (8.2,
//! 8.3), result column origins (9.5) and the sessions' application name (7.3).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::driver::{ColumnOrigin, DbCancel, DbDriver};
use moqui_core::entity::info::{self, SqlRunOptions, SqlStart};
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_lib::object::Value;

use super::super::embedded::EmbeddedCluster;
use super::open_entity_facade;
use super::tests::{temp_cluster_dir, DirGuard};

fn facade_on(driver: Arc<dyn DbDriver>) -> EntityFacade {
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    open_entity_facade(
        &framework,
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap()
}

fn run(facade: &EntityFacade, sql: &str) -> info::SqlResult {
    info::run_sql(facade, "transactional", sql, 0).unwrap_or_else(|err| panic!("{sql}: {err}"))
}

fn names(diff: &[impl AsRef<str>]) -> Vec<String> {
    diff.iter().map(|name| name.as_ref().to_ascii_lowercase()).collect()
}

/// The tables the check made compare clean through the real catalog queries; then a dropped index, an extra index and
/// a dropped foreign key are each reported, and the type of a column the check did not make.
#[test]
fn the_schema_comparison_reads_the_catalog_as_the_check_made_it() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let facade = facade_on(Arc::new(driver));
    let entities = [
        "moqui.basic.EnumerationType",
        "moqui.basic.StatusFlow",
        "moqui.basic.Enumeration",
        "moqui.test.TestEntity",
        "moqui.test.TestFts",
        "moqui.test.FooBar",
    ];
    facade.check_tables(&entities).unwrap();
    let clean = facade.meta().compare(&entities).unwrap();
    assert!(clean.errors.is_empty(), "{:?}", clean.errors);
    // The view counts its two members.
    assert_eq!(clean.checked, 7, "{clean:#?}");
    assert!(clean.tables.is_empty(), "{clean:#?}");

    let index = run(
        &facade,
        "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema() AND tablename = 'enumeration' \
         AND indexname NOT LIKE '%pkey' ORDER BY indexname",
    );
    let dropped = index.records[0][0].as_str().unwrap().to_string();
    run(&facade, &format!("DROP INDEX \"{dropped}\""));
    run(&facade, "CREATE INDEX stray_idx ON enumeration (enum_code)");
    let key = run(
        &facade,
        "SELECT c.conname FROM pg_constraint c JOIN pg_class r ON r.oid = c.confrelid \
         WHERE c.contype = 'f' AND r.relname = 'status_flow'",
    );
    let key = key.records[0][0].as_str().unwrap().to_string();
    run(&facade, &format!("ALTER TABLE enumeration DROP CONSTRAINT \"{key}\""));
    run(
        &facade,
        "ALTER TABLE test_entity ALTER COLUMN test_medium TYPE varchar(20)",
    );

    let diff = facade.meta().compare(&entities).unwrap();
    assert!(diff.errors.is_empty(), "{:?}", diff.errors);
    let enumeration = diff
        .tables
        .iter()
        .find(|table| table.entity_name == "moqui.basic.Enumeration")
        .unwrap_or_else(|| panic!("{diff:#?}"));
    let missing: Vec<&str> = enumeration
        .missing_indexes
        .iter()
        .map(|index| index.name.as_str())
        .collect();
    assert_eq!(names(&missing), [dropped], "{enumeration:#?}");
    let extra: Vec<&str> = enumeration
        .extra_indexes
        .iter()
        .map(|index| index.name.as_str())
        .collect();
    assert_eq!(extra, ["stray_idx"], "{enumeration:#?}");
    let keys: Vec<&str> = enumeration
        .missing_foreign_keys
        .iter()
        .map(|key| key.related_entity.as_str())
        .collect();
    assert_eq!(keys, ["moqui.basic.StatusFlow"], "{enumeration:#?}");
    // The plan's statements run and put it back.
    for statement in enumeration.missing_indexes.iter().flat_map(|index| &index.ddl) {
        run(&facade, statement);
    }
    for key in &enumeration.missing_foreign_keys {
        run(&facade, key.ddl.as_deref().unwrap());
    }
    let test_entity = diff
        .tables
        .iter()
        .find(|table| table.entity_name == "moqui.test.TestEntity")
        .unwrap_or_else(|| panic!("{diff:#?}"));
    assert_eq!(test_entity.column_types.len(), 1, "{test_entity:#?}");
    assert_eq!(test_entity.column_types[0].actual, "character varying(20)");

    run(&facade, "DROP INDEX stray_idx");
    let again = facade.meta().compare(&["moqui.basic.Enumeration"]).unwrap();
    assert!(again.tables.is_empty(), "{again:#?}");
}

/// Every framework table the check makes compares clean: no type, index or key the comparison reads differently from
/// how the DDL builders make it.
#[test]
fn the_whole_framework_schema_compares_clean() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let facade = facade_on(Arc::new(driver));
    facade.check_all_tables().unwrap();
    let started = Instant::now();
    let diff = facade.meta().compare(&[]).unwrap();
    eprintln!("compared {} tables in {:?}", diff.checked, started.elapsed());
    assert!(diff.errors.is_empty(), "{:?}", diff.errors);
    assert!(diff.checked > 100, "{}", diff.checked);
    assert!(diff.tables.is_empty(), "{:#?}", diff.tables);
}

/// A statement's canceller and backend id, as `on_start` hands them out.
type Started = (Box<dyn DbCancel>, Option<i64>);

/// A read-only run refuses a write, a data-modifying CTE among them, in the database; a cancel handed out when the
/// statement starts stops a `pg_sleep(30)`; a result's columns name the table they come from; the sessions carry the
/// process's application name, and a pooled one takes a new name on its next checkout.
#[test]
fn read_only_runs_refuse_writes_and_a_cancel_stops_a_sleep() {
    moqui_core::driver::set_application_name("moqui-tools-test-a");
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let facade = facade_on(Arc::new(driver));
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();

    let read_only = |sql: &str| {
        info::run_sql_with(
            &facade,
            "transactional",
            sql,
            SqlRunOptions {
                read_only: true,
                ..SqlRunOptions::default()
            },
        )
    };
    for sql in [
        "insert into test_entity (test_id) values ('RO')",
        "with made as (insert into test_entity (test_id) values ('RO') returning test_id) select * from made",
        "create table ro_made (id int)",
    ] {
        let err = read_only(sql).unwrap_err();
        assert!(err.to_string().contains("read-only transaction"), "{sql}: {err}");
    }
    run(
        &facade,
        "insert into test_entity (test_id, test_medium) values ('W1', 'written')",
    );
    let rows = read_only("select test_id, test_medium, 1 as one from test_entity").unwrap();
    assert_eq!(rows.records.len(), 1);
    assert!(
        matches!(rows.column_origins[0], Some(ColumnOrigin::Postgres { column: 1, .. })),
        "{:?}",
        rows.column_origins
    );
    assert_eq!(rows.column_origins[2], None);
    let sources = info::resolve_column_origins(&facade, "transactional", &rows.column_origins).unwrap();
    let source = sources[1].as_ref().unwrap();
    assert_eq!(
        (source.table.as_str(), source.column.as_str()),
        ("test_entity", "test_medium")
    );
    assert!(source.is_table);
    assert_eq!(sources[2], None);
    // A view's columns name the view.
    run(
        &facade,
        "create view test_entity_view as select test_id from test_entity",
    );
    let view = read_only("select test_id from test_entity_view").unwrap();
    let sources = info::resolve_column_origins(&facade, "transactional", &view.column_origins).unwrap();
    let source = sources[0].as_ref().unwrap();
    assert_eq!(source.table, "test_entity_view");
    assert!(!source.is_table);

    // Cancel: the canceller is handed out as the statement starts.
    let slot: Arc<Mutex<Option<Started>>> = Arc::new(Mutex::new(None));
    let cancelling = {
        let slot = Arc::clone(&slot);
        std::thread::spawn(move || {
            let started = Instant::now();
            loop {
                if let Some((canceller, backend)) = slot.lock().unwrap().take() {
                    std::thread::sleep(Duration::from_millis(200));
                    canceller.cancel().unwrap();
                    return backend;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "the statement never started"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    let mut on_start = |start: SqlStart| *slot.lock().unwrap() = Some((start.canceller, start.backend_id));
    let started = Instant::now();
    let err = info::run_sql_with(
        &facade,
        "transactional",
        "select pg_sleep(30)",
        SqlRunOptions {
            read_only: true,
            on_start: Some(&mut on_start),
            ..SqlRunOptions::default()
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("canceling statement"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    let backend = cancelling.join().unwrap();
    assert!(backend.is_some_and(|pid| pid > 0), "{backend:?}");

    // The application name: given at connect, and changed on a pooled session's next checkout.
    let name = |facade: &EntityFacade| {
        run(
            facade,
            "select application_name from pg_stat_activity where pid = pg_backend_pid()",
        )
        .records[0][0]
            .clone()
    };
    assert_eq!(name(&facade), Value::string("moqui-tools-test-a"));
    moqui_core::driver::set_application_name("moqui-tools-test-b");
    assert_eq!(name(&facade), Value::string("moqui-tools-test-b"));
}
