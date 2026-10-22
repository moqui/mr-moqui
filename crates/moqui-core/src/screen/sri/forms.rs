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

//! Forms: dynamic and user-column forms, saved finds, and the rows of a form-list being rendered.

use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::sections::guarded;
use super::{active_def, FormRows, Runner};
use crate::context::{Error, ExecutionContext};
use crate::screen::definition::ScreenDef;
use crate::screen::form::{self, FormInstance};
use crate::screen::generate::{Unit, UnitRequest};
use crate::screen::render::RenderState;
use crate::script;
use crate::script::ContextFrame;

fn form_of(state: &RenderState, name: &str) -> Result<Arc<FormInstance>, Error> {
    if let Some(found) = state.dynamic_forms.borrow().get(name) {
        return Ok(Arc::clone(found));
    }
    let def = active_def(state);
    form::assemble(&state.facade, &state.factory, &def, name)
}

/// `sri.renderDynamicForm(name)`: assemble a `dynamic="true"` form from the context as it is now, then
/// generate its code (once for each different result) and run it.
///
/// # Errors
///
/// Returns a security error. Any other error is written as text.
pub fn render_dynamic_form(state: &RenderState, runner: &dyn Runner, name: &str) -> Result<(), Error> {
    let def = active_def(state);
    let label = format!("form [{name}] in screen [{}]", def.location);
    guarded(state, &label, || {
        let form = form::assemble_dynamic(&state.facade, &state.factory, &def, name)?;
        let unit = runner.generate_dynamic(&form_unit_request(state, &def), &form)?;
        run_form_unit(state, runner, name, form, &unit)
    })
}

/// The request for the code of a form assembled for this render.
fn form_unit_request<'a>(state: &'a RenderState, def: &'a Arc<ScreenDef>) -> UnitRequest<'a> {
    UnitRequest {
        factory: &state.factory,
        facade: &state.facade,
        def,
        section: "",
        fail: false,
        mode: &state.render_mode,
        macro_location: &state.macro_location,
    }
}

/// Run the generated code of a form assembled for this render, with that form in place of `name` while it runs.
fn run_form_unit(
    state: &RenderState,
    runner: &dyn Runner,
    name: &str,
    form: Arc<FormInstance>,
    unit: &Unit,
) -> Result<(), Error> {
    let previous = state.dynamic_forms.borrow_mut().insert(name.to_string(), form);
    let result = runner.run(unit);
    match previous {
        Some(previous) => state.dynamic_forms.borrow_mut().insert(name.to_string(), previous),
        None => state.dynamic_forms.borrow_mut().remove(name),
    };
    result
}

/// `sri.getUserFormListFinds(name)`: the saved finds the user may use for a form of the active screen.
///
/// # Errors
///
/// Returns an error when the saved find records cannot be read.
pub fn user_form_list_finds(state: &RenderState, name: &str) -> Result<Vec<Value>, Error> {
    let def = active_def(state);
    let Some(form) = def.forms.get(name) else {
        return Ok(Vec::new());
    };
    let location = crate::screen::saved_finds::form_location(&def.location, form.list, name);
    let node = form.node.node()?;
    crate::screen::saved_finds::user_form_list_finds(&crate::screen::saved_finds::saved_find_location(
        &location,
        form.dynamic,
        &node,
    ))
}

/// `sri.getSavedFindFullLocation(name)`: the location saved finds of this form are kept under.
/// Empty when the active screen has no form of that name.
///
/// # Errors
///
/// Returns an error when the form's source XML does not parse.
pub fn saved_find_full_location(state: &RenderState, name: &str) -> Result<String, Error> {
    let def = active_def(state);
    let Some(form) = def.forms.get(name) else {
        return Ok(String::new());
    };
    let location = crate::screen::saved_finds::form_location(&def.location, form.list, name);
    let node = form.node.node()?;
    Ok(crate::screen::saved_finds::saved_find_location(
        &location,
        form.dynamic,
        &node,
    ))
}

/// `sri.getFormListFindScreenScheduled(formListFindId)`: the newest schedule of a saved find for the user or one of the
/// user's groups, with its cron description.
///
/// # Errors
///
/// Returns an error when the records cannot be read.
pub fn form_list_find_screen_scheduled(find_id: &str) -> Result<Option<Value>, Error> {
    crate::screen::saved_finds::form_list_find_screen_scheduled(find_id)
}

/// `sri.getUserDefaultFormListFindId()`: the user's default saved find of the active screen.
///
/// # Errors
///
/// Returns an error when the record cannot be read.
pub fn user_default_form_list_find_id(state: &RenderState) -> Result<Option<String>, Error> {
    crate::screen::saved_finds::user_default_find_id(&active_def(state).location)
}

/// `sri.renderUserColumnsForm(name)`: a form-list whose columns the user chose (`FormConfig`, through the column dialog
/// or the active saved find) is assembled with those columns, then generated and run as a dynamic form is. Returns
/// false, rendering nothing, when the user has no columns for it, so the form's own code runs.
///
/// # Errors
///
/// Returns a security error. Any other error is written as text.
pub fn render_user_columns_form(state: &RenderState, runner: &dyn Runner, name: &str) -> Result<bool, Error> {
    let def = active_def(state);
    let Some(form_def) = def.forms.get(name) else {
        return Ok(false);
    };
    let location = crate::screen::saved_finds::form_location(&def.location, form_def.list, name);
    let source = form_def.node.node()?;
    let saved = crate::screen::saved_finds::saved_find_location(&location, form_def.dynamic, &source);
    let Some(columns) = crate::screen::saved_finds::user_columns(&location, &saved)? else {
        return Ok(false);
    };
    let label = format!("form [{name}] in screen [{}]", def.location);
    let mut rendered = false;
    guarded(state, &label, || {
        let base = if form_def.dynamic {
            form::assemble_dynamic(&state.facade, &state.factory, &def, name)?
        } else {
            form::assemble(&state.facade, &state.factory, &def, name)?
        };
        let node = base.node()?;
        let fields: Vec<String> = node
            .children_named("field")
            .iter()
            .filter_map(|field| field.attribute("name"))
            .collect();
        node.remove_where(|child| {
            child.name() == "form-list-column"
                || (child.name() == "columns" && child.attribute("type").is_none_or(|kind| kind.is_empty()))
        });
        let config = moqui_lib::node::MNode::new("columns");
        for column in &columns {
            let column_node = config.append("column");
            for field in column.iter().filter(|field| fields.contains(field)) {
                column_node.append("field-ref").set_attribute("name", field.clone());
            }
        }
        node.append_node(config);
        let form = Arc::new(form::FormInstance::from_node(&def, &node)?);
        let unit = runner.generate_dynamic(&form_unit_request(state, &def), &form)?;
        rendered = true;
        run_form_unit(state, runner, name, form, &unit)
    })?;
    Ok(rendered)
}

/// `sri.getFormInstance(name)`: the assembled form.
///
/// # Errors
///
/// Returns an error when the form is not in the screen.
pub fn get_form(state: &RenderState, name: &str) -> Result<Arc<FormInstance>, Error> {
    form_of(state, name)
}

/// `sri.pushSingleFormMapContext(map)`: a frame holding the map's fields and `_formMap`.
pub fn push_single_form_map_context(map: &Value) {
    single_form_map_frame(map).keep();
}

/// A frame holding the map's fields and `_formMap`, popped when dropped.
pub(in crate::screen) fn single_form_map_frame(map: &Value) -> ContextFrame {
    form_map_frame(script::value_map(map).unwrap_or_default())
}

fn form_map_frame(fields: IndexMap<String, Value>) -> ContextFrame {
    let frame = ContextFrame::push();
    script::put_context_all(&fields);
    let _ = script::set_context_name("_formMap", Value::Map(fields));
    frame
}

/// `sri.openFormList(name)`: run the form's list and row actions. Returns the handle and the row count.
///
/// # Errors
///
/// Returns an error when the form, its find, its row actions or a field `from` fails.
pub fn open_form_list(state: &RenderState, name: &str) -> Result<(usize, usize), Error> {
    let form = form_of(state, name)?;
    let rows = form_rows(&form)?;
    let count = rows.len();
    let mut lists = state.form_lists.borrow_mut();
    lists.push(Some(FormRows { rows }));
    Ok((lists.len() - 1, count))
}

/// The rows of a form-list. Each row is the context frame after `row-actions` and the fields' `from`
/// expressions have run on the entry, as `AggregationUtil.aggregateList` makes them.
fn form_rows(form: &FormInstance) -> Result<Vec<IndexMap<String, Value>>, Error> {
    let list_value = match &form.entity_find {
        Some(find) => {
            script::run_actions(&find.xml)?;
            script::context_value(&form.list_name).unwrap_or(Value::Null)
        }
        None => script::eval_value(&form.list_name)?,
    };
    let items = script::value_items(&list_value).unwrap_or_default();
    let count = items.len();
    let mut rows = Vec::with_capacity(count);
    for (index, item) in items.into_iter().enumerate() {
        let fields = script::value_map(&item).unwrap_or_default();
        let frame = ContextFrame::push();
        script::put_context_all(&fields);
        let index_value = Value::I64(index as i64);
        let has_next = Value::Bool(index + 1 < count);
        let prefix = if form.list_entry.is_empty() {
            &form.list_name
        } else {
            &form.list_entry
        };
        if form.list_entry.is_empty() {
            let _ = script::set_context_name(&format!("{prefix}_entry"), item.clone());
        } else {
            let _ = script::set_context_name(&form.list_entry, item.clone());
        }
        let _ = script::set_context_name(&format!("{prefix}_index"), index_value);
        let _ = script::set_context_name(&format!("{prefix}_has_next"), has_next);
        let ran = (|| -> Result<(), Error> {
            if let Some(actions) = &form.row_actions {
                script::run_actions(&actions.xml)?;
            }
            for (field, from) in &form.field_from {
                let value = script::eval_value(from)?;
                script::set_context_name(field, value)?;
            }
            Ok(())
        })();
        let row = frame.take();
        ran?;
        rows.push(row);
    }
    if let Some(aggregation) = &form.aggregation {
        let shown = if aggregation.group_fields.is_empty() {
            None
        } else {
            Some(shown_list_fields(form)?)
        };
        rows = aggregation.apply(rows, shown.as_ref(), |row, name| {
            row.get(name)
                .cloned()
                .or_else(|| script::context_value(name))
                .unwrap_or(Value::Null)
        });
    }
    // After aggregation, so a scheduled screen can abort on the rows that were shown. A bare name does not see this
    // map; ScreenServices reads `ec.context.sharedMap`.
    note_form_list_rows(rows.len());
    Ok(rows)
}

/// The fields of a form-list Java's `getListObject` passes as `includeFields`: the columns of the request's `_uiType`
/// less those whose `hide` holds, evaluated once in the screen context, and for an `entity-find` list the hidden
/// fields with a `default-field`. The node already holds the user's column choice (`render_user_columns_form`).
fn shown_list_fields(form: &FormInstance) -> Result<std::collections::HashSet<String>, Error> {
    let node = form.node()?;
    let ui_type = script::context_value("_uiType").map(|value| moqui_lib::object::to_plain_string(&value));
    let mut shown = std::collections::HashSet::new();
    for (name, hide) in form::list_column_fields(&node, ui_type.as_deref()) {
        if let Some(hide) = hide {
            if script::eval_condition(&hide)? {
                continue;
            }
        }
        shown.insert(name);
    }
    if form.entity_find.is_some() {
        shown.extend(form::hidden_list_fields_with_default(&node));
    }
    Ok(shown)
}

/// `_formListRendered` and `_formListResultCount` on the shared map, as Java's `getListObject`. Several lists add.
fn note_form_list_rows(count: usize) {
    let _ = ExecutionContext::with_current(|ec| {
        let mut shared = ec.shared_map();
        let previous = shared.get("_formListResultCount").map(shared_count).unwrap_or(0);
        shared.insert("_formListRendered".to_string(), Value::Bool(true));
        let added = i64::try_from(count).unwrap_or(i64::MAX);
        shared.insert(
            "_formListResultCount".to_string(),
            Value::I64(previous.saturating_add(added)),
        );
    });
}

fn shared_count(value: &Value) -> i64 {
    match value {
        Value::I64(number) => *number,
        Value::I32(number) => i64::from(*number),
        _ => 0,
    }
}

/// `sri.formListSubRowCount(info, index)`: the number of sub-list rows of a row of an aggregated form-list.
pub fn form_list_sub_row_count(state: &RenderState, handle: usize, index: usize) -> usize {
    let lists = state.form_lists.borrow();
    match lists
        .get(handle)
        .and_then(Option::as_ref)
        .and_then(|list| list.rows.get(index))
        .and_then(|row| row.get("aggregateSubList"))
    {
        Some(Value::List(items)) => items.len(),
        _ => 0,
    }
}

/// `sri.startFormListSubRow(info, index, subIndex)`: a frame over the row's frame holding the sub-list entry and `_formMap`,
/// as Java's `startFormListSubRow`. `sri.endFormListSubRow()` pops it.
///
/// # Errors
///
/// Returns an error for a row or a sub-row that is not there.
pub fn start_form_list_sub_row(state: &RenderState, handle: usize, index: usize, sub: usize) -> Result<(), Error> {
    form_list_sub_row_frame(state, handle, index, sub).map(ContextFrame::keep)
}

/// The frame of `sri.startFormListSubRow`, popped when dropped.
pub(in crate::screen) fn form_list_sub_row_frame(
    state: &RenderState,
    handle: usize,
    index: usize,
    sub: usize,
) -> Result<ContextFrame, Error> {
    let entry = {
        let lists = state.form_lists.borrow();
        let row = lists
            .get(handle)
            .and_then(Option::as_ref)
            .and_then(|list| list.rows.get(index))
            .ok_or_else(|| Error::new("form-list row is not available"))?;
        match row.get("aggregateSubList") {
            Some(Value::List(items)) => items.get(sub).cloned(),
            _ => None,
        }
        .ok_or_else(|| Error::new("form-list sub-row is not available"))?
    };
    Ok(form_map_frame(script::value_map(&entry).unwrap_or_default()))
}

/// `sri.startFormListRow`: a frame holding the row and `_formMap`.
///
/// # Errors
///
/// Returns an error for a list that was closed or a row past the end.
pub fn start_form_list_row(state: &RenderState, handle: usize, index: usize) -> Result<(), Error> {
    form_list_row_frame(state, handle, index).map(ContextFrame::keep)
}

/// The frame of `sri.startFormListRow`, popped when dropped.
pub(in crate::screen) fn form_list_row_frame(
    state: &RenderState,
    handle: usize,
    index: usize,
) -> Result<ContextFrame, Error> {
    let row = {
        let lists = state.form_lists.borrow();
        lists
            .get(handle)
            .and_then(Option::as_ref)
            .and_then(|list| list.rows.get(index))
            .cloned()
            .ok_or_else(|| Error::new("form-list row is not available"))?
    };
    Ok(form_map_frame(row))
}

/// `sri.safeCloseList`: drop a form-list's rows.
pub fn close_form_list(state: &RenderState, handle: usize) {
    if let Some(slot) = state.form_lists.borrow_mut().get_mut(handle) {
        *slot = None;
    }
}
