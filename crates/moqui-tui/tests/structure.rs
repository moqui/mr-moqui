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

//! The structure of a screen with state of its own: a tree, a collapsible box, a wizard, a status bar, a button menu,
//! components, the references of html and markdown, a number card and an image. No Revue.

use std::collections::BTreeMap;

use moqui_tui::{layout_levels, outline_of, Breakpoints, FakeLink, Key, KeyPress, LinkCall, Overlay, Session};

const TREE_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/tree", "title": "Tree" },
  "c": [
    { "t": "tree", "id": "Accounts", "url": "/tree/actions/Accounts", "openPath": "", "parameters": { "orgId": "O1" } },
    { "t": "box", "id": "More", "title": "More", "initial": "closed",
      "c": [ { "t": "field", "id": "F.hidden", "name": "hidden", "title": "Hidden", "w": { "kind": "textLine", "value": "" } } ] },
    { "t": "buttonMenu", "id": "Actions", "text": "Actions",
      "c": [ { "t": "link", "id": "a1", "text": "Approve", "url": "/tree/approve", "kind": "hidden-form-link", "method": "post" },
             { "t": "link", "id": "a2", "text": "Print", "url": "/print" },
             { "t": "dialog", "id": "AddThing", "title": "Add Thing",
               "c": [ { "t": "field", "id": "AddThing.name", "name": "name", "title": "Name", "w": { "kind": "textLine", "value": "" } } ] } ] },
    { "t": "statusBar", "id": "Status", "text": "Approved",
      "stages": [ { "statusId": "P", "label": "Placed", "state": "done" }, { "statusId": "A", "label": "Approved", "state": "current" },
                  { "statusId": "S", "label": "Shipped", "state": "todo" } ],
      "c": [ { "t": "link", "id": "Status.S", "text": "Ship", "url": "/tree/setStatus?statusId=S", "method": "post",
               "kind": "hidden-form-link", "btnType": "primary", "confirm": "Ship?" } ] },
    { "t": "html", "id": "Doc", "blocks": [ { "kind": "p", "text": "See the guide[1]." } ],
      "links": [ { "text": "the guide", "url": "https://docs.example/guide" } ] },
    { "t": "markdown", "id": "Md", "text": "Read [this](/apps/wiki/Page)." },
    { "t": "numberCard", "id": "Open", "title": "Open orders", "value": "12", "url": "/orders?status=open" },
    { "t": "image", "id": "Logo", "alt": "Logo", "url": "https://example.com/logo.png" },
    { "t": "component", "id": "Pipeline", "name": "board", "base": "/tree",
      "props": { "source": "boardRows", "groups": "stages", "groupOf": "stage", "move": "moveCard", "moveGroup": "stage" } },
    { "t": "component", "id": "Mystery", "name": "inbox", "props": {} }
  ]
}"#;

const WIZARD_JSON: &str = r#"{
  "v": 1,
  "screen": { "path": "/wiz/Options", "title": "Options" },
  "c": [
    { "t": "subscreens", "id": "Steps", "kind": "wizard", "finishText": "Finish",
      "items": [ { "title": "Start", "url": "/wiz/Start" }, { "title": "Options", "url": "/wiz/Options", "active": true },
                 { "title": "Confirm", "url": "/wiz/Confirm" } ],
      "c": [ { "t": "form", "id": "Opt", "name": "Opt", "transitionUrl": "/wiz/Options/save",
        "c": [ { "t": "field", "id": "Opt.size", "name": "size", "title": "Size", "w": { "kind": "textLine", "value": "" } } ] } ] },
    { "t": "subscreens", "id": "Trail", "kind": "stack",
      "items": [ { "title": "Orders", "url": "/o" }, { "title": "SO-1", "url": "/o/SO-1", "active": true } ] }
  ]
}"#;

fn press(session: &mut Session, link: &FakeLink, key: Key) -> bool {
    session.press(&KeyPress::plain(key), link)
}

fn focus(session: &mut Session, link: &FakeLink, key: &str) {
    for _ in 0..60 {
        if session.focus_id() == key {
            return;
        }
        press(session, link, Key::Tab);
    }
    panic!("no focus on {key}: {:?}", session.focus_order());
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn tree_session() -> (Session, FakeLink) {
    let link = FakeLink::from_json(&[TREE_JSON]).unwrap();
    link.answer_json(
        "/tree/actions/Accounts",
        map(&[("orgId", "O1"), ("treeNodeId", "#"), ("treeOpenPath", "")]),
        serde_json::json!([
            { "id": "1000", "text": "Assets", "children": true, "li_attr": { "treeNodeName": "account" } },
            { "id": "2000", "text": "Liabilities", "children": false, "a_attr": { "urlText": "/gl?id=2000" } }
        ]),
    );
    link.answer_json(
        "/tree/actions/Accounts",
        map(&[("orgId", "O1"), ("treeNodeId", "1000"), ("treeNodeName", "account"), ("treeOpenPath", "")]),
        serde_json::json!([{ "id": "1100", "text": "Cash", "children": false, "a_attr": { "urlText": "/gl?id=1100" } }]),
    );
    link.answer_json(
        "/tree/boardRows",
        BTreeMap::new(),
        serde_json::json!({ "stages": [{ "id": "q", "name": "Qualify" }, { "id": "w", "name": "Won" }],
            "rows": [{ "id": "O1", "title": "Acme", "stage": "q" }] }),
    );
    let mut session = Session::start("/tree");
    session.run_now(&link);
    (session, link)
}

#[test]
fn a_tree_loads_its_roots_opens_a_node_by_its_children_and_follows_a_leaf() {
    let (mut session, link) = tree_session();
    let rows = |session: &Session| -> Vec<String> {
        session
            .tree("Accounts")
            .map(|tree| {
                tree.rows()
                    .into_iter()
                    .map(|row| format!("{}{}", "  ".repeat(row.depth), row.text))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(rows(&session), ["Assets", "Liabilities"]);
    focus(&mut session, &link, "Accounts");
    press(&mut session, &link, Key::Right);
    assert_eq!(rows(&session), ["Assets", "  Cash", "Liabilities"]);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Left);
    assert_eq!(
        session.tree("Accounts").unwrap().at,
        0,
        "Left on a leaf goes to its parent"
    );
    press(&mut session, &link, Key::Left);
    assert_eq!(rows(&session), ["Assets", "Liabilities"]);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Enter);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Level { path, parameters } if path == "/gl" && parameters.get("id").map(String::as_str) == Some("2000"))));
}

#[test]
fn a_closed_box_keeps_its_fields_out_of_the_focus_until_enter_opens_it() {
    let (mut session, link) = tree_session();
    assert!(!session.focus_order().iter().any(|item| item.key == "F.hidden"));
    let outline = outline_of(&layout_levels(
        session.nav().models(),
        100,
        &Breakpoints::default(),
        &session,
    ));
    assert!(outline.contains("box More w=100 closed"), "{outline}");
    focus(&mut session, &link, "More");
    press(&mut session, &link, Key::Enter);
    assert!(session.focus_order().iter().any(|item| item.key == "F.hidden"));
    assert!(!session.is_collapsed("More"));
}

#[test]
fn a_button_menu_opens_its_items_and_one_posts() {
    let (mut session, link) = tree_session();
    focus(&mut session, &link, "Actions");
    press(&mut session, &link, Key::Enter);
    let Some(Overlay::Menu(menu)) = session.top_overlay() else {
        panic!("{:?}", session.overlays());
    };
    let labels: Vec<&str> = menu.items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, ["Approve", "Print", "Add Thing"]);
    press(&mut session, &link, Key::Enter);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Transition { url, .. } if url == "/tree/approve")));
}

#[test]
fn a_dialog_among_a_button_menus_items_opens_from_the_menu_and_is_no_stop_of_its_own() {
    let (mut session, link) = tree_session();
    assert!(
        !session.focus_order().iter().any(|item| item.key == "AddThing"),
        "{:?}",
        session.focus_order()
    );
    focus(&mut session, &link, "Actions");
    press(&mut session, &link, Key::Enter);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Down);
    press(&mut session, &link, Key::Enter);
    assert!(
        matches!(session.top_overlay(), Some(Overlay::Dialog(id)) if id == "AddThing"),
        "{:?}",
        session.overlays()
    );
    assert_eq!(session.focus_id(), "AddThing.name");
}

#[test]
fn a_status_bar_shows_its_stages_and_its_transitions_ask_first() {
    let (mut session, link) = tree_session();
    let outline = outline_of(&layout_levels(
        session.nav().models(),
        100,
        &Breakpoints::default(),
        &session,
    ));
    assert!(
        outline.contains("status Approved Placed:done Approved:current Shipped:todo"),
        "{outline}"
    );
    assert!(outline.contains("inline Status.S"), "{outline}");
    focus(&mut session, &link, "Status.S");
    press(&mut session, &link, Key::Enter);
    assert!(matches!(session.top_overlay(), Some(Overlay::Confirm(_))));
    press(&mut session, &link, Key::Char('y'));
    assert!(link.calls().iter().any(|call| matches!(
        call,
        LinkCall::Transition { url, fields, .. } if url == "/tree/setStatus" && fields.get("statusId").map(String::as_str) == Some("S")
    )));
}

#[test]
fn references_of_html_and_markdown_a_card_and_an_image_are_links() {
    let (mut session, link) = tree_session();
    let keys: Vec<&str> = session.focus_order().iter().map(|item| item.key.as_str()).collect();
    for key in ["Doc#ref1", "Md#ref1", "Open", "Logo"] {
        assert!(keys.contains(&key), "{key} in {keys:?}");
    }
    focus(&mut session, &link, "Md#ref1");
    press(&mut session, &link, Key::Enter);
    assert!(link
        .calls()
        .iter()
        .any(|call| matches!(call, LinkCall::Level { path, .. } if path == "/apps/wiki/Page")));
    let (mut session, link) = tree_session();
    focus(&mut session, &link, "Logo");
    press(&mut session, &link, Key::Enter);
    assert_eq!(session.take_open().as_deref(), Some("https://example.com/logo.png"));
}

#[test]
fn a_component_gets_its_data_from_its_screen_and_posts_a_move() {
    let (mut session, link) = tree_session();
    assert!(link.calls().iter().any(|call| matches!(
        call,
        LinkCall::Json { url, .. } if url == "/tree/boardRows"
    )));
    let lines = session.component_state("Pipeline").and_then(|state| state.data.clone());
    assert!(lines.is_some());
    let outline = outline_of(&layout_levels(
        session.nav().models(),
        100,
        &Breakpoints::default(),
        &session,
    ));
    assert!(outline.contains("line Qualify · 1"), "{outline}");
    assert!(outline.contains("placeholder The inbox component"), "{outline}");
    focus(&mut session, &link, "Pipeline");
    press(&mut session, &link, Key::Char('m'));
    press(&mut session, &link, Key::Right);
    press(&mut session, &link, Key::Enter);
    assert!(link.calls().iter().any(|call| matches!(
        call,
        LinkCall::Transition { url, fields, .. } if url == "/tree/moveCard" && fields == &map(&[("id", "O1"), ("stage", "w")])
    )));
}

#[test]
fn a_wizard_saves_its_step_before_next_and_a_stack_shows_its_trail() {
    let link = FakeLink::from_json(&[WIZARD_JSON]).unwrap();
    let mut session = Session::start("/wiz/Options");
    session.run_now(&link);
    let outline = outline_of(&layout_levels(
        session.nav().models(),
        100,
        &Breakpoints::default(),
        &session,
    ));
    assert!(
        outline.contains("status Step 2 of 3 · Options Start:done Options:current Confirm:todo"),
        "{outline}"
    );
    assert!(outline.contains("inline Steps#back Steps#next"), "{outline}");
    assert!(outline.contains("text Orders ▸ SO-1"), "{outline}");
    focus(&mut session, &link, "Opt.size");
    press(&mut session, &link, Key::Char('L'));
    // PgDn on a wizard with no list is Next: the step's form is saved, then the next step follows.
    press(&mut session, &link, Key::PageDown);
    let calls = link.calls();
    let saved = calls
        .iter()
        .position(|call| matches!(call, LinkCall::Transition { url, .. } if url == "/wiz/Options/save"));
    let next = calls
        .iter()
        .position(|call| matches!(call, LinkCall::Level { path, .. } if path == "/wiz/Confirm"));
    assert!(saved.is_some() && next.is_some() && saved < next, "{calls:?}");
}

/// A Markdown text area: Alt+B, Alt+I and Alt+K wrap the word at the cursor in its source, which stays the value.
#[test]
fn alt_keys_wrap_a_word_of_markdown_source() {
    let json = r#"{ "v": 1, "screen": { "path": "/md", "title": "Md" }, "c": [
        { "t": "form", "id": "Note", "name": "Note", "transitionUrl": "/md/save", "c": [
          { "t": "field", "id": "Note.body", "name": "body", "title": "Body",
            "w": { "kind": "textArea", "value": "make it bold", "rows": 4, "editorType": "md" } } ] } ] }"#;
    let link = FakeLink::from_json(&[json]).unwrap();
    let mut session = Session::start("/md");
    session.run_now(&link);
    focus(&mut session, &link, "Note.body");
    assert!(session.markdown_field("Note.body"));
    for _ in 0..12 {
        press(&mut session, &link, Key::Right);
    }
    session.press(&KeyPress::with_alt(Key::Char('b')), &link);
    assert_eq!(session.buffer("Note.body"), "make it **bold**");
    session.press(&KeyPress::plain(Key::Char(' ')), &link);
    session.press(&KeyPress::with_alt(Key::Char('k')), &link);
    assert_eq!(session.buffer("Note.body"), "make it **bold** []()");
}

#[test]
fn report_tail_timer_pauses_does_not_block_settle_and_exports_real_bytes() {
    use moqui_tui::Timer;
    let json = r#"{"v":1,"screen":{"path":"/monitor","title":"Monitor"},"c":[
        {"t":"component","id":"Tail","name":"log-tail","props":{"source":"/monitor/tail"}},
        {"t":"component","id":"Dashboard","name":"report-dashboard","props":{"dashboard":{"definitionId":"test-dashboard","variableValues":{},"absoluteTime":{"from":"2026-01-01T00:00:00Z","thru":"2026-01-02T00:00:00Z"},"panels":{"first":{"title":"First"}}},"frames":{}}}] }"#;
    let link = FakeLink::from_json(&[json]).unwrap();
    link.answer_json(
        "/monitor/tail",
        BTreeMap::new(),
        serde_json::json!({"hits":[{"documentId":"otel_logs","id":"first"}],"nextCursor":{"id":1}}),
    );
    link.answer_json(
        "/monitor/tail",
        map(&[("cursor", "{\"id\":1}")]),
        serde_json::json!({"hits":[{"documentId":"otel_logs","id":"second"}],"nextCursor":{"id":2}}),
    );
    link.answer_json(
        "/monitor/tail",
        map(&[("cursor", "{\"id\":2}")]),
        serde_json::json!({"hits":[{"documentId":"otel_logs","id":"third"}],"nextCursor":{"id":3}}),
    );
    link.answer_json("/apps/dashboards/Dashboard/exportPanel",map(&[("definitionId","test-dashboard"),("panelId","first"),("renderMode","xlsx"),("from","2026-01-01T00:00:00Z"),("thru","2026-01-02T00:00:00Z"),("variables","{}")]),serde_json::json!({"filename":"first.xlsx","contentType":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet","contentBase64":"UEsDBA=="}));
    let mut session = Session::start("/monitor");
    session.run_now(&link);
    focus(&mut session, &link, "Tail");
    press(&mut session, &link, Key::Char('p'));
    let deadline = session
        .timer(&Timer::Component("Tail".into()))
        .expect("opt-in tail timer");
    assert!(
        !session.fire_waits(),
        "agent settling never consumes continuous tail timers"
    );
    session.on_time(deadline);
    session.run_now(&link);
    assert_eq!(
        session.component_state("Tail").unwrap().data.as_ref().unwrap()["hits"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    press(&mut session, &link, Key::Char('p'));
    assert!(session.timer(&Timer::Component("Tail".into())).is_none());
    focus(&mut session, &link, "Dashboard");
    press(&mut session, &link, Key::Char('x'));
    let downloads = session.take_downloads();
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].bytes, vec![80, 75, 3, 4]);
    assert_eq!(downloads[0].file_name, "first.xlsx");
}

#[test]
fn report_structured_editors_update_normal_form_buffers_without_posting() {
    let screen = r#"{"v":1,"screen":{"path":"/editor","title":"Editor"},"c":[{"t":"form","id":"EditDefinition","name":"EditDefinition","transitionUrl":"/editor/save","c":[{"t":"field","id":"EditDefinition.definitionJson","name":"definitionJson","w":{"kind":"textArea","value":"{}"}},{"t":"component","id":"Editor","name":"report-definition-editor","props":{"form":"EditDefinition","field":"definitionJson","kind":"dashboard","value":{}}}]}]}"#;
    let link = FakeLink::from_json(&[screen]).unwrap();
    let mut session = Session::start("/editor");
    session.run_now(&link);
    focus(&mut session, &link, "Editor");
    press(&mut session, &link, Key::Char('a'));
    press(&mut session, &link, Key::Enter);
    let body: serde_json::Value = serde_json::from_str(session.buffer("EditDefinition.definitionJson")).unwrap();
    assert!(body.get("layout").is_some());
    assert!(session.form_dirty("EditDefinition"));
    assert!(
        session.take_requests().is_empty(),
        "authoring changes buffers, not persistent data"
    );
    session.set_buffer(
        "EditDefinition.definitionJson",
        r#"{"custom":"manual edit","panels":{}}"#.into(),
    );
    press(&mut session, &link, Key::Char('a'));
    press(&mut session, &link, Key::Enter);
    let body: serde_json::Value = serde_json::from_str(session.buffer("EditDefinition.definitionJson")).unwrap();
    assert_eq!(
        body["custom"], "manual edit",
        "returning to structured controls preserves manual JSON edits"
    );
    assert!(body.get("layout").is_some());
}

#[test]
fn report_playlist_timer_loads_next_authorized_dashboard_and_pauses() {
    use moqui_tui::Timer;
    let screen = r#"{"v":1,"screen":{"path":"/playlist","title":"Playlist"},"c":[{"t":"component","id":"Playlist","name":"report-playlist","props":{"source":"data","definitionId":"list","from":"now-1h","thru":"now","seconds":5,"items":[{"title":"One"},{"title":"Two"}]}}]}"#;
    let link = FakeLink::from_json(&[screen]).unwrap();
    for (index, title) in [("0", "First dashboard"), ("1", "Second dashboard")] {
        link.answer_json(
            "/playlist/data",
            map(&[
                ("definitionId", "list"),
                ("from", "now-1h"),
                ("thru", "now"),
                ("index", index),
            ]),
            serde_json::json!({"dashboard":{"definitionId":index,"panels":{"p":{"title":title}}},"frames":{}}),
        );
    }
    let mut session = Session::start("/playlist");
    session.run_now(&link);
    focus(&mut session, &link, "Playlist");
    let deadline = session
        .timer(&Timer::Component("Playlist".into()))
        .expect("playlist deadline");
    assert!(!session.fire_waits(), "settling never advances scheduled playlists");
    session.on_time(deadline);
    session.run_now(&link);
    assert_eq!(
        session.component_state("Playlist").unwrap().data.as_ref().unwrap()["dashboard"]["definitionId"],
        "1"
    );
    press(&mut session, &link, Key::Char('p'));
    assert!(session.timer(&Timer::Component("Playlist".into())).is_none());
}
