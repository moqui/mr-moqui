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

//! `chart`: the datasets of a chart as a table, a label a row and a series a column, with a bar for the first series
//! scaled to the widest value. A terminal cannot draw the browser's chart; the numbers and one bar read the same.
//!
//! Props: `labels` (the row labels) and `series` (`[{name, data}]`), in the props or in what `source` answers; `title`.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{fit, list, prop, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;

pub struct Chart;

fn number(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(0.0),
        Value::String(text) => text.trim().replace(',', "").parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// A number as a person reads it: no fraction when it is whole, else two places.
fn shown(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

impl Component for Chart {
    fn name(&self) -> &'static str {
        "chart"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        let mut lines = Vec::new();
        let title = prop(props, "title", "");
        if !title.is_empty() {
            lines.push(vec![Span::new(title, Role::Text).bold()]);
        }
        if let Some(line) = waiting(state) {
            lines.push(line);
            return lines;
        }
        let labels: Vec<String> = list(props, state, "labels")
            .into_iter()
            .map(|v| text_of(Some(v)))
            .collect();
        let series: Vec<(String, Vec<f64>)> = list(props, state, "series")
            .into_iter()
            .map(|set| {
                let data = set
                    .get("data")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(number).collect())
                    .unwrap_or_default();
                (text_of(set.get("name")), data)
            })
            .collect();
        if labels.is_empty() || series.is_empty() {
            lines.push(vec![Span::new("No data", Role::Muted)]);
            return lines;
        }
        let label_w = labels
            .iter()
            .map(|l| crate::layout::display_width(l))
            .max()
            .unwrap_or(4)
            .clamp(4, 24);
        let value_w: Vec<usize> = series
            .iter()
            .map(|(name, data)| {
                data.iter()
                    .map(|v| shown(*v).len())
                    .chain(std::iter::once(crate::layout::display_width(name)))
                    .max()
                    .unwrap_or(4)
                    .min(16)
            })
            .collect();
        let used = label_w + value_w.iter().map(|w| w + 2).sum::<usize>() + 2;
        let bar_room = usize::from(width).saturating_sub(used);
        let mut head = vec![Span::new(fit("", label_w), Role::Muted)];
        for ((name, _), w) in series.iter().zip(&value_w) {
            head.push(Span::new(format!("  {:>w$}", name, w = *w), Role::Text).bold());
        }
        lines.push(head);
        let max = series[0].1.iter().copied().fold(0.0_f64, f64::max);
        for (at, label) in labels.iter().enumerate() {
            let mut line = vec![Span::new(fit(label, label_w), Role::Text)];
            for ((_, data), w) in series.iter().zip(&value_w) {
                let value = data.get(at).copied().unwrap_or(0.0);
                line.push(Span::new(format!("  {:>w$}", shown(value), w = *w), Role::Text));
            }
            if bar_room >= 4 && max > 0.0 {
                let value = series[0].1.get(at).copied().unwrap_or(0.0).max(0.0);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                let cells = ((value / max) * bar_room as f64).round() as usize;
                line.push(Span::new(
                    format!("  {}", "█".repeat(cells.min(bar_room))),
                    Role::Primary,
                ));
            }
            lines.push(line);
        }
        lines
    }

    fn key(&self, _props: &Value, _state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        match press.key {
            Key::Char('r') if press.is_plain() => Some(vec![Act::Reload]),
            _ => None,
        }
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[("r", "Read the data again")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::line_text;

    #[test]
    fn a_series_is_a_table_with_bars() {
        let props = serde_json::json!({"title": "Sales", "labels": ["Jan", "Feb"],
            "series": [{"name": "Total", "data": [10, 5]}, {"name": "Count", "data": ["2", "1"]}]});
        let lines: Vec<String> = Chart
            .lines(&props, &ComponentState::default(), 40, false)
            .iter()
            .map(line_text)
            .collect();
        assert_eq!(lines[0], "Sales");
        assert!(lines[1].contains("Total") && lines[1].contains("Count"));
        assert!(lines[2].starts_with("Jan") && lines[2].contains("10"));
        let full = lines[2].matches('█').count();
        let half = lines[3].matches('█').count();
        assert!(full > 0 && half * 2 >= full - 1 && half * 2 <= full + 1, "{lines:?}");
    }
}
