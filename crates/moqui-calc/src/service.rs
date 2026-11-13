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

//! `org.moqui.calc.CalcServices`: open a CalcGrid document into a room, read its cells a viewport at a time, refresh
//! its data, save it, export it and close it (plans/IronCalc-DB.md, Phase 2).
//!
//! A document is a report definition of type `calcgrid` (`org.moqui.report.ReportServices`), so its sharing, versions
//! and git sync are the definitions'. Its body is `dataSheets` (each a `name` and the data grid's spec: `source`,
//! `columns`, `sort`, `filter`), `charts`, and `overlay`, the workbook's stored form in base64: the formulas, styles
//! and sheets of the author, with no row of data ([`crate::fill::stored_form`]).
//!
//! The data sheets are read through the data grid's reader ([`moqui_tools::grid`]) as the caller, with its refusals
//! (restricted entities, password fields). These services are the Tools app's: every one needs the Tools app, and a
//! SQL source needs SQL, whatever calls it. A refusal is a message error and no out-parameters.

use std::collections::BTreeMap;

use base64::Engine;
use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;
use moqui_core::service::call::{sync_checked, CallOptions};
use moqui_core::workbook;
use moqui_lib::error::Error as LibError;
use moqui_lib::json::{from_json, to_json};
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_tools::audit::Actor;
use moqui_tools::grid::{self, Source};
use serde_json::{json, Value as Json};

use crate::fill::{self, CellData, SheetRows};
use crate::room::{DataSheet, Opening, Room, Rooms};

/// The most data rows one data sheet takes; a source with more is cut and the sheet is marked.
pub const MAX_DATA_ROWS: usize = 100_000;
/// The largest stored overlay, in bytes.
pub const MAX_OVERLAY_BYTES: usize = 8 * 1024 * 1024;
/// The largest workbook file opened as a document.
const MAX_WORKBOOK_BYTES: usize = 32 * 1024 * 1024;
const PAGE: usize = grid::MAX_PAGE_SIZE;
const DEFINITION_TYPE: &str = "calcgrid";

type Out = Result<BTreeMap<String, Value>, LibError>;

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, open_calc_document)?;
    rust_service!(map, get_calc_cells)?;
    rust_service!(map, refresh_calc_document)?;
    rust_service!(map, save_calc_document)?;
    rust_service!(map, export_calc_document)?;
    rust_service!(map, close_calc_document)?;
    Ok(())
}

fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

/// A refusal: the message as an error of the call, and no out-parameters.
fn refused(message: impl Into<String>) -> Out {
    add_error(message);
    Ok(BTreeMap::new())
}

fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

fn int(parameters: &BTreeMap<String, Value>, key: &str, default: i32) -> i32 {
    match parameters.get(key) {
        Some(Value::I32(number)) => *number,
        Some(Value::I64(number)) => i32::try_from(*number).unwrap_or(default),
        Some(value) if !is_empty(value) => to_plain_string(value).parse().unwrap_or(default),
        _ => default,
    }
}

/// Days since Excel's day zero, 1899-12-30.
fn excel_date(date: jiff::civil::Date) -> f64 {
    jiff::civil::date(1899, 12, 30)
        .until(date)
        .map_or(0.0, |span| f64::from(span.get_days()))
}

fn excel_time(time: jiff::civil::Time) -> f64 {
    (f64::from(time.hour()) * 3600.0 + f64::from(time.minute()) * 60.0 + f64::from(time.second())) / 86_400.0
}

/// A value of a grid row as a cell: typed, a time stamp in the user's time zone.
fn cell_data(value: &Value, zone: &jiff::tz::TimeZone) -> CellData {
    match value {
        Value::Null => CellData::Empty,
        Value::String(text) if text.is_empty() => CellData::Empty,
        Value::String(text) => CellData::Text(text.clone()),
        Value::Bool(flag) => CellData::Bool(*flag),
        Value::I32(number) => CellData::Number(f64::from(*number)),
        #[allow(clippy::cast_precision_loss)]
        Value::I64(number) => CellData::Number(*number as f64),
        Value::F64(number) => CellData::Number(*number),
        Value::Decimal(number) => {
            use moqui_lib::format::ToPrimitive;
            number
                .to_f64()
                .map_or_else(|| CellData::Text(number.to_string()), CellData::Number)
        }
        Value::Date(date) => CellData::Date(excel_date(*date)),
        Value::Time(time) => CellData::Time(excel_time(*time)),
        Value::Timestamp(stamp) => {
            let local = stamp.to_zoned(zone.clone()).datetime();
            CellData::DateTime(excel_date(local.date()) + excel_time(local.time()))
        }
        Value::Bytes(_) => CellData::Empty,
        other => CellData::Text(to_plain_string(other)),
    }
}

/// Whether the caller may read `source` here: the Tools app, and SQL for a query. An error is added when not.
fn may_read(source: &Source) -> bool {
    if source.is_sql() {
        moqui_tools::sql_service::may_run_sql()
    } else {
        moqui_tools::service::may_view_tools()
    }
}

/// The rows of a data sheet, read through the data grid's reader as the caller, a page at a time, up to
/// [`MAX_DATA_ROWS`].
fn read_sheet(facade: &EntityFacade, sheet: &DataSheet) -> Result<SheetRows, String> {
    let spec = from_json(&sheet.spec);
    let part = |key: &str| match &spec {
        Value::Map(map) => map.get(key).cloned(),
        _ => None,
    };
    let source = Source::parse(part("source").as_ref(), facade).map_err(|refusal| refusal.0)?;
    if !may_read(&source) {
        return Err(format!("The source of {} is not available", sheet.name));
    }
    let requested: Vec<String> = match part("columns") {
        Some(Value::List(names)) => names
            .iter()
            .map(to_plain_string)
            .filter(|name| !name.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let sort = grid::parse_sort(part("sort").as_ref());
    let filter = grid::parse_filter(part("filter").as_ref());
    let actor = Actor::current();
    let columns: Vec<String> = grid::columns(facade, &source, &requested, &actor)
        .map_err(|refusal| refusal.0)?
        .columns
        .into_iter()
        .filter(|column| !column.encrypted && column.kind != "binary")
        .map(|column| column.name)
        .collect();
    if columns.is_empty() {
        return Err(format!("The source of {} has no column to show", sheet.name));
    }
    let zone = moqui_core::l10n::time_zone();
    let mut rows = Vec::new();
    let mut truncated = false;
    let mut page = 1;
    loop {
        let found =
            grid::rows(facade, &source, &columns, page, PAGE, &sort, &filter, &actor).map_err(|refusal| refusal.0)?;
        let count = found.data.len();
        for row in found.data {
            let Value::Map(map) = row else { continue };
            rows.push(
                columns
                    .iter()
                    .map(|name| map.get(name).map_or(CellData::Empty, |value| cell_data(value, &zone)))
                    .collect(),
            );
        }
        if count < PAGE {
            break;
        }
        if rows.len() >= MAX_DATA_ROWS {
            rows.truncate(MAX_DATA_ROWS);
            truncated = true;
            break;
        }
        page += 1;
    }
    Ok(SheetRows {
        name: sheet.name.clone(),
        columns,
        rows,
        truncated,
    })
}

fn read_sheets(facade: &EntityFacade, sheets: &[DataSheet]) -> Result<Vec<SheetRows>, String> {
    sheets.iter().map(|sheet| read_sheet(facade, sheet)).collect()
}

/// A name a sheet takes, from a source's: no `[]:*?/\`, at most 31 characters.
fn sheet_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if "[]:*?/\\'".contains(ch) { '_' } else { ch })
        .take(31)
        .collect();
    if cleaned.trim().is_empty() {
        "Data".to_string()
    } else {
        cleaned.trim().to_string()
    }
}

/// The name of the data sheet of a new document over `spec`: an entity's short name, a file's, or Data.
fn source_sheet_name(spec: &Json) -> String {
    let source = &spec["source"];
    let name = match source["type"].as_str() {
        Some("sql") => "Data",
        Some("file") => source["sheet"]
            .as_str()
            .filter(|sheet| !sheet.is_empty())
            .or_else(|| {
                source["location"]
                    .as_str()
                    .and_then(|location| location.rsplit('/').next())
            })
            .unwrap_or("Data"),
        _ => source["entityName"]
            .as_str()
            .and_then(|name| name.rsplit('.').next())
            .unwrap_or("Data"),
    };
    sheet_name(name)
}

fn call(name: &str, parameters: BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    sync_checked(name, &parameters, &CallOptions::default())
}

/// The saved document `definition_id`, as its definition: `id`, `title`, `version`, `body`.
fn read_definition(definition_id: &str) -> Result<Json, Error> {
    let mut parameters = BTreeMap::new();
    parameters.insert("definitionId".to_string(), Value::string(definition_id));
    let out = call("org.moqui.report.ReportServices.get#Definition", parameters)?;
    let definition = out.get("definition").map(to_json).unwrap_or(Json::Null);
    if definition["type"] != DEFINITION_TYPE {
        return Err(Error::new(format!("{definition_id} is not a CalcGrid document")));
    }
    Ok(definition)
}

fn data_sheets(body: &Json) -> Vec<DataSheet> {
    body["dataSheets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|sheet| {
            let name = sheet["name"].as_str().filter(|name| !name.is_empty())?;
            Some(DataSheet {
                name: name.to_string(),
                spec: sheet.clone(),
            })
        })
        .collect()
}

/// The room of the call's `roomId`, the caller's own.
fn room_of(parameters: &BTreeMap<String, Value>) -> Option<crate::room::SharedRoom> {
    let user_id = moqui_core::user::current_user_id().ok().flatten()?;
    let rooms = ExecutionContext::with_current(|ec| Rooms::of(ec.factory()))?;
    let room = rooms.get(&user_id, &text(parameters, "roomId"));
    if room.is_none() {
        // the view opens the document again on this error
        add_error("ROOM_GONE: this document is no longer open on the server; open it again");
    }
    room
}

/// What a view gets of an open room.
fn room_out(room: &Room) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    out.insert("roomId".to_string(), Value::string(&room.id));
    out.insert("title".to_string(), Value::string(&room.title));
    out.insert(
        "definitionId".to_string(),
        room.definition_id.as_ref().map_or(Value::Null, Value::string),
    );
    out.insert("version".to_string(), room.version.map_or(Value::Null, Value::I64));
    out.insert("sheets".to_string(), from_json(&room.layout()));
    out.insert("charts".to_string(), from_json(&room.charts()));
    out.insert(
        "evaluateMs".to_string(),
        Value::F64((room.evaluate_ms * 10.0).round() / 10.0),
    );
    out.insert(
        "truncated".to_string(),
        Value::List(room.truncated.iter().map(Value::string).collect()),
    );
    out.insert(
        "maxDataRows".to_string(),
        Value::I64(i64::try_from(MAX_DATA_ROWS).unwrap_or(i64::MAX)),
    );
    out
}

/// A workbook file as a model: the engine's reader for an xlsx it takes, else the cells of each sheet as values
/// (`read#Spreadsheet`'s reader: xls, xlsb, ods, and an xlsx without the parts the engine needs).
fn workbook_model(bytes: &[u8], name: &str) -> Result<moqui_core::workbook::ironcalc::base::Model<'static>, Error> {
    match workbook::open_xlsx(bytes, name) {
        Ok(model) => Ok(model),
        Err(engine_error) => {
            let first = moqui_core::service::spreadsheet::read_sheet_bytes(bytes, "", 0).map_err(|_| engine_error)?;
            let mut writer = workbook::WorkbookWriter::new()?;
            let plain = writer.style(&workbook::CellStyle::default());
            let zone = moqui_core::l10n::time_zone();
            for (index, sheet_name) in first.sheet_names.iter().enumerate() {
                let data = if index == 0 {
                    first.clone()
                } else {
                    moqui_core::service::spreadsheet::read_sheet_bytes(bytes, sheet_name, index)
                        .map_err(|err| Error::new(err.to_string()))?
                };
                let sheet = writer.add_sheet(sheet_name)?;
                for (row, number) in data.rows.iter().zip(&data.row_numbers) {
                    let row_number = i32::try_from(*number).map_err(|_| Error::new("Too many rows for a sheet"))?;
                    for (column, value) in row.iter().enumerate() {
                        let column = i32::try_from(column + 1).map_err(|_| Error::new("Too many columns"))?;
                        match cell_data(value, &zone) {
                            CellData::Empty => {}
                            CellData::Text(found) => writer.text(sheet, row_number, column, &found, plain)?,
                            CellData::Bool(flag) => {
                                writer.text(sheet, row_number, column, if flag { "TRUE" } else { "FALSE" }, plain)?;
                            }
                            CellData::Number(found)
                            | CellData::Date(found)
                            | CellData::Time(found)
                            | CellData::DateTime(found) => writer.number(sheet, row_number, column, found, plain)?,
                        }
                    }
                }
                writer.fit_columns(sheet)?;
            }
            writer.into_model()
        }
    }
}

/// `open#CalcDocument`: a saved document (`definitionId`), a new one over a data grid source (`source`, with its
/// `columns`, `sort` and `filter`), or a workbook file (`workbookLocation`), opened into a room of the caller's.
fn open_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !moqui_tools::service::may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let Some(user_id) = moqui_core::user::current_user_id().ok().flatten() else {
        return refused("Not signed in");
    };
    let definition_id = text(parameters, "definitionId");
    let workbook_location = text(parameters, "workbookLocation");
    let (base, sheets, charts, title, version) = if !definition_id.is_empty() {
        let definition = match read_definition(&definition_id) {
            Ok(definition) => definition,
            Err(err) => return refused(err.message()),
        };
        let body = &definition["body"];
        let base = match body["overlay"].as_str().filter(|overlay| !overlay.is_empty()) {
            Some(overlay) => {
                let bytes = match base64::engine::general_purpose::STANDARD.decode(overlay) {
                    Ok(bytes) => bytes,
                    Err(_) => return refused("The document's workbook is not readable"),
                };
                fill::open_stored(&bytes)
            }
            None => fill::new_model("Sheet1"),
        };
        let charts = body["charts"].as_array().cloned().unwrap_or_default();
        let title = definition["title"].as_str().unwrap_or(&definition_id).to_string();
        (base, data_sheets(body), charts, title, definition["version"].as_i64())
    } else if !workbook_location.is_empty() {
        // from the places a file source may be read (the user's uploads, the import places), one vague refusal otherwise
        let bytes = match moqui_tools::etl::access::read_bytes(&workbook_location) {
            Ok(bytes) => bytes,
            Err(err) => return refused(err.to_string()),
        };
        if bytes.len() > MAX_WORKBOOK_BYTES {
            return refused(format!(
                "A workbook to open has at most {} MB",
                MAX_WORKBOOK_BYTES / 1024 / 1024
            ));
        }
        let name = workbook_location.rsplit('/').next().unwrap_or("Workbook").to_string();
        (workbook_model(&bytes, &name), Vec::new(), Vec::new(), name, None)
    } else if parameters.get("source").is_some_and(|source| !is_empty(source)) {
        let mut spec = json!({});
        for key in ["source", "columns", "sort", "filter"] {
            if let Some(value) = parameters.get(key).filter(|value| !is_empty(value)) {
                spec[key] = to_json(value);
            }
        }
        let name = source_sheet_name(&spec);
        spec["name"] = Json::String(name.clone());
        let title = text(parameters, "title");
        let sheet = DataSheet {
            name: name.clone(),
            spec,
        };
        // the data sheet is the workbook's first; the author adds sheets beside it
        (
            fill::new_model(&name),
            vec![sheet],
            Vec::new(),
            if title.is_empty() { name } else { title },
            None,
        )
    } else {
        return refused("A document needs a definitionId, a source or a workbookLocation");
    };
    let base = match base {
        Ok(base) => base,
        Err(err) => return refused(err.message()),
    };
    let rows = match read_sheets(facade, &sheets) {
        Ok(rows) => rows,
        Err(message) => return refused(message),
    };
    let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) else {
        return refused("no execution context is entered");
    };
    let opening = Opening {
        user_id,
        definition_id: Some(definition_id).filter(|id| !id.is_empty()),
        version,
        title,
        data_sheets: sheets,
        charts,
        base,
        rows,
    };
    match rooms.open(opening) {
        Ok(room) => Ok(room_out(&moqui_lib::sync::lock(&room))),
        Err(err) => refused(err.message()),
    }
}

/// `get#CalcCells`: a rectangle of a sheet's cells, as the engine's block in JSON text (`blockJson`).
fn get_calc_cells(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !moqui_tools::service::may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let Some(room) = room_of(parameters) else {
        return Ok(BTreeMap::new());
    };
    let room = moqui_lib::sync::lock(&room);
    let sheet = u32::try_from(int(parameters, "sheet", 0)).unwrap_or(0);
    match room.block(
        sheet,
        int(parameters, "row", 1),
        int(parameters, "column", 1),
        int(parameters, "width", 1),
        int(parameters, "height", 1),
    ) {
        Ok(block) => {
            let mut out = BTreeMap::new();
            out.insert("blockJson".to_string(), Value::String(block));
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// `refresh#CalcDocument`: the data sheets read again as the caller, the workbook evaluated.
fn refresh_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !moqui_tools::service::may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let Some(room) = room_of(parameters) else {
        return Ok(BTreeMap::new());
    };
    let sheets = moqui_lib::sync::lock(&room).data_sheets.clone();
    // read with the room unlocked: a viewport of it is still answered meanwhile
    let rows = match read_sheets(facade, &sheets) {
        Ok(rows) => rows,
        Err(message) => return refused(message),
    };
    let mut room = moqui_lib::sync::lock(&room);
    match room.refresh(rows) {
        Ok(()) => Ok(room_out(&room)),
        Err(err) => refused(err.message()),
    }
}

/// `save#CalcDocument`: the room's document saved as a report definition of type `calcgrid`: a new one under
/// `definitionId` (or an id made from the title), or the one the room was opened from, at the version it was read.
fn save_calc_document(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !moqui_tools::service::may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let Some(room) = room_of(parameters) else {
        return Ok(BTreeMap::new());
    };
    let mut room = moqui_lib::sync::lock(&room);
    let overlay = match room.stored_form() {
        Ok(overlay) => overlay,
        Err(err) => return refused(err.message()),
    };
    if overlay.len() > MAX_OVERLAY_BYTES {
        return refused(format!(
            "The workbook without its data is {} MB; a document stores at most {} MB",
            overlay.len() / 1024 / 1024,
            MAX_OVERLAY_BYTES / 1024 / 1024
        ));
    }
    let title = Some(text(parameters, "title"))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| room.title.clone());
    let given = text(parameters, "definitionId");
    let (id, version) = match (&room.definition_id, given.is_empty()) {
        (Some(id), true) => (id.clone(), room.version),
        (Some(id), false) if *id == given => (given, room.version),
        (_, false) => (given, None),
        (None, true) => {
            let slug: String = title
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() {
                        ch.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            (
                format!("calc-{}-{}", slug.trim_matches('-'), &room.id[1..room.id.len().min(9)]),
                None,
            )
        }
    };
    let mut definition = json!({
        "id": id,
        "type": DEFINITION_TYPE,
        "title": title,
        "body": {
            "dataSheets": room.data_sheets.iter().map(|sheet| sheet.spec.clone()).collect::<Vec<Json>>(),
            "charts": room.charts,
            "overlay": base64::engine::general_purpose::STANDARD.encode(&overlay),
        },
    });
    if let Some(version) = version {
        definition["version"] = json!(version);
    }
    let group = text(parameters, "userGroupId");
    if !group.is_empty() {
        definition["userGroupId"] = Json::String(group);
    }
    let mut call_parameters = BTreeMap::new();
    call_parameters.insert("definition".to_string(), from_json(&definition));
    call_parameters.insert("note".to_string(), Value::string(text(parameters, "note")));
    let saved = match call("org.moqui.report.ReportServices.save#Definition", call_parameters) {
        Ok(out) => out.get("definition").map(to_json).unwrap_or(Json::Null),
        Err(err) => return refused(err.message()),
    };
    room.definition_id = Some(id.clone());
    room.version = saved["version"].as_i64();
    room.title = title;
    let mut out = BTreeMap::new();
    out.insert("definitionId".to_string(), Value::String(id));
    out.insert("version".to_string(), room.version.map_or(Value::Null, Value::I64));
    out.insert("title".to_string(), Value::string(&room.title));
    Ok(out)
}

/// `export#CalcDocument`: the workbook as it is in the room, data and calculated values, as an `.xlsx`.
fn export_calc_document(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    if !moqui_tools::service::may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let Some(room) = room_of(parameters) else {
        return Ok(BTreeMap::new());
    };
    let room = moqui_lib::sync::lock(&room);
    match workbook::model_xlsx(room.model()) {
        Ok(bytes) => {
            let name: String = room
                .title
                .chars()
                .map(|ch| {
                    if ch.is_alphanumeric() || " -_.".contains(ch) {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect();
            let mut out = BTreeMap::new();
            out.insert("contentBytes".to_string(), Value::Bytes(bytes));
            out.insert("filename".to_string(), Value::String(format!("{}.xlsx", name.trim())));
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// `close#CalcDocument`: drop the caller's room.
fn close_calc_document(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let user_id = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    if let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) {
        rooms.close(&user_id, &text(parameters, "roomId"));
    }
    Ok(BTreeMap::new())
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
