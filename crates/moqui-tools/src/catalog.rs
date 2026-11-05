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

//! The probe and catalog SQL of the Tools database screens, a constant per dialect (plans/Tools-Database.md §4.2), and
//! [`query`], which runs one on an entity group's connection. The schema comparison's queries are in
//! `crates/moqui-db/src/sql/` beside the DDL builders, since `moqui-entity` cannot depend on this crate. Each query's
//! columns are aliased in lower case, so a row reads the same on every server.

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use moqui_core::entity::EntityFacade;
use moqui_core::transaction::TransactionAttribute;

/// Postgres, embedded or external.
pub mod postgres {
    /// The server's version, start, uptime and database size, and who the connection is.
    pub const SERVER: &str = "SELECT version() AS version, pg_postmaster_start_time() AS start_time, \
         CAST(EXTRACT(EPOCH FROM now() - pg_postmaster_start_time()) AS BIGINT) AS uptime_seconds, \
         pg_database_size(current_database()) AS database_size, current_database() AS database_name, \
         current_user AS user_name, (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) AS is_superuser";
    /// The monitoring roles the connection's role is in (a superuser is in every one).
    pub const ROLES: &str = "SELECT pg_has_role(current_user, 'pg_monitor', 'MEMBER') AS pg_monitor, \
         pg_has_role(current_user, 'pg_read_all_stats', 'MEMBER') AS pg_read_all_stats, \
         pg_has_role(current_user, 'pg_signal_backend', 'MEMBER') AS pg_signal_backend";
    /// The extensions the screens use, whether the server has them and whether this database has them installed.
    pub const EXTENSIONS: &str = "SELECT name AS name, installed_version AS installed_version \
         FROM pg_available_extensions \
         WHERE name IN ('pg_stat_statements', 'pgstattuple', 'amcheck', 'pg_buffercache', 'pg_visibility')";
    /// The libraries the postmaster loaded at start.
    pub const PRELOAD: &str = "SELECT current_setting('shared_preload_libraries') AS libraries";
    /// Reads only when the library is loaded and the extension installed.
    pub const STATEMENTS_READABLE: &str = "SELECT count(*) AS statements FROM pg_stat_statements";

    // ---- SQL (plans/Tools-Database.md §8) ----
    /// The database the connection is on: what a production confirmation asks to be typed.
    pub const CURRENT_DATABASE: &str = "SELECT current_database() AS database_name";
    /// Every column of the tables and views outside the system schemas, for the editor's completion.
    pub const COLUMNS: &str = "SELECT table_schema AS table_schema, table_name AS table_name, \
         column_name AS column_name, current_schema() AS current_schema FROM information_schema.columns \
         WHERE table_schema NOT IN ('pg_catalog', 'information_schema') AND table_schema NOT LIKE 'pg\\_%' \
         ORDER BY table_schema, table_name, ordinal_position";
    /// The database's own statement stats (§7.2), the most costly first; `?` is the row limit.
    pub const STATEMENT_STATS: &str = "SELECT CAST(queryid AS TEXT) AS query_id, query AS query, calls AS calls, \
         total_exec_time AS total_exec_time, mean_exec_time AS mean_exec_time, rows AS rows, \
         shared_blks_hit AS shared_blks_hit, shared_blks_read AS shared_blks_read, \
         temp_blks_written AS temp_blks_written FROM pg_stat_statements \
         WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) \
         ORDER BY total_exec_time DESC LIMIT ?";
    pub const STATEMENT_STATS_RESET: &str = "SELECT pg_stat_statements_reset() AS reset";
    /// The planner's estimate of a table's rows (`?` is the table name); -1 when it was never analyzed.
    pub const TABLE_ESTIMATE: &str =
        "SELECT CAST(reltuples AS BIGINT) AS estimate FROM pg_class WHERE oid = to_regclass(?)";
    /// Cancel the statement of a session: the caller appends its backend pid, a number, and `)`.
    pub const CANCEL_BACKEND: &str = "SELECT pg_cancel_backend(";
    /// When a session started: the caller appends its backend pid, a number. With the pid it names one session, so a
    /// cancel does not reach a later session that has the same pid.
    pub const BACKEND_START: &str =
        "SELECT CAST(backend_start AS TEXT) AS backend_start FROM pg_stat_activity WHERE pid = ";
}

/// MySQL 8 and MariaDB (the `mysql` and `mysql8` database confs).
pub mod mysql {
    /// The server's version (MariaDB names itself in it), and who the connection is.
    pub const SERVER: &str = "SELECT @@version AS version, @@version_comment AS version_comment, \
         DATABASE() AS database_name, CURRENT_USER() AS user_name";
    pub const UPTIME: &str = "SHOW GLOBAL STATUS LIKE 'Uptime'";
    pub const DATABASE_SIZE: &str = "SELECT SUM(DATA_LENGTH + INDEX_LENGTH) AS database_size \
         FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE()";
    /// 1 when `performance_schema` is on: MySQL has it on by default, MariaDB ships it off.
    pub const PERFORMANCE_SCHEMA: &str = "SELECT @@performance_schema AS performance_schema";
    /// Reads when `performance_schema` is on and the user may read it.
    pub const DIGESTS_READABLE: &str = "SELECT COUNT(*) AS statements \
         FROM performance_schema.events_statements_summary_by_digest";
    /// The `sys` views of index use (MySQL, MariaDB 10.6 and later).
    pub const SYS_INDEXES_READABLE: &str = "SELECT COUNT(*) AS indexes FROM sys.schema_redundant_indexes";
    pub const GRANTS: &str = "SHOW GRANTS";

    // ---- SQL (plans/Tools-Database.md §8) ----
    pub const CURRENT_DATABASE: &str = "SELECT DATABASE() AS database_name";
    pub const COLUMNS: &str = "SELECT TABLE_SCHEMA AS table_schema, TABLE_NAME AS table_name, \
         COLUMN_NAME AS column_name, DATABASE() AS current_schema FROM information_schema.COLUMNS \
         WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, ORDINAL_POSITION";
    /// The statement digests of this database (§7.2), the most costly first; `?` is the row limit. Timer columns are
    /// picoseconds.
    pub const STATEMENT_STATS: &str = "SELECT DIGEST AS query_id, DIGEST_TEXT AS query, COUNT_STAR AS calls, \
         SUM_TIMER_WAIT AS sum_timer_wait, SUM_ROWS_EXAMINED AS sum_rows_examined, \
         SUM_NO_INDEX_USED AS sum_no_index_used FROM performance_schema.events_statements_summary_by_digest \
         WHERE SCHEMA_NAME = DATABASE() ORDER BY SUM_TIMER_WAIT DESC LIMIT ?";
    pub const STATEMENT_STATS_RESET: &str = "TRUNCATE TABLE performance_schema.events_statements_summary_by_digest";
    /// InnoDB's estimate of a table's rows (`?` is the table name).
    pub const TABLE_ESTIMATE: &str =
        "SELECT TABLE_ROWS AS estimate FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?";
    /// `KILL QUERY` takes no placeholder: the caller appends the connection id, a number.
    pub const KILL_QUERY: &str = "KILL QUERY ";
    /// When the server started, in epoch seconds: with it a connection id names one connection, since the ids start
    /// again only when the server does.
    pub const SERVER_START: &str =
        "SELECT CAST(UNIX_TIMESTAMP() - CAST(VARIABLE_VALUE AS SIGNED) AS SIGNED) AS server_start \
         FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Uptime'";
}

/// The most one catalog query may take: past it the transaction's deadline cancels the statement.
pub const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run one catalog query on `group`'s connection, in a transaction of its own (the caller's is suspended while it
/// runs), so a failed probe cannot abort the caller's transaction, with a deadline of [`QUERY_TIMEOUT`] that cancels a
/// hung statement. Rows are maps of the column names as the server gives them.
///
/// # Errors
///
/// Returns an error for an unknown group or when the statement fails.
pub fn query(facade: &EntityFacade, group: &str, sql: &str) -> Result<Vec<IndexMap<String, Value>>, Error> {
    let tx = facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, _, tx)| tx.clone())
        .ok_or_else(|| Error::new(format!("no entity group named {group}")))?;
    let rows = tx.run(TransactionAttribute::ForceNew, Some(QUERY_TIMEOUT), || {
        tx.with_connection(|conn| conn.query_once(sql, &[]))
    })?;
    Ok(rows
        .into_iter()
        .map(|row| {
            row.columns
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect()
        })
        .collect())
}

/// [`query`] with `params` bound to its `?` placeholders.
///
/// # Errors
///
/// As [`query`].
pub fn query_with(
    facade: &EntityFacade,
    group: &str,
    sql: &str,
    params: &[Value],
) -> Result<Vec<IndexMap<String, Value>>, Error> {
    let tx = group_tx(facade, group)?;
    let rows = tx.run(TransactionAttribute::ForceNew, Some(QUERY_TIMEOUT), || {
        tx.with_connection(|conn| conn.query_once(sql, params))
    })?;
    Ok(rows
        .into_iter()
        .map(|row| {
            row.columns
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect()
        })
        .collect())
}

/// Run one statement that gives no rows (a reset, `KILL QUERY`) as [`query`] runs a query; the rows it changed.
///
/// # Errors
///
/// As [`query`].
pub fn execute(facade: &EntityFacade, group: &str, sql: &str) -> Result<u64, Error> {
    let tx = group_tx(facade, group)?;
    tx.run(TransactionAttribute::ForceNew, Some(QUERY_TIMEOUT), || {
        tx.with_connection(|conn| conn.execute(sql, &[]))
    })
}

/// The transaction manager of `group`.
///
/// # Errors
///
/// Returns an error for an unknown group.
pub fn group_tx(facade: &EntityFacade, group: &str) -> Result<moqui_core::transaction::TransactionManager, Error> {
    facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, _, tx)| tx.clone())
        .ok_or_else(|| Error::new(format!("no entity group named {group}")))
}

/// Whether `group` is on MySQL or MariaDB.
#[must_use]
pub fn group_is_mysql(facade: &EntityFacade, group: &str) -> bool {
    group_dialect(facade, group).is_some_and(|dialect| dialect.starts_with("mysql"))
}

/// The dialect (`database-list` name) of `group`: `postgres`, `mysql` or `mysql8`.
#[must_use]
pub fn group_dialect(facade: &EntityFacade, group: &str) -> Option<String> {
    facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, dialect, _)| dialect.name.clone())
}
