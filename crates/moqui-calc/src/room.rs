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
//! has it open, read a viewport at a time, and dropped when nobody has used it for a while.
//!
//! A saved document is open in a room shared by the people of one scope ([`crate::scope`]): its id is made from the
//! document, the access and the scope ([`document_room_id`]), so everyone who reads the same rows edits one workbook,
//! and a call finds a room only when the caller is one of its members. The rooms of a document's other scopes, and on
//! other servers, apply the same edits from the document's log ([`crate::log`]). A workbook not saved yet (a new one,
//! or a file) is a scratch room of the one person who opened it. The rooms are in this server's memory: a request
//! that reaches another server of a cluster finds none there, and the view opens the document again.
//!
//! Undo is each person's own: before an edit is applied the room takes what it will change ([`Room::inverse`]: the
//! cells as typed and their styles, the widths, the sheet), and an undo applies that. The taken edits stay with the room
//! that took them, as a formula over a data sheet is held there with that room's own rows.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_core::context::{Error, ExecutionContextFactory};
use moqui_core::workbook::ironcalc::base::{Model, UserModel};
use moqui_core::workbook::parse_area;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::fill::{self, SheetRows};
use crate::log::{self, LogRow};

/// A room not used for this long is dropped.
pub const IDLE: Duration = Duration::from_secs(20 * 60);
/// The most rooms one user holds; opening another drops the one used longest ago.
pub const MAX_USER_ROOMS: usize = 8;
/// The most cells of one viewport request.
pub const MAX_BLOCK_CELLS: i64 = 20_000;
/// The most points of one chart series.
const MAX_CHART_POINTS: usize = 2_000;
/// The most rows of one paste.
const MAX_PASTE_ROWS: usize = 5_000;
/// The most cells an edit may change: what it changes is kept to undo it.
pub const MAX_EDIT_CELLS: i64 = 100_000;
/// The newest edits a scratch room keeps for undo.
const MAX_OWN_LOG: usize = 500;

fn engine(message: String) -> Error {
    Error::new(format!("Spreadsheet engine: {message}"))
}

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
    /// A saved document's room, shared by the people of one scope; else one person's scratch workbook.
    pub shared: bool,
    /// Who has the room open, with when each last used it.
    members: HashMap<String, Instant>,
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
    /// The last row of the log applied here: the document's ([`crate::log`]) for a shared room, its own for a scratch
    /// one.
    pub seq: i64,
    /// What each edit applied here since the saved version took, by its row: the edits that take it back.
    inverses: BTreeMap<i64, Vec<Json>>,
    /// A scratch room's log, for its person's undo; a shared room's is the document's.
    own_log: Vec<LogRow>,
    model: UserModel<'static>,
    last_used: Instant,
    /// When the data sheets were last read.
    filled_at: Instant,
}

/// What a room is opened with.
pub struct Opening {
    pub user_id: String,
    /// A saved document's room for a scope ([`document_room_id`]), at the log's saved edit; None for a scratch room.
    pub shared: Option<(String, i64)>,
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
    fn build(opening: Opening) -> Result<Self, Error> {
        let id = match &opening.shared {
            Some((id, _)) => id.clone(),
            None => scratch_id(),
        };
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
            shared: opening.shared.is_some(),
            members: HashMap::from([(opening.user_id, Instant::now())]),
            profile_id: opening.profile_id,
            definition_id: opening.definition_id,
            version: opening.version,
            title: opening.title,
            data_sheets: opening.data_sheets,
            charts: opening.charts,
            data_rows,
            truncated,
            evaluate_ms: started.elapsed().as_secs_f64() * 1000.0,
            seq: opening.shared.map_or(0, |(_, seq)| seq),
            inverses: BTreeMap::new(),
            own_log: Vec::new(),
            model: UserModel::from_model(model),
            last_used: Instant::now(),
            filled_at: Instant::now(),
        })
    }

    /// Whether `user_id` has the room open.
    #[must_use]
    pub fn is_member(&self, user_id: &str) -> bool {
        self.members.contains_key(user_id)
    }

    /// The people who have the room open.
    #[must_use]
    pub fn member_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.members.keys().cloned().collect();
        ids.sort();
        ids
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

    /// The index of a sheet when it is a data sheet, whose cells come from the database and are not edited.
    fn data_sheet_name(&self, sheet: u32) -> Option<String> {
        let name = self.model().workbook.worksheets.get(sheet as usize)?.get_name();
        self.data_rows
            .iter()
            .any(|(data, _)| data.eq_ignore_ascii_case(&name))
            .then_some(name)
    }

    /// Whether `user_id` has an edit of their own to undo in this scratch room, and an undo to redo. A shared room's
    /// are in the document's log ([`log::undo_state`]).
    #[must_use]
    pub fn undo_state(&self, user_id: &str) -> (bool, bool) {
        log::undo_state(&self.own_log, user_id, 0)
    }

    /// What was typed into a cell: a formula with its `=`, a number or a text as entered.
    ///
    /// # Errors
    ///
    /// Returns the engine's error for a cell outside the workbook.
    pub fn cell_content(&self, sheet: u32, row: i32, column: i32) -> Result<String, Error> {
        self.model.get_cell_content(sheet, row, column).map_err(engine)
    }

    /// Apply edits of `user_id` in order to this scratch room, then evaluate once (plans/IronCalc-DB.md, Phase 3). An
    /// edit is a map with `op`:
    ///
    /// - `input` (`sheet`, `row`, `column`, `value`): a cell set as a person typing would; an empty value clears it.
    /// - `paste` (`sheet`, `row`, `column`, `rows`: a list of rows of texts): cells set from a top-left cell.
    /// - `clear`, `clearFormatting` and `style` (`path`, `value`: `font.b`, `font.i`, `font.u`, `font.strike`,
    ///   `font.color`, `fill.color`, `num_fmt`, `alignment.horizontal`, `alignment.wrap_text`) over `sheet`, `row`,
    ///   `column`, `width`, `height`.
    /// - `restore` (`sheet`, `row`, `column`, `cells`: rows of contents as typed, `styles`: rows of the engine's
    ///   styles): a rectangle set as it was, which is how an edit is taken back.
    /// - `fill` (a source rectangle and `toRow` or `toColumn`): the engine's autofill, down or across.
    /// - `width` (`sheet`, `column`, `columnEnd`, `width`) and `height` (`sheet`, `row`, `rowEnd`, `height`), pixels.
    /// - `freeze` (`sheet`, `rows`, `columns`).
    /// - `insertRows`, `deleteRows` (`sheet`, `row`, `count`), `insertColumns`, `deleteColumns` (`sheet`, `column`,
    ///   `count`).
    /// - `addSheet`, `renameSheet` (`sheet`, `name`), `moveSheet` (`sheet`, `to`), `deleteSheet` (`sheet`).
    /// - `charts` (`charts`: the document's charts, replaced).
    /// - `undo`, `redo`, each alone: the person's own last edit taken back, or their last undo.
    ///
    /// A data sheet's cells, rows, columns, place and name are not edited: its widths, heights and frozen panes are.
    ///
    /// # Errors
    ///
    /// Returns the first edit's refusal; the edits before it are applied (and are undone together).
    pub fn edit(&mut self, user_id: &str, ops: &[Json]) -> Result<(), Error> {
        self.model.pause_evaluation();
        let result = self.edit_own(user_id, ops);
        self.evaluate();
        result
    }

    fn edit_own(&mut self, user_id: &str, ops: &[Json]) -> Result<(), Error> {
        match single_kind(ops) {
            Some(log::UNDO) => {
                let target = log::undo_target(&self.own_log, user_id, 0)
                    .map(|row| row.seq)
                    .ok_or_else(|| Error::new(NOTHING_TO_UNDO))?;
                self.undo_row(target)?;
                let seq = self.push_own(user_id, log::UNDO, Vec::new(), Some(target));
                self.mark_own(target, seq);
                Ok(())
            }
            Some(log::REDO) => {
                let undo = log::redo_target(&self.own_log, user_id, 0)
                    .cloned()
                    .ok_or_else(|| Error::new(NOTHING_TO_REDO))?;
                let original = self
                    .own_log
                    .iter()
                    .find(|row| Some(row.seq) == undo.target)
                    .map(|row| row.ops.clone())
                    .unwrap_or_default();
                let (applied, inverse, refused) = self.apply_ops(&original);
                let seq = self.push_own(user_id, log::REDO, applied, Some(undo.seq));
                self.inverses.insert(seq, inverse);
                self.mark_own(undo.seq, seq);
                refused.map_or(Ok(()), Err)
            }
            _ => {
                let (applied, inverse, refused) = self.apply_ops(ops);
                if !applied.is_empty() {
                    let seq = self.push_own(user_id, log::EDIT, applied, None);
                    self.inverses.insert(seq, inverse);
                }
                refused.map_or(Ok(()), Err)
            }
        }
    }

    fn push_own(&mut self, user_id: &str, kind: &str, ops: Vec<Json>, target: Option<i64>) -> i64 {
        self.seq += 1;
        self.own_log.push(LogRow {
            seq: self.seq,
            user_id: user_id.to_string(),
            kind: kind.to_string(),
            ops,
            target,
            undone: None,
        });
        if self.own_log.len() > MAX_OWN_LOG {
            let dropped = self.own_log.remove(0);
            self.inverses.remove(&dropped.seq);
        }
        self.seq
    }

    fn mark_own(&mut self, seq: i64, by: i64) {
        if let Some(row) = self.own_log.iter_mut().find(|row| row.seq == seq) {
            row.undone = Some(by);
        }
    }

    /// Hold evaluation while edits are applied ([`Room::apply_ops`], [`Room::apply_row`]); [`Room::evaluate`] ends it.
    pub fn pause(&mut self) {
        self.model.pause_evaluation();
    }

    /// Evaluate the workbook, and time it.
    pub fn evaluate(&mut self) {
        self.model.resume_evaluation();
        let started = Instant::now();
        self.model.evaluate();
        self.evaluate_ms = started.elapsed().as_secs_f64() * 1000.0;
    }

    /// Apply `ops` in order, each after taking what it changes: the ops applied, the edits that take them all back (the
    /// last applied first), and the refusal that stopped them.
    pub fn apply_ops(&mut self, ops: &[Json]) -> (Vec<Json>, Vec<Json>, Option<Error>) {
        let mut applied = Vec::new();
        let mut parts: Vec<Vec<Json>> = Vec::new();
        let mut refused = None;
        for op in ops {
            match self.inverse(op).and_then(|inverse| self.apply(op).map(|()| inverse)) {
                Ok(inverse) => {
                    applied.push(op.clone());
                    parts.push(inverse);
                }
                Err(err) => {
                    refused = Some(err);
                    break;
                }
            }
        }
        (applied, parts.into_iter().rev().flatten().collect(), refused)
    }

    /// Keep what the edit of row `seq` took, for its undo.
    pub fn record(&mut self, seq: i64, inverse: Vec<Json>) {
        self.inverses.insert(seq, inverse);
    }

    /// Take back the edit of row `target` with what this room took of it.
    ///
    /// # Errors
    ///
    /// An error when this room did not apply that edit, or the engine refused a step of taking it back.
    pub fn undo_row(&mut self, target: i64) -> Result<(), Error> {
        let inverse = self
            .inverses
            .get(&target)
            .cloned()
            .ok_or_else(|| Error::new("That edit is no longer undone here"))?;
        inverse.iter().try_for_each(|op| self.apply(op))
    }

    /// Apply a row of the document's log that another room wrote, and move to it. An edit or a redo another room
    /// applied is applied here over this room's data; a step this room refuses is logged and the rest applied.
    ///
    /// # Errors
    ///
    /// An error when the room cannot follow the log (a revert, an undo of an edit it did not apply): it is to be
    /// opened anew.
    pub fn apply_row(&mut self, row: &LogRow) -> Result<(), Error> {
        match row.kind.as_str() {
            log::EDIT | log::REDO => {
                let (_, inverse, refused) = self.apply_ops(&row.ops);
                if let Some(err) = refused {
                    tracing::warn!(room = %self.id, seq = row.seq, error = %err, "A CalcGrid room refused an edit of its document's log");
                }
                self.inverses.insert(row.seq, inverse);
            }
            log::UNDO => self.undo_row(row.target.unwrap_or(0))?,
            _ => return Err(Error::new(REOPEN)),
        }
        self.seq = row.seq;
        Ok(())
    }

    /// Forget what the edits at or before `seq` took: they are in the saved version and no longer undone.
    pub fn saved_through(&mut self, seq: i64) {
        self.inverses.retain(|at, _| *at > seq);
    }

    /// The used rows and columns of a sheet.
    fn used(&self, sheet: u32) -> (i32, i32) {
        self.model()
            .workbook
            .worksheets
            .get(sheet as usize)
            .map_or((0, 0), |worksheet| {
                let dimension = worksheet.dimension();
                (dimension.max_row, dimension.max_column)
            })
    }

    /// A rectangle as the edits that set it again: each cell's content as typed and its style, within the sheet's used
    /// area (a cell beyond it has neither). For an edit that `writes` cells, the rectangle is first cleared of contents
    /// and formatting, so what it wrote beyond the used area goes too.
    fn snapshot(
        &self,
        sheet: u32,
        row: i32,
        column: i32,
        width: i32,
        height: i32,
        writes: bool,
    ) -> Result<Vec<Json>, Error> {
        let mut edits = Vec::new();
        if writes {
            if i64::from(width.max(1)) * i64::from(height.max(1)) > MAX_EDIT_CELLS {
                return Err(Error::new(format!("An edit changes at most {MAX_EDIT_CELLS} cells")));
            }
            for op in ["clear", "clearFormatting"] {
                edits.push(json!({"op": op, "sheet": sheet, "row": row, "column": column, "width": width.max(1), "height": height.max(1)}));
            }
        }
        let (max_row, max_column) = self.used(sheet);
        let last_row = row.saturating_add(height.max(1) - 1).min(max_row);
        let last_column = column.saturating_add(width.max(1) - 1).min(max_column);
        if last_row < row || last_column < column {
            return Ok(edits);
        }
        if i64::from(last_row - row + 1) * i64::from(last_column - column + 1) > MAX_EDIT_CELLS {
            return Err(Error::new(format!(
                "An edit changes at most {MAX_EDIT_CELLS} cells that hold something"
            )));
        }
        let mut cells = Vec::new();
        let mut styles = Vec::new();
        for at_row in row..=last_row {
            let mut contents = Vec::new();
            let mut row_styles = Vec::new();
            for at_column in column..=last_column {
                contents.push(Json::String(
                    self.model.get_cell_content(sheet, at_row, at_column).map_err(engine)?,
                ));
                let style = self.model.get_cell_style(sheet, at_row, at_column).map_err(engine)?;
                row_styles.push(serde_json::to_value(style).map_err(|err| Error::new(err.to_string()))?);
            }
            cells.push(Json::Array(contents));
            styles.push(Json::Array(row_styles));
        }
        edits.push(
            json!({"op": "restore", "sheet": sheet, "row": row, "column": column, "cells": cells, "styles": styles}),
        );
        Ok(edits)
    }

    /// Every formula of the sheets that are not data sheets, as the edits that type it again: what a deletion of rows,
    /// columns or a sheet turns into `#REF!` where it read them, set back once they are back.
    fn formulas(&self) -> Result<Vec<Json>, Error> {
        let mut edits = Vec::new();
        let mut cells: i64 = 0;
        for (index, worksheet) in self.model().workbook.worksheets.iter().enumerate() {
            let sheet = u32::try_from(index).map_err(|err| Error::new(err.to_string()))?;
            if self.data_sheet_name(sheet).is_some() {
                continue;
            }
            let dimension = worksheet.dimension();
            cells += i64::from(dimension.max_row) * i64::from(dimension.max_column);
            if cells > MAX_EDIT_CELLS {
                return Err(Error::new(format!(
                    "The sheets hold more than {MAX_EDIT_CELLS} cells: a row, column or sheet is not deleted"
                )));
            }
            for row in 1..=dimension.max_row {
                for column in 1..=dimension.max_column {
                    if let Some(formula) = self.model().get_cell_formula(sheet, row, column).map_err(engine)? {
                        edits.push(
                            json!({"op": "input", "sheet": sheet, "row": row, "column": column, "value": formula}),
                        );
                    }
                }
            }
        }
        Ok(edits)
    }

    /// Widths (`width`) or heights of a run of columns or rows as the edits that set them again, a run of equal ones
    /// as one.
    fn sizes(&self, sheet: u32, first: i32, last: i32, columns: bool) -> Result<Vec<Json>, Error> {
        if i64::from(last) - i64::from(first) >= MAX_EDIT_CELLS {
            return Err(Error::new(format!(
                "An edit changes at most {MAX_EDIT_CELLS} rows or columns"
            )));
        }
        let mut edits = Vec::new();
        let mut run: Option<(i32, i32, f64)> = None;
        let flush = |run: Option<(i32, i32, f64)>, edits: &mut Vec<Json>| {
            if let Some((from, to, size)) = run {
                edits.push(if columns {
                    json!({"op": "width", "sheet": sheet, "column": from, "columnEnd": to, "width": size})
                } else {
                    json!({"op": "height", "sheet": sheet, "row": from, "rowEnd": to, "height": size})
                });
            }
        };
        for at in first..=last {
            let size = if columns {
                self.model.get_column_width(sheet, at)
            } else {
                self.model.get_row_height(sheet, at)
            }
            .map_err(engine)?;
            run = match run {
                Some((from, _, same)) if (same - size).abs() < f64::EPSILON => Some((from, at, same)),
                other => {
                    flush(other, &mut edits);
                    Some((at, at, size))
                }
            };
        }
        flush(run, &mut edits);
        Ok(edits)
    }

    /// The edits that take `op` back, taken from the workbook before it is applied: what it changes, as typed (a
    /// formula as its text, never its value), with its styles, sizes and place.
    ///
    /// # Errors
    ///
    /// An error for an edit that changes more than [`MAX_EDIT_CELLS`] used cells, or that the engine cannot read.
    pub fn inverse(&self, op: &Json) -> Result<Vec<Json>, Error> {
        let int = |key: &str, default: i64| op[key].as_i64().unwrap_or(default);
        let small = |key: &str, default: i64| i32::try_from(int(key, default)).unwrap_or(i32::MAX);
        let sheet = u32::try_from(int("sheet", 0)).unwrap_or(u32::MAX);
        let (row, column) = (small("row", 1), small("column", 1));
        let (width, height) = (small("width", 1).max(1), small("height", 1).max(1));
        let sheet_count = u32::try_from(self.model().workbook.worksheets.len()).unwrap_or(0);
        Ok(match op["op"].as_str().unwrap_or_default() {
            "input" => self.snapshot(sheet, row, column, 1, 1, true)?,
            "paste" => {
                let rows = op["rows"].as_array().map(Vec::as_slice).unwrap_or_default();
                let across = rows
                    .iter()
                    .map(|cells| cells.as_array().map_or(0, Vec::len))
                    .max()
                    .unwrap_or(0);
                self.snapshot(
                    sheet,
                    row,
                    column,
                    i32::try_from(across).unwrap_or(i32::MAX),
                    i32::try_from(rows.len()).unwrap_or(i32::MAX),
                    true,
                )?
            }
            "clear" | "clearFormatting" | "restore" => {
                let (width, height) = if op["op"] == "restore" {
                    let cells = op["cells"].as_array().map(Vec::as_slice).unwrap_or_default();
                    (
                        i32::try_from(cells.first().and_then(Json::as_array).map_or(1, Vec::len)).unwrap_or(1),
                        i32::try_from(cells.len()).unwrap_or(1),
                    )
                } else {
                    (width, height)
                };
                self.snapshot(sheet, row, column, width, height, op["op"] == "restore")?
            }
            "style" => {
                // what is styled beyond the used area had no style of its own
                let mut edits = vec![
                    json!({"op": "clearFormatting", "sheet": sheet, "row": row, "column": column, "width": width, "height": height}),
                ];
                edits.extend(self.snapshot(sheet, row, column, width, height, false)?);
                edits
            }
            "fill" => match op["toRow"].as_i64() {
                Some(to_row) => {
                    let to_row = i32::try_from(to_row).unwrap_or(i32::MAX);
                    let (top, bottom) = (row.min(to_row), (row + height - 1).max(to_row));
                    self.snapshot(sheet, top, column, width, bottom - top + 1, true)?
                }
                None => {
                    let to_column = small("toColumn", 1);
                    let (left, right) = (column.min(to_column), (column + width - 1).max(to_column));
                    self.snapshot(sheet, row, left, right - left + 1, height, true)?
                }
            },
            "width" => self.sizes(sheet, column, small("columnEnd", i64::from(column)), true)?,
            "height" => self.sizes(sheet, row, small("rowEnd", i64::from(row)), false)?,
            "freeze" => vec![json!({
                "op": "freeze", "sheet": sheet,
                "rows": self.model.get_frozen_rows_count(sheet).map_err(engine)?,
                "columns": self.model.get_frozen_columns_count(sheet).map_err(engine)?,
            })],
            "insertRows" => vec![json!({"op": "deleteRows", "sheet": sheet, "row": row, "count": small("count", 1)})],
            "insertColumns" => {
                vec![json!({"op": "deleteColumns", "sheet": sheet, "column": column, "count": small("count", 1)})]
            }
            "deleteRows" => {
                let count = small("count", 1).max(1);
                let (_, max_column) = self.used(sheet);
                let mut edits = vec![json!({"op": "insertRows", "sheet": sheet, "row": row, "count": count})];
                edits.extend(self.sizes(sheet, row, row + count - 1, false)?);
                edits.extend(self.snapshot(sheet, row, 1, max_column, count, false)?);
                edits.extend(self.formulas()?);
                edits
            }
            "deleteColumns" => {
                let count = small("count", 1).max(1);
                let (max_row, _) = self.used(sheet);
                let mut edits = vec![json!({"op": "insertColumns", "sheet": sheet, "column": column, "count": count})];
                edits.extend(self.sizes(sheet, column, column + count - 1, true)?);
                edits.extend(self.snapshot(sheet, 1, column, count, max_row, false)?);
                edits.extend(self.formulas()?);
                edits
            }
            "addSheet" => vec![json!({"op": "deleteSheet", "sheet": sheet_count})],
            "renameSheet" => {
                let name = self
                    .model()
                    .workbook
                    .worksheets
                    .get(sheet as usize)
                    .map(|worksheet| worksheet.get_name())
                    .ok_or_else(|| Error::new("No such sheet"))?;
                vec![json!({"op": "renameSheet", "sheet": sheet, "name": name})]
            }
            "moveSheet" => vec![json!({"op": "moveSheet", "sheet": small("to", 0), "to": sheet})],
            "deleteSheet" => {
                let worksheet = self
                    .model()
                    .workbook
                    .worksheets
                    .get(sheet as usize)
                    .ok_or_else(|| Error::new("No such sheet"))?;
                let name = worksheet.get_name();
                let (max_row, max_column) = self.used(sheet);
                // added again at the end, named and moved back, then filled
                let last = sheet_count.saturating_sub(1);
                let mut edits = vec![
                    json!({"op": "addSheet"}),
                    json!({"op": "renameSheet", "sheet": last, "name": name}),
                    json!({"op": "moveSheet", "sheet": last, "to": sheet}),
                ];
                edits.extend(self.sizes(sheet, 1, max_column.max(1), true)?);
                edits.extend(self.sizes(sheet, 1, max_row.max(1), false)?);
                edits.extend(self.snapshot(sheet, 1, 1, max_column, max_row, false)?);
                edits.extend(self.formulas()?);
                edits.push(json!({
                    "op": "freeze", "sheet": sheet,
                    "rows": self.model.get_frozen_rows_count(sheet).map_err(engine)?,
                    "columns": self.model.get_frozen_columns_count(sheet).map_err(engine)?,
                }));
                edits
            }
            "charts" => vec![json!({"op": "charts", "charts": self.charts})],
            // refused when it is applied
            _ => Vec::new(),
        })
    }

    fn apply(&mut self, op: &Json) -> Result<(), Error> {
        let int = |key: &str, default: i64| op[key].as_i64().unwrap_or(default);
        let small = |key: &str, default: i64| i32::try_from(int(key, default)).unwrap_or(i32::MAX);
        let sheet = u32::try_from(int("sheet", 0)).unwrap_or(u32::MAX);
        let text = |key: &str| match &op[key] {
            Json::String(text) => text.clone(),
            Json::Null => String::new(),
            other => other.to_string(),
        };
        let area = || moqui_core::workbook::ironcalc::base::expressions::types::Area {
            sheet,
            row: small("row", 1),
            column: small("column", 1),
            width: small("width", 1).max(1),
            height: small("height", 1).max(1),
        };
        let kind = op["op"].as_str().unwrap_or_default();
        // what changes a sheet's cells, rows, columns, place or name is not done to a data sheet
        let protects = matches!(
            kind,
            "input"
                | "paste"
                | "clear"
                | "clearFormatting"
                | "restore"
                | "style"
                | "fill"
                | "insertRows"
                | "deleteRows"
                | "insertColumns"
                | "deleteColumns"
                | "renameSheet"
                | "moveSheet"
                | "deleteSheet"
        );
        if protects {
            if let Some(name) = self.data_sheet_name(sheet) {
                return Err(Error::new(format!(
                    "{name} is a data sheet: its rows come from the database and are not edited"
                )));
            }
        }
        match kind {
            "input" => self
                .model
                .set_user_input(sheet, small("row", 1), small("column", 1), &text("value")),
            "paste" => {
                let (top, left) = (small("row", 1), small("column", 1));
                let rows = op["rows"].as_array().map(Vec::as_slice).unwrap_or_default();
                if rows.len() > MAX_PASTE_ROWS {
                    return Err(Error::new(format!("A paste has at most {MAX_PASTE_ROWS} rows")));
                }
                rows.iter().enumerate().try_for_each(|(r, cells)| {
                    cells
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                        .iter()
                        .enumerate()
                        .try_for_each(|(c, cell)| {
                            let value = match cell {
                                Json::String(text) => text.clone(),
                                Json::Null => String::new(),
                                other => other.to_string(),
                            };
                            let (row, column) = (
                                top + i32::try_from(r).unwrap_or(i32::MAX),
                                left + i32::try_from(c).unwrap_or(i32::MAX),
                            );
                            self.model.set_user_input(sheet, row, column, &value)
                        })
                })
            }
            "restore" => self.restore(sheet, small("row", 1), small("column", 1), op),
            "clear" => self.model.range_clear_contents(&area()),
            "clearFormatting" => self.model.range_clear_formatting(&area()),
            "style" => self.model.update_range_style(&area(), &text("path"), &text("value")),
            "fill" => match op["toRow"].as_i64() {
                Some(to_row) => self
                    .model
                    .auto_fill_rows(&area(), i32::try_from(to_row).unwrap_or(i32::MAX)),
                None => self.model.auto_fill_columns(&area(), small("toColumn", 1)),
            },
            "width" => {
                let column = small("column", 1);
                self.model.set_columns_width(
                    sheet,
                    column,
                    small("columnEnd", i64::from(column)),
                    op["width"].as_f64().unwrap_or(90.0),
                )
            }
            "height" => {
                let row = small("row", 1);
                self.model.set_rows_height(
                    sheet,
                    row,
                    small("rowEnd", i64::from(row)),
                    op["height"].as_f64().unwrap_or(25.0),
                )
            }
            "freeze" => self
                .model
                .set_frozen_rows_count(sheet, small("rows", 0))
                .and_then(|()| self.model.set_frozen_columns_count(sheet, small("columns", 0))),
            "insertRows" => self.model.insert_rows(sheet, small("row", 1), small("count", 1)),
            "deleteRows" => self.model.delete_rows(sheet, small("row", 1), small("count", 1)),
            "insertColumns" => self.model.insert_columns(sheet, small("column", 1), small("count", 1)),
            "deleteColumns" => self.model.delete_columns(sheet, small("column", 1), small("count", 1)),
            "addSheet" => self.model.new_sheet(),
            "renameSheet" => self.model.rename_sheet(sheet, &text("name")),
            "moveSheet" => self
                .model
                .move_sheet(sheet, u32::try_from(int("to", 0)).unwrap_or(u32::MAX)),
            "deleteSheet" => self.model.delete_sheet(sheet),
            "charts" => {
                self.charts = op["charts"].as_array().cloned().unwrap_or_default();
                Ok(())
            }
            "undo" | "redo" => Err(format!("{kind} is an edit of its own")),
            other => Err(format!("{other:?} is not an edit")),
        }
        .map_err(engine)
    }

    /// A `restore` edit: each cell set as typed (an empty one cleared), then each cell's style.
    fn restore(&mut self, sheet: u32, row: i32, column: i32, op: &Json) -> Result<(), String> {
        use moqui_core::workbook::ironcalc::base::expressions::types::Area;
        use moqui_core::workbook::ironcalc::base::types::Style;
        let cells = op["cells"].as_array().map(Vec::as_slice).unwrap_or_default();
        for (r, contents) in cells.iter().enumerate() {
            let at_row = row + i32::try_from(r).map_err(|err| err.to_string())?;
            for (c, content) in contents
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let at_column = column + i32::try_from(c).map_err(|err| err.to_string())?;
                match content.as_str().unwrap_or_default() {
                    "" => self.model.range_clear_contents(&Area {
                        sheet,
                        row: at_row,
                        column: at_column,
                        width: 1,
                        height: 1,
                    })?,
                    typed => self.model.set_user_input(sheet, at_row, at_column, typed)?,
                }
            }
        }
        let styles: Vec<Vec<Style>> = serde_json::from_value(op["styles"].clone()).unwrap_or_default();
        let width = styles.first().map_or(0, Vec::len);
        if width > 0 && styles.iter().all(|row_styles| row_styles.len() == width) {
            let last_row = row + i32::try_from(styles.len()).map_err(|err| err.to_string())? - 1;
            let last_column = column + i32::try_from(width).map_err(|err| err.to_string())? - 1;
            self.model.set_selected_sheet(sheet)?;
            self.model.set_selected_cell(row, column)?;
            self.model.set_selected_range(row, column, last_row, last_column)?;
            self.model.on_paste_styles(&styles)?;
        }
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
/// The rooms by id.
type RoomMap = HashMap<String, SharedRoom>;

/// An answer of [`Room::apply_row`] that means the room cannot follow its document's log and is opened anew.
pub const REOPEN: &str = "The document was reverted or changed elsewhere; open it again";
const NOTHING_TO_UNDO: &str = "Nothing of yours to undo";
const NOTHING_TO_REDO: &str = "Nothing of yours to redo";

/// The kind of `ops` when it is a lone undo or redo.
#[must_use]
pub fn single_kind(ops: &[Json]) -> Option<&str> {
    match ops {
        [op] => op["op"]
            .as_str()
            .filter(|kind| *kind == log::UNDO || *kind == log::REDO),
        _ => None,
    }
}

/// A scratch room's id: unique on this server.
fn scratch_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("r{nanos:x}{:x}", ROOM_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// The id of a saved document's room for the readers of one scope under one access: the same on every server.
#[must_use]
pub fn document_room_id(definition_id: &str, profile_id: Option<&str>, scope: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{definition_id}\u{1f}{}\u{1f}{scope}", profile_id.unwrap_or("")).as_bytes());
    let hex: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("d{hex}")
}

/// The open rooms of a factory, by id, each with its members.
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

    /// Open a room for `opening`'s user: fill the data sheets, evaluate, and keep it. A saved document's room for a
    /// scope that someone opened meanwhile is joined in place of the one built.
    ///
    /// # Errors
    ///
    /// Returns the engine's error.
    pub fn open(&self, opening: Opening) -> Result<SharedRoom, Error> {
        let user_id = opening.user_id.clone();
        // built outside the lock: a fill and an evaluation take a while
        let built = Room::build(opening)?;
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        let room = match rooms.get(&built.id) {
            Some(existing) => {
                lock(existing).members.insert(user_id.clone(), Instant::now());
                Arc::clone(existing)
            }
            None => {
                let id = built.id.clone();
                let room = Arc::new(Mutex::new(built));
                rooms.insert(id, Arc::clone(&room));
                room
            }
        };
        let id = lock(&room).id.clone();
        Self::limit(&mut rooms, &user_id, &id);
        Ok(room)
    }

    /// Join the room `id` when it is open: a saved document's room for the caller's scope that another reader opened.
    #[must_use]
    pub fn join(&self, user_id: &str, id: &str) -> Option<SharedRoom> {
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        let room = rooms.get(id).cloned()?;
        {
            let mut held = lock(&room);
            held.members.insert(user_id.to_string(), Instant::now());
            held.last_used = Instant::now();
        }
        Self::limit(&mut rooms, user_id, id);
        Some(room)
    }

    /// Drop the memberships of `user_id` beyond [`MAX_USER_ROOMS`], the one used longest ago first, never `keep`; a
    /// room with no member left is dropped.
    fn limit(rooms: &mut RoomMap, user_id: &str, keep: &str) {
        loop {
            let held: Vec<(String, Instant)> = rooms
                .iter()
                .filter_map(|(id, room)| lock(room).members.get(user_id).map(|used| (id.clone(), *used)))
                .collect();
            if held.len() <= MAX_USER_ROOMS {
                return;
            }
            let Some((oldest, _)) = held
                .into_iter()
                .filter(|(id, _)| id != keep)
                .min_by_key(|(_, used)| *used)
            else {
                return;
            };
            Self::leave(rooms, user_id, &oldest);
        }
    }

    fn leave(rooms: &mut RoomMap, user_id: &str, id: &str) {
        let empty = rooms.get(id).is_some_and(|room| {
            let mut room = lock(room);
            room.members.remove(user_id);
            room.members.is_empty()
        });
        if empty {
            rooms.remove(id);
        }
    }

    /// Drop the members not seen for [`IDLE`], and the rooms with none left.
    fn sweep(rooms: &mut RoomMap) {
        rooms.retain(|_, room| {
            let mut room = lock(room);
            room.members.retain(|_, used| used.elapsed() < IDLE);
            !room.members.is_empty()
        });
    }

    /// The room `id` when `user_id` has it open, marked used now. None when they do not: it was never theirs, or they
    /// left it, or it was dropped.
    #[must_use]
    pub fn get(&self, user_id: &str, id: &str) -> Option<SharedRoom> {
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        let room = rooms.get(id).cloned()?;
        {
            let mut held = lock(&room);
            let used = held.members.get_mut(user_id)?;
            *used = Instant::now();
            held.last_used = Instant::now();
        }
        Some(room)
    }

    /// A room `user_id` has open that is the saved document `definition_id` opened under `profile_id`, marked used now:
    /// one a report reads again in place of opening the document for each panel.
    #[must_use]
    pub fn find_document(&self, user_id: &str, definition_id: &str, profile_id: Option<&str>) -> Option<SharedRoom> {
        let mut rooms = lock(&self.rooms);
        Self::sweep(&mut rooms);
        let found = rooms
            .values()
            .find(|room| {
                let room = lock(room);
                room.is_member(user_id)
                    && room.definition_id.as_deref() == Some(definition_id)
                    && room.profile_id.as_deref() == profile_id
            })
            .cloned()?;
        {
            let mut held = lock(&found);
            if let Some(used) = held.members.get_mut(user_id) {
                *used = Instant::now();
            }
        }
        Some(found)
    }

    /// The open rooms of the saved document `definition_id`, of every scope.
    #[must_use]
    pub fn document_rooms(&self, definition_id: &str) -> Vec<SharedRoom> {
        lock(&self.rooms)
            .values()
            .filter(|room| {
                let room = lock(room);
                room.shared && room.definition_id.as_deref() == Some(definition_id)
            })
            .cloned()
            .collect()
    }

    /// Give the scratch room `id` the id of a saved document's room once it is saved: its people go on in it, as the
    /// room the document's other readers of the scope join. A room of that id already open is dropped: its document
    /// was saved over.
    pub fn make_shared(&self, id: &str, shared_id: &str, seq: i64) {
        let mut rooms = lock(&self.rooms);
        let Some(room) = rooms.remove(id) else { return };
        {
            let mut held = lock(&room);
            held.id = shared_id.to_string();
            held.shared = true;
            held.seq = seq;
            held.own_log.clear();
            held.inverses.clear();
        }
        rooms.insert(shared_id.to_string(), room);
    }

    /// Drop the room `id`, whoever has it open: it cannot follow its document's log, and is opened anew.
    pub fn drop_room(&self, id: &str) {
        lock(&self.rooms).remove(id);
    }

    /// `user_id` leaves the room `id`; it is dropped when nobody has it open.
    pub fn close(&self, user_id: &str, id: &str) {
        Self::leave(&mut lock(&self.rooms), user_id, id);
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
