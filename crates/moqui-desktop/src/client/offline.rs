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

//! The server's answers kept in an account's store and answered from it when the server cannot be reached
//! (plans/Mobile-Desktop.md §6, MD4: "the scheme handler answers the existing JSON transitions from the store"), so the
//! shared screens work offline without a fork. The scheme (`scheme.rs`) and the mirror (`mirror.rs`) write through
//! [`keep`]; the scheme answers through [`answer`].
//!
//! The transitions ([`Kind`]): the shell's settings, apps and counts (`moqui_client::cache::PATHS`, by path alone), and
//! under the account's comms app (`calls.appRoot` of its client config, `/qapps/BasicComms` by default, asked for under
//! `/apps` or `/qapps`): Chat's `chatData`, `rooms`, `roomView`, `room` and `timeline`, Mail's `inboxData`, `threads`
//! and `thread`, and Compose's `composeData`.
//!
//! - Every 200 JSON answer of one is kept whole, by path and query, less the query's volatile parameters
//!   ([`VOLATILE`]), and answered as it was for the same request offline.
//! - Some are also kept as rows, so pages never asked for whole still answer offline: a timeline page (and a room
//!   view's first page) as the room's events with the stretch it covers (`Store::put_chat_page`), the room list
//!   (`chatData`, `rooms` unfiltered) as rooms, `inboxData`'s folders, a folder's page of `threads` as its threads, and
//!   a `thread` as its messages. A room's fields the timeline answers with (`room`, `access`, `memberList`, the read
//!   marker, and the names of the users its events name, gathered page after page) are kept beside them.
//! - Offline, these are built from the rows in the shape the server answers, so pushes and later pages show: `timeline`
//!   (but around a seq, pinned only or one event), `roomView` (the room's kept fields and its latest page), `chatData`
//!   and an unfiltered `rooms` (the rooms, with the kept user and preferences), a folder's `threads` (not a search; its
//!   cursor in the server's form, one store's or mr-mantle's several) and `thread` (its kept fields and messages). The
//!   rest, and these when the rows do not cover the request, come from the kept answer. Nothing kept: none, and the
//!   scheme answers 502 as before.

use moqui_client::store::{ChatEvent, ChatRoom, EventPage, EventSlice, MailFolder, MailMessage, MailThread, Store};
use serde_json::{json, Map, Value};

/// The comms app of a server whose client config does not name one.
pub(crate) const DEFAULT_COMMS_ROOT: &str = "/qapps/BasicComms";

/// Query parameters that change from request to request without changing the answer: left out of an answer's key.
pub(crate) const VOLATILE: [&str; 3] = ["_", "moquiSessionToken", "pageTraceId"];

/// Where a room's fields are kept (a path no request has).
const ROOM_META: &str = "~chat-room";
/// Where a mail folder's list fields are kept.
const FOLDER_META: &str = "~mail-folder";
/// Where the user and the chat preferences of `chatData` are kept.
const CHAT_META: &str = "~chat-data";
/// Where a mail thread's fields but its messages are kept.
const THREAD_META: &str = "~mail-thread";

/// The most events a timeline page has, as `get#Timeline`.
const PAGE_MAX: u32 = 200;

/// A transition whose answers are kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The shell's settings, apps or counts, kept by path alone.
    Shell,
    ChatData,
    Rooms,
    RoomView,
    Room,
    Timeline,
    Inbox,
    MailThreads,
    MailThread,
    Compose,
}

/// The kind of a `GET` of `bare` (the path with no query), under the comms app `comms_root`; none for a request whose
/// answer is not kept.
pub(crate) fn classify(bare: &str, comms_root: &str) -> Option<Kind> {
    if moqui_client::cache::PATHS.contains(&bare) {
        return Some(Kind::Shell);
    }
    let app = app_path(comms_root);
    let under = bare.strip_prefix("/apps").or_else(|| bare.strip_prefix("/qapps"))?;
    let tail = under.strip_prefix(app.as_str())?;
    Some(match tail {
        "/Chat/chatData" => Kind::ChatData,
        "/Chat/rooms" => Kind::Rooms,
        "/Chat/roomView" => Kind::RoomView,
        "/Chat/room" => Kind::Room,
        "/Chat/timeline" => Kind::Timeline,
        "/Inbox/Mail/inboxData" => Kind::Inbox,
        "/Inbox/Mail/threads" => Kind::MailThreads,
        "/Inbox/Mail/thread" => Kind::MailThread,
        "/Inbox/Compose/composeData" => Kind::Compose,
        _ => return None,
    })
}

/// The comms app's path under `/apps` (or `/qapps`): `/BasicComms` of `/qapps/BasicComms`.
fn app_path(comms_root: &str) -> String {
    let root = comms_root.trim().trim_end_matches('/');
    let rest = root
        .strip_prefix("/qapps")
        .or_else(|| root.strip_prefix("/apps"))
        .unwrap_or(root);
    if rest.is_empty() || rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

/// The path of Chat's transitions under the comms app, as the pages ask for them: `/apps/BasicComms/Chat`.
pub(crate) fn chat_base(comms_root: &str) -> String {
    format!("/apps{}/Chat", app_path(comms_root))
}

/// The path of Mail's transitions: `/apps/BasicComms/Inbox/Mail`.
pub(crate) fn mail_base(comms_root: &str) -> String {
    format!("/apps{}/Inbox/Mail", app_path(comms_root))
}

/// The path of Compose's transitions: `/apps/BasicComms/Inbox/Compose`.
pub(crate) fn compose_base(comms_root: &str) -> String {
    format!("/apps{}/Inbox/Compose", app_path(comms_root))
}

/// The query an answer of `kind` is kept under: none for the shell's, else `query` less its volatile parameters.
pub(crate) fn kept_query(kind: Kind, query: &str) -> String {
    if kind == Kind::Shell {
        return String::new();
    }
    query
        .trim_start_matches('?')
        .split('&')
        .filter(|parameter| {
            let name = parameter.split_once('=').map_or(*parameter, |(name, _)| name);
            !parameter.is_empty() && !VOLATILE.contains(&name)
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The decoded parameters of a query.
struct Params(Vec<(String, String)>);

impl Params {
    fn of(query: &str) -> Self {
        let query = query.trim_start_matches('?');
        let parsed = tauri::Url::parse(&format!("http://query.invalid/?{query}"));
        Self(parsed.map_or_else(
            |_| Vec::new(),
            |url| {
                url.query_pairs()
                    .map(|(name, value)| (name.into_owned(), value.into_owned()))
                    .collect()
            },
        ))
    }

    /// The first value of `name`, if not empty.
    fn text(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    fn number(&self, name: &str) -> Option<i64> {
        self.text(name).and_then(|value| value.trim().parse().ok())
    }

    fn truthy(&self, name: &str) -> bool {
        self.text(name) == Some("true")
    }
}

/// A number of the server's JSON: a number, or a string of one (a date is epoch millis).
pub(crate) fn number(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Keep a 200 JSON answer of `kind` to `bare` with `query`: whole, and as rows where they serve offline paging. A
/// failure is logged and the rest kept: the store only helps.
pub(crate) fn keep(store: &Store, kind: Kind, bare: &str, query: &str, body: &[u8]) {
    if let Err(err) = store.put_answer(bare, &kept_query(kind, query), "application/json", body) {
        tracing::warn!("The answer of {bare} was not kept: {err}");
    }
    if kind == Kind::Shell || kind == Kind::Compose {
        return;
    }
    let Ok(json) = serde_json::from_slice::<Value>(body) else {
        tracing::warn!("The answer of {bare} is not JSON; kept whole only");
        return;
    };
    let params = Params::of(query);
    let kept = match kind {
        Kind::ChatData => keep_chat_data(store, &params, &json),
        Kind::Rooms => {
            let filtered = params.text("filter").is_some()
                || params.text("searchText").is_some()
                || params.truthy("includeJoinable")
                || params.truthy("joinableOnly");
            if filtered {
                Ok(())
            } else {
                keep_rooms(store, &json["roomList"], params.number("limit").unwrap_or(200))
            }
        }
        Kind::RoomView => keep_room_view(store, &params, &json),
        Kind::Room => match params.text("chatRoomId") {
            Some(room) => keep_room_meta(store, room, &json, json["member"]["lastReadEventSeq"].clone()),
            None => Ok(()),
        },
        Kind::Timeline => keep_timeline(store, &params, &json),
        Kind::Inbox => keep_folders(store, &json),
        Kind::MailThreads => keep_threads(store, &params, &json),
        Kind::MailThread => keep_thread(store, &json),
        Kind::Shell | Kind::Compose => Ok(()),
    };
    if let Err(err) = kept {
        tracing::warn!("The rows of {bare} were not kept: {err}");
    }
}

/// The answer of `kind` to `bare` with `query` from the store, for when the server cannot be reached: built from the
/// rows where they answer it (a timeline page, a room view, the room list, a folder's threads, a thread), so what pushes
/// and later pages changed shows; else the kept answer; none when nothing is kept.
pub(crate) fn answer(store: &Store, kind: Kind, bare: &str, query: &str) -> Option<Vec<u8>> {
    let params = Params::of(query);
    let built = match kind {
        Kind::Timeline => timeline_json(store, &params),
        Kind::RoomView => room_view_json(store, &params),
        Kind::ChatData => rooms_json(store, &params, true),
        Kind::Rooms => rooms_json(store, &params, false),
        Kind::MailThreads => threads_json(store, &params),
        Kind::MailThread => thread_json(store, &params),
        _ => None,
    };
    built
        .map(|json| json.to_string().into_bytes())
        .or_else(|| store.answer(bare, &kept_query(kind, query)).map(|kept| kept.body))
}

/// A kept JSON object of a synthetic path, or none.
fn kept_object(store: &Store, path: &str, query: &str) -> Option<Map<String, Value>> {
    match serde_json::from_slice::<Value>(&store.answer(path, query)?.body).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn put_object(store: &Store, path: &str, query: &str, value: &Value) -> Result<(), String> {
    store
        .put_answer(path, query, "application/json", value.to_string().as_bytes())
        .map_err(|err| err.to_string())
}

// ---- chat ----

/// A room of a room list as the store keeps it: ordered by its last message's date.
pub(crate) fn chat_room(row: &Value) -> Option<ChatRoom> {
    let id = row["chatRoomId"].as_str().filter(|id| !id.is_empty())?;
    let last = &row["lastMessage"];
    Some(ChatRoom {
        chat_room_id: id.to_string(),
        row: row.to_string(),
        last_seq: number(&last["eventSeq"])
            .or_else(|| number(&row["lastEventSeq"]))
            .unwrap_or(0),
        sort_ms: number(&last["sentDate"])
            .or_else(|| number(&row["lastEventDate"]))
            .or_else(|| number(&row["lastUpdatedStamp"]))
            .unwrap_or(0),
    })
}

/// An event of a timeline page as the store keeps it: a thread reply under its root.
pub(crate) fn chat_event(room: &str, event: &Value) -> Option<ChatEvent> {
    let seq = number(&event["eventSeq"])?;
    Some(ChatEvent {
        chat_room_id: room.to_string(),
        event_seq: seq,
        event: event.to_string(),
        thread_seq: number(&event["threadRootEventSeq"]).filter(|root| *root != seq),
    })
}

/// Keep a room list: the whole list replaces the rooms kept (left rooms go with their events) when it is shorter than
/// the `limit` it was asked with, else each room is written and none removed.
fn keep_rooms(store: &Store, list: &Value, limit: i64) -> Result<(), String> {
    let Some(list) = list.as_array() else { return Ok(()) };
    let rooms: Vec<ChatRoom> = list.iter().filter_map(chat_room).collect();
    let whole = i64::try_from(list.len()).unwrap_or(i64::MAX) < limit;
    if whole {
        store.replace_chat_rooms(&rooms).map_err(|err| err.to_string())?;
    } else {
        for room in &rooms {
            store.put_chat_room(room).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

/// `chatData`: the room list, and the user and the chat preferences beside it.
fn keep_chat_data(store: &Store, params: &Params, json: &Value) -> Result<(), String> {
    keep_rooms(store, &json["roomList"], params.number("limit").unwrap_or(200))?;
    put_object(
        store,
        CHAT_META,
        "",
        &json!({ "me": json["me"], "prefs": json["prefs"] }),
    )
}

/// The room list from the store, as `chatData` (`{me, prefs, roomList, unreadTotal, mentionTotal}`, with the kept user
/// and preferences) or an unfiltered `rooms` (`{roomList}`) answer it: pinned first, then by the last message, with
/// the unread counts and last messages pushes changed. None for a filtered list, or when nothing is kept.
fn rooms_json(store: &Store, params: &Params, chat_data: bool) -> Option<Value> {
    let filtered = params.text("filter").is_some()
        || params.text("searchText").is_some()
        || params.truthy("includeJoinable")
        || params.truthy("joinableOnly");
    if filtered {
        return None;
    }
    let limit = u32::try_from(params.number("limit").unwrap_or(200).clamp(1, 500)).unwrap_or(200);
    let mut rooms: Vec<Value> = store
        .chat_rooms(limit)
        .iter()
        .filter_map(|room| serde_json::from_str(&room.row).ok())
        .collect();
    if rooms.is_empty() {
        return None;
    }
    // stable: the store's order (the last message, newest first) within pinned and not
    rooms.sort_by_key(|room| room["isPinned"].as_bool() != Some(true));
    if !chat_data {
        return Some(json!({ "roomList": rooms, "offline": true }));
    }
    let meta = kept_object(store, CHAT_META, "")?;
    let total = |field: &str| rooms.iter().map(|room| number(&room[field]).unwrap_or(0)).sum::<i64>();
    let (unread, mentions) = (total("unreadCount"), total("mentionCount"));
    Some(
        json!({ "me": meta.get("me"), "prefs": meta.get("prefs"), "roomList": rooms, "unreadTotal": unread,
        "mentionTotal": mentions, "offline": true }),
    )
}

/// The `get#Timeline` request of a query; none for one around a seq, pinned only or of one event, which the rows do
/// not answer.
fn event_page(params: &Params) -> Option<EventPage> {
    if params.text("aroundSeq").is_some() || params.text("eventSeq").is_some() || params.truthy("pinnedOnly") {
        return None;
    }
    let limit = params.number("limit").unwrap_or(50).clamp(1, i64::from(PAGE_MAX));
    Some(EventPage {
        before: params.number("beforeSeq"),
        after: params.number("afterSeq"),
        thread: params.number("threadRootEventSeq"),
        limit: u32::try_from(limit).unwrap_or(50),
    })
}

/// Keep a timeline page (as `get#TimelinePage` answers it) of `room` for `request`: its events and stretch, and the
/// room's fields.
fn keep_page(store: &Store, room: &str, request: &EventPage, page: &Value) -> Result<(), String> {
    let events: Vec<ChatEvent> = page["eventList"]
        .as_array()
        .map(|list| list.iter().filter_map(|event| chat_event(room, event)).collect())
        .unwrap_or_default();
    store
        .put_chat_page(
            room,
            request,
            &events,
            page["hasMoreBefore"].as_bool().unwrap_or(false),
            page["hasMoreAfter"].as_bool().unwrap_or(false),
        )
        .map_err(|err| err.to_string())?;
    keep_room_meta(store, room, page, page["lastReadEventSeq"].clone())
}

fn keep_timeline(store: &Store, params: &Params, json: &Value) -> Result<(), String> {
    let Some(room) = params.text("chatRoomId") else {
        return Ok(());
    };
    match event_page(params) {
        Some(request) => keep_page(store, room, &request, json),
        None => keep_room_meta(store, room, json, json["lastReadEventSeq"].clone()),
    }
}

/// A room view: the room's fields, and its first page when it is the latest (not around a seq).
fn keep_room_view(store: &Store, params: &Params, json: &Value) -> Result<(), String> {
    let Some(room) = params.text("chatRoomId") else {
        return Ok(());
    };
    keep_room_meta(store, room, json, json["member"]["lastReadEventSeq"].clone())?;
    let timeline = &json["timeline"];
    if !timeline.is_object() || params.text("aroundSeq").is_some() {
        return Ok(());
    }
    let limit = params.number("limit").unwrap_or(50).clamp(1, i64::from(PAGE_MAX));
    let request = EventPage {
        limit: u32::try_from(limit).unwrap_or(50),
        ..EventPage::default()
    };
    keep_page(store, room, &request, timeline)
}

/// The fields of a room a timeline or room view answers with, kept beside its events: `room`, `member` (mine), `access`
/// and `memberList` replaced by
/// the latest given, the read marker, and `userNames` gathered from every page (an offline page joins events of
/// several).
fn keep_room_meta(store: &Store, room: &str, from: &Value, last_read: Value) -> Result<(), String> {
    let query = format!("chatRoomId={room}");
    let mut meta = room_meta(store, room);
    for field in ["room", "member", "access", "memberList"] {
        if !from[field].is_null() {
            meta.insert(field.to_string(), from[field].clone());
        }
    }
    if !last_read.is_null() {
        meta.insert("lastReadEventSeq".to_string(), last_read);
    }
    if let Some(names) = from["userNames"]
        .as_object()
        .or_else(|| from["timeline"]["userNames"].as_object())
    {
        let kept = meta.entry("userNames").or_insert_with(|| Value::Object(Map::new()));
        if let Some(kept) = kept.as_object_mut() {
            for (id, name) in names {
                kept.insert(id.clone(), name.clone());
            }
        }
    }
    store
        .put_answer(
            ROOM_META,
            &query,
            "application/json",
            Value::Object(meta).to_string().as_bytes(),
        )
        .map_err(|err| err.to_string())
}

/// The kept fields of a room; none kept is an empty map.
fn room_meta(store: &Store, room: &str) -> Map<String, Value> {
    store
        .answer(ROOM_META, &format!("chatRoomId={room}"))
        .and_then(|kept| serde_json::from_slice::<Value>(&kept.body).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

/// The room read up to `last_read` here or on another device (a pushed read marker): the kept read marker, mine too.
pub(crate) fn note_read(store: &Store, room: &str, last_read: &Value) -> Result<(), String> {
    if last_read.is_null() {
        return Ok(());
    }
    let mut meta = room_meta(store, room);
    if meta.is_empty() {
        return Ok(());
    }
    meta.insert("lastReadEventSeq".to_string(), last_read.clone());
    if let Some(member) = meta.get_mut("member").and_then(Value::as_object_mut) {
        member.insert("lastReadEventSeq".to_string(), last_read.clone());
    }
    put_object(store, ROOM_META, &format!("chatRoomId={room}"), &Value::Object(meta))
}

/// A timeline page from the store in `get#TimelinePage`'s shape: `{room, access, eventList, hasMoreBefore,
/// hasMoreAfter, lastReadEventSeq, memberList, userNames}`, with `offline: {gapBefore, gapAfter}` beside them. None
/// when the rows do not answer the request: around a seq, pinned only or one event, no stretch held there, or a page
/// wholly in a gap (older events never fetched, asked for by `beforeSeq`; newer ones, by `afterSeq`), which needs the
/// server.
///
/// `hasMoreBefore` and `hasMoreAfter` say what the store holds: at a gap they are false, so the screen does not ask
/// for what offline cannot answer (and show an error each time); `offline.gapBefore` and `gapAfter` say the server may
/// have more there.
fn timeline_json(store: &Store, params: &Params) -> Option<Value> {
    let room = params.text("chatRoomId")?;
    let page = event_page(params)?;
    let slice = store.chat_events(room, &page);
    let forward = page.after.is_some();
    let in_gap = if forward { slice.gap_after } else { slice.gap_before };
    if slice.events.is_empty() && (in_gap || (slice.gap_before && slice.gap_after)) {
        return None;
    }
    let meta = room_meta(store, room);
    Some(page_json(&slice, &meta))
}

/// A room view from the store, as `roomView` answers it (`{room, member, access, memberList, timeline}`), the timeline
/// its latest page held; for a room whose fields are kept (a timeline page or a room view of it was read). None for a
/// view around a seq.
fn room_view_json(store: &Store, params: &Params) -> Option<Value> {
    let room = params.text("chatRoomId")?;
    if params.text("aroundSeq").is_some() {
        return None;
    }
    let meta = room_meta(store, room);
    if !meta.contains_key("room") {
        return None;
    }
    let limit = params.number("limit").unwrap_or(50).clamp(1, i64::from(PAGE_MAX));
    let latest = EventPage {
        limit: u32::try_from(limit).unwrap_or(50),
        ..EventPage::default()
    };
    let slice = store.chat_events(room, &latest);
    let field = |name: &str| meta.get(name).cloned().unwrap_or(Value::Null);
    Some(json!({
        "room": field("room"),
        "member": field("member"),
        "access": field("access"),
        "memberList": meta.get("memberList").cloned().unwrap_or_else(|| json!([])),
        "timeline": page_json(&slice, &meta),
        "offline": true,
    }))
}

fn page_json(slice: &EventSlice, meta: &Map<String, Value>) -> Value {
    let events: Vec<Value> = slice
        .events
        .iter()
        .filter_map(|event| serde_json::from_str(&event.event).ok())
        .collect();
    let field = |name: &str| meta.get(name).cloned().unwrap_or(Value::Null);
    json!({
        "room": field("room"),
        "access": field("access"),
        "eventList": events,
        "hasMoreBefore": slice.has_more_before,
        "hasMoreAfter": slice.has_more_after,
        "lastReadEventSeq": field("lastReadEventSeq"),
        "memberList": meta.get("memberList").cloned().unwrap_or_else(|| json!([])),
        "userNames": meta.get("userNames").cloned().unwrap_or_else(|| json!({})),
        "offline": { "gapBefore": slice.gap_before, "gapAfter": slice.gap_after },
    })
}

// ---- mail ----

/// The folder a `threads` query lists, as the store keys its threads: `account=A&folder=F`, `account=A&label=L`, or
/// `view=V` with `account=A` (the server's order: a folder, then a label, then the view, `inbox` by default). None for a
/// search, an unread-only list or a mailbox, which are kept as answers only.
fn folder_key(params: &Params) -> Option<String> {
    if params.text("q").is_some() || params.truthy("unreadOnly") || params.text("mailbox").is_some() {
        return None;
    }
    let account = params.text("account").map(|account| format!("account={account}&"));
    let account = account.as_deref().unwrap_or("");
    Some(if let Some(folder) = params.text("folder") {
        format!("{account}folder={folder}")
    } else if let Some(label) = params.text("label") {
        format!("{account}label={label}")
    } else {
        format!("{account}view={}", params.text("view").unwrap_or("inbox"))
    })
}

/// The key of an account's folder, as a `threads` query of it gives ([`folder_key`]).
pub(crate) fn account_folder_key(account: &str, folder: &str) -> String {
    format!("account={account}&folder={folder}")
}

/// A `threads` cursor: each store's date, `store:<millis>` (the sortDate of the last row of that store shown) or
/// `store:end` (no more rows), joined by commas, as mr-mantle's `find#InboxThreads` gives it; BasicComms' one store's
/// is `email:<millis>` or `email:end`.
#[derive(Debug, PartialEq, Eq)]
enum Cursor {
    /// No cursor: the folder's first page.
    First,
    /// Each store's date; none for a store at its end.
    Stores(Vec<(String, Option<i64>)>),
    /// A cursor of another form.
    Unknown,
}

fn cursor_of(params: &Params) -> Cursor {
    let Some(text) = params.text("cursor") else {
        return Cursor::First;
    };
    let mut stores = Vec::new();
    for part in text.split(',') {
        let Some((name, value)) = part.trim().split_once(':') else {
            return Cursor::Unknown;
        };
        let date = match value {
            "end" => None,
            number => match number.parse() {
                Ok(date) => Some(date),
                Err(_) => return Cursor::Unknown,
            },
        };
        stores.push((name.to_string(), date));
    }
    if stores.is_empty() {
        Cursor::Unknown
    } else {
        Cursor::Stores(stores)
    }
}

/// The store of a thread row's key (`email:…`, `internal:…`).
fn store_of(thread_id: &str) -> &str {
    thread_id.split_once(':').map_or("", |(store, _)| store)
}

/// `inboxData`'s folders, each account's, keyed as their `threads` queries are.
fn keep_folders(store: &Store, json: &Value) -> Result<(), String> {
    let Some(accounts) = json["accountList"].as_array() else {
        return Ok(());
    };
    let mut folders = Vec::new();
    for account in accounts {
        let Some(account_id) = account["emailAccountId"].as_str() else {
            continue;
        };
        for folder in account["folderList"].as_array().into_iter().flatten() {
            let Some(folder_id) = folder["emailFolderId"].as_str() else {
                continue;
            };
            let mut row = folder.clone();
            if let Some(map) = row.as_object_mut() {
                map.insert("emailAccountId".to_string(), json!(account_id));
            }
            folders.push(MailFolder {
                folder_id: account_folder_key(account_id, folder_id),
                row: row.to_string(),
            });
        }
    }
    store
        .replace_mail_folders(&folders)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// A row of a `threads` page as the store keeps it, by its `rowKey` (a thread, or a message in the conversation view
/// of messages).
pub(crate) fn mail_thread(folder: &str, row: &Value) -> Option<MailThread> {
    let id = row["rowKey"].as_str().map(str::to_string).or_else(|| {
        row["threadId"]
            .as_str()
            .map(|thread| format!("{}:{thread}", row["store"].as_str().unwrap_or("email")))
    })?;
    Some(MailThread {
        folder_id: folder.to_string(),
        thread_id: id,
        sort_ms: number(&row["sortDate"]).unwrap_or(0),
        row: row.to_string(),
    })
}

/// A folder's page of `threads`. Its threads replace the folder's in the stretch the page covers when that is sure: the
/// first page (from its oldest row on), or a page whose cursor has every store's date (below the earliest of them, as
/// every row of every store from there to the page's oldest is on this page). A page after a store ended, or with a
/// cursor of another form, only adds its threads. A change of the conversation view (threads or messages) empties the
/// folder first, as its rows are of another kind.
fn keep_threads(store: &Store, params: &Params, json: &Value) -> Result<(), String> {
    let Some(folder) = folder_key(params) else {
        return Ok(());
    };
    let threads: Vec<MailThread> = json["threadList"]
        .as_array()
        .map(|list| list.iter().filter_map(|row| mail_thread(&folder, row)).collect())
        .unwrap_or_default();
    let meta_query = folder_meta_query(&folder);
    let view = json["conversationView"].as_str();
    let kept_view = kept_object(store, FOLDER_META, &meta_query)
        .and_then(|meta| meta.get("conversationView").and_then(Value::as_str).map(str::to_string));
    if let (Some(view), Some(kept_view)) = (view, kept_view) {
        if view != kept_view {
            store
                .replace_mail_page(&folder, None, &[], false)
                .map_err(|err| err.to_string())?;
        }
    }
    let more = json["hasMore"].as_bool().unwrap_or(false);
    let written = match cursor_of(params) {
        Cursor::First => store.replace_mail_page(&folder, None, &threads, more),
        Cursor::Stores(stores) if stores.iter().all(|(_, date)| date.is_some()) => {
            let before = stores.iter().filter_map(|(_, date)| *date).min();
            store.replace_mail_page(&folder, before, &threads, more)
        }
        Cursor::Stores(_) | Cursor::Unknown => store.put_mail_threads(&threads),
    };
    written.map_err(|err| err.to_string())?;
    put_object(
        store,
        FOLDER_META,
        &meta_query,
        &json!({ "storeList": json["storeList"], "conversationView": json["conversationView"] }),
    )
}

fn folder_meta_query(folder: &str) -> String {
    format!("folder={}", folder.replace('&', ";"))
}

/// A `thread` answer's messages, replacing what was kept of the thread.
fn keep_thread(store: &Store, json: &Value) -> Result<(), String> {
    let thread = &json["thread"];
    let Some(thread_id) = thread["threadId"].as_str() else {
        return Ok(());
    };
    let id = format!("{}:{thread_id}", thread["store"].as_str().unwrap_or("email"));
    let messages: Vec<MailMessage> = thread["messageList"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|message| {
                    let message_id = message["key"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| message["emailMessageId"].as_str().map(|id| format!("email:{id}")))?;
                    Some(MailMessage {
                        message_id,
                        thread_id: id.clone(),
                        sort_ms: number(&message["date"]).unwrap_or(0),
                        row: message.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    store
        .replace_thread_messages(&id, &messages)
        .map_err(|err| err.to_string())?;
    let mut fields = thread.clone();
    if let Some(map) = fields.as_object_mut() {
        map.remove("messageList");
    }
    put_object(store, THREAD_META, &format!("thread={id}"), &fields)
}

/// A thread from the store as `thread` answers it (`{thread}`): its kept fields with its messages' rows, oldest first.
fn thread_json(store: &Store, params: &Params) -> Option<Value> {
    let id = format!(
        "{}:{}",
        params.text("store").unwrap_or("email"),
        params.text("threadId")?
    );
    let mut thread = kept_object(store, THREAD_META, &format!("thread={id}"))?;
    let messages: Vec<Value> = store
        .mail_messages(&id)
        .iter()
        .filter_map(|message| serde_json::from_str(&message.row).ok())
        .collect();
    if messages.is_empty() {
        return None;
    }
    thread.insert("messageList".to_string(), Value::Array(messages));
    Some(json!({ "thread": thread, "offline": true }))
}

/// A page of a folder's threads from the store in `find#InboxRows`' shape: `{threadList, hasMore, nextCursor,
/// storeList, failedStores, termList, conversationView}`, with `offline: true`. Paged by the cursor as the server pages
/// (each store's rows older than its own date, none of a store at its end), and its `nextCursor` in the same form, so
/// the screen sends it back: `end` for a store with no more rows held, else its last row on this page, or the date it
/// came with. None for a search, an unread-only list, a mailbox or a cursor of another form, and when the
/// store holds none of the folder's threads there.
fn threads_json(store: &Store, params: &Params) -> Option<Value> {
    let folder = folder_key(params)?;
    let meta = kept_object(store, FOLDER_META, &folder_meta_query(&folder)).unwrap_or_default();
    let mut store_names: Vec<String> = meta
        .get("storeList")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    if store_names.is_empty() {
        store_names.push("email".to_string());
    }
    let view = meta
        .get("conversationView")
        .and_then(Value::as_str)
        .unwrap_or("threads")
        .to_string();
    let shape = |rows: Vec<Value>, more: bool, next: Option<String>| {
        json!({ "threadList": rows, "hasMore": more, "nextCursor": next, "storeList": store_names, "failedStores": [],
            "termList": [], "conversationView": view, "offline": true })
    };
    let given: Vec<(String, Option<i64>)> = match cursor_of(params) {
        Cursor::First => Vec::new(),
        Cursor::Stores(stores) => stores,
        Cursor::Unknown => return None,
    };
    if !given.is_empty() && given.iter().all(|(_, date)| date.is_none()) {
        return Some(shape(Vec::new(), false, None));
    }
    let size = usize::try_from(params.number("pageSize").unwrap_or(50).clamp(1, 200)).unwrap_or(50);
    // rows below the latest store date, each then kept only below its own store's
    let before = given.iter().filter_map(|(_, date)| *date).max();
    let read = u32::try_from(size * 4 + 50).unwrap_or(850);
    let fetched = store.mail_threads(&folder, before, read);
    // all the folder's rows there were read: a store with none left is at its end
    let whole = fetched.len() < usize::try_from(read).unwrap_or(usize::MAX);
    let rows: Vec<MailThread> = fetched
        .into_iter()
        .filter(
            |row| match given.iter().find(|(name, _)| name == store_of(&row.thread_id)) {
                Some((_, Some(date))) => row.sort_ms < *date,
                Some((_, None)) => false,
                None => true,
            },
        )
        .collect();
    if rows.is_empty() {
        return None;
    }
    let (page, rest) = rows.split_at(rows.len().min(size));
    let more = !rest.is_empty();
    let next = more.then(|| {
        let mut names = store_names.clone();
        for row in page.iter().chain(rest) {
            let name = store_of(&row.thread_id).to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let last_shown = page.last().map_or(0, |row| row.sort_ms);
        names
            .iter()
            .map(|name| {
                let shown = page.iter().rev().find(|row| store_of(&row.thread_id) == name);
                let left = rest.iter().any(|row| store_of(&row.thread_id) == name);
                let value = match shown {
                    _ if whole && !left => "end".to_string(),
                    Some(row) => row.sort_ms.to_string(),
                    None if left => given
                        .iter()
                        .find(|(given_name, _)| given_name == name)
                        .and_then(|(_, date)| *date)
                        .unwrap_or(last_shown)
                        .to_string(),
                    None => "end".to_string(),
                };
                format!("{name}:{value}")
            })
            .collect::<Vec<_>>()
            .join(",")
    });
    let list = page
        .iter()
        .filter_map(|row| serde_json::from_str(&row.row).ok())
        .collect();
    Some(shape(list, more, next))
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;
