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

//! `artifact-browser`: a reference graph as Miller columns (the terminal form of the Dev AIDE's Artifact Browser,
//! plans/archive/design-dev-aide.md 7.3): what uses the center on the left, the center in the middle, what it uses on the
//! right, each side in sections by kind. Enter on a row makes it the center and puts it on the trail, which Backspace
//! and `]` walk back and forward; a row already on the trail is marked `↺`. Narrower than [`STACK_BELOW`] columns the
//! three panes stack and one shows at a time. `k`, `h` and `x` change the filter, which the source applies; the
//! center's links (`center.links`) each have a key of their own.
//!
//! Props, each with its default: `source` ("browse", the transition answering the graph), `artifactId` (the first
//! center), `idParameter` ("artifactId", the name the center is asked for by), `usedByTitle` ("Used by") and
//! `usesTitle` ("Uses").
//!
//! The answer: `{center: {id, kind, shortName, name, lines: [text], links: [{key, label, url}]}, usedBy: [{kind, count,
//! rows: [{id, kind, shortName, name, how, calls, observedOnly, resolved}]}], uses: [..], kinds: [kind],
//! filter: {kinds, hideFramework, observed}}`; with no such center, `center` is null and `message` says why. The filter
//! goes back as the parameters `kinds`, `hideFramework` and `observed` (`Y` or `N`) once the user changes it, so the
//! source's own defaults hold until then.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{fit, prop, text_of, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::layout::{display_width, wrap};
use crate::theme::Role;

pub struct ArtifactBrowser;

/// Below this many columns the panes stack.
pub const STACK_BELOW: u16 = 100;

/// The columns: what uses the center, the center, what it uses.
const USED_BY: usize = 0;
const CENTER: usize = 1;
const USES: usize = 2;

/// What the browser keeps between frames, in the state's `local`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct View {
    /// The centers visited, and which of them is the center now.
    trail: Vec<String>,
    at: usize,
    /// The focused column; none until the first answer chooses one.
    column: Option<usize>,
    /// The highlighted row of each side, and the first line shown.
    cursor: [usize; 2],
    top: [usize; 2],
    /// The filter as the user set it; none keeps the source's default.
    kind: Option<String>,
    hide_framework: Option<bool>,
    observed: Option<bool>,
}

impl View {
    fn of(state: &ComponentState) -> Self {
        serde_json::from_value(state.local.clone()).unwrap_or_default()
    }

    fn save(&self, state: &mut ComponentState) {
        state.local = serde_json::to_value(self).unwrap_or(Value::Null);
    }

    /// The center asked for: the trail's, else the props'.
    fn current<'a>(&'a self, props: &'a Value) -> &'a str {
        self.trail
            .get(self.at)
            .map_or_else(|| prop(props, "artifactId", ""), String::as_str)
    }

    fn on_trail(&self, id: &str) -> bool {
        self.trail.iter().any(|seen| seen == id)
    }
}

/// One line of a side: a section's heading or one of its rows (by its place among the side's rows).
enum Entry<'a> {
    Heading(String, usize),
    Row(usize, &'a Value),
}

/// A side's sections as lines, and its rows in order.
fn side<'a>(data: Option<&'a Value>, column: usize) -> (Vec<Entry<'a>>, Vec<&'a Value>) {
    let key = if column == USED_BY { "usedBy" } else { "uses" };
    let mut entries = Vec::new();
    let mut rows = Vec::new();
    let sections = data.and_then(|data| data.get(key)).and_then(Value::as_array);
    for section in sections.into_iter().flatten() {
        let section_rows: Vec<&Value> = section
            .get("rows")
            .and_then(Value::as_array)
            .map(|rows| rows.iter().collect())
            .unwrap_or_default();
        let count = section
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(section_rows.len());
        entries.push(Entry::Heading(text_of(section.get("kind")), count));
        for row in section_rows {
            entries.push(Entry::Row(rows.len(), row));
            rows.push(row);
        }
    }
    (entries, rows)
}

fn side_count(data: Option<&Value>, column: usize) -> usize {
    side(data, column).1.len()
}

/// The line of a side's entries that shows row `row`.
fn line_of(entries: &[Entry<'_>], row: usize) -> usize {
    entries
        .iter()
        .position(|entry| matches!(entry, Entry::Row(at, _) if *at == row))
        .unwrap_or(0)
}

/// How many lines of a side show at once: the terminal's rows less the screen's frame, all of them before it is sized.
fn window(state: &ComponentState) -> usize {
    if state.rows == 0 {
        usize::MAX
    } else {
        usize::from(state.rows).saturating_sub(12).max(5)
    }
}

fn center(data: Option<&Value>) -> Option<&Value> {
    data.and_then(|data| data.get("center"))
        .filter(|center| center.is_object())
}

fn links(data: Option<&Value>) -> Vec<(String, String, String)> {
    center(data)
        .and_then(|center| center.get("links"))
        .and_then(Value::as_array)
        .map(|links| {
            links
                .iter()
                .map(|link| {
                    (
                        text_of(link.get("key")),
                        text_of(link.get("label")),
                        text_of(link.get("url")),
                    )
                })
                .filter(|(key, _, url)| !key.is_empty() && !url.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// A flag of the answer's filter, as the source applied it.
fn applied(data: Option<&Value>, name: &str) -> Option<bool> {
    data.and_then(|data| data.get("filter"))
        .and_then(|filter| filter.get(name))
        .and_then(Value::as_bool)
}

fn flag(value: bool) -> String {
    if value { "Y" } else { "N" }.to_string()
}

/// The source's parameters: the center and the filter the user set.
fn parameters(props: &Value, view: &View) -> BTreeMap<String, String> {
    let mut out = BTreeMap::from([(
        prop(props, "idParameter", "artifactId").to_string(),
        view.current(props).to_string(),
    )]);
    if let Some(kind) = view.kind.as_ref().filter(|kind| !kind.is_empty()) {
        out.insert("kinds".to_string(), kind.clone());
    }
    if let Some(hide) = view.hide_framework {
        out.insert("hideFramework".to_string(), flag(hide));
    }
    if let Some(observed) = view.observed {
        out.insert("observed".to_string(), flag(observed));
    }
    out
}

/// Ask for the current center again, with the filter.
fn call(props: &Value, view: &View) -> Vec<Act> {
    vec![Act::Call {
        url: prop(props, "source", "browse").to_string(),
        parameters: parameters(props, view),
    }]
}

impl ArtifactBrowser {
    /// The column a new answer starts on: the first side with rows, else the center.
    fn first_column(data: Option<&Value>) -> usize {
        [USED_BY, USES]
            .into_iter()
            .find(|column| side_count(data, *column) > 0)
            .unwrap_or(CENTER)
    }

    fn row_line(row: &Value, width: usize, cursor: bool, focused: bool, view: &View, center_id: &str) -> Line {
        let id = text_of(row.get("id"));
        let mut suffix = String::new();
        let calls = row.get("calls").and_then(Value::as_i64).unwrap_or(0);
        if calls > 0 {
            suffix.push_str(&format!(" {calls}"));
        }
        if row.get("observedOnly").and_then(Value::as_bool).unwrap_or(false) {
            suffix.push_str(" ◌");
        }
        let resolved = row.get("resolved").and_then(Value::as_bool).unwrap_or(true);
        if !resolved {
            suffix.push_str(" ?");
        }
        let mark = if !id.is_empty() && (id == center_id || view.on_trail(&id)) {
            "↺"
        } else {
            " "
        };
        let lead = format!("{}{mark}", if cursor { "▸" } else { " " });
        let name_width = width.saturating_sub(display_width(&lead) + display_width(&suffix));
        let text = format!("{lead}{}{suffix}", fit(&text_of(row.get("shortName")), name_width));
        let role = if resolved { Role::Text } else { Role::Muted };
        let span = Span::new(fit(&text, width), role);
        vec![if cursor && focused { span.reverse() } else { span }]
    }

    /// A side's lines at `width`: its title, then its window of sections and rows.
    fn side_lines(props: &Value, state: &ComponentState, view: &View, column: usize, width: usize) -> Vec<Line> {
        let data = state.data.as_ref();
        let (entries, rows) = side(data, column);
        let s = usize::from(column == USES);
        let focused = view.column == Some(column);
        let title = if column == USED_BY {
            prop(props, "usedByTitle", "Used by")
        } else {
            prop(props, "usesTitle", "Uses")
        };
        let heading = Span::new(fit(&format!("{title} ({})", rows.len()), width), Role::Primary).bold();
        let mut lines = vec![vec![if focused { heading.underline() } else { heading }]];
        if rows.is_empty() {
            lines.push(vec![Span::new(fit("  Nothing", width), Role::Muted)]);
            return lines;
        }
        let center_id = center(data).map(|center| text_of(center.get("id"))).unwrap_or_default();
        let top = view.top[s].min(entries.len().saturating_sub(1));
        let shown = window(state);
        if top > 0 {
            lines.push(vec![Span::new(fit(&format!("  ▲ {top} more"), width), Role::Muted)]);
        }
        for entry in entries.iter().skip(top).take(shown) {
            lines.push(match entry {
                Entry::Heading(kind, count) => {
                    vec![Span::new(fit(&format!("── {kind} ({count}) ──"), width), Role::Muted).bold()]
                }
                Entry::Row(at, row) => Self::row_line(row, width, *at == view.cursor[s], focused, view, &center_id),
            });
        }
        let below = entries.len().saturating_sub(top.saturating_add(shown));
        if below > 0 {
            lines.push(vec![Span::new(fit(&format!("  ▼ {below} more"), width), Role::Muted)]);
        }
        lines
    }

    /// The center's lines at `width`: its kind and name, its own lines, then its links with their keys.
    fn center_lines(state: &ComponentState, view: &View, width: usize) -> Vec<Line> {
        let data = state.data.as_ref();
        let Some(center) = center(data) else {
            let message = data
                .map(|data| text_of(data.get("message")))
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| "Nothing to show".to_string());
            return wrap(&message, width)
                .into_iter()
                .map(|line| vec![Span::new(fit(&line, width), Role::Muted)])
                .collect();
        };
        let heading = Span::new(fit(&format!("▣ {}", text_of(center.get("kind"))), width), Role::Info);
        let mut lines = vec![vec![if view.column == Some(CENTER) {
            heading.underline()
        } else {
            heading
        }]];
        lines.push(vec![Span::new(
            fit(&text_of(center.get("shortName")), width),
            Role::Text,
        )
        .bold()]);
        let texts = center.get("lines").and_then(Value::as_array);
        for text in texts.into_iter().flatten() {
            for line in wrap(&text_of(Some(text)), width) {
                lines.push(vec![Span::new(fit(&line, width), Role::Muted)]);
            }
        }
        let labels: Vec<String> = links(data)
            .into_iter()
            .map(|(key, label, _)| format!("[{key}] {label}"))
            .collect();
        let mut line = String::new();
        for label in labels {
            if !line.is_empty() && display_width(&line) + 2 + display_width(&label) > width {
                lines.push(vec![Span::new(fit(&line, width), Role::Primary)]);
                line.clear();
            }
            if !line.is_empty() {
                line.push_str("  ");
            }
            line.push_str(&label);
        }
        if !line.is_empty() {
            lines.push(vec![Span::new(fit(&line, width), Role::Primary)]);
        }
        lines
    }

    /// The line over the panes: the trail, the filter, and whether an answer is on its way.
    fn status_line(state: &ComponentState, view: &View, width: usize) -> Line {
        let data = state.data.as_ref();
        let kind = view.kind.as_deref().filter(|kind| !kind.is_empty()).unwrap_or("all");
        let hide = view
            .hide_framework
            .or_else(|| applied(data, "hideFramework"))
            .unwrap_or(false);
        let observed = view.observed.or_else(|| applied(data, "observed")).unwrap_or(true);
        let mut text = format!(
            "Trail {}/{} · Kind: {kind} · Framework: {} · Observed: {}",
            view.at + 1,
            view.trail.len().max(1),
            if hide { "hidden" } else { "shown" },
            if observed { "shown" } else { "hidden" }
        );
        if state.loading {
            text.push_str(" · Loading…");
        }
        vec![Span::new(fit(&text, width), Role::Muted)]
    }

    /// Move the focused side's cursor to `row` and keep it in the window.
    fn place(state: &ComponentState, view: &mut View, column: usize, row: usize) {
        let (entries, rows) = side(state.data.as_ref(), column);
        let s = usize::from(column == USES);
        let row = row.min(rows.len().saturating_sub(1));
        view.cursor[s] = row;
        let line = line_of(&entries, row);
        let shown = window(state);
        // the section's heading shows with its first row
        let wanted = if line > 0 && matches!(entries.get(line - 1), Some(Entry::Heading(..))) {
            line - 1
        } else {
            line
        };
        if wanted < view.top[s] {
            view.top[s] = wanted;
        } else if shown != usize::MAX && line >= view.top[s] + shown {
            view.top[s] = line + 1 - shown;
        }
    }

    /// Make `id` the center: on the trail after the current one, the cursors back at the top.
    fn recenter(props: &Value, view: &mut View, id: String) -> Vec<Act> {
        let keep = (view.at + 1).min(view.trail.len());
        view.trail.truncate(keep);
        view.trail.push(id);
        view.at = view.trail.len() - 1;
        view.cursor = [0, 0];
        view.top = [0, 0];
        call(props, view)
    }
}

impl Component for ArtifactBrowser {
    fn name(&self) -> &'static str {
        "artifact-browser"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        let view = View::of(state);
        (!view.current(props).is_empty())
            .then(|| (prop(props, "source", "browse").to_string(), parameters(props, &view)))
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        let view = View::of(state);
        let total = usize::from(width);
        let mut lines = Vec::new();
        if !state.error.is_empty() {
            lines.push(vec![Span::new(format!("! {}", state.error), Role::Negative)]);
        }
        if state.data.is_none() {
            let text = if state.loading {
                "Loading…"
            } else if view.current(props).is_empty() {
                "No artifact chosen"
            } else {
                ""
            };
            if !text.is_empty() {
                lines.push(vec![Span::new(text, Role::Muted)]);
            }
            return lines;
        }
        lines.push(Self::status_line(state, &view, total));
        if width < STACK_BELOW {
            // One pane at a time, under a line naming the three with the shown one marked.
            let shown = view.column.unwrap_or(CENTER);
            let data = state.data.as_ref();
            let titles = [
                format!(
                    "{} ({})",
                    prop(props, "usedByTitle", "Used by"),
                    side_count(data, USED_BY)
                ),
                "Center".to_string(),
                format!("{} ({})", prop(props, "usesTitle", "Uses"), side_count(data, USES)),
            ];
            let mut tabs = Vec::new();
            for (column, title) in titles.into_iter().enumerate() {
                let span = Span::new(format!(" {title} "), Role::Primary);
                tabs.push(if column == shown { span.reverse().bold() } else { span });
                tabs.push(Span::new(" ", Role::Text));
            }
            lines.push(tabs);
            lines.extend(match shown {
                CENTER => Self::center_lines(state, &view, total),
                column => Self::side_lines(props, state, &view, column, total),
            });
            return lines;
        }
        // Three panes across, the center as wide as a side or a little wider.
        let gap = " │ ";
        let side_width = total.saturating_sub(6) / 3;
        let center_width = total.saturating_sub(6 + 2 * side_width);
        let columns = [
            Self::side_lines(props, state, &view, USED_BY, side_width),
            Self::center_lines(state, &view, center_width),
            Self::side_lines(props, state, &view, USES, side_width),
        ];
        let widths = [side_width, center_width, side_width];
        let deepest = columns.iter().map(Vec::len).max().unwrap_or(0);
        for at in 0..deepest {
            let mut line = Vec::new();
            for (column, (parts, width)) in columns.iter().zip(widths).enumerate() {
                if column > 0 {
                    line.push(Span::new(gap, Role::Border));
                }
                match parts.get(at) {
                    Some(part) => line.extend(part.iter().cloned()),
                    None => line.push(Span::new(" ".repeat(width), Role::Text)),
                }
            }
            lines.push(line);
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        if !press.is_plain() {
            return None;
        }
        let mut view = View::of(state);
        let data = state.data.as_ref();
        let column = view.column.unwrap_or_else(|| Self::first_column(data));
        view.column = Some(column);
        let s = usize::from(column == USES);
        let count = if column == CENTER { 0 } else { side_count(data, column) };
        let page = match window(state) {
            usize::MAX => 10,
            shown => shown.saturating_sub(1).max(1),
        };
        let acts = match press.key {
            Key::Down | Key::Up | Key::PageDown | Key::PageUp | Key::Home | Key::End => {
                if count == 0 {
                    return None;
                }
                let at = view.cursor[s];
                let target = match press.key {
                    Key::Down if at + 1 < count => at + 1,
                    Key::Up if at > 0 => at - 1,
                    // past either end the key is the screen's: the next or previous widget
                    Key::Down | Key::Up => return None,
                    Key::PageDown => at.saturating_add(page),
                    Key::PageUp => at.saturating_sub(page),
                    Key::Home => 0,
                    _ => count - 1,
                };
                Self::place(state, &mut view, column, target);
                Vec::new()
            }
            Key::Left => {
                view.column = Some(column.saturating_sub(1));
                Vec::new()
            }
            Key::Right => {
                view.column = Some((column + 1).min(USES));
                Vec::new()
            }
            Key::Enter => {
                if column == CENTER {
                    return Some(Vec::new());
                }
                let (_, rows) = side(data, column);
                let Some(row) = rows.get(view.cursor[s]) else {
                    return Some(Vec::new());
                };
                let id = text_of(row.get("id"));
                if id.is_empty() || !row.get("resolved").and_then(Value::as_bool).unwrap_or(true) {
                    let name = text_of(row.get("name"));
                    vec![Act::Message(format!("{name} is not in the index"))]
                } else {
                    Self::recenter(props, &mut view, id)
                }
            }
            Key::Backspace => {
                if view.at == 0 {
                    vec![Act::Message("The start of the trail".to_string())]
                } else {
                    view.at -= 1;
                    view.cursor = [0, 0];
                    view.top = [0, 0];
                    call(props, &view)
                }
            }
            Key::Char(']') => {
                if view.at + 1 >= view.trail.len() {
                    vec![Act::Message("The end of the trail".to_string())]
                } else {
                    view.at += 1;
                    view.cursor = [0, 0];
                    view.top = [0, 0];
                    call(props, &view)
                }
            }
            Key::Char('k') => {
                let kinds: Vec<String> = data
                    .and_then(|data| data.get("kinds"))
                    .and_then(Value::as_array)
                    .map(|kinds| kinds.iter().map(|kind| text_of(Some(kind))).collect())
                    .unwrap_or_default();
                // all, then each kind in turn, then all again
                let now = view.kind.clone().unwrap_or_default();
                let next = match kinds.iter().position(|kind| *kind == now) {
                    Some(at) => kinds.get(at + 1).cloned().unwrap_or_default(),
                    None if now.is_empty() => kinds.first().cloned().unwrap_or_default(),
                    None => String::new(),
                };
                view.kind = Some(next);
                view.cursor = [0, 0];
                view.top = [0, 0];
                call(props, &view)
            }
            Key::Char('h') => {
                let hide = view
                    .hide_framework
                    .or_else(|| applied(data, "hideFramework"))
                    .unwrap_or(false);
                view.hide_framework = Some(!hide);
                view.cursor = [0, 0];
                view.top = [0, 0];
                call(props, &view)
            }
            Key::Char('x') => {
                let observed = view.observed.or_else(|| applied(data, "observed")).unwrap_or(true);
                view.observed = Some(!observed);
                view.cursor = [0, 0];
                view.top = [0, 0];
                call(props, &view)
            }
            Key::Char(typed) => {
                let link = links(data)
                    .into_iter()
                    .find(|(key, _, _)| key.chars().eq(std::iter::once(typed)));
                match link {
                    Some((_, _, url)) => vec![Act::Follow(url)],
                    None => return None,
                }
            }
            _ => return None,
        };
        view.save(state);
        Some(acts)
    }

    fn receive(&self, props: &Value, state: &mut ComponentState, value: Value) {
        let mut view = View::of(state);
        // The trail holds the center's own id, so a name the props gave matches the rows that name it.
        let id = center(Some(&value))
            .map(|center| text_of(center.get("id")))
            .unwrap_or_default();
        if !id.is_empty() {
            if view.trail.is_empty() {
                view.trail.push(id);
                view.at = 0;
            } else if let Some(at) = view.trail.get_mut(view.at) {
                *at = id;
            }
        } else if view.trail.is_empty() {
            let asked = prop(props, "artifactId", "");
            if !asked.is_empty() {
                view.trail.push(asked.to_string());
            }
        }
        // The first answer starts on its first side with rows, and a new center keeps the column, so Enter drills on
        // down one side; a cursor past the rows comes back to the last.
        let column = view.column.unwrap_or_else(|| Self::first_column(Some(&value)));
        view.column = Some(column);
        for (s, column) in [USED_BY, USES].into_iter().enumerate() {
            let count = side_count(Some(&value), column);
            view.cursor[s] = view.cursor[s].min(count.saturating_sub(1));
        }
        state.data = Some(value);
        view.save(state);
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("Up/Down, PgUp/PgDn, Home/End", "Move in the column"),
            ("Left/Right", "The column before / after (one at a time when narrow)"),
            ("Enter", "Make the row the center, on the trail"),
            ("Backspace / ]", "Back / forward on the trail"),
            ("k", "The next kind shown (all, then each kind)"),
            ("h", "Hide or show the framework's own artifacts"),
            ("x", "Hide or show the references only observed"),
            ("[letter]", "Follow the center's link of that key"),
        ]
    }
}

#[cfg(test)]
#[path = "artifact_browser_tests.rs"]
mod tests;
