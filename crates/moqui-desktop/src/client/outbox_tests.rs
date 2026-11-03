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

use moqui_client::store::{MailThread, OutboxState, Store};
use serde_json::{json, Value};

use super::*;
use crate::client::offline::chat_room;

const NOW: i64 = 1_700_000_000_000;

fn store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-outbox-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::open(&dir, "a", 1 << 24).unwrap(), dir)
}

fn chat_item(client_ref: &str, room: &str) -> Value {
    json!({ "clientRef": client_ref, "chatRoomId": room, "url": "/apps/BasicComms/Chat/send",
        "params": { "body": "<p>hi</p>", "replyToEventSeq": "", "threadRootEventSeq": "" }, "text": "hi",
        "threadRootEventSeq": null, "hasFiles": false, "createdDate": NOW - 1000, "status": "waiting", "attempts": 2 })
}

fn checked(kind: &str, payload: &Value) -> Result<OutboxOp, String> {
    check(kind, payload, &[], NOW)
}

#[test]
fn classify_and_backoff_follow_webroot_chat() {
    for status in [0, 408, 429, 500, 502, 503] {
        assert_eq!(classify(status), Next::Retry, "{status}");
    }
    assert_eq!(classify(401), Next::Hold);
    assert_eq!(classify(403), Next::Hold);
    for status in [200, 400, 404, 409, 422] {
        assert_eq!(classify(status), Next::Fail, "{status}");
    }
    assert_eq!(
        [0, 1, 2, 3, 4, 9].map(backoff_ms),
        [2_000, 2_000, 5_000, 15_000, 60_000, 60_000]
    );
}

#[test]
fn a_chat_send_is_checked_and_keeps_the_pages_item() {
    let op = checked(CHAT_SEND, &chat_item("c1", "R1")).unwrap();
    assert_eq!(
        (op.id.as_str(), op.kind.as_str(), op.target.as_str()),
        ("c1", CHAT_SEND, "R1")
    );
    assert_eq!(op.created_ms, NOW - 1000);
    let payload: Value = serde_json::from_str(&op.payload).unwrap();
    assert_eq!(payload["text"], "hi");
    // the page's own state is the host's now
    assert!(payload.get("status").is_none() && payload.get("attempts").is_none());
    // under /qapps too, and a thread reply is ordered within its thread
    let mut item = chat_item("c2", "R1");
    item["url"] = json!("/qapps/BasicComms/Chat/send");
    item["threadRootEventSeq"] = json!(7);
    assert_eq!(checked(CHAT_SEND, &item).unwrap().target, "R1:7");
    // a comms app of another root
    // any app's Chat on the account's server
    let mut item = chat_item("c3", "R1");
    item["url"] = json!("/apps/boss/Comms/Chat/send");
    assert!(check(CHAT_SEND, &item, &[], NOW).is_ok());
    // the server's clientRef characters
    assert!(checked(CHAT_SEND, &chat_item("web:c4.1-x_y", "R1")).is_ok());
}

#[test]
fn a_page_cannot_send_elsewhere_or_too_much() {
    for url in [
        "https://evil.example.com/apps/BasicComms/Chat/send",
        "//evil.example.com/apps/BasicComms/Chat/send",
        "/apps/BasicComms/Chat/edit",
        "/apps/../Chat/send",
        "/apps//Chat/send",
        "/apps/Chat/send",
        "/files/BasicComms/Chat/send",
        "/apps/BasicComms/Chat/send?x=1",
        "",
    ] {
        let mut item = chat_item("c1", "R1");
        item["url"] = json!(url);
        assert!(checked(CHAT_SEND, &item).is_err(), "{url}");
    }
    let mut item = chat_item("bad ref!", "R1");
    assert!(checked(CHAT_SEND, &item).is_err());
    item = chat_item("c1", "");
    assert!(checked(CHAT_SEND, &item).is_err());
    item = chat_item("c1", "R1");
    item["params"] = json!({ "body": { "nested": true } });
    assert!(checked(CHAT_SEND, &item).is_err());
    item = chat_item("c1", "R1");
    item["params"]["body"] = json!("x".repeat(ITEM_BYTES));
    assert!(checked(CHAT_SEND, &item).unwrap_err().contains("too large"));
    // a chat message's files were uploaded already; files that are not the store's never
    assert!(check(CHAT_SEND, &chat_item("c1", "R1"), &["f1".to_string()], NOW).is_err());
    assert!(check(MAIL_SEND, &json!({}), &["../x".to_string()], NOW).is_err());
    assert!(checked("chat-edit", &json!({})).is_err());
    assert!(checked(CHAT_SEND, &json!([1])).is_err());
}

#[test]
fn mail_operations_are_checked() {
    let send = json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1", "toJson": "[]", "subject": "s",
        "isForward": false, "holdSeconds": null }, "files": [] });
    // holdSeconds is not a field it carries
    assert!(checked(MAIL_SEND, &send).unwrap_err().contains("holdSeconds"));
    let send = json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1", "toJson": "[]", "subject": "s",
        "isForward": false }, "files": [{ "file": "f-a.txt", "name": "a.txt", "contentType": "text/plain" }] });
    let op = check(MAIL_SEND, &send, &["f-a.txt".to_string()], NOW).unwrap();
    assert_eq!((op.id.as_str(), op.target.as_str()), ("m1", ""));
    assert_eq!(op.files, ["f-a.txt"]);
    let payload: Value = serde_json::from_str(&op.payload).unwrap();
    assert_eq!(payload["fields"]["isForward"], "false");
    // a file not among the operation's, or one with no entry
    assert!(check(MAIL_SEND, &send, &[], NOW).is_err());
    let bare = json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1" } });
    assert!(check(MAIL_SEND, &bare, &["f-a.txt".to_string()], NOW).is_err());
    assert!(checked(MAIL_SEND, &json!({ "clientRef": "m1", "fields": {} })).is_err());

    let state = json!({ "targets": [{ "store": "email", "threadId": "T1", "rowKey": "email:T1" }], "action": "read" });
    let op = checked(MAIL_STATE, &state).unwrap();
    assert_eq!(op.target, MAIL_STATE_TARGET);
    assert!(checked(MAIL_STATE, &json!({ "targets": [{}], "action": "labels" })).is_err());
    assert!(checked(MAIL_STATE, &json!({ "targets": [], "action": "read" })).is_err());
    assert!(checked(MAIL_STATE, &json!({ "targets": [{}], "action": "move" })).is_err());
    assert!(checked(
        MAIL_STATE,
        &json!({ "targets": [{}], "action": "move", "emailFolderId": "F2" })
    )
    .is_ok());
    assert!(checked(CHAT_READ, &json!({ "chatRoomId": "R1" })).is_err());
    // an internal message's send is not taken once by its ref yet: it needs a connection
    let internal = json!({ "clientRef": "m2", "fields": { "fromValue": "internal", "toJson": "[]" } });
    assert!(checked(MAIL_SEND, &internal).unwrap_err().contains("connection"));
}

#[test]
fn adding_again_keeps_one_and_the_outbox_holds_fifty() {
    let (store, dir) = store("add");
    let (op, added) = add(&store, checked(CHAT_SEND, &chat_item("c1", "R1")).unwrap()).unwrap();
    assert!(added && op.id == "c1");
    // an import, or a page adding again after a crash
    let (kept, added) = add(&store, checked(CHAT_SEND, &chat_item("c1", "R1")).unwrap()).unwrap();
    assert!(!added);
    assert_eq!(kept.created_ms, op.created_ms);
    for at in 2..=OUTBOX_MAX {
        add(&store, checked(CHAT_SEND, &chat_item(&format!("c{at}"), "R1")).unwrap()).unwrap();
    }
    let full = add(&store, checked(CHAT_SEND, &chat_item("c51", "R1")).unwrap());
    assert!(full.unwrap_err().contains("full"));
    // one taken and waiting for its echo does not count
    store
        .outbox_update(
            "c1",
            &moqui_client::store::OutboxUpdate {
                state: OutboxState::Acked,
                attempts: 1,
                next_ms: NOW,
                error: None,
            },
        )
        .unwrap();
    assert!(add(&store, checked(CHAT_SEND, &chat_item("c51", "R1")).unwrap()).is_ok());
    assert_eq!(list_json(&store).as_array().unwrap().len(), OUTBOX_MAX);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mark_read_replaces_one_waiting_and_reads_the_room_at_once() {
    let (store, dir) = store("read");
    let room = json!({ "chatRoomId": "R1", "unreadCount": 4, "mentionCount": 1, "lastMessage": { "eventSeq": 9 } });
    store.put_chat_room(&chat_room(&room).unwrap()).unwrap();
    add(
        &store,
        checked(CHAT_READ, &json!({ "chatRoomId": "R1", "lastReadEventSeq": 7 })).unwrap(),
    )
    .unwrap();
    let row: Value = serde_json::from_str(&store.chat_room("R1").unwrap().row).unwrap();
    assert_eq!(
        (row["unreadCount"].as_i64(), row["mentionCount"].as_i64()),
        (Some(0), Some(0))
    );
    assert_eq!(row["lastReadEventSeq"], 7);
    // a later one, even with a lower seq (another tab), keeps the furthest
    add(
        &store,
        checked(CHAT_READ, &json!({ "chatRoomId": "R1", "lastReadEventSeq": 5 })).unwrap(),
    )
    .unwrap();
    add(
        &store,
        checked(CHAT_READ, &json!({ "chatRoomId": "R2", "lastReadEventSeq": 1 })).unwrap(),
    )
    .unwrap();
    let reads: Vec<_> = store.outbox_list().into_iter().filter(|op| op.target == "R1").collect();
    assert_eq!(reads.len(), 1);
    assert_eq!(payload_of(&reads[0])["lastReadEventSeq"], 7);
    assert_eq!(store.outbox_list().len(), 2);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mail_state_change_is_applied_to_the_stored_rows() {
    let (store, dir) = store("mail");
    let row = |folder: &str, id: &str| MailThread {
        folder_id: folder.to_string(),
        thread_id: id.to_string(),
        sort_ms: 10,
        row: json!({ "rowKey": id, "isUnread": true, "unreadCount": 2, "isFlagged": false }).to_string(),
    };
    store
        .put_mail_threads(&[
            row("view=inbox", "email:T1"),
            row("view=all", "email:T1"),
            row("view=inbox", "email:T2"),
            row("view=all", "email:T2"),
            row("account=A&folder=F1", "email:T2"),
            row("view=inbox", "email:T3"),
        ])
        .unwrap();
    let state = |action: &str, keys: &[&str]| {
        let targets: Vec<Value> = keys.iter().map(|key| json!({ "rowKey": key })).collect();
        let mut payload = json!({ "targets": targets, "action": action });
        if action == "move" {
            payload["emailFolderId"] = json!("F9");
        }
        add(&store, checked(MAIL_STATE, &payload).unwrap()).unwrap();
    };
    let fields = |folder: &str, id: &str| -> Option<Value> {
        store
            .mail_threads(folder, None, 50)
            .into_iter()
            .find(|thread| thread.thread_id == id)
            .map(|thread| serde_json::from_str(&thread.row).unwrap())
    };
    state("read", &["email:T1"]);
    for folder in ["view=inbox", "view=all"] {
        let read = fields(folder, "email:T1").unwrap();
        assert_eq!(
            (read["isUnread"].as_bool(), read["unreadCount"].as_i64()),
            (Some(false), Some(0))
        );
    }
    state("flag", &["email:T1"]);
    assert_eq!(fields("view=all", "email:T1").unwrap()["isFlagged"], true);
    state("unread", &["email:T1"]);
    assert_eq!(fields("view=inbox", "email:T1").unwrap()["unreadCount"], 1);
    // archive leaves the inbox only; move leaves all but all mail; trash leaves every folder
    state("archive", &["email:T1"]);
    assert!(fields("view=inbox", "email:T1").is_none() && fields("view=all", "email:T1").is_some());
    state("move", &["email:T2"]);
    assert!(fields("view=inbox", "email:T2").is_none() && fields("account=A&folder=F1", "email:T2").is_none());
    assert!(fields("view=all", "email:T2").is_some());
    state("trash", &["email:T3", "email:T1"]);
    assert!(store.mail_threads_of("email:T3").is_empty() && store.mail_threads_of("email:T1").is_empty());
    // each change waits in one order
    assert!(store.outbox_list().iter().all(|op| op.target == MAIL_STATE_TARGET));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_echo_removes_only_its_chat_send_and_retry_makes_one_due() {
    let (store, dir) = store("echo");
    add(&store, checked(CHAT_SEND, &chat_item("c1", "R1")).unwrap()).unwrap();
    let read = add(
        &store,
        checked(CHAT_READ, &json!({ "chatRoomId": "R1", "lastReadEventSeq": 1 })).unwrap(),
    )
    .unwrap()
    .0;
    assert!(!echoed(&store, &read.id));
    assert!(!echoed(&store, "nope"));
    assert!(echoed(&store, "c1"));
    assert!(store.outbox_get("c1").is_none());

    add(&store, checked(CHAT_SEND, &chat_item("c2", "R1")).unwrap()).unwrap();
    store
        .outbox_update(
            "c2",
            &moqui_client::store::OutboxUpdate {
                state: OutboxState::Failed,
                attempts: 3,
                next_ms: NOW,
                error: Some("No".to_string()),
            },
        )
        .unwrap();
    assert!(retry(&store, "c2", NOW).unwrap());
    let op = store.outbox_get("c2").unwrap();
    assert_eq!((op.state, op.attempts, op.error), (OutboxState::Retry, 0, None));
    assert!(!retry(&store, "nope", NOW).unwrap());
    // what a shared sign-out would lose
    store.put_draft("boss.chat.draft.R1", "\"x\"", &[]).unwrap();
    let unsent = unsent_json(&store);
    assert_eq!(unsent["operations"].as_array().unwrap().len(), 2);
    assert_eq!(unsent["drafts"][0]["key"], "boss.chat.draft.R1");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_echo_of_my_message_names_its_client_ref() {
    let frame = |sender: &str, topic: &str| {
        json!({ "topic": topic, "message": { "kind": "event", "chatRoomId": "R1",
            "event": { "eventSeq": 4, "senderUserId": sender, "clientRef": "c1" } } })
        .to_string()
    };
    assert_eq!(
        crate::client::push::echo_ref("ME", &frame("ME", "chatRoom:R1")).as_deref(),
        Some("c1")
    );
    assert_eq!(
        crate::client::push::echo_ref("ME", &frame("OTHER", "chatRoom:R1")),
        None
    );
    assert_eq!(crate::client::push::echo_ref("ME", &frame("ME", "chatUser:ME")), None);
    // the message as JSON text
    let text = json!({ "topic": "chatRoom:R1", "message": json!({ "kind": "event",
        "event": { "senderUserId": "ME", "clientRef": "c2" } }).to_string() })
    .to_string();
    assert_eq!(crate::client::push::echo_ref("ME", &text).as_deref(), Some("c2"));
    assert_eq!(crate::client::push::echo_ref("ME", "not json"), None);
}

#[test]
fn drafts_and_files_are_capped() {
    let (store, dir) = store("caps");
    assert!(draft_room(&store, "k", 10).is_ok());
    assert!(draft_room(&store, "k", DRAFTS_BYTES + 1).is_err());
    for at in 0..DRAFTS_MAX {
        store.put_draft(&format!("d{at}"), "x", &[]).unwrap();
    }
    assert!(draft_room(&store, "new", 1).unwrap_err().contains("Too many"));
    // one that replaces a kept draft does not add one
    assert!(draft_room(&store, "d0", 1).is_ok());
    store.save_file("a.bin", &[0_u8; 1000]).unwrap();
    assert!(file_room(&store, 24, 4 * 1024).is_ok());
    assert!(file_room(&store, 25, 4 * 1024).unwrap_err().contains("too large"));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
