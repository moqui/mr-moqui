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

//! A run nested in a running oxide script, under a host call that lent the engine.

use moqui_lib::object::Value;

use crate::context::Error;
use crate::script::engine::{with_script_engine, ScriptEngine};
use crate::script::run::STACK;
use crate::script::{clear_context_stack, push_context_frame};

/// A host call that pushes a context frame of its own (a service's) and runs a script there leaves the outer run's
/// tracked values as they are, as QuickJS's nested run does: they are written back to the outer run's frame at its
/// end, not to the call's frame, which goes with the call (`setup#UserOrganizationInfo` lost the organization it
/// pushed before an entity find).
#[test]
fn a_nested_run_leaves_the_outer_runs_values_to_its_frame() -> Result<(), Error> {
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    let context = with_script_engine(ScriptEngine::Oxide, super::engine::context_for_tests);
    let global = context.global_object().unwrap();
    context
        .define_function(
            &global,
            "serviceCall",
            0,
            |scope: &mut Scope<'_>| -> Result<JsValue, _> {
                let ran = scope.lend(|| {
                    // a context of its own, as a service call has: the outer frames are out of its sight
                    STACK.with(|stack| stack.borrow_mut().push_context());
                    let ran = crate::script::run_script("inner = 1");
                    STACK
                        .with(|stack| stack.borrow_mut().pop_context())
                        .map_err(|err| Error::new(err.message()))?;
                    ran
                });
                match ran {
                    Ok(()) => Ok(JsValue::Undefined),
                    Err(err) => {
                        Err(scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, err.message()))
                    }
                }
            },
        )
        .unwrap();
    with_script_engine(ScriptEngine::Oxide, || {
        crate::script::run_script(
            "ids = []; ids.push('a'); serviceCall(); if (!ids.includes('b')) { serviceCall(); ids.push('b') }",
        )
    })?;
    assert_eq!(
        crate::script::context_value("ids"),
        Some(Value::List(vec![Value::string("a"), Value::string("b")]))
    );
    assert_eq!(
        crate::script::context_value("inner"),
        None,
        "the call's frame went with it"
    );
    Ok(())
}

/// A lend outside the table's methods (here the engine's own, `bind::lend`) whose nested script runs in the same frame
/// reads what the outer run changed in place, as a QuickJS script nested there does, and a value the nested script
/// assigns is not written over at the outer run's end.
#[test]
fn a_nested_run_in_the_same_frame_reads_the_outer_runs_changes() -> Result<(), Error> {
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    crate::script::set_context_name("items", Value::List(vec![Value::I32(1)]))?;
    let context = with_script_engine(ScriptEngine::Oxide, super::engine::context_for_tests);
    let global = context.global_object().unwrap();
    context
        .define_function(&global, "sameFrame", 0, |scope: &mut Scope<'_>| -> Result<JsValue, _> {
            let ran = super::bind::lend(scope, || {
                Ok(crate::script::run_script("seen = items.length; items = [9]"))
            })?;
            match ran {
                Ok(()) => Ok(JsValue::Undefined),
                Err(err) => {
                    Err(scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, err.message()))
                }
            }
        })
        .unwrap();
    with_script_engine(ScriptEngine::Oxide, || {
        crate::script::run_script("items.push(5); sameFrame()")
    })?;
    assert_eq!(crate::script::context_value("seen"), Some(Value::I32(2)));
    assert_eq!(
        crate::script::context_value("items"),
        Some(Value::List(vec![Value::I32(9)]))
    );
    Ok(())
}
