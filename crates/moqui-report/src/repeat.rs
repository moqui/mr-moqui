//! Repeating panels and rows: a panel or a row/tab item with `repeat: {variable, direction, maxPerRow}` is copied once
//! for each value of the variable, each copy with the variable pinned to its value. The dashboard is expanded here,
//! before its queries run, so the browser, the TUI and the print document get a plain layout and panel map.
//!
//! The first copy keeps the panel's id, so links to it stay good; the others are `<id>~<n>` (`<id>~<row>-<n>` inside a
//! repeated row). Each copy carries `repeatOf` (the source panel's id) and `repeatValues` (its pinned values).
use serde_json::{json, Map, Value as Json};

/// The most copies one repeat makes; past this the rest are left out with a notice.
pub const REPEAT_LIMIT: usize = 24;
/// The most panels an expanded dashboard has, as for a stored one.
pub const PANEL_LIMIT: usize = 100;
/// The default number of copies on one grid row of a horizontal repeat.
const MAX_PER_ROW: usize = 4;

pub struct Expanded {
    pub panels: Map<String, Json>,
    pub layout: Json,
    pub notices: Vec<Json>,
}

struct Expander<'a> {
    source: &'a Map<String, Json>,
    panels: Map<String, Json>,
    notices: Vec<Json>,
    values_of: &'a mut dyn FnMut(&str) -> Vec<Json>,
}

/// The variable a panel or layout item repeats for; an empty name is no repeat.
fn variable_of(item: &Json) -> Option<&str> {
    item["repeat"]["variable"].as_str().filter(|name| !name.is_empty())
}

/// Whether a body has anything to repeat.
pub fn has_repeats(body: &Json) -> bool {
    fn layout(node: &Json) -> bool {
        node["items"]
            .as_array()
            .or_else(|| node["children"].as_array())
            .into_iter()
            .flatten()
            .any(|item| variable_of(item).is_some() || layout(item) || layout(&item["layout"]))
    }
    body["panels"]
        .as_object()
        .is_some_and(|panels| panels.values().any(|panel| variable_of(panel).is_some()))
        || layout(&body["layout"])
}

/// The panels and layout of a body with each repeat expanded. `values_of` gives a variable's values to repeat over (its
/// selection, or all its options for All). A body without a layout is laid out as the auto grid of its panels.
pub fn expand(body: &Json, values_of: &mut dyn FnMut(&str) -> Vec<Json>) -> Expanded {
    let source = body["panels"].as_object().cloned().unwrap_or_default();
    let layout = if body["layout"].is_object() {
        body["layout"].clone()
    } else {
        json!({"type":"autoGrid","items":source.keys().map(|id|json!({"panelId":id})).collect::<Vec<_>>()})
    };
    let mut expander = Expander {
        source: &source,
        panels: Map::new(),
        notices: Vec::new(),
        values_of,
    };
    let layout = expander.node(&layout, &Map::new(), &[], 0);
    // Panels the layout does not place stay in the map as they were, as they would without repeats.
    for (id, panel) in &source {
        if !expander.panels.contains_key(id) && variable_of(panel).is_none() {
            expander.panels.insert(id.clone(), panel.clone());
        }
    }
    Expanded {
        panels: expander.panels,
        layout,
        notices: expander.notices,
    }
}

/// A value as text, for a title.
fn value_text(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// `${name}` and `$name` in a text replaced by pinned values; other names stay as written.
fn substitute(text: &str, pins: &Map<String, Json>) -> String {
    let mut out = text.to_owned();
    for (name, value) in pins {
        out = out.replace(&format!("${{{name}}}"), &value_text(value));
        let bare = format!("${name}");
        let replacement = value_text(value);
        let mut replaced = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some((before, after)) = rest.split_once(bare.as_str()) {
            replaced.push_str(before);
            // `$name` followed by more of a name is another name
            let continues = after.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
            replaced.push_str(if continues { &bare } else { &replacement });
            rest = after;
        }
        replaced.push_str(rest);
        out = replaced;
    }
    out
}

impl Expander<'_> {
    fn values(&mut self, item: &Json, owner: &str) -> Vec<Json> {
        let Some(variable) = variable_of(item) else {
            return Vec::new();
        };
        let mut values = (self.values_of)(variable);
        if values.len() > REPEAT_LIMIT {
            tracing::warn!(%owner, %variable, count = values.len(), "dashboard repeat exceeds its limit; keeping the first {REPEAT_LIMIT}");
            self.notices.push(json!({"level":"warning","code":"repeat_limit","variable":variable,"message":format!("{owner} repeats for the first {REPEAT_LIMIT} of {} values of {variable}",values.len())}));
            values.truncate(REPEAT_LIMIT);
        }
        values
    }

    /// The copies of a panel: (id, pins), one for each value of its own repeat under the inherited pins.
    fn panel_copies(&mut self, id: &str, pins: &Map<String, Json>, suffix: &[usize]) -> Vec<String> {
        let Some(panel) = self.source.get(id) else {
            // A layout leaf naming no panel is left for the renderers to show as unavailable.
            return vec![id.to_owned()];
        };
        let repeats = variable_of(panel);
        let own: Vec<Option<(String, Json)>> = match repeats {
            Some(variable) => {
                let values = self.values(panel, id);
                if values.is_empty() {
                    vec![None]
                } else {
                    values
                        .into_iter()
                        .map(|value| Some((variable.to_owned(), value)))
                        .collect()
                }
            }
            None => vec![None],
        };
        let mut ids = Vec::new();
        for (index, pin) in own.into_iter().enumerate() {
            let mut parts = suffix.to_vec();
            if repeats.is_some() {
                parts.push(index);
            }
            let copy_id = if parts.iter().all(|part| *part == 0) {
                id.to_owned()
            } else {
                format!(
                    "{id}~{}",
                    parts.iter().map(usize::to_string).collect::<Vec<_>>().join("-")
                )
            };
            if self.panels.contains_key(&copy_id) {
                ids.push(copy_id);
                continue;
            }
            if self.panels.len() >= PANEL_LIMIT {
                tracing::warn!(panel = %id, "dashboard repeats exceed {PANEL_LIMIT} panels; leaving out the rest");
                if !self.notices.iter().any(|notice| notice["code"] == "panel_limit") {
                    self.notices.push(json!({"level":"warning","code":"panel_limit","message":format!("Repeats stop at {PANEL_LIMIT} panels; narrow the variables to see the rest")}));
                }
                break;
            }
            let mut values = pins.clone();
            if let Some((name, value)) = pin {
                values.insert(name, value);
            }
            let mut copy = panel.clone();
            if let Some(map) = copy.as_object_mut() {
                map.remove("repeat");
                if !values.is_empty() {
                    if let Some(title) = map.get("title").and_then(Json::as_str) {
                        let title = substitute(title, &values);
                        map.insert("title".into(), json!(title));
                    }
                    map.insert("repeatOf".into(), json!(id));
                    map.insert("repeatValues".into(), Json::Object(values));
                }
            }
            self.panels.insert(copy_id.clone(), copy);
            ids.push(copy_id);
        }
        ids
    }

    fn node(&mut self, node: &Json, pins: &Map<String, Json>, suffix: &[usize], depth: usize) -> Json {
        if depth > 16 {
            return node.clone();
        }
        if let Some(id) = node.as_str().or_else(|| node["panelId"].as_str()) {
            let ids = self.panel_copies(id, pins, suffix);
            return if ids.len() == 1 && ids[0] == id && node.is_object() {
                node.clone()
            } else if ids.len() == 1 {
                json!({"panelId":ids[0]})
            } else {
                json!({"type":"autoGrid","items":ids.into_iter().map(|id|json!({"panelId":id})).collect::<Vec<_>>()})
            };
        }
        let key = if node["items"].is_array() { "items" } else { "children" };
        let Some(items) = node[key].as_array() else {
            return node.clone();
        };
        let mut out = node.clone();
        let expanded = match node["type"].as_str() {
            Some("grid") => self.grid(items, pins, suffix),
            Some("rows" | "row" | "tabs") => self.sections(items, pins, suffix, depth),
            _ => {
                let mut list = Vec::new();
                for item in items {
                    match item.as_str().or_else(|| item["panelId"].as_str()) {
                        Some(id) => list.extend(
                            self.panel_copies(id, pins, suffix)
                                .into_iter()
                                .map(|id| json!({"panelId":id})),
                        ),
                        None => list.push(self.node(item, pins, suffix, depth + 1)),
                    }
                }
                list
            }
        };
        out[key] = Json::Array(expanded);
        out
    }

    /// Grid items: a repeated panel's copies tile across (`h`, the default) or down (`v`) from its place, and the items
    /// below it move down by the rows the copies added.
    fn grid(&mut self, items: &[Json], pins: &Map<String, Json>, suffix: &[usize]) -> Vec<Json> {
        // Each placed item with the y of the item it came from, which decides the repeats it moves below.
        let mut placed: Vec<(Json, i64)> = Vec::new();
        let mut shifts: Vec<(i64, i64)> = Vec::new();
        for item in items {
            let Some(id) = item["panelId"].as_str() else {
                placed.push((item.clone(), item["y"].as_i64().unwrap_or(0)));
                continue;
            };
            let ids = self.panel_copies(id, pins, suffix);
            let (x, y) = (item["x"].as_i64().unwrap_or(0), item["y"].as_i64().unwrap_or(0));
            let (w, h) = (item["w"].as_i64().unwrap_or(12), item["h"].as_i64().unwrap_or(1).max(1));
            if ids.len() <= 1 {
                let mut item = item.clone();
                if let Some(id) = ids.first() {
                    item["panelId"] = json!(id);
                }
                placed.push((item, y));
                continue;
            }
            let repeat = self
                .source
                .get(id)
                .map(|panel| panel["repeat"].clone())
                .unwrap_or(Json::Null);
            let count = i64::try_from(ids.len()).unwrap_or(i64::MAX);
            let rows = if repeat["direction"] == "v" {
                for (at, id) in ids.iter().enumerate() {
                    let at = i64::try_from(at).unwrap_or(0);
                    placed.push((json!({"panelId":id,"x":x,"y":y+at*h,"w":w,"h":h}), y));
                }
                count
            } else {
                let per = repeat["maxPerRow"]
                    .as_i64()
                    .unwrap_or(MAX_PER_ROW as i64)
                    .clamp(1, 24)
                    .min(count);
                let width = (24 / per).max(1);
                for (at, id) in ids.iter().enumerate() {
                    let at = i64::try_from(at).unwrap_or(0);
                    placed.push((
                        json!({"panelId":id,"x":(at%per)*width,"y":y+(at/per)*h,"w":width,"h":h}),
                        y,
                    ));
                }
                (count + per - 1) / per
            };
            shifts.push((y, (rows - 1) * h));
        }
        // An item that starts below a repeat moves down by the rows that repeat added.
        placed
            .into_iter()
            .map(|(mut item, origin)| {
                let moved: i64 = shifts.iter().filter(|(y, _)| origin > *y).map(|(_, rows)| rows).sum();
                if moved > 0 {
                    item["y"] = json!(item["y"].as_i64().unwrap_or(0) + moved);
                }
                item
            })
            .collect()
    }

    /// Rows and tabs: a repeated item is copied for each value, its title and every panel in it pinned to the value.
    fn sections(&mut self, items: &[Json], pins: &Map<String, Json>, suffix: &[usize], depth: usize) -> Vec<Json> {
        let mut out = Vec::new();
        for item in items {
            let inner = |item: &Json| {
                item.get("layout")
                    .filter(|layout| layout.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"autoGrid","items":item["items"].clone()}))
            };
            let Some(variable) = variable_of(item) else {
                let mut item = item.clone();
                let layout = self.node(&inner(&item), pins, suffix, depth + 1);
                if let Some(map) = item.as_object_mut() {
                    map.remove("items");
                    map.insert("layout".into(), layout);
                }
                out.push(item);
                continue;
            };
            let title = item["title"].as_str().unwrap_or("").to_owned();
            let mut values = self.values(item, if title.is_empty() { "A row" } else { &title });
            if values.is_empty() {
                values.push(Json::Null);
            }
            for (index, value) in values.into_iter().enumerate() {
                let mut row_pins = pins.clone();
                if !value.is_null() {
                    row_pins.insert(variable.to_owned(), value);
                }
                let mut row_suffix = suffix.to_vec();
                row_suffix.push(index);
                let layout = self.node(&inner(item), &row_pins, &row_suffix, depth + 1);
                let mut copy = item.clone();
                if let Some(map) = copy.as_object_mut() {
                    map.remove("repeat");
                    map.remove("items");
                    map.insert("title".into(), json!(substitute(&title, &row_pins)));
                    map.insert("layout".into(), layout);
                }
                out.push(copy);
            }
        }
        out
    }
}

/// The values a repeat goes over: the selected values, or, for All or no selection, the options.
pub fn selected_values(selected: Option<&Json>, options: impl FnOnce() -> Vec<Json>) -> Vec<Json> {
    let chosen: Vec<Json> = match selected {
        Some(Json::Array(items)) => items.clone(),
        Some(Json::Null) | None => Vec::new(),
        Some(value) => vec![value.clone()],
    };
    if chosen.is_empty() || chosen.iter().any(|value| value == "$__all" || value == "") {
        options()
    } else {
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regions(name: &str) -> Vec<Json> {
        assert_eq!(name, "region");
        vec![json!("east"), json!("west"), json!("north")]
    }

    #[test]
    fn grid_repeats_tile_across_and_push_lower_items_down() {
        let body = json!({"panels":{
            "sales":{"title":"Sales in $region","repeat":{"variable":"region","maxPerRow":2}},
            "notes":{"title":"Notes"}
        },"layout":{"type":"grid","items":[{"panelId":"sales","x":0,"y":0,"w":24,"h":2},{"panelId":"notes","x":0,"y":2,"w":24,"h":1}]}});
        let expanded = expand(&body, &mut regions);
        let items = expanded.layout["items"].as_array().unwrap();
        let at = |id: &str| items.iter().find(|item| item["panelId"] == id).unwrap().clone();
        assert_eq!(at("sales"), json!({"panelId":"sales","x":0,"y":0,"w":12,"h":2}));
        assert_eq!(at("sales~1"), json!({"panelId":"sales~1","x":12,"y":0,"w":12,"h":2}));
        assert_eq!(at("sales~2"), json!({"panelId":"sales~2","x":0,"y":2,"w":12,"h":2}));
        assert_eq!(
            at("notes")["y"],
            json!(4),
            "two rows of copies push the next item down by one row of height 2"
        );
        let copy = &expanded.panels["sales~2"];
        assert_eq!(copy["title"], json!("Sales in north"));
        assert_eq!(copy["repeatOf"], json!("sales"));
        assert_eq!(copy["repeatValues"], json!({"region":"north"}));
        assert!(copy.get("repeat").is_none());
        assert_eq!(expanded.panels["notes"], json!({"title":"Notes"}));
    }

    #[test]
    fn row_repeats_pin_every_panel_and_keep_the_first_ids() {
        let body = json!({"panels":{"orders":{"title":"Orders ${region}x"}},
            "layout":{"type":"rows","items":[{"title":"Region $region","repeat":{"variable":"region"},"layout":{"type":"autoGrid","items":["orders"]}}]}});
        let expanded = expand(&body, &mut regions);
        let rows = expanded.layout["items"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["title"], json!("Region west"));
        assert_eq!(rows[0]["layout"]["items"], json!([{"panelId":"orders"}]));
        assert_eq!(rows[2]["layout"]["items"], json!([{"panelId":"orders~2"}]));
        assert_eq!(expanded.panels["orders"]["title"], json!("Orders eastx"));
        assert_eq!(expanded.panels["orders~1"]["repeatValues"], json!({"region":"west"}));
    }

    #[test]
    fn repeats_without_a_layout_or_values_and_over_the_limits() {
        let body = json!({"panels":{"a":{"title":"$regionx $region","repeat":{"variable":"region"}}}});
        assert!(has_repeats(&body));
        let expanded = expand(&body, &mut regions);
        assert_eq!(expanded.layout["items"].as_array().unwrap().len(), 3);
        assert_eq!(
            expanded.panels["a"]["title"],
            json!("$regionx east"),
            "a longer name is not a variable"
        );
        let none = expand(&body, &mut |_: &str| Vec::new());
        assert_eq!(none.panels.len(), 1, "no values renders the panel once, unpinned");
        assert!(none.panels["a"].get("repeatValues").is_none());
        let mut many = |_: &str| (0..30).map(|n| json!(n)).collect::<Vec<_>>();
        let capped = expand(&body, &mut many);
        assert_eq!(capped.panels.len(), REPEAT_LIMIT);
        assert_eq!(capped.notices[0]["code"], json!("repeat_limit"));
        let rows = json!({"panels":{"a":{"repeat":{"variable":"region"}}},"layout":{"type":"rows","items":[{"title":"r","repeat":{"variable":"other"},"layout":{"type":"autoGrid","items":["a"]}}]}});
        let mut both = |_: &str| (0..24).map(|n| json!(n)).collect::<Vec<_>>();
        let limited = expand(&rows, &mut both);
        assert_eq!(limited.panels.len(), PANEL_LIMIT);
        assert!(limited.notices.iter().any(|notice| notice["code"] == "panel_limit"));
        assert!(!has_repeats(&json!({"panels":{"a":{}}})));
    }

    #[test]
    fn selected_values_fall_back_to_options_for_all() {
        let options = || vec![json!("a"), json!("b")];
        assert_eq!(
            selected_values(Some(&json!("$__all")), options),
            vec![json!("a"), json!("b")]
        );
        assert_eq!(selected_values(Some(&json!(["x"])), options), vec![json!("x")]);
        assert_eq!(selected_values(None, options), vec![json!("a"), json!("b")]);
    }

    #[test]
    fn substitute_replaces_whole_names_only() {
        let pins = json!({"a":"X","n":3}).as_object().unwrap().clone();
        assert_eq!(
            substitute("$a and ${a}, not $ab or $a_ but $a. $n€ $", &pins),
            "X and X, not $ab or $a_ but X. 3€ $"
        );
        let pins = json!({"a":"$a"}).as_object().unwrap().clone();
        assert_eq!(substitute("$a$a", &pins), "$a$a", "a replacement is not replaced again");
        assert_eq!(substitute("€$a€", &pins), "€$a€");
    }
}
