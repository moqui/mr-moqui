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

//! The engine work of a CalcGrid workbook (plans/IronCalc-DB.md, Architecture and Phase 0 results), with no
//! ExecutionContext: a data sheet filled with rows ([`fill`]), the stored form of a workbook ([`stored_form`]) and
//! what a view needs of its sheets ([`layout`]).
//!
//! A data sheet is a header row of column names and a row for each record, declared as an Excel table so a formula may
//! name its columns (`Orders[amount]`). The engine turns such a name into a fixed range when the formula is typed, so
//! the ranges are kept right by the engine's own row insertion: the stored form keeps two empty data rows (2 and 3)
//! under the header, a fill inserts the rows it needs between them, and the stored form deletes them again. The stored
//! form has no row of data, no value computed from one, and none of the data's texts in the workbook's shared strings.

use std::collections::HashMap;

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::types::{
    Alignment, Cell, HorizontalAlignment, Style, Table, TableColumn, TableStyleInfo, VerticalAlignment, Workbook,
};
use moqui_core::workbook::ironcalc::base::Model;
use serde_json::{json, Value as Json};

/// The data rows a stored data sheet keeps, empty, so ranges over the data have rows to stretch from.
pub const STUB_ROWS: i32 = 2;
/// A character and a cell's padding in pixels, for a fitted column, and the limits of one.
const FIT_CHAR: f64 = 7.5;
const FIT_PAD: f64 = 16.0;
const FIT_MIN: f64 = 50.0;
const FIT_MAX: f64 = 360.0;

/// A cell of a data sheet. A date, a time and a time stamp are Excel serial numbers, shown by their formats.
#[derive(Clone, Debug, PartialEq)]
pub enum CellData {
    Empty,
    Text(String),
    Number(f64),
    Bool(bool),
    Date(f64),
    Time(f64),
    DateTime(f64),
}

/// The rows of one data sheet, as its source gave them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetRows {
    /// The sheet's name in the workbook.
    pub name: String,
    /// The header row: the columns' names.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CellData>>,
    /// The source had more rows than the sheet takes.
    pub truncated: bool,
}

fn engine(message: String) -> Error {
    Error::new(format!("Spreadsheet engine: {message}"))
}

/// A new workbook with one empty sheet named `sheet`.
///
/// # Errors
///
/// Returns the engine's error for a sheet name it refuses.
pub fn new_model(sheet: &str) -> Result<Model<'static>, Error> {
    let mut model = Model::new_empty("workbook", "en", "UTC", "en").map_err(engine)?;
    model.rename_sheet_by_index(0, sheet).map_err(engine)?;
    Ok(model)
}

/// The model of a stored workbook ([`stored_form`]).
///
/// # Errors
///
/// Returns the engine's error for bytes that are not one.
pub fn open_stored(bytes: &[u8]) -> Result<Model<'static>, Error> {
    Model::from_bytes(bytes, "en").map_err(engine)
}

fn sheet_index(model: &Model<'_>, name: &str) -> Option<u32> {
    model
        .workbook
        .get_worksheet_names()
        .iter()
        .position(|found| found.eq_ignore_ascii_case(name))
        .and_then(|index| u32::try_from(index).ok())
}

/// The table name of a data sheet: its name as an identifier, which a formula's `Name[column]` takes.
#[must_use]
pub fn table_name(sheet: &str) -> String {
    let mut name: String = sheet
        .chars()
        .map(|ch| if ch.is_alphanumeric() { ch } else { '_' })
        .collect();
    if !name.chars().next().is_some_and(char::is_alphabetic) {
        name.insert(0, 'T');
    }
    name
}

fn column_letters(column: i32) -> String {
    moqui_core::workbook::ironcalc::base::expressions::utils::number_to_column(column)
        .unwrap_or_else(|| "A".to_string())
}

/// The table over a data sheet of `rows` data rows (never fewer than the stub's).
fn table(sheet: &str, columns: &[String], rows: i32) -> Table {
    let last = column_letters(i32::try_from(columns.len().max(1)).unwrap_or(1));
    let name = table_name(sheet);
    Table {
        name: name.clone(),
        display_name: name,
        sheet_name: sheet.to_string(),
        reference: format!("A1:{last}{}", rows.max(STUB_ROWS) + 1),
        totals_row_count: 0,
        header_row_count: 1,
        header_row_dxf_id: None,
        data_dxf_id: None,
        totals_row_dxf_id: None,
        columns: columns
            .iter()
            .enumerate()
            .map(|(index, name)| TableColumn {
                id: u32::try_from(index + 1).unwrap_or(u32::MAX),
                name: name.clone(),
                ..TableColumn::default()
            })
            .collect(),
        style_info: TableStyleInfo {
            name: None,
            show_first_column: false,
            show_last_column: false,
            show_row_stripes: false,
            show_column_stripes: false,
        },
        has_filters: false,
    }
}

/// The style indexes a fill writes with, found or made in the workbook.
struct Styles {
    header: i32,
    date: i32,
    time: i32,
    date_time: i32,
}

impl Styles {
    fn of(workbook: &mut Workbook) -> Self {
        let mut index = |style: &Style| {
            workbook
                .styles
                .get_style_index(style)
                .unwrap_or_else(|| workbook.styles.create_new_style(style))
        };
        let mut header = Style::default();
        header.font.b = true;
        let format = |format: &str| Style {
            num_fmt: format.to_string(),
            alignment: Some(Alignment {
                horizontal: HorizontalAlignment::Left,
                vertical: VerticalAlignment::Bottom,
                wrap_text: false,
            }),
            ..Style::default()
        };
        Self {
            header: index(&header),
            date: index(&format("yyyy-mm-dd")),
            time: index(&format("hh:mm:ss")),
            date_time: index(&format("yyyy-mm-dd hh:mm:ss")),
        }
    }
}

/// Fill each data sheet of `model` with its rows, adding a sheet the workbook does not have, and give the model made
/// again over the filled workbook, not yet evaluated. A sheet the workbook has is in the stored form: its header and
/// [`STUB_ROWS`] empty rows, with the workbook's formulas over them.
///
/// # Errors
///
/// Returns the engine's error: a sheet name it refuses, or rows that would break an array formula.
pub fn fill(mut model: Model<'static>, sheets: &[SheetRows]) -> Result<Model<'static>, Error> {
    // the engine's own steps first, while its caches match the workbook: new sheets, and the rows that stretch ranges
    let mut indexes = Vec::with_capacity(sheets.len());
    for sheet in sheets {
        let rows = i32::try_from(sheet.rows.len()).map_err(|_| Error::new("Too many rows for a sheet"))?;
        let index = match sheet_index(&model, &sheet.name) {
            Some(index) => {
                if rows > STUB_ROWS {
                    model
                        .insert_rows(index, STUB_ROWS + 1, rows - STUB_ROWS)
                        .map_err(engine)?;
                }
                index
            }
            None => {
                model.add_sheet(&sheet.name).map_err(engine)?;
                sheet_index(&model, &sheet.name).ok_or_else(|| Error::new("The sheet was not added"))?
            }
        };
        indexes.push((index, rows));
    }
    let mut workbook = model.workbook;
    let styles = Styles::of(&mut workbook);
    let mut strings: HashMap<String, i32> = HashMap::new();
    for (index, text) in workbook.shared_strings.iter().enumerate() {
        strings
            .entry(text.clone())
            .or_insert(i32::try_from(index).unwrap_or(i32::MAX));
    }
    for (sheet, (index, rows)) in sheets.iter().zip(indexes) {
        let mut shared = |text: &str, all: &mut Vec<String>| -> i32 {
            if let Some(found) = strings.get(text) {
                return *found;
            }
            let made = i32::try_from(all.len()).unwrap_or(i32::MAX);
            all.push(text.to_string());
            strings.insert(text.to_string(), made);
            made
        };
        let mut all = std::mem::take(&mut workbook.shared_strings);
        let worksheet = workbook
            .worksheets
            .get_mut(index as usize)
            .ok_or_else(|| Error::new("The sheet is not in the workbook"))?;
        let mut widths: Vec<usize> = sheet.columns.iter().map(|name| name.chars().count()).collect();
        let header: Vec<(i32, Cell)> = sheet
            .columns
            .iter()
            .enumerate()
            .map(|(column, name)| {
                (
                    i32::try_from(column + 1).unwrap_or(i32::MAX),
                    Cell::SharedString {
                        si: shared(name, &mut all),
                        s: styles.header,
                    },
                )
            })
            .collect();
        worksheet.sheet_data.set_row(1, header);
        for (row, cells) in sheet.rows.iter().enumerate() {
            let written: Vec<(i32, Cell)> = cells
                .iter()
                .enumerate()
                .filter_map(|(column, cell)| {
                    let (made, shown) = match cell {
                        CellData::Empty => return None,
                        CellData::Text(text) => (
                            Cell::SharedString {
                                si: shared(text, &mut all),
                                s: 0,
                            },
                            text.chars().count(),
                        ),
                        CellData::Number(number) => (Cell::NumberCell { v: *number, s: 0 }, 10),
                        CellData::Bool(flag) => (Cell::BooleanCell { v: *flag, s: 0 }, 5),
                        CellData::Date(number) => (
                            Cell::NumberCell {
                                v: *number,
                                s: styles.date,
                            },
                            10,
                        ),
                        CellData::Time(number) => (
                            Cell::NumberCell {
                                v: *number,
                                s: styles.time,
                            },
                            8,
                        ),
                        CellData::DateTime(number) => (
                            Cell::NumberCell {
                                v: *number,
                                s: styles.date_time,
                            },
                            19,
                        ),
                    };
                    if let Some(width) = widths.get_mut(column) {
                        *width = (*width).max(shown);
                    }
                    Some((i32::try_from(column + 1).unwrap_or(i32::MAX), made))
                })
                .collect();
            worksheet
                .sheet_data
                .set_row(i32::try_from(row + 2).unwrap_or(i32::MAX), written);
        }
        worksheet.set_frozen_rows(1).map_err(engine)?;
        for (column, chars) in widths.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let pixels = (*chars as f64 * FIT_CHAR + FIT_PAD).clamp(FIT_MIN, FIT_MAX);
            worksheet
                .set_column_width(i32::try_from(column + 1).unwrap_or(i32::MAX), pixels)
                .map_err(engine)?;
        }
        workbook.shared_strings = all;
        let name = worksheet_name(&workbook, index);
        workbook
            .tables
            .insert(table_name(&name), table(&name, &sheet.columns, rows));
    }
    Model::from_workbook(workbook, "en").map_err(engine)
}

fn worksheet_name(workbook: &Workbook, index: u32) -> String {
    workbook
        .worksheets
        .get(index as usize)
        .map(|sheet| sheet.get_name())
        .unwrap_or_default()
}

/// The stored form of a workbook: `model` (as bytes of [`Model::to_bytes`]) with each data sheet (its name and the
/// data rows it holds) cut back to its header and [`STUB_ROWS`] empty rows, the texts only the data used dropped from
/// the shared strings, and every formula evaluated over the empty data.
///
/// # Errors
///
/// Returns the engine's error.
pub fn stored_form(bytes: &[u8], data: &[(String, usize)]) -> Result<Vec<u8>, Error> {
    let mut model = Model::from_bytes(bytes, "en").map_err(engine)?;
    let mut emptied = Vec::new();
    for (name, rows) in data {
        let Some(index) = sheet_index(&model, name) else {
            continue;
        };
        let rows = i32::try_from(*rows).map_err(|_| Error::new("Too many rows for a sheet"))?;
        if rows > STUB_ROWS {
            model
                .delete_rows(index, STUB_ROWS + 1, rows - STUB_ROWS)
                .map_err(engine)?;
        }
        emptied.push(index);
    }
    let mut workbook = model.workbook;
    for index in &emptied {
        let Some(worksheet) = workbook.worksheets.get_mut(*index as usize) else {
            continue;
        };
        for row in 2..=(STUB_ROWS + 1) {
            worksheet.sheet_data.remove_row(row);
        }
        let name = worksheet.get_name();
        if let Some(found) = workbook.tables.get_mut(&table_name(&name)) {
            let columns: Vec<String> = found.columns.iter().map(|column| column.name.clone()).collect();
            *found = table(&name, &columns, 0);
        }
    }
    // the shared strings again, from the cells that are left
    let old = std::mem::take(&mut workbook.shared_strings);
    let mut kept: Vec<String> = Vec::new();
    let mut moved: HashMap<i32, i32> = HashMap::new();
    for worksheet in &mut workbook.worksheets {
        let texts: Vec<(i32, i32, i32, i32)> = worksheet
            .sheet_data
            .cells()
            .filter_map(|(row, column, cell)| match cell.as_ref() {
                Cell::SharedString { si, s } => Some((row, column, *si, *s)),
                _ => None,
            })
            .collect();
        for (row, column, si, s) in texts {
            let new = *moved.entry(si).or_insert_with(|| {
                kept.push(
                    usize::try_from(si)
                        .ok()
                        .and_then(|at| old.get(at))
                        .cloned()
                        .unwrap_or_default(),
                );
                i32::try_from(kept.len() - 1).unwrap_or(i32::MAX)
            });
            worksheet
                .sheet_data
                .set_cell(row, column, Cell::SharedString { si: new, s });
        }
    }
    workbook.shared_strings = kept;
    let mut model = Model::from_workbook(workbook, "en").map_err(engine)?;
    model.evaluate();
    Ok(model.to_bytes())
}

/// What a view needs of each sheet: its name, its used rows and columns, the width of each used column in pixels,
/// the frozen rows and columns, and whether it is one of `data` (a read-only data sheet).
#[must_use]
pub fn layout(model: &Model<'_>, data: &[(String, usize)]) -> Json {
    let sheets: Vec<Json> = model
        .workbook
        .worksheets
        .iter()
        .enumerate()
        .map(|(index, worksheet)| {
            let name = worksheet.get_name();
            let dimension = worksheet.dimension();
            let widths: Vec<f64> = (1..=dimension.max_column.max(1))
                .map(|column| worksheet.get_column_width(column).unwrap_or(90.0).round())
                .collect();
            let rows = data
                .iter()
                .find(|(found, _)| found.eq_ignore_ascii_case(&name))
                .map(|(_, rows)| *rows);
            json!({
                "index": index,
                "name": name,
                "rows": dimension.max_row.max(1),
                "columns": dimension.max_column.max(1),
                "widths": widths,
                "frozenRows": worksheet.frozen_rows,
                "frozenColumns": worksheet.frozen_columns,
                "data": rows.is_some(),
                "dataRows": rows,
                "hidden": !matches!(worksheet.state, moqui_core::workbook::ironcalc::base::types::SheetState::Visible),
            })
        })
        .collect();
    Json::Array(sheets)
}

#[cfg(test)]
#[path = "fill_tests.rs"]
mod tests;
