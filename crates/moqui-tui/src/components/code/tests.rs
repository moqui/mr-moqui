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

fn props(text: &str) -> Value {
    json!({"text": text, "language": "xml", "form": "Edit", "field": "body", "rows": 10,
        "check": "check?path=a.xml", "complete": "schema", "parameters": {"rootId": "r1"}})
}

fn press(props: &Value, state: &mut ComponentState, press: KeyPress) -> Vec<Act> {
    Code.key(props, state, &press, false).expect("the editor's key")
}

fn plain(props: &Value, state: &mut ComponentState, key: Key) -> Vec<Act> {
    press(props, state, KeyPress::plain(key))
}

fn ctrl(props: &Value, state: &mut ComponentState, key: Key) -> Vec<Act> {
    press(props, state, KeyPress::with_ctrl(key))
}

fn shift(props: &Value, state: &mut ComponentState, key: Key) -> Vec<Act> {
    press(
        props,
        state,
        KeyPress {
            shift: true,
            ..KeyPress::plain(key)
        },
    )
}

fn typed(props: &Value, state: &mut ComponentState, text: &str) -> Vec<Act> {
    let mut acts = Vec::new();
    for ch in text.chars() {
        acts = plain(props, state, Key::Char(ch));
    }
    acts
}

fn editor(props: &Value, state: &ComponentState) -> Editor {
    Editor::of(props, state)
}

fn text(props: &Value, state: &ComponentState) -> String {
    editor(props, state).text
}

fn shown(props: &Value, state: &ComponentState) -> Vec<String> {
    Code.lines(props, state, 60, false).iter().map(line_text).collect()
}

#[test]
fn typing_edits_the_text_and_sets_the_form_field() {
    let props = props("<a>\n    <b/>\n</a>");
    let mut state = ComponentState::default();
    plain(&props, &mut state, Key::Down);
    plain(&props, &mut state, Key::End);
    // Enter keeps the line's indent
    plain(&props, &mut state, Key::Enter);
    let acts = typed(&props, &mut state, "<c/>");
    assert_eq!(text(&props, &state), "<a>\n    <b/>\n    <c/>\n</a>");
    assert_eq!(
        acts,
        vec![Act::SetFormField {
            form: "Edit".into(),
            field: "body".into(),
            value: "<a>\n    <b/>\n    <c/>\n</a>".into()
        }]
    );
    plain(&props, &mut state, Key::Backspace);
    plain(&props, &mut state, Key::Home);
    plain(&props, &mut state, Key::Delete);
    assert_eq!(text(&props, &state), "<a>\n    <b/>\n   <c/\n</a>");
    let lines = shown(&props, &state);
    let status = lines.last().unwrap();
    assert!(status.starts_with("Ln 3, Col 1  xml  ●"), "{status}");
    // moves set no field
    assert!(plain(&props, &mut state, Key::Up).is_empty());
}

#[test]
fn shift_selects_and_copy_cut_and_paste_take_it() {
    let props = props("alpha beta\ngamma");
    let mut state = ComponentState::default();
    for _ in 0..5 {
        shift(&props, &mut state, Key::Right);
    }
    assert_eq!(editor(&props, &state).selection(), Some((0, 5)));
    assert_eq!(
        ctrl(&props, &mut state, Key::Char('c')),
        vec![Act::Copy("alpha".into())]
    );
    let acts = ctrl(&props, &mut state, Key::Char('x'));
    assert_eq!(acts[0], Act::Copy("alpha".into()));
    assert_eq!(text(&props, &state), " beta\ngamma");
    ctrl(&props, &mut state, Key::End);
    ctrl(&props, &mut state, Key::Char('v'));
    assert_eq!(text(&props, &state), " beta\ngammaalpha");
    // typing over a selection replaces it
    press(
        &props,
        &mut state,
        KeyPress {
            ctrl: true,
            shift: true,
            ..KeyPress::plain(Key::Left)
        },
    );
    typed(&props, &mut state, "z");
    assert_eq!(text(&props, &state), " beta\nz");
    // the terminal's paste, its line ends made one
    let acts = Code.paste(&props, &mut state, "1\r\n2").unwrap();
    assert_eq!(text(&props, &state), " beta\nz1\n2");
    assert!(matches!(&acts[..], [Act::SetFormField { value, .. }] if value == " beta\nz1\n2"));
    // nothing selected: the line is copied
    plain(&props, &mut state, Key::Up);
    assert_eq!(ctrl(&props, &mut state, Key::Char('c')), vec![Act::Copy("z1\n".into())]);
}

#[test]
fn words_pages_and_the_ends_move_the_cursor() {
    let text: String = (1..=30).map(|n| format!("line {n}.x\n")).collect();
    let props = props(&text);
    let mut state = ComponentState::default();
    ctrl(&props, &mut state, Key::Right);
    assert_eq!(editor(&props, &state).cursor, 4);
    ctrl(&props, &mut state, Key::Right);
    assert_eq!(editor(&props, &state).cursor, 6);
    ctrl(&props, &mut state, Key::Right);
    assert_eq!(editor(&props, &state).cursor, 7);
    ctrl(&props, &mut state, Key::Left);
    assert_eq!(editor(&props, &state).cursor, 6);
    plain(&props, &mut state, Key::PageDown);
    assert_eq!(editor(&props, &state).row_col(), (10, 6));
    ctrl(&props, &mut state, Key::End);
    let at_end = editor(&props, &state);
    assert_eq!(at_end.row_col(), (30, 0));
    // the view follows the cursor
    assert_eq!(at_end.top, 21);
    assert!(shown(&props, &state)[9].trim_start().starts_with("31"));
    ctrl(&props, &mut state, Key::Home);
    assert_eq!((editor(&props, &state).cursor, editor(&props, &state).top), (0, 0));
}

#[test]
fn undo_takes_back_typed_words_whole_and_redo_puts_them_again() {
    let props = props("x");
    let mut state = ComponentState::default();
    plain(&props, &mut state, Key::End);
    typed(&props, &mut state, "abc");
    plain(&props, &mut state, Key::Enter);
    typed(&props, &mut state, "de");
    assert_eq!(text(&props, &state), "xabc\nde");
    let acts = ctrl(&props, &mut state, Key::Char('z'));
    assert!(matches!(&acts[..], [Act::SetFormField { value, .. }] if value == "xabc\n"));
    ctrl(&props, &mut state, Key::Char('z'));
    ctrl(&props, &mut state, Key::Char('z'));
    assert_eq!(text(&props, &state), "x");
    assert_eq!(editor(&props, &state).cursor, 1);
    ctrl(&props, &mut state, Key::Char('z'));
    assert_eq!(editor(&props, &state).message, "Nothing to undo");
    ctrl(&props, &mut state, Key::Char('y'));
    assert_eq!(text(&props, &state), "xabc");
    // an edit after an undo drops what redo had
    typed(&props, &mut state, "!");
    ctrl(&props, &mut state, Key::Char('y'));
    assert_eq!(text(&props, &state), "xabc!");
    assert_eq!(editor(&props, &state).message, "Nothing to redo");
}

#[test]
fn tab_indents_and_shift_tab_outdents_until_esc_lets_go() {
    let props = props("<a>\n<b/>\n\n<c/>\n</a>");
    let mut state = ComponentState::default();
    plain(&props, &mut state, Key::Down);
    plain(&props, &mut state, Key::Right);
    plain(&props, &mut state, Key::Tab);
    assert_eq!(text(&props, &state), "<a>\n    <b/>\n\n<c/>\n</a>");
    assert_eq!(editor(&props, &state).row_col(), (1, 5));
    // a selection's lines, the blank one left alone; the line the selection ends at the start of is not one
    for _ in 0..3 {
        shift(&props, &mut state, Key::Down);
    }
    plain(&props, &mut state, Key::Tab);
    assert_eq!(text(&props, &state), "<a>\n        <b/>\n\n    <c/>\n</a>");
    plain(&props, &mut state, Key::BackTab);
    plain(&props, &mut state, Key::BackTab);
    assert_eq!(text(&props, &state), "<a>\n<b/>\n\n<c/>\n</a>");
    plain(&props, &mut state, Key::BackTab);
    assert_eq!(editor(&props, &state).message, "Nothing to outdent");
    // Esc lets go of the focus: Tab is the screen's, then any other key takes the focus again
    plain(&props, &mut state, Key::Escape);
    assert!(Code
        .key(&props, &mut state, &KeyPress::plain(Key::Tab), false)
        .is_none());
    assert!(Code
        .key(&props, &mut state, &KeyPress::plain(Key::Escape), false)
        .is_none());
    ctrl(&props, &mut state, Key::Home);
    plain(&props, &mut state, Key::Tab);
    assert!(text(&props, &state).starts_with("    <a>"));
}

#[test]
fn find_goes_to_the_matches_and_go_to_line_to_the_line() {
    let props = props("one Foo\ntwo foo\nthree FOO");
    let mut state = ComponentState::default();
    ctrl(&props, &mut state, Key::Char('f'));
    typed(&props, &mut state, "foo");
    assert!(shown(&props, &state).last().unwrap().starts_with("Find: foo"));
    plain(&props, &mut state, Key::Enter);
    assert_eq!(editor(&props, &state).selection(), Some((4, 7)));
    plain(&props, &mut state, Key::F(3));
    assert_eq!(editor(&props, &state).selection(), Some((12, 15)));
    plain(&props, &mut state, Key::F(3));
    plain(&props, &mut state, Key::F(3));
    assert_eq!(editor(&props, &state).selection(), Some((4, 7)));
    assert_eq!(editor(&props, &state).message, "Found from the top");
    shift(&props, &mut state, Key::F(3));
    assert_eq!(editor(&props, &state).selection(), Some((22, 25)));
    ctrl(&props, &mut state, Key::Char('f'));
    typed(&props, &mut state, "x");
    plain(&props, &mut state, Key::Enter);
    assert_eq!(editor(&props, &state).message, "Not found: FOOx");
    ctrl(&props, &mut state, Key::Char('g'));
    typed(&props, &mut state, "2a");
    plain(&props, &mut state, Key::Enter);
    assert_eq!(editor(&props, &state).row_col(), (1, 0));
    ctrl(&props, &mut state, Key::Char('g'));
    plain(&props, &mut state, Key::Escape);
    assert!(editor(&props, &state).prompt.is_none());
}

#[test]
fn highlighting_carries_comments_cdata_and_tags_across_lines() {
    let roles = |language, lines: &[&str]| -> Vec<Vec<Role>> {
        let mut carry = Carry::default();
        lines
            .iter()
            .map(|text| {
                let (paints, next) = highlight::line(language, text, carry);
                carry = next;
                paints.into_iter().map(|(role, _)| role).collect()
            })
            .collect()
    };
    let xml = roles(
        Language::Xml,
        &[
            "<!-- a",
            "b --> <field name=\"x\"",
            "   type=\"${t}\"/><![CDATA[",
            "if (a) ${b}",
            "]]>",
        ],
    );
    assert!(xml[0].iter().all(|role| *role == Role::Muted));
    assert_eq!(xml[1][0], Role::Muted, "the comment goes on");
    assert_eq!(xml[1][7], Role::Primary, "the tag");
    assert_eq!(xml[1][13], Role::Info, "an attribute's name");
    assert_eq!(xml[1][19], Role::Positive, "its value");
    assert_eq!(xml[2][3], Role::Info, "the tag goes on to the next line");
    assert_eq!(xml[2][9], Role::Warning, "${{}} in a value");
    assert_eq!(xml[2][14], Role::Primary, "the end of the tag");
    assert_eq!(xml[3][0], Role::Text, "CDATA is text");
    assert_eq!(xml[3][7], Role::Warning, "with its ${{}}");
    assert!(xml[4].iter().all(|role| *role == Role::Muted));
    let js = roles(Language::Js, &["const a = `x", "${b}` // c", "/* d", "e */ return 1"]);
    assert_eq!(js[0][0], Role::Secondary);
    assert_eq!(js[0][10], Role::Positive);
    assert_eq!(js[1][0], Role::Warning);
    assert_eq!(js[1][4], Role::Positive);
    assert_eq!(js[1][7], Role::Muted);
    assert_eq!(js[3][0], Role::Muted);
    assert_eq!(js[3][5], Role::Secondary);
    assert_eq!(js[3][12], Role::Info);
    let sql = roles(Language::Sql, &["SELECT 'a' -- b", "FROM x"]);
    assert_eq!(
        (sql[0][0], sql[0][7], sql[0][11]),
        (Role::Secondary, Role::Positive, Role::Muted)
    );
    let json = roles(Language::Json, &[r#"{"a": "b", "c": true}"#]);
    assert_eq!(
        (json[0][1], json[0][6], json[0][16]),
        (Role::Info, Role::Positive, Role::Secondary)
    );
    let md = roles(Language::Md, &["# Title", "```", "# not", "```"]);
    assert_eq!((md[0][0], md[2][0]), (Role::Primary, Role::Info));
    assert!(highlight::JS_KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(highlight::SQL_KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn problems_are_marked_in_the_gutter_and_f8_goes_through_them() {
    let mut props = props("<a>\n  <b x=\"1\"/>\n  <c/>\n</a>");
    props["problems"] = json!([
        {"line": 3, "column": 4, "severity": "warning", "message": "c is old"},
        {"line": 2, "column": 6, "severity": "error", "message": "x is not an attribute of b"}
    ]);
    let mut state = ComponentState::default();
    let lines = shown(&props, &state);
    assert_eq!(lines[1], "  2! ".to_string() + "  <b x=\"1\"/>");
    assert_eq!(lines[2], "  3·   <c/>");
    assert_eq!(lines[0], "  1  <a>");
    plain(&props, &mut state, Key::F(8));
    assert_eq!(editor(&props, &state).row_col(), (1, 5));
    assert!(shown(&props, &state)
        .last()
        .unwrap()
        .ends_with("! x is not an attribute of b"));
    plain(&props, &mut state, Key::F(8));
    assert_eq!(editor(&props, &state).row_col(), (2, 3));
    plain(&props, &mut state, Key::F(8));
    assert_eq!(editor(&props, &state).row_col(), (1, 5), "around to the first");
    shift(&props, &mut state, Key::F(8));
    assert_eq!(editor(&props, &state).row_col(), (2, 3));
}

#[test]
fn a_check_follows_the_typing_and_its_problems_replace_the_props() {
    let props = props("<a/>");
    let mut state = ComponentState::default();
    assert_eq!(Code.poll_millis(&props, &state), None);
    typed(&props, &mut state, "x");
    assert_eq!(Code.poll_millis(&props, &state), Some(CHECK_MILLIS));
    let acts = Code.tick(&props, &mut state);
    let [Act::Call { url, parameters }] = &acts[..] else {
        panic!("a check: {acts:?}");
    };
    assert_eq!(url, "check");
    assert_eq!(parameters["text"], "x<a/>");
    assert_eq!(parameters["path"], "a.xml");
    assert_eq!(parameters["rootId"], "r1");
    assert_eq!(parameters["language"], "xml");
    assert_eq!(Code.poll_millis(&props, &state), None, "one check for one pause");
    Code.receive(
        &props,
        &mut state,
        json!({"problems": [{"line": 1, "column": 1, "severity": "error", "message": "text before the root"}]}),
    );
    assert!(shown(&props, &state)[0].starts_with("  1!"));
    assert!(Code.tick(&props, &mut state).is_empty());
}

#[test]
fn read_only_text_refuses_edits_and_lets_tab_move_on() {
    let mut props = props("<a/>");
    props["readOnly"] = json!(true);
    let mut state = ComponentState::default();
    for key in [Key::Char('x'), Key::Enter, Key::Backspace, Key::Delete] {
        assert!(plain(&props, &mut state, key).is_empty());
        assert_eq!(editor(&props, &state).message, "Read only");
    }
    for key in ['x', 'v', 'z', 'y'] {
        ctrl(&props, &mut state, Key::Char(key));
        assert_eq!(editor(&props, &state).message, "Read only");
    }
    assert!(Code.paste(&props, &mut state, "y").unwrap().is_empty());
    assert_eq!(text(&props, &state), "<a/>");
    assert!(Code
        .key(&props, &mut state, &KeyPress::plain(Key::Tab), false)
        .is_none());
    // moving, selecting and copying still work
    shift(&props, &mut state, Key::End);
    assert_eq!(ctrl(&props, &mut state, Key::Char('c')), vec![Act::Copy("<a/>".into())]);
    assert!(shown(&props, &state).last().unwrap().contains("read only"));
    assert_eq!(Code.poll_millis(&props, &state), None);
}

#[test]
fn ctrl_space_completes_xml_from_the_schema_digest() {
    let props = props("<screen>\n    <widgets>\n        <lab\n    </widgets>\n</screen>");
    let digest = json!({"digest": {"root": "screen", "elements": [
        {"name": "screen", "top": true, "children": ["widgets"], "attributes": [{"name": "require-authentication"}]},
        {"name": "widgets", "children": ["label", "link"], "attributes": []},
        {"name": "label", "children": [], "doc": "Text to show.\nMore.",
            "attributes": [{"name": "text", "required": true}, {"name": "type", "values": ["h1", "h2"]}]}
    ]}});
    let mut state = ComponentState::default();
    ctrl(&props, &mut state, Key::Char('g'));
    typed(&props, &mut state, "3");
    plain(&props, &mut state, Key::Enter);
    plain(&props, &mut state, Key::End);
    let acts = ctrl(&props, &mut state, Key::Char(' '));
    let [Act::Call { url, parameters }] = &acts[..] else {
        panic!("a digest asked for: {acts:?}");
    };
    assert_eq!((url.as_str(), parameters["rootElement"].as_str()), ("schema", "screen"));
    assert_eq!(
        (parameters["line"].as_str(), parameters["prefix"].as_str()),
        ("3", "lab")
    );
    Code.receive(&props, &mut state, digest);
    let lines = shown(&props, &state);
    assert!(lines[3].contains(" label  Text to show. "), "{lines:#?}");
    // Enter takes it; then the tag's attributes, the required one first, are offered from the digest kept
    plain(&props, &mut state, Key::Enter);
    typed(&props, &mut state, " ");
    assert!(
        ctrl(&props, &mut state, Key::Char(' ')).is_empty(),
        "the digest is kept"
    );
    let popup = editor(&props, &state).popup.unwrap();
    let labels: Vec<&str> = popup.all.iter().map(|choice| choice.label.as_str()).collect();
    assert_eq!(labels, ["text", "type"]);
    plain(&props, &mut state, Key::Down);
    plain(&props, &mut state, Key::Tab);
    assert_eq!(text(&props, &state).lines().nth(2), Some("        <label type=\"\""));
    // inside the quotes: the attribute's values, narrowed as it is typed
    ctrl(&props, &mut state, Key::Char(' '));
    typed(&props, &mut state, "h2");
    assert_eq!(
        editor(&props, &state)
            .shown(editor(&props, &state).popup.as_ref().unwrap())
            .len(),
        1
    );
    plain(&props, &mut state, Key::Enter);
    assert_eq!(text(&props, &state).lines().nth(2), Some("        <label type=\"h2\""));
    // between tags: the elements the parent takes, written with their bracket
    let (from, choices) = complete::xml(&editor(&props, &state).digest, "<screen>\n  w", 12);
    assert_eq!((from, choices[0].insert.as_str()), (11, "<widgets"));
    assert!(complete::xml(&editor(&props, &state).digest, "<screen><!-- w", 14)
        .1
        .is_empty());
}

#[test]
fn a_list_answer_completes_any_language() {
    let mut props = props("ec.ent");
    props["language"] = json!("js");
    let mut state = ComponentState::default();
    plain(&props, &mut state, Key::End);
    let acts = ctrl(&props, &mut state, Key::Char(' '));
    let [Act::Call { parameters, .. }] = &acts[..] else {
        panic!("{acts:?}");
    };
    assert_eq!(
        (parameters["text"].as_str(), parameters["prefix"].as_str()),
        ("ec.ent", "ec.ent")
    );
    Code.receive(
        &props,
        &mut state,
        json!({"items": ["ec.entity", {"label": "ec.web", "detail": "web"}]}),
    );
    assert_eq!(
        editor(&props, &state).popup.as_ref().map(|popup| popup.all.len()),
        Some(2)
    );
    plain(&props, &mut state, Key::Enter);
    assert_eq!(text(&props, &state), "ec.entity");
}

#[test]
fn long_lines_scroll_by_half_the_width() {
    assert_eq!(scroll_left(9, 10), 0);
    assert_eq!(scroll_left(10, 10), 5);
    assert_eq!(scroll_left(14, 10), 5);
    assert_eq!(scroll_left(15, 10), 10);
    let line: String = (0..100).map(|n| char::from(b'a' + (n % 26) as u8)).collect();
    let mut props = props(&line);
    props["language"] = json!("plain");
    let mut state = ComponentState::default();
    plain(&props, &mut state, Key::End);
    // 60 columns less a gutter of 5: the cursor at column 100 shows past the half-width step
    let first = &shown(&props, &state)[0];
    assert!(first.starts_with("  1  "), "{first}");
    assert!(first.ends_with("uv "), "{first}");
    assert_eq!(first.chars().count(), 5 + 47);
}

#[test]
fn new_props_load_the_text_again_and_cursor_line_places_the_cursor() {
    let mut props = props("a\nb\nc");
    props["cursorLine"] = json!(3);
    let mut state = ComponentState::default();
    plain(&props, &mut state, Key::Right);
    assert_eq!(editor(&props, &state).row_col(), (2, 1));
    props["text"] = json!("x\ny");
    props["cursorLine"] = json!("2");
    assert_eq!(editor(&props, &state).row_col(), (1, 0));
    assert!(!editor(&props, &state).modified());
}
