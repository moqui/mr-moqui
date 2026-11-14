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
//! (restricted entities, password fields). A call is made one of two ways ([`Access`]). With no `profileId` it is the
//! Tools app's: it needs the Tools app, and a SQL source needs SQL, whatever calls it. With a `profileId` it is under
//! that access profile ([`crate::profile`]), which the caller must hold: only entities and views the profile offers
//! are read, with the entity filters of the profile's grant, and no SQL or file. A room is read only the way it was
//! opened. A refusal is a message error and no out-parameters.

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
use crate::profile::{self, Profile};
use crate::room::{DataSheet, Opening, Room, Rooms, SharedRoom, REOPEN};
use crate::together::{self, Presence};

/// The functions a formula being typed completes to, in the browser and the terminal: the engine's list is not public,
/// so these are the common ones it has.
pub const FUNCTIONS: &[&str] = &[
    "ABS",
    "AND",
    "AVERAGE",
    "AVERAGEIF",
    "AVERAGEIFS",
    "CEILING",
    "CHOOSE",
    "CONCAT",
    "CONCATENATE",
    "COUNT",
    "COUNTA",
    "COUNTBLANK",
    "COUNTIF",
    "COUNTIFS",
    "DATE",
    "DATEDIF",
    "DAY",
    "DAYS",
    "EDATE",
    "EOMONTH",
    "EXACT",
    "FILTER",
    "FIND",
    "FLOOR",
    "HLOOKUP",
    "HOUR",
    "IF",
    "IFERROR",
    "IFNA",
    "IFS",
    "INDEX",
    "INT",
    "ISBLANK",
    "ISERROR",
    "ISNUMBER",
    "ISTEXT",
    "LARGE",
    "LEFT",
    "LEN",
    "LOWER",
    "MATCH",
    "MAX",
    "MAXIFS",
    "MEDIAN",
    "MID",
    "MIN",
    "MINIFS",
    "MINUTE",
    "MOD",
    "MONTH",
    "NETWORKDAYS",
    "NOT",
    "NOW",
    "OR",
    "POWER",
    "PRODUCT",
    "PROPER",
    "RAND",
    "RANDBETWEEN",
    "RANK",
    "REPLACE",
    "RIGHT",
    "ROUND",
    "ROUNDDOWN",
    "ROUNDUP",
    "ROWS",
    "SEARCH",
    "SECOND",
    "SMALL",
    "SORT",
    "SQRT",
    "STDEV",
    "SUBSTITUTE",
    "SUBTOTAL",
    "SUM",
    "SUMIF",
    "SUMIFS",
    "SUMPRODUCT",
    "SWITCH",
    "TEXT",
    "TEXTJOIN",
    "TIME",
    "TODAY",
    "TRIM",
    "UNIQUE",
    "UPPER",
    "VALUE",
    "VLOOKUP",
    "WEEKDAY",
    "WEEKNUM",
    "XLOOKUP",
    "YEAR",
];

/// What a view gets when it opens a room: the room ([`room_out`]) and the function names it completes.
fn opened_out(facade: &EntityFacade, room: &Room, user_id: &str) -> BTreeMap<String, Value> {
    let mut out = room_out(facade, room, user_id);
    out.insert(
        "functions".to_string(),
        Value::List(FUNCTIONS.iter().map(|name| Value::string(*name)).collect()),
    );
    out
}

/// The most data rows one data sheet takes; a source with more is cut and the sheet is marked.
pub const MAX_DATA_ROWS: usize = 100_000;
/// The most edits of one `edit#CalcDocument` call.
const MAX_EDIT_OPS: usize = 2_000;
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
    rust_service!(map, get_calc_profiles)?;
    rust_service!(map, find_calc_entities)?;
    rust_service!(map, get_calc_frame)?;
    rust_service!(map, set_calc_profile)?;
    rust_service!(map, edit_calc_document)?;
    rust_service!(map, get_calc_cell_content)?;
    rust_service!(map, get_calc_scope)?;
    rust_service!(map, render_calc_document)?;
    rust_service!(map, revert_calc_document)?;
    rust_service!(map, get_calc_state)?;
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

/// How a call reaches the data: the Tools app's own access, or an access profile the caller holds.
pub(crate) enum Access {
    Tools,
    Profile(Profile),
}

impl Access {
    /// The access of a call, from its `profileId`. None, with an error added, when the caller has neither the Tools
    /// app (no `profileId`) nor the profile named.
    pub(crate) fn of(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Self> {
        let id = text(parameters, "profileId");
        if id.is_empty() {
            return moqui_tools::service::may_view_tools().then_some(Self::Tools);
        }
        match Profile::held(facade, &id) {
            Ok(profile) => Some(Self::Profile(profile)),
            Err(err) => {
                add_error(err.message());
                None
            }
        }
    }

    pub(crate) fn profile_id(&self) -> Option<String> {
        match self {
            Self::Tools => None,
            Self::Profile(profile) => Some(profile.id.clone()),
        }
    }

    /// The access of a call that names no profile and is not a screen's: a dashboard panel, a scheduled report. It is
    /// the caller's current one ([`PROFILE_PREFERENCE`], which the CalcGrid screen sets when a profile is chosen)
    /// while they still hold it, else the Tools app's own access when they have it, else the first profile they hold.
    /// None, with nothing added, when they have none.
    pub(crate) fn current(facade: &EntityFacade) -> Option<Self> {
        let tools = || {
            let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
            moqui_core::user::permitted_own("AT_XML_SCREEN", "component://tools/screen/Tools.xml", "view")
                .unwrap_or(false)
        };
        let chosen = moqui_core::user::get_preference(PROFILE_PREFERENCE)
            .ok()
            .flatten()
            .unwrap_or_default();
        if chosen == TOOLS_ACCESS {
            if tools() {
                return Some(Self::Tools);
            }
        } else if !chosen.is_empty() {
            if let Ok(profile) = Profile::held(facade, &chosen) {
                return Some(Self::Profile(profile));
            }
        }
        if tools() {
            return Some(Self::Tools);
        }
        let first = Profile::held_ids(facade).ok()?.into_iter().next()?;
        Profile::held(facade, &first).ok().map(Self::Profile)
    }
}

/// The user preference that keeps the access a person chose in CalcGrid: a profile's id, or [`TOOLS_ACCESS`].
pub const PROFILE_PREFERENCE: &str = "calc.profile";
/// The value of [`PROFILE_PREFERENCE`] for the Tools app's own access.
pub const TOOLS_ACCESS: &str = "tools";
/// How old a room's data may be for a report to read it without reading the sources again.
const FRAME_DATA_AGE: std::time::Duration = std::time::Duration::from_secs(60);
/// The most rows of a range a report takes (the report engine's own limit for a service's rows).
const MAX_FRAME_ROWS: usize = 10_000;

/// What a data sheet reads, checked as the caller ([`sheet_access`]): the source, the columns shown, its sort and
/// filter, the entity's full name when the source is an entity or view, and the profile entered while it is read.
pub(crate) struct SheetAccess {
    pub(crate) source: Source,
    pub(crate) columns: Vec<String>,
    pub(crate) entity: Option<String>,
    sort: Vec<grid::Sort>,
    filter: Vec<grid::Filter>,
    _entered: Option<profile::Entered>,
}

/// Whether the caller may read `sheet` under `access`, and what it reads: every check [`read_sheet`] makes before it
/// reads a row. Under a profile the source is an entity or view the profile offers, and the profile stays entered
/// while the answer is held, so finds made meanwhile get the entity filters of the profile's grant.
pub(crate) fn sheet_access(facade: &EntityFacade, sheet: &DataSheet, access: &Access) -> Result<SheetAccess, String> {
    let spec = from_json(&sheet.spec);
    let part = |key: &str| match &spec {
        Value::Map(map) => map.get(key).cloned(),
        _ => None,
    };
    let source = Source::parse(part("source").as_ref(), facade).map_err(|refusal| refusal.0)?;
    let entity = match &source {
        Source::Entity { entity_name, .. } => moqui_core::entity::info::entity_full_name(facade, entity_name),
        _ => None,
    };
    let entered = match access {
        Access::Tools => {
            let allowed = if source.is_sql() {
                moqui_tools::sql_service::may_run_sql()
            } else {
                moqui_tools::service::may_view_tools()
            };
            if !allowed {
                return Err(format!("The source of {} is not available", sheet.name));
            }
            None
        }
        Access::Profile(profile) => {
            let not_available = || format!("{}: Not available", sheet.name);
            let full = entity.clone().ok_or_else(not_available)?;
            if !profile.offers(facade, &full) {
                tracing::info!(profile = %profile.id, entity = %full, "Refused an entity a CalcGrid profile does not offer");
                return Err(not_available());
            }
            let entered = profile.enter().map_err(|_| not_available())?;
            profile.check_scoped(facade, &full).map_err(|_| not_available())?;
            Some(entered)
        }
    };
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
    Ok(SheetAccess {
        source,
        columns,
        entity,
        sort,
        filter,
        _entered: entered,
    })
}

/// The rows of a data sheet, read through the data grid's reader as the caller, a page at a time, up to
/// [`MAX_DATA_ROWS`], after the checks of [`sheet_access`].
fn read_sheet(facade: &EntityFacade, sheet: &DataSheet, access: &Access) -> Result<SheetRows, String> {
    let checked = sheet_access(facade, sheet, access)?;
    let (source, columns, sort, filter) = (&checked.source, &checked.columns, &checked.sort, &checked.filter);
    let actor = Actor::current();
    let zone = moqui_core::l10n::time_zone();
    let mut rows = Vec::new();
    let mut truncated = false;
    let mut page = 1;
    loop {
        let found =
            grid::rows(facade, source, columns, page, PAGE, sort, filter, &actor).map_err(|refusal| refusal.0)?;
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
        columns: checked.columns.clone(),
        rows,
        truncated,
    })
}

/// The rows of each data sheet. With `lenient` (a saved document, whose reader may hold less than its author) a sheet
/// that does not read is given as one cell saying so, and the others are read; otherwise the first refusal is the
/// answer.
fn read_sheets(
    facade: &EntityFacade,
    sheets: &[DataSheet],
    access: &Access,
    lenient: bool,
) -> Result<Vec<SheetRows>, String> {
    let mut all = Vec::with_capacity(sheets.len());
    for sheet in sheets {
        match read_sheet(facade, sheet, access) {
            Ok(rows) => all.push(rows),
            Err(message) if lenient => {
                tracing::info!(sheet = %sheet.name, reason = %message, "A CalcGrid data sheet is not available to its reader");
                all.push(SheetRows {
                    name: sheet.name.clone(),
                    columns: vec!["Not available".to_string()],
                    rows: Vec::new(),
                    truncated: false,
                });
            }
            Err(message) => return Err(message),
        }
    }
    Ok(all)
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

const ROOM_GONE: &str = "ROOM_GONE: this document is no longer open on the server; open it again";

/// The room of the call's `roomId`: one the caller has open, opened the way the call is made (the same profile, or
/// none), and brought up to its document's edit log ([`together::catch_up`]). A room that cannot follow its log is
/// dropped, and the view opens the document again.
fn room_of(parameters: &BTreeMap<String, Value>, access: &Access, facade: &EntityFacade) -> Option<SharedRoom> {
    let user_id = moqui_core::user::current_user_id().ok().flatten()?;
    let rooms = ExecutionContext::with_current(|ec| Rooms::of(ec.factory()))?;
    let id = text(parameters, "roomId");
    let room = rooms
        .get(&user_id, &id)
        .filter(|room| moqui_lib::sync::lock(room).profile_id == access.profile_id());
    let Some(room) = room else {
        // the view opens the document again on this error
        add_error(ROOM_GONE);
        return None;
    };
    let caught = together::catch_up(facade, &mut moqui_lib::sync::lock(&room));
    if let Err(err) = caught {
        tracing::info!(room = %id, error = %err, "A CalcGrid room is opened anew");
        rooms.drop_room(&id);
        add_error(ROOM_GONE);
        return None;
    }
    Some(room)
}

/// The caller's user id, for the rooms and the log.
fn user_id() -> String {
    moqui_core::user::current_user_id().ok().flatten().unwrap_or_default()
}

/// What a view gets of an open room, for `user_id`: its sheets and charts, and their own undo and redo.
fn room_out(facade: &EntityFacade, room: &Room, user_id: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    out.insert("roomId".to_string(), Value::string(&room.id));
    out.insert("shared".to_string(), Value::Bool(room.shared));
    out.insert("seq".to_string(), Value::I64(room.seq));
    out.insert("unsaved".to_string(), Value::I64(together::unsaved(facade, room)));
    out.insert(
        "members".to_string(),
        Value::I64(i64::try_from(room.member_ids().len()).unwrap_or(0)),
    );
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
        "memoryBytes".to_string(),
        Value::I64(i64::try_from(room.memory_bytes()).unwrap_or(i64::MAX)),
    );
    out.insert(
        "truncated".to_string(),
        Value::List(room.truncated.iter().map(Value::string).collect()),
    );
    let (can_undo, can_redo) = together::undo_state(facade, room, user_id);
    out.insert("canUndo".to_string(), Value::Bool(can_undo));
    out.insert("canRedo".to_string(), Value::Bool(can_redo));
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
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    open_document_with(parameters, facade, access)
}

/// [`open_calc_document`] under an access already decided.
fn open_document_with(parameters: &BTreeMap<String, Value>, facade: &EntityFacade, access: Access) -> Out {
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
    } else if matches!(access, Access::Profile(_)) && (!workbook_location.is_empty() || !profile::may_author()) {
        // under a profile a new document is over an entity the profile offers, and is its author's to make
        return refused("Not available");
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
    let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) else {
        return refused("no execution context is entered");
    };
    // a saved document is open in a room for the readers of one scope: joined when someone of the scope has it open
    let shared = if definition_id.is_empty() {
        None
    } else {
        let scope = crate::scope::fingerprint(facade, &sheets, &access);
        let id = crate::room::document_room_id(&definition_id, access.profile_id().as_deref(), &scope);
        if let Some(room) = rooms.join(&user_id, &id) {
            let caught = together::catch_up(facade, &mut moqui_lib::sync::lock(&room));
            if caught.is_ok() {
                return Ok(opened_out(facade, &moqui_lib::sync::lock(&room), &user_id));
            }
            rooms.drop_room(&id);
        }
        match together::opening_seq(facade, &definition_id, &user_id, version) {
            Ok(seq) => Some((id, seq)),
            Err(err) => return refused(err.message()),
        }
    };
    let rows = match read_sheets(facade, &sheets, &access, !definition_id.is_empty()) {
        Ok(rows) => rows,
        Err(message) => return refused(message),
    };
    let opening = Opening {
        user_id: user_id.clone(),
        shared,
        profile_id: access.profile_id(),
        definition_id: Some(definition_id).filter(|id| !id.is_empty()),
        version,
        title,
        data_sheets: sheets,
        charts,
        base,
        rows,
    };
    let room = match rooms.open(opening) {
        Ok(room) => room,
        Err(err) => return refused(err.message()),
    };
    let mut held = moqui_lib::sync::lock(&room);
    if let Err(err) = together::catch_up(facade, &mut held) {
        let id = held.id.clone();
        drop(held);
        rooms.drop_room(&id);
        return refused(err.message());
    }
    Ok(opened_out(facade, &held, &user_id))
}

/// `get#CalcCells`: a rectangle of a sheet's cells, as the engine's block in JSON text (`blockJson`), with the room's
/// last edit (`seq`). With `since` at that edit the block is left out (`unchanged`): a view that polls asks so. With a
/// `client`, the view's place (`selection`: `row`, `column`, `width`, `height`) is kept for the others, and the others
/// are answered (`peers`).
fn get_calc_cells(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let room = moqui_lib::sync::lock(&room);
    let sheet = u32::try_from(int(parameters, "sheet", 0)).unwrap_or(0);
    let mut out = BTreeMap::new();
    out.insert("seq".to_string(), Value::I64(room.seq));
    let client = text(parameters, "client");
    if !client.is_empty() {
        if let Some(presence) = ExecutionContext::with_current(|ec| Presence::of(ec.factory())) {
            let key = together::presence_key(&room);
            let user = user_id();
            let selection = parameters.get("selection").map_or(Json::Null, to_json);
            presence.set(
                &key,
                &client,
                &user,
                &together::display_name(facade, &user),
                Some(sheet),
                selection,
            );
            out.insert(
                "peers".to_string(),
                Value::List(presence.peers(&key, &client).iter().map(from_json).collect()),
            );
        }
    }
    let since = text(parameters, "since");
    if !since.is_empty() && since.parse::<i64>().ok() == Some(room.seq) {
        out.insert("unchanged".to_string(), Value::Bool(true));
        return Ok(out);
    }
    match room.block(
        sheet,
        int(parameters, "row", 1),
        int(parameters, "column", 1),
        int(parameters, "width", 1),
        int(parameters, "height", 1),
    ) {
        Ok(block) => {
            out.insert("blockJson".to_string(), Value::String(block));
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// `refresh#CalcDocument`: the data sheets read again as the caller, the workbook evaluated. The room's other people
/// are told: their data is the same rows, read again.
fn refresh_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let sheets = moqui_lib::sync::lock(&room).data_sheets.clone();
    // read with the room unlocked: a viewport of it is still answered meanwhile
    let rows = match read_sheets(facade, &sheets, &access, true) {
        Ok(rows) => rows,
        Err(message) => return refused(message),
    };
    let user = user_id();
    let mut room = moqui_lib::sync::lock(&room);
    match room.refresh(rows) {
        Ok(()) => {
            if let Some(definition_id) = room.definition_id.as_deref().filter(|_| room.shared) {
                crate::log::announce(facade, definition_id, room.seq, &user, "refresh");
            }
            Ok(room_out(facade, &room, &user))
        }
        Err(err) => refused(err.message()),
    }
}

/// The room's workbook without its data, as a document stores it, within [`MAX_OVERLAY_BYTES`].
fn overlay_of(room: &Room) -> Result<Vec<u8>, Error> {
    let overlay = room.stored_form()?;
    if overlay.len() > MAX_OVERLAY_BYTES {
        return Err(Error::new(format!(
            "The workbook without its data is {} MB; a document stores at most {} MB",
            overlay.len() / 1024 / 1024,
            MAX_OVERLAY_BYTES / 1024 / 1024
        )));
    }
    Ok(overlay)
}

/// Write the room's document as the definition `id` with `save#Definition`, at `version` when it is one already: the
/// version saved.
fn write_document(
    room: &Room,
    id: &str,
    title: &str,
    version: Option<i64>,
    group: &str,
    note: &str,
) -> Result<Option<i64>, Error> {
    let overlay = overlay_of(room)?;
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
    if !group.is_empty() {
        definition["userGroupId"] = Json::String(group.to_string());
    }
    let mut call_parameters = BTreeMap::new();
    call_parameters.insert("definition".to_string(), from_json(&definition));
    call_parameters.insert("note".to_string(), Value::string(note));
    let saved = call("org.moqui.report.ReportServices.save#Definition", call_parameters)?
        .get("definition")
        .map(to_json)
        .unwrap_or(Json::Null);
    Ok(saved["version"].as_i64())
}

/// `save#CalcDocument`: the room's document saved as a report definition of type `calcgrid`. A saved document's room
/// saves that document, under its edit log's lock, at the version the log last saved. A scratch room saves a new one
/// under `definitionId` (or an id made from the title), and becomes the new document's room for the saver's scope
/// (`roomId` is its new id).
fn save_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    if matches!(access, Access::Profile(_)) && !profile::may_author() {
        return refused("Not available");
    }
    let Some(shared_room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) else {
        return refused("no execution context is entered");
    };
    let user = user_id();
    let given = text(parameters, "definitionId");
    let group = text(parameters, "userGroupId");
    let note = text(parameters, "note");
    let mut room = moqui_lib::sync::lock(&shared_room);
    let title = Some(text(parameters, "title"))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| room.title.clone());
    let out = |id: &str, version: Option<i64>, title: &str, room_id: &str| {
        let mut out = BTreeMap::new();
        out.insert("definitionId".to_string(), Value::string(id));
        out.insert("version".to_string(), version.map_or(Value::Null, Value::I64));
        out.insert("title".to_string(), Value::string(title));
        out.insert("roomId".to_string(), Value::string(room_id));
        out
    };
    if room.shared {
        let id = room.definition_id.clone().unwrap_or_default();
        if !given.is_empty() && given != id {
            return refused("A document open with others is saved as itself; copy it from the list of documents");
        }
        let saved = together::save(facade, &mut room, &user, |room, version| {
            write_document(room, &id, &title, version, &group, &note)
        });
        return match saved {
            Ok(version) => {
                room.title = title.clone();
                Ok(out(&id, version, &title, &room.id))
            }
            Err(err) => {
                if err.message().contains(REOPEN) {
                    let room_id = room.id.clone();
                    drop(room);
                    rooms.drop_room(&room_id);
                    return refused(ROOM_GONE);
                }
                refused(err.message())
            }
        };
    }
    let id = if given.is_empty() {
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
        format!("calc-{}-{}", slug.trim_matches('-'), &room.id[1..room.id.len().min(9)])
    } else {
        given
    };
    let version = match write_document(&room, &id, &title, None, &group, &note) {
        Ok(version) => version,
        Err(err) => return refused(err.message()),
    };
    room.definition_id = Some(id.clone());
    room.version = version;
    room.title = title.clone();
    let (scratch_id, sheets) = (room.id.clone(), room.data_sheets.clone());
    drop(room);
    // the room is now the document's for the saver's scope, at the start of its log (one of a document saved over
    // begins again, and its rooms are opened anew)
    let scope = crate::scope::fingerprint(facade, &sheets, &access);
    let shared_id = crate::room::document_room_id(&id, access.profile_id().as_deref(), &scope);
    match together::begin_again(facade, &id, &user, version) {
        Ok(seq) => rooms.make_shared(&scratch_id, &shared_id, seq),
        Err(err) => {
            tracing::warn!(document = %id, error = %err, "A saved CalcGrid document's log did not begin; its room stays its saver's");
            return Ok(out(&id, version, &title, &scratch_id));
        }
    }
    Ok(out(&id, version, &title, &shared_id))
}

/// `get#CalcState`: the room's state as `open#CalcDocument` answers it, brought up to its document's log: what a view
/// reads when it is told of another's edit.
fn get_calc_state(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let held = moqui_lib::sync::lock(&room);
    Ok(room_out(facade, &held, &user_id()))
}

/// `revert#CalcDocument`: discard the edits of a saved document since it was saved, everyone's: its log begins again
/// from the saved version, every room of it is opened anew, and the caller's room is answered as `open#CalcDocument`
/// answers. An author's to do.
fn revert_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    if matches!(access, Access::Profile(_)) && !profile::may_author() {
        return refused("Not available");
    }
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) else {
        return refused("no execution context is entered");
    };
    let (definition_id, version) = {
        let held = moqui_lib::sync::lock(&room);
        match held.definition_id.clone().filter(|_| held.shared) {
            Some(id) => (id, held.version),
            None => return refused("Only a saved document has changes to discard"),
        }
    };
    if let Err(err) = together::begin_again(facade, &definition_id, &user_id(), version) {
        return refused(err.message());
    }
    for open in rooms.document_rooms(&definition_id) {
        let id = moqui_lib::sync::lock(&open).id.clone();
        rooms.drop_room(&id);
    }
    let mut again = BTreeMap::new();
    again.insert("definitionId".to_string(), Value::string(&definition_id));
    if let Some(profile_id) = access.profile_id() {
        again.insert("profileId".to_string(), Value::String(profile_id));
    }
    open_document_with(&again, facade, access)
}

/// `export#CalcDocument`: the workbook as it is in the room, data and calculated values, as an `.xlsx`.
fn export_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let room = moqui_lib::sync::lock(&room);
    let written = if room.charts.is_empty() {
        workbook::model_xlsx(room.model())
    } else {
        // the document's charts go into a copy of the workbook, on a sheet of their own
        with_charts(room.model(), &room.charts).and_then(|model| workbook::model_xlsx(&model))
    };
    match written {
        Ok(bytes) => {
            let mut out = BTreeMap::new();
            out.insert("contentBytes".to_string(), Value::Bytes(bytes));
            out.insert("filename".to_string(), Value::String(file_name(&room.title, "xlsx")));
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// A copy of `model` with the document's `charts` on a sheet of their own ([`crate::charts`]).
fn with_charts(
    model: &moqui_core::workbook::ironcalc::base::Model<'_>,
    charts: &[Json],
) -> Result<moqui_core::workbook::ironcalc::base::Model<'static>, Error> {
    let mut copy = fill::open_stored(&model.to_bytes())?;
    add_charts(&mut copy, charts)?;
    Ok(copy)
}

/// Puts the document's `charts` in `model`; a chart whose ranges are not in the workbook is left out, and logged.
fn add_charts(model: &mut moqui_core::workbook::ironcalc::base::Model<'static>, charts: &[Json]) -> Result<(), Error> {
    for left_out in crate::charts::add_to_model(model, charts)? {
        tracing::info!("CalcGrid export: {left_out}");
    }
    Ok(())
}

/// `close#CalcDocument`: drop the caller's room.
fn close_calc_document(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let user_id = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    if let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) {
        rooms.close(&user_id, &text(parameters, "roomId"));
    }
    Ok(BTreeMap::new())
}

/// `get#CalcProfiles`: how the caller may use CalcGrid: `tools` (the Tools app's own access: every source of the data
/// grid, no filter), the access `profiles` they hold (`profileId`, `description`), and `canAuthor`.
fn get_calc_profiles(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let tools = {
        let _required = ExecutionContext::with_current(ExecutionContext::require_authz);
        moqui_core::user::permitted_own("AT_XML_SCREEN", "component://tools/screen/Tools.xml", "view").unwrap_or(false)
    };
    let mut profiles = Vec::new();
    for id in Profile::held_ids(facade).map_err(|err| LibError::new(err.message()))? {
        if let Ok(held) = Profile::held(facade, &id) {
            profiles.push(from_json(
                &json!({"profileId": held.id, "description": held.description}),
            ));
        }
    }
    let mut out = BTreeMap::new();
    out.insert("tools".to_string(), Value::Bool(tools));
    out.insert("profiles".to_string(), Value::List(profiles));
    out.insert("canAuthor".to_string(), Value::Bool(tools || profile::may_author()));
    Ok(out)
}

/// `find#CalcEntities`: the entities and views a new document may be over, whose name holds `term`, fifty at most:
/// the ones the profile offers, or with no profile every entity of the model.
fn find_calc_entities(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let term = text(parameters, "term");
    let names = match &access {
        Access::Profile(held) => held.offered_entities(facade, &term, 50),
        Access::Tools => {
            let term = term.to_lowercase();
            moqui_core::entity::info::all_entity_names(facade, "")
                .into_iter()
                .filter(|name| !term.is_empty() && name.to_lowercase().contains(&term))
                .take(50)
                .collect()
        }
    };
    let mut out = BTreeMap::new();
    out.insert(
        "entityNames".to_string(),
        Value::List(names.into_iter().map(Value::String).collect()),
    );
    Ok(out)
}

/// `edit#CalcDocument`: apply edits to the room's workbook in order and evaluate once ([`Room::edit`]). Under a profile
/// only an author edits. The room's sheets, charts and undo state are answered whether or not an edit was refused, with
/// the refusal as an error: the edits before it stay.
fn edit_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    if matches!(access, Access::Profile(_)) && !profile::may_author() {
        return refused("Not available");
    }
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let ops: Vec<Json> = match parameters.get("ops").map(to_json) {
        Some(Json::Array(ops)) => ops,
        _ => return refused("ops is a list of edits"),
    };
    if ops.len() > MAX_EDIT_OPS {
        return refused(format!("At most {MAX_EDIT_OPS} edits in one call"));
    }
    let user = user_id();
    let mut held = moqui_lib::sync::lock(&room);
    if held.shared {
        match together::edit(facade, &mut held, &user, &ops) {
            Ok(edited) => {
                if let Some(err) = edited.refused {
                    add_error(err.message());
                }
            }
            Err(err) if err.message().contains(REOPEN) => {
                let id = held.id.clone();
                drop(held);
                if let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) {
                    rooms.drop_room(&id);
                }
                return refused(ROOM_GONE);
            }
            Err(err) => add_error(err.message()),
        }
    } else if let Err(err) = held.edit(&user, &ops) {
        add_error(err.message());
    }
    let mut out = room_out(facade, &held, &user);
    if let Some(changed) = held.changed() {
        out.insert("changed".to_string(), from_json(&together::areas_json(changed)));
    }
    Ok(out)
}

/// `get#CalcCellContent`: what was typed into a cell (a formula with its `=`), for the formula bar and a cell's editor.
fn get_calc_cell_content(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = Access::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let Some(room) = room_of(parameters, &access, facade) else {
        return Ok(BTreeMap::new());
    };
    let room = moqui_lib::sync::lock(&room);
    let sheet = u32::try_from(int(parameters, "sheet", 0)).unwrap_or(0);
    match room.cell_content(sheet, int(parameters, "row", 1), int(parameters, "column", 1)) {
        Ok(content) => {
            let mut out = BTreeMap::new();
            out.insert("content".to_string(), Value::String(content));
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// `get#CalcFrame`: a range of a saved document as a table for the report engine (a dataset with this service as its
/// `serviceName`): the first row of `range` names the columns, each row below is a record. The document is opened as
/// the caller under `profileId`, or with none under their current access ([`Access::current`]), its data read as
/// they may read it; a room the caller already has for the document is read again, its data read anew when it is
/// over a minute old.
fn get_calc_frame(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = access_or_current(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let definition_id = text(parameters, "definitionId");
    let range = text(parameters, "range");
    if definition_id.is_empty() || range.is_empty() {
        return refused("A CalcGrid frame needs a definitionId and a range");
    }
    let Some(user_id) = moqui_core::user::current_user_id().ok().flatten() else {
        return refused("Not signed in");
    };
    let Some(rooms) = ExecutionContext::with_current(|ec| Rooms::of(ec.factory())) else {
        return refused("no execution context is entered");
    };
    let profile_id = access.profile_id();
    let room = match rooms.find_document(&user_id, &definition_id, profile_id.as_deref()) {
        Some(room) => {
            let (stale, sheets) = {
                let room = moqui_lib::sync::lock(&room);
                (room.data_age() > FRAME_DATA_AGE, room.data_sheets.clone())
            };
            if stale {
                let rows = match read_sheets(facade, &sheets, &access, true) {
                    Ok(rows) => rows,
                    Err(message) => return refused(message),
                };
                if let Err(err) = moqui_lib::sync::lock(&room).refresh(rows) {
                    return refused(err.message());
                }
            }
            room
        }
        None => {
            let mut open = BTreeMap::new();
            open.insert("definitionId".to_string(), Value::string(&definition_id));
            if let Some(profile) = &profile_id {
                open.insert("profileId".to_string(), Value::string(profile));
            }
            // opened as any other room, with its checks; an empty answer is a refusal whose error is added
            let opened = open_document_with(&open, facade, access)?;
            let Some(room) = opened
                .get("roomId")
                .and_then(Value::as_str)
                .and_then(|id| rooms.get(&user_id, id))
            else {
                return Ok(BTreeMap::new());
            };
            room
        }
    };
    let room = moqui_lib::sync::lock(&room);
    match room.frame(&range, MAX_FRAME_ROWS) {
        Ok(frame) => {
            let mut out = BTreeMap::new();
            out.insert("frame".to_string(), from_json(&frame));
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// The access of a call that may name a profile and otherwise reads under the caller's current one: a dashboard
/// panel's, a scheduled report's. None, with an error added, when the caller has none.
fn access_or_current(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Access> {
    if text(parameters, "profileId").is_empty() {
        let access = Access::current(facade);
        if access.is_none() {
            add_error("Not available");
        }
        access
    } else {
        Access::of(parameters, facade)
    }
}

/// `get#CalcScope`: the fingerprint of the caller's scope for a saved document ([`crate::scope::fingerprint`]) under
/// `profileId`, or under their current access: equal for two people only when their data sheets hold the same rows.
fn get_calc_scope(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = access_or_current(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let definition = match read_definition(&text(parameters, "definitionId")) {
        Ok(definition) => definition,
        Err(err) => return refused(err.message()),
    };
    let sheets = data_sheets(&definition["body"]);
    let mut out = BTreeMap::new();
    out.insert(
        "scope".to_string(),
        Value::String(crate::scope::fingerprint(facade, &sheets, &access)),
    );
    out.insert(
        "access".to_string(),
        Value::String(access.profile_id().unwrap_or_else(|| TOOLS_ACCESS.to_string())),
    );
    Ok(out)
}

/// A file name from a title: letters, digits, space, `-`, `_` and `.`.
fn file_name(title: &str, extension: &str) -> String {
    let name: String = title
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || " -_.".contains(ch) {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim();
    format!("{}.{extension}", if name.is_empty() { "CalcGrid" } else { name })
}

/// `render#CalcDocument`: a saved document as an `.xlsx` with its data and calculated values, with no room: what a
/// scheduled report of the document sends. The saved version is read, the data sheets as the caller under `profileId`
/// or their current access, and every data sheet must read whole: one not available, or with more rows than a sheet
/// takes, refuses the run, as a report with an incomplete panel is refused.
fn render_calc_document(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let Some(access) = access_or_current(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let definition = match read_definition(&text(parameters, "definitionId")) {
        Ok(definition) => definition,
        Err(err) => return refused(err.message()),
    };
    let body = &definition["body"];
    let base = match body["overlay"].as_str().filter(|overlay| !overlay.is_empty()) {
        Some(overlay) => match base64::engine::general_purpose::STANDARD.decode(overlay) {
            Ok(bytes) => fill::open_stored(&bytes),
            Err(_) => return refused("The document's workbook is not readable"),
        },
        None => fill::new_model("Sheet1"),
    };
    let base = match base {
        Ok(base) => base,
        Err(err) => return refused(err.message()),
    };
    let rows = match read_sheets(facade, &data_sheets(body), &access, false) {
        Ok(rows) => rows,
        Err(message) => return refused(message),
    };
    if let Some(cut) = rows.iter().find(|sheet| sheet.truncated) {
        return refused(format!(
            "{} has more than {MAX_DATA_ROWS} rows; a document's export holds every row",
            cut.name
        ));
    }
    let row_count: usize = rows.iter().map(|sheet| sheet.rows.len()).sum();
    let mut model = match fill::fill(base, &rows) {
        Ok(model) => model,
        Err(err) => return refused(err.message()),
    };
    model.evaluate();
    let charts = body["charts"].as_array().map(Vec::as_slice).unwrap_or_default();
    if let Err(err) = add_charts(&mut model, charts) {
        return refused(err.message());
    }
    match workbook::model_xlsx(&model) {
        Ok(bytes) => {
            let title = definition["title"].as_str().unwrap_or("CalcGrid").to_string();
            let mut out = BTreeMap::new();
            out.insert("contentBytes".to_string(), Value::Bytes(bytes));
            out.insert("filename".to_string(), Value::String(file_name(&title, "xlsx")));
            out.insert("title".to_string(), Value::String(title));
            out.insert(
                "rowCount".to_string(),
                Value::I64(i64::try_from(row_count).unwrap_or(i64::MAX)),
            );
            Ok(out)
        }
        Err(err) => refused(err.message()),
    }
}

/// `set#CalcProfile`: keep the access the caller chose, a profile they hold or `tools`, as their current one: what a
/// dashboard panel and a scheduled report over a CalcGrid document read under when they name none.
fn set_calc_profile(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let chosen = text(parameters, "profileId");
    if chosen != TOOLS_ACCESS && Profile::held(facade, &chosen).is_err() {
        return refused("Not available");
    }
    moqui_core::user::set_preference(PROFILE_PREFERENCE, Some(&chosen)).map_err(|err| LibError::new(err.message()))?;
    Ok(BTreeMap::new())
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
