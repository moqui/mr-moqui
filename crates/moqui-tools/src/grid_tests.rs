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

use moqui_lib::object::Value;

use super::{
    csv_cell_text, escape_like, export_location_allowed, parse_filter, parse_sort, stamp_matches,
    statement_shape_refusal, wrap, Filter, Sort,
};
use crate::audit::Actor;

fn filter(field: &str, kind: &str, value: Value) -> Filter {
    Filter {
        field: field.to_string(),
        kind: kind.to_string(),
        value,
    }
}

#[test]
fn a_sql_source_is_wrapped_with_bound_filters_and_quoted_names() {
    let filters = vec![
        filter("description", "like", Value::string("50%_x")),
        filter("n", ">=", Value::I64(3)),
        filter("id", "in", Value::string("a, b")),
        filter("none", "in", Value::List(Vec::new())),
        filter("note", "empty", Value::Null),
        filter("code", "!empty", Value::Null),
    ];
    let sort = vec![Sort {
        field: "Total \"Due\"".to_string(),
        descending: true,
    }];
    let wrapped = wrap("SELECT * FROM t", false, &sort, &filters, " LIMIT 11 OFFSET 0").unwrap();
    assert_eq!(
        wrapped.sql,
        "SELECT * FROM (SELECT * FROM t) q WHERE LOWER(CAST(\"description\" AS TEXT)) LIKE LOWER(?) AND \"n\" >= ? \
         AND \"id\" IN (?, ?) AND 1 = 0 AND (\"note\" IS NULL OR CAST(\"note\" AS TEXT) = '') \
         AND NOT (\"code\" IS NULL OR CAST(\"code\" AS TEXT) = '') ORDER BY \"Total \"\"Due\"\"\" DESC LIMIT 11 OFFSET 0"
    );
    assert_eq!(wrapped.params[0], Value::string("%50\\%\\_x%"));
    assert!(
        wrap("SELECT ? AS a", false, &[], &filters, "").is_err(),
        "a ? of its own beside filters"
    );
    assert_eq!(escape_like("a\\b"), "a\\\\b");
}

#[test]
fn a_result_of_one_table_named_once_is_the_only_shape_saved() {
    let sql = "SELECT enum_id, description FROM enumeration WHERE enum_type_id = 'x'";
    assert_eq!(statement_shape_refusal(sql, false, "ENUMERATION"), None);
    for sql in [
        "SELECT a.enum_id FROM enumeration a JOIN enumeration b ON b.enum_id = a.enum_id",
        "SELECT a.enum_id FROM enumeration a, enumeration_type t",
        "SELECT DISTINCT enum_id FROM enumeration",
        "SELECT enum_id FROM enumeration GROUP BY enum_id",
        "SELECT enum_id FROM enumeration UNION SELECT enum_id FROM enumeration",
        "SELECT enum_id FROM enumeration WHERE enum_id IN (SELECT enum_id FROM enumeration)",
    ] {
        assert!(statement_shape_refusal(sql, false, "enumeration").is_some(), "{sql}");
    }
    // a table name in a quoted string does not count
    assert_eq!(
        statement_shape_refusal(
            "SELECT enum_id FROM enumeration WHERE description = 'enumeration'",
            false,
            "enumeration"
        ),
        None
    );
}

/// The CSV export is for a person: a text cell a spreadsheet would run as a formula is guarded, a number is not
/// (SEC-SCR-89).
#[test]
fn a_csv_export_cell_is_guarded_against_formulas() {
    assert_eq!(
        csv_cell_text(&Value::string("=1+cmd|' /C calc'!A0")),
        "'=1+cmd|' /C calc'!A0"
    );
    assert_eq!(csv_cell_text(&Value::string("@SUM(A1)")), "'@SUM(A1)");
    assert_eq!(csv_cell_text(&Value::I64(-12)), "-12");
    assert_eq!(csv_cell_text(&Value::string("plain")), "plain");
    assert_eq!(csv_cell_text(&Value::Null), "");
}

#[test]
fn exports_go_to_the_users_own_places_unless_an_admin_says_otherwise() {
    let ana = Actor {
        user_id: Some("ANA".to_string()),
        ..Actor::default()
    };
    assert!(export_location_allowed(
        "runtime://tmp/grid-export/ANA/x/a.csv",
        &ana,
        false
    ));
    assert!(export_location_allowed(
        "dbresource://moqui/database/export/ANA/a.csv",
        &ana,
        false
    ));
    assert!(!export_location_allowed(
        "runtime://tmp/grid-export/BO/x/a.csv",
        &ana,
        false
    ));
    assert!(!export_location_allowed(
        "runtime://tmp/grid-export/ANA/../BO/a.csv",
        &ana,
        false
    ));
    assert!(!export_location_allowed("dbresource://other/a.csv", &ana, false));
    assert!(!export_location_allowed(
        "component://framework/data/a.csv",
        &ana,
        false
    ));
    assert!(export_location_allowed("dbresource://other/a.csv", &ana, true));
    assert!(export_location_allowed("runtime://export/a.csv", &ana, true));
    assert!(export_location_allowed(
        "runtime://tmp/grid-export/ANA/x/a.csv",
        &ana,
        true
    ));
    for refused in [
        "component://framework/data/a.csv",
        "file:/etc/a.csv",
        "runtime://conf/a.csv",
        "runtime://tmp/other/a.csv",
        "runtime://tmp/grid-export/BO/x/a.csv",
    ] {
        assert!(!export_location_allowed(refused, &ana, true), "{refused}");
    }
}

#[test]
fn tabulator_sorts_filters_and_stamps_are_read() {
    let mut sort = indexmap::IndexMap::default();
    sort.insert("field".to_string(), Value::string("a"));
    sort.insert("dir".to_string(), Value::string("desc"));
    assert_eq!(
        parse_sort(Some(&Value::List(vec![Value::Map(sort)]))),
        vec![Sort {
            field: "a".to_string(),
            descending: true
        }]
    );
    let mut map = indexmap::IndexMap::default();
    map.insert("field".to_string(), Value::string("b"));
    map.insert("value".to_string(), Value::I64(1));
    assert_eq!(parse_filter(Some(&Value::List(vec![Value::Map(map)])))[0].kind, "=");
    let stored = "2026-11-05T10:00:00.123456Z".parse::<jiff::Timestamp>().unwrap();
    assert!(stamp_matches(
        Some(&Value::Timestamp(stored)),
        "2026-11-05T10:00:00.123456Z"
    ));
    assert!(!stamp_matches(Some(&Value::Timestamp(stored)), "2026-11-05T10:00:00Z"));
}
