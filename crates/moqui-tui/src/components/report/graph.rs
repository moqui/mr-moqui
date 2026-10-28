/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! The funnel, node graph, flame graph and geomap panels in text, from the frames the browser draws: funnel steps as
//! centred bars, a node graph as each service's calls, a flame graph as an indented tree with a bar of each path's
//! share, and a geomap as braille points over a land mask. Each answers None when the frame has nothing it can draw.
use super::{all_configured_fields, formatted, is_metadata, number, numeric_field, text_of};
use crate::components::{fit, Line, Span};
use crate::layout::display_width;
use crate::theme::Role;
use serde_json::Value;
use std::collections::BTreeMap;

/// The most steps, edges, paths or places drawn; the table and exports keep the rest.
const MAX_ITEMS: usize = 60;

/// The visible fields of a frame, configured, with their values.
fn fields(frame: &Value, panel: &Value) -> Vec<Value> {
    all_configured_fields(frame, panel)
        .into_iter()
        .filter(|field| !is_metadata(field))
        .collect()
}
fn named<'a>(fields: &'a [Value], option: &Value, name: &str) -> Option<&'a Value> {
    let wanted = option.as_str().unwrap_or(name);
    fields.iter().find(|field| field["name"] == wanted)
}
fn values(field: &Value) -> &[Value] {
    field["values"].as_array().map_or(&[], Vec::as_slice)
}
fn first_number<'a>(fields: &'a [Value], option: &Value) -> Option<&'a Value> {
    option
        .as_str()
        .and_then(|name| fields.iter().find(|field| field["name"] == name))
        .or_else(|| fields.iter().find(|field| numeric_field(field)))
}
fn first_label(fields: &[Value]) -> Option<&Value> {
    fields
        .iter()
        .find(|field| !numeric_field(field) && field["type"] != "time")
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn cells(fraction: f64, width: usize) -> usize {
    (fraction.clamp(0.0, 1.0) * width as f64).round() as usize
}

/// Funnel steps: a centred bar each, with the share of the first step and of the one before.
pub(super) fn funnel(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let fields = fields(frame, panel);
    let options = &panel["funnel"];
    let value = first_number(&fields, &options["value"])?;
    let steps: Vec<(String, f64, String)> = match options["label"]
        .as_str()
        .and_then(|name| fields.iter().find(|field| field["name"] == name))
        .or_else(|| first_label(&fields))
    {
        Some(label) => values(label)
            .iter()
            .zip(values(value))
            .filter_map(|(name, amount)| number(amount).map(|n| (text_of(Some(name)), n, formatted(amount, value))))
            .collect(),
        None => fields
            .iter()
            .filter(|field| numeric_field(field))
            .filter_map(|field| {
                let amount = values(field).first()?;
                number(amount).map(|n| (text_of(Some(&field["name"])), n, formatted(amount, field)))
            })
            .collect(),
    };
    let first = steps.first()?.1;
    let max = steps.iter().map(|step| step.1).fold(0.0, f64::max);
    let mut lines = Vec::new();
    let mut previous: Option<f64> = None;
    for (label, amount, text) in steps.iter().take(MAX_ITEMS) {
        let share = |base: f64| {
            if base == 0.0 {
                String::new()
            } else {
                format!("{:.1}%", amount / base * 100.0)
            }
        };
        let mut caption = format!("{label}  {text}");
        if let Some(before) = previous {
            caption.push_str(&format!(" · {} of first · {} of previous", share(first), share(before)));
        }
        lines.push(vec![Span::new(fit(&caption, width), Role::Text)]);
        let drawn = cells(if max > 0.0 { amount.max(0.0) / max } else { 0.0 }, width).max(1);
        let pad = (width - drawn.min(width)) / 2;
        lines.push(vec![Span::new(
            format!("{}{}", " ".repeat(pad), "█".repeat(drawn.min(width))),
            Role::Primary,
        )]);
        previous = Some(*amount);
    }
    if steps.len() > MAX_ITEMS {
        lines.push(vec![Span::new(
            format!("Showing {MAX_ITEMS} of {} steps", steps.len()),
            Role::Muted,
        )]);
    }
    Some(lines)
}

/// A node graph as each source and the targets it calls, busiest first, with calls, errors and mean duration.
pub(super) fn node_graph(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let fields = fields(frame, panel);
    let options = &panel["nodeGraph"];
    let source = named(&fields, &options["source"], "source")?;
    let target = named(&fields, &options["target"], "target")?;
    let count = named(&fields, &options["value"], "count");
    let errors = named(&fields, &options["errors"], "errors");
    let duration = named(&fields, &options["duration"], "duration");
    let at = |field: Option<&Value>, row: usize| {
        field
            .and_then(|field| values(field).get(row))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let mut by_source: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut calls: BTreeMap<String, f64> = BTreeMap::new();
    for row in 0..values(source).len().min(200) {
        let from = text_of(values(source).get(row));
        *calls.entry(from.clone()).or_default() += number(&at(count, row)).unwrap_or(1.0);
        by_source.entry(from).or_default().push(row);
    }
    if by_source.is_empty() {
        return None;
    }
    let mut order: Vec<_> = by_source.into_iter().collect();
    order.sort_by(|a, b| calls[&b.0].total_cmp(&calls[&a.0]).then_with(|| a.0.cmp(&b.0)));
    let mut lines = Vec::new();
    for (from, rows) in order {
        lines.push(vec![Span::new(fit(&from, width), Role::Text).bold()]);
        for row in rows {
            let failed = number(&at(errors, row)).unwrap_or(0.0);
            let mut text = format!(
                "  → {}  {} calls",
                text_of(values(target).get(row)),
                formatted(&at(count, row), &Value::Null)
            );
            if failed > 0.0 {
                text.push_str(&format!(", {failed} errors"));
            }
            if let Some(field) = duration.filter(|_| !at(duration, row).is_null()) {
                text.push_str(&format!(", mean {}", formatted(&at(duration, row), field)));
            }
            lines.push(vec![Span::new(
                fit(&text, width),
                if failed > 0.0 { Role::Negative } else { Role::Text },
            )]);
            if lines.len() >= MAX_ITEMS * 2 {
                lines.push(vec![Span::new("More edges are in the table and exports", Role::Muted)]);
                return Some(lines);
            }
        }
    }
    Some(lines)
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    own: f64,
    total: f64,
}
impl Node {
    fn sum(&mut self) -> f64 {
        self.total = self.own + self.children.values_mut().map(Node::sum).sum::<f64>();
        self.total
    }
}

/// A flame graph as an indented tree of span paths, the largest first, each with a bar of its share of the whole.
/// Paths under 1% are left out.
pub(super) fn flame(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let fields = fields(frame, panel);
    let options = &panel["flame"];
    let path = named(&fields, &options["path"], "path").or_else(|| first_label(&fields))?;
    let value = first_number(&fields, &options["value"])?;
    let separator = options["separator"].as_str().unwrap_or(";");
    let mut root = Node::default();
    for (name, amount) in values(path).iter().zip(values(value)) {
        let Some(amount) = number(amount).filter(|amount| *amount > 0.0) else {
            continue;
        };
        let mut at = &mut root;
        for part in text_of(Some(name)).split(separator).take(64) {
            at = at.children.entry(part.to_owned()).or_default();
        }
        at.own += amount;
    }
    let total = root.sum();
    if total <= 0.0 {
        return None;
    }
    let bar_w = (width / 4).clamp(4, 20);
    let mut lines = Vec::new();
    fn walk(node: &Node, depth: usize, total: f64, bar_w: usize, width: usize, value: &Value, lines: &mut Vec<Line>) {
        let mut children: Vec<_> = node.children.iter().collect();
        children.sort_by(|a, b| b.1.total.total_cmp(&a.1.total).then_with(|| a.0.cmp(b.0)));
        for (name, child) in children {
            let share = child.total / total;
            if share < 0.01 || lines.len() >= MAX_ITEMS {
                continue;
            }
            let drawn = cells(share, bar_w);
            let label = format!(
                "{:>5.1}% {}{name}  {} (self {})",
                share * 100.0,
                "  ".repeat(depth),
                formatted(&Value::from(child.total), value),
                formatted(&Value::from(child.own), value)
            );
            lines.push(vec![
                Span::new(
                    format!("{}{} ", "█".repeat(drawn), " ".repeat(bar_w - drawn)),
                    Role::Warning,
                ),
                Span::new(fit(&label, width.saturating_sub(bar_w + 1)), Role::Text),
            ]);
            walk(child, depth + 1, total, bar_w, width, value, lines);
        }
    }
    walk(&root, 0, total, bar_w, width, value, &mut lines);
    Some(lines)
}

/// Land at 2° cells, from 90°N (first row) to 60°S, 180°W to 180°E (first column the high bit): the country polygons of
/// `libs/naturalearth/countries-110m.json` (Natural Earth 110m, public domain), each cell set when its centre is on land.
const LAND: [&str; 75] = [
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "0000000000007f800ffc0000000000000000000000000",
    "000000000017ff3fffffe000000000000040000000000",
    "00000000061df0ffffff0000f8000000003c000000000",
    "000000030027c3ffffff8000200000000006000000000",
    "0000000038afc007ffff00000000030007ffc001d8000",
    "0000000e88b7b003fffe000000000c003ffffec000000",
    "7fffffffffffffffffffffffffffffffffffffffffffe",
    "00ffffffffffffffffffffffff0fff120000000000020",
    "1fffffffffffffffffffffffff8140020000000000000",
    "cfffffffffffffffffffffffff9400000000000000000",
    "00fffffffff8034078000003e7fffffffffffffffffff",
    "01fdffffffe01e0038000007e7ffffffffffffffff2f0",
    "007807ffffe01e4000000007e3ffffffffffffff82200",
    "001001fffff80fe000000182c7fffffffffffffe00f00",
    "0080007fffff9ff8000003820ffffffffffffffc00e00",
    "0000007fffff9ffc000006cfffffffffffffffffc0800",
    "0000003ffffffffc000000dfffffffffffffffff40000",
    "00000017ffffff460000007fffffffffffffffff40000",
    "0000000fffffffc10000007ffffffffffffffffe40000",
    "0000000fffffff600000007f7f97cffffffffffc00000",
    "0000000ffffffe00000007f19f03cffffffffff8c0000",
    "0000000ffffffc00000003c26f7fe7ffffffffe080000",
    "0000000ffffff000000007c02dffe7fffffff84080000",
    "00000007fffff0000000018740ffe7fffffffe6300000",
    "00000003fffff000000001fe022ffffffffffc4f00000",
    "00000001ffffc000000003ff000ffffffffffc1800000",
    "00000000ffff8000000007ffef7ffffffffffe0000000",
    "000000003fc04000000007ffffffdffffffffe0000000",
    "000000005f80400000001fffffefe7fffffffc0000000",
    "000000002f80000000001fffffe7f41ffffffc0000000",
    "000000000780c00000003ffffff7ff07fffff20000000",
    "000000000786100000007ffffffbfe07fcff000000000",
    "0000000003cc020000003ffffff9fe03f07e800000000",
    "0000000000fc000000003ffffff9f801e07f020000000",
    "00000000000f000000007ffffffde001c01f020000000",
    "000000000003000000003ffffffe8001c01f820000000",
    "0000000000010f0000003fffffff3000c013008000000",
    "000000000000aff000001fffffffe000a012000000000",
    "0000000000001ff800000fffffffe0002000008000000",
    "0000000000001fff00000687ffffc000002c180000000",
    "0000000000001fff80000001ffff80000014380000000",
    "0000000000003fff80000001ffff000000187a0000000",
    "0000000000003fffe0000003fffe0000000c781800000",
    "0000000000003ffffc000001fffc00000006762b80000",
    "0000000000007fffff000000fffc000000020101e0000",
    "0000000000003fffff800000fffc00000001c001f0800",
    "0000000000001fffff000000fffc000000000880d0200",
    "0000000000001ffffe0000007ffc00000000000000000",
    "0000000000000ffffe000000fffc20000000001c40000",
    "0000000000000ffffe000000fffc2000000000fc60040",
    "00000000000003fffc000000fff8e000000001fe60000",
    "00000000000001fffc000000fff0e000000001ffe0000",
    "00000000000001fffc0000007ff0c00000000ffff8080",
    "00000000000001fff00000007ff0c00000001ffff8000",
    "00000000000001ffc00000007fe0800000001ffffc000",
    "00000000000001ffc00000003fc0000000001ffffe000",
    "00000000000003ff800000003fc0000000001ffffe000",
    "00000000000003ff800000001f80000000000ffffe000",
    "00000000000003ff000000001f00000000000f07fc000",
    "00000000000003f8000000000000000000000801f8008",
    "00000000000007fc000000000000000000000000f8004",
    "00000000000007e000000000000000000000000000006",
    "00000000000007a00000000000000000000000003000c",
    "00000000000003c000000000000000000000000010018",
    "000000000000078000000000000000000000000000030",
    "000000000000078000000000000000000000000000000",
    "0000000000000f0000000000000000080000000000000",
    "0000000000000e0000000000000000000000000000000",
    "000000000000070000000000000000000000000000000",
    "000000000000030000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
];

fn land(lon: f64, lat: f64) -> bool {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (column, row) = (
        ((lon + 180.0) / 2.0).floor() as usize,
        ((90.0 - lat) / 2.0).floor() as usize,
    );
    let Some(hex) = LAND.get(row) else {
        return false;
    };
    let column = column.min(179);
    hex.as_bytes()
        .get(column / 4)
        .and_then(|digit| (*digit as char).to_digit(16))
        .is_some_and(|digit| digit & (8 >> (column % 4)) != 0)
}

/// A geomap: braille points (latitude and longitude fields) over the land, then the places by value; a country frame
/// lists the countries by value.
pub(super) fn geomap(frame: &Value, panel: &Value, width: usize) -> Option<Vec<Line>> {
    let fields = fields(frame, panel);
    let options = &panel["geomap"];
    let find = |option: &Value, pattern: &str| {
        let pattern = regex::Regex::new(pattern).ok()?;
        option
            .as_str()
            .and_then(|name| fields.iter().find(|field| field["name"] == name))
            .or_else(|| {
                fields
                    .iter()
                    .find(|field| pattern.is_match(field["name"].as_str().unwrap_or("")))
            })
    };
    let lat = find(
        &options["latitude"],
        r"(?i)^(?:lat|latitude)$|[._](?:lat|latitude)$|Latitude$",
    );
    let lon = find(
        &options["longitude"],
        r"(?i)^(?:lon|lng|long|longitude)$|[._](?:lon|lng|longitude)$|Longitude$",
    );
    let label = first_label(&fields);
    let mut lines = Vec::new();
    let mut places: Vec<(String, f64, String)> = Vec::new();
    if let (Some(lat), Some(lon)) = (lat, lon) {
        let value = options["value"]
            .as_str()
            .and_then(|name| fields.iter().find(|field| field["name"] == name))
            .or_else(|| {
                fields
                    .iter()
                    .find(|field| numeric_field(field) && *field != lat && *field != lon)
            });
        let columns = width.min(120);
        let rows = (columns / 4).max(6);
        let (dots_x, dots_y) = (columns * 2, rows * 4);
        let mut grid = vec![vec![0u8; columns]; rows];
        let dot = |grid: &mut [Vec<u8>], lon: f64, lat: f64| -> (usize, usize) {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let (x, y) = (
                (((lon + 180.0) / 360.0) * dots_x as f64)
                    .floor()
                    .clamp(0.0, (dots_x - 1) as f64) as usize,
                (((90.0 - lat) / 150.0) * dots_y as f64)
                    .floor()
                    .clamp(0.0, (dots_y - 1) as f64) as usize,
            );
            const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
            grid[y / 4][x / 2] |= BITS[y % 4][x % 2];
            (y / 4, x / 2)
        };
        let mut points = vec![vec![false; columns]; rows];
        for row in 0..values(lat).len().min(5000) {
            let (Some(y), Some(x)) = (number(&values(lat)[row]), values(lon).get(row).and_then(number)) else {
                continue;
            };
            if !(-90.0..=90.0).contains(&y) || !(-180.0..=180.0).contains(&x) || y < -60.0 {
                continue;
            }
            let (r, c) = dot(&mut grid, x, y);
            points[r][c] = true;
            let amount = value
                .and_then(|field| values(field).get(row))
                .cloned()
                .unwrap_or(Value::Null);
            places.push((
                label.map_or_else(|| format!("{y:.2}, {x:.2}"), |field| text_of(values(field).get(row))),
                number(&amount).unwrap_or(0.0),
                value.map_or_else(String::new, |field| formatted(&amount, field)),
            ));
        }
        if places.is_empty() {
            return None;
        }
        // Land dots go under the points; a cell with a point shows it in the point colour.
        #[expect(clippy::cast_precision_loss)]
        for (r, line) in grid.iter_mut().enumerate() {
            for (c, cell) in line.iter_mut().enumerate() {
                if points[r][c] {
                    continue;
                }
                let lon = -180.0 + (c as f64 + 0.5) * 360.0 / columns as f64;
                let lat = 90.0 - (r as f64 + 0.5) * 150.0 / rows as f64;
                if land(lon, lat) {
                    *cell = 0x24;
                }
            }
        }
        for (r, line) in grid.iter().enumerate() {
            lines.push(
                line.iter()
                    .enumerate()
                    .map(|(c, bits)| {
                        let glyph = char::from_u32(0x2800 + u32::from(*bits)).unwrap_or(' ');
                        Span::new(
                            glyph.to_string(),
                            if points[r][c] { Role::Negative } else { Role::Muted },
                        )
                    })
                    .collect(),
            );
        }
    } else {
        let country = find(&options["country"], r"(?i)country|geo(?:Id)?$")?;
        let value = first_number(&fields, &options["value"]);
        let mut totals: BTreeMap<String, f64> = BTreeMap::new();
        for (row, code) in values(country).iter().enumerate() {
            let amount = value
                .and_then(|field| values(field).get(row))
                .and_then(number)
                .unwrap_or(1.0);
            *totals.entry(text_of(Some(code))).or_default() += amount;
        }
        places = totals
            .into_iter()
            .map(|(code, amount)| {
                let text = value.map_or_else(|| amount.to_string(), |field| formatted(&Value::from(amount), field));
                (code, amount, text)
            })
            .collect();
        if places.is_empty() {
            return None;
        }
    }
    places.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let max = places.first().map_or(0.0, |place| place.1);
    let label_w = places
        .iter()
        .take(10)
        .map(|place| display_width(&place.0))
        .max()
        .unwrap_or(4)
        .clamp(4, width / 3);
    let bar_w = width.saturating_sub(label_w + 20).min(30);
    for (name, amount, text) in places.iter().take(10) {
        let drawn = if max > 0.0 { cells(amount / max, bar_w) } else { 0 };
        lines.push(vec![
            Span::new(fit(name, label_w), Role::Text),
            Span::new(" │", Role::Border),
            Span::new("█".repeat(drawn), Role::Primary),
            Span::new(format!(" {text}"), Role::Text),
        ]);
    }
    if places.len() > 10 {
        lines.push(vec![Span::new(
            format!("Top 10 of {} places", places.len()),
            Role::Muted,
        )]);
    }
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

    #[test]
    fn funnel_steps_show_shares_of_first_and_previous() {
        let frame = json!({"fields":[
            {"name":"step","type":"string","values":["Visited","Carted","Bought"]},
            {"name":"visitors","type":"number","values":[200,50,10]}
        ]});
        let shown = text(&funnel(&frame, &json!({}), 60).unwrap());
        assert!(
            shown.contains("Carted  50 · 25.0% of first · 25.0% of previous"),
            "{shown}"
        );
        assert!(
            shown.contains("Bought  10 · 5.0% of first · 20.0% of previous"),
            "{shown}"
        );
    }

    #[test]
    fn node_graph_lists_each_source_with_its_calls() {
        let frame = json!({"fields":[
            {"name":"source","type":"string","values":["web","web","orders"]},
            {"name":"target","type":"string","values":["orders","db:postgresql","db:postgresql"]},
            {"name":"count","type":"number","values":[10,4,30]},
            {"name":"errors","type":"number","values":[2,0,0]},
            {"name":"duration","type":"number","unit":"ns","values":[2000000,1000000,500000]}
        ]});
        let shown = text(&node_graph(&frame, &json!({}), 80).unwrap());
        assert!(shown.starts_with("orders"), "the busiest source first: {shown}");
        assert!(shown.contains("→ orders  10 calls, 2 errors, mean 2"), "{shown}");
    }

    #[test]
    fn flame_tree_orders_paths_by_share() {
        let frame = json!({"fields":[
            {"name":"path","type":"string","values":["GET /","GET /;find","GET /;find;SELECT","GET /;render"]},
            {"name":"value","type":"number","unit":"ms","values":[2,1,5,2]}
        ]});
        let lines = flame(&frame, &json!({}), 80).unwrap();
        let shown = text(&lines);
        assert!(line_text(&lines[0]).contains("100.0% GET /"), "{shown}");
        assert!(line_text(&lines[1]).contains("60.0%   find"), "{shown}");
        assert!(line_text(&lines[2]).contains("50.0%     SELECT"), "{shown}");
    }

    #[test]
    fn geomap_draws_points_over_land_and_lists_places() {
        assert!(land(2.35, 48.85), "Paris is on land");
        assert!(!land(-30.0, 30.0), "the mid Atlantic is not");
        let frame = json!({"fields":[
            {"name":"city","type":"string","values":["Paris","Sydney"]},
            {"name":"lat","type":"number","values":[48.85,-33.87]},
            {"name":"lon","type":"number","values":[2.35,151.21]},
            {"name":"orders","type":"number","values":[5,9]}
        ]});
        let lines = geomap(&frame, &json!({}), 60).unwrap();
        let shown = text(&lines);
        assert!(
            lines.iter().flatten().any(|span| span.role == Role::Negative),
            "the points are drawn"
        );
        assert!(shown.find("Sydney").unwrap() < shown.find("Paris").unwrap(), "{shown}");
        let countries = json!({"fields":[
            {"name":"countryGeoId","type":"string","values":["USA","FRA","USA"]},
            {"name":"sales","type":"number","values":[1,2,3]}
        ]});
        let shown = text(&geomap(&countries, &json!({}), 60).unwrap());
        assert!(shown.find("USA").unwrap() < shown.find("FRA").unwrap(), "{shown}");
    }
}
