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

use super::{generate, generate_apart, generate_with, run_actions};
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::entity::{EntityFacade, EntityValue};
use crate::script::runtime::{clear_context_stack, eval_condition, set_context_name};

const MACROS: &[&str] = &[
    "actions",
    "and",
    "assert",
    "break",
    "check-errors",
    "compare",
    "condition",
    "continue",
    "date-filter",
    "default-parameters",
    "econdition",
    "econdition-object",
    "econditions",
    "else",
    "else-if",
    "entity-create",
    "entity-data",
    "entity-delete",
    "entity-delete-by-condition",
    "entity-delete-related",
    "entity-find",
    "entity-find-count",
    "entity-find-one",
    "entity-find-related",
    "entity-find-related-one",
    "entity-make-value",
    "entity-sequenced-id-primary",
    "entity-sequenced-id-secondary",
    "entity-set",
    "entity-update",
    "expression",
    "field-map",
    "filter-map-list",
    "having-econditions",
    "if",
    "iterate",
    "limit-range",
    "limit-view",
    "log",
    "message",
    "not",
    "or",
    "order-by",
    "order-map-list",
    "return",
    "script",
    "search-form-inputs",
    "select-field",
    "service-call",
    "set",
    "then",
    "use-iterator",
    "while",
];

const INLINE: &str = r#"<actions>
    <entity-find entity-name="moqui.test.TestEntity" list="found">
        <econdition field-name="testId" from="seedId"/>
    </entity-find>
    <entity-find-one entity-name="moqui.test.TestEntity" value-field="oneRow">
        <field-map field-name="testId" from="seedId"/>
    </entity-find-one>
    <script>foundCount = found.length; foundId = found.get(0).get("testId"); oneId = oneRow.get("testId");</script>
    <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
    <script>made.set("testId", "B"); made.set("testMedium", "made");</script>
    <entity-create value-field="made"/>
</actions>"#;

fn assert_golden(xml: &str, expected: &str) {
    assert_eq!(generate(xml).unwrap(), expected);
    assert_eq!(generate(xml).unwrap(), expected);
}

/// `testdata/{name}.xml` generates `testdata/{name}.js`; `UPDATE_GOLDEN=1` writes the file instead.
fn assert_golden_named(name: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata");
    let xml = std::fs::read_to_string(dir.join(format!("{name}.xml"))).unwrap();
    let path = dir.join(format!("{name}.js"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, generate(&xml).unwrap()).unwrap();
        return;
    }
    assert_golden(&xml, &std::fs::read_to_string(&path).unwrap());
}

#[test]
fn iterate_continue_advances_to_the_next_entry() {
    clear_context_stack();
    run_actions(
        r#"<actions>
                <set field="seen" from="[]"/>
                <iterate list="[1, 2, 3, 4]" entry="n">
                    <if condition="n % 2 == 0"><continue/></if>
                    <script>seen.push(n)</script>
                </iterate>
                <set field="seenText" from="seen.join(',')"/>
            </actions>"#,
    )
    .unwrap();
    let text = crate::script::context_value("seenText").map(|value| moqui_lib::object::to_plain_string(&value));
    assert_eq!(text.as_deref(), Some("1,3"));
}

#[test]
fn list_helpers_and_the_condition_factory_run() {
    clear_context_stack();
    run_actions(
        r#"<actions>
                <set field="items" from="[{n: 'b', q: 2}, {n: 'c', q: 2}, {n: 'a', q: 1}]"/>
                <order-map-list list="items"><order-by field-name="n"/></order-map-list>
                <filter-map-list list="items"><field-map field-name="q" value="2"/></filter-map-list>
                <set field="names" from="items.map(function (item) { return item.n; }).join(',')"/>
                <set field="seesNames" from="context.names"/>
                <set field="made" from="ec.entity.conditionFactory.makeCondition('a', 'greater', 3) != null"/>
                <set field="skipped" from="ec.entity.conditionFactory.makeActionConditionDirect('a', 'equals', null, null, null, false, true, false, 'false') === null"/>
                <set field="dated" from="ec.entity.conditionFactory.makeConditionDate('fromDate', 'thruDate', null, false, 'false') != null"/>
                <set field="ignoreFlag" from="true"/>
                <set field="ignoredDate" from="ec.entity.conditionFactory.makeConditionDate('fromDate', 'thruDate', null, false, ignoreFlag) === null"/>
            </actions>"#,
    )
    .unwrap();
    let text = |name: &str| crate::script::context_value(name).map(|value| moqui_lib::object::to_plain_string(&value));
    assert_eq!(text("names").as_deref(), Some("b,c"));
    assert_eq!(text("seesNames").as_deref(), Some("b,c"));
    assert_eq!(text("made").as_deref(), Some("true"));
    assert_eq!(text("skipped").as_deref(), Some("true"));
    assert_eq!(text("dated").as_deref(), Some("true"));
    assert_eq!(text("ignoredDate").as_deref(), Some("true"));
}

/// A list of maps passed in keeps what an iterate sets on an element when another iterate (after a service call)
/// reads the list again, as the Java maps are the same objects.
#[test]
fn an_element_changed_in_an_iterate_is_changed_in_the_next_one() {
    clear_context_stack();
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    let item = |name: &str| {
        let mut map = indexmap::IndexMap::default();
        map.insert("name".to_string(), Value::string(name));
        Value::Map(map)
    };
    set_context_name("datasets", Value::List(vec![item("a"), item("b")])).unwrap();
    run_actions(
        r#"<actions>
                <iterate list="datasets" entry="dataset">
                    <set field="dataset.classSet" from="[]"/>
                    <!-- a call that may change the context releases the script's values, as a service call does -->
                    <script>ec.resource.expand('x', '', {k: 1})</script>
                    <script>dataset.classSet.push(dataset.name)</script>
                </iterate>
                <set field="seen" from="datasets.map(function (d) { return d.classSet == null ? 'none' : d.classSet.join(','); }).join('|')"/>
            </actions>"#,
    )
    .unwrap();
    let seen = crate::script::context_value("seen").map(|value| moqui_lib::object::to_plain_string(&value));
    assert_eq!(seen.as_deref(), Some("a|b"));
    clear_context_stack();
}

/// `ignore` is an expression in the XML: the generated code passes its value, and a plain `econdition` with an
/// `ignore` is not made the shorthand `.condition(field, value)` that has no way to skip.
#[test]
fn the_ignore_attribute_is_an_expression() {
    let script = generate_with(
        r#"<actions><entity-find entity-name="a.B" list="rows">
                <econdition field-name="statusId" ignore="!statusId &amp;&amp; !strictAll"/>
                <econdition field-name="partyId"/>
                <date-filter ignore="skipDates"/>
            </entity-find></actions>"#,
        false,
    )
    .unwrap();
    assert!(script.contains("!!(!statusId && !strictAll)"), "{script}");
    assert!(script.contains("!!(skipDates)"), "{script}");
    assert!(script.contains(r#".condition("partyId", partyId)"#), "{script}");
}

#[test]
fn generated_scripts_are_shared_by_threads() {
    let xml = r#"<actions><set field="sharedGenerationProbe" value="1"/></actions>"#;
    let here = super::generated(xml).unwrap();
    let there = std::thread::spawn(move || super::generated(xml).unwrap())
        .join()
        .unwrap();
    assert!(
        std::ptr::eq(here.script.text(), there.script.text()),
        "one generated text for every thread"
    );
}

#[test]
fn condition_element_expression_may_span_lines() {
    use moqui_lib::object::Value as V;
    for (name, value) in [("multiA", V::Bool(true)), ("multiB", V::string("x"))] {
        crate::script::set_context_name(name, value).unwrap();
    }
    let xml = "<condition><expression>multiA &amp;&amp; (\n  (['x', 'y'].includes(multiB) &amp;&amp; multiA) ||\n  (multiB == 'q'))</expression></condition>";
    assert!(super::eval_condition_element(xml).unwrap());
    crate::script::set_context_name("multiB", V::string("n")).unwrap();
    assert!(!super::eval_condition_element(xml).unwrap());
}

#[test]
fn golden_service_call() {
    assert_golden_named("service-call");
}

#[test]
fn web_send_json_response_false_sends_nothing_and_a_name_sends_before_the_error_check() {
    let off = generate(
        r#"<actions><service-call name="org.moqui.example.ExampleServices.do#Thing" web-send-json-response="false"/></actions>"#,
    )
    .unwrap();
    assert!(!off.contains("sendJsonResponse"), "{off}");
    let named = generate(
        r#"<actions><service-call name="org.moqui.example.ExampleServices.do#Thing" web-send-json-response="resultList"/></actions>"#,
    )
    .unwrap();
    let send = named.find("ec.web.sendJsonResponse(").expect(&named);
    let check = named.find("if (ec.message.hasError()) return;").expect(&named);
    assert!(send < check, "{named}");
    assert!(named.contains(r#"call_service_result["resultList"]"#), "{named}");
}

#[test]
fn golden_script() {
    assert_golden_named("script");
}

#[test]
fn golden_set() {
    assert_golden_named("set");
}

#[test]
fn golden_entity_find() {
    assert_golden_named("entity-find");
}

#[test]
fn a_limit_or_offset_with_an_expression_is_expanded_and_a_literal_is_as_written() {
    let expression = generate(
        r#"<actions><entity-find entity-name="moqui.basic.Enumeration" list="found" limit="${sideLimit}" offset="${sideOffset}"/></actions>"#,
    )
    .unwrap();
    assert!(
        expression
            .contains(r#".offset(ec.resource.expand("${sideOffset}")).limit(ec.resource.expand("${sideLimit}"))"#),
        "{expression}"
    );
    let literal = generate(
        r#"<actions><entity-find entity-name="moqui.basic.Enumeration" list="found" limit="30" offset="10"/></actions>"#,
    )
    .unwrap();
    assert!(literal.contains(".offset(10).limit(30)"), "{literal}");
    // What `lint --screens` runs: both parse.
    super::check_actions(
        r#"<actions><entity-find entity-name="moqui.basic.Enumeration" list="found" limit="${sideLimit}"/></actions>"#,
    )
    .unwrap();
    super::check_actions(
        r#"<actions><entity-find entity-name="moqui.basic.Enumeration" list="found" limit="30"/></actions>"#,
    )
    .unwrap();
}

#[test]
fn golden_entity_value() {
    assert_golden_named("entity-value");
}

#[test]
fn golden_control() {
    assert_golden_named("control");
}

#[test]
fn golden_message() {
    assert_golden_named("message");
}

#[test]
fn golden_condition() {
    assert_golden_named("condition");
}

#[test]
fn nested_blocks_indent_but_inline_script_text_is_unchanged() {
    let source = generate("<actions><if condition=\"a\"><script>var s = `one\ntwo`;</script></if></actions>").unwrap();
    assert!(
        source.contains("        // begin inline script\n        var s = `one\ntwo`;\n        // end inline script\n")
    );
    assert!(!source.contains('\u{1}') && !source.contains('\u{2}'));
}

#[test]
fn debug_adds_a_comment_naming_each_element() {
    let source = generate_with("<actions><set field=\"a\" value=\"b\"/></actions>", true).unwrap();
    assert!(source.contains("    // <set field=\"a\" value=\"b\">\n    a = ec.resource.expand(\"b\");\n"));
    let plain = generate_with("<actions><set field=\"a\" value=\"b\"/></actions>", false).unwrap();
    assert!(!plain.contains("//"));
}

#[test]
fn a_bare_expression_statement_ends_before_the_next_statement() {
    let source = generate(
        "<actions><expression>foo()</expression><if condition=\"a\"><expression>bar()</expression></if></actions>",
    )
    .unwrap();
    assert!(
        source.contains("    foo();\n    if (a) {\n        bar();\n    }\n"),
        "{source}"
    );
}

#[test]
fn a_missing_macro_names_the_element() {
    let err = generate("<actions><no-such-action/></actions>").unwrap_err();
    assert!(err.to_string().contains("no-such-action"));
}

#[test]
fn each_concrete_element_has_a_macro() {
    let names = macro_names().unwrap();
    assert_eq!(names, MACROS);
    assert!(!names
        .iter()
        .any(|name| *name == "service-group" || *name == "service-invoke"));
}

#[test]
fn empty_actions_are_ok() {
    run_actions("<actions/>").unwrap();
}

#[test]
fn distribute_is_a_load_error() {
    let err = run_actions(r#"<actions><service-call name="example.do#It" async="distribute"/></actions>"#).unwrap_err();
    assert!(err.to_string().contains("distribute"), "{err}");
}

#[test]
fn entity_data_is_not_implemented() {
    let err = run_actions("<actions><entity-data/></actions>").unwrap_err();
    assert!(err.to_string().contains("entity-data"), "{err}");
}

#[test]
fn call_future_returns_a_future_the_script_can_wait_on() {
    let path = std::env::temp_dir().join(format!("moqui-script-future-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services>
                <service verb="echo" noun="Note" authenticate="false" transaction="ignore">
                    <in-parameters><parameter name="note"/></in-parameters>
                    <out-parameters><parameter name="note"/></out-parameters>
                </service>
            </services>"#,
    )
    .unwrap();
    let ctx =
        crate::context::ExecutionContextFactory::test_context_with("", crate::service::ServicePools::bounded(2, 8));
    ctx.factory().set_entity(crate::testing::open());
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let done = eval_condition(
        "var f = ec.service.async().name('echo#Note').parameter('note', 'hi').callFuture(); \
             var out = f.get(5000); out.note === 'hi' && f.isDone() && f.get().note === 'hi'",
    )
    .unwrap();
    assert!(done);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn service_async_distribute_is_accepted_and_runs_on_this_server() {
    clear_context_stack();
    // Java sends a distributed call to another server of a cluster. A call here always runs on this server's workers,
    // so `distribute(true)` returns the builder unchanged and the call is the same as without it.
    let present =
        eval_condition("(function () { const b = ec.service.async(); return b.distribute(true) === b; })()").unwrap();
    assert!(present);
}

#[test]
fn entity_find_without_an_execution_context_errors() {
    clear_context_stack();
    let err = run_actions(
        r#"<actions><entity-find entity-name="moqui.test.TestEntity" list="found"><econdition field-name="testId" from="seedId"/></entity-find></actions>"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("no execution context is entered"), "{err}");
}

#[test]
fn a_pushed_context_list_keeps_its_rows() {
    clear_context_stack();
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    run_actions(
        r#"<actions><set field="xs" from="[]"/><script>xs.push(1)</script><script>xs.push(2)</script>
                <set field="n" from="xs.length"/></actions>"#,
    )
    .unwrap();
    assert_eq!(crate::script::context_value("n"), Some(Value::I32(2)));
    clear_context_stack();
}

#[test]
fn message_expands_and_records_an_error() {
    clear_context_stack();
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    set_context_name("seedId", Value::string("A")).unwrap();
    run_actions(r#"<actions><message error="true">failed ${seedId}</message></actions>"#).unwrap();
    let errors = ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap();
    assert_eq!(errors, vec!["failed A".to_string()]);
    clear_context_stack();
}

#[test]
fn inline_action_finds_and_creates() -> Result<(), Error> {
    clear_context_stack();
    let facade = open_facade();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _guard = ctx.enter();
    let mut seed = facade.make_value("moqui.test.TestEntity").expect("make seed");
    seed.set("testId", Value::string("A"));
    seed.set("testMedium", Value::string("seed"));
    seed.create(&facade).expect("create seed");
    set_context_name("seedId", Value::string("A"))?;
    run_actions(INLINE)?;
    assert!(eval_condition("foundCount === 1")?, "foundCount");
    assert!(eval_condition("foundId === \"A\"")?, "foundId");
    assert!(eval_condition("oneId === \"A\"")?, "oneId");
    let rows = facade
        .find("moqui.test.TestEntity")
        .expect("find")
        .list()
        .expect("list");
    let mut ids = Vec::new();
    for row in rows.iter() {
        ids.push(field_str(row, "testId"));
    }
    ids.sort();
    assert_eq!(ids, vec!["A".to_string(), "B".to_string()]);
    let made = rows.iter().find(|row| field_str(row, "testId") == "B").expect("row B");
    assert_eq!(field_str(made, "testMedium"), "made");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_paginated_search_sets_the_page_keys_of_the_list() -> Result<(), Error> {
    clear_context_stack();
    let facade = open_facade();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _guard = ctx.enter();
    for index in 0..25 {
        let mut row = facade.make_value("moqui.test.TestEntity").expect("make row");
        row.set("testId", Value::string(format!("P{index:02}")));
        row.create(&facade).expect("create row");
    }
    let actions = r#"<actions><entity-find entity-name="moqui.test.TestEntity" list="found">
            <search-form-inputs default-order-by="testId"/></entity-find></actions>"#;
    // a full page: the count of all rows comes from the database
    set_context_name("pageIndex", Value::I32(1))?;
    set_context_name("pageSize", Value::I32(10))?;
    run_actions(actions)?;
    for check in [
        "found.length === 10",
        "found.get(0).get('testId') === 'P10'",
        "foundCount === 25",
        "foundPageIndex === 1",
        "foundPageSize === 10",
        "foundPageMaxIndex === 2",
        "foundPageRangeLow === 11",
        "foundPageRangeHigh === 20",
    ] {
        assert!(eval_condition(check)?, "{check}");
    }
    // the last page is short: the count is the rows before it and on it
    set_context_name("pageIndex", Value::I32(2))?;
    run_actions(actions)?;
    for check in ["found.length === 5", "foundCount === 25", "foundPageRangeHigh === 25"] {
        assert!(eval_condition(check)?, "{check}");
    }
    clear_context_stack();
    Ok(())
}

fn macro_names() -> Result<Vec<String>, Error> {
    crate::script::oxide::walker::action_macro_names(super::TEMPLATE_SUPPORT, super::XML_ACTIONS)
}

fn field_str(row: &EntityValue, name: &str) -> String {
    row.get(name).and_then(Value::as_str).unwrap_or("").to_string()
}

#[test]
fn a_cache_keeps_a_map_and_a_script_file_returns_its_value() {
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><cache-list><cache name="l10n.message"/></cache-list></moqui-conf>"#,
    );
    let _guard = ctx.enter();
    clear_context_stack();
    run_actions(
        r#"<actions><script>
                const cache = ec.cache.getCache('l10n.message');
                cache.put('mapKey', {count: 2, names: ['a', 'b']});
                const back = cache.get('mapKey');
                kept = back.count + back.names.length;
            </script></actions>"#,
    )
    .unwrap();
    assert_eq!(
        crate::script::runtime::context_value("kept").map(|value| moqui_lib::object::to_plain_string(&value)),
        Some("4".to_string())
    );

    let dir = std::env::temp_dir().join(format!("moqui-script-value-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("Answer.js");
    std::fs::write(&file, "6 * 7").unwrap();
    set_context_name("where", Value::string(file.to_string_lossy())).unwrap();
    run_actions("<actions><script>answer = ec.resource.script(where)</script></actions>").unwrap();
    assert_eq!(
        crate::script::runtime::context_value("answer").map(|value| moqui_lib::object::to_plain_string(&value)),
        Some("42".to_string())
    );
    clear_context_stack();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn script_diagnostics_name_files_and_keep_rust_origins() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    clear_context_stack();
    let dir = std::env::temp_dir().join(format!("moqui-script-diagnostics-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Identical text at different locations must not reuse bytecode with the first file's name.
    let text = "const x = 1;\nthrow new Error('file failed');\n";
    for name in ["First.js", "Second.js"] {
        let path = dir.join(name).to_string_lossy().into_owned();
        std::fs::write(&path, text).unwrap();
        set_context_name("where", Value::string(&path)).unwrap();
        for _ in 0..2 {
            let err = run_actions("<actions><script>ec.resource.script(where)</script></actions>").unwrap_err();
            assert!(err.message().contains(&format!("{path}:2:")), "{}", err.diagnostic());
            assert!(!err.diagnostic().contains("Rust:"), "{}", err.diagnostic());
        }
    }
    let path = dir.join("Resource.js").to_string_lossy().into_owned();
    std::fs::write(
        &path,
        "const x = 1;\nec.resource.getLocationText('component://MissingDiagnostics/screen/A.xml');\n",
    )
    .unwrap();
    let err = crate::script::run_script_location(&path).unwrap_err();
    let diagnostic = err.diagnostic();
    assert!(diagnostic.contains(&format!("{path}:2:")), "{diagnostic}");
    assert!(
        diagnostic.contains("Rust: crates/moqui-core/src/context/resource.rs:"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("component://MissingDiagnostics/screen/A.xml"),
        "{diagnostic}"
    );
    assert!(crate::script::eval_condition("(function () { try { ec.resource.getLocationText('component://MissingDiagnostics/screen/A.xml'); } catch (e) { return e.message === 'component MissingDiagnostics is not registered for location component://MissingDiagnostics/screen/A.xml' && e.stack.includes('Rust: crates/moqui-core/src/context/resource.rs:'); } return false; })()").unwrap());
    let syntax_path = dir.join("Syntax.js").to_string_lossy().into_owned();
    std::fs::write(&syntax_path, "const x = 1;\nconst broken = ;\n").unwrap();
    let err = crate::script::run_script_location(&syntax_path).unwrap_err();
    assert!(
        err.diagnostic().contains(&format!("{syntax_path}:2:")),
        "{}",
        err.diagnostic()
    );
    clear_context_stack();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn set_on_an_entity_value_field_sets_the_field() {
    let facade = open_facade();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _guard = ctx.enter();
    clear_context_stack();
    // A value's fields are properties, as on Java's EntityValue map: set, read, keys, and JSON.
    run_actions(
        r#"<actions>
                <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                <set field="made.testId" value="SetField"/>
                <set field="made.testMedium" value="medium"/>
                <script>made.create(); keys = Object.keys(made).includes('testMedium'); json = JSON.parse(JSON.stringify(made)).testId;
                    try { made.notAField = 1; refused = 'no' } catch (e) { refused = 'yes' }</script>
                <entity-find-one entity-name="moqui.test.TestEntity" value-field="found"><field-map field-name="testId" value="SetField"/></entity-find-one>
                <set field="seenMedium" from="found.testMedium"/>
                <set field="hasIt" from="'testMedium' in found"/>
            </actions>"#,
    )
    .unwrap();
    let read = |name: &str| {
        crate::script::runtime::context_value(name).map(|value| moqui_lib::object::to_plain_string(&value))
    };
    assert_eq!(read("seenMedium"), Some("medium".to_string()));
    assert_eq!(read("keys"), Some("true".to_string()));
    assert_eq!(read("json"), Some("SetField".to_string()));
    assert_eq!(read("refused"), Some("yes".to_string()));
    assert_eq!(read("hasIt"), Some("true".to_string()));
    clear_context_stack();
}

fn open_facade() -> EntityFacade {
    crate::testing::open()
}

/// An inert module (`parse::module_is_inert`) loaded twice in one run is the same value, its functions made once; a
/// module with a memo at its top level is made again at each load.
#[test]
fn an_inert_module_is_kept_for_the_run_and_one_with_a_memo_is_not() {
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    clear_context_stack();
    let dir = std::env::temp_dir().join(format!("moqui-script-modules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text, kept) in [
        ("Inert.js", "function twice(n) { return n * 2 }\n({twice})\n", true),
        (
            "Memo.js",
            "let memo = null\nfunction twice(n) { return n * 2 }\n({twice})\n",
            false,
        ),
    ] {
        let file = dir.join(name);
        std::fs::write(&file, text).unwrap();
        set_context_name("where", Value::string(file.to_string_lossy())).unwrap();
        run_actions(
            "<actions><script>const a = ec.resource.script(where), b = ec.resource.script(where); same = a === b; \
             four = b.twice(2)</script></actions>",
        )
        .unwrap();
        let read = |name: &str| {
            crate::script::runtime::context_value(name).map(|value| moqui_lib::object::to_plain_string(&value))
        };
        assert_eq!(read("same"), Some(kept.to_string()), "{name}");
        assert_eq!(read("four"), Some("4".to_string()), "{name}");
    }
    clear_context_stack();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The actions of the framework's and the components' service files generate the same text (or fail) each time, with
/// and without debug comments.
#[test]
fn service_actions_generate() {
    let mut compared = 0;
    use crate::script::walker::test_files::{component_xml_files, named};
    for file in component_xml_files("service") {
        let Ok(root) = MNode::parse_file(&file) else {
            continue;
        };
        let mut actions = Vec::new();
        named(&root, "actions", &mut actions);
        for node in actions {
            let xml = node.write_string();
            for debug in [false, true] {
                let first = generate_with(&xml, debug);
                let again = generate_with(&xml, debug);
                match (&first, &again) {
                    (Ok(first), Ok(again)) => assert_eq!(first, again, "{}", file.display()),
                    (Err(_), Err(_)) => {}
                    _ => panic!("{}: {first:?}, then {again:?}", file.display()),
                }
            }
            compared += 1;
        }
    }
    assert!(compared > 100, "{compared}");
}

/// Text generated apart from a running script, on the generator thread, is generated by the macro walker.
#[test]
fn actions_generated_apart_are_generated_by_the_walker() {
    use crate::script::oxide::walker::generated_actions_with;
    let xml = "<actions><set field=\"apartOnTheWalker\" value=\"1\"/></actions>";
    let generated = generate_apart(xml, false).unwrap();
    assert!(generated.contains("apartOnTheWalker"), "{generated}");
    assert!(generated_actions_with("apartOnTheWalker"));
}

/// Text generated apart from a running script is bounded by the caller's script deadline, on the generator thread, on
/// either engine.
#[test]
fn actions_generated_apart_keep_the_callers_deadline() {
    let sets: String = (0..2000).map(|index| format!("<set field=\"f{index}\" value=\"{index}\"/>")).collect();
    let xml = format!("<actions>{sets}</actions>");
    let late = {
        let _deadline = crate::script::DeadlineGuard::new(Some(std::time::Duration::ZERO));
        generate_apart(&xml, false)
    };
    let late = late.unwrap_err();
    assert!(late.message().contains("interrupted"), "{}", late.message());
    let in_time = generate_apart(&xml, false).unwrap();
    assert!(in_time.contains("f1999"));
}
