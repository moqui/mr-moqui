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

//! Schema: the comparison of the entity model with the database (`compare#Schema`, plans/archive/Tools-Database.md
//! §5.1), the plan of statements that brings a selection of its differences in line in the online forms
//! (`plan#SchemaChanges`, §5.2), which a job then runs (`crate::jobs`), and the DDL of a set of entities as a `.sql`
//! file (`get#SchemaDdl`).

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use moqui_db::dialect::Dialect;
use moqui_db::model::Relationship;
use moqui_db::sql::{create_fk_index, create_foreign_key, create_index, create_table, physical_name, quote_ident};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::entity::{EntityFacade, IndexKind, TableDiff};

use crate::group_db::{add_error, cell_text, list, GroupDb, Out, Server};
use crate::jobs::takes_strong_lock;

/// What a plan may take on.
pub const CHANGES: &[&str] = &[
    "missingIndexes",
    "invalidIndexes",
    "extraIndexes",
    "missingForeignKeys",
    "extraForeignKeys",
    "dropForeignKeys",
];

/// The non-view entities of `group`, sorted.
pub(crate) fn group_entities(facade: &EntityFacade, group: &str) -> Vec<String> {
    let default_group = facade.default_group().to_string();
    facade.with_model(|model| {
        let names: BTreeSet<String> = model
            .entities()
            .iter()
            .filter(|entity| !entity.is_view && entity.group.as_deref().unwrap_or(&default_group) == group)
            .map(|entity| entity.full_name.clone())
            .collect();
        names.into_iter().collect()
    })
}

/// The entities a service is about: `entityNames`, or every one of the group.
fn selected(parameters: &BTreeMap<String, Value>, facade: &EntityFacade, db: &GroupDb) -> Vec<String> {
    let names = list(parameters, "entityNames");
    if names.is_empty() {
        group_entities(facade, &db.group)
    } else {
        names
    }
}

/// `compare#Schema`: the comparison of `entityNames` (every entity of the group when empty) with the database. Out:
/// `checked`, `tables` and `errors` of `SchemaDiff::to_value`.
pub(crate) fn compare_schema(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of_group(parameters, facade) else {
        return Ok(Out::new());
    };
    let names = selected(parameters, facade, &db);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let diff = match facade.meta().compare(&refs) {
        Ok(diff) => diff,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    let mut out = Out::new();
    if let Value::Map(map) = diff.to_value() {
        out.extend(map);
    }
    out.insert("db".to_string(), Value::string(db.db_text()));
    Ok(out)
}

/// One statement of a plan, as `create#DatabaseJob` takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanStatement {
    pub statement_seq_id: String,
    pub statement_text: String,
    pub depends_on_seq_id: Option<String>,
    pub entity_name: String,
    pub table_name: String,
    /// What it does, for the screen: `create index NAME`, `add foreign key NAME`, ….
    pub what: String,
}

impl PlanStatement {
    pub(crate) fn to_value(&self) -> Value {
        let mut map = IndexMap::new();
        map.insert("statementSeqId".to_string(), Value::string(&self.statement_seq_id));
        map.insert("statementText".to_string(), Value::string(&self.statement_text));
        map.insert(
            "dependsOnSeqId".to_string(),
            self.depends_on_seq_id.as_deref().map_or(Value::Null, Value::string),
        );
        map.insert("entityName".to_string(), Value::string(&self.entity_name));
        map.insert("tableName".to_string(), Value::string(&self.table_name));
        map.insert("what".to_string(), Value::string(&self.what));
        map.insert(
            "lockTimeout".to_string(),
            Value::Bool(takes_strong_lock(&self.statement_text)),
        );
        Value::Map(map)
    }
}

/// A plan being built: its statements, numbered as they are added, and the warnings.
#[derive(Default)]
pub(crate) struct Plan {
    pub(crate) statements: Vec<PlanStatement>,
    pub(crate) warnings: Vec<String>,
}

impl Plan {
    /// Add a statement; its sequence id, for a later one to depend on.
    pub(crate) fn add(
        &mut self,
        text: String,
        depends_on: Option<String>,
        entity: &str,
        table: &str,
        what: String,
    ) -> String {
        let seq = crate::jobs::seq_id(self.statements.len());
        self.statements.push(PlanStatement {
            statement_seq_id: seq.clone(),
            statement_text: text,
            depends_on_seq_id: depends_on,
            entity_name: entity.to_string(),
            table_name: table.to_string(),
            what,
        });
        seq
    }

    pub(crate) fn warn(&mut self, warning: impl Into<String>) {
        let warning = warning.into();
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    pub(crate) fn out(&self) -> Out {
        let mut out = Out::new();
        out.insert(
            "statements".to_string(),
            Value::List(self.statements.iter().map(PlanStatement::to_value).collect()),
        );
        out.insert(
            "warnings".to_string(),
            Value::List(self.warnings.iter().map(Value::string).collect()),
        );
        out
    }

    /// [`Self::out`], with the plan stored as a Pending job of `kind` that `run#DatabaseJob` runs as shown (§5.2):
    /// `jobId`, or null with `jobReason` when there is nothing to run or the user lacks `DATABASE_ADMIN` (the plan is
    /// still shown). `description` and `lockTimeoutMs` come from the parameters.
    pub(crate) fn out_with_job(
        &self,
        facade: &EntityFacade,
        db: &GroupDb,
        kind: &str,
        parameters: &BTreeMap<String, Value>,
    ) -> Out {
        let mut out = self.out();
        let reason = if self.statements.is_empty() {
            Some("nothing to run")
        } else if !moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false) {
            Some("running a plan needs the DATABASE_ADMIN permission")
        } else {
            None
        };
        let job_id = match reason {
            Some(reason) => {
                out.insert("jobReason".to_string(), Value::string(reason));
                None
            }
            None => crate::jobs::create_plan_job(
                facade,
                db,
                kind,
                &crate::group_db::text(parameters, "description"),
                crate::group_db::number(parameters, "lockTimeoutMs"),
                &self.statements,
            ),
        };
        out.insert("jobId".to_string(), job_id.map_or(Value::Null, Value::String));
        out
    }
}

/// The online form of a `CREATE [UNIQUE] INDEX` the DDL builders wrote: Postgres `CREATE INDEX CONCURRENTLY IF NOT
/// EXISTS`, which does not block writes; MySQL `ALGORITHM=INPLACE LOCK=NONE`. A statement of another form is as it
/// is.
#[must_use]
pub fn online_create_index(sql: &str, server: Server) -> String {
    if server != Server::Postgres {
        let head = sql.trim_start().to_ascii_uppercase();
        if head.starts_with("CREATE INDEX ") || head.starts_with("CREATE UNIQUE INDEX ") {
            return format!("{} ALGORITHM=INPLACE LOCK=NONE", sql.trim_end());
        }
        return sql.to_string();
    }
    for (from, to) in [
        (
            "CREATE INDEX IF NOT EXISTS ",
            "CREATE INDEX CONCURRENTLY IF NOT EXISTS ",
        ),
        (
            "CREATE UNIQUE INDEX IF NOT EXISTS ",
            "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS ",
        ),
        ("CREATE INDEX ", "CREATE INDEX CONCURRENTLY IF NOT EXISTS "),
        (
            "CREATE UNIQUE INDEX ",
            "CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS ",
        ),
    ] {
        if let Some(rest) = sql.strip_prefix(from) {
            return format!("{to}{rest}");
        }
    }
    sql.to_string()
}

/// The online drop of an index: Postgres `DROP INDEX CONCURRENTLY IF EXISTS`, MySQL `ALGORITHM=INPLACE LOCK=NONE`.
#[must_use]
pub fn online_drop_index(dialect: &Dialect, server: Server, table: &str, index: &str) -> String {
    if server == Server::Postgres {
        format!("DROP INDEX CONCURRENTLY IF EXISTS {}", quote_ident(dialect, index))
    } else {
        format!(
            "DROP INDEX {} ON {} ALGORITHM=INPLACE LOCK=NONE",
            quote_ident(dialect, index),
            quote_ident(dialect, table)
        )
    }
}

/// The drop of a foreign key.
#[must_use]
pub fn drop_foreign_key(dialect: &Dialect, server: Server, table: &str, key: &str) -> String {
    if server == Server::Postgres {
        format!(
            "ALTER TABLE {} DROP CONSTRAINT IF EXISTS {}",
            quote_ident(dialect, table),
            quote_ident(dialect, key)
        )
    } else {
        format!(
            "ALTER TABLE {} DROP FOREIGN KEY {}, ALGORITHM=INPLACE, LOCK=NONE",
            quote_ident(dialect, table),
            quote_ident(dialect, key)
        )
    }
}

/// The plan's statements for one table's differences, of the `changes` asked for, limited to `only` (index and key
/// names, lower case) when it is not empty.
pub(crate) fn plan_table(
    plan: &mut Plan,
    table: &TableDiff,
    dialect: &Dialect,
    server: Server,
    changes: &BTreeSet<String>,
    only: &BTreeSet<String>,
    expected_ddl: &dyn Fn(&str, &str) -> Option<String>,
) {
    let wanted = |name: &str| only.is_empty() || only.contains(&name.to_ascii_lowercase());
    let entity = table.entity_name.as_str();
    let table_name = table.table_name.as_str();
    if table.missing_table {
        plan.warn(format!(
            "{entity} has no table; the table check (Check Tables) creates it with its indexes and keys"
        ));
        return;
    }
    if changes.contains("missingIndexes") {
        for index in table.missing_indexes.iter().filter(|index| wanted(&index.name)) {
            let mut previous: Option<String> = None;
            for sql in &index.ddl {
                let online = online_create_index(sql, server);
                if online.to_ascii_uppercase().contains(" ADD COLUMN ") {
                    plan.warn(format!(
                        "Adding the full-text column of {table_name} rewrites the table under an exclusive lock"
                    ));
                }
                if server != Server::Postgres && index.kind == IndexKind::FullText {
                    plan.warn(format!(
                        "A full-text index on {table_name} is built with the table locked against writes on MySQL"
                    ));
                }
                let what = format!("create index {}", index.name);
                previous = Some(plan.add(online, previous, entity, table_name, what));
            }
            if index.ddl.is_empty() {
                plan.warn(format!(
                    "The DDL of index {} of {entity} could not be built",
                    index.name
                ));
            }
        }
    }
    if changes.contains("invalidIndexes") {
        for index in table.invalid_indexes.iter().filter(|index| wanted(&index.name)) {
            // An index the model has is dropped and made again; another is rebuilt as it is defined.
            let remake = expected_ddl(entity, &index.name).map(|sql| online_create_index(&sql, server));
            match remake {
                Some(create) => {
                    let drop = online_drop_index(dialect, server, table_name, &index.name);
                    let what = format!("drop invalid index {}", index.name);
                    let dropped = plan.add(drop, None, entity, table_name, what);
                    plan.add(
                        create,
                        Some(dropped),
                        entity,
                        table_name,
                        format!("create index {}", index.name),
                    );
                }
                None => {
                    let rebuild = format!("REINDEX INDEX CONCURRENTLY {}", quote_ident(dialect, &index.name));
                    plan.add(
                        rebuild,
                        None,
                        entity,
                        table_name,
                        format!("rebuild invalid index {}", index.name),
                    );
                }
            }
        }
    }
    if changes.contains("extraIndexes") {
        for index in table.extra_indexes.iter().filter(|index| wanted(&index.name)) {
            // a unique index on InnoDB is a unique constraint: never dropped as an extra index
            if index.unique && server != Server::Postgres {
                plan.warn(format!(
                    "{} is a unique constraint on MySQL; it is not dropped",
                    index.name
                ));
                continue;
            }
            let drop = online_drop_index(dialect, server, table_name, &index.name);
            plan.add(drop, None, entity, table_name, format!("drop index {}", index.name));
        }
    }
    if changes.contains("missingForeignKeys") {
        for key in table.missing_foreign_keys.iter().filter(|key| wanted(&key.name)) {
            let Some(sql) = &key.ddl else {
                plan.warn(format!(
                    "The DDL of foreign key {} of {entity} could not be built",
                    key.name
                ));
                continue;
            };
            if server == Server::Postgres {
                // NOT VALID takes its lock only to add the key; VALIDATE reads the rows under a light lock
                let added = plan.add(
                    format!("{sql} NOT VALID"),
                    None,
                    entity,
                    table_name,
                    format!("add foreign key {}", key.name),
                );
                plan.add(
                    format!(
                        "ALTER TABLE {} VALIDATE CONSTRAINT {}",
                        quote_ident(dialect, table_name),
                        quote_ident(dialect, &physical_name(dialect, &key.name))
                    ),
                    Some(added),
                    entity,
                    table_name,
                    format!("validate foreign key {}", key.name),
                );
            } else {
                plan.warn(format!(
                    "Adding a foreign key on MySQL copies {table_name}, which takes as long as the table is big"
                ));
                plan.add(
                    sql.clone(),
                    None,
                    entity,
                    table_name,
                    format!("add foreign key {}", key.name),
                );
            }
        }
    }
    if changes.contains("extraForeignKeys") {
        for key in table.extra_foreign_keys.iter().filter(|key| wanted(&key.name)) {
            let drop = drop_foreign_key(dialect, server, table_name, &key.name);
            plan.add(drop, None, entity, table_name, format!("drop foreign key {}", key.name));
        }
    }
}

/// `plan#SchemaChanges`: the statements that bring the selected differences in line, in the online forms (§5.2). In:
/// `db`, `entityNames` (every entity of the group when empty), `changes` (some of [`CHANGES`]; all but
/// `extraIndexes`, `extraForeignKeys` and `dropForeignKeys` when empty), `names` (only these index and key names: one
/// row). Out: `statements` (each `statementSeqId`, `statementText`, `dependsOnSeqId`, `entityName`, `tableName`,
/// `what`, `lockTimeout`), `warnings`, `checked`.
pub(crate) fn plan_schema_changes(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of_group(parameters, facade) else {
        return Ok(Out::new());
    };
    let mut changes: BTreeSet<String> = list(parameters, "changes").into_iter().collect();
    if let Some(unknown) = changes.iter().find(|change| !CHANGES.contains(&change.as_str())) {
        add_error(format!("unknown change {unknown}; changes are {}", CHANGES.join(", ")));
        return Ok(Out::new());
    }
    if changes.is_empty() {
        changes = ["missingIndexes", "invalidIndexes", "missingForeignKeys"]
            .iter()
            .map(|change| (*change).to_string())
            .collect();
    }
    let only: BTreeSet<String> = list(parameters, "names")
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    let names = selected(parameters, facade, &db);
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let diff = match facade.meta().compare(&refs) {
        Ok(diff) => diff,
        Err(err) => {
            add_error(err.to_string());
            return Ok(Out::new());
        }
    };
    let mut plan = Plan::default();
    for error in &diff.errors {
        plan.warn(error.clone());
    }
    for table in diff.tables.iter().filter(|table| table.group == db.group) {
        let expected = |entity: &str, index: &str| expected_index_ddl(facade, entity, index);
        plan_table(&mut plan, table, &db.dialect, db.server, &changes, &only, &expected);
    }
    if changes.contains("dropForeignKeys") {
        plan_drop_all_keys(&mut plan, facade, &db, &names, &only);
    }
    if plan
        .statements
        .iter()
        .any(|statement| takes_strong_lock(&statement.statement_text))
    {
        plan.warn("Some statements take a lock that blocks writes; each waits at most the job's lock timeout");
    }
    let mut out = plan.out_with_job(facade, &db, "DbjDdl", parameters);
    out.insert(
        "checked".to_string(),
        Value::I64(i64::try_from(diff.checked).unwrap_or(i64::MAX)),
    );
    out.insert("db".to_string(), Value::string(db.db_text()));
    Ok(out)
}

/// The `CREATE INDEX` the table check makes for the index named `index` of `entity`: an `index` element, or a foreign
/// key's index.
pub(crate) fn expected_index_ddl(facade: &EntityFacade, entity: &str, index: &str) -> Option<String> {
    facade
        .with_entity(entity, |entity, dialect, _| {
            if let Some(found) = entity
                .indexes
                .iter()
                .find(|found| found.name.eq_ignore_ascii_case(index))
            {
                return Ok(create_index(entity, found, dialect).ok());
            }
            Ok(entity
                .relationships
                .iter()
                .filter(|relationship| key_relationship(relationship))
                .find(|relationship| {
                    moqui_db::sql::foreign_key_index_name(entity, relationship, dialect).eq_ignore_ascii_case(index)
                })
                .and_then(|relationship| create_fk_index(entity, relationship, dialect).ok()))
        })
        .ok()
        .flatten()
}

/// Every foreign key of the selected entities' tables, dropped: before a bulk load, say, with the missing keys planned
/// again after it.
fn plan_drop_all_keys(plan: &mut Plan, facade: &EntityFacade, db: &GroupDb, names: &[String], only: &BTreeSet<String>) {
    let tables: BTreeMap<String, String> = names
        .iter()
        .filter_map(|name| {
            facade
                .with_entity(name, |entity, dialect, _| {
                    Ok((!entity.is_view).then(|| {
                        (
                            physical_name(dialect, &entity.table_name).to_ascii_lowercase(),
                            entity.full_name.clone(),
                        )
                    }))
                })
                .ok()
                .flatten()
        })
        .collect();
    let Some(sql) = moqui_db::sql::catalog::schema_catalog(&db.dialect) else {
        return;
    };
    let rows = match db.query(sql.foreign_keys, &[]) {
        Ok(rows) => rows,
        Err(err) => {
            plan.warn(format!("Could not read the foreign keys: {err}"));
            return;
        }
    };
    for row in rows {
        let table = cell_text(row.get("table_name"));
        let key = cell_text(row.get("constraint_name"));
        let Some(entity) = tables.get(&table.to_ascii_lowercase()) else {
            continue;
        };
        if !only.is_empty() && !only.contains(&key.to_ascii_lowercase()) {
            continue;
        }
        let drop = drop_foreign_key(&db.dialect, db.server, &table, &key);
        plan.add(drop, None, entity, &table, format!("drop foreign key {key}"));
    }
}

/// Postgres: each foreign key of the current schema with its definition, to make it again as it was.
const POSTGRES_KEY_DEFINITIONS: &str = "SELECT t.relname AS table_name, c.conname AS constraint_name, \
     pg_get_constraintdef(c.oid) AS definition FROM pg_constraint c JOIN pg_class t ON t.oid = c.conrelid \
     JOIN pg_namespace n ON n.oid = t.relnamespace WHERE c.contype = 'f' AND n.nspname = current_schema() \
     ORDER BY t.relname, c.conname";

/// `plan#LoadWrap`: the two jobs that wrap a bulk load (Data › Load, Snapshots): one that drops every foreign key of
/// the selected entities' tables, and one that makes them again after the load. On Postgres a key is made again from
/// its own definition, `NOT VALID` then `VALIDATE CONSTRAINT`; on MySQL from the model. In: `db`, `entityNames`,
/// `description`, `lockTimeoutMs`. Out: `dropStatements`, `restoreStatements` (plan statements), `warnings`,
/// `dropJobId`, `restoreJobId` (null, with `jobReason`, as for `plan#SchemaChanges`).
pub(crate) fn plan_load_wrap(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of_group(parameters, facade) else {
        return Ok(Out::new());
    };
    let names = selected(parameters, facade, &db);
    let mut drop = Plan::default();
    plan_drop_all_keys(&mut drop, facade, &db, &names, &BTreeSet::new());
    let mut restore = Plan::default();
    if db.is_postgres() {
        let tables: BTreeMap<String, String> = drop
            .statements
            .iter()
            .map(|statement| (statement.table_name.to_ascii_lowercase(), statement.entity_name.clone()))
            .collect();
        match db.query(POSTGRES_KEY_DEFINITIONS, &[]) {
            Ok(rows) => {
                for row in rows {
                    let table = cell_text(row.get("table_name"));
                    let Some(entity) = tables.get(&table.to_ascii_lowercase()) else {
                        continue;
                    };
                    let key = cell_text(row.get("constraint_name"));
                    let definition = cell_text(row.get("definition"));
                    let quoted_table = quote_ident(&db.dialect, &table);
                    let quoted_key = quote_ident(&db.dialect, &key);
                    let definition = definition.trim_end_matches(" NOT VALID");
                    let added = restore.add(
                        format!("ALTER TABLE {quoted_table} ADD CONSTRAINT {quoted_key} {definition} NOT VALID"),
                        None,
                        entity,
                        &table,
                        format!("add foreign key {key}"),
                    );
                    restore.add(
                        format!("ALTER TABLE {quoted_table} VALIDATE CONSTRAINT {quoted_key}"),
                        Some(added),
                        entity,
                        &table,
                        format!("validate foreign key {key}"),
                    );
                }
            }
            Err(err) => drop.warn(format!("Could not read the foreign keys' definitions: {err}")),
        }
    } else {
        for name in &names {
            let built = facade.with_entity(name, |entity, dialect, model| {
                let mut statements = Vec::new();
                if entity.is_view {
                    return Ok(statements);
                }
                for relationship in entity.relationships.iter().filter(|r| key_relationship(r)) {
                    let Ok(related) = model.entity(&relationship.related) else {
                        continue;
                    };
                    if related.is_view || entity.group != related.group {
                        continue;
                    }
                    if let Ok(sql) = create_foreign_key(entity, relationship, related, dialect) {
                        let what = format!("add foreign key to {}", related.full_name);
                        statements.push((sql, physical_name(dialect, &entity.table_name), what));
                    }
                }
                Ok(statements)
            });
            for (sql, table, what) in built.unwrap_or_default() {
                restore.add(sql, None, name, &table, what);
            }
        }
        restore.warn("Adding a foreign key on MySQL copies the table, which takes as long as the table is big");
    }
    let description = crate::group_db::text(parameters, "description");
    let with = |suffix: &str| {
        let mut parameters = parameters.clone();
        parameters.insert(
            "description".to_string(),
            Value::string(format!("{description} ({suffix})").trim().to_string()),
        );
        parameters
    };
    let drop_out = drop.out_with_job(facade, &db, "DbjLoadWrap", &with("drop foreign keys"));
    let restore_out = restore.out_with_job(facade, &db, "DbjLoadWrap", &with("restore foreign keys"));
    let mut out = Out::new();
    let take = |out: &Out, key: &str| out.get(key).cloned().unwrap_or(Value::Null);
    out.insert("dropStatements".to_string(), take(&drop_out, "statements"));
    out.insert("restoreStatements".to_string(), take(&restore_out, "statements"));
    out.insert("dropJobId".to_string(), take(&drop_out, "jobId"));
    out.insert("restoreJobId".to_string(), take(&restore_out, "jobId"));
    let mut warnings = drop.warnings.clone();
    warnings.extend(restore.warnings.iter().cloned());
    out.insert(
        "warnings".to_string(),
        Value::List(warnings.into_iter().map(Value::String).collect()),
    );
    if let Some(reason) = drop_out.get("jobReason").or_else(|| restore_out.get("jobReason")) {
        out.insert("jobReason".to_string(), reason.clone());
    }
    out.insert("db".to_string(), Value::string(db.db_text()));
    Ok(out)
}

/// `get#SchemaDdl`: the DDL of `entityNames` (every entity of the group when empty) in the database's dialect, from
/// the builders the table check uses: each `CREATE TABLE` (with its full-text columns), then the indexes, then the
/// foreign keys among the selected entities. Out: `ddlText`, `fileName`, `entityCount`.
pub(crate) fn get_schema_ddl(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of_group(parameters, facade) else {
        return Ok(Out::new());
    };
    let names = selected(parameters, facade, &db);
    let (text, count) = schema_ddl(facade, &names);
    let mut out = Out::new();
    out.insert("ddlText".to_string(), Value::string(text));
    out.insert(
        "fileName".to_string(),
        Value::string(format!("schema-{}.sql", db.group)),
    );
    out.insert(
        "entityCount".to_string(),
        Value::I64(i64::try_from(count).unwrap_or(i64::MAX)),
    );
    Ok(out)
}

/// The DDL text of `names` and how many tables it makes; a statement that cannot be built is a comment naming why.
pub fn schema_ddl(facade: &EntityFacade, names: &[String]) -> (String, usize) {
    let selected: BTreeSet<String> = names
        .iter()
        .filter_map(|name| moqui_core::entity::info::entity_full_name(facade, name))
        .collect();
    let mut tables = String::new();
    let mut indexes = String::new();
    let mut keys = String::new();
    let mut count = 0;
    for name in &selected {
        let built = facade.with_entity(name, |entity, dialect, model| {
            if entity.is_view {
                return Ok(false);
            }
            statement(&mut tables, create_table(entity, model, dialect), &entity.full_name);
            for index in entity.indexes.iter().filter(|index| !index.fields.is_empty()) {
                statement(&mut indexes, create_index(entity, index, dialect), &entity.full_name);
            }
            for relationship in entity.relationships.iter().filter(|r| key_relationship(r)) {
                if dialect.use_foreign_key_indexes {
                    statement(
                        &mut indexes,
                        create_fk_index(entity, relationship, dialect),
                        &entity.full_name,
                    );
                }
                let Ok(related) = model.entity(&relationship.related) else {
                    continue;
                };
                if dialect.use_foreign_keys && !related.is_view && selected.contains(&related.full_name) {
                    statement(
                        &mut keys,
                        create_foreign_key(entity, relationship, related, dialect),
                        &entity.full_name,
                    );
                }
            }
            Ok(true)
        });
        match built {
            Ok(true) => count += 1,
            Ok(false) => {}
            Err(err) => {
                tables.push_str(&format!("-- {name}: {err}\n"));
            }
        }
    }
    let mut text = String::new();
    text.push_str(&format!("-- DDL of {count} entities, from the entity model\n\n"));
    text.push_str(&tables);
    if !indexes.is_empty() {
        text.push_str("\n-- indexes\n");
        text.push_str(&indexes);
    }
    if !keys.is_empty() {
        text.push_str("\n-- foreign keys\n");
        text.push_str(&keys);
    }
    (text, count)
}

fn statement(out: &mut String, sql: Result<String, LibError>, entity: &str) {
    match sql {
        Ok(sql) => {
            out.push_str(sql.trim_end());
            out.push_str(";\n");
        }
        Err(err) => out.push_str(&format!("-- {entity}: {err}\n")),
    }
}

fn key_relationship(relationship: &Relationship) -> bool {
    !relationship.is_auto_reverse && relationship.relationship_type == "one" && !relationship.key_pairs.is_empty()
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, compare_schema)?;
    rust_service!(map, plan_schema_changes)?;
    rust_service!(map, plan_load_wrap)?;
    rust_service!(map, get_schema_ddl)?;
    Ok(())
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
