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

//! Co-edited documents: a Yjs document (`yrs`, wire-compatible with the browser's Yjs) per record that people edit at
//! once, held in memory on each server that has an editor of it and stored as `moqui.resource.CollabDocument`. The host
//! carries the Yjs sync messages on the notification WebSocket (moqui-host `collab_ws.rs`); this module keeps the rooms:
//! [`join`] (the access check, the document loaded or seeded from the record's text, the diff the client lacks),
//! [`apply`] (an editor's update), [`tick`] (the state written a second or two after an edit, a version of the record
//! after a while without edits on database files, and when the last editor leaves) and [`save`] (a version now, with a
//! note). Guide: `docs/Revision-Management.md`, Co-editing.
//!
//! A document key names its record: `WikiPage:<wikiPageId>` is a wiki page, whose markdown the document is
//! ([`markdown`]); the user needs view access to join and update access to change it (`WikiServices.get#WikiPageAccess`).
//! The page's text is written only as a version (`WikiServices.update#WikiPage`), never on each keystroke, so its history
//! is the saves; the document between saves is its CollabDocument row. A page changed outside co-editing while no one had
//! changes open is read again when the next editor joins; with changes open, the co-edited text is kept (logged) and
//! the next save writes over it, the other change staying in the page's history.

pub mod markdown;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_lib::object::{is_empty, to_plain_string, Value};
use sha2::Digest;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::context::{Error, ExecutionContext};
use crate::service::call::{sync_checked, CallOptions};

const COLLAB_DOCUMENT: &str = "moqui.resource.CollabDocument";
/// The state is written this long after the last edit.
const FLUSH_AFTER: Duration = Duration::from_millis(1500);
/// A document on database files is saved as a version after this long without an edit.
const IDLE_SAVE: Duration = Duration::from_secs(120);

/// Why a message was refused, with the HTTP-like code the client reads.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub code: u16,
    pub message: String,
}

impl Refusal {
    fn new(code: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<Error> for Refusal {
    fn from(err: Error) -> Self {
        Self::new(500, err.message())
    }
}

/// What a joining client gets: whether it may change the document, the update it lacks and the server's state vector.
#[derive(Debug, Clone)]
pub struct Joined {
    pub can_edit: bool,
    pub update: Vec<u8>,
    pub state_vector: Vec<u8>,
    /// The user's name, for the others' presence.
    pub user_name: String,
}

/// Something the host tells the editors of a document.
#[derive(Debug, Clone)]
pub enum Event {
    /// The document was saved as a version of its record.
    Saved {
        document_key: String,
        version_name: Option<String>,
        user_id: String,
        user_name: String,
    },
}

/// The record a document key names.
#[derive(Debug, Clone)]
struct WikiTarget {
    wiki_space_id: String,
    wiki_page_id: String,
    page_path: String,
    page_name: String,
    /// The space keeps its pages on database files (each save a version row), not in git.
    on_database: bool,
}

struct Room {
    doc: Doc,
    target: WikiTarget,
    /// Changed since the state was last written.
    dirty: bool,
    changed_at: Instant,
    /// The users whose changes are in no version yet, and the last of them.
    unsaved: BTreeSet<String>,
    last_editor: Option<String>,
    members: usize,
    can_edit: HashMap<String, bool>,
}

fn rooms() -> &'static Mutex<HashMap<String, Arc<Mutex<Room>>>> {
    static ROOMS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Room>>>>> = OnceLock::new();
    ROOMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn events() -> &'static Mutex<Vec<Event>> {
    static EVENTS: OnceLock<Mutex<Vec<Event>>> = OnceLock::new();
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn room_of(key: &str) -> Option<Arc<Mutex<Room>>> {
    locked(rooms()).get(key).cloned()
}

/// The events since the last call, for the host to send.
pub fn take_events() -> Vec<Event> {
    std::mem::take(&mut *locked(events()))
}

fn text_hash(text: &str) -> String {
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn call_internal(name: &str, parameters: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, Error> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let options = CallOptions {
        disable_authz: true,
        ..Default::default()
    };
    let result = sync_checked(name, &parameters, &options);
    ExecutionContext::with_current(|ec| ec.message().clear_errors());
    result
}

fn text(map: &BTreeMap<String, Value>, key: &str) -> String {
    map.get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

fn flag(map: &BTreeMap<String, Value>, key: &str) -> bool {
    map.get(key)
        .is_some_and(|value| matches!(value, Value::Bool(true)) || to_plain_string(value) == "true")
}

/// The wiki page id of a document key.
fn wiki_page_id(key: &str) -> Result<&str, Refusal> {
    key.strip_prefix("WikiPage:")
        .filter(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
        })
        .ok_or_else(|| Refusal::new(404, format!("{key} is not a document that is co-edited")))
}

/// The user's access to the page.
struct Access {
    can_view: bool,
    can_edit: bool,
    /// The space keeps its pages on database files.
    on_database: bool,
}

fn access(page_id: &str, user_id: &str) -> Result<Access, Refusal> {
    let out = call_internal(
        "org.moqui.impl.WikiServices.get#WikiPageAccess",
        &[
            ("wikiPageId", Value::string(page_id)),
            ("userId", Value::string(user_id)),
        ],
    )
    .map_err(|err| Refusal::new(404, err.message()))?;
    Ok(Access {
        can_view: flag(&out, "canView"),
        can_edit: flag(&out, "canUpdate") && !flag(&out, "readOnly"),
        on_database: text(&out, "storage") == "db",
    })
}

/// The page, its text and the version it is at.
fn read_page(page_id: &str, on_database: bool) -> Result<(WikiTarget, String, Option<String>), Refusal> {
    let info = call_internal(
        "org.moqui.impl.WikiServices.get#WikiPageInfoById",
        &[("wikiPageId", Value::string(page_id))],
    )
    .map_err(|err| Refusal::new(404, err.message()))?;
    let location = text(&info, "pageLocation");
    let (page_text, version) = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        let resources = ec.factory().resource();
        let page_text = resources.location_text(&location).unwrap_or_default();
        let version = resources
            .current_version(&location)
            .ok()
            .flatten()
            .map(|version| version.version_name);
        (page_text, version)
    })
    .ok_or_else(|| Refusal::new(500, "no execution context"))?;
    // the name update#WikiPage keeps the page at its path by: the last segment, the space's own id for its home page
    let wiki_space_id = text(&info, "wikiSpaceId");
    let page_path = text(&info, "pagePath");
    let page_name = match page_path.rsplit_once('/') {
        Some((_, last)) => last.to_string(),
        None if page_path.is_empty() => wiki_space_id.clone(),
        None => page_path.clone(),
    };
    let target = WikiTarget { wiki_space_id, wiki_page_id: page_id.to_string(), page_path, page_name, on_database };
    Ok((target, page_text, version))
}

fn entity() -> Result<crate::entity::EntityFacade, Error> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade"))
}

fn stored(key: &str) -> Result<Option<crate::entity::EntityValue>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    Ok(entity()?
        .find(COLLAB_DOCUMENT)?
        .eq("documentKey", Value::string(key))
        .one()?)
}

fn bytes_of(row: &crate::entity::EntityValue, field: &str) -> Vec<u8> {
    match row.get(field) {
        Some(Value::Bytes(bytes)) => bytes.clone(),
        _ => Vec::new(),
    }
}

fn now_value() -> Value {
    crate::user::now_value()
}

/// The room of a document, loaded from its row or seeded from the page's text.
fn load(key: &str, page_id: &str, on_database: bool) -> Result<Arc<Mutex<Room>>, Refusal> {
    if let Some(room) = room_of(key) {
        return Ok(room);
    }
    let (target, page_text, version) = read_page(page_id, on_database)?;
    let hash = text_hash(&page_text);
    let row = stored(key)?;
    let doc = Doc::new();
    let mut unsaved = BTreeSet::new();
    let mut seed = true;
    if let Some(row) = &row {
        let editors: BTreeSet<String> = row
            .get("editorUserIds")
            .map(to_plain_string)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .collect();
        let same_text = row.get("baseTextHash").map(to_plain_string).as_deref() == Some(hash.as_str());
        if same_text || !editors.is_empty() {
            if !same_text {
                tracing::warn!("co-edited {key}: the page changed outside co-editing while changes were open; the co-edited text is kept and its next save writes over the change, which stays in the page's history");
            }
            let state = bytes_of(row, "stateData");
            if !state.is_empty() {
                match Update::decode_v1(&state) {
                    Ok(update) => {
                        if let Err(err) = doc.transact_mut().apply_update(update) {
                            tracing::error!("co-edited {key}: its stored state was not applied, the page's text is read again: {err}");
                        } else {
                            seed = false;
                            unsaved = editors;
                        }
                    }
                    Err(err) => tracing::error!(
                        "co-edited {key}: its stored state is not a Yjs update, the page's text is read again: {err}"
                    ),
                }
            }
        }
    }
    if seed {
        markdown::seed(&doc, &page_text);
        write_row(
            key,
            &doc,
            Stored::Replace,
            Some(&hash),
            version.as_deref(),
            &unsaved,
            None,
        )?;
    }
    let room = Arc::new(Mutex::new(Room {
        doc,
        target,
        dirty: false,
        changed_at: Instant::now(),
        unsaved,
        last_editor: None,
        members: 0,
        can_edit: HashMap::new(),
    }));
    // another join may have loaded it meanwhile: keep the first
    Ok(Arc::clone(locked(rooms()).entry(key.to_string()).or_insert(room)))
}

/// How a row's stored state is taken.
#[derive(Clone, Copy, PartialEq)]
enum Stored {
    /// Merged into the document, so what another server wrote of it is kept.
    Merge,
    /// Replaced: the document was seeded again from its record's text.
    Replace,
}

/// Write the document's state and snapshot.
fn write_row(
    key: &str,
    doc: &Doc,
    stored: Stored,
    base_hash: Option<&str>,
    base_version: Option<&str>,
    unsaved: &BTreeSet<String>,
    last_editor: Option<&str>,
) -> Result<(), Error> {
    let facade = entity()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(COLLAB_DOCUMENT)?;
    tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
        let existing = facade
            .find(COLLAB_DOCUMENT)?
            .eq("documentKey", Value::string(key))
            .for_update(true)
            .one()?;
        if let Some(row) = existing.as_ref().filter(|_| stored == Stored::Merge) {
            // another server's edits: into this document too, so nothing either wrote is lost
            let state = bytes_of(row, "stateData");
            if !state.is_empty() {
                if let Ok(update) = Update::decode_v1(&state) {
                    let _ = doc.transact_mut().apply_update(update);
                }
            }
        }
        let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
        let snapshot = markdown::render(doc);
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(COLLAB_DOCUMENT)?;
                row.set("documentKey", Value::string(key));
                row.set("createdDate", now_value());
                row
            }
        };
        let creating = row.get("stateData").is_none();
        row.set("stateData", Value::Bytes(state));
        row.set("snapshotText", Value::string(snapshot));
        if let Some(hash) = base_hash {
            row.set("baseTextHash", Value::string(hash));
        }
        if let Some(version) = base_version {
            row.set("baseVersionName", Value::string(version));
        }
        row.set(
            "editorUserIds",
            if unsaved.is_empty() {
                Value::Null
            } else {
                Value::string(unsaved.iter().cloned().collect::<Vec<_>>().join(","))
            },
        );
        if let Some(editor) = last_editor {
            row.set("lastChangeUserId", Value::string(editor));
            row.set("lastChangeDate", now_value());
        }
        if creating {
            row.create(&facade)
        } else {
            row.update(&facade)
        }
    })?;
    Ok(())
}

/// An error unless the current user may view the document.
pub fn check_view(key: &str) -> Result<(), Refusal> {
    let page_id = wiki_page_id(key)?;
    let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    if access(page_id, &user_id)?.can_view {
        Ok(())
    } else {
        Err(Refusal::new(404, format!("{key} not found")))
    }
}

/// A client joins a document as `user_id`: the access check, the room, the update the client's state vector lacks.
/// Runs on a request worker in a fresh context.
pub fn join(key: &str, user_id: &str, client_state_vector: &[u8]) -> Result<Joined, Refusal> {
    let page_id = wiki_page_id(key)?;
    let Access {
        can_view,
        can_edit,
        on_database,
    } = access(page_id, user_id)?;
    if !can_view {
        return Err(Refusal::new(404, format!("{key} not found")));
    }
    let room = load(key, page_id, on_database)?;
    let mut room = locked(&room);
    room.members += 1;
    room.can_edit.insert(user_id.to_string(), can_edit);
    let vector = if client_state_vector.is_empty() {
        StateVector::default()
    } else {
        StateVector::decode_v1(client_state_vector)
            .map_err(|err| Refusal::new(400, format!("not a Yjs state vector: {err}")))?
    };
    let txn = room.doc.transact();
    let update = txn.encode_state_as_update_v1(&vector);
    let state_vector = txn.state_vector().encode_v1();
    drop(txn);
    Ok(Joined {
        can_edit,
        update,
        state_vector,
        user_name: user_name(user_id),
    })
}

fn user_name(user_id: &str) -> String {
    let row = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory()
            .entity()?
            .find("moqui.security.UserAccount")
            .ok()?
            .eq("userId", Value::string(user_id))
            .one()
            .ok()
            .flatten()
    })
    .flatten();
    row.and_then(|row| {
        row.get("userFullName")
            .or_else(|| row.get("username"))
            .map(to_plain_string)
    })
    .filter(|name| !name.is_empty())
    .unwrap_or_else(|| user_id.to_string())
}

/// An update from an editor of the document; refused when the user did not join it or may not change it.
pub fn apply(key: &str, user_id: &str, update: &[u8]) -> Result<(), Refusal> {
    let room = room_of(key).ok_or_else(|| Refusal::new(409, format!("{key} is not open here: join it first")))?;
    let mut room = locked(&room);
    match room.can_edit.get(user_id) {
        Some(true) => {}
        Some(false) => return Err(Refusal::new(403, "you may view this page and not change it")),
        None => return Err(Refusal::new(409, format!("{key} is not open here: join it first"))),
    }
    let update = Update::decode_v1(update).map_err(|err| Refusal::new(400, format!("not a Yjs update: {err}")))?;
    room.doc
        .transact_mut()
        .apply_update(update)
        .map_err(|err| Refusal::new(400, format!("the update was not applied: {err}")))?;
    room.dirty = true;
    room.changed_at = Instant::now();
    room.unsaved.insert(user_id.to_string());
    room.last_editor = Some(user_id.to_string());
    Ok(())
}

/// An update another server applied: into this server's room, when it has the document open. That server writes it.
pub fn apply_remote(key: &str, update: &[u8]) {
    let Some(room) = room_of(key) else { return };
    let room = locked(&room);
    match Update::decode_v1(update) {
        Ok(update) => {
            if let Err(err) = room.doc.transact_mut().apply_update(update) {
                tracing::warn!("co-edited {key}: an update from another server was not applied: {err}");
            }
        }
        Err(err) => tracing::warn!("co-edited {key}: an update from another server is not a Yjs update: {err}"),
    }
}

/// A client left the document. The room is written, saved and closed by the next [`tick`] once no one is in it.
pub fn leave(key: &str) {
    if let Some(room) = room_of(key) {
        let mut room = locked(&room);
        room.members = room.members.saturating_sub(1);
    }
}

/// The markdown of an open document, for a test or a preview.
pub fn snapshot(key: &str) -> Option<String> {
    room_of(key).map(|room| markdown::render(&locked(&room).doc))
}

/// The keys of the documents open on this server.
pub fn open_documents() -> Vec<String> {
    locked(rooms()).keys().cloned().collect()
}

/// Write the rooms' state a moment after their edits, save a version of a document on database files that has been
/// left a while, and save and close the rooms no one is in. Each room stands alone: a failure is logged and the others
/// go on. Runs on a request worker in a fresh context every second or two.
pub fn tick() {
    let keys: Vec<(String, Arc<Mutex<Room>>)> = locked(rooms())
        .iter()
        .map(|(key, room)| (key.clone(), Arc::clone(room)))
        .collect();
    for (key, room) in keys {
        if let Err(err) = tick_room(&key, &room) {
            tracing::error!("co-edited {key}: {}", err.message());
        }
    }
}

fn tick_room(key: &str, room: &Arc<Mutex<Room>>) -> Result<(), Error> {
    let (dirty, quiet, members, unsaved, on_database, last_editor) = {
        let room = locked(room);
        (
            room.dirty,
            room.changed_at.elapsed(),
            room.members,
            !room.unsaved.is_empty(),
            room.target.on_database,
            room.last_editor.clone(),
        )
    };
    if dirty && quiet >= FLUSH_AFTER {
        flush(key, room)?;
    }
    let idle_save = unsaved && on_database && quiet >= IDLE_SAVE;
    if members == 0 || idle_save {
        if unsaved {
            let editor = last_editor
                .or_else(|| locked(room).unsaved.iter().next().cloned())
                .unwrap_or_default();
            save_room(key, room, &editor, None)?;
        } else if dirty {
            flush(key, room)?;
        }
        if members == 0 {
            let still_empty = locked(room).members == 0;
            if still_empty {
                locked(rooms()).remove(key);
            }
        }
    }
    Ok(())
}

/// Write the room's state now.
fn flush(key: &str, room: &Arc<Mutex<Room>>) -> Result<(), Error> {
    let guard = locked(room);
    let unsaved = guard.unsaved.clone();
    let editor = guard.last_editor.clone();
    write_row(key, &guard.doc, Stored::Merge, None, None, &unsaved, editor.as_deref())?;
    drop(guard);
    locked(room).dirty = false;
    Ok(())
}

/// Save the document as a version of its record, as `user_id` (an editor with update access), with `note` or one that
/// names its editors. Out: the version made, none when the text was unchanged.
fn save_room(key: &str, room: &Arc<Mutex<Room>>, user_id: &str, note: Option<&str>) -> Result<Option<String>, Error> {
    let (target, page_text, editors) = {
        let room = locked(room);
        (room.target.clone(), markdown::render(&room.doc), room.unsaved.clone())
    };
    let note = note
        .map(str::to_string)
        .filter(|note| !note.trim().is_empty())
        .unwrap_or_else(|| {
            let names: Vec<String> = editors.iter().map(|id| user_name(id)).collect();
            match names.len() {
                0 | 1 => "Edited".to_string(),
                _ => format!("Edited together by {}", names.join(", ")),
            }
        });
    // the version is the user's: their name on it and their access checked, in their own login
    let username = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory()
            .entity()?
            .find("moqui.security.UserAccount")
            .ok()?
            .eq("userId", Value::string(user_id))
            .one()
            .ok()
            .flatten()
    })
    .flatten()
    .and_then(|row| row.get("username").map(to_plain_string))
    .ok_or_else(|| Error::new(format!("no user {user_id} to save {key} as")))?;
    let current = crate::user::current_user_id().ok().flatten();
    let switched = current.as_deref() != Some(user_id);
    let identity = crate::user::session::identity();
    if switched {
        crate::user::internal_login_user(&username)?;
    }
    let parameters: BTreeMap<String, Value> = [
        ("wikiSpaceId", Value::string(&target.wiki_space_id)),
        ("wikiPageId", Value::string(&target.wiki_page_id)),
        ("pagePath", Value::string(&target.page_path)),
        ("pageName", Value::string(&target.page_name)),
        ("parentPath", target.page_path.rsplit_once('/').map_or(Value::Null, |(parent, _)| Value::string(parent))),
        ("pageText", Value::string(&page_text)),
        ("versionNote", Value::string(&note)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    let result = sync_checked(
        "org.moqui.impl.WikiServices.update#WikiPage",
        &parameters,
        &CallOptions::default(),
    );
    ExecutionContext::with_current(|ec| ec.message().clear_errors());
    if switched {
        crate::user::session::restore_identity(&identity);
    }
    let out = result?;
    let version_name = Some(text(&out, "versionName")).filter(|name| !name.is_empty());
    {
        let mut room = locked(room);
        room.unsaved.clear();
        room.dirty = false;
        let guard = &*room;
        write_row(
            key,
            &guard.doc,
            Stored::Merge,
            Some(&text_hash(&page_text)),
            version_name.as_deref(),
            &BTreeSet::new(),
            None,
        )?;
    }
    {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
        let facade = entity()?;
        facade
            .find(COLLAB_DOCUMENT)?
            .eq("documentKey", Value::string(key))
            .update_all([("lastSaveDate".to_string(), now_value())])?;
    }
    locked(events()).push(Event::Saved {
        document_key: key.to_string(),
        version_name: version_name.clone(),
        user_id: user_id.to_string(),
        user_name: user_name(user_id),
    });
    Ok(version_name)
}

/// Save a document as a version now, as the current user, who needs update access (`CollabServices.save#CollabDocument`,
/// the editor's Save). The document need not be open on this server: its stored state is saved. Out: the version made,
/// none when nothing changed.
pub fn save(key: &str, note: Option<&str>) -> Result<Option<String>, Refusal> {
    let page_id = wiki_page_id(key)?;
    let user_id = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Refusal::new(401, "log in to save"))?;
    let access = access(page_id, &user_id)?;
    if !access.can_edit {
        return Err(Refusal::new(403, "you may not change this page"));
    }
    let room = match room_of(key) {
        Some(room) => {
            flush(key, &room)?;
            room
        }
        None => load(key, page_id, access.on_database)?,
    };
    // the saver's own changes count even when they came through another server
    locked(&room).unsaved.insert(user_id.clone());
    let saved = save_room(key, &room, &user_id, note)?;
    if locked(&room).members == 0 {
        locked(rooms()).remove(key);
    }
    Ok(saved)
}

#[cfg(test)]
mod tests;
