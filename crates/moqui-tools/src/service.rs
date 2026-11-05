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

//! `org.moqui.impl.DatabaseServices`, the Rust services of the Tools database screens.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::datasource::{datasource_conf, DatasourceKind};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

use crate::{capabilities, DbRef, NOT_AVAILABLE};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_capabilities)?;
    rust_service!(map, reset_query_stats)?;
    // ---- SQL (plans/Tools-Database.md §8): crate::sql_service ----
    crate::sql_service::register(map)?;
    Ok(())
}

/// The Tools app's root screen: its own authorization (`TOOLS_APP`) decides who may use these services, whatever
/// screen or service calls them.
pub(crate) const TOOLS_SCREEN: &str = "component://tools/screen/Tools.xml";

/// Whether the current user may view the Tools app; when not, an error is added.
pub(crate) fn may_view_tools() -> bool {
    match moqui_core::user::authorize_own("AT_XML_SCREEN", TOOLS_SCREEN, "view") {
        Ok(()) => true,
        Err(err) => {
            tracing::info!(error = %err, "Refused a Tools database service");
            add_error("Not authorized for the Tools app");
            false
        }
    }
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

pub(crate) fn add_error(message: impl Into<String>) {
    let message = message.into();
    ExecutionContext::with_current(|ec| ec.message().add_error(message));
}

/// Whether `group`'s database is on the embedded cluster this process runs, from the merged conf: its own datasource,
/// else the transactional one, whose database a group without a datasource shares.
pub(crate) fn group_is_embedded(group: &str) -> bool {
    let Some(text) = ExecutionContext::with_current(|ec| ec.factory().conf_text()) else {
        return false;
    };
    let conf = match moqui_lib::node::MNode::parse_shared("moqui-conf", &text) {
        Ok(conf) => conf,
        Err(err) => {
            tracing::warn!(error = %err, "Could not read the conf to tell whether group {group} is embedded");
            return false;
        }
    };
    datasource_conf(&conf, group)
        .or_else(|_| datasource_conf(&conf, "transactional"))
        .is_ok_and(|ds| ds.kind == DatasourceKind::Embedded)
}

/// `get#Capabilities`: what the database `db` allows the Tools screens and why the rest is not available, probed once
/// and kept five minutes (`refresh` probes again). Out: see [`capabilities::Capabilities::to_out`].
fn get_capabilities(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_view_tools() {
        return Ok(BTreeMap::new());
    }
    let db = match DbRef::parse(&text(parameters, "db"), facade.default_group()) {
        Ok(db) => db,
        Err(message) => {
            add_error(message);
            return Ok(BTreeMap::new());
        }
    };
    match db {
        DbRef::External(_) => {
            // external connections come with §15 step 7; until then every one is refused as on production
            add_error(NOT_AVAILABLE);
            Ok(BTreeMap::new())
        }
        DbRef::Group(group) => {
            if !facade.group_names().contains(&group) {
                add_error(format!("no entity group named {group}"));
                return Ok(BTreeMap::new());
            }
            let caps = capabilities::group_capabilities(
                facade,
                &group,
                || group_is_embedded(&group),
                flag(parameters, "refresh"),
            );
            Ok(caps.to_out())
        }
    }
}

/// `reset#QueryStats`: forget the entity facade's statement stats of this instance (`ec.entity.clearQueryStats()`),
/// with the `DATABASE_ADMIN` permission, and write a `DbaStatsReset` audit row.
fn reset_query_stats(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    if !may_view_tools() {
        return Ok(BTreeMap::new());
    }
    if !moqui_core::user::has_permission("DATABASE_ADMIN").unwrap_or(false) {
        add_error("Resetting statement stats needs the DATABASE_ADMIN permission");
        return Ok(BTreeMap::new());
    }
    moqui_core::entity::query_stats::clear();
    crate::audit::record(
        facade,
        &crate::audit::AuditEntry {
            action: "DbaStatsReset",
            is_change: true,
            detail_json: Some(r#"{"stats":"entity-facade"}"#.to_string()),
            ..crate::audit::AuditEntry::default()
        },
    );
    Ok(BTreeMap::new())
}
