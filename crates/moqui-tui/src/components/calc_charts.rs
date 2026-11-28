/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The charts of the terminal sheet (plans/IronCalc-DB2.md CG3.3): drawn below it, and an author's edits of them from
//! the menu, each the `charts` edit of the document's whole list, as the browser's toolbar and chart dialog send it.

use serde_json::{json, Value};

use super::{column_name, edit, Editing, View};
use crate::components::{Act, Component, ComponentState, Line, Span};
use crate::theme::Role;

/// The chart types the menu offers, from its first chart type item on, as the browser's dialog names them.
pub(super) const CHART_TYPES: [&str; 5] = ["column", "bar", "line", "area", "pie"];
/// The menu's first chart item.
pub(super) const CHART_MENU: usize = 16;

/// A range as a formula names it, as the browser's `calc.rangeName` does: `Sheet!A2:A8`, the sheet's name quoted
/// when it is not a plain name.
pub(super) fn range_name(sheet: &str, row: i32, column: i32, last_row: i32, last_column: i32) -> String {
    let mut cells = format!("{}{row}", column_name(column));
    if last_row > row || last_column > column {
        cells.push_str(&format!(":{}{last_row}", column_name(last_column)));
    }
    if sheet.is_empty() {
        return cells;
    }
    let plain = sheet
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && sheet.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if plain {
        format!("{sheet}!{cells}")
    } else {
        format!("'{}'!{cells}", sheet.replace('\'', "''"))
    }
}

/// The document's charts as the `charts` edit takes them, the whole list (as the browser's `chartDefinitions`): each
/// chart's `title`, `type`, `categories` and `series` of `name` and `values`, without what the room read for them.
pub(super) fn chart_definitions(view: &View) -> Vec<Value> {
    view.room["charts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|chart| {
            let series: Vec<Value> = chart["series"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|one| json!({"name": one["name"], "values": one["values"]}))
                .collect();
            json!({"title": chart["title"], "type": chart["type"], "categories": chart["categories"], "series": series})
        })
        .collect()
}

/// A chart of the selection, as the browser's toolbar Chart makes one: its first column the labels and each other
/// column a series when it has more than one column, else the one column a series with no labels. None when the
/// selection is one row.
pub(super) fn chart_of_selection(view: &View) -> Option<Value> {
    let (row, column, last_row, last_column) = view.area();
    if last_row <= row {
        return None;
    }
    let sheet = view.layout()["name"].as_str().unwrap_or("").to_string();
    let several = last_column > column;
    let first = if several { column + 1 } else { column };
    let series: Vec<Value> = (first..=last_column)
        .enumerate()
        .map(|(at, each)| {
            json!({"name": format!("Series {}", at + 1), "values": range_name(&sheet, row, each, last_row, each)})
        })
        .collect();
    let categories = if several {
        range_name(&sheet, row, column, last_row, column)
    } else {
        String::new()
    };
    Some(json!({"title": "", "type": "column", "categories": categories, "series": series}))
}

/// The charts edit: the whole list, with `change` applied to the chart the menu acts on.
pub(super) fn chart_edit(view: &mut View, props: &Value, change: impl FnOnce(&mut Vec<Value>, usize)) -> Vec<Act> {
    let mut charts = chart_definitions(view);
    if charts.is_empty() {
        view.message = "No chart yet: add one of the selection first".to_string();
        return Vec::new();
    }
    let at = view.chart.min(charts.len() - 1);
    change(&mut charts, at);
    view.chart = at.min(charts.len().saturating_sub(1));
    edit(view, props, vec![json!({"op": "charts", "charts": charts})])
}

/// Text typed for a chart (its title, labels or series), set: a new chart's title adds it.
pub(super) fn chart_text(view: &mut View, props: &Value, target: &str, text: &str) -> Vec<Act> {
    let text = text.trim().to_string();
    if target == "chartTitle" {
        if let Some(mut chart) = view.new_chart.take() {
            chart["title"] = json!(text);
            let mut charts = chart_definitions(view);
            charts.push(chart);
            view.chart = charts.len() - 1;
            view.charts = true;
            return edit(view, props, vec![json!({"op": "charts", "charts": charts})]);
        }
    }
    chart_edit(view, props, |charts, at| match target {
        "chartTitle" => charts[at]["title"] = json!(text),
        "chartLabels" => charts[at]["categories"] = json!(text),
        _ => {
            let names: Vec<Value> = charts[at]["series"]
                .as_array()
                .map(|series| series.iter().map(|one| one["name"].clone()).collect())
                .unwrap_or_default();
            let series: Vec<Value> = text
                .split(',')
                .map(str::trim)
                .filter(|values| !values.is_empty())
                .enumerate()
                .map(|(index, values)| {
                    let name = names
                        .get(index)
                        .filter(|name| name.as_str().is_some_and(|name| !name.is_empty()))
                        .cloned()
                        .unwrap_or_else(|| json!(format!("Series {}", index + 1)));
                    json!({"name": name, "values": values})
                })
                .collect();
            charts[at]["series"] = Value::Array(series);
        }
    })
}

/// Start typing a chart's text in the formula bar, with what it has now.
pub(super) fn chart_prompt(view: &mut View, target: &str, text: String) {
    view.editing = Some(Editing {
        cursor: text.chars().count(),
        text,
        target: target.to_string(),
        choice: 0,
    });
}

/// A chart item of the menu, `choice` from [`CHART_MENU`] on: an author's.
pub(super) fn menu_choice(view: &mut View, props: &Value, choice: usize) -> Vec<Act> {
    match choice {
        CHART_MENU => {
            if let Some(chart) = chart_of_selection(view) {
                view.new_chart = Some(chart);
                chart_prompt(view, "chartTitle", String::new());
            } else {
                view.message =
                    "Select the chart's cells first: a column of labels, then a column for each series".into();
            }
            Vec::new()
        }
        choice if choice <= CHART_MENU + 3 => {
            let charts = chart_definitions(view);
            let Some(chart) = charts.get(view.chart.min(charts.len().saturating_sub(1))) else {
                view.message = "No chart yet: add one of the selection first".to_string();
                return Vec::new();
            };
            let (target, text) = match choice - CHART_MENU {
                1 => ("chartTitle", chart["title"].as_str().unwrap_or("").to_string()),
                2 => ("chartLabels", chart["categories"].as_str().unwrap_or("").to_string()),
                _ => (
                    "chartSeries",
                    chart["series"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|one| one["values"].as_str())
                        .collect::<Vec<&str>>()
                        .join(", "),
                ),
            };
            chart_prompt(view, target, text);
            Vec::new()
        }
        choice if choice < CHART_MENU + 4 + CHART_TYPES.len() => {
            let kind = CHART_TYPES[choice - CHART_MENU - 4];
            chart_edit(view, props, |charts, at| charts[at]["type"] = json!(kind))
        }
        _ => chart_edit(view, props, |charts, at| {
            charts.remove(at);
        }),
    }
}

/// The document's charts as the chart component draws a dataset: a table with a bar for the first series.
pub(super) fn chart_lines(view: &View, width: usize, lines: &mut Vec<Line>) {
    let charts = view.room["charts"].as_array().map(Vec::as_slice).unwrap_or_default();
    if charts.is_empty() {
        lines.push(vec![Span::new(
            "No chart: select its cells, then m and Chart: add one of the selection",
            Role::Muted,
        )]);
        return;
    }
    let chosen = view.chart.min(charts.len() - 1);
    for (at, chart) in charts.iter().enumerate() {
        let title = chart["title"]
            .as_str()
            .filter(|title| !title.is_empty())
            .map_or_else(|| format!("Chart {}", at + 1), str::to_string);
        let kind = chart["type"].as_str().unwrap_or("line");
        let head = Span::new(
            format!("{} {title} · {kind}", if at == chosen { ">" } else { " " }),
            Role::Text,
        )
        .bold();
        lines.push(vec![if at == chosen { head.reverse() } else { head }]);
        if let Some(error) = chart["error"].as_str() {
            lines.push(vec![Span::new(format!("! {error}"), Role::Negative)]);
            continue;
        }
        let props = json!({"labels": chart["labels"],
            "series": chart["series"].as_array().map(|series| series.iter()
                .map(|one| json!({"name": one["name"], "data": one["data"]})).collect::<Vec<Value>>())});
        lines.extend(crate::components::chart::Chart.lines(
            &props,
            &ComponentState::default(),
            u16::try_from(width).unwrap_or(u16::MAX),
            false,
        ));
    }
}
