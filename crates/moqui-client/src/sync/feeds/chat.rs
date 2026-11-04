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
//! A change that does not fold has its room and the list read again. These reads are pending reads (`super::PENDING`),
//! noted before the cursor moves past the page.
//!
//! The feed's last page has the heads of the user's rooms (all of them after a socket drop left a head stale, else
//! those active since `headsSince`). A held room whose newest stretch reaches its head's `lastEventSeq` is kept up
//! again (its head restored); another has its latest page read again. A head of a room not held reads the list. The
//! heads' cursor moves on once those reads are noted.
//!
//! Then each held room whose `lastChangeSeq` (its head's, or its row's) is past the room's cursor reads its feed, in
//! order: a redaction is folded as the push folds it; another change has its message read again (`timeline` by
//! `eventSeq`), and a thread reply's root too. The room's cursor stops at the first change that does not fold or read
//! (it is read again next time). More than [`REREAD_MAX`] messages, or a reset, drops the room's held events and reads
//! its latest page. A room with no events held only takes its cursor. Only rooms the store holds are read.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::{names_room, note_pending, Run, CHAT_CURSOR, HEADS_CURSOR, LIMIT, MAX_PAGES};
use crate::offline::{self, chat_base, chat_event, number, ROOM_META};
use crate::store::{Store, ROOM_CURSOR};
use crate::sync::mirror::{encode, Ended};
use crate::sync::push;

/// Messages of one room re-read one by one; past it the room's latest page is read again instead.
const REREAD_MAX: usize = 50;
/// The pending reads of chat, in the order they are read.
const CHAT_PENDING: [&str; 3] = ["list", "room:", "latest:"];

pub(super) fn chat(run: &mut Run<'_, '_>) -> Result<(), Ended> {
    let base = chat_base(&run.job.comms_root);
    // what an earlier run left to read
    run.drain(&CHAT_PENDING)?;
    let cursor = run.store(|store| store.sync_seq(CHAT_CURSOR)).flatten();
    // after a drop every head is stale: all heads are asked for, so each is restored or read again
    let stale = run.store(any_stale).unwrap_or(false);
    let heads_since = if stale {
        None
    } else {
        run.store(|store| store.sync_seq(HEADS_CURSOR)).flatten()
    };
    let mut after = cursor;
    let mut heads: Option<(Vec<Value>, Option<i64>)> = None;
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
            // a cursor the server no longer keeps (purged, another database): what is held may be stale. The drop and
            // the new cursor go together, or neither
            let reset = run.fold("a chat reset", |store| {
                store
                    .atomic(|store| {
                        if cursor.is_some() {
                            forget_chat_reads(store, &base).map_err(moqui_lib::error::Error::new)?;
                        }
                        store.remove_sync_cursors(HEADS_CURSOR)?;
                        store.set_sync_seq(CHAT_CURSOR, last)
                    })
                    .map_err(|err| err.to_string())
            });
            if reset {
                if cursor.is_some() {
                    tracing::info!("Feeds: the chat feed was reset; chat is read again");
                    run.synced.chat_dropped = true;
                }
                run.synced.chat_reread = true;
            }
            return Ok(());
        }
        for change in page["changes"].as_array().into_iter().flatten() {
            let what = format!("chat change {}", change["changeSeq"]);
            if !run.fold(&what, |store| fold_change(store, &base, change)) {
                // read again what it was about
                run.pending("list");
                if let Some(room) = change["chatRoomId"].as_str() {
                    run.pending(&format!("room:{room}"));
                }
            }
        }
        if let Some(last) = number(&page["lastSeq"]) {
            run.fold("the chat cursor", |store| {
                store.set_sync_seq(CHAT_CURSOR, last).map_err(|err| err.to_string())
            });
            after = Some(last);
        }
        if page["more"].as_bool() != Some(true) {
            heads = Some((
                page["heads"].as_array().cloned().unwrap_or_default(),
                number(&page["headsSince"]),
            ));
            break;
        }
    }
    let (heads, since) = heads.unwrap_or_default();
    let held: BTreeSet<String> = run
        .store(Store::chat_room_ids)
        .unwrap_or_default()
        .into_iter()
        .collect();
    // the heads: a stretch that reaches its room's newest event is kept up again, another room's latest page read
    let mut last_change: BTreeMap<String, i64> = BTreeMap::new();
    for head in &heads {
        let Some(room) = head["chatRoomId"].as_str() else {
            continue;
        };
        if !held.contains(room) {
            run.pending("list");
            continue;
        }
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
            .unwrap_or(false);
        if !up {
            run.pending(&format!("latest:{room}"));
        }
    }
    if let Some(since) = since {
        run.fold("the heads cursor", |store| {
            store.set_sync_seq(HEADS_CURSOR, since).map_err(|err| err.to_string())
        });
    }
    run.drain(&CHAT_PENDING)?;
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
    // a room reset or too many changes leave its latest page to read
    run.drain(&["latest:"])
}

/// Whether a held room with events has no head: a drop left it stale.
fn any_stale(store: &Store) -> bool {
    store
        .chat_room_ids()
        .iter()
        .any(|room| store.has_chat_events(room) && store.chat_head(room, None).is_none())
}

/// Fold one change of the user's feed; what it leaves to read is noted as pending.
fn fold_change(store: &Store, base: &str, change: &Value) -> Result<(), String> {
    let Some(room) = change["chatRoomId"].as_str().filter(|room| !room.is_empty()) else {
        return Ok(());
    };
    let data = &change["data"];
    let held = store.chat_room(room);
    match change["kind"].as_str().unwrap_or("") {
        "membership" if data["member"].as_bool() == Some(false) => forget_room(store, room),
        "membership" | "room" => {
            note_pending(store, "list")?;
            if held.is_some() {
                note_pending(store, &format!("room:{room}"))?;
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
                note_pending(store, "list")?;
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

/// A room the user is no longer in: its row, events, cursor and every answer of Chat kept of it go (under any comms
/// app's path), with its pending reads. For the feed and for a pushed room left.
///
/// # Errors
///
/// Returns an error naming what was not removed.
pub(crate) fn forget_room(store: &Store, room: &str) -> Result<(), String> {
    store.remove_chat_room(room).map_err(|err| err.to_string())?;
    store
        .remove_answer(ROOM_META, &format!("chatRoomId={room}"))
        .map_err(|err| err.to_string())?;
    for pending in [format!("room:{room}"), format!("latest:{room}")] {
        store
            .remove_sync_cursor(&format!("{}{pending}", super::PENDING))
            .map_err(|err| err.to_string())?;
    }
    store
        .remove_answers("/", |key| {
            key.split_once('?').is_some_and(|(path, _)| path.contains("/Chat/")) && names_room(key, room)
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// A reset of the user's feed: the cached reads of chat go (rooms, events, rooms' cursors, answers, pending reads),
/// for the mirror to read again.
fn forget_chat_reads(store: &Store, base: &str) -> Result<(), String> {
    store.forget_chat().map_err(|err| err.to_string())?;
    for prefix in [base, ROOM_META, offline::CHAT_META] {
        store.remove_answers(prefix, |_| true).map_err(|err| err.to_string())?;
    }
    for kind in CHAT_PENDING {
        store
            .remove_sync_cursors(&format!("{}{kind}", super::PENDING))
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// A change of a room's feed.
struct RoomChange {
    change_seq: i64,
    event_seq: i64,
    redacted: bool,
    date: Value,
    root: Option<i64>,
}

/// Read a held room's message changes past its cursor, when its `last_change` says there are some.
fn room_feed(run: &mut Run<'_, '_>, base: &str, room: &str, last_change: Option<i64>) -> Result<(), Ended> {
    let name = format!("{ROOM_CURSOR}{room}");
    let (cursor, has_events) = run
        .store(|store| (store.sync_seq(&name), store.has_chat_events(room)))
        .unwrap_or((None, false));
    match (cursor, last_change) {
        // nothing held to change: the cursor is where the room is now (or is taken when a page is held)
        (None, seq) if !has_events => {
            if let Some(seq) = seq {
                run.fold("a room cursor", |store| {
                    store.set_sync_seq(&name, seq).map_err(|err| err.to_string())
                });
            }
            return Ok(());
        }
        (Some(cursor), Some(seq)) if seq <= cursor => return Ok(()),
        (Some(_), None) => return Ok(()),
        _ => {}
    }
    let mut after = cursor;
    let mut changes: Vec<RoomChange> = Vec::new();
    let mut last: Option<i64> = None;
    for _ in 0..MAX_PAGES {
        let mut path = format!("{base}/roomChanges?chatRoomId={}&limit={LIMIT}", encode(room));
        if let Some(after) = after {
            path.push_str(&format!("&afterSeq={after}"));
        }
        let Some(page) = run.json(&path)? else {
            break;
        };
        if page["reset"].as_bool() == Some(true) {
            let reset_to = number(&page["lastSeq"]).unwrap_or(0);
            // the room's events dropped, its latest page left to read, and the cursor, in one unit
            let forget = cursor.is_some() && has_events;
            if forget {
                run.synced.rooms_changed.insert(room.to_string());
            }
            run.fold("a room reset", |store| {
                store
                    .atomic(|store| {
                        if forget {
                            store.forget_chat_events(room)?;
                            note_pending(store, &format!("latest:{room}")).map_err(moqui_lib::error::Error::new)?;
                        }
                        store.set_sync_seq(&name, reset_to)
                    })
                    .map_err(|err| err.to_string())
            });
            return Ok(());
        }
        for change in page["changes"].as_array().into_iter().flatten() {
            let (Some(change_seq), Some(event_seq)) = (number(&change["changeSeq"]), number(&change["eventSeq"]))
            else {
                continue;
            };
            let data = &change["data"];
            changes.push(RoomChange {
                change_seq,
                event_seq,
                redacted: data["redacted"].as_bool() == Some(true),
                date: change["changeDate"].clone(),
                root: number(&data["threadRootEventSeq"]).filter(|root| *root != event_seq),
            });
        }
        last = number(&page["lastSeq"]).or(last);
        after = last.or(after);
        if page["more"].as_bool() != Some(true) {
            break;
        }
    }
    // the messages held that changed: too many are read as the latest page instead
    let held: BTreeSet<i64> = run
        .store(|store| {
            changes
                .iter()
                .filter(|change| !change.redacted)
                .flat_map(|change| std::iter::once(change.event_seq).chain(change.root))
                .filter(|seq| store.chat_event(room, *seq).is_some())
                .collect()
        })
        .unwrap_or_default();
    if held.len() > REREAD_MAX {
        let to = last;
        run.synced.rooms_changed.insert(room.to_string());
        run.fold("a room's events", |store| {
            store
                .atomic(|store| {
                    store.forget_chat_events(room)?;
                    note_pending(store, &format!("latest:{room}")).map_err(moqui_lib::error::Error::new)?;
                    match to {
                        Some(to) => store.set_sync_seq(&name, to),
                        None => Ok(()),
                    }
                })
                .map_err(|err| err.to_string())
        });
        return Ok(());
    }
    // in order; the cursor stops at the first change not done
    let mut done: Option<i64> = None;
    let mut read: BTreeSet<i64> = BTreeSet::new();
    let mut ended = None;
    'changes: for change in &changes {
        if change.redacted {
            if !run.fold("a redaction", |store| {
                push::redact_message(store, room, change.event_seq, &change.date)
            }) {
                break;
            }
            run.synced.rooms_changed.insert(room.to_string());
            read.insert(change.event_seq);
        } else {
            for seq in std::iter::once(change.event_seq).chain(change.root) {
                if !held.contains(&seq) || !read.insert(seq) {
                    continue;
                }
                let path = format!("{base}/timeline?chatRoomId={}&eventSeq={seq}", encode(room));
                let page = match run.json(&path) {
                    Ok(Some(page)) => page,
                    Ok(None) => break 'changes,
                    Err(stop) => {
                        ended = Some(stop);
                        break 'changes;
                    }
                };
                let found = page["eventList"]
                    .as_array()
                    .and_then(|list| list.iter().find(|event| number(&event["eventSeq"]) == Some(seq)))
                    .and_then(|event| chat_event(room, event));
                let written = run.fold("a changed message", |store| match &found {
                    Some(event) => store
                        .put_chat_events(std::slice::from_ref(event))
                        .map(|_| ())
                        .map_err(|err| err.to_string()),
                    None => store.remove_chat_event(room, seq).map_err(|err| err.to_string()),
                });
                if !written {
                    break 'changes;
                }
                run.synced.rooms_changed.insert(room.to_string());
            }
        }
        done = Some(change.change_seq);
    }
    // all done: the page's last number (past changes not listed, none held); else the last done
    let to = if ended.is_none() && done == changes.last().map(|change| change.change_seq) {
        last.or(done)
    } else {
        done
    };
    if let Some(to) = to {
        run.fold("a room cursor", |store| {
            store.set_sync_seq(&name, to).map_err(|err| err.to_string())
        });
    }
    match ended {
        Some(ended) => Err(ended),
        None => Ok(()),
    }
}
