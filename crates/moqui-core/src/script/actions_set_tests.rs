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

use moqui_lib::object::Value;

use super::{SetPlan, GLOBALS};
use crate::context::ExecutionContextFactory;
use crate::script::runtime::{clear_context_stack, context_value, set_context_name};

fn planned(sets: &str) -> bool {
    SetPlan::of(&format!("<actions>{sets}</actions>")).is_some()
}

fn read(name: &str) -> Option<Value> {
    context_value(name)
}

/// `sets` run by the plan, then by the generated script, each from the same context: what each left of `names`.
fn both(context: &[(&str, Value)], sets: &str, names: &[&str]) -> (Vec<Option<Value>>, Vec<Option<Value>>) {
    let xml = format!("<actions>{sets}</actions>");
    let prepared = super::super::generated(&xml).unwrap();
    assert!(prepared.plan.is_some(), "not planned: {sets}");
    let mut results = Vec::new();
    for native in [true, false] {
        clear_context_stack();
        for (name, value) in context {
            set_context_name(name, value.clone()).unwrap();
        }
        if native {
            super::super::run_actions(&xml).unwrap();
        } else {
            crate::script::runtime::eval_script_text(&prepared.script).unwrap();
        }
        results.push(names.iter().map(|name| read(name)).collect::<Vec<_>>());
    }
    clear_context_stack();
    let script = results.pop().unwrap();
    (results.pop().unwrap(), script)
}

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(entries.iter().map(|(key, value)| ((*key).to_string(), value.clone())).collect())
}

#[test]
fn simple_sets_are_planned_and_others_are_not() {
    for sets in [
        r#"<set field="a" from="b"/>"#,
        r#"<set field="a" from=" b "/>"#,
        r#"<set field="a" from="b.c.d"/>"#,
        r#"<set field="a" from="b?.c"/>"#,
        r#"<set field="a" from="0"/><set field="b" from="-12"/><set field="c" from="1.5"/>"#,
        r#"<set field="a" from="'text'"/><set field="b" from='"text"'/>"#,
        r#"<set field="a" from="true"/><set field="b" from="null"/><set field="c" from="[]"/><set field="d" from="{}"/>"#,
        r#"<set field="a" value="text"/><set field="b" value=""/><set field="c" value="${b}"/><set field="d" value="${b.c}"/>"#,
        r#"<set field="a" from="b" default-value="x"/><set field="c" from="b" default-value="${d}"/>"#,
        r#"<set field="a" from="b" type="Integer" set-if-empty="false"/>"#,
        r#"<set field="a" type="NewList"/><set field="b" from="anything + at(all)" type="NewMap"/>"#,
        r#"<set field="a"/>"#,
        r#"<set field="price" value="10$"/>"#,
    ] {
        assert!(planned(sets), "{sets}");
    }
    for sets in [
        r#"<set field="a" from="b + c"/>"#,
        r#"<set field="a" from="b.c()"/>"#,
        r#"<set field="a" from="b[0]"/>"#,
        r#"<set field="a" from="ec.user.userId"/>"#,
        r#"<set field="a" from="context.b"/>"#,
        r#"<set field="a" from="Math.PI"/>"#,
        r#"<set field="a" from="undefined"/>"#,
        r#"<set field="a" from="b.toString"/>"#,
        r#"<set field="a" from="b.get"/>"#,
        r#"<set field="a" from="b.__wrap"/>"#,
        r#"<set field="a" from="b?"/>"#,
        r#"<set field="a" from="'it\'s'"/>"#,
        r#"<set field="a" from="1e3"/><set field="b" from="010"/>"#,
        r#"<set field="a" from=""/>"#,
        r#"<set field="a" value="Hello ${name}"/>"#,
        r#"<set field="a" value="${b + c}"/>"#,
        r#"<set field="a" from="b" default-value="${c}${d}"/>"#,
        r#"<set field="a.b" from="c"/>"#,
        r#"<set field="ec" from="c"/>"#,
        r#"<set field="a" from="b" extra="1"/>"#,
        r#"<set field="a" from="b"/><script>c = 1</script>"#,
        r#"<if condition="b"><set field="a" from="b"/></if>"#,
        "",
    ] {
        assert!(!planned(sets), "{sets}");
    }
}

/// A set the plan runs keeps the host type of what it reads, where the generated script's round trip made dates text
/// and numbers the narrowest kind.
#[test]
fn a_planned_set_keeps_host_types() {
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    let now = jiff::Timestamp::from_millisecond(1_762_000_000_123).unwrap();
    let values = [
        ("text", Value::string("foo")),
        ("small", Value::I32(7)),
        ("long", Value::I64(12)),
        ("integral", Value::F64(3.0)),
        ("fraction", Value::F64(1.25)),
        ("amount", Value::Decimal("12.34".parse().unwrap())),
        ("stamp", Value::Timestamp(now)),
        ("day", Value::Date(jiff::civil::date(2026, 11, 9))),
        ("clock", Value::Time(jiff::civil::time(10, 30, 0, 0))),
        ("flag", Value::Bool(true)),
        ("items", Value::List(vec![Value::I64(1), Value::Timestamp(now)])),
        ("entry", map(&[("when", Value::Timestamp(now)), ("count", Value::I64(2))])),
    ];
    let sets: String = values
        .iter()
        .map(|(name, _)| format!(r#"<set field="{name}Out" from="{name}"/>"#))
        .chain([r#"<set field="whenOut" from="entry.when"/><set field="countOut" from="entry?.count"/>"#.to_string()])
        .collect();
    let mut names: Vec<String> = values.iter().map(|(name, _)| format!("{name}Out")).collect();
    names.extend(["whenOut".to_string(), "countOut".to_string()]);
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (native, script) = both(&values, &sets, &names);
    let mut expected: Vec<Option<Value>> = values.iter().map(|(_, value)| Some(value.clone())).collect();
    expected.extend([Some(Value::Timestamp(now)), Some(Value::I64(2))]);
    assert_eq!(native, expected);
    // the script's: a timestamp as its text, an integral double as 64 bits, a 64-bit integer that fits as 32
    assert!(matches!(script[6], Some(Value::String(_))), "{:?}", script[6]);
    assert_eq!(script[2], Some(Value::I32(12)));
    assert_eq!(script[3], Some(Value::I64(3)));
    assert_eq!(script[5], Some(Value::Decimal("12.34".parse().unwrap())));
}

/// Where the types agree, the plan and the generated script set the same values: literals, templates, defaults,
/// `set-if-empty` and `type`.
#[test]
fn planned_sets_answer_as_the_script_does() {
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    let context = [
        ("text", Value::string("foo")),
        ("empty", Value::string("")),
        ("nothing", Value::Null),
        ("count", Value::I32(5)),
        ("flag", Value::Bool(false)),
        ("numberText", Value::string("12.34")),
        ("intText", Value::string("42")),
        ("entry", map(&[("name", Value::string("bar")), ("inner", map(&[("n", Value::I32(1))]))])),
        ("kept", Value::string("before")),
    ];
    let cases: &[(&str, &[&str])] = &[
        (
            r#"<set field="a" value="lit"/><set field="b" value=""/><set field="c" value="${text}"/>
               <set field="d" value="${nothing}"/><set field="e" value="${count}"/><set field="f" value="${flag}"/>
               <set field="g" value="${entry.name}"/><set field="h" value="${entry.inner?.n}"/><set field="i" value="${unset}"/>
               <set field="j" value="cost 10$"/>"#,
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"],
        ),
        (
            r#"<set field="a" from="'lit'"/><set field="b" from="42"/><set field="c" from="-1"/><set field="d" from="1.5"/>
               <set field="e" from="true"/><set field="f" from="null"/><set field="g" from="[]"/><set field="h" from="{}"/>
               <set field="i" from="entry.missing"/><set field="j" from="nothing?.x"/><set field="k" from="unset"/>"#,
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"],
        ),
        (
            r#"<set field="a" from="nothing" default-value="dflt"/><set field="b" from="empty" default-value="${text}"/>
               <set field="c" from="text" default-value="dflt"/><set field="d" from="flag" default-value="dflt"/>
               <set field="e" value="" default-value="${entry.name}"/><set field="f" default-value="none"/>"#,
            &["a", "b", "c", "d", "e", "f"],
        ),
        (
            r#"<set field="kept" from="nothing" set-if-empty="false"/><set field="a" from="empty" set-if-empty="false"/>
               <set field="b" from="text" set-if-empty="false"/><set field="c" from="nothing" default-value="d" set-if-empty="false"/>"#,
            &["kept", "a", "b", "c"],
        ),
        (
            r#"<set field="a" from="intText" type="Integer"/><set field="b" from="numberText" type="BigDecimal"/>
               <set field="c" value="true" type="Boolean"/><set field="d" from="count" type="String"/>
               <set field="e" from="nothing" type="Integer"/><set field="f" from="text" type="NewList"/>
               <set field="g" type="NewMap"/><set field="h" value="7" type="Integer" set-if-empty="false"/>"#,
            &["a", "b", "c", "d", "e", "f", "g", "h"],
        ),
        (
            // a set reads what an earlier one set
            r#"<set field="first" from="entry"/><set field="second" from="first.name"/><set field="text" value="new"/>
               <set field="third" value="${text}"/>"#,
            &["first", "second", "text", "third"],
        ),
    ];
    for (sets, names) in cases {
        let (native, script) = both(&context, sets, names);
        assert_eq!(native, script, "{sets}");
    }
    // `set-if-empty="false"` with nothing to set leaves the name as it was
    let (native, _) = both(&context, r#"<set field="kept" from="nothing" set-if-empty="false"/>"#, &["kept"]);
    assert_eq!(native, vec![Some(Value::string("before"))]);
}

/// A value the plan does not answer as the script would is left to the script, which sets every name.
#[test]
fn a_value_the_plan_does_not_answer_runs_the_script() {
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    let context = [
        ("items", Value::List(vec![Value::I32(1), Value::I32(2), Value::I32(3)])),
        ("amount", Value::Decimal("12.50".parse().unwrap())),
        ("tools", map(&[("__set__", Value::List(vec![Value::string("x")]))])),
    ];
    let (native, script) = both(
        &context,
        r#"<set field="first" value="a"/><set field="size" from="items.length"/><set field="shown" value="${amount}"/>
           <set field="has" from="tools.size"/>"#,
        &["first", "size", "shown", "has"],
    );
    assert_eq!(native, script);
    assert_eq!(native[1], Some(Value::I32(3)));
    assert_eq!(native[3], Some(Value::I32(1)));
    // a member of null is the script's error
    clear_context_stack();
    let err = super::super::run_actions(r#"<actions><set field="a" from="nothing.name"/></actions>"#).unwrap_err();
    assert!(err.message().contains("null"), "{}", err.message());
    clear_context_stack();
}

/// An entity value's field reads as a script reads it, its type kept; a set on a field of it (or of a map) runs the
/// script.
#[test]
fn a_set_reads_and_writes_an_entity_value_field() {
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _guard = ctx.enter();
    clear_context_stack();
    super::super::run_actions(
        r#"<actions>
                <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                <set field="made.testId" value="PlannedSet"/>
                <set field="made.testMedium" value="medium"/>
                <set field="entry" from="{}"/>
                <set field="entry.name" value="mapped"/>
            </actions>"#,
    )
    .unwrap();
    let xml = r#"<actions><set field="seenMedium" from="made.testMedium"/><set field="seenLong" from="made.testLong"/>
            <set field="shownId" value="${made.testId}"/><set field="mapped" from="entry.name"/></actions>"#;
    assert!(SetPlan::of(xml).is_some());
    super::super::run_actions(xml).unwrap();
    assert_eq!(read("seenMedium"), Some(Value::string("medium")));
    assert_eq!(read("seenLong"), Some(Value::Null));
    assert_eq!(read("shownId"), Some(Value::string("PlannedSet")));
    assert_eq!(read("mapped"), Some(Value::string("mapped")));
    clear_context_stack();
}

/// The global names a bare name reads before the context, as a runtime has them, are the plan's [`GLOBALS`].
#[test]
fn the_plans_globals_are_the_runtimes() {
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    let names = crate::script::eval_value("Object.getOwnPropertyNames(globalThis).filter((n) => /^[a-z_$]/.test(n) && !n.startsWith('__'))")
        .unwrap();
    let Value::List(names) = names else {
        panic!("{names:?}");
    };
    let missing: Vec<String> = names
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| !GLOBALS.contains(name))
        .map(str::to_string)
        .collect();
    assert!(missing.is_empty(), "globals the plan would read from the context: {missing:?}");
}

/// `org.moqui.impl.BasicServices.echo#Data`'s body is planned: its timestamp comes back a timestamp, as in Java.
#[test]
fn echo_data_returns_its_timestamp_as_one() {
    let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/framework/service/org/moqui/impl/BasicServices.xml");
    let root = moqui_lib::node::MNode::parse_file(&file).unwrap();
    let service = root
        .child_iter()
        .find(|node| node.attribute_is("verb", "echo") && node.attribute_is("noun", "Data"))
        .unwrap();
    let actions = service.child_iter().find(|node| node.has_name("actions")).unwrap().write_string();
    assert!(SetPlan::of(&actions).is_some(), "{actions}");
    let now = jiff::Timestamp::from_millisecond(1_762_000_000_123).unwrap();
    let amount = Value::Decimal("12.34".parse().unwrap());
    let (native, _) = both(
        &[("textIn1", Value::string("foo")), ("textIn2", Value::string("ping")), ("numberIn", amount.clone()), ("timestampIn", Value::Timestamp(now))],
        &actions.replace("<actions>", "").replace("</actions>", ""),
        &["textOut1", "textOut2", "numberOut", "timestampOut"],
    );
    assert_eq!(
        native,
        vec![Some(Value::string("foo")), Some(Value::string("ping")), Some(amount), Some(Value::Timestamp(now))]
    );
}

/// A `default-value` is taken when Groovy's `?:` would take it, and `set-if-empty="false"` skips what Java's
/// `ObjectUtilities.isEmpty` calls empty, by the plan and by the generated script alike.
#[test]
fn defaults_and_set_if_empty_follow_java() {
    let ctx = ExecutionContextFactory::test_context();
    let _guard = ctx.enter();
    let zero = Value::Decimal("0.00".parse().unwrap());
    let context = [
        ("nothing", Value::Null),
        ("empty", Value::string("")),
        ("falseFlag", Value::Bool(false)),
        ("trueFlag", Value::Bool(true)),
        ("zeroInt", Value::I32(0)),
        ("zeroLong", Value::I64(0)),
        ("zeroDouble", Value::F64(0.0)),
        ("zeroDecimal", zero),
        ("one", Value::I32(1)),
        ("zeroText", Value::string("0")),
        ("noItems", Value::List(Vec::new())),
        ("items", Value::List(vec![Value::I32(1)])),
        ("noEntries", Value::Map(indexmap::IndexMap::new())),
        ("entries", map(&[("k", Value::I32(1))])),
        ("noSet", map(&[("__set__", Value::List(Vec::new()))])),
        ("kept", Value::string("before")),
    ];
    let names: Vec<&str> = context.iter().map(|(name, _)| *name).filter(|name| *name != "kept").collect();
    let defaults: String = names
        .iter()
        .map(|name| format!(r#"<set field="{name}Default" from="{name}" default-value="dflt"/>"#))
        .collect();
    let default_names: Vec<String> = names.iter().map(|name| format!("{name}Default")).collect();
    let default_names: Vec<&str> = default_names.iter().map(String::as_str).collect();
    let (native, script) = both(&context, &defaults, &default_names);
    assert_eq!(native, script);
    let defaulted: Vec<&str> = names
        .iter()
        .zip(&native)
        .filter(|(_, value)| *value == &Some(Value::string("dflt")))
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        defaulted,
        ["nothing", "empty", "falseFlag", "zeroInt", "zeroLong", "zeroDouble", "zeroDecimal", "noItems", "noEntries", "noSet"]
    );
    for name in names.iter().copied().filter(|name| *name != "noSet") {
        let skipped = matches!(name, "nothing" | "empty" | "noItems" | "noEntries");
        let sets = format!(r#"<set field="kept" from="{name}" set-if-empty="false"/>"#);
        let (native, script) = both(&context, &sets, &["kept"]);
        // the script's round trip narrows a 64-bit zero and an integral double; the set itself is the same
        if !matches!(name, "zeroLong" | "zeroDouble") {
            assert_eq!(native, script, "{name}");
        } else {
            assert_ne!(script, vec![Some(Value::string("before"))], "{name}");
        }
        let expected = if skipped {
            Value::string("before")
        } else {
            context.iter().find(|(held, _)| *held == name).unwrap().1.clone()
        };
        assert_eq!(native, vec![Some(expected)], "{name}");
    }
    // an empty Set is skipped too (it reads back as a list on the script's side)
    let (native, script) = both(&context, r#"<set field="kept" from="noSet" set-if-empty="false"/>"#, &["kept"]);
    assert_eq!((native, script), (vec![Some(Value::string("before"))], vec![Some(Value::string("before"))]));
    // values only a script makes: an empty array, a zero decimal, an empty object
    clear_context_stack();
    super::super::run_actions(
        r#"<actions>
            <set field="fromArray" from="[1].slice(1)" default-value="dflt"/>
            <set field="fromDecimal" from="toDecimal('0')" default-value="dflt"/>
            <set field="fromObject" from="Object.assign({}, {})" default-value="dflt"/>
            <set field="fromFull" from="[1].slice(0)" default-value="dflt"/>
            <set field="kept" value="before"/>
            <set field="kept" from="Object.assign({}, {})" set-if-empty="false"/>
            <set field="kept2" from="toDecimal('0')" set-if-empty="false"/>
        </actions>"#,
    )
    .unwrap();
    assert_eq!(read("fromArray"), Some(Value::string("dflt")));
    assert_eq!(read("fromDecimal"), Some(Value::string("dflt")));
    assert_eq!(read("fromObject"), Some(Value::string("dflt")));
    assert_eq!(read("fromFull"), Some(Value::List(vec![Value::I32(1)])));
    assert_eq!(read("kept"), Some(Value::string("before")));
    assert_eq!(read("kept2"), Some(Value::Decimal("0".parse().unwrap())));
    clear_context_stack();
}
