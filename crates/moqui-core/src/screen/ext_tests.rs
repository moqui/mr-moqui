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

use super::*;

fn node(xml: &str) -> MNode {
    MNode::parse_text("test", xml).unwrap()
}

#[test]
fn conditions_are_simple_field_tests() {
    let truthy = parse_condition("canFile").unwrap().unwrap();
    assert_eq!((truthy.field.as_str(), truthy.op), ("canFile", "truthy"));
    let falsy = parse_condition("!archived").unwrap().unwrap();
    assert_eq!((falsy.field.as_str(), falsy.op), ("archived", "falsy"));
    let equals = parse_condition("kind == 'email'").unwrap().unwrap();
    assert_eq!(
        (equals.field.as_str(), equals.op, equals.value.as_str()),
        ("kind", "equals", "email")
    );
    let not_equals = parse_condition("status != \"Closed\"").unwrap().unwrap();
    assert_eq!((not_equals.op, not_equals.value.as_str()), ("not-equals", "Closed"));
    assert_eq!(parse_condition("  ").unwrap(), None);
    for bad in ["a && b", "fetch('x')", "x == 'a' + 'b'", "x()", "a == b'"] {
        assert!(parse_condition(bad).is_err(), "{bad}");
    }
}

#[test]
fn items_parse_with_one_target_and_sort_by_sequence() {
    let screen = node(
        r#"<screen>
            <ext-action name="b.second" slot="message-actions" sequence="20" label="Second" transition="fileOnRecord">
                <parameter name="chatEventId" from="eventId"/><parameter name="chatRoomId"/>
            </ext-action>
            <ext-action name="a.first" slot="message-actions" sequence="10" label="First" screen="CreateTask" condition="!archived"/>
            <ext-action name="bad.both" slot="x" screen="A" transition="b"/>
            <ext-action name="bad.none" slot="x"/>
            <ext-action name="bad.abs" slot="x" screen="component://x/screen/A.xml"/>
            <ext-action name="bad.cond" slot="x" screen="A" condition="a || b"/>
            <ext-panel name="p.card" slot="room-panel" screen="RecordCard" target="page"/>
            <ext-panel name="bad.panel" slot="room-panel" transition="x"/>
        </screen>"#,
    );
    let items = of(&screen, "test");
    let names: Vec<&str> = items.actions.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, ["a.first", "b.second"]);
    assert_eq!(
        items.actions[1].parameters,
        [
            ("chatEventId".to_string(), "eventId".to_string()),
            ("chatRoomId".to_string(), "chatRoomId".to_string())
        ]
    );
    assert_eq!(items.actions[0].target, "dialog");
    assert_eq!(items.panels.len(), 1);
    assert_eq!(items.panels[0].target, "page");
}

#[test]
fn an_extension_replaces_an_item_by_name_and_adds_new_ones() {
    let screen = node(
        r#"<screen><ext-action name="a" slot="s" label="Old" screen="One"/><ext-action name="b" slot="s" screen="Two"/></screen>"#,
    );
    merge_extend(
        &screen,
        &node(
            r#"<screen-extend><ext-action name="a" slot="s" label="New" transition="go"/><ext-action name="c" slot="s" screen="Three"/></screen-extend>"#,
        ),
        "ext-action",
    );
    let items = of(&screen, "test");
    let summary: Vec<(String, String, String)> = items
        .actions
        .iter()
        .map(|item| (item.name.clone(), item.label.clone(), item.transition.clone()))
        .collect();
    assert_eq!(
        summary,
        [
            ("a".into(), "New".into(), "go".into()),
            ("b".into(), String::new(), String::new()),
            ("c".into(), String::new(), String::new())
        ]
    );
}

#[test]
fn a_relative_path_walks_from_the_screen() {
    let base = vec!["qapps".to_string(), "Comms".to_string(), "Chat".to_string()];
    assert_eq!(relative_names(&base, "Dialog"), ["qapps", "Comms", "Chat", "Dialog"]);
    assert_eq!(
        relative_names(&base, "../Inbox/Compose"),
        ["qapps", "Comms", "Inbox", "Compose"]
    );
    assert_eq!(relative_names(&base, "./A/../B"), ["qapps", "Comms", "Chat", "B"]);
}
