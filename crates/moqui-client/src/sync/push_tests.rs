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

use crate::store::{EventPage, Store};
use serde_json::{json, Value};

use super::keep_frame;
use crate::offline::{chat_event, chat_room};

const ME: &str = "ME";

fn store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-push-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, "a", 1 << 24).unwrap();
    let events: Vec<_> = (1..=3)
        .map(|seq| {
            chat_event(
                "R1",
                &json!({ "eventSeq": seq, "type": "message", "body": format!("m{seq}"),
            "reactions": [], "threadReplyCount": 0 }),
            )
            .unwrap()
        })
        .collect();
    let latest = EventPage {
        limit: 50,
        ..EventPage::default()
    };
    store.put_chat_page("R1", &latest, &events, false, false).unwrap();
    let room = json!({ "chatRoomId": "R1", "unreadCount": 0, "mentionCount": 0, "lastMessage": { "eventSeq": 3, "sentDate": 30 } });
    store.put_chat_room(&chat_room(&room).unwrap()).unwrap();
    (store, dir)
}

fn push(store: &Store, topic: &str, message: Value) {
    keep_frame(store, ME, &json!({ "topic": topic, "message": message }).to_string());
}

fn event_push(store: &Store, event: Value) {
    push(
        store,
        "chatRoom:R1",
        json!({ "kind": "event", "chatRoomId": "R1", "eventSeq": event["eventSeq"], "event": event }),
    );
}

fn kept(store: &Store, seq: i64) -> Value {
    serde_json::from_str(&store.chat_event("R1", seq).unwrap().event).unwrap()
}

fn room(store: &Store) -> Value {
    serde_json::from_str(&store.chat_room("R1").unwrap().row).unwrap()
}

fn latest(store: &Store) -> Vec<i64> {
    let page = EventPage {
        limit: 50,
        ..EventPage::default()
    };
    let slice = store.chat_events("R1", &page);
    assert!(!slice.gap_after, "the head is kept up");
    slice.events.iter().map(|event| event.event_seq).collect()
}

#[test]
fn a_pushed_message_joins_the_timeline_and_counts_once() {
    let (store, dir) = store("message");
    let message = json!({ "eventSeq": 4, "type": "message", "senderUserId": "U2", "body": "hi", "bodyText": "hi",
        "sentDate": 40, "mentions": [ME] });
    event_push(&store, message.clone());
    event_push(&store, message);
    assert_eq!(latest(&store), [1, 2, 3, 4]);
    let four = kept(&store, 4);
    assert_eq!(
        (four["edited"].clone(), four["reactions"].clone()),
        (json!(false), json!([]))
    );
    let row = room(&store);
    assert_eq!(
        (row["unreadCount"].clone(), row["mentionCount"].clone()),
        (json!(1), json!(1))
    );
    assert_eq!(row["lastMessage"]["eventSeq"], json!(4));
    assert_eq!(store.chat_room("R1").unwrap().sort_ms, 40);
    // my own: no unread
    event_push(
        &store,
        json!({ "eventSeq": 5, "type": "message", "senderUserId": ME, "sentDate": 50 }),
    );
    assert_eq!(room(&store)["unreadCount"], json!(1));
    // read on another device
    push(
        &store,
        "chatUser:ME",
        json!({ "kind": "read", "chatRoomId": "R1", "lastReadEventSeq": 5 }),
    );
    let row = room(&store);
    assert_eq!(
        (row["unreadCount"].clone(), row["lastReadEventSeq"].clone()),
        (json!(0), json!(5))
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn edits_reactions_and_redactions_fold_into_their_message() {
    let (store, dir) = store("fold");
    event_push(
        &store,
        json!({ "eventSeq": 4, "type": "reaction", "relatesToEventSeq": 2, "senderUserId": ME,
        "content": { "key": "+1" } }),
    );
    event_push(
        &store,
        json!({ "eventSeq": 4, "type": "reaction", "relatesToEventSeq": 2, "senderUserId": ME,
        "content": { "key": "+1" } }),
    );
    event_push(
        &store,
        json!({ "eventSeq": 5, "type": "reaction", "relatesToEventSeq": 2, "senderUserId": "U2",
        "content": { "key": "+1" } }),
    );
    assert_eq!(
        kept(&store, 2)["reactions"],
        json!([{ "key": "+1", "count": 2, "userIds": [ME, "U2"], "mine": true }])
    );
    // no rows of their own, and the head kept past them: a message after joins the range
    event_push(
        &store,
        json!({ "eventSeq": 6, "type": "edit", "editsEventSeq": 2, "body": "new", "bodyText": "new",
        "sentDate": 60 }),
    );
    let two = kept(&store, 2);
    assert_eq!(
        (two["body"].clone(), two["edited"].clone(), two["editedDate"].clone()),
        (json!("new"), json!(true), json!(60))
    );
    event_push(
        &store,
        json!({ "eventSeq": 7, "type": "redaction", "senderUserId": ME,
        "content": { "target": "reaction", "messageSeq": 2, "key": "+1" } }),
    );
    assert_eq!(
        kept(&store, 2)["reactions"],
        json!([{ "key": "+1", "count": 1, "userIds": ["U2"], "mine": false }])
    );
    event_push(
        &store,
        json!({ "eventSeq": 8, "type": "redaction", "relatesToEventSeq": 3, "sentDate": 80 }),
    );
    let three = kept(&store, 3);
    assert_eq!(
        (three["redacted"].clone(), three["body"].clone()),
        (json!(true), Value::Null)
    );
    event_push(
        &store,
        json!({ "eventSeq": 9, "type": "message", "senderUserId": "U2" }),
    );
    assert_eq!(latest(&store), [1, 2, 3, 9]);
    // a field change with no event: a poll, a link preview
    push(
        &store,
        "chatRoom:R1",
        json!({ "kind": "update", "eventSeq": 9, "fields": { "linkPreviews": [{ "url": "u" }] } }),
    );
    assert_eq!(kept(&store, 9)["linkPreviews"], json!([{ "url": "u" }]));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thread_reply_counts_on_its_root_and_a_left_room_goes() {
    let (store, dir) = store("thread");
    let reply =
        json!({ "eventSeq": 4, "type": "message", "threadRootEventSeq": 1, "senderUserId": "U2", "sentDate": 40 });
    event_push(&store, reply.clone());
    event_push(&store, reply);
    let root = kept(&store, 1);
    assert_eq!(
        (root["threadReplyCount"].clone(), root["threadLastEventSeq"].clone()),
        (json!(1), json!(4))
    );
    // the main timeline does not list it
    assert_eq!(latest(&store), [1, 2, 3]);
    event_push(
        &store,
        json!({ "eventSeq": 5, "type": "redaction", "relatesToEventSeq": 4 }),
    );
    assert_eq!(kept(&store, 1)["threadReplyCount"], json!(0));
    push(
        &store,
        "chatUser:ME",
        json!({ "kind": "room", "action": "left", "chatRoomId": "R1" }),
    );
    assert!(store.chat_room("R1").is_none());
    assert!(store.chat_event("R1", 1).is_none());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_frame_is_passed_over() {
    let (store, dir) = store("bad");
    keep_frame(&store, ME, "not json");
    keep_frame(&store, ME, "{\"topic\":\"chatRoom:R1\",\"message\":\"{broken\"}");
    push(
        &store,
        "chatRoom:R1",
        json!({ "kind": "event", "event": { "type": "message" } }),
    );
    push(&store, "shellCounts", json!({ "chat": 3 }));
    // a message as JSON text, as the server may send it
    keep_frame(
        &store,
        ME,
        &json!({ "topic": "chatRoom:R1", "message": json!({ "kind": "event", "event": { "eventSeq": 4, "type": "message" } }).to_string() })
            .to_string(),
    );
    assert_eq!(latest(&store), [1, 2, 3, 4]);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_encrypted_rooms_changes_fold_as_the_server_shows_them() {
    let (store, dir) = store("encrypted");
    event_push(
        &store,
        json!({ "eventSeq": 4, "type": "message", "senderUserId": "U2", "encrypted": { "c": "m4" },
        "sentDate": 40 }),
    );
    // an edit's ciphertext, with its own seq and date; no body to take
    event_push(
        &store,
        json!({ "eventSeq": 5, "type": "edit", "editsEventSeq": 4, "senderUserId": "U2",
        "content": { "encrypted": { "c": "m4b" } }, "sentDate": 50 }),
    );
    let four = kept(&store, 4);
    assert_eq!(four["encrypted"], json!({ "c": "m4b" }));
    assert_eq!(
        (four["encryptedEventSeq"].clone(), four["encryptedSentDate"].clone()),
        (json!(5), json!(50))
    );
    assert_eq!(four["edited"], json!(true));
    // an encrypted reaction: kept for the page to read, once; taken back by its seq
    let reaction = json!({ "eventSeq": 6, "type": "reaction", "relatesToEventSeq": 4, "senderUserId": ME,
        "encrypted": { "c": "r6" }, "sentDate": 60 });
    event_push(&store, reaction);
    assert_eq!(
        kept(&store, 4)["encryptedReactions"],
        json!([{ "eventSeq": 6, "senderUserId": ME, "sentDate": 60, "encrypted": { "c": "r6" } }])
    );
    event_push(
        &store,
        json!({ "eventSeq": 7, "type": "redaction", "relatesToEventSeq": 6, "senderUserId": ME,
        "content": { "target": "reaction", "messageSeq": 4 } }),
    );
    assert_eq!(kept(&store, 4)["encryptedReactions"], json!([]));
    // redacted: no ciphertext or content stays
    store
        .put_chat_events(&[chat_event(
            "R1",
            &json!({ "eventSeq": 3, "encrypted": { "c": "m3" }, "poll": { "q": 1 },
            "forwardOf": { "eventSeq": 1 }, "linkPreviews": [{ "url": "u" }], "assist": { "a": 1 },
            "encryptedReactions": [{ "eventSeq": 9 }] }),
        )
        .unwrap()])
        .unwrap();
    event_push(
        &store,
        json!({ "eventSeq": 8, "type": "redaction", "relatesToEventSeq": 3, "sentDate": 80 }),
    );
    let three = kept(&store, 3);
    for field in [
        "encrypted",
        "encryptedEventSeq",
        "encryptedSentDate",
        "poll",
        "forwardOf",
        "assist",
    ] {
        assert_eq!(three[field], Value::Null, "{field}");
    }
    assert_eq!(
        (three["linkPreviews"].clone(), three["encryptedReactions"].clone()),
        (json!([]), json!([]))
    );
    assert_eq!(three["redacted"], json!(true));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_late_copy_of_an_applied_push_changes_nothing() {
    let (store, dir) = store("late-copy");
    let react = |seq: i64| {
        json!({ "eventSeq": seq, "type": "reaction", "relatesToEventSeq": 2, "senderUserId": "U2",
        "content": { "key": "+1" } })
    };
    let take_back = json!({ "eventSeq": 5, "type": "redaction", "senderUserId": "U2",
        "content": { "target": "reaction", "messageSeq": 2, "key": "+1" } });
    // one socket: react, take back; the other socket's late copy of the reaction comes after
    event_push(&store, react(4));
    event_push(&store, take_back);
    event_push(&store, react(4));
    assert_eq!(kept(&store, 2)["reactions"], json!([]));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pushed_number_next_after_the_cursor_moves_it_and_a_gap_asks_for_the_feeds() {
    let (store, dir) = store("gaps");
    let frame = |topic: &str, message: Value| json!({ "topic": topic, "message": message }).to_string();
    let user = |message: Value| frame("chatUser:ME", message);
    // no cursor (an older server, or before the first read): nothing is asked
    let read = |seq: i64| user(json!({ "kind": "read", "chatRoomId": "R1", "lastReadEventSeq": 3, "changeSeq": seq }));
    assert!(!keep_frame(&store, ME, &read(9)));
    store.set_sync_seq(crate::sync::feeds::CHAT_CURSOR, 4).unwrap();
    // the next read is folded whole: the cursor moves on
    assert!(!keep_frame(&store, ME, &read(5)));
    assert_eq!(store.sync_seq(crate::sync::feeds::CHAT_CURSOR), Some(5));
    // one seen already changes no cursor; a room joined needs its row; a gap needs the feed
    assert!(!keep_frame(&store, ME, &read(5)));
    let joined = user(json!({ "kind": "room", "action": "joined", "chatRoomId": "R2", "changeSeq": 6 }));
    assert!(keep_frame(&store, ME, &joined));
    assert!(keep_frame(&store, ME, &read(8)));
    assert_eq!(store.sync_seq(crate::sync::feeds::CHAT_CURSOR), Some(5));
    // a room's message change: the next moves the room's cursor, a gap asks
    store.set_sync_seq("room:R1", 2).unwrap();
    let edit = json!({ "kind": "event", "chatRoomId": "R1", "eventSeq": 4, "roomChangeSeq": 3,
        "event": { "eventSeq": 4, "type": "edit", "editsEventSeq": 1, "body": "new", "senderUserId": "U2" } });
    assert!(!keep_frame(&store, ME, &frame("chatRoom:R1", edit)));
    assert_eq!(store.sync_seq("room:R1"), Some(3));
    assert_eq!(kept(&store, 1)["body"], "new");
    let update = json!({ "kind": "update", "chatRoomId": "R1", "eventSeq": 2, "roomChangeSeq": 7,
        "fields": { "linkPreviews": [] } });
    assert!(keep_frame(&store, ME, &frame("chatRoom:R1", update)));
    assert_eq!(store.sync_seq("room:R1"), Some(3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_left_goes_with_all_kept_of_it() {
    let (store, dir) = store("left");
    let meta = crate::offline::ROOM_META;
    store
        .put_answer(meta, "chatRoomId=R1", "application/json", b"{}")
        .unwrap();
    store
        .put_answer(
            "/apps/BasicComms/Chat/timeline",
            "chatRoomId=R1&limit=50",
            "application/json",
            b"{}",
        )
        .unwrap();
    store
        .put_answer(
            "/apps/BasicComms/Chat/timeline",
            "chatRoomId=R2&limit=50",
            "application/json",
            b"{}",
        )
        .unwrap();
    store.set_sync_seq("pending:latest:R1", 0).unwrap();
    push(
        &store,
        "chatUser:ME",
        json!({ "kind": "room", "action": "left", "chatRoomId": "R1" }),
    );
    assert!(store.chat_room("R1").is_none() && store.chat_event("R1", 1).is_none());
    assert!(store.answer(meta, "chatRoomId=R1").is_none());
    assert!(store
        .answer("/apps/BasicComms/Chat/timeline", "chatRoomId=R1&limit=50")
        .is_none());
    assert!(store
        .answer("/apps/BasicComms/Chat/timeline", "chatRoomId=R2&limit=50")
        .is_some());
    assert!(store.sync_cursor("pending:latest:R1").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
