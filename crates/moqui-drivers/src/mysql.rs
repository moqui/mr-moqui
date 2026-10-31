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

//! Sync MySQL client behind [`moqui_core::driver::DbDriver`].

#[cfg(feature = "mysql")]
mod driver {
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    use moqui_core::driver::{DbCancel, DbConnection, DbDriver, DbRow};
    use moqui_lib::error::Error;
    use moqui_lib::object::{basic_convert, Value};
    use mysql::consts::{ColumnFlags, ColumnType};
    use mysql::prelude::Queryable;
    use mysql::Value as MysqlValue;

    use crate::statement_lru::StatementLru;

    const STATEMENT_CACHE_CAP: usize = 100;
    /// MySQL `character_set` 63 is `binary`.
    const CHARSET_BINARY: u16 = 63;
    /// `ER_UNSUPPORTED_PS`: the statement cannot be prepared.
    const ER_UNSUPPORTED_PS: u16 = 1295;

    #[derive(Clone)]
    struct Endpoint {
        host: String,
        port: u16,
        username: String,
        password: String,
        database: String,
        tls: bool,
        /// The session's isolation level as SQL writes it (`READ COMMITTED`); `None` leaves the server's default
        /// (InnoDB's is `REPEATABLE READ`).
        isolation: Option<&'static str>,
    }

    impl Endpoint {
        fn opts(&self) -> mysql::OptsBuilder {
            let database = if self.database.is_empty() {
                None
            } else {
                Some(self.database.as_str())
            };
            mysql::OptsBuilder::new()
                .ip_or_hostname(Some(self.host.as_str()))
                .tcp_port(self.port)
                .user(Some(self.username.as_str()))
                .pass(Some(self.password.as_str()))
                .db_name(database)
                .prefer_socket(false)
                .stmt_cache_size(0usize)
                .ssl_opts(self.tls.then(mysql::SslOpts::default))
                .init(self.init())
        }

        /// What each new connection runs first.
        fn init(&self) -> Vec<String> {
            let mut init = vec!["SET NAMES utf8mb4".to_string()];
            if let Some(level) = self.isolation {
                init.push(format!("SET SESSION TRANSACTION ISOLATION LEVEL {level}"));
            }
            init
        }
    }

    struct KillState {
        endpoint: Endpoint,
        connection_id: AtomicU32,
        busy: AtomicBool,
    }

    struct BusyFlag<'a> {
        flag: &'a AtomicBool,
        keep: bool,
    }

    impl<'a> BusyFlag<'a> {
        fn arm(flag: &'a AtomicBool) -> Self {
            flag.store(true, Ordering::Release);
            Self { flag, keep: false }
        }

        fn keep(&mut self) {
            self.keep = true;
        }
    }

    impl Drop for BusyFlag<'_> {
        fn drop(&mut self) {
            if !self.keep {
                self.flag.store(false, Ordering::Release);
            }
        }
    }

    /// The statement that bounds each later statement of the session to `remaining`; `None` lifts the bound. MariaDB
    /// has `max_statement_time` in seconds; MySQL has `max_execution_time` in milliseconds, which bounds only SELECT.
    fn timeout_sql(mariadb: bool, remaining: Option<std::time::Duration>) -> Result<String, Error> {
        let ms = match remaining {
            Some(remaining) => crate::timeout_millis(remaining)?,
            None => 0,
        };
        Ok(if mariadb {
            format!("SET SESSION max_statement_time = {}.{:03}", ms / 1000, ms % 1000)
        } else {
            format!("SET SESSION max_execution_time = {ms}")
        })
    }

    /// One MySQL or MariaDB server. Each [`DbDriver::connect`] opens one connection.
    pub struct MysqlDriver {
        endpoint: Endpoint,
    }

    impl MysqlDriver {
        pub fn new(
            host: impl Into<String>,
            port: u16,
            username: impl Into<String>,
            password: impl Into<String>,
            database: impl Into<String>,
            tls: bool,
        ) -> Self {
            Self {
                endpoint: Endpoint {
                    host: host.into(),
                    port,
                    username: username.into(),
                    password: password.into(),
                    database: database.into(),
                    tls,
                    isolation: None,
                },
            }
        }

        /// Each connection sets its session's isolation level to `level`, as SQL writes it (`READ COMMITTED`), as
        /// Java's pool sets the datasource's `isolation-level` on each connection.
        #[must_use]
        pub fn isolation(mut self, level: Option<&'static str>) -> Self {
            self.endpoint.isolation = level;
            self
        }

        /// Opens one [`mysql::Conn`]. The crate pool is not used.
        pub fn connect_mysql(&self) -> Result<MysqlConn, Error> {
            let mut conn = mysql::Conn::new(self.endpoint.opts()).map_err(mysql_err)?;
            let version: Option<String> = conn.query_first("SELECT VERSION()").map_err(mysql_err)?;
            let kill = Arc::new(KillState {
                endpoint: self.endpoint.clone(),
                connection_id: AtomicU32::new(conn.connection_id()),
                busy: AtomicBool::new(false),
            });
            Ok(MysqlConn {
                stream: None,
                stream_names: None,
                statements: StatementLru::new(STATEMENT_CACHE_CAP),
                kill,
                mariadb: version.as_deref().is_some_and(is_mariadb),
                conn: NonNull::from(Box::leak(Box::new(conn))),
            })
        }
    }

    /// True for a MariaDB server's `VERSION()`, such as `10.11.6-MariaDB-0+deb12u1`.
    fn is_mariadb(version: &str) -> bool {
        version.contains("MariaDB")
    }

    impl DbDriver for MysqlDriver {
        fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
            Ok(Box::new(self.connect_mysql()?))
        }
    }

    /// One MySQL connection. `stream` is dropped before `conn` is freed.
    ///
    /// `conn` is a leaked `Box`, owned by this value and freed in `Drop`. It is a raw pointer rather than a `Box`, so
    /// the `'static` borrow an open `stream` holds is never invalidated by a move or retag of the owner.
    pub struct MysqlConn {
        stream: Option<mysql::QueryResult<'static, 'static, 'static, mysql::Binary>>,
        /// The open stream's column names, from its first row.
        stream_names: Option<Arc<[Arc<str>]>>,
        statements: StatementLru<mysql::Statement>,
        kill: Arc<KillState>,
        /// The server is MariaDB, which bounds statements with `max_statement_time`.
        mariadb: bool,
        conn: NonNull<mysql::Conn>,
    }

    // SAFETY: `conn` is uniquely owned, as a `Box<mysql::Conn>` would be, and `mysql::Conn` is `Send`. Nothing else
    // holds the pointer except the `stream` borrow, which moves with this value.
    unsafe impl Send for MysqlConn {}

    impl MysqlConn {
        pub fn statement_cache_len(&self) -> usize {
            self.statements.len()
        }

        /// The connection, for a call made while no stream is open.
        fn conn(&mut self) -> &mut mysql::Conn {
            self.parts().0
        }

        /// The connection and the statement cache, borrowed together.
        fn parts(&mut self) -> (&mut mysql::Conn, &mut StatementLru<mysql::Statement>) {
            // SAFETY: `conn` came from a leaked `Box` and is freed only in `Drop`, so it is valid and aligned. Every
            // caller is reached after `ensure_idle` (or is `Drop`, after `stream` is gone), so no stream borrow of the
            // connection is live, and the returned borrow is tied to `&mut self`, so it is the only one.
            let conn = unsafe { self.conn.as_mut() };
            (conn, &mut self.statements)
        }

        fn ensure_idle(&self) -> Result<(), Error> {
            if self.stream.is_some() {
                Err(Error::new("mysql stream is open"))
            } else {
                Ok(())
            }
        }

        fn prep_cached(&mut self, sql: &str) -> Result<(), mysql::Error> {
            if self.statements.contains(sql) {
                return Ok(());
            }
            let statement = self.conn().prep(sql)?;
            if let Some(old) = self.statements.insert(sql, statement) {
                self.conn().close(old)?;
            }
            Ok(())
        }

        fn cached_rows(&mut self, sql: &str, params: mysql::Params) -> Result<Vec<DbRow>, Error> {
            let (conn, statements) = self.parts();
            let Some(statement) = statements.get(sql) else {
                return Err(Error::new("mysql statement cache missed"));
            };
            let result = conn.exec_iter(statement, params).map_err(mysql_err)?;
            collect_rows(result)
        }

        fn cached_drain(&mut self, sql: &str, params: mysql::Params) -> Result<(), Error> {
            let (conn, statements) = self.parts();
            let Some(statement) = statements.get(sql) else {
                return Err(Error::new("mysql statement cache missed"));
            };
            let result = conn.exec_iter(statement, params).map_err(mysql_err)?;
            drain(result)
        }

        fn once_rows(&mut self, sql: &str, params_empty: bool, params: mysql::Params) -> Result<Vec<DbRow>, Error> {
            let statement = match self.conn().prep(sql) {
                Ok(statement) => statement,
                Err(err) if params_empty && unsupported_prepared(&err) => {
                    let result = self.conn().query_iter(sql).map_err(mysql_err)?;
                    return collect_rows(result);
                }
                Err(err) => return Err(mysql_err(err)),
            };
            let rows = match self.conn().exec_iter(&statement, params) {
                Ok(result) => collect_rows(result),
                Err(err) => Err(mysql_err(err)),
            };
            let closed = self.conn().close(statement);
            match (rows, closed) {
                (Ok(rows), Ok(())) => Ok(rows),
                (Err(err), _) => Err(err),
                (_, Err(err)) => Err(mysql_err(err)),
            }
        }

        fn rows(&mut self, sql: &str, params: &[Value], cache: bool) -> Result<Vec<DbRow>, Error> {
            self.ensure_idle()?;
            let mysql_params = to_params(params)?;
            let kill = Arc::clone(&self.kill);
            let _busy = BusyFlag::arm(&kill.busy);
            if !cache {
                return self.once_rows(sql, params.is_empty(), mysql_params);
            }
            match self.prep_cached(sql) {
                Ok(()) => self.cached_rows(sql, mysql_params),
                Err(err) if params.is_empty() && unsupported_prepared(&err) => {
                    let result = self.conn().query_iter(sql).map_err(mysql_err)?;
                    collect_rows(result)
                }
                Err(err) => Err(mysql_err(err)),
            }
        }

        fn stream_result(
            &mut self,
            sql: &str,
            params: mysql::Params,
        ) -> Result<mysql::QueryResult<'static, 'static, 'static, mysql::Binary>, Error> {
            let (conn, statements) = self.parts();
            let Some(statement) = statements.get(sql) else {
                return Err(Error::new("mysql statement cache missed"));
            };
            let result = conn.exec_iter(statement, params).map_err(mysql_err)?;
            // SAFETY: `result` borrows the `Conn` behind `self.conn`, a leaked heap allocation that moving `MysqlConn`
            // does not move and that only `Drop` frees, after it drops `stream`. The caller stores `result` in
            // `self.stream`, and every path that would use the connection again (`execute`, `rows`, `command`,
            // `open_stream` and the transaction verbs) calls `ensure_idle` first, so none does while the stream is
            // open. The result does not borrow the `Statement`, and statements are evicted only before a stream opens.
            let result = unsafe {
                std::mem::transmute::<
                    mysql::QueryResult<'_, '_, '_, mysql::Binary>,
                    mysql::QueryResult<'static, 'static, 'static, mysql::Binary>,
                >(result)
            };
            Ok(result)
        }

        fn command(&mut self, sql: String) -> Result<(), Error> {
            self.ensure_idle()?;
            let kill = Arc::clone(&self.kill);
            let _busy = BusyFlag::arm(&kill.busy);
            self.conn().query_drop(sql).map_err(mysql_err)
        }
    }

    impl Drop for MysqlConn {
        fn drop(&mut self) {
            self.kill.busy.store(false, Ordering::Release);
            drop(self.stream.take());
            self.stream_names = None;
            let statements = std::mem::replace(&mut self.statements, StatementLru::new(1));
            for statement in statements.into_values() {
                let _ = self.conn().close(statement);
            }
            // SAFETY: `conn` came from `Box::leak` in `connect_mysql` and is freed only here, once, after the stream
            // that borrowed it is dropped.
            drop(unsafe { Box::from_raw(self.conn.as_ptr()) });
        }
    }

    impl DbConnection for MysqlConn {
        fn returns_rows(&mut self, sql: &str) -> Result<Option<bool>, Error> {
            self.ensure_idle()?;
            // A statement that does not prepare (some cannot be) is left to the first-word check.
            match self.conn().prep(sql) {
                Ok(statement) => {
                    let columns = statement.num_columns();
                    let _ = self.conn().close(statement);
                    Ok(Some(columns > 0))
                }
                Err(_) => Ok(None),
            }
        }

        fn execute(&mut self, sql: &str, params: &[Value]) -> Result<u64, Error> {
            self.ensure_idle()?;
            let mysql_params = to_params(params)?;
            let kill = Arc::clone(&self.kill);
            let _busy = BusyFlag::arm(&kill.busy);
            match self.prep_cached(sql) {
                Ok(()) => {
                    self.cached_drain(sql, mysql_params)?;
                    Ok(self.conn().affected_rows())
                }
                Err(err) if params.is_empty() && unsupported_prepared(&err) => {
                    self.conn().query_drop(sql).map_err(mysql_err)?;
                    Ok(self.conn().affected_rows())
                }
                Err(err) => Err(mysql_err(err)),
            }
        }

        fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
            self.rows(sql, params, true)
        }

        fn query_once(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
            self.rows(sql, params, false)
        }

        fn open_stream(&mut self, sql: &str, params: &[Value]) -> Result<(), Error> {
            self.ensure_idle()?;
            let mysql_params = to_params(params)?;
            let kill = Arc::clone(&self.kill);
            let mut busy = BusyFlag::arm(&kill.busy);
            self.prep_cached(sql).map_err(mysql_err)?;
            let result = self.stream_result(sql, mysql_params)?;
            self.stream = Some(result);
            self.stream_names = None;
            busy.keep();
            Ok(())
        }

        fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
            let Some(stream) = self.stream.as_mut() else {
                return Ok(None);
            };
            match stream.next() {
                Some(Ok(row)) => Ok(Some(map_row(row, &mut self.stream_names)?)),
                Some(Err(err)) => Err(mysql_err(err)),
                None => Ok(None),
            }
        }

        fn close_stream(&mut self) -> Result<(), Error> {
            self.kill.busy.store(false, Ordering::Release);
            drop(self.stream.take());
            Ok(())
        }

        fn stream_open(&self) -> bool {
            self.stream.is_some()
        }

        fn begin(&mut self) -> Result<(), Error> {
            self.command("START TRANSACTION".to_owned())
        }

        // The end of a transaction ends a stream still open on it, as on Postgres, instead of failing on it.
        fn commit(&mut self) -> Result<(), Error> {
            self.close_stream()?;
            self.command("COMMIT".to_owned())
        }

        fn rollback(&mut self) -> Result<(), Error> {
            self.close_stream()?;
            self.command("ROLLBACK".to_owned())
        }

        fn savepoint(&mut self, name: &str) -> Result<(), Error> {
            self.command(format!("SAVEPOINT {}", quote_ident(name)?))
        }

        fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Error> {
            self.command(format!("ROLLBACK TO SAVEPOINT {}", quote_ident(name)?))
        }

        fn release_savepoint(&mut self, name: &str) -> Result<(), Error> {
            self.command(format!("RELEASE SAVEPOINT {}", quote_ident(name)?))
        }

        fn set_statement_timeout(&mut self, remaining: Option<std::time::Duration>) -> Result<(), Error> {
            let sql = timeout_sql(self.mariadb, remaining)?;
            self.command(sql)
        }

        fn canceller(&self) -> Box<dyn DbCancel> {
            Box::new(MysqlCancel {
                state: Arc::clone(&self.kill),
            })
        }
    }

    struct MysqlCancel {
        state: Arc<KillState>,
    }

    impl DbCancel for MysqlCancel {
        fn cancel(&self) -> Result<(), Error> {
            if !self.state.busy.load(Ordering::Acquire) {
                return Ok(());
            }
            let id = self.state.connection_id.load(Ordering::Acquire);
            if id == 0 {
                return Ok(());
            }
            let mut conn = mysql::Conn::new(self.state.endpoint.opts()).map_err(mysql_err)?;
            conn.query_drop(format!("KILL QUERY {id}")).map_err(mysql_err)
        }
    }

    fn mysql_err(err: mysql::Error) -> Error {
        Error::new("mysql request failed").with_source(err)
    }

    fn unsupported_prepared(err: &mysql::Error) -> bool {
        matches!(err, mysql::Error::MySqlError(server) if server.code == ER_UNSUPPORTED_PS)
    }

    fn quote_ident(name: &str) -> Result<String, Error> {
        if name.is_empty() || name.contains('\0') {
            return Err(Error::new("mysql savepoint name is empty"));
        }
        let mut quoted = String::with_capacity(name.len() + 2);
        quoted.push('`');
        for ch in name.chars() {
            if ch == '`' {
                quoted.push('`');
            }
            quoted.push(ch);
        }
        quoted.push('`');
        Ok(quoted)
    }

    fn collect_rows(iter: impl Iterator<Item = mysql::Result<mysql::Row>>) -> Result<Vec<DbRow>, Error> {
        let mut rows = Vec::new();
        let mut names = None;
        for row in iter {
            rows.push(map_row(row.map_err(mysql_err)?, &mut names)?);
        }
        Ok(rows)
    }

    fn drain(iter: impl Iterator<Item = mysql::Result<mysql::Row>>) -> Result<(), Error> {
        for row in iter {
            row.map_err(mysql_err)?;
        }
        Ok(())
    }

    /// `names` are the column names of the result the row is from, made from its first row and shared by the rest.
    fn map_row(row: mysql::Row, names: &mut Option<Arc<[Arc<str>]>>) -> Result<DbRow, Error> {
        let columns = row.columns();
        let names = Arc::clone(names.get_or_insert_with(|| {
            columns
                .iter()
                .map(|column| Arc::from(column.name_str().as_ref()))
                .collect()
        }));
        let values = row.unwrap();
        let mut out = Vec::with_capacity(columns.len());
        for (index, (column, value)) in columns.iter().zip(values).enumerate() {
            let name = names
                .get(index)
                .map_or_else(|| Arc::from(column.name_str().as_ref()), Arc::clone);
            let cell = match map_cell(column, value) {
                Ok(cell) => cell,
                Err(err) => {
                    return Err(Error::new(format!("mysql column {name}")).with_source(err));
                }
            };
            out.push((name, cell));
        }
        Ok(DbRow { columns: out })
    }

    fn map_cell(column: &mysql::Column, value: MysqlValue) -> Result<Value, Error> {
        if matches!(value, MysqlValue::NULL) {
            return Ok(Value::Null);
        }
        let unsigned = column.flags().contains(ColumnFlags::UNSIGNED_FLAG);
        match column.column_type() {
            ColumnType::MYSQL_TYPE_TINY
            | ColumnType::MYSQL_TYPE_SHORT
            | ColumnType::MYSQL_TYPE_INT24
            | ColumnType::MYSQL_TYPE_LONG
            | ColumnType::MYSQL_TYPE_LONGLONG
            | ColumnType::MYSQL_TYPE_YEAR => map_int(column.column_type(), unsigned, value),
            ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => map_float(value),
            ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => map_decimal(value),
            ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => map_date(value),
            ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => map_time(value),
            ColumnType::MYSQL_TYPE_DATETIME
            | ColumnType::MYSQL_TYPE_DATETIME2
            | ColumnType::MYSQL_TYPE_TIMESTAMP
            | ColumnType::MYSQL_TYPE_TIMESTAMP2 => map_datetime(value),
            ColumnType::MYSQL_TYPE_BIT => map_bit(value),
            ColumnType::MYSQL_TYPE_JSON | ColumnType::MYSQL_TYPE_ENUM | ColumnType::MYSQL_TYPE_SET => map_text(value),
            ColumnType::MYSQL_TYPE_GEOMETRY | ColumnType::MYSQL_TYPE_VECTOR => map_bytes(value),
            _ if column.character_set() == CHARSET_BINARY => map_bytes(value),
            _ => map_text(value),
        }
    }

    fn map_int(column_type: ColumnType, _unsigned: bool, value: MysqlValue) -> Result<Value, Error> {
        let number = match value {
            MysqlValue::Int(number) => i128::from(number),
            MysqlValue::UInt(number) => i128::from(number),
            MysqlValue::Bytes(bytes) => parse_integer(&bytes)?,
            other => return Err(Error::new(format!("mysql integer value was {other:?}"))),
        };
        let small = matches!(
            column_type,
            ColumnType::MYSQL_TYPE_TINY
                | ColumnType::MYSQL_TYPE_SHORT
                | ColumnType::MYSQL_TYPE_INT24
                | ColumnType::MYSQL_TYPE_LONG
                | ColumnType::MYSQL_TYPE_YEAR
        );
        if small {
            if let Ok(narrow) = i32::try_from(number) {
                return Ok(Value::I32(narrow));
            }
        }
        if let Ok(wide) = i64::try_from(number) {
            return Ok(Value::I64(wide));
        }
        convert_text(number.to_string(), "BigDecimal")
    }

    fn map_float(value: MysqlValue) -> Result<Value, Error> {
        let number = match value {
            MysqlValue::Float(number) => f64::from(number),
            MysqlValue::Double(number) => number,
            MysqlValue::Int(number) => number as f64,
            MysqlValue::UInt(number) => number as f64,
            MysqlValue::Bytes(bytes) => {
                let text = utf8(&bytes)?;
                text.parse::<f64>()
                    .map_err(|_| Error::new("mysql float was not a number"))?
            }
            other => return Err(Error::new(format!("mysql float value was {other:?}"))),
        };
        Ok(Value::F64(number))
    }

    fn map_decimal(value: MysqlValue) -> Result<Value, Error> {
        let text = match value {
            MysqlValue::Bytes(bytes) => utf8(&bytes)?,
            MysqlValue::Int(number) => number.to_string(),
            MysqlValue::UInt(number) => number.to_string(),
            other => return Err(Error::new(format!("mysql decimal value was {other:?}"))),
        };
        convert_text(text, "BigDecimal")
    }

    /// A zero date such as `0000-00-00`, which MySQL allows, is null, as Connector/J's
    /// `zeroDateTimeBehavior=convertToNull` reads it.
    fn map_date(value: MysqlValue) -> Result<Value, Error> {
        let text = match value {
            MysqlValue::Date(year, month, day, _, _, _, _) => {
                if year == 0 || month == 0 || day == 0 {
                    return Ok(Value::Null);
                }
                format!("{year:04}-{month:02}-{day:02}")
            }
            MysqlValue::Bytes(bytes) => utf8(&bytes)?,
            other => return Err(Error::new(format!("mysql date value was {other:?}"))),
        };
        if zero_date_text(&text) {
            return Ok(Value::Null);
        }
        convert_text(text, "Date")
    }

    fn map_time(value: MysqlValue) -> Result<Value, Error> {
        let text = match value {
            MysqlValue::Time(negative, days, hour, minute, second, micros) => {
                if negative || days != 0 {
                    return Err(Error::new("mysql time is outside a civil time of day"));
                }
                if micros == 0 {
                    format!("{hour:02}:{minute:02}:{second:02}")
                } else {
                    format!("{hour:02}:{minute:02}:{second:02}.{micros:06}")
                }
            }
            MysqlValue::Bytes(bytes) => utf8(&bytes)?,
            other => return Err(Error::new(format!("mysql time value was {other:?}"))),
        };
        convert_text(text, "Time")
    }

    /// A zero date, as in [`map_date`], is null.
    fn map_datetime(value: MysqlValue) -> Result<Value, Error> {
        let text = match value {
            MysqlValue::Date(year, month, day, hour, minute, second, micros) => {
                if year == 0 || month == 0 || day == 0 {
                    return Ok(Value::Null);
                }
                if micros == 0 {
                    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
                } else {
                    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{micros:06}")
                }
            }
            MysqlValue::Bytes(bytes) => utf8(&bytes)?,
            other => return Err(Error::new(format!("mysql datetime value was {other:?}"))),
        };
        if zero_date_text(&text) {
            return Ok(Value::Null);
        }
        convert_text(text, "Timestamp")
    }

    /// True for text-protocol date text whose year, month or day is zero, such as `0000-00-00 00:00:00`.
    fn zero_date_text(text: &str) -> bool {
        let Some(date) = text.get(..10) else {
            return false;
        };
        let parts: Vec<&str> = date.split('-').collect();
        parts.len() == 3
            && parts
                .iter()
                .any(|part| !part.is_empty() && part.bytes().all(|byte| byte == b'0'))
    }

    fn map_text(value: MysqlValue) -> Result<Value, Error> {
        match value {
            MysqlValue::Bytes(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Ok(Value::String(text)),
                Err(err) => Ok(Value::Bytes(err.into_bytes())),
            },
            MysqlValue::Int(number) => Ok(Value::String(number.to_string())),
            MysqlValue::UInt(number) => Ok(Value::String(number.to_string())),
            other => Err(Error::new(format!("mysql text value was {other:?}"))),
        }
    }

    fn map_bytes(value: MysqlValue) -> Result<Value, Error> {
        match value {
            MysqlValue::Bytes(bytes) => Ok(Value::Bytes(bytes)),
            other => Err(Error::new(format!("mysql binary value was {other:?}"))),
        }
    }

    fn map_bit(value: MysqlValue) -> Result<Value, Error> {
        let bytes = match value {
            MysqlValue::Bytes(bytes) => bytes,
            MysqlValue::Int(number) => return Ok(Value::I64(number)),
            other => return map_int(ColumnType::MYSQL_TYPE_LONGLONG, true, other),
        };
        if bytes.len() > 8 {
            return Ok(Value::Bytes(bytes));
        }
        let mut wide = 0u64;
        for byte in bytes {
            wide = (wide << 8) | u64::from(byte);
        }
        if wide > i64::MAX as u64 {
            map_int(ColumnType::MYSQL_TYPE_LONGLONG, true, MysqlValue::UInt(wide))
        } else {
            Ok(Value::I64(wide as i64))
        }
    }

    fn parse_integer(bytes: &[u8]) -> Result<i128, Error> {
        let text = utf8(bytes)?;
        text.parse::<i128>()
            .map_err(|_| Error::new("mysql integer was not a number"))
    }

    fn utf8(bytes: &[u8]) -> Result<String, Error> {
        String::from_utf8(bytes.to_vec()).map_err(|err| Error::new("mysql text was not utf-8").with_source(err))
    }

    fn convert_text(text: String, java_type: &str) -> Result<Value, Error> {
        let converted = basic_convert(&Value::String(text), java_type);
        if converted.changed {
            Ok(converted.value)
        } else {
            Err(Error::new(format!("could not convert mysql value to {java_type}")))
        }
    }

    fn to_params(params: &[Value]) -> Result<mysql::Params, Error> {
        if params.is_empty() {
            return Ok(mysql::Params::Empty);
        }
        let mut values = Vec::with_capacity(params.len());
        for value in params {
            values.push(to_mysql_value(value)?);
        }
        Ok(mysql::Params::Positional(values))
    }

    fn to_mysql_value(value: &Value) -> Result<MysqlValue, Error> {
        Ok(match value {
            Value::Null => MysqlValue::NULL,
            Value::Bool(flag) => MysqlValue::Int(i64::from(*flag)),
            Value::I32(number) => MysqlValue::Int(i64::from(*number)),
            Value::I64(number) => MysqlValue::Int(*number),
            Value::F64(number) => MysqlValue::Double(*number),
            Value::String(text) => MysqlValue::Bytes(text.as_bytes().to_vec()),
            Value::Bytes(bytes) => MysqlValue::Bytes(bytes.clone()),
            Value::Decimal(number) => MysqlValue::Bytes(number.to_string().into_bytes()),
            Value::Date(date) => MysqlValue::Bytes(date.strftime("%Y-%m-%d").to_string().into_bytes()),
            Value::Time(time) => MysqlValue::Bytes(time.strftime("%H:%M:%S%.6f").to_string().into_bytes()),
            Value::Timestamp(timestamp) => {
                MysqlValue::Bytes(timestamp.strftime("%Y-%m-%d %H:%M:%S%.6f").to_string().into_bytes())
            }
            Value::List(_) | Value::Map(_) | Value::Object(_) => {
                return Err(Error::new("mysql parameters must be scalar values"));
            }
        })
    }

    // Inline `mod driver` loads children from mysql/driver/, not from beside mysql.rs.
    #[cfg(test)]
    mod tests;
}

#[cfg(feature = "mysql")]
pub use driver::{MysqlConn, MysqlDriver};
