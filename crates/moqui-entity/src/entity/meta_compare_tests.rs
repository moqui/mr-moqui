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

use moqui_db::dialect::Dialect;
use moqui_db::model::{EntityDefinition, EntityModel};
use moqui_db::sql::{field_sql_type, foreign_key_name, physical_column_name};

use super::{diff_table, expected_indexes, is_key_relationship, key_columns, Catalog, CatalogForeignKey, CatalogIndex};
use super::{CatalogTable, IndexKind, Rules, TableDiff};
use crate::testing;

/// The catalog of a database the table check made `entity`'s table in, with the tables it refers to.
fn made_catalog(entity: &EntityDefinition, model: &EntityModel, dialect: &Dialect) -> Catalog {
    let mut catalog = Catalog::default();
    let mut table = CatalogTable {
        name: entity.table_name.to_ascii_lowercase(),
        ..CatalogTable::default()
    };
    for field in &entity.fields {
        let sql_type = field_sql_type(model, dialect, field).unwrap();
        table
            .columns
            .push((physical_column_name(dialect, &field.column_name), sql_type));
    }
    table.indexes.push(CatalogIndex {
        name: format!("{}_pkey", table.name),
        columns: vec!["enum_id".to_string()],
        unique: true,
        primary: true,
        valid: true,
        constraint: true,
    });
    for index in expected_indexes(entity, dialect) {
        table.indexes.push(CatalogIndex {
            name: index.name.to_ascii_lowercase(),
            columns: index.columns,
            unique: index.unique,
            valid: true,
            ..CatalogIndex::default()
        });
    }
    for relationship in entity.relationships.iter().filter(|r| is_key_relationship(r)) {
        let related = model.entity(&relationship.related).unwrap();
        table.foreign_keys.push(CatalogForeignKey {
            name: foreign_key_name(entity, relationship, dialect).to_ascii_lowercase(),
            referenced_table: related.table_name.to_ascii_lowercase(),
            columns: key_columns(entity, dialect, relationship),
            valid: true,
        });
        catalog
            .tables
            .entry(related.table_name.to_ascii_lowercase())
            .or_insert_with(|| CatalogTable {
                name: related.table_name.to_ascii_lowercase(),
                ..CatalogTable::default()
            });
    }
    catalog.tables.insert(table.name.clone(), table);
    catalog
}

fn compared(
    change: impl FnOnce(&mut Catalog, &EntityDefinition, &Dialect),
    dialect_change: fn(&mut Dialect),
) -> TableDiff {
    let facade = testing::open();
    facade
        .with_entity("moqui.basic.Enumeration", |entity, dialect, model| {
            let mut dialect = dialect.clone();
            dialect_change(&mut dialect);
            let mut catalog = made_catalog(entity, model, &dialect);
            change(&mut catalog, entity, &dialect);
            let same = |_: Option<&str>, _: Option<&str>| true;
            let rules = Rules {
                foreign_keys: true,
                same_database: &same,
            };
            let mut errors = Vec::new();
            let diff = diff_table(entity, model, &dialect, &catalog, &rules, &mut errors);
            assert!(errors.is_empty(), "{errors:?}");
            Ok(diff)
        })
        .unwrap()
}

fn table<'a>(catalog: &'a mut Catalog, entity: &EntityDefinition) -> &'a mut CatalogTable {
    catalog.tables.get_mut(&entity.table_name.to_ascii_lowercase()).unwrap()
}

#[test]
fn a_table_as_the_check_makes_it_has_no_difference() {
    let diff = compared(|_, _, _| {}, |dialect| dialect.use_foreign_key_indexes = true);
    assert!(diff.is_empty(), "{diff:#?}");
    let diff = compared(|catalog, _, _| catalog.tables.clear(), |_| {});
    assert!(diff.missing_table, "{diff:#?}");
}

/// A dropped index, an extra index and a missing key, with an extra column and a changed type.
#[test]
fn a_dropped_index_an_extra_index_and_a_missing_key_are_reported() {
    let diff = compared(
        |catalog, entity, dialect| {
            let table = table(catalog, entity);
            let dropped = table
                .indexes
                .iter()
                .position(|index| !index.primary)
                .expect("an index of the entity");
            table.indexes.remove(dropped);
            table.indexes.push(CatalogIndex {
                name: "stray_idx".to_string(),
                columns: vec!["enum_code".to_string()],
                valid: true,
                ..CatalogIndex::default()
            });
            table.indexes.push(CatalogIndex {
                name: "half_made_idx".to_string(),
                columns: vec!["sequence_num".to_string()],
                valid: false,
                ..CatalogIndex::default()
            });
            let status_flow = foreign_key_name(
                entity,
                entity
                    .relationships
                    .iter()
                    .find(|relationship| relationship.related == "moqui.basic.StatusFlow")
                    .unwrap(),
                dialect,
            )
            .to_ascii_lowercase();
            table.foreign_keys.retain(|key| key.name != status_flow);
            table.columns.push(("stray_col".to_string(), "text".to_string()));
            let code = table.columns.iter_mut().find(|(name, _)| name == "enum_code").unwrap();
            code.1 = "character varying(20)".to_string();
        },
        |dialect| dialect.use_foreign_key_indexes = true,
    );
    let missing: Vec<(&str, IndexKind)> = diff
        .missing_indexes
        .iter()
        .map(|index| (index.name.as_str(), index.kind))
        .collect();
    assert_eq!(missing.len(), 1, "{diff:#?}");
    assert!(!diff.missing_indexes[0].ddl.is_empty(), "{diff:#?}");
    let extra: Vec<&str> = diff.extra_indexes.iter().map(|index| index.name.as_str()).collect();
    assert_eq!(extra, ["stray_idx", "half_made_idx"], "{diff:#?}");
    let invalid: Vec<&str> = diff.invalid_indexes.iter().map(|index| index.name.as_str()).collect();
    assert_eq!(invalid, ["half_made_idx"]);
    assert_eq!(diff.missing_foreign_keys.len(), 1, "{diff:#?}");
    assert_eq!(diff.missing_foreign_keys[0].related_entity, "moqui.basic.StatusFlow");
    assert!(diff.missing_foreign_keys[0]
        .ddl
        .as_deref()
        .is_some_and(|sql| sql.contains("FOREIGN KEY")));
    assert!(diff.extra_foreign_keys.is_empty());
    assert_eq!(diff.extra_columns, ["stray_col"]);
    assert_eq!(diff.column_types.len(), 1, "{diff:#?}");
    assert_eq!(diff.column_types[0].column, "enum_code");
    assert!(diff.missing_columns.is_empty());
}

/// Without foreign key indexes, a key no index leads with is reported on Postgres, and one an index leads with is not.
#[test]
fn an_unindexed_foreign_key_is_reported_when_the_dialect_makes_no_key_indexes() {
    let diff = compared(
        |catalog, entity, _| {
            let table = table(catalog, entity);
            table.indexes.push(CatalogIndex {
                name: "enum_type_lead_idx".to_string(),
                columns: vec!["enum_type_id".to_string(), "enum_code".to_string()],
                valid: true,
                ..CatalogIndex::default()
            });
        },
        |dialect| dialect.use_foreign_key_indexes = false,
    );
    let unindexed: Vec<&str> = diff
        .unindexed_foreign_keys
        .iter()
        .map(|key| key.referenced_table.as_str())
        .collect();
    assert_eq!(unindexed, ["status_flow"], "{diff:#?}");
    // The index is not one the check makes, so it is also extra.
    assert_eq!(diff.extra_indexes.len(), 1);
}

/// The answer as a service gives it.
#[test]
fn the_diff_is_a_map_of_camel_case_lists() {
    let diff = compared(
        |catalog, entity, _| table(catalog, entity).columns.retain(|(name, _)| name != "enum_code"),
        |_| {},
    );
    let schema = super::SchemaDiff {
        checked: 1,
        tables: vec![diff],
        errors: Vec::new(),
    };
    let moqui_lib::object::Value::Map(map) = schema.to_value() else {
        panic!("a map");
    };
    let moqui_lib::object::Value::List(tables) = &map["tables"] else {
        panic!("a list");
    };
    let moqui_lib::object::Value::Map(table) = &tables[0] else {
        panic!("a map");
    };
    assert_eq!(
        table["entityName"],
        moqui_lib::object::Value::string("moqui.basic.Enumeration")
    );
    assert_eq!(
        table["missingColumns"],
        moqui_lib::object::Value::List(vec![moqui_lib::object::Value::string("enum_code")])
    );
}
