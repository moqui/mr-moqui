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

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::super::tests::{
    delete_enum, enum_ids, enum_reads, enum_statements, found, put, put_enum, run, set_enum, statements, ENUM, TEST,
    TYPE,
};
use crate::entity::EntityFacade;
use crate::testing;
use crate::transaction::{CacheMode, TransactionAttribute};

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

/// plans/High-Level-Review.md HL-034: a find in a transaction-cache frame reads its list once, and the stored rows
/// under the frame's held updates once per frame, in one query, however many rows are held or loaded.
#[test]
fn held_updates_read_their_stored_rows_once_per_frame() {
    for (loaded, held) in [(3, 2), (40, 2), (3, 40), (40, 40)] {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = testing::open_with_tape(Arc::clone(&tape));
        for index in 0..loaded {
            put_enum(&facade, &format!("A{index:03}"), "A", "stored");
        }
        for index in 0..held {
            put_enum(&facade, &format!("C{index:03}"), "C", "stored");
        }
        let tx = facade.transactions_for(ENUM).unwrap();
        run(&tx, TransactionAttribute::Cache, || {
            // Held updates of rows a find of type A does not load, and one of a row it does.
            for index in 0..held {
                set_enum(&facade, &format!("C{index:03}"), "description", "held")?;
            }
            set_enum(&facade, "A000", "description", "held")?;
            let before = enum_reads(&tape);
            let of_a = facade.find(ENUM)?.eq("enumTypeId", Value::string("A")).list()?;
            assert_eq!(of_a.len(), loaded);
            assert_eq!(
                enum_reads(&tape) - before,
                2,
                "the list, then the stored rows of {held} unloaded updates in one query"
            );
            for _ in 0..3 {
                let before = enum_reads(&tape);
                let of_a = facade.find(ENUM)?.eq("enumTypeId", Value::string("A")).list()?;
                assert_eq!(of_a.len(), loaded);
                assert_eq!(
                    enum_reads(&tape) - before,
                    0,
                    "a repeated find is answered from the frame (D3)"
                );
            }
            // A find that loads none of the held rows starts each from the row the frame read before: A000 from the
            // first find's list, the C rows from the stored-row query.
            let before = enum_reads(&tape);
            let of_held = facade
                .find(ENUM)?
                .eq("description", Value::string("stored-elsewhere"))
                .list()?;
            assert!(of_held.is_empty());
            let of_held = facade.find(ENUM)?.eq("description", Value::string("held")).list()?;
            assert_eq!(of_held.len(), held + 1);
            assert!(of_held
                .values()
                .iter()
                .all(|row| row.get("enumTypeId").is_some_and(|kind| !kind.is_null())));
            assert_eq!(enum_reads(&tape) - before, 2, "two lists, and no held row read again");
            Ok(())
        })
        .unwrap();
    }
}

/// A paged find in a transaction-cache frame reads only as far as the page can reach, and its pages are the ones the
/// database gives after the commit: a held create, a delete and an update that moves a row out each shift them.
#[test]
fn a_paged_find_over_held_rows_reads_a_bounded_window() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    for index in 1..=30 {
        put_enum(&facade, &format!("E{index:02}"), "A", "stored");
    }
    let page = |facade: &EntityFacade, offset: i64| -> Result<Vec<String>, Error> {
        let rows = facade
            .find(ENUM)?
            .eq("enumTypeId", Value::string("A"))
            .order_by("enumId", false)
            .offset(offset)
            .limit(3)
            .list()?;
        Ok(enum_ids(&rows))
    };
    let offsets = [0, 3, 6, 24, 27];
    let tx = facade.transactions_for(ENUM).unwrap();
    let held_pages = run(&tx, TransactionAttribute::Cache, || {
        put_enum(&facade, "E00", "A", "held");
        delete_enum(&facade, "E02")?;
        set_enum(&facade, "E05", "enumTypeId", "B")?;
        set_enum(&facade, "E29", "description", "held")?;
        let mut pages = Vec::new();
        for offset in offsets {
            let before = enum_reads(&tape);
            pages.push(page(&facade, offset)?);
            let reads = enum_statements(&tape, true);
            assert!(
                reads[before].contains("LIMIT"),
                "the list is read with a limit: {:?}",
                &reads[before..]
            );
        }
        Ok(pages)
    })
    .unwrap();
    let committed: Vec<Vec<String>> = offsets.iter().map(|&offset| page(&facade, offset).unwrap()).collect();
    assert_eq!(held_pages, committed);
    assert_eq!(committed[0], ["E00", "E01", "E03"]);
    assert_eq!(committed[1], ["E04", "E06", "E07"]);
}

/// The held rows keep their order and their collapsing: a create and a delete of one row write nothing, and an update
/// after a held delete fails, as it would on the database (plans/TX-Cache.md G4), and does not bring the row back.
#[test]
fn held_rows_collapse_a_create_and_delete_and_stay_deleted() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put_enum(&facade, "KEPT", "A", "stored");
    let tx = facade.transactions_for(ENUM).unwrap();
    let before = enum_statements(&tape, false).len();
    run(&tx, TransactionAttribute::Cache, || {
        put_enum(&facade, "BRIEF", "A", "held");
        set_enum(&facade, "BRIEF", "description", "changed")?;
        delete_enum(&facade, "BRIEF")?;
        set_enum(&facade, "KEPT", "description", "held")?;
        delete_enum(&facade, "KEPT")?;
        // An update after the delete finds no row, as on the database.
        let err = set_enum(&facade, "KEPT", "description", "again").unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        let of_a = facade.find(ENUM)?.eq("enumTypeId", Value::string("A")).list()?;
        assert!(of_a.is_empty(), "{:?}", enum_ids(&of_a));
        assert!(facade
            .find(ENUM)?
            .eq("description", Value::string("again"))
            .list()?
            .is_empty());
        put_enum(&facade, "LATER", "A", "held");
        Ok(())
    })
    .unwrap();
    let flushed = enum_statements(&tape, false)[before..].to_vec();
    let kinds: Vec<&str> = flushed
        .iter()
        .map(|sql| sql.split_whitespace().next().unwrap_or_default())
        .collect();
    // A delete is not merged with the update before it (plans/TX-Cache.md D6).
    assert_eq!(kinds, ["UPDATE", "DELETE", "INSERT"], "{flushed:?}");
    assert!(!found(&facade, ENUM, "enumId", "BRIEF"));
    assert!(!found(&facade, ENUM, "enumId", "KEPT"));
    assert!(found(&facade, ENUM, "enumId", "LATER"));
}

/// plans/TX-Cache.md D1: a frame's held writes, its reads, its early flushes by cause and its commit batch are counted,
/// and go to this thread's totals when it ends.
#[test]
fn a_frame_counts_its_held_writes_reads_early_flushes_and_commit_batch() {
    let facade = testing::open();
    put(&facade, TYPE, "enumTypeId", "A");
    put_enum(&facade, "S1", "A", "stored");
    let tx = facade.transactions_for(ENUM).unwrap();
    let before = super::super::tx_cache_stats();
    let trips = crate::driver::thread_round_trips();
    run(&tx, TransactionAttribute::Cache, || {
        put_enum(&facade, "S2", "A", "held");
        set_enum(&facade, "S1", "description", "held")?;
        assert_eq!(facade.find(ENUM)?.eq("enumTypeId", Value::string("A")).list()?.len(), 2);
        put_enum(&facade, "S3", "A", "held");
        Ok(())
    })
    .unwrap();
    let stats = super::super::tx_cache_stats().since(&before);
    assert_eq!(stats.frames, 1);
    assert_eq!((stats.held_creates, stats.held_updates), (2, 1));
    assert!(stats.db_reads >= 1, "{stats:?}");
    assert_eq!(stats.early_flush_count(), 0);
    assert_eq!((stats.batch_statements, stats.batch_rows), (3, 3));
    assert_eq!(
        stats.batch_round_trips, 0,
        "the commit batch is answered by the commit's wait (plans/TX-Cache.md D6)"
    );
    assert!(crate::driver::thread_round_trips() > trips);

    let before = super::super::tx_cache_stats();
    run(&tx, TransactionAttribute::Cache, || {
        put_enum(&facade, "S4", "A", "held");
        let view = facade
            .find("moqui.basic.EnumerationAndType")?
            .eq("enumTypeId", Value::string("A"))
            .list()?;
        assert!(!view.is_empty());
        Ok(())
    })
    .unwrap();
    let stats = super::super::tx_cache_stats().since(&before);
    assert_eq!(stats.early_flushes_of(super::FlushCause::View), 1, "{stats:?}");
    assert_eq!(
        stats.batch_statements, 0,
        "the view read wrote the held row before the commit"
    );
}

/// Strict mode: an early flush marks the frame rollback-only, so the operation fails and writes nothing.
#[test]
fn strict_mode_fails_a_frame_that_flushes_early() {
    let facade = testing::open();
    let tx = facade.transactions_for(ENUM).unwrap();
    tx.set_tx_cache_strict(true);
    let held = run(&tx, TransactionAttribute::Cache, || {
        put_enum(&facade, "STRICT", "A", "held");
        facade
            .find("moqui.basic.EnumerationAndType")?
            .eq("enumTypeId", Value::string("A"))
            .list()?;
        Ok(())
    });
    tx.set_tx_cache_strict(false);
    let err = held.unwrap_err().to_string();
    assert!(err.contains("strict") || err.contains("rollback"), "{err}");
    assert!(!found(&facade, ENUM, "enumId", "STRICT"));
    // A frame that commits in one batch is not affected.
    tx.set_tx_cache_strict(true);
    run(&tx, TransactionAttribute::Cache, || {
        put_enum(&facade, "STRICT", "A", "held");
        Ok(())
    })
    .unwrap();
    tx.set_tx_cache_strict(false);
    assert!(found(&facade, ENUM, "enumId", "STRICT"));
}

/// Census mode: a frame begun without `cache` holds too.
#[test]
fn census_mode_holds_every_frame() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(TEST).unwrap();
    tx.set_tx_cache_everywhere(true);
    let held = run(&tx, TransactionAttribute::UseOrBegin, || {
        put(&facade, TEST, "testId", "Census");
        Ok(statements(&tape, "INSERT"))
    });
    tx.set_tx_cache_everywhere(false);
    assert_eq!(held.unwrap(), 0, "the insert is held");
    assert_eq!(statements(&tape, "INSERT"), 1);
}

/// `no-tx-cache` (`flush_and_disable`) turns the frame to read-through, and a `cache` call that joins it later does
/// not turn holding on again, as Java's read-only cache stays read-only.
#[test]
fn flush_and_disable_turns_the_frame_to_read_through() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TEST, "testId", "Held");
        super::flush_and_disable(&facade, &tx)?;
        assert_eq!(tx.cache_frame().map(|(_, mode)| mode), Some(CacheMode::ReadThrough));
        run(&tx, TransactionAttribute::Cache, || {
            put(&facade, TEST, "testId", "Joined");
            Ok(())
        })?;
        assert_eq!(statements(&tape, "INSERT"), 2, "the joined write went through");
        Ok(())
    })
    .unwrap();
}

fn set_test(facade: &EntityFacade, id: &str, field: &str, value: Value) -> Result<(), Error> {
    let mut row = facade.make_value(TEST)?;
    row.set("testId", Value::string(id));
    row.set(field, value);
    row.update(facade)
}

fn plain(row: &crate::entity::EntityValue, field: &str) -> String {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// G1: a for-update find (lock the counter row, then read it) sees the frame's held rows, a held update and a held
/// create, and still locks with `FOR UPDATE`; a for-update count counts them.
#[test]
fn a_for_update_find_sees_the_frames_held_rows() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put(&facade, TEST, "testId", "CTR");
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        set_test(&facade, "CTR", "testNumberInteger", Value::I64(5))?;
        let locked = facade
            .find(TEST)?
            .eq("testId", Value::string("CTR"))
            .for_update(true)
            .one()?;
        assert_eq!(locked.map(|row| plain(&row, "testNumberInteger")).as_deref(), Some("5"));
        put(&facade, TEST, "testId", "NEW");
        let created = facade
            .find(TEST)?
            .eq("testId", Value::string("NEW"))
            .for_update(true)
            .list()?;
        assert_eq!(created.len(), 1);
        let counted = facade
            .find(TEST)?
            .eq("testId", Value::string("NEW"))
            .for_update(true)
            .count()?;
        assert_eq!(counted, 1);
        Ok(())
    })
    .unwrap();
    assert!(statements(&tape, "FOR UPDATE") >= 1, "{:?}", tape.lock().unwrap());
}

/// G1: a select-field find sees a held create and gives the fields asked for.
#[test]
fn a_select_field_find_sees_the_frames_held_rows() {
    let facade = testing::open();
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        let mut row = facade.make_value(TEST)?;
        row.set("testId", Value::string("SEL"));
        row.set("testMedium", Value::string("held"));
        row.set("value", Value::string("other"));
        row.create(&facade)?;
        let found = facade
            .find(TEST)?
            .eq("testMedium", Value::string("held"))
            .select_field("testId")
            .select_field("testMedium")
            .list()?;
        assert_eq!(found.len(), 1);
        assert_eq!(plain(&found.values()[0], "testId"), "SEL");
        assert!(found.values()[0].get("value").is_none(), "only the fields asked for");
        Ok(())
    })
    .unwrap();
}

/// G1: a distinct find of an entity the frame holds rows of writes the held writes up to that entity's last (an early
/// flush, cause "selective find") and goes on holding the rest.
#[test]
fn a_distinct_find_writes_the_held_prefix_first() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(TEST).unwrap();
    let before = super::super::tx_cache_stats();
    run(&tx, TransactionAttribute::Cache, || {
        let mut row = facade.make_value(TEST)?;
        row.set("testId", Value::string("D1"));
        row.set("testMedium", Value::string("m"));
        row.create(&facade)?;
        put(&facade, TYPE, "enumTypeId", "AFTER");
        // What the find does before its query (the test driver cannot run a DISTINCT).
        let lookup = super::super::FindLookup {
            entity_name: TEST.to_string(),
            use_cache: None,
            condition: None,
            order: Vec::new(),
            limit: None,
            offset: None,
            filtered: false,
        };
        let selective = super::super::Selective {
            aggregate: true,
            ..Default::default()
        };
        assert!(super::super::before_selective_find(&facade, &lookup, &selective)?.is_none());
        assert_eq!(
            statements(&tape, "INSERT INTO \"test_entity\""),
            1,
            "the prefix is written"
        );
        assert_eq!(
            statements(&tape, "INSERT INTO \"enumeration_type\""),
            0,
            "the rest stays held"
        );
        Ok(())
    })
    .unwrap();
    let stats = super::super::tx_cache_stats().since(&before);
    assert_eq!(stats.early_flushes_of(super::FlushCause::SelectiveFind), 1);
    assert!(found(&facade, TYPE, "enumTypeId", "AFTER"));
}

/// G4: a held update of a row that is not there fails, at the commit when the frame knows nothing of the row, and at
/// once when the frame deleted it. A quiet one (`create_or_update`'s update probe) writes nothing, so the row is
/// created.
#[test]
fn a_held_update_of_a_missing_row_fails_and_create_or_update_creates_it() {
    let facade = testing::open();
    let tx = facade.transactions_for(TEST).unwrap();
    let err = run(&tx, TransactionAttribute::Cache, || {
        set_test(&facade, "NOPE", "testMedium", Value::string("x"))
    })
    .unwrap_err()
    .to_string();
    assert!(
        err.contains("not found") && err.contains("moqui.test.TestEntity"),
        "{err}"
    );
    run(&tx, TransactionAttribute::Cache, || {
        let mut row = facade.make_value(TEST)?;
        row.set("testId", Value::string("COU"));
        row.set("testMedium", Value::string("made"));
        row.create_or_update(&facade)?;
        let found = facade.find(TEST)?.eq("testId", Value::string("COU")).one()?;
        assert_eq!(found.map(|row| plain(&row, "testMedium")).as_deref(), Some("made"));
        Ok(())
    })
    .unwrap();
    assert!(found(&facade, TEST, "testId", "COU"));
}

/// G5: a held create of a row the database has fails at the first find that loads the row, not at the commit; a
/// create-if-missing of it writes nothing; and a flush failure names the write and its key.
#[test]
fn a_held_create_of_a_stored_row_fails_at_the_next_find() {
    let facade = testing::open();
    put(&facade, TEST, "testId", "DUP");
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TEST, "testId", "DUP");
        // A list find loads the database's row (a find by key of a row the frame created reads nothing, D2).
        let err = facade.find(TEST)?.list().unwrap_err().to_string();
        assert!(
            err.contains("already in the database") && err.contains("testId=DUP"),
            "{err}"
        );
        Err::<(), _>(Error::new("rolled back"))
    })
    .unwrap_err();
    run(&tx, TransactionAttribute::Cache, || {
        let mut row = facade.make_value(TEST)?;
        row.set("testId", Value::string("DUP"));
        assert!(!row.create_if_missing(&facade)?, "the row is there");
        Ok(())
    })
    .unwrap();
}

/// G7 (plans/TX-Cache.md D2): a find by key of a row the frame created runs no query.
#[test]
fn a_find_by_key_of_a_created_row_runs_no_query() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(TEST).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TEST, "testId", "MINE");
        let reads = statements(&tape, "SELECT");
        assert!(facade.find(TEST)?.eq("testId", Value::string("MINE")).one()?.is_some());
        assert_eq!(statements(&tape, "SELECT"), reads, "no query");
        Ok(())
    })
    .unwrap();
}

/// D6: the commit batch merges each row's writes (a create and its updates are one insert of the final values, updates
/// one update of their columns), keeps a parent before its child, and goes with the commit, one wait for it all.
#[test]
fn the_commit_batch_merges_each_rows_writes_and_waits_once() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put_enum(&facade, "UPD", "A", "stored");
    put(&facade, TYPE, "enumTypeId", "A");
    let tx = facade.transactions_for(ENUM).unwrap();
    let before = enum_statements(&tape, false).len();
    let before_stats = super::super::tx_cache_stats();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TYPE, "enumTypeId", "NEWTYPE");
        put_enum(&facade, "NEW", "NEWTYPE", "one");
        set_enum(&facade, "NEW", "description", "two")?;
        set_enum(&facade, "UPD", "description", "three")?;
        set_enum(&facade, "UPD", "enumCode", "C")?;
        Ok(())
    })
    .unwrap();
    let written = enum_statements(&tape, false)[before..].to_vec();
    let kinds: Vec<&str> = written
        .iter()
        .map(|sql| sql.split_whitespace().next().unwrap_or_default())
        .collect();
    assert_eq!(kinds, ["INSERT", "UPDATE"], "{written:?}");
    assert!(
        written[1].contains("\"description\"") && written[1].contains("\"enum_code\""),
        "{written:?}"
    );
    let stats = super::super::tx_cache_stats().since(&before_stats);
    assert_eq!((stats.batch_statements, stats.batch_rows), (3, 5));
    // the batch goes with the commit, whose wait answers it
    assert_eq!(stats.batch_round_trips, 0);
    let row = facade
        .find(ENUM)
        .unwrap()
        .eq("enumId", Value::string("NEW"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(row.get("description"), Some(&Value::string("two")));
    let row = facade
        .find(ENUM)
        .unwrap()
        .eq("enumId", Value::string("UPD"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(row.get("enumCode"), Some(&Value::string("C")));
}

/// D2: in a cache frame a row read by key is read once (a find the shared entity cache answers is left to it, so these
/// ask for none), and a row read in a list is not read again by key; in read-through
/// mode a write of the entity drops what the frame read of it, so the next find reads the database.
#[test]
fn a_cache_frame_reads_a_row_once_until_it_writes_through() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put_enum(&facade, "K1", "A", "one");
    put_enum(&facade, "K2", "A", "two");
    let tx = facade.transactions_for(ENUM).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        let by_key = |id: &str| -> Result<String, Error> {
            let row = facade
                .find(ENUM)?
                .eq("enumId", Value::string(id))
                .use_cache(false)
                .one()?
                .expect("row");
            Ok(plain(&row, "description"))
        };
        let before = enum_reads(&tape);
        assert_eq!(by_key("K1")?, "one");
        assert_eq!(by_key("K1")?, "one");
        assert_eq!(enum_reads(&tape) - before, 1, "read once by key");
        assert!(facade
            .find(ENUM)?
            .eq("enumId", Value::string("NONE"))
            .use_cache(false)
            .one()?
            .is_none());
        assert!(facade
            .find(ENUM)?
            .eq("enumId", Value::string("NONE"))
            .use_cache(false)
            .one()?
            .is_none());
        assert_eq!(enum_reads(&tape) - before, 2, "a missing row is kept as missing");
        assert_eq!(
            facade
                .find(ENUM)?
                .eq("enumTypeId", Value::string("A"))
                .use_cache(false)
                .list()?
                .len(),
            2
        );
        assert_eq!(by_key("K2")?, "two");
        assert_eq!(
            enum_reads(&tape) - before,
            3,
            "a row from a list is not read again by key"
        );
        // A held update is seen from memory.
        set_enum(&facade, "K2", "description", "held")?;
        assert_eq!(by_key("K2")?, "held");
        assert_eq!(enum_reads(&tape) - before, 3);
        super::flush_and_disable(&facade, &tx)?;
        let before = enum_reads(&tape);
        assert_eq!(by_key("K2")?, "held", "the held write is on the database now");
        assert_eq!(
            enum_reads(&tape) - before,
            1,
            "an early flush drops what the frame read"
        );
        assert_eq!(by_key("K2")?, "held");
        assert_eq!(enum_reads(&tape) - before, 1, "read-through keeps the rows it reads");
        set_enum(&facade, "K2", "description", "direct")?;
        assert_eq!(by_key("K2")?, "direct", "a write through drops the entity's rows");
        assert_eq!(enum_reads(&tape) - before, 2);
        Ok(())
    })
    .unwrap();
}

/// D3: a list find whose condition pins a type-one relationship to a row the frame created is answered from the held
/// creates, with no query.
#[test]
fn the_rows_of_a_created_parent_come_from_the_held_creates() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    let tx = facade.transactions_for(ENUM).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        put(&facade, TYPE, "enumTypeId", "NEWT");
        put_enum(&facade, "N1", "NEWT", "one");
        put_enum(&facade, "N2", "NEWT", "two");
        let before = enum_reads(&tape);
        let rows = facade.find(ENUM)?.eq("enumTypeId", Value::string("NEWT")).list()?;
        assert_eq!(enum_ids(&rows).len(), 2);
        assert_eq!(enum_reads(&tape) - before, 0, "no query");
        Ok(())
    })
    .unwrap();
}

/// D4: a for-update find always reads the database for its lock, and the rows it read are kept.
#[test]
fn a_for_update_find_locks_every_time_and_keeps_its_rows() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put_enum(&facade, "L1", "A", "one");
    put_enum(&facade, "L2", "A", "two");
    let tx = facade.transactions_for(ENUM).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        set_enum(&facade, "L2", "description", "held")?;
        let locks = statements(&tape, "FOR UPDATE");
        for _ in 0..2 {
            let rows = facade
                .find(ENUM)?
                .eq("enumTypeId", Value::string("A"))
                .for_update(true)
                .list()?;
            assert_eq!(rows.len(), 2);
        }
        assert_eq!(
            statements(&tape, "FOR UPDATE") - locks,
            2,
            "each lock goes to the database"
        );
        let before = enum_reads(&tape);
        assert!(facade.find(ENUM)?.eq("enumId", Value::string("L1")).one()?.is_some());
        assert_eq!(enum_reads(&tape) - before, 0, "the locked rows are kept");
        Ok(())
    })
    .unwrap();
}

/// A held update of a row a locking find of the frame read goes out with no check that it changed a row (no other
/// transaction can delete it), so the commit need not wait for it; an update of a row only read keeps the check.
#[test]
fn a_held_update_of_a_locked_row_is_not_checked_at_the_commit() {
    let facade = testing::open();
    put_enum(&facade, "K1", "A", "one");
    put_enum(&facade, "K2", "B", "two");
    let tx = facade.transactions_for(ENUM).unwrap();
    let before = testing::checked_deferred();
    run(&tx, TransactionAttribute::Cache, || {
        let locked = facade
            .find(ENUM)?
            .eq("enumTypeId", Value::string("A"))
            .for_update(true)
            .list()?;
        assert_eq!(locked.len(), 1);
        set_enum(&facade, "K1", "description", "locked")?;
        assert!(facade.find(ENUM)?.eq("enumId", Value::string("K2")).one()?.is_some());
        set_enum(&facade, "K2", "description", "read")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        testing::checked_deferred() - before,
        1,
        "only the row read without a lock is checked"
    );
    let one = facade
        .find(ENUM)
        .unwrap()
        .eq("enumId", Value::string("K1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(one.get("description"), Some(&Value::string("locked")));
}

/// D5: a view read writes the held rows first only when a held row of a member may be in its result.
#[test]
fn a_view_read_flushes_only_for_a_held_row_it_may_show() {
    let facade = testing::open();
    put(&facade, TYPE, "enumTypeId", "A");
    put(&facade, TYPE, "enumTypeId", "Z");
    put_enum(&facade, "V1", "A", "one");
    let tx = facade.transactions_for(ENUM).unwrap();
    let view = "moqui.basic.EnumerationAndType";
    let before = super::super::tx_cache_stats();
    run(&tx, TransactionAttribute::Cache, || {
        // The frame reads V1, so it knows its type before and after the update.
        assert!(facade
            .find(ENUM)?
            .eq("enumId", Value::string("V1"))
            .use_cache(false)
            .one()?
            .is_some());
        set_enum(&facade, "V1", "description", "held")?;
        facade.find(view)?.eq("enumTypeId", Value::string("Z")).list()?;
        Ok(())
    })
    .unwrap();
    let stats = super::super::tx_cache_stats().since(&before);
    assert_eq!(
        stats.early_flushes_of(super::FlushCause::View),
        0,
        "type Z cannot show V1"
    );
    let before = super::super::tx_cache_stats();
    run(&tx, TransactionAttribute::Cache, || {
        assert!(facade
            .find(ENUM)?
            .eq("enumId", Value::string("V1"))
            .use_cache(false)
            .one()?
            .is_some());
        set_enum(&facade, "V1", "description", "again")?;
        let rows = facade.find(view)?.eq("enumTypeId", Value::string("A")).list()?;
        assert_eq!(rows.values()[0].get("description"), Some(&Value::string("again")));
        Ok(())
    })
    .unwrap();
    let stats = super::super::tx_cache_stats().since(&before);
    assert_eq!(stats.early_flushes_of(super::FlushCause::View), 1, "type A shows V1");
}

/// A find by key with a key value of another type than the stored one (a number as text) finds the row, from the
/// database and from what the frame keeps, with a held update on it.
#[test]
fn a_find_by_key_of_another_value_type_finds_the_row() {
    let facade = testing::open();
    let event = "moqui.chat.ChatEvent";
    let mut row = facade.make_value(event).unwrap();
    row.set("chatRoomId", Value::string("R1"));
    row.set("eventSeq", Value::I64(3));
    row.set("eventTypeEnumId", Value::string("CetMessage"));
    row.create(&facade).unwrap();
    let tx = facade.transactions_for(event).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        let find = || {
            facade
                .find(event)?
                .eq("chatRoomId", Value::string("R1"))
                .eq("eventSeq", Value::string("3"))
                .one()
        };
        assert!(find()?.is_some(), "read by key");
        assert!(find()?.is_some(), "kept");
        let mut row = facade.make_value(event)?;
        row.set("chatRoomId", Value::string("R1"));
        row.set("eventSeq", Value::I64(3));
        row.set("eventTypeEnumId", Value::string("CetEdit"));
        row.update(&facade)?;
        let found = find()?.expect("held update");
        assert_eq!(found.get("eventTypeEnumId"), Some(&Value::string("CetEdit")));
        Ok(())
    })
    .unwrap();
}

/// D6: the commit batch does not move a write past another row's write that depends on it. A thread's references are
/// cleared and then its rows deleted, the parent first: each update stays before the deletes, as held.
#[test]
fn the_commit_batch_keeps_a_cleared_reference_before_the_delete_it_allows() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = testing::open_with_tape(Arc::clone(&tape));
    put_enum(&facade, "PARENT", "A", "p");
    put_enum(&facade, "CHILD", "A", "c");
    set_enum(&facade, "CHILD", "parentEnumId", "PARENT").unwrap();
    let tx = facade.transactions_for(ENUM).unwrap();
    let before = enum_statements(&tape, false).len();
    run(&tx, TransactionAttribute::Cache, || {
        set_enum(&facade, "PARENT", "description", "going")?;
        let mut child = facade.make_value(ENUM)?;
        child.set("enumId", Value::string("CHILD"));
        child.set("parentEnumId", Value::Null);
        child.update(&facade)?;
        delete_enum(&facade, "PARENT")?;
        delete_enum(&facade, "CHILD")?;
        Ok(())
    })
    .unwrap();
    let written: Vec<String> = enum_statements(&tape, false)[before..]
        .iter()
        .map(|sql| sql.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .collect();
    assert_eq!(
        written,
        [
            "UPDATE \"enumeration\"",
            "UPDATE \"enumeration\"",
            "DELETE FROM",
            "DELETE FROM"
        ]
    );
    let child_update = enum_statements(&tape, false)[before + 1].clone();
    assert!(child_update.contains("parent_enum_id"), "{child_update}");
}

/// A held row is judged in memory as the database would judge it: a number given as a float or as text in an `in` or
/// an equality matches the stored integer.
#[test]
fn a_held_row_matches_a_condition_of_another_number_type() {
    let facade = testing::open();
    let event = "moqui.chat.ChatEvent";
    let tx = facade.transactions_for(event).unwrap();
    run(&tx, TransactionAttribute::Cache, || {
        let mut row = facade.make_value(event)?;
        row.set("chatRoomId", Value::string("R2"));
        row.set("eventSeq", Value::I64(7));
        row.set("eventTypeEnumId", Value::string("CetMessage"));
        row.create(&facade)?;
        let found = facade
            .find(event)?
            .eq("chatRoomId", Value::string("R2"))
            .condition(moqui_db::sql::Condition::In("eventSeq".into(), vec![Value::F64(7.0)]))
            .list()?;
        assert_eq!(found.len(), 1);
        let found = facade
            .find(event)?
            .eq("chatRoomId", Value::string("R2"))
            .eq("eventTypeEnumId", Value::string("CetMessage"))
            .eq("eventSeq", Value::string("7"))
            .list()?;
        assert_eq!(found.len(), 1);
        Ok(())
    })
    .unwrap();
}
