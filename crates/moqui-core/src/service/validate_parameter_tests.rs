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

fn param(name: &str, java_type: &str, required: Required) -> ParamDef {
    ParamDef {
        name: name.to_string(),
        java_type: java_type.to_string(),
        required,
        default_value: None,
        default_expr: None,
        allow_html: AllowHtml::None,
        checks: Vec::new(),
        auto: None,
        entity_name: None,
        field_name: None,
        format: None,
        description: None,
        children: Vec::new(),
        given: BTreeSet::new(),
    }
}

/// Java's `nestedParameterClean`: a nested parameter is converted and checked, named `parent.child` in an error,
/// and, validating, an undeclared entry is dropped.
#[test]
fn nested_parameters_are_cleaned_and_checked() {
    let mut parent = param("address", "Map", Required::False);
    parent.children = vec![
        param("zip", "Integer", Required::True),
        param("city", "String", Required::False),
    ];
    let input = Value::Map(
        [
            ("zip".to_string(), Value::string("12345")),
            ("city".to_string(), Value::string("Town")),
            ("extra".to_string(), Value::string("x")),
        ]
        .into_iter()
        .collect(),
    );
    let mut failed = Vec::new();
    let Value::Map(out) = nested(&parent, input.clone(), "address", &mut failed, true) else {
        panic!("a map");
    };
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(out.get("zip"), Some(&Value::I32(12345)));
    assert!(!out.contains_key("extra"));
    let Value::Map(kept) = nested(&parent, input, "address", &mut failed, false) else {
        panic!("a map");
    };
    assert!(kept.contains_key("extra"));
    let missing = Value::Map([("city".to_string(), Value::string("Town"))].into_iter().collect());
    nested(&parent, missing, "address", &mut failed, true);
    assert_eq!(
        failed,
        [("address.zip".to_string(), "Field cannot be empty".to_string())]
    );
}

/// An empty value with no default is null for a parameter of another type, so a blank price is no manual price;
/// a String keeps its empty value.
#[test]
fn an_empty_value_for_a_typed_parameter_is_null() {
    let mut scope = DefaultScope::default();
    let params = BTreeMap::from([
        ("unitAmount".to_string(), Value::string("")),
        ("comments".to_string(), Value::string("")),
        ("items".to_string(), Value::List(Vec::new())),
    ]);
    let amount = clean_param(
        &param("unitAmount", "BigDecimal", Required::False),
        &params,
        &mut scope,
        true,
    )
    .unwrap();
    assert!(matches!(amount, Some(Value::Null)), "{amount:?}");
    let comments = clean_param(&param("comments", "String", Required::False), &params, &mut scope, true).unwrap();
    assert!(comments.is_none(), "{comments:?}");
    let items = clean_param(&param("items", "List", Required::False), &params, &mut scope, true).unwrap();
    assert!(items.is_none(), "{items:?}");
    let mut defaulted = param("unitAmount", "BigDecimal", Required::False);
    defaulted.default_value = Some("1".to_string());
    let one = clean_param(&defaulted, &params, &mut scope, true).unwrap();
    assert!(matches!(one, Some(ref value) if !value.is_null()), "{one:?}");
}

/// `val-or` passes when one check passes and otherwise has each check's message; `text-url` takes any scheme.
#[test]
fn val_or_keeps_the_child_messages_and_a_url_takes_any_scheme() {
    let either = Check::Or(vec![Check::TextDigits, Check::TextLetters]);
    assert!(apply_check(&either, &Value::string("abc")).is_empty());
    assert_eq!(
        apply_check(&either, &Value::string("a1!")),
        [
            "Value [a1!] must have only digits.",
            "Value entered (a1!) must have only letters."
        ]
    );
    assert!(apply_check(&Check::Or(Vec::new()), &Value::string("x")).is_empty());
    for url in [
        "https://moqui.org/x",
        "ftp://host/file",
        "s3://bucket/key",
        "file:///tmp/x",
    ] {
        assert!(apply_check(&Check::TextUrl, &Value::string(url)).is_empty(), "{url}");
    }
    for url in ["moqui.org", "https://", "not a url"] {
        assert!(!apply_check(&Check::TextUrl, &Value::string(url)).is_empty(), "{url}");
    }
}

/// Java's `convertType`: a timestamp's text, by the parameter's `format` or not, is read in the session time zone
/// unless it has an offset, so a date picker's "due today" is the user's midnight, not UTC's (which is the evening
/// before in America/Chicago).
#[test]
fn a_timestamp_is_read_in_the_session_time_zone() {
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    crate::user::session::set_time_zone(Some("America/Chicago"));
    let stamp = |text: &str| Value::Timestamp(text.parse::<jiff::Timestamp>().unwrap());
    let mut due = param("dueDate", "Timestamp", Required::False);
    due.format = Some("yyyy-MM-dd".to_string());
    assert_eq!(
        convert_text(&due, &Value::string("2026-10-19")),
        Some(stamp("2026-10-19T05:00:00Z"))
    );
    // text the format does not read takes the usual forms, in the same zone
    assert_eq!(
        convert_text(&due, &Value::string("2026-10-19 08:30")),
        Some(stamp("2026-10-19T13:30:00Z"))
    );
    let plain = param("fromDate", "Timestamp", Required::False);
    assert_eq!(
        convert_text(&plain, &Value::string("2026-01-05 00:00:00.000")),
        Some(stamp("2026-01-05T06:00:00Z")),
        "standard time"
    );
    assert_eq!(
        convert_text(&plain, &Value::string("2026-10-19")),
        Some(stamp("2026-10-19T05:00:00Z"))
    );
    // an offset and epoch millis are what they say
    assert_eq!(
        convert_text(&plain, &Value::string("2026-10-19T10:00:00Z")),
        Some(stamp("2026-10-19T10:00:00Z"))
    );
    assert_eq!(
        convert_text(&plain, &Value::string("1791979200000")),
        Some(stamp("2026-10-14T12:00:00Z"))
    );
    crate::user::session::set_time_zone(Some("UTC"));
    assert_eq!(
        convert_text(&due, &Value::string("2026-10-19")),
        Some(stamp("2026-10-19T00:00:00Z"))
    );
}

/// Java's `convertType`: a date by the parameter's `format`, a number as the locale writes it or by its `format`.
#[test]
fn text_converts_by_the_parameter_format_and_the_locale() {
    let mut date = param("when", "Date", Required::False);
    date.format = Some("dd.MM.yyyy".to_string());
    assert_eq!(
        convert_text(&date, &Value::string("15.01.2024")),
        Some(Value::Date(jiff::civil::date(2024, 1, 15)))
    );
    let mut amount = param("amount", "BigDecimal", Required::False);
    assert_eq!(
        convert_text(&amount, &Value::string("1,234.50")),
        Some(Value::Decimal("1234.50".parse().unwrap()))
    );
    amount.format = Some("0.00%".to_string());
    assert_eq!(
        convert_text(&amount, &Value::string("12.50%")),
        Some(Value::Decimal("0.1250".parse().unwrap()))
    );
    // text the format does not read is Java's error, which names the format
    let mut scope = DefaultScope::default();
    let params = BTreeMap::from([("amount".to_string(), Value::string("12.50"))]);
    assert_eq!(
        clean_param(&amount, &params, &mut scope, true),
        Err(vec![
            "Value entered (12.50) could not be converted to a BigDecimal using format [0.00%]".to_string()
        ])
    );
    let plain = param("count", "Integer", Required::False);
    let params = BTreeMap::from([("count".to_string(), Value::string("many"))]);
    assert_eq!(
        clean_param(&plain, &params, &mut scope, true),
        Err(vec![
            "Value entered (many) could not be converted to a Integer".to_string()
        ])
    );
}

/// Java's `validate="false"` skips the required, HTML and declared checks, and still converts.
#[test]
fn validate_false_converts_but_does_not_check() {
    let mut code = param("code", "Integer", Required::True);
    code.checks = vec![Check::NumberRange {
        min: Some("10".to_string()),
        min_eq: true,
        max: None,
        max_eq: false,
        message: None,
    }];
    let mut scope = DefaultScope::default();
    let empty = BTreeMap::new();
    assert_eq!(
        clean_param(&code, &empty, &mut scope, true),
        Err(vec!["Field cannot be empty".to_string()])
    );
    assert_eq!(clean_param(&code, &empty, &mut scope, false), Ok(None));
    let low = BTreeMap::from([("code".to_string(), Value::string("5"))]);
    assert_eq!(
        clean_param(&code, &low, &mut scope, true),
        Err(vec![
            "Value entered (5) is less than 10 and must be greater than or equal to.".to_string()
        ])
    );
    assert_eq!(clean_param(&code, &low, &mut scope, false), Ok(Some(Value::I32(5))));
    let html = param("note", "String", Required::False);
    let markup = BTreeMap::from([("note".to_string(), Value::string("<b>x</b>"))]);
    assert!(clean_param(&html, &markup, &mut scope, true).is_err());
    assert_eq!(
        clean_param(&html, &markup, &mut scope, false),
        Ok(Some(Value::string("<b>x</b>")))
    );
}

/// As Java, each check of a parameter that fails is a message of its own, and an HTML error does not stop them.
#[test]
fn every_failed_check_has_its_message() {
    let mut code = param("code", "String", Required::False);
    code.checks = vec![
        Check::TextLength {
            min: Some(5),
            max: None,
        },
        Check::TextDigits,
        Check::Matches {
            regexp: "[a-z<>]+".to_string(),
            message: "Use ${regexp} for ${pv}".to_string(),
        },
    ];
    let mut scope = DefaultScope::default();
    let params = BTreeMap::from([("code".to_string(), Value::string("A<"))]);
    assert_eq!(
        clean_param(&code, &params, &mut scope, true),
        Err(vec![
            "HTML not allowed including less-than (<), greater-than (>), etc symbols".to_string(),
            "Value entered (A<), length 2, is shorter than 5 characters.".to_string(),
            "Value [A<] must have only digits.".to_string(),
            "Use [a-z<>]+ for A<".to_string(),
        ])
    );
}

/// Java's `EmailValidator`, `Character.isLetter` and `Character.isDigit`.
#[test]
fn email_letters_and_digits_follow_java() {
    for email in [
        "user@example.com",
        "first.last+tag@sub.example.org",
        "o'brien@example.ie",
        "\"quoted user\"@example.com",
        "user@[192.168.0.1]",
    ] {
        assert!(is_email(email), "{email}");
    }
    for email in [
        "user@localhost",
        "user@example.c",
        "user@-example.com",
        "user.@example.com",
        ".user@example.com",
        "us er@example.com",
        "user@example.com.",
        "user@exam_ple.com",
        "@example.com",
        "user@",
    ] {
        assert!(!is_email(email), "{email}");
    }
    assert!(apply_check(&Check::TextLetters, &Value::string("Ærøskøbing")).is_empty());
    assert!(!apply_check(&Check::TextLetters, &Value::string("abc1")).is_empty());
    // Arabic-Indic and fullwidth digits are digits, a superscript two is not
    assert!(apply_check(&Check::TextDigits, &Value::string("٣٤５")).is_empty());
    assert!(!apply_check(&Check::TextDigits, &Value::string("1²")).is_empty());
}
