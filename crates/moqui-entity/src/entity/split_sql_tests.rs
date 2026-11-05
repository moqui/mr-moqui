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

use super::split_sql;

#[test]
fn splits_at_semicolons_outside_quotes() {
    assert_eq!(
        split_sql("select 'a;b'; select 'it''s;'; ; select \"x;y\" from `t;u`"),
        vec!["select 'a;b'", "select 'it''s;'", "select \"x;y\" from `t;u`"]
    );
    assert_eq!(split_sql("  "), Vec::<&str>::new());
    assert_eq!(split_sql("select 1"), vec!["select 1"]);
}

#[test]
fn a_dollar_quoted_body_is_one_statement() {
    let script = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql;\n\
                  DO $body$ BEGIN PERFORM 1; PERFORM '$$'; END $body$;\n\
                  select 2";
    assert_eq!(
        split_sql(script),
        vec![
            "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql",
            "DO $body$ BEGIN PERFORM 1; PERFORM '$$'; END $body$",
            "select 2",
        ]
    );
    // `$1` is a parameter and `a$b` an identifier, neither the start of a body.
    assert_eq!(
        split_sql("select $1; select a$b$ from t; select 3"),
        vec!["select $1", "select a$b$ from t", "select 3"]
    );
    // An unclosed body runs to the end.
    assert_eq!(split_sql("select $q$ a; b"), vec!["select $q$ a; b"]);
}

#[test]
fn comments_hide_semicolons_and_alone_are_no_statement() {
    let script = "-- first; still a comment\nselect 1; /* a; /* nested; */ still; */ select 2;\n-- trailing; comment\n";
    assert_eq!(
        split_sql(script),
        vec![
            "-- first; still a comment\nselect 1",
            "/* a; /* nested; */ still; */ select 2"
        ]
    );
    assert_eq!(split_sql("select 1; -- done"), vec!["select 1"]);
    assert_eq!(split_sql("/* only */ ; -- only"), Vec::<&str>::new());
    // `-` and `/` alone are operators.
    assert_eq!(split_sql("select 4-1; select 4/2"), vec!["select 4-1", "select 4/2"]);
}

#[test]
fn escape_strings_take_backslashes() {
    assert_eq!(
        split_sql(r"select E'a\';b'; select 'c\'; select 1"),
        vec![r"select E'a\';b'", r"select 'c\'", "select 1"]
    );
    // Text and quoted names keep other scripts' characters whole.
    assert_eq!(split_sql("select 'é;ü'; select 2"), vec!["select 'é;ü'", "select 2"]);
}
