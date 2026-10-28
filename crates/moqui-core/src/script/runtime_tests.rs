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

use super::{
    clear_context_stack, context_value, eval_compiled, eval_condition, eval_source, eval_value, expand, flush_current,
    js_error, push_context_frame, run_script, run_script_location, set_context_name, with_ctx, with_screen_runner,
    MAX_NESTING, STACK, TOO_DEEP,
};
use crate::context::{Error, ExecutionContextFactory};
use moqui_lib::object::{basic_convert, Value};
use rquickjs::Ctx;

const MAX_COEFFICIENT: &str = "79228162514264337593543950335";

fn decimal(text: &str) -> Value {
    let converted = basic_convert(&Value::string(text), "BigDecimal");
    assert!(converted.changed, "expected a decimal from {text}");
    converted.value
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
    set_context_name("ObjectUtilities", Value::string("mine"))?;
    assert!(eval_condition("ObjectUtilities === \"mine\"")?);
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

/// Evaluate `source` in the running script, as a statement between host calls.
fn step(ctx: &Ctx<'_>, source: &str) -> Result<bool, Error> {
    eval_source::<bool>(ctx, source)
}

/// What a host call that may change the stack does first.
fn host_call(ctx: &Ctx<'_>) -> Result<(), Error> {
    flush_current(ctx, true).map_err(js_error)
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
fn a_held_list_survives_a_host_call_that_leaves_it() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("keptList", Value::List(vec![Value::I32(1)]))?;
    with_ctx(|ctx| {
        assert!(step(ctx, "var heldKept = keptList; heldKept.push(2); true")?);
        host_call(ctx)?;
        // Not read again: the push after the call is written back at the end of the script.
        assert!(step(ctx, "heldKept.push(3); true")?);
        host_call(ctx)?;
        assert!(step(ctx, "ec.context.get('keptList').length === 3")?);
        host_call(ctx)?;
        assert!(step(ctx, "keptList === heldKept && keptList.length === 3")?);
        host_call(ctx)?;
        step(ctx, "heldKept.push(4); heldKept = null; true")
    })?;
    holds("keptList.length === 4 && keptList[3] === 4");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_host_call_that_changes_a_held_list_wins() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("replacedList", Value::List(vec![Value::I32(1)]))?;
    set_context_name("appendedList", Value::List(vec![Value::I32(1)]))?;
    with_ctx(|ctx| {
        assert!(step(
            ctx,
            "var heldReplaced = replacedList, heldAppended = appendedList; true"
        )?);
        host_call(ctx)?;
        set_context_name("replacedList", Value::List(vec![Value::I32(9)]))?;
        STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let moqui_lib::context_stack::Lookup::Value(Value::List(mut list)) = stack.get("appendedList") {
                list.push(Value::I32(8));
                stack.put_existing("appendedList", Value::List(list));
            }
        });
        assert!(step(ctx, "heldReplaced.push(2); heldAppended.push(3); true")?);
        assert!(step(
            ctx,
            "replacedList !== heldReplaced && replacedList.length === 1 && replacedList[0] === 9"
        )?);
        step(
            ctx,
            "heldReplaced = heldAppended = null; appendedList.length === 2 && appendedList[1] === 8",
        )
    })
    .map(|seen| assert!(seen))?;
    holds("replacedList.length === 1 && replacedList[0] === 9");
    holds("appendedList.length === 2 && appendedList[1] === 8");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_nested_script_that_changes_a_held_list_wins() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("nestedChanged", Value::List(vec![Value::I32(1)]))?;
    set_context_name("nestedRead", Value::List(vec![Value::I32(1)]))?;
    with_ctx(|ctx| {
        assert!(step(
            ctx,
            "var heldChanged = nestedChanged, heldRead = nestedRead; true"
        )?);
        host_call(ctx)?;
        run_script("nestedChanged.push(5); nestedReadLength = nestedRead.length")?;
        assert!(step(ctx, "heldChanged.push(7); heldRead.push(6); true")?);
        assert!(step(
            ctx,
            "nestedChanged !== heldChanged && nestedChanged.length === 2 && nestedChanged[1] === 5"
        )?);
        step(
            ctx,
            "var same = nestedRead === heldRead && nestedReadLength === 1; heldChanged = heldRead = null; same",
        )
    })
    .map(|same| assert!(same))?;
    holds("nestedChanged.length === 2 && nestedRead.length === 2 && nestedRead[1] === 6");
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

/// The bare-name set hands back the reference rquickjs takes (`keep_assigned`): an extra one each time would keep
/// every assigned value alive, one too few would free it twice.
#[test]
fn a_bare_name_assigned_many_times_does_not_grow_the_runtime() -> Result<(), Error> {
    clear_context_stack();
    let assign = |count: u32| -> Result<i64, Error> {
        run_script(&format!(
            "for (let i = 0; i < {count}; i++) {{ named = 'text ' + i; listed = [i]; held = {{i}} }}"
        ))?;
        Ok(super::engine_memory(true).malloc_count)
    };
    assign(10)?;
    let few = assign(10)?;
    let many = assign(5000)?;
    assert!(many - few < 50, "{few} blocks after 10 assignments, {many} after 5000");
    holds("named === 'text 4999' && listed[0] === 4999 && held.i === 4999");
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
fn context_passed_to_a_host_call_is_the_combined_map() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("outerName", Value::string("outer"))?;
    push_context_frame();
    set_context_name("innerName", Value::I32(2))?;
    with_ctx(|ctx| {
        // A host function that converts its argument as `ec.service` and `ec.resource.expand` convert a map.
        let keys = rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'_>, value: rquickjs::Value<'_>| -> rquickjs::Result<String> {
                let host = super::js_to_host(&ctx, &value)?;
                let Value::Map(map) = host else {
                    return Ok("not a map".to_string());
                };
                let mut keys: Vec<String> = map
                    .iter()
                    .map(|(key, value)| format!("{key}={}", moqui_lib::object::to_plain_string(value)))
                    .collect();
                keys.sort();
                Ok(keys.join(","))
            },
        )
        .map_err(js_error)?;
        // Defined, not set: a set on `globalThis` goes to the context stack.
        ctx.globals()
            .prop("hostMapKeys", rquickjs::object::Property::from(keys))
            .map_err(js_error)?;
        assert!(step(
            ctx,
            "rows = [1]; rows.push(2); seenEntries = hostMapKeys(context); true"
        )?);
        Ok(())
    })?;
    assert_eq!(
        plain("seenEntries"),
        Some("innerName=2,outerName=outer,rows=1,2".to_string())
    );
    // The existing host paths take it too: the map argument of `expand` and `paginateList`.
    holds("ec.resource.expand('${innerName}-${outerName}', '', context) === '2-outer'");
    run_script("CollectionUtilities.paginateList('rows', 'rowsPage', context)")?;
    holds("rowsPage.length === 2 && rowsPageCount === 2");
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
fn an_error_or_a_panic_out_of_a_run_leaves_no_tracked_values() -> Result<(), Error> {
    clear_context_stack();
    set_context_name("heldRows", Value::List(vec![Value::I32(1)]))?;
    let failed = with_ctx(|ctx| -> Result<(), Error> {
        assert!(step(ctx, "heldRows.push(2); true")?);
        Err(Error::new("stopped"))
    });
    assert!(failed.is_err());
    holds("heldRows.length === 2");
    let panicked = std::panic::catch_unwind(|| {
        with_ctx(|ctx| -> Result<(), Error> {
            assert!(step(ctx, "heldRows.length === 2")?);
            panic!("out of the body");
        })
    });
    assert!(panicked.is_err());
    set_context_name("heldRows", Value::List(Vec::new()))?;
    holds("heldRows.length === 0");
    clear_context_stack();
    Ok(())
}

#[test]
fn a_callback_panic_in_a_compiled_script_is_that_scripts_error() -> Result<(), Error> {
    clear_context_stack();
    with_ctx(|ctx| {
        let unguarded =
            rquickjs::Function::new(ctx.clone(), || -> i32 { panic!("unguarded callback") }).map_err(js_error)?;
        // Defined, not set: a set on `globalThis` goes to the context stack.
        ctx.globals()
            .prop("unguardedPanic", rquickjs::object::Property::from(unguarded))
            .map_err(js_error)?;
        let err = eval_compiled::<rquickjs::Value>(ctx, "unguardedPanic()", false).expect_err("panics");
        assert!(err.message().contains("unguarded callback"), "{}", err.message());
        // A panic left in the runtime would be resumed at the next exception rquickjs sees.
        let later = eval_source::<rquickjs::Value>(ctx, "throw new Error('a later error')").expect_err("throws");
        assert!(later.message().contains("a later error"), "{}", later.message());
        Ok(())
    })
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
    // A transaction's own limit replaces the default inside it, and the time inside does not count against it.
    with_ctx(|_| {
        assert!(super::script_time_remaining().is_some_and(|left| left <= Duration::from_millis(200)));
        {
            let _tx = super::DeadlineGuard::new(Some(Duration::from_secs(30)));
            assert!(super::script_time_remaining().is_some_and(|left| left > Duration::from_secs(20)));
            std::thread::sleep(Duration::from_millis(300));
        }
        assert!(super::script_time_remaining().is_some_and(|left| left > Duration::ZERO));
        Ok(())
    })?;
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
fn deep_recursion_in_a_nested_script_is_a_script_error() {
    // Less stack than a worker thread's 2 MiB, so a nested engine with a whole 1 MiB of its own under the outer
    // script's 1 MiB, as before the nested engines had a budget, overflows the thread.
    let outcome = std::thread::Builder::new()
        .stack_size(1536 * 1024)
        .spawn(|| {
            with_ctx(|ctx| {
                let most: i64 = eval_source(
                    ctx,
                    "(function () { let most = 0; function down(n) { most = n; down(n + 1); } \
                        try { down(0); } catch (e) {} return most; })()",
                )?;
                let nested = rquickjs::Function::new(ctx.clone(), || {
                    match run_script("function down(n) { return down(n + 1) + 1; } down(0);") {
                        Ok(()) => "ran".to_string(),
                        Err(err) => err.message().to_string(),
                    }
                })
                .map_err(js_error)?;
                let deep: rquickjs::Function = eval_source(
                    ctx,
                    "(function (nested, depth) { function deep(n) { return n === 0 ? nested() : deep(n - 1); } \
                        return deep(depth); })",
                )?;
                deep.call::<_, String>((nested, most * 9 / 10)).map_err(js_error)
            })
        })
        .unwrap()
        .join()
        .expect("the thread did not overflow");
    let message = outcome.unwrap();
    assert!(message.contains("stack"), "{message}");
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

#[test]
fn running_out_of_memory_frees_the_engine_caches() -> Result<(), Error> {
    clear_context_stack();
    assert!(eval_condition("1 + 1 === 2")?);
    let cached = || super::ENGINES.with(|engines| engines.borrow()[0].scripts.borrow().len());
    assert!(cached() > 0);
    let err = run_script("(function () { const hog = []; while (true) hog.push(new Array(100000).fill(1)); })()")
        .expect_err("out of memory");
    assert!(err.message().contains("out of memory"), "{}", err.message());
    assert_eq!(cached(), 0, "the caches went with the failed script");
    assert!(eval_condition("1 + 1 === 2")?, "the engine runs again");
    Ok(())
}
