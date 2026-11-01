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

//! `org.moqui.impl.EdgeServices` (plans/Edge-Node.md §5.7): the edge link for the System app's Edge Node and Edge
//! Clients screens, through the host's [`crate::edge::EdgeLink`]. In a host without the `edge` feature each call fails
//! with [`crate::edge::NOT_BUILT`], except `get#LinkStatus`, which answers `state` notBuilt (or notStarted
//! in a build with the link that no HTTP listener started) so a screen can say so.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::edge::{EdgeClientInfo, EdgeHostInfo, EdgeInfo, HOST_PASSTHROUGH, HOST_TERMINATE};
use crate::entity::EntityFacade;
use crate::user::stamp_value;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_link_status)?;
    rust_service!(map, check_edge_node)?;
    rust_service!(map, pair_edge_node)?;
    rust_service!(map, unpair_edge_node)?;
    rust_service!(map, create_pairing_code)?;
    rust_service!(map, get_edge_clients)?;
    rust_service!(map, revoke_edge_client)?;
    rust_service!(map, ping_edge_client)?;
    rust_service!(map, get_edge_hosts)?;
    rust_service!(map, add_edge_host)?;
    rust_service!(map, remove_edge_host)?;
    Ok(())
}

type Out = Result<BTreeMap<String, Value>, LibError>;

fn text(parameters: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    parameters
        .get(name)
        .filter(|value| !is_empty(value))
        .map(|value| to_plain_string(value).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn required(parameters: &BTreeMap<String, Value>, name: &str) -> Result<String, LibError> {
    text(parameters, name).ok_or_else(|| LibError::new(format!("{name} is required")))
}

fn strings(items: &[String]) -> Value {
    Value::List(items.iter().map(Value::string).collect())
}

fn put_edge(out: &mut BTreeMap<String, Value>, edge: &EdgeInfo) {
    out.insert("edgeName".to_string(), Value::string(&edge.name));
    out.insert("fingerprint".to_string(), Value::string(&edge.fingerprint));
    out.insert("binaryVersion".to_string(), Value::string(&edge.binary_version));
    out.insert("linkVersion".to_string(), Value::I64(i64::from(edge.link_version)));
    out.insert("features".to_string(), strings(&edge.features));
    out.insert("capabilities".to_string(), strings(&edge.capabilities));
    out.insert("publicNames".to_string(), strings(&edge.public_names));
    out.insert("compatible".to_string(), Value::Bool(edge.compatible));
    out.insert("message".to_string(), Value::string(&edge.message));
}

fn millis(value: Option<i64>) -> Value {
    value.map_or(Value::Null, stamp_value)
}

fn generation(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `get#LinkStatus`: the local node's link.
fn get_link_status(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let mut out = BTreeMap::new();
    out.insert("nodeRole".to_string(), Value::string(crate::edge::node_role()));
    let Ok(link) = crate::edge::link() else {
        let (state, error) = unlinked(crate::build_info::has_feature("edge"));
        out.insert("state".to_string(), Value::string(state));
        out.insert("lastError".to_string(), Value::string(error));
        return Ok(out);
    };
    let status = link.status();
    out.insert("state".to_string(), Value::string(status.state.as_str()));
    out.insert("url".to_string(), Value::string(&status.url));
    out.insert("generation".to_string(), generation(status.generation));
    out.insert("rttMs".to_string(), status.rtt_ms.map_or(Value::Null, Value::F64));
    out.insert("lastConnectDate".to_string(), millis(status.last_connect));
    out.insert("lastError".to_string(), Value::string(&status.last_error));
    out.insert("reconnects".to_string(), generation(status.reconnects));
    out.insert("linkFingerprint".to_string(), Value::string(&status.link_fingerprint));
    put_edge(&mut out, &status.peer);
    Ok(out)
}

/// The state and error of `get#LinkStatus` with no link installed: `notBuilt` when this build has no edge link,
/// `notStarted` when it has one but no HTTP listener started it (`moqui test`, a load).
fn unlinked(built: bool) -> (&'static str, &'static str) {
    if built {
        (
            "notStarted",
            "the edge link is built into this host, but no HTTP listener has started it",
        )
    } else {
        ("notBuilt", crate::edge::NOT_BUILT)
    }
}

/// `check#EdgeNode`: the edge node at `url`, before pairing.
fn check_edge_node(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let url = required(parameters, "url")?;
    let edge = crate::edge::link()?.check(&url)?;
    let mut out = BTreeMap::new();
    put_edge(&mut out, &edge);
    Ok(out)
}

/// `pair#EdgeNode`: pair with the edge node at `url` using its one-use `code`.
fn pair_edge_node(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let url = required(parameters, "url")?;
    let code = required(parameters, "code")?;
    let expect = text(parameters, "expectFingerprint");
    let paired = crate::edge::link()?.pair(&url, &code, expect.as_deref())?;
    let mut out = BTreeMap::new();
    put_edge(&mut out, &paired.edge);
    out.insert("keyFingerprint".to_string(), Value::string(&paired.key_fingerprint));
    Ok(out)
}

/// `unpair#EdgeNode`: revoke this node's key on the edge node and forget the link.
fn unpair_edge_node(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    crate::edge::link()?.unpair()?;
    Ok(BTreeMap::new())
}

/// `create#PairingCode`: on an edge node, a one-use code good for `validMinutes` (60 by default, at most a day).
fn create_pairing_code(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let minutes = match text(parameters, "validMinutes") {
        Some(raw) => raw
            .parse::<u32>()
            .ok()
            .filter(|minutes| (1..=1440).contains(minutes))
            .ok_or_else(|| LibError::new("validMinutes must be a whole number from 1 to 1440"))?,
        None => 60,
    };
    let code = crate::edge::link()?.create_pairing_code(minutes)?;
    Ok(BTreeMap::from([
        ("pairingCodeId".to_string(), Value::string(&code.pairing_code_id)),
        ("code".to_string(), Value::string(&code.code)),
        ("expireDate".to_string(), stamp_value(code.expire_date)),
    ]))
}

fn client_value(client: &EdgeClientInfo) -> Value {
    let mut row = indexmap::IndexMap::new();
    row.insert("pairedNodeId".to_string(), Value::string(&client.paired_node_id));
    row.insert("keyFingerprint".to_string(), Value::string(&client.key_fingerprint));
    row.insert("statusId".to_string(), Value::string(&client.status_id));
    row.insert("linked".to_string(), Value::Bool(client.linked));
    row.insert("down".to_string(), Value::Bool(client.down));
    row.insert("generation".to_string(), generation(client.generation));
    row.insert("lastConnectDate".to_string(), millis(client.last_connect));
    row.insert("remoteAddress".to_string(), Value::string(&client.remote_address));
    row.insert("binaryVersion".to_string(), Value::string(&client.binary_version));
    row.insert("linkVersion".to_string(), Value::I64(i64::from(client.link_version)));
    Value::Map(row)
}

/// `get#EdgeClients`: on an edge node, the paired local nodes with their live links.
fn get_edge_clients(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let clients = crate::edge::link()?.clients()?;
    Ok(BTreeMap::from([(
        "clientList".to_string(),
        Value::List(clients.iter().map(client_value).collect()),
    )]))
}

/// `revoke#EdgeClient`: on an edge node, revoke a local node's key and close its link.
fn revoke_edge_client(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let paired_node_id = required(parameters, "pairedNodeId")?;
    let revoked = crate::edge::link()?.revoke(&paired_node_id)?;
    Ok(BTreeMap::from([("revoked".to_string(), Value::Bool(revoked))]))
}

/// `ping#EdgeClient`: on an edge node, a Ping down a local node's down connection.
fn ping_edge_client(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let paired_node_id = required(parameters, "pairedNodeId")?;
    let rtt = crate::edge::link()?.ping_client(&paired_node_id)?;
    Ok(BTreeMap::from([("rttMs".to_string(), Value::F64(rtt))]))
}

fn host_value(host: &EdgeHostInfo) -> Value {
    let mut row = indexmap::IndexMap::new();
    row.insert("hostName".to_string(), Value::string(&host.host_name));
    row.insert("pairedNodeId".to_string(), Value::string(&host.paired_node_id));
    row.insert("modeEnumId".to_string(), Value::string(&host.mode_enum_id));
    Value::Map(row)
}

/// `get#EdgeHosts`: on an edge node, the public host names of the paired nodes (of one with `pairedNodeId`).
fn get_edge_hosts(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let node = text(parameters, "pairedNodeId");
    let hosts = crate::edge::link()?.hosts(node.as_deref())?;
    Ok(BTreeMap::from([(
        "hostList".to_string(),
        Value::List(hosts.iter().map(host_value).collect()),
    )]))
}

/// `add#EdgeHost`: on an edge node, a public host name for a paired node, passthrough unless `modeEnumId` says
/// terminate; for a name the node has already, its mode changes.
fn add_edge_host(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let mode = text(parameters, "modeEnumId").unwrap_or_else(|| HOST_PASSTHROUGH.to_string());
    if mode != HOST_PASSTHROUGH && mode != HOST_TERMINATE {
        return Err(LibError::new(format!(
            "modeEnumId must be {HOST_PASSTHROUGH} or {HOST_TERMINATE}"
        )));
    }
    let host = EdgeHostInfo {
        host_name: required(parameters, "hostName")?,
        paired_node_id: required(parameters, "pairedNodeId")?,
        mode_enum_id: mode,
    };
    let saved = crate::edge::link()?.add_host(&host)?;
    Ok(BTreeMap::from([
        ("hostName".to_string(), Value::string(&saved.host_name)),
        ("modeEnumId".to_string(), Value::string(&saved.mode_enum_id)),
    ]))
}

/// `remove#EdgeHost`: on an edge node, remove a public host name.
fn remove_edge_host(parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let host_name = required(parameters, "hostName")?;
    let removed = crate::edge::link()?.remove_host(&host_name)?;
    Ok(BTreeMap::from([("removed".to_string(), Value::Bool(removed))]))
}

#[cfg(test)]
#[path = "edge_tests.rs"]
mod tests;
