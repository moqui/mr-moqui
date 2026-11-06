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
                some("000002")
            ),
            ("DROP INDEX CONCURRENTLY IF EXISTS \"stray_idx\"".to_string(), None),
            (
                "ALTER TABLE \"enumeration\" ADD CONSTRAINT \"ENUM_STATUS_FLOW\" FOREIGN KEY (\"status_flow_id\") \
                 REFERENCES \"status_flow\" (\"status_flow_id\") NOT VALID"
                    .to_string(),
                None
            ),
            (
                "ALTER TABLE \"enumeration\" VALIDATE CONSTRAINT \"enum_status_flow\"".to_string(),
                some("000005")
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

/// A MySQL table's differences as the comparison gives them, its DDL from the builders on the `mysql8` conf: the
/// foreign key index and the key to StatusFlow missing, a plain and a unique index extra.
fn mysql_table(dialect: &moqui_db::dialect::Dialect) -> TableDiff {
    let conf = MNode::parse_file(moqui_core::context::framework_dir().join("conf/MoquiDefaultConf.xml")).expect("conf");
    let model =
        moqui_db::model::EntityModel::load_directory(&conf, &moqui_core::context::framework_dir().join("entity"))
            .expect("model");
    let entity = model.entity("moqui.basic.Enumeration").expect("entity");
    let relationship = entity
        .relationships
        .iter()
        .find(|relationship| relationship.related == "moqui.basic.StatusFlow")
        .expect("relationship");
    let related = model.entity("moqui.basic.StatusFlow").expect("related");
    let table = moqui_db::sql::physical_name(dialect, &entity.table_name);
    TableDiff {
        entity_name: entity.full_name.clone(),
        group: "transactional".to_string(),
        table_name: table,
        missing_indexes: vec![IndexDiff {
            name: moqui_db::sql::foreign_key_index_name(entity, relationship, dialect),
            columns: vec!["STATUS_FLOW_ID".to_string()],
            unique: false,
            kind: IndexKind::ForeignKey,
            ddl: vec![moqui_db::sql::create_fk_index(entity, relationship, dialect).expect("index ddl")],
        }],
        extra_indexes: vec![
            CatalogIndex {
                name: "STRAY_IDX".to_string(),
                valid: true,
                ..CatalogIndex::default()
            },
            CatalogIndex {
                name: "STRAY_UNIQUE".to_string(),
                unique: true,
                valid: true,
                ..CatalogIndex::default()
            },
        ],
        missing_foreign_keys: vec![ForeignKeyDiff {
            name: moqui_db::sql::foreign_key_name(entity, relationship, dialect),
            related_entity: related.full_name.clone(),
            related_table: moqui_db::sql::physical_name(dialect, &related.table_name),
            columns: vec!["STATUS_FLOW_ID".to_string()],
            ddl: moqui_db::sql::create_foreign_key(entity, relationship, related, dialect).ok(),
        }],
        ..TableDiff::default()
    }
}

/// MySQL: indexes in place without a lock, a unique index (a constraint on InnoDB) never dropped, and a key added as
/// the builder writes it, with the warning that it copies the table.
#[test]
fn a_mysql_plan_builds_in_place_and_keeps_unique_indexes() {
    let mut plan = Plan::default();
    let none = |_: &str, _: &str| None;
    let dialect = dialect("mysql8");
    let diff = mysql_table(&dialect);
    plan_table(
        &mut plan,
        &diff,
        &dialect,
        Server::Mysql,
        &changes(&["missingIndexes", "extraIndexes", "missingForeignKeys"]),
        &BTreeSet::new(),
        &none,
    );
    let statements: Vec<String> = texts(&plan).into_iter().map(|(text, _)| text).collect();
    assert_eq!(statements.len(), 3, "{statements:?}");
    assert!(
        statements[0].starts_with("CREATE INDEX `") && statements[0].ends_with(") ALGORITHM=INPLACE LOCK=NONE"),
        "{statements:?}"
    );
    assert_eq!(
        statements[1],
        format!(
            "DROP INDEX `STRAY_IDX` ON `{}` ALGORITHM=INPLACE LOCK=NONE",
            diff.table_name
        )
    );
    assert!(
        !statements.iter().any(|sql| sql.contains("STRAY_UNIQUE")),
        "{statements:?}"
    );
    assert!(
        statements[2].starts_with(&format!("ALTER TABLE `{}` ADD CONSTRAINT `", diff.table_name)),
        "{statements:?}"
    );
    assert!(
        plan.warnings.iter().any(|warning| warning.contains("copies")),
        "{:?}",
        plan.warnings
    );
    assert!(
        plan.warnings.iter().any(|warning| warning.contains("STRAY_UNIQUE")),
        "{:?}",
        plan.warnings
    );
}

/// The MySQL catalog marks a unique index as a constraint's, so the comparison never calls it extra.
#[test]
fn the_mysql_catalog_marks_unique_indexes_as_constraints() {
    assert!(moqui_db::sql::catalog::MYSQL_INDEXES
        .contains("INDEX_NAME = 'PRIMARY' OR MIN(NON_UNIQUE) = 0 AS is_constraint"));
}
