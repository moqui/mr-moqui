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

use std::sync::{Arc, Mutex};

use moqui_db::sql::{CompareOp, Condition};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::{eval_condition, judge, may_match, FindLookup, ReadMeta, WriteGate};
use crate::entity::EntityFacade;
use crate::testing;
use crate::transaction::{TransactionAttribute, TransactionManager};

fn row(pairs: &[(&str, Value)]) -> crate::entity::FieldMap {
    pairs
        .iter()
        .map(|(name, value)| ((*name).into(), value.clone()))
        .collect()
}

/// The eviction check: false only when the condition surely does not match what was written.
#[test]
fn a_write_may_match_unless_the_condition_surely_excludes_it() {
    let written = row(&[("enumTypeId", Value::string("B")), ("enumId", Value::string("E9"))]);
    let typed = || Condition::equals("enumTypeId", Value::string("A"));
    assert!(!may_match(Some(&typed()), &written));
    assert!(may_match(
        Some(&Condition::equals("enumTypeId", Value::string("B"))),
        &written
    ));
    // A field the write did not set, or a comparison this cannot judge, may match.
    assert!(may_match(
        Some(&Condition::equals("description", Value::string("x"))),
        &written
    ));
    assert!(may_match(
        Some(&Condition::Like("enumTypeId".into(), Value::string("A%"))),
        &written
    ));
    // Different types are not judged.
    assert_eq!(judge(&Condition::equals("enumTypeId", Value::I64(1)), &written), None);
    assert_eq!(
        judge(
            &Condition::and(vec![typed(), Condition::equals("description", Value::string("x"))]),
            &written
        ),
        Some(false)
    );
    assert_eq!(
        judge(
            &Condition::is_in("enumTypeId", vec![Value::string("A"), Value::string("B")]),
            &written
        ),
        Some(true)
    );
}

/// Java's `EntityCache.clearCacheForValue`: a write evicts the cached finds it may change (one holding the row, or
/// one whose condition may match it), keeps the others, and clears the finds of a view of the entity.
#[test]
fn a_write_evicts_only_the_cached_finds_it_may_change() {
    let facade = testing::open();
    let make = |id: &str, kind: &str| {
        let mut value = facade.make_value("moqui.basic.Enumeration").unwrap();
        value.set("enumId", Value::string(id));
        value.set("enumTypeId", Value::string(kind));
        value
    };
    make("E1", "A").create(&facade).unwrap();
    make("E2", "B").create(&facade).unwrap();
    let cached = |kind: &str| {
        facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumTypeId", Value::string(kind))
            .use_cache(true)
            .list()
            .unwrap()
            .len()
    };
    let entries = || {
        super::list_cache_infos(&facade)
            .into_iter()
            .find(|info| info.name == "entity.record.list.moqui.basic.Enumeration")
            .map_or(0, |info| info.size)
    };
    assert_eq!((cached("A"), cached("B")), (1, 1));
    assert_eq!(entries(), 2);
    // A new row of type B leaves the cached list of type A.
    make("E3", "B").create(&facade).unwrap();
    assert_eq!(entries(), 1);
    assert_eq!(cached("B"), 2);
    // An update of E1, which the list of type A holds, takes that list.
    let mut moved = make("E1", "C");
    moved.set("description", Value::string("moved"));
    moved.update(&facade).unwrap();
    assert_eq!(cached("A"), 0);
    // The cached finds of a view of the entity go when a member is written.
    let view_rows = || {
        facade
            .find("moqui.basic.EnumerationAndType")
            .unwrap()
            .eq("enumTypeId", Value::string("B"))
            .use_cache(true)
            .list()
            .unwrap()
            .len()
    };
    if facade
        .with_entity("moqui.basic.EnumerationAndType", |_, _, _| Ok(()))
        .is_ok()
    {
        let mut kind = facade.make_value("moqui.basic.EnumerationType").unwrap();
        kind.set("enumTypeId", Value::string("B"));
        kind.create(&facade).unwrap();
        assert_eq!(view_rows(), 2);
        make("E4", "B").create(&facade).unwrap();
        assert_eq!(view_rows(), 3);
    }
}

/// Java `shouldCache` and the list find: a find with no condition, a limit or offset, or the user's
/// authorization filters is read from the database.
#[test]
fn only_a_plain_conditioned_find_is_cached() {
    let meta = ReadMeta {
        full_name: "E".to_string(),
        is_view: false,
        cache: true,
        cache_never: false,
        pk_fields: Vec::new(),
    };
    let lookup = |condition: Option<Condition>, limit: Option<i64>, offset: Option<i64>, filtered: bool| FindLookup {
        entity_name: "E".to_string(),
        use_cache: None,
        condition,
        order: Vec::new(),
        limit,
        offset,
        filtered,
    };
    let some = || Some(Condition::equals("a", Value::string("1")));
    assert!(meta.caches(&lookup(some(), None, None, false)));
    assert!(!meta.caches(&lookup(None, None, None, false)));
    assert!(!meta.caches(&lookup(some(), Some(10), None, false)));
    assert!(!meta.caches(&lookup(some(), None, Some(0), false)));
    assert!(!meta.caches(&lookup(some(), None, None, true)));
}

#[test]
fn held_rows_answer_ignore_case_and_field_comparisons() {
    let fields = row(&[
        ("name", Value::string("Gamma")),
        ("low", Value::I32(1)),
        ("high", Value::I32(5)),
    ]);
    let same_case = Condition::ignore_case(Condition::equals("name", Value::string("gAMMA")));
    assert!(eval_condition(&same_case, &fields));
    let other = Condition::ignore_case(Condition::like("name", Value::string("GAM%")));
    assert!(eval_condition(&other, &fields));
    assert!(!eval_condition(
        &Condition::ignore_case(Condition::equals("name", Value::string("beta"))),
        &fields
    ));
    assert!(eval_condition(
        &Condition::compare_fields("low", CompareOp::Less, "high"),
        &fields
    ));
    assert!(!eval_condition(
        &Condition::compare_fields("low", CompareOp::GreaterEqual, "high"),
        &fields
    ));
    assert!(eval_condition(
        &Condition::compare_fields("low", CompareOp::NotEquals, "high"),
        &fields
    ));
}

#[test]
fn handled_is_a_write_gate() {
    assert_eq!(WriteGate::Handled, WriteGate::Handled);
    assert_ne!(WriteGate::Proceed, WriteGate::Handled);
}

const TEST: &str = "moqui.test.TestEntity";
const TYPE: &str = "moqui.basic.EnumerationType";

fn put(facade: &EntityFacade, entity: &str, key: &str, id: &str) {
    let mut value = facade.make_value(entity).unwrap();
    value.set(key, Value::string(id));
    value.create(facade).unwrap();
}

fn found(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> bool {
    facade
        .find(entity)
        .unwrap()
        .eq(key, Value::string(id))
        .one()
        .unwrap()
        .is_some()
}

/// `moqui.test.TestEntity` `EXTST1`, the row the find-cache features start from.
fn seed_test_row(facade: &EntityFacade) {
    let mut value = facade.make_value(TEST).unwrap();
    value.set("testId", Value::string("EXTST1"));
    value.set("testMedium", Value::string("Test Name"));
    value.set("testNumberInteger", Value::I64(4321));
    value.create(facade).unwrap();
}

fn set_test_medium(facade: &EntityFacade, medium: &str) {
    let mut value = facade.make_value(TEST).unwrap();
    value.set("testId", Value::string("EXTST1"));
    value.set("testMedium", Value::string(medium));
    value.update(facade).unwrap();
}

/// A cached `one()` of `EXTST1` by `field`, as `testMedium`.
fn cached_test_medium(facade: &EntityFacade, field: &str, value: Value) -> Option<String> {
    facade
        .find(TEST)
        .unwrap()
        .eq(field, value)
        .use_cache(true)
        .one()
        .unwrap()
        .and_then(|row| row.get("testMedium").and_then(Value::as_str).map(str::to_string))
}

fn run<T>(
    tx: &TransactionManager,
    attr: TransactionAttribute,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    tx.run(attr, None, body)
}

fn statements(tape: &Arc<Mutex<Vec<String>>>, needle: &str) -> usize {
    tape.lock().unwrap().iter().filter(|sql| sql.contains(needle)).count()
}

/// The overlay: an update of a row the condition did not load comes back whole (stored row plus the update), a
/// find by key sees the held write, and a view read writes the held rows first.
#[test]
fn held_updates_merge_onto_the_stored_row_and_a_view_read_flushes_them() {
    let facade = testing::open();
    let entity = "moqui.basic.Enumeration";
    for (id, kind) in [("E1", "A"), ("E2", "A")] {
        let mut value = facade.make_value(entity).unwrap();
        value.set("enumId", Value::string(id));
        value.set("enumTypeId", Value::string(kind));
        value.set("description", Value::string("stored"));
        value.create(&facade).unwrap();
    }
    let mut kind = facade.make_value("moqui.basic.EnumerationType").unwrap();
    kind.set("enumTypeId", Value::string("B"));
    kind.create(&facade).unwrap();
    let tx = facade.transactions_for(entity).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        let mut moved = facade.make_value(entity).unwrap();
        moved.set("enumId", Value::string("E1"));
        moved.set("enumTypeId", Value::string("B"));
        moved.update(&facade)?;
        let of_b = facade.find(entity)?.eq("enumTypeId", Value::string("B")).list()?;
        assert_eq!(of_b.len(), 1);
        assert_eq!(of_b.values()[0].get("description"), Some(&Value::string("stored")));
        let by_key = facade.find(entity)?.eq("enumId", Value::string("E1")).one()?.unwrap();
        assert_eq!(by_key.get("enumTypeId"), Some(&Value::string("B")));
        // A row no held write touches is read from the database by its key.
        let other = facade.find(entity)?.eq("enumId", Value::string("E2")).one()?.unwrap();
        assert_eq!(other.get("enumTypeId"), Some(&Value::string("A")));
        let view = facade
            .find("moqui.basic.EnumerationAndType")?
            .eq("enumTypeId", Value::string("B"))
            .list()?;
        assert_eq!(view.len(), 1);
        Ok(())
    })
    .unwrap();
}

/// Java's `TransactionCacheDb.nextSeq`: a sim takes ids from its own bank, not the production sequence.
#[test]
fn a_sim_takes_its_ids_from_its_own_bank() {
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), None);
    super::SIM.with(|slot| {
        slot.borrow_mut().push(super::SimSession::new("sim_test".to_string()));
    });
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(900_000_001));
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(900_000_002));
    super::SIM.with(|slot| slot.borrow_mut().clear());
}

/// Java's `TransactionCacheDb.noteWrite`: a transaction that wrote to the sim and rolled back ends the sim; one
/// that committed, or a write with no transaction, does not.
#[test]
fn a_rolled_back_sim_write_poisons_the_sim() {
    let facade = testing::open();
    let tx = facade.transactions_for(TEST).unwrap();
    super::SIM.with(|slot| {
        slot.borrow_mut().push(super::SimSession::new("sim_poison".to_string()));
    });
    super::note_sim_write(&tx).unwrap();
    tx.begin(None).unwrap();
    super::note_sim_write(&tx).unwrap();
    super::note_sim_write(&tx).unwrap();
    assert_eq!(super::SIM.with(|slot| slot.borrow().last().unwrap().tracked.len()), 1);
    tx.commit().unwrap();
    assert!(super::sim_check_open().is_ok());
    assert_eq!(super::SIM.with(|slot| slot.borrow().last().unwrap().tracked.len()), 0);
    // A transaction that never wrote to the sim is not tracked.
    tx.begin(None).unwrap();
    tx.rollback().unwrap();
    assert!(super::sim_check_open().is_ok());
    tx.begin(None).unwrap();
    super::note_sim_write(&tx).unwrap();
    tx.rollback().unwrap();
    let err = super::sim_check_open().unwrap_err();
    assert!(err.to_string().contains("rolled back"), "{err}");
    assert!(super::note_sim_write(&tx).is_err());
    let err = super::query_sim(&facade, "sim_poison", "SELECT 1", &[]).unwrap_err();
    assert!(err.to_string().contains("terminated"), "{err}");
    assert!(super::sim_check_find(&facade, TEST).is_err());
    // The sim's own records on production are still read.
    assert!(super::sim_check_find(&facade, "moqui.agent.AgentRun").is_ok());
    super::SIM.with(|slot| slot.borrow_mut().clear());
}

#[test]
fn a_failed_inner_force_new_leaves_the_outer_held_rows() {
    let facade = testing::open();
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TEST, "testId", "X");
        let inner = run(&tx, TransactionAttribute::ForceNew, || -> Result<(), Error> {
            put(&facade, TYPE, "enumTypeId", "Y");
            Err(Error::new("inner fails"))
        });
        assert!(inner.is_err());
        assert!(found(&facade, TEST, "testId", "X"));
        Ok(())
    })
    .unwrap();
    assert!(found(&facade, TEST, "testId", "X"));
    assert!(!found(&facade, TYPE, "enumTypeId", "Y"));
}

#[test]
fn an_inner_force_new_commit_does_not_flush_the_outer_held_rows() {
    let facade = testing::open();
    let tx = facade.transactions_for(TEST).unwrap();
    let outer = run(&tx, TransactionAttribute::Cache, || -> Result<(), Error> {
        put(&facade, TEST, "testId", "X");
        run(&tx, TransactionAttribute::ForceCache, || {
            assert!(
                !found(&facade, TEST, "testId", "X"),
                "a suspended frame's rows are not read"
            );
            put(&facade, TYPE, "enumTypeId", "Y");
            Ok(())
        })?;
        assert!(found(&facade, TEST, "testId", "X"));
        Err(Error::new("outer fails"))
    });
    assert!(outer.is_err());
    assert!(!found(&facade, TEST, "testId", "X"));
    assert!(found(&facade, TYPE, "enumTypeId", "Y"));
}

/// On a database, the inner flush of the outer update would wait on the outer row lock forever.
#[test]
fn a_held_update_of_a_locked_row_stays_with_its_frame() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put(&facade, TEST, "testId", "R");
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        let mut locked = facade
            .find(TEST)?
            .eq("testId", Value::string("R"))
            .for_update(true)
            .one()?
            .expect("R");
        locked.set("testMedium", Value::string("held"));
        locked.update(&facade)?;
        run(&tx, TransactionAttribute::ForceNew, || {
            put(&facade, TYPE, "enumTypeId", "Y");
            Ok(())
        })?;
        assert_eq!(
            statements(&tape, "UPDATE \"test_entity\""),
            0,
            "{:?}",
            tape.lock().unwrap()
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(statements(&tape, "UPDATE \"test_entity\""), 1);
    let row = facade
        .find(TEST)
        .unwrap()
        .eq("testId", Value::string("R"))
        .one()
        .unwrap();
    assert_eq!(
        row.and_then(|row| row.get("testMedium").cloned()),
        Some(Value::string("held"))
    );
}

#[test]
fn the_cache_attribute_holds_until_commit_and_conf_false_turns_it_off() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::UseOrBegin, || {
        put(&facade, TEST, "testId", "Plain");
        assert_eq!(statements(&tape, "INSERT"), 1, "use-or-begin writes through");
        Ok(())
    })
    .unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TEST, "testId", "Held");
        assert_eq!(statements(&tape, "INSERT"), 1, "cache holds the insert");
        assert!(found(&facade, TEST, "testId", "Held"));
        Ok(())
    })
    .unwrap();
    assert_eq!(statements(&tape, "INSERT"), 2);
    assert!(found(&facade, TEST, "testId", "Held"));

    tx.set_use_transaction_cache(false);
    run(&tx, TransactionAttribute::Cache, || {
        assert!(!tx.transaction_cache_active());
        put(&facade, TEST, "testId", "Off");
        assert_eq!(
            statements(&tape, "INSERT"),
            3,
            "use-transaction-cache false writes through"
        );
        Ok(())
    })
    .unwrap();
    assert!(found(&facade, TEST, "testId", "Off"));
}

#[test]
fn flush_and_disable_writes_the_held_rows_once_and_stops_holding() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TEST, "testId", "Held");
        assert_eq!(statements(&tape, "INSERT"), 0, "cache holds the insert");
        super::flush_and_disable(&facade, &tx)?;
        assert!(!tx.transaction_cache_active());
        assert_eq!(statements(&tape, "INSERT"), 1, "the held insert is written now");
        put(&facade, TEST, "testId", "After");
        assert_eq!(statements(&tape, "INSERT"), 2, "a later write goes through");
        Ok(())
    })
    .unwrap();
    assert_eq!(statements(&tape, "INSERT"), 2, "the commit writes nothing twice");
    assert!(found(&facade, TEST, "testId", "Held"));
    assert!(found(&facade, TEST, "testId", "After"));
}

#[test]
fn a_cached_enumeration_is_shared_until_set() {
    let facade = testing::open();
    let mut made = facade.make_value("moqui.basic.Enumeration").unwrap();
    made.set("enumId", Value::string("Share1"));
    made.set("enumTypeId", Value::string("ShareType"));
    made.create(&facade).unwrap();
    let load = || {
        facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumId", Value::string("Share1"))
            .use_cache(true)
            .one()
            .unwrap()
            .unwrap()
    };
    let mut first = load();
    let second = load();
    assert!(first.shares_fields(&second), "a cache hit shares the stored row");
    first.set("description", Value::string("changed"));
    assert!(!first.shares_fields(&second), "set copies the row");
    assert!(second.get("description").is_none_or(Value::is_null));
    assert_eq!(first.get("description").and_then(Value::as_str), Some("changed"));
    let third = load();
    assert!(second.shares_fields(&third), "the cache keeps the unmodified row");
    assert!(!first.shares_fields(&third));
}

#[test]
fn find_keys_keep_sub_millisecond_timestamps_and_value_types_apart() {
    let key = |value: Value| {
        super::owned_lookup_key(&FindLookup {
            entity_name: "moqui.test.TestEntity".to_string(),
            use_cache: Some(true),
            condition: Some(Condition::equals("testDateTime", value)),
            order: Vec::new(),
            limit: None,
            offset: None,
            filtered: false,
        })
    };
    let stamp = |micros: i64| Value::Timestamp(jiff::Timestamp::from_microsecond(micros).unwrap());
    assert_ne!(key(stamp(1_700_000_000_000_001)), key(stamp(1_700_000_000_000_002)));
    assert_eq!(key(stamp(1_700_000_000_000_001)), key(stamp(1_700_000_000_000_001)));
    assert_ne!(key(Value::string("1")), key(Value::I64(1)));
}

#[test]
fn an_and_in_either_order_is_one_cache_entry() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let mut made = facade.make_value("moqui.basic.Enumeration").unwrap();
    made.set("enumId", Value::string("And1"));
    made.set("enumTypeId", Value::string("AndType"));
    made.set("description", Value::string("hello"));
    made.create(&facade).unwrap();
    let find = |first_field: &str, first: &str, second_field: &str, second: &str| {
        facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .condition(Condition::and(vec![
                Condition::equals(first_field, Value::string(first)),
                Condition::equals(second_field, Value::string(second)),
            ]))
            .use_cache(true)
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(find("enumId", "And1", "enumTypeId", "AndType"), 1);
    let selects = statements(&tape, "SELECT");
    assert_eq!(find("enumTypeId", "AndType", "enumId", "And1"), 1);
    assert_eq!(statements(&tape, "SELECT"), selects, "the reversed and hits the cache");
}

#[test]
fn a_cached_find_of_an_entity_the_frame_wrote_is_not_kept_after_rollback() {
    let facade = testing::open();
    let tx = facade.transactions_for(TYPE).unwrap();
    assert!(tx.begin(None).unwrap());
    put(&facade, TYPE, "enumTypeId", "PHANTOM");
    assert!(
        found(&facade, TYPE, "enumTypeId", "PHANTOM"),
        "the frame reads its own write"
    );
    tx.rollback().unwrap();
    assert!(
        !found(&facade, TYPE, "enumTypeId", "PHANTOM"),
        "the rolled-back row was not cached"
    );
    assert!(super::TX_WRITTEN.with(|slot| slot.borrow().is_empty()));
}

#[test]
fn a_commit_clears_rows_another_thread_cached_before_it() {
    let facade = testing::open();
    assert!(!found(&facade, TYPE, "enumTypeId", "NEW"), "caches the empty result");
    let tx = facade.transactions_for(TYPE).unwrap();
    assert!(tx.begin(None).unwrap());
    put(&facade, TYPE, "enumTypeId", "NEW");
    let reader = facade.clone();
    let before_commit = std::thread::spawn(move || found(&reader, TYPE, "enumTypeId", "NEW"))
        .join()
        .unwrap();
    assert!(
        !before_commit,
        "another thread reads the committed rows and caches them"
    );
    assert!(
        found(&facade, TYPE, "enumTypeId", "NEW"),
        "the writing frame does not read the other thread's cached rows"
    );
    tx.commit().unwrap();
    assert!(
        found(&facade, TYPE, "enumTypeId", "NEW"),
        "the commit cleared the cache"
    );
    assert!(super::TX_WRITTEN.with(|slot| slot.borrow().is_empty()));
}

/// The writes of each open frame kept for `full_name`: `None` when the frame keeps none (clears it whole), else
/// how many.
fn kept_writes(full_name: &str) -> Vec<Option<usize>> {
    super::TX_WRITES.with(|slot| {
        slot.borrow()
            .values()
            .filter_map(|entities| entities.get(full_name).map(|writes| writes.as_ref().map(Vec::len)))
            .collect()
    })
}

fn found_by(facade: &EntityFacade, field: &str, value: &str) -> usize {
    facade
        .find(TYPE)
        .unwrap()
        .eq(field, Value::string(value))
        .use_cache(true)
        .list()
        .unwrap()
        .len()
}

/// A frame's update of an entity with a cached find keeps the values written, and the commit evicts a find by a
/// field outside the primary key that another thread cached before it.
#[test]
fn an_update_evicts_a_find_by_another_field_at_commit() {
    let facade = testing::open();
    let mut row = facade.make_value(TYPE).unwrap();
    row.set("enumTypeId", Value::string("UPD"));
    row.set("description", Value::string("before"));
    row.create(&facade).unwrap();
    assert_eq!(found_by(&facade, "description", "after"), 0, "caches the empty result");
    let tx = facade.transactions_for(TYPE).unwrap();
    assert!(tx.begin(None).unwrap());
    row.set("description", Value::string("after"));
    row.update(&facade).unwrap();
    assert_eq!(kept_writes(TYPE), vec![Some(1)]);
    let reader = facade.clone();
    let before_commit = std::thread::spawn(move || found_by(&reader, "description", "after"))
        .join()
        .unwrap();
    assert_eq!(before_commit, 0, "another thread caches the committed rows");
    tx.commit().unwrap();
    assert_eq!(
        found_by(&facade, "description", "after"),
        1,
        "the commit evicted the find"
    );
}

/// A frame's write of an entity with no cached find or count keeps no values; a find another thread caches during
/// the frame, by a field the write set, is still cleared at the commit.
#[test]
fn a_find_cached_during_the_frame_is_cleared_at_commit_without_kept_values() {
    let facade = testing::open();
    let tx = facade.transactions_for(TYPE).unwrap();
    assert!(tx.begin(None).unwrap());
    let mut row = facade.make_value(TYPE).unwrap();
    row.set("enumTypeId", Value::string("LATE"));
    row.set("description", Value::string("late"));
    row.create(&facade).unwrap();
    assert_eq!(kept_writes(TYPE), vec![None], "nothing cached, nothing kept");
    let reader = facade.clone();
    let before_commit = std::thread::spawn(move || found_by(&reader, "description", "late"))
        .join()
        .unwrap();
    assert_eq!(before_commit, 0, "another thread caches the committed rows");
    tx.commit().unwrap();
    assert_eq!(
        found_by(&facade, "description", "late"),
        1,
        "the commit cleared the entity's caches"
    );
    assert!(kept_writes(TYPE).is_empty());
}

/// Java's `auto cache clear for one by primary key`: a cached `one()` by primary key, then an update, then the
/// same `one()` returns the new field.
#[test]
fn a_cached_one_by_primary_key_returns_the_updated_field() {
    let facade = testing::open();
    seed_test_row(&facade);
    assert_eq!(
        cached_test_medium(&facade, "testId", Value::string("EXTST1")).as_deref(),
        Some("Test Name")
    );
    set_test_medium(&facade, "Test Name 3");
    assert_eq!(
        cached_test_medium(&facade, "testId", Value::string("EXTST1")).as_deref(),
        Some("Test Name 3")
    );
}

/// Java's `auto cache clear for one by non-primary key`: a cached `one()` by a non-primary-key field, then an
/// update, then the same `one()` returns the new field. The Groovy feature also conditions on `testDateTime`.
#[test]
fn a_cached_one_by_a_non_primary_key_returns_the_updated_field() {
    let facade = testing::open();
    seed_test_row(&facade);
    assert_eq!(
        cached_test_medium(&facade, "testNumberInteger", Value::I64(4321)).as_deref(),
        Some("Test Name")
    );
    set_test_medium(&facade, "Test Name 4");
    assert_eq!(
        cached_test_medium(&facade, "testNumberInteger", Value::I64(4321)).as_deref(),
        Some("Test Name 4")
    );
}

/// Java's `auto cache clear for one by non-pk and initially no result`: a cached `one()` that found nothing is a
/// hit after the update that makes the row match.
#[test]
fn a_cached_one_that_missed_hits_after_the_update_that_matches() {
    let facade = testing::open();
    seed_test_row(&facade);
    assert_eq!(
        cached_test_medium(&facade, "testMedium", Value::string("Test Name 5")).as_deref(),
        None
    );
    set_test_medium(&facade, "Test Name 5");
    assert_eq!(
        cached_test_medium(&facade, "testMedium", Value::string("Test Name 5")).as_deref(),
        Some("Test Name 5")
    );
}

/// Java's `auto cache clear for count by is not null after update`: a cached `count()` of `is-not-null` goes up
/// by one after a value is set, and back after it is cleared.
#[test]
fn a_cached_count_of_non_null_goes_up_by_one_and_back_when_cleared() {
    let facade = testing::open();
    let entity = "moqui.basic.Enumeration";
    let mut row = facade.make_value(entity).unwrap();
    row.set("enumId", Value::string("DST_PURCHASED_DATA"));
    row.set("enumTypeId", Value::string("DataSourceType"));
    row.create(&facade).unwrap();
    let count = || {
        facade
            .find(entity)
            .unwrap()
            .condition(Condition::is_not_null("enumCode"))
            .use_cache(true)
            .count()
            .unwrap()
    };
    let before = count();
    row.set("enumCode", Value::string("TEST"));
    row.update(&facade).unwrap();
    let after = count();
    row.set("enumCode", Value::Null);
    row.update(&facade).unwrap();
    let reset = count();
    assert_eq!(before + 1, after);
    assert_eq!(reset, before);
}

/// Java's `no cache with for update` checks `isMutable()`, because a cached value throws on `set`. This port
/// copies on write (`a_cached_enumeration_is_shared_until_set`), so the assertion is the cache bypass: `for_update`
/// returns the stored row, not the one the cache still holds.
#[test]
fn a_for_update_find_returns_the_stored_row_not_the_cached_one() {
    let facade = testing::open();
    let mut made = facade.make_value(TEST).unwrap();
    made.set("testId", Value::string("EXTST1"));
    made.set("testMedium", Value::string("cached"));
    made.create(&facade).unwrap();
    assert_eq!(
        cached_test_medium(&facade, "testId", Value::string("EXTST1")).as_deref(),
        Some("cached")
    );
    let sql = facade
        .with_entity(TEST, |entity, dialect, _| {
            let table = moqui_db::sql::physical_name(dialect, &entity.table_name);
            let medium = moqui_db::sql::physical_column_name(dialect, &entity.field("testMedium").unwrap().column_name);
            let id = moqui_db::sql::physical_column_name(dialect, &entity.field("testId").unwrap().column_name);
            Ok(format!("UPDATE \"{table}\" SET \"{medium}\" = ? WHERE \"{id}\" = ?"))
        })
        .unwrap();
    let changed = facade
        .transactions_for(TEST)
        .unwrap()
        .with_connection(|conn| conn.execute(&sql, &[Value::string("stored"), Value::string("EXTST1")]))
        .unwrap();
    assert_eq!(changed, 1, "the stored row changed under the cache");
    assert_eq!(
        cached_test_medium(&facade, "testId", Value::string("EXTST1")).as_deref(),
        Some("cached"),
        "the cache still holds the seeded row"
    );
    assert!(facade.begin(TEST, None).unwrap());
    let locked = facade
        .find(TEST)
        .unwrap()
        .eq("testId", Value::string("EXTST1"))
        .for_update(true)
        .one()
        .unwrap()
        .expect("the stored row");
    assert_eq!(locked.get("testMedium").and_then(Value::as_str), Some("stored"));
    facade.rollback(TEST).unwrap();
    assert_eq!(
        cached_test_medium(&facade, "testId", Value::string("EXTST1")).as_deref(),
        Some("cached"),
        "for-update did not replace the cached row"
    );
}

/// Java's `auto cache clear for view one after update of member` uses `GeoAndType`. This module already reads
/// `EnumerationAndType`: update a member and the cached view `one()` shows the new description, then the reset.
#[test]
fn a_cached_view_one_shows_the_member_update_and_then_the_reset() {
    let facade = testing::open();
    let mut kind = facade.make_value(TYPE).unwrap();
    kind.set("enumTypeId", Value::string("GEOT_COUNTRY"));
    kind.set("description", Value::string("Country"));
    kind.create(&facade).unwrap();
    let mut item = facade.make_value("moqui.basic.Enumeration").unwrap();
    item.set("enumId", Value::string("USA"));
    item.set("enumTypeId", Value::string("GEOT_COUNTRY"));
    item.create(&facade).unwrap();
    let view = "moqui.basic.EnumerationAndType";
    let type_description = || {
        facade
            .find(view)
            .unwrap()
            .eq("enumId", Value::string("USA"))
            .use_cache(true)
            .one()
            .unwrap()
            .expect("the view row")
            .get("typeDescription")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    assert_eq!(type_description().as_deref(), Some("Country"));
    kind.set("description", Value::string("Country2"));
    kind.update(&facade).unwrap();
    assert_eq!(type_description().as_deref(), Some("Country2"));
    kind.set("description", Value::string("Country"));
    kind.update(&facade).unwrap();
    assert_eq!(type_description().as_deref(), Some("Country"));
}

/// Java's `every moqui.llm entity is classified as production-write, so a new one forces a decision`. This port
/// does not treat every `moqui.llm` name as production-write. A defined `moqui.llm` or `moqui.agent` entity is in
/// `SIM_PRODUCTION_ENTITIES` or named here as absent, so adding one means editing this test.
#[test]
fn a_new_llm_or_agent_entity_is_in_the_sim_list_or_named_absent() {
    /// Defined under `moqui.llm` or `moqui.agent` and not written to production during a sim.
    const ABSENT: &[&str] = &[
        "moqui.agent.AgentArtifact",
        "moqui.agent.AgentArtifactPart",
        "moqui.agent.AgentAuditLog",
        "moqui.agent.AgentDefinition",
        "moqui.agent.AgentHandoff",
        "moqui.agent.AgentInput",
        "moqui.agent.AgentLesson",
        "moqui.agent.AgentMcpServer",
        "moqui.agent.AgentMcpServerTool",
        "moqui.agent.AgentMcpUserToken",
        "moqui.agent.AgentMemory",
        "moqui.agent.AgentPushConfig",
        "moqui.agent.AgentPushDelivery",
        "moqui.agent.AgentQuestion",
        "moqui.agent.AgentRemoteA2a",
        "moqui.agent.AgentRemoteTask",
        "moqui.agent.AgentSkill",
        "moqui.agent.AgentSkillPin",
        "moqui.agent.AgentSkillUse",
        "moqui.agent.AgentSuggestedPrompt",
        "moqui.agent.AgentToolExposure",
        "moqui.agent.AgentToolPolicy",
        "moqui.agent.AgentTrigger",
        "moqui.agent.AgentTriggerEvent",
        "moqui.llm.LlmApiClient",
        "moqui.llm.LlmEndpoint",
        "moqui.llm.LlmFeatureBinding",
        "moqui.llm.LlmModel",
        "moqui.llm.LlmModelAlias",
        "moqui.llm.LlmModelCapability",
        "moqui.llm.LlmQuota",
        "moqui.llm.LlmUsageSummary",
    ];
    let facade = testing::open();
    let defined: Vec<String> = crate::entity::info::all_entity_names(&facade, "")
        .into_iter()
        .filter(|name| name.starts_with("moqui.llm.") || name.starts_with("moqui.agent."))
        .collect();
    assert!(
        !defined.is_empty(),
        "the model defines moqui.llm and moqui.agent entities"
    );
    let unclassified: Vec<&str> = defined
        .iter()
        .map(String::as_str)
        .filter(|name| {
            let production = super::SIM_PRODUCTION_ENTITIES.contains(name);
            let absent = ABSENT.contains(name);
            production == absent
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "put each in SIM_PRODUCTION_ENTITIES or in ABSENT, not both and not neither: {unclassified:?}"
    );
    let is_defined = |name: &str| defined.iter().any(|got| got == name);
    let stale: Vec<&str> = ABSENT.iter().copied().filter(|name| !is_defined(name)).collect();
    assert!(stale.is_empty(), "named absent but not defined: {stale:?}");
    let unknown: Vec<&str> = super::SIM_PRODUCTION_ENTITIES
        .iter()
        .copied()
        .filter(|name| name.starts_with("moqui.llm.") || name.starts_with("moqui.agent."))
        .filter(|name| !is_defined(name))
        .collect();
    assert!(
        unknown.is_empty(),
        "in SIM_PRODUCTION_ENTITIES but not defined: {unknown:?}"
    );
    assert!(!super::SIM_PRODUCTION_ENTITIES.contains(&"moqui.test.TestEntity"));
}
