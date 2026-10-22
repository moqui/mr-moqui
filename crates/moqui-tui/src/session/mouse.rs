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

//! Clicks (plans/archive/Screen-TUI-Impl.md Phase 5): a click on what the view drew does what the keys do there. The view finds
//! the target under the pointer; the session acts on it through the same paths as Enter, so every confirmation and
//! the question before leaving a changed form still ask.

use super::*;
use crate::keys::Target;

impl Session {
    /// A click on `target`; `count` is 2 for a double click. A click on a field focuses it (a check box toggles); on a
    /// link, a button, a tab, a menu or a choice it does what Enter does. A list row becomes the current one, and a
    /// double click on it is Enter on it. While a popup (a drop-down list, a menu, the palette, a confirm, the login)
    /// is open, a click elsewhere does nothing, so a stray click loses no edits.
    pub fn click(&mut self, target: &Target, count: u8) {
        if self.message_expired() {
            self.clear_message();
        }
        match target {
            Target::Block => {}
            Target::KeyLine(action) => self.act(*action),
            Target::Choice(at) => {
                if let Some(Overlay::DropDown(drop)) = self.overlays.last_mut() {
                    drop.at = *at;
                    self.enter();
                }
            }
            Target::MenuItem(at) => {
                if let Some(Overlay::Menu(menu)) = self.overlays.last_mut() {
                    menu.at = *at;
                    self.enter();
                }
            }
            Target::PaletteRow(at) => {
                if let Some(Overlay::Palette(palette)) = self.overlays.last_mut() {
                    palette.at = *at;
                    self.enter();
                }
            }
            Target::ConfirmChoice(at) => {
                if let Some(Overlay::Confirm(confirm)) = self.overlays.last_mut() {
                    confirm.at = *at;
                    self.enter();
                }
            }
            Target::LoginField(at) => {
                if let Some(Overlay::Login(login)) = self.overlays.last_mut() {
                    if *at < 3 {
                        login.at = *at;
                    } else {
                        self.enter();
                    }
                }
            }
            Target::PickerDay(date) => {
                if let Some(Overlay::Picker(picker)) = self.overlays.last_mut() {
                    picker.date = *date;
                    self.enter();
                }
            }
            _ if self.popup_open() => {}
            Target::Drawer(at) => {
                if matches!(self.overlays.last(), Some(Overlay::Drawer(_))) {
                    self.overlays.pop();
                }
                self.overlays.push(Overlay::Drawer(*at));
                self.enter();
            }
            Target::DrawerToggle => self.act(Action::Drawer),
            Target::Title => self.act(Action::Palette),
            Target::Tab(url) if !url.is_empty() => self.follow_url(url),
            Target::Tab(_) => {}
            Target::Focus(key) => self.click_focus(key),
            Target::ListRow { list, row } => {
                if self.focus_row(list, *row) && count >= 2 {
                    self.activate();
                }
            }
            Target::ListSelect { list, row } => {
                if self.focus_row(list, *row) {
                    self.toggle_row();
                }
            }
            Target::RowLink { list, row, link } => {
                if self.focus_row(list, *row) {
                    if link.is_empty() {
                        let url = self.list_page(list).get(*row).and_then(row_link).map(str::to_string);
                        if let Some(url) = url {
                            self.follow_url(&url);
                        }
                    } else {
                        self.follow_link(link);
                    }
                }
            }
            Target::ListSort { list, column } => self.sort_column(list, *column),
            Target::TreeRow { tree, row } => {
                if self.focus_key(tree) {
                    if let Some(state) = self.trees.get_mut(tree) {
                        state.at = *row;
                    }
                    self.tree_enter(tree);
                }
            }
        }
    }

    /// A popup that takes the clicks: a click outside it does nothing. A dialog is not one: its fields and buttons
    /// are the focus scope, and a click outside lands on nothing there.
    fn popup_open(&self) -> bool {
        self.overlays.last().is_some_and(|overlay| {
            !matches!(
                overlay,
                Overlay::Dialog(_) | Overlay::Find(_) | Overlay::Row(..) | Overlay::Drawer(_)
            )
        })
    }

    /// Enter on the top overlay.
    fn enter(&mut self) {
        self.on_key(&KeyPress::plain(Key::Enter));
    }

    /// Focus what was clicked, by its key, or the first part of a field drawn in parts (a date range's from), then do
    /// what a click does on its kind.
    fn click_focus(&mut self, key: &str) {
        let found = self.focus_key(key) || {
            let part = self
                .focus
                .iter()
                .find(|item| item.node == key)
                .map(|item| item.key.clone());
            part.is_some_and(|part| self.focus_key(&part))
        };
        if !found {
            return;
        }
        match self.focus_kind() {
            // A field takes the focus only; a check box toggles as Space does, a drop-down opens its list.
            Some(FocusKind::Text | FocusKind::Radio | FocusKind::Component | FocusKind::List | FocusKind::Tree) => {}
            Some(FocusKind::Check) => {
                self.on_key(&KeyPress::plain(Key::Char(' ')));
            }
            Some(_) => self.activate(),
            None => {}
        }
    }

    /// Make `row` the current row of `list`, focusing the list. False when the list is not in the focus scope.
    fn focus_row(&mut self, list: &str, row: usize) -> bool {
        if !self.focus_key(list) {
            return false;
        }
        self.rows.insert(list.to_string(), row);
        true
    }

    /// A click on a column title: sort by it, ascending, then descending on the next click.
    fn sort_column(&mut self, list: &str, column: usize) {
        if !self.focus_key(list) {
            return;
        }
        let Some(field) = self
            .list(list)
            .and_then(|found| found.columns.get(column))
            .map(|found| found.order_by.clone())
            .filter(|field| !field.is_empty())
        else {
            return;
        };
        let bare = |name: &str| name.trim_start_matches('-').trim_start_matches('^').to_string();
        let current = self.order_by().to_string();
        let next = if bare(&current) == bare(&field) && !current.starts_with('-') {
            format!("-{field}")
        } else {
            field
        };
        let mut params = self.parameters().clone();
        params.insert("orderByField".to_string(), next);
        params.remove("pageIndex");
        self.rows.insert(list.to_string(), 0);
        self.reload_with(params);
    }

    /// The wheel over a list, the drawer or the canvas moves by rows; over an open drop-down list, menu or palette it
    /// moves the highlight, as Up and Down do. True when the session took it; else the view scrolls the canvas.
    pub fn wheel(&mut self, down: bool) -> bool {
        let key = KeyPress::plain(if down { Key::Down } else { Key::Up });
        match self.overlays.last() {
            Some(Overlay::DropDown(_) | Overlay::Menu(_) | Overlay::Palette(_) | Overlay::Drawer(_)) => {
                self.on_key(&key);
                true
            }
            _ => false,
        }
    }
}
