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

use super::{concurrent_index_name, is_light, lock_timed_out, seq_id, session_settings, takes_strong_lock};
use crate::group_db::Server;
use crate::schema::{online_create_index, Plan};

/// Only the known light forms, by their leading form, go without the job's lock timeout; every other statement takes
/// it, and a word inside a quoted name or a comment changes nothing.
#[test]
fn only_the_known_light_forms_go_without_a_lock_timeout() {
    for sql in [
        "CREATE INDEX CONCURRENTLY IF NOT EXISTS \"i\" ON \"t\" (\"a\")",
        "create unique index concurrently u on t (a)",
        "DROP INDEX CONCURRENTLY IF EXISTS \"i\"",
        "REINDEX TABLE CONCURRENTLY t",
        "REINDEX (VERBOSE) INDEX CONCURRENTLY \"i\"",
        "ALTER TABLE \"t\" VALIDATE CONSTRAINT \"fk\"",
        "ALTER TABLE ONLY s.t VALIDATE CONSTRAINT fk",
        "VACUUM (ANALYZE) t",
        "VACUUM ANALYZE t",
        "ANALYZE t",
        // the words are names here, inside quotes
        "VACUUM (ANALYZE) \"full\"",
        "ANALYZE \"vacuum full\"",
    ] {
        assert!(is_light(sql), "{sql}");
    }
    for sql in [
        "ALTER TABLE \"t\" ADD CONSTRAINT \"fk\" FOREIGN KEY (\"a\") REFERENCES \"u\" (\"a\") NOT VALID",
        "ALTER TABLE t VALIDATE CONSTRAINT fk, DROP COLUMN x",
        "ALTER TABLE t DROP CONSTRAINT IF EXISTS fk",
        "CREATE INDEX i ON t (a)",
        // CONCURRENTLY as a name, not the form
        "CREATE INDEX i ON \"concurrently\" (a)",
        "CREATE INDEX concurrently_i ON t (a)",
        "VACUUM (FULL, ANALYZE) t",
        "VACUUM FULL t",
        "REINDEX TABLE t",
        "CLUSTER t",
        "OPTIMIZE TABLE t",
        "TRUNCATE t",
        "COMMENT ON TABLE t IS 'x'",
        "/* VACUUM */ DELETE FROM t",
    ] {
        assert!(takes_strong_lock(sql), "{sql}");
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
    assert_eq!(
        session_settings(Server::Postgres, Some(5000)),
        ["SET statement_timeout = 0", "SET lock_timeout = 5000"]
    );
    assert_eq!(session_settings(Server::Postgres, None)[1], "SET lock_timeout = 0");
    assert_eq!(
        session_settings(Server::Mariadb, Some(1500)),
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

/// A plan of more than 999 statements keeps its order when its sequence ids are sorted as text, as the rows are, so
/// the VALIDATE of a large re-plan follows its key.
#[test]
fn sequence_ids_of_a_large_plan_sort_in_their_order() {
    let mut plan = Plan::default();
    let mut previous = None;
    for index in 0..2500 {
        let depends = (index % 2 == 1).then(|| previous.clone()).flatten();
        previous = Some(plan.add(format!("statement {index}"), depends, "e", "t", String::new()));
    }
    let mut ids: Vec<&str> = plan
        .statements
        .iter()
        .map(|statement| statement.statement_seq_id.as_str())
        .collect();
    let in_order = ids.clone();
    ids.sort_unstable();
    assert_eq!(ids, in_order);
    assert_eq!(seq_id(999), "001000");
    // each dependency comes before its dependent in that order
    for statement in &plan.statements {
        if let Some(depends) = &statement.depends_on_seq_id {
            assert!(depends.as_str() < statement.statement_seq_id.as_str());
        }
    }
}
