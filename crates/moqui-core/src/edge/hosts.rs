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

//! The rows of a node that takes links (plans/archive/Edge-Node.md §5.3, plans/Edge-Node2.md §4.10): the public host
//! names it answers for each pairing (`moqui.edge.EdgeHost`) and the pairings themselves (`moqui.edge.EdgeClient`), as
//! the System app's Edge Clients screens and a pairing's Configure read and write them. They need no link: the host's
//! edge link keeps a table of the host names in memory and loads it again when told
//! ([`super::EdgeLink::hosts_changed`]), and adds each pairing's live link to [`clients`].

use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::{EdgeClientInfo, EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

const HOST: &str = "moqui.edge.EdgeHost";
const CLIENT: &str = "moqui.edge.EdgeClient";
/// A pairing that may link.
pub const ACTIVE: &str = "EdgeClientActive";

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// `body` in a transaction on the edge rows (a new one with `own`, committed when it returns, for a change the caller
/// then acts on in memory), with authorization off: the edge node's own bookkeeping, which the services' callers are
/// authorized for.
fn transaction<T>(facade: &EntityFacade, own: bool, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let attribute = if own {
        TransactionAttribute::ForceNew
    } else {
        TransactionAttribute::UseOrBegin
    };
    facade
        .transactions_for(HOST)?
        .run(attribute, Some(Duration::from_secs(30)), body)
}

/// A field of a pairing's kept setup (`EdgeClient.setupJson`) as names: a string or a list of strings, trimmed, the
/// empty ones left out.
pub fn setup_list(setup: &serde_json::Value, field: &str) -> Vec<String> {
    let names: Vec<&str> = match &setup[field] {
        serde_json::Value::String(name) => vec![name.as_str()],
        serde_json::Value::Array(names) => names.iter().filter_map(serde_json::Value::as_str).collect(),
        _ => Vec::new(),
    };
    names
        .into_iter()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// A host name's normal form: trimmed, lower case, with no port or trailing dot, or `None` when it is not a DNS name
/// (letters, digits, `-` and `.`, at most 253 bytes; an IP literal in brackets is not one).
pub fn normalize(name: &str) -> Option<String> {
    let name = name.trim();
    let name = match name.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => host,
        _ => name,
    };
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    let valid = !name.is_empty()
        && name.len() <= 253
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
    valid.then_some(name)
}

/// The host name rows, of one pairing when `node` is given, by name.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn list(facade: &EntityFacade, node: Option<&str>) -> Result<Vec<EdgeHostInfo>, Error> {
    transaction(facade, false, || {
        let mut find = facade.find(HOST)?;
        if let Some(node) = node.map(str::trim).filter(|node| !node.is_empty()) {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        let rows = find.order_by("hostName", false).list()?;
        Ok(rows
            .iter()
            .map(|row| EdgeHostInfo {
                host_name: text(row, "hostName"),
                paired_node_id: text(row, "pairedNodeId"),
                mode_enum_id: text(row, "modeEnumId"),
            })
            .collect())
    })
}

/// Save `host` for its pairing: a new name, or a new mode for a name the pairing has. It commits in a transaction of
/// its own, so a table in memory loaded again after it never holds a change a caller's transaction rolls back.
///
/// # Errors
///
/// Refuses a name that is not a DNS name, a mode other than passthrough or terminate, a pairing that is not active,
/// and a name another pairing has, with the generic [`super::claims::HELD_BY_ANOTHER`] (the first claim holds,
/// plans/Edge-Node2.md §4.1; the Domains screen shows which pairing has it); returns the entity facade's error.
pub fn add(facade: &EntityFacade, host: &EdgeHostInfo) -> Result<EdgeHostInfo, Error> {
    let name = normalize(&host.host_name)
        .ok_or_else(|| Error::new(format!("{} is not a host name", host.host_name.trim())))?;
    if host.mode_enum_id != HOST_PASSTHROUGH && host.mode_enum_id != HOST_TERMINATE {
        return Err(Error::new(format!(
            "the mode must be {HOST_PASSTHROUGH} or {HOST_TERMINATE}"
        )));
    }
    let node = host.paired_node_id.trim();
    transaction(facade, true, || {
        let active = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .one()?
            .is_some_and(|row| text(&row, "statusId") == ACTIVE);
        if node.is_empty() || !active {
            return Err(Error::new(format!(
                "no local node {node} is paired with this edge node, or it was revoked"
            )));
        }
        let existing = facade
            .find(HOST)?
            .eq("hostName", Value::string(&name))
            .for_update(true)
            .one()?
            .map(|row| text(&row, "pairedNodeId"));
        save(facade, &name, node, &host.mode_enum_id, existing)?;
        Ok(EdgeHostInfo {
            host_name: name.clone(),
            paired_node_id: node.to_string(),
            mode_enum_id: host.mode_enum_id.clone(),
        })
    })
}

/// Write the row of `name` for `node`, given its holder as read (`existing`): a new name is inserted only if no row
/// has come since the read (a lock on a missing row holds nothing), else the holder is read again, so of two
/// concurrent claims of a new name the second is refused with [`super::claims::HELD_BY_ANOTHER`] rather than taking
/// it; the holder's own name gets its new mode.
fn save(facade: &EntityFacade, name: &str, node: &str, mode: &str, existing: Option<String>) -> Result<(), Error> {
    let held = || Error::new(format!("{name}: {}", super::claims::HELD_BY_ANOTHER));
    let mut row = facade.make_value(HOST)?;
    row.set("hostName", Value::string(name));
    row.set("pairedNodeId", Value::string(node));
    row.set("modeEnumId", Value::string(mode));
    let holder = match existing {
        Some(holder) => holder,
        None => {
            row.set("createdDate", crate::user::now_value());
            if row.create_if_missing(facade)? {
                return Ok(());
            }
            let Some(holder) = facade
                .find(HOST)?
                .eq("hostName", Value::string(name))
                .for_update(true)
                .one()?
                .map(|row| text(&row, "pairedNodeId"))
            else {
                return Err(Error::new(format!("{name} changed while it was saved; try again")));
            };
            holder
        }
    };
    if holder != node {
        return Err(held());
    }
    let mut update = facade.make_value(HOST)?;
    update.set("hostName", Value::string(name));
    update.set("modeEnumId", Value::string(mode));
    update.update(facade)?;
    Ok(())
}

/// Remove a name, in a transaction of its own; false when there was none.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn remove(facade: &EntityFacade, host_name: &str) -> Result<bool, Error> {
    let Some(name) = normalize(host_name) else {
        return Ok(false);
    };
    transaction(facade, true, || {
        if facade.find(HOST)?.eq("hostName", Value::string(&name)).one()?.is_none() {
            return Ok(false);
        }
        let mut row = facade.make_value(HOST)?;
        row.set("hostName", Value::string(&name));
        row.delete(facade)?;
        Ok(true)
    })
}

/// The pairings, by id, as their rows have them (no live link: the host's edge link adds it).
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn clients(facade: &EntityFacade) -> Result<Vec<EdgeClientInfo>, Error> {
    transaction(facade, false, || {
        let rows = facade.find(CLIENT)?.order_by("pairedNodeId", false).list()?;
        Ok(rows
            .iter()
            .map(|row| EdgeClientInfo {
                paired_node_id: text(row, "pairedNodeId"),
                key_fingerprint: text(row, "keyFingerprint"),
                status_id: text(row, "statusId"),
                linked: false,
                down: false,
                generation: text(row, "linkGeneration").parse().unwrap_or(0),
                last_connect: row.get("lastConnectDate").and_then(crate::user::value_millis),
                remote_address: text(row, "lastAddress"),
                binary_version: text(row, "binaryVersion"),
                link_version: text(row, "linkVersion").parse().unwrap_or(0),
                setup_json: text(row, "setupJson"),
                setup_date: row.get("setupDate").and_then(crate::user::value_millis),
                queue_max_megabytes: text(row, "queueMaxMegabytes")
                    .split('.')
                    .next()
                    .and_then(|number| number.parse::<i64>().ok())
                    .filter(|megabytes| *megabytes > 0),
            })
            .collect())
    })
}

/// Set a pairing's own size cap on the items waiting for it (`EdgeClient.queueMaxMegabytes`, plans/Edge-Node2.md
/// §4.1), or with `None` unset it, so the conf's `queue-max-megabytes` holds for it. The retention pass reads it.
///
/// # Errors
///
/// Refuses a cap of 0 or less (unset it instead) and a pairing with no row; returns the entity facade's error.
pub fn set_queue_cap(facade: &EntityFacade, node: &str, megabytes: Option<i64>) -> Result<(), Error> {
    if megabytes.is_some_and(|megabytes| megabytes <= 0) {
        return Err(Error::new(
            "the queue cap is a number of megabytes above 0; leave it empty for the conf's",
        ));
    }
    let node = node.trim();
    transaction(facade, false, || {
        if node.is_empty()
            || facade
                .find(CLIENT)?
                .eq("pairedNodeId", Value::string(node))
                .one()?
                .is_none()
        {
            return Err(Error::new(format!(
                "no local node {node} is paired with this edge node"
            )));
        }
        let mut row = facade.make_value(CLIENT)?;
        row.set("pairedNodeId", Value::string(node));
        row.set("queueMaxMegabytes", megabytes.map_or(Value::Null, Value::I64));
        row.update(facade)?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
