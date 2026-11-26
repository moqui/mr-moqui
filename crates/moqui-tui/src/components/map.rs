/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! `map`: the terminal form of the `map-view` element (plans/Maps.md §3.2): an overview of the places as braille points
//! over the land mask, then the places as a list, each its label, latitude and longitude, precision and source, and the
//! map data's attribution. With `pick` a point is chosen by typing its latitude and longitude (or Enter on a place);
//! the choice is shown, marked on the overview and copied, and saves nothing by itself.
//!
//! Props, as ScreenTui.js writes them from the element (ScreenSupport's mapViewExpr; WGS84 degrees, longitude first):
//! `points` (`[{id, lon, lat, label, precision, source}]`), `center` (`[lon, lat]`), `extent` (`[west, south, east,
//! north]`), `pick`, `attribution`; the same keys in what `source` answers, when it is given (Maps › Browse's
//! `terminalMap`), with `notes`: texts shown above the places (a profile's notice, a layer that is not shown),
//! `overlays` (`[{label, details}]`: the user's overlays, own and shared, listed after the places) and `postalSearch`
//! (`{query, count, reason}`: the postal search the places start with). With a source, `s` searches a postal code: the
//! text typed (a code, with its country before or after it: `US 02134`) is the source's `postal` parameter, and its
//! approximate places, from the local Geo data, come first.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{list, prop, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;

/// The widest overview drawn: 72 columns are 18 rows of braille.
const OVERVIEW_WIDTH: usize = 72;

pub struct Map;

/// A place of the props, checked: (longitude, latitude, label, details).
struct Place {
    lon: f64,
    lat: f64,
    label: String,
    details: String,
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) => text.trim().parse().ok(),
        _ => None,
    }
    .filter(|number: &f64| number.is_finite())
}

fn in_range(lon: f64, lat: f64) -> bool {
    (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)
}

fn places(props: &Value, state: &ComponentState) -> Vec<Place> {
    list(props, state, "points")
        .into_iter()
        .filter_map(|point| {
            let lon = number(point.get("lon").or_else(|| point.get("longitude")))?;
            let lat = number(point.get("lat").or_else(|| point.get("latitude")))?;
            in_range(lon, lat).then(|| Place {
                lon,
                lat,
                label: text_of(point.get("label")),
                details: [text_of(point.get("precision")), text_of(point.get("source"))]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
            })
        })
        .collect()
}

/// Latitude, longitude, as people read them, to five decimals (about a meter).
fn coordinates(lon: f64, lat: f64) -> String {
    format!("{lat:.5}, {lon:.5}")
}

/// A typed position, latitude first ("41.88, -87.63" or "41.88 -87.63"), as (longitude, latitude); None unless both are
/// numbers in range.
pub(crate) fn parse_lat_lon(text: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = text
        .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == ';')
        .filter(|part| !part.is_empty())
        .collect();
    let [lat, lon] = parts.as_slice() else {
        return None;
    };
    let (lat, lon) = (lat.parse::<f64>().ok()?, lon.parse::<f64>().ok()?);
    (lat.is_finite() && lon.is_finite() && in_range(lon, lat)).then_some((lon, lat))
}

/// The pair of numbers under `key` of the data, else of the props.
fn pair(props: &Value, state: &ComponentState, key: &str) -> Option<Vec<f64>> {
    let from = |value: &Value| value.get(key).and_then(Value::as_array).cloned();
    let values = state.data.as_ref().and_then(from).or_else(|| from(props))?;
    values.iter().map(|value| number(Some(value))).collect()
}

fn picking(props: &Value, state: &ComponentState) -> bool {
    props.get("pick").and_then(Value::as_bool).unwrap_or(false)
        || state
            .data
            .as_ref()
            .and_then(|data| data.get("pick"))
            .and_then(Value::as_bool)
            == Some(true)
}

fn chosen(state: &ComponentState) -> Option<(f64, f64)> {
    let values = state.local.get("chosen")?.as_array()?;
    Some((number(values.first())?, number(values.get(1))?))
}

fn typing(state: &ComponentState) -> bool {
    state.local.get("typing").and_then(Value::as_bool) == Some(true)
}

/// Typing a postal search (a map with a source).
fn searching(state: &ComponentState) -> bool {
    state.local.get("searching").and_then(Value::as_bool) == Some(true)
}

fn has_source(props: &Value) -> bool {
    !prop(props, "source", "").is_empty()
}

/// The longest postal search sent: a code and its country.
const POSTAL_MAX: usize = 40;

/// A character a postal search takes: a code's letters, digits, spaces and hyphens, and a comma or colon before a country.
fn postal_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | ',' | ':')
}

/// Choose a point: kept, shown and copied.
fn choose(state: &mut ComponentState, lon: f64, lat: f64) -> Vec<Act> {
    state.local["chosen"] = json!([lon, lat]);
    state.local["typing"] = json!(false);
    state.local["error"] = json!("");
    let text = coordinates(lon, lat);
    vec![
        Act::Copy(text.clone()),
        Act::Message(format!("Chosen: {text} (copied)")),
    ]
}

impl Component for Map {
    fn name(&self) -> &'static str {
        "map"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let width = usize::from(width).max(20);
        let places = places(props, state);
        let picked = chosen(state);
        let mut lines = Vec::new();
        // the overview spans 90°N to 60°S: a place further south is listed, not drawn on its edge
        let drawn = |point: &(f64, f64)| point.1 >= super::world::SOUTH_EDGE;
        let points: Vec<(f64, f64)> = places
            .iter()
            .map(|place| (place.lon, place.lat))
            .filter(drawn)
            .collect();
        let mut marked: Vec<(f64, f64)> = places
            .get(state.at)
            .map(|place| (place.lon, place.lat))
            .into_iter()
            .collect();
        marked.extend(picked);
        marked.retain(drawn);
        if !points.is_empty() || !marked.is_empty() {
            lines.extend(super::world::braille_map(&points, &marked, width.min(OVERVIEW_WIDTH)));
        }
        for note in list(props, state, "notes").into_iter().filter_map(Value::as_str) {
            lines.push(vec![Span::new(note.to_string(), Role::Warning)]);
        }
        if let Some(search) = state
            .data
            .as_ref()
            .and_then(|data| data.get("postalSearch"))
            .filter(|search| search.is_object())
        {
            let query = text_of(search.get("query"));
            let count = search.get("count").and_then(Value::as_u64).unwrap_or(0);
            let found = match count {
                0 => "nothing found".to_string(),
                1 => "1 approximate place, listed first".to_string(),
                n => format!("{n} approximate places, listed first"),
            };
            lines.push(vec![Span::new(format!("Postal search {query}: {found}"), Role::Info)]);
            let reason = text_of(search.get("reason"));
            if !reason.is_empty() {
                lines.push(vec![Span::new(reason, Role::Warning)]);
            }
        }
        if places.is_empty() {
            lines.push(vec![Span::new("No places", Role::Muted)]);
            if let Some(center) = pair(props, state, "center").filter(|values| values.len() == 2) {
                lines.push(vec![Span::new(
                    format!("Center: {}", coordinates(center[0], center[1])),
                    Role::Text,
                )]);
            }
            if let Some(extent) = pair(props, state, "extent").filter(|values| values.len() == 4) {
                let area = format!(
                    "Area: {} – {}",
                    coordinates(extent[0], extent[1]),
                    coordinates(extent[2], extent[3])
                );
                lines.push(vec![Span::new(area, Role::Text)]);
            }
        } else {
            lines.push(vec![Span::new("Places", Role::Text).bold()]);
        }
        for (at, place) in places.iter().enumerate() {
            let selected = at == state.at;
            let label = if place.label.is_empty() { "Place" } else { &place.label };
            let mut name = Span::new(label.to_string(), Role::Text).bold();
            if selected {
                name = name.reverse();
            }
            let mut line = vec![
                Span::new(if selected { "▸ " } else { "  " }, Role::Primary),
                name,
                Span::new(format!("  {}", coordinates(place.lon, place.lat)), Role::Text),
            ];
            if !place.details.is_empty() {
                line.push(Span::new(format!("  {}", place.details), Role::Muted));
            }
            lines.push(line);
        }
        let overlays: Vec<&Value> = list(props, state, "overlays")
            .into_iter()
            .filter(|entry| entry.is_object())
            .collect();
        if !overlays.is_empty() {
            lines.push(vec![Span::new("Overlays", Role::Text).bold()]);
            for overlay in overlays {
                let label = text_of(overlay.get("label"));
                let mut line = vec![Span::new(
                    format!("  {}", if label.is_empty() { "Overlay" } else { &label }),
                    Role::Text,
                )];
                let details = text_of(overlay.get("details"));
                if !details.is_empty() {
                    line.push(Span::new(format!("  {details}"), Role::Muted));
                }
                lines.push(line);
            }
        }
        if has_source(props) {
            if searching(state) {
                let typed = state.local.get("postal").and_then(Value::as_str).unwrap_or("");
                lines.push(vec![
                    Span::new("Postal code (and its country, such as US 02134): ", Role::Text),
                    Span::new(format!("{typed}▏"), Role::Field),
                ]);
            } else {
                lines.push(vec![Span::new(
                    "s: find a postal code's approximate place (the local Geo data)",
                    Role::Muted,
                )]);
            }
        }
        if picking(props, state) {
            if typing(state) {
                let typed = state.local.get("typed").and_then(Value::as_str).unwrap_or("");
                lines.push(vec![
                    Span::new("Latitude, longitude: ", Role::Text),
                    Span::new(format!("{typed}▏"), Role::Field),
                ]);
            } else {
                lines.push(vec![Span::new(
                    "p: choose a point by its latitude and longitude; Enter: choose the place",
                    Role::Muted,
                )]);
            }
            let error = state.local.get("error").and_then(Value::as_str).unwrap_or("");
            if !error.is_empty() {
                lines.push(vec![Span::new(error.to_string(), Role::Negative)]);
            }
            if let Some((lon, lat)) = picked {
                lines.push(vec![Span::new(
                    format!("Chosen: {}", coordinates(lon, lat)),
                    Role::Positive,
                )]);
            }
        }
        let attribution = state
            .data
            .as_ref()
            .and_then(|data| data.get("attribution"))
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| prop(props, "attribution", ""));
        if !attribution.is_empty() {
            lines.push(vec![Span::new(format!("Map data: {attribution}"), Role::Muted)]);
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        // a Ctrl or Alt chord is the screen's, never a character typed or a key of the map
        if !press.is_plain() {
            return None;
        }
        if searching(state) {
            let mut typed = state
                .local
                .get("postal")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match press.key {
                Key::Char(ch) if postal_char(ch) && typed.chars().count() < POSTAL_MAX => typed.push(ch),
                Key::Char(_) => {}
                Key::Backspace => {
                    typed.pop();
                }
                Key::Escape => {
                    state.local["searching"] = json!(false);
                    return Some(Vec::new());
                }
                Key::Enter => {
                    state.local["searching"] = json!(false);
                    let query = typed.trim().to_string();
                    // an empty search takes the postal places away; the source is asked again either way
                    if query.is_empty() {
                        state.parameters.remove("postal");
                    } else {
                        state.parameters.insert("postal".to_string(), query);
                    }
                    state.at = 0;
                    return Some(vec![Act::Reload]);
                }
                _ => return None,
            }
            state.local["postal"] = json!(typed);
            return Some(Vec::new());
        }
        if typing(state) {
            let mut typed = state
                .local
                .get("typed")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            match press.key {
                Key::Char(ch) if ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | ',' | ' ' | ';') => {
                    typed.push(ch)
                }
                Key::Char(_) => {}
                Key::Backspace => {
                    typed.pop();
                }
                Key::Escape => {
                    state.local["typing"] = json!(false);
                    state.local["error"] = json!("");
                    return Some(Vec::new());
                }
                Key::Enter => {
                    return Some(match parse_lat_lon(&typed) {
                        Some((lon, lat)) => choose(state, lon, lat),
                        None => {
                            state.local["error"] = json!("Enter a latitude and a longitude, such as 41.8781, -87.6298");
                            Vec::new()
                        }
                    });
                }
                _ => return None,
            }
            state.local["typed"] = json!(typed);
            return Some(Vec::new());
        }
        let places = places(props, state);
        match press.key {
            Key::Down if state.at + 1 < places.len() => state.at += 1,
            Key::Up if state.at > 0 => state.at -= 1,
            Key::Char('c') => {
                let place = places.get(state.at)?;
                let text = coordinates(place.lon, place.lat);
                return Some(vec![Act::Copy(text.clone()), Act::Message(format!("Copied {text}"))]);
            }
            Key::Char('s') if has_source(props) => {
                state.local["searching"] = json!(true);
                let last = state.parameters.get("postal").cloned().unwrap_or_default();
                state.local["postal"] = json!(last);
            }
            Key::Char('p') if picking(props, state) => {
                state.local["typing"] = json!(true);
                state.local["typed"] = json!("");
                state.local["error"] = json!("");
            }
            Key::Enter if picking(props, state) => {
                let place = places.get(state.at)?;
                return Some(choose(state, place.lon, place.lat));
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn paste(&self, _props: &Value, state: &mut ComponentState, text: &str) -> Option<Vec<Act>> {
        if searching(state) {
            let mut typed = state
                .local
                .get("postal")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            typed.extend(text.chars().filter(|ch| postal_char(*ch)));
            let typed: String = typed.chars().take(POSTAL_MAX).collect();
            state.local["postal"] = json!(typed);
            return Some(Vec::new());
        }
        if !typing(state) {
            return None;
        }
        let mut typed = state
            .local
            .get("typed")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        typed.extend(
            text.chars()
                .filter(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | ',' | ' ' | ';')),
        );
        state.local["typed"] = json!(typed);
        Some(Vec::new())
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Up/Down", "Previous / next place"),
            ("c", "Copy the place's latitude, longitude"),
            (
                "s",
                "Find a postal code's approximate place: the code, with its country (Maps › Browse)",
            ),
            (
                "p",
                "Choose a point by its latitude and longitude (a map that allows a choice)",
            ),
            ("Enter", "Choose the place, or the point typed"),
            ("Esc", "Stop typing the point or the postal code"),
        ]
    }
}

#[cfg(test)]
#[path = "map_tests.rs"]
mod tests;
