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

//! `org.moqui.resource.SpreadsheetServices.calculate#Spreadsheet`: calculation over a workbook (plans/IronCalc-DB.md,
//! Phase 1). An xlsx with formulas is the template: the service sets input cells, evaluates the workbook on the
//! IronCalc engine ([`crate::workbook`]) and returns the cells asked for, and the calculated workbook as bytes or
//! written to a location.
//!
//! A cell is named as Excel names it: `B2`, or `Inputs!B2` on a sheet other than the first, and a range `A1:C3` to
//! read. A text value is set as a person typing it would (so `=B2*2` is a formula and `12` a number); a number and a
//! boolean are set as they are; null clears the cell. A cell keeps its style, so a number set in a cell formatted as a
//! date or a currency shows as one.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use ironcalc::base::cell::CellValue;
use ironcalc::base::Model;
use moqui_lib::error::Error as LibError;
use moqui_lib::format::ToPrimitive;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::service::util::param_text;
use crate::workbook::{self, ironcalc, Area};

/// The most cells one range of `readCells` may hold.
const MAX_RANGE_CELLS: i64 = 100_000;

pub(super) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, calculate_spreadsheet)
}

fn fail(message: String) -> LibError {
    LibError::new(format!("Spreadsheet engine: {message}"))
}

fn parse_area(model: &Model<'_>, reference: &str) -> Result<Area, LibError> {
    workbook::parse_area(model, reference).map_err(|err| LibError::new(err.message()))
}

/// Set one cell as the module doc says.
fn set_cell(model: &mut Model<'_>, area: Area, value: &Value) -> Result<(), LibError> {
    let (sheet, row, column) = (area.sheet, area.row, area.column);
    match value {
        Value::Null => model.set_user_input(sheet, row, column, String::new()),
        Value::Bool(flag) => model.update_cell_with_bool(sheet, row, column, *flag),
        Value::I32(number) => model.update_cell_with_number(sheet, row, column, f64::from(*number)),
        #[allow(clippy::cast_precision_loss)]
        Value::I64(number) => model.update_cell_with_number(sheet, row, column, *number as f64),
        Value::F64(number) => model.update_cell_with_number(sheet, row, column, *number),
        Value::Decimal(number) => match number.to_f64() {
            Some(found) => model.update_cell_with_number(sheet, row, column, found),
            None => model.set_user_input(sheet, row, column, number.to_string()),
        },
        other => model.set_user_input(sheet, row, column, to_plain_string(other)),
    }
    .map_err(fail)
}

/// A cell's value as `read#Spreadsheet` types it: text, a number (a whole number is a `Long`), a boolean, or null.
fn cell_value(model: &Model<'_>, sheet: u32, row: i32, column: i32) -> Result<Value, LibError> {
    Ok(match model.get_cell_value_by_index(sheet, row, column).map_err(fail)? {
        CellValue::None => Value::Null,
        CellValue::String(text) => Value::String(text),
        CellValue::Boolean(flag) => Value::Bool(flag),
        #[allow(clippy::cast_possible_truncation)]
        CellValue::Number(number) if number.fract() == 0.0 && number.abs() < super::WHOLE_LIMIT => {
            Value::I64(number as i64)
        }
        CellValue::Number(number) => Value::F64(number),
    })
}

/// The value and the formatted text of a cell, or of each cell of a range as a list of rows.
fn read_area(model: &Model<'_>, area: Area, reference: &str) -> Result<(Value, Value), LibError> {
    if area.is_cell() {
        return Ok((
            cell_value(model, area.sheet, area.row, area.column)?,
            Value::String(
                model
                    .get_formatted_cell_value(area.sheet, area.row, area.column)
                    .map_err(fail)?,
            ),
        ));
    }
    let cells = i64::from(area.last_row - area.row + 1) * i64::from(area.last_column - area.column + 1);
    if cells > MAX_RANGE_CELLS {
        return Err(LibError::new(format!(
            "{reference} has {cells} cells; a range to read has at most {MAX_RANGE_CELLS}"
        )));
    }
    let (mut values, mut texts) = (Vec::new(), Vec::new());
    for row in area.row..=area.last_row {
        let (mut value_row, mut text_row) = (Vec::new(), Vec::new());
        for column in area.column..=area.last_column {
            value_row.push(cell_value(model, area.sheet, row, column)?);
            text_row.push(Value::String(
                model.get_formatted_cell_value(area.sheet, row, column).map_err(fail)?,
            ));
        }
        values.push(Value::List(value_row));
        texts.push(Value::List(text_row));
    }
    Ok((Value::List(values), Value::List(texts)))
}

/// What [`calculate`] gives: the out-parameters but the file, and the calculated workbook's bytes when asked for.
type Calculated = (BTreeMap<String, Value>, Option<Vec<u8>>);

/// The work of `calculate#Spreadsheet` on the file's bytes, with the calculated workbook when `want_file`.
fn calculate(bytes: &[u8], parameters: &BTreeMap<String, Value>, want_file: bool) -> Result<Calculated, LibError> {
    let mut model = workbook::open_xlsx(bytes, "workbook").map_err(|err| LibError::new(err.message()))?;
    if let Some(Value::Map(cells)) = parameters.get("cellValues") {
        for (reference, value) in cells {
            let area = parse_area(&model, reference)?;
            if !area.is_cell() {
                return Err(LibError::new(format!(
                    "{reference} is a range; a value is set in one cell"
                )));
            }
            set_cell(&mut model, area, value)?;
        }
    }
    model.evaluate();

    let (mut values, mut texts) = (IndexMap::default(), IndexMap::default());
    if let Some(Value::List(references)) = parameters.get("readCells") {
        for reference in references.iter().filter(|reference| !is_empty(reference)) {
            let reference = to_plain_string(reference);
            let (value, text) = read_area(&model, parse_area(&model, &reference)?, &reference)?;
            values.insert(reference.clone(), value);
            texts.insert(reference, text);
        }
    }
    let mut out = BTreeMap::new();
    out.insert(
        "sheetNameList".to_string(),
        Value::List(
            model
                .workbook
                .get_worksheet_names()
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
    );
    out.insert("valueMap".to_string(), Value::Map(values));
    out.insert("textMap".to_string(), Value::Map(texts));
    let written = if want_file {
        Some(workbook::model_xlsx(&model).map_err(|err| LibError::new(err.message()))?)
    } else {
        None
    };
    Ok((out, written))
}

/// `calculate#Spreadsheet`: set `cellValues`, evaluate, read `readCells`, and give the workbook back when asked.
fn calculate_spreadsheet(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let bytes = super::file_bytes(parameters)?;
    let output_location = param_text(parameters, "outputLocation");
    let return_bytes = super::flag(parameters, "returnBytes", false);
    let (mut out, written) = calculate(&bytes, parameters, return_bytes || !output_location.is_empty())?;
    if let Some(written) = written {
        if !output_location.is_empty() {
            ExecutionContext::with_current(|ctx| ctx.factory().resource().write_bytes(&output_location, &written))
                .ok_or_else(|| LibError::new("no execution context is entered"))?
                .map_err(|err| LibError::new(err.message()))?;
        }
        if return_bytes {
            out.insert("contentBytes".to_string(), Value::Bytes(written));
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "spreadsheet_calc_tests.rs"]
mod tests;
