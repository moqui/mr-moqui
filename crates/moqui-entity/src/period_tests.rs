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

#[test]
fn epoch_millis_transfer_into_the_user_calendar_at_utc_midnight_too() {
    let chicago = TimeZone::get("America/Chicago").unwrap();
    // Java transfers UTC midnight into the user calendar: Nov 1 UTC is Oct 31 in Chicago.
    assert_eq!(
        millis_date(1_793_491_200_000, &chicago),
        Some("2026-10-31".parse().unwrap())
    );
    // a Timestamp at another instant is dated in the zone: 2026-11-01T03:00Z is Oct 31 at 22:00 in Chicago
    assert_eq!(
        millis_date(1_793_502_000_000, &chicago),
        Some("2026-10-31".parse().unwrap())
    );
}

// Change 7: These are Java's parseDate(default-zone) -> getCalendarSafe(user-zone) boundaries.
#[test]
fn explicit_period_anchor_matches_java_at_year_boundary_and_interior() {
    let chicago = TimeZone::get("America/Chicago").unwrap();
    let expected = |from: &str, thru: &str| {
        (
            Value::Timestamp(from.parse().unwrap()),
            Value::Timestamp(thru.parse().unwrap()),
        )
    };
    let boundary = period_range_in_zones("year", "0", "2026-01-01", &chicago, &TimeZone::UTC).unwrap();
    assert_eq!(boundary, expected("2025-01-01T06:00:00Z", "2026-01-01T06:00:00Z"));
    let millis = "2026-01-01T00:00:00Z"
        .parse::<Timestamp>()
        .unwrap()
        .as_millisecond()
        .to_string();
    assert_eq!(
        period_range_in_zones("year", "0", &millis, &chicago, &TimeZone::UTC).unwrap(),
        boundary
    );
    let interior = period_range_in_zones("30d", "-1", "2026-01-15", &chicago, &TimeZone::UTC).unwrap();
    assert_eq!(interior, expected("2025-12-15T06:00:00Z", "2026-01-15T06:00:00Z"));
    let fixture = "2026-01-01T00:00:00Z".parse::<Timestamp>().unwrap();
    for range in [boundary, interior] {
        let (Value::Timestamp(from), Value::Timestamp(thru)) = range else {
            panic!("timestamp range");
        };
        assert!(from <= fixture && fixture < thru, "the paired fixture is included");
    }
    // Configured server zone matters: equal default/user zones keep Jan 1 in 2026.
    assert_eq!(
        period_range_in_zones("year", "0", "2026-01-01", &chicago, &chicago).unwrap(),
        expected("2026-01-01T06:00:00Z", "2027-01-01T06:00:00Z")
    );
}
