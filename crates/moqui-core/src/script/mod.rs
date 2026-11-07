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

//! Script facade: server expressions and XML actions on QuickJS.

mod actions;
pub mod bounded;
pub(crate) mod census;
pub(crate) mod check;
pub mod engine;
pub mod host;
pub(crate) mod oxide;
pub mod parse;
pub(crate) mod run;
mod runtime;
pub mod template;
pub(crate) mod text;
pub mod theme;
pub(crate) mod walker;

pub use census::write_census;
pub use engine::{script_engine, set_thread_script_engine, with_script_engine, ScriptEngine};

pub(crate) use actions::run_actions_at;
pub use actions::{check_actions, eval_condition_element, eval_condition_group, generated_source, run_actions};
pub(crate) use bounded::BoundedMap;
pub(crate) use parse::check_program;
pub use parse::parse_js;
pub use runtime::forget_scripts;
#[cfg(test)]
pub(crate) use runtime::script_time_remaining;
pub(crate) use runtime::with_ctx as walker_context;
pub use runtime::{
    clear_context_stack, context_value, engine_memory, eval_field_value, eval_map, eval_value, expand,
    push_context_frame, run_script, run_script_location, script_list_values, set_context_name, set_widget_wrap,
    take_context_value, top_context_entries,
};
pub(crate) use runtime::{
    context_combined, invoke_web_handler, mark_request_thread, put_context_all, value_entity_name, value_field,
    with_real_ec, with_screen_runner, WidgetWrap,
};
pub use runtime::{
    engine_stats, eval_condition, expand_localized, ContextFrame, DeadlineGuard, EngineMemory, EngineStats,
    IsolatedContext,
};
pub use runtime::{text_upload_to_js, with_ctx, with_spec_ctx};
// for host callbacks outside this crate: moqui-llm's ec.llm
pub use runtime::{eval_named_value, install_spec_global, value_items, value_map};
pub use runtime::{facade_of, host_guard, host_to_js, js_error, js_to_host, throw_lib, with_ec};

#[cfg(test)]
mod tests {
    use super::{parse_js, run_actions};

    /// A module whose top level only declares may be kept for a run; one with a memo, a call or a read of a name it does
    /// not declare (a context value) may not.
    #[test]
    fn a_module_that_only_declares_is_inert() {
        use super::parse::module_is_inert;
        let lib = r#"
            const lib = ec.resource.script("component://framework/service/Lib.js")
            const {find, one} = lib
            const P = "mantle.product.", E = P + "ProductPrice"
            const LIMIT = 24 * 7
            const kinds = new Set(["a", "b"])
            const table = {open: {label: "Open", statuses: ["A"]}}
            const policy = lib.policy
            function execute(op, input) { return operations[op](input) }
            const operations = {find, run: input => one(E, input)};
            ({execute, LIMIT})
        "#;
        assert!(module_is_inert(lib));
        assert!(
            !module_is_inert("let memo = null\nfunction f() { return memo }\n({f})"),
            "a memo"
        );
        assert!(
            !module_is_inert("const rows = ec.entity.find('X').list()\n({rows})"),
            "rows read at load"
        );
        assert!(!module_is_inert("const user = partyId\n({user})"), "a context name");
        assert!(
            !module_is_inert("const table = {}\ntable.a = 1\n({table})"),
            "a change at load"
        );
        assert!(!module_is_inert("function f() {}\nf()"), "a value computed at load");
    }

    #[test]
    fn parse_js_accepts_arithmetic_and_actions_return_ok() {
        parse_js("1 + 1").unwrap();
        parse_js("{a: b, c: [1, 2]}").unwrap();
        assert!(parse_js("def x = 1").is_err());
        assert!(parse_js("a b").is_err());
        run_actions("<actions/>").unwrap();
    }

    #[test]
    fn groovy_hints_find_closure_calls_and_arrows() {
        use super::parse::{has_closure_call, has_groovy_arrow};
        assert!(has_closure_call("list.each { it }"));
        assert!(has_closure_call("each{"));
        assert!(has_closure_call("x.findAll{ a } + list.sort {"));
        assert!(!has_closure_call("list.forEach(x => x)"));
        assert!(!has_closure_call("eachItem { }"));
        assert!(!has_closure_call("preeach { }"));
        assert!(!has_closure_call("\u{e9}each(1)"));
        assert!(has_closure_call("\u{e9} each {"));
        assert!(has_groovy_arrow("{ a -> a }"));
        assert!(has_groovy_arrow("->"));
        assert!(!has_groovy_arrow("a => a"));
        assert!(!has_groovy_arrow("a =-> b"));
    }
}
