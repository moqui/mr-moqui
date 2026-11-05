/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! Playlist rotation and controls over the reporting dashboard component.
use super::ReportDashboard;
use crate::components::{text_of, Act, Component, ComponentState, Line, Span};
use crate::keys::{Key, KeyPress};
use crate::theme::Role;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct ReportPlaylist;

impl Component for ReportPlaylist {
    fn name(&self) -> &'static str {
        "report-playlist"
    }
    fn source(&self, props: &Value, state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        if props["items"].as_array().is_none_or(Vec::is_empty) {
            return None;
        }
        let mut params = BTreeMap::new();
        for key in ["definitionId", "from", "thru"] {
            if let Some(value) = props[key].as_str() {
                params.insert(key.into(), value.into());
            }
        }
        params.insert(
            "index".into(),
            state.local["playlistAt"].as_u64().unwrap_or(0).to_string(),
        );
        Some((props["source"].as_str().unwrap_or("data").into(), params))
    }
    fn lines(&self, props: &Value, state: &ComponentState, width: u16, wide: bool) -> Vec<Line> {
        let at = state.local["playlistAt"].as_u64().unwrap_or(0) as usize;
        let mut lines = vec![vec![Span::new(
            format!(
                "Playlist {}/{}: {} ({}) — n/b change, p pauses",
                at + 1,
                props["items"].as_array().map_or(0, Vec::len),
                text_of(props["items"].get(at).and_then(|item| item.get("title"))),
                if state.closed.contains("__playlist_paused") {
                    "paused"
                } else {
                    "running"
                }
            ),
            Role::Primary,
        )]];
        lines.extend(ReportDashboard.lines(props, state, width, wide));
        lines
    }
    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, wide: bool) -> Option<Vec<Act>> {
        if press.is_plain() {
            match press.key {
                Key::Char('p') => {
                    if !state.closed.remove("__playlist_paused") {
                        state.closed.insert("__playlist_paused".into());
                    }
                    return Some(Vec::new());
                }
                Key::Char('n') | Key::Char('b') => {
                    let count = props["items"].as_array().map_or(0, Vec::len);
                    if count > 0 {
                        let at = state.local["playlistAt"].as_u64().unwrap_or(0) as usize;
                        state.local["playlistAt"] = Value::from(if press.key == Key::Char('b') {
                            (at + count - 1) % count
                        } else {
                            (at + 1) % count
                        });
                        state.at = 0;
                        return Some(vec![Act::Reload]);
                    }
                }
                _ => {}
            }
        }
        ReportDashboard.key(props, state, press, wide)
    }
    fn poll_millis(&self, props: &Value, state: &ComponentState) -> Option<u64> {
        (!state.closed.contains("__playlist_paused") && props["items"].as_array().is_some_and(|items| items.len() > 1))
            .then(|| props["seconds"].as_u64().unwrap_or(30).clamp(5, 3600) * 1000)
    }
    fn tick(&self, props: &Value, state: &mut ComponentState) -> Vec<Act> {
        if state.loading {
            return Vec::new();
        }
        self.key(props, state, &KeyPress::plain(Key::Char('n')), false)
            .unwrap_or_default()
    }
    fn receive(&self, props: &Value, state: &mut ComponentState, value: Value) {
        ReportDashboard.receive(props, state, value);
    }
    fn kiosk(&self, props: &Value, state: &ComponentState) -> bool {
        ReportDashboard.kiosk(props, state)
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            ("n/b", "Next/previous dashboard"),
            ("p", "Pause/resume playlist"),
            ("g", "Table controls"),
            ("i", "Inspect panel"),
            ("d k", "Kiosk mode; Esc leaves"),
        ]
    }
}
