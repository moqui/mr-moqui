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

//! The data grid's server side (plans/Tools-Database.md §9): no state per grid; each call takes the source spec and
//! runs the source again.
//!
//! - **Sources** ([`Source`]): an entity find (a view entity, `DbViewEntity` or `DataDocument.<id>` read only) with
//!   conditions by the `search-form-inputs` rules, a SQL query wrapped as `SELECT * FROM (<sql>) q`, run read only
//!   and audited through [`crate::sql::grid_query`], or a CSV or spreadsheet file, read only, through
//!   [`crate::files`] from the places `etl::access` allows.
//! - **Calls:** [`columns`], [`rows`] (Tabulator's progressive load, never counting), [`count`], [`export`] (CSV
//!   streamed, XLSX capped at [`MAX_XLSX_ROWS`]), [`lookup`] (a related entity's options) and [`save`] (one
//!   transaction of entity-auto calls, with the stamp checked).
//! - **Secrets:** `UserAccount`'s password fields ([`sensitive::USER_ACCOUNT_SECRET_FIELDS`]) are left out by field
//!   name on every entity, so a view that aliases them (`UserAccountAndGroup`) shows none; they and encrypted fields
//!   take no sort, filter or condition, so a count is no oracle of their values.
//! - **Editing a SQL result** (§9.5): every column of one table, in the group's schema, with its entity's whole primary
//!   key and its `lastUpdatedStamp`, decided from the result's column origins, and only for a statement that names the
//!   table once with no join, set operation, `DISTINCT`, `GROUP BY` or subquery in its `FROM`.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert_text, is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::exec::{blank_sql, SqlSyntax};
use moqui_core::entity::sensitive;
use moqui_core::entity::{Condition, EntityFacade, EntityFind, EntityValue};
use moqui_core::service::call::{sync_checked, CallOptions};
use moqui_core::transaction::TransactionAttribute;
use moqui_db::model::{EntityDefinition, FieldDefinition};

use crate::audit::{self, Actor, AuditEntry};
use crate::catalog;
use crate::sql::{self, Refusal};

/// The most rows one `rows` page has.
pub const MAX_PAGE_SIZE: usize = 1000;
/// The most rows an XLSX export writes: the workbook is built in memory (`XlsxBook` fits its columns to the text,
/// which rust_xlsxwriter's constant-memory mode cannot do), so a larger export is cut and says so. CSV is streamed.
pub const MAX_XLSX_ROWS: usize = 100_000;
/// The most rows an export of a SQL source writes: `run_sql_with` reads the result into memory.
pub const MAX_SQL_EXPORT_ROWS: usize = 200_000;
/// Options per `lookup`.
pub const LOOKUP_SIZE: i64 = 20;
const MASK: &str = "********";
const STAMP: &str = "lastUpdatedStamp";
/// The rows a SQL source's columns are read from: the kind of each is that of its first value that is not null.
const COLUMN_PROBE_ROWS: usize = 50;

fn refusal(message: impl Into<String>) -> Refusal {
    Refusal(message.into())
}

/// The short message of a database error, the whole of it to the log: the text after the last `ERROR:` (Postgres) or
/// the driver's last clause, with MySQL's duplicate column named.
fn db_refusal(err: &Refusal) -> Refusal {
    let text = err.0.as_str();
    tracing::info!(error = text, "A data grid query failed");
    if let Some(at) = text.find("Duplicate column name '") {
        let rest = &text[at + "Duplicate column name '".len()..];
        let name = rest.split('\'').next().unwrap_or(rest);
        return refusal(format!("The query has two columns named {name}; give one an alias"));
    }
    let short = text
        .rsplit_once("ERROR: ")
        .map(|(_, tail)| tail)
        .or_else(|| text.rsplit_once("): ").map(|(_, tail)| tail))
        .unwrap_or(text)
        .lines()
        .next()
        .unwrap_or_default()
        .trim();
    refusal(if short.is_empty() { "The query failed" } else { short }.to_string())
}

/// The short message of an entity find's error, its detail to the log.
fn find_refusal(err: &Error) -> Refusal {
    tracing::info!(error = %err, "A data grid find failed");
    refusal(format!(
        "The rows could not be read: {}",
        err.message().lines().next().unwrap_or_default()
    ))
}

/// The source of a grid, from its spec's `source`.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// An entity find; `conditions` are search-form inputs (`field`, `field_op`, `field_not`, `field_ic`, `_from`,
    /// `_thru`, `_period`).
    Entity {
        entity_name: String,
        conditions: BTreeMap<String, Value>,
    },
    /// A query on an entity group's database. `key_columns` and `stamp_column` come back from [`columns`] for an
    /// editable result: the order of its pages ends with the key, and each row gets `_stamp`.
    Sql {
        group: String,
        sql: String,
        key_columns: Vec<String>,
        stamp_column: Option<String>,
    },
    /// A CSV or spreadsheet file, read only: `{type: file, location, sheet, headerRow, format, delimiter}`. Sorted and
    /// filtered in memory, at most [`crate::files::MAX_QUERY_ROWS`] rows.
    File { spec: crate::files::FileSpec },
}

fn list_of(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|name| !name.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

impl Source {
    /// Read the spec's `source` map: `{type: entity, entityName, conditions}`, `{type: sql, db, sql, keyColumns,
    /// stampColumn}` or `{type: file, location, sheet, headerRow, format, delimiter}`.
    ///
    /// # Errors
    ///
    /// Returns the refusal of a spec that names no source, an unknown group, an external database, or a file in a
    /// place the user may not read (`Not available`).
    pub fn parse(source: Option<&Value>, facade: &EntityFacade) -> Result<Self, Refusal> {
        let Some(Value::Map(map)) = source else {
            return Err(refusal(
                "source is required: {type: entity, entityName} or {type: sql, db, sql}",
            ));
        };
        let text = |key: &str| {
            map.get(key)
                .filter(|value| !is_empty(value))
                .map(to_plain_string)
                .unwrap_or_default()
        };
        match text("type").as_str() {
            "entity" | "" if !text("entityName").is_empty() => Ok(Self::Entity {
                entity_name: text("entityName"),
                conditions: match map.get("conditions") {
                    Some(Value::Map(conditions)) => conditions.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                    _ => BTreeMap::new(),
                },
            }),
            "sql" => match crate::DbRef::parse(&text("db"), facade.default_group()) {
                Ok(crate::DbRef::Group(group)) if facade.group_names().contains(&group) => Ok(Self::Sql {
                    group,
                    sql: text("sql").trim().trim_end_matches(';').to_string(),
                    key_columns: list_of(map.get("keyColumns")),
                    stamp_column: Some(text("stampColumn")).filter(|name| !name.is_empty()),
                }),
                Ok(crate::DbRef::Group(group)) => Err(refusal(format!("no entity group named {group}"))),
                Ok(crate::DbRef::External(_)) => Err(refusal(crate::NOT_AVAILABLE)),
                Err(message) => Err(refusal(message)),
            },
            "file" => {
                let spec = crate::files::FileSpec::from_map(map);
                crate::etl::access::readable(&spec.location).map_err(|err| refusal(err.to_string()))?;
                Ok(Self::File { spec })
            }
            other => Err(refusal(format!("source type {other:?} is not entity, sql or file"))),
        }
    }

    #[must_use]
    pub fn is_sql(&self) -> bool {
        matches!(self, Self::Sql { .. })
    }
}

/// A column of the grid.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Column {
    /// The data key of each row: the entity field, or the SQL result's column label.
    pub name: String,
    pub title: String,
    /// `text`, `long-text`, `number`, `date`, `time`, `date-time`, `indicator`, `boolean`, `binary`.
    pub kind: &'static str,
    pub editable: bool,
    /// Tabulator's editor: `input`, `textarea`, `number`, `date`, `time`, `datetime`, `tickCross`, `list`; none when
    /// not editable.
    pub editor: Option<&'static str>,
    pub pk: bool,
    /// Shown masked; never edited in the grid.
    pub encrypted: bool,
    pub not_null: bool,
    /// Set on create only.
    pub create_only: bool,
    /// The most characters a text field takes, from its type.
    pub max_length: Option<i64>,
    /// A related entity this field is the key of: its name and field (the `lookup` options).
    pub related_entity: Option<String>,
    pub related_field: Option<String>,
    /// For an editable SQL result (§9.5): the entity field this column is.
    pub field: Option<String>,
}

impl Column {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let mut map = IndexMap::new();
        map.insert("name".to_string(), Value::string(&self.name));
        map.insert("title".to_string(), Value::string(&self.title));
        map.insert("type".to_string(), Value::string(self.kind));
        map.insert("editable".to_string(), Value::Bool(self.editable));
        map.insert("editor".to_string(), self.editor.map_or(Value::Null, Value::string));
        map.insert("pk".to_string(), Value::Bool(self.pk));
        map.insert("encrypted".to_string(), Value::Bool(self.encrypted));
        map.insert("notNull".to_string(), Value::Bool(self.not_null));
        map.insert("createOnly".to_string(), Value::Bool(self.create_only));
        map.insert("maxLength".to_string(), self.max_length.map_or(Value::Null, Value::I64));
        map.insert("relatedEntity".to_string(), text(&self.related_entity));
        map.insert("relatedField".to_string(), text(&self.related_field));
        map.insert("field".to_string(), text(&self.field));
        Value::Map(map)
    }
}

/// What [`columns`] answers.
#[derive(Clone, Debug, Default)]
pub struct ColumnsOutcome {
    pub columns: Vec<Column>,
    /// The entity the grid saves to: the source's, or an editable SQL result's (§9.5). None when read only.
    pub entity_name: Option<String>,
    /// Why the grid is read only, when it is.
    pub read_only_reason: Option<String>,
    /// The entity has `lastUpdatedStamp`: each row has `_stamp`, which a save sends back.
    pub has_stamp: bool,
    /// An editable SQL result's key columns (labels) and stamp column, for the source spec's `keyColumns` and
    /// `stampColumn`.
    pub key_columns: Vec<String>,
    pub stamp_column: Option<String>,
}

/// The kind and editor of a field type.
fn kind_of(field_type: &str) -> (&'static str, Option<&'static str>) {
    match field_type {
        "text-long" | "text-very-long" => ("long-text", Some("textarea")),
        "text-indicator" => ("indicator", Some("tickCross")),
        "date" => ("date", Some("date")),
        "time" => ("time", Some("time")),
        "date-time" => ("date-time", Some("datetime")),
        "binary-very-long" | "binary" => ("binary", None),
        kind if kind.starts_with("number") || kind.starts_with("currency") => ("number", Some("number")),
        _ => ("text", Some("input")),
    }
}

fn max_length(field_type: &str) -> Option<i64> {
    match field_type {
        "id" => Some(40),
        "id-long" | "text-medium" => Some(255),
        "text-short" => Some(63),
        "text-indicator" => Some(1),
        "text-long" => Some(4095),
        _ => None,
    }
}

/// Why `entity` cannot be read or saved in the grid at all: the identity and secret entities the generic tools refuse.
fn refuse_restricted(entity_name: &str) -> Result<(), Refusal> {
    if sensitive::is_restricted_generic_entity(entity_name) {
        Err(refusal(format!("{entity_name} is not available in the data grid")))
    } else {
        Ok(())
    }
}

/// Why rows of `entity` are not saved, when they are not: a view, a data document, no primary key.
fn read_only_reason(entity: &EntityDefinition) -> Option<String> {
    if entity.is_view {
        Some(format!(
            "{} is a view: open a member entity's record to change it",
            entity.full_name
        ))
    } else if entity.pk_field_names().is_empty() {
        Some(format!("{} has no primary key", entity.full_name))
    } else {
        None
    }
}

/// `UserAccount`'s password fields, by name on any entity: a view that aliases them (`UserAccountAndGroup`,
/// `PersonAndUserAccount`) names them so too, as generic entity REST strips them by name.
fn is_secret(field: &str) -> bool {
    sensitive::USER_ACCOUNT_SECRET_FIELDS.contains(&field)
}

/// A field the grid never sorts, filters or puts a condition on: a secret or an encrypted one.
fn hidden_value(entity: &EntityDefinition, field: &str) -> bool {
    is_secret(field) || entity.field(field).is_some_and(|field| field.encrypt)
}

/// The related entity a field is the single key of: a `one` relationship whose one key pair is this field.
fn related_of(entity: &EntityDefinition, field: &str) -> Option<(String, String)> {
    entity
        .relationships
        .iter()
        .filter(|relationship| relationship.relationship_type.starts_with("one"))
        .find(|relationship| relationship.key_pairs.len() == 1 && relationship.key_pairs[0].0 == field)
        .map(|relationship| {
            (
                relationship.related_entity_name.clone(),
                relationship.key_pairs[0].1.clone(),
            )
        })
}

fn field_column(entity: &EntityDefinition, field: &FieldDefinition, editable_entity: bool) -> Column {
    let (kind, editor) = kind_of(&field.field_type);
    let editable = editable_entity && !field.encrypt && field.name != STAMP && editor.is_some() && !field.is_pk;
    let related = related_of(entity, &field.name);
    Column {
        name: field.name.clone(),
        title: moqui_lib::string::camel_case_to_pretty(&field.name),
        kind,
        editable,
        editor: if !editable {
            None
        } else if related.is_some() {
            Some("list")
        } else {
            editor
        },
        pk: field.is_pk,
        encrypted: field.encrypt,
        not_null: field.not_null || field.is_pk,
        create_only: field.create_only,
        max_length: max_length(&field.field_type),
        related_entity: related.as_ref().map(|(name, _)| name.clone()),
        related_field: related.map(|(_, field)| field),
        field: None,
    }
}

/// The definition of `entity_name` (a view, `DbViewEntity` or `DataDocument.<id>` included). A view's aliases are
/// made its fields here, so the grid reads a view as it reads an entity.
fn definition(facade: &EntityFacade, entity_name: &str) -> Result<EntityDefinition, Refusal> {
    let mut entity = facade
        .with_entity(entity_name, |entity, _, _| Ok(entity.clone()))
        .map_err(|_| refusal(format!("No entity named {entity_name}")))?;
    if entity.is_view && entity.fields.is_empty() {
        entity.fields = entity
            .aliases
            .iter()
            .map(|alias| FieldDefinition {
                name: alias.name.clone(),
                column_name: String::new(),
                field_type: alias.field_type.clone().unwrap_or_else(|| "text-medium".to_string()),
                java_type: alias.java_type.clone(),
                is_pk: alias.is_pk,
                encrypt: alias.encrypt,
                enable_audit_log: None,
                enable_localization: alias.enable_localization,
                create_only: false,
                default_value: None,
                not_null: false,
            })
            .collect();
    }
    Ok(entity)
}

/// The columns of the grid: the source's fields or result columns, of `requested` (all when empty), in that order.
///
/// # Errors
///
/// Returns the refusal: a restricted entity, an unknown entity, a SQL source that is not one plain query, or the
/// database's error.
pub fn columns(
    facade: &EntityFacade,
    source: &Source,
    requested: &[String],
    actor: &Actor,
) -> Result<ColumnsOutcome, Refusal> {
    match source {
        Source::Entity { entity_name, .. } => {
            refuse_restricted(entity_name)?;
            let entity = definition(facade, entity_name)?;
            let reason = read_only_reason(&entity);
            let fields: Vec<&FieldDefinition> = if requested.is_empty() {
                entity.fields.iter().collect()
            } else {
                requested.iter().filter_map(|name| entity.field(name)).collect()
            };
            Ok(ColumnsOutcome {
                columns: fields
                    .into_iter()
                    .filter(|field| !is_secret(&field.name))
                    .map(|field| field_column(&entity, field, reason.is_none()))
                    .collect(),
                entity_name: reason.is_none().then(|| entity.full_name.clone()),
                has_stamp: entity.field(STAMP).is_some() && !entity.is_view,
                read_only_reason: reason,
                ..ColumnsOutcome::default()
            })
        }
        Source::Sql { group, sql, .. } => sql_columns(facade, group, sql, requested, actor),
        Source::File { spec } => {
            let (columns, _) = crate::files::file_columns(spec).map_err(|err| file_refusal(&err))?;
            Ok(ColumnsOutcome {
                columns: columns
                    .into_iter()
                    .filter(|column| requested.is_empty() || requested.contains(&column.name))
                    .filter(|column| !is_secret(&column.name))
                    .map(|column| Column {
                        name: column.name,
                        title: column.title,
                        kind: column.kind,
                        ..Column::default()
                    })
                    .collect(),
                read_only_reason: Some("A file source is read only".to_string()),
                ..ColumnsOutcome::default()
            })
        }
    }
}

/// The refusal of a file source that did not read: `Not available` as it is, else the error's first line.
fn file_refusal(err: &Error) -> Refusal {
    tracing::info!(error = %err, "A data grid file source did not read");
    refusal(
        err.message()
            .lines()
            .next()
            .unwrap_or("The file could not be read")
            .to_string(),
    )
}

/// A file source's rows as the grid's: the `columns` asked for (all when empty), no secret.
fn file_row_values(rows: Vec<IndexMap<String, Value>>, columns: &[String]) -> Vec<Value> {
    rows.into_iter()
        .map(|row| {
            Value::Map(
                row.into_iter()
                    .filter(|(name, _)| (columns.is_empty() || columns.contains(name)) && !is_secret(name))
                    .collect(),
            )
        })
        .collect()
}

/// The grid's wrapped query: `SELECT * FROM (<sql>) q`, with the filters and order, and the values of the filters'
/// placeholders.
struct Wrapped {
    sql: String,
    params: Vec<Value>,
}

/// A column label quoted as the database writes a name: any label (spaces included), the quote character doubled.
fn quote(name: &str, mysql: bool) -> Result<String, Refusal> {
    if name.is_empty() || name.contains('\0') {
        return Err(refusal("A column name to sort or filter on is empty"));
    }
    let quote = if mysql { '`' } else { '"' };
    let mut out = String::with_capacity(name.len() + 2);
    out.push(quote);
    for ch in name.chars() {
        if ch == quote {
            out.push(quote);
        }
        out.push(ch);
    }
    out.push(quote);
    Ok(out)
}

/// A sort of Tabulator's: `{field, dir}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sort {
    pub field: String,
    pub descending: bool,
}

/// A filter of Tabulator's: `{field, type, value}`; `type` is `=`, `!=`, `like`, `starts`, `ends`, `<`, `<=`, `>`,
/// `>=`, `in`, `keywords`.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub field: String,
    pub kind: String,
    pub value: Value,
}

/// Read Tabulator's `sort` list.
#[must_use]
pub fn parse_sort(value: Option<&Value>) -> Vec<Sort> {
    let Some(Value::List(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) => Some(Sort {
                field: map.get("field").map(to_plain_string)?,
                descending: map.get("dir").map(to_plain_string).as_deref() == Some("desc"),
            }),
            _ => None,
        })
        .collect()
}

/// Read Tabulator's `filter` list.
#[must_use]
pub fn parse_filter(value: Option<&Value>) -> Vec<Filter> {
    let Some(Value::List(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Map(map) => Some(Filter {
                field: map.get("field").map(to_plain_string)?,
                kind: map.get("type").map_or_else(|| "=".to_string(), to_plain_string),
                value: map.get("value").cloned().unwrap_or(Value::Null),
            }),
            _ => None,
        })
        .collect()
}

/// `%`, `_` and `\` of a LIKE input escaped with `\`, the escape character of both Postgres and MySQL.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn like_pattern(filter: &Filter) -> Option<String> {
    let text = escape_like(&to_plain_string(&filter.value));
    match filter.kind.as_str() {
        "like" | "keywords" => Some(format!("%{text}%")),
        "starts" => Some(format!("{text}%")),
        "ends" => Some(format!("%{text}")),
        _ => None,
    }
}

fn in_values(value: &Value) -> Vec<Value> {
    match value {
        Value::List(items) => items.clone(),
        Value::Null => Vec::new(),
        other => to_plain_string(other)
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(Value::string)
            .collect(),
    }
}

fn wrap(sql: &str, mysql: bool, sort: &[Sort], filter: &[Filter], tail: &str) -> Result<Wrapped, Refusal> {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    for filter in filter {
        let column = quote(&filter.field, mysql)?;
        if let Some(pattern) = like_pattern(filter) {
            // a number or date column is compared as its text
            clauses.push(format!(
                "LOWER(CAST({column} AS {})) LIKE LOWER(?)",
                if mysql { "CHAR" } else { "TEXT" }
            ));
            params.push(Value::String(pattern));
            continue;
        }
        let op = match filter.kind.as_str() {
            "=" => "=",
            "!=" => "<>",
            "<" => "<",
            "<=" => "<=",
            ">" => ">",
            ">=" => ">=",
            "in" => {
                let values = in_values(&filter.value);
                if values.is_empty() {
                    // as the entity source: an empty list matches no row
                    clauses.push("1 = 0".to_string());
                } else {
                    clauses.push(format!("{column} IN ({})", vec!["?"; values.len()].join(", ")));
                    params.extend(values);
                }
                continue;
            }
            other => return Err(refusal(format!("filter type {other:?} is not one the grid takes"))),
        };
        clauses.push(format!("{column} {op} ?"));
        params.push(filter.value.clone());
    }
    let mut out = format!("SELECT * FROM ({sql}) q");
    if !clauses.is_empty() {
        out.push_str(" WHERE ");
        out.push_str(&clauses.join(" AND "));
    }
    if !sort.is_empty() {
        let order: Result<Vec<String>, Refusal> = sort
            .iter()
            .map(|sort| {
                Ok(format!(
                    "{} {}",
                    quote(&sort.field, mysql)?,
                    if sort.descending { "DESC" } else { "ASC" }
                ))
            })
            .collect();
        out.push_str(" ORDER BY ");
        out.push_str(&order?.join(", "));
    }
    out.push_str(tail);
    if !params.is_empty() && blank_sql(sql, syntax(mysql)).contains('?') {
        return Err(refusal("A grid query with a ? of its own cannot be filtered"));
    }
    Ok(Wrapped { sql: out, params })
}

/// The user's sort, then the key columns not in it: a stable order for paging. Without key columns (a read-only SQL
/// result) the order is the user's sort alone, and rows that sort equal may move between pages.
fn with_key_order(sort: &[Sort], keys: &[String]) -> Vec<Sort> {
    let mut out = sort.to_vec();
    for key in keys {
        if !out.iter().any(|sort| &sort.field == key) {
            out.push(Sort {
                field: key.clone(),
                descending: false,
            });
        }
    }
    out
}

fn syntax(mysql: bool) -> SqlSyntax {
    if mysql {
        SqlSyntax::Mysql
    } else {
        SqlSyntax::Postgres
    }
}

fn check_sql_source(facade: &EntityFacade, group: &str, sql: &str) -> Result<bool, Refusal> {
    let mysql = catalog::group_is_mysql(facade, group);
    if !sql::grid_source_query(sql, sql::group_syntax(facade, group)) {
        return Err(refusal(
            "A grid's SQL is one query: SELECT, WITH (that writes nothing), VALUES or TABLE",
        ));
    }
    Ok(mysql)
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::I32(_) | Value::I64(_) | Value::F64(_) | Value::Decimal(_) => "number",
        Value::Timestamp(_) => "date-time",
        Value::Date(_) => "date",
        Value::Time(_) => "time",
        Value::Bool(_) => "boolean",
        Value::Bytes(_) => "binary",
        _ => "text",
    }
}

/// Why a statement is not one the grid saves the result of, though its columns' origins name one table: origins do not
/// tell an alias of a table from another, so a statement that names the table twice, joins, combines results,
/// collapses rows or reads from a subquery is read only.
fn statement_shape_refusal(sql: &str, mysql: bool, table: &str) -> Option<String> {
    let blank = blank_sql(sql, syntax(mysql)).to_ascii_lowercase();
    let words: Vec<&str> = blank
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '$')
        .filter(|word| !word.is_empty())
        .collect();
    for word in ["join", "union", "intersect", "except", "distinct", "group"] {
        if words.contains(&word) {
            return Some(format!(
                "The query has {}: its rows are not rows of one table",
                word.to_uppercase()
            ));
        }
    }
    let table = table.to_ascii_lowercase();
    if words.iter().filter(|word| **word == table).count() != 1 {
        return Some(format!("The query names {table} more than once"));
    }
    if words.iter().filter(|word| **word == "select").count() != 1 {
        return Some("The query has a subquery".to_string());
    }
    // the FROM clause: up to the next clause keyword
    if let Some(at) = blank.find(" from ").or_else(|| blank.find("\nfrom ")) {
        let rest = &blank[at + 6..];
        let end = [
            " where ", " order ", " limit ", " having ", " window ", " for ", " offset ", " fetch ",
        ]
        .iter()
        .filter_map(|keyword| rest.find(keyword))
        .min()
        .unwrap_or(rest.len());
        let from = &rest[..end];
        if from.contains(',') {
            return Some("The query reads from more than one table".to_string());
        }
        if from.contains('(') {
            return Some("The query reads from a subquery".to_string());
        }
    }
    None
}

/// The columns of a SQL source, from its first rows, and whether it is editable (§9.5).
fn sql_columns(
    facade: &EntityFacade,
    group: &str,
    sql: &str,
    requested: &[String],
    actor: &Actor,
) -> Result<ColumnsOutcome, Refusal> {
    let mysql = check_sql_source(facade, group, sql)?;
    let wrapped = wrap(sql, mysql, &[], &[], &format!(" LIMIT {COLUMN_PROBE_ROWS}"))?;
    let result = sql::grid_query(
        facade,
        group,
        &wrapped.sql,
        &wrapped.params,
        COLUMN_PROBE_ROWS,
        actor,
        "columns",
    )
    .map_err(|err| db_refusal(&err))?;
    if result.columns.is_empty() {
        return Ok(ColumnsOutcome {
            read_only_reason: Some("The query gave no rows, so its columns are not known".to_string()),
            ..ColumnsOutcome::default()
        });
    }
    let mut columns: Vec<Column> = result
        .columns
        .iter()
        .enumerate()
        .filter(|(_, name)| requested.is_empty() || requested.contains(name))
        .map(|(index, name)| {
            // the driver gives no column types; the first value that is not null tells the kind
            let kind = result
                .records
                .iter()
                .filter_map(|row| row.get(index))
                .find(|value| !matches!(value, Value::Null))
                .map_or("text", value_kind);
            Column {
                name: name.clone(),
                title: name.clone(),
                kind,
                ..Column::default()
            }
        })
        .collect();
    let editable = editable_table(facade, group, &result).and_then(|(entity, fields)| {
        match statement_shape_refusal(sql, mysql, &entity.table_name) {
            Some(reason) => Err(reason),
            None => Ok((entity, fields)),
        }
    });
    let mut outcome = ColumnsOutcome::default();
    match editable {
        Ok((entity, fields)) => {
            for column in &mut columns {
                let Some(field_name) = fields.get(&column.name) else {
                    continue;
                };
                if let Some(field) = entity.field(field_name) {
                    let mut from_field = field_column(&entity, field, true);
                    from_field.name = column.name.clone();
                    from_field.title = column.name.clone();
                    from_field.field = Some(field.name.clone());
                    *column = from_field;
                }
            }
            let label_of = |field: &str| {
                fields
                    .iter()
                    .find(|(_, name)| name.as_str() == field)
                    .map(|(label, _)| label.clone())
            };
            outcome.key_columns = entity.pk_field_names().into_iter().filter_map(label_of).collect();
            outcome.stamp_column = label_of(STAMP);
            outcome.has_stamp = outcome.stamp_column.is_some();
            outcome.entity_name = Some(entity.full_name.clone());
        }
        Err(reason) => outcome.read_only_reason = Some(reason),
    }
    columns.retain(|column| !is_secret(&column.name) && column.field.as_deref().is_none_or(|field| !is_secret(field)));
    outcome.columns = columns;
    Ok(outcome)
}

/// The schema (Postgres) or database (MySQL) the group's connection uses.
fn group_schema(facade: &EntityFacade, group: &str) -> Option<String> {
    let sql = if catalog::group_is_mysql(facade, group) {
        catalog::mysql::CURRENT_DATABASE
    } else {
        catalog::postgres::CURRENT_SCHEMA
    };
    let rows = catalog::query(facade, group, sql).ok()?;
    rows.first()?.values().next().map(to_plain_string)
}

/// The entity a SQL result can be saved to (§9.5): every column of one table (not a view) in the group's schema, an
/// entity of `group` on that table, and the result has the entity's whole primary key and, when the entity has one,
/// its `lastUpdatedStamp`. With the column label to field name of each column.
fn editable_table(
    facade: &EntityFacade,
    group: &str,
    result: &moqui_core::entity::info::SqlResult,
) -> Result<(EntityDefinition, BTreeMap<String, String>), String> {
    if result.column_origins.is_empty() || result.column_origins.len() != result.columns.len() {
        return Err("The database did not say which table each column comes from".to_string());
    }
    let sources =
        moqui_core::entity::info::resolve_column_origins(facade, group, &result.column_origins).map_err(|err| {
            tracing::info!(error = %err, "Could not read the tables of a data grid query's columns");
            "The columns' tables could not be read".to_string()
        })?;
    let mut table: Option<(String, String)> = None;
    for source in &sources {
        let Some(source) = source else {
            return Err("A column does not come from a table (an expression)".to_string());
        };
        if !source.is_table {
            return Err(format!("{} is a view, not a table", source.table));
        }
        let this = (source.schema.clone(), source.table.clone());
        if table.as_ref().is_some_and(|table| *table != this) {
            return Err("The columns come from more than one table".to_string());
        }
        table = Some(this);
    }
    let Some((schema, table)) = table else {
        return Err("The query has no columns".to_string());
    };
    match group_schema(facade, group) {
        Some(own) if own.eq_ignore_ascii_case(&schema) => {}
        _ => return Err(format!("{schema}.{table} is not in the schema the entities use")),
    }
    let groups = facade.group_names();
    let default = facade.default_group().to_string();
    let found = facade.with_model(|model| {
        model
            .entities()
            .iter()
            .filter(|entity| !entity.is_view)
            .filter(|entity| {
                entity
                    .group
                    .as_deref()
                    .filter(|name| groups.iter().any(|open| open == name))
                    .unwrap_or(&default)
                    == group
            })
            .find(|entity| entity.table_name.eq_ignore_ascii_case(&table))
            .cloned()
    });
    let Some(entity) = found else {
        return Err(format!("No entity is on table {table}"));
    };
    if sensitive::is_restricted_generic_entity(&entity.full_name) {
        return Err(format!("{} is not available in the data grid", entity.full_name));
    }
    let mut fields = BTreeMap::new();
    for (label, source) in result.columns.iter().zip(sources.iter().flatten()) {
        let field = entity
            .fields
            .iter()
            .find(|field| field.column_name.eq_ignore_ascii_case(&source.column))
            .ok_or_else(|| format!("Column {} is no field of {}", source.column, entity.full_name))?;
        if fields.values().any(|name: &String| name == &field.name) {
            return Err(format!("The query has {} twice", field.name));
        }
        fields.insert(label.clone(), field.name.clone());
    }
    let named: Vec<&String> = fields.values().collect();
    let mut needed: Vec<&str> = entity.pk_field_names();
    if entity.field(STAMP).is_some() {
        needed.push(STAMP);
    }
    if let Some(missing) = needed
        .into_iter()
        .find(|field| !named.iter().any(|name| name.as_str() == *field))
    {
        return Err(format!(
            "The result lacks {missing} of {}, which a save needs",
            entity.full_name
        ));
    }
    Ok((entity, fields))
}

/// The field name a search-form input key is about: the key without its `_op`, `_not`, `_ic`, `_from`, `_thru`,
/// `_period`, `_poffset` or `_pdate`.
fn condition_field(key: &str) -> &str {
    for suffix in ["_op", "_not", "_ic", "_from", "_thru", "_period", "_poffset", "_pdate"] {
        if let Some(field) = key.strip_suffix(suffix) {
            return field;
        }
    }
    key
}

/// The entity find of an entity source with its conditions and the grid's filters and sort, the primary key last in
/// the order so pages are stable.
fn entity_find(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    conditions: &BTreeMap<String, Value>,
    sort: &[Sort],
    filter: &[Filter],
) -> Result<EntityFind, Refusal> {
    for key in conditions.keys() {
        let field = condition_field(key);
        if hidden_value(entity, field) {
            return Err(refusal(format!("{field} takes no condition in the data grid")));
        }
    }
    let find = facade.find(&entity.full_name).map_err(|err| find_refusal(&err))?;
    let mut find = find
        .search_form_map(conditions, &BTreeMap::new(), "", "", false)
        .map_err(|err| refusal(err.message().to_string()))?
        .find;
    let field_of = |name: &str| {
        entity
            .field(name)
            .filter(|_| !hidden_value(entity, name))
            .ok_or_else(|| refusal(format!("{name} is not a field the grid sorts or filters on")))
    };
    for filter in filter {
        let field = field_of(&filter.field)?;
        let name = field.name.as_str();
        // text a person typed: a date-time without an offset is in the session's time zone
        let convert = |value: &Value| basic_convert_text(value, &field.java_type).value;
        let condition = if let Some(pattern) = like_pattern(filter) {
            if !field.is_text() {
                return Err(refusal(format!("{name} is not text: filter it with =, <, >, or in")));
            }
            Condition::ignore_case(Condition::Like(name.into(), Value::String(pattern)))
        } else {
            match filter.kind.as_str() {
                "=" => Condition::equals(name, convert(&filter.value)),
                "!=" => Condition::not_equals(name, convert(&filter.value)),
                "<" => Condition::less(name, convert(&filter.value)),
                "<=" => Condition::less_equal(name, convert(&filter.value)),
                ">" => Condition::greater(name, convert(&filter.value)),
                ">=" => Condition::greater_equal(name, convert(&filter.value)),
                "in" => {
                    let values: Vec<Value> = in_values(&filter.value).iter().map(convert).collect();
                    if values.is_empty() {
                        Condition::where_sql("1 = 0")
                    } else {
                        Condition::In(name.into(), values)
                    }
                }
                other => return Err(refusal(format!("filter type {other:?} is not one the grid takes"))),
            }
        };
        find = find.condition(condition);
    }
    let keys: Vec<String> = if entity.is_view {
        Vec::new()
    } else {
        entity.pk_field_names().into_iter().map(str::to_string).collect()
    };
    for sort in with_key_order(sort, &keys) {
        let field = field_of(&sort.field)?;
        find = find.order_by(&field.name, sort.descending);
    }
    Ok(find)
}

fn row_value(entity: &EntityDefinition, value: &EntityValue, columns: &[String]) -> Value {
    let mut map = IndexMap::new();
    let names: Vec<&str> = if columns.is_empty() {
        entity.fields.iter().map(|field| field.name.as_str()).collect()
    } else {
        columns.iter().map(String::as_str).collect()
    };
    for name in names {
        let Some(field) = entity.field(name) else { continue };
        if is_secret(name) {
            continue;
        }
        let shown = match value.get(name) {
            Some(value) if field.encrypt && !matches!(value, Value::Null) => Value::string(MASK),
            Some(value) => value.clone(),
            None => Value::Null,
        };
        map.insert(name.to_string(), shown);
    }
    if !entity.is_view {
        // the primary key, whether shown or not, for a save
        for pk in entity.pk_field_names() {
            if !map.contains_key(pk) {
                map.insert(pk.to_string(), value.get(pk).cloned().unwrap_or(Value::Null));
            }
        }
        if let Some(Value::Timestamp(stamp)) = value.get(STAMP) {
            map.insert("_stamp".to_string(), Value::String(stamp.to_string()));
        }
    }
    Value::Map(map)
}

/// What [`rows`] answers, Tabulator's progressive load shape.
#[derive(Clone, Debug, Default)]
pub struct RowsOutcome {
    /// The current page plus one while pages come back full; the page itself once one is short.
    pub last_page: usize,
    /// The row count, once a short page shows it; otherwise the catalog's estimate for an unfiltered entity source.
    pub last_row: Option<i64>,
    /// `last_row` is the catalog's estimate, not a count.
    pub estimated: bool,
    pub data: Vec<Value>,
}

/// The catalog's estimate of an entity's rows: Postgres `reltuples`, MySQL `TABLE_ROWS`. None when unknown.
fn estimate(facade: &EntityFacade, entity: &EntityDefinition) -> Option<i64> {
    let groups = facade.group_names();
    let group = entity
        .group
        .clone()
        .filter(|name| groups.contains(name))
        .unwrap_or_else(|| facade.default_group().to_string());
    let mysql = catalog::group_is_mysql(facade, &group);
    let (sql, table) = if mysql {
        (catalog::mysql::TABLE_ESTIMATE, entity.table_name.clone())
    } else {
        (
            catalog::postgres::TABLE_ESTIMATE,
            entity.table_name.to_ascii_lowercase(),
        )
    };
    let rows = catalog::query_with(facade, &group, sql, &[Value::String(table)]).ok()?;
    let estimate = rows
        .first()?
        .get("estimate")
        .map(to_plain_string)?
        .parse::<f64>()
        .ok()?;
    (estimate >= 0.0).then_some(estimate as i64)
}

/// The rows of a SQL source's result as maps of its columns; with the stamp column, `_stamp` as text.
fn sql_rows(result: moqui_core::entity::info::SqlResult, columns: &[String], stamp_column: Option<&str>) -> Vec<Value> {
    result
        .records
        .into_iter()
        .map(|record| {
            let mut map: IndexMap<String, Value> = IndexMap::new();
            for (name, value) in result.columns.iter().zip(record) {
                if Some(name.as_str()) == stamp_column {
                    if let Value::Timestamp(stamp) = &value {
                        map.insert("_stamp".to_string(), Value::String(stamp.to_string()));
                    }
                }
                if (columns.is_empty() || columns.contains(name)) && !is_secret(name) {
                    map.insert(name.clone(), value);
                }
            }
            Value::Map(map)
        })
        .collect()
}

/// One page of the grid: `page` from 1, `size` rows (at most [`MAX_PAGE_SIZE`]).
///
/// # Errors
///
/// Returns the refusal: a restricted entity, a field the grid does not sort or filter on, a SQL source that is not one
/// plain query, or the database's error.
#[allow(clippy::too_many_arguments)]
pub fn rows(
    facade: &EntityFacade,
    source: &Source,
    columns: &[String],
    page: usize,
    size: usize,
    sort: &[Sort],
    filter: &[Filter],
    actor: &Actor,
) -> Result<RowsOutcome, Refusal> {
    let page = page.max(1);
    let size = size.clamp(1, MAX_PAGE_SIZE);
    let offset = (page - 1).saturating_mul(size);
    let (mut data, unfiltered_estimate) = match source {
        Source::Entity {
            entity_name,
            conditions,
        } => {
            refuse_restricted(entity_name)?;
            let entity = definition(facade, entity_name)?;
            let find = entity_find(facade, &entity, conditions, sort, filter)?;
            let list = find
                .offset(i64::try_from(offset).unwrap_or(i64::MAX))
                .limit(i64::try_from(size + 1).unwrap_or(i64::MAX))
                .list()
                .map_err(|err| find_refusal(&err))?;
            let data: Vec<Value> = list.iter().map(|value| row_value(&entity, value, columns)).collect();
            let estimate = (conditions.is_empty() && filter.is_empty() && !entity.is_view)
                .then(|| estimate(facade, &entity))
                .flatten();
            (data, estimate)
        }
        Source::Sql {
            group,
            sql,
            key_columns,
            stamp_column,
        } => {
            let mysql = check_sql_source(facade, group, sql)?;
            let order = with_key_order(sort, key_columns);
            let wrapped = wrap(
                sql,
                mysql,
                &order,
                filter,
                &format!(" LIMIT {} OFFSET {offset}", size + 1),
            )?;
            let result = sql::grid_query(facade, group, &wrapped.sql, &wrapped.params, size + 1, actor, "rows")
                .map_err(|err| db_refusal(&err))?;
            (sql_rows(result, columns, stamp_column.as_deref()), None)
        }
        Source::File { spec } => {
            let rows = if sort.is_empty() && filter.is_empty() {
                crate::files::file_rows(spec, offset, size + 1).map_err(|err| file_refusal(&err))?
            } else {
                let (rows, _) = crate::files::file_query(spec, sort, filter).map_err(|err| file_refusal(&err))?;
                rows.into_iter().skip(offset).take(size + 1).collect()
            };
            (file_row_values(rows, columns), None)
        }
    };
    let more = data.len() > size;
    data.truncate(size);
    Ok(if more {
        RowsOutcome {
            last_page: page + 1,
            last_row: unfiltered_estimate,
            estimated: unfiltered_estimate.is_some(),
            data,
        }
    } else {
        RowsOutcome {
            last_page: page,
            last_row: Some(i64::try_from(offset + data.len()).unwrap_or(i64::MAX)),
            estimated: false,
            data,
        }
    })
}

/// The count of the source's rows, with its conditions and the filters: an entity find count, or `count(*)` over the
/// wrapped SQL.
///
/// # Errors
///
/// As [`rows`].
pub fn count(facade: &EntityFacade, source: &Source, filter: &[Filter], actor: &Actor) -> Result<i64, Refusal> {
    match source {
        Source::Entity {
            entity_name,
            conditions,
        } => {
            refuse_restricted(entity_name)?;
            let entity = definition(facade, entity_name)?;
            entity_find(facade, &entity, conditions, &[], filter)?
                .count()
                .map_err(|err| find_refusal(&err))
        }
        Source::Sql { group, sql, .. } => {
            let mysql = check_sql_source(facade, group, sql)?;
            let wrapped = wrap(sql, mysql, &[], filter, "")?;
            let counted = format!("SELECT count(*) AS row_count FROM ({}) c", wrapped.sql);
            let result = sql::grid_query(facade, group, &counted, &wrapped.params, 1, actor, "count")
                .map_err(|err| db_refusal(&err))?;
            result
                .records
                .first()
                .and_then(|record| record.first())
                .map(to_plain_string)
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| refusal("The count gave no number"))
        }
        Source::File { spec } => {
            let counted = if filter.is_empty() {
                crate::files::file_count(spec)
            } else {
                crate::files::file_query(spec, &[], filter).map(|(rows, _)| rows.len())
            };
            counted
                .map(|count| i64::try_from(count).unwrap_or(i64::MAX))
                .map_err(|err| file_refusal(&err))
        }
    }
}

/// An export's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Xlsx,
}

/// What [`export`] answers.
#[derive(Clone, Debug, Default)]
pub struct ExportOutcome {
    /// Where the file was written.
    pub location: String,
    pub file_name: String,
    pub content_type: &'static str,
    pub row_count: usize,
    /// The export stopped at its cap ([`MAX_XLSX_ROWS`], [`MAX_SQL_EXPORT_ROWS`]).
    pub truncated: bool,
}

fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        other => to_plain_string(other),
    }
}

/// The default export directory of `user_id`: `runtime://tmp/grid-export/<userId>/`, which `clean#GridUploads` empties
/// of files older than a day. A download of a file there checks that the user is the one its path names.
#[must_use]
pub fn export_dir(user_id: &str) -> String {
    format!("runtime://tmp/grid-export/{user_id}/")
}

/// Whether `actor` may write an export to `location`: under its own [`export_dir`], or its own
/// `dbresource://moqui/database/export/<userId>/`. With `admin` (`DATABASE_ADMIN`) also any `dbresource://` location
/// and anything under `runtime://export/`; never `component://`, `file:` or another `runtime://` place.
#[must_use]
pub fn export_location_allowed(location: &str, actor: &Actor, admin: bool) -> bool {
    let location = location.trim();
    if location.is_empty() || location.split('/').any(|part| part == ".." || part == ".") || location.contains('\\') {
        return false;
    }
    let admin_place = |location: &str| {
        ["dbresource://", "runtime://export/"]
            .iter()
            .any(|prefix| location.starts_with(prefix) && location.len() > prefix.len())
    };
    if admin && admin_place(location) {
        return true;
    }
    let Some(user) = actor
        .user_id
        .as_deref()
        .filter(|user| !user.is_empty() && !user.contains('/'))
    else {
        return false;
    };
    [export_dir(user), format!("dbresource://moqui/database/export/{user}/")]
        .iter()
        .any(|prefix| location.starts_with(prefix.as_str()) && location.len() > prefix.len())
}

/// Write the whole source (its conditions, filters and sort) as CSV or XLSX to `location`, or to a new location under
/// the user's [`export_dir`] when empty. The caller checked the location ([`export_location_allowed`]). An entity
/// source is read through the find's cursor and CSV is written row by row to an owner-only temporary file, removed
/// whatever happens; XLSX is built in memory and capped at [`MAX_XLSX_ROWS`]; a SQL source is capped at
/// [`MAX_SQL_EXPORT_ROWS`]. Encrypted fields are masked and secrets left out, as in the grid.
///
/// # Errors
///
/// Returns the refusal: as [`rows`], or the file could not be written.
#[allow(clippy::too_many_arguments)]
pub fn export(
    facade: &EntityFacade,
    source: &Source,
    columns: &[String],
    sort: &[Sort],
    filter: &[Filter],
    format: ExportFormat,
    location: &str,
    actor: &Actor,
) -> Result<ExportOutcome, Refusal> {
    let (extension, content_type) = match format {
        ExportFormat::Csv => ("csv", "text/csv"),
        ExportFormat::Xlsx => (
            "xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
    };
    let base = match source {
        Source::Entity { entity_name, .. } => entity_name.rsplit('.').next().unwrap_or("export").to_string(),
        Source::Sql { .. } => "query".to_string(),
        Source::File { spec } => spec
            .location
            .rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.').map(|(stem, _)| stem).or(Some(name)))
            .filter(|stem| !stem.is_empty())
            .unwrap_or("file")
            .to_string(),
    };
    let file_name = format!("{base}.{extension}");
    let location = if location.trim().is_empty() {
        let id = moqui_core::entity::uuid_v4()
            .map_err(|err| refusal(err.to_string()))?
            .replace('-', "");
        let user = actor.user_id.clone().unwrap_or_else(|| "_NA_".to_string());
        format!("{}{id}/{file_name}", export_dir(&user))
    } else {
        location.trim().to_string()
    };
    let cap = match source {
        Source::Sql { .. } => MAX_SQL_EXPORT_ROWS,
        Source::Entity { .. } => usize::MAX,
        Source::File { .. } => crate::files::MAX_QUERY_ROWS,
    };
    let cap = if format == ExportFormat::Xlsx {
        cap.min(MAX_XLSX_ROWS)
    } else {
        cap
    };
    let mut truncated = false;
    let writer = match source {
        Source::Entity {
            entity_name,
            conditions,
        } => {
            refuse_restricted(entity_name)?;
            let entity = definition(facade, entity_name)?;
            let names: Vec<String> = if columns.is_empty() {
                entity.fields.iter().map(|field| field.name.clone()).collect()
            } else {
                columns
                    .iter()
                    .filter(|name| entity.field(name).is_some())
                    .cloned()
                    .collect()
            };
            let names: Vec<String> = names.into_iter().filter(|name| !is_secret(name)).collect();
            let mut iterator = entity_find(facade, &entity, conditions, sort, filter)?
                .iterator()
                .map_err(|err| find_refusal(&err))?;
            // the checks are done: only now the temporary file
            let mut writer = TableWriter::new(format).map_err(|err| refusal(err.to_string()))?;
            writer.header(&names).map_err(|err| refusal(err.to_string()))?;
            while let Some(value) = iterator.next_value().map_err(|err| find_refusal(&err))? {
                if writer.rows >= cap {
                    truncated = true;
                    break;
                }
                let Value::Map(row) = row_value(&entity, &value, &names) else {
                    continue;
                };
                let cells: Vec<Value> = names
                    .iter()
                    .map(|name| row.get(name).cloned().unwrap_or(Value::Null))
                    .collect();
                writer.row(&cells).map_err(|err| refusal(err.to_string()))?;
            }
            writer
        }
        Source::Sql {
            group,
            sql,
            key_columns,
            ..
        } => {
            let mysql = check_sql_source(facade, group, sql)?;
            let wrapped = wrap(sql, mysql, &with_key_order(sort, key_columns), filter, "")?;
            let result = sql::grid_query(facade, group, &wrapped.sql, &wrapped.params, cap, actor, "export")
                .map_err(|err| db_refusal(&err))?;
            truncated = result.limit_reached;
            let keep: Vec<usize> = result
                .columns
                .iter()
                .enumerate()
                .filter(|(_, name)| (columns.is_empty() || columns.contains(name)) && !is_secret(name))
                .map(|(index, _)| index)
                .collect();
            let names: Vec<String> = keep.iter().map(|index| result.columns[*index].clone()).collect();
            let mut writer = TableWriter::new(format).map_err(|err| refusal(err.to_string()))?;
            writer.header(&names).map_err(|err| refusal(err.to_string()))?;
            for record in &result.records {
                let cells: Vec<Value> = keep
                    .iter()
                    .map(|index| record.get(*index).cloned().unwrap_or(Value::Null))
                    .collect();
                writer.row(&cells).map_err(|err| refusal(err.to_string()))?;
            }
            writer
        }
        Source::File { spec } => {
            let (file_columns, _) = crate::files::file_columns(spec).map_err(|err| file_refusal(&err))?;
            let names: Vec<String> = file_columns
                .into_iter()
                .map(|column| column.name)
                .filter(|name| (columns.is_empty() || columns.contains(name)) && !is_secret(name))
                .collect();
            let (rows, more) = crate::files::file_query(spec, sort, filter).map_err(|err| file_refusal(&err))?;
            truncated = more || rows.len() > cap;
            let mut writer = TableWriter::new(format).map_err(|err| refusal(err.to_string()))?;
            writer.header(&names).map_err(|err| refusal(err.to_string()))?;
            for row in rows.iter().take(cap) {
                let cells: Vec<Value> = names
                    .iter()
                    .map(|name| row.get(name).cloned().unwrap_or(Value::Null))
                    .collect();
                writer.row(&cells).map_err(|err| refusal(err.to_string()))?;
            }
            writer
        }
    };
    let row_count = writer.rows;
    writer.finish(&location).map_err(|err| {
        tracing::warn!(error = %err, "Could not write the grid export to {location}");
        refusal("The export could not be written (the server log has the detail)")
    })?;
    Ok(ExportOutcome {
        location,
        file_name,
        content_type,
        row_count,
        truncated,
    })
}

/// CSV into an owner-only temporary file, written to the location at the end as a stream and removed whatever
/// happens (also when the writer is dropped on a failure); XLSX into an `XlsxBook`.
struct TableWriter {
    csv: Option<csv::Writer<std::fs::File>>,
    path: Option<std::path::PathBuf>,
    book: Option<moqui_core::screen::XlsxBook>,
    rows: usize,
}

impl Drop for TableWriter {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

impl TableWriter {
    fn new(format: ExportFormat) -> Result<Self, Error> {
        Ok(match format {
            ExportFormat::Csv => {
                let path = std::env::temp_dir().join(format!(
                    "moqui-grid-{}.csv",
                    moqui_core::entity::uuid_v4()?.replace('-', "")
                ));
                let file = private_file(&path).map_err(|err| Error::new(format!("export file: {err}")))?;
                Self {
                    csv: Some(csv::Writer::from_writer(file)),
                    path: Some(path),
                    book: None,
                    rows: 0,
                }
            }
            ExportFormat::Xlsx => {
                let mut book = moqui_core::screen::XlsxBook::default();
                book.start_sheet("Data");
                Self {
                    csv: None,
                    path: None,
                    book: Some(book),
                    rows: 0,
                }
            }
        })
    }

    fn header(&mut self, names: &[String]) -> Result<(), Error> {
        if let Some(csv) = &mut self.csv {
            csv.write_record(names).map_err(|err| Error::new(err.to_string()))?;
        }
        if let Some(book) = &mut self.book {
            for name in names {
                book.header(name);
            }
        }
        Ok(())
    }

    fn row(&mut self, cells: &[Value]) -> Result<(), Error> {
        if let Some(csv) = &mut self.csv {
            csv.write_record(cells.iter().map(cell_text))
                .map_err(|err| Error::new(err.to_string()))?;
        }
        if let Some(book) = &mut self.book {
            for cell in cells {
                book.cell(
                    &[(cell.clone(), String::new())],
                    moqui_core::screen::CellHints::default(),
                )
                .map_err(|err| Error::new(err.to_string()))?;
            }
            book.end_row();
        }
        self.rows += 1;
        Ok(())
    }

    fn finish(mut self, location: &str) -> Result<(), Error> {
        let write = |input: &mut dyn std::io::Read, size: Option<u64>| {
            ExecutionContext::with_current(|ec| ec.factory().resource().write_from(location, input, size))
                .ok_or_else(|| Error::new("no execution context to write the export"))?
                .map_err(|err| Error::new(err.to_string()))
        };
        if let Some(mut csv) = self.csv.take() {
            csv.flush().map_err(|err| Error::new(err.to_string()))?;
            drop(csv);
            let path = self.path.clone().unwrap_or_default();
            let mut file = std::fs::File::open(&path).map_err(|err| Error::new(err.to_string()))?;
            let size = file.metadata().ok().map(|meta| meta.len());
            return write(&mut file, size);
        }
        let bytes = self
            .book
            .take()
            .map(|book| book.to_bytes())
            .transpose()
            .map_err(|err| Error::new(err.to_string()))?
            .unwrap_or_default();
        let size = Some(bytes.len() as u64);
        write(&mut std::io::Cursor::new(bytes), size)
    }
}

/// One option of [`lookup`].
#[derive(Clone, Debug, PartialEq)]
pub struct LookupOption {
    pub value: Value,
    pub label: String,
}

/// The options of the related-entity column `field` of `entity_name`: the related entity's rows whose key or label
/// holds `text` (ignoring case), [`LOOKUP_SIZE`] at a time, from `offset`. The label is the related entity's
/// `description` field, else a `…Name` field, else the key.
///
/// # Errors
///
/// Returns the refusal: a restricted entity on either side, or a field with no single-key related entity.
pub fn lookup(
    facade: &EntityFacade,
    entity_name: &str,
    field: &str,
    text: &str,
    offset: i64,
) -> Result<Vec<LookupOption>, Refusal> {
    refuse_restricted(entity_name)?;
    let entity = definition(facade, entity_name)?;
    let Some((related_name, related_field)) = related_of(&entity, field) else {
        return Err(refusal(format!("{field} of {entity_name} names no related entity")));
    };
    refuse_restricted(&related_name)?;
    let related = definition(facade, &related_name)?;
    let label_field = related
        .field("description")
        .or_else(|| {
            related
                .fields
                .iter()
                .find(|field| field.name.ends_with("Name") && field.is_text())
        })
        .filter(|field| !field.encrypt && !is_secret(&field.name))
        .map(|field| field.name.clone());
    let mut find = facade.find(&related.full_name).map_err(|err| find_refusal(&err))?;
    let text = text.trim();
    if !text.is_empty() {
        let pattern = Value::String(format!("%{}%", escape_like(&text.to_lowercase())));
        let mut any = vec![Condition::ignore_case(Condition::Like(
            related_field.as_str().into(),
            pattern.clone(),
        ))];
        if let Some(label) = &label_field {
            any.push(Condition::ignore_case(Condition::Like(label.as_str().into(), pattern)));
        }
        find = find.condition(Condition::Or(any));
    }
    let order = label_field.clone().unwrap_or_else(|| related_field.clone());
    let list = find
        .order_by(&order, false)
        .order_by(&related_field, false)
        .offset(offset.max(0))
        .limit(LOOKUP_SIZE)
        .list()
        .map_err(|err| find_refusal(&err))?;
    Ok(list
        .iter()
        .map(|row| {
            let value = row.get(&related_field).cloned().unwrap_or(Value::Null);
            let key = to_plain_string(&value);
            let label = label_field
                .as_ref()
                .and_then(|label| row.get(label))
                .map(to_plain_string)
                .filter(|label| !label.is_empty())
                .map_or_else(|| key.clone(), |label| format!("{label} [{key}]"));
            LookupOption { value, label }
        })
        .collect())
}

// ---- save (§9.4) ----

/// One change of a save: the primary key (updates and deletes), the stamp the grid read, and the edited fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub pk: BTreeMap<String, Value>,
    pub stamp: Option<String>,
    pub fields: BTreeMap<String, Value>,
}

/// The changes of [`save`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    pub updates: Vec<Change>,
    pub creates: Vec<Change>,
    pub deletes: Vec<Change>,
}

fn map_of(value: Option<&Value>) -> BTreeMap<String, Value> {
    match value {
        Some(Value::Map(map)) => map.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
        _ => BTreeMap::new(),
    }
}

impl Changes {
    /// Read `updates`, `creates` and `deletes`: lists of `{pk, stamp, fields}`.
    #[must_use]
    pub fn parse(parameters: &BTreeMap<String, Value>) -> Self {
        let list = |key: &str| match parameters.get(key) {
            Some(Value::List(items)) => items
                .iter()
                .filter_map(|item| match item {
                    Value::Map(map) => Some(Change {
                        pk: map_of(map.get("pk")),
                        stamp: map.get("stamp").filter(|value| !is_empty(value)).map(to_plain_string),
                        fields: map_of(map.get("fields")),
                    }),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Self {
            updates: list("updates"),
            creates: list("creates"),
            deletes: list("deletes"),
        }
    }

    fn len(&self) -> usize {
        self.updates.len() + self.creates.len() + self.deletes.len()
    }
}

/// A refused row of a save.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RefusedRow {
    /// `update`, `create` or `delete`.
    pub kind: &'static str,
    /// Its place in its list.
    pub index: usize,
    pub pk: BTreeMap<String, Value>,
    pub message: String,
    /// Field name to message.
    pub field_errors: BTreeMap<String, String>,
    /// The row changed, or is gone, since the grid read it.
    pub conflict: bool,
}

impl RefusedRow {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert("kind".to_string(), Value::string(self.kind));
        map.insert(
            "index".to_string(),
            Value::I64(i64::try_from(self.index).unwrap_or(i64::MAX)),
        );
        map.insert("pk".to_string(), Value::Map(self.pk.clone().into_iter().collect()));
        map.insert("message".to_string(), Value::string(&self.message));
        map.insert(
            "fieldErrors".to_string(),
            Value::Map(
                self.field_errors
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::string(v)))
                    .collect(),
            ),
        );
        map.insert("conflict".to_string(), Value::Bool(self.conflict));
        Value::Map(map)
    }
}

/// What [`save`] answers.
#[derive(Clone, Debug, Default)]
pub struct SaveOutcome {
    /// Everything was saved; otherwise nothing was.
    pub saved: bool,
    pub refused: Vec<RefusedRow>,
    pub updated: usize,
    pub created: usize,
    pub deleted: usize,
    /// The primary key of each created row, in the order of `creates` (a sequenced key the service made included).
    pub created_keys: Vec<BTreeMap<String, Value>>,
}

/// The one answer to a row changed or gone since the grid read it, so it does not tell rows the user's filters hide.
const CONFLICT: &str = "The row was changed or removed by someone else since you loaded it: reload it";

/// Save the grid's changes to `entity_name` in one transaction: each through the entity's `update#`, `create#` or
/// `delete#` (its authorization, validation, ECA rules and audit log apply). For an entity with `lastUpdatedStamp` an
/// update or delete must send the stamp the grid read, and a row whose stamp differs (read `for update`), or that is
/// gone, is refused as a conflict. A refused row rolls back every other: the answer lists the refused rows with their
/// field errors. Every row is checked before any is written, so all the conflicts and field refusals show at once; a
/// refusal of the entity-auto service stops the rest. Restricted entities, views, `UserAccount` secrets and encrypted
/// fields are refused. Writes a `DbaGridSave` audit row with the entity and the counts.
///
/// # Errors
///
/// Returns the refusal of a save that did not start: a restricted entity, a view, no changes.
pub fn save(
    facade: &EntityFacade,
    entity_name: &str,
    changes: &Changes,
    actor: &Actor,
) -> Result<SaveOutcome, Refusal> {
    refuse_restricted(entity_name)?;
    let entity = definition(facade, entity_name)?;
    if let Some(reason) = read_only_reason(&entity) {
        return Err(refusal(reason));
    }
    if changes.len() == 0 {
        return Err(refusal("There are no changes to save"));
    }
    let pk_names: Vec<String> = entity.pk_field_names().into_iter().map(str::to_string).collect();
    let tx = facade
        .transactions_for(&entity.full_name)
        .map_err(|err| refusal(err.to_string()))?;
    let mut outcome = SaveOutcome::default();
    let started = std::time::Instant::now();
    let ran = tx.run(TransactionAttribute::ForceNew, None, || {
        // check every row first
        for (kind, list) in [
            ("update", &changes.updates),
            ("create", &changes.creates),
            ("delete", &changes.deletes),
        ] {
            for (index, change) in list.iter().enumerate() {
                if let Some(refused) = check_change(facade, &entity, &pk_names, kind, index, change)? {
                    outcome.refused.push(refused);
                }
            }
        }
        if !outcome.refused.is_empty() {
            return Err(Error::new("refused rows"));
        }
        for (kind, list) in [
            ("update", &changes.updates),
            ("create", &changes.creates),
            ("delete", &changes.deletes),
        ] {
            for (index, change) in list.iter().enumerate() {
                match apply_change(&entity, &pk_names, kind, index, change) {
                    Err(refused) => {
                        outcome.refused.push(refused);
                        return Err(Error::new("refused row"));
                    }
                    Ok(key) => match kind {
                        "update" => outcome.updated += 1,
                        "create" => {
                            outcome.created += 1;
                            outcome.created_keys.push(key);
                        }
                        _ => outcome.deleted += 1,
                    },
                }
            }
        }
        Ok(())
    });
    outcome.saved = ran.is_ok();
    if let Err(err) = &ran {
        if outcome.refused.is_empty() {
            // the commit, or a check, failed
            tracing::warn!(error = %err, "A data grid save of {} failed", entity.full_name);
            outcome.refused.push(RefusedRow {
                kind: "save",
                message: "The changes could not be saved (the server log has the detail)".to_string(),
                ..RefusedRow::default()
            });
        }
        outcome.updated = 0;
        outcome.created = 0;
        outcome.deleted = 0;
        outcome.created_keys.clear();
    }
    audit::record_as(
        facade,
        &AuditEntry {
            action: "DbaGridSave",
            is_change: outcome.saved,
            rows_affected: Some(i64::try_from(outcome.updated + outcome.created + outcome.deleted).unwrap_or(i64::MAX)),
            duration_ms: Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)),
            error_text: (!outcome.saved).then(|| format!("{} rows refused; nothing saved", outcome.refused.len())),
            detail_json: Some(
                serde_json::json!({"entity": entity.full_name, "updates": changes.updates.len(),
                    "creates": changes.creates.len(), "deletes": changes.deletes.len(), "saved": outcome.saved,
                    "refused": outcome.refused.len()})
                .to_string(),
            ),
            ..AuditEntry::default()
        },
        actor,
    );
    Ok(outcome)
}

fn stamp_matches(stored: Option<&Value>, given: &str) -> bool {
    let Some(Value::Timestamp(stored)) = stored else {
        // no stamp stored: nothing to compare
        return true;
    };
    match given.trim().parse::<jiff::Timestamp>() {
        Ok(given) => given.as_microsecond() == stored.as_microsecond(),
        Err(_) => given
            .trim()
            .parse::<i64>()
            .is_ok_and(|millis| millis == stored.as_millisecond()),
    }
}

/// The refusal of a change found before any is written: a missing key or stamp, a field the grid may not write, a
/// conflict. The row is read with the user's authorization, so a row the user may not see is a conflict as one that
/// is gone.
fn check_change(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    pk_names: &[String],
    kind: &'static str,
    index: usize,
    change: &Change,
) -> Result<Option<RefusedRow>, Error> {
    let mut refused = RefusedRow {
        kind,
        index,
        pk: change.pk.clone(),
        ..RefusedRow::default()
    };
    let key_source = if kind == "create" { &change.fields } else { &change.pk };
    for pk in pk_names {
        if key_source.get(pk).is_none_or(is_empty) && !(kind == "create" && pk_names.len() == 1) {
            refused.field_errors.insert(pk.clone(), format!("{pk} is required"));
        }
    }
    for name in change.fields.keys() {
        let Some(field) = entity.field(name) else {
            refused
                .field_errors
                .insert(name.clone(), format!("{name} is not a field of {}", entity.full_name));
            continue;
        };
        if is_secret(name) {
            refused
                .field_errors
                .insert(name.clone(), format!("{name} is not written through the data grid"));
        } else if field.encrypt {
            refused.field_errors.insert(
                name.clone(),
                format!("{name} is encrypted: change it in the record's form"),
            );
        } else if kind == "update" && field.is_pk {
            refused
                .field_errors
                .insert(name.clone(), format!("{name} is part of the key and is not changed"));
        } else if name == STAMP {
            refused
                .field_errors
                .insert(name.clone(), format!("{STAMP} is the database's"));
        }
    }
    let stamped = entity.field(STAMP).is_some();
    if refused.field_errors.is_empty() && kind != "create" {
        if stamped && change.stamp.is_none() {
            refused.message = "Reload the row: a change of it needs the stamp it was loaded with".to_string();
        } else {
            let mut find = facade.find(&entity.full_name)?.for_update(true);
            for pk in pk_names {
                find = find.eq(pk, change.pk.get(pk).cloned().unwrap_or(Value::Null));
            }
            let current = find.one();
            let current = match current {
                Ok(current) => current,
                // a row the user's authorization does not let them read is answered as a gone one
                Err(err) if err.message().contains("not authorized") => None,
                Err(err) => return Err(err),
            };
            let conflict = match (&current, &change.stamp) {
                (None, _) => true,
                (Some(row), Some(given)) if stamped => !stamp_matches(row.get(STAMP), given),
                _ => false,
            };
            if conflict {
                refused.message = CONFLICT.to_string();
                refused.conflict = true;
            }
        }
    }
    if refused.field_errors.is_empty() && refused.message.is_empty() {
        return Ok(None);
    }
    if refused.message.is_empty() {
        refused.message = "Some fields cannot be saved".to_string();
    }
    Ok(Some(refused))
}

/// Run one change through the entity-auto service. The errors it adds are the refusal's, taken off the messages. The
/// key of the row: for a create, the given key with what the service made.
fn apply_change(
    entity: &EntityDefinition,
    pk_names: &[String],
    kind: &'static str,
    index: usize,
    change: &Change,
) -> Result<BTreeMap<String, Value>, RefusedRow> {
    let mut parameters: BTreeMap<String, Value> = change.fields.clone();
    if kind != "create" {
        for pk in pk_names {
            parameters.insert(pk.clone(), change.pk.get(pk).cloned().unwrap_or(Value::Null));
        }
    }
    if kind == "delete" {
        parameters.retain(|name, _| pk_names.contains(name));
    }
    sensitive::remove_user_account_secrets(&mut parameters);
    let service = format!("{kind}#{}", entity.full_name);
    let marks = ExecutionContext::with_current(|ec| {
        let message = ec.message();
        (message.error_mark(), message.validation_errors().len())
    });
    let result = sync_checked(&service, &parameters, &CallOptions::default());
    let err = match result {
        Ok(out) => {
            return Ok(pk_names
                .iter()
                .filter_map(|pk| {
                    out.get(pk)
                        .filter(|value| !is_empty(value))
                        .or_else(|| parameters.get(pk))
                        .map(|value| (pk.clone(), value.clone()))
                })
                .collect())
        }
        Err(err) => err,
    };
    tracing::info!(error = %err, "The data grid's {service} refused a row");
    let mut refused = RefusedRow {
        kind,
        index,
        pk: if kind == "create" {
            pk_names
                .iter()
                .filter_map(|pk| change.fields.get(pk).map(|value| (pk.clone(), value.clone())))
                .collect()
        } else {
            change.pk.clone()
        },
        message: err.message().lines().next().unwrap_or_default().to_string(),
        ..RefusedRow::default()
    };
    if let Some((mark, validations)) = marks {
        ExecutionContext::with_current(|ec| {
            let mut message = ec.message();
            for validation in message.validation_errors().iter().skip(validations) {
                refused
                    .field_errors
                    .insert(validation.field.clone(), validation.message.clone());
            }
            message.take_errors_since(mark);
        });
    }
    Err(refused)
}

#[cfg(test)]
mod tests {
    use moqui_lib::object::Value;

    use super::{
        escape_like, export_location_allowed, parse_filter, parse_sort, stamp_matches, statement_shape_refusal, wrap,
        Filter, Sort,
    };
    use crate::audit::Actor;

    fn filter(field: &str, kind: &str, value: Value) -> Filter {
        Filter {
            field: field.to_string(),
            kind: kind.to_string(),
            value,
        }
    }

    #[test]
    fn a_sql_source_is_wrapped_with_bound_filters_and_quoted_names() {
        let filters = vec![
            filter("description", "like", Value::string("50%_x")),
            filter("n", ">=", Value::I64(3)),
            filter("id", "in", Value::string("a, b")),
            filter("none", "in", Value::List(Vec::new())),
        ];
        let sort = vec![Sort {
            field: "Total \"Due\"".to_string(),
            descending: true,
        }];
        let wrapped = wrap("SELECT * FROM t", false, &sort, &filters, " LIMIT 11 OFFSET 0").unwrap();
        assert_eq!(
            wrapped.sql,
            "SELECT * FROM (SELECT * FROM t) q WHERE LOWER(CAST(\"description\" AS TEXT)) LIKE LOWER(?) AND \"n\" >= ? \
             AND \"id\" IN (?, ?) AND 1 = 0 ORDER BY \"Total \"\"Due\"\"\" DESC LIMIT 11 OFFSET 0"
        );
        assert_eq!(wrapped.params[0], Value::string("%50\\%\\_x%"));
        assert!(
            wrap("SELECT ? AS a", false, &[], &filters, "").is_err(),
            "a ? of its own beside filters"
        );
        assert_eq!(escape_like("a\\b"), "a\\\\b");
    }

    #[test]
    fn a_result_of_one_table_named_once_is_the_only_shape_saved() {
        let sql = "SELECT enum_id, description FROM enumeration WHERE enum_type_id = 'x'";
        assert_eq!(statement_shape_refusal(sql, false, "ENUMERATION"), None);
        for sql in [
            "SELECT a.enum_id FROM enumeration a JOIN enumeration b ON b.enum_id = a.enum_id",
            "SELECT a.enum_id FROM enumeration a, enumeration_type t",
            "SELECT DISTINCT enum_id FROM enumeration",
            "SELECT enum_id FROM enumeration GROUP BY enum_id",
            "SELECT enum_id FROM enumeration UNION SELECT enum_id FROM enumeration",
            "SELECT enum_id FROM enumeration WHERE enum_id IN (SELECT enum_id FROM enumeration)",
        ] {
            assert!(statement_shape_refusal(sql, false, "enumeration").is_some(), "{sql}");
        }
        // a table name in a quoted string does not count
        assert_eq!(
            statement_shape_refusal(
                "SELECT enum_id FROM enumeration WHERE description = 'enumeration'",
                false,
                "enumeration"
            ),
            None
        );
    }

    #[test]
    fn exports_go_to_the_users_own_places_unless_an_admin_says_otherwise() {
        let ana = Actor {
            user_id: Some("ANA".to_string()),
            ..Actor::default()
        };
        assert!(export_location_allowed(
            "runtime://tmp/grid-export/ANA/x/a.csv",
            &ana,
            false
        ));
        assert!(export_location_allowed(
            "dbresource://moqui/database/export/ANA/a.csv",
            &ana,
            false
        ));
        assert!(!export_location_allowed(
            "runtime://tmp/grid-export/BO/x/a.csv",
            &ana,
            false
        ));
        assert!(!export_location_allowed(
            "runtime://tmp/grid-export/ANA/../BO/a.csv",
            &ana,
            false
        ));
        assert!(!export_location_allowed("dbresource://other/a.csv", &ana, false));
        assert!(!export_location_allowed(
            "component://framework/data/a.csv",
            &ana,
            false
        ));
        assert!(export_location_allowed("dbresource://other/a.csv", &ana, true));
        assert!(export_location_allowed("runtime://export/a.csv", &ana, true));
        assert!(export_location_allowed(
            "runtime://tmp/grid-export/ANA/x/a.csv",
            &ana,
            true
        ));
        for refused in [
            "component://framework/data/a.csv",
            "file:/etc/a.csv",
            "runtime://conf/a.csv",
            "runtime://tmp/other/a.csv",
            "runtime://tmp/grid-export/BO/x/a.csv",
        ] {
            assert!(!export_location_allowed(refused, &ana, true), "{refused}");
        }
    }

    #[test]
    fn tabulator_sorts_filters_and_stamps_are_read() {
        let mut sort = indexmap::IndexMap::new();
        sort.insert("field".to_string(), Value::string("a"));
        sort.insert("dir".to_string(), Value::string("desc"));
        assert_eq!(
            parse_sort(Some(&Value::List(vec![Value::Map(sort)]))),
            vec![Sort {
                field: "a".to_string(),
                descending: true
            }]
        );
        let mut map = indexmap::IndexMap::new();
        map.insert("field".to_string(), Value::string("b"));
        map.insert("value".to_string(), Value::I64(1));
        assert_eq!(parse_filter(Some(&Value::List(vec![Value::Map(map)])))[0].kind, "=");
        let stored = "2026-11-05T10:00:00.123456Z".parse::<jiff::Timestamp>().unwrap();
        assert!(stamp_matches(
            Some(&Value::Timestamp(stored)),
            "2026-11-05T10:00:00.123456Z"
        ));
        assert!(!stamp_matches(Some(&Value::Timestamp(stored)), "2026-11-05T10:00:00Z"));
    }
}
