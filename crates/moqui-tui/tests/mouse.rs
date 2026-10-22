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

//! The mouse (plans/archive/Screen-TUI-Impl.md Phase 5): the hit map of a drawn frame, clicks on its targets, the wheel, a drag
//! that selects and copies, and Alt+P.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_tui::{
    gallery_link, Action, AppView, FakeLink, Key, KeyPress, LinkCall, Mouse, MouseKind, Overlay, ScreenLink,
    ScreenModel, Target, GALLERY_DETAIL_PATH, GALLERY_LIST_PATH,
};

const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

fn list_view() -> (AppView, FakeLink) {
    let link = gallery_link();
    let model = link.level(GALLERY_LIST_PATH, &BTreeMap::new()).unwrap();
    let mut view = AppView::new(model, link.clone());
    view.set_clock("Tue 14:32");
    // The screen's own first request is not one a click made.
    view.run_requests();
    (view, link)
}

/// The first cell whose target is `target` in the frame just drawn.
fn spot(view: &AppView, target: &Target) -> (u16, u16) {
    for row in 0..HEIGHT {
        for column in 0..WIDTH {
            if view.target_at(column, row).as_ref() == Some(target) {
                return (column, row);
            }
        }
    }
    panic!("{target:?} is not on the frame");
}

fn report(view: &mut AppView, kind: MouseKind, (column, row): (u16, u16), now: Instant) {
    view.on_mouse(&Mouse::at(kind, column, row), now);
}

/// Draw, click at `at`, and run what the click asked for.
fn click(view: &mut AppView, at: (u16, u16)) {
    view.screen_text(WIDTH, HEIGHT);
    let now = Instant::now();
    report(view, MouseKind::Press, at, now);
    report(view, MouseKind::Release, at, now);
    view.run_requests();
}

/// The screens asked for after the first, which the screen asked for itself.
fn levels(link: &FakeLink) -> Vec<String> {
    link.calls()
        .into_iter()
        .skip(1)
        .filter_map(|call| match call {
            LinkCall::Level { path, .. } => Some(path),
            _ => None,
        })
        .collect()
}

#[test]
fn the_hit_map_names_what_is_under_each_cell() {
    let (mut view, _) = list_view();
    let text = view.screen_text(WIDTH, HEIGHT);
    let lines: Vec<&str> = text.lines().collect();
    // "Open detail" is a link, the description a field over its label and its box, a list row its row, and the title
    // line the palette.
    let open = lines.iter().position(|line| line.contains("Open detail")).unwrap();
    let open = u16::try_from(open).unwrap();
    assert_eq!(view.target_at(2, open), Some(Target::Focus("openDetail".to_string())));
    let description = lines.iter().position(|line| line.contains("Description  :")).unwrap();
    let description = u16::try_from(description).unwrap();
    assert_eq!(
        view.target_at(2, description),
        Some(Target::Focus("description".to_string()))
    );
    assert_eq!(view.target_at(20, description), view.target_at(2, description));
    let third = lines.iter().position(|line| line.contains("ENUM_02")).unwrap();
    let third = u16::try_from(third).unwrap();
    assert_eq!(
        view.target_at(30, third),
        Some(Target::ListRow {
            list: "EnumList".to_string(),
            row: 2
        })
    );
    // The first row's identifier is its link, over the row.
    let first = u16::try_from(lines.iter().position(|line| line.contains("ENUM_00")).unwrap()).unwrap();
    assert_eq!(
        view.target_at(4, first),
        Some(Target::RowLink {
            list: "EnumList".to_string(),
            row: 0,
            link: "row0".to_string()
        })
    );
    assert_eq!(view.target_at(5, 0), Some(Target::Title));
    // The key line's F3 is Back.
    let keys = lines.len() - 1;
    let back = lines[keys].find("F3=Back").unwrap();
    assert_eq!(
        view.target_at(u16::try_from(back + 1).unwrap(), u16::try_from(keys).unwrap()),
        Some(Target::KeyLine(Action::Back))
    );
}

#[test]
fn a_click_follows_a_link_focuses_a_field_and_presses_a_key_line_entry() {
    let (mut view, link) = list_view();
    view.screen_text(WIDTH, HEIGHT);
    let field = spot(&view, &Target::Focus("description".to_string()));
    click(&mut view, field);
    assert_eq!(view.session().focus_id(), "description");
    assert!(levels(&link).is_empty(), "a field only takes the focus");
    let open = spot(&view, &Target::Focus("openDetail".to_string()));
    click(&mut view, open);
    assert_eq!(view.session().path(), GALLERY_DETAIL_PATH);
    assert_eq!(levels(&link), [GALLERY_DETAIL_PATH]);
    // F3 on the key line goes back.
    view.screen_text(WIDTH, HEIGHT);
    let back = spot(&view, &Target::KeyLine(Action::Back));
    click(&mut view, back);
    assert_eq!(view.session().path(), GALLERY_LIST_PATH);
}

#[test]
fn a_row_click_makes_it_current_and_its_link_follows() {
    let (mut view, link) = list_view();
    view.screen_text(WIDTH, HEIGHT);
    let row = spot(
        &view,
        &Target::ListRow {
            list: "EnumList".to_string(),
            row: 3,
        },
    );
    // A click on the row away from its link: the row is the current one, nothing is asked for.
    click(&mut view, (row.0 + 30, row.1));
    assert_eq!(view.session().focus_id(), "EnumList");
    assert_eq!(view.session().row_at("EnumList"), 3);
    assert!(levels(&link).is_empty());
    // The first row's identifier is its link.
    let first = spot(
        &view,
        &Target::RowLink {
            list: "EnumList".to_string(),
            row: 0,
            link: "row0".to_string(),
        },
    );
    click(&mut view, first);
    assert_eq!(view.session().row_at("EnumList"), 0);
    assert_eq!(levels(&link), [GALLERY_DETAIL_PATH]);
}

#[test]
fn a_drop_down_opens_on_a_click_and_a_choice_picks() {
    let (mut view, _) = list_view();
    view.screen_text(WIDTH, HEIGHT);
    let field = spot(&view, &Target::Focus("enumTypeId".to_string()));
    click(&mut view, field);
    assert!(matches!(view.session().top_overlay(), Some(Overlay::DropDown(_))));
    let choices = view.session().dropdown_choices();
    view.screen_text(WIDTH, HEIGHT);
    let second = spot(&view, &Target::Choice(1));
    // A click on the screen under the list does nothing while it is open.
    click(&mut view, (field.0, HEIGHT - 4));
    assert!(matches!(view.session().top_overlay(), Some(Overlay::DropDown(_))));
    click(&mut view, second);
    assert!(view.session().top_overlay().is_none());
    assert_eq!(view.session().buffer("enumTypeId"), choices[1].value);
}

const DIALOG_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/dialog", "title": "Dialog" },
  "c": [
    { "t": "dialog", "id": "New", "title": "New enumeration", "button": "New", "c": [
      { "t": "form", "id": "NewForm", "name": "NewForm", "transitionUrl": "/new", "c": [
        { "t": "field", "id": "NewForm.name", "name": "name", "title": "Name", "w": { "kind": "textLine", "value": "" } },
        { "t": "field", "id": "NewForm.code", "name": "code", "title": "Code", "w": { "kind": "textLine", "value": "" } },
        { "t": "field", "id": "NewForm.go", "name": "go", "title": "Create", "w": { "kind": "submit", "text": "Create" } }
      ]}
    ]},
    { "t": "link", "id": "away", "text": "Elsewhere", "url": "/elsewhere" }
  ]
}"#;

#[test]
fn a_dialog_takes_its_clicks_and_one_outside_does_nothing() {
    let link = FakeLink::from_json(&[DIALOG_JSON]).unwrap();
    let mut view = AppView::new(ScreenModel::from_json(DIALOG_JSON).unwrap(), link.clone());
    view.screen_text(WIDTH, HEIGHT);
    let button = spot(&view, &Target::Focus("New".to_string()));
    let away = spot(&view, &Target::Focus("away".to_string()));
    click(&mut view, button);
    assert_eq!(view.session().dialogs_open(), ["New"]);
    view.screen_text(WIDTH, HEIGHT);
    let code = spot(&view, &Target::Focus("NewForm.code".to_string()));
    click(&mut view, code);
    assert_eq!(view.session().focus_id(), "NewForm.code");
    // The link behind the dialog: under it, or beside it, a click does not leave the dialog or lose its edits.
    click(&mut view, away);
    assert_eq!(view.session().dialogs_open(), ["New"]);
    assert!(levels(&link).iter().all(|path| path != "/elsewhere"));
    // Its button submits.
    let create = spot(&view, &Target::Focus("NewForm.go".to_string()));
    click(&mut view, create);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Transition { url, .. } if url == "/new")));
}

#[test]
fn the_wheel_scrolls_the_canvas_until_a_key() {
    let (mut view, _) = list_view();
    let first = view.screen_text(WIDTH, 16);
    assert!(first.contains("Type . ."), "{first}");
    let now = Instant::now();
    view.on_mouse(&Mouse::at(MouseKind::WheelDown, 30, 8), now);
    view.on_mouse(&Mouse::at(MouseKind::WheelDown, 30, 8), now);
    let scrolled = view.screen_text(WIDTH, 16);
    assert!(!scrolled.contains("Type . ."), "{scrolled}");
    view.on_mouse(&Mouse::at(MouseKind::WheelUp, 30, 8), now);
    view.on_mouse(&Mouse::at(MouseKind::WheelUp, 30, 8), now);
    assert!(view.screen_text(WIDTH, 16).contains("Type . ."));
    view.on_mouse(&Mouse::at(MouseKind::WheelDown, 30, 8), now);
    view.on_mouse(&Mouse::at(MouseKind::WheelDown, 30, 8), now);
    assert!(!view.screen_text(WIDTH, 16).contains("Type . ."));
    // A key brings the focus, at the top of the screen, back into view.
    view.on_key(&KeyPress::plain(Key::Home));
    assert!(view.screen_text(WIDTH, 16).contains("Type . ."));
}

#[test]
fn a_drag_selects_rows_and_copies_them_once() {
    let (mut view, _) = list_view();
    let text = view.screen_text(WIDTH, HEIGHT);
    let lines: Vec<&str> = text.lines().collect();
    let first = u16::try_from(lines.iter().position(|line| line.contains("ENUM_00")).unwrap()).unwrap();
    let now = Instant::now();
    report(&mut view, MouseKind::Press, (3, first), now);
    report(&mut view, MouseKind::Drag, (10, first + 1), now);
    report(&mut view, MouseKind::Drag, (20, first + 2), now);
    assert!(view.selection().is_some());
    report(&mut view, MouseKind::Release, (20, first + 2), now);
    let copied = view.session_mut().take_copy().expect("the selection is copied");
    let expected = [
        lines[usize::from(first)][3..].trim_end().to_string(),
        lines[usize::from(first) + 1].trim_end().to_string(),
        lines[usize::from(first) + 2][..21].trim_end().to_string(),
    ]
    .join("\n");
    assert_eq!(copied, expected);
    assert!(
        view.session().message().starts_with("Copied "),
        "{}",
        view.session().message()
    );
    assert!(
        view.session().message().contains("Shift+drag"),
        "the first copy says how to fall back"
    );
    assert!(view.session_mut().take_copy().is_none(), "one copy per drag");
    // The selection shows in reverse video until a key.
    view.screen_text(WIDTH, HEIGHT);
    view.on_key(&KeyPress::plain(Key::Escape));
    assert!(view.selection().is_none());
    // Alt makes it a rectangle: the identifier column of three rows.
    report(&mut view, MouseKind::Press, (3, first), now);
    let mut rect = Mouse::at(MouseKind::Drag, 9, first + 2);
    rect.alt = true;
    view.on_mouse(&rect, now);
    report(&mut view, MouseKind::Release, (9, first + 2), now);
    assert_eq!(
        view.session_mut().take_copy().as_deref(),
        Some("ENUM_00\nENUM_01\nENUM_02")
    );
}

#[test]
fn a_double_click_on_plain_text_copies_the_word() {
    let (mut view, _) = list_view();
    let text = view.screen_text(WIDTH, HEIGHT);
    let lines: Vec<&str> = text.lines().collect();
    let footer = lines.iter().position(|line| line.contains("Rows 1")).unwrap();
    let line: Vec<char> = lines[footer].chars().collect();
    let column = u16::try_from(
        line.windows(4)
            .position(|w| w.iter().collect::<String>() == "Page")
            .unwrap(),
    )
    .unwrap();
    let at = (column + 1, u16::try_from(footer).unwrap());
    // The footer is no target in its own right: the list's last row is not under it.
    assert!(view.target_at(at.0, at.1).is_none(), "{:?}", view.target_at(at.0, at.1));
    let now = Instant::now();
    for gap in [0, 100] {
        report(&mut view, MouseKind::Press, at, now + Duration::from_millis(gap));
        report(&mut view, MouseKind::Release, at, now + Duration::from_millis(gap));
    }
    assert_eq!(view.session_mut().take_copy().as_deref(), Some("Page"));
}

#[test]
fn alt_p_turns_the_mouse_off_and_keeps_the_choice() {
    let (mut view, link) = list_view();
    assert_eq!(view.session_mut().take_mouse_mode(), Some(true), "on until chosen");
    assert_eq!(view.session_mut().take_mouse_mode(), None, "told once");
    view.on_key(&KeyPress::with_alt(Key::Char('p')));
    assert_eq!(view.session_mut().take_mouse_mode(), Some(false));
    assert!(link.calls().iter().any(|call| matches!(
        call,
        LinkCall::Json { url, parameters, .. }
            if url == "/apps/setPreference" && parameters.get("preferenceKey").map(String::as_str) == Some("TUI_MOUSE")
    )));
    // Off: reports change nothing.
    view.screen_text(WIDTH, HEIGHT);
    let open = spot(&view, &Target::Focus("openDetail".to_string()));
    click(&mut view, open);
    assert_eq!(view.session().path(), GALLERY_LIST_PATH);
}
