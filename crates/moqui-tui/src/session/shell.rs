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

//! The shell around the screens (plans/archive/Screen-TUI-Impl.md 1.6): the palette and go to, quick create, the actions menu
//! (F10) and a button-menu's items, find text, the tray, the counters, the drawer's choice and saved finds, the help's
//! documents, a list's exports, saved finds and columns. Menus share one overlay: a title and entries, each with what
//! it does.

use super::*;

/// The extra dialog of the tray, and of a quick create's form, a document and an export as text. No `#` in them: that
/// is the separator of a field's parts.
pub const TRAY: &str = "shell-tray";
const QUICK: &str = "shell-quick";
const DOC: &str = "shell-doc";
const TEXT: &str = "shell-text";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuEntry>,
    pub at: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    pub label: String,
    pub act: MenuAct,
}

/// What a menu entry or a palette row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAct {
    /// Follow a link of the screen, by id.
    Link(String),
    /// Open a screen, or an outside address in the browser.
    Url(String),
    /// Open a dialog of the screen: a row action.
    Dialog(String),
    /// Open a form of another screen in a dialog: a quick create.
    Load { title: String, url: String },
    /// An export of a list: its index in the list's exports.
    Export { list: String, at: usize },
    /// The saved finds of a list.
    SavedFinds(String),
    /// Save the list's current find under a name.
    SaveFind(String),
    /// Post to a saved finds transition: make a find the default, or delete it.
    Post {
        url: String,
        fields: BTreeMap<String, String>,
    },
    /// The columns of a list.
    Columns(String),
    /// The find dialog of a list.
    Find(String),
    /// Edit the focused row.
    EditRow,
    /// A key's action.
    Act(Action),
    /// An Assist pane's choice.
    Assist(crate::assist::Act),
}

/// The palette: what is typed, the highlighted row, and whether it is `g`'s go to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub text: String,
    pub at: usize,
    pub go: bool,
}

/// One row of the palette, under its group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteRow {
    pub group: String,
    pub title: String,
    pub subtitle: String,
    pub act: MenuAct,
}

/// A click-to-edit value being edited: the text, and for a select the option chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    pub field: String,
    pub text: String,
    pub cursor: usize,
    pub at: usize,
}

/// The columns of a list being arranged: each field, shown in a column or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Columns {
    pub list: String,
    pub entries: Vec<ColumnEntry>,
    pub at: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnEntry {
    pub name: String,
    pub title: String,
    /// The column it is in, None when it is not shown.
    pub column: Option<usize>,
}

/// A line to type, and what Enter does with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub title: String,
    pub text: String,
    pub then: PromptThen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptThen {
    /// Save the list's find under the typed name.
    SaveFind(String),
    /// The Assist message of several lines (F4 in the pane): Alt+Enter sends it.
    AssistText,
    /// The feedback of an approval rejected: what Assist should do instead.
    AssistReject(String),
    /// The text answer to a question.
    AssistAnswer { run_id: String, question_id: String },
}

impl Session {
    // ---- what the view reads ----

    /// The drawer's rows as `(title, url, depth)`: the app's menu, the screen's saved finds, and Home. A row with no url
    /// is a heading.
    pub fn drawer_rows(&self) -> Vec<(String, String, usize)> {
        let mut rows = drawer_rows(&self.shell.menu);
        if !self.finds.is_empty() {
            rows.push(("Saved finds".to_string(), String::new(), 0));
            rows.extend(self.finds.iter().map(|find| (find.title.clone(), find.url.clone(), 1)));
        }
        if !rows.is_empty() {
            rows.push(("Home".to_string(), "/apps".to_string(), 0));
        }
        rows
    }

    /// The badge counts, by name.
    pub fn counts(&self) -> &BTreeMap<String, i64> {
        &self.counts
    }

    /// The last messages, oldest first.
    pub fn history(&self) -> &VecDeque<Message> {
        &self.history
    }

    /// The screen's documents, title and index.
    pub fn docs(&self) -> &[(String, String)] {
        &self.docs
    }

    /// The dialogs that are not in a screen.
    pub fn extras(&self) -> &[Extra] {
        &self.extras
    }

    pub fn tray_tab(&self) -> usize {
        self.tray_tab
    }

    /// The text `/` looks for.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// Files the server sent, for the loop to save; each is handed out once.
    pub fn take_downloads(&mut self) -> Vec<RawBody> {
        std::mem::take(&mut self.downloads)
    }

    /// The loop saved a download, or could not.
    pub fn saved(&mut self, result: Result<String, String>) {
        match result {
            Ok(path) => self.set_message(format!("Saved {path}"), false),
            Err(err) => self.set_message(format!("Could not save the file: {err}"), true),
        }
    }

    /// The palette's rows for what is typed: the shell's own (apps, recent screens, actions; the menu in go to) first,
    /// then what the server's search found for the same text.
    pub fn palette_rows(&self) -> Vec<PaletteRow> {
        let Some(Overlay::Palette(palette)) = self.overlays.last() else {
            return Vec::new();
        };
        let needle = palette.text.trim().to_lowercase();
        let hit = |text: &str| needle.is_empty() || text.to_lowercase().contains(&needle);
        let row = |group: &str, title: &str, subtitle: &str, act: MenuAct| PaletteRow {
            group: group.to_string(),
            title: title.to_string(),
            subtitle: subtitle.to_string(),
            act,
        };
        let mut rows = Vec::new();
        if palette.go {
            if palette.text.starts_with('/') {
                rows.push(row(
                    "Go to",
                    &palette.text,
                    "",
                    MenuAct::Url(palette.text.trim().to_string()),
                ));
            }
            for (title, url, _) in self.drawer_rows().into_iter().filter(|(_, url, _)| !url.is_empty()) {
                if hit(&title) {
                    rows.push(row("Menu", &title, &url, MenuAct::Url(url.clone())));
                }
            }
            return rows;
        }
        for app in &self.shell.apps {
            if hit(&app.title) {
                rows.push(row("Apps", &app.title, &app.url, MenuAct::Url(app.url.clone())));
            }
        }
        let mut seen = BTreeSet::new();
        for location in self.nav.history().iter().rev() {
            let url = location_text(location);
            if hit(&url) && seen.insert(url.clone()) && seen.len() <= 8 {
                rows.push(row("Recent", &location.path, &url, MenuAct::Url(url.clone())));
            }
        }
        for action in self.shell.actions.iter().chain(&self.shell.quick_create) {
            if hit(&action.label) {
                rows.push(row(
                    "Actions",
                    &action.label,
                    &action.app,
                    MenuAct::Load {
                        title: action.label.clone(),
                        url: action.url.clone(),
                    },
                ));
            }
        }
        if self.palette_found.0 == palette.text {
            rows.extend(self.palette_found.1.iter().cloned());
        }
        rows
    }

    // ---- the palette ----

    pub(super) fn open_palette(&mut self, go: bool) {
        self.overlays.push(Overlay::Palette(Palette {
            go,
            text: if go { "/".to_string() } else { String::new() },
            at: 0,
        }));
    }

    pub(super) fn palette_key(&mut self, mut palette: Palette, press: &KeyPress) {
        self.overlays.pop();
        let rows = {
            self.overlays.push(Overlay::Palette(palette.clone()));
            let rows = self.palette_rows();
            self.overlays.pop();
            rows
        };
        let last = rows.len().saturating_sub(1);
        match &press.key {
            Key::Escape | Key::F(12) => return,
            Key::Up => palette.at = palette.at.saturating_sub(1),
            Key::Down | Key::Tab => palette.at = (palette.at + 1).min(last),
            Key::Enter => {
                if let Some(row) = rows.get(palette.at) {
                    let act = row.act.clone();
                    self.run_menu(act);
                } else if palette.go && !palette.text.trim().is_empty() {
                    self.follow_url(palette.text.trim());
                }
                return;
            }
            Key::Backspace => {
                palette.text.pop();
                palette.at = 0;
                self.palette_typed(&palette);
            }
            Key::Char(ch) if press.is_plain() => {
                palette.text.push(*ch);
                palette.at = 0;
                self.palette_typed(&palette);
            }
            _ => {}
        }
        self.overlays.push(Overlay::Palette(palette));
    }

    /// The server's search waits until the typing stops; go to searches nothing.
    fn palette_typed(&mut self, palette: &Palette) {
        if palette.go || palette.text.trim().chars().count() < 2 {
            self.timers.remove(&Timer::Palette);
            return;
        }
        self.set_timer(Timer::Palette, Instant::now() + Duration::from_millis(SEARCH_MILLIS));
    }

    pub(super) fn request_palette(&mut self) {
        let Some(Overlay::Palette(palette)) = self.overlays.last() else {
            return;
        };
        let text = palette.text.trim().to_string();
        self.palette_found.0 = String::new();
        self.lanes.push(
            Purpose::Palette,
            LinkCall::Json {
                url: "/shellSearch".to_string(),
                method: "get".to_string(),
                parameters: BTreeMap::from([("text".to_string(), text), ("limit".to_string(), "5".to_string())]),
            },
        );
    }

    /// `shellSearch`'s groups of rows, each a screen to open.
    pub(super) fn palette_reply(&mut self, result: Result<serde_json::Value, LinkError>) {
        let value = match result {
            Ok(value) => value,
            Err(err) => {
                self.set_message(format!("The search failed: {err}"), true);
                return;
            }
        };
        let Some(Overlay::Palette(palette)) = self.overlays.last() else {
            return;
        };
        let text = palette.text.clone();
        let mut rows = Vec::new();
        for group in value
            .get("groups")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let label = json_text(group, "label");
            for found in group
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let url = json_text(found, "url");
                if url.is_empty() {
                    continue;
                }
                rows.push(PaletteRow {
                    group: label.clone(),
                    title: json_text(found, "title"),
                    subtitle: json_text(found, "subtitle"),
                    act: MenuAct::Url(url),
                });
            }
        }
        self.palette_found = (text, rows);
    }

    // ---- menus ----

    pub(super) fn open_menu(&mut self, title: &str, items: Vec<MenuEntry>) {
        if items.is_empty() {
            self.set_message(format!("{title}: nothing to choose."), false);
            return;
        }
        self.overlays.push(Overlay::Menu(Menu {
            title: title.to_string(),
            items,
            at: 0,
        }));
    }

    pub(super) fn menu_key(&mut self, mut menu: Menu, press: &KeyPress) {
        self.overlays.pop();
        let last = menu.items.len().saturating_sub(1);
        match &press.key {
            Key::Escape | Key::F(12) | Key::F(10) => return,
            Key::Up => menu.at = menu.at.saturating_sub(1),
            Key::Down | Key::Tab => menu.at = (menu.at + 1).min(last),
            Key::Home => menu.at = 0,
            Key::End => menu.at = last,
            Key::Enter | Key::Right => {
                if let Some(entry) = menu.items.get(menu.at) {
                    let act = entry.act.clone();
                    self.run_menu(act);
                }
                return;
            }
            // The first letter of an entry picks it.
            Key::Char(ch) if press.is_plain() => {
                let found = menu.items.iter().position(|entry| {
                    entry
                        .label
                        .chars()
                        .next()
                        .is_some_and(|first| first.eq_ignore_ascii_case(ch))
                });
                if let Some(at) = found {
                    let act = menu.items[at].act.clone();
                    self.run_menu(act);
                    return;
                }
            }
            _ => {}
        }
        self.overlays.push(Overlay::Menu(menu));
    }

    pub(super) fn run_menu(&mut self, act: MenuAct) {
        match act {
            MenuAct::Link(id) => self.follow_link(&id),
            MenuAct::Url(url) => self.follow_url(&url),
            MenuAct::Dialog(id) => self.open_dialog(&id),
            MenuAct::Load { title, url } => self.open_extra(QUICK, &title, &url),
            MenuAct::Export { list, at } => self.export(&list, at),
            MenuAct::SavedFinds(list) => self.open_saved_finds(&list),
            MenuAct::SaveFind(list) => self.overlays.push(Overlay::Prompt(Prompt {
                title: "Name of the new find".to_string(),
                text: String::new(),
                then: PromptThen::SaveFind(list),
            })),
            MenuAct::Post { url, fields } => self.post_simple(&url, fields),
            MenuAct::Columns(list) => self.open_columns(&list),
            MenuAct::Find(list) => {
                if let Some(at) = self.focus.iter().position(|item| item.key == list) {
                    self.focus_at = at;
                }
                self.open_find();
            }
            MenuAct::EditRow => self.edit_row(),
            MenuAct::Act(action) => self.act(action),
            MenuAct::Assist(act) => self.run_assist_act(act),
        }
    }

    /// A transition with no form: its answer is shown and the screen asked for again.
    pub(super) fn post_simple(&mut self, url: &str, fields: BTreeMap<String, String>) {
        self.submitted = Some(Submitted {
            form: String::new(),
            keys: Vec::new(),
            submit_message: String::new(),
            background_hide_id: String::new(),
            background_reload_id: String::new(),
            then: None,
        });
        self.lanes.push(
            Purpose::Transition,
            LinkCall::Transition {
                url: url.to_string(),
                method: "post".to_string(),
                fields,
            },
        );
    }

    /// A button-menu's items.
    pub(super) fn open_button_menu(&mut self, id: &str) {
        let Some(menu) = self.index.menus.get(id).cloned() else {
            return;
        };
        let items = menu
            .links
            .iter()
            .filter_map(|link| self.index.links.get(link))
            .filter(|link| !link.disabled)
            .map(|link| MenuEntry {
                label: link.text.clone(),
                act: MenuAct::Link(link.id.clone()),
            })
            .collect();
        self.open_menu(if menu.text.is_empty() { "More" } else { &menu.text }, items);
    }

    /// F10: the screen's button menus, then the focused list's row actions and tools.
    pub(super) fn open_actions(&mut self) {
        let mut items = Vec::new();
        for menu in self.index.menus.values() {
            for link in menu.links.iter().filter_map(|link| self.index.links.get(link)) {
                if !link.disabled {
                    items.push(MenuEntry {
                        label: if menu.text.is_empty() {
                            link.text.clone()
                        } else {
                            format!("{} ▸ {}", menu.text, link.text)
                        },
                        act: MenuAct::Link(link.id.clone()),
                    });
                }
            }
        }
        if let Some(list_id) = self.target_list() {
            items.extend(self.list_actions(&list_id));
        }
        self.open_menu("Actions", items);
    }

    /// What can be done with a list: its row actions, the row editor, the find, saved finds, columns and exports.
    pub(super) fn list_actions(&self, list_id: &str) -> Vec<MenuEntry> {
        let mut items = Vec::new();
        let Some(list) = self.list(list_id) else {
            return items;
        };
        for (dialog, owner) in &self.index.dialog_lists {
            if owner == list_id {
                let title = self.index.dialogs.get(dialog).cloned().unwrap_or_default();
                items.push(MenuEntry {
                    label: format!("{title}…"),
                    act: MenuAct::Dialog(dialog.clone()),
                });
            }
        }
        let row = (list_id.to_string(), self.row_at(list_id));
        for dialog in self.index.row_dialogs.get(&row).into_iter().flatten() {
            let title = self.index.dialogs.get(dialog).cloned().unwrap_or_default();
            items.push(MenuEntry {
                label: format!("{title}…"),
                act: MenuAct::Dialog(dialog.clone()),
            });
        }
        if self.index.rows.contains_key(&row) {
            items.push(MenuEntry {
                label: "Edit row".to_string(),
                act: MenuAct::EditRow,
            });
        }
        if !list.header_fields.is_empty() {
            items.push(MenuEntry {
                label: "Find…".to_string(),
                act: MenuAct::Find(list_id.to_string()),
            });
        }
        if list.saved_finds.is_some() {
            items.push(MenuEntry {
                label: "Saved finds…".to_string(),
                act: MenuAct::SavedFinds(list_id.to_string()),
            });
        }
        if list.select_columns.is_some() {
            items.push(MenuEntry {
                label: "Columns…".to_string(),
                act: MenuAct::Columns(list_id.to_string()),
            });
        }
        for (at, export) in list.exports.iter().enumerate() {
            let label = if export.label.is_empty() {
                export.kind.to_uppercase()
            } else {
                export.label.clone()
            };
            items.push(MenuEntry {
                label: format!("Export {label}"),
                act: MenuAct::Export {
                    list: list_id.to_string(),
                    at,
                },
            });
        }
        items
    }

    /// F6: the apps' quick create items, each a form in a dialog.
    pub(super) fn open_quick_create(&mut self) {
        let items = self
            .shell
            .quick_create
            .iter()
            .map(|action| MenuEntry {
                label: action.label.clone(),
                act: MenuAct::Load {
                    title: action.label.clone(),
                    url: action.url.clone(),
                },
            })
            .collect();
        self.open_menu("Create", items);
    }

    // ---- extra dialogs: a quick create, the tray, a document, an export as text ----

    /// A dialog of another screen's render: it opens at once, and its body comes when the server answers.
    pub(super) fn open_extra(&mut self, id: &str, title: &str, url: &str) {
        self.set_extra(id, title, Vec::new());
        self.lanes.push(
            Purpose::Fragment(id.to_string()),
            LinkCall::Fragment {
                url: url.to_string(),
                parameters: BTreeMap::new(),
            },
        );
        self.show_extra(id);
    }

    pub(super) fn set_extra(&mut self, id: &str, title: &str, c: Vec<Node>) {
        self.extras.retain(|extra| extra.id != id);
        self.extras.push(Extra {
            id: id.to_string(),
            title: title.to_string(),
            c,
        });
        self.reindex(true);
    }

    fn show_extra(&mut self, id: &str) {
        self.overlays
            .retain(|overlay| overlay != &Overlay::Dialog(id.to_string()));
        self.overlays.push(Overlay::Dialog(id.to_string()));
        self.rebuild_focus();
        self.focus_first_in_scope();
    }

    /// An extra dialog no longer open goes, with its fields.
    pub(super) fn drop_closed_extras(&mut self) {
        let open: Vec<String> = self.dialogs_open().into_iter().map(str::to_string).collect();
        let before = self.extras.len();
        self.extras.retain(|extra| open.contains(&extra.id));
        if self.extras.len() != before {
            self.reindex(true);
        }
    }

    // ---- the tray ----

    /// Alt+T: the tray's screen of tab `tab`, in a dialog; Left and Right go to the other tabs.
    pub(super) fn open_tray(&mut self, tab: usize) {
        let tray = self.shell.tray.clone();
        let Some((label, url)) = tray.get(tab) else {
            self.set_message("This theme has no tray.".to_string(), false);
            return;
        };
        self.tray_tab = tab;
        let tabs: Vec<String> = tray
            .iter()
            .enumerate()
            .map(|(at, (name, _))| if at == tab { format!("[{name}]") } else { name.clone() })
            .collect();
        let title = format!("{} · {}", label, tabs.join(" "));
        self.open_extra(TRAY, &title, &url.clone());
    }

    pub(super) fn tray_key(&mut self, press: &KeyPress) -> bool {
        let count = self.shell.tray.len().max(1);
        match press.key {
            Key::Right => self.open_tray((self.tray_tab + 1) % count),
            Key::Left => self.open_tray((self.tray_tab + count - 1) % count),
            _ => return false,
        }
        true
    }

    // ---- the counters and the drawer's choice ----

    pub(super) fn read_counts(&mut self) {
        self.lanes.push(
            Purpose::Counts,
            LinkCall::Json {
                url: "/shellCounts".to_string(),
                method: "get".to_string(),
                parameters: BTreeMap::new(),
            },
        );
    }

    /// The counts come back; with a count service, they are read again in a minute.
    pub(super) fn counts_reply(&mut self, result: Result<serde_json::Value, LinkError>) {
        if let Ok(value) = result {
            let counts = crate::link::counts_of(&value);
            if !counts.is_empty() {
                self.counts = counts;
                self.set_timer(Timer::Counts, Instant::now() + self.counts_every());
            }
        }
    }

    /// How long until the counters are read again: a minute, or ten while the link pushes.
    pub(super) fn counts_every(&self) -> Duration {
        Duration::from_secs(if self.pushed {
            PUSHED_COUNTS_SECONDS
        } else {
            COUNTS_SECONDS
        })
    }

    /// Whether the link pushes notifications (the loop subscribed), so the minute's read of the counters can wait.
    pub fn set_pushed(&mut self, pushed: bool) {
        self.pushed = pushed;
    }

    /// A notification came: the counters are read now, and its title goes on the message line unless an error holds it.
    pub fn on_push(&mut self, push: &crate::link::Push) {
        self.read_counts();
        if !push.title.is_empty() && !self.message_is_error() {
            self.set_message(push.title.clone(), false);
        }
        if push.topic == "AgentRun" {
            self.assist_push(push);
        }
    }

    /// The drawer's choice is kept as the user's preference, as qvt keeps `QUASAR_LEFT_OPEN`.
    pub(super) fn keep_drawer(&mut self, open: bool) {
        self.keep_preference("TUI_DRAWER_OPEN", open);
    }

    /// A choice of the user's kept on the server: the drawer, the mouse.
    pub(super) fn keep_preference(&mut self, key: &str, value: bool) {
        self.lanes.push(
            Purpose::Preference,
            LinkCall::Json {
                url: "/apps/setPreference".to_string(),
                method: "post".to_string(),
                parameters: BTreeMap::from([
                    ("preferenceKey".to_string(), key.to_string()),
                    ("preferenceValue".to_string(), value.to_string()),
                ]),
            },
        );
    }

    // ---- the screen's menu data: its documents and saved finds ----

    pub(super) fn request_menu(&mut self) {
        let path = self.path().to_string();
        if path.is_empty() {
            return;
        }
        let url = format!(
            "/menuData{}",
            if path.starts_with('/') {
                path.clone()
            } else {
                format!("/{path}")
            }
        );
        self.lanes.push(
            Purpose::Menu,
            LinkCall::Json {
                url,
                method: "get".to_string(),
                parameters: self.parameters().clone(),
            },
        );
    }

    /// The last entry of `menuData` is the screen: its `screenDocList` for the help, and its `savedFinds` for the drawer.
    pub(super) fn menu_reply(&mut self, result: Result<serde_json::Value, LinkError>) {
        let Ok(value) = result else {
            // The help and drawer work without them.
            return;
        };
        let Some(screen) = value.as_array().and_then(|entries| entries.last()) else {
            return;
        };
        self.docs = screen
            .get("screenDocList")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|doc| (json_text(doc, "title"), json_text(doc, "index")))
            .collect();
        self.finds = screen
            .get("savedFinds")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .map(|find| {
                let title = json_text(find, "title");
                MenuItem {
                    title: if title.is_empty() {
                        json_text(find, "name")
                    } else {
                        title
                    },
                    url: json_text(find, "pathWithParams"),
                    children: Vec::new(),
                }
            })
            .collect();
    }

    /// A document's number in the help: its text, converted on the server, in a dialog.
    pub(super) fn open_doc(&mut self, digit: char) {
        let Some((title, index)) = digit
            .to_digit(10)
            .and_then(|n| usize::try_from(n).ok())
            .and_then(|n| n.checked_sub(1))
            .and_then(|at| self.docs.get(at))
            .cloned()
        else {
            return;
        };
        self.set_extra(DOC, &title, Vec::new());
        self.show_extra(DOC);
        let url = format!("{}/screenDoc", self.path());
        self.lanes.push(
            Purpose::Help,
            LinkCall::Json {
                url,
                method: "get".to_string(),
                parameters: BTreeMap::from([
                    ("docIndex".to_string(), index),
                    ("renderMode".to_string(), "tui".to_string()),
                ]),
            },
        );
    }

    pub(super) fn doc_reply(&mut self, result: Result<serde_json::Value, LinkError>) {
        let title = self
            .extras
            .iter()
            .find(|extra| extra.id == DOC)
            .map(|extra| extra.title.clone())
            .unwrap_or_default();
        let node = match result.map(crate::model::node_of) {
            Ok(Ok(node)) => node,
            Ok(Err(err)) => Node::Error {
                id: "doc".to_string(),
                text: format!("The document did not read: {err}"),
            },
            Err(err) => Node::Error {
                id: "doc".to_string(),
                text: format!("The document did not load: {err}"),
            },
        };
        let mut nodes = vec![node];
        crate::model::prefix_ids(&mut nodes, "doc/");
        self.set_extra(DOC, &title, nodes);
    }

    // ---- find text ----

    pub(super) fn search_key(&mut self, mut text: String, press: &KeyPress) {
        self.overlays.pop();
        match &press.key {
            Key::Escape | Key::F(12) => {}
            Key::Enter => {
                if !text.is_empty() {
                    self.search = text;
                }
                self.find_next(true);
            }
            Key::Backspace => {
                text.pop();
                self.overlays.push(Overlay::Search(text));
            }
            Key::Char(ch) if press.is_plain() => {
                text.push(*ch);
                self.overlays.push(Overlay::Search(text));
            }
            _ => self.overlays.push(Overlay::Search(text)),
        }
    }

    /// The next (or previous) item of the focus order whose text holds the search: a link, a field's title or value,
    /// a list row. A list's matching row becomes its current row.
    pub(super) fn find_next(&mut self, forward: bool) {
        if self.search.is_empty() {
            self.overlays.push(Overlay::Search(String::new()));
            return;
        }
        let needle = self.search.to_lowercase();
        let mut stops: Vec<(usize, Option<usize>)> = Vec::new();
        for (at, item) in self.focus.iter().enumerate() {
            match item.kind {
                FocusKind::List => {
                    for (row, cells) in self.list_page(&item.key).iter().enumerate() {
                        let text: String = cells
                            .cells
                            .iter()
                            .flat_map(crate::model::Cell::parts)
                            .map(|part| format!("{} ", part.text()))
                            .collect();
                        if text.to_lowercase().contains(&needle) {
                            stops.push((at, Some(row)));
                        }
                    }
                }
                _ => {
                    if self.item_text(item).to_lowercase().contains(&needle) {
                        stops.push((at, None));
                    }
                }
            }
        }
        let here = (
            self.focus_at,
            self.focus()
                .filter(|item| item.kind == FocusKind::List)
                .map(|item| self.row_at(&item.key)),
        );
        let found = if forward {
            stops
                .iter()
                .find(|stop| stop.0 > here.0 || (stop.0 == here.0 && stop.1 > here.1))
                .or(stops.first())
        } else {
            stops
                .iter()
                .rev()
                .find(|stop| stop.0 < here.0 || (stop.0 == here.0 && stop.1 < here.1))
                .or(stops.last())
        }
        .copied();
        match found {
            Some((at, row)) => {
                self.focus_at = at;
                if let (Some(row), Some(item)) = (row, self.focus.get(at).cloned()) {
                    self.rows.insert(item.key, row);
                }
                self.set_message(format!("Found \"{}\"", self.search), false);
            }
            None => self.set_message(format!("\"{}\" is not on this screen", self.search), true),
        }
    }

    fn item_text(&self, item: &FocusItem) -> String {
        if let Some(link) = self.index.links.get(&item.key) {
            return link.text.clone();
        }
        if let Some(field) = self.index.fields.get(&item.node) {
            return format!("{} {}", field.title, self.buffer(&item.key));
        }
        if let Some(title) = self.index.dialogs.get(&item.key) {
            return title.clone();
        }
        if let Some(menu) = self.index.menus.get(&item.key) {
            return menu.text.clone();
        }
        item.key.clone()
    }

    // ---- exports ----

    /// An export of a list: a file the loop saves (csv, xlsx, pdf), or the text render shown in a dialog.
    pub(super) fn export(&mut self, list_id: &str, at: usize) {
        let Some(export) = self.list(list_id).and_then(|list| list.exports.get(at)).cloned() else {
            return;
        };
        let location = Location::of_url(&export.url);
        self.lanes.push(
            Purpose::Export,
            LinkCall::Raw {
                url: location.path,
                parameters: location.parameters,
            },
        );
        self.set_message(format!("Exporting {}…", export.kind), false);
    }

    pub(super) fn export_reply(&mut self, result: Result<RawBody, LinkError>) {
        let body = match result {
            Ok(body) => body,
            Err(err) => {
                self.set_message(format!("The export failed: {err}"), true);
                return;
            }
        };
        if body.content_type.starts_with("text/plain") {
            let text = String::from_utf8_lossy(&body.bytes).into_owned();
            let node = Node::Text {
                id: "text/body".to_string(),
                text: crate::model::clean_text(&text),
                style: "pre".to_string(),
            };
            self.set_extra(TEXT, "Export as text", vec![node]);
            self.show_extra(TEXT);
            return;
        }
        self.downloads.push(body);
    }

    // ---- saved finds ----

    pub(super) fn open_saved_finds(&mut self, list_id: &str) {
        let Some(saved) = self.list(list_id).and_then(|list| list.saved_finds.clone()) else {
            return;
        };
        let mut items = Vec::new();
        if !saved.clear_url.is_empty() {
            items.push(MenuEntry {
                label: "Clear the current find".to_string(),
                act: MenuAct::Url(saved.clear_url.clone()),
            });
        }
        for find in &saved.finds {
            let mut label = find.name.clone();
            if find.default {
                label.push_str(" (default)");
            }
            if find.active {
                label = format!("• {label}");
            }
            items.push(MenuEntry {
                label,
                act: MenuAct::Url(find.url.clone()),
            });
        }
        if !saved.parameters.is_empty() {
            items.push(MenuEntry {
                label: "Save the current find…".to_string(),
                act: MenuAct::SaveFind(list_id.to_string()),
            });
        }
        let base = |id: &str| {
            let mut fields = saved.parameters.clone();
            fields.insert("formLocation".to_string(), saved.form_location.clone());
            fields.insert("formListFindId".to_string(), id.to_string());
            fields
        };
        for find in &saved.finds {
            let (button, label) = if find.default {
                ("ClearDefault", format!("Clear default: {}", find.name))
            } else {
                ("MakeDefault", format!("Make default: {}", find.name))
            };
            let mut fields = base(&find.id);
            fields.insert(button.to_string(), "Y".to_string());
            items.push(MenuEntry {
                label,
                act: MenuAct::Post {
                    url: saved.save_url.clone(),
                    fields,
                },
            });
            if find.mine {
                let mut fields = base(&find.id);
                fields.insert("DeleteFind".to_string(), "Y".to_string());
                items.push(MenuEntry {
                    label: format!("Delete: {}", find.name),
                    act: MenuAct::Post {
                        url: saved.save_url.clone(),
                        fields,
                    },
                });
            }
        }
        self.open_menu("Saved finds", items);
    }

    pub(super) fn prompt_key(&mut self, mut prompt: Prompt, press: &KeyPress) {
        self.overlays.pop();
        let PromptThen::SaveFind(list_id) = prompt.then.clone() else {
            self.assist_prompt_key(prompt, press);
            return;
        };
        match &press.key {
            Key::Escape | Key::F(12) => {}
            Key::Enter => {
                let Some(saved) = self.list(&list_id).and_then(|list| list.saved_finds.clone()) else {
                    return;
                };
                if prompt.text.trim().is_empty() {
                    self.set_message("A find needs a name.".to_string(), true);
                    self.overlays.push(Overlay::Prompt(prompt));
                    return;
                }
                let mut fields = saved.parameters.clone();
                fields.insert("formLocation".to_string(), saved.form_location.clone());
                fields.insert("_findDescription".to_string(), prompt.text.trim().to_string());
                self.post_simple(&saved.save_url, fields);
            }
            _ => {
                let mut cursor = prompt.text.chars().count();
                edit::apply(&mut prompt.text, &mut cursor, press, &Limits::default());
                self.overlays.push(Overlay::Prompt(prompt));
            }
        }
    }

    // ---- columns ----

    pub(super) fn open_columns(&mut self, list_id: &str) {
        let Some(select) = self.list(list_id).and_then(|list| list.select_columns.clone()) else {
            return;
        };
        let mut entries = Vec::new();
        for (column, fields) in select.shown.iter().enumerate() {
            for field in fields {
                entries.push(ColumnEntry {
                    name: field.name.clone(),
                    title: field.title.clone(),
                    column: Some(column),
                });
            }
        }
        entries.extend(select.hidden.iter().map(|field| ColumnEntry {
            name: field.name.clone(),
            title: field.title.clone(),
            column: None,
        }));
        self.overlays.push(Overlay::Columns(Columns {
            list: list_id.to_string(),
            entries,
            at: 0,
        }));
    }

    /// Space shows or hides a field; Alt+Up and Alt+Down move it among the columns; Enter saves; `r` puts back the
    /// list's own columns.
    pub(super) fn columns_key(&mut self, mut columns: Columns, press: &KeyPress) {
        self.overlays.pop();
        let last = columns.entries.len().saturating_sub(1);
        match (&press.key, press.alt) {
            (Key::Escape | Key::F(12), _) => return,
            (Key::Up, true) => columns.move_by(-1),
            (Key::Down, true) => columns.move_by(1),
            (Key::Up, false) => columns.at = columns.at.saturating_sub(1),
            (Key::Down | Key::Tab, false) => columns.at = (columns.at + 1).min(last),
            (Key::Char(' '), _) => columns.toggle(),
            (Key::Enter, _) => {
                self.save_columns(&columns, false);
                return;
            }
            (Key::Char('r'), false) => {
                self.save_columns(&columns, true);
                return;
            }
            _ => {}
        }
        self.overlays.push(Overlay::Columns(columns));
    }

    /// The arrangement as qvt's `columnsTree`: the hidden fields, then a column a group.
    fn save_columns(&mut self, columns: &Columns, reset: bool) {
        let Some(select) = self.list(&columns.list).and_then(|list| list.select_columns.clone()) else {
            return;
        };
        let mut fields = select.parameters.clone();
        fields.insert("formLocation".to_string(), select.form_location.clone());
        if reset {
            fields.insert("ResetColumns".to_string(), "ResetColumns".to_string());
        } else {
            fields.insert("SaveColumns".to_string(), "SaveColumns".to_string());
            fields.insert("columnsTree".to_string(), columns.tree().to_string());
        }
        self.post_simple(&select.url, fields);
    }
}

impl Columns {
    /// Show a hidden field in a new last column; hide a shown one.
    fn toggle(&mut self) {
        let next = self
            .entries
            .iter()
            .filter_map(|entry| entry.column)
            .max()
            .map_or(0, |n| n + 1);
        if let Some(entry) = self.entries.get_mut(self.at) {
            entry.column = match entry.column {
                Some(_) => None,
                None => Some(next),
            };
        }
        self.renumber();
    }

    /// Move the highlighted field's column before or after its neighbour's.
    fn move_by(&mut self, delta: isize) {
        let Some(column) = self.entries.get(self.at).and_then(|entry| entry.column) else {
            return;
        };
        let count = self
            .entries
            .iter()
            .filter_map(|entry| entry.column)
            .max()
            .map_or(0, |n| n + 1);
        let Some(other) = column.checked_add_signed(delta).filter(|other| *other < count) else {
            return;
        };
        for entry in &mut self.entries {
            entry.column = match entry.column {
                Some(n) if n == column => Some(other),
                Some(n) if n == other => Some(column),
                kept => kept,
            };
        }
        let name = self.entries[self.at].name.clone();
        self.renumber();
        self.at = self.entries.iter().position(|entry| entry.name == name).unwrap_or(0);
    }

    /// Shown fields first in column order, the columns numbered from 0 with no gaps, then the hidden ones.
    fn renumber(&mut self) {
        let name = self.entries.get(self.at).map(|entry| entry.name.clone());
        self.entries.sort_by_key(|entry| entry.column.unwrap_or(usize::MAX));
        let mut map = BTreeMap::new();
        for entry in &mut self.entries {
            if let Some(column) = entry.column {
                let next = map.len();
                entry.column = Some(*map.entry(column).or_insert(next));
            }
        }
        if let Some(name) = name {
            self.at = self.entries.iter().position(|entry| entry.name == name).unwrap_or(0);
        }
    }

    /// `[{id: "hidden", children}, {id: "column_0", children}, …]`, each child `{id: field name}`.
    pub fn tree(&self) -> serde_json::Value {
        let child = |entry: &ColumnEntry| serde_json::json!({"id": entry.name, "label": entry.title});
        let hidden: Vec<serde_json::Value> = self.entries.iter().filter(|e| e.column.is_none()).map(child).collect();
        let mut tree = vec![serde_json::json!({"id": "hidden", "children": hidden})];
        let count = self
            .entries
            .iter()
            .filter_map(|entry| entry.column)
            .max()
            .map_or(0, |n| n + 1);
        for column in 0..count {
            let children: Vec<serde_json::Value> = self
                .entries
                .iter()
                .filter(|entry| entry.column == Some(column))
                .map(child)
                .collect();
            tree.push(serde_json::json!({"id": format!("column_{column}"), "children": children}));
        }
        serde_json::Value::Array(tree)
    }
}

/// A url of a location, with its parameters.
fn location_text(location: &Location) -> String {
    if location.parameters.is_empty() {
        return location.path.clone();
    }
    let query: Vec<String> = location
        .parameters
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect();
    format!("{}?{}", location.path, query.join("&"))
}

pub(super) fn json_text(value: &serde_json::Value, name: &str) -> String {
    match value.get(name) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}
