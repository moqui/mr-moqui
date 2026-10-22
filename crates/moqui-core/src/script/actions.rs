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

//! XML actions. One cached script per action XML text. Names in that script are bare.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use moqui_lib::node::MNode;
use rquickjs::{Ctx, Object};

use super::runtime::ScriptText;
use super::walker::{self, Walk};
use crate::context::Error;

const TEMPLATE_SUPPORT: &str = include_str!("../../../../runtime/base-component/framework/template/TemplateSupport.js");
const XML_ACTIONS: &str = include_str!("../../../../runtime/base-component/framework/template/XmlActions.js");

/// Set this to add a comment naming the XML element before each generated statement. The value `dump` also prints
/// each generated script to stderr, with line numbers as an error's `eval_script:LINE` has them.
const DEBUG_ENV: &str = "MOQUI_ACTIONS_DEBUG";

/// The generated scripts kept, and the XML and script text they may add up to.
const MAX_CACHED_SCRIPTS: usize = 4096;
const MAX_CACHED_SCRIPT_BYTES: usize = 32 * 1024 * 1024;

type Generated = Mutex<super::bounded::BoundedMap<String, ScriptText>>;

/// The script generated for each action XML text, for every thread: generation is a function of the text alone.
fn generated_scripts() -> &'static Generated {
    static SCRIPTS: OnceLock<Generated> = OnceLock::new();
    SCRIPTS.get_or_init(|| {
        Mutex::new(super::bounded::BoundedMap::with_bytes(
            MAX_CACHED_SCRIPTS,
            MAX_CACHED_SCRIPT_BYTES,
        ))
    })
}

pub fn run_actions(actions: &str) -> Result<(), Error> {
    run_actions_at(actions, None)
}

/// Name generated JavaScript after its owning actions, without pretending generated lines are XML lines.
pub(crate) fn run_actions_at(actions: &str, source: Option<&moqui_lib::error::SourceLocation>) -> Result<(), Error> {
    if actions.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let script = generated(actions)?;
    let file_name = source.and_then(|source| {
        source
            .file
            .as_ref()
            .map(|file| format!("{file}#actions-{}-{}.generated.js", source.line, source.column))
    });
    let script = match &file_name {
        Some(file_name) => script.with_file_name(file_name),
        None => script,
    };
    let file_name = file_name.as_deref().unwrap_or("eval_script");
    // Data preparation may do what a browser cannot, so a widget check in progress stands down while the actions run.
    let _unchecked = super::runtime::WidgetWrap::set(false);
    super::runtime::eval_script_text(&script).map_err(|err| with_source_line(err, script.text(), file_name))
}

/// Evaluate a `<condition>` element as `<if>` does. Its children are xml-actions conditions
/// (`expression`, `compare`, `and`, `or`, `not`), and all of them must hold. An element with no
/// condition macro is an error, never a pass.
pub fn eval_condition_element(xml: &str) -> Result<bool, Error> {
    if xml.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let root = MNode::parse_text("condition", xml)?;
    if root.name() != "condition" {
        return Err(Error::new(format!(
            "expected a condition element, found {}",
            root.name()
        )));
    }
    // The children of an `and` render joined by `&&`, or `true` when there are none.
    let mut combined = String::from("<and>");
    for child in root.children() {
        combined.push_str(&child.write_string());
    }
    combined.push_str("</and>");
    let script = generated(&combined)?;
    // Only the root `actions` macro strips the raw-region marks of multi-line user code.
    super::runtime::eval_condition(&script.text().replace(['\u{1}', '\u{2}'], ""))
}

/// [`eval_condition_element`] for a condition group made once: `<and>` holding the condition macros.
pub fn eval_condition_group(and_xml: &str) -> Result<bool, Error> {
    if and_xml.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let script = generated(and_xml)?;
    super::runtime::eval_condition(&script.text().replace(['\u{1}', '\u{2}'], ""))
}

/// An error at `eval_script:LINE:COL` with that line of the generated script after it, so a failure in actions points at
/// the statement. With `MOQUI_ACTIONS_DEBUG` set, the element comment above it is shown too.
fn with_source_line(err: Error, source: &str, file_name: &str) -> Error {
    let message = err.message();
    let prefix = format!("{file_name}:");
    let line = message
        .find(&prefix)
        .and_then(|at| message[at + prefix.len()..].split(':').next()?.parse::<usize>().ok());
    let Some(line) = line.filter(|line| *line > 0) else {
        return err;
    };
    let lines: Vec<&str> = source.lines().collect();
    let Some(text) = lines.get(line - 1) else {
        return err;
    };
    let mut detail = format!("\n    at generated line {line}: {}", text.trim());
    if let Some(comment) = lines[..line - 1]
        .iter()
        .rev()
        .map(|text| text.trim())
        .find(|text| text.starts_with("//"))
    {
        detail.push_str(&format!("\n    after {comment}"));
    }
    let message = format!("{}{detail}", message.trim_end());
    err.with_message(message)
}

/// Generate the script for `xml` and parse it, without running anything. Definition load calls
/// this, so an expression that is not JavaScript fails the service there.
///
/// # Errors
///
/// Returns an error when the XML does not generate, or the generated script does not parse.
pub fn check_actions(xml: &str) -> Result<(), Error> {
    if xml.trim().is_empty() {
        return Ok(());
    }
    let script = generated(xml)?;
    super::parse::check_program(script.text())
}

fn generated(xml: &str) -> Result<ScriptText, Error> {
    let scripts = generated_scripts();
    let cached = scripts.lock().unwrap_or_else(PoisonError::into_inner).get(xml);
    if let Some(script) = cached {
        return Ok(script);
    }
    // Not under the lock: two threads may generate the same text, and the second insert is the same script.
    let script = ScriptText::new(generate(xml)?);
    let bytes = xml.len() + script.text().len();
    scripts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert_sized(xml.to_string(), script.clone(), bytes);
    Ok(script)
}

fn generate(xml: &str) -> Result<String, Error> {
    let debug = std::env::var_os(DEBUG_ENV);
    let source = generate_with(xml, debug.is_some())?;
    if debug.is_some_and(|value| value == "dump") {
        eprintln!("---- generated action script ----");
        for (index, line) in source.lines().enumerate() {
            eprintln!("{:>4}: {line}", index + 1);
        }
    }
    Ok(source)
}

fn generate_with(xml: &str, debug: bool) -> Result<String, Error> {
    let root = MNode::parse_text("actions", xml)?;
    reject(&root)?;
    super::runtime::with_ctx(|ctx| {
        let name = if debug { "xml-actions-debug" } else { "xml-actions" };
        let macros = super::runtime::engine_object(ctx, name, |ctx| load_macros(ctx, debug))?;
        let (source, _) = render(ctx, &macros, &root)?;
        Ok(source)
    })
}

/// The macro table. The support script evaluates to an object, and the actions script
/// is a function of that object, so no names are left in the global scope.
fn load_macros<'js>(ctx: &Ctx<'js>, debug: bool) -> Result<Object<'js>, Error> {
    let loaded = walker::load_chain(ctx, TEMPLATE_SUPPORT, &[XML_ACTIONS], debug)?;
    loaded
        .into_iter()
        .next_back()
        .ok_or_else(|| Error::new("no XML actions macros"))
}

fn reject(node: &MNode) -> Result<(), Error> {
    let name = node.name();
    if name == "entity-data" {
        return Err(Error::new("entity-data is not implemented"));
    }
    if name == "service-call" && node.attribute("async").as_deref() == Some("distribute") {
        return Err(Error::new("async=\"distribute\" is not a host method"));
    }
    for child in node.children() {
        reject(&child)?;
    }
    Ok(())
}

fn render<'js>(ctx: &Ctx<'js>, macros: &Object<'js>, node: &MNode) -> Result<(String, Object<'js>), Error> {
    let opaque = HashSet::new();
    let walk = Walk {
        macros,
        opaque: &opaque,
        resolve: None,
        missing: "XML action macro",
        fallback: None,
    };
    walker::walk(ctx, &walk, node, None)
}

#[cfg(test)]
mod tests {

    use super::{generate, generate_with, load_macros, run_actions};
    use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
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
        let text =
            |name: &str| crate::script::context_value(name).map(|value| moqui_lib::object::to_plain_string(&value));
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
            let mut map = indexmap::IndexMap::new();
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
    fn generated_scripts_are_shared_by_threads_and_the_macros_kept_per_engine() {
        let xml = r#"<actions><set field="sharedGenerationProbe" value="1"/></actions>"#;
        let here = super::generated(xml).unwrap();
        let there = std::thread::spawn(move || super::generated(xml).unwrap())
            .join()
            .unwrap();
        assert!(
            std::ptr::eq(here.text(), there.text()),
            "one generated text for every thread"
        );
        super::generated(r#"<actions><set field="macrosKeptProbe" value="2"/></actions>"#).unwrap();
        crate::script::runtime::with_ctx(|ctx| {
            crate::script::runtime::engine_object(ctx, "xml-actions", |_| {
                Err(Error::new("the macros were loaded again"))
            })
            .map(|_| ())
        })
        .unwrap();
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
        assert_golden(
            include_str!("testdata/service-call.xml"),
            include_str!("testdata/service-call.js"),
        );
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
        assert_golden(include_str!("testdata/script.xml"), include_str!("testdata/script.js"));
    }

    #[test]
    fn golden_set() {
        assert_golden(include_str!("testdata/set.xml"), include_str!("testdata/set.js"));
    }

    #[test]
    fn golden_entity_find() {
        assert_golden(
            include_str!("testdata/entity-find.xml"),
            include_str!("testdata/entity-find.js"),
        );
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
        assert_golden(
            include_str!("testdata/entity-value.xml"),
            include_str!("testdata/entity-value.js"),
        );
    }

    #[test]
    fn golden_control() {
        assert_golden(
            include_str!("testdata/control.xml"),
            include_str!("testdata/control.js"),
        );
    }

    #[test]
    fn golden_message() {
        assert_golden(
            include_str!("testdata/message.xml"),
            include_str!("testdata/message.js"),
        );
    }

    #[test]
    fn golden_condition() {
        assert_golden(
            include_str!("testdata/condition.xml"),
            include_str!("testdata/condition.js"),
        );
    }

    #[test]
    fn nested_blocks_indent_but_inline_script_text_is_unchanged() {
        let source =
            generate("<actions><if condition=\"a\"><script>var s = `one\ntwo`;</script></if></actions>").unwrap();
        assert!(source
            .contains("        // begin inline script\n        var s = `one\ntwo`;\n        // end inline script\n"));
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
        let err =
            run_actions(r#"<actions><service-call name="example.do#It" async="distribute"/></actions>"#).unwrap_err();
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
            eval_condition("(function () { const b = ec.service.async(); return b.distribute(true) === b; })()")
                .unwrap();
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
        crate::script::runtime::with_ctx(|ctx| {
            let macros = load_macros(ctx, false)?;
            let mut names = Vec::new();
            for key in macros.keys::<String>() {
                names.push(key.map_err(|err| Error::new(err.to_string()))?);
            }
            names.sort();
            Ok(names)
        })
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
}
