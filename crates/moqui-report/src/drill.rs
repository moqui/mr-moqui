//! Drill: from a value of a saved panel to its breakdown and the records behind it (BOSS impl-insights P6, INS-04;
//! design-insights §4 Drill; docs/tools/Monitoring.md "Drill").
//!
//! The request names a saved definition, a panel, one of the panel's queries and the buckets the viewer chose; it never
//! carries a query, an entity or a service. The engine reads the definition as the viewer, runs the panel as its dashboard
//! does (the variables given fill only the saved variables, and each dataset's admission and row filter apply), and takes
//! each chosen bucket's predicate from the result it computed itself, so an "Other" or a missing bucket keeps its exact
//! predicate (everything but the buckets shown, a field that is absent). The breakdown and the records are then the saved
//! query narrowed by those predicates: the same scope, the same filters, the same range.
//!
//! A viewer who reads a dataset's aggregates and not its records (`rows: "restricted"`, semantics.rs) gets the breakdown
//! and an explanation; no record is fetched. A panel the viewer cannot read answers as unavailable with its reason, never
//! with what it would have shown.
use super::{
    dashboard_frames_filtered, dataset, definitions, documents_of, execute_query, fail, input, json, metadata, output,
    search, select_panel, semantics, text, Error, ExecutionContext, Json, Parameters, Query, ServiceInfo,
};

/// The most buckets a drill selects, rows of a result one selection may span, and records of a page.
const SELECTION_LIMIT: usize = 8;
const SPAN_LIMIT: usize = 200;
const PAGE_LIMIT: u64 = 100;
/// The records a drill pages through: past these the viewer narrows the selection (a deep page is a scan of all before it).
const WINDOW_LIMIT: u64 = 10_000;

fn plain(value: &Json) -> String {
    match value {
        Json::Null => String::new(),
        Json::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The queries the panel ran, as the engine resolved them (variables applied, the range set): the current period's.
fn resolved_requests(panel: &Json) -> Vec<Json> {
    let mut out = Vec::new();
    fn collect(entries: &Json, out: &mut Vec<Json>) {
        for entry in entries.as_array().into_iter().flatten() {
            if entry["period"] == "previous" {
                continue;
            }
            if entry["request"].is_object() {
                out.push(entry["request"].clone());
            } else {
                collect(&entry["requests"], out);
            }
        }
    }
    collect(&panel["requests"], &mut out);
    out
}

/// The predicate of the chosen buckets, from the panel's own result: the rows where every chosen field has the chosen
/// value, and for each chosen field the predicates the engine recorded for those rows.
fn selection(panel: &Json, dataset_id: &str, selected: &[Json]) -> Result<(Vec<Json>, Vec<Json>), Error> {
    if selected.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    if selected.len() > SELECTION_LIMIT {
        return Err(fail(format!("a drill selects at most {SELECTION_LIMIT} buckets")));
    }
    let matches = |field: &Json, index: usize, value: &Json| {
        let shown = &field["values"][index];
        let label = &field["config"]["bucket"]["labels"][index];
        if value.is_null() {
            shown.is_null()
        } else {
            plain(shown) == plain(value) || (!label.is_null() && plain(label) == plain(value))
        }
    };
    for frame in panel["frames"].as_array().into_iter().flatten() {
        let fields = frame["fields"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let mut chosen = Vec::new();
        for choice in selected {
            let name = text(choice, "field", "");
            let Some(field) = fields.iter().find(|field| {
                field["name"] == name
                    && field["config"]["bucket"].is_object()
                    && field["config"]["bucket"]["datasetId"] == dataset_id
            }) else {
                break;
            };
            chosen.push((field, &choice["value"]));
        }
        if chosen.len() != selected.len() {
            continue;
        }
        let length = chosen[0].0["values"].as_array().map_or(0, Vec::len);
        let rows = (0..length)
            .filter(|index| chosen.iter().all(|(field, value)| matches(field, *index, value)))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            continue;
        }
        let mut predicates = Vec::new();
        let mut crumbs = Vec::new();
        for (field, value) in &chosen {
            let bucket = &field["config"]["bucket"];
            let mut unique = std::collections::BTreeSet::new();
            let mut queries = Vec::new();
            for index in &rows {
                let filter = &bucket["filters"][*index];
                if filter.is_null() || !unique.insert(filter.to_string()) {
                    continue;
                }
                if queries.len() >= SPAN_LIMIT {
                    return Err(fail(format!(
                        "the selection spans more than {SPAN_LIMIT} buckets; choose a bucket of each group"
                    )));
                }
                queries.push(Query::from_value(&moqui_lib::json::from_json(filter))?);
            }
            if queries.is_empty() {
                return Err(fail(format!(
                    "{} has no source predicate in this panel's result",
                    plain(&field["name"])
                )));
            }
            predicates.push(moqui_lib::json::to_json(&Query::any(queries).to_value()));
            let label = &bucket["labels"][rows[0]];
            crumbs.push(
                json!({"field":field["name"],"sourceField":bucket["field"],"kind":bucket["kind"],
                "label":if label.is_null() { plain(value) } else { plain(label) }}),
            );
        }
        return Ok((predicates, crumbs));
    }
    Err(fail(
        "the selected bucket is not in this panel's result for this range and scope",
    ))
}

/// The saved query narrowed by the chosen buckets: its dataset, query, filters and range, nothing from the request.
fn narrowed(base: &Json, predicates: &[Json], from: &Json, thru: &Json) -> Json {
    let mut query = json!({"datasetId":base["datasetId"],"from":if base["from"].is_null(){from}else{&base["from"]},
        "thru":if base["thru"].is_null(){thru}else{&base["thru"]}});
    for key in ["query", "queryString"] {
        if !base[key].is_null() {
            query[key] = base[key].clone();
        }
    }
    let mut filters = base["filters"].as_array().cloned().unwrap_or_default();
    filters.extend(predicates.iter().map(|predicate| json!({"query":predicate})));
    query["filters"] = json!(filters);
    query
}

pub fn get_drill(ec: &ExecutionContext, _service: &ServiceInfo<'_>, params: &Parameters) -> Result<Parameters, Error> {
    let params = input(params);
    let definition_id = text(&params, "definitionId", "");
    let mut definition = definitions::read(ec, definition_id)?;
    let panel_id = match text(&definition, "type", "") {
        "panel" => {
            // a library panel drills as a dashboard of itself, as it renders
            definition["body"] = json!({"panels":{definition_id:{"panelRef":definition_id}},
                "layout":{"type":"autoGrid","items":[{"panelId":definition_id}]}});
            definition_id.to_owned()
        }
        "dashboard" => text(&params, "panelId", "").to_owned(),
        _ => return Err(fail("a drill starts from a dashboard panel or a library panel")),
    };
    if panel_id.is_empty() {
        return Err(fail("a drill names the panel it starts from"));
    }
    if !definition["body"].is_object() {
        return Err(fail("dashboard body must be an object"));
    }
    select_panel(&mut definition, &panel_id)?;
    definition["body"]["inspect"] = json!(true);
    let range = &params["range"];
    let from = range["from"]
        .as_str()
        .or_else(|| params["from"].as_str())
        .unwrap_or("now-7d");
    let thru = range["thru"]
        .as_str()
        .or_else(|| params["thru"].as_str())
        .unwrap_or("now");
    let rendered = dashboard_frames_filtered(ec, &definition, &params["variables"], from, thru, &json!([]))?;
    let panel = rendered["panels"]
        .get(&panel_id)
        .or_else(|| rendered["panels"].as_object().and_then(|panels| panels.values().next()))
        .cloned()
        .ok_or_else(|| fail("dashboard panel does not exist"))?;
    let mut answer = json!({"definitionId":definition_id,"panelId":panel_id,"title":panel["title"],"range":rendered["range"],
        "available":true,"selected":[],"rows":[],"rowsAvailable":false,"notices":[]});
    let unavailable = |mut answer: Json, reason: String| {
        answer["available"] = json!(false);
        answer["reason"] = json!(reason);
        Ok(output(answer))
    };
    // a panel the viewer cannot read says why (its dataset is not theirs, or not admitted), and nothing else
    if let Some(refused) = panel["notices"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|notice| notice["level"] == "error" && notice["complete"] == false)
    {
        if panel["frames"].as_array().is_none_or(Vec::is_empty) {
            return unavailable(answer, plain(&refused["message"]));
        }
    }
    // the panel's own result, whose buckets are the ones a viewer may choose
    answer["panelFrames"] = panel["frames"].clone();
    let requests = resolved_requests(&panel);
    let reference = &params["queryReference"];
    let index = match reference {
        Json::Null => Some(0),
        Json::Number(number) => number.as_u64().map(|index| index as usize),
        Json::String(text) if text.is_empty() => Some(0),
        Json::String(text) => text.parse::<usize>().ok().or_else(|| {
            requests
                .iter()
                .position(|request| request["refId"] == *text || request["id"] == *text)
        }),
        _ => None,
    };
    let Some(base) = index.and_then(|index| requests.get(index)) else {
        if requests.is_empty() {
            return unavailable(
                answer,
                "This panel's value is not an aggregate of a dataset, so it has no drill".into(),
            );
        }
        return Err(fail("the drill's query is not one of this panel's"));
    };
    let dataset_id = text(base, "datasetId", "").to_owned();
    answer["queryReference"] = json!(index);
    answer["datasetId"] = json!(dataset_id);
    let selected = params["selectedBuckets"].as_array().cloned().unwrap_or_default();
    let (predicates, crumbs) = selection(&panel, &dataset_id, &selected)?;
    answer["selected"] = json!(crumbs);
    let resolved = dataset(ec, &dataset_id)?;
    // the dimensions a breakdown may add: the dataset's own aggregatable fields
    let facade = search::current()?;
    let mut dimensions = std::collections::BTreeSet::new();
    metadata(ec, || {
        for document in facade.target_documents(&resolved.target)? {
            for (name, field) in &facade.mapping(&document)?.root.fields {
                if field.can_sort() && field.kind != search::FieldKind::Binary {
                    dimensions.insert(name.clone());
                }
            }
        }
        Ok(())
    })?;
    answer["dimensions"] = json!(dimensions);
    let narrowed_query = narrowed(
        base,
        &predicates,
        &rendered["range"]["from"],
        &rendered["range"]["thru"],
    );
    if let Some(dimension) = params["breakdownDimension"].as_str().filter(|name| !name.is_empty()) {
        if !dimensions.contains(dimension) {
            return Err(fail(format!("{dimension} is not a dimension of dataset {dataset_id}")));
        }
        let mut query = narrowed_query.clone();
        let mut aggregations =
            vec![json!({"id":dimension,"type":"terms","role":"x-axis","params":{"field":dimension,"size":20}})];
        // the saved query's own measures, so the breakdown's values are the panel's
        aggregations.extend(
            base["aggregations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|aggregation| text(aggregation, "role", "metric") == "metric")
                .cloned(),
        );
        query["aggregations"] = json!(aggregations);
        answer["breakdown"] = match execute_query(ec, &query) {
            Ok(result) => {
                json!({"dimension":dimension,"frames":result["frames"],"notices":result["notices"],"sourceCount":result["sourceCount"]})
            }
            // a breakdown the guards refuse (amounts of several units) says so; the records are still read
            Err(error) => json!({"dimension":dimension,"frames":[],"refused":error.message()}),
        };
    }
    // the records: only for a viewer who reads the dataset's records, and nothing is fetched otherwise
    if let Err(error) = semantics::check_rows(ec, &resolved) {
        answer["rowsReason"] = json!(error.message());
        return Ok(output(answer));
    }
    let page_size = params["pageSize"].as_u64().unwrap_or(20).clamp(1, PAGE_LIMIT);
    let page_index = params["pageIndex"].as_u64().unwrap_or(0);
    if page_index.saturating_mul(page_size) >= WINDOW_LIMIT {
        return Err(fail(format!(
            "a drill pages through at most {WINDOW_LIMIT} records; choose a bucket or a shorter range"
        )));
    }
    let mut rows_query = narrowed_query;
    rows_query["pageSize"] = json!(page_size);
    rows_query["pageIndex"] = json!(page_index);
    let found = documents_of(ec, &rows_query, &resolved)?;
    let rows = found["hits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|hit| json!({"id":hit["id"],"documentId":hit["documentId"],"title":hit["documentTitle"],"links":hit["links"],"body":hit["body"]}))
        .collect::<Vec<_>>();
    let total = found["total"].as_u64().unwrap_or(0);
    // the columns that explain a record's part in the value: the time, the groups and the measured fields of the query
    let mut columns = vec![json!(resolved.time_field)];
    for aggregation in base["aggregations"].as_array().into_iter().flatten() {
        if let Some(field) = aggregation["params"]["field"].as_str() {
            if !columns.iter().any(|column| column == field) {
                columns.push(json!(field));
            }
        }
    }
    answer["columns"] = json!(columns);
    answer["rowsAvailable"] = json!(true);
    answer["total"] = json!(total);
    answer["totalRelation"] = found["totalRelation"].clone();
    answer["pageIndex"] = json!(page_index);
    answer["pageSize"] = json!(page_size);
    // a page is a sample of the matching records, and says so
    answer["bounded"] = json!(total > rows.len() as u64);
    answer["rows"] = json!(rows);
    Ok(output(answer))
}
