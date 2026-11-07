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

/// The value of `source` on the oxide engine, an error for an error or a fallback.
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

fn fallback(source: &str) -> &'static str {
    match eval_value(source, hash_text(source), false) {
        Err(Outcome::Fallback(reason)) => reason,
        Ok(value) => panic!("{source} gave {value:?}"),
        Err(Outcome::Error(err)) => panic!("{source}: {}", err.message()),
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

/// A host function that lends the engine (the fork's F3a) runs a nested small script on oxide itself, under
/// small-oxide, with the outer run's values set aside and given back; without the lend it goes to QuickJS.
#[test]
fn a_script_nested_under_a_lend_runs_on_oxide() -> Result<(), Error> {
    use crate::script::{with_script_engine, ScriptEngine};
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    set_context_name("n", Value::I32(1))?;
    set_context_name("items", Value::List(vec![Value::I32(1)]))?;
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    let oxide_values = || crate::script::engine::oxide_runs_here(crate::script::engine::EntryKind::Value);
    for (name, lend) in [("lent", true), ("unlent", false)] {
        context
            .define_function(&global, name, 0, move |scope: &mut Scope<'_>| -> Result<JsValue, _> {
                let nested = || crate::script::eval_value("n + items.length");
                let got = if lend { scope.lend(nested) } else { nested() };
                match got {
                    Ok(Value::I32(number)) => Ok(JsValue::Int(number)),
                    other => Err(scope.throw_error(
                        quickjs_oxide::engine::api::core::NativeErrorKind::Type,
                        &format!("{other:?}"),
                    )),
                }
            })
            .unwrap();
    }
    with_script_engine(ScriptEngine::SmallOxide, || -> Result<(), Error> {
        let before = oxide_values();
        assert_eq!(value("items.length + lent() + items.length")?, Value::I32(4));
        assert_eq!(oxide_values(), before + 1, "the nested script ran on oxide");
        let before = oxide_values();
        assert_eq!(value("unlent()")?, Value::I32(2));
        assert_eq!(oxide_values(), before, "without a lend it ran on QuickJS");
        Ok(())
    })
}

/// A run that wrote to the stack and then met what the bindings lack is an error, not a second run on QuickJS that
/// would write again.
#[test]
fn a_fallback_after_a_write_is_an_error() -> Result<(), Error> {
    use crate::script::{with_script_engine, ScriptEngine};
    clear_context_stack();
    push_context_frame();
    set_context_name("total", Value::I32(1))?;
    let mut tools = indexmap::IndexMap::new();
    tools.insert("__wrap".to_string(), Value::string("x"));
    set_context_name("tools", Value::Map(tools))?;
    with_script_engine(ScriptEngine::SmallOxide, || -> Result<(), Error> {
        let err = crate::script::run_actions(
            r#"<actions><set field="total" from="total + 1"/><set field="copy" from="tools"/></actions>"#,
        )
        .unwrap_err();
        assert!(err.message().contains("after a host call"), "{}", err.message());
        assert_eq!(crate::script::context_value("total"), Some(Value::I32(2)), "added once");
        Ok(())
    })
}

/// A QuickJS script a lent host function runs inside an oxide script reads what the oxide script changed in place,
/// and the oxide script then reads by name what the QuickJS one wrote.
#[test]
fn a_quickjs_script_nested_in_oxide_sees_its_changes() -> Result<(), Error> {
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    set_context_name("items", Value::List(vec![Value::I32(1)]))?;
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    context
        .define_function(&global, "quick", 0, |scope: &mut Scope<'_>| -> Result<JsValue, _> {
            // a script kind oxide does not run: on QuickJS, nested in the oxide script
            let ran = scope.lend(|| crate::script::run_script("seenLength = items.length; items.push(7)"));
            match ran {
                Ok(()) => Ok(JsValue::Undefined),
                Err(err) => {
                    Err(scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, err.message()))
                }
            }
        })
        .unwrap();
    assert_eq!(
        written("(() => { items.push(5); quick(); return items.length })()")?,
        Value::I32(3)
    );
    assert_eq!(crate::script::context_value("seenLength"), Some(Value::I32(2)));
    assert_eq!(
        crate::script::context_value("items"),
        Some(Value::List(vec![Value::I32(1), Value::I32(5), Value::I32(7)]))
    );
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
fn local_time_is_the_process_zone_as_on_quickjs() -> Result<(), Error> {
    clear_context_stack();
    let offset = super::host::local_offset_minutes(0.0);
    assert_eq!(value("new Date(0).getTimezoneOffset()")?, Value::I32(offset));
    let quickjs = crate::script::with_ctx(|ctx| {
        ctx.eval::<i32, _>("new Date(0).getTimezoneOffset()")
            .map_err(crate::script::js_error)
    })?;
    assert_eq!(offset, quickjs);
    Ok(())
}

#[test]
fn what_the_bindings_do_not_have_yet_falls_back() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let mut tools = indexmap::IndexMap::new();
    tools.insert("__wrap".to_string(), Value::string("x"));
    set_context_name("tools", Value::Map(tools))?;
    assert_eq!(fallback("tools"), "a tools object");
    assert_eq!(fallback("context"), "the stack as context");
    assert_eq!(fallback("compare"), "a QuickJS global");
    assert_eq!(value("toDecimal('1.50').add(1).toString()")?, Value::string("2.50"));
    assert_eq!(value("toNumber('12.5') + toNumber(null)")?, Value::F64(12.5));
    let condition_object = Value::Object(HostObject::new(String::from("not a binding")));
    set_context_name("cond", condition_object)?;
    assert_eq!(fallback("cond"), "a host object the oxide bindings do not have yet");
    // the engine runs again after a fallback
    assert_eq!(value("1 + 1")?, Value::I32(2));
    Ok(())
}

/// Behind the static check: no function reaches `Function` by a computed key, and the legacy accessor definers are gone.
#[test]
fn the_runtime_cuts_the_ways_to_function_and_shared_accessors() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    set_context_name("key", Value::string("constructor"))?;
    for source in [
        "''[key][key]('return 1')()",
        "[].map[key]('return 1')()",
        "(() => 1)[key]('return 1')()",
    ] {
        let err = value(source).unwrap_err();
        assert!(
            err.message().contains("Function is not available"),
            "{source}: {}",
            err.message()
        );
    }
    assert_eq!(value("typeof ({}).__defineGetter__")?, Value::string("undefined"));
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
    let mut set = indexmap::IndexMap::new();
    set.insert("__set__".to_string(), Value::List(vec![Value::I32(1)]));
    set_context_name("seen", Value::Map(set))?;
    assert_eq!(fallback("seen.add(2)"), "a Set from the stack");
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
    assert_eq!(fallback("order.getEntityName()"), "an entity value method");
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
    let names = crate::script::with_script_engine(crate::script::ScriptEngine::Rquickjs, || {
        crate::script::eval_value("Object.getOwnPropertyNames(Object.getPrototypeOf(value)).sort().join(',')")
    })?;
    let names = moqui_lib::object::to_plain_string(&names);
    let quickjs: Vec<&str> = names.split(',').collect();
    let mut listed: Vec<&str> = super::names::ENTITY_METHODS.to_vec();
    listed.sort_unstable();
    assert_eq!(quickjs, listed);
    Ok(())
}
