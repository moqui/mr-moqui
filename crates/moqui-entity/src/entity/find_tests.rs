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
use moqui_lib::object::Value;

fn seeded() -> (crate::entity::EntityFacade, Arc<Mutex<Vec<String>>>) {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    for (id, medium) in [("A", "alpha"), ("B", "beta")] {
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string(id));
        row.set("testMedium", Value::string(medium));
        row.create(&facade).unwrap();
    }
    (facade, tape)
}

/// Java's `EntityListIterator`: a scrollable cursor moves back and jumps; a forward-only one refuses to move back.
#[test]
fn a_scrollable_iterator_moves_back_and_jumps_and_a_forward_one_does_not() {
    let (facade, _tape) = seeded();
    let id = |row: Option<crate::entity::EntityValue>| {
        row.and_then(|row| row.get("testId").and_then(Value::as_str).map(str::to_string))
    };
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
        let mut scroll = facade
            .find("moqui.test.TestEntity")?
            .order_by("testId", false)
            .scrollable(true)
            .iterator()?;
        assert!(scroll.has_next()?);
        assert_eq!(id(scroll.next_value()?).as_deref(), Some("A"));
        assert_eq!(id(scroll.next_value()?).as_deref(), Some("B"));
        assert_eq!(id(scroll.previous_value()?).as_deref(), Some("A"));
        assert!(scroll.absolute(-1)?);
        assert_eq!(scroll.current_index(), 2);
        assert!(!scroll.relative(1)?);
        let page = scroll.get_partial_list(2, 5)?;
        assert_eq!(page.len(), 1);
        assert_eq!(scroll.get_complete_list()?.len(), 2);
        drop(scroll);
        let mut third = facade.make_value("moqui.test.TestEntity")?;
        third.set("testId", Value::string("C"));
        third.create(&facade)?;
        let mut forward = facade
            .find("moqui.test.TestEntity")?
            .order_by("testId", false)
            .iterator()?;
        assert_eq!(id(forward.next_value()?).as_deref(), Some("A"));
        assert!(forward.has_next()?);
        assert_eq!(id(forward.next_value()?).as_deref(), Some("B"));
        assert_eq!(id(forward.next_value()?).as_deref(), Some("C"));
        // The last two rows are kept, so one step back works; the first row is gone.
        assert_eq!(id(forward.previous_value()?).as_deref(), Some("B"));
        assert!(forward.absolute(1).is_err());
        assert!(forward.before_first().is_err());
        assert!(forward.last().is_err());
        drop(forward);
        // Java's first, last, beforeFirst, afterLast, hasPrevious and currentEntityValue, by resultSetType
        let mut moved = facade
            .find("moqui.test.TestEntity")?
            .order_by("testId", false)
            .result_set_type(crate::entity::TYPE_SCROLL_INSENSITIVE)
            .iterator()?;
        assert!(!moved.has_previous());
        assert!(moved.current_value()?.is_none());
        assert!(moved.last()?);
        assert_eq!(id(moved.current_value()?).as_deref(), Some("C"));
        assert!(moved.has_previous());
        assert!(moved.first()?);
        assert_eq!(id(moved.current_value()?).as_deref(), Some("A"));
        moved.after_last()?;
        assert_eq!(id(moved.previous_value()?).as_deref(), Some("C"));
        moved.before_first()?;
        assert_eq!(id(moved.next_value()?).as_deref(), Some("A"));
        // a find for update scrolls, as Java's scroll-sensitive one; the default is forward only
        let find = facade.find("moqui.test.TestEntity")?;
        assert_eq!(find.get_result_set_type(), crate::entity::TYPE_FORWARD_ONLY);
        assert_eq!(
            find.for_update(true).get_result_set_type(),
            crate::entity::TYPE_SCROLL_SENSITIVE
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn select_field_reads_only_those_columns_and_ignores_a_missing_one() {
    let (facade, tape) = seeded();
    let rows = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .select_field("testId")
        .eq("testId", Value::string("A"))
        .list()
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows.values()[0].get("testMedium").is_none(),
        "only the selected field is read"
    );
    let sql = tape.lock().unwrap().last().cloned().unwrap();
    assert!(sql.starts_with("SELECT \"test_id\" AS \"testId\" FROM"), "{sql}");
    // As Java's `selectField`, a name that is not a field is skipped; with none left every field is read.
    let rows = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .select_field("nope")
        .eq("testId", Value::string("A"))
        .list()
        .unwrap();
    assert!(rows.values()[0].get("testMedium").is_some(), "every field is read");
}

#[test]
fn an_iterator_that_requires_search_parameters_finds_nothing_without_them() {
    let (facade, tape) = seeded();
    let before = tape.lock().unwrap().len();
    let mut found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .require_search_form_parameters(true)
        .iterator()
        .unwrap();
    assert!(found.next_value().unwrap().is_none());
    assert_eq!(tape.lock().unwrap().len(), before, "no statement runs");
}

#[test]
fn for_update_needs_a_transaction_and_is_written_inside_one() {
    let (facade, tape) = seeded();
    let err = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .for_update(true)
        .list()
        .unwrap_err();
    assert!(err.to_string().contains("transaction"), "{err}");
    facade.begin("moqui.test.TestEntity", None).unwrap();
    let outcome = facade.find("moqui.test.TestEntity").unwrap().for_update(true).list();
    facade.rollback("moqui.test.TestEntity").unwrap();
    // The in-memory driver does not read FOR UPDATE; the statement is what counts here.
    let _ = outcome;
    assert!(tape.lock().unwrap().iter().any(|sql| sql.ends_with(" FOR UPDATE")));
}

/// A no-key lock is written as its own text; both lock calls on are refused when the find runs, in either order, and a
/// `false` of one leaves the other's lock.
#[test]
fn for_no_key_update_is_written_and_not_with_for_update() {
    let (facade, tape) = seeded();
    let find = || facade.find("moqui.test.TestEntity").unwrap();
    facade.begin("moqui.test.TestEntity", None).unwrap();
    let _ = find().for_no_key_update(true).list();
    let _ = find().for_update(true).for_no_key_update(false).list();
    let both = [
        find().for_update(true).for_no_key_update(true).list(),
        find().for_no_key_update(true).for_update(true).list(),
    ];
    facade.rollback("moqui.test.TestEntity").unwrap();
    for outcome in both {
        let err = outcome.unwrap_err();
        assert!(err.to_string().contains("cannot be both"), "{err}");
    }
    let tape = tape.lock().unwrap();
    assert!(tape.iter().any(|sql| sql.ends_with(" FOR NO KEY UPDATE")));
    assert!(tape
        .iter()
        .any(|sql| sql.ends_with(" FOR UPDATE") && !sql.contains("NO KEY")));
}

/// Java's `condition(field, value)` keeps one value per field: the second replaces the first, also inside an `AND`.
#[test]
fn a_second_equality_on_a_field_replaces_the_first() {
    let (facade, _tape) = seeded();
    let found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("B"))
        .eq("testId", Value::string("A"))
        .list()
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found.values()[0].get("testId").and_then(Value::as_str), Some("A"));
    let find = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testMedium", Value::string("beta"))
        .condition(Condition::and(vec![
            Condition::equals("testMedium", Value::string("alpha")),
            Condition::not_equals("testId", Value::string("B")),
        ]))
        .condition(Condition::not_equals("testId", Value::string("C")));
    assert_eq!(
        find.conditions,
        [
            Condition::equals("testMedium", Value::string("alpha")),
            Condition::not_equals("testId", Value::string("B")),
            Condition::not_equals("testId", Value::string("C")),
        ]
    );
    assert_eq!(find.list().unwrap().len(), 1);
}

#[test]
fn distinct_and_having_reach_the_statement() {
    let (facade, tape) = seeded();
    let _ = facade.find("moqui.test.TestEntity").unwrap().distinct(true).list();
    assert!(tape
        .lock()
        .unwrap()
        .iter()
        .any(|sql| sql.starts_with("SELECT DISTINCT ")));
    // A plain entity has no aggregate to apply a having condition to.
    let err = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .having(Condition::greater("testId", Value::I32(1)))
        .list()
        .unwrap_err();
    assert!(err.to_string().contains("no aggregate"), "{err}");
}

#[test]
fn ignore_case_binds_upper_case_text_and_field_comparisons_reach_the_statement() {
    let (facade, tape) = seeded();
    // The in-memory driver cannot read UPPER(); the statement and its bound values are the point.
    let _ = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::ignore_case(Condition::equals(
            "testMedium",
            Value::string("Alpha"),
        )))
        .list();
    let _ = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::compare_fields("testId", CompareOp::LessEqual, "testMedium"))
        .list();
    let tape = tape.lock().unwrap();
    assert!(
        tape.iter()
            .any(|sql| sql.ends_with("WHERE UPPER(\"test_medium\") = UPPER(?)")),
        "{tape:?}"
    );
    assert!(
        tape.iter()
            .any(|sql| sql.ends_with("WHERE \"test_id\" <= \"test_medium\"")),
        "{tape:?}"
    );
    let dialect = moqui_db::dialect::Dialects::load(
        &moqui_lib::node::MNode::parse_text("conf", r#"<database-list><database name="postgres"/></database-list>"#)
            .unwrap(),
    )
    .unwrap();
    let rendered = moqui_db::sql::render_condition(
        &Condition::ignore_case(Condition::equals("x", Value::string("Alpha"))),
        dialect.get("postgres").unwrap(),
    )
    .unwrap();
    assert_eq!(rendered.params, vec![Value::string("ALPHA")]);
}

/// plans/High-Level-Review.md HL-003: an iterator opened with no transaction in place begins one, which holds its
/// connection while it reads and commits when it closes.
#[test]
fn an_iterator_outside_a_transaction_reads_in_one_it_began() {
    let (facade, _tape) = seeded();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    assert!(!tx.in_transaction());
    let mut iter = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .order_by("testId", false)
        .iterator()
        .unwrap();
    assert!(tx.in_transaction(), "the iterator began a transaction");
    let id = |row: Option<crate::entity::EntityValue>| row.and_then(|row| row.get("testId").cloned());
    assert_eq!(id(iter.next_value().unwrap()), Some(Value::string("A")));
    assert_eq!(id(iter.next_value().unwrap()), Some(Value::string("B")));
    assert!(iter.next_value().unwrap().is_none());
    assert!(!tx.in_transaction(), "reading to the end closed it and committed");
    drop(iter);
    // Dropped early, it commits too.
    let mut early = facade.find("moqui.test.TestEntity").unwrap().iterator().unwrap();
    assert!(early.next_value().unwrap().is_some());
    drop(early);
    assert!(!tx.in_transaction());
}

/// An iterator whose transaction has ended does not close the cursor of the iterator open in the next one, and fails
/// rather than read from it.
#[test]
fn a_stale_iterator_does_not_close_the_current_cursor() {
    let (facade, _tape) = seeded();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    let find = || facade.find("moqui.test.TestEntity").unwrap().order_by("testId", false);
    tx.begin(None).unwrap();
    let mut stale = find().iterator().unwrap();
    assert!(stale.next_value().unwrap().is_some());
    tx.commit().unwrap();
    tx.begin(None).unwrap();
    let mut current = find().iterator().unwrap();
    assert!(stale.next_value().is_err(), "its transaction ended");
    drop(stale);
    let id = |row: Option<crate::entity::EntityValue>| row.and_then(|row| row.get("testId").cloned());
    assert_eq!(id(current.next_value().unwrap()), Some(Value::string("A")));
    assert_eq!(id(current.next_value().unwrap()), Some(Value::string("B")));
    drop(current);
    tx.commit().unwrap();
}

/// A find one with the whole primary key keeps an equality the caller set on its own: a guard such as
/// `reportPending = Y` or an owner beside the key filters the row. An equality or `IS NULL` from a map (the context of
/// an `entity-find-one`, which holds whatever the context does) is left out, as Java's `oneInternal` leaves out its
/// simple map.
#[test]
fn a_find_one_by_key_keeps_its_own_conditions_and_leaves_out_a_maps() {
    let (facade, _tape) = seeded();
    let by_key = || {
        facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("A"))
    };
    let id = |row: Option<crate::entity::EntityValue>| row.and_then(|row| row.get("testId").cloned());
    assert!(by_key()
        .eq("testMedium", Value::string("beta"))
        .one()
        .unwrap()
        .is_none());
    assert_eq!(
        id(by_key().eq("testMedium", Value::string("alpha")).one().unwrap()),
        Some(Value::string("A"))
    );
    let from_map = by_key()
        .condition_from_map(Condition::equals("testMedium", Value::string("stale")))
        .condition_from_map(Condition::IsNull("value".into()))
        .one()
        .unwrap();
    assert_eq!(id(from_map), Some(Value::string("A")));
    // A field the caller sets again on its own after the map counts as its own.
    let reset = by_key()
        .condition_from_map(Condition::equals("testMedium", Value::string("alpha")))
        .eq("testMedium", Value::string("beta"))
        .one()
        .unwrap();
    assert!(reset.is_none());
}
