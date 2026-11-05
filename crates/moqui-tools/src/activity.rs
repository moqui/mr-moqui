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

//! Activity (plans/Tools-Database.md §7.3): the database's sessions with their state, waits and statement, the app's
//! own marked by the application name the drivers give them, and the blocking chains as trees
//! (`get#DatabaseActivity`); cancelling and ending a session (`cancel#DatabaseSession`, `terminate#DatabaseSession`).
//! And the index health queries of §4.2 (`get#IndexHealth`): foreign keys with no index, unused, duplicate and
//! invalid indexes, and transaction id wraparound.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::entity::EntityFacade;

use crate::group_db::{add_error, cell_number, cell_text, number, require_admin, rows_value, GroupDb, Out, Server};

/// Postgres: the sessions of this database, with what blocks each.
pub const POSTGRES_ACTIVITY: &str = "SELECT a.pid, a.usename AS user_name, a.application_name, \
     CAST(a.client_addr AS TEXT) AS client_addr, a.backend_type, a.state, a.wait_event_type, a.wait_event, \
     a.backend_start, a.xact_start, a.query_start, a.state_change, \
     CAST(EXTRACT(EPOCH FROM now() - a.query_start) * 1000 AS BIGINT) AS query_ms, \
     CAST(EXTRACT(EPOCH FROM now() - a.xact_start) * 1000 AS BIGINT) AS xact_ms, \
     CAST(EXTRACT(EPOCH FROM now() - a.state_change) * 1000 AS BIGINT) AS state_ms, \
     array_to_string(pg_blocking_pids(a.pid), ',') AS blocked_by, a.query, a.pid = pg_backend_pid() AS is_self \
     FROM pg_stat_activity a WHERE a.datname = current_database() ORDER BY a.query_start NULLS LAST";

/// MySQL and MariaDB: the sessions of the server.
pub const MYSQL_ACTIVITY: &str = "SELECT ID AS pid, USER AS user_name, HOST AS client_addr, DB AS database_name, \
     COMMAND AS command, TIME AS time_seconds, STATE AS state, INFO AS query, ID = CONNECTION_ID() AS is_self \
     FROM information_schema.PROCESSLIST ORDER BY TIME DESC";
/// MySQL 8: who waits on whom.
pub const MYSQL_LOCK_WAITS: &str = "SELECT waiting_pid AS pid, blocking_pid AS blocking_pid FROM sys.innodb_lock_waits";
/// MariaDB: who waits on whom.
pub const MARIADB_LOCK_WAITS: &str = "SELECT r.trx_mysql_thread_id AS pid, b.trx_mysql_thread_id AS blocking_pid \
     FROM information_schema.INNODB_LOCK_WAITS w \
     JOIN information_schema.INNODB_TRX r ON r.trx_id = w.requesting_trx_id \
     JOIN information_schema.INNODB_TRX b ON b.trx_id = w.blocking_trx_id";
/// MySQL and MariaDB: each session's `application_name` connection attribute.
pub const MYSQL_APPLICATION_NAMES: &str = "SELECT PROCESSLIST_ID AS pid, ATTR_VALUE AS application_name \
     FROM performance_schema.session_connect_attrs WHERE ATTR_NAME = 'application_name'";

/// Idle in a transaction this long is marked.
pub const IDLE_IN_TRANSACTION_MS: i64 = 60_000;
/// Running this long is marked.
pub const LONG_RUNNING_MS: i64 = 10_000;

/// The pids each session waits on, from `blocked_by` (`1,2`) or the lock waits.
fn parse_pids(text: &str) -> Vec<i64> {
    text.split(',').filter_map(|pid| pid.trim().parse().ok()).collect()
}

/// The blocking chains as trees: each session that blocks another and is blocked by none is a root, with the sessions
/// it blocks under it (`pid`, `blocks`). A cycle is cut where it comes back.
#[must_use]
pub fn blocking_trees(blocked_by: &BTreeMap<i64, Vec<i64>>) -> Vec<Value> {
    let mut blocks: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for (pid, blockers) in blocked_by {
        for blocker in blockers {
            blocks.entry(*blocker).or_default().insert(*pid);
        }
    }
    let waiting: BTreeSet<i64> = blocked_by
        .iter()
        .filter(|(_, blockers)| !blockers.is_empty())
        .map(|(pid, _)| *pid)
        .collect();
    let mut roots: Vec<i64> = blocks.keys().filter(|pid| !waiting.contains(pid)).copied().collect();
    if roots.is_empty() {
        // every blocker also waits: a cycle (a deadlock the server has not broken yet); start at the lowest
        roots.extend(blocks.keys().next().copied());
    }
    fn tree(pid: i64, blocks: &BTreeMap<i64, BTreeSet<i64>>, seen: &mut BTreeSet<i64>) -> Value {
        let mut node = IndexMap::new();
        node.insert("pid".to_string(), Value::I64(pid));
        let mut children = Vec::new();
        if seen.insert(pid) {
            for child in blocks.get(&pid).into_iter().flatten() {
                children.push(tree(*child, blocks, seen));
            }
        }
        node.insert("blocks".to_string(), Value::List(children));
        Value::Map(node)
    }
    let mut seen = BTreeSet::new();
    roots.into_iter().map(|root| tree(root, &blocks, &mut seen)).collect()
}

/// `get#DatabaseActivity`: the database's sessions and the blocking chains. Out: `sessions` (List of Map: the query's
/// columns, plus `own_app` (this instance's sessions, by the `application_name` its drivers set to the instance id),
/// `idle_in_transaction_long` and `running_long` marks, and `blocked_by` as a list), `blockingChains` (List of trees: `pid`, `blocks`), `instanceName`
/// (this instance's application name), `reasons` (Map).
pub(crate) fn get_database_activity(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(Out::new());
    };
    let own_name = moqui_core::driver::application_name().map(|name| name.to_string());
    let mut reasons = IndexMap::new();
    let (mut rows, waits, app_names) = if db.is_postgres() {
        let rows = db.query_or_note(POSTGRES_ACTIVITY, &[], "sessions", &mut reasons);
        (rows, HashMap::new(), HashMap::new())
    } else {
        let rows = db.query_or_note(MYSQL_ACTIVITY, &[], "sessions", &mut reasons);
        let waits_sql = if db.server == Server::Mariadb {
            MARIADB_LOCK_WAITS
        } else {
            MYSQL_LOCK_WAITS
        };
        let mut waits: HashMap<i64, Vec<i64>> = HashMap::new();
        for row in db.query_or_note(waits_sql, &[], "lockWaits", &mut reasons) {
            if let (Some(pid), Some(blocker)) = (cell_number(row.get("pid")), cell_number(row.get("blocking_pid"))) {
                waits.entry(pid).or_default().push(blocker);
            }
        }
        let names: HashMap<i64, String> = db
            .query_or_note(MYSQL_APPLICATION_NAMES, &[], "applicationNames", &mut reasons)
            .into_iter()
            .filter_map(|row| Some((cell_number(row.get("pid"))?, cell_text(row.get("application_name")))))
            .collect();
        (rows, waits, names)
    };
    let mut blocked: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for row in &mut rows {
        let Some(pid) = cell_number(row.get("pid")) else {
            continue;
        };
        let blockers = if db.is_postgres() {
            parse_pids(&cell_text(row.get("blocked_by")))
        } else {
            waits.get(&pid).cloned().unwrap_or_default()
        };
        if let Some(name) = app_names.get(&pid) {
            row.insert("application_name".to_string(), Value::string(name));
        }
        let name = cell_text(row.get("application_name"));
        let own = own_name.as_deref().is_some_and(|own| own == name);
        let state = cell_text(row.get("state")).to_ascii_lowercase();
        let state_ms = cell_number(row.get("state_ms"))
            .or_else(|| cell_number(row.get("time_seconds")).map(|seconds| seconds * 1000))
            .unwrap_or(0);
        let query_ms = cell_number(row.get("query_ms"))
            .or_else(|| cell_number(row.get("time_seconds")).map(|seconds| seconds * 1000))
            .unwrap_or(0);
        let command = cell_text(row.get("command")).to_ascii_lowercase();
        let active = state == "active" || (!db.is_postgres() && command != "sleep" && command != "daemon");
        row.insert("own_app".to_string(), Value::Bool(own));
        row.insert(
            "idle_in_transaction_long".to_string(),
            Value::Bool(state.starts_with("idle in transaction") && state_ms >= IDLE_IN_TRANSACTION_MS),
        );
        row.insert(
            "running_long".to_string(),
            Value::Bool(active && query_ms >= LONG_RUNNING_MS),
        );
        row.insert(
            "blocked_by".to_string(),
            Value::List(blockers.iter().map(|pid| Value::I64(*pid)).collect()),
        );
        blocked.insert(pid, blockers);
    }
    let mut out = Out::new();
    out.insert("sessions".to_string(), rows_value(rows));
    out.insert("blockingChains".to_string(), Value::List(blocking_trees(&blocked)));
    out.insert("instanceName".to_string(), own_name.map_or(Value::Null, Value::String));
    out.insert("reasons".to_string(), Value::Map(reasons));
    Ok(out)
}

/// `cancel#DatabaseSession`: cancel the statement of session `pid` (`pg_cancel_backend`, `KILL QUERY`). Out:
/// `signalled` (Boolean: whether the server found the session). Needs `DATABASE_ADMIN`; writes a `DbaCancel` audit row.
pub(crate) fn cancel_database_session(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    signal_session(parameters, facade, false)
}

/// `terminate#DatabaseSession`: end session `pid` (`pg_terminate_backend`, `KILL`). Out: `signalled`. Needs
/// `DATABASE_ADMIN`; writes a `DbaTerminate` audit row.
pub(crate) fn terminate_database_session(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    signal_session(parameters, facade, true)
}

fn signal_session(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
    terminate: bool,
) -> Result<BTreeMap<String, Value>, LibError> {
    let what = if terminate {
        "Ending a database session"
    } else {
        "Cancelling a database session"
    };
    if !crate::service::may_view_tools() || !require_admin(what) {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(Out::new());
    };
    let Some(pid) = number(parameters, "pid").filter(|pid| *pid > 0) else {
        add_error("pid must be a session's id, a positive number");
        return Ok(Out::new());
    };
    let result = crate::jobs::signal_backend(&db, pid, terminate);
    crate::audit::record(
        facade,
        &crate::audit::AuditEntry {
            action: if terminate { "DbaTerminate" } else { "DbaCancel" },
            db: Some(db.db_text()),
            is_change: true,
            error_text: result.as_ref().err().map(ToString::to_string),
            detail_json: Some(format!(r#"{{"pid":{pid}}}"#)),
            ..crate::audit::AuditEntry::default()
        },
    );
    let mut out = Out::new();
    match result {
        Ok(signalled) => {
            out.insert("signalled".to_string(), Value::Bool(signalled));
        }
        Err(err) => add_error(format!("Could not signal session {pid}: {err}")),
    }
    Ok(out)
}

/// Postgres: indexes never scanned since the counters started, not unique and not a primary key's, largest first.
pub const POSTGRES_UNUSED_INDEXES: &str = "SELECT s.relname AS table_name, s.indexrelname AS index_name, \
     pg_relation_size(s.indexrelid) AS index_bytes, s.idx_scan FROM pg_stat_user_indexes s \
     JOIN pg_index x ON x.indexrelid = s.indexrelid WHERE s.schemaname = current_schema() AND s.idx_scan = 0 \
     AND NOT x.indisunique AND NOT x.indisprimary ORDER BY pg_relation_size(s.indexrelid) DESC";
/// Postgres: since when the counters count.
pub const POSTGRES_STATS_SINCE: &str = "SELECT d.stats_reset, pg_postmaster_start_time() AS server_start \
     FROM pg_stat_database d WHERE d.datname = current_database()";
/// Postgres: the transaction id age of each database, oldest first.
pub const POSTGRES_DATABASE_AGE: &str = "SELECT datname AS database_name, age(datfrozenxid) AS xid_age, \
     current_setting('autovacuum_freeze_max_age')::bigint AS freeze_max_age FROM pg_database ORDER BY 2 DESC";
/// Postgres: the tables of the current schema with the oldest transaction ids.
pub const POSTGRES_TABLE_AGE: &str = "SELECT c.relname AS table_name, age(c.relfrozenxid) AS xid_age \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE n.nspname = current_schema() AND c.relkind IN ('r', 'm', 't') ORDER BY 2 DESC LIMIT 20";
/// MySQL and MariaDB (`sys`, MariaDB 10.6 and later): indexes not used since the server started.
pub const MYSQL_UNUSED_INDEXES: &str = "SELECT object_name AS table_name, index_name AS index_name \
     FROM sys.schema_unused_indexes WHERE object_schema = DATABASE()";

/// One index of the catalog, for the duplicate and unindexed checks.
struct IndexRow {
    table: String,
    name: String,
    columns: Vec<String>,
    unique: bool,
    primary: bool,
    valid: bool,
}

/// Indexes whose columns lead another index of the same table (and are not unique), as pghero checks: the other
/// covers every query the first serves.
fn duplicates(indexes: &[IndexRow]) -> Vec<Value> {
    let mut out = Vec::new();
    for index in indexes
        .iter()
        .filter(|index| !index.unique && !index.primary && !index.columns.is_empty())
    {
        let covering = indexes.iter().find(|other| {
            other.table == index.table
                && other.name != index.name
                && other.columns.len() >= index.columns.len()
                && other.columns[..index.columns.len()] == index.columns[..]
                && (other.columns.len() > index.columns.len() || other.name < index.name)
        });
        if let Some(other) = covering {
            let mut row = IndexMap::new();
            row.insert("table_name".to_string(), Value::string(&index.table));
            row.insert("index_name".to_string(), Value::string(&index.name));
            row.insert("columns".to_string(), Value::string(index.columns.join(", ")));
            row.insert("covered_by".to_string(), Value::string(&other.name));
            row.insert(
                "covered_by_columns".to_string(),
                Value::string(other.columns.join(", ")),
            );
            out.push(Value::Map(row));
        }
    }
    out
}

/// `get#IndexHealth`: the catalog checks of §4.2. Out: `unindexedForeignKeys` (Postgres; InnoDB indexes its keys
/// itself), `unusedIndexes`, `duplicateIndexes`, `invalidIndexes`, `databaseAge` and `tableAge` (Postgres
/// wraparound), each a List of Map; `statsSince` (Map: `stats_reset`, `server_start`), `reasons` (Map).
pub(crate) fn get_index_health(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !crate::service::may_view_tools() {
        return Ok(Out::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(Out::new());
    };
    let mut reasons = IndexMap::new();
    let mut out = Out::new();
    let Some(catalog) = moqui_db::sql::catalog::schema_catalog(&db.dialect) else {
        add_error("This database has no index catalog the Tools read");
        return Ok(out);
    };
    let indexes: Vec<IndexRow> = db
        .query_or_note(catalog.indexes, &[], "indexes", &mut reasons)
        .into_iter()
        .map(|row| {
            let truth = |key: &str| match row.get(key) {
                Some(Value::Bool(value)) => *value,
                other => cell_number(other).is_some_and(|value| value != 0),
            };
            IndexRow {
                table: cell_text(row.get("table_name")),
                name: cell_text(row.get("index_name")),
                columns: cell_text(row.get("column_names"))
                    .split(',')
                    .filter(|column| !column.is_empty())
                    .map(str::to_string)
                    .collect(),
                unique: truth("is_unique"),
                primary: truth("is_primary"),
                valid: truth("is_valid"),
            }
        })
        .collect();
    let invalid: Vec<Value> = indexes
        .iter()
        .filter(|index| !index.valid)
        .map(|index| {
            let mut row = IndexMap::new();
            row.insert("table_name".to_string(), Value::string(&index.table));
            row.insert("index_name".to_string(), Value::string(&index.name));
            Value::Map(row)
        })
        .collect();
    out.insert("invalidIndexes".to_string(), Value::List(invalid));
    out.insert("duplicateIndexes".to_string(), Value::List(duplicates(&indexes)));
    let mut unindexed = Vec::new();
    if db.is_postgres() {
        for row in db.query_or_note(catalog.foreign_keys, &[], "foreignKeys", &mut reasons) {
            let table = cell_text(row.get("table_name"));
            let columns: Vec<String> = cell_text(row.get("column_names"))
                .split(',')
                .filter(|column| !column.is_empty())
                .map(str::to_string)
                .collect();
            let wanted: BTreeSet<&String> = columns.iter().collect();
            let indexed = indexes.iter().any(|index| {
                index.table == table
                    && index.columns.len() >= columns.len()
                    && index.columns[..columns.len()].iter().collect::<BTreeSet<_>>() == wanted
            });
            if !indexed {
                let mut out_row = IndexMap::new();
                out_row.insert("table_name".to_string(), Value::string(&table));
                out_row.insert(
                    "constraint_name".to_string(),
                    Value::string(cell_text(row.get("constraint_name"))),
                );
                out_row.insert(
                    "referenced_table".to_string(),
                    Value::string(cell_text(row.get("referenced_table"))),
                );
                out_row.insert("columns".to_string(), Value::string(columns.join(", ")));
                unindexed.push(Value::Map(out_row));
            }
        }
        let unused = db.query_or_note(POSTGRES_UNUSED_INDEXES, &[], "unusedIndexes", &mut reasons);
        out.insert("unusedIndexes".to_string(), rows_value(unused));
        let since = db.query_or_note(POSTGRES_STATS_SINCE, &[], "statsSince", &mut reasons);
        out.insert(
            "statsSince".to_string(),
            since.into_iter().next().map_or(Value::Null, Value::Map),
        );
        let databases = db.query_or_note(POSTGRES_DATABASE_AGE, &[], "databaseAge", &mut reasons);
        out.insert("databaseAge".to_string(), rows_value(databases));
        let tables = db.query_or_note(POSTGRES_TABLE_AGE, &[], "tableAge", &mut reasons);
        out.insert("tableAge".to_string(), rows_value(tables));
    } else {
        let unused = db.query_or_note(MYSQL_UNUSED_INDEXES, &[], "unusedIndexes", &mut reasons);
        out.insert("unusedIndexes".to_string(), rows_value(unused));
        out.insert("databaseAge".to_string(), Value::List(Vec::new()));
        out.insert("tableAge".to_string(), Value::List(Vec::new()));
    }
    out.insert("unindexedForeignKeys".to_string(), Value::List(unindexed));
    out.insert("reasons".to_string(), Value::Map(reasons));
    Ok(out)
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, get_database_activity)?;
    rust_service!(map, cancel_database_session)?;
    rust_service!(map, terminate_database_session)?;
    rust_service!(map, get_index_health)?;
    Ok(())
}

#[cfg(test)]
#[path = "activity_tests.rs"]
mod tests;
