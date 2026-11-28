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
//! one has its data sheets cut back to their stored stub, and is refused for a table or name of a data sheet's table
//! name that is not the data sheet's own, or a sheet that only looks like a data sheet ([`saved_overlay`]). So the data
//! a formula reads is only the rows the reader's access gives. Names are compared as the engine compares sheet names
//! (in capitals), after trimming and without format characters (zero-width and bidirectional marks), so a tab that
//! only looks like the data sheet's is one too. Everything else of the overlay is its author's own, as if typed: its
//! cell values (which the author could read), formulas over its own sheets and over the data sheets, defined names
//! over them. The engine reads no external workbook and fetches nothing, so a reference into another file is an error
//! value, and a hyperlink is text the view does not open. An upload's size is held by the file limit, the
//! inflated-size check and the stored overlay's 8 MB ([`prepare_for_room`]).

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::types::Cell;
use moqui_core::workbook::ironcalc::base::Model;

use crate::fill::{self, table_name, SheetRows};
use crate::service::{MAX_OVERLAY_BYTES, MAX_WORKBOOK_BYTES};

/// Whether `ch` is a format character a name may hide: a soft hyphen, the zero-width ones, the bidirectional marks,
/// embeddings and isolates, the word joiner and invisible operators, and the byte order mark.
fn is_format(ch: char) -> bool {
    matches!(ch, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}'
        | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206f}' | '\u{feff}')
}

/// A name as names are compared here: trimmed, without format characters, in capitals as the engine compares sheet
/// names.
fn name_key(name: &str) -> String {
    name.chars()
        .filter(|ch| !is_format(*ch))
        .collect::<String>()
        .trim()
        .to_uppercase()
}

fn same(left: &str, right: &str) -> bool {
    name_key(left) == name_key(right)
}

/// Why `model` may not be the overlay of a document whose data sheets are `data_sheets`, or None. `saved`: the
/// overlay of a saved document, which holds each data sheet's stub and its table; else an upload, which holds none.
fn refusal(model: &Model<'_>, data_sheets: &[String], saved: bool) -> Option<String> {
    let workbook = &model.workbook;
    for sheet in data_sheets {
        let table = table_name(sheet);
        // the sheet a fill takes as the data sheet's, which a saved overlay holds
        let own = |name: &str| saved && name.eq_ignore_ascii_case(sheet);
        if let Some(found) = workbook
            .get_worksheet_names()
            .into_iter()
            .find(|name| same(name, sheet) && !own(name))
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

/// A saved document's overlay as a room opens it, whatever wrote it: refused for a sheet that only looks like a data
/// sheet, or a table, defined name or named function of a data sheet's table name that is not on the data sheet; and
/// each data sheet that holds anything below its header emptied there, as its stored form is (the header and the
/// stub's empty rows; [`fill::stored_form`]), so a fill keeps no row the overlay brought. The cells are taken out and
/// no row is deleted, so no formula's range moves. A stored form CalcGrid wrote holds none, and is answered as it is.
///
/// # Errors
///
/// The refusal, or the engine's error.
pub fn saved_overlay(model: Model<'static>, data_sheets: &[String]) -> Result<Model<'static>, Error> {
    if let Some(refused) = refusal(&model, data_sheets, true) {
        return Err(Error::new(refused));
    }
    let is_data = |name: &str| data_sheets.iter().any(|sheet| sheet.eq_ignore_ascii_case(name));
    let holds = |worksheet: &moqui_core::workbook::ironcalc::base::types::Worksheet| {
        is_data(&worksheet.get_name()) && worksheet.sheet_data.rows().into_iter().any(|row| row > 1)
    };
    if !model.workbook.worksheets.iter().any(holds) {
        return Ok(model);
    }
    let mut workbook = model.workbook;
    for worksheet in &mut workbook.worksheets {
        if !holds(worksheet) {
            continue;
        }
        tracing::warn!(sheet = %worksheet.get_name(), "A saved CalcGrid overlay held rows in a data sheet; they are taken out");
        for row in worksheet.sheet_data.rows().into_iter().filter(|row| *row > 1) {
            worksheet.sheet_data.remove_row(row);
        }
    }
    Model::from_workbook(workbook, "en").map_err(|message| Error::new(format!("Spreadsheet engine: {message}")))
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
