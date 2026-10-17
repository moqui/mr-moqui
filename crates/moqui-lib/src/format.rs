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

//! Number, currency, and date formatting with the Java `DecimalFormat` and date pattern subset
//! that screens use.
//!
//! Number patterns: `#`, `0`, `,` grouping, `.` decimal, literal prefix and suffix, quotes,
//! `%` and per-mille scaling, `\u{a4}` for the currency symbol, and a `;` negative sub-pattern.
//! Rounding is `HALF_EVEN`, as Java does, so 0.125 at two places is 0.12. An exponent pattern is
//! an error. Date patterns are in [`crate::datefmt`]. Separators come from a small language
//! table, not from CLDR, and a language that is not in it uses the English ones.
//!
//! The shared vectors are in `testdata/format-vectors.json`. The browser formatter is checked
//! against the same file.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned};
pub use rust_decimal::prelude::ToPrimitive;
pub use rust_decimal::{Decimal, RoundingStrategy};

use crate::error::Error;

const DEFAULT_MAX_FRACTION: usize = 12;
const CURRENCY_SIGN: char = '\u{a4}';

/// Grouping and decimal separators for a locale such as `en_US`, `de` or `fr-CA`.
fn separators(locale: &str) -> (&'static str, &'static str) {
    let language = locale.split(['_', '-']).next().unwrap_or("");
    let region = locale.split(['_', '-']).nth(1).unwrap_or("");
    match (language, region) {
        ("de" | "it", "CH") => ("\u{2019}", "."),
        ("fr", "CH") => ("\u{202f}", ","),
        ("es", "MX") => (",", "."),
        ("de" | "es" | "it" | "nl" | "pt" | "id" | "tr" | "da" | "el" | "ro" | "hr" | "sl", _) => (".", ","),
        ("fr", _) => ("\u{202f}", ","),
        ("ru" | "pl" | "cs" | "sk" | "sv" | "fi" | "nb" | "no" | "uk" | "bg" | "hu" | "lt" | "lv", _) => {
            ("\u{a0}", ",")
        }
        _ => (",", "."),
    }
}

/// The currency pattern of a locale, as the JDK's CLDR data has it. `number` is the digits
/// part, such as `#,##0.00`.
fn currency_pattern(locale: &str, number: &str) -> String {
    let language = locale.split(['_', '-']).next().unwrap_or("");
    let region = locale.split(['_', '-']).nth(1).unwrap_or("");
    match (language, region) {
        ("nl", "NL") => format!("{CURRENCY_SIGN}\u{a0}{number};{CURRENCY_SIGN}\u{a0}-{number}"),
        ("de" | "it", "CH") => format!("{CURRENCY_SIGN}\u{a0}{number};{CURRENCY_SIGN}-{number}"),
        ("pt", "BR") => format!("{CURRENCY_SIGN}\u{a0}{number}"),
        ("es", "MX") => format!("{CURRENCY_SIGN}{number}"),
        _ if currency_suffix(language) => format!("{number}\u{a0}{CURRENCY_SIGN}"),
        _ => format!("{CURRENCY_SIGN}{number}"),
    }
}

/// Whether the currency symbol follows the number in this language. `currency_pattern` handles
/// the regions that differ.
fn currency_suffix(language: &str) -> bool {
    matches!(
        language,
        "de" | "fr"
            | "es"
            | "it"
            | "nl"
            | "pt"
            | "ru"
            | "pl"
            | "cs"
            | "sk"
            | "sv"
            | "fi"
            | "nb"
            | "no"
            | "da"
            | "uk"
            | "bg"
            | "hu"
            | "lt"
            | "lv"
            | "ro"
            | "hr"
            | "sl"
            | "el"
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Layout {
    prefix: String,
    suffix: String,
    min_int: usize,
    min_frac: usize,
    max_frac: usize,
    group: usize,
    scale: i32,
}

fn parse_pattern(pattern: &str, symbol: &str) -> Result<(Layout, Option<Layout>), Error> {
    let mut halves = split_negative(pattern);
    let positive = parse_half(&halves.remove(0), symbol, pattern)?;
    let negative = match halves.pop() {
        Some(half) => {
            let mut parsed = parse_half(&half, symbol, pattern)?;
            // The negative sub-pattern only supplies the affixes.
            parsed.min_int = positive.min_int;
            parsed.min_frac = positive.min_frac;
            parsed.max_frac = positive.max_frac;
            parsed.group = positive.group;
            parsed.scale = positive.scale;
            Some(parsed)
        }
        None => None,
    };
    Ok((positive, negative))
}

fn split_negative(pattern: &str) -> Vec<String> {
    let mut halves = vec![String::new()];
    let mut quoted = false;
    for ch in pattern.chars() {
        if ch == '\'' {
            quoted = !quoted;
        }
        if ch == ';' && !quoted && halves.len() == 1 {
            halves.push(String::new());
            continue;
        }
        if let Some(last) = halves.last_mut() {
            last.push(ch);
        }
    }
    halves
}

fn parse_half(half: &str, symbol: &str, whole: &str) -> Result<Layout, Error> {
    let mut layout = Layout::default();
    let mut section = 0; // 0 prefix, 1 number, 2 suffix
    let mut in_fraction = false;
    let mut last_group_at: Option<usize> = None;
    let mut int_digits = 0;
    let chars: Vec<char> = half.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        index += 1;
        if ch == '\'' {
            let mut literal = String::new();
            if chars.get(index) == Some(&'\'') {
                literal.push('\'');
                index += 1;
            } else {
                while index < chars.len() {
                    if chars[index] == '\'' {
                        if chars.get(index + 1) != Some(&'\'') {
                            break;
                        }
                        index += 1;
                    }
                    literal.push(chars[index]);
                    index += 1;
                }
                index += 1;
            }
            push_affix(&mut layout, &mut section, &literal);
            continue;
        }
        let number_char = matches!(ch, '#' | '0' | ',' | '.');
        if number_char && section < 2 {
            section = 1;
            match ch {
                '#' => {
                    if in_fraction {
                        layout.max_frac += 1;
                    } else {
                        int_digits += 1;
                    }
                }
                '0' => {
                    if in_fraction {
                        layout.max_frac += 1;
                        layout.min_frac += 1;
                    } else {
                        int_digits += 1;
                        layout.min_int += 1;
                    }
                }
                ',' => last_group_at = Some(int_digits),
                _ => in_fraction = true,
            }
            continue;
        }
        if ch == 'E' && section == 1 {
            return Err(Error::new(format!("exponent number pattern is not supported: {whole}")));
        }
        let text = match ch {
            CURRENCY_SIGN => symbol.to_string(),
            '%' => {
                layout.scale = 2;
                "%".to_string()
            }
            '\u{2030}' => {
                layout.scale = 3;
                "\u{2030}".to_string()
            }
            other => other.to_string(),
        };
        push_affix(&mut layout, &mut section, &text);
    }
    if let Some(at) = last_group_at {
        layout.group = int_digits - at;
    }
    Ok(layout)
}

fn push_affix(layout: &mut Layout, section: &mut usize, text: &str) {
    if *section == 0 {
        layout.prefix.push_str(text);
    } else {
        *section = 2;
        layout.suffix.push_str(text);
    }
}

/// Digits of `value` rounded half-even to `max_frac` places, as `(integer, fraction)` text.
fn digits(value: Decimal, layout: &Layout) -> (String, String) {
    let mut value = value.abs();
    for _ in 0..layout.scale {
        value = value.checked_mul(Decimal::TEN).unwrap_or(value);
    }
    let max_frac = u32::try_from(layout.max_frac).unwrap_or(u32::MAX).min(28);
    let rounded = value.round_dp_with_strategy(max_frac, RoundingStrategy::MidpointNearestEven);
    let text = format!("{:.*}", max_frac as usize, rounded);
    let (int_part, frac_part) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let mut frac = frac_part.trim_end_matches('0').to_string();
    while frac.len() < layout.min_frac {
        frac.push('0');
    }
    let mut int_text = int_part.trim_start_matches('0').to_string();
    while int_text.len() < layout.min_int {
        int_text.insert(0, '0');
    }
    // Java prints a zero integer digit when the pattern asks for none, so `#.##` gives 0.5 and 0.
    if int_text.is_empty() {
        int_text.push('0');
    }
    (int_text, frac)
}

fn group_digits(int_text: &str, size: usize, separator: &str) -> String {
    if size == 0 || int_text.len() <= size {
        return int_text.to_string();
    }
    let mut out = String::new();
    let lead = int_text.len() % size;
    if lead > 0 {
        out.push_str(&int_text[..lead]);
    }
    let mut at = lead;
    while at < int_text.len() {
        if !out.is_empty() {
            out.push_str(separator);
        }
        out.push_str(&int_text[at..at + size]);
        at += size;
    }
    out
}

fn render(value: Decimal, positive: &Layout, negative: Option<&Layout>, locale: &str) -> String {
    let (group_sep, decimal_sep) = separators(locale);
    let (int_text, frac) = digits(value, positive);
    let zero = int_text.chars().all(|c| c == '0') && frac.chars().all(|c| c == '0');
    let negative_value = value.is_sign_negative() && !zero;
    let used = if negative_value { negative } else { None };
    let (prefix, suffix) = match used {
        Some(layout) => (layout.prefix.clone(), layout.suffix.clone()),
        None if negative_value => (format!("-{}", positive.prefix), positive.suffix.clone()),
        None => (positive.prefix.clone(), positive.suffix.clone()),
    };
    let mut out = prefix;
    out.push_str(&group_digits(&int_text, positive.group, group_sep));
    if !frac.is_empty() {
        out.push_str(decimal_sep);
        out.push_str(&frac);
    }
    out.push_str(&suffix);
    out
}

/// A number written for `locale`, as Java's `BigDecimalValidator` reads it: the locale's grouping separator (or a
/// space) is dropped and its decimal separator is the point. A leading `-` or `+` is the sign. None for text that is
/// not a number.
pub fn parse_decimal(text: &str, locale: &str) -> Option<Decimal> {
    let (grouping, decimal) = separators(locale);
    let mut plain = String::with_capacity(text.len());
    for ch in text.trim().chars() {
        if grouping.contains(ch) || matches!(ch, '\u{a0}' | '\u{202f}' | ' ') && !decimal.contains(ch) {
            continue;
        }
        if decimal.contains(ch) {
            plain.push('.');
        } else {
            plain.push(ch);
        }
    }
    if plain.is_empty() || plain.matches('.').count() > 1 {
        return None;
    }
    plain.parse().ok()
}

/// A number written by the `DecimalFormat` `pattern` for `locale`, as Java's `BigDecimalValidator.validate(text,
/// pattern, locale)` reads it: the pattern's prefix and suffix around the digits (the negative sub-pattern's, else `-`
/// before the prefix, for a negative number), the locale's decimal separator, its grouping separator only when the
/// pattern groups, and a `%` or per-mille pattern scaled back. None for text the pattern does not read.
pub fn parse_decimal_pattern(text: &str, pattern: &str, locale: &str) -> Option<Decimal> {
    let (positive, negative) = parse_pattern(pattern, "").ok()?;
    let text = text.trim();
    let implicit = Layout {
        prefix: format!("-{}", positive.prefix),
        ..positive.clone()
    };
    let negative = negative.unwrap_or(implicit);
    let strip = |layout: &Layout| {
        text.strip_prefix(layout.prefix.as_str())
            .and_then(|rest| rest.strip_suffix(layout.suffix.as_str()))
    };
    // The longer prefix first, so `-` is not read as part of the digits.
    let (body, negate) = match (strip(&negative), strip(&positive)) {
        (Some(body), _) if negative.prefix.len() >= positive.prefix.len() => (body, true),
        (_, Some(body)) => (body, false),
        (Some(body), None) => (body, true),
        (None, None) => return None,
    };
    let (grouping, decimal) = separators(locale);
    let mut plain = String::with_capacity(body.len());
    for ch in body.chars() {
        if ch.is_ascii_digit() {
            plain.push(ch);
        } else if decimal.contains(ch) {
            plain.push('.');
        } else if positive.group > 0 && (grouping.contains(ch) || matches!(ch, '\u{a0}' | '\u{202f}')) {
            continue;
        } else {
            return None;
        }
    }
    if plain.is_empty() || plain == "." || plain.matches('.').count() > 1 {
        return None;
    }
    let mut value: Decimal = plain.parse().ok()?;
    for _ in 0..positive.scale {
        value = value.checked_div(Decimal::TEN)?;
    }
    Some(if negate { -value } else { value })
}

/// A number as text. With no `pattern` it has grouping, at least one integer digit and up to 12
/// fraction digits, which is what `L10nFacade.format` does for a number with no format.
///
/// # Errors
///
/// Returns an error for a pattern with an exponent.
pub fn format_number(value: Decimal, pattern: Option<&str>, locale: &str) -> Result<String, Error> {
    let pattern = match pattern.filter(|text| !text.is_empty()) {
        Some(text) => text.to_string(),
        None => format!("#,##0.{}", "#".repeat(DEFAULT_MAX_FRACTION)),
    };
    let (positive, negative) = parse_pattern(&pattern, "")?;
    Ok(render(value, &positive, negative.as_ref(), locale))
}

/// A number from a float, using the shortest decimal text for it as Java does.
///
/// # Errors
///
/// Same as [`format_number`], and an error for NaN or infinity.
pub fn format_float(value: f64, pattern: Option<&str>, locale: &str) -> Result<String, Error> {
    if !value.is_finite() {
        return Err(Error::new("cannot format a number that is not finite"));
    }
    let decimal = format!("{value}")
        .parse::<Decimal>()
        .or_else(|_| format!("{value:e}").parse::<Decimal>())
        .map_err(|err| Error::new(format!("cannot format {value}: {err}")))?;
    format_number(decimal, pattern, locale)
}

/// Currency text. `symbol` is the currency symbol or empty to hide it. The fraction digit count
/// is both the minimum and the maximum.
pub fn format_currency(value: Decimal, symbol: &str, fraction_digits: usize, locale: &str) -> String {
    let frac = if fraction_digits > 0 {
        format!(".{}", "0".repeat(fraction_digits))
    } else {
        String::new()
    };
    let number = format!("#,##0{frac}");
    let pattern = if symbol.is_empty() {
        number
    } else {
        currency_pattern(locale, &number)
    };
    match parse_pattern(&pattern, symbol) {
        Ok((positive, negative)) => render(value, &positive, negative.as_ref(), locale),
        Err(_) => value.to_string(),
    }
}

/// A time stamp in `zone`. The default pattern is `yyyy-MM-dd HH:mm`.
///
/// # Errors
///
/// Returns an error for a date pattern letter that is not supported.
pub fn format_timestamp(value: Timestamp, pattern: Option<&str>, zone: &TimeZone) -> Result<String, Error> {
    let zoned: Zoned = value.to_zoned(zone.clone());
    crate::datefmt::format(
        &zoned,
        pattern.filter(|text| !text.is_empty()).unwrap_or("yyyy-MM-dd HH:mm"),
    )
}

/// A date. Dates have no zone, so the civil date is formatted as is. Default `yyyy-MM-dd`.
///
/// # Errors
///
/// Returns an error for a date pattern letter that is not supported.
pub fn format_date(value: Date, pattern: Option<&str>) -> Result<String, Error> {
    let zoned = value
        .to_datetime(Time::midnight())
        .to_zoned(TimeZone::UTC)
        .map_err(|err| Error::new(err.to_string()))?;
    crate::datefmt::format(&zoned, pattern.filter(|text| !text.is_empty()).unwrap_or("yyyy-MM-dd"))
}

/// A time of day. Default `HH:mm:ss`.
///
/// # Errors
///
/// Returns an error for a date pattern letter that is not supported.
pub fn format_time(value: Time, pattern: Option<&str>) -> Result<String, Error> {
    let zoned = DateTime::from_parts(Date::constant(1970, 1, 1), value)
        .to_zoned(TimeZone::UTC)
        .map_err(|err| Error::new(err.to_string()))?;
    crate::datefmt::format(&zoned, pattern.filter(|text| !text.is_empty()).unwrap_or("HH:mm:ss"))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use jiff::tz::TimeZone;
    use jiff::Timestamp;
    use rust_decimal::Decimal;

    use super::*;

    #[test]
    fn shared_vectors() {
        let text = include_str!("../testdata/format-vectors.json");
        let vectors: serde_json::Value = serde_json::from_str(text).expect("vectors parse");
        let list = vectors.as_array().expect("vectors are a list");
        assert!(list.len() > 20);
        for vector in list {
            let kind = vector["kind"].as_str().unwrap_or("");
            let expect = vector["expect"].as_str().expect("expect");
            let locale = vector["locale"].as_str().unwrap_or("en_US");
            let pattern = vector["pattern"].as_str();
            let got = match kind {
                "number" => {
                    let value = Decimal::from_str(vector["value"].as_str().expect("value")).expect("decimal");
                    format_number(value, pattern, locale).expect("number")
                }
                "currency" => {
                    let value = Decimal::from_str(vector["value"].as_str().expect("value")).expect("decimal");
                    let digits = usize::try_from(vector["fractionDigits"].as_u64().unwrap_or(2)).unwrap_or(2);
                    format_currency(value, vector["symbol"].as_str().unwrap_or(""), digits, locale)
                }
                "timestamp" => {
                    let value = Timestamp::from_str(vector["value"].as_str().expect("value")).expect("timestamp");
                    let zone = TimeZone::get(vector["tz"].as_str().unwrap_or("UTC")).expect("zone");
                    format_timestamp(value, pattern, &zone).expect("timestamp")
                }
                "date" => {
                    let value = Date::from_str(vector["value"].as_str().expect("value")).expect("date");
                    format_date(value, pattern).expect("date")
                }
                "time" => {
                    let value = Time::from_str(vector["value"].as_str().expect("value")).expect("time");
                    format_time(value, pattern).expect("time")
                }
                other => panic!("unknown vector kind {other}"),
            };
            assert_eq!(got, expect, "{vector}");
        }
    }

    #[test]
    fn half_even_rounding() {
        let value = Decimal::from_str("0.125").unwrap();
        assert_eq!(format_number(value, Some("0.00"), "en_US").unwrap(), "0.12");
        let value = Decimal::from_str("0.135").unwrap();
        assert_eq!(format_number(value, Some("0.00"), "en_US").unwrap(), "0.14");
        let value = Decimal::from_str("2.5").unwrap();
        assert_eq!(format_number(value, Some("0"), "en_US").unwrap(), "2");
    }

    #[test]
    fn floats_use_their_shortest_text() {
        assert_eq!(format_float(0.125, Some("0.00"), "en_US").unwrap(), "0.12");
        assert_eq!(format_float(1234.5, None, "en_US").unwrap(), "1,234.5");
        assert!(format_float(f64::NAN, None, "en_US").is_err());
    }

    #[test]
    fn an_exponent_pattern_is_an_error() {
        assert!(format_number(Decimal::ONE, Some("0.###E0"), "en_US").is_err());
    }

    /// `BigDecimalValidator.validate(text, pattern, locale)`: the pattern's affixes, the locale's separators, grouping
    /// only when the pattern groups, and a percent scaled back.
    #[test]
    fn a_number_reads_by_its_pattern() {
        let read = |text: &str, pattern: &str, locale: &str| parse_decimal_pattern(text, pattern, locale);
        let decimal = |text: &str| Some(Decimal::from_str(text).unwrap());
        assert_eq!(read("1,234.50", "#,##0.00", "en_US"), decimal("1234.50"));
        assert_eq!(read("1.234,5", "#,##0.00", "de"), decimal("1234.5"));
        assert_eq!(read("-12.5", "0.00", "en_US"), decimal("-12.5"));
        assert_eq!(read("(12.5)", "0.00;(0.00)", "en_US"), decimal("-12.5"));
        assert_eq!(read("$12", "'$'0", "en_US"), decimal("12"));
        assert_eq!(read("50%", "0%", "en_US"), decimal("0.5"));
        // no grouping in the pattern, no prefix in the text, and text that is not a number do not read
        assert_eq!(read("1,234", "0", "en_US"), None);
        assert_eq!(read("12", "'$'0", "en_US"), None);
        assert_eq!(read("abc", "0.00", "en_US"), None);
    }
}
