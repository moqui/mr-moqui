/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! The report charts in text: bar charts, pie and donut, heatmaps, state timelines and gauges (and, from `graph`, the
//! funnel, node graph, flame graph and geomap). Each draws the same frame the table does, in theme roles (so they read
//! in light and dark themes) with a fill per series or state as well (so they read without colour). Each answers None
//! when there is nothing it can draw or no room, and the panel shows its table instead.
use super::{all_configured_fields, formatted, is_metadata, mappings_for, number, text_of, trend_series, Trends};
use crate::components::{fit, role_of_color, Line, Span};
use crate::layout::display_width;
use crate::theme::Role;
use serde_json::Value;
use std::collections::BTreeMap;

/// Narrower than this, a chart is its table.
pub(super) const MIN_WIDTH: usize = 30;
/// The most categories, slices, tracks or gauges a chart draws; the table and the exports keep the rest.
const MAX_ITEMS: usize = 50;

/// The browser's series colours in order (`report.color`), as theme roles.
const PALETTE: [Role; 6] = [
    Role::Primary,
    Role::Positive,
    Role::Warning,
    Role::Info,
    Role::Negative,
    Role::Secondary,
];
/// A fill per item, so neighbouring slices and states differ in a terminal without colour too.
const FILLS: [char; 4] = ['█', '▓', '▒', '░'];
const EIGHTHS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];

/// The chart for a panel's visualization, when it has one in a terminal.
pub(super) fn chart(frame: &Value, panel: &Value, visual: &str, width: usize) -> Option<Vec<Line>> {
    if width < MIN_WIDTH {
        return None;
    }
    match visual {
        "bar" | "barChart" => bars(frame, panel, width),
        "pie" | "donut" => pie(frame, panel, width, visual == "donut"),
        "heatmap" => heatmap(frame, panel, width),
        "stateTimeline" | "state-timeline" => states(frame, panel, width),
        "gauge" | "barGauge" => gauges(frame, panel, width, visual == "barGauge"),
        "funnel" => super::graph::funnel(frame, panel, width),
        "nodeGraph" => super::graph::node_graph(frame, panel, width),
        "flameGraph" => super::graph::flame(frame, panel, width),
        "geomap" => super::graph::geomap(frame, panel, width),
        _ => None,
    }
}

fn palette(at: usize) -> (Role, char) {
    (PALETTE[at % PALETTE.len()], FILLS[at % FILLS.len()])
}

/// A theme role for a threshold or mapping colour name; a hex colour has none.
fn color_role(name: &str) -> Option<Role> {
    match name {
        "primary" | "secondary" | "positive" | "negative" | "warning" | "info" => Some(role_of_color(name)),
        "accent" => Some(Role::Secondary),
        _ => None,
    }
}

/// The colour of the last threshold a value reaches, as the browser's `report.thresholdColor` picks it.
pub(super) fn threshold_role(value: &Value, field: &Value) -> Option<Role> {
    let mut at = number(value).filter(|value| value.is_finite())?;
    if field["thresholdMode"] == "percent" {
        let min = number(&field["min"]).unwrap_or(0.0);
        let max = number(&field["max"]).unwrap_or(100.0);
        let span = if max - min == 0.0 { 1.0 } else { max - min };
        at = 100.0 * (at - min) / span;
    }
    let mut role = None;
    for threshold in field["thresholds"].as_array().into_iter().flatten() {
        if threshold["value"].is_null() || number(&threshold["value"]).is_some_and(|value| at >= value) {
            role = threshold["color"].as_str().and_then(color_role);
        }
    }
    role
}

/// A bar of `fraction` of `width` cells, in eighths.
fn bar(fraction: f64, width: usize) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let eighths = (fraction.clamp(0.0, 1.0) * width as f64 * 8.0).round() as usize;
    let mut text = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        text.push(EIGHTHS[eighths % 8 - 1]);
    }
    text
}

/// The label of a trend's point: its x formatted as its field, else the row number.
fn coordinate_label(trends: &Trends, at: usize) -> String {
    let value = trends.coordinates.get(at).unwrap_or(&Value::Null);
    trends
        .x
        .as_ref()
        .map_or_else(|| (at + 1).to_string(), |field| formatted(value, field))
}

/// Category bars: (label, [(value, formatted, series)]). One row of several metrics is a bar per metric.
type Category = (String, Vec<(Option<f64>, String, usize)>);

fn categories(trends: &Trends) -> Vec<Category> {
    if trends.coordinates.len() == 1 && trends.series.len() > 1 {
        return trends
            .series
            .iter()
            .enumerate()
            .map(|(at, series)| {
                let value = series.values.first().unwrap_or(&Value::Null);
                (
                    series.label.clone(),
                    vec![(number(value), formatted(value, &series.field), at)],
                )
            })
            .collect();
    }
    (0..trends.coordinates.len())
        .map(|at| {
            (
                coordinate_label(trends, at),
                trends
                    .series
                    .iter()
                    .enumerate()
                    .map(|(index, series)| {
                        let value = series.values.get(at).unwrap_or(&Value::Null);
                        (number(value), formatted(value, &series.field), index)
                    })
                    .collect(),
            )
        })
        .collect()
}

fn legend(trends: &Trends) -> Line {
    let mut line = Vec::new();
    for (at, series) in trends.series.iter().enumerate().take(MAX_ITEMS) {
        let (role, fill) = palette(at);
        line.push(Span::new(format!("{fill} "), role));
        line.push(Span::new(format!("{}  ", series.label), Role::Text));
    }
    line
}

fn more(shown: usize, count: usize, what: &str) -> Option<Line> {
    (count > shown).then(|| {
        vec![Span::new(
            format!("Showing {shown} of {count} {what}; the table and exports have all of them"),
            Role::Muted,
        )]
    })
}

/// A horizontal bar chart: a row a category (and a row a series in each), so long labels keep their room.
fn bars(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let trends = trend_series(frame, panel);
    if trends.series.is_empty() {
        return None;
    }
    let rows = categories(&trends);
    let shown = &rows[..rows.len().min(MAX_ITEMS)];
    let grouped = shown.iter().any(|(_, values)| values.len() > 1);
    let label_w = shown
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(4)
        .clamp(4, width / 3);
    let value_w = shown
        .iter()
        .flat_map(|(_, values)| values.iter().map(|(_, text, _)| display_width(text)))
        .max()
        .unwrap_or(1)
        .min(16);
    let room = width.saturating_sub(label_w + value_w + 4);
    if room < 8 {
        return None;
    }
    let max = shown
        .iter()
        .flat_map(|(_, values)| values.iter().filter_map(|(value, _, _)| *value))
        .map(f64::abs)
        .fold(0.0, f64::max);
    let mut lines = Vec::new();
    if grouped {
        lines.push(legend(&trends));
    }
    for (label, values) in shown {
        for (at, (value, text, series)) in values.iter().enumerate() {
            let field = &trends.series[*series].field;
            let role = value
                .and_then(|value| threshold_role(&Value::from(value), field))
                .unwrap_or(if grouped { palette(*series).0 } else { Role::Primary });
            let fraction = value.map_or(0.0, |value| if max > 0.0 { value.abs() / max } else { 0.0 });
            let drawn = bar(fraction, room);
            let pad = room.saturating_sub(display_width(&drawn));
            lines.push(vec![
                Span::new(fit(if at == 0 { label } else { "" }, label_w), Role::Text),
                Span::new(" │", Role::Border),
                Span::new(drawn, role),
                Span::new(format!("{} {:>value_w$}", " ".repeat(pad), text), Role::Text),
            ]);
        }
    }
    lines.extend(more(shown.len(), rows.len(), "categories"));
    Some(lines)
}

/// A pie as a proportional bar over the whole width, two rows high; a donut one row with its total. Then the legend:
/// each slice with its value and percent.
fn pie(frame: &Value, panel: &Value, width: usize, donut: bool) -> Option<Vec<Line>> {
    let trends = trend_series(frame, panel);
    let first = trends.series.first()?;
    let slices: Vec<(String, f64, String)> = categories(&trends)
        .into_iter()
        .filter_map(|(label, values)| {
            let (value, text, _) = values.into_iter().next()?;
            value
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|value| (label, value, text))
        })
        .collect();
    let total: f64 = slices.iter().map(|(_, value, _)| value).sum();
    if slices.is_empty() || total <= 0.0 {
        return None;
    }
    let shown = slices.len().min(MAX_ITEMS);
    let room = width.saturating_sub(2);
    let mut track = Vec::new();
    let mut before = 0.0;
    let mut start = 0;
    for (at, (_, value, _)) in slices.iter().enumerate().take(shown) {
        before += value;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let end = ((before / total) * room as f64).round() as usize;
        let (role, fill) = palette(at);
        if end > start {
            track.push(Span::new(fill.to_string().repeat(end - start), role));
        }
        start = end.max(start);
    }
    let mut lines = vec![track.clone()];
    if donut {
        let field = if trends.coordinates.len() == 1 && trends.series.len() > 1 {
            Value::Null
        } else {
            first.field.clone()
        };
        lines.push(vec![Span::new(
            format!("Total {}", formatted(&Value::from(total), &field)),
            Role::Text,
        )
        .bold()]);
    } else {
        lines.push(track);
    }
    let label_w = slices
        .iter()
        .take(shown)
        .map(|(label, _, _)| display_width(label))
        .max()
        .unwrap_or(4)
        .clamp(4, width / 2);
    let value_w = slices
        .iter()
        .take(shown)
        .map(|(_, _, text)| display_width(text))
        .max()
        .unwrap_or(1)
        .min(16);
    for (at, (label, value, text)) in slices.iter().enumerate().take(shown) {
        let (role, fill) = palette(at);
        lines.push(vec![
            Span::new(format!("{fill} "), role),
            Span::new(fit(label, label_w), Role::Text),
            Span::new(
                format!("  {text:>value_w$}  {:>5.1}%", 100.0 * value / total),
                Role::Text,
            ),
        ]);
    }
    if slices.len() > shown {
        let rest: f64 = slices[shown..].iter().map(|(_, value, _)| value).sum();
        lines.push(vec![Span::new(
            format!(
                "{} more slices, {:.1}%; the table and exports have all of them",
                slices.len() - shown,
                100.0 * rest / total
            ),
            Role::Muted,
        )]);
    }
    Some(lines)
}

/// The x field of a grid chart (time, else the first category) and the rows in its order.
fn grid_rows(fields: &[Value]) -> Option<(usize, Vec<usize>)> {
    let x = fields
        .iter()
        .position(|field| !is_metadata(field) && field["type"] == "time")
        .or_else(|| {
            fields
                .iter()
                .position(|field| !is_metadata(field) && !matches!(field["type"].as_str(), Some("number" | "integer")))
        })?;
    let count = fields[x]["values"].as_array().map_or(0, Vec::len);
    let mut rows: Vec<usize> = (0..count).collect();
    if fields[x]["type"] == "time" {
        let time = |row: &usize| {
            let value = &fields[x]["values"][*row];
            number(value).or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.parse::<jiff::Timestamp>().ok())
                    .and_then(|time| time.as_millisecond().to_string().parse().ok())
            })
        };
        rows.sort_by(|a, b| match (time(a), time(b)) {
            (Some(left), Some(right)) => left.total_cmp(&right),
            _ => a.cmp(b),
        });
    }
    (count > 0).then_some((x, rows))
}

/// The columns of a grid chart: the most recent points that fit, and how wide each cell is.
fn grid_columns(rows: &[usize], room: usize) -> (usize, usize) {
    let points = rows.len().min(room.max(1));
    (rows.len() - points, (room / points.max(1)).clamp(1, 8))
}

/// The time axis under a grid: the first point's label at the left and the last one's at the right.
fn axis(fields: &[Value], x: usize, rows: &[usize], indent: usize, room: usize) -> Line {
    let label = |row: usize| formatted(&fields[x]["values"][row], &fields[x]);
    let first = label(rows[0]);
    let last = label(rows[rows.len() - 1]);
    let gap = room.saturating_sub(display_width(&first) + display_width(&last));
    let text = if rows.len() < 2 {
        first
    } else if gap >= 2 {
        format!("{first}{}{last}", " ".repeat(gap))
    } else {
        format!("{first} → {last}")
    };
    vec![Span::new(format!("{}{text}", " ".repeat(indent)), Role::Muted)]
}

/// Cells as spans, a run of the same look one span.
fn runs(cells: impl Iterator<Item = (char, Role)>, cell: usize) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for (fill, role) in cells {
        let text = fill.to_string().repeat(cell);
        match spans.last_mut() {
            Some(last) if last.role == role && last.text.starts_with(fill) => last.text.push_str(&text),
            _ => spans.push(Span::new(text, role)),
        }
    }
    spans
}

/// The bucket number of a histogram bucket field the frame builder writes, `latency.bucket.3`.
fn bucket_index(field: &Value) -> Option<u64> {
    let name = field["name"].as_str()?;
    let (_, index) = name.rsplit_once(".bucket.")?;
    index.parse().ok()
}

/// A heatmap over time × bucket: a row a histogram bucket (highest at the top), a column a time, shaded by count.
/// Without histogram bucket fields, a row a numeric field, as the browser's grid has them.
fn heatmap(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let fields: Vec<Value> = all_configured_fields(frame, panel)
        .into_iter()
        .filter(|field| field["hidden"] != true)
        .collect();
    let (x, rows) = grid_rows(&fields)?;
    let mut buckets: Vec<(u64, &Value)> = fields
        .iter()
        .filter_map(|field| bucket_index(field).map(|index| (index, field)))
        .collect();
    buckets.sort_by_key(|(index, _)| *index);
    let buckets: Vec<(String, &Value)> = if buckets.is_empty() {
        fields
            .iter()
            .enumerate()
            .filter(|(at, field)| {
                *at != x && !is_metadata(field) && matches!(field["type"].as_str(), Some("number" | "integer"))
            })
            .map(|(_, field)| (text_of(field.get("displayName").or(field.get("name"))), field))
            .collect()
    } else {
        buckets
            .into_iter()
            .map(|(index, field)| {
                let label = field["displayName"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| field["le"].as_str().map(|bound| format!("≤{bound}")))
                    .or_else(|| number(&field["le"]).map(|bound| format!("≤{bound}")))
                    .unwrap_or_else(|| format!("bucket {index}"));
                (label, field)
            })
            .collect()
    };
    if buckets.is_empty() {
        return None;
    }
    let shown = buckets.len().min(MAX_ITEMS);
    let label_w = buckets
        .iter()
        .take(shown)
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(4)
        .clamp(4, 16);
    let room = width.saturating_sub(label_w + 2);
    let (start, cell) = grid_columns(&rows, room);
    let rows = &rows[start..];
    let max = buckets
        .iter()
        .take(shown)
        .flat_map(|(_, field)| rows.iter().filter_map(|row| number(&field["values"][*row])))
        .fold(0.0, f64::max);
    let mut lines = Vec::new();
    for (label, field) in buckets.iter().take(shown).rev() {
        let cells = rows.iter().map(|row| match number(&field["values"][*row]) {
            None => ('·', Role::Muted),
            Some(value) if value <= 0.0 || max <= 0.0 => (' ', Role::Primary),
            #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(value) => {
                let level = ((value / max) * 4.0).ceil() as usize;
                (FILLS[4 - level.clamp(1, 4)], Role::Primary)
            }
        });
        let mut line = vec![Span::new(format!("{} │", fit(label, label_w)), Role::Text)];
        line.extend(runs(cells, cell));
        lines.push(line);
    }
    lines.push(axis(&fields, x, rows, label_w + 2, rows.len() * cell));
    let top = formatted(&Value::from(max), buckets[0].1);
    lines.push(vec![Span::new(
        format!("░ to 25%  ▒ to 50%  ▓ to 75%  █ to the most, {top}; · no value"),
        Role::Muted,
    )]);
    lines.extend(more(rows.len(), rows.len() + start, "times"));
    lines.extend(more(shown, buckets.len(), "buckets"));
    Some(lines)
}

/// A state timeline: a track a series over time, each state a colour and a fill (a threshold or mapping colour when
/// the field has one), with the states' key under it.
fn states(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let fields: Vec<Value> = all_configured_fields(frame, panel)
        .into_iter()
        .filter(|field| field["hidden"] != true)
        .collect();
    let (x, rows) = grid_rows(&fields)?;
    // The state fields: text, and numbers with thresholds or mappings to colour them; plain numbers only when there
    // is nothing else, as each distinct number is a state of its own.
    let candidates: Vec<&Value> = fields
        .iter()
        .enumerate()
        .filter(|(at, field)| *at != x && !is_metadata(field))
        .map(|(_, field)| field)
        .collect();
    let stated = |field: &&Value| {
        !matches!(field["type"].as_str(), Some("number" | "integer"))
            || field["thresholds"].as_array().is_some_and(|items| !items.is_empty())
            || field["mappings"].as_array().is_some_and(|items| !items.is_empty())
    };
    let tracks: Vec<&Value> = if candidates.iter().any(stated) {
        candidates.into_iter().filter(stated).collect()
    } else {
        candidates
    };
    if tracks.is_empty() {
        return None;
    }
    let shown = tracks.len().min(MAX_ITEMS);
    let label = |field: &Value| text_of(field.get("displayName").or(field.get("name")));
    let label_w = tracks
        .iter()
        .take(shown)
        .map(|field| display_width(&label(field)))
        .max()
        .unwrap_or(4)
        .clamp(4, 20);
    let room = width.saturating_sub(label_w + 2);
    let (start, cell) = grid_columns(&rows, room);
    let rows = &rows[start..];
    // The states in the order they first show, each with its look.
    let mut keyed: BTreeMap<String, usize> = BTreeMap::new();
    let mut key: Vec<(String, Role, char)> = Vec::new();
    let mut thresholds = false;
    let mut lines = Vec::new();
    for field in tracks.iter().take(shown) {
        let mut cells = Vec::new();
        for row in rows {
            let value = &field["values"][*row];
            if value.is_null() {
                cells.push((' ', Role::Muted));
                continue;
            }
            if let Some(role) = threshold_role(value, field) {
                thresholds = true;
                cells.push(('█', role));
                continue;
            }
            let text = formatted(value, field);
            let at = *keyed.entry(text.clone()).or_insert_with(|| {
                let (mut role, fill) = palette(key.len());
                if let Some(color) = mappings_for(value, field)
                    .find_map(|mapping| mapping["color"].as_str())
                    .and_then(color_role)
                {
                    role = color;
                }
                key.push((text, role, fill));
                key.len() - 1
            });
            cells.push((key[at].2, key[at].1));
        }
        let mut line = vec![Span::new(format!("{} │", fit(&label(field), label_w)), Role::Text)];
        line.extend(runs(cells.into_iter(), cell));
        lines.push(line);
    }
    lines.push(axis(&fields, x, rows, label_w + 2, rows.len() * cell));
    let mut legend = Vec::new();
    for (text, role, fill) in key.iter().take(MAX_ITEMS) {
        legend.push(Span::new(format!("{fill} "), *role));
        legend.push(Span::new(format!("{text}  "), Role::Text));
    }
    if thresholds {
        legend.push(Span::new("█ by threshold", Role::Muted));
    }
    if !legend.is_empty() {
        lines.push(legend);
    }
    lines.extend(more(rows.len(), rows.len() + start, "times"));
    lines.extend(more(shown, tracks.len(), "series"));
    Some(lines)
}

/// Gauges: a series' latest value between its field's min and max (0 to 100 unless set, or the largest value when one
/// passes 100), in its threshold colour. A gauge is a line, a bar gauge a bar in eighths.
fn gauges(frame: &Value, panel: &Value, width: usize, solid: bool) -> Option<Vec<Line>> {
    let trends = trend_series(frame, panel);
    if trends.series.is_empty() {
        return None;
    }
    // Over time (or rows), a series' latest value; over categories, a gauge for each category.
    let over_time = trends
        .x
        .as_ref()
        .is_none_or(|field| matches!(field["type"].as_str(), Some("time" | "number" | "integer")));
    let items: Vec<(String, Value, &Value)> = if over_time || trends.coordinates.len() == 1 {
        trends
            .series
            .iter()
            .map(|series| {
                let latest = series.values.iter().rev().find(|value| !value.is_null());
                (
                    series.label.clone(),
                    latest.cloned().unwrap_or(Value::Null),
                    &series.field,
                )
            })
            .collect()
    } else {
        let trends = &trends;
        let several = trends.series.len() > 1;
        (0..trends.coordinates.len())
            .flat_map(|at| {
                let category = coordinate_label(trends, at);
                trends.series.iter().map(move |series| {
                    let label = if several {
                        format!("{category} · {}", series.label)
                    } else {
                        category.clone()
                    };
                    (label, series.values[at].clone(), &series.field)
                })
            })
            .collect()
    };
    let largest = items
        .iter()
        .filter_map(|(_, value, _)| number(value))
        .fold(f64::NEG_INFINITY, f64::max);
    let shown = items.len().min(MAX_ITEMS);
    let texts: Vec<String> = items.iter().map(|(_, value, field)| formatted(value, field)).collect();
    let label_w = items
        .iter()
        .take(shown)
        .map(|(label, _, _)| display_width(label))
        .max()
        .unwrap_or(4)
        .clamp(4, width / 3);
    let value_w = texts
        .iter()
        .take(shown)
        .map(|text| display_width(text))
        .max()
        .unwrap_or(1)
        .min(16);
    let room = width.saturating_sub(label_w + value_w + 3);
    if room < 8 {
        return None;
    }
    let mut lines = Vec::new();
    for ((label, value, field), text) in items.iter().zip(&texts).take(shown) {
        let min = number(&field["min"]).unwrap_or(0.0);
        let max = number(&field["max"]).unwrap_or(if largest > 100.0 { largest } else { 100.0 });
        let role = threshold_role(value, field).unwrap_or(Role::Primary);
        let fraction = number(value).map_or(0.0, |value| if max > min { (value - min) / (max - min) } else { 1.0 });
        let (filled, rest) = if solid {
            let drawn = bar(fraction, room);
            let rest = room.saturating_sub(display_width(&drawn));
            (drawn, "░".repeat(rest))
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let cells = ((fraction.clamp(0.0, 1.0) * room as f64).round() as usize).min(room);
            ("━".repeat(cells), "─".repeat(room - cells))
        };
        lines.push(vec![
            Span::new(format!("{} ", fit(label, label_w)), Role::Text),
            Span::new(filled, role),
            Span::new(rest, Role::Muted),
            Span::new(format!(" {text:>value_w$}"), role).bold(),
        ]);
    }
    lines.extend(more(shown, items.len(), "gauges"));
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::line_text;
    use serde_json::json;

    fn text(lines: &[Line]) -> String {
        lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
    }

    fn roles(lines: &[Line]) -> Vec<Role> {
        lines.iter().flatten().map(|span| span.role).collect()
    }

    #[test]
    fn bar_charts_scale_to_the_largest_and_keep_long_labels() {
        let frame = json!({"fields":[
            {"name":"status","type":"string","values":["Approved and shipped","Cancelled"]},
            {"name":"orders","type":"number","values":[40,10]}
        ]});
        let lines = chart(&frame, &json!({}), "bar", 60).unwrap();
        let shown = text(&lines);
        assert!(shown.contains("Approved and shipped"), "{shown}");
        let full = line_text(&lines[0]).matches('█').count();
        let quarter = line_text(&lines[1]).matches('█').count();
        assert!(full > 3 * quarter && quarter > 0, "{shown}");
        assert!(line_text(&lines[0]).trim_end().ends_with("40"));
        // Too narrow for a chart: the table.
        assert!(chart(&frame, &json!({}), "bar", 20).is_none());
    }

    #[test]
    fn grouped_bars_have_a_legend_and_a_row_a_series() {
        let frame = json!({"fields":[
            {"name":"region","type":"string","values":["East","West"]},
            {"name":"sales","type":"number","values":[5,7]},
            {"name":"returns","type":"number","values":[1,null]}
        ]});
        let lines = chart(&frame, &json!({}), "bar", 60).unwrap();
        assert!(line_text(&lines[0]).contains("sales") && line_text(&lines[0]).contains("returns"));
        assert_eq!(lines.len(), 5);
        assert!(line_text(&lines[4]).contains('—'), "a missing value is not a zero bar");
        assert!(roles(&lines[1..3]).contains(&Role::Positive));
    }

    #[test]
    fn pies_and_donuts_are_proportional_with_percentages() {
        let frame = json!({"fields":[
            {"name":"status","type":"string","values":["Paid","Open","Void"]},
            {"name":"amount","type":"number","unit":"USD","values":[75,25,0]}
        ]});
        let lines = chart(&frame, &json!({}), "pie", 42).unwrap();
        let track = line_text(&lines[0]);
        assert_eq!(track.chars().filter(|fill| *fill == '█').count(), 30);
        assert_eq!(track.chars().filter(|fill| *fill == '▓').count(), 10);
        assert_eq!(lines[0], lines[1], "a pie is two rows high");
        let shown = text(&lines);
        assert!(
            shown.contains("Paid") && shown.contains("75.0%") && shown.contains("$75.00"),
            "{shown}"
        );
        assert!(!shown.contains("Void"), "an empty slice has no share");
        let donut = text(&chart(&frame, &json!({}), "donut", 42).unwrap());
        assert!(donut.contains("Total $100.00"), "{donut}");
        let empty = json!({"fields":[{"name":"status","type":"string","values":["A"]},{"name":"n","type":"number","values":[0]}]});
        assert!(chart(&empty, &json!({}), "pie", 42).is_none());
    }

    #[test]
    fn heatmaps_shade_histogram_buckets_over_time() {
        let frame = json!({"fields":[
            {"name":"time","type":"time","values":[2000,1000]},
            {"name":"latency.bucket.0","type":"number","values":[0,8]},
            {"name":"latency.bucket.1","type":"number","values":[2,null]},
            {"name":"latency.p50","type":"number","values":[1,1],"hidden":true}
        ]});
        let lines = chart(&frame, &json!({}), "heatmap", 40).unwrap();
        // The highest bucket on top; time runs left to right in time order.
        assert!(line_text(&lines[0]).starts_with("bucket 1"), "{}", text(&lines));
        let top = line_text(&lines[0]);
        let bottom = line_text(&lines[1]);
        assert!(top.contains("···") && top.contains("░░░"), "{top}");
        assert!(bottom.contains("███   "), "{bottom}");
        assert!(text(&lines).contains("1970-01-01T00:00:01Z"));
        assert!(text(&lines).contains("to the most, 8"));
        assert!(chart(
            &json!({"fields":[{"name":"time","type":"time","values":[1]}]}),
            &json!({}),
            "heatmap",
            40
        )
        .is_none());
    }

    #[test]
    fn state_timelines_colour_states_by_mapping_and_threshold() {
        let frame = json!({"fields":[
            {"name":"time","type":"time","values":[0,1000,2000,3000]},
            {"name":"health","type":"string","values":["up","up","down",null],
                "mappings":[{"value":"down","text":"Down","color":"negative"}]},
            {"name":"load","type":"number","values":[10,95,20,30],
                "thresholds":[{"value":null,"color":"positive"},{"value":90,"color":"negative"}]}
        ]});
        let lines = chart(&frame, &json!({}), "stateTimeline", 40).unwrap();
        let health = &lines[0];
        assert!(line_text(health).starts_with("health"));
        assert!(health
            .iter()
            .any(|span| span.role == Role::Negative && span.text.starts_with('▓')));
        assert!(health
            .iter()
            .any(|span| span.role == Role::Primary && span.text.starts_with('█')));
        assert!(line_text(health).ends_with("   "), "a missing state is a gap");
        let load = &lines[1];
        assert_eq!(
            load.iter().skip(1).map(|span| span.role).collect::<Vec<_>>(),
            [Role::Positive, Role::Negative, Role::Positive]
        );
        let key = line_text(lines.last().unwrap());
        assert!(
            key.contains("up") && key.contains("Down") && key.contains("by threshold"),
            "{key}"
        );
    }

    #[test]
    fn gauges_fill_to_the_value_in_threshold_colours() {
        let frame = json!({"fields":[
            {"name":"time","type":"time","values":[0,1000]},
            {"name":"cpu","type":"number","unit":"percent","values":[10,75],
                "thresholds":[{"value":null,"color":"positive"},{"value":70,"color":"warning"}]}
        ]});
        let lines = chart(&frame, &json!({}), "gauge", 50).unwrap();
        let line = &lines[0];
        assert_eq!(line[1].role, Role::Warning);
        let filled = line[1].text.chars().count();
        let rest = line[2].text.chars().count();
        assert_eq!((filled, rest), (30, 10), "three quarters of the room");
        assert!(line_text(line).ends_with("75%"));
        let bars = chart(&frame, &json!({}), "barGauge", 50).unwrap();
        assert!(bars[0][1].text.contains('█') && bars[0][2].text.contains('░'));
        // A value past 100 without a max scales to the largest value.
        let big = json!({"fields":[{"name":"requests","type":"number","values":[400]},{"name":"errors","type":"number","values":[200]}]});
        let lines = chart(&big, &json!({}), "gauge", 50).unwrap();
        assert!(lines[0][2].text.is_empty() && !lines[1][2].text.is_empty());
        // Over categories, a gauge a category.
        let warehouses = json!({"fields":[{"name":"warehouse","type":"string","values":["North","South"]},{"name":"full","type":"number","unit":"percent","values":[40,90]}]});
        let lines = chart(&warehouses, &json!({}), "barGauge", 50).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).starts_with("North") && line_text(&lines[1]).ends_with("90%"));
    }

    #[test]
    fn thresholds_follow_the_browser_rules() {
        let field = json!({"thresholdMode":"percent","min":0,"max":200,"thresholds":[{"value":null,"color":"positive"},{"value":50,"color":"#ff0000"},{"value":75,"color":"negative"}]});
        assert_eq!(threshold_role(&json!(20), &field), Some(Role::Positive));
        assert_eq!(
            threshold_role(&json!(120), &field),
            None,
            "a hex colour has no terminal role"
        );
        assert_eq!(threshold_role(&json!(160), &field), Some(Role::Negative));
        assert_eq!(threshold_role(&json!("x"), &field), None);
    }
}
