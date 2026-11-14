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

use std::sync::{Arc, Mutex, OnceLock, Weak};

use moqui_calc::together::{self, Presence};
use moqui_core::context::ExecutionContextFactory;
use moqui_core::notify::{TopicMessage, CALC_PRESENCE, CALC_UPDATE};
use moqui_lib::sync::lock;
use serde_json::{json, Value as Json};

use super::{Push, Slot};

/// A view on this server: its socket, its room and the room's document (the presence key), its client id and user.
#[derive(Clone)]
struct Member {
    slot: Weak<Slot>,
    slot_id: u64,
    room: String,
    doc: String,
    client: String,
    user_id: String,
    name: String,
}

fn members() -> &'static Mutex<Vec<Member>> {
    static MEMBERS: OnceLock<Mutex<Vec<Member>>> = OnceLock::new();
    MEMBERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn send(slot: &Slot, message: &Json) {
    // a full queue closes the socket the next time a notification finds it full; a calc frame is dropped
    let _ = slot.tx.try_send(Push {
        text: message.to_string(),
        message_id: String::new(),
    });
}

fn topic(room: &str) -> String {
    format!("calc:{room}")
}

fn error(slot: &Slot, room: &str, client: &str, code: u16, message: &str) {
    send(
        slot,
        &json!({"topic": topic(room), "type": "error", "client": client, "code": code, "message": message}),
    );
}

/// Send `frame` to the views of the document `doc` on this server, each on its own room's topic, but the client
/// `except`; with `rooms`, only to the views of those rooms.
fn broadcast(doc: &str, except: Option<&str>, rooms: Option<&[String]>, frame: &Json) {
    let targets: Vec<Member> = lock(members())
        .iter()
        .filter(|member| member.doc == doc && except != Some(member.client.as_str()))
        .filter(|member| rooms.is_none_or(|rooms| rooms.contains(&member.room)))
        .cloned()
        .collect();
    for member in targets {
        if let Some(slot) = member.slot.upgrade() {
            let mut message = frame.clone();
            message["topic"] = Json::String(topic(&member.room));
            send(&slot, &message);
        }
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
        "join" => join(factory, slot, room, client),
        "select" => select(factory, slot, &room, &client, &message),
        "leave" => leave(factory, slot.id, Some(&client)),
        _ => error(slot, &room, &client, 400, &format!("unknown calc message {kind}")),
    }
    true
}

fn join(factory: &Arc<ExecutionContextFactory>, slot: &Arc<Slot>, room: String, client: String) {
    let factory = Arc::clone(factory);
    let slot = Arc::clone(slot);
    tokio::spawn(async move {
        let user_id = slot.user_id.clone();
        let (job_factory, job_room) = (Arc::clone(&factory), room.clone());
        let found = super::on_pool(&factory, move || {
            let doc = together::member_of(&job_factory, &user_id, &job_room);
            let name = job_factory
                .entity()
                .map_or_else(|| user_id.clone(), |facade| together::display_name(&facade, &user_id));
            Ok((doc, name))
        })
        .await;
        let (doc, name) = match found {
            Ok((Some(doc), name)) => (doc, name),
            Ok((None, _)) => return error(&slot, &room, &client, 403, "Open the document first"),
            Err(err) => return error(&slot, &room, &client, 503, err.message()),
        };
        {
            let mut all = lock(members());
            // a join again (after a reconnect) replaces the client's place
            all.retain(|member| !(member.client == client && member.slot_id == slot.id));
            all.push(Member {
                slot: Arc::downgrade(&slot),
                slot_id: slot.id,
                room: room.clone(),
                doc: doc.clone(),
                client: client.clone(),
                user_id: slot.user_id.clone(),
                name: name.clone(),
            });
        }
        let presence = Presence::of(&factory);
        presence.set(&doc, &client, &slot.user_id, &name, None, Json::Null);
        send(
            &slot,
            &json!({"topic": topic(&room), "type": "joined", "client": client, "peers": presence.peers(&doc, &client)}),
        );
        let peer = json!({"type": "peer", "client": client, "userId": slot.user_id, "name": name, "joined": true});
        broadcast(&doc, Some(&client), None, &peer);
        publish(&factory, &doc, &peer);
    });
}

fn member(slot_id: u64, client: &str) -> Option<Member> {
    lock(members())
        .iter()
        .find(|member| member.slot_id == slot_id && member.client == client)
        .cloned()
}

fn select(factory: &ExecutionContextFactory, slot: &Slot, room: &str, client: &str, message: &Json) {
    let Some(member) = member(slot.id, client).filter(|member| member.room == room) else {
        return error(slot, room, client, 409, "join the document first");
    };
    let sheet = message
        .get("sheet")
        .and_then(Json::as_u64)
        .and_then(|sheet| u32::try_from(sheet).ok());
    // a place is four numbers; anything else of the client's is not passed on
    let area = message.get("area").map_or(Json::Null, |area| {
        json!({"row": area["row"].as_i64(), "column": area["column"].as_i64(),
            "width": area["width"].as_i64(), "height": area["height"].as_i64()})
    });
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
    let presence = Presence::of(factory);
    for member in gone {
        presence.leave(&member.doc, &member.client);
        let frame = json!({"type": "peer", "client": member.client, "userId": member.user_id, "name": member.name,
            "joined": false});
        broadcast(&member.doc, None, None, &frame);
        publish(factory, &member.doc, &frame);
    }
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
    let has_views = lock(members()).iter().any(|member| member.doc == doc);
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
        Ok((followed, dropped, name))
    })
    .await;
    let (followed, dropped, name) = match caught {
        Ok(caught) => caught,
        Err(err) => {
            tracing::warn!(document = %doc, "CalcGrid: the rooms did not follow an edit: {}", err.message());
            return;
        }
    };
    let frame = json!({"type": "changed", "seq": body["seq"], "userId": user_id, "name": name, "kind": body["kind"]});
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
                slot: Arc::downgrade(slot),
                slot_id: slot.id,
                room: room.into(),
                doc: doc.into(),
                client: client.into(),
                user_id: slot.user_id.clone(),
                name: name.into(),
            });
        }
        select(
            &factory,
            &ann,
            "d-west",
            "a1",
            &json!({"sheet": 1, "area": {"row": 2, "column": 3, "width": 1, "height": 1, "value": "not passed on"}}),
        );
        // a client that did not join is refused, and its place goes nowhere
        select(&factory, &vic, "d-west", "v1", &json!({"sheet": 0}));
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
}
