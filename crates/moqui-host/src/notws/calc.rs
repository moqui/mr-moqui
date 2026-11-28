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

//! CalcGrid's editing together on the notification socket (moqui-calc `together.rs`; docs/ui/Qvt-Client.md, CalcGrid
//! rooms). The cells and the edits go over the screen's transitions; this socket tells a view when to read again and
//! where the others are. A client sends JSON frames with `calc` (`join`, `select`, `leave`), each naming its `room` and
//! `client`, and gets frames on the topic `calc:<room>`:
//!
//! - `joined` (`peers`: who else has the document open, and where), `peer` (one came or went, `joined`), `select`
//!   (where one is now: `sheet`, `area`), across the rooms of every scope of the document and every server
//!   (`calc-presence`); a name and a place, never a value.
//! - `changed` (`seq`, `userId`, `name`, `kind`): an edit, a save, a refresh or a revert of the document on any server
//!   (`calc-update`), after this server's rooms of it applied it; `reopen` when the view's room could not follow and
//!   was dropped.
//!
//! A join is allowed when the socket's user has the room open ([`moqui_calc::together::member_of`]), which only a
//! transition the user may call makes them.
//!
//! The terminal client in this process (SSH, the browser terminal: `crate::tui`'s in-process link) has no socket: it
//! is a member through a [`LinkFrames`] sink beside the sockets', which takes the same frames with the same checks
//! (the room open for the link's user, the same presence caps) and is given the same frames a socket member is.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use moqui_calc::together::{self, Presence};
use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::{TopicMessage, CALC_PRESENCE, CALC_UPDATE};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::{Push, Slot};

/// Where a member's frames go: a notification socket, or the sink of an in-process terminal link.
trait Out: Send + Sync {
    /// The socket's id, or the sink's (from the same counter): the presence caps are kept by it.
    fn id(&self) -> u64;
    /// The user it is for now; None for a link nobody is logged in to.
    fn user(&self) -> Option<String>;
    /// Queue one frame; a socket with a full queue drops it.
    fn send(&self, message: &Json);
    fn is_closed(&self) -> bool;
}

impl Out for Slot {
    fn id(&self) -> u64 {
        self.id
    }

    fn user(&self) -> Option<String> {
        Some(self.user_id.clone())
    }

    fn send(&self, message: &Json) {
        // a full queue closes the socket the next time a notification finds it full; a calc frame is dropped
        let _ = self.tx.try_send(Push {
            text: message.to_string(),
            message_id: String::new(),
        });
    }

    fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// A view on this server: its socket, its room and the room's document (the presence key), its client id and user.
#[derive(Clone)]
struct Member {
    slot: Weak<dyn Out>,
    slot_id: u64,
    room: String,
    doc: String,
    client: String,
    user_id: String,
    name: String,
    /// A view that follows the document without being one of its viewers (a dashboard's range panel): it is told of
    /// the document's changes, and has no place or presence for the others.
    watch: bool,
}

fn members() -> &'static Mutex<Vec<Member>> {
    static MEMBERS: OnceLock<Mutex<Vec<Member>>> = OnceLock::new();
    MEMBERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// A frame to a member: not to a link whose user is no longer the member's (another logged in since the join).
fn send_to(member: &Member, message: &Json) {
    if let Some(out) = member.slot.upgrade() {
        if out.user().as_deref() == Some(member.user_id.as_str()) {
            out.send(message);
        }
    }
}

fn topic(room: &str) -> String {
    format!("calc:{room}")
}

fn error(out: &dyn Out, room: &str, client: &str, code: u16, message: &str) {
    out.send(&json!({"topic": topic(room), "type": "error", "client": client, "code": code, "message": message}));
}

/// Send `frame` to the views of the document `doc` on this server, each on its own room's topic, but the client
/// `except`; with `rooms`, only to the views of those rooms, watchers too; without, a frame of the viewers, to the
/// views that are not watchers.
fn broadcast(doc: &str, except: Option<&str>, rooms: Option<&[String]>, frame: &Json) {
    let targets: Vec<Member> = lock(members())
        .iter()
        .filter(|member| member.doc == doc && except != Some(member.client.as_str()))
        .filter(|member| rooms.is_none_or(|rooms| rooms.contains(&member.room)))
        // a frame of the viewers (sent to every room) is not a watcher's: it has no place among them
        .filter(|member| rooms.is_some() || !member.watch)
        .cloned()
        .collect();
    for member in targets {
        let mut message = frame.clone();
        message["topic"] = Json::String(topic(&member.room));
        send_to(&member, &message);
    }
}

fn publish(factory: &ExecutionContextFactory, doc: &str, frame: &Json) {
    let Some(facade) = factory.entity() else { return };
    let sent = moqui_core::cluster::prepare_message(json!({"doc": doc, "frame": frame}))
        .and_then(|message| moqui_core::cluster::publish(&facade, CALC_PRESENCE, &message));
    if let Err(err) = sent {
        tracing::warn!("CalcGrid presence: not published to the cluster: {}", err.message());
    }
}

/// A frame from a client: true when it was a CalcGrid frame (handled), false for anything else.
pub(super) fn handle(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, text: &str) -> bool {
    if !text.trim_start().starts_with('{') {
        return false;
    }
    let Ok(message) = serde_json::from_str::<Json>(text) else {
        return false;
    };
    let Some(kind) = message.get("calc").and_then(Json::as_str) else {
        return false;
    };
    let field = |name: &str| message.get(name).and_then(Json::as_str).unwrap_or_default().to_string();
    let (room, client) = (field("room"), field("client"));
    if room.is_empty() || client.is_empty() || room.len() > 100 || client.len() > 100 {
        return true;
    }
    match kind {
        "join" => join(
            factory,
            slot,
            room,
            client,
            message.get("watch") == Some(&Json::Bool(true)),
        ),
        "select" => select(factory, &**slot, &room, &client, &message),
        "leave" => leave(factory, slot.id, Some(&client)),
        _ => error(&**slot, &room, &client, 400, &format!("unknown calc message {kind}")),
    }
    true
}

/// The checks a join passes before the room is looked at: a socket's (or a link's) presence is bounded, its joins a
/// minute and its clients in one room (SEC-TLS-113). The place held, or None when an error was sent.
fn join_held(
    factory: &ExecutionContextFactory,
    out: &dyn Out,
    user_id: &str,
    room: &str,
    client: &str,
) -> Option<super::Reserved> {
    let caps = super::PresenceCaps::of(factory);
    let others = lock(members())
        .iter()
        .filter(|member| member.slot_id == out.id() && member.room == room && member.client != client)
        .count();
    // the place is held before the join goes off, so joins in flight together count against the cap
    let Some(reserved) = super::reserve(out.id(), room, others, caps.room_clients) else {
        error(
            out,
            room,
            client,
            429,
            "this connection has as many clients in the room as it may",
        );
        return None;
    };
    if !super::join_allowed(out.id(), user_id, &caps) {
        error(out, room, client, 429, "too many joins; wait a minute");
        return None;
    }
    Some(reserved)
}

/// The document of `room` when `user_id` has the room open, and the user's name: run on a request worker.
fn room_of(factory: &ExecutionContextFactory, user_id: &str, room: &str) -> (Option<String>, String) {
    let doc = together::member_of(factory, user_id, room);
    let name = factory.entity().map_or_else(
        || user_id.to_string(),
        |facade| together::display_name(&facade, user_id),
    );
    (doc, name)
}

/// What [`room_of`] found for a join.
type Found = Result<(Option<String>, String), moqui_core::context::Error>;

fn join(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, room: String, client: String, watch: bool) {
    let Some(reserved) = join_held(factory, &**slot, &slot.user_id, &room, &client) else {
        return;
    };
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        let _reserved = reserved;
        let user_id = slot.user_id.clone();
        let (job_factory, job_room, job_user) = (Arc::clone(&factory), room.clone(), user_id.clone());
        let found = super::on_pool(&factory, move || Ok(room_of(&job_factory, &job_user, &job_room))).await;
        let out: Arc<dyn Out> = slot;
        joined(&factory, &out, found, &user_id, room, client, watch);
    });
}

/// The rest of a join, once the room was looked at for `user_id`: refused (403) unless the user has it open, else the
/// member is in as that user, is told who is there, and the others are told of it.
fn joined(
    factory: &ExecutionContextFactory,
    out: &Arc<dyn Out>,
    found: Found,
    user_id: &str,
    room: String,
    client: String,
    watch: bool,
) {
    let (doc, name) = match found {
        Ok((Some(doc), name)) => (doc, name),
        Ok((None, _)) => return error(&**out, &room, &client, 403, "Open the document first"),
        Err(err) => return error(&**out, &room, &client, 503, err.message()),
    };
    let member = Member {
        slot: Arc::downgrade(out),
        slot_id: out.id(),
        room: room.clone(),
        doc: doc.clone(),
        client: client.clone(),
        user_id: user_id.to_string(),
        name: name.clone(),
        watch,
    };
    if !admit(factory, &**out, member) {
        return;
    }
    if watch {
        // a watcher is told of changes only: no place of its own, and the others are not told of it
        out.send(&json!({"topic": topic(&room), "type": "joined", "client": client, "peers": []}));
        return;
    }
    let presence = Presence::of(factory);
    presence.set(&doc, &client, user_id, &name, None, Json::Null);
    out.send(
        &json!({"topic": topic(&room), "type": "joined", "client": client, "peers": presence.peers(&doc, &client)}),
    );
    let peer = json!({"type": "peer", "client": client, "userId": user_id, "name": name, "joined": true});
    broadcast(&doc, Some(&client), None, &peer);
    publish(factory, &doc, &peer);
}

/// Put `member` among the members; false when the socket closed while the join was worked. Its close found no member
/// to take away then, so the member leaves at once (SEC-TLS-111).
fn admit(factory: &ExecutionContextFactory, out: &dyn Out, member: Member) -> bool {
    let client = member.client.clone();
    {
        let mut all = lock(members());
        // a join again (after a reconnect) replaces the client's place
        all.retain(|known| !(known.client == client && known.slot_id == out.id()));
        all.push(member);
    }
    if out.is_closed() {
        leave(factory, out.id(), Some(&client));
        return false;
    }
    true
}

/// The sink of an in-process terminal link, beside the sockets: a member's frames go to the link's push sink while the
/// link's user is still the one it joined as.
#[cfg_attr(not(feature = "tui"), allow(dead_code))] // the in-process link is its only maker
struct LinkSink {
    id: u64,
    /// The link's user, as its calls last found it.
    user: Arc<Mutex<Option<String>>>,
    deliver: Box<dyn Fn(&Json) + Send + Sync>,
    closed: AtomicBool,
    /// The rooms' frames it may still be handed now, and when that was counted ([`LinkSink::send`]).
    budget: Mutex<(f64, std::time::Instant)>,
    /// The frames dropped since the last warning, and when that warning was.
    dropped: Mutex<(u64, Option<std::time::Instant>)>,
}

#[cfg_attr(not(feature = "tui"), allow(dead_code))]
impl LinkSink {
    fn new(user: Arc<Mutex<Option<String>>>, deliver: Box<dyn Fn(&Json) + Send + Sync>) -> Self {
        Self {
            id: super::NEXT_SOCKET.fetch_add(1, Ordering::Relaxed),
            user,
            deliver,
            closed: AtomicBool::new(false),
            budget: Mutex::new((LINK_FRAMES, std::time::Instant::now())),
            dropped: Mutex::new((0, None)),
        }
    }

    /// Whether the rate leaves room for one more of the rooms' frames now, taking it.
    fn take(&self) -> bool {
        let mut budget = lock(&self.budget);
        let now = std::time::Instant::now();
        let tokens = (budget.0 + now.duration_since(budget.1).as_secs_f64() * LINK_FRAMES).min(LINK_FRAMES);
        let taken = tokens >= 1.0;
        *budget = (if taken { tokens - 1.0 } else { tokens }, now);
        taken
    }

    /// One frame dropped: a warning with the count, at most once a minute while frames go on being dropped.
    fn note_dropped(&self) {
        let mut dropped = lock(&self.dropped);
        dropped.0 += 1;
        if dropped.1.is_none_or(|at| at.elapsed() >= DROPPED_WARNING) {
            tracing::warn!(
                link = self.id,
                dropped = dropped.0,
                "CalcGrid: a terminal's room frames come faster than {LINK_FRAMES} a second; frames were dropped"
            );
            *dropped = (0, Some(std::time::Instant::now()));
        }
    }
}

/// The rooms' frames a link is handed: at most this many a second, in bursts of this many (the size of a socket's
/// queue, `SOCKET_QUEUE`); past it a frame is dropped, as a socket with a full queue drops one. The session takes the
/// frames on an unbounded channel, so this is a bound on their rate, not on the frames waiting there.
const LINK_FRAMES: f64 = super::SOCKET_QUEUE as f64;
/// How often a link's dropped frames are warned of, at most.
const DROPPED_WARNING: std::time::Duration = std::time::Duration::from_secs(60);

/// A frame for the link itself, which no rate drops: the answer to its own join (`joined`, `error`, only ever sent to
/// the joiner), and `reopen`, which the join caps bound already.
fn own_frame(message: &Json) -> bool {
    matches!(message["type"].as_str(), Some("joined" | "error")) || message["reopen"] == true
}

impl Out for LinkSink {
    fn id(&self) -> u64 {
        self.id
    }

    fn user(&self) -> Option<String> {
        lock(&self.user).clone()
    }

    fn send(&self, message: &Json) {
        if self.is_closed() {
            return;
        }
        if own_frame(message) || self.take() {
            (self.deliver)(message);
        } else {
            self.note_dropped();
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

/// The thread that takes a link's frames: not started before the first, its sender while it runs, and failed (or let
/// go) after, so a spawn that failed is not tried again for each frame.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
enum Thread {
    NotYet,
    Running(std::sync::mpsc::Sender<Json>),
    Failed,
}

/// An in-process terminal link's CalcGrid frames (`join`, `select`, `leave`), taken in order on a thread of their own
/// as a socket's are on its task, with the socket's checks: a join only into a room the link's user has open, and the
/// same presence caps, the sink counted as a socket. The thread starts with the first frame. Dropping it leaves every
/// room the link joined.
#[cfg_attr(not(feature = "tui"), allow(dead_code))] // the in-process link is its only maker
pub(crate) struct LinkFrames {
    factory: Weak<ExecutionContextFactory>,
    thread: Mutex<Thread>,
    sink: Arc<LinkSink>,
}

#[cfg_attr(not(feature = "tui"), allow(dead_code))]
impl LinkFrames {
    /// Frames for the link of `user` (the link's own record of its user), whose answers and the rooms' frames go to
    /// `deliver`.
    pub(crate) fn start(
        factory: Weak<ExecutionContextFactory>,
        user: Arc<Mutex<Option<String>>>,
        deliver: Box<dyn Fn(&Json) + Send + Sync>,
    ) -> Self {
        Self {
            factory,
            thread: Mutex::new(Thread::NotYet),
            sink: Arc::new(LinkSink::new(user, deliver)),
        }
    }

    /// The host has the rooms, so the link takes frames.
    pub(crate) fn takes_frames(&self) -> bool {
        true
    }

    /// Queue one frame, starting the thread for the first; false once the link let go or the thread did not start.
    pub(crate) fn send(&self, frame: Json) -> bool {
        let mut thread = lock(&self.thread);
        if matches!(*thread, Thread::NotYet) {
            *thread = self.spawn().map_or(Thread::Failed, Thread::Running);
        }
        match &*thread {
            Thread::Running(tx) => tx.send(frame).is_ok(),
            Thread::NotYet | Thread::Failed => false,
        }
    }

    fn spawn(&self) -> Option<std::sync::mpsc::Sender<Json>> {
        let (tx, rx) = std::sync::mpsc::channel::<Json>();
        let (factory, held) = (Weak::clone(&self.factory), Arc::clone(&self.sink));
        std::thread::Builder::new()
            .name("tui-calc".to_string())
            .spawn(move || {
                for frame in rx {
                    let Some(factory) = factory.upgrade() else {
                        break;
                    };
                    link_frame(&factory, &held, &frame);
                }
                held.closed.store(true, Ordering::Release);
                if let Some(factory) = factory.upgrade() {
                    leave(&factory, held.id, None);
                }
            })
            .map_err(|err| tracing::warn!("CalcGrid frames of the terminal not taken: {err}"))
            .ok()?;
        Some(tx)
    }
}

impl Drop for LinkFrames {
    fn drop(&mut self) {
        // nothing more goes to the link, and its thread leaves the rooms once the frames queued are done
        self.sink.closed.store(true, Ordering::Release);
        *lock(&self.thread) = Thread::Failed;
    }
}

/// A link's join: refused with nobody logged in to the link, else held to the caps and the room looked at for the
/// link's user by `look` (on a request worker, [`room_of`]), as a socket's join is.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
fn link_join(
    factory: &ExecutionContextFactory,
    sink: &Arc<LinkSink>,
    room: String,
    client: String,
    look: impl FnOnce(&str, &str) -> Found,
) {
    let Some(user_id) = sink.user() else {
        return error(&**sink, &room, &client, 401, "Log in first");
    };
    let Some(_reserved) = join_held(factory, &**sink, &user_id, &room, &client) else {
        return;
    };
    let found = look(&user_id, &room);
    let out: Arc<dyn Out> = Arc::clone(sink) as Arc<dyn Out>;
    joined(factory, &out, found, &user_id, room, client, false);
}

/// One frame of an in-process link, as [`handle`] takes a socket's.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
fn link_frame(factory: &Arc<ExecutionContextFactory>, sink: &Arc<LinkSink>, message: &Json) {
    let Some(kind) = message.get("calc").and_then(Json::as_str) else {
        return;
    };
    let field = |name: &str| message.get(name).and_then(Json::as_str).unwrap_or_default().to_string();
    let (room, client) = (field("room"), field("client"));
    if room.is_empty() || client.is_empty() || room.len() > 100 || client.len() > 100 {
        return;
    }
    match kind {
        "join" => link_join(factory, sink, room, client, |user_id, room| {
            let (job_factory, job_room, job_user) = (Arc::clone(factory), room.to_string(), user_id.to_string());
            match factory.request_pool().try_submit(Arc::clone(factory), move || {
                Ok(room_of(&job_factory, &job_user, &job_room))
            }) {
                Ok(rx) => rx
                    .recv()
                    .unwrap_or_else(|_| Err(moqui_core::context::Error::new("request worker dropped the result"))),
                Err(err) => Err(err),
            }
        }),
        "select" => select(factory, &**sink, &room, &client, message),
        "leave" => leave(factory, sink.id, Some(&client)),
        _ => error(&**sink, &room, &client, 400, &format!("unknown calc message {kind}")),
    }
}

fn member(slot_id: u64, client: &str) -> Option<Member> {
    lock(members())
        .iter()
        .find(|member| member.slot_id == slot_id && member.client == client)
        .cloned()
}

fn select(factory: &ExecutionContextFactory, out: &dyn Out, room: &str, client: &str, message: &Json) {
    let Some(member) = member(out.id(), client).filter(|member| {
        member.room == room && !member.watch && out.user().as_deref() == Some(member.user_id.as_str())
    }) else {
        return error(out, room, client, 409, "join the document first");
    };
    let sheet = message
        .get("sheet")
        .and_then(Json::as_u64)
        .and_then(|sheet| u32::try_from(sheet).ok());
    let area = message.get("area").map_or(Json::Null, together::place_of);
    Presence::of(factory).set(&member.doc, client, &member.user_id, &member.name, sheet, area.clone());
    let frame = json!({"type": "select", "client": client, "userId": member.user_id, "name": member.name,
        "sheet": sheet, "area": area});
    broadcast(&member.doc, Some(client), None, &frame);
    publish(factory, &member.doc, &frame);
}

/// A client leaves its document: `client`, or every client of the socket.
fn leave(factory: &ExecutionContextFactory, slot_id: u64, client: Option<&str>) {
    let gone: Vec<Member> = {
        let mut all = lock(members());
        let (gone, kept): (Vec<Member>, Vec<Member>) = all
            .drain(..)
            .partition(|member| member.slot_id == slot_id && client.is_none_or(|client| member.client == client));
        *all = kept;
        gone
    };
    for member in gone {
        gone_peer(factory, &member);
    }
}

/// `member` is no longer a view of its document: the others are told, here and on every server.
fn gone_peer(factory: &ExecutionContextFactory, member: &Member) {
    if member.watch {
        return;
    }
    Presence::of(factory).leave(&member.doc, &member.client);
    let frame = json!({"type": "peer", "client": member.client, "userId": member.user_id, "name": member.name,
        "joined": false});
    broadcast(&member.doc, None, None, &frame);
    publish(factory, &member.doc, &frame);
}

/// The socket closed: its clients leave their documents.
pub(super) fn socket_closed(factory: &ExecutionContextFactory, slot_id: u64) {
    leave(factory, slot_id, None);
}

/// Another server's presence frame: kept here too (a terminal view reads it), and sent to this server's views.
fn deliver_presence(factory: &ExecutionContextFactory, message: &TopicMessage) {
    if message.origin_id == moqui_core::notify::origin_id() {
        return;
    }
    let (Some(doc), Some(frame)) = (message.body["doc"].as_str(), message.body.get("frame")) else {
        return;
    };
    let client = frame["client"].as_str().unwrap_or_default();
    let presence = Presence::of(factory);
    if frame["type"] == "peer" && frame["joined"] == false {
        presence.leave(doc, client);
    } else {
        let sheet = frame["sheet"].as_u64().and_then(|sheet| u32::try_from(sheet).ok());
        presence.set(
            doc,
            client,
            frame["userId"].as_str().unwrap_or_default(),
            frame["name"].as_str().unwrap_or_default(),
            sheet,
            frame["area"].clone(),
        );
    }
    broadcast(doc, Some(client), None, frame);
}

/// An edit, save, refresh or revert of a document on any server, this one too: this server's rooms of it apply it on a
/// request worker, then their views are told; a view whose room was dropped is told to open the document again.
async fn deliver_update(factory: Arc<ExecutionContextFactory>, body: Json) {
    let Some(doc) = body["doc"].as_str().map(str::to_string) else {
        return;
    };
    let views: Vec<(String, String)> = lock(members())
        .iter()
        .filter(|member| member.doc == doc)
        .map(|member| (member.user_id.clone(), member.room.clone()))
        .collect();
    let has_views = !views.is_empty();
    let (job_factory, job_doc) = (Arc::clone(&factory), doc.clone());
    let user_id = body["userId"].as_str().unwrap_or_default().to_string();
    let job_user = user_id.clone();
    let caught = super::on_pool(&factory, move || {
        let (followed, dropped) = together::catch_up_document(&job_factory, &job_doc);
        let name = if has_views {
            job_factory
                .entity()
                .map_or_else(|| job_user.clone(), |facade| together::display_name(&facade, &job_user))
        } else {
            String::new()
        };
        // the views whose person left their room (it is open for others still): one who may no longer read the
        // document, or whose scope changed (service.rs room_of)
        let rooms = moqui_calc::room::Rooms::of(&job_factory);
        let left: Vec<(String, String)> = views
            .into_iter()
            .filter(|(user, room)| !rooms.has_member(user, room))
            .collect();
        Ok((followed, dropped, name, left))
    })
    .await;
    let (followed, dropped, name, left) = match caught {
        Ok(caught) => caught,
        Err(err) => {
            tracing::warn!(document = %doc, "CalcGrid: the rooms did not follow an edit: {}", err.message());
            return;
        }
    };
    let frame = json!({"type": "changed", "seq": body["seq"], "userId": user_id, "name": name, "kind": body["kind"]});
    if !left.is_empty() {
        // told to open the document again, which is refused when they may no longer read it, and no longer a view of
        // the room: its edits and its viewers are not sent to them (DASH-UI-008)
        let mut reopen = frame.clone();
        reopen["reopen"] = Json::Bool(true);
        let gone: Vec<Member> = {
            let mut all = lock(members());
            let (gone, kept): (Vec<Member>, Vec<Member>) = all.drain(..).partition(|member| {
                member.doc == doc
                    && left
                        .iter()
                        .any(|(user, room)| *user == member.user_id && *room == member.room)
            });
            *all = kept;
            gone
        };
        for member in gone {
            let mut message = reopen.clone();
            message["topic"] = Json::String(topic(&member.room));
            send_to(&member, &message);
            gone_peer(&factory, &member);
        }
    }
    // a person's leaving a room changed no cell: nothing more to tell
    if body["kind"] == "left" {
        return;
    }
    let kept: Vec<String> = lock(members())
        .iter()
        .filter(|member| member.doc == doc && !dropped.contains(&member.room))
        .map(|member| member.room.clone())
        .collect();
    // A room that changed values only tells its views which areas, from which edit: a view one edit behind reads
    // those cells again and keeps the rest. Rooms of other scopes changed other cells, so each is told its own.
    let mut told: Vec<String> = Vec::new();
    for room in &followed {
        let Some(changed) = &room.changed else {
            continue;
        };
        if !kept.contains(&room.room) {
            continue;
        }
        let mut own = frame.clone();
        own["from"] = json!(room.from);
        own["changed"] = together::areas_json(changed);
        broadcast(&doc, None, Some(std::slice::from_ref(&room.room)), &own);
        told.push(room.room.clone());
    }
    let rest: Vec<String> = kept.into_iter().filter(|room| !told.contains(room)).collect();
    broadcast(&doc, None, Some(&rest), &frame);
    if !dropped.is_empty() {
        let mut reopen = frame.clone();
        reopen["reopen"] = Json::Bool(true);
        broadcast(&doc, None, Some(&dropped), &reopen);
    }
}

/// Subscribe to the edits and the presence of every server's CalcGrid rooms, until the server stops: the subscriptions
/// are the task's, and the presence listener holds the factory weakly, so a restart releases it. An edit's message is
/// handled on the task, never on the thread that published it: that thread holds the editor's room.
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Json>();
    let updates = moqui_core::notify::subscribe(
        CALC_UPDATE,
        Arc::new(move |message: &TopicMessage| {
            let _ = tx.send(message.body.clone());
        }),
    );
    let weak = Arc::downgrade(factory);
    let presence = moqui_core::notify::subscribe(
        CALC_PRESENCE,
        Arc::new(move |message: &TopicMessage| {
            if let Some(factory) = weak.upgrade() {
                deliver_presence(&factory, message);
            }
        }),
    );
    let subscriptions = match (updates, presence) {
        (Ok(updates), Ok(presence)) => vec![updates, presence],
        (Err(err), _) | (_, Err(err)) => {
            tracing::error!("CalcGrid: the cluster topics were not subscribed: {}", err.message());
            return;
        }
    };
    let weak = Arc::downgrade(factory);
    tokio::spawn(async move {
        let _subscriptions = subscriptions;
        loop {
            tokio::select! {
                body = rx.recv() => match (body, weak.upgrade()) {
                    (Some(body), Some(factory)) => deliver_update(factory, body).await,
                    _ => break,
                },
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weak(slot: &Arc<Slot>) -> Weak<dyn Out> {
        let slot: Arc<dyn Out> = Arc::clone(slot) as Arc<dyn Out>;
        Arc::downgrade(&slot)
    }

    fn slot(user: &str) -> (Arc<Slot>, tokio::sync::mpsc::Receiver<Push>) {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: user.into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        (slot, rx)
    }

    #[test]
    fn a_place_goes_to_the_other_views_of_the_document_on_their_own_rooms_topics_and_a_stranger_is_refused() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (ann, mut ann_rx) = slot("ann");
        let (otto, mut otto_rx) = slot("otto");
        let (vic, mut vic_rx) = slot("vic");
        let doc = "calc-presence-regression";
        for (slot, room, client, name) in [(&ann, "d-west", "a1", "Ann"), (&otto, "d-east", "o1", "Otto")] {
            lock(members()).push(Member {
                slot: weak(slot),
                slot_id: slot.id,
                room: room.into(),
                doc: doc.into(),
                client: client.into(),
                user_id: slot.user_id.clone(),
                name: name.into(),
                watch: false,
            });
        }
        select(
            &factory,
            &*ann,
            "d-west",
            "a1",
            &json!({"sheet": 1, "area": {"row": 2, "column": 3, "width": 1, "height": 1, "value": "not passed on"}}),
        );
        // a client that did not join is refused, and its place goes nowhere
        select(&factory, &*vic, "d-west", "v1", &json!({"sheet": 0}));
        let to_otto: Json = serde_json::from_str(&otto_rx.try_recv().unwrap().text).unwrap();
        assert_eq!(to_otto["topic"], "calc:d-east");
        assert_eq!(to_otto["type"], "select");
        assert_eq!(to_otto["name"], "Ann");
        assert_eq!(to_otto["area"]["column"], 3);
        assert!(to_otto["area"].get("value").is_none());
        assert!(ann_rx.try_recv().is_err(), "not to oneself");
        let refused: Json = serde_json::from_str(&vic_rx.try_recv().unwrap().text).unwrap();
        assert_eq!(refused["code"], 409);
        assert!(otto_rx.try_recv().is_err());
        assert_eq!(Presence::of(&factory).peers(doc, "o1")[0]["name"], "Ann");
        // the socket closed: Ann leaves, and Otto is told
        socket_closed(&factory, ann.id);
        let left: Json = serde_json::from_str(&otto_rx.try_recv().unwrap().text).unwrap();
        assert_eq!(
            (left["type"].as_str(), left["joined"].as_bool()),
            (Some("peer"), Some(false))
        );
        assert!(Presence::of(&factory).peers(doc, "o1").is_empty());
        socket_closed(&factory, otto.id);
    }

    /// DASH-UI-008: a watcher (a dashboard's range panel) is told of the document's changes on its room's topic, but
    /// not of its viewers, and it has no place for them.
    #[test]
    fn a_watcher_is_told_of_changes_but_not_of_the_viewers_and_has_no_place() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (ann, mut ann_rx) = slot("ann-watch");
        let (dash, mut dash_rx) = slot("dash-watch");
        let doc = "calc-watch-regression";
        for (slot, client, name, watch) in [(&ann, "a1", "Ann", false), (&dash, "w1", "Dash", true)] {
            lock(members()).push(Member {
                slot: weak(slot),
                slot_id: slot.id,
                room: "d-watch".into(),
                doc: doc.into(),
                client: client.into(),
                user_id: slot.user_id.clone(),
                name: name.into(),
                watch,
            });
        }
        // Ann's place goes to no watcher; the watcher's own place is refused
        select(
            &factory,
            &*ann,
            "d-watch",
            "a1",
            &json!({"sheet": 0, "area": {"row": 1, "column": 1, "width": 1, "height": 1}}),
        );
        assert!(dash_rx.try_recv().is_err(), "a viewer's place is not a watcher's");
        select(&factory, &*dash, "d-watch", "w1", &json!({"sheet": 0}));
        let refused: Json = serde_json::from_str(&dash_rx.try_recv().unwrap().text).unwrap();
        assert_eq!(refused["code"], 409);
        assert!(ann_rx.try_recv().is_err());
        assert!(
            Presence::of(&factory).peers(doc, "a1").is_empty(),
            "a watcher is not among the viewers"
        );
        // a change of the room's document goes to the watcher on the room's topic
        let rooms = ["d-watch".to_string()];
        broadcast(doc, None, Some(&rooms), &json!({"type": "changed", "seq": 3}));
        let changed: Json = serde_json::from_str(&dash_rx.try_recv().unwrap().text).unwrap();
        assert_eq!(
            (changed["topic"].as_str(), changed["type"].as_str()),
            (Some("calc:d-watch"), Some("changed"))
        );
        // the watcher's leaving tells the viewers nothing
        while ann_rx.try_recv().is_ok() {}
        socket_closed(&factory, dash.id);
        assert!(ann_rx.try_recv().is_err(), "a watcher's leaving is not a viewer's");
        socket_closed(&factory, ann.id);
    }

    /// SEC-TLS-113: a socket keeps at most four clients in one room, its joins in flight counted with them; SEC-TLS-111:
    /// a member admitted after its socket closed leaves at once.
    #[test]
    fn a_sockets_clients_in_a_room_are_bounded_and_a_closed_socket_keeps_no_member() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (ann, mut ann_rx) = slot("ann-cap");
        let member = |client: &str| Member {
            slot: weak(&ann),
            slot_id: ann.id,
            room: "cap-room".into(),
            doc: "cap-doc".into(),
            client: client.into(),
            user_id: "ann-cap".into(),
            name: "Ann".into(),
            watch: false,
        };
        for client in ["a", "b", "c"] {
            lock(members()).push(member(client));
        }
        // a fourth in flight holds the last place
        let held = super::super::reserve(ann.id, "cap-room", 3, 4).expect("the fourth place");
        join(&factory, &ann, "cap-room".into(), "e".into(), false);
        let refused: Json = serde_json::from_str(&ann_rx.try_recv().unwrap().text).unwrap();
        assert_eq!(refused["code"], 429);
        drop(held);
        socket_closed(&factory, ann.id);

        let (gone, gone_rx) = slot("gone");
        drop(gone_rx);
        let late = Member {
            slot: weak(&gone),
            slot_id: gone.id,
            room: "late-room".into(),
            doc: "late-doc".into(),
            client: "g".into(),
            user_id: "gone".into(),
            name: "Gone".into(),
            watch: false,
        };
        assert!(!admit(&factory, &*gone, late));
        assert!(
            super::member(gone.id, "g").is_none(),
            "no member is left of a closed socket"
        );
    }

    /// A sink of an in-process link, for `user`, and the frames it was given.
    fn link_sink(user: Option<&str>) -> (Arc<LinkSink>, Arc<Mutex<Vec<Json>>>) {
        let got = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&got);
        let sink = Arc::new(LinkSink::new(
            Arc::new(Mutex::new(user.map(str::to_string))),
            Box::new(move |frame: &Json| lock(&seen).push(frame.clone())),
        ));
        (sink, got)
    }

    /// The member `client` of the link sink `sink` in `room`, as a join makes it.
    fn link_member(sink: &Arc<LinkSink>, room: &str, client: &str, user: &str) -> Member {
        let out: Arc<dyn Out> = Arc::clone(sink) as Arc<dyn Out>;
        Member {
            slot: Arc::downgrade(&out),
            slot_id: sink.id,
            room: room.into(),
            doc: format!("{room}-doc"),
            client: client.into(),
            user_id: user.into(),
            name: user.into(),
            watch: false,
        }
    }

    /// CG3.4: a link is handed at most 256 of the rooms' frames a second, in bursts of 256, and the rest are dropped;
    /// its own `joined` and `error` and a `reopen` are handed past the bound.
    #[test]
    fn a_link_is_handed_at_most_its_rate_of_frames_but_always_its_own() {
        let queue = super::super::SOCKET_QUEUE;
        let (sink, got) = link_sink(Some("link-burst"));
        for seq in 0..queue * 3 {
            sink.send(&json!({"type": "changed", "seq": seq}));
        }
        let handed = lock(&got).len();
        // a burst, and what came back while it was sent (a slow machine takes longer than a millisecond)
        assert!((queue..2 * queue).contains(&handed), "{handed} handed");
        lock(&got).clear();
        for own in [
            json!({"type": "joined", "client": "c1", "peers": []}),
            json!({"type": "error", "client": "c1", "code": 409}),
            json!({"type": "changed", "seq": 9, "reopen": true}),
        ] {
            sink.send(&own);
        }
        assert_eq!(lock(&got).len(), 3, "its own frames are not dropped");
    }

    /// CG3.4: a link's frames thread starts with its first frame, and letting go of the link leaves its rooms.
    #[test]
    fn letting_go_of_a_link_leaves_its_rooms() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<Json>();
        let seen_tx = Mutex::new(seen_tx);
        let frames = LinkFrames::start(
            Arc::downgrade(&factory),
            Arc::new(Mutex::new(Some("link-leaver".to_string()))),
            Box::new(move |frame: &Json| {
                let _ = lock(&seen_tx).send(frame.clone());
            }),
        );
        assert!(
            matches!(*lock(&frames.thread), Thread::NotYet),
            "no thread before a frame"
        );
        let id = frames.sink.id;
        lock(members()).push(link_member(&frames.sink, "leave-room", "l1", "link-leaver"));
        // a frame starts the thread: a place of a client not joined, refused
        assert!(frames.send(json!({"calc": "select", "room": "leave-room", "client": "l9", "sheet": 0})));
        let refused = seen_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("an answer");
        assert_eq!(refused["code"], 409);
        drop(frames);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while super::member(id, "l1").is_some() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(super::member(id, "l1").is_none(), "the link left its room");
    }

    /// CG3.4: an in-process terminal link takes a room's frames through a sink beside the sockets', with a socket's
    /// checks: no join with nobody logged in (401), nor into a room its user does not have open (403), and no place
    /// before a join (409); a member through the sink is given the room's frames as a socket's member is, and none once
    /// another user is logged in to the link.
    #[test]
    fn a_links_join_is_refused_for_a_room_its_user_does_not_have_open() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let codes = |got: &Arc<Mutex<Vec<Json>>>| -> Vec<i64> {
            lock(got).drain(..).filter_map(|frame| frame["code"].as_i64()).collect()
        };
        let (nobody, got) = link_sink(None);
        link_frame(
            &factory,
            &nobody,
            &json!({"calc": "join", "room": "link-room", "client": "t0"}),
        );
        assert_eq!(codes(&got), [401]);
        // the room looked at as a request worker looks at it (the test's factory has no workers)
        let (stranger, got) = link_sink(Some("link-stranger"));
        link_join(&factory, &stranger, "link-room".into(), "t1".into(), |user, room| {
            Ok(room_of(&factory, user, room))
        });
        assert_eq!(codes(&got), [403], "the room is not open for the link's user");
        assert!(super::member(stranger.id, "t1").is_none(), "and it is no member");
        link_frame(
            &factory,
            &stranger,
            &json!({"calc": "select", "room": "link-room", "client": "t1", "sheet": 0}),
        );
        assert_eq!(codes(&got), [409]);
        // a member through the sink is told of the document's changes on its room's topic
        let (ann, got) = link_sink(Some("link-ann"));
        lock(members()).push(link_member(&ann, "link-room", "a1", "link-ann"));
        let rooms = ["link-room".to_string()];
        broadcast(
            "link-room-doc",
            None,
            Some(&rooms),
            &json!({"type": "changed", "seq": 2}),
        );
        let frames: Vec<Json> = lock(&got).drain(..).collect();
        assert_eq!(
            (frames[0]["topic"].as_str(), frames[0]["type"].as_str()),
            (Some("calc:link-room"), Some("changed"))
        );
        // another user logged in to the link since: the member's frames are not theirs
        *lock(&ann.user) = Some("link-otto".into());
        broadcast(
            "link-room-doc",
            None,
            Some(&rooms),
            &json!({"type": "changed", "seq": 3}),
        );
        assert!(lock(&got).is_empty());
        link_frame(
            &factory,
            &ann,
            &json!({"calc": "select", "room": "link-room", "client": "a1", "sheet": 0}),
        );
        assert_eq!(codes(&got), [409], "nor may they place it");
        socket_closed(&factory, ann.id);
        assert!(super::member(ann.id, "a1").is_none());
    }
}
