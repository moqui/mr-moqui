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

//! The rooms of open CalcGrid documents (plans/IronCalc-DB.md, Thin client): a workbook held in memory while someone
//! has it open, read a viewport at a time, and dropped when it has not been used for a while.
//!
//! A room is its user's: its key is the user and the room id, so a call finds only a room the caller opened, and each
//! is filled with what that user may read. (Rooms shared by the people of one scope come with editing and the access
//! profiles, Phases 2b and 3.) The rooms are in this server's memory: a request that reaches another server of a
//! cluster finds none, and the view opens the document again there.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_core::context::{Error, ExecutionContextFactory};
use moqui_core::workbook::ironcalc::base::{Model, UserModel};
use moqui_core::workbook::parse_area;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::fill::{self, SheetRows};

/// A room not used for this long is dropped.
pub const IDLE: Duration = Duration::from_secs(20 * 60);
/// The most rooms one user holds; opening another drops the one used longest ago.
pub const MAX_USER_ROOMS: usize = 8;
/// The most cells of one viewport request.
pub const MAX_BLOCK_CELLS: i64 = 20_000;
/// The most points of one chart series.
const MAX_CHART_POINTS: usize = 2_000;

static ROOM_SEQ: AtomicU64 = AtomicU64::new(0);

/// A data sheet of a document: its name and the data grid's spec of what fills it (`source`, `columns`, `sort`,
/// `filter`), as the document's body keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct DataSheet {
    pub name: String,
    pub spec: Json,
}

/// An open document.
pub struct Room {
    pub id: String,
    pub user_id: String,
    /// The access profile the room was opened under and is read under; none is the Tools app's own access.
    pub profile_id: Option<String>,
    /// The saved document this is, when it is one, and the version it was read or saved at.
    pub definition_id: Option<String>,
    pub version: Option<i64>,
    pub title: String,
    pub data_sheets: Vec<DataSheet>,
    pub charts: Vec<Json>,
    /// Each data sheet's name and the data rows it holds now.
    pub data_rows: Vec<(String, usize)>,
    /// The data sheets whose source had more rows than were read.
    pub truncated: Vec<String>,
    /// How long the last evaluation took, in milliseconds.
    pub evaluate_ms: f64,
    model: UserModel<'static>,
    last_used: Instant,
    /// When the data sheets were last read.
    filled_at: Instant,
}

/// What a room is opened with.
pub struct Opening {
    pub user_id: String,
    pub profile_id: Option<String>,
    pub definition_id: Option<String>,
    pub version: Option<i64>,
    pub title: String,
    pub data_sheets: Vec<DataSheet>,
    pub charts: Vec<Json>,
    /// The workbook before its data: a stored form, a workbook read from a file, or a new one.
    pub base: Model<'static>,
    /// The rows of each data sheet, read as the user.
    pub rows: Vec<SheetRows>,
}

impl Room {
    fn build(id: String, opening: Opening) -> Result<Self, Error> {
        let data_rows: Vec<(String, usize)> = opening
            .rows
            .iter()
            .map(|sheet| (sheet.name.clone(), sheet.rows.len()))
            .collect();
        let truncated = opening
            .rows
            .iter()
            .filter(|sheet| sheet.truncated)
            .map(|sheet| sheet.name.clone())
            .collect();
        let mut model = fill::fill(opening.base, &opening.rows)?;
        let started = Instant::now();
        model.evaluate();
        Ok(Self {
            id,
            user_id: opening.user_id,
            profile_id: opening.profile_id,
            definition_id: opening.definition_id,
            version: opening.version,
            title: opening.title,
            data_sheets: opening.data_sheets,
            charts: opening.charts,
            data_rows,
            truncated,
            evaluate_ms: started.elapsed().as_secs_f64() * 1000.0,
            model: UserModel::from_model(model),
            last_used: Instant::now(),
            filled_at: Instant::now(),
        })
    }

    /// The engine's model, to read.
    #[must_use]
    pub fn model(&self) -> &Model<'_> {
        self.model.get_model()
    }

    /// The workbook's stored form ([`fill::stored_form`]): no data, no value from it.
    ///
    /// # Errors
    ///
    /// Returns the engine's error.
    pub fn stored_form(&self) -> Result<Vec<u8>, Error> {
        fill::stored_form(&self.model.to_bytes(), &self.data_rows)
    }

    /// Fill the data sheets again with `rows`, keeping the overlay as it is now, and evaluate.
    ///
    /// # Errors
    ///
    /// Returns the engine's error; the room is then as it was.
    pub fn refresh(&mut self, rows: Vec<SheetRows>) -> Result<(), Error> {
        let base = fill::open_stored(&self.stored_form()?)?;
        let data_rows: Vec<(String, usize)> = rows
            .iter()
            .map(|sheet| (sheet.name.clone(), sheet.rows.len()))
            .collect();
        let truncated = rows
            .iter()
            .filter(|sheet| sheet.truncated)
            .map(|sheet| sheet.name.clone())
            .collect();
        let mut model = fill::fill(base, &rows)?;
        let started = Instant::now();
        model.evaluate();
        self.evaluate_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.model = UserModel::from_model(model);
        self.data_rows = data_rows;
        self.truncated = truncated;
        self.filled_at = Instant::now();
        Ok(())
    }

    /// How long ago the data sheets were read.
    #[must_use]
    pub fn data_age(&self) -> Duration {
        self.filled_at.elapsed()
    }

    /// A range as a table for a report: its first row the columns' names, each row below a record. Every value is as
    /// the cell holds it (a number, a text, a boolean, or null), at most `max_rows` rows.
    ///
    /// # Errors
    ///
    /// Returns an error for a text that is not a range of the workbook.
    pub fn frame(&self, reference: &str, max_rows: usize) -> Result<Json, Error> {
        use moqui_core::workbook::ironcalc::base::cell::CellValue;
        let model = self.model();
        let area = parse_area(model, reference)?;
        let value = |row: i32, column: i32| match model.get_cell_value_by_index(area.sheet, row, column) {
            Ok(CellValue::Number(number)) => json!(number),
            Ok(CellValue::String(text)) => Json::String(text),
            Ok(CellValue::Boolean(flag)) => Json::Bool(flag),
            _ => Json::Null,
        };
        let last_row = area
            .last_row
            .min(area.row.saturating_add(i32::try_from(max_rows).unwrap_or(i32::MAX)));
        let mut fields = Vec::new();
        for column in area.column..=area.last_column {
            let name = model
                .get_formatted_cell_value(area.sheet, area.row, column)
                .ok()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Column {}", column - area.column + 1));
            let values: Vec<Json> = ((area.row + 1)..=last_row).map(|row| value(row, column)).collect();
            let number =
                values.iter().any(Json::is_number) && values.iter().all(|value| value.is_number() || value.is_null());
            fields.push(json!({"name": name, "type": if number { "number" } else { "string" }, "values": values}));
        }
        Ok(json!({ "fields": fields }))
    }

    /// The sheets as a view lays them out ([`fill::layout`]).
    #[must_use]
    pub fn layout(&self) -> Json {
        fill::layout(self.model(), &self.data_rows)
    }

    /// A rectangle of cells as JSON, the engine's block: the distinct styles, a style index for each cell, the texts
    /// of the cells that have one, and the fills as runs.
    ///
    /// # Errors
    ///
    /// Returns an error for a rectangle outside the sheet or over [`MAX_BLOCK_CELLS`].
    pub fn block(&self, sheet: u32, row: i32, column: i32, width: i32, height: i32) -> Result<String, Error> {
        if i64::from(width) * i64::from(height) > MAX_BLOCK_CELLS {
            return Err(Error::new(format!("A viewport has at most {MAX_BLOCK_CELLS} cells")));
        }
        let block = self
            .model
            .get_cells_block(sheet, row, column, width, height)
            .map_err(|message| Error::new(format!("Spreadsheet engine: {message}")))?;
        serde_json::to_string(&block).map_err(|err| Error::new(err.to_string()))
    }

    /// The document's charts with the values of the ranges they name, for a view to draw: each chart as it is kept
    /// (`title`, `type`, `categories`, `series` of `name` and `values`) with `labels` and each series' `data`. A chart
    /// whose range does not read is given with its `error`.
    #[must_use]
    pub fn charts(&self) -> Json {
        let model = self.model();
        let cells = |reference: &str| -> Result<Vec<(u32, i32, i32)>, Error> {
            let area = parse_area(model, reference)?;
            let mut cells = Vec::new();
            for row in area.row..=area.last_row {
                for column in area.column..=area.last_column {
                    if cells.len() >= MAX_CHART_POINTS {
                        return Ok(cells);
                    }
                    cells.push((area.sheet, row, column));
                }
            }
            Ok(cells)
        };
        let resolved: Vec<Json> = self
            .charts
            .iter()
            .map(|chart| {
                let mut out = chart.clone();
                let read = || -> Result<(Json, Json), Error> {
                    let labels: Vec<Json> = match chart["categories"].as_str().filter(|text| !text.is_empty()) {
                        Some(reference) => cells(reference)?
                            .into_iter()
                            .map(|(sheet, row, column)| {
                                Json::String(model.get_formatted_cell_value(sheet, row, column).unwrap_or_default())
                            })
                            .collect(),
                        None => Vec::new(),
                    };
                    let mut series = Vec::new();
                    for one in chart["series"].as_array().map(Vec::as_slice).unwrap_or_default() {
                        let reference = one["values"].as_str().unwrap_or_default();
                        let data: Vec<Json> = cells(reference)?
                            .into_iter()
                            .map(
                                |(sheet, row, column)| match model.get_cell_value_by_index(sheet, row, column) {
                                    Ok(moqui_core::workbook::ironcalc::base::cell::CellValue::Number(number)) => {
                                        json!(number)
                                    }
                                    _ => Json::Null,
                                },
                            )
                            .collect();
                        series.push(json!({"name": one["name"], "values": reference, "data": data}));
                    }
                    Ok((Json::Array(labels), Json::Array(series)))
                };
                match read() {
                    Ok((labels, series)) => {
                        out["labels"] = labels;
                        out["series"] = series;
                    }
                    Err(err) => out["error"] = Json::String(err.message().to_string()),
                }
                out
            })
            .collect();
        Json::Array(resolved)
    }
}

/// A room as its callers hold it.
pub type SharedRoom = Arc<Mutex<Room>>;
/// The rooms by user id and room id.
type RoomMap = HashMap<(String, String), SharedRoom>;

/// The open rooms of a factory, by user and room id.
#[derive(Default)]
pub struct Rooms {
    rooms: Mutex<RoomMap>,
}

impl Rooms {
    /// The rooms of `factory`.
    #[must_use]
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|_| Self::default())
    }

    /// Open a room: fill the data sheets, evaluate, and keep it under a new id.
    ///
    /// # Errors
    ///
    /// Returns the engine's error.
    pub fn open(&self, opening: Opening) -> Result<SharedRoom, Error> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let id = format!("r{nanos:x}{:x}", ROOM_SEQ.fetch_add(1, Ordering::Relaxed));
        let user_id = opening.user_id.clone();
        // built outside the lock: a fill and an evaluation take a while
        let room = Arc::new(Mutex::new(Room::build(id.clone(), opening)?));
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        while rooms.keys().filter(|(user, _)| *user == user_id).count() >= MAX_USER_ROOMS {
            let oldest = rooms
                .iter()
                .filter(|((user, _), _)| *user == user_id)
                .min_by_key(|(_, room)| lock(room).last_used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => rooms.remove(&key),
                None => break,
            };
        }
        rooms.insert((user_id, id), Arc::clone(&room));
        Ok(room)
    }

    fn sweep(rooms: &mut RoomMap) {
        rooms.retain(|_, room| lock(room).last_used.elapsed() < IDLE);
    }

    /// The room `id` of `user_id`, marked used now. None when the user has no such room: it was never theirs, or it
    /// was dropped.
    #[must_use]
    pub fn get(&self, user_id: &str, id: &str) -> Option<SharedRoom> {
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        let room = rooms.get(&(user_id.to_string(), id.to_string())).cloned()?;
        lock(&room).last_used = Instant::now();
        Some(room)
    }

    /// A room of `user_id` that is the saved document `definition_id` opened under `profile_id`, marked used now: one a
    /// report reads again in place of opening the document for each panel.
    #[must_use]
    pub fn find_document(&self, user_id: &str, definition_id: &str, profile_id: Option<&str>) -> Option<SharedRoom> {
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        let found = rooms
            .iter()
            .filter(|((user, _), _)| user == user_id)
            .map(|(_, room)| room)
            .find(|room| {
                let room = lock(room);
                room.definition_id.as_deref() == Some(definition_id) && room.profile_id.as_deref() == profile_id
            })
            .cloned()?;
        lock(&found).last_used = Instant::now();
        Some(found)
    }

    /// Drop the room `id` of `user_id`.
    pub fn close(&self, user_id: &str, id: &str) {
        lock(&self.rooms).remove(&(user_id.to_string(), id.to_string()));
    }

    /// How many rooms are open.
    #[must_use]
    pub fn count(&self) -> usize {
        lock(&self.rooms).len()
    }
}

#[cfg(test)]
#[path = "room_tests.rs"]
mod tests;
