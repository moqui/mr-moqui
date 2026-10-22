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

//! What loads after a screen and what a list posts: dynamic containers and dialogs, a server-search drop-down, value
//! and default urls, click to edit, row forms, a multi form and a row action with its selection. No Revue.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use moqui_tui::{Choice, FakeLink, Key, KeyPress, LinkCall, Overlay, Session, TransitionOutcome};

const DYN_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/dyn", "title": "Dynamic" },
  "c": [
    { "t": "dynamicContainer", "id": "Part", "url": "/part" },
    { "t": "dynamicDialog", "id": "View", "url": "/view", "button": "View", "title": "View it" },
    {
      "t": "form", "id": "F", "name": "F", "transitionUrl": "/dyn/save",
      "c": [
        { "t": "field", "id": "F.party", "name": "partyId", "title": "Party",
          "w": { "kind": "dropDown", "value": "P1", "options": [{ "value": "P1", "label": "Acme" }],
                 "optionsUrl": "/dyn/parties", "serverSearch": true, "minLength": 2, "allowEmpty": true } },
        { "t": "field", "id": "F.country", "name": "countryGeoId", "title": "Country", "w": { "kind": "textLine", "value": "" } },
        { "t": "field", "id": "F.countryName", "name": "countryName", "title": "Country name",
          "w": { "kind": "display", "text": "", "value": "", "valueUrl": "/dyn/countryName", "dependsOn": { "geoId": "countryGeoId" } } },
        { "t": "field", "id": "F.currency", "name": "currency", "title": "Currency",
          "w": { "kind": "textLine", "value": "", "defaultUrl": "/dyn/currency", "dependsOn": { "geoId": "countryGeoId" } } },
        { "t": "field", "id": "F.note", "name": "note", "title": "Note",
          "w": { "kind": "editable", "text": "Old note", "url": "/dyn/note", "parameters": { "noteId": "N1" },
                 "parameterName": "value", "widgetType": "text" } }
      ]
    }
  ]
}"#;

const PART_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/part" },
  "c": [
    { "t": "form", "id": "PF", "name": "PF", "transitionUrl": "/part/save", "backgroundReloadId": "Part",
      "c": [ { "t": "field", "id": "PF.qty", "name": "qty", "title": "Qty", "w": { "kind": "textLine", "value": "1" } } ] }
  ]
}"#;

const VIEW_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/view" },
  "c": [ { "t": "text", "id": "t", "text": "Viewed" }, { "t": "link", "id": "l", "text": "Back home", "url": "/dyn" } ]
}"#;

const LISTS_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/lists", "title": "Lists" },
  "c": [
    { "t": "formList", "id": "Rows", "name": "Rows", "transitionUrl": "/lists/update",
      "hidden": [{ "name": "moquiFormName", "value": "Rows" }],
      "columns": [{ "title": "Item" }, { "title": "Quantity" }, { "title": "" }],
      "rows": [
        { "id": "R1", "cells": [ { "t": "link", "id": "Rows.item.R1", "text": "Item one", "url": "/item?itemId=R1" },
            { "t": "field", "id": "Rows.qty.R1", "name": "qty", "title": "", "w": { "kind": "textLine", "value": "1" } },
            { "t": "field", "id": "Rows.update.R1", "name": "update", "title": "Update", "w": { "kind": "submit", "text": "Update" } } ],
          "hidden": [{ "name": "itemId", "value": "R1" }] },
        { "id": "R2", "cells": [ "Item two",
            [ { "t": "field", "id": "Rows.qty.R2", "name": "qty", "title": "", "w": { "kind": "textLine", "value": "2" } }, "each" ],
            { "t": "field", "id": "Rows.update.R2", "name": "update", "title": "Update", "w": { "kind": "submit", "text": "Update" } } ],
          "hidden": [{ "name": "itemId", "value": "R2" }] }
      ] },
    { "t": "formList", "id": "Multi", "name": "Multi", "transitionUrl": "/lists/multi", "multi": true,
      "hidden": [{ "name": "_isMulti", "value": "true" }, { "name": "moquiFormName", "value": "Multi" }],
      "columns": [{ "title": "Item" }, { "title": "Quantity" }],
      "rows": [
        { "id": "0", "cells": [ "A", { "t": "field", "id": "Multi.qty.0", "name": "qty_0", "title": "", "w": { "kind": "textLine", "value": "2" } } ],
          "hidden": [{ "name": "itemId_0", "value": "A" }] },
        { "id": "1", "cells": [ "B", { "t": "field", "id": "Multi.qty.1", "name": "qty_1", "title": "", "w": { "kind": "textLine", "value": "3" } } ],
          "hidden": [{ "name": "itemId_1", "value": "B" }] }
      ],
      "buttons": [ { "t": "field", "id": "Multi.save", "name": "save", "title": "Save all", "w": { "kind": "submit", "text": "Save all" } } ] },
    { "t": "formList", "id": "Sel", "name": "Sel",
      "rowSelection": { "idField": "itemId", "parameter": "itemIds", "listMode": "true" },
      "columns": [{ "title": "Item" }],
      "rows": [ { "id": "S1", "selectId": "S1", "cells": ["one"] }, { "id": "S2", "selectId": "S2", "cells": ["two"] } ] },
    { "t": "dialog", "id": "Approve", "title": "Approve", "button": "Approve", "list": "Sel",
      "c": [ { "t": "form", "id": "ApproveForm", "name": "ApproveForm", "transitionUrl": "/lists/approve",
        "c": [ { "t": "field", "id": "ApproveForm.note", "name": "note", "title": "Note", "w": { "kind": "textLine", "value": "" } },
               { "t": "field", "id": "ApproveForm.go", "name": "go", "title": "Approve", "w": { "kind": "submit", "text": "Approve" } } ] } ] }
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

fn focus(session: &mut Session, link: &FakeLink, key: &str) {
    for _ in 0..50 {
        if session.focus_id() == key {
            return;
        }
        press(session, link, Key::Tab);
    }
    panic!("no focus on {key}: {:?}", session.focus_order());
}

fn calls(link: &FakeLink) -> Vec<LinkCall> {
    link.calls()
}

fn transitions(link: &FakeLink) -> Vec<(String, BTreeMap<String, String>)> {
    calls(link)
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Transition { url, fields, .. } => Some((url, fields)),
            _ => None,
        })
        .collect()
}

fn started(path: &str, files: &[&str]) -> (Session, FakeLink) {
    let link = FakeLink::from_json(files).unwrap();
    let mut session = Session::start(path);
    session.run_now(&link);
    (session, link)
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_container_loads_with_the_screen_and_a_save_reloads_only_it() {
    let (mut session, link) = started("/dyn", &[DYN_JSON, PART_JSON, VIEW_JSON]);
    let fragments = |link: &FakeLink| {
        calls(link)
            .iter()
            .filter(|call| matches!(call, LinkCall::Fragment { url, .. } if url == "/part"))
            .count()
    };
    assert_eq!(fragments(&link), 1);
    // The body's ids are the container's: they cannot meet the screen's.
    assert!(
        session.index().fields.contains_key("Part/PF.qty"),
        "{:?}",
        session.index().fields.keys()
    );
    focus(&mut session, &link, "Part/PF.qty");
    typed(&mut session, &link, "2");
    press(&mut session, &link, Key::Enter);
    assert_eq!(transitions(&link).last().map(|t| t.0.as_str()), Some("/part/save"));
    // The form names the container in its background-reload-id: it loads again, and the screen does not.
    let screens = calls(&link)
        .iter()
        .filter(|call| matches!(call, LinkCall::Level { .. }))
        .count();
    assert_eq!(fragments(&link), 2);
    assert_eq!(screens, 1);
}

#[test]
fn a_dynamic_dialog_asks_for_its_body_as_it_opens_and_its_links_take_the_focus() {
    let (mut session, link) = started("/dyn", &[DYN_JSON, PART_JSON, VIEW_JSON]);
    focus(&mut session, &link, "View");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.dialogs_open(), ["View"]);
    assert!(calls(&link)
        .iter()
        .any(|call| matches!(call, LinkCall::Fragment { url, .. } if url == "/view")));
    assert_eq!(session.focus_id(), "View/l");
    assert_eq!(session.index().dialogs.get("View").map(String::as_str), Some("View it"));
}

#[test]
fn a_server_search_waits_for_the_typing_to_stop_and_keeps_the_value_chosen() {
    let (mut session, link) = started("/dyn", &[DYN_JSON, PART_JSON, VIEW_JSON]);
    link.answer_options(
        "/dyn/parties",
        map(&[("term", "be"), ("pageIndex", "0")]),
        vec![Choice {
            value: "P2".to_string(),
            label: "Beta".to_string(),
        }],
    );
    focus(&mut session, &link, "F.party");
    press(&mut session, &link, Key::Enter);
    let options = |link: &FakeLink| {
        calls(link)
            .iter()
            .filter(|call| matches!(call, LinkCall::Options { .. }))
            .count()
    };
    assert_eq!(options(&link), 0, "a search does not load as it opens");
    typed(&mut session, &link, "be");
    assert_eq!(options(&link), 0, "nothing is asked while the typing goes on");
    assert!(session.on_time(Instant::now() + Duration::from_secs(1)));
    session.run_now(&link);
    assert_eq!(options(&link), 1);
    let labels: Vec<String> = session
        .dropdown_choices()
        .into_iter()
        .map(|choice| choice.label)
        .collect();
    assert_eq!(labels, ["Acme", "Beta"]);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.buffer("F.party"), "P2");
}

#[test]
fn a_changed_field_asks_for_the_values_that_depend_on_it() {
    let (mut session, link) = started("/dyn", &[DYN_JSON, PART_JSON, VIEW_JSON]);
    link.answer_json(
        "/dyn/countryName",
        BTreeMap::new(),
        serde_json::json!([{ "value": "USA", "label": "United States" }]),
    );
    link.answer_json("/dyn/currency", BTreeMap::new(), serde_json::json!("USD"));
    focus(&mut session, &link, "F.country");
    typed(&mut session, &link, "USA");
    assert_eq!(session.shown_text("F.countryName"), Some("United States"));
    assert_eq!(session.buffer("F.countryName"), "USA");
    assert_eq!(session.buffer("F.currency"), "USD");
    let asked = calls(&link)
        .into_iter()
        .filter_map(|call| match call {
            LinkCall::Json { url, parameters, .. } if url == "/dyn/currency" => Some(parameters),
            _ => None,
        })
        .next_back();
    assert_eq!(asked, Some(map(&[("geoId", "USA")])));
}

#[test]
fn click_to_edit_posts_the_value_to_its_transition() {
    let (mut session, link) = started("/dyn", &[DYN_JSON, PART_JSON, VIEW_JSON]);
    focus(&mut session, &link, "F.note");
    press(&mut session, &link, Key::Enter);
    assert!(matches!(session.top_overlay(), Some(Overlay::Editor(_))));
    typed(&mut session, &link, "!");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        transitions(&link).last(),
        Some(&(
            "/dyn/note".to_string(),
            map(&[("noteId", "N1"), ("value", "Old note!")])
        ))
    );
    assert!(session.top_overlay().is_none());
}

#[test]
fn a_row_form_posts_its_row_and_the_editor_closes() {
    let (mut session, link) = started("/lists", &[LISTS_JSON]);
    focus(&mut session, &link, "Rows");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.top_overlay(), Some(&Overlay::Row("Rows".to_string(), 0)));
    assert_eq!(session.focus_id(), "Rows.qty.R1");
    typed(&mut session, &link, "5");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        transitions(&link).last(),
        Some(&(
            "/lists/update".to_string(),
            map(&[("itemId", "R1"), ("moquiFormName", "Rows"), ("qty", "15")])
        ))
    );
    assert!(session.row_editing().is_none());
    // Right follows the row's link, as Lynx does.
    press(&mut session, &link, Key::Right);
    assert!(calls(&link)
        .iter()
        .any(|call| matches!(call, LinkCall::Level { path, .. } if path == "/item")));
}

#[test]
fn a_stacked_cell_takes_its_lines_and_its_field_edits_in_the_row() {
    let (mut session, link) = started("/lists", &[LISTS_JSON]);
    focus(&mut session, &link, "Rows");
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Char('e'));
    assert_eq!(session.row_editing(), Some(("Rows", 1)));
    let laid = moqui_tui::layout(session.model(), 100);
    let outline = moqui_tui::outline_of(&laid);
    assert!(outline.contains("list Rows scroll=false rows=2"), "{outline}");
}

#[test]
fn a_multi_list_posts_every_row_once() {
    let (mut session, link) = started("/lists", &[LISTS_JSON]);
    focus(&mut session, &link, "Multi");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.focus_id(), "Multi.qty.0");
    typed(&mut session, &link, "0");
    session.press(&KeyPress::with_ctrl(Key::Char('s')), &link);
    assert_eq!(
        transitions(&link).last(),
        Some(&(
            "/lists/multi".to_string(),
            map(&[
                ("_isMulti", "true"),
                ("itemId_0", "A"),
                ("itemId_1", "B"),
                ("moquiFormName", "Multi"),
                ("qty_0", "20"),
                ("qty_1", "3"),
            ])
        ))
    );
}

#[test]
fn a_row_action_posts_the_selected_rows_and_waits_for_a_selection() {
    let (mut session, link) = started("/lists", &[LISTS_JSON]);
    link.answer_transition("/lists/approve", TransitionOutcome::saved("Approved"));
    focus(&mut session, &link, "Sel");
    press(&mut session, &link, Key::F(10));
    let Some(Overlay::Menu(menu)) = session.top_overlay() else {
        panic!("no actions menu: {:?}", session.overlays());
    };
    assert_eq!(menu.items[0].label, "Approve…");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.dialogs_open(), ["Approve"]);
    typed(&mut session, &link, "ok");
    press(&mut session, &link, Key::Enter);
    assert!(transitions(&link).is_empty());
    assert!(session.message().contains("Select the rows first"));
    press(&mut session, &link, Key::Escape);
    focus(&mut session, &link, "Sel");
    press(&mut session, &link, Key::Char(' '));
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Char(' '));
    focus(&mut session, &link, "Approve");
    press(&mut session, &link, Key::Enter);
    focus(&mut session, &link, "ApproveForm.go");
    press(&mut session, &link, Key::Enter);
    assert_eq!(
        transitions(&link).last(),
        Some(&(
            "/lists/approve".to_string(),
            map(&[("itemIds", "S1,S2"), ("note", "ok")])
        ))
    );
    assert!(session.selected("Sel").is_none_or(|set| set.is_empty()));
}
