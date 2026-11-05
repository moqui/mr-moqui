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

//! A database's settings, read only (plans/Tools-Database.md §4.4): Postgres `pg_settings`, MySQL and MariaDB
//! `SHOW VARIABLES`. On the embedded cluster four of them come from the transactional datasource's conf
//! (`shared-buffers`, `work-mem`, `maintenance-work-mem`, `effective-cache-size`): those rows name their attribute,
//! the conf's value, and whether the running value differs, which a restart of the cluster applies. Nothing here
//! writes the conf or the server.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use moqui_core::entity::datasource::EmbeddedSettings;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

use crate::group_db::{GroupDb, Server};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_database_settings)?;
    Ok(())
}

const POSTGRES_SETTINGS: &str = "SELECT name AS name, setting AS setting, unit AS unit, category AS category, \
     short_desc AS short_desc, source AS source, pending_restart AS pending_restart, context AS context \
     FROM pg_settings ORDER BY name";
const MYSQL_VARIABLES: &str = "SHOW VARIABLES";
/// The bytes of a `pg_settings` row's value: `setting` times its `unit` (`8kB`, `kB`, `MB`, `B`).
fn running_bytes(row: &IndexMap<String, Value>) -> Option<u64> {
    let setting: u64 = row.get("setting").map(to_plain_string)?.trim().parse().ok()?;
    let unit = row.get("unit").map(to_plain_string).unwrap_or_default();
    let digits = unit.chars().take_while(char::is_ascii_digit).count();
    let count: u64 = if digits == 0 { 1 } else { unit[..digits].parse().ok()? };
    let per = moqui_core::entity::datasource::size_bytes(&format!("1{}", &unit[digits..]))?;
    setting.checked_mul(count)?.checked_mul(per)
}

/// What [`read`] answers.
#[derive(Clone, Debug, Default)]
pub struct SettingsOutcome {
    /// Each row: Postgres `name`, `setting`, `unit`, `category`, `short_desc`, `source`, `pending_restart`, `context`;
    /// MySQL and MariaDB `name` and `setting`. On the embedded cluster the conf's four also have `confAttribute`,
    /// `confValue` (null when the conf does not set it) and `differs`.
    pub settings: Vec<IndexMap<String, Value>>,
    pub embedded: bool,
}

/// The settings of `db` whose name holds `filter` (ignoring case; all when empty). `embedded` is the conf's settings
/// of the embedded cluster when `db` is that cluster.
///
/// # Errors
///
/// When the settings do not read.
pub fn read(db: &GroupDb, filter: &str, embedded: Option<&EmbeddedSettings>) -> Result<SettingsOutcome, Error> {
    let filter = filter.trim().to_ascii_lowercase();
    let keep = |name: &str| filter.is_empty() || name.to_ascii_lowercase().contains(&filter);
    let mut settings = Vec::new();
    if db.server == Server::Postgres {
        for row in db.query(POSTGRES_SETTINGS, &[])? {
            let name = row.get("name").map(to_plain_string).unwrap_or_default();
            if !keep(&name) {
                continue;
            }
            let mut row = row;
            if let Some(conf) = embedded {
                if let Some((attribute, _, value)) = conf.entries().into_iter().find(|(_, setting, _)| *setting == name)
                {
                    row.insert("confAttribute".to_string(), Value::string(attribute));
                    row.insert("confValue".to_string(), value.map_or(Value::Null, Value::string));
                    let differs = match value {
                        None => Value::Null,
                        Some(value) => match (moqui_core::entity::datasource::size_bytes(value), running_bytes(&row)) {
                            (Some(conf), Some(running)) => Value::Bool(conf != running),
                            _ => {
                                tracing::warn!(
                                    "Could not compare {name}: the conf has {value}, the server {:?} {:?}",
                                    row.get("setting"),
                                    row.get("unit")
                                );
                                Value::Null
                            }
                        },
                    };
                    row.insert("differs".to_string(), differs);
                }
            }
            settings.push(row);
        }
    } else {
        for row in db.query(MYSQL_VARIABLES, &[])? {
            let name = row.get("variable_name").map(to_plain_string).unwrap_or_default();
            if !keep(&name) {
                continue;
            }
            let mut out = IndexMap::new();
            out.insert("name".to_string(), Value::String(name));
            out.insert("setting".to_string(), row.get("value").cloned().unwrap_or(Value::Null));
            settings.push(out);
        }
    }
    Ok(SettingsOutcome {
        settings,
        embedded: embedded.is_some(),
    })
}

/// The conf's settings of the embedded cluster: the transactional datasource's, which the cluster starts with.
fn embedded_settings() -> Option<EmbeddedSettings> {
    let text = moqui_core::context::ExecutionContext::with_current(|ec| ec.factory().conf_text())?;
    let conf = moqui_lib::node::MNode::parse_shared("moqui-conf", &text).ok()?;
    moqui_core::entity::datasource::datasource_conf(&conf, "transactional")
        .ok()
        .map(|ds| ds.embedded_settings)
}

/// `get#DatabaseSettings`.
fn get_database_settings(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, Error> {
    if !crate::service::may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let Some(db) = GroupDb::of(parameters, facade) else {
        return Ok(BTreeMap::new());
    };
    let embedded = (!db.external && crate::service::group_is_embedded(&db.group))
        .then(embedded_settings)
        .flatten();
    match read(&db, &crate::service::text(parameters, "filter"), embedded.as_ref()) {
        Ok(outcome) => {
            let mut out = BTreeMap::new();
            out.insert(
                "settings".to_string(),
                Value::List(outcome.settings.into_iter().map(Value::Map).collect()),
            );
            out.insert("embedded".to_string(), Value::Bool(outcome.embedded));
            out.insert(
                "note".to_string(),
                if outcome.embedded {
                    Value::string(
                        "shared_buffers, work_mem, maintenance_work_mem and effective_cache_size come from the \
                         transactional datasource's conf; a restart of the server applies a change of them",
                    )
                } else {
                    Value::string("The settings are read only here")
                },
            );
            Ok(out)
        }
        Err(err) => {
            tracing::info!(error = %err, "The settings of {} do not read", db.db_text());
            crate::service::add_error("The settings could not be read (the server log has the detail)");
            Ok(BTreeMap::new())
        }
    }
}
