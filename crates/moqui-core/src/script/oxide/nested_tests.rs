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
    let context = super::engine::context_for_tests();
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
    {
        crate::script::run_script(
            "ids = []; ids.push('a'); serviceCall(); if (!ids.includes('b')) { serviceCall(); ids.push('b') }",
        )
    }?;
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
    let context = super::engine::context_for_tests();
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
    {
        crate::script::run_script("items.push(5); sameFrame()")
    }?;
    assert_eq!(crate::script::context_value("seen"), Some(Value::I32(2)));
    assert_eq!(
        crate::script::context_value("items"),
        Some(Value::List(vec![Value::I32(9)]))
    );
    Ok(())
}

/// A screen unit run inside a running script (a host call that renders a section) is a part of that run, as QuickJS's
/// `in_unit_scope`: the caller's tracked values are written back and released before the unit, its released ones set
/// aside while the unit runs, and what the unit read written back after, so the caller reads by name what the unit
/// left. A unit that throws gives the caller's values back the same way.
#[test]
fn a_unit_releases_the_callers_values_and_sets_them_aside() -> Result<(), Error> {
    use crate::screen::sri::Runner;
    use quickjs_oxide::engine::api::core::{Scope, Value as JsValue};
    clear_context_stack();
    push_context_frame();
    crate::script::set_context_name("rows", Value::List(Vec::new()))?;
    let unit = |id: u64, source: &str| crate::screen::generate::Unit {
        id,
        source: source.to_string(),
        runtime: std::sync::Arc::from(""),
        runtime_id: 0,
    };
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    context
        .define_function(&global, "renderUnit", 1, move |scope: &mut Scope<'_>| -> Result<JsValue, _> {
            let throws = matches!(scope.arg(0), JsValue::Bool(true));
            let ran = super::bind::lend(scope, || {
                Ok(if throws {
                    super::units::RUNNER.run(&unit(
                        u64::MAX - 2,
                        "(function ($depth) { rows.push('unit'); throw new Error('unit failed') })",
                    ))
                } else {
                    super::units::RUNNER.run(&unit(u64::MAX - 1, "(function ($depth) { rows.push('unit') })"))
                })
            })?;
            match ran {
                Ok(()) => Ok(JsValue::Undefined),
                Err(err) => {
                    Err(scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, err.message()))
                }
            }
        })
        .unwrap();
    {
        crate::script::run_script(
            "rows.push('before'); renderUnit(false); rows.push('after'); \
             try { renderUnit(true) } catch (e) { failed = String(e.message).includes('unit failed') } \
             rows.push('last')",
        )
    }?;
    assert_eq!(crate::script::context_value("failed"), Some(Value::Bool(true)));
    let texts = |names: &[&str]| Value::List(names.iter().map(|name| Value::string(*name)).collect());
    assert_eq!(
        crate::script::context_value("rows"),
        Some(texts(&["before", "unit", "after", "unit", "last"]))
    );
    Ok(())
}

/// A value the caller keeps by name (a decimal) is set aside while a unit runs in a context frame that names it
/// differently: the unit reads its frame's, and the caller its own after.
#[test]
fn a_unit_reads_its_own_frame_not_the_callers_kept_values() -> Result<(), Error> {
    use crate::screen::sri::Runner;
    use quickjs_oxide::engine::api::core::{JsString, Scope, Value as JsValue};
    let decimal = |text: &str| moqui_lib::object::basic_convert(&Value::string(text), "BigDecimal").value;
    clear_context_stack();
    push_context_frame();
    crate::script::set_context_name("price", decimal("1.5"))?;
    let context = super::engine::context_for_tests();
    let global = context.global_object().unwrap();
    context
        .define_function(&global, "renderPriced", 0, move |scope: &mut Scope<'_>| -> Result<JsValue, _> {
            let ran = super::bind::lend(scope, || {
                let _frame = crate::script::runtime::ContextFrame::push();
                let ran = crate::script::set_context_name("price", decimal("2.5")).and_then(|()| {
                    super::units::RUNNER.run(&crate::screen::generate::Unit {
                        id: u64::MAX - 3,
                        source: "(function ($depth) { unitPrice = price.toString() })".to_string(),
                        runtime: std::sync::Arc::from(""),
                        runtime_id: 0,
                    })
                });
                Ok(ran.map(|()| crate::script::context_value("unitPrice")))
            })?;
            let text = match ran {
                Ok(Some(value)) => moqui_lib::object::to_plain_string(&value),
                Ok(None) => "none".to_string(),
                Err(err) => err.message().to_string(),
            };
            Ok(JsValue::String(JsString::try_from_utf8(&text).unwrap()))
        })
        .unwrap();
    {
        crate::script::run_script(
            "before = price.toString(); seen = renderPriced(); after = price.toString()",
        )
    }?;
    assert_eq!(crate::script::context_value("before"), Some(Value::string("1.5")));
    assert_eq!(crate::script::context_value("seen"), Some(Value::string("2.5")), "the unit's frame");
    assert_eq!(crate::script::context_value("after"), Some(Value::string("1.5")));
    Ok(())
}

/// The stack each level of [`nest_on_thread`] holds: about twice what a nested service call takes in a Debug build.
const LEVEL_STACK: usize = 96 * 1024;

/// Run `body` with [`LEVEL_STACK`] of this thread's stack held under it.
#[inline(never)]
fn holding_stack<T>(body: impl FnOnce() -> T) -> T {
    let held = [0_u8; LEVEL_STACK];
    std::hint::black_box(&held);
    let out = body();
    std::hint::black_box(&held);
    out
}

/// `nest(levels)` on a new thread with 8 MiB of stack, declared to the engine or not: each level is a host call that
/// holds [`LEVEL_STACK`] and runs the next level as a nested script, as a service a script calls does.
fn nest_on_thread(levels: i32, declared: bool) -> Result<(), String> {
    let body = move || -> Result<(), String> {
        use quickjs_oxide::engine::api::core::{NativeErrorKind, Scope, Value as JsValue};
        clear_context_stack();
        push_context_frame();
        let context = super::engine::context_for_tests();
        let global = context.global_object().unwrap();
        context
            .define_function(&global, "nest", 1, |scope: &mut Scope<'_>| -> Result<JsValue, _> {
                let JsValue::Int(left @ 1..) = scope.arg(0) else {
                    return Ok(JsValue::Undefined);
                };
                let ran = scope.lend(|| holding_stack(|| crate::script::run_script(&format!("nest({})", left - 1))));
                match ran {
                    Ok(()) => Ok(JsValue::Undefined),
                    Err(err) => Err(scope.throw_error(NativeErrorKind::Type, err.message())),
                }
            })
            .unwrap();
        crate::script::run_script(&format!("nest({levels})")).map_err(|err| err.message().to_string())
    };
    let size = 8 * 1024 * 1024;
    let spawned = if declared {
        crate::script::spawn_script_thread("nest".to_string(), size, body)
    } else {
        std::thread::Builder::new().stack_size(size).spawn(body)
    };
    spawned
        .unwrap()
        .join()
        .expect("a nesting too deep fails as a script error, not by overflowing the thread")
}

/// Scripts nested 20 deep under host calls that hold about 2 MiB of the thread's stack run on a thread whose 8 MiB stack
/// is declared, in every build. Not declared, quickjs-oxide counts on 1 MiB of it, less its reserve: a Debug data load
/// stopped with `stack overflow` a dozen services deep (`load#DemoOrder` in mr-boss), where release went on.
#[test]
fn a_declared_thread_stack_holds_scripts_nested_deep() {
    assert_eq!(nest_on_thread(20, true), Ok(()));
    let err = nest_on_thread(20, false).unwrap_err();
    assert!(err.contains("stack"), "{err}");
}
