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

//! Quartz cron expressions, as Moqui Java's `ScheduledJobRunner` reads them (cron-utils' QUARTZ definition): the
//! parse, the times an expression comes due, and an English description.
//!
//! An expression has six or seven fields: second, minute, hour, day of month, month, day of week and an optional year.
//! Months are 1 to 12 or `JAN` to `DEC`; days of the week are 1 (Sunday) to 7 (Saturday) or `SUN` to `SAT`; years are
//! 1970 to 2099. Case does not matter.
//!
//! - `*` is every value, `a-b` a range (one whose end is before its start wraps around: `FRI-MON`, `22-2`), `a/n` or
//!   `a-b/n` every nth value from `a`, `*/n` (or `/n`) every nth from the field's first. Items join with `,`.
//! - Day of month: `L` the last day, `L-n` n days before it, `LW` the last weekday (Monday to Friday), `nW` the weekday
//!   nearest day n, in the same month.
//! - Day of week: `L` alone is Saturday, `nL` the last day n of the month, `n#k` the kth day n of the month (k 1 to 5).
//! - One of the two day fields is `?` (or `*`), and the other decides the day. Quartz requires the `?`; `*` is taken
//!   too. Both given is refused, as Quartz refuses it.
//!
//! The times are wall-clock times in the time zone of the moment given ([`Cron::next_after`],
//! [`Cron::last_at_or_before`]). A time a clock change skips comes due as far past the change as it was past the
//! skipped start (02:30 is 03:30 when 02:00 jumps to 03:00), as Java's `Calendar` resolves it. A time in the hour a
//! change repeats comes due once, at its first occurrence.

use std::fmt;
use std::str::FromStr;

use jiff::civil::{Date, DateTime, Time};
use jiff::{ToSpan as _, Zoned};

mod describe;

/// The years a Quartz expression can name.
const FIRST_YEAR: i16 = 1970;
const LAST_YEAR: i16 = 2099;

/// How many years a search looks ahead or back. The Gregorian calendar repeats every 400 years, so a day that does not
/// come within them never comes.
const SEARCH_YEARS: i16 = 400;

/// A cron expression that does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronError(String);

impl fmt::Display for CronError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CronError {}

/// A parsed Quartz cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    seconds: Field,
    minutes: Field,
    hours: Field,
    days: DaysOfMonth,
    months: Field,
    weekdays: DaysOfWeek,
    years: Years,
}

impl FromStr for Cron {
    type Err = CronError;

    fn from_str(text: &str) -> Result<Self, CronError> {
        Cron::parse(text)
    }
}

impl Cron {
    /// Parses a Quartz expression.
    ///
    /// # Errors
    ///
    /// A [`CronError`] naming the field and what is wrong with it.
    pub fn parse(text: &str) -> Result<Cron, CronError> {
        let upper = text.trim().to_ascii_uppercase();
        let fields: Vec<&str> = upper.split_whitespace().collect();
        if !(6..=7).contains(&fields.len()) {
            return Err(CronError(format!(
                "a cron expression has 6 or 7 fields (second, minute, hour, day of month, month, day of week and an \
                 optional year), not {}",
                fields.len()
            )));
        }
        for (index, field) in fields.iter().enumerate() {
            if field.contains('?') && index != 3 && index != 5 {
                return Err(CronError(format!(
                    "the {} field: ? is only for the day of month or the day of week",
                    KINDS[index].name
                )));
            }
        }
        let cron = Cron {
            seconds: Field::parse(fields[0], &SECOND)?,
            minutes: Field::parse(fields[1], &MINUTE)?,
            hours: Field::parse(fields[2], &HOUR)?,
            days: DaysOfMonth::parse(fields[3])?,
            months: Field::parse(fields[4], &MONTH)?,
            weekdays: DaysOfWeek::parse(fields[5])?,
            years: fields.get(6).map_or(Ok(Years::Every), |field| Years::parse(field))?,
        };
        if !cron.days.any && !cron.weekdays.any {
            return Err(CronError(
                "give a day of month or a day of week, not both: put ? in the other".to_string(),
            ));
        }
        Ok(cron)
    }

    /// The first time strictly after `after` that the expression is due, in `after`'s time zone. `None` when it is
    /// never due again (its years are past, or a date such as 31 February that never comes).
    pub fn next_after(&self, after: &Zoned) -> Option<Zoned> {
        let zone = after.time_zone();
        let mut from = whole_second(after.datetime())?.checked_add(1.second()).ok()?;
        loop {
            let due = self.first_at_or_after(from)?;
            let zoned = zone.to_ambiguous_zoned(due).compatible().ok()?;
            if zoned.timestamp() > after.timestamp() {
                return Some(zoned);
            }
            // The repeated hour of a clock change, at its second occurrence: the first has already come due.
            from = due.checked_add(1.second()).ok()?;
        }
    }

    /// The latest time at or before `at` that the expression came due, in `at`'s time zone. `None` when it never did.
    pub fn last_at_or_before(&self, at: &Zoned) -> Option<Zoned> {
        let zone = at.time_zone();
        let mut from = whole_second(at.datetime())?;
        loop {
            let due = self.last_at_or_before_civil(from)?;
            let zoned = zone.to_ambiguous_zoned(due).compatible().ok()?;
            if zoned.timestamp() <= at.timestamp() {
                return Some(zoned);
            }
            // A time a clock change skipped, which comes due after the change: not yet.
            from = due.checked_sub(1.second()).ok()?;
        }
    }

    /// An English description: "At 02:00.", "Every 15 minutes.", "At 06:00, on Monday through Friday.".
    pub fn describe(&self) -> String {
        describe::describe(self)
    }

    /// Whether `date` is a day the expression runs on, by its day-of-month and day-of-week fields.
    fn day_matches(&self, date: Date) -> bool {
        if self.days.any && self.weekdays.any {
            return true;
        }
        // A day of the month is 1 to 31, so these do not truncate.
        let day = date.day().unsigned_abs();
        let last = date.days_in_month().unsigned_abs();
        if self.days.any {
            self.weekdays.matches(date, day, last)
        } else {
            self.days.matches(date, day, last)
        }
    }

    /// The first due wall-clock time at or after `from`.
    fn first_at_or_after(&self, from: DateTime) -> Option<DateTime> {
        let horizon = from.year().saturating_add(SEARCH_YEARS);
        let mut date = from.date();
        let mut floor = Some(from.time());
        loop {
            let year = date.year();
            if year > horizon {
                return None;
            }
            let due_year = self.years.first_at_or_after(year)?;
            if due_year != year {
                date = Date::new(due_year, 1, 1).ok()?;
                floor = None;
                continue;
            }
            let month = date.month().unsigned_abs();
            if !self.months.contains(month) {
                date = match self.months.first_at_or_after(month + 1) {
                    Some(next) => Date::new(year, i8::try_from(next).ok()?, 1).ok()?,
                    None => Date::new(year.checked_add(1)?, 1, 1).ok()?,
                };
                floor = None;
                continue;
            }
            if self.day_matches(date) {
                if let Some(time) = self.first_time_at_or_after(floor.unwrap_or(Time::midnight())) {
                    return Some(date.to_datetime(time));
                }
            }
            date = date.tomorrow().ok()?;
            floor = None;
        }
    }

    /// The last due wall-clock time at or before `from`.
    fn last_at_or_before_civil(&self, from: DateTime) -> Option<DateTime> {
        let horizon = from.year().saturating_sub(SEARCH_YEARS);
        let mut date = from.date();
        let mut ceiling = Some(from.time());
        loop {
            let year = date.year();
            if year < horizon {
                return None;
            }
            let due_year = self.years.last_at_or_before(year)?;
            if due_year != year {
                date = Date::new(due_year, 12, 31).ok()?;
                ceiling = None;
                continue;
            }
            let month = date.month().unsigned_abs();
            if !self.months.contains(month) {
                date = match self.months.last_at_or_before(month - 1) {
                    Some(previous) => Date::new(year, i8::try_from(previous).ok()?, 1).ok()?.last_of_month(),
                    None => Date::new(year.checked_sub(1)?, 12, 31).ok()?,
                };
                ceiling = None;
                continue;
            }
            if self.day_matches(date) {
                let last_second = Time::new(23, 59, 59, 0).ok()?;
                if let Some(time) = self.last_time_at_or_before(ceiling.unwrap_or(last_second)) {
                    return Some(date.to_datetime(time));
                }
            }
            date = date.yesterday().ok()?;
            ceiling = None;
        }
    }

    /// The first due time of a day at or after `from`.
    fn first_time_at_or_after(&self, from: Time) -> Option<Time> {
        let (hour, minute, second) = hms(from);
        let mut h = self.hours.first_at_or_after(hour)?;
        loop {
            let mut m = self.minutes.first_at_or_after(if h == hour { minute } else { 0 });
            while let Some(min) = m {
                let floor = if h == hour && min == minute { second } else { 0 };
                if let Some(s) = self.seconds.first_at_or_after(floor) {
                    return time(h, min, s);
                }
                m = self.minutes.first_at_or_after(min + 1);
            }
            h = self.hours.first_at_or_after(h + 1)?;
        }
    }

    /// The last due time of a day at or before `from`.
    fn last_time_at_or_before(&self, from: Time) -> Option<Time> {
        let (hour, minute, second) = hms(from);
        let mut h = self.hours.last_at_or_before(hour)?;
        loop {
            let mut m = self.minutes.last_at_or_before(if h == hour { minute } else { 59 });
            while let Some(min) = m {
                let ceiling = if h == hour && min == minute { second } else { 59 };
                if let Some(s) = self.seconds.last_at_or_before(ceiling) {
                    return time(h, min, s);
                }
                m = min
                    .checked_sub(1)
                    .and_then(|before| self.minutes.last_at_or_before(before));
            }
            h = self.hours.last_at_or_before(h.checked_sub(1)?)?;
        }
    }
}

/// `at` without its fraction of a second.
fn whole_second(at: DateTime) -> Option<DateTime> {
    let (hour, minute, second) = hms(at.time());
    Some(at.date().to_datetime(time(hour, minute, second)?))
}

fn hms(at: Time) -> (u8, u8, u8) {
    (
        at.hour().unsigned_abs(),
        at.minute().unsigned_abs(),
        at.second().unsigned_abs(),
    )
}

fn time(hour: u8, minute: u8, second: u8) -> Option<Time> {
    Time::new(
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(second).ok()?,
        0,
    )
    .ok()
}

/// What a field holds and what its values are called.
struct Kind {
    name: &'static str,
    min: u16,
    max: u16,
    /// Names for the values from `min` on (`JAN` is 1, `SUN` is 1).
    names: &'static [&'static str],
    /// Whether a range may run past the end of the field and on from its start.
    wraps: bool,
}

const SECOND: Kind = Kind {
    name: "seconds",
    min: 0,
    max: 59,
    names: &[],
    wraps: true,
};
const MINUTE: Kind = Kind {
    name: "minutes",
    min: 0,
    max: 59,
    names: &[],
    wraps: true,
};
const HOUR: Kind = Kind {
    name: "hours",
    min: 0,
    max: 23,
    names: &[],
    wraps: true,
};
const DAY: Kind = Kind {
    name: "day of month",
    min: 1,
    max: 31,
    names: &[],
    wraps: true,
};
const MONTH: Kind = Kind {
    name: "month",
    min: 1,
    max: 12,
    names: &[
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ],
    wraps: true,
};
const WEEKDAY: Kind = Kind {
    name: "day of week",
    min: 1,
    max: 7,
    names: &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
    wraps: true,
};
const YEAR: Kind = Kind {
    name: "year",
    min: FIRST_YEAR as u16,
    max: LAST_YEAR as u16,
    names: &[],
    wraps: false,
};
const KINDS: [&Kind; 7] = [&SECOND, &MINUTE, &HOUR, &DAY, &MONTH, &WEEKDAY, &YEAR];

impl Kind {
    fn error(&self, message: impl fmt::Display) -> CronError {
        CronError(format!("the {} field: {message}", self.name))
    }

    /// A number or a name of this field.
    fn value(&self, text: &str) -> Result<u16, CronError> {
        let value = match self.names.iter().position(|name| *name == text) {
            // There are at most 12 names.
            Some(index) => self.min + index as u16,
            None => text
                .parse()
                .map_err(|_| self.error(format!("{text:?} is not a number or a name")))?,
        };
        if !(self.min..=self.max).contains(&value) {
            return Err(self.error(format!("{value} is not in {}-{}", self.min, self.max)));
        }
        Ok(value)
    }

    /// The values of one item: a value, a range, `*`, each with an optional step.
    fn expand(&self, item: &str) -> Result<Vec<u16>, CronError> {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => {
                let step: u16 = step
                    .parse()
                    .map_err(|_| self.error(format!("{step:?} is not a step")))?;
                if step == 0 || step > self.max {
                    return Err(self.error(format!("the step {step} is not in 1-{}", self.max)));
                }
                (range, Some(step))
            }
            None => (item, None),
        };
        let (start, end) = if range == "*" || (range.is_empty() && step.is_some()) {
            (self.min, self.max)
        } else if let Some((start, end)) = range.split_once('-') {
            (self.value(start)?, self.value(end)?)
        } else {
            let start = self.value(range)?;
            (start, if step.is_some() { self.max } else { start })
        };
        let span = self.max - self.min + 1;
        let stop = if end >= start {
            end
        } else if self.wraps {
            end + span
        } else {
            return Err(self.error(format!("the range {start}-{end} runs backward")));
        };
        let step = step.unwrap_or(1);
        let mut values = Vec::new();
        let mut value = start;
        while value <= stop {
            values.push(if value > self.max { value - span } else { value });
            value += step;
        }
        Ok(values)
    }
}

/// The values of a field up to 63, one bit each.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Field(u64);

impl Field {
    fn parse(text: &str, kind: &Kind) -> Result<Field, CronError> {
        let mut field = Field::default();
        for item in text.split(',') {
            field.add(kind, item)?;
        }
        Ok(field)
    }

    fn add(&mut self, kind: &Kind, item: &str) -> Result<(), CronError> {
        if item.is_empty() {
            return Err(kind.error("an empty item"));
        }
        for value in kind.expand(item)? {
            self.0 |= 1 << value;
        }
        Ok(())
    }

    fn contains(self, value: u8) -> bool {
        value < 64 && self.0 >> value & 1 == 1
    }

    fn first_at_or_after(self, value: u8) -> Option<u8> {
        let rest = self.0.checked_shr(u32::from(value)).unwrap_or(0);
        // At most 63, the highest bit.
        (rest != 0).then(|| value + rest.trailing_zeros() as u8)
    }

    fn last_at_or_before(self, value: u8) -> Option<u8> {
        let below = if value >= 63 {
            self.0
        } else {
            self.0 & ((1 << (value + 1)) - 1)
        };
        // At most 63, the highest bit.
        (below != 0).then(|| 63 - below.leading_zeros() as u8)
    }

    fn values(self) -> impl Iterator<Item = u8> {
        (0..64).filter(move |value| self.contains(*value))
    }
}

/// The day-of-month field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DaysOfMonth {
    /// `*` or `?`: the day of week decides.
    any: bool,
    days: Field,
    /// `L` (bit 0) and `L-n` (bit n): the day n before the last.
    before_last: u32,
    /// `LW`.
    last_weekday: bool,
    /// `nW` (bit n): the weekday nearest day n.
    nearest_weekday: u32,
}

impl DaysOfMonth {
    fn parse(text: &str) -> Result<DaysOfMonth, CronError> {
        let mut days = DaysOfMonth::default();
        if text == "*" || text == "?" {
            days.any = true;
            return Ok(days);
        }
        for item in text.split(',') {
            if item == "L" {
                days.before_last |= 1;
            } else if item == "LW" {
                days.last_weekday = true;
            } else if let Some(before) = item.strip_prefix("L-") {
                let before: u32 = before.parse().map_err(|_| DAY.error(format!("{item:?} is not L-n")))?;
                if before > 30 {
                    return Err(DAY.error(format!("{item}: n in L-n is not in 0-30")));
                }
                days.before_last |= 1 << before;
            } else if let Some(day) = item.strip_suffix('W') {
                days.nearest_weekday |= 1 << DAY.value(day)?;
            } else {
                days.days.add(&DAY, item)?;
            }
        }
        Ok(days)
    }

    fn matches(&self, date: Date, day: u8, last: u8) -> bool {
        if self.days.contains(day) || self.before_last >> (last - day) & 1 == 1 {
            return true;
        }
        if self.last_weekday && day == weekday_near(date.last_of_month(), last) {
            return true;
        }
        (1..=last)
            .filter(|n| self.nearest_weekday >> n & 1 == 1)
            .any(|n| Date::new(date.year(), date.month(), n as i8).is_ok_and(|on| weekday_near(on, last) == day))
    }
}

/// The weekday nearest `date` in its month: `date` itself from Monday to Friday, else the Friday before or the Monday
/// after, whichever is in the month (Quartz's `W`).
fn weekday_near(date: Date, last: u8) -> u8 {
    let day = date.day().unsigned_abs();
    match date.weekday() {
        jiff::civil::Weekday::Saturday if day == 1 => 3,
        jiff::civil::Weekday::Saturday => day - 1,
        jiff::civil::Weekday::Sunday if day == last => day - 2,
        jiff::civil::Weekday::Sunday => day + 1,
        _ => day,
    }
}

/// The day-of-week field, 1 (Sunday) to 7 (Saturday).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DaysOfWeek {
    /// `*` or `?`: the day of month decides.
    any: bool,
    days: Field,
    /// `nL` (bit n): the last day n of the month.
    last: u8,
    /// `n#k`: the kth day n of the month.
    nth: Vec<(u8, u8)>,
}

impl DaysOfWeek {
    fn parse(text: &str) -> Result<DaysOfWeek, CronError> {
        let mut days = DaysOfWeek::default();
        if text == "*" || text == "?" {
            days.any = true;
            return Ok(days);
        }
        for item in text.split(',') {
            if item == "L" {
                days.days.add(&WEEKDAY, "7")?;
            } else if let Some((day, nth)) = item.split_once('#') {
                let nth: u8 = nth
                    .parse()
                    .map_err(|_| WEEKDAY.error(format!("{nth:?} is not 1-5 in n#k")))?;
                if !(1..=5).contains(&nth) {
                    return Err(WEEKDAY.error(format!("{nth} is not 1-5 in n#k")));
                }
                // A day of the week is 1 to 7.
                days.nth.push((WEEKDAY.value(day)? as u8, nth));
            } else if let Some(day) = item.strip_suffix('L') {
                days.last |= 1 << WEEKDAY.value(day)?;
            } else {
                days.days.add(&WEEKDAY, item)?;
            }
        }
        Ok(days)
    }

    fn matches(&self, date: Date, day: u8, last: u8) -> bool {
        // Sunday is 1, Saturday 7.
        let weekday = date.weekday().to_sunday_one_offset().unsigned_abs();
        self.days.contains(weekday)
            || (self.last >> weekday & 1 == 1 && day + 7 > last)
            || self
                .nth
                .iter()
                .any(|&(on, nth)| on == weekday && (day - 1) / 7 + 1 == nth)
    }
}

/// The year field.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Years {
    Every,
    /// Ascending, each once.
    Only(Vec<i16>),
}

impl Years {
    fn parse(text: &str) -> Result<Years, CronError> {
        if text == "*" {
            return Ok(Years::Every);
        }
        let mut years = Vec::new();
        for item in text.split(',') {
            if item.is_empty() {
                return Err(YEAR.error("an empty item"));
            }
            // A year is 1970 to 2099.
            years.extend(YEAR.expand(item)?.into_iter().map(|year| year as i16));
        }
        years.sort_unstable();
        years.dedup();
        Ok(Years::Only(years))
    }

    fn first_at_or_after(&self, year: i16) -> Option<i16> {
        match self {
            Years::Every => Some(year),
            Years::Only(years) => years.iter().copied().find(|due| *due >= year),
        }
    }

    fn last_at_or_before(&self, year: i16) -> Option<i16> {
        match self {
            Years::Every => Some(year),
            Years::Only(years) => years.iter().rev().copied().find(|due| *due <= year),
        }
    }
}

#[cfg(test)]
mod tests;
