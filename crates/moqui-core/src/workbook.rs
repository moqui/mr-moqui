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

//! A workbook built cell by cell on the IronCalc engine (plans/IronCalc-DB.md, Phase 1), for the xlsx writers: the
//! `xlsx` render mode and the data grid's export ([`crate::screen::XlsxBook`]) and a scheduled report's xlsx
//! (moqui-report). [`WorkbookWriter`] writes typed cells with styles below the engine's formula parser, so a text that
//! starts with `=` stays a text, and gives the engine's [`Model`] ([`WorkbookWriter::into_model`]) or the `.xlsx` file
//! ([`WorkbookWriter::to_xlsx`]). [`open_xlsx`] and [`model_xlsx`] are the engine's reader and writer with this crate's
//! error. The engine is re-exported as [`ironcalc`], so the crates on this one use the revision this one pins.

use std::collections::HashMap;

pub use ironcalc;
use ironcalc::base::expressions::utils::column_to_number;
use ironcalc::base::types::{Alignment, Cell, Color, Fill, HorizontalAlignment, Style, VerticalAlignment};
use ironcalc::base::Model;

use crate::context::Error;

/// The font of every cell a writer fills: the engine's own default (Inter) is on few machines.
const FONT_NAME: &str = "Calibri";
const FONT_SIZE: i32 = 11;
/// The narrowest and the widest a fitted column gets, in pixels.
const FIT_MIN: f64 = 40.0;
const FIT_MAX: f64 = 600.0;
/// A character of the cell font and a cell's side padding, in pixels, for [`WorkbookWriter::fit_columns`].
const FIT_CHAR: f64 = 7.0;
const FIT_PAD: f64 = 12.0;

/// How a cell looks. [`WorkbookWriter::style`] gives the index to write cells with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub bold: bool,
    pub align: Align,
    pub wrap: bool,
    /// A fill colour as `#RRGGBB`.
    pub fill: Option<String>,
    /// An Excel number format; none is General.
    pub number_format: Option<String>,
}

/// A cell's horizontal alignment. `General` is Excel's: text left, numbers right.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    General,
    Left,
    Center,
    Right,
}

/// A workbook being written. Rows and columns count from 1, as in the engine.
pub struct WorkbookWriter {
    /// The engine's model, used for what it keeps in step (sheets and their names). Cells, styles and texts are
    /// written to its workbook directly, so its own caches are stale until [`Self::into_model`] makes it again.
    model: Model<'static>,
    /// How many sheets were added; the engine's new workbook has one, which the first [`Self::add_sheet`] takes.
    sheets: usize,
    strings: HashMap<String, i32>,
    styles: HashMap<CellStyle, i32>,
    /// The widest text of each column of each sheet, in characters, for [`Self::fit_columns`].
    widths: Vec<HashMap<i32, usize>>,
}

impl WorkbookWriter {
    /// An empty workbook.
    ///
    /// # Errors
    ///
    /// Returns the engine's error when it cannot make one.
    pub fn new() -> Result<Self, Error> {
        let model = Model::new_empty("workbook", "en", "UTC", "en").map_err(engine_error)?;
        Ok(Self {
            model,
            sheets: 0,
            strings: HashMap::new(),
            styles: HashMap::new(),
            widths: Vec::new(),
        })
    }

    /// Add a sheet and return its index. The name is one Excel takes (the caller's to make so).
    ///
    /// # Errors
    ///
    /// Returns the engine's error for a name it refuses or one already used.
    pub fn add_sheet(&mut self, name: &str) -> Result<usize, Error> {
        if self.sheets == 0 {
            self.model.rename_sheet_by_index(0, name)
        } else {
            self.model.add_sheet(name)
        }
        .map_err(engine_error)?;
        self.sheets += 1;
        self.widths.push(HashMap::new());
        Ok(self.sheets - 1)
    }

    /// The index of a style, made once for each distinct one.
    pub fn style(&mut self, style: &CellStyle) -> i32 {
        if let Some(index) = self.styles.get(style) {
            return *index;
        }
        let mut made = Style::default();
        made.font.name = FONT_NAME.to_string();
        made.font.sz = FONT_SIZE;
        made.font.b = style.bold;
        made.alignment = Some(Alignment {
            horizontal: match style.align {
                Align::General => HorizontalAlignment::General,
                Align::Left => HorizontalAlignment::Left,
                Align::Center => HorizontalAlignment::Center,
                Align::Right => HorizontalAlignment::Right,
            },
            vertical: VerticalAlignment::Top,
            wrap_text: style.wrap,
        });
        if let Some(color) = &style.fill {
            made.fill = Fill {
                color: Color::Rgb(color.clone()),
            };
        }
        if let Some(format) = &style.number_format {
            made.num_fmt.clone_from(format);
        }
        let index = self.model.workbook.styles.create_new_style(&made);
        self.styles.insert(style.clone(), index);
        index
    }

    /// Write a text cell. The text is stored as it is: nothing is parsed as a formula, a number or a date.
    ///
    /// # Errors
    ///
    /// Returns an error for a sheet that was not added or a cell outside the sheet.
    pub fn text(&mut self, sheet: usize, row: i32, column: i32, text: &str, style: i32) -> Result<(), Error> {
        let si = if let Some(index) = self.strings.get(text) {
            *index
        } else {
            let index =
                i32::try_from(self.model.workbook.shared_strings.len()).map_err(|_| Error::new("Too many texts"))?;
            self.model.workbook.shared_strings.push(text.to_string());
            self.strings.insert(text.to_string(), index);
            index
        };
        let longest = text.lines().map(|line| line.chars().count()).max().unwrap_or(0);
        self.note_width(sheet, column, longest);
        self.set(sheet, row, column, Cell::SharedString { si, s: style })
    }

    /// Write a number cell, which its style's number format shows (a date is its Excel serial number).
    ///
    /// # Errors
    ///
    /// Returns an error for a sheet that was not added or a cell outside the sheet.
    pub fn number(&mut self, sheet: usize, row: i32, column: i32, number: f64, style: i32) -> Result<(), Error> {
        // a width for the fit: the digits of the plain number, or a date's
        let shown = format!("{number:.2}").len().max(10);
        self.note_width(sheet, column, shown);
        self.set(sheet, row, column, Cell::NumberCell { v: number, s: style })
    }

    fn set(&mut self, sheet: usize, row: i32, column: i32, cell: Cell) -> Result<(), Error> {
        if !(1..=1_048_576).contains(&row) || !(1..=16_384).contains(&column) {
            return Err(Error::new(format!("Cell {row}, {column} is outside the sheet")));
        }
        let worksheet = self
            .model
            .workbook
            .worksheets
            .get_mut(sheet)
            .ok_or_else(|| Error::new("No such sheet"))?;
        worksheet.sheet_data.set_cell(row, column, cell);
        Ok(())
    }

    fn note_width(&mut self, sheet: usize, column: i32, chars: usize) {
        if let Some(widths) = self.widths.get_mut(sheet) {
            let width = widths.entry(column).or_insert(0);
            *width = (*width).max(chars);
        }
    }

    /// Keep the first `rows` rows of a sheet in view.
    ///
    /// # Errors
    ///
    /// Returns an error for a sheet that was not added.
    pub fn freeze_rows(&mut self, sheet: usize, rows: i32) -> Result<(), Error> {
        let worksheet = self
            .model
            .workbook
            .worksheets
            .get_mut(sheet)
            .ok_or_else(|| Error::new("No such sheet"))?;
        worksheet.set_frozen_rows(rows).map_err(engine_error)
    }

    /// Set each column of a sheet to the width of its widest text, by a character count (the engine measures no font).
    ///
    /// # Errors
    ///
    /// Returns an error for a sheet that was not added.
    pub fn fit_columns(&mut self, sheet: usize) -> Result<(), Error> {
        let widths = self.widths.get(sheet).cloned().unwrap_or_default();
        let worksheet = self
            .model
            .workbook
            .worksheets
            .get_mut(sheet)
            .ok_or_else(|| Error::new("No such sheet"))?;
        let mut columns: Vec<(i32, usize)> = widths.into_iter().collect();
        columns.sort_unstable();
        for (column, chars) in columns {
            #[allow(clippy::cast_precision_loss)]
            let pixels = (chars as f64 * FIT_CHAR + FIT_PAD).clamp(FIT_MIN, FIT_MAX);
            worksheet.set_column_width(column, pixels).map_err(engine_error)?;
        }
        Ok(())
    }

    /// The engine's model of the workbook, to calculate or edit it.
    ///
    /// # Errors
    ///
    /// Returns the engine's error when the workbook is not one it takes.
    pub fn into_model(self) -> Result<Model<'static>, Error> {
        Model::from_workbook(self.model.workbook, "en").map_err(engine_error)
    }

    /// The `.xlsx` file.
    ///
    /// # Errors
    ///
    /// Returns the engine's error.
    pub fn to_xlsx(self) -> Result<Vec<u8>, Error> {
        model_xlsx(&self.into_model()?)
    }
}

/// A model as an `.xlsx` file. Formulas are written with the values they have now: evaluate first.
///
/// # Errors
///
/// Returns the writer's error.
pub fn model_xlsx(model: &Model<'_>) -> Result<Vec<u8>, Error> {
    ironcalc::export::save_xlsx_to_writer(model, std::io::Cursor::new(Vec::new()))
        .map(std::io::Cursor::into_inner)
        .map_err(|err| Error::new(format!("Error writing xlsx: {err}")))
}

/// The engine's model of an `.xlsx` file, with its formulas, styles and names, not yet evaluated: the cells hold the
/// values the file had. The engine reads xlsx only, and refuses a file without the parts Excel writes (a styles
/// part); `read#Spreadsheet` reads the other formats and such files, as values.
///
/// # Errors
///
/// Returns the reader's error.
pub fn open_xlsx(bytes: &[u8], name: &str) -> Result<Model<'static>, Error> {
    let workbook = ironcalc::import::load_from_xlsx_bytes(bytes, name, "en", "UTC")
        .map_err(|err| Error::new(format!("Error reading xlsx: {err}")))?;
    Model::from_workbook(workbook, "en").map_err(engine_error)
}

/// A cell or a range of one sheet, rows and columns from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub sheet: u32,
    pub row: i32,
    pub column: i32,
    pub last_row: i32,
    pub last_column: i32,
}

impl Area {
    /// One cell, not a range.
    #[must_use]
    pub fn is_cell(&self) -> bool {
        self.row == self.last_row && self.column == self.last_column
    }
}

/// `B2` as (row, column), `$` signs allowed.
#[must_use]
pub fn parse_cell(text: &str) -> Option<(i32, i32)> {
    let plain: String = text.chars().filter(|ch| *ch != '$').collect();
    let split = plain.find(|ch: char| ch.is_ascii_digit())?;
    let (letters, digits) = plain.split_at(split);
    if letters.is_empty() || !letters.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    let column = column_to_number(&letters.to_ascii_uppercase()).ok()?;
    let row: i32 = digits.parse().ok()?;
    ((1..=1_048_576).contains(&row) && (1..=16_384).contains(&column)).then_some((row, column))
}

/// `B2`, `A1:C3`, `Inputs!B2` or `'My sheet'!A1:C3` in `model`; without a sheet, the first.
///
/// # Errors
///
/// Returns an error for a text that is neither, or a sheet the workbook does not have.
pub fn parse_area(model: &Model<'_>, reference: &str) -> Result<Area, Error> {
    let bad = || Error::new(format!("{reference} is not a cell or a range (B2, Inputs!B2, A1:C3)"));
    let reference = reference.trim();
    let (sheet, cells) = match reference.rsplit_once('!') {
        Some((name, cells)) => {
            let name = name.trim().trim_matches('\'').replace("''", "'");
            let names = model.workbook.get_worksheet_names();
            let index = names
                .iter()
                .position(|found| found.eq_ignore_ascii_case(&name))
                .ok_or_else(|| {
                    Error::new(format!(
                        "The spreadsheet has no sheet named {name}, it has {}",
                        names.join(", ")
                    ))
                })?;
            (u32::try_from(index).map_err(|_| bad())?, cells)
        }
        None => (0, reference),
    };
    let (first, last) = cells.split_once(':').unwrap_or((cells, cells));
    let (row, column) = parse_cell(first.trim()).ok_or_else(bad)?;
    let (last_row, last_column) = parse_cell(last.trim()).ok_or_else(bad)?;
    Ok(Area {
        sheet,
        row: row.min(last_row),
        column: column.min(last_column),
        last_row: row.max(last_row),
        last_column: column.max(last_column),
    })
}

fn engine_error(message: String) -> Error {
    Error::new(format!("Spreadsheet engine: {message}"))
}

#[cfg(test)]
#[path = "workbook_tests.rs"]
mod tests;
