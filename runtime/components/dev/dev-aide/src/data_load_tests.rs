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

//! The three-way plan of Load the changed records as vectors: base, file and database for each rule of
//! plans/design-dev-aide.md 12, and the choices that settle it.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::*;

const ENTITY: &str = "moqui.basic.Enumeration";

/// A record of `ENTITY` with id `id` and `fields`.
fn record(id: &str, fields: &[(&str, &str)]) -> Record {
    Record {
        entity: ENTITY.to_string(),
        pk: vec![("enumId".to_string(), Value::string(id))],
        fields: values(fields),
        blind: false,
    }
}

fn values(fields: &[(&str, &str)]) -> BTreeMap<String, Value> {
    fields
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect()
}

fn key(id: &str) -> String {
    format!("{ENTITY}[enumId={id}]")
}

/// A row of the database by id: `None` when it has none.
type Row<'a> = (&'a str, Option<&'a [(&'a str, &'a str)]>);

/// The plan with the database's rows by id.
fn planned(base: &[Record], file: &[Record], db: &[Row<'_>]) -> Vec<Planned> {
    let rows: BTreeMap<String, Db> = db
        .iter()
        .map(|(id, row)| (key(id), row.map_or(Db::Absent, |fields| Db::Present(values(fields)))))
        .collect();
    plan(base, file, &mut |record| {
        rows.get(&record.key()).cloned().unwrap_or(Db::Absent)
    })
}

fn actions(plan: &[Planned]) -> Vec<(String, Change, Action)> {
    plan.iter()
        .map(|item| (item.key.clone(), item.change, item.action))
        .collect()
}

fn names(item: &Planned) -> Vec<&str> {
    item.fields.iter().map(|sides| sides.name.as_str()).collect()
}

#[test]
fn a_record_unchanged_in_the_file_is_not_written_whatever_the_database_has() {
    let base = [record("A", &[("description", "One")])];
    let file = [record("A", &[("description", "One")])];
    let plan = planned(&base, &file, &[("A", Some(&[("description", "Changed by hand")]))]);
    assert_eq!(actions(&plan), [(key("A"), Change::Same, Action::Keep)]);
    assert!(plan[0].fields.is_empty());
}

#[test]
fn a_changed_record_is_written_where_the_database_still_has_the_base() {
    let base = [record("A", &[("description", "One"), ("sequenceNum", "1")])];
    let file = [record("A", &[("description", "Uno"), ("sequenceNum", "1")])];
    let plan = planned(
        &base,
        &file,
        &[(
            "A",
            Some(&[("description", "One"), ("sequenceNum", "1"), ("enumCode", "x")]),
        )],
    );
    assert_eq!(actions(&plan), [(key("A"), Change::Changed, Action::Write)]);
    // the preview names what the file changed, not what it kept
    assert_eq!(names(&plan[0]), ["description"]);
    assert_eq!(plan[0].fields[0].base, Some(Value::string("One")));
    assert_eq!(plan[0].fields[0].file, Some(Value::string("Uno")));
    assert_eq!(plan[0].file_index, Some(0));
}

#[test]
fn a_changed_record_the_database_changed_too_is_a_conflict_with_its_fields() {
    let base = [record("A", &[("description", "One"), ("sequenceNum", "1")])];
    let file = [record("A", &[("description", "Uno"), ("sequenceNum", "1")])];
    let plan = planned(
        &base,
        &file,
        &[("A", Some(&[("description", "One"), ("sequenceNum", "5")]))],
    );
    assert_eq!(actions(&plan), [(key("A"), Change::Changed, Action::Conflict)]);
    // the field the file changed and the one the database changed, each with all three sides
    assert_eq!(names(&plan[0]), ["description", "sequenceNum"]);
    let sequence = &plan[0].fields[1];
    assert_eq!(
        (sequence.base.clone(), sequence.file.clone(), sequence.db.clone()),
        (
            Some(Value::string("1")),
            Some(Value::string("1")),
            Some(Value::string("5"))
        )
    );
}

#[test]
fn a_changed_record_the_database_deleted_is_a_conflict() {
    let base = [record("A", &[("description", "One")])];
    let file = [record("A", &[("description", "Uno")])];
    let plan = planned(&base, &file, &[("A", None)]);
    assert_eq!(actions(&plan), [(key("A"), Change::Changed, Action::Conflict)]);
    assert_eq!(plan[0].fields[0].db, None);
}

#[test]
fn a_record_the_database_has_as_the_file_has_it_is_applied_so_a_retry_writes_nothing() {
    let base = [record("A", &[("description", "One")])];
    let file = [
        record("A", &[("description", "Uno")]),
        record("B", &[("description", "New")]),
    ];
    let plan = planned(
        &base,
        &file,
        &[
            ("A", Some(&[("description", "Uno")])),
            ("B", Some(&[("description", "New")])),
        ],
    );
    assert_eq!(
        actions(&plan),
        [
            (key("A"), Change::Changed, Action::Applied),
            (key("B"), Change::Added, Action::Applied)
        ]
    );
}

#[test]
fn an_added_record_is_written_when_absent_and_a_conflict_when_the_database_differs() {
    let file = [
        record("A", &[("description", "New")]),
        record("B", &[("description", "Mine")]),
    ];
    let plan = planned(&[], &file, &[("B", Some(&[("description", "Theirs")]))]);
    assert_eq!(
        actions(&plan),
        [
            (key("A"), Change::Added, Action::Write),
            (key("B"), Change::Added, Action::Conflict)
        ]
    );
    assert_eq!(plan[1].fields[0].base, None);
    assert_eq!(plan[1].fields[0].db, Some(Value::string("Theirs")));
}

#[test]
fn a_removed_record_is_a_choice_when_the_database_has_the_base_and_a_conflict_when_not() {
    let base = [
        record("A", &[("description", "One")]),
        record("B", &[("description", "Two")]),
        record("C", &[("description", "Three")]),
    ];
    let plan = planned(
        &base,
        &[],
        &[
            ("A", Some(&[("description", "One")])),
            ("B", Some(&[("description", "Changed")])),
            ("C", None),
        ],
    );
    assert_eq!(
        actions(&plan),
        [
            (key("A"), Change::Removed, Action::Remove),
            (key("B"), Change::Removed, Action::Conflict),
            (key("C"), Change::Removed, Action::Gone)
        ]
    );
    assert_eq!(plan[1].fields[0].file, None);
}

#[test]
fn numbers_and_blanks_compare_as_the_data_check_compares_them() {
    // the loader converts a field's text to its type, as the database has it
    let decimal = |text: &str| moqui_lib::object::basic_convert(&Value::string(text), "BigDecimal").value;
    assert!(same(&decimal("1.50"), &decimal("1.5")));
    assert!(same(&decimal("1.50"), &Value::string("1.5")));
    // text stays text: ids "01" and "1" are two
    assert!(!same(&Value::string("01"), &Value::string("1")));
    assert!(same(&Value::string(""), &Value::Null));
    assert!(!same(&Value::string("a"), &Value::Null));
    assert!(!same(&Value::string("1.5"), &Value::string("1.6")));
    // a field the base names as empty and the database has as null is the base's
    let base = [record("A", &[("description", "One"), ("enumCode", "")])];
    let file = [record("A", &[("description", "Uno"), ("enumCode", "")])];
    let plan = planned(&base, &file, &[("A", Some(&[("description", "One")]))]);
    assert_eq!(plan[0].action, Action::Write);
}

#[test]
fn a_record_without_a_whole_primary_key_is_passed_over() {
    let mut partial = record("", &[("description", "Sequenced")]);
    partial.pk[0].1 = Value::Null;
    let plan = planned(&[], &[partial], &[]);
    assert_eq!(plan[0].action, Action::Skip);
}

#[test]
fn the_last_record_of_a_key_wins_as_in_a_load() {
    let file = [
        record("A", &[("description", "First")]),
        record("A", &[("description", "Second")]),
    ];
    let plan = planned(&[], &file, &[]);
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].file_index, Some(1));
}

#[test]
fn choices_settle_the_conflicts_and_removals_and_none_is_settled_by_default() {
    let base = [
        record("A", &[("description", "One")]),
        record("B", &[("description", "Two")]),
        record("R", &[("description", "Removed")]),
        record("S", &[("description", "Removed, changed")]),
    ];
    let file = [
        record("A", &[("description", "Uno")]),
        record("B", &[("description", "Dos")]),
        record("N", &[("description", "New")]),
    ];
    let plan = planned(
        &base,
        &file,
        &[
            ("A", Some(&[("description", "Hand")])),
            ("B", Some(&[("description", "Hand")])),
            ("R", Some(&[("description", "Removed")])),
            ("S", Some(&[("description", "Hand")])),
        ],
    );
    // the conflicts with no choice, the removed S among them: refused, naming each
    assert_eq!(
        resolve(&plan, &Choices::new()).unwrap_err(),
        [key("A"), key("B"), key("S")]
    );
    let choices: Choices = [
        (key("A"), "file".to_string()),
        (key("B"), "database".to_string()),
        (key("S"), "file".to_string()),
    ]
    .into_iter()
    .collect();
    let resolved = resolve(&plan, &choices).unwrap();
    let keys = |items: &[&Planned]| items.iter().map(|item| item.key.clone()).collect::<Vec<_>>();
    // Take file for A writes it, N is written; Take file for the removed S deletes it; R stays unless chosen
    assert_eq!(keys(&resolved.writes), [key("A"), key("N")]);
    assert_eq!(keys(&resolved.deletes), [key("S")]);
    assert_eq!(resolved.kept, 2);
    // R, a removal nothing was chosen for, holds the baseline; B was chosen
    assert_eq!(resolved.unchosen, 1);
    let mut with_removal = choices.clone();
    with_removal.insert(key("R"), "file".to_string());
    assert_eq!(
        keys(&resolve(&plan, &with_removal).unwrap().deletes),
        [key("R"), key("S")]
    );
}

#[test]
fn a_record_the_caller_may_not_read_differs_from_its_base_only_and_is_written_only_when_chosen() {
    let blind = |mut record: Record| {
        record.blind = true;
        record
    };
    let base = [
        blind(record("A", &[("description", "One")])),
        blind(record("R", &[("description", "Gone")])),
    ];
    let file = [
        blind(record("A", &[("description", "Uno")])),
        blind(record("S", &[("description", "Same")])),
    ];
    let mut read = Vec::new();
    // the database has A as the file has it: a plan that compared would say applied, which would confirm a guess
    let plan = plan(&base, &file, &mut |record| {
        read.push(record.key());
        Db::Present(values(&[("description", "Uno")]))
    });
    assert!(read.is_empty(), "the database is never read for them: {read:?}");
    assert_eq!(
        actions(&plan),
        [
            (key("A"), Change::Changed, Action::Differs),
            (key("S"), Change::Added, Action::Differs),
            (key("R"), Change::Removed, Action::Differs)
        ]
    );
    assert!(plan.iter().all(|item| item.fields.is_empty()));
    // kept unless chosen; Take file writes, or deletes the removed one
    assert_eq!(resolve(&plan, &Choices::new()).unwrap().kept, 3);
    assert_eq!(resolve(&plan, &Choices::new()).unwrap().unchosen, 3);
    let choices: Choices = [(key("A"), "file".to_string()), (key("R"), "file".to_string())]
        .into_iter()
        .collect();
    let resolved = resolve(&plan, &choices).unwrap();
    assert_eq!(resolved.writes.len(), 1);
    assert_eq!(resolved.deletes[0].key, key("R"));
}
