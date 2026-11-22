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

//! Co-editing on the notification socket: the Yjs sync messages of `m-coedit` (docs/ui/Qvt-Client.md, Collaborative
//! editing) for the rooms of `moqui_collab`. A client sends JSON frames with `collab` (`join`, `update`,
//! `awareness`, `leave`), and gets frames on the topic `collab:<doc>` (`joined`, `update`, `ack`, `awareness`, `peer`,
//! `saved`, `error`). A join runs on a request worker (the access check and the load); an update is applied in memory
//! at once, acknowledged, sent to the document's other editors on this server and published on the cluster topic
//! `collab-update` for those on the others. A socket that closes leaves its documents. Every second the rooms are
//! written, saved and closed as `moqui_collab::tick` decides, and a save is told to the editors; changes another server
//! stored that a room lacked (its relayed update lost) go to the room's editors as an `update` from `server`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use base64::Engine;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::{TopicMessage, COLLAB_RECHECK, COLLAB_UPDATE};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::{Push, Slot};

static REMOTE: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();
static RECHECK: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();

/// An editor on this server: its socket, its client id, user and presence id.
#[derive(Clone)]
struct Member {
    slot: Weak<Slot>,
    slot_id: u64,
    client: String,
    user_id: String,
    name: String,
    awareness: Json,
    meeting: Option<moqui_collab::Meeting>,
}

impl Member {
    fn actor_key(&self) -> String {
        self.meeting
            .as_ref()
            .map_or_else(|| self.user_id.clone(), moqui_collab::Meeting::key)
    }
}

fn members() -> &'static Mutex<HashMap<String, Vec<Member>>> {
    static MEMBERS: OnceLock<Mutex<HashMap<String, Vec<Member>>>> = OnceLock::new();
    MEMBERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn send(slot: &Slot, message: &Json) {
    // a full queue closes the socket the next time a notification finds it full; a collab frame is dropped
    let _ = slot.tx.try_send(Push {
        text: message.to_string(),
        message_id: String::new(),
    });
}

fn topic(doc: &str) -> String {
    format!("collab:{doc}")
}

fn error(slot: &Slot, doc: &str, client: &str, code: u16, message: &str) {
    send(
        slot,
        &json!({"topic": topic(doc), "type": "error", "client": client, "code": code, "message": message}),
    );
}

/// Send `message` to the document's editors on this server, but the client `except`.
fn broadcast(doc: &str, except: Option<&str>, message: &Json) {
    let targets: Vec<Member> = lock(members()).get(doc).cloned().unwrap_or_default();
    for member in targets {
        if except == Some(member.client.as_str()) {
            continue;
        }
        if let Some(slot) = member.slot.upgrade() {
            send(&slot, message);
        }
    }
}

/// Publish to the other servers of the cluster; a single server has no peers and it is a local no-op.
fn publish_remote(factory: &ExecutionContextFactory, body: Json) {
    match remote_message(body) {
        Ok(message) => publish_message(factory, &message),
        Err(err) => tracing::warn!("co-editing: not published to the cluster: {}", err.message()),
    }
}

fn remote_message(body: Json) -> Result<TopicMessage, moqui_core::context::Error> {
    moqui_core::cluster::prepare_message(body)
}

fn publish_message(factory: &ExecutionContextFactory, message: &TopicMessage) {
    let Some(facade) = factory.entity() else { return };
    if let Err(err) = moqui_core::cluster::publish(&facade, COLLAB_UPDATE, message) {
        tracing::warn!("co-editing: not published to the cluster: {}", err.message());
    }
}

/// A frame from a client: true when it was a co-editing message (handled), false for anything else.
pub(super) async fn handle(
    factory: &Arc<ExecutionContextFactory>,
    slot: &Arc<Slot>,
    text: &str,
    guest: Option<&moqui_collab::Meeting>,
) -> bool {
    if !text.trim_start().starts_with('{') {
        return false;
    }
    let Ok(message) = serde_json::from_str::<Json>(text) else {
        return false;
    };
    let Some(kind) = message.get("collab").and_then(Json::as_str) else {
        return false;
    };
    let doc = message
        .get("doc")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let client = message
        .get("client")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    if doc.is_empty() || client.is_empty() || doc.len() > 200 || client.len() > 100 {
        return true;
    }
    let scope = if kind == "join" {
        if let Some(guest) = guest {
            Some(guest.clone())
        } else if let Some(call) = message.get("callSessionId").and_then(Json::as_str) {
            let leg = message.get("callLegId").and_then(Json::as_str).unwrap_or_default();
            if !super::rtc::has_binding(slot.id, call, leg) {
                error(slot, &doc, &client, 403, "join the call first");
                return true;
            }
            Some(moqui_collab::Meeting {
                call_id: call.to_string(),
                leg_id: leg.to_string(),
                user_id: slot.user_id.clone(),
                credential: None,
            })
        } else {
            None
        }
    } else {
        member(&doc, &client, slot.id).and_then(|m| m.meeting)
    };
    if matches!(kind, "awareness" | "update") {
        // the room's grant, which the tick keeps current (moqui_collab::meeting_allows): no service call per frame
        if let Some(scope) = &scope {
            if !moqui_collab::meeting_allows(&doc, &scope.key(), kind == "update") {
                error(slot, &doc, &client, 403, "the call no longer grants this action");
                return true;
            }
        } else if guest.is_some() {
            error(slot, &doc, &client, 403, "join the meeting board first");
            return true;
        }
    }
    match kind {
        "join" => join(factory, slot, doc, client, &message, scope),
        "update" => update(factory, slot, &doc, &client, &message),
        "awareness" => awareness(factory, slot, &doc, &client, &message),
        "leave" => leave(factory, slot.id, &doc, Some(&client)),
        _ => error(slot, &doc, &client, 400, &format!("unknown collab message {kind}")),
    }
    true
}

fn join(
    factory: &Arc<ExecutionContextFactory>,
    slot: &Arc<Slot>,
    doc: String,
    client: String,
    message: &Json,
    meeting: Option<moqui_collab::Meeting>,
) {
    let vector = match b64().decode(message.get("sv").and_then(Json::as_str).unwrap_or_default()) {
        Ok(vector) => vector,
        Err(_) => return error(slot, &doc, &client, 400, "sv is not base64"),
    };
    let awareness = message.get("awareness").cloned().unwrap_or(Json::Null);
    // a socket's presence is bounded: its joins a minute, its clients on one page, an awareness's size (SEC-TLS-113)
    let caps = super::PresenceCaps::of(factory);
    if awareness.to_string().len() > caps.awareness_bytes {
        return error(slot, &doc, &client, 413, "the awareness is too large");
    }
    let others = lock(members()).get(&doc).map_or(0, |list| {
        list.iter()
            .filter(|member| member.slot_id == slot.id && member.client != client)
            .count()
    });
    // the place is held before the join goes off, so joins in flight together count against the cap
    let Some(reserved) = super::reserve(slot.id, &doc, others, caps.room_clients) else {
        return error(
            slot,
            &doc,
            &client,
            429,
            "this connection has as many clients on the page as it may",
        );
    };
    if !super::join_allowed(slot.id, &slot.user_id, &caps) {
        return error(slot, &doc, &client, 429, "too many joins; wait a minute");
    }
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        let _reserved = reserved;
        let user_id = slot.user_id.clone();
        let job_doc = doc.clone();
        let scope = meeting.clone();
        let joined = super::on_pool(&factory, move || {
            Ok(moqui_collab::join_scoped(&job_doc, &user_id, &vector, scope))
        })
        .await;
        let joined = match joined {
            Ok(Ok(joined)) => joined,
            Ok(Err(refusal)) => return error(&slot, &doc, &client, refusal.code, &refusal.message),
            Err(err) => return error(&slot, &doc, &client, 503, err.message()),
        };
        let member = Member {
            slot: Arc::downgrade(&slot),
            slot_id: slot.id,
            client: client.clone(),
            user_id: slot.user_id.clone(),
            name: joined.user_name.clone(),
            awareness: awareness.clone(),
            meeting,
        };
        let Some(peers) = admit(&factory, &slot, &doc, member) else {
            return;
        };
        send(
            &slot,
            &json!({"topic": topic(&doc), "type": "joined", "client": client, "canEdit": joined.can_edit,
                "update": b64().encode(&joined.update), "sv": b64().encode(&joined.state_vector), "peers": peers}),
        );
        let peer = json!({"topic": topic(&doc), "type": "peer", "client": client, "userId": slot.user_id, "name": joined.user_name,
            "awareness": awareness, "joined": true});
        broadcast(&doc, Some(&client), &peer);
        publish_remote(&factory, json!({"doc": doc, "frame": peer}));
    });
}

/// Put `member` among the members of `doc`, and answer the others there; none when the socket closed while the join
/// was worked. Its close found no member to take away then, so the member leaves at once, or the room would count it
/// for good and never close (SEC-TLS-111).
fn admit(factory: &ExecutionContextFactory, slot: &Slot, doc: &str, member: Member) -> Option<Vec<Json>> {
    let client = member.client.clone();
    let peers = {
        let mut all = lock(members());
        let list = all.entry(doc.to_string()).or_default();
        // a join again (after a reconnect) replaces the client's place; the room counted it again, so it leaves once
        let had = list
            .iter()
            .position(|known| known.client == client && known.slot_id == slot.id);
        if let Some(index) = had {
            let replaced = list.remove(index);
            moqui_collab::leave_as(doc, Some(&replaced.actor_key()));
        }
        let peers = list
            .iter()
            .map(|peer| json!({"client": peer.client, "userId": peer.user_id, "name": peer.name, "awareness": peer.awareness}))
            .collect();
        list.push(member);
        peers
    };
    if slot.tx.is_closed() {
        leave(factory, slot.id, doc, Some(&client));
        return None;
    }
    Some(peers)
}

fn member(doc: &str, client: &str, slot_id: u64) -> Option<Member> {
    lock(members())
        .get(doc)?
        .iter()
        .find(|m| m.client == client && m.slot_id == slot_id)
        .cloned()
}

/// Credentials and room membership may change after a frame was queued, so a board frame leaves the socket only while
/// one of the socket's memberships of the document still allows it: the one the frame names by `client` when it is
/// the socket's, else any. A meeting membership is answered from the room's grant in memory
/// (`moqui_collab::meeting_allows`, kept current by the tick and `collab-recheck`); a wiki one is the user's, rechecked
/// by the tick, and never a guest's.
pub(super) fn may_deliver(slot: &Slot, text: &str, guest: bool) -> bool {
    if !text.contains("collab:") {
        return true;
    }
    let Ok(frame) = serde_json::from_str::<Json>(text) else {
        return true;
    };
    let Some(doc) = frame
        .get("topic")
        .and_then(Json::as_str)
        .and_then(|topic| topic.strip_prefix("collab:"))
    else {
        return true;
    };
    if frame["type"] == "error" {
        return true;
    }
    let mine: Vec<Member> = lock(members())
        .get(doc)
        .map(|list| list.iter().filter(|m| m.slot_id == slot.id).cloned().collect())
        .unwrap_or_default();
    deliverable(&mine, frame.get("client").and_then(Json::as_str), guest, |scope| {
        moqui_collab::meeting_allows(doc, &scope.key(), false)
    })
}

/// Whether a frame for the socket's memberships `mine` of a document may go out: judged by the membership whose client
/// the frame names, when the socket has it, else by any of them.
fn deliverable(
    mine: &[Member],
    client: Option<&str>,
    guest: bool,
    allows: impl Fn(&moqui_collab::Meeting) -> bool,
) -> bool {
    let allowed = |member: &Member| match &member.meeting {
        Some(scope) => allows(scope),
        None => !guest,
    };
    match mine.iter().find(|member| client == Some(member.client.as_str())) {
        Some(named) => allowed(named),
        None => mine.iter().any(allowed),
    }
}

/// Refused updates in a row past which a socket's clients leave the page.
const REFUSED_IN_A_ROW: u32 = 5;

/// Whether the socket may send one more update (`update`) or awareness frame this minute: a token bucket per socket
/// (`PresenceCaps`, SEC-TLS-113).
fn within_rate(caps: &super::PresenceCaps, slot_id: u64, update: bool) -> bool {
    static UPDATES: OnceLock<Mutex<HashMap<u64, super::Bucket>>> = OnceLock::new();
    static AWARENESS: OnceLock<Mutex<HashMap<u64, super::Bucket>>> = OnceLock::new();
    if update {
        super::take(UPDATES.get_or_init(Default::default), &slot_id, caps.updates_per_minute)
    } else {
        super::take(
            AWARENESS.get_or_init(Default::default),
            &slot_id,
            caps.awareness_per_minute,
        )
    }
}

/// The socket's updates refused in a row, by document; one accepted puts it back to none.
fn refused_in_a_row() -> &'static Mutex<HashMap<(u64, String), u32>> {
    static REFUSED: OnceLock<Mutex<HashMap<(u64, String), u32>>> = OnceLock::new();
    REFUSED.get_or_init(Default::default)
}

/// An update over the socket's rate: refused (429), and at the fifth in a row its clients leave the page, told why.
fn refuse_rate(factory: &ExecutionContextFactory, slot: &Slot, doc: &str, client: &str) {
    let count = {
        let mut refused = lock(refused_in_a_row());
        let count = refused.entry((slot.id, doc.to_string())).or_insert(0);
        *count += 1;
        *count
    };
    if count < REFUSED_IN_A_ROW {
        return error(slot, doc, client, 429, "too many updates; slow down");
    }
    lock(refused_in_a_row()).remove(&(slot.id, doc.to_string()));
    error(slot, doc, client, 429, "too many updates; join the page again");
    leave(factory, slot.id, doc, None);
}

fn update(factory: &ExecutionContextFactory, slot: &Slot, doc: &str, client: &str, message: &Json) {
    let Some(member) = member(doc, client, slot.id) else {
        return error(slot, doc, client, 409, "join the document first");
    };
    if !within_rate(&super::PresenceCaps::of(factory), slot.id, true) {
        return refuse_rate(factory, slot, doc, client);
    }
    let encoded = message.get("update").and_then(Json::as_str).unwrap_or_default();
    let frame = json!({"topic": topic(doc), "type": "update", "client": client, "update": encoded});
    let remote = match remote_message(json!({"doc": doc, "frame": frame})) {
        Ok(message) => message,
        Err(err) => return error(slot, doc, client, 413, err.message()),
    };
    let Ok(update) = b64().decode(encoded) else {
        return error(slot, doc, client, 400, "update is not base64");
    };
    if let Err(refusal) = moqui_collab::apply(doc, &member.actor_key(), &update) {
        return error(slot, doc, client, refusal.code, &refusal.message);
    }
    lock(refused_in_a_row()).remove(&(slot.id, doc.to_string()));
    if let Some(id) = message.get("id") {
        send(
            slot,
            &json!({"topic": topic(doc), "type": "ack", "client": client, "id": id}),
        );
    }
    broadcast(doc, Some(client), &frame);
    publish_message(factory, &remote);
}

fn awareness(factory: &ExecutionContextFactory, slot: &Slot, doc: &str, client: &str, message: &Json) {
    if member(doc, client, slot.id).is_none() {
        return;
    }
    let caps = super::PresenceCaps::of(factory);
    // a cursor's moves past the socket's rate are dropped: the next one carries where it is
    if !within_rate(&caps, slot.id, false) {
        return;
    }
    // an awareness goes to every editor: held to its size (SEC-TLS-113)
    if message
        .get("update")
        .is_some_and(|update| update.to_string().len() > caps.awareness_bytes)
    {
        return error(slot, doc, client, 413, "the awareness is too large");
    }
    let frame = json!({"topic": topic(doc), "type": "awareness", "client": client,
        "update": message.get("update").cloned().unwrap_or(Json::Null)});
    broadcast(doc, Some(client), &frame);
    publish_remote(factory, json!({"doc": doc, "frame": frame}));
}

/// A client leaves a document: `client`, or every client of the socket.
fn leave(factory: &ExecutionContextFactory, slot_id: u64, doc: &str, client: Option<&str>) {
    let left: Vec<Member> = {
        let mut all = lock(members());
        let Some(list) = all.get_mut(doc) else { return };
        let (gone, kept): (Vec<Member>, Vec<Member>) = list
            .drain(..)
            .partition(|member| member.slot_id == slot_id && client.is_none_or(|client| member.client == client));
        *list = kept;
        if list.is_empty() {
            all.remove(doc);
        }
        gone
    };
    for member in left {
        moqui_collab::leave_as(doc, Some(&member.actor_key()));
        let frame = json!({"topic": topic(doc), "type": "peer", "client": member.client, "userId": member.user_id, "name": member.name,
            "awareness": member.awareness, "joined": false});
        broadcast(doc, None, &frame);
        publish_remote(factory, json!({"doc": doc, "frame": frame}));
    }
}

/// The socket closed: its clients leave every document.
pub(super) fn socket_closed(factory: &ExecutionContextFactory, slot_id: u64) {
    lock(refused_in_a_row()).retain(|(slot, _), _| *slot != slot_id);
    let docs: Vec<String> = lock(members())
        .iter()
        .filter(|(_, list)| list.iter().any(|member| member.slot_id == slot_id))
        .map(|(doc, _)| doc.clone())
        .collect();
    for doc in docs {
        leave(factory, slot_id, &doc, None);
    }
}

/// Another server's frame: its update into this server's room, and the frame to this server's editors.
fn deliver_remote(message: &TopicMessage) {
    if message.origin_id == moqui_core::notify::origin_id() {
        return;
    }
    let Some(doc) = message.body.get("doc").and_then(Json::as_str) else {
        return;
    };
    if let Some(update) = message
        .body
        .get("frame")
        .filter(|frame| frame.get("type").and_then(Json::as_str) == Some("update"))
        .and_then(|frame| frame.get("update"))
        .and_then(Json::as_str)
        .and_then(|text| b64().decode(text).ok())
    {
        moqui_collab::apply_remote(doc, &update);
    }
    if let Some(frame) = message.body.get("frame") {
        broadcast(doc, None, frame);
    }
}

/// What a tick of the rooms did, to their editors: a save to those on every server; the changes another server stored
/// that a room here lacked (an update relayed to this server and lost, plans/archive/High-Level-Review.md HL-018) to this
/// server's editors only, as every server reads the rows itself.
pub(crate) fn deliver_events(factory: &ExecutionContextFactory, events: Vec<moqui_collab::Event>) {
    for event in events {
        match event {
            moqui_collab::Event::Saved {
                document_key,
                version_name,
                user_id,
                user_name,
            } => {
                let frame = json!({"topic": topic(&document_key), "type": "saved", "versionName": version_name,
                    "userId": user_id, "name": user_name});
                broadcast(&document_key, None, &frame);
                publish_remote(factory, json!({"doc": document_key, "frame": frame}));
            }
            moqui_collab::Event::Update { document_key, update } => {
                let frame = json!({"topic": topic(&document_key), "type": "update", "client": "server",
                    "update": b64().encode(&update)});
                broadcast(&document_key, None, &frame);
            }
            // the user may no longer view the page: each client of theirs leaves it, told why (SEC-TLS-112)
            moqui_collab::Event::Revoked { document_key, user_id } => {
                let slots: Vec<(u64, String, std::sync::Weak<Slot>)> = lock(members())
                    .get(&document_key)
                    .map(|list| {
                        list.iter()
                            .filter(|member| member.actor_key() == user_id)
                            .map(|member| (member.slot_id, member.client.clone(), member.slot.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                for (slot_id, client, slot) in slots {
                    if let Some(slot) = slot.upgrade() {
                        error(&slot, &document_key, &client, 403, "you may no longer view this page");
                    }
                    leave(factory, slot_id, &document_key, Some(&client));
                }
            }
            // the right to change the page changed and the view stays: each client of the user is told (SEC-TLS-112)
            moqui_collab::Event::CanEdit {
                document_key,
                user_id,
                can_edit,
            } => {
                let slots: Vec<(String, std::sync::Weak<Slot>)> = lock(members())
                    .get(&document_key)
                    .map(|list| {
                        list.iter()
                            .filter(|member| member.actor_key() == user_id)
                            .map(|member| (member.client.clone(), member.slot.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                for (client, slot) in slots {
                    if let Some(slot) = slot.upgrade() {
                        send(
                            &slot,
                            &json!({"topic": topic(&document_key), "type": "access", "client": client,
                                "canEdit": can_edit}),
                        );
                    }
                }
            }
        }
    }
}

/// Subscribe to the other servers' frames, and start the tick that writes, saves and closes the rooms.
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    if REMOTE.get().is_none() {
        match moqui_core::notify::subscribe(COLLAB_UPDATE, Arc::new(deliver_remote)) {
            Ok(subscription) => {
                let _ = REMOTE.set(subscription);
            }
            Err(err) => tracing::error!("co-editing: the cluster topic was not subscribed: {}", err.message()),
        }
    }
    if RECHECK.get().is_none() {
        // a call changed on some server: its meeting grants here are asked again at the next tick
        let recheck = |message: &TopicMessage| {
            if let Some(call) = message.body.get("callSessionId").and_then(Json::as_str) {
                moqui_collab::recheck_meeting(call);
            }
        };
        match moqui_core::notify::subscribe(COLLAB_RECHECK, Arc::new(recheck)) {
            Ok(subscription) => {
                let _ = RECHECK.set(subscription);
            }
            Err(err) => tracing::error!("co-editing: the recheck topic was not subscribed: {}", err.message()),
        }
    }
    let factory = Arc::clone(factory);
    tokio::spawn(async move {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = every.tick() => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
            if moqui_collab::open_documents().is_empty() {
                continue;
            }
            match super::on_pool(&factory, || {
                moqui_collab::tick();
                Ok(moqui_collab::take_events())
            })
            .await
            {
                Ok(events) => deliver_events(&factory, events),
                Err(err) => tracing::warn!("co-editing: the rooms were not written: {}", err.message()),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_oversized_update_is_refused_before_apply_ack_or_broadcast() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: "editor".into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        let doc = "oversized-regression";
        lock(members()).insert(
            doc.into(),
            vec![Member {
                slot: Arc::downgrade(&slot),
                slot_id: slot.id,
                client: "client".into(),
                user_id: "editor".into(),
                name: "Editor".into(),
                awareness: Json::Null,
                meeting: None,
            }],
        );
        update(
            &factory,
            &slot,
            doc,
            "client",
            &json!({"id":"ack", "update":"A".repeat(moqui_core::cluster::MAX_CLUSTER_BODY)}),
        );
        lock(members()).remove(doc);
        let response: Json = serde_json::from_str(&rx.try_recv().unwrap().text).unwrap();
        assert_eq!(response["type"], "error");
        assert_eq!(response["code"], 413);
        assert!(rx.try_recv().is_err(), "no acknowledgement or update was broadcast");
        let small = remote_message(json!({"doc":"document", "frame":{"type":"update", "update":"AAAA"}})).unwrap();
        assert!(small.body.get("update").is_none(), "the update is carried once");
        assert_eq!(small.body["frame"]["update"], "AAAA");
    }

    /// SEC-TLS-113: a socket keeps at most four clients on one page, and an awareness over 4 KiB goes to no one; both
    /// are refused before the room or the other editors see them.
    #[test]
    fn a_sockets_clients_on_a_page_and_its_awareness_are_bounded() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: "editor".into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        let doc = "presence-bounds";
        let member = |client: &str| Member {
            slot: Arc::downgrade(&slot),
            slot_id: slot.id,
            client: client.into(),
            user_id: "editor".into(),
            name: "Editor".into(),
            awareness: Json::Null,
            meeting: None,
        };
        lock(members()).insert(doc.into(), ["a", "b", "c", "d"].map(member).into());
        join(&factory, &slot, doc.into(), "e".into(), &json!({"sv": ""}), None);
        let refused: Json = serde_json::from_str(&rx.try_recv().unwrap().text).unwrap();
        assert_eq!(refused["code"], 429);
        awareness(
            &factory,
            &slot,
            doc,
            "a",
            &json!({"update": {"cursor": "x".repeat(5000)}}),
        );
        lock(members()).remove(doc);
        let refused: Json = serde_json::from_str(&rx.try_recv().unwrap().text).unwrap();
        assert_eq!(refused["code"], 413);
        assert!(rx.try_recv().is_err(), "nothing else was sent");
    }

    /// SEC-TLS-112: a lost edit right is told to the user's clients, and a revoked user's clients are told and leave.
    #[test]
    fn a_revoked_users_clients_are_told_and_leave() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: "lost".into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        let doc = "revoked-in-host";
        lock(members()).insert(
            doc.into(),
            vec![Member {
                slot: Arc::downgrade(&slot),
                slot_id: slot.id,
                client: "l".into(),
                user_id: "lost".into(),
                name: "Lost".into(),
                awareness: Json::Null,
                meeting: None,
            }],
        );
        deliver_events(
            &factory,
            vec![moqui_collab::Event::CanEdit {
                document_key: doc.into(),
                user_id: "lost".into(),
                can_edit: false,
            }],
        );
        let told: Json = serde_json::from_str(&rx.try_recv().unwrap().text).unwrap();
        assert_eq!(told["type"], "access");
        assert_eq!(told["canEdit"], false);
        deliver_events(
            &factory,
            vec![moqui_collab::Event::Revoked {
                document_key: doc.into(),
                user_id: "lost".into(),
            }],
        );
        let refused: Json = serde_json::from_str(&rx.try_recv().unwrap().text).unwrap();
        assert_eq!(refused["code"], 403);
        assert!(lock(members()).get(doc).is_none(), "the revoked user's clients left");
    }

    /// SEC-TLS-111: a join worked after its socket closed leaves no member, so the page's room can close.
    #[test]
    fn a_member_admitted_after_its_socket_closed_leaves_at_once() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        drop(rx);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: "closer".into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        let doc = "closed-before-joined";
        let member = Member {
            slot: Arc::downgrade(&slot),
            slot_id: slot.id,
            client: "c".into(),
            user_id: "closer".into(),
            name: "Closer".into(),
            awareness: Json::Null,
            meeting: None,
        };
        assert!(admit(&factory, &slot, doc, member).is_none());
        assert!(
            lock(members()).get(doc).is_none(),
            "no member is left of a closed socket"
        );
    }

    /// SEC-TLS-111: the socket's end closes its receiver before it lets the members go, so a join that admits its member
    /// after that (while the socket task still holds the receiver) finds the socket closed and leaves.
    #[test]
    fn a_join_admitted_while_its_socket_ends_finds_it_closed() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: "ender".into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        super::super::end_socket(&factory, slot.id, &mut rx);
        let doc = "joined-while-ending";
        let member = Member {
            slot: Arc::downgrade(&slot),
            slot_id: slot.id,
            client: "c".into(),
            user_id: "ender".into(),
            name: "Ender".into(),
            awareness: Json::Null,
            meeting: None,
        };
        assert!(admit(&factory, &slot, doc, member).is_none());
        assert!(
            lock(members()).get(doc).is_none(),
            "the receiver still held, no member is kept"
        );
        drop(rx);
    }

    /// SEC-TLS-113: a socket's updates and awareness frames a minute are each held to their rate, and at the fifth
    /// refused update in a row its clients leave the page.
    #[test]
    fn a_socket_over_its_update_rate_is_refused_then_leaves_the_page() {
        let context = ExecutionContextFactory::test_context();
        let factory = Arc::clone(context.factory());
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let slot = Arc::new(Slot {
            id: super::super::NEXT_SOCKET.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            user_id: "flooder".into(),
            session_id: "session".into(),
            topics: Mutex::new(Default::default()),
            tx,
            overflow: tokio::sync::Notify::new(),
        });
        let mut caps = super::super::PresenceCaps::of(&factory);
        caps.updates_per_minute = 2;
        caps.awareness_per_minute = 1;
        assert!(within_rate(&caps, slot.id, true) && within_rate(&caps, slot.id, true));
        assert!(!within_rate(&caps, slot.id, true), "the third update in the minute");
        assert!(within_rate(&caps, slot.id, false), "awareness has its own bucket");
        assert!(!within_rate(&caps, slot.id, false));

        let doc = "flooded";
        lock(members()).insert(
            doc.into(),
            vec![Member {
                slot: Arc::downgrade(&slot),
                slot_id: slot.id,
                client: "f".into(),
                user_id: "flooder".into(),
                name: "Flooder".into(),
                awareness: Json::Null,
                meeting: None,
            }],
        );
        for _ in 1..REFUSED_IN_A_ROW {
            refuse_rate(&factory, &slot, doc, "f");
        }
        assert!(lock(members()).get(doc).is_some(), "still on the page before the fifth");
        refuse_rate(&factory, &slot, doc, "f");
        assert!(
            lock(members()).get(doc).is_none(),
            "the fifth in a row takes it off the page"
        );
        let mut codes = Vec::new();
        while let Ok(push) = rx.try_recv() {
            codes.push(serde_json::from_str::<Json>(&push.text).unwrap()["code"].clone());
        }
        assert_eq!(codes, vec![json!(429); REFUSED_IN_A_ROW as usize]);
    }

    /// A socket with two memberships of one board: a frame naming one of its clients is judged by that membership, a
    /// frame naming another socket's client goes out while any of them allows it, and a guest socket's wiki membership
    /// allows nothing.
    #[test]
    fn a_frame_is_judged_by_the_membership_it_names_else_by_any() {
        let meeting = |leg: &str| moqui_collab::Meeting {
            call_id: "call".into(),
            leg_id: leg.into(),
            user_id: "member".into(),
            credential: None,
        };
        let member = |client: &str, scope: Option<moqui_collab::Meeting>| Member {
            slot: Weak::new(),
            slot_id: 1,
            client: client.into(),
            user_id: "member".into(),
            name: "Member".into(),
            awareness: Json::Null,
            meeting: scope,
        };
        let mine = [
            member("old", Some(meeting("left"))),
            member("new", Some(meeting("kept"))),
        ];
        let allows = |scope: &moqui_collab::Meeting| scope.leg_id == "kept";
        assert!(
            !deliverable(&mine, Some("old"), false, allows),
            "the named membership left"
        );
        assert!(deliverable(&mine, Some("new"), false, allows));
        assert!(
            deliverable(&mine, Some("elsewhere"), false, allows),
            "another membership allows it"
        );
        assert!(deliverable(&mine, None, false, allows));
        assert!(
            !deliverable(&mine[..1], Some("elsewhere"), false, allows),
            "no membership allows it"
        );
        assert!(!deliverable(&[], None, false, allows), "no membership");
        let wiki = [member("wiki", None)];
        assert!(deliverable(&wiki, None, false, allows));
        assert!(
            !deliverable(&wiki, None, true, allows),
            "a guest socket has no wiki membership"
        );
    }
}
