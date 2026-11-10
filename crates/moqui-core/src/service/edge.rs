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

//! `org.moqui.edge.EdgeServices` (plans/Edge-Node.md §5.2, §5.7): the edge link for the System app's Edge Node and
//! Edge Clients screens, through the host's [`crate::edge::EdgeLink`]. In a host without the `edge` feature each call
//! fails with [`crate::edge::NOT_BUILT`], except `get#LinkStatus`, which answers `state` notBuilt (or notStarted in a
//! build with the link that no HTTP listener started) so a screen can say so. The queue's and replication's services
//! (`enqueue#Item`, `get#QueueStatus`, `pause#QueueDelivery`, `resync#Replication`, `get#ReplicaValues`) work on the
//! rows and state in [`crate::edge`] and need no link. The wizard's steps after pairing (§5.7, E10): `get#EdgeOffer`,
//! `apply#EdgeSetup` and `test#EdgeSetup` on a local node, and `receive#EdgeEcho`, the handler of the Test step's
//! round trips on both ends ([`crate::edge::setup`]).

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
    rust_service!(map, get_edge_offer)?;
    rust_service!(map, apply_edge_setup)?;
    rust_service!(map, test_edge_setup)?;
    rust_service!(map, receive_edge_echo)?;
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
    out.insert("mediaAddress".to_string(), Value::string(&edge.media_address));
    out.insert("turnUrls".to_string(), strings(&edge.turn_urls));
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
    out.insert(
        "nodeRole".to_string(),
        Value::string(crate::edge::node_role_kind().as_str()),
    );
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
    let mut row = indexmap::IndexMap::default();
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
    row.insert(
        "setup".to_string(),
        if client.setup_json.is_empty() {
            Value::Null
        } else {
            moqui_lib::json::from_json(&serde_json::from_str(&client.setup_json).unwrap_or_default())
        },
    );
    row.insert("setupDate".to_string(), millis(client.setup_date));
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
    let mut row = indexmap::IndexMap::default();
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
    // only a dbresource:// file, which the entity facade reads under the caller's own authorization of the call, and
    // never a file or component path of the server's
    if let Some(location) = text(parameters, "contentLocation") {
        if !location.trim().starts_with("dbresource://") {
            return Err(LibError::new(format!(
                "contentLocation {location} is not a dbresource:// location"
            )));
        }
    }
    let item = crate::edge::queue::NewItem {
        kind: required(parameters, "kind")?,
        conversation_key: required(parameters, "conversationKey")?,
        idempotency_key: text(parameters, "idempotencyKey").unwrap_or_default(),
        payload: parameters
            .get("payload")
            .filter(|value| !is_empty(value))
            .map(to_plain_string),
        content: None,
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
            let mut row = indexmap::IndexMap::default();
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
            let mut row = indexmap::IndexMap::default();
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
            let mut row = indexmap::IndexMap::default();
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
    if crate::edge::settings(&factory).takes_links() {
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
            let mut row = indexmap::IndexMap::default();
            row.insert("pairedNodeId".to_string(), Value::string(&value.paired_node_id));
            row.insert("entityName".to_string(), Value::string(&value.entity_name));
            row.insert("pk".to_string(), moqui_lib::json::from_json(&value.pk));
            row.insert("fields".to_string(), moqui_lib::json::from_json(&value.fields));
            Value::Map(row)
        })
        .collect();
    Ok(BTreeMap::from([("valueList".to_string(), Value::List(value_list))]))
}

/// A Boolean parameter: true, or the text true, Y or yes.
fn flag(parameters: &BTreeMap<String, Value>, name: &str) -> bool {
    match parameters.get(name) {
        Some(Value::Bool(on)) => *on,
        Some(value) => matches!(
            to_plain_string(value).trim().to_ascii_lowercase().as_str(),
            "true" | "y" | "yes"
        ),
        None => false,
    }
}

/// A Boolean parameter that may be left out: `None` when it is absent or empty, else as [`flag`] reads it.
fn optional_flag(parameters: &BTreeMap<String, Value>, name: &str) -> Option<bool> {
    parameters
        .get(name)
        .filter(|value| !is_empty(value))
        .map(|_| flag(parameters, name))
}

/// A list parameter as texts: a list, or a comma-separated text; blanks left out.
fn list(parameters: &BTreeMap<String, Value>, name: &str) -> Vec<String> {
    let items: Vec<String> = match parameters.get(name) {
        Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
        Some(value) if !is_empty(value) => to_plain_string(value).split(',').map(str::to_string).collect(),
        _ => Vec::new(),
    };
    items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn map(pairs: Vec<(&str, Value)>) -> Value {
    Value::Map(
        pairs
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

fn offer_value(offer: &crate::edge::EdgeOffer) -> Value {
    let mail = &offer.mail;
    map(vec![
        ("publicNames", strings(&offer.public_names)),
        ("hostModes", strings(&offer.host_modes)),
        (
            "mail",
            map(vec![
                ("available", Value::Bool(mail.available)),
                ("mta", Value::string(&mail.mta)),
                ("listener", Value::Bool(mail.listener)),
                ("mxHost", Value::string(&mail.mx_host)),
                ("bounceDomain", Value::string(&mail.bounce_domain)),
                ("dkimSelector", Value::string(&mail.dkim_selector)),
                ("dkimRecord", Value::string(&mail.dkim_record)),
            ]),
        ),
        (
            "chat",
            map(vec![
                ("webhooks", Value::Bool(offer.chat.webhooks)),
                ("publicUrl", Value::string(&offer.chat.public_url)),
            ]),
        ),
        (
            "pass",
            map(vec![
                ("available", Value::Bool(offer.pass.available)),
                ("clientId", Value::string(&offer.pass.client_id)),
            ]),
        ),
        (
            "xmpp",
            map(vec![
                ("available", Value::Bool(offer.xmpp.available)),
                ("port", Value::I64(i64::from(offer.xmpp.port))),
                ("tlsPort", Value::I64(i64::from(offer.xmpp.tls_port))),
                ("otherNode", Value::Bool(offer.xmpp.other_node)),
                ("needsNode", Value::Bool(offer.xmpp.needs_node)),
            ]),
        ),
        (
            "calls",
            map(vec![
                ("available", Value::Bool(offer.calls.available)),
                ("mediaAddress", Value::string(&offer.calls.media_address)),
                ("turnUrls", strings(&offer.calls.turn_urls)),
                ("otherNodes", Value::Bool(offer.calls.other_nodes)),
                // this node's own setting (edge_calls_enabled), for Discover's default and Status
                ("enabled", Value::Bool(crate::edge::calls::calls_enabled())),
            ]),
        ),
        (
            "sip",
            map(vec![
                ("available", Value::Bool(offer.sip.available)),
                ("sipAddress", Value::string(&offer.sip.sip_address)),
                ("tlsAddress", Value::string(&offer.sip.tls_address)),
                ("domain", Value::string(&offer.sip.domain)),
                ("registrar", Value::Bool(offer.sip.registrar)),
                (
                    "trunks",
                    Value::List(
                        offer
                            .sip
                            .trunks
                            .iter()
                            .map(|trunk| {
                                map(vec![
                                    ("voiceTrunkId", Value::string(&trunk.voice_trunk_id)),
                                    ("name", Value::string(&trunk.name)),
                                    ("state", Value::string(&trunk.state)),
                                    ("error", Value::string(&trunk.error)),
                                ])
                            })
                            .collect(),
                    ),
                ),
                ("fallbacks", count(offer.sip.fallbacks)),
                ("otherNodes", Value::Bool(offer.sip.other_nodes)),
                // this node's own setting (edge_sip_enabled), for Discover's default and Status
                ("enabled", Value::Bool(crate::edge::sip::sip_enabled())),
            ]),
        ),
        ("notAvailable", strings(&offer.not_available)),
        ("addresses", strings(&offer.addresses)),
        (
            "nodeHosts",
            Value::List(offer.node_hosts.iter().map(host_value).collect()),
        ),
    ])
}

/// `get#EdgeOffer`: on a local node, what its edge node offers it now, asked on the link.
fn get_edge_offer(_parameters: &BTreeMap<String, Value>, _facade: &EntityFacade) -> Out {
    let link = crate::edge::link()?;
    let offer = link.offer()?;
    Ok(BTreeMap::from([
        ("offer".to_string(), offer_value(&offer)),
        ("edgeUrl".to_string(), Value::string(&link.status().url)),
    ]))
}

fn line_value(line: &crate::edge::setup::SetupLine) -> Value {
    map(vec![
        ("area", Value::string(&line.area)),
        ("ok", Value::Bool(line.ok)),
        ("text", Value::string(&line.text)),
    ])
}

/// The host names of `apply#EdgeSetup`'s `hostList`: maps of hostName and modeEnumId (passthrough by default).
fn host_list(parameters: &BTreeMap<String, Value>) -> Vec<EdgeHostInfo> {
    let Some(Value::List(items)) = parameters.get("hostList") else {
        return list(parameters, "hostList")
            .into_iter()
            .map(|name| EdgeHostInfo {
                host_name: name,
                paired_node_id: String::new(),
                mode_enum_id: HOST_PASSTHROUGH.to_string(),
            })
            .collect();
    };
    items
        .iter()
        .filter_map(|item| {
            let (name, mode) = match item {
                Value::Map(row) => (
                    row.get("hostName").map(to_plain_string).unwrap_or_default(),
                    row.get("modeEnumId").map(to_plain_string).unwrap_or_default(),
                ),
                other => (to_plain_string(other), String::new()),
            };
            let name = name.trim().to_string();
            (!name.is_empty()).then(|| EdgeHostInfo {
                host_name: name,
                paired_node_id: String::new(),
                mode_enum_id: if mode.trim().is_empty() {
                    HOST_PASSTHROUGH.to_string()
                } else {
                    mode.trim().to_string()
                },
            })
        })
        .collect()
}

/// The local node's XMPP domain (`xmpp_domain`), lower case with no trailing dot; empty when it runs no XMPP server.
fn xmpp_domain() -> String {
    moqui_lib::env::get_prop_or_env("xmpp_domain")
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// The XMPP choice of `apply#EdgeSetup` (plans/Edge-Node.md §5.7, E10): this node's `xmpp_domain`, which must be one of
/// its host names on the edge node, since uploads (`https://<domain>`) and `/xmpp-websocket` ride on web access. With
/// web access on, the domain is added to `hosts` as passthrough when it is not listed; with it off, the edge node should
/// have it already, and the answer's second part is false when it does not (a failed line, the rest still applied).
/// Why not, when XMPP cannot be set up at all.
fn xmpp_choice(
    offer: &crate::edge::EdgeOffer,
    web: bool,
    hosts: &mut Vec<EdgeHostInfo>,
) -> Result<(String, bool), String> {
    let domain = xmpp_domain();
    if domain.is_empty() {
        return Err("this node runs no XMPP server: set xmpp_domain".to_string());
    }
    if !offer.xmpp.available {
        return Err("the edge node has no XMPP tunnel ports (edge_xmpp_port, edge_xmpp_tls_port)".to_string());
    }
    let named = |name: &str| name.trim().trim_end_matches('.').eq_ignore_ascii_case(&domain);
    if web {
        if !hosts.iter().any(|host| named(&host.host_name)) {
            hosts.push(EdgeHostInfo {
                host_name: domain.clone(),
                paired_node_id: String::new(),
                mode_enum_id: HOST_PASSTHROUGH.to_string(),
            });
        }
        return Ok((domain, true));
    }
    let hosted = offer.node_hosts.iter().any(|host| named(&host.host_name));
    Ok((domain, hosted))
}

/// The lines of the XMPP area after Configure: the domain through the edge node, and what the operator still has to
/// keep in step (its host name refused, the tunnels going to another node, Chat apps showing other port numbers).
fn xmpp_lines(domain: &str, hosted: bool, answer: &crate::edge::Configured) -> Vec<crate::edge::setup::SetupLine> {
    use crate::edge::setup::SetupLine;
    let xmpp = &answer.offer.xmpp;
    let ports: Vec<String> = [(xmpp.port, "STARTTLS"), (xmpp.tls_port, "direct TLS")]
        .iter()
        .filter(|(port, _)| *port > 0)
        .map(|(port, kind)| format!("{port} ({kind})"))
        .collect();
    let mut lines = vec![SetupLine::new(
        "xmpp",
        true,
        format!(
            "XMPP clients of {domain} come through the edge node's port {}",
            ports.join(" and ")
        ),
    )];
    if let Some(host) = answer
        .hosts
        .iter()
        .find(|host| host.host_name == domain && !host.error.is_empty())
    {
        lines.push(SetupLine::new(
            "xmpp",
            false,
            format!(
                "Uploads and the WebSocket need {domain} on web access, and the edge node did not take it: {}",
                host.error
            ),
        ));
    }
    if !hosted {
        lines.push(SetupLine::new(
            "xmpp",
            false,
            format!(
                "Uploads and the WebSocket need {domain} on web access, and web access was off with the edge node \
                 not having the name: turn web access on and apply again"
            ),
        ));
    }
    if xmpp.other_node {
        lines.push(SetupLine::new(
            "xmpp",
            false,
            "The edge node's edge_xmpp_node names another local node, so its XMPP clients do not come here",
        ));
    }
    if xmpp.needs_node {
        lines.push(SetupLine::new(
            "xmpp",
            false,
            "Several local nodes are linked to the edge node and its edge_xmpp_node is empty, so XMPP clients are \
             refused: set edge_xmpp_node there to this node",
        ));
    }
    // Chat apps (get#ClientSettings) shows this node's own ports: the edge node's should have the same numbers
    for (name, edge_port) in [("xmpp_port", xmpp.port), ("xmpp_tls_port", xmpp.tls_port)] {
        let own = moqui_lib::env::get_prop_or_env(name).trim().to_string();
        if !own.is_empty() && edge_port > 0 && own != edge_port.to_string() {
            lines.push(SetupLine::new(
                "xmpp",
                false,
                format!(
                    "Chat apps shows this node's {name} {own}, and the edge node's port is {edge_port}: give them \
                     the same number"
                ),
            ));
        }
    }
    lines
}

/// `apply#EdgeSetup`: on a paired local node, check the whole setup first (nothing changes when any of it is not
/// input), send the edge node its part (Configure; when it is not taken nothing changes here either), then write the
/// chosen areas' local rows and settings, and send every replicated set whole again. Each step is a line; once the
/// edge node has its part, a local step that fails does not stop the others. XMPP writes nothing here: the domain is
/// this node's `xmpp_domain`, sent in Configure and added to the host names ([`xmpp_choice`]). Calls send nothing to
/// the edge node: with its media node offered, `callsOn` is this node's `edge_calls_enabled`
/// ([`crate::edge::calls::apply`]). Phones neither: with its SIP engine offered, `sipOn` is this node's
/// `edge_sip_enabled` ([`crate::edge::sip::apply`]), which holds the trunks and fallbacks back while off.
fn apply_edge_setup(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    use crate::edge::setup::{self, LocalChoices, MailChoice, PassChoice, SetupLine};
    let factory = current_factory()?;
    if crate::edge::settings(&factory).takes_links() {
        return Err(LibError::new(
            "an edge node is set up in its own .env and conf, not by apply#EdgeSetup",
        ));
    }
    let link = crate::edge::link()?;
    let status = link.status();
    let offer = link.offer().map_err(|err| {
        LibError::new(format!(
            "the edge node's offer was not read, so nothing was changed: {err}"
        ))
    })?;
    let (web, mail, chat, pass, xmpp) = (
        flag(parameters, "webOn"),
        flag(parameters, "mailOn"),
        flag(parameters, "chatOn"),
        flag(parameters, "passOn"),
        flag(parameters, "xmppOn"),
    );
    let bridges = list(parameters, "chatBridgeIds");
    let sites = list(parameters, "passSites");
    let choices = LocalChoices {
        edge_url: status.url.clone(),
        mail: mail.then(|| MailChoice {
            system: flag(parameters, "systemMail"),
            account_ids: list(parameters, "emailAccountIds"),
        }),
        chat: chat.then(|| bridges.clone()),
        pass: pass.then(|| PassChoice {
            sites: sites.clone(),
            callback: text(parameters, "passCallback").unwrap_or_default(),
        }),
    };
    let mut hosts = if web { host_list(parameters) } else { Vec::new() };
    let (xmpp_domain, xmpp_hosted) = if xmpp {
        xmpp_choice(&offer, web, &mut hosts).map_err(|why| LibError::new(format!("nothing was changed: {why}")))?
    } else {
        (String::new(), true)
    };
    let request = crate::edge::EdgeSetup {
        hosts,
        set_hosts: web,
        mail_domains: if mail {
            list(parameters, "mailDomains")
        } else {
            Vec::new()
        },
        chat_bridge_ids: if chat { bridges } else { Vec::new() },
        pass_sites: if pass { sites } else { Vec::new() },
        xmpp_domain: xmpp_domain.clone(),
    };
    // the whole setup is checked before anything changes, here or on the edge node
    let checked = setup::check_setup(&request).map_err(|err| LibError::new(format!("nothing was changed: {err}")))?;
    for host in &checked.hosts {
        setup::check_host_name(&host.host_name)
            .map_err(|why| LibError::new(format!("nothing was changed: {}: {why}", host.host_name)))?;
    }
    if let Some(pass) = &choices.pass {
        setup::check_pass(pass, &offer).map_err(|why| LibError::new(format!("nothing was changed: {why}")))?;
    }
    let mut lines = Vec::new();
    let mut host_rows = Vec::new();
    // the edge node's part first: when it is not taken, this node is left as it was
    let answer = match link.configure(&request) {
        Ok(answer) => answer,
        Err(err) => {
            tracing::error!("Edge setup: the edge node was not configured: {err}");
            lines.push(SetupLine::new(
                "link",
                false,
                format!("The edge node did not take the setup, so nothing was changed on this node: {err}"),
            ));
            return Ok(BTreeMap::from([
                (
                    "lineList".to_string(),
                    Value::List(lines.iter().map(line_value).collect()),
                ),
                ("hostList".to_string(), Value::List(host_rows)),
                ("configured".to_string(), Value::Bool(false)),
            ]));
        }
    };
    for host in &answer.hosts {
        let ok = host.error.is_empty();
        let mode = if host.mode_enum_id == HOST_TERMINATE {
            "terminate"
        } else {
            "passthrough"
        };
        lines.push(SetupLine::new(
            "web",
            ok,
            if ok {
                format!("{} goes to this node ({mode})", host.host_name)
            } else {
                format!("{}: {}", host.host_name, host.error)
            },
        ));
        host_rows.push(map(vec![
            ("hostName", Value::string(&host.host_name)),
            ("modeEnumId", Value::string(&host.mode_enum_id)),
            ("error", Value::string(&host.error)),
        ]));
    }
    lines.push(SetupLine::new("link", true, "The edge node has this node's setup"));
    let env_file = factory.runtime_dir().join(".env");
    lines.extend(setup::apply_local(facade, &env_file, &choices, &answer.offer));
    if xmpp {
        lines.extend(xmpp_lines(&xmpp_domain, xmpp_hosted, &answer));
    }
    // calls: on or off when the edge node has a media node and the caller chose (callsOn given; Discover passes it
    // only when it offered calls); absent leaves edge_calls_enabled as it is. Nothing is sent to the edge node.
    let calls = optional_flag(parameters, "callsOn");
    if let (Some(on), true) = (calls, answer.offer.calls.available) {
        lines.extend(crate::edge::calls::apply(
            &factory,
            facade,
            &env_file,
            on,
            &status.peer,
            &answer.offer.calls,
        ));
    } else if calls == Some(true) {
        lines.push(SetupLine::new(
            "calls",
            false,
            "The edge node runs no media node (rtc_media_port), so calls stay on this node's own media nodes",
        ));
    }
    // phones the same way: sipOn given (Discover passes it only when it offered phones), and the edge node's SIP
    // engine offered; absent leaves edge_sip_enabled as it is
    let sip = optional_flag(parameters, "sipOn");
    if let (Some(on), true) = (sip, answer.offer.sip.available) {
        lines.extend(crate::edge::sip::apply(
            facade,
            &env_file,
            on,
            &status.peer,
            &answer.offer.sip,
        ));
    } else if sip == Some(true) {
        lines.push(SetupLine::new(
            "sip",
            false,
            "The edge node runs no SIP engine (sip_port, sip_tls_port), so this node's trunks do not go through it",
        ));
    }
    let sets = match crate::edge::replica::replica(facade) {
        Some(replica) => {
            replica.resync_all();
            replica.sets().len()
        }
        None => 0,
    };
    if sets > 0 {
        lines.push(SetupLine::new(
            "link",
            true,
            format!("{sets} replicated sets go to the edge node whole again"),
        ));
    }
    Ok(BTreeMap::from([
        (
            "lineList".to_string(),
            Value::List(lines.iter().map(line_value).collect()),
        ),
        ("hostList".to_string(), Value::List(host_rows)),
        ("configured".to_string(), Value::Bool(true)),
    ]))
}

/// How long the Test step waits for the edge node's answers, all together.
const TEST_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A line of the Test step: passed, failed, or not checked (nothing chosen to check it with).
struct TestLine {
    check: &'static str,
    label: &'static str,
    /// `None`: not checked.
    ok: Option<bool>,
    message: String,
}

/// `test#EdgeSetup`: on a paired local node, check the chosen areas end to end: the link is up; for mail, the edge
/// node takes mail for an address (its answer comes back on the queue, as inbound mail does), the system's and the
/// chosen accounts' mail goes through the edge mail server, and the edge node can send it (its DKIM key, or its
/// Postfix relay); for chat, it takes the bridges' webhooks; for PASS, the client settings are there and the edge
/// node has mr-pass; for XMPP, a client stream through each of the edge node's tunnel ports reaches this node's XMPP
/// server ([`crate::edge::EdgeLink::xmpp_probe`]) and the domain is one of this node's host names there; for calls,
/// the edge node's media node row and its TURN ([`calls_tests`]); for phones, the edge node's SIP node row, its
/// engine's trunks and its copy of the fallbacks ([`sip_tests`]). No outside mail or provider is needed. A check with
/// nothing to check is "not checked".
fn test_edge_setup(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    use crate::edge::setup::{start_echo, wait_echo, EchoCheck};
    let factory = current_factory()?;
    let mut tests: Vec<TestLine> = Vec::new();
    let link = crate::edge::link();
    let status = link.as_ref().map(|link| link.status()).ok();
    let (ok, message) = match &status {
        Some(status) if status.state == crate::edge::LinkState::Linked => (
            true,
            match status.rtt_ms {
                Some(rtt) => format!("Linked, round trip {rtt:.1} ms"),
                None => "Linked".to_string(),
            },
        ),
        Some(status) => (
            false,
            format!("The link is {}: {}", status.state.as_str(), status.last_error),
        ),
        None => (false, crate::edge::NOT_BUILT.to_string()),
    };
    tests.push(TestLine {
        check: "link",
        label: "The link",
        ok: Some(ok),
        message,
    });
    // the round trips go out together, then each answer is waited for until one deadline
    let mut pending: Vec<(&'static str, &'static str, Result<String, LibError>)> = Vec::new();
    let mut echo = |check: &'static str, label: &'static str, echo: EchoCheck| {
        pending.push((check, label, start_echo(&factory, facade, &echo)));
    };
    let mail_on = flag(parameters, "mailOn");
    let accounts = list(parameters, "emailAccountIds");
    let mut not_checked: Vec<TestLine> = Vec::new();
    if mail_on {
        let address = text(parameters, "mailAddress").or_else(|| hosted_address(facade));
        match address {
            Some(address) => echo(
                "mailIn",
                "Mail in",
                EchoCheck {
                    check: "mailIn".to_string(),
                    address,
                    bridge_ids: Vec::new(),
                },
            ),
            None => not_checked.push(TestLine {
                check: "mailIn",
                label: "Mail in",
                ok: None,
                message: "Not checked: this node has no hosted or shared account".to_string(),
            }),
        }
        echo(
            "mailOut",
            "Mail out",
            EchoCheck {
                check: "mailOut".to_string(),
                ..EchoCheck::default()
            },
        );
    }
    let bridges = list(parameters, "chatBridgeIds");
    if flag(parameters, "chatOn") {
        if bridges.is_empty() {
            not_checked.push(TestLine {
                check: "webhook",
                label: "Webhooks",
                ok: None,
                message: "Not checked: no chat bridge was chosen".to_string(),
            });
        } else {
            echo(
                "webhook",
                "Webhooks",
                EchoCheck {
                    check: "webhook".to_string(),
                    address: String::new(),
                    bridge_ids: bridges,
                },
            );
        }
    }
    // the XMPP probes run on a thread of their own while the round trips are on their way
    let xmpp_probes = flag(parameters, "xmppOn").then(|| {
        let link = link.as_ref().ok().map(std::sync::Arc::clone);
        std::thread::spawn(move || xmpp_tests(link.as_deref()))
    });
    // calls: the row and the credential here, now; the TURN allocation on a thread of its own
    let (calls_lines, turn_probe) = if flag(parameters, "callsOn") {
        calls_tests(&factory, facade, link.as_ref().ok())
    } else {
        (Vec::new(), None)
    };
    let deadline = std::time::Instant::now() + TEST_WAIT;
    for (check, label, started) in pending {
        let (mut ok, mut message) = match started {
            Err(err) => (false, format!("Not queued: {err}")),
            Ok(token) => match wait_echo(&token, deadline) {
                Some(answer) => (answer.ok, answer.message),
                None => (
                    false,
                    "No answer from the edge node in 10 seconds: its link, or its queue, is not working".to_string(),
                ),
            },
        };
        if ok && check == "mailOut" {
            if let Some(problem) = mail_out_problem(facade, flag(parameters, "systemMail"), &accounts) {
                (ok, message) = (false, problem);
            }
        }
        tests.push(TestLine {
            check,
            label,
            ok: Some(ok),
            message,
        });
    }
    tests.extend(not_checked);
    if let Some(probes) = xmpp_probes {
        tests.extend(probes.join().unwrap_or_else(|_| {
            vec![TestLine {
                check: "xmpp",
                label: "XMPP",
                ok: Some(false),
                message: "The XMPP check stopped with an error; the log has why".to_string(),
            }]
        }));
    }
    tests.extend(calls_lines);
    if flag(parameters, "sipOn") {
        tests.extend(sip_tests(facade, link.as_ref().ok()));
    }
    if let Some(probe) = turn_probe {
        tests.push(probe.join().unwrap_or_else(|_| TestLine {
            check: "turn",
            label: "TURN",
            ok: Some(false),
            message: "The TURN check stopped with an error; the log has why".to_string(),
        }));
    }
    if flag(parameters, "passOn") {
        let (ok, message) = pass_ready(link.ok().as_deref(), &list(parameters, "passSites"));
        tests.push(TestLine {
            check: "pass",
            label: "PASS",
            ok: Some(ok),
            message,
        });
    }
    let passed = tests.iter().all(|test| test.ok != Some(false));
    let test_list = tests
        .into_iter()
        .map(|test| {
            map(vec![
                ("check", Value::string(test.check)),
                ("label", Value::string(test.label)),
                ("ok", Value::Bool(test.ok == Some(true))),
                ("checked", Value::Bool(test.ok.is_some())),
                ("message", Value::string(test.message)),
            ])
        })
        .collect();
    Ok(BTreeMap::from([
        ("testList".to_string(), Value::List(test_list)),
        ("passed".to_string(), Value::Bool(passed)),
    ]))
}

/// The Test step's XMPP lines: a client stream through each tunnel port the edge node has on (not checked for one
/// that is off), and the domain among this node's host names there, for uploads and the WebSocket.
fn xmpp_tests(link: Option<&dyn crate::edge::EdgeLink>) -> Vec<TestLine> {
    let failed = |message: String| TestLine {
        check: "xmpp",
        label: "XMPP",
        ok: Some(false),
        message,
    };
    let Some(link) = link else {
        return vec![failed(crate::edge::NOT_BUILT.to_string())];
    };
    let offer = match link.offer() {
        Ok(offer) => offer,
        Err(err) => return vec![failed(format!("The edge node's offer was not read: {err}"))],
    };
    if !offer.xmpp.available {
        return vec![failed("The edge node has no XMPP tunnel ports".to_string())];
    }
    let mut tests = Vec::new();
    for (port, direct, check, label) in [
        (offer.xmpp.port, false, "xmpp", "XMPP (STARTTLS)"),
        (offer.xmpp.tls_port, true, "xmpps", "XMPP (direct TLS)"),
    ] {
        let (ok, message) = if port == 0 {
            (None, "Not checked: the edge node's port for it is off".to_string())
        } else {
            match link.xmpp_probe(port, direct) {
                Ok(reached) => (Some(true), reached),
                Err(err) => (Some(false), err.to_string()),
            }
        };
        tests.push(TestLine {
            check,
            label,
            ok,
            message,
        });
    }
    let domain = xmpp_domain();
    let hosted = !domain.is_empty()
        && offer
            .node_hosts
            .iter()
            .any(|host| host.host_name.eq_ignore_ascii_case(&domain));
    tests.push(TestLine {
        check: "xmppWeb",
        label: "XMPP uploads",
        ok: Some(hosted),
        message: if hosted {
            format!("Uploads and the WebSocket of {domain} ride on the edge node's web access")
        } else {
            format!("{domain} is not one of this node's host names on the edge node: apply again with web access on")
        },
    });
    tests
}

/// The Test step's calls lines (plans/Edge-Node.md §5.7, E10 calls): the edge node's row is live and preferred
/// (`callsNode`, [`crate::edge::calls::row_check`]), `make#IceServers` gives its TURN URLs a credential signed with
/// the secret its link handed this node (`iceServers`), and, on a thread of its own, that credential allocates on its
/// TURN with a permission for its media address's IP (`turn`, [`crate::edge::EdgeLink::turn_probe`]). A check with no TURN
/// is not checked.
fn calls_tests(
    factory: &std::sync::Arc<crate::context::ExecutionContextFactory>,
    facade: &EntityFacade,
    link: Option<&std::sync::Arc<dyn crate::edge::EdgeLink>>,
) -> (Vec<TestLine>, Option<std::thread::JoinHandle<TestLine>>) {
    let line = |check: &'static str, label: &'static str, ok: Option<bool>, message: String| TestLine {
        check,
        label,
        ok,
        message,
    };
    let Some(link) = link else {
        let failed = line("callsNode", "Calls", Some(false), crate::edge::NOT_BUILT.to_string());
        return (vec![failed], None);
    };
    let status = link.status();
    let edge = match crate::edge::calls::linked_media_node(&status) {
        Ok(edge) => edge,
        Err(why) => return (vec![line("callsNode", "Calls", Some(false), why)], None),
    };
    if !crate::edge::calls::calls_enabled() {
        let message = format!(
            "Not checked: calls are off here ({}), so new calls do not use the edge node",
            crate::edge::calls::CALLS_ENABLED
        );
        return (vec![line("callsNode", "Calls", None, message)], None);
    }
    let (ok, message) = crate::edge::calls::row_check(facade, edge, crate::user::now_millis());
    let mut lines = vec![line("callsNode", "Calls", Some(ok), message)];
    if edge.turn_urls.is_empty() {
        let message = "Not checked: the edge node runs no TURN".to_string();
        lines.push(line("iceServers", "TURN credentials", None, message));
        return (lines, None);
    }
    let row_urls = crate::edge::calls::media_row(facade, &edge.instance_id)
        .ok()
        .flatten()
        .map(|row| row.turn_urls)
        .unwrap_or_default();
    let leg = format!(
        "{}.{}",
        crate::edge::calls::PROBE_LEG,
        crate::user::random_string(12).unwrap_or_else(|_| crate::user::now_millis().to_string())
    );
    let (urls, username, credential) = match crate::edge::calls::ice_check(factory, edge, &row_urls, &leg) {
        Ok(ice) => ice,
        Err(why) => {
            lines.push(line("iceServers", "TURN credentials", Some(false), why));
            return (lines, None);
        }
    };
    lines.push(line(
        "iceServers",
        "TURN credentials",
        Some(true),
        format!(
            "make#IceServers gives the edge node's {} a credential signed with its secret",
            urls.join(", ")
        ),
    ));
    let Some(url) = urls
        .iter()
        .find(|url| url.starts_with("turn:") && !url.contains("transport=tcp"))
        .cloned()
    else {
        let message = "Not checked: the edge node's TURN has no UDP listener".to_string();
        lines.push(line("turn", "TURN", None, message));
        return (lines, None);
    };
    let link = std::sync::Arc::clone(link);
    let peer = edge.media_address.clone();
    let probe = std::thread::spawn(move || {
        let (ok, message) = match link.turn_probe(&url, &username, &credential, &peer) {
            Ok(done) => (true, done),
            Err(err) => (false, err.to_string()),
        };
        TestLine {
            check: "turn",
            label: "TURN",
            ok: Some(ok),
            message,
        }
    });
    (lines, Some(probe))
}

/// The Test step's phones lines (plans/Edge-Node.md §5.7, E10 phones): the edge node's `SipNode` row is live with its
/// Hello's addresses (`sipNode`, [`crate::edge::sip::row_check`]), its engine has loaded this node's enabled trunks,
/// none failed (`sipTrunks`), and its copy holds each number's fallback with nothing waiting (`sipFallbacks`). Not
/// checked while `edge_sip_enabled` is off. No SIP request is sent: the edge node's engine answers OPTIONS only from
/// its trunks.
fn sip_tests(facade: &EntityFacade, link: Option<&std::sync::Arc<dyn crate::edge::EdgeLink>>) -> Vec<TestLine> {
    let line = |check: &'static str, label: &'static str, ok: Option<bool>, message: String| TestLine {
        check,
        label,
        ok,
        message,
    };
    let Some(link) = link else {
        return vec![line(
            "sipNode",
            "Phones",
            Some(false),
            crate::edge::NOT_BUILT.to_string(),
        )];
    };
    let status = link.status();
    let edge = match crate::edge::sip::linked_sip_node(&status) {
        Ok(edge) => edge,
        Err(why) => return vec![line("sipNode", "Phones", Some(false), why)],
    };
    if !crate::edge::sip::sip_enabled() {
        let message = format!(
            "Not checked: phones are off here ({}), so this node's trunks do not go through the edge node",
            crate::edge::sip::SIP_ENABLED
        );
        return vec![line("sipNode", "Phones", None, message)];
    }
    let (ok, message) = match crate::edge::sip::sip_row(facade, &edge.instance_id) {
        Ok(row) => crate::edge::sip::row_check(row.as_ref(), edge, crate::user::now_millis()),
        Err(err) => (false, format!("The edge node's SIP node row was not read: {err}")),
    };
    let mut lines = vec![line("sipNode", "Phones", Some(ok), message)];
    let offer = match link.offer() {
        Ok(offer) => offer,
        Err(err) => {
            let message = format!("The edge node's offer was not read: {err}");
            lines.push(line("sipTrunks", "Trunks", Some(false), message));
            return lines;
        }
    };
    let (ok, message) = match crate::edge::sip::local_trunks(facade) {
        Ok(local) => crate::edge::sip::trunk_check(&local, &offer.sip),
        Err(err) => (Some(false), format!("This node's trunks were not read: {err}")),
    };
    lines.push(line("sipTrunks", "Trunks", ok, message));
    let set = crate::edge::replica::replica(facade).and_then(|replica| {
        replica
            .status()
            .into_iter()
            .find(|set| set.name == crate::edge::sip::FALLBACKS_SET)
    });
    let (ok, message) = match crate::edge::sip::coverage(facade) {
        Ok(coverage) => crate::edge::sip::fallback_check(coverage.fallbacks, &offer.sip, set.as_ref()),
        Err(err) => (false, format!("This node's fallbacks were not read: {err}")),
    };
    lines.push(line("sipFallbacks", "Voicemail fallback", Some(ok), message));
    lines
}

/// The address of a hosted or shared account here, for the mail-in check when none was chosen.
fn hosted_address(facade: &EntityFacade) -> Option<String> {
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let rows = facade
        .find("moqui.basic.email.EmailAccount")
        .ok()?
        .condition(crate::entity::Condition::is_in(
            "accountKindEnumId",
            vec![Value::string("EakHosted"), Value::string("EakShared")],
        ))
        .order_by("emailAddress", false)
        .limit(1)
        .disable_authz()
        .list()
        .ok()?;
    rows.values()
        .first()
        .and_then(|row| row.get("emailAddress").map(to_plain_string))
        .filter(|address| !address.is_empty())
}

/// Why this node's mail does not go to the edge node, when it does not: the edge mail server, `SYSTEM` (when chosen)
/// or a chosen account is not on `edge:`.
fn mail_out_problem(facade: &EntityFacade, system: bool, accounts: &[String]) -> Option<String> {
    use crate::edge::setup::{EDGE_SERVER_ID, EDGE_SMTP_HOST, SYSTEM_SERVER_ID};
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    let one = |entity: &str, field: &str, id: &str| -> Result<Option<crate::entity::EntityValue>, LibError> {
        facade
            .find(entity)?
            .eq(field, Value::string(id))
            .disable_authz()
            .use_cache(false)
            .one()
    };
    let smtp_host = |server: &str| -> Result<String, LibError> {
        Ok(one("moqui.basic.email.EmailServer", "emailServerId", server)?
            .and_then(|row| row.get("smtpHost").map(to_plain_string))
            .unwrap_or_default())
    };
    let check = || -> Result<Option<String>, LibError> {
        let mut servers = vec![EDGE_SERVER_ID.to_string()];
        if system {
            servers.push(SYSTEM_SERVER_ID.to_string());
        }
        for account in accounts {
            let server = one("moqui.basic.email.EmailAccount", "emailAccountId", account)?
                .and_then(|row| row.get("emailServerId").map(to_plain_string))
                .unwrap_or_default();
            if server.is_empty() {
                return Ok(Some(format!("Account {account} has no mail server: apply again")));
            }
            servers.push(server);
        }
        for server in servers {
            if smtp_host(&server)? != EDGE_SMTP_HOST {
                return Ok(Some(format!(
                    "Mail server {server} does not send through the edge node: apply again"
                )));
            }
        }
        Ok(None)
    };
    match check() {
        Ok(problem) => problem,
        Err(err) => Some(format!("The mail servers were not read: {err}")),
    }
}

/// PASS readiness on the local node: the edge node's client has the sites and a callback, and the edge node has
/// mr-pass.
fn pass_ready(link: Option<&dyn crate::edge::EdgeLink>, sites: &[String]) -> (bool, String) {
    let client = crate::pass_authority::Client::edge_client_id();
    let registered = moqui_lib::env::get_prop_or_env(&format!("pass_client_{client}_sites"));
    let registered: Vec<&str> = registered
        .split(',')
        .map(str::trim)
        .filter(|site| !site.is_empty())
        .collect();
    let missing: Vec<&str> = sites
        .iter()
        .map(String::as_str)
        .filter(|site| !registered.contains(site))
        .collect();
    if registered.is_empty() || !missing.is_empty() {
        let names = if missing.is_empty() {
            "the sites".to_string()
        } else {
            missing.join(", ")
        };
        return (
            false,
            format!("pass_client_{client}_sites does not name {names}: apply again"),
        );
    }
    if moqui_lib::env::get_prop_or_env(&format!("pass_client_{client}_callback"))
        .trim()
        .is_empty()
    {
        return (false, format!("pass_client_{client}_callback is not set: apply again"));
    }
    match link.map(|link| link.offer()) {
        Some(Ok(offer)) if offer.pass.available => (
            true,
            format!("The edge node serves {} as PASS client {client}", registered.join(", ")),
        ),
        Some(Ok(_)) => (false, "The edge node has no mr-pass".to_string()),
        Some(Err(err)) => (false, format!("The edge node's offer was not read: {err}")),
        None => (false, crate::edge::NOT_BUILT.to_string()),
    }
}

/// `receive#EdgeEcho`: the handler of the `edgeEcho` queue kind, a Test step round trip.
fn receive_edge_echo(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let factory = current_factory()?;
    let node = required(parameters, "pairedNodeId")?;
    let payload = text(parameters, "payload").unwrap_or_default();
    crate::edge::setup::receive_echo(&factory, facade, &node, &payload)?;
    Ok(BTreeMap::new())
}

#[cfg(test)]
#[path = "edge_tests.rs"]
mod tests;
