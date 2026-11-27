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

/// Where a place a geomap lists opens in Maps (`report.mapsUrl` in WebrootReport.js): a point, or a country's extent
/// `[west, south, east, north]` (`world::country_extent`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MapsPlace {
    Point { lon: f64, lat: f64 },
    Extent([f64; 4]),
}

/// The most places a geomap lists.
const LISTED: usize = 10;

/// A place a geomap lists: its name, its amount and the amount as shown, and where it opens in Maps (None for a country
/// code with no known extent).
struct Place {
    name: String,
    amount: f64,
    text: String,
    target: Option<MapsPlace>,
}

/// A geomap's places by value, the largest first: its points (latitude and longitude fields), or its countries.
fn places(frame: &Value, panel: &Value) -> Option<Vec<Place>> {
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
    let mut places: Vec<Place> = Vec::new();
    if let (Some(lat), Some(lon)) = (lat, lon) {
        let value = options["value"]
            .as_str()
            .and_then(|name| fields.iter().find(|field| field["name"] == name))
            .or_else(|| {
                fields
                    .iter()
                    .find(|field| numeric_field(field) && *field != lat && *field != lon)
            });
        for row in 0..values(lat).len().min(5000) {
            let (Some(y), Some(x)) = (number(&values(lat)[row]), values(lon).get(row).and_then(number)) else {
                continue;
            };
            if !(-90.0..=90.0).contains(&y) || !(-180.0..=180.0).contains(&x) || y < -60.0 {
                continue;
            }
            let amount = value
                .and_then(|field| values(field).get(row))
                .cloned()
                .unwrap_or(Value::Null);
            places.push(Place {
                name: label.map_or_else(|| format!("{y:.2}, {x:.2}"), |field| text_of(values(field).get(row))),
                amount: number(&amount).unwrap_or(0.0),
                text: value.map_or_else(String::new, |field| formatted(&amount, field)),
                target: Some(MapsPlace::Point { lon: x, lat: y }),
            });
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
            .map(|(code, amount)| Place {
                text: value.map_or_else(|| amount.to_string(), |field| formatted(&Value::from(amount), field)),
                target: crate::components::world::country_extent(&code).map(MapsPlace::Extent),
                name: code,
                amount,
            })
            .collect();
    }
    if places.is_empty() {
        return None;
    }
    places.sort_by(|a, b| b.amount.total_cmp(&a.amount).then_with(|| a.name.cmp(&b.name)));
    Some(places)
}

/// Where each place a geomap lists opens in Maps, in the order it lists them; empty when it has nothing to draw.
pub(super) fn geomap_targets(frame: &Value, panel: &Value) -> Vec<Option<MapsPlace>> {
    places(frame, panel).map_or_else(Vec::new, |places| {
        places.iter().take(LISTED).map(|place| place.target).collect()
    })
}

/// The path and query of Maps' Browse screen (`maps`) at a place, as `report.mapsUrl` builds them: `extent=w,s,e,n` (the
/// commas encoded, as URLSearchParams writes them) or `longitude` and `latitude`, each number as JavaScript writes it.
pub(super) fn maps_url(maps: &str, place: MapsPlace) -> String {
    fn js(number: f64) -> String {
        if number == 0.0 {
            "0".into()
        } else if number.abs() < 1e-6 {
            format!("{number:e}")
        } else {
            number.to_string()
        }
    }
    match place {
        MapsPlace::Extent(extent) => format!("{maps}?extent={}", extent.map(js).join("%2C")),
        MapsPlace::Point { lon, lat } => format!("{maps}?longitude={}&latitude={}", js(lon), js(lat)),
    }
}

/// A geomap: braille points (latitude and longitude fields) over the land, then the places by value; a country frame
/// lists the countries by value. `selected` (where the dashboard or panel offers Maps and the panel has the keys)
/// highlights that listed place and says how to open it.
pub(super) fn geomap(frame: &Value, panel: &Value, width: usize, selected: Option<usize>) -> Option<Vec<Line>> {
    let places = places(frame, panel)?;
    let points: Vec<(f64, f64)> = places
        .iter()
        .filter_map(|place| match place.target {
            Some(MapsPlace::Point { lon, lat }) => Some((lon, lat)),
            _ => None,
        })
        .collect();
    let mut lines = Vec::new();
    if !points.is_empty() {
        lines.extend(crate::components::world::braille_map(&points, &[], width));
    }
    let max = places.first().map_or(0.0, |place| place.amount);
    let label_w = places
        .iter()
        .take(LISTED)
        .map(|place| display_width(&place.name))
        .max()
        .unwrap_or(4)
        .clamp(4, width / 3);
    let bar_w = width.saturating_sub(label_w + 20).min(30);
    let selected = selected.map(|at| at.min(places.len().min(LISTED) - 1));
    for (at, place) in places.iter().take(LISTED).enumerate() {
        let drawn = if max > 0.0 { cells(place.amount / max, bar_w) } else { 0 };
        let name = Span::new(fit(&place.name, label_w), Role::Text);
        lines.push(vec![
            if selected == Some(at) { name.reverse() } else { name },
            Span::new(" │", Role::Border),
            Span::new("█".repeat(drawn), Role::Primary),
            Span::new(format!(" {}", place.text), Role::Text),
        ]);
    }
    if places.len() > LISTED {
        lines.push(vec![Span::new(
            format!("Top {LISTED} of {} places", places.len()),
            Role::Muted,
        )]);
    }
    if selected.is_some() {
        lines.push(vec![Span::new("↑/↓ select a place, Enter: open in Maps", Role::Muted)]);
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
        let frame = json!({"fields":[
            {"name":"city","type":"string","values":["Paris","Sydney"]},
            {"name":"lat","type":"number","values":[48.85,-33.87]},
            {"name":"lon","type":"number","values":[2.35,151.21]},
            {"name":"orders","type":"number","values":[5,9]}
        ]});
        let lines = geomap(&frame, &json!({}), 60, None).unwrap();
        let shown = text(&lines);
        assert!(
            lines.iter().flatten().any(|span| span.role == Role::Negative),
            "the points are drawn"
        );
        assert!(shown.find("Sydney").unwrap() < shown.find("Paris").unwrap(), "{shown}");
        // with no Maps offered nothing is selected and no hint is shown
        assert!(!lines.iter().flatten().any(|span| span.reverse), "{shown}");
        assert!(!shown.contains("open in Maps"), "{shown}");
        // each listed place keeps where it opens in Maps, in the order listed
        assert_eq!(
            geomap_targets(&frame, &json!({})),
            [
                Some(MapsPlace::Point {
                    lon: 151.21,
                    lat: -33.87
                }),
                Some(MapsPlace::Point { lon: 2.35, lat: 48.85 })
            ]
        );
        // a selected place (where Maps is offered) is highlighted, with the hint
        let lines = geomap(&frame, &json!({}), 60, Some(1)).unwrap();
        let selected: Vec<&Span> = lines.iter().flatten().filter(|span| span.reverse).collect();
        assert!(
            matches!(&selected[..], [span] if span.text.starts_with("Paris")),
            "{selected:?}"
        );
        assert!(text(&lines).ends_with("↑/↓ select a place, Enter: open in Maps"));
        let countries = json!({"fields":[
            {"name":"countryGeoId","type":"string","values":["USA","FRA","USA","XYZ"]},
            {"name":"sales","type":"number","values":[1,2,3,0]}
        ]});
        let shown = text(&geomap(&countries, &json!({}), 60, None).unwrap());
        assert!(shown.find("USA").unwrap() < shown.find("FRA").unwrap(), "{shown}");
        // a country opens at its extent; a code with none has no target
        let targets = geomap_targets(&countries, &json!({}));
        assert_eq!(targets[1], Some(MapsPlace::Extent([-4.5918, 42.3433, 8.0983, 51.148])));
        assert_eq!(targets[2], None);
        assert_eq!(
            maps_url("/apps/maps/Browse", targets[1].unwrap()),
            "/apps/maps/Browse?extent=-4.5918%2C42.3433%2C8.0983%2C51.148"
        );
        assert_eq!(
            maps_url("/apps/maps/Browse", MapsPlace::Point { lon: 2.35, lat: 48.85 }),
            "/apps/maps/Browse?longitude=2.35&latitude=48.85"
        );
        assert_eq!(
            maps_url("/apps/maps/Browse", MapsPlace::Extent([-180.0, -0.0, 180.0, 1e-7])),
            "/apps/maps/Browse?extent=-180%2C0%2C180%2C1e-7",
            "numbers as JavaScript writes them"
        );
    }
}
