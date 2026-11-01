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

/// Voicemail (V2) on a real server: the trunk calls john, who is in another call, so the call goes straight to his box.
/// The engine answers it, plays the greeting and the beep, records what the caller says until `#`, says goodbye and hangs
/// up; the message is stored as Ogg Opus in john's inbox, and john missed the call.
#[test]
fn a_busy_user_s_caller_leaves_a_voicemail_through_a_fake_trunk() {
    if !crate::common::own_process() {
        return;
    }
    let trunk = Trunk::new();
    let gate = crate::common::start_with_properties(
        "sip-voicemail",
        None,
        &[
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42400-42499"),
        ],
    );
    let sip_at = gate.server.sip_address().expect("the SIP listener is up");
    let factory = std::sync::Arc::clone(gate.server.factory());
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
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
        // john is in another call
        call(
            "store#moqui.security.UserStatus",
            &[("userId", Value::string(&john)), ("inCall", Value::string("Y"))],
        );
    }
    let (register, from) = trunk.request("REGISTER");
    let contact = values(&register.headers, "Contact")[0].clone();
    trunk.reply(&register, from, 200, &format!("Contact: {contact};expires=300\r\n"), "");

    let started = Instant::now();
    let phone_local = trunk.local_media();
    let offer = sdp::offer(&phone_local);
    let invite = format!(
        "INVITE sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-vm1\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=vm1\r\nTo: <sip:{NUMBER}@{sip_at}>\r\nCall-ID: vm-call-1\r\nCSeq: 1 INVITE\r\n\
         Contact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        trunk.addr(),
        trunk.addr(),
        offer.len()
    );
    trunk.send(&invite, sip_at);
    // no ringing for a busy user: the engine answers for voicemail
    let ok = trunk.response(200);
    let settled = sdp::accept(&phone_local, &sdp::parse(&String::from_utf8_lossy(&ok.body)).unwrap()).unwrap();
    let to = ok.to_header().unwrap().value().to_string();
    let ack = format!(
        "ACK sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-vmack\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=vm1\r\nTo: {to}\r\nCall-ID: vm-call-1\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n",
        trunk.addr()
    );
    trunk.send(&ack, sip_at);
    let mut phone = PhoneMedia::new(&settled).unwrap();

    // the caller talks over the greeting and the beep (1.4 s) and after it, then presses #
    let mut buf = vec![0u8; 2048];
    let mut heard = 0;
    let mut tick = 0;
    let mut next = Instant::now();
    let hash_at = Instant::now() + Duration::from_millis(3000);
    let until = Instant::now() + Duration::from_millis(3600);
    let mut pressed = false;
    while Instant::now() < until {
        if Instant::now() >= next {
            next += Duration::from_millis(20);
            if !pressed && Instant::now() >= hash_at {
                phone.send_digits("#");
                pressed = true;
            }
            if let Some(packet) = phone.send(&tone(tick)) {
                trunk.rtp.send_to(&packet, phone.remote()).unwrap();
            }
            tick += 1;
        }
        while let Ok((len, from)) = trunk.rtp.recv_from(&mut buf) {
            phone.receive(from, &buf[..len]);
            heard += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        heard >= 50,
        "the caller hears the greeting and the beep: {heard} packets"
    );
    // goodbye, then the engine hangs up
    let (bye, from) = trunk.request("BYE");
    trunk.reply(&bye, from, 200, "", "");

    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    let voicemail = loop {
        let found = call("org.moqui.impl.VoicemailServices.find#Voicemails", &[]);
        if let Some(Value::List(list)) = found.get("voicemailList") {
            if let Some(Value::Map(first)) = list.first() {
                break first.clone();
            }
        }
        assert!(Instant::now() < until, "the voicemail is stored: {found:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    let duration: u64 = voicemail
        .get("durationMs")
        .map(to_plain_string)
        .unwrap_or_default()
        .parse()
        .unwrap_or(0);
    assert!(
        (1000..=2600).contains(&duration),
        "about 1.6 s recorded after the beep: {duration} ms"
    );
    assert_eq!(
        voicemail.get("fromNumber").map(to_plain_string).as_deref(),
        Some(OUTSIDE)
    );
    let voicemail_id = voicemail.get("voicemailId").map(to_plain_string).unwrap_or_default();
    let file = call(
        "org.moqui.impl.VoicemailServices.get#VoicemailAudio",
        &[("voicemailId", Value::string(&voicemail_id))],
    );
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let bytes = factory
        .resource()
        .location_bytes(&text(&file, "location"))
        .unwrap()
        .expect("the audio is stored");
    assert!(bytes.starts_with(b"OggS"), "Ogg Opus");
    let missed = call("org.moqui.impl.CallServices.count#MissedCalls", &[]);
    assert_eq!(text(&missed, "missedCount"), "1", "john missed the call");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
}

/// A 16-bit PCM mono WAV at 8 kHz: `millis` of a 440 Hz tone.
fn wav_tone(millis: usize) -> Vec<u8> {
    let samples: Vec<i16> = (0..millis * 8)
        .map(|i| (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 8000.0).sin()) as i16)
        .collect();
    let data_len = u32::try_from(samples.len() * 2).unwrap();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
}

/// Phone access (V2) on a real server: a caller on the access number keys john's box number and `#`, his PIN and `#`;
/// the engine checks it, plays his new message (Ogg Opus decoded to the leg), and `7` sends it to the trash; then no more
/// messages, goodbye, and the engine hangs up.
#[test]
fn the_access_number_takes_a_box_and_pin_plays_a_message_and_7_deletes_it() {
    if !crate::common::own_process() {
        return;
    }
    const ACCESS: &str = "+15551239999";
    let trunk = Trunk::new();
    let gate = crate::common::start_with_properties(
        "sip-voicemail-access",
        None,
        &[
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42500-42599"),
        ],
    );
    let sip_at = gate.server.sip_address().expect("the SIP listener is up");
    let factory = std::sync::Arc::clone(gate.server.factory());
    let voicemail_id = "VM_IT_ACCESS";
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
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
        for (number, route, route_id) in [(NUMBER, "PnrUser", john.as_str()), (ACCESS, "PnrVoicemail", "")] {
            call(
                "org.moqui.impl.VoiceServices.store#PhoneNumber",
                &[
                    ("phoneNumber", Value::string(number)),
                    ("voiceTrunkId", Value::string(&trunk_id)),
                    ("routeTypeEnumId", Value::string(route)),
                    ("routeId", Value::string(route_id)),
                ],
            );
        }
        call(
            "org.moqui.impl.VoicemailServices.set#VoicemailPin",
            &[("pin", Value::string("471935"))],
        );
        let settings = call("org.moqui.impl.VoicemailServices.get#VoicemailSettings", &[]);
        let Some(Value::Map(mailbox)) = settings.get("box") else {
            panic!("john has a box: {settings:?}")
        };
        let box_id = mailbox.get("voicemailBoxId").map(to_plain_string).unwrap_or_default();
        // the message: a second of tone, made Ogg Opus by the greeting conversion
        let source = "dbresource://moqui/call/voicemail/greeting/it/message.wav";
        let audio = "dbresource://moqui/call/voicemail/greeting/it/message.ogg";
        {
            let _authz = moqui_core::context::ExecutionContext::with_current(
                moqui_core::context::ExecutionContext::disable_authz,
            );
            factory.resource().write_bytes(source, &wav_tone(1000)).unwrap();
        }
        call(
            "org.moqui.impl.VoicemailServices.convert#GreetingAudio",
            &[
                ("sourceLocation", Value::string(source)),
                ("targetLocation", Value::string(audio)),
            ],
        );
        call(
            "store#moqui.call.Voicemail",
            &[
                ("voicemailId", Value::string(voicemail_id)),
                ("voicemailBoxId", Value::string(&box_id)),
                ("fromNumber", Value::string(OUTSIDE)),
                ("durationMs", Value::I64(1000)),
                ("audioLocation", Value::string(audio)),
                ("folder", Value::string("inbox")),
                ("statusId", Value::string("VmsStored")),
            ],
        );
    }
    let (register, from) = trunk.request("REGISTER");
    let contact = values(&register.headers, "Contact")[0].clone();
    trunk.reply(&register, from, 200, &format!("Contact: {contact};expires=300\r\n"), "");

    let started = Instant::now();
    let phone_local = trunk.local_media();
    let offer = sdp::offer(&phone_local);
    let invite = format!(
        "INVITE sip:{ACCESS}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-va1\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=va1\r\nTo: <sip:{ACCESS}@{sip_at}>\r\nCall-ID: va-call-1\r\nCSeq: 1 INVITE\r\n\
         Contact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        trunk.addr(),
        trunk.addr(),
        offer.len()
    );
    trunk.send(&invite, sip_at);
    let ok = trunk.response(200);
    let settled = sdp::accept(&phone_local, &sdp::parse(&String::from_utf8_lossy(&ok.body)).unwrap()).unwrap();
    let to = ok.to_header().unwrap().value().to_string();
    let ack = format!(
        "ACK sip:{ACCESS}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-vaack\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=va1\r\nTo: {to}\r\nCall-ID: va-call-1\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n",
        trunk.addr()
    );
    trunk.send(&ack, sip_at);
    let mut phone = PhoneMedia::new(&settled).unwrap();

    // the box number (the last digits of john's number) and #, the PIN and # (13 digits, 160 ms each); then 7 while
    // the message plays
    assert_eq!(phone.send_digits("30000#471935#"), 13);
    let mut buf = vec![0u8; 2048];
    let mut next = Instant::now();
    // the PIN is checked by about 2.3 s; the message, then its menu three times, takes until about 5 s
    let seven_at = Instant::now() + Duration::from_millis(3400);
    let until = Instant::now() + Duration::from_millis(3800);
    let mut pressed = false;
    let silence = vec![0i16; 960];
    while Instant::now() < until {
        if Instant::now() >= next {
            next += Duration::from_millis(20);
            if !pressed && Instant::now() >= seven_at {
                phone.send_digits("7");
                pressed = true;
            }
            if let Some(packet) = phone.send(&silence) {
                trunk.rtp.send_to(&packet, phone.remote()).unwrap();
            }
        }
        while let Ok((len, from)) = trunk.rtp.recv_from(&mut buf) {
            phone.receive(from, &buf[..len]);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // no more messages, goodbye, then the engine hangs up
    let (bye, from) = trunk.request("BYE");
    trunk.reply(&bye, from, 200, "", "");

    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let trash = call(
        "org.moqui.impl.VoicemailServices.find#Voicemails",
        &[("folder", Value::string("trash"))],
    );
    let in_trash = matches!(trash.get("voicemailList"), Some(Value::List(list))
        if list.iter().any(|v| matches!(v, Value::Map(m) if m.get("voicemailId").map(to_plain_string).as_deref() == Some(voicemail_id))));
    assert!(in_trash, "7 sent the message to the trash: {trash:?}");
    assert!(started.elapsed() < Duration::from_secs(8), "{:?}", started.elapsed());
}

/// The phone's REGISTER, unsigned or with its digest answer (V4).
fn register_text(phone: SocketAddr, server: SocketAddr, user: &str, cseq: u32, auth: Option<&str>) -> String {
    let auth = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    format!(
        "REGISTER sip:{server} SIP/2.0\r\nVia: SIP/2.0/UDP {phone};branch=z9hG4bK-reg{cseq};rport\r\nMax-Forwards: 70\r\n\
         From: <sip:{user}@{server}>;tag=desk\r\nTo: <sip:{user}@{server}>\r\nCall-ID: desk-reg-1\r\nCSeq: {cseq} REGISTER\r\n\
         Contact: <sip:{user}@{phone}>\r\nExpires: 300\r\n{auth}User-Agent: Spec Desk Phone\r\nContent-Length: 0\r\n\r\n"
    )
}

/// The registrar (V4) on a real server: a user's desk phone registers with the credentials it was given once, a call
/// from a colleague rings it, it answers in G.722, and the colleague's browser hears it.
#[test]
fn a_registered_desk_phone_rings_answers_and_is_heard() {
    if !crate::common::own_process() {
        return;
    }
    const CALLEE: &str = "sip.desk";
    const PASSWORD: &str = "Sip-desk-77!";
    // the phone: its signaling and media sockets
    let phone = Trunk::new();
    let gate = crate::common::start_with_properties(
        "sip",
        None,
        &[
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42400-42499"),
            ("sip_registrar_enabled", "true"),
        ],
    );
    let http = gate.server.address();
    let sip_at = gate.server.sip_address().expect("the SIP listener is up");
    let factory = std::sync::Arc::clone(gate.server.factory());
    let (callee, credentials) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let created = call(
            "org.moqui.impl.UserServices.create#UserAccount",
            &[
                ("username", Value::string(CALLEE)),
                ("newPassword", Value::string(PASSWORD)),
                ("newPasswordVerify", Value::string(PASSWORD)),
                ("userFullName", Value::string("Desk Callee")),
            ],
        );
        let callee = text(&created, "userId");
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        let device = call(
            "org.moqui.impl.SipDeviceServices.create#SipDevice",
            &[("name", Value::string("Front desk"))],
        );
        (callee, device)
    };
    let (user, password) = (text(&credentials, "username"), text(&credentials, "password"));
    assert_eq!(credentials.get("registrarEnabled"), Some(&Value::Bool(true)));

    // the phone registers: challenged, then signed with SHA-256
    let at = phone.addr();
    phone.send(&register_text(at, sip_at, &user, 1, None), sip_at);
    let challenge = phone.response(401);
    let challenges: Vec<moqui_sip::digest::Challenge> = values(&challenge.headers, "WWW-Authenticate")
        .iter()
        .filter_map(|v| moqui_sip::digest::Challenge::parse(v))
        .collect();
    let credentials = moqui_sip::digest::Credentials {
        username: user.clone(),
        password,
        algorithms: vec![moqui_sip::digest::Algorithm::Sha256],
        ..Default::default()
    };
    let chosen = moqui_sip::digest::choose(&challenges, &credentials).expect("a SHA-256 challenge");
    let auth = moqui_sip::digest::answer(&credentials, chosen, "REGISTER", &format!("sip:{sip_at}"), "c1").unwrap();
    phone.send(&register_text(at, sip_at, &user, 2, Some(&auth)), sip_at);
    let ok = phone.response(200);
    assert!(
        values(&ok.headers, "Contact")[0].contains("expires="),
        "the binding's expiry"
    );

    // the binding is kept as a row; then john calls the callee, whose phone rings beside the tabs
    let (call_id, caller_leg) = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user(CALLEE).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let listed = call("org.moqui.impl.SipDeviceServices.find#SipDevices", &[]);
            let registered = match listed.get("deviceList") {
                Some(Value::List(list)) => list.iter().any(|d| match d {
                    Value::Map(m) => matches!(m.get("registrationList"), Some(Value::List(r)) if !r.is_empty()),
                    _ => false,
                }),
                _ => false,
            };
            if registered {
                break;
            }
            assert!(Instant::now() < until, "the registration is kept: {listed:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let started = call(
            "org.moqui.impl.CallServices.start#Call",
            &[("calleeUserIdList", Value::List(vec![Value::string(&callee)]))],
        );
        (text(&started, "callSessionId"), text(&started, "callLegId"))
    };
    let (invite, from) = phone.request("INVITE");
    assert!(invite.uri.to_string().contains(&user), "{}", invite.uri);
    assert_eq!(from, sip_at, "over the flow the phone registered on");
    let offer = sdp::parse(&String::from_utf8_lossy(&invite.body)).unwrap();
    let mut desk = phone.local_media();
    desk.codecs = vec!["g722", "pcmu"];
    let (answer, settled) = sdp::answer(&offer, &desk).unwrap();
    assert_eq!(settled.codec.name, "g722", "a wideband desk phone answers G.722");
    phone.reply(&invite, from, 180, "", "");
    phone.reply(&invite, from, 200, "", &answer);
    phone.request("ACK");

    // the call is answered on the phone; john's browser hears it, and it hears john's side of the call
    let cookie = crate::common::sign_in(http, "john.doe", "moqui");
    factory.flush_sessions().unwrap();
    let mut media = PhoneMedia::new(&settled).unwrap();
    let mut peer = Peer::new(open_socket(http, &cookie), &call_id, &caller_leg);
    peer.join();
    let got = talk(&phone, &mut media, &mut peer, Duration::from_secs(6), 20);
    assert!(
        peer.received >= 20,
        "the browser hears the desk phone: {} frames",
        peer.received
    );
    assert!(got >= 20, "the desk phone gets the call's audio: {got} packets");
    let state = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        call(
            "org.moqui.impl.CallServices.get#Call",
            &[("callSessionId", Value::string(&call_id))],
        )
    };
    assert_eq!(status_of(&state).as_deref(), Some("active"));
    // john hangs up: the phone gets a BYE
    peer.send(json!({"rtc": "leave", "id": 5}));
    let (bye, from) = phone.request("BYE");
    phone.reply(&bye, from, 200, "", "");
}

/// V5: a call to a number routed to an IVR menu is answered by the engine; the caller presses 1, which goes to a queue; the
/// queue offers the call to its logged-in agent at once, the agent answers, and the caller hears hold music until then.
#[test]
fn an_ivr_key_puts_the_caller_in_a_queue_and_its_agent_answers() {
    if !crate::common::own_process() {
        return;
    }
    let trunk = Trunk::new();
    let gate = crate::common::start_with_properties(
        "sip-ivr",
        None,
        &[
            ("rtc_media_port", "0"),
            ("sip_port", "0"),
            ("sip_rtp_ports", "42500-42599"),
            ("call_queue_tick_seconds", "1"),
        ],
    );
    let sip_at = gate.server.sip_address().expect("the SIP listener is up");
    let factory = std::sync::Arc::clone(gate.server.factory());
    let queue_id = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
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
        let queue_id = text(
            &call(
                "org.moqui.impl.CallQueueServices.store#CallQueue",
                &[("name", Value::string("Support")), ("announceSeconds", Value::I32(0))],
            ),
            "callQueueId",
        );
        call(
            "org.moqui.impl.CallQueueServices.store#CallQueueAgent",
            &[
                ("callQueueId", Value::string(&queue_id)),
                ("userId", Value::string(&john)),
            ],
        );
        call(
            "org.moqui.impl.CallQueueServices.set#QueueAgentStatus",
            &[
                ("callQueueId", Value::string(&queue_id)),
                ("status", Value::string("in")),
            ],
        );
        let menu_id = text(
            &call(
                "org.moqui.impl.IvrServices.store#IvrMenu",
                &[("name", Value::string("Main"))],
            ),
            "ivrMenuId",
        );
        call(
            "org.moqui.impl.IvrServices.store#IvrOption",
            &[
                ("ivrMenuId", Value::string(&menu_id)),
                ("digits", Value::string("1")),
                ("targetTypeEnumId", Value::string("CtgQueue")),
                ("targetId", Value::string(&queue_id)),
            ],
        );
        call(
            "org.moqui.impl.VoiceServices.store#PhoneNumber",
            &[
                ("phoneNumber", Value::string(NUMBER)),
                ("voiceTrunkId", Value::string(&trunk_id)),
                ("routeTypeEnumId", Value::string("PnrIvr")),
                ("routeId", Value::string(&menu_id)),
            ],
        );
        queue_id
    };
    let (register, from) = trunk.request("REGISTER");
    let contact = values(&register.headers, "Contact")[0].clone();
    trunk.reply(&register, from, 200, &format!("Contact: {contact};expires=300\r\n"), "");

    let started = Instant::now();
    let phone_local = trunk.local_media();
    let offer = sdp::offer(&phone_local);
    let invite = format!(
        "INVITE sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-ivr1\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=ivr1\r\nTo: <sip:{NUMBER}@{sip_at}>\r\nCall-ID: ivr-call-1\r\nCSeq: 1 INVITE\r\n\
         Contact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        trunk.addr(),
        trunk.addr(),
        offer.len()
    );
    trunk.send(&invite, sip_at);
    // the engine answers for the menu at once
    let ok = trunk.response(200);
    let settled = sdp::accept(&phone_local, &sdp::parse(&String::from_utf8_lossy(&ok.body)).unwrap()).unwrap();
    let to = ok.to_header().unwrap().value().to_string();
    let ack = format!(
        "ACK sip:{NUMBER}@{sip_at} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-ivrack\r\nMax-Forwards: 70\r\n\
         From: <sip:{OUTSIDE}@127.0.0.1>;tag=ivr1\r\nTo: {to}\r\nCall-ID: ivr-call-1\r\nCSeq: 1 ACK\r\nContent-Length: 0\r\n\r\n",
        trunk.addr()
    );
    trunk.send(&ack, sip_at);
    let mut phone = PhoneMedia::new(&settled).unwrap();

    // the caller hears the menu's tones and presses 1 during them; then the queue's music while john's tabs ring
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let mut buf = vec![0u8; 2048];
    let mut heard = 0;
    let mut tick = 0;
    let mut next = Instant::now();
    let key_at = Instant::now() + Duration::from_millis(300);
    let until = Instant::now() + Duration::from_secs(4);
    let mut pressed = false;
    let mut ringing = None;
    while Instant::now() < until && ringing.is_none() {
        if Instant::now() >= next {
            next += Duration::from_millis(20);
            if !pressed && Instant::now() >= key_at {
                phone.send_digits("1");
                pressed = true;
            }
            if let Some(packet) = phone.send(&tone(tick)) {
                trunk.rtp.send_to(&packet, phone.remote()).unwrap();
            }
            tick += 1;
            if pressed && tick % 10 == 0 {
                let active = call("org.moqui.impl.CallServices.find#ActiveCalls", &[]);
                if let Some(Value::List(list)) = active.get("callList") {
                    ringing = list.iter().find_map(|c| match c {
                        Value::Map(m) if m.get("myLegStatus").map(to_plain_string).as_deref() == Some("offered") => {
                            m.get("callSessionId").map(to_plain_string)
                        }
                        _ => None,
                    });
                }
            }
        }
        while let Ok((len, from)) = trunk.rtp.recv_from(&mut buf) {
            phone.receive(from, &buf[..len]);
            heard += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let call_id = ringing.expect("the queue offers the call to john, its agent");
    assert!(heard >= 10, "the caller hears the menu and the music: {heard} packets");
    let answered = call(
        "org.moqui.impl.CallServices.answer#Call",
        &[("callSessionId", Value::string(&call_id))],
    );
    assert_eq!(status_of(&answered).as_deref(), Some("active"));
    // the queue's entry is answered: nothing waits, and john, its agent, is in the call
    let mine = call("org.moqui.impl.CallQueueServices.find#MyCallQueues", &[]);
    let Some(Value::List(queues)) = mine.get("queueList") else {
        panic!("john's queues: {mine:?}")
    };
    let Some(Value::Map(support)) = queues.first() else {
        panic!("john's queue: {mine:?}")
    };
    assert_eq!(
        support.get("callQueueId").map(to_plain_string).as_deref(),
        Some(queue_id.as_str())
    );
    assert_eq!(support.get("waitingCount").map(to_plain_string).as_deref(), Some("0"));
    assert_eq!(support.get("state").map(to_plain_string).as_deref(), Some("inCall"));
    // john hangs up: the engine ends the caller's leg
    call(
        "org.moqui.impl.CallServices.leave#Call",
        &[("callSessionId", Value::string(&call_id))],
    );
    let (bye, from) = trunk.request("BYE");
    trunk.reply(&bye, from, 200, "", "");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
}
