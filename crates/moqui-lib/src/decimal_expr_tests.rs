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

use std::collections::BTreeMap;
use std::str::FromStr;

use rust_decimal::Decimal;

use super::*;

fn d(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

fn inputs(pairs: &[(&str, &str)]) -> BTreeMap<String, Decimal> {
    pairs.iter().map(|(name, value)| (name.to_string(), d(value))).collect()
}

fn eval(text: &str, pairs: &[(&str, &str)]) -> Decimal {
    evaluate(text, &inputs(pairs)).unwrap_or_else(|error| panic!("{text}: {error}"))
}

fn code(text: &str, pairs: &[(&str, &str)]) -> &'static str {
    match evaluate(text, &inputs(pairs)) {
        Ok(value) => panic!("{text} gave {value}, not a refusal"),
        Err(error) => error.code,
    }
}

#[test]
fn arithmetic_is_exact_decimal() {
    assert_eq!(eval("0.1 + 0.2", &[]), d("0.3"));
    assert_eq!(eval("0.1 + 0.2", &[]).to_string(), "0.3");
    let annual = eval("salary * 12", &[("salary", "1000.10")]);
    assert_eq!(annual.to_string(), "12001.20");
    assert_eq!(eval("1 - 0.0", &[]).to_string(), "1.0");
    assert_eq!(eval("10 / 4", &[]), d("2.5"));
    // division keeps the decimal type's full precision, rounded by nothing
    assert_eq!(eval("1 / 3", &[]).to_string(), "0.3333333333333333333333333333");
    assert_eq!(eval("1 / 3 * 3", &[]), d("0.9999999999999999999999999999"));
}

#[test]
fn precedence_and_unary_minus() {
    assert_eq!(eval("2 + 3 * 4", &[]), d("14"));
    assert_eq!(eval("(2 + 3) * 4", &[]), d("20"));
    assert_eq!(eval("10 - 4 - 3", &[]), d("3"));
    assert_eq!(eval("24 / 4 / 2", &[]), d("3"));
    assert_eq!(eval("-2 * 3", &[]), d("-6"));
    assert_eq!(eval("2 * -3", &[]), d("-6"));
    assert_eq!(eval("- -4", &[]), d("4"));
    assert_eq!(
        eval("-(hours - 40) * rate", &[("hours", "45"), ("rate", "20.50")]),
        d("-102.50")
    );
}

#[test]
fn functions() {
    let pay = [("hours", "45"), ("rate", "20")];
    assert_eq!(eval("if(hours > 40, (hours - 40) * rate * 1.5, 0)", &pay), d("150"));
    assert_eq!(eval("if(hours <= 40, 1, 2)", &pay), d("2"));
    assert_eq!(eval("min(3, 1, 2)", &[]), d("1"));
    assert_eq!(eval("max(3, 1, 7.5)", &[]), d("7.5"));
    assert_eq!(eval("abs(-4.25)", &[]), d("4.25"));
    assert_eq!(eval("round(2.345, 2)", &[]).to_string(), "2.35");
    assert_eq!(eval("round(-2.345, 2)", &[]).to_string(), "-2.35");
    assert_eq!(eval("round(-2.5, 0)", &[]).to_string(), "-3");
    assert_eq!(eval("round(5, 2)", &[]).to_string(), "5.00");
    assert_eq!(eval("floor(2.349, 2)", &[]).to_string(), "2.34");
    assert_eq!(eval("floor(-2.341, 2)", &[]).to_string(), "-2.35");
    assert_eq!(eval("ceil(2.341, 2)", &[]).to_string(), "2.35");
    assert_eq!(eval("ceil(-2.349, 2)", &[]).to_string(), "-2.34");
    assert_eq!(
        eval("round(x, places)", &[("x", "1.005"), ("places", "2")]).to_string(),
        "1.01"
    );
}

#[test]
fn conditions() {
    let values = [("a", "5"), ("b", "0")];
    assert_eq!(eval("if(a > 1 and a < 10, 1, 0)", &values), d("1"));
    assert_eq!(eval("if(a < 1 or a == 5, 1, 0)", &values), d("1"));
    assert_eq!(eval("if(not a != 5, 1, 0)", &values), d("1"));
    assert_eq!(eval("if(a >= 5 and not (b > 0), 1, 0)", &values), d("1"));
    // only the branch taken, and only the operands that decide, are evaluated
    assert_eq!(eval("if(b == 0, 0, a / b)", &values), d("0"));
    assert_eq!(eval("if(b != 0 and a / b > 1, 1, 2)", &values), d("2"));
    assert_eq!(eval("if(b == 0 or a / b > 1, 1, 2)", &values), d("1"));
    // 1.0 and 1.00 are equal
    assert_eq!(eval("if(1.0 == 1.00, 1, 0)", &[]), d("1"));
}

#[test]
fn names_and_functions_listed() {
    let expression = parse("round(max(rate, minimum) * hours + rate, 2)", None).unwrap();
    assert_eq!(expression.names(), ["hours", "minimum", "rate"]);
    assert_eq!(expression.functions(), ["max", "round"]);
    let missing = expression.missing_names(&inputs(&[("rate", "1")]));
    assert_eq!(missing, ["hours", "minimum"]);
    let allowed = vec!["rate".to_string(), "hours".to_string()];
    assert!(parse("rate * hours", Some(&allowed)).is_ok());
    let error = parse("rate * bonus", Some(&allowed)).unwrap_err();
    assert_eq!((error.code, error.position), ("UNKNOWN_NAME", Some(8)));
}

#[test]
fn refusals() {
    assert_eq!(code("", &[]), "EXPRESSION_REQUIRED");
    assert_eq!(code("   ", &[]), "EXPRESSION_REQUIRED");
    assert_eq!(code(&"1+".repeat(1001), &[]), "EXPRESSION_TOO_LONG");
    assert_eq!(
        code(&format!("{}1{}", "(".repeat(40), ")".repeat(40)), &[]),
        "EXPRESSION_TOO_DEEP"
    );
    assert_eq!(code(&format!("{}1", "-".repeat(40)), &[]), "EXPRESSION_TOO_DEEP");
    assert_eq!(code("a = 1", &[]), "SYNTAX_ERROR");
    assert_eq!(code("a; b", &[]), "SYNTAX_ERROR");
    assert_eq!(code("ec.user", &[]), "SYNTAX_ERROR");
    assert_eq!(code("\"text\"", &[]), "SYNTAX_ERROR");
    assert_eq!(code("a[0]", &[]), "SYNTAX_ERROR");
    assert_eq!(code("1 +", &[]), "SYNTAX_ERROR");
    assert_eq!(code("(1", &[]), "SYNTAX_ERROR");
    assert_eq!(code("1 2", &[]), "SYNTAX_ERROR");
    assert_eq!(code("a && b", &[]), "SYNTAX_ERROR");
    assert_eq!(code("1 < 2 < 3", &[]), "SYNTAX_ERROR");
    assert_eq!(code("and + 1", &[]), "SYNTAX_ERROR");
    assert_eq!(code(".5", &[]), "NUMBER_INVALID");
    assert_eq!(code("1.", &[]), "NUMBER_INVALID");
    assert_eq!(code("1e5", &[]), "NUMBER_INVALID");
    assert_eq!(code("1.2.3", &[]), "NUMBER_INVALID");
    assert_eq!(code("12345678901234567890123456789", &[]), "NUMBER_INVALID");
    assert_eq!(code("ec(1)", &[]), "UNKNOWN_FUNCTION");
    assert_eq!(code("eval(1)", &[]), "UNKNOWN_FUNCTION");
    assert_eq!(code("abs(1, 2)", &[]), "ARGUMENT_COUNT");
    assert_eq!(code("min(1)", &[]), "ARGUMENT_COUNT");
    assert_eq!(code("if(1 > 0, 1)", &[]), "ARGUMENT_COUNT");
    assert_eq!(code("1 > 0", &[]), "TYPE_MISMATCH");
    assert_eq!(code("if(1, 2, 3)", &[]), "TYPE_MISMATCH");
    assert_eq!(code("1 + (2 > 1)", &[]), "TYPE_MISMATCH");
    assert_eq!(code("if(not 1, 2, 3)", &[]), "TYPE_MISMATCH");
    assert_eq!(code("if(1 and 1 > 0, 2, 3)", &[]), "TYPE_MISMATCH");
    assert_eq!(code("a + 1", &[]), "MISSING_INPUT");
    assert_eq!(code("a / b", &[("a", "1"), ("b", "0")]), "DIVIDE_BY_ZERO");
    assert_eq!(code("79228162514264337593543950335 * 2", &[]), "NUMBER_INVALID");
    assert_eq!(code("a * 2", &[("a", "79228162514264337593543950335")]), "OVERFLOW");
    assert_eq!(code("round(1, 1.5)", &[]), "ROUNDING_PLACES_INVALID");
    assert_eq!(code("round(1, 29)", &[]), "ROUNDING_PLACES_INVALID");
    assert_eq!(code("round(1, places)", &[("places", "-1")]), "ROUNDING_PLACES_INVALID");
}

#[test]
fn refusals_name_their_position() {
    let error = parse("a + ec.user", None).unwrap_err();
    assert_eq!((error.code, error.position), ("SYNTAX_ERROR", Some(7)));
    let error = parse("1 + foo(2)", None).unwrap_err();
    assert_eq!((error.code, error.position), ("UNKNOWN_FUNCTION", Some(5)));
    let error = parse("1 +", None).unwrap_err();
    assert_eq!((error.code, error.position), ("SYNTAX_ERROR", Some(4)));
    assert!(!error.message.is_empty());
}

#[test]
fn a_long_flat_formula_is_not_deep() {
    let text = vec!["1"; 900].join("+");
    assert_eq!(eval(&text, &[]), d("900"));
}

#[test]
fn inputs_as_text() {
    assert_eq!(decimal_input(" 12.50 "), Some(d("12.50")));
    assert_eq!(decimal_input("-3"), Some(d("-3")));
    assert_eq!(decimal_input("1e5"), None);
    assert_eq!(decimal_input("abc"), None);
    assert_eq!(decimal_input(""), None);
}

#[test]
fn nesting_of_every_kind_is_bounded() {
    let nested_if = |levels: usize| format!("{}1{}", "if(1 > 0, ".repeat(levels), ", 0)".repeat(levels));
    assert_eq!(eval(&nested_if(31), &[]), d("1"));
    assert_eq!(code(&nested_if(33), &[]), "EXPRESSION_TOO_DEEP");
    assert_eq!(
        code(&format!("if({}1 > 0, 1, 0)", "not ".repeat(40)), &[]),
        "EXPRESSION_TOO_DEEP"
    );
    assert_eq!(
        code(&format!("{}1{}", "abs(".repeat(40), ")".repeat(40)), &[]),
        "EXPRESSION_TOO_DEEP"
    );
    // a long run of one operator is a flat chain, not a deep one, and stops at the type's range
    let product = vec!["a"; 600].join("*");
    assert_eq!(code(&product, &[("a", "1.5")]), "OVERFLOW");
}

#[test]
fn numbers_stay_in_the_decimal_type() {
    // leading zeros are not significant digits; a trailing zero is
    assert_eq!(eval(&format!("{}1", "0".repeat(1990)), &[]), d("1"));
    assert_eq!(code("1.0000000000000000000000000000", &[]), "NUMBER_INVALID");
    assert_eq!(code(&format!("0.{}1", "0".repeat(40)), &[]), "NUMBER_INVALID");
    assert_eq!(code("round(1, 99999999999999999999)", &[]), "ROUNDING_PLACES_INVALID");
    assert_eq!(eval("round(1.234, 2.0)", &[]).to_string(), "1.23");
    assert_eq!(
        code("79228162514264337593543950 / 0.0000000000000000000000000001", &[]),
        "OVERFLOW"
    );
    // a product past 28 places rounds to the type, as division does
    assert_eq!(
        eval("0.00000000000001 * 0.00000000000001 * 0.00000000000001", &[]),
        d("0")
    );
    assert_eq!(decimal_input(&"9".repeat(40)), None);
    assert_eq!(decimal_input("+-1"), None);
}

#[test]
fn zero_has_no_sign() {
    assert_eq!(eval("-0", &[]).to_string(), "0");
    assert_eq!(eval("-a", &[("a", "0.00")]).to_string(), "0.00");
    assert_eq!(eval("a * -1", &[("a", "0.00")]).to_string(), "0.00");
    assert_eq!(eval("round(-0.001, 2)", &[]).to_string(), "0.00");
}
