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

//! The decimal class scripts see, and its checked arithmetic.

use super::*;

#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostDecimal {
    #[qjs(skip_trace)]
    pub(super) inner: Value,
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostDecimal {
    fn value_of(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || Err(Exception::throw_message(&ctx, VALUE_OF_MESSAGE)))
    }

    /// The value as a JavaScript number, for code that wants float math or `Number` APIs and accepts the rounding.
    fn to_number(&self, ctx: Ctx<'_>) -> rquickjs::Result<f64> {
        host_guard(&ctx, || {
            match moqui_lib::object::basic_convert(&self.inner, "Double").value {
                Value::F64(number) => Ok(number),
                _ => Err(Exception::throw_message(&ctx, "not a decimal")),
            }
        })
    }

    #[qjs(rename = "toString")]
    fn js_string(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(moqui_lib::object::to_plain_string(&self.inner)))
    }

    /// What `JSON.stringify` writes: always a JSON number, as Java's JSON writers write a `BigDecimal`. A value a
    /// JavaScript number holds exactly is that number; any other is its plain digits, unrounded (`JSON.rawJSON`).
    #[qjs(rename = "toJSON")]
    fn to_json<'js>(&self, ctx: Ctx<'js>, _key: Opt<JsValue<'js>>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            if let Value::F64(number) = moqui_lib::object::basic_convert(&self.inner, "Double").value {
                let exact = number.is_finite()
                    && decimal_of(&Value::String(number.to_string()))
                        .is_some_and(|back| decimals_equal(&back, &self.inner).unwrap_or(false));
                if exact {
                    return number.into_js(&ctx);
                }
            }
            let digits = moqui_lib::object::to_plain_string(&self.inner);
            let json: rquickjs::Object<'js> = ctx.globals().get("JSON")?;
            match json.get::<_, Option<rquickjs::Function<'js>>>("rawJSON")? {
                Some(raw) => raw.call((digits,)),
                // an engine without rawJSON: the digits as text rather than a rounded number
                None => digits.into_js(&ctx),
            }
        })
    }

    fn add<'js>(&self, ctx: Ctx<'js>, other: JsValue<'js>) -> rquickjs::Result<HostDecimal> {
        let other = operand(&ctx, &other)?;
        host_arith(&ctx, &self.inner, &other, Arith::Add)
    }

    fn subtract<'js>(&self, ctx: Ctx<'js>, other: JsValue<'js>) -> rquickjs::Result<HostDecimal> {
        let other = operand(&ctx, &other)?;
        host_arith(&ctx, &self.inner, &other, Arith::Subtract)
    }

    fn multiply<'js>(&self, ctx: Ctx<'js>, other: JsValue<'js>) -> rquickjs::Result<HostDecimal> {
        let other = operand(&ctx, &other)?;
        host_arith(&ctx, &self.inner, &other, Arith::Multiply)
    }

    fn divide<'js>(
        &self,
        ctx: Ctx<'js>,
        other: JsValue<'js>,
        scale: Opt<u32>,
        mode: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<HostDecimal> {
        if let Some(scale) = scale.0 {
            return self.divide_scaled(ctx, other, scale, mode);
        }
        let other = operand(&ctx, &other)?;
        host_arith(&ctx, &self.inner, &other, Arith::Divide)
    }

    #[qjs(rename = "eq")]
    fn equal<'js>(&self, ctx: Ctx<'js>, other: JsValue<'js>) -> rquickjs::Result<bool> {
        let other = operand(&ctx, &other)?;
        host_guard(&ctx, || {
            decimals_equal(&self.inner, &other).map_err(|err| throw_lib(&ctx, &err))
        })
    }

    fn compare<'js>(&self, ctx: Ctx<'js>, other: JsValue<'js>) -> rquickjs::Result<i32> {
        let other = operand(&ctx, &other)?;
        host_guard(&ctx, || {
            let ordering = decimals_compare(&self.inner, &other).map_err(|err| throw_lib(&ctx, &err))?;
            Ok(match ordering {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            })
        })
    }

    /// `setScale(scale, roundingMode)`, as Java's `BigDecimal.setScale`. The mode is a `RoundingMode` name such as
    /// `HALF_UP`, or the old `BigDecimal.ROUND_*` number. Without a mode, a value that would need rounding is an error.
    fn set_scale<'js>(&self, ctx: Ctx<'js>, scale: u32, mode: Opt<JsValue<'js>>) -> rquickjs::Result<HostDecimal> {
        use moqui_lib::format::RoundingStrategy;
        host_guard(&ctx, || {
            let Some(number) = self.inner.as_decimal() else {
                return Err(Exception::throw_message(&ctx, "not a decimal"));
            };
            let strategy = rounding_strategy(&ctx, mode.0.as_ref())?;
            let mut scaled = match strategy {
                Some(strategy) => number.round_dp_with_strategy(scale, strategy),
                None => {
                    let rounded = number.round_dp_with_strategy(scale, RoundingStrategy::ToZero);
                    if rounded != number {
                        return Err(Exception::throw_message(
                            &ctx,
                            "rounding necessary; give setScale a rounding mode",
                        ));
                    }
                    rounded
                }
            };
            // a larger scale adds trailing zeros, as Java's does
            scaled.rescale(scale);
            Ok(HostDecimal {
                inner: Value::Decimal(scaled),
            })
        })
    }

    /// `abs()`, with the scale kept, as Java's `BigDecimal.abs`.
    fn abs(&self, ctx: Ctx<'_>) -> rquickjs::Result<HostDecimal> {
        host_guard(&ctx, || {
            let Some(number) = self.inner.as_decimal() else {
                return Err(Exception::throw_message(&ctx, "not a decimal"));
            };
            Ok(HostDecimal {
                inner: Value::Decimal(number.abs()),
            })
        })
    }

    /// `negate()`, with the scale kept.
    fn negate(&self, ctx: Ctx<'_>) -> rquickjs::Result<HostDecimal> {
        host_guard(&ctx, || {
            let Some(number) = self.inner.as_decimal() else {
                return Err(Exception::throw_message(&ctx, "not a decimal"));
            };
            Ok(HostDecimal {
                inner: Value::Decimal(-number),
            })
        })
    }

    /// `divide(other, scale, roundingMode)`, as Java's `BigDecimal.divide`: the quotient rounded to `scale` places by
    /// the mode, without the intermediate rounding of `divide(other)`.
    fn divide_scaled<'js>(
        &self,
        ctx: Ctx<'js>,
        other: JsValue<'js>,
        scale: u32,
        mode: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<HostDecimal> {
        host_guard(&ctx, || {
            let other = operand(&ctx, &other)?;
            let (Some(left), Some(right)) = (self.inner.as_decimal(), other.as_decimal()) else {
                return Err(Exception::throw_message(&ctx, "not a decimal"));
            };
            if moqui_lib::object::decimal_is_zero(right) {
                return Err(Exception::throw_message(&ctx, "BigInteger divide by zero"));
            }
            let quotient = left
                .checked_div(right)
                .ok_or_else(|| Exception::throw_message(&ctx, "decimal overflow"))?;
            let strategy = rounding_strategy(&ctx, mode.0.as_ref())?
                .unwrap_or(moqui_lib::format::RoundingStrategy::MidpointNearestEven);
            let mut scaled = quotient.round_dp_with_strategy(scale, strategy);
            scaled.rescale(scale);
            Ok(HostDecimal {
                inner: Value::Decimal(scaled),
            })
        })
    }

    /// `scale()`: the digits after the decimal point.
    fn scale(&self, ctx: Ctx<'_>) -> rquickjs::Result<u32> {
        host_guard(&ctx, || {
            self.inner
                .as_decimal()
                .map(|number| number.scale())
                .ok_or_else(|| Exception::throw_message(&ctx, "not a decimal"))
        })
    }

    fn is_zero<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || {
            let Some(number) = self.inner.as_decimal() else {
                return Err(Exception::throw_message(&ctx, "not a decimal"));
            };
            Ok(moqui_lib::object::decimal_is_zero(number))
        })
    }
}

/// A `RoundingMode` name, or the old `BigDecimal.ROUND_*` number, as a strategy. `None` is `UNNECESSARY`, also the
/// meaning of no mode.
pub(super) fn rounding_strategy<'js>(
    ctx: &Ctx<'js>,
    mode: Option<&JsValue<'js>>,
) -> rquickjs::Result<Option<moqui_lib::format::RoundingStrategy>> {
    use moqui_lib::format::RoundingStrategy;
    let mode = match mode {
        Some(value) if !value.is_null() && !value.is_undefined() => {
            moqui_lib::object::to_plain_string(&js_to_host(ctx, value)?)
        }
        _ => String::new(),
    };
    let strategy = match mode.as_str() {
        "UP" | "0" => Some(RoundingStrategy::AwayFromZero),
        "DOWN" | "1" => Some(RoundingStrategy::ToZero),
        "CEILING" | "2" => Some(RoundingStrategy::ToPositiveInfinity),
        "FLOOR" | "3" => Some(RoundingStrategy::ToNegativeInfinity),
        "HALF_UP" | "4" => Some(RoundingStrategy::MidpointAwayFromZero),
        "HALF_DOWN" | "5" => Some(RoundingStrategy::MidpointTowardZero),
        "HALF_EVEN" | "6" => Some(RoundingStrategy::MidpointNearestEven),
        "" | "UNNECESSARY" | "7" => None,
        other => {
            return Err(Exception::throw_message(
                ctx,
                &format!("rounding mode {other} is not known"),
            ))
        }
    };
    Ok(strategy)
}

/// The other side of a decimal method: a decimal, or a JavaScript number or numeric string converted as
/// `basicConvert(x, 'BigDecimal')` does. A number that is not finite, or other text, is an error.
fn operand<'js>(ctx: &Ctx<'js>, value: &JsValue<'js>) -> rquickjs::Result<Value> {
    if let Some(object) = value.as_object() {
        if let Some(decimal) = object.as_class::<HostDecimal>() {
            return Ok(decimal.borrow().inner.clone());
        }
    }
    host_guard(ctx, || {
        let host = js_to_host(ctx, value)?;
        decimal_of(&host).ok_or_else(|| {
            Exception::throw_message(
                ctx,
                &format!(
                    "a decimal method takes a decimal, a number or a numeric string, not {}",
                    moqui_lib::object::to_plain_string(&host)
                ),
            )
        })
    })
}

/// `value` as a decimal: a decimal, an integer, a finite float, or numeric text.
pub(super) fn decimal_of(value: &Value) -> Option<Value> {
    if matches!(value, Value::Bool(_) | Value::Null) {
        return None;
    }
    match moqui_lib::object::basic_convert(value, "BigDecimal").value {
        decimal @ Value::Decimal(_) => Some(decimal),
        _ => None,
    }
}

pub(super) enum Arith {
    Add,
    Subtract,
    Multiply,
    Divide,
}

pub(super) fn host_arith(ctx: &Ctx<'_>, left: &Value, right: &Value, op: Arith) -> rquickjs::Result<HostDecimal> {
    host_guard(ctx, || {
        let inner = arith(left, right, op).map_err(|err| throw_lib(ctx, &err))?;
        Ok(HostDecimal { inner })
    })
}

pub(super) fn arith(left: &Value, right: &Value, op: Arith) -> Result<Value, moqui_lib::error::Error> {
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

pub(super) fn decimals_equal(left: &Value, right: &Value) -> Result<bool, moqui_lib::error::Error> {
    let Some(left) = left.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    let Some(right) = right.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    Ok(moqui_lib::object::decimal_eq(left, right))
}

pub(super) fn decimals_compare(left: &Value, right: &Value) -> Result<Ordering, moqui_lib::error::Error> {
    let Some(left) = left.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    let Some(right) = right.as_decimal() else {
        return Err(moqui_lib::error::Error::new("not a decimal"));
    };
    Ok(moqui_lib::object::decimal_compare(left, right))
}
