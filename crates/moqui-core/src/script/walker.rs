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

//! The macro walker shared by XML actions and screens.
//!
//! A macro module is a JavaScript function of the support object and of the modules loaded before
//! it. The walker visits an XML tree in post-order and calls `macros[name](node, children)` for
//! each element. `node` is `{name, attrs, text, children, parent, source}` and `children` is the
//! already generated text of the child elements. A macro that needs a particular child reads
//! `node.children[i].source`. The macros of an opaque element's descendants are not called, but
//! their node objects are built so the opaque element's macro can read them.

use std::collections::HashSet;

use super::runtime::js_error;

use moqui_lib::node::MNode;
use rquickjs::{Array, CatchResultExt, Ctx, Function, Object};

use crate::context::Error;

/// Replaces an element before it is walked, such as a form by its assembled form.
pub(crate) type Resolve<'a> = &'a dyn Fn(&MNode) -> Result<Option<MNode>, Error>;

/// How to walk one tree.
pub(crate) struct Walk<'a, 'js> {
    pub macros: &'a Object<'js>,
    /// Elements whose descendants get no macro call, such as an `entity-find` inside a form field.
    pub opaque: &'a HashSet<String>,
    pub resolve: Option<Resolve<'a>>,
    /// Names what is missing in an error: "XML action macro", "screen macro for text".
    pub missing: &'a str,
    /// The macro an element with none of its own gets, Java's FreeMarker `@element`: the element is logged and the
    /// rest renders (the failure-tolerance rule). With none, a screen's unknown element writes nothing. `None` keeps
    /// it an error, as for XML actions.
    pub fallback: Option<&'a str>,
}

/// Load a chain of macro modules. The first source evaluates to the support object. Each module
/// after it is a function called with the support object and the results before it.
///
/// # Errors
///
/// Returns an error when a source does not evaluate or a module throws.
pub(crate) fn load_chain<'js>(
    ctx: &Ctx<'js>,
    support: &str,
    modules: &[&str],
    debug: bool,
) -> Result<Vec<Object<'js>>, Error> {
    load_chain_for(ctx, support, modules, debug, "")
}

/// [`load_chain`] with the render mode the modules generate for, which a module that serves more than one
/// mode reads from `options.mode` when its macros run.
///
/// # Errors
///
/// Same as [`load_chain`].
pub(crate) fn load_chain_for<'js>(
    ctx: &Ctx<'js>,
    support: &str,
    modules: &[&str],
    debug: bool,
    mode: &str,
) -> Result<Vec<Object<'js>>, Error> {
    let support: Object = super::runtime::eval_source(ctx, support)?;
    let options: Object = support.get("options").map_err(js_error)?;
    options.set("debug", debug).map_err(js_error)?;
    options.set("mode", mode).map_err(js_error)?;
    let mut results = vec![support];
    let mut built = Vec::new();
    for module in modules {
        let function: Function = super::runtime::eval_source(ctx, module)?;
        let mut call = rquickjs::function::Args::new(ctx.clone(), results.len());
        for earlier in &results {
            call.push_arg(earlier.clone()).map_err(js_error)?;
        }
        let made: Object = call
            .apply(&function)
            .catch(ctx)
            .map_err(|caught| Error::new(caught.to_string()))?;
        results.push(made.clone());
        built.push(made);
    }
    Ok(built)
}

/// Visit `node` and return its generated text and its node object.
///
/// The whole tree of node objects is built first, so a macro can read any ancestor or sibling element.
/// Then the macros run in post-order, and each node gets its `source` as its macro returns.
///
/// # Errors
///
/// Returns an error for an element with no macro, or when a macro throws.
pub(crate) fn walk<'js>(
    ctx: &Ctx<'js>,
    walk: &Walk<'_, 'js>,
    node: &MNode,
    parent: Option<&Object<'js>>,
) -> Result<(String, Object<'js>), Error> {
    let built = build(ctx, walk, node, parent, false)?;
    let source = run(ctx, walk, &built)?;
    Ok((source, built))
}

fn build<'js>(
    ctx: &Ctx<'js>,
    walk: &Walk<'_, 'js>,
    node: &MNode,
    parent: Option<&Object<'js>>,
    inert: bool,
) -> Result<Object<'js>, Error> {
    let replaced = match walk.resolve {
        Some(resolve) if !inert => resolve(node)?,
        _ => None,
    };
    let node = replaced.as_ref().unwrap_or(node);
    let name = node.name();
    let node_obj = Object::new(ctx.clone()).map_err(js_error)?;
    node_obj.set("name", name.as_str()).map_err(js_error)?;
    node_obj
        .set("text", node.text().unwrap_or_default())
        .map_err(js_error)?;
    node_obj.set("attrs", attrs_object(ctx, node)?).map_err(js_error)?;
    node_obj.set("inert", inert).map_err(js_error)?;
    if let Some(parent) = parent {
        node_obj.set("parent", parent.clone()).map_err(js_error)?;
    }
    let children = Array::new(ctx.clone()).map_err(js_error)?;
    node_obj.set("children", children.clone()).map_err(js_error)?;
    let inert_children = inert || walk.opaque.contains(&name);
    for (index, child) in node.children().iter().enumerate() {
        let child_obj = build(ctx, walk, child, Some(&node_obj), inert_children)?;
        children.set(index, child_obj).map_err(js_error)?;
    }
    Ok(node_obj)
}

fn run<'js>(ctx: &Ctx<'js>, walk: &Walk<'_, 'js>, node_obj: &Object<'js>) -> Result<String, Error> {
    let inert: bool = node_obj.get("inert").map_err(js_error)?;
    if inert {
        node_obj.set("source", "").map_err(js_error)?;
        return Ok(String::new());
    }
    let children: Array = node_obj.get("children").map_err(js_error)?;
    let mut combined = String::new();
    for child in children.iter::<Object>() {
        combined.push_str(&run(ctx, walk, &child.map_err(js_error)?)?);
    }
    let name: String = node_obj.get("name").map_err(js_error)?;
    let mut macro_name = name.as_str();
    if !walk.macros.contains_key(macro_name).map_err(js_error)? {
        let Some(fallback) = walk.fallback else {
            return Err(Error::new(format!("no {} for {name}", walk.missing)));
        };
        tracing::error!("No {} for {name}; it renders as an unknown element", walk.missing);
        if !walk.macros.contains_key(fallback).map_err(js_error)? {
            node_obj.set("source", "").map_err(js_error)?;
            return Ok(String::new());
        }
        // Java's @element does not recurse, so the children are not rendered either.
        combined.clear();
        macro_name = fallback;
    }
    let function: Function = walk.macros.get(macro_name).map_err(js_error)?;
    let source: String = function
        .call((node_obj.clone(), combined))
        .catch(ctx)
        .map_err(|caught| Error::new(caught.to_string()))?;
    node_obj.set("source", source.clone()).map_err(js_error)?;
    Ok(source)
}

fn attrs_object<'js>(ctx: &Ctx<'js>, node: &MNode) -> Result<Object<'js>, Error> {
    let attrs = Object::new(ctx.clone()).map_err(js_error)?;
    for (key, value) in node.attributes() {
        attrs.set(key, value).map_err(js_error)?;
    }
    Ok(attrs)
}

/// The elements in a JavaScript array of names.
///
/// # Errors
///
/// Returns an error when the value is not an array of strings.
pub(crate) fn names_of(array: &Array<'_>) -> Result<HashSet<String>, Error> {
    let mut names = HashSet::new();
    for item in array.iter::<String>() {
        names.insert(item.map_err(js_error)?);
    }
    Ok(names)
}
