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

//! Run one statement: a SQL client span inside a trace, the `db.client.operation.duration` metric, the statement
//! counters, and the slow-statement warning.

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
pub struct PipelineWrites(());

impl PipelineWrites {
    pub fn enter() -> Self {
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

pub fn execute_tx(tx: &TransactionManager, sql: &str, params: &[Value]) -> Result<u64, Error> {
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
pub fn with_reserve_tx<T, F>(tx: &TransactionManager, body: F) -> Result<T, Error>
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

/// How a database quotes and comments SQL text, for [`split_sql_as`] and [`blank_sql`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SqlSyntax {
    /// Postgres and standard SQL: `'…'` with doubled quotes, `E'…'` with backslash escapes, `"…"` names, dollar-quoted
    /// bodies (`$$…$$`, `$tag$…$tag$`), `--` comments and nested `/* … */` comments.
    #[default]
    Postgres,
    /// MySQL and MariaDB: `'…'` and `"…"` with backslash escapes and doubled quotes, `` `…` `` names, `#` and `-- `
    /// (with a space after) comments, and `/* … */` comments that do not nest.
    Mysql,
}

impl SqlSyntax {
    /// The syntax of a dialect's database.
    #[must_use]
    pub fn of(dialect: &moqui_db::dialect::Dialect) -> Self {
        if moqui_db::sql::is_mysql(dialect) {
            Self::Mysql
        } else {
            Self::Postgres
        }
    }
}

/// The statements of a Postgres script: [`split_sql_as`] with [`SqlSyntax::Postgres`].
pub fn split_sql(sql: &str) -> Vec<&str> {
    split_sql_as(sql, SqlSyntax::Postgres)
}

/// The statements of a script, split at each `;` outside quoted text, quoted names, dollar-quoted bodies and comments
/// as `syntax` writes them (plans/archive/Tools-Database.md 8.1). Each statement is trimmed and keeps its comments; a
/// piece that is only comments and space is left out.
pub fn split_sql_as(sql: &str, syntax: SqlSyntax) -> Vec<&str> {
    let blank = blank_sql(sql, syntax);
    let mut out = Vec::new();
    let mut start = 0;
    let mut push = |start: usize, end: usize| {
        if !blank[start..end].trim().is_empty() {
            out.push(sql[start..end].trim());
        }
    };
    for (at, byte) in blank.bytes().enumerate() {
        if byte == b';' {
            push(start, at);
            start = at + 1;
        }
    }
    push(start, sql.len());
    out
}

/// `sql` with each comment made spaces and the inside of each quoted text, quoted name and dollar-quoted body made
/// `_` (their quotes kept), byte for byte, so what is left is the statement's own words at the same positions: what
/// [`split_sql_as`] splits, and what a check of the statement's keywords reads.
///
/// A scanner over byte positions: a region begins and ends at an ASCII byte, and every byte in it is replaced, so the
/// text stays UTF-8.
#[must_use]
pub fn blank_sql(sql: &str, syntax: SqlSyntax) -> String {
    let bytes = sql.as_bytes();
    let mut out = bytes.to_vec();
    let mysql = syntax == SqlSyntax::Mysql;
    let fill = |out: &mut Vec<u8>, from: usize, to: usize, with: u8| {
        for byte in &mut out[from..to.min(bytes.len())] {
            *byte = with;
        }
    };
    let mut at = 0;
    while at < bytes.len() {
        let next = bytes.get(at + 1).copied();
        match bytes[at] {
            b'-' if next == Some(b'-')
                && (!mysql || bytes.get(at + 2).is_none_or(|byte| byte.is_ascii_whitespace())) =>
            {
                let end = sql[at..].find('\n').map_or(bytes.len(), |end| at + end);
                fill(&mut out, at, end, b' ');
                at = end;
            }
            b'#' if mysql => {
                let end = sql[at..].find('\n').map_or(bytes.len(), |end| at + end);
                fill(&mut out, at, end, b' ');
                at = end;
            }
            b'/' if next == Some(b'*') => {
                let end = block_comment_end(bytes, at + 2, !mysql);
                fill(&mut out, at, end, b' ');
                at = end;
            }
            quote @ (b'\'' | b'"' | b'`') => {
                let escapes = if mysql {
                    quote != b'`'
                } else {
                    // `E'…'` takes backslash escapes; an `E` that ends a longer word is not that prefix.
                    quote == b'\''
                        && at > 0
                        && matches!(bytes[at - 1], b'E' | b'e')
                        && !(at > 1 && is_word_byte(bytes[at - 2]))
                };
                let end = quoted_end(bytes, at + 1, quote, escapes);
                // The closing quote stays, unless the text runs to the end unclosed.
                let inner_end = if end <= bytes.len() && bytes.get(end - 1) == Some(&quote) && end > at + 1 {
                    end - 1
                } else {
                    end
                };
                fill(&mut out, at + 1, inner_end, b'_');
                at = end;
            }
            b'$' if !mysql && !(at > 0 && is_word_byte(bytes[at - 1])) => match dollar_tag(&sql[at..]) {
                Some(tag) => {
                    let body = at + tag.len();
                    let close = sql[body..].find(tag).map_or(bytes.len(), |end| body + end);
                    fill(&mut out, body, close, b'_');
                    at = (close + tag.len()).min(bytes.len());
                }
                None => at += 1,
            },
            _ => at += 1,
        }
    }
    // Every replaced region starts and ends at an ASCII byte and is replaced whole, so this cannot fail; the original
    // is the fallback all the same.
    String::from_utf8(out).unwrap_or_else(|_| sql.to_string())
}

/// Past the `*/` that closes the block comment whose body starts at `at`, counting nested `/*` when `nests`.
fn block_comment_end(bytes: &[u8], mut at: usize, nests: bool) -> usize {
    let mut depth = 1;
    while at < bytes.len() && depth > 0 {
        match (bytes[at], bytes.get(at + 1)) {
            (b'/', Some(b'*')) if nests => {
                depth += 1;
                at += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                at += 2;
            }
            _ => at += 1,
        }
    }
    at.min(bytes.len())
}

/// Past the `quote` that closes the quoted text whose body starts at `at`; a doubled quote is one quote in the text.
fn quoted_end(bytes: &[u8], mut at: usize, quote: u8, escapes: bool) -> usize {
    while at < bytes.len() {
        if escapes && bytes[at] == b'\\' {
            at += 2;
        } else if bytes[at] == quote {
            if bytes.get(at + 1) != Some(&quote) {
                return at + 1;
            }
            at += 2;
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// A letter, digit, `_` or `$`, which continue an identifier, or a byte of a multi-byte character.
fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

/// The dollar-quote tag `text` starts with: `$$` or `$name$`, a name not starting with a digit (`$1` is a parameter).
fn dollar_tag(text: &str) -> Option<&str> {
    let (name, _) = text.strip_prefix('$')?.split_once('$')?;
    let valid = name
        .chars()
        .enumerate()
        .all(|(index, ch)| ch == '_' || ch.is_alphabetic() || (index > 0 && ch.is_ascii_digit()));
    text.get(..name.len() + 2).filter(|_| valid)
}

#[cfg(test)]
#[path = "split_sql_tests.rs"]
mod split_sql_tests;

fn trace<T>(sql: &str, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let started = Instant::now();
    let trace = crate::hooks::trace_statement(sql);
    let result = body();
    let elapsed = started.elapsed();
    let duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    if super::query_stats::enabled() {
        super::query_stats::record(sql, elapsed, result.is_err());
    }
    STATEMENT_COUNT.fetch_add(1, Ordering::Relaxed);
    STATEMENT_MILLIS.fetch_add(duration_ms, Ordering::Relaxed);
    if duration_ms >= SLOW_MS {
        // The warning reaches the telemetry logs, so the statement's literals are left out as on its span.
        tracing::warn!(sql = %redact_sql(sql), duration_ms, "slow statement");
    }
    if let Some(trace) = trace {
        trace.finish(result.as_ref().err().map(ToString::to_string).as_deref());
    }
    result
}

/// Literal values are never exported, including SQL outside the parameterized entity paths. At most about 4 KiB.
pub fn redact_sql(sql: &str) -> String {
    redact_sql_to(sql, 4096)
}

/// [`redact_sql`] of the whole text: the key of the query stats (`query_stats::statement_key`).
pub(crate) fn redact_sql_uncapped(sql: &str) -> String {
    redact_sql_to(sql, usize::MAX)
}

fn redact_sql_to(sql: &str, cap: usize) -> String {
    let mut output = String::with_capacity(sql.len().min(cap));
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if output.len() >= cap {
            break;
        }
        if ch == '\'' {
            output.push('?');
            while let Some(next) = chars.next() {
                if next == '\\' {
                    chars.next();
                    continue;
                }
                if next == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        } else if ch == '-' && chars.peek() == Some(&'-') {
            for next in chars.by_ref() {
                if next == '\n' {
                    break;
                }
            }
            output.push(' ');
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = ' ';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            output.push(' ');
        } else if ch == '$' {
            let mut ahead = chars.clone();
            let mut delimiter = String::from("$");
            let mut found = false;
            for next in ahead.by_ref().take(64) {
                if next == '$' {
                    delimiter.push('$');
                    found = true;
                    break;
                }
                if !next.is_ascii_alphanumeric() && next != '_' {
                    break;
                }
                delimiter.push(next);
            }
            if found {
                for _ in 1..delimiter.len() {
                    chars.next();
                }
                let mut suffix = String::new();
                for next in chars.by_ref() {
                    suffix.push(next);
                    if suffix.ends_with(&delimiter) {
                        break;
                    }
                    if suffix.len() > delimiter.len() * 2 {
                        let keep = suffix
                            .char_indices()
                            .rev()
                            .nth(delimiter.len())
                            .map_or(0, |(index, _)| index);
                        suffix.drain(..keep);
                    }
                }
                output.push('?');
            } else {
                output.push('$');
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    if let Some(next) = chars.next() {
                        output.push(next);
                    }
                }
            }
        } else if ch.is_ascii_digit()
            && !output
                .chars()
                .last()
                .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_')
        {
            output.push('?');
            while chars.peek().is_some_and(|next| next.is_ascii_digit() || *next == '.') {
                chars.next();
            }
        } else {
            output.push(ch);
        }
    }
    output
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
