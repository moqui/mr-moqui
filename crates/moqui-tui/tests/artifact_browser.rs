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

//! The `artifact-browser` component through the real draw pipeline and the session: the Miller columns at 120
//! columns (a golden), Enter re-centering through the source, and the stacked panes below 100 columns.
//!
//! `UPDATE_GOLDEN=1` rewrites `tests/snapshots/artifact-browser-*.txt`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use moqui_tui::{AppView, FakeLink, Key, KeyPress, LinkCall, Session};
use serde_json::{json, Value};

const BROWSER_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/dev/Artifacts/BrowseTui", "title": "Browser" },
  "c": [
    { "t": "component", "id": "DevArtifactBrowser", "name": "artifact-browser", "base": "/dev/Artifacts/BrowseTui",
      "props": { "source": "browse", "artifactId": "entity:moqui.example.Example" } }
  ]
}"#;

const SOURCE: &str = "/dev/Artifacts/BrowseTui/browse";

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

fn press(view: &mut AppView, key: Key) -> bool {
    view.on_key(&KeyPress::plain(key))
}

fn row(id: &str, kind: &str, short: &str, calls: i64, observed: bool) -> Value {
    json!({ "id": id, "kind": kind, "shortName": short, "name": id, "how": "entity-find", "calls": calls,
        "observedOnly": observed, "resolved": true })
}

fn answer(id: &str, short: &str, kind: &str) -> Value {
    json!({
        "center": { "id": id, "kind": kind, "shortName": short, "name": id,
            "lines": [id, "example · ExampleEntities.xml:12", "An example entity"],
            "links": [{ "key": "t", "label": "Text", "url": "/qapps/dev/Workspace?rootId=example&path=entity/ExampleEntities.xml&line=12" },
                { "key": "d", "label": "Data", "url": "/qapps/dev/Entities/Edit?artifact=entity%3Amoqui.example.Example" },
                { "key": "r", "label": "Refs", "url": "/qapps/dev/Artifacts/Refs?artifactId=entity%3Amoqui.example.Example" },
                { "key": "D", "label": "Doc", "url": "/qapps/dev/Artifacts/Doc?artifactId=entity%3Amoqui.example.Example" }] },
        "usedBy": [
            { "kind": "screen", "count": 2, "rows": [
                row("screen:component://example/screen/ExampleDetail.xml", "screen", "ExampleDetail", 0, false),
                row("screen:component://example/screen/FindExample.xml", "screen", "FindExample", 4, false)] },
            { "kind": "service", "count": 1, "rows": [
                row("service:moqui.example.ExampleServices.update#Example", "service", "update#Example", 0, true)] }
        ],
        "uses": [
            { "kind": "entity", "count": 1, "rows": [row("entity:moqui.basic.Enumeration", "entity", "Enumeration", 0, false)] },
            { "kind": "field", "count": 2, "rows": [
                row("field:moqui.example.Example.exampleId", "field", "exampleId", 0, false),
                row("field:moqui.example.Example.exampleName", "field", "exampleName", 0, false)] }
        ],
        "kinds": ["entity", "field", "screen", "service"],
        "filter": { "kinds": [], "hideFramework": true, "observed": true }
    })
}

#[test]
fn the_artifact_browser_recenters_and_stacks() {
    let link = FakeLink::from_json(&[BROWSER_JSON]).unwrap();
    let ask = |id: &str| BTreeMap::from([("artifactId".to_string(), id.to_string())]);
    let example = "entity:moqui.example.Example";
    let find = "screen:component://example/screen/FindExample.xml";
    link.answer_json(SOURCE, ask(example), answer(example, "Example", "entity"));
    link.answer_json(SOURCE, ask(find), answer(find, "FindExample", "screen"));
    let mut view = AppView::with_session(Session::start("/dev/Artifacts/BrowseTui"), link.clone());
    view.set_clock("Tue 14:32");
    view.session_mut().resize(120, 40);
    view.run_requests();
    while view.session().focus_id() != "DevArtifactBrowser" {
        press(&mut view, Key::Tab);
    }
    golden("artifact-browser-120x40", &view.screen_text(120, 40));
    // Down to FindExample, Enter makes it the center: a post of the source with its id
    press(&mut view, Key::Down);
    press(&mut view, Key::Enter);
    view.run_requests();
    assert!(
        link.calls()
            .iter()
            .any(|call| matches!(call, LinkCall::Json { url, method, parameters }
            if url == SOURCE && method == "post" && parameters["artifactId"] == find)),
        "{:?}",
        link.calls()
    );
    let screen = view.screen_text(120, 40);
    assert!(screen.contains("▣ screen") && screen.contains("Trail 2/2"), "{screen}");
    // the row that is the center now is on the trail
    assert!(screen.contains("↺FindExample"), "{screen}");
    // narrow, one pane at a time under the line naming them
    view.session_mut().resize(80, 24);
    let narrow = view.screen_text(80, 24);
    assert!(
        narrow.contains(" Used by (3) ") && narrow.contains(" Center "),
        "{narrow}"
    );
    assert!(!narrow.contains("Enumeration"), "{narrow}");
}
