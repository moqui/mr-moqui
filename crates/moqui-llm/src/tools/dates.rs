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

//! Date phrases (plans/LLM-A2A-Assist.md 7.7): a date argument may be a phrase the server resolves in the user's time
//! zone, and the result echoes what was used (`resolved: {thruDate: "2026-09-30 23:59:59"}`). The phrases: `now`,
//! `today`, `yesterday`, `tomorrow`; `N days ago`, `in N weeks` (day, week, month, year); `start of last week`, `end
//! of this month`, `beginning of next quarter` (day, week, month, quarter, year, with this, last or next); and `last
//! month` or `this year` alone, which is the start of it, or its end for a thru, to or end argument
//! ([`is_end_name`]).

use jiff::civil::{Date, Weekday};
use jiff::{Span, Zoned};

/// Whether an argument name is the end of a range (`thruDate`, `createdDate_thru`, `toDate`, `endDate`).
pub fn is_end_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with("thru") || lower.contains("thrudate") || lower.starts_with("to") || lower.starts_with("end")
}

/// Whether an argument name is a date (`fromDate`, `orderDate_from`, `createdStamp`, `entryDate_period`).
pub fn is_date_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with("date")
        || lower.ends_with("_from")
        || lower.ends_with("_thru")
        || lower.ends_with("stamp")
        || lower.ends_with("time")
        || lower.contains("date_")
}

/// The day a week starts on, from its name; Monday when it is not one.
pub fn weekday_of(name: &str) -> Weekday {
    match name.trim().to_ascii_lowercase().get(..3).unwrap_or("") {
        "sun" => Weekday::Sunday,
        "sat" => Weekday::Saturday,
        "tue" => Weekday::Tuesday,
        "wed" => Weekday::Wednesday,
        "thu" => Weekday::Thursday,
        "fri" => Weekday::Friday,
        _ => Weekday::Monday,
    }
}

/// The date and time a phrase stands for, as `YYYY-MM-DD HH:mm` (a start or a moment) or `YYYY-MM-DD 23:59:59` (an
/// end when `end` or the phrase says so); `None` when the text is not a phrase, such as a date already written out.
pub fn resolve(phrase: &str, now: &Zoned, week_start: Weekday, end: bool) -> Option<String> {
    let words: Vec<String> = phrase
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let today = now.date();
    let day = |date: Date, end: bool| {
        if end {
            format!("{} 23:59:59", date.strftime("%Y-%m-%d"))
        } else {
            format!("{} 00:00", date.strftime("%Y-%m-%d"))
        }
    };
    match words.as_slice() {
        ["now"] => return Some(now.strftime("%Y-%m-%d %H:%M").to_string()),
        ["today"] => return Some(day(today, end)),
        ["yesterday"] => return Some(day(today.yesterday().ok()?, end)),
        ["tomorrow"] => return Some(day(today.tomorrow().ok()?, end)),
        [count, unit, "ago"] => {
            let count: i64 = count.parse().ok()?;
            return Some(day(shift(today, unit, -count)?, end));
        }
        ["in", count, unit] => {
            let count: i64 = count.parse().ok()?;
            return Some(day(shift(today, unit, count)?, end));
        }
        _ => {}
    }
    let (at_end, rest): (bool, &[&str]) = match words.as_slice() {
        ["start" | "beginning", "of", rest @ ..] => (false, rest),
        ["end", "of", rest @ ..] => (true, rest),
        rest => (end, rest),
    };
    let (offset, unit) = match rest {
        ["today"] => (0, "day"),
        ["yesterday"] => (-1, "day"),
        ["tomorrow"] => (1, "day"),
        ["this" | "the", unit] => (0, *unit),
        ["last" | "previous", unit] => (-1, *unit),
        ["next", unit] => (1, *unit),
        _ => return None,
    };
    let (first, last) = period(today, unit.trim_end_matches('s'), offset, week_start)?;
    Some(day(if at_end { last } else { first }, at_end))
}

/// `date` moved by `count` of `unit` (day, week, month, year).
fn shift(date: Date, unit: &str, count: i64) -> Option<Date> {
    let span = match unit.trim_end_matches('s') {
        "day" => Span::new().try_days(count).ok()?,
        "week" => Span::new().try_weeks(count).ok()?,
        "month" => Span::new().try_months(count).ok()?,
        "year" => Span::new().try_years(count).ok()?,
        _ => return None,
    };
    date.checked_add(span).ok()
}

/// The first and last day of the period of `unit` `offset` periods from the one `today` is in.
fn period(today: Date, unit: &str, offset: i64, week_start: Weekday) -> Option<(Date, Date)> {
    match unit {
        "day" => {
            let date = shift(today, "day", offset)?;
            Some((date, date))
        }
        "week" => {
            let back = i64::from(today.weekday().since(week_start));
            let first = shift(today, "day", offset * 7 - back)?;
            Some((first, shift(first, "day", 6)?))
        }
        "month" => {
            let first = shift(today.first_of_month(), "month", offset)?;
            Some((first, first.last_of_month()))
        }
        "quarter" => {
            let start_month = (today.month() - 1) / 3 * 3 + 1;
            let first = shift(Date::new(today.year(), start_month, 1).ok()?, "month", offset * 3)?;
            let last = shift(first, "month", 2)?.last_of_month();
            Some((first, last))
        }
        "year" => {
            let first = shift(today.first_of_year(), "year", offset)?;
            Some((first, first.last_of_year()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Zoned {
        // a Wednesday
        "2026-10-21T14:30:00[America/Los_Angeles]".parse().unwrap()
    }

    #[test]
    fn phrases_resolve_in_the_users_zone_and_week() {
        let now = now();
        let monday = Weekday::Monday;
        assert_eq!(resolve("now", &now, monday, false).unwrap(), "2026-10-21 14:30");
        assert_eq!(resolve("today", &now, monday, false).unwrap(), "2026-10-21 00:00");
        assert_eq!(resolve("today", &now, monday, true).unwrap(), "2026-10-21 23:59:59");
        assert_eq!(resolve("Yesterday", &now, monday, false).unwrap(), "2026-10-20 00:00");
        assert_eq!(resolve("3 days ago", &now, monday, false).unwrap(), "2026-10-18 00:00");
        assert_eq!(resolve("in 2 weeks", &now, monday, false).unwrap(), "2026-11-04 00:00");
        assert_eq!(
            resolve("start of last week", &now, monday, false).unwrap(),
            "2026-10-12 00:00"
        );
        assert_eq!(
            resolve("start of this week", &now, Weekday::Sunday, false).unwrap(),
            "2026-10-18 00:00"
        );
        assert_eq!(
            resolve("end of last month", &now, monday, false).unwrap(),
            "2026-09-30 23:59:59"
        );
        assert_eq!(
            resolve("last month", &now, monday, true).unwrap(),
            "2026-09-30 23:59:59"
        );
        assert_eq!(
            resolve("this quarter", &now, monday, false).unwrap(),
            "2026-10-01 00:00"
        );
        assert_eq!(
            resolve("end of next quarter", &now, monday, false).unwrap(),
            "2027-03-31 23:59:59"
        );
        assert_eq!(
            resolve("beginning of this year", &now, monday, false).unwrap(),
            "2026-01-01 00:00"
        );
        assert!(resolve("2026-10-01", &now, monday, false).is_none());
        assert!(resolve("acme corp", &now, monday, false).is_none());
        assert!(is_end_name("thruDate") && is_end_name("orderDate_thru") && !is_end_name("fromDate"));
        assert!(is_date_name("orderDate_from") && is_date_name("createdStamp") && !is_date_name("statusId"));
        assert_eq!(weekday_of("Sunday"), Weekday::Sunday);
    }
}
