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

//! Run one statement. The span records SQL text and `duration_ms`.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::cache;
use super::facade::EntityFacade;
use super::load_feed;
use crate::driver::{DbConnection, DbRow};
use crate::transaction::TransactionManager;

const SLOW_MS: u64 = 200;

static STATEMENT_COUNT: AtomicU64 = AtomicU64::new(0);
static STATEMENT_MILLIS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// [`PipelineWrites`] scopes open on this thread.
    static PIPELINE: Cell<u32> = const { Cell::new(0) };
}

/// While one is held, a create or update in a transaction is sent without waiting for its result
/// ([`DbConnection::execute_deferred`]), so a data file's writes share their round trips. A failure is reported by the
/// next statement of the transaction that waits, or by [`TransactionManager::finish_deferred`], which the data loader
/// calls before the file's transaction ends, so it fails the file as it would have failed it.
pub(crate) struct PipelineWrites(());

impl PipelineWrites {
    pub(crate) fn enter() -> Self {
        PIPELINE.with(|depth| depth.set(depth.get() + 1));
        Self(())
    }
}

impl Drop for PipelineWrites {
    fn drop(&mut self) {
        PIPELINE.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Whether a write in `tx` is sent without waiting: in a [`PipelineWrites`] scope and a transaction.
pub(crate) fn pipelining(tx: &TransactionManager) -> bool {
    PIPELINE.with(Cell::get) > 0 && tx.in_transaction()
}

/// Statements run through this module, and the sum of their durations in milliseconds.
pub fn statement_stats() -> (u64, u64) {
    (
        STATEMENT_COUNT.load(Ordering::Relaxed),
        STATEMENT_MILLIS.load(Ordering::Relaxed),
    )
}

pub(crate) fn query_tx(tx: &TransactionManager, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
    tx.with_connection(|conn| query_conn(conn, sql, params))
}

pub(crate) fn execute_tx(tx: &TransactionManager, sql: &str, params: &[Value]) -> Result<u64, Error> {
    tx.with_connection(|conn| execute_conn(conn, sql, params))
}

/// See [`DbConnection::execute_deferred`].
pub(crate) fn execute_deferred_tx(
    tx: &TransactionManager,
    sql: &str,
    params: Vec<Value>,
    zero_rows: Option<String>,
) -> Result<(), Error> {
    tx.with_connection(|conn| trace(sql, || conn.execute_deferred(sql, params, zero_rows)))
}

/// See [`DbConnection::execute_deferred_row`].
pub(crate) fn execute_deferred_row_tx(
    tx: &TransactionManager,
    sql: &str,
    params: Vec<Value>,
    key: u64,
) -> Result<(), Error> {
    tx.with_connection(|conn| trace(sql, || conn.execute_deferred_row(sql, params, key)))
}

pub(crate) fn query_conn(conn: &mut dyn DbConnection, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
    trace(sql, || conn.query(sql, params))
}

pub(crate) fn execute_conn(conn: &mut dyn DbConnection, sql: &str, params: &[Value]) -> Result<u64, Error> {
    trace(sql, || conn.execute(sql, params))
}

pub(crate) fn execute_script(conn: &mut dyn DbConnection, sql: &str) -> Result<(), Error> {
    for statement in split_sql(sql) {
        execute_conn(conn, statement, &[])?;
    }
    Ok(())
}

/// Begin and commit on a reserve connection. An error rolls that connection back.
pub(crate) fn with_reserve_tx<T, F>(tx: &TransactionManager, body: F) -> Result<T, Error>
where
    F: FnOnce(&mut dyn DbConnection) -> Result<T, Error>,
{
    tx.with_reserve(|conn| {
        conn.begin()?;
        match body(conn) {
            Ok(value) => {
                if let Err(err) = conn.commit() {
                    let _ = conn.rollback();
                    return Err(err);
                }
                Ok(value)
            }
            Err(err) => {
                let _ = conn.rollback();
                Err(err)
            }
        }
    })
}

/// Register the cache and feed hooks once for the open frame.
pub(crate) fn after_statement(facade: &EntityFacade, tx: &TransactionManager, feed: bool) {
    if !tx.in_transaction() {
        return;
    }
    if !tx.has_cache_flush() && cache::register_cache_flush(facade, tx).is_err() {
        return;
    }
    // One registration per frame: the pre-commit insert, the after-commit receive, and the
    // rollback cleanup are added together, whatever other commit hooks the frame has.
    if feed && !tx.has_feed_insert() {
        let _ = load_feed::register_feed_insert(facade, tx);
    }
}

pub(crate) fn already_exists(err: &Error) -> bool {
    let mut current = Some(err as &dyn std::error::Error);
    while let Some(item) = current {
        if existence_text(&item.to_string()) || existence_text(&format!("{item:?}")) {
            return true;
        }
        current = item.source();
    }
    false
}

/// The object a DDL statement creates is there already: Postgres and SQLite say "already exists",
/// MySQL says "Duplicate key name" (1061), "Duplicate column name" (1060) or "Duplicate foreign key
/// constraint name" (1826). Duplicate data ("duplicate key value", "Duplicate entry") is a real failure.
fn existence_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("already exists")
        || text.contains("duplicate key name")
        || text.contains("duplicate column name")
        || text.contains("duplicate foreign key constraint name")
}

pub(crate) fn split_sql(sql: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quote = false;
    let mut chars = sql.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if in_quote {
            if ch == '\'' {
                if chars.peek().is_some_and(|(_, next)| *next == '\'') {
                    chars.next();
                } else {
                    in_quote = false;
                }
            }
        } else if ch == '\'' {
            in_quote = true;
        } else if ch == ';' {
            let piece = sql[start..index].trim();
            if !piece.is_empty() {
                out.push(piece);
            }
            start = index + ch.len_utf8();
        }
    }
    let tail = sql[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn trace<T>(sql: &str, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let started = Instant::now();
    let span = tracing::info_span!("sql", sql = %sql, duration_ms = tracing::field::Empty).entered();
    let result = body();
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    STATEMENT_COUNT.fetch_add(1, Ordering::Relaxed);
    STATEMENT_MILLIS.fetch_add(duration_ms, Ordering::Relaxed);
    span.record("duration_ms", duration_ms);
    if duration_ms >= SLOW_MS {
        tracing::warn!(sql = %sql, duration_ms, "slow statement");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::already_exists;
    use moqui_lib::error::Error;

    #[test]
    fn an_existing_object_is_not_duplicate_data() {
        for text in [
            "postgres execute: relation \"user_account\" already exists",
            "ERROR 1061 (42000): Duplicate key name 'USERACCT_USERNAME'",
            "ERROR 1060 (42S21): Duplicate column name 'user_id'",
            "ERROR 1826 (HY000): Duplicate foreign key constraint name 'FK1'",
        ] {
            assert!(already_exists(&Error::new(text)), "{text}");
        }
        for text in [
            "postgres execute: could not create unique index \"useracct_username\": Key (username)=(a) is duplicated.",
            "postgres execute: duplicate key value violates unique constraint \"user_account_pk\"",
            "ERROR 1062 (23000): Duplicate entry 'a' for key 'USERACCT_USERNAME'",
        ] {
            assert!(!already_exists(&Error::new(text)), "{text}");
        }
    }
}
