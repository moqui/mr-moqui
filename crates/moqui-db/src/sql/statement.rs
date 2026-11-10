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

//! Insert, update, delete, select, and count on an entity, and DDL text.
//!
//! Field names are written as the field's `column-name`. A selected field whose column has
//! another name is read back under the field name with `AS`.

use std::collections::HashSet;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::dialect::Dialect;
use crate::model::{EntityDefinition, EntityModel, FieldDefinition, IndexDefinition, Relationship};

use super::condition::{fts_config, Column, Condition, FieldRef, LimitOffset, OrderBy, Writer};
use super::ident::{
    clip_length, clip_name, is_mysql, is_sqlite, physical_column_name, physical_name, placeholders, quote_column,
    quote_ident as quote_alias, quote_list, quote_physical_ident as quote_ident, shrink_name,
};
use super::Statement;

/// The row lock a select takes until the transaction ends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Lock {
    /// No lock.
    #[default]
    None,
    /// `FOR UPDATE`, or the database's `for-update` text.
    Update,
    /// `FOR NO KEY UPDATE` where the database has it (Postgres), else as [`Lock::Update`]: the database's
    /// `for-no-key-update` text, falling back to its `for-update` text. Unlike `FOR UPDATE` it does not block another
    /// transaction's foreign key check on the row (`FOR KEY SHARE`), so serializing on a parent row (an organization's
    /// `Party`) does not deadlock with inserts of its children. For a lock whose transaction changes no key column of
    /// the row.
    NoKeyUpdate,
}

impl Lock {
    /// Whether the select takes a lock at all.
    #[must_use]
    pub fn is_locking(self) -> bool {
        self != Self::None
    }

    /// Whether a row already held with this lock needs no further lock for `wanted`: a `FOR UPDATE` covers both
    /// modes, a `FOR NO KEY UPDATE` only itself.
    #[must_use]
    pub fn covers(self, wanted: Self) -> bool {
        match wanted {
            Self::None => true,
            Self::NoKeyUpdate => self.is_locking(),
            Self::Update => self == Self::Update,
        }
    }

    /// The stronger of two locks on one row.
    #[must_use]
    pub fn stronger(self, other: Self) -> Self {
        if self.covers(other) {
            self
        } else {
            other
        }
    }
}

/// What a select can ask for beyond conditions, order, and paging.
#[derive(Debug, Clone, Default)]
pub struct SelectOptions {
    pub distinct: bool,
    /// Lock the selected rows. Needs a transaction, and cannot go with `distinct` or a `GROUP BY`.
    pub lock: Lock,
    /// With a `lock`: leave out the rows another transaction holds instead of waiting for them (`SKIP LOCKED`), on
    /// the databases that have it ([`crate::dialect::Dialect::skips_locked`]); elsewhere the lock waits as the lock alone
    /// does.
    pub skip_locked: bool,
    /// A condition on an aggregate. Only a view with an aggregate alias has a `GROUP BY` for it.
    pub having: Option<Condition>,
    /// For a view: the aliases to select. Empty selects every alias. For a count with
    /// `distinct`, the fields that are distinct.
    pub fields: Vec<String>,
    /// For a view: member tables read as a transaction sees them before its held writes reach the database
    /// ([`TableShadow`]).
    pub shadows: Vec<TableShadow>,
}

/// A table as one transaction sees it while it holds writes in memory (the transaction cache): the stored rows but
/// those of `hidden` keys, and `rows` in their place. A view read renders each member table that has a shadow as
/// `(SELECT columns FROM table WHERE NOT (key of hidden ...) UNION ALL SELECT CAST(? AS type), ... )`, so the
/// database joins, filters, groups and counts the rows the transaction would see, and none of its writes is sent
/// early.
#[derive(Debug, Clone, Default)]
pub struct TableShadow {
    /// The entity's full name.
    pub entity_name: String,
    /// The primary key values of every row the transaction wrote, in the entity's primary key field order: those rows
    /// of the table are left out.
    pub hidden: Vec<Vec<Value>>,
    /// Each row the transaction's writes leave, every field in the entity's field order.
    pub rows: Vec<Vec<Value>>,
    /// The SQL type of each field, in the entity's field order ([`field_sql_type`]), for the casts of `rows`.
    pub types: Vec<String>,
}

/// `entity`'s table in a select's `FROM`, or its shadow among `shadows` as a sub-select of the same columns
/// ([`TableShadow`]).
pub(crate) fn write_table(
    out: &mut Writer<'_>,
    entity: &EntityDefinition,
    shadows: &[TableShadow],
) -> Result<(), Error> {
    let dialect = out.dialect;
    let Some(shadow) = shadows.iter().find(|shadow| shadow.entity_name == entity.full_name) else {
        out.push(&quote_ident(dialect, &entity.table_name));
        return Ok(());
    };
    let columns: Vec<String> = entity
        .fields
        .iter()
        .map(|field| quote_column(dialect, &field.column_name))
        .collect();
    let pk_columns: Vec<String> = entity
        .fields
        .iter()
        .filter(|field| field.is_pk)
        .map(|field| quote_column(dialect, &field.column_name))
        .collect();
    if shadow.types.len() != columns.len() {
        return Err(Error::new(format!(
            "a shadow of {} has {} types, the entity has {} fields",
            entity.full_name,
            shadow.types.len(),
            columns.len()
        )));
    }
    out.push("(SELECT ");
    out.push(&columns.join(", "));
    out.push(" FROM ");
    out.push(&quote_ident(dialect, &entity.table_name));
    let pk_types: Vec<&str> = entity
        .fields
        .iter()
        .zip(&shadow.types)
        .filter(|(field, _)| field.is_pk)
        .map(|(_, sql_type)| sql_type.as_str())
        .collect();
    // Past a few rows, on Postgres, each column goes as one array, unnested: the statement's text and its parameter
    // count stay the same however many rows the transaction holds.
    let arrays = dialect.name.starts_with("postgres")
        && (shadow.rows.len() > SHADOW_ARRAY_AT || shadow.hidden.len() > SHADOW_ARRAY_AT);
    if arrays && !shadow.hidden.is_empty() {
        out.push(" WHERE (");
        out.push(&pk_columns.join(", "));
        out.push(") NOT IN (SELECT * FROM ");
        let keys: Vec<Vec<&Value>> = (0..pk_columns.len())
            .map(|at| shadow.hidden.iter().filter_map(|key| key.get(at)).collect())
            .collect();
        unnest(out, &keys, &pk_types)?;
        out.push(")");
    } else if !shadow.hidden.is_empty() {
        out.push(" WHERE NOT (");
        for (index, key) in shadow.hidden.iter().enumerate() {
            if key.len() != pk_columns.len() {
                return Err(Error::new(format!(
                    "a shadow of {} hides a key of {} values, the entity has {}",
                    entity.full_name,
                    key.len(),
                    pk_columns.len()
                )));
            }
            out.push(if index == 0 { "(" } else { " OR (" });
            for (at, (column, value)) in pk_columns.iter().zip(key).enumerate() {
                if at > 0 {
                    out.push(" AND ");
                }
                out.push(column);
                out.push(" = ");
                out.bind(value.clone());
            }
            out.push(")");
        }
        out.push(")");
    }
    if arrays && !shadow.rows.is_empty() {
        out.push(" UNION ALL SELECT * FROM ");
        let columns_of: Vec<Vec<&Value>> = (0..columns.len())
            .map(|at| shadow.rows.iter().filter_map(|row| row.get(at)).collect())
            .collect();
        let types: Vec<&str> = shadow.types.iter().map(String::as_str).collect();
        unnest(out, &columns_of, &types)?;
    }
    for row in shadow.rows.iter().filter(|_| !arrays) {
        if row.len() != columns.len() {
            return Err(Error::new(format!(
                "a shadow row of {} has {} values, the entity has {} fields",
                entity.full_name,
                row.len(),
                columns.len()
            )));
        }
        out.push(" UNION ALL SELECT ");
        for (at, (value, sql_type)) in row.iter().zip(&shadow.types).enumerate() {
            if at > 0 {
                out.push(", ");
            }
            out.push("CAST(");
            out.bind(value.clone());
            out.push(" AS ");
            out.push(sql_type);
            out.push(")");
        }
    }
    out.push(")");
    Ok(())
}

/// Rows of a shadow past which, on Postgres, it is written as arrays ([`write_table`]).
const SHADOW_ARRAY_AT: usize = 16;

/// `unnest(CAST(? AS type[]), ...) AS u`: one array parameter per column, `columns[i]` its values.
fn unnest(out: &mut Writer<'_>, columns: &[Vec<&Value>], types: &[&str]) -> Result<(), Error> {
    out.push("unnest(");
    for (at, (values, sql_type)) in columns.iter().zip(types).enumerate() {
        if at > 0 {
            out.push(", ");
        }
        out.push("CAST(");
        out.bind(Value::string(array_literal(values)?));
        out.push(" AS ");
        out.push(sql_type);
        out.push("[])");
    }
    out.push(") AS shadow_rows");
    Ok(())
}

/// A Postgres array literal of `values`, each as its text parameter is sent (`NULL` for a null), quoted.
fn array_literal(values: &[&Value]) -> Result<String, Error> {
    let mut text = String::from("{");
    for (at, value) in values.iter().enumerate() {
        if at > 0 {
            text.push(',');
        }
        let element = match value {
            Value::Null => {
                text.push_str("NULL");
                continue;
            }
            Value::String(string) => string.to_string(),
            Value::Bool(flag) => flag.to_string(),
            Value::I32(number) => number.to_string(),
            Value::I64(number) => number.to_string(),
            Value::F64(number) if number.is_nan() => "NaN".to_string(),
            Value::F64(number) if number.is_infinite() && number.is_sign_negative() => "-Infinity".to_string(),
            Value::F64(number) if number.is_infinite() => "Infinity".to_string(),
            Value::F64(number) => number.to_string(),
            Value::Decimal(number) => number.to_string(),
            Value::Timestamp(stamp) => stamp.to_string(),
            Value::Date(date) => date.to_string(),
            Value::Time(time) => time.to_string(),
            Value::Bytes(bytes) => {
                let mut hex = String::with_capacity(2 + bytes.len() * 2);
                hex.push_str("\\x");
                for byte in bytes.iter() {
                    hex.push_str(&format!("{byte:02x}"));
                }
                hex
            }
            Value::List(_) | Value::Map(_) | Value::Object(_) => {
                return Err(Error::new("a shadow row value must be a scalar"));
            }
        };
        text.push('"');
        for ch in element.chars() {
            if ch == '"' || ch == '\\' {
                text.push('\\');
            }
            text.push(ch);
        }
        text.push('"');
    }
    text.push('}');
    Ok(text)
}

/// [`write_table`] in a single-entity select: a shadow takes the table's name as its alias, so the columns read as
/// the table's.
fn write_from(out: &mut Writer<'_>, entity: &EntityDefinition, shadows: &[TableShadow]) -> Result<(), Error> {
    let shadowed = shadows.iter().any(|shadow| shadow.entity_name == entity.full_name);
    write_table(out, entity, shadows)?;
    if shadowed {
        out.push(" ");
        out.push(&quote_ident(out.dialect, &entity.table_name));
    }
    Ok(())
}

/// The column a field name maps to on a plain entity, and whether it is text.
pub(crate) fn entity_column(entity: &EntityDefinition, dialect: &Dialect, field: &FieldRef) -> Column {
    if field.expr.is_some() || field.entity_alias.is_some() {
        return Column::plain(dialect, field);
    }
    match entity.field(&field.name) {
        Some(definition) => Column {
            sql: quote_column(dialect, &definition.column_name),
            text: Some(definition.is_text()),
            java_type: entity.java_type_of(&field.name).map(str::to_string),
            fts: (definition.field_type == FTS_TYPE)
                .then(|| quote_ident(dialect, &fts_column_name(definition, dialect))),
        },
        None => Column::plain(dialect, field),
    }
}

fn column_name<'a>(entity: &'a EntityDefinition, name: &'a str) -> &'a str {
    entity.field(name).map_or(name, |field| field.column_name.as_str())
}

/// `INSERT INTO table (columns) VALUES (?, ?)`. The caller chooses the fields and binds them.
pub fn insert(entity: &EntityDefinition, dialect: &Dialect, columns: &[&str]) -> Result<String, Error> {
    if columns.is_empty() {
        return Err(Error::new("insert has no columns"));
    }
    let names: Vec<&str> = columns.iter().map(|name| column_name(entity, name)).collect();
    let mut sql = String::new();
    sql.push_str("INSERT INTO ");
    sql.push_str(&quote_ident(dialect, &entity.table_name));
    sql.push_str(" (");
    sql.push_str(&quote_list(dialect, &names));
    sql.push_str(") VALUES (");
    sql.push_str(&placeholders(dialect, columns.len()));
    sql.push(')');
    Ok(sql)
}

/// `UPDATE table SET col = ? WHERE ...`. The caller binds the `SET` values, then the statement's
/// params. An empty condition omits `WHERE`.
pub fn update(
    entity: &EntityDefinition,
    dialect: &Dialect,
    columns: &[&str],
    condition: Option<&Condition>,
) -> Result<Statement, Error> {
    if columns.is_empty() {
        return Err(Error::new("update has no columns"));
    }
    let mut out = Writer::new(dialect);
    out.push("UPDATE ");
    out.push(&quote_ident(dialect, &entity.table_name));
    out.push(" SET ");
    for (index, column) in columns.iter().enumerate() {
        if index > 0 {
            out.push(", ");
        }
        out.push(&quote_column(dialect, column_name(entity, column)));
        out.push(" = ");
        out.push(dialect.placeholder);
    }
    where_clause(&mut out, entity, condition)?;
    Ok(out.finish())
}

/// `DELETE FROM table WHERE ...`. An empty condition omits `WHERE`.
pub fn delete(entity: &EntityDefinition, dialect: &Dialect, condition: Option<&Condition>) -> Result<Statement, Error> {
    let mut out = Writer::new(dialect);
    out.push("DELETE FROM ");
    out.push(&quote_ident(dialect, &entity.table_name));
    where_clause(&mut out, entity, condition)?;
    Ok(out.finish())
}

/// `SELECT fields FROM table WHERE ... ORDER BY ...` plus limit and offset.
/// An empty `fields` list selects every field. Schema is omitted.
pub fn select(
    entity: &EntityDefinition,
    dialect: &Dialect,
    fields: &[FieldRef],
    condition: Option<&Condition>,
    order_by: &[OrderBy],
    page: LimitOffset,
) -> Result<Statement, Error> {
    select_with(
        entity,
        dialect,
        fields,
        condition,
        order_by,
        page,
        &SelectOptions::default(),
    )
}

/// [`select`] with [`SelectOptions`]. `options.fields` is not used; `fields` chooses the columns.
pub fn select_with(
    entity: &EntityDefinition,
    dialect: &Dialect,
    fields: &[FieldRef],
    condition: Option<&Condition>,
    order_by: &[OrderBy],
    page: LimitOffset,
    options: &SelectOptions,
) -> Result<Statement, Error> {
    if entity.is_view {
        return Err(Error::new(format!(
            "{} is a view-entity; use select_view",
            entity.full_name
        )));
    }
    if options.having.is_some() {
        return Err(Error::new(format!(
            "{} has no aggregate fields, so a having condition has nothing to apply to",
            entity.full_name
        )));
    }
    if options.lock.is_locking() && options.distinct {
        return Err(Error::new("a select cannot be both distinct and for update"));
    }
    let mut out = Writer::new(dialect);
    out.push(if options.distinct {
        "SELECT DISTINCT "
    } else {
        "SELECT "
    });
    // A distinct select can only be ordered by what it selects, so the order-by fields are selected too, as Java's
    // EntityFindBase adds them.
    if options.distinct && !fields.is_empty() {
        let mut selected: Vec<FieldRef> = fields.to_vec();
        for order in order_by {
            if order.field.expr.is_none()
                && order.field.entity_alias.is_none()
                && !selected.iter().any(|field| field.name == order.field.name)
            {
                selected.push(FieldRef::new(&order.field.name));
            }
        }
        out.push(&select_list(entity, dialect, &selected));
    } else {
        out.push(&select_list(entity, dialect, fields));
    }
    out.push(" FROM ");
    write_from(&mut out, entity, &options.shadows)?;
    select_tail(out, entity, dialect, condition, order_by, page, options)
}

/// `SELECT <fields> FROM <table>` of a select that is not distinct, the part of [`select_with`] that does not change
/// with the condition. Empty `fields` selects every field. For [`select_with_head`].
pub fn select_head(entity: &EntityDefinition, dialect: &Dialect, fields: &[FieldRef]) -> String {
    let mut head = String::from("SELECT ");
    head.push_str(&select_list(entity, dialect, fields));
    head.push_str(" FROM ");
    head.push_str(&quote_ident(dialect, &entity.table_name));
    head
}

/// [`select_with`] after a [`select_head`] of the same entity and dialect, so a find that renders the same columns again
/// does not render them again. Not for a distinct select, whose head can change with the order.
pub fn select_with_head(
    head: &str,
    entity: &EntityDefinition,
    dialect: &Dialect,
    condition: Option<&Condition>,
    order_by: &[OrderBy],
    page: LimitOffset,
    options: &SelectOptions,
) -> Result<Statement, Error> {
    if options.distinct {
        return Err(Error::new("a select head is not distinct; use select_with"));
    }
    if options.having.is_some() {
        return Err(Error::new(format!(
            "{} has no aggregate fields, so a having condition has nothing to apply to",
            entity.full_name
        )));
    }
    let mut out = Writer::new(dialect);
    out.push(head);
    select_tail(out, entity, dialect, condition, order_by, page, options)
}

fn select_tail(
    mut out: Writer<'_>,
    entity: &EntityDefinition,
    dialect: &Dialect,
    condition: Option<&Condition>,
    order_by: &[OrderBy],
    page: LimitOffset,
    options: &SelectOptions,
) -> Result<Statement, Error> {
    where_clause(&mut out, entity, condition)?;
    let columns = |field: &FieldRef| Ok(entity_column(entity, dialect, field));
    out.order_by(order_by, page.is_set(), &columns)?;
    out.page(page);
    lock_rows(&mut out, options.lock);
    if options.lock.is_locking() && options.skip_locked && out.dialect.skips_locked_with(options.lock) {
        out.push(" SKIP LOCKED");
    }
    Ok(out.finish())
}

/// The database's text for `lock` ([`Dialect::lock_text`]), when it has one.
pub(crate) fn lock_rows(out: &mut Writer<'_>, lock: Lock) {
    // the dialect outlives the writer's borrow, so its text is pushed as it is, not copied first
    let dialect = out.dialect;
    let text = dialect.lock_text(lock);
    if text.is_empty() {
        return;
    }
    out.push(" ");
    out.push(text);
}

fn select_list(entity: &EntityDefinition, dialect: &Dialect, fields: &[FieldRef]) -> String {
    let all: Vec<FieldRef>;
    let fields = if fields.is_empty() {
        // Not with a `text-fts` field: its table has the generated full-text column, which is no field.
        if entity.fields.iter().all(|field| {
            physical_column_name(dialect, &field.column_name) == field.name && field.field_type != FTS_TYPE
        }) {
            return "*".to_string();
        }
        all = entity.fields.iter().map(|field| FieldRef::new(&field.name)).collect();
        &all
    } else {
        fields
    };
    let mut sql = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        let column = entity_column(entity, dialect, field);
        sql.push_str(&column.sql);
        if field.expr.is_none()
            && field.entity_alias.is_none()
            && physical_column_name(dialect, column_name(entity, &field.name)) != field.name
        {
            sql.push_str(" AS ");
            sql.push_str(&quote_alias(dialect, &field.name));
        }
    }
    sql
}

/// `SELECT COUNT(*) FROM table WHERE ...`. No `ORDER BY` and no limit.
pub fn count(entity: &EntityDefinition, dialect: &Dialect, condition: Option<&Condition>) -> Result<Statement, Error> {
    count_with(entity, dialect, condition, &SelectOptions::default())
}

/// [`count`] with [`SelectOptions`]. A distinct count counts a `SELECT DISTINCT` sub-select of
/// `options.fields`, or of every field when none are given, as Java does.
pub fn count_with(
    entity: &EntityDefinition,
    dialect: &Dialect,
    condition: Option<&Condition>,
    options: &SelectOptions,
) -> Result<Statement, Error> {
    if entity.is_view {
        return Err(Error::new(format!(
            "{} is a view-entity; use count_view",
            entity.full_name
        )));
    }
    let mut out = Writer::new(dialect);
    // As Java's `makeCountFunction`: a distinct count counts the distinct rows of the selected fields, all of them when
    // none are selected.
    let wrapped = options.distinct;
    if wrapped {
        let fields: Vec<FieldRef> = options.fields.iter().map(FieldRef::new).collect();
        out.push("SELECT COUNT(*) FROM (SELECT DISTINCT ");
        out.push(&select_list(entity, dialect, &fields));
    } else {
        out.push("SELECT COUNT(*)");
    }
    out.push(" FROM ");
    write_from(&mut out, entity, &options.shadows)?;
    where_clause(&mut out, entity, condition)?;
    if wrapped {
        out.push(") TEMP_NAME");
    }
    Ok(out.finish())
}

fn where_clause(out: &mut Writer<'_>, entity: &EntityDefinition, condition: Option<&Condition>) -> Result<(), Error> {
    let Some(condition) = condition else {
        return Ok(());
    };
    let dialect = out.dialect;
    let columns = |field: &FieldRef| Ok(entity_column(entity, dialect, field));
    let mut inner = Writer::new(dialect);
    inner.condition(condition, &columns)?;
    let rendered = inner.finish();
    if !rendered.sql.is_empty() {
        out.push(" WHERE ");
        out.append(rendered);
    }
    Ok(())
}

/// Whether [`insert_or_update`] writes one statement for `dialect`: Postgres and SQLite (`ON CONFLICT`) and MySQL
/// (`ON DUPLICATE KEY UPDATE`).
pub fn supports_upsert(dialect: &Dialect) -> bool {
    dialect.name == "postgres" || is_mysql(dialect) || is_sqlite(dialect)
}

/// Insert, or update the given columns on a primary key conflict, in one statement. Postgres and SQLite write
/// `ON CONFLICT (pk) DO UPDATE SET`, MySQL `ON DUPLICATE KEY UPDATE`; see [`supports_upsert`].
pub fn insert_or_update(entity: &EntityDefinition, dialect: &Dialect, columns: &[&str]) -> Result<String, Error> {
    if !supports_upsert(dialect) {
        return Err(Error::new(format!("database {} has no upsert statement", dialect.name)));
    }
    let Some(&first_column) = columns.first() else {
        return Err(Error::new("insert_or_update has no columns"));
    };
    let mut sql = insert(entity, dialect, columns)?;
    let pk = entity.pk_field_names();
    let pk_owned: HashSet<&str> = pk.iter().copied().collect();
    let updates: Vec<&str> = columns
        .iter()
        .copied()
        .filter(|column| !pk_owned.contains(column))
        .map(|column| column_name(entity, column))
        .collect();
    if is_mysql(dialect) {
        sql.push_str(" ON DUPLICATE KEY UPDATE ");
        let targets: Vec<&str> = if updates.is_empty() {
            vec![column_name(entity, first_column)]
        } else {
            updates
        };
        for (index, column) in targets.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            let quoted = quote_column(dialect, column);
            sql.push_str(&quoted);
            sql.push_str(" = VALUES(");
            sql.push_str(&quoted);
            sql.push(')');
        }
        return Ok(sql);
    }
    if pk.is_empty() {
        return Err(Error::new(format!(
            "insert_or_update needs a primary key on {}",
            entity.full_name
        )));
    }
    let pk_columns: Vec<&str> = pk.iter().map(|name| column_name(entity, name)).collect();
    sql.push_str(" ON CONFLICT (");
    sql.push_str(&quote_list(dialect, &pk_columns));
    sql.push(')');
    if updates.is_empty() {
        sql.push_str(" DO NOTHING");
        return Ok(sql);
    }
    sql.push_str(" DO UPDATE SET ");
    for (index, column) in updates.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        let quoted = quote_column(dialect, column);
        sql.push_str(&quoted);
        sql.push_str(" = EXCLUDED.");
        sql.push_str(&quoted);
    }
    Ok(sql)
}

/// Whether [`insert_if_missing`] writes one statement for `dialect`: the databases of [`supports_upsert`].
pub fn supports_insert_if_missing(dialect: &Dialect) -> bool {
    supports_upsert(dialect)
}

/// Insert, or do nothing when a row with the same primary key is there, in one statement that does not fail on that
/// key, so a create-or-update that found no row and races another for the same key loses without aborting its
/// transaction. The statement affects one row when it inserted and none when the row was there. Postgres and SQLite
/// write `ON CONFLICT (pk) DO NOTHING`; MySQL `ON DUPLICATE KEY UPDATE pk = pk`, which counts no row when it changes
/// none (the driver does not set `CLIENT_FOUND_ROWS`), but also matches another unique key. See
/// [`supports_insert_if_missing`].
pub fn insert_if_missing(entity: &EntityDefinition, dialect: &Dialect, columns: &[&str]) -> Result<String, Error> {
    if !supports_insert_if_missing(dialect) {
        return Err(Error::new(format!(
            "database {} has no insert-if-missing statement",
            dialect.name
        )));
    }
    let pk = entity.pk_field_names();
    if pk.is_empty() {
        return Err(Error::new(format!(
            "insert_if_missing needs a primary key on {}",
            entity.full_name
        )));
    }
    let mut sql = insert(entity, dialect, columns)?;
    let pk_columns: Vec<&str> = pk.iter().map(|name| column_name(entity, name)).collect();
    if is_mysql(dialect) {
        let quoted = quote_column(dialect, pk_columns[0]);
        sql.push_str(" ON DUPLICATE KEY UPDATE ");
        sql.push_str(&quoted);
        sql.push_str(" = ");
        sql.push_str(&quoted);
        return Ok(sql);
    }
    sql.push_str(" ON CONFLICT (");
    sql.push_str(&quote_list(dialect, &pk_columns));
    sql.push_str(") DO NOTHING");
    Ok(sql)
}

/// `CREATE TABLE`, plus the full-text column and index of each `text-fts` field ([`fts_index_sql`]) when the
/// profile is `postgres-tsvector`. MySQL puts `FULLTEXT` on the table and appends `ENGINE`, character set, and
/// collate when the profile sets them. Primary key columns are `NOT NULL`, and the key constraint is `PK_` plus the
/// table name, clipped.
pub fn create_table(entity: &EntityDefinition, model: &EntityModel, dialect: &Dialect) -> Result<String, Error> {
    if entity.is_view {
        return Err(Error::new(format!(
            "cannot create a table for view-entity {}",
            entity.full_name
        )));
    }
    if entity.fields.is_empty() {
        return Err(Error::new(format!("entity {} has no fields", entity.full_name)));
    }
    let mut sql = String::from("CREATE TABLE ");
    sql.push_str(&quote_ident(dialect, &entity.table_name));
    sql.push_str(" (");
    for (index, field) in entity.fields.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        write_column(&mut sql, model, dialect, field, true)?;
    }
    let pk = entity.pk_field_names();
    if !pk.is_empty() {
        sql.push_str(", ");
        if dialect.legacy_names || dialect.use_pk_constraint_names {
            sql.push_str("CONSTRAINT ");
            let pk_name = clip_name(&format!("PK_{}", entity.table_name), clip_length(dialect));
            sql.push_str(&quote_ident(dialect, &pk_name));
            sql.push(' ');
        }
        sql.push_str("PRIMARY KEY (");
        let pk_columns: Vec<&str> = pk.iter().map(|name| column_name(entity, name)).collect();
        sql.push_str(&quote_list(dialect, &pk_columns));
        sql.push(')');
    }
    if is_mysql(dialect) && dialect.fts_ddl && dialect.fts_style.as_deref() == Some("mysql-fulltext") {
        for field in entity.fields.iter().filter(|field| field.field_type == FTS_TYPE) {
            sql.push_str(", FULLTEXT INDEX ");
            sql.push_str(&quote_ident(dialect, &fts_index_name(entity, field, dialect)));
            sql.push_str(" (");
            sql.push_str(&quote_column(dialect, &field.column_name));
            sql.push(')');
        }
    }
    sql.push(')');
    if let Some(engine) = dialect.table_engine.as_deref() {
        sql.push_str(" ENGINE=");
        sql.push_str(engine);
    }
    if let Some(character_set) = dialect.character_set.as_deref() {
        sql.push_str(" CHARACTER SET ");
        sql.push_str(character_set);
    }
    if let Some(collate) = dialect.collate.as_deref() {
        sql.push_str(" COLLATE ");
        sql.push_str(collate);
    }
    if dialect.fts_style.as_deref() == Some("postgres-tsvector") {
        for field in entity.fields.iter().filter(|field| field.field_type == FTS_TYPE) {
            for statement in fts_index_sql(entity, field, dialect)? {
                sql.push_str(";\n");
                sql.push_str(&statement);
            }
        }
    }
    Ok(sql)
}

/// The field type that has a full-text index.
pub(crate) const FTS_TYPE: &str = "text-fts";

/// The statements that add the full-text column and index of a `text-fts` field to an existing table, as Java's
/// `FtsDdl`: on Postgres the generated `{column}_tsv` column and a GIN index on it, on MySQL a `FULLTEXT` index. Both
/// are named by [`fts_index_name`]. Empty when the profile's `fts-style` has no index of its own or `fts-ddl` is false.
pub fn fts_index_sql(
    entity: &EntityDefinition,
    field: &FieldDefinition,
    dialect: &Dialect,
) -> Result<Vec<String>, Error> {
    if !dialect.fts_ddl {
        return Ok(Vec::new());
    }
    let index_name = quote_ident(dialect, &fts_index_name(entity, field, dialect));
    let table = quote_ident(dialect, &entity.table_name);
    let column = quote_column(dialect, &field.column_name);
    match dialect.fts_style.as_deref() {
        Some("postgres-tsvector") => {
            let config = fts_config(dialect)?;
            let tsv = quote_ident(dialect, &fts_column_name(field, dialect));
            Ok(vec![
                format!(
                    "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {tsv} tsvector GENERATED ALWAYS AS \
                     (to_tsvector('{config}', coalesce({column}, ''))) STORED"
                ),
                format!("CREATE INDEX IF NOT EXISTS {index_name} ON {table} USING GIN ({tsv})"),
            ])
        }
        Some("mysql-fulltext") => Ok(vec![format!(
            "ALTER TABLE {table} ADD FULLTEXT INDEX {index_name} ({column})"
        )]),
        _ => Ok(Vec::new()),
    }
}

/// Java's `FtsDdl` index name, as the database stores it: `fts_{table}_{column}` on Postgres and `{table}_{column}_FTS`
/// on MySQL, characters other than letters, digits and `_` as `_`, clipped to 60.
pub fn fts_index_name(entity: &EntityDefinition, field: &FieldDefinition, dialect: &Dialect) -> String {
    let column = physical_column_name(dialect, &field.column_name);
    let name = if is_mysql(dialect) {
        format!("{}_{column}_FTS", entity.table_name)
    } else {
        format!("fts_{}_{column}", entity.table_name)
    };
    let clipped: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    physical_name(dialect, &clipped)
}

/// The full-text index name this port used before it followed Java's `FtsDdl`: `{table}_{field}_FTS`, clipped to the
/// constraint name length. On Postgres it is an expression index with no generated column.
pub fn fts_old_index_name(entity: &EntityDefinition, field: &FieldDefinition, dialect: &Dialect) -> String {
    physical_name(
        dialect,
        &clip_name(
            &format!("{}_{}_FTS", entity.table_name, field.name),
            clip_length(dialect),
        ),
    )
}

/// The statement that drops a full-text index named `name` (as [`fts_old_index_name`] gives it) from the table.
pub fn drop_fts_index(entity: &EntityDefinition, name: &str, dialect: &Dialect) -> String {
    if is_mysql(dialect) {
        format!(
            "ALTER TABLE {} DROP INDEX {}",
            quote_ident(dialect, &entity.table_name),
            quote_alias(dialect, name)
        )
    } else {
        format!("DROP INDEX IF EXISTS {}", quote_alias(dialect, name))
    }
}

/// The generated `tsvector` column of a `text-fts` field on Postgres, Java's `{column}_tsv`.
pub fn fts_column_name(field: &FieldDefinition, dialect: &Dialect) -> String {
    format!("{}_tsv", physical_column_name(dialect, &field.column_name))
}

/// `CREATE [UNIQUE] INDEX name ON table (fields)`. The name is written as defined, as Java does.
pub fn create_index(entity: &EntityDefinition, index: &IndexDefinition, dialect: &Dialect) -> Result<String, Error> {
    if index.fields.is_empty() {
        return Err(Error::new(format!("index {} has no fields", index.name)));
    }
    let mut sql = String::from("CREATE ");
    // As Java's `createIndexes`: `use-indexes-unique="false"` makes a unique index a plain one.
    if index.unique && dialect.use_indexes_unique {
        sql.push_str("UNIQUE ");
        if dialect.use_indexes_unique_where_not_null {
            sql.push_str("WHERE NOT NULL ");
        }
    }
    sql.push_str("INDEX ");
    sql.push_str(&quote_ident(dialect, &index.name));
    sql.push_str(" ON ");
    sql.push_str(&quote_ident(dialect, &entity.table_name));
    sql.push_str(" (");
    let columns: Vec<&str> = index.fields.iter().map(|name| column_name(entity, name)).collect();
    sql.push_str(&quote_list(dialect, &columns));
    sql.push(')');
    Ok(sql)
}

fn require_foreign_key(entity: &EntityDefinition, relationship: &Relationship) -> Result<(), Error> {
    if relationship.is_auto_reverse || relationship.relationship_type != "one" {
        return Err(Error::new(format!(
            "foreign key requires a type=one relationship that is not auto-reverse, found {} on {}",
            relationship.relationship_type, entity.full_name
        )));
    }
    if relationship.key_pairs.is_empty() {
        return Err(Error::new(format!(
            "relationship {} on {} has no key fields",
            relationship.name, entity.full_name
        )));
    }
    Ok(())
}

/// `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY ... REFERENCES ...`, `INITIALLY DEFERRED` under
/// `use-fk-initially-deferred`.
///
/// Only a `type=one` relationship that is not `is-auto-reverse`. The keys are the relationship's
/// expanded key map. The name is [`foreign_key_name`].
///
/// `fk-style="name_fk"` (MySQL) is not followed: Java writes `ADD FOREIGN KEY name (...)` there, and MySQL 8.0.16 and
/// later ignore that name and call the constraint `{table}_ibfk_N`. The check that skips a foreign key already there
/// goes by this name, so it would add the key again on each check.
pub fn create_foreign_key(
    entity: &EntityDefinition,
    relationship: &Relationship,
    related: &EntityDefinition,
    dialect: &Dialect,
) -> Result<String, Error> {
    require_foreign_key(entity, relationship)?;
    let mut sql = String::from("ALTER TABLE ");
    sql.push_str(&quote_ident(dialect, &entity.table_name));
    sql.push_str(" ADD CONSTRAINT ");
    sql.push_str(&quote_ident(dialect, &foreign_key_name(entity, relationship, dialect)));
    sql.push_str(" FOREIGN KEY (");
    let sources: Vec<&str> = relationship
        .key_pairs
        .iter()
        .map(|(source, _)| column_name(entity, source))
        .collect();
    let targets: Vec<&str> = relationship
        .key_pairs
        .iter()
        .map(|(_, target)| column_name(related, target))
        .collect();
    sql.push_str(&quote_list(dialect, &sources));
    sql.push_str(") REFERENCES ");
    sql.push_str(&quote_ident(dialect, &related.table_name));
    sql.push_str(" (");
    sql.push_str(&quote_list(dialect, &targets));
    sql.push(')');
    if dialect.use_fk_initially_deferred {
        sql.push_str(" INITIALLY DEFERRED");
    }
    Ok(sql)
}

/// `CREATE INDEX` on the key fields of a `type=one` relationship, as Java `createIndexes` does
/// for every foreign key. Postgres does not index a foreign key on its own.
pub fn create_fk_index(
    entity: &EntityDefinition,
    relationship: &Relationship,
    dialect: &Dialect,
) -> Result<String, Error> {
    require_foreign_key(entity, relationship)?;
    let mut sql = String::from("CREATE INDEX ");
    sql.push_str(&quote_ident(
        dialect,
        &foreign_key_index_name(entity, relationship, dialect),
    ));
    sql.push_str(" ON ");
    sql.push_str(&quote_ident(dialect, &entity.table_name));
    sql.push_str(" (");
    let columns: Vec<&str> = relationship
        .key_pairs
        .iter()
        .map(|(source, _)| column_name(entity, source))
        .collect();
    sql.push_str(&quote_list(dialect, &columns));
    sql.push(')');
    Ok(sql)
}

/// `ALTER TABLE ... ADD column type`.
pub fn add_column(
    entity: &EntityDefinition,
    field: &FieldDefinition,
    model: &EntityModel,
    dialect: &Dialect,
) -> Result<String, Error> {
    if entity.is_view {
        return Err(Error::new(format!(
            "cannot add a column on view-entity {}",
            entity.full_name
        )));
    }
    let mut sql = String::from("ALTER TABLE ");
    sql.push_str(&quote_ident(dialect, &entity.table_name));
    sql.push_str(" ADD ");
    write_column(&mut sql, model, dialect, field, false)?;
    Ok(sql)
}

/// Java `makeFkConstraintName`: `fk-name`, or the entity name, the title (sharing a prefix with
/// the entity name, the shared part is reduced to its capitals), and the related entity name,
/// shrunk to the clip length.
pub fn foreign_key_name(entity: &EntityDefinition, relationship: &Relationship, dialect: &Dialect) -> String {
    let name = match relationship.fk_name.as_deref() {
        Some(fk_name) => fk_name.to_string(),
        None => {
            let title: Vec<char> = relationship.title.as_deref().unwrap_or("").chars().collect();
            let common = common_prefix(&title, &entity.entity_name.chars().collect::<Vec<_>>());
            let mut name = entity.entity_name.clone();
            if common > 0 {
                name.extend(title[..common].iter().filter(|ch| ch.is_uppercase()));
                name.extend(&title[common..]);
            } else {
                name.extend(&title);
            }
            name.push_str(&relationship.related_entity_name);
            name
        }
    };
    shrink_name(&name, clip_length(dialect))
}

/// Java `makeFkIndexName`: like [`foreign_key_name`], with the related name's end that repeats
/// the entity name's end reduced to its capitals, shrunk to three less than the clip length, and
/// `IDX` in front.
pub fn foreign_key_index_name(entity: &EntityDefinition, relationship: &Relationship, dialect: &Dialect) -> String {
    let name = match relationship.fk_name.as_deref() {
        Some(fk_name) => fk_name.to_string(),
        None => {
            let title: Vec<char> = relationship.title.as_deref().unwrap_or("").chars().collect();
            let own: Vec<char> = entity.entity_name.chars().collect();
            let related: Vec<char> = relationship.related_entity_name.chars().collect();
            let common = common_prefix(&title, &own);
            let related_length = related.len();
            let mut end = related_length.saturating_sub(1);
            while end > 0 && own.len() > end && related[end] == own[own.len() - (related_length - end)] {
                end -= 1;
            }
            let mut name = entity.entity_name.clone();
            if common > 0 {
                name.extend(title[..common].iter().filter(|ch| ch.is_uppercase()));
                name.extend(&title[common..]);
            } else {
                name.extend(&title);
            }
            if !related.is_empty() {
                name.extend(&related[..=end]);
                if end + 1 < related_length {
                    name.extend(related[end + 1..].iter().filter(|ch| ch.is_uppercase()));
                }
            }
            name
        }
    };
    let mut out = String::from("IDX");
    out.push_str(&shrink_name(&name, clip_length(dialect).saturating_sub(3)));
    out
}

fn common_prefix(left: &[char], right: &[char]) -> usize {
    left.iter().zip(right).take_while(|(a, b)| a == b).count()
}

fn write_column(
    sql: &mut String,
    model: &EntityModel,
    dialect: &Dialect,
    field: &FieldDefinition,
    in_create: bool,
) -> Result<(), Error> {
    let (sql_type, java_type) = column_type(model, dialect, field)?;
    sql.push_str(&quote_column(dialect, &field.column_name));
    sql.push(' ');
    sql.push_str(&sql_type);
    if java_type.ends_with("String") {
        if let Some(character_set) = dialect.character_set.as_deref() {
            sql.push_str(" CHARACTER SET ");
            sql.push_str(character_set);
        }
        if let Some(collate) = dialect.collate.as_deref() {
            sql.push_str(" COLLATE ");
            sql.push_str(collate);
        }
    }
    if in_create && (field.is_pk || field.not_null) {
        if dialect.always_use_constraint_keyword {
            sql.push_str(" CONSTRAINT");
        }
        sql.push_str(" NOT NULL");
    }
    Ok(())
}

/// The SQL type [`create_table`] and [`add_column`] give the field's column: the dialect's `database-type`, else the
/// dictionary type's `default-sql-type`.
///
/// # Errors
///
/// Returns an error when the field's type is not in the dictionary.
pub fn field_sql_type(model: &EntityModel, dialect: &Dialect, field: &FieldDefinition) -> Result<String, Error> {
    column_type(model, dialect, field).map(|(sql_type, _)| sql_type)
}

fn column_type(model: &EntityModel, dialect: &Dialect, field: &FieldDefinition) -> Result<(String, String), Error> {
    let dictionary = model.dictionary_type(&field.field_type).ok_or_else(|| {
        Error::new(format!(
            "no dictionary type {} for field {}",
            field.field_type, field.name
        ))
    })?;
    let sql_type = dialect
        .sql_type(&field.field_type)
        .unwrap_or(dictionary.default_sql_type.as_str())
        .to_string();
    Ok((sql_type, dictionary.java_type.clone()))
}
