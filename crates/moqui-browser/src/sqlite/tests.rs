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

use std::str::FromStr;

use moqui_core::driver::DbConnection;
use moqui_lib::object::{basic_convert, Value};

use super::SqliteDriver;

fn one(conn: &mut dyn DbConnection, sql: &str) -> Value {
    let rows = conn.query(sql, &[]).unwrap();
    rows[0].columns[0].1.clone()
}

fn count(conn: &mut dyn DbConnection) -> i64 {
    match one(conn, "SELECT COUNT(*) FROM t") {
        Value::I64(n) => n,
        other => panic!("{other:?}"),
    }
}

fn setup() -> SqliteDriver {
    let driver = SqliteDriver::memory().unwrap();
    let mut conn = driver.connect_sqlite();
    conn.execute(
        "CREATE TABLE t (id TEXT PRIMARY KEY, n INTEGER, amount NUMERIC, stamp TEXT, day TEXT, at TEXT, data BLOB)",
        &[],
    )
    .unwrap();
    driver
}

#[test]
fn values_round_trip_through_the_entity_conversions() {
    let driver = setup();
    let mut conn = driver.connect_sqlite();
    let stamp = jiff::Timestamp::from_str("2026-10-14T12:34:56.789Z").unwrap();
    let date = jiff::civil::date(2026, 10, 14);
    let time = jiff::civil::time(8, 9, 10, 500_000_000);
    let amount = Value::Decimal(rust_decimal::Decimal::from_str("12.3400").unwrap());
    let changed = conn
        .execute(
            "INSERT INTO t (id, n, amount, stamp, day, at, data) VALUES (?, ?, ?, ?, ?, ?, ?)",
            &[
                Value::string("a"),
                Value::I32(7),
                amount,
                Value::Timestamp(stamp),
                Value::Date(date),
                Value::Time(time),
                Value::Bytes(vec![0, 1, 2]),
            ],
        )
        .unwrap();
    assert_eq!(changed, 1);
    let rows = conn
        .query(
            "SELECT n, amount, stamp, day, at, data FROM t WHERE id = ?",
            &[Value::string("a")],
        )
        .unwrap();
    let row = &rows[0];
    assert_eq!(row.get("n"), Some(&Value::I64(7)));
    assert_eq!(row.get("stamp"), Some(&Value::string("2026-10-14 12:34:56.789000")));
    assert_eq!(row.get("data"), Some(&Value::Bytes(vec![0, 1, 2])));
    let convert = |name: &str, java: &str| basic_convert(row.get(name).unwrap(), java).value;
    assert_eq!(convert("stamp", "Timestamp"), Value::Timestamp(stamp));
    assert_eq!(convert("day", "Date"), Value::Date(date));
    assert_eq!(convert("at", "Time"), Value::Time(time));
    // The value survives; the scale does not (NUMERIC affinity stores 12.34).
    let Value::Decimal(read) = convert("amount", "BigDecimal") else {
        panic!("amount is not a decimal");
    };
    assert_eq!(read.normalize(), rust_decimal::Decimal::from_str("12.34").unwrap());
    // Text in a fixed width compares in time order.
    let later = Value::Timestamp(stamp.checked_add(jiff::SignedDuration::from_millis(1)).unwrap());
    let rows = conn.query("SELECT id FROM t WHERE stamp < ?", &[later]).unwrap();
    assert_eq!(rows.len(), 1);
    // A decimal parameter compares as a number against a NUMERIC column.
    let rows = conn
        .query(
            "SELECT id FROM t WHERE amount > ?",
            &[Value::Decimal(rust_decimal::Decimal::from(9))],
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn a_reserve_transaction_nests_in_the_open_one_instead_of_waiting() {
    let driver = setup();
    let mut outer = driver.connect_sqlite();
    let mut reserve = driver.connect_sqlite();
    outer.begin().unwrap();
    outer.execute("INSERT INTO t (id) VALUES ('outer')", &[]).unwrap();
    reserve.begin().unwrap();
    reserve.execute("INSERT INTO t (id) VALUES ('reserve')", &[]).unwrap();
    reserve.commit().unwrap();
    outer.commit().unwrap();
    assert_eq!(count(&mut outer), 2);

    outer.begin().unwrap();
    reserve.begin().unwrap();
    reserve.execute("INSERT INTO t (id) VALUES ('kept')", &[]).unwrap();
    reserve.commit().unwrap();
    outer.execute("INSERT INTO t (id) VALUES ('dropped')", &[]).unwrap();
    reserve.begin().unwrap();
    reserve.execute("INSERT INTO t (id) VALUES ('undone')", &[]).unwrap();
    reserve.rollback().unwrap();
    outer.rollback().unwrap();
    // The nested commit was part of the outer transaction, so its row went with the rollback.
    assert_eq!(count(&mut outer), 2);
}

#[test]
fn savepoints_streams_and_dropped_connections() {
    let driver = setup();
    let mut conn = driver.connect_sqlite();
    conn.begin().unwrap();
    conn.execute("INSERT INTO t (id) VALUES ('1')", &[]).unwrap();
    conn.savepoint("sp1").unwrap();
    conn.execute("INSERT INTO t (id) VALUES ('2')", &[]).unwrap();
    conn.rollback_to_savepoint("sp1").unwrap();
    conn.release_savepoint("sp1").unwrap();
    conn.commit().unwrap();
    assert_eq!(count(&mut conn), 1);
    assert!(conn.savepoint("bad name").is_err());

    for index in 0..5 {
        conn.execute(
            "INSERT INTO t (id, n) VALUES (?, ?)",
            &[Value::string(format!("s{index}")), Value::I64(index)],
        )
        .unwrap();
    }
    conn.open_stream("SELECT n FROM t WHERE n IS NOT NULL ORDER BY n", &[])
        .unwrap();
    assert!(conn.stream_open());
    let mut seen = Vec::new();
    while let Some(row) = conn.next_row().unwrap() {
        seen.push(row.columns[0].1.clone());
    }
    conn.close_stream().unwrap();
    assert!(!conn.stream_open());
    assert_eq!(seen.len(), 5);

    {
        let mut abandoned = driver.connect_sqlite();
        abandoned.begin().unwrap();
        abandoned.execute("INSERT INTO t (id) VALUES ('lost')", &[]).unwrap();
    }
    assert_eq!(count(&mut conn), 6);
    assert_eq!(
        conn.query_once("SELECT 1 AS one", &[]).unwrap()[0].get("one"),
        Some(&Value::I64(1))
    );
    assert!(conn.canceller().cancel().is_ok());
    let returned = conn
        .execute("INSERT INTO t (id) VALUES ('r') RETURNING id", &[])
        .unwrap();
    assert_eq!(returned, 1);
}
