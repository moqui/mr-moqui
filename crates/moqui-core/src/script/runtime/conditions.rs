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

//! `ec.entity.conditionFactory`: conditions a script builds for an entity find.

use super::*;

/// A condition node a script passes to `EntityFind.condition`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostCondition {
    #[qjs(skip_trace)]
    pub(super) inner: Condition,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostCondition {
    /// Java's `EntityCondition.ignoreCase()`: compare text without regard to case. A null test is as it was.
    fn ignore_case<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let inner = self.inner.clone();
            condition_js(
                &ctx,
                Some(if ignorable(&inner) {
                    Condition::ignore_case(inner)
                } else {
                    inner
                }),
            )
        })
    }
}

pub(super) fn condition_js<'js>(ctx: &Ctx<'js>, condition: Option<Condition>) -> rquickjs::Result<JsValue<'js>> {
    match condition {
        Some(inner) => Class::instance(ctx.clone(), HostCondition { inner }).and_then(|class| class.into_js(ctx)),
        None => Null.into_js(ctx),
    }
}

pub(super) fn condition_of(value: &JsValue<'_>) -> Option<Condition> {
    let object = value.as_object()?;
    let class = object.as_class::<HostCondition>()?;
    let inner = class.borrow().inner.clone();
    Some(inner)
}

pub(super) fn list_values(value: Value) -> Vec<Value> {
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
pub(super) fn comparison(field: &str, operator: &str, value: Value) -> Result<Condition, moqui_lib::error::Error> {
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

pub(super) fn join(conditions: Vec<Condition>, operator: &str) -> Option<Condition> {
    let mut conditions = conditions;
    match conditions.len() {
        0 => None,
        1 => conditions.pop(),
        _ if operator.eq_ignore_ascii_case("or") => Some(Condition::or(conditions)),
        _ => Some(Condition::and(conditions)),
    }
}

/// `makeCondition(field, operator, value, orNull)`, `makeCondition(listOfConditions, "and"|"or")`,
/// or `makeCondition(map)` as an AND of equalities.
pub(super) fn make_condition<'js>(
    ctx: Ctx<'js>,
    first: JsValue<'js>,
    second: Opt<JsValue<'js>>,
    third: Opt<JsValue<'js>>,
    or_null: Opt<JsValue<'js>>,
    ignore_case: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let as_text = |value: Option<&JsValue<'js>>| -> rquickjs::Result<String> {
            match value {
                Some(value) if value.is_string() => js_string(&ctx, value),
                _ => Ok(String::new()),
            }
        };
        if let Some(items) = first.as_array() {
            let mut parts = Vec::new();
            for item in items.iter::<JsValue>() {
                if let Some(condition) = condition_of(&item?) {
                    parts.push(condition);
                }
            }
            let operator = as_text(second.0.as_ref())?;
            return condition_js(&ctx, join(parts, &operator));
        }
        if first.is_string() {
            let field = js_string(&ctx, &first)?;
            let operator = as_text(second.0.as_ref())?;
            let value = match &third.0 {
                Some(value) => js_to_host(&ctx, value)?,
                None => Value::Null,
            };
            let mut condition = comparison(&field, &operator, value).map_err(|err| throw_lib(&ctx, &err))?;
            // The fifth argument is not Java's: `makeCondition(...).ignoreCase()` is, and does the same.
            if ignore_case.0.as_ref().and_then(JsValue::as_bool) == Some(true) && ignorable(&condition) {
                condition = Condition::ignore_case(condition);
            }
            // Java's fourth argument: `orNull`.
            if or_null.0.as_ref().and_then(JsValue::as_bool) == Some(true) {
                condition = Condition::or(vec![condition, Condition::is_null(field.as_str())]);
            }
            return condition_js(&ctx, Some(condition));
        }
        if let Some(object) = first.as_object() {
            let mut parts = Vec::new();
            for key in object.keys::<String>() {
                let key = key?;
                let property: JsValue = object.get(key.as_str())?;
                let value = js_to_host(&ctx, &property)?;
                parts.push(comparison(&key, "equals", value).map_err(|err| throw_lib(&ctx, &err))?);
            }
            return condition_js(&ctx, join(parts, "and"));
        }
        Err(Exception::throw_message(
            &ctx,
            "makeCondition needs a field, a list, or a map",
        ))
    })
}

/// `makeConditionWhere(sql)`: a SQL fragment used as written, null for empty text.
pub(super) fn make_condition_where<'js>(ctx: Ctx<'js>, sql: Opt<JsValue<'js>>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let text = match sql.0.as_ref() {
            Some(value) if !value.is_null() && !value.is_undefined() => js_string(&ctx, value)?,
            _ => String::new(),
        };
        if text.is_empty() {
            return Null.into_js(&ctx);
        }
        condition_js(&ctx, Some(Condition::where_sql(text)))
    })
}

pub(super) fn truthy_text(value: &str) -> bool {
    value.eq_ignore_ascii_case("true")
}

/// The `ignore` argument of `makeActionConditionDirect` and `makeConditionDate`: the generated code passes the value
/// of the element's `ignore` expression, so a flag; the text "true" is a flag too.
fn ignore_flag(value: &JsValue<'_>) -> bool {
    if let Some(flag) = value.as_bool() {
        return flag;
    }
    if let Some(text) = value.as_string() {
        return text.to_string().is_ok_and(|text| truthy_text(&text));
    }
    js_truthy(value)
}

/// From and thru date columns that cover `validDate`, or now: `(from is null or from <= d) and (thru is null or thru > d)`.
pub(super) fn make_condition_date<'js>(
    ctx: Ctx<'js>,
    from_name: String,
    thru_name: String,
    valid: JsValue<'js>,
    ignore_if_empty: Opt<bool>,
    ignore: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        if ignore.0.as_ref().is_some_and(ignore_flag) {
            return Null.into_js(&ctx);
        }
        let valid = js_to_host(&ctx, &valid)?;
        condition_js(
            &ctx,
            date_range_condition(&from_name, &thru_name, valid, ignore_if_empty.0.unwrap_or(false)),
        )
    })
}

/// `(from is null or from <= valid) and (thru is null or thru > valid)`. An empty `valid` is now, or no condition with
/// `ignore_if_empty`.
pub(super) fn date_range_condition(
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

/// The condition one `econdition` element describes. A skipped condition is null.
///
/// Arguments: field, operator, from, value, to-field, ignore-case, ignore-if-empty, or-null, ignore.
pub(super) fn make_action_condition<'js>(ctx: Ctx<'js>, args: Rest<JsValue<'js>>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let arg = |index: usize| {
            args.0
                .get(index)
                .cloned()
                .unwrap_or_else(|| Undefined.into_value(ctx.clone()))
        };
        let flag = |index: usize| arg(index).as_bool().unwrap_or(false);
        let field = js_string(&ctx, &arg(0))?;
        let operator = js_string(&ctx, &arg(1))?;
        if ignore_flag(&arg(8)) {
            return Null.into_js(&ctx);
        }
        let to_field = arg(4);
        let ignore_case = flag(5);
        if !to_field.is_null() && !to_field.is_undefined() {
            // Field against field. Case does not apply to a column compared with a column here.
            let other = js_string(&ctx, &to_field)?;
            let op = compare_op(&operator).map_err(|err| throw_lib(&ctx, &err))?;
            let mut condition = Condition::compare_fields(field.as_str(), op, other.as_str());
            if flag(7) {
                condition = Condition::or(vec![condition, Condition::is_null(field.as_str())]);
            }
            return condition_js(&ctx, Some(condition));
        }
        let from = js_to_host(&ctx, &arg(2))?;
        let operand = if from.is_null() {
            js_to_host(&ctx, &arg(3))?
        } else {
            from
        };
        if flag(6) && moqui_lib::object::is_empty(&operand) {
            return Null.into_js(&ctx);
        }
        let mut condition = comparison(&field, &operator, operand).map_err(|err| throw_lib(&ctx, &err))?;
        if ignore_case && ignorable(&condition) {
            condition = Condition::ignore_case(condition);
        }
        if flag(7) {
            condition = Condition::or(vec![condition, Condition::is_null(field.as_str())]);
        }
        condition_js(&ctx, Some(condition))
    })
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

/// A comparison with a value can ignore case; a null test has no text to fold.
fn ignorable(condition: &Condition) -> bool {
    !matches!(condition, Condition::IsNull(_) | Condition::IsNotNull(_))
}
