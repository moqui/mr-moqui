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

//! SQLite [`DbDriver`]. One physical connection behind every logical connection.
//!
//! The pool hands out several logical connections: the enlisted one, and a reserve for sequence
//! refills, table checks, and other side-channel work. SQLite has one writer, and the browser has
//! one thread, so a second physical connection that writes while the first holds a write
//! transaction would wait forever. Instead every logical connection shares one handle. A `begin`
//! while another logical connection has the transaction open becomes a savepoint inside it, and its
//! commit is a release. A nested transaction is therefore undone when the outer one rolls back.
//! The sequence bank stays correct because a refill takes the in-memory bank position as its floor.
//!
//! Values are bound by storage class. Timestamps are UTC text `YYYY-MM-DD HH:MM:SS.ffffff`, dates
//! `YYYY-MM-DD`, times `HH:MM:SS.ffffff`, so text order is time order. Decimals are text, and a
//! `NUMERIC` column stores them as an integer or a real, so a decimal keeps its value to about 15
//! significant digits but not its scale (`12.3400` reads back as `12.34`). Rows come back as the
//! storage class; the entity layer converts to the field type.
//!
//! A stream reads its whole result when it opens. There is no statement cancel.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use moqui_core::driver::{DbCancel, DbConnection, DbDriver, DbRow};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use rusqlite::types::{Value as SqlValue, ValueRef};

/// Prepared statements kept per database, keyed by SQL text. The driver contract caps it at 100.
const STATEMENT_CACHE: usize = 100;

/// Opens logical connections onto one SQLite database.
pub struct SqliteDriver {
    shared: Arc<Mutex<Shared>>,
}

struct Shared {
    conn: rusqlite::Connection,
    next_id: u64,
}

impl SqliteDriver {
    /// Open `path`. `:memory:` is a private in-memory database.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be opened.
    pub fn open(path: &str) -> Result<Self, Error> {
        let conn = rusqlite::Connection::open(path).map_err(sqlite_err)?;
        conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        // LIKE is case-sensitive, as on Postgres. Entity conditions use UPPER() for ignore-case.
        // Foreign keys are not created on this database (see the sqlite profile), so none to enforce.
        conn.execute_batch("PRAGMA case_sensitive_like = ON;")
            .map_err(sqlite_err)?;
        // WasmFS fails the rollback journal's unlink at commit (SQLITE_IOERR_DELETE). PERSIST keeps
        // the journal file and zeroes its header, which also saves an OPFS file create and delete
        // per commit. Durability is unchanged.
        #[cfg(target_os = "emscripten")]
        conn.execute_batch("PRAGMA journal_mode = PERSIST;")
            .map_err(sqlite_err)?;
        Ok(Self {
            shared: Arc::new(Mutex::new(Shared { conn, next_id: 1 })),
        })
    }

    /// Run `body` in one SQLite transaction on the shared handle, so its autocommit statements on
    /// any logical connection (such as DDL on a reserve connection) commit once. A `begin` inside
    /// becomes a savepoint, as with any open transaction. Committed when `body` returns `Ok`, rolled
    /// back on `Err`. Inside an open transaction, `body` just runs.
    ///
    /// On OPFS every commit is a run of suspended file writes; creating every framework table
    /// one commit at a time took about 18 s, and one commit is far less.
    ///
    /// # Errors
    ///
    /// Returns `body`'s error, or an error when the transaction cannot begin or commit.
    pub fn bulk<T>(&self, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let began = {
            let shared = lock(&self.shared);
            if shared.conn.is_autocommit() {
                shared.conn.execute_batch("BEGIN").map_err(sqlite_err)?;
                true
            } else {
                false
            }
        };
        let result = body();
        if began {
            let shared = lock(&self.shared);
            if !shared.conn.is_autocommit() {
                let end = if result.is_ok() { "COMMIT" } else { "ROLLBACK" };
                shared.conn.execute_batch(end).map_err(sqlite_err)?;
            }
        }
        result
    }

    /// A private in-memory database.
    ///
    /// # Errors
    ///
    /// Returns an error when SQLite cannot allocate it.
    pub fn memory() -> Result<Self, Error> {
        Self::open(":memory:")
    }

    /// A logical connection with its concrete type, for tests.
    pub fn connect_sqlite(&self) -> SqliteConn {
        let id = {
            let mut shared = lock(&self.shared);
            let id = shared.next_id;
            shared.next_id += 1;
            id
        };
        SqliteConn {
            shared: Arc::clone(&self.shared),
            id,
            tx: None,
            stream: None,
        }
    }
}

impl DbDriver for SqliteDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(self.connect_sqlite()))
    }
}

/// How this logical connection holds a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tx {
    /// It issued the physical `BEGIN`.
    Root,
    /// Another logical connection had the transaction open; this is a savepoint inside it.
    Nested(String),
}

/// One logical connection.
pub struct SqliteConn {
    shared: Arc<Mutex<Shared>>,
    id: u64,
    tx: Option<Tx>,
    stream: Option<VecDeque<DbRow>>,
}

impl SqliteConn {
    fn savepoint_name(&self, name: &str) -> Result<String, Error> {
        if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(Error::new(format!("savepoint name {name:?} is not a plain identifier")));
        }
        // Logical connections share one handle, so their savepoint names must not collide.
        Ok(format!("c{}_{name}", self.id))
    }

    fn rows(&self, sql: &str, params: &[Value], cached: bool) -> Result<Vec<DbRow>, Error> {
        let shared = lock(&self.shared);
        run_query(&shared.conn, sql, params, cached).map_err(|err| statement_err(err, sql))
    }
}

impl Drop for SqliteConn {
    fn drop(&mut self) {
        if self.tx.is_some() {
            let _ = self.rollback();
        }
    }
}

impl DbConnection for SqliteConn {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
        let shared = lock(&self.shared);
        let bound = to_params(params)?;
        let mut statement = shared.conn.prepare_cached(sql).map_err(|err| statement_err(err, sql))?;
        if statement.column_count() > 0 {
            // RETURNING, or a query sent through execute: step it once and count the rows.
            return collect(&mut statement, &bound)
                .map(|rows| rows.len() as u64)
                .map_err(|err| statement_err(err, sql));
        }
        statement
            .execute(rusqlite::params_from_iter(bound.iter()))
            .map(|changed| changed as u64)
            .map_err(|err| statement_err(err, sql))
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.rows(sql, params, true)
    }

    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.rows(sql, params, false)
    }

    fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error> {
        if self.stream.is_some() {
            return Err(Error::new("a stream is already open on this connection"));
        }
        let rows = self.rows(sql, params, false)?;
        self.stream = Some(rows.into());
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(self.stream.as_mut().and_then(VecDeque::pop_front))
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        self.stream = None;
        Ok(())
    }

    fn stream_open(&self) -> bool {
        self.stream.is_some()
    }

    fn begin(&mut self) -> Result<(), Error> {
        if self.tx.is_some() {
            return Err(Error::new("a transaction is already open on this connection"));
        }
        let shared = lock(&self.shared);
        if shared.conn.is_autocommit() {
            shared.conn.execute_batch("BEGIN").map_err(sqlite_err)?;
            self.tx = Some(Tx::Root);
        } else {
            let name = format!("c{}_tx", self.id);
            shared
                .conn
                .execute_batch(&format!("SAVEPOINT {name}"))
                .map_err(sqlite_err)?;
            self.tx = Some(Tx::Nested(name));
        }
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        let Some(tx) = self.tx.take() else {
            return Err(Error::new("no transaction is open on this connection"));
        };
        let shared = lock(&self.shared);
        match tx {
            Tx::Root => {
                if shared.conn.is_autocommit() {
                    // SQLite already ended it (an error that rolls back the whole transaction).
                    return Ok(());
                }
                shared.conn.execute_batch("COMMIT").map_err(sqlite_err)
            }
            Tx::Nested(name) => release_if_open(&shared.conn, &name),
        }
    }

    fn rollback(&mut self) -> Result<(), Error> {
        let Some(tx) = self.tx.take() else {
            return Ok(());
        };
        let shared = lock(&self.shared);
        self.stream = None;
        match tx {
            Tx::Root => {
                if shared.conn.is_autocommit() {
                    return Ok(());
                }
                shared.conn.execute_batch("ROLLBACK").map_err(sqlite_err)
            }
            Tx::Nested(name) => {
                if shared.conn.is_autocommit() {
                    return Ok(());
                }
                shared
                    .conn
                    .execute_batch(&format!("ROLLBACK TO {name}"))
                    .map_err(sqlite_err)?;
                release_if_open(&shared.conn, &name)
            }
        }
    }

    fn savepoint(&mut self, name: &str) -> Result<(), Error> {
        let name = self.savepoint_name(name)?;
        let shared = lock(&self.shared);
        shared
            .conn
            .execute_batch(&format!("SAVEPOINT {name}"))
            .map_err(sqlite_err)
    }

    fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Error> {
        let name = self.savepoint_name(name)?;
        let shared = lock(&self.shared);
        shared
            .conn
            .execute_batch(&format!("ROLLBACK TO {name}"))
            .map_err(sqlite_err)
    }

    fn release_savepoint(&mut self, name: &str) -> Result<(), Error> {
        let name = self.savepoint_name(name)?;
        let shared = lock(&self.shared);
        release_if_open(&shared.conn, &name)
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}

/// There is one thread in the browser and no statement deadline, so nothing can cancel.
struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

/// `RELEASE`, where an outer `COMMIT` or `ROLLBACK` already ended the savepoint is not an error.
fn release_if_open(conn: &rusqlite::Connection, name: &str) -> Result<(), Error> {
    if conn.is_autocommit() {
        return Ok(());
    }
    match conn.execute_batch(&format!("RELEASE {name}")) {
        Ok(()) => Ok(()),
        Err(err) if err.to_string().contains("no such savepoint") => Ok(()),
        Err(err) => Err(sqlite_err(err)),
    }
}

fn run_query(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[Value],
    cached: bool,
) -> Result<Vec<DbRow>, rusqlite::Error> {
    let bound = to_params(params).map_err(|err| rusqlite::Error::ToSqlConversionFailure(err.into()))?;
    if cached {
        let mut statement = conn.prepare_cached(sql)?;
        collect(&mut statement, &bound)
    } else {
        collect(&mut conn.prepare(sql)?, &bound)
    }
}

fn collect(statement: &mut rusqlite::Statement<'_>, bound: &[SqlValue]) -> Result<Vec<DbRow>, rusqlite::Error> {
    let names: Vec<std::sync::Arc<str>> = statement
        .column_names()
        .iter()
        .map(|name| std::sync::Arc::from(*name))
        .collect();
    let mut rows = statement.query(rusqlite::params_from_iter(bound.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let mut columns = Vec::with_capacity(names.len());
        for (index, name) in names.iter().enumerate() {
            columns.push((name.clone(), from_sql(row.get_ref(index)?)));
        }
        out.push(DbRow { columns });
    }
    Ok(out)
}

fn from_sql(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => Value::I64(number),
        ValueRef::Real(number) => Value::F64(number),
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => Value::String(text.to_string()),
            Err(_) => Value::Bytes(bytes.to_vec()),
        },
        ValueRef::Blob(bytes) => Value::Bytes(bytes.to_vec()),
    }
}

fn to_params(params: &[Value]) -> Result<Vec<SqlValue>, Error> {
    params.iter().map(to_sql).collect()
}

/// Bind one value by storage class. See the module comment for the text formats.
pub fn to_sql(value: &Value) -> Result<SqlValue, Error> {
    Ok(match value {
        Value::Null => SqlValue::Null,
        Value::Bool(flag) => SqlValue::Integer(i64::from(*flag)),
        Value::I32(number) => SqlValue::Integer(i64::from(*number)),
        Value::I64(number) => SqlValue::Integer(*number),
        Value::F64(number) => SqlValue::Real(*number),
        Value::String(text) => SqlValue::Text(text.clone()),
        Value::Bytes(bytes) => SqlValue::Blob(bytes.clone()),
        Value::Decimal(number) => SqlValue::Text(number.to_string()),
        Value::Timestamp(timestamp) => SqlValue::Text(timestamp.strftime("%Y-%m-%d %H:%M:%S%.6f").to_string()),
        Value::Date(date) => SqlValue::Text(date.strftime("%Y-%m-%d").to_string()),
        Value::Time(time) => SqlValue::Text(time.strftime("%H:%M:%S%.6f").to_string()),
        Value::List(_) | Value::Map(_) | Value::Object(_) => {
            return Err(Error::new("a list, map, or object is not a SQL parameter"));
        }
    })
}

fn sqlite_err(err: rusqlite::Error) -> Error {
    Error::new(format!("sqlite: {}", describe(&err)))
}

fn statement_err(err: rusqlite::Error, sql: &str) -> Error {
    Error::new(format!("sqlite: {}", describe(&err))).with_source(Error::new(format!("statement: {sql}")))
}

/// The message, plus the extended result code for an I/O error, which names the failing call
/// (SQLITE_IOERR_FSYNC and the like).
fn describe(err: &rusqlite::Error) -> String {
    match err {
        rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::SystemIoFailure => {
            format!("{err} (extended code {})", code.extended_code)
        }
        _ => err.to_string(),
    }
}

#[cfg(test)]
mod tests;
