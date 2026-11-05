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
}

/// Run one catalog query on `group`'s connection, outside any transaction of the caller (suspended while it runs), so a
/// failed probe cannot abort the caller's transaction. Rows are maps of the column names as the server gives them.
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
    let rows = tx.run(TransactionAttribute::IgnoreSuspend, None, || {
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

/// The dialect (`database-list` name) of `group`: `postgres`, `mysql` or `mysql8`.
#[must_use]
pub fn group_dialect(facade: &EntityFacade, group: &str) -> Option<String> {
    facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, dialect, _)| dialect.name.clone())
}
