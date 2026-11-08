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

//! The test harness's spec runtime on quickjs-oxide (`script_engine=oxide`), the counterpart of QuickJS's spec engine
//! (`runtime.rs`, `EngineKind::Spec`): one per thread and of its own, so the globals the harness defines (`call`,
//! `login`, ...) are not in front of the context stack for the scripts of the services a spec calls, which run on the
//! thread's main runtime. It runs whole scripts, so it has no small-script lockdown, and has the globals a whole
//! runtime has (`host::globals`, the helper objects). A host call of a spec that may run
//! a script lends the runtime (the fork's F3a); the script runs on the main runtime as a run of its own, and the
//! conversions follow the innermost runtime ([`super::engine::Active`]).

use std::cell::RefCell;
use std::rc::Rc;

use moqui_lib::object::{HostObject, Value as HostValue};
use quickjs_oxide::engine::api::core::{EvalOptions, OrdinaryPropertyDescriptor, RuntimeError, Value};

use super::convert::{key, to_host, Conv};
use super::engine::{start_whole, take_fallback, Active, OxideEngine, RunState};
use super::errors::runtime_error;
use super::tracked::Flush;
use crate::context::Error;
use crate::script::engine::{count, EntryKind, Route};
use crate::script::host::HostClass;
use crate::script::run::{self, EngineTag, ScriptLimit, MAX_SCRIPT_DEPTH};

thread_local! {
    static SPEC: RefCell<Option<Rc<OxideEngine>>> = const { RefCell::new(None) };
}

/// This thread's spec runtime, when it has started.
pub(super) fn current() -> Option<Rc<OxideEngine>> {
    SPEC.with(|spec| spec.borrow().clone())
}

/// This thread's spec runtime, started the first time.
fn spec_engine() -> Result<Rc<OxideEngine>, Error> {
    if let Some(engine) = SPEC.with(|spec| spec.borrow().clone()) {
        return Ok(engine);
    }
    let started =
        start_whole().map_err(|err| Error::new(format!("the quickjs-oxide spec runtime did not start: {err}")))?;
    SPEC.with(|spec| *spec.borrow_mut() = Some(Rc::clone(&started)));
    Ok(started)
}

/// Run `source` once in the thread's spec runtime, not kept, named `file_name` in stack traces, and give its completion
/// value as a host value ([`crate::script::eval_named_value`]). Like any script on the thread it counts in the shared
/// depth, deadline and default limit, and writes back what it read by name at its end.
pub(crate) fn eval_named_value(source: &str, file_name: &str) -> Result<HostValue, Error> {
    let depth = run::depth();
    if depth >= MAX_SCRIPT_DEPTH {
        return Err(Error::new("scripts are nested too deeply"));
    }
    let engine = spec_engine()?;
    let nested = Active::holds(&engine);
    if nested && !engine.runtime().is_lent() {
        return Err(Error::new(
            "a spec script nested in a running spec script that did not lend its engine",
        ));
    }
    run::stack_budget(depth)?;
    count(EntryKind::Spec, Route::Oxide);
    let _active = Active::enter(&engine);
    let _tag = EngineTag::Oxide.enter();
    let _limit = (depth == 0).then(ScriptLimit::arm);
    let _depth = run::enter_depth(depth);
    let state = RunState::enter(&engine, nested);
    let mut context = engine.context().clone();
    let mut poisoned = false;
    let mut out_of_memory = false;
    let options = EvalOptions::new(file_name).transient();
    let mut outcome = match context.eval_with_options(source, &options) {
        Ok(value) => to_host(&mut context, &engine, &value).map_err(|conv| conv_error(&mut context, conv)),
        Err(err) => {
            poisoned = matches!(err, RuntimeError::Poisoned);
            out_of_memory = matches!(err, RuntimeError::OutOfMemory);
            Err(runtime_error(&mut context, err))
        }
    };
    if !poisoned {
        // as QuickJS does at a script's end, whether it ended well or not
        let flushed = super::tracked::flush(&mut context, Flush::End);
        if let (Ok(_), Err(conv)) = (&outcome, flushed) {
            outcome = Err(conv_error(&mut context, conv));
        }
    }
    if let Some(reason) = take_fallback() {
        outcome = Err(Error::new(format!(
            "script_engine=oxide: this spec needs {reason}, which the quickjs-oxide bindings do not have yet"
        )));
    }
    drop(state);
    if poisoned && !nested {
        tracing::error!(file = %file_name, "a host function panicked in the quickjs-oxide spec runtime; it restarts on the next spec script");
        SPEC.with(|spec| *spec.borrow_mut() = None);
    } else if !poisoned && !nested {
        // the run's garbage goes before the next spec script, as the main runtime's does after one that ran out
        if out_of_memory {
            let _ = engine.runtime().run_gc();
        }
        engine.sample_heap();
    }
    outcome
}

/// Define the global `name` in the thread's spec runtime as an object of the host table's `class` (the harness's
/// `__moquiTestHost`), writable and configurable, as a spec's globals are.
pub(crate) fn install_global(name: &str, class: &'static HostClass) -> Result<(), Error> {
    let engine = spec_engine()?;
    let _active = Active::starting(&engine);
    let mut context = engine.context().clone();
    let defined = (|| -> Result<(), Conv> {
        let object = super::bind::class_instance(&mut context, HostObject::new(()), class)?;
        let global = context.global_object()?;
        context.define_own_property(
            &global,
            &key(name)?,
            &OrdinaryPropertyDescriptor::data(Value::Object(object), true, false, true),
        )?;
        Ok(())
    })();
    defined.map_err(|conv| conv_error(&mut context, conv))
}

fn conv_error(context: &mut quickjs_oxide::engine::api::core::Context, conv: Conv) -> Error {
    match conv {
        Conv::Js(err) => runtime_error(context, err),
        Conv::Error(message) => Error::new(message),
        Conv::Unsupported(reason) => Error::new(format!(
            "script_engine=oxide: this spec needs {reason}, which the quickjs-oxide bindings do not have yet"
        )),
    }
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
