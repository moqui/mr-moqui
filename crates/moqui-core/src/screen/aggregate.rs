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
            let show_total = field.with_attribute("show-total", |value| match value {
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
            });
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
        rows: Vec<moqui_lib::object::ValueMap>,
        include: Option<&HashSet<String>>,
        value_of: impl Fn(&moqui_lib::object::ValueMap, &str) -> Value,
    ) -> Vec<moqui_lib::object::ValueMap> {
        let make_sub_list = !self.group_fields.is_empty();
        let mut result: Vec<moqui_lib::object::ValueMap> = Vec::new();
        let mut groups: Vec<(Vec<Value>, usize)> = Vec::new();
        let mut totals: moqui_lib::object::ValueMap = IndexMap::default();
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
                    let mut sub: Option<moqui_lib::object::ValueMap> = None;
                    let mut sub_totals: Option<moqui_lib::object::ValueMap> = None;
                    for field in &self.fields {
                        let value = value_of(&row, &field.name);
                        if value.is_null() {
                            continue;
                        }
                        if make_sub_list && field.sub_list {
                            sub.get_or_insert_with(Default::default)
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
                                    sub_totals.get_or_insert_with(Default::default),
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
                    let mut sub: Option<moqui_lib::object::ValueMap> = None;
                    for field in &self.fields {
                        let value = value_of(&row, &field.name);
                        if value.is_null() {
                            continue;
                        }
                        if field.sub_list {
                            sub.get_or_insert_with(Default::default)
                                .insert(field.name.clone(), value.clone());
                        } else if let Some(function) = field.function {
                            apply_function(function, out, &field.name, &value);
                        }
                        if let Some(total) = field.show_total {
                            if field.sub_list {
                                let mut sub_totals = match out.shift_remove("aggregateSubListTotals") {
                                    Some(Value::Map(map)) => map,
                                    _ => IndexMap::default(),
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
fn apply_function(function: Function, map: &mut moqui_lib::object::ValueMap, name: &str, value: &Value) {
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
#[path = "aggregate_tests.rs"]
mod tests;
