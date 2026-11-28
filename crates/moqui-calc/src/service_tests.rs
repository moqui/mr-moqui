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

use std::io::Write;

use moqui_lib::object::Value;

use super::*;

/// An xlsx as a minimal writer makes one: a workbook and a sheet, no styles part, which the engine's reader refuses.
fn minimal_xlsx() -> Vec<u8> {
    xlsx_without_styles(&[(
        "Costs",
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>item</t></is></c><c r="B1" t="inlineStr"><is><t>cost</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Truck</t></is></c><c r="B2"><v>120.5</v></c></row>"#,
    )])
}

/// An xlsx of `sheets` (each a name and its `sheetData` rows) with no styles part, which the engine's reader refuses.
fn xlsx_without_styles(sheets: &[(&str, &str)]) -> Vec<u8> {
    let numbered = || sheets.iter().enumerate().map(|(index, sheet)| (index + 1, sheet));
    let overrides: String = numbered()
        .map(|(number, _)| format!(r#"<Override PartName="/xl/worksheets/sheet{number}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#))
        .collect();
    let listed: String = numbered()
        .map(|(number, (name, _))| format!(r#"<sheet name="{name}" sheetId="{number}" r:id="rId{number}"/>"#))
        .collect();
    let relations: String = numbered()
        .map(|(number, _)| format!(r#"<Relationship Id="rId{number}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{number}.xml"/>"#))
        .collect();
    let mut parts: Vec<(String, String)> = vec![
        (
            "[Content_Types].xml".to_string(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>{overrides}</Types>"#),
        ),
        (
            "_rels/.rels".to_string(),
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/workbook.xml".to_string(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{listed}</sheets></workbook>"#),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_string(),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{relations}</Relationships>"#),
        ),
    ];
    for (number, (_, rows)) in numbered() {
        parts.push((
            format!("xl/worksheets/sheet{number}.xml"),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#),
        ));
    }
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// A sheet whose first used cell is not A1 opens with each value in its own cell: calamine's range starts at the first
/// used row and column, and the fallback places the values from there (plans/IronCalc-DB2.md CG1.5).
#[test]
fn a_sheet_the_engine_refuses_keeps_its_cells_where_they_are_when_it_starts_past_a1() {
    let bytes = xlsx_without_styles(&[
        (
            "Costs",
            r#"<row r="3"><c r="C3" t="inlineStr"><is><t>item</t></is></c><c r="D3" t="inlineStr"><is><t>cost</t></is></c></row><row r="4"><c r="C4" t="inlineStr"><is><t>Truck</t></is></c><c r="E4"><v>120.5</v></c></row>"#,
        ),
        (
            "Notes",
            r#"<row r="2"><c r="B2" t="inlineStr"><is><t>first</t></is></c></row><row r="5"><c r="D5"><v>7</v></c></row>"#,
        ),
    ]);
    assert!(
        workbook::open_xlsx(&bytes, "offset").is_err(),
        "the fallback is what reads it"
    );
    let model = workbook_model(&bytes, "offset").unwrap();
    assert_eq!(model.workbook.get_worksheet_names(), ["Costs", "Notes"]);
    let value = |sheet, row, column| model.get_formatted_cell_value(sheet, row, column).unwrap();
    assert_eq!(value(0, 3, 3), "item");
    assert_eq!(value(0, 3, 4), "cost");
    assert_eq!(value(0, 4, 3), "Truck");
    assert_eq!(value(0, 4, 4), "", "an empty cell between stays empty");
    assert_eq!(value(0, 4, 5), "120.5");
    assert_eq!(value(0, 3, 1), "", "nothing moved left to column A");
    assert_eq!(value(1, 2, 2), "first");
    assert_eq!(value(1, 5, 4), "7");
    assert_eq!(value(1, 2, 1), "");
}

#[test]
fn a_workbook_the_engine_refuses_opens_as_values_and_one_it_takes_keeps_its_formulas() {
    let bytes = minimal_xlsx();
    assert!(
        workbook::open_xlsx(&bytes, "minimal").is_err(),
        "the engine's reader needs a styles part"
    );
    let model = workbook_model(&bytes, "minimal").unwrap();
    assert_eq!(model.workbook.get_worksheet_names(), ["Costs"]);
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "item");
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "Truck");
    assert_eq!(model.get_formatted_cell_value(0, 2, 2).unwrap(), "120.5");

    // a workbook the engine writes is one it reads, with the formula
    let mut made = fill::new_model("Sheet1").unwrap();
    made.set_user_input(0, 1, 1, "2".to_string()).unwrap();
    made.set_user_input(0, 2, 1, "=A1*21".to_string()).unwrap();
    made.evaluate();
    let model = workbook_model(&workbook::model_xlsx(&made).unwrap(), "made").unwrap();
    assert_eq!(model.get_cell_formula(0, 2, 1).unwrap().as_deref(), Some("=A1*21"));
    assert!(workbook_model(b"not a workbook", "bad").is_err());
}

#[test]
fn a_grid_value_is_a_typed_cell_and_a_source_names_its_sheet() {
    let zone = jiff::tz::TimeZone::UTC;
    assert_eq!(cell_data(&Value::Null, &zone), CellData::Empty);
    assert_eq!(cell_data(&Value::string(""), &zone), CellData::Empty);
    assert_eq!(
        cell_data(&Value::string("=1+1"), &zone),
        CellData::Text("=1+1".to_string())
    );
    assert_eq!(cell_data(&Value::I64(7), &zone), CellData::Number(7.0));
    assert_eq!(cell_data(&Value::Bool(true), &zone), CellData::Bool(true));
    assert_eq!(
        cell_data(&Value::Date(jiff::civil::date(2024, 3, 1)), &zone),
        CellData::Date(45_352.0)
    );
    let stamp: jiff::Timestamp = "2024-03-01T12:00:00Z".parse().unwrap();
    assert_eq!(cell_data(&Value::Timestamp(stamp), &zone), CellData::DateTime(45_352.5));
    assert_eq!(cell_data(&Value::Bytes(vec![1, 2]), &zone), CellData::Empty);

    let name = |spec: Json| source_sheet_name(&json!({ "source": spec }));
    assert_eq!(
        name(json!({"type": "entity", "entityName": "mantle.order.OrderHeader"})),
        "OrderHeader"
    );
    assert_eq!(name(json!({"type": "sql", "sql": "select 1"})), "Data");
    assert_eq!(
        name(json!({"type": "file", "location": "dbresource://moqui/etl/costs.csv"})),
        "costs.csv"
    );
    assert_eq!(
        name(json!({"type": "file", "location": "x/costs.xlsx", "sheet": "2026/Q1"})),
        "2026_Q1"
    );
    assert_eq!(name(json!({})), "Data");
}
