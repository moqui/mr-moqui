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

//! The agent mode: one command line in, one settled answer out, with elements found by id, number or label.

use std::collections::BTreeMap;

use moqui_tui::{
    gallery_link, Agent, Choice, FakeLink, LinkCall, MenuItem, ShellInfo, GALLERY_DETAIL_PATH, GALLERY_LIST_PATH,
};

const GEO_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/geo", "title": "Geo" },
  "c": [ { "t": "form", "id": "Geo", "name": "Geo", "transitionUrl": "/geo/save", "c": [
    { "t": "field", "id": "countryGeoId", "name": "countryGeoId", "title": "Country",
      "w": { "kind": "dropDown", "value": "USA", "options": [ { "value": "USA", "label": "USA" }, { "value": "CAN", "label": "Canada" } ] } },
    { "t": "field", "id": "state", "name": "stateProvinceGeoId", "title": "State",
      "w": { "kind": "dropDown", "value": "CA", "options": [ { "value": "CA", "label": "California" } ],
             "optionsUrl": "/states", "dependsOn": { "countryGeoId": "countryGeoId" } } },
    { "t": "field", "id": "city", "name": "city", "title": "City", "w": { "kind": "textLine", "value": "" } }
  ] } ]
}"#;

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

#[test]
fn an_answer_names_the_place_and_focus_and_ends_with_its_outcome() {
    let mut agent = Agent::open(gallery_link(), GALLERY_LIST_PATH, 100, 40);
    let answer = agent.run("look");
    assert!(answer.ok);
    let text = lines(&answer.text);
    assert_eq!(text[0], format!("path {GALLERY_LIST_PATH}"));
    assert!(text[1].starts_with("title "), "{}", answer.text);
    assert!(text[2].starts_with("focus "), "{}", answer.text);
    assert_eq!(text[3], "overlay none");
    assert_eq!(text.last(), Some(&"--- ok"));
    // The focused element is drawn in reverse video, so it is between [[ and ]].
    assert!(answer.text.contains("[["), "{}", answer.text);
}

#[test]
fn elements_are_found_by_label_id_and_number() {
    let mut agent = Agent::open(gallery_link(), GALLERY_LIST_PATH, 100, 40);
    let listed = agent.run("elements");
    assert!(listed.ok);
    let line = listed
        .text
        .lines()
        .find(|line| line.contains("openDetail"))
        .unwrap_or_else(|| panic!("{}", listed.text));
    assert!(line.contains("link") && line.contains("Open detail"), "{line}");
    let number: String = line[2..].chars().take_while(char::is_ascii_digit).collect();

    let focused = agent.run(&format!("focus #{number}"));
    assert!(focused.ok, "{}", focused.text);
    assert!(focused.text.contains("focus link openDetail"), "{}", focused.text);
    assert!(agent.run("focus openDetail").ok);
    let done = agent.run("do open detail");
    assert!(done.ok, "{}", done.text);
    assert_eq!(agent.session().path(), GALLERY_DETAIL_PATH);
    assert!(done.text.starts_with(&format!("path {GALLERY_DETAIL_PATH}")));
}

#[test]
fn set_types_a_field_and_chooses_a_drop_down_the_way_a_person_does() {
    let link = FakeLink::from_json(&[GEO_JSON]).unwrap();
    link.answer_options(
        "/states",
        BTreeMap::from([("countryGeoId".to_string(), "CAN".to_string())]),
        vec![Choice {
            value: "ON".to_string(),
            label: "Ontario".to_string(),
        }],
    );
    let mut agent = Agent::open(link.clone(), "/geo", 100, 30);
    let city = agent.run("set City Ottawa East");
    assert!(city.ok, "{}", city.text);
    assert_eq!(agent.session().buffer("city"), "Ottawa East");
    // By label; the dependent state's options are asked for and settled before the answer.
    let country = agent.run("set Country Canada");
    assert!(country.ok, "{}", country.text);
    assert_eq!(agent.session().buffer("countryGeoId"), "CAN");
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Options { url, .. } if url == "/states")));
    // By value.
    let state = agent.run("set state ON");
    assert!(state.ok, "{}", state.text);
    assert_eq!(agent.session().buffer("state"), "ON");
    let elements = agent.run("elements");
    assert!(elements.text.contains("= \"ON\""), "{}", elements.text);
    assert!(elements.text.contains("[changed]"), "{}", elements.text);
    // A choice that is not there is an error, and the field keeps its value.
    let wrong = agent.run("set Country Atlantis");
    assert!(!wrong.ok);
    assert!(
        wrong
            .text
            .ends_with("--- error no choice of countryGeoId matches \"Atlantis\""),
        "{}",
        wrong.text
    );
    assert_eq!(agent.session().buffer("countryGeoId"), "CAN");
}

#[test]
fn a_palette_search_is_settled_before_the_answer() {
    let link = gallery_link();
    link.answer_shell(ShellInfo {
        menu: vec![MenuItem {
            title: "Gallery".to_string(),
            url: "/Gallery".to_string(),
            children: Vec::new(),
        }],
        ..ShellInfo::default()
    });
    link.answer_json(
        "/shellSearch",
        BTreeMap::from([
            ("text".to_string(), "so-1".to_string()),
            ("limit".to_string(), "5".to_string()),
        ]),
        serde_json::json!({ "groups": [{ "label": "Orders", "rows": [{ "title": "SO-10023", "url": "/o?orderId=SO-10023" }] }] }),
    );
    let mut agent = Agent::open(link, GALLERY_LIST_PATH, 100, 40);
    assert!(agent.run("key Ctrl+K").ok);
    let typed = agent.run("type so-1");
    assert!(typed.text.contains("SO-10023"), "{}", typed.text);
    assert!(typed.text.contains("overlay palette \"so-1\""), "{}", typed.text);
}

#[test]
fn json_answers_parse_and_errors_leave_the_session() {
    let mut agent = Agent::open(gallery_link(), GALLERY_LIST_PATH, 100, 40);
    let answer = agent.run("look --json");
    let value: serde_json::Value = serde_json::from_str(&answer.text).unwrap();
    assert_eq!(value["ok"], serde_json::json!(true));
    assert_eq!(value["path"], serde_json::json!(GALLERY_LIST_PATH));
    assert!(value["elements"]
        .as_array()
        .unwrap()
        .iter()
        .any(|element| element["id"] == "openDetail"));
    assert!(value["screen"].as_array().unwrap().len() > 3);

    let unknown = agent.run("dance");
    assert!(!unknown.ok);
    assert!(unknown.text.ends_with("--- error unknown command dance; try help"));
    let missing = agent.run("do Nowhere at all");
    assert!(!missing.ok);
    assert!(missing.text.contains("no element named"), "{}", missing.text);
    assert!(!agent.run("key NotAKey").ok);
    assert!(agent.run("help").text.contains("Keys on this screen:"));
    assert!(agent.run("size 160x50").ok);
    assert!(!agent.run("size big").ok);

    assert!(agent.run("close").ok);
    assert!(agent.closed());
    assert!(!agent.run("look").ok);
}

const CONFIRM_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/c", "title": "Confirm" },
  "c": [ { "t": "form", "id": "C", "name": "C", "transitionUrl": "/c/save", "c": [
    { "t": "field", "id": "C.note", "name": "note", "title": "Note", "w": { "kind": "textLine", "value": "" } },
    { "t": "field", "id": "C.go", "name": "go", "title": "Go", "w": { "kind": "submit", "text": "Go", "confirm": "Really?" } }
  ] } ]
}"#;

#[test]
fn a_confirm_is_answered_by_its_choice_and_empty_rows_are_counted() {
    let link = FakeLink::from_json(&[CONFIRM_JSON]).unwrap();
    link.answer_transition("/c/save", moqui_tui::TransitionOutcome::saved("Saved"));
    let mut agent = Agent::open(link.clone(), "/c", 100, 40);
    let asked = agent.run("do Go");
    assert!(
        asked
            .text
            .contains("overlay confirm \"Really?\", choices No|Yes, at \"No\""),
        "{}",
        asked.text
    );
    let wrong = agent.run("do Maybe");
    assert!(
        wrong.text.ends_with("--- error the confirm's choices are No, Yes"),
        "{}",
        wrong.text
    );
    let saved = agent.run("do yes");
    assert!(saved.ok, "{}", saved.text);
    assert!(saved.text.contains("message Saved"), "{}", saved.text);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Transition { url, .. } if url == "/c/save")));
    // The rows between the form and the key line are one counted line, not thirty empty ones.
    assert!(saved.text.contains("empty rows)"), "{}", saved.text);
    assert!(!saved.text.contains("\n\n\n"), "{}", saved.text);
}

/// The login dialog's fields are elements while it is on top: `set` fills them, `do Log in` sends it, and the
/// password is never in an answer.
#[test]
fn the_login_dialog_is_filled_by_set_and_sent_by_do() {
    let link = gallery_link();
    link.require_login("secret-pw");
    link.require_code("123456", Vec::new());
    let mut agent = Agent::open(link.clone(), GALLERY_LIST_PATH, 100, 40);
    let listed = agent.run("elements");
    assert!(listed.text.contains("login.password"), "{}", listed.text);
    assert!(agent.run("set Username john").ok);
    let answer = agent.run("set Password secret-pw");
    assert!(answer.ok, "{}", answer.text);
    assert!(!answer.text.contains("secret-pw"), "{}", answer.text);
    let json = agent.run("elements --json");
    assert!(!json.text.contains("secret-pw"), "{}", json.text);
    assert!(json.text.contains("•••••••••"), "{}", json.text);
    // The right password wants the code: an error answer that says so, the dialog still open.
    let wanted = agent.run("do Log in");
    assert!(!wanted.ok, "{}", wanted.text);
    assert!(wanted.text.contains("Enter the code"), "{}", wanted.text);
    assert!(agent.run("set Code 123456").ok);
    let done = agent.run("do Log in");
    assert!(done.ok, "{}", done.text);
    assert!(done.text.contains("overlay none"), "{}", done.text);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Login { username, code, .. } if username == "john" && code == "123456")));
}

/// A form whose transition writes, in a section-iterate entry (its domId ends `_1`), a find field with a from and a
/// thru part, a link that writes and one that does not.
const EDIT_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/edit", "title": "Edit" },
  "c": [
    { "t": "form", "id": "EditThing", "name": "EditThing", "domId": "EditThing_1", "writes": true, "transitionUrl": "/edit/save", "c": [
      { "t": "field", "id": "EditThing.description", "name": "description", "title": "Description", "w": { "kind": "textLine", "value": "Old" } },
      { "t": "field", "id": "EditThing.secret", "name": "secret", "title": "Secret", "w": { "kind": "password" } },
      { "t": "field", "id": "EditThing.when", "name": "when", "title": "When", "w": { "kind": "dateFind", "from": "", "thru": "" } },
      { "t": "field", "id": "EditThing.save", "name": "save", "title": "Save", "w": { "kind": "submit", "text": "Save" } }
    ] },
    { "t": "form", "id": "Peek", "name": "Peek", "domId": "Peek", "transitionUrl": "/edit/peek", "c": [
      { "t": "field", "id": "Peek.note", "name": "note", "title": "Note", "w": { "kind": "textLine", "value": "" } },
      { "t": "field", "id": "Peek.look", "name": "look", "title": "Look", "w": { "kind": "submit", "text": "Look" } }
    ] },
    { "t": "link", "id": "deleteIt", "text": "Delete", "url": "/edit/delete", "method": "post", "writes": true },
    { "t": "link", "id": "openOther", "text": "Other", "url": "/other" }
  ]
}"#;

#[test]
fn writes_are_marked_and_browser_values_go_in_by_dom_id_and_name() {
    let link = FakeLink::from_json(&[EDIT_JSON]).unwrap();
    let mut agent = Agent::open(link, "/edit", 100, 30);
    let plain = agent.run("elements");
    assert!(!plain.text.contains("[needs approval]"), "{}", plain.text);
    agent.set_marks_writes(true);
    let listed = agent.run("elements");
    let line = |id: &str| {
        listed
            .text
            .lines()
            .find(|line| line.contains(id))
            .unwrap_or_else(|| panic!("{id}: {}", listed.text))
            .to_string()
    };
    assert!(line("EditThing.save").ends_with("[needs approval]"), "{}", listed.text);
    assert!(line("deleteIt").ends_with("[needs approval]"), "{}", listed.text);
    assert!(!line("Peek.look").contains("[needs approval]"), "{}", listed.text);
    assert!(!line("openOther").contains("[needs approval]"), "{}", listed.text);
    let look = agent.run("look");
    assert!(look.text.contains("[needs approval] Save, Delete"), "{}", look.text);
    let json: serde_json::Value = serde_json::from_str(&agent.run("elements --json").text).unwrap();
    let writes: Vec<&str> = json["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|element| element["writes"] == serde_json::json!(true))
        .map(|element| element["id"].as_str().unwrap())
        .collect();
    assert_eq!(writes, ["EditThing.save", "deleteIt"]);

    // the browser's values, by the form's qvt id and the names it posts; a password and an unknown name are not taken
    let before = agent.snapshot();
    let ignored = agent
        .set_values(
            "EditThing_1",
            &BTreeMap::from([
                ("description".to_string(), "New".to_string()),
                ("when_from".to_string(), "2026-10-01".to_string()),
                ("secret".to_string(), "pw".to_string()),
                ("nothing".to_string(), "x".to_string()),
            ]),
        )
        .unwrap();
    assert_eq!(ignored, ["nothing", "secret"]);
    assert_eq!(agent.session().buffer("EditThing.description"), "New");
    assert_eq!(agent.session().buffer("EditThing.when#from"), "2026-10-01");
    assert!(agent.set_values("NoSuchForm", &BTreeMap::new()).is_err());
    assert_eq!(
        agent.unsaved(),
        BTreeMap::from([(
            "EditThing_1".to_string(),
            BTreeMap::from([
                ("description".to_string(), "New".to_string()),
                ("when_from".to_string(), "2026-10-01".to_string())
            ])
        )])
    );
    assert_eq!(
        agent.original_value("EditThing_1", "description").as_deref(),
        Some("Old")
    );

    // what the page does to follow: a fill per changed value, naming the form, its occurrence and the field
    let events = agent.follow(&before);
    let fills: Vec<&serde_json::Value> = events
        .iter()
        .filter(|(kind, _)| kind == "ui.fill")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(fills.len(), 2, "{events:?}");
    let description = fills.iter().find(|fill| fill["field"] == "description").unwrap();
    assert_eq!(description["form"], "EditThing");
    assert_eq!(description["domId"], "EditThing_1");
    assert_eq!(description["occurrence"], 0);
    assert_eq!(description["scope"], "page");
    assert_eq!(description["value"], "New");
    assert_eq!(description["label"], "Description");
    assert_eq!(description["tuiId"], "EditThing.description");
    assert_eq!(description["part"], serde_json::Value::Null);
    let when = fills.iter().find(|fill| fill["field"] == "when_from").unwrap();
    assert_eq!(when["part"], "from");
    // the focus moved on the last set, not by a command
    let moved = agent.snapshot();
    assert!(agent.run("focus Note").ok);
    let focus = agent.follow(&moved);
    assert_eq!(focus.len(), 1, "{focus:?}");
    assert_eq!(focus[0].0, "ui.focus");
    assert_eq!(focus[0].1["tuiId"], "Peek.note");
    assert_eq!(focus[0].1["form"], "Peek");
    // a navigation is one event, and the edits it kept follow with it
    let here = agent.snapshot();
    agent.run("go /edit?x=1");
    let navigated = agent.follow(&here);
    assert_eq!(navigated[0].0, "ui.navigate", "{navigated:?}");
    assert_eq!(navigated[0].1["parameters"]["x"], "1");
}

#[test]
fn a_name_that_finds_no_element_answers_the_nearest() {
    let link = FakeLink::from_json(&[EDIT_JSON]).unwrap();
    let mut agent = Agent::open(link, "/edit", 100, 30);
    let missing = agent.run("do Svae");
    assert!(!missing.ok);
    assert!(
        missing
            .text
            .ends_with("--- error no element named \"Svae\"; did you mean: Save (button)?"),
        "{}",
        missing.text
    );
    let far = agent.run("do Quantity");
    assert!(
        far.text.ends_with("no element named \"Quantity\"; see elements"),
        "{}",
        far.text
    );
    let set = agent.run("set Descripton Thing");
    assert!(set.text.contains("did you mean: Description (text)"), "{}", set.text);
}

/// A dialog with a form, and a list with find fields (one a text find that begins).
const DIALOG_FIND_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/groups", "title": "Groups" },
  "c": [
    { "t": "dialog", "id": "NewGroupDialog", "title": "New Group", "button": "New Group", "c": [
      { "t": "form", "id": "NewGroup", "name": "NewGroup", "transitionUrl": "/groups/create", "c": [
        { "t": "field", "id": "NewGroup.description", "name": "description", "title": "Description",
          "w": { "kind": "textLine", "value": "" } },
        { "t": "field", "id": "NewGroup.go", "name": "go", "title": "Create", "w": { "kind": "submit", "text": "Create" } }
      ]}
    ]},
    { "t": "formList", "id": "Groups", "name": "Groups",
      "columns": [{ "title": "Group" }],
      "rows": [ { "id": "G1", "cells": ["G1"] } ],
      "paging": { "index": 0, "size": 10, "count": 1 },
      "headerFields": [
        { "t": "field", "id": "Groups.groupName", "name": "groupName", "title": "Group Name",
          "w": { "kind": "textFind", "value": "", "op": "begins", "not": false, "ic": true } },
        { "t": "field", "id": "Groups.owner", "name": "owner", "title": "Owner",
          "w": { "kind": "textFind", "value": "", "op": "contains", "not": false, "ic": true } },
        { "t": "field", "id": "Groups.find", "name": "find", "title": "Find", "w": { "kind": "submit", "text": "Find" } }
      ] }
  ]
}"#;

#[test]
fn a_field_out_of_reach_says_how_to_reach_it_and_a_find_filters_with_its_operator() {
    let link = FakeLink::from_json(&[DIALOG_FIND_JSON]).unwrap();
    let mut agent = Agent::open(link.clone(), "/groups", 100, 40);
    // the name and the value apart: a value with spaces is not read as part of the name
    let closed = agent.set_field("Description", "Agent eval hand-off");
    assert!(!closed.ok);
    assert!(
        closed.text.contains(
            "--- error Description is a field of the dialog \"New Group\", which is closed; open it first with do \"New Group\""
        ),
        "{}",
        closed.text
    );
    // the text command says the same of its leading words, not of the label and part of the value
    let typed = agent.run("set Description Agent eval hand-off");
    assert!(typed.text.contains("Description is a field of the dialog \"New Group\""), "{}", typed.text);
    let find_field = agent.set_field("Group Name", "adm");
    assert!(
        find_field.text.contains("Group Name is a find field of the list Groups; do Find to open its find"),
        "{}",
        find_field.text
    );
    assert!(agent.run("elements").text.contains("[find: do Find]"));

    assert!(agent.run("do New Group").ok);
    let set = agent.set_field("Description", "Agent eval hand-off");
    assert!(set.ok, "{}", set.text);
    assert_eq!(agent.session().buffer("NewGroup.description"), "Agent eval hand-off");
    assert!(agent.run("key Esc").ok);

    let opened = agent.run("do Find");
    assert!(opened.ok, "{}", opened.text);
    assert!(opened.text.contains("overlay find"), "{}", opened.text);
    assert!(agent.set_field("Group Name", "adm").ok);
    let found = agent.run("do Find");
    assert!(found.ok, "{}", found.text);
    let (path, parameters) = link
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Level { path, parameters } => Some((path, parameters)),
            _ => None,
        })
        .next_back()
        .expect("the list again");
    assert_eq!(path, "/groups");
    assert_eq!(parameters.get("groupName").map(String::as_str), Some("adm"));
    assert_eq!(parameters.get("groupName_op").map(String::as_str), Some("begins"), "the field's own operator");
    assert!(!parameters.contains_key("owner_op") && !parameters.contains_key("owner_ic"), "{parameters:?}");
}
