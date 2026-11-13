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

//! Cascading media nodes (plans/archive/Comms-Framework.md §6.2 "The W7 gate", §6.3 "Cluster"): a relay link between the call's
//! media node (its origin) and another node holding some of its legs (a leg node). A link is one str0m peer on each
//! side for one room, on the nodes' media sockets: the origin is ICE-lite, as it is to browsers, and the leg node is
//! full ICE, the controlling side, which offers first. The offers, answers and the rest go between the nodes as
//! [`RelayMessage`]s, which the caller carries (the host by the cluster's send to one node).
//!
//! - Each side sends the other every track of the room it has that did not come from that side: its legs', its injected
//!   sources', and at the origin the other leg nodes'. So the origin sees every leg (its taps: recording, captions, the
//!   SIP mix), and each leg node every leg the origin has.
//! - A video track crosses once, whatever the number of subscribers behind the link, in its simulcast layers up to the
//!   highest any of them wants (a [`RelayMessage::Want`] from the side that receives it, sent when that changes); each
//!   subscriber then gets its own layer as on one node. Nothing wanted, no video crosses. A tap of video at the origin
//!   (the recorder) wants the high layer; an audio-only one (the mixer, captions) none.
//! - Caps on links per room and per node; past them a link is refused explicitly ([`RelayMessage::Refused`]).
//! - Every message names its link; a link opened again (after a loss) has a new one, and the old one's messages are
//!   dropped. A link that is lost or closed is reported by [`SfuEvent::RelayDown`]; what happens to the legs is the
//!   caller's (the host's reconnect path).
//! - Fencing: a room that ended here takes no new link (a leg node's late offer is answered with `Close`, reason
//!   `ended`), and a node that was a leg node of a room never takes a new link of it from its origin (a late offer of a
//!   link it closed). Both are remembered for [`FENCE_KEPT`]; a room's id is never used again.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::{Candidate, Rtc};

use super::peer::Peer;
use super::{Layer, LegKey, PeerId, Sfu, SfuError, SfuEvent, SfuOutput};

/// Which side of a link this node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayRole {
    /// The call's media node: ICE-lite, it answers.
    Origin,
    /// A node with some of the call's legs: full ICE, it offers first.
    LegNode,
}

/// A track a relay sends, by its mid on the sending side, as its offer names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayTrack {
    pub mid: String,
    /// The leg publishing it.
    pub leg: String,
    /// `audio` or `video`.
    pub kind: String,
    /// `microphone`, `camera`, `screen` ([`super::Source`]).
    pub source: String,
    pub simulcast: bool,
}

/// The highest layer the receiving side wants of a publisher's video; `None` stops it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayWant {
    pub leg: String,
    pub layer: Option<String>,
}

/// What two nodes' SFUs tell each other about a link, carried by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RelayMessage {
    Offer {
        link: u64,
        offer_id: u64,
        sdp: String,
        tracks: Vec<RelayTrack>,
    },
    Answer {
        link: u64,
        offer_id: u64,
        sdp: String,
    },
    /// What the sender's tracks carry changed (a source).
    Tracks {
        link: u64,
        tracks: Vec<RelayTrack>,
    },
    Want {
        link: u64,
        wants: Vec<RelayWant>,
    },
    /// The link ends: the room ended, or no leg is left behind it.
    Close {
        link: u64,
        reason: String,
    },
    /// The origin did not take the link: [`SfuError::code`] of why.
    Refused {
        link: u64,
        code: String,
    },
}

impl RelayMessage {
    pub fn link(&self) -> u64 {
        match self {
            RelayMessage::Offer { link, .. }
            | RelayMessage::Answer { link, .. }
            | RelayMessage::Tracks { link, .. }
            | RelayMessage::Want { link, .. }
            | RelayMessage::Close { link, .. }
            | RelayMessage::Refused { link, .. } => *link,
        }
    }
}

/// How long a node remembers a room that ended, or the origin of a room it was a leg node of, to drop late relay
/// messages.
pub(super) const FENCE_KEPT: Duration = Duration::from_secs(600);

/// How often the links' wants and track names are looked at: a want waits at most this, and a node busy with packets
/// does not rebuild them for each one.
pub(super) const RELAY_LOOK_EVERY: Duration = Duration::from_millis(50);

/// A link not connected this long after it opened is dropped (`unreachable`): its offer or answer was lost, or the
/// nodes do not reach each other's media ports.
pub(super) const RELAY_CONNECT_WITHIN: Duration = Duration::from_secs(10);

/// What a node remembers to drop late relay messages: the rooms that ended here, and the origin of each room this node
/// is or was a leg node of, with when each was last touched.
#[derive(Default)]
pub(super) struct Fences {
    ended: HashMap<Arc<str>, Instant>,
    leg_node_of: HashMap<Arc<str>, (Arc<str>, Instant)>,
    /// The time of the SFU's last input.
    pub clock: Option<Instant>,
}

impl Fences {
    pub fn ended(&mut self, now: Instant, room: &str) {
        self.prune(now);
        self.ended.insert(room.into(), now);
        self.leg_node_of.remove(room);
    }

    fn is_ended(&self, room: &str) -> bool {
        self.ended.contains_key(room)
    }

    fn leg_node_of(&mut self, now: Instant, room: &str, origin: &str) {
        self.prune(now);
        self.leg_node_of.insert(room.into(), (origin.into(), now));
    }

    fn origin_of(&self, room: &str) -> Option<&str> {
        self.leg_node_of.get(room).map(|(origin, _)| &**origin)
    }

    fn prune(&mut self, now: Instant) {
        self.ended
            .retain(|_, at| now.saturating_duration_since(*at) < FENCE_KEPT);
        self.leg_node_of
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < FENCE_KEPT);
    }
}

/// The relay side of a [`Peer`].
pub(super) struct Relay {
    pub role: RelayRole,
    /// The node at the other end.
    pub node: Arc<str>,
    pub link: u64,
    /// The other side's tracks, by their mid, from its last offer.
    pub remote: HashMap<String, RelayTrack>,
    /// The wants last sent to the other side.
    pub wants_sent: Option<HashMap<Arc<str>, Option<Layer>>>,
    /// The tracks last named to the other side, by mid: their source.
    pub named: HashMap<String, String>,
    pub needs_channel: bool,
    pub connected: bool,
    /// When this side opened or took the link: one not connected [`RELAY_CONNECT_WITHIN`] after is dropped.
    pub opened: Instant,
    /// Frames sent over the link: audio, video without layers, and each layer low to high.
    pub sent: [u64; 5],
}

/// The leg name a relay peer is kept under in its room (never a callLegId, which is a sequenced id).
fn relay_leg(node: &str) -> String {
    format!("relay:{node}")
}

impl Sfu {
    /// This node is a leg node of `room`, whose media is on `origin`: opens a link to it when there is none. The link
    /// closes when no leg, injected source or tap is left in the room here, or when the origin closes it.
    pub fn link(&mut self, now: Instant, room: &str, origin: &str) -> Result<(), SfuError> {
        if self.relay_id(room, origin).is_some() {
            return Ok(());
        }
        // a room that ended here is not opened again by a late leg or control
        if self.fences.is_ended(room) {
            return Err(SfuError::UnknownRoom);
        }
        if self.relays.len() >= self.config.max_relays {
            return Err(SfuError::TooManyRelays);
        }
        let name = self.room_or_new(room)?;
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_vp8(true)
            .build(now);
        for addr in &self.config.udp_candidates {
            let c = Candidate::host(*addr, "udp").map_err(|e| SfuError::Rtc(e.to_string()))?;
            rtc.add_local_candidate(c);
        }
        let link = self.new_link();
        self.fences.leg_node_of(now, room, origin);
        self.add_relay(now, &name, origin, RelayRole::LegNode, link, rtc);
        Ok(())
    }

    /// The links of a room: the node at the other end, this node's side, and whether it is connected.
    pub fn relays(&self, room: &str) -> Vec<(Arc<str>, RelayRole, bool)> {
        self.relays
            .iter()
            .filter(|((r, _), _)| &**r == room)
            .filter_map(|(_, id)| self.peers.get(id))
            .filter_map(|p| p.relay.as_ref().map(|r| (r.node.clone(), r.role, r.connected)))
            .collect()
    }

    /// The frames a link sent: audio, video without layers, then each layer low to high.
    pub fn relay_sent(&self, room: &str, node: &str) -> Option<[u64; 5]> {
        let id = self.relay_id(room, node)?;
        self.peers.get(&id)?.relay.as_ref().map(|r| r.sent)
    }

    /// Ends a link from this side; the other node is told why.
    pub fn unlink(&mut self, now: Instant, room: &str, node: &str, reason: &str) {
        if let Some(id) = self.relay_id(room, node) {
            self.send_close(id, reason);
            self.remove_relay(id, reason);
        }
        self.touch_all();
        self.drive(now);
    }

    /// A message from the node `from` about its link for `room`. A message of an older link is dropped.
    pub fn handle_relay(
        &mut self,
        now: Instant,
        room: &str,
        from: &str,
        message: RelayMessage,
    ) -> Result<(), SfuError> {
        let link = message.link();
        let current = self.relay_id(room, from);
        let same = current.filter(|id| self.peers.get(id).and_then(|p| p.relay.as_ref()).map(|r| r.link) == Some(link));
        let result = match message {
            RelayMessage::Offer {
                offer_id, sdp, tracks, ..
            } => {
                let id = match same {
                    Some(id) => id,
                    None => {
                        // a leg node's offer for a new link: a link from that node again replaces the old one; an offer
                        // of an origin's link this leg node no longer has is stale
                        if let Some(old) = current {
                            if self.peers.get(&old).and_then(|p| p.relay.as_ref()).map(|r| r.role)
                                == Some(RelayRole::LegNode)
                            {
                                return Ok(());
                            }
                            self.remove_relay(old, "replaced");
                        } else if self.fences.origin_of(room) == Some(from) {
                            // a late offer of a link this leg node closed: the room's origin never opens one
                            return Ok(());
                        }
                        if self.fences.is_ended(room) {
                            // the room ended here: the leg node's late link ends too
                            self.out.push_back(SfuOutput::Event(SfuEvent::Relay {
                                room: room.into(),
                                node: from.into(),
                                message: RelayMessage::Close {
                                    link,
                                    reason: "ended".to_string(),
                                },
                            }));
                            return Ok(());
                        }
                        match self.accept_link(now, room, from, link) {
                            Ok(id) => id,
                            Err(e) => {
                                self.out.push_back(SfuOutput::Event(SfuEvent::Relay {
                                    room: room.into(),
                                    node: from.into(),
                                    message: RelayMessage::Refused {
                                        link,
                                        code: e.code().to_string(),
                                    },
                                }));
                                return Err(e);
                            }
                        }
                    }
                };
                self.relay_offer(now, id, offer_id, &sdp, tracks)
            }
            RelayMessage::Answer { offer_id, sdp, .. } => {
                let Some(id) = same else { return Ok(()) };
                let answer = SdpAnswer::from_sdp_string(&sdp).map_err(|e| SfuError::BadSdp(e.to_string()))?;
                let peer = self.peers.get_mut(&id).ok_or(SfuError::UnknownLeg)?;
                peer.handle_answer(offer_id, answer)
            }
            RelayMessage::Tracks { tracks, .. } => {
                let Some(id) = same else { return Ok(()) };
                if let Some(p) = self.peers.get_mut(&id) {
                    if let Some(r) = p.relay.as_mut() {
                        for t in tracks {
                            r.remote.insert(t.mid.clone(), t);
                        }
                    }
                    p.rename_tracks(&mut self.out);
                }
                Ok(())
            }
            RelayMessage::Want { wants, .. } => {
                let Some(id) = same else { return Ok(()) };
                let config = std::sync::Arc::clone(&self.config);
                if let Some(p) = self.peers.get_mut(&id) {
                    for w in wants {
                        p.set_wanted(&w.leg, w.layer.as_deref().and_then(Layer::parse));
                    }
                    p.choose_layers(&config, now);
                }
                Ok(())
            }
            RelayMessage::Close { reason, .. } => {
                if let Some(id) = same {
                    self.remove_relay(id, &reason);
                }
                Ok(())
            }
            RelayMessage::Refused { code, .. } => {
                if let Some(id) = same {
                    self.remove_relay(id, &format!("refused:{code}"));
                }
                Ok(())
            }
        };
        self.touch_all();
        self.drive(now);
        result
    }

    fn relay_id(&self, room: &str, node: &str) -> Option<PeerId> {
        self.relays
            .get(&(Arc::<str>::from(room), Arc::<str>::from(node)))
            .copied()
    }

    /// A new link id: unique across this node's restarts, so the other node never takes a new link for an old one.
    fn new_link(&mut self) -> u64 {
        self.next_link = self.next_link.wrapping_add(1);
        self.next_link
    }

    /// The origin takes a leg node's new link, within the caps.
    fn accept_link(&mut self, now: Instant, room: &str, from: &str, link: u64) -> Result<PeerId, SfuError> {
        let in_room = self.relays.keys().filter(|(r, _)| &**r == room).count();
        if in_room >= self.config.max_relays_per_room || self.relays.len() >= self.config.max_relays {
            return Err(SfuError::TooManyRelays);
        }
        let name = self.room_or_new(room)?;
        let mut rtc = Rtc::builder()
            .set_ice_lite(true)
            .clear_codecs()
            .enable_opus(true, false)
            .enable_vp8(true)
            .build(now);
        for addr in &self.config.udp_candidates {
            let c = Candidate::host(*addr, "udp").map_err(|e| SfuError::Rtc(e.to_string()))?;
            rtc.add_local_candidate(c);
        }
        Ok(self.add_relay(now, &name, from, RelayRole::Origin, link, rtc))
    }

    fn add_relay(&mut self, now: Instant, room: &Arc<str>, node: &str, role: RelayRole, link: u64, rtc: Rtc) -> PeerId {
        let id = self.next_id;
        self.next_id += 1;
        let key = LegKey::new(room, &relay_leg(node));
        let mut peer = Peer::new(id, key, 0, rtc, now);
        peer.relay = Some(Relay {
            role,
            node: node.into(),
            link,
            remote: HashMap::new(),
            wants_sent: None,
            named: HashMap::new(),
            needs_channel: role == RelayRole::LegNode,
            connected: false,
            opened: now,
            sent: [0; 5],
        });
        if let Some(r) = self.rooms.get_mut(room) {
            for other in r.peers.iter().filter_map(|pid| self.peers.get(pid)) {
                for t in other.tracks_in() {
                    peer.handle_track_open(Arc::downgrade(t));
                }
            }
            for inj in &r.injected {
                peer.handle_track_open(Arc::downgrade(&inj.track));
            }
            r.peers.push(id);
        }
        self.peers.insert(id, peer);
        self.relays.insert((room.clone(), node.into()), id);
        self.touch_all();
        self.drive(now);
        id
    }

    /// The other node's offer on a link: its tracks named, then answered. The leg node does not give way to the
    /// origin's offer while its own is out (glare): the origin gives way to the leg node's and offers again.
    fn relay_offer(
        &mut self,
        now: Instant,
        id: PeerId,
        offer_id: u64,
        sdp: &str,
        tracks: Vec<RelayTrack>,
    ) -> Result<(), SfuError> {
        let offer = SdpOffer::from_sdp_string(sdp).map_err(|e| SfuError::BadSdp(e.to_string()))?;
        let peer = self.peers.get_mut(&id).ok_or(SfuError::UnknownLeg)?;
        let Some(relay) = peer.relay.as_mut() else {
            return Err(SfuError::UnknownLeg);
        };
        if relay.role == RelayRole::LegNode && peer.has_pending_offer() {
            return Ok(());
        }
        let Some(relay) = peer.relay.as_mut() else {
            return Err(SfuError::UnknownLeg);
        };
        relay.remote = tracks.into_iter().map(|t| (t.mid.clone(), t)).collect();
        let (node, link) = (relay.node.clone(), relay.link);
        let answer = peer.handle_offer(offer)?;
        peer.drop_unnamed_tracks(&mut self.out);
        peer.rename_tracks(&mut self.out);
        self.out.push_back(SfuOutput::Event(SfuEvent::Relay {
            room: peer.key.room.clone(),
            node,
            message: RelayMessage::Answer {
                link,
                offer_id,
                sdp: answer.to_sdp_string(),
            },
        }));
        self.touch_all();
        self.drive(now);
        Ok(())
    }

    /// Tells the other node a link ends.
    fn send_close(&mut self, id: PeerId, reason: &str) {
        let Some(p) = self.peers.get(&id) else { return };
        let Some(r) = &p.relay else { return };
        self.out.push_back(SfuOutput::Event(SfuEvent::Relay {
            room: p.key.room.clone(),
            node: r.node.clone(),
            message: RelayMessage::Close {
                link: r.link,
                reason: reason.to_string(),
            },
        }));
    }

    /// Drops a link here: its tracks end on this side's subscribers, and the caller hears why
    /// ([`SfuEvent::RelayDown`]).
    pub(super) fn remove_relay(&mut self, id: PeerId, reason: &str) {
        let Some(mut peer) = self.peers.remove(&id) else { return };
        self.forget_peer(id);
        peer.rtc.disconnect();
        let Some(relay) = peer.relay.take() else { return };
        self.relays.remove(&(peer.key.room.clone(), relay.node.clone()));
        // a link that failed before it ever connected: the nodes do not reach each other's media ports
        let reason = if reason == "lost" && !relay.connected {
            "unreachable"
        } else {
            reason
        };
        if relay.role == RelayRole::LegNode {
            if let Some(now) = self.fences.clock {
                self.fences.leg_node_of(now, &peer.key.room, &relay.node);
            }
        }
        for t in peer.tracks_in() {
            self.out.push_back(SfuOutput::Event(SfuEvent::Track {
                key: LegKey {
                    room: peer.key.room.clone(),
                    leg: t.leg.clone(),
                },
                mid: t.mid.to_string(),
                kind: t.kind(),
                source: t.source(),
                open: false,
            }));
        }
        self.out.push_back(SfuOutput::Event(SfuEvent::RelayDown {
            room: peer.key.room.clone(),
            node: relay.node.clone(),
            reason: reason.to_string(),
        }));
        if let Some(room) = self.rooms.get_mut(&peer.key.room) {
            room.peers.retain(|p| *p != id);
            self.pending.extend(room.peers.iter().copied());
            if room.is_empty() {
                self.rooms.remove(&peer.key.room);
            }
        }
    }

    /// A leg node room with no leg, injected source or tap of its own left closes its links to the origin.
    pub(super) fn close_idle_leg_node(&mut self, room: &Arc<str>) {
        let Some(r) = self.rooms.get(room) else { return };
        if !r.injected.is_empty() || !r.taps.is_empty() {
            return;
        }
        let mut leg_node_links = Vec::new();
        for id in &r.peers {
            match self.peers.get(id).and_then(|p| p.relay.as_ref()) {
                None => return,
                Some(relay) if relay.role == RelayRole::LegNode => leg_node_links.push(*id),
                Some(_) => {}
            }
        }
        for id in leg_node_links {
            self.send_close(id, "empty");
            self.remove_relay(id, "empty");
        }
    }

    /// Ends every link of a room (the call ended), telling the other nodes.
    pub(super) fn close_relays(&mut self, room: &str, reason: &str) {
        let ids: Vec<PeerId> = self
            .relays
            .iter()
            .filter(|((r, _), _)| &**r == room)
            .map(|(_, id)| *id)
            .collect();
        for id in ids {
            self.send_close(id, reason);
            self.remove_relay(id, reason);
        }
    }

    /// Looks at the links every [`RELAY_LOOK_EVERY`] while there is one: drops those that did not connect in time, and
    /// tells the others' nodes of changed wants and track names. Keeps `next` (the
    /// SFU's next timeout) no later than the next look.
    pub(super) fn relays_tick(&mut self, now: Instant, next: &mut Option<Instant>) {
        self.fences.clock = Some(now);
        if self.relays.is_empty() {
            self.relays_due = None;
            return;
        }
        let due = match self.relays_due {
            Some(due) if now < due => due,
            _ => {
                let stalled: Vec<PeerId> = self
                    .relays
                    .values()
                    .copied()
                    .filter(|id| {
                        self.peers.get(id).and_then(|p| p.relay.as_ref()).is_some_and(|r| {
                            !r.connected && now.saturating_duration_since(r.opened) >= RELAY_CONNECT_WITHIN
                        })
                    })
                    .collect();
                for id in stalled {
                    self.send_close(id, "unreachable");
                    self.remove_relay(id, "unreachable");
                }
                self.update_relays();
                // the links' changed wants are polled at the next drive, as every peer was before
                self.pending.extend(self.relays.values().copied());
                now + RELAY_LOOK_EVERY
            }
        };
        self.relays_due = Some(due);
        *next = Some(next.map_or(due, |n| n.min(due)));
    }

    /// Tells each link's other node what this side wants of the video it relays, and what the tracks this side relays
    /// carry, when either changed. The want of a publisher is the highest any subscriber here (a leg, or another link's
    /// node) wants; a room with a tap wants the high layer.
    pub(super) fn update_relays(&mut self) {
        if self.relays.is_empty() {
            return;
        }
        let ids: Vec<PeerId> = self.relays.values().copied().collect();
        for id in ids {
            let Some(p) = self.peers.get(&id) else { continue };
            let Some(room) = self.rooms.get(&p.key.room) else {
                continue;
            };
            // a tap of video (the recorder) wants the high layer; the mixer and captions take only audio
            let tapped = room.taps.iter().any(|(_, tap)| tap.wants_video());
            let mut wants: HashMap<Arc<str>, Option<Layer>> = HashMap::new();
            for t in p.tracks_in().filter(|t| t.kind() == crate::tap::Kind::Video) {
                let mut cap = tapped.then_some(Layer::High);
                for other in room
                    .peers
                    .iter()
                    .filter(|o| **o != id)
                    .filter_map(|o| self.peers.get(o))
                {
                    cap = cap.max(other.wanted_of(&t.leg));
                }
                let entry = wants.entry(t.leg.clone()).or_insert(None);
                *entry = (*entry).max(cap);
            }
            let tracks = p.relay_tracks();
            let Some(relay) = p.relay.as_ref() else { continue };
            let renamed: Vec<_> = tracks
                .iter()
                .filter(|t| relay.named.get(&t.mid).is_some_and(|s| *s != t.source))
                .cloned()
                .collect();
            let (room_name, node, link) = (p.key.room.clone(), relay.node.clone(), relay.link);
            let changed = relay.wants_sent.as_ref() != Some(&wants);
            if let Some(relay) = self.peers.get_mut(&id).and_then(|p| p.relay.as_mut()) {
                relay.named = tracks.into_iter().map(|t| (t.mid, t.source)).collect();
                if changed {
                    relay.wants_sent = Some(wants.clone());
                }
            }
            if !renamed.is_empty() {
                self.out.push_back(SfuOutput::Event(SfuEvent::Relay {
                    room: room_name.clone(),
                    node: node.clone(),
                    message: RelayMessage::Tracks { link, tracks: renamed },
                }));
            }
            if changed {
                let mut wants: Vec<RelayWant> = wants
                    .into_iter()
                    .map(|(leg, layer)| RelayWant {
                        leg: leg.to_string(),
                        layer: layer.map(|l| l.as_str().to_string()),
                    })
                    .collect();
                wants.sort_by(|a, b| a.leg.cmp(&b.leg));
                self.out.push_back(SfuOutput::Event(SfuEvent::Relay {
                    room: room_name,
                    node,
                    message: RelayMessage::Want { link, wants },
                }));
            }
        }
    }
}
