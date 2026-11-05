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

//! What the schema, job, table and activity services share (plans/Tools-Database.md §5–§7): the entity group's
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

/// An entity group's database.
pub(crate) struct GroupDb {
    pub(crate) group: String,
    pub(crate) tx: TransactionManager,
    pub(crate) dialect: moqui_db::dialect::Dialect,
    pub(crate) server: Server,
}

impl GroupDb {
    /// The group `db` names. `ext:` is refused with [`NOT_AVAILABLE`] until external connections are built (§4.3,
    /// §15 step 7). A refusal adds the error and is `None`.
    pub(crate) fn of(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Option<Self> {
        let db = match DbRef::parse(&text(parameters, "db"), facade.default_group()) {
            Ok(db) => db,
            Err(message) => {
                add_error(message);
                return None;
            }
        };
        match db {
            DbRef::External(_) => {
                add_error(NOT_AVAILABLE);
                None
            }
            DbRef::Group(group) => {
                let found = Self::named(facade, &group);
                if found.is_none() {
                    add_error(format!("no entity group named {group}"));
                }
                found
            }
        }
    }

    /// The group named `group`, or `None` when the facade has none or its database is not Postgres or MySQL.
    pub(crate) fn named(facade: &EntityFacade, group: &str) -> Option<Self> {
        let (dialect, tx) = facade
            .groups()
            .find(|(name, _, _)| *name == group)
            .map(|(_, dialect, tx)| (dialect.clone(), tx.clone()))?;
        let server = if moqui_db::sql::is_mysql(&dialect) {
            if is_mariadb(group, &tx) {
                Server::Mariadb
            } else {
                Server::Mysql
            }
        } else {
            Server::Postgres
        };
        Some(Self {
            group: group.to_string(),
            tx,
            dialect,
            server,
        })
    }

    /// `group:<name>`, as the rows keep it.
    pub(crate) fn db_text(&self) -> String {
        format!("group:{}", self.group)
    }

    pub(crate) fn is_postgres(&self) -> bool {
        self.server == Server::Postgres
    }

    /// Run one query with `params` (`?` placeholders) in a transaction of its own, the caller's suspended, so a failed
    /// query cannot abort the caller's transaction, within [`crate::catalog::QUERY_TIMEOUT`].
    pub(crate) fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<IndexMap<String, Value>>, Error> {
        let rows = self.tx.run(
            TransactionAttribute::ForceNew,
            Some(crate::catalog::QUERY_TIMEOUT),
            || self.tx.with_connection(|conn| conn.query_once(sql, params)),
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
fn is_mariadb(group: &str, tx: &TransactionManager) -> bool {
    static KNOWN: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();
    let known = KNOWN.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(answer) = lock(known).get(group) {
        return *answer;
    }
    let answer = tx
        .run(TransactionAttribute::IgnoreSuspend, None, || {
            tx.with_connection(|conn| conn.query_once("SELECT VERSION() AS version", &[]))
        })
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|row| row.columns.into_iter().next())
        .is_some_and(|(_, value)| to_plain_string(&value).contains("MariaDB"));
    lock(known).insert(group.to_string(), answer);
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
