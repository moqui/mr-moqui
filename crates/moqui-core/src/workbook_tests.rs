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

use calamine::{Data, DataType, Reader};

use super::*;

fn read(bytes: Vec<u8>) -> calamine::Sheets<std::io::Cursor<Vec<u8>>> {
    calamine::open_workbook_auto_from_rs(std::io::Cursor::new(bytes)).unwrap()
}

#[test]
fn typed_cells_on_two_sheets_are_read_back_by_another_reader() {
    let mut book = WorkbookWriter::new().unwrap();
    let plain = book.style(&CellStyle::default());
    let money = book.style(&CellStyle {
        align: Align::Right,
        number_format: Some("#,##0.00".to_string()),
        ..CellStyle::default()
    });
    assert_ne!(plain, money);
    assert_eq!(book.style(&CellStyle::default()), plain, "a style is made once");
    let first = book.add_sheet("Orders").unwrap();
    let second = book.add_sheet("Notes").unwrap();
    book.text(first, 1, 1, "Item", plain).unwrap();
    book.number(first, 2, 1, 12.5, money).unwrap();
    // a text is never parsed: not as a formula, a number or a date
    book.text(first, 3, 1, "=SUM(A1:A2)", plain).unwrap();
    book.text(first, 4, 1, "0042", plain).unwrap();
    book.text(second, 2, 3, "two\nlines", plain).unwrap();
    book.freeze_rows(first, 1).unwrap();
    book.fit_columns(first).unwrap();
    assert!(book.text(7, 1, 1, "x", plain).is_err(), "a sheet that was not added");
    assert!(book.number(first, 0, 1, 1.0, plain).is_err(), "a row outside the sheet");
    assert!(book.add_sheet("Orders").is_err(), "a name already used");

    let mut sheets = read(book.to_xlsx().unwrap());
    assert_eq!(sheets.sheet_names(), ["Orders", "Notes"]);
    let orders = sheets.worksheet_range("Orders").unwrap();
    assert_eq!(orders.get_value((0, 0)), Some(&Data::String("Item".to_string())));
    assert_eq!(orders.get_value((1, 0)).unwrap().get_float(), Some(12.5));
    assert_eq!(orders.get_value((2, 0)), Some(&Data::String("=SUM(A1:A2)".to_string())));
    assert_eq!(orders.get_value((3, 0)), Some(&Data::String("0042".to_string())));
    assert!(
        sheets.worksheet_formula("Orders").unwrap().is_empty(),
        "no formula was written"
    );
    let notes = sheets.worksheet_range("Notes").unwrap();
    assert_eq!(notes.get_value((1, 2)), Some(&Data::String("two\nlines".to_string())));
}

#[test]
fn a_written_workbook_opens_in_the_engine_and_calculates() {
    let mut book = WorkbookWriter::new().unwrap();
    let plain = book.style(&CellStyle::default());
    let sheet = book.add_sheet("Data").unwrap();
    for row in 1..=4 {
        book.number(sheet, row, 1, f64::from(row), plain).unwrap();
    }
    let mut model = book.into_model().unwrap();
    model.set_user_input(0, 5, 1, "=SUM(A1:A4)".to_string()).unwrap();
    model.evaluate();
    assert_eq!(model.get_formatted_cell_value(0, 5, 1).unwrap(), "10");

    // the file keeps the formula and its value, and the engine reads both back
    let bytes = model_xlsx(&model).unwrap();
    let mut sheets = read(bytes.clone());
    assert_eq!(
        sheets
            .worksheet_range("Data")
            .unwrap()
            .get_value((4, 0))
            .unwrap()
            .get_float(),
        Some(10.0)
    );
    let mut again = open_xlsx(&bytes, "again").unwrap();
    assert_eq!(again.get_cell_formula(0, 5, 1).unwrap().as_deref(), Some("=SUM(A1:A4)"));
    again.set_user_input(0, 1, 1, "11".to_string()).unwrap();
    again.evaluate();
    assert_eq!(again.get_formatted_cell_value(0, 5, 1).unwrap(), "20");
    assert!(open_xlsx(b"not a zip", "bad").is_err());
}

#[test]
fn columns_are_fitted_to_their_text_within_limits() {
    let mut book = WorkbookWriter::new().unwrap();
    let plain = book.style(&CellStyle::default());
    let sheet = book.add_sheet("Fit").unwrap();
    book.text(sheet, 1, 1, "a", plain).unwrap();
    book.text(sheet, 1, 2, "a text of thirty characters___", plain).unwrap();
    book.text(sheet, 1, 3, &"x".repeat(500), plain).unwrap();
    book.fit_columns(sheet).unwrap();
    let model = book.into_model().unwrap();
    let width = |column| model.get_column_width(0, column).unwrap();
    assert!((width(1) - FIT_MIN).abs() < 0.5, "{}", width(1));
    assert!(width(2) > 200.0 && width(2) < 260.0, "{}", width(2));
    assert!((width(3) - FIT_MAX).abs() < 0.5, "{}", width(3));
}

#[test]
fn a_cell_is_parsed_with_dollar_signs_and_within_the_sheet() {
    assert_eq!(parse_cell("B2"), Some((2, 2)));
    assert_eq!(parse_cell("$aa$10"), Some((10, 27)));
    assert_eq!(parse_cell("XFD1048576"), Some((1_048_576, 16_384)));
    assert_eq!(parse_cell("XFE1"), None);
    assert_eq!(parse_cell("A1048577"), None);
    assert_eq!(parse_cell("12"), None);
    assert_eq!(parse_cell("A1B"), None);
}
