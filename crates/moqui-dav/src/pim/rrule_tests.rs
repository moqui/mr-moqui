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

use super::*;

fn tz(name: &str) -> TimeZone {
    TimeZone::get(name).unwrap()
}

fn at(tz: &TimeZone, text: &str) -> Timestamp {
    let Some(Until::Local(local)) = parse_date_value(text) else {
        panic!("{text}")
    };
    instant(tz, local).unwrap()
}

/// The instances as wall-clock text in the zone, with the UTC offset.
fn run(rule: &str, start: &str, zone: &str, range: Option<(&str, &str)>) -> Vec<String> {
    let zone = tz(zone);
    let rule = Rule::parse(rule).unwrap();
    let range = range.map(|(from, to)| (at(&zone, from), at(&zone, to)));
    expand(
        &rule,
        at(&zone, start),
        &zone,
        range.map(|r| r.0),
        range.map(|r| r.1),
        MAX_INSTANCES,
    )
    .into_iter()
    .map(|instant| {
        instant
            .to_zoned(zone.clone())
            .strftime("%Y-%m-%d %H:%M %:z %a")
            .to_string()
    })
    .collect()
}

#[test]
fn weekly_keeps_the_wall_clock_across_daylight_saving() {
    let out = run("FREQ=WEEKLY;COUNT=4", "20261025T100000", "America/Chicago", None);
    assert_eq!(
        out,
        [
            "2026-10-25 10:00 -05:00 Sun",
            "2026-11-01 10:00 -06:00 Sun",
            "2026-11-08 10:00 -06:00 Sun",
            "2026-11-15 10:00 -06:00 Sun"
        ]
    );
    // the spec's weekly one on one: two of five in two weeks
    let out = run(
        "FREQ=WEEKLY;COUNT=5",
        "20270510T100000",
        "America/Chicago",
        Some(("20270509T000000", "20270522T235959")),
    );
    assert_eq!(out, ["2027-05-10 10:00 -05:00 Mon", "2027-05-17 10:00 -05:00 Mon"]);
}

#[test]
fn a_time_in_the_gap_moves_later_and_a_repeated_time_is_the_first() {
    let out = run("FREQ=DAILY;COUNT=3", "20270313T023000", "America/New_York", None);
    assert_eq!(
        out,
        [
            "2027-03-13 02:30 -05:00 Sat",
            "2027-03-14 03:30 -04:00 Sun",
            "2027-03-15 02:30 -04:00 Mon"
        ]
    );
    let out = run("FREQ=DAILY;COUNT=2", "20261031T013000", "America/New_York", None);
    assert_eq!(out, ["2026-10-31 01:30 -04:00 Sat", "2026-11-01 01:30 -04:00 Sun"]);
}

#[test]
fn monthly_byday_ordinals_and_setpos() {
    // the second Tuesday, the last Friday
    assert_eq!(
        run(
            "FREQ=MONTHLY;BYDAY=2TU;COUNT=3",
            "20260113T090000",
            "Europe/Berlin",
            None
        ),
        [
            "2026-01-13 09:00 +01:00 Tue",
            "2026-02-10 09:00 +01:00 Tue",
            "2026-03-10 09:00 +01:00 Tue"
        ]
    );
    assert_eq!(
        run("FREQ=MONTHLY;BYDAY=-1FR;COUNT=3", "20260130T090000", "UTC", None),
        [
            "2026-01-30 09:00 +00:00 Fri",
            "2026-02-27 09:00 +00:00 Fri",
            "2026-03-27 09:00 +00:00 Fri"
        ]
    );
    // the last weekday of the month
    assert_eq!(
        run(
            "FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=3",
            "20260130T170000",
            "UTC",
            None
        ),
        [
            "2026-01-30 17:00 +00:00 Fri",
            "2026-02-27 17:00 +00:00 Fri",
            "2026-03-31 17:00 +00:00 Tue"
        ]
    );
    // Friday the 13th
    assert_eq!(
        run(
            "FREQ=MONTHLY;BYDAY=FR;BYMONTHDAY=13;COUNT=3",
            "20260213T000000",
            "UTC",
            None
        ),
        [
            "2026-02-13 00:00 +00:00 Fri",
            "2026-03-13 00:00 +00:00 Fri",
            "2026-11-13 00:00 +00:00 Fri"
        ]
    );
    // the 31st skips the short months and they do not count
    assert_eq!(
        run("FREQ=MONTHLY;BYMONTHDAY=31;COUNT=3", "20260131T080000", "UTC", None),
        [
            "2026-01-31 08:00 +00:00 Sat",
            "2026-03-31 08:00 +00:00 Tue",
            "2026-05-31 08:00 +00:00 Sun"
        ]
    );
    assert_eq!(
        run("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3", "20260131T080000", "UTC", None),
        [
            "2026-01-31 08:00 +00:00 Sat",
            "2026-02-28 08:00 +00:00 Sat",
            "2026-03-31 08:00 +00:00 Tue"
        ]
    );
}

#[test]
fn yearly_rules() {
    // US Thanksgiving: the fourth Thursday of November
    assert_eq!(
        run(
            "FREQ=YEARLY;BYMONTH=11;BYDAY=4TH;COUNT=3",
            "20261126T000000",
            "America/New_York",
            None
        ),
        [
            "2026-11-26 00:00 -05:00 Thu",
            "2027-11-25 00:00 -05:00 Thu",
            "2028-11-23 00:00 -05:00 Thu"
        ]
    );
    // a birthday on the 29th of February comes every four years
    assert_eq!(
        run("FREQ=YEARLY;COUNT=2", "20280229T120000", "UTC", None),
        ["2028-02-29 12:00 +00:00 Tue", "2032-02-29 12:00 +00:00 Sun"]
    );
    // the 100th day, and week 20's Monday
    assert_eq!(
        run("FREQ=YEARLY;BYYEARDAY=100;COUNT=2", "20260410T000000", "UTC", None).len(),
        2
    );
    assert_eq!(
        run(
            "FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO;COUNT=2",
            "20260511T090000",
            "UTC",
            None
        ),
        ["2026-05-11 09:00 +00:00 Mon", "2027-05-17 09:00 +00:00 Mon"]
    );
    // RFC 5545's example: every 20th Monday of the year
    assert_eq!(
        run(
            "FREQ=YEARLY;BYDAY=20MO;COUNT=3",
            "19970519T090000",
            "America/New_York",
            None
        ),
        [
            "1997-05-19 09:00 -04:00 Mon",
            "1998-05-18 09:00 -04:00 Mon",
            "1999-05-17 09:00 -04:00 Mon"
        ]
    );
}

#[test]
fn until_interval_wkst_and_range() {
    // UNTIL is inclusive
    assert_eq!(
        run(
            "FREQ=DAILY;INTERVAL=2;UNTIL=20260107T090000Z",
            "20260101T090000",
            "UTC",
            None
        ),
        [
            "2026-01-01 09:00 +00:00 Thu",
            "2026-01-03 09:00 +00:00 Sat",
            "2026-01-05 09:00 +00:00 Mon",
            "2026-01-07 09:00 +00:00 Wed"
        ]
    );
    assert_eq!(
        run("FREQ=WEEKLY;UNTIL=20260115", "20260101T090000", "UTC", None).len(),
        3
    );
    // RFC 5545: WKST changes which week a day belongs to with INTERVAL=2
    assert_eq!(
        run(
            "FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=MO",
            "19970805T090000",
            "America/New_York",
            None
        ),
        [
            "1997-08-05 09:00 -04:00 Tue",
            "1997-08-10 09:00 -04:00 Sun",
            "1997-08-19 09:00 -04:00 Tue",
            "1997-08-24 09:00 -04:00 Sun"
        ]
    );
    assert_eq!(
        run(
            "FREQ=WEEKLY;INTERVAL=2;COUNT=4;BYDAY=TU,SU;WKST=SU",
            "19970805T090000",
            "America/New_York",
            None
        ),
        [
            "1997-08-05 09:00 -04:00 Tue",
            "1997-08-17 09:00 -04:00 Sun",
            "1997-08-19 09:00 -04:00 Tue",
            "1997-08-31 09:00 -04:00 Sun"
        ]
    );
    // far in the future without COUNT: the range only
    let out = run(
        "FREQ=WEEKLY;BYDAY=MO,WE",
        "20200106T090000",
        "Europe/London",
        Some(("20400101T000000", "20400114T235959")),
    );
    assert_eq!(
        out,
        [
            "2040-01-02 09:00 +00:00 Mon",
            "2040-01-04 09:00 +00:00 Wed",
            "2040-01-09 09:00 +00:00 Mon",
            "2040-01-11 09:00 +00:00 Wed"
        ]
    );
    // a rule that never matches ends
    assert!(run("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30", "20260101T000000", "UTC", None).len() == 1);
}

#[test]
fn dtstart_counts_even_off_the_rule() {
    // DTSTART on a Wednesday with BYDAY=MO: the Wednesday, then Mondays
    assert_eq!(
        run("FREQ=WEEKLY;BYDAY=MO;COUNT=3", "20261021T090000", "UTC", None),
        [
            "2026-10-21 09:00 +00:00 Wed",
            "2026-10-26 09:00 +00:00 Mon",
            "2026-11-02 09:00 +00:00 Mon"
        ]
    );
}

#[test]
fn hourly_and_minutely() {
    assert_eq!(
        run("FREQ=HOURLY;INTERVAL=3;COUNT=3", "20260101T090000", "UTC", None),
        [
            "2026-01-01 09:00 +00:00 Thu",
            "2026-01-01 12:00 +00:00 Thu",
            "2026-01-01 15:00 +00:00 Thu"
        ]
    );
    assert_eq!(
        run(
            "FREQ=MINUTELY;INTERVAL=15;BYHOUR=9,10;COUNT=6",
            "20260101T090000",
            "UTC",
            None
        )
        .len(),
        6
    );
    assert_eq!(
        run(
            "FREQ=DAILY;BYHOUR=9,17;BYMINUTE=0,30;COUNT=5",
            "20260101T090000",
            "UTC",
            None
        ),
        [
            "2026-01-01 09:00 +00:00 Thu",
            "2026-01-01 09:30 +00:00 Thu",
            "2026-01-01 17:00 +00:00 Thu",
            "2026-01-01 17:30 +00:00 Thu",
            "2026-01-02 09:00 +00:00 Fri"
        ]
    );
}

#[test]
fn sets_add_rdates_and_remove_exdates() {
    let zone = tz("America/Chicago");
    let start = at(&zone, "20261005T100000");
    let resolve = |name: &str| TimeZone::get(name).ok();
    let out = expand_set(
        Some("FREQ=WEEKLY;COUNT=4"),
        start,
        &zone,
        &["20261012T100000".to_string(), "20261026".to_string()],
        &[
            "20261030T150000Z,20261031".to_string(),
            "TZID=Europe/Paris:20261101T090000".to_string(),
        ],
        None,
        None,
        &resolve,
    )
    .unwrap();
    let text: Vec<String> = out
        .iter()
        .map(|at| at.to_zoned(zone.clone()).strftime("%m-%d %H:%M").to_string())
        .collect();
    assert_eq!(
        text,
        [
            "10-05 10:00",
            "10-19 10:00",
            "10-30 10:00",
            "10-31 10:00",
            "11-01 02:00"
        ]
    );
    // without a rule: DTSTART and the RDATEs
    let out = expand_set(
        None,
        start,
        &zone,
        &[],
        &["20261006T100000".to_string()],
        None,
        None,
        &resolve,
    )
    .unwrap();
    assert_eq!(out.len(), 2);
    assert!(expand_set(Some("WEEKLY"), start, &zone, &[], &[], None, None, &resolve)
        .unwrap_err()
        .contains("not an RRULE"));
    assert!(expand_set(
        None,
        start,
        &zone,
        &["next week".to_string()],
        &[],
        None,
        None,
        &resolve
    )
    .is_err());
}

#[test]
fn rules_parse_and_refuse() {
    let rule = Rule::parse("RRULE:freq=monthly;byday=+1MO,-2fr;interval=2;wkst=su;x-name=1").unwrap();
    assert_eq!(rule.freq, Freq::Monthly);
    assert_eq!(rule.by_day, [(1, Weekday::Monday), (-2, Weekday::Friday)]);
    assert_eq!(rule.interval, 2);
    assert_eq!(rule.wkst, Weekday::Sunday);
    assert!(Rule::parse("FREQ=FORTNIGHTLY").is_err());
    assert!(Rule::parse("FREQ=DAILY;BYMONTHDAY=0").is_err());
    assert!(Rule::parse("FREQ=DAILY;INTERVAL=0").is_err());
    assert!(Rule::parse("COUNT=3").unwrap_err().contains("not an RRULE"));
}
