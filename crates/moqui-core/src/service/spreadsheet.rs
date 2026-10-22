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

//! `org.moqui.impl.SpreadsheetServices`: read a spreadsheet (xlsx, xlsm, xlsb, xls, or ods) into rows, and load those
//! rows into an entity. New with the moqui-poi port; the Java component wrote xlsx and read nothing.
//!
//! The file is `contentBytes` or the resource at `location`. The format is found from the content. A cell keeps its
//! type: text, a number (a whole number is a `Long`), a boolean, or a date as a time stamp in the user's zone. An empty
//! or error cell is null, and a row of only empty cells is skipped.
//!
//! Files previews read through [`preview_rows`] (the first rows of one sheet) and [`sheet_text`] (the text of every
//! sheet, for search). Both stop reading an xlsx or xlsb sheet as soon as they have enough.

use std::collections::BTreeMap;
use std::io::Cursor;

use super::util::{has_error, param_text};

use calamine::{Cell, Data, DataRef, Range, Reader, Sheets};
use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::service::call::{sync_with, CallOptions};

/// Whole numbers up to this size read as a `Long`; Excel keeps every number as a double.
const WHOLE_LIMIT: f64 = 9_007_199_254_740_992.0;

/// The most cells, empty ones included, the used area of an xlsx or xlsb sheet may span. calamine makes a dense grid
/// from the first used cell to the last, so two cells far apart would ask for billions.
const MAX_SHEET_CELLS: u64 = 10_000_000;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, read_spreadsheet)?;
    rust_service!(map, load_spreadsheet_entity)?;
    Ok(())
}

fn flag(parameters: &BTreeMap<String, Value>, name: &str, default: bool) -> bool {
    match parameters.get(name) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) if !is_empty(value) => {
            let text = to_plain_string(value);
            text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("y")
        }
        _ => default,
    }
}

fn number(parameters: &BTreeMap<String, Value>, name: &str) -> Option<usize> {
    match parameters.get(name) {
        Some(Value::I32(number)) => usize::try_from(*number).ok(),
        Some(Value::I64(number)) => usize::try_from(*number).ok(),
        Some(value) if !is_empty(value) => to_plain_string(value).parse().ok(),
        _ => None,
    }
}

/// The rows of a sheet: the first row as the header when `headerRow` holds, then each row.
struct Sheet {
    names: Vec<String>,
    headers: Vec<String>,
    rows: Vec<Vec<Value>>,
}

fn read_sheet(parameters: &BTreeMap<String, Value>) -> Result<Sheet, LibError> {
    let bytes = match parameters.get("contentBytes") {
        Some(Value::Bytes(bytes)) if !bytes.is_empty() => bytes.clone(),
        _ => {
            let location = param_text(parameters, "location");
            if location.is_empty() {
                return Err(LibError::new("A spreadsheet needs contentBytes or a location"));
            }
            let found = ExecutionContext::with_current(|ctx| ctx.factory().resource().get(&location))
                .ok_or_else(|| LibError::new("no execution context is entered"))?;
            match found {
                Ok(resource) if resource.exists() => {
                    resource.read_bytes().map_err(|err| LibError::new(err.message()))?
                }
                _ => return Err(LibError::new(format!("Could not find spreadsheet at [{location}]"))),
            }
        }
    };
    let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|err| LibError::new(format!("Could not read spreadsheet: {err}")))?;
    let names = book.sheet_names();
    let sheet_name = param_text(parameters, "sheetName");
    let chosen = if sheet_name.is_empty() {
        let index = number(parameters, "sheetIndex").unwrap_or(0);
        names
            .get(index)
            .cloned()
            .ok_or_else(|| LibError::new(format!("The spreadsheet has no sheet {index}, it has {}", names.len())))?
    } else if names.contains(&sheet_name) {
        sheet_name
    } else {
        return Err(LibError::new(format!(
            "The spreadsheet has no sheet named {sheet_name}, it has {}",
            names.join(", ")
        )));
    };
    let range = sheet_range(&mut book, &chosen)?;
    let max_rows = number(parameters, "maxRows");
    let (headers, rows) = sheet_rows(&range, flag(parameters, "headerRow", true), max_rows);
    Ok(Sheet { names, headers, rows })
}

/// The used area of sheet `name`. An xlsx or xlsb sheet is read cell by cell, and refused when its used area spans more
/// than [`MAX_SHEET_CELLS`]. An xls sheet is at most 65,536 rows by 256 columns, and calamine caps the cells of an ods
/// sheet.
fn sheet_range(book: &mut Sheets<Cursor<Vec<u8>>>, name: &str) -> Result<Range<Data>, LibError> {
    let failed = |err: &dyn std::fmt::Display| LibError::new(format!("Could not read sheet {name}: {err}"));
    match book {
        Sheets::Xlsx(xlsx) => {
            let mut reader = match xlsx.worksheet_cells_reader(name) {
                Ok(reader) => reader,
                Err(calamine::XlsxError::NotAWorksheet(_)) => return Ok(Range::default()),
                Err(err) => return Err(failed(&err)),
            };
            bounded_range(name, || reader.next_cell().map_err(|err| failed(&err)))
        }
        Sheets::Xlsb(xlsb) => {
            let mut reader = xlsb.worksheet_cells_reader(name).map_err(|err| failed(&err))?;
            bounded_range(name, || reader.next_cell().map_err(|err| failed(&err)))
        }
        _ => book.worksheet_range(name).map_err(|err| failed(&err)),
    }
}

/// The non-empty cells `next` reads, as calamine's `worksheet_range` makes them, or an error once their area spans
/// more than [`MAX_SHEET_CELLS`].
fn bounded_range<'a>(
    name: &str,
    mut next: impl FnMut() -> Result<Option<Cell<DataRef<'a>>>, LibError>,
) -> Result<Range<Data>, LibError> {
    let mut cells = Vec::new();
    let (mut first, mut last) = ((u32::MAX, u32::MAX), (0_u32, 0_u32));
    while let Some(cell) = next()? {
        if matches!(cell.get_value(), DataRef::Empty) {
            continue;
        }
        let (row, column) = cell.get_position();
        first = (first.0.min(row), first.1.min(column));
        last = (last.0.max(row), last.1.max(column));
        let span = (u64::from(last.0 - first.0) + 1) * (u64::from(last.1 - first.1) + 1);
        if span > MAX_SHEET_CELLS {
            return Err(LibError::new(format!(
                "Sheet {name} spans more than {MAX_SHEET_CELLS} cells, from row {} column {} to row {} column {}",
                first.0 + 1,
                first.1 + 1,
                last.0 + 1,
                last.1 + 1
            )));
        }
        cells.push(Cell::new((row, column), Data::from(cell.get_value().clone())));
    }
    Ok(Range::from_sparse(cells))
}

fn sheet_rows(range: &Range<Data>, header_row: bool, max_rows: Option<usize>) -> (Vec<String>, Vec<Vec<Value>>) {
    let zone = crate::l10n::time_zone();
    let mut rows = range.rows();
    let headers = if header_row {
        rows.next()
            .map(|row| {
                row.iter()
                    .map(|cell| cell_value(cell, &zone))
                    .map(|value| to_plain_string(&value).trim().to_string())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    for row in rows {
        if max_rows.is_some_and(|max| out.len() >= max) {
            break;
        }
        let values: Vec<Value> = row.iter().map(|cell| cell_value(cell, &zone)).collect();
        if values.iter().all(Value::is_null) {
            continue;
        }
        out.push(values);
    }
    (headers, out)
}

fn cell_value(cell: &Data, zone: &jiff::tz::TimeZone) -> Value {
    match cell {
        Data::Empty | Data::Error(_) => Value::Null,
        Data::String(text) => Value::string(text),
        Data::Bool(flag) => Value::Bool(*flag),
        Data::Int(number) => Value::I64(*number),
        #[allow(clippy::cast_possible_truncation)]
        Data::Float(number) => {
            if number.fract() == 0.0 && number.abs() < WHOLE_LIMIT {
                Value::I64(*number as i64)
            } else {
                Value::F64(*number)
            }
        }
        Data::DateTime(stamp) => {
            if stamp.is_duration() {
                Value::F64(stamp.as_f64())
            } else {
                excel_timestamp(stamp.as_f64(), zone).unwrap_or(Value::F64(stamp.as_f64()))
            }
        }
        Data::DateTimeIso(text) | Data::DurationIso(text) => Value::string(text),
    }
}

/// An Excel serial date (days since 1899-12-30) as a time stamp of that wall time in `zone`.
#[allow(clippy::cast_possible_truncation)]
fn excel_timestamp(serial: f64, zone: &jiff::tz::TimeZone) -> Option<Value> {
    let millis = (serial * 86_400_000.0).round() as i64;
    let zero = jiff::civil::date(1899, 12, 30).at(0, 0, 0, 0);
    let civil = zero.checked_add(jiff::Span::new().milliseconds(millis)).ok()?;
    let stamp = civil.to_zoned(zone.clone()).ok()?.timestamp();
    Some(Value::Timestamp(stamp))
}

/// Visit the non-empty cells of sheet `name` in row order until `visit` returns false. An xlsx or xlsb sheet is read
/// cell by cell, so a visit that stops early reads no more of it; an xls or ods sheet is read whole first.
fn visit_cells(
    book: &mut Sheets<Cursor<Vec<u8>>>,
    name: &str,
    mut visit: impl FnMut(u32, u32, Data) -> bool,
) -> Result<(), LibError> {
    let failed = |err: &dyn std::fmt::Display| LibError::new(format!("Could not read sheet {name}: {err}"));
    match book {
        Sheets::Xlsx(xlsx) => {
            let mut reader = match xlsx.worksheet_cells_reader(name) {
                Ok(reader) => reader,
                Err(calamine::XlsxError::NotAWorksheet(_)) => return Ok(()),
                Err(err) => return Err(failed(&err)),
            };
            while let Some(cell) = reader.next_cell().map_err(|err| failed(&err))? {
                if matches!(cell.get_value(), DataRef::Empty) {
                    continue;
                }
                let (row, column) = cell.get_position();
                if !visit(row, column, Data::from(cell.get_value().clone())) {
                    break;
                }
            }
        }
        Sheets::Xlsb(xlsb) => {
            let mut reader = xlsb.worksheet_cells_reader(name).map_err(|err| failed(&err))?;
            while let Some(cell) = reader.next_cell().map_err(|err| failed(&err))? {
                if matches!(cell.get_value(), DataRef::Empty) {
                    continue;
                }
                let (row, column) = cell.get_position();
                if !visit(row, column, Data::from(cell.get_value().clone())) {
                    break;
                }
            }
        }
        _ => {
            let range = book.worksheet_range(name).map_err(|err| failed(&err))?;
            let (top, left) = range.start().unwrap_or((0, 0));
            for (row, column, data) in range.used_cells() {
                let (row, column) = (
                    top + u32::try_from(row).unwrap_or(u32::MAX - top),
                    left + u32::try_from(column).unwrap_or(u32::MAX - left),
                );
                if !visit(row, column, data.clone()) {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// A cell as a preview shows it: [`cell_value`], with a date as its text in `zone` (the time left off at midnight).
fn preview_cell(cell: &Data, zone: &jiff::tz::TimeZone) -> Value {
    match cell_value(cell, zone) {
        Value::Timestamp(stamp) => {
            let zoned = stamp.to_zoned(zone.clone());
            let format = if zoned.hour() == 0 && zoned.minute() == 0 && zoned.second() == 0 {
                "%Y-%m-%d"
            } else {
                "%Y-%m-%d %H:%M"
            };
            Value::string(zoned.strftime(format).to_string())
        }
        other => other,
    }
}

fn open_book(bytes: &[u8]) -> Result<Sheets<Cursor<Vec<u8>>>, LibError> {
    calamine::open_workbook_auto_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|err| LibError::new(format!("Could not read spreadsheet: {err}")))
}

/// The first rows of one sheet, for a preview.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct SheetPreview {
    pub sheet_names: Vec<String>,
    pub sheet_name: String,
    /// From the first used row and column; an empty cell is null.
    pub rows: Vec<Vec<Value>>,
    /// Whether the sheet has rows or columns past those in `rows`.
    pub truncated: bool,
}

/// The first `max_rows` rows and `max_cols` columns of sheet `sheet_name` (the first sheet when empty), from the first
/// used row and column.
///
/// # Errors
///
/// Returns an error when the bytes are not a spreadsheet calamine reads, or it has no sheet of that name.
pub(crate) fn preview_rows(
    bytes: &[u8],
    sheet_name: &str,
    max_rows: usize,
    max_cols: usize,
) -> Result<SheetPreview, LibError> {
    let mut book = open_book(bytes)?;
    let names = book.sheet_names();
    let chosen = if sheet_name.is_empty() {
        match names.first() {
            Some(first) => first.clone(),
            None => return Ok(SheetPreview::default()),
        }
    } else if names.iter().any(|name| name == sheet_name) {
        sheet_name.to_string()
    } else {
        return Err(LibError::new(format!(
            "The spreadsheet has no sheet named {sheet_name}, it has {}",
            names.join(", ")
        )));
    };
    let zone = crate::l10n::time_zone();
    let max_rows = u32::try_from(max_rows.max(1)).unwrap_or(u32::MAX);
    let max_cols = u32::try_from(max_cols.max(1)).unwrap_or(u32::MAX);
    // A very wide sheet: no more cells are kept than a square preview of the widest columns would hold.
    let cap = usize::try_from(u64::from(max_rows) * u64::from(max_cols) * 4).unwrap_or(usize::MAX);
    let mut cells = Vec::new();
    let mut first_row = None;
    let mut truncated = false;
    visit_cells(&mut book, &chosen, |row, column, data| {
        let first = *first_row.get_or_insert(row);
        if row - first >= max_rows {
            truncated = true;
            return false;
        }
        if cells.len() < cap {
            cells.push((row - first, column, preview_cell(&data, &zone)));
        } else {
            truncated = true;
        }
        true
    })?;
    let left = cells.iter().map(|(_, column, _)| *column).min().unwrap_or(0);
    let mut rows: Vec<Vec<Value>> = Vec::new();
    for (row, column, value) in cells {
        let column = column - left;
        if column >= max_cols {
            truncated = true;
            continue;
        }
        let (row, column) = (row as usize, column as usize);
        if rows.len() <= row {
            rows.resize_with(row + 1, Vec::new);
        }
        let cells = &mut rows[row];
        if cells.len() <= column {
            cells.resize(column + 1, Value::Null);
        }
        cells[column] = value;
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize(width, Value::Null);
    }
    Ok(SheetPreview {
        sheet_names: names,
        sheet_name: chosen,
        rows,
        truncated,
    })
}

/// The text of every sheet, for search: each sheet's name on a line, then its rows, the cells of a row separated by a
/// tab. At most `max_chars` characters; reading stops there.
///
/// # Errors
///
/// Returns an error when the bytes are not a spreadsheet calamine reads. A sheet that cannot be read is logged and
/// left out.
pub(crate) fn sheet_text(bytes: &[u8], max_chars: usize) -> Result<String, LibError> {
    let mut book = open_book(bytes)?;
    let zone = crate::l10n::time_zone();
    let mut text = String::new();
    let mut chars = 0;
    for name in book.sheet_names() {
        if chars >= max_chars {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&name);
        chars += name.chars().count() + 1;
        let mut last_row = None;
        let visited = visit_cells(&mut book, &name, |row, _column, data| {
            text.push(if last_row == Some(row) { '\t' } else { '\n' });
            last_row = Some(row);
            let cell = to_plain_string(&preview_cell(&data, &zone));
            chars += cell.chars().count() + 1;
            text.push_str(&cell);
            chars < max_chars
        });
        if let Err(err) = visited {
            tracing::warn!("{}", err.message());
        }
    }
    if chars > max_chars {
        if let Some((cut, _)) = text.char_indices().nth(max_chars) {
            text.truncate(cut);
        }
    }
    Ok(text)
}

/// `read#Spreadsheet`: the sheet names and the rows of one sheet, each a map by header or, without a header row, a
/// list of the cells.
fn read_spreadsheet(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let sheet = read_sheet(parameters)?;
    let header_row = flag(parameters, "headerRow", true);
    let row_list = if header_row {
        sheet
            .rows
            .iter()
            .map(|row| Value::Map(row_map(&sheet.headers, row)))
            .collect()
    } else {
        sheet.rows.into_iter().map(Value::List).collect()
    };
    let mut out = BTreeMap::new();
    out.insert(
        "sheetNameList".to_string(),
        Value::List(sheet.names.into_iter().map(Value::String).collect()),
    );
    out.insert(
        "headerList".to_string(),
        Value::List(sheet.headers.into_iter().map(Value::String).collect()),
    );
    out.insert("rowList".to_string(), Value::List(row_list));
    Ok(out)
}

/// A row by header. A column with a blank header is left out; a repeated header keeps the first value.
fn row_map(headers: &[String], row: &[Value]) -> IndexMap<String, Value> {
    let mut map = IndexMap::new();
    for (header, value) in headers.iter().zip(row) {
        if !header.is_empty() && !map.contains_key(header) {
            map.insert(header.clone(), value.clone());
        }
    }
    map
}

/// `load#SpreadsheetEntity`: each row of the sheet stored (or created) as a record of `entityName`, through the entity
/// auto service, so authorization, ECAs, and the caller's transaction apply. A column's field is its header unless
/// `fieldMap` names another; a column that is not a field of the entity is not set.
fn load_spreadsheet_entity(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let entity_name = param_text(parameters, "entityName");
    if entity_name.is_empty() {
        return Err(LibError::new("load#SpreadsheetEntity needs an entityName"));
    }
    let verb = match param_text(parameters, "loadMode").as_str() {
        "" | "store" => "store",
        "create" => "create",
        other => return Err(LibError::new(format!("loadMode {other} is not create or store"))),
    };
    let mut reading = parameters.clone();
    reading.insert("headerRow".to_string(), Value::Bool(true));
    let sheet = read_sheet(&reading)?;
    let field_map: BTreeMap<String, String> = match parameters.get("fieldMap") {
        Some(Value::Map(map)) => map
            .iter()
            .map(|(column, field)| (column.clone(), to_plain_string(field)))
            .collect(),
        _ => BTreeMap::new(),
    };
    let fields: Vec<String> = sheet
        .headers
        .iter()
        .map(|header| field_map.get(header).cloned().unwrap_or_else(|| header.clone()))
        .collect();
    let service = format!("{verb}#{entity_name}");
    let mut count: i64 = 0;
    for row in &sheet.rows {
        let values: BTreeMap<String, Value> = row_map(&fields, row)
            .into_iter()
            .filter(|(_, value)| !value.is_null())
            .collect();
        sync_with(&service, &values, &CallOptions::default()).map_err(|err| LibError::new(err.message()))?;
        if has_error() {
            break;
        }
        count += 1;
    }
    let mut out = BTreeMap::new();
    out.insert("recordCount".to_string(), Value::I64(count));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_numbers_are_longs_and_dates_are_time_stamps() {
        let zone = jiff::tz::TimeZone::UTC;
        assert_eq!(cell_value(&Data::Float(3.0), &zone), Value::I64(3));
        assert_eq!(cell_value(&Data::Float(2.5), &zone), Value::F64(2.5));
        assert_eq!(cell_value(&Data::Empty, &zone), Value::Null);
        let stamp = excel_timestamp(45352.5, &zone).unwrap();
        assert_eq!(stamp, Value::Timestamp("2024-03-01T12:00:00Z".parse().unwrap()));
    }

    #[test]
    fn a_sheet_whose_cells_span_too_large_an_area_is_refused() {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_string(0, 0, "first").unwrap();
        sheet.write_string(1_048_575, 16_383, "last").unwrap();
        let bytes = book.save_to_buffer().unwrap();
        let mut parameters = BTreeMap::new();
        parameters.insert("contentBytes".to_string(), Value::Bytes(bytes));
        let Err(err) = read_sheet(&parameters) else {
            panic!("a sparse sheet over the cell limit is refused");
        };
        assert!(err.message().contains("spans more than"), "{}", err.message());

        // The same two cells close together read as before.
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet();
        sheet.write_string(0, 0, "first").unwrap();
        sheet.write_string(2, 3, "last").unwrap();
        let mut parameters = BTreeMap::new();
        parameters.insert("contentBytes".to_string(), Value::Bytes(book.save_to_buffer().unwrap()));
        let read = read_sheet(&parameters).unwrap();
        assert_eq!(read.headers, vec!["first", "", "", ""]);
        assert_eq!(
            read.rows,
            vec![vec![Value::Null, Value::Null, Value::Null, Value::string("last")]]
        );
    }

    fn preview_book() -> Vec<u8> {
        let mut book = rust_xlsxwriter::Workbook::new();
        let sheet = book.add_worksheet().set_name("Orders").unwrap();
        // The used area starts at row 2, column B.
        sheet.write_string(1, 1, "item").unwrap();
        sheet.write_string(1, 2, "qty").unwrap();
        sheet.write_string(1, 3, "note").unwrap();
        for row in 0..50_u32 {
            sheet.write_string(row + 2, 1, format!("W-{row}")).unwrap();
            sheet.write_number(row + 2, 2, f64::from(row)).unwrap();
        }
        sheet.write_string(4, 3, "rush").unwrap();
        let other = book.add_worksheet().set_name("Notes").unwrap();
        other.write_string(0, 0, "Hello sheet").unwrap();
        book.save_to_buffer().unwrap()
    }

    #[test]
    fn a_preview_has_the_first_rows_from_the_used_area_and_stops_there() {
        let bytes = preview_book();
        let preview = preview_rows(&bytes, "", 5, 10).unwrap();
        assert_eq!(preview.sheet_names, vec!["Orders", "Notes"]);
        assert_eq!(preview.sheet_name, "Orders");
        assert!(preview.truncated);
        assert_eq!(preview.rows.len(), 5);
        assert_eq!(
            preview.rows[0],
            vec![Value::string("item"), Value::string("qty"), Value::string("note")]
        );
        assert_eq!(preview.rows[1], vec![Value::string("W-0"), Value::I64(0), Value::Null]);
        assert_eq!(preview.rows[3][2], Value::string("rush"));

        // Two columns: the third is cut.
        let narrow = preview_rows(&bytes, "Orders", 2, 2).unwrap();
        assert_eq!(
            narrow.rows,
            vec![
                vec![Value::string("item"), Value::string("qty")],
                vec![Value::string("W-0"), Value::I64(0)],
            ]
        );
        assert!(narrow.truncated);

        let notes = preview_rows(&bytes, "Notes", 5, 5).unwrap();
        assert_eq!(notes.rows, vec![vec![Value::string("Hello sheet")]]);
        assert!(!notes.truncated);
        assert!(preview_rows(&bytes, "Missing", 5, 5).is_err());
        assert!(preview_rows(b"not a spreadsheet", "", 5, 5).is_err());
    }

    #[test]
    fn sheet_text_has_every_sheet_and_stops_at_the_limit() {
        let bytes = preview_book();
        let text = sheet_text(&bytes, 100_000).unwrap();
        assert!(text.starts_with("Orders\nitem\tqty\tnote\nW-0\t0\n"), "{text:?}");
        assert!(text.contains("W-49\t49"));
        assert!(text.ends_with("Notes\nHello sheet"), "{text:?}");
        let short = sheet_text(&bytes, 12).unwrap();
        assert_eq!(short, "Orders\nitem");
        assert!(sheet_text(b"nope", 10).is_err());
    }

    #[test]
    fn a_header_row_keys_the_rows_and_blank_rows_are_skipped() {
        let mut book = crate::screen::xlsx::XlsxBook::default();
        book.start_sheet("Data");
        book.header("itemId");
        book.header("qty");
        for (id, qty) in [("A", 2.0), ("", 0.0), ("B", 3.5)] {
            let id_value = if id.is_empty() { Value::Null } else { Value::string(id) };
            let qty_value = if id.is_empty() { Value::Null } else { Value::F64(qty) };
            book.cell(&[(id_value, String::new())], Default::default()).unwrap();
            book.cell(&[(qty_value, String::new())], Default::default()).unwrap();
            book.end_row();
        }
        let bytes = book.to_bytes().unwrap();
        let mut parameters = BTreeMap::new();
        parameters.insert("contentBytes".to_string(), Value::Bytes(bytes));
        let sheet = read_sheet(&parameters).unwrap();
        assert_eq!(sheet.names, vec!["Data".to_string()]);
        assert_eq!(sheet.headers, vec!["itemId".to_string(), "qty".to_string()]);
        assert_eq!(sheet.rows.len(), 2);
        let first = row_map(&sheet.headers, &sheet.rows[0]);
        assert_eq!(first.get("itemId"), Some(&Value::string("A")));
        assert_eq!(first.get("qty"), Some(&Value::I64(2)));
        assert_eq!(
            row_map(&sheet.headers, &sheet.rows[1]).get("qty"),
            Some(&Value::F64(3.5))
        );
    }
}
