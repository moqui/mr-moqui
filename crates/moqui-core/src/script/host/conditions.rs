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

//! Entity conditions: `ec.entity.conditionFactory` ([`CONDITION_FACTORY`]) and the condition objects it makes
//! ([`CONDITION`]), over the helpers the finds of [`super::entities`] use too (a comparison from Java's operator names,
//! the date range of `conditionDate` and `filterByDate`, the condition an argument holds), and
//! `ec.entity.getDataDocuments`, which takes a condition.

use moqui_db::sql::{CompareOp, Condition};
use moqui_lib::object::{HostObject, Value};

use super::{method, Call, HostClass, HostError, Method, Ret};

/// The condition a host value holds: a condition object a script made (`conditionFactory.makeCondition`), else none.
pub(crate) fn condition_in(value: &Value) -> Option<Condition> {
    match value {
        Value::Object(object) => object
            .downcast::<Condition>()
            .map(|condition| Condition::clone(&condition)),
        _ => None,
    }
}

/// The items of an `in` or `between` value: a list, comma separated text, or one value; none for null.
pub(crate) fn list_values(value: Value) -> Vec<Value> {
    match value {
        Value::List(items) => items,
        Value::String(text) => text.split(',').map(|part| Value::string(part.trim())).collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

/// The name this port compares by for any name Java's `stringComparisonOperatorMap` has: `less-than`, `not-equal`,
/// `IS NULL`, and the other spellings. A name that is in neither is returned as it is, for the error to name.
fn normalized_operator(operator: &str) -> &str {
    match operator {
        "" => "equals",
        "not-equal" => "not-equals",
        "less-than" => "less",
        "greater-than" => "greater",
        "less-than-equal-to" => "less-equals",
        "greater-than-equal-to" => "greater-equals",
        "IN" => "in",
        "NOT IN" => "not-in",
        "BETWEEN" => "between",
        "NOT BETWEEN" => "not-between",
        "LIKE" => "like",
        "NOT LIKE" => "not-like",
        "IS NULL" => "is-null",
        "IS NOT NULL" => "is-not-null",
        other => other,
    }
}

/// One comparison from an operator name. Equality with null is `IS NULL`, as the Java factory does.
pub(crate) fn comparison(field: &str, operator: &str, value: Value) -> Result<Condition, moqui_lib::error::Error> {
    let operator = normalized_operator(operator);
    let condition = match operator {
        "equals" | "=" if value.is_null() => Condition::is_null(field),
        "equals" | "=" => Condition::equals(field, value),
        "not-equals" | "!=" | "<>" if value.is_null() => Condition::is_not_null(field),
        "not-equals" | "!=" | "<>" => Condition::not_equals(field, value),
        "less" | "<" => Condition::less(field, value),
        "greater" | ">" => Condition::greater(field, value),
        "less-equals" | "<=" => Condition::less_equal(field, value),
        "greater-equals" | ">=" => Condition::greater_equal(field, value),
        "like" => Condition::like(field, value),
        "not-like" => Condition::not_like(field, value),
        "in" => Condition::is_in(field, list_values(value)),
        "not-in" => Condition::not_in(field, list_values(value)),
        "between" => {
            let mut bounds = list_values(value).into_iter();
            match (bounds.next(), bounds.next()) {
                (Some(low), Some(high)) => Condition::between(field, low, high),
                _ => return Err(moqui_lib::error::Error::new("between needs a low and a high value")),
            }
        }
        "is-null" => Condition::is_null(field),
        "is-not-null" => Condition::is_not_null(field),
        other => {
            return Err(moqui_lib::error::Error::new(format!(
                "unknown condition operator {other}"
            )));
        }
    };
    Ok(condition)
}

/// A time stamp from a script's value: epoch millis, a time stamp, or text, which is in the session time zone unless it
/// has an offset.
pub(crate) fn timestamp_of(value: &Value) -> Value {
    crate::l10n::basic_convert(value, "java.sql.Timestamp").value
}

/// The context's effective time, or the clock (`ec.user.nowTimestamp`).
pub(crate) fn now_timestamp() -> Value {
    timestamp_of(&Value::I64(crate::user::now_effective_millis()))
}

/// `(from is null or from <= valid) and (thru is null or thru > valid)`. An empty `valid` is now, or no condition with
/// `ignore_if_empty`.
pub(crate) fn date_range_condition(
    from_name: &str,
    thru_name: &str,
    valid: Value,
    ignore_if_empty: bool,
) -> Option<Condition> {
    let valid = if moqui_lib::object::is_empty(&valid) {
        if ignore_if_empty {
            return None;
        }
        now_timestamp()
    } else {
        timestamp_of(&valid)
    };
    let started = Condition::or(vec![
        Condition::is_null(from_name),
        Condition::less_equal(from_name, valid.clone()),
    ]);
    let open = Condition::or(vec![
        Condition::is_null(thru_name),
        Condition::greater(thru_name, valid),
    ]);
    Some(Condition::and(vec![started, open]))
}

/// `ec.entity.conditionFactory`: conditions a script builds for an entity find.
pub(crate) static CONDITION_FACTORY: HostClass = HostClass {
    name: "host:EntityConditionFactory",
    methods: &[
        method!(
            "ec.entity.conditionFactory",
            "makeCondition",
            [Value, OptValue, OptValue, OptValue, OptValue],
            // an entity value's localized fields are finds
            make_condition
        ),
        method!(
            "ec.entity.conditionFactory",
            "makeConditionWhere",
            [OptValue],
            None,
            leaf,
            make_condition_where
        ),
        method!(
            "ec.entity.conditionFactory",
            "makeConditionDate",
            [Text, Text, Value, OptBool, OptValue],
            None,
            leaf,
            make_condition_date
        ),
        method!(
            "ec.entity.conditionFactory",
            "makeActionConditionDirect",
            [OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue, OptValue],
            None,
            leaf,
            make_action_condition
        ),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// A condition node a script passes to `EntityFind.condition`.
pub(crate) static CONDITION: HostClass = HostClass {
    name: "host:EntityCondition",
    methods: &[method!("EntityCondition", "ignoreCase", [], None, leaf, ignore_case)],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// A condition as a script value, or null.
pub(crate) fn condition_ret(condition: Option<Condition>) -> Ret {
    match condition {
        Some(condition) => Ret::Object(HostObject::new(condition), &CONDITION),
        None => Ret::Value(Value::Null),
    }
}

/// The conditions joined: none, the one, or an `and` (an `or` for the operator `or`) of them.
pub(crate) fn join(conditions: Vec<Condition>, operator: &str) -> Option<Condition> {
    let mut conditions = conditions;
    match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ if operator.eq_ignore_ascii_case("or") => Some(Condition::or(conditions)),
        _ => Some(Condition::and(conditions)),
    }
}

/// A comparison with a value can ignore case; a null test has no text to fold.
fn ignorable(condition: &Condition) -> bool {
    !matches!(condition, Condition::IsNull(_) | Condition::IsNotNull(_))
}

/// Java's `EntityCondition.ignoreCase()`: compare text without regard to case. A null test is as it was.
fn ignore_case(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let inner = Condition::clone(&*call.this::<Condition>()?);
    Ok(condition_ret(Some(if ignorable(&inner) {
        Condition::ignore_case(inner)
    } else {
        inner
    })))
}

/// The text of a string argument, empty for anything else.
fn text_or_blank(call: &Call<'_>, index: usize) -> String {
    match call.value(index) {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The text of a value that must be a string.
fn string_value(value: &Value) -> Result<String, HostError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        _ => Err(HostError::message("expected a string")),
    }
}

/// Whether an argument is `true` itself.
fn is_true(call: &Call<'_>, index: usize) -> bool {
    matches!(call.value(index), Some(Value::Bool(true)))
}

/// `makeCondition(field, operator, value, orNull)`, `makeCondition(listOfConditions, "and"|"or")`, or
/// `makeCondition(map)` as an AND of equalities.
fn make_condition(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let first = call.value_or_null(0);
    match &first {
        Value::List(items) => {
            let parts = items.iter().filter_map(condition_in).collect();
            Ok(condition_ret(join(parts, &text_or_blank(call, 1))))
        }
        Value::String(field) => {
            let operator = text_or_blank(call, 1);
            let mut condition = comparison(field, &operator, call.value_or_null(2)).map_err(HostError::Lib)?;
            // The fifth argument is not Java's: `makeCondition(...).ignoreCase()` is, and does the same.
            if is_true(call, 4) && ignorable(&condition) {
                condition = Condition::ignore_case(condition);
            }
            // Java's fourth argument: `orNull`.
            if is_true(call, 3) {
                condition = Condition::or(vec![condition, Condition::is_null(field.as_str())]);
            }
            Ok(condition_ret(Some(condition)))
        }
        Value::Map(_) | Value::Object(_) => {
            // a map's entries, an entity value's fields; another object (a condition) has none
            let entries = crate::script::value_map(&first).unwrap_or_default();
            let mut parts = Vec::new();
            for (key, value) in entries {
                parts.push(comparison(&key, "equals", value).map_err(HostError::Lib)?);
            }
            Ok(condition_ret(join(parts, "and")))
        }
        _ => Err(HostError::message("makeCondition needs a field, a list, or a map")),
    }
}

/// `makeConditionWhere(sql)`: a SQL fragment used as written, null for empty text.
fn make_condition_where(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = match call.value(0) {
        None | Some(Value::Null) => String::new(),
        Some(value) => string_value(value)?,
    };
    if text.is_empty() {
        return Ok(Ret::Value(Value::Null));
    }
    Ok(condition_ret(Some(Condition::where_sql(text))))
}

/// The `ignore` argument of `makeActionConditionDirect` and `makeConditionDate`: the generated code passes the value of
/// the element's `ignore` expression, so a flag; the text "true" is a flag too, other text by its truthiness.
fn ignore_flag(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.eq_ignore_ascii_case("true"),
        other => super::truthy(other),
    }
}

/// From and thru date columns that cover `validDate`, or now: `(from is null or from <= d) and (thru is null or thru >
/// d)`.
fn make_condition_date(call: &mut Call<'_>) -> Result<Ret, HostError> {
    if call.passed(4) && ignore_flag(&call.value_or_null(4)) {
        return Ok(Ret::Value(Value::Null));
    }
    Ok(condition_ret(date_range_condition(
        &call.text_or_empty(0),
        &call.text_or_empty(1),
        call.value_or_null(2),
        call.bool(3).unwrap_or(false),
    )))
}

/// The condition one `econdition` element describes. A skipped condition is null.
///
/// Arguments: field, operator, from, value, to-field, ignore-case, ignore-if-empty, or-null, ignore.
fn make_action_condition(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let field = string_value(&call.value_or_null(0))?;
    let operator = string_value(&call.value_or_null(1))?;
    if ignore_flag(&call.value_or_null(8)) {
        return Ok(Ret::Value(Value::Null));
    }
    if let Some(to_field) = call.value(4).filter(|value| !value.is_null()) {
        // Field against field. Case does not apply to a column compared with a column here.
        let other = string_value(to_field)?;
        let op = compare_op(&operator).map_err(HostError::Lib)?;
        let mut condition = Condition::compare_fields(field.as_str(), op, other.as_str());
        if is_true(call, 7) {
            condition = Condition::or(vec![condition, Condition::is_null(field.as_str())]);
        }
        return Ok(condition_ret(Some(condition)));
    }
    let from = call.value_or_null(2);
    let operand = if from.is_null() { call.value_or_null(3) } else { from };
    if is_true(call, 6) && moqui_lib::object::is_empty(&operand) {
        return Ok(Ret::Value(Value::Null));
    }
    let mut condition = comparison(&field, &operator, operand).map_err(HostError::Lib)?;
    if is_true(call, 5) && ignorable(&condition) {
        condition = Condition::ignore_case(condition);
    }
    if is_true(call, 7) {
        condition = Condition::or(vec![condition, Condition::is_null(field.as_str())]);
    }
    Ok(condition_ret(Some(condition)))
}

fn compare_op(operator: &str) -> Result<CompareOp, moqui_lib::error::Error> {
    Ok(match operator {
        "equals" | "=" => CompareOp::Equals,
        "not-equals" | "!=" | "<>" => CompareOp::NotEquals,
        "less" | "<" => CompareOp::Less,
        "greater" | ">" => CompareOp::Greater,
        "less-equals" | "<=" => CompareOp::LessEqual,
        "greater-equals" | ">=" => CompareOp::GreaterEqual,
        other => {
            return Err(moqui_lib::error::Error::new(format!(
                "operator {other} cannot compare two fields"
            )));
        }
    })
}

/// `ec.entity.getDataDocuments(dataDocumentId, condition?, fromUpdateStamp?, thruUpdateStamp?)`. The condition is one
/// from `conditionFactory` or a map of alias to value. Its documents' expressions read the context, so the engine's
/// tracked values are written back and released first.
pub(super) static GET_DATA_DOCUMENTS: Method = method!(
    "ec.entity",
    "getDataDocuments",
    [Text, OptValue, OptValue, OptValue],
    Release,
    get_data_documents
);

fn get_data_documents(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let facade = super::entity_facade()?;
    let condition = match call.value(1) {
        None | Some(Value::Null) => None,
        Some(value) => match condition_in(value) {
            Some(condition) => Some(condition),
            None => match value {
                Value::Map(map) => join(
                    map.iter()
                        .map(|(field, value)| Condition::equals(field.as_str(), value.clone()))
                        .collect(),
                    "and",
                ),
                _ => return Err(HostError::message("getDataDocuments takes a condition or a map")),
            },
        },
    };
    let stamp = |index: usize| match call.arg(index) {
        super::Arg::Missing => None,
        _ => crate::search::postgres::compile::parse_date_bound(&call.value_or_null(index)),
    };
    let (from, thru) = (stamp(2), stamp(3));
    let documents = crate::entity::document::data_documents(&facade, &call.text_or_empty(0), condition, from, thru)
        .map_err(HostError::Lib)?;
    Ok(Ret::Value(Value::List(documents.into_iter().map(Value::Map).collect())))
}
