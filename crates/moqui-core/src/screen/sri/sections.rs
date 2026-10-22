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

//! Render control: the active screen, sections, section-iterate, subscreens, included screens and text.

use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::{active_def, encode_html, write, NodeLit, Runner};
use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::screen::cache::OutputKind;
use crate::screen::definition::{ScreenDef, SectionDef, SectionKind};
use crate::screen::generate::UnitRequest;
use crate::screen::render::{check_condition, Frames, RenderState};
use crate::script::ContextFrame;
use crate::{script, web};

/// An error that answers who may see what: it stops a render, and is not written in place of the output.
pub(in crate::screen) fn is_security(err: &Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::AuthenticationRequired | ErrorKind::Unauthorized | ErrorKind::Tarpit
    )
}

/// Run a nested render. As in Java, an error other than a security one is logged and its message is
/// written as text where the output would have been, so one section does not lose the page. In production the text
/// names only an error id; the detail is in the log under it.
///
/// A qvt page is a Vue template, which does not compile with the elements the failed render left open, so there what it
/// wrote is dropped before the message, and a brace in the message is a character reference, not an interpolation.
/// A JSON document drops the same bytes, then writes one `error` node so the array stays valid.
pub(super) fn guarded(state: &RenderState, label: &str, body: impl FnOnce() -> Result<(), Error>) -> Result<(), Error> {
    let capture = matches!(state.output_kind, OutputKind::Vue | OutputKind::Json);
    let written = capture.then(|| web::output_body_len().unwrap_or(0));
    match body() {
        Err(err) if !is_security(&err) => {
            let detail = format!("Error rendering {label}: {err}");
            let text = if web::is_production() {
                format!(
                    "Error rendering {label}: internal error, id {}",
                    web::log_error_id(&detail)
                )
            } else {
                log_render_failure(&detail);
                format!("Error rendering {label}: {}", err.message())
            };
            if let Some(len) = written {
                web::truncate_output(len);
            }
            let body = match state.output_kind {
                OutputKind::Vue => encode_html(&text).replace('{', "&#123;"),
                OutputKind::Json => json_error_array(&text),
                OutputKind::Text | OutputKind::Html => encode_html(&text),
            };
            write(state, &body);
            Ok(())
        }
        other => other,
    }
}

/// Log a failed section or screen on the subscriber that is current now.
///
/// Dispatched directly. A `tracing` callsite first hit with no subscriber stays disabled for the rest of the
/// process, which hides this line from a subscriber installed by a later test.
fn log_render_failure(detail: &str) {
    static META: tracing::Metadata<'static> = tracing::metadata! {
        name: "render failure",
        target: module_path!(),
        level: tracing::Level::ERROR,
        fields: &["message"],
        callsite: &CALLSITE,
        kind: tracing::metadata::Kind::EVENT,
    };
    static CALLSITE: tracing::callsite::DefaultCallsite = tracing::callsite::DefaultCallsite::new(&META);

    tracing::dispatcher::get_default(|dispatch| {
        if !dispatch.enabled(&META) {
            return;
        }
        // The value set borrows temporaries the macro builds. Passing it into a call keeps them alive for the event.
        emit_event(dispatch, &META, &tracing::valueset!(META.fields(), message = %detail));
    });
}

fn emit_event<'a>(
    dispatch: &tracing::Dispatch,
    meta: &'static tracing::Metadata<'static>,
    values: &tracing::field::ValueSet<'a>,
) {
    dispatch.event(&tracing::Event::new(meta, values));
}

/// Drop C0 and C1 controls other than tab and newline, so a value cannot carry a terminal sequence into the document.
pub(in crate::screen) fn strip_controls(text: &str) -> String {
    text.chars()
        .filter(|ch| {
            let code = u32::from(*ch);
            *ch == '\t' || *ch == '\n' || (code >= 0x20 && code != 0x7F && !(0x80..0xA0).contains(&code))
        })
        .collect()
}

/// One `error` node in an array, the shape a JSON section writes when it fails.
pub(super) fn json_error_array(text: &str) -> String {
    let encoded = serde_json::to_string(&strip_controls(text)).unwrap_or_else(|_| "\"\"".to_string());
    format!(r#"[{{"t":"error","text":{encoded}}}]"#)
}

/// `ScreenDefinition.render`: the active screen's frame, then its root section.
///
/// # Errors
///
/// Returns an error when the user may not view the screen, or a section raises one. In qvt below the root, only a
/// security error.
pub fn render_active_screen(state: &RenderState, runner: &dyn Runner, is_target: bool) -> Result<(), Error> {
    let def = active_def(state);
    // A qvt or tui render below the root screen is a screen the client shows inside its parents, so its error is
    // written in its place and the page continues.
    if matches!(state.output_kind, OutputKind::Vue | OutputKind::Json) && state.screen_path_index.get() > 0 {
        return guarded(state, &format!("screen [{}]", def.location), || {
            render_def(state, runner, &def, is_target)
        });
    }
    render_def(state, runner, &def, is_target)
}

fn render_def(state: &RenderState, runner: &dyn Runner, def: &Arc<ScreenDef>, is_target: bool) -> Result<(), Error> {
    let mut frames = Frames::default();
    frames.enter_screen(def, is_target)?;
    frames.hit_detail(Some(&state.content_type), def.track_artifact_hit);
    render_section_of(state, runner, def, "")
}

/// `sri.renderSection(name)`.
///
/// # Errors
///
/// Returns a security error. Any other error is written as text, as Java does.
pub fn render_section(state: &RenderState, runner: &dyn Runner, name: &str) -> Result<(), Error> {
    let def = active_def(state);
    let label = format!("section [{name}] in screen [{}]", def.location);
    json_value(state, || {
        guarded(state, &label, || render_section_of(state, runner, &def, name))
    })
}

/// In a JSON document a nested render is the value of its parent's `"c"`, so one that writes nothing (an empty
/// section, a stopped screen, a screen with no widgets) writes `[]`.
fn json_value(state: &RenderState, body: impl FnOnce() -> Result<(), Error>) -> Result<(), Error> {
    let before = (state.output_kind == OutputKind::Json).then(|| web::output_body_len().unwrap_or(0));
    body()?;
    if before.is_some_and(|before| web::output_body_len().unwrap_or(0) == before) {
        write(state, "[]");
    }
    Ok(())
}

/// The screen of a form that one of `def`'s forms `extends`, when that screen has the section `name`: a section
/// inside an extended form's field, which Java adds to the extending screen's sections (`ScreenForm`).
fn extended_section_screen(state: &RenderState, def: &ScreenDef, name: &str) -> Result<Option<Arc<ScreenDef>>, Error> {
    for form in def.forms.values() {
        let Some(extends) = form.node.node()?.attribute("extends") else {
            continue;
        };
        let Some((location, _)) = extends.split_once('#') else {
            continue;
        };
        if location == def.location || location.contains("${") {
            continue;
        }
        let other = state.facade.screen_def(&state.factory, location)?;
        if other.sections.contains_key(name) {
            return Ok(Some(other));
        }
    }
    Ok(None)
}

/// `sri.renderSectionInclude(node)`.
///
/// # Errors
///
/// Same as [`render_section`].
pub fn render_section_include(state: &RenderState, runner: &dyn Runner, include: &NodeLit) -> Result<(), Error> {
    let location = include.attr("location");
    let name = include.attr("name");
    if location.contains("${") || name.contains("${") {
        let def = active_def(state);
        let label = format!("section include [{location}#{name}] in screen [{}]", def.location);
        return json_value(state, || {
            guarded(state, &label, || {
                let location = script::expand(location)?;
                let name = script::expand(name)?;
                let other = state.facade.screen_def(&state.factory, &location)?;
                let previous = state.override_def.replace(Some(Arc::clone(&other)));
                let result = render_section_of(state, runner, &other, &name);
                state.override_def.replace(previous);
                result
            })
        });
    }
    let name = match (name.is_empty(), location.split_once('#')) {
        (true, Some((_, section))) => section,
        _ => name,
    };
    render_section(state, runner, name)
}

fn render_section_of(state: &RenderState, runner: &dyn Runner, def: &Arc<ScreenDef>, name: &str) -> Result<(), Error> {
    if !name.is_empty() && !def.sections.contains_key(name) {
        if let Some(other) = extended_section_screen(state, def, name)? {
            let previous = state.override_def.replace(Some(Arc::clone(&other)));
            let result = render_section_of(state, runner, &other, name);
            state.override_def.replace(previous);
            return result;
        }
    }
    let section = if name.is_empty() {
        &def.root
    } else {
        def.sections
            .get(name)
            .ok_or_else(|| Error::new(format!("No section with name [{name}] in screen [{}]", def.location)))?
    };
    if let SectionKind::Include { location, name } = &section.kind {
        let request = NodeLit {
            name: "section-include".to_string(),
            attrs: IndexMap::from([
                ("location".to_string(), location.clone()),
                ("name".to_string(), name.clone()),
            ]),
            ..NodeLit::default()
        };
        return render_section_include(state, runner, &request);
    }
    match &section.kind {
        SectionKind::Iterate {
            list,
            entry,
            key,
            paginate,
        } => {
            let value = script::eval_value(list)?;
            if is_empty(&value) {
                if state.output_kind == OutputKind::Json {
                    write(state, "[]");
                }
                return Ok(());
            }
            let mut items = iteration_entries(&value);
            let _frame = paginate.then(ContextFrame::push);
            if *paginate {
                items = paginate_items(list, items)?;
            }
            iterate_section(state, runner, def, section, name, entry, key.as_deref(), &items)
        }
        _ => render_single(state, runner, def, section, name),
    }
}

/// The entries of a section-iterate list: each item, and for a map its key and value.
pub(in crate::screen) fn iteration_entries(value: &Value) -> Vec<(Option<String>, Value)> {
    match value {
        Value::Map(entries) => entries
            .iter()
            .map(|(key, item)| (Some(key.clone()), item.clone()))
            .collect(),
        other => script::value_items(other)
            .unwrap_or_default()
            .into_iter()
            .map(|item| (None, item))
            .collect(),
    }
}

/// `CollectionUtilities.paginateList`: the page of `items` named by `pageIndex` and `pageSize`, with
/// the list's count and page variables put in the context. A list that is already a page (`{list}AlreadyPaginated`,
/// or a `{list}Count` from a paginated find) is returned as it is.
pub(in crate::screen) fn paginate_items(
    list_name: &str,
    items: Vec<(Option<String>, Value)>,
) -> Result<Vec<(Option<String>, Value)>, Error> {
    let mut context = script::context_combined();
    let already = format!("{list_name}AlreadyPaginated");
    if context.contains_key(&already) {
        return Ok(items);
    }
    let size = i32::try_from(items.len()).map_err(|_| Error::new("list is too long to paginate"))?;
    moqui_lib::collection::paginate_parameters(size, list_name, &mut context)?;
    if context.contains_key(&already) {
        return Ok(items);
    }
    for suffix in [
        "Count",
        "PageIndex",
        "PageSize",
        "PageMaxIndex",
        "PageRangeLow",
        "PageRangeHigh",
    ] {
        let name = format!("{list_name}{suffix}");
        if let Some(value) = context.get(&name) {
            script::set_context_name(&name, value.clone())?;
        }
    }
    let number = |suffix: &str| {
        context
            .get(&format!("{list_name}{suffix}"))
            .map(to_plain_string)
            .and_then(|text| text.parse::<usize>().ok())
            .unwrap_or(0)
    };
    let (low, high) = (number("PageRangeLow"), number("PageRangeHigh"));
    let start = low.saturating_sub(1).min(items.len());
    let end = high.min(items.len()).max(start);
    Ok(items[start..end].to_vec())
}

#[expect(clippy::too_many_arguments)]
fn iterate_section(
    state: &RenderState,
    runner: &dyn Runner,
    def: &Arc<ScreenDef>,
    section: &SectionDef,
    name: &str,
    entry: &str,
    key: Option<&str>,
    items: &[(Option<String>, Value)],
) -> Result<(), Error> {
    let json = state.output_kind == OutputKind::Json;
    if json {
        write(state, "[");
    }
    let mut have_item = false;
    for (index, (item_key, item)) in items.iter().enumerate() {
        let frame = ContextFrame::push();
        let _ = script::set_context_name(entry, item.clone());
        if let (Some(key_name), Some(item_key)) = (key, item_key) {
            let _ = script::set_context_name(key_name, Value::string(item_key));
        }
        let _ = script::set_context_name("sectionEntryIndex", Value::I64(index as i64));
        let _ = script::set_context_name(&format!("{entry}_index"), Value::I64(index as i64));
        let _ = script::set_context_name(&format!("{entry}_has_next"), Value::Bool(index + 1 < items.len()));
        let start = json.then(|| web::output_body_len().unwrap_or(0));
        let result = render_single(state, runner, def, section, name);
        drop(frame);
        if let Some(start) = start {
            if result.is_ok() {
                have_item = splice_json_array_item(start, have_item);
            }
        }
        result?;
        if state.dont_do_render.get() {
            break;
        }
    }
    if json {
        write(state, "]");
    }
    Ok(())
}

/// One iteration wrote `[inner]` or `[]`. Fold it into the array this loop opened: drop an empty array, and put a
/// comma before the next inner value. Bytes that are not an array would break the document, so they are logged and
/// become one error node.
fn splice_json_array_item(start: usize, have_item: bool) -> bool {
    let tail = web::with_body(|body| body.get(start..).map(<[u8]>::to_vec).unwrap_or_default());
    let trimmed = tail.trim_ascii();
    let error;
    let inner = match trimmed.strip_prefix(b"[").and_then(|rest| rest.strip_suffix(b"]")) {
        Some(inner) => inner.trim_ascii(),
        None if trimmed.is_empty() => trimmed,
        None => {
            tracing::error!("A section-iterate entry did not render a JSON array");
            error = json_error_array("a section-iterate entry did not render as JSON");
            &error.as_bytes()[1..error.len() - 1]
        }
    };
    if inner.is_empty() {
        web::truncate_output(start);
        return have_item;
    }
    let mut next = Vec::with_capacity(inner.len() + usize::from(have_item));
    if have_item {
        next.push(b',');
    }
    next.extend_from_slice(inner);
    web::rewrite_tail(start, &next);
    true
}

fn render_single(
    state: &RenderState,
    runner: &dyn Runner,
    def: &Arc<ScreenDef>,
    section: &SectionDef,
    name: &str,
) -> Result<(), Error> {
    let holds = match &section.condition {
        Some(condition) => check_condition(condition)?,
        None => true,
    };
    let fail = !holds;
    if holds {
        if let Some(actions) = &section.actions {
            script::run_actions(&actions.xml)?;
        }
        if state.dont_do_render.get() {
            return Ok(());
        }
    }
    let request = UnitRequest {
        factory: &state.factory,
        facade: &state.facade,
        def,
        section: name,
        fail,
        mode: &state.render_mode,
        macro_location: &state.macro_location,
    };
    let Some(unit) = runner.generate(&request)? else {
        return Ok(());
    };
    let errors = ExecutionContext::with_current(|ec| ec.message().errors().join("\n")).unwrap_or_default();
    if holds && !errors.is_empty() {
        // Rendering widgets after an error in the actions would only add more errors.
        if state.output_kind == OutputKind::Json {
            write(state, &json_error_array(&errors));
        } else {
            write(state, &encode_html(&errors));
        }
        return Ok(());
    }
    runner.run(&unit)
}

/// `sri.renderSubscreen()`: the next screen in the path.
///
/// # Errors
///
/// Returns a security error, and the error of a subscreen that is not available.
pub fn render_subscreen(state: &RenderState, runner: &dyn Runner) -> Result<(), Error> {
    json_value(state, || render_next_screen(state, runner))
}

fn render_next_screen(state: &RenderState, runner: &dyn Runner) -> Result<(), Error> {
    let defs = &state.url_info.path_defs;
    let index = state.screen_path_index.get();
    if index + 1 >= defs.len() {
        // include-child-content: the file under the target screen is the subscreen, rendered as a template.
        if let Some(file) = &state.url_info.file_resource {
            let text = crate::script::template::TemplateRegistry::of(&state.factory).render_location_default(
                &state.factory,
                file,
                None,
                "",
            )?;
            if state.output_kind == OutputKind::Json {
                let encoded = serde_json::to_string(&strip_controls(&text)).unwrap_or_else(|_| "\"\"".to_string());
                write(state, &format!(r#"[{{"t":"text","text":{encoded}}}]"#));
            } else {
                write(state, &text);
            }
            return Ok(());
        }
        // An always-standalone mode (`tui`) asks for each level on its own: the level under this one is the client's
        // next request, so a parent rendered alone has nothing here, with or without a default item.
        if state.always_standalone {
            return Ok(());
        }
        let def = active_def(state);
        let message = format!(
            "Tried to render subscreen in screen [{}] but there is no subscreens.@default-item, and no more valid subscreen names in the screen path [{:?}]",
            def.location, state.url_info.full_path
        );
        if state.output_kind == OutputKind::Json {
            write(state, &json_error_array(&message));
        } else {
            write(state, &encode_html(&message));
        }
        return Ok(());
    }
    let next = Arc::clone(&defs[index + 1]);
    let item = state
        .url_info
        .full_path
        .get(index)
        .and_then(|name| active_def(state).subscreens.get(name).cloned());
    // An item limited to a user group is there only for its members, at any depth.
    if item.as_ref().is_some_and(|item| !item.is_valid_in_current_context()) {
        return Err(Error::of_kind(
            ErrorKind::Unauthorized,
            format!("The screen {} is not available", next.screen_name),
        ));
    }
    // `disable-when` blocks the screen below the first level. (Java defines it but reads it nowhere; kept here.)
    let disabled = match item.as_ref().and_then(|item| item.disable_when.as_ref()) {
        Some(disable) if index > 0 => script::eval_condition(disable)?,
        _ => false,
    };
    if disabled {
        return Err(Error::of_kind(
            ErrorKind::Unauthorized,
            format!("The screen {} is not available", next.screen_name),
        ));
    }
    state.screen_path_index.set(index + 1);
    let result = if state.stopped.borrow().contains(&next.location) {
        Ok(())
    } else {
        let is_target = index + 2 >= defs.len();
        guarded(state, &format!("screen [{}]", next.location), || {
            render_def(state, runner, &next, is_target)
        })
    };
    state.screen_path_index.set(index);
    result
}

/// `sri.renderIncludeScreen(location, shareScope)`.
///
/// # Errors
///
/// Returns a security error. Any other error is written as text.
pub fn render_include_screen(
    state: &RenderState,
    runner: &dyn Runner,
    location: &str,
    share_scope: bool,
) -> Result<(), Error> {
    let _frame = (!share_scope).then(ContextFrame::push);
    let previous = state.override_def.borrow().clone();
    let result = json_value(state, || {
        guarded(state, &format!("screen [{location}]"), || {
            let def = state.facade.screen_def(&state.factory, location)?;
            *state.override_def.borrow_mut() = Some(Arc::clone(&def));
            render_def(state, runner, &def, false)
        })
    });
    *state.override_def.borrow_mut() = previous;
    result
}

/// `sri.renderText(location, template)`. With template `false` the file text is returned. Otherwise
/// the renderer registered for the file extension renders it with the live context and the result is
/// written, and the return is empty. A location whose extension has no renderer is a trusted fancy
/// template. FreeMarker files are not rendered.
///
/// # Errors
///
/// Returns an error when the file cannot be read, is a FreeMarker template, or fails to render.
pub fn render_text(state: &RenderState, location: &str, template: &str) -> Result<String, Error> {
    if template == "false" {
        return render_text_value(state, location, template);
    }
    let rendered = render_text_value(state, location, template)?;
    write(state, &rendered);
    Ok(String::new())
}

/// `sri.renderTextValue(location, template)`: as [`render_text`], but the rendered text is returned rather than
/// written. The tui mode checks a text as JSON before it writes it.
///
/// # Errors
///
/// As [`render_text`].
pub fn render_text_value(state: &RenderState, location: &str, template: &str) -> Result<String, Error> {
    if location.is_empty() || location == "null" {
        tracing::warn!(
            "Not rendering text in screen [{}], location was empty",
            active_def(state).location
        );
        return Ok(String::new());
    }
    let registry = script::template::TemplateRegistry::of(&state.factory);
    if template == "false" {
        return registry
            .location_text(&state.factory, location)
            .map(|text| text.to_string());
    }
    if location.ends_with(".ftl") {
        return Err(Error::new(format!(
            "FreeMarker template {location} is not supported; port it to the fancy dialect"
        )));
    }
    // As Java's renderText: the renderer of the location's extension, else of the `template` value as an extension,
    // else the file's text as it is, so a .js or .css file is not run as a template.
    let _frame = ContextFrame::push();
    registry.render_location_default(&state.factory, location, None, template)
}

pub fn append_to_after_screen_writer(state: &RenderState, text: &str) {
    state.after_text.borrow_mut().push_str(text);
}

pub fn append_to_script_writer(state: &RenderState, text: &str) {
    state.script_text.borrow_mut().push_str(text);
}
