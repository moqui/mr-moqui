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

use super::*;

fn row(pairs: &[(&str, Value)]) -> moqui_lib::object::ValueMap {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect()
}

fn form(xml: &str) -> MNode {
    MNode::parse_text("form", xml).unwrap()
}

#[test]
fn group_by_merges_rows_with_functions_sub_lists_and_totals() {
    let node = form(
        r#"<form-list name="F" list="l">
                <field name="party" aggregate="group-by"/>
                <field name="amount" aggregate="sum" show-total="true"/>
                <field name="lines" aggregate="count"/>
                <field name="item" aggregate="sub-list"/>
                <field name="qty" aggregate="sub-list" show-total="sum"/>
            </form-list>"#,
    );
    let aggregation = Aggregation::of(&node).unwrap().unwrap();
    let d = |text: &str| Value::Decimal(text.parse().unwrap());
    let rows = vec![
        row(&[
            ("party", Value::string("A")),
            ("amount", d("1.50")),
            ("lines", Value::string("x")),
            ("item", Value::string("i1")),
            ("qty", Value::I32(2)),
        ]),
        row(&[
            ("party", Value::string("B")),
            ("amount", d("3")),
            ("lines", Value::string("x")),
            ("item", Value::string("i2")),
            ("qty", Value::I32(5)),
        ]),
        row(&[
            ("party", Value::string("A")),
            ("amount", d("2.25")),
            ("lines", Value::string("x")),
            ("item", Value::string("i3")),
            ("qty", Value::I32(1)),
        ]),
    ];
    let out = aggregation.apply(rows, None, |row, name| row.get(name).cloned().unwrap_or(Value::Null));
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(out[0].get("party"), Some(&Value::string("A")));
    assert_eq!(out[0].get("amount"), Some(&d("3.75")));
    assert_eq!(out[0].get("lines"), Some(&Value::I32(2)));
    assert!(!out[0].contains_key("item"));
    let Some(Value::List(subs)) = out[0].get("aggregateSubList") else {
        panic!("{out:?}")
    };
    // two sub rows and the sub-list totals last
    assert_eq!(subs.len(), 3, "{subs:?}");
    let Value::Map(totals) = &subs[2] else { panic!() };
    assert_eq!(totals.get("qty"), Some(&Value::I32(3)));
    assert_eq!(out[2].get("_moquiRowType"), Some(&Value::string("total")));
    assert_eq!(out[2].get("amount"), Some(&d("6.75")));
}

/// The first row of a group is part of an average. 10, 20, 30 is 20 with a count of 3.
#[test]
fn a_group_average_counts_the_first_row() {
    let node = form(
        r#"<form-list name="F" list="l">
                <field name="party" aggregate="group-by"/>
                <field name="n" aggregate="avg"/>
            </form-list>"#,
    );
    let aggregation = Aggregation::of(&node).unwrap().unwrap();
    let d = |number: i32| Value::Decimal(Decimal::from(number));
    let rows = vec![
        row(&[("party", Value::string("A")), ("n", d(10))]),
        row(&[("party", Value::string("A")), ("n", d(20))]),
        row(&[("party", Value::string("A")), ("n", d(30))]),
    ];
    let out = aggregation.apply(rows, None, |row, name| row.get(name).cloned().unwrap_or(Value::Null));
    assert_eq!(out.len(), 1, "{out:?}");
    let Value::Decimal(average) = out[0].get("n").unwrap() else {
        panic!("{out:?}")
    };
    assert!(moqui_lib::object::decimal_eq(*average, Decimal::from(20)), "{average}");
    assert_eq!(out[0].get("nCount"), Some(&d(3)));
}

/// A `group-by` field the list does not show is left out of the group key, as Java's `includeFields`. With none
/// shown, every row is one group.
#[test]
fn a_group_by_field_that_is_not_shown_is_not_in_the_group_key() {
    let node = form(
        r#"<form-list name="F" list="l">
                <field name="party" aggregate="group-by"/>
                <field name="invoice" aggregate="group-by"/>
                <field name="amount" aggregate="sum"/>
            </form-list>"#,
    );
    let aggregation = Aggregation::of(&node).unwrap().unwrap();
    let d = |number: i32| Value::Decimal(Decimal::from(number));
    let rows = || {
        vec![
            row(&[
                ("party", Value::string("A")),
                ("invoice", Value::string("i1")),
                ("amount", d(1)),
            ]),
            row(&[
                ("party", Value::string("A")),
                ("invoice", Value::string("i2")),
                ("amount", d(2)),
            ]),
            row(&[
                ("party", Value::string("B")),
                ("invoice", Value::string("i3")),
                ("amount", d(4)),
            ]),
        ]
    };
    let value_of = |row: &moqui_lib::object::ValueMap, name: &str| row.get(name).cloned().unwrap_or(Value::Null);
    assert_eq!(aggregation.apply(rows(), None, value_of).len(), 3);
    let shown: HashSet<String> = ["party", "amount"].into_iter().map(String::from).collect();
    let out = aggregation.apply(rows(), Some(&shown), value_of);
    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0].get("amount"), Some(&d(3)));
    assert_eq!(out[1].get("amount"), Some(&d(4)));
    let out = aggregation.apply(rows(), Some(&HashSet::new()), value_of);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0].get("amount"), Some(&d(7)));
}

/// A show-total that is not a function is logged and ignored, as a bad aggregate is (failure tolerance).
#[test]
fn a_bad_show_total_is_ignored_and_the_form_still_aggregates() {
    let node = form(
        r#"<form-list name="F" list="l">
                <field name="party" aggregate="group-by"/>
                <field name="n" aggregate="sum" show-total="tally"/>
            </form-list>"#,
    );
    let aggregation = Aggregation::of(&node).unwrap().unwrap();
    assert_eq!(aggregation.fields[1].show_total, None);
    assert_eq!(aggregation.fields[1].function, Some(Function::Sum));
}

#[test]
fn show_total_alone_adds_a_total_row_and_aggregate_needs_a_group() {
    let node = form(r#"<form-list name="F" list="l"><field name="n" show-total="max"/><field name="m"/></form-list>"#);
    let aggregation = Aggregation::of(&node).unwrap().unwrap();
    let rows = vec![
        row(&[("n", Value::I32(4))]),
        row(&[("n", Value::I32(9))]),
        row(&[("n", Value::I32(2))]),
    ];
    let out = aggregation.apply(rows, None, |row, name| row.get(name).cloned().unwrap_or(Value::Null));
    assert_eq!(out.len(), 4);
    assert_eq!(out[3].get("n"), Some(&Value::I32(9)));
    assert!(
        Aggregation::of(&form(r#"<form-list name="F"><field name="n"/></form-list>"#))
            .unwrap()
            .is_none()
    );
    let err = Aggregation::of(&form(
        r#"<form-list name="F"><field name="n" aggregate="sum"/></form-list>"#,
    ))
    .unwrap_err();
    assert!(err.to_string().contains("no group-by"), "{err}");
}

#[test]
fn avg_skips_a_value_whose_total_overflows() {
    let mut map = IndexMap::default();
    let max = Value::Decimal(Decimal::MAX);
    apply_function(Function::Avg, &mut map, "n", &max);
    apply_function(Function::Avg, &mut map, "n", &max);
    assert_eq!(map.get("n"), Some(&max));
    assert_eq!(map.get("nCount"), Some(&Value::Decimal(Decimal::ONE)));
    apply_function(Function::Avg, &mut map, "n", &Value::Decimal(Decimal::from(-1)));
    assert_eq!(map.get("nCount"), Some(&Value::Decimal(Decimal::TWO)));
}
