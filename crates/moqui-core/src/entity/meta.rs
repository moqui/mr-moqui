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

//! Check and create tables, columns, indexes, foreign keys, and text-fts indexes.
//! Runtime DDL uses a reserve connection.

use std::collections::{HashMap, HashSet};

use moqui_db::dialect::Dialect;
use moqui_db::sql::{
    add_column, create_fk_index, create_foreign_key, create_index, create_table, fts_index_sql, is_mysql, is_sqlite,
    legacy_name, physical_column_name, physical_name,
};
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::exec::{already_exists, execute_conn, execute_script, query_conn, query_tx};
use super::facade::EntityFacade;
use crate::driver::DbConnection;
use crate::transaction::TransactionManager;

/// Database metadata for one facade.
pub struct EntityDbMeta<'a> {
    facade: &'a EntityFacade,
}

impl EntityFacade {
    pub fn meta(&self) -> EntityDbMeta<'_> {
        EntityDbMeta { facade: self }
    }

    pub fn check_tables(&self, names: &[&str]) -> Result<(), Error> {
        self.meta().check(names)
    }

    pub fn check_all_tables(&self) -> Result<(), Error> {
        self.meta().check_all()
    }

    /// Java's `checkAndAddAllTables` first pass; true when any table was created.
    pub fn check_tables_startup(&self, names: &[&str]) -> Result<bool, Error> {
        self.meta().check_startup(names)
    }

    /// Java's `checkAndAddAllTables` second pass: every missing foreign key, each group in its own transaction.
    pub fn check_fks_startup(&self, names: &[&str]) -> Result<(), Error> {
        self.meta().check_fks_startup(names)
    }

    pub fn table_exists(&self, entity_name: &str) -> Result<bool, Error> {
        let (probe, tx) = self.with_group(entity_name, |entity, dialect, _, tx| {
            Ok((table_probe(dialect, &entity.table_name), tx.clone()))
        })?;
        let rows = query_tx(&tx, &probe.sql, &probe.params)?;
        Ok(rows.iter().any(|row| row_matches(&row.columns, &probe.table)))
    }
}

impl EntityDbMeta<'_> {
    /// Create tables for these entities. View-entities expand to their member entities.
    pub fn check(&self, names: &[&str]) -> Result<(), Error> {
        self.run(names, false)
    }

    /// Create every non-view entity. A foreign key that cannot be created is logged and skipped.
    pub fn check_all(&self) -> Result<(), Error> {
        let owned: Vec<String> = self
            .facade
            .inner
            .model
            .model()
            .entities()
            .iter()
            .filter(|entity| !entity.is_view)
            .map(|entity| entity.full_name.clone())
            .collect();
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        self.run(&refs, true)
    }

    fn batches(&self, names: &[&str]) -> Result<HashMap<String, Batch>, Error> {
        let concrete = expand(self.facade, names)?;
        let default_group = self.facade.inner.default_group.clone();
        let mut batches: HashMap<String, Batch> = HashMap::new();
        for name in concrete {
            let (group, tx) = self.facade.with_group(&name, |entity, _, _, tx| {
                Ok((
                    entity.group.clone().unwrap_or_else(|| default_group.clone()),
                    tx.clone(),
                ))
            })?;
            match batches.entry(group) {
                std::collections::hash_map::Entry::Occupied(mut entry) => entry.get_mut().names.push(name),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(Batch { tx, names: vec![name] });
                }
            }
        }
        Ok(batches)
    }

    /// Groups in name order and, within each group, entities in full-name order (Java's TreeSets).
    fn sorted_batches(&self, names: &[&str]) -> Result<Vec<(String, Batch)>, Error> {
        let mut batches: Vec<_> = self.batches(names)?.into_iter().collect();
        batches.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, batch) in &mut batches {
            batch.names.sort();
        }
        Ok(batches)
    }

    fn check_startup(&self, names: &[&str]) -> Result<bool, Error> {
        let skip_fk = super::fk::fk_create_disabled(self.facade);
        let mut created = false;
        for (group, batch) in self.sorted_batches(names)? {
            let facade = self.facade;
            let body = |conn: &mut dyn DbConnection| apply_startup(facade, conn, &batch.names, skip_fk);
            let counts = if batch.tx.in_transaction() {
                batch.tx.with_connection(body)?
            } else {
                batch.tx.with_reserve(body)?
            };
            tracing::info!(
                "Checked {} entities in group {group}: {} tables created, {} columns added, {} indexes, {} full-text indexes, {} foreign keys",
                batch.names.len(),
                counts.tables_created,
                counts.columns,
                counts.indexes,
                counts.fts,
                counts.fks
            );
            created |= counts.tables_created > 0;
        }
        Ok(created)
    }

    fn check_fks_startup(&self, names: &[&str]) -> Result<(), Error> {
        if super::fk::fk_create_disabled(self.facade) {
            return Ok(());
        }
        for (group, batch) in self.sorted_batches(names)? {
            tracing::info!("Tables were created, checking FKs for all entities in group {group}");
            let facade = self.facade;
            batch.tx.with_reserve(|conn| {
                for name in &batch.names {
                    ensure_fks(facade, conn, name, false)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    fn run(&self, names: &[&str], lenient_fk: bool) -> Result<(), Error> {
        let batches = self.batches(names)?;
        let skip_fk = super::fk::fk_create_disabled(self.facade);
        let started = std::time::Instant::now();
        let mut totals = Counts::default();
        let mut groups: Vec<String> = batches.keys().cloned().collect();
        groups.sort();
        tracing::info!("Checking tables for entities in groups [{}]", groups.join(", "));
        let mut batches: Vec<_> = batches.into_iter().collect();
        if crate::deterministic::enabled() {
            batches.sort_by(|a, b| a.0.cmp(&b.0));
        }
        for (group, batch) in batches {
            let facade = self.facade;
            let names = batch.names;
            let group_started = std::time::Instant::now();
            let counts = if crate::deterministic::enabled() && batch.tx.in_transaction() {
                batch.tx.with_connection(|conn| apply(facade, conn, &group, &names, skip_fk, lenient_fk))?
            } else {
                batch.tx.with_reserve(|conn| apply(facade, conn, &group, &names, skip_fk, lenient_fk))?
            };
            tracing::info!(
                "Checked {} entities in group {group} in {:.3} seconds: {} tables created, {} already existed, {} columns added, {} indexes, {} full-text indexes, {} foreign keys",
                names.len(),
                group_started.elapsed().as_secs_f64(),
                counts.tables_created,
                names.len().saturating_sub(counts.tables_created),
                counts.columns,
                counts.indexes,
                counts.fts,
                counts.fks
            );
            totals.add(&counts);
        }
        if totals.tables_created > 0 {
            tracing::info!("Created {} tables", totals.tables_created);
        } else {
            tracing::info!("No tables needed to be created");
        }
        tracing::info!(
            "Checked tables for all entities in {:.3} seconds",
            started.elapsed().as_secs_f64()
        );
        Ok(())
    }
}

#[derive(Default)]
struct Counts {
    tables_created: usize,
    columns: usize,
    indexes: usize,
    fts: usize,
    fks: usize,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
        self.tables_created += other.tables_created;
        self.columns += other.columns;
        self.indexes += other.indexes;
        self.fts += other.fts;
        self.fks += other.fks;
    }
}

struct Batch {
    tx: TransactionManager,
    names: Vec<String>,
}

struct Probe {
    sql: String,
    params: Vec<Value>,
    table: String,
}

fn expand(facade: &EntityFacade, names: &[&str]) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for name in names {
        collect(facade, name, &mut out, &mut seen)?;
    }
    Ok(out)
}

fn collect(facade: &EntityFacade, name: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) -> Result<(), Error> {
    let (is_view, full_name, members) = facade.with_entity(name, |entity, _, _| {
        let members = if entity.is_view {
            entity
                .member_entities
                .iter()
                .map(|member| member.entity_name.clone())
                .collect()
        } else {
            Vec::new()
        };
        Ok((entity.is_view, entity.full_name.clone(), members))
    })?;
    if is_view {
        for member in members {
            collect(facade, &member, out, seen)?;
        }
        return Ok(());
    }
    if seen.insert(full_name.clone()) {
        out.push(full_name);
    }
    Ok(())
}

fn apply(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    group: &str,
    names: &[String],
    skip_fk: bool,
    lenient_fk: bool,
) -> Result<Counts, Error> {
    // Detect old quoted Rust tables/columns before doing any DDL in this batch.
    // Never create an empty parallel table or add empty replacement columns over legacy data.
    for name in names {
        facade.with_entity(name, |entity, dialect, _| {
            if dialect.legacy_names {
                return Ok(());
            }
            let table = physical_name(dialect, &entity.table_name);
            for old in [&entity.entity_name, &entity.table_name] {
                let old = legacy_name(dialect, old);
                if old != table && exact_table_exists(conn, dialect, &old)? {
                    return Err(legacy_schema_error(name));
                }
            }
            if table_exists_conn(conn, dialect, &entity.table_name)? {
                let present = column_names(conn, dialect, &entity.table_name)?;
                for field in &entity.fields {
                    let column = physical_column_name(dialect, &field.column_name);
                    if !has_name(dialect, &present, &column)
                        && [&field.name, &field.column_name]
                            .iter()
                            .any(|old| present.contains(&legacy_name(dialect, old)))
                    {
                        return Err(legacy_schema_error(name));
                    }
                }
            }
            Ok(())
        })?;
    }
    let mut counts = Counts::default();
    for name in names {
        let sql = facade.with_entity(name, |entity, dialect, model| {
            if table_exists_conn(conn, dialect, &entity.table_name)? {
                Ok(None)
            } else {
                let pks: Vec<&str> = entity
                    .fields
                    .iter()
                    .filter(|field| field.is_pk)
                    .map(|field| field.name.as_str())
                    .collect();
                tracing::info!("Creating table for {name} pks: [{}]", pks.join(", "));
                Ok(Some((create_table(entity, model, dialect)?, entity.table_name.clone())))
            }
        })?;
        if let Some((sql, table)) = sql {
            execute_script(conn, &sql)?;
            counts.tables_created += 1;
            tracing::info!("Created table {table} for entity {name} in group {group}");
        } else {
            tracing::debug!("Table for entity {name} already exists");
        }
    }
    for name in names {
        counts.columns += ensure_columns(facade, conn, name)?;
        counts.indexes += ensure_indexes(facade, conn, name)?;
        counts.fts += ensure_fts(facade, conn, name)?;
    }
    if !skip_fk {
        if counts.tables_created > 0 {
            tracing::info!("Tables were created, checking FKs for all entities in group {group}");
        }
        for name in names {
            counts.fks += ensure_fks(facade, conn, name, lenient_fk)?;
        }
    } else {
        tracing::info!("Not checking foreign keys for group {group}, disable-fk-create is set");
    }
    Ok(counts)
}

// Mirrors Java's checkAndAddAllTables: an existing table only gets missing columns; a new table
// gets its indexes and the foreign keys to tables that already exist, in entity-name order.
fn apply_startup(facade: &EntityFacade, conn: &mut dyn DbConnection, names: &[String], skip_fk: bool) -> Result<Counts, Error> {
    let mut counts = Counts::default();
    for name in names {
        let sql = facade.with_entity(name, |entity, dialect, model| {
            if table_exists_conn(conn, dialect, &entity.table_name)? {
                Ok(None)
            } else {
                Ok(Some(create_table(entity, model, dialect)?))
            }
        })?;
        match sql {
            None => counts.columns += ensure_columns(facade, conn, name)?,
            Some(sql) => {
                execute_script(conn, &sql)?;
                counts.tables_created += 1;
                tracing::info!("Created table for entity {name}");
                counts.indexes += ensure_indexes(facade, conn, name)?;
                if !skip_fk {
                    counts.fks += ensure_fks(facade, conn, name, false)?;
                }
            }
        }
        counts.fts += ensure_fts(facade, conn, name)?;
    }
    Ok(counts)
}

fn ensure_columns(facade: &EntityFacade, conn: &mut dyn DbConnection, name: &str) -> Result<usize, Error> {
    let statements = facade.with_entity(name, |entity, dialect, model| {
        let present = column_names(conn, dialect, &entity.table_name)?;
        let mut statements = Vec::new();
        for field in &entity.fields {
            if has_name(dialect, &present, &physical_column_name(dialect, &field.column_name)) {
                continue;
            }
            statements.push((add_column(entity, field, model, dialect)?, field.column_name.clone()));
        }
        Ok(statements)
    })?;
    let mut added = 0;
    for (sql, column) in statements {
        if run_statement(conn, &sql, false)? {
            added += 1;
            tracing::info!("Added column {column} to the table of entity {name}");
        }
    }
    Ok(added)
}

fn ensure_indexes(facade: &EntityFacade, conn: &mut dyn DbConnection, name: &str) -> Result<usize, Error> {
    let statements = facade.with_entity(name, |entity, dialect, _| {
        let mut statements = Vec::new();
        for index in &entity.indexes {
            statements.push((create_index(entity, index, dialect)?, index.name.clone()));
        }
        // Java creates an index on the keys of each foreign key; Postgres does not index them itself.
        if dialect.use_foreign_key_indexes {
            for relationship in &entity.relationships {
                if relationship.is_auto_reverse
                    || relationship.relationship_type != "one"
                    || relationship.key_pairs.is_empty()
                {
                    continue;
                }
                statements.push((
                    create_fk_index(entity, relationship, dialect)?,
                    format!("foreign key to {}", relationship.related),
                ));
            }
        }
        Ok(statements)
    })?;
    let mut created = 0;
    for (sql, label) in statements {
        if run_statement(conn, &sql, false)? {
            created += 1;
            tracing::info!("Created index {label} for entity {name}");
        }
    }
    Ok(created)
}

fn ensure_fts(facade: &EntityFacade, conn: &mut dyn DbConnection, name: &str) -> Result<usize, Error> {
    let statements = facade.with_entity(name, |entity, dialect, _| {
        let mut statements = Vec::new();
        for field in entity.fields.iter().filter(|field| field.field_type == "text-fts") {
            if let Some(sql) = fts_index_sql(entity, field, dialect)? {
                statements.push((sql, field.name.clone()));
            }
        }
        Ok(statements)
    })?;
    let mut created = 0;
    for (sql, field) in statements {
        if run_statement(conn, &sql, false)? {
            created += 1;
            tracing::info!("Created full-text index on field {field} of entity {name}");
        }
    }
    Ok(created)
}

fn ensure_fks(facade: &EntityFacade, conn: &mut dyn DbConnection, name: &str, lenient: bool) -> Result<usize, Error> {
    let statements = facade.with_entity(name, |entity, dialect, model| {
        let mut statements = Vec::new();
        // SQLite cannot add a foreign key to an existing table, and the browser copy is not the authority.
        if is_sqlite(dialect) {
            return Ok(statements);
        }
        for relationship in &entity.relationships {
            if relationship.is_auto_reverse || relationship.relationship_type != "one" {
                continue;
            }
            let related = model.entity(&relationship.related)?;
            if related.is_view || !table_exists_conn(conn, dialect, &related.table_name)? {
                continue;
            }
            statements.push(create_foreign_key(entity, relationship, related, dialect)?);
        }
        Ok(statements)
    })?;
    let mut created = 0;
    for sql in statements {
        if run_statement(conn, &sql, lenient)? {
            created += 1;
        }
    }
    if created > 0 {
        tracing::info!("Created {created} FKs for entity {name}");
    }
    Ok(created)
}

/// True when the statement ran, false when the object existed or a lenient statement failed.
fn run_statement(conn: &mut dyn DbConnection, sql: &str, lenient: bool) -> Result<bool, Error> {
    // Normal reserve DDL is autocommit. In deterministic mode it belongs to an explicit
    // transaction, so the existing tolerated duplicate/FK errors need a savepoint.
    if crate::deterministic::enabled() {
        conn.savepoint("moqui_meta_statement")?;
        let result = execute_conn(conn, sql, &[]);
        if result.is_err() {
            conn.rollback_to_savepoint("moqui_meta_statement")?;
        }
        conn.release_savepoint("moqui_meta_statement")?;
        return match result {
            Ok(_) => Ok(true),
            Err(err) if already_exists(&err) => Ok(false),
            Err(err) if lenient => {
                tracing::warn!(%err, sql = %sql, "foreign key was not created");
                Ok(false)
            }
            Err(err) => Err(err),
        };
    }
    match execute_conn(conn, sql, &[]) {
        Ok(_) => Ok(true),
        Err(err) if already_exists(&err) => Ok(false),
        Err(err) if lenient => {
            tracing::warn!(%err, sql = %sql, "foreign key was not created");
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

fn table_exists_conn(conn: &mut dyn DbConnection, dialect: &Dialect, table: &str) -> Result<bool, Error> {
    let probe = table_probe(dialect, table);
    let rows = query_conn(conn, &probe.sql, &probe.params)?;
    Ok(rows.iter().any(|row| row_matches(&row.columns, table)))
}

fn column_names(conn: &mut dyn DbConnection, dialect: &Dialect, table: &str) -> Result<Vec<String>, Error> {
    let probe = column_probe(dialect, table);
    let rows = query_conn(conn, &probe.sql, &probe.params)?;
    let mut names = Vec::new();
    for row in rows {
        if let Some(Value::String(name)) = row.columns.first().map(|(_, value)| value) {
            names.push(name.clone());
        }
    }
    Ok(names)
}

fn table_probe(dialect: &Dialect, table: &str) -> Probe {
    let table = physical_name(dialect, table);
    let table = table.as_str();
    if is_sqlite(dialect) {
        return Probe {
            sql: "SELECT name FROM sqlite_master WHERE type = 'table' AND lower(name) = lower(?)".to_string(),
            params: vec![Value::string(table)],
            table: table.to_string(),
        };
    }
    Probe {
        sql: format!(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = {} AND {}",
            schema_expr(dialect),
            table_predicate(dialect)
        ),
        params: vec![Value::string(table)],
        table: table.to_string(),
    }
}

fn column_probe(dialect: &Dialect, table: &str) -> Probe {
    let table = physical_name(dialect, table);
    let table = table.as_str();
    if is_sqlite(dialect) {
        return Probe {
            sql: "SELECT name FROM pragma_table_info(?)".to_string(),
            params: vec![Value::string(table)],
            table: table.to_string(),
        };
    }
    Probe {
        sql: format!(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = {} AND {}",
            schema_expr(dialect),
            table_predicate(dialect)
        ),
        params: vec![Value::string(table)],
        table: table.to_string(),
    }
}

fn table_predicate(dialect: &Dialect) -> &'static str {
    if dialect.name.starts_with("postgres") {
        "table_name = ?"
    } else {
        "lower(table_name) = lower(?)"
    }
}

fn legacy_schema_error(name: &str) -> Error {
    Error::new(format!(
        "legacy Rust SQL names detected for {name}; no automatic migration is performed. \
         Set entity_sql_name_style=legacy to keep the existing schema, or migrate it offline first"
    ))
}

fn exact_table_exists(conn: &mut dyn DbConnection, dialect: &Dialect, table: &str) -> Result<bool, Error> {
    let sql = if is_sqlite(dialect) {
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?".to_string()
    } else {
        format!(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = {} AND table_name = ?",
            schema_expr(dialect)
        )
    };
    Ok(query_conn(conn, &sql, &[Value::string(table)])?.iter().any(|row| {
        row.columns
            .first()
            .is_some_and(|(_, value)| value.as_str() == Some(table))
    }))
}

fn schema_expr(dialect: &Dialect) -> &'static str {
    if is_mysql(dialect) {
        "database()"
    } else {
        "current_schema()"
    }
}

fn row_matches(columns: &[(String, Value)], table: &str) -> bool {
    let Some(Value::String(name)) = columns.first().map(|(_, value)| value) else {
        return false;
    };
    name == table || name.eq_ignore_ascii_case(table)
}

fn has_name(dialect: &Dialect, names: &[String], want: &str) -> bool {
    names.iter().any(|name| name == want)
        || (!dialect.name.starts_with("postgres") && names.iter().any(|name| name.eq_ignore_ascii_case(want)))
}
