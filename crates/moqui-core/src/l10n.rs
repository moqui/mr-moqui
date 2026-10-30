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

//! `ec.l10n`: localized messages and formatting in the session locale and time zone.
//!
//! The formatters are in `moqui_lib::format`. The locale is the session locale, else `en_US`.
//! The time zone is the session time zone (the user's `UserAccount.timeZone`), else the server's: the zone
//! `default_time_zone` names at start (MoquiDefaultConf's `UTC`), as Java's `TimeZone.setDefault` makes it the JVM
//! default that `ec.user.getTimeZone()` falls back to.
//!
//! Timestamp text without an offset is read in the session time zone wherever text becomes a timestamp, as Java's
//! `ec.l10n.parseTimestamp` does for a parameter, a field set from text, a data file and a data check:
//! [`basic_convert`] and [`convert_checked`] are `moqui_lib`'s conversions in that zone. Only text a database driver
//! gives back is read in UTC (`moqui_lib::object::basic_convert`). Dates and times are civil values with no zone.

use std::sync::RwLock;

use jiff::tz::TimeZone;
use moqui_lib::format;
use moqui_lib::format::Decimal;
use moqui_lib::object::{to_plain_string, Converted, Value};
use moqui_lib::sync::{read, write};

use crate::context::{Error, ExecutionContext};

const LOCALIZED_MESSAGE: &str = "moqui.basic.LocalizedMessage";
const MAX_ORIGINAL: usize = 255;
const DEFAULT_LOCALE: &str = "en_US";

/// The session locale such as `en_US`, else the request's `Accept-Language`, else `en_US` (Java's `UserFacade.getLocale`
/// falls back to the request locale the same way).
pub fn locale() -> String {
    crate::user::locale()
        .ok()
        .flatten()
        .filter(|text| !text.is_empty())
        .or_else(crate::web::request_locale)
        .map(|text| text.replace('-', "_"))
        .unwrap_or_else(|| DEFAULT_LOCALE.to_string())
}

/// The session time zone, else the server's ([`server_time_zone`]): Java's `ec.user.getTimeZone()`.
pub fn time_zone() -> TimeZone {
    crate::user::time_zone()
        .ok()
        .flatten()
        .filter(|name| !name.trim().is_empty())
        .and_then(|name| TimeZone::get(name.trim()).ok())
        .unwrap_or_else(server_time_zone)
}

/// The server's zone once [`init_server_time_zone`] has read it.
static SERVER_TIME_ZONE: RwLock<Option<TimeZone>> = RwLock::new(None);

/// Java's start, which sets the JVM default zone (`TimeZone.setDefault`) to the one the `default_time_zone` property
/// or environment variable names, `UTC` from MoquiDefaultConf unless the runtime names another. An unknown name is
/// logged and leaves the zone as it was. Also registers [`time_zone`] as the zone `moqui_lib` reads timestamp text in
/// where it converts a person's or a script's text (a condition value against a timestamp column).
pub fn init_server_time_zone() {
    moqui_lib::object::set_text_time_zone(time_zone);
    let name = moqui_lib::env::get_prop_or_env("default_time_zone");
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    match TimeZone::get(name) {
        Ok(zone) => {
            *write(&SERVER_TIME_ZONE) = Some(zone);
            tracing::info!("Default time zone {name}");
        }
        Err(err) => tracing::error!("Error setting default time zone to {name}: {err}"),
    }
}

/// The zone a session with no user zone is in: what `default_time_zone` named at start, else the machine's (the JVM
/// default with no property), else UTC. A unit test with no start has UTC, MoquiDefaultConf's default, so it reads the
/// same on any machine.
pub fn server_time_zone() -> TimeZone {
    let named = read(&SERVER_TIME_ZONE).clone();
    named.unwrap_or_else(|| {
        if cfg!(test) {
            TimeZone::UTC
        } else {
            TimeZone::try_system().unwrap_or(TimeZone::UTC)
        }
    })
}

/// The server zone's name for `ec.user.timeZone` with no user zone: the IANA name, else `UTC`.
pub fn server_time_zone_name() -> String {
    server_time_zone().iana_name().unwrap_or("UTC").to_string()
}

/// `moqui_lib::object::basic_convert` with timestamp text that has no offset read in the session time zone, as Java's
/// `EntityValue.setString`, `setFields` and `ParameterInfo.convertType` read it through `ec.l10n.parseTimestamp`. The
/// zone is looked up only for timestamp text.
pub fn basic_convert(value: &Value, java_type: &str) -> Converted {
    if is_timestamp_text(value, java_type) {
        moqui_lib::object::basic_convert_in(value, java_type, &time_zone())
    } else {
        moqui_lib::object::basic_convert(value, java_type)
    }
}

/// `moqui_lib::object::convert_checked` in the session time zone, as [`basic_convert`].
pub fn convert_checked(value: &Value, java_type: &str) -> Option<Value> {
    if is_timestamp_text(value, java_type) {
        moqui_lib::object::convert_checked_in(value, java_type, &time_zone())
    } else {
        moqui_lib::object::convert_checked(value, java_type)
    }
}

/// `moqui_lib::object::compare` (`if-compare` with a `type`) in the session time zone, as [`basic_convert`].
///
/// # Errors
///
/// Returns an error for values an ordered operator cannot order, or a bad `matches` pattern.
pub fn compare(
    field: &Value,
    operator: &str,
    to_field: &Value,
    java_type: &str,
) -> Result<bool, moqui_lib::error::Error> {
    if is_timestamp_text(field, java_type) || is_timestamp_text(to_field, java_type) {
        moqui_lib::object::compare_in(field, operator, to_field, java_type, &time_zone())
    } else {
        moqui_lib::object::compare(field, operator, to_field, java_type)
    }
}

fn is_timestamp_text(value: &Value, java_type: &str) -> bool {
    matches!(value, Value::String(_)) && java_type.rsplit('.').next() == Some("Timestamp")
}

/// The text of `original` in the session locale. With no `LocalizedMessage` row the text is returned as is, minus any
/// `##` suffix used to tell two uses of the same words apart. A text longer than 255 characters cannot be a
/// `LocalizedMessage.original`, so it is returned unchanged (logged at debug): a long paragraph a screen passes through
/// `localize` shows as written, where Java threw.
///
/// # Errors
///
/// Returns an error when the message cache fails.
pub fn localize(original: &str) -> Result<String, Error> {
    localize_in(original, &locale())
}

/// [`localize`] in `locale`.
///
/// # Errors
///
/// Same as [`localize`].
pub fn localize_in(original: &str, locale: &str) -> Result<String, Error> {
    if original.is_empty() {
        return Ok(String::new());
    }
    let length = original.chars().count();
    if length > MAX_ORIGINAL {
        tracing::debug!(length, "text of more than {MAX_ORIGINAL} characters is not localized");
        return Ok(original.to_string());
    }
    let key = format!("{original}::{locale}");
    let cache = ExecutionContext::with_current(|ec| ec.factory().cache().get("l10n.message"))
        .and_then(Result::ok)
        .flatten();
    if let Some(cache) = &cache {
        clear_after_message_write(cache)?;
        if let Some(found) = cache.get(&key)? {
            return Ok(moqui_lib::object::to_plain_string(&found));
        }
    }
    let (result, store) = lookup(original, locale);
    if store {
        if let Some(cache) = &cache {
            cache.put(key, Value::string(&result))?;
        }
    }
    Ok(result)
}

/// The `LocalizedEntityField`/`LocalizedMessage` write count the message cache was last filled at, kept with the entity
/// facade.
#[derive(Default)]
struct MessageGeneration(std::sync::atomic::AtomicU64);

/// Clear the message cache once a `LocalizedMessage` has been written since it was filled, here or on a cluster peer, so
/// a translation loaded or edited on a running server shows without waiting for the cache to expire (an original with
/// no row is cached as itself).
fn clear_after_message_write(cache: &moqui_lib::cache::MCache<String, Value>) -> Result<(), Error> {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(());
    };
    let generation = crate::entity::cache::localized_generation(&facade);
    let seen = facade.with_slot_read(MessageGeneration::default, |seen| {
        seen.0.swap(generation, std::sync::atomic::Ordering::AcqRel)
    });
    if seen != generation {
        cache.clear()?;
    }
    Ok(())
}

/// The message, and whether it is safe to cache (false when there was no entity facade).
fn lookup(original: &str, locale: &str) -> (String, bool) {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return (original.to_string(), false);
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    // A lookup that fails (no database yet, a lost connection) is not a missing message: the result is not cached.
    let failed = std::cell::Cell::new(false);
    let find = |text: &str, locale: &str| -> Option<String> {
        let found = facade.find(LOCALIZED_MESSAGE).and_then(|find| {
            find.eq("original", Value::string(text))
                .eq("locale", Value::string(locale))
                .use_cache(true)
                .one()
        });
        match found {
            Ok(row) => row.and_then(|row| row.get("localized").map(to_plain_string)),
            Err(_) => {
                failed.set(true);
                None
            }
        }
    };
    let chain = |text: &str| -> Option<String> {
        find(text, locale)
            .or_else(|| locale.split_once('_').and_then(|(language, _)| find(text, language)))
            .or_else(|| find(text, "default"))
    };
    if let Some(found) = chain(original) {
        return (found, true);
    }
    if failed.get() {
        return (original.to_string(), false);
    }
    // Words that precede a `##` suffix are the message when the full text has no row.
    let close_curly = original.rfind('}');
    if let Some(hash) = original
        .rfind("##")
        .filter(|hash| *hash > 0 && close_curly.is_none_or(|close| *hash > close))
    {
        let base = &original[..hash];
        let text = chain(base).unwrap_or_else(|| base.to_string());
        return (text, !failed.get());
    }
    (original.to_string(), !failed.get())
}

fn decimal_of(value: &Value) -> Option<Decimal> {
    match value {
        Value::Decimal(number) => Some(*number),
        Value::I32(number) => Some(Decimal::from(*number)),
        Value::I64(number) => Some(Decimal::from(*number)),
        Value::F64(number) => format!("{number}").parse().ok(),
        // Text as the user's locale writes numbers, as Java's BigDecimalValidator with the locale.
        Value::String(text) => format::parse_decimal(text, &locale()),
        _ => None,
    }
}

/// Whether `pattern` is a Java date pattern (`SimpleDateFormat`, as `L10nFacade` formats with) rather than a
/// `DecimalFormat` number pattern: outside quotes it has a date or time field letter (`G y Y M L w W D d F E u a H
/// k K h m s S z Z X x`) and none of the number pattern's digits `#` and `0`. So `h:mm a`, `mm:ss` and `EEE` are
/// dates, `#,##0.00`, `0.###E0` and `#,##0 'units'` numbers.
pub fn is_date_pattern(pattern: &str) -> bool {
    let mut quoted = false;
    let mut date_letter = false;
    for ch in pattern.chars() {
        if ch == '\'' {
            quoted = !quoted;
        } else if !quoted {
            if ch == '#' || ch == '0' {
                return false;
            }
            date_letter |= "GyYMLwWDdFEuaHkKhmsSzZXx".contains(ch);
        }
    }
    date_letter
}

/// Epoch millis text, as a time stamp reaches a script or a server-static screen's data (a `Timestamp` field is its
/// millis text there): 12 or 13 digits from 2001-09-09 (`1000000000000`) to before 2100. `ec.l10n.format` with no
/// pattern reads such text as the time stamp it was, which Java, holding the `Timestamp`, formats `yyyy-MM-dd HH:mm`.
pub fn epoch_millis_text(text: &str) -> Option<i64> {
    let text = text.trim();
    if !(12..=13).contains(&text.len()) || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>()
        .ok()
        .filter(|millis| (1_000_000_000_000..4_102_444_800_000).contains(millis))
}

/// `ec.l10n.format`: a value as text. A string is itself. Dates use the zone, numbers use the
/// pattern or the default of grouping with up to twelve fraction digits, and a map or list is JSON.
///
/// # Errors
///
/// Returns an error for a pattern that is not supported.
pub fn format_value(value: &Value, pattern: Option<&str>) -> Result<String, Error> {
    let locale = locale();
    let pattern = pattern.filter(|text| !text.is_empty());
    Ok(match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Timestamp(stamp) => format::format_timestamp(*stamp, pattern, &time_zone())?,
        Value::Date(date) => format::format_date(*date, pattern)?,
        Value::Time(time) => format::format_time(*time, pattern)?,
        Value::F64(number) => format::format_float(*number, pattern, &locale)?,
        Value::I32(_) | Value::I64(_) | Value::Decimal(_) => {
            let number = decimal_of(value).unwrap_or_default();
            format::format_number(number, pattern, &locale)?
        }
        Value::List(_) | Value::Map(_) => {
            let json = moqui_lib::json::to_json(value).to_string();
            if json.len() > 128 {
                serde_json::to_string_pretty(&moqui_lib::json::to_json(value)).unwrap_or(json)
            } else {
                json
            }
        }
        other => to_plain_string(other),
    })
}

struct CurrencyInfo {
    symbol: String,
    fraction_digits: Option<usize>,
}

/// ISO currencies the framework data uses. Others print their code and two fraction digits.
fn iso_currency(code: &str) -> Option<(&'static str, usize)> {
    Some(match code {
        "USD" => ("$", 2),
        "EUR" => ("\u{20ac}", 2),
        "GBP" => ("\u{a3}", 2),
        "JPY" => ("\u{a5}", 0),
        "CNY" => ("CN\u{a5}", 2),
        "INR" => ("\u{20b9}", 2),
        "CAD" => ("CA$", 2),
        "AUD" => ("A$", 2),
        "MXN" => ("MX$", 2),
        "BRL" => ("R$", 2),
        "CHF" => ("CHF", 2),
        "KRW" => ("\u{20a9}", 0),
        _ => return None,
    })
}

/// ISO 4217 minor units of the currencies that do not have two, for a code with no symbol of ours: Java's
/// `Currency.getDefaultFractionDigits`.
fn iso_minor_digits(code: &str) -> usize {
    match code {
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX" | "UYI" | "VND"
        | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        _ => 2,
    }
}

fn currency_info(uom_id: Option<&str>) -> CurrencyInfo {
    let mut info = CurrencyInfo {
        symbol: String::new(),
        fraction_digits: None,
    };
    let Some(uom_id) = uom_id.filter(|text| !text.is_empty()) else {
        return info;
    };
    let mut symbol = None;
    if let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        // From the entity cache: a form-list formats a currency on every row.
        let row = facade.find("moqui.basic.Uom").ok().and_then(|find| {
            find.eq("uomId", Value::string(uom_id))
                .eq("uomTypeEnumId", Value::string("UT_CURRENCY_MEASURE"))
                .use_cache(true)
                .one()
                .ok()
                .flatten()
        });
        if let Some(row) = row {
            symbol = row.get("symbol").map(to_plain_string).filter(|text| !text.is_empty());
            info.fraction_digits = row
                .get("fractionDigits")
                .and_then(decimal_of)
                .and_then(|digits| usize::try_from(digits.trunc().mantissa()).ok());
        }
    }
    let iso = iso_currency(uom_id);
    info.symbol = symbol.unwrap_or_else(|| iso.map_or_else(|| uom_id.to_string(), |(symbol, _)| symbol.to_string()));
    if info.fraction_digits.is_none() {
        info.fraction_digits = Some(iso.map_or_else(|| iso_minor_digits(uom_id), |(_, digits)| digits));
    }
    info
}

/// `ec.l10n.formatCurrency`. A string amount is parsed, an empty one is empty text. The symbol and
/// fraction digits come from the `Uom` row of type currency, else the ISO table, else two digits.
///
/// # Errors
///
/// Returns an error for an amount that is not a number.
pub fn format_currency(
    amount: &Value,
    uom_id: Option<&str>,
    fraction_digits: Option<usize>,
    hide_symbol: bool,
) -> Result<String, Error> {
    if amount.is_null() || amount.as_str().is_some_and(str::is_empty) {
        return Ok(String::new());
    }
    let number = decimal_of(amount)
        .ok_or_else(|| Error::new(format!("cannot format {} as currency", to_plain_string(amount))))?;
    let info = currency_info(uom_id);
    let digits = fraction_digits.or(info.fraction_digits).unwrap_or(2);
    let symbol = if hide_symbol { "" } else { info.symbol.as_str() };
    Ok(format::format_currency(number, symbol, digits, &locale()))
}

/// A time stamp in the session time zone from the usual forms (epoch milliseconds, ISO, `yyyy-MM-dd HH:mm:ss`). None
/// for text that is not a time stamp.
pub fn parse_timestamp(input: &str) -> Option<Value> {
    moqui_lib::object::parse_timestamp(input, &time_zone()).map(Value::Timestamp)
}

/// `ec.l10n.parseTimestamp(input, format)`: by the Java pattern first, in the session time zone unless the text has an
/// offset, then the usual forms, as Java's L10nFacade.
pub fn parse_timestamp_format(input: &str, format: Option<&str>) -> Option<Value> {
    if input.trim().is_empty() {
        return None;
    }
    if let Some(pattern) = format.filter(|pattern| !pattern.is_empty()) {
        if let Some(stamp) = moqui_lib::datefmt::parse(input, pattern).and_then(|parsed| parsed_timestamp(&parsed)) {
            return Some(Value::Timestamp(stamp));
        }
    }
    parse_timestamp(input)
}

fn parsed_timestamp(parsed: &moqui_lib::datefmt::ParsedDate) -> Option<jiff::Timestamp> {
    let nanos = parsed.millisecond.unwrap_or(0).checked_mul(1_000_000)?;
    let civil = jiff::civil::DateTime::new(
        parsed.year.unwrap_or(1970),
        parsed.month.unwrap_or(1),
        parsed.day.unwrap_or(1),
        parsed.hour.unwrap_or(0),
        parsed.minute.unwrap_or(0),
        parsed.second.unwrap_or(0),
        nanos,
    )
    .ok()?;
    match parsed.offset_seconds {
        Some(seconds) => jiff::tz::Offset::from_seconds(seconds).ok()?.to_timestamp(civil).ok(),
        None => civil.to_zoned(time_zone()).ok().map(|zoned| zoned.timestamp()),
    }
}

/// `ec.l10n.parseDate(input, format)`: the pattern (default `yyyy-MM-dd`), then `MM/dd/yyyy` and ISO, then the usual
/// forms. A date has no time zone, as in Java.
pub fn parse_date_format(input: &str, format: Option<&str>) -> Option<Value> {
    let format = format.filter(|pattern| !pattern.is_empty()).unwrap_or("yyyy-MM-dd");
    for pattern in [format, "MM/dd/yyyy", "yyyy-MM-dd'T'HH:mm:ssZ"] {
        if let Some(parsed) = moqui_lib::datefmt::parse(input, pattern) {
            if let Ok(date) = jiff::civil::Date::new(
                parsed.year.unwrap_or(1970),
                parsed.month.unwrap_or(1),
                parsed.day.unwrap_or(1),
            ) {
                return Some(Value::Date(date));
            }
        }
    }
    moqui_lib::object::parse_date(input).map(Value::Date)
}

/// `ec.l10n.parseTime(input, format)`: the pattern (default `HH:mm:ss.SSS`), then `HH:mm:ss`, `HH:mm`, `h:mm a` and
/// `h:mm:ss a`, then the usual forms.
pub fn parse_time_format(input: &str, format: Option<&str>) -> Option<Value> {
    let format = format.filter(|pattern| !pattern.is_empty()).unwrap_or("HH:mm:ss.SSS");
    for pattern in [format, "HH:mm:ss", "HH:mm", "h:mm a", "h:mm:ss a"] {
        if let Some(parsed) = moqui_lib::datefmt::parse(input, pattern) {
            let nanos = parsed.millisecond.unwrap_or(0).saturating_mul(1_000_000);
            if let Ok(time) = jiff::civil::Time::new(
                parsed.hour.unwrap_or(0),
                parsed.minute.unwrap_or(0),
                parsed.second.unwrap_or(0),
                nanos,
            ) {
                return Some(Value::Time(time));
            }
        }
    }
    moqui_lib::object::parse_time(input).map(Value::Time)
}

/// `ec.l10n.parseNumber(input, format)`: a number as the session locale writes it. None for text that is not one.
pub fn parse_number(input: &str) -> Option<Value> {
    format::parse_decimal(input, &locale()).map(Value::Decimal)
}

/// [`parse_number`] by the Java `DecimalFormat` pattern `format` when there is one, as `ec.l10n.parseNumber(input,
/// format)`. None for text the pattern does not read.
pub fn parse_number_format(input: &str, format: Option<&str>) -> Option<Value> {
    match format.filter(|pattern| !pattern.is_empty()) {
        Some(pattern) => format::parse_decimal_pattern(input, pattern, &locale()).map(Value::Decimal),
        None => parse_number(input),
    }
}

/// `ec.l10n.roundCurrency(amount, uomId, precise)`: rounded half up to the currency's fraction digits (one more when
/// `precise`), from its `Uom` row, else ISO 4217, as Java does.
///
/// # Errors
///
/// Returns an error for an amount that is not a number.
pub fn round_currency(amount: &Value, uom_id: &str, precise: bool) -> Result<Value, Error> {
    round_currency_with(amount, uom_id, precise, format::RoundingStrategy::MidpointAwayFromZero)
}

/// [`round_currency`] with the rounding mode of Java's four-argument form.
///
/// # Errors
///
/// Returns an error for an amount that is not a number.
pub fn round_currency_with(
    amount: &Value,
    uom_id: &str,
    precise: bool,
    strategy: format::RoundingStrategy,
) -> Result<Value, Error> {
    if amount.is_null() {
        return Ok(Value::Null);
    }
    let number = decimal_of(amount)
        .ok_or_else(|| Error::new(format!("cannot round {} as currency", to_plain_string(amount))))?;
    let digits = currency_info(Some(uom_id)).fraction_digits.unwrap_or(2) + usize::from(precise);
    let digits = u32::try_from(digits).unwrap_or(2);
    Ok(Value::Decimal(number.round_dp_with_strategy(digits, strategy)))
}

#[cfg(test)]
#[path = "l10n_tests.rs"]
mod tests;
