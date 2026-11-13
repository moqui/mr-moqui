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

//! The public host names an edge node answers for its paired local nodes (plans/archive/Edge-Node.md §5.3):
//! `moqui.edge.EdgeHost` rows, and the table in memory the listener reads for each connection and request. Only a
//! name in the table goes down a link, and only to its own node, so the edge node is no open proxy.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::{EdgeHostInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::{read, write};

use super::store;

const HOST: &str = "moqui.edge.EdgeHost";
const CLIENT: &str = "moqui.edge.EdgeClient";

/// How the edge node handles a host's connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The TLS goes down the link unopened; the local node ends it.
    Passthrough,
    /// The edge node ends the TLS and forwards each request.
    Terminate,
}

impl Mode {
    fn from_enum(id: &str) -> Option<Self> {
        match id {
            HOST_PASSTHROUGH => Some(Self::Passthrough),
            HOST_TERMINATE => Some(Self::Terminate),
            _ => None,
        }
    }
}

/// A host's node and mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub node: String,
    pub mode: Mode,
}

/// The host names by their normal form ([`normalize`]), with the number of the load that wrote them.
#[derive(Default)]
pub(crate) struct Hosts {
    table: RwLock<(u64, HashMap<String, Route>)>,
    loads: AtomicU64,
}

impl Hosts {
    /// The route of `name` (a `Host` header or a TLS server name, normalized here).
    pub(crate) fn route(&self, name: &str) -> Option<Route> {
        let name = normalize(name)?;
        read(&self.table).1.get(&name).cloned()
    }

    /// Replace the table with the rows, unless a load that began later has written it already.
    pub(crate) fn load(&self, factory: &ExecutionContextFactory) -> Result<usize, Error> {
        let load = self.loads.fetch_add(1, Ordering::AcqRel) + 1;
        let rows = list(factory, None)?;
        let mut table = HashMap::with_capacity(rows.len());
        for row in rows {
            let Some(name) = normalize(&row.host_name) else {
                tracing::error!("Edge host {} is not a host name; it is left out", row.host_name);
                continue;
            };
            match Mode::from_enum(&row.mode_enum_id) {
                Some(mode) => {
                    table.insert(
                        name,
                        Route {
                            node: row.paired_node_id,
                            mode,
                        },
                    );
                }
                None => tracing::error!(
                    "Edge host {} has mode {}, which is not passthrough or terminate; it is left out",
                    row.host_name,
                    row.mode_enum_id
                ),
            }
        }
        let count = table.len();
        let mut current = write(&self.table);
        if current.0 < load {
            *current = (load, table);
        }
        Ok(count)
    }
}

/// A host name's normal form: trimmed, lower case, with no port or trailing dot, or `None` when it is not a DNS name
/// (letters, digits, `-` and `.`, at most 253 bytes; an IP literal in brackets is not one).
pub(crate) fn normalize(name: &str) -> Option<String> {
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

fn text(value: Option<&Value>) -> String {
    value.map(to_plain_string).unwrap_or_default()
}

/// The rows, of one node when `node` is given, by name.
pub(crate) fn list(factory: &ExecutionContextFactory, node: Option<&str>) -> Result<Vec<EdgeHostInfo>, Error> {
    let facade = store::facade(factory)?;
    store::transaction(&facade, HOST, || {
        let mut find = facade.find(HOST)?;
        if let Some(node) = node {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        let rows = find.order_by("hostName", false).list()?;
        Ok(rows
            .iter()
            .map(|row| EdgeHostInfo {
                host_name: text(row.get("hostName")),
                paired_node_id: text(row.get("pairedNodeId")),
                mode_enum_id: text(row.get("modeEnumId")),
            })
            .collect())
    })
}

/// Save `host` for its node: a new name, or a new mode for a name the node has. A name of another node, a node that is
/// not paired or was revoked, and a name that is not a DNS name are refused. It commits in a transaction of its own,
/// so the table in memory, loaded again after it, never holds a change a caller's transaction rolls back.
pub(crate) fn add(factory: &ExecutionContextFactory, host: &EdgeHostInfo) -> Result<EdgeHostInfo, Error> {
    let name = normalize(&host.host_name)
        .ok_or_else(|| Error::new(format!("{} is not a host name", host.host_name.trim())))?;
    if Mode::from_enum(&host.mode_enum_id).is_none() {
        return Err(Error::new(format!(
            "the mode must be {HOST_PASSTHROUGH} or {HOST_TERMINATE}"
        )));
    }
    let facade = store::facade(factory)?;
    store::committed(&facade, HOST, || {
        let node = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(&host.paired_node_id))
            .one()?;
        let active = node
            .as_ref()
            .is_some_and(|node| text(node.get("statusId")) == store::ACTIVE);
        if !active {
            return Err(Error::new(format!(
                "no local node {} is paired with this edge node, or it was revoked",
                host.paired_node_id
            )));
        }
        let existing = facade
            .find(HOST)?
            .eq("hostName", Value::string(&name))
            .for_update(true)
            .one()?;
        if let Some(existing) = &existing {
            let owner = text(existing.get("pairedNodeId"));
            if owner != host.paired_node_id {
                return Err(Error::new(format!("{name} is a host of local node {owner}")));
            }
        }
        let mut row = facade.make_value(HOST)?;
        row.set("hostName", Value::string(&name));
        row.set("pairedNodeId", Value::string(&host.paired_node_id));
        row.set("modeEnumId", Value::string(&host.mode_enum_id));
        if existing.is_none() {
            row.set("createdDate", moqui_core::user::now_value());
        }
        row.create_or_update(&facade)?;
        Ok(EdgeHostInfo {
            host_name: name.clone(),
            paired_node_id: host.paired_node_id.clone(),
            mode_enum_id: host.mode_enum_id.clone(),
        })
    })
}

/// Remove a name; false when there was none.
pub(crate) fn remove(factory: &ExecutionContextFactory, host_name: &str) -> Result<bool, Error> {
    let Some(name) = normalize(host_name) else {
        return Ok(false);
    };
    let facade = store::facade(factory)?;
    store::committed(&facade, HOST, || {
        if facade.find(HOST)?.eq("hostName", Value::string(&name)).one()?.is_none() {
            return Ok(false);
        }
        let mut row = facade.make_value(HOST)?;
        row.set("hostName", Value::string(&name));
        row.delete(&facade)?;
        Ok(true)
    })
}

#[cfg(test)]
#[path = "hosts_tests.rs"]
mod tests;
