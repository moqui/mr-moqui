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
        "not-between" => {
            let mut bounds = list_values(value).into_iter();
            match (bounds.next(), bounds.next()) {
                (Some(low), Some(high)) => Condition::not_between(field, low, high),
                _ => return Err(moqui_lib::error::Error::new("not-between needs a low and a high value")),
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
    crate::l10n::basic_convert(value, "Timestamp").value
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
            "makeConditionToField",
            [Text, Text, Text],
            None,
            leaf,
            make_condition_to_field
        ),
        method!(
            "ec.entity.conditionFactory",
            "getTrueCondition",
            [],
            None,
            leaf,
            true_condition
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
    shim_js: Some(FACTORY_JS),
    shim_members: &["comparisonOperatorFromEnumId"],
};

/// Java's `comparisonOperatorFromEnumId`: the operator name of a `ComparisonOperator` enumeration id, null for
/// another id. Operators are names here, so the result is what `makeCondition` takes.
const FACTORY_JS: &str = r#"(function (proto) {
    const operators = { ENTCO_LESS: "less", ENTCO_GREATER: "greater", ENTCO_LESS_EQ: "less-equals",
        ENTCO_GTR_EQ: "greater-equals", ENTCO_EQUALS: "equals", ENTCO_NOT_EQUALS: "not-equals", ENTCO_IN: "in",
        ENTCO_NOT_IN: "not-in", ENTCO_BETWEEN: "between", ENTCO_NOT_BETWEEN: "not-between", ENTCO_LIKE: "like",
        ENTCO_NOT_LIKE: "not-like", ENTCO_IS_NULL: "is-null", ENTCO_IS_NOT_NULL: "is-not-null" };
    Object.defineProperty(proto, "comparisonOperatorFromEnumId", { writable: true, configurable: true,
        value: function comparisonOperatorFromEnumId(enumId) { return operators[String(enumId)] ?? null; } });
})"#;

/// A condition node a script passes to `EntityFind.condition`.
pub(crate) static CONDITION: HostClass = HostClass {
    name: "host:EntityCondition",
    methods: &[
        method!("EntityCondition", "ignoreCase", [], None, leaf, ignore_case),
        method!("EntityCondition", "mapMatches", [Value], None, leaf, map_matches),
        method!("EntityCondition", "mapMatchesAny", [Value], None, leaf, map_matches_any),
        method!(
            "EntityCondition",
            "mapKeysNotContained",
            [Value],
            None,
            leaf,
            map_keys_not_contained
        ),
        method!("EntityCondition", "populateMap", [OptValue], None, leaf, populate_map),
        method!("EntityCondition", "toString", [], None, leaf, condition_text),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// The fields of a map or entity value argument.
fn fields_of(value: Option<&Value>) -> crate::entity::FieldMap {
    match value {
        Some(Value::Map(entries)) => entries
            .iter()
            .map(|(name, value)| (std::sync::Arc::from(name.as_str()), value.clone()))
            .collect(),
        Some(other) => crate::script::value_map(other)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| (std::sync::Arc::from(name.as_str()), value))
            .collect(),
        None => crate::entity::FieldMap::new(),
    }
}

/// Java's `mapMatches`: whether a map (or entity value) of fields meets the condition.
fn map_matches(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let condition = call.this::<Condition>()?;
    Ok(crate::entity::cache::condition_matches(&condition, &fields_of(call.value(0))).into())
}

/// The comparisons of a condition, flattened: each part of an and, an or and a not.
fn leaves(condition: &Condition, into: &mut Vec<Condition>) {
    match condition {
        Condition::And(parts) | Condition::Or(parts) => {
            for part in parts {
                leaves(part, into);
            }
        }
        Condition::Not(inner) => leaves(inner, into),
        other => into.push(other.clone()),
    }
}

/// Java's `mapMatchesAny`: whether the map meets any one comparison of the condition.
fn map_matches_any(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let condition = call.this::<Condition>()?;
    let fields = fields_of(call.value(0));
    let mut parts = Vec::new();
    leaves(&condition, &mut parts);
    Ok(parts
        .iter()
        .any(|part| crate::entity::cache::condition_matches(part, &fields))
        .into())
}

/// Java's `mapKeysNotContained`: whether the condition names none of the map's keys.
fn map_keys_not_contained(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let condition = call.this::<Condition>()?;
    let fields = fields_of(call.value(0));
    Ok((!condition.any_field(&mut |field| fields.contains_key(field.name.as_str()))).into())
}

/// Java's `populateMap`: the fields the condition requires to equal a value, put in the map given (or a new one),
/// which is returned. Only equalities joined by and say what a field must be.
fn populate_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    fn equalities(condition: &Condition, into: &mut moqui_lib::object::ValueMap) {
        match condition {
            Condition::Equals(field, value) => {
                into.insert(field.name.clone(), value.clone());
            }
            Condition::And(parts) => {
                for part in parts {
                    equalities(part, into);
                }
            }
            _ => {}
        }
    }
    let condition = call.this::<Condition>()?;
    let mut map = match call.value(0) {
        Some(Value::Map(entries)) => entries.clone(),
        _ => moqui_lib::object::ValueMap::default(),
    };
    equalities(&condition, &mut map);
    Ok(Ret::Value(Value::Map(map)))
}

/// The condition as text, for a log line: its structure with field names and values, not the SQL a dialect writes.
fn condition_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    fn text(condition: &Condition) -> String {
        let value = moqui_lib::object::to_plain_string;
        match condition {
            Condition::Equals(field, expected) => format!("{} = {}", field.name, value(expected)),
            Condition::NotEquals(field, expected) => format!("{} <> {}", field.name, value(expected)),
            Condition::Less(field, expected) => format!("{} < {}", field.name, value(expected)),
            Condition::Greater(field, expected) => format!("{} > {}", field.name, value(expected)),
            Condition::LessEqual(field, expected) => format!("{} <= {}", field.name, value(expected)),
            Condition::GreaterEqual(field, expected) => format!("{} >= {}", field.name, value(expected)),
            Condition::In(field, items) | Condition::NotIn(field, items) => {
                let items: Vec<String> = items.iter().map(value).collect();
                let not = if matches!(condition, Condition::NotIn(..)) {
                    "NOT "
                } else {
                    ""
                };
                format!("{} {not}IN ({})", field.name, items.join(", "))
            }
            Condition::Between(field, low, high) => {
                format!("{} BETWEEN {} AND {}", field.name, value(low), value(high))
            }
            Condition::NotBetween(field, low, high) => {
                format!("{} NOT BETWEEN {} AND {}", field.name, value(low), value(high))
            }
            Condition::Like(field, pattern) => format!("{} LIKE {}", field.name, value(pattern)),
            Condition::NotLike(field, pattern) => format!("{} NOT LIKE {}", field.name, value(pattern)),
            Condition::Matches(field, words) => format!("{} MATCHES {}", field.name, value(words)),
            Condition::IsNull(field) => format!("{} IS NULL", field.name),
            Condition::IsNotNull(field) => format!("{} IS NOT NULL", field.name),
            Condition::And(parts) | Condition::Or(parts) => {
                let joiner = if matches!(condition, Condition::Or(_)) {
                    " OR "
                } else {
                    " AND "
                };
                let parts: Vec<String> = parts.iter().map(text).collect();
                format!("({})", parts.join(joiner))
            }
            Condition::Not(inner) => format!("NOT ({})", text(inner)),
            Condition::CompareFields(left, _, right) => format!("{} ~ {}", left.name, right.name),
            Condition::IgnoreCase(inner) => format!("IGNORE CASE ({})", text(inner)),
            Condition::Where(sql) => sql.clone(),
            Condition::Bound(field, _) => format!("{} ~ ?", field.name),
        }
    }
    let condition = call.this::<Condition>()?;
    Ok(text(&condition).into())
}

/// Java's `makeConditionToField(field, operator, toField)`: one field against another.
fn make_condition_to_field(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let op = compare_op(&call.text_or_empty(1)).map_err(HostError::Lib)?;
    Ok(condition_ret(Some(Condition::compare_fields(
        call.text_or_empty(0).as_str(),
        op,
        call.text_or_empty(2).as_str(),
    ))))
}

/// Java's `getTrueCondition()`: a condition every row meets.
fn true_condition(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(condition_ret(Some(Condition::where_sql("1 = 1"))))
}

/// The entries of a map as comparisons with `operator`, joined by `join_operator`.
fn map_condition(
    entries: moqui_lib::object::ValueMap,
    operator: &str,
    join_operator: &str,
) -> Result<Option<Condition>, HostError> {
    let operator = if operator.is_empty() { "equals" } else { operator };
    let mut parts = Vec::new();
    for (key, value) in entries {
        parts.push(comparison(&key, operator, value).map_err(HostError::Lib)?);
    }
    Ok(join(parts, join_operator))
}

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

/// Java's forms: `makeCondition(field, operator, value, orNull)`, `makeCondition(condition, "and"|"or", condition)`,
/// `makeCondition(listOfConditions, "and"|"or", mapOperator, mapJoinOperator)` (a map in the list is its entries
/// compared and joined by the last two, equals and and by default), and
/// `makeCondition(map, operator, joinOperator)`, an AND of equalities by default.
fn make_condition(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let first = call.value_or_null(0);
    if let Some(left) = condition_in(&first) {
        let parts = match call.value(2).and_then(condition_in) {
            Some(right) => vec![left, right],
            None => vec![left],
        };
        return Ok(condition_ret(join(parts, &text_or_blank(call, 1))));
    }
    match &first {
        Value::List(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                if let Some(condition) = condition_in(item) {
                    parts.push(condition);
                } else if let Value::Map(entries) = item {
                    let of_map = map_condition(entries.clone(), &text_or_blank(call, 2), &text_or_blank(call, 3))?;
                    parts.extend(of_map);
                }
            }
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
            // a map's entries, an entity value's fields
            let entries = crate::script::value_map(&first).unwrap_or_default();
            Ok(condition_ret(map_condition(
                entries,
                &text_or_blank(call, 1),
                &text_or_blank(call, 2),
            )?))
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
