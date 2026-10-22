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

//! The dialect registry, the canvas's tail line, the read proxy's rows and a Mutation's request parameters.

use serde_json::json;

use super::*;
use crate::llm::tools::{caps, ClientCaps};

#[test]
fn the_dialects_register_and_vue_sfc_only_for_an_agent_that_names_it() {
    let names: Vec<&str> = dialects().iter().map(|dialect| dialect.name()).collect();
    assert_eq!(names, ["openui", "vue-sfc"]);
    assert!(dialect("openui").unwrap().available());
    assert_eq!(dialect("openui").unwrap().skill(), "openui-lang");
    assert!(dialect("vue-sfc").unwrap().available());
    assert_eq!(dialect("vue-sfc").unwrap().skill(), "vue-sfc");
    let drawer = ClientCaps::for_surface(crate::llm::prompt::Surface::Drawer);
    let offered_here: Vec<&str> = offered(&drawer, false).iter().map(|dialect| dialect.name()).collect();
    assert_eq!(offered_here, ["openui", "vue-sfc"]);
    let chat = ClientCaps::of([caps::QUESTION_INLINE]);
    assert!(offered_names(&chat).is_empty());
    // an agent writes OpenUI Lang only unless its canvasDialects name another (Vue SFC is off by default)
    let plain = crate::llm::run::AgentDef::default();
    assert!(plain.allows_dialect("openui") && !plain.allows_dialect("vue-sfc"));
    let lowered = drawer.clone().dialects_only(|name| plain.allows_dialect(name));
    assert!(lowered.has(caps::UI_OPENUI) && !lowered.has(caps::UI_VUE_SFC) && lowered.has(caps::APPROVAL_INLINE));
    let both = crate::llm::run::AgentDef { canvas_dialects: vec!["openui".to_string(), "vue-sfc".to_string()], ..Default::default() };
    assert!(drawer.clone().dialects_only(|name| both.allows_dialect(name)).has(caps::UI_VUE_SFC));
    let only_sfc = crate::llm::run::AgentDef { canvas_dialects: vec!["vue-sfc".to_string()], ..Default::default() };
    assert!(!only_sfc.allows_dialect("openui"));
}

fn offered_names(caps: &ClientCaps) -> Vec<&'static str> {
    offered(caps, true).iter().map(|dialect| dialect.name()).collect()
}

#[test]
fn the_tail_names_the_canvas_and_its_open_problems() {
    let canvas = Canvas {
        title: "Logins by day".to_string(),
        version: 3,
        dialect: "openui".to_string(),
        data: json!({"problems": [{"problem": "the Query hits failed: no list", "skill": "openui-lang#query"}]}),
        ..Canvas::default()
    };
    assert_eq!(
        canvas.tail_line(),
        "canvas: \"Logins by day\" v3 (openui), problems the page reported: the Query hits failed: no list (openui-lang#query)"
    );
}

#[test]
fn a_list_read_answers_rows_keyed_by_field_name() {
    let list = json!({"t": "formList", "name": "VisitList", "paging": {"count": 7},
        "columns": [{"title": "Visit ID / From Date", "fields": ["visitId", "fromDate"]}, {"title": "Hit Count", "fields": ["hitCount"]},
            {"title": "Note"}],
        "rows": [{"id": "V1", "cells": [[{"t": "link", "url": "/qapps/system/Visit/VisitDetail?visitId=V1", "text": "V1"}, "2026-10-20 10:00"], "12", "x"]}]});
    let answer = proxy::rows_of(&list, &["VisitList".to_string()]);
    assert_eq!(
        answer["rows"][0],
        json!({"_id": "V1", "visitId": "V1", "fromDate": "2026-10-20 10:00", "hitCount": "12", "note": "x",
            "_link": "/apps/system/Visit/VisitDetail?visitId=V1"})
    );
    assert_eq!(answer["totalCount"], json!(7));
    assert_eq!(answer["columns"][1], json!({"key": "fromDate", "title": "From Date"}));
    assert_eq!(proxy::title_key("Hit Count"), "hitCount");
}

#[test]
fn a_mutation_body_becomes_flat_request_parameters() {
    let parameters = write::parameters_of(&json!({"a": "x", "n": 3, "flag": true, "list": ["p", "q"], "none": null}));
    assert_eq!(
        parameters.into_iter().collect::<Vec<_>>(),
        [
            ("a".to_string(), "x".to_string()),
            ("flag".to_string(), "true".to_string()),
            ("list".to_string(), "p,q".to_string()),
            ("n".to_string(), "3".to_string()),
            // a null is posted empty, as a form posts an empty field: every parameter the body names is sent and shown
            ("none".to_string(), String::new())
        ]
    );
    let error = crate::llm::tools::ui::skill_required("openui-lang");
    let error: serde_json::Value = serde_json::from_str(&error).unwrap();
    assert_eq!(error["error"], json!("skill_required"));
    assert_eq!(
        error["instruction"],
        json!("write_ui needs the openui-lang skill. Call skill with name \"openui-lang\", then call write_ui again.")
    );
}

#[test]
fn a_streaming_write_ui_gives_the_program_as_it_grows() {
    let mut programs = StreamingPrograms::default();
    programs.start(0, "write_ui");
    programs.start(1, "screen");
    assert_eq!(programs.args(0, "{\"title\":\"Hits\",\"prog"), None);
    let first = programs.args(0, "ram\":\"root = Stack([a])\\nhits = Query(").unwrap();
    assert_eq!(first, json!({"index": 0, "patch": false, "offset": 0, "text": "root = Stack([a])\nhits = Query("}));
    let second = programs.args(0, "\\\"request\\\"").unwrap();
    assert_eq!(second["offset"], json!(31));
    assert_eq!(second["text"], json!("\"request\""));
    assert_eq!(programs.args(1, "{\"go\":\"/apps\"}"), None);
    assert_eq!(partial_program("{\"patch\":\"chart = null\"}"), Some((true, "chart = null".to_string())));
}
