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

//! Messages and presence, both ways (docs/interfaces/XMPP.md "Chat").
//!
//! - **In.** A message to a user goes to the direct room with them (`get#DirectRoom` makes it), one to a room to that
//!   room, through ChatServices as the user: a body is `send#Message` (its id the clientRef, a reply XEP-0461 and an
//!   uploaded file XEP-0363 with it), a correction (XEP-0308) `edit#Message`, a retraction (XEP-0424) `redact#Message`,
//!   reactions (XEP-0444) `react#Message` for each one added or taken back, a displayed marker (XEP-0333) `mark#Read`,
//!   a chat state (XEP-0085) `signal#Typing`. An OMEMO message (both namespaces) is kept as it came, unread by Moqui
//!   (`XmppServices.store#EncryptedMessage`), and refused in a room with a bridge. A message with none of these (Jingle
//!   Message Initiation) goes to the other user's sessions as it is.
//! - **Out.** The chat pushes (the ones the web client gets) become stanzas: a direct room's messages as `chat` from
//!   the other user, with the user's own messages sent from elsewhere as carbons (XEP-0280); a room's as `groupchat`
//!   from `room@rooms.domain/username`, to the sessions that joined it, the sender's own included (reflection).
//! - **MUC** (XEP-0045): joining a room the user is a member of (or a public one, which joins it in Moqui); the
//!   members are the occupants, by username; nicknames are the usernames (a client's other choice is replaced, status
//!   210); invitations (XEP-0249, and mediated ones) add the member; a Moqui room that is end-to-end encrypted (C7) is
//!   refused with its reason, as the server cannot read or write its messages.
//! - **Presence** is the user's Moqui status (`UserStatus`): available, away and dnd set it; the pushes on `userStatus`
//!   become the presence of the people of the user's rooms.

use std::sync::atomic::Ordering;

use minidom::Element;
use serde_json::{json, Value as Json};

use super::iq::Target;
use super::xml::{attr, child_text, text_el, E};
use super::{RoomMeta, Session};
use crate::backend::{self, services};
use crate::{jids, ns};

/// The OMEMO payload's namespaces.
const OMEMO: [&str; 2] = [ns::OMEMO_LEGACY, ns::OMEMO2];
/// The most of one kind a single stanza acts on (reactions, invitations, linked uploads): each is a service call.
const MOST_PER_STANZA: usize = 20;

/// A clientRef as send#Message takes it.
fn valid_ref(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

/// Plain text as the safe HTML `send#Message` takes.
pub(crate) fn text_to_html(text: &str) -> String {
    let escaped = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!("<p>{}</p>", escaped.replace('\n', "<br>"))
}

/// The text of a message body in HTML.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end].trim().to_ascii_lowercase();
        if tag.starts_with("br") || tag == "/p" || tag == "/li" || tag == "/div" {
            text.push('\n');
        }
        rest = &rest[start + end + 1..];
    }
    text.push_str(rest);
    let decoded = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    decoded.trim().to_string()
}

/// A MUC affiliation and role for a Moqui member role.
fn affiliation(role: &str) -> (&'static str, &'static str) {
    match role {
        "CmrOwner" => ("owner", "moderator"),
        "CmrModerator" => ("admin", "moderator"),
        _ => ("member", "participant"),
    }
}

impl Session {
    // ---------- in ----------

    pub(super) async fn on_message(&mut self, message: Element) {
        let kind = attr(&message, "type");
        if kind == "error" {
            return;
        }
        let to = attr(&message, "to");
        match self.target(&to).await {
            Target::Room(room, None) if kind == "groupchat" => {
                if !self.joined.contains_key(&room) {
                    return self.refuse(&message, "modify", "not-acceptable", Some("join the room first"));
                }
                self.message_in(&room, &message, true).await;
            }
            Target::Room(room, None) => self.mediated_invite(&room, &message).await,
            Target::Room(_, Some(_)) => {
                if message.has_child("body", ns::CLIENT) {
                    self.refuse(
                        &message,
                        "cancel",
                        "feature-not-implemented",
                        Some("private messages in a room"),
                    );
                }
            }
            Target::User(user_id, resource) => {
                if let Some(invite) = message.get_child("x", ns::CONFERENCE) {
                    return self.direct_invite(&user_id, &attr(invite, "jid"), &message).await;
                }
                if !self.storable(&message) {
                    let mut message = message;
                    super::xml::set_attr(&mut message, "from", self.my_full());
                    return self.route_to(&user_id, resource.as_deref(), message).await;
                }
                let room = match self.call(services::DIRECT_ROOM, json!({"userId": user_id})).await {
                    Ok(found) => backend::text(&found, "chatRoomId"),
                    Err(err) => return self.refuse(&message, "cancel", "not-allowed", Some(&err.0)),
                };
                self.message_in(&room, &message, false).await;
            }
            Target::Own => {
                // a message to oneself goes to one's other sessions
                let user_id = self.user_id();
                let mut message = message;
                super::xml::set_attr(&mut message, "from", self.my_full());
                for entry in self.server.registry.sessions_of(&user_id) {
                    if entry.id != self.entry_id {
                        self.server
                            .registry
                            .send(&entry, crate::registry::Inbound::Stanza(message.clone()));
                    }
                }
            }
            Target::Unknown => self.refuse(&message, "cancel", "remote-server-not-found", None),
            _ => {}
        }
    }

    /// Whether a 1:1 message carries something Moqui keeps or acts on.
    fn storable(&self, message: &Element) -> bool {
        message.has_child("body", ns::CLIENT)
            || OMEMO.iter().any(|omemo| message.has_child("encrypted", *omemo))
            || message.has_child("replace", ns::CORRECT)
            || message.has_child("retract", ns::RETRACT)
            || message.has_child("reactions", ns::REACTIONS)
            || message.has_child("displayed", ns::MARKERS)
            || message.children().any(|child| child.ns() == ns::CHATSTATES)
    }

    /// The event a message id names in a room: a stanza-id, our own `moqui-<seq>`, or a clientRef.
    async fn event_of(&mut self, room: &str, id: &str) -> Option<i64> {
        if id.is_empty() {
            return None;
        }
        if let Ok(seq) = id.rsplit_once(':').map_or(id, |(_, seq)| seq).parse::<i64>() {
            return Some(seq);
        }
        if let Some(seq) = id.strip_prefix("moqui-").and_then(|seq| seq.parse::<i64>().ok()) {
            return Some(seq);
        }
        let client_ref = self
            .ids
            .iter()
            .find(|(_, original)| original.as_str() == id)
            .map(|(client_ref, _)| client_ref.clone())
            .unwrap_or_else(|| id.to_string());
        let found = self
            .call(
                services::FIND_EVENT,
                json!({"chatRoomId": room, "clientRef": client_ref}),
            )
            .await
            .ok()?;
        backend::number(&found, "eventSeq")
    }

    async fn message_in(&mut self, room: &str, message: &Element, group: bool) {
        let Some(meta) = self.room(room, false).await else {
            return self.refuse(message, "cancel", "item-not-found", None);
        };
        if meta.encrypted {
            return self.refuse(
                message,
                "cancel",
                "not-acceptable",
                Some("this room is end-to-end encrypted in Moqui and is not served over XMPP"),
            );
        }
        let failed = |result: Result<Json, crate::backend::CallError>| result.err().map(|err| err.0);
        if let Some(replace) = message.get_child("replace", ns::CORRECT) {
            let Some(seq) = self.event_of(room, &attr(replace, "id")).await else {
                return self.refuse(message, "cancel", "item-not-found", None);
            };
            let body = text_to_html(&child_text(message, "body", ns::CLIENT));
            let result = self
                .call(
                    services::EDIT,
                    json!({"chatRoomId": room, "eventSeq": seq, "body": body}),
                )
                .await;
            if let Some(error) = failed(result) {
                self.refuse(message, "cancel", "not-allowed", Some(&error));
            }
            return;
        }
        if let Some(retract) = message.get_child("retract", ns::RETRACT) {
            let Some(seq) = self.event_of(room, &attr(retract, "id")).await else {
                return self.refuse(message, "cancel", "item-not-found", None);
            };
            if let Some(error) = failed(
                self.call(services::REDACT, json!({"chatRoomId": room, "eventSeq": seq}))
                    .await,
            ) {
                self.refuse(message, "cancel", "not-allowed", Some(&error));
            }
            return;
        }
        if let Some(reactions) = message.get_child("reactions", ns::REACTIONS) {
            return self.reactions_in(room, message, reactions).await;
        }
        if let Some(displayed) = message.get_child("displayed", ns::MARKERS) {
            if let Some(seq) = self.event_of(room, &attr(displayed, "id")).await {
                let _ = self
                    .call(services::MARK_READ, json!({"chatRoomId": room, "eventSeq": seq}))
                    .await;
            }
            return;
        }
        let (client_ref, original) = self.client_ref_of(message);
        if let Some(encrypted) = OMEMO.iter().find_map(|omemo| message.get_child("encrypted", *omemo)) {
            if meta.bridged {
                return self.refuse(
                    message,
                    "cancel",
                    "not-acceptable",
                    Some("an OMEMO message cannot go to this room's bridged networks"),
                );
            }
            self.note_ref(&client_ref, &original);
            let result = self
                .call(
                    services::STORE_ENCRYPTED,
                    json!({"chatRoomId": room, "xml": crate::framing::write(encrypted),
                        "namespace": encrypted.ns(), "clientRef": client_ref}),
                )
                .await;
            if let Some(error) = failed(result) {
                self.refuse(message, "cancel", "not-allowed", Some(&error));
            }
            return;
        }
        let Some(body) = message.get_child("body", ns::CLIENT).map(Element::text) else {
            let typing = message.has_child("composing", ns::CHATSTATES);
            if message.children().any(|child| child.ns() == ns::CHATSTATES) {
                let _ = self
                    .call(services::TYPING, json!({"chatRoomId": room, "typing": typing}))
                    .await;
            }
            return;
        };
        let mut body = body;
        let mut parameters = json!({"chatRoomId": room, "clientRef": client_ref});
        if let Some(reply) = message.get_child("reply", ns::REPLY) {
            if let Some(seq) = self.event_of(room, &attr(reply, "id")).await {
                parameters["replyToEventSeq"] = json!(seq);
                body = strip_fallback(message, &body, ns::REPLY);
            }
        }
        // a file uploaded here (XEP-0363) and linked (XEP-0066) is the message's attachment
        let links: Vec<String> = message
            .children()
            .filter(|child| child.is("x", ns::OOB))
            .map(|oob| child_text(oob, "url", ns::OOB))
            .filter(|url| url.starts_with(&format!("{}/xmpp-upload/", self.server.config.upload_base_url)))
            .take(MOST_PER_STANZA)
            .collect();
        let mut attachments = Vec::new();
        for url in &links {
            match self
                .call(services::ATTACH_UPLOAD, json!({"chatRoomId": room, "url": url}))
                .await
            {
                Ok(found) => {
                    attachments.push(backend::text(&found, "chatAttachmentId"));
                    body = body.replace(url.as_str(), "").trim().to_string();
                }
                Err(err) => tracing::warn!("XMPP: upload {url} not attached: {err}"),
            }
        }
        if !attachments.is_empty() {
            parameters["chatAttachmentIdList"] = json!(attachments);
        }
        if !body.trim().is_empty() {
            parameters["body"] = json!(text_to_html(&body));
        }
        self.note_ref(&client_ref, &original);
        if let Some(error) = failed(self.call(services::SEND, parameters).await) {
            self.refuse(message, "cancel", "not-allowed", Some(&error));
        }
        let _ = group;
    }

    /// The clientRef for a message: its origin-id or id when that is one, else a new one mapped to it.
    fn client_ref_of(&self, message: &Element) -> (String, String) {
        let original = message
            .get_child("origin-id", ns::SID)
            .map(|origin| attr(origin, "id"))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| attr(message, "id"));
        if valid_ref(&original) {
            (original.clone(), original)
        } else {
            (format!("x{}", super::xml::random_id()), original)
        }
    }

    fn note_ref(&mut self, client_ref: &str, original: &str) {
        self.remember_ref(client_ref);
        if client_ref != original && !original.is_empty() {
            self.ids.insert(client_ref.to_string(), original.to_string());
        }
    }

    async fn reactions_in(&mut self, room: &str, message: &Element, reactions: &Element) {
        let Some(seq) = self.event_of(room, &attr(reactions, "id")).await else {
            return self.refuse(message, "cancel", "item-not-found", None);
        };
        let wanted: Vec<String> = reactions
            .children()
            .filter(|reaction| reaction.is("reaction", ns::REACTIONS))
            .map(|reaction| reaction.text().trim().to_string())
            .filter(|key| !key.is_empty())
            .take(MOST_PER_STANZA)
            .collect();
        let found = self
            .call(services::FIND_EVENT, json!({"chatRoomId": room, "eventSeq": seq}))
            .await
            .unwrap_or(Json::Null);
        let mine: Vec<String> = backend::list(&found, "myReactions")
            .iter()
            .filter_map(Json::as_str)
            .map(str::to_string)
            .collect();
        for key in wanted.iter().filter(|key| !mine.contains(key)) {
            let _ = self
                .call(
                    services::REACT,
                    json!({"chatRoomId": room, "eventSeq": seq, "key": key, "mode": "add"}),
                )
                .await;
        }
        for key in mine.iter().filter(|key| !wanted.contains(key)) {
            let _ = self
                .call(
                    services::REACT,
                    json!({"chatRoomId": room, "eventSeq": seq, "key": key, "mode": "remove"}),
                )
                .await;
        }
    }

    async fn direct_invite(&mut self, invitee: &str, room_jid: &str, message: &Element) {
        let room = jids::parse(room_jid)
            .filter(|jid| jid.domain == self.server.config.rooms_domain)
            .and_then(|jid| jid.node)
            .and_then(|node| jids::room_id(&node));
        let Some(room) = room else {
            return self.refuse(message, "modify", "bad-request", None);
        };
        if let Err(err) = self
            .call(services::ADD_MEMBER, json!({"chatRoomId": room, "userId": invitee}))
            .await
        {
            self.refuse(message, "cancel", "not-allowed", Some(&err.0));
        }
    }

    async fn mediated_invite(&mut self, room: &str, message: &Element) {
        let Some(x) = message.get_child("x", ns::MUC_USER) else {
            return;
        };
        for invite in x
            .children()
            .filter(|child| child.is("invite", ns::MUC_USER))
            .take(MOST_PER_STANZA)
        {
            if let Target::User(user_id, _) = self.target(&attr(invite, "to")).await {
                if let Err(err) = self
                    .call(services::ADD_MEMBER, json!({"chatRoomId": room, "userId": user_id}))
                    .await
                {
                    self.refuse(message, "cancel", "not-allowed", Some(&err.0));
                }
            }
        }
    }

    pub(super) async fn on_presence(&mut self, presence: Element) {
        let kind = attr(&presence, "type");
        let to = attr(&presence, "to");
        if to.is_empty() {
            return self.own_presence(&presence, &kind).await;
        }
        match self.target(&to).await {
            Target::Room(room, Some(nick)) => {
                if kind == "unavailable" {
                    self.leave(&room).await;
                } else if kind.is_empty() {
                    self.join(&room, &nick, &presence).await;
                }
            }
            Target::User(user_id, resource) => match kind.as_str() {
                // subscriptions follow the user's rooms: a request to a local user is answered for them
                "subscribe" => {
                    if let Some(bare) = self.jid_of(&user_id).await {
                        let subscribed = E::new("presence", ns::CLIENT)
                            .a("type", "subscribed")
                            .a("from", bare)
                            .build();
                        self.send(subscribed);
                    }
                }
                "subscribed" | "unsubscribe" | "unsubscribed" | "probe" => {}
                _ => {
                    let mut presence = presence;
                    super::xml::set_attr(&mut presence, "from", self.my_full());
                    self.route_to(&user_id, resource.as_deref(), presence).await;
                }
            },
            _ => {}
        }
    }

    async fn own_presence(&mut self, presence: &Element, kind: &str) {
        if kind == "unavailable" {
            self.available.store(false, Ordering::Relaxed);
            let user_id = self.user_id();
            let others = self
                .server
                .registry
                .sessions_of(&user_id)
                .iter()
                .any(|entry| entry.id != self.entry_id && entry.is_available());
            if !others {
                let _ = self.call(services::DISCONNECT, json!({"userId": user_id})).await;
            }
            return;
        }
        if !kind.is_empty() {
            return;
        }
        if let Some(caps) = presence.get_child("c", ns::CAPS) {
            self.ask_caps(&attr(caps, "node"), &attr(caps, "ver"));
        }
        let first = !self.available.swap(true, Ordering::Relaxed);
        let status = match child_text(presence, "show", ns::CLIENT).as_str() {
            "away" | "xa" => "away",
            "dnd" => "dnd",
            _ => "online",
        };
        let _ = self.call(services::SET_STATUS, json!({"status": status})).await;
        self.last_beat = Some(std::time::Instant::now());
        let _ = self
            .call(services::HEARTBEAT, json!({"idleSeconds": 0, "active": true}))
            .await;
        // the presence back to the user's own session, then the presence of the people in the roster
        let mut own = presence.clone();
        super::xml::set_attr(&mut own, "to", self.my_full());
        self.send(own);
        if first {
            let found = self.call(services::ROSTER, json!({})).await.unwrap_or(Json::Null);
            let user_ids: Vec<String> = backend::list(&found, "userIdList")
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_string)
                .collect();
            if user_ids.is_empty() {
                return;
            }
            let statuses = self
                .call(services::STATUSES, json!({"userIdList": user_ids}))
                .await
                .unwrap_or(Json::Null);
            let Some(map) = statuses.get("statusMap").and_then(Json::as_object) else {
                return;
            };
            for status in map.values() {
                self.presence_out(status).await;
            }
        }
    }

    async fn join(&mut self, room: &str, nick: &str, presence: &Element) {
        let room_jid = self.room_jid(room);
        let error = |session: &mut Session, condition: &str, text: Option<&str>| {
            let mut presence = presence.clone();
            super::xml::set_attr(&mut presence, "to", format!("{room_jid}/{nick}"));
            session.refuse(&presence, "cancel", condition, text);
        };
        let Some(mut meta) = self.room(room, true).await else {
            return error(self, "item-not-found", None);
        };
        if meta.direct {
            return error(
                self,
                "not-allowed",
                Some("a direct conversation is a chat with the person"),
            );
        }
        if meta.encrypted {
            return error(
                self,
                "not-acceptable",
                Some("this room is end-to-end encrypted in Moqui and is not served over XMPP"),
            );
        }
        let me = self.user_id();
        if !meta.members.iter().any(|(id, _, _)| *id == me) {
            if !meta.public {
                return error(self, "registration-required", Some("members only"));
            }
            if let Err(err) = self.call(services::ADD_MEMBER, json!({"chatRoomId": room})).await {
                return error(self, "not-allowed", Some(&err.0));
            }
            match self.room(room, true).await {
                Some(fresh) => meta = fresh,
                None => return error(self, "item-not-found", None),
            }
        }
        let my_nick = self
            .account
            .as_ref()
            .map(|account| account.username.clone())
            .unwrap_or_default();
        for (user_id, username, role) in meta.members.iter().filter(|(id, _, _)| *id != me) {
            let occupant = self.occupant(&room_jid, user_id, username, role, false).await;
            self.send(occupant);
        }
        let my_role = meta
            .members
            .iter()
            .find(|(id, _, _)| *id == me)
            .map(|(_, _, role)| role.clone())
            .unwrap_or_default();
        let mut own = self.occupant(&room_jid, &me, &my_nick, &my_role, true).await;
        if nick != my_nick {
            if let Some(x) = own.get_child_mut("x", ns::MUC_USER) {
                x.append_child(E::new("status", ns::MUC_USER).a("code", "210").build());
            }
        }
        self.send(own);
        self.joined.insert(room.to_string(), my_nick);
        // history (XEP-0045 §7.2.14): what the client asks for, 20 messages when it does not say
        let history = presence
            .get_child("x", ns::MUC)
            .and_then(|x| x.get_child("history", ns::MUC));
        let limit = history
            .and_then(|history| history.attr("maxstanzas"))
            .and_then(|max| max.parse::<i64>().ok())
            .unwrap_or(20)
            .clamp(0, 100);
        if limit > 0 {
            let found = self
                .call(
                    services::ARCHIVE,
                    json!({"chatRoomId": room, "limit": limit, "latest": true}),
                )
                .await
                .unwrap_or(Json::Null);
            for event in backend::list(&found, "eventList").to_vec() {
                if let Some(message) = self.event_message(room, &meta, &event, true).await {
                    self.send(message);
                }
            }
        }
        let subject = E::new("message", ns::CLIENT)
            .a("type", "groupchat")
            .a("from", room_jid)
            .a("to", self.my_full())
            .a("id", super::xml::random_id())
            .c(text_el("subject", ns::CLIENT, meta.topic.clone()))
            .build();
        self.send(subject);
    }

    async fn occupant(&mut self, room_jid: &str, user_id: &str, username: &str, role: &str, own: bool) -> Element {
        let (affiliation, muc_role) = affiliation(role);
        let jid = if own {
            self.my_full()
        } else {
            self.jid_of(user_id).await.unwrap_or_default()
        };
        let mut x = E::new("x", ns::MUC_USER).c(E::new("item", ns::MUC_USER)
            .a("affiliation", affiliation)
            .a("role", muc_role)
            .a("jid", jid));
        if own {
            x = x
                .c(E::new("status", ns::MUC_USER).a("code", "100"))
                .c(E::new("status", ns::MUC_USER).a("code", "110"));
        }
        E::new("presence", ns::CLIENT)
            .a("from", format!("{room_jid}/{username}"))
            .a("to", self.my_full())
            .c(x)
            .build()
    }

    async fn leave(&mut self, room: &str) {
        let Some(nick) = self.joined.remove(room) else {
            return;
        };
        let presence = E::new("presence", ns::CLIENT)
            .a("type", "unavailable")
            .a("from", format!("{}/{nick}", self.room_jid(room)))
            .a("to", self.my_full())
            .c(E::new("x", ns::MUC_USER)
                .c(E::new("item", ns::MUC_USER)
                    .a("affiliation", "member")
                    .a("role", "none"))
                .c(E::new("status", ns::MUC_USER).a("code", "110")))
            .build();
        self.send(presence);
    }

    // ---------- out ----------

    pub(super) async fn on_notice(&mut self, topic: &str, message: &Json) {
        let kind = backend::text(message, "kind");
        if let Some(room) = topic.strip_prefix("chatRoom:") {
            match kind.as_str() {
                "event" => {
                    if let Some(event) = message.get("event") {
                        self.event_out(room, event).await;
                    }
                }
                "typing" => self.typing_out(room, message).await,
                "read" => self.read_out(room, message).await,
                _ => {}
            }
        } else if topic.starts_with("chatUser:") && kind == "room" {
            let room = backend::text(message, "chatRoomId");
            let action = backend::text(message, "action");
            self.rooms.remove(&room);
            match action.as_str() {
                "joined" => {
                    if let Some(meta) = self.room(&room, true).await {
                        if !meta.direct && !meta.encrypted && !self.joined.contains_key(&room) {
                            let invitation = E::new("message", ns::CLIENT)
                                .a("from", self.room_jid(&room))
                                .a("to", self.my_bare())
                                .a("id", super::xml::random_id())
                                .c(E::new("x", ns::CONFERENCE).a("jid", self.room_jid(&room)))
                                .build();
                            self.send(invitation);
                        }
                    }
                }
                "left" => self.leave(&room).await,
                _ => {}
            }
        } else if topic == "userStatus" && backend::text(message, "userId") != self.user_id() {
            self.presence_out(message).await;
        }
    }

    async fn presence_out(&mut self, status: &Json) {
        let user_id = backend::text(status, "userId");
        if user_id.is_empty() || user_id == self.user_id() {
            return;
        }
        let Some(bare) = self.jid_of(&user_id).await else {
            return;
        };
        if self.is_blocked(&bare).await {
            return;
        }
        let presence = backend::text(status, "presence");
        let mut built = E::new("presence", ns::CLIENT)
            .a("from", format!("{bare}/moqui"))
            .a("to", self.my_full());
        match presence.as_str() {
            "offline" | "" => built = built.a("type", "unavailable"),
            "away" => built = built.c(text_el("show", ns::CLIENT, "away")),
            "dnd" => built = built.c(text_el("show", ns::CLIENT, "dnd")),
            _ => {}
        }
        let text = backend::text(status, "statusText");
        if !text.is_empty() {
            built = built.c(text_el("status", ns::CLIENT, text));
        }
        self.send_deferrable(built.build());
    }

    async fn typing_out(&mut self, room: &str, message: &Json) {
        let user_id = backend::text(message, "userId");
        if user_id == self.user_id() {
            return;
        }
        let Some(meta) = self.room(room, false).await else {
            return;
        };
        let state = if backend::flag(message, "typing") {
            "composing"
        } else {
            "active"
        };
        let from = if meta.direct {
            match self.jid_of(&user_id).await {
                Some(bare) => bare,
                None => return,
            }
        } else {
            match (self.joined.contains_key(room), self.username_of(&user_id).await) {
                (true, Some(nick)) => format!("{}/{nick}", self.room_jid(room)),
                _ => return,
            }
        };
        let kind = if meta.direct { "chat" } else { "groupchat" };
        let built = E::new("message", ns::CLIENT)
            .a("type", kind)
            .a("from", from)
            .a("to", self.my_full())
            .c(E::new(state, ns::CHATSTATES))
            .c(E::new("no-store", ns::HINTS))
            .build();
        self.send_deferrable(built);
    }

    async fn read_out(&mut self, room: &str, message: &Json) {
        let user_id = backend::text(message, "userId");
        let Some(seq) = backend::number(message, "lastReadEventSeq") else {
            return;
        };
        if user_id.is_empty() || user_id == self.user_id() {
            return;
        }
        let Some(meta) = self.room(room, false).await.filter(|meta| meta.direct) else {
            return;
        };
        let found = self
            .call(services::FIND_EVENT, json!({"chatRoomId": room, "eventSeq": seq}))
            .await
            .unwrap_or(Json::Null);
        if backend::text(&found, "senderUserId") != self.user_id() {
            return;
        }
        let id = self.message_id(room, seq, &backend::text(&found, "clientRef"), &meta);
        let Some(from) = self.jid_of(&user_id).await else {
            return;
        };
        let marker = E::new("message", ns::CLIENT)
            .a("type", "chat")
            .a("from", from)
            .a("to", self.my_full())
            .c(E::new("displayed", ns::MARKERS).a("id", id))
            .build();
        self.send(marker);
    }

    /// The id a message has on the wire: what the client sent it with (its clientRef), else `moqui-<seq>`.
    pub(crate) fn message_id(&self, _room: &str, seq: i64, client_ref: &str, _meta: &RoomMeta) -> String {
        if client_ref.is_empty() {
            return format!("moqui-{seq}");
        }
        self.ids
            .get(client_ref)
            .cloned()
            .unwrap_or_else(|| client_ref.to_string())
    }

    /// The stanza-id (XEP-0359) of an event: its eventSeq in a room's archive, `<room>:<seq>` in the user's own.
    pub(crate) fn stanza_id(&self, room: &str, seq: i64, meta: &RoomMeta) -> String {
        if meta.direct {
            format!("{}:{seq}", jids::room_node(room))
        } else {
            seq.to_string()
        }
    }

    async fn event_out(&mut self, room: &str, event: &Json) {
        let Some(meta) = self.room(room, false).await else {
            return;
        };
        if meta.encrypted || (!meta.direct && !self.joined.contains_key(room)) {
            return;
        }
        let me = self.user_id();
        let sender = backend::text(event, "senderUserId");
        match backend::text(event, "type").as_str() {
            "message" => {
                if sender != me {
                    self.push_waiting().await;
                }
                let client_ref = backend::text(event, "clientRef");
                if meta.direct && sender == me {
                    // the user's own message: as a carbon to the sessions that did not send it
                    if !self.carbons.load(Ordering::Relaxed) || self.sent_refs.contains(&client_ref) {
                        return;
                    }
                    let Some(inner) = self.event_message(room, &meta, event, false).await else {
                        return;
                    };
                    let carbon = E::new("message", ns::CLIENT)
                        .a("type", "chat")
                        .a("from", self.my_bare())
                        .a("to", self.my_full())
                        .c(E::new("sent", ns::CARBONS).c(E::new("forwarded", ns::FORWARD).c(inner)))
                        .build();
                    self.send(carbon);
                    return;
                }
                if let Some(message) = self.event_message(room, &meta, event, false).await {
                    self.send(message);
                }
            }
            "edit" | "reaction" | "redaction" => {
                if let Some(message) = self.change_message(room, &meta, event).await {
                    if meta.direct && sender == me {
                        if self.carbons.load(Ordering::Relaxed) {
                            let carbon = E::new("message", ns::CLIENT)
                                .a("type", "chat")
                                .a("from", self.my_bare())
                                .a("to", self.my_full())
                                .c(E::new("sent", ns::CARBONS).c(E::new("forwarded", ns::FORWARD).c(message)))
                                .build();
                            self.send(carbon);
                        }
                    } else {
                        self.send(message);
                    }
                }
            }
            "membership" if !meta.direct => self.membership_out(room, &meta, event).await,
            _ => {}
        }
    }

    async fn membership_out(&mut self, room: &str, old: &RoomMeta, event: &Json) {
        let Some(fresh) = self.room(room, true).await else {
            return;
        };
        let content = event.get("content").cloned().unwrap_or(Json::Null);
        let target = backend::text(&content, "userId");
        if target.is_empty() || target == self.user_id() {
            return;
        }
        let room_jid = self.room_jid(room);
        match fresh.members.iter().find(|(id, _, _)| *id == target).cloned() {
            Some((user_id, username, role)) => {
                let occupant = self.occupant(&room_jid, &user_id, &username, &role, false).await;
                self.send(occupant);
            }
            None => {
                if let Some((_, username, _)) = old.members.iter().find(|(id, _, _)| *id == target) {
                    let gone = E::new("presence", ns::CLIENT)
                        .a("type", "unavailable")
                        .a("from", format!("{room_jid}/{username}"))
                        .a("to", self.my_full())
                        .c(E::new("x", ns::MUC_USER)
                            .c(E::new("item", ns::MUC_USER).a("affiliation", "none").a("role", "none")))
                        .build();
                    self.send(gone);
                }
            }
        }
    }

    /// The addressing of a message in `room` from `sender`: (from, to, type).
    async fn addressing(
        &mut self,
        room: &str,
        meta: &RoomMeta,
        event: &Json,
    ) -> Option<(String, String, &'static str)> {
        let sender = backend::text(event, "senderUserId");
        if meta.direct {
            let me = self.user_id();
            if sender == me {
                let peer = meta.peer_user_id.clone()?;
                return Some((self.my_bare(), self.jid_of(&peer).await?, "chat"));
            }
            let from = self.jid_of(&sender).await?;
            if self.is_blocked(&from).await {
                return None;
            }
            return Some((from, self.my_full(), "chat"));
        }
        let nick = match self.username_of(&sender).await {
            Some(name) if !sender.is_empty() => name,
            _ => {
                let name = backend::text(event, "senderName");
                if name.is_empty() {
                    "someone".to_string()
                } else {
                    name
                }
            }
        };
        Some((format!("{}/{nick}", self.room_jid(room)), self.my_full(), "groupchat"))
    }

    /// The stanza of a message event (a live push, or `history` with its delay).
    pub(crate) async fn event_message(
        &mut self,
        room: &str,
        meta: &RoomMeta,
        event: &Json,
        history: bool,
    ) -> Option<Element> {
        let seq = backend::number(event, "eventSeq")?;
        if backend::flag(event, "redacted") {
            return None;
        }
        let (from, to, kind) = self.addressing(room, meta, event).await?;
        let client_ref = backend::text(event, "clientRef");
        let id = self.message_id(room, seq, &client_ref, meta);
        let by = if meta.direct {
            self.my_bare()
        } else {
            self.room_jid(room)
        };
        let mut message = E::new("message", ns::CLIENT)
            .a("type", kind)
            .a("from", from)
            .a("to", to)
            .a("id", id);
        let content = event.get("content").cloned().unwrap_or(Json::Null);
        let encrypted = content
            .get("xmppEncrypted")
            .and_then(|stored| stored.get("xml"))
            .and_then(Json::as_str)
            .and_then(|xml| crate::framing::parse_element(xml).ok());
        if let Some(encrypted) = encrypted {
            let namespace = encrypted.ns();
            message = message
                .c(text_el(
                    "body",
                    ns::CLIENT,
                    "This message is encrypted with OMEMO; your client cannot read it.",
                ))
                .c(E::new("encryption", ns::EME)
                    .a("namespace", namespace)
                    .a("name", "OMEMO"))
                .c(encrypted)
                .c(E::new("store", ns::HINTS));
        } else {
            let mut text = backend::text(event, "bodyText");
            if text.is_empty() {
                text = html_to_text(&backend::text(event, "body"));
            }
            let urls = if backend::list(event, "attachments").is_empty() {
                Vec::new()
            } else {
                let found = self
                    .call(services::ATTACHMENT_URLS, json!({"chatRoomId": room, "eventSeq": seq}))
                    .await
                    .unwrap_or(Json::Null);
                backend::list(&found, "urlList")
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            };
            if text.is_empty() && urls.is_empty() {
                return None;
            }
            if text.is_empty() {
                text = urls.join("\n");
            }
            message = message.c(text_el("body", ns::CLIENT, text));
            for url in urls {
                message = message.c(E::new("x", ns::OOB).c(text_el("url", ns::OOB, url)));
            }
        }
        if let Some(reply_seq) = backend::number(event, "replyToEventSeq") {
            let found = self
                .call(services::FIND_EVENT, json!({"chatRoomId": room, "eventSeq": reply_seq}))
                .await
                .unwrap_or(Json::Null);
            let reply_id = if meta.direct {
                self.message_id(room, reply_seq, &backend::text(&found, "clientRef"), meta)
            } else {
                self.stanza_id(room, reply_seq, meta)
            };
            let reply_to = match backend::text(&found, "senderUserId") {
                sender if sender.is_empty() => String::new(),
                sender if meta.direct => self.jid_of(&sender).await.unwrap_or_default(),
                sender => match self.username_of(&sender).await {
                    Some(nick) => format!("{}/{nick}", self.room_jid(room)),
                    None => String::new(),
                },
            };
            message = message.c(E::new("reply", ns::REPLY).a("to", reply_to).a("id", reply_id));
        }
        if meta.direct {
            message = message.c(E::new("markable", ns::MARKERS));
        }
        message = message.c(E::new("stanza-id", ns::SID)
            .a("by", by)
            .a("id", self.stanza_id(room, seq, meta)));
        if history {
            let stamp = super::xml::stamp(backend::number(event, "sentDate").unwrap_or(0));
            message = message.c(E::new("delay", ns::DELAY)
                .a("from", self.room_jid(room))
                .a("stamp", stamp));
        }
        Some(message.build())
    }

    /// The stanza of an edit, a reaction or a deletion.
    async fn change_message(&mut self, room: &str, meta: &RoomMeta, event: &Json) -> Option<Element> {
        let kind = backend::text(event, "type");
        let content = event.get("content").cloned().unwrap_or(Json::Null);
        let target = backend::number(event, "editsEventSeq")
            .or_else(|| backend::number(event, "relatesToEventSeq"))
            .or_else(|| backend::number(&content, "messageSeq"))?;
        let reaction_target =
            kind == "reaction" || (kind == "redaction" && backend::text(&content, "target") == "reaction");
        let target = if reaction_target {
            backend::number(&content, "messageSeq").unwrap_or(target)
        } else {
            target
        };
        let sender = backend::text(event, "senderUserId");
        let found = self
            .call(
                services::FIND_EVENT,
                json!({"chatRoomId": room, "eventSeq": target, "reactorUserId": sender}),
            )
            .await
            .ok()?;
        let (from, to, message_kind) = self.addressing(room, meta, event).await?;
        let referenced = if meta.direct || kind == "edit" {
            self.message_id(room, target, &backend::text(&found, "clientRef"), meta)
        } else {
            self.stanza_id(room, target, meta)
        };
        let mut message = E::new("message", ns::CLIENT)
            .a("type", message_kind)
            .a("from", from)
            .a("to", to)
            .a("id", super::xml::random_id());
        if kind == "edit" {
            let mut text = backend::text(&content, "bodyText");
            if text.is_empty() {
                text = html_to_text(&backend::text(&content, "body"));
            }
            message = message
                .c(text_el("body", ns::CLIENT, text))
                .c(E::new("replace", ns::CORRECT).a("id", referenced));
        } else if reaction_target {
            let mut reactions = E::new("reactions", ns::REACTIONS).a("id", referenced);
            for key in backend::list(&found, "reactorReactions")
                .iter()
                .filter_map(Json::as_str)
            {
                reactions = reactions.c(text_el("reaction", ns::REACTIONS, key));
            }
            message = message.c(reactions).c(E::new("store", ns::HINTS));
        } else {
            message = message
                .c(E::new("retract", ns::RETRACT).a("id", referenced))
                .c(E::new("fallback", ns::FALLBACK).a("for", ns::RETRACT))
                .c(text_el("body", ns::CLIENT, "This message was deleted."))
                .c(E::new("store", ns::HINTS));
        }
        Some(message.build())
    }
}

/// The body without the part a fallback element (XEP-0428) marks for `for_ns`.
fn strip_fallback(message: &Element, body: &str, for_ns: &str) -> String {
    let fallback = message
        .children()
        .find(|child| child.is("fallback", ns::FALLBACK) && attr(child, "for") == for_ns);
    let Some(range) = fallback.and_then(|fallback| fallback.get_child("body", ns::FALLBACK)) else {
        return body.to_string();
    };
    let (Ok(start), Ok(end)) = (
        attr(range, "start").parse::<usize>(),
        attr(range, "end").parse::<usize>(),
    ) else {
        return body.to_string();
    };
    // the range counts Unicode code points
    let chars: Vec<char> = body.chars().collect();
    if start > end || end > chars.len() {
        return body.to_string();
    }
    chars[..start]
        .iter()
        .chain(&chars[end..])
        .collect::<String>()
        .trim()
        .to_string()
}
