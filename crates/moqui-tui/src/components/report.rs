/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Reporting twins consume the same authorized columnar frames as the browser and exports.
use super::{fit, prop, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::{
    keys::{Key, KeyPress},
    theme::Role,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

mod graph;
mod playlist;
mod visual;

pub use playlist::ReportPlaylist;

pub struct ReportPanel;
pub struct ReportDashboard;
pub struct TraceWaterfall;
pub struct LogTail;

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

/// The value mappings of a field that match a value: by value, then by range.
fn mappings_for<'a>(value: &'a Value, field: &'a Value) -> impl Iterator<Item = &'a Value> {
    field["mappings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |mapping| {
            let exact = mapping
                .get("value")
                .is_some_and(|mapped| mapped == value || text_of(Some(mapped)) == text_of(Some(value)));
            let range = number(value)
                .zip(number(&mapping["from"]))
                .zip(number(&mapping["to"]))
                .is_some_and(|((value, from), to)| value >= from && value <= to);
            exact || range
        })
}

/// The ISO 4217 code of a currency unit: `USD` or `currency:USD`, as the browser's `report.format` takes it.
fn currency_code(unit: &str) -> Option<&str> {
    let code = unit.strip_prefix("currency:").unwrap_or(unit);
    (code.len() == 3 && code.bytes().all(|byte| byte.is_ascii_uppercase())).then_some(code)
}

/// A currency amount as the browser's `toLocaleString(undefined, {style: "currency"})` shows it in English: the
/// symbol, grouped thousands, at most `decimals` (default 2) places and at least the currency's minor units.
fn currency_text(value: f64, code: &str, decimals: Option<u64>) -> String {
    let (symbol, minor) = match code {
        "USD" => ("$", 2),
        "EUR" => ("€", 2),
        "GBP" => ("£", 2),
        "JPY" => ("¥", 0),
        "CNY" => ("CN¥", 2),
        "INR" => ("₹", 2),
        "CAD" => ("CA$", 2),
        "AUD" => ("A$", 2),
        "NZD" => ("NZ$", 2),
        "HKD" => ("HK$", 2),
        "MXN" => ("MX$", 2),
        "BRL" => ("R$", 2),
        "KRW" => ("₩", 0),
        "ILS" => ("₪", 2),
        "VND" => ("₫", 0),
        "PHP" => ("₱", 2),
        "TWD" => ("NT$", 2),
        _ => ("", 2),
    };
    let most = decimals.unwrap_or(2).min(12) as usize;
    let least = minor.min(most);
    let mut text = format!("{:.most$}", value.abs());
    if let Some(point) = text.find('.') {
        let keep = text.trim_end_matches('0').len().max(point + 1 + least);
        text.truncate(keep);
        if text.ends_with('.') {
            text.pop();
        }
    }
    let (whole, fraction) = text
        .split_once('.')
        .map_or((text.as_str(), ""), |(whole, fraction)| (whole, fraction));
    let mut grouped = String::new();
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if !fraction.is_empty() {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    let sign = if value < 0.0 && grouped.bytes().any(|byte| byte.is_ascii_digit() && byte != b'0') {
        "-"
    } else {
        ""
    };
    if symbol.is_empty() {
        format!("{sign}{code} {grouped}")
    } else {
        format!("{sign}{symbol}{grouped}")
    }
}

/// [`formatted`] in the row's own unit, when the field has one per row (`units`, from a dataset field's `unitField`).
fn formatted_at(value: &Value, field: &Value, row: usize) -> String {
    match field["units"][row].as_str() {
        Some(unit) => {
            let mut field = field.clone();
            field["unit"] = Value::String(unit.into());
            formatted(value, &field)
        }
        None => formatted(value, field),
    }
}

fn formatted(value: &Value, field: &Value) -> String {
    if value.is_null() {
        return field["noValue"].as_str().unwrap_or("—").into();
    }
    if let Some(text) = mappings_for(value, field).find_map(|mapping| mapping["text"].as_str()) {
        return text.into();
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
        if let Some(code) = field["unit"].as_str().and_then(currency_code) {
            return currency_text(value, code, field["decimals"].as_u64());
        }
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
    /// The x of each point, in order, and the field they come from (none: the row number).
    coordinates: Vec<Value>,
    x: Option<Value>,
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
        x: x.map(|index| fields[index].clone()),
        coordinates,
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
                let field: Value = [
                    "name",
                    "type",
                    "unit",
                    "decimals",
                    "noValue",
                    "mappings",
                    "min",
                    "max",
                    "thresholds",
                    "thresholdMode",
                ]
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
    panel_view(frame, panel, width, true)
}

/// A panel's lines. A chart replaces the table unless `table` (a single panel, or a dashboard panel in table mode,
/// whose cells take the arrows, links and filters); a chart that does not fit or has nothing to draw is the table.
fn panel_view(frame: &Value, panel: &Value, width: u16, table: bool) -> Vec<Line> {
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
        // A panel that could not be read (a dataset the viewer may not read, a failed query) says so, never "no data".
        match unavailable_notice(panel) {
            Some(message) => lines.push(vec![Span::new(format!("Not available: {message}"), Role::Negative)]),
            None => lines.push(vec![Span::new("No data in this time range", Role::Muted)]),
        }
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
    if let Some(chart) = visual::chart(frame, panel, visual, usize::from(width)) {
        lines.extend(chart);
        if !table || matches!(visual, "gauge" | "barGauge") {
            return lines;
        }
    }
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
            .map(|field| fit(&formatted_at(&field["values"][at], field, at), col_width))
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
            let Some(explore) = app_link(panel, "explore", "Explore") else {
                return Some(vec![Act::Message("Explore is not available here".into())]);
            };
            return Some(vec![Act::Follow(format!(
                "{explore}?datasetId={}&filtersJson={}",
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
            // The dashboard's time range, so a drill-down keeps it.
            for (name, key) in [("${__from}", "from"), ("${__thru}", "thru")] {
                let range = dashboard.map_or(Value::Null, |dashboard| dashboard["absoluteTime"][key].clone());
                url = url.replace(name, &encoded(&text_of(Some(&range))));
            }
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
/// The path of a report screen where the app that renders the dashboard mounts it: `appLinks` (Dashboard.xml, on a
/// dashboard or a standalone panel) maps `definition`, `panel`, `explore`, `alertRule`, `dashboard` and `drill` to server paths,
/// and a link the app leaves out or sets to null is not offered (None). Without `appLinks` it is Tools' Dashboards
/// screen (`tools`).
fn app_link(owner: &Value, key: &str, tools: &str) -> Option<String> {
    let Some(links) = owner["appLinks"].as_object() else {
        return Some(format!("/apps/dashboards/{tools}"));
    };
    links
        .get(key)
        .and_then(Value::as_str)
        .filter(|path| path.starts_with('/') && !path.starts_with("//") && !path.contains(['?', '#']))
        .map(str::to_owned)
}

/// The message of a panel's first error notice that left it incomplete, when it has no rows to show.
fn unavailable_notice(panel: &Value) -> Option<String> {
    panel["notices"]
        .as_array()?
        .iter()
        .find(|notice| notice["level"] == "error" && notice["complete"] == false)
        .map(|notice| {
            notice["message"]
                .as_str()
                .unwrap_or("this panel could not be read")
                .to_owned()
        })
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
    // A view of the dashboard keeps its single-panel view; the panel requests name their panel.
    let panel_id = panel_id.or_else(|| {
        dashboard["panelId"]
            .as_str()
            .filter(|panel| action.is_empty() && !panel.is_empty())
    });
    if let Some(panel) = panel_id {
        pairs.push(("panelId".into(), panel.into()));
    }
    if let Some(mode) = mode {
        pairs.push(("renderMode".into(), mode.into()));
    }
    // The screen that renders the dashboard (a Monitor screen includes Dashboard and its transitions).
    let screen = dashboard["screenUrl"]
        .as_str()
        .filter(|url| url.starts_with('/') && !url.starts_with("//") && !url.contains(['?', '#']))
        .unwrap_or("/apps/dashboards/Dashboard");
    let path = if action.is_empty() {
        screen.to_string()
    } else {
        format!("{screen}/{action}")
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
/// Kiosk mode: the dashboard alone on the terminal, as `kiosk=true` opens it or `d k` turns it on.
fn kiosk(data: &Value, state: &ComponentState) -> bool {
    state.local["kiosk"]
        .as_bool()
        .unwrap_or(data["dashboard"]["kiosk"] == true)
}

/// The dashboard's refresh interval in seconds; under 5 it does not refresh on its own, as in the browser.
fn refresh_seconds(data: &Value) -> Option<u64> {
    number(&data["dashboard"]["refreshSeconds"])
        .filter(|seconds| *seconds >= 5.0)
        .map(|seconds| (seconds.min(86_400.0)) as u64)
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

/// The time picker's quick ranges, as the web picker lists them: from, through and label.
/// The route of System › Monitor, where a trace opens.
const MONITOR_PATH: &str = "/apps/system/Monitor";

const QUICK_RANGES: [(&str, &str, &str); 22] = [
    ("now-5m", "now", "Last 5 minutes"),
    ("now-15m", "now", "Last 15 minutes"),
    ("now-30m", "now", "Last 30 minutes"),
    ("now-1h", "now", "Last 1 hour"),
    ("now-3h", "now", "Last 3 hours"),
    ("now-6h", "now", "Last 6 hours"),
    ("now-12h", "now", "Last 12 hours"),
    ("now-24h", "now", "Last 24 hours"),
    ("now-2d", "now", "Last 2 days"),
    ("now-7d", "now", "Last 7 days"),
    ("now-30d", "now", "Last 30 days"),
    ("now-90d", "now", "Last 90 days"),
    ("now/d", "now/d", "Today"),
    ("now/d", "now", "Today so far"),
    ("now-1d/d", "now-1d/d", "Yesterday"),
    ("now/w", "now/w", "This week"),
    ("now/w", "now", "This week so far"),
    ("now-1w/w", "now-1w/w", "Previous week"),
    ("now/M", "now/M", "This month"),
    ("now-1M/M", "now-1M/M", "Previous month"),
    ("now/y", "now/y", "This year"),
    ("now-1y/y", "now-1y/y", "Previous year"),
];

/// The panel menu's items for a panel, as the web panel menu has them: an id and its label.
fn panel_menu(dashboard: &Value, panel: &Value) -> Vec<(&'static str, &'static str)> {
    let mut items = vec![("view", "View")];
    if app_link(dashboard, "definition", "Definition").is_some() {
        items.push(("edit", "Edit"));
    }
    items.push(("share", "Share"));
    if panel["queries"].as_array().is_some_and(|queries| !queries.is_empty())
        && app_link(dashboard, "explore", "Explore").is_some()
    {
        items.push(("explore", "Explore"));
    }
    items.push(("inspect", "Inspect"));
    if drill_link(dashboard).is_some() {
        items.push(("drill", "Drill"));
    }
    if dashboard["editable"] == true && app_link(dashboard, "dashboard", "Dashboard").is_some() {
        items.push(("duplicate", "Duplicate"));
        items.push(("remove", "Remove"));
    }
    let alerting = panel["queries"].as_array().into_iter().flatten().any(|query| {
        query["aggregations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|aggregation| aggregation["role"] == "metric")
    });
    if alerting && app_link(dashboard, "alertRule", "AlertRule").is_some() {
        items.push(("alert", "New alert rule"));
    }
    items
}

/// The drill screen of the app that mounts the dashboard (`appLinks.drill`): Tools mounts none, so there is no fallback.
fn drill_link(dashboard: &Value) -> Option<String> {
    dashboard["appLinks"].as_object()?;
    app_link(dashboard, "drill", "")
}

/// Explore over the panel's first query, with its filters and the dashboard's filters on the same dataset, and the range.
fn explore_url(dashboard: &Value, panel: &Value) -> Option<String> {
    let query = panel["queries"].get(0)?;
    let dataset = query["datasetId"].as_str()?;
    let mut pills: Vec<Value> = query["filters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            if item["query"].is_object() {
                let mut pill = item.clone();
                pill["datasetId"] = Value::String(dataset.into());
                pill
            } else {
                serde_json::json!({"datasetId":dataset,"query":item,"label":"","negated":false,"disabled":false,"pinned":false})
            }
        })
        .collect();
    pills.extend(
        dashboard["filters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|pill| pill["datasetId"].as_str().is_none_or(|id| id == dataset))
            .cloned(),
    );
    pills.truncate(32);
    let explore = app_link(dashboard, "explore", "Explore")?;
    let mut url = format!(
        "{explore}?datasetId={}&filtersJson={}",
        encoded(dataset),
        encoded(&Value::Array(pills).to_string())
    );
    if let Some(text) = query["queryString"].as_str().filter(|text| !text.is_empty()) {
        url.push_str(&format!("&queryString={}", encoded(text)));
    }
    for key in ["from", "thru"] {
        if let Some(value) = dashboard["absoluteTime"][key].as_str() {
            url.push_str(&format!("&{key}={}", encoded(value)));
        }
    }
    Some(url)
}

/// What a panel menu item does. Remove asks for a second Enter first.
fn panel_menu_act(item: &str, dashboard: &Value, id: &str, panel: &Value, state: &mut ComponentState) -> Vec<Act> {
    let source = panel["repeatOf"].as_str().unwrap_or(id);
    let definition = dashboard["definitionId"].as_str().unwrap_or("");
    match item {
        "view" => vec![Act::Follow(dashboard_url(dashboard, "", Some(id), None))],
        "edit" => app_link(dashboard, "definition", "Definition").map_or_else(Vec::new, |editor| {
            vec![Act::Follow(format!(
                "{editor}?definitionId={}&panelId={}",
                encoded(definition),
                encoded(source)
            ))]
        }),
        "share" => vec![Act::Message(format!(
            "Link to this panel with the time made absolute: {}",
            dashboard_url(dashboard, "", Some(id), None)
        ))],
        "explore" => explore_url(dashboard, panel).map_or_else(Vec::new, |url| vec![Act::Follow(url)]),
        "drill" => drill_link(dashboard).map_or_else(Vec::new, |drill| {
            let mut url = format!(
                "{drill}?definitionId={}&panelId={}",
                encoded(definition),
                encoded(source)
            );
            for key in ["from", "thru"] {
                if let Some(value) = dashboard["absoluteTime"][key].as_str() {
                    url.push_str(&format!("&{key}={}", encoded(value)));
                }
            }
            if dashboard["variableValues"].is_object() {
                url.push_str(&format!(
                    "&variablesJson={}",
                    encoded(&dashboard["variableValues"].to_string())
                ));
            }
            vec![Act::Follow(url)]
        }),
        "inspect" => {
            state.closed.insert("__report_inspector".into());
            state.local["inspectionRequest"] = Value::Bool(true);
            vec![Act::Read(dashboard_url(dashboard, "inspectPanel", Some(id), None))]
        }
        "duplicate" | "remove" => {
            if item == "remove" && state.local["confirmRemove"].as_str() != Some(id) {
                state.local["confirmRemove"] = Value::String(id.into());
                state.local["menu"] = state.local["menu"].clone();
                return vec![Act::Message(
                    "Enter again removes the panel; a new version is saved, and an earlier one can be restored".into(),
                )];
            }
            state.local["confirmRemove"] = Value::Null;
            let Some(screen) = app_link(dashboard, "dashboard", "Dashboard") else {
                return Vec::new();
            };
            vec![Act::Post {
                url: format!(
                    "{screen}/{}",
                    if item == "remove" {
                        "removePanel"
                    } else {
                        "duplicatePanel"
                    }
                ),
                fields: BTreeMap::from([
                    ("definitionId".to_string(), definition.to_string()),
                    ("panelId".to_string(), source.to_string()),
                ]),
            }]
        }
        "alert" => app_link(dashboard, "alertRule", "AlertRule").map_or_else(Vec::new, |rule| {
            vec![Act::Follow(format!(
                "{rule}?dashboardId={}&panelId={}",
                encoded(definition),
                encoded(id)
            ))]
        }),
        _ => Vec::new(),
    }
}

/// A list of choices drawn under a heading, the chosen one marked.
fn choice_lines(heading: &str, choices: &[&str], at: usize) -> Vec<Line> {
    let mut lines = vec![vec![Span::new(heading.to_string(), Role::Primary)]];
    lines.extend(choices.iter().enumerate().map(|(index, text)| {
        vec![Span::new(
            format!("{} {text}", if index == at { "▶" } else { " " }),
            if index == at { Role::Primary } else { Role::Text },
        )]
    }));
    lines
}

/// Up/Down move in a list of `count` open choices kept at `state.local[key]`, Esc closes it, Enter returns the chosen
/// index. Any other key keeps the list open and does nothing.
fn choose(state: &mut ComponentState, key: &str, count: usize, press: &KeyPress) -> Option<Option<usize>> {
    let at = state.local[key].as_u64()? as usize;
    match press.key {
        Key::Up => state.local[key] = Value::from(at.saturating_sub(1)),
        Key::Down => state.local[key] = Value::from((at + 1).min(count.saturating_sub(1))),
        Key::Escape => {
            state.local[key] = Value::Null;
            state.local["confirmRemove"] = Value::Null;
        }
        Key::Enter => return Some(Some(at.min(count.saturating_sub(1)))),
        _ => {}
    }
    Some(None)
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
        let kiosk = kiosk(data, state);
        if kiosk {
            lines.push(vec![Span::new(
                format!(
                    "{}  (kiosk; Esc or d k leaves)",
                    data["dashboard"]["title"].as_str().unwrap_or("Dashboard")
                ),
                Role::Muted,
            )]);
        } else if let Some(panel) = data["dashboard"]["panelId"].as_str().filter(|panel| !panel.is_empty()) {
            lines.push(vec![Span::new(
                format!("Showing panel {panel}; Esc shows all panels"),
                Role::Muted,
            )]);
        }
        if let Some(at) = state.local["picker"].as_u64() {
            let labels: Vec<&str> = QUICK_RANGES.iter().map(|(_, _, label)| *label).collect();
            lines.extend(choice_lines(
                "Time range (↑/↓ choose, Enter applies, Esc closes)",
                &labels,
                at as usize,
            ));
        }
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
            // Kiosk mode scrolls by panel: the selected one is at the top.
            if kiosk && at < state.at {
                continue;
            }
            if at == state.at && !kiosk {
                lines.push(vec![Span::new(
                    "▶ Selected panel; m opens its menu, i inspects, c/x exports CSV/XLSX",
                    Role::Muted,
                )]);
                if let Some(chosen) = state.local["menu"].as_u64() {
                    let items = panel_menu(&data["dashboard"], panel);
                    let labels: Vec<&str> = items.iter().map(|(_, label)| *label).collect();
                    lines.extend(choice_lines(
                        "Panel menu (↑/↓ choose, Enter runs, Esc closes)",
                        &labels,
                        chosen as usize,
                    ));
                }
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
            lines.extend(panel_view(
                &frame,
                panel,
                width,
                at == state.at && state.closed.contains("__table_mode"),
            ));
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
        // PgUp/PgDn scroll within kiosk mode; the title line stays.
        let offset = state.local["kioskOffset"].as_u64().unwrap_or(0) as usize;
        if kiosk && offset > 0 && lines.len() > 1 {
            let skip = offset.min(lines.len() - 2);
            lines.drain(1..1 + skip);
        }
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        let data = state.data.as_ref().unwrap_or(props).clone();
        if press.is_plain() {
            let dashboard = &data["dashboard"];
            // An open time picker or panel menu takes the keys until Enter or Esc.
            if let Some(chosen) = choose(state, "picker", QUICK_RANGES.len(), press) {
                if let Some(at) = chosen {
                    state.local["picker"] = Value::Null;
                    let (from, thru, _) = QUICK_RANGES[at];
                    let mut next = dashboard.clone();
                    next["absoluteTime"] = serde_json::json!({"from":from,"thru":thru});
                    return Some(vec![Act::Follow(dashboard_url(&next, "", None, None))]);
                }
                return Some(Vec::new());
            }
            let entries = dashboard_entries(&data, state);
            if let Some((id, panel)) = entries.get(state.at) {
                let items = panel_menu(dashboard, panel);
                if let Some(chosen) = choose(state, "menu", items.len(), press) {
                    if let Some(at) = chosen {
                        let acts = panel_menu_act(items[at].0, dashboard, id, panel, state);
                        if state.local["confirmRemove"].is_null() {
                            state.local["menu"] = Value::Null;
                        }
                        return Some(acts);
                    }
                    return Some(Vec::new());
                }
            }
            if kiosk(&data, state) && matches!(press.key, Key::PageDown | Key::PageUp) {
                let offset = state.local["kioskOffset"].as_u64().unwrap_or(0);
                state.local["kioskOffset"] = Value::from(if press.key == Key::PageDown {
                    offset + 10
                } else {
                    offset.saturating_sub(10)
                });
                return Some(Vec::new());
            }
        }
        // Esc leaves kiosk mode first, then the inspector, then the single-panel view; `d k` and Alt+K toggle it.
        if press.alt && !press.ctrl && press.key == Key::Char('k') {
            state.local["kiosk"] = Value::Bool(!kiosk(&data, state));
            return Some(Vec::new());
        }
        if press.is_plain() {
            if press.key == Key::Escape && kiosk(&data, state) {
                state.closed.remove("__d_prefix");
                state.local["kiosk"] = Value::Bool(false);
                return Some(Vec::new());
            }
            if state.closed.remove("__d_prefix") {
                if press.key == Key::Char('k') {
                    state.local["kiosk"] = Value::Bool(!kiosk(&data, state));
                    return Some(Vec::new());
                }
            } else if press.key == Key::Char('d') && !state.closed.contains("__time_prefix") {
                state.closed.insert("__d_prefix".into());
                return Some(Vec::new());
            }
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
            // Esc closes the inspector, then leaves a single-panel view for all the panels.
            if press.key == Key::Escape {
                state.closed.remove("__time_prefix");
                if state.closed.remove("__report_inspector") {
                    return Some(Vec::new());
                }
                if dashboard["panelId"].as_str().is_some_and(|panel| !panel.is_empty()) {
                    let mut all = dashboard.clone();
                    all["panelId"] = Value::Null;
                    return Some(vec![Act::Follow(dashboard_url(&all, "", None, None))]);
                }
                return None;
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
                    state.local["kioskOffset"] = Value::from(0);
                    return Some(Vec::new());
                }
                Key::Down => {
                    state.at = (state.at + 1).min(entries.len().saturating_sub(1));
                    state.local["kioskOffset"] = Value::from(0);
                    return Some(Vec::new());
                }
                Key::Char('p') => {
                    let requested = &dashboard["requestedTime"];
                    let current = QUICK_RANGES
                        .iter()
                        .position(|(from, thru, _)| requested["from"] == *from && requested["thru"] == *thru)
                        .unwrap_or(0);
                    state.local["picker"] = Value::from(current);
                    return Some(Vec::new());
                }
                Key::Char('m') if !entries.is_empty() => {
                    state.local["menu"] = Value::from(0);
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
                    let Some(editor) = app_link(dashboard, "definition", "Definition") else {
                        return Some(vec![Act::Message("Editing is not available here".into())]);
                    };
                    return Some(vec![Act::Follow(format!(
                        "{editor}?definitionId={}",
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
    fn kiosk(&self, props: &Value, state: &ComponentState) -> bool {
        kiosk(state.data.as_ref().unwrap_or(props), state)
    }
    fn poll_millis(&self, props: &Value, state: &ComponentState) -> Option<u64> {
        refresh_seconds(state.data.as_ref().unwrap_or(props)).map(|seconds| seconds * 1000)
    }
    fn tick(&self, props: &Value, state: &mut ComponentState) -> Vec<Act> {
        if state.loading {
            return Vec::new();
        }
        vec![if source_of(props, state).is_some() {
            Act::Reload
        } else {
            Act::ReloadScreen
        }]
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
            (
                "m",
                "Panel menu: view, edit, share, explore, inspect, duplicate, remove, new alert rule",
            ),
            ("p", "Time range picker (quick ranges)"),
            ("PgUp/PgDn", "Scroll in kiosk mode (↑/↓ go a panel at a time)"),
            ("v/Enter", "View panel"),
            ("Esc", "Leave kiosk; close inspector; all panels"),
            ("d k", "Kiosk mode (also Alt+K)"),
            ("e", "Edit definition"),
            ("c/x", "Export CSV/XLSX"),
            ("t-", "Zoom out"),
            ("t←/t→", "Shift time range"),
            ("r", "Refresh dashboard (also every refresh interval)"),
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
                let mut line = vec![
                    Span::new(format!("{title} {millis:>8.2}ms "), Role::Text),
                    Span::new(
                        format!("{}{}", " ".repeat(offset), "━".repeat(length.max(1))),
                        if span["status_code"] == "ERROR" {
                            Role::Negative
                        } else {
                            Role::Primary
                        },
                    ),
                ];
                // An LLM call's token use, from its gen_ai.* attributes.
                let tokens = |key: &str| {
                    let value = if span[key].is_null() {
                        &span["attributes"][key]
                    } else {
                        &span[key]
                    };
                    integer(value)
                };
                let (input, output) = (
                    tokens("gen_ai.usage.input_tokens"),
                    tokens("gen_ai.usage.output_tokens"),
                );
                if input.is_some() || output.is_some() {
                    let show = |value: Option<u64>| value.map_or_else(|| "?".to_string(), |value| value.to_string());
                    line.push(Span::new(
                        format!(" {}→{} tokens", show(input), show(output)),
                        Role::Muted,
                    ));
                }
                line
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
                        return Some(vec![Act::Follow(format!("{MONITOR_PATH}/TraceDetail?traceId={trace}"))]);
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
mod tests;

#[cfg(test)]
mod gap_tests;
