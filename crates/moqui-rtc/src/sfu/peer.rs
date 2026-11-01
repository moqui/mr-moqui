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

//! One leg's peer: its str0m [`Rtc`], the tracks it publishes ([`TrackIn`]), the tracks it is sent ([`TrackOut`]) with
//! their negotiation state and simulcast layer, and its TWCC estimate. Modelled on str0m's `examples/chat.rs`, with the
//! signaling over the caller's connection instead of a data channel. A peer may instead be a relay link to another media
//! node ([`Relay`], W7): its tracks in are other legs' tracks, from that node, and its tracks out carry every layer up to
//! the one the other node asks for.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use str0m::bwe::{Bitrate, BweKind};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{
    Direction, KeyframeRequest, KeyframeRequestKind, MediaData, MediaKind, MediaTime, Mid, Rid, Rids, Simulcast,
    SimulcastLayer,
};
use str0m::{Event, IceConnectionState, Input, Output, Rtc};

use super::relay::{Relay, RelayMessage, RelayTrack};
use super::{Layer, LegKey, PeerId, Proto, SfuConfig, SfuError, SfuEvent, SfuOutput, Source, Transmit};
use crate::tap::Kind;

/// A keyframe is asked of a publisher's layer at most this often.
const KEYFRAME_INTERVAL: Duration = Duration::from_millis(500);
/// The layers are chosen again this often, as new layers are seen.
const LAYER_INTERVAL: Duration = Duration::from_secs(1);
/// The gap put between the last frame of one layer and the first of the next, at 90 kHz (a frame at 30 fps).
const SWITCH_GAP: u64 = 3_000;
/// An estimate change smaller than this share is not reported.
const ESTIMATE_REPORT_SHARE: f64 = 0.1;
/// How much a new audio level sample moves the smoothed loudness.
const LOUDNESS_WEIGHT: f32 = 0.3;
/// A leg not heard for this long is silent.
const LOUDNESS_STALE: Duration = Duration::from_secs(1);

/// A track a leg publishes. The publishing peer owns it; subscribers hold a [`Weak`], so it ends with its peer.
#[derive(Debug)]
pub(super) struct TrackIn {
    pub origin: PeerId,
    pub leg: Arc<str>,
    pub mid: Mid,
    kind: MediaKind,
    source: AtomicU8,
    pub injected: bool,
    /// Sent in simulcast layers (rids `l`, `m`, `h`).
    pub simulcast: bool,
}

impl TrackIn {
    fn new(origin: PeerId, leg: Arc<str>, mid: Mid, kind: MediaKind, source: Source, simulcast: bool) -> Self {
        TrackIn {
            origin,
            leg,
            mid,
            kind,
            source: AtomicU8::new(source_code(source)),
            injected: false,
            simulcast,
        }
    }

    /// The audio of an injected source.
    pub fn injected(origin: PeerId, leg: Arc<str>) -> Self {
        TrackIn {
            origin,
            leg,
            mid: Mid::from("inj"),
            kind: MediaKind::Audio,
            source: AtomicU8::new(source_code(Source::Phone)),
            injected: true,
            simulcast: false,
        }
    }

    pub fn kind(&self) -> Kind {
        match self.kind {
            MediaKind::Audio => Kind::Audio,
            MediaKind::Video => Kind::Video,
        }
    }

    pub fn source(&self) -> Source {
        source_of(self.source.load(Ordering::Relaxed))
    }

    pub(super) fn set_source(&self, source: Source) {
        self.source.store(source_code(source), Ordering::Relaxed);
    }
}

fn source_code(s: Source) -> u8 {
    match s {
        Source::Microphone => 0,
        Source::Camera => 1,
        Source::Screen => 2,
        Source::ScreenAudio => 3,
        Source::Phone => 4,
    }
}

fn source_of(c: u8) -> Source {
    match c {
        1 => Source::Camera,
        2 => Source::Screen,
        3 => Source::ScreenAudio,
        4 => Source::Phone,
        _ => Source::Microphone,
    }
}

/// What one peer produced for the others.
pub(super) enum Propagated {
    TrackOpen(PeerId, Weak<TrackIn>),
    MediaData(PeerId, Box<MediaData>, Arc<TrackIn>),
    /// A subscriber asked for a keyframe of a publisher's track.
    KeyframeRequest {
        from: PeerId,
        to: PeerId,
        mid: Mid,
        rid: Option<Rid>,
        kind: KeyframeRequestKind,
    },
}

impl Propagated {
    pub fn origin(&self) -> PeerId {
        match self {
            Propagated::TrackOpen(p, _) | Propagated::MediaData(p, _, _) => *p,
            Propagated::KeyframeRequest { from, .. } => *from,
        }
    }
}

struct TrackInEntry {
    track: Arc<TrackIn>,
    last_keyframe_request: HashMap<Option<Rid>, Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutState {
    ToOpen,
    Negotiating(Mid),
    Open(Mid),
    ToStop(Mid),
    NegotiatingStop(Mid),
}

struct TrackOut {
    track_in: Weak<TrackIn>,
    state: OutState,
    /// The simulcast layers seen of the publisher's track.
    seen: [bool; 3],
    /// The layer chosen; `None` sends no video.
    target: Option<Layer>,
    /// The layer being forwarded (it stays until a keyframe of the target arrives); `None` before the first.
    current: Option<Layer>,
    /// For video without layers: forwarding waits for a keyframe (after a pause).
    waiting_keyframe: bool,
    need_keyframe: bool,
    last_keyframe_request: Option<Instant>,
    /// Added to the publisher's RTP time so a layer switch does not jump the subscriber's timeline.
    offset: u64,
    last_out: Option<u64>,
}

impl TrackOut {
    fn mid(&self) -> Option<Mid> {
        match self.state {
            OutState::ToOpen => None,
            OutState::Negotiating(m) | OutState::Open(m) | OutState::ToStop(m) | OutState::NegotiatingStop(m) => {
                Some(m)
            }
        }
    }

    fn open_mid(&self) -> Option<Mid> {
        match self.state {
            OutState::Open(m) => Some(m),
            _ => None,
        }
    }

    fn simulcast(&self) -> bool {
        self.seen.iter().any(|s| *s)
    }
}

pub(super) struct Peer {
    pub id: PeerId,
    pub key: LegKey,
    pub generation: u64,
    pub rtc: Rtc,
    pending: Option<(u64, SdpPendingOffer)>,
    next_offer_id: u64,
    tracks_in: Vec<TrackInEntry>,
    tracks_out: Vec<TrackOut>,
    sources: HashMap<Mid, Source>,
    wanted: HashMap<Arc<str>, Option<Layer>>,
    estimate_kbps: Option<u64>,
    reported_kbps: Option<u64>,
    last_layers: Instant,
    events: Vec<SfuEvent>,
    /// The smoothed loudness of each leg's microphone this peer publishes (127 plus the audio level, which is in negative
    /// dBov: higher is louder), and when it was last heard: the leg's own, or for a relay each relayed leg's.
    loudness: HashMap<Arc<str>, (f32, Instant)>,
    /// A relay link to another media node, not a leg (W7).
    pub relay: Option<Relay>,
}

impl Peer {
    pub fn new(id: PeerId, key: LegKey, generation: u64, rtc: Rtc, now: Instant) -> Self {
        Peer {
            id,
            key,
            generation,
            rtc,
            pending: None,
            next_offer_id: 1,
            tracks_in: Vec::new(),
            tracks_out: Vec::new(),
            sources: HashMap::new(),
            wanted: HashMap::new(),
            estimate_kbps: None,
            reported_kbps: None,
            last_layers: now,
            events: Vec::new(),
            loudness: HashMap::new(),
            relay: None,
        }
    }

    /// How loud each leg's microphone is now; a leg not heard for a second is left out.
    pub fn loudness(&self, now: Instant) -> impl Iterator<Item = (&Arc<str>, f32)> {
        self.loudness
            .iter()
            .filter(move |(_, (_, at))| now.saturating_duration_since(*at) < LOUDNESS_STALE)
            .map(|(leg, (loud, _))| (leg, *loud))
    }

    /// What a subscriber wants of a publisher's video: a leg by its tile size (high unless it said), a relay what the other
    /// node asked for (nothing until it does).
    pub fn wanted_of(&self, publisher: &str) -> Option<Layer> {
        match (&self.relay, self.wanted.get(publisher)) {
            (_, Some(layer)) => *layer,
            (Some(_), None) => None,
            (None, None) => Some(Layer::High),
        }
    }

    /// The room's event of a published track, under the leg that publishes it (a relayed leg's, not the relay's).
    fn track_event(&self, track: &TrackIn, open: bool) -> SfuEvent {
        SfuEvent::Track {
            key: LegKey {
                room: self.key.room.clone(),
                leg: track.leg.clone(),
            },
            mid: track.mid.to_string(),
            kind: track.kind(),
            source: track.source(),
            open,
        }
    }

    /// A track this peer starts receiving: a leg's own, or for a relay the other node's, as its last offer named it.
    fn open_track_in(
        &mut self,
        mid: Mid,
        kind: MediaKind,
        simulcast: bool,
        out: &mut VecDeque<SfuOutput>,
        propagated: &mut Vec<Propagated>,
    ) {
        let (leg, source) = match &self.relay {
            Some(relay) => {
                let named = relay.remote.get(&mid.to_string());
                let leg = named.map(|t| t.leg.clone()).or_else(|| {
                    self.rtc
                        .media(mid)
                        .map(|m| m.stream_id().to_string())
                        .filter(|s| !s.is_empty())
                });
                let Some(leg) = leg else {
                    tracing::warn!(
                        "call {}: relayed track {mid} names no leg, not forwarded",
                        self.key.room
                    );
                    return;
                };
                let source = named
                    .and_then(|t| Source::parse(&t.source))
                    .unwrap_or_else(|| Source::default_for(kind));
                (Arc::<str>::from(leg), source)
            }
            None => (
                self.key.leg.clone(),
                self.sources
                    .get(&mid)
                    .copied()
                    .unwrap_or_else(|| Source::default_for(kind)),
            ),
        };
        let track = Arc::new(TrackIn::new(self.id, leg, mid, kind, source, simulcast));
        propagated.push(Propagated::TrackOpen(self.id, Arc::downgrade(&track)));
        out.push_back(SfuOutput::Event(self.track_event(&track, true)));
        self.tracks_in.push(TrackInEntry {
            track,
            last_keyframe_request: HashMap::new(),
        });
    }

    /// A relay's tracks in that the other node's last offer no longer names end.
    pub fn drop_unnamed_tracks(&mut self, out: &mut VecDeque<SfuOutput>) {
        let Some(relay) = &self.relay else { return };
        let gone: Vec<usize> = self
            .tracks_in
            .iter()
            .enumerate()
            .filter(|(_, t)| !relay.remote.contains_key(&t.track.mid.to_string()))
            .map(|(i, _)| i)
            .collect();
        for at in gone.into_iter().rev() {
            let track = self.tracks_in.remove(at).track;
            out.push_back(SfuOutput::Event(self.track_event(&track, false)));
        }
    }

    /// The other node changed what its relayed tracks carry (a camera became a screen share).
    pub fn rename_tracks(&mut self, out: &mut VecDeque<SfuOutput>) {
        let Some(relay) = &self.relay else { return };
        let mut events = Vec::new();
        for t in &self.tracks_in {
            let Some(named) = relay.remote.get(&t.track.mid.to_string()) else {
                continue;
            };
            if let Some(source) = Source::parse(&named.source) {
                if t.track.source() != source {
                    t.track.set_source(source);
                    events.push(self.track_event(&t.track, true));
                }
            }
        }
        out.extend(events.into_iter().map(SfuOutput::Event));
    }

    /// An offer of this side's is waiting for its answer.
    pub fn has_pending_offer(&self) -> bool {
        self.pending.is_some()
    }

    pub fn tracks_in(&self) -> impl Iterator<Item = &Arc<TrackIn>> {
        self.tracks_in.iter().map(|t| &t.track)
    }

    pub fn handle_input(&mut self, input: Input) {
        if !self.rtc.is_alive() {
            return;
        }
        if let Err(e) = self.rtc.handle_input(input) {
            tracing::warn!(
                "call {} leg {}: media input failed, leg disconnected: {e}",
                self.key.room,
                self.key.leg
            );
            self.rtc.disconnect();
        }
    }

    /// Polls the peer until its next timeout, sending what is for the caller to `out` and what is for the other peers
    /// to `propagated`.
    pub fn poll_to_timeout(
        &mut self,
        config: &SfuConfig,
        now: Instant,
        out: &mut VecDeque<SfuOutput>,
        propagated: &mut Vec<Propagated>,
    ) -> Instant {
        loop {
            out.extend(self.events.drain(..).map(SfuOutput::Event));
            if !self.rtc.is_alive() {
                return now;
            }
            self.keyframe_requests(now, propagated);
            if self.negotiate_if_needed(out) {
                continue;
            }
            match self.rtc.poll_output() {
                Ok(Output::Timeout(t)) => return t,
                Ok(Output::Transmit(t)) => out.push_back(SfuOutput::Transmit(Transmit {
                    proto: Proto::from_protocol(t.proto),
                    source: t.source,
                    destination: t.destination,
                    contents: t.contents.into(),
                })),
                Ok(Output::Event(e)) => self.handle_event(e, config, now, out, propagated),
                Err(e) => {
                    tracing::warn!(
                        "call {} leg {}: media failed, leg disconnected: {e}",
                        self.key.room,
                        self.key.leg
                    );
                    self.rtc.disconnect();
                    return now;
                }
            }
        }
    }

    fn handle_event(
        &mut self,
        e: Event,
        config: &SfuConfig,
        now: Instant,
        out: &mut VecDeque<SfuOutput>,
        propagated: &mut Vec<Propagated>,
    ) {
        match e {
            Event::Connected => match &mut self.relay {
                Some(relay) => {
                    relay.connected = true;
                    out.push_back(SfuOutput::Event(SfuEvent::RelayUp {
                        room: self.key.room.clone(),
                        node: relay.node.clone(),
                    }));
                }
                None => out.push_back(SfuOutput::Event(SfuEvent::Connected {
                    key: self.key.clone(),
                    generation: self.generation,
                })),
            },
            Event::IceConnectionStateChange(IceConnectionState::Disconnected) => self.rtc.disconnect(),
            Event::MediaAdded(m) => {
                if m.direction.is_receiving() && !self.tracks_in.iter().any(|t| t.track.mid == m.mid) {
                    self.open_track_in(m.mid, m.kind, m.simulcast.is_some(), out, propagated);
                }
            }
            Event::MediaData(data) => {
                let Some(entry) = self.tracks_in.iter().find(|t| t.track.mid == data.mid) else {
                    return;
                };
                let track = entry.track.clone();
                if !data.contiguous && track.kind == MediaKind::Video {
                    self.request_keyframe(data.mid, data.rid, KeyframeRequestKind::Fir, now);
                }
                if track.kind == MediaKind::Audio && track.source() == Source::Microphone {
                    if let Some(level) = data.ext_vals.audio_level {
                        let sample = f32::from(127_i8.saturating_add(level.clamp(-127, 0)));
                        let prior = match self.loudness.get(&track.leg) {
                            Some((loud, at)) if now.saturating_duration_since(*at) < LOUDNESS_STALE => *loud,
                            _ => sample,
                        };
                        self.loudness
                            .insert(track.leg.clone(), (prior + (sample - prior) * LOUDNESS_WEIGHT, now));
                    }
                }
                propagated.push(Propagated::MediaData(self.id, Box::new(data), track));
            }
            Event::KeyframeRequest(req) => self.keyframe_request_from_remote(req, propagated),
            // a publisher stopped sending a track (a screen share ended): it ends on the subscribers; sending again
            // opens it again
            Event::MediaChanged(m) => {
                let held = self.tracks_in.iter().position(|t| t.track.mid == m.mid);
                match (held, m.direction.is_receiving()) {
                    (Some(at), false) => {
                        let gone = self.tracks_in.remove(at).track;
                        out.push_back(SfuOutput::Event(self.track_event(&gone, false)));
                    }
                    (None, true) => {
                        let (kind, simulcast) = self
                            .rtc
                            .media(m.mid)
                            .map(|media| (media.kind(), matches!(media.rids_rx(), Rids::Specific(_))))
                            .unwrap_or((MediaKind::Video, false));
                        self.open_track_in(m.mid, kind, simulcast, out, propagated);
                    }
                    _ => {}
                }
            }
            Event::EgressBitrateEstimate(BweKind::Twcc { estimate, .. }) => {
                let kbps = estimate.as_u64() / 1000;
                self.estimate_kbps = Some(kbps);
                let reported = self.reported_kbps.unwrap_or(0) as f64;
                if self.reported_kbps.is_none() || (kbps as f64 - reported).abs() > reported * ESTIMATE_REPORT_SHARE {
                    self.reported_kbps = Some(kbps);
                    out.push_back(SfuOutput::Event(SfuEvent::Estimate {
                        key: self.key.clone(),
                        kbps,
                    }));
                }
                self.choose_layers(config, now);
            }
            _ => {}
        }
    }

    /// Re-chooses the layers now and then, as layers are first seen.
    pub fn tick(&mut self, config: &SfuConfig, now: Instant) {
        if now.saturating_duration_since(self.last_layers) >= LAYER_INTERVAL {
            self.choose_layers(config, now);
        }
    }

    pub fn set_wanted(&mut self, publisher: &str, layer: Option<Layer>) {
        self.wanted.insert(publisher.into(), layer);
    }

    pub fn set_source(&mut self, mid: &str, source: Source) {
        let mid = Mid::from(mid);
        self.sources.insert(mid, source);
        if let Some(t) = self.tracks_in.iter().find(|t| t.track.mid == mid) {
            if t.track.source() != source {
                t.track.set_source(source);
                self.events.push(SfuEvent::Track {
                    key: self.key.clone(),
                    mid: mid.to_string(),
                    kind: t.track.kind(),
                    source,
                    open: true,
                });
            }
        }
    }

    /// Picks each video track's layer: the highest the subscriber wants that fits its share of the estimate.
    pub fn choose_layers(&mut self, config: &SfuConfig, now: Instant) {
        self.last_layers = now;
        if self.relay.is_some() {
            self.choose_relay_caps();
            return;
        }
        let estimate = self.estimate_kbps.unwrap_or(config.initial_kbps);
        let mut audio = 0u64;
        let mut video = 0u64;
        for o in &self.tracks_out {
            match o.track_in.upgrade().map(|t| t.kind) {
                Some(MediaKind::Audio) => audio += 1,
                Some(MediaKind::Video) => video += 1,
                None => {}
            }
        }
        let share = estimate.saturating_sub(audio * config.audio_kbps) / video.max(1);
        let audio_only = estimate < config.audio_only_below_kbps;
        let mut desired = audio * config.audio_kbps;
        for o in &mut self.tracks_out {
            let Some(t) = o.track_in.upgrade() else { continue };
            if t.kind != MediaKind::Video {
                continue;
            }
            let wanted = self.wanted.get(&t.leg).copied().unwrap_or(Some(Layer::High));
            let target = match wanted {
                None => None,
                Some(_) if audio_only => None,
                Some(w) if o.simulcast() => {
                    let fits = Layer::ALL
                        .iter()
                        .copied()
                        .filter(|l| *l <= w && o.seen[l.index()] && config.layer_kbps[l.index()] <= share)
                        .max();
                    fits.or_else(|| Layer::ALL.iter().copied().find(|l| o.seen[l.index()]))
                }
                Some(w) => Some(w),
            };
            if let Some(w) = wanted {
                desired += config.layer_kbps[w.index()];
            }
            if target != o.target {
                o.target = target;
                if target.is_some() && (!o.simulcast() || target != o.current) {
                    o.need_keyframe = true;
                    if !o.simulcast() {
                        o.waiting_keyframe = true;
                    }
                }
                if target.is_none() {
                    o.current = None;
                }
                self.events.push(SfuEvent::Layer {
                    key: self.key.clone(),
                    publisher: t.leg.clone(),
                    layer: target,
                });
            }
        }
        self.rtc
            .bwe()
            .set_desired_bitrate(Bitrate::kbps(desired.max(config.layer_kbps[0])));
    }

    pub fn handle_track_open(&mut self, track_in: Weak<TrackIn>) {
        self.tracks_out.push(TrackOut {
            track_in,
            state: OutState::ToOpen,
            seen: [false; 3],
            target: Some(Layer::High),
            current: None,
            waiting_keyframe: true,
            need_keyframe: false,
            last_keyframe_request: None,
            offset: 0,
            last_out: None,
        });
    }

    /// A relay forwards each video track's layers up to the one the other node asked for (its cap); it does not share out
    /// an estimate, since the link between nodes is not a subscriber's last mile.
    fn choose_relay_caps(&mut self) {
        for o in &mut self.tracks_out {
            let Some(t) = o.track_in.upgrade() else { continue };
            if t.kind != MediaKind::Video {
                continue;
            }
            let cap = self.wanted.get(&t.leg).copied().flatten();
            if cap != o.target {
                o.target = cap;
                if cap.is_none() {
                    o.current = None;
                }
            }
        }
    }

    /// Forwards a frame to the other node over a relay: every layer up to the cap, each with its own rid and time.
    fn relay_media_data_out(&mut self, track: &Arc<TrackIn>, data: &MediaData) {
        let Some(o) = self
            .tracks_out
            .iter_mut()
            .find(|o| o.track_in.as_ptr() == Arc::as_ptr(track))
        else {
            return;
        };
        let Some(mid) = o.open_mid() else { return };
        let layer = data.rid.as_ref().and_then(|r| Layer::from_rid(r));
        if track.kind == MediaKind::Video {
            let forward = match (layer, o.target) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some(l), Some(cap)) => l <= cap,
            };
            if !forward {
                return;
            }
        }
        let Some(writer) = self.rtc.writer(mid) else { return };
        let Some(pt) = writer.match_params(data.params) else {
            return;
        };
        let writer = match data.ext_vals.audio_level {
            Some(level) => writer.audio_level(level, data.ext_vals.voice_activity.unwrap_or(false)),
            None => writer,
        };
        let writer = match (track.simulcast, data.rid) {
            (true, Some(rid)) => writer.rid(rid),
            _ => writer,
        };
        if let Err(e) = writer.write(pt, data.network_time, data.time, data.data.clone()) {
            tracing::warn!("call {}: relaying failed, the relay is closed: {e}", self.key.room);
            self.rtc.disconnect();
            return;
        }
        if let Some(relay) = &mut self.relay {
            let slot = match (track.kind, layer) {
                (MediaKind::Audio, _) => 0,
                (MediaKind::Video, None) => 1,
                (MediaKind::Video, Some(l)) => 2 + l.index(),
            };
            relay.sent[slot] += 1;
        }
    }

    /// Forwards a frame another peer published to this subscriber, choosing the simulcast layer.
    pub fn handle_media_data_out(&mut self, config: &SfuConfig, track: &Arc<TrackIn>, data: &MediaData, now: Instant) {
        if self.relay.is_some() {
            self.relay_media_data_out(track, data);
            return;
        }
        let Some(o) = self
            .tracks_out
            .iter_mut()
            .find(|o| o.track_in.as_ptr() == Arc::as_ptr(track))
        else {
            return;
        };
        let Some(mid) = o.open_mid() else { return };
        let mut rechoose = false;
        if track.kind == MediaKind::Video {
            let layer = data.rid.as_ref().and_then(|r| Layer::from_rid(r));
            if let Some(l) = layer {
                if !o.seen[l.index()] {
                    o.seen[l.index()] = true;
                    rechoose = true;
                }
            }
            let forward = match (layer, o.target) {
                (_, None) => false,
                (None, Some(_)) => {
                    if o.waiting_keyframe && data.is_keyframe() {
                        o.waiting_keyframe = false;
                    }
                    if o.waiting_keyframe {
                        o.need_keyframe = true;
                    }
                    !o.waiting_keyframe
                }
                (Some(l), Some(target)) if l == target => {
                    if o.current == Some(l) {
                        true
                    } else if data.is_keyframe() {
                        if let Some(last) = o.last_out {
                            o.offset = last.wrapping_add(SWITCH_GAP).wrapping_sub(data.time.numer());
                        }
                        o.current = Some(l);
                        true
                    } else {
                        o.need_keyframe = true;
                        false
                    }
                }
                (Some(l), Some(_)) => o.current == Some(l),
            };
            if !forward {
                if rechoose {
                    self.choose_layers(config, now);
                }
                return;
            }
        }
        let numer = data.time.numer().wrapping_add(o.offset);
        o.last_out = Some(numer);
        let time = MediaTime::new(numer, data.time.frequency());
        let Some(writer) = self.rtc.writer(mid) else { return };
        let Some(pt) = writer.match_params(data.params) else {
            return;
        };
        let writer = match data.ext_vals.audio_level {
            Some(level) => writer.audio_level(level, data.ext_vals.voice_activity.unwrap_or(false)),
            None => writer,
        };
        if let Err(e) = writer.write(pt, data.network_time, time, data.data.clone()) {
            tracing::warn!(
                "call {} leg {}: forwarding failed, leg disconnected: {e}",
                self.key.room,
                self.key.leg
            );
            self.rtc.disconnect();
        }
        if rechoose {
            self.choose_layers(config, now);
        }
    }

    /// Writes an injected source's Opus frame.
    pub fn write_injected(&mut self, track: &Arc<TrackIn>, now: Instant, time: MediaTime, data: &[u8]) {
        let Some(o) = self
            .tracks_out
            .iter()
            .find(|o| o.track_in.as_ptr() == Arc::as_ptr(track))
        else {
            return;
        };
        let Some(mid) = o.open_mid() else { return };
        let Some(writer) = self.rtc.writer(mid) else { return };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == Codec::Opus)
            .map(|p| p.pt())
        else {
            return;
        };
        if let Err(e) = writer.write(pt, now, time, data.to_vec()) {
            tracing::warn!(
                "call {} leg {}: injected audio failed: {e}",
                self.key.room,
                self.key.leg
            );
        }
    }

    /// Asks this peer (a publisher) for a keyframe of its track at `mid`, at most every [`KEYFRAME_INTERVAL`] a layer.
    pub fn request_keyframe(&mut self, mid: Mid, rid: Option<Rid>, kind: KeyframeRequestKind, now: Instant) {
        let Some(entry) = self.tracks_in.iter_mut().find(|t| t.track.mid == mid) else {
            return;
        };
        if entry
            .last_keyframe_request
            .get(&rid)
            .is_some_and(|t| now.saturating_duration_since(*t) < KEYFRAME_INTERVAL)
        {
            return;
        }
        let Some(mut writer) = self.rtc.writer(mid) else { return };
        if writer.request_keyframe(rid, kind).is_ok() {
            entry.last_keyframe_request.insert(rid, now);
        }
    }

    /// Asks this peer for a keyframe of each video track it publishes, every layer it sends (a room's consumer that
    /// starts a file, the recorder). Each layer keeps its [`KEYFRAME_INTERVAL`].
    pub fn request_video_keyframes(&mut self, now: Instant) {
        let mids: Vec<Mid> = self
            .tracks_in
            .iter()
            .filter(|t| t.track.kind == MediaKind::Video && !t.track.injected)
            .map(|t| t.track.mid)
            .collect();
        for mid in mids {
            // a layer the browser does not send is no stream: str0m refuses that request, and nothing goes out
            let layers = Layer::ALL.iter().map(|l| Some(Rid::from(l.rid())));
            for rid in std::iter::once(None).chain(layers) {
                self.request_keyframe(mid, rid, KeyframeRequestKind::Pli, now);
            }
        }
    }

    /// A subscriber's PLI or FIR goes to the publisher, for the layer the subscriber gets.
    fn keyframe_request_from_remote(&mut self, req: KeyframeRequest, propagated: &mut Vec<Propagated>) {
        let Some(o) = self.tracks_out.iter().find(|o| o.mid() == Some(req.mid)) else {
            return;
        };
        let Some(t) = o.track_in.upgrade() else { return };
        if t.injected {
            return;
        }
        // a relay passes on the layer the other node's subscriber asked for
        let rid = if self.relay.is_some() {
            req.rid
        } else {
            o.current
                .or(o.target)
                .filter(|_| o.simulcast())
                .map(|l| Rid::from(l.rid()))
        };
        propagated.push(Propagated::KeyframeRequest {
            from: self.id,
            to: t.origin,
            mid: t.mid,
            rid,
            kind: req.kind,
        });
    }

    /// The keyframes this subscriber's layer choices need.
    fn keyframe_requests(&mut self, now: Instant, propagated: &mut Vec<Propagated>) {
        // a relay's tracks are asked for by the other node's subscribers, each for its own layer
        let relay = self.relay.is_some();
        for o in &mut self.tracks_out {
            if !o.need_keyframe {
                continue;
            }
            if relay {
                o.need_keyframe = false;
                continue;
            }
            if o.last_keyframe_request
                .is_some_and(|t| now.saturating_duration_since(t) < KEYFRAME_INTERVAL)
            {
                continue;
            }
            let Some(t) = o.track_in.upgrade() else { continue };
            if t.injected || t.kind != MediaKind::Video {
                o.need_keyframe = false;
                continue;
            }
            o.need_keyframe = false;
            o.last_keyframe_request = Some(now);
            let rid = o.target.filter(|_| o.simulcast()).map(|l| Rid::from(l.rid()));
            propagated.push(Propagated::KeyframeRequest {
                from: self.id,
                to: t.origin,
                mid: t.mid,
                rid,
                kind: KeyframeRequestKind::Pli,
            });
        }
    }

    /// Offers the tracks to add and stop, when no offer is outstanding.
    fn negotiate_if_needed(&mut self, out: &mut VecDeque<SfuOutput>) -> bool {
        if self.pending.is_some() {
            return false;
        }
        for o in &mut self.tracks_out {
            if let OutState::Open(m) = o.state {
                if o.track_in.upgrade().is_none() {
                    o.state = OutState::ToStop(m);
                }
            }
        }
        self.tracks_out
            .retain(|o| !(o.state == OutState::ToOpen && o.track_in.upgrade().is_none()));
        let relay = self.relay.is_some();
        let mut change = self.rtc.sdp_api();
        if let Some(r) = self.relay.as_mut().filter(|r| r.needs_channel) {
            // a link with no track yet still connects, so the first track does not wait for ICE and DTLS
            change.add_channel("moqui-relay".to_string());
            r.needs_channel = false;
        }
        for o in &mut self.tracks_out {
            match o.state {
                OutState::ToOpen => {
                    if let Some(t) = o.track_in.upgrade() {
                        let track_id = format!("{}:{}", t.leg, t.mid);
                        // a relay passes a simulcast track on in its layers, for the other node's subscribers to choose
                        let layers = (relay && t.simulcast && t.kind == MediaKind::Video).then(|| {
                            let mut s = Simulcast::new();
                            for l in Layer::ALL {
                                s.add_send_layer(SimulcastLayer::new(l.rid()));
                            }
                            s
                        });
                        let mid = change.add_media(
                            t.kind,
                            Direction::SendOnly,
                            Some(t.leg.to_string()),
                            Some(track_id),
                            layers,
                        );
                        o.state = OutState::Negotiating(mid);
                    }
                }
                OutState::ToStop(m) => {
                    change.stop_media(m);
                    o.state = OutState::NegotiatingStop(m);
                }
                _ => {}
            }
        }
        if !change.has_changes() {
            return false;
        }
        let Some((offer, pending)) = change.apply() else {
            return false;
        };
        let offer_id = self.next_offer_id;
        self.next_offer_id += 1;
        self.pending = Some((offer_id, pending));
        if self.relay.is_some() {
            let tracks = self.relay_tracks();
            let Some(relay) = self.relay.as_mut() else { return true };
            // what the offer names each track as, so a later change of its source is told ([`RelayMessage::Tracks`])
            for t in &tracks {
                relay.named.insert(t.mid.clone(), t.source.clone());
            }
            out.push_back(SfuOutput::Event(SfuEvent::Relay {
                room: self.key.room.clone(),
                node: relay.node.clone(),
                message: RelayMessage::Offer {
                    link: relay.link,
                    offer_id,
                    sdp: offer.to_sdp_string(),
                    tracks,
                },
            }));
            return true;
        }
        out.push_back(SfuOutput::Event(SfuEvent::Offer {
            key: self.key.clone(),
            generation: self.generation,
            offer_id,
            sdp: offer.to_sdp_string(),
        }));
        true
    }

    /// What a relay sends the other node: each track being offered or open, by its mid here.
    pub fn relay_tracks(&self) -> Vec<RelayTrack> {
        self.tracks_out
            .iter()
            .filter(|o| matches!(o.state, OutState::Negotiating(_) | OutState::Open(_)))
            .filter_map(|o| {
                let t = o.track_in.upgrade()?;
                Some(RelayTrack {
                    mid: o.mid()?.to_string(),
                    leg: t.leg.to_string(),
                    kind: if t.kind == MediaKind::Audio { "audio" } else { "video" }.to_string(),
                    source: t.source().as_str().to_string(),
                    simulcast: t.simulcast && t.kind == MediaKind::Video,
                })
            })
            .collect()
    }

    pub fn handle_answer(&mut self, offer_id: u64, answer: SdpAnswer) -> Result<(), SfuError> {
        match self.pending.take() {
            Some((id, pending)) if id == offer_id => {
                self.rtc
                    .sdp_api()
                    .accept_answer(pending, answer)
                    .map_err(|e| SfuError::BadSdp(e.to_string()))?;
                for o in &mut self.tracks_out {
                    if let OutState::Negotiating(m) = o.state {
                        o.state = OutState::Open(m);
                        o.need_keyframe = true;
                    }
                }
                self.tracks_out
                    .retain(|o| !matches!(o.state, OutState::NegotiatingStop(_)));
                Ok(())
            }
            other => {
                // An answer to an offer dropped for the peer's own (glare) or already answered.
                self.pending = other;
                Ok(())
            }
        }
    }

    /// The peer's offer wins over an outstanding one of ours, which is offered again after.
    pub fn handle_offer(&mut self, offer: SdpOffer) -> Result<SdpAnswer, SfuError> {
        self.pending = None;
        let answer = self
            .rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| SfuError::BadSdp(e.to_string()))?;
        for o in &mut self.tracks_out {
            match o.state {
                OutState::Negotiating(_) => o.state = OutState::ToOpen,
                OutState::NegotiatingStop(m) => o.state = OutState::ToStop(m),
                _ => {}
            }
        }
        Ok(answer)
    }
}
