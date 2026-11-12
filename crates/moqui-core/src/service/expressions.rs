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

//! `org.moqui.impl.ExpressionServices`: `validate#DecimalExpression` and `evaluate#DecimalExpression`, the decimal
//! evaluator of payroll formulas (IronStone People design PE-09; docs/dev/JS-Framework-Services.md "Expressions") over
//! [`moqui_lib::decimal_expr`]. Neither reads anything but its parameters. A refused expression is answered in the
//! out-parameters (`valid` false, `errorCode`, `errorMessage`, `errorPosition`) and is not a service error, so a caller
//! checking a formula keeps its transaction; only a parameter of the wrong type is an error.

use std::collections::BTreeMap;

use moqui_lib::decimal_expr::{self, ExprError, Expression};
use moqui_lib::error::Error as LibError;
use moqui_lib::format::Decimal;
use moqui_lib::object::Value;

use crate::entity::EntityFacade;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, validate_decimal_expression)?;
    rust_service!(map, evaluate_decimal_expression)?;
    Ok(())
}

fn text_list(values: &[String]) -> Value {
    Value::List(values.iter().map(|value| Value::string(value.as_str())).collect())
}

/// `inputNames`, when given: a list of names (or a comma-separated text).
fn input_names(parameters: &BTreeMap<String, Value>) -> Result<Option<Vec<String>>, LibError> {
    match parameters.get("inputNames") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::List(items)) => items
            .iter()
            .map(|item| match item {
                Value::String(name) => Ok(name.trim().to_string()),
                _ => Err(LibError::new("inputNames is a list of names")),
            })
            .collect::<Result<Vec<String>, LibError>>()
            .map(Some),
        Some(Value::String(names)) => Ok(Some(
            names
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect(),
        )),
        Some(_) => Err(LibError::new("inputNames is a list of names")),
    }
}

fn expression_text(parameters: &BTreeMap<String, Value>) -> Result<String, LibError> {
    match parameters.get("expression") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err(LibError::new("expression is a text")),
    }
}

/// The refusal's out-parameters beside `valid` false.
fn refused(out: &mut BTreeMap<String, Value>, error: &ExprError) {
    out.insert("valid".into(), Value::Bool(false));
    out.insert("errorCode".into(), Value::string(error.code));
    out.insert("errorMessage".into(), Value::string(error.message.as_str()));
    out.insert(
        "errorPosition".into(),
        error
            .position
            .and_then(|position| i32::try_from(position).ok())
            .map_or(Value::Null, Value::I32),
    );
}

fn parsed(parameters: &BTreeMap<String, Value>) -> Result<Result<Expression, ExprError>, LibError> {
    let text = expression_text(parameters)?;
    let names = input_names(parameters)?;
    Ok(decimal_expr::parse(&text, names.as_deref()))
}

/// `validate#DecimalExpression`: `valid`, the `names` the expression reads and the `functions` it calls, or the
/// refusal.
fn validate_decimal_expression(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    match parsed(parameters)? {
        Ok(expression) => {
            out.insert("valid".into(), Value::Bool(true));
            out.insert("names".into(), text_list(expression.names()));
            out.insert("functions".into(), text_list(expression.functions()));
        }
        Err(error) => refused(&mut out, &error),
    }
    Ok(out)
}

/// An input's decimal: a decimal, a whole number, a script number or decimal text. `Ok(None)` for null.
fn input_decimal(value: &Value) -> Result<Option<Decimal>, ()> {
    match value {
        Value::Null => Ok(None),
        Value::Decimal(number) => Ok(Some(*number)),
        Value::I32(number) => Ok(Some(Decimal::from(*number))),
        Value::I64(number) => Ok(Some(Decimal::from(*number))),
        Value::F64(number) if number.is_finite() => {
            decimal_expr::decimal_input(&number.to_string()).map(Some).ok_or(())
        }
        Value::String(text) => decimal_expr::decimal_input(text).map(Some).ok_or(()),
        _ => Err(()),
    }
}

/// `evaluate#DecimalExpression`: `valid` and the `result`, the `names` the expression reads and the `missingNames`
/// of those `inputs` lacks, or the refusal.
fn evaluate_decimal_expression(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    out.insert("result".into(), Value::Null);
    let expression = match parsed(parameters)? {
        Ok(expression) => expression,
        Err(error) => {
            refused(&mut out, &error);
            return Ok(out);
        }
    };
    out.insert("names".into(), text_list(expression.names()));
    let given = match parameters.get("inputs") {
        None | Some(Value::Null) => None,
        Some(Value::Map(map)) => Some(map),
        Some(_) => return Err(LibError::new("inputs is a map of names to decimals")),
    };
    // only the inputs the expression reads are converted; an unread one is not looked at
    let mut inputs = BTreeMap::new();
    for name in expression.names() {
        let Some(value) = given.and_then(|map| map.get(name)) else {
            continue;
        };
        match input_decimal(value) {
            Ok(Some(number)) => {
                inputs.insert(name.clone(), number);
            }
            Ok(None) => {}
            Err(()) => {
                let error = ExprError {
                    code: "INPUT_INVALID",
                    message: format!("The input {name} is not a decimal"),
                    position: None,
                };
                refused(&mut out, &error);
                return Ok(out);
            }
        }
    }
    let missing = expression.missing_names(&inputs);
    out.insert("missingNames".into(), text_list(&missing));
    match expression.evaluate(&inputs) {
        Ok(result) => {
            out.insert("valid".into(), Value::Bool(true));
            out.insert("result".into(), Value::Decimal(result));
        }
        Err(error) => refused(&mut out, &error),
    }
    Ok(out)
}
