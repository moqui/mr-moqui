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

use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::format::Codec;
use str0m::media::{Direction, Frequency, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, Input, Output, Rtc};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, ClientRequestBuilder, Message, WebSocket};

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

const CALLEE: &str = "rtc.callee";
const PASSWORD: &str = "Rtc-callee-77!";

fn open_socket(address: SocketAddr, cookie: &str) -> Ws {
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
struct Peer {
    rtc: Rtc,
    udp: UdpSocket,
    ws: Ws,
    call: String,
    leg: String,
    pending: Option<SdpPendingOffer>,
    audio: Option<Mid>,
    received: usize,
    frames: Vec<Json>,
    start: Instant,
}

impl Peer {
    fn new(ws: Ws, call: &str, leg: &str) -> Self {
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
            call: call.into(),
            leg: leg.into(),
            pending: None,
            audio: None,
            received: 0,
            frames: Vec::new(),
            start: Instant::now(),
        }
    }

    fn send(&mut self, frame: Json) {
        let mut frame = frame;
        frame["callSessionId"] = json!(self.call);
        frame["callLegId"] = json!(self.leg);
        self.ws.send(Message::Text(frame.to_string().into())).unwrap();
    }

    fn join(&mut self) {
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
        let mut buf = [0u8; 2048];
        while let Ok((n, source)) = self.udp.recv_from(&mut buf) {
            let receive = Receive {
                proto: Protocol::Udp,
                source,
                destination: self.udp.local_addr().unwrap(),
                contents: (&buf[..n]).try_into().unwrap(),
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
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Timeout(_)) | Err(_) => break,
                Ok(Output::Transmit(t)) => {
                    let _ = self.udp.send_to(&t.contents, t.destination);
                }
                Ok(Output::Event(Event::MediaData(d)))
                    if self.rtc.media(d.mid).map(|m| m.kind()) == Some(MediaKind::Audio) =>
                {
                    self.received += 1
                }
                Ok(Output::Event(_)) => {}
            }
        }
    }

    fn frame(&self, kind: &str) -> Option<&Json> {
        self.frames.iter().find(|f| f["rtc"] == json!(kind))
    }
}

fn run_until(peers: &mut [&mut Peer], wait: Duration, done: impl Fn(&[&mut Peer]) -> bool) -> bool {
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

fn call(name: &str, parameters: &[(&str, Value)]) -> std::collections::BTreeMap<String, Value> {
    let parameters = parameters.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let out = moqui_core::service::sync(name, &parameters).unwrap();
    moqui_core::context::ExecutionContext::with_current(|ec| {
        assert!(ec.message().errors().is_empty(), "{name}: {:?}", ec.message().errors());
    });
    out
}

fn text(out: &std::collections::BTreeMap<String, Value>, key: &str) -> String {
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
