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

//! Screen units on quickjs-oxide under `script_engine=oxide`: the [`Runner`] a render runs its units with, the oxide
//! side of `runtime/screen_host.rs`. Generation runs on the thread's oxide generator (`screen::generate`,
//! `oxide::walker`); a unit runs on the thread's main oxide runtime.
//!
//! - **At the top level** (a render started from Rust, no oxide script running) each unit is a run of its own
//!   ([`super::engine::with_run`]): its depth, deadline and run state, its tracked values written back at its end.
//! - **Inside a running script** (a `sri.render*` call, which lends the engine) a unit runs in the run that called it,
//!   as QuickJS's `in_unit_scope`: the caller's tracked values are released before and its released values set aside,
//!   and what the unit read is written back and forgotten after.
//!
//! A unit's mode runtime (`unit.runtime`) runs once per runtime, its function (`unit.source`, a `(function($depth)
//! {...})` text) is compiled once per unit id, and it is called with the depth of units it runs inside, one more for
//! the units it renders, given back however it ends. A unit that throws leaves the context stack as it found it.

use std::cell::Cell;
use std::sync::Arc;

use quickjs_oxide::engine::api::core::{Context, RuntimeError, Value};

use super::engine::{current, with_run, Outcome, OxideEngine};
use super::errors::runtime_error;
use super::tracked::Flush;
use crate::context::Error;
use crate::screen::form::FormInstance;
use crate::screen::generate::{self, Unit, UnitRequest};
use crate::screen::sri::Runner;
use crate::script::run::{self, EngineTag};

/// Compiled unit functions a runtime keeps, and their source bytes, as QuickJS's engine keeps them.
pub(super) const MAX_UNITS: usize = 4096;
pub(super) const MAX_UNIT_BYTES: usize = 8 * 1024 * 1024;

/// The runner of the thread's oxide runtime.
pub(crate) struct OxideRunner;

/// The one runner: it holds nothing, the runtime is the thread's.
pub(crate) static RUNNER: OxideRunner = OxideRunner;

impl Runner for OxideRunner {
    fn generate(&self, request: &UnitRequest<'_>) -> Result<Option<Arc<Unit>>, Error> {
        generate::unit_for(None, request)
    }

    fn generate_dynamic(&self, request: &UnitRequest<'_>, form: &FormInstance) -> Result<Arc<Unit>, Error> {
        generate::unit_for_dynamic_form(None, request, form)
    }

    fn run(&self, unit: &Unit) -> Result<(), Error> {
        // the widget check's wrapper of `ec` is QuickJS's only: a unit is refused under it rather than run unchecked
        if crate::script::runtime::widget_wrap_on() {
            return Err(Error::new(
                "script_engine=oxide: a screen unit under the server-static widget check does not run on quickjs-oxide yet",
            ));
        }
        if run::is_running(EngineTag::Oxide) {
            let engine = current().ok_or_else(|| Error::new("no quickjs-oxide runtime is running"))?;
            if !super::is_lent() {
                return Err(Error::new(
                    "a screen unit nested in a running quickjs-oxide script needs the engine lent",
                ));
            }
            let mut context = engine.context().clone();
            return in_unit_scope(&engine, &mut context, |context| call_unit(&engine, context, unit));
        }
        crate::script::engine::count(
            crate::script::engine::EntryKind::Host,
            crate::script::engine::Route::Oxide,
        );
        let ran = with_run(true, "a screen unit", |engine, context, note| {
            call_unit(engine, context, unit).map_err(|err| {
                if err.message().contains(super::errors::POISONED) {
                    note(&RuntimeError::Poisoned);
                }
                Outcome::Error(err)
            })
        });
        ran.map_err(|outcome| match outcome {
            Outcome::Error(err) => err,
            Outcome::Fallback(reason) => Error::new(format!(
                "script_engine=oxide: this screen unit needs {reason}, which the quickjs-oxide bindings do not have yet"
            )),
        })
    }

    fn render_inline_text(&self, body: &str) -> Result<String, Error> {
        crate::script::template::render_trusted(body, crate::script::template::Escape::None)
    }
}

/// [`crate::script::with_screen_runner`] under `script_engine=oxide`. Inside a running oxide script (a script that
/// renders a screen) the render is a nested run, its unit depth starting afresh as on QuickJS; at the top level each
/// unit is a run of its own.
pub(crate) fn with_screen_runner<R>(body: impl FnOnce(&dyn Runner) -> Result<R, Error>) -> Result<R, Error> {
    if !run::is_running(EngineTag::Oxide) {
        return crate::script::engine::cross_into_oxide(true, || body(&RUNNER))?;
    }
    let mut body = Some(body);
    let mut result = None;
    let ran = with_run(true, "a screen render", |_, _, _| {
        if let Some(body) = body.take() {
            result = Some(body(&RUNNER));
        }
        Ok(())
    });
    match (ran, result) {
        (_, Some(Err(err))) | (Err(Outcome::Error(err)), _) => Err(err),
        (Err(Outcome::Fallback(reason)), _) => Err(Error::new(format!(
            "script_engine=oxide: this screen render needs {reason}, which the quickjs-oxide bindings do not have yet"
        ))),
        (Ok(()), Some(Ok(value))) => Ok(value),
        (Ok(()), None) => Err(Error::new("the screen render did not run")),
    }
}

/// Run a unit in the run that called it: the caller's tracked values released before and its released ones set aside,
/// what the unit read written back and forgotten after, as QuickJS's `in_unit_scope`.
fn in_unit_scope<T>(
    engine: &OxideEngine,
    context: &mut Context,
    body: impl FnOnce(&mut Context) -> Result<T, Error>,
) -> Result<T, Error> {
    let flush_error = |context: &mut Context, conv| match conv {
        super::convert::Conv::Js(err) => runtime_error(context, err),
        super::convert::Conv::Error(message) => Error::new(message),
        super::convert::Conv::Unsupported(reason) => Error::new(format!("quickjs-oxide could not write back {reason}")),
    };
    super::tracked::flush(context, Flush::Release).map_err(|conv| flush_error(context, conv))?;
    struct Outer<'a> {
        engine: &'a OxideEngine,
        outer: Option<super::tracked::OuterValues>,
    }
    impl Drop for Outer<'_> {
        fn drop(&mut self) {
            if let (Ok(mut values), Some(outer)) = (self.engine.run_values().try_borrow_mut(), self.outer.take()) {
                values.restore_outer(outer);
            }
        }
    }
    let scope = Outer {
        engine,
        outer: Some(engine.run_values().borrow_mut().take_outer()),
    };
    let result = body(context);
    let flushed = super::tracked::flush(context, Flush::End).map_err(|conv| flush_error(context, conv));
    drop(scope);
    result.and_then(|value| flushed.map(|()| value))
}

/// The depth a unit starts at, given back when it ends however it ends.
struct Depth<'a>(&'a Cell<u32>, u32);

impl Drop for Depth<'_> {
    fn drop(&mut self) {
        self.0.set(self.1);
    }
}

/// Run `unit` on the runtime: its mode runtime first if this runtime has not run it, its function compiled once, called
/// with the unit depth.
fn call_unit(engine: &OxideEngine, context: &mut Context, unit: &Unit) -> Result<(), Error> {
    if !unit.runtime.is_empty() && !engine.unit_runtimes.borrow().contains(&unit.runtime_id) {
        let compiled = engine
            .compile(&unit.runtime, unit.runtime_id, super::compiled::FILE_NAME)
            .and_then(|compiled| context.execute(&compiled));
        compiled.map_err(|err| runtime_error(context, err))?;
        engine.unit_runtimes.borrow_mut().insert(unit.runtime_id);
    }
    let saved = engine.units.borrow_mut().get(&unit.id);
    let function = match saved {
        Some(function) => function,
        None => {
            let hash = crate::script::text::hash_text(&unit.source);
            let made = engine
                .compile(&unit.source, hash, super::compiled::FILE_NAME)
                .and_then(|compiled| context.execute(&compiled))
                .map_err(|err| runtime_error(context, err))?;
            let function = match &made {
                Value::Object(object) => context.as_callable(object).map_err(|err| runtime_error(context, err))?,
                _ => None,
            }
            .ok_or_else(|| Error::new("a screen unit is not a function"))?;
            engine
                .units
                .borrow_mut()
                .insert_sized(unit.id, function.clone(), unit.source.len());
            function
        }
    };
    let depth = engine.unit_depth.get();
    engine.unit_depth.set(depth.saturating_add(1));
    let _depth = Depth(&engine.unit_depth, depth);
    let mark = crate::script::runtime::StackMark::new();
    let called = context.call(&function, Value::Undefined, &[Value::number(f64::from(depth))]);
    match called {
        Ok(_) => {
            mark.keep();
            Ok(())
        }
        Err(err) => Err(runtime_error(context, err)),
    }
}
