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

//! The V1 spike's cases (plans/Comms-Framework.md §5.7) against an in-process fake trunk: a raw SIP peer on a loopback
//! UDP socket that plays the provider's part message by message (so it can fork, cross a CANCEL and challenge as a
//! test needs), and for TLS a second user agent as the far end. The media of a call runs between two
//! [`crate::media::PhoneMedia`] over real loopback sockets.

use std::net::SocketAddr;
use std::time::Duration;

use base64::Engine as _;
use rsipstack::sip::{Request, SipMessage};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

use super::*;
use crate::digest::{Algorithm, Authorization, NonceBook, Verdict};
use crate::media::{PhoneMedia, Received};
use crate::sdp::{self, Direction, LocalMedia};
use crate::srtp::MasterKey;

const WAIT: Duration = Duration::from_secs(3);
const TRUNK_REALM: &str = "fake.trunk";

/// The provider's side, message by message.
struct FakeTrunk {
    socket: UdpSocket,
    addr: SocketAddr,
    nonces: NonceBook,
}

impl FakeTrunk {
    async fn on(ip: &str) -> FakeTrunk {
        let socket = UdpSocket::bind(format!("{ip}:0")).await.unwrap();
        let addr = socket.local_addr().unwrap();
        FakeTrunk {
            socket,
            addr,
            nonces: NonceBook::new(TRUNK_REALM, 60),
        }
    }

    async fn recv(&self, wait: Duration) -> Option<(SipMessage, SocketAddr)> {
        let mut buf = vec![0u8; 8192];
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let (len, from) = tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf))
                .await
                .ok()?
                .ok()?;
            let Ok(text) = std::str::from_utf8(&buf[..len]) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            if let Ok(message) = SipMessage::try_from(text) {
                return Some((message, from));
            }
        }
    }

    /// The next request of `method`, skipping retransmissions and anything else.
    async fn request(&self, method: Method) -> (Request, SocketAddr) {
        self.try_request(method, WAIT)
            .await
            .unwrap_or_else(|| panic!("no {method}"))
    }

    async fn try_request(&self, method: Method, wait: Duration) -> Option<(Request, SocketAddr)> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.recv(left).await? {
                (SipMessage::Request(req), from) if req.method == method => return Some((req, from)),
                _ => continue,
            }
        }
    }

    async fn response(&self, code: u16) -> Response {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.recv(left).await {
                Some((SipMessage::Response(resp), _)) if u16::from(resp.status_code.clone()) == code => return resp,
                Some(_) => continue,
                None => panic!("no {code}"),
            }
        }
    }

    async fn send(&self, text: &str, to: SocketAddr) {
        self.socket.send_to(text.as_bytes(), to).await.unwrap();
    }

    /// A response to `req` with this To tag, extra header lines and body.
    async fn reply(&self, req: &Request, to: SocketAddr, code: u16, tag: &str, extra: &str, body: &str) {
        let vias: String = header_values(&req.headers, "Via")
            .iter()
            .map(|v| format!("Via: {v}\r\n"))
            .collect();
        let to_header = req.to_header().unwrap().value().to_string();
        let to_header = if to_header.contains(";tag=") || tag.is_empty() {
            to_header
        } else {
            format!("{to_header};tag={tag}")
        };
        let content = if body.is_empty() {
            String::new()
        } else {
            "Content-Type: application/sdp\r\n".to_string()
        };
        let text = format!(
            "SIP/2.0 {code} Reason\r\n{vias}From: {}\r\nTo: {to_header}\r\nCall-ID: {}\r\nCSeq: {}\r\n\
             Contact: <sip:trunk@{}>\r\n{extra}{content}Content-Length: {}\r\n\r\n{body}",
            req.from_header().unwrap().value(),
            req.call_id_header().unwrap().value(),
            req.cseq_header().unwrap().value(),
            self.addr,
            body.len()
        );
        self.send(&text, to).await;
    }

    fn challenge(&self, header: &str) -> String {
        let value = &self.nonces.challenges(now_secs(), &[Algorithm::Sha256], false)[0];
        format!("{header}: {value}\r\n")
    }

    fn check(&mut self, req: &Request, header: &str, password: &str) -> Verdict {
        let value = header_values(&req.headers, header).pop().expect("credentials");
        let auth = Authorization::parse(&value).unwrap();
        self.nonces
            .verify(&auth, &req.method.to_string(), Some(password), now_secs())
    }

    /// A new request to the UA at `ua`, as the provider sends an inbound call.
    async fn invite(&self, ua: SocketAddr, call_id: &str, to: &str, from: &str, body: &str) {
        let text = format!(
            "INVITE sip:{to}@{ua} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id}\r\nMax-Forwards: 70\r\n\
             From: \"Caller\" <sip:{from}@{}>;tag=trunk-{call_id}\r\nTo: <sip:{to}@{ua}>\r\nCall-ID: {call_id}\r\n\
             CSeq: 1 INVITE\r\nContact: <sip:trunk@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
            self.addr,
            self.addr,
            self.addr,
            body.len()
        );
        self.send(&text, ua).await;
    }
}

struct Harness {
    ua: Ua,
    events: mpsc::UnboundedReceiver<UaEvent>,
    addr: SocketAddr,
}

impl Harness {
    async fn start() -> Harness {
        let (tx, events) = mpsc::unbounded_channel();
        let ua = Ua::start(
            UaConfig {
                udp: Some("127.0.0.1:0".parse().unwrap()),
                realm: "moqui.test".to_string(),
                guard: GuardPolicy {
                    limit: 3,
                    window: Duration::from_secs(60),
                    block: Duration::from_secs(600),
                },
                ..Default::default()
            },
            tx,
        )
        .await
        .unwrap();
        let addr = ua.bound().udp.unwrap();
        Harness { ua, events, addr }
    }

    async fn event(&mut self) -> UaEvent {
        tokio::time::timeout(WAIT, self.events.recv())
            .await
            .expect("an event in time")
            .expect("events open")
    }

    /// The next event that is not a ringing or a trunk status.
    async fn main_event(&mut self) -> UaEvent {
        loop {
            match self.event().await {
                UaEvent::Ringing { .. } | UaEvent::TrunkStatus { .. } => continue,
                other => return other,
            }
        }
    }
}

fn trunk_config(trunk: &FakeTrunk, register: bool) -> TrunkConfig {
    TrunkConfig {
        id: "T1".to_string(),
        host: trunk.addr.ip().to_string(),
        port: Some(trunk.addr.port()),
        transport: Transport::Udp,
        credentials: Credentials {
            username: "acct".to_string(),
            password: "pw-of-acct".to_string(),
            realm: Some(TRUNK_REALM.to_string()),
            algorithms: vec![Algorithm::Sha256],
            ..Default::default()
        },
        domain: None,
        register,
        inbound: Vec::new(),
        srtp: false,
    }
}

async fn media_socket() -> (UdpSocket, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    (socket, addr)
}

fn tone(hz: f64, tick: usize) -> Vec<i16> {
    (0..moqui_rtc::audio::FRAME)
        .map(|i| {
            let t = (tick * moqui_rtc::audio::FRAME + i) as f64 / 48_000.0;
            (8000.0 * (2.0 * std::f64::consts::PI * hz * t).sin()) as i16
        })
        .collect()
}

async fn receive_all(socket: &UdpSocket, media: &mut PhoneMedia, digits: &mut String) {
    let mut buf = vec![0u8; 2048];
    while let Ok(Ok((len, from))) = tokio::time::timeout(Duration::from_millis(30), socket.recv_from(&mut buf)).await {
        if let Received::Digit(d) = media.receive(from, &buf[..len]) {
            digits.push(d);
        }
    }
}

/// 40 ticks of media both ways over the two sockets: what each side heard (frames above -30 dBov) and the digits.
async fn exchange(
    ours: (&UdpSocket, &mut PhoneMedia),
    theirs: (&UdpSocket, &mut PhoneMedia),
    send_digits: &str,
) -> (usize, usize, String) {
    let (our_socket, our_media) = ours;
    let (their_socket, their_media) = theirs;
    our_media.send_digits(send_digits);
    let mut digits = String::new();
    let mut ignore = String::new();
    let (mut we_heard, mut they_heard) = (0, 0);
    let mut pcm = Vec::new();
    for tick in 0..40 {
        if let Some(p) = our_media.send(&tone(440.0, tick)) {
            our_socket.send_to(&p, our_media.remote()).await.unwrap();
        }
        if let Some(p) = their_media.send(&tone(700.0, tick)) {
            their_socket.send_to(&p, their_media.remote()).await.unwrap();
        }
        receive_all(their_socket, their_media, &mut digits).await;
        receive_all(our_socket, our_media, &mut ignore).await;
        if our_media.pull(&mut pcm) && moqui_rtc::audio::level_dbov(&pcm) > -30.0 {
            we_heard += 1;
        }
        if their_media.pull(&mut pcm) && moqui_rtc::audio::level_dbov(&pcm) > -30.0 {
            they_heard += 1;
        }
    }
    (we_heard, they_heard, digits)
}

#[tokio::test]
async fn a_trunk_registers_with_its_realm_and_a_foreign_realm_is_refused() {
    let mut trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, true)]).await;
    let (first, from) = trunk.request(Method::Register).await;
    assert!(header_values(&first.headers, "Authorization").is_empty());
    assert!(header_values(&first.headers, "Contact")[0].contains(&format!("{}", h.addr.port())));
    trunk
        .reply(&first, from, 401, "", &trunk.challenge("WWW-Authenticate"), "")
        .await;
    let (second, from) = trunk.request(Method::Register).await;
    assert_eq!(trunk.check(&second, "Authorization", "pw-of-acct"), Verdict::Valid);
    let contact = header_values(&second.headers, "Contact")[0].clone();
    trunk
        .reply(
            &second,
            from,
            200,
            "",
            &format!("Contact: {contact};expires=60\r\n"),
            "",
        )
        .await;
    match h.event().await {
        UaEvent::TrunkStatus { trunk, state } => assert_eq!((trunk.as_str(), state), ("T1", TrunkState::Registered)),
        other => panic!("{other:?}"),
    }

    // a registrar that challenges for another realm gets no credentials
    let evil = FakeTrunk {
        nonces: NonceBook::new("evil.example", 60),
        ..FakeTrunk::on("127.0.0.1").await
    };
    let mut h2 = Harness::start().await;
    h2.ua.set_trunks(vec![trunk_config(&evil, true)]).await;
    let (first, from) = evil.request(Method::Register).await;
    evil.reply(&first, from, 401, "", &evil.challenge("WWW-Authenticate"), "")
        .await;
    match h2.event().await {
        UaEvent::TrunkStatus {
            state: TrunkState::Failed(why),
            ..
        } => assert!(why.contains("realm"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(evil
        .try_request(Method::Register, Duration::from_millis(300))
        .await
        .is_none());
    h.ua.shutdown();
    h2.ua.shutdown();
}

#[tokio::test]
async fn an_outbound_call_with_a_challenge_media_dtmf_and_hangup() {
    let mut trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, false)]).await;
    let (our_socket, our_addr) = media_socket().await;
    let local = LocalMedia::new(our_addr.ip(), our_addr.port(), None);
    h.ua.dial(DialRequest {
        leg: "L1".to_string(),
        trunk: "T1".to_string(),
        to: "+15551230000".to_string(),
        from: "+15559870000".to_string(),
        from_name: Some("Moqui".to_string()),
        offer: sdp::offer(&local),
        ring_timeout: Duration::from_secs(30),
        headers: Vec::new(),
    })
    .unwrap();
    let (invite, from) = trunk.request(Method::Invite).await;
    assert!(invite.uri.to_string().contains("+15551230000"));
    trunk
        .reply(&invite, from, 407, "t1", &trunk.challenge("Proxy-Authenticate"), "")
        .await;
    let (invite, from) = trunk.request(Method::Invite).await;
    assert_eq!(
        trunk.check(&invite, "Proxy-Authorization", "pw-of-acct"),
        Verdict::Valid
    );
    // the provider's media
    let (their_socket, their_addr) = media_socket().await;
    let offer = sdp::parse(&String::from_utf8_lossy(&invite.body)).unwrap();
    let (answer, their_side) = sdp::answer(&offer, &LocalMedia::new(their_addr.ip(), their_addr.port(), None)).unwrap();
    trunk.reply(&invite, from, 180, "t2", "", "").await;
    trunk.reply(&invite, from, 200, "t2", "", &answer).await;
    assert!(matches!(h.event().await, UaEvent::Ringing { leg } if leg == "L1"));
    let answered = match h.main_event().await {
        UaEvent::Answered { leg, answer } if leg == "L1" => answer,
        other => panic!("{other:?}"),
    };
    trunk.request(Method::Ack).await;
    let our_side = sdp::accept(&local, &sdp::parse(&answered).unwrap()).unwrap();
    let mut ours = PhoneMedia::new(&our_side).unwrap();
    let mut theirs = PhoneMedia::new(&their_side).unwrap();
    let (we_heard, they_heard, digits) = exchange((&our_socket, &mut ours), (&their_socket, &mut theirs), "5#").await;
    assert!(we_heard > 20 && they_heard > 10, "heard {we_heard} {they_heard}");
    assert_eq!(digits, "5#");

    h.ua.hangup("L1");
    let (bye, from) = trunk.request(Method::Bye).await;
    trunk.reply(&bye, from, 200, "", "", "").await;
    assert!(matches!(h.main_event().await, UaEvent::Ended { leg, reason: EndReason::Hangup } if leg == "L1"));
    h.ua.shutdown();
}

#[tokio::test]
async fn an_inbound_call_rings_is_answered_and_the_far_end_hangs_up() {
    let trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, false)]).await;
    let (their_socket, their_addr) = media_socket().await;
    let their_local = LocalMedia::new(their_addr.ip(), their_addr.port(), None);
    trunk
        .invite(
            h.addr,
            "in-1",
            "+15551230000",
            "+15557770000",
            &sdp::offer(&their_local),
        )
        .await;
    let (our_socket, our_addr) = media_socket().await;
    let our_side = match h.main_event().await {
        UaEvent::Incoming(call) => {
            assert_eq!((call.trunk_id.as_str(), call.to.as_str()), ("T1", "+15551230000"));
            assert_eq!(
                (call.from.as_str(), call.from_name.as_deref()),
                ("+15557770000", Some("Caller"))
            );
            let (answer, settled) = sdp::answer(
                &sdp::parse(&call.offer).unwrap(),
                &LocalMedia::new(our_addr.ip(), our_addr.port(), None),
            )
            .unwrap();
            call.reply
                .send(Decision::Ring {
                    leg: "L2".to_string(),
                    answer,
                })
                .unwrap();
            settled
        }
        other => panic!("{other:?}"),
    };
    trunk.response(180).await;
    h.ua.answer("L2").unwrap();
    let ok = trunk.response(200).await;
    let their_side = sdp::accept(&their_local, &sdp::parse(&String::from_utf8_lossy(&ok.body)).unwrap()).unwrap();
    let to = ok.to_header().unwrap().value().to_string();
    let ack = format!(
        "ACK sip:+15551230000@{} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-ack1\r\nMax-Forwards: 70\r\n\
         From: \"Caller\" <sip:+15557770000@{}>;tag=trunk-in-1\r\nTo: {to}\r\nCall-ID: in-1\r\nCSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n",
        h.addr, trunk.addr, trunk.addr
    );
    trunk.send(&ack, h.addr).await;
    let mut ours = PhoneMedia::new(&our_side).unwrap();
    let mut theirs = PhoneMedia::new(&their_side).unwrap();
    let (we_heard, they_heard, _) = exchange((&our_socket, &mut ours), (&their_socket, &mut theirs), "").await;
    assert!(we_heard > 20 && they_heard > 20, "heard {we_heard} {they_heard}");

    let bye = format!(
        "BYE sip:+15551230000@{} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-bye1\r\nMax-Forwards: 70\r\n\
         From: \"Caller\" <sip:+15557770000@{}>;tag=trunk-in-1\r\nTo: {to}\r\nCall-ID: in-1\r\nCSeq: 2 BYE\r\n\
         Content-Length: 0\r\n\r\n",
        h.addr, trunk.addr, trunk.addr
    );
    trunk.send(&bye, h.addr).await;
    trunk.response(200).await;
    assert!(matches!(h.main_event().await, UaEvent::Ended { leg, reason: EndReason::Bye } if leg == "L2"));
    h.ua.shutdown();
}

#[tokio::test]
async fn cancel_while_ringing_and_an_answer_that_crosses_the_cancel() {
    let trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, false)]).await;
    let local = LocalMedia::new("127.0.0.1".parse().unwrap(), 20_000, None);
    let dial = |leg: &str| DialRequest {
        leg: leg.to_string(),
        trunk: "T1".to_string(),
        to: "+15551230000".to_string(),
        from: "+15559870000".to_string(),
        from_name: None,
        offer: sdp::offer(&local),
        ring_timeout: Duration::from_secs(30),
        headers: Vec::new(),
    };
    // the caller gives up while it rings: CANCEL, 487, ACK
    h.ua.dial(dial("L3")).unwrap();
    let (invite, from) = trunk.request(Method::Invite).await;
    trunk.reply(&invite, from, 180, "c1", "", "").await;
    assert!(matches!(h.event().await, UaEvent::Ringing { .. }));
    h.ua.hangup("L3");
    let (cancel, cfrom) = trunk.request(Method::Cancel).await;
    trunk.reply(&cancel, cfrom, 200, "c1", "", "").await;
    trunk.reply(&invite, from, 487, "c1", "", "").await;
    trunk.request(Method::Ack).await;
    assert!(matches!(h.main_event().await, UaEvent::Ended { leg, reason: EndReason::Canceled } if leg == "L3"));

    // the callee picks up as the CANCEL is on its way: the 2xx is ACKed and ended with BYE, and the leg is canceled
    h.ua.dial(dial("L4")).unwrap();
    let (invite, from) = trunk.request(Method::Invite).await;
    trunk.reply(&invite, from, 180, "c2", "", "").await;
    assert!(matches!(h.event().await, UaEvent::Ringing { .. }));
    h.ua.hangup("L4");
    let (cancel, cfrom) = trunk.request(Method::Cancel).await;
    let answer = sdp::offer(&LocalMedia::new("127.0.0.1".parse().unwrap(), 40_000, None));
    trunk.reply(&invite, from, 200, "c2", "", &answer).await;
    trunk.reply(&cancel, cfrom, 200, "c2", "", "").await;
    trunk.request(Method::Ack).await;
    let (bye, bfrom) = trunk.request(Method::Bye).await;
    trunk.reply(&bye, bfrom, 200, "", "", "").await;
    assert!(matches!(h.main_event().await, UaEvent::Ended { leg, reason: EndReason::Canceled } if leg == "L4"));
    h.ua.shutdown();
}

#[tokio::test]
async fn the_first_of_two_forked_answers_wins_and_the_other_is_ended() {
    let trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, false)]).await;
    let local = LocalMedia::new("127.0.0.1".parse().unwrap(), 20_002, None);
    h.ua.dial(DialRequest {
        leg: "L5".to_string(),
        trunk: "T1".to_string(),
        to: "+15551230000".to_string(),
        from: "+15559870000".to_string(),
        from_name: None,
        offer: sdp::offer(&local),
        ring_timeout: Duration::from_secs(30),
        headers: Vec::new(),
    })
    .unwrap();
    let (invite, from) = trunk.request(Method::Invite).await;
    let answer_a = sdp::offer(&LocalMedia::new("127.0.0.1".parse().unwrap(), 40_002, None));
    let answer_b = sdp::offer(&LocalMedia::new("127.0.0.1".parse().unwrap(), 40_004, None));
    trunk.reply(&invite, from, 200, "phone-a", "", &answer_a).await;
    trunk.reply(&invite, from, 200, "phone-b", "", &answer_b).await;
    match h.main_event().await {
        UaEvent::Answered { leg, answer } => {
            assert_eq!(leg, "L5");
            assert!(answer.contains("40002"), "phone A's answer");
        }
        other => panic!("{other:?}"),
    }
    // phone B's answer is ended
    let (bye, bfrom) = trunk.request(Method::Bye).await;
    assert!(
        bye.to_header().unwrap().value().contains("phone-b"),
        "{}",
        bye.to_header().unwrap().value()
    );
    trunk.reply(&bye, bfrom, 200, "", "", "").await;
    // and no second answer is reported
    let more = tokio::time::timeout(Duration::from_millis(300), h.events.recv()).await;
    assert!(!matches!(more, Ok(Some(UaEvent::Answered { .. }))));
    h.ua.shutdown();
}

#[tokio::test]
async fn hold_by_reinvite_both_ways() {
    let trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, false)]).await;
    let local = LocalMedia::new("127.0.0.1".parse().unwrap(), 20_004, None);
    h.ua.dial(DialRequest {
        leg: "L6".to_string(),
        trunk: "T1".to_string(),
        to: "+15551230000".to_string(),
        from: "+15559870000".to_string(),
        from_name: None,
        offer: sdp::offer(&local),
        ring_timeout: Duration::from_secs(30),
        headers: Vec::new(),
    })
    .unwrap();
    let (invite, from) = trunk.request(Method::Invite).await;
    let mut their_local = LocalMedia::new("127.0.0.1".parse().unwrap(), 40_006, None);
    let (answer, _) = sdp::answer(
        &sdp::parse(&String::from_utf8_lossy(&invite.body)).unwrap(),
        &their_local,
    )
    .unwrap();
    trunk.reply(&invite, from, 200, "h1", "", &answer).await;
    assert!(matches!(h.main_event().await, UaEvent::Answered { .. }));
    trunk.request(Method::Ack).await;

    // the provider holds: a re-INVITE with sendonly, answered recvonly
    their_local.direction = Direction::SendOnly;
    their_local.version = 2;
    let hold = sdp::offer(&their_local);
    let contact = header_values(&invite.headers, "Contact")[0].clone();
    let contact = contact.trim_start_matches('<').trim_end_matches('>').to_string();
    let from_header = invite.from_header().unwrap().value().to_string();
    let to_header = format!("{};tag=h1", invite.to_header().unwrap().value());
    let call_id = invite.call_id_header().unwrap().value().to_string();
    let reinvite = format!(
        "INVITE {contact} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-hold1\r\nMax-Forwards: 70\r\nFrom: {to_header}\r\n\
         To: {from_header}\r\nCall-ID: {call_id}\r\nCSeq: 10 INVITE\r\nContact: <sip:trunk@{}>\r\n\
         Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{hold}",
        trunk.addr,
        trunk.addr,
        hold.len()
    );
    trunk.send(&reinvite, h.addr).await;
    match h.main_event().await {
        UaEvent::Reoffer { leg, offer, reply } => {
            assert_eq!(leg, "L6");
            let (answer, settled) = sdp::answer(&sdp::parse(&offer).unwrap(), &local).unwrap();
            assert_eq!(settled.direction, Direction::RecvOnly);
            reply.send(Some(answer)).unwrap();
        }
        other => panic!("{other:?}"),
    }
    let ok = trunk.response(200).await;
    assert!(String::from_utf8_lossy(&ok.body).contains("a=recvonly"));

    // this side holds
    let mut ours = local.clone();
    ours.direction = Direction::SendOnly;
    ours.version = 2;
    let ua = h.ua.clone();
    let hold = tokio::spawn(async move { ua.reinvite("L6", sdp::offer(&ours)).await });
    let (reinvite, rfrom) = trunk.request(Method::Invite).await;
    assert!(String::from_utf8_lossy(&reinvite.body).contains("a=sendonly"));
    let (answer, _) = sdp::answer(
        &sdp::parse(&String::from_utf8_lossy(&reinvite.body)).unwrap(),
        &their_local,
    )
    .unwrap();
    trunk.reply(&reinvite, rfrom, 200, "", "", &answer).await;
    let answered = hold.await.unwrap().unwrap().unwrap();
    assert!(
        answered.contains("a=inactive") || answered.contains("a=recvonly"),
        "{answered}"
    );
    h.ua.shutdown();
}

#[tokio::test]
async fn failed_digests_block_the_source_and_never_the_trunk() {
    let trunk = FakeTrunk::on("127.0.0.1").await;
    let mut h = Harness::start().await;
    h.ua.set_trunks(vec![trunk_config(&trunk, false)]).await;
    let scanner = FakeTrunk::on("127.0.0.2").await;
    let offer = sdp::offer(&LocalMedia::new("127.0.0.2".parse().unwrap(), 4_000, None));
    // no credentials: a challenge
    scanner.invite(h.addr, "scan-0", "+15551230000", "100", &offer).await;
    let challenge = scanner.response(407).await;
    let challenge = header_values(&challenge.headers, "Proxy-Authenticate");
    assert_eq!(challenge.len(), 2, "SHA-256 and MD5: {challenge:?}");
    let parsed = Challenge::parse(&challenge[0]).unwrap();
    assert_eq!(
        (parsed.realm.as_str(), parsed.algorithm),
        ("moqui.test", Some(Algorithm::Sha256))
    );
    // wrong passwords for a real username: 403, 403, then blocked and unanswered
    let guess = |n: usize, password: &str| {
        let credentials = Credentials {
            username: "acct".to_string(),
            password: password.to_string(),
            ..Default::default()
        };
        let auth = digest::answer(
            &credentials,
            &parsed,
            "INVITE",
            "sip:+15551230000@moqui",
            &format!("c{n}"),
        )
        .unwrap();
        format!("Proxy-Authorization: {auth}\r\n")
    };
    for n in 1..=3 {
        let call_id = format!("scan-{n}");
        let text = format!(
            "INVITE sip:+15551230000@{} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id}\r\nMax-Forwards: 70\r\n\
             From: <sip:100@{}>;tag={call_id}\r\nTo: <sip:+15551230000@{}>\r\nCall-ID: {call_id}\r\nCSeq: 1 INVITE\r\n\
             {}Content-Length: 0\r\n\r\n",
            h.addr,
            scanner.addr,
            scanner.addr,
            h.addr,
            guess(n, "letmein")
        );
        scanner.send(&text, h.addr).await;
        match h.main_event().await {
            UaEvent::AuthFailure {
                source,
                username,
                blocked,
            } => {
                assert_eq!(
                    (source, username.as_str(), blocked),
                    (scanner.addr.ip(), "acct", n == 3)
                );
            }
            other => panic!("{other:?}"),
        }
        if n < 3 {
            scanner.response(403).await;
        }
    }
    // blocked: even a fresh request gets no answer, and the account is untouched
    scanner.invite(h.addr, "scan-9", "+15551230000", "100", &offer).await;
    assert!(
        scanner.recv(Duration::from_millis(400)).await.is_none(),
        "dropped unanswered"
    );
    // the trunk itself still calls in
    trunk
        .invite(h.addr, "in-ok", "+15551230000", "+15557770000", &offer)
        .await;
    assert!(matches!(h.main_event().await, UaEvent::Incoming(call) if call.trunk_id == "T1"));
    // and the right credentials from another address are taken
    let other = FakeTrunk::on("127.0.0.3").await;
    other.invite(h.addr, "pbx-0", "+15551230000", "200", &offer).await;
    let challenge = header_values(&other.response(407).await.headers, "Proxy-Authenticate");
    let parsed = Challenge::parse(&challenge[0]).unwrap();
    let credentials = Credentials {
        username: "acct".to_string(),
        password: "pw-of-acct".to_string(),
        ..Default::default()
    };
    let uri = format!("sip:+15551230000@{}", h.addr);
    let auth = digest::answer(&credentials, &parsed, "INVITE", &uri, "cx").unwrap();
    let text = format!(
        "INVITE {uri} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-pbx-1\r\nMax-Forwards: 70\r\n\
         From: <sip:200@{}>;tag=pbx-1\r\nTo: <{uri}>\r\nCall-ID: pbx-1\r\nCSeq: 1 INVITE\r\nProxy-Authorization: {auth}\r\n\
         Contact: <sip:200@{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{offer}",
        other.addr,
        other.addr,
        other.addr,
        offer.len()
    );
    other.send(&text, h.addr).await;
    assert!(matches!(h.main_event().await, UaEvent::Incoming(call) if call.trunk_id == "T1" && call.from == "200"));
    h.ua.shutdown();
}

fn pem(label: &str, der: &[u8]) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out.into_bytes()
}

#[tokio::test]
async fn a_call_over_tls_with_srtp_between_two_user_agents() {
    let issued = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".to_string()]).unwrap();
    let cert = pem("CERTIFICATE", issued.cert.der());
    let key = pem("PRIVATE KEY", &issued.signing_key.serialize_der());
    let (far_tx, mut far_events) = mpsc::unbounded_channel();
    let far = Ua::start(
        UaConfig {
            tls: Some(("127.0.0.1:0".parse().unwrap(), cert.clone(), key)),
            realm: "far.test".to_string(),
            ..Default::default()
        },
        far_tx,
    )
    .await
    .unwrap();
    let far_tls = far.bound().tls.unwrap();
    let (near_tx, mut near_events) = mpsc::unbounded_channel();
    let near = Ua::start(
        UaConfig {
            tcp: Some("127.0.0.1:0".parse().unwrap()),
            ca_pem: Some(cert),
            realm: "near.test".to_string(),
            ..Default::default()
        },
        near_tx,
    )
    .await
    .unwrap();
    // each is the other's trunk, by address
    let peer = |id: &str, port: u16, transport| TrunkConfig {
        id: id.to_string(),
        host: "127.0.0.1".to_string(),
        port: Some(port),
        transport,
        credentials: Credentials::default(),
        domain: None,
        register: false,
        inbound: Vec::new(),
        srtp: true,
    };
    near.set_trunks(vec![peer("FAR", far_tls.port(), Transport::Tls)]).await;
    far.set_trunks(vec![peer("NEAR", 1, Transport::Tls)]).await;
    let (near_socket, near_addr) = media_socket().await;
    let near_local = LocalMedia::new(near_addr.ip(), near_addr.port(), Some(MasterKey::random()));
    near.dial(DialRequest {
        leg: "S1".to_string(),
        trunk: "FAR".to_string(),
        to: "+15551230000".to_string(),
        from: "+15559870000".to_string(),
        from_name: None,
        offer: sdp::offer(&near_local),
        ring_timeout: Duration::from_secs(30),
        headers: Vec::new(),
    })
    .unwrap();
    let (far_socket, far_addr) = media_socket().await;
    let far_side = match tokio::time::timeout(WAIT, far_events.recv()).await.unwrap().unwrap() {
        UaEvent::Incoming(call) => {
            assert!(call.offer.contains("RTP/SAVP") && call.offer.contains("a=crypto"));
            let (answer, settled) = sdp::answer(
                &sdp::parse(&call.offer).unwrap(),
                &LocalMedia::new(far_addr.ip(), far_addr.port(), Some(MasterKey::random())),
            )
            .unwrap();
            call.reply
                .send(Decision::Ring {
                    leg: "F1".to_string(),
                    answer,
                })
                .unwrap();
            settled
        }
        other => panic!("{other:?}"),
    };
    far.answer("F1").unwrap();
    let answer = loop {
        match tokio::time::timeout(WAIT, near_events.recv()).await.unwrap().unwrap() {
            UaEvent::Answered { answer, .. } => break answer,
            UaEvent::Ringing { .. } => continue,
            other => panic!("{other:?}"),
        }
    };
    let near_side = sdp::accept(&near_local, &sdp::parse(&answer).unwrap()).unwrap();
    assert!(near_side.remote_key.is_some());
    let mut near_media = PhoneMedia::new(&near_side).unwrap();
    let mut far_media = PhoneMedia::new(&far_side).unwrap();
    let (near_heard, far_heard, digits) =
        exchange((&near_socket, &mut near_media), (&far_socket, &mut far_media), "9").await;
    assert!(near_heard > 20 && far_heard > 10, "heard {near_heard} {far_heard}");
    assert_eq!(digits, "9");
    assert_eq!(far_media.stats().rejected, 0);

    near.hangup("S1");
    loop {
        match tokio::time::timeout(WAIT, far_events.recv()).await.unwrap().unwrap() {
            UaEvent::Ended { leg, reason } => {
                assert_eq!((leg.as_str(), reason), ("F1", EndReason::Bye));
                break;
            }
            _ => continue,
        }
    }
    near.shutdown();
    far.shutdown();
}
