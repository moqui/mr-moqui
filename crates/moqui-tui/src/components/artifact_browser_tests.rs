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

use serde_json::json;

use super::*;
use crate::components::line_text;

fn props() -> Value {
    json!({"source": "browse", "artifactId": "Example"})
}

fn row(id: &str, kind: &str, short: &str) -> Value {
    json!({"id": id, "kind": kind, "shortName": short, "name": id, "how": "entity-find", "calls": 0,
        "observedOnly": false, "resolved": true})
}

/// An entity used by two screens and a service, and using one entity and a field it cannot resolve.
fn answer(center_id: &str) -> Value {
    json!({
        "center": {"id": center_id, "kind": "entity", "shortName": "Example", "name": "moqui.example.Example",
            "lines": ["moqui.example.Example", "example · ExampleEntities.xml:12"],
            "links": [{"key": "t", "label": "Text", "url": "/qapps/dev/Workspace?rootId=r&path=p&line=12"},
                {"key": "d", "label": "Data", "url": "/qapps/dev/Entities/Edit?artifact=entity%3Amoqui.example.Example"},
                {"key": "D", "label": "Doc", "url": "/qapps/dev/Artifacts/Doc?artifactId=entity%3Amoqui.example.Example"}]},
        "usedBy": [
            {"kind": "screen", "count": 2, "rows": [row("screen:A", "screen", "ExampleDetail"), row("screen:B", "screen", "FindExample")]},
            {"kind": "service", "count": 1, "rows": [row("service:x.S", "service", "update#Example")]}
        ],
        "uses": [
            {"kind": "entity", "count": 2, "rows": [row("entity:moqui.basic.Enumeration", "entity", "Enumeration"),
                {"id": "", "kind": "entity", "shortName": "NoSuch", "name": "no.Such", "resolved": false}]}
        ],
        "kinds": ["entity", "screen", "service"],
        "filter": {"kinds": [], "hideFramework": true, "observed": true}
    })
}

fn loaded() -> ComponentState {
    let mut state = ComponentState::default();
    ArtifactBrowser.receive(&props(), &mut state, answer("entity:moqui.example.Example"));
    state
}

fn press(state: &mut ComponentState, key: Key) -> Option<Vec<Act>> {
    ArtifactBrowser.key(&props(), state, &KeyPress::plain(key), false)
}

fn called(acts: Option<Vec<Act>>) -> BTreeMap<String, String> {
    match acts.as_deref() {
        Some([Act::Call { url, parameters }]) if url == "browse" => parameters.clone(),
        other => panic!("not a call of browse: {other:?}"),
    }
}

fn text(state: &ComponentState, width: u16) -> Vec<String> {
    ArtifactBrowser
        .lines(&props(), state, width, false)
        .iter()
        .map(line_text)
        .collect()
}

#[test]
fn the_first_read_asks_for_the_props_center_and_the_trail_takes_its_id() {
    let state = ComponentState::default();
    assert_eq!(
        ArtifactBrowser.source(&props(), &state),
        Some((
            "browse".to_string(),
            BTreeMap::from([("artifactId".to_string(), "Example".to_string())])
        ))
    );
    let state = loaded();
    assert_eq!(View::of(&state).trail, ["entity:moqui.example.Example"]);
    // with nothing chosen there is nothing to read
    assert_eq!(ArtifactBrowser.source(&json!({}), &ComponentState::default()), None);
}

#[test]
fn enter_recenters_and_the_trail_goes_back_and_forward() {
    let mut state = loaded();
    // the first side with rows has the focus: Down to FindExample, Enter makes it the center
    assert_eq!(press(&mut state, Key::Down), Some(Vec::new()));
    let asked = called(press(&mut state, Key::Enter));
    assert_eq!(asked["artifactId"], "screen:B");
    ArtifactBrowser.receive(&props(), &mut state, answer("screen:B"));
    assert_eq!(View::of(&state).trail, ["entity:moqui.example.Example", "screen:B"]);
    assert_eq!(View::of(&state).cursor, [0, 0]);
    // back to the entity, then forward again
    assert_eq!(
        called(press(&mut state, Key::Backspace))["artifactId"],
        "entity:moqui.example.Example"
    );
    assert_eq!(View::of(&state).at, 0);
    assert_eq!(called(press(&mut state, Key::Char(']')))["artifactId"], "screen:B");
    assert_eq!(
        press(&mut state, Key::Char(']')),
        Some(vec![Act::Message("The end of the trail".to_string())])
    );
    // a new center after going back drops what was ahead of it
    press(&mut state, Key::Backspace);
    press(&mut state, Key::Right);
    press(&mut state, Key::Right);
    called(press(&mut state, Key::Enter));
    assert_eq!(
        View::of(&state).trail,
        ["entity:moqui.example.Example", "entity:moqui.basic.Enumeration"]
    );
}

#[test]
fn rows_on_the_trail_are_marked_and_an_unresolved_row_does_not_recenter() {
    let mut state = loaded();
    let mut view = View::of(&state);
    view.trail.push("screen:A".to_string());
    view.save(&mut state);
    let lines = text(&state, 120);
    assert!(lines.iter().any(|line| line.contains("▸↺ExampleDetail")), "{lines:#?}");
    assert!(lines.iter().any(|line| line.contains("  FindExample")), "{lines:#?}");
    // the Uses column, its second row is not in the index
    press(&mut state, Key::Right);
    press(&mut state, Key::Right);
    press(&mut state, Key::Down);
    assert_eq!(
        press(&mut state, Key::Enter),
        Some(vec![Act::Message("no.Such is not in the index".to_string())])
    );
}

#[test]
fn k_cycles_the_kinds_and_h_and_x_toggle_the_filter() {
    let mut state = loaded();
    assert_eq!(called(press(&mut state, Key::Char('k')))["kinds"], "entity");
    assert_eq!(called(press(&mut state, Key::Char('k')))["kinds"], "screen");
    assert_eq!(called(press(&mut state, Key::Char('k')))["kinds"], "service");
    // after the last kind, all of them again: no kinds parameter
    assert!(!called(press(&mut state, Key::Char('k'))).contains_key("kinds"));
    // the answer hid the framework; h shows it, and again hides it
    assert_eq!(called(press(&mut state, Key::Char('h')))["hideFramework"], "N");
    assert_eq!(called(press(&mut state, Key::Char('h')))["hideFramework"], "Y");
    let asked = called(press(&mut state, Key::Char('x')));
    assert_eq!(asked["observed"], "N");
    assert_eq!(asked["artifactId"], "entity:moqui.example.Example");
    // the filter stays on the reads that follow, the screen's refresh included
    let (_, parameters) = ArtifactBrowser.source(&props(), &state).unwrap();
    assert_eq!(parameters["observed"], "N");
    assert_eq!(parameters["hideFramework"], "Y");
    assert!(text(&state, 120)[0].contains("Framework: hidden · Observed: hidden"));
}

#[test]
fn the_centers_links_have_their_keys() {
    let mut state = loaded();
    assert_eq!(
        press(&mut state, Key::Char('t')),
        Some(vec![Act::Follow(
            "/qapps/dev/Workspace?rootId=r&path=p&line=12".to_string()
        )])
    );
    assert_eq!(
        press(&mut state, Key::Char('D')),
        Some(vec![Act::Follow(
            "/qapps/dev/Artifacts/Doc?artifactId=entity%3Amoqui.example.Example".to_string()
        )])
    );
    // a letter that is no link is the screen's
    assert_eq!(press(&mut state, Key::Char('y')), None);
    let lines = text(&state, 120);
    assert!(
        lines.iter().any(|line| line.contains("[t] Text  [d] Data")),
        "{lines:#?}"
    );
}

#[test]
fn narrow_the_panes_stack_and_left_and_right_change_the_one_shown() {
    let mut state = loaded();
    let lines = text(&state, 60);
    assert!(
        lines[1].contains("[ Used by (3) ]") || lines[1].contains(" Used by (3) "),
        "{lines:#?}"
    );
    assert!(lines.iter().any(|line| line.contains("ExampleDetail")), "{lines:#?}");
    assert!(!lines.iter().any(|line| line.contains("Enumeration")), "{lines:#?}");
    press(&mut state, Key::Right);
    let lines = text(&state, 60);
    assert!(lines.iter().any(|line| line.starts_with("▣ entity")), "{lines:#?}");
    assert!(!lines.iter().any(|line| line.contains("ExampleDetail")), "{lines:#?}");
    press(&mut state, Key::Right);
    let lines = text(&state, 60);
    assert!(lines.iter().any(|line| line.contains("Enumeration")), "{lines:#?}");
    // at the last pane Right stays
    assert_eq!(press(&mut state, Key::Right), Some(Vec::new()));
    assert_eq!(View::of(&state).column, Some(USES));
    // wide, all three show side by side
    let wide = text(&state, 120);
    assert!(
        wide.iter()
            .any(|line| line.contains("ExampleDetail") && line.contains("Enumeration")),
        "{wide:#?}"
    );
}

#[test]
fn a_long_side_scrolls_with_its_cursor() {
    let rows: Vec<Value> = (0..30)
        .map(|n| row(&format!("screen:S{n}"), "screen", &format!("Screen{n:02}")))
        .collect();
    let mut value = answer("entity:moqui.example.Example");
    value["usedBy"] = json!([{"kind": "screen", "count": 30, "rows": rows}]);
    let mut state = ComponentState {
        rows: 20,
        ..ComponentState::default()
    };
    ArtifactBrowser.receive(&props(), &mut state, value);
    let lines = text(&state, 120);
    assert!(lines.iter().any(|line| line.contains("▼ 23 more")), "{lines:#?}");
    press(&mut state, Key::End);
    assert_eq!(View::of(&state).cursor[0], 29);
    let lines = text(&state, 120);
    assert!(lines.iter().any(|line| line.contains("▸ Screen29")), "{lines:#?}");
    assert!(!lines.iter().any(|line| line.contains("Screen00")), "{lines:#?}");
    press(&mut state, Key::PageUp);
    assert_eq!(View::of(&state).cursor[0], 22);
    press(&mut state, Key::Home);
    assert_eq!(View::of(&state).cursor[0], 0);
    assert_eq!(View::of(&state).top[0], 0);
    // Up at the first row is the screen's
    assert_eq!(press(&mut state, Key::Up), None);
}

#[test]
fn no_such_center_says_so() {
    let mut state = ComponentState::default();
    ArtifactBrowser.receive(
        &props(),
        &mut state,
        json!({"center": null, "message": "No artifact Nope in the index", "usedBy": [], "uses": []}),
    );
    let lines = text(&state, 120);
    assert!(
        lines.iter().any(|line| line.contains("No artifact Nope in the index")),
        "{lines:#?}"
    );
    assert_eq!(View::of(&state).trail, ["Example"]);
}
