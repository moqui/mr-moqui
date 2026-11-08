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

//! The decimal scripts see on quickjs-oxide: a host object with the methods of the QuickJS bindings' `HostDecimal`
//! (`script/runtime/decimal.rs`), on one prototype per runtime, doing the same checked arithmetic.

use std::cmp::Ordering;

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{Context, JsResult, ObjectRef, RuntimeError, Scope, Value};

use super::convert::{text, Conv};
use super::engine::{guarded, OxideEngine};
use crate::script::runtime::{
    arith, decimal_of, decimals_compare, decimals_equal, rounding_named, Arith, VALUE_OF_MESSAGE,
};

/// A decimal's value, held by its host object.
pub(super) struct OxDecimal {
    pub(super) inner: HostValue,
}

/// The decimal prototype of a runtime, its methods defined.
pub(super) fn make_prototype(context: &Context) -> Result<ObjectRef, RuntimeError> {
    let prototype = context.new_object()?;
    method(context, &prototype, "valueOf", 0, |_, _| {
        Err(Conv::Error(VALUE_OF_MESSAGE.to_string()))
    })?;
    method(
        context,
        &prototype,
        "toNumber",
        0,
        |_, this| match moqui_lib::object::basic_convert(this, "Double").value {
            HostValue::F64(number) => Ok(Value::number(number)),
            _ => Err(not_a_decimal()),
        },
    )?;
    method(context, &prototype, "toString", 0, |_, this| {
        text(&moqui_lib::object::to_plain_string(this))
    })?;
    method(context, &prototype, "toJSON", 1, to_json)?;
    arith_method(context, &prototype, "add", || Arith::Add)?;
    arith_method(context, &prototype, "subtract", || Arith::Subtract)?;
    arith_method(context, &prototype, "multiply", || Arith::Multiply)?;
    method(context, &prototype, "divide", 3, |scope, this| {
        if !matches!(scope.arg(1), Value::Undefined) {
            return divide_scaled(scope, this);
        }
        let other = operand(scope, 0)?;
        decimal(
            scope,
            arith(this, &other, Arith::Divide).map_err(|err| Conv::Error(err.message().to_string()))?,
        )
    })?;
    method(context, &prototype, "divideScaled", 3, divide_scaled)?;
    method(context, &prototype, "eq", 1, |scope, this| {
        let other = operand(scope, 0)?;
        Ok(Value::Bool(
            decimals_equal(this, &other).map_err(|err| Conv::Error(err.message().to_string()))?,
        ))
    })?;
    method(context, &prototype, "compare", 1, |scope, this| {
        let other = operand(scope, 0)?;
        let ordering = decimals_compare(this, &other).map_err(|err| Conv::Error(err.message().to_string()))?;
        Ok(Value::Int(match ordering {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }))
    })?;
    method(context, &prototype, "setScale", 2, |scope, this| {
        use moqui_lib::format::RoundingStrategy;
        let Some(number) = this.as_decimal() else {
            return Err(not_a_decimal());
        };
        let scale = scale_arg(scope, 0)?;
        let strategy = rounding_arg(scope, 1)?;
        let mut scaled = match strategy {
            Some(strategy) => number.round_dp_with_strategy(scale, strategy),
            None => {
                let rounded = number.round_dp_with_strategy(scale, RoundingStrategy::ToZero);
                if rounded != number {
                    return Err(Conv::Error(
                        "rounding necessary; give setScale a rounding mode".to_string(),
                    ));
                }
                rounded
            }
        };
        scaled.rescale(scale);
        decimal(scope, HostValue::Decimal(scaled))
    })?;
    method(context, &prototype, "abs", 0, |scope, this| match this.as_decimal() {
        Some(number) => decimal(scope, HostValue::Decimal(number.abs())),
        None => Err(not_a_decimal()),
    })?;
    method(context, &prototype, "negate", 0, |scope, this| {
        match this.as_decimal() {
            Some(number) => decimal(scope, HostValue::Decimal(-number)),
            None => Err(not_a_decimal()),
        }
    })?;
    method(context, &prototype, "scale", 0, |_, this| match this.as_decimal() {
        #[allow(clippy::cast_possible_wrap)]
        Some(number) => Ok(Value::number(f64::from(number.scale()))),
        None => Err(not_a_decimal()),
    })?;
    method(context, &prototype, "isZero", 0, |_, this| match this.as_decimal() {
        Some(number) => Ok(Value::Bool(moqui_lib::object::decimal_is_zero(number))),
        None => Err(not_a_decimal()),
    })?;
    Ok(prototype)
}

/// The global `toDecimal(value)`, as the QuickJS host's (`runtime/helpers.rs`): a decimal from a decimal, a number or
/// numeric text, else an error. Read-only, as the QuickJS host's globals are.
pub(super) fn install_to_decimal(context: &Context) -> Result<(), RuntimeError> {
    let function = context.new_function(
        quickjs_oxide::engine::api::core::FunctionOptions::new("toDecimal").length(1),
        |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |scope| {
                let value = scope.arg(0);
                let host = host_of(scope, &value)?;
                match decimal_of(&host) {
                    Some(inner) => decimal(scope, inner),
                    None => Err(Conv::Error(format!(
                        "toDecimal takes a decimal, a number or numeric text, not {}",
                        moqui_lib::object::to_plain_string(&host)
                    ))),
                }
            })
        },
    )?;
    let global = context.global_object()?;
    context.define_own_property(
        &global,
        &super::convert::key("toDecimal").map_err(|_| RuntimeError::Invariant("a key"))?,
        &quickjs_oxide::engine::api::core::OrdinaryPropertyDescriptor::data(Value::from(function), false, true, false),
    )?;
    // `basicConvert(value, type)`, as the QuickJS host's: the value converted as the type the actions `set` names
    let convert = context.new_function(
        quickjs_oxide::engine::api::core::FunctionOptions::new("basicConvert").length(2),
        |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |scope| {
                let value = scope.arg(0);
                let host = host_of(scope, &value)?;
                let Value::String(kind) = scope.arg(1) else {
                    return Err(Conv::Error(
                        "Error converting from js 'undefined' into type 'string'".to_string(),
                    ));
                };
                let converted = crate::l10n::basic_convert(&host, &kind.to_utf8_lossy()).value;
                let engine = super::engine::current().ok_or(Conv::Unsupported("no oxide engine"))?;
                super::convert::to_js(scope, &engine, &converted)
            })
        },
    )?;
    context.define_own_property(
        &global,
        &super::convert::key("basicConvert").map_err(|_| RuntimeError::Invariant("a key"))?,
        &quickjs_oxide::engine::api::core::OrdinaryPropertyDescriptor::data(Value::from(convert), false, true, false),
    )?;
    Ok(())
}

fn arith_method(
    context: &Context,
    prototype: &ObjectRef,
    name: &'static str,
    op: fn() -> Arith,
) -> Result<(), RuntimeError> {
    method(context, prototype, name, 1, move |scope, this| {
        let other = operand(scope, 0)?;
        decimal(
            scope,
            arith(this, &other, op()).map_err(|err| Conv::Error(err.message().to_string()))?,
        )
    })
}

fn not_a_decimal() -> Conv {
    Conv::Error("not a decimal".to_string())
}

/// Define `name` on the prototype: `body` gets the receiver's decimal; a receiver that is not one throws.
fn method(
    context: &Context,
    prototype: &ObjectRef,
    name: &'static str,
    length: u32,
    body: impl Fn(&mut Scope<'_>, &HostValue) -> Result<Value, Conv> + 'static,
) -> Result<(), RuntimeError> {
    context.define_function(
        prototype,
        name,
        length,
        move |scope: &mut Scope<'_>| -> JsResult<Value> {
            guarded(scope, |scope| {
                let Some(this) = scope.this_host_data::<OxDecimal>()? else {
                    return Err(not_a_decimal());
                };
                body(scope, &this.inner)
            })
        },
    )?;
    Ok(())
}

/// A new decimal host object.
fn decimal(scope: &mut Scope<'_>, inner: HostValue) -> Result<Value, Conv> {
    let engine = super::engine::current().ok_or(Conv::Unsupported("no oxide engine"))?;
    Ok(Value::Object(
        scope.new_host_object(OxDecimal { inner }, Some(&engine.protos.decimal))?,
    ))
}

/// The other side of a decimal method: a decimal, or a number or numeric text converted as `basicConvert(x,
/// 'BigDecimal')` does.
fn operand(scope: &mut Scope<'_>, index: usize) -> Result<HostValue, Conv> {
    let value = scope.arg(index);
    if let Value::Object(object) = &value {
        if let Some(decimal) = scope.host_data::<OxDecimal>(object)? {
            return Ok(decimal.inner.clone());
        }
    }
    let host = host_of(scope, &value)?;
    decimal_of(&host).ok_or_else(|| {
        Conv::Error(format!(
            "a decimal method takes a decimal, a number or a numeric string, not {}",
            moqui_lib::object::to_plain_string(&host)
        ))
    })
}

fn host_of(scope: &mut Scope<'_>, value: &Value) -> Result<HostValue, Conv> {
    let engine: std::rc::Rc<OxideEngine> = super::engine::current().ok_or(Conv::Unsupported("no oxide engine"))?;
    super::convert::to_host(scope, &engine, value)
}

fn scale_arg(scope: &mut Scope<'_>, index: usize) -> Result<u32, Conv> {
    let value = scope.arg(index);
    let number = scope.to_number(&value)?;
    if !number.is_finite() || number < 0.0 || number > f64::from(u32::MAX) {
        return Err(Conv::Error("a scale is a whole number of places".to_string()));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(number as u32)
}

fn rounding_arg(scope: &mut Scope<'_>, index: usize) -> Result<Option<moqui_lib::format::RoundingStrategy>, Conv> {
    let value = scope.arg(index);
    let mode = match value {
        Value::Undefined | Value::Null => String::new(),
        other => moqui_lib::object::to_plain_string(&host_of(scope, &other)?),
    };
    rounding_named(&mode).map_err(Conv::Error)
}

/// `divide(other, scale, roundingMode)`, as Java's `BigDecimal.divide`.
fn divide_scaled(scope: &mut Scope<'_>, this: &HostValue) -> Result<Value, Conv> {
    let other = operand(scope, 0)?;
    let scale = scale_arg(scope, 1)?;
    let (Some(left), Some(right)) = (this.as_decimal(), other.as_decimal()) else {
        return Err(not_a_decimal());
    };
    if moqui_lib::object::decimal_is_zero(right) {
        return Err(Conv::Error("BigInteger divide by zero".to_string()));
    }
    let quotient = left
        .checked_div(right)
        .ok_or_else(|| Conv::Error("decimal overflow".to_string()))?;
    let strategy = rounding_arg(scope, 2)?.unwrap_or(moqui_lib::format::RoundingStrategy::MidpointNearestEven);
    let mut scaled = quotient.round_dp_with_strategy(scale, strategy);
    scaled.rescale(scale);
    decimal(scope, HostValue::Decimal(scaled))
}

/// What `JSON.stringify` writes: a JSON number, the plain digits through `JSON.rawJSON` when a number would round.
fn to_json(scope: &mut Scope<'_>, this: &HostValue) -> Result<Value, Conv> {
    if let HostValue::F64(number) = moqui_lib::object::basic_convert(this, "Double").value {
        let exact = number.is_finite()
            && decimal_of(&HostValue::String(number.to_string()))
                .is_some_and(|back| decimals_equal(&back, this).unwrap_or(false));
        if exact {
            return Ok(Value::number(number));
        }
    }
    let digits = moqui_lib::object::to_plain_string(this);
    let global = scope.global_object()?;
    let json = scope.get_property(&global, &super::convert::key("JSON")?)?;
    if let Value::Object(json) = json {
        if let Value::Object(raw) = scope.get_property(&json, &super::convert::key("rawJSON")?)? {
            if let Some(raw) = scope.as_callable(&raw)? {
                let digits = text(&digits)?;
                return Ok(scope.call(&raw, Value::Undefined, &[digits])?);
            }
        }
    }
    text(&digits)
}
