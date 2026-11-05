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

//! The catalog queries of the schema comparison (`EntityDbMeta::compare`), per dialect, beside the DDL builders whose
//! names and types they are compared with (plans/Tools-Database.md 4.2, 5.1): the tables' columns with their types,
//! the indexes and the foreign keys of the current schema, each in one query for the whole schema. Also the lookups of
//! a SQL result's column origins (9.5). Each query's columns are aliased in lower case, so a row is read by name on
//! every database.

use moqui_lib::object::Value;

use super::ident::is_mysql;
use crate::dialect::Dialect;

/// The three catalog queries of one database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaCatalogSql {
    /// `table_name`, `column_name`, `column_type`; a table with no columns has one row with null column.
    pub columns: &'static str,
    /// `table_name`, `index_name`, `is_unique`, `is_primary`, `is_valid`, `is_constraint` (the index behind a primary
    /// key, unique or exclusion constraint), `column_names` (comma separated, in index order; an expression is left out).
    pub indexes: &'static str,
    /// `table_name`, `constraint_name`, `referenced_table`, `is_valid`, `column_names` (comma separated, in key order).
    pub foreign_keys: &'static str,
}

pub const POSTGRES_COLUMNS: &str = "SELECT c.relname AS table_name, a.attname AS column_name, \
     format_type(a.atttypid, a.atttypmod) AS column_type \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
     WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') \
     ORDER BY c.relname, a.attnum";

pub const POSTGRES_INDEXES: &str = "SELECT t.relname AS table_name, i.relname AS index_name, \
     x.indisunique AS is_unique, x.indisprimary AS is_primary, x.indisvalid AS is_valid, \
     EXISTS (SELECT 1 FROM pg_constraint k WHERE k.conindid = x.indexrelid AND k.conrelid = x.indrelid \
     AND k.contype IN ('p', 'u', 'x')) AS is_constraint, \
     (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(x.indkey::int2[]) WITH ORDINALITY AS k(attnum, ord) \
     JOIN pg_attribute a ON a.attrelid = x.indrelid AND a.attnum = k.attnum) AS column_names \
     FROM pg_index x JOIN pg_class i ON i.oid = x.indexrelid JOIN pg_class t ON t.oid = x.indrelid \
     JOIN pg_namespace n ON n.oid = t.relnamespace \
     WHERE n.nspname = current_schema() AND t.relkind IN ('r', 'p')";

pub const POSTGRES_FOREIGN_KEYS: &str = "SELECT t.relname AS table_name, c.conname AS constraint_name, \
     r.relname AS referenced_table, c.convalidated AS is_valid, \
     (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(c.conkey) WITH ORDINALITY AS k(attnum, ord) \
     JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum) AS column_names \
     FROM pg_constraint c JOIN pg_class t ON t.oid = c.conrelid JOIN pg_namespace n ON n.oid = t.relnamespace \
     JOIN pg_class r ON r.oid = c.confrelid \
     WHERE c.contype = 'f' AND n.nspname = current_schema()";

pub const MYSQL_COLUMNS: &str = "SELECT c.TABLE_NAME AS table_name, c.COLUMN_NAME AS column_name, \
     c.COLUMN_TYPE AS column_type \
     FROM information_schema.COLUMNS c JOIN information_schema.TABLES t \
     ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
     WHERE c.TABLE_SCHEMA = database() AND t.TABLE_TYPE = 'BASE TABLE' \
     ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION";

/// MySQL and MariaDB have no invalid index; a unique index is the unique constraint, which the comparison expects
/// when the entity declares it, so only the primary key counts as a constraint's index.
pub const MYSQL_INDEXES: &str = "SELECT TABLE_NAME AS table_name, INDEX_NAME AS index_name, \
     MIN(NON_UNIQUE) = 0 AS is_unique, INDEX_NAME = 'PRIMARY' AS is_primary, 1 AS is_valid, \
     INDEX_NAME = 'PRIMARY' AS is_constraint, \
     GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') AS column_names \
     FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = database() \
     GROUP BY TABLE_NAME, INDEX_NAME";

pub const MYSQL_FOREIGN_KEYS: &str = "SELECT TABLE_NAME AS table_name, CONSTRAINT_NAME AS constraint_name, \
     REFERENCED_TABLE_NAME AS referenced_table, 1 AS is_valid, \
     GROUP_CONCAT(COLUMN_NAME ORDER BY ORDINAL_POSITION SEPARATOR ',') AS column_names \
     FROM information_schema.KEY_COLUMN_USAGE \
     WHERE TABLE_SCHEMA = database() AND REFERENCED_TABLE_NAME IS NOT NULL \
     GROUP BY TABLE_NAME, CONSTRAINT_NAME, REFERENCED_TABLE_NAME";

/// The catalog queries of `dialect`'s database; `None` for one the comparison does not read (SQLite, the browser's
/// local copy, and the databases no conf of this project runs on).
#[must_use]
pub fn schema_catalog(dialect: &Dialect) -> Option<SchemaCatalogSql> {
    if is_mysql(dialect) {
        Some(SchemaCatalogSql {
            columns: MYSQL_COLUMNS,
            indexes: MYSQL_INDEXES,
            foreign_keys: MYSQL_FOREIGN_KEYS,
        })
    } else if dialect.name.to_ascii_lowercase().starts_with("postgres") {
        Some(SchemaCatalogSql {
            columns: POSTGRES_COLUMNS,
            indexes: POSTGRES_INDEXES,
            foreign_keys: POSTGRES_FOREIGN_KEYS,
        })
    } else {
        None
    }
}

/// The relations of Postgres table oids with their columns: `oid`, `schema_name`, `table_name`, `kind` (`relkind`),
/// `column_number`, `column_name`. The oids are numbers, so they are written into the text. `None` for no oids.
#[must_use]
pub fn pg_relations_of_oids(oids: &[u32]) -> Option<String> {
    let mut oids = oids.to_vec();
    oids.sort_unstable();
    oids.dedup();
    if oids.is_empty() {
        return None;
    }
    let list: Vec<String> = oids.iter().map(u32::to_string).collect();
    Some(format!(
        "SELECT c.oid::bigint AS oid, n.nspname AS schema_name, c.relname AS table_name, c.relkind::text AS kind, \
         a.attnum::int AS column_number, a.attname AS column_name \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
         WHERE c.oid IN ({})",
        list.join(", ")
    ))
}

/// The type of each MySQL table named by (database, table): `TABLE_SCHEMA`, `TABLE_NAME` and `TABLE_TYPE` (`BASE
/// TABLE` or `VIEW`), and the values for its placeholders.
pub fn mysql_table_types<'a>(tables: impl IntoIterator<Item = (&'a str, &'a str)>) -> (String, Vec<Value>) {
    let mut terms = Vec::new();
    let mut params = Vec::new();
    for (schema, table) in tables {
        terms.push("(TABLE_SCHEMA = ? AND TABLE_NAME = ?)");
        params.push(Value::string(schema));
        params.push(Value::string(table));
    }
    if terms.is_empty() {
        terms.push("1 = 0");
    }
    (
        format!(
            "SELECT TABLE_SCHEMA AS table_schema, TABLE_NAME AS table_name, TABLE_TYPE AS table_type \
             FROM information_schema.TABLES WHERE {}",
            terms.join(" OR ")
        ),
        params,
    )
}

/// A SQL type in one spelling for every name the databases and the confs give it, so the type a field would get
/// (`database-type`, `default-sql-type`) compares with the type a column has (`format_type` on Postgres,
/// `COLUMN_TYPE` on MySQL): lower case, no space inside the parentheses, `VARCHAR` and `character varying` alike,
/// `TIMESTAMP` and `timestamp without time zone` alike, `DECIMAL` and `NUMERIC` alike, an integer's display width
/// (`int(11)`) dropped.
#[must_use]
pub fn normalize_sql_type(text: &str) -> String {
    let mut text = text
        .trim()
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for (from, to) in [(" (", "("), ("( ", "("), (" )", ")"), (" ,", ","), (", ", ",")] {
        while text.contains(from) {
            text = text.replace(from, to);
        }
    }
    let mut zone = "";
    for (suffix, kind) in [(" without time zone", ""), (" with time zone", "tz")] {
        if let Some(head) = text.strip_suffix(suffix) {
            text = head.to_string();
            zone = kind;
        }
    }
    let (base, rest) = match text.find('(') {
        Some(open) => text.split_at(open),
        None => match text.split_once(' ') {
            // `double precision`, `character varying`: a name of two words keeps them; `int unsigned` keeps its flag.
            Some((head, _)) if !matches!(head, "double" | "character") => (head, &text[head.len()..]),
            _ => (text.as_str(), ""),
        },
    };
    let base = match base {
        "character varying" | "varchar" | "varchar2" => "varchar",
        "character" | "char" | "bpchar" => "char",
        "int" | "int4" | "integer" => "integer",
        "int8" | "bigint" => "bigint",
        "int2" | "smallint" => "smallint",
        "float8" | "double precision" | "double" => "float8",
        "float4" | "real" => "float4",
        "numeric" | "decimal" => "numeric",
        "bool" | "boolean" => "boolean",
        "timestamp" => "timestamp",
        "time" => "time",
        other => other,
    };
    let mut rest = rest.to_string();
    // A display width means nothing for an integer: MySQL 8.0.19 and later stopped showing it.
    if matches!(base, "integer" | "bigint" | "smallint" | "tinyint" | "mediumint") && rest.starts_with('(') {
        if let Some(close) = rest.find(')') {
            rest = rest[close + 1..].to_string();
        }
    }
    let base = match (base, zone) {
        ("timestamp" | "time", "tz") => format!("{base}tz"),
        _ => base.to_string(),
    };
    format!("{base}{rest}")
}

/// Whether a column's type is the one a field's type gives, by [`normalize_sql_type`].
#[must_use]
pub fn same_sql_type(expected: &str, actual: &str) -> bool {
    normalize_sql_type(expected) == normalize_sql_type(actual)
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
