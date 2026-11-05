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
//! the embedded cluster through a `DatabaseConnection` row, over TCP (a socket host is refused), read only as the row
//! says; a stored password is cleared when its host changes; only the owner or a database admin changes a row; a
//! user without `DATABASE_EXTERNAL`, and a production-purpose instance, get the one vague error.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

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

/// `run#EtlMapping` with `wait` of a mapping from `ext:EXTSPEC`'s `ext_note` into an `EnumerationType`.
fn etl_from_external(check: bool) -> Result<BTreeMap<String, Value>, String> {
    let text = |pairs: &[(&str, &str)]| {
        Value::Map(
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
                .collect(),
        )
    };
    let Value::Map(mut mapping) = text(&[
        ("description", "from the second database"),
        ("sourceTypeEnumId", "EtlsSql"),
        ("sourceDb", "ext:EXTSPEC"),
        (
            "sourceSql",
            "SELECT 'ExtEtlType' AS type_id, note FROM ext_note ORDER BY note LIMIT 1",
        ),
        ("targetTypeEnumId", "EtltEntity"),
        ("targetName", "moqui.basic.EnumerationType"),
    ]) else {
        unreachable!()
    };
    mapping.insert(
        "fields".to_string(),
        Value::List(vec![
            text(&[("targetField", "enumTypeId"), ("sourceColumn", "type_id")]),
            text(&[("targetField", "description"), ("sourceColumn", "note")]),
        ]),
    );
    let parameters: BTreeMap<String, Value> = [
        ("mapping".to_string(), Value::Map(mapping)),
        ("check".to_string(), Value::Bool(check)),
        ("wait".to_string(), Value::Bool(true)),
    ]
    .into_iter()
    .collect();
    let result = moqui_core::service::call::sync_checked(
        "org.moqui.impl.EtlServices.run#EtlMapping",
        &parameters,
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    )
    .map_err(|err| err.message().to_string());
    moqui_core::context::ExecutionContext::with_current(|ec| ec.message().clear_errors());
    result
}

fn entity_auto(name: &str, parameters: &[(&str, Value)]) {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    moqui_core::service::call::sync_checked(
        name,
        &parameters,
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    )
    .unwrap_or_else(|err| panic!("{name}: {err}"));
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

/// Sets `instance_purpose` and puts the old one back when dropped, a panic included.
struct Purpose(String);

impl Purpose {
    fn set(purpose: &str) -> Self {
        let old = moqui_lib::env::get_property("instance_purpose").unwrap_or_default();
        moqui_lib::env::set_property("instance_purpose", purpose);
        Self(old)
    }
}

impl Drop for Purpose {
    fn drop(&mut self) {
        moqui_lib::env::set_property("instance_purpose", self.0.clone());
    }
}

/// A TCP port on loopback that forwards each connection to the cluster's Unix socket, so a connection row reaches the
/// second database over TCP, as rows must (the embedded cluster itself listens on no TCP port).
fn tcp_relay(socket: PathBuf) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let Ok(server) = UnixStream::connect(&socket) else {
                continue;
            };
            relay(client, server);
        }
    });
    port
}

fn relay(client: TcpStream, server: UnixStream) {
    let (mut client_read, mut server_write) = (client.try_clone().unwrap(), server.try_clone().unwrap());
    std::thread::spawn(move || copy(&mut client_read, &mut server_write));
    let (mut server_read, mut client_write) = (server, client);
    std::thread::spawn(move || copy(&mut server_read, &mut client_write));
}

fn copy(from: &mut impl Read, to: &mut impl Write) {
    let mut buffer = [0_u8; 16 * 1024];
    while let Ok(read) = from.read(&mut buffer) {
        if read == 0 || to.write_all(&buffer[..read]).is_err() {
            break;
        }
    }
}

fn store(
    id: &str,
    host: &str,
    port: u16,
    password: Option<&str>,
    read_only: bool,
) -> Result<BTreeMap<String, Value>, String> {
    let mut parameters = vec![
        ("connectionId", Value::string(id)),
        ("description", Value::string("Second database")),
        ("dbType", Value::string("postgres")),
        ("host", Value::string(host)),
        ("port", Value::string(port.to_string())),
        ("databaseName", Value::string("ext_spec")),
        ("userName", Value::string("postgres")),
        ("readOnly", Value::Bool(read_only)),
    ];
    if let Some(password) = password {
        parameters.push(("password", Value::string(password)));
    }
    call("store#DatabaseConnection", &parameters)
}

fn run_sql(db: &str, sql: &str, extra: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, String> {
    let mut parameters = vec![("db", Value::string(db)), ("sqlText", Value::string(sql))];
    parameters.extend(extra.iter().cloned());
    call("run#Sql", &parameters)
}

fn login(user: &str) {
    moqui_core::user::internal_login_user(user).unwrap();
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
    login("john.doe");
    // the gate's server is production-purpose; external databases are a dev or test instance's
    let purpose = Purpose::set("test");

    // a second database of the embedded cluster with a row, reached over TCP through a relay to its socket
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
    let port = tcp_relay(Path::new(&data).join(".s.PGSQL.5432"));

    // a socket host is refused: only TCP
    let socket = store("EXTSOCK", &data.display().to_string(), 5432, Some(&password), true).unwrap_err();
    assert!(socket.contains("TCP"), "{socket}");

    store("EXTSPEC", "127.0.0.1", port, Some(&password), true).unwrap();
    // a store with no password and the same address keeps it; the password is never answered
    let kept = store("EXTSPEC", "127.0.0.1", port, None, true).unwrap();
    assert_eq!(kept.get("passwordCleared"), Some(&Value::Bool(false)));
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

    // a change of the host with no password clears the stored one, so it is never sent elsewhere
    let moved = store("EXTSPEC", "localhost", port, None, true).unwrap();
    assert_eq!(moved.get("passwordCleared"), Some(&Value::Bool(true)));
    let found = call("find#DatabaseConnections", &[]).unwrap();
    assert!(rows(&found, "connections")
        .iter()
        .any(|row| field(row, "connectionId") == Value::string("EXTSPEC")
            && field(row, "hasPassword") == Value::Bool(false)));
    store("EXTSPEC", "127.0.0.1", port, Some(&password), true).unwrap();

    // SQL through the read-only connection: reads, a write refused by the database, changes refused before any token
    let ran = run_sql("ext:EXTSPEC", "SELECT note FROM ext_note", &[]).unwrap();
    let result = rows(&ran, "results").remove(0);
    assert_eq!(
        to_plain_string(&field(&result, "records")),
        to_plain_string(&Value::List(vec![Value::List(vec![Value::string(
            "from the second database"
        )])]))
    );
    let write = run_sql("ext:EXTSPEC", "INSERT INTO ext_note VALUES ('no')", &[]).unwrap();
    let error = to_plain_string(&field(&rows(&write, "results")[0], "error"));
    assert!(error.contains("read-only"), "{error}");
    let changes = run_sql(
        "ext:EXTSPEC",
        "INSERT INTO ext_note VALUES ('no')",
        &[("allowChanges", Value::Bool(true)), ("token", Value::string("x"))],
    )
    .unwrap_err();
    assert_eq!(changes, "This connection is read only");
    let analyze = call(
        "explain#Sql",
        &[
            ("db", Value::string("ext:EXTSPEC")),
            ("sqlText", Value::string("INSERT INTO ext_note VALUES ('no')")),
            ("analyze", Value::Bool(true)),
        ],
    )
    .unwrap_err();
    assert_eq!(analyze, "This connection is read only");
    let reset = call("reset#DatabaseStatements", &[("db", Value::string("ext:EXTSPEC"))]).unwrap_err();
    assert_eq!(reset, "This connection is read only");
    let schema = call("compare#Schema", &[("db", Value::string("ext:EXTSPEC"))]).unwrap_err();
    assert!(schema.contains("not offered on an external database"), "{schema}");

    // a read-write connection lets a confirmed change through
    store("EXTRW", "127.0.0.1", port, Some(&password), false).unwrap();
    let insert = "INSERT INTO ext_note VALUES ('changed')";
    let classified = call(
        "classify#Sql",
        &[("db", Value::string("ext:EXTRW")), ("sqlText", Value::string(insert))],
    )
    .unwrap();
    let changed = run_sql(
        "ext:EXTRW",
        insert,
        &[
            ("allowChanges", Value::Bool(true)),
            ("token", classified.get("token").cloned().unwrap_or(Value::Null)),
        ],
    )
    .unwrap();
    assert_eq!(field(&rows(&changed, "results")[0], "rowsAffected"), Value::I64(1));

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
    assert_eq!(rows(&grid, "data").len(), 2);

    // generic writes of the connection rows are refused, an ADMIN's through REST included
    let john = common::basic("john.doe", "moqui");
    let body = r#"{"connectionId":"EXTREST","dbType":"postgres","host":"203.0.113.9","password":"x"}"#;
    let (status, _, response) = common::exchange(
        gate.server.address(),
        &common::http_request(
            "POST",
            "/rest/e1/moqui.database.DatabaseConnection",
            &[("Authorization", &john), ("Content-Type", "application/json")],
            body,
        ),
    );
    assert!(status == 401 || status == 403, "{status} {response}");

    // another user with DATABASE_EXTERNAL may use a connection but not change it; one without it gets the vague error
    entity_auto(
        "create#moqui.security.UserAccount",
        &[
            ("userId", Value::string("EXT_OTHER")),
            ("username", Value::string("ext.other")),
            ("userFullName", Value::string("External other")),
        ],
    );
    entity_auto(
        "create#moqui.security.UserAccount",
        &[
            ("userId", Value::string("EXT_PLAIN")),
            ("username", Value::string("ext.plain")),
            ("userFullName", Value::string("External plain")),
        ],
    );
    entity_auto(
        "create#moqui.security.UserGroup",
        &[
            ("userGroupId", Value::string("EXT_SPEC_USERS")),
            ("description", Value::string("External spec")),
        ],
    );
    entity_auto(
        "create#moqui.security.UserGroupPermission",
        &[
            ("userGroupId", Value::string("EXT_SPEC_USERS")),
            ("userPermissionId", Value::string("DATABASE_EXTERNAL")),
            ("fromDate", Value::string("2000-01-01T00:00:00Z")),
        ],
    );
    entity_auto(
        "create#moqui.security.UserGroupPermission",
        &[
            ("userGroupId", Value::string("EXT_SPEC_USERS")),
            ("userPermissionId", Value::string("SQL_RUNNER_WEB")),
            ("fromDate", Value::string("2000-01-01T00:00:00Z")),
        ],
    );
    entity_auto(
        "create#moqui.security.UserGroupMember",
        &[
            ("userGroupId", Value::string("EXT_SPEC_USERS")),
            ("userId", Value::string("EXT_OTHER")),
            ("fromDate", Value::string("2000-01-01T00:00:00Z")),
        ],
    );
    login("ext.other");
    let used = run_sql("ext:EXTSPEC", "SELECT note FROM ext_note", &[]);
    let updated = store("EXTSPEC", "203.0.113.9", 5432, None, true);
    let deleted = call(
        "delete#DatabaseConnection",
        &[("connectionId", Value::string("EXTSPEC"))],
    );
    login("ext.plain");
    let plain = call("get#Capabilities", &[("db", Value::string("ext:EXTSPEC"))]).unwrap_err();
    let plain_list = call("find#DatabaseConnections", &[]).unwrap_err();
    let plain_settings = call(
        "get#DatabaseSettings",
        &[
            ("db", Value::string("ext:EXTSPEC")),
            ("filter", Value::string("work_mem")),
        ],
    )
    .unwrap_err();
    login("john.doe");
    assert_eq!(plain_settings, "Not available");
    assert!(used.is_ok(), "{used:?}");
    assert!(updated.unwrap_err().contains("another user's"));
    assert!(deleted.unwrap_err().contains("another user's"));
    assert_eq!(plain, "Not available");
    assert_eq!(plain_list, "Not available");

    // the settings of the embedded cluster, filtered, with the conf's four marked
    let settings = call("get#DatabaseSettings", &[("filter", Value::string("work_mem"))]).unwrap();
    assert_eq!(settings.get("embedded"), Some(&Value::Bool(true)));
    assert!(to_plain_string(&settings["note"]).contains("restart"), "{settings:?}");
    let work_mem = rows(&settings, "settings")
        .into_iter()
        .find(|row| field(row, "name") == Value::string("work_mem"))
        .expect("work_mem");
    assert_eq!(field(&work_mem, "confAttribute"), Value::string("work-mem"));
    assert!(rows(&settings, "settings")
        .iter()
        .all(|row| to_plain_string(&field(row, "name")).contains("work_mem")));
    let external = call("get#DatabaseSettings", &[("db", Value::string("ext:EXTSPEC"))]).unwrap();
    assert_eq!(external.get("embedded"), Some(&Value::Bool(false)));
    // the Tools gate, with authorization on: the service granted outright, as a screen's inheritance would let it run,
    // to a user the Tools app does not let in
    entity_auto(
        "create#moqui.security.ArtifactGroup",
        &[
            ("artifactGroupId", Value::string("EXT_SPEC_API")),
            ("description", Value::string("External spec")),
        ],
    );
    entity_auto(
        "create#moqui.security.ArtifactGroupMember",
        &[
            ("artifactGroupId", Value::string("EXT_SPEC_API")),
            (
                "artifactName",
                Value::string(format!("{SERVICES}.get#DatabaseSettings")),
            ),
            ("artifactTypeEnumId", Value::string("AT_SERVICE")),
            ("inheritAuthz", Value::string("N")),
        ],
    );
    entity_auto(
        "create#moqui.security.ArtifactAuthz",
        &[
            ("artifactAuthzId", Value::string("EXT_SPEC_API_ALL")),
            ("userGroupId", Value::string("ALL_USERS")),
            ("artifactGroupId", Value::string("EXT_SPEC_API")),
            ("authzTypeEnumId", Value::string("AUTHZT_ALLOW")),
            ("authzActionEnumId", Value::string("AUTHZA_ALL")),
        ],
    );
    login("ext.plain");
    let gated = moqui_core::service::call::sync_checked(
        &format!("{SERVICES}.get#DatabaseSettings"),
        &BTreeMap::new(),
        &moqui_core::service::call::CallOptions::default(),
    )
    .map_err(|err| err.message().to_string());
    moqui_core::context::ExecutionContext::with_current(|ec| ec.message().clear_errors());
    login("john.doe");
    assert!(gated.unwrap_err().contains("Tools app"));

    // an ETL from a SQL source reads ext: (its first note, "changed" by EXTRW) through the same gate and pool,
    // read only, audited as DbaSql
    let checked = etl_from_external(true).unwrap();
    assert_eq!(checked.get("extractCount"), Some(&Value::I64(1)), "{checked:?}");
    let ran = etl_from_external(false).unwrap();
    assert_eq!(ran.get("loadCount"), Some(&Value::I64(1)), "{ran:?}");
    let authz_off = context.disable_authz();
    let loaded = facade
        .find("moqui.basic.EnumerationType")
        .unwrap()
        .eq("enumTypeId", Value::string("ExtEtlType"))
        .use_cache(false)
        .one()
        .unwrap()
        .expect("the row the ETL loaded");
    let etl_audited = facade
        .find("moqui.database.DatabaseAudit")
        .unwrap()
        .eq("db", Value::string("ext:EXTSPEC"))
        .eq("actionEnumId", Value::string("DbaSql"))
        .list()
        .unwrap()
        .iter()
        .filter(|row| {
            row.get("detailJson")
                .map(to_plain_string)
                .is_some_and(|detail| detail.contains("\"etl\""))
        })
        .count();
    drop(authz_off);
    assert_eq!(to_plain_string(loaded.get("description").unwrap_or(&Value::Null)), "changed");
    assert_eq!(etl_audited, 2);

    // a production instance refuses ext: and the connection services with the one vague error
    let production = Purpose::set("production");
    let etl_refused = etl_from_external(false);
    let refused = run_sql("ext:EXTSPEC", "SELECT note FROM ext_note", &[]).unwrap_err();
    let listed = call("find#DatabaseConnections", &[]).unwrap_err();
    let databases = call("get#DatabaseList", &[]).unwrap();
    drop(production);
    drop(purpose);
    assert_eq!(refused, "Not available");
    assert_eq!(listed, "Not available");
    let etl_refused = match etl_refused {
        Ok(out) => to_plain_string(&Value::Map(out.into_iter().collect())),
        Err(message) => message,
    };
    assert!(etl_refused.contains("Not available"), "{etl_refused}");
    assert!(!rows(&databases, "databases")
        .iter()
        .any(|db| field(db, "db") == Value::string("ext:EXTSPEC")));

    drop(gate);
}
