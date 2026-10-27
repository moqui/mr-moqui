/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Reporting twins consume the same authorized columnar frames as the browser and exports.
use super::{fit, prop, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::{
    keys::{Key, KeyPress},
    theme::Role,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub struct ReportPanel;
pub struct ReportDashboard;
pub struct TraceWaterfall;
pub struct LogTail;
pub struct ReportPlaylist;

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn integer(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn formatted(value: &Value, field: &Value) -> String {
    if value.is_null() {
        return field["noValue"].as_str().unwrap_or("—").into();
    }
    for mapping in field["mappings"].as_array().into_iter().flatten() {
        let exact = mapping
            .get("value")
            .is_some_and(|mapped| mapped == value || text_of(Some(mapped)) == text_of(Some(value)));
        let range = number(value)
            .zip(number(&mapping["from"]))
            .zip(number(&mapping["to"]))
            .is_some_and(|((value, from), to)| value >= from && value <= to);
        if exact || range {
            if let Some(text) = mapping["text"].as_str() {
                return text.into();
            }
        }
    }
    if field["type"] == "time" {
        return value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
            .map_or_else(|| text_of(Some(value)), |time| time.to_string());
    }
    if field["type"]
        .as_str()
        .is_some_and(|kind| kind != "number" && kind != "integer")
    {
        return text_of(Some(value));
    }
    if let Some(mut value) = number(value) {
        let suffix = match field["unit"].as_str().unwrap_or("") {
            "ns" | "duration" => {
                value /= 1_000_000.0;
                " ms"
            }
            "ms" => " ms",
            "s" => " s",
            "percent" | "%" => "%",
            "bytes" => {
                let units = [" B", " KiB", " MiB", " GiB", " TiB"];
                let mut at = 0;
                while value.abs() >= 1024.0 && at < units.len() - 1 {
                    value /= 1024.0;
                    at += 1;
                }
                units[at]
            }
            _ => "",
        };
        let decimals = field["decimals"]
            .as_u64()
            .unwrap_or(if value.fract() == 0.0 { 0 } else { 2 })
            .min(12) as usize;
        return format!("{value:.decimals$}{suffix}");
    }
    text_of(Some(value))
}

fn all_configured_fields(frame: &Value, panel: &Value) -> Vec<Value> {
    frame["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|field| {
            let mut configured = panel["fieldConfig"]["defaults"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            if let Some(metadata) = field["config"].as_object() {
                configured.extend(metadata.clone());
            }
            if let Some(original) = field.as_object() {
                configured.extend(original.clone());
            }
            let mut field = Value::Object(configured);
            for item in panel["fieldConfig"]["overrides"].as_array().into_iter().flatten() {
                let original = field.clone();
                let matcher = &item["match"];
                let matches = matcher["name"] == original["name"] && matcher["name"].is_string()
                    || matcher["type"] == original["type"] && matcher["type"].is_string()
                    || matcher["regex"]
                        .as_str()
                        .and_then(|pattern| regex::Regex::new(pattern).ok())
                        .is_some_and(|pattern| pattern.is_match(original["name"].as_str().unwrap_or("")));
                if matches {
                    if let (Some(target), Some(config)) = (field.as_object_mut(), item["config"].as_object()) {
                        target.extend(config.clone());
                    }
                }
            }
            field
        })
        .collect()
}

fn configured_fields(frame: &Value, panel: &Value) -> Vec<Value> {
    all_configured_fields(frame, panel)
        .into_iter()
        .filter(|field| field["hidden"] != true)
        .collect()
}

fn split_role(role: &str) -> bool {
    matches!(
        role,
        "series" | "split-series" | "splitSeries" | "splitRows" | "splitChart" | "split-rows" | "split-chart"
    )
}

fn declared_roles(value: &Value, roles: &mut BTreeMap<String, String>) {
    for aggregation in value["aggregations"].as_array().into_iter().flatten() {
        if let Some(role) = aggregation["role"].as_str() {
            for name in [
                aggregation["id"].as_str(),
                aggregation["field"].as_str(),
                aggregation["params"]["field"].as_str(),
            ]
            .into_iter()
            .flatten()
            {
                roles.insert(name.into(), role.into());
            }
        }
        declared_roles(aggregation, roles);
    }
    for query in value["queries"].as_array().into_iter().flatten() {
        declared_roles(query, roles);
    }
}

fn field_role<'a>(field: &'a Value, roles: &'a BTreeMap<String, String>) -> &'a str {
    field["role"]
        .as_str()
        .or_else(|| field["config"]["role"].as_str())
        .or_else(|| {
            let name = field["name"].as_str().unwrap_or("");
            roles
                .get(name)
                .or_else(|| roles.get(name.rsplit('.').next().unwrap_or(name)))
                .or_else(|| field["bucket"]["field"].as_str().and_then(|name| roles.get(name)))
                .map(String::as_str)
        })
        .unwrap_or("")
}

fn numeric_field(field: &Value) -> bool {
    matches!(field["type"].as_str(), Some("number" | "integer"))
}

struct TrendSeries {
    label: String,
    field: Value,
    values: Vec<Value>,
    rows: Vec<Option<usize>>,
}

struct Trends {
    series: Vec<TrendSeries>,
    limited: bool,
}

/// Split before drawing: absent coordinates remain null and duplicate observations
/// get independent tracks. Never invent a sum, a zero, or a last-value reduction.
fn trend_series(frame: &Value, panel: &Value) -> Trends {
    const MAX_POINTS: usize = 4096;
    const MAX_SERIES: usize = 128;
    let fields = all_configured_fields(frame, panel);
    let mut roles = BTreeMap::new();
    for query in panel["queries"].as_array().into_iter().flatten() {
        declared_roles(query, &mut roles);
    }
    let metadata = |field: &Value| is_metadata(field) && field_role(field, &roles).is_empty();
    let x = fields
        .iter()
        .position(|field| !metadata(field) && field["type"] == "time")
        .or_else(|| {
            fields
                .iter()
                .position(|field| !metadata(field) && matches!(field_role(field, &roles), "x" | "x-axis" | "xAxis"))
        })
        .or_else(|| {
            fields.iter().position(|field| {
                !metadata(field) && field["bucket"].is_object() && !split_role(field_role(field, &roles))
            })
        })
        .or_else(|| {
            fields
                .iter()
                .position(|field| !metadata(field) && !numeric_field(field) && !split_role(field_role(field, &roles)))
        });
    let documents = panel["queries"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|query| matches!(query["mode"].as_str(), Some("documents" | "document")));
    let dimensions: Vec<usize> = fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            (Some(index) != x
                && !metadata(field)
                && (split_role(field_role(field, &roles))
                    || field["bucket"].is_object()
                    || (!documents && !numeric_field(field))))
            .then_some(index)
        })
        .collect();
    let metrics: Vec<usize> = fields
        .iter()
        .enumerate()
        .filter_map(|(index, field)| {
            (Some(index) != x
                && !dimensions.contains(&index)
                && numeric_field(field)
                && !metadata(field)
                && field["hidden"] != true)
                .then_some(index)
        })
        .collect();
    let count = fields
        .iter()
        .map(|field| field["values"].as_array().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    let at = |field: usize, row: usize| fields[field]["values"].get(row).cloned().unwrap_or(Value::Null);
    let coordinate = |row: usize| x.map_or_else(|| Value::from(row), |index| at(index, row));
    let mut coordinates = Vec::new();
    let mut seen = BTreeSet::new();
    let mut limited = false;
    for row in 0..count {
        let coordinate = coordinate(row);
        if seen.insert(coordinate.to_string()) {
            if coordinates.len() == MAX_POINTS {
                limited = true;
                break;
            }
            coordinates.push(coordinate);
        }
    }
    if x.is_some_and(|index| fields[index]["type"] == "time" || numeric_field(&fields[index])) {
        let time = |value: &Value| {
            number(value).or_else(|| {
                value
                    .as_str()?
                    .parse::<jiff::Timestamp>()
                    .ok()
                    .and_then(|time| time.as_millisecond().to_string().parse().ok())
            })
        };
        coordinates.sort_by(|a, b| match (time(a), time(b)) {
            (Some(a), Some(b)) => a.total_cmp(&b),
            _ => a.to_string().cmp(&b.to_string()),
        });
    }
    let positions: BTreeMap<String, usize> = coordinates
        .iter()
        .enumerate()
        .map(|(index, coordinate)| (coordinate.to_string(), index))
        .collect();
    let mut groups: BTreeMap<String, (String, Vec<Vec<usize>>)> = BTreeMap::new();
    for row in 0..count {
        let Some(&position) = positions.get(&coordinate(row).to_string()) else {
            continue;
        };
        let values: Vec<Value> = dimensions.iter().map(|&index| at(index, row)).collect();
        // Date gap filling creates rows without a series identity. Keep their x
        // coordinates as gaps, rather than manufacture a Missing group.
        if !dimensions.is_empty()
            && values.iter().all(Value::is_null)
            && metrics.iter().all(|&index| at(index, row).is_null())
            && !dimensions.iter().any(|&index| {
                fields[index]["bucket"]["filters"]
                    .get(row)
                    .is_some_and(|value| !value.is_null())
                    || fields[index]["bucket"]["labels"]
                        .get(row)
                        .is_some_and(|value| !value.is_null())
            })
        {
            continue;
        }
        let key = Value::Array(values.clone()).to_string();
        if !groups.contains_key(&key) && groups.len() == MAX_SERIES {
            limited = true;
            continue;
        }
        let group = groups.entry(key).or_insert_with(|| {
            let label = dimensions
                .iter()
                .zip(&values)
                .map(|(&index, value)| {
                    format!(
                        "{}={}",
                        fields[index]["name"].as_str().unwrap_or(""),
                        formatted(value, &fields[index])
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            (label, vec![Vec::new(); coordinates.len()])
        });
        if group.1[position].len() < MAX_SERIES {
            group.1[position].push(row);
        } else {
            limited = true;
        }
    }
    let mut result = Trends {
        series: Vec::new(),
        limited,
    };
    for (label, points) in groups.values() {
        let tracks = points.iter().map(Vec::len).max().unwrap_or(0);
        for &metric in &metrics {
            for track in 0..tracks {
                if result.series.len() == MAX_SERIES {
                    result.limited = true;
                    return result;
                }
                let rows: Vec<Option<usize>> = points.iter().map(|point| point.get(track).copied()).collect();
                let values = rows
                    .iter()
                    .map(|row| row.map_or(Value::Null, |row| at(metric, row)))
                    .collect();
                let field: Value = ["name", "type", "unit", "decimals", "noValue", "mappings"]
                    .into_iter()
                    .filter_map(|key| fields[metric].get(key).map(|value| (key.to_string(), value.clone())))
                    .collect();
                let name = fields[metric]["name"].as_str().unwrap_or("");
                let mut label = if label.is_empty() {
                    name.into()
                } else {
                    format!("{name} {{{label}}}")
                };
                if tracks > 1 {
                    label.push_str(&format!(" [observation {}]", track + 1));
                }
                result.series.push(TrendSeries {
                    label,
                    field,
                    values,
                    rows,
                });
            }
        }
    }
    result
}

fn trend_lines(frame: &Value, panel: &Value, width: usize, stat: bool) -> Vec<Line> {
    let trends = trend_series(frame, panel);
    let mut lines = Vec::new();
    for series in trends.series {
        let value = if stat {
            format!(
                "{} ",
                formatted(series.values.last().unwrap_or(&Value::Null), &series.field)
            )
        } else {
            String::new()
        };
        lines.push(vec![Span::new(
            format!("{} {}{}", series.label, value, sparkline(&series.values, width)),
            Role::Primary,
        )]);
        // The table below retains original rows for cell links and filter actions.
        let rows = series
            .rows
            .iter()
            .flatten()
            .take(20)
            .map(|row| (row + 1).to_string())
            .collect::<Vec<_>>()
            .join(",");
        lines.push(vec![Span::new(
            format!(
                "  source rows: {rows}{}",
                if series.rows.iter().flatten().count() > 20 {
                    ",…"
                } else {
                    ""
                }
            ),
            Role::Muted,
        )]);
    }
    if trends.limited {
        lines.push(vec![Span::new(
            "Trend preview limited to 128 series / 4096 coordinates; inspect table or export for remaining data",
            Role::Warning,
        )]);
    }
    lines
}

fn sparkline(values: &[Value], width: usize) -> String {
    let values: Vec<Option<f64>> = values.iter().map(number).collect();
    let min = values.iter().flatten().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().flatten().copied().fold(f64::NEG_INFINITY, f64::max);
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if values.is_empty() {
        return String::new();
    }
    (0..width.min(values.len()))
        .map(|at| {
            let size = width.min(values.len());
            let start = at * values.len() / size;
            let end = (at + 1) * values.len() / size;
            let point = if values[start..end].iter().any(Option::is_none) {
                None
            } else {
                values[end - 1]
            };
            match point {
                None => ' ',
                Some(value) => {
                    let scaled = if max > min {
                        (value - min) / (max - min) * 7.0
                    } else {
                        3.0
                    };
                    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let index = scaled.round() as usize;
                    bars[index.min(7)]
                }
            }
        })
        .collect()
}

fn is_metadata(field: &Value) -> bool {
    let name = field["name"].as_str().unwrap_or("");
    let bucket_flag = name
        .split_once('.')
        .map_or(matches!(name, "missing" | "other"), |(base, suffix)| {
            matches!(base, "missing" | "other")
                && !suffix.is_empty()
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
        });
    matches!(name, "partial" | "__subtotal" | "__level") || (field["type"] == "boolean" && bucket_flag)
}

fn panel_lines(frame: &Value, panel: &Value, width: u16) -> Vec<Line> {
    let mut lines = Vec::new();
    let title = panel["title"].as_str().unwrap_or("Report");
    lines.push(vec![Span::new(title, Role::Text).bold()]);
    if panel["visualization"] == "text" || panel["type"] == "text" {
        lines.extend(
            panel["text"]
                .as_str()
                .unwrap_or("")
                .lines()
                .map(|line| vec![Span::new(line, Role::Text)]),
        );
        return lines;
    }
    let fields = configured_fields(frame, panel);
    let count = fields
        .iter()
        .map(|field| field["values"].as_array().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    if count == 0 {
        lines.push(vec![Span::new("No data in this time range", Role::Muted)]);
        return lines;
    }
    for annotation in panel["annotations"].as_array().into_iter().flatten().take(20) {
        lines.push(vec![Span::new(
            format!(
                "↳ {} {}",
                text_of(annotation.get("time")),
                text_of(annotation.get("text"))
            ),
            Role::Muted,
        )]);
    }
    let visual = panel["visualization"].as_str().unwrap_or("table");
    if matches!(
        visual,
        "stat" | "gauge" | "barGauge" | "timeSeries" | "timeseries" | "line" | "area"
    ) {
        lines.extend(trend_lines(
            frame,
            panel,
            usize::from(width).saturating_sub(20).min(60),
            matches!(visual, "stat" | "gauge" | "barGauge"),
        ));
        if matches!(visual, "stat" | "gauge" | "barGauge") {
            return lines;
        }
    }
    let cols = fields.len().min(8);
    if cols == 0 {
        return lines;
    }
    let col_width = (usize::from(width).saturating_sub(cols * 2) / cols).clamp(4, 36);
    lines.push(vec![Span::new(
        fields
            .iter()
            .take(cols)
            .map(|field| fit(field["name"].as_str().unwrap_or(""), col_width))
            .collect::<Vec<_>>()
            .join("  "),
        Role::Text,
    )
    .bold()]);
    for at in 0..count.min(100) {
        let cells: Vec<String> = fields
            .iter()
            .take(cols)
            .map(|field| fit(&formatted(&field["values"][at], field), col_width))
            .collect();
        lines.push(vec![Span::new(cells.join("  "), Role::Text)]);
    }
    if count > 100 {
        lines.push(vec![Span::new(
            format!("Showing 100 of {count} rows; use export for all rows"),
            Role::Muted,
        )]);
    }
    lines
}

impl Component for ReportPanel {
    fn name(&self) -> &'static str {
        "report-panel"
    }
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let data = state.data.as_ref().unwrap_or(props);
        let frame = data.get("frame").or_else(|| data["frames"].get(0)).unwrap_or(data);
        let frame = table_frame(frame, state);
        let mut lines = panel_lines(&frame, &props["panel"], width);
        lines.insert(0, selection_line(&frame, &props["panel"], state));
        if state.closed.contains("__report_inspector") {
            lines.extend(
                serde_json::to_string_pretty(data)
                    .unwrap_or_default()
                    .lines()
                    .take(300)
                    .map(|line| vec![Span::new(line, Role::Muted)]),
            );
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        if press.key == Key::Char('i') && press.is_plain() {
            if !state.closed.remove("__report_inspector") {
                state.closed.insert("__report_inspector".into());
            }
            return Some(Vec::new());
        }
        let data = state.data.as_ref().unwrap_or(props);
        let frame = data
            .get("frame")
            .or_else(|| data["frames"].get(0))
            .unwrap_or(data)
            .clone();
        if let Some(acts) = table_key(&frame, &props["panel"], state, press, None) {
            return Some(acts);
        }
        (press.key == Key::Char('r') && press.is_plain()).then(|| {
            vec![if source_of(props, state).is_some() {
                Act::Reload
            } else {
                Act::ReloadScreen
            }]
        })
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("r", "Refresh report"),
            ("i", "Inspect frame and query"),
            ("arrows", "Select table cell"),
            ("s", "Sort selected column"),
            ("Enter", "Open selected data link"),
            ("f", "Filter by selected dimension bucket"),
        ]
    }
}

fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}
fn all_dashboard_entries(data: &Value) -> Vec<(String, &Value)> {
    if let Some(panels) = data["dashboard"]["panels"].as_object() {
        panels.iter().map(|(id, panel)| (id.clone(), panel)).collect()
    } else {
        data["dashboard"]["panels"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(at, panel)| {
                (
                    panel["id"].as_str().map_or_else(|| at.to_string(), str::to_string),
                    panel,
                )
            })
            .collect()
    }
}
#[derive(Clone)]
struct LayoutControl {
    path: String,
    title: String,
    count: usize,
    active: usize,
    row: bool,
    closed: bool,
}
fn layout_view(data: &Value, state: &ComponentState) -> (BTreeSet<String>, Vec<LayoutControl>) {
    fn visit(
        node: &Value,
        path: String,
        state: &ComponentState,
        ids: &mut BTreeSet<String>,
        controls: &mut Vec<LayoutControl>,
        row_parent: bool,
    ) {
        let items = node["items"].as_array().or_else(|| node["children"].as_array());
        let kind = node["type"].as_str().unwrap_or("");
        let count = items.map_or(0, Vec::len);
        let row = row_parent || kind == "row";
        let tabs = kind == "tabs";
        let closed = state.closed.contains(&format!("layout:{path}")) != (node["collapsed"] == true);
        let active = state.local["tabs"][&path]
            .as_u64()
            .map(|value| value as usize)
            .unwrap_or_else(|| {
                if path == "layout" {
                    state.choosing.unwrap_or(0)
                } else {
                    0
                }
            })
            .min(count.saturating_sub(1));
        if tabs || row {
            controls.push(LayoutControl {
                path: path.clone(),
                title: node["title"]
                    .as_str()
                    .unwrap_or(if row { "Row" } else { "Tabs" })
                    .into(),
                count,
                active,
                row,
                closed,
            });
        }
        if row && closed {
            return;
        }
        if let Some(id) = node.as_str().or_else(|| node["panelId"].as_str()) {
            ids.insert(id.into());
        }
        if let Some(layout) = node.get("layout") {
            visit(layout, format!("{path}/layout"), state, ids, controls, false);
        }
        for (at, item) in items.into_iter().flatten().enumerate() {
            if !tabs || at == active {
                visit(item, format!("{path}/{at}"), state, ids, controls, kind == "rows");
            }
        }
    }
    let mut ids = BTreeSet::new();
    let mut controls = Vec::new();
    visit(
        &data["dashboard"]["layout"],
        "layout".into(),
        state,
        &mut ids,
        &mut controls,
        false,
    );
    (ids, controls)
}
fn dashboard_entries<'a>(data: &'a Value, state: &ComponentState) -> Vec<(String, &'a Value)> {
    let entries = all_dashboard_entries(data);
    let (ids, controls) = layout_view(data, state);
    if ids.is_empty() && controls.is_empty() {
        entries
    } else {
        entries.into_iter().filter(|(id, _)| ids.contains(id)).collect()
    }
}
fn table_frame(frame: &Value, state: &ComponentState) -> Value {
    let mut frame = frame.clone();
    let Some(name) = state.local["sort"].as_str() else {
        return frame;
    };
    let Some(values) = frame["fields"]
        .as_array()
        .and_then(|fields| fields.iter().find(|field| field["name"] == name))
        .and_then(|field| field["values"].as_array())
    else {
        return frame;
    };
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|left, right| {
        let a = &values[*left];
        let b = &values[*right];
        let result = match (number(a), number(b)) {
            (Some(a), Some(b)) => a.total_cmp(&b),
            _ => text_of(Some(a)).cmp(&text_of(Some(b))),
        };
        let result = if state.local["descending"] == true {
            result.reverse()
        } else {
            result
        };
        result.then(left.cmp(right))
    });
    for field in frame["fields"].as_array_mut().into_iter().flatten() {
        let values = field["values"].clone();
        field["values"] = Value::Array(order.iter().map(|at| values[*at].clone()).collect());
        // Canonical bucket predicates are row metadata too. A visual sort must never
        // attach the Missing/Other predicate of another row to the selected value.
        for path in ["/bucket", "/config/bucket"] {
            if let Some(bucket) = field.pointer_mut(path) {
                for key in ["filters", "labels"] {
                    if let Some(values) = bucket
                        .get(key)
                        .and_then(Value::as_array)
                        .filter(|values| values.len() == order.len())
                        .cloned()
                    {
                        bucket[key] = Value::Array(order.iter().map(|at| values[*at].clone()).collect());
                    }
                }
            }
        }
    }
    frame
}
fn table_key(
    frame: &Value,
    panel: &Value,
    state: &mut ComponentState,
    press: &KeyPress,
    dashboard: Option<&Value>,
) -> Option<Vec<Act>> {
    if !press.is_plain() {
        return None;
    }
    let frame = table_frame(frame, state);
    let fields = configured_fields(&frame, panel);
    let count = fields
        .iter()
        .map(|field| field["values"].as_array().map_or(0, Vec::len))
        .max()
        .unwrap_or(0);
    match press.key {
        Key::Up => state.at = state.at.saturating_sub(1),
        Key::Down => state.at = (state.at + 1).min(count.saturating_sub(1)),
        Key::Left => state.choosing = Some(state.choosing.unwrap_or(0).saturating_sub(1)),
        Key::Right => state.choosing = Some((state.choosing.unwrap_or(0) + 1).min(fields.len().saturating_sub(1))),
        Key::Char('s') => {
            if let Some(field) = fields.get(state.choosing.unwrap_or(0)) {
                state.local["descending"] =
                    Value::Bool(state.local["sort"] == field["name"] && state.local["descending"] != true);
                state.local["sort"] = field["name"].clone();
                state.at = 0;
            }
        }
        Key::Char('f') => {
            let field = fields.get(state.choosing.unwrap_or(0))?;
            let bucket = &field["bucket"];
            let Some(query) = bucket["filters"].get(state.at).filter(|query| !query.is_null()) else {
                return Some(vec![Act::Message("Select a dimension bucket to add a filter".into())]);
            };
            let pill = serde_json::json!({"datasetId":bucket["datasetId"],"query":query,"label":formatted(&field["values"][state.at],field),"negated":false,"disabled":false,"pinned":false});
            if let Some(dashboard) = dashboard {
                let mut next = dashboard.clone();
                let mut filters = next["filters"].as_array().cloned().unwrap_or_default();
                filters.push(pill);
                next["filters"] = Value::Array(filters);
                return Some(vec![Act::Follow(dashboard_url(&next, "", None, None))]);
            }
            return Some(vec![Act::Follow(format!(
                "/apps/dashboards/Explore?datasetId={}&filtersJson={}",
                encoded(bucket["datasetId"].as_str().unwrap_or("")),
                encoded(&Value::Array(vec![pill]).to_string())
            ))]);
        }
        Key::Enter | Key::Char('l') => {
            let field = fields.get(state.choosing.unwrap_or(0))?;
            let mut url = field["link"].as_str()?.to_owned();
            for field in frame["fields"].as_array().into_iter().flatten() {
                url = url.replace(
                    &format!("${{{}}}", field["name"].as_str().unwrap_or("")),
                    &encoded(&text_of(field["values"].get(state.at))),
                );
            }
            url = url.replace("${value}", &encoded(&text_of(field["values"].get(state.at))));
            if (url.starts_with('/') && !url.starts_with("//")
                || url.starts_with("https://")
                || url.starts_with("http://"))
                && !url.chars().any(char::is_control)
            {
                return Some(vec![Act::Follow(url)]);
            }
            return Some(vec![Act::Message("This data link is unavailable".into())]);
        }
        _ => return None,
    }
    Some(Vec::new())
}
fn selection_line(frame: &Value, panel: &Value, state: &ComponentState) -> Line {
    let fields = configured_fields(frame, panel);
    let at = state.choosing.unwrap_or(0).min(fields.len().saturating_sub(1));
    vec![Span::new(
        format!(
            "Row {}, column {}: {} = {}  (arrows select, s sorts, Enter opens data link)",
            state.at + 1,
            at + 1,
            fields.get(at).and_then(|field| field["name"].as_str()).unwrap_or(""),
            fields
                .get(at)
                .map(|field| formatted(&field["values"][state.at], field))
                .unwrap_or_default()
        ),
        Role::Muted,
    )]
}
fn dashboard_url(dashboard: &Value, action: &str, panel_id: Option<&str>, mode: Option<&str>) -> String {
    let mut pairs = vec![("definitionId".to_string(), text_of(dashboard.get("definitionId")))];
    if dashboard["filters"].is_array() {
        pairs.push(("filtersJson".into(), dashboard["filters"].to_string()));
    }

    for key in ["from", "thru"] {
        if let Some(value) = dashboard["absoluteTime"][key].as_str() {
            pairs.push((key.into(), value.into()));
        }
    }
    if matches!(action, "exportPanel" | "inspectPanel") {
        pairs.push(("variables".into(), dashboard["variableValues"].to_string()));
    } else {
        for (key, value) in dashboard["variableValues"].as_object().into_iter().flatten() {
            for item in value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(std::slice::from_ref(value))
            {
                pairs.push((format!("var-{key}"), text_of(Some(item))));
            }
        }
    }
    if let Some(panel) = panel_id {
        pairs.push(("panelId".into(), panel.into()));
    }
    if let Some(mode) = mode {
        pairs.push(("renderMode".into(), mode.into()));
    }
    let path = if action.is_empty() {
        "/apps/dashboards/Dashboard".to_string()
    } else {
        format!("/apps/dashboards/Dashboard/{action}")
    };
    format!(
        "{path}?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{}={}", encoded(&key), encoded(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}
fn span_stamp(span: &Value) -> u64 {
    integer(&span["start_nano"]).unwrap_or_else(|| {
        integer(&span["start"])
            .unwrap_or_else(|| {
                span["start"]
                    .as_str()
                    .and_then(|text| text.parse::<jiff::Timestamp>().ok())
                    .and_then(|stamp| u64::try_from(stamp.as_millisecond()).ok())
                    .unwrap_or(0)
            })
            .saturating_mul(1_000_000)
    })
}

impl Component for ReportDashboard {
    fn name(&self) -> &'static str {
        "report-dashboard"
    }
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let data = state.data.as_ref().unwrap_or(props);
        let entries = dashboard_entries(data, state);
        let mut lines = Vec::new();
        let (_, controls) = layout_view(data, state);
        let focus = state.local["layoutFocus"].as_u64().unwrap_or(0) as usize;
        for (at, control) in controls.iter().enumerate() {
            lines.push(vec![Span::new(
                format!(
                    "{} {}: {}  ([/] choose layout, arrows change tabs, Space folds row)",
                    if at == focus { "▶" } else { " " },
                    control.title,
                    if control.row {
                        if control.closed {
                            "closed".into()
                        } else {
                            "open".into()
                        }
                    } else {
                        format!("tab {}/{}", control.active + 1, control.count)
                    },
                ),
                Role::Primary,
            )]);
        }
        for (at, (id, panel)) in entries.iter().enumerate() {
            if at == state.at {
                lines.push(vec![Span::new(
                    "▶ Selected panel; i inspects, c/x exports CSV/XLSX",
                    Role::Muted,
                )]);
            }
            let frame = data["frames"][id].get(0).unwrap_or(&Value::Null);
            let table = ComponentState {
                at: state.local["tables"][id]["row"].as_u64().unwrap_or(0) as usize,
                choosing: Some(state.local["tables"][id]["column"].as_u64().unwrap_or(0) as usize),
                local: state.local["tables"][id].clone(),
                ..Default::default()
            };
            let frame = table_frame(frame, &table);
            if at == state.at && state.closed.contains("__table_mode") {
                lines.push(selection_line(&frame, panel, &table));
            }
            lines.extend(panel_lines(&frame, panel, width));
            if at == state.at && state.closed.contains("__report_inspector") {
                lines.push(vec![Span::new("Inspector: authorized query requests and per-step input/output snapshots (native view capped at 1,200 lines; server sampling/truncation flags included)",Role::Muted)]);
                let inspected = &state.local["inspection"]["dashboard"]["panels"][id];
                lines.extend(serde_json::to_string_pretty(&serde_json::json!({"panelId":id,"panel":panel,"frame":frame,"inspection":inspected["inspection"],"requests":inspected["requests"]})).unwrap_or_default().lines().take(1200).map(|line|vec![Span::new(line,Role::Muted)]));
            }
            lines.push(Vec::new());
        }
        if lines.is_empty() {
            lines.push(vec![Span::new("No panels", Role::Muted)]);
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        if press.is_plain() {
            let data = state.data.as_ref().unwrap_or(props).clone();
            let entries = dashboard_entries(&data, state);
            let dashboard = &data["dashboard"];
            if state.closed.contains("__table_mode") {
                if press.key == Key::Escape || press.key == Key::Char('g') {
                    state.closed.remove("__table_mode");
                    return Some(Vec::new());
                }
                if let Some((id, panel)) = entries.get(state.at) {
                    let mut table = ComponentState {
                        at: state.local["tables"][id]["row"].as_u64().unwrap_or(0) as usize,
                        choosing: Some(state.local["tables"][id]["column"].as_u64().unwrap_or(0) as usize),
                        local: state.local["tables"][id].clone(),
                        ..Default::default()
                    };
                    if let Some(acts) = table_key(
                        data["frames"][id].get(0).unwrap_or(&Value::Null),
                        panel,
                        &mut table,
                        press,
                        Some(dashboard),
                    ) {
                        table.local["row"] = Value::from(table.at);
                        table.local["column"] = Value::from(table.choosing.unwrap_or(0));
                        state.local["tables"][id] = table.local;
                        return Some(acts);
                    }
                }
            }
            let (_, controls) = layout_view(&data, state);
            let focus =
                (state.local["layoutFocus"].as_u64().unwrap_or(0) as usize).min(controls.len().saturating_sub(1));
            if !state.closed.contains("__time_prefix") {
                if matches!(press.key, Key::Char('[') | Key::Char(']')) && !controls.is_empty() {
                    state.local["layoutFocus"] = Value::from(if press.key == Key::Char('[') {
                        focus.saturating_sub(1)
                    } else {
                        (focus + 1).min(controls.len() - 1)
                    });
                    return Some(Vec::new());
                }
                if let Some(control) = controls.get(focus) {
                    if control.row && press.key == Key::Char(' ') {
                        let key = format!("layout:{}", control.path);
                        if !state.closed.remove(&key) {
                            state.closed.insert(key);
                        }
                        state.at = 0;
                        return Some(Vec::new());
                    }
                    if !control.row && control.count > 0 && matches!(press.key, Key::Left | Key::Right) {
                        let at = if press.key == Key::Left {
                            (control.active + control.count - 1) % control.count
                        } else {
                            (control.active + 1) % control.count
                        };
                        state.local["tabs"][&control.path] = Value::from(at);
                        if control.path == "layout" {
                            state.choosing = Some(at);
                        }
                        state.at = 0;
                        return Some(Vec::new());
                    }
                }
            }
            match press.key {
                Key::Up => {
                    state.at = state.at.saturating_sub(1);
                    return Some(Vec::new());
                }
                Key::Down => {
                    state.at = (state.at + 1).min(entries.len().saturating_sub(1));
                    return Some(Vec::new());
                }
                Key::Char('g') => {
                    state.closed.insert("__table_mode".into());
                    return Some(Vec::new());
                }
                Key::Char('i') => {
                    if !state.closed.remove("__report_inspector") {
                        state.closed.insert("__report_inspector".into());
                        if let Some((id, _)) = entries.get(state.at) {
                            state.local["inspectionRequest"] = Value::Bool(true);
                            return Some(vec![Act::Read(dashboard_url(
                                dashboard,
                                "inspectPanel",
                                Some(id),
                                None,
                            ))]);
                        }
                    }
                    return Some(Vec::new());
                }
                Key::Char('e') => {
                    return Some(vec![Act::Follow(format!(
                        "/apps/dashboards/Definition?definitionId={}",
                        encoded(dashboard["definitionId"].as_str().unwrap_or(""))
                    ))]);
                }
                Key::Char('c') | Key::Char('x') => {
                    if let Some((id, _)) = entries.get(state.at) {
                        return Some(vec![Act::DownloadReport(dashboard_url(
                            dashboard,
                            "exportPanel",
                            Some(id),
                            Some(if press.key == Key::Char('c') { "csv" } else { "xlsx" }),
                        ))]);
                    }
                }
                Key::Enter | Key::Char('v') => {
                    if let Some((id, _)) = entries.get(state.at) {
                        return Some(vec![Act::Follow(dashboard_url(dashboard, "", Some(id), None))]);
                    }
                }
                Key::Char('t') => {
                    state.closed.insert("__time_prefix".into());
                    return Some(Vec::new());
                }
                _ => {}
            }
            if state.closed.remove("__time_prefix") {
                let from = dashboard["absoluteTime"]["from"]
                    .as_str()
                    .and_then(|text| text.parse::<jiff::Timestamp>().ok());
                let thru = dashboard["absoluteTime"]["thru"]
                    .as_str()
                    .and_then(|text| text.parse::<jiff::Timestamp>().ok());
                if let (Some(from), Some(thru)) = (from, thru) {
                    let width = thru.as_millisecond().saturating_sub(from.as_millisecond());
                    let offsets = match press.key {
                        Key::Left => Some((-width, -width)),
                        Key::Right => Some((width, width)),
                        Key::Char('-') => Some((-width / 2, width / 2)),
                        _ => None,
                    };
                    if let Some((left, right)) = offsets {
                        let mut next = dashboard.clone();
                        next["absoluteTime"]["from"] = Value::String(
                            jiff::Timestamp::from_millisecond(from.as_millisecond().saturating_add(left))
                                .ok()?
                                .to_string(),
                        );
                        next["absoluteTime"]["thru"] = Value::String(
                            jiff::Timestamp::from_millisecond(thru.as_millisecond().saturating_add(right))
                                .ok()?
                                .to_string(),
                        );
                        return Some(vec![Act::Follow(dashboard_url(&next, "", None, None))]);
                    }
                }
            }
        }
        (press.key == Key::Char('r') && press.is_plain()).then(|| {
            vec![if source_of(props, state).is_some() {
                Act::Reload
            } else {
                Act::ReloadScreen
            }]
        })
    }
    fn receive(&self, _props: &Value, state: &mut ComponentState, value: Value) {
        if state.local["inspectionRequest"] == true {
            state.local["inspection"] = value;
            state.local["inspectionRequest"] = Value::Bool(false);
        } else {
            state.data = Some(value);
        }
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("g", "Enter table cells; Esc leaves"),
            ("f", "Filter by selected bucket in table mode"),
            ("[/]", "Select nested layout control"),
            ("Space", "Fold/unfold selected row"),
            ("↑/↓", "Select panel"),
            ("←/→", "Choose dashboard tab"),
            ("i", "Inspect selected panel"),
            ("v/Enter", "View panel"),
            ("e", "Edit definition"),
            ("c/x", "Export CSV/XLSX"),
            ("t-", "Zoom out"),
            ("t←/t→", "Shift time range"),
            ("r", "Refresh dashboard"),
        ]
    }
}

impl Component for ReportPlaylist {
    fn name(&self) -> &'static str {
        "report-playlist"
    }
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        if props["items"].as_array().is_none_or(Vec::is_empty) {
            return None;
        }
        let mut params = BTreeMap::new();
        for key in ["definitionId", "from", "thru"] {
            if let Some(value) = props[key].as_str() {
                params.insert(key.into(), value.into());
            }
        }
        params.insert(
            "index".into(),
            state.local["playlistAt"].as_u64().unwrap_or(0).to_string(),
        );
        Some((props["source"].as_str().unwrap_or("data").into(), params))
    }
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, wide: bool) -> Vec<Line> {
        let at = state.local["playlistAt"].as_u64().unwrap_or(0) as usize;
        let mut lines = vec![vec![Span::new(
            format!(
                "Playlist {}/{}: {} ({}) — n/b change, p pauses",
                at + 1,
                props["items"].as_array().map_or(0, Vec::len),
                text_of(props["items"].get(at).and_then(|item| item.get("title"))),
                if state.closed.contains("__playlist_paused") {
                    "paused"
                } else {
                    "running"
                }
            ),
            Role::Primary,
        )]];
        lines.extend(ReportDashboard.lines(props, state, width, wide));
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, wide: bool) -> Option<Vec<Act>> {
        if press.is_plain() {
            match press.key {
                Key::Char('p') => {
                    if !state.closed.remove("__playlist_paused") {
                        state.closed.insert("__playlist_paused".into());
                    }
                    return Some(Vec::new());
                }
                Key::Char('n') | Key::Char('b') => {
                    let count = props["items"].as_array().map_or(0, Vec::len);
                    if count > 0 {
                        let at = state.local["playlistAt"].as_u64().unwrap_or(0) as usize;
                        state.local["playlistAt"] = Value::from(if press.key == Key::Char('b') {
                            (at + count - 1) % count
                        } else {
                            (at + 1) % count
                        });
                        state.at = 0;
                        return Some(vec![Act::Reload]);
                    }
                }
                _ => {}
            }
        }
        ReportDashboard.key(props, state, press, wide)
    }
    fn poll_millis(&self, props: &Value, state: &ComponentState) -> Option<u64> {
        (!state.closed.contains("__playlist_paused") && props["items"].as_array().is_some_and(|items| items.len() > 1))
            .then(|| props["seconds"].as_u64().unwrap_or(30).clamp(5, 3600) * 1000)
    }
    fn tick(&self, props: &Value, state: &mut ComponentState) -> Vec<Act> {
        if state.loading {
            return Vec::new();
        }
        self.key(props, state, &KeyPress::plain(Key::Char('n')), false)
            .unwrap_or_default()
    }
    fn receive(&self, props: &Value, state: &mut ComponentState, value: Value) {
        ReportDashboard.receive(props, state, value);
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("n/b", "Next/previous dashboard"),
            ("p", "Pause/resume playlist"),
            ("g", "Table controls"),
            ("i", "Inspect panel"),
        ]
    }
}

impl Component for TraceWaterfall {
    fn name(&self) -> &'static str {
        "trace-waterfall"
    }
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let data = state.data.as_ref().unwrap_or(props);
        let mut spans: Vec<&Value> = data["spans"].as_array().into_iter().flatten().collect();
        if spans.is_empty() {
            return vec![vec![Span::new("No spans; telemetry may be missing", Role::Muted)]];
        }
        spans.sort_by_key(|span| span_stamp(span));
        let first = span_stamp(spans[0]);
        let last = spans
            .iter()
            .map(|span| span_stamp(span).saturating_add(integer(&span["duration_nano"]).unwrap_or(0)))
            .max()
            .unwrap_or(first);
        let duration = last.saturating_sub(first).max(1);
        let chart_width = usize::from(width).saturating_sub(42).min(80);
        let by_id: BTreeMap<&str, &Value> = spans
            .iter()
            .filter_map(|span| span["span_id"].as_str().map(|id| (id, *span)))
            .collect();
        let mut lines: Vec<Line> = spans
            .iter()
            .map(|span| {
                let mut seen = BTreeSet::new();
                let mut parent = span["parent_span_id"].as_str().unwrap_or("");
                let mut depth = 0;
                while let Some(ancestor) = by_id.get(parent) {
                    if !seen.insert(parent) || depth >= 16 {
                        break;
                    }
                    depth += 1;
                    parent = ancestor["parent_span_id"].as_str().unwrap_or("");
                }
                let elapsed = integer(&span["duration_nano"]).unwrap_or(0);
                let offset = ((u128::from(span_stamp(span).saturating_sub(first)) * chart_width as u128)
                    / u128::from(duration)) as usize;
                let length = ((u128::from(elapsed) * chart_width as u128) / u128::from(duration)) as usize;
                let title = fit(&format!("{}{}", " ".repeat(depth * 2), prop(span, "name", "span")), 28);
                #[expect(clippy::cast_precision_loss)]
                let millis = elapsed as f64 / 1_000_000.0;
                vec![
                    Span::new(format!("{title} {millis:>8.2}ms "), Role::Text),
                    Span::new(
                        format!("{}{}", " ".repeat(offset), "━".repeat(length.max(1))),
                        if span["status_code"] == "ERROR" {
                            Role::Negative
                        } else {
                            Role::Primary
                        },
                    ),
                ]
            })
            .collect();
        if state.closed.contains("__report_inspector") {
            if let Some(span) = spans.get(state.at) {
                lines.extend(
                    serde_json::to_string_pretty(span)
                        .unwrap_or_default()
                        .lines()
                        .map(|line| vec![Span::new(line, Role::Muted)]),
                );
            }
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        if press.is_plain() {
            let data = state.data.as_ref().unwrap_or(props);
            let count = data["spans"].as_array().map_or(0, Vec::len);
            match press.key {
                Key::Up => {
                    state.at = state.at.saturating_sub(1);
                    return Some(Vec::new());
                }
                Key::Down => {
                    state.at = (state.at + 1).min(count.saturating_sub(1));
                    return Some(Vec::new());
                }
                Key::Enter | Key::Char('i') => {
                    if !state.closed.remove("__report_inspector") {
                        state.closed.insert("__report_inspector".into());
                    }
                    return Some(Vec::new());
                }
                _ => {}
            }
        }
        (press.key == Key::Char('r') && press.is_plain()).then(|| {
            vec![if source_of(props, state).is_some() {
                Act::Reload
            } else {
                Act::ReloadScreen
            }]
        })
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("↑/↓", "Select span"),
            ("i/Enter", "Inspect span attributes/events"),
            ("r", "Refresh trace"),
        ]
    }
}

impl Component for LogTail {
    fn name(&self) -> &'static str {
        "log-tail"
    }
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        let (url, mut parameters) = source_of(props, state)?;
        if let Some(cursor) = state
            .data
            .as_ref()
            .and_then(|data| data.get("nextCursor"))
            .filter(|cursor| !cursor.is_null())
        {
            parameters.insert("cursor".into(), cursor.to_string());
        }
        Some((url, parameters))
    }
    fn receive(&self, _props: &Value, state: &mut ComponentState, mut value: Value) {
        let previous = state.data.as_ref();
        let mut hits = previous
            .and_then(|data| data["hits"].as_array())
            .cloned()
            .unwrap_or_default();
        let mut ids: BTreeSet<String> = hits
            .iter()
            .map(|hit| format!("{}:{}", text_of(hit.get("documentId")), text_of(hit.get("id"))))
            .collect();
        for hit in value["hits"].as_array().into_iter().flatten() {
            if ids.insert(format!("{}:{}", text_of(hit.get("documentId")), text_of(hit.get("id")))) {
                hits.push(hit.clone());
            }
        }
        let dropped = hits.len().saturating_sub(500);
        if dropped > 0 {
            hits.drain(..dropped);
        }
        if value["nextCursor"].is_null() {
            value["nextCursor"] = previous.map(|data| data["nextCursor"].clone()).unwrap_or(Value::Null);
        }
        value["removed"] =
            Value::from(previous.and_then(|data| data["removed"].as_u64()).unwrap_or(0) + dropped as u64);
        value["hits"] = Value::Array(hits);
        state.data = Some(value);
    }
    fn poll_millis(&self, _props: &Value, state: &ComponentState) -> Option<u64> {
        state.closed.contains("__tail_running").then_some(2000)
    }
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let data = state.data.as_ref().unwrap_or(props);
        let hits = data["hits"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if hits.is_empty() {
            return vec![vec![Span::new(
                "No entries; r refreshes. Missing telemetry does not establish health.",
                Role::Muted,
            )]];
        }
        let mut lines = vec![vec![Span::new(
            if state.closed.contains("__tail_running") {
                "Live tail running; p pauses"
            } else {
                "Live tail paused; p resumes"
            },
            Role::Muted,
        )]];
        lines.extend(hits.iter().enumerate().map(|(at, hit)| {
            let body = &hit["body"];
            let role = if body["severity_number"].as_i64().unwrap_or(0) >= 17 {
                Role::Negative
            } else {
                Role::Text
            };
            vec![Span::new(
                fit(
                    &format!(
                        "{}{} {} {}",
                        if at == state.at { "▶ " } else { "  " },
                        text_of(body.get("timestamp")),
                        prop(body, "severity_text", ""),
                        prop(body, "body", prop(body, "name", ""))
                    ),
                    usize::from(width),
                ),
                role,
            )]
        }));
        if state.closed.contains("__report_inspector") {
            if let Some(hit) = hits.get(state.at) {
                lines.extend(
                    serde_json::to_string_pretty(hit)
                        .unwrap_or_default()
                        .lines()
                        .map(|line| vec![Span::new(line, Role::Text)]),
                );
            }
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        if press.is_plain() {
            match press.key {
                Key::Char('p') => {
                    if state.closed.remove("__tail_running") {
                        return Some(Vec::new());
                    }
                    state.closed.insert("__tail_running".into());
                    return Some(vec![Act::Reload]);
                }
                Key::Char('i') | Key::Enter => {
                    if !state.closed.remove("__report_inspector") {
                        state.closed.insert("__report_inspector".into());
                    }
                    return Some(Vec::new());
                }
                Key::Down => {
                    state.at = (state.at + 1).min(
                        state
                            .data
                            .as_ref()
                            .and_then(|data| data["hits"].as_array())
                            .map_or(0, |hits| hits.len().saturating_sub(1)),
                    );
                    return Some(Vec::new());
                }
                Key::Up => {
                    state.at = state.at.saturating_sub(1);
                    return Some(Vec::new());
                }
                Key::Char('t') => {
                    if let Some(trace) = state
                        .data
                        .as_ref()
                        .and_then(|data| data["hits"].get(state.at))
                        .and_then(|hit| hit["body"]["trace_id"].as_str())
                        .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                    {
                        return Some(vec![Act::Follow(format!("/apps/monitor/TraceDetail?traceId={trace}"))]);
                    }
                }
                _ => {}
            }
        }
        (press.key == Key::Char('r') && press.is_plain()).then(|| {
            vec![if source_of(props, state).is_some() {
                Act::Reload
            } else {
                Act::ReloadScreen
            }]
        })
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("r", "Read latest telemetry"),
            ("p", "Pause/resume live tail"),
            ("↑/↓", "Select entry"),
            ("i/Enter", "Inspect entry"),
            ("t", "Open trace"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::line_text;
    use serde_json::json;
    #[test]
    fn frames_show_null_as_missing_and_share_the_browser_numbers() {
        let props = json!({"panel":{"title":"Latency","visualization":"table"},"frame":{"fields":[{"name":"duration","type":"number","unit":"ns","values":[1250000,null]}]}});
        let lines: Vec<_> = ReportPanel
            .lines(&props, &ComponentState::default(), 60, false)
            .iter()
            .map(line_text)
            .collect();
        assert!(lines.iter().any(|line| line.contains("1.25 ms")));
        assert!(lines.iter().any(|line| line.contains('—')));
    }
    #[test]
    fn waterfall_preserves_nanosecond_offsets_and_parent_depth() {
        let props = json!({"spans":[{"span_id":"root","name":"request","start_nano":"1800000000000000000","duration_nano":10000000},
            {"span_id":"child","parent_span_id":"root","name":"service","start_nano":"1800000000005000000","duration_nano":1000000}]});
        let lines: Vec<_> = TraceWaterfall
            .lines(&props, &ComponentState::default(), 100, false)
            .iter()
            .map(line_text)
            .collect();
        assert!(lines[1].starts_with("  service"));
        assert!(lines[0].contains("10.00ms"));
        assert!(lines[1].contains("1.00ms"));
        assert!(
            lines[1].find('━') > lines[0].find('━'),
            "the child retains its offset when nanoseconds arrive as strings"
        );
    }
    #[test]
    fn field_overrides_and_inline_refresh_are_real() {
        let props = json!({"panel":{"title":"Latency","fieldConfig":{"defaults":{"decimals":1},"overrides":[{"match":{"name":"latency"},"config":{"unit":"s","noValue":"Missing"}}]}},"frame":{"fields":[{"name":"latency","type":"number","values":[1.25,null]}]}});
        let lines: Vec<_> = ReportPanel
            .lines(&props, &ComponentState::default(), 60, false)
            .iter()
            .map(line_text)
            .collect();
        assert!(lines.iter().any(|line| line.contains("1.2 s")));
        assert!(lines.iter().any(|line| line.contains("Missing")));
        let mut state = ComponentState::default();
        assert_eq!(
            ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Char('r')), false),
            Some(vec![Act::ReloadScreen])
        );
        ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
        assert!(ReportPanel
            .lines(&props, &state, 60, false)
            .iter()
            .map(line_text)
            .any(|line| line.contains("fieldConfig")));
    }
    #[test]
    fn dashboard_tabs_inspector_exports_and_time_keys_use_current_scope() {
        let props = json!({"dashboard":{"definitionId":"monitor-artifacts","absoluteTime":{"from":"2026-01-01T00:00:00Z","thru":"2026-01-02T00:00:00Z"},"variableValues":{"service":"ERP"},"layout":{"type":"tabs","items":[{"title":"First","layout":{"items":[{"panelId":"one"}]}},{"title":"Second","layout":{"items":[{"panelId":"two"}]}}]},"panels":{"one":{"title":"Panel one"},"two":{"title":"Panel two","queries":[{"datasetId":"otel_metrics"}]}}},"frames":{}});
        let mut state = ComponentState::default();
        let text = |state: &ComponentState| {
            ReportDashboard
                .lines(&props, state, 100, false)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(text(&state).contains("Panel one"));
        assert!(!text(&state).contains("Panel two"));
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Right), false);
        assert!(text(&state).contains("Panel two"));
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
        assert!(text(&state).contains("otel_metrics"));
        let export = ReportDashboard
            .key(&props, &mut state, &KeyPress::plain(Key::Char('x')), false)
            .unwrap();
        assert!(
            matches!(&export[0],Act::DownloadReport(url) if url.contains("panelId=two") && url.contains("renderMode=xlsx") && url.contains("ERP"))
        );
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('t')), false);
        let shifted = ReportDashboard
            .key(&props, &mut state, &KeyPress::plain(Key::Right), false)
            .unwrap();
        assert!(
            matches!(&shifted[0],Act::Follow(url) if url.contains("from=2026-01-02") && url.contains("var-service=ERP"))
        );
    }
    #[test]
    fn live_tail_merges_cursor_pages_bounds_rows_and_pauses() {
        let props = json!({"source":"/tail"});
        let mut state = ComponentState::default();
        LogTail.receive(
            &props,
            &mut state,
            json!({"hits":[{"documentId":"otel_logs","id":"first","body":{"body":"first"}}],"nextCursor":{"id":1}}),
        );
        assert_eq!(LogTail.poll_millis(&props, &state), None);
        assert_eq!(
            LogTail.key(&props, &mut state, &KeyPress::plain(Key::Char('p')), false),
            Some(vec![Act::Reload])
        );
        assert_eq!(LogTail.poll_millis(&props, &state), Some(2000));
        assert_eq!(LogTail.source(&props, &state).unwrap().1["cursor"], "{\"id\":1}");
        LogTail.receive(&props,&mut state,json!({"hits":[{"documentId":"otel_logs","id":"first"},{"documentId":"otel_logs","id":"second"}],"nextCursor":{"id":2}}));
        assert_eq!(state.data.as_ref().unwrap()["hits"].as_array().unwrap().len(), 2);
        let hits: Vec<_> = (0..600)
            .map(|at| json!({"documentId":"otel_logs","id":at.to_string()}))
            .collect();
        LogTail.receive(&props, &mut state, json!({"hits":hits,"nextCursor":null}));
        assert_eq!(state.data.as_ref().unwrap()["hits"].as_array().unwrap().len(), 500);
        assert_eq!(state.data.as_ref().unwrap()["nextCursor"], json!({"id":2}));
        LogTail.key(&props, &mut state, &KeyPress::plain(Key::Char('p')), false);
        assert_eq!(LogTail.poll_millis(&props, &state), None);
    }
    #[test]
    fn span_inspector_preserves_attributes_and_events() {
        let props = json!({"spans":[{"span_id":"child","name":"child","start_nano":"1800000000000000010","attributes":{"http.route":"/ERP"},"events":[{"name":"exception"}]},{"span_id":"root","name":"root","start_nano":"1800000000000000000"}]});
        let mut state = ComponentState::default();
        TraceWaterfall.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
        TraceWaterfall.key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false);
        let text = TraceWaterfall
            .lines(&props, &state, 100, false)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("http.route") && text.contains("exception"));
    }
    #[test]
    fn text_panels_do_not_require_frames_and_bytes_use_the_browser_scale() {
        let lines = panel_lines(
            &json!({"fields":[]}),
            &json!({"title":"Notes","visualization":"text","text":"Release notes\nKeep watching latency"}),
            80,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Release notes") && text.contains("Keep watching latency"));
        assert!(!text.contains("No data"));
        assert_eq!(
            formatted(&json!(2048), &json!({"type":"number","unit":"bytes"})),
            "2 KiB"
        );
        assert_eq!(
            formatted(&json!(1536), &json!({"type":"number","unit":"bytes"})),
            "1.50 KiB"
        );
        let fields = configured_fields(
            &json!({"fields":[{"name":"value","unit":"s"}]}),
            &json!({"fieldConfig":{"defaults":{"unit":"ms"}}}),
        );
        assert_eq!(fields[0]["unit"], "s");
    }
    #[test]
    fn partial_and_subtotal_metadata_are_not_numeric_chart_series() {
        let frame = json!({"fields":[{"name":"partial","type":"boolean","values":[true]},{"name":"__level","type":"number","values":[0]},{"name":"__subtotal","type":"number","values":[1]},{"name":"time","type":"time","values":[1000]},{"name":"requests","type":"number","values":[3]}]});
        let fields = configured_fields(&frame, &json!({}));
        let series: Vec<_> = fields
            .iter()
            .filter(|field| field["type"] == "number" && !is_metadata(field))
            .map(|field| field["name"].as_str().unwrap())
            .collect();
        assert_eq!(series, ["requests"]);
        let lines = panel_lines(&frame, &json!({"visualization":"stat"}), 80)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(lines.contains('3'));
        assert!(!lines.contains("0.00"));
    }
    #[test]
    fn gaps_are_missing_in_sparklines() {
        assert_eq!(sparkline(&[json!(1), Value::Null, json!(3)], 3), "▁ █");
    }
}

#[cfg(test)]
mod gap_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn split_series_preserve_time_gaps_groups_and_original_rows() {
        let frame = json!({"fields":[
            {"name":"time","type":"time","values":[0,0,1000,2000,2000,3000]},
            {"name":"service","type":"string","values":["A","B","A","A","B",null]},
            {"name":"latency","type":"number","values":[1,100,null,3,300,null]},
            {"name":"__level","type":"number","values":[0,0,0,0,0,0]}
        ]});
        let panel =
            json!({"visualization":"timeSeries","queries":[{"aggregations":[{"id":"service","role":"series"}]}]});
        let trends = trend_series(&frame, &panel);
        assert!(!trends.limited);
        assert_eq!(trends.series.len(), 2);
        assert_eq!(trends.series[0].label, "latency {service=A}");
        assert_eq!(
            trends.series[0].values,
            vec![json!(1), Value::Null, json!(3), Value::Null]
        );
        assert_eq!(trends.series[0].rows, vec![Some(0), Some(2), Some(3), None]);
        assert_eq!(trends.series[1].label, "latency {service=B}");
        assert_eq!(
            trends.series[1].values,
            vec![json!(100), Value::Null, json!(300), Value::Null]
        );
        assert_eq!(trends.series[1].rows, vec![Some(1), None, Some(4), None]);
        let text = panel_lines(&frame, &panel, 100)
            .iter()
            .map(|line| line.iter().map(|span| span.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("service=A") && text.contains("service=B"));
        assert!(text.contains("source rows: 1,3,4") && text.contains("source rows: 2,5"));
        // Compression cannot bridge a missing coordinate.
        assert_eq!(sparkline(&[json!(0), Value::Null, json!(2), json!(3)], 2), " █");
    }

    #[test]
    fn numeric_hidden_dimensions_and_duplicate_points_are_independent_tracks() {
        let frame = json!({"fields":[
            {"name":"time","type":"time","values":[0,0,0,1000]},
            {"name":"code","type":"integer","values":[200,200,500,500],"config":{"role":"split-series","hidden":true}},
            {"name":"requests","type":"integer","values":[0,2,10,11]}
        ]});
        let trends = trend_series(&frame, &json!({}));
        assert_eq!(trends.series.len(), 3);
        assert_eq!(trends.series[0].label, "requests {code=200} [observation 1]");
        assert_eq!(trends.series[0].values, vec![json!(0), Value::Null]);
        assert_eq!(trends.series[1].rows, vec![Some(1), None]);
        assert_eq!(trends.series[2].values, vec![json!(10), json!(11)]);
        let documents = trend_series(
            &json!({"fields":[{"name":"time","type":"time","values":[0,1000]},{"name":"message","type":"string","values":["one","two"]},{"name":"value","type":"number","values":[1,2]}]}),
            &json!({"queries":[{"mode":"documents"}]}),
        );
        assert_eq!(documents.series.len(), 1);
        assert_eq!(documents.series[0].values, vec![json!(1), json!(2)]);
    }

    #[test]
    fn boolean_bucket_flags_do_not_split_trends_unless_explicitly_assigned() {
        let frame = json!({"fields":[
            {"name":"time","type":"time","values":[0,1000]},
            {"name":"missing","type":"boolean","values":[false,true]},
            {"name":"other","type":"boolean","values":[true,false]},
            {"name":"missing.2","type":"boolean","values":[true,false]},
            {"name":"other.2","type":"boolean","values":[false,true]},
            {"name":"requests","type":"number","values":[0,2]}
        ]});
        let trend = trend_series(&frame, &json!({}));
        assert_eq!(trend.series.len(), 1);
        assert_eq!(trend.series[0].label, "requests");
        assert_eq!(trend.series[0].values, vec![json!(0), json!(2)]);
        assert_eq!(trend.series[0].rows, vec![Some(0), Some(1)]);
        // An authored aggregation role wins over the reserved-looking name.
        let explicit = trend_series(
            &frame,
            &json!({"queries":[{"aggregations":[{"id":"missing.2","role":"series"}]}]}),
        );
        assert_eq!(explicit.series.len(), 2);
        assert!(explicit.series.iter().all(|series| series.label.contains("missing.2=")));
        assert!(!is_metadata(&json!({"name":"missing","type":"number"})));
        assert!(!is_metadata(&json!({"name":"other.business","type":"boolean"})));
    }

    #[test]
    fn sorted_bucket_filters_keep_exact_missing_other_predicates_and_dashboard_scope() {
        let predicates = json!([{"not":{"field":{"name":"status","op":"in","values":["A"]}}},{"not":{"field":{"name":"status","op":"exists"}}},{"field":{"name":"status","op":"eq","value":"A"}}]);
        let frame = json!({"fields":[{"name":"status","type":"string","values":["Other","Missing","A"],"config":{"bucket":{"datasetId":"orders","filters":predicates,"labels":["other","missing","A"]}}}]});
        let dashboard = json!({"definitionId":"business","absoluteTime":{"from":"2026-01-01","thru":"2026-01-02"},"filters":[{"query":{"field":{"name":"tenant","op":"eq","value":"T1"}}}],"variableValues":{"region":"west"}});
        let mut state = ComponentState::default();
        table_key(
            &frame,
            &json!({}),
            &mut state,
            &KeyPress::plain(Key::Char('s')),
            Some(&dashboard),
        );
        assert_eq!(
            table_frame(&frame, &state)["fields"][0]["config"]["bucket"]["labels"][0],
            "A"
        );
        for (row, expected) in [(0, 2), (1, 1), (2, 0)] {
            state.at = row;
            let acts = table_key(
                &frame,
                &json!({}),
                &mut state,
                &KeyPress::plain(Key::Char('f')),
                Some(&dashboard),
            )
            .unwrap();
            let Act::Follow(url) = &acts[0] else {
                panic!("expected filtered dashboard")
            };
            let location = crate::nav::Location::of_url(url);
            let filters: Value = serde_json::from_str(&location.parameters["filtersJson"]).unwrap();
            assert_eq!(filters[0], dashboard["filters"][0]);
            assert_eq!(filters[1]["query"], predicates[expected]);
            assert_eq!(location.parameters["var-region"], "west");
            assert_eq!(location.parameters["from"], "2026-01-01");
        }
    }
    #[test]
    fn sorting_keeps_row_link_values_aligned_and_rejects_unsafe_links() {
        let props = json!({"panel":{"visualization":"table","fieldConfig":{"overrides":[{"match":{"name":"id"},"config":{"link":"/record?id=${value}&amount=${amount}"}}]}},"frame":{"fields":[{"name":"id","type":"string","values":["B & C","A"]},{"name":"amount","type":"number","values":[9,0]}]}});
        let mut state = ComponentState {
            choosing: Some(1),
            ..Default::default()
        };
        ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Char('s')), false);
        let frame = table_frame(&props["frame"], &state);
        assert_eq!(frame["fields"][0]["values"][0], "A");
        assert_eq!(frame["fields"][1]["values"][0], 0);
        state.choosing = Some(0);
        assert_eq!(
            ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Enter), false),
            Some(vec![Act::Follow("/record?id=A&amount=0".into())])
        );
        ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Down), false);
        assert_eq!(
            ReportPanel.key(&props, &mut state, &KeyPress::plain(Key::Enter), false),
            Some(vec![Act::Follow("/record?id=B%20%26%20C&amount=9".into())])
        );
        let unsafe_props =
            json!({"panel":{},"frame":{"fields":[{"name":"id","values":["X"],"link":"javascript:alert('${value}')"}]}});
        state.at = 0;
        assert!(
            matches!(ReportPanel.key(&unsafe_props,&mut state,&KeyPress::plain(Key::Enter),false),Some(acts)if matches!(&acts[0],Act::Message(_)))
        );
    }
    #[test]
    fn nested_tabs_and_rows_change_visibility_without_flattening_all_tabs() {
        let props = json!({"dashboard":{"layout":{"type":"row","title":"Business","items":[{"type":"tabs","items":[{"title":"First","layout":{"panelId":"a"}},{"title":"Second","layout":{"type":"row","items":[{"panelId":"b"}]}}]}]},"panels":{"a":{"title":"One"},"b":{"title":"Two"}}}});
        let mut state = ComponentState::default();
        assert_eq!(
            dashboard_entries(&props, &state)
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char(']')), false);
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Right), false);
        assert_eq!(dashboard_entries(&props, &state)[0].0, "b");
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char('[')), false);
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char(' ')), false);
        assert!(dashboard_entries(&props, &state).is_empty());
        ReportDashboard.key(&props, &mut state, &KeyPress::plain(Key::Char(' ')), false);
        assert_eq!(dashboard_entries(&props, &state)[0].0, "b");
    }
    #[test]
    fn inspector_fetches_authorized_steps_with_the_visible_panel_scope() {
        let props = json!({"dashboard":{"definitionId":"sales","absoluteTime":{"from":"2026-01-01T00:00:00Z","thru":"2026-01-02T00:00:00Z"},"variableValues":{"tenant":"A & B"},"panels":{"p":{"title":"Sales","queries":[{"datasetId":"sales"}]}}},"frames":{}});
        let mut state = ComponentState::default();
        let acts = ReportDashboard
            .key(&props, &mut state, &KeyPress::plain(Key::Char('i')), false)
            .unwrap();
        assert!(
            matches!(&acts[0],Act::Read(url)if url.contains("inspectPanel")&&url.contains("panelId=p")&&url.contains("A%20%26%20B")&&url.contains("variables="))
        );
        ReportDashboard.receive(&props,&mut state,json!({"dashboard":{"panels":{"p":{"requests":[{"datasetId":"sales"}],"inspection":[{"index":0,"type":"pivot","input":{"frames":[{"fields":[]}]},"output":{"frames":[{"fields":[{"name":"sales","values":[0,null]}]}]}}]}}}}));
        let text = ReportDashboard
            .lines(&props, &state, 120, false)
            .iter()
            .map(super::super::line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("pivot") && text.contains("input") && text.contains("output") && text.contains("null"));
        assert!(state.data.is_none(), "inspector must not replace dashboard frames");
    }
    #[test]
    fn playlist_rotation_is_bounded_pauses_and_preserves_request_authority() {
        let props = json!({"source":"/apps/dashboards/Playlist/data","definitionId":"my-playlist","from":"now-1h","thru":"now","seconds":1,"items":[{"title":"A"},{"title":"B"}]});
        let mut state = ComponentState::default();
        assert_eq!(ReportPlaylist.poll_millis(&props, &state), Some(5000));
        assert_eq!(ReportPlaylist.source(&props, &state).unwrap().1["index"], "0");
        assert_eq!(ReportPlaylist.tick(&props, &mut state), vec![Act::Reload]);
        let params = ReportPlaylist.source(&props, &state).unwrap().1;
        assert_eq!(params["index"], "1");
        assert_eq!(params["definitionId"], "my-playlist");
        ReportPlaylist.key(&props, &mut state, &KeyPress::plain(Key::Char('p')), false);
        assert_eq!(ReportPlaylist.poll_millis(&props, &state), None);
    }
}
