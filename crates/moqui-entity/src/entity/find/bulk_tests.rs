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

//! plans/High-Level-Review.md HL-033: bulk writes page by primary key in bounded chunks, and a purge commits each one.

use std::sync::{Arc, Mutex};

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::entity::{EntityFacade, EntityFind};
use crate::transaction::TransactionAttribute;

const TEST: &str = "moqui.test.TestEntity";

/// `count` rows of medium `bulk` (`B00`, `B01`, ...) and one of another medium.
fn seeded(count: usize) -> (EntityFacade, Arc<Mutex<Vec<String>>>) {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    for index in 0..count {
        put(&facade, &format!("B{index:02}"), "bulk");
    }
    put(&facade, "OTHER", "other");
    (facade, tape)
}

fn put(facade: &EntityFacade, id: &str, medium: &str) {
    let mut row = facade.make_value(TEST).unwrap();
    row.set("testId", Value::string(id));
    row.set("testMedium", Value::string(medium));
    row.set("testLong", Value::string("a long text the key-only read leaves behind"));
    row.create(facade).unwrap();
}

fn bulk(facade: &EntityFacade) -> EntityFind {
    facade.find(TEST).unwrap().eq("testMedium", Value::string("bulk"))
}

fn count(facade: &EntityFacade, medium: &str) -> i64 {
    facade
        .find(TEST)
        .unwrap()
        .eq("testMedium", Value::string(medium))
        .count()
        .unwrap()
}

/// The reads of `test_entity` on the tape since `from`.
fn reads(tape: &Arc<Mutex<Vec<String>>>, from: usize) -> Vec<String> {
    tape.lock().unwrap()[from..]
        .iter()
        .filter(|sql| sql.starts_with("SELECT") && sql.contains("\"test_entity\""))
        .cloned()
        .collect()
}

/// The columns a read selects (not their aliases), sorted.
fn selected(sql: &str) -> Vec<String> {
    let head = sql.split(" FROM ").next().unwrap_or_default();
    let mut columns: Vec<String> = head
        .split(", ")
        .filter_map(|column| column.split('"').nth(1))
        .map(str::to_string)
        .collect();
    columns.sort();
    columns
}

#[test]
fn delete_all_reads_one_chunk_of_keys_per_query() {
    let (facade, tape) = seeded(10);
    let from = tape.lock().unwrap().len();
    assert_eq!(bulk(&facade).bulk_chunk(3).delete_all().unwrap(), 10);
    let reads = reads(&tape, from);
    assert_eq!(reads.len(), 4, "3 + 3 + 3 + 1 rows: {reads:?}");
    for sql in &reads {
        assert_eq!(selected(sql), ["test_id"], "only the key is read: {sql}");
        assert!(sql.contains("LIMIT"), "{sql}");
    }
    assert_eq!(count(&facade, "bulk"), 0);
    assert_eq!(count(&facade, "other"), 1);
}

#[test]
fn update_all_reads_chunks_of_the_key_and_the_fields_it_sets() {
    let (facade, tape) = seeded(7);
    let from = tape.lock().unwrap().len();
    let set = [("testMedium".to_string(), Value::string("done"))];
    assert_eq!(bulk(&facade).bulk_chunk(3).update_all(set).unwrap(), 7);
    let reads = reads(&tape, from);
    assert_eq!(reads.len(), 3, "3 + 3 + 1 rows: {reads:?}");
    for sql in &reads {
        assert_eq!(selected(sql), ["test_id", "test_medium"], "{sql}");
    }
    assert_eq!(count(&facade, "done"), 7);
    let kept = facade
        .find(TEST)
        .unwrap()
        .eq("testId", Value::string("B03"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(
        kept.get("testLong"),
        Some(&Value::string("a long text the key-only read leaves behind"))
    );
}

/// In a transaction cache the held rows answer the reads, which a find of selected fields would skip, so whole rows are
/// read: the rows the frame created are deleted too.
#[test]
fn delete_all_in_a_transaction_cache_sees_the_held_rows() {
    let (facade, _tape) = seeded(4);
    let tx = facade.transactions_for(TEST).unwrap();
    tx.run(TransactionAttribute::Cache, None, || {
        put(&facade, "B50", "bulk");
        assert_eq!(bulk(&facade).bulk_chunk(2).delete_all()?, 5);
        assert_eq!(count(&facade, "bulk"), 0);
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&facade, "bulk"), 0);
}

/// A purge stopped before its backlog is done keeps the chunks it finished.
#[test]
fn a_purge_stopped_early_keeps_the_chunks_it_finished() {
    let (facade, _tape) = seeded(10);
    let mut chunks = 0;
    let purged = bulk(&facade)
        .bulk_chunk(3)
        .purge_while(|| {
            chunks += 1;
            chunks <= 2
        })
        .unwrap();
    assert_eq!(purged.deleted, 6);
    assert!(purged.stopped_early);
    assert_eq!(count(&facade, "bulk"), 4);
    // The next run goes on from there.
    let purged = bulk(&facade).bulk_chunk(3).purge(None).unwrap();
    assert_eq!((purged.deleted, purged.stopped_early), (4, false));
    assert_eq!(count(&facade, "bulk"), 0);
    assert_eq!(count(&facade, "other"), 1);
}

/// Each chunk commits on its own: a caller's transaction that then rolls back, as one past its timeout does, does not
/// take the purged rows back with it.
#[test]
fn a_purge_commits_each_chunk_apart_from_the_callers_transaction() {
    let (facade, _tape) = seeded(5);
    let tx = facade.transactions_for(TEST).unwrap();
    let outer = tx.run(TransactionAttribute::UseOrBegin, None, || -> Result<(), Error> {
        let purged = bulk(&facade).bulk_chunk(2).purge(None)?;
        assert_eq!(purged.deleted, 5);
        Err(Error::new("the caller fails after the purge"))
    });
    assert!(outer.is_err());
    assert_eq!(count(&facade, "bulk"), 0);
}
