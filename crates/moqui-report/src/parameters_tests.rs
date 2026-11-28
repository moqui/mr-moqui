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

//! A dataset's declared parameters: what a save refuses, and what the editors are given. Their choices, read through
//! a service as the viewer, are the tools CalcGrid spec's (`list#Datasets` of the shipped `calcgrid`).

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use serde_json::{json, Value as Json};

use super::{choice_services, declared, refusal};

fn context() -> ExecutionContext {
    ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    )
}

/// The shipped CalcGrid dataset, as the tools component ships it.
fn calcgrid() -> Json {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/components/base/tools/report/calcgrid.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("calcgrid.json")).expect("calcgrid.json is JSON")
}

#[test]
fn the_shipped_calcgrid_dataset_declares_the_document_the_range_the_chart_and_the_profile() {
    let body = &calcgrid()["body"];
    assert_eq!(refusal(body), None);
    let ec = context();
    let listed = declared(&ec, "calcgrid", body, false);
    let names: Vec<&str> = listed.iter().filter_map(|one| one["name"].as_str()).collect();
    assert_eq!(names, ["definitionId", "range", "chart", "profileId"]);
    assert!(listed.iter().all(|one| one.get("choices").is_none()));
    let by = |name: &str| {
        listed
            .iter()
            .find(|one| one["name"] == name)
            .cloned()
            .unwrap_or(Json::Null)
    };
    assert_eq!(by("definitionId")["optional"], false);
    assert_eq!(by("profileId")["optional"], true);
    assert_eq!(by("range")["kind"], "text");
    assert_eq!(by("chart")["optional"], true);
    assert_eq!(
        choice_services(body),
        [
            "org.moqui.report.ReportServices.list#Definitions",
            "org.moqui.calc.CalcServices.get#CalcProfiles"
        ]
    );
}

#[test]
fn a_declaration_is_named_once_of_a_known_kind_with_whole_choices() {
    let with = |parameters: Json| refusal(&json!({ "parameters": parameters }));
    assert_eq!(refusal(&json!({})), None);
    assert_eq!(
        with(json!([{"name": "a"}, {"name": "b", "kind": "number", "optional": true}])),
        None
    );
    for (parameters, says) in [
        (json!({"name": "a"}), "is a list"),
        (json!([{"name": "1a"}]), "starting with a letter"),
        (json!([{"name": "a b"}]), "starting with a letter"),
        (json!([{"name": "a"}, {"name": "a"}]), "once"),
        (json!([{"name": "a", "kind": "date"}]), "text or number"),
        (json!([{"name": "a", "optional": "yes"}]), "true or false"),
        (json!([{"name": "a", "label": 5}]), "label is a text"),
        (
            json!([{"name": "a", "choices": {"service": "x.y#Z"}}]),
            "the list it answers",
        ),
        (
            json!([{"name": "a", "choices": {"service": "x.y#Z", "list": "l", "value": "v", "parameters": []}}]),
            "a map",
        ),
    ] {
        let refused = with(parameters.clone()).unwrap_or_default();
        assert!(refused.contains(says), "{parameters} refused as {refused:?}");
    }
    let many: Vec<Json> = (0..17).map(|at| json!({"name": format!("p{at}")})).collect();
    assert!(with(Json::Array(many)).is_some_and(|refused| refused.contains("at most 16")));
}

#[test]
fn the_editors_are_given_a_label_and_kind_for_each_and_nothing_for_a_dataset_without() {
    let ec = context();
    assert!(declared(&ec, "d", &json!({"serviceName": "x.y#Z"}), false).is_empty());
    let listed = declared(
        &ec,
        "d",
        &json!({"parameters": [{"name": "region"}, {"name": "bad name"}, {"name": "top", "label": "Top", "kind": "number"}]}),
        false,
    );
    assert_eq!(
        listed,
        [
            json!({"name": "region", "label": "region", "kind": "text", "optional": false}),
            json!({"name": "top", "label": "Top", "kind": "number", "optional": false}),
        ]
    );
}
