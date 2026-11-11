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
