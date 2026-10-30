//! Relative dashboard times, evaluated once against the caller's clock and time zone.
//!
//! The syntax is Elastic date math as Grafana uses it: an anchor (`now`, or an absolute time followed by `||`), then
//! any sequence of `+N<unit>`, `-N<unit>` and `/<unit>` (rounding), so `now-1M/M`, `now/d+8h` and
//! `2026-01-01T00:00:00Z||+1M/w` all read. Units are `s m h d w M y`, `H` for hours, and `fy` (rounding only) for the
//! fiscal year. Weeks start on `<report week-start>` (0 Sunday, 1 Monday, up to 6 Saturday) and the fiscal year on
//! `<report fiscal-start-month fiscal-start-day>`.
//!
//! [`parse`] rounds down. [`parse_up`] rounds up as Elastic does for an `lte` or `gt` bound: a rounding then gives
//! the last millisecond of its unit, and any math after it applies from there. [`parse_thru`] is for an exclusive
//! end such as a report's `thru`: a rounded time closes at the start of the next unit, so `from=now/d&thru=now/d` is
//! all of today.
use jiff::civil::{Date, Weekday};
use jiff::{Span, Timestamp, Zoned};
use moqui_lib::error::Error;

fn fail(err: impl std::fmt::Display) -> Error {
    Error::new(err.to_string())
}

/// A time rounded down.
pub fn parse(text: &str, now: i64) -> Result<i64, Error> {
    eval(text, now, false, None).map(|(at, _)| at)
}

/// A time with each rounding up to the last millisecond of its unit, for an inclusive end (`lte`) or an exclusive
/// start (`gt`).
pub fn parse_up(text: &str, now: i64) -> Result<i64, Error> {
    eval(text, now, true, None).map(|(at, _)| at)
}

/// An exclusive end (`lt`): a rounded time is the start of the unit after it; a time with no rounding is as given.
pub fn parse_thru(text: &str, now: i64) -> Result<i64, Error> {
    let (at, rounded) = eval(text, now, true, None)?;
    if rounded {
        at.checked_add(1).ok_or_else(|| fail("report time overflow"))
    } else {
        Ok(at)
    }
}

/// The first day of the week from `<report week-start>`: 0 is Sunday, 1 Monday (the default), up to 6 Saturday.
pub fn week_start() -> Weekday {
    let number =
        crate::context::ExecutionContext::with_current(|ec| ec.factory().conf_number::<i8>("report", "week-start", 1))
            .unwrap_or(1);
    Weekday::from_sunday_zero_offset(number.rem_euclid(7)).unwrap_or(Weekday::Monday)
}

/// The date histogram `offset` that starts calendar week buckets (which both backends start on Monday) on
/// `start`, or `None` for Monday. A start late in the week shifts back, so a Sunday week is `-1d`.
pub fn week_offset(start: Weekday) -> Option<String> {
    match start.to_monday_zero_offset() {
        0 => None,
        days @ 1..=3 => Some(format!("+{days}d")),
        days => Some(format!("-{}d", 7 - days)),
    }
}

/// The time, and whether any rounding was applied. `week` overrides the configured week start.
fn eval(text: &str, now: i64, up: bool, week: Option<Weekday>) -> Result<(i64, bool), Error> {
    let text = text.trim();
    let (anchor, mut rest) = if let Some(rest) = text.strip_prefix("now") {
        (now, rest)
    } else if let Some((anchor, rest)) = text.split_once("||") {
        (absolute(anchor)?, rest)
    } else {
        return Ok((absolute(text)?, false));
    };
    let zone = crate::l10n::time_zone();
    let mut time = Timestamp::from_millisecond(anchor).map_err(fail)?.to_zoned(zone);
    let mut rounded = false;
    while let Some(op) = rest.chars().next() {
        rest = &rest[1..];
        match op {
            '+' | '-' => {
                let n = rest.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 {
                    return Err(fail(format!("invalid relative report time {text}")));
                }
                let amount = rest[..n].parse::<i64>().map_err(fail)? * if op == '-' { -1 } else { 1 };
                rest = &rest[n..];
                let unit = rest
                    .chars()
                    .next()
                    .ok_or_else(|| Error::new("relative time needs a unit"))?;
                rest = &rest[unit.len_utf8()..];
                time = time.checked_add(span(unit, amount)?).map_err(fail)?;
            }
            '/' => {
                let unit = if rest.starts_with("fy") {
                    "fy"
                } else {
                    rest.get(..1).unwrap_or_default()
                };
                if unit.is_empty() {
                    return Err(fail(format!("report time {text} has no rounding unit")));
                }
                rest = &rest[unit.len()..];
                time = round_down(&time, unit, week)?;
                if up {
                    let step = if unit == "fy" {
                        span('y', 1)?
                    } else {
                        span(unit.chars().next().unwrap(), 1)?
                    };
                    time = time
                        .checked_add(step)
                        .and_then(|next| next.checked_sub(Span::new().milliseconds(1)))
                        .map_err(fail)?;
                }
                rounded = true;
            }
            _ => return Err(fail(format!("invalid report time {text}"))),
        }
    }
    Ok((time.timestamp().as_millisecond(), rounded))
}

fn absolute(text: &str) -> Result<i64, Error> {
    if let Ok(number) = text.parse::<i64>() {
        return Timestamp::from_millisecond(number)
            .map(|t| t.as_millisecond())
            .map_err(fail);
    }
    text.parse::<Timestamp>()
        .map(|t| t.as_millisecond())
        .map_err(|e| fail(format!("invalid report time {text}: {e}")))
}

fn span(unit: char, amount: i64) -> Result<Span, Error> {
    match unit {
        's' => Span::new().try_seconds(amount),
        'm' => Span::new().try_minutes(amount),
        'h' | 'H' => Span::new().try_hours(amount),
        'd' => Span::new().try_days(amount),
        'w' => Span::new().try_weeks(amount),
        'M' => Span::new().try_months(amount),
        'y' => Span::new().try_years(amount),
        _ => return Err(fail(format!("unknown report time unit {unit}"))),
    }
    .map_err(fail)
}

fn round_down(time: &Zoned, unit: &str, week: Option<Weekday>) -> Result<Zoned, Error> {
    let smallest = match unit {
        "s" => Some(jiff::Unit::Second),
        "m" => Some(jiff::Unit::Minute),
        "h" | "H" => Some(jiff::Unit::Hour),
        _ => None,
    };
    if let Some(smallest) = smallest {
        return time
            .round(jiff::ZonedRound::new().smallest(smallest).mode(jiff::RoundMode::Floor))
            .map_err(fail);
    }
    let date = time.date();
    let date = match unit {
        "d" => date,
        "w" => date
            .checked_sub(Span::new().days(i64::from(date.weekday().since(week.unwrap_or_else(week_start)))))
            .map_err(fail)?,
        "M" => date.first_of_month(),
        "y" => date.first_of_year(),
        "fy" => {
            let (month, day) = crate::context::ExecutionContext::with_current(|ec| {
                (
                    ec.factory().conf_number::<i8>("report", "fiscal-start-month", 1),
                    ec.factory().conf_number::<i8>("report", "fiscal-start-day", 1),
                )
            })
            .unwrap_or((1, 1));
            let mut start = Date::new(date.year(), month, day).map_err(fail)?;
            if start > date {
                start = start.checked_sub(Span::new().years(1)).map_err(fail)?;
            }
            start
        }
        _ => return Err(fail(format!("unknown report time rounding {unit}"))),
    };
    date.at(0, 0, 0, 0).to_zoned(time.time_zone().clone()).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_ranges_are_anchored() {
        let now = 1_700_000_000_000;
        assert_eq!(parse("now-6h", now).unwrap(), now - 21_600_000);
        assert_eq!(parse("now+1m-30s", now).unwrap(), now + 30_000);
        assert!(parse("now-whatever", now).is_err());
        assert!(parse("now/", now).is_err());
        assert!(parse("now/q", now).is_err());
    }
    #[test]
    fn accepts_absolute_times() {
        assert_eq!(parse("2024-01-01T00:00:00Z", 0).unwrap(), 1_704_067_200_000);
    }
    fn at(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }
    #[test]
    fn rounding_up_and_math_after_rounding() {
        // With no execution context the zone is the server zone, UTC unless the runtime names one.
        if crate::l10n::time_zone().iana_name() != Some("UTC") {
            return;
        }
        let now = at("2026-01-07T15:30:12.345Z");
        assert_eq!(parse("now/d", now).unwrap(), at("2026-01-07T00:00:00Z"));
        assert_eq!(parse_up("now/d", now).unwrap(), at("2026-01-07T23:59:59.999Z"));
        assert_eq!(parse_thru("now/d", now).unwrap(), at("2026-01-08T00:00:00Z"));
        // No rounding leaves an exclusive end as given.
        assert_eq!(parse_thru("now-1h", now).unwrap(), now - 3_600_000);
        // Math after rounding: the eight in the morning of today, and of the day after a rounded-up today.
        assert_eq!(parse("now/d+8h", now).unwrap(), at("2026-01-07T08:00:00Z"));
        assert_eq!(parse_thru("now/d+8h", now).unwrap(), at("2026-01-08T08:00:00Z"));
        assert_eq!(parse("now-1d/d+8h/h", now).unwrap(), at("2026-01-06T08:00:00Z"));
        assert_eq!(parse_up("now-1M/M", now).unwrap(), at("2025-12-31T23:59:59.999Z"));
        assert_eq!(parse_up("now/y", now).unwrap(), at("2026-12-31T23:59:59.999Z"));
        assert_eq!(parse("now/s", now).unwrap(), at("2026-01-07T15:30:12Z"));
        // An absolute anchor with math, as Elastic writes it.
        assert_eq!(
            parse("2026-01-31T10:00:00Z||+1M/d", 0).unwrap(),
            at("2026-02-28T00:00:00Z")
        );
        // With no context the week starts Monday; 2026-01-07 is a Wednesday.
        assert_eq!(parse("now/w", now).unwrap(), at("2026-01-05T00:00:00Z"));
        assert_eq!(parse_thru("now/w", now).unwrap(), at("2026-01-12T00:00:00Z"));
        // A Sunday week starts 2026-01-04, the day a Sunday `-1d` histogram bucket starts too.
        let sunday = Some(Weekday::Sunday);
        assert_eq!(eval("now/w", now, false, sunday).unwrap().0, at("2026-01-04T00:00:00Z"));
        assert_eq!(
            eval("now/w", now, true, sunday).unwrap().0,
            at("2026-01-10T23:59:59.999Z")
        );
        let sunday_bucket = crate::search::agg::bucket_start(
            Timestamp::from_millisecond(now).unwrap(),
            crate::search::agg::CalendarUnit::Week,
            &jiff::tz::TimeZone::UTC,
            crate::search::agg::parse_offset(&week_offset(Weekday::Sunday).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(sunday_bucket.as_millisecond(), at("2026-01-04T00:00:00Z"));
        // On a Sunday itself the week is that day.
        let on_sunday = at("2026-01-04T09:00:00Z");
        assert_eq!(
            eval("now/w", on_sunday, false, sunday).unwrap().0,
            at("2026-01-04T00:00:00Z")
        );
    }
    #[test]
    fn week_offsets_start_buckets_on_the_week_start() {
        assert_eq!(week_offset(Weekday::Monday), None);
        assert_eq!(week_offset(Weekday::Sunday).as_deref(), Some("-1d"));
        assert_eq!(week_offset(Weekday::Saturday).as_deref(), Some("-2d"));
        assert_eq!(week_offset(Weekday::Tuesday).as_deref(), Some("+1d"));
        assert_eq!(week_offset(Weekday::Thursday).as_deref(), Some("+3d"));
        assert_eq!(week_offset(Weekday::Friday).as_deref(), Some("-3d"));
    }
}
