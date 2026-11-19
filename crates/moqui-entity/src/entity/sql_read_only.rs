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

//! What a read-only run of SQL someone wrote may do (`info::run_sql_with` with `read_only`: Tools › SQL, a grid SQL
//! source, an ETL SQL source; docs/tools/Database.md "SQL").
//!
//! A read-only transaction stops table writes, not what a privileged role reaches past the tables: on Postgres a
//! superuser's `COPY … TO PROGRAM`, `pg_read_file`, `pg_terminate_backend` or `pg_notify` run in one, and on MySQL
//! `SELECT … INTO OUTFILE` and `LOAD_FILE` (SEC-TLS-86). So a read-only run takes only a query ([`refusal`]), refuses
//! the calls that reach the server's files and programs, other sessions, the cluster's notify channel, the session's
//! settings and role, or run SQL text of their own, and on Postgres a superuser's run becomes [`READER_ROLE`] for its
//! transaction ([`reduce`]): the embedded cluster makes it at its start ([`READER_ROLE_SQL`]), and an external
//! server's administrator makes it the same way, or a superuser's read-only run is refused.
//!
//! The refused calls are load-bearing with the role too: the session's authenticated user is still the superuser, so
//! `set_config('session_authorization', …)` (or `role`), alone or inside SQL text a function runs, switches back to it
//! within the statement (Postgres 18). And the role reads more than the application's tables: `pg_read_all_data` reads
//! every table, `pg_authid`'s password verifiers among them, and `pg_monitor` every setting (`pg_read_all_settings`)
//! and every session's statement. A holder of `SQL_RUNNER_WEB` reads all of that; what the role takes away is the
//! server's files, programs and signals.

use moqui_lib::error::Error;

use super::exec::{blank_sql, SqlSyntax};
use crate::driver::DbConnection;

/// The role a superuser's read-only run takes for its transaction: no login and no superuser, reading every table
/// (`pg_read_all_data`, the catalogs' `pg_authid` among them) and the settings and statistics views (`pg_monitor`),
/// and none of the server's files or programs.
pub const READER_ROLE: &str = "moqui_sql_reader";

/// The statements that make [`READER_ROLE`], run again at each start of the embedded cluster.
pub const READER_ROLE_SQL: &[&str] = &[
    "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'moqui_sql_reader') THEN \
     CREATE ROLE moqui_sql_reader NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS; \
     END IF; END $$",
    "GRANT pg_read_all_data, pg_monitor TO moqui_sql_reader",
];

/// Postgres functions a read-only run does not call: the server's files, signals to other sessions and the server,
/// the notify channel, the session's settings (`set_config` sets `role` and `session_authorization` too), and
/// functions that run SQL text given to them.
const POSTGRES_CALLS: &[&str] = &[
    "pg_read_file",
    "pg_read_binary_file",
    "pg_stat_file",
    "pg_current_logfile",
    "lo_import",
    "lo_export",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_signal_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_log_backend_memory_contexts",
    "pg_promote",
    "pg_switch_wal",
    "pg_create_restore_point",
    "pg_backup_start",
    "pg_backup_stop",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    "pg_replication_slot_advance",
    "pg_drop_replication_slot",
    "pg_notify",
    "set_config",
    "query_to_xml",
    "query_to_xmlschema",
    "query_to_xml_and_xmlschema",
    "cursor_to_xml",
    "cursor_to_xmlschema",
    "ts_stat",
    "ts_rewrite",
];

/// The families of [`POSTGRES_CALLS`] by the start of their names.
const POSTGRES_CALL_PREFIXES: &[&str] = &[
    "pg_ls_",
    "pg_file_",
    "pg_advisory_",
    "pg_try_advisory_",
    "pg_logical_",
    "pg_replication_origin_",
    "pg_create_physical_",
    "pg_create_logical_",
    "pg_copy_physical_",
    "pg_copy_logical_",
    "pg_stat_reset",
    "dblink",
];

/// MySQL and MariaDB words a read-only run does not take: a file read or written by the server, a named lock.
const MYSQL_WORDS: &[&str] = &["load_file", "outfile", "dumpfile", "get_lock"];

/// Why `sql` does not run read only, or `None` when it does: a statement that is not a query (`COPY`, `DO`, `CALL`,
/// `SET`, DDL), a refused call ([`POSTGRES_CALLS`], [`MYSQL_WORDS`]) by its name, quoted or not, and on Postgres a
/// `U&"…"` name, whose escapes would hide one.
#[must_use]
pub fn refusal(sql: &str, syntax: SqlSyntax) -> Option<String> {
    let blanked = blank_sql(sql, syntax);
    if syntax == SqlSyntax::Mysql {
        if let Some(refusal) = mysql_raw_refusal(sql) {
            return Some(refusal);
        }
    }
    if !is_query(&blanked) {
        return Some(match syntax {
            SqlSyntax::Mysql => {
                "only a query runs read only on MySQL and MariaDB, where other statements can commit the transaction"
                    .to_string()
            }
            SqlSyntax::Postgres => {
                "only a query runs read only: other statements (COPY, DO, CALL, SET) reach past the read-only \
                 transaction"
                    .to_string()
            }
        });
    }
    if syntax == SqlSyntax::Postgres && blanked.to_ascii_lowercase().contains("u&\"") {
        return Some("a U&\"…\" name does not run read only".to_string());
    }
    let refused = |word: &str| match syntax {
        SqlSyntax::Postgres => {
            POSTGRES_CALLS.contains(&word) || POSTGRES_CALL_PREFIXES.iter().any(|prefix| word.starts_with(prefix))
        }
        SqlSyntax::Mysql => MYSQL_WORDS.contains(&word),
    };
    names(sql, &blanked, syntax)
        .into_iter()
        .find(|word| refused(word))
        .map(|word| format!("{word} does not run read only: it reaches past the database's tables"))
}

/// Whether the blanked text `blanked` reads rows by its first word, past comments and opening parentheses: `SELECT`,
/// `WITH`, `VALUES`, `TABLE`, `SHOW`, `EXPLAIN`, `DESCRIBE` ([`super::info::is_query`] on the statement's own words).
fn is_query(blanked: &str) -> bool {
    super::info::is_query(blanked.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '('))
}

/// On MySQL and MariaDB, what the raw text refuses before it is blanked: an executable comment (`/*!`, `/*M!`, whose
/// inside the server runs) or a [`MYSQL_WORDS`] word anywhere, in a string too. Where a string ends depends on the
/// server's `sql_mode` (`NO_BACKSLASH_ESCAPES`, `ANSI_QUOTES`), which [`blank_sql`] does not know, so
/// `SELECT '\' /*!INTO OUTFILE … */ -- '` would hide one in what it takes as a string; a string that holds one is
/// refused too.
fn mysql_raw_refusal(sql: &str) -> Option<String> {
    let lower = sql.to_ascii_lowercase();
    if lower.contains("/*!") || lower.contains("/*m!") {
        return Some("a /*! … */ comment runs on MySQL and MariaDB, and does not run read only".to_string());
    }
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .find(|word| MYSQL_WORDS.contains(word))
        .map(|word| format!("{word} does not run read only on MySQL and MariaDB, in a string or not"))
}

/// The words of `sql` outside its comments and text, with the inside of each quoted name, all lowercase.
fn names(sql: &str, blanked: &str, syntax: SqlSyntax) -> Vec<String> {
    let lower = blanked.to_ascii_lowercase();
    let mut words: Vec<String> = lower
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    // `blank_sql` keeps the quotes of a quoted name at their places and fills its inside, so each pair of quotes left
    // bounds one name of the original text.
    let quotes: &[u8] = match syntax {
        SqlSyntax::Postgres => b"\"",
        SqlSyntax::Mysql => b"\"`",
    };
    let bytes = blanked.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if quotes.contains(&bytes[at]) {
            let quote = bytes[at];
            let end = bytes[at + 1..]
                .iter()
                .position(|byte| *byte == quote)
                .map_or(bytes.len(), |end| at + 1 + end);
            if let Some(inner) = sql.get(at + 1..end) {
                words.push(inner.to_ascii_lowercase());
            }
            at = end + 1;
        } else {
            at += 1;
        }
    }
    words
}

/// On Postgres, make the read-only transaction just begun on `conn` [`READER_ROLE`]'s when the connection is a
/// superuser's (`SET LOCAL SESSION AUTHORIZATION`, which ends with the transaction; the connection is closed after it
/// anyway). Nothing for a connection of any other role.
///
/// # Errors
///
/// Returns the database's error, and a refusal for a superuser's connection to a server without the role: the run
/// does not go on as the superuser.
pub(crate) fn reduce(conn: &mut dyn DbConnection) -> Result<(), Error> {
    let rows = conn.query(
        &format!(
            "SELECT current_setting('is_superuser') AS superuser, \
             (SELECT count(*) FROM pg_roles WHERE rolname = '{READER_ROLE}')::text AS reader"
        ),
        &[],
    )?;
    let column = |name: &str| {
        rows.first()
            .and_then(|row| row.get(name))
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    if column("superuser") != "on" {
        return Ok(());
    }
    if column("reader") == "0" {
        tracing::error!(
            "A read-only SQL run on a superuser's connection was refused: the server has no role {READER_ROLE}"
        );
        return Err(Error::new(format!(
            "a read-only run on this connection needs the role {READER_ROLE}, which the server does not have: its \
             administrator makes it (NOLOGIN, granted pg_read_all_data and pg_monitor)"
        )));
    }
    conn.execute(&format!("SET LOCAL SESSION AUTHORIZATION {READER_ROLE}"), &[])
        .map(|_| ())
}

#[cfg(test)]
#[path = "sql_read_only_tests.rs"]
mod tests;
