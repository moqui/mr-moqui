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

//! The edge node as a media node of its local node's calls (plans/Edge-Node.md §5.4, "Calls (WebRTC)", E7), as far as
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
/// its `SipNode` row from it instead of the engine registering one (plans/Edge-Node.md §5.4 "Voice (SIP)").
pub fn set_sip_advert(factory: &ExecutionContextFactory, advert: Option<SipAdvert>) {
    *lock(&factory.extension(|_| SipAdverts::default()).0) = advert;
}

/// What this node's SIP engine advertises, while it runs.
pub fn sip_advert(factory: &ExecutionContextFactory) -> Option<SipAdvert> {
    lock(&factory.extension(|_| SipAdverts::default()).0).clone()
}

/// A trunk an edge node's SIP engine loaded (from its copy of `voiceTrunks`), with its state: what its offer tells the
/// local node (plans/Edge-Node.md §5.7, E10 phones).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SipTrunkState {
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
            if let Some(old) = before.iter().find(|old| old.voice_trunk_id == trunk.voice_trunk_id) {
                trunk.state.clone_from(&old.state);
                trunk.error.clone_from(&old.error);
            }
            trunk
        })
        .collect()
}

/// On an edge node, a trunk's registration or ping answered: its state now.
pub fn note_sip_trunk(factory: &ExecutionContextFactory, voice_trunk_id: &str, state: &str, error: &str) {
    let slot = factory.extension(|_| SipTrunks::default());
    let mut trunks = lock(&slot.0);
    if let Some(trunk) = trunks.iter_mut().find(|trunk| trunk.voice_trunk_id == voice_trunk_id) {
        trunk.state = state.to_string();
        trunk.error = error.to_string();
    }
}

/// The trunks this node's SIP engine has loaded now, with their state.
pub fn sip_trunks(factory: &ExecutionContextFactory) -> Vec<SipTrunkState> {
    lock(&factory.extension(|_| SipTrunks::default()).0).clone()
}

/// The edge node's TURN secret on a local node, by the edge node's instance id.
struct PeerTurn {
    instance_id: String,
    secret: String,
}

#[derive(Default)]
struct PeerTurns(Mutex<Option<PeerTurn>>);

/// On a local node, from a new link's Hello answer: the TURN secret of the edge node `instance_id` (empty when its TURN
/// is off, which forgets the one before). Unpairing forgets it ([`clear_peer_turn`]); a link that is only lost keeps
/// it, for the credentials of calls going on.
pub fn set_peer_turn(factory: &ExecutionContextFactory, instance_id: &str, secret: &str) {
    let slot = factory.extension(|_| PeerTurns::default());
    *lock(&slot.0) = (!instance_id.is_empty() && !secret.is_empty()).then(|| PeerTurn {
        instance_id: instance_id.to_string(),
        secret: secret.to_string(),
    });
}

/// On a local node that unpaired: forget its edge node's TURN secret.
pub fn clear_peer_turn(factory: &ExecutionContextFactory) {
    *lock(&factory.extension(|_| PeerTurns::default()).0) = None;
}

/// The secret the TURN servers of the media node `instance_id` check credentials with: its edge node's, when it is
/// this local node's edge node, else none (this node's own `turn_auth_secret` signs them).
pub fn turn_secret(factory: &ExecutionContextFactory, instance_id: &str) -> Option<String> {
    let slot = factory.extension(|_| PeerTurns::default());
    let peer = lock(&slot.0);
    peer.as_ref()
        .filter(|peer| !instance_id.is_empty() && peer.instance_id == instance_id)
        .map(|peer| peer.secret.clone())
}

/// Between a paired node's id and its call's id in a room key: a control character neither holds.
const ROOM_SEPARATOR: char = '\u{1f}';

/// The media driver's room for the call `call`: the call id alone for this node's own call (`node` `None`), else the
/// paired node `node` and the call (plans/Edge-Node.md §10, E12). A call id from `node`'s link is keyed as it is, also
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
