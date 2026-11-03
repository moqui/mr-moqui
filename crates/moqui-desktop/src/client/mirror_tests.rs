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
use std::time::Duration;

use moqui_client::conf::{ClientConf, DeviceMode};
use moqui_client::store::Store;
use serde_json::{json, Value};

use super::*;
use crate::client::offline::{self as answers, Kind, DEFAULT_COMMS_ROOT};

const NOW: i64 = 1_000 * 24 * 3600 * 1000;
const DAY: i64 = 24 * 3600 * 1000;
const CHAT: &str = "/apps/BasicComms/Chat";
const MAIL: &str = "/apps/BasicComms/Inbox/Mail";
const COMPOSE: &str = "/apps/BasicComms/Inbox/Compose/composeData";

/// A server of fixed answers that notes what was asked; anything else is a 404.
struct Server {
    answers: HashMap<String, Fetched>,
    asked: RefCell<Vec<String>>,
}

impl Transport for Server {
    fn get(&self, path: &str) -> Fetched {
        self.asked.borrow_mut().push(path.to_string());
        self.answers.get(path).cloned().unwrap_or(Fetched::Failed(404))
    }
}

fn json_of(value: Value) -> Fetched {
    Fetched::Json(value.to_string().into_bytes())
}

fn threads(view: &str, account: &str, folder: &str) -> String {
    format!("{MAIL}/threads?{}", threads_query(view, account, folder, None))
}

/// Two rooms (R1 the latest, R2 failing), and an inbox with one account's folder.
fn server() -> Server {
    let mut answers = HashMap::new();
    answers.insert(
        format!("{CHAT}/chatData?limit=200"),
        json_of(json!({ "roomList": [
            { "chatRoomId": "R2", "lastMessage": { "sentDate": NOW - 2 * DAY } },
            { "chatRoomId": "R1", "lastMessage": { "sentDate": NOW - DAY } },
            { "chatRoomId": "R9", "isMember": false }] })),
    );
    answers.insert(
        format!("{CHAT}/timeline?chatRoomId=R1&limit=50"),
        json_of(
            json!({ "eventList": [{ "eventSeq": 51, "sentDate": NOW - 10 * DAY }, { "eventSeq": 52, "sentDate": NOW }],
            "hasMoreBefore": true, "hasMoreAfter": false }),
        ),
    );
    answers.insert(
        format!("{CHAT}/timeline?chatRoomId=R1&beforeSeq=51&limit=50"),
        json_of(json!({ "eventList": [{ "eventSeq": 1, "sentDate": NOW - 200 * DAY }, { "eventSeq": 50, "sentDate": NOW - 11 * DAY }],
            "hasMoreBefore": true, "hasMoreAfter": true })),
    );
    answers.insert(format!("{CHAT}/timeline?chatRoomId=R2&limit=50"), Fetched::Failed(500));
    answers.insert(
        format!("{MAIL}/inboxData"),
        json_of(json!({ "accountList": [{ "emailAccountId": "A", "folderList": [{ "emailFolderId": "F1" }] }] })),
    );
    answers.insert(
        COMPOSE.to_string(),
        json_of(json!({ "compose": { "identityList": [{ "value": "account:A", "emailAccountId": "A" }], "fromValue": "account:A",
            "to": [], "subject": "", "kind": "new" } })),
    );
    answers.insert(
        threads("inbox", "", ""),
        json_of(
            json!({ "threadList": [{ "store": "email", "threadId": "T1", "rowKey": "email:T1", "sortDate": NOW },
            { "store": "email", "threadId": "T4", "rowKey": "email:T4", "sortDate": NOW - DAY }], "hasMore": false }),
        ),
    );
    answers.insert(
        threads("", "A", "F1"),
        json_of(
            json!({ "threadList": [{ "store": "email", "threadId": "T2", "rowKey": "email:T2", "sortDate": NOW - DAY },
            { "store": "email", "threadId": "T3", "rowKey": "email:T3", "sortDate": NOW - 400 * DAY }],
            "hasMore": true, "nextCursor": format!("email:{}", NOW - 400 * DAY) }),
        ),
    );
    for thread in ["T1", "T2", "T4"] {
        answers.insert(
            format!("{MAIL}/thread?store=email&threadId={thread}"),
            json_of(json!({ "thread": { "store": "email", "threadId": thread, "messageList": [{ "key": format!("email:M{thread}"), "date": NOW }] } })),
        );
    }
    Server {
        answers,
        asked: RefCell::new(Vec::new()),
    }
}

fn conf() -> ClientConf {
    ClientConf {
        mirror_days: 30,
        prefetch_rooms: 1,
        prefetch_mail: 1,
        ..ClientConf::default()
    }
}

/// Run a mirror of `mode` on `server` into `store`.
fn mirror(server: &Server, store: &Store, mode: DeviceMode, cancelled: &dyn Fn() -> bool) -> Ended {
    let keep = |path: &str, body: &[u8]| {
        let (bare, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(kind) = answers::classify(bare, DEFAULT_COMMS_ROOT) {
            answers::keep(store, kind, bare, query, body);
        }
    };
    run(&Job {
        transport: server,
        keep: &keep,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        mode,
        conf: conf(),
        now_ms: NOW,
        cancelled,
        pause: Duration::ZERO,
        heads_only: false,
    })
}

fn store(name: &str) -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-mirror-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Store::open(&dir, "a", 1 << 24).unwrap(), dir)
}

#[test]
fn a_personal_device_mirrors_rooms_and_folders_back_to_its_days() {
    let server = server();
    let (store, dir) = store("personal");
    assert_eq!(mirror(&server, &store, DeviceMode::Personal, &|| false), Ended::Done);
    let asked = server.asked.borrow().clone();
    assert_eq!(
        asked,
        [
            format!("{CHAT}/chatData?limit=200"),
            // the latest room first, and its older page while within 30 days; the one past them ends it
            format!("{CHAT}/timeline?chatRoomId=R1&limit=50"),
            format!("{CHAT}/timeline?chatRoomId=R1&beforeSeq=51&limit=50"),
            // a room that fails is passed over
            format!("{CHAT}/timeline?chatRoomId=R2&limit=50"),
            format!("{MAIL}/inboxData"),
            // a new message's, so one is written offline
            COMPOSE.to_string(),
            threads("inbox", "", ""),
            // a folder's pages stop at a thread older than 30 days
            threads("", "A", "F1"),
            format!("{MAIL}/thread?store=email&threadId=T1"),
            format!("{MAIL}/thread?store=email&threadId=T4"),
            format!("{MAIL}/thread?store=email&threadId=T2"),
        ]
    );
    // written as the pages' answers are: the timeline answers offline
    let page = answers::answer(
        &store,
        Kind::Timeline,
        &format!("{CHAT}/timeline"),
        "chatRoomId=R1&limit=50",
    )
    .unwrap();
    let page: Value = serde_json::from_slice(&page).unwrap();
    assert_eq!(page["eventList"].as_array().unwrap().len(), 4);
    assert_eq!(store.mail_messages("email:T2").len(), 1);
    // the new message's answers offline as it was kept
    let compose = answers::answer(&store, Kind::Compose, COMPOSE, "").unwrap();
    let compose: Value = serde_json::from_slice(&compose).unwrap();
    assert_eq!(compose["compose"]["fromValue"], "account:A");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shared_device_reads_ahead_only_the_latest() {
    let server = server();
    let (store, dir) = store("shared");
    assert_eq!(mirror(&server, &store, DeviceMode::Shared, &|| false), Ended::Done);
    assert_eq!(
        server.asked.borrow().clone(),
        [
            format!("{CHAT}/chatData?limit=200"),
            format!("{CHAT}/timeline?chatRoomId=R1&limit=50"),
            format!("{MAIL}/inboxData"),
            COMPOSE.to_string(),
            threads("inbox", "", ""),
            format!("{MAIL}/thread?store=email&threadId=T1"),
        ]
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_run_stops_when_offline_signed_out_or_cancelled() {
    let (store, dir) = store("stops");
    let mut offline = server();
    offline
        .answers
        .insert(format!("{CHAT}/timeline?chatRoomId=R1&limit=50"), Fetched::Offline);
    assert_eq!(
        mirror(&offline, &store, DeviceMode::Personal, &|| false),
        Ended::Offline
    );
    assert_eq!(offline.asked.borrow().len(), 2);
    let mut revoked = server();
    revoked
        .answers
        .insert(format!("{CHAT}/chatData?limit=200"), Fetched::Unauthorized);
    assert_eq!(
        mirror(&revoked, &store, DeviceMode::Personal, &|| false),
        Ended::Unauthorized
    );
    assert_eq!(revoked.asked.borrow().len(), 1);
    let cancelled = server();
    assert_eq!(
        mirror(&cancelled, &store, DeviceMode::Personal, &|| true),
        Ended::Cancelled
    );
    assert!(cancelled.asked.borrow().is_empty());
    // no chat on the server: the mail goes on
    let mut no_chat = server();
    no_chat.answers.remove(&format!("{CHAT}/chatData?limit=200"));
    assert_eq!(mirror(&no_chat, &store, DeviceMode::Shared, &|| false), Ended::Done);
    assert_eq!(no_chat.asked.borrow()[1], format!("{MAIL}/inboxData"));
    // no inbox: the new message's is still read, and the mail ends there
    let mut no_inbox = server();
    no_inbox.answers.remove(&format!("{MAIL}/inboxData"));
    assert_eq!(mirror(&no_inbox, &store, DeviceMode::Shared, &|| false), Ended::Done);
    assert_eq!(
        no_inbox.asked.borrow()[2..],
        [format!("{MAIL}/inboxData"), COMPOSE.to_string()]
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_run_at_a_time_and_after_a_drop_the_heads_again() {
    let mirror = Mirror::default();
    let (first, heads) = mirror.begin("a").unwrap();
    assert!(!heads);
    mirror.dropped("a");
    assert_eq!(mirror.begin("a"), None);
    // another account's run starts; the first is cancelled by the switch
    mirror.cancel();
    assert!(mirror.cancelled(first));
    let (other, _) = mirror.begin("b").unwrap();
    assert!(!mirror.cancelled(other));
    mirror.end("a", first, Ended::Cancelled);
    mirror.end("b", other, Ended::Done);
    // b ran lately: not again, unless one of its sockets dropped since, and then the heads only
    assert_eq!(mirror.begin("b"), None);
    mirror.dropped("b");
    let (again, heads) = mirror.begin("b").unwrap();
    assert!(heads);
    mirror.end("b", again, Ended::Done);
    assert_eq!(mirror.begin("b"), None);
    mirror.forget("b");
    assert_eq!(mirror.begin("b").map(|(_, heads)| heads), Some(false));
    // a was cancelled, not done: it runs whole at once next time
    assert_eq!(mirror.begin("a").map(|(_, heads)| heads), Some(false));
}

#[test]
fn a_heads_run_reads_each_rooms_latest_page_only() {
    let server = server();
    let (store, dir) = store("heads");
    let keep = |path: &str, body: &[u8]| {
        let (bare, query) = path.split_once('?').unwrap_or((path, ""));
        if let Some(kind) = answers::classify(bare, DEFAULT_COMMS_ROOT) {
            answers::keep(&store, kind, bare, query, body);
        }
    };
    let ended = run(&Job {
        transport: &server,
        keep: &keep,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        mode: DeviceMode::Personal,
        conf: conf(),
        now_ms: NOW,
        cancelled: &|| false,
        pause: Duration::ZERO,
        heads_only: true,
    });
    assert_eq!(ended, Ended::Done);
    assert_eq!(
        server.asked.borrow().clone(),
        [
            format!("{CHAT}/chatData?limit=200"),
            format!("{CHAT}/timeline?chatRoomId=R1&limit=50"),
            format!("{CHAT}/timeline?chatRoomId=R2&limit=50"),
        ]
    );
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
