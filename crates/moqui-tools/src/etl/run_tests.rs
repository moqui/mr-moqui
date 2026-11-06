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

//! ETL runs on the in-memory facade (plans/archive/Tools-Database.md §15 step 6): the transformer's conversions (an
//! expression only for a user with `GROOVY_SHELL_WEB`), number patterns, a batched run that fails in its third batch,
//! a batched run stopped by a transform error, and a check that finds a missing foreign key and the diff.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::etl::SharedHeaders;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;

use super::{run, RunOptions};
use crate::etl::mapping::Mapping;
use crate::etl::source::SourceOptions;
use crate::etl::transform::{parse_formatted, ErrorLog, MappingTransformer};

fn setup() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    (ctx, facade)
}

fn map(pairs: &[(&str, Value)]) -> IndexMap<String, Value> {
    pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn enum_type(facade: &EntityFacade, id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value("moqui.basic.EnumerationType").unwrap();
    row.set("enumTypeId", Value::string(id));
    row.set("description", Value::string(id));
    row.create(facade).unwrap();
}

/// A user `etl.u` in a group granted `GROOVY_SHELL_WEB`, signed in.
fn sign_in_script_user(facade: &EntityFacade) {
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let rows: [(&str, &[(&str, &str)]); 5] = [
            (
                "moqui.security.UserAccount",
                &[("userId", "EtlU"), ("username", "etl.u"), ("disabled", "N")],
            ),
            ("moqui.security.UserGroup", &[("userGroupId", "EtlScripts")]),
            (
                "moqui.security.UserPermission",
                &[("userPermissionId", "GROOVY_SHELL_WEB")],
            ),
            (
                "moqui.security.UserGroupMember",
                &[("userGroupId", "EtlScripts"), ("userId", "EtlU")],
            ),
            (
                "moqui.security.UserGroupPermission",
                &[("userGroupId", "EtlScripts"), ("userPermissionId", "GROOVY_SHELL_WEB")],
            ),
        ];
        for (entity, fields) in rows {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(*name, Value::string(*value));
            }
            if entity.ends_with("Member") || entity.ends_with("GroupPermission") {
                row.set("fromDate", Value::Timestamp(jiff::Timestamp::UNIX_EPOCH));
            }
            row.create(facade).unwrap();
        }
    }
    moqui_core::user::internal_login_user("etl.u").unwrap();
}

fn exists(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> bool {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(entity)
        .unwrap()
        .eq(field, Value::string(id))
        .one()
        .unwrap()
        .is_some()
}

/// A value map (case-insensitive), a lookup, a date and a number format, an expression with `row` and `value`, and a
/// required field.
#[test]
fn the_transformer_maps_looks_up_formats_and_evaluates() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    enum_type(&facade, "EtlLookupType");
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
        row.set("enumId", Value::string("EtlBlue"));
        row.set("enumTypeId", Value::string("EtlLookupType"));
        row.set("description", Value::string("Blue"));
        row.create(&facade).unwrap();
    }
    let fields = Value::List(vec![
        Value::Map(map(&[
            ("targetField", text("statusId")),
            ("sourceColumn", text("Status")),
            (
                "valueMap",
                Value::Map(map(&[("Active", text("ACTIVE")), ("inactive", text("INACTIVE"))])),
            ),
        ])),
        Value::Map(map(&[
            ("targetField", text("colorId")),
            ("sourceColumn", text("Color")),
            ("lookupEntityName", text("moqui.basic.Enumeration")),
            ("lookupMatchField", text("description")),
        ])),
        Value::Map(map(&[
            ("targetField", text("when")),
            ("sourceColumn", text("C")),
            ("formatPattern", text("dd/MM/yyyy")),
        ])),
        Value::Map(map(&[
            ("targetField", text("amount")),
            ("sourceColumn", text("Amount")),
            ("formatPattern", text("#.##0,00")),
        ])),
        Value::Map(map(&[
            ("targetField", text("label")),
            ("sourceColumn", text("Name")),
            ("trim", text("Y")),
            ("expression", text("value.toUpperCase() + '/' + row.Status")),
        ])),
        Value::Map(map(&[
            ("targetField", text("code")),
            ("sourceColumn", text("Code")),
            ("required", text("Y")),
        ])),
        Value::Map(map(&[
            ("targetField", text("note")),
            ("sourceColumn", text("Note")),
            ("skipIfEmpty", text("Y")),
        ])),
        Value::Map(map(&[("targetField", text("kind")), ("constantValue", text("K1"))])),
    ]);
    let mapping = Mapping::from_map(&map(&[
        ("targetTypeEnumId", text("EtltService")),
        ("targetName", text("org.example.Nothing.do#It")),
        ("fields", fields),
    ]))
    .unwrap();
    let headers: SharedHeaders = Arc::new(Mutex::new(vec![
        "Status".to_string(),
        "Color".to_string(),
        "When".to_string(),
        "Amount".to_string(),
    ]));
    let errors = Arc::new(Mutex::new(ErrorLog::default()));
    let row: BTreeMap<String, Value> = [
        ("Status", "aCTIVE"),
        ("Color", "Blue"),
        ("When", "05/11/2026"),
        ("Amount", "1.234,50"),
        ("Name", "  ann "),
        ("Code", "X1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::string(v)))
    .collect();
    // with no GROOVY_SHELL_WEB the expression is refused, the other fields map
    let refused = MappingTransformer::new(&facade, &mapping, Arc::clone(&headers), Arc::clone(&errors))
        .transform_row(&row)
        .unwrap_err();
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].0, "label");
    assert!(refused[0].1.contains("GROOVY_SHELL_WEB"), "{refused:?}");
    sign_in_script_user(&facade);
    let transformer = MappingTransformer::new(&facade, &mapping, headers, Arc::clone(&errors));
    let out = transformer.transform_row(&row).unwrap();
    assert_eq!(out["statusId"], text("ACTIVE"));
    assert_eq!(out["colorId"], text("EtlBlue"));
    assert_eq!(out["when"], Value::Date(jiff::civil::date(2026, 11, 5)));
    assert_eq!(to_plain_string(&out["amount"]), "1234.50");
    assert_eq!(out["label"], text("ANN/aCTIVE"));
    assert_eq!(out["kind"], text("K1"));
    assert!(!out.contains_key("note"), "skipIfEmpty leaves it out");
    // a miss of the value map, a lookup with no record, and an empty required field: each its field's error
    let mut bad = row.clone();
    bad.insert("Status".to_string(), text("Pending"));
    bad.insert("Color".to_string(), text("Mauve"));
    bad.remove("Code");
    let failed = transformer.transform_row(&bad).unwrap_err();
    let fields: Vec<&str> = failed.iter().map(|(field, _)| field.as_str()).collect();
    assert_eq!(fields, ["statusId", "colorId", "code"], "{failed:?}");
    assert!(failed[2].1.contains("required"));
    assert!(parse_formatted("abc", "#,##0.00").is_err());
    assert!(parse_formatted("2026-13-01", "yyyy-MM-dd").is_err());
}

/// Number patterns as Java's `DecimalFormat` reads them: `#,##0` and `#.##0` group a whole number; the decimal
/// separator is the last one with digits after it that does not group.
#[test]
fn number_patterns_tell_grouping_from_the_decimal_separator() {
    let number = |text: &str, pattern: &str| to_plain_string(&parse_formatted(text, pattern).unwrap());
    assert_eq!(number("1,234", "#,##0"), "1234");
    assert_eq!(number("1.234", "#.##0"), "1234");
    assert_eq!(number("1,234,567", "#,##0"), "1234567");
    assert_eq!(number("1,234.50", "#,##0.00"), "1234.50");
    assert_eq!(number("1.234,50", "#.##0,00"), "1234.50");
    assert_eq!(number("3.25", "0.00"), "3.25");
    assert_eq!(number("3.125", "0.000"), "3.125");
    assert!(parse_formatted("1,234.5.6", "#,##0.00").is_err());
}

fn description(facade: &EntityFacade, id: &str) -> String {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find("moqui.basic.EnumerationType")
        .unwrap()
        .eq("enumTypeId", Value::string(id))
        .one()
        .unwrap()
        .and_then(|row| row.get("description").map(to_plain_string))
        .unwrap_or_default()
}

/// An inline mapping of CSV rows updating EnumerationType in batches of two: the row with no record to update fails
/// the third batch, which is rolled back, the first two batches stay, the run stops there, and it reports the last
/// committed row.
#[test]
fn a_batched_run_keeps_the_batches_before_the_one_that_failed() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    for id in ["EtlB1", "EtlB2", "EtlB3", "EtlB4", "EtlB5", "EtlB7"] {
        enum_type(&facade, id);
    }
    let csv = "id,name\nEtlB1,new\nEtlB2,new\nEtlB3,new\nEtlB4,new\nEtlB5,new\nEtlB6,new\nEtlB7,new\n";
    let mapping = Mapping::from_map(&map(&[
        ("sourceTypeEnumId", text("EtlsFile")),
        ("fileFormat", text("csv")),
        ("targetTypeEnumId", text("EtltEntity")),
        ("targetName", text("moqui.basic.EnumerationType")),
        ("loadModeEnumId", text("EtllUpdate")),
        ("batchSize", Value::I64(2)),
        (
            "fields",
            Value::List(vec![
                Value::Map(map(&[
                    ("targetField", text("enumTypeId")),
                    ("sourceColumn", text("id")),
                ])),
                Value::Map(map(&[
                    ("targetField", text("description")),
                    ("sourceColumn", text("B")),
                ])),
            ]),
        ),
    ]))
    .unwrap();
    let outcome = run(
        &facade,
        &mapping,
        &RunOptions {
            source: SourceOptions {
                content: Some(csv.as_bytes().to_vec()),
                ..SourceOptions::default()
            },
            ..RunOptions::default()
        },
    );
    for id in ["EtlB1", "EtlB2", "EtlB3", "EtlB4"] {
        assert_eq!(description(&facade, id), "new", "{id} {outcome:?}");
    }
    // EtlB5 was in the batch EtlB6 failed; the run stopped before EtlB7
    for id in ["EtlB5", "EtlB7"] {
        assert_eq!(description(&facade, id), id, "{id} {outcome:?}");
    }
    // the header is line 1, so EtlB4 is row 5, and a rerun starts at row 6
    assert_eq!(outcome.last_committed_row, 5, "{outcome:?}");
    assert_eq!(outcome.next_start_row, 6, "{outcome:?}");
    assert_eq!(outcome.load_count, 4, "only the committed rows: {outcome:?}");
    assert_eq!(outcome.committed_batches, 2, "{outcome:?}");
    assert_eq!(outcome.failed_batches, 1, "{outcome:?}");
    assert_eq!(
        outcome
            .failed_ranges
            .iter()
            .map(|range| (range.from_row, range.to_row))
            .collect::<Vec<_>>(),
        [(6, 7)],
        "{outcome:?}"
    );
    assert_eq!(
        outcome.errors.first().map(|error| error.row_number),
        Some(7),
        "{outcome:?}"
    );
    assert!(
        outcome
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("row 7")),
        "{outcome:?}"
    );
}

/// With a batch size and `stopOnError`, a transform error ends the run as failed with its row reported, and the batch
/// it was in is rolled back: only the batches committed before it stay.
#[test]
fn a_transform_error_stops_a_batched_run_as_failed() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    for id in ["EtlS1", "EtlS2", "EtlS3", "EtlS4"] {
        enum_type(&facade, id);
    }
    let csv = "id,name\nEtlS1,new\nEtlS2,new\nEtlS3,new\nEtlS4,\n";
    let mapping = Mapping::from_map(&map(&[
        ("fileFormat", text("csv")),
        ("targetName", text("moqui.basic.EnumerationType")),
        ("loadModeEnumId", text("EtllUpdate")),
        ("batchSize", Value::I64(2)),
        ("stopOnError", text("Y")),
        (
            "fields",
            Value::List(vec![
                Value::Map(map(&[
                    ("targetField", text("enumTypeId")),
                    ("sourceColumn", text("id")),
                ])),
                Value::Map(map(&[
                    ("targetField", text("description")),
                    ("sourceColumn", text("name")),
                    ("required", text("Y")),
                ])),
            ]),
        ),
    ]))
    .unwrap();
    let outcome = run(
        &facade,
        &mapping,
        &RunOptions {
            source: SourceOptions {
                content: Some(csv.as_bytes().to_vec()),
                ..SourceOptions::default()
            },
            ..RunOptions::default()
        },
    );
    assert!(
        outcome
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("row 5")),
        "{outcome:?}"
    );
    assert_eq!(outcome.error_count, 1, "{outcome:?}");
    assert_eq!(outcome.load_count, 2, "{outcome:?}");
    assert_eq!(outcome.last_committed_row, 3, "{outcome:?}");
    assert_eq!(description(&facade, "EtlS2"), "new");
    // EtlS3 was in the batch the error stopped
    assert_eq!(description(&facade, "EtlS3"), "EtlS3", "{outcome:?}");
    assert_eq!(outcome.failed_ranges.len(), 1, "{outcome:?}");
}

/// A check finds the foreign key with no related record, and the rows that would be new, without writing.
#[test]
fn a_check_reports_a_missing_foreign_key_and_the_diff() {
    let (ctx, facade) = setup();
    let _entered = ctx.enter();
    enum_type(&facade, "EtlCheckType");
    let csv = "id,type\nEtlC1,EtlCheckType\nEtlC2,NoSuchType\nEtlC3,NoSuchType\n";
    let mapping = Mapping::from_map(&map(&[
        ("fileFormat", text("csv")),
        ("targetName", text("moqui.basic.Enumeration")),
        // a check never stops at an error, whatever the batches say
        ("batchSize", Value::I64(1)),
        ("stopOnError", text("Y")),
        (
            "fields",
            Value::List(vec![
                Value::Map(map(&[("targetField", text("enumId")), ("sourceColumn", text("id"))])),
                Value::Map(map(&[
                    ("targetField", text("enumTypeId")),
                    ("sourceColumn", text("type")),
                ])),
            ]),
        ),
    ]))
    .unwrap();
    let outcome = run(
        &facade,
        &mapping,
        &RunOptions {
            source: SourceOptions {
                content: Some(csv.as_bytes().to_vec()),
                ..SourceOptions::default()
            },
            check: true,
            ..RunOptions::default()
        },
    );
    assert_eq!(outcome.load_count, 0);
    assert!(!exists(&facade, "moqui.basic.Enumeration", "enumId", "EtlC1"));
    assert_eq!(outcome.errors.len(), 2, "both bad rows, the check went on: {outcome:?}");
    assert_eq!(outcome.error_count, 2, "{outcome:?}");
    let error = &outcome.errors[0];
    assert_eq!(error.row_number, 3);
    assert_eq!(error.field, "enumTypeId");
    assert!(error.message.contains("EnumerationType"), "{error:?}");
    assert!(outcome.failure.is_none(), "{outcome:?}");
    // each row would be new
    let new_ids: Vec<String> = outcome
        .diff
        .iter()
        .filter(|row| row.get("notFound") == Some(&Value::Bool(true)))
        .filter_map(|row| match row.get("pk") {
            Some(Value::Map(pk)) => pk.get("enumId").map(to_plain_string),
            _ => None,
        })
        .collect();
    assert_eq!(new_ids, ["EtlC1", "EtlC2", "EtlC3"], "{:?}", outcome.diff);
}

/// A service target that deletes, or an entity-auto service of a restricted entity, is refused, as an entity target
/// of one is; a plain service and an entity-auto service of an open entity pass.
#[test]
fn service_targets_that_delete_or_reach_restricted_entities_are_refused() {
    let target = |kind: &str, name: &str| {
        Mapping::from_map(&map(&[("targetTypeEnumId", text(kind)), ("targetName", text(name))]))
            .unwrap()
            .validate()
    };
    for refused in [
        ("EtltService", "update#moqui.database.DatabaseConnection"),
        ("EtltService", "create#moqui.database.SqlRunToken"),
        ("EtltService", "update#moqui.etl.EtlMappingField"),
        ("EtltService", "create#moqui.security.UserGroupMember"),
        ("EtltService", "delete#moqui.basic.Enumeration"),
        ("EtltService", "org.example.Things.delete#Thing"),
        ("EtltEntity", "moqui.etl.EtlMappingField"),
        ("EtltEntity", "moqui.database.DatabaseAudit"),
    ] {
        assert!(target(refused.0, refused.1).is_err(), "{refused:?}");
    }
    for allowed in [
        ("EtltService", "store#moqui.basic.Enumeration"),
        ("EtltService", "org.example.Things.create#Thing"),
        ("EtltEntity", "moqui.basic.Enumeration"),
    ] {
        assert!(target(allowed.0, allowed.1).is_ok(), "{allowed:?}");
    }
}
