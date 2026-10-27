/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Structured terminal authoring uses the same stored report schema and ordinary form submission.
use super::{text_of, Act, Component, ComponentState, Line, Span};
use crate::{
    keys::{Key, KeyPress},
    theme::Role,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct ReportDefinitionEditor;
pub struct ReportQueryEditor;

#[derive(Clone)]
struct Row {
    path: Vec<String>,
    label: String,
    value: Value,
}
fn pointer(path: &[String]) -> String {
    path.iter()
        .map(|part| format!("/{}", part.replace('~', "~0").replace('/', "~1")))
        .collect()
}
fn document(props: &Value, state: &ComponentState) -> Value {
    state.local.get("document").cloned().unwrap_or_else(|| {
        props["value"]
            .as_str()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_else(|| props["value"].clone())
    })
}
fn rows(props: &Value, state: &ComponentState) -> Vec<Row> {
    fn visit(value: &Value, path: Vec<String>, label: String, state: &ComponentState, out: &mut Vec<Row>) {
        if out.len() >= 2000 {
            return;
        }
        out.push(Row {
            path: path.clone(),
            label,
            value: value.clone(),
        });
        if state.closed.contains(&pointer(&path)) {
            return;
        }
        match value {
            Value::Object(fields) => {
                for (key, value) in fields {
                    let mut next = path.clone();
                    next.push(key.clone());
                    visit(value, next, key.clone(), state, out);
                }
            }
            Value::Array(items) => {
                for (at, value) in items.iter().enumerate() {
                    let mut next = path.clone();
                    next.push(at.to_string());
                    visit(
                        value,
                        next,
                        format!(
                            "[{}] {}",
                            at + 1,
                            value["type"].as_str().or_else(|| value["name"].as_str()).unwrap_or("")
                        ),
                        state,
                        out,
                    );
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    visit(
        &document(props, state),
        Vec::new(),
        "Definition".into(),
        state,
        &mut out,
    );
    out
}
fn query() -> Value {
    json!({"datasetId":"otel_logs","mode":"documents","queryString":"","pageSize":100,"sort":[],"aggregations":[],"filters":[]})
}
fn panel() -> Value {
    json!({"title":"New panel","visualization":"table","queries":[query()],"transformations":[],"fieldConfig":{"defaults":{},"overrides":[]}})
}
fn add_options(row: &Row, kind: &str) -> Vec<(String, Value)> {
    let key = row.path.last().map(String::as_str).unwrap_or("");
    if row.value.is_array() {
        return match key {
            "queries"=>vec![("Query".into(),query())],
            "filters"=>vec![("Filter pill".into(),json!({"query":{"field":{"name":"","op":"eq","value":""}},"label":"","pinned":false,"negated":false,"disabled":false}))],
            "transformations"=>vec![
                ("Organize fields".into(),json!({"type":"organize","hide":[],"rename":{},"order":[]})),
                ("Join frames".into(),json!({"type":"join","fields":[],"mode":"outer"})),
                ("Join by time".into(),json!({"type":"joinByTime","field":"time","mode":"outer"})),
                ("Pivot".into(),json!({"type":"pivot","row":"","column":"","cell":"","rowTotals":false,"columnTotals":false})),
                ("Reduce".into(),json!({"type":"reduce","calculation":"last"})),
                ("Calculate".into(),json!({"type":"calculate","left":"","right":"","operator":"+","name":"calculated"})),
                ("Filter".into(),json!({"type":"filter","field":"","operator":"eq","value":""})),
                ("Sort".into(),json!({"type":"sort","field":"","descending":false})),
                ("Limit".into(),json!({"type":"limit","count":100})),
                ("Group".into(),json!({"type":"groupBy","fields":[],"calculation":"sum"})),
                ("Running total".into(),json!({"type":"runningTotal","field":"value","name":"total"})),
                ("Moving average".into(),json!({"type":"movingAverage","field":"value","window":5,"name":"average"})),
                ("Delta".into(),json!({"type":"delta","field":"value","name":"delta"})),
                ("Percent of total".into(),json!({"type":"percentOfTotal","field":"value","name":"percent"})),
                ("Time shift".into(),json!({"type":"timeShift","field":"time","millis":0})),
                ("Previous period comparison".into(),json!({"type":"comparePrevious","field":"time","shiftMillis":0})),
                ("Merge".into(),json!({"type":"merge"})),
                ("Partition".into(),json!({"type":"partition","fields":[]})),
                ("Long to wide".into(),json!({"type":"longToWide","timeField":"time","seriesField":"series","valueField":"value"})),
                ("Wide to long".into(),json!({"type":"wideToLong","timeField":"time"})),
                ("Time series to table".into(),json!({"type":"timeSeriesToTable"}))],
            "variables"=>["constant","custom","query","groupBy"].into_iter().map(|kind|(format!("{kind} variable"),json!({"name":"variable","label":"Variable","kind":kind,"default":"","options":[],"datasetId":"","field":"","multi":false,"includeAll":false}))).collect(),
            "dashboards"=>vec![("Dashboard selection".into(),json!({"dashboardRef":"","variables":{},"range":{"from":"now-7d","thru":"now"},"panelIds":[]}))],
            "playlist"=>vec![("Playlist dashboard".into(),json!({"definitionId":""}))],
            "overrides"=>vec![("Field override".into(),json!({"match":{"name":""},"config":{}}))],
            "mappings"=>vec![("Exact mapping".into(),json!({"value":"","text":""})),("Range mapping".into(),json!({"from":0,"to":100,"text":""}))],
            "thresholds"=>vec![("Threshold".into(),json!({"value":0,"color":"negative"}))],
            "aggregations"=>["terms","dateHistogram","count","sum","avg","stats","median","percentiles","cardinality","histogram"].into_iter().map(|kind|(format!("{kind} aggregation"),json!({"id":"value","type":kind,"role":"value","params":{"field":"","size":10}}))).collect(),
            "items"|"children"=>vec![("Panel".into(),json!({"panelId":""})),("Row".into(),json!({"type":"row","title":"Row","items":[]})),("Tabs".into(),json!({"type":"tabs","title":"Tabs","items":[{"title":"Tab","layout":{"type":"autoGrid","items":[]}}]})),("Grid".into(),json!({"type":"grid","columns":2,"items":[]}))],
            _=>vec![("Text".into(),json!("")),("Number".into(),json!(0)),("Boolean".into(),json!(false)),("Object".into(),json!({})),("List".into(),json!([]))]
        };
    }
    let choices = if row.path.is_empty() {
        match kind {
            "panel" => panel(),
            "report" => {
                json!({"dashboards":[],"orientation":"portrait","layout":"grid","appendix":false,"kind":"report","playlist":[],"seconds":30})
            }
            _ => json!({"panels":{},"variables":[],"layout":{"type":"autoGrid","items":[]},"refreshSeconds":0}),
        }
    } else if key == "panels" {
        return vec![
            ("Panel".into(), panel()),
            ("Library panel".into(), json!({"panelRef":"","title":"Library panel"})),
        ];
    } else if key == "fieldConfig" {
        json!({"defaults":{},"overrides":[]})
    } else if matches!(key, "defaults" | "config") {
        json!({"unit":"","decimals":2,"noValue":"—","min":0,"max":100,"hidden":false,"link":"","mappings":[],"thresholds":[],"thresholdMode":"absolute"})
    } else if key == "match" {
        json!({"name":"","type":"","regex":""})
    } else if key == "layout" {
        json!({"type":"autoGrid","title":"Layout","columns":2,"items":[]})
    } else if row.value.get("datasetId").is_some() {
        let mut value = query();
        value["from"] = json!("now-7d");
        value["thru"] = json!("now");
        value["transformations"] = json!([]);
        value
    } else if row.value.get("visualization").is_some() {
        let mut value = panel();
        value["panelRef"] = json!("");
        value["description"] = json!("");
        value
    } else {
        json!({})
    };
    let mut options: Vec<_> = choices
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| row.value.get(*name).is_none())
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    options.push(("Add named field…".into(), Value::Null));
    options
}
fn scalar_choices(props: &Value, state: &ComponentState, row: &Row) -> Vec<String> {
    let key = row.path.last().map(String::as_str).unwrap_or("");
    let choices: Vec<String> = match key {
        "datasetId" => props["datasets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["id"].as_str())
            .map(str::to_string)
            .collect(),
        "panelRef" | "dashboardRef" => props["libraries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| item["id"].as_str())
            .map(str::to_string)
            .collect(),
        "visualization" => [
            "table",
            "timeSeries",
            "stat",
            "gauge",
            "barGauge",
            "bar",
            "pie",
            "heatmap",
            "histogram",
            "text",
            "logs",
            "stateTimeline",
            "statusHistory",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        "mode" => vec!["documents".into(), "aggregate".into()],
        "kind" => ["constant", "custom", "query", "groupBy", "report", "playlist"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        "orientation" => vec!["portrait".into(), "landscape".into()],
        "op" => ["eq", "in", "exists", "prefix", "wildcard", "range"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    let input = state.local["input"].as_str().unwrap_or("");
    choices
        .into_iter()
        .filter(|choice| choice.starts_with(input))
        .take(12)
        .collect()
}
fn write_document(props: &Value, state: &mut ComponentState, value: Value) -> Vec<Act> {
    state.local["document"] = value.clone();
    vec![Act::SetFormField {
        form: props["form"].as_str().unwrap_or("EditDefinition").into(),
        field: props["field"].as_str().unwrap_or("definitionJson").into(),
        value: serde_json::to_string_pretty(&value).unwrap_or_default(),
    }]
}
fn commit_scalar(props: &Value, state: &mut ComponentState, row: &Row) -> Vec<Act> {
    let text = state.local["input"].as_str().unwrap_or("");
    let next = match row.value {
        Value::Bool(_) => match text {
            "true" | "Y" => json!(true),
            "false" | "N" => json!(false),
            _ => {
                state.error = "Enter true or false".into();
                return Vec::new();
            }
        },
        Value::Number(_) => match text.parse::<serde_json::Number>() {
            Ok(value) => Value::Number(value),
            Err(_) => {
                state.error = "Enter a finite number".into();
                return Vec::new();
            }
        },
        Value::Null => {
            if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            }
        }
        _ => json!(text),
    };
    let mut doc = document(props, state);
    if row.path.is_empty() {
        doc = next;
    } else if let Some(value) = doc.pointer_mut(&pointer(&row.path)) {
        *value = next;
    }
    state.closed.remove("__editor_input");
    state.error.clear();
    write_document(props, state, doc)
}
impl Component for ReportDefinitionEditor {
    fn name(&self) -> &'static str {
        "report-definition-editor"
    }
    fn source(&self, _: &Value, _: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        None
    }
    fn lines(&self, props: &Value, state: &ComponentState, _width: u16, _wide: bool) -> Vec<Line> {
        let mut lines = vec![vec![Span::new(
            if props["readOnly"] == true {
                "Read-only shipped definition — save a copy to edit"
            } else {
                "Structured definition — arrows navigate, Enter edits, a adds, d removes, t changes value type, +/- moves list items"
            },
            Role::Primary,
        )]];
        if state.closed.contains("__editor_input") {
            lines.push(vec![Span::new(
                format!(
                    "Edit: {}▏ (Enter applies, Esc cancels)",
                    text_of(state.local.get("input"))
                ),
                Role::Primary,
            )]);
        }
        let all = rows(props, state);
        let at = state.at.min(all.len().saturating_sub(1));
        if state.closed.contains("__editor_input") {
            if let Some(row) = all.get(at) {
                for (index, value) in scalar_choices(props, state, row).iter().enumerate() {
                    lines.push(vec![Span::new(
                        format!(
                            "{} {value} (Tab completes)",
                            if index == state.choosing.unwrap_or(0) {
                                "▶"
                            } else {
                                " "
                            }
                        ),
                        Role::Muted,
                    )]);
                }
            }
        }

        let compact = state.closed.contains("__editor_add") || state.closed.contains("__editor_input");
        let start = if compact { at } else { at.saturating_sub(5) };
        for (index, row) in all.iter().enumerate().skip(start).take(if compact { 1 } else { 12 }) {
            let summary = if row.value.is_object() {
                format!("{{{} fields}}", row.value.as_object().map_or(0, serde_json::Map::len))
            } else if row.value.is_array() {
                format!("[{} items]", row.value.as_array().map_or(0, Vec::len))
            } else {
                row.value.to_string()
            };
            lines.push(vec![Span::new(
                format!(
                    "{}{}{}: {}",
                    if index == at { "▶ " } else { "  " },
                    "  ".repeat(row.path.len().min(12)),
                    row.label,
                    summary
                ),
                if index == at { Role::Primary } else { Role::Text },
            )]);
        }
        if state.closed.contains("__editor_add") {
            if let Some(row) = all.get(at) {
                let options = add_options(
                    row,
                    props["kind"].as_str().unwrap_or(if props["panelOnly"] == true {
                        "panel"
                    } else {
                        "dashboard"
                    }),
                );
                let chosen = state.choosing.unwrap_or(0);
                for (index, (title, _)) in options.iter().enumerate().skip(chosen.saturating_sub(4)).take(10) {
                    lines.push(vec![Span::new(
                        format!("{} {title}", if index == chosen { "▶" } else { " " }),
                        Role::Primary,
                    )]);
                }
            }
        }
        if !state.error.is_empty() {
            lines.push(vec![Span::new(&state.error, Role::Negative)]);
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        if !press.is_plain() {
            return None;
        }
        let all = rows(props, state);
        let row = all.get(state.at.min(all.len().saturating_sub(1)))?.clone();
        if state.closed.contains("__editor_input") {
            match press.key {
                Key::Tab => {
                    let choices = scalar_choices(props, state, &row);
                    if let Some(value) = choices.get(state.choosing.unwrap_or(0)) {
                        state.local["input"] = json!(value);
                    }
                    return Some(Vec::new());
                }
                Key::Up => {
                    state.choosing = Some(state.choosing.unwrap_or(0).saturating_sub(1));
                    return Some(Vec::new());
                }
                Key::Down => {
                    state.choosing = Some(
                        (state.choosing.unwrap_or(0) + 1)
                            .min(scalar_choices(props, state, &row).len().saturating_sub(1)),
                    );
                    return Some(Vec::new());
                }
                Key::Escape => {
                    state.closed.remove("__editor_input");
                    state.closed.remove("__editor_named");
                }
                Key::Enter => {
                    if state.closed.remove("__editor_named") {
                        let key = state.local["input"].as_str().unwrap_or("").to_string();
                        if key.is_empty() {
                            state.error = "Enter a field name".into();
                            return Some(Vec::new());
                        }
                        let mut doc = document(props, state);
                        if let Some(object) = doc.pointer_mut(&pointer(&row.path)).and_then(Value::as_object_mut) {
                            if object.contains_key(&key) {
                                state.error = "This field already exists".into();
                                return Some(Vec::new());
                            }
                            object.insert(key, json!(""));
                        }
                        state.closed.remove("__editor_input");
                        return Some(write_document(props, state, doc));
                    } else {
                        return Some(commit_scalar(props, state, &row));
                    }
                }
                Key::Backspace => {
                    let mut text = text_of(state.local.get("input"));
                    text.pop();
                    state.local["input"] = json!(text);
                }
                Key::Char(ch) => {
                    let mut text = text_of(state.local.get("input"));
                    if text.len() < 65536 {
                        text.push(ch);
                    }
                    state.local["input"] = json!(text);
                }
                _ => return Some(Vec::new()),
            }
            return Some(Vec::new());
        }
        if state.closed.contains("__editor_add") {
            let options = add_options(
                &row,
                props["kind"].as_str().unwrap_or(if props["panelOnly"] == true {
                    "panel"
                } else {
                    "dashboard"
                }),
            );
            let chosen = state.choosing.unwrap_or(0).min(options.len().saturating_sub(1));
            match press.key {
                Key::Up => state.choosing = Some(chosen.saturating_sub(1)),
                Key::Down => state.choosing = Some((chosen + 1).min(options.len().saturating_sub(1))),
                Key::Escape => {
                    state.closed.remove("__editor_add");
                }
                Key::Enter => {
                    let (title, value) = options.get(chosen)?.clone();
                    state.closed.remove("__editor_add");
                    state.closed.remove(&pointer(&row.path));
                    if title == "Add named field…" {
                        state.closed.insert("__editor_named".into());
                        state.closed.insert("__editor_input".into());
                        state.local["input"] = json!("");
                        return Some(Vec::new());
                    }
                    let mut doc = document(props, state);
                    let target = doc.pointer_mut(&pointer(&row.path))?;
                    if let Some(items) = target.as_array_mut() {
                        items.push(value);
                    } else if let Some(fields) = target.as_object_mut() {
                        let key = if row.path.last().is_some_and(|key| key == "panels") {
                            (1..)
                                .map(|at| format!("panel{at}"))
                                .find(|key| !fields.contains_key(key))
                                .unwrap()
                        } else {
                            title
                        };
                        fields.insert(key, value);
                    }
                    return Some(write_document(props, state, doc));
                }
                _ => {}
            }
            return Some(Vec::new());
        }
        match press.key {
            Key::Up => state.at = state.at.saturating_sub(1),
            Key::Down => state.at = (state.at + 1).min(all.len().saturating_sub(1)),
            Key::Left => {
                state.closed.insert(pointer(&row.path));
            }
            Key::Right => {
                state.closed.remove(&pointer(&row.path));
            }
            Key::Enter => {
                if row.value.is_object() || row.value.is_array() {
                    let path = pointer(&row.path);
                    if !state.closed.remove(&path) {
                        state.closed.insert(path);
                    }
                } else if props["readOnly"] != true {
                    state.local["input"] = json!(if row.value.is_string() {
                        text_of(Some(&row.value))
                    } else {
                        row.value.to_string()
                    });
                    state.closed.insert("__editor_input".into());
                }
            }
            Key::Char('a') if props["readOnly"] != true && (row.value.is_object() || row.value.is_array()) => {
                state.closed.insert("__editor_add".into());
                state.choosing = Some(0);
            }
            Key::Char('d') | Key::Delete if props["readOnly"] != true && !row.path.is_empty() => {
                let mut doc = document(props, state);
                let parent = &row.path[..row.path.len() - 1];
                let key = row.path.last()?;
                let target = doc.pointer_mut(&pointer(parent))?;
                if let Some(items) = target.as_array_mut() {
                    if let Ok(index) = key.parse::<usize>() {
                        items.remove(index);
                    }
                } else if let Some(fields) = target.as_object_mut() {
                    fields.remove(key);
                }
                state.at = state.at.saturating_sub(1);
                return Some(write_document(props, state, doc));
            }
            Key::Char('t') if props["readOnly"] != true => {
                let mut doc = document(props, state);
                let target = doc.pointer_mut(&pointer(&row.path))?;
                *target = match target {
                    Value::String(_) => json!(0),
                    Value::Number(_) => json!(false),
                    Value::Bool(_) => json!({}),
                    Value::Object(_) => json!([]),
                    Value::Array(_) => Value::Null,
                    Value::Null => json!(""),
                };
                return Some(write_document(props, state, doc));
            }
            Key::Char('+') | Key::Char('-') if props["readOnly"] != true && !row.path.is_empty() => {
                let mut doc = document(props, state);
                let key = row.path.last()?.parse::<usize>().ok()?;
                let target = doc
                    .pointer_mut(&pointer(&row.path[..row.path.len() - 1]))?
                    .as_array_mut()?;
                let next = if press.key == Key::Char('+') {
                    (key + 1).min(target.len().saturating_sub(1))
                } else {
                    key.saturating_sub(1)
                };
                target.swap(key, next);
                return Some(write_document(props, state, doc));
            }
            _ => return None,
        }
        Some(Vec::new())
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("arrows", "Navigate fields / fold containers"),
            ("Enter", "Edit selected value"),
            ("a", "Add structured field or item"),
            ("d", "Remove selected field/item"),
            ("t", "Change value type"),
            ("+/-", "Reorder list item"),
        ]
    }
}

fn query_text(props: &Value, state: &ComponentState) -> String {
    state.local["query"]
        .as_str()
        .unwrap_or_else(|| props["value"].as_str().unwrap_or(""))
        .into()
}
fn suggestions(props: &Value, state: &ComponentState) -> Vec<String> {
    let query = query_text(props, state);
    let token = query.rsplit(char::is_whitespace).next().unwrap_or("");
    if let Some((field, prefix)) = token.split_once(':') {
        props["values"][field]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| text_of(Some(value)))
            .filter(|value| value.starts_with(prefix.trim_matches('"')))
            .take(12)
            .map(|value| {
                format!(
                    "{field}:{}",
                    if value.contains(' ') {
                        format!("\"{value}\"")
                    } else {
                        value
                    }
                )
            })
            .collect()
    } else {
        props["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|field| field["searchable"] != false)
            .filter_map(|field| field["name"].as_str())
            .filter(|name| name.starts_with(token))
            .take(12)
            .map(|name| format!("{name}:"))
            .collect()
    }
}
fn query_write(props: &Value, state: &mut ComponentState, text: String) -> Vec<Act> {
    state.local["query"] = json!(text);
    state.choosing = Some(0);
    vec![Act::SetFormField {
        form: props["form"].as_str().unwrap_or("SearchOptions").into(),
        field: props["field"].as_str().unwrap_or("queryString").into(),
        value: text_of(state.local.get("query")),
    }]
}
impl Component for ReportQueryEditor {
    fn name(&self) -> &'static str {
        "report-query-editor"
    }
    fn source(&self, _: &Value, _: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        None
    }
    fn lines(&self, props: &Value, state: &ComponentState, _: u16, _: bool) -> Vec<Line> {
        let mut lines = vec![vec![Span::new(
            format!(
                "Query: {}▏ (type to edit, Tab completes, F2 selects filter pills)",
                query_text(props, state)
            ),
            Role::Primary,
        )]];
        for (at, value) in suggestions(props, state).iter().enumerate() {
            lines.push(vec![Span::new(
                format!("{} {value}", if state.choosing.unwrap_or(0) == at { "▶" } else { " " }),
                Role::Muted,
            )]);
        }
        let pills = state.local.get("filters").unwrap_or(&props["filters"]);
        for (at, pill) in pills.as_array().into_iter().flatten().enumerate() {
            lines.push(vec![Span::new(
                format!(
                    "{} {}{}{} {}",
                    if state.closed.contains("__pill_mode") && state.at == at {
                        "▶"
                    } else {
                        " "
                    },
                    if pill["pinned"] == true { "PIN " } else { "" },
                    if pill["negated"] == true { "NOT " } else { "" },
                    pill["label"].as_str().unwrap_or("Filter"),
                    pill["query"]
                ),
                if pill["disabled"] == true {
                    Role::Muted
                } else {
                    Role::Text
                },
            )]);
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _: bool) -> Option<Vec<Act>> {
        if !press.is_plain() {
            return None;
        }
        if press.key == Key::F(2) {
            if !state.closed.remove("__pill_mode") {
                state.closed.insert("__pill_mode".into());
            }
            return Some(Vec::new());
        }
        if state.closed.contains("__pill_mode") {
            let mut pills = state
                .local
                .get("filters")
                .cloned()
                .unwrap_or_else(|| props["filters"].clone());
            let list = pills.as_array_mut()?;
            match press.key {
                Key::Up => state.at = state.at.saturating_sub(1),
                Key::Down => state.at = (state.at + 1).min(list.len().saturating_sub(1)),
                Key::Char('p') | Key::Char('n') | Key::Char('d') => {
                    let pill = list.get_mut(state.at)?;
                    let key = match press.key {
                        Key::Char('p') => "pinned",
                        Key::Char('n') => "negated",
                        _ => "disabled",
                    };
                    pill[key] = json!(pill[key] != true);
                }
                Key::Delete => {
                    if state.at < list.len() {
                        list.remove(state.at);
                        state.at = state.at.min(list.len().saturating_sub(1));
                    }
                }
                Key::Escape => {
                    state.closed.remove("__pill_mode");
                    return Some(Vec::new());
                }
                _ => return None,
            }
            state.local["filters"] = pills.clone();
            return Some(vec![Act::SetFormField {
                form: props["form"].as_str().unwrap_or("SearchOptions").into(),
                field: props["filtersField"].as_str().unwrap_or("filtersJson").into(),
                value: pills.to_string(),
            }]);
        }
        let mut text = query_text(props, state);
        match press.key {
            Key::Char(ch) => text.push(ch),
            Key::Backspace => {
                text.pop();
            }
            Key::Up => {
                state.choosing = Some(state.choosing.unwrap_or(0).saturating_sub(1));
                return Some(Vec::new());
            }
            Key::Down => {
                state.choosing =
                    Some((state.choosing.unwrap_or(0) + 1).min(suggestions(props, state).len().saturating_sub(1)));
                return Some(Vec::new());
            }
            Key::Tab => {
                let choices = suggestions(props, state);
                let choice = choices.get(state.choosing.unwrap_or(0))?;
                let start = text.rfind(char::is_whitespace).map_or(0, |at| at + 1);
                if &text[start..] == choice {
                    return None;
                }
                text.replace_range(start.., choice);
            }
            _ => return None,
        }
        Some(query_write(props, state, text))
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("type", "Edit search query"),
            ("Tab", "Complete authorized field/value"),
            ("↑/↓", "Choose suggestion or filter"),
            ("F2", "Toggle query / filter pills"),
            ("p/n/d", "Pin / negate / disable filter"),
            ("Delete", "Remove selected filter"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
                add_options(&row, "dashboard")
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
}
