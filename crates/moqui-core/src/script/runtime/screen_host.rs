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

//! `sri` in scripts, and the `ec` and static helpers the screen macros call. The logic is in
//! `screen::sri` and `l10n`; this file converts arguments and results.

use rquickjs::{Array, Constructor};

use moqui_lib::node::MNode;

use super::*;
use crate::screen::generate::{self, Unit, UnitRequest};
use crate::screen::sri::{self as logic, NodeLit, Runner};
use crate::screen::{with_current_render, RenderState};

/// Run `body` on the script runtime with a [`Runner`] for the render in progress.
pub(crate) fn with_screen_runner<R>(body: impl FnOnce(&dyn Runner) -> Result<R, Error>) -> Result<R, Error> {
    with_context(|ctx| body(&CtxRunner { ctx }))
}

struct CtxRunner<'a, 'js> {
    ctx: &'a Ctx<'js>,
}

impl Runner for CtxRunner<'_, '_> {
    fn generate(&self, request: &UnitRequest<'_>) -> Result<Option<Arc<Unit>>, Error> {
        generate::unit_for(self.ctx, request)
    }

    fn generate_dynamic(
        &self,
        request: &UnitRequest<'_>,
        form: &crate::screen::form::FormInstance,
    ) -> Result<Arc<Unit>, Error> {
        generate::unit_for_dynamic_form(self.ctx, request, form)
    }

    fn run(&self, unit: &Unit) -> Result<(), Error> {
        run_unit(self.ctx, unit)
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
    let mark = StackMark::new();
    let result = in_unit_scope(ctx, || {
        function
            .call::<_, ()>(())
            .catch(ctx)
            .map_err(|caught| Error::of_kind(caught_kind(&caught), caught.to_string()))
    });
    if result.is_ok() {
        mark.keep();
    }
    result
}

/// Throw an error to the script, carrying a security kind on the error object.
fn throw(ctx: &Ctx<'_>, err: &Error) -> JsError {
    let exception = match Exception::from_message(ctx.clone(), err.message()) {
        Ok(exception) => exception,
        Err(thrown) => return thrown,
    };
    match mark_kind(&exception, err.kind()) {
        Ok(()) => exception.throw(),
        Err(thrown) => thrown,
    }
}

/// Run `body` with the render in progress, flushing script-held lists first so the stack is current.
pub(super) fn with_render<T>(
    ctx: &Ctx<'_>,
    clear: bool,
    body: impl FnOnce(&RenderState) -> Result<T, Error>,
) -> rquickjs::Result<T> {
    flush_current(ctx, clear)?;
    match with_current_render(body) {
        Some(Ok(value)) => Ok(value),
        Some(Err(err)) => Err(throw(ctx, &err)),
        None => Err(Exception::throw_message(ctx, "no screen render is running")),
    }
}

// ---- argument conversion ----

pub(super) fn lit_of(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<NodeLit> {
    lit(ctx, value)
}

fn lit(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<NodeLit> {
    lit_in(ctx, value, &mut Vec::new())
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

fn text_arg(value: &Opt<String>) -> &str {
    value.0.as_deref().unwrap_or("")
}

// ---- sri functions ----

fn sri_write(ctx: Ctx<'_>, text: JsValue<'_>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let text = match text.as_string() {
            Some(text) => text.to_string()?,
            None => to_plain_string_js(&ctx, &text)?,
        };
        with_current_render(|state| logic::write(state, &text))
            .ok_or_else(|| Exception::throw_message(&ctx, "no screen render is running"))
    })
}

fn to_plain_string_js(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<String> {
    Ok(moqui_lib::object::to_plain_string(&js_to_host(ctx, value)?))
}

fn sri_render_section(ctx: Ctx<'_>, name: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let runner = CtxRunner { ctx: &ctx };
        with_render(&ctx, true, |state| logic::render_section(state, &runner, &name))
    })
    .map(|()| String::new())
}

fn sri_render_section_include(ctx: Ctx<'_>, node: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let node = lit(&ctx, &node)?;
        let runner = CtxRunner { ctx: &ctx };
        with_render(&ctx, true, |state| logic::render_section_include(state, &runner, &node))
    })
    .map(|()| String::new())
}

fn sri_render_subscreen(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let runner = CtxRunner { ctx: &ctx };
        with_render(&ctx, true, |state| logic::render_subscreen(state, &runner))
    })
    .map(|()| String::new())
}

fn sri_render_dynamic_form(ctx: Ctx<'_>, name: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let runner = CtxRunner { ctx: &ctx };
        with_render(&ctx, true, |state| logic::render_dynamic_form(state, &runner, &name))
    })
    .map(|()| String::new())
}

fn sri_render_user_columns_form(ctx: Ctx<'_>, name: String) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let runner = CtxRunner { ctx: &ctx };
        with_render(&ctx, true, |state| {
            logic::render_user_columns_form(state, &runner, &name)
        })
    })
}

fn sri_render_include_screen(ctx: Ctx<'_>, location: String, share_scope: Opt<String>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let runner = CtxRunner { ctx: &ctx };
        let share = text_arg(&share_scope) == "true";
        with_render(&ctx, true, |state| {
            logic::render_include_screen(state, &runner, &location, share)
        })
    })
    .map(|()| String::new())
}

fn sri_render_text(ctx: Ctx<'_>, location: String, template: Opt<String>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        with_render(&ctx, true, |state| {
            logic::render_text(state, &location, text_arg(&template))
        })
    })
}

/// `sri.renderInlineText(body)`: a `<text template="true">` body as a trusted fancy template.
fn sri_render_inline_text(ctx: Ctx<'_>, body: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        crate::script::template::render_trusted_here(&ctx, &body, crate::script::template::Escape::None)
            .map_err(|err| Exception::throw_message(&ctx, err.message()))
    })
}

fn sri_do_boundary_comments(ctx: Ctx<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| Ok(logic::do_boundary_comments(state)))
    })
}

fn sri_get_render_mode(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || with_render(&ctx, false, |state| Ok(state.render_mode.clone())))
}

fn sri_encode_html(ctx: Ctx<'_>, text: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || Ok(logic::encode_html(&text)))
}

fn screen_def_object<'js>(ctx: &Ctx<'js>, def: &crate::screen::ScreenDef) -> rquickjs::Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("location", def.location.as_str())?;
    object.set("screenName", def.screen_name.as_str())?;
    object.set("getScreenName", text_getter(ctx, def.screen_name.clone())?)?;
    object.set("getLocation", text_getter(ctx, def.location.clone())?)?;
    object.set("getDefaultMenuName", text_getter(ctx, def.default_menu_title.clone())?)?;
    Ok(object)
}

fn sri_get_active_screen_def<'js>(ctx: Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let def = with_render(&ctx, false, |state| Ok(logic::active_def(state)))?;
        screen_def_object(&ctx, &def)
    })
}

/// `sri.getScreenUrlInfo()`: the screen the request ended at and the one above it, for the page title.
fn sri_get_screen_url_info<'js>(ctx: Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let (target, parent) = with_render(&ctx, false, |state| {
            let defs = &state.url_info.path_defs;
            let parent = defs.len().checked_sub(2).and_then(|index| defs.get(index)).cloned();
            Ok((Arc::clone(&state.url_info.target_screen), parent))
        })?;
        let extra = with_render(&ctx, false, |state| Ok(state.url_info.extra_path.clone()))?;
        let object = Object::new(ctx.clone())?;
        object.set("extraPathNameList", extra)?;
        object.set("targetScreen", screen_def_object(&ctx, &target)?)?;
        if let Some(parent) = parent {
            object.set("parentScreen", screen_def_object(&ctx, &parent)?)?;
        }
        Ok(object)
    })
}

fn sri_push_context(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        push_context_frame();
        Ok(())
    })
    .map(|()| String::new())
}

fn sri_pop_context(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        pop_context_frame();
        Ok(())
    })
    .map(|()| String::new())
}

fn sri_set_in_context(ctx: Ctx<'_>, node: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let node = lit(&ctx, &node)?;
        with_render(&ctx, true, |_| logic::set_in_context(&node))
    })
    .map(|()| String::new())
}

/// The map of a form-single: the value itself, or an expression string as `form-single.@map` is.
fn form_map(ctx: &Ctx<'_>, map: &JsValue<'_>) -> rquickjs::Result<Value> {
    if let Some(expression) = map.as_string() {
        let expression = expression.to_string()?;
        if expression.is_empty() {
            return Ok(Value::Null);
        }
        return eval_value(&expression).map_err(|err| Exception::throw_message(ctx, err.message()));
    }
    js_to_host(ctx, map)
}

fn sri_push_single_form_map_context(ctx: Ctx<'_>, map: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        let host = form_map(&ctx, &map)?;
        logic::push_single_form_map_context(&host);
        Ok(())
    })
    .map(|()| String::new())
}

fn sri_get_single_form_map<'js>(ctx: Ctx<'js>, map: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        flush_current(&ctx, false)?;
        let host = form_map(&ctx, &map)?;
        let fields = value_map(&host).unwrap_or_default();
        collection_to_js(&ctx, Value::Map(fields))
    })
}

fn sri_get_field_value_string(
    ctx: Ctx<'_>,
    node: JsValue<'_>,
    default_value: Opt<String>,
    format: Opt<String>,
    kind: Opt<String>,
) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let node = lit(&ctx, &node)?;
        with_render(&ctx, false, |state| {
            if default_value.0.is_some() || format.0.is_some() {
                logic::get_field_value_string_for(&node, text_arg(&default_value), text_arg(&format), text_arg(&kind))
            } else {
                logic::get_field_value_string(state, &node)
            }
        })
    })
}

fn sri_get_field_value<'js>(
    ctx: Ctx<'js>,
    field: JsValue<'js>,
    default_value: Opt<String>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let field = lit(&ctx, &field)?;
        let value = with_render(&ctx, false, |_| {
            logic::get_field_value(&field, text_arg(&default_value))
        })?;
        host_to_js(&ctx, value)
    })
}

fn sri_get_field_value_plain_string(
    ctx: Ctx<'_>,
    field: JsValue<'_>,
    default_value: Opt<String>,
) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let field = lit(&ctx, &field)?;
        with_render(&ctx, false, |_| {
            logic::get_field_value_plain_string(&field, text_arg(&default_value))
        })
    })
}

fn sri_get_field_value_class(ctx: Ctx<'_>, field: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let field = lit(&ctx, &field)?;
        with_render(&ctx, false, |_| logic::get_field_value_class(&field))
    })
}

fn sri_get_named_value_plain(ctx: Ctx<'_>, name: String, form: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let form = lit(&ctx, &form)?;
        with_render(&ctx, false, |_| logic::get_named_value_plain(&name, &form))
    })
}

fn sri_get_field_entity_value(ctx: Ctx<'_>, widget: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let widget = lit(&ctx, &widget)?;
        with_render(&ctx, false, |_| logic::get_field_entity_value(&widget))
    })
}

/// A `Map` from key to text, so the order of the options is kept whatever the keys look like.
fn sri_get_field_options<'js>(ctx: Ctx<'js>, widget: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let widget = lit(&ctx, &widget)?;
        let options = with_render(&ctx, true, |state| logic::get_field_options(state, &widget))?;
        let pairs = Array::new(ctx.clone())?;
        for (index, (key, text)) in options.into_iter().enumerate() {
            let pair = Array::new(ctx.clone())?;
            pair.set(0, key)?;
            pair.set(1, text)?;
            pairs.set(index, pair)?;
        }
        let constructor: Constructor = ctx.globals().get("Map")?;
        constructor.construct((pairs,))
    })
}

fn sri_open_form_list<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let (id, count) = with_render(&ctx, true, |state| logic::open_form_list(state, &name))?;
        let info = Object::new(ctx.clone())?;
        info.set("id", i32::try_from(id).unwrap_or(i32::MAX))?;
        info.set("count", i32::try_from(count).unwrap_or(i32::MAX))?;
        info.set("name", name)?;
        Ok(info)
    })
}

fn list_id(ctx: &Ctx<'_>, info: &JsValue<'_>) -> rquickjs::Result<usize> {
    let id: i32 = info
        .as_object()
        .ok_or_else(|| Exception::throw_message(ctx, "expected a form-list handle"))?
        .get("id")?;
    usize::try_from(id).map_err(|_| Exception::throw_message(ctx, "bad form-list handle"))
}

fn sri_start_form_list_row(ctx: Ctx<'_>, info: JsValue<'_>, index: i32) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let id = list_id(&ctx, &info)?;
        let index = usize::try_from(index).map_err(|_| Exception::throw_message(&ctx, "bad row index"))?;
        with_render(&ctx, true, |state| logic::start_form_list_row(state, id, index))
    })
    .map(|()| String::new())
}

fn sri_form_list_sub_row_count(ctx: Ctx<'_>, info: JsValue<'_>, index: i32) -> rquickjs::Result<i32> {
    host_guard(&ctx, || {
        let id = list_id(&ctx, &info)?;
        let index = usize::try_from(index).map_err(|_| Exception::throw_message(&ctx, "bad row index"))?;
        let count = with_render(&ctx, false, |state| {
            Ok(logic::form_list_sub_row_count(state, id, index))
        })?;
        Ok(i32::try_from(count).unwrap_or(i32::MAX))
    })
}

fn sri_start_form_list_sub_row(ctx: Ctx<'_>, info: JsValue<'_>, index: i32, sub: i32) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let id = list_id(&ctx, &info)?;
        let index = usize::try_from(index).map_err(|_| Exception::throw_message(&ctx, "bad row index"))?;
        let sub = usize::try_from(sub).map_err(|_| Exception::throw_message(&ctx, "bad sub-row index"))?;
        with_render(&ctx, true, |state| {
            logic::start_form_list_sub_row(state, id, index, sub)
        })
    })
    .map(|()| String::new())
}

fn sri_end_form_list_row(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    sri_pop_context(ctx)
}

fn sri_safe_close_list(ctx: Ctx<'_>, info: JsValue<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        if let Ok(id) = list_id(&ctx, &info) {
            with_render(&ctx, false, |state| {
                logic::close_form_list(state, id);
                Ok(())
            })?;
        }
        Ok(())
    })
    .map(|()| String::new())
}

fn sri_form_list_column_char_widths<'js>(
    ctx: Ctx<'js>,
    columns: Array<'js>,
    line_width: i32,
) -> rquickjs::Result<Vec<i32>> {
    host_guard(&ctx, || {
        let mut specs = Vec::new();
        for column in columns.iter::<Array>() {
            let mut fields = Vec::new();
            for field in column?.iter::<Array>() {
                let field = field?;
                let width: Option<String> = field.get(0).ok();
                let kind: Option<String> = field.get(1).ok();
                fields.push((width.unwrap_or_default(), kind.unwrap_or_default()));
            }
            specs.push(fields);
        }
        let widths =
            logic::form_list_column_char_widths(&specs, i64::from(line_width)).map_err(|err| throw(&ctx, &err))?;
        Ok(widths
            .into_iter()
            .map(|width| i32::try_from(width).unwrap_or(i32::MAX))
            .collect())
    })
}

fn node_object<'js>(ctx: &Ctx<'js>, node: &MNode) -> rquickjs::Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("name", node.name())?;
    object.set("text", node.text().unwrap_or_default())?;
    let attrs = Object::new(ctx.clone())?;
    for (key, value) in node.attributes() {
        attrs.set(key, value)?;
    }
    object.set("attrs", attrs)?;
    let children = Array::new(ctx.clone())?;
    for (index, child) in node.children().iter().enumerate() {
        children.set(index, node_object(ctx, child)?)?;
    }
    object.set("children", children)?;
    Ok(object)
}

fn sri_get_form_node<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let form = with_render(&ctx, false, |state| logic::get_form(state, &name))?;
        let node = form.node().map_err(|err| throw(&ctx, &err))?;
        node_object(&ctx, &node)
    })
}

fn sri_get_form_instance<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let form = with_render(&ctx, false, |state| logic::get_form(state, &name))?;
        let node = form.node().map_err(|err| throw(&ctx, &err))?;
        let instance = Object::new(ctx.clone())?;
        instance.set("formNode", node_object(&ctx, &node)?)?;
        instance.set("name", form.name.as_str())?;
        instance.set("list", form.list)?;
        Ok(instance)
    })
}

/// `sri.getUserFormListFinds(formName)`: the saved finds of the user and the user's groups for a form of the screen.
fn sri_get_user_form_list_finds<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let finds = with_render(&ctx, false, |state| logic::user_form_list_finds(state, &name))?;
        host_to_js(&ctx, Value::List(finds))
    })
}

/// `sri.getUserDefaultFormListFindId()`: the user's default saved find of the screen, or null.
fn sri_get_user_default_form_list_find_id<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let id = with_render(&ctx, false, logic::user_default_form_list_find_id)?;
        host_to_js(&ctx, id.map_or(Value::Null, Value::String))
    })
}

fn sri_get_form_hidden_parameters<'js>(ctx: Ctx<'js>, form: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let form = lit(&ctx, &form)?;
        let hidden = form.child("hidden-parameters").cloned();
        let parameters = match hidden {
            Some(hidden) => with_render(&ctx, false, |_| logic::node_parameters(&hidden))?,
            None => indexmap::IndexMap::new(),
        };
        let mut out = indexmap::IndexMap::new();
        for (key, value) in parameters {
            let text = moqui_lib::object::to_plain_string(&value);
            if !text.is_empty() {
                out.insert(key, Value::String(text));
            }
        }
        collection_to_js(&ctx, Value::Map(out))
    })
}

fn sri_append_to_after_screen_writer(ctx: Ctx<'_>, text: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            logic::append_to_after_screen_writer(state, &text);
            Ok(())
        })
    })
    .map(|()| String::new())
}

fn sri_append_to_script_writer(ctx: Ctx<'_>, text: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            logic::append_to_script_writer(state, &text);
            Ok(())
        })
    })
    .map(|()| String::new())
}

fn sri_get_theme_icon_class<'js>(ctx: Ctx<'js>, text: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let found = with_render(&ctx, false, |state| logic::get_theme_icon_class(state, &text))?;
        match found {
            Some(class) => class.into_js(&ctx),
            None => Null.into_js(&ctx),
        }
    })
}

fn sri_get_theme_values(ctx: Ctx<'_>, resource_type: String, theme_id: Opt<String>) -> rquickjs::Result<Vec<String>> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            logic::get_theme_values(state, &resource_type, text_arg(&theme_id))
        })
    })
}

fn sri_get_field_validation<'js>(ctx: Ctx<'js>, sub: JsValue<'js>) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let sub = lit(&ctx, &sub)?;
        let found = with_render(&ctx, false, |_| logic::field_validation(&sub))?;
        let object = Object::new(ctx.clone())?;
        object.set("classes", found.classes.join(" "))?;
        if let Some((regexp, message)) = found.regexp {
            object.set("regexp", regexp)?;
            object.set("message", message)?;
        }
        Ok(object)
    })
}

/// `getFieldValidationJsRules(sub)`: `[{expr, message}]`, the browser checks of a field's validation.
fn sri_get_field_validation_js_rules<'js>(ctx: Ctx<'js>, sub: JsValue<'js>) -> rquickjs::Result<Array<'js>> {
    host_guard(&ctx, || {
        let sub = lit(&ctx, &sub)?;
        let rules = with_render(&ctx, false, |_| logic::field_validation_js_rules(&sub))?;
        let out = Array::new(ctx.clone())?;
        for (index, (expr, message)) in rules.into_iter().enumerate() {
            let object = Object::new(ctx.clone())?;
            object.set("expr", expr)?;
            object.set("message", message)?;
            out.set(index, object)?;
        }
        Ok(out)
    })
}

/// `getFieldTransitionValue(transition, node, term, labelField, alwaysGet)`: the label a transition gives for a value.
fn sri_get_field_transition_value<'js>(
    ctx: Ctx<'js>,
    transition: String,
    node: JsValue<'js>,
    term: Opt<JsValue<'js>>,
    label_field: Opt<String>,
    always_get: Opt<bool>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let node = lit(&ctx, &node)?;
        let term = match term.0 {
            Some(value) if !value.is_null() && !value.is_undefined() => {
                moqui_lib::object::to_plain_string(&js_to_host(&ctx, &value)?)
            }
            _ => String::new(),
        };
        let label_field = label_field.0.unwrap_or_default();
        let found = with_render(&ctx, false, |state| {
            logic::field_transition_value(
                state,
                &transition,
                &node,
                &term,
                &label_field,
                always_get.0.unwrap_or(false),
            )
        })?;
        host_to_js(&ctx, found.map_or(Value::Null, Value::String))
    })
}

fn sri_get_menu_items<'js>(ctx: Ctx<'js>, screen_path: Opt<String>) -> rquickjs::Result<Array<'js>> {
    host_guard(&ctx, || {
        let items = with_render(&ctx, false, |state| logic::menu_items(state, screen_path.0.as_deref()))?;
        let out = Array::new(ctx.clone())?;
        for (index, item) in items.into_iter().enumerate() {
            let object = Object::new(ctx.clone())?;
            object.set("name", item.name)?;
            object.set("hasParameters", item.has_parameters)?;
            object.set("title", item.title)?;
            object.set("url", item.url)?;
            object.set("disabled", item.disabled)?;
            object.set("inPath", item.in_path)?;
            object.set("image", item.image)?;
            object.set("imageType", item.image_type)?;
            out.set(index, object)?;
        }
        Ok(out)
    })
}

/// `sri.getMenuData(pathNameList)`: the menu data of the `menuData` transition, or null after an error was sent.
fn sri_get_menu_data<'js>(ctx: Ctx<'js>, names: Vec<String>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let found = with_render(&ctx, false, |state| crate::screen::menu::menu_data(state, &names))?;
        match found {
            Some(value) => collection_to_js(&ctx, value),
            None => Null.into_js(&ctx),
        }
    })
}

fn sri_get_after_screen_writer_text(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            Ok(std::mem::take(&mut *state.after_text.borrow_mut()))
        })
    })
}

/// `sri.once(key)`: true the first time a render sees the key.
fn sri_once(ctx: Ctx<'_>, key: String) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| Ok(state.once.borrow_mut().insert(key.clone())))
    })
}

fn sri_get_script_writer_text(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            Ok(std::mem::take(&mut *state.script_text.borrow_mut()))
        })
    })
}

fn sri_get_section_included_node<'js>(ctx: Ctx<'js>, node: JsValue<'js>) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let node = lit(&ctx, &node)?;
        let object = Object::new(ctx.clone())?;
        object.set("name", "section")?;
        let attrs = Object::new(ctx.clone())?;
        let name = if node.attr("name").is_empty() {
            node.attr("location").split_once('#').map_or("", |(_, name)| name)
        } else {
            node.attr("name")
        };
        attrs.set("name", name)?;
        object.set("attrs", attrs)?;
        Ok(object)
    })
}

/// `sri.staticUrl(text, urlType)`: where a link of a static template goes, with no parameters.
fn sri_static_url<'js>(ctx: Ctx<'js>, text: String, url_type: Opt<String>) -> rquickjs::Result<Object<'js>> {
    host_guard(&ctx, || {
        let found = with_render(&ctx, false, |state| {
            crate::screen::urls::static_url(state, &text, url_type.0.as_deref().unwrap_or("transition"))
        })?;
        let object = Object::new(ctx.clone())?;
        object.set("path", found.path)?;
        object.set("screenPath", found.screen_path)?;
        object.set("exists", found.exists)?;
        object.set("isTransition", found.is_transition)?;
        object.set("isScreen", found.is_screen)?;
        object.set("readOnly", found.read_only)?;
        object.set("method", found.method)?;
        object.set("screenUrl", found.screen_url)?;
        Ok(object)
    })
}

/// `sri.staticCheck(json)`: the expressions a static macro wrote, as `[{expr, element, attr}]`. Only a compatibility report looks at them.
fn sri_static_check(ctx: Ctx<'_>, json: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(&json) {
            for item in items {
                let text = |key: &str| {
                    item.get(key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                crate::screen::statics::record_ast(&text("expr"), &text("element"), &text("attr"));
            }
        }
        Ok(())
    })
}

/// `sri.staticNote(kind, element, detail)`: server-only work a static macro could not write into the template.
fn sri_static_note(ctx: Ctx<'_>, kind: String, element: String, detail: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        crate::screen::statics::record_note(&kind, &element, &detail);
        Ok(())
    })
}

// ---- xlsx (the moqui-poi port): ScreenXlsx.js fills the render's workbook ----

fn sri_xlsx_sheet(ctx: Ctx<'_>, name: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            state.xlsx.borrow_mut().start_sheet(&name);
            Ok(())
        })
    })
}

fn sri_xlsx_header(ctx: Ctx<'_>, title: JsValue<'_>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let title = to_plain_string_js(&ctx, &title)?;
        with_render(&ctx, false, |state| {
            state.xlsx.borrow_mut().header(&title);
            Ok(())
        })
    })
}

/// `sri.xlsxCell(values, formats, currency, alignRight)`: one cell from the values of a field's widgets.
fn sri_xlsx_cell<'js>(
    ctx: Ctx<'js>,
    values: Array<'js>,
    formats: Array<'js>,
    currency: Opt<bool>,
    align_right: Opt<bool>,
) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let mut cells = Vec::with_capacity(values.len());
        for (index, value) in values.iter::<JsValue>().enumerate() {
            let value = js_to_host(&ctx, &value?)?;
            let format = formats.get::<Option<String>>(index)?.unwrap_or_default();
            cells.push((value, format));
        }
        let hints = crate::screen::xlsx::CellHints {
            currency: currency.0.unwrap_or(false),
            align_right: align_right.0.unwrap_or(false),
        };
        with_render(&ctx, false, |state| state.xlsx.borrow_mut().cell(&cells, hints))
    })
}

fn sri_xlsx_cell_count(ctx: Ctx<'_>) -> rquickjs::Result<i32> {
    host_guard(&ctx, || {
        let count = with_render(&ctx, false, |state| Ok(state.xlsx.borrow().cell_count()))?;
        Ok(i32::try_from(count).unwrap_or(i32::MAX))
    })
}

fn sri_xlsx_end_row(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        with_render(&ctx, false, |state| {
            state.xlsx.borrow_mut().end_row();
            Ok(())
        })
    })
}

/// Build the global `sri`.
pub(super) fn sri_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let sri = caught(ctx, "sri", Object::new(ctx.clone()))?;
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, sri.set($name, function))?;
        };
    }
    method!("sendRedirectAndStopRender", sri_send_redirect_and_stop_render);
    method!("write", sri_write);
    method!("renderSection", sri_render_section);
    method!("renderSectionInclude", sri_render_section_include);
    method!("renderSubscreen", sri_render_subscreen);
    method!("renderIncludeScreen", sri_render_include_screen);
    method!("renderDynamicForm", sri_render_dynamic_form);
    method!("renderUserColumnsForm", sri_render_user_columns_form);
    method!("renderText", sri_render_text);
    method!("renderInlineText", sri_render_inline_text);
    method!("doBoundaryComments", sri_do_boundary_comments);
    method!("getRenderMode", sri_get_render_mode);
    method!("encodeHtml", sri_encode_html);
    method!("getActiveScreenDef", sri_get_active_screen_def);
    method!("getScreenUrlInfo", sri_get_screen_url_info);
    method!("pushContext", sri_push_context);
    method!("popContext", sri_pop_context);
    method!("setInContext", sri_set_in_context);
    method!("pushSingleFormMapContext", sri_push_single_form_map_context);
    method!("getSingleFormMap", sri_get_single_form_map);
    method!("getFieldValueString", sri_get_field_value_string);
    method!("getFieldValue", sri_get_field_value);
    method!("getFieldValuePlainString", sri_get_field_value_plain_string);
    method!("getFieldValueClass", sri_get_field_value_class);
    method!("getNamedValuePlain", sri_get_named_value_plain);
    method!("getFieldEntityValue", sri_get_field_entity_value);
    method!("getFieldOptions", sri_get_field_options);
    method!("openFormList", sri_open_form_list);
    method!("startFormListRow", sri_start_form_list_row);
    method!("endFormListRow", sri_end_form_list_row);
    method!("formListSubRowCount", sri_form_list_sub_row_count);
    method!("startFormListSubRow", sri_start_form_list_sub_row);
    method!("endFormListSubRow", sri_end_form_list_row);
    method!("safeCloseList", sri_safe_close_list);
    method!("formListColumnCharWidths", sri_form_list_column_char_widths);
    method!("getFormNode", sri_get_form_node);
    method!("getFormInstance", sri_get_form_instance);
    method!("getFormHiddenParameters", sri_get_form_hidden_parameters);
    method!("getUserFormListFinds", sri_get_user_form_list_finds);
    method!("getUserDefaultFormListFindId", sri_get_user_default_form_list_find_id);
    method!("appendToAfterScreenWriter", sri_append_to_after_screen_writer);
    method!("appendToScriptWriter", sri_append_to_script_writer);
    method!("once", sri_once);
    method!("getAfterScreenWriterText", sri_get_after_screen_writer_text);
    method!("getScriptWriterText", sri_get_script_writer_text);
    method!("getThemeIconClass", sri_get_theme_icon_class);
    method!("getThemeValues", sri_get_theme_values);
    method!("getSectionIncludedNode", sri_get_section_included_node);
    method!("getFieldValidation", sri_get_field_validation);
    method!("getFieldValidationJsRules", sri_get_field_validation_js_rules);
    method!("getFieldTransitionValue", sri_get_field_transition_value);
    method!("getMenuItems", sri_get_menu_items);
    method!("getMenuData", sri_get_menu_data);
    method!("staticUrl", sri_static_url);
    method!("staticCheck", sri_static_check);
    method!("staticNote", sri_static_note);
    method!("xlsxSheet", sri_xlsx_sheet);
    method!("xlsxHeader", sri_xlsx_header);
    method!("xlsxCell", sri_xlsx_cell);
    method!("xlsxCellCount", sri_xlsx_cell_count);
    method!("xlsxEndRow", sri_xlsx_end_row);
    super::urls::install(ctx, &sri)?;
    Ok(sri)
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
}
