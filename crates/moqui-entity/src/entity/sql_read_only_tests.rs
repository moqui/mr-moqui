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

use super::refusal;
use crate::entity::exec::SqlSyntax;

/// A read-only run takes a query, with a refused name only in its text or comments; it refuses a statement that is
/// not a query and each way of writing a refused call (SEC-TLS-86).
#[test]
fn a_read_only_run_takes_only_a_query_without_the_calls_past_the_tables() {
    let pg = SqlSyntax::Postgres;
    for sql in [
        "select 1",
        "with t as (select 1 as a) select a from t",
        "select 'pg_read_file(''x'')' as said -- pg_read_file('y')",
        "select test_id from test_entity /* set_config */",
        "explain select * from test_entity",
        "show search_path",
        "select pg_size_pretty(pg_database_size(current_database()))",
        "select \"Test_Id\" from test_entity",
        "-- the open ones\nselect test_id from test_entity",
        "/* a note */ select 1",
        "(select 1) union (select 2)",
    ] {
        assert_eq!(refusal(sql, pg), None, "{sql}");
    }
    for sql in [
        "copy (select 1) to program 'id'",
        "copy test_entity to '/tmp/out'",
        "do $$ begin perform pg_read_file('/etc/passwd'); end $$",
        "call nothing()",
        "set role postgres",
        "listen moqui_cluster",
        "select pg_read_file('/etc/passwd')",
        "SELECT PG_CATALOG.PG_READ_FILE('/etc/passwd')",
        "select \"pg_read_file\"('/etc/passwd')",
        "select U&\"pg\\005fread_file\"('/etc/passwd')",
        "select pg_ls_dir('.')",
        "select lo_export(1, '/tmp/x')",
        "select pg_terminate_backend(1)",
        "select pg_notify('moqui_cluster', 'x')",
        "select set_config('session_authorization', 'postgres', true)",
        "select query_to_xml('select 1', true, false, '')",
        "select * from ts_stat('select 1')",
        "select pg_advisory_lock(1)",
        "explain analyze select pg_read_binary_file('PG_VERSION')",
    ] {
        assert!(refusal(sql, pg).is_some(), "{sql}");
    }
    let mysql = SqlSyntax::Mysql;
    assert_eq!(refusal("SELECT enum_id INTO @id FROM ENUMERATION", mysql), None);
    assert_eq!(refusal("# a note\nSELECT 'the /* notes */' AS said", mysql), None);
    for sql in [
        "SELECT * FROM ENUMERATION INTO OUTFILE '/tmp/out'",
        "SELECT 1 INTO DUMPFILE '/tmp/out'",
        "SELECT LOAD_FILE('/etc/passwd')",
        "SELECT `load_file`('/etc/passwd')",
        "SELECT 1 /*!INTO OUTFILE '/tmp/x'*/",
        "SELECT 1 /*!50110 INTO DUMPFILE '/tmp/x'*/",
        "SELECT 1 /*M! INTO OUTFILE '/tmp/x'*/",
        // under NO_BACKSLASH_ESCAPES the server ends the string at its second quote and runs what follows
        "SELECT '\\' /*!INTO OUTFILE '/tmp/x' */ -- '",
        "SELECT '\\' INTO OUTFILE '/tmp/x' -- '",
        // a string that holds one is refused too
        "SELECT '/*!' AS said",
        "SELECT 'load_file' AS said",
        "CALL nothing()",
    ] {
        assert!(refusal(sql, mysql).is_some(), "{sql}");
    }
}
