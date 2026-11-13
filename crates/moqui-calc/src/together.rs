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

//! Editing together (plans/IronCalc-DB.md): a saved document is open in a room for each scope of its readers, every
//! room follows the document's edit log ([`crate::log`]), and the people in them see where the others are.
//!
//! - [`catch_up`] brings a room up to the log's last edit: before a room is read, under the log's lock before an edit,
//!   and when the host is told of an edit on any server ([`catch_up_document`], `notws/calc.rs` in moqui-host).
//! - [`edit`], [`save`] and [`revert`] lock the log, so edits of every room and server are one order.
//! - The presence ([`Presence`]) is who has a document open and where: a name, a sheet and a selection, never a value.
//!   The host's socket sets it for a browser, a terminal's reads set it for a terminal, and both read it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::{Error, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use crate::log::{self, LogRow};
use crate::room::{single_kind, Room, Rooms, SharedRoom, REOPEN};

/// A viewer not heard from for this long is no longer shown.
pub const PRESENCE_TTL: Duration = Duration::from_secs(60);

/// Bring a shared room up to its document's last edit. True when it applied any.
///
/// # Errors
///
/// [`REOPEN`] when the room cannot follow the log (it is behind a revert, the rows it needs are gone, or it cannot take
/// back an undone edit): the caller drops it, and its viewers open the document again.
pub fn catch_up(facade: &EntityFacade, room: &mut Room) -> Result<bool, Error> {
    let Some(definition_id) = room.definition_id.clone().filter(|_| room.shared) else {
        return Ok(false);
    };
    let Some(head) = log::head(facade, &definition_id)? else {
        return Ok(false);
    };
    if head.last_seq <= room.seq {
        return Ok(false);
    }
    let rows = log::rows_after(facade, &definition_id, room.seq)?;
    follow(room, &rows, &head)?;
    Ok(true)
}

/// Apply `rows` (the log after the room's last edit) to the room, and evaluate once.
fn follow(room: &mut Room, rows: &[LogRow], head: &log::Head) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    if rows.first().map(|row| row.seq) != Some(room.seq + 1) {
        return Err(Error::new(REOPEN));
    }
    room.pause();
    let followed = rows.iter().try_for_each(|row| room.apply_row(row));
    room.evaluate();
    followed.map_err(|_| Error::new(REOPEN))?;
    room.saved_through(head.saved_seq);
    if head.saved_version.is_some() {
        room.version = head.saved_version;
    }
    Ok(())
}

/// Bring every room of `definition_id` on this server up to the log: what the host does when any server announces an
/// edit. The ids of the rooms that changed, and of those dropped (their viewers open the document again).
#[must_use]
pub fn catch_up_document(factory: &ExecutionContextFactory, definition_id: &str) -> (Vec<String>, Vec<String>) {
    let rooms = Rooms::of(factory);
    let Some(facade) = factory.entity() else {
        return (Vec::new(), Vec::new());
    };
    let (mut changed, mut dropped) = (Vec::new(), Vec::new());
    for room in rooms.document_rooms(definition_id) {
        let mut held = lock(&room);
        match catch_up(&facade, &mut held) {
            Ok(true) => changed.push(held.id.clone()),
            Ok(false) => {}
            Err(err) => {
                tracing::info!(room = %held.id, error = %err, "A CalcGrid room is opened anew");
                dropped.push(held.id.clone());
            }
        }
    }
    for id in &dropped {
        rooms.drop_room(id);
    }
    (changed, dropped)
}

/// What an edit of a shared room did: the log's edit it is, and the refusal that stopped it part way.
pub struct Edited {
    pub seq: i64,
    pub kind: &'static str,
    pub refused: Option<Error>,
}

/// Apply `ops` of `user_id` to a shared room as the next edit of its document's log, then evaluate. A lone `undo` or
/// `redo` takes back the person's own last edit, or their last undo, since the document was saved.
///
/// # Errors
///
/// An error when nothing was written: the log refused, the room could not follow it ([`REOPEN`]: the caller drops it),
/// or there was nothing to undo or redo. A refusal part way is [`Edited::refused`], with the edits before it written.
pub fn edit(facade: &EntityFacade, room: &mut Room, user_id: &str, ops: &[Json]) -> Result<Edited, Error> {
    let definition_id = room
        .definition_id
        .clone()
        .ok_or_else(|| Error::new("Not a saved document"))?;
    let kind = single_kind(ops).map(str::to_string);
    // set once the room is changed by this edit: a log that then fails leaves it ahead of the log, so it is opened anew
    let mut touched = false;
    let written = log::locked(facade, &definition_id, user_id, |log| {
        let rows = log.rows_after(room.seq)?;
        follow(room, &rows, &log.head)?;
        if log.head.saved_version.is_none() {
            log.head.saved_version = room.version;
        }
        let base = log.head.saved_seq;
        room.pause();
        match kind.as_deref() {
            Some(log::UNDO) => {
                let mine = log.rows_after(base)?;
                let target = log::undo_target(&mine, user_id, base)
                    .map(|row| row.seq)
                    .ok_or_else(|| Error::new("Nothing of yours to undo since the document was saved"))?;
                touched = true;
                room.undo_row(target)?;
                let seq = log.append(log::UNDO, &[], Some(target))?;
                log.mark_undone(target, seq)?;
                room.seq = seq;
                Ok(Edited {
                    seq,
                    kind: log::UNDO,
                    refused: None,
                })
            }
            Some(log::REDO) => {
                let mine = log.rows_after(base)?;
                let undo = log::redo_target(&mine, user_id, base)
                    .cloned()
                    .ok_or_else(|| Error::new("Nothing of yours to redo"))?;
                let original = mine
                    .iter()
                    .find(|row| Some(row.seq) == undo.target)
                    .map(|row| row.ops.clone())
                    .unwrap_or_default();
                touched = true;
                let (applied, inverse, refused) = room.apply_ops(&original);
                let seq = log.append(log::REDO, &applied, Some(undo.seq))?;
                log.mark_undone(undo.seq, seq)?;
                room.record(seq, inverse);
                room.seq = seq;
                Ok(Edited {
                    seq,
                    kind: log::REDO,
                    refused,
                })
            }
            _ => {
                touched = true;
                let (applied, inverse, refused) = room.apply_ops(ops);
                if applied.is_empty() {
                    return Ok(Edited {
                        seq: room.seq,
                        kind: log::EDIT,
                        refused,
                    });
                }
                let seq = log.append(log::EDIT, &applied, None)?;
                room.record(seq, inverse);
                room.seq = seq;
                Ok(Edited {
                    seq,
                    kind: log::EDIT,
                    refused,
                })
            }
        }
    });
    room.evaluate();
    let edited = match written {
        Ok(edited) => edited,
        Err(err) if touched => {
            tracing::warn!(room = %room.id, error = %err, "A CalcGrid edit was not written to its log; the room is opened anew");
            return Err(Error::new(REOPEN));
        }
        Err(err) => return Err(err),
    };
    if edited.seq > 0 {
        log::announce(facade, &definition_id, edited.seq, user_id, edited.kind);
    }
    Ok(edited)
}

/// Whether `user_id` has an edit of their own to undo in the room, and an undo to redo: from the document's log for a
/// shared room, from the room's own for a scratch one. Neither when the log cannot be read.
#[must_use]
pub fn undo_state(facade: &EntityFacade, room: &Room, user_id: &str) -> (bool, bool) {
    let Some(definition_id) = room.definition_id.as_deref().filter(|_| room.shared) else {
        return room.undo_state(user_id);
    };
    let base = log::head(facade, definition_id)
        .ok()
        .flatten()
        .map_or(0, |head| head.saved_seq);
    log::rows_after(facade, definition_id, base)
        .map(|rows| log::undo_state(&rows, user_id, base))
        .unwrap_or((false, false))
}

/// How many edits of the room's document are not saved yet.
#[must_use]
pub fn unsaved(facade: &EntityFacade, room: &Room) -> i64 {
    room.definition_id
        .as_deref()
        .filter(|_| room.shared)
        .and_then(|definition_id| log::head(facade, definition_id).ok().flatten())
        .map_or(0, |head| (head.last_seq - head.saved_seq).max(0))
}

/// Save a shared room's document with `write`, under the log's lock after bringing the room up to it: the version
/// `write` gets is the saved one, at the room's last edit. `write` answers the version it saved.
///
/// # Errors
///
/// `write`'s error, or the log's; nothing is saved then.
pub fn save(
    facade: &EntityFacade,
    room: &mut Room,
    user_id: &str,
    write: impl FnOnce(&Room, Option<i64>) -> Result<Option<i64>, Error>,
) -> Result<Option<i64>, Error> {
    let definition_id = room
        .definition_id
        .clone()
        .ok_or_else(|| Error::new("Not a saved document"))?;
    let saved = log::locked(facade, &definition_id, user_id, |log| {
        let rows = log.rows_after(room.seq)?;
        follow(room, &rows, &log.head)?;
        let version = write(room, log.head.saved_version.or(room.version))?;
        log.saved(room.seq, version)?;
        Ok((version, room.seq))
    })?;
    room.version = saved.0;
    room.saved_through(saved.1);
    log::announce(facade, &definition_id, saved.1, user_id, "save");
    Ok(saved.0)
}

/// Begin the log of `definition_id` again from its saved version `version`: a document saved over from a scratch room,
/// or saved outside CalcGrid, or whose unsaved edits are discarded. Every open room of it is opened anew. The log's
/// edit the saved version now is.
///
/// # Errors
///
/// The log's error.
pub fn begin_again(
    facade: &EntityFacade,
    definition_id: &str,
    user_id: &str,
    version: Option<i64>,
) -> Result<i64, Error> {
    let seq = log::locked(facade, definition_id, user_id, |log| {
        if log.head.last_seq == 0 {
            log.head.saved_version = version;
            return Ok(0);
        }
        log.begin_again(version)
    })?;
    if seq > 0 {
        log::announce(facade, definition_id, seq, user_id, log::REVERT);
    }
    Ok(seq)
}

/// The log's state for a new room of `definition_id` at `version`: the edit its saved version holds. A saved version
/// other than the log's was saved outside CalcGrid, and the log begins again from it.
///
/// # Errors
///
/// The log's error.
pub fn opening_seq(
    facade: &EntityFacade,
    definition_id: &str,
    user_id: &str,
    version: Option<i64>,
) -> Result<i64, Error> {
    match log::head(facade, definition_id)? {
        None => Ok(0),
        Some(head) if head.saved_version.is_none() || head.saved_version == version => Ok(head.saved_seq),
        Some(_) => begin_again(facade, definition_id, user_id, version),
    }
}

/// A user's name as the others see it: their full name, else their username.
pub fn display_name(facade: &EntityFacade, user_id: &str) -> String {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    facade
        .find("moqui.security.UserAccount")
        .and_then(|find| find.eq("userId", moqui_lib::object::Value::string(user_id)).one())
        .ok()
        .flatten()
        .map(|account| {
            let full = account
                .get("userFullName")
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default();
            if full.is_empty() {
                account
                    .get("username")
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            } else {
                full
            }
        })
        .unwrap_or_else(|| user_id.to_string())
}

/// A reader of a document as the others see them.
#[derive(Clone, Debug)]
struct Peer {
    user_id: String,
    name: String,
    sheet: Option<u32>,
    area: Json,
    seen: Instant,
}

/// Who has each document open and where, by the document (or a scratch room's id) and each view's client id.
#[derive(Default)]
pub struct Presence {
    by_document: Mutex<HashMap<String, HashMap<String, Peer>>>,
}

impl Presence {
    /// The presence of `factory`.
    #[must_use]
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|_| Self::default())
    }

    /// `client` of `user_id` is at `area` (`row`, `column`, `width`, `height`) of `sheet` in `document`, or only there
    /// with no sheet. Kept for [`PRESENCE_TTL`].
    pub fn set(&self, document: &str, client: &str, user_id: &str, name: &str, sheet: Option<u32>, area: Json) {
        let mut all = lock(&self.by_document);
        all.entry(document.to_string()).or_default().insert(
            client.to_string(),
            Peer {
                user_id: user_id.to_string(),
                name: name.to_string(),
                sheet,
                area,
                seen: Instant::now(),
            },
        );
    }

    /// `client` left `document`.
    pub fn leave(&self, document: &str, client: &str) {
        let mut all = lock(&self.by_document);
        if let Some(peers) = all.get_mut(document) {
            peers.remove(client);
            if peers.is_empty() {
                all.remove(document);
            }
        }
    }

    /// The viewers of `document` seen within [`PRESENCE_TTL`] but `client`: `client`, `userId`, `name`, `sheet`,
    /// `area`.
    #[must_use]
    pub fn peers(&self, document: &str, client: &str) -> Vec<Json> {
        let mut all = lock(&self.by_document);
        let Some(peers) = all.get_mut(document) else {
            return Vec::new();
        };
        peers.retain(|_, peer| peer.seen.elapsed() < PRESENCE_TTL);
        let mut out: Vec<Json> = peers
            .iter()
            .filter(|(id, _)| id.as_str() != client)
            .map(|(id, peer)| {
                json!({"client": id, "userId": peer.user_id, "name": peer.name, "sheet": peer.sheet, "area": peer.area})
            })
            .collect();
        out.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        out
    }
}

/// The key a room's viewers are found by: its saved document, so the rooms of every scope see each other, or a
/// scratch room's own id.
#[must_use]
pub fn presence_key(room: &Room) -> String {
    room.definition_id
        .clone()
        .filter(|_| room.shared)
        .unwrap_or_else(|| room.id.clone())
}

/// Whether `user_id` has the room `room_id` open on this server: what a socket's join is allowed by. The room's
/// presence key with it, when they do.
#[must_use]
pub fn member_of(factory: &ExecutionContextFactory, user_id: &str, room_id: &str) -> Option<String> {
    let room: SharedRoom = Rooms::of(factory).get(user_id, room_id)?;
    let held = lock(&room);
    Some(presence_key(&held))
}

#[cfg(test)]
#[path = "together_tests.rs"]
mod tests;
