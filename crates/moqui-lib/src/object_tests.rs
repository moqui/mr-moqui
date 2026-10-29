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

/// The browser's `$mq.compare` (WebrootVue.qvt.js) is checked against the same vectors.
#[test]
fn compare_vectors() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/compare-vectors.json")).unwrap();
    let vectors: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    for vector in vectors {
        let field = crate::json::from_json(&vector[0]);
        let to = crate::json::from_json(&vector[2]);
        let got = compare(&field, vector[1].as_str().unwrap(), &to, vector[3].as_str().unwrap()).unwrap();
        assert_eq!(got, vector[4].as_bool().unwrap(), "{vector}");
    }
}

#[test]
fn a_double_out_of_the_integer_range_does_not_convert() {
    assert_eq!(
        convert_to(&Value::F64(42.0), "Long", &TimeZone::UTC),
        Some(Value::I64(42))
    );
    assert_eq!(
        convert_to(&Value::F64(-9.223_372_036_854_776e18), "Long", &TimeZone::UTC),
        Some(Value::I64(i64::MIN))
    );
    assert_eq!(convert_to(&Value::F64(1e20), "Long", &TimeZone::UTC), None);
    assert_eq!(convert_to(&Value::F64(-1e20), "Long", &TimeZone::UTC), None);
    assert_eq!(
        convert_to(&Value::F64(9.223_372_036_854_776e18), "Long", &TimeZone::UTC),
        None
    );
    assert_eq!(convert_to(&Value::F64(f64::NAN), "Long", &TimeZone::UTC), None);
    assert_eq!(convert_to(&Value::F64(3e10), "Integer", &TimeZone::UTC), None);
    assert_eq!(convert_to(&Value::F64(7.5), "Integer", &TimeZone::UTC), None);
}

#[test]
fn decimal_arithmetic_keeps_the_operand_scale_with_a_zero_operand() {
    let d = |text: &str| Decimal::from_str(text).unwrap();
    assert_eq!(decimal_subtract(d("1"), d("0.0")).unwrap().to_string(), "1.0");
    assert_eq!(decimal_add(d("0.00"), d("3")).unwrap().to_string(), "3.00");
    assert_eq!(decimal_multiply(d("0.0"), d("2.00")).unwrap().to_string(), "0.000");
    assert_eq!(decimal_subtract(d("1"), d("0.5")).unwrap().to_string(), "0.5");
}

#[test]
fn decimal_divide_uses_groovy_scale() {
    let quotient = decimal_divide(Decimal::from(1), Decimal::from(3)).unwrap();
    assert_eq!(quotient.to_string(), "0.3333333333");
    let sixth = decimal_divide(Decimal::from(1), Decimal::from(6)).unwrap();
    assert_eq!(sixth.to_string(), "0.1666666667");
    assert!(decimal_multiply(Decimal::MAX, Decimal::from(2)).is_err());
    assert!(decimal_eq(
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("1.00").unwrap()
    ));
    assert!(
        !decimal_is_zero(Decimal::from_str("0.00").unwrap()) || decimal_is_zero(Decimal::from_str("0.00").unwrap())
    );
    assert!(decimal_is_zero(Decimal::from_str("0.00").unwrap()));
}

#[test]
fn boolean_and_empty_follow_object_utilities() {
    let yes = basic_convert(&Value::string("Y"), "Boolean");
    assert_eq!(yes.value, Value::Bool(true));
    assert!(yes.changed);
    assert_eq!(
        basic_convert(&Value::string("yes"), "Boolean").value,
        Value::Bool(false)
    );
    assert_eq!(
        basic_convert(&Value::string("false"), "java.lang.Boolean").value,
        Value::Bool(false)
    );
    assert!(!is_empty(&Value::I32(0)));
    assert!(!is_empty(&Value::Bool(false)));
    assert!(is_empty(&Value::string("")));
    assert!(is_empty(&Value::Null));
    let bad = basic_convert(&Value::string("nope"), "Integer");
    assert!(!bad.changed);
    assert_eq!(bad.value, Value::string("nope"));
}

/// Timestamp text without an offset is read in the zone given, Java's `parseTimestamp` in the user's zone; an offset,
/// `Z` and epoch millis are the moment they name in any zone, and a date or time stays a civil value.
#[test]
fn timestamp_text_is_read_in_the_zone_given() {
    let chicago = TimeZone::get("America/Chicago").unwrap();
    let stamp = |text: &str| Value::Timestamp(text.parse::<Timestamp>().unwrap());
    for (text, in_chicago, in_utc) in [
        ("2016-11-01", "2016-11-01T05:00:00Z", "2016-11-01T00:00:00Z"),
        ("2016-11-08 00:00:00.0", "2016-11-08T06:00:00Z", "2016-11-08T00:00:00Z"),
        ("2016-11-10 12:00", "2016-11-10T18:00:00Z", "2016-11-10T12:00:00Z"),
        ("2016-11-10T12:00:00Z", "2016-11-10T12:00:00Z", "2016-11-10T12:00:00Z"),
        (
            "2016-11-10T12:00:00-03:00",
            "2016-11-10T15:00:00Z",
            "2016-11-10T15:00:00Z",
        ),
        ("1478779200000", "2016-11-10T12:00:00Z", "2016-11-10T12:00:00Z"),
    ] {
        assert_eq!(
            basic_convert_in(&Value::string(text), "java.sql.Timestamp", &chicago).value,
            stamp(in_chicago),
            "{text}"
        );
        assert_eq!(
            convert_checked_in(&Value::string(text), "Timestamp", &chicago),
            Some(stamp(in_chicago)),
            "{text}"
        );
        assert_eq!(
            basic_convert(&Value::string(text), "java.sql.Timestamp").value,
            stamp(in_utc),
            "{text}"
        );
    }
    assert_eq!(
        basic_convert_in(&Value::string("2016-11-01"), "java.sql.Date", &chicago).value,
        Value::Date(Date::new(2016, 11, 1).unwrap())
    );
    assert!(compare_in(
        &Value::string("2016-11-01 00:00"),
        "equals",
        &stamp("2016-11-01T05:00:00Z"),
        "Timestamp",
        &chicago
    )
    .unwrap());
    // Unregistered, the text zone is UTC.
    if TEXT_TIME_ZONE.get().is_none() {
        assert_eq!(text_time_zone(), TimeZone::UTC);
    }
}

#[test]
fn timestamps_dates_and_plain_millis() {
    let converted = basic_convert(&Value::string("2020-01-02 03:04:05"), "java.sql.Timestamp");
    let Value::Timestamp(timestamp) = converted.value else {
        panic!("not a timestamp");
    };
    let expected = parse_timestamp("2020-01-02T03:04:05Z", &TimeZone::UTC).unwrap();
    assert_eq!(timestamp, expected);
    assert_eq!(
        to_plain_string(&Value::Timestamp(timestamp)),
        expected.as_millisecond().to_string()
    );
    assert_eq!(parse_date("01/02/2020").unwrap(), Date::new(2020, 1, 2).unwrap());
    assert_eq!(parse_time("13:05").unwrap(), Time::new(13, 5, 0, 0).unwrap());
    assert_eq!(
        to_plain_string(&Value::Date(Date::new(1970, 1, 2).unwrap())),
        "86400000"
    );
}

#[test]
fn like_and_compare() {
    assert!(compare_like(&Value::string("Hello"), &Value::string("h%")));
    assert!(compare_like(&Value::string("Hello"), &Value::string("h_llo")));
    assert!(!compare_like(&Value::string("Hello"), &Value::string("x%")));
    assert!(compare_like(&Value::string("a.b"), &Value::string("a.b")));
    assert!(compare_like(&Value::Null, &Value::Null) || compare_like(&Value::string("a"), &Value::Null));
    assert!(compare(&Value::string("10"), "greater", &Value::string("2"), "Integer").unwrap());
}

#[test]
fn like_wildcards_match_without_backtracking_or_recursion() {
    let like = |text: &str, pattern: &str| compare_like(&Value::string(text), &Value::string(pattern));
    assert!(like("", "") && like("", "%") && like("", "%%") && !like("", "_"));
    assert!(like("abc", "%") && like("abc", "a%c") && like("abc", "%b%") && like("abc", "___"));
    assert!(!like("abc", "__") && !like("abc", "____") && !like("abc", "a%b") && !like("ab", "a_%b"));
    assert!(like("abcbc", "a%bc") && like("aXbYc", "a%b%c") && like("ABC", "a_c") && like("mississippi", "%sip%"));
    assert!(!like("mississippi", "%sipx%") && like("xaab", "%a_b") && !like("ab", "%a_b"));
    // A pattern that backtracks exponentially, and a text long enough to overflow the stack by recursion.
    let text = "a".repeat(5_000);
    let pattern = format!("{}b", "%a".repeat(40));
    assert!(!like(&text, &pattern));
    assert!(like(&"x".repeat(1_000_000), "%x"));
}

#[test]
fn add_numbers_errors_on_integer_overflow() {
    assert_eq!(add_numbers(&Value::I64(1), &Value::I64(2)).unwrap(), Value::I64(3));
    assert_eq!(add_numbers(&Value::I32(1), &Value::I32(2)).unwrap(), Value::I32(3));
    let long = add_numbers(&Value::I64(i64::MAX), &Value::I64(1)).unwrap_err();
    assert!(long.to_string().contains("overflowed"), "{long}");
    let int = add_numbers(&Value::I32(i32::MAX), &Value::I32(1)).unwrap_err();
    assert!(int.to_string().contains("overflowed"), "{int}");
    assert!(add_numbers(&Value::I64(i64::MIN), &Value::I32(-1)).is_err());
    assert!(compare(&Value::string(""), "empty", &Value::Null, "String").unwrap());
    assert!(compare(&Value::string("abc"), "matches", &Value::string("a.*"), "").unwrap());
    assert!(compare(&Value::string("abc"), "matches", &Value::string("a.*c"), "").unwrap());
    assert!(compare(&Value::string("abc"), "matches", &Value::string("(ab|a)c"), "").unwrap());
    assert!(compare(&Value::string("abc"), "matches", &Value::string("[a-c]+"), "").unwrap());
    assert!(compare(&Value::string("12"), "matches", &Value::string(r"\d+"), "").unwrap());
    assert!(!compare(&Value::string("abc"), "matches", &Value::string("a.*d"), "").unwrap());
    let one = Decimal::from_str("1.0").unwrap();
    let one_two = Decimal::from_str("1.00").unwrap();
    assert!(!java_equals(&Value::Decimal(one), &Value::Decimal(one_two)));
    assert_eq!(decimal_compare(one, one_two), Ordering::Equal);
}

#[test]
fn fractions_of_a_second_are_read_to_the_nanosecond() {
    for (text, nano) in [
        ("12:34:56.7", 700_000_000),
        ("12:34:56.789", 789_000_000),
        ("12:34:56.000001", 1_000),
        ("12:34:56.123456789", 123_456_789),
    ] {
        assert_eq!(
            parse_time(text).map(|time| time.subsec_nanosecond()),
            Some(nano),
            "{text}"
        );
    }
    assert_eq!(parse_time("12:34:56.1234567890"), None);
    let stamp = parse_timestamp("2026-10-20 12:34:56.789", &TimeZone::UTC).unwrap();
    assert_eq!(stamp.as_millisecond().rem_euclid(1000), 789);
}

#[test]
fn non_ascii_date_time_text_is_rejected() {
    for text in [
        "1é",
        "é",
        "1é:00",
        "12:é0",
        "12:0é",
        "12:00:0é",
        "１２:００",
        "10:30 ｐｍ",
    ] {
        assert_eq!(parse_time(text), None, "{text}");
    }
    for text in ["2024-01-0é", "é/01/2024", "2024-01-01 1é", "２０２４-01-01"] {
        assert_eq!(parse_date(text), None, "{text}");
    }
    for text in [
        "2024-01-01 1é",
        "2024-01-01 12:é0",
        "2024-01-01 12:00:0é",
        "2024-01-01T12:00:00+é:00",
        "2024-01-01 12:00:00.1é",
    ] {
        assert_eq!(parse_timestamp(text, &TimeZone::UTC), None, "{text}");
    }
}

#[test]
fn offsets_are_hhmm_or_hh_colon_mm() {
    assert_eq!(parse_offset("Z"), Some(0));
    assert_eq!(parse_offset("utc"), Some(0));
    assert_eq!(parse_offset("+0530"), Some(19_800));
    assert_eq!(parse_offset("-08:00"), Some(-28_800));
    for text in [
        "", "+", "0530", "+5:30", "+053", "+05:3", "+05300", "+05-30", "+é530", "+05é0",
    ] {
        assert_eq!(parse_offset(text), None, "{text}");
    }
}
