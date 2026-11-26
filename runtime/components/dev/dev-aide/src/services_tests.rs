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

#[test]
fn a_condition_is_tried_with_its_context_set() {
    let context = serde_json::from_value(json!({"amount": 5, "status": "A<B"})).unwrap();
    assert_eq!(
        condition_actions("amount > 3", &context),
        "<actions><set field=\"amount\" from=\"5\"/><set field=\"status\" from=\"&quot;A&lt;B&quot;\"/>\
         <if condition=\"amount &gt; 3\"><message>the condition holds</message><else><message>the condition does not hold</message></else></if></actions>"
            .replace("&gt;", ">")
    );
    assert!(condition_actions("<compare field=\"a\" value=\"1\"/>", &serde_json::Map::new())
        .contains("<if><condition><compare field=\"a\" value=\"1\"/></condition><then><message>the condition holds</message></then><else>"));
}

#[test]
fn a_spec_is_named_by_its_services_file_or_entity() {
    assert_eq!(
        spec_name("moqui.example.ExampleServices.create#Example"),
        "ExampleServices"
    );
    assert_eq!(spec_name("create#moqui.example.Example"), "Example");
}

#[test]
fn a_try_becomes_a_feature_with_its_rows() {
    let given = serde_json::from_value(json!({"exampleName": "A `b`"})).unwrap();
    let out = serde_json::from_value(json!({"exampleId": "100000", "list": [1]})).unwrap();
    let rows = vec![
        json!({"entity": "moqui.example.Example", "action": "create", "fields": [
            {"field": "exampleId", "after": "100000"}, {"field": "exampleName", "after": "A ${x}"}, {"field": "secret", "after": "(hidden)"}]}),
        json!({"entity": "moqui.example.Example", "action": "delete", "fields": []}),
    ];
    let feature = spec_feature("create#moqui.example.Example", &given, &out, &rows);
    assert!(
        feature.contains("return call(\"create#moqui.example.Example\", {\"exampleName\":\"A `b`\"})"),
        "{feature}"
    );
    assert!(feature.contains("out[\"exampleId\"] == \"100000\""), "{feature}");
    assert!(!feature.contains("out[\"list\"]"), "{feature}");
    assert!(
        feature.contains("<moqui.example.Example exampleId=\"100000\" exampleName=\"A \\${x}\"/>"),
        "{feature}"
    );
    assert!(!feature.contains("secret"), "{feature}");
}

const RESULT: &str = "<script><![CDATA[if (typeof result !== 'undefined') ec.message.addMessage('result: ' + JSON.stringify(result))]]></script>";

#[test]
fn a_snippet_sets_its_parameters_then_runs() {
    let given = serde_json::from_value(json!({"a": "1 < 2", "bad name": "x"})).unwrap();
    assert_eq!(
        snippet_actions("xml", "<actions><set field=\"result\" from=\"a\"/></actions>", &given),
        "<actions><set field=\"a\" value=\"1 &lt; 2\"/><set field=\"result\" from=\"a\"/>".to_string()
            + RESULT
            + "</actions>"
    );
    assert_eq!(
        snippet_actions("javascript", "result = 1", &serde_json::Map::new()),
        "<actions><script><![CDATA[result = 1]]></script>".to_string() + RESULT + "</actions>"
    );
}

fn context(pairs: &[(&str, Value)]) -> moqui_lib::object::ValueMap {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

fn filter(
    ref_pattern: Option<&str>,
    ref_type: Option<&str>,
    default_branch_only: bool,
    source: &str,
) -> GitRuleFilter {
    GitRuleFilter {
        ref_pattern: ref_pattern.map(|pattern| moqui_lib::object::CompiledRegex::new(pattern).unwrap()),
        ref_type: ref_type.map(str::to_string),
        default_branch_only,
        sources: sources_of(source),
    }
}

#[test]
fn a_context_is_taken_as_a_git_rule_takes_its_event() {
    let main = context(&[
        ("source", Value::string("push")),
        ("refName", Value::string("refs/heads/main")),
        ("isDefaultBranch", Value::Bool(true)),
    ]);
    let tag = context(&[
        ("source", Value::string("push")),
        ("refName", Value::string("refs/tags/v1")),
        ("isDefaultBranch", Value::Bool(false)),
    ]);
    let after = context(&[("source", Value::string("push"))]);
    let write = context(&[
        ("source", Value::string("write")),
        ("refName", Value::string("refs/heads/main")),
    ]);
    // no narrowing: every push, fetch and import, not a write unless asked
    assert!(filter(None, None, false, "").takes(&main) && filter(None, None, false, "").takes(&after));
    assert!(!filter(None, None, false, "").takes(&write));
    assert!(filter(None, None, false, "push, write").takes(&write));
    // the ref pattern, by the whole name, and never an event of no ref
    assert!(filter(Some("refs/heads/.*"), None, false, "").takes(&main));
    assert!(!filter(Some("refs/heads/.*"), None, false, "").takes(&tag));
    assert!(!filter(Some("refs/heads"), None, false, "").takes(&main));
    assert!(!filter(Some(".*"), None, false, "").takes(&after));
    // the ref type and the default branch
    assert!(filter(None, Some("tag"), false, "").takes(&tag));
    assert!(!filter(None, Some("branch"), false, "").takes(&tag));
    assert!(filter(None, Some("other"), false, "").takes(&after));
    assert!(filter(None, None, true, "").takes(&main));
    assert!(!filter(None, None, true, "").takes(&tag));
    assert!(!filter(None, None, true, "").takes(&after));
}
