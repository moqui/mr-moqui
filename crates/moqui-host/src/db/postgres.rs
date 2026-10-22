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

//! Postgres behind [`moqui_core::driver::DbDriver`], on the protocol client of [`super::pgwire`].
//!
//! A call is a round trip on a blocking socket: one write, one read. A `BEGIN`, a `SET LOCAL statement_timeout`, a
//! deferred write ([`DbConnection::execute_deferred`]), a stream's `DECLARE` and its `CLOSE` are not waited for: they
//! share the round trip of the next call that waits, which reads their replies first and reports the first failure
//! among them.

#[cfg(feature = "postgres")]
mod driver {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Arc;

    use bytes::BytesMut;
    use moqui_core::driver::{DbCancel, DbConnection, DbDriver, DbRow};
    use moqui_lib::codec::hex_encode;
    use moqui_lib::error::Error;
    use moqui_lib::object::{basic_convert, Value};
    use postgres_types::Type;

    use crate::db::pgwire::{self, Column, Endpoint, Wire};
    use crate::db::statement_lru::StatementLru;

    /// Statements kept prepared on a connection, as pgjdbc's `preparedStatementCacheQueries` default. A data load of
    /// many entities prepares several statements of each (see [`BATCH_SIZES`]); at 100 they pushed each other out.
    const CACHE_CAP: usize = 256;
    const FETCH_BATCH: usize = 50;
    /// Rows a statement of [`RowBatch`] takes at most, and the smaller sizes the end of a run of rows goes out in: up
    /// to five statements of an entity's columns prepared on a connection, which [`CACHE_CAP`] leaves room for.
    const BATCH_SIZES: [usize; 5] = [100, 30, 10, 3, 1];
    /// Parameters a [`RowBatch`] statement takes at most, well under the protocol's 65,535.
    const BATCH_PARAMS: usize = 30_000;
    const STREAM_CURSOR: &str = "moqui_stream_cursor";
    /// Every parameter in text: the server parses each as its column's type.
    const ALL_TEXT: [i16; 1] = [0];
    /// Seconds from the Unix epoch to 2000-01-01 00:00:00 UTC.
    const PG_EPOCH_SECS: i64 = 946_684_800;
    const PG_EPOCH_USEC: i64 = PG_EPOCH_SECS * 1_000_000;
    /// Days from 1970-01-01 to 2000-01-01.
    const PG_EPOCH_DAYS: i64 = 10_957;

    /// Username, database, and either a Unix socket directory or a TCP host.
    #[derive(Clone)]
    pub struct PostgresDriver {
        user: String,
        database: String,
        password: Option<String>,
        target: Target,
        /// libpq `options`: server settings for each session, such as `-c statement_timeout=30000`.
        options: Option<String>,
    }

    #[derive(Clone)]
    enum Target {
        #[cfg(unix)]
        Unix { socket_dir: PathBuf, port: u16 },
        /// Loopback TCP with no TLS. Embedded Postgres on Windows uses this.
        TcpNoTls { host: String, port: u16 },
        Tcp {
            host: String,
            port: u16,
            tls: Arc<rustls::ClientConfig>,
        },
    }

    impl Target {
        fn endpoint(&self) -> Endpoint {
            match self {
                #[cfg(unix)]
                Target::Unix { socket_dir, port } => Endpoint::Unix(socket_dir.join(format!(".s.PGSQL.{port}"))),
                Target::TcpNoTls { host, port } => Endpoint::Tcp {
                    host: host.clone(),
                    port: *port,
                    tls: None,
                },
                Target::Tcp { host, port, tls } => Endpoint::Tcp {
                    host: host.clone(),
                    port: *port,
                    tls: Some(Arc::clone(tls)),
                },
            }
        }
    }

    /// One connection. Statement LRU is keyed by the original SQL text.
    pub struct PostgresConn {
        wire: Wire,
        endpoint: Endpoint,
        cache: StatementLru<Arc<Prepared>>,
        /// The number of the next statement prepared, which names it.
        next_statement: u64,
        in_transaction: bool,
        streaming: bool,
        stream_exhausted: bool,
        stream_buf: VecDeque<DbRow>,
        /// Rows [`DbConnection::execute_deferred_row`] holds. Every other call sends them first, and a rollback drops
        /// them.
        batch: Option<RowBatch>,
    }

    /// Rows of one `INSERT ... VALUES (?, ...)` statement, gathered to go out as statements of many rows.
    struct RowBatch {
        sql: String,
        /// Where the `VALUES` row of `sql` starts and ends: what is before it, and what follows it (an
        /// `ON CONFLICT` clause), are the same in a statement of many rows.
        row: std::ops::Range<usize>,
        /// Parameters a row takes.
        width: usize,
        /// The most rows a statement takes.
        most: usize,
        params: Vec<Value>,
        keys: std::collections::HashSet<u64>,
    }

    impl RowBatch {
        /// A batch for `sql` when it is an `INSERT` of one row of `width` placeholders, the `VALUES` row only
        /// placeholders.
        fn new(sql: &str, width: usize) -> Option<Self> {
            if width == 0 || !sql.starts_with("INSERT INTO ") {
                return None;
            }
            let start = sql.find(" VALUES (")? + " VALUES ".len();
            let end = start + sql[start..].find(')')? + 1;
            let inner = &sql[start + 1..end - 1];
            if inner.bytes().any(|byte| !matches!(byte, b'?' | b',' | b' '))
                || inner.bytes().filter(|byte| *byte == b'?').count() != width
            {
                return None;
            }
            Some(Self {
                sql: sql.to_string(),
                row: start..end,
                width,
                most: BATCH_SIZES[0].min(BATCH_PARAMS / width).max(1),
                params: Vec::new(),
                keys: std::collections::HashSet::new(),
            })
        }

        fn rows(&self) -> usize {
            self.params.len() / self.width
        }

        /// The statement of `rows` rows.
        fn statement(&self, rows: usize) -> String {
            if rows == 1 {
                return self.sql.clone();
            }
            let row = &self.sql[self.row.clone()];
            let mut sql = String::with_capacity(self.sql.len() + (row.len() + 2) * (rows - 1));
            sql.push_str(&self.sql[..self.row.start]);
            for index in 0..rows {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push_str(row);
            }
            sql.push_str(&self.sql[self.row.end..]);
            sql
        }
    }

    /// A statement prepared on the connection: its name there, its parameter types and its result columns.
    struct Prepared {
        name: String,
        params: Vec<Type>,
        columns: Arc<[Column]>,
    }

    impl Prepared {
        /// Each parameter in text, except bytes for a `bytea`, sent as they are rather than as hex text twice their size
        /// (which also overflows the 1 GB field limit at half of it). One code for all when every one is text.
        fn formats(&self, params: &[Value]) -> Vec<i16> {
            if !params.iter().zip(&self.params).any(|(value, ty)| is_binary(value, Some(ty))) {
                return ALL_TEXT.to_vec();
            }
            params
                .iter()
                .enumerate()
                .map(|(index, value)| i16::from(is_binary(value, self.params.get(index))))
                .collect()
        }
    }

    struct PgCancel {
        endpoint: Endpoint,
        process_id: i32,
        secret_key: i32,
    }

    impl PostgresDriver {
        /// Unix socket directory and no TLS. The port is the server default, 5432.
        #[cfg(unix)]
        pub fn unix_socket(
            user: impl Into<String>,
            database: impl Into<String>,
            socket_dir: impl Into<PathBuf>,
        ) -> Self {
            Self {
                user: user.into(),
                database: database.into(),
                password: None,
                options: None,
                target: Target::Unix {
                    socket_dir: socket_dir.into(),
                    port: 5432,
                },
            }
        }

        /// The port that names the socket file (`.s.PGSQL.<port>`) of a [`Self::unix_socket`]
        /// driver. A TCP driver keeps the port it was made with.
        #[cfg(unix)]
        #[must_use]
        pub fn port(mut self, value: u16) -> Self {
            if let Target::Unix { port, .. } = &mut self.target {
                *port = value;
            }
            self
        }

        /// TCP host and no TLS. Embedded Windows uses `127.0.0.1` and the ephemeral port.
        /// External servers stay on [`Self::external`].
        #[must_use]
        pub fn tcp_no_tls(
            user: impl Into<String>,
            database: impl Into<String>,
            host: impl Into<String>,
            port: u16,
        ) -> Self {
            Self {
                user: user.into(),
                database: database.into(),
                password: None,
                options: None,
                target: Target::TcpNoTls {
                    host: host.into(),
                    port,
                },
            }
        }

        /// TCP host and TLS (rustls, with the `webpki-roots` certificate authorities), which the server must accept.
        ///
        /// # Errors
        ///
        /// Returns an error when the TLS configuration cannot be built.
        pub fn external(
            user: impl Into<String>,
            database: impl Into<String>,
            host: impl Into<String>,
            port: u16,
        ) -> Result<Self, Error> {
            Self::external_with(user, database, host, port, &crate::db::pg_tls::TlsOptions::default())
        }

        /// TCP host and TLS as `tls` says: the authorities trusted and what of the certificate is checked.
        ///
        /// # Errors
        ///
        /// Returns an error when the root certificate file cannot be read or the TLS configuration cannot be built.
        pub fn external_with(
            user: impl Into<String>,
            database: impl Into<String>,
            host: impl Into<String>,
            port: u16,
            tls: &crate::db::pg_tls::TlsOptions,
        ) -> Result<Self, Error> {
            let tls = crate::db::pg_tls::client_config(tls)?;
            Ok(Self {
                user: user.into(),
                database: database.into(),
                password: None,
                options: None,
                target: Target::Tcp {
                    host: host.into(),
                    port,
                    tls,
                },
            })
        }

        /// Server settings sent with each new session, the libpq `options` string
        /// (`-c statement_timeout=30000 -c default_transaction_read_only=on`).
        #[must_use]
        pub fn options(mut self, options: impl Into<String>) -> Self {
            self.options = Some(options.into());
            self
        }

        /// Password for the role. Embedded initdb uses password auth.
        #[must_use]
        pub fn password(mut self, password: impl Into<String>) -> Self {
            self.password = Some(password.into());
            self
        }

        /// Open one client. The returned connection is the concrete type tests cache-count.
        ///
        /// # Errors
        ///
        /// Returns an error when the server rejects the connection or `SET TIME ZONE`.
        pub fn connect_postgres(&self) -> Result<PostgresConn, Error> {
            let endpoint = self.target.endpoint();
            let wire = Wire::connect(
                &endpoint,
                &self.user,
                &self.database,
                self.password.as_deref(),
                self.options.as_deref(),
            )?;
            let mut conn = PostgresConn {
                wire,
                endpoint,
                cache: StatementLru::new(CACHE_CAP),
                next_statement: 0,
                in_transaction: false,
                streaming: false,
                stream_exhausted: false,
                stream_buf: VecDeque::new(),
                batch: None,
            };
            conn.wire.simple("SET TIME ZONE 'UTC'", "set time zone")?;
            Ok(conn)
        }
    }

    impl DbDriver for PostgresDriver {
        fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
            Ok(Box::new(self.connect_postgres()?))
        }
    }

    impl PostgresConn {
        /// Prepared statements stored for `execute` and `query`. `query_once` does not count.
        #[must_use]
        pub fn statement_cache_len(&self) -> usize {
            self.cache.len()
        }

        /// `LISTEN` on `channel`, an identifier the caller controls.
        ///
        /// # Errors
        ///
        /// Returns an error when the channel is not a plain identifier or the server refuses.
        pub fn listen(&mut self, channel: &str) -> Result<(), Error> {
            if channel.is_empty() || !channel.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                return Err(Error::new(format!("{channel:?} is not a channel name")));
            }
            self.wire.simple(&format!("LISTEN {channel}"), "listen")
        }

        /// The payloads of the notifications that arrive within `timeout` of the call, and any already
        /// waiting behind the first. Empty when none came.
        ///
        /// # Errors
        ///
        /// Returns an error when the connection is lost.
        pub fn notifications(&mut self, timeout: std::time::Duration) -> Result<Vec<String>, Error> {
            self.wire.notifications(timeout)
        }

        /// True when the server has closed this connection.
        #[must_use]
        pub fn is_closed(&self) -> bool {
            self.wire.is_dead()
        }

        fn prepared(&mut self, sql: &str) -> Result<Arc<Prepared>, Error> {
            if let Some(prepared) = self.cache.get(sql) {
                return Ok(Arc::clone(prepared));
            }
            let name = self.statement_name();
            let described = self.wire.prepare(&name, &rewrite_placeholders(sql))?;
            self.remember(sql, name, described)
        }

        /// The cached statement for `sql`; `None` when there is none and it can be prepared in its first run
        /// ([`Self::first_run`]), which sends every parameter in text: none of them is bytes.
        fn cached(&mut self, sql: &str, params: &[Value]) -> Result<Option<Arc<Prepared>>, Error> {
            if let Some(prepared) = self.cache.get(sql) {
                return Ok(Some(Arc::clone(prepared)));
            }
            if params.iter().any(|value| matches!(value, Value::Bytes(_))) {
                return self.prepared(sql).map(Some);
            }
            Ok(None)
        }

        /// Prepare `sql` and run it in one round trip, and keep the statement. Its rows and the rows it changed.
        fn first_run(&mut self, sql: &str, params: &[Value], context: &str) -> Result<(Vec<DbRow>, u64), Error> {
            let name = self.statement_name();
            let (described, rows, changed) = self.wire.prepare_run(
                &name,
                &rewrite_placeholders(sql),
                params.len(),
                &mut |index, buf| encode_param(&params[index], None, buf),
                &decode_cell,
                context,
            )?;
            self.remember(sql, name, described)?;
            Ok((rows, changed))
        }

        /// Send the rows [`DbConnection::execute_deferred_row`] holds, unanswered: as statements of the largest
        /// [`BATCH_SIZES`] that fit, in order.
        fn flush_batch(&mut self) -> Result<(), Error> {
            let Some(batch) = self.batch.take() else {
                return Ok(());
            };
            let rows = batch.rows();
            let mut done = 0;
            while done < rows {
                let left = rows - done;
                // The largest a statement takes, then the smaller sizes.
                let size = std::iter::once(batch.most)
                    .chain(BATCH_SIZES.iter().copied().filter(|size| *size < batch.most))
                    .find(|size| *size <= left)
                    .unwrap_or(1);
                let chunk = &batch.params[done * batch.width..(done + size) * batch.width];
                done += size;
                let prepared = self.prepared(&batch.statement(size))?;
                let formats = prepared.formats(chunk);
                self.wire.send_execute(
                    &prepared.name,
                    &formats,
                    chunk.len(),
                    &mut |index, buf| encode_param(&chunk[index], prepared.params.get(index), buf),
                    None,
                )?;
            }
            Ok(())
        }

        fn statement_name(&mut self) -> String {
            let name = format!("s{}", self.next_statement);
            self.next_statement += 1;
            name
        }

        /// Keep statement `name` for `sql`, and close the one the cache lets go for it.
        fn remember(&mut self, sql: &str, name: String, described: pgwire::Described) -> Result<Arc<Prepared>, Error> {
            let prepared = Arc::new(Prepared {
                name,
                params: described.params,
                columns: described.columns,
            });
            if let Some(evicted) = self.cache.insert(sql, Arc::clone(&prepared)) {
                self.wire.close_statement(&evicted.name)?;
            }
            Ok(prepared)
        }

        /// `COMMIT` waits for the deferred requests first and fails with the first of them that failed: the transaction
        /// is then aborted, and a `COMMIT` would roll it back without an error. `ROLLBACK` drops their failures. `COMMIT`
        /// closes an open stream's cursor first. `ROLLBACK` does not, since it closes the cursor itself and a `CLOSE` in
        /// an aborted transaction would fail before the rollback is sent. Either way the stream is gone.
        fn finish_transaction(&mut self, sql: &str) -> Result<(), Error> {
            let commit = sql == "COMMIT";
            if commit {
                self.flush_batch()?;
                self.wire.finish()?;
            } else {
                self.batch = None;
                self.wire.abandon();
            }
            let close = self.streaming && commit;
            let closed = if close { self.close_stream() } else { Ok(()) };
            self.clear_stream();
            closed?;
            self.wire.simple(sql, sql)?;
            self.in_transaction = false;
            Ok(())
        }

        fn fetch_stream(&mut self) -> Result<(), Error> {
            let sql = format!("FETCH {FETCH_BATCH} FROM {STREAM_CURSOR}");
            let (rows, _) = self
                .wire
                .query_unnamed(&sql, 0, &mut |_, _| Ok(false), &decode_cell, "fetch cursor")?;
            if rows.len() < FETCH_BATCH {
                self.stream_exhausted = true;
            }
            self.stream_buf.extend(rows);
            Ok(())
        }

        fn clear_stream(&mut self) {
            self.streaming = false;
            self.stream_exhausted = false;
            self.stream_buf.clear();
        }

        /// `sql`, with `?` placeholders, as the unnamed statement, parameters in text.
        fn run_unnamed(&mut self, sql: &str, params: &[Value], context: &str) -> Result<(Vec<DbRow>, u64), Error> {
            check_scalar(params)?;
            let rewritten = rewrite_placeholders(sql);
            self.wire.query_unnamed(
                &rewritten,
                params.len(),
                &mut |index, buf| encode_param(&params[index], None, buf),
                &decode_cell,
                context,
            )
        }
    }

    impl DbConnection for PostgresConn {
        /// Also true for an idle connection the server has since closed (a restart), so the pool does not hand it out.
        fn is_closed(&self) -> bool {
            self.wire.closed_while_idle()
        }

        fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
            self.flush_batch()?;
            check_scalar(params)?;
            let Some(prepared) = self.cached(sql, params)? else {
                return Ok(self.first_run(sql, params, "execute")?.1);
            };
            let formats = prepared.formats(params);
            self.wire.execute(
                &prepared.name,
                &formats,
                params.len(),
                &mut |index, buf| encode_param(&params[index], prepared.params.get(index), buf),
            )
        }

        /// In a transaction the statement is sent and its reply left for the next call that waits, or the commit. Out
        /// of one it runs as [`Self::execute`]: the connection goes back to the pool after it.
        fn execute_deferred(&mut self, sql: &str, params: Vec<Value>, zero_rows: Option<String>) -> Result<(), Error> {
            self.flush_batch()?;
            if !self.in_transaction {
                let count = self.execute(sql, &params)?;
                return match zero_rows {
                    Some(message) if count == 0 => Err(Error::new(message)),
                    _ => Ok(()),
                };
            }
            check_scalar(&params)?;
            let prepared = self.prepared(sql)?;
            let formats = prepared.formats(&params);
            self.wire.send_execute(
                &prepared.name,
                &formats,
                params.len(),
                &mut |index, buf| encode_param(&params[index], prepared.params.get(index), buf),
                zero_rows,
            )
        }

        /// In a transaction the row is held with the rows before it of the same statement, and they go out as one
        /// statement when the next row is of another, or names a key among them, or fills the statement, or another
        /// call is made. Out of one, or for a statement that is not an `INSERT` of one row, it goes out alone.
        fn execute_deferred_row(&mut self, sql: &str, params: Vec<Value>, key: u64) -> Result<(), Error> {
            if !self.in_transaction {
                return self.execute_deferred(sql, params, None);
            }
            check_scalar(&params)?;
            if self
                .batch
                .as_ref()
                .is_some_and(|batch| batch.sql != sql || batch.width != params.len() || batch.keys.contains(&key))
            {
                self.flush_batch()?;
            }
            if self.batch.is_none() {
                match RowBatch::new(sql, params.len()) {
                    Some(batch) => self.batch = Some(batch),
                    None => return self.execute_deferred(sql, params, None),
                }
            }
            let Some(batch) = self.batch.as_mut() else {
                return Ok(());
            };
            batch.params.extend(params);
            batch.keys.insert(key);
            // A full statement goes out now, so the server works on it while the next rows are read.
            if batch.rows() >= batch.most {
                self.flush_batch()?;
            }
            Ok(())
        }

        fn finish_deferred(&mut self) -> Result<(), Error> {
            self.flush_batch()?;
            self.wire.finish()
        }

        fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
            self.flush_batch()?;
            check_scalar(params)?;
            let Some(prepared) = self.cached(sql, params)? else {
                return Ok(self.first_run(sql, params, "query")?.0);
            };
            let formats = prepared.formats(params);
            self.wire.query(
                &prepared.name,
                &prepared.columns,
                &formats,
                params.len(),
                &mut |index, buf| encode_param(&params[index], prepared.params.get(index), buf),
                &decode_cell,
            )
        }

        fn returns_rows(&mut self, sql: &str) -> Result<Option<bool>, Error> {
            self.flush_batch()?;
            // A deferred failure is reported, not taken for this statement's.
            self.wire.finish()?;
            // Not kept in the statement cache: a statement typed in the SQL runner runs once. One that does not prepare
            // is left to run, so its own error is the one reported.
            match self.wire.prepare("", &rewrite_placeholders(sql)) {
                Ok(described) => Ok(Some(!described.columns.is_empty())),
                Err(_) => Ok(None),
            }
        }

        fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
            self.flush_batch()?;
            Ok(self.run_unnamed(sql, params, "query")?.0)
        }

        fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error> {
            self.flush_batch()?;
            if !self.in_transaction {
                return Err(Error::new("a transaction is required"));
            }
            if self.streaming {
                return Err(Error::new("a stream is already open"));
            }
            check_scalar(params)?;
            let declare = rewrite_placeholders(&format!("DECLARE {STREAM_CURSOR} NO SCROLL CURSOR FOR {sql}"));
            self.wire.queue_unnamed(
                &declare,
                params.len(),
                &mut |index, buf| encode_param(&params[index], None, buf),
                "declare cursor",
            )?;
            self.streaming = true;
            self.stream_exhausted = false;
            self.stream_buf.clear();
            // The first batch in the same write as the `DECLARE`: one round trip, and a query the server refuses fails
            // here with the `DECLARE`'s error.
            if let Err(err) = self.fetch_stream() {
                self.clear_stream();
                return Err(err);
            }
            Ok(())
        }

        fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
            self.flush_batch()?;
            if !self.streaming {
                return Ok(None);
            }
            if self.stream_buf.is_empty() {
                if self.stream_exhausted {
                    return Ok(None);
                }
                self.fetch_stream()?;
            }
            Ok(self.stream_buf.pop_front())
        }

        fn close_stream(&mut self) -> Result<(), Error> {
            self.flush_batch()?;
            if !self.streaming {
                return Ok(());
            }
            // Not waited for: it goes out with the next request, which reports its failure.
            self.wire
                .queue_simple(&format!("CLOSE {STREAM_CURSOR}"), "close cursor")?;
            self.clear_stream();
            Ok(())
        }

        fn stream_open(&self) -> bool {
            self.streaming
        }

        /// Not waited for: it goes out with the transaction's first statement, in the same write.
        fn begin(&mut self) -> Result<(), Error> {
            self.wire.queue_simple("BEGIN", "begin")?;
            self.in_transaction = true;
            Ok(())
        }

        fn commit(&mut self) -> Result<(), Error> {
            self.finish_transaction("COMMIT")
        }

        fn rollback(&mut self) -> Result<(), Error> {
            self.finish_transaction("ROLLBACK")
        }

        fn savepoint(&mut self, name: &str) -> Result<(), Error> {
            self.flush_batch()?;
            self.wire.simple(&format!("SAVEPOINT {}", quote_ident(name)), "savepoint")
        }

        fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Error> {
            self.flush_batch()?;
            self.wire.simple(
                &format!("ROLLBACK TO SAVEPOINT {}", quote_ident(name)),
                "rollback to savepoint",
            )
        }

        fn release_savepoint(&mut self, name: &str) -> Result<(), Error> {
            self.flush_batch()?;
            self.wire
                .simple(&format!("RELEASE SAVEPOINT {}", quote_ident(name)), "release savepoint")
        }

        /// `SET LOCAL` lasts until the transaction ends, so clearing is a no-op. Not waited for, as `BEGIN` is not.
        fn set_statement_timeout(&mut self, remaining: Option<std::time::Duration>) -> Result<(), Error> {
            self.flush_batch()?;
            let Some(remaining) = remaining else {
                return Ok(());
            };
            let ms = crate::db::timeout_millis(remaining)?;
            self.wire
                .queue_simple(&format!("SET LOCAL statement_timeout = {ms}"), "statement_timeout")
        }

        fn canceller(&self) -> Box<dyn DbCancel> {
            let (process_id, secret_key) = self.wire.cancel_key();
            Box::new(PgCancel {
                endpoint: self.endpoint.clone(),
                process_id,
                secret_key,
            })
        }
    }

    impl DbCancel for PgCancel {
        /// On the calling thread, the deadline thread, on a connection of its own.
        fn cancel(&self) -> Result<(), Error> {
            pgwire::cancel(&self.endpoint, self.process_id, self.secret_key)
        }
    }

    /// Whether `value` goes in binary: bytes for a `bytea` parameter.
    fn is_binary(value: &Value, ty: Option<&Type>) -> bool {
        matches!(value, Value::Bytes(_)) && ty == Some(&Type::BYTEA)
    }

    /// One parameter into the `Bind` message, in the format [`Prepared::formats`] names for it: the text the server
    /// parses as its column's type, or a `bytea`'s bytes. `false` for a NULL.
    fn encode_param(value: &Value, ty: Option<&Type>, out: &mut BytesMut) -> Result<bool, Error> {
        if let (Value::Bytes(bytes), true) = (value, is_binary(value, ty)) {
            out.extend_from_slice(bytes);
            return Ok(true);
        }
        // The common kinds go straight into the buffer, as `value_text` writes them.
        use std::fmt::Write as _;
        let written = match value {
            Value::Null => return Ok(false),
            Value::String(text) => {
                out.extend_from_slice(text.as_bytes());
                Ok(())
            }
            Value::Bool(flag) => {
                out.extend_from_slice(if *flag { b"true" } else { b"false" });
                Ok(())
            }
            Value::I32(number) => write!(out, "{number}"),
            Value::I64(number) => write!(out, "{number}"),
            Value::Decimal(number) => write!(out, "{number}"),
            Value::Timestamp(timestamp) => write!(out, "{timestamp}"),
            Value::Date(date) => write!(out, "{date}"),
            Value::Time(time) => write!(out, "{time}"),
            _ => {
                return match value_text(value).map_err(|err| Error::new(err.to_string()))? {
                    Some(text) => {
                        out.extend_from_slice(text.as_bytes());
                        Ok(true)
                    }
                    None => Ok(false),
                };
            }
        };
        written.map_err(|_| Error::new("could not write a postgres parameter"))?;
        Ok(true)
    }

    /// A result cell, in binary, as its type's value; a NULL is [`Value::Null`].
    fn decode_cell(ty: &Type, raw: Option<&[u8]>) -> Result<Value, Error> {
        match raw {
            None => Ok(Value::Null),
            Some(raw) => decode_value(ty, raw).map_err(|err| Error::new(err.to_string())),
        }
    }

    fn check_scalar(params: &[Value]) -> Result<(), Error> {
        if params
            .iter()
            .any(|value| matches!(value, Value::List(_) | Value::Map(_)))
        {
            return Err(Error::new("postgres parameters must be scalar values"));
        }
        Ok(())
    }

    /// JDBC `?` placeholders as `$1`, `$2`, ... A `?` inside a string (`'..'`, `E'..'`, `$tag$..$tag$`), a quoted
    /// identifier or a comment is left as it is.
    fn rewrite_placeholders(sql: &str) -> String {
        let bytes = sql.as_bytes();
        let mut out = String::with_capacity(sql.len() + 8);
        let mut index = 0u32;
        let mut copied = 0;
        let mut at = 0;
        while let Some(&byte) = bytes.get(at) {
            let next = bytes.get(at + 1).copied();
            at = match byte {
                b'\'' => quoted_end(bytes, at + 1, b'\'', escape_string(bytes, at)),
                b'"' => quoted_end(bytes, at + 1, b'"', false),
                b'-' if next == Some(b'-') => bytes[at..]
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(bytes.len(), |offset| at + offset + 1),
                b'/' if next == Some(b'*') => block_comment_end(bytes, at + 2),
                b'$' => dollar_quote_end(sql, at).unwrap_or(at + 1),
                b'?' => {
                    index += 1;
                    out.push_str(&sql[copied..at]);
                    out.push('$');
                    out.push_str(&index.to_string());
                    copied = at + 1;
                    at + 1
                }
                _ => at + 1,
            };
        }
        out.push_str(&sql[copied..]);
        out
    }

    /// True when the quote at `at` opens an `E'..'` string, in which a backslash escapes the next character.
    fn escape_string(bytes: &[u8], at: usize) -> bool {
        let Some(prefix) = at.checked_sub(1).and_then(|before| bytes.get(before)) else {
            return false;
        };
        let word_before = at
            .checked_sub(2)
            .and_then(|before| bytes.get(before))
            .is_some_and(|&byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$');
        matches!(prefix, b'E' | b'e') && !word_before
    }

    /// The index past the `quote` that closes a string or identifier whose body starts at `from`. A doubled quote, or
    /// with `backslash` a backslash, escapes.
    fn quoted_end(bytes: &[u8], from: usize, quote: u8, backslash: bool) -> usize {
        let mut at = from;
        while let Some(&byte) = bytes.get(at) {
            if backslash && byte == b'\\' {
                at += 2;
            } else if byte == quote {
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

    /// The index past the `*/` that closes a block comment whose body starts at `from`. Postgres comments nest.
    fn block_comment_end(bytes: &[u8], from: usize) -> usize {
        let mut depth = 1usize;
        let mut at = from;
        while let Some(&byte) = bytes.get(at) {
            let next = bytes.get(at + 1).copied();
            if byte == b'/' && next == Some(b'*') {
                depth += 1;
                at += 2;
            } else if byte == b'*' && next == Some(b'/') {
                depth -= 1;
                at += 2;
                if depth == 0 {
                    return at;
                }
            } else {
                at += 1;
            }
        }
        bytes.len()
    }

    /// For a `$tag$` (or `$$`) at `at` that opens a dollar-quoted string, the index past its closing tag. `None` when
    /// the `$` opens no tag, as in a `$1` parameter, or follows an identifier character.
    fn dollar_quote_end(sql: &str, at: usize) -> Option<usize> {
        let bytes = sql.as_bytes();
        let after_word = at
            .checked_sub(1)
            .and_then(|before| bytes.get(before))
            .is_some_and(|&byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if after_word {
            return None;
        }
        let rest = sql.get(at + 1..)?;
        let tag_len = rest.find('$')?;
        let tag = &rest[..tag_len];
        let valid = tag.bytes().enumerate().all(|(position, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (position > 0 && byte.is_ascii_digit())
        });
        if !valid {
            return None;
        }
        let delimiter = &sql[at..at + tag_len + 2];
        let body = at + tag_len + 2;
        Some(
            sql[body..]
                .find(delimiter)
                .map_or(sql.len(), |offset| body + offset + delimiter.len()),
        )
    }

    fn quote_ident(name: &str) -> String {
        let mut quoted = String::with_capacity(name.len() + 2);
        quoted.push('"');
        for ch in name.chars() {
            if ch == '"' {
                quoted.push_str("\"\"");
            } else {
                quoted.push(ch);
            }
        }
        quoted.push('"');
        quoted
    }

    fn value_text(value: &Value) -> Result<Option<String>, Box<dyn std::error::Error + Sync + Send>> {
        let text = match value {
            Value::Null => return Ok(None),
            Value::String(text) => text.clone(),
            Value::Bool(true) => "true".to_string(),
            Value::Bool(false) => "false".to_string(),
            Value::I32(number) => number.to_string(),
            Value::I64(number) => number.to_string(),
            Value::F64(number) => format_f64(*number),
            Value::Decimal(number) => number.to_string(),
            Value::Timestamp(timestamp) => timestamp.to_string(),
            Value::Date(date) => date.to_string(),
            Value::Time(time) => time.to_string(),
            Value::Bytes(bytes) => format_bytea(bytes),
            Value::List(_) | Value::Map(_) | Value::Object(_) => {
                return Err("postgres parameters must be scalar values".into());
            }
        };
        Ok(Some(text))
    }

    fn format_f64(number: f64) -> String {
        if number.is_nan() {
            "NaN".to_string()
        } else if number.is_infinite() && number.is_sign_negative() {
            "-Infinity".to_string()
        } else if number.is_infinite() {
            "Infinity".to_string()
        } else {
            number.to_string()
        }
    }

    fn format_bytea(bytes: &[u8]) -> String {
        let mut text = String::with_capacity(2 + bytes.len() * 2);
        text.push_str("\\x");
        text.push_str(&hex_encode(bytes));
        text
    }

    fn decode_value(ty: &Type, raw: &[u8]) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        if ty == &Type::BOOL {
            decode_bool(raw)
        } else if ty == &Type::INT2 {
            Ok(Value::I32(i32::from(i16::from_be_bytes(read_array(raw)?))))
        } else if ty == &Type::INT4 {
            Ok(Value::I32(i32::from_be_bytes(read_array(raw)?)))
        } else if ty == &Type::INT8 {
            Ok(Value::I64(i64::from_be_bytes(read_array(raw)?)))
        } else if ty == &Type::FLOAT4 {
            Ok(Value::F64(f64::from(f32::from_be_bytes(read_array(raw)?))))
        } else if ty == &Type::FLOAT8 {
            Ok(Value::F64(f64::from_be_bytes(read_array(raw)?)))
        } else if is_text(ty) {
            decode_text(raw)
        } else if ty == &Type::JSONB {
            // Binary jsonb is a version byte, 1, then the JSON text.
            match raw.split_first() {
                Some((1, text)) => decode_text(text),
                _ => Err(bad("bad jsonb")),
            }
        } else if ty == &Type::BYTEA {
            Ok(Value::Bytes(raw.to_vec()))
        } else if ty == &Type::NUMERIC {
            expect_value(numeric_to_text(raw)?, "BigDecimal")
        } else if ty == &Type::TIMESTAMPTZ || ty == &Type::TIMESTAMP {
            decode_timestamp(raw)
        } else if ty == &Type::DATE {
            decode_date(raw)
        } else if ty == &Type::TIME {
            decode_time(raw)
        } else {
            Ok(decode_unknown(ty, raw))
        }
    }

    fn is_text(ty: &Type) -> bool {
        ty == &Type::TEXT
            || ty == &Type::VARCHAR
            || ty == &Type::BPCHAR
            || ty == &Type::NAME
            || ty == &Type::UNKNOWN
            || ty == &Type::JSON
            || ty == &Type::XML
    }

    fn decode_bool(raw: &[u8]) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        match raw {
            [0] => Ok(Value::Bool(false)),
            [1] => Ok(Value::Bool(true)),
            _ => Err(bad("bad bool")),
        }
    }

    fn decode_text(raw: &[u8]) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        let text = std::str::from_utf8(raw).map_err(|_| bad("invalid utf8"))?;
        Ok(Value::string(text))
    }

    fn decode_unknown(ty: &Type, raw: &[u8]) -> Value {
        match std::str::from_utf8(raw) {
            Ok(text) => Value::string(text),
            Err(_) => Value::string(format!("{ty}:{}", hex_encode(raw))),
        }
    }

    fn decode_timestamp(raw: &[u8]) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        let micros = i64::from_be_bytes(read_array(raw)?);
        if micros == i64::MAX {
            return Ok(Value::string("infinity"));
        }
        if micros == i64::MIN {
            return Ok(Value::string("-infinity"));
        }
        // The moment the text below names (UTC, no offset), without writing and parsing that text. Outside jiff's range
        // the text decides.
        if let Some(stamp) = micros
            .checked_add(PG_EPOCH_USEC)
            .and_then(|unix_us| jiff::Timestamp::from_microsecond(unix_us).ok())
        {
            return Ok(Value::Timestamp(stamp));
        }
        expect_value(format_pg_timestamp(micros)?, "Timestamp")
    }

    fn decode_date(raw: &[u8]) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        let days = i32::from_be_bytes(read_array(raw)?);
        if days == i32::MAX {
            return Ok(Value::string("infinity"));
        }
        if days == i32::MIN {
            return Ok(Value::string("-infinity"));
        }
        let unix_days = i64::from(days) + PG_EPOCH_DAYS;
        let (year, month, day) = civil_from_unix_days(unix_days).ok_or_else(|| bad("date"))?;
        expect_value(format!("{year:04}-{month:02}-{day:02}"), "Date")
    }

    fn decode_time(raw: &[u8]) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        let micros = i64::from_be_bytes(read_array(raw)?);
        if micros < 0 {
            return Err(bad("time"));
        }
        let secs = micros / 1_000_000;
        let frac = u32::try_from(micros % 1_000_000).map_err(|_| bad("time"))?;
        if secs > 86_400 {
            return Err(bad("time"));
        }
        let hour = secs / 3_600;
        let minute = (secs % 3_600) / 60;
        let second = secs % 60;
        expect_value(format!("{hour:02}:{minute:02}:{second:02}.{frac:06}"), "Time")
    }

    fn format_pg_timestamp(micros: i64) -> Result<String, Box<dyn std::error::Error + Sync + Send>> {
        let unix_us = micros
            .checked_add(PG_EPOCH_USEC)
            .ok_or_else(|| bad("timestamp overflow"))?;
        let secs = unix_us.div_euclid(1_000_000);
        let frac = u32::try_from(unix_us.rem_euclid(1_000_000)).map_err(|_| bad("timestamp fraction"))?;
        let days = secs.div_euclid(86_400);
        let tod = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_unix_days(days).ok_or_else(|| bad("timestamp date"))?;
        let hour = tod / 3_600;
        let minute = (tod % 3_600) / 60;
        let second = tod % 60;
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{frac:06}"
        ))
    }

    fn expect_value(text: String, kind: &str) -> Result<Value, Box<dyn std::error::Error + Sync + Send>> {
        let text = Value::String(text);
        let converted = basic_convert(&text, kind);
        if kind_matches(&converted.value, kind) {
            Ok(converted.value)
        } else {
            Err(bad(&format!("could not decode {kind} from {}", text.as_str().unwrap_or_default())))
        }
    }

    fn kind_matches(value: &Value, kind: &str) -> bool {
        matches!(
            (value, kind),
            (Value::Decimal(_), "BigDecimal")
                | (Value::Timestamp(_), "Timestamp")
                | (Value::Date(_), "Date")
                | (Value::Time(_), "Time")
        )
    }

    fn read_array<const N: usize>(raw: &[u8]) -> Result<[u8; N], Box<dyn std::error::Error + Sync + Send>> {
        raw.try_into().map_err(|_| bad("short postgres value"))
    }

    fn bad(message: &str) -> Box<dyn std::error::Error + Sync + Send> {
        message.into()
    }

    fn numeric_to_text(raw: &[u8]) -> Result<String, Box<dyn std::error::Error + Sync + Send>> {
        if raw.len() < 8 {
            return Err(bad("short numeric"));
        }
        let ndigits = i16::from_be_bytes([raw[0], raw[1]]);
        let weight = i32::from(i16::from_be_bytes([raw[2], raw[3]]));
        let sign = u16::from_be_bytes([raw[4], raw[5]]);
        let dscale_raw = i16::from_be_bytes([raw[6], raw[7]]);
        if ndigits < 0 || dscale_raw < 0 {
            return Err(bad("bad numeric header"));
        }
        match sign {
            0x0000 | 0x4000 => {}
            0xC000 => return Err(bad("numeric NaN")),
            0xD000 => return Err(bad("numeric infinity")),
            0xF000 => return Err(bad("numeric -infinity")),
            _ => return Err(bad("numeric sign")),
        }
        let ndigits = usize::from(u16::try_from(ndigits).map_err(|_| bad("bad numeric digits"))?);
        let dscale = usize::from(u16::try_from(dscale_raw).map_err(|_| bad("bad numeric scale"))?);
        if ndigits > 2_000 || dscale > 8_000 || !(-2_000..=2_000).contains(&weight) {
            return Err(bad("numeric too large"));
        }
        if raw.len() < 8 + ndigits * 2 {
            return Err(bad("short numeric"));
        }
        let digit_at = |index: usize| {
            let offset = 8 + index * 2;
            u16::from_be_bytes([raw[offset], raw[offset + 1]])
        };
        if (0..ndigits).any(|index| digit_at(index) >= 10_000) {
            return Err(bad("numeric digit"));
        }
        let leading = if weight >= 0 {
            0
        } else {
            let leading = usize::try_from(-weight - 1).map_err(|_| bad("numeric weight"))?;
            if leading > 2_000 {
                return Err(bad("numeric too large"));
            }
            leading
        };
        // Written into one string: base-10000 digit groups, the first without leading zeros, then the fraction to
        // `dscale` digits.
        use std::fmt::Write as _;
        let mut text = String::with_capacity(ndigits * 4 + leading * 4 + dscale + 8);
        if sign == 0x4000 {
            text.push('-');
        }
        let groups = if weight >= 0 {
            let groups = usize::try_from(weight + 1).map_err(|_| bad("numeric weight"))?;
            for index in 0..groups {
                let digit = if index < ndigits { digit_at(index) } else { 0 };
                let _ = if index == 0 {
                    write!(text, "{digit}")
                } else {
                    write!(text, "{digit:04}")
                };
            }
            groups
        } else {
            text.push('0');
            0
        };
        if dscale > 0 {
            text.push('.');
            let fraction_start = text.len();
            for _ in 0..leading {
                text.push_str("0000");
            }
            for index in groups.min(ndigits)..ndigits {
                let _ = write!(text, "{:04}", digit_at(index));
            }
            let fraction_end = fraction_start + dscale;
            if text.len() < fraction_end {
                let pad = fraction_end - text.len();
                text.extend(std::iter::repeat_n('0', pad));
            } else {
                text.truncate(fraction_end);
            }
        }
        Ok(text)
    }

    /// Howard Hinnant's `civil_from_days`. `days` is the Unix day count.
    fn civil_from_unix_days(days: i64) -> Option<(i32, u32, u32)> {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        if month <= 2 {
            year += 1;
        }
        Some((
            i32::try_from(year).ok()?,
            u32::try_from(month).ok()?,
            u32::try_from(day).ok()?,
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_parameter_writes_the_text_value_text_gives() {
            let values = [
                Value::Null,
                Value::string("it's"),
                Value::Bool(true),
                Value::Bool(false),
                Value::I32(-5),
                Value::I64(1 << 40),
                basic_convert(&Value::string("12.340"), "BigDecimal").value,
                Value::Timestamp(jiff::Timestamp::from_microsecond(1_700_000_000_123_456).unwrap()),
                Value::Date(jiff::civil::date(2024, 2, 29)),
                Value::Time(jiff::civil::time(13, 4, 5, 123_000_000)),
                Value::F64(1.5),
                Value::F64(f64::NEG_INFINITY),
            ];
            for value in &values {
                let mut out = BytesMut::new();
                let written = encode_param(value, Some(&Type::TEXT), &mut out).unwrap();
                match value_text(value).unwrap() {
                    Some(text) => {
                        assert!(written, "{value:?}");
                        assert_eq!(&out[..], text.as_bytes(), "{value:?}");
                    }
                    None => assert!(!written, "{value:?}"),
                }
            }
        }

        #[test]
        fn a_binary_timestamp_decodes_to_the_moment_its_text_names() {
            for micros in [0_i64, 1, -1, 999_999, 813_456_789_012_345, -946_684_800_000_000, 2_000_000_000_000_000] {
                let raw = micros.to_be_bytes();
                let direct = decode_timestamp(&raw).unwrap();
                let text = expect_value(format_pg_timestamp(micros).unwrap(), "Timestamp").unwrap();
                assert_eq!(direct, text, "{micros}");
            }
        }

        #[test]
        fn placeholders_skip_quoted_question_marks() {
            assert_eq!(rewrite_placeholders("SELECT ?"), "SELECT $1");
            assert_eq!(rewrite_placeholders("SELECT ?, ?"), "SELECT $1, $2");
            assert_eq!(rewrite_placeholders("SELECT '?'"), "SELECT '?'");
            assert_eq!(rewrite_placeholders("SELECT 'it''s ?', ?"), "SELECT 'it''s ?', $1");
            assert_eq!(rewrite_placeholders("SELECT 'a''?''b' || ?"), "SELECT 'a''?''b' || $1");
        }

        #[test]
        fn placeholders_skip_identifiers_comments_and_other_strings() {
            assert_eq!(rewrite_placeholders(r#"SELECT "a?b", ?"#), r#"SELECT "a?b", $1"#);
            assert_eq!(rewrite_placeholders(r#"SELECT "a""?", ?"#), r#"SELECT "a""?", $1"#);
            assert_eq!(rewrite_placeholders("SELECT ? -- why?\n, ?"), "SELECT $1 -- why?\n, $2");
            assert_eq!(rewrite_placeholders("SELECT ? -- why?"), "SELECT $1 -- why?");
            assert_eq!(
                rewrite_placeholders("SELECT /* a? /* b? */ c? */ ?"),
                "SELECT /* a? /* b? */ c? */ $1"
            );
            assert_eq!(rewrite_placeholders("SELECT $$a?b$$, ?"), "SELECT $$a?b$$, $1");
            assert_eq!(
                rewrite_placeholders("SELECT $fn$ ? $x$ ? $fn$, ?"),
                "SELECT $fn$ ? $x$ ? $fn$, $1"
            );
            assert_eq!(rewrite_placeholders(r"SELECT E'a\'?', ?"), r"SELECT E'a\'?', $1");
            assert_eq!(rewrite_placeholders(r"SELECT e'\\', ?"), r"SELECT e'\\', $1");
            // A backslash in a standard string is a plain character, and `name'..'` is no E-string.
            assert_eq!(rewrite_placeholders(r"SELECT '\', ?"), r"SELECT '\', $1");
            assert_eq!(rewrite_placeholders(r"SELECT some'\', ?"), r"SELECT some'\', $1");
            // `$1` and `a$b` open no dollar quote.
            assert_eq!(rewrite_placeholders("SELECT $1, a$b, ?"), "SELECT $1, a$b, $1");
            assert_eq!(rewrite_placeholders("SELECT 'é?', ?"), "SELECT 'é?', $1");
        }

        #[test]
        fn numeric_binary_decodes_base_10000_digits() {
            let raw = [
                0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x0F, 0x6E, 0x04, 0xD2, 0x15, 0xE0,
            ];
            assert_eq!(numeric_to_text(&raw).unwrap(), "3950.123456");
            let negative = [
                0x00, 0x03, 0x00, 0x00, 0x40, 0x00, 0x00, 0x06, 0x0F, 0x6E, 0x04, 0xD2, 0x15, 0xE0,
            ];
            assert_eq!(numeric_to_text(&negative).unwrap(), "-3950.123456");
            assert_eq!(
                numeric_to_text(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]).unwrap(),
                "0"
            );
        }

        #[test]
        fn civil_dates_match_the_unix_epoch() {
            assert_eq!(civil_from_unix_days(0), Some((1970, 1, 1)));
            assert_eq!(civil_from_unix_days(10_957), Some((2000, 1, 1)));
            assert_eq!(civil_from_unix_days(18_262), Some((2020, 1, 1)));
        }

        #[test]
        fn external_builds_a_rustls_connector_or_reports_it() {
            let driver = PostgresDriver::external("user", "db", "localhost", 5432).expect("rustls connector");
            assert!(matches!(driver.target, Target::Tcp { port: 5432, .. }));
        }

        #[test]
        fn tcp_no_tls_builds_a_loopback_target() {
            let driver = PostgresDriver::tcp_no_tls("postgres", "moqui", "127.0.0.1", 5432);
            assert!(matches!(
                driver.target,
                Target::TcpNoTls {
                    ref host,
                    port: 5432
                } if host == "127.0.0.1"
            ));
        }
    }
}

#[cfg(feature = "postgres")]
pub use driver::{PostgresConn, PostgresDriver};
