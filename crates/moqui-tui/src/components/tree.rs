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

//! `tree`: a list's rows as an indented outline under their parents (`view-tree`, style-guide §3.15), `-` before an open row
//! with children and `+` before a closed one, then its columns and its roll-ups right-aligned. A row whose parent is not
//! among the rows is at the top. All rows start open; Left and Right close and open them.
//!
//! Props: `rows` (`[{key, parent, label, tc: [text], tr: [text], _open}]`, the list's page), `columns` (the titles of the
//! label and of `tc`), `rollups` (the titles of `tr`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::{fit, list, text_of, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;

pub struct Tree;

/// A row shown: its place in the props' rows, its level from 0, and whether it has children.
struct Shown {
    row: usize,
    level: usize,
    parent: bool,
}

fn texts(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text_of(Some(item))).collect())
        .unwrap_or_default()
}

/// The rows shown, depth first: a row's children under it unless it is closed.
fn shown(rows: &[&Value], closed: &BTreeSet<String>) -> Vec<Shown> {
    let keys: Vec<String> = rows.iter().map(|row| text_of(row.get("key"))).collect();
    let known: BTreeSet<&str> = keys.iter().map(String::as_str).filter(|key| !key.is_empty()).collect();
    let mut children: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (at, row) in rows.iter().enumerate() {
        let parent = text_of(row.get("parent"));
        let placed = if !parent.is_empty() && parent != keys[at] && known.contains(parent.as_str()) {
            parent
        } else {
            String::new()
        };
        children.entry(placed).or_default().push(at);
    }
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<(usize, usize)> = children
        .get("")
        .map_or(Vec::new(), |top| top.iter().rev().map(|at| (*at, 0)).collect());
    while let Some((at, level)) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        let kids = children.get(&keys[at]).filter(|_| !keys[at].is_empty());
        let parent = kids.is_some_and(|kids| !kids.is_empty());
        out.push(Shown { row: at, level, parent });
        if parent && !closed.contains(&keys[at]) {
            stack.extend(kids.into_iter().flatten().rev().map(|kid| (*kid, level + 1)));
        }
    }
    out
}

impl Component for Tree {
    fn name(&self) -> &'static str {
        "tree"
    }

    fn source(&self, _props: &Value, _state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        None
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        let rows = list(props, state, "rows");
        if rows.is_empty() {
            return vec![vec![Span::new("Nothing to show", Role::Muted)]];
        }
        let titles = texts(props.get("columns"));
        let rollups = texts(props.get("rollups"));
        let visible = shown(&rows, &state.closed);
        // the columns after the label and the roll-ups: each as wide as its title and texts, at most 14
        let column_texts = |at: usize, key: &str| -> Vec<String> {
            rows.iter()
                .map(|row| texts(row.get(key)).get(at).cloned().unwrap_or_default())
                .collect()
        };
        let widths = |names: &[String], key: &str| -> Vec<usize> {
            names
                .iter()
                .enumerate()
                .map(|(at, name)| {
                    column_texts(at, key)
                        .iter()
                        .chain(std::iter::once(name))
                        .map(|text| crate::layout::display_width(text))
                        .max()
                        .unwrap_or(0)
                        .min(14)
                })
                .collect()
        };
        let extra_titles: Vec<String> = titles.iter().skip(1).cloned().collect();
        let tc_widths = widths(&extra_titles, "tc");
        let tr_widths = widths(&rollups, "tr");
        let rest: usize = tc_widths.iter().chain(&tr_widths).map(|width| width + 2).sum();
        let label_width = usize::from(width).saturating_sub(rest).max(12);
        let right = |text: &str, width: usize| {
            let shown = crate::layout::truncate_width(text, width);
            format!(
                "{}{shown}",
                " ".repeat(width.saturating_sub(crate::layout::display_width(&shown)))
            )
        };
        let mut head = vec![Span::new(fit(titles.first().map_or("", String::as_str), label_width), Role::Muted).bold()];
        for (title, width) in extra_titles.iter().zip(&tc_widths) {
            head.push(Span::new(format!("  {}", fit(title, *width)), Role::Muted).bold());
        }
        for (title, width) in rollups.iter().zip(&tr_widths) {
            head.push(Span::new(format!("  {}", right(title, *width)), Role::Muted).bold());
        }
        let mut lines = vec![head];
        for (at, entry) in visible.iter().enumerate() {
            let row = rows[entry.row];
            let key = text_of(row.get("key"));
            let marker = if !entry.parent {
                "  "
            } else if state.closed.contains(&key) {
                "+ "
            } else {
                "- "
            };
            let label = format!("{}{marker}{}", "  ".repeat(entry.level), text_of(row.get("label")));
            let mut title = Span::new(fit(&label, label_width), Role::Text);
            if entry.parent {
                title = title.bold();
            }
            if at == state.at {
                title = title.reverse();
            }
            let mut line = vec![title];
            for (text, width) in texts(row.get("tc")).iter().zip(&tc_widths) {
                line.push(Span::new(format!("  {}", fit(text, *width)), Role::Text));
            }
            let role = if entry.parent { Role::Text } else { Role::Muted };
            for (text, width) in texts(row.get("tr")).iter().zip(&tr_widths) {
                line.push(Span::new(format!("  {}", right(text, *width)), role));
            }
            lines.push(line);
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        let seen = state.clone();
        let rows = list(props, &seen, "rows");
        let visible = shown(&rows, &seen.closed);
        let entry = visible.get(state.at)?;
        let key = text_of(rows[entry.row].get("key"));
        let parent_at = |level: usize| visible[..state.at].iter().rposition(|other| other.level + 1 == level);
        match press.key {
            Key::Down if state.at + 1 < visible.len() => state.at += 1,
            Key::Up if state.at > 0 => state.at -= 1,
            Key::Right | Key::Char('+') => {
                if entry.parent && state.closed.contains(&key) {
                    state.closed.remove(&key);
                } else if entry.parent {
                    state.at += 1;
                }
            }
            Key::Left | Key::Char('-') => {
                if entry.parent && !state.closed.contains(&key) {
                    state.closed.insert(key);
                } else if let Some(up) = parent_at(entry.level) {
                    state.at = up;
                }
            }
            Key::Enter => {
                let url = text_of(rows[entry.row].get("_open"));
                if url.is_empty() {
                    return Some(vec![Act::Message(text_of(rows[entry.row].get("label")))]);
                }
                return Some(vec![Act::Follow(url)]);
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Up/Down", "Previous / next row"),
            ("Right/+", "Open the row, or to its first child"),
            ("Left/-", "Close the row, or to its parent"),
            ("Enter", "Open the row's record"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::line_text;

    fn props() -> Value {
        serde_json::json!({
            "columns": ["Item", "Status"],
            "rollups": ["Total size"],
            "rows": [
                {"key": "A", "parent": "", "label": "Portal", "tc": ["Open"], "tr": ["30"], "_open": "/apps/Item?id=A"},
                {"key": "B", "parent": "A", "label": "Sign-in", "tc": ["Done"], "tr": ["8"]},
                {"key": "C", "parent": "B", "label": "Reset", "tc": ["Done"], "tr": ["3"]},
                {"key": "D", "parent": "Z", "label": "Orphan", "tc": [""], "tr": ["1"]}
            ]
        })
    }

    #[test]
    fn rows_are_an_outline_that_opens_and_closes() {
        let mut state = ComponentState::default();
        let lines: Vec<String> = Tree.lines(&props(), &state, 50, false).iter().map(line_text).collect();
        assert!(
            lines[0].starts_with("Item") && lines[0].ends_with("Total size"),
            "{lines:?}"
        );
        assert!(lines[1].starts_with("- Portal"), "{lines:?}");
        assert!(lines[2].starts_with("  - Sign-in"), "{lines:?}");
        assert!(lines[3].starts_with("      Reset"), "{lines:?}");
        assert!(
            lines[4].starts_with("  Orphan"),
            "a row whose parent is not here is at the top: {lines:?}"
        );
        assert!(lines[1].ends_with("30") && lines[3].ends_with(" 3"), "{lines:?}");
        let press = |key| KeyPress::plain(key);
        // Down to Sign-in, Left closes it, Left again to its parent; Enter opens Portal's record
        Tree.key(&props(), &mut state, &press(Key::Down), false);
        Tree.key(&props(), &mut state, &press(Key::Left), false);
        let lines: Vec<String> = Tree.lines(&props(), &state, 50, false).iter().map(line_text).collect();
        assert!(
            lines[2].starts_with("  + Sign-in") && lines[3].starts_with("  Orphan"),
            "{lines:?}"
        );
        Tree.key(&props(), &mut state, &press(Key::Left), false);
        assert_eq!(state.at, 0);
        assert_eq!(
            Tree.key(&props(), &mut state, &press(Key::Enter), false),
            Some(vec![Act::Follow("/apps/Item?id=A".to_string())])
        );
        // Right on the closed row opens it again
        state.at = 1;
        Tree.key(&props(), &mut state, &press(Key::Right), false);
        assert!(state.closed.is_empty());
        assert_eq!(Tree.key(&props(), &mut state, &press(Key::Up), false), Some(Vec::new()));
        assert_eq!(Tree.key(&props(), &mut state, &press(Key::Up), false), None);
    }
}
