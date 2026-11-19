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

use std::collections::BTreeMap;

use moqui_lib::etl::SimpleEtl;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};

use super::source::{self, SourceOptions};
use super::{inline_spreadsheet_load, match_fields};

/// `load#SpreadsheetEntity` through its inline mapping: the header names the fields (`fieldMap` renaming one), a
/// column that is no field is left out, the rows go through the entity-auto store in the caller's transaction, and a
/// row that fails stops the load with its error.
#[test]
fn a_spreadsheet_loads_through_an_inline_mapping() {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let book = |rows: &[[&str; 4]]| {
        let mut workbook = rust_xlsxwriter::Workbook::new();
        let sheet = workbook.add_worksheet();
        for (column, title) in ["uomId", "Name", "uomTypeEnumId", "ignored column"].iter().enumerate() {
            sheet.write_string(0, column as u16, *title).unwrap();
        }
        for (row, cells) in rows.iter().enumerate() {
            for (column, cell) in cells.iter().enumerate() {
                sheet.write_string(row as u32 + 1, column as u16, *cell).unwrap();
            }
        }
        workbook.save_to_buffer().unwrap()
    };
    let mut call = BTreeMap::new();
    call.insert("entityName".to_string(), Value::string("moqui.basic.Uom"));
    call.insert(
        "contentBytes".to_string(),
        Value::Bytes(book(&[
            ["ETL_SHEET_A", "Sheet A", "UT_LENGTH_MEASURE", "x"],
            ["ETL_SHEET_B", "Sheet B", "UT_LENGTH_MEASURE", "x"],
        ])),
    );
    let mut field_map = indexmap::IndexMap::default();
    field_map.insert("Name".to_string(), Value::string("description"));
    call.insert("fieldMap".to_string(), Value::Map(field_map));
    let out = inline_spreadsheet_load(&call).unwrap();
    assert_eq!(out.get("recordCount"), Some(&Value::I64(2)));
    let row = facade
        .find("moqui.basic.Uom")
        .unwrap()
        .eq("uomId", Value::string("ETL_SHEET_B"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(row.get("description").map(to_plain_string).as_deref(), Some("Sheet B"));
    // an unknown load mode is refused before any row
    call.insert("loadMode".to_string(), Value::string("merge"));
    assert!(inline_spreadsheet_load(&call).is_err());
    let _ = ExecutionContext::with_current(|ec| ec.message().clear_errors());
}

/// Columns match target fields by name, then by their words, then by the nearest name; each field once.
#[test]
fn columns_match_by_name_then_words_then_nearest() {
    let columns: Vec<String> = ["productId", "Product Name", "QTY", "Descripton", "Unrelated Column"]
        .iter()
        .map(|column| (*column).to_string())
        .collect();
    let fields: Vec<String> = ["productId", "productName", "quantity", "description", "qty"]
        .iter()
        .map(|field| (*field).to_string())
        .collect();
    let matches: Vec<(String, Option<String>, &str)> = match_fields(&columns, &fields);
    let found: Vec<(&str, Option<&str>, &str)> = matches
        .iter()
        .map(|(column, field, how)| (column.as_str(), field.as_deref(), *how))
        .collect();
    assert_eq!(
        found,
        [
            ("productId", Some("productId"), "name"),
            ("Product Name", Some("productName"), "prettyName"),
            ("QTY", Some("qty"), "name"),
            ("Descripton", Some("description"), "nearest"),
            ("Unrelated Column", None, "none"),
        ]
    );
}

/// An entity source reads no encrypted or password field into a row, and no source condition or lookup matches on one
/// or answers it: the entity facade decrypts an encrypted field as it reads it, so a row, a check's sample or a lookup
/// would answer it in clear (plans/Security-Review-Medium.md SEC-TLS-89).
#[test]
fn an_entity_source_and_a_lookup_read_no_encrypted_or_password_field() {
    use super::Mapping;
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut endpoint = facade.make_value("moqui.llm.LlmEndpoint").unwrap();
    endpoint.set("endpointId", Value::string("ETL_SECRET"));
    endpoint.set("description", Value::string("Endpoint"));
    endpoint.set("apiKey", Value::string("stored-api-key"));
    endpoint.create(&facade).unwrap();
    let field = |pairs: &[(&str, &str)]| {
        Value::Map(
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), Value::string(*value)))
                .collect(),
        )
    };
    let mapping = |fields: Vec<Value>, conditions: &[(&str, &str)]| {
        let mut map = indexmap::IndexMap::default();
        map.insert("sourceTypeEnumId".to_string(), Value::string("EtlsEntity"));
        map.insert("sourceEntityName".to_string(), Value::string("moqui.llm.LlmEndpoint"));
        map.insert("targetTypeEnumId".to_string(), Value::string("EtltEntity"));
        map.insert("targetName".to_string(), Value::string("moqui.basic.Enumeration"));
        map.insert("fields".to_string(), Value::List(fields));
        map.insert(
            "sourceConditions".to_string(),
            Value::Map(
                conditions
                    .iter()
                    .map(|(key, value)| (key.to_string(), Value::string(*value)))
                    .collect(),
            ),
        );
        Mapping::from_map(&map).unwrap()
    };
    let plain = mapping(
        vec![field(&[("targetField", "enumId"), ("sourceColumn", "endpointId")])],
        &[],
    );
    plain.validate_with(&facade).unwrap();
    let source = source::open(&facade, &plain, &SourceOptions::default()).unwrap();
    let rows = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut etl = SimpleEtl::from_boxes(source.extractor, Box::new(super::Collect(std::rc::Rc::clone(&rows))));
    etl.process();
    let rows = rows.borrow();
    let row = rows
        .iter()
        .find(|row| row.get("endpointId").map(to_plain_string).as_deref() == Some("ETL_SECRET"))
        .expect("the endpoint's row");
    assert_eq!(row.get("description").map(to_plain_string).as_deref(), Some("Endpoint"));
    assert!(!row.contains_key("apiKey"), "{row:?}");
    // a condition on it would tell whether a guess is its value
    let conditioned = mapping(
        vec![field(&[("targetField", "enumId"), ("sourceColumn", "endpointId")])],
        &[("apiKey", "guess")],
    );
    let refused = conditioned.validate_with(&facade).unwrap_err().to_string();
    assert!(
        refused.contains("apiKey is not available in an ETL source condition"),
        "{refused}"
    );
    // a lookup neither answers it nor matches on it
    for (matched, result) in [("endpointId", "apiKey"), ("apiKey", "endpointId")] {
        let looked_up = mapping(
            vec![
                field(&[("targetField", "enumId"), ("sourceColumn", "endpointId")]),
                field(&[
                    ("targetField", "description"),
                    ("sourceColumn", "endpointId"),
                    ("lookupEntityName", "moqui.llm.LlmEndpoint"),
                    ("lookupMatchField", matched),
                    ("lookupResultField", result),
                ]),
            ],
            &[],
        );
        let refused = looked_up.validate_with(&facade).unwrap_err().to_string();
        assert!(
            refused.contains("apiKey is not available in an ETL lookup"),
            "{refused}"
        );
    }
}
