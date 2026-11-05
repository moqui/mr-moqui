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

//! External databases (plans/Tools-Database.md §4.3, §15 step 7): on a dev instance SQL runs on a second database of
//! the embedded cluster through a `DatabaseConnection` row, read only as the row says; a production-purpose instance
//! refuses `db=ext:…` and the connection services with the one vague error.

use std::collections::BTreeMap;

use moqui_core::driver::DbConnection as _;
use moqui_host::db::postgres::PostgresDriver;
use moqui_lib::object::{to_plain_string, Value};

use crate::common;

const SERVICES: &str = "org.moqui.impl.DatabaseServices";

fn call(verb_noun: &str, parameters: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, String> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let result = moqui_core::service::call::sync_checked(
        &format!("{SERVICES}.{verb_noun}"),
        &parameters,
        // the Tools screens grant the services by inheritance; called alone, each checks the Tools app and its gate
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    )
    .map_err(|err| err.message().to_string());
    moqui_core::context::ExecutionContext::with_current(|ec| ec.message().clear_errors());
    result
}

fn rows(out: &BTreeMap<String, Value>, key: &str) -> Vec<Value> {
    match out.get(key) {
        Some(Value::List(items)) => items.clone(),
        other => panic!("{key}: {other:?}"),
    }
}

fn field(item: &Value, key: &str) -> Value {
    match item {
        Value::Map(map) => map.get(key).cloned().unwrap_or(Value::Null),
        other => panic!("{other:?}"),
    }
}

#[test]
fn sql_runs_on_a_second_database_through_a_connection_row_and_production_refuses_it() {
    if !common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start("database-external");
    let factory = gate.server.factory();
    let facade = factory.entity().expect("entity facade");
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    // the gate's server is production-purpose; external databases are a dev or test instance's
    let purpose = moqui_lib::env::get_property("instance_purpose").unwrap_or_default();
    moqui_lib::env::set_property("instance_purpose", "test");

    // a second database of the embedded cluster, with a row in it
    let data = std::fs::canonicalize(gate.cluster_dir.join("data")).unwrap();
    let password = std::fs::read_to_string(gate.cluster_dir.join("pgpass")).unwrap();
    let password = password.trim().to_string();
    moqui_core::entity::info::run_sql(&facade, "transactional", "CREATE DATABASE ext_spec", 0).unwrap();
    let mut conn = PostgresDriver::unix_socket("postgres", "ext_spec", &data)
        .password(&password)
        .connect_postgres()
        .unwrap();
    conn.execute("CREATE TABLE ext_note (note TEXT)", &[]).unwrap();
    conn.execute("INSERT INTO ext_note VALUES ('from the second database')", &[])
        .unwrap();
    drop(conn);

    let stored = call(
        "store#DatabaseConnection",
        &[
            ("connectionId", Value::string("EXTSPEC")),
            ("description", Value::string("Second database")),
            ("dbType", Value::string("postgres")),
            ("host", Value::string(data.display().to_string())),
            ("port", Value::string("5432")),
            ("databaseName", Value::string("ext_spec")),
            ("userName", Value::string("postgres")),
            ("password", Value::string(&password)),
        ],
    )
    .unwrap();
    assert_eq!(stored.get("connectionId"), Some(&Value::string("EXTSPEC")));
    // the password is never answered, and a store without one keeps it
    call(
        "store#DatabaseConnection",
        &[
            ("connectionId", Value::string("EXTSPEC")),
            ("description", Value::string("Second database, read only")),
            ("dbType", Value::string("postgres")),
            ("host", Value::string(data.display().to_string())),
            ("port", Value::string("5432")),
            ("databaseName", Value::string("ext_spec")),
            ("userName", Value::string("postgres")),
        ],
    )
    .unwrap();
    let found = call("find#DatabaseConnections", &[]).unwrap();
    let connection = rows(&found, "connections")
        .into_iter()
        .find(|row| field(row, "connectionId") == Value::string("EXTSPEC"))
        .expect("the connection");
    assert_eq!(field(&connection, "password"), Value::Null);
    assert_eq!(field(&connection, "hasPassword"), Value::Bool(true));
    assert_eq!(field(&connection, "readOnly"), Value::string("Y"));
    let tested = call("test#DatabaseConnection", &[("connectionId", Value::string("EXTSPEC"))]).unwrap();
    assert_eq!(tested.get("connected"), Some(&Value::Bool(true)), "{tested:?}");

    // SQL through the connection, read only as the row says
    let ran = call(
        "run#Sql",
        &[
            ("db", Value::string("ext:EXTSPEC")),
            ("sqlText", Value::string("SELECT note FROM ext_note")),
        ],
    )
    .unwrap();
    let result = rows(&ran, "results").remove(0);
    assert_eq!(
        to_plain_string(&field(&result, "records")),
        to_plain_string(&Value::List(vec![Value::List(vec![Value::string(
            "from the second database"
        )])]))
    );
    let write = call(
        "run#Sql",
        &[
            ("db", Value::string("ext:EXTSPEC")),
            ("sqlText", Value::string("INSERT INTO ext_note VALUES ('no')")),
        ],
    )
    .unwrap();
    let error = to_plain_string(&field(&rows(&write, "results")[0], "error"));
    assert!(error.contains("read-only"), "{error}");
    let changes = call(
        "run#Sql",
        &[
            ("db", Value::string("ext:EXTSPEC")),
            ("sqlText", Value::string("INSERT INTO ext_note VALUES ('no')")),
            ("allowChanges", Value::Bool(true)),
        ],
    )
    .unwrap_err();
    assert!(
        changes.contains("read only") || changes.contains("confirmation"),
        "{changes}"
    );
    let authz_off = context.disable_authz();
    let audited = facade
        .find("moqui.database.DatabaseAudit")
        .unwrap()
        .eq("db", Value::string("ext:EXTSPEC"))
        .list()
        .unwrap();
    drop(authz_off);
    assert!(audited.len() >= 2, "{}", audited.len());
    let list = call("get#DatabaseList", &[]).unwrap();
    assert!(rows(&list, "databases")
        .iter()
        .any(|db| field(db, "db") == Value::string("ext:EXTSPEC")));
    let grid = call(
        "get#DataGridRows",
        &[
            (
                "source",
                Value::Map(
                    [
                        ("type".to_string(), Value::string("sql")),
                        ("db".to_string(), Value::string("ext:EXTSPEC")),
                        ("sql".to_string(), Value::string("SELECT note FROM ext_note")),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
            ("page", Value::I64(1)),
            ("size", Value::I64(10)),
        ],
    )
    .unwrap();
    assert_eq!(rows(&grid, "data").len(), 1);

    // a production instance refuses ext: and the connection services with the one vague error
    moqui_lib::env::set_property("instance_purpose", "production");
    let refused = call(
        "run#Sql",
        &[
            ("db", Value::string("ext:EXTSPEC")),
            ("sqlText", Value::string("SELECT note FROM ext_note")),
        ],
    )
    .unwrap_err();
    let listed = call("find#DatabaseConnections", &[]).unwrap_err();
    let databases = call("get#DatabaseList", &[]).unwrap();
    moqui_lib::env::set_property("instance_purpose", purpose);
    assert_eq!(refused, "Not available");
    assert_eq!(listed, "Not available");
    assert!(!rows(&databases, "databases")
        .iter()
        .any(|db| field(db, "db") == Value::string("ext:EXTSPEC")));

    drop(gate);
}
