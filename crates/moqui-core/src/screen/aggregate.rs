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

//! Form-list aggregation, a port of `AggregationUtil.aggregateList`: a field's `aggregate` is `group-by`, `sub-list`, or
//! a function (`min`, `max`, `sum`, `avg`, `count`, `first`, `last`), and `show-total` (`true` is `sum`) adds a total.
//!
//! Rows with the same `group-by` values become one row: its function fields aggregate over the group, and the
//! `sub-list` fields of each original row are a map in its `aggregateSubList` (with `aggregateSubListTotals` last when
//! a sub-list field has a total). The totals of the other fields are one more row at the end, marked
//! `_moquiRowType: "total"`.

use std::cmp::Ordering;
use std::collections::HashSet;

use indexmap::IndexMap;
use moqui_lib::format::Decimal;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::context::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Function {
    Min,
    Max,
    Sum,
    Avg,
    Count,
    First,
    Last,
}

impl Function {
    fn of(text: &str) -> Option<Self> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "min" => Self::Min,
            "max" => Self::Max,
            "sum" => Self::Sum,
            "avg" => Self::Avg,
            "count" => Self::Count,
            "first" => Self::First,
            "last" => Self::Last,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AggregateField {
    pub name: String,
    pub function: Option<Function>,
    pub show_total: Option<Function>,
    pub sub_list: bool,
}

/// How a form-list aggregates its rows. `None` from [`Aggregation::of`] when it does not.
#[derive(Clone, Debug)]
pub(crate) struct Aggregation {
    pub fields: Vec<AggregateField>,
    pub group_fields: Vec<String>,
}

impl Aggregation {
    /// The aggregation a form-list's fields ask for: an `aggregate` or a `show-total` on any field.
    ///
    /// # Errors
    ///
    /// Returns an error when a field aggregates and none is `group-by`, as Java does.
    pub(crate) fn of(form: &MNode) -> Result<Option<Self>, Error> {
        let mut fields = Vec::new();
        let mut group_fields = Vec::new();
        let mut any = false;
        let mut aggregates = false;
        for field in form.children_named("field") {
            let name = field.attribute("name").unwrap_or_default();
            let show_total = match field.attribute("show-total").as_deref() {
                None | Some("" | "false") => None,
                Some("true") => Some(Function::Sum),
                Some(other) => {
                    let function = Function::of(other);
                    // Java's AggregateFunction.valueOf throws; here the field is logged and gets no total.
                    if function.is_none() {
                        tracing::error!(
                            "Ignoring show-total {other} on field {name} in form {}, not true, false, or a valid function",
                            form.attribute("name").unwrap_or_default()
                        );
                    }
                    function
                }
            };
            let aggregate = field.attribute("aggregate").unwrap_or_default();
            let group_by = aggregate == "group-by";
            let sub_list = aggregate == "sub-list";
            let function = if aggregate.is_empty() || group_by || sub_list {
                None
            } else {
                let function = Function::of(&aggregate);
                if function.is_none() {
                    tracing::error!(
                        "Ignoring aggregate {aggregate} on field {name} in form {}, not a valid function, group-by, or sub-list",
                        form.attribute("name").unwrap_or_default()
                    );
                }
                function
            };
            aggregates |= !aggregate.is_empty();
            any |= !aggregate.is_empty() || show_total.is_some();
            if group_by {
                group_fields.push(name.clone());
            }
            fields.push(AggregateField {
                name,
                function,
                show_total,
                sub_list,
            });
        }
        if aggregates && group_fields.is_empty() {
            return Err(Error::new(format!(
                "Form {} has aggregate fields but no group-by field, must have at least one",
                form.attribute("name").unwrap_or_default()
            )));
        }
        Ok(any.then_some(Self { fields, group_fields }))
    }

    /// Aggregate rows that row-actions and field `from` already made. `value_of` reads a field of a row when the row
    /// does not hold it (Java reads the context). With `include`, the fields the list shows (Java's `includeFields`), a
    /// `group-by` field that is not shown is left out of the group key; when none is shown, every row is one group.
    pub(crate) fn apply(
        &self,
        rows: Vec<IndexMap<String, Value>>,
        include: Option<&HashSet<String>>,
        value_of: impl Fn(&IndexMap<String, Value>, &str) -> Value,
    ) -> Vec<IndexMap<String, Value>> {
        let make_sub_list = !self.group_fields.is_empty();
        let mut result: Vec<IndexMap<String, Value>> = Vec::new();
        let mut groups: Vec<(Vec<Value>, usize)> = Vec::new();
        let mut totals: IndexMap<String, Value> = IndexMap::new();
        let has_sub_totals = self
            .fields
            .iter()
            .any(|field| field.sub_list && field.show_total.is_some());
        for row in rows {
            let group_key: Vec<Value> = if make_sub_list {
                self.group_fields
                    .iter()
                    .filter(|name| include.is_none_or(|shown| shown.contains(*name)))
                    .map(|name| value_of(&row, name))
                    .collect()
            } else {
                Vec::new()
            };
            let existing = if make_sub_list {
                groups
                    .iter()
                    .find(|(key, _)| keys_equal(key, &group_key))
                    .map(|(_, index)| *index)
            } else {
                None
            };
            match existing {
                None => {
                    let mut out = row.clone();
                    let mut sub: Option<IndexMap<String, Value>> = None;
                    let mut sub_totals: Option<IndexMap<String, Value>> = None;
                    for field in &self.fields {
                        let value = value_of(&row, &field.name);
                        if value.is_null() {
                            continue;
                        }
                        if make_sub_list && field.sub_list {
                            sub.get_or_insert_with(IndexMap::new)
                                .insert(field.name.clone(), value.clone());
                            out.shift_remove(&field.name);
                        } else if field.function == Some(Function::Count) {
                            out.insert(field.name.clone(), Value::I32(1));
                        } else if field.function == Some(Function::Avg) {
                            // Later rows read `{name}Count` and `{name}Total`. Seeding them here keeps the
                            // first value in the average. Copying the raw value leaves those counters empty.
                            apply_function(Function::Avg, &mut out, &field.name, &value);
                        } else {
                            out.insert(field.name.clone(), value.clone());
                        }
                        if let Some(total) = field.show_total {
                            if field.sub_list {
                                apply_function(
                                    total,
                                    sub_totals.get_or_insert_with(IndexMap::new),
                                    &field.name,
                                    &value,
                                );
                            } else {
                                apply_function(total, &mut totals, &field.name, &value);
                            }
                        }
                    }
                    if let Some(sub) = sub {
                        out.insert("aggregateSubList".to_string(), Value::List(vec![Value::Map(sub)]));
                    }
                    if let Some(sub_totals) = sub_totals {
                        out.insert("aggregateSubListTotals".to_string(), Value::Map(sub_totals));
                    }
                    if make_sub_list {
                        groups.push((group_key, result.len()));
                    }
                    result.push(out);
                }
                Some(index) => {
                    let out = &mut result[index];
                    let mut sub: Option<IndexMap<String, Value>> = None;
                    for field in &self.fields {
                        let value = value_of(&row, &field.name);
                        if value.is_null() {
                            continue;
                        }
                        if field.sub_list {
                            sub.get_or_insert_with(IndexMap::new)
                                .insert(field.name.clone(), value.clone());
                        } else if let Some(function) = field.function {
                            apply_function(function, out, &field.name, &value);
                        }
                        if let Some(total) = field.show_total {
                            if field.sub_list {
                                let mut sub_totals = match out.shift_remove("aggregateSubListTotals") {
                                    Some(Value::Map(map)) => map,
                                    _ => IndexMap::new(),
                                };
                                apply_function(total, &mut sub_totals, &field.name, &value);
                                out.insert("aggregateSubListTotals".to_string(), Value::Map(sub_totals));
                            } else {
                                apply_function(total, &mut totals, &field.name, &value);
                            }
                        }
                    }
                    if let Some(sub) = sub {
                        if let Some(Value::List(list)) = out.get_mut("aggregateSubList") {
                            list.push(Value::Map(sub));
                        }
                    }
                }
            }
        }
        if has_sub_totals {
            for out in &mut result {
                let Some(Value::Map(sub_totals)) = out.get("aggregateSubListTotals").cloned() else {
                    continue;
                };
                if let Some(Value::List(list)) = out.get_mut("aggregateSubList") {
                    list.push(Value::Map(sub_totals));
                }
            }
        }
        if !totals.is_empty() {
            totals.insert("_moquiRowType".to_string(), Value::string("total"));
            result.push(totals);
        }
        result
    }
}

fn keys_equal(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(a, b)| {
            a == b
                || moqui_lib::object::compare_obj(a, b).is_ok_and(|ordering| ordering == Ordering::Equal)
                || (a.is_null() && b.is_null())
        })
}

fn decimal_of(value: &Value) -> Option<Decimal> {
    moqui_lib::object::basic_convert(value, "BigDecimal").value.as_decimal()
}

/// `AggregationUtil.doFunction`.
fn apply_function(function: Function, map: &mut IndexMap<String, Value>, name: &str, value: &Value) {
    match function {
        Function::Min | Function::Max => match map.get(name) {
            None => {
                map.insert(name.to_string(), value.clone());
            }
            Some(existing) => {
                let ordering = moqui_lib::object::compare_obj(existing, value).unwrap_or(Ordering::Equal);
                if (function == Function::Min && ordering == Ordering::Greater)
                    || (function == Function::Max && ordering == Ordering::Less)
                {
                    map.insert(name.to_string(), value.clone());
                }
            }
        },
        Function::Sum => {
            let current = match value {
                Value::String(text) => match text.trim().parse::<Decimal>() {
                    Ok(number) => Value::Decimal(number),
                    Err(_) => {
                        tracing::warn!("Tried to sum non-number value {text} of field {name}");
                        return;
                    }
                },
                other => other.clone(),
            };
            let sum = match map.get(name) {
                None => current,
                Some(existing) => match moqui_lib::object::add_numbers(existing, &current) {
                    Ok(sum) => sum,
                    Err(err) => {
                        tracing::warn!("Tried to sum non-number value of field {name}: {err}");
                        return;
                    }
                },
            };
            map.insert(name.to_string(), sum);
        }
        Function::Avg => {
            let Some(number) = decimal_of(value) else {
                return;
            };
            let count_name = format!("{name}Count");
            let total_name = format!("{name}Total");
            let count = map.get(&count_name).and_then(decimal_of);
            let total = map.get(&total_name).and_then(decimal_of);
            match (count, total) {
                (Some(count), Some(total)) => {
                    let (Some(total), Some(count)) = (total.checked_add(number), count.checked_add(Decimal::ONE))
                    else {
                        tracing::warn!("Tried to average value {number} of field {name}: the total overflows");
                        return;
                    };
                    let average = moqui_lib::object::decimal_divide(total, count).unwrap_or(total);
                    map.insert(name.to_string(), Value::Decimal(average));
                    map.insert(count_name, Value::Decimal(count));
                    map.insert(total_name, Value::Decimal(total));
                }
                _ => {
                    map.insert(name.to_string(), Value::Decimal(number));
                    map.insert(count_name, Value::Decimal(Decimal::ONE));
                    map.insert(total_name, Value::Decimal(number));
                }
            }
        }
        Function::Count => {
            let count = match map.get(name) {
                Some(Value::I32(count)) => *count,
                _ => 0,
            };
            map.insert(name.to_string(), Value::I32(count + 1));
        }
        Function::First => {
            if !map.contains_key(name) {
                map.insert(name.to_string(), value.clone());
            }
        }
        Function::Last => {
            map.insert(name.to_string(), value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, Value)]) -> IndexMap<String, Value> {
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
        let value_of = |row: &IndexMap<String, Value>, name: &str| row.get(name).cloned().unwrap_or(Value::Null);
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
        let node =
            form(r#"<form-list name="F" list="l"><field name="n" show-total="max"/><field name="m"/></form-list>"#);
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
        let mut map = IndexMap::new();
        let max = Value::Decimal(Decimal::MAX);
        apply_function(Function::Avg, &mut map, "n", &max);
        apply_function(Function::Avg, &mut map, "n", &max);
        assert_eq!(map.get("n"), Some(&max));
        assert_eq!(map.get("nCount"), Some(&Value::Decimal(Decimal::ONE)));
        apply_function(Function::Avg, &mut map, "n", &Value::Decimal(Decimal::from(-1)));
        assert_eq!(map.get("nCount"), Some(&Value::Decimal(Decimal::TWO)));
    }
}
