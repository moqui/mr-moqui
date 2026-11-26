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

use serde_json::Value as Json;

use super::traced;
use crate::context::ExecutionContextFactory;
use crate::script::runtime::clear_context_stack;
use crate::script::{generated_source, generated_source_with, run_actions};

fn testdata() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata")
}

/// Untraced, the generator writes what it always wrote: each golden of the actions goldens byte for byte, with or
/// without the debug comments; traced, the same statements between their trace calls.
#[test]
fn tracing_off_generates_the_script_of_today_and_on_adds_only_the_trace_calls() {
    for name in [
        "condition",
        "control",
        "entity-find",
        "entity-value",
        "message",
        "script",
        "service-call",
        "set",
    ] {
        let xml = std::fs::read_to_string(testdata().join(format!("{name}.xml"))).unwrap();
        let golden = std::fs::read_to_string(testdata().join(format!("{name}.js"))).unwrap();
        assert_eq!(generated_source_with(&xml, false, false).unwrap(), golden, "{name}");
        assert_eq!(
            generated_source_with(&xml, true, false).unwrap(),
            generated_source(&xml, true).unwrap(),
            "{name}"
        );
        let traced = generated_source_with(&xml, false, true).unwrap();
        assert!(traced.contains("__moquiTrace(0, \"0\");"), "{name}: {traced}");
        let untraced: String = traced
            .lines()
            .filter(|line| !line.trim_start().starts_with("__moquiTrace("))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(untraced, golden, "{name}: the traced script less its trace calls");
    }
}

/// `testdata/control.traced.js` is the traced script of `control.xml`; `UPDATE_GOLDEN=1` writes it.
#[test]
fn a_traced_script_counts_each_statement_by_its_path_and_reads_what_it_set() {
    let xml = std::fs::read_to_string(testdata().join("control.xml")).unwrap();
    let traced = generated_source_with(&xml, false, true).unwrap();
    let path = testdata().join("control.traced.js");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &traced).unwrap();
        return;
    }
    assert_eq!(traced, std::fs::read_to_string(&path).unwrap());
}

fn element<'a>(trace: &'a Json, path: &str) -> &'a Json {
    &trace["blocks"][0]["elements"][path]
}

#[test]
fn a_traced_run_answers_each_elements_count_values_and_the_one_that_failed() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let actions = r#"<actions>
        <set field="total" from="0"/>
        <iterate list="[1, 2, 3]" entry="n">
            <set field="total" from="total + n"/>
        </iterate>
        <if condition="total == 7"><message>never</message></if>
        <set field="password" value="not to be shown"/>
        <set field="order" from="[apiKey: 'k-1', name: 'A']"/>
        <script>throw new Error('boom here')</script>
        <set field="after" value="never"/>
    </actions>"#
        .replace("[apiKey: 'k-1', name: 'A']", "({apiKey: 'k-1', name: 'A'})");
    let (ran, trace) = traced(|| run_actions(&actions));
    let failure = ran.unwrap_err();
    assert!(failure.message().contains("boom here"), "{failure:?}");
    assert_eq!(trace["blocks"].as_array().unwrap().len(), 1, "{trace}");
    assert_eq!(element(&trace, "0")["count"], 1, "{trace}");
    assert_eq!(element(&trace, "0")["values"]["total"], "0", "{trace}");
    assert_eq!(element(&trace, "1")["count"], 1, "{trace}");
    assert_eq!(element(&trace, "1.0")["count"], 3, "{trace}");
    assert_eq!(element(&trace, "1.0")["values"]["total"], "6", "{trace}");
    assert_eq!(element(&trace, "2")["count"], 1, "{trace}");
    assert!(element(&trace, "2.0").is_null(), "the message never ran: {trace}");
    assert_eq!(element(&trace, "3")["values"]["password"], "(hidden)", "{trace}");
    let order = element(&trace, "4")["values"]["order"].as_str().unwrap();
    assert!(order.contains("\"name\":\"A\"") && !order.contains("k-1"), "{order}");
    assert!(
        element(&trace, "5")["failed"].as_str().unwrap().contains("boom here"),
        "{trace}"
    );
    assert!(element(&trace, "6").is_null(), "nothing after the failure ran: {trace}");
    assert_eq!(trace["failed"]["element"], "5", "{trace}");
    // no trace holds the thread after it: the same text runs untraced, its cell the script it always has
    let (_, outer) = traced(|| ());
    assert!(outer["blocks"].as_array().unwrap().is_empty());
    assert!(!super::active());
    clear_context_stack();
}

/// An element that adds an error is the one marked, not the block around it; a break and a return leave their loop
/// and actions without leaving an element open.
#[test]
fn an_error_added_marks_its_element_and_a_break_or_return_closes_what_it_left() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let actions = r#"<actions>
        <iterate list="[1, 2, 3]" entry="n">
            <if condition="n == 2"><break/></if>
            <set field="last" from="n"/>
        </iterate>
        <if condition="true"><message error="true">it went wrong</message></if>
        <return/>
        <set field="never" value="x"/>
    </actions>"#;
    let (ran, trace) = traced(|| run_actions(actions));
    ran.unwrap();
    assert_eq!(element(&trace, "0.0")["count"], 2, "{trace}");
    assert_eq!(element(&trace, "0.0.0")["count"], 1, "{trace}");
    assert_eq!(element(&trace, "0.1")["count"], 1, "{trace}");
    assert_eq!(element(&trace, "0.1")["values"]["last"], "1", "{trace}");
    assert_eq!(element(&trace, "1.0")["failed"], "it went wrong", "{trace}");
    assert!(
        element(&trace, "1")["failed"].is_null(),
        "the if around it is not marked: {trace}"
    );
    assert_eq!(element(&trace, "2")["count"], 1, "{trace}");
    assert!(element(&trace, "3").is_null(), "{trace}");
    assert_eq!(trace["failed"]["element"], "1.0", "{trace}");
    context.message().clear();
    clear_context_stack();
}
