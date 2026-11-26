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

//! Co-edited text files (plans/design-dev-aide.md F15): a Yjs document per file of a Dev AIDE root that its editors
//! type in at once, the text the `Y.Text` [`TEXT_NAME`] (y-codemirror.next's binding of a CodeMirror editor), on the
//! socket protocol of the wiki's documents (moqui-host `notws/collab.rs`). The key is `DevFile:<rootId>:<path>`.
//!
//! Unlike a wiki page's document it is never stored: the room is seeded from the file's text when its first editor
//! joins and dropped when its last one leaves. It is one server's: its frames do not go to the cluster's other
//! servers (each would seed a room of its own), and the host refuses a join while the server has cluster peers, so
//! co-editing is for a shared dev server of one instance. Save stays the editor's: the dev-aide's save pipeline writes the text
//! the user sends, by the hash of the file they read, so one save wins and the other is a conflict, as without
//! co-editing.
//!
//! The dev-aide answers who may join ([`ACCESS_SERVICE`], run as the user): only in shared mode, only a user who may
//! write the file (the tier, the upstream lock, a read-only root), the path confined as a read's is; with the file's
//! text when the room is new, and the bounds: the most bytes of a document ([`TextAccess::max_bytes`], the dev-aide's
//! `max-edit-bytes`; its state, with the edits' history, at most [`STATE_FACTOR`] times that, and one update at most
//! that), the most files a user co-edits at once ([`TextAccess::max_rooms`], counted and joined under one lock) and the
//! most bytes of every room's state together ([`TextAccess::max_total_bytes`], the dev-aide's
//! `coedit-max-total-bytes`). A member's answer is asked again by
//! the [`tick`] every minute (`ACCESS_RECHECK`); one who may no longer write the file is dropped ([`Event::Revoked`]),
//! and one whose answer could not be renewed for three minutes (`ACCESS_STALE`) has their updates refused. Each room
//! stands alone: a failed check or update is logged and the others go on.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

use super::{call_internal, flag, text as field, user_name, Event, Joined, Refusal, ACCESS_RECHECK, ACCESS_STALE};

/// The prefix of a co-edited file's document key.
pub const PREFIX: &str = "DevFile:";
/// The `Y.Text` of the document that holds the file's text (y-codemirror.next's, as the browser binds it).
pub const TEXT_NAME: &str = "codemirror";
/// The dev-aide's answer of who may co-edit a file.
pub const ACCESS_SERVICE: &str = "org.moqui.devaide.WorkspaceServices.get#CoeditAccess";
/// A document's state, with its edits' history, may take this many times the most bytes of its text.
pub const STATE_FACTOR: usize = 4;
/// The most files a user co-edits at once when the dev-aide gives no number.
const DEFAULT_MAX_ROOMS: usize = 20;
/// The most bytes of every room's state together when the dev-aide gives no number.
const DEFAULT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// Whether `key` names a co-edited file.
#[must_use]
pub fn is_key(key: &str) -> bool {
    key.starts_with(PREFIX)
}

/// The key of `path` of the root `root_id`.
#[must_use]
pub fn key_of(root_id: &str, path: &str) -> String {
    format!("{PREFIX}{root_id}:{path}")
}

/// The root id and the path of a key; the path is confined by the dev-aide, here only its form is checked.
fn parse_key(key: &str) -> Result<(&str, &str), Refusal> {
    let refused = || Refusal::new(404, format!("{key} is not a file that is co-edited"));
    let (root, path) = key
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.split_once(':'))
        .ok_or_else(refused)?;
    let root_ok = !root.is_empty()
        && root
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.');
    let path_ok = !path.is_empty() && !path.starts_with('/') && !path.chars().any(char::is_control);
    if root_ok && path_ok {
        Ok((root, path))
    } else {
        Err(refused())
    }
}

/// What the dev-aide answers for a user and a file.
#[derive(Debug, Clone)]
pub struct TextAccess {
    pub can_edit: bool,
    /// Why not, when the user may not.
    pub reason: String,
    /// The file's text, when it was asked for.
    pub text: Option<String>,
    pub max_bytes: usize,
    pub max_rooms: usize,
    /// The most bytes of every text room's state on the server together.
    pub max_total_bytes: usize,
}

/// What [`access`] answers in a test in place of the dev-aide's, by user: `Err` a failed check of that code.
#[cfg(test)]
pub(crate) static ACCESS_FOR_TEST: Mutex<Option<HashMap<String, Result<TextAccess, u16>>>> = Mutex::new(None);
/// Run once by the next [`access`] in a test, before it answers: what another thread does meanwhile.
#[cfg(test)]
pub(crate) static ON_ACCESS_FOR_TEST: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// The dev-aide's answer for `user_id` and the file, with its text when `with_text`.
fn access(root_id: &str, path: &str, user_id: &str, with_text: bool) -> Result<TextAccess, Refusal> {
    #[cfg(test)]
    if let Some(meanwhile) = lock(&ON_ACCESS_FOR_TEST).take() {
        meanwhile();
    }
    #[cfg(test)]
    if let Some(answers) = &*lock(&ACCESS_FOR_TEST) {
        return match answers.get(user_id) {
            Some(Ok(answer)) => Ok(TextAccess {
                text: answer.text.clone().filter(|_| with_text),
                ..answer.clone()
            }),
            Some(Err(code)) => Err(Refusal::new(*code, "the access check failed")),
            None => Err(Refusal::new(403, "not a writer of the file")),
        };
    }
    let out = call_internal(
        ACCESS_SERVICE,
        &[
            ("rootId", Value::string(root_id)),
            ("path", Value::string(path)),
            ("userId", Value::string(user_id)),
            ("withText", Value::Bool(with_text)),
        ],
    )
    .map_err(|err| Refusal::new(404, err.message()))?;
    let number = |name: &str, default: usize| {
        field(&out, name)
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or(default)
    };
    Ok(TextAccess {
        can_edit: flag(&out, "canEdit"),
        reason: field(&out, "reason"),
        text: out
            .get("text")
            .filter(|_| with_text)
            .map(moqui_lib::object::to_plain_string),
        max_bytes: number("maxBytes", 2 * 1024 * 1024),
        max_rooms: number("maxRooms", DEFAULT_MAX_ROOMS),
        max_total_bytes: number("maxTotalBytes", DEFAULT_MAX_TOTAL_BYTES),
    })
}

/// A member's answer, as last asked, and their clients in the room.
struct Grant {
    checked: Instant,
    retry_at: Instant,
    failures: u32,
    clients: usize,
}

struct TextRoom {
    doc: Doc,
    root_id: String,
    path: String,
    members: usize,
    grants: HashMap<String, Grant>,
    /// The size of the seed and of the updates applied since: what [`apply`] holds to `max_bytes` times
    /// [`STATE_FACTOR`].
    state_bytes: usize,
    max_bytes: usize,
    max_total_bytes: usize,
    /// Taken out of the rooms: a join that found it before then seeds it again.
    closed: bool,
}

fn rooms() -> &'static Mutex<HashMap<String, Arc<Mutex<TextRoom>>>> {
    static ROOMS: OnceLock<Mutex<HashMap<String, Arc<Mutex<TextRoom>>>>> = OnceLock::new();
    ROOMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn room_of(key: &str) -> Option<Arc<Mutex<TextRoom>>> {
    lock(rooms()).get(key).cloned()
}

/// The bytes of every open room's state together, which [`TextAccess::max_total_bytes`] bounds.
static TOTAL_BYTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn total_bytes() -> usize {
    TOTAL_BYTES.load(std::sync::atomic::Ordering::SeqCst)
}

/// Take `room` out of the rooms (`all`, locked) and its bytes out of the total.
fn close(all: &mut HashMap<String, Arc<Mutex<TextRoom>>>, key: &str, room: &mut TextRoom) {
    if room.closed {
        return;
    }
    room.closed = true;
    all.remove(key);
    TOTAL_BYTES.fetch_sub(room.state_bytes, std::sync::atomic::Ordering::SeqCst);
}

/// The keys of the files co-edited on this server.
pub(crate) fn open_keys() -> Vec<String> {
    lock(rooms()).keys().cloned().collect()
}

/// The rooms `user_id` is a member of, but `except`.
fn rooms_of(user_id: &str, except: &str) -> usize {
    let open: Vec<(String, Arc<Mutex<TextRoom>>)> = lock(rooms())
        .iter()
        .filter(|(key, _)| key.as_str() != except)
        .map(|(key, room)| (key.clone(), Arc::clone(room)))
        .collect();
    open.iter()
        .filter(|(_, room)| lock(room).grants.contains_key(user_id))
        .count()
}

/// The room of `key`, seeded from `seed` (the file's text) when there is none; `None` when there is none and no seed,
/// for the caller to read the file and ask again: a room is never seeded from a text that was not read.
fn room_seeded(
    key: &str,
    root_id: &str,
    path: &str,
    seed: Option<&str>,
    bounds: (usize, usize),
) -> Result<Option<Arc<Mutex<TextRoom>>>, Refusal> {
    let (max_bytes, max_total_bytes) = bounds;
    if let Some(room) = room_of(key) {
        return Ok(Some(room));
    }
    let Some(seed) = seed else { return Ok(None) };
    if seed.len() > max_bytes {
        return Err(Refusal::new(413, format!("the file is larger than {max_bytes} bytes")));
    }
    let doc = Doc::new();
    {
        let text = doc.get_or_insert_text(TEXT_NAME);
        let mut txn = doc.transact_mut();
        text.insert(&mut txn, 0, seed);
    }
    let state_bytes = doc.transact().encode_state_as_update_v1(&StateVector::default()).len();
    let room = Arc::new(Mutex::new(TextRoom {
        doc,
        root_id: root_id.to_string(),
        path: path.to_string(),
        members: 0,
        grants: HashMap::new(),
        state_bytes,
        max_bytes,
        max_total_bytes,
        closed: false,
    }));
    let mut all = lock(rooms());
    // another join may have seeded it meanwhile: keep the first
    if let Some(first) = all.get(key) {
        return Ok(Some(Arc::clone(first)));
    }
    if total_bytes().saturating_add(state_bytes) > max_total_bytes {
        return Err(Refusal::new(
            503,
            "the server co-edits as many bytes of files as it may: try again later",
        ));
    }
    TOTAL_BYTES.fetch_add(state_bytes, std::sync::atomic::Ordering::SeqCst);
    all.insert(key.to_string(), Arc::clone(&room));
    Ok(Some(room))
}

/// A client of `user_id` joins the file: the dev-aide's answer, the room (seeded from the file when new), the update the
/// client's state vector lacks. Refused (403, with the dev-aide's reason) for a user who may not write the file, and
/// (429) for one who co-edits as many files as they may.
pub(crate) fn join(key: &str, user_id: &str, client_state_vector: &[u8]) -> Result<Joined, Refusal> {
    let (root_id, path) = parse_key(key)?;
    let mut answer = access(root_id, path, user_id, room_of(key).is_none())?;
    if !answer.can_edit {
        let reason = if answer.reason.is_empty() {
            "you may not change this file".to_string()
        } else {
            answer.reason.clone()
        };
        return Err(Refusal::new(403, reason));
    }
    if rooms_of(user_id, key) >= answer.max_rooms {
        return Err(Refusal::new(
            429,
            format!("you co-edit {} files already: close one first", answer.max_rooms),
        ));
    }
    // the client's state vector first: a refused join is no member
    let vector = if client_state_vector.is_empty() {
        StateVector::default()
    } else {
        StateVector::decode_v1(client_state_vector)
            .map_err(|err| Refusal::new(400, format!("not a Yjs state vector: {err}")))?
    };
    // the room, or a new one seeded from the file's text; a room the last leave closed between the lookup and the join,
    // or one gone before a text was read, is seeded again from the file's text read now, never from no text
    let mut seed = answer.text.take();
    let mut tries = 0;
    let room = loop {
        let found = room_seeded(
            key,
            root_id,
            path,
            seed.take().as_deref(),
            (answer.max_bytes, answer.max_total_bytes),
        )?;
        if let Some(room) = found {
            // the rooms of the user counted and the user put in this one under one lock, so two joins at once do not
            // both pass the bound
            let mut all = lock(rooms());
            let others = all
                .iter()
                .filter(|(other, room)| other.as_str() != key && lock(room).grants.contains_key(user_id))
                .count();
            let mut held = lock(&room);
            if !held.closed {
                if others >= answer.max_rooms {
                    if held.members == 0 {
                        close(&mut all, key, &mut held);
                    }
                    return Err(Refusal::new(
                        429,
                        format!("you co-edit {} files already: close one first", answer.max_rooms),
                    ));
                }
                held.members += 1;
                let now = Instant::now();
                let grant = held.grants.entry(user_id.to_string()).or_insert(Grant {
                    checked: now,
                    retry_at: now,
                    failures: 0,
                    clients: 0,
                });
                grant.clients += 1;
                grant.checked = now;
                grant.retry_at = now;
                grant.failures = 0;
                drop(held);
                drop(all);
                break room;
            }
        }
        tries += 1;
        if tries > 3 {
            return Err(Refusal::new(
                503,
                "the file's room closed as it was joined: join it again",
            ));
        }
        let mut fresh = access(root_id, path, user_id, true)?;
        if !fresh.can_edit {
            return Err(Refusal::new(403, "you may not change this file"));
        }
        seed = Some(
            fresh
                .text
                .take()
                .ok_or_else(|| Refusal::new(500, "the dev-aide answered no text of the file"))?,
        );
        answer = fresh;
    };
    let room = lock(&room);
    let txn = room.doc.transact();
    let update = txn.encode_state_as_update_v1(&vector);
    let state_vector = txn.state_vector().encode_v1();
    drop(txn);
    drop(room);
    Ok(Joined {
        can_edit: true,
        update,
        state_vector,
        user_name: user_name(user_id),
    })
}

/// An update from a member; refused when the user did not join, their answer is stale, or it would take the document
/// past its bounds.
pub(crate) fn apply(key: &str, user_id: &str, update: &[u8]) -> Result<(), Refusal> {
    let room = room_of(key).ok_or_else(|| Refusal::new(409, format!("{key} is not open here: join it first")))?;
    let mut room = lock(&room);
    match room.grants.get(user_id) {
        Some(grant) if grant.checked.elapsed() > ACCESS_STALE => {
            return Err(Refusal::new(
                403,
                "your right to change this file could not be checked again",
            ))
        }
        Some(_) => {}
        None => return Err(Refusal::new(409, format!("{key} is not open here: join it first"))),
    }
    if update.len() > room.max_bytes {
        return Err(Refusal::new(
            413,
            format!("an update is larger than {} bytes", room.max_bytes),
        ));
    }
    let max = room.max_bytes.saturating_mul(STATE_FACTOR);
    if room.state_bytes.saturating_add(update.len()) > max {
        return Err(Refusal::new(
            413,
            format!("the co-edited file is larger than {max} bytes: save it and open it again"),
        ));
    }
    if total_bytes().saturating_add(update.len()) > room.max_total_bytes {
        return Err(Refusal::new(
            413,
            "the server co-edits as many bytes of files as it may: save the file and open it again",
        ));
    }
    let size = update.len();
    let update = Update::decode_v1(update).map_err(|err| Refusal::new(400, format!("not a Yjs update: {err}")))?;
    room.doc
        .transact_mut()
        .apply_update(update)
        .map_err(|err| Refusal::new(400, format!("the update was not applied: {err}")))?;
    room.state_bytes = room.state_bytes.saturating_add(size);
    TOTAL_BYTES.fetch_add(size, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// An update another server applied: into this server's room, when it has the file open.
/// Not applied: a co-edited file's room is this server's alone, and another server's room of the file was seeded apart
/// from it, so its update would only wait in this document as pending state that never integrates.
pub(crate) fn apply_remote(key: &str, _update: &[u8]) {
    tracing::debug!("co-edited {key}: an update from another server is not applied: a file's room is one server's");
}

/// A client of `user_id` left: the room counts one member fewer, the user's answer goes with their last client, and the
/// room is dropped with its last member.
pub(crate) fn leave(key: &str, user_id: Option<&str>) {
    let Some(room) = room_of(key) else { return };
    let mut all = lock(rooms());
    let mut room = lock(&room);
    if let Some(user_id) = user_id {
        let last = room.grants.get_mut(user_id).is_some_and(|grant| {
            grant.clients = grant.clients.saturating_sub(1);
            grant.clients == 0
        });
        if last {
            room.grants.remove(user_id);
        }
    }
    room.members = room.members.saturating_sub(1);
    if room.members == 0 {
        close(&mut all, key, &mut room);
    }
}

/// The text of a co-edited file now.
pub(crate) fn snapshot(key: &str) -> Option<String> {
    room_of(key).map(|room| {
        let room = lock(&room);
        let text = room.doc.get_or_insert_text(TEXT_NAME);
        let txn = room.doc.transact();
        text.get_string(&txn)
    })
}

/// Ask again the dev-aide's answer of each member whose answer is [`ACCESS_RECHECK`] old: one who may no longer write
/// the file is dropped ([`Event::Revoked`]); a check that fails is tried again later (5 s, 10 s, ... at most a minute),
/// and after [`ACCESS_STALE`] the member is dropped. Each room and member stands alone.
pub(crate) fn tick(events: &mut Vec<Event>) {
    let open: Vec<(String, Arc<Mutex<TextRoom>>)> = lock(rooms())
        .iter()
        .map(|(key, room)| (key.clone(), Arc::clone(room)))
        .collect();
    for (key, room) in open {
        let (root_id, path, due): (String, String, Vec<String>) = {
            let room = lock(&room);
            let now = Instant::now();
            let due = room
                .grants
                .iter()
                .filter(|(_, grant)| grant.checked.elapsed() >= ACCESS_RECHECK && grant.retry_at <= now)
                .map(|(user, _)| user.clone())
                .collect();
            (room.root_id.clone(), room.path.clone(), due)
        };
        for user_id in due {
            let mut revoke = |user_id: String| {
                lock(&room).grants.remove(&user_id);
                events.push(Event::Revoked {
                    document_key: key.clone(),
                    user_id,
                });
            };
            match access(&root_id, &path, &user_id, false) {
                Ok(found) if found.can_edit => {
                    if let Some(grant) = lock(&room).grants.get_mut(&user_id) {
                        grant.checked = Instant::now();
                        grant.retry_at = grant.checked;
                        grant.failures = 0;
                    }
                }
                Ok(_) => revoke(user_id),
                Err(refusal) => {
                    tracing::warn!(
                        "co-edited {key}: the access of {user_id} was not checked again: {}",
                        refusal.message
                    );
                    let stale = lock(&room)
                        .grants
                        .get(&user_id)
                        .is_some_and(|grant| grant.checked.elapsed() > ACCESS_STALE);
                    if stale {
                        revoke(user_id);
                    } else if let Some(grant) = lock(&room).grants.get_mut(&user_id) {
                        grant.failures = grant.failures.saturating_add(1);
                        let wait = Duration::from_secs(5u64.saturating_mul(1 << grant.failures.min(4)).min(60));
                        grant.retry_at = Instant::now() + wait;
                    }
                }
            }
        }
    }
}

/// Age every member's answer of `key` by `by`, as if it were asked that long ago.
#[cfg(test)]
pub(crate) fn age_grants(key: &str, by: Duration) {
    if let Some(room) = room_of(key) {
        for grant in lock(&room).grants.values_mut() {
            grant.checked = grant.checked.checked_sub(by).unwrap_or(grant.checked);
            grant.retry_at = grant.retry_at.checked_sub(by).unwrap_or(grant.retry_at);
        }
    }
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
