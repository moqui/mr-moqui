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

//! What the schema, job, table and activity services share (plans/archive/Tools-Database.md §5–§7): the entity group's
//! database a service's `db` names ([`GroupDb`]), its queries with parameters, and reading the services' parameters.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::{TransactionAttribute, TransactionManager};

use crate::{DbRef, NOT_AVAILABLE};

/// Out parameters.
pub(crate) type Out = BTreeMap<String, Value>;

/// The kind of server, as the statements differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Server {
    Postgres,
    Mysql,
    Mariadb,
}

/// The database a service's `db` names: an entity group's (`group:<name>`), or an external connection's
/// (`ext:<connectionId>`, plans/archive/Tools-Database.md §4.3), reached only through [`crate::ext::gate`].
#[derive(Clone)]
pub struct GroupDb {
    /// The entity group; for an external connection `ext:<connectionId>`, which no entity names.
    pub group: String,
    pub tx: TransactionManager,
    pub dialect: moqui_db::dialect::Dialect,
    pub server: Server,
    /// An external connection.
    pub external: bool,
    /// An external connection marked `readOnly`: every statement in a read-only transaction.
    pub read_only: bool,
}

impl GroupDb {
    /// The database `db` names, an external one included (its gate first). A refusal adds the error and is `None`:
    /// for an external one the one vague [`NOT_AVAILABLE`].
    pub(crate) fn of(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Self> {
        match Self::resolve(facade, &text(parameters, "db")) {
            Ok(db) => Some(db),
            Err(message) => {
                add_error(message);
                None
            }
        }
    }

    /// [`Self::of`] for what is offered on the entity groups' databases only (schema, jobs, sessions' cancel, table
    /// maintenance): an external connection is refused.
    pub(crate) fn of_group(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Self> {
        let db = Self::of(parameters, facade)?;
        if db.external {
            add_error("This is not offered on an external database");
            return None;
        }
        Some(db)
    }

    /// The database of the `db` text: empty is the default group. An external connection passes
    /// [`crate::ext::gate`] first, and every refusal of one is the vague [`NOT_AVAILABLE`].
    ///
    /// # Errors
    ///
    /// The refusal's message.
    pub fn resolve(facade: &EntityFacade, db: &str) -> Result<Self, String> {
        match DbRef::parse(db, facade.default_group())? {
            DbRef::External(id) => {
                if !crate::ext::gate() {
                    return Err(NOT_AVAILABLE.to_string());
                }
                crate::ext::open(facade, &id).map_err(|err| {
                    tracing::info!(error = %err, "External database {id} is not available");
                    NOT_AVAILABLE.to_string()
                })
            }
            DbRef::Group(group) => Self::named(facade, &group).ok_or_else(|| format!("no entity group named {group}")),
        }
    }

    /// The SQL syntax of the database.
    pub fn syntax(&self) -> moqui_core::entity::exec::SqlSyntax {
        moqui_core::entity::exec::SqlSyntax::of(&self.dialect)
    }

    /// MySQL or MariaDB.
    pub fn is_mysql(&self) -> bool {
        self.server != Server::Postgres
    }

    /// The group named `group`, or `None` when the facade has none or its database is not Postgres or MySQL.
    pub(crate) fn named(facade: &EntityFacade, group: &str) -> Option<Self> {
        let (dialect, tx) = facade
            .groups()
            .find(|(name, _, _)| *name == group)
            .map(|(_, dialect, tx)| (dialect.clone(), tx.clone()))?;
        Some(Self::on(group, tx, dialect, false, false))
    }

    /// The database of `tx` and `dialect` under the name `group`.
    pub(crate) fn on(
        group: &str,
        tx: TransactionManager,
        dialect: moqui_db::dialect::Dialect,
        external: bool,
        read_only: bool,
    ) -> Self {
        let server = if moqui_db::sql::is_mysql(&dialect) {
            if is_mariadb(group, &tx) {
                Server::Mariadb
            } else {
                Server::Mysql
            }
        } else {
            Server::Postgres
        };
        Self {
            group: group.to_string(),
            tx,
            dialect,
            server,
            external,
            read_only,
        }
    }

    /// `group:<name>` or `ext:<connectionId>`, as the services take it and the rows keep it.
    pub fn db_text(&self) -> String {
        if self.external {
            self.group.clone()
        } else {
            format!("group:{}", self.group)
        }
    }

    pub(crate) fn is_postgres(&self) -> bool {
        self.server == Server::Postgres
    }

    /// Run one query with `params` (`?` placeholders) in a transaction of its own, the caller's suspended, so a failed
    /// query cannot abort the caller's transaction, within [`crate::catalog::QUERY_TIMEOUT`].
    pub fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<IndexMap<String, Value>>, Error> {
        let rows = self.tx.run(
            TransactionAttribute::ForceNew,
            Some(crate::catalog::QUERY_TIMEOUT),
            || {
                if self.read_only {
                    self.tx.with_connection(|conn| conn.set_transaction_read_only())?;
                }
                self.tx.with_connection(|conn| conn.query_once(sql, params))
            },
        )?;
        Ok(rows
            .into_iter()
            .map(|row| {
                row.columns
                    .into_iter()
                    .map(|(name, value)| (name.to_ascii_lowercase(), value))
                    .collect()
            })
            .collect())
    }

    /// `body` on a connection of its own, outside any transaction (the caller's is suspended), closed after it rather
    /// than returned to the pool ([`TransactionManager::with_connection_closed_after`] with no frame active): a
    /// database job's statement, which may run long, must not be in a transaction (`CREATE INDEX CONCURRENTLY`) and
    /// leaves its session settings behind.
    pub(crate) fn own_connection<T>(
        &self,
        body: impl FnOnce(&mut dyn moqui_core::driver::DbConnection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.tx.run(TransactionAttribute::IgnoreSuspend, None, || {
            self.tx.with_connection_closed_after(body)
        })
    }

    /// [`Self::query`], an error logged and the rows empty, with the reason in `reasons` under `what`.
    pub(crate) fn query_or_note(
        &self,
        sql: &str,
        params: &[Value],
        what: &str,
        reasons: &mut IndexMap<String, Value>,
    ) -> Vec<IndexMap<String, Value>> {
        match self.query(sql, params) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::info!(error = %err, group = %self.group, "Could not read {what}");
                reasons.insert(what.to_string(), Value::string(err.to_string()));
                Vec::new()
            }
        }
    }
}

/// Whether the MySQL-dialect group's server is MariaDB, asked once per group.
fn known_servers() -> &'static Mutex<BTreeMap<String, bool>> {
    static KNOWN: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Forget what is known of the server of `group` (an external connection whose row changed or went).
pub(crate) fn forget_server(group: &str) {
    lock(known_servers()).remove(group);
}

/// Whether the MySQL-dialect group's server is MariaDB, asked once per group within the catalog deadline. A probe that
/// fails answers MySQL for now and is asked again next time.
fn is_mariadb(group: &str, tx: &TransactionManager) -> bool {
    if let Some(answer) = lock(known_servers()).get(group) {
        return *answer;
    }
    let probed = tx.run(
        TransactionAttribute::ForceNew,
        Some(crate::catalog::QUERY_TIMEOUT),
        || tx.with_connection(|conn| conn.query_once("SELECT VERSION() AS version", &[])),
    );
    let Ok(rows) = probed else {
        return false;
    };
    let answer = rows
        .into_iter()
        .next()
        .and_then(|row| row.columns.into_iter().next())
        .is_some_and(|(_, value)| to_plain_string(&value).contains("MariaDB"));
    lock(known_servers()).insert(group.to_string(), answer);
    answer
}

/// The rows as the services give them: a list of maps.
pub(crate) fn rows_value(rows: Vec<IndexMap<String, Value>>) -> Value {
    Value::List(rows.into_iter().map(Value::Map).collect())
}

pub(crate) fn text(parameters: &BTreeMap<String, Value>, key: &str) -> String {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

pub(crate) fn flag(parameters: &BTreeMap<String, Value>, key: &str) -> bool {
    match parameters.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) => matches!(to_plain_string(value).as_str(), "true" | "Y"),
        None => false,
    }
}

/// A whole number parameter; `None` when absent or not a number.
pub(crate) fn number(parameters: &BTreeMap<String, Value>, key: &str) -> Option<i64> {
    let value = parameters.get(key).filter(|value| !is_empty(value))?;
    match value {
        Value::I32(number) => Some(i64::from(*number)),
        Value::I64(number) => Some(*number),
        other => to_plain_string(other).trim().parse().ok(),
    }
}

/// A list parameter: a list, or text separated by commas.
pub(crate) fn list(parameters: &BTreeMap<String, Value>, key: &str) -> Vec<String> {
    match parameters.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        Some(value) if !is_empty(value) => to_plain_string(value)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

/// Whether the user has `DATABASE_ADMIN`; when not, an error naming `what` is added.
pub(crate) fn require_admin(what: &str) -> bool {
    if moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false) {
        return true;
    }
    add_error(format!("{what} needs the DATABASE_ADMIN permission"));
    false
}

/// A cell as a whole number.
pub(crate) fn cell_number(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::I32(number) => Some(i64::from(*number)),
        Value::I64(number) => Some(*number),
        Value::Null => None,
        other => to_plain_string(other)
            .trim()
            .parse::<f64>()
            .ok()
            .map(|number| number as i64),
    }
}

/// A cell as text; empty for null.
pub(crate) fn cell_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Bytes(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Some(value) => to_plain_string(value),
        None => String::new(),
    }
}
