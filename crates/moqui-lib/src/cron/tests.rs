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

use jiff::Zoned;

use super::Cron;

fn cron(text: &str) -> Cron {
    Cron::parse(text).unwrap_or_else(|err| panic!("{text}: {err}"))
}

fn at(text: &str) -> Zoned {
    text.parse().unwrap()
}

/// The next times `text` is due after `from`, as text.
fn next(text: &str, from: &str, count: usize) -> Vec<String> {
    let cron = cron(text);
    let mut times = Vec::new();
    let mut after = at(from);
    for _ in 0..count {
        let Some(due) = cron.next_after(&after) else { break };
        times.push(due.to_string());
        after = due;
    }
    times
}

fn last(text: &str, at_or_before: &str) -> Option<String> {
    cron(text)
        .last_at_or_before(&at(at_or_before))
        .map(|due| due.to_string())
}

#[test]
fn the_seeded_patterns_parse_and_the_others_are_refused_with_the_field() {
    for text in [
        "0 0/15 * * * ?",
        "0 0 2 * * ?",
        "0 */5 * * * ?",
        "0 0 6 ? * MON",
        "0 0 0 1 1 ? 2099",
        "* * * * * *",
        "0 0 12 ? jan-mar mon-fri",
        "0 0 0 L-3,LW,15W * ?",
        "0 0 0 ? * 6L,2#1,L",
    ] {
        cron(text);
    }
    for (text, error) in [
        ("*/5 * * * *", "6 or 7 fields"),
        ("0 0 0 1 1 ? 2000 1", "6 or 7 fields"),
        ("0 0 12 15 * MON", "not both"),
        ("? 0 12 * * ?", "the seconds field"),
        ("0 60 * * * ?", "the minutes field: 60 is not in 0-59"),
        ("0 0/0 * * * ?", "the step 0"),
        ("0 0/61 * * * ?", "the step 61"),
        ("0 0 0 ? * 2#6", "not 1-5"),
        ("0 0 0 ? * FUN", "not a number or a name"),
        ("0 0 0 1,,2 * ?", "an empty item"),
        ("0 0 0 L-31 * ?", "not in 0-30"),
        ("0 0 0 1 1 ? 2030-2020", "runs backward"),
        ("0 0 0 1 1 ? 2100", "not in 1970-2099"),
    ] {
        let err = Cron::parse(text).unwrap_err().to_string();
        assert!(err.contains(error), "{text}: {err}");
    }
}

#[test]
fn steps_ranges_names_and_wrapping_ranges_come_due_in_order() {
    assert_eq!(
        next("0 0/15 * * * ?", "2026-01-01T00:07:30[UTC]", 3),
        [
            "2026-01-01T00:15:00+00:00[UTC]",
            "2026-01-01T00:30:00+00:00[UTC]",
            "2026-01-01T00:45:00+00:00[UTC]"
        ]
    );
    // Strictly after: a moment that is itself due gives the next one.
    assert_eq!(
        next("0 0 2 * * ?", "2026-01-01T02:00:00[UTC]", 1),
        ["2026-01-02T02:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 2 * * ?", "2026-01-01T01:59:59.5[UTC]", 1),
        ["2026-01-01T02:00:00+00:00[UTC]"]
    );
    // 22-2 wraps past midnight; 20-4/3 steps across it.
    assert_eq!(
        next("0 0 22-2 * * ?", "2026-01-01T12:00:00[UTC]", 5),
        [
            "2026-01-01T22:00:00+00:00[UTC]",
            "2026-01-01T23:00:00+00:00[UTC]",
            "2026-01-02T00:00:00+00:00[UTC]",
            "2026-01-02T01:00:00+00:00[UTC]",
            "2026-01-02T02:00:00+00:00[UTC]"
        ]
    );
    assert_eq!(
        next("0 0 20-4/3 * * ?", "2026-01-01T12:00:00[UTC]", 3),
        [
            "2026-01-01T20:00:00+00:00[UTC]",
            "2026-01-01T23:00:00+00:00[UTC]",
            "2026-01-02T02:00:00+00:00[UTC]"
        ]
    );
    // FRI-MON from Tuesday 2026-01-06: Friday 9, Saturday 10, Sunday 11, Monday 12.
    assert_eq!(
        next("0 0 0 ? * FRI-MON", "2026-01-06T00:00:00[UTC]", 4),
        [
            "2026-01-09T00:00:00+00:00[UTC]",
            "2026-01-10T00:00:00+00:00[UTC]",
            "2026-01-11T00:00:00+00:00[UTC]",
            "2026-01-12T00:00:00+00:00[UTC]"
        ]
    );
    // Sunday is 1, as in Quartz: 2026-01-04 is a Sunday.
    assert_eq!(
        next("0 0 8 ? * 1", "2026-01-01T00:00:00[UTC]", 1),
        ["2026-01-04T08:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 1 MAR,jul ?", "2026-04-01T00:00:00[UTC]", 2),
        ["2026-07-01T00:00:00+00:00[UTC]", "2027-03-01T00:00:00+00:00[UTC]"]
    );
}

#[test]
fn the_day_of_month_specials_follow_quartz() {
    // L in a leap February and the next one; L-3.
    assert_eq!(
        next("0 0 12 L * ?", "2028-02-10T00:00:00[UTC]", 2),
        ["2028-02-29T12:00:00+00:00[UTC]", "2028-03-31T12:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 L-3 * ?", "2026-01-01T00:00:00[UTC]", 2),
        ["2026-01-28T00:00:00+00:00[UTC]", "2026-02-25T00:00:00+00:00[UTC]"]
    );
    // LW: January 2026 ends on a Saturday (Friday 30th), March on a Tuesday, May on a Sunday (Friday 29th).
    assert_eq!(
        next("0 0 0 LW * ?", "2026-01-01T00:00:00[UTC]", 1),
        ["2026-01-30T00:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 LW * ?", "2026-03-01T00:00:00[UTC]", 1),
        ["2026-03-31T00:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 LW * ?", "2026-05-01T00:00:00[UTC]", 1),
        ["2026-05-29T00:00:00+00:00[UTC]"]
    );
    // 15W: the 15th of February 2026 is a Sunday (Monday 16th), of August a Saturday (Friday 14th).
    assert_eq!(
        next("0 0 0 15W * ?", "2026-02-01T00:00:00[UTC]", 1),
        ["2026-02-16T00:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 15W * ?", "2026-08-01T00:00:00[UTC]", 1),
        ["2026-08-14T00:00:00+00:00[UTC]"]
    );
    // 1W stays in its month: 1 August 2026 is a Saturday, so Monday the 3rd. 31W: 31 May 2026 is a Sunday, so
    // Friday the 29th; a month of 30 days has no 31st.
    assert_eq!(
        next("0 0 0 1W * ?", "2026-07-31T00:00:00[UTC]", 1),
        ["2026-08-03T00:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 31W * ?", "2026-04-01T00:00:00[UTC]", 1),
        ["2026-05-29T00:00:00+00:00[UTC]"]
    );
    // 31 February never comes.
    assert!(next("0 0 0 31 2 ?", "2026-01-01T00:00:00[UTC]", 1).is_empty());
}

#[test]
fn the_day_of_week_specials_follow_quartz() {
    // 6L: the last Friday (30 January, 27 February 2026). L alone: Saturday.
    assert_eq!(
        next("0 0 0 ? * 6L", "2026-01-01T00:00:00[UTC]", 2),
        ["2026-01-30T00:00:00+00:00[UTC]", "2026-02-27T00:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 ? * L", "2026-01-01T00:00:00[UTC]", 1),
        ["2026-01-03T00:00:00+00:00[UTC]"]
    );
    // 2#1: the first Monday; 6#5: a fifth Friday, which February to April 2026 do not have.
    assert_eq!(
        next("0 0 9 ? * 2#1", "2026-01-31T00:00:00[UTC]", 1),
        ["2026-02-02T09:00:00+00:00[UTC]"]
    );
    assert_eq!(
        next("0 0 0 ? * FRI#5", "2026-01-31T00:00:00[UTC]", 1),
        ["2026-05-29T00:00:00+00:00[UTC]"]
    );
    // mr-mantle's Columbus Day (HolidayUsaData.xml): the second Monday of October.
    assert_eq!(
        next("0 15 10 ? 10 2#2", "2026-01-01T00:00:00[UTC]", 1),
        ["2026-10-12T10:15:00+00:00[UTC]"]
    );
}

/// Epoch 0 is still 1969 west of Greenwich: the year field's 1970 is not a bound on the search.
#[test]
fn a_moment_before_1970_on_the_wall_clock_has_a_next_time() {
    assert_eq!(
        next("0 0/15 * * * ?", "1969-12-31T18:00:00-06:00[America/Chicago]", 1),
        ["1969-12-31T18:15:00-06:00[America/Chicago]"]
    );
}

#[test]
fn a_year_field_ends_the_schedule() {
    assert_eq!(
        next("0 0 0 1 1 ? 2000", "1970-01-01T00:00:00[UTC]", 2),
        ["2000-01-01T00:00:00+00:00[UTC]"]
    );
    assert_eq!(
        last("0 0 0 1 1 ? 2000", "2026-06-01T00:00:00[UTC]").as_deref(),
        Some("2000-01-01T00:00:00+00:00[UTC]")
    );
    assert_eq!(last("0 0 0 1 1 ? 2030", "2026-06-01T00:00:00[UTC]"), None);
}

#[test]
fn the_last_time_due_is_at_or_before_the_moment() {
    assert_eq!(
        last("0 0 2 * * ?", "2026-01-01T01:00:00[UTC]").as_deref(),
        Some("2025-12-31T02:00:00+00:00[UTC]")
    );
    assert_eq!(
        last("0 0 2 * * ?", "2026-01-01T02:00:00[UTC]").as_deref(),
        Some("2026-01-01T02:00:00+00:00[UTC]")
    );
    assert_eq!(
        last("0 0/15 * * * ?", "2026-01-01T00:14:59.9[UTC]").as_deref(),
        Some("2026-01-01T00:00:00+00:00[UTC]")
    );
    assert_eq!(
        last("0 0 0 ? * 2#1", "2026-02-01T00:00:00[UTC]").as_deref(),
        Some("2026-01-05T00:00:00+00:00[UTC]")
    );
}

#[test]
fn clock_changes_skip_forward_and_repeat_once() {
    // New York, 8 March 2026: 02:00 jumps to 03:00, so 02:30 comes due at 03:30.
    assert_eq!(
        next("0 30 2 * * ?", "2026-03-08T01:00:00-05:00[America/New_York]", 2),
        [
            "2026-03-08T03:30:00-04:00[America/New_York]",
            "2026-03-09T02:30:00-04:00[America/New_York]"
        ]
    );
    assert_eq!(
        last("0 30 2 * * ?", "2026-03-08T03:10:00-04:00[America/New_York]").as_deref(),
        Some("2026-03-07T02:30:00-05:00[America/New_York]")
    );
    assert_eq!(
        last("0 30 2 * * ?", "2026-03-08T03:30:00-04:00[America/New_York]").as_deref(),
        Some("2026-03-08T03:30:00-04:00[America/New_York]")
    );
    // 1 November 2026: 02:00 falls back to 01:00. 01:30 comes due once, and every 15 minutes skips the second 01:xx.
    assert_eq!(
        next("0 30 1 * * ?", "2026-11-01T00:00:00-04:00[America/New_York]", 2),
        [
            "2026-11-01T01:30:00-04:00[America/New_York]",
            "2026-11-02T01:30:00-05:00[America/New_York]"
        ]
    );
    assert_eq!(
        next("0 0/15 * * * ?", "2026-11-01T01:45:00-04:00[America/New_York]", 1),
        ["2026-11-01T02:00:00-05:00[America/New_York]"]
    );
    assert_eq!(
        last("0 0 1 * * ?", "2026-11-01T01:10:00-05:00[America/New_York]").as_deref(),
        Some("2026-11-01T01:00:00-04:00[America/New_York]")
    );
}

#[test]
fn descriptions_read_as_english() {
    for (text, description) in [
        ("* * * * * ?", "Every second."),
        ("0 * * * * ?", "Every minute."),
        ("0 0/15 * * * ?", "Every 15 minutes."),
        ("0 17 * * * ?", "At minute 17 past every hour."),
        ("0 0 2 * * ?", "At 02:00."),
        ("15 30 6 * * ?", "At 06:30:15."),
        ("0 0 8,17 * * ?", "At 08:00 and 17:00."),
        ("0 0 */2 * * ?", "At minute 0, of every 2 hours."),
        ("0 0/10 8-17 ? * MON-FRI", "Every 10 minutes, past hour 8-17, on Monday through Friday."),
        ("0 0 6 ? * MON", "At 06:00, on Monday."),
        ("0 0 12 L * ?", "At 12:00, on the last day of the month."),
        ("0 0 0 1,15 * ?", "At 00:00, on day 1 and 15."),
        ("0 0 0 L-3,LW,15W * ?", "At 00:00, on 3 days before the last day of the month, the last weekday of the month and the weekday nearest day 15."),
        ("0 0 9 ? * 2#1,6L", "At 09:00, on the last Friday of the month and the 1st Monday of the month."),
        ("0 0 0 1 1,7 ? 2099", "At 00:00, on day 1, in January and July, in 2099."),
        ("0 0 0 1 JAN-MAR ?", "At 00:00, on day 1, in January through March."),
    ] {
        assert_eq!(cron(text).describe(), description, "{text}");
    }
}
