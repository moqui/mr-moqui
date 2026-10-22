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

//! What loads after the screen: a server-search drop-down's options as the user types, a display's value and a text
//! line's default from their transitions, and the bodies of dynamic dialogs and containers.
//!
//! A body is its own render, so its ids are given its container's id and `/` before them: they cannot meet the
//! screen's. It goes into the screen's node, and is kept by the container's id, so a reload of the screen (a page, a
//! sort, a save) shows the old body until the new one comes.

use super::*;

/// A field whose value comes from a url: its id, the url, the url's fixed parameters, and the fields it depends on.
type ValueTarget = (String, String, BTreeMap<String, String>, BTreeMap<String, String>);

impl Session {
    // ---- dynamic dialogs and containers ----

    pub(super) fn request_fragment(&mut self, id: &str, url: &str) {
        self.lanes.push(
            Purpose::Fragment(id.to_string()),
            LinkCall::Fragment {
                url: url.to_string(),
                parameters: BTreeMap::new(),
            },
        );
    }

    pub(super) fn fragment_reply(&mut self, id: &str, result: Result<ScreenModel, LinkError>) {
        let mut nodes = match result {
            Ok(model) => model.c,
            Err(err) => vec![Node::Error {
                id: format!("{id}#error"),
                text: format!("This part did not load: {err}"),
            }],
        };
        crate::model::prefix_ids(&mut nodes, &format!("{id}/"));
        if self.extras.iter().any(|extra| extra.id == id) {
            let title = self
                .extras
                .iter()
                .find(|extra| extra.id == id)
                .map(|extra| extra.title.clone())
                .unwrap_or_default();
            self.set_extra(id, &title, nodes);
        } else {
            self.fragments.insert(id.to_string(), nodes);
            self.put_fragments();
            self.reindex(true);
        }
        // The dialog it fills has the focus.
        if self.dialogs_open().last() == Some(&id) {
            self.rebuild_focus();
            self.focus_first_in_scope();
        }
        // A body's own containers, trees and components load as the screen's do.
        self.load_parts(false, Some(id));
    }

    /// The kept bodies go into their nodes of the levels on screen.
    pub(super) fn put_fragments(&mut self) {
        let fragments = self.fragments.clone();
        for model in self.nav.models_mut() {
            for (id, nodes) in &fragments {
                if let Some(Node::DynamicDialog { c, .. } | Node::DynamicContainer { c, .. }) =
                    crate::model::find_mut(&mut model.c, id)
                {
                    c.clone_from(nodes);
                }
            }
        }
    }

    /// A dynamic dialog opens at once and asks for its body each time, as `m-dynamic-dialog` does.
    pub(super) fn open_dynamic(&mut self, id: &str) -> bool {
        let Some(info) = self.index.dynamic.get(id).cloned().filter(|info| info.dialog) else {
            return false;
        };
        self.request_fragment(id, &info.url);
        true
    }

    /// After a save with a `background-reload-id`: the dynamic container it names loads again, and the screen does not.
    /// Returns false when it names none, so the screen is asked for again.
    pub(super) fn reload_container(&mut self, id: &str) -> bool {
        let Some(info) = self.index.dynamic.get(id).cloned().filter(|info| !info.dialog) else {
            return false;
        };
        self.request_fragment(id, &info.url);
        true
    }

    // ---- server search ----

    /// A server-search drop-down was typed in: ask for its options once the typing stops.
    pub(super) fn typed_search(&mut self, field: &str) {
        let delay = Duration::from_millis(SEARCH_MILLIS);
        self.set_timer(Timer::Suggest(field.to_string()), Instant::now() + delay);
    }

    /// The options of a server-search drop-down for the text typed in its list, sent as `term`.
    pub(super) fn request_suggestions(&mut self, field: &str) {
        let Some(Overlay::DropDown(drop)) = self.overlays.last() else {
            return;
        };
        if drop.field != field {
            return;
        }
        let term = drop.filter.clone();
        let min = match self.field_widget(field) {
            Some(Widget::DropDown { min_length, .. }) => usize::try_from(*min_length).unwrap_or(1),
            _ => 1,
        };
        if term.chars().count() < min.max(1) {
            return;
        }
        if let Some((url, mut parameters)) = self.index.option_query(field, &|k| self.edits.get(k).to_string()) {
            parameters.insert("term".to_string(), term);
            parameters.insert("pageIndex".to_string(), "0".to_string());
            self.lanes.push(
                Purpose::Options(field.to_string()),
                LinkCall::Options { url, parameters },
            );
        }
    }

    // ---- value urls ----

    /// A field changed: a display whose value url depends on it asks again, and an empty text line whose default does.
    pub(super) fn refresh_values(&mut self, name: &str) {
        let targets: Vec<ValueTarget> = self
            .index
            .fields
            .values()
            .filter_map(|field| match &field.widget {
                Widget::Display {
                    value_url,
                    value_parameters,
                    depends_on,
                    ..
                } if !value_url.is_empty()
                    && depends_on
                        .values()
                        .any(|dep| forms::names_field(dep, name, &field.name)) =>
                {
                    Some((
                        field.id.clone(),
                        value_url.clone(),
                        value_parameters.clone(),
                        depends_on.clone(),
                    ))
                }
                Widget::TextLine {
                    default_url,
                    default_parameters,
                    depends_on,
                    ..
                } if !default_url.is_empty()
                    && depends_on
                        .values()
                        .any(|dep| forms::names_field(dep, name, &field.name)) =>
                {
                    Some((
                        field.id.clone(),
                        default_url.clone(),
                        default_parameters.clone(),
                        depends_on.clone(),
                    ))
                }
                _ => None,
            })
            .collect();
        for (id, url, fixed, depends) in targets {
            let Some(target) = self.index.fields.get(&id) else {
                continue;
            };
            let mut parameters = fixed;
            let mut all = true;
            for (parameter, other) in &depends {
                let value = self
                    .index
                    .sibling(target, other)
                    .map(|field| self.edits.get(&field.id).to_string())
                    .unwrap_or_default();
                all &= !value.is_empty();
                parameters.insert(parameter.clone(), value);
            }
            if !all {
                // Not every field it depends on has a value: a display empties, a default waits.
                if matches!(self.field_widget(&id), Some(Widget::Display { .. })) {
                    self.shown_text.insert(id.clone(), String::new());
                }
                continue;
            }
            self.lanes.push(
                Purpose::ValueUrl(id),
                LinkCall::Json {
                    url,
                    method: "post".to_string(),
                    parameters,
                },
            );
        }
    }

    /// A value url's answer: a list or `{options}` whose first entry has `label` and `value`, an object with them, or
    /// plain text. A display shows the label and posts the value; a text line takes the text as its value when the
    /// user has not typed one.
    pub(super) fn value_reply(&mut self, field: &str, result: Result<serde_json::Value, LinkError>) {
        let value = match result {
            Ok(value) => value,
            Err(err) => {
                self.set_message(format!("A value did not load: {err}"), true);
                return;
            }
        };
        let first = match &value {
            serde_json::Value::Array(items) => items.first().cloned(),
            serde_json::Value::Object(map) => match map.get("options") {
                Some(serde_json::Value::Array(items)) => items.first().cloned(),
                _ => Some(value.clone()),
            },
            other => Some(other.clone()),
        }
        .unwrap_or_default();
        let (label, plain) = match &first {
            serde_json::Value::String(text) => (text.clone(), text.clone()),
            serde_json::Value::Number(number) => (number.to_string(), number.to_string()),
            object => {
                let label = shell::json_text(object, "label");
                let plain = shell::json_text(object, "value");
                (
                    if label.is_empty() { plain.clone() } else { label },
                    if plain.is_empty() {
                        shell::json_text(object, "label")
                    } else {
                        plain
                    },
                )
            }
        };
        match self.field_widget(field).cloned() {
            Some(Widget::Display { value: posted, .. }) => {
                self.shown_text
                    .insert(field.to_string(), crate::model::clean_text(&label));
                if posted.is_some() {
                    self.edits.set(field, crate::model::clean_text(&plain));
                }
            }
            Some(Widget::TextLine { .. }) if !plain.is_empty() => {
                self.set_buffer(field, crate::model::clean_text(&plain));
            }
            _ => {}
        }
    }
}
