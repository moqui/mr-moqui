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

//! The chat pushes of an account's `/notws`, which the host holds for its pages (`sockets.rs`), written into its store
//! too (plans/Mobile-Desktop.md §6, MD4), so what came while the app ran is there offline. The frames are those
//! `ChatServices.xml` documents ("The pushes"):
//!
//! - `chatRoom:<id>`, kind `event`: a new event. A message (or a pin, or another event the timeline lists) is kept and
//!   the room's head range raised to it (`Store::put_chat_pushed`); a thread reply is kept under its root, whose reply
//!   count and last reply it changes. An edit, a reaction or a redaction is no row of its own: it is folded into the
//!   event it changes, as `get#Timeline` would then give that event (WebrootChat.js `applyEvent`'s rules, and the
//!   server's for an encrypted room: an edit's ciphertext with its seq and date, an encrypted reaction in
//!   `encryptedReactions`, and a redaction that empties the ciphertext, poll, forward, previews and assist too, so none
//!   stays on disk), and the head raised past it. The room's row takes the last message, and an unread count for a
//!   message of another user.
//! - `chatRoom:<id>`, kind `update`: a message's fields changed with no event (its link previews, its poll).
//! - `chatRoom:<id>`, kind `read` of this user, and `chatUser:<me>` kind `read`: the room's read marker, and its unread
//!   and mention counts gone.
//! - `chatUser:<me>`, kind `room`, action `left`: the room and its events forgotten. A room joined or changed carries
//!   no row; the next room list brings it.
//!
//! A push that comes twice (the shell's socket and an app window's) changes nothing the second time: one at or below
//! the head range, or whose row is kept, is in already. A frame that does
//! not parse, or that the store does not take, is logged and passed over.

use moqui_client::store::{ChatEvent, Store};
use serde_json::{json, Value};

use super::offline::{chat_event, number};

/// Keep what a `/notws` text frame of `me`'s says of chat; anything else is passed over.
pub(crate) fn keep_frame(store: &Store, me: &str, text: &str) {
    let Ok(frame) = serde_json::from_str::<Value>(text) else {
        tracing::debug!("A /notws frame that is not JSON was passed over");
        return;
    };
    let topic = frame["topic"].as_str().unwrap_or("");
    if !topic.starts_with("chatRoom:") && !topic.starts_with("chatUser:") {
        return;
    }
    // the message may come as JSON text
    let message = match &frame["message"] {
        Value::String(inner) => serde_json::from_str(inner).unwrap_or(Value::Null),
        other => other.clone(),
    };
    if !message.is_object() {
        return;
    }
    let kept = if let Some(room) = topic.strip_prefix("chatRoom:") {
        let room = message["chatRoomId"].as_str().unwrap_or(room).to_string();
        room_push(store, me, &room, &message)
    } else {
        user_push(store, &message)
    };
    if let Err(err) = kept {
        tracing::warn!("A chat push on {topic} was not kept: {err}");
    }
}

fn room_push(store: &Store, me: &str, room: &str, message: &Value) -> Result<(), String> {
    match message["kind"].as_str().unwrap_or("") {
        "event" if message["event"].is_object() => keep_event(store, me, room, &message["event"]),
        "update" => {
            let Some(seq) = number(&message["eventSeq"]) else {
                return Ok(());
            };
            let Some(fields) = message["fields"].as_object() else {
                return Ok(());
            };
            change_event(store, room, seq, |target| {
                for (name, value) in fields {
                    target[name.as_str()] = value.clone();
                }
            })
        }
        "read" if message["userId"].as_str() == Some(me) => read_room(store, room, message["lastReadEventSeq"].clone()),
        _ => Ok(()),
    }
}

fn user_push(store: &Store, message: &Value) -> Result<(), String> {
    match message["kind"].as_str().unwrap_or("") {
        "read" if message["all"].as_bool() == Some(true) => {
            for room in store.chat_rooms(u32::MAX) {
                read_room(store, &room.chat_room_id, Value::Null)?;
            }
            Ok(())
        }
        "read" => match message["chatRoomId"].as_str() {
            Some(room) => read_room(store, room, message["lastReadEventSeq"].clone()),
            None => Ok(()),
        },
        "room" if message["action"].as_str() == Some("left") => match message["chatRoomId"].as_str() {
            Some(room) => store.remove_chat_room(room).map_err(|err| err.to_string()),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// A new event of `room`, as `get#Timeline` lists one.
fn keep_event(store: &Store, me: &str, room: &str, event: &Value) -> Result<(), String> {
    let Some(seq) = number(&event["eventSeq"]) else {
        return Err("an event with no eventSeq".to_string());
    };
    let thread = number(&event["threadRootEventSeq"]).filter(|root| *root != seq);
    let raise = |store: &Store| -> Result<(), String> {
        store.raise_chat_head(room, None, seq).map_err(|err| err.to_string())?;
        match thread {
            Some(root) => store
                .raise_chat_head(room, Some(root), seq)
                .map_err(|err| err.to_string()),
            None => Ok(()),
        }
    };
    let who = event["senderUserId"]
        .as_str()
        .or_else(|| event["senderActorId"].as_str())
        .unwrap_or("")
        .to_string();
    let mine = event["senderUserId"].as_str() == Some(me);
    // at or below the head the store holds the timeline whole, pushes included: this one is in already (the other
    // socket's copy, or a page read after it), and folding it again could undo a later change
    let applied = store.chat_head(room, None).is_some_and(|high| seq <= high);
    if applied || store.chat_event(room, seq).is_some() {
        return Ok(());
    }
    // the ciphertext of an event of an encrypted room (C7)
    let sealed = if event["encrypted"].is_null() {
        event["content"]["encrypted"].clone()
    } else {
        event["encrypted"].clone()
    };
    match event["type"].as_str().unwrap_or("") {
        "edit" => {
            if let Some(target) = number(&event["editsEventSeq"]) {
                change_event(store, room, target, |item| {
                    if item["redacted"].as_bool() == Some(true) {
                        return;
                    }
                    // as get#Timeline: the edit's body when it has one, and an encrypted edit's ciphertext with its
                    // own seq and date
                    if !event["body"].is_null() {
                        item["body"] = event["body"].clone();
                    }
                    if !sealed.is_null() {
                        item["encrypted"] = sealed.clone();
                        item["encryptedEventSeq"] = json!(seq);
                        item["encryptedSentDate"] = event["sentDate"].clone();
                    }
                    item["bodyText"] = event["bodyText"].clone();
                    item["mentions"] = if event["mentions"].is_array() {
                        event["mentions"].clone()
                    } else {
                        json!([])
                    };
                    item["edited"] = json!(true);
                    item["editedDate"] = event["sentDate"].clone();
                })?;
            }
            return raise(store);
        }
        "reaction" => {
            let key = event["content"]["key"].as_str().map(str::to_string);
            if let Some(target) = number(&event["relatesToEventSeq"]) {
                if !sealed.is_null() {
                    // its key is for the page to read: kept as get#Timeline lists it
                    let entry = json!({ "eventSeq": seq, "senderUserId": event["senderUserId"],
                        "sentDate": event["sentDate"], "encrypted": sealed });
                    change_event(store, room, target, |item| seal_reaction(item, entry))?;
                } else if let Some(key) = key {
                    change_event(store, room, target, |item| react(item, &key, &who, mine))?;
                }
            }
            return raise(store);
        }
        "redaction" => {
            redact(store, room, event, &who, mine)?;
            return raise(store);
        }
        "pin" => {
            if let Some(target) = number(&event["relatesToEventSeq"]) {
                let pinned = event["content"]["pinned"].as_bool() == Some(true);
                change_event(store, room, target, |item| {
                    item["pinned"] = json!(pinned);
                    item["pinnedByUserId"] = if pinned {
                        event["senderUserId"].clone()
                    } else {
                        Value::Null
                    };
                })?;
            }
            // the pin is also a row of the timeline
        }
        _ => {}
    }
    let mut item = json!({ "edited": false, "redacted": false, "pinned": false, "reactions": [], "threadReplyCount": 0,
        "replyTo": null });
    if let (Some(item), Some(given)) = (item.as_object_mut(), event.as_object()) {
        for (name, value) in given {
            item.insert(name.clone(), value.clone());
        }
    }
    let Some(kept) = chat_event(room, &item) else {
        return Err("an event that does not keep".to_string());
    };
    store.put_chat_pushed(&kept).map_err(|err| err.to_string())?;
    if let Some(root) = thread {
        // the reply counts on its root, on the main timeline
        change_event(store, room, root, |item| {
            item["threadReplyCount"] = json!(number(&item["threadReplyCount"]).unwrap_or(0) + 1);
            item["threadLastDate"] = event["sentDate"].clone();
            item["threadLastEventSeq"] = json!(seq);
            item["threadLastUserId"] = event["senderUserId"].clone();
        })?;
        store.raise_chat_head(room, None, seq).map_err(|err| err.to_string())?;
    }
    if event["type"].as_str() == Some("message") {
        new_message(store, me, room, seq, event)?;
    }
    Ok(())
}

/// An encrypted reaction added to a message's `encryptedReactions`, once.
fn seal_reaction(item: &mut Value, entry: Value) {
    if !item["encryptedReactions"].is_array() {
        item["encryptedReactions"] = json!([]);
    }
    if let Some(list) = item["encryptedReactions"].as_array_mut() {
        if !list.iter().any(|kept| kept["eventSeq"] == entry["eventSeq"]) {
            list.push(entry);
        }
    }
}

/// A reaction of `who` with `key` added to a message, once.
fn react(item: &mut Value, key: &str, who: &str, mine: bool) {
    if !item["reactions"].is_array() {
        item["reactions"] = json!([]);
    }
    let Some(reactions) = item["reactions"].as_array_mut() else {
        return;
    };
    let at = match reactions.iter().position(|entry| entry["key"].as_str() == Some(key)) {
        Some(at) => at,
        None => {
            reactions.push(json!({ "key": key, "count": 0, "userIds": [], "mine": false }));
            reactions.len() - 1
        }
    };
    let entry = &mut reactions[at];
    if !entry["userIds"].is_array() {
        entry["userIds"] = json!([]);
    }
    let known = entry["userIds"]
        .as_array()
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(who)));
    if known {
        return;
    }
    if let Some(ids) = entry["userIds"].as_array_mut() {
        ids.push(json!(who));
    }
    entry["count"] = json!(number(&entry["count"]).unwrap_or(0) + 1);
    if mine {
        entry["mine"] = json!(true);
    }
}

/// A redaction: a reaction taken back, or a message's content removed (and a thread reply's count on its root).
fn redact(store: &Store, room: &str, event: &Value, who: &str, mine: bool) -> Result<(), String> {
    let content = &event["content"];
    if content["target"].as_str() == Some("reaction") {
        let Some(target) = number(&content["messageSeq"]) else {
            return Ok(());
        };
        // an encrypted reaction taken back: by the seq of the reaction's event
        if let Some(reaction) = number(&event["relatesToEventSeq"]) {
            change_event(store, room, target, |item| {
                if let Some(list) = item["encryptedReactions"].as_array_mut() {
                    list.retain(|kept| number(&kept["eventSeq"]) != Some(reaction));
                }
            })?;
        }
        let Some(key) = content["key"].as_str() else {
            return Ok(());
        };
        return change_event(store, room, target, |item| {
            let Some(reactions) = item["reactions"].as_array_mut() else {
                return;
            };
            let Some(at) = reactions.iter().position(|entry| entry["key"].as_str() == Some(key)) else {
                return;
            };
            let entry = &mut reactions[at];
            let removed = entry["userIds"].as_array_mut().is_some_and(|ids| {
                let before = ids.len();
                ids.retain(|id| id.as_str() != Some(who));
                ids.len() < before
            });
            if removed {
                entry["count"] = json!(number(&entry["count"]).unwrap_or(1) - 1);
            }
            if mine {
                entry["mine"] = json!(false);
            }
            if number(&entry["count"]).unwrap_or(0) <= 0 {
                reactions.remove(at);
            }
        });
    }
    let Some(target) = number(&event["relatesToEventSeq"]) else {
        return Ok(());
    };
    let Some(kept) = store.chat_event(room, target) else {
        return Ok(());
    };
    let Ok(mut item) = serde_json::from_str::<Value>(&kept.event) else {
        return Ok(());
    };
    if item["redacted"].as_bool() == Some(true) {
        return Ok(());
    }
    for (name, value) in [
        ("redacted", json!(true)),
        ("body", Value::Null),
        ("bodyText", Value::Null),
        ("attachments", json!([])),
        ("records", json!([])),
        ("reactions", json!([])),
        ("mentions", json!([])),
        ("pinned", json!(false)),
        ("edited", json!(false)),
        ("editedDate", Value::Null),
        // as get#Timeline shows a redacted message, so its ciphertext and content leave the disk too
        ("encrypted", Value::Null),
        ("encryptedEventSeq", Value::Null),
        ("encryptedSentDate", Value::Null),
        ("encryptedReactions", json!([])),
        ("poll", Value::Null),
        ("forwardOf", Value::Null),
        ("linkPreviews", json!([])),
        ("assist", Value::Null),
        ("redactedDate", event["sentDate"].clone()),
    ] {
        item[name] = value;
    }
    write_event(store, &kept, &item)?;
    if let Some(root) = kept.thread_seq {
        change_event(store, room, root, |root_item| {
            root_item["threadReplyCount"] = json!((number(&root_item["threadReplyCount"]).unwrap_or(0) - 1).max(0));
        })?;
    }
    Ok(())
}

/// Change the kept event `seq` of `room` with `change` and write it again; an event not kept is passed over (the store
/// shows it changed once a page brings it).
fn change_event(store: &Store, room: &str, seq: i64, change: impl FnOnce(&mut Value)) -> Result<(), String> {
    let Some(kept) = store.chat_event(room, seq) else {
        return Ok(());
    };
    let Ok(mut item) = serde_json::from_str::<Value>(&kept.event) else {
        return Err(format!("the kept event {room} {seq} is not JSON"));
    };
    change(&mut item);
    write_event(store, &kept, &item)
}

fn write_event(store: &Store, kept: &ChatEvent, item: &Value) -> Result<(), String> {
    let changed = ChatEvent {
        event: item.to_string(),
        ..kept.clone()
    };
    store
        .put_chat_events(std::slice::from_ref(&changed))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// A new message on the room's row: its last message and date, and an unread (and a mention) for another user's.
fn new_message(store: &Store, me: &str, room: &str, seq: i64, event: &Value) -> Result<(), String> {
    let Some(mut kept) = store.chat_room(room) else {
        return Ok(());
    };
    let Ok(mut row) = serde_json::from_str::<Value>(&kept.row) else {
        return Ok(());
    };
    if seq <= number(&row["lastMessage"]["eventSeq"]).unwrap_or(0) {
        return Ok(());
    }
    let sender = event["senderUserId"].as_str();
    row["lastMessage"] = json!({ "eventSeq": seq, "senderUserId": event["senderUserId"], "sentDate": event["sentDate"],
        "snippet": event["bodyText"], "encrypted": event["encrypted"] });
    if sender != Some(me) && event["threadRootEventSeq"].is_null() {
        row["unreadCount"] = json!(number(&row["unreadCount"]).unwrap_or(0) + 1);
        let mentioned = event["mentions"]
            .as_array()
            .is_some_and(|mentions| mentions.iter().any(|id| id.as_str() == Some(me)));
        if mentioned {
            row["mentionCount"] = json!(number(&row["mentionCount"]).unwrap_or(0) + 1);
        }
    }
    kept.last_seq = kept.last_seq.max(seq);
    if let Some(sent) = number(&event["sentDate"]) {
        kept.sort_ms = sent;
    }
    kept.row = row.to_string();
    store.put_chat_room(&kept).map_err(|err| err.to_string())
}

/// The room read up to `last_read` (none: all of it) here or on another device: no unread or mentions.
fn read_room(store: &Store, room: &str, last_read: Value) -> Result<(), String> {
    super::offline::note_read(store, room, &last_read)?;
    let Some(mut kept) = store.chat_room(room) else {
        return Ok(());
    };
    let Ok(mut row) = serde_json::from_str::<Value>(&kept.row) else {
        return Ok(());
    };
    if !last_read.is_null() {
        row["lastReadEventSeq"] = last_read;
    }
    row["unreadCount"] = json!(0);
    row["mentionCount"] = json!(0);
    kept.row = row.to_string();
    store.put_chat_room(&kept).map_err(|err| err.to_string())
}

#[cfg(test)]
#[path = "push_tests.rs"]
mod tests;
