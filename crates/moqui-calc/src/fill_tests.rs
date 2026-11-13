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

use super::*;

/// `count` order rows: a customer's name, an amount (10 times the row, from `first`) and a date.
fn orders(first: usize, count: usize) -> SheetRows {
    SheetRows {
        name: "Orders".to_string(),
        columns: vec!["customer".to_string(), "amount".to_string(), "placed".to_string()],
        rows: (first..first + count)
            .map(|row| {
                #[allow(clippy::cast_precision_loss)]
                let amount = row as f64 * 10.0;
                vec![
                    CellData::Text(format!("Customer {row}")),
                    CellData::Number(amount),
                    CellData::Date(45_000.0 + amount),
                ]
            })
            .collect(),
        truncated: false,
    }
}

fn text(model: &Model<'_>, sheet: u32, row: i32, column: i32) -> String {
    model.get_formatted_cell_value(sheet, row, column).unwrap()
}

/// A workbook with a Summary sheet of formulas over an Orders data sheet of five rows.
fn workbook() -> Model<'static> {
    let model = new_model("Summary").unwrap();
    let mut model = fill(model, &[orders(1, 5)]).unwrap();
    for (row, formula) in [
        "=SUM(Orders[amount])",
        "=ROWS(Orders[amount])",
        "=COUNTIF(Orders[customer],\"Customer 2\")",
        "=Orders!B2*2",
    ]
    .iter()
    .enumerate()
    {
        model
            .set_user_input(0, i32::try_from(row + 1).unwrap(), 1, (*formula).to_string())
            .unwrap();
    }
    model
        .set_user_input(0, 5, 1, "the author's own text".to_string())
        .unwrap();
    model.evaluate();
    model
}

#[test]
fn a_data_sheet_is_a_typed_table_a_formula_names_by_column() {
    let model = workbook();
    assert_eq!(model.workbook.get_worksheet_names(), ["Summary", "Orders"]);
    // the header, then a row for each record, typed
    assert_eq!(text(&model, 1, 1, 1), "customer");
    assert_eq!(text(&model, 1, 2, 1), "Customer 1");
    assert_eq!(text(&model, 1, 6, 2), "50");
    assert_eq!(text(&model, 1, 2, 3), "2023-03-25", "a date is shown by its format");
    assert_eq!(text(&model, 0, 1, 1), "150");
    assert_eq!(text(&model, 0, 2, 1), "5");
    assert_eq!(text(&model, 0, 3, 1), "1");
    assert_eq!(text(&model, 0, 4, 1), "20");
    assert_eq!(table_name("Open orders 2026"), "Open_orders_2026");
    assert_eq!(table_name("2026"), "T2026");

    let sheets = layout(&model, &[("Orders".to_string(), 5)]);
    assert_eq!(sheets[0]["name"], "Summary");
    assert_eq!(sheets[0]["data"], false);
    assert_eq!(sheets[1]["data"], true);
    assert_eq!(sheets[1]["dataRows"], 5);
    assert_eq!(sheets[1]["rows"], 6);
    assert_eq!(sheets[1]["columns"], 3);
    assert_eq!(sheets[1]["frozenRows"], 1);
    assert_eq!(sheets[1]["widths"].as_array().unwrap().len(), 3);
}

#[test]
fn the_stored_form_has_no_data_no_value_from_it_and_none_of_its_texts() {
    let model = workbook();
    let stored = stored_form(&model.to_bytes(), &[("Orders".to_string(), 5)]).unwrap();
    let opened = open_stored(&stored).unwrap();
    // the header stays, the rows do not
    assert_eq!(text(&opened, 1, 1, 1), "customer");
    assert_eq!(text(&opened, 1, 2, 1), "");
    assert_eq!(text(&opened, 1, 6, 2), "");
    // each formula is there, evaluated over no data
    assert_eq!(
        opened.get_cell_formula(0, 1, 1).unwrap().as_deref(),
        Some("=SUM(Orders!$B$2:$B$3)")
    );
    assert_eq!(text(&opened, 0, 1, 1), "0");
    assert_eq!(text(&opened, 0, 3, 1), "0");
    assert_eq!(text(&opened, 0, 4, 1), "0");
    assert_eq!(text(&opened, 0, 5, 1), "the author's own text");
    // no text of a record is in the workbook, in a cell or in its shared strings
    assert!(opened
        .workbook
        .shared_strings
        .iter()
        .all(|found| !found.starts_with("Customer")));
    assert!(opened
        .workbook
        .shared_strings
        .contains(&"the author's own text".to_string()));
    assert!(opened.workbook.shared_strings.contains(&"amount".to_string()));
    let needle = b"Customer 3";
    assert!(
        !stored.windows(needle.len()).any(|window| window == needle),
        "a record's text in the stored bytes"
    );
}

#[test]
fn a_stored_workbook_filled_again_with_other_rows_keeps_its_formulas_right() {
    let stored = stored_form(&workbook().to_bytes(), &[("Orders".to_string(), 5)]).unwrap();
    for (first, count, sum) in [(1, 1000, 5_005_000.0), (3, 4, 180.0), (7, 1, 70.0), (1, 0, 0.0)] {
        let mut model = fill(open_stored(&stored).unwrap(), &[orders(first, count)]).unwrap();
        model.evaluate();
        let value = |row| text(&model, 0, row, 1).replace(',', "").parse::<f64>().unwrap();
        assert!(
            (value(1) - sum).abs() < 1e-6,
            "{count} rows from {first}: sum {}",
            value(1)
        );
        // fewer rows than the stub keeps leave its empty rows in the range
        #[allow(clippy::cast_precision_loss)]
        let rows = count.max(2) as f64;
        assert!((value(2) - rows).abs() < 1e-6, "{count} rows: ROWS {}", value(2));
        #[allow(clippy::cast_precision_loss)]
        let doubled = if count == 0 { 0.0 } else { first as f64 * 20.0 };
        assert!((value(4) - doubled).abs() < 1e-6, "{count} rows: B2*2 {}", value(4));
        // and it stores again as it was
        let again = stored_form(&model.to_bytes(), &[("Orders".to_string(), count)]).unwrap();
        let opened = open_stored(&again).unwrap();
        assert_eq!(
            opened.get_cell_formula(0, 1, 1).unwrap().as_deref(),
            Some("=SUM(Orders!$B$2:$B$3)"),
            "{count} rows"
        );
        assert!(opened
            .workbook
            .shared_strings
            .iter()
            .all(|found| !found.starts_with("Customer")));
    }
}

#[test]
fn a_second_data_sheet_is_added_beside_the_first() {
    let stored = stored_form(&workbook().to_bytes(), &[("Orders".to_string(), 5)]).unwrap();
    let mut totals = orders(1, 2);
    totals.name = "Totals".to_string();
    let mut model = fill(open_stored(&stored).unwrap(), &[orders(1, 3), totals]).unwrap();
    model.evaluate();
    assert_eq!(model.workbook.get_worksheet_names(), ["Summary", "Orders", "Totals"]);
    assert_eq!(text(&model, 0, 1, 1), "60");
    assert_eq!(text(&model, 2, 3, 1), "Customer 2");
    assert!(model.workbook.tables.contains_key("Totals"));
}
