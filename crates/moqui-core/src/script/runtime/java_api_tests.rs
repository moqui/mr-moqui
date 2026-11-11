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

use crate::context::ExecutionContextFactory;
use crate::script::{clear_context_stack, eval_condition, run_script};

fn holds(expression: &str) {
    assert!(eval_condition(expression).unwrap(), "{expression}");
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
