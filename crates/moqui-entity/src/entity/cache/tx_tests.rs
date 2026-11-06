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
use crate::transaction::TransactionAttribute;

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
                assert_eq!(enum_reads(&tape) - before, 1, "a repeated find reads only its list");
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
/// after a held delete does not bring the row back.
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
        // An update after the delete writes nothing on the database, so the row stays gone here too.
        set_enum(&facade, "KEPT", "description", "again")?;
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
    assert_eq!(kinds, ["UPDATE", "DELETE", "UPDATE", "INSERT"], "{flushed:?}");
    assert!(!found(&facade, ENUM, "enumId", "BRIEF"));
    assert!(!found(&facade, ENUM, "enumId", "KEPT"));
    assert!(found(&facade, ENUM, "enumId", "LATER"));
}
