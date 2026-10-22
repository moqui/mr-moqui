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
    gallery_link, merge_keys, Action, Choice, FakeLink, Key, KeyMap, KeyPress, LinkCall, Messages, Overlay, Reply,
    ScreenLink, ScreenModel, Sendable, Session, ShellInfo, TransitionOutcome, ValidationError, Widget,
    GALLERY_DETAIL_PATH, GALLERY_LIST_JSON, GALLERY_LIST_PATH,
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
            LinkCall::Level { path, parameters } => Some((path, parameters)),
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
        &KeyMap::baseline(),
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
    // Alt+2 is the second tab of the header; the header stays and is not asked for again.
    session.press(&KeyPress::with_alt(Key::Char('2')), &link);
    assert_eq!(session.paths(), ["/apps/app/Party", "/apps/app/Party/Roles"]);
    assert_eq!(levels(&link).len(), 3);
    // F3 goes back to the screen before, asking for it again.
    press(&mut session, &link, Key::F(3));
    assert_eq!(session.path(), "/apps/app/Party/EditParty");
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
    press(&mut session, &link, Key::F(3));
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
fn paging_and_sort_ask_again_with_the_parameters_of_the_screen() {
    let link = gallery_link();
    let mut session = Session::new(ScreenModel::from_json(GALLERY_LIST_JSON).unwrap());
    session.navigate("/Gallery/List?enumTypeId=ScreenThemeType", false);
    session.run_now(&link);
    press(&mut session, &link, Key::F(8));
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
      "w": { "kind": "check", "value": "", "options": [{ "value": "Y", "label": "Yes" }] } },
    { "t": "field", "id": "W.size", "name": "size", "title": "Size",
      "w": { "kind": "radio", "value": "S", "options": [{ "value": "S", "label": "Small" }, { "value": "L", "label": "Large" }] } },
    { "t": "field", "id": "W.extra", "name": "extra", "title": "Extra", "visibleWhen": { "field": "flag", "value": "Y" },
      "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "W.count", "name": "count", "title": "Count", "rules": [{ "kind": "integer", "message": "Whole number" }],
      "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "W.go", "name": "go", "title": "Go", "w": { "kind": "submit", "text": "Go", "confirm": "Really?" } }
  ]}]
}"#;

#[test]
fn dates_checks_radios_rules_and_visible_when() {
    let link = FakeLink::from_json(&[WIDGETS_JSON]).unwrap();
    let mut session = Session::new(ScreenModel::from_json(WIDGETS_JSON).unwrap());
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
    press(&mut session, &link, Key::Char(' '));
    assert_eq!(session.buffer("W.flag"), "Y");
    assert!(session.focus_order().iter().any(|item| item.key == "W.extra"));
    press(&mut session, &link, Key::Tab);
    press(&mut session, &link, Key::Right);
    assert_eq!(session.buffer("W.size"), "L");
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

#[test]
fn a_late_answer_for_a_request_not_sent_is_ignored() {
    let (mut session, link) = list_session();
    session.on_reply(999, Reply::Level(link.level(GALLERY_DETAIL_PATH, &BTreeMap::new())));
    assert_eq!(session.path(), GALLERY_LIST_PATH);
}
