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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::{DbConnection as _, DbDriver};
use moqui_core::transaction::PoolConfig;
use moqui_lib::object::Value;

use super::super::embedded::EmbeddedCluster;
use super::open_entity_facade;

pub(super) struct DirGuard(pub(super) PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn empty_database_check_create_view_matches_and_stream() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
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
    .unwrap();

    for name in [
        "moqui.test.TestEntity",
        "moqui.test.Foo",
        "moqui.test.Bar",
        "moqui.test.TestFts",
    ] {
        assert!(!facade.table_exists(name).unwrap(), "{name} already exists");
    }
    facade
        .check_tables(&[
            "moqui.test.TestEntity",
            "moqui.test.Foo",
            "moqui.test.Bar",
            "moqui.test.TestFts",
            "moqui.test.FooBar",
        ])
        .unwrap();
    for name in [
        "moqui.test.TestEntity",
        "moqui.test.Foo",
        "moqui.test.Bar",
        "moqui.test.TestFts",
    ] {
        assert!(facade.table_exists(name).unwrap(), "{name} was not created");
    }
    // As Java's `tableExists`, a view is there when one of its member tables is.
    assert!(facade.table_exists("moqui.test.FooBar").unwrap());

    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("T1"));
    row.set("testMedium", Value::string("hello"));
    row.create(&facade).unwrap();
    let found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("T1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(found.get("testMedium").and_then(Value::as_str), Some("hello"));

    let mut foo = facade.make_value("moqui.test.Foo").unwrap();
    foo.set("fooId", Value::string("F1"));
    foo.set("fooText", Value::string("alpha"));
    foo.create(&facade).unwrap();
    let mut bar = facade.make_value("moqui.test.Bar").unwrap();
    bar.set("barId", Value::string("B1"));
    bar.set("fooId", Value::string("F1"));
    bar.set("barRank", Value::I64(3));
    bar.create(&facade).unwrap();
    let view = facade.find("moqui.test.FooBar").unwrap().list().unwrap();
    let joined = view.get(0).unwrap();
    assert_eq!(joined.get("fooText").and_then(Value::as_str), Some("alpha"));
    assert_eq!(joined.get("barRank"), Some(&Value::I64(3)));

    let mut fts = facade.make_value("moqui.test.TestFts").unwrap();
    fts.set("testFtsId", Value::string("FTS1"));
    fts.set("body", Value::string("the quick brown fox jumps over the lazy dog"));
    fts.create(&facade).unwrap();
    let matched = facade
        .find("moqui.test.TestFts")
        .unwrap()
        .matches("body", Value::string("quick brown"))
        .list()
        .unwrap();
    assert_eq!(matched.len(), 1);
    assert_eq!(
        matched.get(0).unwrap().get("testFtsId").and_then(Value::as_str),
        Some("FTS1")
    );
    // A table set up before the generated column has the old expression index. A check adds the column and its
    // index and drops the old one.
    let fts_indexes = |facade: &moqui_core::entity::EntityFacade| -> Vec<String> {
        let rows = facade
            .transactions_for("moqui.test.TestFts")
            .unwrap()
            .with_connection(|conn| {
                conn.query(
                    "SELECT indexname FROM pg_indexes WHERE tablename = 'test_fts' ORDER BY indexname",
                    &[],
                )
            })
            .unwrap();
        rows.iter()
            .filter_map(|row| {
                row.columns
                    .first()
                    .and_then(|(_, value)| value.as_str().map(str::to_string))
            })
            .filter(|name| name.contains("fts_"))
            .collect()
    };
    assert_eq!(fts_indexes(&facade), ["fts_test_fts_body"]);
    facade
        .transactions_for("moqui.test.TestFts")
        .unwrap()
        .with_connection(|conn| {
            conn.execute("ALTER TABLE test_fts DROP COLUMN body_tsv", &[])?;
            conn.execute(
                "CREATE INDEX test_fts_body_fts ON test_fts USING gin (to_tsvector('english', body))",
                &[],
            )
        })
        .unwrap();
    assert_eq!(fts_indexes(&facade), ["test_fts_body_fts"]);
    facade.forget_tables();
    facade.check_tables(&["moqui.test.TestFts"]).unwrap();
    assert_eq!(fts_indexes(&facade), ["fts_test_fts_body"]);
    let rematched = facade
        .find("moqui.test.TestFts")
        .unwrap()
        .matches("body", Value::string("lazy dog"))
        .count()
        .unwrap();
    assert_eq!(rematched, 1);

    facade.begin("moqui.test.TestEntity", None).unwrap();
    for index in 1..=60 {
        let mut streamed = facade.make_value("moqui.test.TestEntity").unwrap();
        streamed.set("testId", Value::string(format!("s-{index:03}")));
        streamed.set("testMedium", Value::string("stream"));
        streamed.create(&facade).unwrap();
    }
    let mut iter = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testMedium", Value::string("stream"))
        .iterator()
        .unwrap();
    let mut seen = 0_i64;
    while iter.next_value().unwrap().is_some() {
        seen += 1;
    }
    assert_eq!(seen, 60);
    drop(iter);
    let counted = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testMedium", Value::string("stream"))
        .count()
        .unwrap();
    assert_eq!(counted, 60);
    facade.commit("moqui.test.TestEntity").unwrap();

    facade.begin("moqui.test.TestEntity", None).unwrap();
    let committed = stream_ids(&facade);
    assert_eq!(committed.len(), 60, "{committed:?}");
    assert!(committed.iter().any(|id| id == "s-001"));
    let mut doomed = facade.make_value("moqui.test.TestEntity").unwrap();
    doomed.set("testId", Value::string("s-001"));
    doomed.delete(&facade).unwrap();
    let held = stream_ids(&facade);
    assert_eq!(held.len(), 59, "{held:?}");
    assert!(held.iter().all(|id| id != "s-001"));
    facade.rollback("moqui.test.TestEntity").unwrap();

    facade.begin("moqui.test.TestEntity", None).unwrap();
    let restored = stream_ids(&facade);
    assert_eq!(restored.len(), 60, "{restored:?}");
    assert!(restored.iter().any(|id| id == "s-001"));
    drop(restored);
    facade.rollback("moqui.test.TestEntity").unwrap();

    drop(facade);
    drop(cluster);
}

#[test]
fn java_physical_names_crud_and_legacy_schema_guard() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
        &framework,
        Arc::clone(&driver),
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            ..Default::default()
        },
    )
    .unwrap();
    facade.set_disable_audit_log(true);
    facade.set_disable_entity_eca(true);
    facade.set_disable_data_feed(true);
    facade.check_tables(&["UserAccount"]).unwrap();
    // A second check must discover the same columns, not try adding differently named ones.
    facade.check_tables(&["UserAccount"]).unwrap();
    let mut conn = driver.connect().unwrap();
    let columns = conn.query("SELECT column_name FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'user_account'", &[]).unwrap();
    assert!(columns.iter().any(|row| row.columns[0].1.as_str() == Some("user_id")));
    assert!(!columns.iter().any(|row| row.columns[0].1.as_str() == Some("userId")));
    let mut user = facade.make_value("UserAccount").unwrap();
    user.set("userId", Value::string("U1"));
    user.set("username", Value::string("before"));
    user.create(&facade).unwrap();
    user.set("username", Value::string("after"));
    user.update(&facade).unwrap();
    let found = facade
        .find("UserAccount")
        .unwrap()
        .eq("userId", Value::string("U1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(found.get("username").and_then(Value::as_str), Some("after"));
    user.delete(&facade).unwrap();
    assert_eq!(facade.find("UserAccount").unwrap().count().unwrap(), 0);

    // Construct an old Rust schema on this disposable database, never a user's database.
    let conf = moqui_lib::node::MNode::parse_file(framework.join("conf/MoquiDefaultConf.xml")).unwrap();
    conf.first_named("entity-facade")
        .unwrap()
        .set_attribute("sql-name-style", "legacy");
    let legacy = moqui_core::entity::EntityFacade::open_components_with_conf(
        &framework,
        &[],
        "transactional",
        vec![moqui_core::entity::EntityGroup::new(
            "transactional",
            "postgres",
            facade.transactions_for("UserAccount").unwrap(),
        )],
        &conf,
    )
    .unwrap();
    legacy.check_tables(&["TestEntity"]).unwrap();
    let mut row = legacy.make_value("TestEntity").unwrap();
    row.set("testId", Value::string("old-data"));
    row.create(&legacy).unwrap();
    let error = facade.check_tables(&["TestEntity"]).unwrap_err();
    assert!(error.to_string().contains("legacy Rust SQL names detected"), "{error}");
    assert!(!facade.table_exists("TestEntity").unwrap());
    conn.execute("CREATE TABLE foo (\"fooId\" VARCHAR(40))", &[]).unwrap();
    let error = facade.check_tables(&["Foo"]).unwrap_err();
    assert!(error.to_string().contains("legacy Rust SQL names detected"), "{error}");
    assert!(legacy
        .find("TestEntity")
        .unwrap()
        .eq("testId", Value::string("old-data"))
        .one()
        .unwrap()
        .is_some());
}

/// A DDL statement that fails is logged and skipped, as Java's `runSqlUpdate` does: the other
/// indexes of the entity and the other entities are still checked. Postgres has no btree index
/// for `json`, so the username index fails.
#[test]
fn failed_ddl_statement_is_skipped_and_the_check_goes_on() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
        &framework,
        Arc::clone(&driver),
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let mut conn = driver.connect().unwrap();
    conn.execute("CREATE TABLE user_account (user_id VARCHAR(40), username JSON)", &[])
        .unwrap();
    facade.check_tables(&["UserAccount", "moqui.test.TestEntity"]).unwrap();
    assert!(facade.table_exists("moqui.test.TestEntity").unwrap());
    let indexes = conn
        .query(
            "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema() AND tablename = 'user_account'",
            &[],
        )
        .unwrap();
    let names: Vec<String> = indexes
        .iter()
        .filter_map(|row| row.columns[0].1.as_str().map(str::to_ascii_lowercase))
        .collect();
    assert!(!names.iter().any(|name| name == "useracct_username"), "{names:?}");
    assert!(names.iter().any(|name| name == "useracct_emailaddr"), "{names:?}");
    let columns = conn
        .query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = current_schema() AND table_name = 'user_account'",
            &[],
        )
        .unwrap();
    assert!(columns
        .iter()
        .any(|row| row.columns[0].1.as_str() == Some("email_address")));
}

/// Every framework view-entity runs a paged select and a count on Postgres, with every
/// framework table and foreign key index in place.
#[test]
fn every_framework_view_selects_and_counts_on_postgres() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let transactions = moqui_core::transaction::TransactionManager::new(
        driver,
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();
    let facade = moqui_core::entity::EntityFacade::open(
        &framework,
        "transactional",
        vec![
            moqui_core::entity::EntityGroup::new("transactional", "postgres", transactions.clone()),
            moqui_core::entity::EntityGroup::new("logging", "postgres", transactions),
        ],
    )
    .unwrap();
    facade.check_all_tables().unwrap();

    let mut views = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(framework.join("entity"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
        .collect();
    paths.sort();
    for path in paths {
        let root = moqui_lib::node::MNode::parse_file(&path).unwrap();
        for view in root.named_iter("view-entity") {
            let package = view.attribute("package").unwrap_or_default();
            let name = view.attribute("entity-name").unwrap_or_default();
            views.push(format!("{package}.{name}"));
        }
    }
    assert!(views.len() > 20, "{views:?}");
    let mut failures = Vec::new();
    for view in &views {
        let listed = facade.find(view).and_then(|find| find.limit(1).list());
        if let Err(err) = listed {
            failures.push(format!("{view} select: {err}"));
        }
        let counted = facade.find(view).and_then(|find| find.count());
        if let Err(err) = counted {
            failures.push(format!("{view} count: {err}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    drop(facade);
    drop(cluster);
}

/// As Java's `EntityDbMeta` and `EntityFindImpl`: with the find check on, a find on a table that does not exist
/// finds nothing; with `runtime-add-missing`, the first use creates the table (a view's member tables too).
#[test]
fn missing_tables_find_nothing_and_runtime_add_missing_creates_them_on_first_use() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
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
    .unwrap();
    facade.set_find_table_check(true);
    let entity = "moqui.test.TestEntity";
    assert!(facade.find(entity).unwrap().list().unwrap().is_empty());
    assert_eq!(facade.find(entity).unwrap().count().unwrap(), 0);
    assert!(facade
        .find(entity)
        .unwrap()
        .eq("testId", Value::string("A"))
        .one()
        .unwrap()
        .is_none());
    assert!(!facade.table_exists(entity).unwrap());

    facade.set_ddl_conf(
        "transactional",
        moqui_core::entity::DdlConf {
            startup_add_missing: true,
            runtime_add_missing: true,
            runtime_add_fks: true,
        },
    );
    let mut row = facade.make_value(entity).unwrap();
    row.set("testId", Value::string("A"));
    row.create(&facade).unwrap();
    assert!(facade.table_exists(entity).unwrap());
    assert_eq!(facade.find(entity).unwrap().count().unwrap(), 1);
    // A view's members are checked when the view is read.
    assert_eq!(facade.find("moqui.test.FooBar").unwrap().count().unwrap(), 0);
    assert!(facade.table_exists("moqui.test.Foo").unwrap());
    assert!(facade.table_exists("moqui.test.Bar").unwrap());

    // The SQL runner asks the database whether a statement gives rows, as JDBC's `execute`.
    let returned = moqui_core::entity::info::run_sql(
        &facade,
        "transactional",
        "insert into test_entity (test_id) values ('B') returning test_id",
        10,
    )
    .unwrap();
    assert_eq!(returned.records, vec![vec![Value::string("B")]]);
    let updated =
        moqui_core::entity::info::run_sql(&facade, "transactional", "update test_entity set test_medium = 'x'", 10)
            .unwrap();
    assert_eq!(updated.rows_affected, Some(2));

    drop(facade);
    drop(cluster);
}

/// A held update of a row the outer frame locked stays with that frame. A force-new frame's commit
/// flushed it once, on the inner connection, where it waited on the outer frame's own row lock.
#[test]
fn a_held_update_of_a_locked_row_does_not_block_an_inner_force_new_on_postgres() {
    use moqui_core::transaction::TransactionAttribute;

    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
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
    .unwrap();
    facade
        .check_tables(&["moqui.test.TestEntity", "moqui.test.Foo"])
        .unwrap();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("R"));
    row.set("testMedium", Value::string("before"));
    row.create(&facade).unwrap();

    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    // A lock wait ends at the deadline instead of hanging the test.
    let deadline = Some(Duration::from_secs(5));
    tx.run(TransactionAttribute::Cache, deadline, || {
        let mut locked = facade
            .find("moqui.test.TestEntity")?
            .eq("testId", Value::string("R"))
            .for_update(true)
            .one()?
            .expect("R");
        locked.set("testMedium", Value::string("held"));
        locked.update(&facade)?;
        tx.run(TransactionAttribute::ForceNew, deadline, || {
            let mut foo = facade.make_value("moqui.test.Foo")?;
            foo.set("fooId", Value::string("F1"));
            foo.create(&facade)
        })
    })
    .unwrap();

    let found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("R"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(found.get("testMedium").and_then(Value::as_str), Some("held"));
    let foo = facade
        .find("moqui.test.Foo")
        .unwrap()
        .eq("fooId", Value::string("F1"))
        .one()
        .unwrap();
    assert!(foo.is_some());

    drop(facade);
    drop(cluster);
}

/// A held update goes out through the pool as one the server fails when it changes no row (Round 18), so its commit
/// waits once for both, and an update of a row gone since the frame read it still fails the commit
/// (plans/Profiling3.md R54: the pooled connection had not passed `execute_deferred_update` on).
#[test]
fn a_held_update_commits_in_one_round_trip_and_fails_for_a_row_gone_on_postgres() {
    use moqui_core::transaction::TransactionAttribute;

    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
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
    .unwrap();
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();
    for id in ["U0", "U1", "U2"] {
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string(id));
        row.set("testMedium", Value::string("before"));
        row.create(&facade).unwrap();
    }
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    let find = |id: &str| {
        facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string(id))
            .one()
            .unwrap()
    };

    let held_update = |id: &str| {
        let mut commit_trips = 0;
        tx.run(TransactionAttribute::Cache, None, || {
            let mut row = find(id).expect("a row");
            row.set("testMedium", Value::string("held"));
            row.update(&facade)?;
            commit_trips = moqui_core::driver::thread_round_trips();
            Ok(())
        })
        .unwrap();
        moqui_core::driver::thread_round_trips() - commit_trips
    };
    // the first prepares the statements
    held_update("U0");
    assert_eq!(held_update("U1"), 1, "the held update and the commit wait once");
    assert_eq!(
        find("U1").unwrap().get("testMedium").and_then(Value::as_str),
        Some("held")
    );

    let gone = tx.run(TransactionAttribute::Cache, None, || {
        let mut row = find("U2").expect("U2");
        tx.run(TransactionAttribute::ForceNew, None, || {
            facade
                .find("moqui.test.TestEntity")?
                .eq("testId", Value::string("U2"))
                .delete_all()
                .map(|_| ())
        })?;
        row.set("testMedium", Value::string("held"));
        row.update(&facade)
    });
    assert!(gone.is_err(), "an update of a row gone fails the commit");
    assert!(find("U2").is_none());

    drop(facade);
    drop(cluster);
}

/// A parent row held `FOR NO KEY UPDATE` does not block another transaction's insert of a child row, whose foreign key
/// check takes `FOR KEY SHARE` on the parent; held `FOR UPDATE` it does, until the inserting transaction's statement
/// timeout. This is the lock an organization's `Party` row takes to serialize postings, which a deadlock with an
/// `Invoice` insert showed (plans/archive/LoadRunner.md section 11).
#[test]
fn a_no_key_lock_does_not_block_a_child_insert_and_a_for_update_lock_does_on_postgres() {
    use moqui_core::transaction::TransactionAttribute;

    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
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
    .unwrap();
    let (parent, child) = ("moqui.basic.EnumerationType", "moqui.basic.Enumeration");
    facade.check_tables(&[parent, child]).unwrap();
    let mut row = facade.make_value(parent).unwrap();
    row.set("enumTypeId", Value::string("NK_TYPE"));
    row.create(&facade).unwrap();

    let tx = facade.transactions_for(parent).unwrap();
    // The insert runs in a transaction of its own (another connection) while the outer one holds the lock. The no-key
    // case has a generous deadline, so a loaded machine does not fail it; the for-update one, meant to block, ends at a
    // short one (statement_timeout) instead of hanging the test.
    let insert_child = |id: &str, no_key: bool| {
        let insert_wait = Some(if no_key {
            Duration::from_secs(10)
        } else {
            Duration::from_millis(750)
        });
        tx.run(TransactionAttribute::UseOrBegin, Some(Duration::from_secs(10)), || {
            let find = facade.find(parent)?.eq("enumTypeId", Value::string("NK_TYPE"));
            let find = if no_key {
                find.for_no_key_update(true)
            } else {
                find.for_update(true)
            };
            assert!(find.one()?.is_some());
            let inserted = tx.run(TransactionAttribute::ForceNew, insert_wait, || {
                let mut enumeration = facade.make_value(child)?;
                enumeration.set("enumId", Value::string(id));
                enumeration.set("enumTypeId", Value::string("NK_TYPE"));
                enumeration.create(&facade)
            });
            Ok(inserted)
        })
        .unwrap()
    };
    // Warm the pool: a second connection is open before either lock is taken.
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        tx.run(TransactionAttribute::ForceNew, None, || facade.find(child)?.count())
    })
    .unwrap();
    insert_child("NK_1", true).expect("the insert does not wait on a no-key lock");
    let blocked = insert_child("NK_2", false);
    let blocked = format!(
        "{:?}",
        blocked.expect_err("the insert waits on a for-update lock until its timeout")
    );
    // 57014: the statement was cancelled at the deadline, waiting on the lock
    assert!(blocked.contains("57014"), "{blocked}");
    let found = |id: &str| {
        facade
            .find(child)
            .unwrap()
            .eq("enumId", Value::string(id))
            .one()
            .unwrap()
            .is_some()
    };
    assert!(found("NK_1") && !found("NK_2"));

    drop(facade);
    drop(cluster);
}

/// A quiet create held in a cache frame (`create_if_missing`, as a create-or-update of a new row makes) goes out at the
/// commit as one that writes nothing when another transaction made the row first, as it would have at once; a plain
/// insert there failed the commit on the key (the racing first chat actor of plans/Profiling3.md R60).
#[test]
fn a_held_quiet_create_of_a_row_made_meanwhile_commits_on_postgres() {
    use moqui_core::transaction::TransactionAttribute;

    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
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
    .unwrap();
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    let row = |medium: &str| {
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string("Q"));
        row.set("testMedium", Value::string(medium));
        row
    };
    tx.run(TransactionAttribute::Cache, None, || {
        assert!(row("held").create_if_missing(&facade)?, "the row is not there yet");
        tx.run(TransactionAttribute::ForceNew, None, || row("first").create(&facade))
    })
    .unwrap();
    let found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("Q"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(found.get("testMedium").and_then(Value::as_str), Some("first"));

    drop(facade);
    drop(cluster);
}

/// A data file's writes go without waiting for their results. A failed one fails its file as before: the file
/// rolls back and the error is the failed statement's, wherever in the file it was.
#[test]
fn a_pipelined_data_file_commits_whole_or_fails_with_the_write_that_failed() {
    use moqui_core::entity::load_feed::{load_entity_xml_text, LoadOptions};

    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, driver, PoolConfig::default()).unwrap();
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();
    let file = |prefix: &str, count: usize, bad: Option<usize>| {
        let mut text = String::from("<entity-facade-xml>\n");
        for index in 0..count {
            // `testIndicator` is CHAR(1): the database refuses the longer text.
            let indicator = if Some(index) == bad { "YYY" } else { "Y" };
            text.push_str(&format!(
                "<moqui.test.TestEntity testId=\"{prefix}{index}\" testMedium=\"{prefix}\" testIndicator=\"{indicator}\"/>\n"
            ));
        }
        text.push_str("</entity-facade-xml>\n");
        text
    };
    let count = |prefix: &str| {
        facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testMedium", Value::string(prefix))
            .count()
            .unwrap()
    };
    let options = LoadOptions::default();

    let loaded = load_entity_xml_text(&facade, "good", &file("G", 600, None), &options).unwrap();
    assert_eq!(loaded.records, 600);
    assert_eq!(count("G"), 600);
    // Again: each row is there, so each store is an update.
    let reloaded = load_entity_xml_text(&facade, "good again", &file("G", 600, None), &options).unwrap();
    assert_eq!(reloaded.records, 600);

    for (prefix, bad) in [("M", 5), ("L", 9)] {
        let err = load_entity_xml_text(&facade, "bad", &file(prefix, 10, Some(bad)), &options).unwrap_err();
        let text = moqui_core::entity::load_feed::error_chain(&err);
        assert!(text.contains("value too long"), "{prefix}: {text}");
        assert_eq!(count(prefix), 0, "{prefix}: the file rolled back");
    }
    assert_eq!(count("G"), 600);
}

/// The content group on a database of its own in the embedded cluster: its tables are made there and not in `moqui`,
/// with the foreign keys among them and none to the transactional tables, and one transaction spans both databases.
#[test]
fn the_content_group_has_its_own_database_in_one_transaction() {
    use moqui_core::entity::{EntityFacade, EntityGroup};
    use moqui_core::transaction::{TransactionAttribute, TransactionManager};

    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let content_driver = cluster.database_driver("moqui_content").unwrap();
    let pool = || PoolConfig {
        min: 0,
        max: 4,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let main = TransactionManager::new(Arc::new(driver.clone()), pool()).unwrap();
    let content = TransactionManager::new(Arc::new(content_driver.clone()), pool()).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = EntityFacade::open(
        &framework,
        "transactional",
        vec![
            EntityGroup::new("transactional", "postgres", main.clone()),
            EntityGroup::new("content", "postgres", content.clone()),
        ],
    )
    .unwrap();
    assert!(main.linked().iter().any(|linked| linked.shares_stack(&content)));
    facade
        .check_tables(&[
            "moqui.test.TestEntity",
            "moqui.resource.GitRepository",
            "moqui.resource.DbResource",
            "moqui.resource.DbResourceFile",
            "moqui.resource.ContentBlob",
            "moqui.resource.GitObject",
        ])
        .unwrap();
    let tables = |driver: &super::super::postgres::PostgresDriver| -> Vec<String> {
        let mut conn = driver.connect_postgres().unwrap();
        let rows = conn
            .query(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name",
                &[],
            )
            .unwrap();
        rows.iter()
            .filter_map(|row| {
                row.get("table_name")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase)
            })
            .collect()
    };
    let in_content = tables(&content_driver);
    let in_main = tables(&driver);
    for table in ["db_resource", "db_resource_file", "content_blob", "git_object"] {
        assert!(
            in_content.contains(&table.to_string()),
            "{table} in moqui_content: {in_content:?}"
        );
        assert!(
            !in_main.contains(&table.to_string()),
            "{table} not in moqui: {in_main:?}"
        );
    }
    assert!(in_main.contains(&"git_repository".to_string()) && in_main.contains(&"test_entity".to_string()));
    let mut conn = content_driver.connect_postgres().unwrap();
    let keys = conn
        .query(
            "SELECT cl.relname AS from_table, ref.relname AS to_table FROM pg_constraint c \
             JOIN pg_class cl ON cl.oid = c.conrelid JOIN pg_class ref ON ref.oid = c.confrelid WHERE c.contype = 'f'",
            &[],
        )
        .unwrap();
    let keys: Vec<(String, String)> = keys
        .iter()
        .map(|row| {
            let text = |name: &str| {
                row.get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
            };
            (text("from_table"), text("to_table"))
        })
        .collect();
    assert!(
        keys.contains(&("db_resource_file".to_string(), "db_resource".to_string())),
        "{keys:?}"
    );
    drop(conn);

    let write = |id: &str| -> Result<(), moqui_lib::error::Error> {
        let mut row = facade.make_value("moqui.test.TestEntity")?;
        row.set("testId", Value::string(id));
        row.create(&facade)?;
        let mut resource = facade.make_value("moqui.resource.DbResource")?;
        resource.set("resourceId", Value::string(id));
        resource.set("filename", Value::string(id));
        resource.set("isFile", Value::string("N"));
        resource.create(&facade)?;
        Ok(())
    };
    let found = |entity: &str, field: &str, id: &str| {
        facade
            .find(entity)
            .unwrap()
            .eq(field, Value::string(id))
            .one()
            .unwrap()
            .is_some()
    };
    let rolled_back = main.run(TransactionAttribute::UseOrBegin, None, || {
        write("ROLLED")?;
        Err::<(), _>(moqui_lib::error::Error::new("the business write fails"))
    });
    assert!(rolled_back.is_err());
    assert!(!found("moqui.test.TestEntity", "testId", "ROLLED"));
    assert!(
        !found("moqui.resource.DbResource", "resourceId", "ROLLED"),
        "the content row rolled back with it"
    );
    main.run(TransactionAttribute::UseOrBegin, None, || write("KEPT"))
        .unwrap();
    assert!(found("moqui.test.TestEntity", "testId", "KEPT"));
    assert!(found("moqui.resource.DbResource", "resourceId", "KEPT"));
    let mut conn = content_driver.connect_postgres().unwrap();
    let rows = conn
        .query("SELECT resource_id FROM db_resource WHERE resource_id = 'KEPT'", &[])
        .unwrap();
    assert_eq!(rows.len(), 1, "the row is in moqui_content");
}

fn stream_ids(facade: &moqui_core::entity::EntityFacade) -> Vec<String> {
    let mut iter = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testMedium", Value::string("stream"))
        .iterator()
        .unwrap();
    let mut ids = Vec::new();
    while let Some(row) = iter.next_value().unwrap() {
        if let Some(id) = row.get("testId").and_then(Value::as_str) {
            ids.push(id.to_string());
        }
    }
    ids
}

pub(super) fn temp_cluster_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // the clock is only microseconds on macOS, so parallel tests need the counter to get different directories
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = format!("moqui-pg-{}-{nanos}-{seq}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}
