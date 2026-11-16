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

//! The session by keys, with its requests run on this thread: key merge, levels, forms, lists, drop-downs, dates, the
//! login and the request lanes. No Revue.

use std::collections::BTreeMap;

use moqui_tui::{
    gallery_link, merge_keys, scheme_keys, Action, AdminSetup, Choice, FakeLink, Key, KeyMap, KeyPress, KeyScheme,
    LinkCall, Login, Messages, Overlay, Reply, ScreenLink, ScreenModel, Sendable, Session, ShellInfo,
    TransitionOutcome, ValidationError, Widget, GALLERY_DETAIL_PATH, GALLERY_LIST_JSON, GALLERY_LIST_PATH,
};

const EDIT_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/edit", "title": "Edit" },
  "c": [
    {
      "t": "form",
      "id": "Edit",
      "name": "Edit",
      "transitionUrl": "/edit/update",
      "submitMessage": "Kept",
      "hidden": [{ "name": "partyId", "value": "P1" }],
      "c": [
        {
          "t": "field",
          "id": "Edit.name",
          "name": "name",
          "title": "Name",
          "required": true,
          "maxlength": 4,
          "w": { "kind": "textLine", "value": "" }
        },
        {
          "t": "field",
          "id": "Edit.password",
          "name": "password",
          "title": "Password",
          "w": { "kind": "password" }
        },
        {
          "t": "field",
          "id": "Edit.note",
          "name": "note",
          "title": "Note",
          "w": { "kind": "textLine", "value": "keep" }
        },
        {
          "t": "field",
          "id": "Edit.save",
          "name": "save",
          "title": "Save",
          "w": { "kind": "submit", "text": "Save" }
        }
      ]
    },
    {
      "t": "form",
      "id": "Other",
      "name": "Other",
      "transitionUrl": "/other",
      "c": [
        {
          "t": "field",
          "id": "Other.extra",
          "name": "extra",
          "title": "Extra",
          "w": { "kind": "textLine", "value": "" }
        }
      ]
    }
  ]
}"#;

const LEAVE_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/leave", "title": "Leave" },
  "c": [
    {
      "t": "form",
      "id": "Edit",
      "name": "Edit",
      "transitionUrl": "/leave/update",
      "c": [
        {
          "t": "field",
          "id": "Edit.note",
          "name": "note",
          "title": "Note",
          "w": { "kind": "textLine", "value": "" }
        }
      ]
    },
    { "t": "link", "id": "away", "text": "Away", "url": "/away" }
  ]
}"#;

const AWAY_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/away", "title": "Away" },
  "c": []
}"#;

const GEO_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/geo", "title": "Geo" },
  "c": [
    {
      "t": "form",
      "id": "Geo",
      "name": "Geo",
      "c": [
        {
          "t": "field",
          "id": "countryGeoId",
          "name": "countryGeoId",
          "title": "Country",
          "w": {
            "kind": "dropDown",
            "value": "USA",
            "options": [
              { "value": "USA", "label": "USA" },
              { "value": "CAN", "label": "Canada" }
            ]
          }
        },
        {
          "t": "field",
          "id": "state",
          "name": "stateProvinceGeoId",
          "title": "State",
          "w": {
            "kind": "dropDown",
            "value": "CA",
            "options": [{ "value": "CA", "label": "California" }],
            "optionsUrl": "/states",
            "dependsOn": { "countryGeoId": "countryGeoId" }
          }
        }
      ]
    }
  ]
}"#;

fn press(session: &mut Session, link: &FakeLink, key: Key) -> bool {
    session.press(&KeyPress::plain(key), link)
}

fn typed(session: &mut Session, link: &FakeLink, text: &str) {
    for ch in text.chars() {
        press(session, link, Key::Char(ch));
    }
}

fn edit_session() -> (Session, FakeLink) {
    let model = ScreenModel::from_json(EDIT_JSON).unwrap();
    let link = FakeLink::from_json(&[EDIT_JSON]).unwrap();
    (Session::new(model), link)
}

fn transitions(link: &FakeLink) -> Vec<(String, BTreeMap<String, String>)> {
    link.calls()
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Transition { url, fields, .. } => Some((url, fields)),
            _ => None,
        })
        .collect()
}

fn levels(link: &FakeLink) -> Vec<(String, BTreeMap<String, String>)> {
    link.calls()
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Level { path, parameters } | LinkCall::Parent { path, parameters, .. } => {
                Some((path, parameters))
            }
            _ => None,
        })
        .collect()
}

fn list_session() -> (Session, FakeLink) {
    let link = gallery_link();
    let session = Session::new(link.level(GALLERY_LIST_PATH, &BTreeMap::new()).unwrap());
    (session, link)
}

#[test]
fn a_second_submit_while_one_runs_is_not_sent() {
    let (mut session, _link) = edit_session();
    session.set_buffer("Edit.name", "Ann".to_string());
    session.on_key(&KeyPress::plain(Key::Enter));
    session.on_key(&KeyPress::plain(Key::Enter));
    let sent = session.take_requests();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(session.is_busy());
}

#[test]
fn merge_keys_skips_a_bad_chord_an_unknown_action_and_a_fixed_key_and_keeps_esc() {
    let (map, skipped) = merge_keys(
        &KeyMap::classic(),
        &[
            ("fly", "f2"),
            ("refresh", "not a chord"),
            ("refresh", "esc"),
            ("back", "f2"),
            ("cancel", "f11"),
        ],
    );
    assert_eq!(skipped[0].reason, "unknown action");
    assert_eq!(skipped[1].reason, "bad chord");
    assert_eq!(skipped[1].chord, "not a chord");
    assert_eq!(skipped[2].reason, "fixed key");
    assert_eq!(map.action(&KeyPress::plain(Key::F(5))), Some(Action::Refresh));
    assert_eq!(map.action(&KeyPress::plain(Key::F(2))), Some(Action::Back));
    assert_eq!(map.action(&KeyPress::plain(Key::F(3))), None);
    // A theme that moves cancel to F11 still leaves Esc on it, so no dialog traps the user.
    assert_eq!(map.action(&KeyPress::plain(Key::F(11))), Some(Action::Cancel));
    assert_eq!(map.action(&KeyPress::plain(Key::Escape)), Some(Action::Cancel));
    assert_eq!(map.action(&KeyPress::plain(Key::F(12))), None);
    assert_eq!(map.action(&KeyPress::plain(Key::Enter)), Some(Action::Activate));
    assert_eq!(map.action(&KeyPress::plain(Key::Tab)), Some(Action::Next));
    assert_eq!(map.hint(Action::Back).as_deref(), Some("F2=Back"));
}

#[test]
fn the_shell_data_brings_the_menu_and_the_theme_keys() {
    let (mut session, link) = list_session();
    link.answer_shell(ShellInfo {
        user: "john.doe".to_string(),
        keys: BTreeMap::from([
            ("back".to_string(), "f2".to_string()),
            ("bogus".to_string(), "f6".to_string()),
        ]),
        ..ShellInfo::default()
    });
    let mut started = Session::start(GALLERY_LIST_PATH);
    started.run_now(&link);
    assert_eq!(started.shell().user, "john.doe");
    assert_eq!(started.keys().action(&KeyPress::plain(Key::F(2))), Some(Action::Back));
    assert_eq!(started.key_skips().len(), 1);
    assert_eq!(started.path(), GALLERY_LIST_PATH);
    let _ = press(&mut session, &link, Key::Tab);
}

#[test]
fn the_schemes_bind_the_arrows_back_and_f3_each_its_own_way() {
    let modern = KeyMap::modern();
    let classic = KeyMap::classic();
    let plain = KeyPress::plain;
    let shift_f3 = KeyPress {
        shift: true,
        ..plain(Key::F(3))
    };
    // Modern: the arrows and j/k move, Enter alone follows, Backspace goes back, F3 finds the next.
    assert_eq!(modern.action(&plain(Key::Right)), Some(Action::Next));
    assert_eq!(modern.action(&plain(Key::Left)), Some(Action::Prev));
    assert_eq!(modern.action(&plain(Key::Char('j'))), Some(Action::Next));
    assert_eq!(modern.action(&plain(Key::Char('k'))), Some(Action::Prev));
    assert_eq!(modern.action(&plain(Key::Enter)), Some(Action::Activate));
    assert_eq!(modern.action(&plain(Key::Backspace)), Some(Action::Back));
    assert_eq!(modern.action(&plain(Key::F(3))), Some(Action::FindNext));
    assert_eq!(modern.action(&shift_f3), Some(Action::FindPrev));
    assert_eq!(
        modern.action(&KeyPress::with_ctrl(Key::Char('a'))),
        Some(Action::SelectAll)
    );
    assert_eq!(
        modern.action(&KeyPress::with_ctrl(Key::Char('p'))),
        Some(Action::Palette)
    );
    assert_eq!(modern.action(&plain(Key::F(12))), None);
    assert_eq!(modern.hint(Action::Back).as_deref(), Some("Backspace=Back"));
    // Classic: Right follows, Left and F3 go back, F12 cancels, and no Ctrl+A.
    assert_eq!(classic.action(&plain(Key::Right)), Some(Action::Activate));
    assert_eq!(classic.action(&plain(Key::Left)), Some(Action::Back));
    assert_eq!(classic.action(&plain(Key::F(3))), Some(Action::Back));
    assert_eq!(classic.action(&plain(Key::F(12))), Some(Action::Cancel));
    assert_eq!(classic.action(&KeyPress::with_ctrl(Key::Char('a'))), None);
    assert_eq!(classic.hint(Action::Back).as_deref(), Some("F3=Back"));
    // Both: Ctrl+C copies, Home and End, Ctrl+PgUp and Ctrl+PgDn change the tab.
    for map in [&modern, &classic] {
        assert_eq!(map.action(&KeyPress::with_ctrl(Key::Char('c'))), Some(Action::Copy));
        assert_eq!(map.action(&plain(Key::Home)), Some(Action::First));
        assert_eq!(map.action(&plain(Key::End)), Some(Action::Last));
        assert_eq!(map.action(&KeyPress::with_ctrl(Key::PageDown)), Some(Action::TabNext));
        assert_eq!(map.action(&KeyPress::with_ctrl(Key::PageUp)), Some(Action::TabPrev));
    }
}

#[test]
fn a_theme_row_for_one_scheme_applies_to_that_scheme_only_and_wins() {
    let rows = [
        ("classic.back", "f3 | f11"),
        ("back", "f2"),
        ("modern.find-next", "f7"),
        ("vi.back", "h"),
    ];
    let (modern, skipped) = scheme_keys(KeyScheme::Modern, &rows);
    assert_eq!(modern.action(&KeyPress::plain(Key::F(2))), Some(Action::Back));
    assert_eq!(modern.action(&KeyPress::plain(Key::F(7))), Some(Action::FindNext));
    assert_eq!(modern.action(&KeyPress::plain(Key::F(3))), None);
    assert_eq!(modern.action(&KeyPress::plain(Key::F(11))), None);
    // vi is no scheme, so its row is an unknown action, logged as any other.
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].action, "vi.back");
    let (classic, _) = scheme_keys(KeyScheme::Classic, &rows);
    assert_eq!(classic.action(&KeyPress::plain(Key::F(11))), Some(Action::Back));
    assert_eq!(classic.action(&KeyPress::plain(Key::F(3))), Some(Action::Back));
    assert_eq!(classic.action(&KeyPress::plain(Key::F(2))), None);
    assert_eq!(classic.action(&KeyPress::plain(Key::F(7))), Some(Action::PagePrev));
    assert_eq!(KeyScheme::parse(" Classic "), Some(KeyScheme::Classic));
    assert_eq!(KeyScheme::parse("vi"), None);
}

#[test]
fn the_shell_data_and_an_override_pick_the_scheme() {
    let link = gallery_link();
    link.answer_shell(ShellInfo {
        key_scheme: Some(KeyScheme::Classic),
        ..ShellInfo::default()
    });
    let mut session = Session::start(GALLERY_LIST_PATH);
    assert_eq!(session.key_scheme(), KeyScheme::Modern, "Modern until the shell says");
    session.run_now(&link);
    assert_eq!(session.key_scheme(), KeyScheme::Classic);
    assert_eq!(session.keys().action(&KeyPress::plain(Key::F(3))), Some(Action::Back));
    session.set_key_scheme_override(Some(KeyScheme::Modern));
    assert_eq!(session.key_scheme(), KeyScheme::Modern);
    assert_eq!(
        session.keys().action(&KeyPress::plain(Key::F(3))),
        Some(Action::FindNext)
    );
    session.set_key_scheme_override(None);
    assert_eq!(session.key_scheme(), KeyScheme::Classic);
}

#[test]
fn modern_esc_closes_what_is_open_then_opens_the_palette() {
    let (mut session, link) = list_session();
    assert_eq!(session.key_scheme(), KeyScheme::Modern);
    press(&mut session, &link, Key::Char('/'));
    assert!(matches!(session.top_overlay(), Some(Overlay::Search(_))));
    press(&mut session, &link, Key::Escape);
    assert!(session.top_overlay().is_none());
    press(&mut session, &link, Key::Escape);
    assert!(matches!(session.top_overlay(), Some(Overlay::Palette(_))));
    press(&mut session, &link, Key::Escape);
    assert!(session.top_overlay().is_none());
    // Classic's Esc with nothing open opens nothing.
    session.set_key_scheme_override(Some(KeyScheme::Classic));
    press(&mut session, &link, Key::Escape);
    assert!(session.top_overlay().is_none());
}

#[test]
fn the_palette_keys_row_switches_the_scheme_and_keeps_it() {
    let (mut session, link) = list_session();
    link.answer_json("/apps/setPreference", BTreeMap::new(), serde_json::json!({}));
    session.press(&KeyPress::with_ctrl(Key::Char('k')), &link);
    typed(&mut session, &link, "keys");
    let titles: Vec<String> = session.palette_rows().into_iter().map(|row| row.title).collect();
    assert_eq!(titles, ["Keys: switch to Classic (green screen)"]);
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.key_scheme(), KeyScheme::Classic);
    assert!(session.message().starts_with("Keys: Classic"), "{}", session.message());
    assert!(link.calls().iter().any(|call| matches!(
        call,
        LinkCall::Json { url, parameters, .. } if url == "/apps/setPreference"
            && parameters.get("preferenceKey").map(String::as_str) == Some("TUI_KEYS")
            && parameters.get("preferenceValue").map(String::as_str) == Some("classic")
    )));
    // and back: the row offers the other scheme
    session.press(&KeyPress::with_ctrl(Key::Char('k')), &link);
    typed(&mut session, &link, "keys");
    let titles: Vec<String> = session.palette_rows().into_iter().map(|row| row.title).collect();
    assert_eq!(titles, ["Keys: switch to Modern"]);
}

#[test]
fn ctrl_c_off_a_text_field_copies_the_row_and_never_quits() {
    let (mut session, link) = list_session();
    assert!(session.focus_key("EnumList"));
    assert!(!session.press(&KeyPress::with_ctrl(Key::Char('c')), &link));
    let copied = session.take_copy().unwrap_or_default();
    assert!(copied.starts_with("ENUM_00\t"), "{copied:?}");
    assert!(!session.quitting());
}

#[test]
fn modern_arrows_move_the_focus_and_home_and_end_go_to_a_lists_ends() {
    let (mut session, link) = list_session();
    assert!(session.focus_key("EnumList"));
    press(&mut session, &link, Key::End);
    assert_eq!(session.row_at("EnumList"), 9);
    press(&mut session, &link, Key::Home);
    assert_eq!(session.row_at("EnumList"), 0);
    // j and k move through the rows, as Down and Up do.
    press(&mut session, &link, Key::Char('j'));
    press(&mut session, &link, Key::Char('j'));
    press(&mut session, &link, Key::Char('k'));
    assert_eq!(session.row_at("EnumList"), 1);
    press(&mut session, &link, Key::Char('k'));
    assert_eq!(session.row_at("EnumList"), 0);
    press(&mut session, &link, Key::Right);
    let after = session.focus_id().to_string();
    assert_ne!(after, "EnumList", "Right moves on, it does not follow the row");
    assert_eq!(session.path(), GALLERY_LIST_PATH);
    press(&mut session, &link, Key::Left);
    assert_eq!(session.focus_id(), "EnumList");
    // Enter follows the row; Backspace off a text field goes back.
    press(&mut session, &link, Key::Enter);
    assert_ne!(session.path(), GALLERY_LIST_PATH);
    for _ in 0..20 {
        if !session.focused_is_text() {
            break;
        }
        press(&mut session, &link, Key::Tab);
    }
    assert!(!session.focused_is_text());
    press(&mut session, &link, Key::Backspace);
    assert_eq!(session.path(), GALLERY_LIST_PATH);
}

#[test]
fn a_field_selects_copies_pastes_and_undoes_and_esc_drops_the_selection_first() {
    let (mut session, link) = edit_session();
    assert!(session.focus_key("Edit.note"));
    let ctrl = |session: &mut Session, ch| session.press(&KeyPress::with_ctrl(Key::Char(ch)), &link);
    ctrl(&mut session, 'a');
    typed(&mut session, &link, "Alice");
    ctrl(&mut session, 'a');
    assert_eq!(session.selection("Edit.note"), Some((0, 5)));
    assert!(!ctrl(&mut session, 'c'), "Ctrl+C copies and does not quit");
    assert_eq!(session.take_copy().as_deref(), Some("Alice"));
    // Esc drops the selection, and only then is the screen's: in Modern, the palette.
    press(&mut session, &link, Key::Escape);
    assert_eq!(session.selection("Edit.note"), None);
    assert!(session.top_overlay().is_none());
    press(&mut session, &link, Key::End);
    ctrl(&mut session, 'v');
    assert_eq!(session.buffer("Edit.note"), "AliceAlice");
    ctrl(&mut session, 'z');
    assert_eq!(session.buffer("Edit.note"), "Alice");
    // A paste from the terminal takes the place of the selection.
    session.press(
        &KeyPress {
            shift: true,
            ..KeyPress::plain(Key::Home)
        },
        &link,
    );
    session.paste("Bob");
    assert_eq!(session.buffer("Edit.note"), "Bob");
    ctrl(&mut session, 'z');
    assert_eq!(session.buffer("Edit.note"), "Alice");
    // the undo brings the selection back as it was
    assert_eq!(session.selection("Edit.note"), Some((0, 5)));
    // Classic leaves Ctrl+A alone.
    session.set_key_scheme_override(Some(KeyScheme::Classic));
    press(&mut session, &link, Key::End);
    ctrl(&mut session, 'a');
    assert_eq!(session.selection("Edit.note"), None);
}

const PARTY_FIND_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/Find", "pathList": ["apps", "app", "Find"], "title": "Find" },
  "c": [{ "t": "link", "id": "edit", "text": "Edit", "url": "/apps/app/Party/EditParty?partyId=P1" }]
}"#;

const PARTY_HEADER_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/Party", "pathList": ["apps", "app", "Party"], "title": "Party" },
  "c": [
    { "t": "text", "id": "head", "text": "Party P1" },
    { "t": "subscreens", "id": "tabs", "kind": "tab", "items": [
      { "title": "Edit", "url": "/apps/app/Party/EditParty?partyId=P1", "active": true },
      { "title": "Roles", "url": "/apps/app/Party/Roles?partyId=P1" }
    ], "c": [] }
  ]
}"#;

const PARTY_EDIT_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/Party/EditParty", "pathList": ["apps", "app", "Party", "EditParty"], "title": "Edit" },
  "c": [{ "t": "text", "id": "body", "text": "edit body" }]
}"#;

const PARTY_ROLES_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/Party/Roles", "pathList": ["apps", "app", "Party", "Roles"], "title": "Roles" },
  "c": [{ "t": "text", "id": "roles", "text": "roles body" }]
}"#;

#[test]
fn a_record_header_level_loads_once_and_stays_for_its_tabs() {
    let link = FakeLink::from_json(&[PARTY_FIND_JSON, PARTY_HEADER_JSON, PARTY_EDIT_JSON, PARTY_ROLES_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(PARTY_FIND_JSON).unwrap());
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.paths(), ["/apps/app/Party", "/apps/app/Party/EditParty"]);
    assert_eq!(session.parameters().get("partyId").map(String::as_str), Some("P1"));
    let asked: Vec<String> = levels(&link).into_iter().map(|(path, _)| path).collect();
    assert_eq!(asked, ["/apps/app/Party/EditParty", "/apps/app/Party.tui"]);
    // the header is a parent level, asked for naming the page it is loaded for (a level the user may not open alone
    // answers as that page's frame)
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Parent { path, page, .. }
        if path == "/apps/app/Party.tui" && page == "/apps/app/Party/EditParty")));
    // Alt+2 is the second tab of the header; the header stays and is not asked for again.
    session.press(&KeyPress::with_alt(Key::Char('2')), &link);
    assert_eq!(session.paths(), ["/apps/app/Party", "/apps/app/Party/Roles"]);
    assert_eq!(levels(&link).len(), 3);
    // Back (Alt+Left in either scheme) goes to the screen before, asking for it again.
    session.press(&KeyPress::with_alt(Key::Left), &link);
    assert_eq!(session.path(), "/apps/app/Party/EditParty");
}

const LINKS_HEADER_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/apps/app/Party", "pathList": ["apps", "app", "Party"], "title": "Party" },
  "c": [ { "t": "link", "id": "first", "text": "First", "url": "/apps/app/Party/EditParty?partyId=P1" },
         { "t": "link", "id": "second", "text": "Second", "url": "/apps/app/Party/EditParty?partyId=P2" } ]
}"#;

/// A navigate to the same page with other parameters asks for its parent level again, and the page alone has nothing to
/// focus until it comes: the focus comes back to where it was (the Workspace's tree went to its first field when a
/// file opened).
#[test]
fn the_focus_stays_when_a_page_comes_before_the_level_that_holds_it() {
    let link = FakeLink::from_json(&[PARTY_FIND_JSON, LINKS_HEADER_JSON, PARTY_EDIT_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(PARTY_FIND_JSON).unwrap());
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.paths(), ["/apps/app/Party", "/apps/app/Party/EditParty"]);
    assert!(session.focus_key("second"), "{:?}", session.focus_order());
    session.navigate("/apps/app/Party/EditParty?partyId=P2", true);
    session.run_now(&link);
    assert_eq!(session.parameters().get("partyId").map(String::as_str), Some("P2"));
    assert_eq!(session.focus_id(), "second");
}

#[test]
fn gallery_list_to_detail_replaces_the_only_level() {
    let (mut session, link) = list_session();
    for _ in 0..4 {
        press(&mut session, &link, Key::Tab);
    }
    assert_eq!(session.focus_id(), "openDetail");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.paths(), vec![GALLERY_DETAIL_PATH]);
}

#[test]
fn an_empty_required_field_is_marked_and_not_sent() {
    let (mut session, link) = edit_session();
    assert_eq!(session.focus_id(), "Edit.name");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.mark("Edit.name"), Some("A value is required"));
    assert_eq!(session.message(), "A value is required");
    assert!(link.calls().is_empty());
    // Typing stops at maxlength; a longer value set from outside is caught at submit.
    typed(&mut session, &link, "abcdef");
    assert_eq!(session.buffer("Edit.name"), "abcd");
    session.set_buffer("Edit.name", "abcde".to_string());
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.mark("Edit.name"), Some("At most 4 characters"));
    assert!(link.calls().is_empty());
}

#[test]
fn a_valid_submit_posts_the_password_clears_that_form_and_keeps_another_forms_edit() {
    let (mut session, link) = edit_session();
    link.answer_transition("/edit/update", TransitionOutcome::saved("Saved"));
    session.set_buffer("Edit.name", "Ann".to_string());
    session.set_buffer("Edit.password", "s3cret".to_string());
    session.set_buffer("Other.extra", "x".to_string());
    press(&mut session, &link, Key::Enter);

    let posted = transitions(&link);
    let (url, fields) = &posted[0];
    assert_eq!(url, "/edit/update");
    assert_eq!(fields.get("name").map(String::as_str), Some("Ann"));
    assert_eq!(fields.get("password").map(String::as_str), Some("s3cret"));
    assert_eq!(fields.get("note").map(String::as_str), Some("keep"));
    assert_eq!(fields.get("partyId").map(String::as_str), Some("P1"));
    assert!(!fields.contains_key("save"));
    assert!(!fields.contains_key("extra"));
    // Staying on the screen reloads it, so the form shows what the server saved.
    assert!(matches!(link.calls().last(), Some(LinkCall::Level { path, .. }) if path == "/edit"));
    assert!(!session.is_dirty("Edit.name"));
    assert_eq!(session.buffer("Edit.name"), "");
    assert!(session.is_dirty("Other.extra"));
    assert_eq!(session.buffer("Other.extra"), "x");
    assert_eq!(session.message(), "Saved");
}

/// Every message a transition sends is on the line, as its errors are: a summary's details follow it (ACC-UI-024, the
/// 1099 run's count and then each vendor that failed).
#[test]
fn a_transition_that_sends_several_messages_shows_them_all() {
    let (mut session, link) = edit_session();
    link.answer_transition(
        "/edit/update",
        TransitionOutcome {
            screen_url: String::new(),
            messages: Messages {
                info: vec![
                    "1099 data made for 1 vendors; 1 failed, 2 not reported".to_string(),
                    "Demo Consultant: no taxpayer identification number".to_string(),
                    // the same message again (ACC-UI-028) is shown once
                    "Demo Consultant: no taxpayer identification number".to_string(),
                ],
                ..Messages::default()
            },
            parameters: BTreeMap::new(),
        },
    );
    session.set_buffer("Edit.name", "Ann".to_string());
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        session.message(),
        "1099 data made for 1 vendors; 1 failed, 2 not reported; Demo Consultant: no taxpayer identification number"
    );
    assert!(!session.message_is_error());
}

#[test]
fn a_service_error_keeps_the_edits_and_stays() {
    let (mut session, link) = edit_session();
    link.answer_transition(
        "/edit/update",
        TransitionOutcome {
            screen_url: String::new(),
            messages: Messages {
                errors: vec!["Party is locked".to_string()],
                ..Messages::default()
            },
            parameters: BTreeMap::new(),
        },
    );
    session.set_buffer("Edit.name", "Ann".to_string());
    press(&mut session, &link, Key::Enter);
    assert!(session.is_dirty("Edit.name"));
    assert_eq!(session.buffer("Edit.name"), "Ann");
    assert_eq!(session.message(), "Party is locked");
    assert!(session.message_is_error());
    assert!(levels(&link).is_empty());
}

#[test]
fn a_cleared_optional_field_posts_empty() {
    let (mut session, link) = edit_session();
    session.set_buffer("Edit.name", "Ann".to_string());
    session.set_buffer("Edit.note", String::new());
    press(&mut session, &link, Key::Enter);
    assert_eq!(transitions(&link)[0].1.get("note").map(String::as_str), Some(""));
}

#[test]
fn save_on_a_leave_from_a_link_posts_the_edited_form_then_goes() {
    let model = ScreenModel::from_json(LEAVE_JSON).unwrap();
    let link = FakeLink::from_json(&[LEAVE_JSON, AWAY_JSON]).unwrap();
    let mut session = Session::new(model);
    session.set_buffer("Edit.note", "hello".to_string());
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "away");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.confirm_choice(), Some("Stay"));
    press(&mut session, &link, Key::Right);
    press(&mut session, &link, Key::Right);
    assert_eq!(session.confirm_choice(), Some("Save"));
    press(&mut session, &link, Key::Enter);
    let posted = transitions(&link);
    assert_eq!(posted[0].0, "/leave/update");
    assert_eq!(posted[0].1.get("note").map(String::as_str), Some("hello"));
    assert_eq!(session.path(), "/away");
}

#[test]
fn back_and_quit_ask_first_when_a_form_changed() {
    let (mut session, link) = edit_session();
    typed(&mut session, &link, "Al");
    session.press(&KeyPress::with_alt(Key::Left), &link);
    assert_eq!(session.confirm_choice(), Some("Stay"));
    press(&mut session, &link, Key::Escape);
    assert!(!session.press(&KeyPress::with_ctrl(Key::Char('q')), &link));
    assert_eq!(session.confirm_choice(), Some("Stay"));
    press(&mut session, &link, Key::Char('d'));
    assert!(session.quitting());
    // Plain q is a letter, not quit.
    let (mut clean, link) = edit_session();
    press(&mut clean, &link, Key::Tab);
    press(&mut clean, &link, Key::Tab);
    assert!(!press(&mut clean, &link, Key::Char('q')));
}

#[test]
fn server_validation_focuses_the_first_bad_field() {
    let (mut session, link) = edit_session();
    link.answer_transition(
        "/edit/update",
        TransitionOutcome::invalid(vec![
            ValidationError {
                form: "Edit".to_string(),
                field: "name".to_string(),
                message: "taken".to_string(),
            },
            ValidationError {
                form: "Edit".to_string(),
                field: "nickname".to_string(),
                message: "not shown here".to_string(),
            },
        ]),
    );
    session.set_buffer("Edit.name", "Ann".to_string());
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "Edit.password");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.focus_id(), "Edit.name");
    assert_eq!(session.mark("Edit.name"), Some("taken"));
    // An error for a field the form does not show is on the message line, not lost.
    assert_eq!(session.message(), "nickname: not shown here");
}

#[test]
fn an_errors_line_repeating_a_validation_error_is_left_out() {
    let (mut session, link) = edit_session();
    let mut outcome = TransitionOutcome::invalid(vec![ValidationError {
        form: "Edit".to_string(),
        field: "name".to_string(),
        message: "Enter the customer name".to_string(),
    }]);
    // the server's errors: the message, its pretty form (no space before the parenthesis, none closing it) and another
    outcome.messages.errors = vec![
        "Enter the customer name".to_string(),
        "Enter the customer name(for field Name on form Edit".to_string(),
        "The customer was not saved".to_string(),
    ];
    link.answer_transition("/edit/update", outcome);
    session.set_buffer("Edit.name", "Ann".to_string());
    press(&mut session, &link, Key::Tab);
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.mark("Edit.name"), Some("Enter the customer name"));
    assert_eq!(session.message(), "The customer was not saved");
}

#[test]
fn paging_and_sort_ask_again_with_the_parameters_of_the_screen() {
    let link = gallery_link();
    let mut session = Session::new(ScreenModel::from_json(GALLERY_LIST_JSON).unwrap());
    session.navigate("/Gallery/List?enumTypeId=ScreenThemeType", false);
    session.run_now(&link);
    press(&mut session, &link, Key::PageDown);
    assert_eq!(session.page_index("EnumList"), 1);
    assert_eq!(session.list_page("EnumList")[0].cells[0].text(), "ENUM_10");
    while session.focus_id() != "EnumList" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::Down);
    assert_eq!(session.row_at("EnumList"), 1);
    press(&mut session, &link, Key::Char('s'));
    press(&mut session, &link, Key::Char('s'));
    press(&mut session, &link, Key::Char('s'));
    press(&mut session, &link, Key::F(5));
    let asked: Vec<BTreeMap<String, String>> = levels(&link).into_iter().map(|(_, params)| params).collect();
    let get = |at: usize, name: &str| asked[at].get(name).cloned();
    // Each request keeps the screen's own parameter.
    assert!(asked
        .iter()
        .all(|params| params.get("enumTypeId").map(String::as_str) == Some("ScreenThemeType")));
    assert_eq!(get(1, "pageIndex").as_deref(), Some("1"));
    // A sort starts again at the first page.
    assert_eq!(get(2, "orderByField").as_deref(), Some("enumId"));
    assert_eq!(get(2, "pageIndex"), None);
    assert_eq!(get(3, "orderByField").as_deref(), Some("-enumId"));
    assert_eq!(get(4, "orderByField").as_deref(), Some("description"));
    // F5 asks for the screen as it is.
    assert_eq!(get(5, "orderByField").as_deref(), Some("description"));
    assert_eq!(session.row_at("EnumList"), 0);
}

#[test]
fn a_held_page_down_sends_one_request_and_keeps_one_waiting() {
    let link = gallery_link();
    let mut session = Session::new(link.level(GALLERY_LIST_PATH, &BTreeMap::new()).unwrap());
    for _ in 0..30 {
        session.on_key(&KeyPress::plain(Key::PageDown));
    }
    let sent = session.take_requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(session.request_counts(), (1, 1));
    // The answer to the first is dropped, and the newest waiting one goes out.
    let reply = link.call(&sent[0].call);
    session.on_reply(sent[0].id, reply);
    let next = session.take_requests();
    assert_eq!(next.len(), 1);
    session.on_reply(next[0].id, link.call(&next[0].call));
    assert_eq!(session.request_counts(), (0, 0));
    assert_eq!(session.page_index("EnumList"), 1);
}

#[test]
fn a_find_form_asks_for_the_screen_with_its_fields_from_the_first_page() {
    let (mut session, link) = list_session();
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "description");
    typed(&mut session, &link, "Enum 1");
    press(&mut session, &link, Key::Enter);
    let (path, params) = levels(&link).pop().expect("level");
    assert_eq!(path, GALLERY_LIST_PATH);
    assert_eq!(params.get("description").map(String::as_str), Some("Enum 1"));
    assert!(!params.contains_key("enumTypeId"), "{params:?}");
    assert!(transitions(&link).is_empty());
}

const GO_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/here", "title": "Here" },
  "c": [{
    "t": "form", "id": "Go", "name": "Go", "transitionUrl": "/there?tab=code&path=old", "method": "get",
    "hidden": [{ "name": "line", "value": "" }],
    "c": [{ "t": "field", "id": "Go.rootId", "name": "rootId", "title": "Root", "w": { "kind": "textLine", "value": "" } }]
  }]
}"#;

#[test]
fn a_form_that_goes_to_a_screen_opens_it_with_its_fields() {
    let link = FakeLink::from_json(&[GO_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(GO_JSON).unwrap());
    session.set_buffer("Go.rootId", "site".to_string());
    press(&mut session, &link, Key::Enter);
    let (path, params) = levels(&link).pop().expect("level");
    assert_eq!(path, "/there");
    assert_eq!(params.get("rootId").map(String::as_str), Some("site"));
    assert_eq!(params.get("tab").map(String::as_str), Some("code"));
    assert_eq!(params.get("path").map(String::as_str), Some("old"));
    // an empty field leaves the parameter out
    assert!(!params.contains_key("line"), "{params:?}");
    assert!(transitions(&link).is_empty());
}

const FIND_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/orders", "title": "Orders" },
  "c": [{
    "t": "formList", "id": "Orders", "name": "Orders",
    "columns": [{ "title": "Order" }, { "title": "Total", "align": "right" }],
    "rowSelection": { "idField": "orderId", "parameter": "orderIds" },
    "rows": [
      { "id": "O1", "selectId": "O1", "cells": ["O1", "10.00"] },
      { "id": "O2", "selectId": "O2", "cells": ["O2", "20.00"] }
    ],
    "paging": { "index": 0, "size": 10, "count": 2 },
    "headerFields": [
      { "t": "field", "id": "Orders.status", "name": "status", "title": "Status",
        "w": { "kind": "textFind", "value": "", "op": "contains", "not": false, "ic": true } },
      { "t": "field", "id": "Orders.placed", "name": "placed", "title": "Placed",
        "w": { "kind": "dateFind", "type": "date", "format": "yyyy-MM-dd", "from": "", "thru": "" } }
    ]
  }]
}"#;

#[test]
fn the_find_dialog_holds_focus_and_its_submit_reloads_the_list() {
    let link = FakeLink::from_json(&[FIND_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(FIND_JSON).unwrap());
    assert_eq!(session.focus_id(), "Orders");
    press(&mut session, &link, Key::Char('f'));
    assert_eq!(session.find_open(), Some("Orders"));
    assert_eq!(session.focus_id(), "Orders.status");
    typed(&mut session, &link, "Approved");
    press(&mut session, &link, Key::F(4));
    assert_eq!(session.buffer("Orders.status#op"), "equals");
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "Orders.placed#from");
    typed(&mut session, &link, "2026-10-01");
    // Focus stays in the dialog.
    press(&mut session, &link, Key::Tab);
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "Orders.status");
    press(&mut session, &link, Key::Enter);
    let (_, params) = levels(&link).pop().expect("level");
    assert_eq!(params.get("status").map(String::as_str), Some("Approved"));
    assert_eq!(params.get("status_op").map(String::as_str), Some("equals"));
    assert_eq!(params.get("placed_from").map(String::as_str), Some("2026-10-01"));
    assert!(!params.contains_key("placed_thru"));
    assert_eq!(session.find_open(), None);
}

#[test]
fn space_selects_a_row_and_star_selects_them_all() {
    let link = FakeLink::from_json(&[FIND_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(FIND_JSON).unwrap());
    press(&mut session, &link, Key::Char(' '));
    assert_eq!(session.selected("Orders").map(|set| set.len()), Some(1));
    press(&mut session, &link, Key::Char('*'));
    assert_eq!(session.selected("Orders").map(|set| set.len()), Some(2));
    press(&mut session, &link, Key::Char('*'));
    assert_eq!(session.selected("Orders").map(|set| set.len()), Some(0));
}

#[test]
fn a_find_dialog_on_a_list_without_find_fields_says_so() {
    let (mut session, link) = list_session();
    while session.focus_id() != "EnumList" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::Char('f'));
    assert_eq!(session.find_open(), None);
    assert!(session.message().contains("no find fields"));
}

#[test]
fn an_unsaved_leave_stays_until_discard() {
    let (mut session, link) = list_session();
    press(&mut session, &link, Key::Tab);
    press(&mut session, &link, Key::Char('z'));
    assert!(session.is_dirty("description"));
    while session.focus_id() != "openDetail" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.confirm_choice(), Some("Stay"));
    assert_eq!(session.path(), GALLERY_LIST_PATH);
    press(&mut session, &link, Key::Escape);
    assert_eq!(session.confirm_choice(), None);
    assert!(session.is_dirty("description"));

    press(&mut session, &link, Key::Enter);
    press(&mut session, &link, Key::Right);
    assert_eq!(session.confirm_choice(), Some("Discard"));
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.path(), GALLERY_DETAIL_PATH);
    assert!(!session.is_dirty("description"));
}

#[test]
fn one_country_change_fetches_states_once_and_clears_a_missing_value() {
    let model = ScreenModel::from_json(GEO_JSON).unwrap();
    let link = FakeLink::from_json(&[GEO_JSON]).unwrap();
    let canada = BTreeMap::from([("countryGeoId".to_string(), "CAN".to_string())]);
    link.answer_options(
        "/states",
        canada.clone(),
        vec![Choice {
            value: "ON".to_string(),
            label: "Ontario".to_string(),
        }],
    );
    let mut session = Session::new(model);
    assert_eq!(session.buffer("state"), "CA");
    // Two quick changes: one request goes, the second waits, and only its answer is applied.
    session.set_buffer("countryGeoId", "MEX".to_string());
    session.set_buffer("countryGeoId", "CAN".to_string());
    assert_eq!(session.request_counts(), (1, 1));
    session.run_now(&link);
    let options: Vec<_> = link
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Options { url, parameters } => Some((url, parameters)),
            _ => None,
        })
        .collect();
    assert_eq!(options.len(), 2);
    assert_eq!(options[1], ("/states".to_string(), canada));
    assert_eq!(session.buffer("state"), "");
    let Some(Widget::DropDown { options, .. }) = session.field_widget("state") else {
        panic!("drop-down");
    };
    assert_eq!(options.len(), 1);
    assert_eq!(options[0].value, "ON");
}

#[test]
fn a_drop_down_filters_as_you_type_and_enter_picks() {
    let link = FakeLink::from_json(&[GEO_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(GEO_JSON).unwrap());
    assert_eq!(session.focus_id(), "countryGeoId");
    press(&mut session, &link, Key::F(4));
    assert!(session.dropdown().is_some());
    typed(&mut session, &link, "can");
    assert_eq!(session.dropdown_choices().len(), 1);
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.buffer("countryGeoId"), "CAN");
    assert!(session.dropdown().is_none());
}

const WIDGETS_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/w", "title": "Widgets" },
  "c": [{ "t": "form", "id": "W", "name": "W", "transitionUrl": "/w/save", "c": [
    { "t": "field", "id": "W.when", "name": "when", "title": "When",
      "w": { "kind": "dateTime", "type": "date", "format": "yyyy-MM-dd", "value": "2026-10-20" } },
    { "t": "field", "id": "W.flag", "name": "flag", "title": "Flag",
      "w": { "kind": "check", "value": "N", "options": [{ "value": "Y", "label": "Yes" }] } },
    { "t": "field", "id": "W.size", "name": "size", "title": "Size",
      "w": { "kind": "radio", "value": "S", "options": [{ "value": "S", "label": "Small" }, { "value": "L", "label": "Large" }] } },
    { "t": "field", "id": "W.extra", "name": "extra", "title": "Extra", "visibleWhen": { "field": "flag", "value": "Y" },
      "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "W.count", "name": "count", "title": "Count", "rules": [{ "kind": "integer", "message": "Whole number" }],
      "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "W.days", "name": "days", "title": "Working days",
      "w": { "kind": "check", "value": "[\"1\",\"2\",\"3\",\"4\",\"5\"]", "options": [
        { "value": "1", "label": "Monday" }, { "value": "2", "label": "Tuesday" }, { "value": "3", "label": "Wednesday" },
        { "value": "4", "label": "Thursday" }, { "value": "5", "label": "Friday" }, { "value": "6", "label": "Saturday" } ] } },
    { "t": "field", "id": "W.groups", "name": "groups", "title": "Groups",
      "w": { "kind": "dropDown", "multiple": true, "value": "[\"staff\",\"ops\"]", "options": [
        { "value": "staff", "label": "Staff" }, { "value": "ops", "label": "Operations" } ] } },
    { "t": "field", "id": "W.legacy", "name": "legacy", "title": "Legacy",
      "w": { "kind": "check", "value": "A,B", "options": [{ "value": "A", "label": "A" }, { "value": "B", "label": "B" }] } },
    { "t": "field", "id": "W.html", "name": "html", "title": "HTML body",
      "w": { "kind": "textArea", "value": "<p>Keep <strong>HTML</strong></p>", "editorType": "rich", "readOnly": true } },
    { "t": "field", "id": "W.go", "name": "go", "title": "Go", "w": { "kind": "submit", "text": "Go", "confirm": "Really?" } }
  ]}]
}"#;

#[test]
fn dates_checks_radios_rules_and_visible_when() {
    let link = FakeLink::from_json(&[WIDGETS_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(WIDGETS_JSON).unwrap());
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5");
    assert_eq!(session.buffer("W.groups"), "staff,ops");
    assert_eq!(session.buffer("W.legacy"), "A,B");
    assert!(!session.is_dirty("W.days") && !session.is_dirty("W.groups"));
    assert_eq!(session.buffer("W.html"), "<p>Keep <strong>HTML</strong></p>");
    assert!(!session.focus_order().iter().any(|item| item.key == "W.html"));
    // F4 opens the picker on the field's date; Right is the next day.
    press(&mut session, &link, Key::F(4));
    assert!(matches!(session.top_overlay(), Some(Overlay::Picker(_))));
    press(&mut session, &link, Key::Right);
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.buffer("W.when"), "2026-10-21");
    // A typed date that does not read is marked at submit.
    session.set_buffer("W.when", "2026-13-40".to_string());
    // Extra is hidden until Flag is checked.
    assert!(!session.focus_order().iter().any(|item| item.key == "W.extra"));
    press(&mut session, &link, Key::Tab);
    assert_eq!(session.focus_id(), "W.flag");
    assert_eq!(session.buffer("W.flag"), "N");
    press(&mut session, &link, Key::Char(' '));
    assert_eq!(session.buffer("W.flag"), "Y");
    assert!(session.dropdown().is_none());
    press(&mut session, &link, Key::Char(' '));
    assert_eq!(session.buffer("W.flag"), "");
    press(&mut session, &link, Key::Char(' '));
    assert_eq!(session.buffer("W.flag"), "Y");
    assert!(session.focus_order().iter().any(|item| item.key == "W.extra"));
    press(&mut session, &link, Key::Tab);
    press(&mut session, &link, Key::Right);
    assert_eq!(session.buffer("W.size"), "L");
    // A checkbox group uses the multiple picker's Space-to-toggle, Enter-to-finish convention.
    assert!(session.focus_key("W.days"));
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Sat");
    assert_eq!(session.dropdown_choices().len(), 1);
    press(&mut session, &link, Key::Enter);
    assert!(session.dropdown().is_none());
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5");
    // Filtering then cancelling makes no selection. Space can also toggle while keeping the picker open.
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Sat");
    press(&mut session, &link, Key::Escape);
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5");
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Sat");
    press(&mut session, &link, Key::Char(' '));
    assert!(session.dropdown().is_some());
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5,6");
    // Enter finishes explicit Space toggles without replacing the other selected values.
    press(&mut session, &link, Key::Enter);
    assert!(session.dropdown().is_none());
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5,6");
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Sat");
    press(&mut session, &link, Key::Char(' '));
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5");
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Sat");
    press(&mut session, &link, Key::Char(' '));
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5,6");
    press(&mut session, &link, Key::Char(' '));
    press(&mut session, &link, Key::Escape);
    assert_eq!(session.buffer("W.days"), "1,2,3,4,5");
    assert!(!session.is_dirty("W.days"));
    // Multiple drop-downs retain their existing Space-to-toggle, Enter-to-finish behavior.
    assert!(session.focus_key("W.groups"));
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Staff");
    press(&mut session, &link, Key::Char(' '));
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.buffer("W.groups"), "ops");
    press(&mut session, &link, Key::Char(' '));
    typed(&mut session, &link, "Staff");
    press(&mut session, &link, Key::Char(' '));
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.buffer("W.groups"), "ops,staff");
    session.set_buffer("W.count", "1.5".to_string());
    while session.focus_id() != "W.go" {
        press(&mut session, &link, Key::Tab);
    }
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.confirm_choice(), Some("No"));
    press(&mut session, &link, Key::Char('y'));
    assert!(session.mark("W.when").is_some_and(|text| text.contains("yyyy-MM-dd")));
    assert_eq!(session.mark("W.count"), Some("Whole number"));
    assert!(transitions(&link).is_empty());
    session.set_buffer("W.when", "2026-10-22".to_string());
    session.set_buffer("W.count", "3".to_string());
    press(&mut session, &link, Key::Enter);
    press(&mut session, &link, Key::Char('y'));
    let posted = transitions(&link);
    assert_eq!(posted[0].1.get("when").map(String::as_str), Some("2026-10-22"));
    assert_eq!(posted[0].1.get("flag").map(String::as_str), Some("Y"));
    assert_eq!(posted[0].1.get("size").map(String::as_str), Some("L"));
    assert_eq!(posted[0].1.get("days").map(String::as_str), Some("1,2,3,4,5"));
    assert_eq!(posted[0].1.get("groups").map(String::as_str), Some("ops,staff"));
    assert_eq!(posted[0].1.get("legacy").map(String::as_str), Some("A,B"));
    assert!(
        !posted[0].1.contains_key("html"),
        "read-only rich text is omitted, not posted as empty HTML"
    );
    assert_eq!(session.buffer("W.html"), "<p>Keep <strong>HTML</strong></p>");
}

const REQUIRE_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/r", "title": "Rules" },
  "c": [{ "t": "form", "id": "R", "name": "R", "transitionUrl": "/r/save", "c": [
    { "t": "field", "id": "R.kind", "name": "kind", "title": "Kind",
      "w": { "kind": "radio", "value": "A", "options": [{ "value": "A", "label": "A" }, { "value": "B", "label": "B" }] } },
    { "t": "field", "id": "R.serial", "name": "serial", "title": "Serial", "requireWhen": { "field": "kind", "value": "B" },
      "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "R.note", "name": "note", "title": "Note", "requireWhen": { "field": "kind", "value": "", "operator": "not-empty" },
      "visibleWhen": { "field": "kind", "value": "A", "operator": "not-equals" }, "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "R.go", "name": "go", "title": "Go", "w": { "kind": "submit", "text": "Go" } }
  ]}]
}"#;

/// A require-when field is required while its rule holds, and a field its visible-when hides is not required.
#[test]
fn require_when_marks_a_field_only_while_its_rule_holds() {
    let link = FakeLink::from_json(&[REQUIRE_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(REQUIRE_JSON).unwrap());
    let submit = |session: &mut Session| {
        while session.focus_id() != "R.go" {
            press(session, &link, Key::Tab);
        }
        press(session, &link, Key::Enter);
    };
    // kind A: serial is not required, and note (required while kind has a value) is hidden
    submit(&mut session);
    assert!(session.mark("R.serial").is_none() && session.mark("R.note").is_none());
    assert_eq!(transitions(&link).len(), 1);
    session.set_buffer("R.kind", "B".to_string());
    submit(&mut session);
    assert!(session.mark("R.serial").is_some(), "serial is required for B");
    assert!(session.mark("R.note").is_some(), "note shows and is required for B");
    assert_eq!(transitions(&link).len(), 1);
}

#[test]
fn numbered_links_follow_by_number_and_paste_is_one_edit() {
    let (mut session, link) = list_session();
    press(&mut session, &link, Key::Tab);
    session.paste("two\nlines");
    assert_eq!(session.buffer("description"), "two lines");
    session.set_buffer("description", String::new());
    press(&mut session, &link, Key::Tab);
    press(&mut session, &link, Key::Char('#'));
    assert!(session.numbering());
    // Link 1 is the first row's link.
    assert_eq!(session.link_number("row0"), Some(1));
    typed(&mut session, &link, "1");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.path(), GALLERY_DETAIL_PATH);
}

#[test]
fn a_login_after_a_401_asks_for_the_screen_again() {
    let link = gallery_link();
    link.require_login("pw");
    let mut session = Session::start(GALLERY_LIST_PATH);
    session.run_now(&link);
    assert!(matches!(session.top_overlay(), Some(Overlay::Login(_))));
    typed(&mut session, &link, "john");
    press(&mut session, &link, Key::Tab);
    typed(&mut session, &link, "wrong");
    press(&mut session, &link, Key::Enter);
    let Some(Overlay::Login(login)) = session.top_overlay() else {
        panic!("login stays");
    };
    assert_eq!(login.error, "Password incorrect");
    assert!(login.password.is_empty(), "the password is not kept");
    typed(&mut session, &link, "pw");
    press(&mut session, &link, Key::Enter);
    assert!(session.top_overlay().is_none());
    assert_eq!(session.path(), GALLERY_LIST_PATH);
    let logins = link
        .calls()
        .into_iter()
        .filter(|call| matches!(call, LinkCall::Login { .. }))
        .count();
    assert_eq!(logins, 2);
}

/// A session that starts with no user (SSH with no key) has the SSH name in its dialog, with the password next.
#[test]
fn the_login_dialog_starts_with_the_login_name() {
    let link = gallery_link();
    link.require_login("pw");
    let mut session = Session::start(GALLERY_LIST_PATH);
    session.set_login_name(Some("john".to_string()));
    session.run_now(&link);
    let Some(Overlay::Login(login)) = session.top_overlay() else {
        panic!("the login dialog opens");
    };
    assert_eq!((login.username.as_str(), login.at), ("john", 1));
    typed(&mut session, &link, "pw");
    press(&mut session, &link, Key::Enter);
    assert!(session.top_overlay().is_none());
    assert_eq!(session.path(), GALLERY_LIST_PATH);
}

/// An account with a second factor: the right password moves the dialog to the code and keeps the password for the
/// next try; Alt+S sends a code to the factor the server named; the code logs in.
#[test]
fn a_login_that_wants_a_code_keeps_the_password_and_asks_for_the_code() {
    let link = gallery_link();
    link.require_login("pw");
    link.require_code(
        "123456",
        vec![Sendable {
            id: "F1".to_string(),
            label: "j***@example.com".to_string(),
        }],
    );
    let mut session = Session::start(GALLERY_LIST_PATH);
    session.set_login_name(Some("john".to_string()));
    session.run_now(&link);
    typed(&mut session, &link, "pw");
    press(&mut session, &link, Key::Enter);
    let Some(Overlay::Login(login)) = session.top_overlay() else {
        panic!("the dialog stays for the code");
    };
    assert_eq!(login.at, 2, "the code is next");
    assert!(login.code_wanted);
    assert_eq!(login.password, "pw", "the password is kept for the try with the code");
    assert_eq!(login.error, "Enter the code from your authenticator app");
    session.press(&KeyPress::with_alt(Key::Char('s')), &link);
    let Some(Overlay::Login(login)) = session.top_overlay() else {
        panic!("the dialog stays");
    };
    assert_eq!(login.error, "A code was sent to j***@example.com");
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::SendCode { factor, .. } if factor == "F1")));
    typed(&mut session, &link, "111111");
    press(&mut session, &link, Key::Enter);
    let Some(Overlay::Login(login)) = session.top_overlay() else {
        panic!("a wrong code keeps the dialog");
    };
    assert_eq!(login.error, "Authentication code is not valid");
    assert_eq!(login.password, "pw");
    assert!(login.code.is_empty(), "the code is typed again");
    typed(&mut session, &link, "123456");
    press(&mut session, &link, Key::Enter);
    assert!(session.top_overlay().is_none(), "logged in");
    assert_eq!(session.path(), GALLERY_LIST_PATH);
}

fn rules() -> AdminSetup {
    AdminSetup {
        min_length: 12,
        min_digits: 1,
        min_others: 1,
        secret_required: false,
    }
}

/// A server with no accounts: the login dialog asks as it opens, and turns to setup with the SSH name kept.
fn setup_session() -> (Session, FakeLink) {
    let link = gallery_link();
    link.require_login("old-pw");
    link.need_initial_admin(rules());
    let mut session = Session::start(GALLERY_LIST_PATH);
    session.set_login_name(Some("admin".to_string()));
    session.run_now(&link);
    (session, link)
}

fn setup_login(session: &Session) -> &Login {
    match session.top_overlay() {
        Some(Overlay::Login(login)) => login,
        other => panic!("the login dialog is on top: {other:?}"),
    }
}

/// Type each of the setup's fields from the password on, Tab between them.
fn fill_setup(session: &mut Session, link: &FakeLink, values: [&str; 4]) {
    for value in values {
        typed(session, link, value);
        press(session, link, Key::Tab);
    }
}

fn creations(link: &FakeLink) -> usize {
    link.calls()
        .iter()
        .filter(|call| matches!(call, LinkCall::CreateInitialAdmin(_)))
        .count()
}

#[test]
fn a_needed_answer_turns_the_login_dialog_to_setup_and_its_fields_cycle() {
    let (mut session, link) = setup_session();
    assert!(link.calls().iter().any(|call| matches!(call, LinkCall::InitialAdmin)));
    let login = setup_login(&session);
    assert_eq!(login.setup, Some(rules()));
    assert_eq!(
        (login.username.as_str(), login.at),
        ("admin", 1),
        "the username kept, the password next"
    );
    assert_eq!(login.labels().len(), 5);
    for at in [2, 3, 4, 0, 1] {
        press(&mut session, &link, Key::Tab);
        assert_eq!(setup_login(&session).at, at);
    }
    press(&mut session, &link, Key::BackTab);
    press(&mut session, &link, Key::Up);
    assert_eq!(setup_login(&session).at, 4, "Shift+Tab and Up go back, around the end");
    press(&mut session, &link, Key::Down);
    assert_eq!(setup_login(&session).at, 0);
    // Enter before the last field is the next field: nothing is sent
    press(&mut session, &link, Key::Enter);
    assert_eq!(setup_login(&session).at, 1);
    assert_eq!(creations(&link), 0);
    press(&mut session, &link, Key::Escape);
    assert!(session.top_overlay().is_none(), "Esc closes the dialog");
}

#[test]
fn setup_checks_its_fields_and_passwords_before_it_sends() {
    let (mut session, link) = setup_session();
    fill_setup(
        &mut session,
        &link,
        ["Secret-pass-1", "Secret-pass-2", "Ada Admin", "ada@example.com"],
    );
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    let login = setup_login(&session);
    assert_eq!(login.error, "The passwords do not match");
    assert!(
        login.password.is_empty() && login.verify.is_empty(),
        "both passwords are typed again"
    );
    assert_eq!(login.at, 1);
    session.set_login_field(4, "");
    session.set_login_field(1, "Secret-pass-1");
    session.set_login_field(2, "Secret-pass-1");
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    let login = setup_login(&session);
    assert_eq!((login.error.as_str(), login.at), ("Email is required", 4));
    assert_eq!(creations(&link), 0, "nothing is sent until the fields are right");
}

/// SEC-WEB-08: from another machine the setup takes the server's setup secret too, masked, and sends nothing without it.
#[test]
fn setup_from_another_machine_asks_for_the_setup_secret() {
    let link = gallery_link();
    link.require_login("old-pw");
    link.need_initial_admin(AdminSetup {
        secret_required: true,
        ..rules()
    });
    let mut session = Session::start(GALLERY_LIST_PATH);
    session.set_login_name(Some("admin".to_string()));
    session.run_now(&link);
    let login = setup_login(&session);
    assert_eq!(login.labels().len(), 6);
    assert!(login.masked(5));
    fill_setup(
        &mut session,
        &link,
        ["Secret-pass-1", "Secret-pass-1", "Ada Admin", "ada@example.com"],
    );
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    let login = setup_login(&session);
    assert_eq!((login.error.as_str(), login.at), ("Setup secret is required", 5));
    assert_eq!(creations(&link), 0);
    session.set_login_field(5, "abcd-efgh-jkmn-pqrs-tuvw");
    assert_eq!(setup_login(&session).setup_secret, "abcd-efgh-jkmn-pqrs-tuvw");
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    assert_eq!(creations(&link), 1);
}

#[test]
fn setup_creates_the_administrator_then_logs_in_with_it() {
    let (mut session, link) = setup_session();
    fill_setup(
        &mut session,
        &link,
        ["Secret-pass-1", "Secret-pass-1", "Ada Admin", "ada@example.com"],
    );
    // Tab after the email went around to the username; Enter in the last field sends
    session.set_login_field(4, "ada@example.com");
    press(&mut session, &link, Key::Enter);
    assert!(session.top_overlay().is_none(), "logged in");
    assert_eq!(session.path(), GALLERY_LIST_PATH);
    let calls = link.calls();
    let created = calls
        .iter()
        .position(|call| {
            matches!(call, LinkCall::CreateInitialAdmin(account)
                if account.username == "admin" && account.full_name == "Ada Admin"
                    && account.email == "ada@example.com")
        })
        .expect("the account is sent");
    let logged = calls
        .iter()
        .position(|call| matches!(call, LinkCall::Login { username, .. } if username == "admin"))
        .expect("then the login");
    assert!(created < logged, "{calls:?}");
}

#[test]
fn a_refused_setup_shows_why_and_clears_the_passwords() {
    let (mut session, link) = setup_session();
    link.refuse_initial_admin("Password must have at least 12 characters", false);
    fill_setup(
        &mut session,
        &link,
        ["short-1", "short-1", "Ada Admin", "ada@example.com"],
    );
    session.press(&KeyPress::with_alt(Key::Enter), &link);
    let login = setup_login(&session);
    assert_eq!(login.error, "Password must have at least 12 characters");
    assert!(login.password.is_empty() && login.verify.is_empty());
    assert!(login.setup.is_some(), "still setup: no account was made");
    assert_eq!((login.full_name.as_str(), login.at), ("Ada Admin", 1));
    // someone else made the first account meanwhile: the dialog is the login again, its username and message kept
    link.refuse_initial_admin(
        "Can only create initial admin account if there are no UserAccount records",
        true,
    );
    session.set_login_field(1, "Secret-pass-1");
    session.set_login_field(2, "Secret-pass-1");
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    let login = setup_login(&session);
    assert!(login.setup.is_none(), "back to the login");
    assert_eq!(login.username, "admin");
    assert!(login.error.contains("no UserAccount records"), "{}", login.error);
    assert!(login.password.is_empty());
    assert_eq!(creations(&link), 2);
}

#[test]
fn a_server_with_accounts_keeps_the_ordinary_login_dialog() {
    let link = gallery_link();
    link.require_login("pw");
    let mut session = Session::start(GALLERY_LIST_PATH);
    session.run_now(&link);
    assert!(link.calls().iter().any(|call| matches!(call, LinkCall::InitialAdmin)));
    let login = setup_login(&session);
    assert!(login.setup.is_none());
    assert_eq!(login.labels(), ["Username", "Password", "Code"]);
    // Ctrl+S is no send in the login: only Enter logs in
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    assert_eq!(creations(&link), 0);
}

#[test]
fn a_late_answer_for_a_request_not_sent_is_ignored() {
    let (mut session, link) = list_session();
    session.on_reply(999, Reply::Level(link.level(GALLERY_DETAIL_PATH, &BTreeMap::new())));
    assert_eq!(session.path(), GALLERY_LIST_PATH);
}

#[test]
fn empty_controls_follow_java_form_exclusion() {
    for exclude in [false, true] {
        let json = EDIT_JSON.replace(
            "\"transitionUrl\"",
            &format!("\"excludeEmptyFields\":{exclude},\"transitionUrl\""),
        );
        let model = ScreenModel::from_json(&json).unwrap();
        let link = FakeLink::from_json(&[&json]).unwrap();
        let mut session = Session::new(model);
        session.set_buffer("Edit.name", "Ann".into());
        // An untouched empty password and an explicitly cleared note are both successful controls.
        session.set_buffer("Edit.note", String::new());
        press(&mut session, &link, Key::Enter);
        let calls = transitions(&link);
        assert_eq!(calls[0].1.get("note").map(String::as_str), (!exclude).then_some(""));
        assert_eq!(calls[0].1.get("password").map(String::as_str), (!exclude).then_some(""));
        assert_eq!(calls[0].1.get("partyId").map(String::as_str), Some("P1"));
    }
}

/// An action disabled for a reason (`disable-condition`, `disable-message`) is drawn dimmed and takes focus, its reason on
/// the hint line and as the message on Enter, and follows nothing; a link disabled with no reason stays out (ACC-UI-039).
#[test]
fn an_action_disabled_for_a_reason_is_shown_and_says_why() {
    const REASON: &str = "Exchange rates are maintained by a finance administrator of every organization";
    let json = format!(
        r#"{{"v":1,"screen":{{"path":"/rates","title":"Rates"}},"c":[
            {{"t":"link","id":"AddRate","text":"Add exchange rate","url":"","kind":"anchor-button","disabled":true,"reason":"{REASON}"}},
            {{"t":"link","id":"Unreached","text":"Not reached","url":"/x","disabled":true}},
            {{"t":"link","id":"Open","text":"Open rates","url":"/open"}}]}}"#
    );
    let model = ScreenModel::from_json(&json).unwrap();
    let link = FakeLink::from_json(&[json.as_str()]).unwrap();
    let text = moqui_tui::render_text(&model, &link, 140, 12);
    assert!(text.contains("[ Add exchange rate ]"), "{text}");
    // focused first: the hint line names it and its reason
    assert!(text.contains(&format!("Add exchange rate · {REASON}")), "{text}");
    assert!(!text.contains("Not reached"), "{text}");

    let mut session = Session::new(model);
    let order: Vec<&str> = session.focus_order().iter().map(|item| item.node.as_str()).collect();
    assert_eq!(order, ["AddRate", "Open"]);
    assert_eq!(session.focus_id(), "AddRate");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.message(), REASON);
    assert!(
        levels(&link).is_empty() && transitions(&link).is_empty(),
        "{:?}",
        link.calls()
    );
}
