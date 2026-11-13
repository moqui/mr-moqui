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

const TODAY_JSON: &str = r#"{ "v": 1, "screen": { "path": "/apps/hub/Today" }, "c": [ { "t": "text", "id": "t", "text": "Nothing due" } ] }"#;
const CHAT_JSON: &str =
    r#"{ "v": 1, "screen": { "path": "/apps/hub/Chat" }, "c": [ { "t": "text", "id": "t", "text": "No chats" } ] }"#;

fn press(session: &mut Session, link: &FakeLink, key: Key) -> bool {
    session.press(&KeyPress::plain(key), link)
}

fn typed(session: &mut Session, link: &FakeLink, text: &str) {
    for ch in text.chars() {
        press(session, link, Key::Char(ch));
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
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
                disabled: false,
                group: String::new(),
            }],
            disabled: false,
            group: String::new(),
        }],
        user: "john.doe".to_string(),
        apps: vec![
            MenuItem {
                title: "Sales".to_string(),
                url: "/qapps/sales?from=palette&note=A%20B".to_string(),
                children: Vec::new(),
                disabled: false,
                group: String::new(),
            },
            MenuItem {
                title: "Tools".to_string(),
                url: "/apps/tools".to_string(),
                children: Vec::new(),
                disabled: false,
                group: String::new(),
            },
            MenuItem {
                title: "Archive".to_string(),
                url: "/qapps-archive/tools".to_string(),
                children: Vec::new(),
                disabled: false,
                group: String::new(),
            },
            MenuItem {
                title: "Outside".to_string(),
                url: "https://example.invalid/qapps/help".to_string(),
                children: Vec::new(),
                disabled: false,
                group: String::new(),
            },
        ],
        quick_create: vec![ShellAction {
            label: "New task".to_string(),
            url: "/qapps/app/QuickTask".to_string(),
            app: "App".to_string(),
        }],
        tray: vec![
            ("Today".to_string(), "/qapps/hub/Today".to_string()),
            ("Chat".to_string(), "/apps/hub/Chat".to_string()),
        ],
        counts: BTreeMap::from([("chat".to_string(), 1), ("today".to_string(), 3)]),
        drawer_open: Some(true),
        ..ShellInfo::default()
    }
}

fn started() -> (Session, FakeLink) {
    let sales =
        r#"{ "v": 1, "screen": { "path": "/apps/sales", "pathList": ["apps", "sales"], "title": "Sales" }, "c": [] }"#;
    let link = FakeLink::from_json(&[MAIN_JSON, QUICK_JSON, TODAY_JSON, CHAT_JSON, sales]).unwrap();
    link.answer_shell(shell());
    link.answer_json(
        "/menuData/apps/app/Main",
        BTreeMap::new(),
        serde_json::json!([{ "title": "App" }, {
            "title": "Main",
            "screenDocList": [{ "title": "How to", "index": "1" }],
            "savedFinds": [{ "name": "F1", "title": "Open things", "pathWithParams": "/apps/app/Main?formListFindId=F1" }],
            "extActions": [
                { "name": "x.report", "label": "Report", "kind": "screen", "url": "/apps/app/Main/Report", "parameters": [] },
                { "name": "x.post", "label": "Post", "kind": "transition", "url": "/apps/app/Main/post", "parameters": [] },
                { "name": "x.record", "label": "Record", "kind": "screen", "url": "/apps/app/Record",
                  "parameters": [{ "name": "recordId", "from": "thingId" }] },
                { "name": "x.closed", "label": "Closed only", "kind": "screen", "url": "/apps/app/Closed", "parameters": [],
                  "condition": { "field": "status", "op": "equals", "value": "Closed" } }
            ]
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
        serde_json::json!({ "groups": [{ "label": "Orders", "rows": [{ "title": "SO-10023", "url": "/qapps/sales/Order?orderId=SO-10023" }] }] }),
    );
    session.press(&KeyPress::with_ctrl(Key::Char('k')), &link);
    let app_urls: Vec<String> = session
        .palette_rows()
        .into_iter()
        .filter(|row| row.group == "Apps")
        .map(|row| row.subtitle)
        .collect();
    assert_eq!(
        app_urls,
        [
            "/apps/sales?from=palette&note=A%20B",
            "/apps/tools",
            "/qapps-archive/tools",
            "https://example.invalid/qapps/help"
        ]
    );
    typed(&mut session, &link, "sa");
    let titles =
        |session: &Session| -> Vec<String> { session.palette_rows().into_iter().map(|row| row.title).collect() };
    assert_eq!(titles(&session), ["Sales"]);
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        session.path(),
        "/apps/sales",
        "the browser mount is never rendered as a terminal screen"
    );
    assert!(calls(&link).iter().any(|call| matches!(call,
        LinkCall::Level { path, parameters } if path == "/apps/sales" && parameters.get("note").map(String::as_str) == Some("A B")
    )));
    press(&mut session, &link, Key::F(3));
    assert_eq!(session.path(), "/apps/app/Main");
    session.press(&KeyPress::with_ctrl(Key::Char('k')), &link);
    typed(&mut session, &link, "sa");
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
    // a plain path and a Material /qapps path both go to the terminal's /apps screen
    for input in ["apps/tools", "qapps/tools"] {
        let (mut session, link) = started();
        press(&mut session, &link, Key::Char('g'));
        typed(&mut session, &link, input);
        press(&mut session, &link, Key::Enter);
        assert!(
            calls(&link)
                .iter()
                .any(|call| matches!(call, LinkCall::Level { path, .. } if path == "/apps/tools")),
            "{input}"
        );
    }
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
    link.answer_json(
        "/shellCounts",
        BTreeMap::new(),
        serde_json::json!({ "counts": { "today": 4, "chat": 0 } }),
    );
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
    let drawer: Vec<String> = session.drawer_rows().into_iter().map(|row| row.title).collect();
    assert!(
        drawer.contains(&"Saved finds".to_string()) && drawer.contains(&"Open things".to_string()),
        "{drawer:?}"
    );
    // The declared actions that open a screen are there too: not a transition, not one whose parameter or condition the
    // page does not meet (screen/ext.rs).
    let actions: Vec<(String, String)> = session
        .drawer_rows()
        .into_iter()
        .skip_while(|row| row.title != "Actions")
        .skip(1)
        .take_while(|row| row.depth == 1)
        .map(|row| (row.title, row.url))
        .collect();
    assert_eq!(actions, [("Report".to_string(), "/apps/app/Main/Report".to_string())]);
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

/// A menu item whose link is disabled (`disableLink`) is a drawer row with no url: drawn dimmed, never followed, and not
/// offered by the palette's Go to.
#[test]
fn a_disabled_menu_item_is_a_drawer_row_without_a_link() {
    let menu = vec![MenuItem {
        title: "App".to_string(),
        url: "/apps/app".to_string(),
        children: vec![
            MenuItem {
                title: "Orders".to_string(),
                url: "/apps/app/Orders".to_string(),
                ..MenuItem::default()
            },
            MenuItem {
                title: "Locked".to_string(),
                url: "/apps/app/Locked".to_string(),
                disabled: true,
                group: String::new(),
                ..MenuItem::default()
            },
        ],
        disabled: false,
        group: String::new(),
    }];
    let rows = moqui_tui::drawer_rows(&menu);
    let shown: Vec<(&str, &str, bool)> = rows
        .iter()
        .map(|row| (row.title.as_str(), row.url.as_str(), row.disabled))
        .collect();
    assert_eq!(
        shown,
        [
            ("App", "/apps/app", false),
            ("Orders", "/apps/app/Orders", false),
            ("Locked", "", true)
        ]
    );
}

/// A menu item's group (a `menu-group`) is a heading above the first of each run of items that share it; an item with
/// no group has none, and a heading is not a link.
#[test]
fn a_menu_group_is_a_heading_above_its_run_of_items() {
    let item = |title: &str, group: &str| MenuItem {
        title: title.to_string(),
        url: format!("/apps/system/{title}"),
        group: group.to_string(),
        ..MenuItem::default()
    };
    let menu = vec![MenuItem {
        title: "System".to_string(),
        url: "/apps/system".to_string(),
        children: vec![
            item("Home", ""),
            item("Caches", "Server"),
            item("Threads", "Server"),
            item("Users", "Security"),
            item("Print", ""),
            item("Jobs", "Server"),
        ],
        ..MenuItem::default()
    }];
    let rows = moqui_tui::drawer_rows(&menu);
    let shown: Vec<(&str, bool)> = rows
        .iter()
        .map(|row| (row.title.as_str(), row.url.is_empty()))
        .collect();
    assert_eq!(
        shown,
        [
            ("System", false),
            ("Home", false),
            ("Server", true),
            ("Caches", false),
            ("Threads", false),
            ("Security", true),
            ("Users", false),
            ("Print", false),
            ("Server", true),
            ("Jobs", false),
        ]
    );
    assert!(
        rows.iter().filter(|row| row.url.is_empty()).all(|row| row.depth == 1),
        "a heading at its items' level"
    );
}

/// Enter or Right on a disabled drawer row does nothing: the drawer stays open on it and nothing is loaded.
#[test]
fn enter_on_a_disabled_drawer_row_keeps_the_drawer_open() {
    let (mut session, link) = started();
    session.set_menu(vec![MenuItem {
        title: "App".to_string(),
        url: "/apps/app".to_string(),
        children: vec![
            MenuItem {
                title: "Orders".to_string(),
                url: "/apps/app/Orders".to_string(),
                ..MenuItem::default()
            },
            MenuItem {
                title: "Locked".to_string(),
                url: "/apps/app/Locked".to_string(),
                disabled: true,
                group: String::new(),
                ..MenuItem::default()
            },
        ],
        disabled: false,
        group: String::new(),
    }]);
    if session.top_overlay() != Some(&Overlay::Drawer(0)) {
        press(&mut session, &link, Key::F(9));
    }
    if session.top_overlay() != Some(&Overlay::Drawer(0)) {
        press(&mut session, &link, Key::F(9));
    }
    assert_eq!(session.top_overlay(), Some(&Overlay::Drawer(0)));
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Down);
    let before = calls(&link).len();
    for key in [Key::Enter, Key::Right] {
        let shown = format!("{key:?}");
        press(&mut session, &link, key);
        assert_eq!(session.top_overlay(), Some(&Overlay::Drawer(2)), "{shown}");
    }
    assert!(
        !calls(&link)[before..]
            .iter()
            .any(|call| matches!(call, LinkCall::Level { path, .. } if path.contains("Locked") || path.is_empty())),
        "{:?}",
        calls(&link)
    );
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
        Some(&map(&[
            ("preferenceKey", "TUI_DRAWER_OPEN"),
            ("preferenceValue", "false")
        ]))
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
    assert_eq!(
        labels,
        ["Find…", "Saved finds…", "Columns…", "Export CSV", "Export Text"]
    );
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
            map(&[
                ("_findDescription", "Mine"),
                ("formLocation", "Main.Things"),
                ("name", "A")
            ])
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

#[test]
fn a_record_header_refreshes_after_a_write_or_f5_and_stays_cached_between_tabs() {
    use moqui_tui::{Node, TransitionOutcome};

    let child = r#"{"v":1,"screen":{"path":"/apps/app/Project/Overview","pathList":["apps","app","Project","Overview"]},"c":[]}"#;
    let team =
        r#"{"v":1,"screen":{"path":"/apps/app/Project/Team","pathList":["apps","app","Project","Team"]},"c":[]}"#;
    let parent = |status: &str| {
        serde_json::json!({"v":1,"screen":{"path":"/apps/app/Project","pathList":["apps","app","Project"]},
            "c":[{"t":"text","id":"status","text":status},
                {"t":"link","id":"change","text":"Change status","url":"/apps/app/Project/change","method":"post"}]})
        .to_string()
    };
    let header = |session: &Session| match &session.nav().parents()[0].model.c[0] {
        Node::Text { text, .. } => text.clone(),
        other => panic!("expected the header's status, got {other:?}"),
    };
    let planning = parent("Planning");
    let old = FakeLink::from_json(&[child, team, &planning]).unwrap();
    let mut session = Session::start("/apps/app/Project/Overview?id=P1");
    session.run_now(&old);
    assert_eq!(header(&session), "Planning");

    let approved = parent("Approved");
    let current = FakeLink::from_json(&[child, team, &approved]).unwrap();
    session.navigate("/apps/app/Project/Team?id=P1", true);
    session.run_now(&current);
    assert_eq!(
        header(&session),
        "Planning",
        "normal tab navigation keeps the parent cache"
    );
    press(&mut session, &current, Key::F(5));
    assert_eq!(
        header(&session),
        "Approved",
        "explicit refresh reloads the record header too"
    );

    let complete = parent("Complete");
    let changed = FakeLink::from_json(&[child, team, &complete]).unwrap();
    let mut outcome = TransitionOutcome::saved("Status changed");
    outcome.screen_url = "/apps/app/Project/Team".into();
    outcome.parameters = map(&[("id", "P1")]);
    changed.answer_transition("/apps/app/Project/change", outcome);
    assert_eq!(session.focus_id(), "change");
    press(&mut session, &changed, Key::Enter);
    assert_eq!(session.path(), "/apps/app/Project/Team");
    assert_eq!(
        header(&session),
        "Complete",
        "a write refreshes the header after its redirect"
    );

    // A save that stays on the screen asks for the header once, not again through the screen's own reload.
    let header_calls = |link: &FakeLink| {
        calls(link)
            .iter()
            .filter(|call| matches!(call, LinkCall::Parent { path, .. } if path == "/apps/app/Project.tui"))
            .count()
    };
    let closed = parent("Closed");
    let stay = FakeLink::from_json(&[child, team, &closed]).unwrap();
    stay.answer_transition("/apps/app/Project/change", TransitionOutcome::saved("Status changed"));
    assert_eq!(session.focus_id(), "change");
    press(&mut session, &stay, Key::Enter);
    assert_eq!(session.path(), "/apps/app/Project/Team");
    assert_eq!(header(&session), "Closed");
    assert_eq!(header_calls(&stay), 1, "{:?}", calls(&stay));
}

#[test]
fn app_links_use_the_terminal_mount_but_explicit_browser_opening_keeps_the_material_address() {
    let main = r#"{"v":1,"screen":{"path":"/apps/app/Main","pathList":["apps","app","Main"]},"c":[
        {"t":"link","id":"inside","text":"See changes","url":"/qapps/app/Changes?id=MR1&note=A%20B"},
        {"t":"link","id":"outside","text":"External","url":"https://example.invalid/qapps/help"},
        {"t":"link","id":"print","text":"Print","url":"/apps/app/Main?renderMode=html-print&id=P1&note=A%20B"},
        {"t":"link","id":"otherMount","text":"Other mount","url":"/qapps-archive/help"},
        {"t":"formList","id":"Files","name":"Files","columns":[{"name":"file","title":"File"}],"rows":[{"key":"R1","cells":[
            {"t":"link","id":"inline","text":"Open estimate","url":"/apps/app/estimateFile?id=E1&note=A%20B","browser":true}
        ]}]}]}"#;
    let changes = r#"{"v":1,"screen":{"path":"/apps/app/Changes","pathList":["apps","app","Changes"]},"c":[]}"#;
    let link = FakeLink::from_json(&[main, changes]).unwrap();
    let mut session = Session::start("/apps/app/Main");
    session.run_now(&link);
    press(&mut session, &link, Key::Char('o'));
    assert_eq!(
        session.take_open().as_deref(),
        Some("/qapps/app/Changes?id=MR1&note=A%20B")
    );
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.path(), "/apps/app/Changes");
    assert_eq!(session.parameters().get("note").map(String::as_str), Some("A B"));
    assert_eq!(session.parameters().get("id").map(String::as_str), Some("MR1"));
    assert!(!link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Level {path, ..} if path.starts_with("/qapps/"))));
    press(&mut session, &link, Key::F(3));
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "outside");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        session.take_open().as_deref(),
        Some("https://example.invalid/qapps/help")
    );
    let before_print = link.calls().len();
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "print");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        session.take_open().as_deref(),
        Some("/apps/app/Main?renderMode=html-print&id=P1&note=A%20B")
    );
    assert_eq!(session.path(), "/apps/app/Main");
    assert_eq!(
        link.calls().len(),
        before_print,
        "print opens in the browser without asking for a TUI model"
    );
    assert!(session.focus_key("Files"));
    let before_inline = link.calls().len();
    press(&mut session, &link, Key::Right);
    assert_eq!(
        session.take_open().as_deref(),
        Some("/apps/app/estimateFile?id=E1&note=A%20B")
    );
    assert_eq!(session.path(), "/apps/app/Main");
    assert_eq!(
        link.calls().len(),
        before_inline,
        "an authored browser row link is not requested as TUI JSON"
    );
    assert!(session.focus_key("otherMount"));
    press(&mut session, &link, Key::Enter);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Level {path, ..} if path == "/qapps-archive/help")));
}

#[test]
fn f10_offers_only_the_focused_rows_actions_and_keeps_their_confirmation() {
    let rows: Vec<serde_json::Value> = ["R1", "R2"]
        .iter()
        .map(|id| {
            serde_json::json!({"id":id,"cells":[
                {"t":"link","id":format!("open-{id}"),"text":format!("Open {id}"),"url":format!("/record?id={id}")},
                {"t":"dialog","id":format!("edit-{id}"),"title":format!("Edit {id}"),"c":[]},
                {"t":"link","id":format!("approve-{id}"),"text":"Approve","url":format!("/approve?id={id}"),
                    "method":"post","writes":true,"browser":true,"confirm":format!("Approve {id}?")}
            ]})
        })
        .collect();
    let main = serde_json::json!({"v":1,"screen":{"path":"/rows"},"c":[
        {"t":"formList","id":"Rows","name":"Rows","columns":[{"title":"Record"},{"title":"Edit"},{"title":"Approve"}],"rows":rows}
    ]}).to_string();
    let link = FakeLink::from_json(&[&main]).unwrap();
    link.answer_transition("/approve", moqui_tui::TransitionOutcome::saved("Approved"));
    let mut session = Session::start("/rows");
    session.run_now(&link);
    let labels = |session: &Session| match session.top_overlay() {
        Some(Overlay::Menu(menu)) => menu.items.iter().map(|item| item.label.clone()).collect::<Vec<_>>(),
        other => panic!("expected row actions, got {other:?}"),
    };
    press(&mut session, &link, Key::F(10));
    assert_eq!(labels(&session), ["Edit R1…", "Approve"]);
    press(&mut session, &link, Key::Escape);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::F(10));
    assert_eq!(labels(&session), ["Edit R2…", "Approve"]);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Enter);
    assert!(matches!(session.top_overlay(), Some(Overlay::Confirm(confirm))
        if matches!(&confirm.kind, moqui_tui::ConfirmKind::Ask(text) if text == "Approve R2?")));
    assert_eq!(last_transition(&link), None);
    press(&mut session, &link, Key::Escape);
    assert_eq!(last_transition(&link), None);
    press(&mut session, &link, Key::F(10));
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Enter);
    press(&mut session, &link, Key::Right);
    press(&mut session, &link, Key::Enter);
    assert_eq!(last_transition(&link), Some(("/approve".into(), map(&[("id", "R2")]))));
}
