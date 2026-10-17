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
mod bounded;
pub mod groovy_convert;
mod lint_hazards;
mod parse;
mod runtime;
pub mod screen_lint;
pub mod template;
pub(crate) mod walker;

pub(crate) use actions::run_actions_at;
pub use actions::{check_actions, eval_condition_element, eval_condition_group, run_actions};
pub(crate) use bounded::BoundedMap;
pub(crate) use parse::check_program;
pub use parse::parse_js;
#[cfg(test)]
pub(crate) use runtime::script_time_remaining;
#[cfg(test)]
pub(crate) use runtime::with_ctx as walker_context;
pub use runtime::{
    clear_context_stack, context_value, eval_map, eval_value, expand, push_context_frame, run_script,
    run_script_location, script_list_values, set_context_name, set_widget_wrap, top_context_entries,
};
pub(crate) use runtime::{
    context_combined, eval_named_value, mark_request_thread, put_context_all, text_upload_to_js, value_entity_name,
    value_field, value_items, value_map, with_ctx, with_screen_runner,
};
pub use runtime::{eval_condition, expand_localized, ContextFrame, DeadlineGuard, IsolatedContext};

#[cfg(test)]
mod tests {
    use super::{parse_js, run_actions};

    #[test]
    fn parse_js_accepts_arithmetic_and_actions_return_ok() {
        parse_js("1 + 1").unwrap();
        parse_js("{a: b, c: [1, 2]}").unwrap();
        assert!(parse_js("def x = 1").is_err());
        assert!(parse_js("a b").is_err());
        run_actions("<actions/>").unwrap();
    }
}
