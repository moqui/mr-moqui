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

//! The media driver thread: the one owner of the media socket and the [`Sfu`]. Each turn it takes the waiting commands,
//! injects the frames of injected legs that are due ([`super::media`]), sends what the SFU has to send, and waits on
//! the socket until the SFU's next timeout or the next injected frame (at most [`MAX_WAIT`], so a command never waits
//! long). A frame's reply, and the SFU's offers and events for a leg, go to the socket that leg's
//! last frame came from ([`Route`]): on this node straight to the socket, on another by `rtc-signal`.
//!
//! Cascading (W7): a leg that joins here for a call whose media node is another (this node is an edge) links the room to
//! that node (the origin); the SFUs' relay messages go between the two by `rtc-signal` (kind `relay`). What this node
//! tells a room's legs of the room itself (a `participant`, a relayed `mute`, a notice, a caption) goes to the room's
//! other nodes too (kind `roomFrame`), and the origin passes an edge's on to the other edges. A link that is lost puts
//! the edge's legs on the reconnect path (`mediaLost`); one the origin refused sends them back to the call's node
//! (`note#MediaEvent` `relayRefused`) the same way; one the origin closed (the call ended) ends them.

use std::collections::{BTreeMap, HashMap};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::{TopicMessage, RTC_SIGNAL};
use moqui_lib::object::Value;
use moqui_rtc::{Layer, LegKey, Proto, RelayMessage, RelayRole, Sfu, SfuConfig, SfuError, SfuEvent, SfuOutput, Source};
use serde_json::{json, Value as Json};

use super::media::Media;
use super::tcp::{self, Streams};
use super::{Command, NoticeKind};

/// Revokes a leg's TURN credential (the embedded TURN server, when there is one).
pub(super) type Revoke = Box<dyn Fn(&str) + Send>;

/// The longest the driver waits on the socket before it looks at its commands again.
const MAX_WAIT: Duration = Duration::from_millis(5);
/// The largest datagram read.
const DATAGRAM: usize = 2_048;
/// SDP larger than this is refused.
const MAX_SDP: usize = 64 * 1024;

/// Where a leg's signaling socket is: its node and its socket id there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Route {
    pub node: String,
    pub socket_id: u64,
}

struct LegRoute {
    generation: u64,
    route: Route,
}

struct Driver<'a> {
    factory: &'a Arc<ExecutionContextFactory>,
    own: String,
    sfu: Sfu,
    routes: HashMap<LegKey, LegRoute>,
    /// The ICE-TCP address browsers are told, and the connections' streams.
    tcp: Option<(SocketAddr, Streams)>,
    revoke: Option<Revoke>,
    /// The consumers' taps and the injected legs.
    media: Media,
    /// The frames each leg gets when it joins: the rooms' recording and captions notices.
    notices: RoomNotices,
    /// The call's media node of each room this node is an edge of (W7).
    origins: HashMap<String, String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    factory: &Arc<ExecutionContextFactory>,
    socket: UdpSocket,
    public: SocketAddr,
    own: String,
    commands: Receiver<Command>,
    tcp: Option<(SocketAddr, Streams)>,
    revoke: Option<Revoke>,
    stop: &AtomicBool,
) {
    let config = SfuConfig {
        udp_candidates: vec![public],
        tcp_candidates: tcp.as_ref().map(|(addr, _)| vec![*addr]).unwrap_or_default(),
        ..SfuConfig::default()
    };
    let mut driver = Driver {
        factory,
        own,
        sfu: Sfu::new(config),
        routes: HashMap::new(),
        tcp,
        revoke,
        media: Media::default(),
        notices: RoomNotices::default(),
        origins: HashMap::new(),
    };
    let mut buf = vec![0u8; DATAGRAM];
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let turn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(command) = commands.try_recv() {
                if matches!(command, Command::Stop) {
                    return false;
                }
                driver.command(command);
            }
            driver.media.turn(&mut driver.sfu, Instant::now());
            let next = driver.flush(&socket);
            let next = driver.media.next_due().map_or(next, |due| due.min(next));
            let wait = next
                .saturating_duration_since(Instant::now())
                .clamp(Duration::from_millis(1), MAX_WAIT);
            let _ = socket.set_read_timeout(Some(wait));
            match socket.recv_from(&mut buf) {
                Ok((n, source)) => {
                    let now = Instant::now();
                    driver.sfu.handle_receive(now, Proto::Udp, source, public, &buf[..n]);
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) => {}
                Err(e) => tracing::debug!("call media socket read failed: {e}"),
            }
            let now = Instant::now();
            if now >= next {
                driver.sfu.handle_timeout(now);
            }
            true
        }));
        match turn {
            Ok(true) => {}
            Ok(false) => break,
            Err(_) => tracing::error!("call media driver turn panicked; going on"),
        }
    }
    tracing::info!("Call media stopped");
}

impl Driver<'_> {
    /// Sends the SFU's datagrams and handles its events; the time of its next timeout.
    fn flush(&mut self, socket: &UdpSocket) -> Instant {
        loop {
            match self.sfu.poll_output() {
                SfuOutput::Transmit(t) => {
                    if t.proto == Proto::Tcp {
                        if let Some((_, streams)) = &self.tcp {
                            tcp::send(streams, t.destination, &t.contents);
                        }
                    } else if let Err(e) = socket.send_to(&t.contents, t.destination) {
                        tracing::debug!("call media send to {} failed: {e}", t.destination);
                    }
                }
                SfuOutput::Event(e) => self.event(e),
                SfuOutput::Timeout(t) => return t,
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Frame {
                route,
                room,
                leg,
                generation,
                origin,
                frame,
            } => self.frame(route, &room, &leg, generation, &origin, &frame),
            Command::Relay { room, from, message } => {
                if let Err(e) = self.sfu.handle_relay(Instant::now(), &room, &from, *message) {
                    tracing::debug!("call {room}: relay message from {from} not taken: {e}");
                }
            }
            Command::Link { room, origin } => {
                if !origin.is_empty() && origin != self.own {
                    self.origins.insert(room.clone(), origin.clone());
                    if let Err(e) = self.sfu.link(Instant::now(), &room, &origin) {
                        tracing::warn!("call {room}: not linked to its media node {origin}: {e}");
                    }
                }
            }
            Command::RoomFrame {
                room,
                from,
                except,
                frame,
                notice,
            } => {
                if let Some((kind, keep)) = notice {
                    self.notices.set(&room, kind, keep.then(|| frame.clone()));
                }
                self.spread(&room, except.as_deref(), frame, notice, Some(&from));
            }
            Command::TcpIn { from, data } => {
                if let Some((addr, _)) = &self.tcp {
                    let addr = *addr;
                    self.sfu.handle_receive(Instant::now(), Proto::Tcp, from, addr, &data);
                }
            }
            Command::CloseLeg { room, leg } => {
                if let Some(revoke) = &self.revoke {
                    revoke(&leg);
                }
                let key = LegKey::new(&room, &leg);
                if let Some(generation) = self.sfu.generation(&key) {
                    let _ = self.sfu.leave(Instant::now(), &key, generation);
                }
                self.media.leg_closed(&mut self.sfu, Instant::now(), &room, &leg);
                if let Some(r) = self.routes.remove(&key) {
                    self.reply(
                        &r.route,
                        &room,
                        json!({"rtc": "ended", "callLegId": leg, "reason": "left"}),
                    );
                }
                self.spread(
                    &room,
                    None,
                    json!({"rtc": "participant", "callLegId": leg, "connected": false}),
                    None,
                    None,
                );
            }
            Command::Keyframes { room } => self.sfu.request_keyframes(Instant::now(), &room),
            Command::CloseRoom { room } => {
                self.sfu.close_room(Instant::now(), &room);
                self.media.room_closed(&room);
                self.notices.room_closed(&room);
                self.origins.remove(&room);
                let legs: Vec<LegKey> = self
                    .routes
                    .keys()
                    .filter(|k| &*k.room == room.as_str())
                    .cloned()
                    .collect();
                for key in legs {
                    if let Some(revoke) = &self.revoke {
                        revoke(&key.leg);
                    }
                    if let Some(r) = self.routes.remove(&key) {
                        self.reply(
                            &r.route,
                            &room,
                            json!({"rtc": "ended", "callLegId": &*key.leg, "reason": "ended"}),
                        );
                    }
                }
            }
            Command::Media(command) => self.media.command(&mut self.sfu, Instant::now(), command),
            Command::ToRoom { room, frame } => self.spread(&room, None, frame, None, None),
            Command::RoomNotice {
                room,
                kind,
                frame,
                keep,
            } => {
                // every leg now; kept, each leg that joins later too (a notice not kept takes its kind's down)
                self.notices.set(&room, kind, keep.then(|| frame.clone()));
                self.spread(&room, None, frame, Some((kind, keep)), None);
            }
            Command::Stop => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(&mut self, route: Route, room: &str, leg: &str, generation: u64, origin: &str, frame: &Json) {
        let key = LegKey::new(room, leg);
        let op = frame.get("rtc").and_then(Json::as_str).unwrap_or_default();
        let id = frame.get("id").cloned().unwrap_or(Json::Null);
        let now = Instant::now();
        let sdp = || {
            frame
                .get("sdp")
                .and_then(Json::as_str)
                .filter(|s| s.len() <= MAX_SDP)
                .unwrap_or_default()
        };
        let result: Result<Option<Json>, SfuError> = match op {
            "join" => self
                .sfu
                .join(now, &key, generation, sdp())
                .and_then(|answer| {
                    // a leg here of a call whose media is on another node: this node is an edge, linked to that one
                    if !origin.is_empty() && origin != self.own {
                        self.origins.insert(room.to_string(), origin.to_string());
                        if let Err(e) = self.sfu.link(now, room, origin) {
                            let _ = self.sfu.leave(now, &key, generation);
                            return Err(e);
                        }
                    }
                    Ok(answer)
                })
                .map(|answer| {
                    self.routes.insert(
                        key.clone(),
                        LegRoute {
                            generation,
                            route: route.clone(),
                        },
                    );
                    Some(json!({"rtc": "joined", "callLegId": leg, "generation": generation, "sdp": answer}))
                }),
            "offer" => self
                .sfu
                .handle_offer(now, &key, generation, sdp())
                .map(|answer| Some(json!({"rtc": "answer", "callLegId": leg, "sdp": answer}))),
            "answer" => {
                let offer_id = frame.get("offerId").and_then(Json::as_u64).unwrap_or(0);
                self.sfu
                    .handle_answer(now, &key, generation, offer_id, sdp())
                    .map(|()| None)
            }
            "candidate" => {
                let candidate = frame.get("candidate").and_then(Json::as_str).unwrap_or_default();
                if candidate.is_empty() {
                    Ok(None)
                } else {
                    self.sfu.add_candidate(&key, generation, candidate).map(|()| None)
                }
            }
            "publish" => {
                let mid = frame.get("mid").and_then(Json::as_str).unwrap_or_default();
                match frame.get("source").and_then(Json::as_str).and_then(Source::parse) {
                    Some(source) if !mid.is_empty() => {
                        self.sfu.set_source(&key, generation, mid, source).map(|()| None)
                    }
                    _ => Err(SfuError::BadSdp("publish needs a mid and a source".to_string())),
                }
            }
            "subscribe" => {
                let publisher = frame.get("publisher").and_then(Json::as_str).unwrap_or_default();
                let layer = frame.get("layer").and_then(Json::as_str).and_then(Layer::parse);
                self.sfu
                    .subscribe(now, &key, generation, publisher, layer)
                    .map(|()| None)
            }
            "leave" => {
                let left = self.sfu.leave(now, &key, generation).map(|()| None);
                self.routes.remove(&key);
                left
            }
            "mute" | "hand" | "reaction" | "unpublish" => match self.sfu.generation(&key) {
                Some(g) if g == generation => {
                    let mut relay = frame.clone();
                    if let Some(o) = relay.as_object_mut() {
                        o.remove("id");
                        o.insert("from".to_string(), Json::String(leg.to_string()));
                    }
                    self.spread(room, Some(leg), relay, None, None);
                    Ok(None)
                }
                Some(_) => Err(SfuError::StaleGeneration),
                None => Err(SfuError::UnknownLeg),
            },
            _ => Err(SfuError::BadSdp(format!("unknown rtc frame {op}"))),
        };
        if let Some(r) = self.routes.get_mut(&key) {
            if r.generation == generation {
                r.route = route.clone();
            }
        }
        let joined = op == "join" && result.is_ok();
        match result {
            Ok(Some(reply)) => self.reply(&route, room, with_id(reply, &id)),
            Ok(None) if !id.is_null() => self.reply(&route, room, json!({"rtc": "ack", "id": id, "callLegId": leg})),
            Ok(None) => {}
            Err(e) => self.reply(
                &route,
                room,
                json!({"rtc": "error", "id": id, "callLegId": leg, "code": e.code(), "message": e.to_string()}),
            ),
        }
        // a leg that joins gets the room's notices (recording, captions) right after its answer, before any media:
        // late joiners and guests too
        if joined {
            for notice in self.notices.of(room) {
                self.to_leg(&key, generation, notice);
            }
        }
    }

    fn event(&mut self, event: SfuEvent) {
        match event {
            SfuEvent::Offer {
                key,
                generation,
                offer_id,
                sdp,
            } => {
                self.to_leg(
                    &key,
                    generation,
                    json!({"rtc": "offer", "offerId": offer_id, "sdp": sdp}),
                );
            }
            SfuEvent::Connected { key, generation } => {
                self.to_leg(&key, generation, json!({"rtc": "connected"}));
                self.spread(
                    &key.room,
                    Some(&key.leg),
                    json!({"rtc": "participant", "callLegId": &*key.leg, "connected": true}),
                    None,
                    None,
                );
                super::note(
                    self.factory,
                    "org.moqui.impl.CallServices.note#MediaEvent",
                    vec![
                        ("callLegId", Value::string(&*key.leg)),
                        ("generation", Value::I64(i64::try_from(generation).unwrap_or(0))),
                        ("event", Value::string("connected")),
                    ],
                );
            }
            SfuEvent::Disconnected { key, generation } => {
                let current = self.routes.get(&key).is_some_and(|r| r.generation == generation);
                if current {
                    self.to_leg(&key, generation, json!({"rtc": "mediaLost"}));
                    self.spread(
                        &key.room,
                        Some(&key.leg),
                        json!({"rtc": "participant", "callLegId": &*key.leg, "connected": false}),
                        None,
                        None,
                    );
                }
            }
            SfuEvent::Track {
                key,
                mid,
                kind,
                source,
                open,
            } => {
                let kind = match kind {
                    moqui_rtc::tap::Kind::Audio => "audio",
                    moqui_rtc::tap::Kind::Video => "video",
                };
                self.to_room(
                    &key.room,
                    None,
                    json!({"rtc": "track", "callLegId": &*key.leg, "mid": mid, "kind": kind, "source": source.as_str(), "open": open}),
                );
            }
            SfuEvent::Layer { key, publisher, layer } => {
                let generation = self.routes.get(&key).map_or(0, |r| r.generation);
                self.to_leg(
                    &key,
                    generation,
                    json!({"rtc": "layer", "publisher": &*publisher, "layer": layer.map(Layer::as_str)}),
                );
            }
            SfuEvent::ActiveSpeaker { room, leg } => {
                self.to_room(&room, None, json!({"rtc": "activeSpeaker", "speaker": &*leg}));
            }
            SfuEvent::Relay { room, node, message } => self.send_relay(&room, &node, &message),
            SfuEvent::RelayUp { room, node } => tracing::debug!("call {room}: relay link to {node} connected"),
            SfuEvent::RelayDown { room, node, reason } => self.relay_down(&room, &node, &reason),
            SfuEvent::Estimate { key, kbps } => {
                let generation = self.routes.get(&key).map_or(0, |r| r.generation);
                self.to_leg(&key, generation, json!({"rtc": "quality", "kbps": kbps}));
            }
        }
    }

    /// A message of the SFU for the node at the other end of a room's relay link.
    fn send_relay(&self, room: &str, node: &str, message: &RelayMessage) {
        let message = match serde_json::to_value(message) {
            Ok(message) => message,
            Err(e) => return tracing::warn!("call {room}: relay message not written: {e}"),
        };
        self.send_node(
            node,
            json!({"target": node, "kind": "relay", "from": self.own, "callSessionId": room, "message": message}),
        );
    }

    /// A room's link ended. For an edge: lost, its legs reconnect (and link again); refused, they go back to the call's
    /// node as they reconnect; closed by the origin (the call ended), they end. Nothing else is done for the others.
    fn relay_down(&mut self, room: &str, node: &str, reason: &str) {
        let edge = self.origins.get(room).is_some_and(|origin| origin == node);
        tracing::info!("call {room}: relay link to {node} ended: {reason}");
        if !edge {
            return;
        }
        if reason == "ended" {
            return self.command(Command::CloseRoom { room: room.to_string() });
        }
        if reason != "lost" && !reason.starts_with("refused") {
            return;
        }
        let legs: Vec<(LegKey, u64)> = self
            .routes
            .iter()
            .filter(|(k, _)| &*k.room == room)
            .map(|(k, r)| (k.clone(), r.generation))
            .collect();
        for (key, generation) in legs {
            if reason.starts_with("refused") {
                super::note(
                    self.factory,
                    "org.moqui.impl.CallServices.note#MediaEvent",
                    vec![
                        ("callLegId", Value::string(&*key.leg)),
                        ("generation", Value::I64(i64::try_from(generation).unwrap_or(0))),
                        ("event", Value::string("relayRefused")),
                    ],
                );
            }
            // the leg's peer goes: its socket hears mediaLost and joins again, with a new generation
            if self.sfu.generation(&key) == Some(generation) {
                let _ = self.sfu.leave(Instant::now(), &key, generation);
            }
        }
    }

    /// To every leg of the room, here and on the room's other nodes (W7), but `except`; not back to `from`. A notice goes
    /// with its kind, so the other nodes keep it for their legs that join later.
    fn spread(
        &self,
        room: &str,
        except: Option<&str>,
        frame: Json,
        notice: Option<(super::NoticeKind, bool)>,
        from: Option<&str>,
    ) {
        for (node, role, _) in self.sfu.relays(room) {
            // an edge's frame goes on from the origin to the other edges; an edge sends only to its origin
            if from == Some(&*node) || (from.is_some() && role != RelayRole::Origin) {
                continue;
            }
            self.send_node(
                &node,
                json!({"target": &*node, "kind": "roomFrame", "from": self.own, "callSessionId": room,
                    "except": except, "frame": frame, "notice": notice.map(|(k, _)| k.as_str()),
                    "keep": notice.is_some_and(|(_, keep)| keep)}),
            );
        }
        self.to_room(room, except, frame);
    }

    fn to_leg(&self, key: &LegKey, generation: u64, frame: Json) {
        let Some(r) = self.routes.get(key).filter(|r| r.generation == generation) else {
            return;
        };
        let mut frame = frame;
        if let Some(o) = frame.as_object_mut() {
            o.insert("callLegId".to_string(), Json::String(key.leg.to_string()));
        }
        self.reply(&r.route, &key.room, frame);
    }

    /// To every leg of the room, but `except`.
    fn to_room(&self, room: &str, except: Option<&str>, frame: Json) {
        for (key, r) in &self.routes {
            if &*key.room == room && except != Some(&*key.leg) {
                let mut f = frame.clone();
                if let Some(o) = f.as_object_mut() {
                    o.entry("callLegId".to_string())
                        .or_insert_with(|| Json::String(key.leg.to_string()));
                    o.insert("to".to_string(), Json::String(key.leg.to_string()));
                }
                self.reply(&r.route, room, f);
            }
        }
    }

    fn reply(&self, route: &Route, room: &str, frame: Json) {
        let mut frame = frame;
        if let Some(o) = frame.as_object_mut() {
            o.insert("topic".to_string(), Json::String(format!("rtc:{room}")));
            o.insert("callSessionId".to_string(), Json::String(room.to_string()));
        }
        if route.node == self.own {
            crate::notws::push_to_socket(route.socket_id, frame.to_string());
            return;
        }
        self.send_node(
            &route.node,
            json!({"target": route.node, "kind": "reply", "socketId": route.socket_id, "frame": frame}),
        );
    }

    /// A message to another node on `rtc-signal`, to that node only.
    fn send_node(&self, node: &str, body: Json) {
        let Some(facade) = self.factory.entity() else { return };
        let message = TopicMessage {
            origin_id: moqui_core::notify::origin_id().to_string(),
            message_id: moqui_core::entity::uuid_v4().unwrap_or_default(),
            body,
        };
        if let Err(err) = moqui_core::cluster::send_to(&facade, node, RTC_SIGNAL, &message) {
            tracing::debug!("call signaling to {node} not sent: {}", err.message());
        }
    }
}

/// The notices of each room, one of each [`NoticeKind`], for the legs that join later.
#[derive(Default)]
pub(super) struct RoomNotices {
    rooms: HashMap<String, BTreeMap<NoticeKind, Json>>,
}

impl RoomNotices {
    /// The room's notice of this kind, or with `None` none.
    pub(super) fn set(&mut self, room: &str, kind: NoticeKind, frame: Option<Json>) {
        match frame {
            Some(frame) => {
                self.rooms.entry(room.to_string()).or_default().insert(kind, frame);
            }
            None => {
                if let Some(kinds) = self.rooms.get_mut(room) {
                    kinds.remove(&kind);
                    if kinds.is_empty() {
                        self.rooms.remove(room);
                    }
                }
            }
        }
    }

    /// The room's current notices, in kind order.
    pub(super) fn of(&self, room: &str) -> Vec<Json> {
        self.rooms
            .get(room)
            .map(|kinds| kinds.values().cloned().collect())
            .unwrap_or_default()
    }

    pub(super) fn room_closed(&mut self, room: &str) {
        self.rooms.remove(room);
    }
}

fn with_id(mut frame: Json, id: &Json) -> Json {
    if !id.is_null() {
        if let Some(o) = frame.as_object_mut() {
            o.insert("id".to_string(), id.clone());
        }
    }
    frame
}
