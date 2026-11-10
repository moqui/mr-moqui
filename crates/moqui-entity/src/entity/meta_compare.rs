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

//! The schema comparison (plans/archive/Tools-Database.md 5.1): each entity's table against the database's catalog, as
//! the table check would make it. Missing tables; missing, extra and differently typed columns; missing, extra and
//! invalid indexes; missing and extra foreign keys; and, on Postgres, foreign keys whose columns no index leads with.
//! The catalog is read in three queries per database (`moqui_db::sql::catalog`), not per table, and names are compared
//! as the DDL builders make them. It reads only: what to do about a difference is the caller's.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use moqui_db::dialect::Dialect;
use moqui_db::model::{EntityDefinition, EntityModel, Relationship};
use moqui_db::sql::catalog::{same_sql_type, schema_catalog};
use moqui_db::sql::{
    create_fk_index, create_foreign_key, create_index, field_sql_type, foreign_key_index_name, foreign_key_name,
    fts_column_name, fts_index_name, fts_index_sql, is_mysql, is_sqlite, physical_column_name, physical_name,
};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::exec::query_conn;
use super::facade::EntityFacade;
use super::info::cell_text;
use crate::driver::{DbConnection, DbRow};
use crate::transaction::{TransactionAttribute, TransactionManager};

/// What [`super::EntityDbMeta::compare`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaDiff {
    /// The tables compared.
    pub checked: usize,
    /// The tables with a difference, in the order compared.
    pub tables: Vec<TableDiff>,
    /// What could not be compared (a group whose catalog would not read, a field type not in the dictionary), each
    /// logged too; the rest was compared.
    pub errors: Vec<String>,
}

/// The differences of one entity's table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableDiff {
    pub entity_name: String,
    pub group: String,
    /// As the database has it.
    pub table_name: String,
    /// No such table; nothing else is compared.
    pub missing_table: bool,
    pub missing_columns: Vec<String>,
    /// Columns no field names.
    pub extra_columns: Vec<String>,
    pub column_types: Vec<ColumnTypeDiff>,
    pub missing_indexes: Vec<IndexDiff>,
    /// Indexes the table check would not make, and not behind a primary key or unique constraint, or (MySQL) the
    /// index InnoDB makes for a foreign key.
    pub extra_indexes: Vec<CatalogIndex>,
    /// Postgres `NOT indisvalid`: a failed `CREATE INDEX CONCURRENTLY` leaves one.
    pub invalid_indexes: Vec<CatalogIndex>,
    pub missing_foreign_keys: Vec<ForeignKeyDiff>,
    /// Foreign keys no `type="one"` relationship asks for.
    pub extra_foreign_keys: Vec<CatalogForeignKey>,
    /// Postgres foreign keys whose columns no index leads with, when the dialect does not make foreign key indexes
    /// (when it does, a missing one is a missing index).
    pub unindexed_foreign_keys: Vec<CatalogForeignKey>,
}

impl TableDiff {
    /// No difference.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.missing_table
            && self.missing_columns.is_empty()
            && self.extra_columns.is_empty()
            && self.column_types.is_empty()
            && self.missing_indexes.is_empty()
            && self.extra_indexes.is_empty()
            && self.invalid_indexes.is_empty()
            && self.missing_foreign_keys.is_empty()
            && self.extra_foreign_keys.is_empty()
            && self.unindexed_foreign_keys.is_empty()
    }
}

/// A column whose type is not the one its field gives ([`same_sql_type`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnTypeDiff {
    pub column: String,
    /// The field's type, as the DDL builder writes it.
    pub expected: String,
    /// The column's, as the catalog gives it.
    pub actual: String,
}

/// Where an expected index comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexKind {
    /// An `index` element of the entity.
    Declared,
    /// The index on a foreign key's columns (`use-foreign-key-indexes`).
    ForeignKey,
    /// A `text-fts` field's full-text index.
    FullText,
}

impl IndexKind {
    fn name(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::ForeignKey => "foreign-key",
            Self::FullText => "full-text",
        }
    }
}

/// An index the table check would make that the table does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDiff {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub kind: IndexKind,
    /// The statements that make it, from the DDL builders; empty when they could not be built.
    pub ddl: Vec<String>,
}

/// A foreign key a `type="one"` relationship asks for that the table does not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignKeyDiff {
    pub name: String,
    pub related_entity: String,
    pub related_table: String,
    pub columns: Vec<String>,
    /// The statement that adds it, `None` when it could not be built.
    pub ddl: Option<String>,
}

/// An index as the catalog has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogIndex {
    pub name: String,
    /// In index order; an expression is left out.
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    pub valid: bool,
    /// The index behind a primary key, unique or exclusion constraint.
    pub constraint: bool,
}

/// A foreign key as the catalog has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogForeignKey {
    pub name: String,
    pub referenced_table: String,
    pub columns: Vec<String>,
    /// Postgres `convalidated`: false for one added `NOT VALID` and not validated yet.
    pub valid: bool,
}

/// One table of the catalog.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CatalogTable {
    pub(crate) name: String,
    /// Name and type, in order.
    pub(crate) columns: Vec<(String, String)>,
    pub(crate) indexes: Vec<CatalogIndex>,
    pub(crate) foreign_keys: Vec<CatalogForeignKey>,
}

/// The tables of one database's current schema, keyed by the lower-case table name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub(crate) tables: HashMap<String, CatalogTable>,
}

impl Catalog {
    fn table(&self, name: &str) -> Option<&CatalogTable> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    fn table_mut(&mut self, name: &str) -> &mut CatalogTable {
        self.tables
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| CatalogTable {
                name: name.to_string(),
                ..CatalogTable::default()
            })
    }
}

/// Read the catalog of the database `conn` is on, in its three queries.
pub(crate) fn read_catalog(conn: &mut dyn DbConnection, dialect: &Dialect) -> Result<Catalog, Error> {
    let Some(sql) = schema_catalog(dialect) else {
        return Err(Error::new(format!(
            "the schema comparison does not read the catalog of database {}",
            dialect.name
        )));
    };
    let mut catalog = Catalog::default();
    for row in query_conn(conn, sql.columns, &[])? {
        let table = catalog.table_mut(&text(&row, "table_name"));
        let column = text(&row, "column_name");
        if !column.is_empty() {
            table.columns.push((column, text(&row, "column_type")));
        }
    }
    for row in query_conn(conn, sql.indexes, &[])? {
        let index = CatalogIndex {
            name: text(&row, "index_name"),
            columns: list(&row, "column_names"),
            unique: flag(&row, "is_unique"),
            primary: flag(&row, "is_primary"),
            valid: flag(&row, "is_valid"),
            constraint: flag(&row, "is_constraint"),
        };
        catalog.table_mut(&text(&row, "table_name")).indexes.push(index);
    }
    for row in query_conn(conn, sql.foreign_keys, &[])? {
        let key = CatalogForeignKey {
            name: text(&row, "constraint_name"),
            referenced_table: text(&row, "referenced_table"),
            columns: list(&row, "column_names"),
            valid: flag(&row, "is_valid"),
        };
        catalog.table_mut(&text(&row, "table_name")).foreign_keys.push(key);
    }
    Ok(catalog)
}

fn text(row: &DbRow, name: &str) -> String {
    row.get(name).map(cell_text).unwrap_or_default()
}

fn list(row: &DbRow, name: &str) -> Vec<String> {
    text(row, name)
        .split(',')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// A catalog flag: a boolean on Postgres, a number on MySQL.
fn flag(row: &DbRow, name: &str) -> bool {
    match row.get(name) {
        Some(Value::Bool(value)) => *value,
        Some(Value::I32(value)) => *value != 0,
        Some(Value::I64(value)) => *value != 0,
        Some(other) => matches!(cell_text(other).as_str(), "t" | "true" | "1"),
        None => false,
    }
}

/// What the comparison of one table needs besides the model and the catalog.
pub(crate) struct Rules<'a> {
    /// Foreign keys are compared: the facade's and the load's `disable-fk-create` and `entity_disable_fk_create` are
    /// off, as for the table check.
    pub(crate) foreign_keys: bool,
    /// Whether two entity groups share a database: no foreign key crosses databases.
    pub(crate) same_database: &'a dyn Fn(Option<&str>, Option<&str>) -> bool,
}

/// An index the table check would make.
struct ExpectedIndex {
    name: String,
    columns: Vec<String>,
    unique: bool,
    kind: IndexKind,
    ddl: Vec<String>,
}

/// The physical, lower-case column of a field name (or of a column name that is not a field's).
fn column_of(entity: &EntityDefinition, dialect: &Dialect, field: &str) -> String {
    let column = entity.field(field).map_or(field, |field| field.column_name.as_str());
    physical_column_name(dialect, column).to_ascii_lowercase()
}

fn key_columns(entity: &EntityDefinition, dialect: &Dialect, relationship: &Relationship) -> Vec<String> {
    relationship
        .key_pairs
        .iter()
        .map(|(source, _)| column_of(entity, dialect, source))
        .collect()
}

fn lower_set(names: &[String]) -> HashSet<String> {
    names.iter().map(|name| name.to_ascii_lowercase()).collect()
}

/// A relationship the table check makes a foreign key and its index for.
fn is_key_relationship(relationship: &Relationship) -> bool {
    !relationship.is_auto_reverse && relationship.relationship_type == "one" && !relationship.key_pairs.is_empty()
}

/// Compare one entity's table with the catalog. `errors` gets what could not be compared.
pub(crate) fn diff_table(
    entity: &EntityDefinition,
    model: &EntityModel,
    dialect: &Dialect,
    catalog: &Catalog,
    rules: &Rules<'_>,
    errors: &mut Vec<String>,
) -> TableDiff {
    let mut diff = TableDiff {
        entity_name: entity.full_name.clone(),
        table_name: physical_name(dialect, &entity.table_name),
        ..TableDiff::default()
    };
    let Some(table) = catalog.table(&entity.table_name) else {
        diff.missing_table = true;
        return diff;
    };
    diff.table_name.clone_from(&table.name);
    diff_columns(entity, model, dialect, table, &mut diff, errors);
    diff_indexes(entity, dialect, table, &mut diff);
    if rules.foreign_keys && !is_sqlite(dialect) && dialect.use_foreign_keys {
        diff_foreign_keys(entity, model, dialect, catalog, table, rules, &mut diff, errors);
    }
    // When the dialect makes an index per foreign key, an unindexed key is already a missing index.
    if dialect.name.to_ascii_lowercase().starts_with("postgres") && !dialect.use_foreign_key_indexes {
        for key in &table.foreign_keys {
            let wanted = lower_set(&key.columns);
            let indexed = table.indexes.iter().any(|index| {
                index.columns.len() >= wanted.len() && lower_set(&index.columns[..wanted.len()]) == wanted
            });
            if !indexed {
                diff.unindexed_foreign_keys.push(key.clone());
            }
        }
    }
    diff
}

fn diff_columns(
    entity: &EntityDefinition,
    model: &EntityModel,
    dialect: &Dialect,
    table: &CatalogTable,
    diff: &mut TableDiff,
    errors: &mut Vec<String>,
) {
    let have: HashMap<String, &(String, String)> = table
        .columns
        .iter()
        .map(|column| (column.0.to_ascii_lowercase(), column))
        .collect();
    let mut wanted = HashSet::new();
    let tsvector = dialect.fts_ddl && dialect.fts_style.as_deref() == Some("postgres-tsvector");
    for field in &entity.fields {
        let column = physical_column_name(dialect, &field.column_name);
        wanted.insert(column.to_ascii_lowercase());
        if tsvector && field.field_type == "text-fts" {
            wanted.insert(fts_column_name(field, dialect).to_ascii_lowercase());
        }
        let Some((_, actual)) = have.get(&column.to_ascii_lowercase()) else {
            diff.missing_columns.push(column);
            continue;
        };
        match field_sql_type(model, dialect, field) {
            Ok(expected) if !same_sql_type(&expected, actual) => diff.column_types.push(ColumnTypeDiff {
                column,
                expected,
                actual: actual.clone(),
            }),
            Ok(_) => {}
            Err(err) => errors.push(format!("{}: {err}", entity.full_name)),
        }
    }
    diff.extra_columns = table
        .columns
        .iter()
        .filter(|(name, _)| !wanted.contains(&name.to_ascii_lowercase()))
        .map(|(name, _)| name.clone())
        .collect();
}

/// The indexes the table check makes for the entity: its `index` elements and its foreign keys' (both only under
/// `use-indexes`), and its full-text ones.
fn expected_indexes(entity: &EntityDefinition, dialect: &Dialect) -> Vec<ExpectedIndex> {
    let mut expected = Vec::new();
    if dialect.use_indexes {
        for index in entity.indexes.iter().filter(|index| !index.fields.is_empty()) {
            expected.push(ExpectedIndex {
                name: index.name.clone(),
                columns: index
                    .fields
                    .iter()
                    .map(|field| column_of(entity, dialect, field))
                    .collect(),
                unique: index.unique && dialect.use_indexes_unique,
                kind: IndexKind::Declared,
                ddl: create_index(entity, index, dialect).into_iter().collect(),
            });
        }
        if dialect.use_foreign_key_indexes {
            for relationship in entity.relationships.iter().filter(|r| is_key_relationship(r)) {
                expected.push(ExpectedIndex {
                    name: foreign_key_index_name(entity, relationship, dialect),
                    columns: key_columns(entity, dialect, relationship),
                    unique: false,
                    kind: IndexKind::ForeignKey,
                    ddl: create_fk_index(entity, relationship, dialect).into_iter().collect(),
                });
            }
        }
    }
    for field in entity.fields.iter().filter(|field| field.field_type == "text-fts") {
        let Ok(ddl) = fts_index_sql(entity, field, dialect) else {
            continue;
        };
        if ddl.is_empty() {
            continue;
        }
        let column = if is_mysql(dialect) {
            physical_column_name(dialect, &field.column_name)
        } else {
            fts_column_name(field, dialect)
        };
        expected.push(ExpectedIndex {
            name: fts_index_name(entity, field, dialect),
            columns: vec![column.to_ascii_lowercase()],
            unique: false,
            kind: IndexKind::FullText,
            ddl,
        });
    }
    expected
}

fn diff_indexes(entity: &EntityDefinition, dialect: &Dialect, table: &CatalogTable, diff: &mut TableDiff) {
    let expected = expected_indexes(entity, dialect);
    let have: HashSet<String> = table
        .indexes
        .iter()
        .map(|index| index.name.to_ascii_lowercase())
        .collect();
    for index in &expected {
        if !have.contains(&index.name.to_ascii_lowercase()) {
            diff.missing_indexes.push(IndexDiff {
                name: index.name.clone(),
                columns: index.columns.clone(),
                unique: index.unique,
                kind: index.kind,
                ddl: index.ddl.clone(),
            });
        }
    }
    let wanted: HashSet<String> = expected.iter().map(|index| index.name.to_ascii_lowercase()).collect();
    let mysql = is_mysql(dialect);
    for index in &table.indexes {
        if !index.valid {
            diff.invalid_indexes.push(index.clone());
        }
        if index.primary || index.constraint || wanted.contains(&index.name.to_ascii_lowercase()) {
            continue;
        }
        // InnoDB makes an index for a foreign key that has none, named after the key; it cannot be dropped.
        if mysql
            && table.foreign_keys.iter().any(|key| {
                key.name.eq_ignore_ascii_case(&index.name) || lower_set(&key.columns) == lower_set(&index.columns)
            })
        {
            continue;
        }
        diff.extra_indexes.push(index.clone());
    }
}

/// The foreign keys of the entity's `type="one"` relationships, by the table check's rule (`ensure_fks` in `meta.rs`): to
/// an entity in the same database whose table is there, found by name or, as Java's `foreignKeyExists`, by its
/// columns to the related table; on MySQL by the columns only, since the server may have named it (Parity-Diffs P3-11).
#[allow(clippy::too_many_arguments)]
fn diff_foreign_keys(
    entity: &EntityDefinition,
    model: &EntityModel,
    dialect: &Dialect,
    catalog: &Catalog,
    table: &CatalogTable,
    rules: &Rules<'_>,
    diff: &mut TableDiff,
    errors: &mut Vec<String>,
) {
    let mysql = is_mysql(dialect);
    let mut matched = vec![false; table.foreign_keys.len()];
    for relationship in entity.relationships.iter().filter(|r| is_key_relationship(r)) {
        let Ok(related) = model.entity(&relationship.related) else {
            errors.push(format!(
                "{}: relationship {} is to unknown entity {}",
                entity.full_name, relationship.name, relationship.related
            ));
            continue;
        };
        if related.is_view
            || !(rules.same_database)(entity.group.as_deref(), related.group.as_deref())
            || catalog.table(&related.table_name).is_none()
        {
            continue;
        }
        let name = foreign_key_name(entity, relationship, dialect);
        let columns = key_columns(entity, dialect, relationship);
        let wanted = lower_set(&columns);
        let found = table.foreign_keys.iter().position(|key| {
            (!mysql && key.name.eq_ignore_ascii_case(&name))
                || (key.referenced_table.eq_ignore_ascii_case(&related.table_name) && lower_set(&key.columns) == wanted)
        });
        match found {
            Some(at) => matched[at] = true,
            None => diff.missing_foreign_keys.push(ForeignKeyDiff {
                name,
                related_entity: related.full_name.clone(),
                related_table: physical_name(dialect, &related.table_name),
                columns,
                ddl: create_foreign_key(entity, relationship, related, dialect).ok(),
            }),
        }
    }
    diff.extra_foreign_keys = table
        .foreign_keys
        .iter()
        .zip(&matched)
        .filter(|(_, matched)| !**matched)
        .map(|(key, _)| key.clone())
        .collect();
}

impl EntityFacade {
    /// [`super::EntityDbMeta::compare`].
    pub fn compare_schema(&self, names: &[&str]) -> Result<SchemaDiff, Error> {
        self.meta().compare(names)
    }
}

/// [`super::EntityDbMeta::compare`]: the entities of `names` (a view's members; every entity when empty) grouped by
/// database, each database's catalog read once.
pub(crate) fn compare(facade: &EntityFacade, names: &[&str]) -> Result<SchemaDiff, Error> {
    let concrete = if names.is_empty() {
        facade
            .inner
            .model
            .model()
            .entities()
            .iter()
            .filter(|entity| !entity.is_view)
            .map(|entity| entity.full_name.clone())
            .collect()
    } else {
        super::meta::expand(facade, names)?
    };
    let skip_fk =
        super::fk::fk_create_disabled(facade) || moqui_lib::env::get_prop_or_env("entity_disable_fk_create") == "true";
    let same_database = |group: Option<&str>, other: Option<&str>| facade.same_database(group, other);
    let rules = Rules {
        foreign_keys: !skip_fk,
        same_database: &same_database,
    };
    let default_group = facade.inner.default_group.clone();
    let mut diff = SchemaDiff::default();
    // One catalog per database: the groups that share a transaction stack share a database.
    let mut catalogs: Vec<(TransactionManager, Option<Catalog>)> = Vec::new();
    for name in concrete {
        let (group, tx, dialect) = facade.with_group(&name, |entity, dialect, _, tx| {
            Ok((
                entity.group.clone().unwrap_or_else(|| default_group.clone()),
                tx.clone(),
                dialect.clone(),
            ))
        })?;
        let at = match catalogs.iter().position(|(known, _)| known.shares_stack(&tx)) {
            Some(at) => at,
            None => {
                // On a connection of its own, the caller's transaction suspended: a query that fails must not mark the
                // caller's transaction rollback-only.
                let read = tx.run(TransactionAttribute::IgnoreSuspend, None, || {
                    tx.with_connection(|conn| read_catalog(conn, &dialect))
                });
                let catalog = match read {
                    Ok(catalog) => Some(catalog),
                    Err(err) => {
                        tracing::error!(error = %err, "Could not read the catalog of group {group} for the schema comparison");
                        diff.errors.push(format!("group {group}: {err}"));
                        None
                    }
                };
                catalogs.push((tx, catalog));
                catalogs.len() - 1
            }
        };
        let Some(catalog) = &catalogs[at].1 else {
            continue;
        };
        let table = facade.with_entity(&name, |entity, dialect, model| {
            Ok(diff_table(entity, model, dialect, catalog, &rules, &mut diff.errors))
        });
        match table {
            Ok(mut table) => {
                diff.checked += 1;
                if !table.is_empty() {
                    table.group = group;
                    diff.tables.push(table);
                }
            }
            Err(err) => diff.errors.push(format!("{name}: {err}")),
        }
    }
    for error in &diff.errors {
        tracing::warn!("Schema comparison: {error}");
    }
    Ok(diff)
}

impl SchemaDiff {
    /// As a map for a service's out-parameters: `checked`, `tables` and `errors`, each table's lists by their camel-case
    /// names (`missingColumns`, `columnTypes`, `missingIndexes`, `missingForeignKeys`, …).
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::default();
        map.insert(
            "checked".to_string(),
            Value::I64(i64::try_from(self.checked).unwrap_or(i64::MAX)),
        );
        map.insert(
            "tables".to_string(),
            Value::List(self.tables.iter().map(TableDiff::to_value).collect()),
        );
        map.insert("errors".to_string(), strings(&self.errors));
        Value::Map(map)
    }
}

impl TableDiff {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::default();
        map.insert("entityName".to_string(), Value::string(&self.entity_name));
        map.insert("groupName".to_string(), Value::string(&self.group));
        map.insert("tableName".to_string(), Value::string(&self.table_name));
        map.insert("missingTable".to_string(), Value::Bool(self.missing_table));
        map.insert("missingColumns".to_string(), strings(&self.missing_columns));
        map.insert("extraColumns".to_string(), strings(&self.extra_columns));
        let types = self.column_types.iter().map(|column| {
            entries([
                ("column", Value::string(&column.column)),
                ("expected", Value::string(&column.expected)),
                ("actual", Value::string(&column.actual)),
            ])
        });
        map.insert("columnTypes".to_string(), Value::List(types.collect()));
        let missing = self.missing_indexes.iter().map(|index| {
            entries([
                ("name", Value::string(&index.name)),
                ("columns", strings(&index.columns)),
                ("unique", Value::Bool(index.unique)),
                ("kind", Value::string(index.kind.name())),
                ("ddl", strings(&index.ddl)),
            ])
        });
        map.insert("missingIndexes".to_string(), Value::List(missing.collect()));
        map.insert("extraIndexes".to_string(), indexes(&self.extra_indexes));
        map.insert("invalidIndexes".to_string(), indexes(&self.invalid_indexes));
        let missing = self.missing_foreign_keys.iter().map(|key| {
            entries([
                ("name", Value::string(&key.name)),
                ("relatedEntity", Value::string(&key.related_entity)),
                ("relatedTable", Value::string(&key.related_table)),
                ("columns", strings(&key.columns)),
                ("ddl", key.ddl.as_deref().map_or(Value::Null, Value::string)),
            ])
        });
        map.insert("missingForeignKeys".to_string(), Value::List(missing.collect()));
        map.insert("extraForeignKeys".to_string(), foreign_keys(&self.extra_foreign_keys));
        map.insert(
            "unindexedForeignKeys".to_string(),
            foreign_keys(&self.unindexed_foreign_keys),
        );
        Value::Map(map)
    }
}

fn entries<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
}

fn strings(items: &[String]) -> Value {
    Value::List(items.iter().map(Value::string).collect())
}

fn indexes(items: &[CatalogIndex]) -> Value {
    Value::List(
        items
            .iter()
            .map(|index| {
                entries([
                    ("name", Value::string(&index.name)),
                    ("columns", strings(&index.columns)),
                    ("unique", Value::Bool(index.unique)),
                    ("primary", Value::Bool(index.primary)),
                    ("valid", Value::Bool(index.valid)),
                ])
            })
            .collect(),
    )
}

fn foreign_keys(items: &[CatalogForeignKey]) -> Value {
    Value::List(
        items
            .iter()
            .map(|key| {
                entries([
                    ("name", Value::string(&key.name)),
                    ("referencedTable", Value::string(&key.referenced_table)),
                    ("columns", strings(&key.columns)),
                    ("valid", Value::Bool(key.valid)),
                ])
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "meta_compare_tests.rs"]
mod tests;
