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

//! The Assist pane (plans/LLM-A2A-Assist.md 4.12) against a fake server: the `/assist/*` calls answered with the shapes
//! of docs/Assist.md, the pane at 100x40 (over the canvas) and 180x50 (a right column) with a thread, an approval and a
//! follow fill, and the calls the pane makes.
//!
//! `UPDATE_GOLDEN=1` rewrites `tests/snapshots/assist-*.txt`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use moqui_tui::{AppView, FakeLink, Key, KeyPress, LinkCall, Push, ScreenModel};
use serde_json::{json, Value};

const TASKS: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/tasks/Tasks", "title": "Tasks" },
  "c": [
    { "t": "text", "id": "intro", "text": "Open tasks" },
    { "t": "link", "id": "open", "text": "Ship it", "url": "/apps/tasks/EditTask?taskId=T1" }
  ]
}"#;

const EDIT: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/tasks/EditTask", "title": "Edit Task" },
  "c": [
    { "t": "form", "id": "EditTask", "name": "EditTask", "transitionUrl": "/apps/tasks/EditTask/updateTask", "c": [
      { "t": "field", "id": "EditTask.taskName", "name": "taskName", "title": "Task", "w": { "kind": "textLine", "value": "Ship it" } },
      { "t": "field", "id": "EditTask.statusId", "name": "statusId", "title": "Status",
        "w": { "kind": "dropDown", "value": "Open", "options": [ { "value": "Open", "label": "Open" }, { "value": "Closed", "label": "Closed" } ] } },
      { "t": "field", "id": "EditTask.note", "name": "note", "title": "Note", "w": { "kind": "textLine", "value": "" } },
      { "t": "field", "id": "EditTask.save", "name": "save", "title": "Save", "w": { "kind": "submit", "text": "Save" } }
    ] }
  ]
}"#;

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}. Set UPDATE_GOLDEN=1", path.display()));
    assert_eq!(expected, actual, "{}", path.display());
}

fn press(view: &mut AppView, key: Key) {
    view.on_key(&KeyPress::plain(key));
}

fn typed(view: &mut AppView, text: &str) {
    for ch in text.chars() {
        press(view, Key::Char(ch));
    }
}

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn event(seq: u64, kind: &str, payload: Value) -> Value {
    json!({"seq": seq, "type": kind, "payload": payload})
}

/// The stored events of run `r1` after `after`, as `GET events` answers them.
fn events(link: &FakeLink, after: u64, list: Vec<Value>, state: &str) {
    let last = list.last().and_then(|event| event["seq"].as_u64()).unwrap_or(after);
    link.answer_json(
        "/assist/events",
        params(&[("runId", "r1"), ("after", &after.to_string())]),
        json!({"events": list, "lastEventSeq": last, "statusId": "AgrWorking", "state": state}),
    );
}

/// A session on the Tasks screen with the pane open (F2) and a message sent: the server makes conversation `c1` and
/// run `r1`, owned by this client at page version 1. Answers the view, the link and this client's id.
fn asked(text: &str) -> (AppView, FakeLink, String) {
    let link = FakeLink::from_json(&[TASKS, EDIT]).unwrap();
    let mut view = AppView::new(ScreenModel::from_json(TASKS).unwrap(), link.clone());
    view.set_clock("Tue 14:32");
    let client = view.session_mut().assist_client();
    link.answer_json(
        "/assist/bootstrap",
        BTreeMap::new(),
        json!({"agents": [{"agentId": "assist", "name": "Assist"}], "defaultAgentId": "assist",
               "features": {"enabled": true, "screenFollow": true}, "limits": {"maxMessageChars": 16000}}),
    );
    link.answer_json(
        "/assist/turn",
        BTreeMap::new(),
        json!({"conversationId": "c1", "runId": "r1", "state": "TASK_STATE_SUBMITTED", "statusId": "AgrSubmitted",
               "queued": false, "existing": false, "lastEventSeq": 0,
               "follow": {"clientId": client, "contextVersion": 1}}),
    );
    press(&mut view, Key::F(2));
    assert!(view.session().assist_focused());
    typed(&mut view, text);
    (view, link, client)
}

/// The calls made to `/assist/<call>`, with their parameters.
fn assist_calls(link: &FakeLink, call: &str) -> Vec<BTreeMap<String, String>> {
    link.calls()
        .into_iter()
        .filter_map(|made| match made {
            LinkCall::Json { url, parameters, .. } if url == format!("/assist/{call}") => Some(parameters),
            _ => None,
        })
        .collect()
}

#[test]
fn a_thread_in_the_pane_over_the_canvas_and_beside_it() {
    let (mut view, link, _) = asked("How many tasks are open?");
    events(
        &link,
        0,
        vec![
            event(1, "run.state", json!({"runId": "r1", "state": "TASK_STATE_WORKING"})),
            event(2, "message.start", json!({"messageId": "a1", "role": "assistant"})),
            event(
                3,
                "tool.call",
                json!({"toolCallId": "t1", "messageId": "a1", "name": "screen",
                                         "arguments": "{\"go\": \"/apps/tasks/Tasks\"}", "classification": "read"}),
            ),
            event(
                4,
                "tool.result",
                json!({"toolCallId": "t1", "name": "screen", "status": "done", "isError": false}),
            ),
            event(
                5,
                "text.chunk",
                json!({"messageId": "a1", "part": 0, "offset": 0,
                                          "text": "One task is open: Ship it, due Friday.\n\nI drew the open tasks by owner."}),
            ),
            event(
                6,
                "canvas.open",
                json!({"artifactId": "90", "title": "Open tasks", "version": 1}),
            ),
            event(7, "usage", json!({"stepSeq": 1, "input": 900, "output": 40})),
            event(8, "run.state", json!({"runId": "r1", "state": "TASK_STATE_COMPLETED"})),
            event(9, "run.done", json!({"runId": "r1", "state": "TASK_STATE_COMPLETED"})),
        ],
        "TASK_STATE_COMPLETED",
    );
    press(&mut view, Key::Enter);
    let turn = &assist_calls(&link, "turn")[0];
    assert_eq!(turn["text"], "How many tasks are open?");
    assert_eq!(turn["surface"], "tui");
    assert_eq!(turn["agentId"], "assist");
    let state: Value = serde_json::from_str(&turn["state"]).unwrap();
    assert_eq!(state["path"], "/apps/tasks/Tasks");
    assert!(!turn["clientMessageId"].is_empty());
    let narrow = view.screen_text(100, 40);
    assert!(narrow.contains("> How many tasks are open?"), "{narrow}");
    assert!(narrow.contains("· screen /apps/tasks/Tasks ok"), "{narrow}");
    assert!(narrow.contains("✓ Done · 1 step · 940 tokens"), "{narrow}");
    assert!(narrow.contains("This answer has a canvas"), "{narrow}");
    golden("assist-thread-100x40", &narrow);
    let wide = view.screen_text(180, 50);
    golden("assist-thread-180x50", &wide);
    // Esc gives the keys back: over the canvas the pane hides, beside it it stays
    press(&mut view, Key::Escape);
    assert!(!view.screen_text(100, 40).contains("This answer has a canvas"));
    assert!(view.screen_text(180, 50).contains("This answer has a canvas"));
    // `o` in the transcript opens the canvas in the browser
    press(&mut view, Key::F(2));
    press(&mut view, Key::Tab);
    press(&mut view, Key::Char('o'));
    assert_eq!(
        view.session_mut().take_open().as_deref(),
        Some("/apps/tasks/Tasks?assist=c1")
    );
}

#[test]
fn an_approval_card_reviews_in_a_confirmation_and_reject_asks_for_feedback() {
    let (mut view, link, _) = asked("Close Ship it");
    events(
        &link,
        0,
        vec![
            event(1, "run.state", json!({"runId": "r1", "state": "TASK_STATE_WORKING"})),
            event(
                2,
                "tool.call",
                json!({"toolCallId": "t1", "messageId": "a1", "name": "screen",
                                         "arguments": "{\"do\": \"Save\"}", "classification": "write"}),
            ),
            event(
                3,
                "approval.requested",
                json!({"approvalId": "ap1", "runId": "r1", "toolCallId": "t1",
                "classification": "write", "targetKind": "screen", "targetName": "updateTask",
                "summary": "Update task Ship it", "canEdit": true, "status": "pending",
                "changes": [{"field": "statusId", "label": "Status", "current": "Open", "proposed": "Closed"},
                            {"field": "note", "label": "Note", "current": "", "proposed": "Done in review"}]}),
            ),
            event(
                4,
                "run.state",
                json!({"runId": "r1", "state": "TASK_STATE_INPUT_REQUIRED", "pauseKind": "approval"}),
            ),
        ],
        "TASK_STATE_INPUT_REQUIRED",
    );
    press(&mut view, Key::Enter);
    let waiting = view.screen_text(100, 40);
    assert!(waiting.contains("Waiting for your approval"), "{waiting}");
    assert!(waiting.contains("Status: Open → Closed"), "{waiting}");
    // the cards are next after the composer; Enter reviews the card
    press(&mut view, Key::Tab);
    press(&mut view, Key::Enter);
    let narrow = view.screen_text(100, 40);
    assert!(
        narrow.contains("[ Approve ]  [ Edit in screen ]  [ Reject ]"),
        "{narrow}"
    );
    assert!(narrow.contains("Note: (empty) → Done in review"), "{narrow}");
    golden("assist-approval-100x40", &narrow);
    golden("assist-approval-180x50", &view.screen_text(180, 50));
    // no letter decides, and Esc leaves it pending
    press(&mut view, Key::Char('a'));
    press(&mut view, Key::Escape);
    assert!(view.session().confirm().is_none());
    assert!(assist_calls(&link, "decide").is_empty());
    // Reject opens a prompt for what Assist should do instead
    press(&mut view, Key::Enter);
    press(&mut view, Key::Right);
    press(&mut view, Key::Right);
    link.answer_json(
        "/assist/decide",
        BTreeMap::new(),
        json!({"approvalId": "ap1", "status": "rejected", "statusId": "AgaRejected"}),
    );
    events(
        &link,
        4,
        vec![
            event(
                5,
                "approval.decided",
                json!({"approvalId": "ap1", "toolCallId": "t1", "decision": "reject",
                                                "status": "rejected", "feedback": "Leave it open"}),
            ),
            event(6, "run.state", json!({"runId": "r1", "state": "TASK_STATE_WORKING"})),
        ],
        "TASK_STATE_WORKING",
    );
    press(&mut view, Key::Enter);
    let asking = view.screen_text(100, 40);
    assert!(asking.contains("Reject: what should Assist do instead?"), "{asking}");
    typed(&mut view, "Leave it open");
    press(&mut view, Key::Enter);
    let decided = assist_calls(&link, "decide");
    assert_eq!(decided.len(), 1);
    assert_eq!(decided[0]["decision"], "reject");
    assert_eq!(decided[0]["feedback"], "Leave it open");
    assert_eq!(decided[0]["approvalId"], "ap1");
    let after = view.screen_text(100, 40);
    assert!(after.contains("Rejected: Update task Ship it"), "{after}");
    assert!(!after.contains("[ Approve ]"), "{after}");
    // the run works again: its events are read on the timer
    let polls = assist_calls(&link, "events").len();
    view.session_mut().on_time(Instant::now() + Duration::from_secs(2));
    view.run_requests();
    let last = assist_calls(&link, "events");
    assert_eq!(last.len(), polls + 1);
    assert_eq!(last[polls]["after"], "6");
}

#[test]
fn follow_navigates_and_fills_the_screen_and_a_hand_off_focuses_save() {
    let (mut view, link, client) = asked("Close Ship it");
    let fill = |seq, field: &str, tui: &str, value: &str, label: &str| {
        event(
            seq,
            "ui.fill",
            json!({"scope": "page", "form": "EditTask", "domId": "EditTask", "occurrence": 0,
            "field": field, "value": value, "label": label, "tuiId": tui, "clientId": client, "contextVersion": 1}),
        )
    };
    events(
        &link,
        0,
        vec![
            event(1, "run.state", json!({"runId": "r1", "state": "TASK_STATE_WORKING"})),
            event(
                2,
                "tool.call",
                json!({"toolCallId": "t1", "messageId": "a1", "name": "screen",
                                         "arguments": "{\"go\": \"/apps/tasks/EditTask?taskId=T1\"}"}),
            ),
            event(
                3,
                "ui.navigate",
                json!({"path": "/qapps/tasks/EditTask", "parameters": {"taskId": "T1"},
                                           "title": "tasks ▸ Edit Task", "clientId": client, "contextVersion": 1}),
            ),
            event(
                4,
                "tool.result",
                json!({"toolCallId": "t1", "name": "screen", "status": "done"}),
            ),
            event(
                5,
                "tool.call",
                json!({"toolCallId": "t2", "messageId": "a1", "name": "screen",
                                         "arguments": "{\"set\": \"Status Closed\"}"}),
            ),
            fill(6, "statusId", "EditTask.statusId", "Closed", "Status"),
            // another client's fill is a note, not a change
            event(
                7,
                "ui.fill",
                json!({"form": "EditTask", "field": "note", "value": "x", "label": "Note",
                                       "tuiId": "EditTask.note", "clientId": "tab-other", "contextVersion": 1}),
            ),
            event(
                8,
                "tool.result",
                json!({"toolCallId": "t2", "name": "screen", "status": "done"}),
            ),
        ],
        "TASK_STATE_WORKING",
    );
    press(&mut view, Key::Enter);
    let session = view.session();
    assert_eq!(session.path(), "/apps/tasks/EditTask");
    assert_eq!(session.parameters().get("taskId").map(String::as_str), Some("T1"));
    assert_eq!(session.buffer("EditTask.statusId"), "Closed");
    assert!(session.is_dirty("EditTask.statusId"));
    assert!(session.assist_filled("EditTask.statusId"));
    assert_eq!(session.buffer("EditTask.note"), "");
    // the agent's navigation is not the user's: no context is posted
    assert!(assist_calls(&link, "context").is_empty());
    let narrow = view.screen_text(100, 40);
    assert!(narrow.contains("[Closed"), "{narrow}");
    assert!(narrow.contains("Assist filled Status"), "{narrow}");
    golden("assist-fill-100x40", &narrow);
    let wide = view.screen_text(180, 50);
    assert!(wide.contains(" Assist"), "{wide}");
    golden("assist-fill-180x50", &wide);
    // a fill by name (no tui id) and then the hand-off: Save takes the focus, the keys go to the screen
    events(
        &link,
        8,
        vec![
            fill(9, "note", "", "Done in review", "Note"),
            event(
                10,
                "handoff.started",
                json!({"handoffId": "h1", "approvalId": "ap1", "path": "/apps/tasks/EditTask",
                "parameters": {"taskId": "T1"}, "form": "EditTask", "transition": "updateTask",
                "values": [{"field": "statusId", "label": "Status", "value": "Closed"}],
                "clientId": client, "contextVersion": 1}),
            ),
            event(
                11,
                "run.state",
                json!({"runId": "r1", "state": "TASK_STATE_INPUT_REQUIRED", "pauseKind": "handoff"}),
            ),
        ],
        "TASK_STATE_INPUT_REQUIRED",
    );
    view.session_mut().on_time(Instant::now() + Duration::from_secs(2));
    view.run_requests();
    let session = view.session();
    assert_eq!(session.buffer("EditTask.note"), "Done in review");
    assert_eq!(session.focus_id(), "EditTask.save");
    assert!(!session.assist_focused());
    assert_eq!(
        session.message(),
        "Assist filled Edit Task — review, then Enter on Save"
    );
    // the user's own navigation raises the page version and tells the server
    view.session_mut().navigate("/apps/tasks/Tasks", true);
    view.run_requests();
    let context = assist_calls(&link, "context");
    assert_eq!(context.len(), 1);
    assert_eq!(context[0]["contextVersion"], "2");
    assert_eq!(context[0]["conversationId"], "c1");
    // a stale fill (page version 1) no longer applies
    events(
        &link,
        11,
        vec![fill(12, "note", "EditTask.note", "late", "Note")],
        "TASK_STATE_WORKING",
    );
    view.session_mut().on_push(&Push {
        topic: "AgentRun".to_string(),
        title: String::new(),
        message: json!({"runId": "r1", "conversationId": "c1", "event": "approval.decided"}),
    });
    view.run_requests();
    assert_eq!(view.session().path(), "/apps/tasks/Tasks");
    assert_eq!(assist_calls(&link, "events").last().unwrap()["after"], "11");
}

#[test]
fn keys_stop_and_the_servers_errors_on_the_message_line() {
    let (mut view, link, _) = asked("Stop now");
    events(
        &link,
        0,
        vec![event(
            1,
            "run.state",
            json!({"runId": "r1", "state": "TASK_STATE_WORKING"}),
        )],
        "TASK_STATE_WORKING",
    );
    press(&mut view, Key::Enter);
    // Alt+X stops the run; a refusal shows the server's text
    view.on_key(&KeyPress::with_alt(Key::Char('x')));
    assert_eq!(assist_calls(&link, "cancel")[0]["runId"], "r1");
    assert!(
        view.session().message().starts_with("no screen at /assist/cancel"),
        "{}",
        view.session().message()
    );
    // F10 offers New, History, Tasks, Agent and Follow; Follow turns off and releases it
    link.answer_json(
        "/assist/follow",
        BTreeMap::new(),
        json!({"conversationId": "c1", "follow": {"clientId": null, "contextVersion": 1}}),
    );
    press(&mut view, Key::F(10));
    let menu = view.screen_text(100, 40);
    for entry in ["New conversation", "History", "Tasks", "Agent", "Follow: on"] {
        assert!(menu.contains(entry), "{entry}: {menu}");
    }
    press(&mut view, Key::Char('f'));
    assert_eq!(assist_calls(&link, "follow")[0]["release"], "true");
    assert!(view.session().assist().follow_off);
    // F4 writes several lines; Alt+Enter sends them; a turn the server refuses is marked not sent
    press(&mut view, Key::F(4));
    typed(&mut view, "one");
    press(&mut view, Key::Enter);
    typed(&mut view, "two");
    view.on_key(&KeyPress::with_alt(Key::Enter));
    let turns = assist_calls(&link, "turn");
    assert_eq!(turns.last().unwrap()["text"], "one\ntwo");
    assert_eq!(turns.last().unwrap()["conversationId"], "c1");
    // F2 closes the pane
    press(&mut view, Key::F(2));
    assert!(!view.session().assist().open);
    let text = view.screen_text(180, 50);
    assert!(!text.contains("Assist ·"), "{text}");
}
