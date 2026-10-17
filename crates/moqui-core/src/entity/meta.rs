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
//!
//! As in Java's `EntityDbMeta`, a statement that fails is logged and the check goes on with the next
//! statement and entity; only the legacy-name check stops the run.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

use moqui_db::dialect::Dialect;
use moqui_db::sql::{
    add_column, create_fk_index, create_foreign_key, create_index, create_table, drop_fts_index,
    foreign_key_index_name, foreign_key_name, fts_index_name, fts_index_sql, fts_old_index_name, is_mysql, is_sqlite,
    legacy_name, physical_column_name, physical_name,
};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::{lock, read, write};

use super::exec::{already_exists, execute_conn, execute_script, query_conn, query_tx};
use super::facade::EntityFacade;
use crate::driver::DbConnection;
use crate::transaction::TransactionManager;

/// Database metadata for one facade.
pub struct EntityDbMeta<'a> {
    facade: &'a EntityFacade,
}

/// The DDL settings of one entity group's `<datasource>`, as Java's `EntityDbMeta` reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DdlConf {
    /// `startup-add-missing`: the startup check covers this group's tables. A group without it is left to the
    /// runtime check or the database.
    pub startup_add_missing: bool,
    /// `runtime-add-missing`: on an entity's first use, create its missing table or add its missing columns.
    pub runtime_add_missing: bool,
    /// `runtime-add-fks`: on that first use, also add the foreign keys of a table that was already there.
    pub runtime_add_fks: bool,
}

impl Default for DdlConf {
    fn default() -> Self {
        Self {
            startup_add_missing: true,
            runtime_add_missing: false,
            runtime_add_fks: true,
        }
    }
}

/// Java's `entityTablesExist` and `entityTablesChecked`, and the settings that use them.
#[derive(Default)]
pub(crate) struct TableState {
    conf: RwLock<HashMap<String, DdlConf>>,
    /// A find on a table that does not exist returns nothing, as Java. Off until the host turns it on, so a test
    /// driver that answers no metadata still runs its finds.
    find_check: AtomicBool,
    exists: Mutex<HashMap<String, bool>>,
    checked: Mutex<HashSet<String>>,
    /// Serializes the runtime check, as Java's `synchronized internalCheckTable`.
    check_lock: Mutex<()>,
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

    /// Whether the entity's table is in the database, asked of the database. A view asks of its members: true when
    /// any of them has a table, as Java's `tableExists`. The answer for a table is remembered.
    pub fn table_exists(&self, entity_name: &str) -> Result<bool, Error> {
        let (full_name, members) = view_members(self, entity_name)?;
        if let Some(members) = members {
            for member in members {
                if self.table_exists(&member)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        let (probe, tx) = self.with_group(entity_name, |entity, dialect, _, tx| {
            Ok((table_probe(dialect, &entity.table_name), tx.clone()))
        })?;
        let rows = query_tx(&tx, &probe.sql, &probe.params)?;
        let exists = rows.iter().any(|row| row_matches(&row.columns, &probe.table));
        lock(&self.inner.tables.exists).insert(full_name, exists);
        Ok(exists)
    }

    /// [`Self::table_exists`], from what is remembered when there is an answer.
    pub fn table_exists_cached(&self, entity_name: &str) -> Result<bool, Error> {
        let (full_name, members) = view_members(self, entity_name)?;
        if let Some(members) = members {
            for member in members {
                if self.table_exists_cached(&member)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if let Some(known) = lock(&self.inner.tables.exists).get(&full_name).copied() {
            return Ok(known);
        }
        self.table_exists(&full_name)
    }

    /// The DDL settings of `group`'s datasource.
    pub fn set_ddl_conf(&self, group: &str, conf: DdlConf) {
        write(&self.inner.tables.conf).insert(group.to_string(), conf);
    }

    /// Make a find on an entity whose table does not exist return nothing, as Java does, in place of a database
    /// error. The server turns this on; a test driver that answers no metadata leaves it off.
    pub fn set_find_table_check(&self, on: bool) {
        self.inner.tables.find_check.store(on, Ordering::Relaxed);
    }

    /// Forget what is known of the tables, as Java's `forceCheckExistingTables`, so the next use asks again.
    pub fn forget_tables(&self) {
        lock(&self.inner.tables.exists).clear();
        lock(&self.inner.tables.checked).clear();
    }

    /// Before a find: the runtime check when the group adds missing tables at runtime, then whether there is a
    /// table to read. False only when the find check is on and the table is known not to exist.
    pub(crate) fn table_ready(&self, entity_name: &str) -> bool {
        self.check_table_runtime(entity_name);
        if !self.inner.tables.find_check.load(Ordering::Relaxed) {
            return true;
        }
        match self.table_exists_cached(entity_name) {
            Ok(exists) => {
                if !exists {
                    tracing::debug!("Table for entity {entity_name} does not exist, the find returns nothing");
                }
                exists
            }
            // Let the find run and report what is wrong.
            Err(err) => {
                tracing::warn!(error = %err, "Could not check whether the table of {entity_name} exists");
                true
            }
        }
    }

    /// Java's `checkTableRuntime`: when the entity's group has `runtime-add-missing`, check its table once (a view's
    /// member tables), creating it or adding missing columns. A failure is logged; the caller goes on.
    pub(crate) fn check_table_runtime(&self, entity_name: &str) {
        let state = &self.inner.tables;
        if lock(&state.checked).contains(entity_name) {
            return;
        }
        let found = self.with_entity(entity_name, |entity, _, _| {
            let members: Vec<String> = entity
                .member_entities
                .iter()
                .map(|member| member.entity_name.clone())
                .collect();
            Ok((entity.full_name.clone(), entity.group.clone(), entity.is_view, members))
        });
        let Ok((full_name, group, is_view, members)) = found else {
            return;
        };
        if is_view {
            for member in members {
                self.check_table_runtime(&member);
            }
            return;
        }
        let group = group.unwrap_or_else(|| self.inner.default_group.clone());
        let conf = read(&state.conf).get(&group).copied().unwrap_or_default();
        if !conf.runtime_add_missing {
            return;
        }
        let _serial = lock(&state.check_lock);
        if lock(&state.checked).contains(&full_name) {
            return;
        }
        if let Err(err) = self.meta().run(&[full_name.as_str()], conf.runtime_add_fks, false) {
            tracing::error!(error = %err, "Could not check the table of entity {full_name} at runtime");
        }
        // Checked even when it failed, as Java: the check is not repeated on every use.
        lock(&state.checked).insert(full_name);
    }
}

/// The entity's full name, and the member entity names when it is a view.
fn view_members(facade: &EntityFacade, entity_name: &str) -> Result<(String, Option<Vec<String>>), Error> {
    facade.with_entity(entity_name, |entity, _, _| {
        let members = entity.is_view.then(|| {
            entity
                .member_entities
                .iter()
                .map(|member| member.entity_name.clone())
                .collect()
        });
        Ok((entity.full_name.clone(), members))
    })
}

impl EntityDbMeta<'_> {
    /// Create tables for these entities. View-entities expand to their member entities.
    pub fn check(&self, names: &[&str]) -> Result<(), Error> {
        self.run(names, true, true)
    }

    /// Create every non-view entity.
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
        self.run(&refs, true, true)
    }

    /// Check the tables of `names`. The foreign keys of a table that was already there are added when
    /// `fks_for_existing`; a new table always gets them, unless foreign key creation is off.
    /// With `startup`, a group whose datasource turns `startup-add-missing` off is left out.
    fn run(&self, names: &[&str], fks_for_existing: bool, startup: bool) -> Result<(), Error> {
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
        // As Java's `shouldCreateFks`: the facade or load flag, or `entity_disable_fk_create`.
        let skip_fk = super::fk::fk_create_disabled(self.facade)
            || moqui_lib::env::get_prop_or_env("entity_disable_fk_create") == "true";
        let started = std::time::Instant::now();
        let mut totals = Counts::default();
        let mut groups: Vec<String> = batches.keys().cloned().collect();
        groups.sort();
        tracing::info!("Checking tables for entities in groups [{}]", groups.join(", "));
        for (group, batch) in batches {
            let facade = self.facade;
            let ddl = read(&facade.inner.tables.conf).get(&group).copied().unwrap_or_default();
            if startup && !ddl.startup_add_missing {
                tracing::info!("Not checking tables of group {group} at startup, startup-add-missing is off");
                continue;
            }
            let names = batch.names;
            let group_started = std::time::Instant::now();
            // The outer error is the connection; the inner one is the legacy-name check, which stops the run.
            let counts = match batch
                .tx
                .with_reserve(|conn| Ok(apply(facade, conn, &group, &names, skip_fk, fks_for_existing)))
            {
                Ok(counts) => counts?,
                Err(err) => {
                    tracing::error!(error = %err, "Could not check tables for group {group}, no connection");
                    totals.errors += 1;
                    continue;
                }
            };
            tracing::info!(
                "Checked {} entities in group {group} in {:.3} seconds: {} tables created, {} already existed, {} columns added, {} indexes, {} full-text indexes, {} foreign keys, {} errors",
                names.len(),
                group_started.elapsed().as_secs_f64(),
                counts.tables_created,
                names.len().saturating_sub(counts.tables_created),
                counts.columns,
                counts.indexes,
                counts.fts,
                counts.fks,
                counts.errors
            );
            // Java's `entityTablesChecked` and `entityTablesExist`: a table that is there now is not checked again.
            {
                let mut exists = lock(&facade.inner.tables.exists);
                let mut checked = lock(&facade.inner.tables.checked);
                for name in names.iter().filter(|name| !counts.failed_tables.contains(name)) {
                    exists.insert(name.clone(), true);
                    checked.insert(name.clone());
                }
            }
            totals.add(&counts);
        }
        if totals.tables_created > 0 {
            tracing::info!("Created {} tables", totals.tables_created);
        } else {
            tracing::info!("No tables needed to be created");
        }
        if totals.errors > 0 {
            tracing::error!(
                "{} table check steps failed and were skipped, see the errors above",
                totals.errors
            );
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
    errors: usize,
    /// Entities left without a table.
    failed_tables: Vec<String>,
}

impl Counts {
    fn add(&mut self, other: &Counts) {
        self.tables_created += other.tables_created;
        self.columns += other.columns;
        self.indexes += other.indexes;
        self.fts += other.fts;
        self.fks += other.fks;
        self.errors += other.errors;
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
    fks_for_existing: bool,
) -> Result<Counts, Error> {
    // One read of the schema's tables and columns for every probe below.
    let _catalog = names
        .first()
        .and_then(|name| facade.with_entity(name, |_, dialect, _| Ok(dialect.clone())).ok())
        .and_then(|dialect| CatalogScope::enter(conn, &dialect));
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
    // Entities without a table after the create pass get no columns, indexes or foreign keys.
    let mut missing: HashSet<&str> = HashSet::new();
    let mut created: HashSet<&str> = HashSet::new();
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
                let columns: Vec<String> = entity
                    .fields
                    .iter()
                    .map(|field| physical_column_name(dialect, &field.column_name))
                    .collect();
                Ok(Some((
                    create_table(entity, model, dialect)?,
                    physical_name(dialect, &entity.table_name),
                    columns,
                )))
            }
        });
        match sql {
            Ok(Some((sql, table, columns))) => match execute_script(conn, &sql) {
                Ok(()) => {
                    catalog_note(&table, &columns);
                    counts.tables_created += 1;
                    created.insert(name.as_str());
                    tracing::info!("Created table {table} for entity {name} in group {group}");
                }
                Err(err) => {
                    tracing::error!(error = %err, sql = %sql, "Could not create table {table} for entity {name}");
                    counts.errors += 1;
                    missing.insert(name.as_str());
                }
            },
            Ok(None) => tracing::debug!("Table for entity {name} already exists"),
            Err(err) => {
                tracing::error!(error = %err, "Could not check or create the table for entity {name}");
                counts.errors += 1;
                missing.insert(name.as_str());
            }
        }
    }
    // One read of the index and foreign key names the group has, so a check does not send a statement for each
    // one that is already there.
    let existing = names
        .first()
        .and_then(|name| facade.with_entity(name, |_, dialect, _| Ok(dialect.clone())).ok())
        .map(|dialect| existing_constraints(conn, &dialect))
        .unwrap_or_default();
    for name in names.iter().filter(|name| !missing.contains(name.as_str())) {
        ensure_columns(facade, conn, name, &mut counts);
        ensure_indexes(facade, conn, name, &existing, &mut counts);
        ensure_fts(facade, conn, name, &existing, &mut counts);
    }
    if !skip_fk {
        if counts.tables_created > 0 {
            tracing::info!("Tables were created, checking FKs for all entities in group {group}");
        }
        for name in names
            .iter()
            .filter(|name| !missing.contains(name.as_str()))
            .filter(|name| fks_for_existing || created.contains(name.as_str()))
        {
            ensure_fks(facade, conn, name, &existing, &mut counts);
        }
    } else {
        tracing::info!("Not checking foreign keys for group {group}, disable-fk-create is set");
    }
    counts.failed_tables = missing.into_iter().map(str::to_string).collect();
    Ok(counts)
}

/// The lower-case index and foreign key constraint names in the current schema. Empty when they cannot be read:
/// each statement then runs and an object that is there is skipped by its error.
fn existing_constraints(conn: &mut dyn DbConnection, dialect: &Dialect) -> HashSet<String> {
    let queries: Vec<String> = if is_sqlite(dialect) {
        vec!["SELECT name FROM sqlite_master WHERE type = 'index'".to_string()]
    } else if is_mysql(dialect) {
        vec![
            "SELECT DISTINCT index_name FROM information_schema.statistics WHERE table_schema = database()".to_string(),
            "SELECT constraint_name FROM information_schema.table_constraints WHERE constraint_schema = database() \
             AND constraint_type = 'FOREIGN KEY'"
                .to_string(),
        ]
    } else if dialect.name.starts_with("postgres") {
        vec![
            "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema()".to_string(),
            "SELECT constraint_name FROM information_schema.table_constraints WHERE constraint_schema = \
             current_schema() AND constraint_type = 'FOREIGN KEY'"
                .to_string(),
        ]
    } else {
        Vec::new()
    };
    let mut names = HashSet::new();
    for sql in queries {
        match query_conn(conn, &sql, &[]) {
            Ok(rows) => names.extend(rows.iter().filter_map(|row| {
                row.columns
                    .first()
                    .and_then(|(_, value)| value.as_str())
                    .map(str::to_ascii_lowercase)
            })),
            Err(err) => {
                tracing::debug!(error = %err, "Could not read the existing index and foreign key names");
                return HashSet::new();
            }
        }
    }
    names
}

fn ensure_columns(facade: &EntityFacade, conn: &mut dyn DbConnection, name: &str, counts: &mut Counts) {
    let statements = facade.with_entity(name, |entity, dialect, model| {
        let present = column_names(conn, dialect, &entity.table_name)?;
        let mut statements = Vec::new();
        for field in &entity.fields {
            if has_name(dialect, &present, &physical_column_name(dialect, &field.column_name)) {
                continue;
            }
            match add_column(entity, field, model, dialect) {
                Ok(sql) => statements.push((sql, field.column_name.clone())),
                Err(err) => {
                    tracing::error!(error = %err, "Could not build the column {} for entity {name}", field.column_name);
                    counts.errors += 1;
                }
            }
        }
        Ok(statements)
    });
    let Some(statements) = planned(statements, "columns", name, counts) else {
        return;
    };
    for (sql, column) in statements {
        if run_statement(conn, &sql, &format!("column {column} of entity {name}"), counts) {
            counts.columns += 1;
            tracing::info!("Added column {column} to the table of entity {name}");
        }
    }
}

fn ensure_indexes(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    name: &str,
    existing: &HashSet<String>,
    counts: &mut Counts,
) {
    let statements = facade.with_entity(name, |entity, dialect, _| {
        let mut statements = Vec::new();
        // As Java's `createIndexes`: `use-indexes="false"` creates none, declared or for foreign keys.
        if !dialect.use_indexes {
            return Ok(statements);
        }
        for index in &entity.indexes {
            if existing.contains(&index.name.to_ascii_lowercase()) {
                continue;
            }
            if index.fields.is_empty() {
                tracing::warn!("Index {} of entity {name} has no fields, not creating it", index.name);
                continue;
            }
            match create_index(entity, index, dialect) {
                Ok(sql) => statements.push((sql, index.name.clone())),
                Err(err) => {
                    tracing::error!(error = %err, "Could not build index {} for entity {name}", index.name);
                    counts.errors += 1;
                }
            }
        }
        // Java creates an index on the keys of each foreign key; Postgres does not index them itself.
        if dialect.use_foreign_key_indexes {
            for relationship in &entity.relationships {
                if relationship.is_auto_reverse
                    || relationship.relationship_type != "one"
                    || relationship.key_pairs.is_empty()
                    || existing.contains(&foreign_key_index_name(entity, relationship, dialect).to_ascii_lowercase())
                {
                    continue;
                }
                match create_fk_index(entity, relationship, dialect) {
                    Ok(sql) => statements.push((sql, format!("foreign key to {}", relationship.related))),
                    Err(err) => {
                        tracing::error!(
                            error = %err,
                            "Could not build the foreign key index to {} for entity {name}",
                            relationship.related
                        );
                        counts.errors += 1;
                    }
                }
            }
        }
        Ok(statements)
    });
    let Some(statements) = planned(statements, "indexes", name, counts) else {
        return;
    };
    for (sql, label) in statements {
        if run_statement(conn, &sql, &format!("index {label} of entity {name}"), counts) {
            counts.indexes += 1;
            tracing::info!("Created index {label} for entity {name}");
        }
    }
}

/// The full-text column and index of each `text-fts` field, as Java's `FtsDdl` (see [`fts_index_sql`]), skipped when
/// the index is there. A database this port set up before has an index of another name and shape on the field
/// ([`fts_old_index_name`]); it is dropped, so the field has one full-text index.
fn ensure_fts(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    name: &str,
    existing: &HashSet<String>,
    counts: &mut Counts,
) {
    let statements = facade.with_entity(name, |entity, dialect, _| {
        let mut statements = Vec::new();
        for field in entity.fields.iter().filter(|field| field.field_type == "text-fts") {
            let index = fts_index_name(entity, field, dialect).to_ascii_lowercase();
            let old = fts_old_index_name(entity, field, dialect);
            let drop_old = old.to_ascii_lowercase() != index && existing.contains(&old.to_ascii_lowercase());
            let mut planned = Vec::new();
            match fts_index_sql(entity, field, dialect) {
                Ok(sql) if sql.is_empty() => continue,
                Ok(_) if existing.contains(&index) => {}
                Ok(sql) => planned.extend(sql),
                Err(err) => {
                    tracing::error!(error = %err, "Could not build the full-text index on field {} of entity {name}", field.name);
                    counts.errors += 1;
                    continue;
                }
            }
            let creates = !planned.is_empty();
            if drop_old {
                planned.push(drop_fts_index(entity, &old, dialect));
            }
            if !planned.is_empty() {
                statements.push((planned, creates, field.name.clone()));
            }
        }
        Ok(statements)
    });
    let Some(statements) = planned(statements, "full-text indexes", name, counts) else {
        return;
    };
    for (sql, creates, field) in statements {
        let what = format!("full-text index on field {field} of entity {name}");
        // The column, then its index, then the old index: a statement runs only after the one before it.
        if sql.iter().all(|sql| run_statement(conn, sql, &what, counts)) && creates {
            counts.fts += 1;
            tracing::info!("Created full-text index on field {field} of entity {name}");
        }
    }
}

fn ensure_fks(
    facade: &EntityFacade,
    conn: &mut dyn DbConnection,
    name: &str,
    existing: &HashSet<String>,
    counts: &mut Counts,
) {
    let statements = facade.with_entity(name, |entity, dialect, model| {
        let mut statements = Vec::new();
        // SQLite cannot add a foreign key to an existing table, and the browser copy is not the authority.
        // `use-foreign-keys="false"` creates none, as Java.
        if is_sqlite(dialect) || !dialect.use_foreign_keys {
            return Ok(statements);
        }
        for relationship in &entity.relationships {
            if relationship.is_auto_reverse || relationship.relationship_type != "one" {
                continue;
            }
            if !relationship.key_pairs.is_empty()
                && existing.contains(&foreign_key_name(entity, relationship, dialect).to_ascii_lowercase())
            {
                continue;
            }
            if relationship.key_pairs.is_empty() {
                tracing::warn!(
                    "Relationship {} of entity {name} has no key fields, not creating a foreign key",
                    relationship.name
                );
                continue;
            }
            let Ok(related) = model.entity(&relationship.related) else {
                tracing::error!(
                    "Relationship {} of entity {name} is to unknown entity {}, not creating a foreign key",
                    relationship.name,
                    relationship.related
                );
                counts.errors += 1;
                continue;
            };
            match table_exists_conn(conn, dialect, &related.table_name) {
                Ok(true) if !related.is_view => {}
                Ok(_) => continue,
                Err(err) => {
                    tracing::error!(error = %err, "Could not check the table of {} for a foreign key of entity {name}", related.full_name);
                    counts.errors += 1;
                    continue;
                }
            }
            match create_foreign_key(entity, relationship, related, dialect) {
                Ok(sql) => statements.push((sql, relationship.related.clone())),
                Err(err) => {
                    tracing::error!(error = %err, "Could not build the foreign key to {} for entity {name}", relationship.related);
                    counts.errors += 1;
                }
            }
        }
        Ok(statements)
    });
    let Some(statements) = planned(statements, "foreign keys", name, counts) else {
        return;
    };
    let mut created = 0;
    for (sql, related) in statements {
        if run_statement(
            conn,
            &sql,
            &format!("foreign key to {related} for entity {name}"),
            counts,
        ) {
            created += 1;
        }
    }
    if created > 0 {
        tracing::info!("Created {created} FKs for entity {name}");
    }
    counts.fks += created;
}

/// The statements planned for one entity, or `None` after logging why they could not be planned.
fn planned<T>(statements: Result<Vec<T>, Error>, what: &str, name: &str, counts: &mut Counts) -> Option<Vec<T>> {
    match statements {
        Ok(statements) => Some(statements),
        Err(err) => {
            tracing::error!(error = %err, "Could not check {what} for entity {name}");
            counts.errors += 1;
            None
        }
    }
}

/// True when the statement ran. An object that already exists is false; a failed statement is logged,
/// counted and false, as Java's `runSqlUpdate` does.
fn run_statement(conn: &mut dyn DbConnection, sql: &str, what: &str, counts: &mut Counts) -> bool {
    match execute_conn(conn, sql, &[]) {
        Ok(_) => true,
        Err(err) if already_exists(&err) => false,
        Err(err) => {
            tracing::error!(error = %err, sql = %sql, "Could not create {what}");
            counts.errors += 1;
            false
        }
    }
}

thread_local! {
    /// The schema's tables and their columns, read once for a check of many entities. See [`CatalogScope`].
    static CATALOG: std::cell::RefCell<Option<HashMap<String, Vec<String>>>> = const { std::cell::RefCell::new(None) };
}

/// The schema's tables and columns, read in two queries, answering the table and column probes of one check in
/// place of a query for each entity. Not for SQLite, whose probes stay per table. Dropped at the end of the check.
struct CatalogScope;

impl CatalogScope {
    fn enter(conn: &mut dyn DbConnection, dialect: &Dialect) -> Option<Self> {
        if is_sqlite(dialect) {
            return None;
        }
        let schema = schema_expr(dialect);
        let mut tables: HashMap<String, Vec<String>> = HashMap::new();
        let read = (|| -> Result<(), Error> {
            let sql = format!("SELECT table_name FROM information_schema.tables WHERE table_schema = {schema}");
            for row in query_conn(conn, &sql, &[])? {
                if let Some(name) = row.columns.first().and_then(|(_, value)| value.as_str()) {
                    tables.entry(name.to_string()).or_default();
                }
            }
            let sql =
                format!("SELECT table_name, column_name FROM information_schema.columns WHERE table_schema = {schema}");
            for row in query_conn(conn, &sql, &[])? {
                let mut cells = row.columns.iter().map(|(_, value)| value.as_str());
                if let (Some(Some(table)), Some(Some(column))) = (cells.next(), cells.next()) {
                    tables.entry(table.to_string()).or_default().push(column.to_string());
                }
            }
            Ok(())
        })();
        match read {
            Ok(()) => {
                CATALOG.with(|slot| *slot.borrow_mut() = Some(tables));
                Some(Self)
            }
            Err(err) => {
                tracing::debug!(error = %err, "Could not read the schema catalog, checking each table");
                None
            }
        }
    }
}

impl Drop for CatalogScope {
    fn drop(&mut self) {
        CATALOG.with(|slot| *slot.borrow_mut() = None);
    }
}

/// The catalog's answer for `table`: `Some` with its columns when it has the table (exact or ignoring case), `Some(None)`
/// when it does not, `None` with no catalog in scope.
fn catalog_table(table: &str) -> Option<Option<Vec<String>>> {
    CATALOG.with(|slot| {
        slot.borrow().as_ref().map(|tables| {
            tables
                .get(table)
                .or_else(|| {
                    tables
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case(table))
                        .map(|(_, columns)| columns)
                })
                .cloned()
        })
    })
}

/// Note a table or column this check made, so later probes see it.
fn catalog_note(table: &str, columns: &[String]) {
    CATALOG.with(|slot| {
        if let Some(tables) = slot.borrow_mut().as_mut() {
            tables
                .entry(table.to_string())
                .or_default()
                .extend(columns.iter().cloned());
        }
    });
}

fn table_exists_conn(conn: &mut dyn DbConnection, dialect: &Dialect, table: &str) -> Result<bool, Error> {
    if let Some(found) = catalog_table(&physical_name(dialect, table)) {
        return Ok(found.is_some());
    }
    let probe = table_probe(dialect, table);
    let rows = query_conn(conn, &probe.sql, &probe.params)?;
    Ok(rows.iter().any(|row| row_matches(&row.columns, table)))
}

fn column_names(conn: &mut dyn DbConnection, dialect: &Dialect, table: &str) -> Result<Vec<String>, Error> {
    if let Some(found) = catalog_table(&physical_name(dialect, table)) {
        return Ok(found.unwrap_or_default());
    }
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
    let known = CATALOG.with(|slot| slot.borrow().as_ref().map(|tables| tables.contains_key(table)));
    if let Some(known) = known {
        return Ok(known);
    }
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
