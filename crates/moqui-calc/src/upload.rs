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

//! A workbook an author uploads as a new document's overlay, its data sheets over a source
//! (plans/IronCalc-DB2.md CG4.4; docs/tools/Database.md, CalcGrid and access profiles): the workbook's sheets,
//! formulas and styles are the author's, and each data sheet is filled from the source's rows as the reader may read
//! them ([`crate::fill::fill`], which adds a sheet the workbook lacks).
//!
//! The workbook must not name what a data sheet is. A sheet of a data sheet's name would be taken as the stored data
//! sheet, its rows below the data kept in it and read by the formulas over the data as if they were the source's; a
//! table, a defined name or a named function of the data sheet's table name would stand where `Orders[amount]` names
//! the data. So each of those is refused ([`check_overlay`]), and the data a formula of the workbook reads is only the
//! rows the reader's access gives. Everything else of the workbook is its author's own, as if typed: its cell values
//! (which the author could read), formulas over its own sheets and over the data sheets, defined names over them. The
//! engine reads no external workbook and fetches nothing, so a reference into another file is an error value, and a
//! hyperlink is text the view does not open. Its size is held by the file limit, the inflated-size check and the
//! stored overlay's 8 MB (service.rs `open_document_with`).

use moqui_core::context::Error;
use moqui_core::workbook::ironcalc::base::types::Cell;
use moqui_core::workbook::ironcalc::base::Model;

use crate::fill::{self, table_name, SheetRows};

fn same(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// Why `model`, an uploaded workbook, may not be the overlay of a document whose data sheets are `data_sheets`, or
/// None: a sheet named as a data sheet, or a table, a defined name or a named function named as a data sheet's table
/// ([`table_name`]), case aside, as the engine compares names.
#[must_use]
pub fn overlay_refusal(model: &Model<'_>, data_sheets: &[String]) -> Option<String> {
    let workbook = &model.workbook;
    for sheet in data_sheets {
        let table = table_name(sheet);
        if let Some(found) = workbook
            .get_worksheet_names()
            .into_iter()
            .find(|name| same(name, sheet))
        {
            return Some(format!(
                "The workbook has a sheet named {found}, which is the data sheet's name; rename the sheet first"
            ));
        }
        let tables = workbook
            .tables
            .iter()
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
