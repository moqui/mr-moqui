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

//! The Revue view through its real draw pipeline: the Gallery fixtures at 72x24, 100x40 and 180x50 (and an ASCII,
//! monochrome set), the frame, the overlays, and drawing that follows the keys.
//!
//! `UPDATE_GOLDEN=1` rewrites `tests/snapshots/`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use moqui_tui::{
    gallery_link, AppView, FakeLink, Key, KeyPress, MenuItem, Role, ScreenLink, ScreenModel, Session, ShellInfo,
    Target, Theme, GALLERY_DETAIL_PATH, GALLERY_LIST_PATH,
};
use revue::layout::Rect;
use revue::render::{Buffer, Modifier};
use revue::style::Color;
use revue::widget::{RenderContext, View};

#[test]
fn section_and_dialog_titles_use_text_colors_instead_of_border_colors() {
    let model = ScreenModel::from_json(
        r#"{ "v": 1, "screen": { "path": "/titles", "title": "Titles" }, "c": [
          { "t": "box", "id": "section", "title": "Section heading", "c": [ { "t": "text", "text": "Content" } ] }
        ] }"#,
    )
    .unwrap();
    for dark in [false, true] {
        let theme = Theme::built_in(dark);
        let rgb = theme.color(Role::Text).unwrap();
        let text_color = Some(Color::rgb(rgb.0, rgb.1, rgb.2));
        let mut view = view_of(model.clone(), FakeLink::from_json(&[]).unwrap());
        view.set_theme(theme);
        for title in ["Section heading", "Keys"] {
            if title == "Keys" {
                press(&mut view, Key::F(1));
            }
            let mut buffer = Buffer::new(100, 40);
            view.render(&mut RenderContext::new(&mut buffer, Rect::new(0, 0, 100, 40)));
            let location = (0..40).find_map(|y| {
                let line: String = (0..100).map(|x| buffer.get(x, y).unwrap().symbol).collect();
                line.find(title).map(|at| (line[..at].chars().count() as u16, y))
            });
            let (x, y) = location.expect("the section or dialog title is visible");
            for offset in 0..title.len() as u16 {
                let cell = buffer.get(x + offset, y).unwrap();
                assert_eq!(cell.fg, text_color, "{title}, dark={dark}");
                assert!(cell.modifier.contains(Modifier::BOLD), "{title} is a heading");
            }
        }
    }
}

fn press(view: &mut AppView, key: Key) -> bool {
    view.on_key(&KeyPress::plain(key))
}

fn view_of(model: ScreenModel, link: FakeLink) -> AppView {
    let mut view = AppView::new(model, link);
    view.set_clock("Tue 14:32");
    view
}

fn list_view() -> AppView {
    let link = gallery_link();
    let model = link.level(GALLERY_LIST_PATH, &BTreeMap::new()).unwrap();
    view_of(model, link)
}

fn ascii() -> Theme {
    Theme::detect(&|name| (name == "NO_COLOR").then(|| "1".to_string()), true, false)
}

#[test]
fn gallery_snapshots() {
    let link = gallery_link();
    for (name, path, width, height, plain) in [
        ("list-72x24", GALLERY_LIST_PATH, 72u16, 24u16, false),
        ("list-100x40", GALLERY_LIST_PATH, 100, 40, false),
        ("list-180x50", GALLERY_LIST_PATH, 180, 50, false),
        ("detail-72x24", GALLERY_DETAIL_PATH, 72, 24, false),
        ("detail-100x40", GALLERY_DETAIL_PATH, 100, 40, false),
        ("detail-180x50", GALLERY_DETAIL_PATH, 180, 50, false),
        ("list-100x40-ascii", GALLERY_LIST_PATH, 100, 40, true),
        ("detail-180x50-ascii", GALLERY_DETAIL_PATH, 180, 50, true),
    ] {
        let mut view = view_of(link.level(path, &BTreeMap::new()).unwrap(), link.clone());
        if plain {
            view.set_theme(ascii());
        }
        let text = view.screen_text(width, height);
        if plain {
            assert!(
                !text.contains('─') && !text.contains('▾') && !text.contains('≡'),
                "{name}: {text}"
            );
        }
        golden(name, &text);
    }
}

#[test]
fn a_painted_list_keeps_unicode_pages_and_caps_a_long_description() {
    let mut view = list_view();
    let wide = view.screen_text(180, 50);
    assert!(wide.contains("東京"), "{wide}");
    assert!(wide.contains("🎉"), "{wide}");
    assert!(wide.contains("Description text that should be capped …"), "{wide}");
    assert!(wide.contains("Rows 1–10 of 20 · Page 1/2"), "{wide}");
    // Labels with spaced leaders, as in the mockups.
    assert!(wide.contains("Type . . . . : ["), "{wide}");
    assert!(wide.contains("Description  : ["), "{wide}");
    for _ in 0..3 {
        press(&mut view, Key::Tab);
    }
    press(&mut view, Key::F(8));
    let next = view.screen_text(180, 50);
    assert!(next.contains("ENUM_10"), "{next}");
    assert!(next.contains("Rows 11–20 of 20 · Page 2/2"), "{next}");
    // The list wider than 72 scrolls: the first column stays, the rest move, with the marks.
    let narrow = view.screen_text(72, 24);
    assert!(narrow.contains('▸'), "{narrow}");
    view.on_key(&KeyPress {
        key: Key::Right,
        shift: true,
        alt: false,
        ctrl: false,
    });
    let scrolled = view.screen_text(72, 24);
    assert!(scrolled.contains('◂'), "{scrolled}");
    assert!(scrolled.contains("ENUM_10"), "{scrolled}");
}

#[test]
fn numbered_list_links_and_hit_targets_stay_inside_their_split_pane() {
    let model = ScreenModel::from_json(
        r#"{"v":1,"screen":{"path":"/split","title":"Split"},"c":[
            {"t":"split","id":"Split","sideWidth":320,"c":[
                {"t":"region","id":"side","name":"side","c":[
                    {"t":"formList","id":"Links","name":"Links","rowSelection":{},
                     "columns":[{"title":"Contact"}],"rows":[
                        {"id":"a","cells":[{"t":"link","id":"a","text":"Alpha contact with a long description that must stay in its pane","url":"/a"}]},
                        {"id":"b","cells":[{"t":"link","id":"b","text":"Beta contact with a long description that must stay in its pane","url":"/b"}]}
                     ]}
                ]},
                {"t":"region","id":"main","name":"main","c":[
                    {"t":"text","text":"RIGHT_PANE_IS_PRESERVED"}
                ]}
            ]}
        ]}"#,
    )
    .unwrap();
    let mut view = view_of(model, FakeLink::from_json(&[]).unwrap());
    press(&mut view, Key::Char('#'));
    assert!(view.session().numbering());
    let mut buffer = Buffer::new(180, 40);
    view.render(&mut RenderContext::new(&mut buffer, Rect::new(0, 0, 180, 40)));
    let (main_x, main_y) = (0..40)
        .find_map(|y| {
            let line: String = (0..180).map(|x| buffer.get(x, y).unwrap().symbol).collect();
            line.find("RIGHT_PANE_IS_PRESERVED")
                .map(|at| (line[..at].chars().count() as u16, y))
        })
        .expect("the adjacent pane still paints after the side pane's clip is restored");
    let side_edge = main_x - 1;
    let side_x = side_edge - 40;
    for (offset, numbered_name) in [(2, "[1]Alpha"), (3, "[2]Beta")] {
        let y = main_y + offset;
        let inside: String = (side_x..side_edge).map(|x| buffer.get(x, y).unwrap().symbol).collect();
        assert!(inside.contains(numbered_name), "{inside}");
        assert!(matches!(view.target_at(side_x + 6, y), Some(Target::RowLink { list, .. }) if list == "Links"));
        for x in side_edge..main_x + 10 {
            let cell = buffer.get(x, y).unwrap();
            assert_eq!(cell.symbol, ' ', "list text crossed its pane at {x},{y}");
            assert!(
                !cell.modifier.contains(Modifier::UNDERLINE),
                "link underline crossed its pane at {x},{y}"
            );
            assert!(
                !matches!(view.target_at(x, y), Some(Target::RowLink { list, .. } | Target::ListRow { list, .. }) if list == "Links"),
                "list hit target crossed its pane at {x},{y}"
            );
        }
    }
}

#[test]
fn typing_shows_in_the_field_with_its_change_mark() {
    let mut view = list_view();
    press(&mut view, Key::Tab);
    for ch in "Enum".chars() {
        press(&mut view, Key::Char(ch));
    }
    let text = view.screen_text(100, 40);
    assert!(text.contains("[Enum"), "{text}");
    assert!(text.contains("]•"), "{text}");
}

const DIALOG_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/dialog", "title": "Dialog" },
  "c": [
    { "t": "dialog", "id": "New", "title": "New enumeration", "button": "New", "c": [
      { "t": "form", "id": "NewForm", "name": "NewForm", "transitionUrl": "/new", "c": [
        { "t": "field", "id": "NewForm.name", "name": "name", "title": "Name", "required": true,
          "w": { "kind": "textLine", "value": "" } },
        { "t": "field", "id": "NewForm.go", "name": "go", "title": "Create", "w": { "kind": "submit", "text": "Create" } }
      ]}
    ]}
  ]
}"#;

#[test]
fn an_open_dialog_and_a_validation_mark_are_on_the_frame() {
    let link = FakeLink::from_json(&[DIALOG_JSON]).unwrap();
    let mut view = view_of(ScreenModel::from_json(DIALOG_JSON).unwrap(), link);
    let closed = view.screen_text(100, 40);
    assert!(closed.contains("[ New… ]"), "{closed}");
    press(&mut view, Key::Enter);
    assert_eq!(view.session().dialogs_open(), ["New"]);
    assert_eq!(view.session().focus_id(), "NewForm.name");
    let dialog = view.screen_text(100, 40);
    assert!(dialog.contains("New enumeration"), "{dialog}");
    assert!(dialog.contains("* Name"), "{dialog}");
    golden("dialog-100x40", &dialog);
    press(&mut view, Key::Enter);
    assert_eq!(view.session().mark("NewForm.name"), Some("A value is required"));
    let marked = view.screen_text(100, 40);
    assert!(marked.contains("! A value is required"), "{marked}");
    assert!(marked.contains("]!"), "{marked}");
    golden("validation-100x40", &marked);
    // Esc closes the dialog; focus is back on its button.
    press(&mut view, Key::Escape);
    press(&mut view, Key::Escape);
    assert!(view.session().dialogs_open().is_empty());
    assert_eq!(view.session().focus_id(), "New");
}

fn menu() -> Vec<MenuItem> {
    vec![MenuItem {
        title: "Gallery".to_string(),
        url: "/Gallery".to_string(),
        children: vec![
            MenuItem {
                title: "List".to_string(),
                url: GALLERY_LIST_PATH.to_string(),
                children: Vec::new(),
                disabled: false,
            },
            MenuItem {
                title: "Detail".to_string(),
                url: GALLERY_DETAIL_PATH.to_string(),
                children: Vec::new(),
                disabled: false,
            },
        ],
        disabled: false,
    }]
}

#[test]
fn the_drawer_docks_when_wide_and_f9_shows_and_focuses_it() {
    let mut view = list_view();
    view.session_mut().set_menu(menu());
    let wide = view.screen_text(180, 40);
    assert!(wide.contains("▾ Gallery"), "{wide}");
    assert!(wide.contains("▶ List"), "{wide}");
    assert!(wide.contains("┬"), "{wide}");
    let narrow = view.screen_text(100, 40);
    assert!(narrow.starts_with(" ≡"), "{narrow}");
    assert!(!narrow.contains("▾ Gallery"), "{narrow}");
    press(&mut view, Key::F(9));
    let shown = view.screen_text(100, 40);
    assert!(shown.contains("▾ Gallery"), "{shown}");
    press(&mut view, Key::Down);
    press(&mut view, Key::Down);
    press(&mut view, Key::Enter);
    assert_eq!(view.session().path(), GALLERY_DETAIL_PATH);
}

#[test]
fn the_drop_down_list_opens_under_its_field() {
    let mut view = list_view();
    press(&mut view, Key::F(4));
    let text = view.screen_text(100, 40);
    assert!(text.contains("Screen Theme Type"), "{text}");
    assert!(text.contains("(none)"), "{text}");
    press(&mut view, Key::Down);
    press(&mut view, Key::Enter);
    let picked = view.screen_text(100, 40);
    assert!(picked.contains("[Screen Theme Type"), "{picked}");
}

#[test]
fn the_leave_confirmation_and_the_help_show_over_the_screen() {
    let mut view = list_view();
    press(&mut view, Key::Tab);
    press(&mut view, Key::Char('z'));
    view.on_key(&KeyPress::with_ctrl(Key::Char('q')));
    let ask = view.screen_text(100, 40);
    assert!(ask.contains("Leave without saving"), "{ask}");
    assert!(ask.contains("[ Stay ]"), "{ask}");
    press(&mut view, Key::Escape);
    // F1 lists the keymap the theme left: here back is on F2.
    let link = view.link().clone();
    link.answer_shell(ShellInfo {
        keys: BTreeMap::from([("back".to_string(), "f2".to_string())]),
        ..ShellInfo::default()
    });
    let mut themed = AppView::with_session(Session::start(GALLERY_LIST_PATH), link);
    themed.set_clock("Tue 14:32");
    themed.run_requests();
    press(&mut themed, Key::F(1));
    let help = themed.screen_text(100, 40);
    assert!(help.contains("Back             F2"), "{help}");
    assert!(!help.contains("F3"), "{help}");
    press(&mut themed, Key::Escape);
    let line = themed.screen_text(100, 40);
    assert!(line.contains("F2=Back"), "{line}");
}

#[test]
fn control_characters_in_a_value_never_reach_the_terminal() {
    let json = "{\"v\":1,\"screen\":{\"path\":\"/x\",\"title\":\"X\"},\"c\":[{\"t\":\"text\",\"id\":\"t\",\"text\":\"a\\u001b]52;c;Zm9v\\u0007b\\u202Ec\"}]}";
    let link = FakeLink::from_json(&[json]).unwrap();
    let mut view = view_of(ScreenModel::from_json(json).unwrap(), link);
    let text = view.screen_text(80, 20);
    assert!(text.contains("a]52;c;Zm9vbc"), "{text}");
    assert!(!text.contains('\u{1b}') && !text.contains('\u{202e}'), "{text}");
}

fn golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshots dir")).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}. Set UPDATE_GOLDEN=1", path.display()));
    assert_eq!(expected, actual, "{}", path.display());
}

const PHASE3_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/p3", "title": "Phase 3" },
  "c": [
    { "t": "statusBar", "id": "Status", "text": "Approved",
      "stages": [ { "statusId": "P", "label": "Placed", "state": "done" }, { "statusId": "A", "label": "Approved", "state": "current" },
                  { "statusId": "S", "label": "Shipped", "state": "todo" } ],
      "c": [ { "t": "link", "id": "Status.S", "text": "Ship", "url": "/p3/setStatus?statusId=S", "method": "post",
               "kind": "hidden-form-link", "btnType": "primary" } ] },
    { "t": "buttonMenu", "id": "More", "text": "More",
      "c": [ { "t": "link", "id": "m1", "text": "Print", "url": "/print" }, { "t": "link", "id": "m2", "text": "Copy", "url": "/copy" } ] },
    { "t": "box", "id": "Tree", "title": "Accounts",
      "c": [ { "t": "tree", "id": "Accounts", "url": "/p3/actions/Accounts" } ] },
    { "t": "box", "id": "Extra", "title": "More options", "initial": "closed",
      "c": [ { "t": "text", "id": "x", "text": "hidden until opened" } ] },
    { "t": "formList", "id": "Items", "name": "Items", "transitionUrl": "/p3/update",
      "columns": [{ "title": "Item" }, { "title": "Qty / Unit" }],
      "rows": [
        { "id": "I1", "cells": [ "Widget", [ { "t": "field", "id": "Items.qty.I1", "name": "qty", "title": "", "w": { "kind": "textLine", "value": "4" } }, "each" ] ],
          "hidden": [{ "name": "itemId", "value": "I1" }] },
        { "id": "I2", "cells": [ "Bracket", [ { "t": "field", "id": "Items.qty.I2", "name": "qty", "title": "", "w": { "kind": "textLine", "value": "12" } }, "box" ] ],
          "hidden": [{ "name": "itemId", "value": "I2" }] }
      ] },
    { "t": "component", "id": "Pipeline", "name": "board", "base": "/p3",
      "props": { "source": "boardRows", "groups": "stages", "groupOf": "stage", "fields": ["amount"] } },
    { "t": "html", "id": "Doc", "blocks": [ { "kind": "p", "text": "See the guide[1]." } ],
      "links": [ { "text": "the guide", "url": "https://docs.example/guide" } ] }
  ]
}"#;

fn phase3_view() -> AppView {
    let link = FakeLink::from_json(&[PHASE3_JSON]).unwrap();
    link.answer_json(
        "/p3/actions/Accounts",
        BTreeMap::new(),
        serde_json::json!([
            { "id": "1000", "text": "Assets", "children": [ { "id": "1100", "text": "Cash", "children": false } ] },
            { "id": "2000", "text": "Liabilities", "children": true }
        ]),
    );
    link.answer_json(
        "/p3/boardRows",
        BTreeMap::new(),
        serde_json::json!({ "stages": [{ "id": "q", "name": "Qualify", "color": "grey-6" }, { "id": "w", "name": "Won", "color": "positive" }],
            "rows": [ { "id": "O1", "title": "Acme expansion", "amount": 120000, "stage": "q" },
                      { "id": "O2", "title": "Beta pilot", "amount": 45000, "stage": "w" } ] }),
    );
    link.answer_json(
        "/shellSearch",
        BTreeMap::new(),
        serde_json::json!({ "groups": [{ "label": "Orders", "rows": [{ "title": "SO-10023", "subtitle": "Acme", "url": "/o?id=SO-10023" }] }] }),
    );
    link.answer_shell(ShellInfo {
        apps: vec![MenuItem {
            title: "Sales".to_string(),
            url: "/apps/sales".to_string(),
            children: Vec::new(),
            disabled: false,
        }],
        counts: BTreeMap::from([
            ("chat".to_string(), 1),
            ("notifications".to_string(), 2),
            ("today".to_string(), 3),
        ]),
        ..ShellInfo::default()
    });
    let mut view = AppView::with_session(Session::start("/p3"), link);
    view.set_clock("Tue 14:32");
    view.session_mut().resize(100, 40);
    view.run_requests();
    view
}

#[test]
fn phase_three_pieces_and_overlays() {
    let mut view = phase3_view();
    golden("structure-100x40", &view.screen_text(100, 40));
    view.session_mut().resize(180, 50);
    golden("structure-180x50", &view.screen_text(180, 50));
    view.session_mut().resize(100, 40);
    // F10 on the list: the row editor, then the actions menu.
    while view.session().focus_id() != "Items" {
        press(&mut view, Key::Tab);
    }
    press(&mut view, Key::Char('e'));
    golden("row-editor-100x40", &view.screen_text(100, 40));
    press(&mut view, Key::Escape);
    press(&mut view, Key::F(10));
    golden("actions-100x40", &view.screen_text(100, 40));
    press(&mut view, Key::Escape);
    view.on_key(&KeyPress::with_ctrl(Key::Char('k')));
    for ch in "sa".chars() {
        press(&mut view, Key::Char(ch));
    }
    view.session_mut()
        .on_time(std::time::Instant::now() + std::time::Duration::from_secs(1));
    view.run_requests();
    golden("palette-100x40", &view.screen_text(100, 40));
}

/// A text area longer than its rows scrolls: the cursor at the end shows the last lines with `▲` on the bar, PgUp shows
/// the lines before with `▼` below them, and a Markdown preview scrolls the same way.
#[test]
fn a_long_text_area_scrolls_with_its_cursor() {
    let body: Vec<String> = (1..=30).map(|n| format!("- line {n:02}")).collect();
    let json = format!(
        r#"{{ "v": 1, "screen": {{ "path": "/note", "title": "Note" }}, "c": [
        {{ "t": "form", "id": "Note", "name": "Note", "transitionUrl": "/note/save", "c": [
          {{ "t": "field", "id": "Note.body", "name": "body", "title": "Body",
            "w": {{ "kind": "textArea", "value": {}, "rows": 5, "editorType": "md" }} }},
          {{ "t": "field", "id": "Note.html", "name": "html", "title": "HTML body",
            "w": {{ "kind": "textArea", "value": "<p>Keep this HTML</p>", "editorType": "html", "readOnly": true }} }}
        ] }} ] }}"#,
        serde_json::to_string(&body.join("\n")).unwrap()
    );
    let link = FakeLink::from_json(&[json.as_str()]).unwrap();
    let model = link.level("/note", &BTreeMap::new()).unwrap();
    let mut view = view_of(model, link);
    assert!(view.session_mut().focus_key("Note.body"));
    // The cursor starts at the end of the text, so its last lines show.
    let text = view.screen_text(100, 30);
    assert!(
        text.contains("line 30") && text.contains("line 26") && !text.contains("line 25"),
        "{text}"
    );
    assert!(text.contains("▲"), "{text}");
    assert!(text.contains("<p>Keep this HTML</p>"), "{text}");
    assert!(
        text.contains("HTML is read-only in the terminal. Use the browser form."),
        "{text}"
    );
    assert_eq!(text.matches("HTML is read-only").count(), 1, "{text}");
    assert!(
        !view.session_mut().focus_key("Note.html"),
        "read-only HTML has no edit target"
    );
    assert_eq!(view.session().buffer("Note.html"), "<p>Keep this HTML</p>");
    press(&mut view, Key::PageUp);
    let text = view.screen_text(100, 30);
    assert!(text.contains("line 21") && !text.contains("line 26"), "{text}");
    assert!(text.contains("▼"), "{text}");
    for _ in 0..40 {
        press(&mut view, Key::Up);
    }
    let text = view.screen_text(100, 30);
    assert!(text.contains("line 01") && !text.contains("line 06"), "{text}");
    // The preview scrolls by PgDn.
    press(&mut view, Key::F(4));
    let text = view.screen_text(100, 30);
    assert!(text.contains("F4=Source") && !text.contains("line 06"), "{text}");
    press(&mut view, Key::PageDown);
    let text = view.screen_text(100, 30);
    assert!(text.contains("line 06") && text.contains("▲"), "{text}");
    let narrow = view.screen_text(40, 30);
    let words = narrow.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        words.contains("HTML is read-only in the terminal. Use the browser form."),
        "{narrow}"
    );
    assert_eq!(view.session().buffer("Note.html"), "<p>Keep this HTML</p>");
}

#[test]
fn radio_labels_wrap_without_covering_the_next_field_or_changing_keyboard_selection() {
    let json = r#"{ "v": 1, "screen": { "path": "/radio", "title": "Radio labels" }, "c": [
      { "t": "form", "id": "R", "name": "R", "c": [
        { "t": "field", "id": "R.audience", "name": "audience", "title": "Reply to",
          "w": { "kind": "radio", "value": "everyone", "options": [
            { "value": "everyone", "label": "Everyone who writes" },
            { "value": "contacts", "label": "My contacts and people inside the organization" },
            { "value": "internal", "label": "Only people inside the organization" }
          ] } },
        { "t": "field", "id": "R.after", "name": "after", "title": "Next field",
          "w": { "kind": "textLine", "value": "AFTER_RADIO" } },
        { "t": "field", "id": "R.days", "name": "days", "title": "Days",
          "w": { "kind": "check", "value": "[\"1\"]", "options": [
            { "value": "1", "label": "Monday" }, { "value": "7", "label": "Sunday" } ] } }
      ] }
    ] }"#;
    let mut beside: serde_json::Value = serde_json::from_str(json).unwrap();
    let fields = beside["c"][0]["c"].as_array_mut().unwrap();
    let audience = fields.remove(0);
    fields.insert(
        0,
        serde_json::json!({ "t": "row", "id": "R.row", "c": [
        audience,
        { "t": "field", "id": "R.neighbor", "name": "neighbor", "title": "Beside radio",
          "w": { "kind": "textLine", "value": "ADJACENT_CELL" } }
    ] }),
    );
    let beside = beside.to_string();
    for (width, alongside) in [(40, false), (100, false), (220, false), (220, true)] {
        let fixture = if alongside { beside.as_str() } else { json };
        let model = ScreenModel::from_json(fixture).unwrap();
        let mut view = view_of(model, FakeLink::from_json(&[fixture]).unwrap());
        assert!(view.session_mut().focus_key("R.audience"));
        let text = view.screen_text(width, 30);
        let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(words.contains("[x] Monday") && words.contains("[ ] Sunday"), "{text}");
        for label in [
            "Everyone who writes",
            "My contacts and people inside the organization",
            "Only people inside the organization",
        ] {
            assert!(words.contains(label), "{width}: {text}");
        }
        assert_eq!(text.matches("(•)").count(), 1, "{text}");
        let rows: Vec<&str> = text.lines().collect();
        let first = rows
            .iter()
            .position(|line| line.contains("Everyone who writes"))
            .unwrap();
        let last = rows.iter().rposition(|line| line.contains("organization")).unwrap();
        let after = rows.iter().position(|line| line.contains("AFTER_RADIO")).unwrap();
        assert!(after > last, "wrapped rows must be reserved: {text}");
        assert_eq!(
            first == last,
            width == 220 && !alongside,
            "inline when the full group fits: {text}"
        );
        if alongside {
            let neighbor = rows.iter().position(|line| line.contains("ADJACENT_CELL")).unwrap();
            assert_eq!(neighbor, first, "the adjacent field remains on the same row: {text}");
        }
        assert_eq!(
            view.session()
                .focus_order()
                .iter()
                .filter(|item| item.key == "R.audience")
                .count(),
            1
        );
        if width == 40 {
            press(&mut view, Key::Right);
            assert_eq!(view.session().buffer("R.audience"), "contacts");
            assert_eq!(view.session().focus_id(), "R.audience");
            let selected = view.screen_text(width, 30);
            assert!(selected.contains("(•) My contacts"), "{selected}");
            assert_eq!(selected.matches("(•)").count(), 1, "{selected}");
            press(&mut view, Key::Left);
            assert_eq!(view.session().buffer("R.audience"), "everyone");
            press(&mut view, Key::Tab);
            assert_eq!(view.session().focus_id(), "R.after");
        }
        if width == 100 {
            assert!(view.session_mut().focus_key("R.days"));
            press(&mut view, Key::Char(' '));
            let picker = view.screen_text(width, 30);
            assert_eq!(picker.matches("[x] Monday").count(), 2, "{picker}");
            assert!(picker.contains("Space=Toggle  Enter=Done"), "{picker}");
        }
    }
}
