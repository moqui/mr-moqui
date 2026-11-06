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

use std::time::Duration;

use moqui_lib::object::Value;

use super::{list, record, statement_key, MAX_ARTIFACTS};

#[test]
fn literal_sql_of_one_shape_is_one_key() {
    assert_eq!(
        statement_key("SELECT *  FROM t\n WHERE id = 5 AND name = 'a''b' -- note\n"),
        "SELECT * FROM t WHERE id = ? AND name = ?"
    );
    assert_eq!(
        statement_key("SELECT * FROM t WHERE id = 6 AND name = 'c'"),
        statement_key("SELECT * FROM t WHERE id = 5 AND name = 'a''b'")
    );
    // placeholders and identifiers with digits are kept
    assert_eq!(
        statement_key("SELECT col1 FROM t2 WHERE a = $1"),
        "SELECT col1 FROM t2 WHERE a = $1"
    );
}

#[test]
fn a_statement_counts_hits_errors_and_times() {
    let sql = "SELECT stats_probe_one FROM stats_probe WHERE n = 41";
    record(sql, Duration::from_micros(100), false);
    record(
        "SELECT stats_probe_one FROM stats_probe WHERE n = 42",
        Duration::from_micros(300),
        true,
    );
    let rows = list("-hitCount", "", "stats_probe_one");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(
        row.get("sql"),
        Some(&Value::string("SELECT stats_probe_one FROM stats_probe WHERE n = ?"))
    );
    assert_eq!(row.get("hitCount"), Some(&Value::I64(2)));
    assert_eq!(row.get("errorCount"), Some(&Value::I64(1)));
    assert_eq!(row.get("minTime"), Some(&Value::F64(100.0)));
    assert_eq!(row.get("maxTime"), Some(&Value::F64(300.0)));
    assert_eq!(row.get("average"), Some(&Value::F64(200.0)));
    let Some(Value::F64(std_dev)) = row.get("stdDev") else {
        panic!("no stdDev in {row:?}");
    };
    assert!((std_dev - 141.42).abs() < 0.01, "{std_dev}");
}

#[test]
fn rows_sort_by_number_and_filter_by_text() {
    for _ in 0..10 {
        record("SELECT stats_sort_many FROM x", Duration::from_micros(5), false);
    }
    for _ in 0..9 {
        record("SELECT stats_sort_few FROM x", Duration::from_micros(5), false);
    }
    let names: Vec<String> = list("-hitCount", "", "STATS_SORT_")
        .iter()
        .map(|row| moqui_lib::object::to_plain_string(row.get("sql").unwrap()))
        .collect();
    // 10 before 9: a text sort would put "9" first
    assert_eq!(names, ["SELECT stats_sort_many FROM x", "SELECT stats_sort_few FROM x"]);
}

#[test]
fn the_artifacts_of_a_statement_are_bounded() {
    let mut stat = super::Stat::default();
    for index in 0..(MAX_ARTIFACTS + 5) {
        super::count(&mut stat, 1000, false, Some(format!("screen{index}")));
    }
    assert_eq!(stat.artifacts.len(), MAX_ARTIFACTS + 1);
    assert_eq!(stat.artifacts.get(super::OTHER), Some(&5));
}

/// Past the bound a new statement counts in the "(other)" row; clearing frees the rows again.
#[test]
fn statements_past_the_bound_count_in_the_other_row() {
    let registry = super::Registry::new(3);
    let at = crate::hooks::StatementArtifact::default;
    for name in ["a", "b", "c", "d", "e"] {
        registry.record(
            &format!("SELECT bound_{name} FROM x"),
            Duration::from_micros(1),
            false,
            at(),
        );
    }
    let rows = registry.list("sql", "", "");
    assert_eq!(rows.len(), 4, "{rows:?}");
    let other = rows
        .iter()
        .find(|row| row.get("sql") == Some(&Value::string(super::OTHER)))
        .expect("the other row");
    assert_eq!(other.get("hitCount"), Some(&Value::I64(2)));
    // a statement already kept still counts in its own row
    registry.record("SELECT bound_a FROM x", Duration::from_micros(1), false, at());
    assert_eq!(
        registry.list("", "", "bound_a")[0].get("hitCount"),
        Some(&Value::I64(2))
    );
    registry.clear();
    assert!(registry.list("", "", "").is_empty());
    for name in ["x", "y", "z"] {
        registry.record(
            &format!("SELECT bound_{name} FROM x"),
            Duration::from_micros(1),
            false,
            at(),
        );
    }
    assert_eq!(registry.list("", "", "").len(), 3, "clearing freed every row");
}

/// Two long statements that differ only past the kept prefix are two rows, each key bounded.
#[test]
fn long_statements_keep_a_bounded_key_with_a_hash_of_the_whole() {
    let columns = (0..2000)
        .map(|index| format!("c{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let first = statement_key(&format!("SELECT {columns} FROM a WHERE b = 'one' AND c_first = 1"));
    let second = statement_key(&format!("SELECT {columns} FROM a WHERE b = 'two' AND c_second = 2"));
    let same = statement_key(&format!("SELECT {columns} FROM a WHERE b = 'three' AND c_first = 3"));
    assert_ne!(first, second);
    assert_eq!(first, same);
    assert!(first.len() < super::KEY_TEXT_MAX + 64, "{}", first.len());
    assert!(first.starts_with("SELECT c0, c1, "));
}
