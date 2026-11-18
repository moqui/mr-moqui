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

//! What comes in from GroupMe, from the push channel or the catch-up, as the bridge layer's input
//! (plans/Comms-Messaging.md §18.3 phase 14, "In"):
//!
//! - a message (`line.create`, `direct_message.create`, a caught-up one) is a message of the GroupMe person into the
//!   room of its group, Topic or conversation (`GroupMeServices.ensure#Room` first), one `ingest#Event` batch
//!   `gm:<message id>` under the bridge's transport mark: its pictures and videos as `media`, a place and a file as a
//!   line of text, a `reply` attachment as its target, the word `@all` or `@everyone` as `@room`; its `favorited_by` to
//!   `receive#Likes`;
//! - `favorite` is the message's likes, as a state (`receive#Likes`);
//! - `message.update` and `message.deleted` (community-documented) are an edit and a delete;
//! - `membership.create` makes the new group's room;
//! - a system message, the owner's own echo of a send (its `source_guid` ours), `typing`, `ping` and the rest are
//!   left.
//!
//! Each event is on its own: one that fails is logged with the bridge and GroupMe's id, and the rest go on.

use std::collections::{HashMap, HashSet};

use moqui_core::entity::EntityFacade;
use moqui_core::service::chat_bridge::BridgeInfo;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

use super::api::id_of;
use super::{parameters, Config, Inner, Room, MEDIA_HOSTS, OUR_GUID};

/// The pages of groups, Topics or messages a catch-up reads at most, of 100 each.
const PAGES: usize = 5;

/// What a session knows of the account's rooms: the names GroupMe gave, and the rooms made this session.
#[derive(Debug, Default)]
pub struct Rooms {
    /// A group's name, by its id.
    pub groups: HashMap<String, String>,
    /// A Topic's (its group's id, its name), by its id.
    pub topics: HashMap<String, (String, String)>,
    /// A person's name, by their id.
    pub people: HashMap<String, String>,
    /// The groups whose Topics were read.
    pub topics_read: HashSet<String>,
    /// The external room ids `ensure#Room` answered this session.
    pub made: HashSet<String>,
}

/// Where a message goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub room: Room,
    pub kind: &'static str,
    pub name: String,
    pub parent_name: Option<String>,
    pub conversation: Option<String>,
}

/// One bridge's input, as one session works it.
pub struct Inbound<'a> {
    pub inner: &'a Inner,
    pub bridge: &'a BridgeInfo,
    pub config: &'a Config,
    /// The token's GroupMe user id.
    pub own: &'a str,
}

fn text_field(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Whether `text` has the word `@all` or `@everyone`.
pub fn mentions_all(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["@all", "@everyone"].iter().any(|word| {
        lower.match_indices(word).any(|(at, found)| {
            let before = lower[..at].chars().next_back();
            let after = lower[at + found.len()..].chars().next();
            !before.is_some_and(|c| c.is_alphanumeric() || c == '@' || c == '_')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    })
}

/// A place as a line of text, with an OpenStreetMap link built from its coordinates (never taken from GroupMe).
fn place_text(place: &Json) -> String {
    let number = |key: &str| {
        place.get(key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse::<f64>().ok()))
        })
    };
    let name: String = text_field(place, "name")
        .unwrap_or_else(|| "a place".to_string())
        .chars()
        .take(200)
        .collect();
    let mut text = format!("Location: {name}");
    if let (Some(lat), Some(lng)) = (number("lat"), number("lng")) {
        if lat.is_finite() && lng.is_finite() && lat.abs() <= 90.0 && lng.abs() <= 180.0 {
            text.push_str(&format!(
                "\nhttps://www.openstreetmap.org/?mlat={lat}&mlon={lng}#map=17/{lat}/{lng}"
            ));
        }
    }
    text
}

impl Inbound<'_> {
    fn id(&self) -> &str {
        &self.bridge.chat_bridge_id
    }

    /// Whether `url` is on GroupMe's file hosts (or a test's fake's).
    fn media_url(&self, url: &str) -> bool {
        MEDIA_HOSTS.iter().any(|host| url.starts_with(host))
            || self
                .config
                .media_base
                .as_deref()
                .is_some_and(|base| url.starts_with(&format!("{base}/")))
    }

    /// One push message's `data`: the events taken.
    pub fn handle(&self, rooms: &mut Rooms, data: &Json) -> usize {
        let subject = data.get("subject").unwrap_or(&Json::Null);
        match data.get("type").and_then(Json::as_str).unwrap_or_default() {
            "line.create" | "direct_message.create" => usize::from(self.message(rooms, subject)),
            "favorite" => usize::from(self.likes(rooms, subject.get("line").unwrap_or(&Json::Null))),
            "message.update" => usize::from(self.changed(rooms, subject, false)),
            "message.deleted" => usize::from(self.changed(rooms, subject, true)),
            "membership.create" => {
                let Some(group) = id_of(subject, "id") else {
                    return 0;
                };
                if !self.config.chosen(&group) {
                    return 0;
                }
                let name = text_field(subject, "name").unwrap_or_default();
                rooms.groups.insert(group.clone(), name.clone());
                let target = Target {
                    room: Room::Group(group),
                    kind: "group",
                    name,
                    parent_name: None,
                    conversation: None,
                };
                usize::from(self.ensure(rooms, &target))
            }
            _ => 0,
        }
    }

    /// The name of group `group`, from what the session knows, else GroupMe.
    fn group_name(&self, rooms: &mut Rooms, group: &str) -> String {
        if let Some(name) = rooms.groups.get(group) {
            return name.clone();
        }
        let name = self
            .inner
            .api(self.config)
            .get(&format!("/groups/{group}"))
            .ok()
            .and_then(|found| text_field(&found, "name"))
            .unwrap_or_default();
        rooms.groups.insert(group.to_string(), name.clone());
        name
    }

    /// The Topics of group `parent`, read once a session (GroupMe's subgroups, community-documented).
    fn read_topics(&self, rooms: &mut Rooms, parent: &str) -> Vec<Json> {
        rooms.topics_read.insert(parent.to_string());
        let found = match self
            .inner
            .api(self.config)
            .get(&format!("/groups/{parent}/subgroups?per_page=100"))
        {
            Ok(found) => found,
            Err(error) => {
                tracing::debug!(
                    "GroupMe bridge {}: the Topics of {parent} were not read: {}",
                    self.id(),
                    error.text()
                );
                return Vec::new();
            }
        };
        let list = found
            .as_array()
            .cloned()
            .or_else(|| found.get("subgroups").and_then(Json::as_array).cloned())
            .unwrap_or_default();
        for topic in &list {
            if let Some(id) = id_of(topic, "id") {
                let name = text_field(topic, "topic")
                    .or_else(|| text_field(topic, "name"))
                    .unwrap_or_default();
                rooms.topics.insert(id, (parent.to_string(), name));
            }
        }
        list
    }

    /// Where message `message` goes, when the bridge carries its room.
    pub fn target(&self, rooms: &mut Rooms, message: &Json) -> Option<Target> {
        let group = id_of(message, "group_id");
        let conversation = id_of(message, "chat_id").or_else(|| id_of(message, "conversation_id"));
        let direct = group.is_none()
            && (message.get("recipient_id").is_some() || conversation.as_deref().is_some_and(|id| id.contains('+')));
        if direct {
            if !self.config.direct_messages {
                return None;
            }
            let sender = id_of(message, "sender_id").or_else(|| id_of(message, "user_id"))?;
            let other = if sender == self.own {
                id_of(message, "recipient_id")?
            } else {
                sender.clone()
            };
            if sender != self.own {
                if let Some(name) = text_field(message, "name") {
                    rooms.people.insert(other.clone(), name);
                }
            }
            return Some(Target {
                room: Room::Direct(other.clone()),
                kind: "direct",
                name: rooms.people.get(&other).cloned().unwrap_or_default(),
                parent_name: None,
                conversation,
            });
        }
        let group = group?;
        let parent = id_of(message, "parent_id").filter(|parent| *parent != group);
        if let Some(parent) = parent {
            if !self.config.topics || !self.config.chosen(&parent) {
                return None;
            }
            if !rooms.topics.contains_key(&group) && !rooms.topics_read.contains(&parent) {
                self.read_topics(rooms, &parent);
            }
            let name = rooms
                .topics
                .get(&group)
                .map(|(_, name)| name.clone())
                .unwrap_or_default();
            let parent_name = self.group_name(rooms, &parent);
            return Some(Target {
                room: Room::Group(group),
                kind: "topic",
                name,
                parent_name: Some(parent_name),
                conversation: None,
            });
        }
        if let Some((parent, name)) = rooms.topics.get(&group).cloned() {
            if !self.config.topics || !self.config.chosen(&parent) {
                return None;
            }
            let parent_name = self.group_name(rooms, &parent);
            return Some(Target {
                room: Room::Group(group),
                kind: "topic",
                name,
                parent_name: Some(parent_name),
                conversation: None,
            });
        }
        if !self.config.chosen(&group) {
            return None;
        }
        let name = self.group_name(rooms, &group);
        Some(Target {
            room: Room::Group(group),
            kind: "group",
            name,
            parent_name: None,
            conversation: None,
        })
    }

    /// The room of `target`, found or made (`ensure#Room`), once a session: whether it is there.
    pub fn ensure(&self, rooms: &mut Rooms, target: &Target) -> bool {
        let external = target.room.external_id();
        if rooms.made.contains(&external) {
            return true;
        }
        let mut call = json!({"chatBridgeId": self.id(), "externalRoomId": external, "kind": target.kind,
            "name": target.name});
        if let Some(parent) = &target.parent_name {
            call["parentName"] = Json::String(parent.clone());
        }
        if let Some(conversation) = &target.conversation {
            call["conversationId"] = Json::String(conversation.clone());
        }
        match crate::call("org.moqui.chat.GroupMeServices.ensure#Room", parameters(&call)) {
            Ok(out) if out.get("chatRoomId").is_some_and(|id| !to_plain_string(id).is_empty()) => {
                rooms.made.insert(external);
                true
            }
            Ok(out) => {
                tracing::warn!(
                    "GroupMe bridge {}: no room for {external}: {}",
                    self.id(),
                    out.get("reasonCode").map(to_plain_string).unwrap_or_default()
                );
                false
            }
            Err(error) => {
                tracing::error!(
                    "GroupMe bridge {}: the room of {external} was not made: {error}",
                    self.id()
                );
                false
            }
        }
    }

    /// A batch of `items` to `ingest#Event` as transaction `transaction`, under the bridge's transport mark: whether
    /// it was taken (stored; each item's own outcome is logged when it is refused).
    fn ingest(&self, transaction: &str, items: Vec<Json>) -> bool {
        let call = json!({"chatBridgeId": self.id(), "transactionId": transaction, "transportVerified": true,
            "itemList": items});
        let result = crate::sms::with_mark(self.id(), || {
            crate::call("org.moqui.chat.ChatBridgeServices.ingest#Event", parameters(&call))
        });
        match result {
            Ok(out) => {
                if let Some(Value::List(outcomes)) = out.get("outcomeList") {
                    for outcome in outcomes {
                        let outcome = moqui_lib::json::to_json(outcome);
                        if outcome.get("outcome").and_then(Json::as_str) == Some("rejected") {
                            tracing::warn!(
                                "GroupMe bridge {}: {transaction} was refused: {} {}",
                                self.id(),
                                outcome.get("reasonCode").and_then(Json::as_str).unwrap_or_default(),
                                outcome.get("message").and_then(Json::as_str).unwrap_or_default()
                            );
                        }
                    }
                }
                true
            }
            Err(error) => {
                tracing::error!("GroupMe bridge {}: {transaction} was not taken: {error}", self.id());
                false
            }
        }
    }

    /// A message from GroupMe: whether it was taken.
    pub fn message(&self, rooms: &mut Rooms, message: &Json) -> bool {
        let Some(id) = id_of(message, "id") else {
            return false;
        };
        if message.get("system").and_then(Json::as_bool) == Some(true) {
            return false;
        }
        let Some(sender) = id_of(message, "sender_id").or_else(|| id_of(message, "user_id")) else {
            return false;
        };
        // the echo of a send of ours, which may come before the send's mapping is kept
        if sender == self.own
            && message
                .get("source_guid")
                .and_then(Json::as_str)
                .is_some_and(|guid| guid.starts_with(OUR_GUID))
        {
            return false;
        }
        let Some(target) = self.target(rooms, message) else {
            return false;
        };
        if !self.ensure(rooms, &target) {
            return false;
        }
        let external = target.room.external_id();
        let mut lines: Vec<String> = text_field(message, "text").into_iter().collect();
        let mut media: Vec<Json> = Vec::new();
        let mut reply = None;
        for attachment in message
            .get("attachments")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            match attachment.get("type").and_then(Json::as_str).unwrap_or_default() {
                kind @ ("image" | "video") => {
                    if let Some(url) = text_field(attachment, "url").filter(|url| self.media_url(url)) {
                        if media.len() < 10 {
                            media.push(json!({"url": url, "contentType": if kind == "video" { Json::from("video/mp4") } else { Json::Null },
                                "maxBytes": self.config.max_inbound_media_bytes}));
                        }
                    }
                }
                "location" => lines.push(place_text(attachment)),
                "file" => lines.push("[A file on GroupMe: open GroupMe to get it]".to_string()),
                "reply" => reply = id_of(attachment, "reply_id"),
                _ => {}
            }
        }
        let text = lines.join("\n");
        if text.trim().is_empty() && media.is_empty() {
            return false;
        }
        let mut item = json!({"itemId": id, "type": "message", "externalRoomId": external, "externalEventId": id,
            "externalUserId": sender, "text": text, "media": media});
        if let Some(name) = text_field(message, "name") {
            item["displayName"] = Json::String(name.chars().take(200).collect());
        }
        if let Some(avatar) = text_field(message, "avatar_url").filter(|url| url.starts_with("https://i.groupme.com/"))
        {
            item["avatarUrl"] = Json::String(avatar);
        }
        if let Some(at) = message.get("created_at").and_then(Json::as_i64) {
            item["originDate"] = json!(at.saturating_mul(1000));
        }
        if let Some(reply) = reply {
            item["replyToExternalEventId"] = Json::String(reply);
        }
        if mentions_all(&text) {
            item["mentions"] = json!(["@room"]);
        }
        if item.get("media").and_then(Json::as_array).is_some_and(Vec::is_empty) {
            if let Some(object) = item.as_object_mut() {
                object.remove("media");
            }
        }
        let taken = self.ingest(&format!("gm:{id}"), vec![item]);
        if message
            .get("favorited_by")
            .and_then(Json::as_array)
            .is_some_and(|likers| !likers.is_empty())
        {
            self.likes(rooms, message);
        }
        taken
    }

    /// The likes of a message, as a state (`receive#Likes`): whether they changed anything.
    pub fn likes(&self, rooms: &mut Rooms, message: &Json) -> bool {
        let Some(id) = id_of(message, "id") else {
            return false;
        };
        let Some(target) = self.target(rooms, message) else {
            return false;
        };
        let liked: Vec<Json> = message
            .get("favorited_by")
            .and_then(Json::as_array)
            .map(|likers| {
                likers
                    .iter()
                    .filter_map(|liker| match liker {
                        Json::String(id) => Some(Json::String(id.clone())),
                        Json::Number(id) => Some(Json::String(id.to_string())),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let names: serde_json::Map<String, Json> = rooms
            .people
            .iter()
            .map(|(id, name)| (id.clone(), Json::String(name.clone())))
            .collect();
        let call = json!({"chatBridgeId": self.id(), "externalRoomId": target.room.external_id(), "messageId": id,
            "likedByList": liked, "ownGroupMeId": self.own, "nameMap": names});
        let result = crate::sms::with_mark(self.id(), || {
            crate::call("org.moqui.chat.GroupMeServices.receive#Likes", parameters(&call))
        });
        match result {
            Ok(out) => out.get("outcome").map(to_plain_string).as_deref() == Some("sent"),
            Err(error) => {
                tracing::error!(
                    "GroupMe bridge {}: the likes of {id} were not taken: {error}",
                    self.id()
                );
                false
            }
        }
    }

    /// A message changed (`message.update`) or deleted on GroupMe.
    fn changed(&self, rooms: &mut Rooms, message: &Json, deleted: bool) -> bool {
        let Some(id) = id_of(message, "id") else {
            return false;
        };
        let Some(sender) = id_of(message, "sender_id").or_else(|| id_of(message, "user_id")) else {
            return false;
        };
        let Some(target) = self.target(rooms, message) else {
            return false;
        };
        let external = target.room.external_id();
        let (transaction, item) = if deleted {
            (
                format!("gmd:{id}"),
                json!({"itemId": id, "type": "redaction", "externalRoomId": external, "externalEventId": format!("delete:{id}"),
                    "externalUserId": sender, "relatesToExternalEventId": id}),
            )
        } else {
            let Some(text) = text_field(message, "text") else {
                return false;
            };
            let at = message.get("updated_at").and_then(Json::as_i64).unwrap_or_default();
            (
                format!("gme:{id}:{at}"),
                json!({"itemId": id, "type": "edit", "externalRoomId": external, "externalEventId": format!("edit:{id}:{at}"),
                    "externalUserId": sender, "relatesToExternalEventId": id, "text": text, "originDate": at.saturating_mul(1000)}),
            )
        };
        self.ingest(&transaction, vec![item])
    }

    /// The newest GroupMe message id the room `external` has mapped (sent or taken), none for a room with none.
    fn newest(&self, facade: &EntityFacade, external: &str) -> Option<String> {
        let found = facade
            .find("moqui.chat.ChatEventExternal")
            .and_then(|find| {
                find.eq("chatBridgeId", Value::string(self.id()))
                    .eq("externalRoomId", Value::string(external))
                    .order_by("createdDate", true)
                    .limit(20)
                    .disable_authz()
                    .list()
            })
            .ok()?;
        found
            .values()
            .iter()
            .filter_map(|row| row.get("externalEventId").map(to_plain_string))
            .find(|id| !id.contains(':'))
    }

    /// The messages of `room` after its newest mapped one, when GroupMe's `last` is another: the count taken.
    fn catch_up(&self, rooms: &mut Rooms, facade: &EntityFacade, room: &Room, last: Option<&str>) -> usize {
        let Some(mut after) = self.newest(facade, &room.external_id()) else {
            // a new room starts at GroupMe's newest message: history is an import's (§15)
            return 0;
        };
        if last.is_some_and(|last| last == after) {
            return 0;
        }
        let mut taken = 0;
        for _ in 0..PAGES {
            let (path, key) = match room {
                Room::Group(group) => (
                    format!("/groups/{group}/messages?after_id={after}&limit=100"),
                    "messages",
                ),
                Room::Direct(other) => (
                    format!("/direct_messages?other_user_id={other}&after_id={after}&limit=100"),
                    "direct_messages",
                ),
            };
            let page = match self.inner.api(self.config).get(&path) {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(
                        "GroupMe bridge {}: {} was not caught up: {}",
                        self.id(),
                        room.external_id(),
                        error.text()
                    );
                    break;
                }
            };
            let mut messages = page.get(key).and_then(Json::as_array).cloned().unwrap_or_default();
            if messages.is_empty() {
                break;
            }
            messages.sort_by_key(|message| message.get("created_at").and_then(Json::as_i64).unwrap_or_default());
            let full = messages.len() >= 100;
            for message in &messages {
                if self.message(rooms, message) {
                    taken += 1;
                }
                if let Some(id) = id_of(message, "id") {
                    after = id;
                }
            }
            if !full {
                break;
            }
        }
        taken
    }

    /// The catch-up (the polling fallback): the rooms of the groups, Topics (`with_topics`) and conversations the bridge
    /// carries, made when missing, and the messages each missed. The count taken.
    pub fn sync(&self, rooms: &mut Rooms, facade: &EntityFacade, with_topics: bool) -> usize {
        let mut taken = 0;
        let api = self.inner.api(self.config);
        let mut groups: Vec<Json> = Vec::new();
        for page in 1..=PAGES {
            match api.get(&format!("/groups?omit=memberships&per_page=100&page={page}")) {
                Ok(Json::Array(list)) => {
                    let more = list.len() >= 100;
                    groups.extend(list);
                    if !more {
                        break;
                    }
                }
                Ok(_) => break,
                Err(error) => {
                    tracing::warn!(
                        "GroupMe bridge {}: its groups were not read: {}",
                        self.id(),
                        error.text()
                    );
                    break;
                }
            }
        }
        for group in &groups {
            let Some(id) = id_of(group, "id") else { continue };
            let name = text_field(group, "name").unwrap_or_default();
            rooms.groups.insert(id.clone(), name.clone());
            if !self.config.chosen(&id) {
                continue;
            }
            let last = group
                .get("messages")
                .and_then(|messages| id_of(messages, "last_message_id"));
            let target = Target {
                room: Room::Group(id.clone()),
                kind: "group",
                name: name.clone(),
                parent_name: None,
                conversation: None,
            };
            if self.ensure(rooms, &target) {
                taken += self.catch_up(rooms, facade, &target.room, last.as_deref());
            }
            if with_topics && self.config.topics {
                for topic in self.read_topics(rooms, &id) {
                    let Some(topic_id) = id_of(&topic, "id") else { continue };
                    let target = Target {
                        room: Room::Group(topic_id),
                        kind: "topic",
                        name: text_field(&topic, "topic")
                            .or_else(|| text_field(&topic, "name"))
                            .unwrap_or_default(),
                        parent_name: Some(name.clone()),
                        conversation: None,
                    };
                    let last = topic
                        .get("messages")
                        .and_then(|messages| id_of(messages, "last_message_id"));
                    if self.ensure(rooms, &target) {
                        taken += self.catch_up(rooms, facade, &target.room, last.as_deref());
                    }
                }
            }
        }
        if self.config.direct_messages {
            match api.get("/chats?per_page=100") {
                Ok(Json::Array(chats)) => {
                    for chat in &chats {
                        let other = chat.get("other_user").unwrap_or(&Json::Null);
                        let Some(other_id) = id_of(other, "id") else { continue };
                        let name = text_field(other, "name").unwrap_or_default();
                        rooms.people.insert(other_id.clone(), name.clone());
                        let last_message = chat.get("last_message").unwrap_or(&Json::Null);
                        let target = Target {
                            room: Room::Direct(other_id),
                            kind: "direct",
                            name,
                            parent_name: None,
                            conversation: id_of(last_message, "conversation_id"),
                        };
                        let last = id_of(last_message, "id");
                        if self.ensure(rooms, &target) {
                            taken += self.catch_up(rooms, facade, &target.room, last.as_deref());
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        "GroupMe bridge {}: its conversations were not read: {}",
                        self.id(),
                        error.text()
                    );
                }
            }
        }
        taken
    }
}
