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

//! Host checks for the entity one-cache, the transaction hold, and the sim overlay.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::{DbDriver, DbRow};
use moqui_core::entity::cache::{enter_sim, exit_sim};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_lib::object::Value;

use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::db::entity_facade::open_entity_facade;
use moqui_host::db::sim::SimOverlay;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn one_cache_hits_and_sim_write_stays_off_main() {
    let _lock = crate::common::shared();
    let main_dir = temp_cluster_dir("main");
    let overlay_dir = temp_cluster_dir("overlay");
    let _main_guard = DirGuard(main_dir.clone());
    let _overlay_guard = DirGuard(overlay_dir.clone());
    let (main_cluster, main_driver) = EmbeddedCluster::start(&main_dir, 20).unwrap();
    let (overlay_cluster, overlay_driver) = EmbeddedCluster::start(&overlay_dir, 20).unwrap();
    let main_driver: Arc<dyn DbDriver> = Arc::new(main_driver);
    let overlay_driver: Arc<dyn DbDriver> = Arc::new(overlay_driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, Arc::clone(&main_driver), test_pool()).unwrap();

    let err = enter_sim(&facade).unwrap_err();
    assert!(!facade.sim_active(), "{err}");
    let sim = TransactionManager::new(overlay_driver, test_pool()).unwrap();
    facade.set_sim_transactions(Some(sim));

    facade
        .check_tables(&["moqui.test.TestEntity", "moqui.basic.EnumerationType"])
        .unwrap();

    assert_one_cache(&facade, &main_driver);
    assert_hold_commit_and_rollback(&facade, &main_driver);
    assert_sim_overlay(&facade, &main_driver);

    drop(facade);
    drop(main_cluster);
    drop(overlay_cluster);
}

#[test]
fn first_enter_sim_starts_the_overlay_and_stop_removes_it() {
    let _lock = crate::common::shared();
    let main_dir = temp_cluster_dir("lazy");
    let _main_guard = DirGuard(main_dir.clone());
    let (main_cluster, main_driver) = EmbeddedCluster::start(&main_dir, 20).unwrap();
    let main_driver: Arc<dyn DbDriver> = Arc::new(main_driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, Arc::clone(&main_driver), test_pool()).unwrap();
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();

    // A directory left by a process that is gone goes away at install.
    let orphan = main_dir.join("sim-4000000000");
    std::fs::create_dir_all(orphan.join("data")).unwrap();
    let overlay = SimOverlay::install(&facade, &main_dir, Some(main_dir.join("install")));
    assert!(!orphan.exists());
    assert!(!overlay.started());
    assert!(facade.sim_transactions().is_none());

    enter_sim(&facade).unwrap();
    assert!(overlay.started());
    let sim_dir = overlay.data_dir().unwrap();
    // The cluster canonicalizes its directory: on macOS /tmp resolves to /private/tmp.
    let main_real = std::fs::canonicalize(&main_dir).unwrap();
    assert!(sim_dir.starts_with(main_real.join(format!("sim-{}", std::process::id()))));
    assert!(
        !sim_dir.parent().unwrap().join("install").exists(),
        "the overlay shares the entity install"
    );
    let settings = facade
        .sim_transactions()
        .unwrap()
        .with_connection(|conn| {
            let mut shown = Vec::new();
            for name in ["synchronous_commit", "shared_buffers", "fsync"] {
                let rows = conn.query(&format!("SHOW {name}"), &[])?;
                shown.push(rows[0].columns[0].1.as_str().unwrap_or_default().to_string());
            }
            Ok(shown)
        })
        .unwrap();
    assert_eq!(settings[0], "off");
    assert!(
        settings[1] == "8MB" || settings[1] == "8192kB",
        "shared_buffers={}",
        settings[1]
    );
    assert_eq!(settings[2], "on");

    create_test(&facade, "LAZY1", "sim");
    assert!(find_one(&facade, "moqui.test.TestEntity", "testId", "LAZY1").is_some());
    exit_sim(&facade).unwrap();
    assert!(find_one(&facade, "moqui.test.TestEntity", "testId", "LAZY1").is_none());
    // A second sim reuses the running overlay. A transaction that wrote to it and rolled back ends it, as Java's
    // `TransactionCacheDb` poison: the sim write is not undone.
    enter_sim(&facade).unwrap();
    facade.begin("moqui.test.TestEntity", None).unwrap();
    create_test(&facade, "LAZY2", "sim");
    facade.rollback("moqui.test.TestEntity").unwrap();
    let err = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("LAZY2"))
        .one()
        .unwrap_err();
    assert!(err.to_string().contains("terminated"), "{err}");
    exit_sim(&facade).unwrap();
    enter_sim(&facade).unwrap();
    assert!(find_one(&facade, "moqui.test.TestEntity", "testId", "LAZY2").is_none());
    exit_sim(&facade).unwrap();

    drop(facade);
    drop(overlay);
    assert!(!sim_dir.exists(), "{} is still there", sim_dir.display());
    drop(main_cluster);
}

/// As Java's copy-on-read for views: a view read in a sim sees the sim's creates, updates and deletes, joined to
/// production rows the sim never read; an aggregate view counts both; production is untouched.
#[test]
fn views_in_a_sim_see_the_sims_rows() {
    let _lock = crate::common::shared();
    let main_dir = temp_cluster_dir("simview");
    let overlay_dir = temp_cluster_dir("simview-overlay");
    let _main_guard = DirGuard(main_dir.clone());
    let _overlay_guard = DirGuard(overlay_dir.clone());
    let (main_cluster, main_driver) = EmbeddedCluster::start(&main_dir, 20).unwrap();
    let (overlay_cluster, overlay_driver) = EmbeddedCluster::start(&overlay_dir, 20).unwrap();
    let main_driver: Arc<dyn DbDriver> = Arc::new(main_driver);
    let overlay_driver: Arc<dyn DbDriver> = Arc::new(overlay_driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, Arc::clone(&main_driver), test_pool()).unwrap();
    facade.set_sim_transactions(Some(TransactionManager::new(overlay_driver, test_pool()).unwrap()));
    facade
        .check_tables(&[
            "moqui.basic.Enumeration",
            "moqui.basic.EnumerationType",
            "moqui.test.TestEntity",
        ])
        .unwrap();
    facade.set_disable_fk_create(true);

    enum_type(&facade, "VT1", "Type One");
    enum_type(&facade, "VT3", "Type Three");
    for (id, description, seq) in [("VE1", "first", 1), ("VE2", "second", 2), ("VE3", "third", 3)] {
        enumeration(&facade, id, "VT1", description, seq);
    }
    for (id, medium) in [("AG1", "M"), ("AG2", "M"), ("AG3", "N")] {
        create_test(&facade, id, medium);
    }
    assert_eq!(view_ids(&facade, "VT1"), ["VE1", "VE2", "VE3"]);

    enter_sim(&facade).unwrap();
    // A sim row joins a production row the sim never read.
    enumeration(&facade, "VE4", "VT1", "fourth", 4);
    let rows = view_rows(&facade, "VT1");
    assert_eq!(ids(&rows), ["VE1", "VE2", "VE3", "VE4"]);
    assert_eq!(text(&rows[3], "typeDescription"), "Type One");

    // A type with no production enumerations: only the join walk from the sim's row finds it.
    enumeration(&facade, "VE5", "VT3", "fifth", 5);
    let rows = view_rows(&facade, "VT3");
    assert_eq!(ids(&rows), ["VE5"]);
    assert_eq!(text(&rows[0], "typeDescription"), "Type Three");

    // Updates and deletes of production members.
    let mut second = find_one(&facade, "moqui.basic.Enumeration", "enumId", "VE2").unwrap();
    second.set("description", Value::string("changed"));
    second.update(&facade).unwrap();
    let mut third = facade.make_value("moqui.basic.Enumeration").unwrap();
    third.set("enumId", Value::string("VE3"));
    third.delete(&facade).unwrap();
    let rows = view_rows(&facade, "VT1");
    assert_eq!(ids(&rows), ["VE1", "VE2", "VE4"]);
    assert_eq!(text(&rows[1], "description"), "changed");

    // A row that moved to another type leaves this one, and joins the sim's new type.
    enum_type(&facade, "VT2", "Type Two");
    let mut first = find_one(&facade, "moqui.basic.Enumeration", "enumId", "VE1").unwrap();
    first.set("enumTypeId", Value::string("VT2"));
    first.update(&facade).unwrap();
    assert_eq!(view_ids(&facade, "VT1"), ["VE2", "VE4"]);
    let moved = view_rows(&facade, "VT2");
    assert_eq!(ids(&moved), ["VE1"]);
    assert_eq!(text(&moved[0], "typeDescription"), "Type Two");

    // Paging and counting read the same rows.
    let page = facade
        .find("moqui.basic.EnumerationAndType")
        .unwrap()
        .eq("enumTypeId", Value::string("VT1"))
        .order_by("enumId", false)
        .limit(1)
        .offset(1)
        .list()
        .unwrap();
    assert_eq!(ids(page.values()), ["VE4"]);
    let count = facade
        .find("moqui.basic.EnumerationAndType")
        .unwrap()
        .eq("enumTypeId", Value::string("VT1"))
        .count()
        .unwrap();
    assert_eq!(count, 2);
    let mut iterator = facade
        .find("moqui.basic.EnumerationAndType")
        .unwrap()
        .eq("enumTypeId", Value::string("VT1"))
        .order_by("enumId", false)
        .iterator()
        .unwrap();
    let mut iterated = Vec::new();
    while let Some(row) = iterator.next_value().unwrap() {
        iterated.push(row);
    }
    assert_eq!(ids(&iterated), ["VE2", "VE4"]);

    // A production row copied for a view read updates in place, and the plain read sees the update.
    let mut type_one = find_one(&facade, "moqui.basic.EnumerationType", "enumTypeId", "VT1").unwrap();
    type_one.set("description", Value::string("Type One Sim"));
    type_one.update(&facade).unwrap();
    let type_one = find_one(&facade, "moqui.basic.EnumerationType", "enumTypeId", "VT1").unwrap();
    assert_eq!(text(&type_one, "description"), "Type One Sim");
    assert_eq!(text(&view_rows(&facade, "VT1")[0], "typeDescription"), "Type One Sim");

    // An aggregate view counts production's rows and the sim's.
    create_test(&facade, "AG4", "M");
    let counted = facade
        .find("moqui.test.TestEntityCount")
        .unwrap()
        .eq("testMedium", Value::string("M"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(text(&counted, "entityCount"), "3");
    exit_sim(&facade).unwrap();

    let rows = view_rows(&facade, "VT1");
    assert_eq!(ids(&rows), ["VE1", "VE2", "VE3"]);
    assert_eq!(text(&rows[1], "description"), "second");
    assert_eq!(text(&rows[0], "typeDescription"), "Type One");
    let counted = facade
        .find("moqui.test.TestEntityCount")
        .unwrap()
        .eq("testMedium", Value::string("M"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(text(&counted, "entityCount"), "2");

    drop(facade);
    drop(main_cluster);
    drop(overlay_cluster);
}

fn enum_type(facade: &EntityFacade, id: &str, description: &str) {
    let mut row = facade.make_value("moqui.basic.EnumerationType").unwrap();
    row.set("enumTypeId", Value::string(id));
    row.set("description", Value::string(description));
    row.create(facade).unwrap();
}

fn enumeration(facade: &EntityFacade, id: &str, type_id: &str, description: &str, seq: i64) {
    let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string(id));
    row.set("enumTypeId", Value::string(type_id));
    row.set("description", Value::string(description));
    row.set("sequenceNum", Value::I64(seq));
    row.create(facade).unwrap();
}

fn view_rows(facade: &EntityFacade, type_id: &str) -> Vec<EntityValue> {
    facade
        .find("moqui.basic.EnumerationAndType")
        .unwrap()
        .eq("enumTypeId", Value::string(type_id))
        .order_by("enumId", false)
        .list()
        .unwrap()
        .into_values()
}

fn view_ids(facade: &EntityFacade, type_id: &str) -> Vec<String> {
    ids(&view_rows(facade, type_id))
}

fn ids(rows: &[EntityValue]) -> Vec<String> {
    rows.iter().map(|row| text(row, "enumId")).collect()
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn assert_one_cache(facade: &EntityFacade, driver: &Arc<dyn DbDriver>) {
    let mut row = facade.make_value("moqui.basic.EnumerationType").unwrap();
    row.set("enumTypeId", Value::string("CACHE1"));
    row.set("description", Value::string("cached"));
    row.create(facade).unwrap();
    let found = find_one(facade, "moqui.basic.EnumerationType", "enumTypeId", "CACHE1").unwrap();
    assert_eq!(found.get("description").and_then(Value::as_str), Some("cached"));

    let mut conn = driver.connect().unwrap();
    let deleted = conn
        .execute(
            "DELETE FROM enumeration_type WHERE enum_type_id = ?",
            &[Value::string("CACHE1")],
        )
        .unwrap();
    assert_eq!(deleted, 1);
    drop(conn);
    assert!(raw_rows(
        driver,
        "SELECT enum_type_id FROM enumeration_type WHERE enum_type_id = ?",
        "CACHE1"
    )
    .is_empty());

    let cached = find_one(facade, "moqui.basic.EnumerationType", "enumTypeId", "CACHE1").unwrap();
    assert_eq!(cached.get("description").and_then(Value::as_str), Some("cached"));
}

fn assert_hold_commit_and_rollback(facade: &EntityFacade, driver: &Arc<dyn DbDriver>) {
    facade.begin("moqui.test.TestEntity", None).unwrap();
    create_test(facade, "COMMIT1", "committed");
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "COMMIT1").is_some());
    assert!(raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "COMMIT1").is_empty());
    facade.commit("moqui.test.TestEntity").unwrap();
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "COMMIT1").is_some());
    assert_eq!(
        raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "COMMIT1").len(),
        1
    );

    facade.begin("moqui.test.TestEntity", None).unwrap();
    create_test(facade, "HOLD1", "held");
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "HOLD1").is_some());
    facade.rollback("moqui.test.TestEntity").unwrap();
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "HOLD1").is_none());
    assert!(raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "HOLD1").is_empty());
}

fn assert_sim_overlay(facade: &EntityFacade, driver: &Arc<dyn DbDriver>) {
    create_test(facade, "T1", "main");
    assert_eq!(
        raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "T1").len(),
        1
    );

    enter_sim(facade).unwrap();
    assert!(facade.sim_active());
    create_test(facade, "SIM2", "overlay");
    let sim_row = find_one(facade, "moqui.test.TestEntity", "testId", "SIM2").unwrap();
    assert_eq!(sim_row.get("testMedium").and_then(Value::as_str), Some("overlay"));
    assert!(raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "SIM2").is_empty());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "T1").is_some());

    let mut hidden = facade.make_value("moqui.test.TestEntity").unwrap();
    hidden.set("testId", Value::string("T1"));
    hidden.delete(facade).unwrap();
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "T1").is_none());
    assert_eq!(
        raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "T1").len(),
        1
    );

    exit_sim(facade).unwrap();
    assert!(!facade.sim_active());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "SIM2").is_none());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "T1").is_some());

    enter_sim(facade).unwrap();
    create_test(facade, "SIM-OUTER", "outer");
    enter_sim(facade).unwrap();
    assert!(facade.sim_active());
    create_test(facade, "SIM-INNER", "inner");
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "SIM-INNER").is_some());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "SIM-OUTER").is_none());
    exit_sim(facade).unwrap();
    assert!(facade.sim_active());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "SIM-OUTER").is_some());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "SIM-INNER").is_none());
    exit_sim(facade).unwrap();
    assert!(!facade.sim_active());
    assert!(find_one(facade, "moqui.test.TestEntity", "testId", "SIM-OUTER").is_none());
    assert!(raw_rows(driver, "SELECT test_id FROM test_entity WHERE test_id = ?", "SIM-OUTER").is_empty());
}

fn create_test(facade: &EntityFacade, id: &str, medium: &str) {
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string(id));
    row.set("testMedium", Value::string(medium));
    row.create(facade).unwrap();
}

fn test_pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 4,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

fn find_one(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> Option<EntityValue> {
    facade.find(entity).unwrap().eq(field, Value::string(id)).one().unwrap()
}

fn raw_rows(driver: &Arc<dyn DbDriver>, sql: &str, id: &str) -> Vec<DbRow> {
    let mut conn = driver.connect().unwrap();
    conn.query(sql, &[Value::string(id)]).unwrap()
}

fn temp_cluster_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("moqui-pg-{label}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}
