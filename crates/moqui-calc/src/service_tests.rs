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
    let parts: [(&str, &str); 5] = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Costs" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
        ),
        (
            "xl/worksheets/sheet1.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>item</t></is></c><c r="B1" t="inlineStr"><is><t>cost</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>Truck</t></is></c><c r="B2"><v>120.5</v></c></row></sheetData></worksheet>"#,
        ),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, text) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(text.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
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
