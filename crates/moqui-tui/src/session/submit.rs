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

//! Form submits, their answers, and what a field's change sets off: depends-on options and visible-when.

use super::*;

impl Session {
    // ---- forms ----

    pub(super) fn submit_form_of(&mut self, key: &str, then: Option<Then>, confirmed: bool) {
        let node = key.split('#').next().unwrap_or(key);
        if !confirmed {
            if let Some(Widget::Submit { confirm, .. }) = self.field_widget(node) {
                if !confirm.is_empty() {
                    let ask = Confirm::ask(confirm.clone(), Then::Submit(node.to_string()));
                    self.overlays.push(Overlay::Confirm(ask));
                    return;
                }
            }
        }
        let Some(form) = self.index.form_of(node).map(|form| form.id.clone()) else {
            return;
        };
        self.submit_form(&form, then);
    }

    pub(super) fn submit_form(&mut self, form_id: &str, then: Option<Then>) {
        if self.lanes.busy(&Purpose::Transition) {
            return;
        }
        let Some(form) = self.index.forms.get(form_id).cloned() else {
            return;
        };
        let values = |key: &str| self.edits.get(key).to_string();
        let marks = forms::check(&self.index, &form.fields, &values);
        if !marks.is_empty() {
            self.apply_marks(marks);
            return;
        }
        let dirty = |key: &str| self.edits.is_dirty(key);
        let mut fields = forms::posted(&self.index, &form.fields, &form.hidden, &values, &dirty);
        // A row action's form posts the selected rows as its list's selection parameter.
        if let Some(list) = &form.selection {
            match self.selection_parameters(list) {
                Some(selected) => fields.extend(selected),
                None => {
                    self.set_message(
                        "Select the rows first: Space selects a row, * all of them.".to_string(),
                        true,
                    );
                    return;
                }
            }
        }
        if form.transition_url.is_empty() {
            // A find form: the screen again, with the form's fields as its parameters, from the first page.
            let mut parameters = self.parameters().clone();
            for (name, value) in fields {
                if value.is_empty() {
                    parameters.remove(&name);
                } else {
                    parameters.insert(name, value);
                }
            }
            parameters.remove("pageIndex");
            self.reload_with(parameters);
            return;
        }
        let keys = form
            .fields
            .iter()
            .filter_map(|id| self.index.fields.get(id))
            .flat_map(forms::field_keys)
            .collect();
        self.submitted = Some(Submitted {
            form: form.id.clone(),
            keys,
            submit_message: form.submit_message.clone(),
            background_hide_id: form.background_hide_id.clone(),
            background_reload_id: form.background_reload_id.clone(),
            then,
        });
        self.lanes.push(
            Purpose::Transition,
            LinkCall::Transition {
                url: form.transition_url.clone(),
                method: form.method.clone(),
                fields,
            },
        );
    }

    /// The find dialog of a list: the list again with its header fields as parameters.
    pub(super) fn submit_find(&mut self, list_id: &str) {
        let Some(list) = self.list(list_id) else {
            return;
        };
        let mut ids = Vec::new();
        crate::model::walk(&list.header_fields, &mut |node| {
            if let crate::model::Node::Field { id, .. } = node {
                ids.push(id.clone());
            }
        });
        let values = |key: &str| self.edits.get(key).to_string();
        let marks = forms::check(&self.index, &ids, &values);
        if !marks.is_empty() {
            self.apply_marks(marks);
            return;
        }
        let dirty = |key: &str| self.edits.is_dirty(key);
        let fields = forms::posted(&self.index, &ids, &[], &values, &dirty);
        let mut parameters = self.parameters().clone();
        for (name, value) in fields {
            if value.is_empty() || value == "N" && name.ends_with("_not") {
                parameters.remove(&name);
            } else {
                parameters.insert(name, value);
            }
        }
        parameters.remove("pageIndex");
        // a find field left empty leaves its operator and its case and not flags out too, so the URL holds only the
        // finds that are made
        let made = |base: &str, parameters: &BTreeMap<String, String>| {
            ["", "_from", "_thru", "_period", "_poffset"]
                .iter()
                .any(|end| parameters.get(&format!("{base}{end}")).is_some_and(|value| !value.is_empty()))
        };
        let flags: Vec<String> = parameters
            .keys()
            .filter(|key| {
                ["_op", "_ic", "_not"]
                    .iter()
                    .find_map(|end| key.strip_suffix(end))
                    .is_some_and(|base| !made(base, &parameters))
            })
            .cloned()
            .collect();
        for key in flags {
            parameters.remove(&key);
        }
        self.reload_with(parameters);
    }

    pub(super) fn transition_reply(&mut self, result: Result<TransitionOutcome, LinkError>) {
        let submitted = self.submitted.take();
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(err) => {
                self.set_message(err.to_string(), true);
                return;
            }
        };
        if !outcome.messages.validation_errors.is_empty() {
            let marks: Vec<(String, String)> = outcome
                .messages
                .validation_errors
                .iter()
                .map(|err| {
                    let id = submitted
                        .as_ref()
                        .and_then(|done| self.index.forms.get(&done.form))
                        .and_then(|form| {
                            form.fields
                                .iter()
                                .find(|id| self.index.fields.get(*id).is_some_and(|field| field.name == err.field))
                        })
                        .cloned()
                        .unwrap_or_else(|| err.field.clone());
                    (id, err.message.clone())
                })
                .collect();
            // An error for a field the form does not show is on the message line, not lost.
            let unshown: Vec<String> = marks
                .iter()
                .filter(|(id, _)| !self.index.fields.contains_key(id))
                .map(|(id, text)| format!("{id}: {text}"))
                .collect();
            // The line shows the first mark, unless the server sent a message or named a field this form does not show.
            self.apply_marks(marks);
            let mut line = outcome.messages.errors.clone();
            line.extend(unshown);
            if !line.is_empty() {
                self.set_message(line.join("; "), true);
            }
            return;
        }
        if !outcome.messages.errors.is_empty() {
            // A service error with no field to mark. The edits stay for another try.
            self.set_message(outcome.messages.errors.join("; "), true);
            return;
        }
        let text = outcome
            .messages
            .info
            .first()
            .cloned()
            .or_else(|| submitted.as_ref().map(|done| done.submit_message.clone()))
            .filter(|text| !text.is_empty())
            // A form saved; a link that posts (Run Job) did something, and saved nothing the user typed.
            .unwrap_or_else(|| {
                if submitted.as_ref().is_some_and(|done| done.form.is_empty()) {
                    "Done".to_string()
                } else {
                    "Saved".to_string()
                }
            });
        if let Some(done) = &submitted {
            self.edits.drop_keys(&done.keys);
            self.edits.clear_marks();
            if !done.background_hide_id.is_empty() {
                let hide = done.background_hide_id.clone();
                self.overlays
                    .retain(|overlay| overlay != &Overlay::Dialog(hide.clone()));
            }
        }
        // A saved row's editor, and a row action's selection, are done with. So is the dialog the form was in: in the
        // browser the screen comes again and its dialogs are closed.
        self.overlays.retain(|overlay| !matches!(overlay, Overlay::Row(..)));
        if let Some(done) = &submitted {
            let dialog = self.index.forms.get(&done.form).and_then(|form| {
                self.index.order.iter().find_map(|item| match &item.scope {
                    Scope::Dialog(id) if form.fields.contains(&item.node) => Some(id.clone()),
                    _ => None,
                })
            });
            if let Some(dialog) = dialog {
                self.overlays
                    .retain(|overlay| overlay != &Overlay::Dialog(dialog.clone()));
            }
        }
        if let Some(list) = submitted
            .as_ref()
            .and_then(|done| self.index.forms.get(&done.form))
            .and_then(|form| form.selection.clone())
        {
            self.selected.remove(&list);
        }
        self.drop_closed_extras();
        let then = submitted.as_ref().and_then(|done| done.then.clone());
        let reload_id = submitted
            .as_ref()
            .map(|done| done.background_reload_id.clone())
            .unwrap_or_default();
        match then {
            Some(Then::Navigate(url)) => self.navigate(&url, true),
            Some(Then::Quit) => self.quit = true,
            Some(Then::Back) => self.go_back_now(),
            // A form whose background-reload-id names a dynamic container on screen: that container loads again,
            // and the rest of the screen, with its edits, stays.
            _ if !reload_id.is_empty() && self.reload_container(&reload_id) => {}
            _ if !outcome.screen_url.is_empty() => {
                let mut location = Location::of_url(&outcome.screen_url);
                location.parameters.extend(outcome.parameters.clone());
                self.request_level(&location.path, location.parameters, Intent::Navigate { history: true });
            }
            // Staying here: the screen again, so it shows what was saved, as the web client does.
            _ => self.reload(),
        }
        self.rebuild_focus();
        self.set_message(text.clone(), false);
        self.after_level = Some(text);
    }

    pub(super) fn reset_form(&mut self, key: &str) {
        let Some(form) = self.index.form_of(key).cloned() else {
            return;
        };
        let keys: Vec<String> = form
            .fields
            .iter()
            .filter_map(|id| self.index.fields.get(id))
            .flat_map(forms::field_keys)
            .collect();
        self.edits.drop_keys(&keys);
        for key in &keys {
            self.cursors.remove(key);
        }
    }

    pub(super) fn apply_marks(&mut self, marks: Vec<(String, String)>) {
        let first = marks.first().map(|(id, _)| id.clone());
        self.edits.set_marks(marks);
        if let Some(id) = first {
            if let Some(at) = self.focus.iter().position(|item| item.key == id || item.node == id) {
                self.focus_at = at;
            }
            if let Some(text) = self.edits.mark(&id) {
                let text = text.to_string();
                self.set_message(text, true);
            }
        }
    }

    pub(super) fn first_dirty_form(&self) -> Option<String> {
        let key = self.edits.dirty_keys().next()?;
        let node = key.split('#').next().unwrap_or(key);
        self.index.form_of(node).map(|form| form.id.clone())
    }

    /// After a field changed: drop-downs that depend on it fetch their options, and fields shown by its value appear.
    pub(super) fn after_edit(&mut self, key: &str) {
        let node = key.split('#').next().unwrap_or(key).to_string();
        let Some(name) = self.index.fields.get(&node).map(|field| field.name.clone()) else {
            return;
        };
        for target in self.index.dependents(&name) {
            if let Some((url, params)) = self.index.option_query(&target, &|k| self.edits.get(k).to_string()) {
                self.lanes.push(
                    Purpose::Options(target),
                    LinkCall::Options {
                        url,
                        parameters: params,
                    },
                );
            }
        }
        if self.index.watched_by(&name) {
            self.rebuild_focus();
        }
        self.refresh_values(&name);
    }

    pub(super) fn apply_choices(&mut self, field: &str, choices: Vec<Choice>) {
        let value = self.buffer(field).to_string();
        let multiple = matches!(self.field_widget(field), Some(Widget::DropDown { multiple: true, .. }));
        let combo = matches!(self.field_widget(field), Some(Widget::DropDown { combo: true, .. }));
        let search = matches!(
            self.field_widget(field),
            Some(Widget::DropDown {
                server_search: true,
                ..
            })
        );
        let known = choices.iter().any(|choice| choice.value == value);
        // A search's answer is the matches for what was typed: the value chosen before stays, with its label.
        let mut choices = choices;
        if search {
            if let Some(current) = self.index.choices(field).iter().find(|choice| choice.value == value) {
                if !value.is_empty() && !known {
                    choices.insert(0, current.clone());
                }
            }
        } else if !value.is_empty() && !combo && !multiple && !known {
            self.edits.set(field, String::new());
        }
        self.index.set_choices(field, choices);
        if let Some(Overlay::DropDown(drop)) = self.overlays.last_mut() {
            if drop.field == field {
                drop.at = drop.at.min(self.index.choices(field).len().saturating_sub(1));
            }
        }
    }
}
