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

//! An uploaded workbook as a document's overlay: its formulas over the data sheet read the source's rows, and a sheet,
//! table or name of the data sheet's is refused.

use moqui_core::workbook::ironcalc::base::DefinedNameEntry;
use moqui_core::workbook::ironcalc::base::Model;

use super::*;
use crate::fill::{self, CellData, SheetRows};

fn orders(count: usize) -> SheetRows {
    SheetRows {
        name: "Orders".to_string(),
        columns: vec!["customer".to_string(), "amount".to_string()],
        rows: (1..=count)
            .map(|row| {
                #[allow(clippy::cast_precision_loss)]
                let amount = row as f64 * 10.0;
                vec![CellData::Text(format!("Customer {row}")), CellData::Number(amount)]
            })
            .collect(),
        truncated: false,
    }
}

/// An author's workbook as a file: a Summary sheet whose formulas name the Orders data before it is there, as a
/// workbook made elsewhere does, and a sheet of the author's own values.
fn uploaded() -> Model<'static> {
    let mut model = fill::new_model("Summary").unwrap();
    for (row, formula) in ["=SUM(Orders[amount])", "=COUNTA(Orders!A:A)-1", "=Notes!A1*2"]
        .iter()
        .enumerate()
    {
        model
            .set_user_input(0, i32::try_from(row + 1).unwrap(), 1, (*formula).to_string())
            .unwrap();
    }
    model.add_sheet("Notes").unwrap();
    model.set_user_input(1, 1, 1, "21".to_string()).unwrap();
    model.evaluate();
    let bytes = moqui_core::workbook::model_xlsx(&model).unwrap();
    moqui_core::workbook::open_xlsx(&bytes, "upload.xlsx").unwrap()
}

#[test]
fn an_uploaded_workbooks_formulas_read_the_data_sheet_filled_from_the_source() {
    let model = uploaded();
    assert_eq!(overlay_refusal(&model, &["Orders".to_string()]), None);
    // read from the file, a table it does not have yet is an error until the formula is typed again
    let mut unready = fill::fill(uploaded(), &[orders(3)]).unwrap();
    unready.evaluate();
    assert_eq!(unready.get_formatted_cell_value(0, 1, 1).unwrap(), "#ERROR!");
    // prepared over the data sheet's stub, then filled as a room fills it: every formula reads the source's rows
    let prepared = prepare(model, &[orders(3)]).unwrap();
    assert_eq!(prepared.workbook.get_worksheet_names(), ["Summary", "Notes", "Orders"]);
    assert_eq!(
        prepared.get_formatted_cell_value(2, 2, 1).unwrap(),
        "",
        "no row of the source is in the prepared workbook"
    );
    let stored = fill::open_stored(&prepared.to_bytes()).unwrap();
    let mut filled = fill::fill(stored, &[orders(3)]).unwrap();
    filled.evaluate();
    let text = |row| filled.get_formatted_cell_value(0, row, 1).unwrap();
    assert_eq!(text(1), "60", "the data sheet's table, by its column");
    assert_eq!(text(2), "3", "the data sheet by its name");
    assert_eq!(text(3), "42", "the author's own sheet");
    // filled again with more rows, the table's range stretches as a saved document's does
    let mut more = fill::fill(fill::open_stored(&prepared.to_bytes()).unwrap(), &[orders(5)]).unwrap();
    more.evaluate();
    assert_eq!(more.get_formatted_cell_value(0, 1, 1).unwrap(), "150");
}

#[test]
fn a_sheet_table_or_name_of_the_data_sheets_is_refused() {
    let data = ["Orders".to_string()];
    // a sheet of the data sheet's name, whatever its case: its rows would stand below the data
    let mut model = uploaded();
    model.add_sheet("orders").unwrap();
    let refused = overlay_refusal(&model, &data).unwrap_or_default();
    assert!(refused.contains("sheet named orders"), "{refused}");
    // a table named as the data sheet's, on another sheet
    let mut model = fill::fill(fill::new_model("Summary").unwrap(), &[orders(2)]).unwrap();
    model.rename_sheet_by_index(1, "Old orders").unwrap();
    let mut model = Model::from_bytes(&model.to_bytes(), "en").unwrap();
    assert!(model.workbook.tables.values().any(|table| table.name == "Orders"));
    let refused = overlay_refusal(&model, &data).unwrap_or_default();
    assert!(refused.contains("table named Orders"), "{refused}");
    // a defined name of it
    model.workbook.tables.clear();
    let mut model = Model::from_workbook(model.workbook, "en").unwrap();
    model
        .new_defined_name(DefinedNameEntry::new("ORDERS", "Summary!$A$1"))
        .unwrap();
    let refused = overlay_refusal(&model, &data).unwrap_or_default();
    assert!(refused.contains("name ORDERS"), "{refused}");
    // a table name that is the sheet's with its spaces made _
    let sheets = ["Open orders".to_string()];
    let mut model = fill::fill(
        fill::new_model("Summary").unwrap(),
        &[SheetRows {
            name: "Open orders".into(),
            ..orders(1)
        }],
    )
    .unwrap();
    model.rename_sheet_by_index(1, "Kept").unwrap();
    let model = Model::from_bytes(&model.to_bytes(), "en").unwrap();
    assert!(overlay_refusal(&model, &sheets).is_some_and(|refused| refused.contains("Open_orders")));
}
