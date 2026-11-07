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

//! Screen units on QuickJS: the [`Runner`] a render runs its units with, and the conversion of the element literals a
//! unit passes to `sri`. The global `sri` itself is the host table's `sri` shim (`script/host/sri.rs`).

use rquickjs::Array;

use super::*;
use crate::screen::generate::{self, Unit, UnitRequest};
use crate::screen::sri::{NodeLit, Runner};

/// Run `body` on the script runtime with a [`Runner`] for the render in progress: under `script_engine=oxide` the
/// thread's quickjs-oxide runtime's (`oxide::units`).
pub(crate) fn with_screen_runner<R>(body: impl FnOnce(&dyn Runner) -> Result<R, Error>) -> Result<R, Error> {
    if crate::script::engine::script_engine() == crate::script::engine::ScriptEngine::Oxide {
        return crate::script::oxide::units::with_screen_runner(body);
    }
    with_context(|ctx| body(&CtxRunner { ctx }))
}

/// The runner of the context a script or a host call runs in.
pub(super) struct CtxRunner<'a, 'js> {
    pub(super) ctx: &'a Ctx<'js>,
}

impl Runner for CtxRunner<'_, '_> {
    fn generate(&self, request: &UnitRequest<'_>) -> Result<Option<Arc<Unit>>, Error> {
        generate::unit_for(Some(self.ctx), request)
    }

    fn generate_dynamic(
        &self,
        request: &UnitRequest<'_>,
        form: &crate::screen::form::FormInstance,
    ) -> Result<Arc<Unit>, Error> {
        generate::unit_for_dynamic_form(Some(self.ctx), request, form)
    }

    fn run(&self, unit: &Unit) -> Result<(), Error> {
        run_unit(self.ctx, unit)
    }

    fn render_inline_text(&self, body: &str) -> Result<String, Error> {
        crate::script::template::render_trusted_here(self.ctx, body, crate::script::template::Escape::None)
    }
}

fn run_unit(ctx: &Ctx<'_>, unit: &Unit) -> Result<(), Error> {
    let engine = current_engine().ok_or_else(|| Error::new("no script engine is running"))?;
    if !unit.runtime.is_empty() && !engine.runtimes.borrow().contains(&unit.runtime_id) {
        eval_js::<JsValue>(ctx, &unit.runtime)?;
        engine.runtimes.borrow_mut().insert(unit.runtime_id);
    }
    let saved = engine.units.borrow_mut().get(&unit.id);
    let function = match saved {
        Some(saved) => saved.restore(ctx).map_err(js_error)?,
        None => {
            let function = eval_js::<Function>(ctx, &unit.source)?;
            engine
                .units
                .borrow_mut()
                .insert_sized(unit.id, Persistent::save(ctx, function.clone()), unit.source.len());
            function
        }
    };
    // the depth this unit starts at, back to it when the unit ends however it ends
    struct Depth<'a>(&'a Cell<u32>, u32);
    impl Drop for Depth<'_> {
        fn drop(&mut self) {
            self.0.set(self.1);
        }
    }
    let depth = engine.unit_depth.get();
    engine.unit_depth.set(depth.saturating_add(1));
    let _depth = Depth(&engine.unit_depth, depth);
    let mark = StackMark::new();
    let result = in_unit_scope(ctx, || {
        function
            .call::<_, ()>((depth,))
            .catch(ctx)
            .map_err(|caught| caught_error(Error::new(caught.to_string()), &caught))
    });
    if result.is_ok() {
        mark.keep();
    }
    result
}

/// Throw an error to the script, carrying a security kind on the error object.
#[cfg(test)]
fn throw(ctx: &Ctx<'_>, err: &Error) -> JsError {
    let exception = match Exception::from_message(ctx.clone(), err.message()) {
        Ok(exception) => exception,
        Err(thrown) => return thrown,
    };
    match mark_kind(&exception, err.kind()).and_then(|()| mark_retry_after(&exception, err.retry_after())) {
        Ok(()) => exception.throw(),
        Err(thrown) => thrown,
    }
}

/// The property of a unit's element literal that holds its conversion, which `sri.holdNodeLits` attaches.
const HELD_LIT: &str = "$nodeLit";

/// The element `value` is: what a unit's literal holds (a host object of `host:NodeLit`), else converted now.
pub(super) fn lit_of(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<Arc<NodeLit>> {
    if let Some(object) = value.as_object() {
        let held: JsValue = object.get(HELD_LIT)?;
        if let Some(class) = held.as_object().and_then(|held| held.as_class::<super::bind::HostNeutral>()) {
            if let Some(node) = class.borrow().obj.downcast::<NodeLit>() {
                return Ok(node);
            }
        }
    }
    lit_in(ctx, value, &mut Vec::new()).map(Arc::new)
}

/// `path` holds the element objects above this one, through `children` and `parent`.
fn lit_in<'js>(ctx: &Ctx<'_>, value: &JsValue<'js>, path: &mut Vec<JsValue<'js>>) -> rquickjs::Result<NodeLit> {
    let Some(object) = value.as_object() else {
        return Err(Exception::throw_message(ctx, "expected an element object"));
    };
    enter_nested(path, value).map_err(|err| Exception::throw_message(ctx, err.message()))?;
    let mut node = NodeLit::default();
    if let Ok(name) = object.get::<_, String>("name") {
        node.name = name;
    }
    if let Ok(text) = object.get::<_, String>("text") {
        node.text = text;
    }
    if let Ok(attrs) = object.get::<_, Object>("attrs") {
        for entry in attrs.props::<String, String>() {
            let (key, item) = entry?;
            node.attrs.insert(key, item);
        }
    }
    if let Ok(children) = object.get::<_, Array>("children") {
        for item in children.iter::<JsValue>() {
            node.children.push(lit_in(ctx, &item?, path)?);
        }
    }
    if let Ok(parent) = object.get::<_, JsValue>("parent") {
        if parent.is_object() {
            node.parent = Some(Box::new(lit_in(ctx, &parent, path)?));
        }
    }
    path.pop();
    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kind `source` ends with, after `deny()` throws an unauthorized error and `fail()` a general one.
    fn kind_after(source: &str) -> ErrorKind {
        with_context(|ctx| {
            let deny = Function::new(ctx.clone(), |ctx: Ctx<'_>| -> rquickjs::Result<()> {
                Err(throw(&ctx, &Error::of_kind(ErrorKind::Unauthorized, "denied")))
            })
            .map_err(js_error)?;
            let fail = Function::new(ctx.clone(), |ctx: Ctx<'_>| -> rquickjs::Result<()> {
                Err(throw(&ctx, &Error::new("failed")))
            })
            .map_err(js_error)?;
            let function = eval_js::<Function>(ctx, &format!("(function (deny, fail) {{ {source} }})"))?;
            let caught = function
                .call::<_, ()>((deny, fail))
                .catch(ctx)
                .expect_err("the script did not throw");
            Ok(caught_kind(&caught))
        })
        .unwrap()
    }

    #[test]
    fn a_security_kind_goes_with_its_error_only() {
        assert_eq!(kind_after("deny();"), ErrorKind::Unauthorized);
        assert_eq!(
            kind_after("try { deny(); } catch (e) { throw e; }"),
            ErrorKind::Unauthorized
        );
        // Caught and gone on from: the later, unrelated failure is general.
        assert_eq!(kind_after("try { deny(); } catch (e) {} fail();"), ErrorKind::General);
        assert_eq!(
            kind_after("try { deny(); } catch (e) {} throw new Error('x');"),
            ErrorKind::General
        );
        // A general failure caught first does not hide the security error after it.
        assert_eq!(
            kind_after("try { fail(); } catch (e) {} deny();"),
            ErrorKind::Unauthorized
        );
        assert_eq!(kind_after("throw 'text';"), ErrorKind::General);
    }

    #[test]
    fn sri_encode_html_is_null_safe() {
        let holds = |expression: &str| assert!(super::super::eval_condition(expression).unwrap(), "{expression}");
        super::super::clear_context_stack();
        holds(r#"sri.encodeHtml(null) === "" && sri.encodeHtml(undefined) === "" && sri.encodeHtml() === """#);
        holds(
            r#"sri.encodeHtml(7) === "7" && sri.encodeHtml(true) === "true" && sri.encodeHtml("<&>") === "&lt;&amp;&gt;""#,
        );
    }
}
