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

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;
use crate::offline::DEFAULT_COMMS_ROOT;
use crate::store::{ChatEvent, ChatRoom, EventPage, MailMessage, MailThread, SyncCursor, ROOM_CURSOR};

const CHAT: &str = "/apps/BasicComms/Chat";
const MAIL: &str = "/apps/BasicComms/Inbox/Mail";

/// A server of fixed answers that notes what was asked; anything else is a 404.
#[derive(Default)]
struct Server {
    answers: RefCell<HashMap<String, Fetched>>,
    asked: RefCell<Vec<String>>,
}

impl Server {
    fn answer(&self, path: &str, value: Value) {
        self.answers
            .borrow_mut()
            .insert(path.to_string(), Fetched::Json(value.to_string().into_bytes()));
    }

    fn asked(&self, path: &str) -> bool {
        self.asked.borrow().iter().any(|asked| asked == path)
    }
}

impl Transport for Server {
    fn get(&self, path: &str) -> Fetched {
        self.asked.borrow_mut().push(path.to_string());
        self.answers.borrow().get(path).cloned().unwrap_or(Fetched::Failed(404))
    }
}

struct Held(Mutex<Store>);

impl StoreAccess for Held {
    fn with_store(&self, work: &mut dyn FnMut(&Store)) -> bool {
        work(&self.0.lock().unwrap());
        true
    }
}

fn held(name: &str) -> (Held, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-client-feeds-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Held(Mutex::new(Store::open(&dir, "a", 1 << 24).unwrap())), dir)
}

fn sync4() -> ServerSync {
    ServerSync {
        client_api: 4,
        ..ServerSync::default()
    }
}

fn feeds(server: &Server, store: &Held, sync: ServerSync) -> Synced {
    feeds_of(server, store, sync, Scope::All)
}

fn feeds_of(server: &Server, store: &Held, sync: ServerSync, scope: Scope) -> Synced {
    run(&Job {
        transport: server,
        store,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        server: sync,
        cancelled: &|| false,
        pause: Duration::ZERO,
        scope,
    })
}

fn room(store: &Store, id: &str, row: Value) {
    store
        .put_chat_room(&ChatRoom {
            chat_room_id: id.to_string(),
            row: row.to_string(),
            last_seq: 0,
            sort_ms: 0,
        })
        .unwrap();
}

/// A room R1 with events 1..=5 held whole up to the head.
fn room_with_events(store: &Store) {
    room(
        store,
        "R1",
        json!({ "chatRoomId": "R1", "lastMessage": { "eventSeq": 5 }, "unreadCount": 3, "mentionCount": 1 }),
    );
    let events: Vec<ChatEvent> = (1..=5)
        .map(|seq| ChatEvent {
            chat_room_id: "R1".to_string(),
            event_seq: seq,
            event: json!({ "eventSeq": seq, "type": "message", "body": format!("m{seq}") }).to_string(),
            thread_seq: None,
        })
        .collect();
    let page = EventPage {
        limit: 50,
        ..EventPage::default()
    };
    store.put_chat_page("R1", &page, &events, false, false).unwrap();
}

fn changes(list: Value, last: i64) -> Value {
    json!({ "changes": list, "lastSeq": last, "more": false, "reset": false, "heads": [], "headsSince": 77 })
}

#[test]
fn a_server_below_client_api_4_reads_no_feed() {
    let (store, dir) = held("md4");
    let server = Server::default();
    let synced = feeds(&server, &store, ServerSync::default());
    assert_eq!(synced.requests, 0);
    assert!(server.asked.borrow().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_first_read_takes_the_cursors_and_has_the_mirror_read() {
    let (store, dir) = held("first");
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200"),
        json!({ "changes": [], "lastSeq": 12, "reset": true }),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200"),
        json!({ "changes": [], "lastSeq": 40, "reset": true }),
    );
    let synced = feeds(&server, &store, sync4());
    assert!(synced.chat_reread && synced.mail_reread && synced.ended.is_none());
    let kept = store.0.lock().unwrap();
    assert_eq!(kept.sync_seq(CHAT_CURSOR), Some(12));
    assert_eq!(kept.sync_seq(MAIL_CURSOR), Some(40));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_user_feed_folds_membership_settings_read_and_retention() {
    let (store, dir) = held("user");
    {
        let kept = store.0.lock().unwrap();
        room_with_events(&kept);
        room(&kept, "R2", json!({ "chatRoomId": "R2" }));
        kept.put_answer(
            &format!("{CHAT}/timeline"),
            "chatRoomId=R2&limit=50",
            "application/json",
            b"{}",
        )
        .unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 10).unwrap();
        kept.set_sync_seq(&format!("{ROOM_CURSOR}R1"), 0).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 1).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=10"),
        changes(
            json!([
                { "changeSeq": 11, "kind": "membership", "chatRoomId": "R2", "data": { "member": false, "action": "left" } },
                { "changeSeq": 12, "kind": "settings", "chatRoomId": "R1", "data": { "isMuted": true } },
                { "changeSeq": 13, "kind": "read", "chatRoomId": "R1", "data": { "lastReadEventSeq": 5 } },
                { "changeSeq": 14, "kind": "retention", "chatRoomId": "R1", "eventSeq": 2, "data": { "deletedCount": 2 } },
                { "changeSeq": 16, "kind": "bogus", "chatRoomId": "R1", "data": {} }
            ]),
            16,
        ),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=1"),
        json!({ "changes": [], "lastSeq": 1, "more": false, "reset": false }),
    );
    let synced = feeds(&server, &store, sync4());
    assert!(synced.ended.is_none() && !synced.chat_reread);
    // folded whole: no list read
    assert!(!server.asked(&format!("{CHAT}/chatData?limit=200")));
    let kept = store.0.lock().unwrap();
    assert!(kept.chat_room("R2").is_none());
    assert!(kept
        .answer(&format!("{CHAT}/timeline"), "chatRoomId=R2&limit=50")
        .is_none());
    let row: Value = serde_json::from_str(&kept.chat_room("R1").unwrap().row).unwrap();
    assert_eq!(row["isMuted"], true);
    assert_eq!(row["unreadCount"], 0);
    assert_eq!(row["lastReadEventSeq"], 5);
    // retention: events up to 2 gone, and the stretch left starts the timeline
    assert!(kept.chat_event("R1", 2).is_none() && kept.chat_event("R1", 3).is_some());
    let slice = kept.chat_events(
        "R1",
        &EventPage {
            limit: 50,
            ..EventPage::default()
        },
    );
    assert!(!slice.gap_before);
    assert_eq!(kept.sync_seq(CHAT_CURSOR), Some(16));
    assert_eq!(kept.sync_seq(HEADS_CURSOR), Some(77));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_joined_or_changed_reads_the_list_and_the_room_again() {
    let (store, dir) = held("joined");
    {
        let kept = store.0.lock().unwrap();
        room(&kept, "R1", json!({ "chatRoomId": "R1", "roomName": "Old" }));
        kept.set_sync_seq(CHAT_CURSOR, 1).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 0).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=1"),
        changes(
            json!([
                { "changeSeq": 2, "kind": "membership", "chatRoomId": "R3", "data": { "member": true, "action": "joined" } },
                { "changeSeq": 3, "kind": "room", "chatRoomId": "R1", "data": { "reason": "details" } }
            ]),
            3,
        ),
    );
    server.answer(
        &format!("{CHAT}/chatData?limit=200"),
        json!({ "roomList": [{ "chatRoomId": "R1", "roomName": "New" }, { "chatRoomId": "R3" }] }),
    );
    server.answer(
        &format!("{CHAT}/room?chatRoomId=R1"),
        json!({ "room": { "chatRoomId": "R1", "roomName": "New" }, "memberList": [] }),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=0"),
        json!({ "changes": [], "lastSeq": 0, "more": false, "reset": false }),
    );
    feeds(&server, &store, sync4());
    assert!(server.asked(&format!("{CHAT}/room?chatRoomId=R1")));
    let kept = store.0.lock().unwrap();
    assert!(kept.chat_room("R3").is_some());
    let row: Value = serde_json::from_str(&kept.chat_room("R1").unwrap().row).unwrap();
    assert_eq!(row["roomName"], "New");
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reset_with_a_cursor_drops_chat_and_has_it_read_again() {
    let (store, dir) = held("reset");
    {
        let kept = store.0.lock().unwrap();
        room_with_events(&kept);
        kept.put_answer(&format!("{CHAT}/chatData"), "limit=200", "application/json", b"{}")
            .unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 3).unwrap();
        kept.set_sync_seq(&format!("{ROOM_CURSOR}R1"), 1).unwrap();
        kept.set_sync_cursor(
            MAIL_CURSOR,
            &SyncCursor {
                seq: None,
                value: Some(UNSUPPORTED.to_string()),
            },
        )
        .unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=3"),
        json!({ "changes": [], "lastSeq": 90, "reset": true }),
    );
    server.answer(&format!("{MAIL}/mailChanges?limit=200"), json!({ "unsupported": true }));
    let synced = feeds(&server, &store, sync4());
    assert!(synced.chat_reread && synced.mail_unsupported && !synced.mail_reread);
    let kept = store.0.lock().unwrap();
    assert!(kept.chat_room_ids().is_empty());
    assert!(kept.answer(&format!("{CHAT}/chatData"), "limit=200").is_none());
    assert_eq!(kept.sync_seq(&format!("{ROOM_CURSOR}R1")), None);
    assert_eq!(kept.sync_seq(CHAT_CURSOR), Some(90));
    assert_eq!(
        kept.sync_cursor(MAIL_CURSOR).unwrap().value.as_deref(),
        Some(UNSUPPORTED)
    );
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn heads_restore_a_stretch_that_is_current_and_read_the_latest_page_of_one_that_is_not() {
    let (store, dir) = held("heads");
    {
        let kept = store.0.lock().unwrap();
        room_with_events(&kept);
        room(&kept, "R2", json!({ "chatRoomId": "R2" }));
        let event = ChatEvent {
            chat_room_id: "R2".to_string(),
            event_seq: 1,
            event: json!({ "eventSeq": 1 }).to_string(),
            thread_seq: None,
        };
        let page = EventPage {
            limit: 50,
            ..EventPage::default()
        };
        kept.put_chat_page("R2", &page, &[event], false, false).unwrap();
        // a drop: both heads stale
        kept.mark_chat_heads_stale().unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 4).unwrap();
        kept.set_sync_seq(HEADS_CURSOR, 1000).unwrap();
        for id in ["R1", "R2"] {
            kept.set_sync_seq(&format!("{ROOM_CURSOR}{id}"), 0).unwrap();
        }
        kept.set_sync_seq(MAIL_CURSOR, 0).unwrap();
    }
    let server = Server::default();
    // stale heads: all of them are asked for (no headsSince)
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=4"),
        json!({ "changes": [], "lastSeq": 4, "more": false, "reset": false, "headsSince": 2000, "heads": [
            { "chatRoomId": "R1", "lastEventSeq": 5, "lastChangeSeq": 0 },
            { "chatRoomId": "R2", "lastEventSeq": 9, "lastChangeSeq": 0 }] }),
    );
    server.answer(
        &format!("{CHAT}/timeline?chatRoomId=R2&limit=50"),
        json!({ "eventList": [{ "eventSeq": 9 }], "hasMoreBefore": true, "hasMoreAfter": false }),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=0"),
        json!({ "changes": [], "lastSeq": 0, "more": false, "reset": false }),
    );
    feeds(&server, &store, sync4());
    assert!(!server.asked(&format!("{CHAT}/timeline?chatRoomId=R1&limit=50")));
    assert!(server.asked(&format!("{CHAT}/timeline?chatRoomId=R2&limit=50")));
    let kept = store.0.lock().unwrap();
    assert_eq!(kept.chat_head("R1", None), Some(5));
    assert_eq!(kept.chat_head("R2", None), Some(9));
    assert_eq!(kept.sync_seq(HEADS_CURSOR), Some(2000));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_feed_folds_a_redaction_and_reads_changed_messages_again() {
    let (store, dir) = held("room");
    {
        let kept = store.0.lock().unwrap();
        room_with_events(&kept);
        // a thread reply under 2
        kept.put_chat_events(&[ChatEvent {
            chat_room_id: "R1".to_string(),
            event_seq: 6,
            event: json!({ "eventSeq": 6, "threadRootEventSeq": 2, "body": "reply" }).to_string(),
            thread_seq: Some(2),
        }])
        .unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 1).unwrap();
        kept.set_sync_seq(&format!("{ROOM_CURSOR}R1"), 7).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 0).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=1"),
        json!({ "changes": [], "lastSeq": 1, "more": false, "reset": false,
            "heads": [{ "chatRoomId": "R1", "lastEventSeq": 5, "lastChangeSeq": 10 }] }),
    );
    server.answer(
        &format!("{CHAT}/roomChanges?chatRoomId=R1&limit=200&afterSeq=7"),
        json!({ "changes": [
            { "changeSeq": 8, "eventSeq": 3, "data": { "cause": "redaction", "redacted": true } },
            { "changeSeq": 9, "eventSeq": 4, "data": { "cause": "edit" } },
            { "changeSeq": 10, "eventSeq": 6, "data": { "cause": "reaction", "threadRootEventSeq": 2 } },
            { "changeSeq": 11, "eventSeq": 5, "data": { "cause": "pin" } }],
            "lastSeq": 11, "more": false, "reset": false }),
    );
    for (seq, event) in [
        (4, json!([{ "eventSeq": 4, "body": "edited", "edited": true }])),
        (
            6,
            json!([{ "eventSeq": 6, "threadRootEventSeq": 2, "reactions": [{ "key": "+1" }] }]),
        ),
        (2, json!([{ "eventSeq": 2, "threadReplyCount": 1 }])),
        (5, json!([])),
    ] {
        server.answer(
            &format!("{CHAT}/timeline?chatRoomId=R1&eventSeq={seq}"),
            json!({ "eventList": event }),
        );
    }
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=0"),
        json!({ "changes": [], "lastSeq": 0, "more": false, "reset": false }),
    );
    feeds(&server, &store, sync4());
    let kept = store.0.lock().unwrap();
    let event = |seq: i64| -> Value { serde_json::from_str(&kept.chat_event("R1", seq).unwrap().event).unwrap() };
    assert_eq!(event(3)["redacted"], true);
    assert!(event(3)["body"].is_null());
    assert_eq!(event(4)["body"], "edited");
    assert_eq!(event(6)["reactions"][0]["key"], "+1");
    assert_eq!(event(6)["threadRootEventSeq"], 2);
    assert_eq!(event(2)["threadReplyCount"], 1);
    // the server no longer has 5: it goes
    assert!(kept.chat_event("R1", 5).is_none());
    assert!(!server.asked(&format!("{CHAT}/timeline?chatRoomId=R1&eventSeq=3")));
    assert_eq!(kept.sync_seq(&format!("{ROOM_CURSOR}R1")), Some(11));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_without_held_events_takes_its_cursor_and_a_room_reset_reads_its_latest_page() {
    let (store, dir) = held("room-reset");
    {
        let kept = store.0.lock().unwrap();
        room_with_events(&kept);
        room(&kept, "R2", json!({ "chatRoomId": "R2", "lastChangeSeq": 33 }));
        kept.set_sync_seq(CHAT_CURSOR, 1).unwrap();
        kept.set_sync_seq(&format!("{ROOM_CURSOR}R1"), 2).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 0).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=1"),
        json!({ "changes": [], "lastSeq": 1, "more": false, "reset": false,
            "heads": [{ "chatRoomId": "R1", "lastEventSeq": 5, "lastChangeSeq": 50 }] }),
    );
    server.answer(
        &format!("{CHAT}/roomChanges?chatRoomId=R1&limit=200&afterSeq=2"),
        json!({ "changes": [], "lastSeq": 50, "reset": true }),
    );
    server.answer(
        &format!("{CHAT}/timeline?chatRoomId=R1&limit=50"),
        json!({ "eventList": [{ "eventSeq": 5, "body": "new" }], "hasMoreBefore": true, "hasMoreAfter": false }),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=0"),
        json!({ "changes": [], "lastSeq": 0, "more": false, "reset": false }),
    );
    feeds(&server, &store, sync4());
    assert!(!server.asked(&format!("{CHAT}/roomChanges?chatRoomId=R2&limit=200")));
    let kept = store.0.lock().unwrap();
    assert_eq!(kept.sync_seq(&format!("{ROOM_CURSOR}R2")), Some(33));
    assert_eq!(kept.sync_seq(&format!("{ROOM_CURSOR}R1")), Some(50));
    assert!(kept.chat_event("R1", 1).is_none());
    let latest: Value = serde_json::from_str(&kept.chat_event("R1", 5).unwrap().event).unwrap();
    assert_eq!(latest["body"], "new");
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

fn mail_store(store: &Store) {
    let row = |folder: &str| MailThread {
        folder_id: folder.to_string(),
        thread_id: "email:T1".to_string(),
        sort_ms: 10,
        row: json!({ "threadId": "T1", "isUnread": true, "unreadCount": 2 }).to_string(),
    };
    store
        .put_mail_threads(&[
            row("view=inbox"),
            row("account=A&folder=F1"),
            row("account=A&folder=F2"),
        ])
        .unwrap();
    let message = |id: &str| MailMessage {
        message_id: format!("email:{id}"),
        thread_id: "email:T1".to_string(),
        sort_ms: 1,
        row: json!({ "key": format!("email:{id}"), "isUnread": true, "html": "<p>kept</p>" }).to_string(),
    };
    store
        .put_mail_messages(&[message("T1"), message("M2"), message("M3")])
        .unwrap();
}

#[test]
fn the_mail_feed_makes_a_thread_match_the_server() {
    let (store, dir) = held("mail");
    {
        let kept = store.0.lock().unwrap();
        mail_store(&kept);
        kept.put_mail_threads(&[MailThread {
            folder_id: "view=inbox".to_string(),
            thread_id: "email:T9".to_string(),
            sort_ms: 1,
            row: "{}".to_string(),
        }])
        .unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 0).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 5).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=0"),
        changes(json!([]), 0),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=5"),
        json!({ "changes": [
            { "changeSeq": 6, "kind": "state", "threadId": "T1", "removedMessageIds": ["M3"], "deleted": false, "messages": [
                { "emailMessageId": "T1", "emailAccountId": "A", "emailFolderId": "F1", "isRead": true, "isFlagged": true,
                  "archived": false, "messageDate": 20 },
                { "emailMessageId": "M4", "emailAccountId": "A", "emailFolderId": "F1", "isRead": false, "archived": false,
                  "messageDate": 30, "snippet": "newest" }] },
            { "changeSeq": 7, "kind": "delete", "threadId": "T9", "deleted": true, "messages": [] }],
            "lastSeq": 7, "more": false, "reset": false, "uncoveredStores": [] }),
    );
    server.answer(
        &format!("{MAIL}/thread?store=email&threadId=T1"),
        json!({ "thread": { "store": "email", "threadId": "T1", "messageList": [
            { "key": "email:T1", "date": 20 }, { "key": "email:M4", "date": 30 }] } }),
    );
    feeds(&server, &store, sync4());
    let kept = store.0.lock().unwrap();
    // M2 left the thread, M3 was removed; M4 is new, so the thread was read again
    assert!(kept.mail_message("email:M2").is_none() && kept.mail_message("email:M3").is_none());
    assert!(kept.mail_message("email:M4").is_some());
    assert!(server.asked(&format!("{MAIL}/thread?store=email&threadId=T1")));
    let folders: Vec<String> = kept
        .mail_threads_of("email:T1")
        .iter()
        .map(|row| row.folder_id.clone())
        .collect();
    assert!(folders.contains(&"view=inbox".to_string()) && folders.contains(&"account=A&folder=F1".to_string()));
    assert!(!folders.contains(&"account=A&folder=F2".to_string()));
    let row: Value = serde_json::from_str(&kept.mail_threads_of("email:T1")[0].row).unwrap();
    assert_eq!(row["unreadCount"], 1);
    assert_eq!(row["isFlagged"], true);
    assert_eq!(row["lastEmailMessageId"], "M4");
    assert_eq!(row["snippet"], "newest");
    assert!(kept.mail_threads_of("email:T9").is_empty());
    assert_eq!(kept.sync_seq(MAIL_CURSOR), Some(7));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mail_state_fold_keeps_bodies_and_a_mail_reset_drops_the_threads() {
    let (store, dir) = held("mail-reset");
    {
        let kept = store.0.lock().unwrap();
        mail_store(&kept);
        kept.set_sync_seq(CHAT_CURSOR, 0).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 5).unwrap();
        kept.put_answer(&format!("{MAIL}/threads"), "view=inbox", "application/json", b"{}")
            .unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=0"),
        changes(json!([]), 0),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=5"),
        json!({ "changes": [], "lastSeq": 60, "reset": true }),
    );
    let synced = feeds(&server, &store, sync4());
    assert!(synced.mail_reread);
    let kept = store.0.lock().unwrap();
    assert!(kept.mail_threads_of("email:T1").is_empty());
    assert!(kept.mail_message("email:T1").is_none());
    assert!(kept.answer(&format!("{MAIL}/threads"), "view=inbox").is_none());
    assert_eq!(kept.sync_seq(MAIL_CURSOR), Some(60));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_stores_the_mail_cursor_does_not_cover_are_read_again() {
    let (store, dir) = held("uncovered");
    {
        let kept = store.0.lock().unwrap();
        kept.put_mail_messages(&[MailMessage {
            message_id: "internal:C1".to_string(),
            thread_id: "internal:C1".to_string(),
            sort_ms: 1,
            row: "{}".to_string(),
        }])
        .unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 0).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 5).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=0"),
        changes(json!([]), 0),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=5"),
        json!({ "changes": [], "lastSeq": 5, "more": false, "reset": false, "uncoveredStores": ["internal"] }),
    );
    server.answer(
        &format!("{MAIL}/thread?store=internal&threadId=C1"),
        json!({ "thread": { "store": "internal", "threadId": "C1", "messageList": [{ "key": "internal:C1", "date": 2, "text": "fresh" }] } }),
    );
    feeds(&server, &store, sync4());
    assert!(server.asked(&format!("{MAIL}/thread?store=internal&threadId=C1")));
    let kept = store.0.lock().unwrap();
    let row: Value = serde_json::from_str(&kept.mail_message("internal:C1").unwrap().row).unwrap();
    assert_eq!(row["text"], "fresh");
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_offline_feed_stops_the_run_and_keeps_the_cursor() {
    let (store, dir) = held("offline");
    store.0.lock().unwrap().set_sync_seq(CHAT_CURSOR, 3).unwrap();
    let server = Server::default();
    server
        .answers
        .borrow_mut()
        .insert(format!("{CHAT}/chatChanges?limit=200&afterSeq=3"), Fetched::Offline);
    let synced = feeds(&server, &store, sync4());
    assert_eq!(synced.ended, Some(Ended::Offline));
    assert_eq!(store.0.lock().unwrap().sync_seq(CHAT_CURSOR), Some(3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feeds_run_one_at_a_time_and_once_more_when_asked_meanwhile() {
    let runs = Feeds::default();
    assert!(runs.begin("a", Scope::Chat));
    assert!(!runs.begin("a", Scope::All));
    assert!(!runs.begin("a", Scope::Chat));
    assert!(runs.begin("b", Scope::All));
    // the run asked for meanwhile is of the widest scope asked
    assert_eq!(runs.finish("a"), Some(Scope::All));
    assert_eq!(runs.finish("a"), None);
    assert!(runs.begin("a", Scope::All));
    assert_eq!(runs.finish("b"), None);
    // a run that stops early lets go, whatever was asked meanwhile: the next start runs
    assert!(!runs.begin("a", Scope::Chat));
    runs.end("a");
    assert!(runs.begin("a", Scope::All));
    // the mirror waits for a run under way, and gives up when cancelled
    assert!(!runs.acquire("a", &|| true));
    runs.end("a");
    assert!(runs.acquire("a", &|| false));
    assert!(!runs.begin("a", Scope::Chat));
    assert_eq!(runs.finish("a"), Some(Scope::Chat));
    assert_eq!(runs.finish("a"), None);
}

#[test]
fn a_mark_let_go_on_another_thread_frees_a_mirror_waiting_for_it() {
    let runs = std::sync::Arc::new(Feeds::default());
    assert!(runs.begin("a", Scope::All));
    let other = std::sync::Arc::clone(&runs);
    let freer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        other.end("a");
    });
    assert!(runs.acquire("a", &|| false));
    freer.join().unwrap();
}

#[test]
fn a_follow_up_read_not_done_stays_pending_past_the_cursor_until_it_answers() {
    let (store, dir) = held("pending");
    {
        let kept = store.0.lock().unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 1).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 0).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=1"),
        changes(
            json!([{ "changeSeq": 2, "kind": "membership", "chatRoomId": "R3", "data": { "member": true } }]),
            2,
        ),
    );
    server
        .answers
        .borrow_mut()
        .insert(format!("{CHAT}/chatData?limit=200"), Fetched::Failed(500));
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=0"),
        json!({ "changes": [], "lastSeq": 0, "more": false, "reset": false }),
    );
    feeds(&server, &store, sync4());
    {
        let kept = store.0.lock().unwrap();
        // the cursor moved past the change, and what it left to read is kept, its tries counted
        assert_eq!(kept.sync_seq(CHAT_CURSOR), Some(2));
        assert!(kept.sync_seq(&format!("{PENDING}list")).is_some_and(|tries| tries >= 1));
    }
    // offline next time: still pending
    server
        .answers
        .borrow_mut()
        .insert(format!("{CHAT}/chatData?limit=200"), Fetched::Offline);
    assert_eq!(feeds(&server, &store, sync4()).ended, Some(Ended::Offline));
    assert!(store.0.lock().unwrap().sync_cursor(&format!("{PENDING}list")).is_some());
    // it answers: read, and done
    server.answer(
        &format!("{CHAT}/chatData?limit=200"),
        json!({ "roomList": [{ "chatRoomId": "R3" }] }),
    );
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=2"),
        changes(json!([]), 2),
    );
    feeds(&server, &store, sync4());
    let kept = store.0.lock().unwrap();
    assert!(kept.sync_cursor(&format!("{PENDING}list")).is_none());
    assert!(kept.chat_room("R3").is_some());
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pending_read_that_never_answers_is_given_up() {
    let (store, dir) = held("give-up");
    store
        .0
        .lock()
        .unwrap()
        .set_sync_seq(&format!("{PENDING}room:R9"), PENDING_TRIES - 1)
        .unwrap();
    let server = Server::default();
    feeds(&server, &store, sync4());
    assert!(server.asked(&format!("{CHAT}/room?chatRoomId=R9")));
    assert!(store
        .0
        .lock()
        .unwrap()
        .sync_cursor(&format!("{PENDING}room:R9"))
        .is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_cursor_stops_at_the_first_change_not_read_and_a_room_with_nothing_held_asks_nothing() {
    let (store, dir) = held("room-stop");
    {
        let kept = store.0.lock().unwrap();
        room_with_events(&kept);
        room(&kept, "R2", json!({ "chatRoomId": "R2" }));
        kept.set_sync_seq(CHAT_CURSOR, 1).unwrap();
        kept.set_sync_seq(&format!("{ROOM_CURSOR}R1"), 7).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=1"),
        json!({ "changes": [], "lastSeq": 1, "more": false, "reset": false,
            "heads": [{ "chatRoomId": "R1", "lastEventSeq": 5, "lastChangeSeq": 10 }] }),
    );
    server.answer(
        &format!("{CHAT}/roomChanges?chatRoomId=R1&limit=200&afterSeq=7"),
        json!({ "changes": [
            { "changeSeq": 8, "eventSeq": 4, "data": { "cause": "edit" } },
            { "changeSeq": 9, "eventSeq": 3, "data": { "cause": "edit" } },
            { "changeSeq": 10, "eventSeq": 2, "data": { "cause": "edit" } }],
            "lastSeq": 10, "more": false, "reset": false }),
    );
    server.answer(
        &format!("{CHAT}/timeline?chatRoomId=R1&eventSeq=4"),
        json!({ "eventList": [{ "eventSeq": 4, "body": "four" }] }),
    );
    server.answers.borrow_mut().insert(
        format!("{CHAT}/timeline?chatRoomId=R1&eventSeq=3"),
        Fetched::Failed(500),
    );
    feeds_of(&server, &store, sync4(), Scope::Chat);
    let kept = store.0.lock().unwrap();
    assert_eq!(kept.sync_seq(&format!("{ROOM_CURSOR}R1")), Some(8));
    // a chat-only run reads no mail, and R2 (nothing held, no change known) is not asked
    assert!(!server.asked(&format!("{MAIL}/mailChanges?limit=200")));
    assert!(!server.asked(&format!("{CHAT}/roomChanges?chatRoomId=R2&limit=200")));
    assert!(!server.asked(&format!("{CHAT}/timeline?chatRoomId=R1&eventSeq=2")));
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_new_thread_gets_a_row_in_the_lists_kept_and_a_message_view_row_its_own_state() {
    let (store, dir) = held("new-rows");
    {
        let kept = store.0.lock().unwrap();
        let row = |folder: &str, id: &str| MailThread {
            folder_id: folder.to_string(),
            thread_id: id.to_string(),
            sort_ms: 5,
            row: json!({ "rowKey": id, "isUnread": true }).to_string(),
        };
        // the inbox lists threads; the folder F1 lists messages
        kept.put_mail_threads(&[row("view=inbox", "email:OLD"), row("account=A&folder=F1", "email:M7")])
            .unwrap();
        crate::offline::put_object(
            &kept,
            crate::offline::FOLDER_META,
            &crate::offline::folder_meta_query("account=A&folder=F1"),
            &json!({ "conversationView": "messages" }),
        )
        .unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 0).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 1).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=0"),
        changes(json!([]), 0),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=1"),
        json!({ "changes": [{ "changeSeq": 2, "kind": "new", "threadId": "T5", "deleted": false, "messages": [
            { "emailMessageId": "T5", "emailAccountId": "A", "emailFolderId": "F1", "isRead": false, "archived": false,
              "messageDate": 50, "subject": "New", "snippet": "hello" },
            { "emailMessageId": "M7", "emailAccountId": "A", "emailFolderId": "F1", "isRead": true, "isFlagged": true,
              "archived": false, "messageDate": 60 }] }],
            "lastSeq": 2, "more": false, "reset": false }),
    );
    feeds(&server, &store, sync4());
    let kept = store.0.lock().unwrap();
    let inbox = kept.mail_threads("view=inbox", None, 10);
    let new_row = inbox.iter().find(|row| row.thread_id == "email:T5").unwrap();
    let fields: Value = serde_json::from_str(&new_row.row).unwrap();
    assert_eq!(
        (fields["subject"].as_str(), fields["unreadCount"].as_i64()),
        (Some("New"), Some(1))
    );
    assert_eq!(new_row.sort_ms, 60);
    // the message view keeps its own rows: M7's takes M7's state, and no thread row is added there
    let listed = kept.mail_threads("account=A&folder=F1", None, 10);
    assert_eq!(listed.len(), 1);
    let message_row: Value = serde_json::from_str(&listed[0].row).unwrap();
    assert_eq!(
        (message_row["isUnread"].as_bool(), message_row["isFlagged"].as_bool()),
        (Some(false), Some(true))
    );
    drop(kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_uncovered_rereads_go_round_the_threads_kept() {
    let (store, dir) = held("rotate");
    {
        let kept = store.0.lock().unwrap();
        let messages: Vec<MailMessage> = (0..51)
            .map(|n| MailMessage {
                message_id: format!("internal:C{n:02}"),
                thread_id: format!("internal:C{n:02}"),
                sort_ms: 1,
                row: "{}".to_string(),
            })
            .collect();
        kept.put_mail_messages(&messages).unwrap();
        kept.set_sync_seq(CHAT_CURSOR, 0).unwrap();
        kept.set_sync_seq(MAIL_CURSOR, 5).unwrap();
    }
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/chatChanges?limit=200&afterSeq=0"),
        changes(json!([]), 0),
    );
    server.answer(
        &format!("{MAIL}/mailChanges?limit=200&afterSeq=5"),
        json!({ "changes": [], "lastSeq": 5, "more": false, "reset": false, "uncoveredStores": ["internal"] }),
    );
    let last = format!("{MAIL}/thread?store=internal&threadId=C50");
    feeds(&server, &store, sync4());
    assert!(!server.asked(&last));
    server.asked.borrow_mut().clear();
    feeds(&server, &store, sync4());
    assert!(server.asked(&last));
    assert!(!server.asked(&format!("{MAIL}/thread?store=internal&threadId=C00")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_query_names_its_room() {
    assert!(names_room("/x/timeline?chatRoomId=R1&limit=50", "R1"));
    assert!(names_room("/x/timeline?afterSeq=2&chatRoomId=R1", "R1"));
    assert!(!names_room("/x/timeline?chatRoomId=R12", "R1"));
}
