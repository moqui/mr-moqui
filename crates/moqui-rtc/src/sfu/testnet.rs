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

//! An in-memory network for the SFU tests: str0m client peers (standing in for browsers) and the [`Sfu`] exchange
//! datagrams through queues on a simulated clock, with a latency and, per client, a downlink capacity that queues and
//! then drops what exceeds it (so TWCC sees congestion). No sockets, no threads, no wall-clock waits.
//!
//! With [`Net::with_leg_node`] a second node's [`Sfu`] (the leg node, W7) is on the network too: a client may join
//! there, and the two SFUs' relay messages go between them with the same latency, as the host's cluster send to one
//! node carries them. [`Net::link_up`] false cuts the leg node off from the origin, datagrams and messages both.

use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Simulcast, SimulcastLayer};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};

use super::{Layer, LegKey, Proto, RelayMessage, Sfu, SfuConfig, SfuEvent, SfuOutput};
use crate::audio::opus::OpusEncoder;
use crate::audio::FRAME;

pub const ROOM: &str = "CS1";
/// The call's media node, and the second node of [`Net::with_leg_node`].
pub const ORIGIN: &str = "nodeA";
pub const LEG_NODE: &str = "nodeB";
/// The simulated step.
const STEP: Duration = Duration::from_millis(2);
/// A downlink queue longer than this drops.
const MAX_QUEUE: Duration = Duration::from_millis(300);

struct Packet {
    at: Instant,
    proto: Protocol,
    source: SocketAddr,
    destination: SocketAddr,
    contents: Vec<u8>,
}

/// A browser stand-in.
pub struct Client {
    pub key: LegKey,
    /// The node its media is on: 0 the origin, 1 the leg node.
    pub node: usize,
    pub generation: u64,
    pub addr: SocketAddr,
    pub rtc: Rtc,
    pub proto: Protocol,
    pending: Option<SdpPendingOffer>,
    inbox: VecDeque<Packet>,
    /// Downlink capacity in bits a second; `None` is unlimited.
    pub downlink_bps: Option<u64>,
    link_free_at: Instant,
    pub audio_mid: Option<Mid>,
    pub video_mid: Option<Mid>,
    pub simulcast: bool,
    pub connected: bool,
    /// Frames received, by (sending leg, kind).
    pub received: HashMap<(String, MediaKind), usize>,
    /// The bytes of the last frame received from each leg's video, which tells the layer.
    pub last_video_len: HashMap<String, usize>,
    pub last_audio: HashMap<String, Vec<u8>>,
    pub keyframe_asked: bool,
    /// The keyframe requests (PLI or FIR) this client got.
    pub keyframe_requests: usize,
    frame: u64,
    /// The audio level each audio frame carries, in negative dBov (0 loudest).
    pub audio_level: i8,
    /// A second video track (a screen share) the client sends on, when it has one.
    pub screen_mid: Option<Mid>,
    /// The sending leg of each mid the SFU offered, from the offer's `a=msid` (the stream id is the leg).
    streams: HashMap<String, String>,
    /// When set, the client sends real Opus: a tone at this frequency (0 for silence), from this encoder.
    pub tone: Option<(f32, OpusEncoder)>,
    /// When set, every audio frame received is kept, by sending leg: (RTP time, payload).
    pub keep_audio: bool,
    pub audio_frames: HashMap<String, Vec<(u64, Vec<u8>)>>,
}

/// The second media node of [`Net::with_leg_node`].
pub struct LegNode {
    pub sfu: Sfu,
    pub addr: SocketAddr,
    to_sfu: VecDeque<Packet>,
    pub events: Vec<SfuEvent>,
}

/// A relay message on its way to a node.
struct Message {
    at: Instant,
    to: usize,
    from: &'static str,
    room: String,
    message: RelayMessage,
}

pub struct Net {
    pub now: Instant,
    pub sfu: Sfu,
    pub sfu_addr: SocketAddr,
    pub clients: Vec<Client>,
    to_sfu: VecDeque<Packet>,
    pub leg_node: Option<LegNode>,
    messages: VecDeque<Message>,
    /// The relay messages sent, each node's: (from, the message).
    pub relayed: Vec<(&'static str, RelayMessage)>,
    /// False cuts the leg node off from the origin: their datagrams and messages are dropped.
    pub link_up: bool,
    pub latency: Duration,
    pub events: Vec<SfuEvent>,
    /// The leg whose offers from the SFU the test holds back.
    pub hold_offers: Option<String>,
    pub held: Vec<SfuEvent>,
    start: Instant,
    /// Time spent in the origin SFU's `handle_receive`, by the wall clock of this thread, and the peers its drives
    /// polled there; and the time spent in its `handle_timeout`.
    pub receive_time: Duration,
    pub receive_polls: u64,
    pub timeout_time: Duration,
}

/// The size of each layer's video frame, low to high; a frame's size tells which layer arrived.
pub const LAYER_BYTES: [usize; 3] = [400, 1_600, 5_000];
pub const SINGLE_BYTES: usize = 1_200;
pub const SCREEN_BYTES: usize = 2_400;

impl Net {
    pub fn new() -> Self {
        let now = Instant::now();
        let sfu_addr: SocketAddr = (Ipv4Addr::new(10, 0, 0, 1), 10_000).into();
        let sfu_tcp_addr: SocketAddr = (Ipv4Addr::new(10, 0, 0, 1), 10_001).into();
        let config = SfuConfig {
            udp_candidates: vec![sfu_addr],
            tcp_candidates: vec![sfu_tcp_addr],
            ..SfuConfig::default()
        };
        Net {
            now,
            sfu: Sfu::new(config),
            sfu_addr,
            clients: Vec::new(),
            to_sfu: VecDeque::new(),
            leg_node: None,
            messages: VecDeque::new(),
            relayed: Vec::new(),
            link_up: true,
            latency: Duration::from_millis(10),
            events: Vec::new(),
            hold_offers: None,
            held: Vec::new(),
            start: now,
            receive_time: Duration::ZERO,
            receive_polls: 0,
            timeout_time: Duration::ZERO,
        }
    }

    /// A network of two media nodes, each with `config`: the origin ([`Net::sfu`]) and a leg node.
    pub fn with_leg_node(config: SfuConfig) -> Self {
        let mut net = Net::new();
        net.sfu = Sfu::new(SfuConfig {
            udp_candidates: vec![net.sfu_addr],
            tcp_candidates: Vec::new(),
            ..config.clone()
        });
        let addr: SocketAddr = (Ipv4Addr::new(10, 0, 0, 2), 10_000).into();
        net.leg_node = Some(LegNode {
            sfu: Sfu::new(SfuConfig {
                udp_candidates: vec![addr],
                tcp_candidates: Vec::new(),
                ..config
            }),
            addr,
            to_sfu: VecDeque::new(),
            events: Vec::new(),
        });
        net
    }

    pub fn leg_node(&self) -> &LegNode {
        self.leg_node.as_ref().expect("a network with a leg node")
    }

    pub fn leg_node_sfu(&mut self) -> &mut Sfu {
        &mut self.leg_node.as_mut().expect("a network with a leg node").sfu
    }

    fn node_sfu(&mut self, node: usize) -> &mut Sfu {
        if node == 0 {
            &mut self.sfu
        } else {
            self.leg_node_sfu()
        }
    }

    /// Adds a client that publishes audio and video (in three layers when `simulcast`) and joins.
    pub fn join(&mut self, leg: &str, simulcast: bool) -> usize {
        self.join_with(leg, simulcast, Protocol::Udp)
    }

    /// A client that joins at the leg node, whose SFU links the room to the origin as the host does for a leg there.
    pub fn join_leg_node(&mut self, leg: &str, simulcast: bool) -> usize {
        self.join_at(leg, simulcast, Protocol::Udp, 1)
    }

    pub fn join_with(&mut self, leg: &str, simulcast: bool, proto: Protocol) -> usize {
        self.join_at(leg, simulcast, proto, 0)
    }

    /// A client that joins `room` on the origin, instead of [`ROOM`].
    pub fn join_room(&mut self, room: &str, leg: &str) -> usize {
        self.join_in(room, leg, false, Protocol::Udp, 0)
    }

    fn join_at(&mut self, leg: &str, simulcast: bool, proto: Protocol, node: usize) -> usize {
        self.join_in(ROOM, leg, simulcast, proto, node)
    }

    fn join_in(&mut self, room: &str, leg: &str, simulcast: bool, proto: Protocol, node: usize) -> usize {
        let n = self.clients.len() as u8 + 2;
        let addr: SocketAddr = (Ipv4Addr::new(10, 0, 1 + node as u8, n), 5_000).into();
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_vp8(true)
            .build(self.now);
        let candidate = match proto {
            Protocol::Udp => Candidate::host(addr, "udp").expect("a test address"),
            _ => Candidate::builder()
                .tcp()
                .host(addr)
                .tcptype(str0m::net::TcpType::Active)
                .build()
                .expect("a test address"),
        };
        rtc.add_local_candidate(candidate);
        let mut change = rtc.sdp_api();
        let audio_mid = change.add_media(MediaKind::Audio, Direction::SendOnly, Some(leg.into()), None, None);
        let sim = simulcast.then(|| {
            let mut s = Simulcast::new();
            for l in Layer::ALL {
                s.add_send_layer(SimulcastLayer::new(l.rid()));
            }
            s
        });
        let video_mid = change.add_media(MediaKind::Video, Direction::SendOnly, Some(leg.into()), None, sim);
        let (offer, pending) = change.apply().expect("an offer");
        let key = LegKey::new(room, leg);
        let now = self.now;
        let answer = self
            .node_sfu(node)
            .join(now, &key, 1, &offer.to_sdp_string())
            .expect("join");
        if node == 1 {
            self.leg_node_sfu().link(now, room, ORIGIN).expect("linked");
        }
        rtc.sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).expect("answer"))
            .expect("accepted");
        self.clients.push(Client {
            key,
            node,
            generation: 1,
            addr,
            rtc,
            proto,
            pending: None,
            inbox: VecDeque::new(),
            downlink_bps: None,
            link_free_at: self.now,
            audio_mid: Some(audio_mid),
            video_mid: Some(video_mid),
            simulcast,
            connected: false,
            received: HashMap::new(),
            last_video_len: HashMap::new(),
            last_audio: HashMap::new(),
            keyframe_asked: true,
            keyframe_requests: 0,
            frame: 0,
            audio_level: -30,
            screen_mid: None,
            streams: HashMap::new(),
            tone: None,
            keep_audio: false,
            audio_frames: HashMap::new(),
        });
        self.pump();
        self.clients.len() - 1
    }

    pub fn client(&self, leg: &str) -> &Client {
        self.clients.iter().find(|c| &*c.key.leg == leg).expect("a client")
    }

    pub fn client_mut(&mut self, leg: &str) -> &mut Client {
        self.clients.iter_mut().find(|c| &*c.key.leg == leg).expect("a client")
    }

    pub fn received(&self, at: &str, from: &str, kind: MediaKind) -> usize {
        self.client(at)
            .received
            .get(&(from.to_string(), kind))
            .copied()
            .unwrap_or(0)
    }

    /// Runs the network for `d`, every publishing client sending audio every 20 ms and video every 33 ms.
    pub fn run(&mut self, d: Duration) {
        let end = self.now + d;
        while self.now < end {
            self.now += STEP;
            let ms = (self.now - self.start).as_millis() as u64;
            for i in 0..self.clients.len() {
                if ms.is_multiple_of(20) {
                    self.send_audio(i);
                }
                if ms.is_multiple_of(34) {
                    self.send_video(i);
                }
            }
            self.pump();
        }
    }

    fn send_audio(&mut self, i: usize) {
        let c = &mut self.clients[i];
        let Some(mid) = c.audio_mid.filter(|_| c.connected) else {
            return;
        };
        let Some(writer) = c.rtc.writer(mid) else { return };
        let Some(pt) = writer
            .payload_params()
            .find(|p| p.spec().codec == Codec::Opus)
            .map(|p| p.pt())
        else {
            return;
        };
        let t = (self.now - self.start).as_millis() as u64 * 48;
        let payload = match c.tone.as_mut() {
            Some((hz, encoder)) => {
                let pcm = tone_frame(*hz, t);
                encoder.encode(&pcm).expect("a tone frame encodes").to_vec()
            }
            None => format!("audio:{}", c.key.leg).into_bytes(),
        };
        let level = c.audio_level;
        let _ =
            writer
                .audio_level(level, true)
                .write(pt, self.now, MediaTime::new(t, Frequency::FORTY_EIGHT_KHZ), payload);
    }

    fn send_video(&mut self, i: usize) {
        let now = self.now;
        let t = (now - self.start).as_millis() as u64 * 90;
        let c = &mut self.clients[i];
        let Some(mid) = c.video_mid.filter(|_| c.connected) else {
            return;
        };
        c.frame += 1;
        let key = c.keyframe_asked || c.frame.is_multiple_of(150);
        c.keyframe_asked = false;
        let layers: Vec<(Option<&str>, usize)> = if c.simulcast {
            Layer::ALL
                .iter()
                .map(|l| (Some(l.rid()), LAYER_BYTES[l.index()]))
                .collect()
        } else {
            vec![(None, SINGLE_BYTES)]
        };
        if let Some(screen) = c.screen_mid {
            if let Some(writer) = c.rtc.writer(screen) {
                let pt = writer
                    .payload_params()
                    .find(|p| p.spec().codec == Codec::Vp8)
                    .map(|p| p.pt());
                if let Some(pt) = pt {
                    let mut frame = vec![0u8; SCREEN_BYTES];
                    frame[0] = if key { 0x00 } else { 0x01 };
                    let _ = writer.write(pt, now, MediaTime::new(t, Frequency::NINETY_KHZ), frame);
                }
            }
        }
        for (rid, size) in layers {
            let Some(writer) = c.rtc.writer(mid) else { return };
            let Some(pt) = writer
                .payload_params()
                .find(|p| p.spec().codec == Codec::Vp8)
                .map(|p| p.pt())
            else {
                return;
            };
            let mut frame = vec![0u8; size];
            frame[0] = if key { 0x00 } else { 0x01 };
            let writer = match rid {
                Some(r) => writer.rid(r.into()),
                None => writer,
            };
            let _ = writer.write(pt, now, MediaTime::new(t, Frequency::NINETY_KHZ), frame);
        }
    }

    /// Moves everything that is due and polls every endpoint until it has nothing more now.
    pub fn pump(&mut self) {
        for _ in 0..50 {
            let mut moved = self.pump_node(0);
            if self.leg_node.is_some() {
                moved |= self.pump_node(1);
                moved |= self.deliver_messages();
            }
            for i in 0..self.clients.len() {
                moved |= self.poll_client(i);
            }
            if !moved {
                break;
            }
        }
    }

    /// The relay messages that are due go to their node's SFU.
    fn deliver_messages(&mut self) -> bool {
        let mut moved = false;
        while self.messages.front().is_some_and(|m| m.at <= self.now) {
            let m = self.messages.pop_front().expect("a message");
            moved = true;
            if !self.link_up {
                continue;
            }
            let now = self.now;
            let _ = self.node_sfu(m.to).handle_relay(now, &m.room, m.from, m.message);
        }
        moved
    }

    /// One node: its due datagrams in, its timeouts, and what it sends out.
    fn pump_node(&mut self, node: usize) -> bool {
        let mut moved = false;
        let now = self.now;
        loop {
            let queue = if node == 0 {
                &mut self.to_sfu
            } else {
                &mut self.leg_node.as_mut().expect("a leg node").to_sfu
            };
            if !queue.front().is_some_and(|p| p.at <= now) {
                break;
            }
            let p = queue.pop_front().expect("a packet");
            let proto = Proto::from_protocol(p.proto);
            let (started, polls) = (Instant::now(), self.node_sfu(node).stats().peer_polls);
            self.node_sfu(node)
                .handle_receive(now, proto, p.source, p.destination, &p.contents);
            if node == 0 {
                self.receive_time += started.elapsed();
                self.receive_polls += self.sfu.stats().peer_polls - polls;
            }
            moved = true;
        }
        let started = Instant::now();
        self.node_sfu(node).handle_timeout(now);
        if node == 0 {
            self.timeout_time += started.elapsed();
        }
        loop {
            match self.node_sfu(node).poll_output() {
                SfuOutput::Transmit(t) => {
                    moved = true;
                    // to the other node: the link between the two
                    if let Some(leg_node) = self.leg_node.as_mut() {
                        let to = match node {
                            0 if t.destination == leg_node.addr => Some(&mut leg_node.to_sfu),
                            1 if t.destination == self.sfu_addr => Some(&mut self.to_sfu),
                            _ => None,
                        };
                        if let Some(queue) = to {
                            if self.link_up {
                                queue.push_back(Packet {
                                    at: now + self.latency,
                                    proto: Protocol::Udp,
                                    source: t.source,
                                    destination: t.destination,
                                    contents: t.contents,
                                });
                            }
                            continue;
                        }
                    }
                    {
                        let Some(c) = self.clients.iter_mut().find(|c| c.addr == t.destination) else {
                            continue;
                        };
                        let mut at = self.now + self.latency;
                        if let Some(bps) = c.downlink_bps {
                            let start = c.link_free_at.max(self.now);
                            if start - self.now > MAX_QUEUE {
                                continue;
                            }
                            let tx = Duration::from_nanos(t.contents.len() as u64 * 8 * 1_000_000_000 / bps);
                            c.link_free_at = start + tx;
                            at = c.link_free_at + self.latency;
                        }
                        let proto = if t.proto == Proto::Udp {
                            Protocol::Udp
                        } else {
                            Protocol::Tcp
                        };
                        c.inbox.push_back(Packet {
                            at,
                            proto,
                            source: t.source,
                            destination: t.destination,
                            contents: t.contents,
                        });
                    }
                }
                SfuOutput::Event(e) => {
                    moved = true;
                    self.sfu_event(node, e);
                }
                SfuOutput::Timeout(_) => break,
            }
        }
        moved
    }

    fn sfu_event(&mut self, node: usize, e: SfuEvent) {
        match &e {
            SfuEvent::Offer {
                key,
                offer_id,
                sdp,
                generation,
            } if self.hold_offers.as_deref() != Some(&*key.leg) => {
                let Some(c) = self
                    .clients
                    .iter_mut()
                    .find(|c| c.key == *key && c.generation == *generation && c.node == node)
                else {
                    return;
                };
                c.streams.extend(msid_streams(sdp));
                let offer = SdpOffer::from_sdp_string(sdp).expect("an SFU offer");
                let answer = c.rtc.sdp_api().accept_offer(offer).expect("the client answers");
                let key = key.clone();
                let generation = *generation;
                let now = self.now;
                self.node_sfu(node)
                    .handle_answer(now, &key, generation, *offer_id, &answer.to_sdp_string())
                    .expect("the answer is taken");
            }
            SfuEvent::Offer { .. } => self.held.push(e.clone()),
            SfuEvent::Relay { room, message, .. } => {
                let from = if node == 0 { ORIGIN } else { LEG_NODE };
                self.relayed.push((from, message.clone()));
                self.messages.push_back(Message {
                    at: self.now + self.latency,
                    to: 1 - node,
                    from,
                    room: room.to_string(),
                    message: message.clone(),
                });
            }
            _ => {}
        }
        if node == 0 {
            self.events.push(e);
        } else if let Some(leg_node) = self.leg_node.as_mut() {
            leg_node.events.push(e);
        }
    }

    fn poll_client(&mut self, i: usize) -> bool {
        let now = self.now;
        let mut moved = false;
        let c = &mut self.clients[i];
        while c.inbox.front().is_some_and(|p| p.at <= now) {
            let p = c.inbox.pop_front().expect("a packet");
            let contents = (&p.contents[..]).try_into().expect("a datagram");
            let input = Input::Receive(
                now,
                Receive {
                    proto: p.proto,
                    source: p.source,
                    destination: p.destination,
                    contents,
                },
            );
            let _ = c.rtc.handle_input(input);
            moved = true;
        }
        let _ = c.rtc.handle_input(Input::Timeout(now));
        loop {
            match c.rtc.poll_output() {
                Ok(Output::Timeout(_)) | Err(_) => break,
                Ok(Output::Transmit(t)) => {
                    moved = true;
                    // A client on TCP only reaches the SFU's TCP candidate, as when UDP is blocked.
                    let node_addr = if c.node == 0 {
                        self.sfu_addr
                    } else {
                        self.leg_node.as_ref().map_or(self.sfu_addr, |e| e.addr)
                    };
                    if c.proto == Protocol::Udp && t.destination != node_addr {
                        continue;
                    }
                    let queue = match (c.node, self.leg_node.as_mut()) {
                        (1, Some(leg_node)) => &mut leg_node.to_sfu,
                        _ => &mut self.to_sfu,
                    };
                    queue.push_back(Packet {
                        at: now + self.latency,
                        proto: t.proto,
                        source: t.source,
                        destination: t.destination,
                        contents: t.contents.into(),
                    });
                }
                Ok(Output::Event(e)) => {
                    moved = true;
                    match e {
                        Event::Connected => c.connected = true,
                        Event::MediaData(d) => {
                            let from = c.streams.get(&d.mid.to_string()).cloned().unwrap_or_default();
                            let kind = c.rtc.media(d.mid).map(|m| m.kind()).unwrap_or(MediaKind::Audio);
                            *c.received.entry((from.clone(), kind)).or_default() += 1;
                            if kind == MediaKind::Video {
                                c.last_video_len.insert(from, d.data.len());
                            } else {
                                if c.keep_audio {
                                    c.audio_frames
                                        .entry(from.clone())
                                        .or_default()
                                        .push((d.time.numer(), d.data.to_vec()));
                                }
                                c.last_audio.insert(from, d.data.to_vec());
                            }
                        }
                        Event::KeyframeRequest(_) => {
                            c.keyframe_asked = true;
                            c.keyframe_requests += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        let _ = &c.pending;
        moved
    }

    /// The SFU offers another track to this client (for the glare test): the client's own offer, made before it
    /// sees the SFU's.
    pub fn client_offer(&mut self, leg: &str) -> String {
        let c = self.client_mut(leg);
        let mut change = c.rtc.sdp_api();
        change.add_media(MediaKind::Audio, Direction::SendOnly, Some(leg.into()), None, None);
        let (offer, pending) = change.apply().expect("an offer");
        c.pending = Some(pending);
        offer.to_sdp_string()
    }

    /// The client starts sharing its screen: a video track of its own, offered and declared (`publish` source screen).
    pub fn share_screen(&mut self, leg: &str) -> Mid {
        let now = self.now;
        let c = self.client_mut(leg);
        let mut change = c.rtc.sdp_api();
        let mid = change.add_media(MediaKind::Video, Direction::SendOnly, Some(leg.into()), None, None);
        let (offer, pending) = change.apply().expect("an offer");
        let (key, node) = (c.key.clone(), c.node);
        let answer = self
            .node_sfu(node)
            .handle_offer(now, &key, 1, &offer.to_sdp_string())
            .expect("the share is answered");
        let c = self.client_mut(leg);
        c.rtc
            .sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).expect("answer"))
            .expect("accepted");
        c.screen_mid = Some(mid);
        self.node_sfu(node)
            .set_source(&key, 1, &mid.to_string(), super::Source::Screen)
            .expect("declared");
        self.pump();
        mid
    }

    /// The client stops sharing: the track goes inactive, offered again.
    pub fn stop_screen(&mut self, leg: &str) {
        let now = self.now;
        let c = self.client_mut(leg);
        let Some(mid) = c.screen_mid.take() else { return };
        let mut change = c.rtc.sdp_api();
        change.set_direction(mid, Direction::Inactive);
        let (offer, pending) = change.apply().expect("an offer");
        let (key, node) = (c.key.clone(), c.node);
        let answer = self
            .node_sfu(node)
            .handle_offer(now, &key, 1, &offer.to_sdp_string())
            .expect("the stop is answered");
        let c = self.client_mut(leg);
        c.rtc
            .sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(&answer).expect("answer"))
            .expect("accepted");
        self.pump();
    }

    pub fn client_accept_answer(&mut self, leg: &str, answer: &str) {
        let c = self.client_mut(leg);
        let pending = c.pending.take().expect("an offer outstanding");
        c.rtc
            .sdp_api()
            .accept_answer(pending, SdpAnswer::from_sdp_string(answer).expect("answer"))
            .expect("accepted");
    }
}

/// A 20 ms frame of a tone at `hz` (silence at 0), starting at sample `t` of 48 kHz, at -12 dBFS.
pub fn tone_frame(hz: f32, t: u64) -> Vec<i16> {
    (0..FRAME as u64)
        .map(|i| {
            let x = (2.0 * std::f64::consts::PI * f64::from(hz) * (t + i) as f64 / 48_000.0).sin();
            (x * 8_000.0) as i16
        })
        .collect()
}

/// The stream id of each m-section's mid in an SDP.
fn msid_streams(sdp: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let (mut mid, mut stream) = (None::<String>, None::<String>);
    for line in sdp.lines().chain(std::iter::once("m=end")) {
        if line.starts_with("m=") {
            if let (Some(m), Some(s)) = (mid.take(), stream.take()) {
                found.push((m, s));
            }
            mid = None;
            stream = None;
        } else if let Some(m) = line.strip_prefix("a=mid:") {
            mid = Some(m.trim().to_string());
        } else if let Some(m) = line.strip_prefix("a=msid:") {
            stream = m.split_whitespace().next().map(str::to_string);
        }
    }
    found
}
