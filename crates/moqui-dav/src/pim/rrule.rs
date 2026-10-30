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

//! RFC 5545 recurrence rules (§3.3.10) and recurrence sets (§3.8.5): RRULE parsing and expansion in local time of a
//! time zone, with EXDATE and RDATE.
//!
//! The rule is expanded on the wall clock of the series' time zone, so a weekly 10:00 meeting stays at 10:00 across
//! a daylight-saving change, and each local time becomes an instant as RFC 5545 §3.3.5 says: a time that occurs twice
//! is the first of them, a time in a gap is read with the offset before the gap (jiff's `compatible`). Each period of
//! FREQ (a year, month, week, day, hour, minute or second, INTERVAL apart) gives the days that pass every BYxxx day
//! part (BYMONTH, BYWEEKNO, BYYEARDAY, BYMONTHDAY, BYDAY with ordinals relative to the month or the year), times
//! BYHOUR, BYMINUTE and BYSECOND (from DTSTART where absent), then BYSETPOS over the period's sorted set. That is the
//! expand-or-limit table of §3.3.10: a part limits when the period holds one value of it and expands otherwise. As
//! there, a rule with no day part takes DTSTART's day (yearly its month and day, monthly its day, weekly its weekday).
//! DTSTART is always the first instance and counts for COUNT; UNTIL is inclusive; a date that does not exist (the
//! 30th of February, the 31st of a 30-day month) is no instance and is not counted.

use jiff::civil::{Date, DateTime, Time, Weekday};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

/// The most instances one expansion returns, and the most periods it looks at, so a rule that never matches or a
/// SECONDLY rule over years cannot run away.
pub(crate) const MAX_INSTANCES: usize = 100_000;
const MAX_PERIODS: usize = 2_000_000;
/// Without COUNT, UNTIL or the end of a range, the instances given.
pub(crate) const OPEN_ENDED_INSTANCES: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Freq {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Until {
    /// A DATE: the instances on or before that day, on the wall clock.
    Date(Date),
    /// A floating DATE-TIME, on the wall clock of the series' zone.
    Local(DateTime),
    /// A UTC DATE-TIME.
    Utc(Timestamp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rule {
    pub(crate) freq: Freq,
    pub(crate) interval: i64,
    pub(crate) count: Option<u32>,
    pub(crate) until: Option<Until>,
    pub(crate) by_second: Vec<i8>,
    pub(crate) by_minute: Vec<i8>,
    pub(crate) by_hour: Vec<i8>,
    /// The ordinal (0 for every one) and the weekday.
    pub(crate) by_day: Vec<(i16, Weekday)>,
    pub(crate) by_month_day: Vec<i8>,
    pub(crate) by_year_day: Vec<i16>,
    pub(crate) by_week_no: Vec<i8>,
    pub(crate) by_month: Vec<i8>,
    pub(crate) by_set_pos: Vec<i16>,
    pub(crate) wkst: Weekday,
}

fn weekday_of(text: &str) -> Option<Weekday> {
    Some(match text.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

pub(crate) fn weekday_code(day: Weekday) -> &'static str {
    match day {
        Weekday::Monday => "MO",
        Weekday::Tuesday => "TU",
        Weekday::Wednesday => "WE",
        Weekday::Thursday => "TH",
        Weekday::Friday => "FR",
        Weekday::Saturday => "SA",
        Weekday::Sunday => "SU",
    }
}

fn numbers<T: std::str::FromStr + Copy>(key: &str, value: &str, valid: impl Fn(i64) -> bool) -> Result<Vec<T>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let number: i64 = part
                .trim_start_matches('+')
                .parse()
                .map_err(|_| format!("{key} has {part}, not a number"))?;
            if !valid(number) {
                return Err(format!("{key} has {part}, out of range"));
            }
            part.trim_start_matches('+')
                .parse::<T>()
                .map_err(|_| format!("{key} has {part}, out of range"))
        })
        .collect()
}

/// An RFC 5545 DATE (`20261020`), floating DATE-TIME (`20261020T090000`) or UTC DATE-TIME (`...Z`); dashes and colons
/// of the ISO forms are tolerated.
pub(crate) fn parse_date_value(text: &str) -> Option<Until> {
    let compact: String = text.trim().chars().filter(|ch| *ch != '-' && *ch != ':').collect();
    let compact = compact.to_ascii_uppercase();
    let (body, utc) = match compact.strip_suffix('Z') {
        Some(body) => (body, true),
        None => (compact.as_str(), false),
    };
    let digits = |range: std::ops::Range<usize>| body.get(range).and_then(|part| part.parse::<i64>().ok());
    if body.len() == 8 && body.bytes().all(|byte| byte.is_ascii_digit()) {
        let date = Date::new(digits(0..4)? as i16, digits(4..6)? as i8, digits(6..8)? as i8).ok()?;
        return Some(Until::Date(date));
    }
    if body.len() >= 15 && body.as_bytes()[8] == b'T' {
        // fractional seconds are not RFC 5545, but some writers add them
        let date = Date::new(digits(0..4)? as i16, digits(4..6)? as i8, digits(6..8)? as i8).ok()?;
        let second = digits(13..15)?.min(59);
        let time = Time::new(digits(9..11)? as i8, digits(11..13)? as i8, second as i8, 0).ok()?;
        let local = DateTime::from_parts(date, time);
        if utc {
            return local
                .to_zoned(TimeZone::UTC)
                .ok()
                .map(|zoned| Until::Utc(zoned.timestamp()));
        }
        return Some(Until::Local(local));
    }
    None
}

impl Rule {
    /// An RRULE value, with or without the `RRULE:` name. Parts this does not know (X- names, RSCALE) are left out.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let text = if text.len() >= 6 && text[..6].eq_ignore_ascii_case("RRULE:") {
            &text[6..]
        } else {
            text
        };
        let mut freq = None;
        let mut rule = Rule {
            freq: Freq::Daily,
            interval: 1,
            count: None,
            until: None,
            by_second: Vec::new(),
            by_minute: Vec::new(),
            by_hour: Vec::new(),
            by_day: Vec::new(),
            by_month_day: Vec::new(),
            by_year_day: Vec::new(),
            by_week_no: Vec::new(),
            by_month: Vec::new(),
            by_set_pos: Vec::new(),
            wkst: Weekday::Monday,
        };
        for part in text.split(';').map(str::trim).filter(|part| !part.is_empty()) {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| format!("{text} is not an RRULE ({part} is not KEY=VALUE)"))?;
            let key = key.trim().to_ascii_uppercase();
            let value = value.trim();
            match key.as_str() {
                "FREQ" => {
                    freq = Some(match value.to_ascii_uppercase().as_str() {
                        "SECONDLY" => Freq::Secondly,
                        "MINUTELY" => Freq::Minutely,
                        "HOURLY" => Freq::Hourly,
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return Err(format!("FREQ {value} is not a frequency")),
                    })
                }
                "INTERVAL" => {
                    rule.interval = value
                        .parse::<i64>()
                        .ok()
                        .filter(|interval| *interval >= 1)
                        .ok_or_else(|| format!("INTERVAL {value} is not a positive number"))?
                }
                "COUNT" => {
                    rule.count = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| format!("COUNT {value} is not a number"))?,
                    )
                }
                "UNTIL" => {
                    rule.until = Some(parse_date_value(value).ok_or_else(|| format!("UNTIL {value} is not a date"))?)
                }
                "BYSECOND" => rule.by_second = numbers(&key, value, |n| (0..=60).contains(&n))?,
                "BYMINUTE" => rule.by_minute = numbers(&key, value, |n| (0..=59).contains(&n))?,
                "BYHOUR" => rule.by_hour = numbers(&key, value, |n| (0..=23).contains(&n))?,
                "BYMONTHDAY" => rule.by_month_day = numbers(&key, value, |n| n != 0 && (-31..=31).contains(&n))?,
                "BYYEARDAY" => rule.by_year_day = numbers(&key, value, |n| n != 0 && (-366..=366).contains(&n))?,
                "BYWEEKNO" => rule.by_week_no = numbers(&key, value, |n| n != 0 && (-53..=53).contains(&n))?,
                "BYMONTH" => rule.by_month = numbers(&key, value, |n| (1..=12).contains(&n))?,
                "BYSETPOS" => rule.by_set_pos = numbers(&key, value, |n| n != 0 && (-366..=366).contains(&n))?,
                "BYDAY" => {
                    for item in value.split(',').map(str::trim).filter(|item| !item.is_empty()) {
                        if item.len() < 2 {
                            return Err(format!("BYDAY has {item}"));
                        }
                        let (ordinal, day) = item.split_at(item.len() - 2);
                        let day = weekday_of(day).ok_or_else(|| format!("BYDAY has {item}"))?;
                        let ordinal = if ordinal.is_empty() {
                            0
                        } else {
                            ordinal
                                .trim_start_matches('+')
                                .parse::<i16>()
                                .ok()
                                .filter(|n| *n != 0 && (-53..=53).contains(n))
                                .ok_or_else(|| format!("BYDAY has {item}"))?
                        };
                        rule.by_day.push((ordinal, day));
                    }
                }
                "WKST" => rule.wkst = weekday_of(value).ok_or_else(|| format!("WKST {value} is not a weekday"))?,
                _ => {}
            }
        }
        rule.freq = freq.ok_or_else(|| format!("{text} is not an RRULE (it needs FREQ)"))?;
        // a leap second is the last second of its minute
        for second in &mut rule.by_second {
            *second = (*second).min(59);
        }
        rule.by_second.sort_unstable();
        rule.by_second.dedup();
        Ok(rule)
    }
}

/// Days from 1970-01-01.
pub(crate) fn epoch_day(date: Date) -> i64 {
    let (year, month, day) = (i64::from(date.year()), i64::from(date.month()), i64::from(date.day()));
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn add_days(date: Date, days: i64) -> Option<Date> {
    date.checked_add(days.days()).ok()
}

fn offset(day: Weekday) -> i64 {
    i64::from(day.to_monday_zero_offset())
}

/// The first day of the week that holds `date`, the week starting on `wkst`.
fn week_start(date: Date, wkst: Weekday) -> Option<Date> {
    add_days(date, -(offset(date.weekday()) - offset(wkst)).rem_euclid(7))
}

/// The first day of week 1 of `year`: the first week, starting on `wkst`, with at least four days in the year.
fn week_one(year: i16, wkst: Weekday) -> Option<Date> {
    let jan1 = Date::new(year, 1, 1).ok()?;
    let into = (offset(jan1.weekday()) - offset(wkst)).rem_euclid(7);
    if into <= 3 {
        add_days(jan1, -into)
    } else {
        add_days(jan1, 7 - into)
    }
}

/// The week number of `date` and the number of weeks of the year that week belongs to.
fn week_number(date: Date, wkst: Weekday) -> Option<(i64, i64)> {
    let year = date.year();
    let this = week_one(year, wkst)?;
    let (start, week_year) = if date < this {
        (week_one(year - 1, wkst)?, year - 1)
    } else {
        let next = week_one(year + 1, wkst)?;
        if date >= next {
            (next, year + 1)
        } else {
            (this, year)
        }
    };
    let week = (epoch_day(date) - epoch_day(start)) / 7 + 1;
    let weeks = (epoch_day(week_one(week_year + 1, wkst)?) - epoch_day(week_one(week_year, wkst)?)) / 7;
    Some((week, weeks))
}

/// One expansion: the rule with DTSTART's defaults filled in, over the series' wall clock.
struct Plan<'a> {
    rule: &'a Rule,
    start: DateTime,
    by_month: Vec<i8>,
    by_month_day: Vec<i8>,
    /// BYDAY without an ordinal, and those whose ordinal does not count for this frequency.
    every_weekday: Vec<Weekday>,
    nth_weekday: Vec<(i16, Weekday)>,
    /// The times of a day for DAILY and longer, sorted.
    times: Vec<Time>,
}

impl<'a> Plan<'a> {
    fn new(rule: &'a Rule, start: DateTime) -> Self {
        let mut by_month = rule.by_month.clone();
        let mut by_month_day = rule.by_month_day.clone();
        let mut by_day = rule.by_day.clone();
        if rule.by_week_no.is_empty() && rule.by_year_day.is_empty() && by_month_day.is_empty() && by_day.is_empty() {
            match rule.freq {
                Freq::Yearly => {
                    if by_month.is_empty() {
                        by_month = vec![start.month()];
                    }
                    by_month_day = vec![start.day()];
                }
                Freq::Monthly => by_month_day = vec![start.day()],
                Freq::Weekly => by_day = vec![(0, start.weekday())],
                _ => {}
            }
        }
        // an ordinal counts within the month (MONTHLY, or YEARLY with BYMONTH) or the year (YEARLY), not with BYWEEKNO
        let ordinals = matches!(rule.freq, Freq::Monthly) || (rule.freq == Freq::Yearly && rule.by_week_no.is_empty());
        let mut every_weekday = Vec::new();
        let mut nth_weekday = Vec::new();
        for (ordinal, day) in by_day {
            if ordinal == 0 || !ordinals {
                every_weekday.push(day);
            } else {
                nth_weekday.push((ordinal, day));
            }
        }
        let hours = if !rule.by_hour.is_empty() {
            rule.by_hour.clone()
        } else {
            vec![start.hour()]
        };
        let minutes = if !rule.by_minute.is_empty() {
            rule.by_minute.clone()
        } else {
            vec![start.minute()]
        };
        let seconds = if !rule.by_second.is_empty() {
            rule.by_second.clone()
        } else {
            vec![start.second()]
        };
        let mut times = Vec::new();
        for hour in &hours {
            for minute in &minutes {
                for second in &seconds {
                    if let Ok(time) = Time::new(*hour, *minute, *second, start.subsec_nanosecond()) {
                        times.push(time);
                    }
                }
            }
        }
        times.sort_unstable();
        times.dedup();
        Plan {
            rule,
            start,
            by_month,
            by_month_day,
            every_weekday,
            nth_weekday,
            times,
        }
    }

    fn day_matches(&self, date: Date) -> bool {
        let rule = self.rule;
        if !self.by_month.is_empty() && !self.by_month.contains(&date.month()) {
            return false;
        }
        if !rule.by_week_no.is_empty() && rule.freq == Freq::Yearly {
            let Some((week, weeks)) = week_number(date, rule.wkst) else {
                return false;
            };
            if !rule.by_week_no.iter().any(|n| {
                let n = i64::from(*n);
                n == week || (n < 0 && weeks + 1 + n == week)
            }) {
                return false;
            }
        }
        if !rule.by_year_day.is_empty() {
            let day = i64::from(date.day_of_year());
            let days = i64::from(date.days_in_year());
            if !rule.by_year_day.iter().any(|n| {
                let n = i64::from(*n);
                n == day || (n < 0 && days + 1 + n == day)
            }) {
                return false;
            }
        }
        if !self.by_month_day.is_empty() {
            let day = i64::from(date.day());
            let days = i64::from(date.days_in_month());
            if !self.by_month_day.iter().any(|n| {
                let n = i64::from(*n);
                n == day || (n < 0 && days + 1 + n == day)
            }) {
                return false;
            }
        }
        if !self.every_weekday.is_empty() || !self.nth_weekday.is_empty() {
            let weekday = date.weekday();
            if self.every_weekday.contains(&weekday) {
                return true;
            }
            // the place of this weekday in its month or year, from the start and from the end
            let in_month = rule.freq == Freq::Monthly || !rule.by_month.is_empty();
            let (index, length) = if in_month {
                (i64::from(date.day()), i64::from(date.days_in_month()))
            } else {
                (i64::from(date.day_of_year()), i64::from(date.days_in_year()))
            };
            let from_start = (index - 1) / 7 + 1;
            let from_end = -((length - index) / 7 + 1);
            return self.nth_weekday.iter().any(|(ordinal, day)| {
                *day == weekday && (i64::from(*ordinal) == from_start || i64::from(*ordinal) == from_end)
            });
        }
        true
    }

    /// The period `index` FREQ units from DTSTART's own period: its datetimes, sorted, BYSETPOS applied. None past
    /// the dates jiff holds.
    fn period(&self, index: i64) -> Option<Vec<DateTime>> {
        let rule = self.rule;
        let start = self.start;
        let mut out = Vec::new();
        let add_day = |date: Date, out: &mut Vec<DateTime>| {
            if self.day_matches(date) {
                for time in &self.times {
                    out.push(DateTime::from_parts(date, *time));
                }
            }
        };
        match rule.freq {
            Freq::Yearly => {
                let year = i64::from(start.year()) + index;
                let first = Date::new(i16::try_from(year).ok()?, 1, 1).ok()?;
                // only the months that can match
                for month in 1..=12i8 {
                    if !self.by_month.is_empty() && !self.by_month.contains(&month) {
                        continue;
                    }
                    let first_of_month = Date::new(first.year(), month, 1).ok()?;
                    for day in 0..i64::from(first_of_month.days_in_month()) {
                        add_day(add_days(first_of_month, day)?, &mut out);
                    }
                }
            }
            Freq::Monthly => {
                let first = start.date().first_of_month().checked_add(index.months()).ok()?;
                for day in 0..i64::from(first.days_in_month()) {
                    add_day(add_days(first, day)?, &mut out);
                }
            }
            Freq::Weekly => {
                let first = add_days(week_start(start.date(), rule.wkst)?, index * 7)?;
                for day in 0..7 {
                    add_day(add_days(first, day)?, &mut out);
                }
            }
            Freq::Daily => add_day(add_days(start.date(), index)?, &mut out),
            Freq::Hourly | Freq::Minutely | Freq::Secondly => {
                let base = match rule.freq {
                    Freq::Hourly => start.with().minute(0).second(0).subsec_nanosecond(0).build().ok()?,
                    Freq::Minutely => start.with().second(0).subsec_nanosecond(0).build().ok()?,
                    _ => start.with().subsec_nanosecond(0).build().ok()?,
                };
                let at = match rule.freq {
                    Freq::Hourly => base.checked_add(index.hours()).ok()?,
                    Freq::Minutely => base.checked_add(index.minutes()).ok()?,
                    _ => base.checked_add(index.seconds()).ok()?,
                };
                if !self.day_matches(at.date()) {
                    return Some(out);
                }
                if !rule.by_hour.is_empty() && !rule.by_hour.contains(&at.hour()) {
                    return Some(out);
                }
                let minutes = if rule.freq == Freq::Hourly {
                    if rule.by_minute.is_empty() {
                        vec![start.minute()]
                    } else {
                        rule.by_minute.clone()
                    }
                } else {
                    if !rule.by_minute.is_empty() && !rule.by_minute.contains(&at.minute()) {
                        return Some(out);
                    }
                    vec![at.minute()]
                };
                let seconds = if rule.freq == Freq::Secondly {
                    if !rule.by_second.is_empty() && !rule.by_second.contains(&at.second()) {
                        return Some(out);
                    }
                    vec![at.second()]
                } else if rule.by_second.is_empty() {
                    vec![start.second()]
                } else {
                    rule.by_second.clone()
                };
                for minute in &minutes {
                    for second in &seconds {
                        if let Ok(time) = Time::new(at.hour(), *minute, *second, start.subsec_nanosecond()) {
                            out.push(DateTime::from_parts(at.date(), time));
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        if !rule.by_set_pos.is_empty() && !out.is_empty() {
            let length = out.len() as i64;
            let mut picked: Vec<DateTime> = rule
                .by_set_pos
                .iter()
                .filter_map(|position| {
                    let position = i64::from(*position);
                    let at = if position > 0 { position - 1 } else { length + position };
                    usize::try_from(at).ok().and_then(|at| out.get(at).copied())
                })
                .collect();
            picked.sort_unstable();
            picked.dedup();
            out = picked;
        }
        Some(out)
    }

    /// The periods from DTSTART's to the one that holds `target`, a whole number of intervals.
    fn periods_to(&self, target: DateTime) -> Option<i64> {
        let start = self.start;
        let count = match self.rule.freq {
            Freq::Yearly => i64::from(target.year()) - i64::from(start.year()),
            Freq::Monthly => {
                (i64::from(target.year()) - i64::from(start.year())) * 12 + i64::from(target.month())
                    - i64::from(start.month())
            }
            Freq::Weekly => {
                (epoch_day(week_start(target.date(), self.rule.wkst)?)
                    - epoch_day(week_start(start.date(), self.rule.wkst)?))
                    / 7
            }
            Freq::Daily => epoch_day(target.date()) - epoch_day(start.date()),
            Freq::Hourly | Freq::Minutely | Freq::Secondly => {
                let seconds = |at: DateTime| {
                    epoch_day(at.date()) * 86_400
                        + i64::from(at.hour()) * 3600
                        + i64::from(at.minute()) * 60
                        + i64::from(at.second())
                };
                let unit = match self.rule.freq {
                    Freq::Hourly => 3600,
                    Freq::Minutely => 60,
                    _ => 1,
                };
                (seconds(target) - seconds(start)).div_euclid(unit)
            }
        };
        Some(count)
    }
}

/// A local time on the series' wall clock as an instant (RFC 5545 §3.3.5).
pub(crate) fn instant(tz: &TimeZone, local: DateTime) -> Option<Timestamp> {
    tz.to_ambiguous_zoned(local)
        .compatible()
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// The instances of `rule` from `dtstart`, those that start from `range_start` to `range_end` (both included),
/// ascending, at most `limit` of them. DTSTART is the first instance.
pub(crate) fn expand(
    rule: &Rule,
    dtstart: Timestamp,
    tz: &TimeZone,
    range_start: Option<Timestamp>,
    range_end: Option<Timestamp>,
    limit: usize,
) -> Vec<Timestamp> {
    let start = dtstart.to_zoned(tz.clone()).datetime();
    let plan = Plan::new(rule, start);
    let in_range = |at: Timestamp| range_start.is_none_or(|from| at >= from) && range_end.is_none_or(|to| at <= to);
    let mut out = Vec::new();
    if in_range(dtstart) {
        out.push(dtstart);
    }
    let mut count: u32 = 1;
    if rule.count.is_some_and(|max| count >= max) {
        return out;
    }
    let until_passed = |local: DateTime, at: Timestamp| match rule.until {
        Some(Until::Date(date)) => local.date() > date,
        Some(Until::Local(until)) => local > until,
        Some(Until::Utc(until)) => at > until,
        None => false,
    };
    // without COUNT nothing before the range needs counting: start a few intervals before it
    let mut index = 0;
    if rule.count.is_none() {
        if let Some(from) = range_start.filter(|from| *from > dtstart) {
            let target = from.to_zoned(tz.clone()).datetime();
            if let Some(periods) = plan.periods_to(target) {
                let whole = periods.div_euclid(rule.interval) - 1;
                if whole > 0 {
                    index = whole * rule.interval;
                }
            }
        }
    }
    let end_local = range_end.map(|to| to.to_zoned(tz.clone()).datetime());
    let mut empty_periods = 0;
    for _ in 0..MAX_PERIODS {
        let Some(candidates) = plan.period(index) else { break };
        index += rule.interval;
        if candidates.is_empty() {
            empty_periods += 1;
            // a rule that gives nothing for a long time (BYMONTHDAY=30;BYMONTH=2) has ended
            if empty_periods > 50_000 {
                break;
            }
            if let (Some(end), Some(first)) = (end_local, plan.period_first_day(index)) {
                if first > end.date().checked_add(2.days()).unwrap_or(end.date()) {
                    break;
                }
            }
            continue;
        }
        empty_periods = 0;
        for local in candidates {
            if local <= start {
                continue;
            }
            let Some(at) = instant(tz, local) else { continue };
            if until_passed(local, at) {
                return finish(out);
            }
            count += 1;
            if let Some(to) = range_end {
                // a day past the end, so a fold of the wall clock cannot hide a later instance in range
                if at > to && local.date() > end_local.map_or(local.date(), |end| end.date()) {
                    return finish(out);
                }
            }
            if in_range(at) {
                out.push(at);
                if out.len() >= limit {
                    return finish(out);
                }
            }
            if rule.count.is_some_and(|max| count >= max) {
                return finish(out);
            }
        }
        if range_end.is_none() && rule.count.is_none() && rule.until.is_none() && out.len() >= OPEN_ENDED_INSTANCES {
            return finish(out);
        }
    }
    finish(out)
}

impl Plan<'_> {
    /// The first day of the period `index`, to stop at the end of a range when periods come out empty.
    fn period_first_day(&self, index: i64) -> Option<Date> {
        let start = self.start.date();
        match self.rule.freq {
            Freq::Yearly => Date::new(i16::try_from(i64::from(start.year()) + index).ok()?, 1, 1).ok(),
            Freq::Monthly => start.first_of_month().checked_add(index.months()).ok(),
            Freq::Weekly => add_days(week_start(start, self.rule.wkst)?, index * 7),
            Freq::Daily => add_days(start, index),
            Freq::Hourly => add_days(start, index.div_euclid(24)),
            Freq::Minutely => add_days(start, index.div_euclid(1440)),
            Freq::Secondly => add_days(start, index.div_euclid(86_400)),
        }
    }
}

fn finish(mut out: Vec<Timestamp>) -> Vec<Timestamp> {
    out.sort_unstable();
    out.dedup();
    out
}

/// An EXDATE or RDATE value as stored: `20261020` (a date), `20261020T090000` (the series' wall clock),
/// `20261020T140000Z` (UTC), `TZID=Europe/Paris:20261020T090000`, a PERIOD's start, or epoch millis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SetDate {
    Date(Date),
    Instant(Timestamp),
}

pub(crate) fn parse_set_date(text: &str, tz: &TimeZone, resolve: &dyn Fn(&str) -> Option<TimeZone>) -> Option<SetDate> {
    let mut text = text.trim();
    let mut zone = tz.clone();
    if text.len() > 5 && text[..5].eq_ignore_ascii_case("TZID=") {
        let (name, value) = text[5..].rsplit_once(':')?;
        zone = resolve(name.trim().trim_matches('"'))?;
        text = value;
    }
    // a PERIOD is start/end or start/duration
    let text = text.split('/').next().unwrap_or(text).trim();
    if text.len() > 9 && text.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-') && !text.contains('T') {
        if let Ok(millis) = text.parse::<i64>() {
            return Timestamp::from_millisecond(millis).ok().map(SetDate::Instant);
        }
    }
    match parse_date_value(text)? {
        Until::Date(date) => Some(SetDate::Date(date)),
        Until::Local(local) => instant(&zone, local).map(SetDate::Instant),
        Until::Utc(at) => Some(SetDate::Instant(at)),
    }
}

/// A recurrence set (RFC 5545 §3.8.5): the rule's instances, or DTSTART alone without one, plus the RDATEs, less
/// the EXDATEs, within the range, ascending. A DATE EXDATE removes the instances on that day of the wall clock; a
/// DATE RDATE adds that day at DTSTART's time. A value that cannot be read is an error.
#[allow(clippy::too_many_arguments)]
pub(crate) fn expand_set(
    rrule: Option<&str>,
    dtstart: Timestamp,
    tz: &TimeZone,
    exdates: &[String],
    rdates: &[String],
    range_start: Option<Timestamp>,
    range_end: Option<Timestamp>,
    resolve: &dyn Fn(&str) -> Option<TimeZone>,
) -> Result<Vec<Timestamp>, String> {
    let in_range = |at: Timestamp| range_start.is_none_or(|from| at >= from) && range_end.is_none_or(|to| at <= to);
    let mut out = match rrule.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => expand(&Rule::parse(text)?, dtstart, tz, range_start, range_end, MAX_INSTANCES),
        None => {
            if in_range(dtstart) {
                vec![dtstart]
            } else {
                Vec::new()
            }
        }
    };
    let start_time = dtstart.to_zoned(tz.clone()).time();
    for text in rdates
        .iter()
        .flat_map(|text| text.split(','))
        .filter(|text| !text.trim().is_empty())
    {
        let at = match parse_set_date(text, tz, resolve).ok_or_else(|| format!("RDATE {text} is not a date"))? {
            SetDate::Instant(at) => Some(at),
            SetDate::Date(date) => instant(tz, DateTime::from_parts(date, start_time)),
        };
        if let Some(at) = at.filter(|at| in_range(*at)) {
            out.push(at);
        }
    }
    let mut excluded_days = Vec::new();
    let mut excluded = Vec::new();
    for text in exdates
        .iter()
        .flat_map(|text| text.split(','))
        .filter(|text| !text.trim().is_empty())
    {
        match parse_set_date(text, tz, resolve).ok_or_else(|| format!("EXDATE {text} is not a date"))? {
            SetDate::Instant(at) => excluded.push(at),
            SetDate::Date(date) => excluded_days.push(date),
        }
    }
    out.retain(|at| {
        !excluded.contains(at) && (excluded_days.is_empty() || !excluded_days.contains(&at.to_zoned(tz.clone()).date()))
    });
    Ok(finish(out))
}

#[cfg(test)]
#[path = "rrule_tests.rs"]
mod tests;
