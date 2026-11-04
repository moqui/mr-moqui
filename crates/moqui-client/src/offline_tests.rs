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

use std::path::PathBuf;

use crate::store::Store;
use serde_json::{json, Value};

use super::*;

const TIMELINE: &str = "/apps/BasicComms/Chat/timeline";
const THREADS: &str = "/apps/BasicComms/Inbox/Mail/threads";

fn store(name: &str) -> (Store, PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-offline-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::open(&dir, "a", 1 << 24).unwrap(), dir)
}

fn events(seqs: std::ops::RangeInclusive<i64>) -> Vec<Value> {
    seqs.map(|seq| json!({ "eventSeq": seq, "type": "message", "senderUserId": "U1", "body": format!("m{seq}") }))
        .collect()
}

fn page(seqs: std::ops::RangeInclusive<i64>, more_before: bool, names: Value) -> Vec<u8> {
    json!({ "room": { "chatRoomId": "R1", "roomName": "Ops" }, "access": { "canPost": true }, "eventList": events(seqs),
        "hasMoreBefore": more_before, "hasMoreAfter": false, "lastReadEventSeq": 9, "memberList": [{ "userId": "U1" }],
        "userNames": names })
    .to_string()
    .into_bytes()
}

fn offline(store: &Store, kind: Kind, bare: &str, query: &str) -> Option<Value> {
    answer(store, kind, bare, query).map(|body| serde_json::from_slice(&body).unwrap())
}

fn seqs(page: &Value) -> Vec<i64> {
    page["eventList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["eventSeq"].as_i64().unwrap())
        .collect()
}

#[test]
fn the_comms_transitions_are_known_under_the_servers_comms_app() {
    let root = DEFAULT_COMMS_ROOT;
    assert_eq!(classify("/qapps/clientConfig", root), Some(Kind::Shell));
    assert_eq!(classify("/shellCounts", root), Some(Kind::Shell));
    assert_eq!(classify("/apps/BasicComms/Chat/timeline", root), Some(Kind::Timeline));
    assert_eq!(classify("/qapps/BasicComms/Chat/chatData", root), Some(Kind::ChatData));
    assert_eq!(
        classify("/apps/BasicComms/Inbox/Mail/thread", root),
        Some(Kind::MailThread)
    );
    assert_eq!(
        classify("/apps/BasicComms/Inbox/Compose/composeData", root),
        Some(Kind::Compose)
    );
    assert_eq!(classify("/apps/BasicComms/Chat/send", root), None);
    assert_eq!(classify("/apps/BasicComms/Chat/files", root), None);
    assert_eq!(classify("/apps/Other/Chat/timeline", root), None);
    // another app mounts the comms screens
    assert_eq!(
        classify("/apps/boss/Comms/Chat/rooms", "/qapps/boss/Comms"),
        Some(Kind::Rooms)
    );
    assert_eq!(classify("/apps/BasicComms/Chat/rooms", "/qapps/boss/Comms"), None);
    assert_eq!(chat_base("/qapps/boss/Comms/"), "/apps/boss/Comms/Chat");
    assert_eq!(mail_base(DEFAULT_COMMS_ROOT), "/apps/BasicComms/Inbox/Mail");
}

#[test]
fn volatile_parameters_are_not_part_of_an_answers_key() {
    assert_eq!(
        kept_query(
            Kind::Timeline,
            "?chatRoomId=R1&_=1700000000&moquiSessionToken=t&limit=50&pageTraceId=p"
        ),
        "chatRoomId=R1&limit=50"
    );
    assert_eq!(kept_query(Kind::Shell, "_=1"), "");
    let (store, dir) = store("volatile");
    keep(
        &store,
        Kind::Compose,
        "/apps/BasicComms/Inbox/Compose/composeData",
        "to=a%40b.c&_=1",
        b"{\"compose\":{}}",
    );
    let kept = offline(
        &store,
        Kind::Compose,
        "/apps/BasicComms/Inbox/Compose/composeData",
        "_=2&to=a%40b.c",
    );
    assert_eq!(kept, Some(json!({ "compose": {} })));
    assert_eq!(
        offline(
            &store,
            Kind::Compose,
            "/apps/BasicComms/Inbox/Compose/composeData",
            "to=x"
        ),
        None
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn timeline_pages_answer_offline_within_what_is_held_and_stop_at_a_gap() {
    let (store, dir) = store("timeline");
    // the latest page: 8..=10, older ones on the server
    keep(
        &store,
        Kind::Timeline,
        TIMELINE,
        "chatRoomId=R1&limit=3",
        &page(8..=10, true, json!({ "U1": "Ann" })),
    );
    let latest = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&limit=50&_=5").unwrap();
    assert_eq!(seqs(&latest), [8, 9, 10]);
    // older events on the server, none held: the screen is told by the gap, not asked to load them
    assert_eq!(latest["hasMoreBefore"], json!(false));
    assert_eq!(latest["hasMoreAfter"], json!(false));
    assert_eq!(latest["offline"], json!({ "gapBefore": true, "gapAfter": false }));
    assert_eq!(latest["room"]["roomName"], json!("Ops"));
    assert_eq!(latest["access"], json!({ "canPost": true }));
    assert_eq!(latest["lastReadEventSeq"], json!(9));
    assert_eq!(latest["memberList"], json!([{ "userId": "U1" }]));
    // older than what is held: needs the server
    assert_eq!(
        offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&beforeSeq=8&limit=3"),
        None
    );

    // the older page read too, to the room's start: the pages join
    keep(
        &store,
        Kind::Timeline,
        TIMELINE,
        "chatRoomId=R1&beforeSeq=8&limit=50",
        &page(1..=7, false, json!({ "U2": "Bo" })),
    );
    let whole = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&limit=50").unwrap();
    assert_eq!(seqs(&whole), (1..=10).collect::<Vec<_>>());
    assert_eq!(whole["hasMoreBefore"], json!(false));
    assert_eq!(whole["userNames"], json!({ "U1": "Ann", "U2": "Bo" }));
    let short = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&limit=3").unwrap();
    assert_eq!(seqs(&short), [8, 9, 10]);
    assert_eq!(short["hasMoreBefore"], json!(true));
    let older = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&beforeSeq=8&limit=3").unwrap();
    assert_eq!(seqs(&older), [5, 6, 7]);
    assert_eq!(
        (older["hasMoreBefore"].clone(), older["hasMoreAfter"].clone()),
        (json!(true), json!(true))
    );
    let newer = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&afterSeq=6&limit=2").unwrap();
    assert_eq!(seqs(&newer), [7, 8]);
    assert_eq!(newer["hasMoreAfter"], json!(true));

    // the socket dropped: the newest held is the latest offline, the newer ones a gap
    store.mark_chat_heads_stale().unwrap();
    let stale = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&limit=3").unwrap();
    assert_eq!(seqs(&stale), [8, 9, 10]);
    assert_eq!(stale["hasMoreAfter"], json!(false));
    assert_eq!(stale["offline"]["gapAfter"], json!(true));
    assert_eq!(
        offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&afterSeq=10"),
        None
    );
    // around a seq: the kept answer only, of that request
    assert_eq!(
        offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&aroundSeq=5"),
        None
    );
    keep(
        &store,
        Kind::Timeline,
        TIMELINE,
        "chatRoomId=R1&aroundSeq=5",
        &page(4..=6, true, json!({})),
    );
    assert_eq!(
        seqs(&offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&aroundSeq=5").unwrap()),
        [4, 5, 6]
    );
    // another room: nothing held
    assert_eq!(offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R2"), None);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_views_first_page_and_a_threads_replies_are_kept_as_rows() {
    let (store, dir) = store("room-view");
    let view = json!({ "room": { "chatRoomId": "R1" }, "member": { "lastReadEventSeq": 2 }, "access": {}, "memberList": [],
        "timeline": serde_json::from_slice::<Value>(&page(1..=3, false, json!({ "U1": "Ann" }))).unwrap() });
    keep(
        &store,
        Kind::RoomView,
        "/apps/BasicComms/Chat/roomView",
        "chatRoomId=R1",
        view.to_string().as_bytes(),
    );
    let latest = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1").unwrap();
    assert_eq!(seqs(&latest), [1, 2, 3]);
    assert_eq!(latest["offline"]["gapBefore"], json!(false));
    // a thread's page: the root and its replies, apart from the main timeline
    let thread = json!({ "eventList": [{ "eventSeq": 2 }, { "eventSeq": 4, "threadRootEventSeq": 2 }],
        "hasMoreBefore": false, "hasMoreAfter": false });
    keep(
        &store,
        Kind::Timeline,
        TIMELINE,
        "chatRoomId=R1&threadRootEventSeq=2",
        thread.to_string().as_bytes(),
    );
    let replies = offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1&threadRootEventSeq=2").unwrap();
    assert_eq!(seqs(&replies), [2, 4]);
    assert_eq!(
        seqs(&offline(&store, Kind::Timeline, TIMELINE, "chatRoomId=R1").unwrap()),
        [1, 2, 3]
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_whole_room_list_replaces_the_rooms_and_a_cut_one_only_adds() {
    let (store, dir) = store("rooms");
    let rooms = |ids: &[&str]| {
        json!({ "roomList": ids.iter().enumerate().map(|(at, id)| json!({ "chatRoomId": id,
            "lastMessage": { "eventSeq": 5, "sentDate": 1000 + at } })).collect::<Vec<_>>() })
        .to_string()
        .into_bytes()
    };
    keep(
        &store,
        Kind::ChatData,
        "/apps/BasicComms/Chat/chatData",
        "",
        &rooms(&["R1", "R2", "R3"]),
    );
    assert_eq!(store.chat_rooms(10).len(), 3);
    assert_eq!(store.chat_room("R3").unwrap().sort_ms, 1002);
    assert_eq!(store.chat_room("R3").unwrap().last_seq, 5);
    // the panel's 2 of more: none removed
    keep(
        &store,
        Kind::ChatData,
        "/apps/BasicComms/Chat/chatData",
        "limit=2",
        &rooms(&["R1", "R2"]),
    );
    assert_eq!(store.chat_rooms(10).len(), 3);
    // a filtered list: not the room list
    keep(
        &store,
        Kind::Rooms,
        "/apps/BasicComms/Chat/rooms",
        "filter=unread",
        &rooms(&["R1"]),
    );
    assert_eq!(store.chat_rooms(10).len(), 3);
    // the whole list again, without R3 (left)
    keep(
        &store,
        Kind::Rooms,
        "/apps/BasicComms/Chat/rooms",
        "",
        &rooms(&["R1", "R2"]),
    );
    assert!(store.chat_room("R3").is_none());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

fn threads_page(rows: &[(&str, i64)], more: bool) -> Vec<u8> {
    let list: Vec<Value> = rows
        .iter()
        .map(|(id, date)| {
            json!({ "store": "email", "threadId": id, "rowKey": format!("email:{id}"), "sortDate": date,
            "subject": format!("s{id}") })
        })
        .collect();
    let next = rows.last().filter(|_| more).map(|(_, date)| format!("email:{date}"));
    json!({ "threadList": list, "hasMore": more, "nextCursor": next, "storeList": ["email"], "failedStores": [],
        "termList": [], "conversationView": "threads" })
    .to_string()
    .into_bytes()
}

#[test]
fn a_folders_thread_pages_answer_offline_in_the_servers_shape() {
    let (store, dir) = store("threads");
    let first = "view=inbox&account=&folder=&label=&mailbox=&q=&unreadOnly=false&pageSize=2";
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        first,
        &threads_page(&[("T3", 300), ("T2", 200)], true),
    );
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        &format!("{first}&cursor=email%3A200"),
        &threads_page(&[("T1", 100)], false),
    );
    let page = offline(&store, Kind::MailThreads, THREADS, &format!("{first}&_=9")).unwrap();
    let ids: Vec<&str> = page["threadList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["threadId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["T3", "T2"]);
    assert_eq!(page["hasMore"], json!(true));
    assert_eq!(page["nextCursor"], json!("email:200"));
    assert_eq!(page["conversationView"], json!("threads"));
    assert_eq!(page["storeList"], json!(["email"]));
    assert_eq!(page["offline"], json!(true));
    let next = offline(&store, Kind::MailThreads, THREADS, &format!("{first}&cursor=email:200")).unwrap();
    assert_eq!(next["threadList"].as_array().unwrap().len(), 1);
    assert_eq!(
        (next["hasMore"].clone(), next["nextCursor"].clone()),
        (json!(false), Value::Null)
    );
    // the same folder asked as the server takes it: no view is the inbox
    let bare = offline(&store, Kind::MailThreads, THREADS, "pageSize=50").unwrap();
    assert_eq!(bare["threadList"].as_array().unwrap().len(), 3);
    // a search: the kept answer of that search only
    assert_eq!(offline(&store, Kind::MailThreads, THREADS, "q=invoice"), None);
    // another folder: nothing held
    assert_eq!(offline(&store, Kind::MailThreads, THREADS, "account=A&folder=F9"), None);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thread_and_the_folders_are_kept_as_rows() {
    let (store, dir) = store("mail-rows");
    let thread = json!({ "thread": { "store": "email", "threadId": "T1", "messageList": [
        { "key": "email:M2", "emailMessageId": "M2", "date": 200 },
        { "key": "email:M1", "emailMessageId": "M1", "date": 100 }] } });
    keep(
        &store,
        Kind::MailThread,
        "/apps/BasicComms/Inbox/Mail/thread",
        "store=email&threadId=T1",
        thread.to_string().as_bytes(),
    );
    let messages: Vec<String> = store
        .mail_messages("email:T1")
        .into_iter()
        .map(|message| message.message_id)
        .collect();
    assert_eq!(messages, ["email:M1", "email:M2"]);
    // offline, the thread is built from its fields and its messages, oldest first
    let built = offline(
        &store,
        Kind::MailThread,
        "/apps/BasicComms/Inbox/Mail/thread",
        "threadId=T1&store=email",
    )
    .unwrap();
    assert_eq!(built["thread"]["threadId"], json!("T1"));
    let dates: Vec<i64> = built["thread"]["messageList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["date"].as_i64().unwrap())
        .collect();
    assert_eq!(dates, [100, 200]);
    assert_eq!(built["offline"], json!(true));
    let inbox = json!({ "accountList": [{ "emailAccountId": "A", "folderList": [{ "emailFolderId": "F1" }, { "emailFolderId": "F2" }] }] });
    keep(
        &store,
        Kind::Inbox,
        "/apps/BasicComms/Inbox/Mail/inboxData",
        "",
        inbox.to_string().as_bytes(),
    );
    let folders: Vec<String> = store
        .mail_folders()
        .into_iter()
        .map(|folder| folder.folder_id)
        .collect();
    assert_eq!(folders, ["account=A&folder=F1", "account=A&folder=F2"]);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_two_store_cursor_pages_without_losing_the_first_page() {
    let (store, dir) = store("two-stores");
    let rows = |list: &[(&str, &str, i64)]| -> Vec<Value> {
        list.iter()
            .map(|(kind, id, date)| json!({ "store": kind, "threadId": id, "rowKey": format!("{kind}:{id}"), "sortDate": date }))
            .collect()
    };
    let page = |list: Vec<Value>, next: Option<&str>| {
        json!({ "threadList": list, "hasMore": next.is_some(), "nextCursor": next, "storeList": ["email", "internal"],
            "conversationView": "threads" })
        .to_string()
        .into_bytes()
    };
    let first = "view=inbox&pageSize=2";
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        first,
        &page(
            rows(&[("email", "E3", 300), ("internal", "I3", 290)]),
            Some("email:300,internal:290"),
        ),
    );
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        &format!("{first}&cursor=email%3A300%2Cinternal%3A290"),
        &page(
            rows(&[("email", "E2", 200), ("internal", "I2", 150)]),
            Some("email:end,internal:150"),
        ),
    );
    // a store at its end: only added, nothing of the pages before removed
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        &format!("{first}&cursor=email%3Aend%2Cinternal%3A150"),
        &page(rows(&[("internal", "I1", 100)]), None),
    );
    assert_eq!(store.mail_threads("view=inbox", None, 50).len(), 5);

    // offline, paged as the server pages, with a cursor of the same form
    let ids = |page: &Value| -> Vec<String> {
        page["threadList"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["threadId"].as_str().unwrap().to_string())
            .collect()
    };
    let one = offline(&store, Kind::MailThreads, THREADS, first).unwrap();
    assert_eq!(ids(&one), ["E3", "I3"]);
    assert_eq!(one["nextCursor"], json!("email:300,internal:290"));
    let cursor = one["nextCursor"]
        .as_str()
        .unwrap()
        .replace(':', "%3A")
        .replace(',', "%2C");
    let two = offline(&store, Kind::MailThreads, THREADS, &format!("{first}&cursor={cursor}")).unwrap();
    assert_eq!(ids(&two), ["E2", "I2"]);
    assert_eq!(two["nextCursor"], json!("email:end,internal:150"));
    let three = offline(
        &store,
        Kind::MailThreads,
        THREADS,
        &format!("{first}&cursor=email:end,internal:150"),
    )
    .unwrap();
    assert_eq!(ids(&three), ["I1"]);
    assert_eq!(
        (three["hasMore"].clone(), three["nextCursor"].clone()),
        (json!(false), Value::Null)
    );
    assert_eq!(
        offline(
            &store,
            Kind::MailThreads,
            THREADS,
            &format!("{first}&cursor=email:end,internal:end")
        )
        .unwrap()["threadList"],
        json!([])
    );
    // a cursor of another form: the kept answer only
    assert_eq!(
        offline(&store, Kind::MailThreads, THREADS, &format!("{first}&cursor=page2")),
        None
    );

    // a page whose cursor has every store's date replaces below the earliest: I2, gone on the server, goes
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        &format!("{first}&cursor=email%3A300%2Cinternal%3A290"),
        &page(
            rows(&[("email", "E2", 200), ("internal", "I1", 100)]),
            Some("email:200,internal:100"),
        ),
    );
    let left: Vec<String> = store
        .mail_threads("view=inbox", None, 50)
        .into_iter()
        .map(|row| row.thread_id)
        .collect();
    assert_eq!(left, ["email:E3", "internal:I3", "email:E2", "internal:I1"]);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_change_of_conversation_view_empties_the_folder() {
    let (store, dir) = store("view-change");
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        "pageSize=2",
        &threads_page(&[("T3", 300), ("T2", 200)], true),
    );
    let messages = json!({ "threadList": [{ "store": "email", "threadId": "T3", "rowKey": "email:M9", "sortDate": 50 }],
        "hasMore": true, "nextCursor": "email:50", "storeList": ["email"], "conversationView": "messages" });
    keep(
        &store,
        Kind::MailThreads,
        THREADS,
        "pageSize=1&cursor=email%3A60",
        messages.to_string().as_bytes(),
    );
    let left: Vec<String> = store
        .mail_threads("view=inbox", None, 50)
        .into_iter()
        .map(|row| row.thread_id)
        .collect();
    assert_eq!(left, ["email:M9"]);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_view_and_the_room_list_answer_offline_from_the_rows() {
    let (store, dir) = store("room-view-offline");
    let chat_data = json!({ "me": { "userId": "ME" }, "prefs": { "enter": true }, "roomList": [
        { "chatRoomId": "R1", "isPinned": false, "unreadCount": 2, "lastMessage": { "eventSeq": 3, "sentDate": 300 } },
        { "chatRoomId": "R2", "isPinned": true, "unreadCount": 1, "mentionCount": 1, "lastMessage": { "eventSeq": 1, "sentDate": 100 } }] });
    keep(
        &store,
        Kind::ChatData,
        "/apps/BasicComms/Chat/chatData",
        "limit=200",
        chat_data.to_string().as_bytes(),
    );
    // the panel's other limit, never asked online: built from the rows, pinned first
    let rooms = offline(&store, Kind::ChatData, "/apps/BasicComms/Chat/chatData", "limit=40").unwrap();
    let ids: Vec<&str> = rooms["roomList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|room| room["chatRoomId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["R2", "R1"]);
    assert_eq!(
        (rooms["unreadTotal"].clone(), rooms["mentionTotal"].clone()),
        (json!(3), json!(1))
    );
    assert_eq!(rooms["me"], json!({ "userId": "ME" }));
    assert_eq!(
        offline(&store, Kind::Rooms, "/apps/BasicComms/Chat/rooms", "").unwrap()["roomList"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // a room the mirror read by its timeline: Chat opens it by roomView, which is built
    keep(
        &store,
        Kind::Timeline,
        TIMELINE,
        "chatRoomId=R1&limit=50",
        &page(1..=3, false, json!({ "U1": "Ann" })),
    );
    let view = json!({ "room": { "chatRoomId": "R1" }, "member": { "lastReadEventSeq": 1 }, "access": {}, "memberList": [],
        "timeline": serde_json::from_slice::<Value>(&page(1..=3, false, json!({}))).unwrap() });
    keep(
        &store,
        Kind::Room,
        "/apps/BasicComms/Chat/room",
        "chatRoomId=R1",
        view.to_string().as_bytes(),
    );
    let built = offline(
        &store,
        Kind::RoomView,
        "/apps/BasicComms/Chat/roomView",
        "chatRoomId=R1&aroundSeq=&limit=50",
    )
    .unwrap();
    assert_eq!(built["room"]["chatRoomId"], json!("R1"));
    assert_eq!(built["member"]["lastReadEventSeq"], json!(1));
    assert_eq!(seqs(&built["timeline"]), [1, 2, 3]);
    assert_eq!(built["timeline"]["userNames"], json!({ "U1": "Ann" }));
    // a read marker pushed since shows
    super::note_read(&store, "R1", &json!(3)).unwrap();
    let read = offline(
        &store,
        Kind::RoomView,
        "/apps/BasicComms/Chat/roomView",
        "chatRoomId=R1",
    )
    .unwrap();
    assert_eq!(read["member"]["lastReadEventSeq"], json!(3));
    assert_eq!(read["timeline"]["lastReadEventSeq"], json!(3));
    // a room never read: nothing to build
    assert_eq!(
        offline(
            &store,
            Kind::RoomView,
            "/apps/BasicComms/Chat/roomView",
            "chatRoomId=R2"
        ),
        None
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

const COMPOSE: &str = "/apps/BasicComms/Inbox/Compose/composeData";

/// The new message's `composeData`, kept as the mirror keeps it: two identities of account A, and the internal one.
fn keep_compose_base(store: &Store) {
    let base = json!({ "compose": { "identityList": [
            { "value": "account:A", "emailAccountId": "A", "emailAddress": "me@ex.com", "isDefault": true },
            { "value": "identity:A2", "emailAccountId": "A", "emailAddress": "sales@ex.com" },
            { "value": "internal", "emailAccountId": null, "isInternal": true }],
        "fromValue": "internal", "me": { "userId": "U1" }, "to": [], "cc": [], "bcc": [], "subject": "", "body": "",
        "quoteHtml": null, "emailMessageId": null, "parentEmailMessageId": null, "isForward": false, "record": null,
        "attachmentList": [], "undoSeconds": 10, "signaturePlacement": "above", "threadStore": null, "threadId": null,
        "kind": "new" } });
    keep(store, Kind::Compose, COMPOSE, "", base.to_string().as_bytes());
}

/// A thread of root R1: Pat's message M1 to me (on sales@), Lee and Kim, and my draft M2 replying to it.
fn keep_mail_thread(store: &Store) {
    let thread = json!({ "thread": { "store": "email", "threadId": "R1", "subject": "Lunch", "messageList": [
        { "key": "email:M1", "emailMessageId": "M1", "emailAccountId": "A",
            "from": { "address": "pat@ex.com", "name": "Pat", "isMe": false },
            "toList": [{ "address": "sales@ex.com", "isMe": true }, { "address": "lee@ex.com", "name": "Lee" }],
            "ccList": [{ "address": "kim@ex.com", "name": null }, { "address": "pat@ex.com" }],
            "date": 1_700_000_000_000_i64, "subject": "Lunch", "html": "<p>Noon?</p><script>steal()</script>",
            "text": null, "attachmentList": [{ "id": "F0", "name": "map.png" }] },
        { "key": "email:M2", "emailMessageId": "M2", "emailAccountId": "A",
            "from": { "address": "sales@ex.com", "isMe": true },
            "toList": [{ "address": "pat@ex.com", "name": "Pat" }], "ccList": [], "bccList": [{ "address": "boss@ex.com" }],
            "date": 1_700_000_100_000_i64, "subject": "Re: Lunch", "isDraft": true, "statusId": "ES_DRAFT", "canSend": true,
            "html": "<p>Yes</p><div class=\"ironstone-quote\"><p>On …</p></div>", "text": null,
            "attachmentList": [{ "id": "F1", "name": "menu.pdf", "size": 10, "contentType": "application/pdf" }] }] } });
    keep(
        store,
        Kind::MailThread,
        "/apps/BasicComms/Inbox/Mail/thread",
        "store=email&threadId=R1",
        thread.to_string().as_bytes(),
    );
}

fn addresses(chips: &Value) -> Vec<String> {
    chips
        .as_array()
        .unwrap()
        .iter()
        .map(|chip| chip["value"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_new_message_answers_offline_from_the_kept_new_messages_compose_data() {
    let (store, dir) = store("compose-new");
    // nothing kept: the scheme answers 502 and Compose starts a blank message
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, ""), None);
    keep_compose_base(&store);
    let base = offline(&store, Kind::Compose, COMPOSE, "_=1").unwrap();
    assert_eq!(base["compose"]["kind"], "new");
    assert_eq!(base["compose"]["identityList"].as_array().unwrap().len(), 3);
    // mailto-style values on it, the body's HTML cleaned
    let mailto = offline(
        &store,
        Kind::Compose,
        COMPOSE,
        "to=%22Pat%20Q%22%20%3Cpat%40ex.com%3E%3B%20mailto%3Alee%40ex.com&subject=Hi&body=%3Cb%3Eyo%3C%2Fb%3E%3Cscript%3Ex()%3C%2Fscript%3E",
    )
    .unwrap();
    let compose = &mailto["compose"];
    assert_eq!(addresses(&compose["to"]), ["\"Pat Q\" <pat@ex.com>", "lee@ex.com"]);
    assert_eq!(compose["to"][0]["label"], "Pat Q <pat@ex.com>");
    assert_eq!(compose["subject"], "Hi");
    assert_eq!(compose["body"], "<b>yo</b>");
    assert_eq!(compose["fromValue"], "internal");
    assert_eq!(mailto["offline"], true);
    // an internal reply or a mailbox post needs the server
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, "replyToCommEventId=CE1"), None);
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, "contactListId=L1"), None);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reply_or_forward_is_built_from_the_kept_message() {
    let (store, dir) = store("compose-reply");
    keep_compose_base(&store);
    keep_mail_thread(&store);
    let reply = offline(
        &store,
        Kind::Compose,
        COMPOSE,
        "replyToEmailMessageId=M1&replyAll=false",
    )
    .unwrap();
    let compose = &reply["compose"];
    assert_eq!(reply["offline"], true);
    assert_eq!(compose["kind"], "reply");
    // from the identity it came to, to its sender
    assert_eq!(compose["fromValue"], "identity:A2");
    assert_eq!(addresses(&compose["to"]), ["\"Pat\" <pat@ex.com>"]);
    assert_eq!(addresses(&compose["cc"]), Vec::<String>::new());
    assert_eq!(compose["subject"], "Re: Lunch");
    assert_eq!(compose["parentEmailMessageId"], "M1");
    assert_eq!(compose["isForward"], false);
    assert_eq!(
        (&compose["threadStore"], &compose["threadId"]),
        (&json!("email"), &json!("R1"))
    );
    let quote = compose["quoteHtml"].as_str().unwrap();
    assert!(quote.starts_with("<div class=\"ironstone-quote\"><p>On "), "{quote}");
    assert!(
        quote.contains(", Pat &lt;pat@ex.com&gt; wrote:</p><blockquote><p>Noon?</p>"),
        "{quote}"
    );
    assert!(!quote.contains("script"), "{quote}");

    // reply all: the other recipients but me, each once
    let all = offline(&store, Kind::Compose, COMPOSE, "replyToEmailMessageId=M1&replyAll=true").unwrap();
    assert_eq!(all["compose"]["kind"], "replyAll");
    assert_eq!(
        addresses(&all["compose"]["to"]),
        ["\"Pat\" <pat@ex.com>", "\"Lee\" <lee@ex.com>"]
    );
    assert_eq!(addresses(&all["compose"]["cc"]), ["kim@ex.com"]);

    // a forward: no draft offline, its first save makes it, without the message's file, which is said
    let forward = offline(&store, Kind::Compose, COMPOSE, "forwardEmailMessageId=M1").unwrap();
    assert_eq!(forward["offline"], json!({ "forwardFilesMissing": 1 }));
    let compose = &forward["compose"];
    assert_eq!(compose["kind"], "forward");
    assert_eq!(compose["subject"], "Fwd: Lunch");
    assert_eq!(compose["isForward"], true);
    assert_eq!(compose["emailMessageId"], Value::Null);
    assert_eq!(compose["to"], json!([]));
    let quote = compose["quoteHtml"].as_str().unwrap();
    assert!(
        quote.contains("---------- Forwarded message ---------<br>From: Pat &lt;pat@ex.com&gt;<br>"),
        "{quote}"
    );
    assert!(
        quote.contains("Subject: Lunch<br>To: sales@ex.com, Lee &lt;lee@ex.com&gt;</p><p>Noon?</p></div>"),
        "{quote}"
    );

    // the server's own answer of the same reply comes first; a forward's (a draft made then) does not
    let server_reply = json!({ "compose": { "kind": "reply", "subject": "Re: Lunch (server)" } }).to_string();
    keep(
        &store,
        Kind::Compose,
        COMPOSE,
        "replyToEmailMessageId=M1&replyAll=false",
        server_reply.as_bytes(),
    );
    let kept = offline(
        &store,
        Kind::Compose,
        COMPOSE,
        "replyToEmailMessageId=M1&replyAll=false",
    )
    .unwrap();
    assert_eq!(kept["compose"]["subject"], "Re: Lunch (server)");
    let server_forward = json!({ "compose": { "kind": "forward", "emailMessageId": "OLD" } }).to_string();
    keep(
        &store,
        Kind::Compose,
        COMPOSE,
        "forwardEmailMessageId=M1",
        server_forward.as_bytes(),
    );
    let built = offline(&store, Kind::Compose, COMPOSE, "forwardEmailMessageId=M1").unwrap();
    assert_eq!(built["compose"]["emailMessageId"], Value::Null);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reply_to_a_message_not_kept_is_a_new_message_answering_it() {
    let (store, dir) = store("compose-missing");
    // no new message kept either: none, and Compose starts a blank one
    assert_eq!(
        offline(
            &store,
            Kind::Compose,
            COMPOSE,
            "replyToEmailMessageId=GONE&replyAll=false"
        ),
        None
    );
    keep_compose_base(&store);
    let reply = offline(
        &store,
        Kind::Compose,
        COMPOSE,
        "replyToEmailMessageId=GONE&replyAll=false",
    )
    .unwrap();
    assert_eq!(reply["offline"], json!({ "messageMissing": "GONE" }));
    let compose = &reply["compose"];
    assert_eq!(compose["kind"], "reply");
    assert_eq!(compose["parentEmailMessageId"], "GONE");
    assert_eq!(compose["to"], json!([]));
    assert_eq!(compose["subject"], "");
    assert_eq!(compose["quoteHtml"], Value::Null);
    assert_eq!(compose["identityList"].as_array().unwrap().len(), 3);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_draft_opens_offline_from_its_kept_message_or_the_draft_kept_here() {
    let (store, dir) = store("compose-draft");
    keep_compose_base(&store);
    keep_mail_thread(&store);
    store
        .put_draft(
            "ironstone.compose.email.M2",
            &json!({ "email": true, "clientRef": "mREF", "draft": { "subject": "edited" } }).to_string(),
            &[],
        )
        .unwrap();
    let draft = offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M2").unwrap();
    let compose = &draft["compose"];
    assert_eq!(compose["kind"], "draft");
    assert_eq!(compose["fromValue"], "identity:A2");
    assert_eq!(addresses(&compose["to"]), ["\"Pat\" <pat@ex.com>"]);
    assert_eq!(addresses(&compose["bcc"]), ["boss@ex.com"]);
    assert_eq!(compose["subject"], "Re: Lunch");
    assert_eq!(compose["body"], "<p>Yes</p>");
    assert_eq!(compose["quoteHtml"], "<div class=\"ironstone-quote\"><p>On …</p></div>");
    assert_eq!(compose["attachmentList"][0]["name"], "menu.pdf");
    assert_eq!(compose["clientRef"], "mREF");
    assert_eq!(compose["threadId"], "R1");

    // a kept message that is not a draft (sent since): none, though a draft and an answer of it are kept here
    store
        .put_draft(
            "ironstone.compose.email.M1",
            &json!({ "email": true, "clientRef": "mSENT", "draft": { "subject": "Sent since" } }).to_string(),
            &[],
        )
        .unwrap();
    keep(
        &store,
        Kind::Compose,
        COMPOSE,
        "emailMessageId=M1",
        br#"{"compose":{"kind":"draft"}}"#,
    );
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M1"), None);

    // no message kept: the draft kept here
    store
        .put_draft(
            "ironstone.compose.email.M5",
            &json!({ "email": true, "clientRef": "mOTHER", "draft": { "fromValue": "account:A",
                "to": [{ "value": "lee@ex.com", "label": "lee@ex.com", "kind": "address", "locked": false }],
                "subject": "Kept", "body": "<p>here</p>", "quoteHtml": null } })
            .to_string(),
            &[],
        )
        .unwrap();
    let local = offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M5").unwrap();
    let compose = &local["compose"];
    assert_eq!(compose["kind"], "draft");
    assert_eq!(compose["emailMessageId"], "M5");
    assert_eq!(compose["subject"], "Kept");
    assert_eq!(addresses(&compose["to"]), ["lee@ex.com"]);
    assert_eq!(compose["clientRef"], "mOTHER");
    assert_eq!(compose["identityList"].as_array().unwrap().len(), 3);

    // neither: it needs the server
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M9"), None);
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_kept_drafts_answer_gives_what_its_row_does_not_carry() {
    let (store, dir) = store("compose-draft-answer");
    keep_compose_base(&store);
    keep_mail_thread(&store);
    let answer = json!({ "compose": { "kind": "draft", "emailMessageId": "M2", "parentEmailMessageId": "M1",
        "isForward": false, "record": { "recordId": "O1" }, "clientRef": "mSERVER", "subject": "older" } });
    keep(
        &store,
        Kind::Compose,
        COMPOSE,
        "emailMessageId=M2",
        answer.to_string().as_bytes(),
    );
    let draft = offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M2").unwrap();
    let compose = &draft["compose"];
    // the row's content, the answer's links
    assert_eq!(compose["subject"], "Re: Lunch");
    assert_eq!(compose["parentEmailMessageId"], "M1");
    assert_eq!(compose["record"]["recordId"], "O1");
    assert_eq!(compose["clientRef"], "mSERVER");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_no_new_message_kept_another_compose_answers_identities_serve() {
    let (store, dir) = store("compose-identities");
    store
        .put_draft(
            "ironstone.compose.email.M5",
            &json!({ "email": true, "clientRef": "mX", "draft": { "subject": "Kept" } }).to_string(),
            &[],
        )
        .unwrap();
    // no composeData ever kept: a draft kept here has no identities to open with, and needs the server
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M5"), None);
    assert_eq!(offline(&store, Kind::Compose, COMPOSE, "subject=Hi"), None);
    // a reply's answer read online: its identities make the new message's, From the default account
    let reply = json!({ "compose": { "identityList": [
            { "value": "internal", "isInternal": true },
            { "value": "account:A", "emailAccountId": "A", "emailAddress": "me@ex.com" },
            { "value": "account:B", "emailAccountId": "B", "emailAddress": "b@ex.com", "isDefault": true }],
        "fromValue": "account:A", "undoSeconds": 5, "kind": "reply", "subject": "Re: x" } });
    keep(
        &store,
        Kind::Compose,
        COMPOSE,
        "replyToEmailMessageId=M1&replyAll=false",
        reply.to_string().as_bytes(),
    );
    let new = offline(&store, Kind::Compose, COMPOSE, "subject=Hi").unwrap();
    let compose = &new["compose"];
    assert_eq!(compose["identityList"].as_array().unwrap().len(), 3);
    assert_eq!(compose["fromValue"], "account:B");
    assert_eq!(compose["undoSeconds"], 5);
    assert_eq!(compose["subject"], "Hi");
    assert_eq!(compose["kind"], "new");
    let draft = offline(&store, Kind::Compose, COMPOSE, "emailMessageId=M5").unwrap();
    assert_eq!(draft["compose"]["subject"], "Kept");
    assert_eq!(draft["compose"]["identityList"].as_array().unwrap().len(), 3);
    // the new message's own answer then gives its default From
    keep_compose_base(&store);
    store.remove_answer(COMPOSE, "").unwrap();
    let new = offline(&store, Kind::Compose, COMPOSE, "subject=Hi").unwrap();
    assert_eq!(new["compose"]["fromValue"], "internal");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
