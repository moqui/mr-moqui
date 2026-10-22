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

//! The client components a `tui` text names (plans/archive/Screen-TUI-Impl.md 2.9): the terminal twin of a `.qvue` screen or of
//! an inline `qvt` part. A component is registered by name with `inventory`, as native components are; an add-on crate
//! registers its own the same way, behind a `tui` feature of its crate:
//!
//! ```ignore
//! inventory::submit! { moqui_tui::Registered(&MyInbox) }
//! ```
//!
//! A component draws itself as lines of text in theme roles, so it needs no Revue import and its tests run without a
//! terminal. Its data comes from the screen's own transitions (the ones its `.qvue` twin calls), through the session's
//! link; it holds its state in a [`ComponentState`] the session keeps by node id.

mod board;
mod calendar;
mod chart;
mod timeline;
mod tree;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::keys::KeyPress;
use crate::theme::Role;

/// A run of text in one look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub role: Role,
    pub bold: bool,
    pub reverse: bool,
    pub underline: bool,
}

impl Span {
    pub fn new(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
            bold: false,
            reverse: false,
            underline: false,
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }
}

/// One line of a component, its spans left to right.
pub type Line = Vec<Span>;

/// The text of a line, for tests and the find.
pub fn line_text(line: &Line) -> String {
    line.iter().map(|span| span.text.as_str()).collect()
}

/// What a component keeps between frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComponentState {
    /// What its transition answered, once it has.
    pub data: Option<Value>,
    /// Why the data did not load.
    pub error: String,
    /// The highlighted item.
    pub at: usize,
    /// A second choice: a board's target column while a card moves.
    pub choosing: Option<usize>,
    /// Its own request parameters: a calendar's month.
    pub parameters: BTreeMap<String, String>,
    /// The data was asked for and has not come.
    pub loading: bool,
    /// The rows a tree has closed, by key.
    pub closed: BTreeSet<String>,
}

/// What a key asks of the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Ask for the data again, with the state's parameters.
    Reload,
    /// Post to a transition of the screen (a name under its path, or a path), then ask for the data again.
    Post {
        url: String,
        fields: BTreeMap<String, String>,
    },
    /// Open a screen.
    Follow(String),
    /// A line for the message line.
    Message(String),
}

/// A client component.
pub trait Component: Sync {
    fn name(&self) -> &'static str;

    /// Where the data comes from: a transition (a name under the screen's path, or a path) and its parameters. None when
    /// the props hold everything.
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)>;

    /// The lines at `width` columns; `wide` is the lg width class and up, where a board shows its columns side by side.
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, wide: bool) -> Vec<Line>;

    /// A key while the component has focus. None when it is not the component's key, so it goes on to the screen's.
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, wide: bool) -> Option<Vec<Act>>;

    /// The component's keys for the help: key, then what it does.
    fn help(&self) -> &'static [(&'static str, &'static str)];
}

/// A component as `inventory` collects it.
pub struct Registered(pub &'static dyn Component);

inventory::collect!(Registered);

inventory::submit! { Registered(&board::Board) }
inventory::submit! { Registered(&calendar::Calendar) }
inventory::submit! { Registered(&chart::Chart) }
inventory::submit! { Registered(&timeline::Timeline) }
inventory::submit! { Registered(&tree::Tree) }

/// The component registered under `name`.
pub fn find(name: &str) -> Option<&'static dyn Component> {
    inventory::iter::<Registered>
        .into_iter()
        .map(|registered| registered.0)
        .find(|component| component.name() == name)
}

/// The names of the components this client knows, for `=` (a remote client knows only those built into it).
pub fn names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = inventory::iter::<Registered>
        .into_iter()
        .map(|registered| registered.0.name())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

// ---- helpers the components share ----

/// A string prop, or the default.
fn prop<'a>(props: &'a Value, name: &str, default: &'a str) -> &'a str {
    props.get(name).and_then(Value::as_str).unwrap_or(default)
}

/// A value as display text: a string as it is, a number as JSON writes it.
fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

/// The list under `key` of the data, else of the props, else the data itself when it is a list.
fn list<'a>(props: &'a Value, state: &'a ComponentState, key: &str) -> Vec<&'a Value> {
    let from = |value: &'a Value| value.get(key).and_then(Value::as_array);
    state
        .data
        .as_ref()
        .and_then(from)
        .or_else(|| from(props))
        .or_else(|| state.data.as_ref().and_then(Value::as_array))
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

/// The data source: the `source` prop and the state's parameters.
fn source_of(props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
    let source = prop(props, "source", "");
    (!source.is_empty()).then(|| (source.to_string(), state.parameters.clone()))
}

/// A Quasar color name as a theme role.
fn role_of_color(color: &str) -> Role {
    let base = color.split('-').next().unwrap_or(color);
    match base {
        "primary" | "blue" | "indigo" => Role::Primary,
        "positive" | "green" | "teal" => Role::Positive,
        "negative" | "red" => Role::Negative,
        "warning" | "orange" | "amber" | "yellow" => Role::Warning,
        "info" | "cyan" | "light" => Role::Info,
        "secondary" | "purple" => Role::Secondary,
        _ => Role::Muted,
    }
}

/// `text` cut or padded to `width` columns.
fn fit(text: &str, width: usize) -> String {
    let shown = crate::layout::truncate_width(text, width);
    let pad = width.saturating_sub(crate::layout::display_width(&shown));
    format!("{shown}{}", " ".repeat(pad))
}

/// The state's message when the data has not come: loading, or why not.
fn waiting(state: &ComponentState) -> Option<Line> {
    if !state.error.is_empty() {
        return Some(vec![Span::new(format!("! {}", state.error), Role::Negative)]);
    }
    (state.data.is_none() && state.loading).then(|| vec![Span::new("Loading…", Role::Muted)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_framework_set_is_registered() {
        assert_eq!(names(), ["board", "calendar", "chart", "timeline", "tree"]);
        assert!(find("board").is_some());
        assert!(find("inbox").is_none());
    }
}
