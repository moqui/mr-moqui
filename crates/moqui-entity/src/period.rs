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

//! Period ranges for search forms: "this week", "last month", "next 7 days". A port of
//! `UserFacadeImpl.getPeriodRange`. The range is `[from, thru)` in the user's time zone, and the
//! week starts on Sunday as it does for Java's default US calendar.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};
use moqui_lib::object::{basic_convert, Value};

use crate::error::Error;

/// The range for `period` (`day`, `week`, `weeks`, `month`, `months`, `quarter`, `year`, `7d`,
/// `30r`, and so on), shifted by `offset` periods from `basis`, or from today when `basis` is empty.
///
/// # Errors
///
/// Returns an error when `offset` is not a whole number or `basis` is not a `yyyy-MM-dd` date or epoch millis.
pub fn period_range(period: &str, offset: &str, basis: &str) -> Result<(Value, Value), Error> {
    period_range_in_zones(period, offset, basis, &zone(), &crate::hooks::server_time_zone())
}

fn period_range_in_zones(
    period: &str,
    offset: &str,
    basis: &str,
    zone: &TimeZone,
    server_zone: &TimeZone,
) -> Result<(Value, Value), Error> {
    let offset = if offset.trim().is_empty() {
        0
    } else {
        offset
            .trim()
            .parse::<i32>()
            .map_err(|_| Error::new(format!("period offset {offset} is not a whole number")))?
    };
    let basis = if basis.trim().is_empty() {
        Timestamp::now().to_zoned(zone.clone()).date()
    } else if let Ok(millis) = basis.trim().parse::<i64>() {
        millis_date(millis, zone).ok_or_else(|| Error::new(format!("period date {basis} is out of range")))?
    } else {
        // Change 7: Java parses pdate at JVM-default midnight, then transfers that instant to the user's calendar.
        // Keep this conversion local to period ranges; a general civil Date still has no time zone.
        let parsed = basis
            .trim()
            .parse::<Date>()
            .map_err(|_| Error::new(format!("period date {basis} is not yyyy-MM-dd or epoch millis")))?;
        parsed
            .at(0, 0, 0, 0)
            .to_zoned(server_zone.clone())
            .map_err(|err| Error::new(format!("period date {basis} has no server midnight: {err}")))?
            .timestamp()
            .to_zoned(zone.clone())
            .date()
    };
    let (from, thru) = range_dates(period, offset, basis)?;
    Ok((stamp(from, zone)?, stamp(thru, zone)?))
}

/// Java transfers a Date/Timestamp's epoch millis into the user's calendar even at UTC midnight.
fn millis_date(millis: i64, zone: &TimeZone) -> Option<Date> {
    Some(Timestamp::from_millisecond(millis).ok()?.to_zoned(zone.clone()).date())
}

/// The session time zone, else the server's, as Java's `ec.user.getCalendarSafe()`.
fn zone() -> TimeZone {
    crate::hooks::time_zone()
}

fn stamp(date: Date, zone: &TimeZone) -> Result<Value, Error> {
    let zoned = date
        .at(0, 0, 0, 0)
        .to_zoned(zone.clone())
        .map_err(|err| Error::new(format!("period date {date} has no start of day: {err}")))?;
    Ok(basic_convert(&Value::I64(zoned.timestamp().as_millisecond()), "java.sql.Timestamp").value)
}

fn add_days(date: Date, days: i32) -> Result<Date, Error> {
    Span::new()
        .try_days(days)
        .and_then(|span| date.checked_add(span))
        .map_err(|err| Error::new(format!("period date out of range: {err}")))
}

fn add_weeks(date: Date, weeks: i32) -> Result<Date, Error> {
    add_days(date, weeks.saturating_mul(7))
}

fn add_months(date: Date, months: i32) -> Result<Date, Error> {
    Span::new()
        .try_months(months)
        .and_then(|span| date.checked_add(span))
        .map_err(|err| Error::new(format!("period date out of range: {err}")))
}

fn start_of_week(date: Date) -> Result<Date, Error> {
    add_days(date, -i32::from(date.weekday().to_sunday_zero_offset()))
}

fn range_dates(period: &str, offset: i32, basis: Date) -> Result<(Date, Date), Error> {
    let period = if period.trim().is_empty() {
        "day".to_string()
    } else {
        period.trim().to_ascii_lowercase()
    };
    let numbered = period.chars().next().is_some_and(|ch| ch.is_ascii_digit());
    if let Some(count) = period.strip_suffix('d').filter(|_| numbered) {
        let days = days_of(&period, count)?;
        return if offset < 0 {
            Ok((add_days(basis, offset.saturating_mul(days))?, add_days(basis, 1)?))
        } else {
            let count = offset.checked_add(1).ok_or_else(|| offset_error(offset))?;
            Ok((basis, add_days(basis, count.saturating_mul(days))?))
        };
    }
    if let Some(count) = period.strip_suffix('r').filter(|_| numbered) {
        let days = days_of(&period, count)?;
        let around = offset
            .checked_abs()
            .ok_or_else(|| offset_error(offset))?
            .saturating_mul(days);
        return Ok((add_days(basis, -around)?, add_days(basis, around)?));
    }
    match period.as_str() {
        "week" => {
            let from = add_weeks(start_of_week(basis)?, offset)?;
            Ok((from, add_weeks(from, 1)?))
        }
        "weeks" => {
            let week = start_of_week(basis)?;
            if offset < 0 {
                Ok((add_weeks(week, offset + 1)?, add_weeks(week, 1)?))
            } else {
                Ok((week, add_weeks(week, if offset == 0 { 1 } else { offset })?))
            }
        }
        "month" => {
            let from = add_months(basis.first_of_month(), offset)?;
            Ok((from, add_months(from, 1)?))
        }
        "months" => {
            let month = basis.first_of_month();
            if offset < 0 {
                Ok((add_months(month, offset + 1)?, add_months(month, 1)?))
            } else {
                Ok((month, add_months(month, if offset == 0 { 1 } else { offset })?))
            }
        }
        "quarter" => {
            let quarter_start = basis
                .first_of_month()
                .with()
                .month((basis.month() - 1) / 3 * 3 + 1)
                .build();
            let from = add_months(
                quarter_start.map_err(|err| Error::new(format!("quarter start: {err}")))?,
                offset.saturating_mul(3),
            )?;
            Ok((from, add_months(from, 3)?))
        }
        "year" => {
            let from = add_months(basis.first_of_year(), offset.saturating_mul(12))?;
            Ok((from, add_months(from, 12)?))
        }
        _ => {
            let from = add_days(basis, offset)?;
            Ok((from, add_days(from, 1)?))
        }
    }
}

fn offset_error(offset: i32) -> Error {
    Error::new(format!("period offset {offset} is out of range"))
}

/// The number of days `count` names, the part of `period` before its unit letter.
fn days_of(period: &str, count: &str) -> Result<i32, Error> {
    count
        .parse::<i32>()
        .map_err(|_| Error::new(format!("period {period} does not start with a number of days")))
}

#[cfg(test)]
#[path = "period_tests.rs"]
mod tests;
