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

//! Whose trunk, number, call and phone the SIP engine handles (plans/archive/Edge-Node.md §10, E12): the server's own, or the
//! paired local node's. An all-in-one node has both: its own trunks, numbers and phones, as a normal node, and its
//! paired node's, as an edge node (E8). A dedicated edge node has only the paired node's.
//!
//! - **Keys.** The two databases' ids may be the same, so the engine keys each id of a paired node's item by that node
//!   ([`key_ids`], `moqui_core::edge::media::room_key`): its trunks, calls, legs and phones. A call's key is also its
//!   media room, as the media driver keys the browsers' frames of that node's calls. A service call gets the ids back
//!   bare ([`bare_ids`]) and runs as the item's owner (`moqui_core::edge::owner::with`), so a paired node's goes down
//!   its link and the server's own runs here; its answer's ids are keyed again.
//! - **Trunks** come from both sides ([`merge_config`]): the server's `get#EngineConfig` and the paired node's copy of
//!   `voiceTrunks`. The dial policy and the blocked sources are the server's.
//! - **An inbound call** goes by the number called ([`number_owner`]): the server's own `PhoneNumber`, else a number
//!   the paired node's copy of `voiceFallbacks` has, else the owner of the trunk it came on.
//! - **Phones** go by the domain they register to: the server's `sip_domain` (the registrar's own realm) is the server's,
//!   the domain of the paired node's Configure (`sipDomain`) is the paired node's ([`paired_domain`]).

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::claims::{self, ClaimKind};
use moqui_core::edge::owner::Owner;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::Value as Json;

/// The fields that hold an id of a call, a leg, a trunk or a phone in the engine's messages and the services'.
pub(crate) const ID_FIELDS: [&str; 4] = ["callSessionId", "callLegId", "voiceTrunkId", "sipDeviceId"];

/// The engine's key of `id` of `owner`'s item.
pub(crate) fn key(owner: &Owner, id: &str) -> String {
    if id.is_empty() {
        return String::new();
    }
    moqui_core::edge::media::room_key(owner.node(), id)
}

/// The owner of an item by the engine's key of its id.
pub(crate) fn owner_of(key: &str) -> Owner {
    moqui_core::edge::media::room_owner(key)
}

/// An id as its own database has it, from the engine's key of it.
pub(crate) fn bare(key: &str) -> &str {
    moqui_core::edge::media::split_room(key).1
}

/// The bare id of the trunk `trunk` (the engine's key of it) when it is `owner`'s, else `None`: a trunk of the other
/// side means nothing in the owner's database (an inbound call routed by its number, E12).
pub(crate) fn trunk_of<'a>(owner: &Owner, trunk: &'a str) -> Option<&'a str> {
    (!trunk.is_empty() && owner_of(trunk) == *owner).then(|| bare(trunk))
}

/// Each id in `value` ([`ID_FIELDS`], at any depth), from `node`'s link or its answer, keyed by `node`. Its ids are
/// its own database's and always bare, so each is keyed, also one that holds the key's separator: such an id stays
/// under `node` and never names another node's item or the server's own.
pub(crate) fn key_ids(node: &str, value: &mut Json) {
    match value {
        Json::Object(map) => {
            for (name, field) in map.iter_mut() {
                match field {
                    Json::String(id) if ID_FIELDS.contains(&name.as_str()) => {
                        if !id.is_empty() {
                            *id = moqui_core::edge::media::room_key(Some(node), id);
                        }
                    }
                    other => key_ids(node, other),
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(|item| key_ids(node, item)),
        _ => {}
    }
}

/// Each id in `value` ([`ID_FIELDS`], at any depth) as its own database has it.
pub(crate) fn bare_ids(value: &mut Json) {
    match value {
        Json::Object(map) => {
            for (name, field) in map.iter_mut() {
                match field {
                    Json::String(id) if ID_FIELDS.contains(&name.as_str()) => {
                        let bare = bare(id).to_string();
                        *id = bare;
                    }
                    other => bare_ids(other),
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(bare_ids),
        _ => {}
    }
}

/// The owner of a note by the ids it names: a paired node's when one of them is keyed by that node, else the server's.
pub(crate) fn owner_in(value: &Json) -> Owner {
    ID_FIELDS
        .iter()
        .filter_map(|name| value.get(*name).and_then(Json::as_str))
        .find_map(|id| moqui_core::edge::media::split_room(id).0)
        .map_or(Owner::Own, Owner::paired)
}

/// The owner of an inbound call's number: the server's when it has the number (`own`), else the paired node whose
/// copies have it (`replica`), else the owner of the trunk it came on.
pub(crate) fn number_owner(own: bool, replica: Option<String>, trunk: Owner) -> Owner {
    if own {
        return Owner::Own;
    }
    match replica.filter(|node| !node.is_empty()) {
        Some(node) => Owner::paired(&node),
        None => trunk,
    }
}

/// The engine's configuration from the server's own (`own`, `get#EngineConfig`'s answer, `None` on a dedicated edge
/// node) and the paired node's (`paired`, its node and the answer from its copy, E8): every trunk of both, a paired
/// node's keyed by the node; the policy and the blocked sources of the server's own when it has them, else the paired
/// node's.
pub(crate) fn merge_config(own: Option<Json>, paired: Option<(&str, Json)>) -> Json {
    let mut trunks = Vec::new();
    let mut policy = None;
    let mut blocked = None;
    if let Some(own) = own {
        if let Some(Json::Array(list)) = own.get("trunkList") {
            trunks.extend(list.iter().cloned());
        }
        policy = own.get("policy").cloned();
        blocked = own.get("blockedSourceList").cloned();
    }
    if let Some((node, config)) = paired {
        if let Some(Json::Array(list)) = config.get("trunkList") {
            for trunk in list {
                let mut trunk = trunk.clone();
                if let Some(Json::String(id)) = trunk.get_mut("voiceTrunkId") {
                    *id = moqui_core::edge::media::room_key(Some(node), id);
                }
                trunks.push(trunk);
            }
        }
        policy = policy.or_else(|| config.get("policy").cloned());
        blocked = blocked.or_else(|| config.get("blockedSourceList").cloned());
    }
    serde_json::json!({"trunkList": trunks, "policy": policy.unwrap_or(Json::Null),
        "blockedSourceList": blocked.unwrap_or_else(|| Json::Array(Vec::new()))})
}

/// Who has the number `number` (E.164) on an all-in-one node: `Some(Own)` for the server's `PhoneNumber`, a paired
/// node's when its copies have it, `None` when neither does (logged when it cannot be read).
pub(crate) async fn number_holder(factory: &std::sync::Arc<ExecutionContextFactory>, number: &str) -> Option<Owner> {
    let key = number.to_string();
    let found = crate::worker::on_pool(&std::sync::Arc::downgrade(factory), move |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let own = claims::own_claim(factory, &facade, ClaimKind::PhoneNumber, &key)?.is_some();
        let replica = if own {
            None
        } else {
            claims::replica_claim(&facade, ClaimKind::PhoneNumber, &key)?
        };
        Ok((own, replica))
    })
    .await;
    match found {
        Ok((false, None)) => None,
        Ok((own, replica)) => Some(number_owner(own, replica, Owner::Own)),
        Err(err) => {
            tracing::error!("SIP: who has the number {number} was not read: {}", err.message());
            None
        }
    }
}

/// The paired node and the SIP device domain of its last Configure (`sipDomain`, E12), lower case; `None` while no
/// node is paired, the domain empty when it sent none.
pub(crate) async fn paired_domain(factory: &std::sync::Arc<ExecutionContextFactory>) -> Option<(String, String)> {
    let read = crate::worker::on_pool(&std::sync::Arc::downgrade(factory), |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let rows = facade
            .find("moqui.edge.EdgeClient")?
            .eq("statusId", Value::string("EdgeClientActive"))
            .select_field("pairedNodeId")
            .select_field("setupJson")
            .disable_authz()
            .limit(2)
            .list()?;
        let [only] = rows.values() else { return Ok(None) };
        let node = only.get("pairedNodeId").map(to_plain_string).unwrap_or_default();
        let setup = only.get("setupJson").map(to_plain_string).unwrap_or_default();
        Ok((!node.is_empty()).then(|| (node, domain_of(&setup))))
    })
    .await;
    read.unwrap_or_else(|err| {
        tracing::warn!("SIP: the paired node's SIP domain was not read: {}", err.message());
        None
    })
}

/// The `sipDomain` of a kept Configure (`EdgeClient.setupJson`), lower case with no trailing dot.
pub(crate) fn domain_of(setup: &str) -> String {
    let setup: Json = serde_json::from_str(setup).unwrap_or_default();
    setup["sipDomain"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

#[cfg(test)]
#[path = "owners_tests.rs"]
mod tests;
