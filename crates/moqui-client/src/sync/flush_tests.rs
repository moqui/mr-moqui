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

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::store::{OutboxState, Store};
use serde_json::{json, Value};

use super::*;
use crate::login::ServerSync;
use crate::offline::DEFAULT_COMMS_ROOT;
use crate::sync::outbox::{add, check, echoed};

const NOW: i64 = 1_700_000_000_000;
/// Past the wall clock, which a release of held operations sets them due at.
const LATER: i64 = 4_000_000_000_000;
const CHAT: &str = "/apps/BasicComms/Chat";
const COMPOSE: &str = "/apps/BasicComms/Inbox/Compose";

/// A server of scripted answers by path (each answered in turn, the last one again), noting what was asked; a path with
/// none answers 404.
#[derive(Default)]
struct Server {
    answers: RefCell<HashMap<String, VecDeque<Sent>>>,
    asked: RefCell<Vec<(String, String)>>,
}

impl Server {
    fn answer(&self, path: &str, sent: Sent) {
        self.answers
            .borrow_mut()
            .entry(path.to_string())
            .or_default()
            .push_back(sent);
    }

    fn paths(&self) -> Vec<String> {
        self.asked.borrow().iter().map(|(path, _)| path.clone()).collect()
    }

    fn take(&self) -> Vec<(String, String)> {
        std::mem::take(&mut self.asked.borrow_mut())
    }
}

impl Transport for Server {
    fn post(&self, path: &str, _content_type: &str, body: Vec<u8>) -> Sent {
        self.asked
            .borrow_mut()
            .push((path.to_string(), String::from_utf8_lossy(&body).into_owned()));
        let mut answers = self.answers.borrow_mut();
        let Some(list) = answers.get_mut(path) else {
            return Sent::Answer(404, Vec::new());
        };
        if list.len() > 1 {
            list.pop_front().unwrap_or(Sent::Offline)
        } else {
            list.front().cloned().unwrap_or(Sent::Offline)
        }
    }

    fn get(&self, path: &str) -> Sent {
        self.post(path, "", Vec::new())
    }
}

impl StoreAccess for Mutex<Store> {
    fn with_store(&self, work: &mut dyn FnMut(&Store)) -> bool {
        work(&self.lock().unwrap());
        true
    }
}

fn ok(json: Value) -> Sent {
    Sent::Answer(200, json.to_string().into_bytes())
}

fn store(name: &str) -> (Mutex<Store>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-flush-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (Mutex::new(Store::open(&dir, "a", 1 << 24).unwrap()), dir)
}

fn queue(store: &Mutex<Store>, kind: &str, payload: Value, files: &[String], created: i64) {
    let op = check(kind, &payload, files, created).unwrap();
    add(&store.lock().unwrap(), op).unwrap();
}

fn chat(store: &Mutex<Store>, client_ref: &str, room: &str, created: i64) {
    let item = json!({ "clientRef": client_ref, "chatRoomId": room, "url": format!("{CHAT}/send"),
        "params": { "body": client_ref }, "createdDate": created });
    queue(store, outbox::CHAT_SEND, item, &[], created);
}

fn state(store: &Mutex<Store>, id: &str) -> Option<OutboxState> {
    store.lock().unwrap().outbox_get(id).map(|op| op.state)
}

fn run(server: &Server, store: &Mutex<Store>, now: i64) -> Flushed {
    let clock = move || now;
    flush(&Job {
        transport: server,
        store,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        now: &clock,
        claimed: &|| {},
        server: ServerSync::default(),
    })
}

#[test]
fn a_chat_send_is_acked_by_its_answer_and_then_by_its_echo() {
    let (store, dir) = store("acked");
    chat(&store, "c1", "R1", NOW);
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/send"),
        ok(json!({ "eventSeq": 5, "event": { "eventSeq": 5 } })),
    );
    let flushed = run(&server, &store, NOW);
    assert!(flushed.changed);
    assert_eq!(flushed.sent.len(), 1);
    assert_eq!(flushed.sent[0]["clientRef"], "c1");
    assert_eq!(flushed.sent[0]["chatRoomId"], "R1");
    assert_eq!(flushed.sent[0]["json"]["eventSeq"], 5);
    let asked = server.take();
    assert!(
        asked[0].1.contains("clientRef=c1") && asked[0].1.contains("chatRoomId=R1") && asked[0].1.contains("body=c1")
    );
    assert_eq!(state(&store, "c1"), Some(OutboxState::Acked));
    // nothing more to send; the echo takes it off
    assert!(!run(&server, &store, NOW + 1).changed);
    assert!(echoed(&store.lock().unwrap(), "c1"));
    assert_eq!(state(&store, "c1"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_answer_is_retried_with_backoff_and_order_kept_per_room() {
    let (store, dir) = store("retry");
    chat(&store, "c1", "R1", NOW);
    chat(&store, "c2", "R1", NOW + 1);
    chat(&store, "c3", "R2", NOW + 2);
    let server = Server::default();
    let send = format!("{CHAT}/send");
    server.answer(&send, Sent::Offline);
    let flushed = run(&server, &store, NOW + 10);
    // c2 waits behind c1 in its room; c3 of another room is tried
    assert_eq!(server.take().len(), 2);
    assert_eq!(state(&store, "c1"), Some(OutboxState::Retry));
    assert_eq!(state(&store, "c2"), Some(OutboxState::Queued));
    assert_eq!(state(&store, "c3"), Some(OutboxState::Retry));
    let first = store.lock().unwrap().outbox_get("c1").unwrap();
    assert_eq!(first.next_ms, NOW + 10 + 2_000);
    assert_eq!(flushed.next_ms, Some(NOW + 10 + 2_000));
    // not due yet: nothing sent
    run(&server, &store, NOW + 500);
    assert!(server.take().is_empty());
    // a 503 the second time: a longer wait
    server.answers.borrow_mut().clear();
    server.answer(&send, Sent::Answer(503, Vec::new()));
    run(&server, &store, NOW + 2_010);
    assert_eq!(
        store.lock().unwrap().outbox_get("c1").unwrap().next_ms,
        NOW + 2_010 + 5_000
    );
    // then answered: c1 then c2, in order
    server.answers.borrow_mut().clear();
    server.answer(&send, ok(json!({ "eventSeq": 1, "event": {} })));
    run(&server, &store, NOW + 10_000);
    let bodies: Vec<String> = server.take().into_iter().map(|(_, body)| body).collect();
    let at = |name: &str| {
        bodies
            .iter()
            .position(|body| body.contains(&format!("clientRef={name}")))
            .unwrap()
    };
    assert!(at("c1") < at("c2"));
    assert_eq!(state(&store, "c2"), Some(OutboxState::Acked));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unauthorized_is_held_until_released_and_a_refusal_fails_with_the_message() {
    let (store, dir) = store("held");
    chat(&store, "c1", "R1", NOW);
    chat(&store, "c2", "R2", NOW + 1);
    let server = Server::default();
    let send = format!("{CHAT}/send");
    server.answer(&send, Sent::Answer(401, Vec::new()));
    let flushed = run(&server, &store, NOW + 10);
    assert!(flushed.held);
    assert_eq!(state(&store, "c1"), Some(OutboxState::Held));
    assert_eq!(state(&store, "c2"), Some(OutboxState::Held));
    // held: not sent again until released
    run(&server, &store, NOW + 100_000);
    assert_eq!(server.take().len(), 2);
    assert_eq!(store.lock().unwrap().outbox_release_held().unwrap(), 2);
    server.answers.borrow_mut().clear();
    server.answer(
        &send,
        Sent::Answer(400, json!({ "errors": ["Not a member"] }).to_string().into_bytes()),
    );
    run(&server, &store, LATER);
    let failed = store.lock().unwrap().outbox_get("c1").unwrap();
    assert_eq!(failed.state, OutboxState::Failed);
    assert_eq!(failed.error.as_deref(), Some("Not a member"));
    // an answer with no event is no send
    chat(&store, "c3", "R3", NOW + 2);
    server.answers.borrow_mut().clear();
    server.answer(&send, ok(json!({ "reasonCode": null })));
    run(&server, &store, LATER + 1);
    assert_eq!(state(&store, "c3"), Some(OutboxState::Failed));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_failed_operation_does_not_hold_up_the_others() {
    let (store, dir) = store("tolerant");
    chat(&store, "c1", "R1", NOW);
    // a chat send whose url the comms app no longer has (checked again when sent)
    {
        let locked = store.lock().unwrap();
        let mut bad = crate::store::OutboxOp::new(
            "c0",
            outbox::CHAT_SEND,
            "R9",
            &json!({ "chatRoomId": "R9", "url": "https://elsewhere.example.com/" }).to_string(),
        );
        bad.created_ms = NOW - 5;
        bad.next_ms = NOW - 5;
        locked.outbox_add(&bad).unwrap();
    }
    queue(
        &store,
        outbox::CHAT_READ,
        json!({ "chatRoomId": "R1", "lastReadEventSeq": 3 }),
        &[],
        NOW + 1,
    );
    let server = Server::default();
    server.answer(&format!("{CHAT}/send"), ok(json!({ "eventSeq": 1, "event": {} })));
    server.answer(&format!("{CHAT}/markRead"), ok(json!({ "lastReadEventSeq": 3 })));
    run(&server, &store, NOW + 10);
    assert_eq!(state(&store, "c0"), Some(OutboxState::Failed));
    assert_eq!(state(&store, "c1"), Some(OutboxState::Acked));
    // the mark-read went with its absolute seq, and is done
    let asked = server.take();
    let read = asked.iter().find(|(path, _)| path.ends_with("/markRead")).unwrap();
    assert!(read.1.contains("eventSeq=3") && read.1.contains("chatRoomId=R1"));
    assert!(store
        .lock()
        .unwrap()
        .outbox_list()
        .iter()
        .all(|op| op.kind != outbox::CHAT_READ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mail_send_saves_uploads_and_sends_each_once_by_its_client_ref() {
    let (store, dir) = store("mail");
    let file = store.lock().unwrap().save_file("a.txt", b"hello").unwrap();
    let send = json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1", "toJson": "[{\"value\":\"a@x.org\"}]",
        "subject": "Hi", "body": "<p>b</p>", "scheduledSendDate": "" },
        "files": [{ "file": file, "name": "a.txt", "contentType": "text/plain" }] });
    queue(&store, outbox::MAIL_SEND, send, std::slice::from_ref(&file), NOW);
    let server = Server::default();
    server.answer(
        &format!("{COMPOSE}/saveDraft"),
        ok(json!({ "emailMessageId": "E1", "duplicate": false })),
    );
    // the upload's answer is lost once: the send starts over
    server.answer(&format!("{COMPOSE}/upload"), Sent::Offline);
    server.answer(
        &format!("{COMPOSE}/upload"),
        ok(json!({ "attachmentList": [{ "id": "A1" }] })),
    );
    server.answer(
        &format!("{COMPOSE}/send"),
        ok(json!({ "mode": "email", "emailMessageId": "E1", "duplicate": true })),
    );
    run(&server, &store, NOW + 10);
    assert_eq!(
        server.paths(),
        [format!("{COMPOSE}/saveDraft"), format!("{COMPOSE}/upload")]
    );
    assert_eq!(state(&store, "m1"), Some(OutboxState::Retry));
    server.take();
    let flushed = run(&server, &store, NOW + 10 + 2_000);
    let asked = server.take();
    let paths: Vec<&str> = asked.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            format!("{COMPOSE}/saveDraft"),
            format!("{COMPOSE}/upload"),
            format!("{COMPOSE}/send")
        ]
    );
    assert!(asked[0].1.contains("clientRef=m1") && asked[0].1.contains("subject=Hi"));
    // the draft's fields only, not the send's
    assert!(!asked[0].1.contains("scheduledSendDate"));
    assert!(asked[1].1.contains("name=\"clientRef\"\r\n\r\nm1") && asked[1].1.contains("filename=\"a.txt\""));
    assert!(asked[1].1.contains("hello"));
    assert!(asked[2].1.contains("emailMessageId=E1") && asked[2].1.contains("clientRef=m1"));
    // a send that went before (duplicate) is done: the operation and its file go
    assert_eq!(state(&store, "m1"), None);
    assert_eq!(flushed.sent[0]["json"]["duplicate"], true);
    assert!(store.lock().unwrap().read_file(&file).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mail_state_change_posts_the_state_transition() {
    let (store, dir) = store("state");
    let targets = json!([{ "store": "email", "threadId": "T1", "rowKey": "email:T1" }]);
    queue(
        &store,
        outbox::MAIL_STATE,
        json!({ "targets": targets, "action": "move", "emailFolderId": "F2" }),
        &[],
        NOW,
    );
    let server = Server::default();
    server.answer(
        "/apps/BasicComms/Inbox/Mail/state",
        ok(json!({ "updatedCount": 1, "failedList": [] })),
    );
    run(&server, &store, NOW + 1);
    let asked = server.take();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.contains("action=move") && asked[0].1.contains("emailFolderId=F2"));
    assert!(asked[0].1.contains("targetsJson="));
    assert!(store.lock().unwrap().outbox_list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_store_that_is_gone_ends_the_pass() {
    struct Gone;
    impl StoreAccess for Gone {
        fn with_store(&self, _work: &mut dyn FnMut(&Store)) -> bool {
            false
        }
    }
    let server = Server::default();
    let calls = Cell::new(0);
    let clock = || {
        calls.set(calls.get() + 1);
        NOW
    };
    let flushed = flush(&Job {
        transport: &server,
        store: &Gone,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        now: &clock,
        claimed: &|| {},
        server: ServerSync::default(),
    });
    assert!(!flushed.changed && flushed.next_ms.is_none());
    assert!(server.take().is_empty());
}

#[test]
fn form_and_multipart_bodies_are_encoded() {
    let body = form_body(&[
        ("a b".to_string(), "c&d=é".to_string()),
        ("e".to_string(), String::new()),
    ]);
    assert_eq!(body, "a+b=c%26d%3D%C3%A9&e=");
    let (kind, bytes) = multipart(&[("clientRef", "m1")], "we\"ird\r\n.txt", "text/plain", b"x");
    assert!(kind.starts_with("multipart/form-data; boundary=----moqui"));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("filename=\"we_ird__.txt\""));
    assert!(text.ends_with("--\r\n"));
}

/// A server whose answer to a path runs `then` first (an echo, a discard arriving while the request is out).
struct During<'a> {
    server: &'a Server,
    path: String,
    then: Box<dyn Fn() + 'a>,
}

impl Transport for During<'_> {
    fn post(&self, path: &str, content_type: &str, body: Vec<u8>) -> Sent {
        if path == self.path {
            (self.then)();
        }
        self.server.post(path, content_type, body)
    }
}

fn run_with(transport: &dyn Transport, store: &Mutex<Store>, now: i64, claimed: &dyn Fn()) -> Flushed {
    let clock = move || now;
    flush(&Job {
        transport,
        store,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        now: &clock,
        claimed,
        server: ServerSync::default(),
    })
}

#[test]
fn a_reopened_server_draft_is_sent_by_its_own_ref_or_else_by_its_id() {
    let (store, dir) = store("reopened");
    // Compose had the draft's ref from composeData: it is the operation's
    let send = |client_ref: &str| json!({ "clientRef": client_ref, "fields": { "fromValue": "identity:1", "emailMessageId": "E1", "subject": "Hi" } });
    queue(&store, outbox::MAIL_SEND, send("refA"), &[], NOW);
    let server = Server::default();
    server.answer(&format!("{COMPOSE}/saveDraft"), ok(json!({ "emailMessageId": "E1" })));
    server.answer(
        &format!("{COMPOSE}/send"),
        ok(json!({ "mode": "email", "emailMessageId": "E1" })),
    );
    run(&server, &store, NOW + 1);
    let asked = server.take();
    assert!(asked[0].1.contains("emailMessageId=E1") && asked[0].1.contains("clientRef=refA"));
    assert!(asked[1].1.contains("emailMessageId=E1") && asked[1].1.contains("clientRef=refA"));
    assert_eq!(state(&store, "refA"), None);
    // a draft whose ref is another: sent by its id, without the operation's ref
    queue(&store, outbox::MAIL_SEND, send("refB"), &[], NOW + 2);
    server.answers.borrow_mut().clear();
    let refused = json!({ "errors": "Email message E1 has another clientRef" })
        .to_string()
        .into_bytes();
    server.answer(&format!("{COMPOSE}/saveDraft"), Sent::Answer(400, refused));
    server.answer(&format!("{COMPOSE}/saveDraft"), ok(json!({ "emailMessageId": "E1" })));
    server.answer(
        &format!("{COMPOSE}/send"),
        ok(json!({ "mode": "email", "emailMessageId": "E1" })),
    );
    run(&server, &store, NOW + 3);
    let asked = server.take();
    assert_eq!(asked.len(), 3);
    assert!(asked[0].1.contains("clientRef=refB"));
    assert!(!asked[1].1.contains("clientRef") && asked[1].1.contains("emailMessageId=E1"));
    assert!(!asked[2].1.contains("clientRef") && asked[2].1.contains("emailMessageId=E1"));
    assert_eq!(state(&store, "refB"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mail_send_whose_answer_was_lost_is_sent_again_and_taken_once() {
    let (store, dir) = store("lost-send");
    queue(
        &store,
        outbox::MAIL_SEND,
        json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1", "subject": "s" } }),
        &[],
        NOW,
    );
    let server = Server::default();
    server.answer(&format!("{COMPOSE}/saveDraft"), ok(json!({ "emailMessageId": "E1" })));
    server.answer(&format!("{COMPOSE}/send"), Sent::Offline);
    server.answer(
        &format!("{COMPOSE}/send"),
        ok(json!({ "emailMessageId": "E1", "duplicate": true })),
    );
    run(&server, &store, NOW + 1);
    assert_eq!(state(&store, "m1"), Some(OutboxState::Retry));
    let flushed = run(&server, &store, NOW + 10_000);
    assert_eq!(state(&store, "m1"), None);
    assert_eq!(flushed.sent[0]["json"]["duplicate"], true);
    let sends: Vec<String> = server
        .take()
        .into_iter()
        .filter(|(path, _)| path.ends_with("/send"))
        .map(|(_, body)| body)
        .collect();
    assert_eq!(sends.len(), 2);
    assert!(sends.iter().all(|body| body.contains("clientRef=m1")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_operation_discarded_or_echoed_meanwhile_is_not_sent_again() {
    let (store, dir) = store("meanwhile");
    chat(&store, "c1", "R1", NOW);
    chat(&store, "c2", "R2", NOW + 1);
    let server = Server::default();
    server.answer(&format!("{CHAT}/send"), ok(json!({ "eventSeq": 1, "event": {} })));
    // c1 discarded after the claim, before its request
    let discard = || {
        let locked = store.lock().unwrap();
        locked.outbox_remove("c1").unwrap();
    };
    // c2's echo comes before its answer
    let during = During {
        server: &server,
        path: format!("{CHAT}/send"),
        then: Box::new(|| {
            echoed(&store.lock().unwrap(), "c2");
        }),
    };
    let flushed = run_with(&during, &store, NOW + 10, &discard);
    let asked = server.take();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.contains("clientRef=c2"));
    assert_eq!(state(&store, "c1"), None);
    // the answer still tells the pages; nothing is left to wait for an echo
    assert_eq!(state(&store, "c2"), None);
    assert_eq!(flushed.sent.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn held_three_times_fails_and_old_files_fail_a_chat_message() {
    let (store, dir) = store("holds");
    chat(&store, "c1", "R1", NOW);
    let server = Server::default();
    server.answer(&format!("{CHAT}/send"), Sent::Answer(403, Vec::new()));
    for round in 1..=3 {
        run(&server, &store, LATER + round);
        let op = store.lock().unwrap().outbox_get("c1").unwrap();
        if round < 3 {
            assert_eq!(op.state, OutboxState::Held);
            assert_eq!(
                op.error.as_deref(),
                Some(format!("Waiting for a sign-in ({round})").as_str())
            );
            store.lock().unwrap().outbox_release_held().unwrap();
        } else {
            assert_eq!(op.state, OutboxState::Failed);
        }
    }
    // a message with files queued more than 20 hours ago: its files are gone on the server
    let item = json!({ "clientRef": "c2", "chatRoomId": "R2", "url": format!("{CHAT}/send"), "params": {}, "hasFiles": true,
        "createdDate": NOW });
    queue(&store, outbox::CHAT_SEND, item, &[], NOW);
    server.take();
    run(&server, &store, NOW + 21 * 3600 * 1000);
    assert!(server.take().is_empty());
    let op = store.lock().unwrap().outbox_get("c2").unwrap();
    assert_eq!(op.state, OutboxState::Failed);
    assert!(op.error.unwrap().contains("files"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_internal_message_queued_before_is_refused() {
    let (store, dir) = store("internal");
    {
        let locked = store.lock().unwrap();
        let op = crate::store::OutboxOp::new(
            "m9",
            outbox::MAIL_SEND,
            "",
            &json!({ "clientRef": "m9", "fields": { "fromValue": "internal" }, "files": [] }).to_string(),
        );
        locked.outbox_add(&op).unwrap();
    }
    let server = Server::default();
    run(&server, &store, LATER);
    assert!(server.take().is_empty());
    assert_eq!(state(&store, "m9"), Some(OutboxState::Failed));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- MD5: a server at clientApi 4 ----

const MAIL: &str = "/apps/BasicComms/Inbox/Mail";

fn md5(internal: bool) -> ServerSync {
    ServerSync {
        client_api: 4,
        internal_send_ref: internal,
        mail_uncovered: Vec::new(),
    }
}

fn run_md5(server: &Server, store: &Mutex<Store>, now: i64) -> Flushed {
    let clock = move || now;
    flush(&Job {
        transport: server,
        store,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        now: &clock,
        claimed: &|| {},
        server: md5(false),
    })
}

fn queue_md5(store: &Mutex<Store>, kind: &str, payload: Value, files: &[String], created: i64, internal: bool) {
    let op = outbox::check_with(kind, &payload, files, created, &md5(internal)).unwrap();
    add(&store.lock().unwrap(), op).unwrap();
}

fn forget(server: &Server, path: &str) {
    server.answers.borrow_mut().remove(path);
}

#[test]
fn a_message_change_goes_once_by_its_ref_in_the_order_of_its_message() {
    let (store, dir) = store("message-kinds");
    // not queued for a server of MD4
    let edit = json!({ "clientRef": "e1", "chatRoomId": "R1", "eventSeq": 5, "body": "<p>new</p>" });
    assert!(check(outbox::CHAT_EDIT, &edit, &[], NOW).is_err());
    queue_md5(&store, outbox::CHAT_EDIT, edit, &[], NOW, false);
    let react = json!({ "clientRef": "r1", "chatRoomId": "R1", "eventSeq": 5, "key": "+1" });
    queue_md5(&store, outbox::CHAT_REACT, react, &[], NOW + 1, false);
    let vote = json!({ "clientRef": "v1", "chatRoomId": "R1", "eventSeq": 7, "optionIds": [2, 3] });
    queue_md5(&store, outbox::POLL_VOTE, vote, &[], NOW + 2, false);
    let redact = json!({ "clientRef": "d1", "chatRoomId": "R1", "eventSeq": 8 });
    queue_md5(&store, outbox::CHAT_REDACT, redact, &[], NOW + 3, false);
    let server = Server::default();
    // the edit's answer is lost: its reaction waits behind it, the other messages' changes go
    server.answer(&format!("{CHAT}/edit"), Sent::Offline);
    server.answer(
        &format!("{CHAT}/votePoll"),
        ok(json!({ "poll": {}, "duplicate": true })),
    );
    server.answer(&format!("{CHAT}/redact"), ok(json!({ "redactionEventSeq": 9 })));
    run_md5(&server, &store, NOW + 10);
    let asked = server.take();
    let paths: Vec<&str> = asked.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            format!("{CHAT}/edit"),
            format!("{CHAT}/votePoll"),
            format!("{CHAT}/redact")
        ]
    );
    assert!(asked[0].1.contains("clientRef=e1") && asked[0].1.contains("eventSeq=5"));
    assert!(asked[1].1.contains("optionIds=2%2C3"));
    assert_eq!(state(&store, "v1"), None);
    assert_eq!(state(&store, "d1"), None);
    assert_eq!(state(&store, "r1"), Some(OutboxState::Queued));
    // sent again: the server took it before and says so; then the reaction goes
    forget(&server, &format!("{CHAT}/edit"));
    server.answer(
        &format!("{CHAT}/edit"),
        ok(json!({ "editEventSeq": 9, "duplicate": true })),
    );
    server.answer(&format!("{CHAT}/react"), ok(json!({ "added": true })));
    run_md5(&server, &store, NOW + 10 + 2_000);
    assert_eq!(server.paths(), [format!("{CHAT}/edit"), format!("{CHAT}/react")]);
    assert!(store.lock().unwrap().outbox_list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refusal_is_a_rejection_kept_with_its_status() {
    let (store, dir) = store("rejected");
    let redact = json!({ "clientRef": "d1", "chatRoomId": "R1", "eventSeq": 8 });
    queue_md5(&store, outbox::CHAT_REDACT, redact, &[], NOW, false);
    let server = Server::default();
    server.answer(
        &format!("{CHAT}/redact"),
        Sent::Answer(400, json!({ "errors": "Not yours" }).to_string().into_bytes()),
    );
    run_md5(&server, &store, NOW + 1);
    let op = store.lock().unwrap().outbox_get("d1").unwrap();
    assert!(op.rejected() && op.status == 400);
    let listed = &outbox::list_json(&store.lock().unwrap())[0];
    assert_eq!(listed["rejected"], true);
    assert_eq!(listed["error"], "Not yours");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mail_state_change_sends_what_was_seen_and_keeps_a_conflict() {
    let (store, dir) = store("seen");
    {
        let locked = store.lock().unwrap();
        let row = |id: &str, unread: bool| crate::store::MailThread {
            folder_id: "view=inbox".to_string(),
            thread_id: format!("email:{id}"),
            sort_ms: 1,
            row: json!({ "threadId": id, "isUnread": unread, "lastEmailMessageId": format!("{id}-last") }).to_string(),
        };
        locked.put_mail_threads(&[row("T1", true), row("T2", true)]).unwrap();
    }
    let targets = json!([{ "store": "email", "threadId": "T1", "rowKey": "email:T1" },
        { "store": "email", "threadId": "T2", "rowKey": "email:T2" }]);
    queue_md5(
        &store,
        outbox::MAIL_STATE,
        json!({ "targets": targets, "action": "read" }),
        &[],
        NOW,
        false,
    );
    // shown read at once, but seen as it was
    let op = store.lock().unwrap().outbox_list().remove(0);
    let payload = outbox::payload_of(&op);
    assert_eq!(
        payload["targets"][0]["seen"],
        json!({ "isRead": false, "latestMessageId": "T1-last" })
    );
    let server = Server::default();
    server.answer(
        &format!("{MAIL}/state"),
        ok(
            json!({ "updatedCount": 1, "failedList": [{ "key": "email:T2", "message": "Changed on the server",
            "reason": "conflict", "current": { "isRead": true } }] }),
        ),
    );
    run_md5(&server, &store, NOW + 1);
    let asked = server.take();
    assert!(asked[0].1.contains(&format!("clientRef={}", op.id)));
    assert!(asked[0].1.contains("seen"));
    let kept = store.lock().unwrap().outbox_get(&op.id).unwrap();
    assert_eq!(kept.state, OutboxState::Conflict);
    let current: Value = serde_json::from_str(kept.current.as_deref().unwrap()).unwrap();
    assert_eq!(current["conflicts"][0]["current"]["isRead"], true);
    let narrowed = outbox::payload_of(&kept);
    assert_eq!(narrowed["targets"].as_array().unwrap().len(), 1);
    assert_eq!(narrowed["targets"][0]["threadId"], "T2");
    // a conflict holds up no later change
    queue_md5(
        &store,
        outbox::MAIL_STATE,
        json!({ "targets": [{ "store": "email", "threadId": "T3" }], "action": "flag" }),
        &[],
        NOW + 2,
        false,
    );
    server.answer(
        &format!("{MAIL}/state"),
        ok(json!({ "updatedCount": 1, "failedList": [] })),
    );
    forget(&server, &format!("{MAIL}/state"));
    server.answer(
        &format!("{MAIL}/state"),
        ok(json!({ "updatedCount": 1, "failedList": [] })),
    );
    run_md5(&server, &store, NOW + 3);
    assert_eq!(server.take().len(), 1);
    // the user's retry sends it anyway: no seen, another request ref
    outbox::retry(&store.lock().unwrap(), &op.id, NOW + 4).unwrap();
    run_md5(&server, &store, NOW + 5);
    let asked = server.take();
    assert!(!asked[0].1.contains("seen") && !asked[0].1.ends_with(&format!("clientRef={}", op.id)));
    assert!(store.lock().unwrap().outbox_list().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_server_of_md4_is_sent_no_seen() {
    let (store, dir) = store("seen-md4");
    store
        .lock()
        .unwrap()
        .put_mail_threads(&[crate::store::MailThread {
            folder_id: "view=inbox".to_string(),
            thread_id: "email:T1".to_string(),
            sort_ms: 1,
            row: json!({ "isUnread": true }).to_string(),
        }])
        .unwrap();
    let targets = json!([{ "store": "email", "threadId": "T1", "rowKey": "email:T1" }]);
    queue(
        &store,
        outbox::MAIL_STATE,
        json!({ "targets": targets, "action": "read" }),
        &[],
        NOW,
    );
    let server = Server::default();
    server.answer(
        &format!("{MAIL}/state"),
        ok(json!({ "updatedCount": 1, "failedList": [] })),
    );
    run(&server, &store, NOW + 1);
    let asked = server.take();
    assert!(!asked[0].1.contains("seen") && !asked[0].1.contains("clientRef"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_draft_changed_on_the_server_is_kept_as_a_copy_and_the_servers_stays() {
    let (store, dir) = store("draft-conflict");
    let file = store.lock().unwrap().save_file("a.txt", b"hello").unwrap();
    let send = json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1", "emailMessageId": "E1",
        "toJson": "[{\"value\":\"a@x.org\"}]", "subject": "Mine", "body": "<p>mine</p>", "baseStamp": "100" },
        "files": [{ "file": file, "name": "a.txt", "contentType": "text/plain" }] });
    queue_md5(&store, outbox::MAIL_SEND, send, std::slice::from_ref(&file), NOW, false);
    let server = Server::default();
    server.answer(
        &format!("{COMPOSE}/saveDraft"),
        ok(json!({ "conflict": true, "compose": { "emailMessageId": "E1", "subject": "Theirs" } })),
    );
    run_md5(&server, &store, NOW + 1);
    let asked = server.take();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.contains("baseStamp=100") && asked[0].1.contains("saveRef="));
    let kept = store.lock().unwrap().outbox_get("m1").unwrap();
    assert_eq!(kept.state, OutboxState::Conflict);
    let current: Value = serde_json::from_str(kept.current.as_deref().unwrap()).unwrap();
    assert_eq!(current["compose"]["subject"], "Theirs");
    let copy_key = current["copyKey"].as_str().unwrap().to_string();
    assert!(copy_key.starts_with("ironstone.compose.email."));
    let copy = store.lock().unwrap().draft(&copy_key).unwrap();
    let value: Value = serde_json::from_str(&copy.value).unwrap();
    assert_eq!(value["draft"]["subject"], "Mine");
    assert_eq!(value["draft"]["to"][0]["value"], "a@x.org");
    assert!(value["draft"]["emailMessageId"].is_null());
    assert_eq!(copy.files, vec![file.clone()]);
    // the copy is reachable by its clientRef, as Compose keeps a draft written offline
    assert_eq!(value["clientRef"], current["copyRef"]);
    assert_eq!(
        copy_key,
        format!("ironstone.compose.email.{}", current["copyRef"].as_str().unwrap())
    );
    // a discard of the send keeps the copy, and its file
    {
        let locked = store.lock().unwrap();
        let mut other = kept.clone();
        other.id = "m2".to_string();
        locked.outbox_add(&other).unwrap();
        outbox::discard(&locked, "m2").unwrap();
        assert!(locked.draft(&copy_key).is_some() && locked.read_file(&file).is_some());
    }
    // a retry sends the user's text anyway, without its baseStamp: the copy goes, so it is not sent twice
    let save_ref = outbox::payload_of(&kept)["fields"]["saveRef"].clone();
    outbox::retry(&store.lock().unwrap(), "m1", NOW + 2).unwrap();
    let retried = outbox::payload_of(&store.lock().unwrap().outbox_get("m1").unwrap());
    assert!(retried["fields"]["baseStamp"].is_null());
    assert_ne!(retried["fields"]["saveRef"], save_ref);
    assert!(store.lock().unwrap().draft(&copy_key).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_state_change_the_server_did_not_make_puts_the_rows_back_and_reads_the_thread_again() {
    let (store, dir) = store("undo");
    {
        let locked = store.lock().unwrap();
        let row = |id: &str| crate::store::MailThread {
            folder_id: "view=inbox".to_string(),
            thread_id: format!("email:{id}"),
            sort_ms: 1,
            row: json!({ "threadId": id, "isUnread": true, "isArchived": false }).to_string(),
        };
        locked.put_mail_threads(&[row("T1"), row("T2")]).unwrap();
    }
    let targets = json!([{ "store": "email", "threadId": "T1", "rowKey": "email:T1" },
        { "store": "email", "threadId": "T2", "rowKey": "email:T2" }]);
    queue_md5(
        &store,
        outbox::MAIL_STATE,
        json!({ "targets": targets, "action": "archive" }),
        &[],
        NOW,
        false,
    );
    // archived here at once: out of the inbox
    assert!(store.lock().unwrap().mail_threads("view=inbox", None, 10).is_empty());
    let server = Server::default();
    server.answer(
        &format!("{MAIL}/state"),
        ok(json!({ "updatedCount": 1, "failedList": [{ "key": "email:T2", "reason": "conflict", "current": {} }] })),
    );
    run_md5(&server, &store, NOW + 1);
    let locked = store.lock().unwrap();
    // the server's state stands for T2: back in the inbox, and read again; T1 was archived
    let inbox: Vec<String> = locked
        .mail_threads("view=inbox", None, 10)
        .into_iter()
        .map(|row| row.thread_id)
        .collect();
    assert_eq!(inbox, vec!["email:T2".to_string()]);
    assert!(locked.sync_cursor("pending:thread:email:T2").is_some());
    assert!(locked.sync_cursor("pending:thread:email:T1").is_none());
    let op = locked.outbox_list().remove(0);
    assert!(outbox::payload_of(&op)["undo"].is_null());
    drop(locked);
    // a refusal puts all of its threads back
    queue_md5(
        &store,
        outbox::MAIL_STATE,
        json!({ "targets": [{ "store": "email", "threadId": "T2", "rowKey": "email:T2" }], "action": "trash" }),
        &[],
        NOW + 2,
        false,
    );
    assert!(store.lock().unwrap().mail_threads("view=inbox", None, 10).is_empty());
    forget(&server, &format!("{MAIL}/state"));
    server.answer(
        &format!("{MAIL}/state"),
        Sent::Answer(400, json!({ "errors": "No" }).to_string().into_bytes()),
    );
    run_md5(&server, &store, NOW + 3);
    assert_eq!(store.lock().unwrap().mail_threads("view=inbox", None, 10).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn seen_is_what_the_server_had_but_for_a_field_a_queued_change_makes() {
    let (store, dir) = store("seen-baseline");
    store
        .lock()
        .unwrap()
        .put_mail_threads(&[crate::store::MailThread {
            folder_id: "view=inbox".to_string(),
            thread_id: "email:T1".to_string(),
            sort_ms: 1,
            row: json!({ "threadId": "T1", "isUnread": true, "isFlagged": false, "lastEmailMessageId": "OLD" })
                .to_string(),
        }])
        .unwrap();
    let target = json!([{ "store": "email", "threadId": "T1", "rowKey": "email:T1", "lastEmailMessageId": "M9" }]);
    let state = |action: &str, at: i64| {
        queue_md5(
            &store,
            outbox::MAIL_STATE,
            json!({ "targets": target, "action": action }),
            &[],
            at,
            false,
        );
        let op = store.lock().unwrap().outbox_list().pop().unwrap();
        outbox::payload_of(&op)["targets"][0]["seen"].clone()
    };
    // the newest message is the page's row's
    assert_eq!(state("read", NOW), json!({ "isRead": false, "latestMessageId": "M9" }));
    // a flag is of another field: as the server has it
    assert_eq!(state("flag", NOW + 1), json!({ "isFlagged": false }));
    // unread after the queued read: as the read leaves it
    assert_eq!(
        state("unread", NOW + 2),
        json!({ "isRead": true, "latestMessageId": "M9" })
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn internal_sends_keep_one_order() {
    let send = json!({ "clientRef": "i1", "fields": { "fromValue": "internal", "subject": "Hi" } });
    let op = outbox::check_with(outbox::MAIL_SEND, &send, &[], NOW, &md5(true)).unwrap();
    assert_eq!(op.target, outbox::INTERNAL_SEND);
}

#[test]
fn a_large_file_goes_in_chunks_and_goes_on_from_where_the_server_is() {
    let (store, dir) = store("chunks");
    let bytes: Vec<u8> = (0..1_500_000_u32).map(|n| (n % 251) as u8).collect();
    let file = store.lock().unwrap().save_file("big.bin", &bytes).unwrap();
    let send = json!({ "clientRef": "m1", "fields": { "fromValue": "identity:1", "subject": "Big" },
        "files": [{ "file": file, "name": "big.bin", "contentType": "application/octet-stream" }] });
    queue_md5(&store, outbox::MAIL_SEND, send, std::slice::from_ref(&file), NOW, false);
    let server = Server::default();
    server.answer(
        &format!("{COMPOSE}/saveDraft"),
        ok(json!({ "emailMessageId": "E1", "draftStamp": 5 })),
    );
    let chunk = format!("{COMPOSE}/uploadChunk");
    server.answer(
        &chunk,
        ok(json!({ "received": 1_048_576, "total": 1_500_000, "accepted": true, "complete": false })),
    );
    server.answer(&chunk, Sent::Offline);
    run_md5(&server, &store, NOW + 1);
    let asked = server.take();
    assert_eq!(asked.len(), 3);
    assert!(asked[1].1.contains("name=\"chunk\"") && asked[1].1.contains("name=\"offset\"\r\n\r\n0\r\n"));
    let upload = store.lock().unwrap().upload("m1", &file).unwrap();
    assert_eq!(upload.offset, 1_048_576);
    assert_eq!(state(&store, "m1"), Some(OutboxState::Retry));
    // after a restart: the server holds the first chunk; the rest goes from there, then the file by its uploadId
    let status = format!("{COMPOSE}/uploadStatus?uploadId={}", upload.upload_id);
    server.answer(
        &status,
        ok(json!({ "found": true, "received": 1_048_576, "total": 1_500_000, "complete": false })),
    );
    forget(&server, &chunk);
    server.answer(
        &chunk,
        ok(json!({ "received": 1_500_000, "total": 1_500_000, "accepted": true, "complete": true })),
    );
    server.answer(&format!("{COMPOSE}/upload"), ok(json!({ "attachmentList": [] })));
    server.answer(
        &format!("{COMPOSE}/send"),
        ok(json!({ "mode": "email", "emailMessageId": "E1" })),
    );
    run_md5(&server, &store, NOW + 10_000);
    let asked = server.take();
    let paths: Vec<&str> = asked.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            format!("{COMPOSE}/saveDraft"),
            status.clone(),
            chunk.clone(),
            format!("{COMPOSE}/upload"),
            format!("{COMPOSE}/send")
        ]
    );
    assert!(asked[2].1.contains("name=\"offset\"\r\n\r\n1048576\r\n"));
    assert!(asked[3].1.contains(&format!("uploadId={}", upload.upload_id)) && asked[3].1.contains("clientRef=m1"));
    // the send compares with the stamp the save left
    assert!(asked[4].1.contains("baseStamp=5"));
    assert!(store.lock().unwrap().outbox_list().is_empty());
    assert!(store.lock().unwrap().upload("m1", &file).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_internal_message_is_queued_where_the_server_takes_its_send_once() {
    let (store, dir) = store("internal-send");
    let send = json!({ "clientRef": "i1", "fields": { "fromValue": "internal", "toJson": "[]", "subject": "Hi" } });
    // MD4, or a server without the feature: it needs a connection
    assert!(check(outbox::MAIL_SEND, &send, &[], NOW).is_err());
    assert!(outbox::check_with(outbox::MAIL_SEND, &send, &[], NOW, &md5(false)).is_err());
    let op = outbox::check_with(outbox::MAIL_SEND, &send, &[], NOW, &md5(true)).unwrap();
    assert_eq!(op.kind, outbox::INTERNAL_SEND);
    add(&store.lock().unwrap(), op).unwrap();
    let server = Server::default();
    server.answer(
        &format!("{COMPOSE}/send"),
        ok(json!({ "mode": "internal", "duplicate": true })),
    );
    let clock = || NOW + 1;
    flush(&Job {
        transport: &server,
        store: &store,
        comms_root: DEFAULT_COMMS_ROOT.to_string(),
        now: &clock,
        claimed: &|| {},
        server: md5(true),
    });
    let asked = server.take();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].1.contains("clientRef=i1") && asked[0].1.contains("fromValue=internal"));
    assert!(store.lock().unwrap().outbox_list().is_empty());
    let kinds = outbox::kinds_json(&md5(true));
    assert_eq!(kinds["internalSend"], true);
    assert!(kinds["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|kind| kind == "chat-edit"));
    assert_eq!(
        outbox::kinds_json(&ServerSync::default())["kinds"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let _ = std::fs::remove_dir_all(&dir);
}
