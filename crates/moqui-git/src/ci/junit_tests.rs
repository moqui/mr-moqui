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

use super::parse;

#[test]
fn failed_tests_are_read_with_their_files_and_lines() {
    let report = parse(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites>
  <testsuite name="comp-a.OrderFlow" tests="3" failures="1" errors="1" skipped="0">
    <testcase name="places an order" classname="comp-a.OrderFlow" time="0.120"/>
    <testcase name="ships it" classname="comp-a.OrderFlow" time="1.5">
      <failure message="Condition not satisfied:" type="assertion">[assertion in then at test/OrderFlow.spec.js:41]
expected 2 but was 3</failure>
    </testcase>
    <testcase name="bills it" file="test/Billing.spec.js" line="7"><error message="boom"/></testcase>
  </testsuite>
  <testsuite name="other"><testcase name="fine"/></testsuite>
</testsuites>"#,
    )
    .unwrap();
    assert_eq!(report.tests, 4);
    assert_eq!(report.failures.len(), 2);
    let ships = &report.failures[0];
    assert_eq!(
        (ships.suite.as_str(), ships.test.as_str()),
        ("comp-a.OrderFlow", "ships it")
    );
    // no file attribute: the first path:line of the failure's text
    assert_eq!(
        (ships.file.as_deref(), ships.line),
        (Some("test/OrderFlow.spec.js"), Some(41))
    );
    assert!(
        ships.message.starts_with("Condition not satisfied:"),
        "{}",
        ships.message
    );
    assert!(ships.message.contains("expected 2 but was 3"), "{}", ships.message);
    assert_eq!(ships.duration_millis, Some(1500));
    let bills = &report.failures[1];
    assert_eq!(
        (bills.file.as_deref(), bills.line),
        (Some("test/Billing.spec.js"), Some(7))
    );
    assert_eq!(bills.message, "boom");
    // a single suite as the root, and what is not a report
    assert_eq!(
        parse(r#"<testsuite name="s"><testcase name="t"/></testsuite>"#)
            .unwrap()
            .tests,
        1
    );
    assert!(parse("<html/>")
        .unwrap_err()
        .contains("not <testsuites> or <testsuite>"));
    assert!(parse("not xml").is_err());
}
