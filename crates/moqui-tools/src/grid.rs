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
//!   conditions by the `search-form-inputs` rules, or a SQL query wrapped as `SELECT * FROM (<sql>) q`, run read only
//!   and audited through [`crate::sql::grid_query`].
//! - **Calls:** [`columns`], [`rows`] (Tabulator's progressive load, never counting), [`count`], [`export`] (CSV
//!   streamed, XLSX capped at [`MAX_XLSX_ROWS`]), [`lookup`] (a related entity's options) and [`save`] (one
//!   transaction of entity-auto calls, with the stamp checked).
//! - **Editing a SQL result** (§9.5): every column of one table with its entity's whole primary key, decided from the
//!   result's column origins.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
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

fn refusal(message: impl Into<String>) -> Refusal {
    Refusal(message.into())
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
    /// A query on an entity group's database.
    Sql { group: String, sql: String },
}

impl Source {
    /// Read the spec's `source` map: `{type: entity, entityName, conditions}` or `{type: sql, db, sql}`.
    ///
    /// # Errors
    ///
    /// Returns the refusal of a spec that names no source, an unknown group, or an external database.
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
                }),
                Ok(crate::DbRef::Group(group)) => Err(refusal(format!("no entity group named {group}"))),
                Ok(crate::DbRef::External(_)) => Err(refusal(crate::NOT_AVAILABLE)),
                Err(message) => Err(refusal(message)),
            },
            other => Err(refusal(format!("source type {other:?} is neither entity nor sql"))),
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

fn is_secret(entity: &EntityDefinition, field: &str) -> bool {
    sensitive::is_user_account_entity(&entity.full_name) && sensitive::USER_ACCOUNT_SECRET_FIELDS.contains(&field)
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

/// The definition of `entity_name` (a view, `DbViewEntity` or `DataDocument.<id>` included).
fn definition(facade: &EntityFacade, entity_name: &str) -> Result<EntityDefinition, Refusal> {
    facade
        .with_entity(entity_name, |entity, _, _| Ok(entity.clone()))
        .map_err(|err| refusal(err.to_string()))
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
                    .filter(|field| !is_secret(&entity, &field.name))
                    .map(|field| field_column(&entity, field, reason.is_none()))
                    .collect(),
                entity_name: reason.is_none().then(|| entity.full_name.clone()),
                has_stamp: entity.field(STAMP).is_some(),
                read_only_reason: reason,
            })
        }
        Source::Sql { group, sql } => sql_columns(facade, group, sql, requested, actor),
    }
}

/// The grid's wrapped query: `SELECT * FROM (<sql>) q`, with the filters and order, and the values of the filters'
/// placeholders.
struct Wrapped {
    sql: String,
    params: Vec<Value>,
}

fn quote(name: &str, mysql: bool) -> Result<String, Refusal> {
    if name.is_empty() || !name.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
        return Err(refusal(format!(
            "{name:?} is not a column name the grid sorts or filters on"
        )));
    }
    Ok(if mysql {
        format!("`{name}`")
    } else {
        format!("\"{name}\"")
    })
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

fn like_pattern(filter: &Filter) -> Option<String> {
    let text = to_plain_string(&filter.value);
    match filter.kind.as_str() {
        "like" | "keywords" => Some(format!("%{text}%")),
        "starts" => Some(format!("{text}%")),
        "ends" => Some(format!("%{text}")),
        _ => None,
    }
}

fn wrap(sql: &str, mysql: bool, sort: &[Sort], filter: &[Filter], tail: &str) -> Result<Wrapped, Refusal> {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    for filter in filter {
        let column = quote(&filter.field, mysql)?;
        if let Some(pattern) = like_pattern(filter) {
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
                let values: Vec<Value> = match &filter.value {
                    Value::List(items) => items.clone(),
                    other => to_plain_string(other)
                        .split(',')
                        .map(|part| Value::string(part.trim()))
                        .collect(),
                };
                if values.is_empty() {
                    continue;
                }
                clauses.push(format!("{column} IN ({})", vec!["?"; values.len()].join(", ")));
                params.extend(values);
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
    if !params.is_empty() && moqui_core::entity::exec::blank_sql(sql, syntax(mysql)).contains('?') {
        return Err(refusal("A grid query with a ? of its own cannot be filtered"));
    }
    Ok(Wrapped { sql: out, params })
}

fn syntax(mysql: bool) -> moqui_core::entity::exec::SqlSyntax {
    if mysql {
        moqui_core::entity::exec::SqlSyntax::Mysql
    } else {
        moqui_core::entity::exec::SqlSyntax::Postgres
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

/// The columns of a SQL source, from one row of it, and whether it is editable (§9.5).
fn sql_columns(
    facade: &EntityFacade,
    group: &str,
    sql: &str,
    requested: &[String],
    actor: &Actor,
) -> Result<ColumnsOutcome, Refusal> {
    let mysql = check_sql_source(facade, group, sql)?;
    let wrapped = wrap(sql, mysql, &[], &[], " LIMIT 1")?;
    let result = sql::grid_query(facade, group, &wrapped.sql, &wrapped.params, 1, actor, "columns")?;
    let first = result.records.first();
    let editable = editable_table(facade, group, &result);
    let mut columns: Vec<Column> = result
        .columns
        .iter()
        .enumerate()
        .filter(|(_, name)| requested.is_empty() || requested.contains(name))
        .map(|(index, name)| {
            let kind = first.and_then(|row| row.get(index)).map_or("text", value_kind);
            Column {
                name: name.clone(),
                title: name.clone(),
                kind,
                ..Column::default()
            }
        })
        .collect();
    let (entity_name, reason) = match editable {
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
            (Some(entity.full_name.clone()), None)
        }
        Err(reason) => (None, Some(reason)),
    };
    if result.columns.is_empty() {
        return Ok(ColumnsOutcome {
            read_only_reason: Some("The query gave no rows, so its columns are not known".to_string()),
            ..ColumnsOutcome::default()
        });
    }
    Ok(ColumnsOutcome {
        columns,
        has_stamp: false,
        entity_name,
        read_only_reason: reason,
    })
}

/// The entity a SQL result can be saved to (§9.5): every column of one table (not a view), an entity of `group` on
/// that table, and the result has the entity's whole primary key. With the column label to field name of each column.
fn editable_table(
    facade: &EntityFacade,
    group: &str,
    result: &moqui_core::entity::info::SqlResult,
) -> Result<(EntityDefinition, BTreeMap<String, String>), String> {
    if result.column_origins.is_empty() || result.column_origins.len() != result.columns.len() {
        return Err("The database did not say which table each column comes from".to_string());
    }
    let sources = moqui_core::entity::info::resolve_column_origins(facade, group, &result.column_origins)
        .map_err(|err| format!("The columns' tables could not be read: {err}"))?;
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
    let Some((_, table)) = table else {
        return Err("The query has no columns".to_string());
    };
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
        fields.insert(label.clone(), field.name.clone());
    }
    let named: Vec<&String> = fields.values().collect();
    if let Some(missing) = entity
        .pk_field_names()
        .into_iter()
        .find(|pk| !named.iter().any(|name| name.as_str() == *pk))
    {
        return Err(format!(
            "The result lacks {missing}, part of the primary key of {}",
            entity.full_name
        ));
    }
    Ok((entity, fields))
}

/// The entity find of an entity source with its conditions and the grid's filters and sort.
fn entity_find(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    conditions: &BTreeMap<String, Value>,
    sort: &[Sort],
    filter: &[Filter],
) -> Result<EntityFind, Refusal> {
    let find = facade.find(&entity.full_name).map_err(|err| refusal(err.to_string()))?;
    let mut find = find
        .search_form_map(conditions, &BTreeMap::new(), "", "", false)
        .map_err(|err| refusal(err.to_string()))?
        .find;
    let field_of = |name: &str| {
        entity
            .field(name)
            .filter(|field| !field.encrypt && !is_secret(entity, name))
            .ok_or_else(|| refusal(format!("{name} is not a field the grid sorts or filters on")))
    };
    for filter in filter {
        let field = field_of(&filter.field)?;
        let name = field.name.as_str();
        let convert = |value: &Value| basic_convert(value, &field.java_type).value;
        let condition = if let Some(pattern) = like_pattern(filter) {
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
                    let values: Vec<Value> = match &filter.value {
                        Value::List(items) => items.iter().map(convert).collect(),
                        other => to_plain_string(other)
                            .split(',')
                            .map(|part| convert(&Value::string(part.trim())))
                            .collect(),
                    };
                    Condition::In(name.into(), values)
                }
                other => return Err(refusal(format!("filter type {other:?} is not one the grid takes"))),
            }
        };
        find = find.condition(condition);
    }
    for sort in sort {
        let field = field_of(&sort.field)?;
        find = find.order_by(&field.name, sort.descending);
    }
    if sort.is_empty() {
        // a stable order for paging
        for pk in entity.pk_field_names() {
            find = find.order_by(pk, false);
        }
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
        if is_secret(entity, name) {
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
                .map_err(|err| refusal(err.to_string()))?;
            let data: Vec<Value> = list.iter().map(|value| row_value(&entity, value, columns)).collect();
            let estimate = (conditions.is_empty() && filter.is_empty() && !entity.is_view)
                .then(|| estimate(facade, &entity))
                .flatten();
            (data, estimate)
        }
        Source::Sql { group, sql } => {
            let mysql = check_sql_source(facade, group, sql)?;
            let wrapped = wrap(
                sql,
                mysql,
                sort,
                filter,
                &format!(" LIMIT {} OFFSET {offset}", size + 1),
            )?;
            let result = sql::grid_query(facade, group, &wrapped.sql, &wrapped.params, size + 1, actor, "rows")?;
            let data = result
                .records
                .into_iter()
                .map(|record| {
                    Value::Map(
                        result
                            .columns
                            .iter()
                            .cloned()
                            .zip(record)
                            .filter(|(name, _)| columns.is_empty() || columns.contains(name))
                            .collect(),
                    )
                })
                .collect();
            (data, None)
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
                .map_err(|err| refusal(err.to_string()))
        }
        Source::Sql { group, sql } => {
            let mysql = check_sql_source(facade, group, sql)?;
            let wrapped = wrap(sql, mysql, &[], filter, "")?;
            let counted = format!("SELECT count(*) AS row_count FROM ({}) c", wrapped.sql);
            let result = sql::grid_query(facade, group, &counted, &wrapped.params, 1, actor, "count")?;
            result
                .records
                .first()
                .and_then(|record| record.first())
                .map(to_plain_string)
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| refusal("The count gave no number"))
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

/// Write the whole source (its conditions, filters and sort) as CSV or XLSX to `location`, or to a new location under
/// `runtime://tmp/grid-export/<user>/` when empty. An entity source is read through the find's cursor and CSV is
/// written row by row; XLSX is built in memory and capped at [`MAX_XLSX_ROWS`]; a SQL source is capped at
/// [`MAX_SQL_EXPORT_ROWS`]. Encrypted fields are masked as in the grid.
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
    };
    let file_name = format!("{base}.{extension}");
    let location = if location.trim().is_empty() {
        let id = moqui_core::entity::uuid_v4()
            .map_err(|err| refusal(err.to_string()))?
            .replace('-', "");
        let user = actor.user_id.clone().unwrap_or_else(|| "_NA_".to_string());
        format!("runtime://tmp/grid-export/{user}/{id}/{file_name}")
    } else {
        location.trim().to_string()
    };
    let mut writer = TableWriter::new(format).map_err(|err| refusal(err.to_string()))?;
    let cap = match source {
        Source::Sql { .. } => MAX_SQL_EXPORT_ROWS,
        Source::Entity { .. } => usize::MAX,
    };
    let cap = if format == ExportFormat::Xlsx {
        cap.min(MAX_XLSX_ROWS)
    } else {
        cap
    };
    let mut truncated = false;
    match source {
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
            let names: Vec<String> = names.into_iter().filter(|name| !is_secret(&entity, name)).collect();
            writer.header(&names).map_err(|err| refusal(err.to_string()))?;
            let mut iterator = entity_find(facade, &entity, conditions, sort, filter)?
                .iterator()
                .map_err(|err| refusal(err.to_string()))?;
            while let Some(value) = iterator.next_value().map_err(|err| refusal(err.to_string()))? {
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
        }
        Source::Sql { group, sql } => {
            let mysql = check_sql_source(facade, group, sql)?;
            let wrapped = wrap(sql, mysql, sort, filter, "")?;
            let result = sql::grid_query(facade, group, &wrapped.sql, &wrapped.params, cap, actor, "export")?;
            truncated = result.limit_reached;
            let keep: Vec<usize> = result
                .columns
                .iter()
                .enumerate()
                .filter(|(_, name)| columns.is_empty() || columns.contains(name))
                .map(|(index, _)| index)
                .collect();
            let names: Vec<String> = keep.iter().map(|index| result.columns[*index].clone()).collect();
            writer.header(&names).map_err(|err| refusal(err.to_string()))?;
            for record in &result.records {
                let cells: Vec<Value> = keep
                    .iter()
                    .map(|index| record.get(*index).cloned().unwrap_or(Value::Null))
                    .collect();
                writer.row(&cells).map_err(|err| refusal(err.to_string()))?;
            }
        }
    }
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

/// CSV into a temporary file, written to the location at the end as a stream; XLSX into an `XlsxBook`.
struct TableWriter {
    csv: Option<csv::Writer<std::fs::File>>,
    path: Option<std::path::PathBuf>,
    book: Option<moqui_core::screen::XlsxBook>,
    rows: usize,
}

impl TableWriter {
    fn new(format: ExportFormat) -> Result<Self, Error> {
        Ok(match format {
            ExportFormat::Csv => {
                let path = std::env::temp_dir().join(format!(
                    "moqui-grid-{}.csv",
                    moqui_core::entity::uuid_v4()?.replace('-', "")
                ));
                let file = std::fs::File::create(&path).map_err(|err| Error::new(format!("export file: {err}")))?;
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

    fn finish(self, location: &str) -> Result<(), Error> {
        let write = |input: &mut dyn std::io::Read, size: Option<u64>| {
            ExecutionContext::with_current(|ec| ec.factory().resource().write_from(location, input, size))
                .ok_or_else(|| Error::new("no execution context to write the export"))?
                .map_err(|err| Error::new(err.to_string()))
        };
        if let Some(mut csv) = self.csv {
            csv.flush().map_err(|err| Error::new(err.to_string()))?;
            drop(csv);
            let path = self.path.unwrap_or_default();
            let written = (|| {
                let mut file = std::fs::File::open(&path).map_err(|err| Error::new(err.to_string()))?;
                let size = file.metadata().ok().map(|meta| meta.len());
                write(&mut file, size)
            })();
            let _ = std::fs::remove_file(&path);
            return written;
        }
        let bytes = self
            .book
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
        .filter(|field| !field.encrypt)
        .map(|field| field.name.clone());
    let mut find = facade
        .find(&related.full_name)
        .map_err(|err| refusal(err.to_string()))?;
    let text = text.trim();
    if !text.is_empty() {
        let pattern = Value::String(format!("%{}%", text.to_lowercase()));
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
        .offset(offset.max(0))
        .limit(LOOKUP_SIZE)
        .list()
        .map_err(|err| refusal(err.to_string()))?;
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
    /// The row changed since the grid read it.
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
}

const CONFLICT: &str = "changed by someone else since you loaded it";

/// Save the grid's changes to `entity_name` in one transaction: each through the entity's `update#`, `create#` or
/// `delete#` (its authorization, validation, ECA rules and audit log apply). An update or delete whose row has a
/// `lastUpdatedStamp` other than the one the grid read is refused as a conflict (the row is read `for update`). A
/// refused row rolls back every other: the answer lists the refused rows with their field errors. Every row is checked
/// before any is written, so all the conflicts and field refusals show at once; a refusal of the entity-auto service
/// stops the rest. Restricted entities, views, `UserAccount` secrets and encrypted fields are refused. Writes a
/// `DbaGridSave` audit row with the entity and the counts.
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
                if let Err(refused) = apply_change(&entity, &pk_names, kind, index, change) {
                    outcome.refused.push(refused);
                    return Err(Error::new("refused row"));
                }
                match kind {
                    "update" => outcome.updated += 1,
                    "create" => outcome.created += 1,
                    _ => outcome.deleted += 1,
                }
            }
        }
        Ok(())
    });
    outcome.saved = ran.is_ok();
    if let Err(err) = &ran {
        if outcome.refused.is_empty() {
            // the commit, or a check, failed
            outcome.refused.push(RefusedRow {
                kind: "save",
                message: err.to_string(),
                ..RefusedRow::default()
            });
        }
        outcome.updated = 0;
        outcome.created = 0;
        outcome.deleted = 0;
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

/// The refusal of a change found before any is written: a missing key, a field the grid may not write, a conflict.
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
        if is_secret(entity, name) {
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
    if refused.field_errors.is_empty() && kind != "create" {
        let mut find = facade.find(&entity.full_name)?.for_update(true);
        for pk in pk_names {
            find = find.eq(pk, change.pk.get(pk).cloned().unwrap_or(Value::Null));
        }
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        match find.one()? {
            None => refused.message = "The row is gone: deleted by someone else since you loaded it".to_string(),
            Some(row) => {
                if let Some(given) = &change.stamp {
                    if entity.field(STAMP).is_some() && !stamp_matches(row.get(STAMP), given) {
                        refused.message = format!("The row was {CONFLICT}");
                        refused.conflict = true;
                    }
                }
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

/// Run one change through the entity-auto service. The errors it adds are the refusal's, taken off the messages.
fn apply_change(
    entity: &EntityDefinition,
    pk_names: &[String],
    kind: &'static str,
    index: usize,
    change: &Change,
) -> Result<(), RefusedRow> {
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
    let Err(err) = result else {
        return Ok(());
    };
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
        message: err.message().to_string(),
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

    use super::{parse_filter, parse_sort, stamp_matches, wrap, Filter, Sort};

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
            filter("description", "like", Value::string("x'; DROP")),
            filter("n", ">=", Value::I64(3)),
            filter("id", "in", Value::string("a, b")),
        ];
        let sort = vec![Sort {
            field: "n".to_string(),
            descending: true,
        }];
        let wrapped = wrap("SELECT * FROM t", false, &sort, &filters, " LIMIT 11 OFFSET 0").unwrap();
        assert_eq!(
            wrapped.sql,
            "SELECT * FROM (SELECT * FROM t) q WHERE LOWER(CAST(\"description\" AS TEXT)) LIKE LOWER(?) AND \"n\" >= ? \
             AND \"id\" IN (?, ?) ORDER BY \"n\" DESC LIMIT 11 OFFSET 0"
        );
        assert_eq!(wrapped.params[0], Value::string("%x'; DROP%"));
        assert!(wrap("SELECT 1", true, &[], &[filter("a\" OR 1=1", "=", Value::Null)], "").is_err());
        assert!(
            wrap("SELECT ? AS a", false, &[], &filters, "").is_err(),
            "a ? of its own beside filters"
        );
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
