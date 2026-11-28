/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! The terminal report editors: a definition's structured authoring, and the query bar with its filter pills.
use super::*;
#[test]
fn visualization_choices_put_the_suggested_ones_first() {
    let panel = json!({"queries":[{"aggregations":[{"id":"t","type":"date_histogram","role":"x-axis"},{"id":"n","type":"sum","role":"metric"}]}]});
    assert_eq!(suggested_visualizations(&panel)[0], ("timeSeries", "Time and a number"));
    let props = json!({"value":{"panels":{"p":panel.clone()}}});
    let row = Row {
        path: vec!["panels".into(), "p".into(), "visualization".into()],
        label: "visualization".into(),
        value: json!(""),
    };
    let choices = scalar_choices(&props, &ComponentState::default(), &row);
    assert_eq!(&choices[..2], ["timeSeries", "stat"]);
    assert_eq!(choices.iter().filter(|name| *name == "stat").count(), 1);
    let map = json!({"queries":[{"mode":"serviceMap"}]});
    assert_eq!(suggested_visualizations(&map)[0].0, "nodeGraph");
}
fn key(props: &Value, state: &mut ComponentState, key: Key) -> Vec<Act> {
    ReportDefinitionEditor
        .key(props, state, &KeyPress::plain(key), false)
        .unwrap_or_default()
}
fn select(props: &Value, state: &mut ComponentState, path: &[&str]) {
    let path: Vec<_> = path.iter().map(|s| s.to_string()).collect();
    state.at = rows(props, state)
        .iter()
        .position(|row| row.path == path)
        .expect("editable path");
}
#[test]
fn author_queries_join_pivot_overrides_variables_layout_and_reports_without_json_input() {
    let props = json!({"kind":"dashboard","form":"EditDefinition","field":"definitionJson","value":{"panels":{"p":{"title":"Original","queries":[query()],"transformations":[],"fieldConfig":{"defaults":{},"overrides":[]}}},"variables":[],"layout":{"type":"tabs","items":[]}}});
    let mut state = ComponentState::default();
    select(&props, &mut state, &["panels", "p", "queries"]);
    key(&props, &mut state, Key::Char('a'));
    let acts = key(&props, &mut state, Key::Enter);
    assert_eq!(
        document(&props, &state)["panels"]["p"]["queries"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(matches!(&acts[0],Act::SetFormField{form,field,..}if form=="EditDefinition"&&field=="definitionJson"));
    for wanted in ["Join frames", "Pivot", "Organize fields"] {
        select(&props, &mut state, &["panels", "p", "transformations"]);
        key(&props, &mut state, Key::Char('a'));
        let row = rows(&props, &state)[state.at].clone();
        state.choosing = Some(
            add_options(&row, "dashboard", &[])
                .iter()
                .position(|(label, _)| label == wanted)
                .unwrap(),
        );
        key(&props, &mut state, Key::Enter);
    }
    select(&props, &mut state, &["panels", "p", "fieldConfig", "overrides"]);
    key(&props, &mut state, Key::Char('a'));
    key(&props, &mut state, Key::Enter);
    select(&props, &mut state, &["variables"]);
    key(&props, &mut state, Key::Char('a'));
    key(&props, &mut state, Key::Enter);
    select(&props, &mut state, &["layout", "items"]);
    key(&props, &mut state, Key::Char('a'));
    state.choosing = Some(1);
    key(&props, &mut state, Key::Enter);
    let doc = document(&props, &state);
    assert_eq!(doc["panels"]["p"]["title"], "Original");
    assert_eq!(doc["panels"]["p"]["transformations"][0]["type"], "join");
    assert_eq!(doc["panels"]["p"]["transformations"][1]["type"], "pivot");
    assert_eq!(doc["layout"]["items"][0]["type"], "row");
    let report = json!({"kind":"report","value":{"dashboards":[]}});
    let mut state = ComponentState::default();
    select(&report, &mut state, &["dashboards"]);
    key(&report, &mut state, Key::Char('a'));
    key(&report, &mut state, Key::Enter);
    key(&report, &mut state, Key::Char('a'));
    key(&report, &mut state, Key::Enter);
    assert_eq!(document(&report, &state)["dashboards"].as_array().unwrap().len(), 2);
}
/// A query over a dataset that declares parameters (list#Datasets with parameterChoices) is given a field for each
/// it needs, an optional one when asked for, each shown by its label and chosen from its choices.
#[test]
fn a_datasets_declared_parameters_are_fields_of_its_query_chosen_from_their_choices() {
    let props = json!({"kind":"dashboard","value":{"panels":{"p":{"title":"Range","queries":[{"datasetId":"calcgrid"}]}}},
        "datasets":[{"id":"calcgrid","service":true,"parameters":[
            {"name":"definitionId","label":"Document","kind":"text","optional":false,"choices":[{"value":"doc-a","label":"Sales by month"},{"value":"doc-b","label":"doc-b"}]},
            {"name":"range","label":"Range","kind":"text","optional":false},
            {"name":"profileId","label":"Read as","kind":"text","optional":true,"choices":[{"value":"sales","label":"Sales"}]}]}]});
    let mut state = ComponentState::default();
    select(&props, &mut state, &["panels", "p", "queries", "0"]);
    key(&props, &mut state, Key::Char('a'));
    let row = rows(&props, &state)[state.at].clone();
    let options = add_options(&row, "dashboard", &add_parameters(&props, &state, &row));
    state.choosing = options.iter().position(|(name, _)| name == "parameters");
    key(&props, &mut state, Key::Enter);
    assert_eq!(
        document(&props, &state)["panels"]["p"]["queries"][0]["parameters"],
        json!({"definitionId": "", "range": ""})
    );
    // the optional one is offered once the query has parameters, and the ones it has are not
    select(&props, &mut state, &["panels", "p", "queries", "0", "parameters"]);
    key(&props, &mut state, Key::Char('a'));
    let row = rows(&props, &state)[state.at].clone();
    let offered: Vec<String> = add_options(&row, "dashboard", &add_parameters(&props, &state, &row))
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(offered, ["profileId", "Add named field…"]);
    key(&props, &mut state, Key::Enter);
    assert_eq!(
        document(&props, &state)["panels"]["p"]["queries"][0]["parameters"]["profileId"],
        ""
    );
    // a field is shown by its label, and its choices by theirs
    select(
        &props,
        &mut state,
        &["panels", "p", "queries", "0", "parameters", "definitionId"],
    );
    key(&props, &mut state, Key::Enter);
    let shown: String = ReportDefinitionEditor
        .lines(&props, &state, 120, true)
        .iter()
        .flat_map(|line| line.iter().map(|span| span.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.contains("Document (definitionId)"), "{shown}");
    assert!(shown.contains("doc-a — Sales by month"), "{shown}");
    assert!(!shown.contains("doc-b — doc-b"), "{shown}");
    key(&props, &mut state, Key::Tab);
    key(&props, &mut state, Key::Enter);
    assert_eq!(
        document(&props, &state)["panels"]["p"]["queries"][0]["parameters"]["definitionId"],
        "doc-a"
    );
    // a query over a dataset that declares none is added as before
    let plain =
        json!({"value":{"panels":{"p":{"queries":[{"datasetId":"otel_logs"}]}}},"datasets":[{"id":"otel_logs"}]});
    let mut state = ComponentState::default();
    select(&plain, &mut state, &["panels", "p", "queries", "0"]);
    let row = rows(&plain, &state)[state.at].clone();
    assert!(add_options(&row, "dashboard", &add_parameters(&plain, &state, &row))
        .iter()
        .all(|(name, _)| name != "parameters"));
}
#[test]
fn readonly_definition_cannot_emit_mutations_and_typed_numbers_are_validated() {
    let props = json!({"readOnly":true,"value":{"count":5}});
    let mut state = ComponentState::default();
    for press in [Key::Char('a'), Key::Char('d'), Key::Char('t'), Key::Enter] {
        assert!(key(&props, &mut state, press).is_empty());
    }
    assert!(state.local.get("document").is_none());
    let props = json!({"value":{"count":5}});
    let mut state = ComponentState::default();
    select(&props, &mut state, &["count"]);
    key(&props, &mut state, Key::Enter);
    key(&props, &mut state, Key::Backspace);
    key(&props, &mut state, Key::Char('x'));
    assert!(key(&props, &mut state, Key::Enter).is_empty());
    assert_eq!(document(&props, &state)["count"], 5);
    key(&props, &mut state, Key::Backspace);
    key(&props, &mut state, Key::Char('0'));
    key(&props, &mut state, Key::Enter);
    assert_eq!(document(&props, &state)["count"], 0);
}
#[test]
fn query_completion_and_filter_pills_bind_real_form_fields() {
    let props = json!({"value":"sev","fields":[{"name":"severity_text","searchable":true},{"name":"secret","searchable":false}],"values":{"severity_text":["ERROR","INFO"]},"filters":[{"label":"Errors","query":{"field":{"name":"severity_text","op":"eq","value":"ERROR"}}}],"form":"SearchOptions","field":"queryString","filtersField":"filtersJson"});
    let mut state = ComponentState::default();
    let acts = ReportQueryEditor
        .key(&props, &mut state, &KeyPress::plain(Key::Tab), false)
        .unwrap();
    assert!(matches!(&acts[0],Act::SetFormField{value,..}if value=="severity_text:"));
    ReportQueryEditor.key(&props, &mut state, &KeyPress::plain(Key::Char('E')), false);
    ReportQueryEditor.key(&props, &mut state, &KeyPress::plain(Key::Tab), false);
    assert_eq!(query_text(&props, &state), "severity_text:ERROR");
    ReportQueryEditor.key(&props, &mut state, &KeyPress::plain(Key::F(2)), false);
    let acts = ReportQueryEditor
        .key(&props, &mut state, &KeyPress::plain(Key::Char('n')), false)
        .unwrap();
    assert!(
        matches!(&acts[0],Act::SetFormField{field,value,..}if field=="filtersJson"&&value.contains("\"negated\":true"))
    );
}
#[test]
fn query_completion_replaces_the_word_after_a_no_break_space() {
    let props = json!({"value":"a\u{a0}sev","fields":[{"name":"severity_text"}]});
    let mut state = ComponentState::default();
    ReportQueryEditor.key(&props, &mut state, &KeyPress::plain(Key::Tab), false);
    assert_eq!(query_text(&props, &state), "a\u{a0}severity_text:");
    assert!(ReportQueryEditor
        .key(&props, &mut state, &KeyPress::plain(Key::Tab), false)
        .is_none());
}
