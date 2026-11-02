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
//!   row from the Hello.

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
