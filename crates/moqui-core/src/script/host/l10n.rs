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

//! `ec.l10n`: localized text, formatting and parsing in the session's locale and time zone.

use moqui_lib::format::RoundingStrategy;
use moqui_lib::object::{to_plain_string, Value};

use super::{method, Call, HostClass, HostError, Ret};
use crate::context::Error;
use crate::l10n;

pub(super) static L10N: HostClass = HostClass {
    name: "host:L10n",
    methods: &[
        method!("ec.l10n", "localize", [Value, OptValue], localize),
        method!("ec.l10n", "format", [Value, OptValue, OptValue, OptValue], format),
        method!(
            "ec.l10n",
            "formatCurrency",
            [Value, OptValue, OptValue],
            format_currency
        ),
        method!(
            "ec.l10n",
            "formatCurrencyNoSymbol",
            [Value, OptValue, OptValue],
            format_currency_no_symbol
        ),
        method!("ec.l10n", "parseTimestamp", [Value, OptValue], parse_timestamp),
        method!("ec.l10n", "parseDate", [Value, OptValue], parse_date),
        method!("ec.l10n", "parseTime", [Value, OptValue], parse_time),
        method!("ec.l10n", "parseNumber", [Value, OptValue], parse_number),
        method!(
            "ec.l10n",
            "roundCurrency",
            [Value, OptValue, OptValue, OptValue],
            round_currency
        ),
        // `formatNumber(n, format)` and `formatValue` are `format` for a number, as in Java.
        method!("ec.l10n", "formatNumber", [Value, OptValue, OptValue, OptValue], format),
        method!("ec.l10n", "formatValue", [Value, OptValue, OptValue, OptValue], format),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// A library failure as the facade throws it: a framework error of the message.
fn failed(message: &str) -> HostError {
    HostError::Context(Error::new(message))
}

/// `localize(original, locale)`: in the given locale, else the session's.
fn localize(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = to_plain_string(&call.value_or_null(0));
    let localized = match call.opt_text(1).filter(|locale| !locale.is_empty()) {
        Some(locale) => l10n::localize_in(&text, &locale.replace('-', "_")),
        None => l10n::localize(&text),
    };
    localized.map(Ret::from).map_err(|err| failed(err.message()))
}

/// `format(value, pattern)`, and Java's `format(value, pattern, locale, timeZone)`: a time stamp in the named zone (an
/// IANA name; empty or null is the session's), as the call handling schedules read a local time.
fn format(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let host = call.value_or_null(0);
    let pattern = call.opt_text(1);
    let host = match pattern.as_deref() {
        // Any date pattern (`h:mm a` as well as `yyyy-MM-dd`), as `l10n::is_date_pattern` reads one.
        Some(pattern) if l10n::is_date_pattern(pattern) => epoch_millis_timestamp(&host).unwrap_or(host),
        // No pattern: a time stamp is epoch millis text in a script (`temporal_text`), so such text is formatted as the
        // time stamp, `yyyy-MM-dd HH:mm`, as Java formats the `Timestamp` it holds.
        Some("") | None => match &host {
            Value::String(text) => l10n::epoch_millis_text(text)
                .and_then(|millis| epoch_millis_timestamp(&Value::I64(millis)))
                .unwrap_or(host),
            _ => host,
        },
        _ => host,
    };
    let zone = call.opt_text(3).filter(|zone| !zone.trim().is_empty());
    if let (Some(zone), Value::Timestamp(stamp)) = (zone.as_deref(), &host) {
        let zone = jiff::tz::TimeZone::get(zone.trim())
            .map_err(|_| HostError::Context(Error::new(format!("Time zone {zone} is not known"))))?;
        let pattern = pattern
            .as_deref()
            .filter(|p| !p.is_empty())
            .unwrap_or("yyyy-MM-dd HH:mm");
        return moqui_lib::format::format_timestamp(*stamp, Some(pattern), &zone)
            .map(Ret::from)
            .map_err(|err| failed(err.message()));
    }
    l10n::format_value(&host, pattern.as_deref())
        .map(Ret::from)
        .map_err(|err| failed(err.message()))
}

/// A date or timestamp reaches a script as epoch millis, so a whole number formatted with a date pattern is a
/// timestamp.
fn epoch_millis_timestamp(value: &Value) -> Option<Value> {
    let millis = match value {
        Value::I64(millis) => *millis,
        Value::I32(millis) => i64::from(*millis),
        #[allow(clippy::cast_possible_truncation)] // a whole number
        Value::F64(millis) if millis.fract() == 0.0 => *millis as i64,
        Value::String(text) => match text.trim().parse::<i64>() {
            Ok(millis) => millis,
            // A script holds a date as text such as `2026-10-16` or `2026-10-16 08:30:00`, which the date pattern formats.
            Err(_) if is_date_text(text) => {
                // Date only text is a Date, which has no time zone to shift it.
                let java_type = if text.trim().len() == 10 {
                    "java.sql.Date"
                } else {
                    "java.sql.Timestamp"
                };
                // In the session time zone, which the pattern formats it in, so the text shows as written.
                let converted = crate::l10n::basic_convert(value, java_type);
                return (!matches!(converted.value, Value::Null | Value::String(_))).then_some(converted.value);
            }
            Err(_) => return None,
        },
        _ => return None,
    };
    Some(moqui_lib::object::basic_convert(&Value::I64(millis), "java.sql.Timestamp").value)
}

/// Text that begins `yyyy-MM-dd`.
fn is_date_text(text: &str) -> bool {
    let bytes = text.trim().as_bytes();
    bytes.len() >= 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn currency(call: &Call<'_>, hide_symbol: bool) -> Result<Ret, HostError> {
    let amount = call.value_or_null(0);
    let uom = call.opt_text(1);
    let digits = call.opt_text(2).and_then(|digits| digits.parse::<usize>().ok());
    l10n::format_currency(&amount, uom.as_deref(), digits, hide_symbol)
        .map(Ret::from)
        .map_err(|err| failed(err.message()))
}

fn format_currency(call: &mut Call<'_>) -> Result<Ret, HostError> {
    currency(call, false)
}

fn format_currency_no_symbol(call: &mut Call<'_>) -> Result<Ret, HostError> {
    currency(call, true)
}

fn parsed_or_null(value: Option<Value>) -> Result<Ret, HostError> {
    Ok(Ret::Value(value.unwrap_or(Value::Null)))
}

fn parse_timestamp(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let input = to_plain_string(&call.value_or_null(0));
    parsed_or_null(l10n::parse_timestamp_format(&input, call.opt_text(1).as_deref()))
}

fn parse_date(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let input = to_plain_string(&call.value_or_null(0));
    parsed_or_null(l10n::parse_date_format(&input, call.opt_text(1).as_deref()))
}

fn parse_time(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let input = to_plain_string(&call.value_or_null(0));
    parsed_or_null(l10n::parse_time_format(&input, call.opt_text(1).as_deref()))
}

fn parse_number(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let input = to_plain_string(&call.value_or_null(0));
    parsed_or_null(l10n::parse_number(&input))
}

fn round_currency(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let amount = call.value_or_null(0);
    let uom = call.opt_text(1).unwrap_or_else(|| "USD".to_string());
    let precise = matches!(call.value(2), Some(Value::Bool(true)));
    // The fourth argument is a `RoundingMode` name or an old `ROUND_*` number; none is HALF_UP, as Java.
    let strategy = match call.opt_text(3) {
        Some(mode) => rounding_strategy_named(&mode)
            .map_err(HostError::Message)?
            .ok_or_else(|| HostError::message("rounding necessary"))?,
        None => RoundingStrategy::MidpointAwayFromZero,
    };
    let rounded = l10n::round_currency_with(&amount, &uom, precise, strategy).map_err(HostError::Context)?;
    Ok(Ret::Value(rounded))
}

/// A Java `RoundingMode` by name or old `ROUND_*` number; `None` for `UNNECESSARY` or nothing. An unknown mode is the
/// error's message.
pub(crate) fn rounding_strategy_named(mode: &str) -> Result<Option<RoundingStrategy>, String> {
    Ok(match mode {
        "UP" | "0" => Some(RoundingStrategy::AwayFromZero),
        "DOWN" | "1" => Some(RoundingStrategy::ToZero),
        "CEILING" | "2" => Some(RoundingStrategy::ToPositiveInfinity),
        "FLOOR" | "3" => Some(RoundingStrategy::ToNegativeInfinity),
        "HALF_UP" | "4" => Some(RoundingStrategy::MidpointAwayFromZero),
        "HALF_DOWN" | "5" => Some(RoundingStrategy::MidpointTowardZero),
        "HALF_EVEN" | "6" => Some(RoundingStrategy::MidpointNearestEven),
        "" | "UNNECESSARY" | "7" => None,
        other => return Err(format!("rounding mode {other} is not known")),
    })
}
