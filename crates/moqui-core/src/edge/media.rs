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

//! The edge node as a media node of its local node's calls (plans/archive/Edge-Node.md §5.4, "Calls (WebRTC)", E7), as far as
//! it needs no network.
//!
//! - On an edge node the media node tells what it advertises ([`set_advert`]: its media address and TURN URLs), which
//!   its Hello carries to the local node, instead of registering a `CallMediaNode` row of its own: it has no call
//!   state.
//! - On a local node the link keeps the edge node's row from the Hello and the link's liveness, and keeps the edge
//!   node's TURN secret in memory ([`set_peer_turn`]): the edge node's embedded TURN checks credentials with its own
//!   `turn_auth_secret`, so `make#IceServers` signs the credentials of that node's TURN URLs with it ([`turn_secret`]).
//!   The secret comes only in the up connection's Hello answer, on a link whose keys both ends pinned; it is never
//!   logged, stored or shown.
//! - Its SIP engine (E8) tells its addresses the same way ([`set_sip_advert`]), and the local node keeps its `SipNode`
//!   row from the Hello; the trunks it loaded, with their state, are for its offer ([`set_sip_trunks`], E10 phones).
//! - The media driver keys its rooms by [`room_key`] (E12): the call id alone for a call of this node's own, the
//!   paired node and the call for a call whose signaling came over a link, as call ids from two databases may be
//!   the same. [`split_room`] gives back the node and the call id, for what goes out (a reply's `callSessionId`, the
//!   services a room's work calls, the node its recording is queued to).
//! - **Several pairings** (plans/Edge-Node2.md §4.1, E16). The edge node hands each local node a TURN secret of its
//!   pairing alone (`moqui_rtc::turn::pairing_secret`), and the local node names its pairing in each username
//!   ([`turn_username`]), so the TURN server checks it with that pairing's secret and refuses it once the pairing is
//!   revoked ([`active_pairings`]). A trunk the SIP engine loaded is kept with its pairing, for that pairing's offer
//!   only ([`sip_trunks_of`]).

use std::sync::Mutex;

use moqui_lib::sync::lock;

use crate::context::ExecutionContextFactory;

/// What an edge node's media node advertises: the media address browsers send to, and its TURN URLs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaAdvert {
    pub media_address: String,
    pub turn_urls: Vec<String>,
    /// When the media socket started (epoch milliseconds): a restart since a recording began fails it.
    pub started_ms: i64,
}

#[derive(Default)]
struct Advert(Mutex<Option<MediaAdvert>>);

/// On an edge node, as its media socket starts (or `None` as it stops): what its Hello tells its local nodes.
pub fn set_advert(factory: &ExecutionContextFactory, advert: Option<MediaAdvert>) {
    *lock(&factory.extension(|_| Advert::default()).0) = advert;
}

/// What this node's media node advertises, while its media socket is up.
pub fn advert(factory: &ExecutionContextFactory) -> Option<MediaAdvert> {
    lock(&factory.extension(|_| Advert::default()).0).clone()
}

/// What an edge node's SIP engine advertises (E8): the addresses trunks and phones reach it at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SipAdvert {
    /// For UDP and TCP, `host:port`; empty when that listener is off.
    pub sip_address: String,
    /// For TLS, `host:port`; empty when off.
    pub tls_address: String,
    /// The realm its registrar challenges phones with (`sip_domain`, else `sip_public_address`, else `moqui`).
    pub domain: String,
    /// Its registrar is on (`sip_registrar_enabled`).
    pub registrar: bool,
}

#[derive(Default)]
struct SipAdverts(Mutex<Option<SipAdvert>>);

/// On an edge node, as its SIP engine starts (or `None` as it stops): what its Hello tells its local nodes, which keep
/// its `SipNode` row from it instead of the engine registering one (plans/archive/Edge-Node.md §5.4 "Voice (SIP)").
pub fn set_sip_advert(factory: &ExecutionContextFactory, advert: Option<SipAdvert>) {
    *lock(&factory.extension(|_| SipAdverts::default()).0) = advert;
}

/// What this node's SIP engine advertises, while it runs.
pub fn sip_advert(factory: &ExecutionContextFactory) -> Option<SipAdvert> {
    lock(&factory.extension(|_| SipAdverts::default()).0).clone()
}

#[derive(Default)]
struct SipRealms(Mutex<Option<Vec<(String, String)>>>);

/// On a node that takes links, the pairings and their SIP device domains as its SIP engine's registrar takes them now
/// (E16); `None` as it stops.
pub fn set_sip_realms(factory: &ExecutionContextFactory, realms: Option<Vec<(String, String)>>) {
    *lock(&factory.extension(|_| SipRealms::default()).0) = realms;
}

/// The SIP device domain of the pairing `node` as the SIP engine's registrar takes it now: empty for one it takes in no
/// domain of its own; `None` while the engine has not read the pairings.
pub fn applied_sip_domain(factory: &ExecutionContextFactory, node: &str) -> Option<String> {
    let slot = factory.extension(|_| SipRealms::default());
    let realms = lock(&slot.0);
    let realms = realms.as_ref()?;
    Some(
        realms
            .iter()
            .find(|(paired, _)| paired == node)
            .map(|(_, domain)| domain.clone())
            .unwrap_or_default(),
    )
}

/// A trunk an edge node's SIP engine loaded (from its copy of `voiceTrunks`), with its state: what its offer tells the
/// local node (plans/archive/Edge-Node.md §5.7, E10 phones).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SipTrunkState {
    /// The pairing whose copy it came from (E16): its offer tells that pairing only.
    pub paired_node_id: String,
    pub voice_trunk_id: String,
    pub name: String,
    /// `loaded` (no answer yet), `registered`, `ok` (a trunk that does not register answered OPTIONS), `failed` or
    /// `unregistered`.
    pub state: String,
    /// Why it failed; empty otherwise.
    pub error: String,
}

#[derive(Default)]
struct SipTrunks(Mutex<Vec<SipTrunkState>>);

/// On an edge node, as its SIP engine loads its trunks (empty as it stops): a new trunk is `loaded` until it answers;
/// one it had loaded before keeps the state it had, as a reload (any batch of `voiceTrunks`) does not make a failing
/// trunk work.
pub fn set_sip_trunks(factory: &ExecutionContextFactory, trunks: Vec<SipTrunkState>) {
    let slot = factory.extension(|_| SipTrunks::default());
    let mut held = lock(&slot.0);
    let merged = merge_sip_trunks(&held, trunks);
    *held = merged;
}

/// `trunks` with the state of each one `before` had (by id).
pub fn merge_sip_trunks(before: &[SipTrunkState], trunks: Vec<SipTrunkState>) -> Vec<SipTrunkState> {
    trunks
        .into_iter()
        .map(|mut trunk| {
            if let Some(old) = before
                .iter()
                .find(|old| old.voice_trunk_id == trunk.voice_trunk_id && old.paired_node_id == trunk.paired_node_id)
            {
                trunk.state.clone_from(&old.state);
                trunk.error.clone_from(&old.error);
            }
            trunk
        })
        .collect()
}

/// On an edge node, the trunk `voice_trunk_id` of the pairing `node` answered its registration or ping: its state now.
pub fn note_sip_trunk(factory: &ExecutionContextFactory, node: &str, voice_trunk_id: &str, state: &str, error: &str) {
    let slot = factory.extension(|_| SipTrunks::default());
    let mut trunks = lock(&slot.0);
    if let Some(trunk) = trunks
        .iter_mut()
        .find(|trunk| trunk.voice_trunk_id == voice_trunk_id && trunk.paired_node_id == node)
    {
        trunk.state = state.to_string();
        trunk.error = error.to_string();
    }
}

/// The trunks this node's SIP engine has loaded now, with their state.
pub fn sip_trunks(factory: &ExecutionContextFactory) -> Vec<SipTrunkState> {
    lock(&factory.extension(|_| SipTrunks::default()).0).clone()
}

/// The trunks of the pairing `node` this node's SIP engine has loaded now: what its offer tells that pairing.
pub fn sip_trunks_of(factory: &ExecutionContextFactory, node: &str) -> Vec<SipTrunkState> {
    lock(&factory.extension(|_| SipTrunks::default()).0)
        .iter()
        .filter(|trunk| trunk.paired_node_id == node)
        .cloned()
        .collect()
}

/// The TURN secret of each edge member on a local node, by the member's instance id: the secret, and the id this node
/// paired as.
#[derive(Default)]
struct PeerTurns(Mutex<std::collections::HashMap<String, (String, String)>>);

/// The most edge members whose TURN secrets a local node keeps.
const MAX_PEER_TURNS: usize = 64;

/// On a local node, from a new link's Hello answer: the TURN secret of the edge node or edge cluster member
/// `instance_id` (empty when its TURN is off, which forgets the one before), its pairing's own (E16), and `pairing`, the
/// id this node paired as, which its credentials name ([`turn_username`]). Each member of an edge cluster is a media
/// node of its own (plans/Edge-Node2.md §4.6), so each link keeps its member's; they share `turn_auth_secret` and the
/// pairing row, so the secrets are the same. Unpairing forgets them ([`clear_peer_turn`]); a link that is only lost
/// keeps its member's, for the credentials of calls going on.
pub fn set_peer_turn(factory: &ExecutionContextFactory, instance_id: &str, secret: &str, pairing: &str) {
    if instance_id.is_empty() {
        return;
    }
    let slot = factory.extension(|_| PeerTurns::default());
    let mut peers = lock(&slot.0);
    if secret.is_empty() {
        peers.remove(instance_id);
    } else if peers.len() < MAX_PEER_TURNS || peers.contains_key(instance_id) {
        peers.insert(instance_id.to_string(), (secret.to_string(), pairing.to_string()));
    } else {
        tracing::error!(
            "Edge link: the TURN secret of edge member {instance_id} is not kept, as {MAX_PEER_TURNS} are already"
        );
    }
}

/// On a local node whose edge cluster's list changed: forget the TURN secrets of members gone from it.
pub fn retain_peer_turns(factory: &ExecutionContextFactory, keep: &[String]) {
    lock(&factory.extension(|_| PeerTurns::default()).0).retain(|id, _| keep.contains(id));
}

/// On a local node that unpaired: forget its edge members' TURN secrets.
pub fn clear_peer_turn(factory: &ExecutionContextFactory) {
    lock(&factory.extension(|_| PeerTurns::default()).0).clear();
}

/// The secret the TURN servers of the media node `instance_id` check credentials with: its edge node's, when it is
/// this local node's edge node or one of its edge cluster's members, else none (this node's own `turn_auth_secret` signs them).
pub fn turn_secret(factory: &ExecutionContextFactory, instance_id: &str) -> Option<String> {
    turn_peer(factory, instance_id).map(|(secret, _)| secret)
}

/// [`turn_secret`] with the pairing this node's credentials for that media node name.
pub fn turn_peer(factory: &ExecutionContextFactory, instance_id: &str) -> Option<(String, String)> {
    let slot = factory.extension(|_| PeerTurns::default());
    let peers = lock(&slot.0);
    peers.get(instance_id).cloned()
}

/// Between a leg and its pairing in a TURN username (`moqui_rtc::turn::PAIRING_SEPARATOR`).
const TURN_PAIRING_SEPARATOR: char = '@';

/// The leg part of a TURN username for the leg `leg`: the leg alone for this node's own (`pairing` `None`), else the
/// leg and the pairing it belongs to on the edge node (E16), whose TURN signs it with that pairing's secret.
pub fn turn_username(expiry: u64, leg: &str, pairing: Option<&str>) -> String {
    match pairing.filter(|pairing| !pairing.is_empty()) {
        Some(pairing) => format!("{expiry}:{leg}{TURN_PAIRING_SEPARATOR}{pairing}"),
        None => format!("{expiry}:{leg}"),
    }
}

/// The leg the TURN server knows a leg `leg` of the media driver's room `room` by: with the room's pairing when it is
/// a paired node's ([`turn_username`]), so ending it revokes that pairing's credential only.
pub fn turn_leg(room: &str, leg: &str) -> String {
    match split_room(room).0 {
        Some(node) => format!("{leg}{TURN_PAIRING_SEPARATOR}{node}"),
        None => leg.to_string(),
    }
}

/// What a pairing's TURN secret is derived from besides its id: its key's fingerprint and when it paired, so a node
/// that pairs again under the same id (after a revoke) gets a new secret, and the credentials of the pairing before
/// are refused.
fn turn_salt(row: &crate::entity::EntityValue) -> String {
    let text = |field: &str| {
        row.get(field)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    format!("{}|{}", text("keyFingerprint"), text("pairedDate"))
}

/// On a node that takes links, the active pairings (`EdgeClientActive`) by id, each with what its TURN secret is
/// derived from besides its id: the TURN server takes their legs only, each with that pairing's secret.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn active_pairings(facade: &crate::entity::EntityFacade) -> Result<Vec<(String, String)>, moqui_lib::error::Error> {
    Ok(facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", moqui_lib::object::Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .select_field("keyFingerprint")
        .select_field("pairedDate")
        .order_by("pairedNodeId", false)
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| {
            let node = row.get("pairedNodeId").map(moqui_lib::object::to_plain_string)?;
            (!node.is_empty()).then(|| (node, turn_salt(row)))
        })
        .collect())
}

/// What the TURN secret of the active pairing `node` is derived from besides its id ([`active_pairings`]); `None`
/// for a pairing that is not active.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn pairing_turn_salt(
    facade: &crate::entity::EntityFacade,
    node: &str,
) -> Result<Option<String>, moqui_lib::error::Error> {
    Ok(facade
        .find("moqui.edge.EdgeClient")?
        .eq("pairedNodeId", moqui_lib::object::Value::string(node))
        .eq("statusId", moqui_lib::object::Value::string("EdgeClientActive"))
        .select_field("keyFingerprint")
        .select_field("pairedDate")
        .disable_authz()
        .one()?
        .map(|row| turn_salt(&row)))
}

/// Between a paired node's id and its call's id in a room key: a control character neither holds.
const ROOM_SEPARATOR: char = '\u{1f}';

/// The media driver's room for the call `call`: the call id alone for this node's own call (`node` `None`), else the
/// paired node `node` and the call (plans/archive/Edge-Node.md §10, E12). A call id from `node`'s link is keyed as it is, also
/// one that holds the separator (which none of its own does): [`split_room`] splits at the first one, so the key stays
/// `node`'s and never names another node's call or the server's own.
pub fn room_key(node: Option<&str>, call: &str) -> String {
    match node.filter(|node| !node.is_empty()) {
        Some(node) => format!("{node}{ROOM_SEPARATOR}{call}"),
        None => call.to_string(),
    }
}

/// The paired node of a room key (`None` for this node's own call) and its call id: the reverse of [`room_key`].
pub fn split_room(room: &str) -> (Option<&str>, &str) {
    match room.split_once(ROOM_SEPARATOR) {
        Some((node, call)) if !node.is_empty() => (Some(node), call),
        Some((_, call)) => (None, call),
        None => (None, room),
    }
}

/// The owner of the room `room`: its paired node, else the server's own.
pub fn room_owner(room: &str) -> super::owner::Owner {
    match split_room(room).0 {
        Some(node) => super::owner::Owner::paired(node),
        None => super::owner::Owner::Own,
    }
}

/// Where a node that takes links keeps a paired node's call audio at `location` (a greeting or prompt of that node,
/// fetched by its SIP engine or queued as `callAudio`): the location itself on a dedicated edge node, which has none of
/// its own; on an all-in-one node (E12) the same path under `dbresource://moqui/edge-node/<node>/`, apart from the
/// server's own audio at that path.
pub fn kept_audio_location(factory: &ExecutionContextFactory, node: &str, location: &str) -> String {
    if super::settings(factory).edge_node() {
        return location.to_string();
    }
    paired_audio_location(node, location)
}

/// [`kept_audio_location`] on an all-in-one node.
pub fn paired_audio_location(node: &str, location: &str) -> String {
    let node: String = node
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = location.split_once("://").map_or(location, |(_, path)| path);
    format!("dbresource://moqui/edge-node/{node}/{path}")
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
