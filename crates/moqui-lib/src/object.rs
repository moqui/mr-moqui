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

//! Field values and coercion. Decimals are `rust_decimal`. Timestamps are `jiff`.
//!
//! `divide` uses scale `max(left, right, 10)` and half-up, so `1/3` is `0.3333333333`.
//! That is the Groovy rule. The half-even screen formatter is a later phase.
//!
//! A timestamp with no offset is read in the zone passed to the parser. [`basic_convert`] and
//! [`convert_checked`] read it in UTC, the form a database driver gives this port's timestamps
//! back in. Text a person, a script or a data file wrote is in the session time zone, as Java's
//! `ec.l10n.parseTimestamp` reads it: [`basic_convert_in`] and [`convert_checked_in`] take that
//! zone, and [`text_time_zone`] is it where the framework has registered it (`moqui-core`'s `l10n`).
//! `java.sql` date and time values are civil dates and times. Their plain form is the
//! UTC epoch millis of that civil value, which matches `getTime` when the JVM zone is UTC.

use std::cmp::Ordering;
use std::str::FromStr;

use indexmap::IndexMap;
use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{Offset, TimeZone};
use jiff::Timestamp;
use rust_decimal::Decimal;
use rust_decimal::RoundingStrategy;

use crate::error::Error;

#[path = "regex_match.rs"]
mod regex_match;
pub use regex_match::CompiledRegex;

/// A value that can cross a facade without an `ExecutionContext`.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    String(String),
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Decimal(Decimal),
    Timestamp(Timestamp),
    Date(Date),
    Time(Time),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(IndexMap<String, Value>),
    /// A shared host object a script holds by reference, such as an entity value.
    Object(HostObject),
}

/// Opaque shared handle. Equal only to the same allocation.
#[derive(Clone)]
pub struct HostObject(pub std::sync::Arc<dyn std::any::Any + Send + Sync>);

impl HostObject {
    pub fn new<T: std::any::Any + Send + Sync>(value: T) -> Self {
        Self(std::sync::Arc::new(value))
    }

    pub fn downcast<T: std::any::Any + Send + Sync>(&self) -> Option<std::sync::Arc<T>> {
        std::sync::Arc::clone(&self.0).downcast::<T>().ok()
    }
}

impl std::fmt::Debug for HostObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HostObject")
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        java_equals(self, other)
    }
}

/// Result of [`basic_convert`]. A failed conversion keeps the original value.
#[derive(Clone, Debug, PartialEq)]
pub struct Converted {
    pub value: Value,
    pub changed: bool,
}

impl Value {
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_decimal(&self) -> Option<Decimal> {
        match self {
            Self::Decimal(value) => Some(*value),
            _ => None,
        }
    }
}

pub fn decimal_add(left: Decimal, right: Decimal) -> Result<Decimal, Error> {
    left.checked_add(right)
        .ok_or_else(|| Error::new("decimal add overflowed"))
}

pub fn decimal_subtract(left: Decimal, right: Decimal) -> Result<Decimal, Error> {
    left.checked_sub(right)
        .ok_or_else(|| Error::new("decimal subtract overflowed"))
}

pub fn decimal_multiply(left: Decimal, right: Decimal) -> Result<Decimal, Error> {
    left.checked_mul(right)
        .ok_or_else(|| Error::new("decimal multiply overflowed"))
}

pub fn decimal_divide(left: Decimal, right: Decimal) -> Result<Decimal, Error> {
    if right.is_zero() {
        return Err(Error::new("decimal divide by zero"));
    }
    let scale = left.scale().max(right.scale()).max(10);
    let quotient = left
        .checked_div(right)
        .ok_or_else(|| Error::new("decimal divide overflowed"))?;
    Ok(quotient.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero))
}

/// Numerical equality. `1.0` and `1.00` are equal. Script `==` is not this method.
pub fn decimal_eq(left: Decimal, right: Decimal) -> bool {
    left == right
}

pub fn decimal_compare(left: Decimal, right: Decimal) -> Ordering {
    left.cmp(&right)
}

pub fn decimal_is_zero(value: Decimal) -> bool {
    value.is_zero()
}

/// Null, empty string, empty list, or empty map. `0` and `false` are not empty.
pub fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::List(items) => items.is_empty(),
        Value::Map(entries) => entries.is_empty(),
        _ => false,
    }
}

pub fn to_plain_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::I32(number) => number.to_string(),
        Value::I64(number) => number.to_string(),
        Value::F64(number) => number.to_string(),
        Value::Decimal(number) => number.to_string(),
        Value::Timestamp(timestamp) => timestamp.as_millisecond().to_string(),
        Value::Date(date) => date_epoch_millis(*date).to_string(),
        Value::Time(time) => time_epoch_millis(*time).to_string(),
        Value::Bytes(bytes) => crate::codec::hex_encode(bytes),
        Value::List(items) => {
            let mut out = String::new();
            for item in items {
                if item.is_null() {
                    continue;
                }
                if !out.is_empty() {
                    out.push(',');
                }
                out.push_str(&to_plain_string(item));
            }
            out
        }
        Value::Object(_) => "<object>".to_string(),
        Value::Map(entries) => {
            let mut out = String::from("{");
            for (index, (key, item)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(key);
                out.push('=');
                out.push_str(&to_plain_string(item));
            }
            out.push('}');
            out
        }
    }
}

/// SQL `LIKE`. The pattern is case-insensitive. `%` and `_` are the wildcards.
/// A null pattern matches. A null value does not.
pub fn compare_like(value: &Value, pattern: &Value) -> bool {
    if pattern.is_null() {
        return true;
    }
    let (Some(text), Some(like)) = (value.as_str(), pattern.as_str()) else {
        return false;
    };
    like_match(text, like)
}

/// Operators from `ObjectUtilities.compare`. Ordered compares error when the values
/// cannot be ordered. `equals` uses Java `equals`: decimal scale matters.
pub fn compare(field: &Value, operator: &str, to_field: &Value, java_type: &str) -> Result<bool, Error> {
    compare_in(field, operator, to_field, java_type, &TimeZone::UTC)
}

/// [`compare`] with timestamp text that has no offset read in `zone`, as [`basic_convert_in`].
pub fn compare_in(
    field: &Value,
    operator: &str,
    to_field: &Value,
    java_type: &str,
    zone: &TimeZone,
) -> Result<bool, Error> {
    let field = basic_convert_in(field, java_type, zone).value;
    let to_field = if is_empty(to_field) {
        to_field.clone()
    } else {
        basic_convert_in(to_field, java_type, zone).value
    };
    let result = match operator {
        "less" => compare_obj(&field, &to_field)? == Ordering::Less,
        "greater" => compare_obj(&field, &to_field)? == Ordering::Greater,
        "less-equals" => compare_obj(&field, &to_field)? != Ordering::Greater,
        "greater-equals" => compare_obj(&field, &to_field)? != Ordering::Less,
        "contains" => to_plain_string(&field).contains(&to_plain_string(&to_field)),
        "not-contains" => !to_plain_string(&field).contains(&to_plain_string(&to_field)),
        "empty" => is_empty(&field),
        "not-empty" => !is_empty(&field),
        "matches" => regex_full_match(&to_plain_string(&field), &to_plain_string(&to_field))?,
        "not-matches" => !regex_full_match(&to_plain_string(&field), &to_plain_string(&to_field))?,
        "not-equals" => !java_equals(&field, &to_field),
        _ => java_equals(&field, &to_field),
    };
    Ok(result)
}

/// Null on the left sorts after a value. Null on the right is an error, as `compareTo` would be.
pub fn compare_obj(left: &Value, right: &Value) -> Result<Ordering, Error> {
    if left.is_null() {
        return Ok(if right.is_null() {
            Ordering::Equal
        } else {
            Ordering::Greater
        });
    }
    if right.is_null() {
        return Err(Error::new("cannot compare a value to null"));
    }
    match (left, right) {
        (Value::String(a), Value::String(b)) => Ok(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Ok(a.cmp(b)),
        (Value::I32(a), Value::I32(b)) => Ok(a.cmp(b)),
        (Value::I64(a), Value::I64(b)) => Ok(a.cmp(b)),
        (Value::F64(a), Value::F64(b)) => a.partial_cmp(b).ok_or_else(|| Error::new("cannot compare NaN")),
        (Value::Decimal(a), Value::Decimal(b)) => Ok(a.cmp(b)),
        (Value::Timestamp(a), Value::Timestamp(b)) => Ok(a.cmp(b)),
        (Value::Date(a), Value::Date(b)) => Ok(a.cmp(b)),
        (Value::Time(a), Value::Time(b)) => Ok(a.cmp(b)),
        (Value::Bytes(a), Value::Bytes(b)) => Ok(a.cmp(b)),
        _ => Err(Error::new("values are not the same comparable type")),
    }
}

pub fn add_numbers(left: &Value, right: &Value) -> Result<Value, Error> {
    if left.is_null() {
        return Ok(right.clone());
    }
    if right.is_null() {
        return Ok(left.clone());
    }
    if let (Some(a), Some(b)) = (as_decimal(left), as_decimal(right)) {
        if matches!(left, Value::Decimal(_)) || matches!(right, Value::Decimal(_)) {
            return Ok(Value::Decimal(decimal_add(a, b)?));
        }
    }
    if let (Some(a), Some(b)) = (as_f64(left), as_f64(right)) {
        if matches!(left, Value::F64(_)) || matches!(right, Value::F64(_)) {
            return Ok(Value::F64(a + b));
        }
    }
    if let (Some(a), Some(b)) = (as_i64(left), as_i64(right)) {
        if matches!(left, Value::I64(_)) || matches!(right, Value::I64(_)) {
            return a
                .checked_add(b)
                .map(Value::I64)
                .ok_or_else(|| Error::new("long add overflowed"));
        }
    }
    if let (Some(a), Some(b)) = (as_i32(left), as_i32(right)) {
        return a
            .checked_add(b)
            .map(Value::I32)
            .ok_or_else(|| Error::new("integer add overflowed"));
    }
    Err(Error::new("add_numbers needs numeric values"))
}

/// Coerce `value` to the Java type name used on an entity field.
///
/// Boolean strings are only `true` and `y`, case-insensitive. A failed conversion
/// returns the original value and `changed == false`. Timestamp text without an offset is UTC, the form a database
/// driver gives back; text a person or a script wrote goes through [`basic_convert_in`] with the session time zone.
pub fn basic_convert(value: &Value, java_type: &str) -> Converted {
    basic_convert_in(value, java_type, &TimeZone::UTC)
}

/// [`basic_convert_in`] in [`text_time_zone`], for text a person or a script wrote that reaches a layer with no session
/// at hand (a condition value bound against a timestamp column). The zone is looked up only for timestamp text.
pub fn basic_convert_text(value: &Value, java_type: &str) -> Converted {
    if matches!(value, Value::String(_)) && simple_type_name(java_type) == "Timestamp" {
        basic_convert_in(value, java_type, &text_time_zone())
    } else {
        basic_convert(value, java_type)
    }
}

/// [`basic_convert`] with timestamp text that has no offset read in `zone`, the session time zone for text a person,
/// a script or a data file wrote (Java's `ec.l10n.parseTimestamp`). Text with an offset or `Z`, and epoch millis, are
/// the moment they name in any zone.
pub fn basic_convert_in(value: &Value, java_type: &str, zone: &TimeZone) -> Converted {
    if value.is_null() || java_type.is_empty() {
        return Converted {
            value: value.clone(),
            changed: false,
        };
    }
    let target = simple_type_name(java_type);
    if matches_target(value, target) {
        return Converted {
            value: value.clone(),
            changed: false,
        };
    }
    let converted = convert_to(value, target, zone);
    match converted {
        Some(next) => Converted {
            value: next,
            changed: true,
        },
        None => Converted {
            value: value.clone(),
            changed: false,
        },
    }
}

/// The zone [`text_time_zone`] asks for, registered once by the framework.
static TEXT_TIME_ZONE: std::sync::OnceLock<fn() -> TimeZone> = std::sync::OnceLock::new();

/// Registers the zone timestamp text without an offset is read in where a layer below the framework converts text a
/// person or a script wrote (a condition value bound against a timestamp column). `moqui-core` registers the session
/// time zone; the first registration stands.
pub fn set_text_time_zone(zone: fn() -> TimeZone) {
    let _ = TEXT_TIME_ZONE.set(zone);
}

/// The registered zone for timestamp text a person or a script wrote: the session time zone once the framework is up,
/// else UTC.
#[must_use]
pub fn text_time_zone() -> TimeZone {
    TEXT_TIME_ZONE.get().map_or(TimeZone::UTC, |zone| zone())
}

pub fn parse_timestamp(input: &str, zone: &TimeZone) -> Option<Timestamp> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if is_integer_text(input) {
        if let Ok(millis) = input.parse::<i64>() {
            return Timestamp::from_millisecond(millis).ok();
        }
    }
    if let Some(parts) = parse_civil(input) {
        return parts_to_timestamp(parts, zone);
    }
    Timestamp::from_str(input).ok()
}

pub fn parse_date(input: &str) -> Option<Date> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(date) = parse_ymd(input) {
        return Some(date);
    }
    if let Some(date) = parse_mdy(input) {
        return Some(date);
    }
    if let Some(parts) = parse_civil(input) {
        return Date::new(parts.year, parts.month, parts.day).ok();
    }
    if is_integer_text(input) {
        let millis = input.parse::<i64>().ok()?;
        let timestamp = Timestamp::from_millisecond(millis).ok()?;
        return Some(timestamp.to_zoned(TimeZone::UTC).date());
    }
    None
}

pub fn parse_time(input: &str) -> Option<Time> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(time) = parse_hms(input, true) {
        return Some(time);
    }
    if let Some(time) = parse_clock(input, true) {
        return Some(time);
    }
    if let Some(time) = parse_ampm(input) {
        return Some(time);
    }
    if let Some(parts) = parse_civil(input) {
        return Time::new(parts.hour, parts.minute, parts.second, parts.nano).ok();
    }
    if is_integer_text(input) {
        let millis = input.parse::<i64>().ok()?;
        let timestamp = Timestamp::from_millisecond(millis).ok()?;
        return Some(timestamp.to_zoned(TimeZone::UTC).time());
    }
    None
}

/// Convert like [`basic_convert`], but say when it could not. `None` means `value` is not the
/// scalar type `java_type` names and cannot become one. A type this module does not convert
/// (`Object`, `List`, and so on) accepts the value as it is.
pub fn convert_checked(value: &Value, java_type: &str) -> Option<Value> {
    convert_checked_in(value, java_type, &TimeZone::UTC)
}

/// [`convert_checked`] with timestamp text that has no offset read in `zone`, as [`basic_convert_in`].
pub fn convert_checked_in(value: &Value, java_type: &str, zone: &TimeZone) -> Option<Value> {
    if value.is_null() || java_type.is_empty() {
        return Some(value.clone());
    }
    let target = simple_type_name(java_type);
    if matches_target(value, target) {
        return Some(value.clone());
    }
    match convert_to(value, target, zone) {
        Some(next) => Some(next),
        None if is_scalar_target(target) => None,
        None => Some(value.clone()),
    }
}

fn is_scalar_target(target: &str) -> bool {
    matches!(
        target,
        "String"
            | "Boolean"
            | "Integer"
            | "Long"
            | "Double"
            | "Float"
            | "BigDecimal"
            | "Timestamp"
            | "Date"
            | "Time"
            | "byte[]"
            | "Blob"
    )
}

fn simple_type_name(java_type: &str) -> &str {
    match java_type {
        "byte[]" | "[B" => "byte[]",
        other => other.rsplit('.').next().unwrap_or(other),
    }
}

/// Whether `value` is already the Java type `java_type` names (`java.lang.String`, `String`, and so on).
///
/// A null value and an empty type name count as already converted: there is nothing to coerce.
#[must_use]
pub fn is_java_type(value: &Value, java_type: &str) -> bool {
    if value.is_null() || java_type.is_empty() {
        return true;
    }
    matches_target(value, simple_type_name(java_type))
}

fn matches_target(value: &Value, target: &str) -> bool {
    matches!(
        (value, target),
        (Value::String(_), "String")
            | (Value::Bool(_), "Boolean")
            | (Value::I32(_), "Integer")
            | (Value::I64(_), "Long")
            | (Value::F64(_), "Double" | "Float")
            | (Value::Decimal(_), "BigDecimal")
            | (Value::Timestamp(_), "Timestamp")
            | (Value::Date(_), "Date")
            | (Value::Time(_), "Time")
            | (Value::Bytes(_), "byte[]" | "Blob")
    )
}

fn convert_to(value: &Value, target: &str, zone: &TimeZone) -> Option<Value> {
    match target {
        "String" => Some(Value::String(to_plain_string(value))),
        "Boolean" => Some(Value::Bool(to_bool(value)?)),
        "Integer" => Some(Value::I32(to_i32(value)?)),
        "Long" => Some(Value::I64(to_i64(value)?)),
        "Double" | "Float" => Some(Value::F64(to_f64(value)?)),
        "BigDecimal" => Some(Value::Decimal(to_decimal(value)?)),
        "Timestamp" => Some(Value::Timestamp(to_timestamp(value, zone)?)),
        "Date" => Some(Value::Date(to_date(value)?)),
        "Time" => Some(Value::Time(to_time(value)?)),
        "byte[]" | "Blob" => Some(Value::Bytes(to_bytes(value)?)),
        _ => None,
    }
}

fn to_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => Some(text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("y")),
        Value::I32(number) => Some(*number != 0),
        Value::I64(number) => Some(*number != 0),
        Value::F64(number) => Some(*number != 0.0),
        Value::Decimal(number) => Some(!number.is_zero()),
        _ => None,
    }
}

fn to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::I32(number) => Some(*number),
        Value::I64(number) => i32::try_from(*number).ok(),
        Value::String(text) => text.parse().ok(),
        Value::Decimal(number) => i32::try_from(number.trunc()).ok(),
        Value::F64(number) => f64_whole(*number).and_then(|whole| i32::try_from(whole).ok()),
        _ => None,
    }
}

fn to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::I64(number) => Some(*number),
        Value::I32(number) => Some(i64::from(*number)),
        Value::String(text) => text.parse().ok(),
        Value::Decimal(number) => i64::try_from(number.trunc()).ok(),
        Value::F64(number) => f64_whole(*number),
        _ => None,
    }
}

/// A whole `f64` as an `i64`; `None` for a fraction, NaN, or a number outside the range, which `as` would clamp to
/// `i64::MAX` or `MIN` and pass off as the value.
fn f64_whole(number: f64) -> Option<i64> {
    // 2^63 is exact as an `f64`, so the range is `-2^63 <= n < 2^63`
    (number.fract() == 0.0 && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&number))
        .then_some(number as i64)
}

fn to_f64(value: &Value) -> Option<f64> {
    match value {
        Value::F64(number) => Some(*number),
        Value::I32(number) => Some(f64::from(*number)),
        Value::I64(number) => Some(*number as f64),
        Value::String(text) => text.parse().ok(),
        Value::Decimal(number) => number.to_string().parse().ok(),
        _ => None,
    }
}

fn to_decimal(value: &Value) -> Option<Decimal> {
    match value {
        Value::Decimal(number) => Some(*number),
        Value::I32(number) => Some(Decimal::from(*number)),
        Value::I64(number) => Some(Decimal::from(*number)),
        Value::String(text) => Decimal::from_str(text).ok(),
        Value::F64(number) => Decimal::from_str(&number.to_string()).ok(),
        _ => None,
    }
}

fn to_timestamp(value: &Value, zone: &TimeZone) -> Option<Timestamp> {
    match value {
        Value::Timestamp(timestamp) => Some(*timestamp),
        Value::String(text) => parse_timestamp(text, zone),
        Value::I64(millis) => Timestamp::from_millisecond(*millis).ok(),
        Value::I32(millis) => Timestamp::from_millisecond(i64::from(*millis)).ok(),
        Value::Date(date) => date_to_timestamp(*date),
        _ => None,
    }
}

fn to_date(value: &Value) -> Option<Date> {
    match value {
        Value::Date(date) => Some(*date),
        Value::String(text) => parse_date(text),
        Value::Timestamp(timestamp) => Some(timestamp.to_zoned(TimeZone::UTC).date()),
        _ => None,
    }
}

fn to_time(value: &Value) -> Option<Time> {
    match value {
        Value::Time(time) => Some(*time),
        Value::String(text) => parse_time(text),
        Value::Timestamp(timestamp) => Some(timestamp.to_zoned(TimeZone::UTC).time()),
        _ => None,
    }
}

fn to_bytes(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::Bytes(bytes) => Some(bytes.clone()),
        Value::String(text) => Some(text.as_bytes().to_vec()),
        _ => None,
    }
}

fn as_decimal(value: &Value) -> Option<Decimal> {
    to_decimal(value)
}

fn as_f64(value: &Value) -> Option<f64> {
    to_f64(value)
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::I64(number) => Some(*number),
        Value::I32(number) => Some(i64::from(*number)),
        _ => None,
    }
}

fn as_i32(value: &Value) -> Option<i32> {
    match value {
        Value::I32(number) => Some(*number),
        _ => None,
    }
}

/// Java `Objects.equals` for these values. Decimal scale is significant.
pub fn java_equals(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Decimal(a), Value::Decimal(b)) => a.mantissa() == b.mantissa() && a.scale() == b.scale(),
        (Value::F64(a), Value::F64(b)) => a.to_bits() == b.to_bits(),
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::I32(a), Value::I32(b)) => a == b,
        (Value::I64(a), Value::I64(b)) => a == b,
        (Value::Timestamp(a), Value::Timestamp(b)) => a == b,
        (Value::Date(a), Value::Date(b)) => a == b,
        (Value::Time(a), Value::Time(b)) => a == b,
        (Value::Bytes(a), Value::Bytes(b)) => a == b,
        (Value::List(a), Value::List(b)) => a == b,
        (Value::Map(a), Value::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| value == other))
        }
        (Value::Object(a), Value::Object(b)) => std::sync::Arc::ptr_eq(&a.0, &b.0),
        _ => false,
    }
}

struct CivilParts {
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
    second: i8,
    nano: i32,
    offset_seconds: Option<i32>,
    zone_name: Option<String>,
}

fn parse_civil(input: &str) -> Option<CivilParts> {
    // Tried as whole-string matches, so a shorter pattern cannot steal a longer one.
    let patterns: &[fn(&str) -> Option<CivilParts>] = &[
        parse_date_time_frac_zone,
        parse_date_time_frac_offset,
        parse_date_time_offset,
        parse_date_time_frac,
        parse_date_time_seconds,
        parse_date_time_minutes,
        parse_date_t_seconds,
        parse_date_only,
    ];
    patterns.iter().find_map(|parse| parse(input))
}

fn parse_date_only(input: &str) -> Option<CivilParts> {
    let date = parse_ymd(input)?;
    Some(parts_from_date(date))
}

fn parse_date_time_minutes(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, ' ')?;
    let (hour, minute, rest) = take_hm(rest)?;
    if !rest.is_empty() {
        return None;
    }
    Some(stamp(date, hour, minute, 0, 0, None, None))
}

fn parse_date_time_seconds(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, ' ')?;
    let (hour, minute, second, rest) = take_hms(rest)?;
    if !rest.is_empty() {
        return None;
    }
    Some(stamp(date, hour, minute, second, 0, None, None))
}

fn parse_date_time_frac(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, ' ')?;
    let (hour, minute, second, nano, rest) = take_hms_frac(rest)?;
    if !rest.is_empty() {
        return None;
    }
    Some(stamp(date, hour, minute, second, nano, None, None))
}

fn parse_date_t_seconds(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, 'T')?;
    let (hour, minute, second, rest) = take_hms(rest)?;
    if !rest.is_empty() {
        return None;
    }
    Some(stamp(date, hour, minute, second, 0, None, None))
}

fn parse_date_time_offset(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, 'T')?;
    let (hour, minute, second, rest) = take_hms(rest)?;
    let offset = parse_offset(rest)?;
    Some(stamp(date, hour, minute, second, 0, Some(offset), None))
}

fn parse_date_time_frac_offset(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, 'T')?;
    let (hour, minute, second, nano, rest) = take_hms_frac(rest)?;
    let offset = parse_offset(rest)?;
    Some(stamp(date, hour, minute, second, nano, Some(offset), None))
}

fn parse_date_time_frac_zone(input: &str) -> Option<CivilParts> {
    let (date, rest) = split_date(input, ' ')?;
    let (hour, minute, second, nano, rest) = take_hms_frac(rest)?;
    let zone = rest
        .strip_prefix(' ')
        .filter(|name| !name.is_empty() && name.chars().all(is_zone_char))?;
    Some(stamp(date, hour, minute, second, nano, None, Some(zone.to_string())))
}

fn parts_from_date(date: Date) -> CivilParts {
    stamp(date, 0, 0, 0, 0, None, None)
}

fn stamp(
    date: Date,
    hour: i8,
    minute: i8,
    second: i8,
    nano: i32,
    offset_seconds: Option<i32>,
    zone_name: Option<String>,
) -> CivilParts {
    CivilParts {
        year: date.year(),
        month: date.month(),
        day: date.day(),
        hour,
        minute,
        second,
        nano,
        offset_seconds,
        zone_name,
    }
}

fn parts_to_timestamp(parts: CivilParts, default_zone: &TimeZone) -> Option<Timestamp> {
    let date = Date::new(parts.year, parts.month, parts.day).ok()?;
    let time = Time::new(parts.hour, parts.minute, parts.second, parts.nano).ok()?;
    let date_time = DateTime::from_parts(date, time);
    let zone = if let Some(seconds) = parts.offset_seconds {
        TimeZone::fixed(Offset::from_seconds(seconds).ok()?)
    } else if let Some(name) = parts.zone_name.as_deref() {
        resolve_zone(name)?
    } else {
        default_zone.clone()
    };
    Some(date_time.to_zoned(zone).ok()?.timestamp())
}

fn resolve_zone(name: &str) -> Option<TimeZone> {
    if name.eq_ignore_ascii_case("GMT") || name.eq_ignore_ascii_case("UTC") || name == "Z" {
        return Some(TimeZone::UTC);
    }
    TimeZone::get(name).ok()
}

fn split_date(input: &str, sep: char) -> Option<(Date, &str)> {
    let (date_text, rest) = input.split_once(sep)?;
    Some((parse_ymd(date_text)?, rest))
}

fn parse_ymd(input: &str) -> Option<Date> {
    let mut parts = input.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Date::new(year, month, day).ok()
}

fn parse_mdy(input: &str) -> Option<Date> {
    let mut parts = input.split('/');
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    let year = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Date::new(year, month, day).ok()
}

fn take_hm(input: &str) -> Option<(i8, i8, &str)> {
    let (hour, rest) = take_n_digits(input, 2)?;
    let rest = rest.strip_prefix(':')?;
    let (minute, rest) = take_n_digits(rest, 2)?;
    Some((hour as i8, minute as i8, rest))
}

fn take_hms(input: &str) -> Option<(i8, i8, i8, &str)> {
    let (hour, minute, rest) = take_hm(input)?;
    let rest = rest.strip_prefix(':')?;
    let (second, rest) = take_n_digits(rest, 2)?;
    Some((hour, minute, second as i8, rest))
}

fn take_hms_frac(input: &str) -> Option<(i8, i8, i8, i32, &str)> {
    let (hour, minute, second, rest) = take_hms(input)?;
    let rest = rest.strip_prefix('.')?;
    let len = rest.bytes().take_while(u8::is_ascii_digit).count();
    let digits = &rest[..len];
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    let nano = frac_to_nano(digits)?;
    Some((hour, minute, second, nano, &rest[len..]))
}

fn parse_hms(input: &str, allow_short: bool) -> Option<Time> {
    if let Some((hour, minute, second, nano, rest)) = take_hms_frac(input) {
        if rest.is_empty() {
            return Time::new(hour, minute, second, nano).ok();
        }
    }
    if allow_short {
        parse_clock(input, false)
    } else {
        None
    }
}

fn parse_clock(input: &str, with_seconds: bool) -> Option<Time> {
    if with_seconds {
        if let Some((hour, minute, second, rest)) = take_hms(input) {
            if rest.is_empty() {
                return Time::new(hour, minute, second, 0).ok();
            }
        }
    }
    let (hour, minute, rest) = take_hm(input)?;
    if !rest.is_empty() {
        return None;
    }
    Time::new(hour, minute, 0, 0).ok()
}

fn parse_ampm(input: &str) -> Option<Time> {
    let (clock, marker) = input.rsplit_once(' ')?;
    let pm = match marker.to_ascii_lowercase().as_str() {
        "am" => false,
        "pm" => true,
        _ => return None,
    };
    let (mut hour, minute, second) = if let Some((hour, minute, second, rest)) = take_hms(clock) {
        if !rest.is_empty() {
            return None;
        }
        (hour, minute, second)
    } else {
        let (hour, minute, rest) = take_hm(clock)?;
        if !rest.is_empty() {
            return None;
        }
        (hour, minute, 0)
    };
    if !(1..=12).contains(&hour) {
        return None;
    }
    if hour == 12 {
        hour = 0;
    }
    if pm {
        hour += 12;
    }
    Time::new(hour, minute, second, 0).ok()
}

fn parse_offset(input: &str) -> Option<i32> {
    if input == "Z" || input.eq_ignore_ascii_case("GMT") || input.eq_ignore_ascii_case("UTC") {
        return Some(0);
    }
    let (sign, rest) = match input.chars().next()? {
        '+' => (1, &input[1..]),
        '-' => (-1, &input[1..]),
        _ => return None,
    };
    if rest.len() == 4 && rest.chars().all(|ch| ch.is_ascii_digit()) {
        let hours: i32 = rest[..2].parse().ok()?;
        let minutes: i32 = rest[2..].parse().ok()?;
        return Some(sign * (hours * 3600 + minutes * 60));
    }
    if rest.len() == 5 && rest.as_bytes().get(2) == Some(&b':') {
        let hours: i32 = rest[..2].parse().ok()?;
        let minutes: i32 = rest[3..].parse().ok()?;
        return Some(sign * (hours * 3600 + minutes * 60));
    }
    None
}

fn take_n_digits(input: &str, n: usize) -> Option<(u32, &str)> {
    let head = input.get(..n)?;
    if !head.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &input[n..]))
}

/// Up to 9 fraction digits as nanoseconds: the digits padded with zeros on the right.
fn frac_to_nano(digits: &str) -> Option<i32> {
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut nano = 0i32;
    for byte in digits.bytes().chain(std::iter::repeat_n(b'0', 9 - digits.len())) {
        nano = nano * 10 + i32::from(byte - b'0');
    }
    Some(nano)
}

fn is_zone_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '/' || ch == '_' || ch == '-' || ch == '+'
}

fn is_integer_text(input: &str) -> bool {
    let digits = input.strip_prefix('-').unwrap_or(input);
    !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
}

fn date_to_timestamp(date: Date) -> Option<Timestamp> {
    let date_time = DateTime::from_parts(date, Time::midnight());
    Some(date_time.to_zoned(TimeZone::UTC).ok()?.timestamp())
}

fn date_epoch_millis(date: Date) -> i64 {
    date_to_timestamp(date)
        .map(|timestamp| timestamp.as_millisecond())
        .unwrap_or(0)
}

fn time_epoch_millis(time: Time) -> i64 {
    let date_time = DateTime::from_parts(Date::new(1970, 1, 1).expect("epoch date"), time);
    date_time
        .to_zoned(TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp().as_millisecond())
        .unwrap_or(0)
}

fn like_match(text: &str, pattern: &str) -> bool {
    let text: Vec<char> = text.chars().flat_map(|ch| ch.to_lowercase()).collect();
    let pattern: Vec<char> = pattern.chars().flat_map(|ch| ch.to_lowercase()).collect();
    // Two pointers: on a mismatch go back to the last `%` and let it take one more char. Only the last `%` needs
    // retrying, so this is linear in the text for each pattern char, with no recursion.
    let (mut ti, mut pi) = (0, 0);
    let mut last_wildcard: Option<(usize, usize)> = None;
    while ti < text.len() {
        match pattern.get(pi) {
            Some('%') => {
                pi += 1;
                last_wildcard = Some((pi, ti));
            }
            Some(&ch) if ch == '_' || ch == text[ti] => {
                ti += 1;
                pi += 1;
            }
            _ => match last_wildcard {
                Some((after, taken)) => {
                    pi = after;
                    ti = taken + 1;
                    last_wildcard = Some((after, ti));
                }
                None => return false,
            },
        }
    }
    pattern[pi..].iter().all(|ch| *ch == '%')
}

/// True when `pattern` matches the whole `text`.
pub fn regex_full_match(text: &str, pattern: &str) -> Result<bool, Error> {
    regex_match::is_full_match(text, pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser's `$mq.compare` (WebrootVue.qvt.js) is checked against the same vectors.
    #[test]
    fn compare_vectors() {
        let text =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/compare-vectors.json")).unwrap();
        let vectors: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        for vector in vectors {
            let field = crate::json::from_json(&vector[0]);
            let to = crate::json::from_json(&vector[2]);
            let got = compare(&field, vector[1].as_str().unwrap(), &to, vector[3].as_str().unwrap()).unwrap();
            assert_eq!(got, vector[4].as_bool().unwrap(), "{vector}");
        }
    }

    #[test]
    fn a_double_out_of_the_integer_range_does_not_convert() {
        assert_eq!(
            convert_to(&Value::F64(42.0), "Long", &TimeZone::UTC),
            Some(Value::I64(42))
        );
        assert_eq!(
            convert_to(&Value::F64(-9.223_372_036_854_776e18), "Long", &TimeZone::UTC),
            Some(Value::I64(i64::MIN))
        );
        assert_eq!(convert_to(&Value::F64(1e20), "Long", &TimeZone::UTC), None);
        assert_eq!(convert_to(&Value::F64(-1e20), "Long", &TimeZone::UTC), None);
        assert_eq!(
            convert_to(&Value::F64(9.223_372_036_854_776e18), "Long", &TimeZone::UTC),
            None
        );
        assert_eq!(convert_to(&Value::F64(f64::NAN), "Long", &TimeZone::UTC), None);
        assert_eq!(convert_to(&Value::F64(3e10), "Integer", &TimeZone::UTC), None);
        assert_eq!(convert_to(&Value::F64(7.5), "Integer", &TimeZone::UTC), None);
    }

    #[test]
    fn decimal_divide_uses_groovy_scale() {
        let quotient = decimal_divide(Decimal::from(1), Decimal::from(3)).unwrap();
        assert_eq!(quotient.to_string(), "0.3333333333");
        let sixth = decimal_divide(Decimal::from(1), Decimal::from(6)).unwrap();
        assert_eq!(sixth.to_string(), "0.1666666667");
        assert!(decimal_multiply(Decimal::MAX, Decimal::from(2)).is_err());
        assert!(decimal_eq(
            Decimal::from_str("1.0").unwrap(),
            Decimal::from_str("1.00").unwrap()
        ));
        assert!(
            !decimal_is_zero(Decimal::from_str("0.00").unwrap()) || decimal_is_zero(Decimal::from_str("0.00").unwrap())
        );
        assert!(decimal_is_zero(Decimal::from_str("0.00").unwrap()));
    }

    #[test]
    fn boolean_and_empty_follow_object_utilities() {
        let yes = basic_convert(&Value::string("Y"), "Boolean");
        assert_eq!(yes.value, Value::Bool(true));
        assert!(yes.changed);
        assert_eq!(
            basic_convert(&Value::string("yes"), "Boolean").value,
            Value::Bool(false)
        );
        assert_eq!(
            basic_convert(&Value::string("false"), "java.lang.Boolean").value,
            Value::Bool(false)
        );
        assert!(!is_empty(&Value::I32(0)));
        assert!(!is_empty(&Value::Bool(false)));
        assert!(is_empty(&Value::string("")));
        assert!(is_empty(&Value::Null));
        let bad = basic_convert(&Value::string("nope"), "Integer");
        assert!(!bad.changed);
        assert_eq!(bad.value, Value::string("nope"));
    }

    /// Timestamp text without an offset is read in the zone given, Java's `parseTimestamp` in the user's zone; an offset,
    /// `Z` and epoch millis are the moment they name in any zone, and a date or time stays a civil value.
    #[test]
    fn timestamp_text_is_read_in_the_zone_given() {
        let chicago = TimeZone::get("America/Chicago").unwrap();
        let stamp = |text: &str| Value::Timestamp(text.parse::<Timestamp>().unwrap());
        for (text, in_chicago, in_utc) in [
            ("2016-11-01", "2016-11-01T05:00:00Z", "2016-11-01T00:00:00Z"),
            ("2016-11-08 00:00:00.0", "2016-11-08T06:00:00Z", "2016-11-08T00:00:00Z"),
            ("2016-11-10 12:00", "2016-11-10T18:00:00Z", "2016-11-10T12:00:00Z"),
            ("2016-11-10T12:00:00Z", "2016-11-10T12:00:00Z", "2016-11-10T12:00:00Z"),
            (
                "2016-11-10T12:00:00-03:00",
                "2016-11-10T15:00:00Z",
                "2016-11-10T15:00:00Z",
            ),
            ("1478779200000", "2016-11-10T12:00:00Z", "2016-11-10T12:00:00Z"),
        ] {
            assert_eq!(
                basic_convert_in(&Value::string(text), "java.sql.Timestamp", &chicago).value,
                stamp(in_chicago),
                "{text}"
            );
            assert_eq!(
                convert_checked_in(&Value::string(text), "Timestamp", &chicago),
                Some(stamp(in_chicago)),
                "{text}"
            );
            assert_eq!(
                basic_convert(&Value::string(text), "java.sql.Timestamp").value,
                stamp(in_utc),
                "{text}"
            );
        }
        assert_eq!(
            basic_convert_in(&Value::string("2016-11-01"), "java.sql.Date", &chicago).value,
            Value::Date(Date::new(2016, 11, 1).unwrap())
        );
        assert!(compare_in(
            &Value::string("2016-11-01 00:00"),
            "equals",
            &stamp("2016-11-01T05:00:00Z"),
            "Timestamp",
            &chicago
        )
        .unwrap());
        // Unregistered, the text zone is UTC.
        if TEXT_TIME_ZONE.get().is_none() {
            assert_eq!(text_time_zone(), TimeZone::UTC);
        }
    }

    #[test]
    fn timestamps_dates_and_plain_millis() {
        let converted = basic_convert(&Value::string("2020-01-02 03:04:05"), "java.sql.Timestamp");
        let Value::Timestamp(timestamp) = converted.value else {
            panic!("not a timestamp");
        };
        let expected = parse_timestamp("2020-01-02T03:04:05Z", &TimeZone::UTC).unwrap();
        assert_eq!(timestamp, expected);
        assert_eq!(
            to_plain_string(&Value::Timestamp(timestamp)),
            expected.as_millisecond().to_string()
        );
        assert_eq!(parse_date("01/02/2020").unwrap(), Date::new(2020, 1, 2).unwrap());
        assert_eq!(parse_time("13:05").unwrap(), Time::new(13, 5, 0, 0).unwrap());
        assert_eq!(
            to_plain_string(&Value::Date(Date::new(1970, 1, 2).unwrap())),
            "86400000"
        );
    }

    #[test]
    fn like_and_compare() {
        assert!(compare_like(&Value::string("Hello"), &Value::string("h%")));
        assert!(compare_like(&Value::string("Hello"), &Value::string("h_llo")));
        assert!(!compare_like(&Value::string("Hello"), &Value::string("x%")));
        assert!(compare_like(&Value::string("a.b"), &Value::string("a.b")));
        assert!(compare_like(&Value::Null, &Value::Null) || compare_like(&Value::string("a"), &Value::Null));
        assert!(compare(&Value::string("10"), "greater", &Value::string("2"), "Integer").unwrap());
    }

    #[test]
    fn like_wildcards_match_without_backtracking_or_recursion() {
        let like = |text: &str, pattern: &str| compare_like(&Value::string(text), &Value::string(pattern));
        assert!(like("", "") && like("", "%") && like("", "%%") && !like("", "_"));
        assert!(like("abc", "%") && like("abc", "a%c") && like("abc", "%b%") && like("abc", "___"));
        assert!(!like("abc", "__") && !like("abc", "____") && !like("abc", "a%b") && !like("ab", "a_%b"));
        assert!(like("abcbc", "a%bc") && like("aXbYc", "a%b%c") && like("ABC", "a_c") && like("mississippi", "%sip%"));
        assert!(!like("mississippi", "%sipx%") && like("xaab", "%a_b") && !like("ab", "%a_b"));
        // A pattern that backtracks exponentially, and a text long enough to overflow the stack by recursion.
        let text = "a".repeat(5_000);
        let pattern = format!("{}b", "%a".repeat(40));
        assert!(!like(&text, &pattern));
        assert!(like(&"x".repeat(1_000_000), "%x"));
    }

    #[test]
    fn add_numbers_errors_on_integer_overflow() {
        assert_eq!(add_numbers(&Value::I64(1), &Value::I64(2)).unwrap(), Value::I64(3));
        assert_eq!(add_numbers(&Value::I32(1), &Value::I32(2)).unwrap(), Value::I32(3));
        let long = add_numbers(&Value::I64(i64::MAX), &Value::I64(1)).unwrap_err();
        assert!(long.to_string().contains("overflowed"), "{long}");
        let int = add_numbers(&Value::I32(i32::MAX), &Value::I32(1)).unwrap_err();
        assert!(int.to_string().contains("overflowed"), "{int}");
        assert!(add_numbers(&Value::I64(i64::MIN), &Value::I32(-1)).is_err());
        assert!(compare(&Value::string(""), "empty", &Value::Null, "String").unwrap());
        assert!(compare(&Value::string("abc"), "matches", &Value::string("a.*"), "").unwrap());
        assert!(compare(&Value::string("abc"), "matches", &Value::string("a.*c"), "").unwrap());
        assert!(compare(&Value::string("abc"), "matches", &Value::string("(ab|a)c"), "").unwrap());
        assert!(compare(&Value::string("abc"), "matches", &Value::string("[a-c]+"), "").unwrap());
        assert!(compare(&Value::string("12"), "matches", &Value::string(r"\d+"), "").unwrap());
        assert!(!compare(&Value::string("abc"), "matches", &Value::string("a.*d"), "").unwrap());
        let one = Decimal::from_str("1.0").unwrap();
        let one_two = Decimal::from_str("1.00").unwrap();
        assert!(!java_equals(&Value::Decimal(one), &Value::Decimal(one_two)));
        assert_eq!(decimal_compare(one, one_two), Ordering::Equal);
    }

    #[test]
    fn fractions_of_a_second_are_read_to_the_nanosecond() {
        for (text, nano) in [
            ("12:34:56.7", 700_000_000),
            ("12:34:56.789", 789_000_000),
            ("12:34:56.000001", 1_000),
            ("12:34:56.123456789", 123_456_789),
        ] {
            assert_eq!(parse_time(text).map(|time| time.subsec_nanosecond()), Some(nano), "{text}");
        }
        assert_eq!(parse_time("12:34:56.1234567890"), None);
        let stamp = parse_timestamp("2026-10-20 12:34:56.789", &TimeZone::UTC).unwrap();
        assert_eq!(stamp.as_millisecond().rem_euclid(1000), 789);
    }

    #[test]
    fn non_ascii_date_time_text_is_rejected() {
        for text in [
            "1é",
            "é",
            "1é:00",
            "12:é0",
            "12:0é",
            "12:00:0é",
            "１２:００",
            "10:30 ｐｍ",
        ] {
            assert_eq!(parse_time(text), None, "{text}");
        }
        for text in ["2024-01-0é", "é/01/2024", "2024-01-01 1é", "２０２４-01-01"] {
            assert_eq!(parse_date(text), None, "{text}");
        }
        for text in [
            "2024-01-01 1é",
            "2024-01-01 12:é0",
            "2024-01-01 12:00:0é",
            "2024-01-01T12:00:00+é:00",
            "2024-01-01 12:00:00.1é",
        ] {
            assert_eq!(parse_timestamp(text, &TimeZone::UTC), None, "{text}");
        }
    }
}
