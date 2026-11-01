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

//! A call on a seeded server with its media port on (plans/Comms-Framework.md W1): john.doe calls another user, both
//! answer through the call services, and each browser stand-in (a str0m peer on a loopback UDP socket) joins its leg
//! over the notification socket and exchanges audio through the server's SFU, renegotiating as the other's tracks
//! arrive. Then a second socket of the same leg takes the next generation and the first socket's frames are refused,
//! the other user cannot join a leg that is not theirs, and leaving ends the call for both.
//!
//! [`cascade`] runs a call on two servers of one cluster, each with its media port (W7).

mod cascade;

use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid, Simulcast, SimulcastLayer};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, ClientRequestBuilder, Message, WebSocket};

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

const CALLEE: &str = "rtc.callee";
const PASSWORD: &str = "Rtc-callee-77!";

pub(crate) fn open_socket(address: SocketAddr, cookie: &str) -> Ws {
    let uri: tungstenite::http::Uri = format!("ws://{address}/notws").parse().unwrap();
    let (socket, _) = connect(ClientRequestBuilder::new(uri).with_header("Cookie", cookie)).unwrap();
    if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        tcp.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    }
    socket
}

fn read_frames(ws: &mut Ws) -> Vec<Json> {
    let mut frames = Vec::new();
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let value: Json = serde_json::from_str(&text).unwrap_or(Json::Null);
                if value.get("rtc").is_some() {
                    frames.push(value);
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                return frames
            }
            // the server closed the socket (a refused guest): what came before is the answer
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return frames,
            Err(e) => panic!("socket read: {e}"),
        }
    }
}

/// A browser stand-in: a str0m peer on its own UDP socket, signaling over a notification socket.
pub(crate) struct Peer {
    rtc: Rtc,
    udp: UdpSocket,
    ws: Ws,
    /// How the peer's datagrams go: straight over UDP, framed over ICE-TCP, or relayed by TURN.
    via: Via,
    call: String,
    leg: String,
    pending: Option<SdpPendingOffer>,
    audio: Option<Mid>,
    pub(crate) received: usize,
    pub(crate) frames: Vec<Json>,
    start: Instant,
    /// A camera track, sent in three simulcast layers of [`VIDEO_LAYER_BYTES`] when set.
    video: Option<Mid>,
    video_sent: Instant,
    video_frame: u64,
    keyframe_asked: bool,
    /// Video frames received, and the size of the last (which tells the layer).
    pub(crate) video_received: usize,
    pub(crate) last_video_len: Option<usize>,
}

/// The bytes of each simulcast layer's frame a peer with video sends, low to high: a frame's size tells its layer.
pub(crate) const VIDEO_LAYER_BYTES: [usize; 3] = [300, 900, 2_400];
const RIDS: [&str; 3] = ["l", "m", "h"];

impl Peer {
    pub(crate) fn new(ws: Ws, call: &str, leg: &str) -> Self {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        udp.set_nonblocking(true).unwrap();
        let mut rtc = Rtc::builder()
            .clear_codecs()
            .enable_opus(true, false)
            .enable_vp8(true)
            .build(Instant::now());
        rtc.add_local_candidate(Candidate::host(udp.local_addr().unwrap(), "udp").unwrap());
        Peer {
            rtc,
            udp,
            ws,
            via: Via::Udp,
            call: call.into(),
            leg: leg.into(),
            pending: None,
            audio: None,
            received: 0,
            frames: Vec::new(),
            start: Instant::now(),
            video: None,
            video_sent: Instant::now(),
            video_frame: 0,
            keyframe_asked: true,
            video_received: 0,
            last_video_len: None,
        }
    }

    /// Joins with its microphone and a camera in three simulcast layers.
    pub(crate) fn join_with_video(&mut self) {
        let mut change = self.rtc.sdp_api();
        self.audio = Some(change.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None));
        let mut layers = Simulcast::new();
        for rid in RIDS {
            layers.add_send_layer(SimulcastLayer::new(rid));
        }
        self.video = Some(change.add_media(MediaKind::Video, Direction::SendOnly, None, None, Some(layers)));
        let (offer, pending) = change.apply().unwrap();
        self.pending = Some(pending);
        self.send(json!({"rtc": "join", "id": 1, "sdp": offer.to_sdp_string()}));
    }

    /// A video frame in each layer, every 33 ms: a keyframe when one was asked for, and every 30th.
    fn send_video(&mut self) {
        let Some(mid) = self.video else { return };
        if self.video_sent.elapsed() < Duration::from_millis(33) {
            return;
        }
        self.video_sent = Instant::now();
        self.video_frame += 1;
        let key = self.keyframe_asked || self.video_frame.is_multiple_of(30);
        self.keyframe_asked = false;
        let t = self.start.elapsed().as_millis() as u64 * 90;
        for (rid, size) in RIDS.iter().zip(VIDEO_LAYER_BYTES) {
            let Some(writer) = self.rtc.writer(mid) else { return };
            let Some(pt) = writer
                .payload_params()
                .find(|p| p.spec().codec == Codec::Vp8)
                .map(|p| p.pt())
            else {
                return;
            };
            let mut frame = vec![0u8; size];
            frame[0] = if key { 0x00 } else { 0x01 };
            let _ = writer
                .rid((*rid).into())
                .write(pt, Instant::now(), MediaTime::new(t, Frequency::NINETY_KHZ), frame);
        }
    }

    pub(crate) fn send(&mut self, frame: Json) {
        let mut frame = frame;
        frame["callSessionId"] = json!(self.call);
        frame["callLegId"] = json!(self.leg);
        self.ws.send(Message::Text(frame.to_string().into())).unwrap();
    }

    pub(crate) fn join(&mut self) {
        let mut change = self.rtc.sdp_api();
        self.audio = Some(change.add_media(MediaKind::Audio, Direction::SendOnly, None, None, None));
        let (offer, pending) = change.apply().unwrap();
        self.pending = Some(pending);
        self.send(json!({"rtc": "join", "id": 1, "sdp": offer.to_sdp_string()}));
    }

    /// One turn: signaling frames, datagrams, timers, and an audio frame every 20 ms once connected.
    fn turn(&mut self, sent: &mut Instant) {
        for frame in read_frames(&mut self.ws) {
            match frame["rtc"].as_str() {
                Some("joined") => {
                    let answer = SdpAnswer::from_sdp_string(frame["sdp"].as_str().unwrap()).unwrap();
                    self.rtc
                        .sdp_api()
                        .accept_answer(self.pending.take().unwrap(), answer)
                        .unwrap();
                }
                Some("offer") => {
                    let offer = SdpOffer::from_sdp_string(frame["sdp"].as_str().unwrap()).unwrap();
                    let answer = self.rtc.sdp_api().accept_offer(offer).unwrap();
                    self.send(json!({"rtc": "answer", "offerId": frame["offerId"], "sdp": answer.to_sdp_string()}));
                }
                _ => {}
            }
            self.frames.push(frame);
        }
        for (proto, source, destination, data) in self.via.receive(&self.udp) {
            let receive = Receive {
                proto,
                source,
                destination,
                contents: (&data[..]).try_into().unwrap(),
            };
            let _ = self.rtc.handle_input(Input::Receive(Instant::now(), receive));
        }
        let _ = self.rtc.handle_input(Input::Timeout(Instant::now()));
        if sent.elapsed() >= Duration::from_millis(20) {
            *sent = Instant::now();
            if let Some(mid) = self.audio {
                if let Some(writer) = self.rtc.writer(mid) {
                    let pt = writer
                        .payload_params()
                        .find(|p| p.spec().codec == Codec::Opus)
                        .map(|p| p.pt());
                    if let Some(pt) = pt {
                        let t = self.start.elapsed().as_millis() as u64 * 48;
                        let _ = writer.write(
                            pt,
                            Instant::now(),
                            MediaTime::new(t, Frequency::FORTY_EIGHT_KHZ),
                            vec![1u8; 40],
                        );
                    }
                }
            }
        }
        self.send_video();
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Timeout(_)) | Err(_) => break,
                Ok(Output::Transmit(t)) => self.via.send(&self.udp, t.proto, t.destination, &t.contents),
                Ok(Output::Event(Event::MediaData(d)))
                    if self.rtc.media(d.mid).map(|m| m.kind()) == Some(MediaKind::Audio) =>
                {
                    self.received += 1
                }
                Ok(Output::Event(Event::MediaData(d))) => {
                    self.video_received += 1;
                    self.last_video_len = Some(d.data.len());
                }
                Ok(Output::Event(Event::KeyframeRequest(_))) => self.keyframe_asked = true,
                Ok(Output::Event(_)) => {}
            }
        }
    }

    pub(crate) fn frame(&self, kind: &str) -> Option<&Json> {
        self.frames.iter().find(|f| f["rtc"] == json!(kind))
    }
}

pub(crate) fn run_until(peers: &mut [&mut Peer], wait: Duration, done: impl Fn(&[&mut Peer]) -> bool) -> bool {
    let until = Instant::now() + wait;
    let mut sent = vec![Instant::now(); peers.len()];
    while Instant::now() < until {
        for (i, p) in peers.iter_mut().enumerate() {
            p.turn(&mut sent[i]);
        }
        if done(peers) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    false
}

pub(crate) fn call(name: &str, parameters: &[(&str, Value)]) -> std::collections::BTreeMap<String, Value> {
    let parameters = parameters.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let out = moqui_core::service::sync(name, &parameters).unwrap();
    moqui_core::context::ExecutionContext::with_current(|ec| {
        assert!(ec.message().errors().is_empty(), "{name}: {:?}", ec.message().errors());
    });
    out
}

pub(crate) fn text(out: &std::collections::BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

#[test]
fn a_direct_call_over_the_socket_and_the_media_port() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties("rtc", None, &[("rtc_media_port", "0")]);
    let http = gate.server.address();
    assert!(gate.server.rtc_address().is_some(), "the media socket is up");
    let factory = std::sync::Arc::clone(gate.server.factory());

    let (call_id, caller_leg, callee_leg) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let created = call(
            "org.moqui.impl.UserServices.create#UserAccount",
            &[
                ("username", Value::string(CALLEE)),
                ("newPassword", Value::string(PASSWORD)),
                ("newPasswordVerify", Value::string(PASSWORD)),
                ("userFullName", Value::string("Rtc Callee")),
            ],
        );
        let callee = text(&created, "userId");
        // the node registers as it starts; wait for its row
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            let caps = call("org.moqui.impl.CallServices.get#Capabilities", &[]);
            if caps.get("active") == Some(&Value::Bool(true)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let started = call(
            "org.moqui.impl.CallServices.start#Call",
            &[("calleeUserIdList", Value::List(vec![Value::string(&callee)]))],
        );
        let call_id = text(&started, "callSessionId");
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        let answered = call(
            "org.moqui.impl.CallServices.answer#Call",
            &[("callSessionId", Value::string(&call_id))],
        );
        (call_id, text(&started, "callLegId"), text(&answered, "callLegId"))
    };
    assert!(!call_id.is_empty() && !caller_leg.is_empty() && !callee_leg.is_empty());

    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    let callee_cookie = crate::common::sign_in(http, CALLEE, PASSWORD);
    factory.flush_sessions().unwrap();
    let mut john = Peer::new(open_socket(http, &john_cookie), &call_id, &caller_leg);
    let mut callee = Peer::new(open_socket(http, &callee_cookie), &call_id, &callee_leg);
    john.join();
    callee.join();
    let flowed = run_until(&mut [&mut john, &mut callee], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 20)
    });
    assert!(
        flowed,
        "audio both ways: john {} callee {}; frames {:?} {:?}",
        john.received, callee.received, john.frames, callee.frames
    );
    assert_eq!(john.frame("joined").map(|f| f["generation"].clone()), Some(json!(1)));
    assert!(john.frame("connected").is_some());
    assert!(callee
        .frames
        .iter()
        .any(|f| f["rtc"] == json!("track") && f["callLegId"] == json!(caller_leg)));

    // the other user cannot bind john's leg
    callee
        .ws
        .send(Message::Text(
            json!({"rtc": "join", "id": 9, "sdp": "v=0", "callSessionId": call_id,
        "callLegId": caller_leg})
            .to_string()
            .into(),
        ))
        .unwrap();
    // a frame of a leg this socket never joined
    callee
        .ws
        .send(Message::Text(
            json!({"rtc": "subscribe", "id": 10, "callSessionId": call_id, "callLegId": caller_leg,
        "publisher": callee_leg, "layer": "low"})
            .to_string()
            .into(),
        ))
        .unwrap();
    // a second socket of john's takes generation 2; the first socket's frames are now refused
    let mut again = Peer::new(open_socket(http, &john_cookie), &call_id, &caller_leg);
    again.join();
    run_until(&mut [&mut john, &mut callee, &mut again], Duration::from_secs(5), |p| {
        p[2].frame("joined").is_some()
    });
    assert_eq!(again.frame("joined").map(|f| f["generation"].clone()), Some(json!(2)));
    john.send(json!({"rtc": "subscribe", "id": 11, "publisher": callee_leg, "layer": "low"}));
    run_until(&mut [&mut john, &mut callee, &mut again], Duration::from_secs(5), |p| {
        p[0].frames.iter().any(|f| f["id"] == json!(11))
            && p[1].frames.iter().filter(|f| f["rtc"] == json!("error")).count() >= 2
    });
    let stale = john
        .frames
        .iter()
        .find(|f| f["id"] == json!(11))
        .cloned()
        .unwrap_or(Json::Null);
    assert_eq!(stale["code"], json!("staleGeneration"), "{stale}");
    let refused: Vec<&Json> = callee.frames.iter().filter(|f| f["rtc"] == json!("error")).collect();
    assert!(
        refused
            .iter()
            .any(|f| f["id"] == json!(9) && f["code"] == json!("notFound")),
        "{refused:?}"
    );
    assert!(
        refused
            .iter()
            .any(|f| f["id"] == json!(10) && f["code"] == json!("notJoined")),
        "{refused:?}"
    );

    // leaving ends the direct call; the other side hears it ended
    again.send(json!({"rtc": "leave", "id": 12}));
    let ended = run_until(&mut [&mut again, &mut callee], Duration::from_secs(5), |p| {
        p[1].frame("ended").is_some()
    });
    assert!(ended, "callee frames {:?}", callee.frames);
    let state = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        call(
            "org.moqui.impl.CallServices.get#Call",
            &[("callSessionId", Value::string(&call_id))],
        )
    };
    let status = state.get("call").and_then(|c| match c {
        Value::Map(m) => m.get("status").map(to_plain_string),
        _ => None,
    });
    assert_eq!(status.as_deref(), Some("ended"));
}

fn open_guest_socket(address: SocketAddr) -> Ws {
    let uri: tungstenite::http::Uri = format!("ws://{address}/callws").parse().unwrap();
    let (socket, _) = connect(ClientRequestBuilder::new(uri)).unwrap();
    if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        tcp.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    }
    socket
}

fn first_frame(ws: &mut Ws, wait: Duration) -> Option<Json> {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if let Some(frame) = read_frames(ws).into_iter().next() {
            return Some(frame);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn a_guest_by_link_joins_over_its_own_socket_and_revoking_the_link_ends_it() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties("rtc-guest", None, &[("rtc_media_port", "0")]);
    let http = gate.server.address();
    let factory = std::sync::Arc::clone(gate.server.factory());

    let (call_id, host_leg, guest_leg, credential, link_id) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if call("org.moqui.impl.CallServices.get#Capabilities", &[]).get("active") == Some(&Value::Bool(true)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let link = call(
            "org.moqui.impl.CallServices.create#CallLink",
            &[("name", Value::string("Guest test"))],
        );
        let link_id = text(&link, "callLinkId");
        let token = text(&link, "token");
        let opened = call(
            "org.moqui.impl.CallServices.open#CallLink",
            &[("callLinkId", Value::string(&link_id))],
        );
        moqui_core::user::logout().unwrap();
        let guest = call(
            "org.moqui.impl.CallServices.join#AsGuest",
            &[
                ("token", Value::string(&token)),
                ("displayName", Value::string("Visitor")),
            ],
        );
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let state = call(
            "org.moqui.impl.CallServices.get#Call",
            &[("callSessionId", Value::string(text(&opened, "callSessionId")))],
        );
        let waiting = match state.get("call") {
            Some(Value::Map(m)) => match m.get("participants") {
                Some(Value::List(list)) => list
                    .iter()
                    .find_map(|p| match p {
                        Value::Map(p) if p.get("admission").map(to_plain_string).as_deref() == Some("waiting") => {
                            p.get("callParticipantId").map(to_plain_string)
                        }
                        _ => None,
                    })
                    .unwrap_or_default(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        call(
            "org.moqui.impl.CallServices.admit#Guest",
            &[
                ("callSessionId", Value::string(text(&opened, "callSessionId"))),
                ("callParticipantId", Value::string(&waiting)),
            ],
        );
        (
            text(&opened, "callSessionId"),
            text(&opened, "callLegId"),
            text(&guest, "callLegId"),
            text(&guest, "credential"),
            link_id,
        )
    };
    assert!(!call_id.is_empty() && !host_leg.is_empty() && !guest_leg.is_empty() && !credential.is_empty());

    // a socket whose first frame is no credential that works is refused
    let mut bad = open_guest_socket(http);
    bad.send(Message::Text(
        json!({"rtc": "guestAuth", "credential": "x".repeat(40)})
            .to_string()
            .into(),
    ))
    .unwrap();
    let refused = first_frame(&mut bad, Duration::from_secs(5)).expect("an answer");
    assert_eq!(refused["code"], json!("notFound"), "{refused}");

    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    factory.flush_sessions().unwrap();
    let mut host = Peer::new(open_socket(http, &john_cookie), &call_id, &host_leg);
    let mut guest_ws = open_guest_socket(http);
    guest_ws
        .send(Message::Text(
            json!({"rtc": "guestAuth", "credential": credential}).to_string().into(),
        ))
        .unwrap();
    let authed = first_frame(&mut guest_ws, Duration::from_secs(5)).expect("authed");
    assert_eq!(authed["rtc"], json!("guestAuthed"), "{authed}");
    assert_eq!(authed["callLegId"], json!(guest_leg));
    // a guest socket takes no topics
    guest_ws.send(Message::Text("subscribe:ALL".into())).unwrap();
    let mut guest = Peer::new(guest_ws, &call_id, &guest_leg);
    host.join();
    guest.join();
    let flowed = run_until(&mut [&mut host, &mut guest], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 20)
    });
    assert!(
        flowed,
        "audio both ways: host {} guest {}; frames {:?} {:?}",
        host.received, guest.received, host.frames, guest.frames
    );
    // a guest signals only for its own leg
    guest
        .ws
        .send(Message::Text(
            json!({"rtc": "subscribe", "id": 21, "callSessionId": call_id, "callLegId": host_leg, "publisher": guest_leg, "layer": "low"})
                .to_string()
                .into(),
        ))
        .unwrap();
    run_until(&mut [&mut host, &mut guest], Duration::from_secs(3), |p| {
        p[1].frames.iter().any(|f| f["id"] == json!(21))
    });
    let other = guest
        .frames
        .iter()
        .find(|f| f["id"] == json!(21))
        .cloned()
        .unwrap_or(Json::Null);
    assert_eq!(other["code"], json!("notAllowed"), "{other}");
    assert!(
        !guest.frames.iter().any(|f| f.get("rtc").is_none()),
        "no notification reached the guest"
    );

    // revoking the link removes the guest: its page hears the call ended for it
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        call(
            "org.moqui.impl.CallServices.revoke#CallLink",
            &[("callLinkId", Value::string(&link_id))],
        );
    }
    let ended = run_until(&mut [&mut host, &mut guest], Duration::from_secs(5), |p| {
        p[1].frame("ended").is_some()
    });
    assert!(ended, "guest frames {:?}", guest.frames);
}

/// A peer's transport for the W4 tests.
enum Via {
    Udp,
    /// ICE-TCP to the media node's TCP port: RFC 4571 frames on one stream.
    Tcp {
        stream: TcpStream,
        local: SocketAddr,
        remote: SocketAddr,
        buf: Vec<u8>,
    },
    /// Every datagram through a TURN allocation (the UDP socket talks only to the TURN server).
    Turn {
        client: Box<turn_client_proto::udp::TurnClientUdp>,
        relayed: SocketAddr,
        start: Instant,
    },
}

impl Via {
    fn turn_now(start: Instant) -> turn_client_proto::types::Instant {
        turn_client_proto::types::Instant::from_nanos(i64::try_from(start.elapsed().as_nanos()).unwrap())
    }

    fn send(&mut self, udp: &UdpSocket, proto: Protocol, to: SocketAddr, data: &[u8]) {
        use turn_client_proto::api::TurnClientApi;
        match self {
            Via::Udp => {
                let _ = udp.send_to(data, to);
            }
            Via::Tcp { stream, remote, .. } => {
                if proto == Protocol::Tcp && to == *remote {
                    let mut frame = (u16::try_from(data.len()).unwrap()).to_be_bytes().to_vec();
                    frame.extend_from_slice(data);
                    let _ = std::io::Write::write_all(stream, &frame);
                }
            }
            Via::Turn { client, start, .. } => {
                let now = Self::turn_now(*start);
                if !client.have_permission(turn_client_proto::types::TransportType::Udp, to.ip()) {
                    let _ = client.create_permission(turn_client_proto::types::TransportType::Udp, to.ip(), now);
                }
                if let Ok(Some(t)) = client.send_to(turn_client_proto::types::TransportType::Udp, to, data, now) {
                    let t = t.build();
                    let _ = udp.send_to(&t.data, t.to);
                }
                while let Some(t) = client.poll_transmit(now) {
                    let _ = udp.send_to(t.data.as_ref(), t.to);
                }
            }
        }
    }

    /// What arrived: (proto, source, destination, data).
    fn receive(&mut self, udp: &UdpSocket) -> Vec<(Protocol, SocketAddr, SocketAddr, Vec<u8>)> {
        use turn_client_proto::api::{TurnClientApi, TurnRecvRet};
        let mut out = Vec::new();
        let mut buf = [0u8; 2048];
        match self {
            Via::Udp => {
                while let Ok((n, source)) = udp.recv_from(&mut buf) {
                    out.push((Protocol::Udp, source, udp.local_addr().unwrap(), buf[..n].to_vec()));
                }
            }
            Via::Tcp {
                stream,
                local,
                remote,
                buf: pending,
            } => {
                while let Ok(n) = std::io::Read::read(stream, &mut buf) {
                    if n == 0 {
                        break;
                    }
                    pending.extend_from_slice(&buf[..n]);
                }
                while pending.len() >= 2 {
                    let len = usize::from(u16::from_be_bytes([pending[0], pending[1]]));
                    if pending.len() < 2 + len {
                        break;
                    }
                    out.push((Protocol::Tcp, *remote, *local, pending[2..2 + len].to_vec()));
                    pending.drain(..2 + len);
                }
            }
            Via::Turn { client, relayed, start } => {
                let now = Self::turn_now(*start);
                while let Ok((n, from)) = udp.recv_from(&mut buf) {
                    let transmit = turn_client_proto::api::Transmit {
                        data: &buf[..n],
                        transport: turn_client_proto::types::TransportType::Udp,
                        from,
                        to: udp.local_addr().unwrap(),
                    };
                    if let TurnRecvRet::PeerData(peer) = client.recv(transmit, now) {
                        out.push((Protocol::Udp, peer.peer, *relayed, peer.data().to_vec()));
                    }
                }
                let _ = client.poll(now);
                while let Some(t) = client.poll_transmit(now) {
                    let _ = udp.send_to(t.data.as_ref(), t.to);
                }
                while let Some(data) = client.poll_recv(now) {
                    out.push((Protocol::Udp, data.peer, *relayed, data.data().to_vec()));
                }
            }
        }
        out
    }
}

/// A peer that reaches the media node only over ICE-TCP: its one candidate is TCP, and its UDP socket is never used.
fn tcp_peer(ws: Ws, call: &str, leg: &str, media_tcp: SocketAddr) -> Peer {
    let stream = TcpStream::connect(media_tcp).unwrap();
    stream.set_nonblocking(true).unwrap();
    stream.set_nodelay(true).unwrap();
    let local = stream.local_addr().unwrap();
    let mut peer = Peer::new(ws, call, leg);
    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_opus(true, false)
        .enable_vp8(true)
        .build(Instant::now());
    rtc.add_local_candidate(
        Candidate::builder()
            .tcp()
            .host(local)
            .tcptype(str0m::net::TcpType::Active)
            .build()
            .unwrap(),
    );
    peer.rtc = rtc;
    peer.via = Via::Tcp {
        stream,
        local,
        remote: media_tcp,
        buf: Vec::new(),
    };
    peer
}

/// A peer whose datagrams all go through a TURN allocation: its one candidate is the relayed address.
fn relayed_peer(ws: Ws, call: &str, leg: &str, turn_server: SocketAddr, username: &str, password: &str) -> Peer {
    use turn_client_proto::api::{TurnClientApi, TurnEvent};
    let mut peer = Peer::new(ws, call, leg);
    let start = Instant::now();
    let mut config =
        turn_client_proto::api::TurnConfig::new(turn_client_proto::types::TurnCredentials::new(username, password));
    config.set_anonymous_username(turn_client_proto::stun::auth::Feature::Disabled);
    let local = peer.udp.local_addr().unwrap();
    let mut client = turn_client_proto::udp::TurnClientUdp::allocate(local, turn_server, config);
    let mut buf = [0u8; 2048];
    let until = Instant::now() + Duration::from_secs(10);
    let relayed = loop {
        assert!(Instant::now() < until, "no TURN allocation");
        let now = Via::turn_now(start);
        let _ = client.poll(now);
        while let Some(t) = client.poll_transmit(now) {
            let _ = peer.udp.send_to(t.data.as_ref(), t.to);
        }
        while let Ok((n, from)) = peer.udp.recv_from(&mut buf) {
            let transmit = turn_client_proto::api::Transmit {
                data: &buf[..n],
                transport: turn_client_proto::types::TransportType::Udp,
                from,
                to: local,
            };
            let _ = client.recv(transmit, now);
        }
        let mut got = None;
        while let Some(e) = client.poll_event() {
            match e {
                TurnEvent::AllocationCreated(_, relayed) => got = Some(relayed),
                TurnEvent::AllocationCreateFailed(_) => panic!("the TURN allocation failed"),
                _ => {}
            }
        }
        if let Some(relayed) = got {
            break relayed;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut rtc = Rtc::builder()
        .clear_codecs()
        .enable_opus(true, false)
        .enable_vp8(true)
        .build(Instant::now());
    rtc.add_local_candidate(Candidate::relayed(relayed, local, "udp").unwrap());
    peer.rtc = rtc;
    peer.via = Via::Turn { client: Box::new(client), relayed, start };
    peer
}

fn ice_servers(out: &std::collections::BTreeMap<String, Value>) -> (Vec<String>, String, String) {
    let Some(Value::List(list)) = out.get("iceServers") else {
        return (Vec::new(), String::new(), String::new());
    };
    let Some(Value::Map(server)) = list.first() else {
        return (Vec::new(), String::new(), String::new());
    };
    let urls = match server.get("urls") {
        Some(Value::List(urls)) => urls.iter().map(to_plain_string).collect(),
        _ => Vec::new(),
    };
    let field = |k: &str| server.get(k).map(to_plain_string).unwrap_or_default();
    (urls, field("username"), field("credential"))
}

#[test]
fn ice_tcp_and_a_turn_relay_carry_a_call_when_udp_does_not() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties(
        "rtc-turn",
        None,
        &[
            ("rtc_media_port", "0"),
            ("rtc_media_tcp_port", "0"),
            ("turn_port", "0"),
            ("turn_auth_secret", "rtc-turn-test-secret"),
            ("turn_relay_ports", "51000-51100"),
        ],
    );
    let http = gate.server.address();
    let factory = std::sync::Arc::clone(gate.server.factory());

    let (call_id, caller_leg, callee_leg, ice, node_urls) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let created = call(
            "org.moqui.impl.UserServices.create#UserAccount",
            &[
                ("username", Value::string(CALLEE)),
                ("newPassword", Value::string(PASSWORD)),
                ("newPasswordVerify", Value::string(PASSWORD)),
                ("userFullName", Value::string("Rtc Callee")),
            ],
        );
        let callee = text(&created, "userId");
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if call("org.moqui.impl.CallServices.get#Capabilities", &[]).get("active") == Some(&Value::Bool(true)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let node_urls = {
            let _authz = moqui_core::context::ExecutionContext::with_current(
                moqui_core::context::ExecutionContext::disable_authz,
            );
            let rows = factory
                .entity()
                .unwrap()
                .find("moqui.call.CallMediaNode")
                .unwrap()
                .list()
                .unwrap();
            let urls = rows
                .iter()
                .next()
                .and_then(|r| r.get("turnUrls").map(to_plain_string))
                .unwrap_or_default();
            urls
        };
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let started = call(
            "org.moqui.impl.CallServices.start#Call",
            &[("calleeUserIdList", Value::List(vec![Value::string(&callee)]))],
        );
        let call_id = text(&started, "callSessionId");
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        let answered = call(
            "org.moqui.impl.CallServices.answer#Call",
            &[("callSessionId", Value::string(&call_id))],
        );
        let ice = ice_servers(&call(
            "org.moqui.impl.CallServices.get#IceServers",
            &[("callSessionId", Value::string(&call_id))],
        ));
        (
            call_id,
            text(&started, "callLegId"),
            text(&answered, "callLegId"),
            ice,
            node_urls,
        )
    };
    // the media node registered its TURN listeners, and the admitted leg got them with its own credential
    assert!(
        node_urls.contains("turn:") && node_urls.contains("transport=udp"),
        "{node_urls}"
    );
    let (urls, username, credential) = ice;
    assert!(
        urls.iter()
            .any(|u| u.starts_with("turn:") && u.ends_with("transport=udp")),
        "{urls:?}"
    );
    assert!(username.ends_with(&format!(":{callee_leg}")), "{username}");
    assert!(!credential.is_empty());

    let turn_udp: SocketAddr = urls
        .iter()
        .find(|u| u.ends_with("transport=udp"))
        .and_then(|u| u.strip_prefix("turn:"))
        .and_then(|u| u.split('?').next())
        .and_then(|a| a.parse().ok())
        .expect("the TURN UDP address");

    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    let callee_cookie = crate::common::sign_in(http, CALLEE, PASSWORD);
    factory.flush_sessions().unwrap();
    // john is relayed by TURN; the callee joins over UDP first to learn the TCP candidate, then the test checks ICE-TCP
    let mut john = relayed_peer(
        open_socket(http, &john_cookie),
        &call_id,
        &caller_leg,
        turn_udp,
        &username,
        &credential,
    );
    let mut callee = Peer::new(open_socket(http, &callee_cookie), &call_id, &callee_leg);
    john.join();
    callee.join();
    let flowed = run_until(&mut [&mut john, &mut callee], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 20)
    });
    assert!(
        flowed,
        "relayed audio both ways: john {} callee {}; frames {:?}",
        john.received, callee.received, john.frames
    );

    // the callee again, over ICE-TCP only: the TCP candidate of the media node's answer
    let answer = callee
        .frame("joined")
        .and_then(|f| f["sdp"].as_str().map(str::to_string))
        .unwrap_or_default();
    let tcp_addr: SocketAddr = answer
        .lines()
        .find(|l| l.contains(" tcp ") && l.contains("typ host"))
        .and_then(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            Some(SocketAddr::new(
                parts.get(4)?.parse().ok()?,
                parts.get(5)?.parse().ok()?,
            ))
        })
        .expect("a TCP candidate in the answer");
    let mut again = tcp_peer(open_socket(http, &callee_cookie), &call_id, &callee_leg, tcp_addr);
    again.join();
    john.received = 0;
    let flowed = run_until(&mut [&mut john, &mut again], Duration::from_secs(10), |p| {
        p.iter().all(|p| p.received >= 20)
    });
    assert!(
        flowed,
        "ICE-TCP audio both ways: john {} callee {}; frames {:?}",
        john.received, again.received, again.frames
    );
}

/// The `call` helper's map values, for the lists `find#Recordings` gives.
fn list_of(value: Option<&Value>) -> Vec<std::collections::BTreeMap<String, Value>> {
    match value {
        Some(Value::List(list)) => list
            .iter()
            .filter_map(|v| match v {
                Value::Map(m) => Some(m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn a_recorded_call_stores_each_leg_s_track_and_the_mixed_audio_in_segments() {
    if !crate::common::own_process() {
        return;
    }
    let gate = crate::common::start_with_properties(
        "rtc-record",
        None,
        &[("rtc_media_port", "0"), ("call_recording_enabled", "true")],
    );
    let http = gate.server.address();
    let factory = std::sync::Arc::clone(gate.server.factory());
    let (call_id, caller_leg, callee_leg) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let created = call(
            "org.moqui.impl.UserServices.create#UserAccount",
            &[
                ("username", Value::string(CALLEE)),
                ("newPassword", Value::string(PASSWORD)),
                ("newPasswordVerify", Value::string(PASSWORD)),
                ("userFullName", Value::string("Rtc Callee")),
            ],
        );
        let callee = text(&created, "userId");
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if call("org.moqui.impl.CallServices.get#Capabilities", &[]).get("active") == Some(&Value::Bool(true)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let started = call(
            "org.moqui.impl.CallServices.start#Call",
            &[("calleeUserIdList", Value::List(vec![Value::string(&callee)]))],
        );
        let call_id = text(&started, "callSessionId");
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        let answered = call(
            "org.moqui.impl.CallServices.answer#Call",
            &[("callSessionId", Value::string(&call_id))],
        );
        (call_id, text(&started, "callLegId"), text(&answered, "callLegId"))
    };
    let john_cookie = crate::common::sign_in(http, "john.doe", "moqui");
    let callee_cookie = crate::common::sign_in(http, CALLEE, PASSWORD);
    factory.flush_sessions().unwrap();
    let mut john = Peer::new(open_socket(http, &john_cookie), &call_id, &caller_leg);
    let mut callee = Peer::new(open_socket(http, &callee_cookie), &call_id, &callee_leg);
    john.join();
    callee.join();
    assert!(
        run_until(&mut [&mut john, &mut callee], Duration::from_secs(5), |p| p
            .iter()
            .all(|p| p.received >= 5)),
        "audio both ways first"
    );

    // john, the host, records; both legs hear of it on their sockets
    let as_john = |name: &str, parameters: &[(&str, Value)]| {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        call(name, parameters)
    };
    let started = as_john(
        "org.moqui.impl.CallRecordingServices.start#Recording",
        &[("callSessionId", Value::string(&call_id))],
    );
    let recording_id = text(&started, "callRecordingId");
    assert!(!recording_id.is_empty());
    let noticed = |p: &Peer, state: &str| {
        p.frames.iter().any(|f| {
            f["rtc"] == json!("recording") && f["state"] == json!(state) && f["callRecordingId"] == json!(recording_id)
        })
    };
    assert!(
        run_until(&mut [&mut john, &mut callee], Duration::from_secs(3), |p| {
            noticed(p[0], "recording") && noticed(p[1], "recording")
        }),
        "the notice: john {:?} callee {:?}",
        john.frames,
        callee.frames
    );
    // a second of audio both ways while recording, then the stop
    run_until(&mut [&mut john, &mut callee], Duration::from_millis(1_000), |_| false);
    as_john(
        "org.moqui.impl.CallRecordingServices.stop#Recording",
        &[("callSessionId", Value::string(&call_id))],
    );
    let mut recordings = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        run_until(&mut [&mut john, &mut callee], Duration::from_millis(100), |_| false);
        let found = as_john(
            "org.moqui.impl.CallRecordingServices.find#Recordings",
            &[("callSessionId", Value::string(&call_id))],
        );
        recordings = list_of(found.get("recordingList"));
        let segments = recordings
            .first()
            .map(|r| {
                list_of(r.get("periods"))
                    .iter()
                    .map(|p| list_of(p.get("segments")).len())
                    .sum::<usize>()
            })
            .unwrap_or(0);
        if segments >= 3 {
            break;
        }
    }
    assert!(
        noticed(&john, "stopped") || noticed(&callee, "stopped"),
        "the stop's notice"
    );
    let recording = recordings.first().expect("john's recording");
    assert_eq!(recording.get("state").map(to_plain_string).as_deref(), Some("stopped"));
    let periods = list_of(recording.get("periods"));
    assert_eq!(periods.len(), 1, "{periods:?}");
    let segments = list_of(periods[0].get("segments"));
    let kind_of =
        |s: &std::collections::BTreeMap<String, Value>, k: &str| s.get(k).map(to_plain_string).unwrap_or_default();
    let mut legs: Vec<String> = segments
        .iter()
        .filter(|s| kind_of(s, "trackKind") == "audio")
        .map(|s| kind_of(s, "callLegId"))
        .collect();
    legs.sort();
    let mut expected = vec![caller_leg.clone(), callee_leg.clone()];
    expected.sort();
    assert_eq!(legs, expected, "a track per leg: {segments:?}");
    assert_eq!(
        segments.iter().filter(|s| kind_of(s, "trackKind") == "mix").count(),
        1,
        "the mixed audio"
    );
    for segment in &segments {
        let file = as_john(
            "org.moqui.impl.CallRecordingServices.get#RecordingFile",
            &[("callRecordingSegmentId", segment["callRecordingSegmentId"].clone())],
        );
        let location = text(&file, "location");
        let bytes = {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = moqui_core::context::ExecutionContext::with_current(
                moqui_core::context::ExecutionContext::disable_authz,
            );
            factory
                .resource()
                .location_bytes(&location)
                .unwrap()
                .unwrap_or_default()
        };
        assert_eq!(&bytes[..4.min(bytes.len())], b"OggS", "{location}");
        assert!(bytes.len() > 200, "{location}: {} bytes", bytes.len());
    }
    // the spool is empty again, and the call went on
    let spool = factory.runtime_dir().join("tmp/call-recording").join(&recording_id);
    assert!(!spool.exists(), "the spool of the recording is gone");
}
