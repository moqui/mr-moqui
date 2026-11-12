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

//! plans/archive/High-Level-Review.md HL-033: bulk writes page by primary key in bounded chunks, and a purge commits each one.

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

const MEMBER: &str = "moqui.entity.view.DbViewEntityMember";

fn member(facade: &EntityFacade, alias: &str, join_from: Option<&str>) -> crate::entity::EntityValue {
    let mut row = facade.make_value(MEMBER).unwrap();
    row.set("dbViewEntityName", Value::string("SpecView"));
    row.set("entityAlias", Value::string(alias));
    row.set("entityName", Value::string("moqui.basic.Enumeration"));
    if let Some(from) = join_from {
        row.set("joinFromAlias", Value::string(from));
    }
    row
}

/// `DbViewEntityMember` refers to itself (`joinFromAlias`): a row is deleted only after the rows that join from it, in
/// whatever order they were read, so the self-referencing foreign key does not refuse the delete.
#[test]
fn rows_of_a_self_referencing_entity_are_deleted_referring_rows_first() {
    let facade = crate::testing::open();
    let rows = vec![
        member(&facade, "MASTER", None),
        member(&facade, "MEMBER2", Some("MEMBER1")),
        member(&facade, "MEMBER1", Some("MASTER")),
        member(&facade, "OTHER", Some("GONE")),
    ];
    let pk = ["dbViewEntityName".to_string(), "entityAlias".to_string()];
    let self_refs = vec![vec![
        ("dbViewEntityName".to_string(), "dbViewEntityName".to_string()),
        ("joinFromAlias".to_string(), "entityAlias".to_string()),
    ]];
    let ordered: Vec<String> = super::children_first(rows, &pk, &self_refs)
        .iter()
        .map(|row| {
            row.get("entityAlias")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let at = |alias: &str| ordered.iter().position(|got| got == alias).unwrap();
    assert!(
        at("MEMBER2") < at("MEMBER1") && at("MEMBER1") < at("MASTER"),
        "{ordered:?}"
    );
    assert_eq!(ordered.len(), 4);
}

/// The delete of such an entity reads its rows in one query, and a find with its own order keeps it.
#[test]
fn a_self_referencing_delete_and_an_ordered_update_are_read_in_one_query() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    for row in [
        member(&facade, "MASTER", None),
        member(&facade, "MEMBER1", Some("MASTER")),
    ] {
        row.create(&facade).unwrap();
    }
    let from = tape.lock().unwrap().len();
    let deleted = facade
        .find(MEMBER)
        .unwrap()
        .eq("dbViewEntityName", Value::string("SpecView"))
        .bulk_chunk(1)
        .delete_all()
        .unwrap();
    assert_eq!(deleted, 2);
    let member_reads = tape.lock().unwrap()[from..]
        .iter()
        .filter(|sql| sql.starts_with("SELECT") && sql.contains("\"db_view_entity_member\""))
        .count();
    assert_eq!(member_reads, 1, "{:?}", &tape.lock().unwrap()[from..]);

    let (facade, tape) = seeded(5);
    let from = tape.lock().unwrap().len();
    let set = [("testMedium".to_string(), Value::string("done"))];
    assert_eq!(
        bulk(&facade)
            .order_by("testId", true)
            .bulk_chunk(2)
            .update_all(set)
            .unwrap(),
        5
    );
    let reads = reads(&tape, from);
    assert_eq!(reads.len(), 1, "{reads:?}");
    assert!(reads[0].contains("ORDER BY \"test_id\" DESC"), "{reads:?}");
}

/// The statements of `table` on the tape since `from` that begin with `verb`.
fn statements(tape: &Arc<Mutex<Vec<String>>>, from: usize, verb: &str, table: &str) -> Vec<String> {
    let quoted = format!("\"{table}\"");
    tape.lock().unwrap()[from..]
        .iter()
        .filter(|sql| sql.starts_with(verb) && sql.contains(&quoted))
        .cloned()
        .collect()
}

/// A chunk of rows read by key alone is deleted in one statement, by an IN list of its keys, not one statement a row.
#[test]
fn a_chunk_of_rows_read_by_key_is_deleted_in_one_statement() {
    let (facade, tape) = seeded(10);
    let from = tape.lock().unwrap().len();
    assert_eq!(bulk(&facade).bulk_chunk(4).delete_all().unwrap(), 10);
    let deletes = statements(&tape, from, "DELETE", "test_entity");
    assert_eq!(deletes.len(), 3, "4 + 4 + 2 rows: {deletes:?}");
    for sql in &deletes {
        assert!(sql.contains(" IN ("), "{sql}");
    }
    assert_eq!(count(&facade, "bulk"), 0);
    assert_eq!(count(&facade, "other"), 1);

    // A purge's chunk too, each in its own transaction.
    let (facade, tape) = seeded(5);
    let from = tape.lock().unwrap().len();
    let purged = bulk(&facade).bulk_chunk(3).purge(None).unwrap();
    assert_eq!((purged.deleted, purged.failed_chunks), (5, 0));
    assert_eq!(statements(&tape, from, "DELETE", "test_entity").len(), 2);
    assert_eq!(count(&facade, "bulk"), 0);
}

const MESSAGE: &str = "moqui.basic.LocalizedMessage";

fn message(facade: &EntityFacade, original: &str, locale: &str) {
    let mut row = facade.make_value(MESSAGE).unwrap();
    row.set("original", Value::string(original));
    row.set("locale", Value::string(locale));
    row.set("localized", Value::string(format!("{original} in {locale}")));
    row.create(facade).unwrap();
}

/// The rows of the cached list of messages whose `field` is `value`; `None` when it is not cached.
fn cached_messages(facade: &EntityFacade, field: &str, value: &str) -> Option<usize> {
    facade
        .find(MESSAGE)
        .unwrap()
        .eq(field, Value::string(value))
        .use_cache(true)
        .list_from_cache()
        .unwrap()
        .map(|rows| rows.len())
}

/// The rows of an entity with a composite key go in one statement that names each row's whole key, and the cached
/// lists that held a deleted row are evicted, while one that held none stays.
#[test]
fn a_composite_key_chunk_is_one_statement_and_evicts_the_lists_holding_its_rows() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    for original in ["M1", "M2", "M3"] {
        message(&facade, original, "xx");
        message(&facade, original, "yy");
    }
    let list = |field: &str, value: &str| {
        facade
            .find(MESSAGE)
            .unwrap()
            .eq(field, Value::string(value))
            .use_cache(true)
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(list("locale", "xx"), 3);
    assert_eq!(list("locale", "yy"), 3);
    assert_eq!(list("original", "M2"), 2);
    assert_eq!(cached_messages(&facade, "original", "M2"), Some(2), "cached");

    let from = tape.lock().unwrap().len();
    let deleted = facade
        .find(MESSAGE)
        .unwrap()
        .eq("locale", Value::string("xx"))
        .delete_all()
        .unwrap();
    assert_eq!(deleted, 3);
    let deletes = statements(&tape, from, "DELETE", "localized_message");
    assert_eq!(deletes.len(), 1, "{deletes:?}");
    assert_eq!(
        deletes[0].matches(" OR ").count(),
        2,
        "one key for each row: {}",
        deletes[0]
    );
    assert_eq!(cached_messages(&facade, "locale", "xx"), None, "held deleted rows");
    assert_eq!(cached_messages(&facade, "original", "M2"), None, "held M2 in xx");
    assert_eq!(cached_messages(&facade, "locale", "yy"), Some(3), "held none of them");
    assert_eq!(list("original", "M2"), 1);
}

/// A statement binds at most 2000 values and names at most 1000 rows.
#[test]
fn a_delete_statement_stays_within_the_bind_limits() {
    use crate::entity::value::delete_rows_per_statement;
    assert_eq!(delete_rows_per_statement(1), 1000);
    assert_eq!(delete_rows_per_statement(2), 1000);
    assert_eq!(delete_rows_per_statement(3), 666);
    assert_eq!(delete_rows_per_statement(4000), 1);
}

/// A chunk deleted in a transaction removes its rows at commit by their keys, also a row another transaction changed
/// meanwhile: the tests' database replays the delete by the key columns of its IN list, as a database deletes the rows
/// it names.
#[test]
fn a_chunk_deleted_in_a_transaction_goes_through_another_changed_a_row_meanwhile() {
    let (facade, _tape) = seeded(3);
    let tx = facade.transactions_for(TEST).unwrap();
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        assert_eq!(count(&facade, "bulk"), 3);
        let other = facade.clone();
        std::thread::spawn(move || {
            let mut row = other.make_value(TEST).unwrap();
            row.set("testId", Value::string("B01"));
            row.set("testLong", Value::string("changed by another transaction"));
            row.update(&other).unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(bulk(&facade).delete_all()?, 3);
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&facade, "bulk"), 0);
}
