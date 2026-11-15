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

fn other(id: &str, kind: &str, short: &str, name: &str, component: &str) -> Json {
    json!({"id": id, "kind": kind, "shortName": short, "name": name, "component": component})
}

/// What uses `entity:mantle.order.OrderHeader`: a screen twice (a find and an observed call), a service, a framework
/// screen, and two forms of one short name in different screens.
fn incoming() -> Vec<Json> {
    let screen = other(
        "screen:component://shop/screen/Order.xml",
        "screen",
        "Order",
        "component://shop/screen/Order.xml",
        "shop",
    );
    vec![
        json!({"from": "screen:component://shop/screen/Order.xml", "refType": "entity-find", "other": screen, "calls": 0,
            "url": "/qapps/dev/Workspace?rootId=shop&path=screen/Order.xml&line=9"}),
        json!({"from": "screen:component://shop/screen/Order.xml", "refType": "entity-find-one", "other": screen, "calls": 3}),
        json!({"from": "service:shop.OrderServices.place#Order", "refType": "entity-find", "calls": 0,
            "other": other("service:shop.OrderServices.place#Order", "service", "place#Order", "shop.OrderServices.place#Order", "shop")}),
        json!({"from": "screen:component://tools/screen/Tools/Entity.xml", "refType": "entity-find", "calls": 0,
            "other": other("screen:component://tools/screen/Tools/Entity.xml", "screen", "Entity", "component://tools/screen/Tools/Entity.xml", "tools")}),
        json!({"from": "form:component://shop/screen/A.xml#Find", "refType": "form-entity", "calls": 0,
            "other": other("form:component://shop/screen/A.xml#Find", "form", "Find", "component://shop/screen/A.xml#Find", "shop")}),
        json!({"from": "form:component://shop/screen/B.xml#Find", "refType": "form-entity", "calls": 0, "observedOnly": true,
            "other": other("form:component://shop/screen/B.xml#Find", "form", "Find", "component://shop/screen/B.xml#Find", "shop")}),
        // a user the index does not have: its kind is its id's prefix
        json!({"from": "rest:/orders", "refType": "rest-entity", "calls": 0}),
    ]
}

fn ids(section: &Json) -> Vec<&str> {
    section["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect()
}

#[test]
fn incoming_references_group_by_kind_one_row_per_user() {
    let sections = sections(&incoming(), Direction::In, &Filter::default());
    let kinds: Vec<&str> = sections
        .iter()
        .map(|section| section["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["form", "rest", "screen", "service"]);
    let screens = &sections[2];
    assert_eq!(screens["count"], 2);
    assert_eq!(
        ids(screens),
        [
            "screen:component://shop/screen/Order.xml",
            "screen:component://tools/screen/Tools/Entity.xml"
        ]
    );
    // the screen's two references are one row, its ways joined and its calls added
    let order = &screens["rows"][0];
    assert_eq!(order["how"], "entity-find, entity-find-one");
    assert_eq!(order["calls"], 3);
    assert_eq!(order["resolved"], true);
    assert_eq!(order["shortName"], "Order");
    assert_eq!(
        order["url"],
        "/qapps/dev/Workspace?rootId=shop&path=screen/Order.xml&line=9"
    );
    // two forms named Find are told apart by their owner
    let forms: Vec<&str> = sections[0]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["shortName"].as_str().unwrap())
        .collect();
    assert_eq!(forms, ["A.xml#Find", "B.xml#Find"]);
    let rest = &sections[1]["rows"][0];
    assert_eq!(rest["id"], "rest:/orders");
    assert_eq!(rest["resolved"], false);
}

#[test]
fn the_filter_leaves_out_kinds_the_framework_and_observed_references() {
    let hidden = Filter {
        hide_framework: true,
        observed: false,
        ..Filter::default()
    };
    let sections_hidden = sections(&incoming(), Direction::In, &hidden);
    let screens = sections_hidden
        .iter()
        .find(|section| section["kind"] == "screen")
        .unwrap();
    assert_eq!(ids(screens), ["screen:component://shop/screen/Order.xml"]);
    let forms = sections_hidden
        .iter()
        .find(|section| section["kind"] == "form")
        .unwrap();
    assert_eq!(forms["count"], 1);
    let only = Filter {
        kinds: vec!["service".to_string()],
        ..Filter::default()
    };
    let sections_only = sections(&incoming(), Direction::In, &only);
    assert_eq!(sections_only.len(), 1);
    assert_eq!(sections_only[0]["kind"], "service");
    let no_static = Filter {
        static_links: false,
        ..Filter::default()
    };
    let observed = sections(&incoming(), Direction::In, &no_static);
    assert_eq!(observed.len(), 1);
    assert_eq!(ids(&observed[0]), ["form:component://shop/screen/B.xml#Find"]);
    // the kinds offered are all of both sides', before any filter
    let refs = incoming();
    let all: Vec<(Direction, &Json)> = refs.iter().map(|entry| (Direction::In, entry)).collect();
    assert_eq!(kinds(&all), ["form", "rest", "screen", "service"]);
}

#[test]
fn outgoing_references_name_their_target() {
    let refs = vec![
        json!({"toKind": "entity", "toName": "mantle.party.Party", "toId": "entity:mantle.party.Party", "refType": "relationship",
            "other": other("entity:mantle.party.Party", "entity", "Party", "mantle.party.Party", "mr-mantle")}),
        json!({"toKind": "entity", "toName": "no.Such", "refType": "relationship"}),
    ];
    let sections = sections(&refs, Direction::Out, &Filter::default());
    assert_eq!(sections.len(), 1);
    assert_eq!(ids(&sections[0]), ["entity:mantle.party.Party", "entity:no.Such"]);
    let missing = &sections[0]["rows"][1];
    assert_eq!(missing["shortName"], "no.Such");
    assert_eq!(missing["resolved"], false);
}

#[test]
fn the_centers_links_follow_its_kind() {
    let keys = |artifact: Json| -> Vec<(String, String)> {
        center_links(&artifact)
            .iter()
            .map(|link| {
                (
                    link["key"].as_str().unwrap().to_string(),
                    link["url"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let entity = keys(json!({"id": "entity:moqui.example.Example", "kind": "entity",
        "url": "/qapps/dev/Workspace?rootId=example&path=entity/ExampleEntities.xml&line=12"}));
    assert_eq!(
        entity,
        [
            (
                "t".to_string(),
                "/qapps/dev/Workspace?rootId=example&path=entity/ExampleEntities.xml&line=12".to_string()
            ),
            (
                "d".to_string(),
                "/qapps/dev/Entities/Edit?artifact=entity%3Amoqui.example.Example".to_string()
            ),
            (
                "r".to_string(),
                "/qapps/dev/Artifacts/Refs?artifactId=entity%3Amoqui.example.Example".to_string()
            ),
            (
                "D".to_string(),
                "/qapps/dev/Artifacts/Doc?artifactId=entity%3Amoqui.example.Example".to_string()
            ),
        ]
    );
    let service = keys(json!({"id": "service:a.B.do#It", "kind": "service"}));
    assert_eq!(
        service[0],
        (
            "y".to_string(),
            "/qapps/dev/Services/Edit?artifact=service%3Aa.B.do%23It".to_string()
        )
    );
    let screen = keys(
        json!({"id": "screen:component://shop/screen/Order.xml", "kind": "screen",
        "rootId": "shop", "relativePath": "screen/Order.xml"}),
    );
    assert_eq!(
        screen[0],
        (
            "p".to_string(),
            "/qapps/dev/Screens/Design?rootId=shop&path=screen/Order.xml".to_string()
        )
    );
    let lines = center_lines(&json!({"name": "moqui.example.Example", "component": "example",
        "relativePath": "entity/ExampleEntities.xml", "line": 12, "summary": "An example"}));
    assert_eq!(
        lines,
        [
            "moqui.example.Example",
            "example · ExampleEntities.xml:12",
            "An example"
        ]
    );
}
