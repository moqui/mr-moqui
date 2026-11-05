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

//! The data grid's services of `org.moqui.impl.DatabaseServices` (plans/Tools-Database.md §9), on [`crate::grid`]. An
//! entity source needs the Tools app; a SQL source needs SQL (`SQL_RUNNER_WEB` and all actions on the Tools app). A
//! refusal is a message error and no out-parameters.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

use crate::audit::Actor;
use crate::grid::{self, Source};
use crate::service::{add_error, flag, may_view_tools, text};
use crate::sql_service::{current_user, may_run_sql, number};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_data_grid_columns)?;
    rust_service!(map, get_data_grid_rows)?;
    rust_service!(map, count_data_grid_rows)?;
    rust_service!(map, export_data_grid)?;
    rust_service!(map, get_data_grid_lookup)?;
    rust_service!(map, save_data_grid_changes)?;
    rust_service!(map, find_grid_views)?;
    rust_service!(map, store_grid_view)?;
    rust_service!(map, delete_grid_view)?;
    Ok(())
}

const GRID_VIEW: &str = "moqui.database.GridView";

type Out = Result<BTreeMap<String, Value>, LibError>;

fn none() -> Out {
    Ok(BTreeMap::new())
}

/// The source of the call, when the user may use it: the Tools app for an entity, SQL for a query.
fn source_of(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Source> {
    let source = match Source::parse(parameters.get("source"), facade) {
        Ok(source) => source,
        Err(refusal) => {
            add_error(refusal.0);
            return None;
        }
    };
    let allowed = if source.is_sql() {
        may_run_sql()
    } else {
        may_view_tools()
    };
    allowed.then_some(source)
}

fn names(parameters: &BTreeMap<String, Value>, key: &str) -> Vec<String> {
    match parameters.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|name| !name.is_empty())
            .collect(),
        Some(Value::String(text)) => text
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn int(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `get#DataGridColumns`.
fn get_data_grid_columns(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(source) = source_of(parameters, facade) else {
        return none();
    };
    match grid::columns(facade, &source, &names(parameters, "columns"), &Actor::current()) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert(
                "columns".to_string(),
                Value::List(outcome.columns.iter().map(grid::Column::to_value).collect()),
            );
            out.insert(
                "entityName".to_string(),
                outcome.entity_name.map_or(Value::Null, Value::String),
            );
            out.insert(
                "readOnlyReason".to_string(),
                outcome.read_only_reason.map_or(Value::Null, Value::String),
            );
            out.insert("hasStamp".to_string(), Value::Bool(outcome.has_stamp));
            out.insert(
                "keyColumns".to_string(),
                Value::List(outcome.key_columns.into_iter().map(Value::String).collect()),
            );
            out.insert(
                "stampColumn".to_string(),
                outcome.stamp_column.map_or(Value::Null, Value::String),
            );
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `get#DataGridRows`: one page, Tabulator's `{last_page, last_row, data}`.
fn get_data_grid_rows(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(source) = source_of(parameters, facade) else {
        return none();
    };
    let size = match number(parameters, "size") {
        0 => 100,
        size => size,
    };
    match grid::rows(
        facade,
        &source,
        &names(parameters, "columns"),
        number(parameters, "page").max(1),
        size,
        &grid::parse_sort(parameters.get("sort")),
        &grid::parse_filter(parameters.get("filter")),
        &Actor::current(),
    ) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert("last_page".to_string(), int(outcome.last_page));
            out.insert("last_row".to_string(), outcome.last_row.map_or(Value::Null, Value::I64));
            out.insert("estimated".to_string(), Value::Bool(outcome.estimated));
            out.insert("data".to_string(), Value::List(outcome.data));
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `count#DataGridRows`.
fn count_data_grid_rows(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(source) = source_of(parameters, facade) else {
        return none();
    };
    match grid::count(
        facade,
        &source,
        &grid::parse_filter(parameters.get("filter")),
        &Actor::current(),
    ) {
        Ok(count) => {
            let mut out = BTreeMap::new();
            out.insert("count".to_string(), Value::I64(count));
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `export#DataGrid`.
fn export_data_grid(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(source) = source_of(parameters, facade) else {
        return none();
    };
    let format = match text(parameters, "format").to_ascii_lowercase().as_str() {
        "" | "csv" => grid::ExportFormat::Csv,
        "xlsx" => grid::ExportFormat::Xlsx,
        other => {
            add_error(format!("format {other} is neither csv nor xlsx"));
            return none();
        }
    };
    let location = text(parameters, "location");
    let actor = Actor::current();
    let admin = moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false);
    if !location.is_empty() && !grid::export_location_allowed(&location, &actor, admin) {
        add_error(
            "An export is written under your own runtime://tmp/grid-export/<userId>/ or \
             dbresource://moqui/database/export/<userId>/; another place needs DATABASE_ADMIN",
        );
        return none();
    }
    match grid::export(
        facade,
        &source,
        &names(parameters, "columns"),
        &grid::parse_sort(parameters.get("sort")),
        &grid::parse_filter(parameters.get("filter")),
        format,
        &location,
        &actor,
    ) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert("location".to_string(), Value::String(outcome.location));
            out.insert("fileName".to_string(), Value::String(outcome.file_name));
            out.insert("contentType".to_string(), Value::string(outcome.content_type));
            out.insert("rowCount".to_string(), int(outcome.row_count));
            out.insert("truncated".to_string(), Value::Bool(outcome.truncated));
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `get#DataGridLookup`.
fn get_data_grid_lookup(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_view_tools() {
        return none();
    }
    match grid::lookup(
        facade,
        &text(parameters, "entityName"),
        &text(parameters, "field"),
        &text(parameters, "term"),
        i64::try_from(number(parameters, "offset")).unwrap_or(0),
    ) {
        Ok(options) => {
            let mut out = BTreeMap::new();
            out.insert(
                "options".to_string(),
                Value::List(
                    options
                        .into_iter()
                        .map(|option| {
                            let mut map = IndexMap::new();
                            map.insert("value".to_string(), option.value);
                            map.insert("label".to_string(), Value::String(option.label));
                            Value::Map(map)
                        })
                        .collect(),
                ),
            );
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

/// `save#DataGridChanges`.
fn save_data_grid_changes(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_view_tools() {
        return none();
    }
    let entity_name = text(parameters, "entityName");
    if entity_name.is_empty() {
        add_error("entityName is required");
        return none();
    }
    match grid::save(
        facade,
        &entity_name,
        &grid::Changes::parse(parameters),
        &Actor::current(),
    ) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert("saved".to_string(), Value::Bool(outcome.saved));
            out.insert(
                "refused".to_string(),
                Value::List(outcome.refused.iter().map(grid::RefusedRow::to_value).collect()),
            );
            out.insert("updated".to_string(), int(outcome.updated));
            out.insert("created".to_string(), int(outcome.created));
            out.insert("deleted".to_string(), int(outcome.deleted));
            out.insert(
                "createdKeys".to_string(),
                Value::List(
                    outcome
                        .created_keys
                        .into_iter()
                        .map(|key| Value::Map(key.into_iter().collect()))
                        .collect(),
                ),
            );
            Ok(out)
        }
        Err(refusal) => {
            add_error(refusal.0);
            none()
        }
    }
}

fn view_value(row: &moqui_core::entity::EntityValue) -> Value {
    let mut entry = IndexMap::new();
    for field in [
        "viewId",
        "description",
        "ownerUserId",
        "isShared",
        "sourceJson",
        "layoutJson",
    ] {
        entry.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
    }
    Value::Map(entry)
}

/// `find#GridViews`: the user's own grid views and the shared ones, by description.
fn find_grid_views(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_view_tools() {
        return none();
    }
    let user = current_user().unwrap_or_default();
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(GRID_VIEW)?
        .condition(Condition::Or(vec![
            Condition::equals("ownerUserId", Value::String(user)),
            Condition::equals("isShared", Value::string("Y")),
        ]))
        .order_by("description", false)
        .list()?;
    let mut out = BTreeMap::new();
    out.insert(
        "gridViews".to_string(),
        Value::List(rows.iter().map(view_value).collect()),
    );
    Ok(out)
}

/// The grid view `viewId` when the current user owns it; an error added when not.
fn owned_view(
    facade: &EntityFacade,
    view_id: &str,
    user: &str,
) -> Result<Option<moqui_core::entity::EntityValue>, LibError> {
    match facade.find(GRID_VIEW)?.eq("viewId", Value::string(view_id)).one()? {
        Some(row) if row.get("ownerUserId").map(to_plain_string).as_deref() == Some(user) => Ok(Some(row)),
        Some(_) => {
            add_error("That grid view is another user's");
            Ok(None)
        }
        None => {
            add_error(format!("No grid view {view_id}"));
            Ok(None)
        }
    }
}

fn json_or_error(parameters: &BTreeMap<String, Value>, key: &str) -> Option<Value> {
    let text = text(parameters, key);
    if text.is_empty() {
        return Some(Value::Null);
    }
    if serde_json::from_str::<serde_json::Value>(&text).is_err() {
        add_error(format!("{key} is not JSON"));
        return None;
    }
    Some(Value::String(text))
}

/// `store#GridView`: create a grid view (no `viewId`), owned by the current user, or update one the user owns.
fn store_grid_view(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_view_tools() {
        return none();
    }
    let Some(user) = current_user() else {
        add_error("Grid views are a signed-in user's");
        return none();
    };
    let (Some(source), Some(layout)) = (
        json_or_error(parameters, "sourceJson"),
        json_or_error(parameters, "layoutJson"),
    ) else {
        return none();
    };
    if matches!(source, Value::Null) {
        add_error("sourceJson is required");
        return none();
    }
    let view_id = text(parameters, "viewId");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = if view_id.is_empty() {
        let mut row = facade.make_value(GRID_VIEW)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("ownerUserId", Value::string(&user));
        row
    } else {
        match owned_view(facade, &view_id, &user)? {
            Some(row) => row,
            None => return none(),
        }
    };
    row.set("description", Value::String(text(parameters, "description")));
    row.set(
        "isShared",
        Value::string(if flag(parameters, "isShared") { "Y" } else { "N" }),
    );
    row.set("sourceJson", source);
    row.set("layoutJson", layout);
    if view_id.is_empty() {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    let mut out = BTreeMap::new();
    out.insert("viewId".to_string(), row.get("viewId").cloned().unwrap_or(Value::Null));
    Ok(out)
}

/// `delete#GridView`: delete a grid view the user owns.
fn delete_grid_view(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !may_view_tools() {
        return none();
    }
    let Some(user) = current_user() else {
        add_error("Grid views are a signed-in user's");
        return none();
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if let Some(row) = owned_view(facade, &text(parameters, "viewId"), &user)? {
        row.delete(facade)?;
    }
    none()
}
