//! Component tools in scripts: `ec.getTool(name, ...parameters)`, `ec.factory.getTool` and
//! `ec.factory.getToolFactory(name)`.
//!
//! A tool is an exotic object over the Rust [`Tool`](crate::native::Tool): a name in its `methods()` reads as a function
//! that converts its arguments and result as service parameters are, and any other name reads through its
//! `property()`. Only those cross the boundary, never arbitrary Rust objects. The object holds the tool, not data, so it
//! is the same tool after the context stack keeps it (`<set field="tool" from="ec.factory.getTool('X')"/>`), and a
//! method can return another tool ([`crate::native::tool_value`]).

use rquickjs::class::{PropertyDescriptor, PropertyName};
use rquickjs::function::Rest;
use rquickjs::Atom;

use super::*;
use crate::native::ToolHandle;

#[derive(Trace, JsLifetime)]
#[rquickjs::class(exotic)]
pub(super) struct HostTool {
    #[qjs(skip_trace)]
    pub(super) inner: Arc<ToolHandle>,
}

/// `tool` as a script value.
pub(super) fn tool_to_js<'js>(ctx: &Ctx<'js>, tool: Arc<ToolHandle>) -> rquickjs::Result<JsValue<'js>> {
    Class::instance(ctx.clone(), HostTool { inner: tool }).and_then(|class| class.into_js(ctx))
}

fn get_tool<'js>(ctx: Ctx<'js>, name: String, parameters: Rest<JsValue<'js>>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let parameters = parameters
            .0
            .iter()
            .map(|value| js_to_host(&ctx, value))
            .collect::<rquickjs::Result<Vec<_>>>()?;
        let tool = with_ec(&ctx, |ec| ec.factory().get_tool(&name, &parameters))?
            .map_err(|error| Exception::throw_message(&ctx, error.message()))?;
        tool_to_js(&ctx, Arc::new(ToolHandle(tool)))
    })
}

/// `ec.factory.getToolFactory(name)`: `{ name, getInstance(...parameters) }`, or null when no tool factory has that name.
fn get_factory<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let tool = with_ec(&ctx, |ec| ec.factory().get_tool_factory(&name))?;
        let Some(tool) = tool else {
            return Ok(JsValue::new_null(ctx.clone()));
        };
        let object = Object::new(ctx.clone())?;
        object.set("name", tool.name())?;
        let get = Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<JsValue<'js>>| {
            get_tool(ctx, name.clone(), args)
        })?;
        object.set("getInstance", get)?;
        Ok(object.into_value())
    })
}

pub(super) fn install<'js>(ctx: &Ctx<'js>, ec: &Object<'js>, factory: &Object<'js>) -> Result<(), Error> {
    let get = Function::new(ctx.clone(), get_tool).map_err(js_error)?;
    ec.set("getTool", get.clone()).map_err(js_error)?;
    factory.set("getTool", get).map_err(js_error)?;
    factory
        .set(
            "getToolFactory",
            Function::new(ctx.clone(), get_factory).map_err(js_error)?,
        )
        .map_err(js_error)
}

impl HostTool {
    fn is_method(&self, name: &str) -> bool {
        self.inner.0.methods().contains(&name)
    }

    /// A function that calls `method` on this tool, in the calling script's execution context.
    fn method<'js>(&self, ctx: &Ctx<'js>, method: String) -> rquickjs::Result<JsValue<'js>> {
        let tool = Arc::clone(&self.inner);
        let function = Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, args: Rest<JsValue<'js>>| -> rquickjs::Result<JsValue<'js>> {
                host_guard(&ctx, || {
                    let args = args
                        .0
                        .iter()
                        .map(|value| js_to_host(&ctx, value))
                        .collect::<rquickjs::Result<Vec<_>>>()?;
                    let result = with_ec(&ctx, |ec| tool.0.call(ec, &method, &args))?
                        .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
                    host_to_js(&ctx, result)
                })
            },
        )?;
        function.into_js(ctx)
    }

    /// A method of the tool first, then the class prototype and `Object.prototype`, then the tool's own property.
    fn property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(ctx, || {
            let name = atom_name(&atom)?;
            if let Some(name) = name.as_deref().filter(|name| self.is_method(name)) {
                return self.method(ctx, name.to_string());
            }
            if let Some(found) = prototype_member::<HostTool>(ctx, &atom)? {
                return Ok(found);
            }
            let Some(name) = name else {
                return Undefined.into_js(ctx);
            };
            match with_ec(ctx, |ec| self.inner.0.property(ec, &name))? {
                Ok(Some(value)) => host_to_js(ctx, value),
                Ok(None) => Undefined.into_js(ctx),
                Err(err) => Err(Exception::throw_message(ctx, err.message())),
            }
        })
    }

    fn own_property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> rquickjs::Result<Option<PropertyDescriptor<'js>>> {
        match atom_name(&atom)? {
            Some(name) if self.is_method(&name) => Ok(Some(PropertyDescriptor::new_value(
                self.method(ctx, name.to_string())?,
                false,
                false,
                false,
            ))),
            _ => Ok(None),
        }
    }

    fn own_names<'js>(&self, ctx: &Ctx<'js>) -> rquickjs::Result<Vec<PropertyName<'js>>> {
        self.inner
            .0
            .methods()
            .iter()
            .map(|name| {
                Ok(PropertyName {
                    atom: Atom::from_str(ctx.clone(), name)?,
                    is_enumerable: false,
                })
            })
            .collect()
    }
}

/// The exotic methods `#[rquickjs::class(exotic)]` calls. A tool is read-only: a set or delete is refused, which a
/// strict-mode script sees as a TypeError.
#[allow(non_snake_case)]
mod __impl_exotic_HostTool__ {
    use super::*;
    use rquickjs::class::JsCell;

    pub(crate) struct ExoticImpl;

    impl ExoticImpl {
        pub fn exotic_get_property<'js>(
            this: &JsCell<'js, HostTool>,
            ctx: &Ctx<'js>,
            atom: Atom<'js>,
            _receiver: JsValue<'js>,
        ) -> rquickjs::Result<JsValue<'js>> {
            this.borrow().property(ctx, atom)
        }

        pub fn exotic_set_property<'js>(
            _this: &JsCell<'js, HostTool>,
            _ctx: &Ctx<'js>,
            _atom: Atom<'js>,
            _receiver: JsValue<'js>,
            value: JsValue<'js>,
        ) -> rquickjs::Result<bool> {
            keep_assigned(value);
            Ok(false)
        }

        pub fn exotic_delete_property<'js>(
            _this: &JsCell<'js, HostTool>,
            _ctx: &Ctx<'js>,
            _atom: Atom<'js>,
        ) -> rquickjs::Result<bool> {
            Ok(false)
        }

        pub fn exotic_has_property<'js>(
            this: &JsCell<'js, HostTool>,
            ctx: &Ctx<'js>,
            atom: Atom<'js>,
        ) -> rquickjs::Result<bool> {
            if atom_name(&atom)?.is_some_and(|name| this.borrow().is_method(&name)) {
                return Ok(true);
            }
            prototype_has::<HostTool>(ctx, &atom)
        }

        pub fn exotic_get_own_property<'js>(
            this: &JsCell<'js, HostTool>,
            ctx: &Ctx<'js>,
            atom: Atom<'js>,
        ) -> rquickjs::Result<Option<PropertyDescriptor<'js>>> {
            this.borrow().own_property(ctx, atom)
        }

        pub fn exotic_get_own_property_names<'js>(
            this: &JsCell<'js, HostTool>,
            ctx: &Ctx<'js>,
        ) -> rquickjs::Result<Vec<PropertyName<'js>>> {
            this.borrow().own_names(ctx)
        }
    }
}
