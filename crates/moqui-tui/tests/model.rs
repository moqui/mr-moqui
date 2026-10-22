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

//! Screen model round trip. Unknown fields are ignored. A version other than 1 is refused.

use moqui_tui::{
    gallery_link, Cell, ModelError, Node, ScreenLink, ScreenModel, Widget, GALLERY_DETAIL_JSON, GALLERY_DETAIL_PATH,
    GALLERY_LIST_JSON, GALLERY_LIST_PATH,
};

#[test]
fn gallery_fixtures_round_trip() {
    for text in [GALLERY_LIST_JSON, GALLERY_DETAIL_JSON] {
        let model = ScreenModel::from_json(text).unwrap_or_else(|err| panic!("{err}"));
        let again = serde_json::to_string(&model).unwrap();
        let parsed = ScreenModel::from_json(&again).unwrap();
        assert_eq!(model, parsed);
    }
    let link = gallery_link();
    assert_eq!(
        link.level(GALLERY_LIST_PATH, &std::collections::BTreeMap::new())
            .unwrap()
            .screen
            .title,
        "Enumerations"
    );
    assert_eq!(
        link.level(GALLERY_DETAIL_PATH, &std::collections::BTreeMap::new())
            .unwrap()
            .screen
            .path,
        GALLERY_DETAIL_PATH
    );
}

#[test]
fn list_rows_carry_the_unicode_cells() {
    let model = ScreenModel::from_json(GALLERY_LIST_JSON).unwrap();
    let Node::FormList { list } = &model.c[1] else {
        panic!("list fixture child 1 is the form list");
    };
    assert_eq!(list.rows.len(), 20);
    assert_eq!(list.paging.count, 20);
    assert_eq!(list.rows[0].cells[0].text(), "ENUM_00");
    assert_eq!(list.rows[0].cells[0].link_url(), Some(GALLERY_DETAIL_PATH));
    assert_eq!(list.rows[0].cells[3].text(), "東京");
    assert_eq!(list.rows[1].cells[3].text(), "🎉");
    assert_eq!(list.rows[2].cells[3].text(), "👨\u{200D}👩\u{200D}👧");
    assert_eq!(list.rows[3].cells[1].text().chars().count(), 45);
    assert_eq!(list.rows[4].cells[3].text(), "family-note");
    assert_eq!(list.rows[10].cells[0].text(), "ENUM_10");
    assert!(matches!(list.rows[10].cells[0], Cell::Text(_)));
}

#[test]
fn unknown_fields_are_ignored_and_a_new_version_is_refused() {
    let text = r#"{"v":1,"extra":true,"screen":{"path":"/t","title":"T","later":1},"c":[
        {"t":"field","id":"a","name":"a","title":"A","nope":2,"w":{"kind":"textLine","value":"v","extra":1}}
    ]}"#;
    let model = ScreenModel::from_json(text).unwrap();
    assert_eq!(model.screen.path, "/t");
    assert_eq!(model.c[0].id(), "a");
    let again = ScreenModel::from_json(&serde_json::to_string(&model).unwrap()).unwrap();
    assert_eq!(model, again);

    let err = ScreenModel::from_json(r#"{"v":2,"screen":{"path":"/t","title":""},"c":[]}"#).unwrap_err();
    assert_eq!(err, ModelError::Version(2));

    // A node or widget this client does not know is a placeholder, so one new type does not lose the page.
    let unknown = ScreenModel::from_json(
        r#"{"v":1,"screen":{"path":"/t"},"c":[{"t":"nope","id":"x"},
            {"t":"field","id":"f","name":"f","w":{"kind":"colorPicker","value":"red"}}]}"#,
    )
    .unwrap();
    assert!(
        matches!(&unknown.c[0], Node::Placeholder { id, text, .. } if id == "x" && text == "nope"),
        "{unknown:?}"
    );
    assert!(
        matches!(&unknown.c[1], Node::Field { w: moqui_tui::Widget::Display { text, .. }, .. } if text == "[colorPicker]"),
        "{unknown:?}"
    );
}

/// D9: control and bidi characters in any string of a document, keys included, are gone before anything draws.
#[test]
fn control_and_bidi_characters_never_reach_the_screen() {
    let text = "{\"v\":1,\"screen\":{\"path\":\"/t\",\"title\":\"a\\u001b]52;c;x\\u0007b\"},\
        \"parameters\":{\"k\\u001b\":\"\\u202Eevil\\u2066\"},\
        \"c\":[{\"t\":\"text\",\"id\":\"t\",\"text\":\"line\\none\\ttab\\u0085\\u009b31m\"}]}";
    let model = ScreenModel::from_json(text).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(model.screen.title, "a]52;c;xb");
    assert_eq!(model.parameters.get("k").map(String::as_str), Some("evil"));
    let Node::Text { text, .. } = &model.c[0] else {
        panic!("{model:?}");
    };
    assert_eq!(text, "line\none\ttab31m");
    assert_eq!(moqui_tui::clean_text("ok\u{1b}[2J"), "ok[2J");
}

#[test]
fn exit_nodes_parse_and_a_password_keeps_no_value() {
    let text = r#"{"v":1,"screen":{"path":"/p","pathList":["p"],"title":"P","standalone":1},
        "parameters":{"keep":"yes"},"focus":"F.name","messages":{"info":["saved"],"errors":[],
        "validationErrors":[{"form":"F","field":"name","message":"required"}]},
        "c":[
            {"t":"section","id":"S","name":"S","iterate":true,"c":[
                {"t":"text","id":"label","text":"a"},
                {"t":"error","text":"boom"},
                {"t":"placeholder","id":"ph","text":"chart","url":"/chart"}
            ]},
            {"t":"row","id":"r","sm":"6","c":[
                {"t":"numberCard","id":"card","title":"Open","value":"3","url":"/open"}
            ]},
            {"t":"dialog","id":"D","button":"New","c":[
                {"t":"form","id":"F","name":"F","c":[
                    {"t":"field","id":"F.pw","name":"pw","w":{"kind":"password","value":"s3cret"}},
                    {"t":"field","id":"F.st","name":"st","w":{"kind":"dropDown","value":"","optionsUrl":"/states",
                        "dependsOn":{"countryGeoId":"country"},"optionsParameters":{"geoTypeEnumId":"GEOT_STATE"}}}
                ]}
            ]},
            {"t":"subscreens","id":"tabs","kind":"tab","items":[{"title":"Find","url":"/find","active":true}]}
        ]}"#;
    let model = ScreenModel::from_json(text).unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(model.screen.standalone, 1);
    assert_eq!(model.parameters.get("keep").map(String::as_str), Some("yes"));
    assert_eq!(model.messages.validation_errors[0].field, "name");
    assert_eq!(model.focus, "F.name");
    let Node::Section { iterate, c, .. } = &model.c[0] else {
        panic!("section");
    };
    assert!(*iterate);
    assert!(matches!(c[1], Node::Error { .. }));
    assert_eq!(c[1].id(), "");
    let Node::Dialog { c: dialog, .. } = &model.c[2] else {
        panic!("dialog");
    };
    let Node::Form { c: fields, .. } = &dialog[0] else {
        panic!("form");
    };
    let Node::Field { w, .. } = &fields[0] else {
        panic!("password field");
    };
    assert!(matches!(w, Widget::Password {}));
    assert_eq!(w.value(), "");
    let again = ScreenModel::from_json(&serde_json::to_string(&model).unwrap()).unwrap();
    assert_eq!(model, again);
    assert!(!serde_json::to_string(&model).unwrap().contains("s3cret"));
}
