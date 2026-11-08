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

use super::convert::{MAX_NESTING, TOO_DEEP};
use super::{
    clear_context_stack, context_value, eval_condition, eval_value, expand, js_error, push_context_frame, run_script,
    run_script_location, set_context_name, with_screen_runner,
};
use crate::context::{Error, ExecutionContextFactory};
use moqui_lib::object::{basic_convert, Value};

const MAX_COEFFICIENT: &str = "79228162514264337593543950335";

fn decimal(text: &str) -> Value {
    let converted = basic_convert(&Value::string(text), "BigDecimal");
    assert!(converted.changed, "expected a decimal from {text}");
    converted.value
}

/// Each small entry runs and is counted by its kind: conditions, values, field values, `${}` strings and map literals.
#[test]
fn the_small_entries_run_and_are_counted_by_kind() -> Result<(), Error> {
    use crate::script::engine::{runs_here, EntryKind};
    let oxide_runs = |kind: &str| {
        EntryKind::ALL
            .iter()
            .find(|entry| entry.name() == kind)
            .map_or(0, |entry| runs_here(*entry))
    };
    clear_context_stack();
    set_context_name("n", Value::I32(3))?;
    let conditions = oxide_runs("eval_condition");
    assert!(eval_condition("n > 2")?);
    assert!(oxide_runs("eval_condition") > conditions);
    let values = oxide_runs("eval_value");
    assert_eq!(eval_value("n * 2")?, Value::I32(6));
    assert!(oxide_runs("eval_value") > values);
    let fields = oxide_runs("eval_field_value");
    assert_eq!(crate::script::eval_field_value("n")?, Value::I32(3));
    assert!(oxide_runs("eval_field_value") > fields);
    // `${}` strings and map literals
    let expands = oxide_runs("expand");
    assert_eq!(expand("n is ${n}, \\${not} `ticked`")?, "n is 3, ${not} `ticked`");
    assert_eq!(crate::script::expand_localized("${n + 1}")?, "4");
    assert!(oxide_runs("expand") >= expands + 2);
    let maps = oxide_runs("eval_map");
    let map = crate::script::eval_map("{a: n, b: [n]}")?;
    assert_eq!(map.get("a"), Some(&Value::I32(3)));
    assert!(oxide_runs("eval_map") > maps);
    let err = crate::script::eval_map("[n]").unwrap_err();
    assert_eq!(err.message(), "script value is not an object");
    Ok(())
}

/// Generated XML actions (set, if, iterate over the bound values, a script) leave the context they compute, and are
/// counted as actions.
#[test]
fn actions_leave_the_context_they_compute() -> Result<(), Error> {
    use crate::script::engine::{runs_here, EntryKind};
    const ACTIONS: &str = r#"<actions>
        <set field="total" from="a + 1"/>
        <if condition="total &gt; 2"><set field="big" from="true"/><else><set field="big" from="false"/></else></if>
        <set field="sum" from="0"/>
        <iterate list="items" entry="item"><set field="sum" from="sum + item.qty"/></iterate>
        <set field="names" from="items.map(i => i.name)"/>
        <set field="names2" from="names" set-if-empty="false"/>
        <set field="price" from="'12.50'" type="BigDecimal"/>
        <script>items.push({name: 'z', qty: 1})</script>
    </actions>"#;
    clear_context_stack();
    set_context_name("a", Value::I32(2))?;
    let item = |name: &str, qty: i32| {
        let mut map = indexmap::IndexMap::new();
        map.insert("name".to_string(), Value::string(name));
        map.insert("qty".to_string(), Value::I32(qty));
        Value::Map(map)
    };
    set_context_name("items", Value::List(vec![item("x", 2), item("y", 5)]))?;
    let before = runs_here(EntryKind::Actions);
    crate::script::run_actions(ACTIONS)?;
    assert!(runs_here(EntryKind::Actions) > before, "counted as actions");
    assert_eq!(context_value("total"), Some(Value::I32(3)));
    assert_eq!(context_value("big"), Some(Value::Bool(true)));
    assert_eq!(context_value("sum"), Some(Value::I32(7)));
    assert_eq!(
        context_value("names"),
        Some(Value::List(vec![Value::string("x"), Value::string("y")]))
    );
    assert_eq!(context_value("names2"), context_value("names"));
    assert_eq!(context_value("price"), Some(decimal("12.50")));
    match context_value("items") {
        Some(Value::List(items)) => assert_eq!(items.len(), 3, "the script's push is on the stack"),
        other => panic!("{other:?}"),
    }
    Ok(())
}

#[test]
fn unset_bare_name_is_null() -> Result<(), Error> {
    clear_context_stack();
    assert!(!eval_condition("unsetBareName")?);
    assert!(eval_condition("unsetBareName === null")?);
    assert!(eval_condition("typeof unsetBareName === \"object\"")?);
    assert!(!eval_condition("0")?);
    assert!(!eval_condition("false")?);
    Ok(())
}

#[test]
fn stack_name_evaluates_to_its_value() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("orderId", Value::string("1000"))?;
    push_context_frame();
    set_context_name("orderId", Value::string("2000"))?;
    assert!(eval_condition("orderId === \"2000\"")?);
    assert!(eval_condition("(assignedName = \"x\") === \"x\"")?);
    set_context_name("assignedName", Value::string("y"))?;
    assert!(eval_condition("assignedName === \"y\"")?);
    set_context_name("Array", Value::string("hidden"))?;
    assert!(eval_condition("typeof Array === \"function\"")?);
    // a helper object is a global of the runtime, which a context name does not hide
    set_context_name("ObjectUtilities", Value::string("mine"))?;
    assert!(eval_condition("typeof ObjectUtilities === \"object\"")?);
    clear_context_stack();
    assert!(eval_condition(
        "typeof ObjectUtilities === \"object\" && ObjectUtilities !== null"
    )?);
    Ok(())
}

#[test]
fn decimal_overflow_is_a_js_exception() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("big", decimal(MAX_COEFFICIENT))?;
    set_context_name("zero", decimal("0"))?;
    assert!(eval_condition("zero")?);
    assert!(eval_condition("zero.isZero()")?);
    assert!(!eval_condition("big.isZero()")?);
    // `==` compares objects, not values: a name read twice in a script is one object, another name is another.
    set_context_name("big2", decimal(MAX_COEFFICIENT))?;
    assert!(eval_condition("big == big")?);
    assert!(!eval_condition("big == big2")?);
    assert!(eval_condition("big.eq(big2)")?);
    let overflow = eval_condition("big.add(big)").expect_err("overflow");
    assert!(overflow.to_string().contains("overflow"), "{overflow}");
    let value_of = eval_condition("big + 1").expect_err("valueOf");
    assert!(value_of.to_string().contains("valueOf"), "{value_of}");
    Ok(())
}

#[test]
fn decimal_methods_take_numbers_and_text_and_to_number_is_explicit() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("price", decimal("10.25"))?;
    holds("price.add(1).eq('11.25')");
    holds("price.subtract(0.25).eq(10)");
    holds("price.multiply('2').toString() === '20.50'");
    holds("price.compare(70) === -1 && price.compare('10.250') === 0");
    holds("price.toNumber() === 10.25 && Number(price.toString()) === 10.25");
    holds("toDecimal(3).add(price).eq('13.25') && toDecimal('0.1').add(toDecimal('0.2')).eq('0.3')");
    let nan = eval_condition("price.add(NaN)").expect_err("not finite");
    assert!(nan.to_string().contains("a decimal method takes"), "{nan}");
    let text = eval_condition("price.add('abc')").expect_err("not numeric");
    assert!(text.to_string().contains("abc"), "{text}");
    let null = eval_condition("toDecimal(null)").expect_err("null");
    assert!(null.to_string().contains("toDecimal"), "{null}");
    let number = eval_condition("Number(price) > 1").expect_err("valueOf still throws");
    assert!(number.to_string().contains("toNumber"), "{number}");
    Ok(())
}

/// `JSON.stringify` writes a decimal as a JSON number, as Java's JSON writers write a `BigDecimal`: one a JavaScript
/// number holds exactly as that number, any other with all its digits.
#[test]
fn json_stringify_writes_a_decimal_as_a_number() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("price", decimal("10.25"))?;
    set_context_name("big", decimal("12345678901234567.89"))?;
    holds("JSON.stringify(price) === '10.25'");
    holds("JSON.stringify({price, list: [toDecimal('0.1'), toDecimal(-3)]}) === '{\"price\":10.25,\"list\":[0.1,-3]}'");
    holds("JSON.parse(JSON.stringify({price})).price === 10.25");
    holds("JSON.stringify(big) === '12345678901234567.89'");
    holds("JSON.stringify([big]) === '[12345678901234567.89]'");
    holds("typeof JSON.parse(JSON.stringify(big)) === 'number'");
    Ok(())
}

#[test]
fn expand_substitutes_a_context_field() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("name", Value::string("Ada"))?;
    set_context_name("big", decimal(MAX_COEFFICIENT))?;
    assert_eq!(expand("Hello ${name}")?, "Hello Ada");
    assert_eq!(expand("Hello ${name}")?, "Hello Ada");
    assert_eq!(expand("no dollars")?, "no dollars");
    assert_eq!(expand("trailing$")?, "trailing$");
    assert_eq!(expand("${big}")?, MAX_COEFFICIENT);
    Ok(())
}

fn holds(expression: &str) {
    assert!(eval_condition(expression).unwrap(), "{expression}");
}

#[test]
fn a_push_of_a_host_call_result_reaches_the_list() -> Result<(), Error> {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    set_context_name("messages", Value::List(Vec::new()))?;
    set_context_name("results", Value::List(Vec::new()))?;
    set_context_name("rows", Value::List(Vec::new()))?;
    run_script(
        r#"messages.push(ec.resource.expand('Only ${n} rows.', '', {n: 3}));
            messages.push(ec.resource.expand('${messages.length} before', '', {a: 1}));
            sameScript = messages.length;
            results.push(ec.service.sync().name('no.such.Services.run#Nothing').call());
            {
                const l = rows;
                for (let i = 0; i < 3; i++) {
                    ec.service.sync().name('no.such.Services.run#Nothing').call();
                    l.push(i);
                }
            }"#,
    )?;
    holds("sameScript === 2 && messages[0] === 'Only 3 rows.' && messages[1] === '1 before'");
    holds("results.length === 1 && results[0] === null");
    holds("rows.length === 3 && rows[2] === 2");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_compiled_script_runs_again_with_the_context_of_each_run() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("n", Value::I32(1))?;
    run_script("total = n * 10")?;
    assert_eq!(
        context_value("total").map(|v| moqui_lib::object::to_plain_string(&v)),
        Some("10".to_string())
    );
    set_context_name("n", Value::I32(4))?;
    run_script("total = n * 10")?;
    assert_eq!(
        context_value("total").map(|v| moqui_lib::object::to_plain_string(&v)),
        Some("40".to_string())
    );
    assert!(eval_condition("n > 3")?);
    set_context_name("n", Value::I32(2))?;
    assert!(!eval_condition("n > 3")?);
    let failed = run_script("this is not javascript").unwrap_err();
    assert!(failed.message().contains("expecting"), "{}", failed.message());
    let thrown = run_script("throw new Error('boom ' + n)").unwrap_err();
    assert!(thrown.message().contains("boom 2"), "{}", thrown.message());
    Ok(())
}

fn too_deep(outcome: Result<impl std::fmt::Debug, Error>) {
    let err = outcome.expect_err("cyclic or too deep");
    assert!(err.message().contains(TOO_DEEP), "{}", err.message());
}

#[test]
fn a_cyclic_or_too_deep_value_is_an_error() -> Result<(), Error> {
    clear_context_stack();
    too_deep(run_script("var a = {}; a.self = a; result = a;"));
    too_deep(eval_value("(() => { var a = []; a.push(a); return a; })()"));
    too_deep(eval_value("(() => { var s = new Set(); s.add([s]); return s; })()"));
    too_deep(eval_value(
        "(() => { var a = []; for (var i = 0; i < 100000; i++) a = [a]; return a; })()",
    ));
    too_deep(run_script("items = []; items.push(items);"));
    Ok(())
}

#[test]
fn a_shared_value_converts_once_for_each_place() -> Result<(), Error> {
    clear_context_stack();
    let value = eval_value("(() => { var s = {x: 1}; return {a: s, b: [s, s], c: new Set([s])}; })()")?;
    let Value::Map(map) = value else {
        panic!("expected a map, got {value:?}")
    };
    let Some(Value::List(both)) = map.get("b") else {
        panic!("expected a list, got {map:?}")
    };
    assert_eq!(map.get("a"), both.get(1));
    assert!(matches!(map.get("a"), Some(Value::Map(shared)) if shared.get("x") == Some(&Value::I32(1))));
    let deepest = format!("(() => {{ var a = []; for (var i = 1; i < {MAX_NESTING}; i++) a = [a]; return a; }})()");
    set_context_name("deep", eval_value(&deepest)?)?;
    assert!(eval_condition("deep.length === 1 && deep[0][0].length === 1")?);
    too_deep(run_script("wrapped = [deep];"));
    let mut host = Value::List(Vec::new());
    for _ in 0..1000 {
        host = Value::List(vec![host]);
    }
    set_context_name("hostDeep", host)?;
    too_deep(eval_condition("hostDeep.length === 1"));
    Ok(())
}

#[test]
fn an_integer_past_the_safe_range_is_an_error() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("smallest", Value::I64(i64::MIN))?;
    let err = eval_condition("smallest < 0").expect_err("i64::MIN is not exact");
    assert!(err.message().contains("too large"), "{}", err.message());
    set_context_name("safe", Value::I64(-9_007_199_254_740_991))?;
    assert!(eval_condition("safe === -Number.MAX_SAFE_INTEGER")?);
    Ok(())
}

fn plain(name: &str) -> Option<String> {
    context_value(name).map(|value| moqui_lib::object::to_plain_string(&value))
}

#[test]
fn context_is_the_stack_itself() -> Result<(), Error> {
    clear_context_stack();
    // The StatusFlows pattern: a script-local value written to the stack through `context`.
    run_script("let t = 'a'; context.t2 = t + 'b'")?;
    assert_eq!(plain("t2"), Some("ab".to_string()));
    holds("t2 === 'ab' && context.t2 === 'ab'");
    // A read through `context` sees what the host set, and a missing name reads as a bare one does.
    set_context_name("fromHost", Value::string("host"))?;
    holds("context.fromHost === 'host' && context.noSuchName === null && 'fromHost' in context");
    holds("!('noSuchName' in context) && 'context' in context && context.context === context");
    holds("Object.keys(context).includes('fromHost') && Object.keys(context).includes('t2')");
    holds("!Object.keys(context).includes('context') && ({...context}).fromHost === 'host'");
    run_script("let seen = []; for (const k in context) seen.push(k); forInSaw = seen.includes('t2')")?;
    assert_eq!(plain("forInSaw"), Some("true".to_string()));
    // Java's Map methods, for names no frame holds.
    run_script("context.put('viaPut', 1); gotViaGet = context.get('viaPut'); hasIt = context.containsKey('viaPut')")?;
    assert_eq!(plain("viaPut"), Some("1".to_string()));
    assert_eq!(plain("gotViaGet"), Some("1".to_string()));
    assert_eq!(plain("hasIt"), Some("true".to_string()));
    set_context_name("get", Value::string("a value named get"))?;
    holds("context.get === 'a value named get'");
    // `delete` removes the name from the top frame; the stack stays valid.
    run_script("delete context.viaPut; delete context.get")?;
    assert_eq!(plain("viaPut"), None);
    holds("!('viaPut' in context) && typeof context.get === 'function'");
    // The stack refuses the name `context` itself, through the proxy as for a bare assignment.
    assert!(run_script("context.context = 1").is_err());
    assert!(run_script("context = {}").is_err());
    clear_context_stack();
    Ok(())
}

#[test]
fn a_map_assigned_through_context_is_tracked_like_a_bare_one() -> Result<(), Error> {
    clear_context_stack();
    run_script("const m = {a: 1}; context.held = m; m.b = 2; const l = [1]; context.heldList = l; l.push(2)")?;
    holds("held.a === 1 && held.b === 2 && heldList.length === 2");
    // A read through `context` is the script's own tracked object, as a bare read is.
    run_script("const again = context.held; again.c = 3; context.heldList.push(3)")?;
    holds("held.c === 3 && heldList.length === 3 && context.heldList === heldList");
    // `context` itself is never tracked as a map: holding it under a name stores the visible map once.
    run_script("copied = context; copied2 = {inner: context}")?;
    holds("typeof copied === 'object' && copied.held.c === 3 && copied2.inner.heldList.length === 3");
    run_script("more = 1")?;
    holds("copied.more === undefined && copied2.inner.more === undefined");
    clear_context_stack();
    Ok(())
}

/// A list or map a script read and did not change is not converted back: the stack keeps its values as they were,
/// a time stamp a time stamp rather than its text. A change anywhere in it, a property added or removed, or a
/// getter is written back as before.
#[test]
fn a_tracked_value_the_script_did_not_change_keeps_the_stack_value() -> Result<(), Error> {
    clear_context_stack();
    let stamp = Value::Timestamp(jiff::Timestamp::from_second(1_700_000_000).unwrap());
    let row = |id: &str| {
        Value::Map(indexmap::IndexMap::from([
            ("id".to_string(), Value::string(id)),
            ("qty".to_string(), Value::I64(3)),
            ("when".to_string(), stamp.clone()),
        ]))
    };
    set_context_name("rows", Value::List(vec![row("A"), row("B")]))?;
    run_script("let n = 0; for (const r of rows) n += r.qty; total = n")?;
    let kept = context_value("rows").unwrap();
    let Value::List(items) = &kept else { panic!("{kept:?}") };
    let Value::Map(first) = &items[0] else {
        panic!("{kept:?}")
    };
    assert!(matches!(first.get("when"), Some(Value::Timestamp(_))), "{kept:?}");
    assert!(matches!(first.get("qty"), Some(Value::I64(3))), "{kept:?}");
    run_script("rows[1].qty = 4")?;
    holds("rows[1].qty === 4 && rows.length === 2");
    run_script("delete rows[0].when")?;
    holds("rows[0].when === undefined && rows[1].when !== undefined");
    run_script("rows.push({id: 'C'})")?;
    holds("rows.length === 3 && rows[2].id === 'C'");
    run_script("rows[0].extra = 1")?;
    holds("rows[0].extra === 1");
    // A getter that writes a bare name while the flush reads the object: the write is the script's, and the value
    // is converted as before.
    run_script("held = {}; Object.defineProperty(held, 'g', {get() { touched = 1; return 2 }, enumerable: true})")?;
    holds("held.g === 2");
    run_script("const h = held; h.x = 1")?;
    holds("held.x === 1");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_script_file_runs_in_names_of_its_own() -> Result<(), Error> {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    clear_context_stack();
    let dir = std::env::temp_dir().join(format!("moqui-file-scope-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(js_error)?;
    let write = |name: &str, text: &str| -> Result<String, Error> {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(js_error)?;
        Ok(path.to_string_lossy().into_owned())
    };
    let declares = write(
        "Declares.js",
        "const base = n * 2;\nlet more = base + 1;\nvar shadow = 'file';\nfunction twice(x) { return x * 2; }\nresult = twice(more);\n",
    )?;
    let stops = write("Stops.js", "if (stop) return;\nreached = true;\n")?;
    let throws = write("Throws.js", "thrown = 1;\n\nthrow new Error('on line three');\n")?;

    // The same declarations run again on this thread's engine, and the bare assignment reaches the context.
    set_context_name("n", Value::I32(1))?;
    run_script_location(&declares)?;
    assert_eq!(plain("result"), Some("6".to_string()));
    set_context_name("n", Value::I32(2))?;
    run_script_location(&declares)?;
    assert_eq!(plain("result"), Some("10".to_string()));
    // A `var` or `function` of the file is not a global that hides the context name in a later script.
    set_context_name("shadow", Value::string("context"))?;
    holds("shadow === 'context' && twice === null && base === null");

    set_context_name("stop", Value::Bool(true))?;
    run_script_location(&stops)?;
    assert_eq!(plain("reached"), None);
    set_context_name("stop", Value::Bool(false))?;
    run_script_location(&stops)?;
    assert_eq!(plain("reached"), Some("true".to_string()));

    let err = run_script_location(&throws).expect_err("throws");
    assert!(err.message().contains(&format!("{throws}:3:")), "{}", err.message());
    assert!(!err.diagnostic().contains("Rust:"), "{}", err.diagnostic());
    assert_eq!(plain("thrown"), Some("1".to_string()));

    // The host's globals are read-only: an assignment throws, in sloppy code too, and does not replace them for the
    // later scripts of the thread.
    let assigned = run_script("compare = 'a context value';").expect_err("compare is a host global");
    assert!(
        assigned
            .message()
            .contains("compare is a host global and cannot be assigned"),
        "{}",
        assigned.message()
    );
    assert!(run_script("toNumber = null;").is_err());
    run_script("ec = null; sri = null;")?;
    holds("typeof toNumber === 'function' && typeof compare === 'function' && ec !== null && ec.resource != null");
    holds("sri !== null && typeof sri.pushContext === 'function'");
    clear_context_stack();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn paginate_list_leaves_a_page_from_a_paginated_find_as_it_is() -> Result<(), Error> {
    clear_context_stack();
    // Page 2 of a paginated find: the list is the page, and the count and range are those of the whole result.
    run_script(
        "rows = [{a: 21}, {a: 22}]; rowsCount = 22; rowsPageRangeLow = 21; rowsPageRangeHigh = 22;
            CollectionUtilities.paginateList('rows', null, context)",
    )?;
    holds("rows.length === 2 && rows[0].a === 21");
    run_script(
        "all = [1, 2, 3]; pageSize = 2; pageIndex = 1; CollectionUtilities.paginateList('all', 'allPage', context)",
    )?;
    holds("allPage.length === 1 && allPage[0] === 3 && allPageCount === 3 && all.length === 3");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_unit_that_throws_after_a_push_leaves_the_stack_as_it_found_it() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("level", Value::string("outer"))?;
    let unit = crate::screen::generate::Unit {
        id: u64::MAX - 1,
        source: "(function () { sri.pushContext(); level = 'inner'; throw new Error('in the unit'); })".to_string(),
        runtime: std::sync::Arc::from(""),
        runtime_id: 0,
    };
    let err = with_screen_runner(|runner| runner.run(&unit)).expect_err("throws");
    assert!(err.message().contains("in the unit"), "{}", err.message());
    holds("level === 'outer'");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_loop_outside_a_transaction_stops_at_the_default_limit() -> Result<(), Error> {
    use std::time::{Duration, Instant};
    clear_context_stack();
    // As a `resource-facade` `script-timeout` of 200 ms would set it.
    super::SCRIPT_LIMIT.with(|limit| limit.set(Some(Some(Duration::from_millis(200)))));
    let started = Instant::now();
    let err = run_script("while (true) {}").expect_err("interrupted");
    assert!(err.message().contains("interrupted"), "{}", err.message());
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        super::script_time_remaining(),
        None,
        "the default goes when the script ends"
    );
    super::SCRIPT_LIMIT.with(|limit| limit.set(Some(None)));
    assert!(eval_condition("true")?);
    assert_eq!(super::script_time_remaining(), None);
    Ok(())
}

#[test]
fn the_default_limit_is_script_timeout_else_the_request_timeout_on_a_request_worker() {
    use std::time::Duration;
    let limit = super::script_limit_for;
    assert_eq!(limit("", false, ""), None, "a service or job thread");
    assert_eq!(limit("", true, ""), Some(Duration::from_secs(300)));
    assert_eq!(limit("", true, "90"), Some(Duration::from_secs(90)));
    assert_eq!(limit("", true, "0"), None);
    assert_eq!(limit("20", false, ""), Some(Duration::from_secs(20)));
    assert_eq!(limit("20", true, "90"), Some(Duration::from_secs(20)));
    assert_eq!(limit("0", true, "90"), None);
    assert_eq!(limit("junk", false, ""), None);
}

#[test]
fn request_scripts_have_room_for_nested_app_templates() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            super::mark_request_thread();
            assert_eq!(super::stack_budget(0).unwrap(), super::REQUEST_SCRIPT_STACK);
            assert!(super::stack_budget(1).unwrap() > super::NESTED_STACK);
            let err = run_script("function down(n) { return down(n + 1) + 1; } down(0);").unwrap_err();
            assert!(err.message().contains("stack"), "{err}");
        })
        .unwrap()
        .join()
        .expect("request recursion must fail as a script error, not overflow the worker");
}

/// A script cannot replace a helper for the later scripts on the thread: the helper objects, `System` and their
/// functions are frozen on each engine, so an assignment does nothing in sloppy code and throws in strict code. A
/// `UserSecurityChecks` check replaced by one script would otherwise let a non-member's later write of an admin group
/// membership through.
#[test]
fn the_helper_objects_and_their_functions_are_frozen() -> Result<(), Error> {
    let held = "[ObjectUtilities, CollectionUtilities, StringUtilities, WebUtilities, UserSecurityChecks, System]\
                .every(Object.isFrozen) && Object.isFrozen(UserSecurityChecks.assertPrivilegedGroupMembership) \
                && UserSecurityChecks.assertSealedPermissionGrant.extra === undefined \
                && typeof StringUtilities.base64Encode === 'function' && typeof WebUtilities.encodeHtml === 'function' \
                && typeof System.getProperty === 'function' && CollectionUtilities.added === undefined \
                && ObjectUtilities.added === undefined";
    clear_context_stack();
    run_script(
        "UserSecurityChecks.assertPrivilegedGroupMembership = function () {};
         UserSecurityChecks.assertSealedPermissionGrant.extra = 1;
         StringUtilities.base64Encode = null; WebUtilities.encodeHtml = null;
         CollectionUtilities.added = 1; ObjectUtilities.added = 1; System.getProperty = null;",
    )?;
    assert!(eval_condition(held)?);
    let strict = "(function () { 'use strict'; \
                  UserSecurityChecks.assertPrivilegedGroupMembership = function () {}; })()";
    let err = run_script(strict)
    .expect_err("a strict assignment throws");
    let text = err.to_string();
    assert!(text.contains("read-only") || text.contains("TypeError"), "{text}");
    assert!(eval_condition(held)?);
    Ok(())
}

/// `ResourceReference` is a global of a whole runtime on each engine, as the tools shim makes it on QuickJS's main
/// context: its one static, `isTextFilename`, by the file name's content type, and the global read-only.
#[test]
fn resource_reference_is_a_global_of_each_whole_runtime() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    run_script(
        "seen = [typeof ResourceReference, Object.keys(ResourceReference).join(), \
         ResourceReference.isTextFilename('A.JSON'), ResourceReference.isTextFilename('a.png'), \
         (() => { try { ResourceReference = 1; return 'assigned' } catch (e) { return 'read-only' } })(), \
         typeof ResourceReference].join('|')",
    )?;
    assert_eq!(
        context_value("seen"),
        Some(Value::string("object|isTextFilename|true|false|read-only|object")),
    );
    Ok(())
}


/// The builtins QuickJS's main context had that quickjs-oxide lacks (`queueMicrotask`, `atob`, `btoa`, `performance`,
/// `DOMException`, `SuppressedError`, `DisposableStack`, `AsyncDisposableStack`, `Symbol.dispose` and
/// `Symbol.asyncDispose`) are on the runtime (`oxide/builtins.rs`), each answering as QuickJS's did: its descriptor,
/// length, members, errors and results (the answers QuickJS gave, kept when it was removed).
#[test]
fn the_quickjs_builtins_are_on_the_runtime() -> Result<(), Error> {
    const PROBE: &str = r#"(() => { const out = []; const d = (n) => { try { return n + ': ' + (0, eval)(n) } catch (e) { return n + ' threw ' + e + ' [' + (e && e.name) + ']' } };
  for (const n of ["queueMicrotask","atob","btoa","performance","DOMException","DisposableStack","AsyncDisposableStack","SuppressedError"]) {
    const v = globalThis[n]; const desc = Object.getOwnPropertyDescriptor(globalThis, n);
    if (!desc) { out.push(n + ' MISSING'); continue; }
    out.push(n + ' ' + typeof v + ' len=' + (v && v.length) + ' w' + desc.writable + 'e' + desc.enumerable + 'c' + desc.configurable + ' own=' + Object.getOwnPropertyNames(v).join() + ' proto=' + (v.prototype ? Object.getOwnPropertyNames(v.prototype).sort().join() + '/' + Object.getOwnPropertySymbols(v.prototype).map(String).join() : '-') + ' tag=' + Object.prototype.toString.call(v));
  }
  out.push(d("typeof Symbol.dispose"), d("typeof Symbol.asyncDispose"), d("btoa('hiÿ')"), d("atob('aGk=')"), d("atob('!')"), d("btoa('Ā')"), d("atob(' aG k ')"), d("atob('aGk')"), d("atob('a')"), d("atob(null)"), d("btoa(null)"), d("btoa()"));
  out.push(d("JSON.stringify(Object.getOwnPropertyNames(performance))"), d("typeof performance.now()"), d("performance.now() < 1e7"), d("Object.prototype.toString.call(performance)"), d("Object.getOwnPropertyNames(Object.getPrototypeOf(performance)).join()"));
  out.push(d("(() => { const e = new DOMException('m', 'NotFoundError'); return [e.name, e.message, e.code, e instanceof Error, String(e), Object.keys(e).join(), typeof e.stack, Object.getOwnPropertyNames(e).join()].join('|') })()"));
  out.push(d("(() => { const e = new DOMException(); return [e.name, e.message, e.code].join('|') })()"));
  out.push(d("(() => { const e = new DOMException('x', 'Bogus'); return [e.name, e.message, e.code, DOMException.NOT_FOUND_ERR, DOMException.prototype.NOT_FOUND_ERR].join('|') })()"));
  out.push(d("Object.getOwnPropertyNames(DOMException).join()"));
  out.push(d("(() => { const e = new SuppressedError(1, 2, 'm'); return [e.name, e.message, e.error, e.suppressed, e instanceof Error, Object.getOwnPropertyNames(e).filter(n => n !== 'stack').join(), String(e)].join('|') })()"));
  out.push(d("(() => { const log = []; const s = new DisposableStack(); s.defer(() => log.push('a')); s.use({[Symbol.dispose]() { log.push('b') }}); s.adopt(1, (v) => log.push('c' + v)); s.dispose(); return log.join() + '|' + s.disposed })()"));
  out.push(d("(() => { const s = new DisposableStack(); s.defer(() => { throw new Error('one') }); s.defer(() => { throw new Error('two') }); try { s.dispose() } catch (e) { return [e.name, e.error && e.error.message, e.suppressed && e.suppressed.message].join('|') } })()"));
  out.push(d("(() => { const s = new DisposableStack(); const m = s.move(); return [s.disposed, m.disposed, typeof s[Symbol.dispose], s[Symbol.toStringTag]].join('|') })()"));
  out.push(d("(() => { const s = new AsyncDisposableStack(); return [typeof s.disposeAsync, typeof s[Symbol.asyncDispose], s[Symbol.toStringTag], String(s.disposeAsync())].join('|') })()"));
  out.push(d("(() => { let r; queueMicrotask(() => { r = 1 }); return typeof r })()"));
  out.push(d("(() => { try { queueMicrotask(1) } catch (e) { return String(e) } })()"));
  out.push(d("(() => { try { new DisposableStack().use(1) } catch (e) { return String(e) } })()"));
  out.push(d("(() => { try { DisposableStack() } catch (e) { return e.name } })()"));
  out.push(d("Object.getPrototypeOf(DOMException) === Error"), d("Object.getPrototypeOf(DOMException.prototype) === Error.prototype"),
    d("['IndexSizeError','DOMStringSizeError','HierarchyRequestError','WrongDocumentError','InvalidCharacterError','NoDataAllowedError','NoModificationAllowedError','NotFoundError','NotSupportedError','InUseAttributeError','InvalidStateError','SyntaxError','InvalidModificationError','NamespaceError','InvalidAccessError','ValidationError','TypeMismatchError','SecurityError','NetworkError','AbortError','URLMismatchError','QuotaExceededError','TimeoutError','InvalidNodeTypeError','DataCloneError','EncodingError','NotReadableError'].map(n => new DOMException('', n).code).join()"),
    d("JSON.stringify(Object.getOwnPropertyDescriptor(DOMException, 'NOT_FOUND_ERR'))"), d("JSON.stringify(Object.getOwnPropertyDescriptor(DOMException.prototype, 'NOT_FOUND_ERR'))"),
    d("typeof Object.getOwnPropertyDescriptor(DOMException.prototype, 'name').get"),
    d("(() => { try { atob('!') } catch (e) { return [e instanceof DOMException, e.code, e.name].join() } })()"),
    d("(() => { try { DOMException() } catch (e) { return e.name } })()"),
    d("(() => { const e = SuppressedError(1, 2); return [e instanceof SuppressedError, Object.getOwnPropertyNames(e).filter(n => n !== 'stack').join(), e.message === '', Object.getPrototypeOf(SuppressedError) === Error].join() })()"),
    d("(() => { const e = new DOMException('m', 'NotFoundError'); const s = Object.getOwnPropertyDescriptor(e, 'stack'); return [typeof s.value, s.writable, s.enumerable, s.configurable].join() })()"),
    d("JSON.stringify(Object.getOwnPropertyDescriptor(DisposableStack.prototype, 'disposed').get !== undefined)"),
    d("DisposableStack.prototype[Symbol.dispose] === DisposableStack.prototype.dispose"),
    d("AsyncDisposableStack.prototype[Symbol.asyncDispose] === AsyncDisposableStack.prototype.disposeAsync"),
    d("(() => { const s = new DisposableStack(); s.dispose(); try { s.defer(() => 1) } catch (e) { return String(e) } })()"),
    d("(() => { const s = new DisposableStack(); s.defer(() => { throw new Error('one') }); try { s.dispose() } catch (e) { return String(e) } })()"),
    d("(() => { const e = new SuppressedError(1, 2); return JSON.stringify(Object.getOwnPropertyDescriptor(e, 'error')) })()"),
    d("queueMicrotask.name + queueMicrotask.length + atob.name + btoa.name + (typeof queueMicrotask.prototype)"),
    d("Object.getOwnPropertyNames(Symbol).filter(n => n.includes('ispose')).join()"),
    d("JSON.stringify(Object.getOwnPropertyDescriptor(Symbol, 'dispose'))"),
    d("btoa(String.fromCharCode(0,1,2,255))"), d("atob('AAEC/w==').split('').map(c => c.charCodeAt(0)).join()"), d("atob('AAEC/w=')"), d("atob('AA==A')"), d("atob('A===')"),
    d("[DisposableStack.prototype.dispose.name, DisposableStack.prototype.use.length, DisposableStack.prototype.adopt.length, AsyncDisposableStack.prototype.disposeAsync.name, SuppressedError.prototype.name, DOMException.name, DOMException.length].join()"));
  return out.join('\n') })()"#;
    clear_context_stack();
    let answer = moqui_lib::object::to_plain_string(&eval_value(PROBE)?);
    assert!(!answer.contains("MISSING") && !answer.contains("ReferenceError]"), "{answer}");
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/quickjs-builtins.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, format!("{answer}\n")).unwrap();
    }
    let kept = std::fs::read_to_string(&path).unwrap();
    for (line, (now, then)) in answer.lines().zip(kept.lines()).enumerate() {
        assert_eq!(now, then, "line {}", line + 1);
    }
    assert_eq!(answer.lines().count(), kept.lines().count());
    Ok(())
}

/// The builtins' jobs and stacks do on each engine what QuickJS's do: a `queueMicrotask` callback runs when the jobs
/// run, `move()` hands the resources over, and `disposeAsync()` disposes last first (a null `Symbol.asyncDispose`
/// falling back to `Symbol.dispose`, a null resource awaited), a later error wrapping the earlier one. A script that
/// changes the builtins they would call changes none of that.
#[test]
fn the_builtins_jobs_and_stacks_run_on_each_engine() -> Result<(), Error> {
    const SCRIPT: &str = r#"
        micro = "";
        queueMicrotask(() => { micro = micro + "ran" });
        const calls = [];
        const first = new DisposableStack();
        first.defer(() => calls.push("deferred"));
        const moved = first.move();
        first.dispose();
        const before = calls.length;
        moved.dispose();
        movedCalls = before + ":" + calls.join();
        asyncLog = "";
        const stack = new AsyncDisposableStack();
        stack.defer(() => { asyncLog = asyncLog + "1" });
        stack.use({ [Symbol.asyncDispose]: null, [Symbol.dispose]() { asyncLog = asyncLog + "2" } });
        stack.use(null);
        stack.defer(async () => { asyncLog = asyncLog + "3"; throw new Error("three") });
        stack.defer(() => { asyncLog = asyncLog + "4"; throw new Error("four") });
        stack.disposeAsync().then(() => { asyncLog = asyncLog + "|ok" },
            (e) => { asyncLog = asyncLog + "|" + e.name + ":" + e.error.message + ":" + e.suppressed.message });
    "#;
    const TAMPER: &str = r#"
        const saved = { then: Promise.prototype.then, get: WeakMap.prototype.get, set: WeakMap.prototype.set,
            charCodeAt: String.prototype.charCodeAt };
        Promise.prototype.then = function () { throw new Error("then tampered") };
        WeakMap.prototype.get = function () { return { kind: "DisposableStack", disposed: false, resources: [] } };
        WeakMap.prototype.set = function () { throw new Error("set tampered") };
        String.prototype.charCodeAt = function () { return 65 };
        Object.defineProperty(Object.prototype, "get", { value: () => 1, configurable: true, writable: true });
        Object.defineProperty(Array.prototype, "0", { set() { throw new Error("index tampered") }, configurable: true });
        let seen;
        try {
            const stack = new DisposableStack();
            stack.defer(() => {});
            stack.dispose();
            seen = [btoa("hi"), atob("aGk="), stack.disposed, new DOMException("m", "NotFoundError").code].join("|");
        } catch (e) {
            seen = String(e);
        } finally {
            Promise.prototype.then = saved.then;
            WeakMap.prototype.get = saved.get;
            WeakMap.prototype.set = saved.set;
            String.prototype.charCodeAt = saved.charCodeAt;
            delete Object.prototype.get;
            delete Array.prototype[0];
        }
        tampered = seen;
    "#;
    clear_context_stack();
    push_context_frame();
    run_script(SCRIPT)?;
    assert_eq!(context_value("movedCalls"), Some(Value::string("0:deferred")));
    assert_eq!(context_value("micro"), Some(Value::string("")), "not before the jobs run");
    crate::script::oxide::run_jobs_for_tests()?;
    assert_eq!(context_value("micro"), Some(Value::string("ran")));
    assert_eq!(
        context_value("asyncLog"),
        Some(Value::string("4321|SuppressedError:three:four"))
    );
    run_script(TAMPER)?;
    assert_eq!(context_value("tampered"), Some(Value::string("aGk=|hi|true|8")));
    Ok(())
}

/// A live-form template is not a widget expression: under the widget check it runs with the real `ec` on each engine,
/// as QuickJS's `run_template_function` runs it.
#[test]
fn a_template_under_the_widget_check_reads_the_real_ec() -> Result<(), Error> {
    clear_context_stack();
    let _wrap = super::WidgetWrap::set(true);
    let text = crate::script::template::render_trusted(
        "${typeof ec.entity}",
        crate::script::template::Escape::None,
    )?;
    assert_eq!(text, "object");
    Ok(())
}
