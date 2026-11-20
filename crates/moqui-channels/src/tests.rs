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

//! SMS and MMS (plans/archive/Comms-Framework.md §7.1 C4-SMS) against a fake Telnyx on loopback: a provider and a number with no
//! SIP engine; webhooks signed, badly signed, replayed and out of their window; duplicate callbacks; delivery receipts;
//! MMS in and out within the limits; an uncertain send that is not sent again; refusals before the provider (an opt-out, a
//! promotional text with no consent, an international number, a number not granted, the day's limit); a campaign's
//! promotional text asked of a conf's sms-consent provider and counted for its number, apart from the system's own;
//! the number pair's room shared by a text and a call; and a sign-in code through the real sender, kept in no room.

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

// ---- the provider under test ----

/// The keys the bed signs webhooks with: each provider's (Twilio's and Sinch's secrets join Telnyx's key pair).
struct Keys {
    /// Telnyx's: the account's Ed25519 key, whose public half is the bridge's `publicKey`.
    ed25519: Ed25519KeyPair,
}

/// A provider the bed runs against: its adapter, its bridge's settings, the fake's answer to a send, and how a webhook
/// is signed. Batches of other providers add theirs beside [`TELNYX`].
#[derive(Clone, Copy)]
struct Provider {
    adapter_name: &'static str,
    bridge_name: &'static str,
    /// The bridge's `configJson`, given the fake's address and the bed's keys.
    config: fn(base: &str, keys: &Keys) -> Json,
    /// The fake's answer to the `n`th send (from 1) it has no script for.
    accepted: fn(n: u64, path: &str) -> Reply,
    /// The headers that sign `body` at `at_seconds`, posted to `url` (the public URL, for a provider that signs it).
    sign: Signer,
}

/// How a provider signs a webhook: the headers of `body` at `at_seconds`, posted to `url`.
type Signer = fn(keys: &Keys, body: &[u8], at_seconds: i64, url: &str) -> Vec<(String, String)>;

const TELNYX: Provider = Provider {
    adapter_name: "sms",
    bridge_name: "Telnyx",
    config: |base, keys| {
        let public = base64::engine::general_purpose::STANDARD.encode(keys.ed25519.public_key().as_ref());
        json!({"apiKey": "KEY-1", "publicKey": public, "apiBaseUrl": base, "allowInternalAddresses": true,
            "messagingProfileId": "profile-1", "campaignId": "C1234", "maxMediaCount": 3, "maxMediaBytes": 2000})
    },
    accepted: |n, _path| {
        Reply::Status(
            200,
            json!({"data": {"id": format!("msg-{n}"), "record_type": "message", "to": [{"status": "queued"}]}})
                .to_string(),
        )
    },
    sign: |keys, body, at_seconds, _url| {
        let stamp = at_seconds.to_string();
        let mut message = stamp.as_bytes().to_vec();
        message.push(b'|');
        message.extend_from_slice(body);
        let signature = base64::engine::general_purpose::STANDARD.encode(keys.ed25519.sign(&message).as_ref());
        vec![
            ("telnyx-signature-ed25519".to_string(), signature),
            ("telnyx-timestamp".to_string(), stamp),
            ("content-type".to_string(), "application/json".to_string()),
        ]
    },
};

// ---- the fake provider ----

/// What the fake answers a send with next.
#[derive(Clone, Debug)]
enum Reply {
    Status(u16, String),
    /// Read the request, then close the connection with no answer.
    Drop,
}

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

struct FakeState {
    sends: Vec<Seen>,
    script: VecDeque<Reply>,
    next: u64,
    files: HashMap<String, (String, Vec<u8>)>,
    /// The provider's answer to a send with no script.
    accepted: fn(n: u64, path: &str) -> Reply,
}

struct Fake {
    base: String,
    state: Arc<Mutex<FakeState>>,
}

impl Fake {
    /// A fake provider on loopback that answers a send as `provider` does.
    fn start(provider: &Provider) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FakeState {
            sends: Vec::new(),
            script: VecDeque::new(),
            next: 0,
            files: HashMap::new(),
            accepted: provider.accepted,
        }));
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
    // a GET is a file the fake holds, but a search of Twilio's Content API (v2) is answered as a send is
    if method == "GET" && !path.starts_with("/v2/Content") {
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
            method: method.clone(),
            path: path.clone(),
            headers: headers.clone(),
            body: body.clone(),
        });
        state.next += 1;
        let (next, accepted) = (state.next, state.accepted);
        state.script.pop_front().unwrap_or_else(|| accepted(next, &path))
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
    provider: Provider,
    fake: Fake,
    keys: Keys,
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
    bed_for(TELNYX, "")
}

/// [`bed`] on the conf `conf_text`.
fn bed_with(conf_text: &str) -> Bed {
    bed_for(TELNYX, conf_text)
}

/// [`bed`] for `provider`'s adapter and fake, on the conf `conf_text`.
fn bed_for(provider: Provider, conf_text: &str) -> Bed {
    moqui_lib::env::set_property("moqui_conf__channels__public_url", "https://moqui.example.test");
    moqui_lib::env::set_property("moqui_conf__channels__media_key", "a-test-media-key");
    let ctx = ExecutionContextFactory::test_context_in(
        conf_text,
        moqui_core::service::ServicePools::inline(),
        moqui_core::context::framework_dir(),
    );
    // the replay cache MoquiDefaultConf.xml declares (moqui.security.hmac.replay), which a test context has not
    ctx.factory()
        .cache()
        .get_or_create("moqui.security.hmac.replay")
        .unwrap();
    let facade = moqui_core::testing::open();
    // the files (dbresource://) are kept in tables the in-memory facade has
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    let fake = Fake::start(&provider);
    let keys = Keys {
        ed25519: Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap(),
    };
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
                ("adapterName", text(provider.adapter_name)),
                ("name", text(provider.bridge_name)),
                ("statusId", text("CbsNotConnected")),
                ("configJson", text(&(provider.config)(&fake.base, &keys).to_string())),
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
        provider,
        fake,
        keys,
        seq: std::cell::Cell::new(0),
    }
}

impl Bed {
    fn next(&self) -> u64 {
        self.seq.set(self.seq.get() + 1);
        self.seq.get()
    }

    /// A webhook request of JSON `body`, signed at `at_seconds` as the bed's provider signs.
    fn signed(&self, body: &Json, at_seconds: i64) -> WebhookRequest {
        self.signed_bytes(body.to_string().into_bytes(), at_seconds)
    }

    /// A webhook request of `body` as the provider sends it (JSON or a form), signed at `at_seconds`.
    fn signed_bytes(&self, body: Vec<u8>, at_seconds: i64) -> WebhookRequest {
        let path = format!("/channels/{BRIDGE}/webhook");
        let url = format!("https://moqui.example.test{path}");
        WebhookRequest {
            method: "POST".to_string(),
            url: path,
            headers: (self.provider.sign)(&self.keys, &body, at_seconds, &url),
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

    /// A text from `from` the provider received at `received_at` (RFC 3339), signed now; the response.
    fn text_in_at(&self, from: &str, body: &str, received_at: &str) -> WebhookResponse {
        let n = self.next();
        let mut event = self.received(&format!("ev-{n}"), &format!("in-{n}"), from, body, json!([]));
        event["data"]["payload"]["received_at"] = json!(received_at);
        self.hook(&self.signed(&event, now() / 1000))
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

    /// A group MMS from `from` to `to` and `cc`, signed now; the response.
    fn group_in(&self, from: &str, to: &[&str], cc: &[&str], body: &str) -> WebhookResponse {
        let n = self.next();
        let entries = |numbers: &[&str]| -> Json {
            numbers
                .iter()
                .map(|number| json!({"phone_number": number, "status": "webhook_delivered"}))
                .collect()
        };
        let event = json!({"data": {"event_type": "message.received", "id": format!("ev-g{n}"),
            "occurred_at": "2026-11-01T12:00:00.000+00:00", "record_type": "event", "payload": {"id": format!("in-g{n}"),
                "direction": "inbound", "type": "MMS", "from": {"phone_number": from, "carrier": "T"}, "to": entries(to),
                "cc": entries(cc), "text": body, "media": [], "received_at": "2026-11-01T12:00:00.000+00:00"}}, "meta": {"attempt": 1}});
        self.hook(&self.signed(&event, now() / 1000))
    }

    /// A group MMS's receipt at one recipient: `message` its own id, `group` the group's.
    fn group_receipt(&self, message: &str, group: &str, to: &str, status: &str) -> WebhookResponse {
        let n = self.next();
        let mut payload = json!({"id": message, "group_message_id": group, "direction": "outbound",
            "to": [{"phone_number": to, "status": status}]});
        if status.ends_with("failed") {
            payload["errors"] = json!([{"code": "40002", "title": "Blocked", "detail": "The carrier refused it"}]);
        }
        let event = json!({"data": {"event_type": "message.finalized", "id": format!("ev-gr{n}"),
            "occurred_at": "2026-11-01T12:00:05.000+00:00", "payload": payload}});
        self.hook(&self.signed(&event, now() / 1000))
    }

    /// The day's count of `key` (SmsSendCount sentCount), 0 when none.
    fn sent_count(&self, key: &str) -> i64 {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.call.SmsSendCount")
            .unwrap()
            .eq("counterKey", text(key))
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| {
                row.get("sentCount")
                    .map(to_plain_string)
                    .unwrap_or_default()
                    .parse::<i64>()
                    .unwrap_or(0)
            })
            .sum()
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
            "org.moqui.chat.ChatServices.send#Message",
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
            "org.moqui.chat.ChatBridgeServices.run#BridgeDeliveries",
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

/// The bridge's settings as the bed stored them.
fn bridge_config(bed: &Bed) -> Json {
    (bed.provider.config)(&bed.fake.base, &bed.keys)
}

fn bridge_info(bed: &Bed) -> moqui_core::service::chat_bridge::BridgeInfo {
    moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtSms".to_string(),
        adapter_name: bed.provider.adapter_name.to_string(),
        name: bed.provider.bridge_name.to_string(),
        config: bridge_config(bed),
    }
}

/// The Telnyx adapter, as `lib.rs` registers it.
fn telnyx() -> crate::sms::SmsAdapter<crate::providers::telnyx::Telnyx> {
    crate::sms::SmsAdapter::new(crate::providers::telnyx::Telnyx)
}

/// SMS is never end-to-end encrypted, not even in transit past Telnyx: the room shows the SMS label (Comms-Messaging
/// §6.3); and a text carries no edit, delete or reaction.
#[test]
fn a_text_says_it_is_not_encrypted() {
    use moqui_core::service::chat_bridge::{ChatBridgeAdapter as _, Encryption};
    let bridge = moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtSms".to_string(),
        adapter_name: "sms".to_string(),
        name: "Telnyx".to_string(),
        config: json!({"apiKey": "KEY-1"}),
    };
    let caps = telnyx().capabilities(&bridge);
    assert_eq!(caps.encryption, Encryption::None);
    assert_eq!(caps.encryption.as_str(), "None");
    assert!(!caps.edit && !caps.delete && !caps.reaction);
    assert_eq!(caps.group_max, 8, "Telnyx's group MMS");
}

#[test]
fn an_unknown_or_disabled_bridge_answers_anyone_but_its_network_as_an_unsigned_request() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let n = bed.next();
    let event = bed.received(&format!("ev-{n}"), "in-probe", THEIRS, "Hello", json!([]));
    let signed = bed.signed(&event, now() / 1000);
    let mut unsigned = signed.clone();
    unsigned.headers.retain(|(name, _)| name != "telnyx-signature-ed25519");
    let hook_to = |bridge: &str, request: &WebhookRequest| {
        let context = bed.ctx.factory().new_context();
        let _entered = context.enter();
        webhook(bed.ctx.factory(), bridge, request)
    };
    // the same request, unsigned, to the bridge and to ids that are no bridge's: one answer
    let known = hook_to(BRIDGE, &unsigned);
    assert_eq!(known, WebhookResponse::not_signed());
    assert_eq!(hook_to("NO_SUCH_BRIDGE", &unsigned), known);
    assert_eq!(hook_to("NO_SUCH_BRIDGE", &signed), known);
    // disabled: anyone but its network gets the same; the network, by its signature, 503 (it tries again later)
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
        row.set("statusId", text("CbsDisabled"));
        row.update(&bed.facade).unwrap();
    }
    assert_eq!(hook_to(BRIDGE, &unsigned), known);
    let disabled = hook_to(BRIDGE, &signed);
    assert_eq!(status(&disabled), 503);
    assert!(!disabled.auth_failure);
    assert!(bed.room(THEIRS).is_none(), "nothing was taken while disabled");
}

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

    // the same request again is a replay: answered as done, not worked again, and not counted against the provider's
    // address (its signature is good)
    let replay = bed.hook(&request);
    assert_eq!(status(&replay), 200);
    assert_eq!(body_json(&replay)["outcome"], "duplicate");
    assert!(!replay.auth_failure);
    // the signature is held in the replay table the system-message HMAC check uses, not in this adapter, so a copy
    // that reaches another node (another adapter, the same table) is a replay there too (SEC-DAT-90)
    let worked = {
        let _authz = bed.ctx.disable_authz();
        bed.ctx
            .factory()
            .entity()
            .unwrap()
            .find("moqui.security.SignatureReplay")
            .unwrap()
            .eq("worked", Value::string("Y"))
            .use_cache(false)
            .count()
            .unwrap()
    };
    assert!(worked >= 1, "the worked webhook's key is kept");
    {
        use moqui_core::service::chat_bridge::ChatBridgeAdapter as _;
        let context = bed.ctx.factory().new_context();
        let _entered = context.enter();
        let on_another_node = telnyx().webhook(&bridge_info(&bed), &request);
        assert_eq!(body_json(&on_another_node)["outcome"], "duplicate");
    }
    // a timestamp past what a time can be is refused, not a panic
    let mut huge = request.clone();
    for (name, value) in &mut huge.headers {
        if name == "telnyx-timestamp" {
            *value = i64::MAX.to_string();
        }
    }
    let refused_huge = bed.hook(&huge);
    assert_eq!(refused_huge, WebhookResponse::not_signed());
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
    let stale = bed.signed(&event, now() / 1000 - 600);
    // each refusal the same answer, whatever was wrong
    for bad in [&tampered, &forged, &unsigned, &stale] {
        assert_eq!(bed.hook(bad), WebhookResponse::not_signed());
    }
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
        "org.moqui.chat.ChatBridgeServices.get#EventDeliveries",
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

    // a file over the bridge's inbound bound (maxInboundMediaBytes) is not read past it, whatever bound the caller asks
    let large = bed.fake.file("large.jpg", "image/jpeg", &[7_u8; 3000]);
    use moqui_core::service::chat_bridge::ChatBridgeAdapter as _;
    let adapter = telnyx();
    let bridge = moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtSms".to_string(),
        adapter_name: "sms".to_string(),
        name: "Telnyx".to_string(),
        config: json!({"apiKey": "KEY-1", "allowInternalAddresses": true, "maxInboundMediaBytes": 2000}),
    };
    let refused = adapter.fetch_media(&bridge, &large, u64::MAX).unwrap_err();
    assert_eq!(refused, "the file is over 2000 bytes");
    assert_eq!(
        adapter.fetch_media(&bridge, &picture, u64::MAX).unwrap().bytes,
        vec![1, 2, 3, 4, 5]
    );

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

    // over the bridge's limits (3 files, 2000 bytes in all): the files refused before the provider, the words sent
    // alone in their place (plans/Comms-Messaging.md §18.3 phase 20)
    let many: Vec<String> = (0..4).map(|i| bed.upload(&room, &format!("f{i}.png"), 10)).collect();
    let too_many = bed.send(&room, "Many", &many);
    let large = bed.upload(&room, "big.png", 2500);
    let too_large = bed.send(&room, "Big", &[large]);
    bed.round(0);
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 3);
    assert!(
        sends[1..].iter().all(|seen| !seen.body.contains("media")),
        "the words alone"
    );
    assert_eq!(bed.delivery(&room, too_many).0, "CdsAccepted");
    assert_eq!(bed.delivery(&room, too_large).0, "CdsAccepted");
    // a message of files with no words stays refused
    let silent = bed.send(&room, "", &[bed.upload(&room, "big2.png", 2500)]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, silent).1, "mediaTooLarge");
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
    // and the pair's STOP says so too, so the next text is refused before the provider; their START opts back in
    assert_eq!(
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))]
        )["optedOut"],
        "Y"
    );
    // a START sent after that refusal (its time the provider's, later than the opt-out it undoes: SEC-COM-90)
    bed.text_in_at(THEIRS, "START", &jiff::Timestamp::now().to_string());

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
    // the day's count holds what reached the provider (the uncertain one and the one taken after its 503), not the
    // refused, the retried or the unreachable attempts
    let today = &jiff::Timestamp::now().to_string()[..10];
    assert_eq!(
        bed.one(
            "moqui.call.SmsSendCount",
            &[("counterKey", text("user:alice")), ("dayDate", text(today))]
        )["sentCount"],
        "2"
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
        "org.moqui.call.SmsServices.open#PairRoom",
        &[("toNumber", text("+442071234567")), ("fromNumber", text(OURS))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "internationalNotEnabled");
    errors();
    // carol may text from no number; the same parameters as alice's
    login("carol");
    let out = call(
        "org.moqui.call.SmsServices.open#PairRoom",
        &[("toNumber", text("+15552223333")), ("fromNumber", text(OURS))],
    );
    assert_eq!(out_text(&out, "reasonCode"), "noMessaging");
    errors();
    login("alice");
    let opened = ok(
        "org.moqui.call.SmsServices.open#PairRoom",
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

    // the people's texts at their day's limit in all (smsMaxPerDay) do not hold back the system's own: sign-in codes
    // have a budget of their own (smsSystemMaxPerDay)
    let set_policy = |field: &str, value: i64| {
        let _authz = bed.ctx.disable_authz();
        let mut policy = bed
            .facade
            .find("moqui.call.VoicePolicy")
            .unwrap()
            .eq("voicePolicyId", text("DEFAULT"))
            .one()
            .unwrap()
            .unwrap();
        policy.set(field, Value::I64(value));
        policy.update(&bed.facade).unwrap();
    };
    set_policy("smsMaxPerDay", 1);
    let day = jiff::Timestamp::now().strftime("%Y-%m-%d").to_string();
    {
        let _authz = bed.ctx.disable_authz();
        let found = bed
            .facade
            .find("moqui.call.SmsSendCount")
            .unwrap()
            .eq("counterKey", text("all"))
            .eq("dayDate", text(&day))
            .one()
            .unwrap();
        let mut count = match found {
            Some(count) => count,
            None => {
                let mut count = bed.facade.make_value("moqui.call.SmsSendCount").unwrap();
                count.set("counterKey", text("all"));
                count.set("dayDate", text(&day));
                count.set("sentCount", Value::I64(0));
                count.create(&bed.facade).unwrap();
                count
            }
        };
        count.set("sentCount", Value::I64(1));
        count.update(&bed.facade).unwrap();
    }
    assert_eq!(system(THEIRS, false).0, "true");
    assert_eq!(bed.fake.sends().len(), 3);
    assert_eq!(
        bed.one(
            "moqui.call.SmsSendCount",
            &[("counterKey", text("all")), ("dayDate", text(&day))]
        )["sentCount"],
        "1",
        "the system's texts are not the people's"
    );
    // the system's own day's limit
    set_policy("smsSystemMaxPerDay", 2);
    assert_eq!(system(THEIRS, false), ("false".to_string(), "dailyLimit".to_string()));
    assert_eq!(bed.fake.sends().len(), 3);
}

/// The sms-consent provider a component's conf registers (as Mantle does): yes for THEIRS, no for NO_CONSENT, nothing
/// for another number (read from ec.context: toNumber is also the scripts' global number helper). Its service needs a user, so it answers for the system only because it runs with authorization
/// disabled.
const CONSENT_SERVICES: &str = r#"<services>
    <service verb="check" noun="SmsConsent">
        <in-parameters><parameter name="fromNumber" required="true"/><parameter name="toNumber" required="true"/></in-parameters>
        <out-parameters><parameter name="consent" type="Boolean"/></out-parameters>
        <actions><script>const to = String(ec.context.toNumber); consent = to === '+15557778888' || to === '+15556667777' ? true : to === '+15552223333' ? false : null</script></actions>
    </service>
    <service verb="check" noun="SmsConsentDown">
        <in-parameters><parameter name="fromNumber" required="true"/><parameter name="toNumber" required="true"/></in-parameters>
        <out-parameters><parameter name="consent" type="Boolean"/></out-parameters>
        <actions><script>if (String(ec.context.toNumber) === '+15556667777') throw new Error('the consent records are down'); consent = null</script></actions>
    </service>
</services>"#;
const NO_CONSENT: &str = "+15552223333";
/// A number the first conf provider says yes to and the second fails on.
const PROVIDER_DOWN: &str = "+15556667777";

#[test]
fn a_campaign_text_is_asked_of_the_conf_provider_and_counts_for_its_number() {
    let dir = std::env::temp_dir().join(format!("moqui-sms-consent-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("service/test")).unwrap();
    let services = dir.join("service/test/Consent.xml");
    std::fs::write(&services, CONSENT_SERVICES).unwrap();
    let bed = bed_with(
        r#"<moqui-conf><screen-facade><shell-providers>
            <sms-consent name="test.consent" service="test.Consent.check#SmsConsent"/>
            <sms-consent name="test.down" service="test.Consent.check#SmsConsentDown" sequence="5"/>
            <inbox-source name="test.inbox" service="test.Consent.check#SmsConsent"/>
        </shell-providers></screen-facade></moqui-conf>"#,
    );
    let _entered = bed.ctx.enter();
    let failures = moqui_core::service::lint(std::slice::from_ref(&services)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    // a campaign's text: the system's, with no user signed in
    let promotion = |to: &str| {
        let out = call(
            "org.moqui.SmsServices.send#SmsMessage",
            &[
                ("contactNumber", text(to)),
                ("message", text("Sale today")),
                ("isPromotional", Value::Bool(true)),
            ],
        );
        errors();
        (
            out_text(&out, "successful"),
            out_text(&out, "reasonCode"),
            out_text(&out, "deliveryOutcome"),
        )
    };
    let no = |reason: &str, outcome: &str| ("false".to_string(), reason.to_string(), outcome.to_string());
    assert_eq!(promotion(THEIRS), ("true".to_string(), String::new(), String::new()));
    // a no from the provider, and a number it says nothing of, are refused before the provider
    assert_eq!(promotion(NO_CONSENT), no("consentRequired", "permanent"));
    assert_eq!(promotion("+15554445555"), no("consentRequired", "permanent"));
    // a provider that fails may have said no: a yes from another does not let the text go
    assert_eq!(promotion(PROVIDER_DOWN), no("consentRequired", "permanent"));
    assert_eq!(bed.fake.sends().len(), 1);
    // for a ledger of the caller's (Mantle's): an answer lost after the request went is uncertain, never to be sent again
    // on its own, and counts; the provider away (503) is retryable, and its place in the day's count goes back
    bed.fake.script(&[Reply::Drop]);
    assert_eq!(promotion(THEIRS).2, "uncertain");
    bed.fake.script(&[Reply::Status(
        503,
        r#"{"errors":[{"code":"10007","title":"Unavailable"}]}"#.to_string(),
    )]);
    assert_eq!(promotion(THEIRS).2, "retryable");
    let sends_before = bed.fake.sends().len();
    // only sms-consent is answerable from the conf: its inbox-source is left out (internal work, as Sms.js asks)
    let authz = bed.ctx.disable_authz();
    let inbox = ok(
        "org.moqui.ui.ShellServices.call#Providers",
        &[
            ("hook", text("inbox-source")),
            ("parameters", Value::Map(Default::default())),
        ],
    );
    assert_eq!(out_text(&inbox, "unavailable"), "true", "{inbox:?}");
    let listed = ok(
        "org.moqui.ui.ShellServices.get#Providers",
        &[("hook", text("sms-consent"))],
    );
    assert!(
        to_plain_string(&listed["providerList"]).contains("test.consent"),
        "{listed:?}"
    );
    drop(authz);

    // the number's day of promotional texts: the campaign's count there, not against the system's own budget
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
        policy.set("smsMaxPromotionalPerNumber", Value::I64(3));
        policy.set("smsSystemMaxPerDay", Value::I64(1));
        policy.update(&bed.facade).unwrap();
    }
    assert_eq!(promotion(THEIRS).0, "true");
    // a ledger tries it again later (send#SmsMessage's deliveryOutcome)
    assert_eq!(promotion(THEIRS), no("dailyLimit", "retryable"));
    let day = jiff::Timestamp::now().strftime("%Y-%m-%d").to_string();
    assert_eq!(
        bed.one(
            "moqui.call.SmsSendCount",
            &[("counterKey", text(&format!("promo:{OURS}"))), ("dayDate", text(&day))]
        )["sentCount"],
        "3",
        "the taken, the uncertain and the last taken; not the retried or the refused"
    );
    // a sign-in code still goes: the system's budget (1) was not spent by the campaign
    let code = call(
        "org.moqui.SmsServices.send#SmsMessage",
        &[
            ("contactNumber", text(THEIRS)),
            ("message", text("Your code is 123456")),
        ],
    );
    assert_eq!(out_text(&code, "successful"), "true", "{}", errors());
    assert_eq!(bed.fake.sends().len(), sends_before + 2);
    let _ = std::fs::remove_dir_all(&dir);
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
            "org.moqui.call.VoiceServices.receive#PhoneCall",
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
        "org.moqui.call.CallServices.get#Call",
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

#[test]
fn the_pair_room_keeps_only_the_users_the_number_reaches() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.text_in(THEIRS, "Hello", json!([]));
    let room = bed.room(THEIRS).unwrap();
    let member = |user: &str| {
        bed.count(
            "moqui.chat.ChatRoomMember",
            &[("chatRoomId", text(&room)), ("userId", text(user))],
        )
    };
    assert_eq!((member("alice"), member("bob")), (1, 1));

    // a member may not add someone the number does not reach; the same parameters as for a room of their own
    login("alice");
    call(
        "org.moqui.chat.ChatServices.add#Member",
        &[("chatRoomId", text(&room)), ("userId", text("carol"))],
    );
    assert!(errors().contains("members its number reaches"));
    assert_eq!(member("carol"), 0);

    // bob's grant ends: he is out of the pair's room, and does not see its next text
    ec_revoke(&bed, "bob");
    assert_eq!((member("alice"), member("bob")), (1, 0));
    bed.text_in(THEIRS, "Still there?", json!([]));
    assert_eq!(
        member("bob"),
        0,
        "a text does not bring back a user the number no longer reaches"
    );
    // granted again, he is back at the pair's next text
    row(
        &bed.facade,
        "moqui.call.PhoneNumberUser",
        &[
            ("phoneNumber", text(OURS)),
            ("userId", text("bob")),
            ("fromDate", Value::I64(now() - 1_000)),
        ],
    );
    bed.text_in(THEIRS, "Hello again", json!([]));
    assert_eq!(member("bob"), 1);

    // a caller with authorization on cannot say a batch's transport was checked
    login("alice");
    let out = call(
        "org.moqui.chat.ChatBridgeServices.ingest#Event",
        &[
            ("chatBridgeId", text(BRIDGE)),
            ("transactionId", text("forged-1")),
            ("transportVerified", Value::Bool(true)),
            (
                "itemList",
                Value::List(vec![moqui_lib::json::from_json(
                    &json!({"itemId": "1", "type": "message",
                    "externalRoomId": format!("{OURS}/{THEIRS}"), "externalEventId": "forged-ev",
                    "externalUserId": THEIRS, "text": "Forged"}),
                )]),
            ),
        ],
    );
    let refused = errors();
    assert!(refused.contains("token does not match"), "{out:?} {refused}");
    assert!(!bed.messages(&room).iter().any(|(_, body)| body == "Forged"));
}

/// Revoke `user`'s grant of our number as a voice administrator does (VoiceServices.revoke#CallerId).
fn ec_revoke(bed: &Bed, user: &str) {
    for (entity, fields) in [
        (
            "moqui.security.UserAccount",
            vec![("userId", text("voiceadmin")), ("username", text("voiceadmin"))],
        ),
        ("moqui.security.UserGroup", vec![("userGroupId", text("VADMIN"))]),
        (
            "moqui.security.UserGroupMember",
            vec![
                ("userGroupId", text("VADMIN")),
                ("userId", text("voiceadmin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
        (
            "moqui.security.UserGroupPermission",
            vec![
                ("userGroupId", text("VADMIN")),
                ("userPermissionId", text("VoiceAdmin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
    ] {
        row(&bed.facade, entity, &fields);
    }
    login("voiceadmin");
    let out = ok(
        "org.moqui.call.VoiceServices.revoke#CallerId",
        &[("phoneNumber", text(OURS)), ("userId", text(user))],
    );
    assert_eq!(out_text(&out, "revokedCount"), "1", "{out:?}");
}

#[test]
fn an_edge_node_keys_a_webhook_by_its_event_and_their_number() {
    use crate::sms::SmsProvider as _;
    let event_key = |request: &WebhookRequest| crate::providers::telnyx::Telnyx.event_key(request);
    let request = |body: Json| WebhookRequest {
        method: "POST".to_string(),
        url: format!("/channels/{BRIDGE}/webhook"),
        headers: vec![("telnyx-signature-ed25519".to_string(), "c2lnbmF0dXJl".to_string())],
        body: body.to_string().into_bytes(),
        client_ip: "198.51.100.7".to_string(),
        now_millis: now(),
    };
    let text = json!({"data": {"event_type": "message.received", "id": "ev-9",
        "payload": {"from": {"phone_number": THEIRS}, "to": [{"phone_number": OURS}]}}});
    let key = event_key(&request(text)).unwrap();
    assert_eq!(key.event_id, "ev-9");
    assert_eq!(key.conversation.as_deref(), Some(THEIRS));
    // a receipt: ordered by the number it went to
    let receipt = json!({"data": {"event_type": "message.finalized", "id": "ev-10",
        "payload": {"to": [{"phone_number": THEIRS, "status": "delivered"}]}}});
    let key = event_key(&request(receipt)).unwrap();
    assert_eq!(
        (key.event_id.as_str(), key.conversation.as_deref()),
        ("ev-10", Some(THEIRS))
    );
    // no event id: a hash of the body, the same on the provider's retry (whose signature differs); not JSON: none
    let key = event_key(&request(json!({"data": {}}))).unwrap();
    assert!(
        key.event_id.starts_with("sha256:") && key.event_id.len() == 71,
        "{}",
        key.event_id
    );
    assert_eq!(key.conversation, None);
    let mut retried = request(json!({"data": {}}));
    retried.headers = vec![("telnyx-signature-ed25519".to_string(), "b3RoZXI=".to_string())];
    assert_eq!(event_key(&retried).unwrap().event_id, key.event_id);
    let mut bad = request(json!({}));
    bad.body = b"not json".to_vec();
    assert!(event_key(&bad).is_none());
}

/// SEC-COM-10: the API base is https, unless `allowInternalAddresses` (a test's fake) allows http; that setting is
/// ignored on a production instance.
#[test]
fn a_bridge_sends_its_key_over_https_and_reaches_no_internal_address_on_production() {
    use moqui_core::service::chat_bridge::ChatBridgeAdapter as _;
    let bridge = |config: Json| moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: "B".to_string(),
        type_enum_id: String::new(),
        adapter_name: "sms".to_string(),
        name: "B".to_string(),
        config,
    };
    let adapter = telnyx();
    let plain = json!({"apiKey": "K", "apiBaseUrl": "http://api.example.test"});
    assert!(adapter.start(&bridge(plain)).is_err());
    let fake = json!({"apiKey": "K", "apiBaseUrl": "http://127.0.0.1:9", "allowInternalAddresses": true});
    assert!(adapter.start(&bridge(fake.clone())).is_ok());
    let settings = |config: &Json| adapter.settings(&bridge(config.clone()));
    assert!(settings(&fake).allow_internal);
    moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(Some(true)));
    let on_production = (settings(&fake).allow_internal, adapter.start(&bridge(fake)).is_err());
    moqui_core::web::TEST_PRODUCTION.with(|flag| flag.set(None));
    assert_eq!(on_production, (false, true));
}

/// SEC-COM-88: every provider call is checked where it is sent, so a bridge whose start failed, or a direct send that
/// never starts it, does not post its key to an http address; a test's fake (a client that allows internal
/// addresses) still takes http.
#[test]
fn a_provider_call_is_never_sent_in_clear_text_whatever_started() {
    use crate::sms::{call, ApiBody, ApiRequest};
    let request = |url: &str| ApiRequest {
        method: moqui_lib::http::Method::Post,
        url: url.to_string(),
        headers: vec![("authorization".to_string(), "Bearer K".to_string())],
        body: ApiBody::None,
    };
    let public = moqui_lib::http::RestClient::new().deny_internal_addresses(true);
    for url in [
        "http://api.example.test/v2/messages",
        "ftp://api.example.test/x",
        "api.example.test",
    ] {
        let refused = call(&public, &request(url)).unwrap_err();
        assert!(refused.message().contains("not an https address"), "{url}: {refused}");
    }
    let fake = moqui_lib::http::RestClient::new()
        .deny_internal_addresses(false)
        .connect_timeout(Duration::from_millis(200));
    let unreached = call(&fake, &request("http://127.0.0.1:9/v2/messages")).unwrap_err();
    assert!(!unreached.message().contains("not an https address"), "{unreached}");
}

/// plans/Comms-Messaging.md §9, group texts: a group MMS that comes in (our number among its `to` and `cc`) is one room
/// for that set of outside numbers, the same room whoever writes and in whatever order; a reply goes out as one group
/// MMS to every number; Text a number with several numbers opens that room; a group with a number a text to would be
/// refused is refused whole, before the provider, and not counted.
#[test]
fn a_group_text_is_one_room_for_its_numbers_and_goes_out_as_one_group_mms() {
    const OTHER: &str = "+15553334444";
    let bed = bed();
    let _entered = bed.ctx.enter();
    let group_in = |from: &str, to: &[&str], cc: &[&str], body: &str| bed.group_in(from, to, cc, body);
    // from THEIRS to us and OTHER; then from OTHER, our number not first: the same room
    let first = group_in(THEIRS, &[OURS], &[OTHER], "Dinner at eight?");
    assert_eq!(status(&first), 200, "{}", String::from_utf8_lossy(&first.body));
    assert_eq!(body_json(&first)["outcome"], "accepted");
    let mut sorted = [THEIRS, OTHER];
    sorted.sort_unstable();
    let key = sorted.join(",");
    let room = bed.room(&key).expect("the group's room");
    let second = group_in(OTHER, &[THEIRS, OURS], &[], "Yes");
    assert_eq!(body_json(&second)["outcome"], "accepted");
    assert_eq!(bed.room(&key).as_deref(), Some(room.as_str()));
    assert_eq!(bed.messages(&room).len(), 2);
    assert!(bed.room(THEIRS).is_none(), "no pair room of the sender");
    let row = bed.one("moqui.chat.ChatRoom", &[("chatRoomId", text(&room))]);
    assert_eq!(row["roomTypeEnumId"], "CrtExternal");
    assert!(row["roomName"].contains(THEIRS) && row["roomName"].contains(OTHER));
    // its two outside people are two actors
    let senders: std::collections::BTreeSet<String> = bed.messages(&room).into_iter().map(|(actor, _)| actor).collect();
    assert_eq!(senders.len(), 2);

    // a reply: one group MMS to both
    let seq = bed.send(&room, "Eight it is", &[]);
    bed.round(0);
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].path, "/v2/messages/group_mms");
    let sent: Json = serde_json::from_str(&sends[0].body).unwrap();
    assert_eq!(sent["from"], OURS);
    assert_eq!(sent["to"], json!(sorted));
    assert_eq!(sent["text"], "Eight it is");
    // the group endpoint takes no messaging profile (CreateGroupMMSMessageRequest has none)
    assert!(sent.get("messaging_profile_id").is_none(), "{sent}");
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(
        bed.sent_count("user:alice"),
        2,
        "counted once a recipient, as the carrier bills it"
    );

    // Text a number with both numbers: that room
    login("alice");
    let out = ok(
        "org.moqui.call.SmsServices.open#PairRoom",
        &[
            ("toNumber", text(&format!("{OTHER}, {THEIRS}"))),
            ("fromNumber", text(OURS)),
        ],
    );
    assert_eq!(out_text(&out, "chatRoomId"), room);
    // a group with an international number: refused whole at open, and a send to such a group refused before the
    // provider and not counted
    let refused = call(
        "org.moqui.call.SmsServices.open#PairRoom",
        &[
            ("toNumber", text(&format!("{THEIRS},+442071234567"))),
            ("fromNumber", text(OURS)),
        ],
    );
    assert_eq!(out_text(&refused, "reasonCode"), "internationalNotEnabled");
    errors();
    let abroad = ok(
        "org.moqui.call.SmsServices.ensure#PairRoom",
        &[
            ("ourNumber", text(OURS)),
            (
                "theirNumberList",
                Value::List(vec![text(THEIRS), text("+442071234567")]),
            ),
        ],
    );
    let abroad_room = out_text(&abroad, "chatRoomId");
    let blocked = bed.send(&abroad_room, "Hello", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&abroad_room, blocked).1, "internationalNotEnabled");
    assert_eq!(bed.fake.sends().len(), 1, "nothing more reached the provider");
    assert_eq!(bed.sent_count("user:alice"), 2, "not counted");
    // more than eight others: refused
    let many: Vec<Value> = (0..9).map(|i| text(&format!("+1555000{:04}", 2000 + i))).collect();
    let too_many = call(
        "org.moqui.call.SmsServices.ensure#PairRoom",
        &[("ourNumber", text(OURS)), ("theirNumberList", Value::List(many))],
    );
    assert_eq!(out_text(&too_many, "reasonCode"), "tooManyNumbers");
    errors();
}

/// A group text's receipts come one a recipient, named by the group's message: each is kept, one failed recipient does
/// not fail the delivery while another got it, and the delivery fails only when every recipient failed; a STOP from one
/// member refuses the whole send before the provider.
#[test]
fn a_group_texts_receipts_are_each_recipients_and_one_stop_refuses_the_whole_send() {
    const OTHER: &str = "+15553334444";
    let bed = bed();
    let _entered = bed.ctx.enter();
    assert_eq!(
        body_json(&bed.group_in(THEIRS, &[OURS], &[OTHER], "Hi all"))["outcome"],
        "accepted"
    );
    let mut sorted = [THEIRS, OTHER];
    sorted.sort_unstable();
    let room = bed.room(&sorted.join(",")).expect("the group's room");

    // one recipient delivered, the other failed: delivered to the one, the delivery stands
    let seq = bed.send(&room, "Lunch?", &[]);
    bed.round(0);
    let remote = bed.delivery(&room, seq).3;
    assert!(!remote.is_empty());
    assert_eq!(status(&bed.group_receipt("r-1", &remote, THEIRS, "delivered")), 200);
    assert_eq!(
        body_json(&bed.group_receipt("r-2", &remote, OTHER, "delivery_failed"))["found"],
        true
    );
    let (state, _, _, _, receipt) = bed.delivery(&room, seq);
    assert_eq!((state.as_str(), receipt.as_str()), ("CdsAccepted", "delivered"));
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(seq))],
    );
    let recipients: Json = serde_json::from_str(&row["recipientReceipts"]).unwrap();
    assert_eq!(recipients[THEIRS], "delivered");
    assert_eq!(recipients[OTHER], "failed");

    // both failed: the delivery fails
    let seq = bed.send(&room, "Dinner?", &[]);
    bed.round(0);
    let remote = bed.delivery(&room, seq).3;
    bed.group_receipt("r-3", &remote, THEIRS, "delivery_failed");
    assert_eq!(
        bed.delivery(&room, seq).0,
        "CdsAccepted",
        "one of two failed: not failed yet"
    );
    bed.group_receipt("r-4", &remote, OTHER, "sending_failed");
    let (state, reason, _, _, _) = bed.delivery(&room, seq);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "deliveryFailed"));

    // a STOP from one of them: the next group send is refused whole, before the provider, and not counted
    let (stopped, _) = bed.text_in(OTHER, "STOP", json!([]));
    assert_eq!(status(&stopped), 200);
    let sends = bed.fake.sends().len();
    let counted = bed.sent_count("user:alice");
    let blocked = bed.send(&room, "Anyone?", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&room, blocked).1, "optedOut");
    assert_eq!(bed.fake.sends().len(), sends, "nothing reached the provider");
    assert_eq!(bed.sent_count("user:alice"), counted, "not counted");
}

/// Who a group text is with: the sender is kept whatever is cut (then seven others), our own numbers are never its
/// others, a short code keeps a room of its own, and past five new groups a day from one number a group text goes to
/// the sender's pair room, said so.
#[test]
fn a_group_texts_people_keep_the_sender_leave_out_ours_and_new_groups_are_limited() {
    const OURS2: &str = "+15550001001";
    const OTHER: &str = "+15553334444";
    let bed = bed();
    let _entered = bed.ctx.enter();
    // a second number of ours, on no bridge of this test
    row(
        &bed.facade,
        "moqui.call.PhoneNumber",
        &[
            ("phoneNumber", text(OURS2)),
            ("routeTypeEnumId", text("PnrUser")),
            ("routeId", text("bob")),
        ],
    );

    // twelve recipients, the sender last in order: the sender and the first seven others
    const SENDER: &str = "+15559999999";
    let many: Vec<String> = (0..12).map(|i| format!("+1555000{:04}", 3000 + i)).collect();
    let many_refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        body_json(&bed.group_in(SENDER, &[OURS], &many_refs, "Big group"))["outcome"],
        "accepted"
    );
    let mut kept: Vec<&str> = many_refs[..7].to_vec();
    kept.push(SENDER);
    kept.sort_unstable();
    assert!(
        bed.room(&kept.join(",")).is_some(),
        "the sender kept, with seven others"
    );

    // two of our numbers in one group: both ours, never one of its others
    assert_eq!(
        body_json(&bed.group_in(THEIRS, &[OURS, OURS2], &[OTHER], "Ours twice"))["outcome"],
        "accepted"
    );
    let mut pair = [THEIRS, OTHER];
    pair.sort_unstable();
    assert!(bed.room(&pair.join(",")).is_some());
    // only ours besides the sender: the sender's pair room
    assert_eq!(
        body_json(&bed.group_in(THEIRS, &[OURS, OURS2], &[], "Just us"))["outcome"],
        "accepted"
    );
    assert!(bed.room(THEIRS).is_some());
    login("alice");
    let own = ok(
        "org.moqui.call.SmsServices.ensure#PairRoom",
        &[
            ("ourNumber", text(OURS)),
            (
                "theirNumberList",
                Value::List(vec![text(THEIRS), text(OTHER), text(OURS2)]),
            ),
        ],
    );
    assert_eq!(
        Some(out_text(&own, "chatRoomId")),
        bed.room(&pair.join(",")),
        "a send never goes to our own number"
    );

    // a short code with others: its own room, never another person's
    assert_eq!(
        body_json(&bed.group_in("22333", &[OURS], &[OTHER], "Your code"))["outcome"],
        "accepted"
    );
    assert!(bed.room("22333").is_some(), "the short code's own room");
    assert!(bed.room(OTHER).is_none(), "not the other person's pair room");

    // five new groups from one number a day; the sixth goes to its pair room, said so
    const FLOOD: &str = "+15556667777";
    for i in 0..5 {
        let other = format!("+1555444{:04}", i);
        assert_eq!(
            body_json(&bed.group_in(FLOOD, &[OURS], &[&other], "Join"))["outcome"],
            "accepted"
        );
    }
    assert!(bed.room(FLOOD).is_none(), "no pair room yet");
    assert_eq!(
        body_json(&bed.group_in(FLOOD, &[OURS], &["+15554449999"], "Join again"))["outcome"],
        "accepted"
    );
    let pair_room = bed.room(FLOOD).expect("the sender's pair room");
    let texts = bed.messages(&pair_room);
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].1.contains("more new groups came from this number today") && texts[0].1.contains("Join again"),
        "{texts:?}"
    );
    let mut sixth = [FLOOD, "+15554449999"];
    sixth.sort_unstable();
    assert!(bed.room(&sixth.join(",")).is_none(), "no sixth group room");
    // a group already there still takes its texts
    let first = format!("+1555444{:04}", 0);
    assert_eq!(
        body_json(&bed.group_in(FLOOD, &[OURS], &[&first], "Still here"))["outcome"],
        "accepted"
    );
    assert_eq!(bed.messages(&pair_room).len(), 1);
}

/// Telnyx's API and webhooks under another adapter, with its own group maximum and events of its own: an opt-out
/// signal and a text in a conversation of the provider's, as Twilio and Sinch send.
#[derive(Clone, Copy)]
struct TestTelnyx {
    adapter: &'static str,
    group_max: usize,
}

/// No group texts: a provider such as Sinch's Conversation API.
const NO_GROUPS: TestTelnyx = TestTelnyx {
    adapter: "sms-nogroups",
    group_max: 0,
};
/// Group texts as conversations of the provider's (Twilio Conversations).
const THREADS: TestTelnyx = TestTelnyx {
    adapter: "sms-threads",
    group_max: 10,
};

impl crate::sms::SmsProvider for TestTelnyx {
    type Config = crate::providers::telnyx::Config;

    fn name(&self) -> &'static str {
        "Test provider"
    }

    fn adapter_name(&self) -> &'static str {
        self.adapter
    }

    fn defaults(&self) -> crate::sms::Defaults {
        crate::providers::telnyx::Telnyx.defaults()
    }

    fn config(&self, config: &Json) -> Self::Config {
        crate::providers::telnyx::Telnyx.config(config)
    }

    fn check(&self, config: &Self::Config) -> Result<(), String> {
        crate::providers::telnyx::Telnyx.check(config)
    }

    fn group_max(&self) -> usize {
        self.group_max
    }

    fn request(
        &self,
        config: &Self::Config,
        settings: &crate::sms::Settings,
        out: &crate::sms::Outgoing<'_>,
    ) -> crate::sms::ApiRequest {
        crate::providers::telnyx::Telnyx.request(config, settings, out)
    }

    fn accepted_id(&self, out: &crate::sms::Outgoing<'_>, body: &str) -> Option<String> {
        crate::providers::telnyx::Telnyx.accepted_id(out, body)
    }

    fn error_of(&self, body: &str) -> crate::sms::ProviderError {
        crate::providers::telnyx::Telnyx.error_of(body)
    }

    fn refusal(&self, status: u16, error: &crate::sms::ProviderError) -> crate::sms::Refusal {
        crate::providers::telnyx::Telnyx.refusal(status, error)
    }

    fn signature_header(&self) -> &'static str {
        crate::providers::telnyx::Telnyx.signature_header()
    }

    fn verifies_on_edge(&self) -> bool {
        true
    }

    fn verify(
        &self,
        bridge: &moqui_core::service::chat_bridge::BridgeInfo,
        config: &Self::Config,
        settings: &crate::sms::Settings,
        request: &WebhookRequest,
    ) -> bool {
        crate::providers::telnyx::Telnyx.verify(bridge, config, settings, request)
    }

    fn event_key(&self, request: &WebhookRequest) -> Option<moqui_core::service::chat_bridge::WebhookEventKey> {
        crate::providers::telnyx::Telnyx.event_key(request)
    }

    /// Telnyx's events, and an opt-out signal of its own shape (`test.opt_signal`), as a provider that tells of a STOP
    /// it took itself sends one.
    fn parse(&self, request: &WebhookRequest) -> Result<crate::sms::SmsEvent, WebhookResponse> {
        let event: Json = serde_json::from_slice(&request.body).unwrap_or(Json::Null);
        let field = |pointer: &str| {
            event
                .pointer(pointer)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        match event.pointer("/data/event_type").and_then(Json::as_str) {
            Some("test.opt_signal") => {}
            Some("test.thread_text") => {
                return Ok(crate::sms::SmsEvent::Inbound(crate::sms::Inbound {
                    event_id: field("/data/id"),
                    message_id: field("/data/id"),
                    our_number: field("/data/to"),
                    their_number: field("/data/from"),
                    text: Some(field("/data/text")),
                    thread_id: Some(field("/data/thread")),
                    ..crate::sms::Inbound::default()
                }))
            }
            _ => return crate::providers::telnyx::Telnyx.parse(request),
        }
        Ok(crate::sms::SmsEvent::OptSignal(crate::sms::OptSignal {
            event_id: field("/data/id"),
            our_number: field("/data/to"),
            their_number: field("/data/from"),
            keyword: if field("/data/keyword") == "STOP" {
                crate::sms::Keyword::Stop
            } else {
                crate::sms::Keyword::Start
            },
        }))
    }
}

/// The bed's bridge moved to `provider`'s adapter.
fn use_provider(bed: &Bed, provider: TestTelnyx) {
    use crate::sms::SmsProvider as _;
    moqui_core::service::chat_bridge::register_adapter(
        bed.ctx.factory(),
        Arc::new(crate::sms::SmsAdapter::new(provider)),
    );
    let _authz = bed.ctx.disable_authz();
    let mut row = bed
        .facade
        .find("moqui.chat.ChatBridge")
        .unwrap()
        .eq("chatBridgeId", text(BRIDGE))
        .one()
        .unwrap()
        .unwrap();
    row.set("adapterName", text(provider.adapter_name()));
    row.update(&bed.facade).unwrap();
}

/// A provider's opt-out and opt-in signals (not a text: a STOP it took itself) keep the pair's opt-out, each event once:
/// a copy the replay cache no longer catches (a new signature, as from a provider with no timestamp in it) changes
/// nothing; one for a number not on the bridge is left.
#[test]
fn a_providers_opt_out_signal_is_kept_once() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    use_provider(&bed, NO_GROUPS);
    let signal = |id: &str, to: &str, keyword: &str, at: i64| {
        let event =
            json!({"data": {"event_type": "test.opt_signal", "id": id, "to": to, "from": THEIRS, "keyword": keyword}});
        bed.hook(&bed.signed(&event, at))
    };
    let opted_out = || {
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
        )["optedOut"]
            .clone()
    };
    let stop = signal("sig-1", OURS, "STOP", now() / 1000);
    assert_eq!(status(&stop), 200, "{}", String::from_utf8_lossy(&stop.body));
    assert_eq!(body_json(&stop)["outcome"], "noted");
    assert_eq!(opted_out(), "Y");
    assert_eq!(
        bed.count(
            "moqui.chat.ChatBridgeInbound",
            &[("chatBridgeId", text(BRIDGE)), ("transactionId", text("signal:sig-1"))]
        ),
        1
    );
    assert_eq!(
        body_json(&signal("sig-2", OURS, "START", now() / 1000))["outcome"],
        "noted"
    );
    assert_eq!(opted_out(), "N");
    // the STOP again, signed anew: applied before, so the START stands
    let again = signal("sig-1", OURS, "STOP", now() / 1000 + 1);
    assert_eq!(body_json(&again)["outcome"], "duplicate");
    assert_eq!(opted_out(), "N");
    // a number that is not this bridge's
    let stray = signal("sig-3", "+15550009999", "STOP", now() / 1000);
    assert_eq!(
        (
            body_json(&stray)["outcome"].clone(),
            body_json(&stray)["reasonCode"].clone()
        ),
        (json!("ignored"), json!("unknownNumber"))
    );
}

/// SEC-COM-90: a STOP or START older than the pair's last change changes nothing, so an old signed text posted again
/// (as a new message, past the once-only mark) cannot undo a later keyword; a newer one still does.
#[test]
fn an_older_keyword_does_not_undo_a_later_one() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let opted_out = || {
        bed.one(
            "moqui.call.SmsOptOut",
            &[("phoneNumber", text(OURS)), ("otherNumber", text(THEIRS))],
        )["optedOut"]
            .clone()
    };
    assert_eq!(
        status(&bed.text_in_at(THEIRS, "STOP", "2026-11-01T12:05:00.000+00:00")),
        200
    );
    assert_eq!(opted_out(), "Y");
    bed.text_in_at(THEIRS, "START", "2026-11-01T12:00:00.000+00:00");
    assert_eq!(opted_out(), "Y", "a START sent before the STOP does not opt back in");
    bed.text_in_at(THEIRS, "START", "2026-11-01T12:10:00.000+00:00");
    assert_eq!(opted_out(), "N", "a later START does");
}

/// SEC-COM-91: texts from outside senders are capped: past channels@new-pair-rooms-per-day new conversations a day a
/// text from another new number is not taken (a known one still is), and past channels@inbound-media-bytes-per-day a
/// bridge's file is named as not received.
#[test]
fn new_conversations_and_inbound_files_have_a_day_cap() {
    let bed =
        bed_with(r#"<moqui-conf><channels new-pair-rooms-per-day="1" inbound-media-bytes-per-day="6"/></moqui-conf>"#);
    let _entered = bed.ctx.enter();
    let picture = bed.fake.file("pic.jpg", "image/jpeg", &[1, 2, 3, 4, 5]);
    let media = |url: &str| json!([{"url": url, "content_type": "image/jpeg", "size": 5}]);
    let (first, _) = bed.text_in(THEIRS, "Hi", media(&picture));
    assert_eq!(body_json(&first)["outcome"], "accepted");
    let room = bed.room(THEIRS).unwrap();
    const NEW: &str = "+15556667777";
    let (refused, _) = bed.text_in(NEW, "Hello", json!([]));
    assert_eq!(
        (
            body_json(&refused)["outcome"].clone(),
            body_json(&refused)["reasonCode"].clone()
        ),
        (json!("ignored"), json!("tooManyNewConversations"))
    );
    assert!(bed.room(NEW).is_none(), "no room for the new number");
    // the known number's next text comes in, and its file is past the bridge's bytes of the day
    let (again, _) = bed.text_in(THEIRS, "Another", media(&picture));
    assert_eq!(body_json(&again)["outcome"], "accepted");
    let messages = bed.messages(&room);
    assert_eq!(messages.len(), 2);
    assert!(messages[1].1.contains("A file not received"), "{}", messages[1].1);
}

/// A bridge whose provider carries no group texts (its capabilities say `groupTexts` false): Text a number with several
/// numbers is refused saying why, a group room the bridge had is refused at the send (before the provider, not
/// counted), and a group text that comes in is the sender's pair room; a text to one number still goes.
#[test]
fn a_group_room_on_a_bridge_without_group_texts_refuses_to_send() {
    const OTHER: &str = "+15553334444";
    let bed = bed();
    let _entered = bed.ctx.enter();
    // a group room made while the bridge's provider carried group texts
    assert_eq!(
        body_json(&bed.group_in(THEIRS, &[OURS], &[OTHER], "Hi all"))["outcome"],
        "accepted"
    );
    let mut sorted = [THEIRS, OTHER];
    sorted.sort_unstable();
    let group_room = bed.room(&sorted.join(",")).expect("the group's room");

    // the bridge moves to a provider without group texts
    use_provider(&bed, NO_GROUPS);
    login("alice");
    let caps = ok(
        "org.moqui.chat.ChatServices.get#BridgeCapabilities",
        &[("chatBridgeIdList", Value::List(vec![text(BRIDGE)]))],
    );
    let Some(Value::Map(caps)) = caps.get("capabilityMap") else {
        panic!("no capabilityMap: {caps:?}")
    };
    let Some(Value::Map(bridge)) = caps.get(BRIDGE) else {
        panic!("no capabilities of {BRIDGE}: {caps:?}")
    };
    assert_eq!(bridge.get("groupTexts"), Some(&Value::Bool(false)));
    assert_eq!(bridge.get("groupMax").map(to_plain_string).as_deref(), Some("0"));

    // the group room refuses to send, saying why; nothing reached the provider, nothing counted
    let counted = bed.sent_count("user:alice");
    let seq = bed.send(&group_room, "Anyone?", &[]);
    bed.round(0);
    let (state, reason, ..) = bed.delivery(&group_room, seq);
    assert_eq!(
        (state.as_str(), reason.as_str()),
        ("CdsFailed", "groupTextsNotSupported")
    );
    let row = bed.one(
        "moqui.chat.ChatBridgeDelivery",
        &[("chatRoomId", text(&group_room)), ("eventSeq", Value::I64(seq))],
    );
    assert!(row["lastErrorText"].contains("does not carry group texts"), "{row:?}");
    assert!(bed.fake.sends().is_empty());
    assert_eq!(bed.sent_count("user:alice"), counted, "not counted");

    // Text a number with two numbers: refused, saying why, and no room made
    let refused = call(
        "org.moqui.call.SmsServices.open#PairRoom",
        &[
            ("toNumber", text(&format!("{THEIRS},+15553335555"))),
            ("fromNumber", text(OURS)),
        ],
    );
    assert_eq!(out_text(&refused, "reasonCode"), "groupTextsNotSupported");
    assert!(errors().contains("does not carry group texts"));
    let mut asked = [THEIRS, "+15553335555"];
    asked.sort_unstable();
    assert!(bed.room(&asked.join(",")).is_none(), "no room made");

    // a group text that comes in is the sender's pair room, and a reply there goes as a text to one number
    let response = bed.group_in(THEIRS, &[OURS], &["+15553336666"], "New group?");
    assert_eq!(body_json(&response)["outcome"], "accepted");
    let pair = bed.room(THEIRS).expect("the sender's pair room");
    let reply = bed.send(&pair, "Just you", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&pair, reply).0, "CdsAccepted");
    assert_eq!(bed.fake.sends()[0].path, "/v2/messages");
}

/// The URL a send names for its receipts (a provider's status callback): the bridge's `webhookUrl` as it is, else
/// `channels@public-url` and the bridge's webhook route.
#[test]
fn a_send_names_the_bridges_webhook_url_for_its_receipts() {
    // the value every bed sets
    moqui_lib::env::set_property("moqui_conf__channels__public_url", "https://moqui.example.test");
    let bridge = |config: Json| moqui_core::service::chat_bridge::BridgeInfo {
        chat_bridge_id: BRIDGE.to_string(),
        type_enum_id: "CbtSms".to_string(),
        adapter_name: "sms".to_string(),
        name: "B".to_string(),
        config,
    };
    assert_eq!(
        crate::sms::callback_url(&bridge(json!({}))).as_deref(),
        Some("https://moqui.example.test/channels/SMS1/webhook")
    );
    assert_eq!(
        crate::sms::callback_url(&bridge(json!({"webhookUrl": "https://hooks.example/sms?b=1"}))).as_deref(),
        Some("https://hooks.example/sms?b=1")
    );
}

/// A text in a provider's own conversation (`thread_id`, a Twilio Conversation) comes to the room of the mapping that
/// keeps it (store#MappingThread, `crate::sms::store_thread`), whoever in it writes, not to the sender's pair room.
#[test]
fn a_text_in_a_providers_conversation_comes_to_its_room() {
    const OTHER: &str = "+15553334444";
    let bed = bed();
    let _entered = bed.ctx.enter();
    assert_eq!(
        body_json(&bed.group_in(THEIRS, &[OURS], &[OTHER], "Hi all"))["outcome"],
        "accepted"
    );
    let mut sorted = [THEIRS, OTHER];
    sorted.sort_unstable();
    let external = format!("{OURS}/{}", sorted.join(","));
    let group_room = bed.room(&sorted.join(",")).expect("the group's room");
    use_provider(&bed, THREADS);
    crate::sms::store_thread(&bridge_info(&bed), &external, "CH-77").unwrap();
    errors();
    let event = json!({"data": {"event_type": "test.thread_text", "id": "th-1", "to": OURS, "from": OTHER,
        "text": "In the conversation", "thread": "CH-77"}});
    let response = bed.hook(&bed.signed(&event, now() / 1000));
    assert_eq!(
        body_json(&response)["outcome"],
        "accepted",
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(
        bed.messages(&group_room).last().map(|(_, text)| text.as_str()),
        Some("In the conversation")
    );
    assert!(bed.room(OTHER).is_none(), "not the sender's pair room");
}

/// Mark our number for room texts (`PhoneNumber.roomTextsEnabled`), or not.
fn room_texts(bed: &Bed, on: bool) {
    let _authz = bed.ctx.disable_authz();
    let mut row = bed
        .facade
        .find("moqui.call.PhoneNumber")
        .unwrap()
        .eq("phoneNumber", text(OURS))
        .one()
        .unwrap()
        .unwrap();
    row.set("roomTextsEnabled", if on { text("Y") } else { Value::Null });
    row.update(&bed.facade).unwrap();
}

#[test]
fn a_text_member_gets_the_rooms_messages_with_the_senders_name_and_its_reply_comes_back() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    room_texts(&bed, true);
    login("alice");
    let room = out_text(
        &ok(
            "org.moqui.chat.ChatServices.create#Room",
            &[
                ("roomTypeEnumId", text("CrtPrivate")),
                ("roomName", text("Soccer parents")),
                ("memberUserIdList", Value::List(vec![text("bob")])),
            ],
        ),
        "chatRoomId",
    );
    let added = ok(
        "org.moqui.call.SmsRoomServices.add#TextMember",
        &[
            ("chatRoomId", text(&room)),
            ("phoneNumber", text(THEIRS)),
            ("memberName", text("Dee")),
            ("consent", text("Y")),
        ],
    );
    assert_eq!(out_text(&added, "ourNumber"), OURS);
    // the welcome: straight to the provider, naming the room and how to leave
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 1);
    let welcome: Json = serde_json::from_str(&sends[0].body).unwrap();
    assert_eq!(welcome["to"], THEIRS);
    assert!(welcome["text"].as_str().unwrap().contains("Soccer parents"));
    assert!(welcome["text"].as_str().unwrap().contains("Reply STOP"));

    // a message of the room: as a text that names its sender, checked and counted as the room's
    let seq = bed.send(&room, "Practice at six", &[]);
    bed.round(0);
    let sends = bed.fake.sends();
    assert_eq!(sends.len(), 2);
    let sent: Json = serde_json::from_str(&sends[1].body).unwrap();
    assert_eq!((sent["from"].as_str(), sent["to"].as_str()), (Some(OURS), Some(THEIRS)));
    assert_eq!(sent["text"], "ALICE: Practice at six");
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(
        bed.count(
            "moqui.call.SmsSendCount",
            &[("counterKey", text(&format!("room:{room}")))]
        ),
        1
    );
    let room_count = |bed: &Bed| {
        bed.one(
            "moqui.call.SmsSendCount",
            &[("counterKey", text(&format!("room:{room}")))],
        )["sentCount"]
            .clone()
    };
    assert_eq!(room_count(&bed), "2", "the welcome and the message");
    // the adder's day counts them too: the room's texts go on alice, who added the member
    assert_eq!(
        bed.one("moqui.call.SmsSendCount", &[("counterKey", text("user:alice"))])["sentCount"],
        "2"
    );

    // the member's reply: in the room as the member, and nothing back to them
    bed.text_in(THEIRS, "Running late", json!([]));
    assert_eq!(
        bed.messages(&room).last().map(|(_, body)| body.as_str()),
        Some("Running late")
    );
    assert!(bed.room(THEIRS).is_none(), "not the pair room");
    bed.round(0);
    assert_eq!(bed.fake.sends().len(), 2);

    // the number no longer carries room texts: refused before the provider, its place given back
    room_texts(&bed, false);
    let refused = bed.send(&room, "Anyone?", &[]);
    bed.round(0);
    let (state, reason, ..) = bed.delivery(&room, refused);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "roomTextsOff"));
    assert_eq!(bed.fake.sends().len(), 2);
    assert_eq!(room_count(&bed), "2");

    // a group text with the member goes as ever (inRoomByText is a pair room's), and the pair room's text is refused
    room_texts(&bed, true);
    let group = out_text(
        &ok(
            "org.moqui.call.SmsServices.open#PairRoom",
            &[
                ("fromNumber", text(OURS)),
                ("toNumber", text(&format!("{THEIRS},+15557778899"))),
            ],
        ),
        "chatRoomId",
    );
    let grouped = bed.send(&group, "Both of you", &[]);
    bed.round(0);
    assert_eq!(bed.delivery(&group, grouped).0, "CdsAccepted");
    assert_eq!(bed.fake.sends().len(), 3);
    let pair = out_text(
        &ok(
            "org.moqui.call.SmsServices.open#PairRoom",
            &[("fromNumber", text(OURS)), ("toNumber", text(THEIRS))],
        ),
        "chatRoomId",
    );
    let paired = bed.send(&pair, "Just you", &[]);
    bed.round(0);
    let (state, reason, ..) = bed.delivery(&pair, paired);
    assert_eq!((state.as_str(), reason.as_str()), ("CdsFailed", "inRoomByText"));
    assert_eq!(bed.fake.sends().len(), 3);
}

#[test]
fn a_member_mappings_room_id_reads_as_its_pair_and_its_text_names_its_sender() {
    use crate::sms::{member_of, member_request, numbers_of, pair_of};
    let id = format!("{OURS}/{THEIRS}#100200");
    assert_eq!(
        member_of(&id),
        Some((OURS.to_string(), THEIRS.to_string(), "100200".to_string()))
    );
    assert_eq!(numbers_of(&id), Some((OURS.to_string(), vec![THEIRS.to_string()])));
    assert_eq!(pair_of(&id), Some((OURS.to_string(), THEIRS.to_string())));
    assert_eq!(member_of(&format!("{OURS}/{THEIRS}")), None);
    assert_eq!(member_of(&format!("{OURS}/{THEIRS}#")), None);
    assert_eq!(member_of(&format!("{OURS}/{THEIRS},+15557778889#1")), None);
    let request = moqui_core::service::chat_bridge::DeliveryRequest {
        chat_bridge_delivery_id: "1".to_string(),
        idempotency_key: "k".to_string(),
        chat_room_id: "100200".to_string(),
        event_seq: 3,
        external_room_id: id,
        external_thread_id: None,
        kind: moqui_core::service::chat_bridge::DeliveryKind::Message,
        text: Some("See the schedule".to_string()),
        html: None,
        sender_name: Some("A very long sender name that goes past thirty".to_string()),
        sender_user_id: None,
        attachments: vec![
            moqui_core::service::chat_bridge::DeliveryAttachment {
                chat_attachment_id: "a".to_string(),
                file_name: Some("photo.png".to_string()),
                content_type: Some("image/png".to_string()),
                file_size: Some(1000),
            },
            moqui_core::service::chat_bridge::DeliveryAttachment {
                chat_attachment_id: "b".to_string(),
                file_name: Some("report.pdf".to_string()),
                content_type: Some("application/pdf".to_string()),
                file_size: Some(1000),
            },
        ],
        target_remote_id: None,
        reaction_key: None,
        rich: None,
        location: None,
        text_form: None,
        attempt: 1,
    };
    let shaped = member_request(&request, 1600, 10, 5_000_000);
    assert_eq!(
        shaped.text.as_deref(),
        Some("A very long sender name that g: See the schedule\n[report.pdf: a file, not sent by text]")
    );
    assert_eq!(shaped.attachments.len(), 1);
    let cut = member_request(&request, 20, 10, 5_000_000);
    assert_eq!(cut.text.as_deref().map(|text| text.chars().count()), Some(20));
    assert!(cut.text.unwrap().ends_with('…'));
}

mod rcs;
mod sinch;
mod twilio;
