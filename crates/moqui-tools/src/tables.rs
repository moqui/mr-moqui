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

//! Tables (plans/archive/Tools-Database.md §7.4): sizes, estimates and maintenance times from one catalog query per
//! dialect (`get#TableStats`), an exact count of one table on request (`count#TableRows`), and maintenance planned as
//! the statements of a job (`plan#TableMaintenance`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use moqui_db::sql::{physical_name, quote_ident};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::entity::EntityFacade;

use crate::group_db::{add_error, cell_number, cell_text, flag, list, text, GroupDb, Out, Server};
use crate::schema::Plan;

/// Postgres: each table of the current schema, largest first.
pub const POSTGRES_TABLE_STATS: &str = "SELECT c.relname AS table_name, \
     CAST(GREATEST(c.reltuples, 0) AS BIGINT) AS estimated_rows, pg_total_relation_size(c.oid) AS total_bytes, \
     pg_relation_size(c.oid) AS table_bytes, pg_indexes_size(c.oid) AS index_bytes, s.n_live_tup AS live_rows, \
     s.n_dead_tup AS dead_rows, s.last_vacuum, s.last_autovacuum, s.last_analyze, s.last_autoanalyze, \
     s.seq_scan, s.idx_scan, s.n_mod_since_analyze AS modified_since_analyze \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     LEFT JOIN pg_stat_user_tables s ON s.relid = c.oid \
     WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'p') \
     ORDER BY pg_total_relation_size(c.oid) DESC";

/// MySQL and MariaDB: each base table of the database, largest first; `TABLE_ROWS` is InnoDB's estimate.
pub const MYSQL_TABLE_STATS: &str = "SELECT TABLE_NAME AS table_name, TABLE_ROWS AS estimated_rows, \
     DATA_LENGTH + INDEX_LENGTH AS total_bytes, DATA_LENGTH AS table_bytes, INDEX_LENGTH AS index_bytes, \
     DATA_FREE AS free_bytes, UPDATE_TIME AS update_time, CHECK_TIME AS check_time, ENGINE AS engine \
     FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
     ORDER BY DATA_LENGTH + INDEX_LENGTH DESC";

/// The table named `?` in the current schema, as the catalog spells it.
const POSTGRES_TABLE: &str = "SELECT c.relname AS table_name FROM pg_class c \
     JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = current_schema() \
     AND c.relkind IN ('r', 'p', 'm') AND c.relname = ?";
const MYSQL_TABLE: &str = "SELECT TABLE_NAME AS table_name FROM information_schema.TABLES \
     WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' AND TABLE_NAME = ?";

/// The maintenance actions, and the servers each runs on.
pub const ACTIONS: &[(&str, &[Server])] = &[
    ("vacuumAnalyze", &[Server::Postgres]),
    ("analyze", &[Server::Postgres, Server::Mysql, Server::Mariadb]),
    ("reindex", &[Server::Postgres]),
    ("vacuumFull", &[Server::Postgres]),
    ("cluster", &[Server::Postgres]),
    ("optimize", &[Server::Mysql, Server::Mariadb]),
];

/// The entity of each table of `group`, by lower-case table name.
fn table_entities(facade: &EntityFacade, group: &str) -> HashMap<String, String> {
    let default_group = facade.default_group().to_string();
    let Some(dialect) = facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, dialect, _)| dialect.clone())
    else {
        return HashMap::new();
    };
    facade.with_model(|model| {
        model
            .entities()
            .iter()
            .filter(|entity| !entity.is_view && entity.group.as_deref().unwrap_or(&default_group) == group)
            .map(|entity| {
                (
                    physical_name(&dialect, &entity.table_name).to_ascii_lowercase(),
                    entity.full_name.clone(),
                )
            })
            .collect()
    })
}

/// `get#TableStats`: each table's estimated rows, total, table and index size, dead rows, last vacuum and analyze
/// (automatic and not), and sequential against index scans (Postgres), or InnoDB's estimates and free space (MySQL),
/// largest first, from one catalog query, with the entity of the table. Out: `tables` (List of Map, keys as the
/// query's columns plus `entity_name`), `reason` (when the query failed).
pub(crate) fn get_table_stats(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(Out::new());
    };
    let sql = if db.is_postgres() {
        POSTGRES_TABLE_STATS
    } else {
        MYSQL_TABLE_STATS
    };
    let mut out = Out::new();
    let entities = table_entities(facade, &db.group);
    match db.query(sql, &[]) {
        Ok(rows) => {
            let tables = rows
                .into_iter()
                .map(|mut row| {
                    let table = cell_text(row.get("table_name")).to_ascii_lowercase();
                    let entity = entities.get(&table).map_or(Value::Null, Value::string);
                    row.insert("entity_name".to_string(), entity);
                    Value::Map(row)
                })
                .collect();
            out.insert("tables".to_string(), Value::List(tables));
        }
        Err(err) => {
            out.insert("tables".to_string(), Value::List(Vec::new()));
            out.insert("reason".to_string(), Value::string(err.to_string()));
        }
    }
    Ok(out)
}

/// The table `name` as the catalog spells it, or `None` (with an error added) when the schema has no such table.
fn known_table(db: &GroupDb, name: &str) -> Option<String> {
    let sql = if db.is_postgres() { POSTGRES_TABLE } else { MYSQL_TABLE };
    let candidates = [name.to_string(), name.to_ascii_lowercase()];
    for candidate in candidates.iter().collect::<BTreeSet<_>>() {
        match db.query(sql, &[Value::string(candidate)]) {
            Ok(rows) if !rows.is_empty() => return Some(cell_text(rows[0].get("table_name"))),
            Ok(_) => {}
            Err(err) => {
                add_error(format!("Could not look up table {name}: {err}"));
                return None;
            }
        }
    }
    add_error(format!("no table named {name}"));
    None
}

/// `count#TableRows`: the exact row count of one table. In: `db`, `tableName`. Out: `rowCount`, `tableName`.
pub(crate) fn count_table_rows(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(Out::new());
    };
    let Some(table) = known_table(&db, &text(parameters, "tableName")) else {
        return Ok(Out::new());
    };
    let sql = format!("SELECT COUNT(*) AS row_count FROM {}", quote_ident(&db.dialect, &table));
    let mut out = Out::new();
    match db.query(&sql, &[]) {
        Ok(rows) => {
            let count = cell_number(rows.first().and_then(|row| row.get("row_count"))).unwrap_or(0);
            out.insert("rowCount".to_string(), Value::I64(count));
            out.insert("tableName".to_string(), Value::string(table));
        }
        Err(err) => add_error(format!("Could not count the rows of {table}: {err}")),
    }
    Ok(out)
}

/// The statement of `action` on `table` (quoted), and the warning it carries.
#[must_use]
pub fn maintenance_statement(action: &str, table: &str) -> Option<(String, Option<&'static str>)> {
    Some(match action {
        "vacuumAnalyze" => (format!("VACUUM (ANALYZE) {table}"), None),
        "analyze" => (format!("ANALYZE {table}"), None),
        "reindex" => (format!("REINDEX TABLE CONCURRENTLY {table}"), None),
        "vacuumFull" => (
            format!("VACUUM (FULL, ANALYZE) {table}"),
            Some("VACUUM FULL rewrites the table under an ACCESS EXCLUSIVE lock: no reads or writes until it ends"),
        ),
        "cluster" => (
            format!("CLUSTER {table}"),
            Some("CLUSTER rewrites the table under an ACCESS EXCLUSIVE lock, and needs a clustered index set before"),
        ),
        _ => return None,
    })
}

/// The MySQL statement of `action`.
fn mysql_statement(action: &str, table: &str) -> Option<(String, Option<&'static str>)> {
    Some(match action {
        "analyze" => (format!("ANALYZE TABLE {table}"), None),
        "optimize" => (
            format!("OPTIMIZE TABLE {table}"),
            Some("OPTIMIZE TABLE rebuilds the table; writes wait at its start and end"),
        ),
        _ => return None,
    })
}

/// `plan#TableMaintenance`: the statements of a maintenance job on `tableNames`. In: `db`, `tableNames`, `action` (one
/// of [`ACTIONS`]: `vacuumAnalyze`, `analyze`, `reindex`, `vacuumFull`, `cluster` on Postgres; `analyze`, `optimize`
/// on MySQL and MariaDB), `allowExclusiveLock` (needed for `vacuumFull` and `cluster`). Out: `statements`, `warnings`,
/// `exclusiveLock` (Boolean), as `plan#SchemaChanges`; `create#DatabaseJob` with `kindEnumId` `DbjMaintenance` runs
/// them.
pub(crate) fn plan_table_maintenance(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of_group(parameters, facade) else {
        return Ok(Out::new());
    };
    let action = text(parameters, "action");
    let Some((_, servers)) = ACTIONS.iter().find(|(name, _)| *name == action) else {
        let names: Vec<&str> = ACTIONS.iter().map(|(name, _)| *name).collect();
        add_error(format!(
            "unknown maintenance action {action:?}; the actions are {}",
            names.join(", ")
        ));
        return Ok(Out::new());
    };
    if !servers.contains(&db.server) {
        add_error(format!("{action} does not run on this database"));
        return Ok(Out::new());
    }
    let exclusive = matches!(action.as_str(), "vacuumFull" | "cluster");
    if exclusive && !flag(parameters, "allowExclusiveLock") {
        add_error(format!(
            "{action} takes an ACCESS EXCLUSIVE lock on each table; set allowExclusiveLock to plan it"
        ));
        return Ok(Out::new());
    }
    let tables = list(parameters, "tableNames");
    if tables.is_empty() {
        add_error("Maintenance needs tableNames");
        return Ok(Out::new());
    }
    let entities = table_entities(facade, &db.group);
    let mut plan = Plan::default();
    for name in tables {
        let Some(table) = known_table(&db, &name) else {
            continue;
        };
        let quoted = quote_ident(&db.dialect, &table);
        let statement = if db.is_postgres() {
            maintenance_statement(&action, &quoted)
        } else {
            mysql_statement(&action, &quoted)
        };
        let Some((sql, warning)) = statement else {
            continue;
        };
        if let Some(warning) = warning {
            plan.warn(warning);
        }
        let entity = entities.get(&table.to_ascii_lowercase()).cloned().unwrap_or_default();
        plan.add(sql, None, &entity, &table, format!("{action} {table}"));
    }
    let mut out = plan.out_with_job(facade, &db, "DbjMaintenance", parameters);
    out.insert("exclusiveLock".to_string(), Value::Bool(exclusive));
    out.insert("db".to_string(), Value::string(db.db_text()));
    out.insert("kindEnumId".to_string(), Value::string("DbjMaintenance"));
    Ok(out)
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, get_table_stats)?;
    rust_service!(map, count_table_rows)?;
    rust_service!(map, plan_table_maintenance)?;
    Ok(())
}
