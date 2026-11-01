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

//! SMS and MMS (plans/Comms-Framework.md §7.1 C4-SMS) against a fake Telnyx on loopback: a provider and a number with no
//! SIP engine; webhooks signed, badly signed, replayed and out of their window; duplicate callbacks; delivery receipts;
//! MMS in and out within the limits; an uncertain send that is not sent again; refusals before the provider (an opt-out, a
//! promotional text with no consent, an international number, a number not granted, the day's limit); the number pair's
//! room shared by a text and a call; and a sign-in code through the real sender, kept in no room.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair};
use base64::Engine;
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::service::chat_bridge::{webhook, WebhookRequest, WebhookResponse};
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

const OURS: &str = "+15550001000";
const THEIRS: &str = "+15557778888";
const BRIDGE: &str = "SMS1";

// ---- the fake Telnyx ----

/// What the fake answers a send with next.
#[derive(Clone, Debug)]
enum Reply {
    Status(u16, String),
    /// Read the request, then close the connection with no answer.
    Drop,
}

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

#[derive(Default)]
struct FakeState {
    sends: Vec<Seen>,
    script: VecDeque<Reply>,
    next: u64,
    files: HashMap<String, (String, Vec<u8>)>,
}

struct Fake {
    base: String,
    state: Arc<Mutex<FakeState>>,
}

impl Fake {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FakeState::default()));
        let served = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let state = Arc::clone(&served);
                std::thread::spawn(move || serve(stream, &state));
            }
        });
        Self { base, state }
    }

    fn script(&self, replies: &[Reply]) {
        self.state.lock().unwrap().script.extend(replies.iter().cloned());
    }

    fn file(&self, name: &str, content_type: &str, bytes: &[u8]) -> String {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(name.to_string(), (content_type.to_string(), bytes.to_vec()));
        format!("{}/media/{name}", self.base)
    }

    fn sends(&self) -> Vec<Seen> {
        self.state.lock().unwrap().sends.clone()
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<FakeState>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let Ok(read) = stream.read(&mut chunk) else { return };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let Ok(read) = stream.read(&mut chunk) else { return };
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = String::from_utf8_lossy(&buffer[head_end..]).to_string();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let answer = |stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]| {
        let head = format!(
            "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    };
    if method == "GET" {
        let file = state
            .lock()
            .unwrap()
            .files
            .get(path.trim_start_matches("/media/"))
            .cloned();
        match file {
            Some((content_type, bytes)) => answer(&mut stream, 200, &content_type, &bytes),
            None => answer(&mut stream, 404, "text/plain", b"no such file"),
        }
        return;
    }
    let reply = {
        let mut state = state.lock().unwrap();
        state.sends.push(Seen {
            path: path.clone(),
            headers: headers.clone(),
            body: body.clone(),
        });
        state.next += 1;
        let next = state.next;
        state.script.pop_front().unwrap_or_else(|| {
            Reply::Status(
                200,
                json!({"data": {"id": format!("msg-{next}"), "record_type": "message", "to": [{"status": "queued"}]}})
                    .to_string(),
            )
        })
    };
    match reply {
        Reply::Status(status, text) => answer(&mut stream, status, "application/json", text.as_bytes()),
        Reply::Drop => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

// ---- the bed ----

struct Bed {
    ctx: ExecutionContext,
    facade: EntityFacade,
    fake: Fake,
    key: Ed25519KeyPair,
    seq: std::cell::Cell<u64>,
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, Value)]) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value(entity).unwrap();
    for (field, value) in fields {
        row.set(*field, value.clone());
    }
    row.create(facade).unwrap();
}

fn call(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    moqui_core::service::call::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// The errors so far, cleared.
fn errors() -> String {
    ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default()
}

fn ok(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let out = call(name, parameters);
    let errors = errors();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn out_text(out: &BTreeMap<String, Value>, key: &str) -> String {
    out.get(key).map(to_plain_string).unwrap_or_default()
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    moqui_core::user::internal_login_user(user_id).unwrap();
}

/// A number (ours) routed to alice, granted to alice and bob, its texts through bridge SMS1 on a fake Telnyx; the system
/// texts from it.
fn bed() -> Bed {
    moqui_lib::env::set_property("channels_public_url", "https://moqui.example.test");
    moqui_lib::env::set_property("channels_media_key", "a-test-media-key");
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::inline(),
        moqui_core::context::framework_dir(),
    );
    let facade = moqui_core::testing::open();
    // the files (dbresource://) are kept in tables the in-memory facade has
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let fake = Fake::start();
    let key = Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap();
    let public = base64::engine::general_purpose::STANDARD.encode(key.public_key().as_ref());
    {
        let _entered = ctx.enter();
        for id in ["alice", "bob", "carol"] {
            row(
                &facade,
                "moqui.security.UserAccount",
                &[
                    ("userId", text(id)),
                    ("username", text(id)),
                    ("userFullName", text(&id.to_ascii_uppercase())),
                ],
            );
        }
        row(
            &facade,
            "moqui.chat.ChatBridge",
            &[
                ("chatBridgeId", text(BRIDGE)),
                ("typeEnumId", text("CbtSms")),
                ("adapterName", text("sms")),
                ("name", text("Telnyx")),
                ("statusId", text("CbsNotConnected")),
                (
                    "configJson",
                    text(
                        &json!({"apiKey": "KEY-1", "publicKey": public, "apiBaseUrl": fake.base, "allowInternalAddresses": true,
                            "messagingProfileId": "profile-1", "campaignId": "C1234", "maxMediaCount": 3, "maxMediaBytes": 2000})
                        .to_string(),
                    ),
                ),
            ],
        );
        row(
            &facade,
            "moqui.call.PhoneNumber",
            &[
                ("phoneNumber", text(OURS)),
                ("messagingBridgeId", text(BRIDGE)),
                ("routeTypeEnumId", text("PnrUser")),
                ("routeId", text("alice")),
            ],
        );
        for user in ["alice", "bob"] {
            row(
                &facade,
                "moqui.call.PhoneNumberUser",
                &[
                    ("phoneNumber", text(OURS)),
                    ("userId", text(user)),
                    ("fromDate", Value::I64(now() - 60_000)),
                ],
            );
        }
        row(
            &facade,
            "moqui.call.VoicePolicy",
            &[
                ("voicePolicyId", text("DEFAULT")),
                ("homeCountryCode", text("1")),
                ("smsSystemNumber", text(OURS)),
            ],
        );
    }
    Bed {
        ctx,
        facade,
        fake,
        key,
        seq: std::cell::Cell::new(0),
    }
}

impl Bed {
    fn next(&self) -> u64 {
        self.seq.set(self.seq.get() + 1);
        self.seq.get()
    }

    /// A webhook request signed at `at_seconds` (the key's own unless `forged`).
    fn signed(&self, body: &Json, at_seconds: i64) -> WebhookRequest {
        let body = body.to_string().into_bytes();
        let stamp = at_seconds.to_string();
        let mut message = stamp.as_bytes().to_vec();
        message.push(b'|');
        message.extend_from_slice(&body);
        let signature = base64::engine::general_purpose::STANDARD.encode(self.key.sign(&message).as_ref());
        WebhookRequest {
            method: "POST".to_string(),
            headers: vec![
                ("telnyx-signature-ed25519".to_string(), signature),
                ("telnyx-timestamp".to_string(), stamp),
                ("content-type".to_string(), "application/json".to_string()),
            ],
            body,
            client_ip: "198.51.100.7".to_string(),
            now_millis: now(),
        }
    }

    fn hook(&self, request: &WebhookRequest) -> WebhookResponse {
        // the host's request worker: a fresh context of its own
        let context = self.ctx.factory().new_context();
        let _entered = context.enter();
        webhook(self.ctx.factory(), BRIDGE, request)
    }

    fn received(&self, event: &str, message: &str, from: &str, body: &str, media: Json) -> Json {
        json!({"data": {"event_type": "message.received", "id": event, "occurred_at": "2026-11-01T12:00:00.000+00:00",
            "record_type": "event", "payload": {"id": message, "direction": "inbound", "type": "SMS",
                "from": {"phone_number": from, "carrier": "T"}, "to": [{"phone_number": OURS, "status": "webhook_delivered"}],
                "text": body, "media": media, "received_at": "2026-11-01T12:00:00.000+00:00"}},
            "meta": {"attempt": 1}})
    }

    /// A text from `from`, signed now; the response.
    fn text_in(&self, from: &str, body: &str, media: Json) -> (WebhookResponse, String) {
        let n = self.next();
        let message = format!("in-{n}");
        let event = self.received(&format!("ev-{n}"), &message, from, body, media);
        (self.hook(&self.signed(&event, now() / 1000)), message)
    }

    fn receipt(&self, message: &str, status: &str) -> WebhookResponse {
        let n = self.next();
        let mut payload =
            json!({"id": message, "direction": "outbound", "to": [{"phone_number": THEIRS, "status": status}]});
        if status.ends_with("failed") {
            payload["errors"] = json!([{"code": "40002", "title": "Blocked", "detail": "The carrier refused it"}]);
        }
        let event = json!({"data": {"event_type": "message.finalized", "id": format!("ev-r{n}"),
            "occurred_at": "2026-11-01T12:00:05.000+00:00", "payload": payload}});
        self.hook(&self.signed(&event, now() / 1000))
    }

    fn room(&self, theirs: &str) -> Option<String> {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.chat.ChatRoomBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .eq("externalRoomId", text(&format!("{OURS}/{theirs}")))
            .one()
            .unwrap()
            .and_then(|row| row.get("chatRoomId").map(to_plain_string))
    }

    fn count(&self, entity: &str, fields: &[(&str, Value)]) -> i64 {
        let _authz = self.ctx.disable_authz();
        let mut find = self.facade.find(entity).unwrap();
        for (name, value) in fields {
            find = find.eq(name, value.clone());
        }
        find.count().unwrap()
    }

    fn one(&self, entity: &str, fields: &[(&str, Value)]) -> BTreeMap<String, String> {
        let _authz = self.ctx.disable_authz();
        let mut find = self.facade.find(entity).unwrap();
        for (name, value) in fields {
            find = find.eq(name, value.clone());
        }
        let row = find.one().unwrap().unwrap_or_else(|| panic!("no {entity} {fields:?}"));
        row.fields()
            .iter()
            .map(|(name, value)| (name.to_string(), to_plain_string(value)))
            .collect()
    }

    /// The messages of a room: (sender actor, body text), oldest first.
    fn messages(&self, room: &str) -> Vec<(String, String)> {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.chat.ChatEvent")
            .unwrap()
            .eq("chatRoomId", text(room))
            .eq("eventTypeEnumId", text("CetMessage"))
            .order_by("eventSeq", false)
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| {
                let get = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
                (get("senderActorId"), get("bodyText"))
            })
            .collect()
    }

    /// alice writes in the room; its eventSeq.
    fn send(&self, room: &str, body: &str, files: &[String]) -> i64 {
        login("alice");
        let out = ok(
            "org.moqui.impl.ChatServices.send#Message",
            &[
                ("chatRoomId", text(room)),
                ("body", text(&format!("<p>{body}</p>"))),
                (
                    "chatAttachmentIdList",
                    Value::List(files.iter().map(|id| text(id)).collect()),
                ),
            ],
        );
        out_text(&out, "eventSeq").parse().unwrap()
    }

    fn round(&self, offset: i64) {
        let _authz = self.ctx.disable_authz();
        ok(
            "org.moqui.impl.ChatBridgeServices.run#BridgeDeliveries",
            &[("nowMillis", Value::I64(now() + offset))],
        );
    }

    /// The delivery of event `seq` of `room`: (state, reason, attempts, remote id, receipt).
    fn delivery(&self, room: &str, seq: i64) -> (String, String, String, String, String) {
        let row = self.one(
            "moqui.chat.ChatBridgeDelivery",
            &[("chatRoomId", text(room)), ("eventSeq", Value::I64(seq))],
        );
        let get = |name: &str| row.get(name).cloned().unwrap_or_default();
        (
            get("statusEnumId"),
            get("reasonCode"),
            get("attemptCount"),
            get("remoteEventId"),
            get("receiptStatus"),
        )
    }

    /// A file alice uploaded to a room, `size` bytes.
    fn upload(&self, room: &str, name: &str, size: usize) -> String {
        let _authz = self.ctx.disable_authz();
        let mut file = self.facade.make_value("moqui.chat.ChatAttachment").unwrap();
        file.set_sequenced_id_primary(&self.facade).unwrap();
        let id = file.get("chatAttachmentId").map(to_plain_string).unwrap();
        let location = format!("dbresource://chat/{room}/{id}/{name}");
        self.ctx
            .factory()
            .resource()
            .write_bytes(&location, &vec![b'x'; size])
            .unwrap();
        for (field, value) in [
            ("chatRoomId", text(room)),
            ("location", text(&location)),
            ("fileName", text(name)),
            ("contentType", text("image/png")),
            ("fileSize", Value::I64(i64::try_from(size).unwrap())),
            ("uploadedByUserId", text("alice")),
        ] {
            file.set(field, value);
        }
        file.create(&self.facade).unwrap();
        id
    }
}

fn status(response: &WebhookResponse) -> u16 {
    response.status
}

fn body_json(response: &WebhookResponse) -> Json {
    serde_json::from_slice(&response.body).unwrap_or(Json::Null)
}

// ---- the tests ----

#[test]
fn signed_texts_come_in_once_and_bad_or_replayed_ones_are_refused() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let n = bed.next();
    let event = bed.received(&format!("ev-{n}"), "in-1", THEIRS, "Hello there", json!([]));
    let request = bed.signed(&event, now() / 1000);
    let first = bed.hook(&request);
    assert_eq!(status(&first), 200, "{}", String::from_utf8_lossy(&first.body));
    assert_eq!(body_json(&first)["outcome"], "accepted");
    let room = bed.room(THEIRS).expect("the pair room");
    let kind = bed.one("moqui.chat.ChatRoom", &[("chatRoomId", text(&room))]);
    assert_eq!(kind["roomTypeEnumId"], "CrtExternal");
    // its members: alice (routed and granted), bob (granted); not carol; and the outside number
    for (user, expected) in [("alice", 1), ("bob", 1), ("carol", 0)] {
        assert_eq!(
            bed.count(
                "moqui.chat.ChatRoomMember",
                &[("chatRoomId", text(&room)), ("userId", text(user))]
            ),
            expected,
            "{user}"
        );
    }
    assert_eq!(bed.messages(&room).len(), 1);
    assert_eq!(bed.messages(&room)[0].1, "Hello there");

    // the same request again is a replay
    let replay = bed.hook(&request);
    assert_eq!(status(&replay), 409);
    assert!(replay.auth_failure);
    // a changed body under the same signature, a wrong key's signature, no signature, a time out of the window
    let mut tampered = request.clone();
    tampered.body = event.to_string().replace("Hello there", "Send money").into_bytes();
    assert_eq!(status(&bed.hook(&tampered)), 401);
    let other = Ed25519KeyPair::from_seed_unchecked(&[9u8; 32]).unwrap();
    let mut forged = bed.signed(&event, now() / 1000);
    let mut message = forged.header("telnyx-timestamp").unwrap().as_bytes().to_vec();
    message.push(b'|');
    message.extend_from_slice(&forged.body);
    forged.headers[0].1 = base64::engine::general_purpose::STANDARD.encode(other.sign(&message).as_ref());
    let refused = bed.hook(&forged);
    assert_eq!(status(&refused), 401);
    assert!(refused.auth_failure);
    let mut unsigned = request.clone();
    unsigned.headers.retain(|(name, _)| name != "telnyx-signature-ed25519");
    assert_eq!(status(&bed.hook(&unsigned)), 401);
    let stale = bed.signed(&event, now() / 1000 - 600);
    assert_eq!(status(&bed.hook(&stale)), 401);
    assert_eq!(bed.messages(&room).len(), 1, "nothing refused was taken");

    // the provider's duplicate callback (the same event, signed again later) changes nothing
    let again = bed.hook(&bed.signed(&event, now() / 1000 + 1));
    assert_eq!(status(&again), 200);
    assert_eq!(body_json(&again)["outcome"], "duplicate");
    // the same message in another event (a retry under a new event id): the message's id dedupes it
    let resent = bed.received("ev-other", "in-1", THEIRS, "Hello there", json!([]));
    assert_eq!(
        body_json(&bed.hook(&bed.signed(&resent, now() / 1000)))["outcome"],
        "duplicate"
    );
    assert_eq!(bed.messages(&room).len(), 1);

    // a text to a number that is not ours is left, answered so the provider stops
    let stray = json!({"data": {"event_type": "message.received", "id": "ev-stray", "payload": {"id": "in-x",
        "from": {"phone_number": THEIRS}, "to": [{"phone_number": "+15550009999"}], "text": "hi"}}});
    let left = bed.hook(&bed.signed(&stray, now() / 1000));
    assert_eq!(status(&left), 200);
    assert_eq!(body_json(&left)["outcome"], "ignored");
}

#[test]
fn a_reply_goes_out_and_its_receipts_come_back() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.text_in(THEIRS, "Are you open?", json!([]));
    let room = bed.room(THEIRS).unwrap();
    let seq = bed.send(&room, "Until six", &[]);
    bed.round(0);
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 1);
    let sent: Json = serde_json::from_str(&sends[0].body).unwrap();
    assert_eq!(sends[0].path, "/v2/messages");
    assert_eq!(sent["from"], OURS);
    assert_eq!(sent["to"], THEIRS);
    assert_eq!(sent["text"], "Until six");
    assert_eq!(sent["messaging_profile_id"], "profile-1");
    let header = |name: &str| {
        sends[0]
            .headers
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(header("authorization").as_deref(), Some("Bearer KEY-1"));
    assert!(header("idempotency-key").is_some_and(|key| key.starts_with("moqui-chat:")));
    let (state, _, _, remote, _) = bed.delivery(&room, seq);
    assert_eq!((state.as_str(), remote.as_str()), ("CdsAccepted", "msg-1"));
    assert_eq!(
        bed.one("moqui.chat.ChatBridge", &[("chatBridgeId", text(BRIDGE))])["statusId"],
        "CbsConnected"
    );
    // the send counted for alice's day
    assert_eq!(
        bed.count("moqui.call.SmsSendCount", &[("counterKey", text("user:alice"))]),
        1
    );

    // receipts: sent, then delivered; sent again (late) does not take it back; the same twice changes nothing
    assert_eq!(body_json(&bed.receipt("msg-1", "sent"))["found"], true);
    assert_eq!(bed.delivery(&room, seq).4, "sent");
    bed.receipt("msg-1", "delivered");
    bed.receipt("msg-1", "sent");
    bed.receipt("msg-1", "delivered");
    let (state, _, _, _, receipt) = bed.delivery(&room, seq);
    assert_eq!((state.as_str(), receipt.as_str()), ("CdsAccepted", "delivered"));
    login("alice");
    let deliveries = ok(
        "org.moqui.impl.ChatBridgeServices.get#EventDeliveries",
        &[
            ("chatRoomId", text(&room)),
            ("eventSeq", Value::I64(seq)),
            ("eventSeqList", Value::List(vec![Value::I64(seq)])),
        ],
    );
    assert!(out_text(&deliveries, "deliveryMap").contains("delivered"));

    // a carrier's refusal fails the delivery, with its reason, and it may be sent again
    let second = bed.send(&room, "Or seven", &[]);
    bed.round(0);
    let remote = bed.delivery(&room, second).3;
    bed.receipt(&remote, "delivery_failed");
    let (state, reason, ..) = bed.delivery(&room, second);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "deliveryFailed"));
    // a receipt of a message we do not know (another system's on the account) is answered and left
    assert_eq!(body_json(&bed.receipt("msg-unknown", "delivered"))["found"], false);
}

#[test]
fn mms_comes_in_and_goes_out_within_the_limits() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let picture = bed.fake.file("pic.jpg", "image/jpeg", &[1, 2, 3, 4, 5]);
    let page = bed.fake.file("page.html", "text/html", b"<script>alert(1)</script>");
    let missing = format!("{}/media/gone.png", bed.fake.base);
    let (response, _) = bed.text_in(
        THEIRS,
        "",
        json!([{"url": picture, "content_type": "image/jpeg", "size": 5},
            {"url": page, "content_type": "text/html", "size": 25},
            {"url": missing, "content_type": "image/png", "size": 9}]),
    );
    assert_eq!(
        body_json(&response)["outcome"],
        "accepted",
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let room = bed.room(THEIRS).unwrap();
    let messages = bed.messages(&room);
    assert_eq!(messages.len(), 1);
    // the file that could not be fetched is named; the others are the message's files
    assert!(
        messages[0].1.contains("A file not received: image/png"),
        "{}",
        messages[0].1
    );
    let _authz = bed.ctx.disable_authz();
    let files = bed
        .facade
        .find("moqui.chat.ChatAttachment")
        .unwrap()
        .eq("chatRoomId", text(&room))
        .order_by("attachmentSeq", false)
        .list()
        .unwrap();
    let files: Vec<_> = files.values().to_vec();
    assert_eq!(files.len(), 2);
    let get = |index: usize, name: &str| files[index].get(name).map(to_plain_string).unwrap_or_default();
    assert_eq!(
        (get(0, "contentType").as_str(), get(0, "fileSize").as_str()),
        ("image/jpeg", "5")
    );
    // a page from the network is never kept as one
    assert_eq!(get(1, "contentType"), "application/octet-stream");
    assert!(get(0, "uploadedByUserId").is_empty());
    drop(_authz);

    // out: a picture goes by a signed address, which serves it to the provider and nobody else
    let file = bed.upload(&room, "map.png", 100);
    let seq = bed.send(&room, "Here", std::slice::from_ref(&file));
    bed.round(0);
    let sent: Json = serde_json::from_str(&bed.fake.sends()[0].body).unwrap();
    let url = sent["media_urls"][0].as_str().unwrap().to_string();
    assert!(
        url.starts_with(&format!("https://moqui.example.test/channels/{BRIDGE}/media/{file}.")),
        "{url}"
    );
    let token = url.rsplit('/').next().unwrap();
    let served = moqui_core::service::chat_bridge::media(bed.ctx.factory(), BRIDGE, token, now()).unwrap();
    assert_eq!((served.bytes.len(), served.content_type.as_str()), (100, "image/png"));
    assert!(moqui_core::service::chat_bridge::media(bed.ctx.factory(), "OTHER", token, now()).is_none());
    assert!(moqui_core::service::chat_bridge::media(bed.ctx.factory(), BRIDGE, token, now() + 2 * 3_600_000).is_none());
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");

    // over the bridge's limits (3 files, 2000 bytes in all): refused before the provider, for good
    let many: Vec<String> = (0..4).map(|i| bed.upload(&room, &format!("f{i}.png"), 10)).collect();
    let too_many = bed.send(&room, "Many", &many);
    let large = bed.upload(&room, "big.png", 2500);
    let too_large = bed.send(&room, "Big", &[large]);
    bed.round(0);
    assert_eq!(bed.fake.sends().len(), 1);
    assert_eq!(bed.delivery(&room, too_many).1, "tooManyFiles");
    assert_eq!(bed.delivery(&room, too_large).1, "mediaTooLarge");
}

#[test]
fn an_uncertain_send_is_not_sent_again() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.text_in(THEIRS, "Hi", json!([]));
    let room = bed.room(THEIRS).unwrap();

    // the answer is lost after the request went: uncertain, and no round sends it again
    bed.fake.script(&[Reply::Drop]);
    let lost = bed.send(&room, "Lost", &[]);
    bed.round(0);
    let (state, reason, attempts, ..) = bed.delivery(&room, lost);
    assert_eq!(
        (state.as_str(), reason.as_str(), attempts.as_str()),
        ("CdsUncertain", "uncertain", "1")
    );
    bed.round(120_000);
    assert_eq!(bed.fake.sends().len(), 1);

    // the service away (503): tried again after the backoff, then taken
    bed.fake.script(&[Reply::Status(
        503,
        r#"{"errors":[{"code":"10007","title":"Unavailable"}]}"#.to_string(),
    )]);
    let away = bed.send(&room, "Later", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, away).0, "CdsPending");
    bed.round(10_000);
    assert_eq!(bed.delivery(&room, away).0, "CdsAccepted");
    assert_eq!(bed.fake.sends().len(), 3);

    // a STOP the provider holds (its error 40300) fails it for good
    bed.fake.script(&[Reply::Status(
        422,
        r#"{"errors":[{"code":"40300","title":"Blocked due to STOP message","detail":"The number opted out"}]}"#
            .to_string(),
    )]);
    let blocked = bed.send(&room, "Blocked", &[]);
    bed.round(20_000);
    let (state, reason, ..) = bed.delivery(&room, blocked);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "optedOut"));

    // the provider unreachable: not connected, tried again
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    {
        let _authz = bed.ctx.disable_authz();
        let mut row = bed
            .facade
            .find("moqui.chat.ChatBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .one()
            .unwrap()
            .unwrap();
        let mut config: Json = serde_json::from_str(&row.get("configJson").map(to_plain_string).unwrap()).unwrap();
        config["apiBaseUrl"] = Json::String(format!("http://{address}"));
        row.set("configJson", text(&config.to_string()));
        row.update(&bed.facade).unwrap();
    }
    let down = bed.send(&room, "Down", &[]);
    bed.round(30_000);
    assert_eq!(bed.delivery(&room, down).0, "CdsPending");
    assert_eq!(
        bed.one("moqui.chat.ChatBridge", &[("chatBridgeId", text(BRIDGE))])["statusId"],
        "CbsNotConnected"
    );
}

#[test]
fn refusals_come_before_the_provider() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.text_in(THEIRS, "STOP", json!([]));
    let room = bed.room(THEIRS).unwrap();
    assert_eq!(
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))]
        )["optedOut"],
        "Y"
    );
    let refused = bed.send(&room, "Are you sure?", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, refused).1, "optedOut");
    assert!(bed.fake.sends().is_empty());
    // START opts back in
    bed.text_in(THEIRS, "start", json!([]));
    let allowed = bed.send(&room, "Welcome back", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, allowed).0, "CdsAccepted");
    assert_eq!(bed.fake.sends().len(), 1);

    // an international number: refused for a user and for the system, before any room or send
    login("alice");
    let out = call(
        "org.moqui.impl.SmsServices.open#PairRoom",
        &[("toNumber", text("+442071234567")), ("fromNumber", text(OURS))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "internationalNotEnabled");
    errors();
    // carol may text from no number; the same parameters as alice's
    login("carol");
    let out = call(
        "org.moqui.impl.SmsServices.open#PairRoom",
        &[("toNumber", text("+15552223333")), ("fromNumber", text(OURS))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "noMessaging");
    errors();
    login("alice");
    let opened = ok(
        "org.moqui.impl.SmsServices.open#PairRoom",
        &[("toNumber", text("(555) 222-3333")), ("fromNumber", text(OURS))],
    );
    assert!(!out_text(&opened, "chatRoomId").is_empty());
    assert_eq!(bed.room("+15552223333"), Some(out_text(&opened, "chatRoomId")));

    let system = |to: &str, promotional: bool| {
        let out = call(
            "org.moqui.SmsServices.send#SmsMessage",
            &[
                ("contactNumber", text(to)),
                ("message", text("Sale today")),
                ("isPromotional", Value::Bool(promotional)),
            ],
        );
        errors();
        (out_text(&out, "successful"), out_text(&out, "reasonCode"))
    };
    assert_eq!(
        system("+442071234567", false),
        ("false".to_string(), "internationalNotEnabled".to_string())
    );
    // a promotional text with no sms-consent provider to say yes
    assert_eq!(
        system("+15552223333", true),
        ("false".to_string(), "consentRequired".to_string())
    );
    assert_eq!(system(THEIRS, false).0, "true");
    assert_eq!(bed.fake.sends().len(), 2);

    // the day's limit
    {
        let _authz = bed.ctx.disable_authz();
        let mut policy = bed
            .facade
            .find("moqui.call.VoicePolicy")
            .unwrap()
            .eq("voicePolicyId", text("DEFAULT"))
            .one()
            .unwrap()
            .unwrap();
        policy.set("smsMaxPerDay", Value::I64(2));
        policy.update(&bed.facade).unwrap();
    }
    assert_eq!(system(THEIRS, false), ("false".to_string(), "dailyLimit".to_string()));
    assert_eq!(bed.fake.sends().len(), 2);
}

#[test]
fn a_text_and_a_call_share_the_pair_room() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.text_in(THEIRS, "Call me", json!([]));
    let room = bed.room(THEIRS).unwrap();
    // a call from the same number, as the SIP engine hands it over (no engine runs: a node's rows say one is up)
    for entity in ["moqui.call.SipNode", "moqui.call.CallMediaNode"] {
        row(
            &bed.facade,
            entity,
            &[("instanceId", text("node-1")), ("heartbeatDate", Value::I64(now()))],
        );
    }
    let received = {
        let _authz = bed.ctx.disable_authz();
        ok(
            "org.moqui.impl.VoiceServices.receive#PhoneCall",
            &[
                ("toNumber", text(OURS)),
                ("fromNumber", text(THEIRS)),
                ("instanceId", text("node-1")),
            ],
        )
    };
    let session = out_text(&received, "callSessionId");
    assert!(!session.is_empty(), "no call: {received:?}");
    assert_eq!(
        bed.one("moqui.call.CallSession", &[("callSessionId", text(&session))])["chatRoomId"],
        room
    );
    let calls = bed.count(
        "moqui.chat.ChatEvent",
        &[("chatRoomId", text(&room)), ("eventTypeEnumId", text("CetCall"))],
    );
    assert_eq!(calls, 1, "the call's start is in the pair room");
    // one room for the pair still
    assert_eq!(
        bed.count("moqui.chat.ChatRoomBridge", &[("chatBridgeId", text(BRIDGE))]),
        1
    );
    // bob, a member of the room, is not in the call by being in the room
    login("bob");
    let seen = call(
        "org.moqui.impl.CallServices.get#Call",
        &[("callSessionId", text(&session))],
    );
    assert!(out_text(&seen, "call").is_empty());
    errors();
}

#[test]
fn a_sign_in_code_goes_through_the_real_sender() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.security.UserAuthcFactor",
        &[
            ("factorId", text("F1")),
            ("userId", text("alice")),
            ("factorTypeEnumId", text("UafSms")),
            ("factorOption", text(THEIRS)),
        ],
    );
    let _authz = bed.ctx.disable_authz();
    let out = ok(
        "org.moqui.impl.UserServices.send#AuthcCodeSms",
        &[("factorId", text("F1"))],
    );
    // the code made for it (a single-use factor of alice's) is in the text
    let single = bed.one(
        "moqui.security.UserAuthcFactor",
        &[("factorId", text(&out_text(&out, "singleUseFactorId")))],
    );
    assert_eq!(single["userId"], "alice");
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 1);
    let sent: Json = serde_json::from_str(&sends[0].body).unwrap();
    assert_eq!((sent["from"].as_str(), sent["to"].as_str()), (Some(OURS), Some(THEIRS)));
    let words = sent["text"].as_str().unwrap();
    // the code's text (UserAuthcOtpMessage) is localized data the in-memory database does not load
    assert!(!words.is_empty());
    // kept in no room
    assert!(bed.room(THEIRS).is_none());
    assert_eq!(
        bed.count("moqui.call.SmsSendCount", &[("counterKey", text("system"))]),
        1
    );
}
