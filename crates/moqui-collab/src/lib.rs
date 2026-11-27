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
//! carries the Yjs sync messages on the notification WebSocket (moqui-host `notws/collab.rs`); this crate keeps the rooms:
//! [`join`] (the access check, the document loaded or seeded from the record's text, the diff the client lacks),
//! [`apply`] (an editor's update), [`tick`] (the state written a second or two after an edit, a version of the record
//! after a while without edits on database files, and when the last editor leaves) and [`save`] (a version now, with a
//! note). Guide: `docs/data/Revision-Management.md`, Co-editing.
//!
//! A document key names its record: `WikiPage:<wikiPageId>` is a wiki page, whose markdown the document is
//! ([`markdown`]); the user needs view access to join and update access to change it (`WikiServices.get#WikiPageAccess`).
//! The page's text is written only as a version (`WikiServices.update#WikiPage`), never on each keystroke, so its history
//! is the saves; the document between saves is its CollabDocument row. A page changed outside co-editing while no one had
//! changes open is read again when the next editor joins; with changes open, the co-edited text is kept (logged) and
//! the next save writes over it, the other change staying in the page's history.
//!
//! In a cluster an update is relayed once to the other servers that have the document open (`collab-update`). One that
//! is lost there is recovered from the row (plans/archive/High-Level-Review.md HL-018): each [`tick`] reads the stamps of the
//! open documents' rows in one query, merges the state of each row another server wrote since this one last read or
//! wrote it, and gives the part its room lacked to the room's clients ([`Event::Update`]); a version is saved from the
//! document with the stored state merged first. Yjs updates are idempotent, so merging what a room already has changes
//! nothing.
//!
//! A key `DevFile:<rootId>:<path>` is a file of a Dev AIDE root co-edited in shared mode ([`text`]): its own rooms,
//! seeded from the file and kept in memory only, joined and changed through the same functions.
//!
//! This crate is on moqui-core (plans/archive/Crate-Split.md), which reaches it through the [`moqui_core::extension`]
//! entry below: `org.moqui.resource.CollabServices` ([`service`]). moqui-host links it in every build and calls the rooms
//! from its WebSocket; the browser build leaves it out, and its services report that `collab` is not built in.

mod board;
pub mod markdown;
mod meeting;
mod nesting;
pub use meeting::Meeting;
pub mod service;
pub mod text;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["collab"],
        register_services: service::register,
        ..moqui_core::extension::Extension::EMPTY
    }
}

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::lock;
use sha2::Digest;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::Condition;
use moqui_core::service::call::{sync_checked, CallOptions};

const COLLAB_DOCUMENT: &str = "moqui.resource.CollabDocument";
/// The state is written this long after the last edit.
const FLUSH_AFTER: Duration = Duration::from_millis(1500);
/// A document on database files is saved as a version after this long without an edit.
const IDLE_SAVE: Duration = Duration::from_secs(120);
/// A member's access to the page is asked again by the [`tick`] once its answer is this old (SEC-TLS-112).
const ACCESS_RECHECK: Duration = Duration::from_secs(60);
/// An answer this old is not used for an update: the tick did not ask again.
const ACCESS_STALE: Duration = Duration::from_secs(180);
/// A meeting member's grant is asked again this often; a change to the call asks it at the next tick
/// ([`recheck_meeting`]).
const MEETING_RECHECK: Duration = Duration::from_secs(10);

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
    /// A co-edited file's room ([`text`]): its seed's id, new each time the room is seeded, by which a client that
    /// joins again knows a room seeded again since (and starts a new document rather than merge an old copy).
    pub seed: Option<String>,
    /// A co-edited file's room: the hash of the file it was seeded from, which a joining tab compares with its own.
    pub base_hash: Option<String>,
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
    /// Changes another server stored that this server's room lacked, now merged into it: the Yjs update (v1) for the
    /// room's clients.
    Update { document_key: String, update: Vec<u8> },
    /// The user may no longer view the page: the host drops that user's clients from the document (SEC-TLS-112).
    Revoked { document_key: String, user_id: String },
    /// The user's right to change the page changed while they may still view it: the host tells their clients.
    CanEdit {
        document_key: String,
        user_id: String,
        can_edit: bool,
    },
}

/// The record a document key names.
#[derive(Debug, Clone)]
struct WikiTarget {
    codec: Codec,
    wiki_space_id: String,
    wiki_page_id: String,
    page_path: String,
    page_name: String,
    /// The space keeps its pages on database files (each save a version row), not in git.
    on_database: bool,
}

#[derive(Debug, Clone, Copy)]
enum Codec {
    Markdown,
    Board,
}

impl Codec {
    fn seed(self, doc: &Doc, text: &str) -> Result<(), Refusal> {
        match self {
            Self::Markdown => {
                markdown::seed(doc, text);
                Ok(())
            }
            Self::Board => board::seed(doc, text).map_err(|e| Refusal::new(422, e)),
        }
    }

    fn render(self, doc: &Doc) -> String {
        match self {
            Self::Markdown => markdown::render(doc),
            Self::Board => board::render(doc),
        }
    }

    fn search_text(self, doc: &Doc) -> String {
        match self {
            Self::Markdown => markdown::render(doc),
            Self::Board => board::search_text(doc),
        }
    }

    /// What its documents may hold (SEC-TLS-118): a page, its fragment of XML elements and texts; a board, its three
    /// maps of plain values.
    fn shape(self) -> nesting::Shape {
        match self {
            Self::Markdown => nesting::Shape {
                roots: &[markdown::FRAGMENT],
                nested: nesting::Nested::Xml,
            },
            Self::Board => nesting::Shape {
                roots: &["elements", "files", "app"],
                nested: nesting::Nested::None,
            },
        }
    }
}

struct Room {
    saving: Arc<Mutex<()>>,
    doc: Doc,
    board_candidate: Option<Doc>,
    /// The depth of the document's shared types, which an editor's update may not take past its bound (SEC-TLS-118).
    nesting: nesting::Nesting,
    target: WikiTarget,
    /// Changed since the state was last written.
    dirty: bool,
    changed_at: Instant,
    /// The users whose changes are in no version yet, and the last of them.
    unsaved: BTreeSet<String>,
    meeting_names: BTreeSet<String>,
    last_editor: Option<String>,
    members: usize,
    /// Taken out of [`rooms`]: a join that found it before then loads the room again.
    closed: bool,
    /// When a save of the room last failed for every editor it could be saved as; the idle save waits [`IDLE_SAVE`]
    /// after it, rather than trying again every second.
    save_failed_at: Option<Instant>,
    /// Each member's access, by user, as last asked.
    can_edit: HashMap<String, Grant>,
    /// The size of the document's state as last written, and of the updates applied since: what [`apply`] holds to
    /// [`max_state_bytes`].
    state_bytes: usize,
    /// The row's `lastUpdatedStamp` (milliseconds) as this server last read or wrote it; a row with another was written
    /// by another server since, and [`tick`] merges it. `None` when not known, which only costs a read.
    stamp: Option<i64>,
}

/// Whether a member may change the document, when that was last answered, when to ask again after a check that
/// failed, and how many clients of the user are in the room.
#[derive(Clone)]
struct Grant {
    meeting: Option<Meeting>,
    writer: String,
    name: String,
    can_edit: bool,
    checked: Instant,
    retry_at: Instant,
    failures: u32,
    clients: usize,
    /// Asked again at the next tick, whatever the answer's age ([`recheck_meeting`]).
    nudged: bool,
}

fn rooms() -> &'static Mutex<HashMap<String, Arc<Mutex<Room>>>> {
    static ROOMS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Room>>>>> = OnceLock::new();
    ROOMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn events() -> &'static Mutex<Vec<Event>> {
    static EVENTS: OnceLock<Mutex<Vec<Event>>> = OnceLock::new();
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn room_of(key: &str) -> Option<Arc<Mutex<Room>>> {
    lock(rooms()).get(key).cloned()
}

/// The events since the last call, for the host to send.
pub fn take_events() -> Vec<Event> {
    std::mem::take(&mut *lock(events()))
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

/// What [`access`] answers in a test in place of the wiki's: `Some` answers that, `None` asks the wiki.
#[cfg(test)]
static ACCESS_FOR_TEST: Mutex<Option<Result<(bool, bool), u16>>> = Mutex::new(None);

fn access(page_id: &str, user_id: &str) -> Result<Access, Refusal> {
    #[cfg(test)]
    if let Some(answer) = *lock(&ACCESS_FOR_TEST) {
        return match answer {
            Ok((can_view, can_edit)) => Ok(Access {
                can_view,
                can_edit,
                on_database: true,
            }),
            Err(code) => Err(Refusal::new(code, "the access check failed")),
        };
    }
    let out = call_internal(
        "org.moqui.resource.WikiServices.get#WikiPageAccess",
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
        "org.moqui.resource.WikiServices.get#WikiPageInfoById",
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
    let target = WikiTarget {
        codec: if location.ends_with(".excalidraw") {
            Codec::Board
        } else {
            Codec::Markdown
        },
        wiki_space_id,
        wiki_page_id: page_id.to_string(),
        page_path,
        page_name,
        on_database,
    };
    Ok((target, page_text, version))
}

fn entity() -> Result<moqui_core::entity::EntityFacade, Error> {
    ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade"))
}

fn stored(key: &str) -> Result<Option<moqui_core::entity::EntityValue>, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    Ok(entity()?
        .find(COLLAB_DOCUMENT)?
        .eq("documentKey", Value::string(key))
        .one()?)
}

fn bytes_of(row: &moqui_core::entity::EntityValue, field: &str) -> Vec<u8> {
    match row.get(field) {
        Some(Value::Bytes(bytes)) => bytes.clone(),
        _ => Vec::new(),
    }
}

fn now_value() -> Value {
    moqui_core::user::now_value()
}

/// A row's `lastUpdatedStamp` in milliseconds.
fn stamp_of(row: &moqui_core::entity::EntityValue) -> Option<i64> {
    match row.get("lastUpdatedStamp") {
        Some(Value::Timestamp(stamp)) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

/// Merge a stored state into `doc`. Out: the update of what `doc` lacked, for its clients; `None` when it had it all
/// or the state is not one.
fn merge_into(key: &str, doc: &Doc, state: &[u8]) -> Option<Vec<u8>> {
    if state.is_empty() {
        return None;
    }
    let update = match Update::decode_v1(state) {
        Ok(update) => update,
        Err(err) => {
            tracing::warn!("co-edited {key}: its stored state is not a Yjs update and was not merged: {err}");
            return None;
        }
    };
    let before = doc.transact().state_vector();
    if let Err(err) = doc.transact_mut().apply_update(update) {
        tracing::warn!("co-edited {key}: its stored state was not merged: {err}");
        return None;
    }
    let txn = doc.transact();
    (txn.state_vector() != before).then(|| txn.encode_state_as_update_v1(&before))
}

/// What another server stored: merged into the room, and what the room lacked queued for its clients.
fn merge_stored(key: &str, room: &mut Room, row: &moqui_core::entity::EntityValue) {
    room.board_candidate = None;
    if let Some(update) = merge_into(key, &room.doc, &bytes_of(row, "stateData")) {
        lock(events()).push(Event::Update {
            document_key: key.to_string(),
            update,
        });
    }
    room.stamp = stamp_of(row);
}

/// Rows read in one query by [`refresh`].
const REFRESH_CHUNK: usize = 500;

/// Merge into the open rooms what other servers stored since this one last read or wrote their rows: the stamps of
/// all of them in one query, then the state of those another server wrote (HL-018). A chunk that cannot be read is
/// logged and the others go on.
fn refresh(open: &[(String, Arc<Mutex<Room>>)]) {
    let facade = match entity() {
        Ok(facade) => facade,
        Err(err) => return tracing::error!("co-editing: the stored documents were not read: {}", err.message()),
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    for chunk in open.chunks(REFRESH_CHUNK) {
        if let Err(err) = refresh_chunk(&facade, chunk) {
            tracing::error!("co-editing: the stored documents were not read: {}", err.message());
        }
    }
}

fn refresh_chunk(facade: &moqui_core::entity::EntityFacade, open: &[(String, Arc<Mutex<Room>>)]) -> Result<(), Error> {
    let by_key: HashMap<&str, &Arc<Mutex<Room>>> = open.iter().map(|(key, room)| (key.as_str(), room)).collect();
    let keys = open.iter().map(|(key, _)| Value::string(key)).collect();
    let stamps = facade
        .find(COLLAB_DOCUMENT)?
        .condition(Condition::is_in("documentKey", keys))
        .select_field("documentKey,lastUpdatedStamp")
        .list()?;
    let present: BTreeSet<String> = stamps
        .iter()
        .filter_map(|row| row.get("documentKey").map(to_plain_string))
        .collect();
    for (key, room) in open.iter().filter(|(key, _)| !present.contains(key)) {
        // The page was deleted on this node or elsewhere. A new join may already
        // have loaded a replacement, so retire only the room in this snapshot.
        let mut all = lock(rooms());
        if all.get(key).is_some_and(|current| Arc::ptr_eq(current, room)) {
            all.remove(key);
            drop(all);
            for user_id in lock(room).can_edit.keys() {
                lock(events()).push(Event::Revoked {
                    document_key: key.clone(),
                    user_id: user_id.clone(),
                });
            }
        }
    }
    let changed: Vec<Value> = stamps
        .iter()
        .filter(|row| {
            let key = row.get("documentKey").map(to_plain_string).unwrap_or_default();
            by_key
                .get(key.as_str())
                .is_some_and(|room| lock(room).stamp != stamp_of(row))
        })
        .filter_map(|row| row.get("documentKey").cloned())
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let rows = facade
        .find(COLLAB_DOCUMENT)?
        .condition(Condition::is_in("documentKey", changed))
        .select_field("documentKey,stateData,lastUpdatedStamp")
        .list()?;
    for row in rows.iter() {
        let key = row.get("documentKey").map(to_plain_string).unwrap_or_default();
        if let Some(room) = by_key.get(key.as_str()) {
            merge_stored(&key, &mut lock(room), row);
        }
    }
    Ok(())
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
    let mut meeting_names = BTreeSet::new();
    let mut seed = true;
    let mut stamp = row.as_ref().and_then(stamp_of);
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
                            meeting_names = row
                                .get("meetingEditorNames")
                                .and_then(|v| serde_json::from_str(&to_plain_string(v)).ok())
                                .unwrap_or_default();
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
        target.codec.seed(&doc, &page_text)?;
        let written = write_row(
            key,
            (target.codec, &doc),
            if row.is_none() { Stored::Insert } else { Stored::Replace },
            Some(&hash),
            version.as_deref(),
            (&unsaved, &meeting_names),
            None,
        )?;
        if written.adopt_existing {
            // Another node seeded the document while this join read its wiki page.
            // Its Yjs item ids are already authoritative for other editors.
            return load(key, page_id, on_database);
        }
        stamp = written.stamp;
    }
    let state_bytes = doc.transact().encode_state_as_update_v1(&StateVector::default()).len();
    let room = Arc::new(Mutex::new(Room {
        saving: Arc::default(),
        doc,
        board_candidate: None,
        nesting: nesting::Nesting::default(),
        target,
        dirty: false,
        changed_at: Instant::now(),
        unsaved,
        meeting_names,
        last_editor: None,
        members: 0,
        closed: false,
        save_failed_at: None,
        can_edit: HashMap::new(),
        state_bytes,
        stamp,
    }));
    // another join may have loaded it meanwhile: keep the first
    Ok(Arc::clone(lock(rooms()).entry(key.to_string()).or_insert(room)))
}

/// How a row's stored state is taken.
#[derive(Clone, Copy, PartialEq)]
enum Stored {
    /// Seed only if another server has not created the document in the meantime.
    Insert,
    /// Merged into the document, so what another server wrote of it is kept.
    Merge,
    /// Merged after a version save, recording its timestamp in the checkpoint transaction.
    Save,
    /// Replaced: the document was seeded again from its record's text.
    Replace,
}

/// What [`write_row`] did: the stamp it wrote, and the update of what another server had stored that the document
/// lacked (merged into it, for its clients).
struct Written {
    adopt_existing: bool,
    stamp: Option<i64>,
    merged: Option<Vec<u8>>,
    /// The size of the state written.
    state_bytes: usize,
}

/// Write the document's state and snapshot.
fn write_row(
    key: &str,
    (codec, doc): (Codec, &Doc),
    stored: Stored,
    base_hash: Option<&str>,
    base_version: Option<&str>,
    (unsaved, meeting_names): (&BTreeSet<String>, &BTreeSet<String>),
    last_editor: Option<&str>,
) -> Result<Written, Error> {
    let facade = entity()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(COLLAB_DOCUMENT)?;
    tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
        let page_id = key.strip_prefix("WikiPage:").unwrap_or_default();
        if facade
            .find("moqui.resource.wiki.WikiPage")?
            .eq("wikiPageId", Value::string(page_id))
            .for_update(true)
            .one()?
            .is_none()
        {
            return Err(moqui_lib::error::Error::new("the co-edited page was removed"));
        }
        let existing = facade
            .find(COLLAB_DOCUMENT)?
            .eq("documentKey", Value::string(key))
            .for_update(true)
            .one()?;
        if stored == Stored::Insert && existing.is_some() {
            return Ok(Written {
                adopt_existing: true,
                stamp: None,
                merged: None,
                state_bytes: 0,
            });
        }
        let merge = matches!(stored, Stored::Merge | Stored::Save);
        if merge && existing.is_none() {
            return Err(moqui_lib::error::Error::new("the co-edited document was removed"));
        }
        let mut merged = None;
        if let Some(row) = existing.as_ref().filter(|_| merge) {
            // another server's edits: into this document too, so nothing either wrote is lost
            merged = merge_into(key, doc, &bytes_of(row, "stateData"));
        }
        // the stamp the write gets: when this transaction began, as the entity facade sets it
        let stamp = facade
            .transactions_for(COLLAB_DOCUMENT)?
            .start_time_millis()
            .filter(|millis| *millis > 0);
        let state = doc.transact().encode_state_as_update_v1(&StateVector::default());
        let state_bytes = state.len();
        let snapshot = codec.search_text(doc);
        // a compacted row has no state but is there (compact_row)
        let creating = existing.is_none();
        let mut row = match existing {
            Some(row) => row,
            None => {
                let mut row = facade.make_value(COLLAB_DOCUMENT)?;
                row.set("documentKey", Value::string(key));
                row.set("createdDate", now_value());
                row
            }
        };
        row.set("stateData", Value::Bytes(state));
        row.set("snapshotText", Value::string(snapshot));
        if stored == Stored::Save {
            row.set("lastSaveDate", now_value());
        }
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
        row.set(
            "meetingEditorNames",
            if meeting_names.is_empty() {
                Value::Null
            } else {
                Value::string(serde_json::json!(meeting_names).to_string())
            },
        );
        if let Some(editor) = last_editor {
            row.set("lastChangeUserId", Value::string(editor));
            row.set("lastChangeDate", now_value());
        }
        if creating {
            row.create(&facade)?;
        } else {
            row.update(&facade)?;
        }
        Ok(Written {
            adopt_existing: false,
            stamp,
            merged,
            state_bytes,
        })
    })
    .map_err(Error::from)
}

/// Note what [`write_row`] did in the room: its stamp, and the merged update for the room's clients.
fn wrote(key: &str, room: &mut Room, written: Written) {
    room.stamp = written.stamp;
    room.state_bytes = written.state_bytes;
    room.board_candidate = None;
    if let Some(update) = written.merged {
        lock(events()).push(Event::Update {
            document_key: key.to_string(),
            update,
        });
    }
}

/// An error unless the current user may view the document.
pub fn check_view(key: &str) -> Result<(), Refusal> {
    if text::is_key(key) {
        return Err(Refusal::new(
            404,
            format!("{key} is a co-edited file: its editors join it"),
        ));
    }
    let page_id = wiki_page_id(key)?;
    let user_id = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    if access(page_id, &user_id)?.can_view {
        Ok(())
    } else {
        Err(Refusal::new(404, format!("{key} not found")))
    }
}

/// A client joins a document as `user_id`: the access check, the room, the update the client's state vector lacks.
/// Runs on a request worker in a fresh context.
pub fn join(key: &str, user_id: &str, client_state_vector: &[u8]) -> Result<Joined, Refusal> {
    join_scoped(key, user_id, client_state_vector, None)
}

pub fn join_scoped(
    key: &str,
    user_id: &str,
    client_state_vector: &[u8],
    meeting: Option<Meeting>,
) -> Result<Joined, Refusal> {
    if text::is_key(key) {
        // a file is co-edited by its writers, never from a meeting
        if meeting.is_some() {
            return Err(Refusal::new(404, "board not found"));
        }
        return text::join(key, user_id, client_state_vector);
    }
    let page_id = wiki_page_id(key)?;
    let key_of_actor = meeting.as_ref().map_or_else(|| user_id.to_string(), Meeting::key);
    let (
        Access {
            can_view,
            can_edit,
            on_database,
        },
        writer,
        name,
    ) = if let Some(scope) = &meeting {
        scope.grant(page_id)?
    } else {
        (access(page_id, user_id)?, user_id.to_string(), user_name(user_id))
    };
    if !can_view {
        return Err(Refusal::new(404, format!("{key} not found")));
    }
    // the client's state vector first: a refused join is no member, which no leave would ever take away (SEC-TLS-111)
    let vector = if client_state_vector.is_empty() {
        StateVector::default()
    } else {
        StateVector::decode_v1(client_state_vector)
            .map_err(|err| Refusal::new(400, format!("not a Yjs state vector: {err}")))?
    };
    // a room the tick closed between the load and the lock is loaded again, or the member would be in no room
    let room = loop {
        let room = load(key, page_id, on_database)?;
        if !lock(&room).closed {
            break room;
        }
    };
    let mut room = lock(&room);
    if meeting.is_some() && !matches!(room.target.codec, Codec::Board) {
        return Err(Refusal::new(404, "board not found"));
    }
    room.members += 1;
    let now = Instant::now();
    let clients = room.can_edit.get(&key_of_actor).map_or(0, |grant| grant.clients) + 1;
    room.can_edit.insert(
        key_of_actor,
        Grant {
            meeting,
            writer,
            name: name.clone(),
            can_edit,
            checked: now,
            retry_at: now,
            failures: 0,
            clients,
            nudged: false,
        },
    );
    let txn = room.doc.transact();
    let update = txn.encode_state_as_update_v1(&vector);
    let state_vector = txn.state_vector().encode_v1();
    drop(txn);
    Ok(Joined {
        can_edit,
        update,
        state_vector,
        user_name: name,
        seed: None,
        base_hash: None,
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
    if text::is_key(key) {
        return text::apply(key, user_id, update);
    }
    let room = room_of(key).ok_or_else(|| Refusal::new(409, format!("{key} is not open here: join it first")))?;
    let mut room = lock(&room);
    match room.can_edit.get(user_id) {
        Some(grant) if grant.checked.elapsed() > ACCESS_STALE => {
            return Err(Refusal::new(403, "your access to this page could not be checked again"))
        }
        Some(grant) if grant.can_edit => {}
        Some(_) => return Err(Refusal::new(403, "you may view this page and not change it")),
        None => return Err(Refusal::new(409, format!("{key} is not open here: join it first"))),
    }
    // the whole state is held in memory and written again at each flush: bounded (SEC-TLS-113)
    let max = match room.target.codec {
        Codec::Markdown => max_state_bytes(),
        Codec::Board => {
            if update.len() > board::limit("board@max-update-bytes", 1 << 20) {
                return Err(Refusal::new(413, "board update exceeds the size limit"));
            }
            board::limit("board@max-document-bytes", 20 << 20)
        }
    };
    if room.state_bytes.saturating_add(update.len()) > max {
        return Err(Refusal::new(413, format!("the document is larger than {max} bytes")));
    }
    // its roots and nested types, before anything of it is applied, here or to the board's candidate (SEC-TLS-118)
    let shape = room.target.codec.shape();
    {
        let Room { doc, nesting, .. } = &mut *room;
        nesting
            .check(doc, shape, update)
            .map_err(|(status, why)| Refusal::new(status, why))?;
    }
    if matches!(room.target.codec, Codec::Board) {
        let Room {
            doc, board_candidate, ..
        } = &mut *room;
        board::validate_update(doc, board_candidate, update)?;
    }
    let size = update.len();
    let update = Update::decode_v1(update).map_err(|err| Refusal::new(400, format!("not a Yjs update: {err}")))?;
    room.doc
        .transact_mut()
        .apply_update(update)
        .map_err(|err| Refusal::new(400, format!("the update was not applied: {err}")))?;
    room.state_bytes = room.state_bytes.saturating_add(size);
    room.dirty = true;
    room.changed_at = Instant::now();
    let grant = &room.can_edit[user_id];
    let writer = grant.writer.clone();
    if grant.meeting.is_some() {
        let name = grant.name.clone();
        room.meeting_names.insert(name);
    }
    room.unsaved.insert(writer.clone());
    room.last_editor = Some(writer);
    Ok(())
}

/// The most bytes a co-edited document's state may take: `resource-facade@collab-max-state-bytes`, 8 MiB by default.
fn max_state_bytes() -> usize {
    moqui_core::context::conf_env::setting("resource-facade@collab-max-state-bytes")
        .parse::<usize>()
        .ok()
        .filter(|max| *max > 0)
        .unwrap_or(8 * 1024 * 1024)
}

/// An update another server applied: into this server's room, when it has the document open. That server writes it.
pub fn apply_remote(key: &str, update: &[u8]) {
    if text::is_key(key) {
        return text::apply_remote(key, update);
    }
    let Some(room) = room_of(key) else { return };
    let mut room = lock(&room);
    room.board_candidate = None;
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
    leave_as(key, None);
}

/// A client of `user_id` left the document: the room counts one member fewer, and the user's access answer goes with
/// their last client (SEC-TLS-112).
pub fn leave_as(key: &str, user_id: Option<&str>) {
    if text::is_key(key) {
        return text::leave(key, user_id);
    }
    if let (Some(room), Some(user_id)) = (room_of(key), user_id) {
        let mut room = lock(&room);
        let last = room.can_edit.get_mut(user_id).is_some_and(|grant| {
            grant.clients = grant.clients.saturating_sub(1);
            grant.clients == 0
        });
        if last {
            room.can_edit.remove(user_id);
        }
    }
    leave_count(key);
}

fn leave_count(key: &str) {
    if let Some(room) = room_of(key) {
        let mut room = lock(&room);
        room.members = room.members.saturating_sub(1);
    }
}

/// Searchable text of an open document: Markdown, or board text in reading order.
pub fn snapshot(key: &str) -> Option<String> {
    if text::is_key(key) {
        return text::snapshot(key);
    }
    room_of(key).map(|room| {
        let room = lock(&room);
        room.target.codec.search_text(&room.doc)
    })
}

/// The keys of the documents open on this server.
pub fn open_documents() -> Vec<String> {
    let mut keys: Vec<String> = lock(rooms()).keys().cloned().collect();
    keys.extend(text::open_keys());
    keys
}

/// Write the rooms' state a moment after their edits, save a version of a document on database files that has been
/// left a while, and save and close the rooms no one is in. Each room stands alone: a failure is logged and the others
/// go on. Runs on a request worker in a fresh context every second or two.
pub fn tick() {
    let mut revoked = Vec::new();
    text::tick(&mut revoked);
    lock(events()).extend(revoked);
    let keys: Vec<(String, Arc<Mutex<Room>>)> = lock(rooms())
        .iter()
        .map(|(key, room)| (key.clone(), Arc::clone(room)))
        .collect();
    if keys.is_empty() {
        return;
    }
    refresh(&keys);
    for (key, room) in keys {
        if !room_of(&key).is_some_and(|current| Arc::ptr_eq(&current, &room)) {
            continue;
        }
        recheck_access(&key, &room);
        if let Err(err) = tick_room(&key, &room) {
            tracing::error!("co-edited {key}: {}", err.message());
        }
    }
}

/// Ask again the access of each member whose answer is [`ACCESS_RECHECK`] old: a user whose right to change the page
/// was taken away has their updates refused from then on, and one who may no longer view it is dropped from the
/// document ([`Event::Revoked`]). A check that fails leaves the answer to go stale, when [`apply`] refuses it
/// (SEC-TLS-112).
fn recheck_access(key: &str, room: &Mutex<Room>) {
    let (page_id, due): (String, Vec<_>) = {
        let room = lock(room);
        let now = Instant::now();
        let due = room
            .can_edit
            .iter()
            .filter(|(_, grant)| {
                let every = if grant.meeting.is_some() {
                    MEETING_RECHECK
                } else {
                    ACCESS_RECHECK
                };
                (grant.nudged || grant.checked.elapsed() >= every) && grant.retry_at <= now
            })
            .map(|(user, grant)| (user.clone(), grant.meeting.clone()))
            .collect();
        (room.target.wiki_page_id.clone(), due)
    };
    for (user_id, scope) in due {
        let revoke = |user_id: String| {
            lock(room).can_edit.remove(&user_id);
            lock(events()).push(Event::Revoked {
                document_key: key.to_string(),
                user_id,
            });
        };
        let found = if let Some(scope) = &scope {
            scope.grant(&page_id).map(|(access, _, _)| access)
        } else {
            access(&page_id, &user_id)
        };
        match found {
            Ok(found) if found.can_view => {
                let mut room = lock(room);
                let Some(grant) = room.can_edit.get_mut(&user_id) else {
                    continue;
                };
                let changed = grant.can_edit != found.can_edit;
                grant.can_edit = found.can_edit;
                grant.checked = Instant::now();
                grant.retry_at = grant.checked;
                grant.failures = 0;
                grant.nudged = false;
                drop(room);
                if changed {
                    lock(events()).push(Event::CanEdit {
                        document_key: key.to_string(),
                        user_id,
                        can_edit: found.can_edit,
                    });
                }
            }
            Ok(_) => revoke(user_id),
            Err(refusal) => {
                tracing::warn!(
                    "co-edited {key}: the access of {user_id} was not checked again: {}",
                    refusal.message
                );
                // an answer that could not be renewed past ACCESS_STALE takes the member out; until then the check is
                // tried again later each time (5 s, 10 s, ... at most a minute)
                let stale = lock(room)
                    .can_edit
                    .get(&user_id)
                    .is_some_and(|grant| grant.checked.elapsed() > ACCESS_STALE);
                if stale || scope.is_some() {
                    revoke(user_id);
                } else if let Some(grant) = lock(room).can_edit.get_mut(&user_id) {
                    grant.failures = grant.failures.saturating_add(1);
                    let wait = Duration::from_secs(5u64.saturating_mul(1 << grant.failures.min(4)).min(60));
                    grant.retry_at = Instant::now() + wait;
                }
            }
        }
    }
}

/// Whether the meeting member `actor_key` ([`Meeting::key`]) of the open document `key` may see its frames, or with
/// `editing` change it: answered from the room's grant, which the [`tick`] renews every [`MEETING_RECHECK`] and at
/// once after [`recheck_meeting`], so the socket asks no service per frame. A grant past [`ACCESS_STALE`] allows
/// nothing.
pub fn meeting_allows(key: &str, actor_key: &str, editing: bool) -> bool {
    let Some(room) = room_of(key) else { return false };
    let room = lock(&room);
    room.can_edit.get(actor_key).is_some_and(|grant| {
        grant.meeting.is_some() && grant.checked.elapsed() <= ACCESS_STALE && (!editing || grant.can_edit)
    })
}

/// The call `call_id` changed (a leg left, a board closed, a guest was removed): its members' grants on this server
/// are asked again at the next [`tick`], which drops or tells those that changed. The host calls it for each
/// `collab-recheck` message (`CollabServices.recheck#MeetingBoards`).
pub fn recheck_meeting(call_id: &str) {
    let open: Vec<Arc<Mutex<Room>>> = lock(rooms()).values().cloned().collect();
    let now = Instant::now();
    for room in open {
        for grant in lock(&room).can_edit.values_mut() {
            if grant.meeting.as_ref().is_some_and(|meeting| meeting.call_id == call_id) {
                grant.nudged = true;
                grant.retry_at = now;
            }
        }
    }
}

/// The space `wiki_space_id`'s access changed (a member left the chat room whose boards it holds, the room was
/// archived): the grants of the editors of its pages open on this server are asked again at the next [`tick`], so one
/// who lost the page stops changing it within about a second rather than at the next [`ACCESS_RECHECK`].
pub fn recheck_space(wiki_space_id: &str) {
    let open: Vec<Arc<Mutex<Room>>> = lock(rooms()).values().cloned().collect();
    let now = Instant::now();
    for room in open {
        let mut room = lock(&room);
        if room.target.wiki_space_id != wiki_space_id {
            continue;
        }
        for grant in room.can_edit.values_mut().filter(|grant| grant.meeting.is_none()) {
            grant.nudged = true;
            grant.retry_at = now;
        }
    }
}

/// Age every member's access answer of `key` by `by`, as if it were asked that long ago.
#[cfg(test)]
fn age_grants(key: &str, by: Duration) {
    if let Some(room) = room_of(key) {
        for grant in lock(&room).can_edit.values_mut() {
            grant.checked = grant.checked.checked_sub(by).unwrap_or(grant.checked);
            grant.retry_at = grant.retry_at.checked_sub(by).unwrap_or(grant.retry_at);
        }
    }
}

fn tick_room(key: &str, room: &Mutex<Room>) -> Result<(), Error> {
    let (dirty, quiet, members, unsaved, on_database, failed_lately) = {
        let room = lock(room);
        (
            room.dirty,
            room.changed_at.elapsed(),
            room.members,
            !room.unsaved.is_empty(),
            room.target.on_database,
            room.save_failed_at.is_some_and(|at| at.elapsed() < IDLE_SAVE),
        )
    };
    if dirty && quiet >= FLUSH_AFTER {
        flush(key, room)?;
    }
    let idle_save = unsaved && on_database && quiet >= IDLE_SAVE && !failed_lately;
    if members == 0 || idle_save {
        if unsaved {
            if !save_as_an_editor(key, room) && lock(room).dirty {
                // the state, with its editors, is in the row: the next join or Save versions it
                flush(key, room)?;
            }
        } else if dirty {
            flush(key, room)?;
        }
        if members == 0 {
            // taken out under both locks, so a join between finds the room closed and loads it again
            let (closed, compact, stamp) = {
                let mut all = lock(rooms());
                let mut room = lock(room);
                let compact = matches!(room.target.codec, Codec::Board) && room.unsaved.is_empty() && !room.dirty;
                if room.members == 0 {
                    room.closed = true;
                    all.remove(key);
                }
                (room.closed, compact, room.stamp)
            };
            if closed && compact {
                if let Err(err) = compact_row(key, stamp) {
                    tracing::warn!("co-edited {key}: its stored state was not compacted: {}", err.message());
                }
            }
        }
    }
    Ok(())
}

/// Save the room as a version, as the first of its editors who may still change the page: the last editor, the others
/// whose changes are unsaved, then the members who may edit now. An editor who lost access (left the room's chat,
/// was removed from the space) is passed over. Out: whether a version was saved; when none could be, the failure is
/// logged once and the idle save waits a while before it tries again.
fn save_as_an_editor(key: &str, room: &Mutex<Room>) -> bool {
    let candidates: Vec<String> = {
        let room = lock(room);
        let mut ordered: Vec<String> = room.last_editor.iter().cloned().collect();
        ordered.extend(room.unsaved.iter().cloned());
        ordered.extend(
            room.can_edit
                .iter()
                .filter(|(_, grant)| grant.can_edit && grant.meeting.is_none())
                .map(|(user, _)| user.clone()),
        );
        let mut seen = BTreeSet::new();
        ordered.retain(|user| !user.is_empty() && seen.insert(user.clone()));
        ordered
    };
    let mut failures = Vec::new();
    for editor in &candidates {
        match save_room(key, room, editor, None) {
            Ok(_) => {
                lock(room).save_failed_at = None;
                return true;
            }
            Err(err) => failures.push(format!("{editor}: {}", err.message())),
        }
    }
    let mut room = lock(room);
    if room.save_failed_at.is_none() {
        tracing::error!(
            "co-edited {key}: no editor could save it as a version ({}); its state is kept for the next save",
            failures.join("; ")
        );
    }
    room.save_failed_at = Some(Instant::now());
    false
}

/// A board's room closed with every change in a version: its row's state is cleared, so the next join seeds the
/// document again from the saved scene ([`load`], [`Stored::Replace`]) and its Yjs history (every revision of every
/// element) does not grow for good. The saved scene keeps the deleted elements' tombstones (`board::render`). Left as
/// it is when another server wrote the row since this one did (`stamp`), as it may have the board open.
fn compact_row(key: &str, stamp: Option<i64>) -> Result<(), Error> {
    let facade = entity()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let tx = facade.transactions_for(COLLAB_DOCUMENT)?;
    tx.run(moqui_core::transaction::TransactionAttribute::ForceNew, None, || {
        let Some(mut row) = facade
            .find(COLLAB_DOCUMENT)?
            .eq("documentKey", Value::string(key))
            .for_update(true)
            .one()?
        else {
            return Ok(());
        };
        let unsaved = row.get("editorUserIds").is_some_and(|ids| !is_empty(ids));
        if unsaved || stamp.is_none() || stamp_of(&row) != stamp {
            return Ok(());
        }
        row.set("stateData", Value::Null);
        row.update(&facade)?;
        Ok(())
    })
    .map_err(Error::from)
}

/// Write the room's state now.
fn flush(key: &str, room: &Mutex<Room>) -> Result<(), Error> {
    let mut guard = lock(room);
    let unsaved = guard.unsaved.clone();
    let editor = guard.last_editor.clone();
    let written = write_row(
        key,
        (guard.target.codec, &guard.doc),
        Stored::Merge,
        None,
        None,
        (&unsaved, &guard.meeting_names),
        editor.as_deref(),
    )?;
    wrote(key, &mut guard, written);
    guard.dirty = false;
    Ok(())
}

/// Save the document as a version of its record, as `user_id` (an editor with update access), with `note` or one that
/// names its editors. Out: the version made, none when the text was unchanged.
fn save_room(key: &str, room: &Mutex<Room>, user_id: &str, note: Option<&str>) -> Result<Option<String>, Error> {
    let saving = Arc::clone(&lock(room).saving);
    let _save = lock(&saving);
    // what other servers stored goes into the version too, though an update of theirs was lost on its way here
    let row = stored(key)?;
    let (target, page_text, editors, meeting_names, saved_at) = {
        let mut room = lock(room);
        if let Some(row) = &row {
            merge_stored(key, &mut room, row);
        }
        (
            room.target.clone(),
            room.target.codec.render(&room.doc),
            room.unsaved.clone(),
            room.meeting_names.clone(),
            room.changed_at,
        )
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
    let note = if meeting_names.is_empty() {
        note
    } else {
        format!("{note} — {}", meeting_names.into_iter().collect::<Vec<_>>().join(", "))
    };
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
    let current = moqui_core::user::current_user_id().ok().flatten();
    let switched = current.as_deref() != Some(user_id);
    let identity = moqui_core::user::session::identity();
    if switched {
        moqui_core::user::internal_login_user(&username)?;
    }
    let parameters: BTreeMap<String, Value> = [
        ("wikiSpaceId", Value::string(&target.wiki_space_id)),
        ("wikiPageId", Value::string(&target.wiki_page_id)),
        ("pagePath", Value::string(&target.page_path)),
        ("pageName", Value::string(&target.page_name)),
        (
            "parentPath",
            target
                .page_path
                .rsplit_once('/')
                .map_or(Value::Null, |(parent, _)| Value::string(parent)),
        ),
        ("pageText", Value::string(&page_text)),
        ("versionNote", Value::string(&note)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    // No artifact authorization: an idle save runs on the socket's thread with no screen to inherit a grant from, and
    // update#WikiPage applies the wiki's access rule to the logged-in editor itself.
    let options = CallOptions {
        // Release the WikiPage version lock before writing the CRDT checkpoint
        // (a separate transaction), including when called while a meeting ends.
        require_new_transaction: Some(true),
        disable_authz: true,
        ..Default::default()
    };
    let result = sync_checked("org.moqui.resource.WikiServices.update#WikiPage", &parameters, &options);
    ExecutionContext::with_current(|ec| ec.message().clear_errors());
    if switched {
        moqui_core::user::session::restore_identity(&identity);
    }
    let out = result?;
    let version_name = Some(text(&out, "versionName")).filter(|name| !name.is_empty());
    {
        let mut room = lock(room);
        // The page service ran without the room lock. A stroke made meanwhile belongs
        // to the following version, even when it came from the same editor.
        let unsaved = if room.changed_at == saved_at {
            BTreeSet::new()
        } else {
            room.unsaved.clone()
        };
        let names = if unsaved.is_empty() {
            BTreeSet::new()
        } else {
            room.meeting_names.clone()
        };
        let written = write_row(
            key,
            (room.target.codec, &room.doc),
            Stored::Save,
            Some(&text_hash(&page_text)),
            version_name.as_deref(),
            (&unsaved, &names),
            None,
        )?;
        room.meeting_names = names;
        room.unsaved = unsaved;
        room.dirty = false;
        wrote(key, &mut room, written);
    }
    lock(events()).push(Event::Saved {
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
    let user_id = moqui_core::user::current_user_id()
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
    lock(&room).unsaved.insert(user_id.clone());
    let saved = save_room(key, &room, &user_id, note)?;
    {
        let mut all = lock(rooms());
        let mut room = lock(&room);
        if room.members == 0 {
            room.closed = true;
            all.remove(key);
        }
    }
    Ok(saved)
}

#[cfg(test)]
mod tests;
