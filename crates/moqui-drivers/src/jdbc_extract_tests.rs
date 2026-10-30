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

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::DbDriver;
use moqui_core::transaction::PoolConfig;
use moqui_lib::error::Error;
use moqui_lib::etl::{EntryTransform, Loader, SimpleEntry, SimpleEtl};
use moqui_lib::object::Value;

use super::super::embedded::EmbeddedCluster;
use super::super::entity_facade::open_entity_facade;
use super::JdbcExtractor;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Collect(Rc<RefCell<Vec<SimpleEntry>>>);

impl Loader for Collect {
    fn init(&mut self, _timeout: Option<u32>) {}

    fn load(&mut self, entry: &mut SimpleEntry) -> Result<(), Error> {
        self.0.borrow_mut().push(entry.clone());
        Ok(())
    }

    fn complete(&mut self, _etl: &SimpleEtl) {}
}

fn temp_cluster_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    PathBuf::from("/tmp").join(format!("moqui-pg-etl-{}-{nanos}", std::process::id()))
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_string)
}

#[test]
fn jdbc_extract_loads_entities_and_find_extracts() {
    let dir = temp_cluster_dir();
    let _guard = DirGuard(dir.clone());
    let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(
        &framework,
        Arc::clone(&driver),
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

    let mut conn = driver.connect().unwrap();
    conn.execute(
        "CREATE TABLE etl_source (source_id TEXT PRIMARY KEY, label TEXT, amount NUMERIC(10,2))",
        &[],
    )
    .unwrap();
    for (id, label, amount) in [("S1", "one", "1.50"), ("S2", "two", "20.00"), ("S3", "three", "3.25")] {
        conn.execute(
            &format!("INSERT INTO etl_source (source_id, label, amount) VALUES (?, ?, {amount})"),
            &[Value::string(id), Value::string(label)],
        )
        .unwrap();
    }
    drop(conn);

    // Rows into an entity, renamed by a transformer.
    let extractor = JdbcExtractor::new().set_driver(Arc::clone(&driver)).set_sql_info(
        "moqui.test.TestEntity",
        "SELECT source_id, label, amount FROM etl_source ORDER BY source_id",
    );
    assert_eq!(extractor.record_type(), "moqui.test.TestEntity");
    let mut etl = SimpleEtl::new(extractor, facade.make_etl_loader());
    etl.add_transformer(|transform: &mut EntryTransform| {
        let values = &mut transform.entry_mut().values;
        let id = values.remove("source_id").unwrap_or(Value::Null);
        let label = values.remove("label").unwrap_or(Value::Null);
        let amount = values.remove("amount").unwrap_or(Value::Null);
        values.insert("testId".to_string(), id);
        values.insert("testMedium".to_string(), label);
        values.insert("testNumberDecimal".to_string(), amount);
        Ok(())
    });
    etl.process();
    assert!(
        !etl.has_error(),
        "{:?}",
        etl.single_error_cause().map(ToString::to_string)
    );
    assert_eq!(etl.extract_count(), 3);
    assert_eq!(etl.load_count(), 3);
    let stored = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("S2"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(text(stored.get("testMedium")).as_deref(), Some("two"));
    assert_eq!(
        stored
            .get("testNumberDecimal")
            .and_then(Value::as_decimal)
            .map(|amount| amount.normalize().to_string())
            .as_deref(),
        Some("20")
    );

    // The entity rows back out through a find.
    let collected = Rc::new(RefCell::new(Vec::new()));
    let find = facade.find("moqui.test.TestEntity").unwrap().order_by("testId", false);
    let mut etl = SimpleEtl::new(find, Collect(Rc::clone(&collected)));
    etl.process();
    assert!(
        !etl.has_error(),
        "{:?}",
        etl.single_error_cause().map(ToString::to_string)
    );
    let ids: Vec<String> = collected
        .borrow()
        .iter()
        .map(|entry| text(entry.values.get("testId")).unwrap_or_default())
        .collect();
    assert_eq!(ids, ["S1", "S2", "S3"]);
    assert!(collected
        .borrow()
        .iter()
        .all(|entry| entry.etl_type == "moqui.test.TestEntity"));

    // A bad query is the extract error, named by its SQL.
    let bad = JdbcExtractor::new()
        .set_driver(Arc::clone(&driver))
        .set_sql_info("x", "SELECT nope FROM missing_table");
    let mut etl = SimpleEtl::new(bad, Collect(Rc::new(RefCell::new(Vec::new()))));
    etl.process();
    let message = etl.extract_error().map(ToString::to_string).unwrap_or_default();
    assert!(
        message.contains("Error in SQL query SELECT nope FROM missing_table"),
        "{message}"
    );
    assert!(JdbcExtractor::new()
        .set_db_info("derby", "h", "", "d", "u", "p")
        .is_err());
    drop(facade);
    drop(cluster);
}
