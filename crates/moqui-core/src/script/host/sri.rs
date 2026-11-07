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

//! The global `sri`, the screen render the generated screen units and the macros call: a [`Shim`] that puts the hidden
//! functions here and in `urls.rs` on `sri`, in the order the QuickJS object had them. The logic is in `screen::sri`;
//! this file converts arguments and results.
//!
//! What a host value cannot carry is done in the shim: the `Map` of `getFieldOptions`, the functions of a screen
//! definition, the properties a QuickJS object had as `undefined`, and `holdNodeLits`, which attaches a [`NODE_LIT`]
//! object to each element literal of a compiled unit (as `$nodeLit`, read back by each binder's element conversion)
//! and freezes it.
//!
//! A method that renders (`renderSection`, `renderInlineText`, ...) runs units and templates through
//! [`EngineOps::screen_runner`], in the engine that called it.

use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::{HostObject, Value};

use super::{method, urls, Call, HostClass, HostError, Method, Ret, Shim};
use crate::screen::sri::{self as logic, NodeLit};
use crate::screen::{with_current_render, RenderState};

const SRI_JS: &str = r#"(function (global, hidden) {
    "use strict";
    const sri = global.sri || (global.sri = {});
    const freeze = Object.freeze;
    const defineProperty = Object.defineProperty;
    const isArray = Array.isArray;
    const isObject = (value) => value !== null && (typeof value === "object" || typeof value === "function");
    const screenDef = (def) => {
        const object = { location: def.location, screenName: def.screenName };
        object.getScreenName = () => def.screenName;
        object.getLocation = () => def.location;
        object.getDefaultMenuName = () => def.defaultMenuName;
        return object;
    };
    // a property the QuickJS object had as undefined (an Option that was None) comes as null
    const undefinedIfNull = (object, names) => {
        for (const name of names) if (object[name] === null) object[name] = undefined;
    };
    // attach the conversion of each element object (its parent and children too) and freeze it
    const hold = (value, held) => {
        if (!isObject(value)) return;
        const parent = hidden.nodeLitParent(held);
        if (parent !== null) hold(value.parent, parent);
        const children = value.children;
        if (isArray(children)) {
            for (let index = 0; index < children.length; index++) {
                const child = hidden.nodeLitChild(held, index);
                if (child === null) break;
                hold(children[index], child);
            }
        }
        defineProperty(value, "$nodeLit", { value: held });
    };
    const deepFreeze = (value) => {
        if (!isObject(value)) return;
        for (const name of ["attrs", "children", "parent"]) {
            const inner = value[name];
            if (isArray(inner)) {
                for (let index = 0; index < inner.length; index++) deepFreeze(inner[index]);
                freeze(inner);
            } else if (isObject(inner)) {
                deepFreeze(inner);
            }
        }
        freeze(value);
    };
    const wrapped = {
        holdNodeLits: function holdNodeLits(literals) {
            if (!isArray(literals)) {
                throw new TypeError("Error converting from js '" + (literals === null ? "null" : typeof literals) +
                    "' into type 'array'");
            }
            for (let index = 0; index < literals.length; index++) {
                const literal = literals[index];
                hold(literal, hidden.nodeLit(literal));
                deepFreeze(literal);
            }
            return literals;
        },
        getActiveScreenDef: function getActiveScreenDef() {
            return screenDef(hidden.getActiveScreenDef());
        },
        getScreenUrlInfo: function getScreenUrlInfo() {
            const info = hidden.getScreenUrlInfo();
            info.targetScreen = screenDef(info.targetScreen);
            if (info.parentScreen) info.parentScreen = screenDef(info.parentScreen);
            return info;
        },
        getFieldOptions: function getFieldOptions(widget) {
            return new Map(hidden.getFieldOptions(...arguments));
        },
        getMenuItems: function getMenuItems() {
            const items = hidden.getMenuItems(...arguments);
            for (const item of items) undefinedIfNull(item, ["menuIndex"]);
            return items;
        },
        staticUrl: function staticUrl(text) {
            const found = hidden.staticUrl(...arguments);
            for (const parameter of found.parameters) undefinedIfNull(parameter, ["from", "value"]);
            for (const parameter of found.screenParameters) undefinedIfNull(parameter, ["from", "value"]);
            return found;
        }
    };
    for (const name of ["sendRedirectAndStopRender", "baseLinkUrl", "getBaseLinkUrl", "setFocusField", "uniqueId",
        "htmlText", "write", "stamp", "renderSection", "renderSectionInclude", "renderSubscreen", "renderIncludeScreen",
        "renderDynamicForm", "renderUserColumnsForm", "renderText", "renderTextValue", "renderInlineText",
        "doBoundaryComments", "getRenderMode", "encodeHtml", "getActiveScreenDef", "getScreenUrlInfo", "pushContext",
        "popContext", "setInContext", "pushSingleFormMapContext", "getSingleFormMap", "holdNodeLits",
        "getFieldValueString", "getFieldValue", "getFieldValuePlainString", "getFieldValueClass", "getFieldValueTexts",
        "getNamedValuePlain", "getFieldEntityValue", "getFieldOptions", "openFormList", "startFormListRow",
        "endFormListRow", "formListSubRowCount", "startFormListSubRow", "endFormListSubRow", "safeCloseList",
        "formListColumnCharWidths", "getFormNode", "getFormInstance", "getFormHiddenParameters",
        "getSavedFindFullLocation", "getUserFormListFinds", "getUserDefaultFormListFindId",
        "getFormListFindScreenScheduled", "appendToAfterScreenWriter", "appendToScriptWriter", "once",
        "getAfterScreenWriterText", "getScriptWriterText", "getThemeIconClass", "getCurrentThemeId", "getThemeValues",
        "getSectionIncludedNode", "getFieldValidation", "getFieldValidationJsRules", "getFieldValidationRules",
        "getFieldTransitionValue", "getMenuItems", "getMenuData", "getExtItems", "getShellData", "getShellProviders",
        "getShellSearch", "staticUrl", "staticDisableKey", "staticCheck", "staticNote", "xlsxSheet", "xlsxHeader",
        "xlsxCell", "xlsxCellCount", "xlsxEndRow",
        "buildUrl", "buildUrlFromTarget", "getScreenUrlInstance", "getCurrentScreenUrl", "makeUrlByType",
        "isAnchorLink"]) {
        sri[name] = wrapped[name] || hidden[name];
    }
    defineProperty(sri, "activeInCurrentMenu", { get: hidden.activeInCurrentMenu, enumerable: true });
})"#;

pub(super) static SHIM: Shim = Shim {
    js: SRI_JS,
    hidden: &[SRI, urls::SRI_URLS],
};

/// The state of an element literal's `$nodeLit`: its conversion, read back by each binder's element conversion.
pub(super) static NODE_LIT: HostClass = HostClass {
    name: "host:NodeLit",
    methods: &[],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

static SRI: &[Method] = &[
    method!(
        "sri",
        "sendRedirectAndStopRender",
        [Text],
        None,
        leaf,
        send_redirect_and_stop_render
    ),
    method!("sri", "baseLinkUrl", [OptValue], Keep, leaf, base_link_url),
    method!("sri", "getBaseLinkUrl", [], Keep, leaf, get_base_link_url),
    method!("sri", "setFocusField", [Text], Keep, leaf, set_focus_field),
    method!("sri", "uniqueId", [Text], Keep, leaf, unique_id),
    method!("sri", "htmlText", [Text], None, leaf, html_text),
    method!("sri", "write", [Value], None, leaf, write),
    method!("sri", "stamp", [Value], None, leaf, stamp),
    method!("sri", "renderSection", [Text], Release, render_section),
    method!("sri", "renderSectionInclude", [Node], Release, render_section_include),
    method!("sri", "renderSubscreen", [], Release, render_subscreen),
    method!(
        "sri",
        "renderIncludeScreen",
        [Text, OptText],
        Release,
        render_include_screen
    ),
    method!("sri", "renderDynamicForm", [Text], Release, render_dynamic_form),
    method!(
        "sri",
        "renderUserColumnsForm",
        [Text],
        Release,
        render_user_columns_form
    ),
    method!("sri", "renderText", [Text, OptText], Release, render_text),
    method!("sri", "renderTextValue", [Text, OptText], Release, render_text_value),
    method!("sri", "renderInlineText", [Text], render_inline_text),
    method!("sri", "doBoundaryComments", [], Keep, leaf, do_boundary_comments),
    method!("sri", "getRenderMode", [], Keep, leaf, get_render_mode),
    method!("sri", "encodeHtml", [OptValue], None, leaf, encode_html),
    method!("sri", "getActiveScreenDef", [], Keep, leaf, get_active_screen_def),
    method!("sri", "getScreenUrlInfo", [], Keep, leaf, get_screen_url_info),
    method!("sri", "pushContext", [], Release, leaf, push_context),
    method!("sri", "popContext", [], Release, leaf, pop_context),
    method!("sri", "setInContext", [Node], Release, set_in_context),
    method!(
        "sri",
        "pushSingleFormMapContext",
        [Value],
        Release,
        push_single_form_map_context
    ),
    method!("sri", "getSingleFormMap", [Value], Keep, get_single_form_map),
    method!(
        "sri",
        "getFieldValueString",
        [Node, OptText, OptText, OptText],
        Keep,
        get_field_value_string
    ),
    method!("sri", "getFieldValue", [Node, OptText], Keep, get_field_value),
    method!(
        "sri",
        "getFieldValuePlainString",
        [Node, OptText],
        Keep,
        get_field_value_plain_string
    ),
    method!("sri", "getFieldValueClass", [Node], Keep, get_field_value_class),
    method!(
        "sri",
        "getFieldValueTexts",
        [Node, OptBool],
        Keep,
        get_field_value_texts
    ),
    method!("sri", "getNamedValuePlain", [Text, Node], Keep, get_named_value_plain),
    method!("sri", "getFieldEntityValue", [Node], Keep, get_field_entity_value),
    method!("sri", "getFieldOptions", [Node], Release, get_field_options),
    method!("sri", "openFormList", [Text], Release, open_form_list),
    method!("sri", "startFormListRow", [Value, Int], Release, start_form_list_row),
    method!("sri", "endFormListRow", [], Release, leaf, pop_context),
    method!(
        "sri",
        "formListSubRowCount",
        [Value, Int],
        Keep,
        form_list_sub_row_count
    ),
    method!(
        "sri",
        "startFormListSubRow",
        [Value, Int, Int],
        Release,
        start_form_list_sub_row
    ),
    method!("sri", "endFormListSubRow", [], Release, leaf, pop_context),
    method!("sri", "safeCloseList", [Value], Keep, safe_close_list),
    method!(
        "sri",
        "formListColumnCharWidths",
        [Value, Int],
        None,
        leaf,
        form_list_column_char_widths
    ),
    method!("sri", "getFormNode", [Text], Keep, get_form_node),
    method!("sri", "getFormInstance", [Text], Keep, get_form_instance),
    method!(
        "sri",
        "getFormHiddenParameters",
        [Node],
        Keep,
        get_form_hidden_parameters
    ),
    method!(
        "sri",
        "getSavedFindFullLocation",
        [Text],
        Keep,
        get_saved_find_full_location
    ),
    method!("sri", "getUserFormListFinds", [Text], Keep, get_user_form_list_finds),
    method!(
        "sri",
        "getUserDefaultFormListFindId",
        [],
        Keep,
        get_user_default_form_list_find_id
    ),
    method!(
        "sri",
        "getFormListFindScreenScheduled",
        [Text],
        Keep,
        get_form_list_find_screen_scheduled
    ),
    method!(
        "sri",
        "appendToAfterScreenWriter",
        [Text],
        Keep,
        leaf,
        append_to_after_screen_writer
    ),
    method!(
        "sri",
        "appendToScriptWriter",
        [Text],
        Keep,
        leaf,
        append_to_script_writer
    ),
    method!("sri", "once", [Text], Keep, leaf, once),
    method!(
        "sri",
        "getAfterScreenWriterText",
        [],
        Keep,
        leaf,
        get_after_screen_writer_text
    ),
    method!("sri", "getScriptWriterText", [], Keep, leaf, get_script_writer_text),
    method!("sri", "getThemeIconClass", [Text], Keep, get_theme_icon_class),
    method!("sri", "getCurrentThemeId", [], Keep, get_current_theme_id),
    method!("sri", "getThemeValues", [Text, OptText], Keep, get_theme_values),
    method!(
        "sri",
        "getSectionIncludedNode",
        [Node],
        None,
        leaf,
        get_section_included_node
    ),
    method!("sri", "getFieldValidation", [Node], Keep, get_field_validation),
    method!(
        "sri",
        "getFieldValidationJsRules",
        [Node],
        Keep,
        get_field_validation_js_rules
    ),
    method!(
        "sri",
        "getFieldValidationRules",
        [Node],
        Keep,
        get_field_validation_rules
    ),
    method!(
        "sri",
        "getFieldTransitionValue",
        [Text, Node, OptValue, OptText, OptBool],
        Keep,
        get_field_transition_value
    ),
    method!("sri", "getMenuItems", [OptText], Keep, get_menu_items),
    method!("sri", "getMenuData", [Texts], Keep, get_menu_data),
    method!("sri", "getExtItems", [Texts], Keep, get_ext_items),
    method!("sri", "getShellData", [], Keep, get_shell_data),
    method!("sri", "getShellProviders", [OptValue], Keep, get_shell_providers),
    method!(
        "sri",
        "getShellSearch",
        [OptValue, OptValue, OptValue],
        Keep,
        get_shell_search
    ),
    method!("sri", "staticUrl", [Text, OptText], Keep, static_url),
    method!("sri", "staticDisableKey", [Node], None, leaf, static_disable_key),
    method!("sri", "staticCheck", [Text], None, leaf, static_check),
    method!("sri", "staticNote", [Text, Text, Text], None, leaf, static_note),
    method!("sri", "xlsxSheet", [Text], Keep, leaf, xlsx_sheet),
    method!("sri", "xlsxHeader", [Value], Keep, leaf, xlsx_header),
    method!("sri", "xlsxCell", [Value, Value, OptBool, OptBool], Keep, xlsx_cell),
    method!("sri", "xlsxCellCount", [], Keep, leaf, xlsx_cell_count),
    method!("sri", "xlsxEndRow", [], Keep, leaf, xlsx_end_row),
    // the conversion holdNodeLits attaches to an element literal, and that of its parent and children
    method!("sri", "nodeLit", [Node], None, leaf, node_lit),
    method!("sri", "nodeLitParent", [Host], None, leaf, node_lit_parent),
    method!("sri", "nodeLitChild", [Host, Int], None, leaf, node_lit_child),
];

/// An error as the QuickJS `sri` functions threw it: its message, kind and retry seconds, with no diagnostic on the
/// stack.
fn thrown(err: &crate::context::Error) -> HostError {
    let mut plain = crate::context::Error::script(err.message()).with_kind(err.kind());
    if let Some(seconds) = err.retry_after() {
        plain = plain.with_retry_after(seconds);
    }
    HostError::Context(plain)
}

/// `body` on the running screen render (the method's flush ran first).
fn in_render<T>(body: impl FnOnce(&RenderState) -> Result<T, crate::context::Error>) -> Result<T, HostError> {
    match with_current_render(body) {
        Some(Ok(value)) => Ok(value),
        Some(Err(err)) => Err(thrown(&err)),
        None => Err(HostError::message("no screen render is running")),
    }
}

/// [`in_render`] of a body that does not fail.
fn read_render<T>(body: impl FnOnce(&RenderState) -> T) -> Result<T, HostError> {
    in_render(|state| Ok(body(state)))
}

/// What the QuickJS functions returned for their effect: empty text.
fn done() -> Ret {
    Ret::from(String::new())
}

/// The element literal of a [`super::ArgKind::Node`] argument.
fn node(call: &Call<'_>, index: usize) -> Result<Arc<NodeLit>, HostError> {
    call.node(index)
        .cloned()
        .ok_or_else(|| HostError::message("expected an element object"))
}

fn opt_text<'c>(call: &'c Call<'_>, index: usize) -> &'c str {
    call.text(index).unwrap_or("")
}

fn count(number: usize) -> Value {
    Value::I32(i32::try_from(number).unwrap_or(i32::MAX))
}

fn map(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn text(value: impl Into<String>) -> Value {
    Value::String(value.into())
}

/// The type name a QuickJS conversion error gives a converted value.
fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::I32(_) | Value::I64(_) => "int",
        Value::F64(_) => "float",
        Value::String(_) => "string",
        Value::List(_) => "array",
        _ => "object",
    }
}

fn not_of_type(value: &Value, type_name: &str) -> HostError {
    HostError::Type(format!(
        "Error converting from js '{}' into type '{type_name}'",
        type_of(value)
    ))
}

// ---- writing and rendering ----

fn send_redirect_and_stop_render(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::screen::send_redirect_and_stop_render(opt_text(call, 0));
    Ok(Ret::Undefined)
}

/// `sri.stamp(src)`: the source stamp of the next element written, in a render that writes stamps (F10).
fn stamp(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = match call.value(0) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    crate::screen::stamp::set_pending(text);
    Ok(Ret::Undefined)
}

/// `sri.write(text)`: a string as it is (the macros' every write, so not copied), anything else as its plain text.
fn write(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = match call.value(0) {
        Some(Value::String(text)) => std::borrow::Cow::Borrowed(text.as_str()),
        other => std::borrow::Cow::Owned(moqui_lib::object::to_plain_string(other.unwrap_or(&Value::Null))),
    };
    read_render(|state| logic::write(state, &text))?;
    Ok(Ret::Undefined)
}

fn render_section(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = opt_text(call, 0).to_string();
    let runner = call.ops.screen_runner()?;
    in_render(|state| logic::render_section(state, runner, &name))?;
    Ok(done())
}

fn render_section_include(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let node = node(call, 0)?;
    let runner = call.ops.screen_runner()?;
    in_render(|state| logic::render_section_include(state, runner, &node))?;
    Ok(done())
}

fn render_subscreen(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let runner = call.ops.screen_runner()?;
    in_render(|state| logic::render_subscreen(state, runner))?;
    Ok(done())
}

fn render_dynamic_form(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = opt_text(call, 0).to_string();
    let runner = call.ops.screen_runner()?;
    in_render(|state| logic::render_dynamic_form(state, runner, &name))?;
    Ok(done())
}

fn render_user_columns_form(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = opt_text(call, 0).to_string();
    let runner = call.ops.screen_runner()?;
    in_render(|state| logic::render_user_columns_form(state, runner, &name)).map(Ret::from)
}

fn render_include_screen(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = opt_text(call, 0).to_string();
    let share = opt_text(call, 1) == "true";
    let runner = call.ops.screen_runner()?;
    in_render(|state| logic::render_include_screen(state, runner, &location, share))?;
    Ok(done())
}

fn render_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|state| logic::render_text(state, opt_text(call, 0), opt_text(call, 1))).map(Ret::from)
}

/// `sri.renderTextValue(location, template)`: the rendered text of a location, returned rather than written.
fn render_text_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|state| logic::render_text_value(state, opt_text(call, 0), opt_text(call, 1))).map(Ret::from)
}

/// `sri.renderInlineText(body)`: a `<text template="true">` body as a trusted fancy template. A security or tarpit
/// refusal of a screen the template renders keeps its kind and retry seconds.
fn render_inline_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let body = opt_text(call, 0).to_string();
    let runner = call.ops.screen_runner()?;
    runner
        .render_inline_text(&body)
        .map(Ret::from)
        .map_err(|err| thrown(&err))
}

fn do_boundary_comments(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(logic::do_boundary_comments).map(Ret::from)
}

fn get_render_mode(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| state.render_mode.clone()).map(Ret::from)
}

/// `sri.encodeHtml(text)`: null-safe as Java's `WebUtilities.encodeHtml`, so a null or undefined argument is empty
/// text and a number or a boolean is its text.
fn encode_html(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(logic::encode_html(&call.text_or_empty(0)).into())
}

/// `sri.htmlText(html)`: the JSON text of the blocks and numbered links of an `html` tui node.
fn html_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::screen::html_text::html_text(opt_text(call, 0))
        .to_json()
        .to_string()
        .into())
}

/// A screen definition as the shim makes its object of: `location`, `screenName`, and the default menu name.
fn screen_def(def: &crate::screen::ScreenDef) -> Value {
    map([
        ("location", text(def.location.as_str())),
        ("screenName", text(def.screen_name.as_str())),
        ("defaultMenuName", text(def.default_menu_title.as_str())),
    ])
}

fn get_active_screen_def(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let def = read_render(logic::active_def)?;
    Ok(Ret::Value(screen_def(&def)))
}

/// `sri.baseLinkUrl(url)`: set (or with null clear) the base URL of relative links in rendered content. Returns `sri`'s
/// answer in Java, the render; scripts call it for its effect, so this returns nothing.
fn base_link_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let url = call.opt_text(0).filter(|url| !url.is_empty());
    read_render(|state| *state.base_link_url.borrow_mut() = url)?;
    Ok(Ret::Undefined)
}

/// `sri.setFocusField(id)`: the envelope focus, the first id a form asks for. Later calls leave it.
fn set_focus_field(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| logic::set_focus_field(state, opt_text(call, 0)))?;
    Ok(Ret::Undefined)
}

/// `sri.uniqueId(id)`: a tui node id no earlier node of the document has.
fn unique_id(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| logic::unique_id(state, opt_text(call, 0))).map(Ret::from)
}

/// The base URL `sri.baseLinkUrl` set, or null.
fn get_base_link_url(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| state.base_link_url.borrow().clone()).map(Ret::from)
}

fn texts(list: &[String]) -> Value {
    Value::List(list.iter().cloned().map(Value::String).collect())
}

/// `sri.getScreenUrlInfo()`: the screen the request ended at and the one above it, for the page title, the path
/// name lists (`fullPathNameList`, `minimalPathNameList`, `preTransitionPathNameList`, `extraPathNameList`), and
/// `density`, that of the deepest screen on the path that sets one (handheld), else empty.
fn get_screen_url_info(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| {
        let info = &state.url_info;
        let defs = &info.path_defs;
        let parent = defs.len().checked_sub(2).and_then(|index| defs.get(index));
        let density = defs
            .iter()
            .rev()
            .find_map(|def| def.density.clone())
            .unwrap_or_default();
        let mut object = IndexMap::new();
        object.insert("extraPathNameList".to_string(), texts(&info.extra_path));
        object.insert("fullPathNameList".to_string(), texts(&info.full_path));
        object.insert("minimalPathNameList".to_string(), texts(&info.minimal_path));
        object.insert(
            "preTransitionPathNameList".to_string(),
            texts(&info.pre_transition_path),
        );
        object.insert("targetScreen".to_string(), screen_def(&info.target_screen));
        object.insert("density".to_string(), Value::String(density));
        if let Some(parent) = parent {
            object.insert("parentScreen".to_string(), screen_def(parent));
        }
        Ret::Value(Value::Map(object))
    })
}

// ---- the context ----

fn push_context(_: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::script::push_context_frame();
    Ok(done())
}

fn pop_context(_: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::script::runtime::pop_context_frame();
    Ok(done())
}

fn set_in_context(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let node = node(call, 0)?;
    in_render(|_| logic::set_in_context(&node))?;
    Ok(done())
}

/// The map of a form-single: the value itself, or an expression string as `form-single.@map` is.
fn form_map(call: &Call<'_>) -> Result<Value, HostError> {
    match call.value_or_null(0) {
        Value::String(expression) if expression.is_empty() => Ok(Value::Null),
        Value::String(expression) => {
            crate::script::eval_value(&expression).map_err(|err| HostError::message(err.message()))
        }
        other => Ok(other),
    }
}

fn push_single_form_map_context(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let host = form_map(call)?;
    logic::push_single_form_map_context(&host);
    Ok(done())
}

fn get_single_form_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let host = form_map(call)?;
    let fields = crate::script::value_map(&host).unwrap_or_default();
    Ok(Ret::Value(Value::Map(fields)))
}

// ---- fields ----

fn get_field_value_string(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let node = node(call, 0)?;
    let given = call.passed(1) || call.passed(2);
    in_render(|state| {
        if given {
            logic::get_field_value_string_for(&node, opt_text(call, 1), opt_text(call, 2), opt_text(call, 3))
        } else {
            logic::get_field_value_string(state, &node)
        }
    })
    .map(Ret::from)
}

fn get_field_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let field = node(call, 0)?;
    in_render(|_| logic::get_field_value(&field, opt_text(call, 1))).map(Ret::Value)
}

fn get_field_value_plain_string(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let field = node(call, 0)?;
    in_render(|_| logic::get_field_value_plain_string(&field, opt_text(call, 1))).map(Ret::from)
}

/// `sri.getFieldValueTexts(widget, withString)`: `[value string, plain string, value class]` of a display widget's field,
/// one call where a cell made three.
fn get_field_value_texts(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let widget = node(call, 0)?;
    let with_string = call.bool(1).unwrap_or(true);
    let found = in_render(|state| logic::get_field_value_texts(state, &widget, with_string))?;
    Ok(Ret::from(Vec::from(found)))
}

fn get_field_value_class(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let field = node(call, 0)?;
    in_render(|_| logic::get_field_value_class(&field)).map(Ret::from)
}

fn get_named_value_plain(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let form = node(call, 1)?;
    in_render(|_| logic::get_named_value_plain(opt_text(call, 0), &form)).map(Ret::from)
}

fn get_field_entity_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let widget = node(call, 0)?;
    in_render(|_| logic::get_field_entity_value(&widget)).map(Ret::from)
}

/// The `[key, text]` pairs the shim makes a `Map` of, so the order of the options is kept whatever the keys look like.
fn get_field_options(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let widget = node(call, 0)?;
    let options = in_render(|state| logic::get_field_options(state, &widget))?;
    let pairs = options
        .into_iter()
        .map(|(key, text)| Value::List(vec![Value::String(key), Value::String(text)]))
        .collect();
    Ok(Ret::Value(Value::List(pairs)))
}

// ---- form lists ----

fn open_form_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = opt_text(call, 0).to_string();
    let (id, rows) = in_render(|state| logic::open_form_list(state, &name))?;
    Ok(Ret::Value(map([
        ("id", count(id)),
        ("count", count(rows)),
        ("name", Value::String(name)),
    ])))
}

/// The id of a form-list handle `openFormList` gave.
fn list_id(call: &Call<'_>) -> Result<usize, HostError> {
    let Some(Value::Map(info)) = call.value(0) else {
        return Err(HostError::message("expected a form-list handle"));
    };
    let id = match info.get("id") {
        Some(Value::I32(id)) => i64::from(*id),
        Some(Value::I64(id)) => *id,
        Some(other) => return Err(not_of_type(other, "i32")),
        None => {
            return Err(HostError::Type(
                "Error converting from js 'undefined' into type 'i32'".to_string(),
            ))
        }
    };
    usize::try_from(id).map_err(|_| HostError::message("bad form-list handle"))
}

fn index_arg(call: &Call<'_>, index: usize, what: &str) -> Result<usize, HostError> {
    usize::try_from(call.int(index).unwrap_or(-1)).map_err(|_| HostError::message(format!("bad {what} index")))
}

fn start_form_list_row(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let id = list_id(call)?;
    let index = index_arg(call, 1, "row")?;
    in_render(|state| logic::start_form_list_row(state, id, index))?;
    Ok(done())
}

fn form_list_sub_row_count(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let id = list_id(call)?;
    let index = index_arg(call, 1, "row")?;
    let rows = read_render(|state| logic::form_list_sub_row_count(state, id, index))?;
    Ok(Ret::Value(count(rows)))
}

fn start_form_list_sub_row(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let id = list_id(call)?;
    let index = index_arg(call, 1, "row")?;
    let sub = index_arg(call, 2, "sub-row")?;
    in_render(|state| logic::start_form_list_sub_row(state, id, index, sub))?;
    Ok(done())
}

fn safe_close_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    if let Ok(id) = list_id(call) {
        read_render(|state| logic::close_form_list(state, id))?;
    }
    Ok(done())
}

fn form_list_column_char_widths(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let columns = call.value_or_null(0);
    let Value::List(columns) = columns else {
        return Err(not_of_type(&columns, "array"));
    };
    let mut specs = Vec::with_capacity(columns.len());
    for column in &columns {
        let Value::List(fields) = column else {
            return Err(not_of_type(column, "array"));
        };
        let mut spec = Vec::with_capacity(fields.len());
        for field in fields {
            let Value::List(parts) = field else {
                return Err(not_of_type(field, "array"));
            };
            let part = |index: usize| match parts.get(index) {
                Some(Value::String(text)) => text.clone(),
                _ => String::new(),
            };
            spec.push((part(0), part(1)));
        }
        specs.push(spec);
    }
    let line_width = i64::from(call.int(1).unwrap_or(0));
    let widths = logic::form_list_column_char_widths(&specs, line_width).map_err(|err| thrown(&err))?;
    Ok(Ret::Value(Value::List(
        widths
            .into_iter()
            .map(|width| Value::I32(i32::try_from(width).unwrap_or(i32::MAX)))
            .collect(),
    )))
}

// ---- forms ----

/// An element as `{name, text, attrs, children}`.
fn node_value(node: &MNode) -> Value {
    let mut object = IndexMap::new();
    object.insert("name".to_string(), node.with_name(|name| text(name)));
    object.insert(
        "text".to_string(),
        node.with_text(|found| text(found.unwrap_or_default())),
    );
    let mut attrs = IndexMap::new();
    node.for_each_attribute(|key, value| {
        attrs.insert(key.to_string(), text(value));
    });
    object.insert("attrs".to_string(), Value::Map(attrs));
    let children = node.children().iter().map(node_value).collect();
    object.insert("children".to_string(), Value::List(children));
    Value::Map(object)
}

fn get_form_node(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let form = in_render(|state| logic::get_form(state, opt_text(call, 0)))?;
    let node = form.shared_node().map_err(|err| thrown(&err))?;
    Ok(Ret::Value(node_value(&node)))
}

fn get_form_instance(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let form = in_render(|state| logic::get_form(state, opt_text(call, 0)))?;
    let node = form.shared_node().map_err(|err| thrown(&err))?;
    Ok(Ret::Value(map([
        ("formNode", node_value(&node)),
        ("name", text(form.name.as_str())),
        ("list", Value::Bool(form.list)),
    ])))
}

/// `sri.getSavedFindFullLocation(formName)`: `{screen}.form_list${name}`, plus `#entity` or `#service` when dynamic.
fn get_saved_find_full_location(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|state| logic::saved_find_full_location(state, opt_text(call, 0))).map(Ret::from)
}

/// `sri.getUserFormListFinds(formName)`: the saved finds of the user and the user's groups for a form of the screen.
fn get_user_form_list_finds(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|state| logic::user_form_list_finds(state, opt_text(call, 0))).map(|finds| Ret::Value(Value::List(finds)))
}

/// `sri.getFormListFindScreenScheduled(formListFindId)`: the user's schedule of a saved find, or null.
fn get_form_list_find_screen_scheduled(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|_| logic::form_list_find_screen_scheduled(opt_text(call, 0)))
        .map(|found| Ret::Value(found.unwrap_or(Value::Null)))
}

/// `sri.getUserDefaultFormListFindId()`: the user's default saved find of the screen, or null.
fn get_user_default_form_list_find_id(_: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(logic::user_default_form_list_find_id).map(Ret::from)
}

fn get_form_hidden_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let form = node(call, 0)?;
    let parameters = match form.child("hidden-parameters") {
        Some(hidden) => in_render(|_| logic::node_parameters(hidden))?,
        None => IndexMap::new(),
    };
    let mut out = IndexMap::new();
    for (key, value) in parameters {
        let text = moqui_lib::object::to_plain_string(&value);
        if !text.is_empty() {
            out.insert(key, Value::String(text));
        }
    }
    Ok(Ret::Value(Value::Map(out)))
}

// ---- the writers and the theme ----

fn append_to_after_screen_writer(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| logic::append_to_after_screen_writer(state, opt_text(call, 0)))?;
    Ok(done())
}

fn append_to_script_writer(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| logic::append_to_script_writer(state, opt_text(call, 0)))?;
    Ok(done())
}

/// `sri.once(key)`: true the first time a render sees the key.
fn once(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| state.once.borrow_mut().insert(opt_text(call, 0).to_string())).map(Ret::from)
}

fn get_after_screen_writer_text(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| std::mem::take(&mut *state.after_text.borrow_mut())).map(Ret::from)
}

fn get_script_writer_text(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| std::mem::take(&mut *state.script_text.borrow_mut())).map(Ret::from)
}

fn get_theme_icon_class(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|state| logic::get_theme_icon_class(state, opt_text(call, 0))).map(Ret::from)
}

fn get_current_theme_id(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(logic::current_theme_id).map(Ret::from)
}

fn get_theme_values(call: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(|state| logic::get_theme_values(state, opt_text(call, 0), opt_text(call, 1))).map(Ret::from)
}

fn get_section_included_node(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let node = node(call, 0)?;
    let name = if node.attr("name").is_empty() {
        node.attr("location").split_once('#').map_or("", |(_, name)| name)
    } else {
        node.attr("name")
    };
    Ok(Ret::Value(map([
        ("name", text("section")),
        ("attrs", map([("name", text(name))])),
    ])))
}

// ---- validation and options ----

fn get_field_validation(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let sub = node(call, 0)?;
    let found = in_render(|_| logic::field_validation(&sub))?;
    let mut object = IndexMap::new();
    object.insert("classes".to_string(), text(found.classes.join(" ")));
    if let Some((regexp, message)) = found.regexp {
        object.insert("regexp".to_string(), Value::String(regexp));
        object.insert("message".to_string(), Value::String(message));
    }
    Ok(Ret::Value(Value::Map(object)))
}

/// `getFieldValidationJsRules(sub)`: `[{expr, message}]`, the browser checks of a field's validation.
fn get_field_validation_js_rules(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let sub = node(call, 0)?;
    let rules = in_render(|_| logic::field_validation_rules(&sub))?;
    let out = rules
        .into_iter()
        .map(|(rule, message)| map([("expr", text(rule.js())), ("message", Value::String(message))]))
        .collect();
    Ok(Ret::Value(Value::List(out)))
}

/// `getFieldValidationRules(sub)`: the same checks as data, `[{kind, ..., message}]`, for a client that does not run
/// JavaScript (tui).
fn get_field_validation_rules(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let sub = node(call, 0)?;
    let rules = in_render(|_| logic::field_validation_rules(&sub))?;
    let mut out = Vec::with_capacity(rules.len());
    for (rule, message) in rules {
        let mut object = IndexMap::new();
        // A rule's JSON is one flat object of strings, booleans and nulls.
        if let serde_json::Value::Object(fields) = rule.json() {
            for (key, value) in fields {
                match value {
                    serde_json::Value::String(found) => {
                        object.insert(key, Value::String(found));
                    }
                    serde_json::Value::Bool(flag) => {
                        object.insert(key, Value::Bool(flag));
                    }
                    _ => {}
                }
            }
        }
        object.insert("message".to_string(), Value::String(message));
        out.push(Value::Map(object));
    }
    Ok(Ret::Value(Value::List(out)))
}

/// `getFieldTransitionValue(transition, node, term, labelField, alwaysGet)`: the label a transition gives for a value.
fn get_field_transition_value(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let node = node(call, 1)?;
    let term = call.text_or_empty(2);
    let always_get = call.bool(4).unwrap_or(false);
    in_render(|state| {
        logic::field_transition_value(state, opt_text(call, 0), &node, &term, opt_text(call, 3), always_get)
    })
    .map(Ret::from)
}

// ---- menus and the shell ----

fn get_menu_items(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let items = in_render(|state| logic::menu_items(state, call.text(0)))?;
    let out = items
        .into_iter()
        .map(|item| {
            map([
                ("name", Value::String(item.name)),
                ("hasParameters", Value::Bool(item.has_parameters)),
                ("title", Value::String(item.title)),
                ("url", Value::String(item.url)),
                ("disabled", Value::Bool(item.disabled)),
                ("inPath", Value::Bool(item.in_path)),
                ("image", Value::String(item.image)),
                ("imageType", Value::String(item.image_type)),
                ("location", Value::String(item.location)),
                ("menuIndex", item.menu_index.map_or(Value::Null, Value::I64)),
                ("component", Value::String(item.component)),
            ])
        })
        .collect();
    Ok(Ret::Value(Value::List(out)))
}

/// A found value, or null after an error was sent.
fn or_null(found: Option<Value>) -> Ret {
    Ret::Value(found.unwrap_or(Value::Null))
}

fn path_names(call: &Call<'_>) -> Vec<String> {
    call.texts(0).map(<[String]>::to_vec).unwrap_or_default()
}

/// `sri.getMenuData(pathNameList)`: the menu data of the `menuData` transition, or null after an error was sent.
fn get_menu_data(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let names = path_names(call);
    in_render(|state| crate::screen::menu::menu_data(state, &names)).map(or_null)
}

/// `sri.getExtItems(pathNames)`: the declared actions and panels of the screen at the path (`screen/ext.rs`), or null
/// after an error was sent.
fn get_ext_items(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let names = path_names(call);
    in_render(|state| crate::screen::ext::ext_items(state, &names)).map(or_null)
}

/// `sri.getShellData()`: the shell data of the `shellData` transition (`screen/shell.rs`), or null after an error was sent.
fn get_shell_data(_: &mut Call<'_>) -> Result<Ret, HostError> {
    in_render(crate::screen::shell::shell_data).map(or_null)
}

/// `sri.getShellProviders(kind)`: the providers of the apps the user may open without the menus and counts of
/// `getShellData` (`screen/shell.rs` `shell_providers`), only `kind` in each app's providers when given; kept per session.
fn get_shell_providers(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let kind = call.opt_text(0);
    in_render(|state| crate::screen::shell::shell_providers(state, kind.as_deref())).map(Ret::Value)
}

/// `sri.getShellSearch(text, groups, limit)`: the palette groups of the `shellSearch` transition, or null after an error
/// was sent. Each argument may be null.
fn get_shell_search(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (text, groups, limit) = (call.text_or_empty(0), call.text_or_empty(1), call.text_or_empty(2));
    in_render(|state| crate::screen::shell::shell_search(state, &text, &groups, &limit)).map(or_null)
}

// ---- static templates ----

/// `sri.staticDisableKey(node)`: the request-package key of a static control's availability.
fn static_disable_key(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let node = node(call, 0)?;
    Ok(crate::screen::statics::disable_key(&node).into())
}

/// Parameter definitions as `[{name, from, value}]` (a `from` or `value` that is not there is made undefined by the
/// shim).
fn static_parameters(parameters: Vec<crate::screen::ParameterDef>) -> Value {
    Value::List(
        parameters
            .into_iter()
            .map(|parameter| {
                map([
                    ("name", Value::String(parameter.name)),
                    ("from", parameter.from.map_or(Value::Null, Value::String)),
                    ("value", parameter.value.map_or(Value::Null, Value::String)),
                ])
            })
            .collect(),
    )
}

fn pairs(pairs: Vec<(String, String)>) -> Value {
    Value::Map(
        pairs
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect(),
    )
}

/// `sri.staticUrl(text, urlType)`: a static link's target and parameter definitions.
fn static_url(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let url_type = call.text(1).unwrap_or("transition");
    let found = in_render(|state| crate::screen::urls::static_url(state, opt_text(call, 0), url_type))?;
    Ok(Ret::Value(map([
        ("inlineParameters", pairs(found.inline_parameters)),
        ("screenInlineParameters", pairs(found.screen_inline_parameters)),
        ("path", Value::String(found.path)),
        ("screenPath", Value::String(found.screen_path)),
        ("exists", Value::Bool(found.exists)),
        ("isTransition", Value::Bool(found.is_transition)),
        ("isScreen", Value::Bool(found.is_screen)),
        ("readOnly", Value::Bool(found.read_only)),
        ("method", Value::String(found.method)),
        ("screenUrl", Value::Bool(found.screen_url)),
        ("parameters", static_parameters(found.parameters)),
        ("screenParameters", static_parameters(found.screen_parameters)),
    ])))
}

/// `sri.staticCheck(json)`: the expressions a static macro wrote, as `[{expr, element, attr}]`. Only a compatibility
/// report looks at them.
fn static_check(call: &mut Call<'_>) -> Result<Ret, HostError> {
    if let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(opt_text(call, 0)) {
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
    Ok(Ret::Undefined)
}

/// `sri.staticNote(kind, element, detail)`: server-only work a static macro could not write into the template.
fn static_note(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::screen::statics::record_note(opt_text(call, 0), opt_text(call, 1), opt_text(call, 2));
    Ok(Ret::Undefined)
}

// ---- xlsx (the moqui-poi port): ScreenXlsx.js fills the render's workbook ----

fn xlsx_sheet(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| state.xlsx.borrow_mut().start_sheet(opt_text(call, 0)))?;
    Ok(Ret::Undefined)
}

fn xlsx_header(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let title = moqui_lib::object::to_plain_string(&call.value_or_null(0));
    read_render(|state| state.xlsx.borrow_mut().header(&title))?;
    Ok(Ret::Undefined)
}

/// `sri.xlsxCell(values, formats, currency, alignRight)`: one cell from the values of a field's widgets.
fn xlsx_cell(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let values = call.value_or_null(0);
    let Value::List(values) = values else {
        return Err(not_of_type(&values, "array"));
    };
    let formats = call.value_or_null(1);
    let Value::List(formats) = formats else {
        return Err(not_of_type(&formats, "array"));
    };
    let mut cells = Vec::with_capacity(values.len());
    for (index, value) in values.into_iter().enumerate() {
        let format = match formats.get(index) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(format)) => format.clone(),
            Some(other) => return Err(not_of_type(other, "string")),
        };
        cells.push((value, format));
    }
    let hints = crate::screen::xlsx::CellHints {
        currency: call.bool(2).unwrap_or(false),
        align_right: call.bool(3).unwrap_or(false),
    };
    in_render(|state| state.xlsx.borrow_mut().cell(&cells, hints))?;
    Ok(Ret::Undefined)
}

fn xlsx_cell_count(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| state.xlsx.borrow().cell_count()).map(|cells| Ret::Value(count(cells)))
}

fn xlsx_end_row(_: &mut Call<'_>) -> Result<Ret, HostError> {
    read_render(|state| state.xlsx.borrow_mut().end_row())?;
    Ok(Ret::Undefined)
}

// ---- element literals ----

fn lit_object(node: Arc<NodeLit>) -> Ret {
    Ret::Object(HostObject(node), &NODE_LIT)
}

/// The conversion of a unit's element literal, which `holdNodeLits` attaches to it.
fn node_lit(call: &mut Call<'_>) -> Result<Ret, HostError> {
    node(call, 0).map(lit_object)
}

fn held(call: &Call<'_>) -> Result<Arc<NodeLit>, HostError> {
    call.host::<NodeLit>(0)
        .ok_or_else(|| HostError::Type("expected an element literal's conversion".to_string()))
}

/// The conversion of the element's parent, or null.
fn node_lit_parent(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let held = held(call)?;
    Ok(match &held.parent {
        Some(parent) => lit_object(Arc::new(NodeLit::clone(parent))),
        None => Ret::Value(Value::Null),
    })
}

/// The conversion of the element's child at the index, or null.
fn node_lit_child(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let held = held(call)?;
    let child = usize::try_from(call.int(1).unwrap_or(-1))
        .ok()
        .and_then(|index| held.children.get(index));
    Ok(match child {
        Some(child) => lit_object(Arc::new(child.clone())),
        None => Ret::Value(Value::Null),
    })
}

/// A script of `sri.holdNodeLits` and the element conversions, which each binder's tests run (`script/runtime/bind_tests.rs`,
/// `script/oxide/node_lits.rs`).
#[cfg(test)]
pub(crate) const HOLD_SCRIPT: &str = "(() => { \
    const field = { name: 'field', attrs: { name: 'f1' }, children: [] }; \
    const lits = sri.holdNodeLits([ \
        { name: 'section-include', attrs: { location: 'a.xml#Inner' }, parent: field, \
          children: [{ name: 'parameter', attrs: { name: 'p' } }] }, \
        { name: 'section', attrs: { name: 'Named' } }]); \
    const out = []; \
    out.push(Object.isFrozen(lits[0]) && Object.isFrozen(lits[0].attrs) && Object.isFrozen(lits[0].children) \
        && Object.isFrozen(lits[0].children[0]) && Object.isFrozen(field)); \
    out.push(Object.keys(lits[0]).join(',')); \
    out.push([typeof lits[0].$nodeLit, typeof field.$nodeLit, typeof lits[0].children[0].$nodeLit].join(',')); \
    out.push(JSON.stringify(sri.getSectionIncludedNode(lits[0]))); \
    out.push(JSON.stringify(sri.getSectionIncludedNode(lits[1]))); \
    out.push(JSON.stringify(sri.getSectionIncludedNode({ name: 'x', attrs: { location: 'b#Plain' } }))); \
    out.push(sri.staticDisableKey(lits[0]) === sri.staticDisableKey({ name: 'section-include', \
        attrs: { location: 'a.xml#Inner' }, parent: { name: 'field', attrs: { name: 'f1' }, children: [] }, \
        children: [{ name: 'parameter', attrs: { name: 'p' } }] })); \
    for (const bad of [() => sri.getSectionIncludedNode(7), () => sri.holdNodeLits([1]), \
            () => sri.holdNodeLits('x'), () => sri.getSectionIncludedNode({ attrs: { n: 1 } })]) { \
        try { bad(); out.push('ran'); } catch (e) { out.push(e.message); } \
    } \
    return out.join(' | '); })()";

/// What [`HOLD_SCRIPT`] gives.
#[cfg(test)]
pub(crate) const HOLD_EXPECTED: &str = "true | name,attrs,parent,children | object,object,object \
    | {\"name\":\"section\",\"attrs\":{\"name\":\"Inner\"}} | {\"name\":\"section\",\"attrs\":{\"name\":\"Named\"}} \
    | {\"name\":\"section\",\"attrs\":{\"name\":\"Plain\"}} | true | expected an element object \
    | expected an element object | Error converting from js 'string' into type 'array' \
    | Error converting from js 'int' into type 'string'";
