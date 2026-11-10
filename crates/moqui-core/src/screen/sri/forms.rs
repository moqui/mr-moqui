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
    crate::screen::saved_finds::user_form_list_finds(&crate::screen::saved_finds::saved_find_location(
        &location,
        form.dynamic,
        &form.find_key,
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
    Ok(crate::screen::saved_finds::saved_find_location(
        &location,
        form.dynamic,
        &form.find_key,
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
    let saved = crate::screen::saved_finds::saved_find_location(&location, form_def.dynamic, &form_def.find_key);
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
            child.has_name("form-list-column")
                || (child.has_name("columns") && child.attribute("type").is_none_or(|kind| kind.is_empty()))
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
    // the fields and `_formMap`, one copy of them made, as a put of each and then of `_formMap` leaves the frame
    let mut entries = fields.clone();
    entries.insert("_formMap".to_string(), Value::Map(fields));
    ContextFrame::push_entries(entries)
}

/// `sri.openFormList(name, streamed)`: run the form's list and row actions. Returns the handle and the row count.
///
/// With `streamed` (the csv, text and xml macros, which read the rows once in order) a list with no aggregation is
/// read a row at a time instead (docs/ui/Screen-Output.md): its find opens a cursor, each row's actions run when
/// [`start_form_list_row`] asks for it, and the count is `None`. The rows are then never held together.
///
/// # Errors
///
/// Returns an error when the form, its find, its row actions or a field `from` fails.
pub fn open_form_list(state: &RenderState, name: &str, streamed: bool) -> Result<(usize, Option<usize>), Error> {
    let form = form_of(state, name)?;
    if streamed && form.aggregation.is_none() {
        let stream = RowStream::open(form)?;
        let mut lists = state.form_lists.borrow_mut();
        lists.push(Some(FormRows {
            rows: Vec::new(),
            stream: Some(stream),
        }));
        return Ok((lists.len() - 1, None));
    }
    let rows = form_rows(&form)?;
    let count = rows.len();
    let mut lists = state.form_lists.borrow_mut();
    lists.push(Some(FormRows { rows, stream: None }));
    Ok((lists.len() - 1, Some(count)))
}

/// The list a form-list shows: what its find gives (the cursor variant when `cursor`), else its `list`.
fn list_value(form: &FormInstance, cursor: bool) -> Result<Value, Error> {
    let find = if cursor {
        form.entity_find_cursor.as_ref().or(form.entity_find.as_ref())
    } else {
        form.entity_find.as_ref()
    };
    Ok(match find {
        Some(find) => {
            find.run()?;
            script::context_value(&form.list_name).unwrap_or(Value::Null)
        }
        // A plain name held in the context is that value, as Java's `expression(list)` gives the object itself: through
        // JavaScript a list's time stamps would come back as their milliseconds and a long as an integer. Anything
        // else (an expression, a name not in the context) is evaluated.
        None => plain_name(&form.list_name)
            .then(|| script::context_value(&form.list_name))
            .flatten()
            .map_or_else(|| script::eval_value(&form.list_name), Ok)?,
    })
}

/// The context names of a row: its entry, its index, and whether another follows.
#[derive(Debug)]
struct RowNames {
    entry: String,
    index: String,
    has_next: String,
}

impl RowNames {
    fn of(form: &FormInstance) -> Self {
        let prefix = if form.list_entry.is_empty() {
            &form.list_name
        } else {
            &form.list_entry
        };
        let entry = if form.list_entry.is_empty() {
            format!("{prefix}_entry")
        } else {
            form.list_entry.clone()
        };
        Self {
            entry,
            index: format!("{prefix}_index"),
            has_next: format!("{prefix}_has_next"),
        }
    }
}

/// One row: the context frame after `row-actions` and the fields' `from` expressions have run on the entry, as
/// `AggregationUtil.aggregateList` makes it.
fn form_row(
    form: &FormInstance,
    names: &RowNames,
    item: Value,
    index: usize,
    has_next: bool,
) -> Result<IndexMap<String, Value>, Error> {
    // the item's fields, the entry, its index and whether one follows, in the order a put of each leaves them
    let mut entries = script::value_map(&item).unwrap_or_default();
    entries.insert(names.entry.clone(), item);
    entries.insert(names.index.clone(), Value::I64(index as i64));
    entries.insert(names.has_next.clone(), Value::Bool(has_next));
    let frame = ContextFrame::push_entries(entries);
    let ran = (|| -> Result<(), Error> {
        if let Some(actions) = &form.row_actions {
            actions.run()?;
        }
        for (field, from) in &form.field_from {
            let value = script::eval_value(from)?;
            script::set_context_name(field, value)?;
        }
        Ok(())
    })();
    let row = frame.take();
    ran?;
    Ok(row)
}

/// The rows of a form-list, each made by [`form_row`], aggregated when the form says so.
fn form_rows(form: &FormInstance) -> Result<Vec<IndexMap<String, Value>>, Error> {
    let list_value = list_value(form, false)?;
    let items = script::value_items(&list_value).unwrap_or_default();
    let count = items.len();
    let names = RowNames::of(form);
    let mut rows = Vec::with_capacity(count);
    for (index, item) in items.into_iter().enumerate() {
        rows.push(form_row(form, &names, item, index, index + 1 < count)?);
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

/// Whether `text` is a JavaScript identifier, a name a script reads from the context.
fn plain_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// The fields of a form-list Java's `getListObject` passes as `includeFields`: the columns of the request's `_uiType`
/// less those whose `hide` holds, evaluated once in the screen context, and for an `entity-find` list the hidden
/// fields with a `default-field`. The node already holds the user's column choice (`render_user_columns_form`).
fn shown_list_fields(form: &FormInstance) -> Result<std::collections::HashSet<String>, Error> {
    let parsed;
    let columns = match &form.list_columns {
        Some(columns) => columns,
        None => {
            parsed = form::ListColumns::of(&form.shared_node()?);
            &parsed
        }
    };
    let ui_type = script::context_value("_uiType").map(|value| moqui_lib::object::to_plain_string(&value));
    let mut shown = std::collections::HashSet::new();
    for (name, hide) in columns.for_ui_type(ui_type.as_deref()) {
        if let Some(hide) = hide {
            if script::eval_condition(hide)? {
                continue;
            }
        }
        shown.insert(name.clone());
    }
    if form.entity_find.is_some() {
        shown.extend(columns.hidden_with_default.iter().cloned());
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

/// The rows of a form-list read one at a time ([`open_form_list`] streamed): from a cursor, or from a list whose rows
/// are made as they are asked for.
pub(crate) struct RowStream {
    form: Arc<FormInstance>,
    names: RowNames,
    source: RowSource,
    /// The index of the next row.
    next: usize,
    ended: bool,
}

enum RowSource {
    Cursor(Arc<script::host::list_iterator::IteratorHandle>),
    Items(std::vec::IntoIter<Value>),
}

impl std::fmt::Debug for RowStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowStream")
            .field("form", &self.form.name)
            .field("next", &self.next)
            .field("ended", &self.ended)
            .finish()
    }
}

impl RowStream {
    fn open(form: Arc<FormInstance>) -> Result<Self, Error> {
        let value = list_value(&form, true)?;
        let source = match script::host::list_iterator::iterator_of(&value) {
            Some(cursor) => RowSource::Cursor(cursor),
            None => RowSource::Items(script::value_items(&value).unwrap_or_default().into_iter()),
        };
        Ok(Self {
            names: RowNames::of(&form),
            form,
            source,
            next: 0,
            ended: false,
        })
    }

    /// Row `index`, which must be the next; `None` after the last.
    fn row(&mut self, index: usize) -> Result<Option<IndexMap<String, Value>>, Error> {
        if self.ended {
            return Ok(None);
        }
        if index != self.next {
            return Err(Error::new(format!(
                "form-list {}: a list sent a row at a time is read once, in order; row {index} asked for after {}",
                self.form.name, self.next
            )));
        }
        let item = match &mut self.source {
            RowSource::Cursor(cursor) => cursor.next()?.map(script::host::entities::value_of),
            RowSource::Items(items) => items.next(),
        };
        let Some(item) = item else {
            self.end();
            return Ok(None);
        };
        let has_next = match &mut self.source {
            RowSource::Cursor(cursor) => cursor.has_next()?,
            RowSource::Items(items) => items.len() > 0,
        };
        let row = form_row(&self.form, &self.names, item, index, has_next)?;
        self.next += 1;
        Ok(Some(row))
    }

    /// No more rows: the cursor closes, and the rows sent count as the form-list's.
    fn end(&mut self) {
        if self.ended {
            return;
        }
        self.ended = true;
        if let RowSource::Cursor(cursor) = &self.source {
            cursor.close();
        }
        note_form_list_rows(self.next);
    }
}

/// `sri.startFormListRow`: a frame holding the row and `_formMap`; false, with no frame, past the last row. A list sent
/// a row at a time makes the row now.
///
/// # Errors
///
/// Returns an error for a list that was closed, a streamed row asked for out of order, or a row whose actions fail.
pub fn start_form_list_row(state: &RenderState, handle: usize, index: usize) -> Result<bool, Error> {
    // a streamed row runs actions and scripts, so the stream is out of the render's lists while it does
    let streamed = state
        .form_lists
        .borrow_mut()
        .get_mut(handle)
        .and_then(Option::as_mut)
        .and_then(|list| list.stream.take());
    if let Some(mut stream) = streamed {
        let row = stream.row(index);
        if let Some(Some(list)) = state.form_lists.borrow_mut().get_mut(handle) {
            list.stream = Some(stream);
        }
        return Ok(match row? {
            Some(row) => {
                form_map_frame(row).keep();
                true
            }
            None => false,
        });
    }
    let past_end = state
        .form_lists
        .borrow()
        .get(handle)
        .and_then(Option::as_ref)
        .is_some_and(|list| index >= list.rows.len());
    if past_end {
        return Ok(false);
    }
    form_list_row_frame(state, handle, index).map(ContextFrame::keep)?;
    Ok(true)
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

/// `sri.safeCloseList`: drop a form-list's rows; a streamed one's cursor closes.
pub fn close_form_list(state: &RenderState, handle: usize) {
    let closed = state.form_lists.borrow_mut().get_mut(handle).and_then(Option::take);
    if let Some(mut stream) = closed.and_then(|list| list.stream) {
        stream.end();
    }
}
