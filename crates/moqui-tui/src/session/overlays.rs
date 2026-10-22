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

//! The overlays' keys: the confirmation, the login, a drop-down, the date picker and the drawer; and opening them.

use super::*;

impl Session {
    pub(super) fn confirm_key(&mut self, mut confirm: Confirm, press: &KeyPress) {
        // An approval has no letter keys and Esc leaves it pending: only Enter on a choice decides (plan 4.11).
        let approval = matches!(confirm.kind, ConfirmKind::Approval(_));
        let chosen = match &press.key {
            Key::Escape | Key::F(12) | Key::Char(_) if approval => {
                if !matches!(press.key, Key::Char(_)) {
                    self.pop_confirm();
                }
                return;
            }
            Key::Left | Key::Up | Key::BackTab => {
                confirm.move_by(-1);
                None
            }
            Key::Right | Key::Down | Key::Tab => {
                confirm.move_by(1);
                None
            }
            Key::Enter => Some(confirm.choice()),
            Key::Escape | Key::F(12) => Some(confirm.choices()[0]),
            Key::Char(ch) => confirm
                .choices()
                .iter()
                .find(|choice| {
                    choice
                        .chars()
                        .next()
                        .is_some_and(|first| first.eq_ignore_ascii_case(ch))
                })
                .copied(),
            _ => None,
        };
        self.pop_confirm();
        match chosen {
            None => self.overlays.push(Overlay::Confirm(confirm)),
            Some(choice) => self.finish_confirm(&confirm, choice),
        }
    }

    pub(super) fn pop_confirm(&mut self) {
        if let Some(at) = self
            .overlays
            .iter()
            .rposition(|overlay| matches!(overlay, Overlay::Confirm(_)))
        {
            self.overlays.remove(at);
        }
    }

    pub(super) fn finish_confirm(&mut self, confirm: &Confirm, choice: &str) {
        match (&confirm.kind, choice) {
            (ConfirmKind::Leave, "Discard") => {
                self.edits.reset();
                self.cursors.clear();
                self.run_then(confirm.then.clone());
            }
            (ConfirmKind::Leave, "Save") => {
                if let Some(form) = self.first_dirty_form() {
                    self.submit_form(&form, Some(confirm.then.clone()));
                } else {
                    self.set_message("These edits are not in a form that can be saved.".to_string(), true);
                }
            }
            (ConfirmKind::Ask(_), "Yes") => self.run_then(confirm.then.clone()),
            (ConfirmKind::Approval(card), choice) => {
                let id = card.approval_id.clone();
                self.assist_decide(&id, choice);
            }
            _ => {}
        }
    }

    pub(super) fn run_then(&mut self, then: Then) {
        match then {
            Then::Navigate(url) => self.navigate(&url, true),
            Then::Back => self.go_back_now(),
            Then::Quit => self.quit = true,
            Then::Submit(id) => self.submit_form_of(&id, None, true),
            Then::FollowLink(id) => self.follow_link_now(&id),
            Then::Nothing => {}
        }
    }

    pub(super) fn login_key(&mut self, mut login: Login, press: &KeyPress) {
        let fields = 3;
        match &press.key {
            Key::Tab | Key::Down => login.at = (login.at + 1) % fields,
            Key::BackTab | Key::Up => login.at = (login.at + fields - 1) % fields,
            Key::Enter => {
                self.overlays.pop();
                // The password stays until the answer: a right one that wants a code is kept for the try with the
                // code, and any other answer clears it (`login_reply`).
                self.lanes.push(
                    Purpose::Login,
                    LinkCall::Login {
                        username: login.username.clone(),
                        password: login.password.clone(),
                        code: std::mem::take(&mut login.code),
                    },
                );
                login.error.clear();
                self.overlays.push(Overlay::Login(login));
                return;
            }
            Key::Char('s') if press.alt && login.code_wanted => {
                if let Some(factor) = login.sendable.first() {
                    self.lanes.push(
                        Purpose::Login,
                        LinkCall::SendCode {
                            username: login.username.clone(),
                            password: login.password.clone(),
                            factor: factor.id.clone(),
                        },
                    );
                    login.error = format!("Sending a code to {}…", factor.label);
                }
            }
            Key::Escape | Key::F(12) => {
                self.overlays.pop();
                self.set_message("Not logged in.".to_string(), true);
                return;
            }
            _ => {
                let text = match login.at {
                    0 => &mut login.username,
                    1 => &mut login.password,
                    _ => &mut login.code,
                };
                let mut cursor = text.chars().count();
                edit::apply(text, &mut cursor, press, &Limits::default());
            }
        }
        self.overlays.pop();
        self.overlays.push(Overlay::Login(login));
    }

    pub(super) fn dropdown_key(&mut self, mut drop: DropDown, press: &KeyPress) {
        let choices = self.drop_choices(&drop.field, &drop.filter);
        let multiple = matches!(
            self.field_widget(&drop.field),
            Some(Widget::DropDown { multiple: true, .. })
        );
        let combo = matches!(
            self.field_widget(&drop.field),
            Some(Widget::DropDown { combo: true, .. })
        );
        let last = choices.len().saturating_sub(1);
        match &press.key {
            Key::Up => drop.at = drop.at.saturating_sub(1),
            Key::Down => drop.at = (drop.at + 1).min(last),
            Key::PageUp => drop.at = drop.at.saturating_sub(10),
            Key::PageDown => drop.at = (drop.at + 10).min(last),
            Key::Home => drop.at = 0,
            Key::End => drop.at = last,
            Key::Char(' ') if multiple => {
                if let Some(choice) = choices.get(drop.at) {
                    self.toggle_value(&drop.field.clone(), &choice.value);
                }
            }
            Key::Enter => {
                self.overlays.pop();
                if !multiple {
                    let field = drop.field.clone();
                    match choices.get(drop.at) {
                        Some(choice) => self.set_buffer(&field, choice.value.clone()),
                        None if combo && !drop.filter.is_empty() => self.set_buffer(&field, drop.filter.clone()),
                        None => {}
                    }
                    self.submit_on_select(&field);
                }
                return;
            }
            Key::Escape | Key::F(4) | Key::F(12) => {
                self.overlays.pop();
                return;
            }
            Key::Backspace => {
                drop.filter.pop();
                drop.at = 0;
                if self.is_search(&drop.field) {
                    self.typed_search(&drop.field);
                }
            }
            Key::Char(ch) if press.is_plain() => {
                drop.filter.push(*ch);
                drop.at = 0;
                if self.is_search(&drop.field) {
                    self.typed_search(&drop.field);
                }
            }
            _ => {}
        }
        self.overlays.pop();
        self.overlays.push(Overlay::DropDown(drop));
    }

    /// A drop-down whose options the server filters by what is typed.
    pub(super) fn is_search(&self, field: &str) -> bool {
        matches!(
            self.field_widget(field),
            Some(Widget::DropDown {
                server_search: true,
                ..
            })
        )
    }

    pub(super) fn submit_on_select(&mut self, field: &str) {
        if matches!(
            self.field_widget(field),
            Some(Widget::DropDown {
                submit_on_select: true,
                ..
            })
        ) {
            self.submit_form_of(field, None, false);
        }
    }

    pub(super) fn toggle_value(&mut self, field: &str, value: &str) {
        let mut values: Vec<String> = self
            .buffer(field)
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(at) = values.iter().position(|item| item == value) {
            values.remove(at);
        } else {
            values.push(value.to_string());
        }
        self.set_buffer(field, values.join(","));
    }

    pub(super) fn picker_key(&mut self, mut picker: Picker, press: &KeyPress) {
        let days = |n: i64| jiff::Span::new().days(n);
        let moved = match &press.key {
            Key::Left => picker.date.checked_add(days(-1)).ok(),
            Key::Right => picker.date.checked_add(days(1)).ok(),
            Key::Up => picker.date.checked_add(days(-7)).ok(),
            Key::Down => picker.date.checked_add(days(7)).ok(),
            Key::PageUp => picker.date.checked_add(jiff::Span::new().months(-1)).ok(),
            Key::PageDown => picker.date.checked_add(jiff::Span::new().months(1)).ok(),
            Key::Home | Key::Char('t') => Some(jiff::Zoned::now().date()),
            Key::Enter => {
                self.overlays.pop();
                if let Some((format, date_type)) = self.date_format(&picker.key) {
                    let text = forms::date_text(picker.date, self.buffer(&picker.key), &format, &date_type);
                    self.set_buffer(&picker.key.clone(), text);
                }
                return;
            }
            Key::Escape | Key::F(4) | Key::F(12) => {
                self.overlays.pop();
                return;
            }
            _ => None,
        };
        if let Some(date) = moved {
            picker.date = date;
        }
        self.overlays.pop();
        self.overlays.push(Overlay::Picker(picker));
    }

    pub(super) fn date_format(&self, key: &str) -> Option<(String, String)> {
        match self.field_widget(key)? {
            Widget::DateTime { format, date_type, .. } => Some((format.clone(), date_type.clone())),
            Widget::DateFind { format, date_type, .. } => Some((format.clone(), date_type.clone())),
            Widget::DatePeriod { time, .. } => {
                Some((String::new(), if *time { "date-time" } else { "date" }.to_string()))
            }
            _ => None,
        }
    }

    pub(super) fn drawer_key(&mut self, at: usize, press: &KeyPress) -> bool {
        let rows = self.drawer_rows();
        let last = rows.len().saturating_sub(1);
        let next = match &press.key {
            Key::Up => at.saturating_sub(1),
            Key::Down => (at + 1).min(last),
            Key::Enter | Key::Right => {
                self.overlays.pop();
                if let Some(url) = rows.get(at).map(|row| row.1.clone()) {
                    self.follow_url(&url);
                }
                return true;
            }
            Key::F(9) => {
                self.overlays.pop();
                self.drawer = Some(false);
                self.keep_drawer(false);
                return true;
            }
            Key::Escape | Key::F(12) | Key::Tab => {
                self.overlays.pop();
                return true;
            }
            _ => return self.keys.action(press) != Some(Action::Quit),
        };
        self.overlays.pop();
        self.overlays.push(Overlay::Drawer(next));
        true
    }

    pub(super) fn open_dropdown(&mut self, key: &str) {
        let current = self.buffer(key).to_string();
        // A search asks as the user types; other remote options load as the list opens.
        let query = self
            .index
            .option_query(key, &|k| self.edits.get(k).to_string())
            .filter(|_| !self.is_search(key));
        if let Some((url, params)) = query {
            self.lanes.push(
                Purpose::Options(key.to_string()),
                LinkCall::Options {
                    url,
                    parameters: params,
                },
            );
        }
        let at = self
            .index
            .choices(key)
            .iter()
            .position(|choice| choice.value == current)
            .unwrap_or(0);
        self.overlays.push(Overlay::DropDown(DropDown {
            field: key.to_string(),
            at,
            filter: String::new(),
        }));
    }

    pub(super) fn open_dialog(&mut self, id: &str) {
        self.open_dynamic(id);
        if !self.dialogs_open().contains(&id) {
            self.overlays.push(Overlay::Dialog(id.to_string()));
        }
        self.rebuild_focus();
        self.focus_first_in_scope();
    }

    pub(super) fn open_find(&mut self) {
        let list = self
            .focus()
            .filter(|item| item.kind == FocusKind::List)
            .map(|item| item.key.clone())
            .or_else(|| self.first_list(true));
        let Some(list) = list else {
            return;
        };
        if self.list(&list).is_none_or(|list| list.header_fields.is_empty()) {
            self.set_message("This list has no find fields.".to_string(), false);
            return;
        }
        self.overlays.push(Overlay::Find(list));
        self.rebuild_focus();
        self.focus_first_in_scope();
    }

    pub(super) fn ask_login(&mut self) {
        if !self.overlays.iter().any(|overlay| matches!(overlay, Overlay::Login(_))) {
            // The SSH login name starts the dialog at the password, as an `ssh user@host` prompt would.
            let (username, at) = match (&self.shell.user, &self.login_name) {
                (user, Some(name)) if user.is_empty() => (name.clone(), 1),
                (user, _) => (user.clone(), 0),
            };
            self.overlays.push(Overlay::Login(Login {
                username,
                at,
                ..Login::default()
            }));
        }
    }

    pub(super) fn login_reply(&mut self, result: Result<(), LinkError>) {
        match result {
            Ok(()) => {
                self.overlays.retain(|overlay| !matches!(overlay, Overlay::Login(_)));
                // The screen asked for when the server wanted the login, and the shell for it. A save that was refused
                // is not sent again: its edits stay, and the user saves again.
                let location = self.last_location.clone().unwrap_or_else(|| self.nav.location());
                self.request_shell(if location.path.is_empty() { "/" } else { &location.path });
                self.request_level(&location.path, location.parameters, Intent::Navigate { history: false });
            }
            Err(err) => {
                if let Some(login) = self.login_mut() {
                    login.error = err.to_string();
                    if let LinkError::Code(ask) = err {
                        // The password was right: keep it, and ask for the code.
                        login.code_wanted = true;
                        login.sendable = ask.sendable;
                        login.at = 2;
                    } else {
                        login.password.clear();
                        login.code_wanted = false;
                        login.sendable.clear();
                        login.at = 1;
                    }
                }
            }
        }
    }

    /// The answer to Alt+S in the login dialog: where the code went, or why not.
    pub(super) fn code_sent(&mut self, result: Result<String, LinkError>) {
        if let Some(login) = self.login_mut() {
            // The factor's label is the client's; the server's answer may hold only its id.
            let to = login
                .sendable
                .first()
                .map_or_else(String::new, |factor| factor.label.clone());
            login.error = match result {
                Ok(_) => format!("A code was sent to {to}"),
                Err(err) => format!("No code sent: {err}"),
            };
            login.at = 2;
        }
    }

    fn login_mut(&mut self) -> Option<&mut Login> {
        self.overlays.iter_mut().rev().find_map(|overlay| match overlay {
            Overlay::Login(login) => Some(login),
            _ => None,
        })
    }
}

pub(super) fn filtered(choices: &[Choice], filter: &str) -> Vec<Choice> {
    if filter.is_empty() {
        return choices.to_vec();
    }
    let needle = filter.to_lowercase();
    choices
        .iter()
        .filter(|choice| choice.label.to_lowercase().contains(&needle) || choice.value.to_lowercase().contains(&needle))
        .cloned()
        .collect()
}
