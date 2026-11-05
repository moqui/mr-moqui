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

//! The spreadsheet reading of a build without the `xlsx` feature: the same API, each read an error that says so.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

const NOT_BUILT: &str = "Spreadsheets are not built into this server (the xlsx feature)";

/// One sheet read whole; see the `xlsx` build's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetData {
    pub sheet_names: Vec<String>,
    pub sheet_name: String,
    pub rows: Vec<Vec<Value>>,
    pub row_numbers: Vec<u32>,
}

/// The first rows of one sheet, for a preview.
#[derive(Debug, Default, PartialEq)]
pub struct SheetPreview {
    pub sheet_names: Vec<String>,
    pub sheet_name: String,
    pub rows: Vec<Vec<Value>>,
    pub truncated: bool,
}

/// # Errors
///
/// Always: spreadsheets are not built in.
pub fn read_sheet_bytes(_bytes: &[u8], _sheet_name: &str, _sheet_index: usize) -> Result<SheetData, LibError> {
    Err(LibError::new(NOT_BUILT))
}

/// # Errors
///
/// Always: spreadsheets are not built in.
pub fn preview_rows(
    _bytes: &[u8],
    _sheet_name: &str,
    _max_rows: usize,
    _max_cols: usize,
) -> Result<SheetPreview, LibError> {
    Err(LibError::new(NOT_BUILT))
}

/// A run of an inline ETL mapping in place of `load#SpreadsheetEntity`, which this build does not have.
pub type InlineLoad = fn(&BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError>;

/// Nothing to install into: this build has no `load#SpreadsheetEntity`.
pub fn install_inline_load(_load: InlineLoad) {}
