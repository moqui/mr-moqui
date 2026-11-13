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

//! The selective forwarding unit: rooms of legs, one str0m [`Rtc`] per leg, and the forwarding between them
//! (plans/archive/Comms-Framework.md §6.2). A room is a call (its `callSessionId`), a leg a `callLegId`. Each leg's peer is an
//! ICE-lite endpoint on the node's shared media sockets; datagrams are routed to the peer whose ICE session accepts them.
//!
//! - A published track opens a send-only track on every other peer of the room, which the SFU offers to that peer
//!   ([`SfuEvent::Offer`]); the peer's answer comes back by [`Sfu::handle_answer`]. A peer may offer too (ICE restart,
//!   a new track); the SFU is the polite side and drops its own outstanding offer for the peer's.
//! - Video sent in simulcast layers (rids `l`, `m`, `h`) is forwarded one layer per subscriber: the highest the
//!   subscriber asked for ([`Sfu::subscribe`]) that fits its share of the TWCC estimate, switched at a keyframe of the
//!   new layer. Below [`SfuConfig::audio_only_below_kbps`] a subscriber gets audio only.
//! - Every command carries the leg's connection generation; one from an older generation is refused, and a join with a
//!   newer one replaces the old peer.
//! - A room may span media nodes (W7, [`relay`]): a relay link to another node is a peer of the room too, whose tracks in
//!   are that node's legs' tracks.

mod peer;
mod relay;

pub use relay::{RelayMessage, RelayRole, RelayTrack, RelayWant};

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use str0m::bwe::Bitrate;
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::format::Codec;
use str0m::media::{Frequency, MediaKind, MediaTime};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Input, Rtc};

use crate::tap::{Kind, MediaTap, TapFrame};
use peer::{Peer, Propagated, TrackIn};

/// A leg of a room: the call and the leg ids.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LegKey {
    pub room: Arc<str>,
    pub leg: Arc<str>,
}

impl LegKey {
    pub fn new(room: &str, leg: &str) -> Self {
        LegKey {
            room: room.into(),
            leg: leg.into(),
        }
    }
}

/// A datagram transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    Udp,
    Tcp,
}

impl Proto {
    fn from_protocol(p: Protocol) -> Self {
        match p {
            Protocol::Udp => Proto::Udp,
            _ => Proto::Tcp,
        }
    }

    fn protocol(self) -> Protocol {
        match self {
            Proto::Udp => Protocol::Udp,
            Proto::Tcp => Protocol::Tcp,
        }
    }
}

/// A datagram to send from one of the node's media sockets.
#[derive(Debug, Clone)]
pub struct Transmit {
    pub proto: Proto,
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub contents: Vec<u8>,
}

/// What a published track carries, as the publishing client declares it (the `publish` frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Microphone,
    Camera,
    Screen,
    ScreenAudio,
    /// A phone or SIP leg's audio, put in through [`Sfu::inject`].
    Phone,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Microphone => "microphone",
            Source::Camera => "camera",
            Source::Screen => "screen",
            Source::ScreenAudio => "screenAudio",
            Source::Phone => "phone",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "microphone" => Source::Microphone,
            "camera" => Source::Camera,
            "screen" => Source::Screen,
            "screenAudio" => Source::ScreenAudio,
            "phone" => Source::Phone,
            _ => return None,
        })
    }

    fn default_for(kind: MediaKind) -> Self {
        match kind {
            MediaKind::Audio => Source::Microphone,
            MediaKind::Video => Source::Camera,
        }
    }
}

/// A simulcast layer, by its rid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    Low,
    Mid,
    High,
}

impl Layer {
    pub const ALL: [Layer; 3] = [Layer::Low, Layer::Mid, Layer::High];

    pub fn rid(self) -> &'static str {
        match self {
            Layer::Low => "l",
            Layer::Mid => "m",
            Layer::High => "h",
        }
    }

    pub fn from_rid(rid: &str) -> Option<Self> {
        Some(match rid {
            "l" => Layer::Low,
            "m" => Layer::Mid,
            "h" => Layer::High,
            _ => return None,
        })
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Layer::Low,
            "mid" => Layer::Mid,
            "high" => Layer::High,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Low => "low",
            Layer::Mid => "mid",
            Layer::High => "high",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The node's media settings and limits.
#[derive(Debug, Clone)]
pub struct SfuConfig {
    /// The UDP addresses peers send to: the media socket's bound address, or its public address behind 1:1 NAT.
    pub udp_candidates: Vec<SocketAddr>,
    /// The passive ICE-TCP addresses.
    pub tcp_candidates: Vec<SocketAddr>,
    pub max_rooms: usize,
    pub max_legs_per_room: usize,
    /// The TWCC estimate a new peer starts from.
    pub initial_kbps: u64,
    /// Below this estimate a subscriber gets no video.
    pub audio_only_below_kbps: u64,
    /// What each simulcast layer needs, low to high.
    pub layer_kbps: [u64; 3],
    /// The estimate kept for each audio track before video is shared out.
    pub audio_kbps: u64,
    /// The relay links one room may have to other media nodes (W7).
    pub max_relays_per_room: usize,
    /// The relay links this node may have, all rooms together.
    pub max_relays: usize,
}

impl Default for SfuConfig {
    fn default() -> Self {
        SfuConfig {
            udp_candidates: Vec::new(),
            tcp_candidates: Vec::new(),
            max_rooms: 200,
            max_legs_per_room: 50,
            initial_kbps: 600,
            audio_only_below_kbps: 120,
            layer_kbps: [150, 500, 1_500],
            audio_kbps: 40,
            max_relays_per_room: 8,
            max_relays: 64,
        }
    }
}

/// Something the caller passes on: to the leg's signaling connection, or to the call services.
#[derive(Debug, Clone, PartialEq)]
pub enum SfuEvent {
    /// An offer for the leg (a track to add or stop); its answer comes back with the same `offer_id`.
    Offer {
        key: LegKey,
        generation: u64,
        offer_id: u64,
        sdp: String,
    },
    /// ICE and DTLS are up.
    Connected { key: LegKey, generation: u64 },
    /// The peer is gone: ICE failed or timed out, or it was replaced or removed.
    Disconnected { key: LegKey, generation: u64 },
    /// A track a leg publishes opened or ended.
    Track {
        key: LegKey,
        mid: String,
        kind: Kind,
        source: Source,
        open: bool,
    },
    /// The layer a subscriber now gets of a publisher's video; `None` is audio only.
    Layer {
        key: LegKey,
        publisher: Arc<str>,
        layer: Option<Layer>,
    },
    /// A subscriber's TWCC estimate changed.
    Estimate { key: LegKey, kbps: u64 },
    /// The leg now speaking in a room, from the audio level header extension: the loudest above the floor, held for two
    /// looks so a cough does not move the speaker view.
    ActiveSpeaker { room: Arc<str>, leg: Arc<str> },
    /// A message for the node at the other end of a room's relay link (W7), which it passes to its
    /// [`Sfu::handle_relay`].
    Relay {
        room: Arc<str>,
        node: Arc<str>,
        message: RelayMessage,
    },
    /// A relay link is connected.
    RelayUp { room: Arc<str>, node: Arc<str> },
    /// A relay link ended: `lost` (its ICE or DTLS failed after it connected), `unreachable` (it did not connect within
    /// 10 seconds, or failed before it did), `refused:<code>` (the origin did not take it), `empty` (no leg was left
    /// behind it), `replaced` (the leg node opened it again) or the reason the other node gave (`ended`,
    /// `unreachable`).
    RelayDown {
        room: Arc<str>,
        node: Arc<str>,
        reason: String,
    },
}

/// What [`Sfu::poll_output`] gives.
#[derive(Debug)]
pub enum SfuOutput {
    Transmit(Transmit),
    Event(SfuEvent),
    /// Nothing more until this time or the next input.
    Timeout(Instant),
}

/// A refused command. [`SfuError::code`] is the reason code the signaling frame carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfuError {
    TooManyRooms,
    RoomFull,
    /// Past [`SfuConfig::max_relays_per_room`] or [`SfuConfig::max_relays`].
    TooManyRelays,
    /// The command is for an older connection of the leg.
    StaleGeneration,
    UnknownLeg,
    UnknownRoom,
    BadSdp(String),
    Rtc(String),
}

impl SfuError {
    pub fn code(&self) -> &'static str {
        match self {
            SfuError::TooManyRooms => "tooManyRooms",
            SfuError::RoomFull => "roomFull",
            SfuError::TooManyRelays => "tooManyRelays",
            SfuError::StaleGeneration => "staleGeneration",
            SfuError::UnknownLeg => "unknownLeg",
            SfuError::UnknownRoom => "unknownRoom",
            SfuError::BadSdp(_) => "badSdp",
            SfuError::Rtc(_) => "rtcError",
        }
    }
}

impl std::fmt::Display for SfuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SfuError::BadSdp(m) | SfuError::Rtc(m) => write!(f, "{}: {m}", self.code()),
            _ => f.write_str(self.code()),
        }
    }
}

type PeerId = u64;

/// A participant with no browser, whose frames the caller writes.
struct Injected {
    leg: Arc<str>,
    track: Arc<TrackIn>,
}

/// A tap added to a room, by the id [`Sfu::add_tap`] gave it.
pub type TapId = u64;

#[derive(Default)]
struct Room {
    peers: Vec<PeerId>,
    injected: Vec<Injected>,
    taps: Vec<(TapId, Box<dyn MediaTap>)>,
    speaker: Speaker,
}

impl Room {
    /// A room with no peers, injected sources or taps is let go.
    fn is_empty(&self) -> bool {
        self.peers.is_empty() && self.injected.is_empty() && self.taps.is_empty()
    }

    /// Shows a frame to every tap, then drops the taps whose consumer is gone.
    fn tap(&mut self, frame: &TapFrame<'_>) {
        if self.taps.is_empty() {
            return;
        }
        for (_, tap) in &mut self.taps {
            tap.on_frame(frame);
        }
        self.taps.retain(|(_, tap)| !tap.is_closed());
    }

    /// Tells every tap a leg left.
    fn leave(&mut self, leg: &Arc<str>) {
        for (_, tap) in &mut self.taps {
            tap.on_leave(leg);
        }
        self.taps.retain(|(_, tap)| !tap.is_closed());
    }
}

/// The room's active speaker, and the one about to be, by leg.
#[derive(Default)]
struct Speaker {
    current: Option<Arc<str>>,
    candidate: Option<Arc<str>>,
    looks: u8,
    looked: Option<Instant>,
}

/// How often the speaker is looked at again.
const SPEAKER_EVERY: Duration = Duration::from_millis(300);
/// The loudness a speaker must reach (127 plus the level in negative dBov: 67 is louder than -60 dBov).
const SPEAKER_FLOOR: f32 = 67.0;
/// The looks a new speaker must lead.
const SPEAKER_LOOKS: u8 = 2;

/// Counts of the SFU's per-packet work since it started (plans/archive/High-Level-Review.md HL-008), to size the node-wide
/// passes before changing them: each received datagram is offered to peers until one accepts it, and each accepted
/// datagram, timeout and signalling input drives every peer of the node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SfuStats {
    /// Datagrams [`Sfu::handle_receive`] was given.
    pub received: u64,
    /// Datagrams a peer accepted.
    pub accepted: u64,
    /// Peers asked whether they accept a datagram (`Rtc::accepts`), over all datagrams.
    pub accept_checks: u64,
    /// Calls of the all-peer drive.
    pub drives: u64,
    /// Passes of those drives (each polls every peer).
    pub drive_passes: u64,
    /// Peers polled to their timeout, over all passes.
    pub peer_polls: u64,
}

/// The rooms of one media node.
///
/// Only the peers with work are polled (plans/archive/High-Level-Review.md HL-008): the peer a datagram or a command was for,
/// the peers a peer's output fans out to, and the peers whose deadline (str0m's timeout, or the next look at the
/// layers) has come. A datagram is offered first to the peer that last took one on its addresses.
pub struct Sfu {
    config: Arc<SfuConfig>,
    rooms: HashMap<Arc<str>, Room>,
    peers: HashMap<PeerId, Peer>,
    by_leg: HashMap<LegKey, PeerId>,
    /// The relay links, by room and the node at the other end.
    relays: HashMap<(Arc<str>, Arc<str>), PeerId>,
    next_link: u64,
    /// The rooms that ended and the rooms' origins, for dropping late relay messages.
    fences: relay::Fences,
    /// When the links' wants and track names are looked at next ([`relay::RELAY_LOOK_EVERY`]).
    relays_due: Option<Instant>,
    next_id: PeerId,
    out: VecDeque<SfuOutput>,
    next_timeout: Option<Instant>,
    stats: SfuStats,
    /// The peers the next drive polls.
    pending: HashSet<PeerId>,
    /// Each polled peer's deadline, and the peers by deadline.
    deadline_of: HashMap<PeerId, Instant>,
    deadlines: BTreeSet<(Instant, PeerId)>,
    /// The peer that last accepted a datagram of each transport and address pair. Only a first guess: the peer's own
    /// ICE session still decides, and a datagram it does not take is offered to every peer.
    by_addr: HashMap<(Proto, SocketAddr, SocketAddr), PeerId>,
}

/// A peer's deadline this close is due now. str0m's pacer asks to be woken a microsecond after each packet it sends;
/// a clock that does not move between calls (the test network's, or two datagrams in one tick) would otherwise send
/// one packet per tick. Waking a timer early does nothing in str0m but look at it.
const DUE_EARLY: Duration = Duration::from_micros(10);

/// How many passes [`Sfu::drive`] makes before leaving the rest for the next input; each pass forwards what the last
/// produced, so this bounds the work of one input.
const MAX_PASSES: usize = 64;

impl Sfu {
    pub fn new(config: SfuConfig) -> Self {
        Sfu {
            config: Arc::new(config),
            rooms: HashMap::new(),
            peers: HashMap::new(),
            by_leg: HashMap::new(),
            relays: HashMap::new(),
            // a link's id is unique across this node's restarts: the other node drops an old link's messages by it
            next_link: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64),
            fences: relay::Fences::default(),
            relays_due: None,
            next_id: 1,
            out: VecDeque::new(),
            next_timeout: None,
            stats: SfuStats::default(),
            pending: HashSet::new(),
            deadline_of: HashMap::new(),
            deadlines: BTreeSet::new(),
            by_addr: HashMap::new(),
        }
    }

    pub fn config(&self) -> &SfuConfig {
        &self.config
    }

    /// The work counts since this node started ([`SfuStats`]).
    pub fn stats(&self) -> SfuStats {
        self.stats
    }

    /// The legs with a peer or an injected source in a room.
    pub fn legs(&self, room: &str) -> Vec<Arc<str>> {
        let Some(r) = self.rooms.get(room) else {
            return Vec::new();
        };
        let mut legs: Vec<Arc<str>> = r
            .peers
            .iter()
            .filter_map(|id| self.peers.get(id))
            .filter(|p| p.relay.is_none())
            .map(|p| p.key.leg.clone())
            .collect();
        legs.extend(r.injected.iter().map(|i| i.leg.clone()));
        legs
    }

    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// A room's legs here: its peers but the relay links, and its injected sources.
    fn legs_in(&self, room: &Room) -> usize {
        let relays = room
            .peers
            .iter()
            .filter(|id| self.peers.get(id).is_some_and(|p| p.relay.is_some()))
            .count();
        room.peers.len() - relays + room.injected.len()
    }

    /// The generation of a leg's peer, when it has one.
    pub fn generation(&self, key: &LegKey) -> Option<u64> {
        self.by_leg
            .get(key)
            .and_then(|id| self.peers.get(id))
            .map(|p| p.generation)
    }

    /// A leg joins (or rejoins with a newer generation) with its offer; the answer goes back on its signaling connection.
    pub fn join(&mut self, now: Instant, key: &LegKey, generation: u64, offer: &str) -> Result<String, SfuError> {
        if let Some(old) = self.by_leg.get(key).and_then(|id| self.peers.get(id)) {
            if old.generation >= generation {
                return Err(SfuError::StaleGeneration);
            }
        }
        if !self.rooms.contains_key(&key.room) && self.rooms.len() >= self.config.max_rooms {
            return Err(SfuError::TooManyRooms);
        }
        let replacing = self.by_leg.get(key).copied();
        let in_room = self.rooms.get(&key.room).map(|r| self.legs_in(r)).unwrap_or(0);
        if replacing.is_none() && in_room >= self.config.max_legs_per_room {
            return Err(SfuError::RoomFull);
        }
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| SfuError::BadSdp(e.to_string()))?;

        let mut rtc = Rtc::builder()
            .set_ice_lite(true)
            .clear_codecs()
            .enable_opus(true, false)
            .enable_vp8(true)
            .enable_bwe(Some(Bitrate::kbps(self.config.initial_kbps)))
            .build(now);
        for addr in &self.config.udp_candidates {
            let c = Candidate::host(*addr, "udp").map_err(|e| SfuError::Rtc(e.to_string()))?;
            rtc.add_local_candidate(c);
        }
        for addr in &self.config.tcp_candidates {
            let c = Candidate::builder()
                .tcp()
                .host(*addr)
                .tcptype(str0m::net::TcpType::Passive)
                .build()
                .map_err(|e| SfuError::Rtc(e.to_string()))?;
            rtc.add_local_candidate(c);
        }
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| SfuError::BadSdp(e.to_string()))?;

        if let Some(old) = replacing {
            self.remove_peer(old);
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut peer = Peer::new(id, key.clone(), generation, rtc, now);
        let room = self.rooms.entry(key.room.clone()).or_default();
        for other in room.peers.iter().filter_map(|pid| self.peers.get(pid)) {
            for t in other.tracks_in() {
                peer.handle_track_open(Arc::downgrade(t));
            }
        }
        for inj in &room.injected {
            peer.handle_track_open(Arc::downgrade(&inj.track));
        }
        room.peers.push(id);
        self.peers.insert(id, peer);
        self.by_leg.insert(key.clone(), id);
        self.pending.insert(id);
        self.drive(now);
        Ok(answer.to_sdp_string())
    }

    /// The leg's answer to an [`SfuEvent::Offer`]. An answer to an offer the SFU dropped (glare) is ignored.
    pub fn handle_answer(
        &mut self,
        now: Instant,
        key: &LegKey,
        generation: u64,
        offer_id: u64,
        answer: &str,
    ) -> Result<(), SfuError> {
        let answer = SdpAnswer::from_sdp_string(answer).map_err(|e| SfuError::BadSdp(e.to_string()))?;
        let id = self.peer_id(key, generation)?;
        self.pending.insert(id);
        let peer = self.peers.get_mut(&id).ok_or(SfuError::UnknownLeg)?;
        peer.handle_answer(offer_id, answer)?;
        self.drive(now);
        Ok(())
    }

    /// An offer from the leg (an ICE restart, a track added or removed); returns the answer.
    pub fn handle_offer(
        &mut self,
        now: Instant,
        key: &LegKey,
        generation: u64,
        offer: &str,
    ) -> Result<String, SfuError> {
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| SfuError::BadSdp(e.to_string()))?;
        let id = self.peer_id(key, generation)?;
        self.pending.insert(id);
        let peer = self.peers.get_mut(&id).ok_or(SfuError::UnknownLeg)?;
        let answer = peer.handle_offer(offer)?;
        self.drive(now);
        Ok(answer.to_sdp_string())
    }

    /// A trickled remote candidate (`candidate:...`, as in the browser's `RTCIceCandidate.candidate`).
    pub fn add_candidate(&mut self, key: &LegKey, generation: u64, candidate: &str) -> Result<(), SfuError> {
        let text = candidate.strip_prefix("a=").unwrap_or(candidate);
        let c = Candidate::from_sdp_string(text).map_err(|e| SfuError::BadSdp(e.to_string()))?;
        let id = self.peer_id(key, generation)?;
        self.pending.insert(id);
        self.peer_mut(key, generation)?.rtc.add_remote_candidate(c);
        Ok(())
    }

    /// What a leg's track at `mid` carries (camera, screen ...), from its `publish` frame.
    pub fn set_source(&mut self, key: &LegKey, generation: u64, mid: &str, source: Source) -> Result<(), SfuError> {
        let id = self.peer_id(key, generation)?;
        self.pending.insert(id);
        self.peer_mut(key, generation)?.set_source(mid, source);
        Ok(())
    }

    /// The layer a subscriber wants of a publisher's video, by its tile size; `None` stops the video.
    pub fn subscribe(
        &mut self,
        now: Instant,
        key: &LegKey,
        generation: u64,
        publisher: &str,
        layer: Option<Layer>,
    ) -> Result<(), SfuError> {
        let config = Arc::clone(&self.config);
        let id = self.peer_id(key, generation)?;
        self.pending.insert(id);
        let peer = self.peers.get_mut(&id).ok_or(SfuError::UnknownLeg)?;
        peer.set_wanted(publisher, layer);
        peer.choose_layers(&config, now);
        self.drive(now);
        Ok(())
    }

    /// The leg leaves; its tracks stop on the other peers.
    pub fn leave(&mut self, now: Instant, key: &LegKey, generation: u64) -> Result<(), SfuError> {
        let id = self.peer_id(key, generation)?;
        self.remove_peer(id);
        self.drive(now);
        Ok(())
    }

    /// Ends a room: every peer is removed and its injected sources and taps dropped (their consumers see the queue
    /// close).
    pub fn close_room(&mut self, now: Instant, room: &str) {
        self.close_relays(room, "ended");
        self.fences.ended(now, room);
        let Some(r) = self.rooms.get(room) else { return };
        for id in r.peers.clone() {
            self.remove_peer(id);
        }
        self.rooms.remove(room);
        self.drive(now);
    }

    /// Asks every browser publishing video in a room for a keyframe: a tap's consumer (the recorder) starts a file
    /// only at one, and browsers send one only now and then unless asked.
    pub fn request_keyframes(&mut self, now: Instant, room: &str) {
        let Some(r) = self.rooms.get(room) else { return };
        for id in r.peers.clone() {
            if let Some(p) = self.peers.get_mut(&id) {
                p.request_video_keyframes(now);
                self.pending.insert(id);
            }
        }
        self.drive(now);
    }

    /// Adds a tap that sees every frame published in a room, a browser's or an injected source's; a room has as many as
    /// its consumers (recording, captions, the SIP mixer). The room is made when it has no leg yet, and kept while the
    /// tap is there. A tap whose consumer is gone ([`MediaTap::is_closed`]) is dropped at its next frame.
    pub fn add_tap(&mut self, room: &str, tap: Box<dyn MediaTap>) -> Result<TapId, SfuError> {
        let r = self.room_or_new(room)?;
        let id = self.next_id;
        self.next_id += 1;
        if let Some(r) = self.rooms.get_mut(&r) {
            r.taps.push((id, tap));
        }
        Ok(id)
    }

    /// Removes a tap; false when the room has no such tap.
    pub fn remove_tap(&mut self, room: &str, id: TapId) -> bool {
        let Some(r) = self.rooms.get_mut(room) else {
            return false;
        };
        let before = r.taps.len();
        r.taps.retain(|(tap, _)| *tap != id);
        let removed = r.taps.len() != before;
        if r.is_empty() {
            self.rooms.remove(room);
        } else if let Some((name, _)) = self.rooms.get_key_value(room) {
            let name = name.clone();
            self.close_idle_leg_node(&name);
        }
        removed
    }

    /// The room's name, made when it is not there yet (within [`SfuConfig::max_rooms`]).
    fn room_or_new(&mut self, room: &str) -> Result<Arc<str>, SfuError> {
        if let Some((name, _)) = self.rooms.get_key_value(room) {
            return Ok(name.clone());
        }
        if self.rooms.len() >= self.config.max_rooms {
            return Err(SfuError::TooManyRooms);
        }
        let name: Arc<str> = room.into();
        self.rooms.insert(name.clone(), Room::default());
        Ok(name)
    }

    /// Adds a participant whose audio the caller writes with [`Sfu::inject`]: a phone leg's voice (§5.1). The room is
    /// made when it has no leg yet (a call of phones only).
    pub fn add_injected_source(&mut self, now: Instant, room: &str, leg: &str) -> Result<(), SfuError> {
        let key = LegKey::new(room, leg);
        if self.by_leg.contains_key(&key) {
            return Err(SfuError::StaleGeneration);
        }
        let name = self.room_or_new(room)?;
        let r = self.rooms.get(&name).ok_or(SfuError::UnknownRoom)?;
        if r.injected.iter().any(|i| &*i.leg == leg) {
            return Ok(());
        }
        if self.legs_in(r) >= self.config.max_legs_per_room {
            return Err(SfuError::RoomFull);
        }
        let r = self.rooms.get_mut(&name).ok_or(SfuError::UnknownRoom)?;
        let id = self.next_id;
        self.next_id += 1;
        let track = Arc::new(TrackIn::injected(id, key.leg.clone()));
        for pid in &r.peers {
            if let Some(p) = self.peers.get_mut(pid) {
                p.handle_track_open(Arc::downgrade(&track));
                self.pending.insert(*pid);
            }
        }
        r.injected.push(Injected { leg: key.leg, track });
        self.drive(now);
        Ok(())
    }

    /// Writes one Opus frame of an injected source to every peer of the room, and shows it to the room's taps.
    /// `rtp_time` is at 48 kHz.
    pub fn inject(&mut self, now: Instant, room: &str, leg: &str, rtp_time: u64, data: &[u8]) -> Result<(), SfuError> {
        let r = self.rooms.get_mut(room).ok_or(SfuError::UnknownRoom)?;
        let inj = r.injected.iter().find(|i| &*i.leg == leg).ok_or(SfuError::UnknownLeg)?;
        let track = inj.track.clone();
        r.tap(&TapFrame {
            leg: &track.leg,
            kind: Kind::Audio,
            source: Source::Phone,
            rid: None,
            codec: "opus",
            rtp_time,
            clock_rate: 48_000,
            data,
            keyframe: false,
            audio_level: None,
        });
        let time = MediaTime::new(rtp_time, Frequency::FORTY_EIGHT_KHZ);
        for pid in &r.peers {
            if let Some(p) = self.peers.get_mut(pid) {
                p.write_injected(&track, now, time, data);
                self.pending.insert(*pid);
            }
        }
        self.drive(now);
        Ok(())
    }

    /// Removes an injected source.
    pub fn remove_injected_source(&mut self, now: Instant, room: &str, leg: &str) {
        if let Some(r) = self.rooms.get_mut(room) {
            self.pending.extend(r.peers.iter().copied());
            if let Some(at) = r.injected.iter().position(|i| &*i.leg == leg) {
                let gone = r.injected.remove(at);
                r.leave(&gone.leg);
            }
            if r.is_empty() {
                self.rooms.remove(room);
            } else if let Some((name, _)) = self.rooms.get_key_value(room) {
                let name = name.clone();
                self.close_idle_leg_node(&name);
            }
        }
        self.drive(now);
    }

    /// A datagram arrived on one of the media sockets. False when no peer's ICE session takes it.
    pub fn handle_receive(
        &mut self,
        now: Instant,
        proto: Proto,
        source: SocketAddr,
        destination: SocketAddr,
        contents: &[u8],
    ) -> bool {
        let Ok(contents) = contents.try_into() else {
            return false;
        };
        let input = Input::Receive(
            now,
            Receive {
                proto: proto.protocol(),
                source,
                destination,
                contents,
            },
        );
        self.stats.received += 1;
        let Some(id) = self.accepting_peer(proto, source, destination, &input) else {
            return false;
        };
        self.stats.accepted += 1;
        if let Some(peer) = self.peers.get_mut(&id) {
            peer.handle_input(input);
        }
        self.pending.insert(id);
        self.drive(now);
        true
    }

    /// The peer whose ICE session takes `input`: first the one that took the last datagram on these addresses, else
    /// any, which the addresses then lead to (a peer that moved to a new address, an ICE restart, a relay change).
    fn accepting_peer(
        &mut self,
        proto: Proto,
        source: SocketAddr,
        destination: SocketAddr,
        input: &Input,
    ) -> Option<PeerId> {
        let addresses = (proto, source, destination);
        if let Some(id) = self.by_addr.get(&addresses).copied() {
            self.stats.accept_checks += 1;
            if self.peers.get(&id).is_some_and(|p| p.rtc.accepts(input)) {
                return Some(id);
            }
        }
        let checks = &mut self.stats.accept_checks;
        let id = self
            .peers
            .values()
            .find(|p| {
                *checks += 1;
                p.rtc.accepts(input)
            })
            .map(|p| p.id)?;
        self.by_addr.insert(addresses, id);
        Some(id)
    }

    /// Time passed.
    pub fn handle_timeout(&mut self, now: Instant) {
        let config = Arc::clone(&self.config);
        while let Some(&(due, id)) = self.deadlines.first() {
            if due > now + DUE_EARLY {
                break;
            }
            self.deadlines.pop_first();
            self.deadline_of.remove(&id);
            if let Some(p) = self.peers.get_mut(&id) {
                p.handle_input(Input::Timeout(now));
                p.tick(&config, now);
                self.pending.insert(id);
            }
        }
        self.look_for_speakers(now);
        self.drive(now);
    }

    /// Keeps `id`'s deadline: the earlier of its str0m timeout and its next look at the layers.
    fn set_deadline(&mut self, id: PeerId, deadline: Instant) {
        if let Some(old) = self.deadline_of.insert(id, deadline) {
            self.deadlines.remove(&(old, id));
        }
        self.deadlines.insert((deadline, id));
    }

    /// Drops what this node keeps of a peer that is gone besides the peer itself: its deadline, its pending work and
    /// the addresses that led to it.
    fn forget_peer(&mut self, id: PeerId) {
        if let Some(old) = self.deadline_of.remove(&id) {
            self.deadlines.remove(&(old, id));
        }
        self.pending.remove(&id);
        self.by_addr.retain(|_, peer| *peer != id);
    }

    /// Every peer of the node has work: for the commands that may change any of them (relay links).
    fn touch_all(&mut self) {
        self.pending.extend(self.peers.keys().copied());
    }

    fn look_for_speakers(&mut self, now: Instant) {
        for (name, room) in &mut self.rooms {
            let s = &mut room.speaker;
            if s.looked
                .is_some_and(|at| now.saturating_duration_since(at) < SPEAKER_EVERY)
            {
                continue;
            }
            s.looked = Some(now);
            // by leg: a relay link carries the other node's legs, each with its own loudness
            let loudest = room
                .peers
                .iter()
                .filter_map(|id| self.peers.get(id))
                .flat_map(|p| p.loudness(now))
                .filter(|(_, loud)| *loud >= SPEAKER_FLOOR)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(leg, _)| leg.clone());
            let Some(best) = loudest else {
                s.candidate = None;
                s.looks = 0;
                continue;
            };
            if s.current.as_ref() == Some(&best) {
                s.candidate = None;
                s.looks = 0;
                continue;
            }
            if s.candidate.as_ref() == Some(&best) {
                s.looks += 1;
            } else {
                s.candidate = Some(best);
                s.looks = 1;
                continue;
            }
            if s.looks >= SPEAKER_LOOKS {
                s.current = s.candidate.take();
                s.looks = 0;
                if let Some(leg) = s.current.clone() {
                    self.out.push_back(SfuOutput::Event(SfuEvent::ActiveSpeaker {
                        room: name.clone(),
                        leg,
                    }));
                }
            }
        }
    }

    /// The next datagram or event, or the time of the next timeout.
    pub fn poll_output(&mut self) -> SfuOutput {
        if let Some(o) = self.out.pop_front() {
            return o;
        }
        SfuOutput::Timeout(
            self.next_timeout
                .unwrap_or_else(|| Instant::now() + Duration::from_millis(100)),
        )
    }

    fn peer_id(&self, key: &LegKey, generation: u64) -> Result<PeerId, SfuError> {
        let id = *self.by_leg.get(key).ok_or(SfuError::UnknownLeg)?;
        let p = self.peers.get(&id).ok_or(SfuError::UnknownLeg)?;
        if p.generation != generation {
            return Err(SfuError::StaleGeneration);
        }
        Ok(id)
    }

    fn peer_mut(&mut self, key: &LegKey, generation: u64) -> Result<&mut Peer, SfuError> {
        let id = self.peer_id(key, generation)?;
        self.peers.get_mut(&id).ok_or(SfuError::UnknownLeg)
    }

    fn remove_peer(&mut self, id: PeerId) {
        if self.peers.get(&id).is_some_and(|p| p.relay.is_some()) {
            self.remove_relay(id, "lost");
            return;
        }
        let Some(mut peer) = self.peers.remove(&id) else { return };
        self.forget_peer(id);
        peer.rtc.disconnect();
        if self.by_leg.get(&peer.key) == Some(&id) {
            self.by_leg.remove(&peer.key);
        }
        for t in peer.tracks_in() {
            self.out.push_back(SfuOutput::Event(SfuEvent::Track {
                key: peer.key.clone(),
                mid: t.mid.to_string(),
                kind: t.kind(),
                source: t.source(),
                open: false,
            }));
        }
        self.out.push_back(SfuOutput::Event(SfuEvent::Disconnected {
            key: peer.key.clone(),
            generation: peer.generation,
        }));
        if let Some(room) = self.rooms.get_mut(&peer.key.room) {
            room.peers.retain(|p| *p != id);
            // the others' tracks from it end
            self.pending.extend(room.peers.iter().copied());
            room.leave(&peer.key.leg);
            if room.is_empty() {
                self.rooms.remove(&peer.key.room);
            } else {
                self.close_idle_leg_node(&peer.key.room);
            }
        }
    }

    /// Polls the peers with work to their timeouts and forwards what they produced, which gives the peers it reaches
    /// work, until nothing is left to forward.
    fn drive(&mut self, now: Instant) {
        let config = Arc::clone(&self.config);
        self.stats.drives += 1;
        for _ in 0..MAX_PASSES {
            if self.pending.is_empty() {
                break;
            }
            let mut propagated: Vec<Propagated> = Vec::new();
            let polled: Vec<PeerId> = self.pending.drain().collect();
            self.stats.drive_passes += 1;
            self.stats.peer_polls += polled.len() as u64;
            let mut dead: Vec<PeerId> = Vec::new();
            for &id in &polled {
                let Some(p) = self.peers.get_mut(&id) else { continue };
                let t = p.poll_to_timeout(&config, now, &mut self.out, &mut propagated);
                if !p.rtc.is_alive() {
                    dead.push(id);
                    continue;
                }
                let deadline = t.min(p.next_own_deadline());
                self.set_deadline(id, deadline);
            }
            for id in dead {
                self.remove_peer(id);
            }
            for item in propagated {
                self.propagate(item, now);
            }
        }
        // work left past the passes: the next timeout is now
        let mut next = if self.pending.is_empty() {
            self.deadlines.first().map(|(due, _)| *due)
        } else {
            Some(now)
        };
        self.relays_tick(now, &mut next);
        self.next_timeout = next;
    }

    fn propagate(&mut self, item: Propagated, now: Instant) {
        let origin = item.origin();
        let Some(room_name) = self.peers.get(&origin).map(|p| p.key.room.clone()) else {
            return;
        };
        let Some(room) = self.rooms.get_mut(&room_name) else {
            return;
        };
        if let Propagated::MediaData(_, ref data, ref track) = item {
            if !room.taps.is_empty() {
                let codec = data.params.spec().codec;
                let rid = data.rid.as_ref().map(|r| r.to_string());
                room.tap(&TapFrame {
                    leg: &track.leg,
                    kind: track.kind(),
                    source: track.source(),
                    rid: rid.as_deref(),
                    codec: codec_name(codec),
                    rtp_time: data.time.numer(),
                    clock_rate: data.time.frequency().get(),
                    data: &data.data,
                    keyframe: data.is_keyframe(),
                    audio_level: data.ext_vals.audio_level,
                });
            }
        }
        let config = &self.config;
        for &pid in &room.peers {
            let Some(p) = self.peers.get_mut(&pid) else { continue };
            let reaches = match &item {
                Propagated::TrackOpen(..) | Propagated::MediaData(..) => pid != origin,
                Propagated::KeyframeRequest { to, .. } => pid == *to,
            };
            if !reaches {
                continue;
            }
            match &item {
                Propagated::TrackOpen(_, track) if pid != origin => p.handle_track_open(track.clone()),
                Propagated::MediaData(_, data, track) if pid != origin => {
                    p.handle_media_data_out(config, track, data, now)
                }
                Propagated::KeyframeRequest { to, mid, rid, kind, .. } if pid == *to => {
                    p.request_keyframe(*mid, *rid, *kind, now)
                }
                _ => continue,
            }
            self.pending.insert(pid);
        }
    }
}

fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::Opus => "opus",
        Codec::Vp8 => "vp8",
        Codec::Vp9 => "vp9",
        Codec::H264 => "h264",
        Codec::Av1 => "av1",
        Codec::PCMU => "pcmu",
        Codec::PCMA => "pcma",
        Codec::G722 => "g722",
        _ => "other",
    }
}

#[cfg(test)]
mod testnet;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod audio_tests;

#[cfg(test)]
mod relay_tests;
