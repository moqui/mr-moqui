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

//! External databases (plans/Tools-Database.md §4.3, §1 decision 4): `moqui.database.DatabaseConnection` rows, reached
//! as `db=ext:<connectionId>`, never on a production instance.
//!
//! - **The gate** ([`gate`]): not production (`moqui_core::web::is_production`) and the `DATABASE_EXTERNAL` permission.
//!   Every refusal is the one vague [`crate::NOT_AVAILABLE`], naming neither cause, and on production a connection row
//!   is neither read nor used.
//! - **Connecting** ([`open`]): through the host's conf connection factory (`moqui_core::driver::conf_driver`,
//!   `moqui_drivers::conf_driver`), with the row's `sslMode` and `sslRootCert`. One pool per connection, closed after
//!   [`IDLE_CLOSE`] unused, or when the row changes. A `readOnly` connection runs every statement read only.
//! - **The services** ([`register`]): store, find, delete and test of connections; the password is never answered,
//!   and a store without one keeps the stored one.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;
use moqui_core::transaction::{PoolConfig, TransactionManager};

use crate::group_db::GroupDb;
use crate::service::{add_error, flag, text};
use crate::NOT_AVAILABLE;

pub const DATABASE_CONNECTION: &str = "moqui.database.DatabaseConnection";
/// A connection's pool is closed after this long unused.
pub const IDLE_CLOSE: Duration = Duration::from_secs(600);
/// The connections a pool keeps open.
const POOL_MAX: u32 = 4;
/// The database confs a connection may name.
const DB_TYPES: &[&str] = &["postgres", "mysql", "mysql8"];

/// Whether external databases may be used here, by this user: not a production instance, and `DATABASE_EXTERNAL`.
#[must_use]
pub fn gate() -> bool {
    !moqui_core::web::is_production() && moqui_core::user::has_permission("DATABASE_EXTERNAL").unwrap_or(false)
}

struct Open {
    tx: TransactionManager,
    dialect: moqui_db::dialect::Dialect,
    read_only: bool,
    last_used: Instant,
}

fn pools() -> &'static Mutex<HashMap<String, Open>> {
    static POOLS: OnceLock<Mutex<HashMap<String, Open>>> = OnceLock::new();
    POOLS.get_or_init(|| {
        // closes the pools unused past IDLE_CLOSE, once a minute, even when no request comes
        let _ = std::thread::Builder::new()
            .name("moqui-ext-db-idle".to_string())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(60));
                close_idle();
            });
        Mutex::new(HashMap::new())
    })
}

/// Close the pools unused past [`IDLE_CLOSE`].
pub fn close_idle() {
    lock(pools()).retain(|id, open| {
        let keep = open.last_used.elapsed() < IDLE_CLOSE;
        if !keep {
            tracing::info!("Closed the idle pool of external database {id}");
        }
        keep
    });
}

/// Forget the pool of `connection_id` (its row changed or went).
pub fn forget(connection_id: &str) {
    lock(pools()).remove(connection_id);
    crate::group_db::forget_server(&format!("ext:{connection_id}"));
}

/// A connection row's fields as the conf connection factory takes them.
fn conf_of(row: &moqui_core::entity::EntityValue) -> moqui_core::driver::ConfDb {
    let field = |name: &str| {
        row.get(name)
            .filter(|value| !is_empty(value))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    moqui_core::driver::ConfDb {
        db_type: field("dbType"),
        host: field("host"),
        port: field("port"),
        database: field("databaseName"),
        user: field("userName"),
        password: field("password"),
        ssl_mode: field("sslMode"),
        ssl_root_cert: field("sslRootCert"),
    }
}

/// The dialect of a database conf: an open group's when one uses it, else the conf's `database-list` entry.
fn dialect_of(facade: &EntityFacade, db_type: &str) -> Result<moqui_db::dialect::Dialect, Error> {
    if let Some((_, dialect, _)) = facade.groups().find(|(_, dialect, _)| dialect.name == db_type) {
        return Ok(dialect.clone());
    }
    let text = ExecutionContext::with_current(|ec| ec.factory().conf_text())
        .ok_or_else(|| Error::new("no execution context to read the database list"))?;
    let conf = moqui_lib::node::MNode::parse_shared("moqui-conf", &text)?;
    moqui_db::dialect::Dialects::load(&conf)?
        .get(db_type)
        .cloned()
        .ok_or_else(|| Error::new(format!("the database list has no {db_type}")))
}

/// The connection row `connection_id`, read past the entity authorization (the gate decided).
fn row_of(facade: &EntityFacade, connection_id: &str) -> Result<Option<moqui_core::entity::EntityValue>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .find(DATABASE_CONNECTION)?
        .eq("connectionId", Value::string(connection_id))
        .one()
}

/// The database of the connection `connection_id`, its pool made or reused. The caller passed [`gate`].
///
/// # Errors
///
/// No such row, an unknown database conf, or no driver for it.
pub fn open(facade: &EntityFacade, connection_id: &str) -> Result<GroupDb, Error> {
    let name = format!("ext:{connection_id}");
    // the row is read and the pool made once, under the map's lock, so two requests share one pool; a pool is
    // forgotten when its row is stored or deleted, the only ways it changes (generic writes are refused)
    let (tx, dialect, read_only) = {
        let mut pools = lock(pools());
        if let Some(open) = pools.get_mut(connection_id) {
            open.last_used = Instant::now();
            (open.tx.clone(), open.dialect.clone(), open.read_only)
        } else {
            let row =
                row_of(facade, connection_id)?.ok_or_else(|| Error::new(format!("no connection {connection_id}")))?;
            let conf = conf_of(&row);
            if !DB_TYPES.contains(&conf.db_type.as_str()) {
                return Err(Error::new(format!(
                    "database conf {} is not postgres, mysql or mysql8",
                    conf.db_type
                )));
            }
            check_host(&conf.host).map_err(Error::new)?;
            let read_only = row.get("readOnly").map(to_plain_string).as_deref() != Some("N");
            let dialect = dialect_of(facade, &conf.db_type)?;
            let driver = moqui_core::driver::conf_driver(&conf)?;
            let tx = TransactionManager::new(
                driver,
                PoolConfig {
                    min: 0,
                    max: POOL_MAX,
                    reserve: 0,
                    acquire_timeout: Duration::from_secs(30),
                    idle_timeout: Some(IDLE_CLOSE),
                    ..PoolConfig::default()
                },
            )?;
            pools.insert(
                connection_id.to_string(),
                Open {
                    tx: tx.clone(),
                    dialect: dialect.clone(),
                    read_only,
                    last_used: Instant::now(),
                },
            );
            (tx, dialect, read_only)
        }
    };
    Ok(GroupDb::on(&name, tx, dialect, true, read_only))
}

/// A connection's host: a name or address reached over TCP. A Unix socket directory (a path) or a Windows pipe is
/// refused, so a holder of `DATABASE_EXTERNAL` cannot reach this machine's server through local trust or peer
/// authentication.
fn check_host(host: &str) -> Result<(), &'static str> {
    let host = host.trim();
    if host.is_empty() {
        return Err("host is required: a name or address reached over TCP");
    }
    if host.starts_with('/') || host.starts_with('\\') || host.starts_with('@') || host.contains(char::is_whitespace) {
        return Err("host is a name or address reached over TCP, not a socket path");
    }
    Ok(())
}

/// A host on this machine's loopback, the only kind a connection may reach without TLS by default.
fn is_loopback(host: &str) -> bool {
    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1" | "[::1]"
    )
}

/// `sslMode` as stored: `disable`, `require`, `verify-ca` or `verify-full` (MySQL's `DISABLED`, `REQUIRED`,
/// `VERIFY_CA` and `VERIFY_IDENTITY` read so too). Empty is `verify-full` for a host that is not loopback, and stays
/// empty (no TLS) for a loopback one.
fn ssl_mode_of(mode: &str, host: &str) -> Result<String, &'static str> {
    let mode = mode.trim().to_ascii_lowercase().replace('_', "-");
    Ok(match mode.as_str() {
        "" if is_loopback(host) => String::new(),
        "" | "verify-full" | "verify-identity" => "verify-full".to_string(),
        "disable" | "disabled" => "disable".to_string(),
        "require" | "required" => "require".to_string(),
        "verify-ca" => "verify-ca".to_string(),
        _ => return Err("sslMode is disable, require, verify-ca or verify-full"),
    })
}

/// The group name (`ext:<connectionId>`) of an external connection the user may use now: [`gate`] passes and the
/// connection opens. `None` otherwise, the reason logged and never answered.
#[must_use]
pub fn external_group(facade: &EntityFacade, connection_id: &str) -> Option<String> {
    if !gate() {
        return None;
    }
    match open(facade, connection_id) {
        Ok(db) => Some(db.group),
        Err(err) => {
            tracing::info!(error = %err, "External database {connection_id} is not available");
            None
        }
    }
}

/// The connection rows the user may pick, as the database list shows them: `db`, `description`, `databaseConf`,
/// `readOnly`. Empty when the gate does not pass.
#[must_use]
pub fn database_list(facade: &EntityFacade) -> Vec<IndexMap<String, Value>> {
    if !gate() {
        return Vec::new();
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(DATABASE_CONNECTION)
        .and_then(|find| find.order_by("connectionId", false).list());
    match rows {
        Ok(rows) => rows
            .iter()
            .map(|row| {
                let id = row.get("connectionId").map(to_plain_string).unwrap_or_default();
                let mut map = IndexMap::new();
                map.insert("db".to_string(), Value::String(format!("ext:{id}")));
                map.insert(
                    "description".to_string(),
                    row.get("description").cloned().unwrap_or(Value::Null),
                );
                map.insert(
                    "databaseConf".to_string(),
                    row.get("dbType").cloned().unwrap_or(Value::Null),
                );
                map.insert(
                    "readOnly".to_string(),
                    Value::Bool(row.get("readOnly").map(to_plain_string).as_deref() != Some("N")),
                );
                map.insert("external".to_string(), Value::Bool(true));
                map
            })
            .collect(),
        Err(err) => {
            tracing::warn!(error = %err, "Could not list the external database connections");
            Vec::new()
        }
    }
}

// ---- services ----

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, find_database_connections)?;
    rust_service!(map, store_database_connection)?;
    rust_service!(map, delete_database_connection)?;
    rust_service!(map, test_database_connection)?;
    rust_service!(map, get_database_list)?;
    Ok(())
}

type Out = Result<BTreeMap<String, Value>, Error>;

fn none() -> Out {
    Ok(BTreeMap::new())
}

/// The gate of a connection service: the Tools app, then [`gate`]; a refusal adds [`NOT_AVAILABLE`].
fn gated() -> bool {
    if !crate::service::may_view_tools() {
        return false;
    }
    if gate() {
        true
    } else {
        add_error(NOT_AVAILABLE);
        false
    }
}

/// A connection row as the services answer it: every field but the password, and `hasPassword`.
fn connection_value(row: &moqui_core::entity::EntityValue) -> Value {
    let mut map = IndexMap::new();
    for field in [
        "connectionId",
        "description",
        "dbType",
        "host",
        "port",
        "databaseName",
        "userName",
        "sslMode",
        "sslRootCert",
        "readOnly",
        "ownerUserId",
    ] {
        map.insert(field.to_string(), row.get(field).cloned().unwrap_or(Value::Null));
    }
    map.insert(
        "hasPassword".to_string(),
        Value::Bool(row.get("password").is_some_and(|value| !is_empty(value))),
    );
    Value::Map(map)
}

/// `find#DatabaseConnections`: every connection row, without passwords.
fn find_database_connections(_parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !gated() {
        return none();
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let rows = facade
        .find(DATABASE_CONNECTION)?
        .order_by("connectionId", false)
        .list()?;
    let mut out = BTreeMap::new();
    out.insert(
        "connections".to_string(),
        Value::List(rows.iter().map(connection_value).collect()),
    );
    Ok(out)
}

fn valid_connection_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

/// `store#DatabaseConnection`: create or update a connection row. A store with no `password` keeps the stored one.
fn store_database_connection(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !gated() {
        return none();
    }
    let connection_id = text(parameters, "connectionId");
    if !valid_connection_id(&connection_id) {
        add_error("connectionId is 1 to 40 letters, digits, _ or -");
        return none();
    }
    let db_type = text(parameters, "dbType");
    if !DB_TYPES.contains(&db_type.as_str()) {
        add_error("dbType is postgres, mysql (MariaDB too) or mysql8");
        return none();
    }
    let port = text(parameters, "port");
    if !port.is_empty() && port.parse::<u16>().is_err() {
        add_error("port is a number");
        return none();
    }
    let host = text(parameters, "host").trim().to_string();
    if let Err(message) = check_host(&host) {
        add_error(message);
        return none();
    }
    let ssl_mode = match ssl_mode_of(&text(parameters, "sslMode"), &host) {
        Ok(mode) => mode,
        Err(message) => {
            add_error(message);
            return none();
        }
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let existing = facade
        .find(DATABASE_CONNECTION)?
        .eq("connectionId", Value::string(&connection_id))
        .one()?;
    if let Some(row) = &existing {
        if !may_manage(row) {
            add_error("That connection is another user's to change");
            return none();
        }
    }
    let mut row = match &existing {
        Some(row) => row.clone(),
        None => {
            let mut row = facade.make_value(DATABASE_CONNECTION)?;
            row.set("connectionId", Value::string(&connection_id));
            row.set(
                "ownerUserId",
                moqui_core::user::current_user_id()
                    .ok()
                    .flatten()
                    .map_or(Value::Null, Value::String),
            );
            row
        }
    };
    let port_value = port.parse::<i64>().map_or(Value::Null, Value::I64);
    let new_values: [(&str, Value); 6] = [
        ("dbType", Value::String(db_type)),
        ("host", Value::String(host)),
        ("port", port_value),
        ("userName", Value::String(text(parameters, "userName"))),
        ("sslMode", Value::String(ssl_mode)),
        ("sslRootCert", Value::String(text(parameters, "sslRootCert"))),
    ];
    // where the password goes: a change of any of these without a new password clears it, so a stored password is
    // never sent to a host, user or weaker TLS other than the one it was entered for
    let text_of = |value: Option<&Value>| {
        value
            .filter(|value| !is_empty(value))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let redirected = existing.as_ref().is_some_and(|old| {
        new_values
            .iter()
            .any(|(field, value)| text_of(old.get(field)) != text_of(Some(value)))
    });
    for (field, value) in new_values {
        row.set(field, value);
    }
    for field in ["description", "databaseName"] {
        row.set(field, Value::String(text(parameters, field)));
    }
    let read_only = parameters.get("readOnly").is_none_or(is_empty) || flag(parameters, "readOnly");
    row.set("readOnly", Value::string(if read_only { "Y" } else { "N" }));
    let password = text(parameters, "password");
    let mut password_cleared = false;
    if !password.is_empty() {
        row.set("password", Value::String(password));
    } else if redirected {
        password_cleared = existing
            .as_ref()
            .is_some_and(|old| old.get("password").is_some_and(|value| !is_empty(value)));
        row.set("password", Value::Null);
    }
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    forget(&connection_id);
    let mut out = BTreeMap::new();
    out.insert("connectionId".to_string(), Value::String(connection_id));
    out.insert("passwordCleared".to_string(), Value::Bool(password_cleared));
    Ok(out)
}

/// Whether the current user may change, delete or test the connection `row`: its owner, or a `DATABASE_ADMIN`
/// holder. Others may only use it.
fn may_manage(row: &moqui_core::entity::EntityValue) -> bool {
    let owner = row
        .get("ownerUserId")
        .map(to_plain_string)
        .filter(|owner| !owner.is_empty());
    let user = moqui_core::user::current_user_id().ok().flatten();
    (owner.is_some() && owner == user) || moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false)
}

/// `delete#DatabaseConnection`.
fn delete_database_connection(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !gated() {
        return none();
    }
    let connection_id = text(parameters, "connectionId");
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    if let Some(row) = facade
        .find(DATABASE_CONNECTION)?
        .eq("connectionId", Value::string(&connection_id))
        .one()?
    {
        if !may_manage(&row) {
            add_error("That connection is another user's to delete");
            return none();
        }
        row.delete(facade)?;
    }
    forget(&connection_id);
    none()
}

/// `test#DatabaseConnection`: connect and ask the server's version. Out: `connected`, `version`, `reason` (short; the
/// detail is logged).
fn test_database_connection(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !gated() {
        return none();
    }
    let connection_id = text(parameters, "connectionId");
    match row_of(facade, &connection_id)? {
        Some(row) if may_manage(&row) => {}
        Some(_) => {
            add_error("That connection is another user's to test");
            return none();
        }
        None => {
            add_error(format!("No connection {connection_id}"));
            return none();
        }
    }
    let mut out = BTreeMap::new();
    let tested = open(facade, &connection_id).and_then(|db| {
        let sql = if db.is_mysql() {
            "SELECT VERSION() AS version"
        } else {
            "SELECT version() AS version"
        };
        db.query(sql, &[])
    });
    match tested {
        Ok(rows) => {
            out.insert("connected".to_string(), Value::Bool(true));
            out.insert(
                "version".to_string(),
                rows.first()
                    .and_then(|row| row.get("version"))
                    .cloned()
                    .unwrap_or(Value::Null),
            );
        }
        Err(err) => {
            tracing::info!(error = %err, "The test of external database {connection_id} failed");
            forget(&connection_id);
            out.insert("connected".to_string(), Value::Bool(false));
            out.insert(
                "reason".to_string(),
                Value::string("The connection did not open (the server log has the detail)"),
            );
        }
    }
    Ok(out)
}

/// `get#DatabaseList`: the databases a picker offers: each entity group's database (`group:<name>`, the groups that
/// share it in `groupNames`), then, when the gate passes and `includeExternal` is not false, each connection
/// (`ext:<id>`). Out: `databases`, each `{db, groupNames, databaseConf, description, readOnly, external}`.
fn get_database_list(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    if !crate::service::may_view_tools() {
        return none();
    }
    let mut databases: Vec<Value> = Vec::new();
    let mut seen: Vec<TransactionManager> = Vec::new();
    for name in facade.group_names() {
        let Some((_, dialect, tx)) = facade.groups().find(|(group, _, _)| *group == name) else {
            continue;
        };
        if let Some(index) = seen.iter().position(|other| other.shares_stack(tx)) {
            if let Some(Value::Map(map)) = databases.get_mut(index) {
                if let Some(Value::List(names)) = map.get_mut("groupNames") {
                    names.push(Value::String(name.clone()));
                }
            }
            continue;
        }
        seen.push(tx.clone());
        let mut map = IndexMap::new();
        map.insert("db".to_string(), Value::String(format!("group:{name}")));
        map.insert("groupNames".to_string(), Value::List(vec![Value::String(name.clone())]));
        map.insert("databaseConf".to_string(), Value::string(&dialect.name));
        map.insert("description".to_string(), Value::Null);
        map.insert("readOnly".to_string(), Value::Bool(false));
        map.insert("external".to_string(), Value::Bool(false));
        databases.push(Value::Map(map));
    }
    let include_external =
        parameters.get("includeExternal").is_none_or(is_empty) || flag(parameters, "includeExternal");
    if include_external {
        databases.extend(database_list(facade).into_iter().map(Value::Map));
    }
    let mut out = BTreeMap::new();
    out.insert("databases".to_string(), Value::List(databases));
    Ok(out)
}
