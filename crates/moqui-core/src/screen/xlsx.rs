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

//! The `xlsx` render mode's workbook, the port of moqui-poi's `FormListExcelRender` and `ScreenWidgetRenderExcel`.
//!
//! `ScreenXlsx.js` walks the screen as the Java widget render did and fills this through `sri.xlsxSheet`,
//! `sri.xlsxHeader`, `sri.xlsxCell` and `sri.xlsxEndRow`: a sheet for each form-list, a header row of field titles,
//! and a row for each list entry. The render then sends [`XlsxBook::to_bytes`] as the response.
//!
//! The styles are the Java ones: a bold centered header on grey, the header row frozen, a string left-aligned and
//! wrapped, a number centered (right-aligned with `align="right"`), a currency or `.00` format right-aligned with
//! negatives in red, and dates as `yyyy-MM-dd`, times as `HH:mm:ss`, time stamps as `yyyy-MM-dd HH:mm`. Columns are
//! fitted to their text. A date is an Excel serial number with a date format, as POI wrote it.

use std::borrow::Cow;

use jiff::civil::{Date, DateTime, Time};
use moqui_lib::format::ToPrimitive;
use moqui_lib::object::Value;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatPattern, Workbook};

use crate::context::Error;
use crate::l10n;

/// POI's `IndexedColors.GREY_40_PERCENT`.
const HEADER_FILL: u32 = 0x0096_9696;
/// Excel's limit on a sheet name.
const SHEET_NAME_MAX: usize = 31;
/// Excel's limit on the characters in one cell.
const CELL_TEXT_MAX: usize = 32_767;

/// How a number cell is shown: the field's `align` and the widget's currency or `.00` format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellHints {
    pub currency: bool,
    pub align_right: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum CellValue {
    Empty,
    Text(String),
    Number(f64),
    Date(Date),
    Time(Time),
    DateTime(DateTime),
}

#[derive(Clone, Debug)]
struct Cell {
    value: CellValue,
    hints: CellHints,
}

#[derive(Debug, Default)]
struct Sheet {
    name: String,
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
    row: Vec<Cell>,
}

/// The workbook a render fills: one sheet for each form-list.
#[derive(Debug, Default)]
pub struct XlsxBook {
    sheets: Vec<Sheet>,
}

impl XlsxBook {
    /// Start the sheet of a form-list. Its rows follow.
    pub fn start_sheet(&mut self, name: &str) {
        self.sheets.push(Sheet {
            name: name.to_string(),
            ..Sheet::default()
        });
    }

    /// A header cell of the current sheet.
    pub fn header(&mut self, title: &str) {
        if let Some(sheet) = self.sheets.last_mut() {
            sheet.headers.push(title.to_string());
        }
    }

    /// A cell of the current row from the values of a field's widgets, each with its format. One value keeps its type,
    /// as FormListExcelRender did for a field with one widget. Several are formatted and joined with line breaks. None
    /// (a hidden or ignored field) is an empty cell, so the columns stay under their headers.
    ///
    /// # Errors
    ///
    /// Returns an error when a value cannot be formatted with its widget's format.
    pub fn cell(&mut self, values: &[(Value, String)], hints: CellHints) -> Result<(), Error> {
        let value = if values.is_empty() {
            CellValue::Empty
        } else if let [(value, _)] = values {
            typed_value(value)
        } else {
            let mut text = String::new();
            for (index, (value, format)) in values.iter().enumerate() {
                match value {
                    Value::Null => {}
                    Value::String(found) => text.push_str(found),
                    other => text.push_str(&l10n::format_value(other, Some(format.as_str()))?),
                }
                if index + 1 < values.len() && !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            CellValue::Text(text)
        };
        if let Some(sheet) = self.sheets.last_mut() {
            sheet.row.push(Cell { value, hints });
        }
        Ok(())
    }

    /// End the current row.
    pub fn end_row(&mut self) {
        if let Some(sheet) = self.sheets.last_mut() {
            let row = std::mem::take(&mut sheet.row);
            sheet.rows.push(row);
        }
    }

    /// The cells of the current row so far.
    pub fn cell_count(&self) -> usize {
        self.sheets.last().map_or(0, |sheet| sheet.row.len())
    }

    pub fn is_empty(&self) -> bool {
        self.sheets.is_empty()
    }

    /// The `.xlsx` file.
    ///
    /// # Errors
    ///
    /// Returns "No sheets rendered" when the screen had no form-list, as in Java, or the writer's error.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        if self.sheets.is_empty() {
            return Err(Error::new("No sheets rendered"));
        }
        let styles = Styles::new();
        let mut workbook = Workbook::new();
        let mut used: Vec<String> = Vec::new();
        for sheet in &self.sheets {
            let name = unique_sheet_name(&sheet.name, &used);
            used.push(name.to_lowercase());
            let worksheet = workbook.add_worksheet();
            worksheet.set_name(name).map_err(xlsx_error)?;
            for (col, title) in sheet.headers.iter().enumerate() {
                worksheet
                    .write_string_with_format(0, column(col)?, cell_text(title), &styles.header)
                    .map_err(xlsx_error)?;
            }
            worksheet.set_freeze_panes(1, 0).map_err(xlsx_error)?;
            for (index, row) in sheet.rows.iter().enumerate() {
                let row_num = u32::try_from(index + 1).map_err(|_| Error::new("Too many rows for a sheet"))?;
                for (col, cell) in row.iter().enumerate() {
                    let col = column(col)?;
                    match &cell.value {
                        CellValue::Empty => continue,
                        CellValue::Text(text) => {
                            worksheet.write_string_with_format(row_num, col, cell_text(text), &styles.text)
                        }
                        CellValue::Number(number) => {
                            let format = if cell.hints.currency {
                                &styles.currency
                            } else if cell.hints.align_right {
                                &styles.number_right
                            } else {
                                &styles.number
                            };
                            worksheet.write_number_with_format(row_num, col, *number, format)
                        }
                        CellValue::Date(date) => {
                            worksheet.write_number_with_format(row_num, col, excel_date(*date), &styles.date)
                        }
                        CellValue::Time(time) => {
                            worksheet.write_number_with_format(row_num, col, excel_time(*time), &styles.time)
                        }
                        CellValue::DateTime(stamp) => worksheet.write_number_with_format(
                            row_num,
                            col,
                            excel_date(stamp.date()) + excel_time(stamp.time()),
                            &styles.date_time,
                        ),
                    }
                    .map_err(xlsx_error)?;
                }
            }
            worksheet.autofit();
        }
        workbook.save_to_buffer().map_err(xlsx_error)
    }
}

struct Styles {
    header: Format,
    text: Format,
    number: Format,
    number_right: Format,
    currency: Format,
    date: Format,
    time: Format,
    date_time: Format,
}

impl Styles {
    fn new() -> Self {
        let base = Format::new().set_align(FormatAlign::Top);
        Self {
            header: base
                .clone()
                .set_bold()
                .set_align(FormatAlign::Center)
                .set_background_color(Color::RGB(HEADER_FILL))
                .set_pattern(FormatPattern::Solid),
            text: base.clone().set_align(FormatAlign::Left).set_text_wrap(),
            number: base
                .clone()
                .set_align(FormatAlign::Center)
                .set_num_format("#,##0.######"),
            number_right: base
                .clone()
                .set_align(FormatAlign::Right)
                .set_num_format("#,##0.######"),
            currency: base
                .clone()
                .set_align(FormatAlign::Right)
                .set_num_format("#,##0.00_);[Red](#,##0.00)"),
            date: base.clone().set_align(FormatAlign::Left).set_num_format("yyyy-mm-dd"),
            time: base.clone().set_align(FormatAlign::Left).set_num_format("hh:mm:ss"),
            date_time: base.set_align(FormatAlign::Left).set_num_format("yyyy-mm-dd hh:mm"),
        }
    }
}

fn typed_value(value: &Value) -> CellValue {
    match value {
        Value::Null => CellValue::Empty,
        Value::String(text) => CellValue::Text(text.clone()),
        Value::I32(number) => CellValue::Number(f64::from(*number)),
        #[allow(clippy::cast_precision_loss)]
        Value::I64(number) => CellValue::Number(*number as f64),
        Value::F64(number) => CellValue::Number(*number),
        Value::Decimal(number) => number
            .to_f64()
            .map_or_else(|| CellValue::Text(number.to_string()), CellValue::Number),
        Value::Date(date) => CellValue::Date(*date),
        Value::Time(time) => CellValue::Time(*time),
        Value::Timestamp(stamp) => CellValue::DateTime(stamp.to_zoned(l10n::time_zone()).datetime()),
        other => CellValue::Text(moqui_lib::object::to_plain_string(other)),
    }
}

/// Days since Excel's day zero, 1899-12-30 (the 1900 date system, leap-year bug included).
fn excel_date(date: Date) -> f64 {
    let zero = jiff::civil::date(1899, 12, 30);
    zero.until(date).map_or(0.0, |span| f64::from(span.get_days()))
}

fn excel_time(time: Time) -> f64 {
    let seconds = f64::from(time.hour()) * 3600.0
        + f64::from(time.minute()) * 60.0
        + f64::from(time.second())
        + f64::from(time.subsec_nanosecond()) / 1e9;
    seconds / 86_400.0
}

/// The text cut to Excel's cell limit, ending in an ellipsis when cut, so one long value does not
/// fail the whole export.
fn cell_text(text: &str) -> Cow<'_, str> {
    if text.chars().nth(CELL_TEXT_MAX).is_none() {
        return Cow::Borrowed(text);
    }
    let kept: String = text.chars().take(CELL_TEXT_MAX - 1).collect();
    Cow::Owned(format!("{kept}\u{2026}"))
}

fn column(index: usize) -> Result<u16, Error> {
    u16::try_from(index).map_err(|_| Error::new("Too many columns for a sheet"))
}

/// A name Excel takes: no `[]:*?/\`, at most 31 characters, not blank, and not one already used in the workbook.
fn unique_sheet_name(name: &str, used: &[String]) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if "[]:*?/\\".contains(ch) { '_' } else { ch })
        .collect();
    let cleaned = cleaned.trim().trim_matches('\'');
    let base = if cleaned.is_empty() { "Sheet" } else { cleaned };
    let truncate = |text: &str, max: usize| text.chars().take(max).collect::<String>();
    let mut candidate = truncate(base, SHEET_NAME_MAX);
    let mut counter = 2;
    while used.contains(&candidate.to_lowercase()) {
        let suffix = format!(" ({counter})");
        candidate = format!("{}{suffix}", truncate(base, SHEET_NAME_MAX - suffix.len()));
        counter += 1;
    }
    candidate
}

fn xlsx_error(err: rust_xlsxwriter::XlsxError) -> Error {
    Error::new(format!("Error writing xlsx: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> XlsxBook {
        let mut book = XlsxBook::default();
        book.start_sheet("ListForm");
        book.header("Name");
        book.header("Amount");
        book.header("Date");
        book.cell(&[(Value::string("A"), String::new())], CellHints::default())
            .unwrap();
        book.cell(
            &[(Value::F64(12.5), "#,##0.00".to_string())],
            CellHints {
                currency: true,
                align_right: false,
            },
        )
        .unwrap();
        book.cell(
            &[(Value::Date(jiff::civil::date(2024, 3, 1)), String::new())],
            CellHints::default(),
        )
        .unwrap();
        book.end_row();
        book
    }

    #[test]
    fn writes_a_zip_with_one_sheet() {
        let bytes = book().to_bytes().unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let mut sheets = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes)).unwrap();
        use calamine::{DataType, Reader};
        assert_eq!(sheets.sheet_names(), vec!["ListForm".to_string()]);
        let range = sheets.worksheet_range("ListForm").unwrap();
        assert_eq!(range.get_value((0, 0)).unwrap().to_string(), "Name");
        assert_eq!(range.get_value((1, 0)).unwrap().to_string(), "A");
        assert_eq!(range.get_value((1, 1)).unwrap().get_float(), Some(12.5));
        // 2024-03-01 is serial 45352
        let date = range.get_value((1, 2)).unwrap();
        assert_eq!(date.get_datetime().map(|found| found.as_f64()), Some(45352.0));
    }

    #[test]
    fn a_cell_over_the_excel_limit_is_cut_with_an_ellipsis() {
        let mut book = XlsxBook::default();
        book.start_sheet("Long");
        let long = "\u{e9}".repeat(CELL_TEXT_MAX + 10);
        book.cell(&[(Value::string(long.as_str()), String::new())], CellHints::default())
            .unwrap();
        book.end_row();
        let bytes = book.to_bytes().unwrap();
        let mut sheets = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes)).unwrap();
        use calamine::Reader;
        let range = sheets.worksheet_range("Long").unwrap();
        let text = range.get_value((1, 0)).unwrap().to_string();
        assert_eq!(text.chars().count(), CELL_TEXT_MAX);
        assert!(text.ends_with('\u{2026}'));
        assert_eq!(cell_text("short"), "short");
    }

    #[test]
    fn no_sheets_is_the_java_error() {
        let err = XlsxBook::default().to_bytes().unwrap_err();
        assert!(err.message().contains("No sheets rendered"));
    }

    #[test]
    fn several_values_join_as_text() {
        let mut book = XlsxBook::default();
        book.start_sheet("A");
        book.cell(
            &[
                (Value::string("one"), String::new()),
                (Value::Null, String::new()),
                (Value::string("two"), String::new()),
            ],
            CellHints::default(),
        )
        .unwrap();
        assert_eq!(book.sheets[0].row[0].value, CellValue::Text("one\ntwo".to_string()));
    }

    #[test]
    fn sheet_names_are_cleaned_and_unique() {
        let used = vec!["list".to_string()];
        assert_eq!(unique_sheet_name("List", &used), "List (2)");
        assert_eq!(unique_sheet_name("a/b:c", &[]), "a_b_c");
        assert_eq!(unique_sheet_name("", &[]), "Sheet");
        assert_eq!(unique_sheet_name(&"x".repeat(40), &[]).len(), 31);
    }
}
