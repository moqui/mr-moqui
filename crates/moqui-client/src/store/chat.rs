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

//! Chat rooms and their events in the store, as the server's chat transitions give them (`ChatServices.get#Rooms`,
//! `get#Timeline`) and `/notws` pushes them: a room's row and an event's are the server's JSON, kept as text.
//!
//! The store knows which stretches of a timeline it holds whole: a range of event seqs per room (and per thread) from
//! each page the server gave, joined when pages touch. A page read from the store stops at its range's edge, so a gap
//! (messages never fetched) is never skipped over; [`EventSlice`] says where it stopped at a gap.
//!
//! There is no event type column: an edit, a reaction or a redaction the server pushes is folded by the caller into
//! the JSON of the event it changes (as `get#Timeline` gives it), and that event written again.
//!
//! An event that disappears (plans/Comms-Messaging.md phase 16) has its `expireDate` in `expire_ms`, which every write
//! takes from its JSON. It is removed when that time comes ([`Store::expire_chat_events`]) or when the server says it
//! went ([`Store::expire_room_events`], for its push and its changes feed's marker), with the reply count of its
//! thread's root and the room's last message set back. The ranges stay as they are: a hole in a stretch held whole is
//! no gap, as the server no longer has the event either.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use super::{failed, text_len, Read, Store, ROW_BYTES};
use moqui_lib::error::Error;

/// The most events a page has, as `get#Timeline`.
const PAGE_MAX: u32 = 200;

/// The prefix of a room's changes feed cursor (`room:<chatRoomId>`, [`Store::sync_cursor`]).
pub const ROOM_CURSOR: &str = "room:";

/// A room as the room list gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatRoom {
    pub chat_room_id: String,
    /// The room's JSON.
    pub row: String,
    /// The last event seq known; a write never lowers it, and stored events raise it.
    pub last_seq: i64,
    /// What the room list sorts by, newest first: its last activity, ms since the epoch.
    pub sort_ms: i64,
}

/// An event of a room's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatEvent {
    pub chat_room_id: String,
    pub event_seq: i64,
    /// The event's JSON.
    pub event: String,
    /// The seq of the thread's root for a thread reply (`threadRootEventSeq`); none on the main timeline.
    pub thread_seq: Option<i64>,
}

/// Which events of a room, as `get#Timeline` pages them: the `limit` latest, those before `before`, or those after
/// `after` (which wins when both are given, as on the server); of the main timeline, or of the thread whose root is
/// `thread` (the root and its replies).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventPage {
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub thread: Option<i64>,
    /// 1 to 200, as on the server; 0 is taken as 1.
    pub limit: u32,
}

/// A page of events read from the store, oldest first, with `get#Timeline`'s flags as far as the store holds the
/// timeline whole.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventSlice {
    pub events: Vec<ChatEvent>,
    /// More events before these in the store's range.
    pub has_more_before: bool,
    /// More events after these in the store's range.
    pub has_more_after: bool,
    /// Older events may exist that the store does not hold (the range does not reach the timeline's start): they need
    /// the server.
    pub gap_before: bool,
    /// Newer events may exist that the store does not hold (the range is not kept up to the latest).
    pub gap_after: bool,
}

/// A stretch of a timeline held whole.
struct Range {
    low: i64,
    high: i64,
    head: bool,
}

impl Store {
    /// Keep a room, replacing its row. Writing it counts as reading it for the size cap: an active room stays.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn put_chat_room(&self, room: &ChatRoom) -> Result<(), Error> {
        let size = self.write_chat_room(room).map_err(failed("chat room not kept"))?;
        self.wrote(size);
        Ok(())
    }

    /// Keep the user's whole room list: each room written, and the rooms not in it (left, or removed) forgotten with
    /// their events. A room that cannot be written is logged and left out; gives how many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit cannot begin or end, or a left room cannot be removed.
    pub fn replace_chat_rooms(&self, rooms: &[ChatRoom]) -> Result<usize, Error> {
        self.atomic(|store| {
            let kept_ids: Vec<String> =
                store.rows("chat rooms", "SELECT chat_room_id FROM chat_room", [], |row| row.get(0));
            for gone in kept_ids
                .iter()
                .filter(|id| !rooms.iter().any(|room| &room.chat_room_id == *id))
            {
                store.remove_chat_room(gone)?;
            }
            store.put_each("chat room", rooms, |store, room| {
                store
                    .write_chat_room(room)
                    .map_err(|err| (room.chat_room_id.clone(), err))
            })
        })
    }

    /// The room `chat_room_id`, which counts as a read; none when it is not kept or cannot be read (logged).
    pub fn chat_room(&self, chat_room_id: &str) -> Option<ChatRoom> {
        let found = self
            .conn
            .prepare_cached("SELECT chat_room_id, row, last_seq, sort_ms FROM chat_room WHERE chat_room_id = ?1")
            .and_then(|mut statement| statement.query_row([chat_room_id], room_of).optional());
        match found {
            Ok(Some(room)) => {
                self.touch(Read::Room(chat_room_id.to_string()));
                Some(room)
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(
                    "Client store {}: chat room {chat_room_id} not read: {err}",
                    self.account
                );
                None
            }
        }
    }

    /// The `limit` rooms of latest activity, newest first.
    pub fn chat_rooms(&self, limit: u32) -> Vec<ChatRoom> {
        self.rows(
            "chat rooms",
            "SELECT chat_room_id, row, last_seq, sort_ms FROM chat_room ORDER BY sort_ms DESC, chat_room_id LIMIT ?1",
            [limit],
            room_of,
        )
    }

    /// Forget a room, its events, its ranges and the cursor of its changes feed.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be removed.
    pub fn remove_chat_room(&self, chat_room_id: &str) -> Result<(), Error> {
        self.atomic(|store| {
            for table in ["chat_event", "chat_range", "chat_room"] {
                store
                    .conn
                    .execute(&format!("DELETE FROM {table} WHERE chat_room_id = ?1"), [chat_room_id])
                    .map_err(failed("chat room not removed"))?;
            }
            store
                .conn
                .execute(
                    "DELETE FROM sync_cursor WHERE name = ?1",
                    [format!("{ROOM_CURSOR}{chat_room_id}")],
                )
                .map_err(failed("chat room not removed"))?;
            Ok(())
        })
    }

    /// Whether the store holds any event of a room.
    pub fn has_chat_events(&self, chat_room_id: &str) -> bool {
        !self
            .rows(
                "chat events",
                "SELECT 1 FROM chat_event WHERE chat_room_id = ?1 LIMIT 1",
                [chat_room_id],
                |row| row.get::<_, i64>(0),
            )
            .is_empty()
    }

    /// The ids of the rooms kept.
    pub fn chat_room_ids(&self) -> Vec<String> {
        self.rows("chat rooms", "SELECT chat_room_id FROM chat_room", [], |row| row.get(0))
    }

    /// Forget a room's events and ranges, keeping its row (a room's changes feed reset: its events are read again).
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be removed.
    pub fn forget_chat_events(&self, chat_room_id: &str) -> Result<(), Error> {
        self.atomic(|store| {
            for table in ["chat_event", "chat_range"] {
                store
                    .conn
                    .execute(&format!("DELETE FROM {table} WHERE chat_room_id = ?1"), [chat_room_id])
                    .map_err(failed("chat events not removed"))?;
            }
            Ok(())
        })
    }

    /// Forget all of chat: rooms, events, ranges and the rooms' feed cursors (the user's feed reset).
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be removed.
    pub fn forget_chat(&self) -> Result<(), Error> {
        self.atomic(|store| {
            for table in ["chat_event", "chat_range", "chat_room"] {
                store
                    .conn
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(failed("chat not removed"))?;
            }
            store.remove_sync_cursors(ROOM_CURSOR)?;
            Ok(())
        })
    }

    /// Forget an event the server no longer has (a change re-read found none).
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be removed.
    pub fn remove_chat_event(&self, chat_room_id: &str, event_seq: i64) -> Result<(), Error> {
        self.conn
            .execute(
                "DELETE FROM chat_event WHERE chat_room_id = ?1 AND event_seq = ?2",
                params![chat_room_id, event_seq],
            )
            .map_err(failed("chat event not removed"))?;
        Ok(())
    }

    /// The room's retention deleted its events up to `through` on the server: they go here too, with the threads
    /// whose root went, and a range that reaches past them starts the timeline now (low 0), as nothing older is left.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be written.
    pub fn trim_chat_retention(&self, chat_room_id: &str, through: i64) -> Result<(), Error> {
        self.atomic(|store| {
            let run = |sql: &str| {
                store
                    .conn
                    .execute(sql, params![chat_room_id, through])
                    .map_err(failed("chat retention not kept"))
            };
            run("DELETE FROM chat_event WHERE chat_room_id = ?1 AND event_seq <= ?2")?;
            run("DELETE FROM chat_range WHERE chat_room_id = ?1 AND thread_seq > 0 AND thread_seq <= ?2")?;
            run("DELETE FROM chat_range WHERE chat_room_id = ?1 AND high <= ?2")?;
            run("UPDATE chat_range SET low = 0 WHERE chat_room_id = ?1 AND low <= ?2 + 1")?;
            Ok(())
        })
    }

    /// The main timeline's newest stretch held is the head again when it reaches `latest`, the room's newest event as
    /// the server says now (its changes feed's head): nothing was missed, so it needs no page read again. False when
    /// no stretch reaches it.
    ///
    /// # Errors
    ///
    /// Returns an error when the ranges cannot be written.
    pub fn restore_chat_head(&self, chat_room_id: &str, latest: i64) -> Result<bool, Error> {
        let Some(range) = self.range_where(chat_room_id, 0, "high >= ?3 ORDER BY high DESC", latest) else {
            return Ok(false);
        };
        self.atomic(|store| {
            let failed_range = failed("chat range not kept");
            store
                .conn
                .execute(
                    "UPDATE chat_range SET head = 0 WHERE chat_room_id = ?1 AND thread_seq = 0",
                    [chat_room_id],
                )
                .map_err(&failed_range)?;
            store
                .conn
                .execute(
                    "UPDATE chat_range SET head = 1 WHERE chat_room_id = ?1 AND thread_seq = 0 AND low = ?2",
                    params![chat_room_id, range.low],
                )
                .map_err(&failed_range)?;
            Ok(true)
        })
    }

    /// Keep events, replacing any of the same room and seq, and raise each kept room's `last_seq` to its newest. This
    /// adds no range: the events are kept, but a page read from the store shows them only inside a range (a page the
    /// server gave, [`Self::put_chat_page`], or a push kept up with it, [`Self::put_chat_pushed`]). An event that cannot
    /// be written is logged and left out; gives how many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit of writes cannot begin or end.
    pub fn put_chat_events(&self, events: &[ChatEvent]) -> Result<usize, Error> {
        self.put_each("chat event", events, |store, event| {
            store
                .write_chat_event(event)
                .map_err(|err| (format!("{} {}", event.chat_room_id, event.event_seq), err))
        })
    }

    /// Keep a page of a room's timeline the server gave for `request` (its `get#Timeline` parameters), with the page's
    /// `hasMoreBefore` and `hasMoreAfter`: its events, and the stretch it covers as a range, joined with the ranges it
    /// touches. A page that reached the latest event makes its range the head, which [`Self::put_chat_pushed`] keeps
    /// up. Gives how many events were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit cannot begin or end, or the range cannot be written.
    pub fn put_chat_page(
        &self,
        chat_room_id: &str,
        request: &EventPage,
        events: &[ChatEvent],
        has_more_before: bool,
        has_more_after: bool,
    ) -> Result<usize, Error> {
        self.atomic(|store| {
            let kept = store.put_chat_events(events)?;
            let seqs = events.iter().map(|event| event.event_seq);
            let (first, last) = (seqs.clone().min(), seqs.max());
            let before = if request.after.is_some() { None } else { request.before };
            // the stretch the page answers for: from what it was asked after (or the start, or its first event) to
            // what it was asked before (or its last event)
            let low = match (request.after, has_more_before, first) {
                (Some(after), _, _) => after + 1,
                (None, false, _) => 0,
                (None, true, Some(first)) => first,
                (None, true, None) => return Ok(kept),
            };
            let high = match (before, last) {
                (Some(before), _) => before - 1,
                (None, Some(last)) => last,
                (None, None) => low.max(request.after.unwrap_or(0)),
            };
            store.add_range(chat_room_id, request.thread.unwrap_or(0), low, high, !has_more_after)?;
            Ok(kept)
        })
    }

    /// Keep an event pushed on `/notws`: written, and the head range of its timeline (or thread) raised to it, as the
    /// socket has been up since that range was read. Call [`Self::mark_chat_heads_stale`] when the socket drops.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be written.
    pub fn put_chat_pushed(&self, event: &ChatEvent) -> Result<(), Error> {
        let size = self.atomic(|store| {
            let size = store.write_chat_event(event).map_err(failed("chat event not kept"))?;
            store
                .conn
                .execute(
                    "UPDATE chat_range SET high = MAX(high, ?1)
                    WHERE chat_room_id = ?2 AND thread_seq = ?3 AND head = 1",
                    params![event.event_seq, event.chat_room_id, event.thread_seq.unwrap_or(0)],
                )
                .map_err(failed("chat range not kept"))?;
            Ok(size)
        })?;
        self.wrote(size);
        Ok(())
    }

    /// Raise the head range of a timeline (or of the thread whose root is `thread`) to `seq`, for a pushed event that is
    /// no row of its own (an edit, a reaction or a redaction, folded into the event it changes): its seq is part of the
    /// stretch the socket has kept up.
    ///
    /// # Errors
    ///
    /// Returns an error when the range cannot be written.
    pub fn raise_chat_head(&self, chat_room_id: &str, thread: Option<i64>, seq: i64) -> Result<(), Error> {
        self.conn
            .execute(
                "UPDATE chat_range SET high = MAX(high, ?1) WHERE chat_room_id = ?2 AND thread_seq = ?3 AND head = 1",
                params![seq, chat_room_id, thread.unwrap_or(0)],
            )
            .map_err(failed("chat range not kept"))?;
        Ok(())
    }

    /// The top of the head range of a timeline (or of the thread whose root is `thread`): the newest seq the store has
    /// kept up with pushes. None when no range is the head (never read, or the socket dropped since).
    pub fn chat_head(&self, chat_room_id: &str, thread: Option<i64>) -> Option<i64> {
        self.rows(
            "chat head",
            "SELECT high FROM chat_range WHERE chat_room_id = ?1 AND thread_seq = ?2 AND head = 1",
            params![chat_room_id, thread.unwrap_or(0)],
            |row| row.get(0),
        )
        .pop()
    }

    /// The event `event_seq` of a room, whether or not it is in a range; none when it is not kept or cannot be read
    /// (logged). For folding a pushed change into the event it changes.
    pub fn chat_event(&self, chat_room_id: &str, event_seq: i64) -> Option<ChatEvent> {
        self.rows(
            "chat event",
            "SELECT chat_room_id, event_seq, event, thread_seq FROM chat_event WHERE chat_room_id = ?1 AND event_seq = ?2",
            params![chat_room_id, event_seq],
            event_of,
        )
        .pop()
    }

    /// No range is the head any more: pushes may have been missed (the socket dropped, the app stopped), so newer
    /// events are a gap until a page from the server reaches the latest again.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be written.
    pub fn mark_chat_heads_stale(&self) -> Result<(), Error> {
        self.conn
            .execute("UPDATE chat_range SET head = 0 WHERE head = 1", [])
            .map_err(failed("chat heads not kept"))?;
        Ok(())
    }

    /// A page of a room's events as `get#Timeline` gives it, oldest first, within the range the page starts in; reading
    /// counts as a read of the room. No events, and both gaps, when the store holds no range there.
    pub fn chat_events(&self, chat_room_id: &str, page: &EventPage) -> EventSlice {
        self.touch(Read::Room(chat_room_id.to_string()));
        let limit = page.limit.clamp(1, PAGE_MAX);
        let thread = page.thread.unwrap_or(0);
        let after = page.after;
        let before = if after.is_some() { None } else { page.before };
        let found = match (after, before) {
            (Some(after), _) => self.range_where(chat_room_id, thread, "low <= ?3 + 1 AND high >= ?3", after),
            (None, Some(before)) => self.range_where(chat_room_id, thread, "low <= ?3 AND high >= ?3 - 1", before),
            (None, None) => self.range_where(chat_room_id, thread, "?3 = ?3 ORDER BY head DESC, high DESC", 0),
        };
        let Some(range) = found else {
            return EventSlice {
                gap_before: true,
                gap_after: true,
                ..EventSlice::default()
            };
        };
        let take = i64::from(limit) + 1;
        let (from, to, ascending) = match (after, before) {
            (Some(after), _) => ((after + 1).max(range.low), range.high, true),
            (None, Some(before)) => (range.low, (before - 1).min(range.high), false),
            (None, None) => (range.low, range.high, false),
        };
        let mut events = self.events_between(chat_room_id, page.thread, from, to, ascending, take);
        let more = events.len() > usize::try_from(limit).unwrap_or(usize::MAX);
        events.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        if !ascending {
            events.reverse();
        }
        let (has_more_before, has_more_after) = match (after, before) {
            (Some(after), _) => (self.any_between(chat_room_id, page.thread, range.low, after), more),
            (None, Some(before)) => (more, self.any_between(chat_room_id, page.thread, before, range.high)),
            (None, None) => (more, false),
        };
        EventSlice {
            events,
            has_more_before,
            has_more_after,
            gap_before: !has_more_before && range.low > 0,
            gap_after: !has_more_after && !range.head,
        }
    }

    /// Every event kept whose `expireDate` is at or before `now_ms` goes: its time came, on the server too. Gives the
    /// rooms it went from (their kept pages, which may show it, are the caller's to drop). A room whose rows cannot be
    /// removed is logged and left; they are due again next time.
    ///
    /// # Errors
    ///
    /// Returns an error when the due events cannot be read.
    pub fn expire_chat_events(&self, now_ms: i64) -> Result<Vec<String>, Error> {
        let due = self.query_events(
            "SELECT chat_room_id, event_seq, event, thread_seq FROM chat_event WHERE expire_ms <= ?1",
            params![now_ms],
        )?;
        let mut by_room: BTreeMap<String, Vec<ChatEvent>> = BTreeMap::new();
        for event in due {
            by_room.entry(event.chat_room_id.clone()).or_default().push(event);
        }
        let mut touched = Vec::new();
        for (room, gone) in by_room {
            match self.remove_expired(&room, &gone) {
                Ok(()) => touched.push(room),
                Err(err) => tracing::warn!(
                    "Client store {}: expired events of {room} not removed: {err}",
                    self.account
                ),
            }
        }
        if !touched.is_empty() {
            self.checkpoint();
        }
        Ok(touched)
    }

    /// The server says events of a room went (its `expired` push, or its changes feed's marker): those of `seqs`, and
    /// every one whose `expireDate` is at or before `before`. Gives how many of them were kept here.
    ///
    /// # Errors
    ///
    /// Returns an error when the events cannot be read or removed.
    pub fn expire_room_events(&self, chat_room_id: &str, before: Option<i64>, seqs: &[i64]) -> Result<usize, Error> {
        let mut gone = match before {
            Some(before) => self.query_events(
                "SELECT chat_room_id, event_seq, event, thread_seq FROM chat_event
                WHERE chat_room_id = ?1 AND expire_ms <= ?2",
                params![chat_room_id, before],
            )?,
            None => Vec::new(),
        };
        for seq in seqs {
            if gone.iter().any(|event| event.event_seq == *seq) {
                continue;
            }
            if let Some(event) = self.chat_event(chat_room_id, *seq) {
                gone.push(event);
            }
        }
        if gone.is_empty() {
            return Ok(0);
        }
        self.remove_expired(chat_room_id, &gone)?;
        self.checkpoint();
        Ok(gone.len())
    }

    /// Remove the expired events `gone` of a room in one unit: their rows, a reply's count on its thread's root (as a
    /// redaction takes it down; a reply redacted already was taken off then), and the room's last message when it was
    /// one of them.
    fn remove_expired(&self, chat_room_id: &str, gone: &[ChatEvent]) -> Result<(), Error> {
        let seqs: BTreeSet<i64> = gone.iter().map(|event| event.event_seq).collect();
        self.atomic(|store| {
            for event in gone {
                store
                    .conn
                    .prepare_cached("DELETE FROM chat_event WHERE chat_room_id = ?1 AND event_seq = ?2")
                    .and_then(|mut statement| statement.execute(params![chat_room_id, event.event_seq]))
                    .map_err(failed("expired chat event not removed"))?;
            }
            let mut replies: BTreeMap<i64, i64> = BTreeMap::new();
            for event in gone {
                let Some(root) = event.thread_seq else { continue };
                let redacted = serde_json::from_str::<Value>(&event.event)
                    .is_ok_and(|item| item["redacted"].as_bool() == Some(true));
                if !redacted && !seqs.contains(&root) {
                    *replies.entry(root).or_default() += 1;
                }
            }
            for (root, count) in replies {
                let Some(kept) = store.chat_event(chat_room_id, root) else {
                    continue;
                };
                let Ok(mut item) = serde_json::from_str::<Value>(&kept.event) else {
                    continue;
                };
                item["threadReplyCount"] = json!((json_i64(&item["threadReplyCount"]).unwrap_or(0) - count).max(0));
                let changed = ChatEvent {
                    event: item.to_string(),
                    ..kept
                };
                store
                    .write_chat_event(&changed)
                    .map_err(failed("thread root not kept"))?;
            }
            store.set_back_last_message(chat_room_id, &seqs)
        })
    }

    /// The room's last message, when it is one of `gone`: the newest message of its timeline still kept, or none.
    fn set_back_last_message(&self, chat_room_id: &str, gone: &BTreeSet<i64>) -> Result<(), Error> {
        let Some(mut room) = self.chat_room(chat_room_id) else {
            return Ok(());
        };
        let Ok(mut row) = serde_json::from_str::<Value>(&room.row) else {
            return Ok(());
        };
        if !json_i64(&row["lastMessage"]["eventSeq"]).is_some_and(|seq| gone.contains(&seq)) {
            return Ok(());
        }
        let latest = self.query_events(
            "SELECT chat_room_id, event_seq, event, thread_seq FROM chat_event
            WHERE chat_room_id = ?1 AND thread_seq IS NULL ORDER BY event_seq DESC LIMIT 50",
            params![chat_room_id],
        )?;
        let standing = latest
            .iter()
            .filter_map(|event| serde_json::from_str::<Value>(&event.event).ok())
            .find(|item| item["type"].as_str() == Some("message") && item["redacted"].as_bool() != Some(true));
        row["lastMessage"] = match standing {
            Some(item) => json!({ "eventSeq": item["eventSeq"], "senderUserId": item["senderUserId"],
                "sentDate": item["sentDate"], "snippet": item["bodyText"], "encrypted": item["encrypted"] }),
            None => Value::Null,
        };
        room.row = row.to_string();
        let size = self.write_chat_room(&room).map_err(failed("chat room not kept"))?;
        self.wrote(size);
        Ok(())
    }

    /// The old pages of deleted rows written out of the WAL, so they do not stay there either; logged when it fails.
    /// Inside a unit it waits for a later removal (a checkpoint cannot pass an open write).
    fn checkpoint(&self) {
        if !self.conn.is_autocommit() {
            return;
        }
        let done = self
            .conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get::<_, i64>(0));
        if let Err(err) = done {
            tracing::warn!("Client store {}: WAL not checkpointed: {err}", self.account);
        }
    }

    /// The events of a query; an error when it does not run.
    fn query_events(&self, sql: &str, params: impl rusqlite::Params) -> Result<Vec<ChatEvent>, Error> {
        let mut statement = self.conn.prepare_cached(sql).map_err(failed("chat events not read"))?;
        let rows = statement
            .query_map(params, event_of)
            .map_err(failed("chat events not read"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(failed("chat events not read"))
    }

    fn write_chat_room(&self, room: &ChatRoom) -> rusqlite::Result<i64> {
        let size = ROW_BYTES + text_len(&room.chat_room_id) + text_len(&room.row);
        self.conn
            .prepare_cached(
                "INSERT INTO chat_room (chat_room_id, row, last_seq, sort_ms, read_ms, size)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT (chat_room_id) DO UPDATE SET row = excluded.row,
                    last_seq = MAX(last_seq, excluded.last_seq), sort_ms = excluded.sort_ms,
                    read_ms = excluded.read_ms, size = excluded.size",
            )
            .and_then(|mut statement| {
                statement.execute(params![
                    room.chat_room_id,
                    room.row,
                    room.last_seq,
                    room.sort_ms,
                    self.tick(),
                    size
                ])
            })
            .map(|_| size)
    }

    fn write_chat_event(&self, event: &ChatEvent) -> rusqlite::Result<i64> {
        let size = ROW_BYTES + text_len(&event.chat_room_id) + text_len(&event.event);
        self.conn
            .prepare_cached(
                "INSERT INTO chat_event (chat_room_id, event_seq, event, thread_seq, size, expire_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT (chat_room_id, event_seq) DO UPDATE SET event = excluded.event,
                    thread_seq = excluded.thread_seq, size = excluded.size, expire_ms = excluded.expire_ms",
            )
            .and_then(|mut statement| {
                statement.execute(params![
                    event.chat_room_id,
                    event.event_seq,
                    event.event,
                    event.thread_seq,
                    size,
                    expire_of(&event.event)
                ])
            })?;
        self.conn.execute(
            "UPDATE chat_room SET last_seq = MAX(last_seq, ?1) WHERE chat_room_id = ?2",
            params![event.event_seq, event.chat_room_id],
        )?;
        Ok(size)
    }

    /// Add the range `low..=high` of a timeline (thread 0 for the main one), joined with every range it overlaps or
    /// touches; the joined range is the head when one at its top was, or when this one is and reaches its top. A new
    /// head ends any other.
    fn add_range(&self, chat_room_id: &str, thread: i64, low: i64, high: i64, head: bool) -> Result<(), Error> {
        let touching: Vec<(i64, i64, bool)> = {
            let mut statement = self
                .conn
                .prepare_cached(
                    "SELECT low, high, head FROM chat_range
                    WHERE chat_room_id = ?1 AND thread_seq = ?2 AND low <= ?4 + 1 AND high + 1 >= ?3",
                )
                .map_err(failed("chat ranges not read"))?;
            let rows = statement
                .query_map(params![chat_room_id, thread, low, high], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0))
                })
                .map_err(failed("chat ranges not read"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(failed("chat ranges not read"))?
        };
        if low > high && touching.is_empty() {
            // a page with nothing in it that touches nothing kept: no stretch to keep
            return Ok(());
        }
        let joined_low = touching.iter().map(|range| range.0).fold(low, i64::min);
        let joined_high = touching.iter().map(|range| range.1).fold(high, i64::max);
        let joined_head =
            (head && high >= joined_high) || touching.iter().any(|range| range.2 && range.1 >= joined_high);
        let failed_range = failed("chat range not kept");
        if joined_head {
            self.conn
                .execute(
                    "UPDATE chat_range SET head = 0 WHERE chat_room_id = ?1 AND thread_seq = ?2",
                    params![chat_room_id, thread],
                )
                .map_err(&failed_range)?;
        }
        for range in &touching {
            self.conn
                .execute(
                    "DELETE FROM chat_range WHERE chat_room_id = ?1 AND thread_seq = ?2 AND low = ?3",
                    params![chat_room_id, thread, range.0],
                )
                .map_err(&failed_range)?;
        }
        self.conn
            .execute(
                "INSERT INTO chat_range (chat_room_id, thread_seq, low, high, head) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    chat_room_id,
                    thread,
                    joined_low,
                    joined_high.max(joined_low),
                    joined_head
                ],
            )
            .map_err(&failed_range)?;
        Ok(())
    }

    /// The first range of a timeline matching `condition`, whose `?3` is `seq`.
    fn range_where(&self, chat_room_id: &str, thread: i64, condition: &str, seq: i64) -> Option<Range> {
        let sql = format!(
            "SELECT low, high, head FROM chat_range WHERE chat_room_id = ?1 AND thread_seq = ?2 AND {condition} LIMIT 1"
        );
        let found = self.conn.prepare_cached(&sql).and_then(|mut statement| {
            statement
                .query_row(params![chat_room_id, thread, seq], |row| {
                    Ok(Range {
                        low: row.get(0)?,
                        high: row.get(1)?,
                        head: row.get::<_, i64>(2)? != 0,
                    })
                })
                .optional()
        });
        found.unwrap_or_else(|err| {
            tracing::warn!(
                "Client store {}: chat ranges of {chat_room_id} not read: {err}",
                self.account
            );
            None
        })
    }

    /// Up to `take` events of the main timeline (`thread` none) or a thread with seqs in `from..=to`.
    fn events_between(
        &self,
        chat_room_id: &str,
        thread: Option<i64>,
        from: i64,
        to: i64,
        ascending: bool,
        take: i64,
    ) -> Vec<ChatEvent> {
        let sql = format!(
            "SELECT chat_room_id, event_seq, event, thread_seq FROM chat_event
            WHERE chat_room_id = ?1 AND event_seq BETWEEN ?2 AND ?3
                AND ((?4 IS NULL AND thread_seq IS NULL) OR event_seq = ?4 OR thread_seq = ?4)
            ORDER BY event_seq {} LIMIT ?5",
            if ascending { "ASC" } else { "DESC" }
        );
        self.rows(
            "chat events",
            &sql,
            params![chat_room_id, from, to, thread, take],
            event_of,
        )
    }

    /// Whether the timeline or thread has an event with a seq in `from..=to`.
    fn any_between(&self, chat_room_id: &str, thread: Option<i64>, from: i64, to: i64) -> bool {
        !self.events_between(chat_room_id, thread, from, to, true, 1).is_empty()
    }
}

fn room_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatRoom> {
    Ok(ChatRoom {
        chat_room_id: row.get(0)?,
        row: row.get(1)?,
        last_seq: row.get(2)?,
        sort_ms: row.get(3)?,
    })
}

/// An event's `expireDate` (ms since the epoch), when it has one. Most events have none, so the JSON is parsed only when
/// it names one.
fn expire_of(event: &str) -> Option<i64> {
    if !event.contains("\"expireDate\"") {
        return None;
    }
    let item: Value = serde_json::from_str(event).ok()?;
    json_i64(&item["expireDate"])
}

/// A number of the server's JSON: a number, or a string of one.
fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn event_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatEvent> {
    Ok(ChatEvent {
        chat_room_id: row.get(0)?,
        event_seq: row.get(1)?,
        event: row.get(2)?,
        thread_seq: row.get(3)?,
    })
}
