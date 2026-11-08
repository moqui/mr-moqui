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

//! The oxide engine itself, run directly (not through the router or the static check).

use std::sync::Arc;

use moqui_lib::object::{basic_convert, HostObject, Value};

use super::{eval_condition, eval_value, Outcome};
use crate::context::Error;
use crate::script::text::hash_text;
use crate::script::{clear_context_stack, push_context_frame, set_context_name};

fn value(source: &str) -> Result<Value, Error> {
    match eval_value(source, hash_text(source), false) {
        Ok(value) => Ok(value),
        Err(Outcome::Error(err)) => Err(err),
        Err(Outcome::Fallback(reason)) => Err(Error::new(format!("fallback: {reason}"))),
    }
}

fn condition(source: &str) -> bool {
    match eval_condition(source, hash_text(source), false) {
        Ok(value) => value,
        Err(Outcome::Error(err)) => panic!("{source}: {}", err.message()),
        Err(Outcome::Fallback(reason)) => panic!("{source}: fallback {reason}"),
    }
}

/// [`value`] of a text that writes: its tracked values are written back at its end.
fn written(source: &str) -> Result<Value, Error> {
    match eval_value(source, hash_text(source), true) {
        Ok(value) => Ok(value),
        Err(Outcome::Error(err)) => Err(err),
        Err(Outcome::Fallback(reason)) => Err(Error::new(format!("fallback: {reason}"))),
    }
}

/// A list or map a run read by name and changed is written back at its end; one it did not change is left as the
/// stack holds it; a bare assignment puts the value on the top frame, and a plain object stays the script's.
#[test]
fn tracked_values_are_written_back_at_the_end() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("items", Value::List(vec![Value::I32(2), Value::I32(1)]))?;
    let mut map = indexmap::IndexMap::new();
    map.insert("a".to_string(), Value::I32(1));
    set_context_name("map", Value::Map(map))?;
    assert_eq!(written("items.sort()[0]")?, Value::I32(1));
    assert_eq!(
        crate::script::context_value("items"),
        Some(Value::List(vec![Value::I32(1), Value::I32(2)]))
    );
    assert_eq!(
        written("(() => { items.push(3); map.b = items.length; return 0 })()")?,
        Value::I32(0)
    );
    assert_eq!(
        crate::script::context_value("items"),
        Some(Value::List(vec![Value::I32(1), Value::I32(2), Value::I32(3)]))
    );
    let Some(Value::Map(map)) = crate::script::context_value("map") else {
        panic!("a map");
    };
    assert_eq!(map.get("b"), Some(&Value::I32(3)));
    assert_eq!(
        written("(() => { made = {x: 1}; made.y = 2; return 0 })()")?,
        Value::I32(0)
    );
    let Some(Value::Map(made)) = crate::script::context_value("made") else {
        panic!("the assigned map");
    };
    assert_eq!(
        made.get("y"),
        Some(&Value::I32(2)),
        "a plain object assigned stays the script's, tracked"
    );
    // a read-only run writes nothing and the stack keeps its own value
    let before = super::engine::changed_copies();
    assert_eq!(value("items.length")?, Value::I32(3));
    assert_eq!(super::engine::changed_copies(), before);
    Ok(())
}

/// A host function that lends the engine (the fork's F3a) runs a nested small script on the runtime itself, with the
/// outer run's values set aside and given back.
#[test]
fn a_script_nested_under_a_lend_runs_on_oxide() -> Result<(), Error> {
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    set_context_name("n", Value::I32(1))?;
    set_context_name("items", Value::List(vec![Value::I32(1)]))?;
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    let oxide_values = || crate::script::engine::runs_here(crate::script::engine::EntryKind::Value);
    context
        .define_function(&global, "lent", 0, move |scope: &mut Scope<'_>| -> Result<JsValue, _> {
            match scope.lend(|| crate::script::eval_value("n + items.length")) {
                Ok(Value::I32(number)) => Ok(JsValue::Int(number)),
                other => Err(scope.throw_error(
                    quickjs_oxide::engine::api::core::NativeErrorKind::Type,
                    &format!("{other:?}"),
                )),
            }
        })
        .unwrap();
    let before = oxide_values();
    assert_eq!(value("items.length + lent() + items.length")?, Value::I32(4));
    assert_eq!(oxide_values(), before + 1, "the nested script ran");
    Ok(())
}

fn decimal(text: &str) -> Value {
    basic_convert(&Value::string(text), "BigDecimal").value
}

#[test]
fn bare_names_read_the_context_stack() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("orderId", Value::string("2000"))?;
    set_context_name("count", Value::I64(7))?;
    set_context_name("flag", Value::Bool(true))?;
    set_context_name("items", Value::List(vec![Value::I32(1), Value::string("b")]))?;
    let mut map = indexmap::IndexMap::new();
    map.insert("a".to_string(), Value::I32(1));
    set_context_name("map", Value::Map(map))?;
    assert!(condition("orderId === \"2000\""));
    assert!(condition("count === 7 && flag"));
    assert!(condition("items.length === 2 && items[1] === 'b'"));
    assert!(condition("map.a === 1"));
    assert!(
        condition("items === items && map == map"),
        "one object per name in a run"
    );
    assert!(!condition("unsetBareName"));
    assert!(condition("unsetBareName === null && typeof unsetBareName === 'object'"));
    assert!(condition("typeof Array === 'function'"));
    assert_eq!(value("count + 1")?, Value::I32(8));
    assert_eq!(value("items")?, Value::List(vec![Value::I32(1), Value::string("b")]));
    Ok(())
}

#[test]
fn values_convert_back_as_on_quickjs() -> Result<(), Error> {
    clear_context_stack();
    assert_eq!(value("1.5 * 2")?, Value::I64(3));
    assert_eq!(value("0.5")?, Value::F64(0.5));
    assert_eq!(value("'x'")?, Value::string("x"));
    assert_eq!(value("null")?, Value::Null);
    assert_eq!(value("undefined")?, Value::Null);
    let map = value("({a: 1, f: () => 1, b: [true]})")?;
    let Value::Map(map) = map else {
        panic!("a map: {map:?}");
    };
    assert_eq!(map.keys().collect::<Vec<_>>(), ["a", "b"], "a method is not data");
    let set = value("new Set([1, 2])")?;
    let Value::Map(set) = set else {
        panic!("a Set: {set:?}");
    };
    assert_eq!(
        set.get("__set__"),
        Some(&Value::List(vec![Value::I32(1), Value::I32(2)]))
    );
    assert!(matches!(value("new Date(0)")?, Value::Timestamp(_)));
    assert_eq!(value("new Uint8Array([1, 2])")?, Value::Bytes(vec![1, 2]));
    let err = value("(() => 1)").unwrap_err();
    assert_eq!(err.message(), "cannot convert this value");
    let err = value("(() => { var a = []; a.push(a); return a; })()").unwrap_err();
    assert_eq!(err.message(), crate::script::runtime::TOO_DEEP);
    Ok(())
}

#[test]
fn an_integer_past_the_safe_range_is_an_error() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("smallest", Value::I64(i64::MIN))?;
    let err = value("smallest < 0").unwrap_err();
    assert!(err.message().contains("too large"), "{}", err.message());
    set_context_name("safe", Value::I64(-9_007_199_254_740_991))?;
    assert!(condition("safe === -Number.MAX_SAFE_INTEGER"));
    Ok(())
}

#[test]
fn decimals_are_host_objects_with_the_quickjs_methods() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("price", decimal("10.25"))?;
    set_context_name("zero", decimal("0"))?;
    assert!(condition("zero"), "a decimal is true");
    assert!(condition("zero.isZero()"));
    assert!(condition("price == price"), "a name read twice is one object");
    assert_eq!(value("price.add(1)")?, decimal("11.25"));
    assert_eq!(value("price.multiply('2')")?, decimal("20.50"));
    assert_eq!(value("price.subtract(price)")?, decimal("0.00"));
    assert_eq!(value("price.divide(4, 2, 'HALF_UP')")?, decimal("2.56"));
    assert_eq!(value("price.setScale(1, 'HALF_UP').toString()")?, Value::string("10.3"));
    assert_eq!(value("price.compare(11)")?, Value::I32(-1));
    assert!(condition("price.eq('10.250')"));
    assert_eq!(value("price.scale()")?, Value::I32(2));
    assert_eq!(value("price.negate().abs()")?, decimal("10.25"));
    assert_eq!(value("price.toNumber()")?, Value::F64(10.25));
    assert_eq!(value("JSON.stringify({p: price})")?, Value::string("{\"p\":10.25}"));
    let err = value("price + 1").unwrap_err();
    assert!(
        err.message().contains("decimal valueOf is not supported"),
        "{}",
        err.message()
    );
    let err = value("price.add('abc')").unwrap_err();
    assert!(err.message().contains("a decimal method takes"), "{}", err.message());
    let err = value("price.setScale(1)").unwrap_err();
    assert!(err.message().contains("rounding necessary"), "{}", err.message());
    let err = value("price.divide(1, 2, 'SIDEWAYS')").unwrap_err();
    assert!(
        err.message().contains("rounding mode SIDEWAYS is not known"),
        "{}",
        err.message()
    );
    Ok(())
}

#[test]
fn an_error_has_the_quickjs_form_with_its_location() {
    clear_context_stack();
    let err = value("null.x").unwrap_err();
    assert!(err.message().starts_with("Error: "), "{}", err.message());
    assert!(err.message().contains("eval_script:1:"), "{}", err.message());
    let err = value("(1).foo()").unwrap_err();
    assert!(
        err.message()
            .starts_with("Error: not a function\n    at <eval> (eval_script:1:"),
        "{}",
        err.message()
    );
}

#[test]
fn the_deadline_interrupts_an_oxide_script() {
    clear_context_stack();
    let started = std::time::Instant::now();
    let _deadline = crate::script::DeadlineGuard::new(Some(std::time::Duration::from_millis(50)));
    let err = value("(() => { while (true) {} })()").unwrap_err();
    assert!(err.message().contains("interrupted"), "{}", err.message());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn local_time_is_the_process_zone() -> Result<(), Error> {
    clear_context_stack();
    let offset = super::host::local_offset_minutes(0.0);
    assert_eq!(value("new Date(0).getTimezoneOffset()")?, Value::I32(offset));
    Ok(())
}

/// The runtime starts with every host shim installed, as QuickJS's does (`runtime.rs`, `install_context`): the tools
/// wrappers, `EdiHandler`, `SftpClient`, `sri`'s URL functions and what the tools shim adds under `ec`; a shim's global
/// is the global object's own, not a context name.
#[test]
fn every_host_shim_is_installed_on_the_oxide_runtime() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let kinds = value(
        "[typeof __moquiWrap, typeof EdiHandler, typeof SftpClient, typeof sri.buildUrl, \
         typeof ResourceReference.isTextFilename, typeof ec.context.putAll, typeof ec.cache.cacheExists, \
         typeof ec.entity.isEntityDefined].join()",
    )?;
    assert_eq!(kinds, Value::string(["function"; 8].join(",")));
    assert_eq!(crate::script::context_value("EdiHandler"), None, "not a context name");
    Ok(())
}

/// A tools map (a map with `__wrap`) a host call returns, or the stack holds, reads its members on oxide as on QuickJS:
/// the tools shim's wrapper gives it its methods, and it goes back to the host as its data.
#[test]
fn a_tools_map_from_a_host_call_reads_its_members() -> Result<(), Error> {
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    let mut text = indexmap::IndexMap::new();
    text.insert("__wrap".to_string(), Value::string("Text"));
    text.insert("text".to_string(), Value::string("a rule"));
    set_context_name("rule", Value::Map(text))?;
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    context
        .define_function(&global, "resourceRef", 0, |scope: &mut Scope<'_>| -> Result<JsValue, _> {
            let mut reference = indexmap::IndexMap::new();
            reference.insert("__wrap".to_string(), Value::string("Resource"));
            reference.insert("location".to_string(), Value::string("component://x/page.md"));
            reference.insert("fileName".to_string(), Value::string("page.md"));
            reference.insert("exists".to_string(), Value::Bool(false));
            reference.insert("file".to_string(), Value::Bool(true));
            let engine = super::engine::current().expect("the engine");
            super::convert::to_js(scope, &engine, &Value::Map(reference)).map_err(|conv| match conv {
                super::convert::Conv::Js(err) => err,
                _ => scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, "not converted"),
            })
        })
        .unwrap();
    assert_eq!(
        value("(() => { const ref = resourceRef(); return [ref.getFileName(), ref.getLocation(), ref.exists, ref.isFile()].join() })()")?,
        Value::string("page.md,component://x/page.md,false,true")
    );
    assert_eq!(value("rule.toString()")?, Value::string("a rule"));
    let Value::Map(back) = value("resourceRef()")? else {
        panic!("not a map");
    };
    assert_eq!(back.get("__wrap"), Some(&Value::string("Resource")));
    assert_eq!(back.get("fileName"), Some(&Value::string("page.md")));
    Ok(())
}

/// The runtime runs whole scripts, as QuickJS's main context did: `Function` and the rest are whole.
#[test]
fn the_runtime_runs_whole_scripts() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    (|| {
        assert_eq!(value("Function('return 6 * 7')()")?, Value::I32(42));
        assert_eq!(
            value("[typeof Promise, typeof Symbol(), typeof Reflect.ownKeys, typeof new Proxy({}, {})].join()")?,
            Value::string("function,symbol,function,object")
        );
        assert_eq!(value("typeof sri.encodeHtml")?, Value::string("function"));
        assert_eq!(value("(sri = 1, typeof sri)")?, Value::string("object"), "sri is read-only");
        // a Set the stack holds is a Set, tracked and written back
        let set = |items: Vec<Value>| {
            Value::Map(indexmap::IndexMap::from([(
                crate::script::runtime::SET_KEY.to_string(),
                Value::List(items),
            )]))
        };
        set_context_name("seen", set(vec![Value::I32(1)]))?;
        crate::script::run_script("if (!(seen instanceof Set)) throw new Error('not a Set'); seen.add(2)")?;
        assert_eq!(crate::script::context_value("seen"), Some(set(vec![Value::I32(1), Value::I32(2)])));
        Ok::<_, Error>(())
    })()?;
    Ok(())
}

/// The debug safety net: a list an oxide run changed in place (here past the check, run directly) is logged.
#[test]
fn a_changed_copy_of_a_context_value_is_noticed() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("items", Value::List(vec![Value::I32(2), Value::I32(1)]))?;
    let before = super::engine::changed_copies();
    assert_eq!(value("items.sort()[0]")?, Value::I32(1));
    assert_eq!(super::engine::changed_copies(), before + 1);
    Ok(())
}

#[test]
fn an_entity_value_reads_its_fields() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(Arc::from("statusId"), Value::string("OrderPlaced"));
    fields.insert(Arc::from("grandTotal"), decimal("12.50"));
    let entity = crate::entity::EntityValue::from_read_fields(Arc::from("mantle.order.OrderHeader"), fields);
    let cell: Arc<crate::script::runtime::EntityCell> = Arc::new(std::sync::Mutex::new(entity));
    set_context_name("order", Value::Object(HostObject(cell.clone())))?;
    assert!(condition("order.statusId === 'OrderPlaced'"));
    assert!(condition("order.grandTotal.compare(10) > 0"));
    assert!(condition("order === order"));
    let back = value("order")?;
    let Value::Object(back) = back else {
        panic!("the entity value: {back:?}");
    };
    assert!(back
        .downcast::<crate::script::runtime::EntityCell>()
        .is_some_and(|back| Arc::ptr_eq(&back, &cell)));
    // the methods are the host table's, on the prototype, as on QuickJS
    assert_eq!(value("order.getEntityName()")?, Value::string("mantle.order.OrderHeader"));
    assert_eq!(value("order.getMap().statusId")?, Value::string("OrderPlaced"));
    assert!(condition(
        "typeof order.entrySet === 'function' && order.entrySet()[0].getKey() === 'grandTotal' \
         && order.set.length === 2 && !Object.keys(order).includes('set')"
    ));
    // the exotic hooks: keys, `in`, own property reads and JSON see the fields; other names are Object.prototype's
    assert_eq!(
        value("Object.keys(order).join()")?,
        Value::string("grandTotal,statusId")
    );
    assert!(condition("'statusId' in order && !('nothing' in order)"));
    assert_eq!(
        value("JSON.stringify(order)")?,
        Value::string("{\"grandTotal\":12.5,\"statusId\":\"OrderPlaced\"}")
    );
    assert!(condition(
        "typeof order.toString === 'function' && order.nothing === undefined"
    ));
    assert!(condition(
        "Object.getOwnPropertyDescriptor(order, 'statusId').value === 'OrderPlaced'"
    ));
    // a set through a value on another object's prototype chain defines on that object, not on the entity
    assert!(condition(
        "(() => { const o = Object.create(order); o.statusId = 'X'; return o.statusId === 'X' })()"
    ));
    assert!(condition("order.statusId === 'OrderPlaced'"));
    // a field set on the value itself writes the cell the stack shares
    assert_eq!(
        value("(() => { order.statusId = 'OrderApproved'; return order.statusId })()")?,
        Value::string("OrderApproved")
    );
    assert_eq!(
        cell.lock().unwrap().get("statusId").cloned(),
        Some(Value::string("OrderApproved"))
    );
    Ok(())
}

/// The entity value methods the oxide side sends back to QuickJS are the QuickJS entity prototype's own names.
#[test]
fn the_entity_method_list_is_the_quickjs_prototypes() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let entity =
        crate::entity::EntityValue::from_read_fields(Arc::from("moqui.Test"), std::collections::BTreeMap::new());
    set_context_name("value", Value::Object(HostObject::new(std::sync::Mutex::new(entity))))?;
    let names = {
        crate::script::eval_value("Object.getOwnPropertyNames(Object.getPrototypeOf(value)).sort().join(',')")
    }?;
    let names = moqui_lib::object::to_plain_string(&names);
    let quickjs: Vec<&str> = names.split(',').collect();
    let mut listed: Vec<&str> = super::names::ENTITY_METHODS.to_vec();
    listed.sort_unstable();
    assert_eq!(quickjs, listed);
    Ok(())
}

/// A text run twice on one thread leaves its code image for the process (plans/archive/Script-Engine-Oxide.md section 6, F1):
/// another thread's first run of it loads the image and parses nothing.
#[test]
fn a_second_threads_first_run_of_a_text_does_no_parse() -> Result<(), Error> {
    use quickjs_oxide::engine::api::core::front_end_parses;
    let source = "[1, 2, 3].map(n => n * 2).join('-') + ' images ' + 7";
    let expected = Value::String("2-4-6 images 7".to_string());
    assert_eq!(value(source)?, expected);
    let before = super::compiled::image_stats();
    assert_eq!(value(source)?, expected);
    assert!(super::compiled::image_stats().made > before.made, "the second run makes the image");
    let other = std::thread::spawn(move || {
        // the runtime's start parses its own scripts; the text's first run here parses nothing
        super::engine::context_for_tests();
        let parses = front_end_parses();
        let result = value(source).map_err(|err| err.message().to_string());
        (front_end_parses() - parses, result)
    })
    .join()
    .expect("the thread runs");
    assert_eq!(other, (0, Ok(expected)));
    assert!(super::compiled::image_stats().loads > before.loads);
    Ok(())
}

/// What the bindings lack is no exception a script can act on: a `catch` of it reads and writes no context name, and
/// the run falls back with nothing written (the Actions profile allows `try`) and stops at its next poll.
#[test]
fn a_fallback_is_not_caught_by_the_script() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("cond", Value::Object(HostObject::new(String::from("not a binding"))))?;
    let source = "(function() { try { r = cond.x } catch (e) { r = 'caught' } finally { f = 'ran' } after = 1 })()";
    let outcome = super::run_actions(source, hash_text(source), "catch.js");
    assert!(matches!(outcome, Err(Outcome::Fallback(_))), "falls back");
    assert_eq!(crate::script::context_value("r"), None);
    assert_eq!(crate::script::context_value("after"), None);
    Ok(())
}

/// An actions script of the wider profile (`for...of`, `for...in`, `try`, a function only called, a block-bodied
/// arrow) runs on oxide and gives what it gives on QuickJS.
#[test]
fn loops_over_values_try_and_local_functions_run_alike() -> Result<(), Error> {
    use crate::script::engine::{runs_here, EntryKind};
    let actions = r#"<actions><script><![CDATA[
        function label(row) { const {name, qty = 0} = row; return name + ':' + qty }
        labels = []
        for (const row of rows) labels.push(label(row))
        keys = ''
        for (const key in totals) { if (key === 'skip') continue; keys += key }
        try { missing.field = 1 } catch (e) { failed = e instanceof TypeError } finally { done = true }
        sums = rows.map((row) => { let total = 0; for (const [, v] of Object.entries(row)) if (typeof v === 'number') total += v; return total })
    ]]></script></actions>"#;
    clear_context_stack();
    push_context_frame();
    let row = |name: &str, qty: Option<i32>| {
        let mut map = indexmap::IndexMap::new();
        map.insert("name".to_string(), Value::string(name));
        if let Some(qty) = qty {
            map.insert("qty".to_string(), Value::I32(qty));
        }
        Value::Map(map)
    };
    set_context_name("rows", Value::List(vec![row("a", Some(2)), row("b", None)]))?;
    let mut totals = indexmap::IndexMap::new();
    for key in ["x", "skip", "y"] {
        totals.insert(key.to_string(), Value::I32(1));
    }
    set_context_name("totals", Value::Map(totals))?;
    let before = runs_here(EntryKind::Actions);
    crate::script::run_actions(actions)?;
    assert_eq!(runs_here(EntryKind::Actions), before + 1, "counted as actions");
    assert_eq!(
        crate::script::context_value("labels"),
        Some(Value::List(vec![Value::string("a:2"), Value::string("b:0")]))
    );
    assert_eq!(crate::script::context_value("keys"), Some(Value::string("xy")));
    assert_eq!(crate::script::context_value("failed"), Some(Value::Bool(true)));
    assert_eq!(crate::script::context_value("done"), Some(Value::Bool(true)));
    assert_eq!(
        crate::script::context_value("sums"),
        Some(Value::List(vec![Value::I32(2), Value::I32(0)]))
    );
    Ok(())
}

/// `context` is the stack on oxide as on QuickJS: a name read and written through it, `in`, its keys, the `Map`
/// methods for a name no frame holds, `delete` from the top frame, and the whole stack as a map handed to a host call.
#[test]
fn context_is_the_stack_as_on_quickjs() -> Result<(), Error> {
    use crate::script::engine::{runs_here, EntryKind};
    let actions = r#"<actions><script><![CDATA[
        context.made = context.base + 1
        has = ['base' in context, 'context' in context, 'nothing' in context, context.containsKey('base')]
        keys = Object.keys(context).filter((key) => key === 'base' || key === 'made').sort()
        old = context.put('put', 'a name')
        got = [context.get('put'), context.get('missing')]
        context.putAll({fromMap: 2})
        delete context.made
        gone = context.made === null
    ]]></script></actions>"#;
    (|| -> Result<(), Error> {
        clear_context_stack();
        push_context_frame();
        set_context_name("base", Value::I32(41))?;
        let before = runs_here(EntryKind::Actions);
        crate::script::run_actions(actions)?;
        assert_eq!(runs_here(EntryKind::Actions), before + 1, "counted as actions");
        let value = |name: &str| crate::script::context_value(name);
        assert_eq!(
            value("has"),
            Some(Value::List(vec![Value::Bool(true), Value::Bool(true), Value::Bool(false), Value::Bool(true)]))
        );
        assert_eq!(
            value("keys"),
            Some(Value::List(vec![Value::string("base"), Value::string("made")]))
        );
        assert_eq!(value("old"), Some(Value::Null));
        assert_eq!(value("got"), Some(Value::List(vec![Value::string("a name"), Value::Null])));
        assert_eq!(value("fromMap"), Some(Value::I32(2)));
        assert_eq!(value("gone"), Some(Value::Bool(true)));
        Ok::<(), Error>(())
    })()?;
    // passed on as a value, the stack is the map of the visible names
    clear_context_stack();
    push_context_frame();
    set_context_name("base", Value::I32(41))?;
    let whole = value("context")?;
    assert!(
        matches!(&whole, Value::Map(map) if map.get("base") == Some(&Value::I32(41))),
        "{whole:?}"
    );
    Ok(())
}

/// A read-only run that reads a tools map by name: its wrapper's members (a Node's `childNodes` and `attributes`, a
/// Resource's `size` and `lastModified` getters, read by a host call) are no change of the script's, so the debug check
/// of the unflushed run notes no changed copy.
#[test]
fn a_tools_map_read_by_a_read_only_run_is_not_a_changed_copy() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let mut reference = indexmap::IndexMap::new();
    reference.insert("__wrap".to_string(), Value::string("Resource"));
    reference.insert("location".to_string(), Value::string("component://none/page.md"));
    reference.insert("fileName".to_string(), Value::string("page.md"));
    reference.insert("exists".to_string(), Value::Bool(true));
    reference.insert("file".to_string(), Value::Bool(true));
    set_context_name("ref", Value::Map(reference))?;
    let mut kid = indexmap::IndexMap::new();
    kid.insert("__wrap".to_string(), Value::string("Node"));
    kid.insert("name".to_string(), Value::string("kid"));
    let mut node = indexmap::IndexMap::new();
    node.insert("__wrap".to_string(), Value::string("Node"));
    node.insert("name".to_string(), Value::string("root"));
    node.insert("children".to_string(), Value::List(vec![Value::Map(kid)]));
    set_context_name("node", Value::Map(node))?;
    set_context_name("nodes", Value::List(vec![crate::script::context_value("node").unwrap()]))?;
    let copies = super::engine::changed_copies();
    assert_eq!(
        value("ref.getFileName() + ':' + node.first().name + ':' + nodes[0].children('kid').length")?,
        Value::string("page.md:kid:1")
    );
    assert_eq!(super::engine::changed_copies(), copies, "no changed copy");
    Ok(())
}

/// The runtime's start is not cut off by a script deadline that has passed (a start nested in a QuickJS script under
/// its deadline): the interrupt does not stop it while it starts, and the start itself gets through. A script run
/// during the start (from a shim's host call) is a script again, which the deadline stops.
#[test]
fn the_deadline_does_not_stop_the_runtimes_start() {
    use quickjs_oxide::engine::api::core::Host;
    super::engine::restart_for_tests(false);
    let passed = std::time::Instant::now() - std::time::Duration::from_secs(1);
    crate::script::run::DEADLINE.with(|deadline| deadline.set(Some(passed)));
    assert!(super::host::MoquiHost.interrupt(), "a script past its deadline stops");
    let started = {
        let _starting = super::engine::starting_for_tests();
        assert!(!super::host::MoquiHost.interrupt(), "the start goes on");
        super::engine::start_for_tests().map(|_| {
            // a script nested in the start
            value("(() => { let n = 0; for (let i = 0; i < 1000000; i++) n++; return n })()")
        })
    };
    crate::script::run::DEADLINE.with(|deadline| deadline.set(None));
    let nested = started.unwrap_or_else(|err| panic!("the start: {}", err.message()));
    assert!(nested.is_err(), "the nested script ran past its deadline: {nested:?}");
}

/// The globals the host shims make on oxide are read-only, as QuickJS makes them: an assignment throws, in sloppy code
/// too, and the global keeps its value for the later scripts.
#[test]
fn the_host_globals_cannot_be_assigned() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    for name in ["EdiHandler", "SftpClient", "__moquiWrap", "ResourceReference"] {
        let source =
            format!("(() => {{ try {{ {name} = 1; return 'assigned' }} catch (e) {{ return String(e) }} }})()");
        let Value::String(got) = value(&source)? else {
            panic!("{source}: not text");
        };
        assert!(got.contains("is a host global and cannot be assigned"), "{name}: {got}");
        let kind = if name == "ResourceReference" { "object" } else { "function" };
        assert_eq!(value(&format!("typeof {name}"))?, Value::string(kind), "{name}");
    }
    Ok(())
}

/// A context name the text does not bind is the run's own copy: a function cannot be put on the stack, so
/// `context.f = fn; f.x = 1` (which the static check lets through) changes no builtin.
#[test]
fn a_function_put_in_the_context_is_not_read_back_as_itself() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let source = "(function() { try { context.f = ''.toString } catch (e) {} try { f.x = 1 } catch (e) {} })()";
    let _ = super::run_actions(source, hash_text(source), "put_function");
    assert_eq!(value("typeof ''.toString.x")?, Value::string("undefined"));
    Ok(())
}

/// A panic that unwinds through a nested run in `ec.resource`'s lend goes on to the embedder, as through a table
/// method's lend: the script's `catch` does not see it.
#[test]
fn a_panic_through_a_resource_lend_is_not_the_scripts_to_catch() {
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    {
        let context = super::engine::context_for_tests();
        let global = context.global_object().unwrap();
        context
            .define_function(&global, "boom", 0, |_: &mut Scope<'_>| -> Result<JsValue, _> {
                panic!("boom in a nested run")
            })
            .unwrap();
        let ran = std::panic::catch_unwind(|| {
            value("(() => { try { return ec.resource.expression('boom()') } catch (e) { return 'caught' } })()")
        });
        assert!(ran.is_err(), "the script caught a panic: {ran:?}");
        super::engine::restart_for_tests(false);
    }
}

/// Script web handlers on oxide: the invoker is made once per runtime, a listener is called by its event, and what a
/// handler throws, or a file that is no handler, is the request's error.
#[test]
fn web_handlers_run_on_oxide() -> Result<(), Error> {
    let dir = std::env::temp_dir().join(format!("moqui-oxide-web-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let listener = dir.join("Listener.js");
    std::fs::write(&listener, "({ started(event, config) { throw new Error('heard ' + event.type + ' ' + config.name) } })")
        .unwrap();
    let not_handler = dir.join("Number.js");
    std::fs::write(&not_handler, "42").unwrap();
    let data = |location: &std::path::Path| {
        Value::Map(indexmap::IndexMap::from([
            ("name".to_string(), Value::string("probe")),
            ("location".to_string(), Value::string(location.to_string_lossy())),
            ("event".to_string(), Value::string("started")),
        ]))
    };
    (|| {
        clear_context_stack();
        for _ in 0..2 {
            let location = listener.to_string_lossy();
            let err = super::web::invoke(&location, "listener", data(&listener)).unwrap_err();
            assert!(err.message().contains("heard started probe"), "{}", err.message());
        }
        assert!(super::engine::start_for_tests()?.web_invoker().get().is_some(), "the invoker is kept");
        let err = super::web::invoke(&not_handler.to_string_lossy(), "servlet", data(&not_handler)).unwrap_err();
        assert!(err.message().contains("must evaluate to a handler object"), "{}", err.message());
        Ok::<_, Error>(())
    })()?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// `ec.resource` on oxide: an additional-context map is a frame of its own for the text and gone after it; an inert
/// module's value is the same within a run and made again in the next.
#[test]
fn resource_texts_and_modules_on_oxide() -> Result<(), Error> {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    let dir = std::env::temp_dir().join(format!("moqui-oxide-modules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let inert = dir.join("Inert.js");
    std::fs::write(&inert, "function twice(n) { return n * 2 }\n({twice})\n").unwrap();
    clear_context_stack();
    push_context_frame();
    set_context_name("b", Value::I32(2))?;
    set_context_name("where", Value::string(inert.to_string_lossy()))?;
    (|| {
        crate::script::run_script(
            "expanded = ec.resource.expand('${a}-${b}', null, {a: 1}); \
             plain = ec.resource.expandNoL10n('${b}'); \
             held = ec.resource.condition('a === 1 && b === 2', null, {a: 1}); \
             valued = ec.resource.expression('a + b', null, {a: 40}); \
             const one = ec.resource.script(where); \
             same = one === ec.resource.script(where) && one.twice(2) === 4; \
             globalThis.keptModule = one",
        )?;
        crate::script::run_script("again = ec.resource.script(where) === globalThis.keptModule")?;
        Ok::<_, Error>(())
    })()?;
    let read = |name: &str| crate::script::context_value(name);
    assert_eq!(read("expanded"), Some(Value::string("1-2")));
    assert_eq!(read("plain"), Some(Value::string("2")));
    assert_eq!(read("held"), Some(Value::Bool(true)));
    assert_eq!(read("valued"), Some(Value::I32(42)));
    assert_eq!(read("a"), None, "the map's frame is gone");
    assert_eq!(read("same"), Some(Value::Bool(true)), "kept within the run");
    assert_eq!(read("again"), Some(Value::Bool(false)), "made again in the next run");
    clear_context_stack();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// A top-level render is one run with one deadline: its units run with the depth they are given (back after a unit
/// that throws), a script between them runs nested in the render, and what a unit wrote by name is on the stack.
#[test]
fn a_top_level_render_is_one_run() -> Result<(), Error> {
    let unit = |id: u64, source: &str| crate::screen::generate::Unit {
        id,
        source: source.to_string(),
        runtime: Arc::from(""),
        runtime_id: 0,
    };
    clear_context_stack();
    push_context_frame();
    crate::script::with_screen_runner(|runner| {
        // the render is the run: one depth, one limit armed at its start
        assert!(crate::script::run::is_running(crate::script::run::EngineTag::Oxide), "the render's run");
        assert_eq!(crate::script::run::depth(), 1);
        runner.run(&unit(u64::MAX - 1, "(function ($depth) { firstDepth = $depth })"))?;
        let thrown = runner.run(&unit(u64::MAX - 2, "(function ($depth) { sri.pushContext(); throw new Error('unit failed') })"));
        assert!(thrown.unwrap_err().message().contains("unit failed"));
        assert_eq!(crate::script::eval_value("firstDepth + 1")?, Value::I32(1), "a script nested in the render");
        runner.run(&unit(u64::MAX - 3, "(function ($depth) { secondDepth = $depth })"))
    })?;
    assert_eq!(crate::script::context_value("firstDepth"), Some(Value::I32(0)));
    assert_eq!(crate::script::context_value("secondDepth"), Some(Value::I32(0)), "the depth is back after a throw");
    Ok(())
}

/// A tracked list of plain maps is compared with the stack in place at a run's end; one with code in it (an accessor,
/// a Proxy's trap) is compared on a copy with the stack free, since the code may write the stack.
#[test]
fn a_tracked_value_with_code_in_it_is_compared_with_the_stack_free() -> Result<(), Error> {
    let row = |a: i32| Value::Map([("a".to_string(), Value::I32(a))].into_iter().collect());
    clear_context_stack();
    push_context_frame();
    crate::script::set_context_name("rows", Value::List(vec![row(1), row(2)]))?;
    crate::script::run_script("n = rows.length")?;
    assert_eq!(crate::script::context_value("rows"), Some(Value::List(vec![row(1), row(2)])));
    crate::script::run_script(
        "Object.defineProperty(rows[0], 'a', {get() { seen = 1; return 1 }, enumerable: true, configurable: true})",
    )?;
    assert_eq!(crate::script::context_value("seen"), Some(Value::I32(1)));
    assert_eq!(crate::script::context_value("rows"), Some(Value::List(vec![row(1), row(2)])));
    crate::script::run_script("rows[1] = new Proxy({a: 3}, {ownKeys(t) { seen2 = 2; return Reflect.ownKeys(t) }})")?;
    assert_eq!(crate::script::context_value("seen2"), Some(Value::I32(2)));
    assert_eq!(crate::script::context_value("rows"), Some(Value::List(vec![row(1), row(3)])));
    Ok(())
}

/// Review fixes: a Proxy in a tracked value is compared with the stack free; `ec.resource.expression` and `condition`
/// run strict, as on QuickJS. Under the server-static widget check a widget expression runs with the wrapped `ec`
/// (`oxide::widget`): an allowed member is the real one, any other throws, and the real `ec` is back after; a screen
/// unit runs with the real `ec`, and an `ec.resource.expression` it calls runs wrapped, its violation `null`.
#[test]
fn oxide_proxies_strict_expressions_and_the_widget_check() -> Result<(), Error> {
    use crate::screen::sri::Runner;
    clear_context_stack();
    push_context_frame();
    // a trap that writes a bare name while the run's end compares the object with the stack
    crate::script::run_script(
        "m = new Proxy({}, {getPrototypeOf() { other2 = 2; return Object.prototype }}); m.a = 1",
    )?;
    assert_eq!(crate::script::context_value("other2"), Some(Value::I32(2)));
    assert_eq!(
        value("ec.resource.expression('(function () { return typeof this })()')")?,
        Value::string("undefined")
    );
    assert_eq!(
        value("ec.resource.condition('(function () { return this === undefined })()')")?,
        Value::Bool(true)
    );
    let wrap = crate::script::WidgetWrap::set(true);
    // a coercion, JSON and a promise's probe read an area as a plain object, not as violations
    assert_eq!(
        crate::script::expand(
            "${ec.user}|${JSON.stringify(ec.l10n)}|${typeof ec.user.then}|${ec.l10n.valueOf() === ec.l10n}"
        )?,
        "[object Object]|{}|undefined|true"
    );
    let err = value("ec.user.context").unwrap_err().to_string();
    assert!(err.contains("ec.user.context is not available"), "{err}");
    // an expression the framework evaluates for itself reads the real `ec`
    assert!(crate::script::with_real_ec(|| crate::script::eval_condition(
        "typeof ec.user.context === 'object'"
    ))?);
    // the value converts inside the wrapper, as on QuickJS: a getter the conversion runs is checked too
    let err = crate::script::eval_map("({ get area() { return typeof ec.entity } })").unwrap_err().to_string();
    assert!(err.contains("ec.entity is not available"), "{err}");
    // a script or actions run is not a widget expression, on either engine
    crate::script::run_script("scriptSaw = typeof ec.entity")?;
    assert_eq!(crate::script::context_value("scriptSaw"), Some(Value::string("object")));
    let tried = |call: &str| {
        value(&format!("(() => {{ try {{ return String({call}) }} catch (e) {{ return String(e) }} }})()"))
            .map(|text| moqui_lib::object::to_plain_string(&text))
    };
    assert_eq!(tried("ec.resource.expand('${1}')")?, "1");
    assert_eq!(tried("typeof ec.user.getPreference")?, "function");
    for call in ["ec.resource.condition('true')", "ec.entity", "ec.l10n.getAvailableLocales"] {
        let text = tried(call)?;
        assert!(text.contains("is not available in a server-static qvt screen"), "{call}: {text}");
    }
    let unit = crate::screen::generate::Unit {
        id: u64::MAX,
        source: "(function ($depth) { unitSaw = String(ec.resource.expression('ec.entity')) + '|' + typeof ec.entity })"
            .to_string(),
        runtime: Arc::from(""),
        runtime_id: 0,
    };
    super::units::RUNNER.run(&unit)?;
    assert_eq!(crate::script::context_value("unitSaw"), Some(Value::string("null|object")));
    // a script a widget expression's host call runs is a run of its own, with the real `ec`
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    context
        .define_function(&global, "nestedRead", 0, |scope: &mut quickjs_oxide::engine::api::core::Scope<'_>| {
            let read = scope.lend(|| crate::script::eval_value("typeof ec.entity"));
            let text = read.map_or_else(|err| err.to_string(), |value| moqui_lib::object::to_plain_string(&value));
            Ok(quickjs_oxide::engine::api::core::Value::String(
                quickjs_oxide::engine::api::core::JsString::try_from_utf8(&text).unwrap(),
            ))
        })
        .unwrap();
    assert_eq!(
        value("nestedRead() + '|' + (() => { try { ec.entity; return 'real' } catch (e) { return 'wrapped' } })()")?,
        Value::string("object|wrapped")
    );
    drop(wrap);
    assert_eq!(value("typeof ec.entity")?, Value::string("object"), "the real ec is back");
    Ok(())
}
