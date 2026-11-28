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

//! The workbook of a document beside its data sheets, its overlay, wherever it came from: one an author uploads as a
//! new document's (plans/IronCalc-DB2.md CG4.4; docs/tools/Database.md, CalcGrid and access profiles), and a saved
//! document's stored overlay, which a definition written without CalcGrid (the definition editor's JSON, an import, a
//! git pull, the report REST resources) may hold too. Its sheets, formulas and styles are the author's, and each data
//! sheet is filled from the source's rows as the reader may read them ([`crate::fill::fill`], which adds a sheet the
//! workbook lacks).
//!
//! The overlay must not name what a data sheet is. A sheet of a data sheet's name is taken as the stored data sheet,
//! and rows of it below the data would stay in it and be read by the formulas over the data as if they were the
//! source's; a table, a defined name or a named function of the data sheet's table name would stand where
//! `Orders[amount]` names the data. An uploaded workbook with any of those is refused ([`overlay_refusal`]); a saved
//! one has its data sheets set back to their stored form (no cell below the header, nothing drawn over the rows or
//! hiding them), a sheet that only looks like a data sheet renamed, and is refused for a table or name of a data
//! sheet's table name that is not the data sheet's own ([`saved_overlay`]). So the data a formula reads, and what a
//! reader sees of it, is only the rows the reader's access gives. A sheet is not renamed like a data sheet in a room
//! either (room.rs `apply`). Names are compared in Unicode's composed form, without the characters a name may hold
//! unseen (zero-width, bidirectional, variation selectors, fillers, tags), trimmed and in capitals as the engine
//! compares sheet names ([`name_key`]), so a tab that only differs by those is the data sheet's name; one of other
//! letters that look alike is not caught, and holds only what its author typed. Everything else of the overlay is its author's own, as if typed: its
//! cell values (which the author could read), formulas over its own sheets and over the data sheets, defined names
//! over them. The engine reads no external workbook and fetches nothing, so a reference into another file is an error
//! value, and a hyperlink is text the view does not open. An upload's size is held by the file limit, the
//! inflated-size check and the stored overlay's 8 MB ([`prepare_for_room`]).

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::types::{Cell, Col, Row, SheetState, Worksheet};
use moqui_core::workbook::ironcalc::base::{
    Model, COLUMN_WIDTH_FACTOR, DEFAULT_COLUMN_WIDTH, DEFAULT_ROW_HEIGHT, ROW_HEIGHT_FACTOR,
};

use crate::fill::{self, table_name, SheetRows};
use crate::service::{MAX_OVERLAY_BYTES, MAX_WORKBOOK_BYTES};

/// Whether `ch` is one a name may hold without showing it: Unicode's Default_Ignorable_Code_Point set (the soft hyphen,
/// the combining grapheme joiner, the Hangul fillers, the Khmer inherent vowels, the Mongolian variation selectors and
/// vowel separator, the zero-width ones, the bidirectional marks, embeddings and isolates, the word joiner and
/// invisible operators, the variation selectors, the byte order mark, the shorthand format controls, the musical
/// formatting symbols, the tags) and the interlinear annotation marks.
fn is_ignorable(ch: char) -> bool {
    matches!(ch,
        '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{115f}'..='\u{1160}' | '\u{17b4}'..='\u{17b5}' | '\u{180b}'..='\u{180f}'
        | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{3164}'
        | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}' | '\u{fff0}'..='\u{fffb}' | '\u{1bca0}'..='\u{1bca3}'
        | '\u{1d173}'..='\u{1d17a}' | '\u{e0000}'..='\u{e0fff}')
}

/// A name as names are compared here: without the characters it may hold unseen ([`is_ignorable`]), first, so one
/// between a letter and its accent does not keep them apart, then in Unicode's composed form (NFC), trimmed, in
/// capitals as the engine compares sheet names. A name of other letters that only look
/// alike (a Cyrillic `О` for a Latin `O`) is another name: it is not caught, and needs not be, since such a sheet is
/// never taken as a data sheet and holds only what its author typed.
pub(crate) fn name_key(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.chars()
        .filter(|ch| !is_ignorable(*ch))
        .nfc()
        .collect::<String>()
        .trim()
        .to_uppercase()
}

/// Whether two names are one ([`name_key`]).
pub(crate) fn same(left: &str, right: &str) -> bool {
    name_key(left) == name_key(right)
}

/// Why `model` may not be the overlay of a document whose data sheets are `data_sheets`, or None. `saved`: the
/// overlay of a saved document, which holds each data sheet's stub and its table, and whose sheets that only look like
/// a data sheet are renamed, not refused ([`saved_overlay`]); else an upload, which holds none.
fn refusal(model: &Model<'_>, data_sheets: &[String], saved: bool) -> Option<String> {
    let workbook = &model.workbook;
    for sheet in data_sheets {
        let table = table_name(sheet);
        // the sheet a fill takes as the data sheet's, which a saved overlay holds
        let own = |name: &str| saved && name.eq_ignore_ascii_case(sheet);
        if let Some(found) = workbook
            .get_worksheet_names()
            .into_iter()
            .find(|name| !saved && same(name, sheet))
        {
            return Some(format!(
                "The workbook has a sheet named {found}, which is the data sheet's name; rename the sheet first"
            ));
        }
        let tables = workbook
            .tables
            .iter()
            .filter(|(_, found)| !own(&found.sheet_name))
            .flat_map(|(key, found)| [key.as_str(), found.name.as_str(), found.display_name.as_str()]);
        if let Some(found) = tables.into_iter().find(|name| same(name, &table)) {
            return Some(format!(
                "The workbook has a table named {found}, which names the data sheet's rows; rename the table first"
            ));
        }
        let names = workbook
            .defined_names
            .iter()
            .map(|defined| defined.name.as_str())
            .chain(workbook.named_functions.iter().map(|function| function.name.as_str()));
        if let Some(found) = names.into_iter().find(|name| same(name, &table)) {
            return Some(format!(
                "The workbook has a name {found}, which names the data sheet's rows; rename it first"
            ));
        }
    }
    None
}

/// Why `model`, an uploaded workbook, may not be the overlay of a document whose data sheets are `data_sheets`, or
/// None: a sheet named as a data sheet, or a table, a defined name or a named function named as a data sheet's table
/// ([`table_name`]), compared as [`name_key`] does.
#[must_use]
pub fn overlay_refusal(model: &Model<'_>, data_sheets: &[String]) -> Option<String> {
    refusal(model, data_sheets, false)
}

/// The narrowest a data sheet's column is, in pixels: a width below it would hide the column for every reader, as
/// hiding it would, so a room's width edit of a data sheet is held to it and [`MAX_DATA_WIDTH`] (room.rs `apply`) and
/// a saved overlay's size out of them is not kept.
pub(crate) const MIN_DATA_WIDTH: f64 = 20.0;
/// The lowest a data sheet's row is, in pixels (as [`MIN_DATA_WIDTH`]).
pub(crate) const MIN_DATA_HEIGHT: f64 = 12.0;
/// The widest a data sheet's column is, in pixels: Excel's 255 characters. A wider one, or a taller row than
/// [`MAX_DATA_HEIGHT`], would push the data past it out of reach.
pub(crate) const MAX_DATA_WIDTH: f64 = 255.0 * COLUMN_WIDTH_FACTOR;
/// The tallest a data sheet's row is, in pixels: Excel's 409 points.
pub(crate) const MAX_DATA_HEIGHT: f64 = 409.0 * 4.0 / 3.0;

/// A data sheet's size, in pixels, within its bounds: one that is not a number is `fallback`.
pub(crate) fn data_size(pixels: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if pixels.is_finite() {
        pixels.clamp(min, max)
    } else {
        fallback
    }
}

/// Whether a size, in pixels, is out of a data sheet's bounds, or not a number.
fn out_of_bounds(pixels: f64, min: f64, max: f64) -> bool {
    !(pixels >= min && pixels <= max)
}

fn too_narrow(col: &Col) -> bool {
    out_of_bounds(col.width * COLUMN_WIDTH_FACTOR, MIN_DATA_WIDTH, MAX_DATA_WIDTH)
}

fn too_low(row: &Row) -> bool {
    out_of_bounds(row.height * ROW_HEIGHT_FACTOR, MIN_DATA_HEIGHT, MAX_DATA_HEIGHT)
}

/// Whether a data sheet of a saved overlay holds what its stored form does not: a cell below its header, or what a
/// view draws over its rows or hides of them (conditional formats, merged cells, comments, drawings, links, a row's or
/// column's style or hiding, a width or height that hides one, the sheet hidden). Its widths, heights and frozen panes
/// are an author's to set.
fn holds_more(worksheet: &Worksheet) -> bool {
    worksheet.sheet_data.rows().into_iter().any(|row| row > 1)
        || !worksheet.conditional_formatting.is_empty()
        || !worksheet.merged_cells.is_empty()
        || !worksheet.comments.is_empty()
        || !worksheet.drawings.is_empty()
        || !worksheet.links.is_empty()
        || !matches!(worksheet.state, SheetState::Visible)
        || worksheet
            .rows
            .iter()
            .any(|row| row.hidden || row.s != 0 || row.custom_format || too_low(row))
        || worksheet
            .cols
            .iter()
            .any(|col| col.hidden || col.style.is_some() || too_narrow(col))
}

/// A data sheet of a saved overlay back to its stored form: no cell below its header, nothing drawn over its rows or
/// hiding them; its widths, heights and frozen panes kept.
fn to_stored_form(worksheet: &mut Worksheet) {
    for row in worksheet.sheet_data.rows().into_iter().filter(|row| *row > 1) {
        worksheet.sheet_data.remove_row(row);
    }
    worksheet.conditional_formatting.clear();
    worksheet.merged_cells.clear();
    worksheet.comments.clear();
    worksheet.drawings.clear();
    worksheet.links.clear();
    worksheet.state = SheetState::Visible;
    for row in &mut worksheet.rows {
        row.hidden = false;
        row.s = 0;
        row.custom_format = false;
        if too_low(row) {
            row.height = DEFAULT_ROW_HEIGHT / ROW_HEIGHT_FACTOR;
            row.custom_height = false;
        }
    }
    for col in &mut worksheet.cols {
        col.hidden = false;
        col.style = None;
        if too_narrow(col) {
            col.width = DEFAULT_COLUMN_WIDTH / COLUMN_WIDTH_FACTOR;
            col.custom_width = false;
        }
    }
}

/// The most characters of a sheet's name (the engine's `is_valid_sheet_name`).
const MAX_SHEET_NAME: usize = 31;

/// Rename the sheet at `index`, one that looks like the data sheet `data`, to a name no sheet and no data sheet has:
/// `<data> (renamed)`, then `(renamed 2)` and on, `data` cut so the name fits the engine's 31 characters, else
/// `Renamed <n>` ([`rename_aside`]). The name the engine took, or None when it took none (each refusal logged).
fn rename_lookalike(model: &mut Model<'static>, index: u32, data: &str, data_sheets: &[String]) -> Option<String> {
    let fits = |suffix: &str| {
        let room = MAX_SHEET_NAME.saturating_sub(suffix.chars().count());
        let cut: String = data.chars().take(room).collect();
        format!("{}{suffix}", cut.trim_end())
    };
    let named = (1..=20).map(|n| {
        fits(&if n == 1 {
            " (renamed)".to_string()
        } else {
            format!(" (renamed {n})")
        })
    });
    rename_to_first(model, index, named, data_sheets).or_else(|| rename_aside(model, index, data_sheets))
}

/// Rename the sheet at `index` to `Renamed <n>`, the first `n` no sheet and no data sheet has: there is always one
/// among the first sheets-and-data-sheets-and-one, each within the engine's 31 characters. The name the engine took,
/// or None when it refused every one (each refusal logged).
pub(crate) fn rename_aside(model: &mut Model<'static>, index: u32, data_sheets: &[String]) -> Option<String> {
    let count = model.workbook.worksheets.len() + data_sheets.len() + 1;
    let plain = (1..=count).map(|n| format!("Renamed {n}"));
    rename_to_first(model, index, plain, data_sheets)
}

/// Rename the sheet at `index` to the first of `candidates` that no sheet and no data sheet has and the engine takes.
fn rename_to_first(
    model: &mut Model<'static>,
    index: u32,
    candidates: impl Iterator<Item = String>,
    data_sheets: &[String],
) -> Option<String> {
    for renamed in candidates {
        let taken = model.workbook.get_worksheet_names();
        if taken.iter().any(|one| same(one, &renamed)) || data_sheets.iter().any(|sheet| same(sheet, &renamed)) {
            continue;
        }
        match model.rename_sheet_by_index(index, &renamed) {
            Ok(()) => return Some(renamed),
            Err(error) => {
                tracing::warn!(renamed = %renamed, %error, "The engine refused a sheet's new name; trying another")
            }
        }
    }
    None
}

/// A saved document's overlay as a room opens it, whatever wrote it (plans/IronCalc-DB2.md CG4 review): refused for a
/// table, defined name or named function of a data sheet's table name that is not on the data sheet; a sheet that only
/// looks like a data sheet ([`same`]) renamed, with a warning to the opener, so no document is past opening for a tab
/// name; and each data sheet that holds more than its stored form ([`holds_more`]) set back to it, so a fill keeps no
/// row and draws nothing the overlay brought. The cells are taken out and no row is deleted, so no formula's range
/// moves. A stored form CalcGrid wrote holds none, and is answered as it is.
///
/// # Errors
///
/// The refusal, or the engine's error.
pub fn saved_overlay(mut model: Model<'static>, data_sheets: &[String]) -> Result<Model<'static>, Error> {
    let engine = |message: String| Error::new(format!("Spreadsheet engine: {message}"));
    if let Some(refused) = refusal(&model, data_sheets, true) {
        return Err(Error::new(refused));
    }
    let is_data = |name: &str| data_sheets.iter().any(|sheet| sheet.eq_ignore_ascii_case(name));
    let names = model.workbook.get_worksheet_names();
    for (index, name) in names.iter().enumerate() {
        let Some(data) = data_sheets.iter().find(|sheet| !is_data(name) && same(sheet, name)) else {
            continue;
        };
        let index = u32::try_from(index).map_err(|_| Error::new("Too many sheets"))?;
        let Some(renamed) = rename_lookalike(&mut model, index, data, data_sheets) else {
            // the engine took no name: the tab stays as it is, which no fill takes for the data sheet (fill.rs
            // `sheet_index`), and the document still opens
            tracing::error!(sheet = %name, "A saved CalcGrid overlay's sheet named like a data sheet could not be renamed; it is left");
            continue;
        };
        tracing::warn!(sheet = %name, renamed = %renamed, "A saved CalcGrid overlay held a sheet named like a data sheet; it is renamed");
        let message = format!("The sheet {name:?} looked like the data sheet {data} and is renamed {renamed}; save the document to keep it so");
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().add_message_typed(message, "warning"));
    }
    let more = |worksheet: &Worksheet| is_data(&worksheet.get_name()) && holds_more(worksheet);
    if !model.workbook.worksheets.iter().any(more) {
        return Ok(model);
    }
    let mut workbook = model.workbook;
    for worksheet in &mut workbook.worksheets {
        if more(worksheet) {
            tracing::warn!(sheet = %worksheet.get_name(), "A saved CalcGrid overlay held more than a data sheet's stored form; it is set back to it");
            to_stored_form(worksheet);
        }
    }
    Model::from_workbook(workbook, "en").map_err(engine)
}

/// An uploaded workbook's file name as a title may take it: its last part, no control character, at most 200.
#[must_use]
pub fn workbook_name(given: &str) -> String {
    let name: String = given
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_control())
        .collect();
    let name = name.trim();
    let skip = name.chars().count().saturating_sub(200);
    let name: String = name.chars().skip(skip).collect();
    if name.is_empty() || name == "." || name == ".." {
        "Workbook".to_string()
    } else {
        name
    }
}

/// An uploaded workbook as a model, at most [`MAX_WORKBOOK_BYTES`] and read as a workbook file is
/// (`service::workbook_model`: inflated within 64 MiB first), refused when it names a data sheet's
/// ([`overlay_refusal`]), with its title: `title`, else its file name without the extension. A sheet that could not be
/// read is named to the opener as a warning.
///
/// # Errors
///
/// The size, the reader's error, or the refusal.
pub fn open(
    bytes: &[u8],
    file_name: &str,
    title: &str,
    data_sheets: &[String],
) -> Result<(Model<'static>, String), Error> {
    if bytes.len() > MAX_WORKBOOK_BYTES {
        return Err(Error::new(format!(
            "A workbook to open has at most {} MB",
            MAX_WORKBOOK_BYTES / 1024 / 1024
        )));
    }
    let name = workbook_name(file_name);
    let (model, left_out) = crate::service::workbook_model(bytes, &name)?;
    if !left_out.is_empty() {
        let message = format!("Not opened, as they could not be read: {}", left_out.join(", "));
        moqui_core::context::ExecutionContext::with_current(|ec| ec.message().add_message_typed(message, "warning"));
    }
    if let Some(refused) = overlay_refusal(&model, data_sheets) {
        return Err(Error::new(refused));
    }
    let title = Some(title.trim())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem))
        .to_string();
    Ok((model, title))
}

/// An uploaded workbook over the data sheets whose rows are `rows`, as a room opens it: [`prepare`]d, and held to
/// what a document stores without its data ([`MAX_OVERLAY_BYTES`]).
///
/// # Errors
///
/// The engine's error, or the size refusal.
pub fn prepare_for_room(model: Model<'static>, rows: &[SheetRows]) -> Result<Model<'static>, Error> {
    let prepared = prepare(model, rows)?;
    let size = prepared.to_bytes().len();
    if size > MAX_OVERLAY_BYTES {
        return Err(Error::new(format!(
            "The workbook without its data is {} MB; a document stores at most {} MB",
            size / 1024 / 1024,
            MAX_OVERLAY_BYTES / 1024 / 1024
        )));
    }
    Ok(prepared)
}

/// An uploaded workbook made ready to open as the overlay of the data sheets `data` (their columns as their source
/// gave them; no row of theirs is written): each data sheet is added in its stored form, its header and the stub's
/// empty rows ([`fill::fill`] with no row), and each formula of the workbook that named a data sheet's table before
/// the sheet was there is typed again. The engine reads a table's name (`Orders[amount]`) only as a formula is typed,
/// turning it into the table's range, so such a formula read from the file is an error until then; typed again over
/// the stub, its range stretches with the rows a fill inserts, as the formulas of a saved document's do.
///
/// # Errors
///
/// The engine's error: a sheet name it refuses.
pub fn prepare(model: Model<'static>, data: &[SheetRows]) -> Result<Model<'static>, Error> {
    let stubs: Vec<SheetRows> = data
        .iter()
        .map(|sheet| SheetRows {
            name: sheet.name.clone(),
            columns: sheet.columns.clone(),
            rows: Vec::new(),
            truncated: false,
        })
        .collect();
    let mut model = fill::fill(model, &stubs)?;
    model.evaluate();
    let mut again = Vec::new();
    for (index, worksheet) in model.workbook.worksheets.iter().enumerate() {
        let Ok(sheet) = u32::try_from(index) else {
            break;
        };
        if data.iter().any(|one| same(&one.name, &worksheet.get_name())) {
            continue;
        }
        for (row, column, cell) in worksheet.sheet_data.cells() {
            let failed = matches!(cell.as_ref(), Cell::CellFormula { .. })
                && model
                    .get_formatted_cell_value(sheet, row, column)
                    .is_ok_and(|shown| shown == "#ERROR!");
            if let (true, Ok(Some(formula))) = (failed, model.get_cell_formula(sheet, row, column)) {
                again.push((sheet, row, column, formula));
            }
        }
    }
    for (sheet, row, column, formula) in again {
        model
            .set_user_input(sheet, row, column, formula)
            .map_err(|message| Error::new(format!("Spreadsheet engine: {message}")))?;
    }
    Ok(model)
}

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
