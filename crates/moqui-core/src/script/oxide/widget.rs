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

//! The server-static widget check's wrapper of `ec` on quickjs-oxide, as QuickJS's `with_widget_ec` (`runtime.rs`):
//! while the check is on (`runtime::WidgetWrap`), a widget expression runs with the global `ec` replaced by
//! [`crate::script::runtime::WRAPPED_EC`]'s wrapper, which records an area outside the browser's host through `deny`
//! and throws. The real `ec` is put back when the expression ends, however it ends, and the check is off for what the
//! expression calls into (an `expand` inside it is not checked twice). A run nested in it (a host function that runs a
//! script) gets the real `ec` for its run ([`RealEcForNested`]), as on QuickJS.

use std::cell::RefCell;
use std::rc::Rc;

use quickjs_oxide::engine::api::core::{
    Context, FunctionOptions, Js, JsResult, ObjectRef, OrdinaryPropertyDescriptor, RuntimeError, Scope, Value,
};

use super::convert::key;
use super::engine::OxideEngine;

/// The name the wrapper's maker is compiled under, and kept by in the runtime's compiled cache.
const FILE_NAME: &str = "wrapped_ec";

thread_local! {
    /// The real `ec` while a widget expression runs with the wrapper in its place, with the runtime it is of.
    static WRAPPED_REAL: RefCell<Option<(*const OxideEngine, Value)>> = const { RefCell::new(None) };
}

/// Run `body`, a widget expression, with the wrapped `ec` when the widget check is on. `expression`, when given, is
/// the text the check records a violation against; the small kinds' entry points set it before they route
/// ([`crate::script::runtime::note_widget_expression`]).
pub(super) fn with_widget_ec<T>(
    engine: &OxideEngine,
    expression: Option<&str>,
    body: impl FnOnce() -> Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    if !crate::script::runtime::widget_wrap_on() {
        return body();
    }
    if let Some(expression) = expression {
        crate::screen::statics::set_expression(expression);
    }
    let mut context = engine.context().clone();
    let global = context.global_object()?;
    let ec = key("ec").map_err(|_| RuntimeError::Invariant("a key"))?;
    let real = context.get_property(&global, &ec)?;
    let wrapper = make_wrapper(engine, &mut context, real.clone())?;
    define_ec(&mut context, &global, wrapper)?;
    let previous = WRAPPED_REAL.with(|held| held.replace(Some((std::ptr::from_ref(engine), real.clone()))));
    let _real = RealEc {
        engine,
        context,
        global,
        real,
        previous,
    };
    let _unchecked = crate::script::runtime::WidgetWrap::set(false);
    body()
}

/// The wrapper of `real`, from the maker compiled once for the runtime.
fn make_wrapper(engine: &OxideEngine, context: &mut Context, real: Value) -> Result<Value, RuntimeError> {
    let source = crate::script::runtime::WRAPPED_EC;
    let hash = crate::script::text::hash_text(source);
    let compiled = engine.compile(source, hash, FILE_NAME)?;
    let maker = match context.execute(&compiled)? {
        Value::Object(object) => context.as_callable(&object)?,
        _ => None,
    }
    .ok_or(RuntimeError::Invariant("the wrapped ec's maker"))?;
    let deny = context.new_function(
        FunctionOptions::new("deny").length(1),
        |scope: &mut Scope<'_>| -> JsResult<Value> {
            let area = match scope.arg(0) {
                Value::String(text) => text.to_utf8_lossy(),
                other => scope.to_js_string(&other)?.to_utf8_lossy(),
            };
            crate::screen::statics::record_wrapped(&area);
            Ok(Value::Undefined)
        },
    )?;
    context.call(&maker, Value::Undefined, &[real, Value::from(deny)])
}

/// Replace the global `ec`, which is read-only to scripts and so defined again, as QuickJS's `set_ec`.
fn define_ec(context: &mut Context, global: &ObjectRef, value: Value) -> Result<(), RuntimeError> {
    let ec = key("ec").map_err(|_| RuntimeError::Invariant("a key"))?;
    context.define_own_property(
        global,
        &ec,
        &OrdinaryPropertyDescriptor::data(value, false, false, true),
    )?;
    Ok(())
}

/// The real `ec` put back on the global object when dropped. Should that fail, the wrapper would stay in front of
/// `ec` for every later script on the thread, so the runtime is dropped and the next script starts a new one.
struct RealEc<'a> {
    engine: &'a OxideEngine,
    context: Context,
    global: ObjectRef,
    real: Value,
    /// What [`WRAPPED_REAL`] held before.
    previous: Option<(*const OxideEngine, Value)>,
}

impl Drop for RealEc<'_> {
    fn drop(&mut self) {
        let real = std::mem::replace(&mut self.real, Value::Undefined);
        if let Err(err) = define_ec(&mut self.context, &self.global, real) {
            tracing::error!(error = ?err, "the real ec could not be put back after a widget expression; the quickjs-oxide runtime is dropped and restarts on the next script");
            super::engine::discard(self.engine);
        }
        let previous = self.previous.take();
        WRAPPED_REAL.with(|held| *held.borrow_mut() = previous);
    }
}

/// A run nested in a widget expression's (a host function it calls runs a script) has the real `ec` while it runs,
/// and the global it found is put back when it ends: it is not the screen's widget expression. Only in the runtime
/// the wrapped expression runs in: a run of another (the spec runtime's) has its own `ec`. A put-back that fails
/// drops the runtime, as [`RealEc`]'s does.
pub(super) struct RealEcForNested {
    engine: Rc<OxideEngine>,
    outer: Option<(Context, ObjectRef, Value)>,
}

impl RealEcForNested {
    pub(super) fn enter(engine: &Rc<OxideEngine>) -> Self {
        let held = WRAPPED_REAL.with(|held| held.borrow().clone());
        let Some(real) = held.and_then(|(of, real)| std::ptr::eq(of, Rc::as_ptr(engine)).then_some(real)) else {
            return Self {
                engine: Rc::clone(engine),
                outer: None,
            };
        };
        let mut context = engine.context().clone();
        let swapped = (|| -> Result<(ObjectRef, Value), RuntimeError> {
            let global = context.global_object()?;
            let ec = key("ec").map_err(|_| RuntimeError::Invariant("a key"))?;
            let outer = context.get_property(&global, &ec)?;
            define_ec(&mut context, &global, real)?;
            Ok((global, outer))
        })();
        match swapped {
            Ok((global, outer)) => Self {
                engine: Rc::clone(engine),
                outer: Some((context, global, outer)),
            },
            Err(err) => {
                tracing::error!(error = ?err, "a run nested in a widget expression could not have the real ec");
                Self {
                    engine: Rc::clone(engine),
                    outer: None,
                }
            }
        }
    }
}

impl Drop for RealEcForNested {
    fn drop(&mut self) {
        if let Some((mut context, global, outer)) = self.outer.take() {
            if let Err(err) = define_ec(&mut context, &global, outer) {
                tracing::error!(error = ?err, "the widget check's ec could not be put back after a nested run; the quickjs-oxide runtime is dropped and restarts on the next script");
                super::engine::discard(&self.engine);
            }
        }
    }
}
