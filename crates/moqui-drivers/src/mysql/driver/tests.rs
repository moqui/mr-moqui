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

use super::*;

fn must<T>(result: Result<T, Error>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("{err}"),
    }
}

fn column(column_type: ColumnType) -> mysql::Column {
    mysql::Column::new(column_type).with_character_set(45)
}

#[test]
fn types_are_send() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Send + Sync>() {}
    assert_send::<MysqlConn>();
    assert_sync::<MysqlCancel>();
    assert_sync::<MysqlDriver>();
}

#[test]
fn maps_mysql_values() {
    assert_eq!(
        must(map_cell(&column(ColumnType::MYSQL_TYPE_LONG), MysqlValue::NULL)),
        Value::Null
    );
    assert_eq!(
        must(map_cell(&column(ColumnType::MYSQL_TYPE_LONG), MysqlValue::Int(7))),
        Value::I32(7)
    );
    assert_eq!(
        must(map_cell(&column(ColumnType::MYSQL_TYPE_LONGLONG), MysqlValue::Int(7))),
        Value::I64(7)
    );
    let unsigned_int = column(ColumnType::MYSQL_TYPE_LONG).with_flags(ColumnFlags::UNSIGNED_FLAG);
    assert_eq!(
        must(map_cell(&unsigned_int, MysqlValue::Int(3_000_000_000))),
        Value::I64(3_000_000_000)
    );
    let big = must(map_cell(
        &column(ColumnType::MYSQL_TYPE_LONGLONG).with_flags(ColumnFlags::UNSIGNED_FLAG),
        MysqlValue::UInt(u64::MAX),
    ));
    assert_eq!(
        big,
        basic_convert(&Value::String(u64::MAX.to_string()), "BigDecimal").value
    );
    assert_eq!(
        must(map_cell(
            &column(ColumnType::MYSQL_TYPE_DOUBLE),
            MysqlValue::Double(1.5)
        )),
        Value::F64(1.5)
    );
    let decimal = must(map_cell(
        &column(ColumnType::MYSQL_TYPE_NEWDECIMAL),
        MysqlValue::Bytes(b"12.50".to_vec()),
    ));
    assert_eq!(
        decimal,
        basic_convert(&Value::String("12.50".into()), "BigDecimal").value
    );
    let date = must(map_cell(
        &column(ColumnType::MYSQL_TYPE_DATE),
        MysqlValue::Date(2020, 1, 2, 0, 0, 0, 0),
    ));
    assert_eq!(date, basic_convert(&Value::String("2020-01-02".into()), "Date").value);
    let time = must(map_cell(
        &column(ColumnType::MYSQL_TYPE_TIME),
        MysqlValue::Time(false, 0, 3, 4, 5, 6),
    ));
    assert_eq!(
        time,
        basic_convert(&Value::String("03:04:05.000006".into()), "Time").value
    );
    let stamp = must(map_cell(
        &column(ColumnType::MYSQL_TYPE_DATETIME),
        MysqlValue::Date(2020, 1, 2, 3, 4, 5, 0),
    ));
    assert_eq!(
        stamp,
        basic_convert(&Value::String("2020-01-02 03:04:05".into()), "Timestamp").value
    );
    assert_eq!(
        must(map_cell(
            &column(ColumnType::MYSQL_TYPE_VARCHAR),
            MysqlValue::Bytes(b"one".to_vec())
        )),
        Value::string("one")
    );
    let binary = mysql::Column::new(ColumnType::MYSQL_TYPE_BLOB).with_character_set(CHARSET_BINARY);
    assert_eq!(
        must(map_cell(&binary, MysqlValue::Bytes(b"\0\xff".to_vec()))),
        Value::Bytes(vec![0, 255])
    );
    assert_eq!(
        must(map_cell(
            &column(ColumnType::MYSQL_TYPE_BIT),
            MysqlValue::Bytes(vec![0x01])
        )),
        Value::I64(1)
    );
    assert!(to_mysql_value(&Value::List(vec![])).is_err());
    assert_eq!(must(quote_ident("a`b")), "`a``b`");
    assert!(quote_ident("").is_err());
}

#[test]
fn zero_dates_are_null() {
    let date = column(ColumnType::MYSQL_TYPE_DATE);
    let stamp = column(ColumnType::MYSQL_TYPE_DATETIME);
    assert_eq!(
        must(map_cell(&date, MysqlValue::Date(0, 0, 0, 0, 0, 0, 0))),
        Value::Null
    );
    assert_eq!(
        must(map_cell(&date, MysqlValue::Date(2020, 0, 0, 0, 0, 0, 0))),
        Value::Null
    );
    assert_eq!(
        must(map_cell(&stamp, MysqlValue::Date(0, 0, 0, 0, 0, 0, 0))),
        Value::Null
    );
    assert_eq!(
        must(map_cell(&date, MysqlValue::Bytes(b"0000-00-00".to_vec()))),
        Value::Null
    );
    assert_eq!(
        must(map_cell(&stamp, MysqlValue::Bytes(b"0000-00-00 00:00:00".to_vec()))),
        Value::Null
    );
    assert_eq!(
        must(map_cell(&stamp, MysqlValue::Bytes(b"2020-01-02 03:04:05".to_vec()))),
        basic_convert(&Value::String("2020-01-02 03:04:05".into()), "Timestamp").value
    );
}

#[test]
fn the_statement_timeout_suits_the_server_and_never_lifts_by_rounding() {
    use std::time::Duration;

    assert_eq!(
        must(timeout_sql(false, Some(Duration::from_millis(2500)))),
        "SET SESSION max_execution_time = 2500"
    );
    assert_eq!(
        must(timeout_sql(true, Some(Duration::from_millis(2500)))),
        "SET SESSION max_statement_time = 2.500"
    );
    assert_eq!(
        must(timeout_sql(false, Some(Duration::from_micros(300)))),
        "SET SESSION max_execution_time = 1"
    );
    assert_eq!(
        must(timeout_sql(true, Some(Duration::from_micros(300)))),
        "SET SESSION max_statement_time = 0.001"
    );
    assert!(timeout_sql(false, Some(Duration::ZERO)).is_err());
    assert!(timeout_sql(true, Some(Duration::ZERO)).is_err());
    assert_eq!(must(timeout_sql(false, None)), "SET SESSION max_execution_time = 0");
    assert_eq!(must(timeout_sql(true, None)), "SET SESSION max_statement_time = 0.000");
    assert!(is_mariadb("10.11.6-MariaDB-0+deb12u1"));
    assert!(!is_mariadb("8.0.36"));
}

fn parse_test_spec(spec: &str) -> Option<(&str, &str, &str, u16, &str)> {
    let spec = spec.trim();
    let (userinfo, rest) = spec.rsplit_once('@')?;
    let (username, password) = userinfo.split_once(':')?;
    let (hostport, database) = rest.split_once('/')?;
    let (host, port) = hostport.rsplit_once(':')?;
    let port = port.parse().ok()?;
    if host.is_empty() || database.is_empty() {
        return None;
    }
    Some((username, password, host, port, database))
}

fn test_driver() -> Option<MysqlDriver> {
    if let Ok(spec) = std::env::var("MOQUI_TEST_MYSQL") {
        let (username, password, host, port, database) =
            parse_test_spec(&spec).expect("MOQUI_TEST_MYSQL must be user:pass@host:port/database");
        let driver = MysqlDriver::new(host, port, username, password, database, false);
        return driver.connect_mysql().ok().map(|_| driver);
    }
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], 3306));
    if std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(1)).is_err() {
        return None;
    }
    let driver = MysqlDriver::new("127.0.0.1", 3306, "root", "", "mysql", false);
    driver.connect_mysql().ok().map(|_| driver)
}

#[test]
fn parses_mysql_test_target() {
    assert_eq!(
        parse_test_spec("root:secret@127.0.0.1:3307/app"),
        Some(("root", "secret", "127.0.0.1", 3307, "app"))
    );
    assert_eq!(
        parse_test_spec("root:@127.0.0.1:3306/mysql"),
        Some(("root", "", "127.0.0.1", 3306, "mysql"))
    );
    assert!(parse_test_spec("not a target").is_none());
}

/// The server bounds only SELECT by `max_execution_time` (MariaDB bounds every statement by `max_statement_time`);
/// a statement of another kind that runs past the transaction's deadline is ended by the deadline timer's
/// `KILL QUERY`, so neither server keeps a timed-out write running.
#[test]
fn a_statement_past_the_transaction_deadline_is_killed() {
    let Some(driver) = test_driver() else {
        tracing::debug!("MySQL was not reachable");
        return;
    };
    let manager = must(moqui_core::transaction::TransactionManager::new(
        Arc::new(driver),
        moqui_core::transaction::PoolConfig {
            min: 0,
            max: 2,
            reserve: 1,
            acquire_timeout: std::time::Duration::from_secs(10),
            ..Default::default()
        },
    ));
    let started = std::time::Instant::now();
    let outcome = manager.run(
        moqui_core::transaction::TransactionAttribute::ForceNew,
        Some(std::time::Duration::from_secs(1)),
        || manager.with_connection(|conn| conn.execute("DO SLEEP(6)", &[])),
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(4),
        "ran {elapsed:?}: {outcome:?}"
    );
    // MySQL ends a killed DO SLEEP early with a result of 1 rather than an error; the frame is rollback-only
    // either way, so the run fails.
    assert!(outcome.is_err(), "{outcome:?}");
}

#[test]
fn round_trip_stream_and_statement_cache() {
    let Some(driver) = test_driver() else {
        tracing::debug!("MySQL was not reachable");
        return;
    };
    let mut conn = must(driver.connect_mysql());
    must(conn.execute(
        "CREATE TEMPORARY TABLE moqui_mysql_roundtrip (
                    id INT NOT NULL PRIMARY KEY,
                    name VARCHAR(64) NULL
                )",
        &[],
    ));
    let insert = "INSERT INTO moqui_mysql_roundtrip (id, name) VALUES (?, ?)";
    assert_eq!(must(conn.execute(insert, &[Value::I32(1), Value::string("one")])), 1);
    assert_eq!(must(conn.execute(insert, &[Value::I32(2), Value::string("two")])), 1);
    let select = "SELECT id, name FROM moqui_mysql_roundtrip ORDER BY id";
    let rows = must(conn.query(select, &[]));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get("id"), Some(&Value::I32(1)));
    assert_eq!(rows[0].get("name"), Some(&Value::string("one")));
    assert_eq!(rows[1].get("id"), Some(&Value::I32(2)));
    assert_eq!(rows[1].get("name"), Some(&Value::string("two")));

    must(conn.open_stream(select, &[]));
    assert!(conn.stream_open());
    assert!(conn.execute("SELECT 1", &[]).is_err());
    let first = must(conn.next_row()).expect("first streamed row");
    let second = must(conn.next_row()).expect("second streamed row");
    assert!(must(conn.next_row()).is_none());
    must(conn.close_stream());
    assert!(!conn.stream_open());
    assert_eq!(first.get("id"), Some(&Value::I32(1)));
    assert_eq!(second.get("name"), Some(&Value::string("two")));

    let mut cached = must(driver.connect_mysql());
    let probe = "SELECT ? AS id";
    let first = must(cached.query(probe, &[Value::I32(1)]));
    let second = must(cached.query(probe, &[Value::I32(2)]));
    assert_eq!(cached.statement_cache_len(), 1);
    assert!(matches!(first[0].columns[0].1, Value::I32(1) | Value::I64(1)));
    assert!(matches!(second[0].columns[0].1, Value::I32(2) | Value::I64(2)));
    let len = cached.statement_cache_len();
    let once = must(cached.query_once("SELECT ? AS extra", &[Value::string("z")]));
    assert_eq!(cached.statement_cache_len(), len);
    assert_eq!(once[0].columns[0].1, Value::string("z"));

    let mut boxed = must(DbDriver::connect(&driver));
    must(boxed.query_once("SELECT 1", &[]));
}
