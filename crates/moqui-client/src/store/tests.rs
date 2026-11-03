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

use super::*;

/// A new directory of this test's own.
fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-client-store-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn room(id: &str, sort_ms: i64) -> ChatRoom {
    ChatRoom {
        chat_room_id: id.to_string(),
        row: format!("{{\"chatRoomId\":\"{id}\"}}"),
        last_seq: 0,
        sort_ms,
    }
}

fn event(room: &str, seq: i64, thread_seq: Option<i64>) -> ChatEvent {
    ChatEvent {
        chat_room_id: room.to_string(),
        event_seq: seq,
        event: format!("{{\"eventSeq\":{seq}}}"),
        thread_seq,
    }
}

fn events(room: &str, seqs: std::ops::RangeInclusive<i64>) -> Vec<ChatEvent> {
    seqs.map(|seq| event(room, seq, None)).collect()
}

fn page(before: Option<i64>, after: Option<i64>, limit: u32) -> EventPage {
    EventPage {
        before,
        after,
        thread: None,
        limit,
    }
}

fn seqs(slice: &EventSlice) -> Vec<i64> {
    slice.events.iter().map(|kept| kept.event_seq).collect()
}

fn thread(folder: &str, id: &str, sort_ms: i64) -> MailThread {
    MailThread {
        folder_id: folder.to_string(),
        thread_id: id.to_string(),
        sort_ms,
        row: "{}".to_string(),
    }
}

fn message(thread_id: &str, id: &str, sort_ms: i64) -> MailMessage {
    MailMessage {
        message_id: id.to_string(),
        thread_id: thread_id.to_string(),
        sort_ms,
        row: "{}".to_string(),
    }
}

fn op(id: &str, kind: &str, target: &str, created_ms: i64) -> OutboxOp {
    let mut op = OutboxOp::new(id, kind, target, "{}");
    op.created_ms = created_ms;
    op.next_ms = created_ms;
    op
}

fn ids(ops: Vec<OutboxOp>) -> Vec<String> {
    ops.into_iter().map(|op| op.id).collect()
}

#[test]
fn the_schema_is_made_once_and_kept() {
    let dir = dir("migrate");
    {
        let store = Store::open(&dir, "a", 1 << 20).unwrap();
        store.put_draft("k", "{}", &[]).unwrap();
    }
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    let version: i64 = store
        .conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(usize::try_from(version).unwrap(), MIGRATIONS.len());
    let journal: String = store
        .conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal, "wal");
    assert!(store.draft("k").is_some());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("data")), 0o700);
        assert_eq!(mode(&dir.join("data/a.db")), 0o600);
    }

    // a newer client's file is not opened; an id that leaves the directory is refused
    store.conn.pragma_update(None, "user_version", 99).unwrap();
    drop(store);
    assert!(Store::open(&dir, "a", 1 << 20).is_err());
    assert!(Store::open(&dir, "../a", 1 << 20).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn answers_are_found_by_path_and_sorted_query() {
    let dir = dir("answers");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    assert_eq!(answer_key("/chat/rooms", ""), "/chat/rooms");
    assert_eq!(answer_key("/x", "?b=2&a=1&&a=0"), "/x?a=1&a=0&b=2");
    store
        .put_answer("/chat/timeline", "?chatRoomId=R1&limit=50", "application/json", b"[1]")
        .unwrap();
    let found = store.answer("/chat/timeline", "limit=50&chatRoomId=R1").unwrap();
    assert_eq!(found.body, b"[1]");
    assert_eq!(found.content_type, "application/json");
    assert!(found.stored_ms > 0);
    assert!(store.answer("/chat/timeline", "chatRoomId=R2&limit=50").is_none());
    store
        .put_answer("/chat/timeline", "chatRoomId=R1&limit=50", "application/json", b"[2]")
        .unwrap();
    assert_eq!(
        store.answer("/chat/timeline", "chatRoomId=R1&limit=50").unwrap().body,
        b"[2]"
    );
    store.remove_answer("/chat/timeline", "chatRoomId=R1&limit=50").unwrap();
    assert!(store.answer("/chat/timeline", "chatRoomId=R1&limit=50").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_read_is_held_in_memory_and_written_later() {
    let dir = dir("reads");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    store.put_answer("/a", "", "text/plain", b"a").unwrap();
    let read_ms = || -> i64 {
        store
            .conn
            .query_row("SELECT read_ms FROM answer", [], |row| row.get(0))
            .unwrap()
    };
    let written = read_ms();
    assert!(store.answer("/a", "").is_some());
    assert_eq!(read_ms(), written);
    store.flush_reads();
    assert!(read_ms() > written);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn chat_rooms_are_kept_and_a_room_list_replaces_them() {
    let dir = dir("rooms");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    store.put_chat_room(&room("R1", 100)).unwrap();
    store.put_chat_room(&room("R2", 300)).unwrap();
    store.put_chat_room(&room("R3", 200)).unwrap();
    let room_ids = |limit| -> Vec<String> {
        store
            .chat_rooms(limit)
            .into_iter()
            .map(|kept| kept.chat_room_id)
            .collect()
    };
    assert_eq!(room_ids(2), ["R2", "R3"]);
    store.put_chat_events(&events("R1", 1..=12)).unwrap();
    assert_eq!(store.chat_room("R1").unwrap().last_seq, 12);
    // a room written again with an older last_seq keeps the higher
    store.put_chat_room(&room("R1", 400)).unwrap();
    assert_eq!(store.chat_room("R1").unwrap().last_seq, 12);

    // R1 left: gone with its events
    store
        .put_chat_page("R1", &page(None, None, 50), &events("R1", 1..=12), false, false)
        .unwrap();
    assert_eq!(
        store
            .replace_chat_rooms(&[room("R2", 300), room("R3", 200), room("R4", 500)])
            .unwrap(),
        3
    );
    assert_eq!(room_ids(10), ["R4", "R2", "R3"]);
    assert!(store.chat_events("R1", &page(None, None, 50)).events.is_empty());
    let left: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM chat_event", [], |row| row.get(0))
        .unwrap();
    assert_eq!(left, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_timeline_read_offline_pages_as_the_server_and_stops_at_a_gap() {
    let dir = dir("timeline");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    // the latest page (21..=30, more before), then the page before it (11..=20, more before)
    store
        .put_chat_page("R1", &page(None, None, 10), &events("R1", 21..=30), true, false)
        .unwrap();
    store
        .put_chat_page("R1", &page(Some(21), None, 10), &events("R1", 11..=20), true, true)
        .unwrap();
    // an old page apart from them (1..=5, the start), so 6..=10 is a gap
    store
        .put_chat_page("R1", &page(Some(6), None, 10), &events("R1", 1..=5), false, true)
        .unwrap();

    let latest = store.chat_events("R1", &page(None, None, 5));
    assert_eq!(seqs(&latest), [26, 27, 28, 29, 30]);
    assert!(latest.has_more_before && !latest.has_more_after && !latest.gap_after);
    let older = store.chat_events("R1", &page(Some(26), None, 10));
    assert_eq!(seqs(&older), [16, 17, 18, 19, 20, 21, 22, 23, 24, 25]);
    assert!(older.has_more_before && older.has_more_after);
    // the edge of the range: 11 is the oldest held whole; before it is a gap, not the start
    let edge = store.chat_events("R1", &page(Some(16), None, 10));
    assert_eq!(seqs(&edge), [11, 12, 13, 14, 15]);
    assert!(!edge.has_more_before && edge.gap_before);
    let in_gap = store.chat_events("R1", &page(Some(11), None, 10));
    assert!(in_gap.events.is_empty() && in_gap.gap_before);
    // the old range reaches the start: no gap before it
    let start = store.chat_events("R1", &page(Some(4), None, 10));
    assert_eq!(seqs(&start), [1, 2, 3]);
    assert!(!start.gap_before && start.has_more_after);
    // after wins over before, as on the server
    let after = store.chat_events("R1", &page(Some(5), Some(27), 10));
    assert_eq!(seqs(&after), [28, 29, 30]);
    assert!(after.has_more_before && !after.has_more_after && !after.gap_after);
    let after_end = store.chat_events("R1", &page(None, Some(3), 10));
    assert_eq!(seqs(&after_end), [4, 5]);
    assert!(after_end.gap_after);
    // the limit is clamped to the server's 200
    store
        .put_chat_page("R2", &page(None, None, 200), &events("R2", 1..=300), false, false)
        .unwrap();
    assert_eq!(store.chat_events("R2", &page(None, None, 1000)).events.len(), 200);
    assert_eq!(store.chat_events("R2", &page(None, None, 0)).events.len(), 1);
    assert!(store.chat_events("R9", &page(None, None, 5)).gap_before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pushes_keep_the_head_up_until_the_socket_drops() {
    let dir = dir("pushes");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    store
        .put_chat_page("R1", &page(None, None, 10), &events("R1", 1..=3), false, false)
        .unwrap();
    store.put_chat_pushed(&event("R1", 4, None)).unwrap();
    let slice = store.chat_events("R1", &page(None, None, 10));
    assert_eq!(seqs(&slice), [1, 2, 3, 4]);
    assert!(!slice.gap_after);

    // a thread: the root and its replies, apart from the main timeline
    let thread_page = EventPage {
        thread: Some(2),
        limit: 10,
        ..EventPage::default()
    };
    store
        .put_chat_page(
            "R1",
            &thread_page,
            &[event("R1", 2, None), event("R1", 5, Some(2))],
            false,
            false,
        )
        .unwrap();
    store.put_chat_pushed(&event("R1", 6, Some(2))).unwrap();
    assert_eq!(seqs(&store.chat_events("R1", &thread_page)), [2, 5, 6]);
    assert_eq!(seqs(&store.chat_events("R1", &page(None, None, 10))), [1, 2, 3, 4]);

    // dropped: a push may have been missed, so a push now is kept but not shown past a gap
    store.mark_chat_heads_stale().unwrap();
    store.put_chat_pushed(&event("R1", 9, None)).unwrap();
    let stale = store.chat_events("R1", &page(None, None, 10));
    assert_eq!(seqs(&stale), [1, 2, 3, 4]);
    assert!(stale.gap_after);
    // the latest page read again, apart from the old range (7..=9, more before)
    store
        .put_chat_page("R1", &page(None, None, 3), &events("R1", 7..=9), true, false)
        .unwrap();
    let fresh = store.chat_events("R1", &page(None, None, 10));
    assert_eq!(seqs(&fresh), [7, 8, 9]);
    assert!(fresh.gap_before && !fresh.gap_after);

    // an edit folded into its event by the caller replaces the event's JSON
    let mut edited = event("R1", 9, None);
    edited.event = "{\"edited\":true}".to_string();
    store.put_chat_pushed(&edited).unwrap();
    assert_eq!(
        store.chat_events("R1", &page(None, None, 1)).events[0].event,
        "{\"edited\":true}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mail_threads_page_by_folder_and_a_page_read_again_replaces() {
    let dir = dir("mail");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    let folder = |id: &str| MailFolder {
        folder_id: id.to_string(),
        row: "{}".to_string(),
    };
    assert_eq!(
        store
            .replace_mail_folders(&[folder("INBOX"), folder("Archive"), folder("Old")])
            .unwrap(),
        3
    );
    store
        .replace_mail_folders(&[folder("INBOX"), folder("Archive")])
        .unwrap();
    assert_eq!(store.mail_folders().len(), 2);

    let threads = [
        thread("INBOX", "T1", 10),
        thread("INBOX", "T2", 20),
        thread("INBOX", "T3", 30),
    ];
    assert_eq!(store.put_mail_threads(&threads).unwrap(), 3);
    store.put_mail_threads(&[thread("Archive", "T1", 10)]).unwrap();
    let thread_ids = |folder: &str, before: Option<i64>| -> Vec<String> {
        store
            .mail_threads(folder, before, 2)
            .into_iter()
            .map(|kept| kept.thread_id)
            .collect()
    };
    assert_eq!(thread_ids("INBOX", None), ["T3", "T2"]);
    assert_eq!(thread_ids("INBOX", Some(20)), ["T1"]);

    // the latest page again, down to T2: T3 is gone from the inbox; T1, older than the page, stays
    store
        .replace_mail_page(
            "INBOX",
            None,
            &[thread("INBOX", "T4", 40), thread("INBOX", "T2", 20)],
            true,
        )
        .unwrap();
    let all: Vec<String> = store
        .mail_threads("INBOX", None, 10)
        .into_iter()
        .map(|kept| kept.thread_id)
        .collect();
    assert_eq!(all, ["T4", "T2", "T1"]);
    // the last page (no more): what is older than it is gone too
    store.replace_mail_page("INBOX", Some(20), &[], false).unwrap();
    assert_eq!(thread_ids("INBOX", None), ["T4", "T2"]);
    assert!(store.mail_threads("INBOX", Some(20), 10).is_empty());

    // moved in one unit
    assert!(store.move_mail_thread("T2", "INBOX", "Archive").unwrap());
    assert!(!store.move_mail_thread("T2", "INBOX", "Archive").unwrap());
    assert_eq!(thread_ids("INBOX", None), ["T4"]);
    assert_eq!(thread_ids("Archive", None), ["T2", "T1"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mail_messages_are_kept_once_and_a_thread_read_again_replaces_them() {
    let dir = dir("messages");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    store
        .put_mail_threads(&[thread("INBOX", "T1", 10), thread("Archive", "T1", 10)])
        .unwrap();
    store
        .put_mail_messages(&[message("T1", "M2", 2), message("T1", "M1", 1)])
        .unwrap();
    let message_ids = || -> Vec<String> {
        store
            .mail_messages("T1")
            .into_iter()
            .map(|kept| kept.message_id)
            .collect()
    };
    assert_eq!(message_ids(), ["M1", "M2"]);
    assert_eq!(
        store
            .replace_thread_messages("T1", &[message("T1", "M1", 1), message("T1", "M3", 3)])
            .unwrap(),
        2
    );
    assert_eq!(message_ids(), ["M1", "M3"]);

    // out of one folder the messages stay for the other; out of both they go
    store.remove_mail_thread("INBOX", "T1").unwrap();
    assert_eq!(store.mail_messages("T1").len(), 2);
    store.remove_mail_thread("Archive", "T1").unwrap();
    assert!(store.mail_messages("T1").is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_outbox_keeps_operations_through_their_states() {
    let dir = dir("outbox");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    let file = store.save_file("photo of me.jpg", b"jpeg").unwrap();
    assert!(file.ends_with("-photo_of_me.jpg"), "{file}");
    assert_eq!(store.read_file(&file).as_deref(), Some(&b"jpeg"[..]));

    let mut first = op("ref-1", "mail-send", "", 1);
    first.files = vec![file.clone()];
    let second = op("ref-2", "chat-send", "R1", 2);
    assert!(store.outbox_add(&second).unwrap());
    assert!(store.outbox_add(&first).unwrap());
    // added again (after a crash) it is not queued twice
    assert!(!store.outbox_add(&first).unwrap());
    let mut bad = op("ref-3", "mail-send", "", 3);
    bad.files = vec!["../x".to_string()];
    assert!(store.outbox_add(&bad).is_err());

    assert_eq!(ids(store.outbox_list()), ["ref-1", "ref-2"]);
    assert_eq!(ids(store.outbox_due(10, 10)), ["ref-1", "ref-2"]);
    assert_eq!(store.outbox_get("ref-1").unwrap(), first);

    let update = OutboxUpdate {
        state: OutboxState::Retry,
        attempts: 1,
        next_ms: 100,
        error: Some("offline".to_string()),
    };
    assert!(store.outbox_update("ref-1", &update).unwrap());
    assert!(!store.outbox_update("missing", &update).unwrap());
    assert_eq!(ids(store.outbox_due(10, 10)), ["ref-2"]);
    assert_eq!(ids(store.outbox_due(100, 10)), ["ref-1", "ref-2"]);
    let kept = store.outbox_get("ref-1").unwrap();
    assert_eq!(
        (kept.state, kept.attempts, kept.error.as_deref()),
        (OutboxState::Retry, 1, Some("offline"))
    );

    // held until released
    let held = OutboxUpdate {
        state: OutboxState::Held,
        attempts: 1,
        next_ms: 0,
        error: None,
    };
    store.outbox_update("ref-2", &held).unwrap();
    assert_eq!(ids(store.outbox_due(100, 10)), ["ref-1"]);
    assert_eq!(store.outbox_release_held().unwrap(), 1);
    assert_eq!(store.outbox_get("ref-2").unwrap().state, OutboxState::Retry);

    // a row a newer client wrote with a state this one does not know is left out of a list, not the others
    store
        .conn
        .execute("UPDATE outbox SET state = 'paused' WHERE id = 'ref-2'", [])
        .unwrap();
    assert_eq!(ids(store.outbox_list()), ["ref-1"]);
    assert!(store.outbox_get("ref-2").is_none());
    // and it is still removed
    assert!(store.outbox_remove("ref-2").unwrap());

    // removed with its files
    assert!(store.outbox_remove("ref-1").unwrap());
    assert!(!store.outbox_remove("ref-1").unwrap());
    assert!(store.read_file(&file).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_claim_takes_each_operation_once_in_order_per_target() {
    let dir = dir("claim");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    for added in [
        op("r1-a", "chat-send", "R1", 1),
        op("r1-b", "chat-send", "R1", 2),
        op("r2-a", "chat-send", "R2", 3),
        op("read", "chat-read", "R1", 4),
        op("free", "mail-send", "", 5),
    ] {
        store.outbox_add(&added).unwrap();
    }
    // r1-a waits for a retry: r1-b does not pass it, the other room and kinds go
    let wait = OutboxUpdate {
        state: OutboxState::Retry,
        attempts: 1,
        next_ms: 1000,
        error: None,
    };
    store.outbox_update("r1-a", &wait).unwrap();
    assert_eq!(ids(store.outbox_due(10, 10)), ["r2-a", "read", "free"]);
    let claimed = store.outbox_claim(10, 2);
    assert_eq!(ids(claimed.clone()), ["r2-a", "read"]);
    assert!(claimed
        .iter()
        .all(|op| op.state == OutboxState::Sending && op.attempts == 1));
    // taken once: a second claim gets the rest only
    assert_eq!(ids(store.outbox_claim(10, 10)), ["free"]);
    assert!(store.outbox_claim(10, 10).is_empty());
    // r1-a due: it and r1-b go together, in order
    assert_eq!(ids(store.outbox_claim(1000, 10)), ["r1-a", "r1-b"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_open_makes_what_was_being_sent_due_and_drops_old_acks() {
    let dir = dir("recover");
    {
        let store = Store::open(&dir, "a", 1 << 20).unwrap();
        store.outbox_add(&op("sent", "chat-send", "R1", 1)).unwrap();
        store.outbox_add(&op("acked-old", "chat-read", "R1", 2)).unwrap();
        store.outbox_add(&op("acked-new", "chat-read", "R2", 3)).unwrap();
        assert_eq!(store.outbox_claim(10, 10).len(), 3);
        let acked = OutboxUpdate {
            state: OutboxState::Acked,
            attempts: 1,
            next_ms: 0,
            error: None,
        };
        store.outbox_update("acked-old", &acked).unwrap();
        store.outbox_update("acked-new", &acked).unwrap();
        store
            .conn
            .execute("UPDATE outbox SET updated_ms = 1 WHERE id = 'acked-old'", [])
            .unwrap();
        // the app stops while "sent" is being sent
    }
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    assert_eq!(ids(store.outbox_list()), ["sent", "acked-new"]);
    let sent = store.outbox_get("sent").unwrap();
    assert_eq!((sent.state, sent.attempts), (OutboxState::Retry, 1));
    assert_eq!(ids(store.outbox_claim(now_ms(), 10)), ["sent"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drafts_are_kept_by_key_with_their_files() {
    let dir = dir("drafts");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    let file = store.save_file("a.pdf", b"pdf").unwrap();
    store.put_draft("boss.chat.draft.R1", "{\"text\":\"hi\"}", &[]).unwrap();
    store.put_draft("boss.chat.draft.R2", "{}", &[]).unwrap();
    store
        .put_draft("ironstone.compose.1", "{}", std::slice::from_ref(&file))
        .unwrap();
    store
        .put_draft("boss.chat.draft.R1", "{\"text\":\"hello\"}", &[])
        .unwrap();
    assert_eq!(store.draft("boss.chat.draft.R1").unwrap().value, "{\"text\":\"hello\"}");
    assert_eq!(store.draft("ironstone.compose.1").unwrap().files, std::slice::from_ref(&file));
    assert_eq!(store.drafts("boss.chat.").len(), 2);
    assert_eq!(store.drafts("").len(), 3);
    assert!(store.put_draft("x", "{}", &["a/b".to_string()]).is_err());
    assert!(store.remove_draft("boss.chat.draft.R2").unwrap());
    assert!(!store.remove_draft("boss.chat.draft.R2").unwrap());
    assert!(store.draft("boss.chat.draft.R2").is_none());

    // a file the outbox also sends stays when the draft goes, and goes with the operation
    let mut send = op("ref-1", "mail-send", "", 1);
    send.files = vec![file.clone()];
    store.outbox_add(&send).unwrap();
    store.remove_draft("ironstone.compose.1").unwrap();
    assert!(store.read_file(&file).is_some());
    store.outbox_remove("ref-1").unwrap();
    assert!(store.read_file(&file).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_no_row_refers_to_are_collected_but_not_inside_a_unit() {
    let dir = dir("gc");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    let loose = store.save_file("loose.txt", b"x").unwrap();
    let drafted = store.save_file("drafted.txt", b"x").unwrap();
    let queued = store.save_file("queued.txt", b"x").unwrap();
    store.put_draft("d", "{}", std::slice::from_ref(&drafted)).unwrap();
    let mut send = op("ref-1", "mail-send", "", 1);
    send.files = vec![queued.clone()];
    store.outbox_add(&send).unwrap();
    // a new file is in its grace
    assert_eq!(store.gc_files(attach::FILE_GRACE), 0);
    // inside a unit nothing goes: the unit could still be undone
    let removed = store.atomic(|store| {
        store.outbox_remove("ref-1")?;
        Ok(store.gc_files(Duration::ZERO))
    });
    assert_eq!(removed.unwrap(), 0);
    assert!(store.read_file(&queued).is_some());
    assert_eq!(store.gc_files(Duration::ZERO), 2);
    assert!(store.read_file(&loose).is_none() && store.read_file(&queued).is_none());
    assert!(store.read_file(&drafted).is_some());

    // names that leave the directory are refused
    for name in ["..", "../a.db", "a/b", "/etc/passwd", ".hidden", ""] {
        assert!(store.file_path(name).is_none(), "{name}");
        assert!(store.read_file(name).is_none(), "{name}");
        assert!(store.remove_file(name).is_err(), "{name}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn past_the_cap_the_least_read_go_and_the_outbox_and_drafts_stay() {
    let dir = dir("cap");
    let mut store = Store::open(&dir, "a", 1 << 30).unwrap();
    let body = vec![b'x'; 1000];
    store.put_answer("/old", "", "application/json", &body).unwrap();
    store.put_chat_room(&room("R1", 1)).unwrap();
    store
        .put_chat_events(&[event("R1", 1, None), event("R1", 2, None)])
        .unwrap();
    store.put_answer("/new", "", "application/json", &body).unwrap();
    store.put_mail_threads(&[thread("INBOX", "T1", 1)]).unwrap();
    let mut queued = op("ref-1", "chat-send", "R1", 1);
    queued.payload = "y".repeat(2000);
    queued.files = vec![store.save_file("a.txt", &[b'z'; 2000]).unwrap()];
    store.outbox_add(&queued).unwrap();
    store.put_draft("d", &"w".repeat(2000), &[]).unwrap();
    // read /old so it is newer than the room and /new
    assert!(store.answer("/old", "").is_some());
    let before = store.size_bytes();

    // just past the cap: only the least read goes, the room with its events
    store.set_max_bytes(before - 100);
    assert_eq!(store.trim().unwrap(), 1);
    assert!(store.chat_room("R1").is_none());
    assert!(store.chat_events("R1", &page(None, None, 10)).events.is_empty());
    assert!(store.answer("/new", "").is_some());

    // a cap below what is pinned keeps a quarter of it for cached reads: here /old, read last, fits
    assert!(store.answer("/old", "").is_some());
    let pinned = store.bytes().pinned;
    store.set_max_bytes(u64::try_from(pinned).unwrap());
    store.trim().unwrap();
    let reads = store.bytes().reads;
    assert!(reads > 0 && reads <= pinned / 4, "{reads} of {pinned}");
    assert!(store.answer("/old", "").is_some());
    assert!(store.answer("/new", "").is_none());
    assert!(store.mail_threads("INBOX", None, 10).is_empty());
    assert!(store.outbox_get("ref-1").is_some());
    assert!(store.draft("d").is_some());
    assert!(store.read_file(&queued.files[0]).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn writes_past_the_cap_evict_on_their_own() {
    let dir = dir("cap-writes");
    let store = Store::open(&dir, "a", 10_000).unwrap();
    let body = vec![b'x'; 1000];
    for index in 0..30 {
        store
            .put_answer(&format!("/a{index}"), "", "application/json", &body)
            .unwrap();
    }
    assert!(store.size_bytes() <= 10_000, "{}", store.size_bytes());
    assert!(store.answer("/a29", "").is_some());
    assert!(store.answer("/a0", "").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_unit_keeps_all_or_none_of_its_writes() {
    let dir = dir("atomic");
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    let failed: Result<(), Error> = store.atomic(|store| {
        store.put_draft("d", "{}", &[])?;
        store.outbox_add(&op("ref-1", "chat-send", "R1", 1))?;
        Err(Error::new("no"))
    });
    assert!(failed.is_err());
    assert!(store.draft("d").is_none() && store.outbox_get("ref-1").is_none());

    // nested: an inner failure the outer catches undoes the inner writes only
    store
        .atomic(|store| {
            store.put_draft("outer", "{}", &[])?;
            let inner: Result<(), Error> = store.atomic(|store| {
                store.put_draft("inner", "{}", &[])?;
                Err(Error::new("inner"))
            });
            assert!(inner.is_err());
            store.atomic(|store| store.put_draft("inner-ok", "{}", &[]))
        })
        .unwrap();
    assert!(store.draft("outer").is_some() && store.draft("inner-ok").is_some());
    assert!(store.draft("inner").is_none());

    // an inner unit that succeeded is undone with the outer one
    let failed: Result<(), Error> = store.atomic(|store| {
        store.atomic(|store| store.put_draft("nested", "{}", &[]))?;
        Err(Error::new("outer"))
    });
    assert!(failed.is_err());
    assert!(store.draft("nested").is_none());

    // a panic undoes the unit, and the store goes on outside any unit
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), Error> = store.atomic(|store| {
            store.put_draft("panic", "{}", &[])?;
            panic!("in a unit");
        });
    }));
    assert!(panicked.is_err());
    assert!(store.conn.is_autocommit());
    assert!(store.draft("panic").is_none());
    store.put_draft("after", "{}", &[]).unwrap();
    assert!(store.draft("after").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn forget_removes_the_database_and_files() {
    let dir = dir("forget");
    {
        let store = Store::open(&dir, "a", 1 << 20).unwrap();
        let file = store.save_file("f.txt", b"f").unwrap();
        store.put_draft("d", "{}", &[file]).unwrap();
        let _other = Store::open(&dir, "b", 1 << 20).unwrap();
    }
    assert!(dir.join("data/a.db").exists() && dir.join("data/a.files").exists());
    forget(&dir, "a").unwrap();
    let left = |prefix: &str| -> Vec<String> {
        std::fs::read_dir(dir.join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(prefix))
            .collect()
    };
    assert!(left("a.").is_empty(), "{:?}", left("a."));
    assert!(dir.join("data/b.db").exists());
    // none there is not an error
    forget(&dir, "a").unwrap();
    assert!(forget(&dir, "..").is_err());
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    assert!(store.draft("d").is_none());
    // the store's own forget closes it first
    Store::open(&dir, "b", 1 << 20).unwrap().forget().unwrap();
    assert!(left("b.").is_empty(), "{:?}", left("b."));
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
