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

//! `timeline`: entries in time order, each its time and title on a marked line with its text under it.
//!
//! Props: `items` (`[{time, title, text, kind, url}]`) in the props or in what `source` answers.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{list, prop, source_of, text_of, waiting, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;

pub struct Timeline;

impl Component for Timeline {
    fn name(&self) -> &'static str {
        "timeline"
    }

    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        source_of(props, state)
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        if let Some(line) = waiting(state) {
            return vec![line];
        }
        let items = list(props, state, prop(props, "items", "items"));
        if items.is_empty() {
            return vec![vec![Span::new("Nothing yet", Role::Muted)]];
        }
        let mut lines = Vec::new();
        let room = usize::from(width).saturating_sub(4);
        for (at, item) in items.iter().enumerate() {
            let time = text_of(item.get("time"));
            let mut title = Span::new(text_of(item.get("title")), Role::Text).bold();
            if at == state.at {
                title = title.reverse();
            }
            let role = super::role_of_color(&text_of(item.get("kind")));
            lines.push(vec![
                Span::new("● ", role),
                Span::new(format!("{time}  "), Role::Muted),
                title,
            ]);
            let text = text_of(item.get("text"));
            for part in crate::layout::wrap(&text, room) {
                if !part.is_empty() {
                    lines.push(vec![Span::new("│   ", Role::Border), Span::new(part, Role::Text)]);
                }
            }
        }
        lines
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        let count = list(props, state, prop(props, "items", "items")).len();
        match press.key {
            Key::Down if state.at + 1 < count => state.at += 1,
            Key::Up if state.at > 0 => state.at -= 1,
            Key::Enter => {
                let url = list(props, state, prop(props, "items", "items"))
                    .get(state.at)
                    .map(|item| text_of(item.get("url")))
                    .unwrap_or_default();
                return (!url.is_empty()).then(|| vec![Act::Follow(url)]);
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[("Up/Down", "Previous / next entry"), ("Enter", "Open the entry")]
    }
}
