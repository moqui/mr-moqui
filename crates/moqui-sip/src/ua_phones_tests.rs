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

//! The registrar's cases (plans/Comms-Framework.md §7.1 V4) against in-memory phones: raw SIP peers on loopback UDP
//! sockets and a TCP connection that play a desk or soft phone message by message, so a test can register, unregister,
//! replay, answer, ring or keep a flow alive as it needs. The bindings' own rules (expiry limits, ordering, Outbound
//! keys) are `registrar_tests.rs`, on simulated time.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use rsipstack::sip::{Method, Request, Response, SipMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;

use super::*;
use crate::digest::{self, Algorithm, Challenge, Credentials};
use crate::ua::{header_values, Decision, EndReason, UaConfig};

const WAIT: Duration = Duration::from_secs(3);
const REALM: &str = "moqui.test";
const PASSWORD: &str = "Rk7v-desk-phone-secret";

/// The devices the voice services would answer for.
#[derive(Default)]
struct Directory {
    devices: Mutex<HashMap<String, PhoneDevice>>,
    /// How many times it was asked: a service call each in the host.
    lookups: std::sync::atomic::AtomicUsize,
    /// Whoever checks the answers cannot be asked (an edge node's local node away).
    away: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl DeviceDirectory for Directory {
    async fn device(&self, realm: &str, username: &str) -> Option<PhoneDevice> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        lock(&self.devices)
            .get(&aor_key(username, (realm != REALM).then_some(realm)))
            .cloned()
    }

    async fn verify(
        &self,
        realm: &str,
        username: &str,
        check: &digest::ResponseCheck,
    ) -> Result<Option<(PhoneDevice, bool)>, String> {
        if self.away.load(Ordering::Relaxed) {
            return Err("the local node is away".to_string());
        }
        let Some(device) = self.device(realm, username).await else {
            return Ok(None);
        };
        let verifier = device
            .verifiers
            .iter()
            .find(|(a, _)| *a == check.algorithm)
            .map(|(_, h)| h.clone());
        let matched = check.matches(verifier.as_deref());
        Ok(Some((device, matched)))
    }
}

impl Directory {
    /// A device with verifiers of `password` for these algorithms.
    fn put(&self, id: &str, username: &str, revision: u32, password: &str, algorithms: &[Algorithm]) {
        self.put_in(REALM, id, username, revision, password, algorithms);
    }

    /// A device of the realm `realm` (a device domain besides the registrar's own, E12).
    fn put_in(&self, realm: &str, id: &str, username: &str, revision: u32, password: &str, algorithms: &[Algorithm]) {
        let verifiers = algorithms
            .iter()
            .map(|a| (*a, digest::ha1(*a, username, realm, password)))
            .collect();
        lock(&self.devices).insert(
            aor_key(username, (realm != REALM).then_some(realm)),
            PhoneDevice {
                id: id.to_string(),
                username: username.to_string(),
                revision,
                verifiers,
            },
        );
    }
}

struct Harness {
    ua: Ua,
    events: mpsc::UnboundedReceiver<UaEvent>,
    udp: SocketAddr,
    tcp: SocketAddr,
    directory: Arc<Directory>,
}

impl Harness {
    async fn start(limits: RegistrarLimits) -> Harness {
        let (tx, events) = mpsc::unbounded_channel();
        let directory = Arc::new(Directory::default());
        directory.put("D1", "p100", 1, PASSWORD, &[Algorithm::Sha256]);
        let ua = Ua::start(
            UaConfig {
                udp: Some("127.0.0.1:0".parse().unwrap()),
                tcp: Some("127.0.0.1:0".parse().unwrap()),
                realm: REALM.to_string(),
                guard: crate::guard::GuardPolicy {
                    limit: 3,
                    window: Duration::from_secs(60),
                    block: Duration::from_secs(600),
                },
                registrar: Some(RegistrarConfig {
                    limits,
                    directory: Arc::clone(&directory) as Arc<dyn DeviceDirectory>,
                }),
                ..Default::default()
            },
            tx,
        )
        .await
        .unwrap();
        let bound = ua.bound();
        Harness {
            ua,
            events,
            udp: bound.udp.unwrap(),
            tcp: bound.tcp.unwrap(),
            directory,
        }
    }

    /// The next event that is not a ringing, skipping others until `pick` takes one.
    async fn event<T>(&mut self, mut pick: impl FnMut(UaEvent) -> Option<T>) -> T {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let event = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .expect("the event in time")
                .expect("events open");
            if let Some(found) = pick(event) {
                return found;
            }
        }
    }

    async fn binding(&mut self, want: BindingChange) -> Binding {
        self.event(|e| match e {
            UaEvent::Binding { change, binding } if change == want => Some(binding),
            _ => None,
        })
        .await
    }

    fn ring(&self, leg: &str) -> Result<usize, String> {
        self.ua.ring_device(DeviceRing {
            leg: leg.to_string(),
            username: "p100".to_string(),
            from: "+15551230000".to_string(),
            from_name: Some("Front desk".to_string()),
            offer_plain: offer(),
            offer_srtp: None,
            ring_timeout: Duration::from_secs(10),
        })
    }
}

fn offer() -> String {
    let mut local = crate::sdp::LocalMedia::new("127.0.0.1".parse().unwrap(), 20_000, None);
    local.codecs = crate::sdp::DEVICE_CODECS.to_vec();
    crate::sdp::offer(&local)
}

fn answer_sdp(port: u16) -> String {
    format!(
        "v=0\r\no=phone 1 1 IN IP4 192.168.1.20\r\ns=-\r\nc=IN IP4 192.168.1.20\r\nt=0 0\r\n\
         m=audio {port} RTP/AVP 9 101\r\na=rtpmap:9 G722/8000\r\na=rtpmap:101 telephone-event/8000\r\na=sendrecv\r\n"
    )
}

/// What a REGISTER asks.
struct Reg<'a> {
    contact: &'a str,
    expires: Option<u32>,
    algorithm: Algorithm,
    password: &'a str,
    call_id: &'a str,
    cseq: u32,
    /// The Via's sent-by: the phone's private address, behind a NAT.
    sent_by: Option<&'a str>,
    extra: &'a str,
    /// The domain of From and To: the registrar's realm, or a device domain.
    domain: &'a str,
}

impl Default for Reg<'_> {
    fn default() -> Self {
        Reg {
            contact: "<sip:p100@192.168.1.20:5060>",
            expires: Some(300),
            algorithm: Algorithm::Sha256,
            password: PASSWORD,
            call_id: "reg-1",
            cseq: 1,
            sent_by: None,
            extra: "",
            domain: REALM,
        }
    }
}

fn register_text(r: &Reg, transport: &str, sent_by: &str, ua: SocketAddr, auth: Option<&str>) -> String {
    let expires = r.expires.map(|e| format!("Expires: {e}\r\n")).unwrap_or_default();
    let auth = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    format!(
        "REGISTER sip:{ua} SIP/2.0\r\nVia: SIP/2.0/{transport} {sent_by};branch=z9hG4bK-{}-{};rport\r\n\
         Max-Forwards: 70\r\nFrom: <sip:p100@{domain}>;tag=ph-{}\r\nTo: <sip:p100@{domain}>\r\nCall-ID: {}\r\n\
         CSeq: {} REGISTER\r\nContact: {}\r\n{expires}{auth}{}User-Agent: TestPhone/1.0\r\nContent-Length: 0\r\n\r\n",
        r.call_id,
        r.cseq,
        r.call_id,
        r.call_id,
        r.cseq,
        r.contact,
        r.extra,
        domain = r.domain
    )
}

/// The digest answer to a 401 for `r`'s algorithm and password.
fn authorization(resp: &Response, r: &Reg, ua: SocketAddr) -> String {
    let challenges: Vec<Challenge> = header_values(&resp.headers, "WWW-Authenticate")
        .iter()
        .filter_map(|v| Challenge::parse(v))
        .collect();
    assert!(
        challenges.first().and_then(|c| c.algorithm) == Some(Algorithm::Sha256),
        "SHA-256 is offered first"
    );
    let credentials = Credentials {
        username: "p100".to_string(),
        password: r.password.to_string(),
        algorithms: vec![r.algorithm],
        ..Default::default()
    };
    let challenge = digest::choose(&challenges, &credentials).expect("a challenge of the algorithm");
    digest::answer(&credentials, challenge, "REGISTER", &format!("sip:{ua}"), "cn-1").unwrap()
}

/// A phone on a loopback UDP socket.
struct UdpPhone {
    socket: UdpSocket,
    addr: SocketAddr,
}

impl UdpPhone {
    async fn new() -> UdpPhone {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        UdpPhone { socket, addr }
    }

    async fn send(&self, text: &str, to: SocketAddr) {
        self.socket.send_to(text.as_bytes(), to).await.unwrap();
    }

    async fn recv(&self, wait: Duration) -> Option<SipMessage> {
        let mut buf = vec![0u8; 8192];
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let (len, _) = tokio::time::timeout_at(deadline, self.socket.recv_from(&mut buf))
                .await
                .ok()?
                .ok()?;
            let text = String::from_utf8_lossy(&buf[..len]).to_string();
            if text.trim().is_empty() {
                continue;
            }
            if let Ok(message) = SipMessage::try_from(text.as_str()) {
                return Some(message);
            }
        }
    }

    async fn response(&self) -> Response {
        loop {
            match self.recv(WAIT).await.expect("a response") {
                SipMessage::Response(resp) if u16::from(resp.status_code.clone()) >= 200 => return resp,
                _ => continue,
            }
        }
    }

    async fn request(&self, method: Method, wait: Duration) -> Option<Request> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.recv(left).await? {
                SipMessage::Request(req) if req.method == method => return Some(req),
                _ => continue,
            }
        }
    }

    /// A REGISTER with its challenge answered; the final answer, and the authorized text (to replay).
    async fn register(&self, ua: SocketAddr, r: &Reg<'_>) -> (Response, String) {
        let sent_by = r.sent_by.map(str::to_string).unwrap_or_else(|| self.addr.to_string());
        self.send(&register_text(r, "UDP", &sent_by, ua, None), ua).await;
        let challenge = self.response().await;
        assert_eq!(u16::from(challenge.status_code.clone()), 401);
        let auth = authorization(&challenge, r, ua);
        let signed = register_text(&Reg { cseq: r.cseq + 1, ..*r }, "UDP", &sent_by, ua, Some(&auth));
        self.send(&signed, ua).await;
        (self.response().await, signed)
    }

    /// Answers an INVITE as a phone does: 180, then 200 with `body` (or `code` without one).
    async fn reply(&self, req: &Request, to: SocketAddr, code: u16, tag: &str, body: &str) {
        self.send(&response_text(req, code, tag, body, self.addr), to).await;
    }
}

fn response_text(req: &Request, code: u16, tag: &str, body: &str, contact: SocketAddr) -> String {
    let vias: String = header_values(&req.headers, "Via")
        .iter()
        .map(|v| format!("Via: {v}\r\n"))
        .collect();
    let to = req.to_header().unwrap().value().to_string();
    let to = if to.contains(";tag=") || tag.is_empty() {
        to
    } else {
        format!("{to};tag={tag}")
    };
    let content = if body.is_empty() {
        String::new()
    } else {
        "Content-Type: application/sdp\r\n".to_string()
    };
    format!(
        "SIP/2.0 {code} Reason\r\n{vias}From: {}\r\nTo: {to}\r\nCall-ID: {}\r\nCSeq: {}\r\n\
         Contact: <sip:p100@{contact}>\r\n{content}Content-Length: {}\r\n\r\n{body}",
        req.from_header().unwrap().value(),
        req.call_id_header().unwrap().value(),
        req.cseq_header().unwrap().value(),
        body.len()
    )
}

fn code(resp: &Response) -> u16 {
    u16::from(resp.status_code.clone())
}

/// An all-in-one node's registrar (plans/Edge-Node.md §10, E12): a phone of the paired node's domain is challenged in
/// that realm and checked by the directory for it, and its bindings are kept apart from the same username's in the
/// registrar's own realm.
#[tokio::test]
async fn a_device_domain_is_challenged_in_its_realm_and_kept_apart() {
    const PAIRED: &str = "sip.paired.example";
    let mut h = Harness::start(RegistrarLimits::default()).await;
    h.ua.set_device_realms(&[PAIRED.to_string(), REALM.to_string()]);
    h.directory
        .put_in(PAIRED, "P1", "p100", 1, "another password", &[Algorithm::Sha256]);
    let phone = UdpPhone::new().await;
    let paired = Reg {
        domain: PAIRED,
        password: "another password",
        call_id: "reg-paired",
        ..Default::default()
    };
    let (ok, _) = phone.register(h.udp, &paired).await;
    assert_eq!(code(&ok), 200);
    let added = h.binding(BindingChange::Added).await;
    assert_eq!(added.device, "P1");
    assert_eq!(h.ua.bindings(&aor_key("p100", Some(PAIRED))).len(), 1);
    assert!(
        h.ua.bindings("p100").is_empty(),
        "the registrar's own realm has none of p100's yet"
    );
    // the own realm's p100, with its own password
    let own = UdpPhone::new().await;
    let (ok, _) = own.register(h.udp, &Reg::default()).await;
    assert_eq!(code(&ok), 200);
    assert_eq!(h.binding(BindingChange::Added).await.device, "D1");
    assert_eq!(h.ua.bindings("p100").len(), 1);
    // the paired domain's password does not open the own realm
    let wrong = UdpPhone::new().await;
    let (refused, _) = wrong
        .register(
            h.udp,
            &Reg {
                password: "another password",
                call_id: "reg-wrong",
                ..Default::default()
            },
        )
        .await;
    assert_eq!(code(&refused), 403);
    // a domain no longer served: its phones are challenged in the registrar's own realm
    h.ua.set_device_realms(&[]);
    let gone = UdpPhone::new().await;
    let (refused, _) = gone
        .register(
            h.udp,
            &Reg {
                call_id: "reg-gone",
                ..paired
            },
        )
        .await;
    assert_eq!(code(&refused), 403);
    h.ua.shutdown();
}

#[tokio::test]
async fn a_phone_registers_with_sha_256_and_md5_is_refused_for_a_standard_device() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let phone = UdpPhone::new().await;
    let (ok, _) = phone.register(h.udp, &Reg::default()).await;
    assert_eq!(code(&ok), 200);
    let contacts = header_values(&ok.headers, "Contact");
    assert_eq!(contacts.len(), 1);
    assert!(contacts[0].contains("expires=300"), "{contacts:?}");
    let added = h.binding(BindingChange::Added).await;
    assert_eq!((added.device.as_str(), added.flow.source), ("D1", phone.addr));
    assert_eq!(added.user_agent.as_deref(), Some("TestPhone/1.0"));
    // the same password with MD5: a standard device has no MD5 verifier, so the answer fails and counts
    let md5 = UdpPhone::new().await;
    let (refused, _) = md5
        .register(
            h.udp,
            &Reg {
                algorithm: Algorithm::Md5,
                call_id: "reg-md5",
                ..Default::default()
            },
        )
        .await;
    assert_eq!(code(&refused), 403);
    let username = h
        .event(|e| match e {
            UaEvent::AuthFailure { username, .. } => Some(username),
            _ => None,
        })
        .await;
    assert_eq!(username, "p100");
    assert_eq!(h.ua.bindings("p100").len(), 1);
    // a legacy device has an MD5 verifier too
    h.directory
        .put("D1", "p100", 1, PASSWORD, &[Algorithm::Sha256, Algorithm::Md5]);
    let (legacy, _) = md5
        .register(
            h.udp,
            &Reg {
                algorithm: Algorithm::Md5,
                contact: "<sip:p100@192.168.1.21:5060>",
                call_id: "reg-md5b",
                ..Default::default()
            },
        )
        .await;
    assert_eq!(code(&legacy), 200);
    h.ua.shutdown();
}

/// An edge node's registrar while its local node is away (plans/Edge-Node.md §5.4 "Voice (SIP)", E8): the answer
/// cannot be checked, so the REGISTER is answered 503 and is no failed attempt; past the guard's limit the phone still
/// registers once it can be checked again.
#[tokio::test]
async fn an_answer_that_cannot_be_checked_is_503_and_no_failed_attempt() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let phone = UdpPhone::new().await;
    h.directory.away.store(true, Ordering::Relaxed);
    for attempt in 0..4u32 {
        let call_id = format!("away-{attempt}");
        let (refused, _) = phone
            .register(
                h.udp,
                &Reg {
                    call_id: &call_id,
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(code(&refused), 503);
    }
    h.directory.away.store(false, Ordering::Relaxed);
    let (ok, _) = phone.register(h.udp, &Reg::default()).await;
    assert_eq!(code(&ok), 200, "not blocked: none of the four counted");
    h.binding(BindingChange::Added).await;
}

#[tokio::test]
async fn a_replayed_register_is_refused() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let phone = UdpPhone::new().await;
    let (ok, signed) = phone.register(h.udp, &Reg::default()).await;
    assert_eq!(code(&ok), 200);
    h.binding(BindingChange::Added).await;
    // the same nonce and nonce count again (a captured request sent back): a replay, refused and counted
    let replayed = signed
        .replace("CSeq: 2 REGISTER", "CSeq: 3 REGISTER")
        .replace("reg-1-2;", "reg-1-3;");
    phone.send(&replayed, h.udp).await;
    assert_eq!(code(&phone.response().await), 403);
    h.event(|e| matches!(e, UaEvent::AuthFailure { .. }).then_some(()))
        .await;
}

#[tokio::test]
async fn bindings_unregister_one_by_one_or_all_and_only_live_ones_ring() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let (desk, soft) = (UdpPhone::new().await, UdpPhone::new().await);
    let desk_contact = format!("<sip:p100@{}>", desk.addr);
    let soft_contact = format!("<sip:p100@{}>", soft.addr);
    fn reg<'a>(contact: &'a str, call_id: &'a str) -> Reg<'a> {
        Reg {
            contact,
            call_id,
            ..Default::default()
        }
    }
    assert_eq!(code(&desk.register(h.udp, &reg(&desk_contact, "desk")).await.0), 200);
    let (two, _) = soft.register(h.udp, &reg(&soft_contact, "soft")).await;
    assert_eq!(
        header_values(&two.headers, "Contact").len(),
        2,
        "a device keeps several bindings"
    );
    assert_eq!(h.ua.bindings("p100").len(), 2);
    // the desk phone unregisters its contact (Expires: 0)
    let gone = Reg {
        expires: Some(0),
        cseq: 3,
        ..reg(&desk_contact, "desk")
    };
    assert_eq!(code(&desk.register(h.udp, &gone).await.0), 200);
    let removed = h.binding(BindingChange::Removed).await;
    assert_eq!(removed.flow.source, desk.addr);
    // a call rings the live binding only
    assert_eq!(h.ring("L1"), Ok(1));
    let invite = soft.request(Method::Invite, WAIT).await.expect("the soft phone rings");
    assert!(desk.request(Method::Invite, Duration::from_millis(300)).await.is_none());
    soft.reply(&invite, h.udp, 486, "s1", "").await;
    let reason = h
        .event(|e| match e {
            UaEvent::Ended { leg, reason } if leg == "L1" => Some(reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, EndReason::Busy);
    // Contact: * removes them all; the device then cannot ring
    let all = Reg {
        contact: "*",
        expires: Some(0),
        cseq: 5,
        ..reg(&soft_contact, "soft")
    };
    assert_eq!(code(&soft.register(h.udp, &all).await.0), 200);
    assert!(h.ua.bindings("p100").is_empty());
    assert_eq!(h.ring("L2"), Err("noBinding".to_string()));
}

#[tokio::test]
async fn a_binding_expires_and_then_cannot_ring() {
    let mut h = Harness::start(RegistrarLimits {
        min_expires: 1,
        ..Default::default()
    })
    .await;
    let phone = UdpPhone::new().await;
    let r = Reg {
        expires: Some(1),
        ..Default::default()
    };
    assert_eq!(code(&phone.register(h.udp, &r).await.0), 200);
    h.binding(BindingChange::Added).await;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !h.ua.bindings("p100").is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the binding expires");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.ring("L1"), Err("noBinding".to_string()));
}

#[tokio::test]
async fn rotation_ends_the_old_credentials_and_revocation_the_device() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let phone = UdpPhone::new().await;
    assert_eq!(code(&phone.register(h.udp, &Reg::default()).await.0), 200);
    h.binding(BindingChange::Added).await;
    // rotated: new verifiers of a new revision; the bindings end
    h.directory
        .put("D1", "p100", 2, "a-new-password-2", &[Algorithm::Sha256]);
    assert_eq!(h.ua.drop_device("p100"), 1);
    h.binding(BindingChange::Removed).await;
    assert_eq!(h.ring("L1"), Err("noBinding".to_string()));
    let old = Reg {
        call_id: "reg-2",
        ..Default::default()
    };
    assert_eq!(
        code(&phone.register(h.udp, &old).await.0),
        403,
        "the old password no longer signs in"
    );
    let new = Reg {
        password: "a-new-password-2",
        call_id: "reg-3",
        ..Default::default()
    };
    assert_eq!(code(&phone.register(h.udp, &new).await.0), 200);
    assert_eq!(h.binding(BindingChange::Added).await.revision, 2);
    // revoked: the directory no longer has it
    lock(&h.directory.devices).clear();
    h.ua.drop_device("p100");
    let again = Reg {
        password: "a-new-password-2",
        call_id: "reg-4",
        ..Default::default()
    };
    assert_eq!(code(&phone.register(h.udp, &again).await.0), 403);
    assert_eq!(h.ring("L2"), Err("noBinding".to_string()));
}

#[tokio::test]
async fn a_call_forks_to_every_binding_and_the_first_answer_wins() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let (desk, soft) = (UdpPhone::new().await, UdpPhone::new().await);
    for (phone, call_id) in [(&desk, "desk"), (&soft, "soft")] {
        let contact = format!("<sip:p100@{}>", phone.addr);
        let r = Reg {
            contact: &contact,
            call_id,
            ..Default::default()
        };
        assert_eq!(code(&phone.register(h.udp, &r).await.0), 200);
    }
    assert_eq!(h.ring("L1"), Ok(2));
    let to_desk = desk.request(Method::Invite, WAIT).await.expect("the desk phone rings");
    let to_soft = soft.request(Method::Invite, WAIT).await.expect("the soft phone rings");
    assert!(String::from_utf8_lossy(&to_desk.body).contains("G722/8000"));
    assert!(to_desk.from_header().unwrap().value().contains("Front desk"));
    desk.reply(&to_desk, h.udp, 180, "d1", "").await;
    soft.reply(&to_soft, h.udp, 180, "s1", "").await;
    h.event(|e| matches!(e, UaEvent::Ringing { ref leg } if leg == "L1").then_some(()))
        .await;
    // the desk phone answers first
    desk.reply(&to_desk, h.udp, 200, "d1", &answer_sdp(40_010)).await;
    let (answer, source) = h
        .event(|e| match e {
            UaEvent::DeviceAnswered { leg, answer, source } if leg == "L1" => Some((answer, source)),
            _ => None,
        })
        .await;
    assert!(answer.contains("G722"));
    assert_eq!(source, desk.addr, "the media latches toward the flow that answered");
    desk.request(Method::Ack, WAIT).await.expect("the answer is ACKed");
    // the soft phone's offer is canceled
    let cancel = soft
        .request(Method::Cancel, WAIT)
        .await
        .expect("the other binding is canceled");
    soft.reply(&cancel, h.udp, 200, "s1", "").await;
    soft.reply(&to_soft, h.udp, 487, "s1", "").await;
    // the call goes on: hanging up sends the winner a BYE
    h.ua.hangup("L1");
    let bye = desk
        .request(Method::Bye, WAIT)
        .await
        .expect("BYE to the phone that answered");
    desk.reply(&bye, h.udp, 200, "", "").await;
    let reason = h
        .event(|e| match e {
            UaEvent::Ended { leg, reason } if leg == "L1" => Some(reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, EndReason::Hangup);
}

#[tokio::test]
async fn a_busy_phone_ends_only_its_ring_and_a_decline_declines() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let (desk, soft) = (UdpPhone::new().await, UdpPhone::new().await);
    for (phone, call_id) in [(&desk, "desk"), (&soft, "soft")] {
        let contact = format!("<sip:p100@{}>", phone.addr);
        let r = Reg {
            contact: &contact,
            call_id,
            ..Default::default()
        };
        assert_eq!(code(&phone.register(h.udp, &r).await.0), 200);
    }
    let ended = |leg: &'static str| {
        move |e: UaEvent| match e {
            UaEvent::Ended { leg: l, reason } if l == leg => Some(reason),
            _ => None,
        }
    };
    // both bindings answer 486 (on another call): the leg ends as busy, which is not the person's decline
    assert_eq!(h.ring("L1"), Ok(2));
    let to_desk = desk.request(Method::Invite, WAIT).await.expect("the desk phone rings");
    let to_soft = soft.request(Method::Invite, WAIT).await.expect("the soft phone rings");
    desk.reply(&to_desk, h.udp, 486, "d1", "").await;
    soft.reply(&to_soft, h.udp, 486, "s1", "").await;
    assert_eq!(h.event(ended("L1")).await, EndReason::Busy);
    // one binding busy, the other's reject key (603): the person declined
    assert_eq!(h.ring("L2"), Ok(2));
    let to_desk = desk.request(Method::Invite, WAIT).await.expect("the desk phone rings");
    let to_soft = soft.request(Method::Invite, WAIT).await.expect("the soft phone rings");
    desk.reply(&to_desk, h.udp, 486, "d2", "").await;
    soft.reply(&to_soft, h.udp, 603, "s2", "").await;
    assert_eq!(h.event(ended("L2")).await, EndReason::Declined);
    // 600 Busy Everywhere is a decline too
    assert_eq!(h.ring("L3"), Ok(2));
    let to_desk = desk.request(Method::Invite, WAIT).await.expect("the desk phone rings");
    let to_soft = soft.request(Method::Invite, WAIT).await.expect("the soft phone rings");
    desk.reply(&to_desk, h.udp, 600, "d3", "").await;
    soft.reply(&to_soft, h.udp, 486, "s3", "").await;
    assert_eq!(h.event(ended("L3")).await, EndReason::Declined);
}

#[tokio::test]
async fn rport_is_honored_and_an_outbound_flow_is_kept_and_used() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    // over UDP behind a NAT: the Via names a private address the phone cannot be reached at
    let natted = UdpPhone::new().await;
    let r = Reg {
        sent_by: Some("192.168.1.20:5060"),
        expires: Some(3600),
        ..Default::default()
    };
    let (ok, _) = natted.register(h.udp, &r).await;
    assert_eq!(code(&ok), 200);
    let added = h.binding(BindingChange::Added).await;
    assert_eq!(
        added.flow.source, natted.addr,
        "the flow is where the request came from (rport)"
    );
    assert!(
        added.remaining(now_secs()) <= 60,
        "behind a NAT without Outbound, a short expiry"
    );
    assert_eq!(h.ring("L0"), Ok(1));
    let invite = natted
        .request(Method::Invite, WAIT)
        .await
        .expect("the INVITE reaches the NAT's address");
    natted.reply(&invite, h.udp, 486, "n1", "").await;
    h.event(|e| matches!(e, UaEvent::Ended { ref leg, .. } if leg == "L0").then_some(()))
        .await;
    h.ua.drop_device("p100");
    h.binding(BindingChange::Removed).await;

    // over TCP with Outbound: the phone's connection is the flow; it keeps it up and calls come over it
    let mut tcp = TcpStream::connect(h.tcp).await.unwrap();
    let local = tcp.local_addr().unwrap();
    let outbound = Reg {
        contact: "<sip:p100@192.168.1.20:5099;transport=tcp;ob>;+sip.instance=\"<urn:uuid:00000000-0000-1000-8000-000a95a0e128>\";reg-id=1",
        expires: Some(3600),
        extra: "Supported: outbound, path\r\n",
        call_id: "reg-tcp",
        ..Default::default()
    };
    let sent_by = "192.168.1.20:5099";
    tcp.write_all(register_text(&outbound, "TCP", sent_by, h.tcp, None).as_bytes())
        .await
        .unwrap();
    let mut reader = StreamReader::default();
    let challenge = reader.response(&mut tcp).await;
    assert_eq!(code(&challenge), 401);
    let auth = authorization(&challenge, &outbound, h.tcp);
    let signed = Reg { cseq: 2, ..outbound };
    tcp.write_all(register_text(&signed, "TCP", sent_by, h.tcp, Some(&auth)).as_bytes())
        .await
        .unwrap();
    let ok = reader.response(&mut tcp).await;
    assert_eq!(code(&ok), 200);
    assert_eq!(header_values(&ok.headers, "Require"), vec!["outbound".to_string()]);
    assert_eq!(header_values(&ok.headers, "Flow-Timer"), vec!["120".to_string()]);
    let added = h.binding(BindingChange::Added).await;
    assert!(added.outbound);
    assert_eq!(added.flow.source, local);
    assert!(added.remaining(now_secs()) > 3000, "an Outbound flow keeps its expiry");
    // the keepalive: a CRLF ping is answered with a CRLF pong
    tcp.write_all(b"\r\n\r\n").await.unwrap();
    let mut pong = [0u8; 2];
    tokio::time::timeout(WAIT, tcp.read_exact(&mut pong))
        .await
        .expect("a pong in time")
        .unwrap();
    assert_eq!(&pong, b"\r\n");
    // a call to the device comes over the same connection (nothing listens at the contact's address)
    assert_eq!(h.ring("L1"), Ok(1));
    let invite = reader.request(&mut tcp, Method::Invite).await;
    assert!(invite.uri.to_string().contains("192.168.1.20:5099"));
    tcp.write_all(response_text(&invite, 486, "t1", "", local).as_bytes())
        .await
        .unwrap();
    h.event(|e| matches!(e, UaEvent::Ended { ref leg, .. } if leg == "L1").then_some(()))
        .await;
    // the connection closing ends the binding over it
    drop(tcp);
    let removed = h.binding(BindingChange::Removed).await;
    assert_eq!(removed.flow.source, local);
    assert!(h.ua.bindings("p100").is_empty());
}

#[tokio::test]
async fn a_registered_phone_places_a_call_signed_with_its_credentials() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let phone = UdpPhone::new().await;
    let uri = format!("sip:+15557654321@{}", h.udp);
    let invite = |auth: &str, call_id: &str| {
        let body = answer_sdp(40_020);
        format!(
            "INVITE {uri} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-{call_id};rport\r\nMax-Forwards: 70\r\n\
             From: <sip:p100@{REALM}>;tag={call_id}\r\nTo: <{uri}>\r\nCall-ID: {call_id}\r\nCSeq: 1 INVITE\r\n\
             Contact: <sip:p100@{}>\r\n{auth}Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
            phone.addr,
            phone.addr,
            body.len()
        )
    };
    phone.send(&invite("", "out-0"), h.udp).await;
    let challenge = phone.response().await;
    assert_eq!(code(&challenge), 407);
    let challenges: Vec<Challenge> = header_values(&challenge.headers, "Proxy-Authenticate")
        .iter()
        .filter_map(|v| Challenge::parse(v))
        .collect();
    let credentials = Credentials {
        username: "p100".to_string(),
        password: PASSWORD.to_string(),
        algorithms: vec![Algorithm::Sha256],
        ..Default::default()
    };
    let auth = digest::answer(
        &credentials,
        digest::choose(&challenges, &credentials).unwrap(),
        "INVITE",
        &uri,
        "cn",
    )
    .unwrap();
    phone
        .send(&invite(&format!("Proxy-Authorization: {auth}\r\n"), "out-1"), h.udp)
        .await;
    let call = h
        .event(|e| match e {
            UaEvent::Incoming(call) => Some(call),
            _ => None,
        })
        .await;
    assert_eq!(call.device.as_deref(), Some("D1"));
    assert_eq!((call.trunk_id.as_str(), call.to.as_str()), ("", "+15557654321"));
    assert_eq!(call.source, phone.addr);
    let _ = call.reply.send(Decision::Reject(486));
    let mut refused = phone.response().await;
    while code(&refused) < 400 {
        refused = phone.response().await;
    }
    assert_eq!(code(&refused), 486);
}

/// Reads SIP messages off a stream, as a phone's TCP connection does.
#[derive(Default)]
struct StreamReader {
    buffer: Vec<u8>,
}

impl StreamReader {
    async fn next(&mut self, tcp: &mut TcpStream) -> SipMessage {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            while self.buffer.starts_with(b"\r\n") {
                self.buffer.drain(..2);
            }
            if let Some(end) = self.buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&self.buffer[..end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                if self.buffer.len() >= end + 4 + length {
                    let message: Vec<u8> = self.buffer.drain(..end + 4 + length).collect();
                    return SipMessage::try_from(String::from_utf8_lossy(&message).as_ref()).expect("a SIP message");
                }
            }
            let mut chunk = [0u8; 4096];
            let read = tokio::time::timeout_at(deadline, tcp.read(&mut chunk))
                .await
                .expect("a message in time")
                .unwrap();
            assert!(read > 0, "the connection closed");
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    async fn response(&mut self, tcp: &mut TcpStream) -> Response {
        loop {
            if let SipMessage::Response(resp) = self.next(tcp).await {
                if code(&resp) >= 200 {
                    return resp;
                }
            }
        }
    }

    async fn request(&mut self, tcp: &mut TcpStream, method: Method) -> Request {
        loop {
            if let SipMessage::Request(req) = self.next(tcp).await {
                if req.method == method {
                    return req;
                }
            }
        }
    }
}

#[tokio::test]
async fn with_the_registrar_off_a_register_is_refused_without_a_challenge() {
    let (tx, mut events) = mpsc::unbounded_channel();
    let ua = Ua::start(
        UaConfig {
            udp: Some("127.0.0.1:0".parse().unwrap()),
            realm: REALM.to_string(),
            ..Default::default()
        },
        tx,
    )
    .await
    .unwrap();
    let udp = ua.bound().udp.unwrap();
    let phone = UdpPhone::new().await;
    for cseq in 1..=4 {
        let r = Reg {
            cseq,
            ..Default::default()
        };
        phone
            .send(&register_text(&r, "UDP", &phone.addr.to_string(), udp, None), udp)
            .await;
        assert_eq!(code(&phone.response().await), 403, "no challenge to answer");
    }
    // nothing counted against the phone's address, which a misconfigured phone retrying would otherwise get blocked
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, UaEvent::AuthFailure { .. }), "{event:?}");
    }
    ua.shutdown();
}

#[tokio::test]
async fn a_forged_nonce_is_not_looked_up_and_a_via_does_not_choose_the_transport() {
    let mut h = Harness::start(RegistrarLimits::default()).await;
    let phone = UdpPhone::new().await;
    // an answer to a nonce this side never issued fails whoever it names, without a lookup of the username
    let forged = format!(
        "Digest username=\"p100\", realm=\"{REALM}\", nonce=\"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\", \
         uri=\"sip:{}\", response=\"{}\", algorithm=SHA-256, qop=auth, nc=00000001, cnonce=\"c1\"",
        h.udp,
        "0".repeat(64)
    );
    let r = Reg::default();
    phone
        .send(
            &register_text(&r, "UDP", &phone.addr.to_string(), h.udp, Some(&forged)),
            h.udp,
        )
        .await;
    assert_eq!(code(&phone.response().await), 403);
    h.event(|e| matches!(e, UaEvent::AuthFailure { .. }).then_some(()))
        .await;
    assert_eq!(h.directory.lookups.load(Ordering::Relaxed), 0);
    // a REGISTER over UDP whose Via says TCP: its binding is the UDP flow it came over, so a call to it goes there
    let r = Reg {
        call_id: "reg-via",
        ..Default::default()
    };
    phone
        .send(&register_text(&r, "TCP", &phone.addr.to_string(), h.udp, None), h.udp)
        .await;
    let challenge = phone.response().await;
    assert_eq!(code(&challenge), 401);
    let auth = authorization(&challenge, &r, h.udp);
    let signed = Reg { cseq: 2, ..r };
    phone
        .send(
            &register_text(&signed, "TCP", &phone.addr.to_string(), h.udp, Some(&auth)),
            h.udp,
        )
        .await;
    assert_eq!(code(&phone.response().await), 200);
    assert_eq!(h.directory.lookups.load(Ordering::Relaxed), 1);
    let added = h.binding(BindingChange::Added).await;
    assert_eq!((added.flow.transport, added.flow.source), (Transport::Udp, phone.addr));
    h.ua.shutdown();
}
