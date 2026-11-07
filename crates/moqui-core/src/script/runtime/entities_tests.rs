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

use std::sync::{Arc, Mutex};

use moqui_lib::{object::Value, sync::lock};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::script::template::{render_trusted, Escape};
use crate::script::{clear_context_stack, eval_condition, run_script};

fn holds(expression: &str) {
    assert!(eval_condition(expression).unwrap(), "{expression}");
}

/// A context with the in-memory database: types T1 and T2, and enumerations E1 to E3 of T1.
fn enumerations() -> ExecutionContext {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    context
}

fn seed() {
    run_script(
        r#"for (const [id, text] of [["T1", "Type 1"], ["T2", "Type 2"]]) {
                const type = ec.entity.makeValue("moqui.basic.EnumerationType");
                type.enumTypeId = id;
                type.description = text;
                type.create();
            }
            for (const [id, text, n] of [["E2", "second", 2], ["E1", "first", 1], ["E3", "third", 3]]) {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = id;
                value.enumTypeId = "T1";
                value.description = text;
                value.sequenceNum = n;
                value.create();
            }"#,
    )
    .unwrap();
}

#[test]
fn xml_find_retains_executed_sql_for_later_screen_sections() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let tape = Arc::new(Mutex::new(Vec::new()));
    context
        .factory()
        .set_entity(crate::testing::open_with_tape(Arc::clone(&tape)));
    let _entered = context.enter();
    seed();
    lock(&tape).clear();
    crate::script::run_actions(
        r#"<actions><entity-find entity-name="moqui.basic.Enumeration" list="diagnosticRows" cache="false">
                <econdition field-name="enumTypeId" value="T1"/>
            </entity-find></actions>"#,
    )
    .unwrap();
    holds("diagnosticRows.length === 3");
    holds("diagnosticRows_xafind?.getQueryTextList().length === 1");
    run_script("diagnosticSql = diagnosticRows_xafind.getQueryTextList()[0];").unwrap();
    let actual = crate::script::context_value("diagnosticSql").unwrap();
    assert_eq!(actual, Value::string(lock(&tape).last().unwrap()));
    assert!(!moqui_lib::object::to_plain_string(&actual).contains("T1"));
    // A cached second read does not invent another executed query.
    run_script(
        r#"const find = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").useCache(true);
                find.list();
                firstQueryCount = find.getQueryTextList().length;"#,
    )
    .unwrap();
    run_script(
        r#"const cached = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").useCache(true);
                cached.list();
                cachedQueryCount = cached.getQueryTextList().length;"#,
    )
    .unwrap();
    holds("firstQueryCount === 1 && cachedQueryCount === 0");
}

#[test]
fn tx_cache_db_refuses_flush_mode_and_needs_an_overlay() {
    let context = enumerations();
    let _entered = context.enter();
    holds("ec.entity.isTxCacheActive() === false");
    let flush = run_script("ec.entity.startTxCacheDb(false)").unwrap_err();
    assert!(flush.to_string().contains("no longer supported"), "{flush}");
    let none = run_script("ec.entity.startTxCacheDb(true)").unwrap_err();
    assert!(none.to_string().contains("not configured"), "{none}");
    holds("ec.entity.isTxCacheActive() === false");
    run_script("ec.entity.stopTxCache()").unwrap();
}

#[test]
fn a_field_reads_and_writes_as_a_property() {
    let context = enumerations();
    let _entered = context.enter();
    holds(
        r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = "X1";
                value["description"] = "by index";
                value.sequenceNum = 4;
                return value.get("enumId") === "X1" && value.description === "by index" && value.sequenceNum === 4
                    && value.sequenceNum === value.get("sequenceNum");
            })()"#,
    );
    // A field the value does not hold is null, and optional chaining goes through it.
    holds(
        r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.set("enumCode", "C");
                return value.parentEnumId === null && value.parentEnumId?.length === undefined && value.enumCode === "C";
            })()"#,
    );
}

/// As Java's `get` and `getMap`: a script reads a localized field in the user's locale; the stored value is what
/// an update writes.
#[test]
fn a_script_reads_localized_fields_and_writes_the_stored_ones() {
    let context = enumerations();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    seed();
    run_script(
        r#"const message = ec.entity.makeValue("moqui.basic.LocalizedMessage");
            message.original = "first"; message.locale = "fr"; message.localized = "premier";
            message.create();"#,
    )
    .unwrap();
    crate::user::session::set_locale(Some("fr"));
    holds(
        r#"(function () {
                const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
                const map = value.getMap();
                const json = JSON.parse(JSON.stringify(value));
                value.sequenceNum = 9;
                value.update();
                const stored = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").useCache(false).one();
                return value.description === "premier" && value.get("description") === "premier"
                    && map.description === "premier" && json.description === "premier"
                    && stored.sequenceNum === 9 && stored.description === "premier";
            })()"#,
    );
    crate::user::session::set_locale(Some("de"));
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").useCache(false).one().description
                === "first""#,
    );
    crate::user::session::set_locale(None);
}

/// The allocations the outermost script runtime holds now, after a collection. Not while a script runs: that
/// holds the runtime's lock.
fn live_allocations() -> i64 {
    let engine = super::super::ENGINES
        .with(|engines| engines.borrow().first().cloned())
        .expect("a started engine");
    engine.runtime.run_gc();
    engine.runtime.memory_usage().malloc_count
}

/// `keep_assigned` matches what rquickjs does with an assigned value: an extra reference each time would keep
/// every assigned string alive.
#[test]
fn a_field_assigned_many_times_does_not_grow_the_runtime() {
    let context = enumerations();
    let _entered = context.enter();
    let assign = |count: u32| {
        run_script(&format!(
            r#"(function () {{
                    const value = ec.entity.makeValue("moqui.basic.Enumeration");
                    for (let i = 0; i < {count}; i++) {{ value.description = "text " + i; }}
                }})()"#
        ))
        .unwrap();
    };
    assign(1_000);
    let before = live_allocations();
    assign(100_000);
    let after = live_allocations();
    assert!(
        after - before < 1_000,
        "{} allocations kept after 100k assignments",
        after - before
    );
}

#[test]
fn an_eeca_action_reads_the_value_being_written() {
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        clear_context_stack();
        let facade = crate::testing::open_with(
            &crate::testing::framework_dir(),
            crate::testing::Options {
                entity_eca: true,
                data_feed: false,
                tape: None,
            },
        );
        crate::entity::load_feed::add_eca_rules(
            &facade,
            r#"<eecas>
                    <eeca id="TestReadsShared" entity="moqui.test.TestEntity" on-create="true" on-update="true">
                        <actions><script>
                            var foo = ec.entity.makeValue("moqui.test.Foo");
                            foo.fooId = "F-" + shared.testMedium;
                            foo.fooText = shared.testId;
                            foo.create();
                        </script></actions>
                    </eeca>
                </eecas>"#,
        )
        .unwrap();
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        let written = run_script(
            r#"shared = ec.entity.makeValue("moqui.test.TestEntity");
                shared.testId = "T1";
                shared.testMedium = "first";
                shared.create();
                shared.testMedium = "second";
                shared.update();"#,
        )
        .map_err(|err| err.to_string());
        let foos = facade
            .find("moqui.test.Foo")
            .and_then(|find| find.list())
            .map(|list| {
                list.values()
                    .iter()
                    .map(|row| format!("{:?}={:?}", row.get("fooId"), row.get("fooText")))
                    .collect::<Vec<_>>()
            })
            .map_err(|err| err.to_string());
        let _ = done_tx.send(written.and(foos));
    });
    let foos = done_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the writes finished: the rule's read of the value did not deadlock")
        .unwrap();
    assert_eq!(foos.len(), 2, "{foos:?}");
    assert!(
        foos.iter().any(|foo| foo.contains("F-first") && foo.contains("T1")),
        "{foos:?}"
    );
    assert!(foos.iter().any(|foo| foo.contains("F-second")), "{foos:?}");
}

#[test]
fn an_unknown_name_reads_undefined_and_does_not_write() {
    let context = enumerations();
    let _entered = context.enter();
    holds(r#"ec.entity.makeValue("moqui.basic.Enumeration").noSuchField === undefined"#);
    holds(r#"ec.entity.makeValue("moqui.basic.Enumeration").get("noSuchField") === null"#);
    for script in [
        r#"ec.entity.makeValue("moqui.basic.Enumeration").noSuchField = 1"#,
        r#"ec.entity.makeValue("moqui.basic.Enumeration").set("noSuchField", 1)"#,
        r#"ec.entity.makeValue("moqui.basic.Enumeration").update = 1"#,
    ] {
        let err = run_script(script).unwrap_err();
        assert!(
            err.message()
                .contains("is not valid for entity moqui.basic.Enumeration"),
            "{script}: {err}"
        );
    }
}

#[test]
fn a_set_through_the_prototype_chain_defines_the_property_on_the_receiver() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    holds(
        r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = "X1";
                const child = Object.create(value);
                child.enumId = "C1";
                child.noSuchField = 2;
                const other = {};
                Reflect.set(value, "description", "other", other);
                return value.enumId === "X1" && value.get("enumId") === "X1" && child.enumId === "C1"
                    && Object.keys(child).join() === "enumId,noSuchField" && child.noSuchField === 2
                    && value.description === null && other.description === "other"
                    && Reflect.set(value, "enumId", "X2", value) && value.enumId === "X2";
            })()"#,
    );
    holds(
        r#"(function () {
                const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
                const child = Object.create(list);
                child.extra = "e";
                child[0] = "zero";
                list.extra = "ignored";
                return child.extra === "e" && child[0] === "zero" && list[0].enumId === "E1"
                    && list.extra === undefined && child.length === 3;
            })()"#,
    );
}

#[test]
fn set_fields_empty_strings_clear_to_null() {
    let ctx = enumerations();
    let _entered = ctx.enter();
    holds(
        r#"(() => {
                const row = ec.entity.makeValue("moqui.basic.Enumeration");
                row.enumId = "empty-fields";
                row.description = "keep";
                row.enumCode = "keep code";
                row.setFields({description: "", enumCode: null}, false, null, null);
                if (row.description !== "keep" || row.enumCode !== "keep code") return false;
                row.setFields({description: "", enumCode: " "}, true, null, false);
                if (row.description !== null || row.enumCode !== " " || row.enumId !== "empty-fields") return false;
                row.create();
                const stored = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "empty-fields").one();
                return stored.description === null && stored.enumCode === " ";
            })()"#,
    );
}

#[test]
fn clone_value_is_a_separate_copy() {
    let ctx = enumerations();
    let _entered = ctx.enter();
    holds(
        r#"(() => { const a = ec.entity.makeValue("moqui.basic.Enumeration"); a.enumId = "X"; a.description = "one";
                const b = a.cloneValue(); b.description = "two";
                return a.description === "one" && b.description === "two" && b.enumId === "X"; })()"#,
    );
}

#[test]
fn equals_compares_the_entity_and_field_values_not_identity() {
    let ctx = enumerations();
    let _entered = ctx.enter();
    holds(
        r#"(() => { const a = ec.entity.makeValue("moqui.basic.Enumeration"); a.enumId = "X"; a.description = "one";
                const b = a.cloneValue(); const c = a.cloneValue(); c.description = "two";
                const other = ec.entity.makeValue("moqui.basic.EnumerationType"); other.enumTypeId = "X"; other.description = "one";
                return a !== b && a.equals(b) && !a.equals(c) && !a.equals(other) && !a.equals(null) && !a.equals({}); })()"#,
    );
}

#[test]
fn a_method_name_wins_over_a_field_name() {
    let context = enumerations();
    let _entered = context.enter();
    holds(
        r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                return typeof value.get === "function" && typeof value.findRelatedOne === "function"
                    && typeof value.toString === "function" && "update" in value && "enumId" in value
                    && !("noSuchField" in value) && value[Symbol.iterator] === undefined;
            })()"#,
    );
}

#[test]
fn a_relationship_name_reads_the_related_value_or_list() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().type.description === "Type 1""#);
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one()["moqui.basic.EnumerationType"].enumTypeId === "T1""#,
    );
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().parent === null"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().groupMembers.length === 0"#);
    // get(name) reads a relationship the same way, as Java's EntityValue.get does
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().get("moqui.basic.EnumerationType").enumTypeId === "T1""#,
    );
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().get("parent") === null"#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().get("noSuchName") === null"#);
}

#[test]
fn filter_map_list_copies_or_filters_an_entity_list() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    crate::script::run_actions(
        r#"<actions>
                <entity-find entity-name="moqui.basic.Enumeration" list="all"/>
                <filter-map-list list="all" to-list="firsts"><field-map field-name="enumId" value="E1"/></filter-map-list>
                <set field="allBefore" from="all.length"/>
                <filter-map-list list="all"><field-map field-name="enumId" value="E2"/></filter-map-list>
                <set field="seen" from="allBefore + ':' + firsts.length + firsts[0].enumId + ':' + all.length + all[0].enumId"/>
            </actions>"#,
    )
    .unwrap();
    let seen = crate::script::context_value("seen").map(|value| moqui_lib::object::to_plain_string(&value));
    assert_eq!(seen.as_deref(), Some("3:1E1:1E2"));
}

#[test]
fn keys_json_spread_and_for_in_see_the_fields() {
    let context = enumerations();
    let _entered = context.enter();
    holds(
        r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = "X1";
                value.description = "d";
                const seen = [];
                for (const key in value) seen.push(key);
                const copy = { ...value };
                return Object.keys(value).join() === "description,enumId" && seen.join() === "description,enumId"
                    && JSON.stringify(value) === '{"description":"d","enumId":"X1"}'
                    && copy.enumId === "X1" && Object.keys(copy).length === 2
                    && Object.prototype.hasOwnProperty.call(value, "enumId")
                    && Object.getOwnPropertyDescriptor(value, "description").value === "d";
            })()"#,
    );
}

#[test]
fn a_list_indexes_iterates_and_sizes() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    holds(
        r#"(function () {
                const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
                const ids = [];
                for (const value of list) ids.push(value.enumId);
                return list[0].enumId === "E1" && list[2].description === "third" && list[3] === undefined
                    && list.length === 3 && list.size() === 3 && !list.isEmpty() && "1" in list && !("3" in list)
                    && ids.join() === "E1,E2,E3" && list.map((value) => value.sequenceNum).join() === "1,2,3"
                    && list.filter((value) => value.sequenceNum > 1).length === 2
                    && JSON.parse(JSON.stringify(list))[1].enumId === "E2" && Object.keys(list).join() === "0,1,2";
            })()"#,
    );
}

/// A script's `update()` of a value it found writes only the fields it changed, by property, `set` or `setFields`:
/// another value read with it keeps the field it changed, a field set to what it held is not written, and after an
/// update a field set back to what it was read with is.
#[test]
fn a_script_update_writes_only_the_fields_it_changed() {
    clear_context_stack();
    let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let context = ExecutionContextFactory::test_context();
    context
        .factory()
        .set_entity(crate::testing::open_with_tape(std::sync::Arc::clone(&tape)));
    let _entered = context.enter();
    let _authz = context.disable_authz();
    seed();
    let updates = || -> Vec<String> {
        moqui_lib::sync::lock(&tape)
            .iter()
            .filter(|sql| sql.starts_with("UPDATE"))
            .map(|sql| sql.split(" WHERE ").next().unwrap_or_default().to_string())
            .collect()
    };
    holds(
        r#"(function () {
                const read = () => ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").useCache(false).one();
                const a = read(), b = read(), c = read();
                a.description = "by property";
                a.update();
                b.setFields({ sequenceNum: 7, description: "first" }, true, null, null);
                b.update();
                c.set("enumCode", "C1");
                c.description = c.description;
                c.update();
                const stored = read();
                return stored.description === "by property" && stored.sequenceNum === 7 && stored.enumCode === "C1";
            })()"#,
    );
    let written = updates();
    assert_eq!(written.len(), 3, "{written:?}");
    assert!(
        written[0].contains("description") && !written[0].contains("sequence_num"),
        "{written:?}"
    );
    assert!(
        written[1].contains("sequence_num") && !written[1].contains("description"),
        "{written:?}"
    );
    assert!(
        written[2].contains("enum_code") && !written[2].contains("description"),
        "{written:?}"
    );
    holds(
        r#"(function () {
                const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E2").useCache(false).one();
                value.description = "changed";
                value.update();
                value.update();
                value.description = "second";
                value.update();
                return ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E2").useCache(false).one()
                    .description === "second";
            })()"#,
    );
    assert_eq!(updates().len(), 5, "{:?}", updates());
}

#[test]
fn a_field_set_on_a_list_row_is_kept_in_the_list() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    holds(
        r#"(() => {
                const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
                list[0].description = "changed";
                list.get(1).description = "by get";
                for (const value of list) value.enumCode = "C" + value.sequenceNum;
                list.forEach((value) => { value.optionValue = "f"; });
                const part = list.getPartialList(2, 1, false);
                part[0].description = "by part";
                return list[0].description === "changed" && list.get(1).description === "by get"
                    && list[2].enumCode === "C3" && list[1].optionValue === "f" && list[2].description === "by part";
            })()"#,
    );
    // the list kept on the context has the change, and the database and a new find do not
    run_script(
        r#"enums = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list(); enums[0].description = "kept""#,
    )
    .unwrap();
    holds(r#"enums[0].description === "kept" && ec.context.get("enums")[0].description === "kept""#);
    holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().description === "first""#);
}

#[test]
fn a_cached_find_list_is_not_changed_by_a_script() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    holds(
        r#"(() => {
                const first = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").orderBy("sequenceNum").useCache(true).list();
                first[0].description = "scribbled";
                const again = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").orderBy("sequenceNum").useCache(true).list();
                return again[0].description === "first";
            })()"#,
    );
}

#[test]
fn an_iterate_entry_is_the_row_of_the_list() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    crate::script::actions::run_actions(
        r#"<actions>
                <entity-find entity-name="moqui.basic.Enumeration" list="enumList"><order-by field-name="sequenceNum"/></entity-find>
                <iterate list="enumList" entry="enumValue"><set field="enumValue.description" from="'it ' + enumValue.enumId"/></iterate>
            </actions>"#,
    )
    .unwrap();
    holds(r#"enumList[0].description === "it E1" && enumList[2].description === "it E3""#);
}

#[test]
fn a_map_list_read_through_context_get_keeps_its_writes() {
    clear_context_stack();
    run_script(r#"rows = [{a: 1}, {a: 2}]"#).unwrap();
    run_script(r#"ec.context.get("rows")[0].a = 5; ec.context.get("rows").push({a: 3})"#).unwrap();
    holds(r#"rows.length === 3 && rows[0].a === 5"#);
}

#[test]
fn a_template_loop_reads_fields_by_dot() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    run_script(r#"enumList = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list()"#).unwrap();
    let text = render_trusted(
        "<#list enumList as row>${row.enumId}=${row.description} (${row.type.description});</#list>",
        Escape::None,
    )
    .unwrap();
    assert_eq!(text, "E1=first (Type 1);E2=second (Type 1);E3=third (Type 1);");
}

/// The engine counts its heap's live bytes as it allocates (`CountingAllocator`), close to what QuickJS's own walk
/// of the heap reports.
#[test]
fn the_engine_counts_its_heap_as_quickjs_does() {
    let context = enumerations();
    let _entered = context.enter();
    run_script("globalThis.big = []; for (let i = 0; i < 20000; i++) globalThis.big.push('x' + i)").unwrap();
    let engine = super::super::ENGINES
        .with(|engines| engines.borrow().first().cloned())
        .expect("a started engine");
    engine.runtime.run_gc();
    let counted = i64::try_from(engine.heap_bytes.get()).unwrap();
    let walked = engine.runtime.memory_usage().malloc_size;
    assert!(
        counted > walked / 2 && counted < walked * 2,
        "counted {counted} against QuickJS's {walked}"
    );
}

/// A script one thread's engine compiled is read from the process's bytecode by another thread's engine, not parsed
/// again, and runs the same.
#[test]
fn another_threads_engine_reads_a_compiled_script_s_bytecode() {
    let source = "(function () { let total = 0; for (let i = 1; i <= 10; i++) total += i; return 'sum ' + total })() /* shared bytecode test */";
    let run = move || {
        let context = enumerations();
        let _entered = context.enter();
        super::super::eval_value(source).unwrap()
    };
    let first = std::thread::spawn(run).join().unwrap();
    let before = super::compiled::shared_bytecode_reads();
    let second = std::thread::spawn(run).join().unwrap();
    assert_eq!(first, Value::string("sum 55"));
    assert_eq!(second, first);
    assert!(
        super::compiled::shared_bytecode_reads() > before,
        "the second engine read the bytecode"
    );
}
