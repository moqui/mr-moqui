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

//! The chat feeds folded into the store (`ChatServices.get#UserChanges` and `get#RoomChanges`, the Chat screen's
//! `chatChanges` and `roomChanges`).
//!
//! The user's feed, one change per room and kind:
//! - `membership` with `member` false: the room is forgotten, with its events and its cached answers. A membership of
//!   a room not held is a new room: the room list is read again (its row). Another (a role) reads the room again.
//! - `room` (details or members changed): the room list and the room are read again.
//! - `settings`: the room's row takes `notificationLevelEnumId`, `isMuted`, `isPinned`.
//! - `read`: the read marker, and no unread or mentions when it reaches the room's last message (else the list is
//!   read again for the counts).
//! - `retention`: the room's events up to `eventSeq` go with the ranges below them, and its cached timeline answers.
//!
//! The feed's last page has the heads of the user's rooms (all of them after a socket drop left a head stale, else
//! those active since `headsSince`). A held room whose newest stretch reaches its head's `lastEventSeq` is kept up
//! again (its head restored); another has its latest page read again. A head of a room not held reads the list.
//!
//! Then each held room whose `lastChangeSeq` (its head's, or its row's) is past the room's cursor reads its feed: a
//! redaction is folded as the push folds it; another change has its message read again (`timeline` by `eventSeq`), and
//! a thread reply's root too. More than [`REREAD_MAX`] messages, or a reset, drops the room's held events and reads its
//! latest page. A room with no events held only takes its cursor. Only rooms the store holds are read.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{names_room, Run, CHAT_CURSOR, HEADS_CURSOR, LIMIT, MAX_PAGES};
use crate::offline::{self, chat_base, chat_event, number, ROOM_META};
use crate::store::{Store, ROOM_CURSOR};
use crate::sync::mirror::{encode, Ended};
use crate::sync::push;

/// Messages of one room re-read one by one; past it the room's latest page is read again instead.
const REREAD_MAX: usize = 50;
/// Events of the latest page read again.
const PAGE: u32 = 50;

/// What the folded changes leave to read.
#[derive(Default)]
struct Needs {
    /// The room list (new rooms, counts).
    list: bool,
    /// Rooms whose fields are read again.
    rooms: BTreeSet<String>,
}

pub(super) fn chat(run: &mut Run<'_, '_>) -> Result<(), Ended> {
    let base = chat_base(&run.job.comms_root);
    let cursor = run.store(|store| store.sync_seq(CHAT_CURSOR)).flatten();
    // after a drop every head is stale: all heads are asked for, so each is restored or read again
    let stale = run.store(any_stale).unwrap_or(false);
    let heads_since = if stale {
        None
    } else {
        run.store(|store| store.sync_seq(HEADS_CURSOR)).flatten()
    };
    let mut after = cursor;
    let mut needs = Needs::default();
    let mut heads: Option<Vec<Value>> = None;
    for _ in 0..MAX_PAGES {
        let mut path = format!("{base}/chatChanges?limit={LIMIT}");
        if let Some(after) = after {
            path.push_str(&format!("&afterSeq={after}"));
        }
        if let Some(since) = heads_since {
            path.push_str(&format!("&headsSince={since}"));
        }
        let Some(page) = run.json(&path)? else {
            return Ok(());
        };
        if page["reset"].as_bool() == Some(true) {
            let last = number(&page["lastSeq"]).unwrap_or(0);
            if cursor.is_some() {
                // a cursor the server no longer keeps (purged, another database): what is held may be stale
                run.fold("a chat reset", |store| forget_chat_reads(store, &base));
                tracing::info!("Feeds: the chat feed was reset; chat is read again");
            }
            run.fold("the chat cursor", |store| {
                store.remove_sync_cursors(HEADS_CURSOR).map_err(|err| err.to_string())?;
                store.set_sync_seq(CHAT_CURSOR, last).map_err(|err| err.to_string())
            });
            run.synced.chat_reread = true;
            return Ok(());
        }
        for change in page["changes"].as_array().into_iter().flatten() {
            let what = format!("chat change {}", change["changeSeq"]);
            run.fold(&what, |store| fold_change(store, &base, change, &mut needs));
        }
        if let Some(last) = number(&page["lastSeq"]) {
            run.fold("the chat cursor", |store| {
                store.set_sync_seq(CHAT_CURSOR, last).map_err(|err| err.to_string())
            });
            after = Some(last);
        }
        if page["more"].as_bool() != Some(true) {
            heads = page["heads"].as_array().cloned();
            if let Some(since) = number(&page["headsSince"]) {
                run.fold("the heads cursor", |store| {
                    store.set_sync_seq(HEADS_CURSOR, since).map_err(|err| err.to_string())
                });
            }
            break;
        }
    }
    let heads = heads.unwrap_or_default();
    let held: BTreeSet<String> = run
        .store(Store::chat_room_ids)
        .unwrap_or_default()
        .into_iter()
        .collect();
    if heads
        .iter()
        .filter_map(|head| head["chatRoomId"].as_str())
        .any(|room| !held.contains(room))
    {
        needs.list = true;
    }
    if needs.list {
        run.keep(&format!("{base}/chatData?limit=200"))?;
    }
    for room in needs.rooms.iter().filter(|room| held.contains(*room)) {
        run.keep(&format!("{base}/room?chatRoomId={}", encode(room)))?;
    }
    // the heads: a stretch that reaches its room's newest event is kept up again, another room's latest page read
    let mut last_change: BTreeMap<String, i64> = BTreeMap::new();
    for head in &heads {
        let Some(room) = head["chatRoomId"].as_str().filter(|room| held.contains(*room)) else {
            continue;
        };
        if let Some(seq) = number(&head["lastChangeSeq"]) {
            last_change.insert(room.to_string(), seq);
        }
        let Some(latest) = number(&head["lastEventSeq"]) else {
            continue;
        };
        let up = run
            .store(|store| {
                if !store.has_chat_events(room) || store.chat_head(room, None).is_some_and(|high| high >= latest) {
                    return Ok(true);
                }
                store.restore_chat_head(room, latest)
            })
            .and_then(Result::ok)
            .unwrap_or(true);
        if !up {
            run.keep(&latest_page(&base, room))?;
        }
    }
    // the rooms' feeds, for the held rooms with message changes past their cursor
    let rooms: Vec<(String, Option<i64>)> = run
        .store(|store| {
            store
                .chat_rooms(u32::MAX)
                .into_iter()
                .map(|room| {
                    let from_row = serde_json::from_str::<Value>(&room.row)
                        .ok()
                        .and_then(|row| number(&row["lastChangeSeq"]));
                    let seq = last_change.get(&room.chat_room_id).copied().or(from_row);
                    (room.chat_room_id, seq)
                })
                .collect()
        })
        .unwrap_or_default();
    for (room, seq) in rooms {
        room_feed(run, &base, &room, seq)?;
    }
    Ok(())
}

/// Whether a held room with events has no head: a drop left it stale.
fn any_stale(store: &Store) -> bool {
    store
        .chat_room_ids()
        .iter()
        .any(|room| store.has_chat_events(room) && store.chat_head(room, None).is_none())
}

fn latest_page(base: &str, room: &str) -> String {
    format!("{base}/timeline?chatRoomId={}&limit={PAGE}", encode(room))
}

/// Fold one change of the user's feed.
fn fold_change(store: &Store, base: &str, change: &Value, needs: &mut Needs) -> Result<(), String> {
    let Some(room) = change["chatRoomId"].as_str().filter(|room| !room.is_empty()) else {
        return Ok(());
    };
    let data = &change["data"];
    let held = store.chat_room(room);
    match change["kind"].as_str().unwrap_or("") {
        "membership" if data["member"].as_bool() == Some(false) => forget_room(store, base, room),
        "membership" | "room" => {
            needs.list = true;
            if held.is_some() {
                needs.rooms.insert(room.to_string());
            }
            Ok(())
        }
        "settings" => {
            let Some(mut kept) = held else { return Ok(()) };
            let Ok(mut row) = serde_json::from_str::<Value>(&kept.row) else {
                return Ok(());
            };
            for name in ["notificationLevelEnumId", "isMuted", "isPinned"] {
                if let Some(value) = data.get(name) {
                    row[name] = value.clone();
                }
            }
            kept.row = row.to_string();
            store.put_chat_room(&kept).map_err(|err| err.to_string())
        }
        "read" => {
            let last_read = data["lastReadEventSeq"].clone();
            offline::note_read(store, room, &last_read)?;
            let Some(mut kept) = held else { return Ok(()) };
            let Ok(mut row) = serde_json::from_str::<Value>(&kept.row) else {
                return Ok(());
            };
            let read = number(&last_read).unwrap_or(0);
            if !last_read.is_null() {
                row["lastReadEventSeq"] = last_read;
            }
            if read >= number(&row["lastMessage"]["eventSeq"]).unwrap_or(0) {
                row["unreadCount"] = json!(0);
                row["mentionCount"] = json!(0);
            } else {
                // read part of the way: the server counts the rest
                needs.list = true;
            }
            kept.row = row.to_string();
            store.put_chat_room(&kept).map_err(|err| err.to_string())
        }
        "retention" => {
            let Some(through) = number(&change["eventSeq"]) else {
                return Ok(());
            };
            store
                .trim_chat_retention(room, through)
                .map_err(|err| err.to_string())?;
            // the answers kept whole may hold what was deleted
            for tail in ["/timeline", "/roomView"] {
                store
                    .remove_answers(&format!("{base}{tail}"), |key| names_room(key, room))
                    .map_err(|err| err.to_string())?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A room the user is no longer in: its row, events, cursor and every answer kept of it go.
fn forget_room(store: &Store, base: &str, room: &str) -> Result<(), String> {
    store.remove_chat_room(room).map_err(|err| err.to_string())?;
    store
        .remove_answer(ROOM_META, &format!("chatRoomId={room}"))
        .map_err(|err| err.to_string())?;
    store
        .remove_answers(base, |key| names_room(key, room))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// A reset of the user's feed: the cached reads of chat go (rooms, events, rooms' cursors, answers), for the mirror
/// to read again.
fn forget_chat_reads(store: &Store, base: &str) -> Result<(), String> {
    store.forget_chat().map_err(|err| err.to_string())?;
    for prefix in [base, ROOM_META, offline::CHAT_META] {
        store.remove_answers(prefix, |_| true).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Read a held room's message changes past its cursor, when its `last_change` says there are some.
fn room_feed(run: &mut Run<'_, '_>, base: &str, room: &str, last_change: Option<i64>) -> Result<(), Ended> {
    let name = format!("{ROOM_CURSOR}{room}");
    let (cursor, has_events) = run
        .store(|store| (store.sync_seq(&name), store.has_chat_events(room)))
        .unwrap_or((None, false));
    match (cursor, last_change) {
        // nothing held to change: the cursor is where the room is now
        (None, Some(seq)) if !has_events => {
            run.fold("a room cursor", |store| {
                store.set_sync_seq(&name, seq).map_err(|err| err.to_string())
            });
            return Ok(());
        }
        (Some(cursor), Some(seq)) if seq <= cursor => return Ok(()),
        (Some(_), None) => return Ok(()),
        _ => {}
    }
    let mut after = cursor;
    let mut reread: BTreeSet<i64> = BTreeSet::new();
    let mut redacted: BTreeMap<i64, Value> = BTreeMap::new();
    for _ in 0..MAX_PAGES {
        let mut path = format!("{base}/roomChanges?chatRoomId={}&limit={LIMIT}", encode(room));
        if let Some(after) = after {
            path.push_str(&format!("&afterSeq={after}"));
        }
        let Some(page) = run.json(&path)? else {
            return Ok(());
        };
        if page["reset"].as_bool() == Some(true) {
            let last = number(&page["lastSeq"]).unwrap_or(0);
            if cursor.is_some() && has_events {
                run.fold("a room reset", |store| {
                    store.forget_chat_events(room).map_err(|err| err.to_string())
                });
                run.keep(&latest_page(base, room))?;
            }
            run.fold("a room cursor", |store| {
                store.set_sync_seq(&name, last).map_err(|err| err.to_string())
            });
            return Ok(());
        }
        for change in page["changes"].as_array().into_iter().flatten() {
            let Some(seq) = number(&change["eventSeq"]) else {
                continue;
            };
            let data = &change["data"];
            if data["redacted"].as_bool() == Some(true) {
                redacted.insert(seq, change["changeDate"].clone());
            } else {
                reread.insert(seq);
            }
            if let Some(root) = number(&data["threadRootEventSeq"]).filter(|root| *root != seq) {
                reread.insert(root);
            }
        }
        if let Some(last) = number(&page["lastSeq"]) {
            after = Some(last);
        }
        if page["more"].as_bool() != Some(true) {
            break;
        }
    }
    for (seq, date) in &redacted {
        run.fold("a redaction", |store| push::redact_message(store, room, *seq, date));
        reread.remove(seq);
    }
    let held: Vec<i64> = run
        .store(|store| {
            reread
                .iter()
                .copied()
                .filter(|seq| store.chat_event(room, *seq).is_some())
                .collect()
        })
        .unwrap_or_default();
    if held.len() > REREAD_MAX {
        run.fold("a room's events", |store| {
            store.forget_chat_events(room).map_err(|err| err.to_string())
        });
        run.keep(&latest_page(base, room))?;
    } else {
        for seq in held {
            let path = format!("{base}/timeline?chatRoomId={}&eventSeq={seq}", encode(room));
            let Some(page) = run.json(&path)? else {
                continue;
            };
            let found = page["eventList"]
                .as_array()
                .and_then(|list| list.iter().find(|event| number(&event["eventSeq"]) == Some(seq)))
                .and_then(|event| chat_event(room, event));
            run.fold("a changed message", |store| match &found {
                Some(event) => store
                    .put_chat_events(std::slice::from_ref(event))
                    .map(|_| ())
                    .map_err(|err| err.to_string()),
                None => store.remove_chat_event(room, seq).map_err(|err| err.to_string()),
            });
        }
    }
    if let Some(after) = after {
        run.fold("a room cursor", |store| {
            store.set_sync_seq(&name, after).map_err(|err| err.to_string())
        });
    }
    Ok(())
}
