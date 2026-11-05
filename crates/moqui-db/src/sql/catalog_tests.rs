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

use super::{normalize_sql_type, pg_relations_of_oids, same_sql_type};

/// The conf's types match what each database reports for the column it made of them.
#[test]
fn conf_types_match_the_catalog_spelling() {
    for (expected, actual) in [
        ("VARCHAR(40)", "character varying(40)"),
        ("CHAR(1)", "character(1)"),
        ("TIMESTAMP", "timestamp without time zone"),
        ("TIME", "time without time zone"),
        ("NUMERIC(26,6)", "numeric(26,6)"),
        ("NUMERIC(26, 6)", "numeric(26,6)"),
        ("FLOAT8", "double precision"),
        ("TEXT", "text"),
        ("BYTEA", "bytea"),
        ("DATE", "date"),
        // MySQL's COLUMN_TYPE
        ("DECIMAL(26,6)", "decimal(26,6)"),
        ("DATETIME(3)", "datetime(3)"),
        ("DOUBLE", "double"),
        ("LONGTEXT", "longtext"),
        ("BIGINT", "bigint(20)"),
        ("INTEGER", "int(11)"),
        ("INT UNSIGNED", "int(10) unsigned"),
    ] {
        assert!(same_sql_type(expected, actual), "{expected} against {actual}");
    }
    for (expected, actual) in [
        ("VARCHAR(40)", "character varying(255)"),
        ("NUMERIC(26,6)", "numeric(24,4)"),
        ("TIMESTAMP", "timestamp with time zone"),
        ("TEXT", "character varying(4095)"),
        ("CHAR(1)", "varchar(1)"),
    ] {
        assert!(!same_sql_type(expected, actual), "{expected} against {actual}");
    }
    assert_eq!(normalize_sql_type("timestamp(3) with time zone"), "timestamptz(3)");
}

#[test]
fn oid_lookup_lists_each_oid_once() {
    assert_eq!(pg_relations_of_oids(&[]), None);
    let sql = pg_relations_of_oids(&[42, 7, 42]).unwrap();
    assert!(sql.ends_with("WHERE c.oid IN (7, 42)"), "{sql}");
}
