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

//! Phone calls on a real server (plans/Comms-Framework.md §5.2, V1): a fake trunk (a raw SIP peer on a loopback UDP
//! socket, with its own RTP socket) registers the trunk, takes an outbound call and places an inbound one, and a
//! browser stand-in (the str0m peer of `rtc.rs`) hears the phone's audio through the call's SFU room.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use moqui_lib::object::{to_plain_string, Value};
use moqui_sip::media::PhoneMedia;
use moqui_sip::sdp::{self, LocalMedia};
use rsipstack::sip::prelude::HeadersExt;
use rsipstack::sip::{Request, Response, SipMessage};
use serde_json::json;

use crate::rtc::{call, open_socket, run_until, text, Peer};

const NUMBER: &str = "+15551230000";
const OUTSIDE: &str = "+15557770000";

/// The provider's signaling and media, message by message.
struct Trunk {
    sip: UdpSocket,
    rtp: UdpSocket,
}

fn values(headers: &rsipstack::sip::Headers, name: &str) -> Vec<String> {
    headers
        .iter()
        .filter(|h| h.name().eq_ignore_ascii_case(name))
        .map(|h| h.value().to_string())
        .collect()
}

impl Trunk {
    fn new() -> Trunk {
        let sip = UdpSocket::bind("127.0.0.1:0").unwrap();
        sip.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
        rtp.set_nonblocking(true).unwrap();
        Trunk { sip, rtp }
    }

    fn addr(&self) -> SocketAddr {
        self.sip.local_addr().unwrap()
    }

    fn recv(&self, until: Instant) -> Option<(SipMessage, SocketAddr)> {
        let mut buf = vec![0u8; 8192];
        while Instant::now() < until {
            let Ok((len, from)) = self.sip.recv_from(&mut buf) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&buf[..len]) else {
                continue;
            };
            if let Ok(message) = SipMessage::try_from(text) {
                return Some((message, from));
            }
        }
        None
    }

    fn request(&self, method: &str) -> (Request, SocketAddr) {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match self.recv(until) {
                Some((SipMessage::Request(req), from)) if req.method.to_string() == method => return (req, from),
                Some(_) => continue,
                None => panic!("the trunk got no {method}"),
            }
        }
    }

    fn response(&self, code: u16) -> Response {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match self.recv(until) {
                Some((SipMessage::Response(resp), _)) if u16::from(resp.status_code.clone()) == code => return resp,
                Some(_) => continue,
                None => panic!("the trunk got no {code}"),
            }
        }
    }

    fn send(&self, text: &str, to: SocketAddr) {
        self.sip.send_to(text.as_bytes(), to).unwrap();
    }

    fn reply(&self, req: &Request, to: SocketAddr, code: u16, extra: &str, body: &str) {
        let vias: String = values(&req.headers, "Via")
            .iter()
            .map(|v| format!("Via: {v}\r\n"))
            .collect();
        let to_header = req.to_header().unwrap().value().to_string();
        let to_header = if to_header.contains(";tag=") {
            to_header
        } else {
            format!("{to_header};tag=trunk")
        };
        let content = if body.is_empty() {
            ""
        } else {
            "Content-Type: application/sdp\r\n"
        };
        let text = format!(
            "SIP/2.0 {code} Reason\r\n{vias}From: {}\r\nTo: {to_header}\r\nCall-ID: {}\r\nCSeq: {}\r\n\
             Contact: <sip:trunk@{}>\r\n{extra}{content}Content-Length: {}\r\n\r\n{body}",
            req.from_header().unwrap().value(),
            req.call_id_header().unwrap().value(),
            req.cseq_header().unwrap().value(),
            self.addr(),
            body.len()
        );
        self.send(&text, to);
    }

    fn local_media(&self) -> LocalMedia {
        let at = self.rtp.local_addr().unwrap();
        LocalMedia::new(at.ip(), at.port(), None)
    }
}

fn tone(tick: usize) -> Vec<i16> {
    (0..960)
        .map(|i| (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * (tick * 960 + i) as f64 / 48_000.0).sin()) as i16)
        .collect()
}

/// The phone talks while the browser stand-in runs: its RTP out, ours in; how many packets the phone got.
fn talk(trunk: &Trunk, phone: &mut PhoneMedia, peer: &mut Peer, wait: Duration, frames: usize) -> usize {
    let until = Instant::now() + wait;
    let mut tick = 0;
    let mut got = 0;
    let mut buf = vec![0u8; 2048];
    let mut next = Instant::now();
    while Instant::now() < until {
        if Instant::now() >= next {
            next += Duration::from_millis(20);
            if let Some(packet) = phone.send(&tone(tick)) {
                trunk.rtp.send_to(&packet, phone.remote()).unwrap();
            }
            tick += 1;
        }
        while let Ok((len, from)) = trunk.rtp.recv_from(&mut buf) {
            phone.receive(from, &buf[..len]);
            got += 1;
        }
        run_until(&mut [&mut *peer], Duration::from_millis(5), |_| true);
        if peer.received >= frames && got >= frames {
            break;
        }
    }
    got
}

fn status_of(out: &std::collections::BTreeMap<String, Value>) -> Option<String> {
    out.get("call").and_then(|c| match c {
        Value::Map(m) => m.get("status").map(to_plain_string),
        _ => None,
    })
}

#[test]
fn a_phone_call_each_way_through_a_fake_trunk_heard_by_a_browser() {
    if !crate::common::own_process() {
        return;
    }
    let mut trunk = Trunk::new();
    let gate = crate::common::start_with_properties(
        "sip",
        None,
        &[
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42300-42399"),
        ],
    );
    let http = gate.server.address();
    let sip_at = gate.server.sip_address().expect("the SIP listener is up");
    let factory = std::sync::Arc::clone(gate.server.factory());

    // the trunk, its number for john and his caller ID; the engine reloads and registers the trunk
    let john = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        // the media node and the SIP engine register as they start; wait for both rows
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            let status = call("org.moqui.impl.VoiceServices.get#VoiceStatus", &[]);
            let up = |key: &str| matches!(status.get(key), Some(Value::List(list)) if !list.is_empty());
            if up("sipNodeList") && up("mediaNodeList") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let trunk_id = text(
            &call(
                "org.moqui.impl.VoiceServices.store#VoiceTrunk",
                &[
                    ("name", Value::string("Fake trunk")),
                    ("host", Value::string("127.0.0.1")),
                    ("port", Value::I32(i32::from(trunk.addr().port()))),
                    ("transportEnumId", Value::string("VtrUdp")),
                    ("mediaEncryptionEnumId", Value::string("VmePlain")),
                    ("providerProfileEnumId", Value::string("VppPlainLan")),
                    ("username", Value::string("acct")),
                    ("password", Value::string("pw-of-acct")),
                    ("registerFlag", Value::string("Y")),
                ],
            ),
            "voiceTrunkId",
        );
        let john = moqui_core::user::current_user_id().unwrap().expect("john is signed in");
        call(
            "org.moqui.impl.VoiceServices.store#PhoneNumber",
            &[
                ("phoneNumber", Value::string(NUMBER)),
                ("voiceTrunkId", Value::string(&trunk_id)),
                ("routeTypeEnumId", Value::string("PnrUser")),
                ("routeId", Value::string(&john)),
            ],
        );
        call(
            "org.moqui.impl.VoiceServices.grant#CallerId",
            &[("phoneNumber", Value::string(NUMBER)), ("userId", Value::string(&john))],
        );
        call(
            "org.moqui.impl.VoiceServices.store#VoiceUserSetting",
            &[
                ("userId", Value::string(&john)),
                ("outboundEnabled", Value::string("Y")),
            ],
        );
        john
    };
    let (register, from) = trunk.request("REGISTER");
    let contact = values(&register.headers, "Contact")[0].clone();
    trunk.reply(&register, from, 200, &format!("Contact: {contact};expires=300\r\n"), "");
    let cookie = crate::common::sign_in(http, "john.doe", "moqui");
    factory.flush_sessions().unwrap();

    // ---- outbound: john calls a number; the trunk answers; john's browser hears the phone ----
    let placed = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        call(
            "org.moqui.impl.VoiceServices.place#PhoneCall",
            &[
                ("phoneNumber", Value::string(OUTSIDE)),
                ("fromNumber", Value::string(NUMBER)),
            ],
        )
    };
    let (call_id, web_leg) = (text(&placed, "callSessionId"), text(&placed, "callLegId"));
    let (invite, from) = trunk.request("INVITE");
    assert!(invite.uri.to_string().contains(OUTSIDE), "{}", invite.uri);
    assert!(invite.from_header().unwrap().value().contains(NUMBER), "the caller ID");
    let offer = sdp::parse(&String::from_utf8_lossy(&invite.body)).unwrap();
    let (answer, settled) = sdp::answer(&offer, &trunk.local_media()).unwrap();
    trunk.reply(&invite, from, 200, "", &answer);
    trunk.request("ACK");
    let mut phone = PhoneMedia::new(&settled).unwrap();
    let mut peer = Peer::new(open_socket(http, &cookie), &call_id, &web_leg);
    peer.join();
    let got = talk(&trunk, &mut phone, &mut peer, Duration::from_secs(6), 20);
    assert!(
        peer.received >= 20,
        "the browser hears the phone: {} frames; {:?}",
        peer.received,
        peer.frames
    );
    assert!(got >= 20, "the phone gets the call's audio: {got} packets");
    // john hangs up: the trunk gets a BYE
    peer.send(json!({"rtc": "leave", "id": 5}));
    let (bye, from) = trunk.request("BYE");
    trunk.reply(&bye, from, 200, "", "");

    // ---- inbound: the trunk calls john's number; he answers; the trunk hangs up ----
    // the phone of this call has a socket of its own: the last call's leg may still have packets on the way
    trunk.rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
    trunk.rtp.set_nonblocking(true).unwrap();
    let phone_local = trunk.local_media();
    let offer = sdp::offer(&phone_local);
    let invite = format!(
        "INVITE sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-in1\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=in1\r\nTo: <sip:{NUMBER}@{sip_at}>\r\nCall-ID: in-call-1\r\nCSeq: 1 INVITE\r\n\
         Contact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        trunk.addr(),
        trunk.addr(),
        offer.len()
    );
    trunk.send(&invite, sip_at);
    trunk.response(180);
    let (call_id, web_leg) = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let active = call("org.moqui.impl.CallServices.find#ActiveCalls", &[]);
        let ringing = match active.get("callList") {
            Some(Value::List(list)) => list.first().cloned(),
            _ => None,
        };
        let call_id = match ringing {
            Some(Value::Map(m)) => m.get("callSessionId").map(to_plain_string).unwrap_or_default(),
            _ => panic!("the call rings for john: {active:?}"),
        };
        let answered = call(
            "org.moqui.impl.CallServices.answer#Call",
            &[("callSessionId", Value::string(&call_id))],
        );
        (call_id, text(&answered, "callLegId"))
    };
    let ok = trunk.response(200);
    let settled = sdp::accept(&phone_local, &sdp::parse(&String::from_utf8_lossy(&ok.body)).unwrap()).unwrap();
    let to = ok.to_header().unwrap().value().to_string();
    let in_dialog = |method: &str, seq: u32| {
        format!(
            "{method} sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{method}{seq}\r\n\
             Max-Forwards: 70\r\nFrom: <sip:{OUTSIDE}@127.0.0.1>;tag=in1\r\nTo: {to}\r\nCall-ID: in-call-1\r\n\
             CSeq: {seq} {method}\r\nContent-Length: 0\r\n\r\n",
            trunk.addr()
        )
    };
    trunk.send(&in_dialog("ACK", 1), sip_at);
    let mut phone = PhoneMedia::new(&settled).unwrap();
    let mut peer = Peer::new(open_socket(http, &cookie), &call_id, &web_leg);
    peer.join();
    let got = talk(&trunk, &mut phone, &mut peer, Duration::from_secs(6), 20);
    assert!(
        peer.received >= 20,
        "the browser hears the caller: {} frames",
        peer.received
    );
    assert!(got >= 20, "the caller gets the call's audio: {got} packets");
    trunk.send(&in_dialog("BYE", 2), sip_at);
    trunk.response(200);
    let ended = run_until(&mut [&mut peer], Duration::from_secs(5), |p| {
        p[0].frame("ended").is_some()
    });
    assert!(ended, "the call ends when the phone hangs up: {:?}", peer.frames);
    let state = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        call(
            "org.moqui.impl.CallServices.get#Call",
            &[("callSessionId", Value::string(&call_id))],
        )
    };
    assert_eq!(status_of(&state).as_deref(), Some("ended"));
    let _ = john;
}
