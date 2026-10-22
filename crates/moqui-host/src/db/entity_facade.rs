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

//! Register one entity facade. The driver is the host's. Core does not open Postgres.

use std::path::Path;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::DbDriver;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_lib::error::Error;

/// Load the framework model onto the `transactional` group using the `postgres` dialect.
pub fn open_entity_facade(
    framework_dir: &Path,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
) -> Result<EntityFacade, Error> {
    open_entity_facade_dialect(framework_dir, driver, pool, "postgres")
}

/// Load the framework model onto the `transactional` group using `dialect`.
///
/// `dialect` is a `database-list` name such as `postgres` or `mysql`.
///
/// # Errors
///
/// Returns an error when the conf or the entity model cannot be read, or when `dialect` is not defined.
pub fn open_entity_facade_dialect(
    framework_dir: &Path,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
) -> Result<EntityFacade, Error> {
    EntityFacade::open_with_driver(framework_dir, driver, pool, dialect, &["transactional"])
}

/// [`open_entity_facade_dialect`] with the `entity` directories of the components
/// ([`component_entity_dirs`]): their entities, extends, and entity ECA rules join the framework's.
///
/// # Errors
///
/// Returns an error when the conf or the entity model cannot be read, or when `dialect` is not defined.
pub fn open_entity_facade_components(
    framework_dir: &Path,
    component_entity_dirs: &[std::path::PathBuf],
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
) -> Result<EntityFacade, Error> {
    EntityFacade::open_with_driver_components(
        framework_dir,
        component_entity_dirs,
        driver,
        pool,
        dialect,
        &["transactional"],
    )
}

/// Open the runtime's merged configuration, not just the framework defaults.
/// In particular, physical naming overrides must be identical for DDL and queries.
pub fn open_entity_facade_for_factory(
    factory: &ExecutionContextFactory,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
) -> Result<EntityFacade, Error> {
    let conf = moqui_lib::node::MNode::parse_text("moqui-conf", factory.conf_text())?;
    let tx = moqui_core::transaction::TransactionManager::new(driver, pool)?;
    tx.configure_from_conf(&conf)?;
    let facade = EntityFacade::open_components_with_conf(
        factory.framework_dir(),
        &component_entity_dirs(factory),
        "transactional",
        vec![moqui_core::entity::EntityGroup::new("transactional", dialect, tx)],
        &conf,
    )?;
    facade.crypt_from_conf(&conf)?;
    Ok(facade)
}

/// Java discovers database-backed views before checking/creating the entity schema.
/// Keep the real read in an execution context so its transaction has entity identity.
pub fn discover_db_views(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade) -> Result<(), Error> {
    if !moqui_core::deterministic::enabled() {
        return Ok(());
    }
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let views = facade.find("moqui.entity.view.DbViewEntity")?.list()?;
    tracing::info!("Found {} view-entity definitions in database (DbViewEntity records)", views.len());
    if !views.is_empty() {
        return Err(Error::new("Database-backed view definitions require model registration before startup can continue"));
    }
    Ok(())
}

/// The `entity` directory of each component the factory found, in its load order, the framework
/// left out (its directory is always read).
pub fn component_entity_dirs(factory: &ExecutionContextFactory) -> Vec<std::path::PathBuf> {
    factory
        .components()
        .iter()
        .filter(|info| !crate::same_dir(&info.location, factory.framework_dir()))
        .map(|info| info.location.join("entity"))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Install the facade so `dbresource://` reads and writes through it, with the conf connection drivers scripts use for
/// another database.
pub fn attach(factory: &ExecutionContextFactory, facade: EntityFacade) {
    super::install_conf_drivers();
    factory.set_entity(facade);
}

#[cfg(all(test, unix, feature = "embedded-postgres", feature = "postgres"))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use moqui_core::driver::DbDriver;
    use moqui_core::transaction::PoolConfig;
    use moqui_lib::object::Value;

    use super::super::embedded::EmbeddedCluster;
    use super::open_entity_facade;

    struct DirGuard(PathBuf);

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
        assert!(!facade.table_exists("moqui.test.FooBar").unwrap());

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
            for view in root.children_named("view-entity") {
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

    fn temp_cluster_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = format!("moqui-pg-{}-{nanos}", std::process::id());
        if Path::new("/tmp").is_dir() {
            PathBuf::from("/tmp").join(name)
        } else {
            std::env::temp_dir().join(name)
        }
    }
}
