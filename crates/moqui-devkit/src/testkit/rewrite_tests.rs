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

fn conditions_of(source: &str) -> Rewritten {
    rewrite_spec(source).unwrap()
}

#[test]
fn comparisons_become_groovy_compare_with_value_slots() {
    let rewritten = conditions_of("feature(\"f\", {\n  then({ priceMap }) {\n    priceMap.price == 16.99\n  },\n})\n");
    assert_eq!(
        rewritten.source,
        "feature(\"f\", {\n  then$pa({ priceMap }) {\n    __pa(0, ($pa_v) => ($pa_v(0, __cmp(\"==\", $pa_v(1, $pa_v(2, priceMap).price), 16.99))))\n  },\n})\n"
    );
    let condition = &rewritten.conditions[0];
    assert_eq!(condition.text, "priceMap.price == 16.99");
    assert_eq!(condition.line, 3);
    assert!(!condition.call);
    assert_eq!(
        condition.slots,
        vec![
            (15, "priceMap.price == 16.99".to_string()),
            (9, "priceMap.price".to_string()),
            (0, "priceMap".to_string())
        ]
    );
}

#[test]
fn not_and_logical_use_groovy_truth_and_calls_keep_this() {
    let rewritten = conditions_of("feature('f', { expect: () => { !str.errorMessages && list.every(t => t) } })");
    assert_eq!(
        rewritten.source,
        "feature('f', { expect$pa: () => { __pa(0, ($pa_v) => ($pa_v(0, (__truthy($pa_v(1, !__truthy($pa_v(2, $pa_v(3, str).errorMessages)))) && __truthy($pa_v(4, $pa_v(5, list).every(t => t))))))) } })"
    );
    assert_eq!(rewritten.conditions[0].slots[0].0, 19);
}

#[test]
fn assignments_are_not_conditions_and_other_members_and_calls_are_left() {
    let source = "let a\nfeature('f', {\n  when() { x == 1 },\n  then: function () { a = 2; check(a) }\n})\nother({ then() { b == 1 } })\n";
    let rewritten = conditions_of(source);
    assert_eq!(rewritten.conditions.len(), 1);
    assert!(rewritten.conditions[0].call);
    assert!(rewritten.source.contains("when() { x == 1 }"));
    assert!(rewritten.source.contains("then$pa: function () { a = 2; __pa(0,"));
    assert!(rewritten.source.contains("other({ then() { b == 1 } })"));
    assert_eq!(rewritten.source.lines().count(), source.lines().count());
}

#[test]
fn concise_arrow_body_and_quoted_key_and_where_rows() {
    let rewritten = conditions_of("feature('f #a', { where: [{ a: 1 }], 'expect': ({ a }) => a < 2 })");
    assert_eq!(
        rewritten.source,
        "feature('f #a', { where: [{ a: 1 }], expect$pa: ({ a }) => __pa(0, ($pa_v) => ($pa_v(0, __cmp(\"<\", $pa_v(1, a), 2)))) })"
    );
}

#[test]
fn strict_equality_and_arithmetic_keep_their_operators() {
    let rewritten = conditions_of("feature('f', { then() { a.b + 1 === c[0] } })");
    assert_eq!(
        rewritten.source,
        "feature('f', { then$pa() { __pa(0, ($pa_v) => ($pa_v(0, $pa_v(1, $pa_v(2, $pa_v(3, a).b) + 1) === $pa_v(4, $pa_v(5, c)[0])))) } })"
    );
}

#[test]
fn depends_on_is_read_without_running() {
    let source =
        "const t = Date.now()\nspec({ dependsOn: ['A', \"B\"], stepwise: true })\nfeature('f', { expect() { 1 } })";
    assert_eq!(spec_depends_on(source), vec!["A".to_string(), "B".to_string()]);
    assert!(spec_offline(
        "spec({ server: false })\nfeature('f', { expect() { 1 } })"
    ));
    assert!(!spec_offline("spec({})\nfeature('f', { expect() { 1 } })"));
    assert!(!spec_offline("spec({ server: true })"));
    assert!(spec_database_free(
        "spec({ database: false })\nfeature('f', { expect() { 1 } })"
    ));
    assert!(!spec_database_free("spec({})\nfeature('f', { expect() { 1 } })"));
    assert!(!spec_database_free("spec({ database: true })"));
    assert!(spec_slow("spec({ slow: true })\nfeature('f', { expect() { 1 } })"));
    assert!(!spec_slow("spec({ slow: false })"));
    assert!(!spec_slow("spec({})\nfeature('f', { slow: true, expect() { 1 } })"));
    assert!(spec_depends_on("spec({})").is_empty());
    // a condition line that starts with a regular expression, which plain JavaScript reads as a division
    let regex_line =
        "spec({ dependsOn: ['A'] })\nfeature('f', { expect() {\n    !x.length\n    /^\\d+$/.test(y)\n} })\n";
    assert_eq!(spec_depends_on(regex_line), vec!["A".to_string()]);
    assert_eq!(
        spec_requires("spec({ requires: ['git'], dependsOn: ['A'] })"),
        vec!["git".to_string()]
    );
}

#[test]
fn suite_order_reads_the_names() {
    assert_eq!(
        suite_order("// the order of the suite\nsuite([\"A\", 'B'])\n").unwrap(),
        vec!["A".to_string(), "B".to_string()]
    );
    assert!(suite_order("suite([A])").is_err());
}

#[test]
fn suite_server_reads_the_declaration() {
    let source = "suite([\"A\", \"B\"], {\n    server: { components: [\"mr-mantle\", 'mr-marble'], specs: [\"Render\"], conf: \"conf/MoquiProductionConf.xml\" },\n})\n";
    assert_eq!(suite_order(source).unwrap(), vec!["A".to_string(), "B".to_string()]);
    assert_eq!(
        suite_server(source).unwrap(),
        Some(SuiteServer {
            components: vec!["mr-mantle".to_string(), "mr-marble".to_string()],
            specs: vec!["Render".to_string()],
            conf: Some("conf/MoquiProductionConf.xml".to_string()),
        })
    );
    assert_eq!(
        suite_server("suite(['A'], { server: {} })").unwrap(),
        Some(SuiteServer::default())
    );
    assert_eq!(suite_server("suite(['A'])").unwrap(), None);
    assert_eq!(suite_server("suite(['A'], { other: 1 })").unwrap(), None);
    for bad in [
        "suite(['A'], 'x')",
        "suite(['A'], { server: 'x' })",
        "suite(['A'], { server: { components: [name] } })",
        "suite(['A'], { server: { specs: 'S' } })",
        "suite(['A'], { server: { conf: 1 } })",
        "suite(['A'], { server: { port: 1 } })",
        "suite(['A'], { server: { ...more } })",
        "suite([",
    ] {
        assert!(suite_server(bad).is_err(), "{bad}");
    }
}

#[test]
fn unicode_in_regex_literals_strings_and_templates() {
    let source = "feature('f', {\n  then() {\n    /^Today · \\d+ — é$/.test(label)\n    label == `· ${n} ·`\n    x.match(/[·•]/u) != null && 'é·' == y\n  },\n})\n";
    let rewritten = conditions_of(source);
    assert_eq!(rewritten.conditions.len(), 3);
    assert_eq!(rewritten.conditions[0].text, "/^Today · \\d+ — é$/.test(label)");
    assert!(
        rewritten
            .source
            .contains("$pa_v(0, /^Today · \\d+ — é$/.test($pa_v(1, label)))"),
        "{}",
        rewritten.source
    );
    assert!(
        rewritten.source.contains("__cmp(\"==\", $pa_v(1, label), `· ${n} ·`)"),
        "{}",
        rewritten.source
    );
    assert_eq!(rewritten.conditions[2].text, "x.match(/[·•]/u) != null && 'é·' == y");
    assert_eq!(rewritten.source.lines().count(), source.lines().count());
}

#[test]
fn a_then_line_starting_with_a_regex_is_its_own_condition() {
    let rewritten = conditions_of("feature('f', {\n  then() {\n    a == 1\n    /Today · 3/.test(label)\n  },\n})\n");
    assert_eq!(rewritten.conditions.len(), 2);
    assert_eq!(rewritten.conditions[1].text, "/Today · 3/.test(label)");
    assert_eq!(rewritten.conditions[1].line, 4);
    // the column of `test` counts `·` as one character
    assert_eq!(rewritten.conditions[1].slots[0].0, 12);
}

#[test]
fn a_syntax_error_names_its_line() {
    let err = rewrite_spec("feature('f', {\n  then() { a == }\n})").unwrap_err();
    assert!(err.to_string().starts_with("line 2: "), "{err}");
}
