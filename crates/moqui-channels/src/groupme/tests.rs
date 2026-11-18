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

//! The GroupMe adapter (plans/Comms-Messaging.md §18.3 phase 14) against a fake GroupMe on loopback: its REST API, its
//! image service and its push server, on an in-memory facade. The push session is driven a step at a time
//! ([`Session::step`]), with no thread and no timing. Not GroupMe itself: docs/interfaces/GroupMe.md lists the facts
//! not confirmed.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::service::chat_bridge::{BridgeInfo, ChatBridgeAdapter as _, Encryption};
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

use super::push::{Session, Step, REFUSED};
use super::GroupMeAdapter;

const BRIDGE: &str = "GM1";
const OWN: &str = "1000";
const BOB_GM: &str = "2000";
const CAROL_GM: &str = "3000";
const GROUP: &str = "500";
const TOPIC: &str = "501";

/// The bridge's access token: shaped like one, so built here and never one literal.
fn token() -> String {
    format!("{}{}", "gmTestToken", "0123456789abcdef")
}

// ---- the fake GroupMe ----

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Seen {
    fn json(&self) -> Json {
        serde_json::from_slice(&self.body).unwrap_or(Json::Null)
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An answer the fake gives once to the next request whose method and path start as named.
#[derive(Clone, Debug)]
enum Reply {
    Status(u16, String),
    /// Keep the message, then close the connection with no answer.
    Drop,
}

#[derive(Default)]
struct State {
    base: String,
    token: String,
    seen: Vec<Seen>,
    script: VecDeque<(String, Reply)>,
    /// The push events the next `/meta/connect` answers.
    push: VecDeque<Json>,
    /// Each conversation's messages, oldest first (a group's by its id, a person's by theirs).
    messages: HashMap<String, Vec<Json>>,
    files: HashMap<String, (String, Vec<u8>)>,
    next: u64,
}

struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
}

impl Fake {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            base: base.clone(),
            token: token(),
            ..State::default()
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

    fn script(&self, prefix: &str, reply: Reply) {
        self.state.lock().unwrap().script.push_back((prefix.to_string(), reply));
    }

    fn push(&self, data: Json) {
        self.state.lock().unwrap().push.push_back(data);
    }

    fn message(&self, conversation: &str, message: Json) {
        self.state
            .lock()
            .unwrap()
            .messages
            .entry(conversation.to_string())
            .or_default()
            .push(message);
    }

    fn file(&self, name: &str, content_type: &str, bytes: &[u8]) -> String {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(name.to_string(), (content_type.to_string(), bytes.to_vec()));
        format!("{}/media/{name}", self.base)
    }

    fn seen(&self, method: &str, prefix: &str) -> Vec<Seen> {
        self.state
            .lock()
            .unwrap()
            .seen
            .iter()
            .filter(|seen| seen.method == method && seen.path.starts_with(prefix))
            .cloned()
            .collect()
    }

    fn set_token(&self, token: &str) {
        self.state.lock().unwrap().token = token.to_string();
    }
}

fn group_json() -> Json {
    json!({"id": GROUP, "name": "Family", "messages": {"last_message_id": null},
        "members": [{"user_id": OWN, "nickname": "Alice"}, {"user_id": BOB_GM, "nickname": "Bob"},
            {"user_id": CAROL_GM, "nickname": "Carol"}]})
}

/// The fake's answer to one request.
fn answer(state: &Mutex<State>, seen: &Seen) -> Option<(u16, String, Vec<u8>)> {
    let ok = |value: Json| {
        Some((
            200,
            "application/json".to_string(),
            json!({"response": value}).to_string().into_bytes(),
        ))
    };
    let mut state = state.lock().unwrap();
    state.seen.push(seen.clone());
    let key = format!("{} {}", seen.method, seen.path);
    if let Some(at) = state
        .script
        .iter()
        .position(|(prefix, _)| key.starts_with(prefix.as_str()))
    {
        let (_, reply) = state.script.remove(at).unwrap();
        match reply {
            Reply::Status(status, body) => return Some((status, "application/json".to_string(), body.into_bytes())),
            Reply::Drop => {
                // kept, as GroupMe would have, then no answer
                if let Some(group) = seen
                    .path
                    .strip_prefix("/groups/")
                    .and_then(|rest| rest.strip_suffix("/messages"))
                {
                    let message = seen.json()["message"].clone();
                    state.next += 1;
                    let mut kept = message;
                    kept["id"] = json!(format!("m{}", state.next));
                    kept["user_id"] = json!(OWN);
                    state.messages.entry(group.to_string()).or_default().push(kept);
                }
                return None;
            }
        }
    }
    let (path, query) = seen.path.split_once('?').unwrap_or((&seen.path, ""));
    let param = |name: &str| {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    };
    if seen.method == "GET" && path.starts_with("/media/") {
        return state
            .files
            .get(path.trim_start_matches("/media/"))
            .cloned()
            .map(|(kind, bytes)| (200, kind, bytes))
            .or(Some((404, "text/plain".to_string(), b"none".to_vec())));
    }
    if path == "/faye" {
        let messages = seen.json();
        let first = messages.get(0).cloned().unwrap_or(Json::Null);
        let reply = match first["channel"].as_str().unwrap_or_default() {
            "/meta/handshake" => {
                json!([{"id": first["id"], "channel": "/meta/handshake", "successful": true, "version": "1.0",
                "clientId": "client-1", "advice": {"reconnect": "retry", "interval": 0, "timeout": 30000}}])
            }
            "/meta/subscribe" => {
                if first["ext"]["access_token"].as_str() == Some(state.token.as_str()) {
                    json!([{"id": first["id"], "channel": "/meta/subscribe", "clientId": "client-1", "successful": true,
                        "subscription": first["subscription"]}])
                } else {
                    json!([{"id": first["id"], "channel": "/meta/subscribe", "successful": false, "error": "401::Unauthorized"}])
                }
            }
            _ => {
                let mut list = vec![
                    json!({"id": first["id"], "channel": "/meta/connect", "clientId": "client-1", "successful": true,
                    "advice": {"reconnect": "retry", "interval": 0, "timeout": 30000}}),
                ];
                while let Some(data) = state.push.pop_front() {
                    list.push(
                        json!({"channel": format!("/user/{OWN}"), "data": data, "clientId": "client-1", "id": "p"}),
                    );
                }
                Json::Array(list)
            }
        };
        return Some((200, "application/json".to_string(), reply.to_string().into_bytes()));
    }
    if path == "/pictures" {
        state.next += 1;
        let name = format!("pic{}.png", state.next);
        let kind = seen.header("content-type").unwrap_or_default().to_string();
        state.files.insert(name.clone(), (kind, seen.body.clone()));
        let url = format!("{}/media/{name}", state.base);
        return Some((
            200,
            "application/json".to_string(),
            json!({"payload": {"url": url, "picture_url": url}})
                .to_string()
                .into_bytes(),
        ));
    }
    // the API: the token in its header, or 401
    if seen.header("x-access-token") != Some(state.token.as_str()) {
        return Some((
            401,
            "application/json".to_string(),
            json!({"meta": {"code": 401, "errors": ["unauthorized"]}, "response": null})
                .to_string()
                .into_bytes(),
        ));
    }
    let after = |list: &[Json], after: Option<String>| -> Vec<Json> {
        match after {
            Some(after) => list
                .iter()
                .skip_while(|message| message["id"].as_str() != Some(after.as_str()))
                .skip(1)
                .cloned()
                .collect(),
            None => list.iter().rev().take(20).cloned().collect(),
        }
    };
    match (seen.method.as_str(), path) {
        ("GET", "/users/me") => ok(json!({"id": OWN, "name": "Alice"})),
        ("GET", "/groups") => {
            let mut group = group_json();
            group["members"] = Json::Null;
            group["messages"]["last_message_id"] = state
                .messages
                .get(GROUP)
                .and_then(|list| list.last())
                .map_or(Json::Null, |message| message["id"].clone());
            ok(if param("page").as_deref() == Some("1") {
                json!([group])
            } else {
                json!([])
            })
        }
        ("GET", "/chats") => ok(json!([])),
        ("GET", path) if path == format!("/groups/{GROUP}") => ok(group_json()),
        ("GET", path) if path == format!("/groups/{GROUP}/subgroups") => {
            ok(json!([{"id": TOPIC, "topic": "Trips", "parent_id": GROUP, "messages": {"last_message_id": null}}]))
        }
        ("GET", path) if path.starts_with("/groups/") && path.ends_with("/messages") => {
            let group = path.trim_start_matches("/groups/").trim_end_matches("/messages");
            let list = state.messages.get(group).cloned().unwrap_or_default();
            ok(json!({"count": list.len(), "messages": after(&list, param("after_id"))}))
        }
        ("GET", "/direct_messages") => {
            let other = param("other_user_id").unwrap_or_default();
            let list = state.messages.get(&other).cloned().unwrap_or_default();
            ok(json!({"count": list.len(), "direct_messages": after(&list, param("after_id"))}))
        }
        ("POST", path) if path.starts_with("/groups/") && path.ends_with("/messages") => {
            let group = path
                .trim_start_matches("/groups/")
                .trim_end_matches("/messages")
                .to_string();
            state.next += 1;
            let mut message = seen.json()["message"].clone();
            message["id"] = json!(format!("m{}", state.next));
            message["user_id"] = json!(OWN);
            message["group_id"] = json!(group);
            state.messages.entry(group).or_default().push(message.clone());
            Some((
                201,
                "application/json".to_string(),
                json!({"response": {"message": message}}).to_string().into_bytes(),
            ))
        }
        ("POST", "/direct_messages") => {
            state.next += 1;
            let mut message = seen.json()["direct_message"].clone();
            let other = message["recipient_id"].as_str().unwrap_or_default().to_string();
            message["id"] = json!(format!("d{}", state.next));
            message["conversation_id"] = json!(format!("{OWN}+{other}"));
            state.messages.entry(other).or_default().push(message.clone());
            Some((
                201,
                "application/json".to_string(),
                json!({"response": {"direct_message": message}})
                    .to_string()
                    .into_bytes(),
            ))
        }
        ("POST", path) if path.starts_with("/messages/") => ok(Json::Null),
        _ => Some((
            404,
            "application/json".to_string(),
            json!({"meta": {"code": 404, "errors": ["not found"]}})
                .to_string()
                .into_bytes(),
        )),
    }
}

fn serve(mut stream: TcpStream, state: &Mutex<State>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
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
    let mut parts = request_line.split(' ');
    let seen = Seen {
        method: parts.next().unwrap_or_default().to_string(),
        path: parts.next().unwrap_or_default().to_string(),
        headers,
        body: buffer[head_end..].to_vec(),
    };
    match answer(state, &seen) {
        Some((status, content_type, body)) => {
            let head = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
        None => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

// ---- the bed ----

struct Bed {
    ctx: ExecutionContext,
    facade: EntityFacade,
    fake: Fake,
    adapter: GroupMeAdapter,
    session: std::cell::RefCell<Session>,
}

fn text(value: &str) -> Value {
    Value::string(value)
}

fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
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

fn config(base: &str) -> Json {
    json!({"accessToken": token(), "ownerUserId": "alice", "apiBaseUrl": base, "imageBaseUrl": base,
        "pushUrl": format!("{base}/faye"), "mediaBaseUrl": format!("{base}/media"), "allowInternalAddresses": true,
        "push": false})
}

/// alice's GroupMe account (her GroupMe id OWN, in the group Family with Bob and Carol) as bridge GM1, on a fake
/// GroupMe; bob is another local user.
fn bed() -> Bed {
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
    {
        let _entered = ctx.enter();
        for (id, name) in [("alice", "Alice Adams"), ("bob", "Bob Brown")] {
            row(
                &facade,
                "moqui.security.UserAccount",
                &[
                    ("userId", text(id)),
                    ("username", text(id)),
                    ("userFullName", text(name)),
                ],
            );
        }
        row(
            &facade,
            "moqui.chat.ChatBridge",
            &[
                ("chatBridgeId", text(BRIDGE)),
                ("typeEnumId", text("CbtGroupMe")),
                ("adapterName", text("groupme")),
                ("name", text("GroupMe of Alice")),
                ("statusId", text("CbsNotConnected")),
                ("configJson", text(&config(&fake.base).to_string())),
                ("createdDate", Value::I64(now())),
            ],
        );
    }
    Bed {
        ctx,
        facade,
        fake,
        adapter: GroupMeAdapter::default(),
        session: std::cell::RefCell::new(Session::new()),
    }
}

impl Bed {
    fn info(&self) -> BridgeInfo {
        let (info, _) = moqui_core::service::chat_bridge::bridge_row(&self.facade, BRIDGE)
            .unwrap()
            .unwrap();
        info
    }

    /// One push step, as the bridge's thread takes it.
    fn step(&self) -> Step {
        let _authz = self.ctx.disable_authz();
        let info = self.info();
        self.session
            .borrow_mut()
            .step(self.adapter.inner(), &self.facade, &info)
    }

    /// Subscribe (the first step), with the catch-up that makes the group's room.
    fn subscribe(&self) {
        assert!(matches!(self.step(), Step::Subscribed(_)));
    }

    fn one(&self, entity: &str, fields: &[(&str, Value)]) -> Option<BTreeMap<String, String>> {
        let _authz = self.ctx.disable_authz();
        let mut find = self.facade.find(entity).unwrap();
        for (name, value) in fields {
            find = find.eq(name, value.clone());
        }
        find.one().unwrap().map(|row| {
            row.fields()
                .iter()
                .map(|(name, value)| (name.to_string(), to_plain_string(value)))
                .collect()
        })
    }

    fn count(&self, entity: &str, fields: &[(&str, Value)]) -> i64 {
        let _authz = self.ctx.disable_authz();
        let mut find = self.facade.find(entity).unwrap();
        for (name, value) in fields {
            find = find.eq(name, value.clone());
        }
        find.count().unwrap()
    }

    /// The room of `external` (g:<id>, dm:<id>).
    fn room(&self, external: &str) -> String {
        self.one(
            "moqui.chat.ChatRoomBridge",
            &[("chatBridgeId", text(BRIDGE)), ("externalRoomId", text(external))],
        )
        .unwrap_or_else(|| panic!("no room {external}"))["chatRoomId"]
            .clone()
    }

    /// The messages of a room: (sender actor, body HTML), oldest first.
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
                let body = serde_json::from_str::<Json>(&get("contentJson")).unwrap_or(Json::Null)["body"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                (get("senderActorId"), body)
            })
            .collect()
    }

    /// The heart reactions on event `seq` of `room` not taken back: their senders' actors (or users).
    fn hearts(&self, room: &str, seq: i64) -> Vec<String> {
        let _authz = self.ctx.disable_authz();
        self.facade
            .find("moqui.chat.ChatEvent")
            .unwrap()
            .eq("chatRoomId", text(room))
            .eq("eventTypeEnumId", text("CetReaction"))
            .eq("relatesToEventSeq", Value::I64(seq))
            .order_by("eventSeq", false)
            .list()
            .unwrap()
            .values()
            .iter()
            .filter(|row| {
                row.get("redactedDate")
                    .is_none_or(|date| to_plain_string(date).is_empty())
            })
            .filter(|row| {
                let content = row.get("contentJson").map(to_plain_string).unwrap_or_default();
                serde_json::from_str::<Json>(&content).unwrap_or(Json::Null)["key"] == "❤️"
            })
            .map(|row| row.get("senderActorId").map(to_plain_string).unwrap_or_default())
            .collect()
    }

    /// The local event of GroupMe message `id` in room `external`.
    fn seq_of(&self, external: &str, id: &str) -> i64 {
        self.one(
            "moqui.chat.ChatEventExternal",
            &[
                ("chatBridgeId", text(BRIDGE)),
                ("externalRoomId", text(external)),
                ("externalEventId", text(id)),
            ],
        )
        .unwrap_or_else(|| panic!("no event {id}"))["eventSeq"]
            .parse()
            .unwrap()
    }

    /// `user` writes `body` (HTML) in the room; its eventSeq.
    fn send(&self, user: &str, room: &str, body: &str, files: &[String]) -> i64 {
        login(user);
        let out = ok(
            "org.moqui.chat.ChatServices.send#Message",
            &[
                ("chatRoomId", text(room)),
                ("body", text(body)),
                (
                    "chatAttachmentIdList",
                    Value::List(files.iter().map(|id| text(id)).collect()),
                ),
            ],
        );
        out_text(&out, "eventSeq").parse().unwrap()
    }

    fn react(&self, room: &str, seq: i64, key: &str, mode: &str) {
        login("alice");
        ok(
            "org.moqui.chat.ChatServices.react#Message",
            &[
                ("chatRoomId", text(room)),
                ("eventSeq", Value::I64(seq)),
                ("key", text(key)),
                ("mode", text(mode)),
            ],
        );
    }

    fn round(&self) {
        let _authz = self.ctx.disable_authz();
        ok(
            "org.moqui.chat.ChatBridgeServices.run#BridgeDeliveries",
            &[("nowMillis", Value::I64(now()))],
        );
    }

    /// The delivery of event `seq` of `room`: (state, reason, remote id, last error).
    fn delivery(&self, room: &str, seq: i64) -> (String, String, String, String) {
        let row = self
            .one(
                "moqui.chat.ChatBridgeDelivery",
                &[("chatRoomId", text(room)), ("eventSeq", Value::I64(seq))],
            )
            .unwrap_or_else(|| panic!("no delivery of {seq}"));
        let get = |name: &str| row.get(name).cloned().unwrap_or_default();
        (
            get("statusEnumId"),
            get("reasonCode"),
            get("remoteEventId"),
            get("lastErrorText"),
        )
    }

    /// A file alice put in a room.
    fn upload(&self, room: &str, name: &str, content_type: &str, bytes: &[u8]) -> String {
        let _authz = self.ctx.disable_authz();
        let mut file = self.facade.make_value("moqui.chat.ChatAttachment").unwrap();
        file.set_sequenced_id_primary(&self.facade).unwrap();
        let id = file.get("chatAttachmentId").map(to_plain_string).unwrap();
        let location = format!("dbresource://chat/{room}/{id}/{name}");
        self.ctx.factory().resource().write_bytes(&location, bytes).unwrap();
        for (field, value) in [
            ("chatRoomId", text(room)),
            ("location", text(&location)),
            ("fileName", text(name)),
            ("contentType", text(content_type)),
            ("fileSize", Value::I64(i64::try_from(bytes.len()).unwrap())),
            ("uploadedByUserId", text("alice")),
        ] {
            file.set(field, value);
        }
        file.create(&self.facade).unwrap();
        id
    }

    fn status(&self) -> (String, String) {
        let row = self
            .one("moqui.chat.ChatBridge", &[("chatBridgeId", text(BRIDGE))])
            .unwrap();
        (
            row["statusId"].clone(),
            row.get("lastErrorText").cloned().unwrap_or_default(),
        )
    }
}

/// A group message of `from` (their GroupMe id and name).
fn line(id: &str, group: &str, from: &str, name: &str, words: &str) -> Json {
    json!({"type": "line.create", "alert": format!("{name}: {words}"), "subject": {"id": id, "group_id": group,
        "user_id": from, "sender_id": from, "name": name, "avatar_url": "https://i.groupme.com/60x60.png.abc",
        "text": words, "created_at": 1_700_000_000, "system": false, "source_guid": format!("guid-{id}"),
        "attachments": [], "favorited_by": []}})
}

// ---- the tests ----

/// GroupMe has no E2EE and carries hearts only: the capabilities say so, and the push channel subscribes with the
/// token, makes the group's room (alice its owner) and brings a message in once, as its GroupMe person; the owner's
/// echo of a send and a system message stay out.
#[test]
fn the_push_channel_brings_a_message_into_the_groups_room_once() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let caps = bed.adapter.capabilities(&bed.info());
    assert_eq!(caps.encryption, Encryption::None);
    assert!(caps.reaction && !caps.edit && !caps.delete && !caps.idempotent);

    bed.subscribe();
    let subscribe = bed.fake.seen("POST", "/faye");
    let sent = subscribe
        .iter()
        .map(Seen::json)
        .find(|body| body[0]["channel"] == "/meta/subscribe")
        .unwrap();
    assert_eq!(sent[0]["subscription"], format!("/user/{OWN}"));
    assert_eq!(sent[0]["ext"]["access_token"], token().as_str());
    assert!(bed.fake.seen("GET", "/users/me")[0].header("x-access-token") == Some(token().as_str()));
    assert_eq!(bed.status().0, "CbsConnected");
    let room = bed.room(&format!("g:{GROUP}"));
    let owner = bed
        .one(
            "moqui.chat.ChatRoomMember",
            &[("chatRoomId", text(&room)), ("userId", text("alice"))],
        )
        .unwrap();
    assert_eq!(owner["roleEnumId"], "CmrOwner");
    assert_eq!(
        bed.one("moqui.chat.ChatRoom", &[("chatRoomId", text(&room))]).unwrap()["roomName"],
        "Family"
    );

    bed.fake.push(line("g1", GROUP, BOB_GM, "Bob", "Dinner at six?"));
    assert_eq!(bed.step(), Step::Polled(1));
    // the same message again (another node's connection, or a repeat) and the owner's echo of a send
    bed.fake.push(line("g1", GROUP, BOB_GM, "Bob", "Dinner at six?"));
    let mut echo = line("g2", GROUP, OWN, "Alice", "from here");
    echo["subject"]["source_guid"] = json!("moqui-chat:R1:1:M1");
    bed.fake.push(echo);
    let mut system = line("g3", GROUP, "system", "GroupMe", "Bob changed the topic");
    system["subject"]["system"] = json!(true);
    bed.fake.push(system);
    bed.step();
    let messages = bed.messages(&room);
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].1.contains("Dinner at six?"));
    let person = bed
        .one(
            "moqui.chat.ChatExternalUser",
            &[("chatBridgeId", text(BRIDGE)), ("externalUserId", text(BOB_GM))],
        )
        .unwrap();
    assert_eq!(person["displayName"], "Bob");
    assert_eq!(person["avatarUrl"], "https://i.groupme.com/60x60.png.abc");
}

/// A message goes out with the delivery's key as its source_guid and the token in its header; a text over 1,000
/// characters, and another local member's message, are refused.
#[test]
fn a_message_goes_out_with_its_key_and_only_from_the_owner() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    let seq = bed.send("alice", &room, "<p>On my way</p>", &[]);
    bed.round();
    let (state, _, remote, _) = bed.delivery(&room, seq);
    assert_eq!(state, "CdsAccepted");
    let posted = bed.fake.seen("POST", &format!("/groups/{GROUP}/messages"));
    assert_eq!(posted.len(), 1);
    let message = &posted[0].json()["message"];
    assert_eq!(message["text"], "On my way");
    let key = bed
        .one(
            "moqui.chat.ChatBridgeDelivery",
            &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(seq))],
        )
        .unwrap()["idempotencyKey"]
        .clone();
    assert_eq!(message["source_guid"], key.as_str());
    assert_eq!(posted[0].header("x-access-token"), Some(token().as_str()));
    assert_eq!(remote, "m1");

    let long = bed.send("alice", &room, &format!("<p>{}</p>", "x".repeat(1001)), &[]);
    // bob, in the room, speaks for no one's GroupMe account
    row(
        &bed.facade,
        "moqui.chat.ChatRoomMember",
        &[
            ("chatRoomId", text(&room)),
            ("chatActorId", text("bob")),
            ("userId", text("bob")),
            ("roleEnumId", text("CmrMember")),
            ("joinedDate", Value::I64(now())),
            ("lastReadEventSeq", Value::I64(0)),
        ],
    );
    row(
        &bed.facade,
        "moqui.chat.ChatActor",
        &[
            ("chatActorId", text("bob")),
            ("kindEnumId", text("CakLocal")),
            ("userId", text("bob")),
        ],
    );
    let bobs = bed.send("bob", &room, "<p>Hi from Bob</p>", &[]);
    bed.round();
    assert_eq!(bed.delivery(&room, long).1, "textTooLong");
    assert_eq!(bed.delivery(&room, bobs).1, "notBridgeOwner");
    assert_eq!(bed.fake.seen("POST", &format!("/groups/{GROUP}/messages")).len(), 1);
}

/// A like is a heart both ways: alice's heart goes as a like and its removal as an unlike, another emoji is refused;
/// GroupMe's likes come in as hearts and go when taken back, and alice's own like is not doubled.
#[test]
fn a_like_is_a_heart_both_ways() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    bed.fake.push(line("g1", GROUP, BOB_GM, "Bob", "Photos are up"));
    bed.step();
    let seq = bed.seq_of(&format!("g:{GROUP}"), "g1");

    bed.react(&room, seq, "❤️", "add");
    bed.round();
    assert_eq!(bed.fake.seen("POST", &format!("/messages/{GROUP}/g1/like")).len(), 1);
    bed.react(&room, seq, "👍", "add");
    bed.round();
    let refused = {
        let _authz = bed.ctx.disable_authz();
        bed.facade
            .find("moqui.chat.ChatBridgeDelivery")
            .unwrap()
            .eq("chatRoomId", text(&room))
            .eq("reasonCode", text("reactionNotCarried"))
            .count()
            .unwrap()
    };
    assert_eq!(refused, 1);

    // GroupMe's likes: alice's own (from here) and Carol's
    let mut liked = line("g1", GROUP, BOB_GM, "Bob", "Photos are up");
    liked["subject"]["favorited_by"] = json!([OWN, CAROL_GM]);
    bed.fake
        .push(json!({"type": "favorite", "alert": "", "subject": {"line": liked["subject"], "user_id": CAROL_GM}}));
    bed.step();
    let hearts = bed.hearts(&room, seq);
    assert_eq!(hearts.len(), 2, "alice's and Carol's, not alice's twice: {hearts:?}");
    let carol = bed
        .one(
            "moqui.chat.ChatExternalUser",
            &[("chatBridgeId", text(BRIDGE)), ("externalUserId", text(CAROL_GM))],
        )
        .unwrap();
    let carol_actor = bed
        .one(
            "moqui.chat.ChatActor",
            &[("chatExternalUserId", text(&carol["chatExternalUserId"]))],
        )
        .unwrap()["chatActorId"]
        .clone();
    assert!(hearts.contains(&carol_actor));
    // the same state again changes nothing; Carol takes hers back
    bed.fake
        .push(json!({"type": "favorite", "alert": "", "subject": {"line": liked["subject"], "user_id": CAROL_GM}}));
    liked["subject"]["favorited_by"] = json!([OWN]);
    bed.fake
        .push(json!({"type": "favorite", "alert": "", "subject": {"line": liked["subject"], "user_id": CAROL_GM}}));
    bed.step();
    assert_eq!(bed.hearts(&room, seq), vec!["alice".to_string()]);

    // alice takes hers back: an unlike of the like this server sent
    bed.react(&room, seq, "❤️", "remove");
    bed.round();
    assert_eq!(bed.fake.seen("POST", &format!("/messages/{GROUP}/g1/unlike")).len(), 1);
}

/// `@all` from GroupMe mentions the room, and alice's `@room` goes as `@all` with every other member mentioned.
#[test]
fn all_and_room_mentions_map_both_ways() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    bed.fake.push(line("g1", GROUP, BOB_GM, "Bob", "@all dinner is ready"));
    bed.step();
    let messages = bed.messages(&room);
    assert!(
        messages[0].1.contains("<span data-mention-id=\"@room\">@all</span>"),
        "{messages:?}"
    );

    let seq = bed.send(
        "alice",
        &room,
        "<p><span data-mention-id=\"@room\">@room</span> coming</p>",
        &[],
    );
    bed.round();
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    let message = bed.fake.seen("POST", &format!("/groups/{GROUP}/messages"))[0].json()["message"].clone();
    assert_eq!(message["text"], "@all coming");
    let mentions = message["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attachment| attachment["type"] == "mentions")
        .cloned()
        .unwrap();
    assert_eq!(mentions["user_ids"], json!([BOB_GM, CAROL_GM]));
    assert_eq!(mentions["loci"], json!([[0, 4], [0, 4]]));
}

/// A picture goes out through the image service, then as an image attachment; a PDF is refused; a picture from
/// GroupMe comes in as the message's file, and one on another host is not fetched.
#[test]
fn pictures_go_through_the_image_service_and_come_in_as_files() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    let picture = bed.upload(&room, "cake.png", "image/png", b"\x89PNG-cake");
    let seq = bed.send("alice", &room, "<p>Cake</p>", &[picture]);
    bed.round();
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    let uploaded = bed.fake.seen("POST", "/pictures");
    assert_eq!(uploaded.len(), 1);
    assert_eq!(uploaded[0].body, b"\x89PNG-cake");
    assert_eq!(uploaded[0].header("content-type"), Some("image/png"));
    assert_eq!(uploaded[0].header("x-access-token"), Some(token().as_str()));
    let message = bed.fake.seen("POST", &format!("/groups/{GROUP}/messages"))[0].json()["message"].clone();
    assert_eq!(message["attachments"][0]["type"], "image");
    assert!(message["attachments"][0]["url"]
        .as_str()
        .unwrap()
        .ends_with("/media/pic1.png"));

    let pdf = bed.upload(&room, "menu.pdf", "application/pdf", b"%PDF-1.4");
    let refused = bed.send("alice", &room, "<p>Menu</p>", &[pdf]);
    bed.round();
    assert_eq!(bed.delivery(&room, refused).1, "fileNotCarried");

    let url = bed.fake.file("in.png", "image/png", b"\x89PNG-in");
    let mut with_picture = line("g9", GROUP, BOB_GM, "Bob", "Look");
    with_picture["subject"]["attachments"] = json!([{"type": "image", "url": url},
        {"type": "image", "url": "https://elsewhere.example/x.png"}]);
    bed.fake.push(with_picture);
    bed.step();
    let seq = bed.seq_of(&format!("g:{GROUP}"), "g9");
    assert_eq!(
        bed.count(
            "moqui.chat.ChatAttachment",
            &[("chatRoomId", text(&room)), ("eventSeq", Value::I64(seq))]
        ),
        1
    );
    assert!(bed
        .adapter
        .fetch_media(&bed.info(), "https://elsewhere.example/x.png", 1000)
        .is_err());
}

/// A Topic is a room of its own, "Group › Topic", and a message to it goes to the Topic's id.
#[test]
fn a_topic_is_a_room_of_its_own() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let mut in_topic = line("t1", TOPIC, BOB_GM, "Bob", "Flights booked");
    in_topic["subject"]["parent_id"] = json!(GROUP);
    bed.fake.push(in_topic);
    bed.step();
    let room = bed.room(&format!("g:{TOPIC}"));
    assert_eq!(
        bed.one("moqui.chat.ChatRoom", &[("chatRoomId", text(&room))]).unwrap()["roomName"],
        "Family › Trips"
    );
    assert_eq!(bed.messages(&room).len(), 1);
    let seq = bed.send("alice", &room, "<p>Great</p>", &[]);
    bed.round();
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    assert_eq!(bed.fake.seen("POST", &format!("/groups/{TOPIC}/messages")).len(), 1);
}

/// A direct message is a room of its own; the reply goes to the person, and a heart to their chat.
#[test]
fn a_direct_message_is_a_room_of_its_own() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    bed.fake.push(json!({"type": "direct_message.create", "alert": "Bob: hi", "subject": {"id": "d1",
        "chat_id": format!("{OWN}+{BOB_GM}"), "sender_id": BOB_GM, "user_id": BOB_GM, "recipient_id": OWN, "name": "Bob",
        "text": "Are you free?", "created_at": 1_700_000_000, "attachments": [], "favorited_by": []}}));
    bed.step();
    let room = bed.room(&format!("dm:{BOB_GM}"));
    assert_eq!(
        bed.one("moqui.chat.ChatRoom", &[("chatRoomId", text(&room))]).unwrap()["roomName"],
        "Bob"
    );
    let mapping = bed
        .one("moqui.chat.ChatRoomBridge", &[("chatRoomId", text(&room))])
        .unwrap();
    assert_eq!(mapping["externalThreadId"], format!("{OWN}+{BOB_GM}"));
    let seq = bed.send("alice", &room, "<p>Yes</p>", &[]);
    bed.round();
    assert_eq!(bed.delivery(&room, seq).0, "CdsAccepted");
    let sent = bed.fake.seen("POST", "/direct_messages")[0].json();
    assert_eq!(sent["direct_message"]["recipient_id"], BOB_GM);
    let target = bed.seq_of(&format!("dm:{BOB_GM}"), "d1");
    bed.react(&room, target, "❤️", "add");
    bed.round();
    assert_eq!(
        bed.fake
            .seen("POST", &format!("/messages/{OWN}+{BOB_GM}/d1/like"))
            .len(),
        1
    );
}

/// An expired token: a send waits with the bridge not connected, the push channel marks it not connected with what
/// to do, and a new token connects it again.
#[test]
fn an_expired_token_marks_the_bridge_not_connected_until_a_new_one() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    let new_token = format!("{}{}", "gmNewToken", "fedcba9876543210");
    bed.fake.set_token(&new_token);
    let seq = bed.send("alice", &room, "<p>Still there?</p>", &[]);
    bed.round();
    let (state, _, _, error) = bed.delivery(&room, seq);
    assert_eq!(state, "CdsPending");
    assert!(error.contains("refused the access token"), "{error}");
    assert_eq!(bed.status().0, "CbsNotConnected");
    // the push channel: a new session asks /users/me again and is refused
    bed.session.borrow_mut().reset();
    assert_eq!(bed.step(), Step::Wait(Duration::from_secs(300)));
    let (status, error) = bed.status();
    assert_eq!(status, "CbsNotConnected");
    assert_eq!(error, REFUSED);
    // the new token, as Settings, Chat (connect#MyGroupMe) keeps it: the bridge not connected until it subscribes
    let new_token_of = |token: &str| {
        let mut config = config(&bed.fake.base);
        config["accessToken"] = json!(token);
        let _authz = bed.ctx.disable_authz();
        let mut row = bed
            .facade
            .find("moqui.chat.ChatBridge")
            .unwrap()
            .eq("chatBridgeId", text(BRIDGE))
            .one()
            .unwrap()
            .unwrap();
        row.set("configJson", text(&config.to_string()));
        row.set("statusId", text("CbsNotConnected"));
        row.update(&bed.facade).unwrap();
    };
    new_token_of(&new_token);
    bed.subscribe();
    assert_eq!(bed.status().0, "CbsConnected");
    // and a token replaced while connected: connected again once subscribed with it
    let third = format!("{}{}", "gmThirdToken", "0a1b2c3d4e5f6789");
    bed.fake.set_token(&third);
    new_token_of(&third);
    bed.subscribe();
    assert_eq!(bed.status().0, "CbsConnected");
}

/// GroupMe's rate limits (420, 429) leave a send to be tried again; the push channel backs off further on each
/// failure, and from the third the bridge is not connected.
#[test]
fn rate_limits_are_tried_again_later() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    let path = format!("POST /groups/{GROUP}/messages");
    bed.fake.script(
        &path,
        Reply::Status(
            420,
            json!({"meta": {"code": 420, "errors": ["Enhance your calm"]}}).to_string(),
        ),
    );
    let seq = bed.send("alice", &room, "<p>One</p>", &[]);
    bed.round();
    let (state, _, _, error) = bed.delivery(&room, seq);
    assert_eq!(state, "CdsPending");
    assert!(error.contains("420"), "{error}");
    assert_eq!(bed.status().0, "CbsConnected", "a rate limit is not a lost connection");

    for _ in 0..3 {
        bed.fake.script("POST /faye", Reply::Status(503, String::new()));
    }
    let waits: Vec<Step> = (0..3).map(|_| bed.step()).collect();
    // the first fails the poll; each later one fails the new handshake
    assert_eq!(
        waits,
        vec![
            Step::Wait(Duration::from_secs(2)),
            Step::Wait(Duration::from_secs(4)),
            Step::Wait(Duration::from_secs(8))
        ]
    );
    assert_eq!(bed.status().0, "CbsNotConnected");
}

/// After a reconnect the catch-up fetches the messages after the room's newest; a send whose answer was lost is found
/// by its source_guid.
#[test]
fn a_reconnect_catches_up_and_a_lost_answer_is_found() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    bed.subscribe();
    let room = bed.room(&format!("g:{GROUP}"));
    let first = line("g1", GROUP, BOB_GM, "Bob", "One");
    bed.fake.message(GROUP, first["subject"].clone());
    bed.fake.push(first);
    bed.step();
    for (id, words) in [("g2", "Two"), ("g3", "Three")] {
        bed.fake
            .message(GROUP, line(id, GROUP, CAROL_GM, "Carol", words)["subject"].clone());
    }
    bed.session.borrow_mut().reset();
    assert_eq!(bed.step(), Step::Subscribed(2));
    assert!(
        bed.fake
            .seen("GET", &format!("/groups/{GROUP}/messages?after_id=g1"))
            .len()
            == 1
    );
    assert_eq!(bed.messages(&room).len(), 3);

    bed.fake.script(&format!("POST /groups/{GROUP}/messages"), Reply::Drop);
    let seq = bed.send("alice", &room, "<p>Lost?</p>", &[]);
    bed.round();
    let (state, _, remote, _) = bed.delivery(&room, seq);
    assert_eq!(state, "CdsAccepted");
    assert!(remote.starts_with('m'), "{remote}");
}

/// A user connects their own GroupMe (one bridge, theirs, the token never shown), connects again with no token, and
/// disconnects (the rooms stay, the token is forgotten); with the property off they cannot; an administrator's
/// GroupMe bridge needs a token and an owner, keeps its owner once it has rooms, and is one per owner.
#[test]
fn a_user_connects_their_own_groupme_and_an_administrator_sets_one_up() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    login("bob");
    let refused = call(
        "org.moqui.chat.GroupMeServices.connect#MyGroupMe",
        &[("accessToken", text("not a token!"))],
    );
    assert_eq!(out_text(&refused, "reasonCode"), "badToken");
    errors();
    let bob_token = format!("{}{}", "gmBobToken", "00112233445566778899");
    let out = ok(
        "org.moqui.chat.GroupMeServices.connect#MyGroupMe",
        &[("accessToken", text(&bob_token))],
    );
    let bridge = out_text(&out, "chatBridgeId");
    let saved = bed
        .one("moqui.chat.ChatBridge", &[("chatBridgeId", text(&bridge))])
        .unwrap();
    assert_eq!(saved["typeEnumId"], "CbtGroupMe");
    let kept: Json = serde_json::from_str(&saved["configJson"]).unwrap();
    assert_eq!(kept["ownerUserId"], "bob");
    assert_eq!(kept["accessToken"], bob_token.as_str());
    let mine = ok("org.moqui.chat.GroupMeServices.get#MyGroupMe", &[]);
    assert!(!format!("{mine:?}").contains(&bob_token), "the token is never shown");
    assert_eq!(out_text(&mine, "hasToken"), "true");
    // again with no token: the same bridge, the token kept
    let again = ok("org.moqui.chat.GroupMeServices.connect#MyGroupMe", &[]);
    assert_eq!(out_text(&again, "chatBridgeId"), bridge);
    assert_eq!(
        bed.count("moqui.chat.ChatBridge", &[("typeEnumId", text("CbtGroupMe"))]),
        2
    );

    ok("org.moqui.chat.GroupMeServices.disconnect#MyGroupMe", &[]);
    let saved = bed
        .one("moqui.chat.ChatBridge", &[("chatBridgeId", text(&bridge))])
        .unwrap();
    assert_eq!(saved["statusId"], "CbsDisabled");
    assert!(!saved["configJson"].contains(&bob_token));
    assert!(saved["lastErrorText"].contains("keep their history"));
    let mine = ok("org.moqui.chat.GroupMeServices.get#MyGroupMe", &[]);
    assert_eq!(out_text(&mine, "statusText"), "Disconnected");

    moqui_lib::env::set_property("chat_groupme_self_service", "false");
    let off = call(
        "org.moqui.chat.GroupMeServices.connect#MyGroupMe",
        &[("accessToken", text(&bob_token))],
    );
    moqui_lib::env::set_property("chat_groupme_self_service", "true");
    assert_eq!(out_text(&off, "reasonCode"), "selfServiceOff");
    errors();

    // an administrator: alice's bridge GM1 has rooms, so it keeps its owner; bob has one already
    bed.subscribe();
    for (entity, fields) in [
        (
            "moqui.security.UserAccount",
            vec![("userId", text("admin")), ("username", text("admin"))],
        ),
        ("moqui.security.UserGroup", vec![("userGroupId", text("BRIDGE_ADMIN"))]),
        (
            "moqui.security.UserGroupMember",
            vec![
                ("userGroupId", text("BRIDGE_ADMIN")),
                ("userId", text("admin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
        (
            "moqui.security.UserGroupPermission",
            vec![
                ("userGroupId", text("BRIDGE_ADMIN")),
                ("userPermissionId", text("ChatBridgeAdmin")),
                ("fromDate", Value::I64(0)),
            ],
        ),
        (
            "moqui.basic.Enumeration",
            vec![
                ("enumId", text("CbtGroupMe")),
                ("enumCode", text("groupme")),
                ("enumTypeId", text("ChatBridgeType")),
            ],
        ),
    ] {
        row(&bed.facade, entity, &fields);
    }
    login("admin");
    let store = |fields: &[(&str, Value)]| {
        call("org.moqui.chat.ChatBridgeServices.store#Bridge", fields);
        errors()
    };
    let error = store(&[
        ("name", text("No token")),
        ("typeEnumId", text("CbtGroupMe")),
        ("adapterName", text("groupme")),
        ("configJson", text(&json!({"ownerUserId": "alice"}).to_string())),
    ]);
    assert!(error.contains("accessToken"), "{error}");
    let mut moved = config(&bed.fake.base);
    moved["ownerUserId"] = json!("bob");
    let error = store(&[("chatBridgeId", text(BRIDGE)), ("configJson", text(&moved.to_string()))]);
    assert!(error.contains("keeps its owner"), "{error}");
    let error = store(&[
        ("name", text("Bob's second")),
        ("typeEnumId", text("CbtGroupMe")),
        ("adapterName", text("groupme")),
        (
            "configJson",
            text(&json!({"ownerUserId": "bob", "accessToken": bob_token}).to_string()),
        ),
    ]);
    assert!(error.contains("has a GroupMe bridge already"), "{error}");
}

/// One node holds a bridge's push connection: another asks and is refused while the lease lasts.
#[test]
fn one_node_holds_the_connection() {
    let bed = bed();
    let _entered = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    assert_eq!(super::push::claim(&bed.facade, BRIDGE, "node-a"), Some(true));
    assert_eq!(super::push::claim(&bed.facade, BRIDGE, "node-b"), Some(false));
    assert_eq!(super::push::claim(&bed.facade, BRIDGE, "node-a"), Some(true), "renewed");
}
