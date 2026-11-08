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

//! Database driver boundary. No Postgres or MySQL types.
//!
//! SQL text uses `?` placeholders outside quotes. A driver rewrites those for
//! its client. `query` and `execute` may cache a prepared statement, cap 100,
//! keyed by SQL text. [`DbConnection::query_once`] must not take a cache slot.
//! An open stream pins the connection until [`DbConnection::close_stream`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use moqui_lib::sync::{read, write};
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::Value;

/// One result row. Column order is the select list order. A driver shares each column name among the rows of one
/// result (and of one prepared statement), so a row allocates for its values only.
#[derive(Clone, Debug, PartialEq)]
pub struct DbRow {
    pub columns: Vec<(Arc<str>, Value)>,
    /// Where each column comes from, in the order of `columns`, shared by the rows of one result; `None` when the
    /// driver does not tell (plans/archive/Tools-Database.md 9.5). A column the server names no table for (an
    /// expression, a constant) is `None` in it.
    pub origins: Option<Arc<[Option<ColumnOrigin>]>>,
}

/// The table column a result column was read from, as the wire protocol describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnOrigin {
    /// A Postgres `RowDescription` field: the table's oid and the column's attribute number. A view's column names
    /// the view. [`crate::entity::info::table_names_of_oids`] gives the table names.
    Postgres { table_oid: u32, column: i16 },
    /// A MySQL column definition: the database, `org_table` and `org_name`.
    Mysql {
        schema: Arc<str>,
        table: Arc<str>,
        column: Arc<str>,
    },
}

impl DbRow {
    /// A row with these columns and no origins.
    #[must_use]
    pub fn new(columns: Vec<(Arc<str>, Value)>) -> Self {
        Self { columns, origins: None }
    }

    /// The origin of the column at `index`, when the driver told it.
    #[must_use]
    pub fn origin(&self, index: usize) -> Option<&ColumnOrigin> {
        self.origins.as_ref()?.get(index)?.as_ref()
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.columns
            .iter()
            .find(|(column, _)| &**column == name)
            .map(|(_, value)| value)
    }
}

/// Waited round trips of every thread ([`note_round_trip`]).
static ROUND_TRIPS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static THREAD_ROUND_TRIPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A driver calls this each time it waits for the reply to what it has sent: one waited round trip, however many
/// statements went out before it (plans/archive/TX-Cache.md D1). Statements sent without waiting count when a later call
/// waits for them.
pub fn note_round_trip() {
    ROUND_TRIPS.fetch_add(1, Ordering::Relaxed);
    THREAD_ROUND_TRIPS.with(|count| count.set(count.get() + 1));
}

/// The waited round trips of every thread so far, beside [`crate::entity::statement_stats`].
#[must_use]
pub fn round_trips() -> u64 {
    ROUND_TRIPS.load(Ordering::Relaxed)
}

/// The waited round trips of this thread so far: a driver waits on the thread that called it.
#[must_use]
pub fn thread_round_trips() -> u64 {
    THREAD_ROUND_TRIPS.with(std::cell::Cell::get)
}

/// Opens connections. The pool calls this. One connection is one client.
pub trait DbDriver: Send + Sync {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error>;
}

/// One checked-out client. `Send` so a transaction can park it off the thread.
///
/// `begin` starts a transaction on this client. It does not check a client out
/// of a pool. Commit and rollback end that transaction. Savepoint names are
/// chosen by the caller.
pub trait DbConnection: Send {
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error>;

    /// [`Self::execute`] without waiting for the result, where the driver can: in a transaction the statement is sent
    /// and the next call that waits (or [`Self::finish_deferred`], or the commit) reports its failure, and
    /// `zero_rows` as the error when it changed no row. Statements run in the order sent. The default runs it now.
    fn execute_deferred(&mut self, sql: &str, params: Vec<Value>, zero_rows: Option<String>) -> Result<(), Error> {
        let count = self.execute(sql, &params)?;
        match zero_rows {
            Some(message) if count == 0 => Err(Error::new(message)),
            _ => Ok(()),
        }
    }

    /// [`Self::execute_deferred`] of an `UPDATE` that must change a row: `missing` is the error when it changed none.
    /// The default checks the count here, which a commit after it has to wait for; a driver that can have the server
    /// fail the statement instead does, so the commit goes out with it.
    fn execute_deferred_update(&mut self, sql: &str, params: Vec<Value>, missing: String) -> Result<(), Error> {
        self.execute_deferred(sql, params, Some(missing))
    }

    /// [`Self::execute_deferred`] of a one-row `INSERT` (an upsert among them) that the driver may hold and send with
    /// the rows of the same statement that follow it, as one `INSERT` of many rows. `key` names the row's primary key:
    /// a row whose key is among those held goes after them, as one upsert cannot change a row twice. A row changing no
    /// row is not an error. The default sends it alone.
    fn execute_deferred_row(&mut self, sql: &str, params: Vec<Value>, key: u64) -> Result<(), Error> {
        let _ = key;
        self.execute_deferred(sql, params, None)
    }

    /// Wait for the statements [`Self::execute_deferred`] sent, and report the first that failed.
    fn finish_deferred(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error>;

    fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error>;

    /// Open a stream of the rows of `sql`, read with [`Self::next_row`] until it gives `None` or
    /// [`Self::close_stream`]. One stream per connection; it lives on the transaction's enlisted connection
    /// (the entity iterator begins a transaction when none is in place), and the pool discards a connection
    /// that comes back with one open.
    ///
    /// How much a stream holds in memory is the driver's: Postgres fetches through a cursor a batch at a time
    /// and MySQL reads an active result, so memory stays bounded; the browser's SQLite reads the whole result
    /// here, before the first row, so a stream there costs as much memory as a list and an early stop still
    /// pays for every row (plans/High-Level-Review.md HL-003).
    fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error>;

    /// The next row of the open stream; `None` at its end, or with no stream open.
    fn next_row(&mut self) -> Result<Option<DbRow>, Error>;

    fn close_stream(&mut self) -> Result<(), Error>;

    fn stream_open(&self) -> bool;

    /// The server has closed this client, or a previous call lost the connection. A local check: it does not
    /// ask the database. The default is false.
    fn is_closed(&self) -> bool {
        false
    }

    fn begin(&mut self) -> Result<(), Error>;

    fn commit(&mut self) -> Result<(), Error>;

    fn rollback(&mut self) -> Result<(), Error>;

    fn savepoint(&mut self, name: &str) -> Result<(), Error>;
    /// [`Self::savepoint`] sent behind the statements still unanswered, with no wait for them: their failure, if any,
    /// comes out of the next call that waits, and a rollback to this savepoint then fails. For a caller that passes
    /// such a failure on as the transaction's. The default is [`Self::savepoint`].
    fn savepoint_behind(&mut self, name: &str) -> Result<(), Error> {
        self.savepoint(name)
    }

    fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Error>;

    fn release_savepoint(&mut self, name: &str) -> Result<(), Error>;

    /// A handle the deadline thread can call while this connection is borrowed.
    fn canceller(&self) -> Box<dyn DbCancel>;

    /// The server's id of this session, as its activity lists it: the Postgres backend pid (`pg_backend_pid()`) or
    /// the MySQL `CONNECTION_ID()`. `None` when the driver does not know it (plans/archive/Tools-Database.md 8.3).
    fn backend_id(&self) -> Option<i64> {
        None
    }

    /// Make the transaction just begun on this connection read only, before any statement in it, so the database
    /// refuses a write (plans/archive/Tools-Database.md 8.2). The default is Postgres' `SET TRANSACTION READ ONLY`.
    fn set_transaction_read_only(&mut self) -> Result<(), Error> {
        self.execute("SET TRANSACTION READ ONLY", &[]).map(|_| ())
    }

    /// Tell the server this session's [`application_name`] after it changed, for a session opened before. A driver
    /// sends the name when it connects; one that can change it later (Postgres) does it here. The default does
    /// nothing.
    fn set_application_name(&mut self, name: &str) -> Result<(), Error> {
        let _ = name;
        Ok(())
    }

    /// Whether `sql` gives rows, from the database's description of the prepared statement, as JDBC's `execute`
    /// tells a query from an update. `None` when this driver cannot tell or the statement does not prepare.
    fn returns_rows(&mut self, sql: &str) -> Result<Option<bool>, Error> {
        let _ = sql;
        Ok(None)
    }

    /// The longest one read from the database waits, so a host that stops answering without a reset does not hold
    /// the caller until the kernel gives up: the manager sets the transaction's remaining time plus a margin when the
    /// connection joins a transaction, and `None` when it goes back to the pool. A read past it fails and the
    /// connection is closed. The default does nothing.
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), Error> {
        let _ = timeout;
        Ok(())
    }

    /// Remaining statement time. `None` clears it. The default does nothing.
    ///
    /// Postgres sets `statement_timeout`. MySQL sets `max_execution_time`. The manager calls this
    /// only when `use-statement-timeout` is on.
    fn set_statement_timeout(&mut self, remaining: Option<Duration>) -> Result<(), Error> {
        let _ = remaining;
        Ok(())
    }
}

/// The name each session of this process gives the database, so its activity can tell the app's own sessions, and
/// one cluster member's from another's (plans/archive/Tools-Database.md 7.3): Postgres `application_name`, a MySQL
/// connection attribute. Core sets it to the instance id. The generation counts the changes, so the pool can bring a
/// session opened before a change up to date ([`DbConnection::set_application_name`]).
static APPLICATION_NAME: RwLock<Option<Arc<str>>> = RwLock::new(None);
static APPLICATION_NAME_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Name this process's database sessions `name` from now on; an empty name is ignored.
pub fn set_application_name(name: &str) {
    if name.is_empty() {
        return;
    }
    let mut slot = write(&APPLICATION_NAME);
    if slot.as_deref() == Some(name) {
        return;
    }
    *slot = Some(Arc::from(name));
    APPLICATION_NAME_GENERATION.fetch_add(1, Ordering::Release);
}

/// The name a new database session gives the server, once [`set_application_name`] has set one.
#[must_use]
pub fn application_name() -> Option<Arc<str>> {
    read(&APPLICATION_NAME).clone()
}

/// How many times [`set_application_name`] changed the name.
#[must_use]
pub fn application_name_generation() -> u64 {
    APPLICATION_NAME_GENERATION.load(Ordering::Acquire)
}

/// A database named by its `database-list` conf name and its address, as Java's `getConfConnection` takes from the
/// `entity_ds_*` entries of a map. The connection is its own, not one from the entity pool.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ConfDb {
    /// The `database-list` name, such as `postgres` or `mysql8`.
    pub db_type: String,
    pub host: String,
    /// Empty is the database's default port.
    pub port: String,
    pub database: String,
    pub user: String,
    pub password: String,
    /// `sslmode`: empty keeps the driver's default (TLS to a Postgres host that is not loopback or a socket, none to
    /// MySQL); `disable` turns TLS off; `require`, `verify-ca` and `verify-full` (MySQL's `REQUIRED`, `VERIFY_CA`,
    /// `VERIFY_IDENTITY`) turn it on and say what of the server's certificate is checked.
    pub ssl_mode: String,
    /// `sslrootcert`: the path of a PEM file of the authorities to trust in place of the public ones.
    pub ssl_root_cert: String,
}

impl std::fmt::Debug for ConfDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfDb")
            .field("db_type", &self.db_type)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("ssl_mode", &self.ssl_mode)
            .field("ssl_root_cert", &self.ssl_root_cert)
            .finish()
    }
}

/// Makes a driver for a [`ConfDb`]. The host installs one; it knows the database clients.
pub type ConfDriverFactory = dyn Fn(&ConfDb) -> Result<Arc<dyn DbDriver>, Error> + Send + Sync;

fn conf_driver_slot() -> &'static RwLock<Option<Arc<ConfDriverFactory>>> {
    static SLOT: RwLock<Option<Arc<ConfDriverFactory>>> = RwLock::new(None);
    &SLOT
}

/// Use `factory` for every [`conf_driver`] in this process.
pub fn install_conf_driver_factory(factory: Arc<ConfDriverFactory>) {
    *write(conf_driver_slot()) = Some(factory);
}

/// A driver for the database `conf` names.
///
/// # Errors
///
/// When the host installed no factory, or the factory cannot make a driver for the conf name.
pub fn conf_driver(conf: &ConfDb) -> Result<Arc<dyn DbDriver>, Error> {
    let factory = read(conf_driver_slot())
        .clone()
        .ok_or_else(|| Error::new("no database client for conf connections is installed in this host"))?;
    factory(conf)
}

/// Cancels the statement running on another thread. A connection with no
/// active statement returns success. A cancel runs on a deadline worker: it bounds its own network work (connect,
/// handshake, the request) with a short deadline of its own, so a host that does not answer does not hold the worker.
pub trait DbCancel: Send + Sync {
    fn cancel(&self) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::*;

    #[test]
    fn conf_db_debug_leaves_out_the_password() {
        let conf = ConfDb {
            db_type: "postgres".to_string(),
            user: "app".to_string(),
            password: "s3cret-pass".to_string(),
            ..ConfDb::default()
        };
        let text = format!("{conf:?}");
        assert!(!text.contains("s3cret-pass"), "{text}");
        assert!(text.contains("postgres") && text.contains("app"), "{text}");
    }

    struct FakeDriver;

    struct FakeConn {
        cancelled: Arc<AtomicBool>,
        streaming: bool,
        rows_left: i64,
    }

    impl DbConnection for FakeConn {
        fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, Error> {
            Ok(1)
        }

        fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
            self.query_once(sql, params)
        }

        fn query_once(&mut self, _sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
            Ok(vec![DbRow {
                columns: vec![("n".into(), params.first().cloned().unwrap_or(Value::Null))],
                origins: None,
            }])
        }

        fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
            self.streaming = true;
            self.rows_left = 2;
            Ok(())
        }

        fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
            if !self.streaming || self.rows_left == 0 {
                return Ok(None);
            }
            self.rows_left -= 1;
            Ok(Some(DbRow {
                columns: vec![("n".into(), Value::I64(self.rows_left))],
                origins: None,
            }))
        }

        fn close_stream(&mut self) -> Result<(), Error> {
            self.streaming = false;
            self.rows_left = 0;
            Ok(())
        }

        fn stream_open(&self) -> bool {
            self.streaming
        }

        fn begin(&mut self) -> Result<(), Error> {
            Ok(())
        }

        fn commit(&mut self) -> Result<(), Error> {
            Ok(())
        }

        fn rollback(&mut self) -> Result<(), Error> {
            Ok(())
        }

        fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
            Ok(())
        }

        fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
            Ok(())
        }

        fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
            Ok(())
        }

        fn canceller(&self) -> Box<dyn DbCancel> {
            let flag = Arc::clone(&self.cancelled);
            Box::new(FlagCancel(flag))
        }
    }

    struct FlagCancel(Arc<AtomicBool>);

    impl DbCancel for FlagCancel {
        fn cancel(&self) -> Result<(), Error> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    impl DbDriver for FakeDriver {
        fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
            Ok(Box::new(FakeConn {
                cancelled: Arc::new(AtomicBool::new(false)),
                streaming: false,
                rows_left: 0,
            }))
        }
    }

    #[test]
    fn trait_objects_query_stream_and_cancel() {
        let driver: Arc<dyn DbDriver> = Arc::new(FakeDriver);
        let mut conn = driver.connect().unwrap();
        let cancel = conn.canceller();
        cancel.cancel().unwrap();
        let rows = conn.query_once("select ?", &[Value::I64(3)]).unwrap();
        assert_eq!(rows[0].get("n"), Some(&Value::I64(3)));
        conn.open_stream("select n", &[]).unwrap();
        assert!(conn.stream_open());
        assert!(conn.next_row().unwrap().is_some());
        assert!(conn.next_row().unwrap().is_some());
        assert!(conn.next_row().unwrap().is_none());
        conn.close_stream().unwrap();
        assert!(!conn.stream_open());
    }
}
