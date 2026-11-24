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

//! The database of `moqui test-regression`: one embedded Postgres cluster on loopback TCP, a new database in it for
//! each target, and the snapshot of a database's rows that the recorder diffs.

use std::collections::BTreeMap;
use std::path::Path;

use moqui_core::driver::{DbConnection, DbDriver};
use moqui_lib::error::Error;
use serde_json::Value;

/// How an engine reaches its database.
#[derive(Clone, Debug)]
pub(crate) struct Connection {
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
}

/// The run's cluster, stopped when dropped.
pub(crate) struct Cluster {
    #[cfg(feature = "embedded-postgres")]
    cluster: crate::db::embedded::EmbeddedCluster,
    port: u16,
}

impl Cluster {
    /// Start a new cluster in `dir`, with the Postgres built into this binary unpacked for `runtime`.
    #[cfg(feature = "embedded-postgres")]
    pub(crate) fn start(runtime: &Path, dir: &Path) -> Result<Self, Error> {
        use crate::db::embedded;
        embedded::unpack_install(runtime)?;
        let port = std::net::TcpListener::bind(("127.0.0.1", 0))
            .and_then(|listener| listener.local_addr())
            .map(|addr| addr.port())
            .map_err(|err| Error::new("could not find a free port for the database").with_source(err))?;
        let mut options = embedded::StartOptions::new(200);
        options.install_dir = Some(embedded::install_dir(runtime));
        options.password = Some(embedded::SuperuserPassword(password()));
        options.tcp_port = Some(port);
        let (cluster, _) = embedded::EmbeddedCluster::start_with(dir, &options)?;
        Ok(Self { cluster, port })
    }

    #[cfg(not(feature = "embedded-postgres"))]
    pub(crate) fn start(_runtime: &Path, _dir: &Path) -> Result<Self, Error> {
        Err(Error::new(
            "moqui test-regression needs the embedded Postgres (the embedded-postgres feature)",
        ))
    }

    /// The database `name`, made when it is not there.
    #[cfg(feature = "embedded-postgres")]
    pub(crate) fn database(&self, name: &str) -> Result<Box<dyn DbDriver>, Error> {
        Ok(Box::new(self.cluster.database_driver(name)?))
    }

    #[cfg(not(feature = "embedded-postgres"))]
    pub(crate) fn database(&self, _name: &str) -> Result<Box<dyn DbDriver>, Error> {
        Err(Error::new("no embedded Postgres in this build"))
    }

    pub(crate) fn connection(&self, database: &str) -> Connection {
        #[cfg(feature = "embedded-postgres")]
        let (user, password) = self.cluster.superuser();
        #[cfg(not(feature = "embedded-postgres"))]
        let (user, password) = (String::new(), String::new());
        Connection {
            port: self.port,
            database: database.to_string(),
            user,
            password,
        }
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        #[cfg(feature = "embedded-postgres")]
        if let Err(err) = self.cluster.stop() {
            tracing::warn!("the test-regression database cluster did not stop: {err}");
        }
    }
}

/// A password of this run only: the cluster is new and removed after it.
fn password() -> String {
    use sha2::Digest as _;
    let seed = format!(
        "{}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default(),
        std::thread::current().id()
    );
    sha2::Sha256::digest(seed.as_bytes())
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A table's rows by key: the primary key's values joined with `|`, or for a table with no primary key the row's
/// JSON (with `#2`, `#3`... for equal rows).
pub(crate) type Rows = BTreeMap<String, Value>;

/// Every table's rows, by table name (`schema.table` outside `public`).
pub(crate) type Snapshot = BTreeMap<String, Rows>;

const TABLES_SQL: &str = "SELECT n.nspname AS schema_name, c.relname AS table_name, \
     COALESCE((SELECT string_agg(a.attname, ',' ORDER BY array_position(i.indkey::int2[], a.attnum)) \
        FROM pg_index i JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
        WHERE i.indrelid = c.oid AND i.indisprimary), '') AS pk_columns \
     FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
     WHERE c.relkind IN ('r', 'p') AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
       AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%' \
     ORDER BY 1, 2";

/// Read every row of every table of the database.
pub(crate) fn snapshot(driver: &dyn DbDriver) -> Result<Snapshot, Error> {
    let mut conn = driver.connect()?;
    let tables = conn.query_once(TABLES_SQL, &[])?;
    let mut out = Snapshot::new();
    for table in tables {
        let text = |name: &str| table.get(name).and_then(|value| value.as_str()).unwrap_or_default().to_string();
        let (schema, name, pk) = (text("schema_name"), text("table_name"), text("pk_columns"));
        let pk: Vec<String> = pk.split(',').filter(|column| !column.is_empty()).map(str::to_string).collect();
        let rows = table_rows(conn.as_mut(), &schema, &name, &pk)?;
        let key = if schema == "public" { name } else { format!("{schema}.{name}") };
        out.insert(key, rows);
    }
    Ok(out)
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn table_rows(conn: &mut dyn DbConnection, schema: &str, table: &str, pk: &[String]) -> Result<Rows, Error> {
    let sql = format!("SELECT row_to_json(t)::text AS row_json FROM {}.{} t", quote(schema), quote(table));
    let mut rows = Rows::new();
    for row in conn.query_once(&sql, &[])? {
        let text = row.get("row_json").and_then(|value| value.as_str()).unwrap_or("null");
        let value: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let base = row_key(&value, pk);
        let mut key = base.clone();
        let mut n = 1;
        while rows.contains_key(&key) {
            n += 1;
            key = format!("{base}#{n}");
        }
        rows.insert(key, value);
    }
    Ok(rows)
}

/// The key of a row: its primary key's values, or with none, the whole row.
pub(crate) fn row_key(row: &Value, pk: &[String]) -> String {
    if pk.is_empty() {
        return row.to_string();
    }
    pk.iter()
        .map(|column| match row.get(column) {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => "∅".to_string(),
            Some(other) => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// One row that a case added, changed or removed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RowChange {
    pub table: String,
    pub key: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
}

impl RowChange {
    pub(crate) fn kind(&self) -> &'static str {
        match (&self.before, &self.after) {
            (None, Some(_)) => "insert",
            (Some(_), None) => "delete",
            _ => "update",
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        serde_json::json!({
            "table": self.table, "key": self.key, "kind": self.kind(), "before": self.before, "after": self.after,
        })
    }
}

/// The rows that differ from `before` to `after`, by table and key.
pub(crate) fn diff(before: &Snapshot, after: &Snapshot) -> Vec<RowChange> {
    let empty = Rows::new();
    let mut tables: Vec<&String> = before.keys().chain(after.keys()).collect();
    tables.sort();
    tables.dedup();
    let mut out = Vec::new();
    for table in tables {
        let old = before.get(table).unwrap_or(&empty);
        let new = after.get(table).unwrap_or(&empty);
        let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
        keys.sort();
        keys.dedup();
        for key in keys {
            let (a, b) = (old.get(key), new.get(key));
            if a != b {
                out.push(RowChange {
                    table: table.clone(),
                    key: key.clone(),
                    before: a.cloned(),
                    after: b.cloned(),
                });
            }
        }
    }
    out
}
