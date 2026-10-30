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

fn memory() -> MemDriver {
    MemDriver {
        store: Arc::new(Mutex::new(BTreeMap::new())),
        tape: None,
    }
}

fn cell(row: &DbRow, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// Two open transactions each change a different row. The later commit has to keep the earlier one.
#[test]
fn a_later_commit_keeps_the_other_transactions_row() {
    let driver = memory();
    let mut seed = driver.connect().unwrap();
    seed.execute(
        "INSERT INTO \"message\" (\"id\", \"status\") VALUES (?, ?)",
        &[Value::string("out"), Value::string("sending")],
    )
    .unwrap();
    seed.execute(
        "INSERT INTO \"message\" (\"id\", \"status\") VALUES (?, ?)",
        &[Value::string("in"), Value::string("received")],
    )
    .unwrap();
    drop(seed);

    let mut sent = driver.connect().unwrap();
    let mut consuming = driver.connect().unwrap();
    sent.begin().unwrap();
    consuming.begin().unwrap();
    assert_eq!(
        sent.execute(
            "UPDATE \"message\" SET \"status\" = ? WHERE \"id\" = ?",
            &[Value::string("sent"), Value::string("out")],
        )
        .unwrap(),
        1
    );
    assert_eq!(
        consuming
            .execute(
                "UPDATE \"message\" SET \"status\" = ? WHERE \"id\" = ?",
                &[Value::string("consuming"), Value::string("in")],
            )
            .unwrap(),
        1
    );
    // The consume commits second. Its snapshot still has the outgoing row as sending.
    sent.commit().unwrap();
    consuming.commit().unwrap();

    let mut check = driver.connect().unwrap();
    let rows = check.query("SELECT \"id\", \"status\" FROM \"message\"", &[]).unwrap();
    let mut found: Vec<(String, String)> = rows.iter().map(|row| (cell(row, "id"), cell(row, "status"))).collect();
    found.sort();
    assert_eq!(
        found,
        vec![
            ("in".to_string(), "consuming".to_string()),
            ("out".to_string(), "sent".to_string()),
        ]
    );
}

/// Both transactions updated the one row from the same snapshot. The later commit writes its columns onto that row.
#[test]
fn a_later_update_of_the_same_row_overlays_the_columns_it_changed() {
    let driver = memory();
    let mut seed = driver.connect().unwrap();
    seed.execute(
        "INSERT INTO \"message\" (\"id\", \"status\") VALUES (?, ?)",
        &[Value::string("out"), Value::string("produced")],
    )
    .unwrap();
    drop(seed);

    let mut sending = driver.connect().unwrap();
    let mut error = driver.connect().unwrap();
    sending.begin().unwrap();
    error.begin().unwrap();
    sending
        .execute(
            "UPDATE \"message\" SET \"status\" = ? WHERE \"id\" = ?",
            &[Value::string("sending"), Value::string("out")],
        )
        .unwrap();
    error
        .execute(
            "UPDATE \"message\" SET \"status\" = ? WHERE \"id\" = ?",
            &[Value::string("error"), Value::string("out")],
        )
        .unwrap();
    sending.commit().unwrap();
    error.commit().unwrap();

    let mut check = driver.connect().unwrap();
    let rows = check.query("SELECT \"id\", \"status\" FROM \"message\"", &[]).unwrap();
    let found: Vec<String> = rows.iter().map(|row| cell(row, "status")).collect();
    assert_eq!(found, vec!["error".to_string()]);
}

/// Two refills of one sequence name stay one row, and the higher number wins.
#[test]
fn concurrent_sequence_refills_keep_one_row_and_the_higher_number() {
    let driver = memory();
    let mut seed = driver.connect().unwrap();
    seed.execute(
        "INSERT INTO \"sequence_value_item\" (\"seq_name\", \"seq_num\") VALUES (?, ?)",
        &[Value::string("order"), Value::I64(100)],
    )
    .unwrap();
    drop(seed);

    let mut low = driver.connect().unwrap();
    let mut high = driver.connect().unwrap();
    low.begin().unwrap();
    high.begin().unwrap();
    low.execute(
        "UPDATE \"sequence_value_item\" SET \"seq_num\" = ? WHERE \"seq_name\" = ?",
        &[Value::I64(150), Value::string("order")],
    )
    .unwrap();
    high.execute(
        "UPDATE \"sequence_value_item\" SET \"seq_num\" = ? WHERE \"seq_name\" = ?",
        &[Value::I64(250), Value::string("order")],
    )
    .unwrap();
    low.commit().unwrap();
    high.commit().unwrap();

    let mut check = driver.connect().unwrap();
    let rows = check
        .query("SELECT \"seq_name\", \"seq_num\" FROM \"sequence_value_item\"", &[])
        .unwrap();
    let found: Vec<String> = rows.iter().map(|row| cell(row, "seq_num")).collect();
    assert_eq!(found, vec!["250".to_string()]);
}

/// The delete started from the row as it was, then another transaction changed that row and committed.
/// The delete still removes it.
#[test]
fn a_delete_removes_the_row_a_committed_update_had_changed() {
    let driver = memory();
    let mut seed = driver.connect().unwrap();
    seed.execute(
        "INSERT INTO \"message\" (\"id\", \"status\") VALUES (?, ?)",
        &[Value::string("srv"), Value::string("token")],
    )
    .unwrap();
    drop(seed);

    let mut delete = driver.connect().unwrap();
    delete.begin().unwrap();
    let mut revise = driver.connect().unwrap();
    revise.begin().unwrap();
    revise
        .execute(
            "UPDATE \"message\" SET \"status\" = ? WHERE \"id\" = ?",
            &[Value::string("cleared"), Value::string("srv")],
        )
        .unwrap();
    revise.commit().unwrap();
    assert_eq!(
        delete
            .execute("DELETE FROM \"message\" WHERE \"id\" = ?", &[Value::string("srv")])
            .unwrap(),
        1
    );
    delete.commit().unwrap();

    let mut check = driver.connect().unwrap();
    let rows = check.query("SELECT \"id\", \"status\" FROM \"message\"", &[]).unwrap();
    assert!(rows.is_empty());
}
