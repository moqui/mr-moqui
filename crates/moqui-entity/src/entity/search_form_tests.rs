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
    result.find.conditions.clone()
}

fn search(pairs: &[(&str, &str)]) -> super::SearchFormResult {
    find()
        .search_form_map(&input(pairs), &BTreeMap::new(), "", "", false)
        .unwrap()
}

#[test]
fn operators_build_the_conditions_java_builds() {
    assert_eq!(
        conditions(&search(&[("testMedium", "alp"), ("testMedium_op", "contains")])),
        [Condition::like("testMedium", Value::string("%alp%"))]
    );
    assert_eq!(
        conditions(&search(&[
            ("testMedium", "alp"),
            ("testMedium_op", "begins"),
            ("testMedium_not", "Y")
        ])),
        [Condition::not_like("testMedium", Value::string("alp%"))]
    );
    assert_eq!(
        conditions(&search(&[("testId", "A,B"), ("testId_op", "in")])),
        [Condition::is_in("testId", vec![Value::string("A"), Value::string("B")])]
    );
    assert_eq!(
        conditions(&search(&[("testId", "A"), ("testId_ic", "true")])),
        [Condition::ignore_case(Condition::equals("testId", Value::string("A")))]
    );
    // "empty" needs no value: null or the empty string, and "not" is neither.
    assert_eq!(
        conditions(&search(&[("testMedium_op", "empty")])),
        [Condition::or(vec![
            Condition::is_null("testMedium"),
            Condition::equals("testMedium", Value::string(""))
        ])]
    );
    // An empty value makes no condition, and so does a field the form did not send.
    let empty = search(&[("testMedium", ""), ("other", "x")]);
    assert!(conditions(&empty).is_empty());
    assert!(!empty.find.has_search_parameters);
}

#[test]
fn ignore_case_is_not_for_in_and_not_also_finds_the_empty_rows() {
    // Java attaches `_ic` to equals, like, contains and begins only.
    assert_eq!(
        conditions(&search(&[
            ("testId", "A,B"),
            ("testId_op", "in"),
            ("testId_ic", "true")
        ])),
        [Condition::is_in("testId", vec![Value::string("A"), Value::string("B")])]
    );
    assert_eq!(
        conditions(&search(&[
            ("testMedium", "alp"),
            ("testMedium_op", "begins"),
            ("testMedium_ic", "Y")
        ])),
        [Condition::ignore_case(Condition::like(
            "testMedium",
            Value::string("alp%")
        ))]
    );
    // Java's `orNull`: "not" on equals and in also finds the rows with no value.
    assert_eq!(
        conditions(&search(&[("testId", "A"), ("testId_not", "Y")])),
        [Condition::or(vec![
            Condition::not_equals("testId", Value::string("A")),
            Condition::is_null("testId")
        ])]
    );
    assert_eq!(
        conditions(&search(&[
            ("testId", "A,B"),
            ("testId_op", "in"),
            ("testId_not", "true")
        ])),
        [Condition::or(vec![
            Condition::not_in("testId", vec![Value::string("A"), Value::string("B")]),
            Condition::is_null("testId")
        ])]
    );
}

#[test]
fn a_form_value_replaces_the_finds_own_and_the_cache_is_off_unless_asked_for() {
    let replaced = find()
        .eq("testId", Value::string("A"))
        .search_form_map(&input(&[("testId", "B")]), &BTreeMap::new(), "", "", false)
        .unwrap();
    assert_eq!(conditions(&replaced), [Condition::equals("testId", Value::string("B"))]);
    // Java's `useCache(false)` when the find did not choose, so a paged form never reads a cached list.
    assert_eq!(replaced.find.use_cache, Some(false));
    // A find that asked for the cache is not paged, so the cached list answers it.
    let cached = find()
        .use_cache(true)
        .search_form_map(&input(&[("pageIndex", "1")]), &BTreeMap::new(), "", "", true)
        .unwrap();
    assert_eq!((cached.find.offset, cached.find.limit), (None, None));
    assert_eq!(cached.find.use_cache, Some(true));
    // Only `true` removes paging, as Java.
    let paged = find()
        .search_form_map(&input(&[("pageNoLimit", "Y")]), &BTreeMap::new(), "", "", true)
        .unwrap();
    assert_eq!((paged.find.offset, paged.find.limit), (Some(0), Some(20)));
}

#[test]
fn a_negative_page_gives_no_negative_offset() {
    let page = search(&[("pageIndex", "-3"), ("pageSize", "10")]);
    assert_eq!((page.find.offset_value(), page.find.limit_value()), (Some(0), Some(10)));
    let page = search(&[("pageIndex", "2"), ("pageSize", "-10")]);
    assert_eq!((page.find.offset_value(), page.find.limit_value()), (Some(0), Some(0)));
    let page = search(&[("pageIndex", "2"), ("pageSize", "10")]);
    assert_eq!(
        (page.find.offset_value(), page.find.limit_value()),
        (Some(20), Some(10))
    );
}

#[test]
fn text_becomes_the_field_type_and_a_date_widens_to_the_whole_day() {
    let typed = search(&[("testNumberInteger", "7")]);
    assert_eq!(
        conditions(&typed),
        [Condition::equals("testNumberInteger", Value::I64(7))]
    );
    let range = search(&[("testDateTime_from", "2026-10-01"), ("testDateTime_thru", "2026-10-02")]);
    let found = conditions(&range);
    assert_eq!(found.len(), 2);
    let Condition::GreaterEqual(_, Value::Timestamp(from)) = &found[0] else {
        panic!("from bound {found:?}");
    };
    let Condition::LessEqual(_, Value::Timestamp(thru)) = &found[1] else {
        panic!("thru bound {found:?}");
    };
    // From the start of 10-01 to the last millisecond of 10-02, both whole days.
    assert_eq!(thru.as_millisecond() - from.as_millisecond(), 2 * 86_400_000 - 1);
}

#[test]
fn a_period_makes_a_half_open_range() {
    let found = conditions(&search(&[
        ("testDateTime_period", "month"),
        ("testDateTime_poffset", "0"),
        ("testDateTime_pdate", "2026-10-14"),
    ]));
    assert!(matches!(&found[0], Condition::GreaterEqual(..)), "{found:?}");
    assert!(matches!(&found[1], Condition::Less(..)), "{found:?}");
}

#[test]
fn defaults_apply_only_without_input_and_order_and_paging_are_read() {
    let defaults = input(&[("testMedium", "alpha")]);
    let none = find()
        .search_form_map(&BTreeMap::new(), &defaults, "", "-testId", false)
        .unwrap();
    assert_eq!(
        conditions(&none),
        [Condition::equals("testMedium", Value::string("alpha"))]
    );
    assert_eq!(none.context.get("testMedium").and_then(Value::as_str), Some("alpha"));
    assert_eq!(
        none.context.get("orderByField").and_then(Value::as_str),
        Some("-testId")
    );
    assert!(!none.find.has_search_parameters, "defaults are not search parameters");
    let given = find()
        .search_form_map(&input(&[("testId", "A")]), &defaults, "", "", false)
        .unwrap();
    assert_eq!(conditions(&given), [Condition::equals("testId", Value::string("A"))]);

    let paged = find()
        .search_form_map(
            &input(&[("pageIndex", "2"), ("pageSize", "10")]),
            &BTreeMap::new(),
            "",
            "",
            false,
        )
        .unwrap();
    assert_eq!((paged.find.offset, paged.find.limit), (Some(20), Some(10)));
    let always = find()
        .search_form_map(&BTreeMap::new(), &BTreeMap::new(), "", "", true)
        .unwrap();
    assert_eq!((always.find.offset, always.find.limit), (Some(0), Some(20)));
    let unlimited = find()
        .search_form_map(&input(&[("pageNoLimit", "true")]), &BTreeMap::new(), "", "", true)
        .unwrap();
    assert_eq!((unlimited.find.offset, unlimited.find.limit), (None, None));
    let skipped = find()
        .search_form_map(&input(&[("testId", "A")]), &BTreeMap::new(), "testId", "", false)
        .unwrap();
    assert!(conditions(&skipped).is_empty());
}
