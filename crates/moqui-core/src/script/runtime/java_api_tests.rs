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

//! The members of Moqui Java's `ec` API a ported script calls, as scripts call them (docs/dev/JS-Core-API.md).

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::script::{clear_context_stack, eval_condition, run_script};

fn holds(expression: &str) {
    assert!(eval_condition(expression).unwrap(), "{expression}");
}

/// A context over an empty test database.
fn database() -> ExecutionContext {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    context
}

#[test]
fn ec_context_is_the_context_stack() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(r#"ec.context === context && ec.getContext() === context"#);
    run_script(r#"ec.context.put("who", "Ada"); ec.context.count = 2; ec.context.putAll({a: 1, b: 2})"#).unwrap();
    holds(r#"who === "Ada" && count === 2 && a === 1 && ec.context.who === "Ada" && ec.context.get("b") === 2"#);
    holds(r#"ec.context.containsKey("who") && !ec.context.containsKey("nobody") && ec.context.keySet().includes("a")"#);
    holds(r#"ec.context.size() === 4 && !ec.context.isEmpty() && ec.context.containsValue("Ada")"#);
    holds(
        r#"ec.context.values().includes(2) && ec.context.entrySet().some((e) => e.getKey() === "who" && e.value === "Ada")"#,
    );
    holds(r#"ec.context.getByString("who") === "Ada" && String(ec.context).startsWith("ContextStack[")"#);
    holds(r#"ec.context.remove("who") === "Ada" && !("who" in context)"#);
    // a context name wins over a method of the same name
    run_script(r#"context.put("size", 100)"#).unwrap();
    holds(r#"context.size === 100 && ec.context.get("size") === 100"#);
    clear_context_stack();
}

#[test]
fn context_frames_push_and_pop() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    run_script(r#"outer = 1; shadowed = "below""#).unwrap();
    holds(
        r#"(function () {
            const same = ec.context.push() === context;
            context.shadowed = "above";
            context.inner = 2;
            const during = shadowed === "above" && inner === 2 && outer === 1;
            const popped = ec.context.pop();
            return same && during && popped.shadowed === "above" && popped.inner === 2
                && shadowed === "below" && !("inner" in context);
        })()"#,
    );
    holds(
        r#"(function () {
            ec.context.push({given: 7});
            const during = given === 7;
            ec.context.pop();
            return during && !("given" in context);
        })()"#,
    );
    // an isolated context sees nothing of the one around it
    holds(
        r#"(function () {
            ec.context.pushContext();
            const hidden = !("outer" in context);
            context.lone = 1;
            ec.context.popContext();
            return hidden && outer === 1 && !("lone" in context);
        })()"#,
    );
    holds(
        r#"(function () {
            ec.context.push();
            ec.context.addRootMap({rooted: "yes"});
            const seen = rooted === "yes" && ec.context.getRootMap().rooted === "yes"
                && ec.contextRoot.rooted === "yes" && ec.getContextRoot().rooted === "yes";
            context.top = 1;
            ec.context.clear();
            const cleared = !("top" in context) && outer === 1;
            ec.context.pop();
            return seen && cleared;
        })()"#,
    );
    clear_context_stack();
}

#[test]
fn a_getter_and_its_method_form_are_both_there() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(
        r#"ec.getUser() === ec.user && ec.getMessage() === ec.message && ec.getL10n() === ec.l10n
            && ec.getEntity() === ec.entity && ec.getService() === ec.service && ec.getResource() === ec.resource
            && ec.getLogger() === ec.logger && ec.getCache() === ec.cache && ec.getWeb() === ec.web
            && ec.getFactory() === ec.factory && ec.getTransaction() === ec.transaction
            && ec.getArtifactExecution() === ec.artifactExecution && ec.getScreen() === ec.screen"#,
    );
    // a getter has its method form, and a no-argument get method its property form
    holds(
        r#"ec.user.getUserId() === ec.user.userId && ec.user.getUsername() === ec.user.username
            && ec.user.getNowTimestamp() != null && ec.user.clientIp === ec.user.getClientIp()
            && ec.message.getErrors().length === 0 && ec.web.getParameters() != null
            && ec.factory.getMoquiVersion() === ec.factory.moquiVersion
            && ec.entity.getConditionFactory() === ec.entity.conditionFactory"#,
    );
    // a get method that needs an argument is not read as a property
    holds(r#"!("preference" in ec.user) && typeof ec.user.getPreference === "function""#);
    // the methods object the shims were lent is not left as a global
    holds(r#"Object.getOwnPropertyDescriptor(globalThis, "__moquiContextMethods") === undefined"#);
    clear_context_stack();
}

#[test]
fn message_lists_are_arrays_as_in_java() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(r#"ec.message.errors.length === 0 && ec.message.messages.length === 0 && !ec.message.hasError()"#);
    run_script(
        r#"ec.message.addMessage("saved", "success");
        ec.message.addPublic("shown to all", "warning");
        ec.message.addError("broken");
        ec.message.addValidationError("EditForm", "fromDate", "update#Thing", "is required");"#,
    )
    .unwrap();
    holds(r#"ec.message.errors.join("|") === "broken|is required" && ec.message.getErrors().length === 2"#);
    holds(
        r#"ec.message.messages.join("|") === "saved|shown to all" && ec.message.getMessagesString() === "saved\nshown to all\n""#,
    );
    holds(
        r#"ec.message.messageInfos.map((info) => info.message + ":" + info.typeString).join("|") === "saved:success|shown to all:warning""#,
    );
    holds(
        r#"ec.message.publicMessages.join() === "shown to all" && ec.message.getPublicMessageInfos()[0].typeString === "warning""#,
    );
    holds(
        r#"(function () {
            const [error] = ec.message.getValidationErrors();
            return ec.message.validationErrors.length === 1 && error.form === "EditForm" && error.field === "fromDate"
                && error.serviceName === "update#Thing" && error.message === "is required"
                && error.fieldPretty === "From Date" && error.stringPretty.startsWith("is required(for field From Date");
        })()"#,
    );
    run_script(r#"ec.message.clearAll()"#).unwrap();
    holds(
        r#"ec.message.errors.length === 0 && ec.message.messages.length === 0 && ec.message.publicMessages.length === 0
            && ec.message.validationErrors.length === 0"#,
    );
    clear_context_stack();
}

#[test]
fn logger_levels_and_user_and_l10n_forms() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(
        r#"ec.logger.ERROR_INT === 200 && ec.logger.WARN_INT === 300 && ec.logger.INFO_INT === 400
            && ec.logger.DEBUG_INT === 500 && ec.logger.TRACE_INT === 600 && ec.logger.OFF_INT === 0
            && ec.logger.logEnabled(ec.logger.OFF_INT) === false && typeof ec.logger.logEnabled("error") === "boolean""#,
    );
    run_script(r#"ec.logger.log(ec.logger.ERROR_INT, "an error"); ec.logger.log(ec.logger.TRACE_INT, "a trace")"#)
        .unwrap();
    // the locale and hide-symbol arguments of formatCurrency, and the zone of parseTimestamp
    holds(
        r#"ec.l10n.formatCurrency(1234.5, "USD", 2, "en_US", true) === ec.l10n.formatCurrencyNoSymbol(1234.5, "USD", 2)
            && ec.l10n.formatCurrency(1234.5, "USD", 2, "en_US").includes("1,234.50")"#,
    );
    holds(
        r#"ec.user.timestampMillis(ec.l10n.parseTimestamp("2026-03-01 12:00", "yyyy-MM-dd HH:mm", null, "UTC"))
            - ec.user.timestampMillis(ec.l10n.parseTimestamp("2026-03-01 12:00", "yyyy-MM-dd HH:mm", null, "America/New_York"))
            === -5 * 60 * 60 * 1000"#,
    );
    holds(r#"ec.user.visitorId === ec.user.getVisitorId() && ec.user.getVisitUserId() === null"#);
    holds(r#"ec.user.currencyUomId === null && ec.user.getNowCalendar() instanceof Date"#);
    // with no database the `##period` suffix of the localized "to" stays: the text has no message row to read
    holds(
        r#"/^This Month \(\d{4}-\d\d-01 to(##period)? \d{4}-\d\d-\d\d\)$/.test(ec.user.getPeriodDescription("month", "0", null))
            && ec.user.getPeriodDescription("7d", "-1", null).startsWith("Last 7 Days (")"#,
    );
    clear_context_stack();
}

#[test]
fn cache_artifact_stack_factory_and_resource_forms() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(
        r#"(function () {
            const cache = ec.cache.getCache("java.api.test");
            cache.clear();
            const first = cache.putIfAbsent("a", 1) === true && cache.putIfAbsent("a", 2) === false && cache.get("a") === 1;
            const swapped = cache.getAndPut("a", 3) === 1 && cache.replace("a", 4) === true && cache.replace("zz", 1) === false;
            cache.putAll({b: 5, c: 6});
            const all = cache.getAll(["a", "c", "nope"]);
            const read = all.a === 4 && all.c === 6 && !("nope" in all) && cache.size() === 3
                && cache.keySet().sort().join() === "a,b,c" && cache.getName() === "java.api.test";
            cache.removeAll(["b"]);
            const removed = cache.getAndRemove("c") === 6 && cache.size() === 1;
            cache.removeAll();
            return first && swapped && read && removed && cache.size() === 0
                && ec.cache.getCacheNames().includes("java.api.test") && ec.cache.cacheNames.includes("java.api.test");
        })()"#,
    );
    holds(
        r#"(function () {
            ec.cache.getCache("java.api.prefix.one").put("k", 1);
            ec.cache.getCache("java.api.other").put("k", 1);
            ec.cache.clearCachesByPrefix("java.api.prefix");
            return ec.cache.getCache("java.api.prefix.one").size() === 0 && ec.cache.getCache("java.api.other").size() === 1;
        })()"#,
    );
    holds(
        r#"(function () {
            const before = ec.artifactExecution.getStack().length;
            ec.artifactExecution.push("java.api.Frame", "AT_OTHER", "AUTHZA_VIEW", false);
            const top = ec.artifactExecution.peek();
            const during = ec.artifactExecution.stack.length === before + 1 && top.name === "java.api.Frame"
                && top.typeEnumId === "AT_OTHER" && ec.artifactExecution.getStackNameString().startsWith("java.api.Frame");
            ec.artifactExecution.pop();
            return during && ec.artifactExecution.getStackArray().length === before;
        })()"#,
    );
    holds(
        r#"ec.factory.entity === ec.entity && ec.factory.getService() === ec.service && ec.factory.isDestroyed() === false
            && typeof ec.factory.getComponentBaseLocations() === "object""#,
    );
    holds(r#"ec.resource.getContentType("a.pdf") === ec.resource.contentTypeOf("a.pdf")"#);
    holds(
        r#"typeof ec.resource.getLocationStream === "function" && typeof ec.resource.getUriReference === "function""#,
    );
    // Java's fourth argument of expand: no localization
    holds(
        r#"ec.resource.expand("a ${1 + 1}", "", null, false) === "a 2" && ec.resource.expand("a ${1 + 1}", "") === "a 2""#,
    );
    holds(
        r#"ec.web.pathInfoList.length === 0 && ec.web.requestUrl === null && typeof ec.web.sendBytesResponse === "function""#,
    );
    holds(r#"ec.web.getFieldValidationErrors("nothing").length === 0"#);
    clear_context_stack();
}

#[test]
fn transactions_begin_commit_and_roll_back_as_in_java() {
    let context = database();
    let _entered = context.enter();
    let count = r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "like", "JTX%").count()"#;
    // each script makes its rows with this function, declared at its start
    let holds = |script: &str| {
        let make = r#"const makeType = (id) => { const type = ec.entity.makeValue("moqui.basic.EnumerationType"); type.enumTypeId = id; type.create(); };"#;
        holds(&script.replacen("(function () {", &format!("(function () {{ {make}"), 1));
    };
    holds(r#"ec.transaction.getStatus() === 6 && ec.transaction.getStatusString() === "No Transaction (6)""#);
    // the usual Java shape: begin, work, commit what was begun
    holds(
        r#"(function () {
            const began = ec.transaction.begin(null);
            const inPlace = ec.transaction.isTransactionInPlace() && ec.transaction.getStatus() === 0;
            const joined = ec.transaction.begin(30);
            makeType("JTX1");
            ec.transaction.commit(joined);
            const still = ec.transaction.isTransactionInPlace();
            ec.transaction.commit(began);
            return began === true && joined === false && inPlace && still && !ec.transaction.isTransactionInPlace();
        })()"#,
    );
    assert!(eval_condition(&format!("{count} === 1")).unwrap());
    // a rollback of what was begun discards the work; a joined rollback marks the transaction
    holds(
        r#"(function () {
            const began = ec.transaction.begin(null);
            makeType("JTX2");
            ec.transaction.rollback(false, "joined work failed", null);
            const marked = ec.transaction.getStatus() === 1 && ec.transaction.getStatusString() === "Marked Rollback-Only (1)";
            ec.transaction.rollback(began, "giving up", new Error("why"));
            return marked && !ec.transaction.isTransactionInPlace();
        })()"#,
    );
    assert!(eval_condition(&format!("{count} === 1")).unwrap());
    // runUseOrBegin commits and gives the body's value; a throw rolls back and is thrown on
    holds(
        r#"(function () {
            return ec.transaction.runUseOrBegin(null, "failed", () => { makeType("JTX3"); return 7; }) === 7;
        })()"#,
    );
    holds(
        r#"(function () {
            try { ec.transaction.runUseOrBegin(60, "failed", () => { makeType("JTX4"); throw new Error("stop here"); }); }
            catch (err) { return String(err.message).includes("stop here") && !ec.transaction.isTransactionInPlace(); }
            return false;
        })()"#,
    );
    assert!(eval_condition(&format!("{count} === 2")).unwrap());
    // runRequireNew commits its own work whatever becomes of the transaction around it
    holds(
        r#"(function () {
            const began = ec.transaction.begin(null);
            makeType("JTX5");
            ec.transaction.runRequireNew(null, "failed", () => makeType("JTX6"));
            ec.transaction.rollback(began, "outer rolled back", null);
            return true;
        })()"#,
    );
    assert!(eval_condition(&format!("{count} === 3")).unwrap());
    holds(r#"ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "JTX6").one() != null"#);
    // suspend and resume around work outside the transaction
    holds(
        r#"(function () {
            const began = ec.transaction.begin(null);
            const suspended = ec.transaction.suspend();
            const outside = !ec.transaction.isTransactionInPlace();
            ec.transaction.resume();
            const back = ec.transaction.isTransactionInPlace();
            ec.transaction.setRollbackOnly("no", null);
            ec.transaction.rollback(began, "done", null);
            return suspended === true && outside && back;
        })()"#,
    );
    clear_context_stack();
}

/// Three enumerations of one type.
fn seed_enums() {
    run_script(
        r#"ec.entity.makeValue("moqui.basic.EnumerationType").setAll({enumTypeId: "JT", description: "Java API"}).create();
        for (const [id, text, n] of [["JE2", "second", 2], ["JE1", "first", 1], ["JE3", "third", 3]]) {
            ec.entity.makeValue("moqui.basic.Enumeration")
                .setAll({enumId: id, enumTypeId: "JT", description: text}).set("sequenceNum", n).create();
        }"#,
    )
    .unwrap();
}

#[test]
fn entity_value_writes_chain_and_have_javas_methods() {
    let context = database();
    let _entered = context.enter();
    seed_enums();
    holds(
        r#"(function () {
            const value = ec.entity.makeValue("moqui.basic.Enumeration");
            const chained = value.set("enumId", "JE9").setFields({enumTypeId: "JT"}, true, null, null).setAll({description: "ninth"});
            const made = chained === value && value.create() === value && value.store() === value && value.update() === value;
            const found = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "JE9").one();
            const unchanged = !found.isModified() && !found.isFieldModified("description");
            found.description = "changed";
            const changed = found.isModified() && found.isFieldModified("description")
                && found.getOriginalDbValue("description") === "ninth";
            const facts = found.isField("description") && !found.isField("nope") && found.isFieldSet("enumId")
                && found.containsPrimaryKey() && !ec.entity.makeValue("moqui.basic.Enumeration").containsPrimaryKey()
                && found.getPrimaryKeysString() === "JE9" && found.isMutable()
                && found.mapMatches({enumTypeId: "JT", enumId: "JE9"}) && !found.mapMatches({enumTypeId: "other"})
                && found.getString("enumId") === "JE9" && found.getNoCheckSimple("enumTypeId") === "JT";
            const other = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "JE1").one();
            const ordered = other.compareTo(found) < 0 && found.compareTo(other) > 0 && found.compareTo(found) === 0;
            const gone = found.delete() === found
                && ec.entity.find("moqui.basic.Enumeration").condition("enumId", "JE9").one() == null;
            return made && unchanged && changed && facts && ordered && gone;
        })()"#,
    );
    holds(
        r#"(function () {
            const type = ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "JT").one();
            // the test entity has no master definition: the call reaches the entity facade, which says so
            try { type.getMasterValueMap(null); } catch (err) { return String(err.message).includes("No master definition"); }
            return false;
        })()"#,
    );
    clear_context_stack();
}

#[test]
fn entity_list_has_javas_list_methods() {
    let context = database();
    let _entered = context.enter();
    seed_enums();
    let ids = |script: &str| {
        holds(&script.replacen(
            "(function () {",
            r#"(function () { const all = () => ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "JT").orderBy("enumId").list(); const ids = (list) => Array.from(list, (row) => row.enumId).join();"#,
            1,
        ));
    };
    // the filters and orders change the list and return it, as in Java
    ids(r#"(function () {
        const list = all();
        const same = list.filterByAnd({sequenceNum: 2}) === list;
        const pairs = all().filterByAnd("description", "third", "enumTypeId", "JT");
        const ordered = all().orderByFields(["-sequenceNum"]);
        const removed = all().removeByAnd({enumId: "JE1"});
        return same && ids(list) === "JE2" && ids(pairs) === "JE3" && ids(ordered) === "JE3,JE2,JE1"
            && ids(removed) === "JE2,JE3" && ids(all().filterByLimit(1, 1)) === "JE2"
            && ids(all().filterByDate(null, null, null)) === "JE1,JE2,JE3";
    })()"#);
    // a new list, the original as it was
    ids(r#"(function () {
        const list = all();
        const found = list.findByAnd({enumId: "JE3"});
        const copy = list.cloneList();
        copy[0].description = "changed in the copy";
        return ids(found) === "JE3" && list.length === 3 && list[0].description === "first" && copy.length === 3
            && list.getFirst().enumId === "JE1" && list.indexMatching({enumId: "JE2"}) === 1
            && list.getValueMapList()[2].enumId === "JE3" && list.getPlainValueList(0)[0].enumId === "JE1";
    })()"#);
    // Java's List changes, and the array methods that change an array
    ids(r#"(function () {
        const list = all();
        const extra = ec.entity.makeValue("moqui.basic.Enumeration").setAll({enumId: "JE0", enumTypeId: "JT"});
        list.add(extra);
        list.add(0, extra.cloneValue().set("enumId", "JE00"));
        const added = ids(list) === "JE00,JE1,JE2,JE3,JE0";
        const removedRow = list.remove(0).enumId === "JE00" && list.remove(extra) === true && list.length === 3;
        list.addIfMissing(list[0]).addIfMissing(extra);
        const missing = list.length === 4;
        list.move(3, 0);
        const moved = ids(list) === "JE0,JE1,JE2,JE3";
        list.sort((a, b) => a.enumId < b.enumId ? 1 : -1);
        const sorted = ids(list) === "JE3,JE2,JE1,JE0";
        list.reverse();
        const popped = list.pop().enumId === "JE3" && list.shift().enumId === "JE0";
        list.push(extra);
        list.set(0, extra);
        const set = ids(list) === "JE0,JE2,JE0";
        list.clear();
        return added && removedRow && missing && moved && sorted && popped && set && list.isEmpty();
    })()"#);
    // the helper functions that change a list work on an entity list
    ids(r#"(function () {
        const list = all();
        orderMapList(list, ["-enumId"]);
        const ordered = ids(list) === "JE3,JE2,JE1";
        filterMapList(list, {enumId: "JE2"});
        return ordered && ids(list) === "JE2";
    })()"#);
    clear_context_stack();
}

#[test]
fn finds_conditions_and_the_entity_facade_take_javas_forms() {
    let context = database();
    let _entered = context.enter();
    seed_enums();
    let ids = |script: &str| {
        holds(&script.replacen(
            "(function () {",
            r#"(function () { const cf = ec.entity.conditionFactory; const find = () => ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "JT"); const ids = (list) => Array.from(list, (row) => row.enumId).join();"#,
            1,
        ));
    };
    // a condition joined to a condition; it was no condition at all before
    ids(r#"(function () {
        const one = cf.makeCondition("enumId", "equals", "JE1"), three = cf.makeCondition("enumId", "equals", "JE3");
        const third = cf.makeCondition("description", "equals", "third");
        const either = cf.makeCondition(one, "or", three), both = cf.makeCondition(one, "and", third);
        return either != null && ids(find().condition(either).orderBy("enumId").list()) === "JE1,JE3"
            && find().condition(both).list().length === 0 && find().condition(cf.makeCondition(three, "and", third)).count() === 1
            && cf.makeCondition(one, "and", null) != null;
    })()"#);
    // the operator arguments of the map and list forms
    ids(r#"(function () {
        const notOne = cf.makeCondition({enumId: "JE1"}, "not-equals", "and");
        const anyOf = cf.makeCondition({enumId: "JE1", description: "third"}, "equals", "or");
        const listed = cf.makeCondition([{enumId: "JE2"}, cf.makeCondition("enumId", "equals", "JE3")], "or");
        return ids(find().condition(notOne).orderBy("enumId").list()) === "JE2,JE3"
            && ids(find().condition(anyOf).orderBy("enumId").list()) === "JE1,JE3"
            && ids(find().condition(listed).orderBy("enumId").list()) === "JE2,JE3";
    })()"#);
    ids(r#"(function () {
        const toField = cf.makeConditionToField("enumId", "not-equals", "enumTypeId");
        return find().condition(toField).count() === 3 && find().conditionToField("enumId", "equals", "enumTypeId").count() === 0
            && String(cf.getTrueCondition()) === "1 = 1" && cf.comparisonOperatorFromEnumId("ENTCO_GTR_EQ") === "greater-equals";
    })()"#);
    // a condition checked against a map or a value in memory
    ids(r#"(function () {
        const condition = cf.makeCondition(cf.makeCondition("enumTypeId", "equals", "JT"), "and", cf.makeCondition("sequenceNum", "greater", 1));
        const row = find().condition("enumId", "JE3").one();
        const populated = cf.makeCondition({enumId: "JE1", enumTypeId: "JT"}).populateMap({kept: true});
        return condition.mapMatches({enumTypeId: "JT", sequenceNum: 2}) && !condition.mapMatches({enumTypeId: "JT", sequenceNum: 1})
            && condition.mapMatches(row) && condition.mapMatchesAny({enumTypeId: "JT", sequenceNum: 0})
            && condition.mapKeysNotContained({other: 1}) && !condition.mapKeysNotContained({sequenceNum: 1})
            && populated.kept === true && populated.enumId === "JE1" && String(condition).includes("enumTypeId = JT")
            && ids(find().orderBy("enumId").list().filterByCondition(condition, true)) === "JE2,JE3";
    })()"#);
    // lists of names, as Java takes them
    ids(r#"(function () {
        const ordered = find().orderBy(["-sequenceNum", "enumId"]).list();
        const selected = find().selectFields(["enumId", "description"]);
        return ids(ordered) === "JE3,JE2,JE1" && selected.getSelectFields().join() === "enumId,description"
            && find().selectField("enumId,enumTypeId").getSelectFields().length === 2
            && find().getEntityName() === "moqui.basic.Enumeration"
            && find().fetchSize(100).maxRows(10).getFetchSize() === null;
    })()"#);
    ids(r#"(function () {
        const one = ec.entity.fastFindOne("moqui.basic.Enumeration", false, true, "JE2");
        ec.entity.createBulk([
            ec.entity.makeValue("moqui.basic.Enumeration").setAll({enumId: "JE7", enumTypeId: "JT"}),
            ec.entity.makeValue("moqui.basic.Enumeration").setAll({enumId: "JE8", enumTypeId: "JT"})]);
        return one.description === "second" && ec.entity.fastFindOne("moqui.basic.Enumeration", true, false) === null
            && find().count() === 5 && ec.entity.getEntityGroupName("moqui.basic.Enumeration") !== undefined
            && ec.entity.getEntityGroupName("no.Such") === null;
    })()"#);
    clear_context_stack();
}

#[test]
fn service_call_names_and_rest_client_members() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    holds(
        r#"ec.service.sync().name("create", "moqui.basic.Enumeration").getServiceName() === "create#moqui.basic.Enumeration"
            && ec.service.sync().name("org.moqui.impl.UserServices", "update", "Password").getServiceName()
                === "org.moqui.impl.UserServices.update#Password"
            && ec.service.sync().name("a.b.do#Thing").getServiceName() === "a.b.do#Thing"
            && ec.service.async().name("store", "x.Y").getServiceName() === "store#x.Y""#,
    );
    holds(
        r#"(function () {
            const call = ec.service.sync().name("a.do#It").parameter("one", 1).parameters({two: 2});
            const held = call.getCurrentParameters();
            return held.one === 1 && held.two === 2;
        })()"#,
    );
    holds(
        r#"(function () {
            const client = ec.service.rest().method("post").basicAuth("user", "pass").acceptContentType("text/xml").encoding("UTF-8");
            const built = ec.service.rest().uri().protocol("https").host("example.com").port(443).path("a/b")
                .parameter("q", "x y").fragment("part").build();
            return client.getMethod() === "POST"
                && client.headerList.some(([name, value]) => name === "Authorization" && value === "Basic " + StringUtilities.base64Encode("user:pass"))
                && client.headerList.some(([name, value]) => name === "Accept" && value === "text/xml")
                && built.getUriString() === "https://example.com:443/a/b?q=x%20y#part"
                && typeof client.callFuture === "function";
        })()"#,
    );
    clear_context_stack();
}
