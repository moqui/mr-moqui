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

//! Decimal arithmetic for the scripts' decimal objects.

use std::cmp::Ordering;

use super::*;

/// `value` as a decimal: a decimal, an integer, a finite float, or numeric text.
pub(crate) fn decimal_of(value: &Value) -> Option<Value> {
    if matches!(value, Value::Bool(_) | Value::Null) {
        return None;
    }
    match moqui_lib::object::basic_convert(value, "BigDecimal").value {
        decimal @ Value::Decimal(_) => Some(decimal),
        _ => None,
    }
}

pub(crate) enum Arith {
    Add,
    Subtract,
    Multiply,
    Divide,
}

pub(crate) fn arith(left: &Value, right: &Value, op: Arith) -> Result<Value, moqui_lib::error::Error> {
    let Some(left) = left.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    let Some(right) = right.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    let number = match op {
        Arith::Add => moqui_lib::object::decimal_add(left, right)?,
        Arith::Subtract => moqui_lib::object::decimal_subtract(left, right)?,
        Arith::Multiply => moqui_lib::object::decimal_multiply(left, right)?,
        Arith::Divide => moqui_lib::object::decimal_divide(left, right)?,
    };
    Ok(Value::Decimal(number))
}

pub(crate) fn decimals_equal(left: &Value, right: &Value) -> Result<bool, moqui_lib::error::Error> {
    let Some(left) = left.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    let Some(right) = right.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    Ok(moqui_lib::object::decimal_eq(left, right))
}

pub(crate) fn decimals_compare(left: &Value, right: &Value) -> Result<Ordering, moqui_lib::error::Error> {
    let Some(left) = left.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    let Some(right) = right.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    Ok(moqui_lib::object::decimal_compare(left, right))
}
