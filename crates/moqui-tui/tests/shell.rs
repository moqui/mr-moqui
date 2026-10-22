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

//! The shell around the screens: the palette and go to, quick create, the counters, the tray, the help's documents,
//! the message history, find text, the drawer's choice, and a list's saved finds, columns and exports. No Revue.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_tui::{
    FakeLink, Key, KeyPress, LinkCall, MenuItem, Overlay, RawBody, Session, ShellAction, ShellInfo, Timer,
};

const MAIN_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/Main", "pathList": ["apps", "app", "Main"], "title": "Main" },
  "c": [
    { "t": "link", "id": "toOrders", "text": "Orders", "url": "/apps/app/Orders" },
    { "t": "link", "id": "toParties", "text": "Parties", "url": "/apps/app/Parties" },
    { "t": "formList", "id": "Things", "name": "Things",
      "columns": [{ "title": "Name" }, { "title": "Status" }],
      "rows": [ { "id": "T1", "cells": ["Anvil", "Open"] }, { "id": "T2", "cells": ["Bellows", "Closed"] } ],
      "headerFields": [ { "t": "field", "id": "Things.name", "name": "name", "title": "Name", "w": { "kind": "textLine", "value": "" } } ],
      "savedFinds": { "saveUrl": "/apps/app/Main/formSaveFind", "formLocation": "Main.Things", "parameters": { "name": "A" },
        "clearUrl": "/apps/app/Main?formListFindId=_clear",
        "finds": [ { "id": "F1", "name": "Open things", "url": "/apps/app/Main?formListFindId=F1", "mine": true } ] },
      "selectColumns": { "url": "/apps/app/Main/formSelectColumns", "formLocation": "Things", "parameters": { "name": "A" },
        "shown": [[{ "name": "name", "title": "Name" }], [{ "name": "status", "title": "Status" }]],
        "hidden": [{ "name": "notes", "title": "Notes" }] },
      "exports": [ { "kind": "csv", "label": "CSV", "url": "/apps/app/Main?renderMode=csv" },
                   { "kind": "text", "label": "Text", "url": "/apps/app/Main?renderMode=text" } ] }
  ]
}"#;

const QUICK_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/QuickTask" },
  "c": [ { "t": "form", "id": "Q", "name": "Q", "transitionUrl": "/apps/app/QuickTask/create",
    "c": [ { "t": "field", "id": "Q.title", "name": "title", "title": "Title", "w": { "kind": "textLine", "value": "" } } ] } ]
}"#;

const TODAY_JSON: &str = r#"{ "v": 1, "screen": { "path": "/apps/my/Today" }, "c": [ { "t": "text", "id": "t", "text": "Nothing due" } ] }"#;
const CHAT_JSON: &str = r#"{ "v": 1, "screen": { "path": "/apps/my/Chat" }, "c": [ { "t": "text", "id": "t", "text": "No chats" } ] }"#;

fn press(session: &mut Session, link: &FakeLink, key: Key) -> bool {
    session.press(&KeyPress::plain(key), link)
}

fn typed(session: &mut Session, link: &FakeLink, text: &str) {
    for ch in text.chars() {
        press(session, link, Key::Char(ch));
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

fn shell() -> ShellInfo {
    ShellInfo {
        menu: vec![MenuItem {
            title: "App".to_string(),
            url: "/apps/app".to_string(),
            children: vec![MenuItem {
                title: "Orders".to_string(),
                url: "/apps/app/Orders".to_string(),
                children: Vec::new(),
            }],
        }],
        user: "john.doe".to_string(),
        apps: vec![
            MenuItem {
                title: "Sales".to_string(),
                url: "/apps/sales".to_string(),
                children: Vec::new(),
            },
            MenuItem {
                title: "Tools".to_string(),
                url: "/apps/tools".to_string(),
                children: Vec::new(),
            },
        ],
        quick_create: vec![ShellAction {
            label: "New task".to_string(),
            url: "/apps/app/QuickTask".to_string(),
            app: "App".to_string(),
        }],
        tray: vec![
            ("Today".to_string(), "/apps/my/Today".to_string()),
            ("Chat".to_string(), "/apps/my/Chat".to_string()),
        ],
        counts: BTreeMap::from([("chat".to_string(), 1), ("today".to_string(), 3)]),
        drawer_open: Some(true),
        ..ShellInfo::default()
    }
}

fn started() -> (Session, FakeLink) {
    let link = FakeLink::from_json(&[MAIN_JSON, QUICK_JSON, TODAY_JSON, CHAT_JSON]).unwrap();
    link.answer_shell(shell());
    link.answer_json(
        "/menuData/apps/app/Main",
        BTreeMap::new(),
        serde_json::json!([{ "title": "App" }, {
            "title": "Main",
            "screenDocList": [{ "title": "How to", "index": "1" }],
            "savedFinds": [{ "name": "F1", "title": "Open things", "pathWithParams": "/apps/app/Main?formListFindId=F1" }]
        }]),
    );
    let mut session = Session::start("/apps/app/Main");
    session.run_now(&link);
    (session, link)
}

fn calls(link: &FakeLink) -> Vec<LinkCall> {
    link.calls()
}

fn last_transition(link: &FakeLink) -> Option<(String, BTreeMap<String, String>)> {
    calls(link)
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Transition { url, fields, .. } => Some((url, fields)),
            _ => None,
        })
        .next_back()
}

#[test]
fn the_palette_finds_apps_then_asks_the_server_once_the_typing_stops() {
    let (mut session, link) = started();
    link.answer_json(
        "/shellSearch",
        map(&[("text", "sa"), ("limit", "5")]),
        serde_json::json!({ "groups": [{ "label": "Orders", "rows": [{ "title": "SO-10023", "url": "/apps/sales/Order?orderId=SO-10023" }] }] }),
    );
    session.press(&KeyPress::with_ctrl(Key::Char('k')), &link);
    typed(&mut session, &link, "sa");
    let titles = |session: &Session| -> Vec<String> { session.palette_rows().into_iter().map(|row| row.title).collect() };
    assert_eq!(titles(&session), ["Sales"]);
    assert!(session.timer(&Timer::Palette).is_some());
    session.on_time(Instant::now() + Duration::from_secs(1));
    session.run_now(&link);
    assert_eq!(titles(&session), ["Sales", "SO-10023"]);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Enter);
    assert!(calls(&link).iter().any(|call| matches!(
        call,
        LinkCall::Level { path, parameters } if path == "/apps/sales/Order" && parameters.get("orderId").map(String::as_str) == Some("SO-10023")
    )));
}

#[test]
fn go_to_takes_a_path_or_a_menu_title() {
    let (mut session, link) = started();
    press(&mut session, &link, Key::Char('g'));
    typed(&mut session, &link, "apps/tools");
    press(&mut session, &link, Key::Enter);
    assert!(calls(&link)
        .iter()
        .any(|call| matches!(call, LinkCall::Level { path, .. } if path == "/apps/tools")));
}

#[test]
fn quick_create_opens_its_form_in_a_dialog() {
    let (mut session, link) = started();
    press(&mut session, &link, Key::F(6));
    let Some(Overlay::Menu(menu)) = session.top_overlay() else {
        panic!("{:?}", session.overlays());
    };
    assert_eq!(menu.items[0].label, "New task");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.dialogs_open().len(), 1);
    assert_eq!(session.focus_id(), "shell-quick/Q.title");
    typed(&mut session, &link, "Call Acme");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        last_transition(&link),
        Some(("/apps/app/QuickTask/create".to_string(), map(&[("title", "Call Acme")])))
    );
    assert!(session.dialogs_open().is_empty());
    assert!(session.extras().is_empty());
}

#[test]
fn the_counters_come_with_the_shell_and_are_read_again_each_minute() {
    let (mut session, link) = started();
    assert_eq!(session.counts().get("today"), Some(&3));
    link.answer_json("/shellCounts", BTreeMap::new(), serde_json::json!({ "counts": { "today": 4, "chat": 0 } }));
    let at = session.timer(&Timer::Counts).expect("a count timer");
    assert!(at > Instant::now() + Duration::from_secs(50));
    session.on_time(at);
    session.run_now(&link);
    assert_eq!(session.counts().get("today"), Some(&4));
    assert!(session.timer(&Timer::Counts).is_some());
}

#[test]
fn the_tray_shows_its_tabs_and_left_and_right_go_between_them() {
    let (mut session, link) = started();
    session.press(&KeyPress::with_alt(Key::Char('t')), &link);
    let extra = &session.extras()[0];
    assert!(extra.title.starts_with("Today"), "{}", extra.title);
    assert!(matches!(&extra.c[0], moqui_tui::Node::Text { text, .. } if text == "Nothing due"));
    press(&mut session, &link, Key::Right);
    assert_eq!(session.tray_tab(), 1);
    assert!(matches!(&session.extras()[0].c[0], moqui_tui::Node::Text { text, .. } if text == "No chats"));
    press(&mut session, &link, Key::Escape);
    assert!(session.extras().is_empty());
}

#[test]
fn the_help_lists_the_screen_documents_and_opens_one() {
    let (mut session, link) = started();
    assert_eq!(session.docs(), [("How to".to_string(), "1".to_string())]);
    link.answer_json(
        "/apps/app/Main/screenDoc",
        map(&[("docIndex", "1"), ("renderMode", "tui")]),
        serde_json::json!({ "t": "html", "id": "screenDoc", "blocks": [{ "kind": "p", "text": "Do this." }] }),
    );
    press(&mut session, &link, Key::F(1));
    press(&mut session, &link, Key::Char('1'));
    let doc = &session.extras()[0];
    assert_eq!(doc.title, "How to");
    assert!(matches!(&doc.c[0], moqui_tui::Node::Html { blocks, .. } if blocks[0].text == "Do this."));
    // The saved finds of the screen are in the drawer.
    let drawer: Vec<String> = session.drawer_rows().into_iter().map(|row| row.0).collect();
    assert!(drawer.contains(&"Saved finds".to_string()) && drawer.contains(&"Open things".to_string()), "{drawer:?}");
}

#[test]
fn find_text_moves_to_a_link_or_a_list_row_and_n_goes_on() {
    let (mut session, link) = started();
    press(&mut session, &link, Key::Char('/'));
    typed(&mut session, &link, "bell");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.focus_id(), "Things");
    assert_eq!(session.row_at("Things"), 1);
    session.press(&KeyPress::plain(Key::Char('/')), &link);
    typed(&mut session, &link, "r");
    press(&mut session, &link, Key::Enter);
    let first = session.focus_id().to_string();
    press(&mut session, &link, Key::Char('n'));
    assert_ne!(session.focus_id(), first, "n goes to the next match");
    press(&mut session, &link, Key::Char('/'));
    typed(&mut session, &link, "zzz");
    press(&mut session, &link, Key::Enter);
    assert!(session.message_is_error());
    // Every message is in the history.
    session.press(&KeyPress::with_alt(Key::Char('m')), &link);
    assert_eq!(session.top_overlay(), Some(&Overlay::Messages));
    assert!(session.history().iter().any(|message| message.text.contains("zzz")));
}

#[test]
fn the_drawer_choice_is_kept_as_a_preference() {
    let (mut session, link) = started();
    // The shell's preference: open.
    assert_eq!(session.drawer_choice(), Some(true));
    link.answer_json("/apps/setPreference", BTreeMap::new(), serde_json::json!({}));
    press(&mut session, &link, Key::F(9));
    press(&mut session, &link, Key::F(9));
    let kept: Vec<BTreeMap<String, String>> = calls(&link)
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Json { url, parameters, .. } if url == "/apps/setPreference" => Some(parameters),
            _ => None,
        })
        .collect();
    assert_eq!(
        kept.last(),
        Some(&map(&[("preferenceKey", "TUI_DRAWER_OPEN"), ("preferenceValue", "false")]))
    );
}

#[test]
fn saved_finds_apply_save_and_delete() {
    let (mut session, link) = started();
    while session.focus_id() != "Things" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::F(10));
    let Some(Overlay::Menu(menu)) = session.top_overlay().cloned() else {
        panic!();
    };
    let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, ["Find…", "Saved finds…", "Columns…", "Export CSV", "Export Text"]);
    let at = labels.iter().position(|label| *label == "Saved finds…").unwrap();
    for _ in 0..at {
        press(&mut session, &link, Key::Down);
    }
    press(&mut session, &link, Key::Enter);
    let Some(Overlay::Menu(finds)) = session.top_overlay().cloned() else {
        panic!();
    };
    let labels: Vec<&str> = finds.items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Clear the current find",
            "Open things",
            "Save the current find…",
            "Make default: Open things",
            "Delete: Open things"
        ]
    );
    for _ in 0..2 {
        press(&mut session, &link, Key::Down);
    }
    press(&mut session, &link, Key::Enter);
    typed(&mut session, &link, "Mine");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        last_transition(&link),
        Some((
            "/apps/app/Main/formSaveFind".to_string(),
            map(&[("_findDescription", "Mine"), ("formLocation", "Main.Things"), ("name", "A")])
        ))
    );
}

#[test]
fn columns_show_hide_and_save_as_the_column_tree() {
    let (mut session, link) = started();
    while session.focus_id() != "Things" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::F(10));
    press(&mut session, &link, Key::Char('c'));
    let Some(Overlay::Columns(columns)) = session.top_overlay().cloned() else {
        panic!("{:?}", session.overlays());
    };
    assert_eq!(columns.entries.len(), 3);
    // Hide Name, then show Notes: Status is column 1, Notes column 2.
    press(&mut session, &link, Key::Char(' '));
    let Some(Overlay::Columns(columns)) = session.top_overlay().cloned() else {
        panic!();
    };
    let at = columns.entries.iter().position(|entry| entry.name == "notes").unwrap();
    for _ in 0..at {
        press(&mut session, &link, Key::Down);
    }
    while !matches!(session.top_overlay(), Some(Overlay::Columns(c)) if c.entries[c.at].name == "notes") {
        press(&mut session, &link, Key::Down);
    }
    press(&mut session, &link, Key::Char(' '));
    press(&mut session, &link, Key::Enter);
    let (url, fields) = last_transition(&link).unwrap();
    assert_eq!(url, "/apps/app/Main/formSelectColumns");
    assert_eq!(fields.get("SaveColumns").map(String::as_str), Some("SaveColumns"));
    let tree: serde_json::Value = serde_json::from_str(&fields["columnsTree"]).unwrap();
    assert_eq!(tree[0]["id"], "hidden");
    assert_eq!(tree[0]["children"][0]["id"], "name");
    assert_eq!(tree[1]["children"][0]["id"], "status");
    assert_eq!(tree[2]["children"][0]["id"], "notes");
}

#[test]
fn an_export_is_a_file_to_save_or_text_to_read() {
    let (mut session, link) = started();
    link.answer_raw(
        "/apps/app/Main",
        RawBody {
            content_type: "text/csv".to_string(),
            file_name: "Things.csv".to_string(),
            bytes: b"name\nAnvil\n".to_vec(),
        },
    );
    while session.focus_id() != "Things" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::F(10));
    while !matches!(session.top_overlay(), Some(Overlay::Menu(m)) if m.items[m.at].label == "Export CSV") {
        press(&mut session, &link, Key::Down);
    }
    press(&mut session, &link, Key::Enter);
    let files = session.take_downloads();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name, "Things.csv");
    assert!(calls(&link).iter().any(|call| matches!(
        call,
        LinkCall::Raw { url, parameters } if url == "/apps/app/Main" && parameters.get("renderMode").map(String::as_str) == Some("csv")
    )));
    session.saved(Ok("/tmp/Things.csv".to_string()));
    assert!(session.message().contains("Things.csv"));
}
