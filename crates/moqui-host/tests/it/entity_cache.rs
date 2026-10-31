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
use moqui_core::entity::cache::{enter_sim, exit_sim, sim_changes, sim_poisoned};
use moqui_core::entity::sim_step;
use moqui_core::entity::{Condition, EntityFacade, EntityValue};
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

/// Steps of one sim run under savepoints of a held overlay transaction (moqui_core::entity::sim_step): a step whose
/// transaction rolled back poisons the sim only until its savepoint is rolled back, which undoes its rows and puts back
/// the session's state, so the steps before and after it stand.
#[test]
fn a_rolled_back_sim_step_leaves_the_steps_around_it() {
    let _lock = crate::common::shared();
    let main_dir = temp_cluster_dir("simstep");
    let overlay_dir = temp_cluster_dir("simstep-overlay");
    let _main_guard = DirGuard(main_dir.clone());
    let _overlay_guard = DirGuard(overlay_dir.clone());
    let (main_cluster, main_driver) = EmbeddedCluster::start(&main_dir, 20).unwrap();
    let (overlay_cluster, overlay_driver) = EmbeddedCluster::start(&overlay_dir, 20).unwrap();
    let main_driver: Arc<dyn DbDriver> = Arc::new(main_driver);
    let overlay_driver: Arc<dyn DbDriver> = Arc::new(overlay_driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, Arc::clone(&main_driver), test_pool()).unwrap();
    facade.set_sim_transactions(Some(TransactionManager::new(overlay_driver, test_pool()).unwrap()));
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();
    create_test(&facade, "SP0", "production");
    const TEST: &str = "moqui.test.TestEntity";
    let medium = |id: &str| find_one(&facade, TEST, "testId", id).map(|row| text(&row, "testMedium"));

    // Without a held transaction there is no savepoint.
    enter_sim(&facade).unwrap();
    assert!(sim_step::savepoint(&facade).is_err());
    let hold = sim_step::hold(&facade).unwrap();

    // Step 1 is kept.
    let step = sim_step::savepoint(&facade).unwrap();
    create_test(&facade, "SP1", "one");
    step.release().unwrap();

    // Step 2 writes in a transaction that rolls back: the sim fails only the step, and its savepoint undoes the
    // create and the update, in the overlay and in the session.
    let step = sim_step::savepoint(&facade).unwrap();
    facade.begin(TEST, None).unwrap();
    create_test(&facade, "SP2", "two");
    let mut production = find_one(&facade, TEST, "testId", "SP0").unwrap();
    production.set("testMedium", Value::string("changed"));
    production.update(&facade).unwrap();
    facade.rollback(TEST).unwrap();
    let reason = sim_poisoned().unwrap();
    assert!(reason.contains("step"), "{reason}");
    step.rollback().unwrap();
    assert_eq!(sim_poisoned(), None);
    assert_eq!(medium("SP0").as_deref(), Some("production"));
    assert_eq!(medium("SP1").as_deref(), Some("one"));
    assert_eq!(medium("SP2"), None);
    let changed: Vec<String> = sim_changes(&facade)
        .unwrap()
        .into_iter()
        .map(|change| change.key[0].1.clone())
        .collect();
    assert_eq!(changed, ["SP1"]);

    // Step 3 runs on what step 1 left: the id step 2 used is free again, and a failed statement inside the step
    // leaves the held transaction usable.
    let step = sim_step::savepoint(&facade).unwrap();
    create_test(&facade, "SP2", "three");
    let mut again = facade.make_value(TEST).unwrap();
    again.set("testId", Value::string("SP1"));
    assert!(again.create(&facade).is_err());
    step.release().unwrap();
    assert_eq!(medium("SP2").as_deref(), Some("three"));

    // A step dropped without an end rolls back too.
    {
        let _step = sim_step::savepoint(&facade).unwrap();
        create_test(&facade, "SP4", "four");
    }
    assert_eq!(medium("SP4"), None);

    // The held transaction commits at its end, so the sim keeps what the steps left; production never saw it.
    hold.end().unwrap();
    assert_eq!(sim_poisoned(), None);
    assert_eq!(medium("SP1").as_deref(), Some("one"));
    assert_eq!(medium("SP2").as_deref(), Some("three"));
    exit_sim(&facade).unwrap();
    assert_eq!(medium("SP1"), None);
    assert_eq!(medium("SP2"), None);
    assert_eq!(medium("SP0").as_deref(), Some("production"));

    drop(facade);
    drop(main_cluster);
    drop(overlay_cluster);
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

/// Java's partial sim update: one column changes on the overlay row and the others stay, and a raw production select
/// is the row as it was written.
#[test]
fn a_partial_sim_update_leaves_the_other_columns() {
    let _lock = crate::common::shared();
    let started = start_facade("part", &["moqui.test.TestEntity"], true);
    let facade = &started.facade;
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("TCPART1"));
    row.set("testMedium", Value::string("keep-me"));
    row.set("testLong", Value::string("long-keep"));
    row.create(facade).unwrap();

    enter_sim(facade).unwrap();
    let _leave = ExitSim(facade);
    let mut changed = facade.make_value("moqui.test.TestEntity").unwrap();
    changed.set("testId", Value::string("TCPART1"));
    changed.set("testMedium", Value::string("changed"));
    changed.update(facade).unwrap();

    let overlay = find_one(facade, "moqui.test.TestEntity", "testId", "TCPART1").unwrap();
    assert_eq!(text(&overlay, "testMedium"), "changed");
    assert_eq!(text(&overlay, "testLong"), "long-keep");
    let stored = production_columns(&started.driver, "TCPART1").unwrap();
    assert_eq!(stored.0, "keep-me");
    assert_eq!(stored.1, "long-keep");
}

/// Java's non-key find in a sim: the updated and created values match, the stale production value does not, and a
/// primary key plus the old value still returns the overlay row.
#[test]
fn a_non_key_find_sees_the_sim_update_not_the_stale_row() {
    let _lock = crate::common::shared();
    let started = start_facade("npk", &["moqui.test.TestEntity"], true);
    let facade = &started.facade;
    create_test(facade, "TCNPK1", "npk-orig");

    enter_sim(facade).unwrap();
    let _leave = ExitSim(facade);
    let mut row = find_one(facade, "moqui.test.TestEntity", "testId", "TCNPK1").unwrap();
    row.set("testMedium", Value::string("npk-changed"));
    row.update(facade).unwrap();
    create_test(facade, "TCNPK2", "npk-new");

    let by_new = find_one(facade, "moqui.test.TestEntity", "testMedium", "npk-changed").unwrap();
    assert_eq!(text(&by_new, "testId"), "TCNPK1");
    assert!(find_one(facade, "moqui.test.TestEntity", "testMedium", "npk-orig").is_none());
    let by_created = find_one(facade, "moqui.test.TestEntity", "testMedium", "npk-new").unwrap();
    assert_eq!(text(&by_created, "testId"), "TCNPK2");
    // A find one with the whole primary key ignores the other condition, in production and in the sim.
    let key_plus_old = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("TCNPK1"))
        .eq("testMedium", Value::string("npk-orig"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(text(&key_plus_old, "testId"), "TCNPK1");
    assert_eq!(text(&key_plus_old, "testMedium"), "npk-changed");
    let stored = production_columns(&started.driver, "TCNPK1").unwrap();
    assert_eq!(stored.0, "npk-orig");
    assert!(production_columns(&started.driver, "TCNPK2").is_none());
}

/// Groovy's "HOLD create of a row that exists in production fails like production, and update of a missing row fails"
/// throws `EntityException` for both. The overlay insert does not see the production key, so both calls return `Ok`
/// and only the sim find changes.
#[test]
fn a_sim_create_of_a_production_key_overlays_it_and_a_missing_update_inserts() {
    let _lock = crate::common::shared();
    let started = start_facade("dup", &["moqui.test.TestEntity"], true);
    let facade = &started.facade;
    create_test(facade, "TCDUP1", "real");

    enter_sim(facade).unwrap();
    let _leave = ExitSim(facade);
    let mut dup = facade.make_value("moqui.test.TestEntity").unwrap();
    dup.set("testId", Value::string("TCDUP1"));
    dup.set("testMedium", Value::string("dup"));
    // Groovy throws. This create returns Ok.
    dup.create(facade).unwrap();
    let seen = find_one(facade, "moqui.test.TestEntity", "testId", "TCDUP1").unwrap();
    assert_eq!(text(&seen, "testId"), "TCDUP1");
    assert_eq!(text(&seen, "testMedium"), "dup");
    let stored = production_columns(&started.driver, "TCDUP1").unwrap();
    assert_eq!(stored.0, "real");

    let mut missing = facade.make_value("moqui.test.TestEntity").unwrap();
    missing.set("testId", Value::string("TCNOROW"));
    missing.set("testMedium", Value::string("x"));
    // Groovy's update of a missing row throws ("does not exist"). This update returns Ok.
    missing.update(facade).unwrap();
    let created = find_one(facade, "moqui.test.TestEntity", "testId", "TCNOROW").unwrap();
    assert_eq!(text(&created, "testId"), "TCNOROW");
    assert_eq!(text(&created, "testMedium"), "x");
    assert!(production_columns(&started.driver, "TCNOROW").is_none());
}

/// Java's paged sim list: offset is applied once, a `selectField` of `testId` does not blank the other columns of a
/// later full read, and `count` ignores `limit`.
#[test]
fn a_sim_page_offsets_once_and_a_partial_select_keeps_other_columns() {
    let _lock = crate::common::shared();
    let started = start_facade("page", &["moqui.test.TestEntity"], true);
    let facade = &started.facade;
    for i in 1..=5 {
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string(format!("TCPG{i}")));
        row.set("testMedium", Value::string(format!("m{i}")));
        row.set("testLong", Value::string(format!("l{i}")));
        row.create(facade).unwrap();
    }

    enter_sim(facade).unwrap();
    let _leave = ExitSim(facade);
    let page = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::like("testId", Value::string("TCPG%")))
        .order_by("testId", false)
        .offset(2)
        .limit(2)
        .list()
        .unwrap();
    assert_eq!(texts(page.values(), "testId"), ["TCPG3", "TCPG4"]);
    let partial = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::like("testId", Value::string("TCPG%")))
        .select_field("testId")
        .order_by("testId", false)
        .list()
        .unwrap();
    assert_eq!(partial.len(), 5);
    assert_eq!(
        texts(partial.values(), "testId"),
        ["TCPG1", "TCPG2", "TCPG3", "TCPG4", "TCPG5"]
    );
    let full = find_one(facade, "moqui.test.TestEntity", "testId", "TCPG1").unwrap();
    assert_eq!(text(&full, "testMedium"), "m1");
    assert_eq!(text(&full, "testLong"), "l1");
    let counted = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::like("testId", Value::string("TCPG%")))
        .limit(1)
        .count()
        .unwrap();
    assert_eq!(counted, 5);
}

/// Java's committed sim write stays visible after `commit` and a new `begin`, and the sim is not poisoned.
/// Rollback after a read or a write stays on `a_rolled_back_sim_write_poisons_the_sim`.
#[test]
fn a_committed_sim_write_is_found_after_a_new_begin() {
    let _lock = crate::common::shared();
    let started = start_facade("commit", &["moqui.test.TestEntity"], true);
    let facade = &started.facade;
    enter_sim(facade).unwrap();
    let _leave = ExitSim(facade);
    facade.begin("moqui.test.TestEntity", None).unwrap();
    create_test(facade, "TCCOM1", "overlay");
    facade.commit("moqui.test.TestEntity").unwrap();
    facade.begin("moqui.test.TestEntity", None).unwrap();
    let found = find_one(facade, "moqui.test.TestEntity", "testId", "TCCOM1").unwrap();
    assert_eq!(text(&found, "testMedium"), "overlay");
    assert!(sim_poisoned().is_none());
    assert!(facade.sim_active());
    assert!(raw_rows(
        &started.driver,
        "SELECT test_id FROM test_entity WHERE test_id = ?",
        "TCCOM1"
    )
    .is_empty());
    facade.commit("moqui.test.TestEntity").unwrap();
}

/// Java's cached `ArtifactAuthzCheckView`: a new `ArtifactAuthz` adds one row per member of its group. The group,
/// its two members, and the authz are created here, so the check does not use the `SCREEN_TREE` seed.
#[test]
fn a_new_artifact_authz_adds_one_view_row_per_member() {
    let _lock = crate::common::shared();
    let started = start_facade("authz", AUTHZ_TABLES, false);
    let facade = &started.facade;
    authz_base(facade, "AUG1", "AGZ1", &["ONE", "TWO"]);
    let before = authz_view(facade, "AUG1");
    assert!(before.is_empty(), "before {}", before.len());
    artifact_authz(facade, "AAZ1", "AUG1", "AGZ1");
    let after = authz_view(facade, "AUG1");
    assert_eq!(after.len(), before.len() + 2);
    assert_eq!(rows_with(&after, "artifactGroupId", "AGZ1"), 2);
}

/// Java's cached `ArtifactAuthzCheckView`: a new `ArtifactGroupMember` adds one row. Two members and the authz
/// already exist, so the group then has three view rows, with no `SCREEN_TREE` seed.
#[test]
fn a_new_artifact_group_member_adds_one_authz_view_row() {
    let _lock = crate::common::shared();
    let started = start_facade("authzm", AUTHZ_TABLES, false);
    let facade = &started.facade;
    authz_base(facade, "AUG1", "AGZ1", &["ONE", "TWO"]);
    artifact_authz(facade, "AAZ1", "AUG1", "AGZ1");
    let before = authz_view(facade, "AUG1");
    assert_eq!(before.len(), 2);
    let mut member = facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
    member.set("artifactGroupId", Value::string("AGZ1"));
    member.set("artifactName", Value::string("TEST"));
    member.set("artifactTypeEnumId", Value::string("AT_XML_SCREEN"));
    member.create(facade).unwrap();
    let after = authz_view(facade, "AUG1");
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(rows_with(&after, "artifactGroupId", "AGZ1"), 3);
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

fn texts(rows: &[EntityValue], field: &str) -> Vec<String> {
    rows.iter().map(|row| text(row, field)).collect()
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

/// Drops the innermost sim. Declared after the facade's owner so it runs while that facade is still up.
struct ExitSim<'a>(&'a EntityFacade);

impl Drop for ExitSim<'_> {
    fn drop(&mut self) {
        let _ = exit_sim(self.0);
    }
}

struct Started {
    _guards: Vec<DirGuard>,
    _main: EmbeddedCluster,
    _overlay: Option<EmbeddedCluster>,
    driver: Arc<dyn DbDriver>,
    facade: EntityFacade,
}

fn start_facade(label: &str, tables: &[&str], with_overlay: bool) -> Started {
    let main_dir = temp_cluster_dir(label);
    let mut guards = vec![DirGuard(main_dir.clone())];
    let (main_cluster, main_driver) = EmbeddedCluster::start(&main_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(main_driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade = open_entity_facade(&framework, Arc::clone(&driver), test_pool()).unwrap();
    let overlay = if with_overlay {
        let overlay_dir = temp_cluster_dir(&format!("{label}-overlay"));
        guards.push(DirGuard(overlay_dir.clone()));
        let (overlay_cluster, overlay_driver) = EmbeddedCluster::start(&overlay_dir, 20).unwrap();
        let overlay_driver: Arc<dyn DbDriver> = Arc::new(overlay_driver);
        facade.set_sim_transactions(Some(TransactionManager::new(overlay_driver, test_pool()).unwrap()));
        Some(overlay_cluster)
    } else {
        None
    };
    facade.check_tables(tables).unwrap();
    Started {
        _guards: guards,
        _main: main_cluster,
        _overlay: overlay,
        driver,
        facade,
    }
}

/// `test_medium` and `test_long` of the production row, or none when production has no such id.
fn production_columns(driver: &Arc<dyn DbDriver>, id: &str) -> Option<(String, String)> {
    let rows = raw_rows(
        driver,
        "SELECT test_medium, test_long FROM test_entity WHERE test_id = ?",
        id,
    );
    let row = rows.into_iter().next()?;
    Some((cell(&row, "test_medium"), cell(&row, "test_long")))
}

fn cell(row: &DbRow, column: &str) -> String {
    match row.get(column) {
        Some(value) => moqui_lib::object::to_plain_string(value),
        None => panic!("no column {column} in {:?}", row.columns),
    }
}

const AUTHZ_TABLES: &[&str] = &[
    "moqui.basic.EnumerationType",
    "moqui.basic.Enumeration",
    "moqui.security.UserGroup",
    "moqui.security.ArtifactGroup",
    "moqui.security.ArtifactGroupMember",
    "moqui.security.ArtifactAuthz",
    "moqui.security.ArtifactAuthzCheckView",
];

fn authz_base(facade: &EntityFacade, user_group_id: &str, artifact_group_id: &str, members: &[&str]) {
    enum_type(facade, "AuthzType", "Authorization Type");
    enum_type(facade, "AuthzAction", "Authorization Action");
    enum_type(facade, "ArtifactType", "Artifact Type");
    enumeration(facade, "AUTHZT_ALWAYS", "AuthzType", "Always Allow", 1);
    enumeration(facade, "AUTHZA_VIEW", "AuthzAction", "View", 1);
    enumeration(facade, "AT_XML_SCREEN", "ArtifactType", "Screen", 1);
    let mut user = facade.make_value("moqui.security.UserGroup").unwrap();
    user.set("userGroupId", Value::string(user_group_id));
    user.set("description", Value::string(user_group_id));
    user.create(facade).unwrap();
    let mut group = facade.make_value("moqui.security.ArtifactGroup").unwrap();
    group.set("artifactGroupId", Value::string(artifact_group_id));
    group.set("description", Value::string(artifact_group_id));
    group.create(facade).unwrap();
    for name in members {
        let mut member = facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
        member.set("artifactGroupId", Value::string(artifact_group_id));
        member.set("artifactName", Value::string(*name));
        member.set("artifactTypeEnumId", Value::string("AT_XML_SCREEN"));
        member.create(facade).unwrap();
    }
}

fn artifact_authz(facade: &EntityFacade, id: &str, user_group_id: &str, artifact_group_id: &str) {
    let mut row = facade.make_value("moqui.security.ArtifactAuthz").unwrap();
    row.set("artifactAuthzId", Value::string(id));
    row.set("userGroupId", Value::string(user_group_id));
    row.set("artifactGroupId", Value::string(artifact_group_id));
    row.set("authzTypeEnumId", Value::string("AUTHZT_ALWAYS"));
    row.set("authzActionEnumId", Value::string("AUTHZA_VIEW"));
    row.create(facade).unwrap();
}

/// Cached, as the Groovy checks: a list kept from before the write would stay at the old size.
fn authz_view(facade: &EntityFacade, user_group_id: &str) -> Vec<EntityValue> {
    facade
        .find("moqui.security.ArtifactAuthzCheckView")
        .unwrap()
        .eq("userGroupId", Value::string(user_group_id))
        .use_cache(true)
        .list()
        .unwrap()
        .into_values()
}

fn rows_with(rows: &[EntityValue], field: &str, value: &str) -> usize {
    rows.iter().filter(|row| text(row, field) == value).count()
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
