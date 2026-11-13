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

use calamine::{DataType, Reader};
use moqui_lib::object::ValueMap;

use super::*;

/// A template from another writer: a quote on `Quote` whose inputs are on `Inputs`.
fn template() -> Vec<u8> {
    let mut book = rust_xlsxwriter::Workbook::new();
    let quote = book.add_worksheet().set_name("Quote").unwrap();
    quote.write_string(0, 0, "Total").unwrap();
    let money = rust_xlsxwriter::Format::new().set_num_format("#,##0.00");
    quote
        .write_formula_with_format(0, 1, "=Inputs!B1*Inputs!B2*(1-Inputs!B3)", &money)
        .unwrap();
    quote.write_string(1, 0, "Note").unwrap();
    quote.write_formula(1, 1, "=IF(B1>1000,\"large\",\"small\")").unwrap();
    quote.write_formula(2, 1, "=1/Inputs!B4").unwrap();
    let inputs = book.add_worksheet().set_name("Inputs").unwrap();
    for (row, (name, value)) in [("quantity", 1.0), ("price", 10.0), ("discount", 0.0), ("divisor", 0.0)]
        .iter()
        .enumerate()
    {
        let row = u32::try_from(row).unwrap();
        inputs.write_string(row, 0, *name).unwrap();
        inputs.write_number(row, 1, *value).unwrap();
    }
    book.save_to_buffer().unwrap()
}

fn parameters(cells: &[(&str, Value)], read: &[&str]) -> BTreeMap<String, Value> {
    let mut map = ValueMap::default();
    for (reference, value) in cells {
        map.insert((*reference).to_string(), value.clone());
    }
    let mut parameters = BTreeMap::new();
    parameters.insert("cellValues".to_string(), Value::Map(map));
    parameters.insert(
        "readCells".to_string(),
        Value::List(read.iter().map(|text| Value::string(*text)).collect()),
    );
    parameters
}

fn map<'a>(out: &'a BTreeMap<String, Value>, name: &str) -> &'a ValueMap {
    match out.get(name) {
        Some(Value::Map(map)) => map,
        other => panic!("{name} is {other:?}"),
    }
}

#[test]
fn inputs_are_set_the_workbook_evaluated_and_cells_and_ranges_read() {
    let set = [
        ("Inputs!B1", Value::I64(120)),
        // a text is set as typed: a formula, and a number
        ("inputs!$B$2", Value::string("=45/2")),
        ("'Inputs'!B3", Value::string("0.1")),
    ];
    let (out, file) = calculate(
        &template(),
        &parameters(&set, &["B1", "Quote!B2", "Inputs!A1:B3", "B3"]),
        true,
    )
    .unwrap();
    assert_eq!(
        out.get("sheetNameList"),
        Some(&Value::List(vec![Value::string("Quote"), Value::string("Inputs")]))
    );
    let values = map(&out, "valueMap");
    assert_eq!(values.get("B1"), Some(&Value::I64(2430)), "120 * 22.5 * 0.9");
    assert_eq!(values.get("Quote!B2"), Some(&Value::string("large")));
    assert_eq!(
        values.get("B3"),
        Some(&Value::string("#DIV/0!")),
        "an error is its text"
    );
    assert_eq!(
        values.get("Inputs!A1:B3"),
        Some(&Value::List(vec![
            Value::List(vec![Value::string("quantity"), Value::I64(120)]),
            Value::List(vec![Value::string("price"), Value::F64(22.5)]),
            Value::List(vec![Value::string("discount"), Value::F64(0.1)]),
        ]))
    );
    // the text is as the cell's format shows it
    assert_eq!(map(&out, "textMap").get("B1"), Some(&Value::string("2,430.00")));

    // the workbook given back holds the values, for a reader that calculates nothing
    let mut sheets = calamine::open_workbook_auto_from_rs(std::io::Cursor::new(file.expect("the file"))).unwrap();
    let quote = sheets.worksheet_range("Quote").unwrap();
    assert_eq!(quote.get_value((0, 1)).unwrap().get_float(), Some(2430.0));
    assert_eq!(quote.get_value((1, 1)).unwrap().to_string(), "large");
    assert_eq!(
        sheets
            .worksheet_range("Inputs")
            .unwrap()
            .get_value((1, 1))
            .unwrap()
            .get_float(),
        Some(22.5)
    );
}

#[test]
fn null_clears_a_cell_and_nothing_set_reads_the_template_evaluated() {
    let (out, file) = calculate(&template(), &parameters(&[], &["B1", "B2"]), false).unwrap();
    assert!(file.is_none());
    assert_eq!(map(&out, "valueMap").get("B1"), Some(&Value::I64(10)));
    assert_eq!(map(&out, "valueMap").get("B2"), Some(&Value::string("small")));

    let set = [("Inputs!B2", Value::Null), ("Inputs!B4", Value::Bool(true))];
    let (out, _) = calculate(
        &template(),
        &parameters(&set, &["B1", "Inputs!B2", "Inputs!B4", "B3"]),
        false,
    )
    .unwrap();
    let values = map(&out, "valueMap");
    assert_eq!(values.get("Inputs!B2"), Some(&Value::Null));
    assert_eq!(
        values.get("B1"),
        Some(&Value::I64(0)),
        "an empty cell is zero in a product"
    );
    assert_eq!(values.get("Inputs!B4"), Some(&Value::Bool(true)));
    assert_eq!(values.get("B3"), Some(&Value::I64(1)), "1 / TRUE");
}

#[test]
fn a_bad_reference_an_unknown_sheet_a_range_to_set_and_a_file_that_is_not_xlsx_are_refused() {
    let refused = |cells: &[(&str, Value)], read: &[&str]| {
        calculate(&template(), &parameters(cells, read), false)
            .expect_err("refused")
            .message()
            .to_string()
    };
    assert!(refused(&[], &["B"]).contains("is not a cell or a range"));
    assert!(refused(&[], &["A0"]).contains("is not a cell or a range"));
    assert!(refused(&[], &["Costs!A1"]).contains("no sheet named Costs, it has Quote, Inputs"));
    assert!(refused(&[("A1:B2", Value::I64(1))], &[]).contains("is a range"));
    assert!(refused(&[], &["A1:Z9999"]).contains("at most 100000"));
    let err = calculate(b"not a workbook", &BTreeMap::new(), false).expect_err("not xlsx");
    assert!(err.message().contains("Error reading xlsx"), "{}", err.message());
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
