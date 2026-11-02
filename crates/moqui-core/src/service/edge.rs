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

//! `org.moqui.impl.EdgeServices` (plans/Edge-Node.md §5.2, §5.7): the edge link for the System app's Edge Node and
//! Edge Clients screens, through the host's [`crate::edge::EdgeLink`]. In a host without the `edge` feature each call
//! fails with [`crate::edge::NOT_BUILT`], except `get#LinkStatus`, which answers `state` notBuilt (or notStarted in a
//! build with the link that no HTTP listener started) so a screen can say so. The queue's and replication's services
//! (`enqueue#Item`, `get#QueueStatus`, `pause#QueueDelivery`, `resync#Replication`, `get#ReplicaValues`) work on the
//! rows and state in [`crate::edge`] and need no link.

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
    rust_service!(map, enqueue_item)?;
    rust_service!(map, get_queue_status)?;
    rust_service!(map, pause_queue_delivery)?;
    rust_service!(map, resync_replication)?;
    rust_service!(map, get_replica_values)?;
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

fn current_factory() -> Result<std::sync::Arc<crate::context::ExecutionContextFactory>, LibError> {
    crate::context::ExecutionContext::with_current(|ctx| std::sync::Arc::clone(ctx.factory()))
        .ok_or_else(|| LibError::new("no execution context"))
}

/// `enqueue#Item`: queue an item for the other end of the link; an idempotency key queued already returns its item.
fn enqueue_item(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let factory = current_factory()?;
    let item = crate::edge::queue::NewItem {
        kind: required(parameters, "kind")?,
        conversation_key: required(parameters, "conversationKey")?,
        idempotency_key: text(parameters, "idempotencyKey").unwrap_or_default(),
        payload: parameters
            .get("payload")
            .filter(|value| !is_empty(value))
            .map(to_plain_string),
        content_location: text(parameters, "contentLocation"),
    };
    let node = text(parameters, "pairedNodeId");
    let enqueued = crate::edge::queue::enqueue(&factory, facade, node.as_deref(), &item)?;
    Ok(BTreeMap::from([
        ("queueItemId".to_string(), Value::string(&enqueued.queue_item_id)),
        ("created".to_string(), Value::Bool(enqueued.created)),
    ]))
}

fn count(value: impl TryInto<i64>) -> Value {
    Value::I64(value.try_into().unwrap_or(i64::MAX))
}

/// `get#QueueStatus`: the queue's depth and oldest item by node, direction and kind; whether delivery is paused; on a
/// local node its replicated sets, on an edge node the cursors of its nodes' sets.
fn get_queue_status(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let node = text(parameters, "pairedNodeId");
    let now = crate::user::now_millis();
    let kinds = crate::edge::queue::stats(facade, node.as_deref())?;
    let kind_list = kinds
        .iter()
        .map(|kind| {
            let mut row = indexmap::IndexMap::new();
            row.insert("pairedNodeId".to_string(), Value::string(&kind.paired_node_id));
            row.insert("directionEnumId".to_string(), Value::string(&kind.direction));
            row.insert("kind".to_string(), Value::string(&kind.kind));
            row.insert("pending".to_string(), Value::I64(kind.pending));
            row.insert("failed".to_string(), Value::I64(kind.failed));
            row.insert("oldestDate".to_string(), millis(kind.oldest_pending));
            row.insert(
                "oldestAgeSeconds".to_string(),
                kind.oldest_pending
                    .map_or(Value::Null, |oldest| Value::I64((now - oldest).max(0) / 1000)),
            );
            Value::Map(row)
        })
        .collect();
    let set_list = crate::edge::replica::replica(facade)
        .map(|replica| replica.status())
        .unwrap_or_default()
        .iter()
        .map(|set| {
            let mut row = indexmap::IndexMap::new();
            row.insert("setName".to_string(), Value::string(&set.name));
            row.insert("epoch".to_string(), Value::string(&set.epoch));
            row.insert("acked".to_string(), count(set.acked));
            row.insert("waiting".to_string(), count(set.waiting));
            row.insert("snapshotNeeded".to_string(), Value::Bool(set.snapshot_needed));
            row.insert("lastAckDate".to_string(), millis(set.last_ack));
            row.insert("lastSnapshotDate".to_string(), millis(set.last_snapshot));
            Value::Map(row)
        })
        .collect();
    let cursor_list = crate::edge::replica::cursors(facade, node.as_deref())?
        .iter()
        .map(|cursor| {
            let mut row = indexmap::IndexMap::new();
            row.insert("pairedNodeId".to_string(), Value::string(&cursor.paired_node_id));
            row.insert("setName".to_string(), Value::string(&cursor.set_name));
            row.insert("epoch".to_string(), Value::string(&cursor.epoch));
            row.insert("sequenceNum".to_string(), count(cursor.seq));
            row.insert("values".to_string(), Value::I64(cursor.values));
            row.insert("snapshotDate".to_string(), millis(cursor.snapshot_date));
            row.insert("appliedDate".to_string(), millis(cursor.applied_date));
            Value::Map(row)
        })
        .collect();
    Ok(BTreeMap::from([
        ("kindList".to_string(), Value::List(kind_list)),
        ("paused".to_string(), Value::Bool(crate::edge::queue::paused(facade))),
        ("setList".to_string(), Value::List(set_list)),
        ("cursorList".to_string(), Value::List(cursor_list)),
    ]))
}

/// `pause#QueueDelivery`: pause (or resume) this node's queue delivery: deliveries to it are answered "paused" and
/// its own outbox waits.
fn pause_queue_delivery(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let paused = text(parameters, "paused").is_none_or(|flag| flag == "true" || flag == "Y");
    crate::edge::queue::set_paused(facade, paused);
    tracing::info!("Edge queue delivery {}", if paused { "paused" } else { "resumed" });
    Ok(BTreeMap::from([("paused".to_string(), Value::Bool(paused))]))
}

/// `resync#Replication`: on a local node, send every replicated set whole again; on an edge node, forget the cursors
/// (of one node when given), so each node's next batch asks for its set whole.
fn resync_replication(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let factory = current_factory()?;
    if crate::edge::settings(&factory).edge_node {
        let node = text(parameters, "pairedNodeId");
        let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
        let mut find = facade.find(crate::edge::replica::CURSOR)?.disable_authz();
        if let Some(node) = &node {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        let forgotten = find.delete_all()?;
        return Ok(BTreeMap::from([("sets".to_string(), Value::I64(forgotten))]));
    }
    let sets = match crate::edge::replica::replica(facade) {
        Some(replica) => {
            replica.resync_all();
            replica.sets().len()
        }
        None => 0,
    };
    Ok(BTreeMap::from([("sets".to_string(), count(sets))]))
}

/// `get#ReplicaValues`: on an edge node, the copies of a replicated set (of one entity, or one record by its key).
fn get_replica_values(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let set = required(parameters, "setName")?;
    let entity = text(parameters, "entityName");
    let node = text(parameters, "pairedNodeId");
    let pk: Option<BTreeMap<String, String>> = match parameters.get("pk") {
        Some(Value::Map(pk)) => Some(
            pk.iter()
                .map(|(field, value)| (field.clone(), to_plain_string(value)))
                .collect(),
        ),
        _ => None,
    };
    let values = match (&entity, pk) {
        (Some(entity), Some(pk)) => crate::edge::replica::get_value(facade, node.as_deref(), &set, entity, &pk)?
            .into_iter()
            .collect(),
        _ => {
            let limit = text(parameters, "limit")
                .and_then(|limit| limit.parse().ok())
                .unwrap_or(1000);
            crate::edge::replica::list_values(facade, node.as_deref(), &set, entity.as_deref(), limit)?
        }
    };
    let value_list = values
        .iter()
        .map(|value| {
            let mut row = indexmap::IndexMap::new();
            row.insert("pairedNodeId".to_string(), Value::string(&value.paired_node_id));
            row.insert("entityName".to_string(), Value::string(&value.entity_name));
            row.insert("pk".to_string(), moqui_lib::json::from_json(&value.pk));
            row.insert("fields".to_string(), moqui_lib::json::from_json(&value.fields));
            Value::Map(row)
        })
        .collect();
    Ok(BTreeMap::from([("valueList".to_string(), Value::List(value_list))]))
}

#[cfg(test)]
#[path = "edge_tests.rs"]
mod tests;
