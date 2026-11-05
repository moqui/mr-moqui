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

use std::collections::BTreeSet;

use moqui_core::entity::{CatalogForeignKey, CatalogIndex, ForeignKeyDiff, IndexDiff, IndexKind, TableDiff};
use moqui_db::dialect::Dialects;
use moqui_lib::node::MNode;

use super::{plan_table, Plan};
use crate::group_db::Server;

fn dialect(name: &str) -> moqui_db::dialect::Dialect {
    let conf = MNode::parse_file(moqui_core::context::framework_dir().join("conf/MoquiDefaultConf.xml")).expect("conf");
    Dialects::load(&conf)
        .expect("dialects")
        .get(name)
        .expect("dialect")
        .clone()
}

fn table() -> TableDiff {
    TableDiff {
        entity_name: "moqui.basic.Enumeration".to_string(),
        group: "transactional".to_string(),
        table_name: "enumeration".to_string(),
        missing_indexes: vec![IndexDiff {
            name: "ENUM_CODE_IDX".to_string(),
            columns: vec!["enum_code".to_string()],
            unique: false,
            kind: IndexKind::Declared,
            ddl: vec!["CREATE INDEX \"ENUM_CODE_IDX\" ON \"enumeration\" (\"enum_code\")".to_string()],
        }],
        invalid_indexes: vec![CatalogIndex {
            name: "half_made".to_string(),
            columns: vec!["description".to_string()],
            ..CatalogIndex::default()
        }],
        extra_indexes: vec![CatalogIndex {
            name: "stray_idx".to_string(),
            valid: true,
            ..CatalogIndex::default()
        }],
        missing_foreign_keys: vec![ForeignKeyDiff {
            name: "ENUM_STATUS_FLOW".to_string(),
            related_entity: "moqui.basic.StatusFlow".to_string(),
            related_table: "status_flow".to_string(),
            columns: vec!["status_flow_id".to_string()],
            ddl: Some(
                "ALTER TABLE \"enumeration\" ADD CONSTRAINT \"ENUM_STATUS_FLOW\" FOREIGN KEY (\"status_flow_id\") \
                 REFERENCES \"status_flow\" (\"status_flow_id\")"
                    .to_string(),
            ),
        }],
        extra_foreign_keys: vec![CatalogForeignKey {
            name: "old_fk".to_string(),
            ..CatalogForeignKey::default()
        }],
        ..TableDiff::default()
    }
}

fn changes(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn texts(plan: &Plan) -> Vec<(String, Option<String>)> {
    plan.statements
        .iter()
        .map(|statement| (statement.statement_text.clone(), statement.depends_on_seq_id.clone()))
        .collect()
}

/// Postgres: concurrent index builds and drops, a key added NOT VALID then validated, an invalid index dropped and
/// made again from the model's definition.
#[test]
fn a_postgres_plan_is_in_the_online_forms() {
    let mut plan = Plan::default();
    let expected = |_: &str, index: &str| {
        (index == "half_made").then(|| "CREATE INDEX \"half_made\" ON \"enumeration\" (\"description\")".to_string())
    };
    plan_table(
        &mut plan,
        &table(),
        &dialect("postgres"),
        Server::Postgres,
        &changes(&[
            "missingIndexes",
            "invalidIndexes",
            "extraIndexes",
            "missingForeignKeys",
            "extraForeignKeys",
        ]),
        &BTreeSet::new(),
        &expected,
    );
    let some = |seq: &str| Some(seq.to_string());
    assert_eq!(
        texts(&plan),
        [
            (
                "CREATE INDEX CONCURRENTLY IF NOT EXISTS \"ENUM_CODE_IDX\" ON \"enumeration\" (\"enum_code\")"
                    .to_string(),
                None
            ),
            ("DROP INDEX CONCURRENTLY IF EXISTS \"half_made\"".to_string(), None),
            (
                "CREATE INDEX CONCURRENTLY IF NOT EXISTS \"half_made\" ON \"enumeration\" (\"description\")"
                    .to_string(),
                some("002")
            ),
            ("DROP INDEX CONCURRENTLY IF EXISTS \"stray_idx\"".to_string(), None),
            (
                "ALTER TABLE \"enumeration\" ADD CONSTRAINT \"ENUM_STATUS_FLOW\" FOREIGN KEY (\"status_flow_id\") \
                 REFERENCES \"status_flow\" (\"status_flow_id\") NOT VALID"
                    .to_string(),
                None
            ),
            (
                "ALTER TABLE \"enumeration\" VALIDATE CONSTRAINT \"ENUM_STATUS_FLOW\"".to_string(),
                some("005")
            ),
            (
                "ALTER TABLE \"enumeration\" DROP CONSTRAINT IF EXISTS \"old_fk\"".to_string(),
                None
            ),
        ]
    );
}

/// One row: only the named index; and an invalid index the model does not have is rebuilt as it is.
#[test]
fn a_plan_of_one_row_and_a_rebuild() {
    let mut plan = Plan::default();
    let none = |_: &str, _: &str| None;
    plan_table(
        &mut plan,
        &table(),
        &dialect("postgres"),
        Server::Postgres,
        &changes(&["missingIndexes", "invalidIndexes"]),
        &changes(&["half_made"]),
        &none,
    );
    assert_eq!(
        texts(&plan),
        [("REINDEX INDEX CONCURRENTLY \"half_made\"".to_string(), None)]
    );
}

/// MySQL: indexes in place without a lock, a key added as the builder writes it, with the warning that it copies the
/// table.
#[test]
fn a_mysql_plan_builds_in_place() {
    let mut plan = Plan::default();
    let none = |_: &str, _: &str| None;
    let mut diff = table();
    diff.invalid_indexes.clear();
    diff.missing_indexes[0].ddl = vec!["CREATE INDEX `ENUM_CODE_IDX` ON `ENUMERATION` (`ENUM_CODE`)".to_string()];
    diff.table_name = "ENUMERATION".to_string();
    plan_table(
        &mut plan,
        &diff,
        &dialect("mysql8"),
        Server::Mysql,
        &changes(&["missingIndexes", "extraIndexes", "missingForeignKeys"]),
        &BTreeSet::new(),
        &none,
    );
    let statements: Vec<String> = texts(&plan).into_iter().map(|(text, _)| text).collect();
    assert_eq!(
        statements[0],
        "CREATE INDEX `ENUM_CODE_IDX` ON `ENUMERATION` (`ENUM_CODE`) ALGORITHM=INPLACE LOCK=NONE"
    );
    assert_eq!(
        statements[1],
        "DROP INDEX `stray_idx` ON `ENUMERATION` ALGORITHM=INPLACE LOCK=NONE"
    );
    assert!(
        statements[2].starts_with("ALTER TABLE \"enumeration\" ADD CONSTRAINT"),
        "{statements:?}"
    );
    assert!(
        plan.warnings.iter().any(|warning| warning.contains("copies")),
        "{:?}",
        plan.warnings
    );
}
