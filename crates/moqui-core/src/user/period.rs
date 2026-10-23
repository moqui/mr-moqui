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

use crate::context::Error;

/// The range for `period` (`day`, `week`, `weeks`, `month`, `months`, `quarter`, `year`, `7d`,
/// `30r`, and so on), shifted by `offset` periods from `basis`, or from today when `basis` is empty.
///
/// # Errors
///
/// Returns an error when `offset` is not a whole number or `basis` is not a `yyyy-MM-dd` date or epoch millis.
pub fn period_range(period: &str, offset: &str, basis: &str) -> Result<(Value, Value), Error> {
    let zone = zone();
    let (from, thru) = range_dates(period, parse_offset(offset)?, basis_date(basis, &zone)?)?;
    Ok((stamp(from, &zone)?, stamp(thru, &zone)?))
}

/// The first and last days of the range for `period`, as `UserFacadeImpl.getPeriodDescription` shows them.
///
/// # Errors
///
/// As [`period_range`].
pub fn period_days(period: &str, offset: &str, basis: &str) -> Result<(Date, Date), Error> {
    let (from, thru) = range_dates(period, parse_offset(offset)?, basis_date(basis, &zone())?)?;
    Ok((from, add_days(thru, -1)?))
}

fn parse_offset(offset: &str) -> Result<i32, Error> {
    if offset.trim().is_empty() {
        return Ok(0);
    }
    offset
        .trim()
        .parse::<i32>()
        .map_err(|_| Error::new(format!("period offset {offset} is not a whole number")))
}

fn basis_date(basis: &str, zone: &TimeZone) -> Result<Date, Error> {
    if basis.trim().is_empty() {
        Ok(moqui_lib::clock::now().to_zoned(zone.clone()).date())
    } else if let Ok(millis) = basis.trim().parse::<i64>() {
        // a Date or Timestamp parameter reaches a script as epoch millis
        Ok(Timestamp::from_millisecond(millis)
            .map_err(|_| Error::new(format!("period date {basis} is out of range")))?
            .to_zoned(zone.clone())
            .date())
    } else {
        basis
            .trim()
            .parse::<Date>()
            .map_err(|_| Error::new(format!("period date {basis} is not yyyy-MM-dd or epoch millis")))
    }
}

fn zone() -> TimeZone {
    super::session::time_zone()
        .ok()
        .flatten()
        .and_then(|name| TimeZone::get(&name).ok())
        .unwrap_or_else(TimeZone::system)
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
    if numbered && period.ends_with('d') {
        let days = days_of(&period)?;
        return if offset < 0 {
            Ok((add_days(basis, offset.saturating_mul(days))?, add_days(basis, 1)?))
        } else {
            let count = offset.checked_add(1).ok_or_else(|| offset_error(offset))?;
            Ok((basis, add_days(basis, count.saturating_mul(days))?))
        };
    }
    if numbered && period.ends_with('r') {
        let days = days_of(&period)?;
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

fn days_of(period: &str) -> Result<i32, Error> {
    period[..period.len() - 1]
        .parse::<i32>()
        .map_err(|_| Error::new(format!("period {period} does not start with a number of days")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(text: &str) -> Date {
        text.parse().unwrap()
    }

    fn range(period: &str, offset: i32, basis: &str) -> (String, String) {
        let (from, thru) = range_dates(period, offset, d(basis)).unwrap();
        (from.to_string(), thru.to_string())
    }

    #[test]
    fn an_extreme_offset_is_an_error_not_an_overflow() {
        for period in ["7d", "7r", "day", "week", "weeks", "month", "months", "quarter", "year"] {
            for offset in [i32::MAX, i32::MIN] {
                assert!(
                    range_dates(period, offset, d("2024-03-15")).is_err(),
                    "{period} {offset}"
                );
            }
        }
    }

    #[test]
    fn day_week_month_quarter_year_ranges_match_the_java_calendar() {
        // 2026-10-14 is a Wednesday.
        assert_eq!(
            range("day", 0, "2026-10-14"),
            ("2026-10-14".into(), "2026-10-15".into())
        );
        assert_eq!(
            range("day", -1, "2026-10-14"),
            ("2026-10-13".into(), "2026-10-14".into())
        );
        assert_eq!(
            range("week", 0, "2026-10-14"),
            ("2026-10-11".into(), "2026-10-18".into())
        );
        assert_eq!(
            range("week", -1, "2026-10-14"),
            ("2026-10-04".into(), "2026-10-11".into())
        );
        assert_eq!(
            range("month", 0, "2026-10-14"),
            ("2026-10-01".into(), "2026-11-01".into())
        );
        assert_eq!(
            range("month", 1, "2026-12-31"),
            ("2027-01-01".into(), "2027-02-01".into())
        );
        assert_eq!(
            range("quarter", 0, "2026-11-20"),
            ("2026-10-01".into(), "2027-01-01".into())
        );
        assert_eq!(
            range("quarter", -1, "2026-02-10"),
            ("2025-10-01".into(), "2026-01-01".into())
        );
        assert_eq!(
            range("year", 0, "2026-10-14"),
            ("2026-01-01".into(), "2027-01-01".into())
        );
        assert_eq!(
            range("year", -1, "2026-10-14"),
            ("2025-01-01".into(), "2026-01-01".into())
        );
    }

    #[test]
    fn an_epoch_millis_basis_is_the_date_it_falls_on() {
        // noon UTC on 2026-10-14 is that date in every zone within eleven hours of UTC
        assert_eq!(
            super::period_range("month", "0", "1791979200000").unwrap(),
            super::period_range("month", "0", "2026-10-14").unwrap()
        );
    }

    #[test]
    fn counted_ranges_look_back_forward_or_around() {
        // Past 7 days including today, next 7 days from today, and plus or minus 7.
        assert_eq!(
            range("7d", -1, "2026-10-14"),
            ("2026-10-07".into(), "2026-10-15".into())
        );
        assert_eq!(range("7d", 0, "2026-10-14"), ("2026-10-14".into(), "2026-10-21".into()));
        assert_eq!(range("7r", 1, "2026-10-14"), ("2026-10-07".into(), "2026-10-21".into()));
        assert_eq!(
            range("months", -2, "2026-10-14"),
            ("2026-09-01".into(), "2026-11-01".into())
        );
        assert_eq!(
            range("weeks", 2, "2026-10-14"),
            ("2026-10-11".into(), "2026-10-25".into())
        );
    }

    #[test]
    fn the_public_function_returns_timestamps_and_checks_its_input() {
        let (from, thru) = period_range("day", "0", "2026-10-14").unwrap();
        assert!(matches!(from, Value::Timestamp(_)) && matches!(thru, Value::Timestamp(_)));
        assert!(period_range("day", "x", "").is_err());
        assert!(period_range("day", "0", "14/10/2026").is_err());
    }
}
