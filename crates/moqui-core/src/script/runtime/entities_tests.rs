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

/// `forNoKeyUpdate` is a lock as `forUpdate` is: it needs a transaction and makes the iterator scroll sensitive, and
/// `forUpdate(false)` leaves it. A related find takes it after `forUpdate`, and not both.
#[test]
fn for_no_key_update_locks_as_for_update_does() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    holds(
        r#"ec.entity.find("moqui.basic.Enumeration").forNoKeyUpdate(true).forUpdate(false).getResultSetType() === 1005
            && ec.entity.find("moqui.basic.Enumeration").forNoKeyUpdate(true).forNoKeyUpdate(false).getResultSetType() === 1003"#,
    );
    let outside =
        run_script(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").forNoKeyUpdate(true).one()"#)
            .unwrap_err();
    assert!(outside.to_string().contains("needs a transaction"), "{outside}");
    let both = run_script(
        r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().findRelatedOne("type", null, true, true)"#,
    )
    .unwrap_err();
    assert!(both.to_string().contains("both forUpdate and forNoKeyUpdate"), "{both}");
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

/// The members of `ec.entity`, a find, a list and a value keep the names and lengths they had as QuickJS classes, and
/// so do their prototypes' own names.
#[test]
fn the_entity_members_keep_their_names_and_lengths() {
    let context = enumerations();
    let _entered = context.enter();
    seed();
    let probe = r#"(() => {
        const members = (object) => Object.getOwnPropertyNames(object).filter((name) => typeof object[name] === "function")
            .sort().map((name) => name + "/" + object[name].length).join(" ");
        const find = ec.entity.find("moqui.basic.Enumeration");
        const list = ec.entity.find("moqui.basic.Enumeration").list();
        const value = list[0];
        return [members(ec.entity), members(Object.getPrototypeOf(find)), members(Object.getPrototypeOf(list)),
            members(Object.getPrototypeOf(value))].join(" | ");
    })()"#;
    let got = crate::script::eval_value(probe).unwrap();
    let expected = [
        "checkTables/1 clearEntityDefinitionFromCache/0 clearQueryStats/0 createAllAutoReverseManyRelationships/0 \
         createBulk/1 fastFindOne/3 find/1 findOneXml/5 formatFieldString/3 getAllEntitiesInfo/0 getAllEntityInfo/0 \
         getAllEntityNames/0 getAllEntityNamesWithMaster/0 getAllEntityRelatedFields/3 getConditionFactory/0 \
         getDataDocuments/1 getDatabaseConfName/1 getDatabaseTimeZone/0 getDatasourceGroupNames/0 getEecaRuleCount/0 \
         getEecaRulesByEntityName/0 getEntityDataDocument/0 getEntityDefinition/1 getEntityGroupName/1 \
         getQueryStatsList/0 getValueListFromPlainMap/2 isEntityDefined/0 isTxCacheActive/0 makeDataLoader/0 \
         makeDataWriter/0 makeValue/1 runSqlCountConf/3 runSqlStatement/2 runSqlUpdateConf/2 sequencedIdPrimary/1 \
         startTxCacheDb/1 stopTxCache/0 tempResetSequencedIdPrimary/1 tempSetSequencedIdPrimary/3 \
         testConfConnection/1",
        "condition/1 conditionDate/2 conditionToField/3 copy/0 count/0 deleteAll/0 disableAuthz/0 distinct/1 \
         fetchSize/0 forNoKeyUpdate/1 forUpdate/1 getEntityName/0 getFetchSize/0 getLimit/0 getMaxRows/0 getOffset/0 \
         getPageIndex/0 getPageSize/0 getQueryTextList/0 getResultSetType/0 getSelectFields/0 havingCondition/1 \
         iterator/0 limit/1 list/0 listMaster/1 maxRows/0 offset/1 one/0 oneMaster/0 orderBy/1 purge/0 \
         requireSearchFormParameters/0 resultSetConcurrency/1 resultSetType/1 searchFormInputs/4 searchFormMap/0 \
         selectField/1 selectFields/1 selectFormFields/1 updateAll/1 useCache/1 useClone/1",
        "add/2 addAll/1 addAllIfMissing/1 addIfMissing/1 at/0 clear/0 cloneList/0 concat/0 entries/0 every/0 filter/2 \
         filterByAnd/0 filterByCondition/2 filterByDate/0 filterByLimit/2 find/0 findByAnd/0 findIndex/0 findLast/0 \
         findLastIndex/0 first/0 flat/0 flatMap/0 forEach/2 get/1 getFirst/0 getMasterValueList/1 getPartialList/2 \
         getPlainValueList/1 getValueMapList/0 includes/0 indexMatching/1 indexOf/0 isEmpty/0 join/0 keys/0 last/0 \
         lastIndexOf/0 map/2 move/2 orderByFields/1 pop/0 push/0 reduce/0 reduceRight/0 remove/1 removeByAnd/1 \
         reverse/0 set/2 shift/0 size/0 slice/0 some/0 sort/1 splice/0 toJSON/0 toReversed/0 toSorted/0 unshift/0 \
         values/0",
        "cloneValue/0 compareTo/1 containsPrimaryKey/0 create/0 createOrUpdate/0 delete/0 deleteRelated/1 \
         deleteWithRelated/0 entrySet/0 equals/1 findRelated/1 findRelatedCount/3 findRelatedFk/0 findRelatedOne/1 \
         get/1 getBigDecimal/1 getBoolean/1 getBytes/1 getDate/1 getDouble/1 getEntityName/0 getEntityNamePretty/0 \
         getLong/1 getMap/0 getMasterValueMap/0 getNoCheckSimple/1 getOriginalDbValue/1 getPlainValueMap/1 \
         getPrimaryKeys/0 getPrimaryKeysString/0 getString/1 getTime/1 getTimestamp/1 isField/1 isFieldModified/1 \
         isFieldSet/1 isModified/0 isMutable/0 mapMatches/1 refresh/0 resolveEntityName/0 resolveEntityNamePretty/0 \
         set/2 setAll/1 setBytes/2 setFields/3 setSequencedIdPrimary/0 setSequencedIdSecondary/0 setString/2 store/0 \
         update/0",
    ]
    .join(" | ");
    assert_eq!(moqui_lib::object::to_plain_string(&got), expected);
}
