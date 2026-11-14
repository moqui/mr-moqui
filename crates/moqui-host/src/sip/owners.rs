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
//! - **Trunks** come from every side ([`merge_config`]): the server's `get#EngineConfig` and each pairing's copy of
//!   `voiceTrunks` (E16). The dial policy and the blocked sources are the server's, else the first pairing's.
//! - **An inbound call** goes by the number called ([`number_owner`]): the server's own `PhoneNumber`, else the
//!   pairing that claims the number (a number of its trunks, or one with a fallback; a second pairing's copy of it is
//!   held back as a replica conflict, E16), else the owner of the trunk it came on.
//! - **Phones** go by the domain they register to ([`realm_owner`]): the server's `sip_domain` (the registrar's own
//!   realm) is the server's, the domain of a pairing's Configure (`sipDomain`) is that pairing's ([`pairing_domains`]),
//!   and a domain two pairings name is refused. On a dedicated edge node the registrar's own realm is the one
//!   pairing's while only one is paired (E8).

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::edge::claims::{self, ClaimKind};
use moqui_core::edge::owner::Owner;
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

/// The owner of an inbound call's number: the server's when it has the number (`own`), else the pairing whose copies
/// hold it (`holder`, its claim: a second pairing's copy of a number is held back as a replica conflict), else the
/// owner of the trunk it came on.
pub(crate) fn number_owner(own: bool, holder: Option<String>, trunk: Owner) -> Owner {
    if own {
        return Owner::Own;
    }
    match holder.filter(|node| !node.is_empty()) {
        Some(node) => Owner::paired(&node),
        None => trunk,
    }
}

/// Whose phones register in `realm`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RealmOwner {
    Is(Owner),
    /// No one's: refused as a failed sign-in would be, or (a dedicated edge node with no pairing) answered 503 until
    /// one pairs.
    NoOne,
    /// Two pairings name it: refused, logged with both.
    Claimed(String),
}

/// Whose phones register in `realm` (E12, E16): the server's own in its realm `own` (not on a dedicated edge node,
/// which has no phones of its own); else the one pairing whose domain it is (`pairings`, each pairing and its domain);
/// else, on a dedicated edge node, the registrar's own realm `own` is the only pairing's while one is paired (E8).
pub(crate) fn realm_owner(realm: &str, own: &str, dedicated: bool, pairings: &[(String, String)]) -> RealmOwner {
    let realm = realm.trim().trim_end_matches('.');
    if !dedicated && realm.eq_ignore_ascii_case(own) {
        return RealmOwner::Is(Owner::Own);
    }
    let named: Vec<&str> = pairings
        .iter()
        .filter(|(_, domain)| !domain.is_empty() && domain.eq_ignore_ascii_case(realm))
        .map(|(node, _)| node.as_str())
        .collect();
    match named.as_slice() {
        [node] => RealmOwner::Is(Owner::paired(node)),
        [] => match pairings {
            [(only, _)] if dedicated && realm.eq_ignore_ascii_case(own) => RealmOwner::Is(Owner::paired(only)),
            _ => RealmOwner::NoOne,
        },
        several => RealmOwner::Claimed(format!("the local nodes {} each name it", several.join(", "))),
    }
}

/// The engine's key of the phone `username` of `owner` among the registrar's bindings: a pairing's phone registered
/// in its own domain is keyed by the domain, as the registrar keys the bindings of a device realm
/// (`moqui_sip::ua::aor_key`), so two pairings' phones of one username stay apart; the server's own, and a phone in
/// a dedicated edge node's own realm, by the username alone.
pub(crate) fn phone_key(username: &str, owner: &Owner, pairings: &[(String, String)]) -> String {
    let domain = owner
        .node()
        .and_then(|node| pairings.iter().find(|(paired, _)| paired == node))
        .map(|(_, domain)| domain.as_str())
        .filter(|domain| !domain.is_empty());
    moqui_sip::ua::aor_key(username, domain)
}

/// The engine's configuration from the server's own (`own`, `get#EngineConfig`'s answer, `None` on a dedicated edge
/// node) and each pairing's (`paired`, its node and the answer from its copy, E8, E16): every trunk of each, a
/// pairing's keyed by its node; the policy and the blocked sources of the server's own when it has them, else the
/// first pairing's.
pub(crate) fn merge_config(own: Option<Json>, paired: &[(String, Json)]) -> Json {
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
    for (node, config) in paired {
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

/// The owner of an inbound call to the number `number` (E.164) on a node that takes links ([`number_owner`]): the
/// server's `PhoneNumber` (not on a dedicated edge node, which has none), else the pairing that claims it
/// (`claims::replica_claim`: its `PhoneNumber` or `PhoneNumberFallback` copy; plans/Edge-Node2.md §4.1), else
/// `trunk`, the owner of the trunk it came on. A number that cannot be read goes by its trunk (logged).
pub(crate) async fn number_route(
    factory: &std::sync::Arc<ExecutionContextFactory>,
    number: &str,
    trunk: Owner,
    dedicated: bool,
) -> Owner {
    let key = number.to_string();
    let found = crate::worker::on_pool(&std::sync::Arc::downgrade(factory), move |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        let own = !dedicated && claims::own_claim(factory, &facade, ClaimKind::PhoneNumber, &key)?.is_some();
        let holder = if own {
            None
        } else {
            claims::replica_claim(&facade, ClaimKind::PhoneNumber, &key)?
        };
        Ok((own, holder))
    })
    .await;
    match found {
        Ok((own, holder)) => number_owner(own, holder, trunk),
        Err(err) => {
            tracing::error!("SIP: who has the number {number} was not read: {}", err.message());
            trunk
        }
    }
}

/// Each active pairing and the SIP device domain of its last Configure (`sipDomain`, E12, E16), lower case, in id
/// order (empty while none is paired); `None` when they could not be read (logged).
pub(crate) async fn pairing_domains(
    factory: &std::sync::Arc<ExecutionContextFactory>,
) -> Option<Vec<(String, String)>> {
    let read = crate::worker::on_pool(&std::sync::Arc::downgrade(factory), |factory| {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
        Ok(moqui_core::edge::sip::pairing_domains(&facade)?)
    })
    .await;
    read.map_err(|err| tracing::warn!("SIP: the paired nodes' SIP domains were not read: {}", err.message()))
        .ok()
}

#[cfg(test)]
#[path = "owners_tests.rs"]
mod tests;
