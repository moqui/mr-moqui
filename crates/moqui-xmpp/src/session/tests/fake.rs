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

//! An in-memory backend for the session tests: two users with app passwords, rooms with timelines, and the pushes a
//! send makes, through the registry as the host's notification bus would. It answers the services by name with the
//! shapes `XmppServices` and `ChatServices` give; the integration test (`it xmpp::`) runs the real ones.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::{json, Value as Json};

use crate::backend::{services, text, Backend, BoxFuture, CallError, Caller};
use crate::{scram, Server};

pub const JOHN_PASSWORD: &str = "abcd-efgh-jkmn-pqrs-tuvw";
pub const JANE_PASSWORD: &str = "wxyz-2345-6789-abcd-efgh";

pub struct Room {
    pub id: String,
    pub direct: bool,
    pub public: bool,
    pub encrypted: bool,
    pub bridged: bool,
    pub topic: String,
    pub members: Vec<String>,
    pub events: Vec<Json>,
}

/// A node's access model and its items (id, payload).
pub type PepNode = (String, Vec<(String, String)>);

#[derive(Default)]
pub struct State {
    pub rooms: Vec<Room>,
    pub fast: Vec<(String, String, String, i64)>,
    pub pep: HashMap<(String, String), PepNode>,
    pub calls: Vec<String>,
    pub statuses: HashMap<String, String>,
    pub push: Vec<(String, Json)>,
}

pub struct Fake {
    pub server: OnceLock<Weak<Server>>,
    pub state: Mutex<State>,
}

fn users() -> [(&'static str, &'static str, &'static str); 2] {
    [("U1", "john.doe", JOHN_PASSWORD), ("U2", "jane", JANE_PASSWORD)]
}

const SALT: &[u8] = b"fixed-test-salt!";

impl Fake {
    pub fn new() -> Arc<Fake> {
        let state = State {
            rooms: vec![
                Room {
                    id: "r1".to_string(),
                    direct: false,
                    public: false,
                    encrypted: false,
                    bridged: false,
                    topic: "Plans".to_string(),
                    members: vec!["U1".to_string(), "U2".to_string()],
                    events: Vec::new(),
                },
                Room {
                    id: "SECRET".to_string(),
                    direct: false,
                    public: false,
                    encrypted: true,
                    bridged: false,
                    topic: String::new(),
                    members: vec!["U1".to_string(), "U2".to_string()],
                    events: Vec::new(),
                },
                Room {
                    id: "bridged".to_string(),
                    direct: false,
                    public: false,
                    encrypted: false,
                    bridged: true,
                    topic: String::new(),
                    members: vec!["U1".to_string(), "U2".to_string()],
                    events: Vec::new(),
                },
            ],
            ..State::default()
        };
        Arc::new(Fake {
            server: OnceLock::new(),
            state: Mutex::new(state),
        })
    }

    fn user_id_of(username: &str) -> Option<&'static str> {
        users()
            .into_iter()
            .find(|(_, name, _)| name.eq_ignore_ascii_case(username))
            .map(|(id, _, _)| id)
    }

    fn username_of(user_id: &str) -> Option<&'static str> {
        users()
            .into_iter()
            .find(|(id, _, _)| *id == user_id)
            .map(|(_, name, _)| name)
    }

    fn push(&self, room: &Room, message: Json) {
        let body = json!({"topic": format!("chatRoom:{}", room.id), "userIds": room.members, "message": message});
        if let Some(server) = self.server.get().and_then(Weak::upgrade) {
            server.registry.deliver_notice(&body);
        }
    }

    fn append(
        &self,
        state: &mut State,
        room_id: &str,
        sender: &str,
        kind: &str,
        fields: Json,
    ) -> Result<Json, CallError> {
        let room = state
            .rooms
            .iter_mut()
            .find(|room| room.id == room_id)
            .ok_or_else(|| CallError("no room".to_string()))?;
        if !room.members.iter().any(|member| member == sender) {
            return Err(CallError("not a member".to_string()));
        }
        let seq = room.events.len() as i64 + 1;
        let mut event = json!({"chatRoomId": room_id, "eventSeq": seq, "type": kind, "senderUserId": sender,
            "sentDate": 1_700_000_000_000_i64 + seq * 1000, "content": {}});
        if let (Some(event), Some(fields)) = (event.as_object_mut(), fields.as_object()) {
            for (key, value) in fields {
                event.insert(key.clone(), value.clone());
            }
        }
        room.events.push(event.clone());
        let pushed = json!({"kind": "event", "chatRoomId": room_id, "eventSeq": seq, "event": event});
        let snapshot = Room {
            id: room.id.clone(),
            direct: room.direct,
            public: room.public,
            encrypted: room.encrypted,
            bridged: room.bridged,
            topic: String::new(),
            members: room.members.clone(),
            events: Vec::new(),
        };
        self.push(&snapshot, pushed);
        Ok(json!({"eventSeq": seq}))
    }

    fn answer(&self, caller: &Caller, service: &str, p: &Json) -> Result<Json, CallError> {
        let me = match caller {
            Caller::User(username) => Fake::user_id_of(username).unwrap_or("").to_string(),
            Caller::Internal => String::new(),
        };
        let mut state = moqui_lib::sync::lock(&self.state);
        state.calls.push(service.to_string());
        let refuse = |text: &str| Err(CallError(text.to_string()));
        match service {
            services::ACCOUNT => {
                let username = text(p, "username");
                match Fake::user_id_of(&username) {
                    Some(id) => Ok(json!({"userId": id, "username": Fake::username_of(id), "canLogin": true})),
                    None => Ok(json!({})),
                }
            }
            services::SCRAM => {
                let username = text(p, "username");
                let Some((_, _, password)) = users()
                    .into_iter()
                    .find(|(_, name, _)| name.eq_ignore_ascii_case(&username))
                else {
                    return Ok(json!({}));
                };
                // as created: the password as shown, and without its dashes
                let shown = scram::derive(password, SALT, 4096);
                let key = scram::derive(&password.replace('-', ""), SALT, 4096);
                Ok(json!({"salt": STANDARD.encode(SALT), "iterations": 4096,
                    "storedKeyList": [STANDARD.encode(&shown.stored_key), STANDARD.encode(&key.stored_key)],
                    "serverKeyList": [STANDARD.encode(&shown.server_key), STANDARD.encode(&key.server_key)]}))
            }
            services::CHECK_PLAIN => {
                let (username, password) = (text(p, "username"), text(p, "password"));
                match users()
                    .into_iter()
                    .find(|(_, name, pw)| name.eq_ignore_ascii_case(&username) && *pw == password)
                {
                    Some((id, name, _)) => Ok(json!({"userId": id, "username": name})),
                    None => refuse("not an app password"),
                }
            }
            services::ISSUE_FAST_TOKEN => {
                let id = format!("F{}", state.fast.len() + 1);
                state.fast.push((id, text(p, "userId"), text(p, "token"), -1));
                Ok(json!({"expiry": "2030-01-01T00:00:00Z"}))
            }
            services::FAST_TOKEN => {
                let Some(user) = Fake::user_id_of(&text(p, "username")) else {
                    return Ok(json!({"tokenList": []}));
                };
                let list: Vec<Json> = state
                    .fast
                    .iter()
                    .filter(|(_, owner, _, _)| owner == user)
                    .map(|(id, _, token, _)| json!({"fastTokenId": id, "token": token}))
                    .collect();
                Ok(json!({"tokenList": list}))
            }
            services::USE_FAST_TOKEN => {
                let id = text(p, "fastTokenId");
                let count = p.get("count").and_then(Json::as_i64);
                let bound = p.get("bound").and_then(Json::as_bool).unwrap_or(false);
                let Some(token) = state.fast.iter_mut().find(|(token_id, ..)| *token_id == id) else {
                    return refuse("no token");
                };
                if !bound {
                    match count {
                        Some(count) if count > token.3 => token.3 = count,
                        _ => return refuse("a replayed count"),
                    }
                }
                Ok(json!({}))
            }
            services::INVALIDATE_FAST_TOKEN => {
                let id = text(p, "fastTokenId");
                state.fast.retain(|(token_id, ..)| *token_id != id);
                Ok(json!({}))
            }
            services::USERS => {
                let mut list = Vec::new();
                for (id, name, _) in users() {
                    let by_id = p
                        .get("userIdList")
                        .and_then(Json::as_array)
                        .is_some_and(|ids| ids.iter().any(|v| v == id));
                    let by_name = p.get("usernameList").and_then(Json::as_array).is_some_and(|names| {
                        names
                            .iter()
                            .any(|v| v.as_str().is_some_and(|v| v.eq_ignore_ascii_case(name)))
                    });
                    if by_id || by_name {
                        list.push(json!({"userId": id, "username": name}));
                    }
                }
                Ok(json!({"userList": list}))
            }
            services::ROSTER => {
                let others: Vec<String> = state
                    .rooms
                    .iter()
                    .filter(|room| room.direct && room.members.contains(&me))
                    .flat_map(|room| room.members.clone())
                    .filter(|member| *member != me)
                    .collect();
                Ok(json!({"userIdList": others}))
            }
            services::DIRECT_ROOM => {
                let other = text(p, "userId");
                let found = state
                    .rooms
                    .iter()
                    .find(|room| room.direct && room.members.contains(&me) && room.members.contains(&other));
                if let Some(room) = found {
                    return Ok(json!({"chatRoomId": room.id}));
                }
                let id = format!("D{}", state.rooms.len() + 1);
                state.rooms.push(Room {
                    id: id.clone(),
                    direct: true,
                    public: false,
                    encrypted: false,
                    bridged: false,
                    topic: String::new(),
                    members: vec![me.clone(), other],
                    events: Vec::new(),
                });
                Ok(json!({"chatRoomId": id}))
            }
            services::ROOM => {
                let id = text(p, "chatRoomId");
                let Some(room) = state
                    .rooms
                    .iter()
                    .find(|room| room.id == id && (room.public || room.members.contains(&me)))
                else {
                    return refuse("not found");
                };
                let members: Vec<Json> = room
                    .members
                    .iter()
                    .map(|member| json!({"userId": member, "username": Fake::username_of(member), "roleEnumId": "CmrMember"}))
                    .collect();
                Ok(
                    json!({"room": {"chatRoomId": room.id, "roomTypeEnumId": if room.direct { "CrtDirect" } else { "CrtPrivate" },
                    "roomName": room.id, "topic": room.topic, "encrypted": room.encrypted,
                    "visibilityEnumId": if room.public { "CrvUsers" } else { "CrvMembers" }},
                    "bridged": room.bridged, "memberList": members}),
                )
            }
            services::FIND_ROOMS => {
                let list: Vec<Json> = state
                    .rooms
                    .iter()
                    .filter(|room| !room.direct && room.members.contains(&me))
                    .map(|room| json!({"chatRoomId": room.id, "displayName": room.id, "encrypted": room.encrypted}))
                    .collect();
                Ok(json!({"roomList": list}))
            }
            services::SEND => {
                let room = text(p, "chatRoomId");
                let body = text(p, "body");
                let body_text = crate::session::chat::html_to_text(&body);
                let mut fields = json!({"body": body, "bodyText": body_text, "clientRef": text(p, "clientRef")});
                if let Some(reply) = p.get("replyToEventSeq") {
                    fields["replyToEventSeq"] = reply.clone();
                }
                self.append(&mut state, &room, &me, "message", fields)
            }
            services::STORE_ENCRYPTED => {
                let room = text(p, "chatRoomId");
                let fields = json!({"clientRef": text(p, "clientRef"),
                    "content": {"xmppEncrypted": {"xml": text(p, "xml"), "namespace": text(p, "namespace")}}});
                self.append(&mut state, &room, &me, "message", fields)
            }
            services::EDIT => {
                let room = text(p, "chatRoomId");
                let fields = json!({"editsEventSeq": p["eventSeq"], "content": {"body": text(p, "body")}});
                self.append(&mut state, &room, &me, "edit", fields)
            }
            services::REACT => {
                let room = text(p, "chatRoomId");
                let fields = json!({"relatesToEventSeq": p["eventSeq"], "content": {"key": text(p, "key")}});
                self.append(&mut state, &room, &me, "reaction", fields)
            }
            services::FIND_EVENT => {
                let room_id = text(p, "chatRoomId");
                let Some(room) = state.rooms.iter().find(|room| room.id == room_id) else {
                    return refuse("no room");
                };
                let client_ref = text(p, "clientRef");
                let seq = p.get("eventSeq").and_then(Json::as_i64);
                let event = room.events.iter().find(|event| {
                    (seq.is_some() && event["eventSeq"].as_i64() == seq)
                        || (!client_ref.is_empty() && text(event, "clientRef") == client_ref)
                });
                let Some(event) = event else {
                    return refuse("no event");
                };
                let target = event["eventSeq"].as_i64();
                let reactor = text(p, "reactorUserId");
                let reactions_of = |user: &str| -> Vec<String> {
                    room.events
                        .iter()
                        .filter(|e| {
                            text(e, "type") == "reaction"
                                && e["relatesToEventSeq"].as_i64() == target
                                && text(e, "senderUserId") == user
                        })
                        .map(|e| text(&e["content"], "key"))
                        .collect()
                };
                Ok(
                    json!({"eventSeq": target, "clientRef": text(event, "clientRef"), "senderUserId": text(event, "senderUserId"),
                    "myReactions": reactions_of(&me), "reactorReactions": reactions_of(&reactor)}),
                )
            }
            services::ARCHIVE => {
                let room_id = text(p, "chatRoomId");
                if room_id.is_empty() {
                    // the user's own archive: the direct rooms (none have messages in these tests' archive cases)
                    return Ok(json!({"eventList": [], "complete": true, "count": 0}));
                }
                let Some(room) = state
                    .rooms
                    .iter()
                    .find(|room| room.id == room_id && room.members.contains(&me))
                else {
                    return refuse("not found");
                };
                let mut events: Vec<Json> = room
                    .events
                    .iter()
                    .filter(|e| text(e, "type") == "message")
                    .cloned()
                    .collect();
                if let Some(after) = p.get("afterSeq").and_then(Json::as_i64) {
                    events.retain(|e| e["eventSeq"].as_i64().unwrap_or(0) > after);
                }
                if let Some(before) = p.get("beforeSeq").and_then(Json::as_i64) {
                    events.retain(|e| e["eventSeq"].as_i64().unwrap_or(0) < before);
                }
                let limit = p.get("limit").and_then(Json::as_i64).unwrap_or(50) as usize;
                let total = events.len();
                let latest = p.get("latest").and_then(Json::as_bool).unwrap_or(false);
                let page: Vec<Json> = if latest {
                    events[total.saturating_sub(limit)..].to_vec()
                } else {
                    events.into_iter().take(limit).collect()
                };
                let complete = page.len() == total;
                Ok(json!({"eventList": page, "complete": complete, "count": total}))
            }
            services::PEP_PUBLISH => {
                let node = text(p, "node");
                let access = text(&p["options"], "access_model");
                let key = (me.clone(), node);
                let entry = state.pep.entry(key).or_insert_with(|| {
                    (
                        if access.is_empty() {
                            "presence".to_string()
                        } else {
                            access.clone()
                        },
                        Vec::new(),
                    )
                });
                if !access.is_empty() && entry.0 != access {
                    return Ok(json!({"preconditionFailed": true}));
                }
                let item_id = text(p, "itemId");
                entry.1.retain(|(id, _)| *id != item_id);
                entry.1.push((item_id, text(p, "payloadXml")));
                let notify: Vec<&str> = if entry.0 == "whitelist" {
                    vec![]
                } else {
                    vec!["U1", "U2"]
                };
                Ok(json!({"notifyUserIdList": notify}))
            }
            services::PEP_ITEMS => {
                let owner = p.get("ownerUserId").and_then(Json::as_str).unwrap_or(&me).to_string();
                let Some((access, items)) = state.pep.get(&(owner.clone(), text(p, "node"))) else {
                    return Ok(json!({"exists": false}));
                };
                if access == "whitelist" && owner != me {
                    return Ok(json!({"exists": true, "denied": true}));
                }
                let list: Vec<Json> = items
                    .iter()
                    .map(|(id, xml)| json!({"itemId": id, "payloadXml": xml}))
                    .collect();
                Ok(json!({"exists": true, "itemList": list, "accessModel": access, "maxItems": "1"}))
            }
            services::UPLOAD_SLOT => Ok(json!({"token": "tok123", "urlName": text(p, "fileName")})),
            services::ICE_SERVERS => Ok(
                json!({"serviceList": [{"type": "turn", "transport": "udp", "host": "turn.moqui.local",
                "port": 3478, "username": "1:user:U1", "password": "pw", "expires": "2030-01-01T00:00:00Z"}]}),
            ),
            services::PUSH_ENABLE => {
                state.push.push((
                    me,
                    json!({"pushJid": text(p, "pushJid"), "node": text(p, "node"), "options": p["options"]}),
                ));
                Ok(json!({}))
            }
            services::PUSH_TARGETS => {
                let user = text(p, "userId");
                let list: Vec<Json> = state
                    .push
                    .iter()
                    .filter(|(owner, _)| *owner == user)
                    .map(|(_, target)| target.clone())
                    .collect();
                Ok(json!({"targetList": list}))
            }
            services::BLOCKS => Ok(json!({"jidList": []})),
            services::STATUSES => Ok(json!({"statusMap": {}})),
            services::SET_STATUS => {
                state.statuses.insert(me, text(p, "status"));
                Ok(json!({}))
            }
            _ => Ok(json!({})),
        }
    }
}

impl Backend for Fake {
    fn call(&self, caller: Caller, service: &str, parameters: Json) -> BoxFuture<'_, Result<Json, CallError>> {
        let result = self.answer(&caller, service, &parameters);
        Box::pin(async move { result })
    }
}
