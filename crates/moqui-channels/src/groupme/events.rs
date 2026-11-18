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
//! **The catch-up mark.** Each room's mapping keeps `syncCursor`, the GroupMe id up to which every message was taken. A
//! catch-up pages on from it (`after_id`), and moves it past a message only once every message before it on the page was
//! taken or left on purpose; a failed page, a failed message or the page cap stops it there, and the next run resumes. A
//! pushed message moves it only in a room whose catch-up reached GroupMe's newest message, so a gap is never skipped.
//!
//! Each event is on its own: one that fails is logged with the bridge and GroupMe's id, and the rest go on.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use moqui_core::entity::EntityFacade;
use moqui_core::service::chat_bridge::BridgeInfo;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

use super::api::id_of;
use super::{parameters, Config, Inner, Room, OUR_GUID};

/// How long a room the bridge may not make (its room limit) is not asked for again.
const REFUSAL_KEPT: Duration = Duration::from_secs(3600);

/// What a session knows of the account's rooms: the names GroupMe gave, the rooms made, their mappings and marks.
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
    /// The external room ids `ensure#Room` answered this session, with their mapping's id.
    pub made: HashMap<String, String>,
    /// The rooms the bridge may not make (its room limit), and when that was answered.
    pub refused: HashMap<String, Instant>,
    /// A room was refused for the limit since the session last said so.
    pub limit_hit: bool,
    /// The rooms whose catch-up reached GroupMe's newest message: a pushed message moves their mark.
    pub caught_up: HashSet<String>,
}

/// Where a message goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub room: Room,
    pub kind: &'static str,
    pub name: String,
    pub parent_name: Option<String>,
    /// A direct conversation's chat id, a Topic's group.
    pub conversation: Option<String>,
}

/// What became of a message from GroupMe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Stored (`ingest#Event` answered; a refusal of the item for what it is counts, as another try changes nothing).
    Stored,
    /// Left on purpose: a system message, an echo, a room not carried.
    Left,
    /// Not stored: the server's own failure. A catch-up stops before it.
    Failed,
}

/// One bridge's input, as one session works it.
pub struct Inbound<'a> {
    pub inner: &'a Inner,
    pub facade: &'a EntityFacade,
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

/// Keep `cursor` as the catch-up mark of mapping `mapping`.
pub fn set_cursor(facade: &EntityFacade, mapping: &str, cursor: &str) {
    let result = facade.find("moqui.chat.ChatRoomBridge").and_then(|find| {
        find.eq("chatRoomBridgeId", Value::string(mapping))
            .disable_authz()
            .update_all(vec![("syncCursor".to_string(), Value::string(cursor))])
    });
    if let Err(error) = result {
        tracing::warn!("GroupMe: the catch-up mark of mapping {mapping} was not kept: {error}");
    }
}

impl Inbound<'_> {
    fn id(&self) -> &str {
        &self.bridge.chat_bridge_id
    }

    /// One push message's `data`: the events taken.
    pub fn handle(&self, rooms: &mut Rooms, data: &Json) -> usize {
        let subject = data.get("subject").unwrap_or(&Json::Null);
        match data.get("type").and_then(Json::as_str).unwrap_or_default() {
            "line.create" | "direct_message.create" => usize::from(self.pushed(rooms, subject) == Taken::Stored),
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
                // a group joined now: its catch-up starts at its newest message, at the next run
                usize::from(self.ensure(rooms, &target, None).is_some())
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
        let parent = id_of(message, "parent_id")
            .filter(|parent| *parent != group)
            .or_else(|| rooms.topics.get(&group).map(|(parent, _)| parent.clone()));
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
                // a Topic's group: its members are the Topic's, for @all
                conversation: Some(parent),
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

    /// The room of `target`, found or made (`ensure#Room`, a new one's catch-up mark `start`), once a session: its
    /// mapping's id and catch-up mark, none when it is not there (refused for the room limit, kept a while, or a
    /// failure).
    pub fn ensure(&self, rooms: &mut Rooms, target: &Target, start: Option<&str>) -> Option<(String, Option<String>)> {
        let external = target.room.external_id();
        if let Some(mapping) = rooms.made.get(&external) {
            return Some((mapping.clone(), None));
        }
        if rooms
            .refused
            .get(&external)
            .is_some_and(|at| at.elapsed() < REFUSAL_KEPT)
        {
            return None;
        }
        let mut call = json!({"chatBridgeId": self.id(), "externalRoomId": external, "kind": target.kind,
            "name": target.name});
        if let Some(parent) = &target.parent_name {
            call["parentName"] = Json::String(parent.clone());
        }
        if let Some(conversation) = &target.conversation {
            call["conversationId"] = Json::String(conversation.clone());
        }
        if let Some(start) = start {
            call["startCursor"] = Json::String(start.to_string());
        }
        match crate::call("org.moqui.chat.GroupMeServices.ensure#Room", parameters(&call)) {
            Ok(out) => {
                let field = |key: &str| out.get(key).map(to_plain_string).filter(|text| !text.is_empty());
                match field("chatRoomBridgeId") {
                    Some(mapping) => {
                        rooms.refused.remove(&external);
                        rooms.made.insert(external, mapping.clone());
                        Some((mapping, field("syncCursor")))
                    }
                    None => {
                        let reason = field("reasonCode").unwrap_or_default();
                        tracing::warn!("GroupMe bridge {}: no room for {external}: {reason}", self.id());
                        if reason == "roomLimit" {
                            rooms.refused.insert(external, Instant::now());
                            rooms.limit_hit = true;
                        }
                        None
                    }
                }
            }
            Err(error) => {
                tracing::error!(
                    "GroupMe bridge {}: the room of {external} was not made: {error}",
                    self.id()
                );
                None
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

    /// A pushed message: taken, and the room's mark moved to it when the room is caught up; a failure leaves a gap the
    /// next catch-up fills.
    pub fn pushed(&self, rooms: &mut Rooms, message: &Json) -> Taken {
        let (taken, room) = self.message(rooms, message, true);
        if let Some(external) = room {
            match taken {
                Taken::Failed => {
                    rooms.caught_up.remove(&external);
                }
                Taken::Stored | Taken::Left => {
                    if rooms.caught_up.contains(&external) {
                        if let (Some(mapping), Some(id)) = (rooms.made.get(&external), id_of(message, "id")) {
                            set_cursor(self.facade, mapping, &id);
                        }
                    }
                }
            }
        }
        taken
    }

    /// A message from GroupMe, and its room's external id when it has one. `pushed`: a new room starts its catch-up
    /// at this message.
    pub fn message(&self, rooms: &mut Rooms, message: &Json, pushed: bool) -> (Taken, Option<String>) {
        let Some(id) = id_of(message, "id") else {
            return (Taken::Left, None);
        };
        let Some(sender) = id_of(message, "sender_id").or_else(|| id_of(message, "user_id")) else {
            return (Taken::Left, None);
        };
        let Some(target) = self.target(rooms, message) else {
            return (Taken::Left, None);
        };
        let external = target.room.external_id();
        if message.get("system").and_then(Json::as_bool) == Some(true) {
            return (Taken::Left, Some(external));
        }
        // the echo of a send of ours, which may come before the send's mapping is kept
        if sender == self.own
            && message
                .get("source_guid")
                .and_then(Json::as_str)
                .is_some_and(|guid| guid.starts_with(OUR_GUID))
        {
            return (Taken::Left, Some(external));
        }
        match self.ensure(rooms, &target, pushed.then_some(id.as_str())) {
            None => {
                // the room limit leaves it; a failure is the server's own
                let refused = rooms.refused.contains_key(&external);
                return (if refused { Taken::Left } else { Taken::Failed }, Some(external));
            }
            // a room this push made starts at this message, so it is caught up
            Some((_, Some(cursor))) if pushed && cursor == id => {
                rooms.caught_up.insert(external.clone());
            }
            Some(_) => {}
        }
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
                    if let Some(url) = text_field(attachment, "url").filter(|url| self.config.media_url(url)) {
                        if media.len() < 10 {
                            let content_type = if kind == "video" {
                                Json::from("video/mp4")
                            } else {
                                Json::Null
                            };
                            media.push(json!({"url": url, "contentType": content_type,
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
            return (Taken::Left, Some(external));
        }
        let mut item = json!({"itemId": id, "type": "message", "externalRoomId": external, "externalEventId": id,
            "externalUserId": sender, "text": text});
        if !media.is_empty() {
            item["media"] = Json::Array(media);
        }
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
        if !self.ingest(&format!("gm:{id}"), vec![item]) {
            return (Taken::Failed, Some(external));
        }
        if message
            .get("favorited_by")
            .and_then(Json::as_array)
            .is_some_and(|likers| !likers.is_empty())
        {
            self.likes(rooms, message);
        }
        (Taken::Stored, Some(external))
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
                json!({"itemId": id, "type": "redaction", "externalRoomId": external,
                    "externalEventId": format!("delete:{id}"), "externalUserId": sender, "relatesToExternalEventId": id}),
            )
        } else {
            let Some(text) = text_field(message, "text") else {
                return false;
            };
            let at = message.get("updated_at").and_then(Json::as_i64).unwrap_or_default();
            (
                format!("gme:{id}:{at}"),
                json!({"itemId": id, "type": "edit", "externalRoomId": external,
                    "externalEventId": format!("edit:{id}:{at}"), "externalUserId": sender,
                    "relatesToExternalEventId": id, "text": text, "originDate": at.saturating_mul(1000)}),
            )
        };
        self.ingest(&transaction, vec![item])
    }

    /// The messages of `room` after its mark `cursor` (its mapping `mapping`), up to GroupMe's `last`: the count taken.
    /// A room with no mark starts at `last`. The mark moves past each page taken whole; a failed page or message, or
    /// the page cap, stops it there. A room that reached `last` is caught up.
    fn catch_up(
        &self,
        rooms: &mut Rooms,
        room: &Room,
        mapping: &str,
        cursor: Option<String>,
        last: Option<&str>,
    ) -> usize {
        let external = room.external_id();
        let Some(mut cursor) = cursor else {
            // a room with no mark starts at GroupMe's newest message: history is an import's (§15)
            if let Some(last) = last {
                set_cursor(self.facade, mapping, last);
            }
            rooms.caught_up.insert(external);
            return 0;
        };
        if last.is_none_or(|last| last == cursor) {
            rooms.caught_up.insert(external);
            return 0;
        }
        let size = self.config.page_size;
        let mut taken = 0;
        for _ in 0..self.config.pages {
            let (path, key) = match room {
                Room::Group(group) => (
                    format!("/groups/{group}/messages?after_id={cursor}&limit={size}"),
                    "messages",
                ),
                Room::Direct(other) => (
                    format!("/direct_messages?other_user_id={other}&after_id={cursor}&limit={size}"),
                    "direct_messages",
                ),
            };
            let page = match self.inner.api(self.config).get(&path) {
                Ok(page) => page,
                Err(error) => {
                    tracing::warn!(
                        "GroupMe bridge {}: {external} was not caught up: {}",
                        self.id(),
                        error.text()
                    );
                    return taken;
                }
            };
            let mut messages = page.get(key).and_then(Json::as_array).cloned().unwrap_or_default();
            if messages.is_empty() {
                rooms.caught_up.insert(external);
                return taken;
            }
            messages.sort_by_key(|message| message.get("created_at").and_then(Json::as_i64).unwrap_or_default());
            let full = messages.len() >= size;
            for message in &messages {
                let (outcome, _) = self.message(rooms, message, false);
                if outcome == Taken::Failed {
                    // stopped before it: the next run takes it again
                    set_cursor(self.facade, mapping, &cursor);
                    return taken;
                }
                if outcome == Taken::Stored {
                    taken += 1;
                }
                if let Some(id) = id_of(message, "id") {
                    cursor = id;
                }
            }
            set_cursor(self.facade, mapping, &cursor);
            if !full || last == Some(cursor.as_str()) {
                rooms.caught_up.insert(external);
                return taken;
            }
        }
        // the page cap: the next run goes on from the mark
        taken
    }

    /// The catch-up (the polling fallback): the rooms of the groups, Topics (`with_topics`) and conversations the bridge
    /// carries, made when missing, and the messages each missed since its mark. The count taken, and whether every
    /// room reached GroupMe's newest message.
    pub fn sync(&self, rooms: &mut Rooms, with_topics: bool) -> (usize, bool) {
        let mut taken = 0;
        let mut complete = true;
        let api = self.inner.api(self.config);
        let mut groups: Vec<Json> = Vec::new();
        for page in 1..=5 {
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
                    complete = false;
                    break;
                }
            }
        }
        // one room, made or found, then caught up from its mark
        let mut room_of = |rooms: &mut Rooms, target: &Target, last: Option<String>| {
            let external = target.room.external_id();
            match self.ensure(rooms, target, last.as_deref()) {
                Some((mapping, cursor)) => {
                    taken += self.catch_up(rooms, &target.room, &mapping, cursor, last.as_deref());
                    if !rooms.caught_up.contains(&external) {
                        complete = false;
                    }
                }
                None => {
                    if !rooms.refused.contains_key(&external) {
                        complete = false;
                    }
                }
            }
        };
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
            room_of(rooms, &target, last);
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
                        conversation: Some(id.clone()),
                    };
                    let last = topic
                        .get("messages")
                        .and_then(|messages| id_of(messages, "last_message_id"));
                    room_of(rooms, &target, last);
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
                        room_of(rooms, &target, id_of(last_message, "id"));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(
                        "GroupMe bridge {}: its conversations were not read: {}",
                        self.id(),
                        error.text()
                    );
                    complete = false;
                }
            }
        }
        (taken, complete)
    }
}
