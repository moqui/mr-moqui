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

//! The English description of a cron expression ([`super::Cron::describe`]): the time of day, then the days, the
//! months and the years when they are not every one. "At 02:00.", "Every 15 minutes.", "At 06:00, on Monday through
//! Friday.", "At 00:00, on the last day of the month, in January and July."

use super::{Cron, Field, Years};

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub(super) fn describe(cron: &Cron) -> String {
    let mut parts = vec![time(cron)];
    parts.extend(days(cron));
    let months: Vec<u16> = cron.months.values().map(u16::from).collect();
    if months.len() < 12 {
        parts.push(format!("in {}", names(&months, &MONTHS, 1)));
    }
    if let Years::Only(years) = &cron.years {
        let years: Vec<u16> = years.iter().map(|year| year.unsigned_abs()).collect();
        parts.push(format!("in {}", numbers(&years)));
    }
    let text = parts.join(", ");
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// The time of day: "every minute", "at 02:00", "every 15 minutes, past hour 8-17".
fn time(cron: &Cron) -> String {
    let seconds = values(cron.seconds);
    let minutes = values(cron.minutes);
    let hours = values(cron.hours);
    let (every_second, every_minute, every_hour) = (seconds.len() == 60, minutes.len() == 60, hours.len() == 24);
    let on_the_minute = seconds == [0];
    if every_second && every_minute && every_hour {
        return "every second".to_string();
    }
    if on_the_minute && every_minute && every_hour {
        return "every minute".to_string();
    }
    if on_the_minute && every_hour {
        if let Some(step) = every_nth(&minutes, 0, 59) {
            return format!("every {step} minutes");
        }
    }
    // A few clock times: "at 02:00", "at 08:00 and 17:00", "at 06:30:15".
    if minutes.len() == 1 && hours.len() <= 4 && (on_the_minute || seconds.len() == 1) {
        let times: Vec<String> = hours
            .iter()
            .map(|hour| match on_the_minute {
                true => format!("{hour:02}:{:02}", minutes[0]),
                false => format!("{hour:02}:{:02}:{:02}", minutes[0], seconds[0]),
            })
            .collect();
        return format!("at {}", list(&times));
    }
    let mut parts = Vec::new();
    if every_second {
        parts.push("every second".to_string());
    } else if !on_the_minute {
        parts.push(match every_nth(&seconds, 0, 59) {
            Some(step) => format!("every {step} seconds"),
            None => format!("at second {}", numbers(&widen(&seconds))),
        });
    }
    if !every_minute {
        parts.push(match every_nth(&minutes, 0, 59) {
            Some(step) => format!("every {step} minutes"),
            None if every_hour => format!("at minute {} past every hour", numbers(&widen(&minutes))),
            None => format!("at minute {}", numbers(&widen(&minutes))),
        });
    } else if parts.is_empty() {
        parts.push("every minute".to_string());
    }
    if !every_hour {
        parts.push(match every_nth(&hours, 0, 23) {
            Some(step) => format!("of every {step} hours"),
            None => format!("past hour {}", numbers(&widen(&hours))),
        });
    }
    parts.join(", ")
}

/// The days, when not every day: "on day 1 and 15", "on Monday through Friday", "on the last Friday of the month".
fn days(cron: &Cron) -> Option<String> {
    let mut parts = Vec::new();
    if !cron.days.any {
        let days = widen(&values(cron.days.days));
        if !days.is_empty() {
            parts.push(format!("day {}", numbers(&days)));
        }
        for before in (0..=30).filter(|before| cron.days.before_last >> before & 1 == 1) {
            parts.push(match before {
                0 => "the last day of the month".to_string(),
                1 => "1 day before the last day of the month".to_string(),
                _ => format!("{before} days before the last day of the month"),
            });
        }
        if cron.days.last_weekday {
            parts.push("the last weekday of the month".to_string());
        }
        for day in (1..=31).filter(|day| cron.days.nearest_weekday >> day & 1 == 1) {
            parts.push(format!("the weekday nearest day {day}"));
        }
    } else if !cron.weekdays.any {
        let days = widen(&values(cron.weekdays.days));
        if !days.is_empty() {
            parts.push(names(&days, &WEEKDAYS, 1));
        }
        for day in (1..=7usize).filter(|day| cron.weekdays.last >> day & 1 == 1) {
            parts.push(format!("the last {} of the month", WEEKDAYS[day - 1]));
        }
        for &(day, nth) in &cron.weekdays.nth {
            let ordinal = match nth {
                1 => "1st",
                2 => "2nd",
                3 => "3rd",
                4 => "4th",
                _ => "5th",
            };
            parts.push(format!("the {ordinal} {} of the month", WEEKDAYS[usize::from(day) - 1]));
        }
    }
    (!parts.is_empty()).then(|| format!("on {}", list(&parts)))
}

fn values(field: Field) -> Vec<u8> {
    field.values().collect()
}

fn widen(values: &[u8]) -> Vec<u16> {
    values.iter().copied().map(u16::from).collect()
}

/// The step when `values` are every nth from `min` to the end of the field (0, 15, 30, 45 of 0-59), with n above 1.
fn every_nth(values: &[u8], min: u8, max: u8) -> Option<u8> {
    let (&first, &last) = (values.first()?, values.last()?);
    let step = values.get(1)?.checked_sub(first)?;
    let evenly = values.windows(2).all(|pair| pair[1] - pair[0] == step);
    (first == min && step > 1 && evenly && u16::from(last) + u16::from(step) > u16::from(max)).then_some(step)
}

/// Runs of three or more as a range: "1, 5 and 8-12".
fn numbers(values: &[u16]) -> String {
    let items: Vec<String> = runs(values)
        .into_iter()
        .flat_map(|(start, end)| match end - start {
            0 => vec![start.to_string()],
            1 => vec![start.to_string(), end.to_string()],
            _ => vec![format!("{start}-{end}")],
        })
        .collect();
    list(&items)
}

/// Names, with runs of three or more as a range: "Monday through Friday". `first` is the value of `names[0]`.
fn names(values: &[u16], names: &[&str], first: u16) -> String {
    let name = |value: u16| {
        names
            .get(usize::from(value - first))
            .copied()
            .unwrap_or("?")
            .to_string()
    };
    let items: Vec<String> = runs(values)
        .into_iter()
        .flat_map(|(start, end)| match end - start {
            0 => vec![name(start)],
            1 => vec![name(start), name(end)],
            _ => vec![format!("{} through {}", name(start), name(end))],
        })
        .collect();
    list(&items)
}

/// Ascending values as runs of consecutive ones, each a start and an end.
fn runs(values: &[u16]) -> Vec<(u16, u16)> {
    let mut runs: Vec<(u16, u16)> = Vec::new();
    for &value in values {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == value => *end = value,
            _ => runs.push((value, value)),
        }
    }
    runs
}

/// "a", "a and b", "a, b and c".
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [front @ .., last] => format!("{} and {last}", front.join(", ")),
    }
}
