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

use super::{concurrent_index_name, lock_timed_out, session_settings, takes_strong_lock};
use crate::group_db::Server;
use crate::schema::online_create_index;

/// DDL and the rewrites take the job's lock timeout; the online forms, VALIDATE and plain maintenance do not.
#[test]
fn strong_locks_are_told_from_light_ones() {
    for sql in [
        "ALTER TABLE \"t\" ADD CONSTRAINT \"fk\" FOREIGN KEY (\"a\") REFERENCES \"u\" (\"a\") NOT VALID",
        "ALTER TABLE t DROP CONSTRAINT IF EXISTS fk",
        "CREATE INDEX i ON t (a)",
        "VACUUM (FULL, ANALYZE) t",
        "REINDEX TABLE t",
        "CLUSTER t",
        "OPTIMIZE TABLE t",
    ] {
        assert!(takes_strong_lock(sql), "{sql}");
    }
    for sql in [
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS \"i\" ON \"t\" (\"a\")",
        "DROP INDEX CONCURRENTLY IF EXISTS \"i\"",
        "ALTER TABLE \"t\" VALIDATE CONSTRAINT \"fk\"",
        "REINDEX TABLE CONCURRENTLY t",
        "VACUUM (ANALYZE) t",
        "ANALYZE t",
        // the word in a name or text does not count
        "ANALYZE \"vacuum full\"",
    ] {
        assert!(!takes_strong_lock(sql), "{sql}");
    }
}

#[test]
fn the_index_a_concurrent_build_makes_is_read_from_its_statement() {
    let online = online_create_index(
        "CREATE INDEX \"ENUM_CODE_IDX\" ON \"enumeration\" (\"enum_code\")",
        Server::Postgres,
    );
    assert_eq!(
        online,
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS \"ENUM_CODE_IDX\" ON \"enumeration\" (\"enum_code\")"
    );
    assert_eq!(concurrent_index_name(&online).as_deref(), Some("ENUM_CODE_IDX"));
    assert_eq!(
        concurrent_index_name("create unique index concurrently if not exists Some_Idx on t (a)").as_deref(),
        Some("some_idx")
    );
    assert_eq!(concurrent_index_name("CREATE INDEX i ON t (a)"), None);
    assert_eq!(
        online_create_index("CREATE UNIQUE INDEX `u` ON `t` (`a`)", Server::Mysql),
        "CREATE UNIQUE INDEX `u` ON `t` (`a`) ALGORITHM=INPLACE LOCK=NONE"
    );
}

#[test]
fn sessions_get_no_statement_timeout_and_the_lock_timeout_only_when_asked() {
    let (setup, reset) = session_settings(Server::Postgres, Some(5000));
    assert_eq!(setup, ["SET statement_timeout = 0", "SET lock_timeout = 5000"]);
    assert_eq!(reset, ["RESET statement_timeout", "RESET lock_timeout"]);
    let (setup, _) = session_settings(Server::Postgres, None);
    assert_eq!(setup[1], "SET lock_timeout = 0");
    let (setup, _) = session_settings(Server::Mariadb, Some(1500));
    assert_eq!(
        setup,
        [
            "SET SESSION max_statement_time = 0",
            "SET SESSION lock_wait_timeout = 2"
        ]
    );
    assert!(lock_timed_out(
        "postgres execute: canceling statement due to lock timeout"
    ));
    assert!(lock_timed_out(
        "ERROR 1205 (HY000): Lock wait timeout exceeded; try restarting transaction"
    ));
    assert!(!lock_timed_out("canceling statement due to user request"));
}
