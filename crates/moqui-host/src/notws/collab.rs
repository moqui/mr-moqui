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

//! Co-editing on the notification socket: the Yjs sync messages of `m-coedit` (js/README.md, Collaborative editing) for
//! the rooms of `moqui_core::collab`. A client sends JSON frames with `collab` (`join`, `update`, `awareness`, `leave`),
//! and gets frames on the topic `collab:<doc>` (`joined`, `update`, `ack`, `awareness`, `peer`, `saved`, `error`). A
//! join runs on a request worker (the access check and the load); an update is applied in memory at once, acknowledged,
//! sent to the document's other editors on this server and published on the cluster topic `collab-update` for those on
//! the others. A socket that closes leaves its documents. Every second the rooms are written, saved and closed as
//! `moqui_core::collab::tick` decides, and a save is told to the editors.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use base64::Engine;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::{TopicMessage, COLLAB_UPDATE};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::{Push, Slot};

static REMOTE: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();

/// An editor on this server: its socket, its client id, user and presence id.
#[derive(Clone)]
struct Member {
    slot: Weak<Slot>,
    slot_id: u64,
    client: String,
    user_id: String,
    name: String,
    awareness: Json,
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
    let Some(facade) = factory.entity() else { return };
    let message = TopicMessage {
        origin_id: moqui_core::notify::origin_id().to_string(),
        message_id: String::new(),
        body,
    };
    if let Err(err) = moqui_core::cluster::publish(&facade, COLLAB_UPDATE, &message) {
        tracing::warn!("co-editing: not published to the cluster: {}", err.message());
    }
}

/// A frame from a client: true when it was a co-editing message (handled), false for anything else.
pub(super) fn handle(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, text: &str) -> bool {
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
    match kind {
        "join" => join(factory, slot, doc, client, &message),
        "update" => update(factory, slot, &doc, &client, &message),
        "awareness" => awareness(factory, slot, &doc, &client, &message),
        "leave" => leave(factory, slot.id, &doc, Some(&client)),
        _ => error(slot, &doc, &client, 400, &format!("unknown collab message {kind}")),
    }
    true
}

fn join(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, doc: String, client: String, message: &Json) {
    let vector = match b64().decode(message.get("sv").and_then(Json::as_str).unwrap_or_default()) {
        Ok(vector) => vector,
        Err(_) => return error(slot, &doc, &client, 400, "sv is not base64"),
    };
    let awareness = message.get("awareness").cloned().unwrap_or(Json::Null);
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        let user_id = slot.user_id.clone();
        let job_doc = doc.clone();
        let joined = super::on_pool(&factory, move || {
            Ok(moqui_core::collab::join(&job_doc, &user_id, &vector))
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
        };
        let peers: Vec<Json> = {
            let mut all = lock(members());
            let list = all.entry(doc.clone()).or_default();
            // a join again (after a reconnect) replaces the client's place; the room counted it again, so it leaves once
            let had = list
                .iter()
                .position(|known| known.client == client && known.slot_id == slot.id);
            if let Some(index) = had {
                list.remove(index);
                moqui_core::collab::leave(&doc);
            }
            let peers = list
                .iter()
                .map(|peer| json!({"client": peer.client, "userId": peer.user_id, "name": peer.name, "awareness": peer.awareness}))
                .collect();
            list.push(member);
            peers
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

fn is_member(doc: &str, client: &str, slot_id: u64) -> bool {
    lock(members()).get(doc).is_some_and(|list| {
        list.iter()
            .any(|member| member.client == client && member.slot_id == slot_id)
    })
}

fn update(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, doc: &str, client: &str, message: &Json) {
    if !is_member(doc, client, slot.id) {
        return error(slot, doc, client, 409, "join the document first");
    }
    let encoded = message.get("update").and_then(Json::as_str).unwrap_or_default();
    let Ok(update) = b64().decode(encoded) else {
        return error(slot, doc, client, 400, "update is not base64");
    };
    if let Err(refusal) = moqui_core::collab::apply(doc, &slot.user_id, &update) {
        return error(slot, doc, client, refusal.code, &refusal.message);
    }
    if let Some(id) = message.get("id") {
        send(
            slot,
            &json!({"topic": topic(doc), "type": "ack", "client": client, "id": id}),
        );
    }
    let frame = json!({"topic": topic(doc), "type": "update", "client": client, "update": encoded});
    broadcast(doc, Some(client), &frame);
    publish_remote(factory, json!({"doc": doc, "update": encoded, "frame": frame}));
}

fn awareness(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, doc: &str, client: &str, message: &Json) {
    if !is_member(doc, client, slot.id) {
        return;
    }
    let frame = json!({"topic": topic(doc), "type": "awareness", "client": client,
        "update": message.get("update").cloned().unwrap_or(Json::Null)});
    broadcast(doc, Some(client), &frame);
    publish_remote(factory, json!({"doc": doc, "frame": frame}));
}

/// A client leaves a document: `client`, or every client of the socket.
fn leave(factory: &Arc<ExecutionContextFactory>, slot_id: u64, doc: &str, client: Option<&str>) {
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
        moqui_core::collab::leave(doc);
        let frame = json!({"topic": topic(doc), "type": "peer", "client": member.client, "userId": member.user_id, "name": member.name,
            "awareness": member.awareness, "joined": false});
        broadcast(doc, None, &frame);
        publish_remote(factory, json!({"doc": doc, "frame": frame}));
    }
}

/// The socket closed: its clients leave every document.
pub(super) fn socket_closed(factory: &Arc<ExecutionContextFactory>, slot_id: u64) {
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
        .get("update")
        .and_then(Json::as_str)
        .and_then(|text| b64().decode(text).ok())
    {
        moqui_core::collab::apply_remote(doc, &update);
    }
    if let Some(frame) = message.body.get("frame") {
        broadcast(doc, None, frame);
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
            if moqui_core::collab::open_documents().is_empty() {
                continue;
            }
            match super::on_pool(&factory, || {
                moqui_core::collab::tick();
                Ok(moqui_core::collab::take_events())
            })
            .await
            {
                Ok(events) => {
                    for event in events {
                        let moqui_core::collab::Event::Saved {
                            document_key,
                            version_name,
                            user_id,
                            user_name,
                        } = event;
                        let frame = json!({"topic": topic(&document_key), "type": "saved", "versionName": version_name,
                            "userId": user_id, "name": user_name});
                        broadcast(&document_key, None, &frame);
                        publish_remote(&factory, json!({"doc": document_key, "frame": frame}));
                    }
                }
                Err(err) => tracing::warn!("co-editing: the rooms were not written: {}", err.message()),
            }
        }
    });
}
