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

use std::collections::BTreeMap;

use moqui_db::sql::Condition;
use moqui_lib::object::Value;

fn input(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::string(*v)))
        .collect()
}

fn find() -> super::EntityFind {
    let facade = crate::testing::open();
    facade.find("moqui.test.TestEntity").unwrap()
}

fn conditions(result: &super::SearchFormResult) -> Vec<Condition> {
    result.find.conditions().to_vec()
}

fn search(pairs: &[(&str, &str)]) -> super::SearchFormResult {
    find()
        .search_form_map(&input(pairs), &BTreeMap::new(), "", "", false)
        .unwrap()
}

/// moqui-entity's `text_becomes_the_field_type_and_a_date_widens_to_the_whole_day` with a session time zone.
#[test]
fn a_form_date_is_a_day_of_the_session_time_zone() {
    // The days are the session time zone's, as the form shows them, and so is a time without an offset.
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    crate::user::session::set_time_zone(Some("America/Chicago"));
    let range = search(&[
        ("testDateTime_from", "2026-10-01"),
        ("testDateTime_thru", "2026-10-01 18:00"),
    ]);
    let found = conditions(&range);
    let stamp = |text: &str| Value::Timestamp(text.parse::<jiff::Timestamp>().unwrap());
    assert_eq!(
        found,
        [
            Condition::greater_equal("testDateTime", stamp("2026-10-01T05:00:00Z")),
            Condition::less_equal("testDateTime", stamp("2026-10-01T23:00:00Z")),
        ]
    );
}

#[test]
fn a_form_with_nothing_to_search_returns_no_rows_when_parameters_are_required() {
    let ec = crate::context::ExecutionContextFactory::test_context();
    let _entered = ec.enter();
    let facade = crate::testing::open();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("A"));
    row.create(&facade).unwrap();
    let all = facade.find("moqui.test.TestEntity").unwrap();
    let result = all
        .require_search_form_parameters(true)
        .search_form_map(&BTreeMap::new(), &BTreeMap::new(), "", "", false)
        .unwrap();
    assert_eq!(result.find.list().unwrap().len(), 0);
    assert_eq!(
        ec.shared_map().get("_entityListNoSearchParms"),
        Some(&Value::Bool(true))
    );
    ec.shared_map().shift_remove("_entityListNoSearchParms");
    let searched = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .require_search_form_parameters(true)
        .search_form_map(&input(&[("testId", "A")]), &BTreeMap::new(), "", "", false)
        .unwrap();
    assert_eq!(searched.find.list().unwrap().len(), 1);
    assert!(!ec.shared_map().contains_key("_entityListNoSearchParms"));
    assert_eq!(facade.find("moqui.test.TestEntity").unwrap().list().unwrap().len(), 1);
    assert!(!ec.shared_map().contains_key("_entityListNoSearchParms"));
}
