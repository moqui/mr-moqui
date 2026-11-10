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
        one_keys: Vec::new(),
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

pub(super) const TEST: &str = "moqui.test.TestEntity";
pub(super) const TYPE: &str = "moqui.basic.EnumerationType";

pub(super) fn put(facade: &EntityFacade, entity: &str, key: &str, id: &str) {
    let mut value = facade.make_value(entity).unwrap();
    value.set(key, Value::string(id));
    value.create(facade).unwrap();
}

pub(super) fn found(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> bool {
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

pub(super) fn run<T>(
    tx: &TransactionManager,
    attr: TransactionAttribute,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    tx.run(attr, None, body)
}

pub(super) fn statements(tape: &Arc<Mutex<Vec<String>>>, needle: &str) -> usize {
    tape.lock().unwrap().iter().filter(|sql| sql.contains(needle)).count()
}

/// Java's `TransactionCacheDb.nextSeq`: a sim takes ids from its own bank, not the production sequence. They count
/// down from 0 for each sequence name, so none can be an id a production sequence has given out.
#[test]
fn a_sim_takes_its_ids_from_its_own_bank() {
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), None);
    super::SIM.with(|slot| {
        slot.borrow_mut().push(super::SimSession::new("sim_test".to_string()));
    });
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(-1));
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(-2));
    assert_eq!(super::sim_next_seq("moqui.test.OtherEntity"), Some(-1));
    assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(-3));
    // the entities a sim leaves on production take production ids
    assert_eq!(super::sim_next_seq("moqui.llm.LlmCall"), None);
    assert_eq!(super::sim_next_seq("moqui.agent.AgentRun"), None);
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

/// The lookup `found` makes of `TYPE` by `id`.
fn type_lookup(id: &str) -> FindLookup {
    FindLookup {
        entity_name: TYPE.to_string(),
        use_cache: None,
        condition: Some(Condition::equals("enumTypeId", Value::string(id))),
        order: Vec::new(),
        limit: None,
        offset: None,
        filtered: false,
    }
}

/// The rows of `TYPE` by `id`, read from the database.
fn type_rows(facade: &EntityFacade, id: &str) -> crate::entity::EntityList {
    facade
        .find(TYPE)
        .unwrap()
        .eq("enumTypeId", Value::string(id))
        .use_cache(false)
        .list()
        .unwrap()
}

/// HL-023: a find whose query ran before another thread's autocommit write, and whose store comes after that write's
/// eviction, is not stored; the next cached read is fresh. With no write between, the store is kept.
#[test]
fn a_find_read_before_an_autocommit_write_is_not_stored_after_its_eviction() {
    let facade = testing::open();
    let lookup = type_lookup("FENCED");
    let fence = super::store_fence(&facade, &lookup).expect("the entity's finds are cached");
    let rows = type_rows(&facade, "FENCED");
    assert!(rows.is_empty());
    let writer = facade.clone();
    std::thread::spawn(move || put(&writer, TYPE, "enumTypeId", "FENCED"))
        .join()
        .unwrap();
    assert!(
        !super::after_find(&facade, &lookup, &fence, &rows).unwrap(),
        "the rows read before the write are refused"
    );
    assert!(
        super::before_find(&facade, &lookup).unwrap().is_none(),
        "nothing stale is cached"
    );
    assert!(found(&facade, TYPE, "enumTypeId", "FENCED"), "the next read is fresh");
    assert!(
        super::before_find(&facade, &lookup)
            .unwrap()
            .is_some_and(|rows| rows.len() == 1),
        "and is cached"
    );
    // The same for a count.
    let count_fence = super::store_fence(&facade, &type_lookup("COUNTED")).unwrap();
    put(&facade, TYPE, "enumTypeId", "COUNTED");
    assert!(!super::after_count(&facade, &type_lookup("COUNTED"), &count_fence, 0).unwrap());
    // With no write since its fence, a find is stored.
    let quiet = type_lookup("QUIET");
    let fence = super::store_fence(&facade, &quiet).unwrap();
    assert!(super::after_find(&facade, &quiet, &fence, &type_rows(&facade, "QUIET")).unwrap());
}

/// HL-023: the query straddles another thread's commit. The writer's frame wrote (and evicted) before the reader took
/// its fence, and the reader's query saw the rows as they were before the commit (as a database at READ COMMITTED
/// shows them); the store lands after the commit's eviction and is refused.
#[test]
fn a_find_whose_query_straddles_a_commit_is_not_stored() {
    use std::sync::mpsc;
    let facade = testing::open();
    let lookup = type_lookup("STRADDLE");
    let before = type_rows(&facade, "STRADDLE");
    assert!(before.is_empty());
    let (wrote_tx, wrote_rx) = mpsc::channel::<()>();
    let (commit_tx, commit_rx) = mpsc::channel::<()>();
    let writer = facade.clone();
    let handle = std::thread::spawn(move || {
        let tx = writer.transactions_for(TYPE).unwrap();
        assert!(tx.begin(None).unwrap());
        put(&writer, TYPE, "enumTypeId", "STRADDLE");
        wrote_tx.send(()).unwrap();
        commit_rx.recv().unwrap();
        tx.commit().unwrap();
    });
    wrote_rx.recv().unwrap();
    let fence = super::store_fence(&facade, &lookup).expect("this thread did not write");
    commit_tx.send(()).unwrap();
    handle.join().unwrap();
    assert!(
        !super::after_find(&facade, &lookup, &fence, &before).unwrap(),
        "the rows from before the commit are refused"
    );
    assert!(found(&facade, TYPE, "enumTypeId", "STRADDLE"), "the next read is fresh");
}

/// HL-023: a remote invalidation and a whole-cache clear move the fence as a local write does.
#[test]
fn a_remote_invalidation_and_a_whole_clear_refuse_an_earlier_find() {
    let facade = testing::open();
    let lookup = type_lookup("REMOTE");
    let rows = type_rows(&facade, "REMOTE");
    let fence = super::store_fence(&facade, &lookup).unwrap();
    super::clear_one_cache(&facade, TYPE, None).unwrap();
    assert!(!super::after_find(&facade, &lookup, &fence, &rows).unwrap());
    let fence = super::store_fence(&facade, &lookup).unwrap();
    let key = row(&[("enumTypeId", Value::string("OTHER"))]);
    super::clear_one_cache(&facade, TYPE, Some(&key)).unwrap();
    assert!(
        !super::after_find(&facade, &lookup, &fence, &rows).unwrap(),
        "a remote eviction by primary key moves the fence too"
    );
    let fence = super::store_fence(&facade, &lookup).unwrap();
    super::clear_all_list_caches(&facade).unwrap();
    assert!(!super::after_find(&facade, &lookup, &fence, &rows).unwrap());
}

/// HL-023: a find of a replica (`use_clone` with a clone group open) is not stored where a find of the primary would
/// read it.
#[test]
fn a_find_of_a_replica_is_not_stored_in_the_shared_cache() {
    let facade = testing::open_groups("transactional#clone1");
    let entity = "moqui.basic.Enumeration";
    let mut row = facade.make_value(entity).unwrap();
    row.set("enumId", Value::string("MAIN"));
    row.create(&facade).unwrap();
    let find = || {
        facade
            .find(entity)
            .unwrap()
            .eq("enumId", Value::string("MAIN"))
            .use_cache(true)
    };
    assert!(find().use_clone(true).one().unwrap().is_none(), "the clone has no row");
    assert!(find().one().unwrap().is_some(), "the primary's read is not the clone's");
}

/// HL-023: a frame that reads one snapshot does not store its finds; outside a transaction it does.
#[test]
fn a_snapshot_frame_does_not_store_its_finds() {
    let facade = testing::open_snapshot_reads();
    let lookup = type_lookup("SNAP");
    assert!(super::store_fence(&facade, &lookup).is_some());
    let tx = facade.transactions_for(TYPE).unwrap();
    assert!(tx.begin(None).unwrap());
    assert!(super::store_fence(&facade, &lookup).is_none());
    tx.commit().unwrap();
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

/// A cached count with no condition (of all rows) is kept, read once, and a create or a delete drops it.
#[test]
fn a_cached_count_of_all_rows_is_kept_until_a_write() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let count = || facade.find(ENUM).unwrap().use_cache(true).count().unwrap();
    let reads = || enum_reads(&tape);
    let before = count();
    let after_first = reads();
    assert_eq!(count(), before);
    assert_eq!(reads(), after_first, "the second count is the cached one");
    put_enum(&facade, "CNT1", "CA", "one");
    assert_eq!(count(), before + 1, "a create dropped it");
    delete_enum(&facade, "CNT1").unwrap();
    assert_eq!(count(), before, "a delete dropped it");
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
        "moqui.agent.AgentDataset",
        "moqui.agent.AgentDatasetItem",
        "moqui.agent.AgentDefinition",
        "moqui.agent.AgentExperiment",
        "moqui.agent.AgentExperimentResult",
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
        "moqui.agent.AgentScore",
        "moqui.agent.AgentScorer",
        "moqui.agent.AgentScorerAttach",
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

pub(super) const ENUM: &str = "moqui.basic.Enumeration";

pub(super) fn put_enum(facade: &EntityFacade, id: &str, kind: &str, description: &str) {
    let mut value = facade.make_value(ENUM).unwrap();
    value.set("enumId", Value::string(id));
    value.set("enumTypeId", Value::string(kind));
    value.set("description", Value::string(description));
    value.create(facade).unwrap();
}

pub(super) fn set_enum(facade: &EntityFacade, id: &str, field: &str, value: &str) -> Result<(), Error> {
    let mut row = facade.make_value(ENUM)?;
    row.set("enumId", Value::string(id));
    row.set(field, Value::string(value));
    row.update(facade)
}

pub(super) fn delete_enum(facade: &EntityFacade, id: &str) -> Result<(), Error> {
    let mut row = facade.make_value(ENUM)?;
    row.set("enumId", Value::string(id));
    row.delete(facade)
}

pub(super) fn enum_statements(tape: &Arc<Mutex<Vec<String>>>, reads: bool) -> Vec<String> {
    tape.lock()
        .unwrap()
        .iter()
        .filter(|sql| sql.starts_with("SELECT") == reads && sql.contains("\"enumeration\""))
        .cloned()
        .collect()
}

pub(super) fn enum_reads(tape: &Arc<Mutex<Vec<String>>>) -> usize {
    enum_statements(tape, true).len()
}

pub(super) fn enum_ids(rows: &crate::entity::EntityList) -> Vec<String> {
    rows.values()
        .iter()
        .map(|row| {
            row.get("enumId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// plans/High-Level-Review.md HL-002: a key of one type compares as a value, and only text against another type
/// compares as text.
#[test]
fn a_cached_rows_key_matches_the_written_one_across_types_only_as_text() {
    assert!(super::same_key(&Value::string("E1"), &Value::string("E1")));
    assert!(!super::same_key(&Value::string("E1"), &Value::string("E2")));
    assert!(super::same_key(&Value::I64(7), &Value::string("7")));
    assert!(super::same_key(&Value::string("7"), &Value::I32(7)));
    assert!(!super::same_key(&Value::I64(7), &Value::I64(8)));
}

/// plans/High-Level-Review.md HL-002, opt-in: the cost of the current eviction (one sweep of every cached list per
/// write, immediately and again at frame end) beside one sweep per entity that checks each cached row against a set of
/// the changed keys. The writes match no cached list, the case where selective eviction scans everything and removes
/// nothing. Run: `cargo test -p moqui-entity --lib eviction_sweep_cost -- --ignored --nocapture`.
#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn eviction_sweep_cost_per_write_and_per_entity() {
    use std::collections::HashSet;
    use std::time::Instant;

    println!("lists rows/list writes | commit-no-cache(ms) commit-cached(ms) | per-write: inspected  lock(ms) | per-entity: inspected  lock(ms)");
    for (lists, rows, writes) in [(20, 20, 10), (100, 20, 50), (200, 20, 200), (200, 50, 200)] {
        let facade = testing::open();
        for list in 0..lists {
            for row in 0..rows {
                put_enum(
                    &facade,
                    &format!("L{list:03}R{row:03}"),
                    &format!("T{list:03}"),
                    "stored",
                );
            }
        }
        for write in 0..writes {
            put_enum(&facade, &format!("W{write:04}"), "W", "stored");
        }
        let fill = |facade: &EntityFacade| {
            for list in 0..lists {
                let found = facade
                    .find(ENUM)
                    .unwrap()
                    .eq("enumTypeId", Value::string(format!("T{list:03}")))
                    .use_cache(true)
                    .list()
                    .unwrap();
                assert_eq!(found.len(), rows);
            }
        };
        let tx = facade.transactions_for(ENUM).unwrap();
        let update_all = |facade: &EntityFacade, text: &str| {
            let started = Instant::now();
            tx.run(TransactionAttribute::UseOrBegin, None, || {
                for write in 0..writes {
                    let mut row = facade.make_value(ENUM)?;
                    row.set("enumId", Value::string(format!("W{write:04}")));
                    row.set("enumTypeId", Value::string("W"));
                    row.set("description", Value::string(text));
                    row.update(facade)?;
                }
                Ok(())
            })
            .unwrap();
            started.elapsed()
        };
        let no_cache = update_all(&facade, "first");
        fill(&facade);
        let cached = update_all(&facade, "second");
        let lists_cache = facade
            .with_slot_read(super::OneCaches::default, |caches| caches.lists.get(ENUM).cloned())
            .unwrap();
        assert_eq!(lists_cache.len(), lists, "the writes matched no cached list");

        let values: Vec<crate::entity::FieldMap> = (0..writes)
            .map(|write| {
                row(&[
                    ("enumId", Value::string(format!("W{write:04}"))),
                    ("enumTypeId", Value::string("W")),
                    ("description", Value::string("third")),
                ])
            })
            .collect();
        let meta = super::read_meta(&facade, ENUM).unwrap();
        // Now: each write sweeps every cached list.
        let mut inspected = 0_usize;
        let mut per_write = std::time::Duration::ZERO;
        for values in &values {
            let pk = [("enumId", values.get("enumId").unwrap())];
            let started = Instant::now();
            lists_cache
                .remove_where(|_, cached| {
                    inspected += cached.rows.len();
                    super::list_affected(cached, "update", values, Some(&pk))
                })
                .unwrap();
            per_write += started.elapsed();
        }
        // One sweep: each cached row's key against the set, each list's condition against each write.
        let keys: HashSet<String> = values
            .iter()
            .filter_map(|values| super::pk_of(values, &meta.pk_fields))
            .collect();
        let mut keyed_inspected = 0_usize;
        let started = Instant::now();
        let mut buffer = String::new();
        lists_cache
            .remove_where(|_, cached| {
                keyed_inspected += cached.rows.len();
                let held = cached.rows.iter().any(|row| {
                    buffer.clear();
                    super::write_pk(row.fields(), &meta.pk_fields, &mut buffer) && keys.contains(buffer.as_str())
                });
                held || values
                    .iter()
                    .any(|values| super::may_match(cached.condition.as_ref(), values))
            })
            .unwrap();
        let per_entity = started.elapsed();
        let ms = |duration: std::time::Duration| duration.as_secs_f64() * 1000.0;
        println!(
            "{lists:>5} {rows:>9} {writes:>6} | {:>19.2} {:>17.2} | {:>20} {:>9.2} | {:>21} {:>9.2}",
            ms(no_cache),
            ms(cached),
            inspected,
            ms(per_write),
            keyed_inspected,
            ms(per_entity),
        );
    }
}

/// plans/High-Level-Review.md HL-002: a frame's writes of one entity are evicted again at its end in one sweep, and the
/// lists another thread cached while the frame was open are fresh after it: one held an updated row, one matches a
/// created row, one holds a deleted row. A list none of the writes touches stays.
#[test]
fn a_frames_writes_of_one_entity_make_one_frame_end_sweep() {
    let facade = testing::open();
    for (id, kind) in [("U1", "A"), ("U2", "A"), ("U3", "A"), ("D1", "D"), ("K1", "K")] {
        put_enum(&facade, id, kind, "stored");
    }
    let cached = |facade: &EntityFacade, kind: &str| -> Vec<String> {
        let rows = facade
            .find(ENUM)
            .unwrap()
            .eq("enumTypeId", Value::string(kind))
            .use_cache(true)
            .list()
            .unwrap();
        let mut found: Vec<String> = rows
            .values()
            .iter()
            .map(|row| {
                let text = |name: &str| row.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
                format!("{}:{}", text("enumId"), text("description"))
            })
            .collect();
        found.sort();
        found
    };
    // Cached before the frame, so its writes keep their values for the frame end.
    for kind in ["A", "C", "D", "K"] {
        let _ = cached(&facade, kind);
    }
    let tx = facade.transactions_for(ENUM).unwrap();
    assert!(tx.begin(None).unwrap());
    for id in ["U1", "U2", "U3"] {
        set_enum(&facade, id, "description", "changed").unwrap();
    }
    put_enum(&facade, "C1", "C", "created");
    delete_enum(&facade, "D1").unwrap();
    // Another thread reads the committed rows and caches them, after the writes' own evictions.
    let reader = facade.clone();
    std::thread::spawn(move || {
        for kind in ["A", "C", "D", "K"] {
            assert!(!cached(&reader, kind).is_empty() || kind == "C");
        }
    })
    .join()
    .unwrap();
    let sweeps = super::frame_sweeps();
    tx.commit().unwrap();
    assert_eq!(super::frame_sweeps() - sweeps, 1, "one sweep for the five writes");
    assert_eq!(cached(&facade, "A"), ["U1:changed", "U2:changed", "U3:changed"]);
    assert_eq!(cached(&facade, "C"), ["C1:created"]);
    assert!(cached(&facade, "D").is_empty());
    assert_eq!(cached(&facade, "K"), ["K1:stored"]);
}

#[test]
fn a_held_time_stamp_is_kept_at_the_precision_its_database_stores() {
    use super::stored_timestamp;
    let stamp = |nanos: i128| jiff::Timestamp::from_nanosecond(nanos).unwrap();
    // Postgres: the nearest microsecond, a half to even
    assert_eq!(
        stored_timestamp(stamp(1_000_123_456_789), 6, true),
        stamp(1_000_123_457_000)
    );
    assert_eq!(
        stored_timestamp(stamp(1_000_123_456_500), 6, true),
        stamp(1_000_123_456_000)
    );
    assert_eq!(
        stored_timestamp(stamp(1_000_123_457_500), 6, true),
        stamp(1_000_123_458_000)
    );
    // MySQL DATETIME(3): six digits cut, then half up to the millisecond
    assert_eq!(
        stored_timestamp(stamp(1_000_123_499_999), 3, false),
        stamp(1_000_123_000_000)
    );
    assert_eq!(
        stored_timestamp(stamp(1_000_123_500_999), 3, false),
        stamp(1_000_124_000_000)
    );
    assert_eq!(
        stored_timestamp(stamp(1_000_623_000_000), 0, false),
        stamp(1_001_000_000_000)
    );
}

#[test]
fn a_cached_find_answers_from_the_cache_alone_only_on_a_hit() {
    let facade = testing::open();
    let mut made = facade.make_value("moqui.basic.Enumeration").unwrap();
    made.set("enumId", Value::string("Probe1"));
    made.set("description", Value::string("probe"));
    made.create(&facade).unwrap();
    let find = facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("Probe1"))
        .use_cache(true);
    assert!(find.one_from_cache().unwrap().is_none(), "a miss runs the find");
    assert!(find.list_from_cache().unwrap().is_none());
    // the find stays as it was after a run, and its rows are now cached
    assert_eq!(
        find.one().unwrap().unwrap().get("description").and_then(Value::as_str),
        Some("probe")
    );
    let hit = find.one_from_cache().unwrap().expect("a hit").expect("the row");
    assert_eq!(hit.get("description").and_then(Value::as_str), Some("probe"));
    assert_eq!(find.list_from_cache().unwrap().expect("a hit").len(), 1);
    assert!(
        find.clone().distinct(true).list_from_cache().unwrap().is_none(),
        "a distinct find reads the database"
    );
    assert!(find.clone().use_cache(false).one_from_cache().unwrap().is_none());
    let more_than_the_key = facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("Probe1"))
        .eq("description", Value::string("probe"));
    assert!(
        more_than_the_key.use_cache(true).one_from_cache().unwrap().is_none(),
        "a find one that narrows runs"
    );
}
