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

use rusqlite::{params, OptionalExtension};

use super::{failed, text_len, Read, Store, ROW_BYTES};
use moqui_lib::error::Error;

/// The most events a page has, as `get#Timeline`.
const PAGE_MAX: u32 = 200;

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

    /// Forget a room, its events and its ranges.
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
            Ok(())
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
                "INSERT INTO chat_event (chat_room_id, event_seq, event, thread_seq, size) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT (chat_room_id, event_seq) DO UPDATE SET event = excluded.event,
                    thread_seq = excluded.thread_seq, size = excluded.size",
            )
            .and_then(|mut statement| {
                statement.execute(params![
                    event.chat_room_id,
                    event.event_seq,
                    event.event,
                    event.thread_seq,
                    size
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

fn event_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChatEvent> {
    Ok(ChatEvent {
        chat_room_id: row.get(0)?,
        event_seq: row.get(1)?,
        event: row.get(2)?,
        thread_seq: row.get(3)?,
    })
}
