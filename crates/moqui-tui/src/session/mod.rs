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

//! The interpreter's state and its key dispatch (D5). It does no I/O: a key queues [`LinkCall`]s, the loop takes them
//! with [`Session::take_requests`], runs them off the UI thread and hands each answer back to [`Session::on_reply`].
//! [`Session::run_now`] does both on this thread, for tests and the headless snapshot.
//!
//! Keys go through the layers of section 3.3, the first that takes one stops it: the confirmation, the login, the help
//! and info boxes, an open drop-down or date picker, the drawer, link numbering, the focused field's editing keys, a
//! list's rows, and last the keymap.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use crate::components::ComponentState;
use crate::edit::{self, Edited, FieldEdit, FieldKeys, Limits};
use crate::forms::{self, Extra, FocusItem, FocusKind, Index, Scope};
use crate::keys::{scheme_keys, Action, Key, KeyMap, KeyPress, KeyScheme, MergeSkip};
use crate::layout::PAGE_ROWS;
use crate::link::{row_link_target, LinkCall, LinkError, MenuItem, RawBody, Reply, ScreenLink, ShellInfo};
use crate::model::{Cell, Choice, FormList, Node, ScreenModel, Widget};
use crate::nav::{level_path, Location, Nav};
use crate::state::{Confirm, Edits, Finished, Lanes, Purpose, Request, Then};

mod assist;
mod dynamic;
mod lists;
mod mouse;
mod overlays;
mod shell;
mod structure;
mod submit;

pub use lists::page_count_of;
use overlays::filtered;
pub use shell::{ColumnEntry, Columns, Editor, Menu, MenuAct, MenuEntry, Palette, PaletteRow, Prompt, PromptThen};
pub use structure::{TreeNode, TreeState};

/// Messages kept for Alt+M (section 1.2).
const HISTORY: usize = 25;

/// The messages of a reply on one line, each once, in order: a transition's message can come again with the screen it
/// redirects to, and the line repeated the summary and its details (ACC-UI-028).
pub(crate) fn distinct_join(messages: &[String]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for message in messages {
        let text = message.trim();
        if !text.is_empty() && !seen.contains(&text) {
            seen.push(text);
        }
    }
    seen.join("; ")
}
/// The counters are read again this often while nothing pushes them (section 1.6).
const COUNTS_SECONDS: u64 = 60;
/// While the link pushes, the counters are still read this often, in case a push was missed.
const PUSHED_COUNTS_SECONDS: u64 = 600;
/// Typing in a server-search drop-down or the palette waits this long before it asks.
const SEARCH_MILLIS: u64 = 300;

/// How long a success message stays on the message line (section 1.7).
/// The most text a copy carries: OSC 52 is limited by terminals, 64 KiB is safe for xterm, kitty and tmux.
const COPY_MAX: usize = 64 * 1024;
const MESSAGE_SECONDS: u64 = 3;

/// An open drop-down list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropDown {
    pub field: String,
    /// The highlighted entry, among the filtered ones.
    pub at: usize,
    /// Typed text that narrows the list.
    pub filter: String,
}

/// The date picker of a date field (F4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub key: String,
    pub date: jiff::civil::Date,
}

/// The login dialog after a 401 (section 1.7). While the server has no accounts yet it is in setup, and creates the
/// first administrator before it logs in (docs/ui/TUI.md, "The login dialog").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    pub password: String,
    pub code: String,
    /// The field: 0 username, 1 password, 2 code; in setup 0 username, 1 password, 2 password again, 3 full name,
    /// 4 email, and 5 the setup secret when the setup asks for it.
    pub at: usize,
    pub error: String,
    /// The password was right and a code is wanted: the password is kept for the next try, and these factors take a
    /// sent code (Alt+S sends to the first).
    pub code_wanted: bool,
    pub sendable: Vec<crate::link::Sendable>,
    /// The server has no accounts: the dialog creates the first administrator, under these password rules.
    pub setup: Option<crate::link::AdminSetup>,
    /// The setup's other fields: the password again, the full name and the email.
    pub verify: String,
    pub full_name: String,
    pub email: String,
    pub setup_secret: String,
}

/// The setup's field labels, by field.
pub const SETUP_FIELDS: [&str; 5] = ["Username", "Password", "Password again", "Full name", "Email"];
/// The setup's field labels when it takes the setup secret of the server's startup log (from another machine).
pub const SETUP_SECRET_FIELDS: [&str; 6] = ["Username", "Password", "Password again", "Full name", "Email", "Setup secret"];
/// The login's field labels, by field.
pub const LOGIN_FIELDS: [&str; 3] = ["Username", "Password", "Code"];

impl Login {
    /// The labels of the fields the dialog has now.
    pub fn labels(&self) -> &'static [&'static str] {
        match &self.setup {
            Some(setup) if setup.secret_required => &SETUP_SECRET_FIELDS,
            Some(_) => &SETUP_FIELDS,
            None => &LOGIN_FIELDS,
        }
    }

    /// Whether field `at` is masked.
    pub fn masked(&self, at: usize) -> bool {
        at == 1 || (self.setup.is_some() && (at == 2 || at == 5))
    }

    /// The text of field `at` (the last field past the end).
    pub fn field(&self, at: usize) -> &str {
        match (self.setup.is_some(), at) {
            (_, 0) => &self.username,
            (_, 1) => &self.password,
            (false, _) => &self.code,
            (true, 2) => &self.verify,
            (true, 3) => &self.full_name,
            (true, 4) => &self.email,
            (true, _) => &self.setup_secret,
        }
    }

    pub fn field_mut(&mut self, at: usize) -> &mut String {
        match (self.setup.is_some(), at) {
            (_, 0) => &mut self.username,
            (_, 1) => &mut self.password,
            (false, _) => &mut self.code,
            (true, 2) => &mut self.verify,
            (true, 3) => &mut self.full_name,
            (true, 4) => &mut self.email,
            (true, _) => &mut self.setup_secret,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Overlay {
    Dialog(String),
    /// The find dialog of a list: its header fields.
    Find(String),
    DropDown(DropDown),
    Picker(Picker),
    Confirm(Confirm),
    Help,
    Info,
    Login(Login),
    /// The drawer has focus, at this row.
    Drawer(usize),
    /// A list of choices: a button-menu, F10's actions, quick create, saved finds, exports.
    Menu(Menu),
    /// Ctrl+K: screens, records, actions and apps by what is typed; `g` goes to a path.
    Palette(Palette),
    /// `/`: the text to find on the screen, as it is typed.
    Search(String),
    /// Alt+M: the last messages.
    Messages,
    /// The editor of a click-to-edit value.
    Editor(Editor),
    /// The fields of row `n` of a list.
    Row(String, usize),
    /// The columns of a list, to show, hide and order.
    Columns(Columns),
    /// A line to type, as a new saved find's name.
    Prompt(Prompt),
}

/// Something the session does at a time rather than on a key (section 3.5): the loop asks [`Session::next_deadline`]
/// when to wake, and calls [`Session::on_time`] then.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Timer {
    /// Read the title line's counters again.
    Counts,
    /// Ask for a server-search drop-down's options, once the typing has stopped.
    Suggest(String),
    /// Ask the palette's search, once the typing has stopped.
    Palette,
    /// Read the Assist run's events again.
    Assist,
    /// An explicitly active live component.
    Component(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub text: String,
    pub error: bool,
    until: Option<Instant>,
}

/// What a level request was for, so its answer is applied the right way.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Intent {
    /// A new screen: the edits go, and with `history` the screen left is remembered.
    Navigate { history: bool },
    /// The same screen again (paging, sort, refresh, after a save): edits that are still dirty stay.
    Reload,
}

/// The form a transition was posted from, to apply its answer.
#[derive(Clone, Debug)]
struct Submitted {
    form: String,
    keys: Vec<String>,
    submit_message: String,
    background_hide_id: String,
    background_reload_id: String,
    then: Option<Then>,
}

#[derive(Clone, Debug)]
pub struct Session {
    nav: Nav,
    index: Index,
    edits: Edits,
    cursors: BTreeMap<String, usize>,
    /// A text field's selection and undo log, by field key.
    field_edits: BTreeMap<String, FieldEdit>,
    focus: Vec<FocusItem>,
    focus_at: usize,
    /// The focus key kept over a rebuild that had nothing to focus (a screen whose content comes in a later level),
    /// so the next rebuild comes back to it.
    focus_kept: String,
    overlays: Vec<Overlay>,
    rows: BTreeMap<String, usize>,
    selected: BTreeMap<String, BTreeSet<String>>,
    hscroll: BTreeMap<String, usize>,
    message: Message,
    lanes: Lanes,
    intents: BTreeMap<u64, (Intent, BTreeMap<String, String>)>,
    submitted: Option<Submitted>,
    /// A save's message, shown when the screen asked for after it arrives.
    after_level: Option<String>,
    keys: KeyMap,
    key_skips: Vec<MergeSkip>,
    /// The key scheme in use, and the one `--keys` (or an agent) asked for, which wins over the shell's.
    scheme: KeyScheme,
    scheme_override: Option<KeyScheme>,
    shell: ShellInfo,
    numbering: bool,
    /// The link pushes notifications, so the counters are read on a push rather than each minute.
    pushed: bool,
    /// The view each list with views shows (`v`), by list id, once the user chose one.
    views: BTreeMap<String, String>,
    /// A list put aside while its board or calendar shows, by list id.
    view_lists: BTreeMap<String, Node>,
    typed_number: String,
    drawer: Option<bool>,
    /// The mouse (Alt+P, `TUI_MOUSE`): None until chosen or read from the shell, and on then. `mouse_told` is the
    /// state the loop last wrote to the terminal.
    mouse: Option<bool>,
    mouse_told: Option<bool>,
    /// Text a drag selected, for the loop to put on the clipboard (OSC 52); and whether a copy was made before, for
    /// the fallback hint the first one gives.
    copy: Option<String>,
    copied_before: bool,
    /// What Ctrl+C or Ctrl+X took last, in a field, a list or the code editor, for Ctrl+V in a text field. The
    /// terminal's own paste (bracketed) brings the system clipboard in.
    clip: String,
    /// The screen asked for last, asked for again after a login.
    last_location: Option<Location>,
    quit: bool,
    open: Option<String>,
    redraw: bool,
    log_open: bool,
    /// The log pane is offered: always in local mode, over SSH to a user who may view the System app.
    log_offered: bool,
    /// The username the login dialog starts with when the shell has no user yet.
    login_name: Option<String>,
    page_rows: Option<usize>,
    mobile: bool,
    timers: BTreeMap<Timer, Instant>,
    /// The terminal's columns, for a component's wide layout.
    columns: u16,
    /// The terminal's rows, for a component that loads what fits on screen ([`ComponentState::rows`]).
    terminal_rows: u16,
    /// Dialogs that are not in a screen: a quick create's form, the tray, a document, an export as text.
    extras: Vec<Extra>,
    /// The tray's tab while the tray shows.
    tray_tab: usize,
    /// Boxes the user opened or closed.
    collapsed: BTreeMap<String, bool>,
    trees: BTreeMap<String, TreeState>,
    components: BTreeMap<String, ComponentState>,
    /// A display's text from its value url, by field id.
    shown_text: BTreeMap<String, String>,
    /// The bodies of dynamic dialogs and containers by node id, kept so a reload of the screen does not blank them.
    fragments: BTreeMap<String, Vec<Node>>,
    /// The last messages, newest last.
    history: VecDeque<Message>,
    /// The text `/` looks for and `n` and `N` go on with.
    search: String,
    /// The palette search's answer: the text it was for, and its rows.
    palette_found: (String, Vec<PaletteRow>),
    /// The screen's documents, title and index, for the help.
    docs: Vec<(String, String)>,
    /// The screen's saved finds, for the drawer.
    finds: Vec<MenuItem>,
    /// The levels of the screen's path below its app's top-level screens, from its menu data: each screen on the path
    /// (by its path) with its subscreens, nested under it in the drawer as the browser's drawer does.
    deep_menu: Vec<MenuLevel>,
    /// The screen's declared actions that open a screen (`extActions` of its menu data, screen/ext.rs), for the drawer.
    ext_actions: Vec<ExtAction>,
    counts: BTreeMap<String, i64>,
    /// Files the server sent (exports), for the loop to save.
    downloads: Vec<RawBody>,
    /// Markdown text areas showing their preview (F4).
    previews: BTreeSet<String>,
    /// The first line a text area shows, by field, kept so the cursor stays in its rows; and a preview's first row.
    area_tops: BTreeMap<String, usize>,
    preview_tops: BTreeMap<String, usize>,
    /// The Assist pane (F2).
    assist: crate::assist::Assist,
}

impl Session {
    pub fn new(model: ScreenModel) -> Self {
        let mut session = Self {
            nav: Nav::new(model),
            index: Index::default(),
            edits: Edits::default(),
            cursors: BTreeMap::new(),
            field_edits: BTreeMap::new(),
            focus: Vec::new(),
            focus_at: 0,
            focus_kept: String::new(),
            overlays: Vec::new(),
            rows: BTreeMap::new(),
            selected: BTreeMap::new(),
            hscroll: BTreeMap::new(),
            message: Message::default(),
            lanes: Lanes::default(),
            intents: BTreeMap::new(),
            submitted: None,
            after_level: None,
            keys: KeyMap::for_scheme(KeyScheme::default()),
            key_skips: Vec::new(),
            scheme: KeyScheme::default(),
            scheme_override: None,
            shell: ShellInfo::default(),
            numbering: false,
            pushed: false,
            views: BTreeMap::new(),
            view_lists: BTreeMap::new(),
            typed_number: String::new(),
            drawer: None,
            mouse: None,
            mouse_told: None,
            copy: None,
            copied_before: false,
            clip: String::new(),
            last_location: None,
            quit: false,
            open: None,
            redraw: false,
            log_open: false,
            log_offered: true,
            login_name: None,
            page_rows: None,
            mobile: false,
            timers: BTreeMap::new(),
            columns: 0,
            terminal_rows: 0,
            extras: Vec::new(),
            tray_tab: 0,
            collapsed: BTreeMap::new(),
            trees: BTreeMap::new(),
            components: BTreeMap::new(),
            shown_text: BTreeMap::new(),
            fragments: BTreeMap::new(),
            history: VecDeque::new(),
            search: String::new(),
            palette_found: (String::new(), Vec::new()),
            docs: Vec::new(),
            finds: Vec::new(),
            deep_menu: Vec::new(),
            ext_actions: Vec::new(),
            counts: BTreeMap::new(),
            downloads: Vec::new(),
            previews: BTreeSet::new(),
            area_tops: BTreeMap::new(),
            preview_tops: BTreeMap::new(),
            assist: crate::assist::Assist::default(),
        };
        session.reindex(false);
        session.show_model_messages();
        session
    }

    /// A session that starts by asking for the shell data and the screen at `path`.
    pub fn start(path: &str) -> Self {
        let mut session = Self::new(ScreenModel {
            v: crate::model::MODEL_VERSION,
            ..ScreenModel::default()
        });
        session.request_shell(path);
        session.navigate(path, false);
        session
    }

    // ---- the loop's side ----

    /// The calls to run now.
    pub fn take_requests(&mut self) -> Vec<Request> {
        self.lanes.take()
    }

    /// The answer to request `id`.
    pub fn on_reply(&mut self, id: u64, reply: Reply) {
        let purpose = match self.lanes.finish(id) {
            Finished::Apply(purpose) => purpose,
            Finished::Superseded | Finished::Unknown => {
                self.intents.remove(&id);
                return;
            }
        };
        if let Some(LinkError::Auth(_)) = reply.error() {
            self.intents.remove(&id);
            if !matches!(purpose, Purpose::Login | Purpose::AdminSetup) {
                self.ask_login();
            }
            return;
        }
        match (purpose, reply) {
            (Purpose::Level, Reply::Level(result)) => self.level_reply(id, result),
            (Purpose::Level, Reply::Raw(result)) => {
                self.intents.remove(&id);
                match result {
                    Ok(body) => self.downloads.push(body),
                    Err(err) => self.set_message(format!("The download failed: {err}"), true),
                }
            }
            (Purpose::Parent(_), Reply::Level(result)) => {
                let parameters = self.intents.remove(&id).map(|(_, params)| params).unwrap_or_default();
                // A parent that does not load leaves the leaf as it is: the header is extra.
                if let Ok(model) = result {
                    self.nav.set_parent(model, parameters);
                    self.reindex(true);
                }
            }
            (Purpose::Transition, Reply::Transition(result)) => self.transition_reply(result),
            // A form whose transition sent a file: the loop saves it, and the screen and its edits stay.
            (Purpose::Transition, Reply::Raw(result)) => {
                self.submitted = None;
                match result {
                    Ok(body) => self.downloads.push(body),
                    Err(err) => self.set_message(format!("The download failed: {err}"), true),
                }
            }
            (Purpose::Options(field), Reply::Options(result)) => match result {
                Ok(choices) => self.apply_choices(&field, choices),
                Err(err) => self.set_message(format!("Could not load the choices: {err}"), true),
            },
            (Purpose::Shell, Reply::Shell(result)) => match result {
                Ok(shell) => self.apply_shell(shell),
                // The screens work without the drawer.
                Err(err) => self.set_message(format!("Could not load the menu: {err}"), true),
            },
            (Purpose::Login, Reply::Login(result)) => self.login_reply(result),
            (Purpose::Login, Reply::SendCode(result)) => self.code_sent(result),
            (Purpose::Login, Reply::CreateInitialAdmin(result)) => self.admin_created(result),
            (Purpose::AdminSetup, Reply::InitialAdmin(result)) => self.admin_setup_reply(result),
            (Purpose::Fragment(id), Reply::Fragment(result)) => self.fragment_reply(&id, result),
            (Purpose::ValueUrl(field), Reply::Json(result)) => self.value_reply(&field, result),
            (Purpose::Palette, Reply::Json(result)) => self.palette_reply(result),
            (Purpose::Counts, Reply::Json(result)) => self.counts_reply(result),
            (Purpose::Tree(key), Reply::Json(result)) => self.tree_reply(&key, result),
            (Purpose::Component(id), Reply::Json(result)) => self.component_reply(&id, result),
            (Purpose::Menu, Reply::Json(result)) => self.menu_reply(result),
            (Purpose::Help, Reply::Json(result)) => self.doc_reply(result),
            (Purpose::Export, Reply::Raw(result)) => self.export_reply(result),
            (Purpose::ReportExport, Reply::Json(result)) => self.report_export_reply(result),
            (Purpose::Assist(lane), Reply::Json(result)) => self.assist_reply(&lane, result),
            (Purpose::Preference, Reply::Json(Err(err))) => {
                self.set_message(format!("Could not keep the choice: {err}"), true);
            }
            _ => {}
        }
    }

    /// Run every queued call on this thread until none is left.
    pub fn run_now(&mut self, link: &dyn ScreenLink) {
        for _ in 0..64 {
            let requests = self.take_requests();
            if requests.is_empty() {
                return;
            }
            for request in requests {
                let reply = link.call(&request.call);
                self.on_reply(request.id, reply);
            }
        }
    }

    /// One key, then the calls it queued, on this thread. Returns true when the session should end.
    pub fn press(&mut self, press: &KeyPress, link: &dyn ScreenLink) -> bool {
        let quit = self.on_key(press);
        self.run_now(link);
        quit
    }

    /// The terminal size changed. The page size of a list follows the rows (10 to 100), and below 80 columns the
    /// mobile column set is asked for (section 1.3). A change reloads the screen.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.columns = columns;
        self.terminal_rows = rows;
        for state in self.components.values_mut() {
            state.rows = rows;
        }
        let page_rows = usize::from(rows).saturating_sub(14).clamp(PAGE_ROWS, 100);
        let mobile = columns < 80;
        let changed = self.page_rows != Some(page_rows) || self.mobile != mobile;
        let first = self.page_rows.is_none();
        self.page_rows = Some(page_rows);
        self.mobile = mobile;
        if changed && !first && self.has_paged_list() {
            self.reload();
        }
    }

    // ---- what the view reads ----

    pub fn nav(&self) -> &Nav {
        &self.nav
    }

    /// The leaf level.
    pub fn model(&self) -> &ScreenModel {
        self.nav.leaf()
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn path(&self) -> &str {
        &self.model().screen.path
    }

    pub fn paths(&self) -> Vec<&str> {
        self.nav.paths()
    }

    pub fn parameters(&self) -> &BTreeMap<String, String> {
        self.nav.leaf_parameters()
    }

    pub fn order_by(&self) -> &str {
        self.parameters().get("orderByField").map(String::as_str).unwrap_or("")
    }

    pub fn focus(&self) -> Option<&FocusItem> {
        self.focus.get(self.focus_at)
    }

    /// The focused item's key: a node id, or `field#part`.
    pub fn focus_id(&self) -> &str {
        self.focus().map(|item| item.key.as_str()).unwrap_or("")
    }

    pub fn focus_kind(&self) -> Option<FocusKind> {
        self.focus().map(|item| item.kind)
    }

    pub fn focus_order(&self) -> &[FocusItem] {
        &self.focus
    }

    /// Move the focus to the item with this key, in the current scope. False when it is not there.
    pub fn focus_key(&mut self, key: &str) -> bool {
        match self.focus.iter().position(|item| item.key == key) {
            Some(at) => {
                self.focus_at = at;
                true
            }
            None => false,
        }
    }

    /// The screen's place, `Gallery ▸ Detail`, each level by its menu title (`Bills of Materials ▸ Find BOM`, not
    /// `BillsOfMaterials ▸ FindBom`): the title line's breadcrumb before it is cut to the width. A server that sends no
    /// titles gives the names, with the leaf's title in place of its name. A level named as the one after it (a
    /// screen whose default subscreen is its own list, `Suppliers ▸ Suppliers`) is one crumb, as in the browser.
    pub fn crumbs(&self) -> Vec<String> {
        let mut crumbs = self.level_names();
        crumbs.dedup();
        crumbs
    }

    fn level_names(&self) -> Vec<String> {
        let screen = &self.model().screen;
        if !screen.path_titles.is_empty() && screen.path_titles.len() == screen.path_list.len() {
            return screen.path_titles.iter().skip(1).cloned().collect();
        }
        let mut names: Vec<String> = screen.path_list.iter().skip(1).cloned().collect();
        if names.is_empty() {
            names = self
                .path()
                .split('/')
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
        }
        let title = &self.model().screen.title;
        if !title.is_empty() {
            names.pop();
            names.push(title.clone());
        }
        names
    }

    pub fn focused_is_text(&self) -> bool {
        self.focus_kind() == Some(FocusKind::Text)
    }

    pub fn buffer(&self, key: &str) -> &str {
        self.edits.get(key)
    }

    /// A text field's selection (Shift with the arrows, Ctrl+A): its start and end, for drawing.
    pub fn selection(&self, key: &str) -> Option<(usize, usize)> {
        self.field_edits.get(key)?.selection(self.cursor(key))
    }

    pub fn cursor(&self, key: &str) -> usize {
        self.cursors
            .get(key)
            .copied()
            .unwrap_or_else(|| self.buffer(key).chars().count())
    }

    /// Set a field's value as if typed.
    pub fn set_buffer(&mut self, key: &str, value: String) {
        let count = value.chars().count();
        self.edits.set(key, value);
        self.cursors.insert(key.to_string(), count);
        if let Some(state) = self.field_edits.get_mut(key) {
            state.anchor = None;
        }
        self.after_edit(key);
    }

    pub fn is_dirty(&self, key: &str) -> bool {
        self.edits.is_dirty(key)
    }

    /// The value the screen came with for a key, before any edit.
    pub fn original(&self, key: &str) -> &str {
        self.edits.original(key)
    }

    /// A form has an edit that is not saved: its primary button is drawn strong.
    pub fn form_dirty(&self, form_id: &str) -> bool {
        self.index.forms.get(form_id).is_some_and(|form| {
            form.fields.iter().any(|id| {
                self.index
                    .fields
                    .get(id)
                    .is_some_and(|field| forms::field_keys(field).iter().any(|key| self.edits.is_dirty(key)))
            })
        })
    }

    pub fn any_dirty(&self) -> bool {
        self.edits.any_dirty()
    }

    pub fn mark(&self, key: &str) -> Option<&str> {
        self.edits.mark(key)
    }

    pub fn marks(&self) -> &BTreeMap<String, String> {
        self.edits.marks()
    }

    /// The message line, empty once a success message has had its 3 seconds.
    pub fn message(&self) -> &str {
        if self.message_expired() {
            ""
        } else {
            &self.message.text
        }
    }

    pub fn message_is_error(&self) -> bool {
        self.message.error && !self.message.text.is_empty()
    }

    fn message_expired(&self) -> bool {
        self.message.until.is_some_and(|until| Instant::now() >= until)
    }

    /// When the loop has something timed to do: the success message's expiry, the counters' next read, a field's
    /// suggestions.
    pub fn next_deadline(&self) -> Option<Instant> {
        let message = self.message.until.filter(|_| !self.message_expired());
        message.into_iter().chain(self.timers.values().copied()).min()
    }

    /// The time is `now`: run the timers that are due. Returns true when the screen changed.
    pub fn on_time(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.message.until.is_some_and(|until| now >= until) && !self.message.text.is_empty() {
            self.clear_message();
            changed = true;
        }
        let due: Vec<Timer> = self
            .timers
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(timer, _)| timer.clone())
            .collect();
        for timer in due {
            self.timers.remove(&timer);
            self.fire(timer);
            changed = true;
        }
        changed
    }

    /// Fire the waits for typing to stop (a server search's options, the palette's search, a code editor's check) now
    /// rather than when they are due: the agent mode answers once the screen has settled. The counters and the
    /// components' polls are left to their time. Returns true when one fired.
    pub fn fire_waits(&mut self) -> bool {
        let waits: Vec<Timer> = self
            .timers
            .keys()
            .filter(|timer| match timer {
                Timer::Counts | Timer::Assist => false,
                Timer::Component(id) => self.component_tick_is_wait(id),
                _ => true,
            })
            .cloned()
            .collect();
        for timer in &waits {
            self.timers.remove(timer);
            self.fire(timer.clone());
        }
        !waits.is_empty()
    }

    fn fire(&mut self, timer: Timer) {
        match timer {
            Timer::Counts => self.read_counts(),
            Timer::Suggest(field) => self.request_suggestions(&field),
            Timer::Palette => self.request_palette(),
            Timer::Assist => self.assist_poll(false),
            Timer::Component(id) => {
                if self.component_poll_enabled(&id) {
                    self.component_tick(&id);
                }
            }
        }
    }

    /// Run `timer` at `at`, in place of an earlier setting of it.
    fn set_timer(&mut self, timer: Timer, at: Instant) {
        self.timers.insert(timer, at);
    }

    pub fn timer(&self, timer: &Timer) -> Option<Instant> {
        self.timers.get(timer).copied()
    }

    pub fn is_busy(&self) -> bool {
        self.lanes.any_busy()
    }

    /// Requests in flight and waiting.
    pub fn request_counts(&self) -> (usize, usize) {
        self.lanes.counts()
    }

    pub fn overlays(&self) -> &[Overlay] {
        &self.overlays
    }

    pub fn top_overlay(&self) -> Option<&Overlay> {
        self.overlays.last()
    }

    /// Write field `at` of the login dialog on top ([`Login::labels`]) and move to it, as typing there does; false with
    /// no login dialog on top.
    pub fn set_login_field(&mut self, at: usize, value: &str) -> bool {
        let Some(Overlay::Login(login)) = self.overlays.last_mut() else {
            return false;
        };
        let at = at.min(login.labels().len() - 1);
        *login.field_mut(at) = value.to_string();
        login.at = at;
        true
    }

    pub fn dialogs_open(&self) -> Vec<&str> {
        self.overlays
            .iter()
            .filter_map(|overlay| match overlay {
                Overlay::Dialog(id) => Some(id.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn find_open(&self) -> Option<&str> {
        self.overlays.iter().rev().find_map(|overlay| match overlay {
            Overlay::Find(id) => Some(id.as_str()),
            _ => None,
        })
    }

    pub fn dropdown(&self) -> Option<&DropDown> {
        match self.top_overlay() {
            Some(Overlay::DropDown(drop)) => Some(drop),
            _ => None,
        }
    }

    pub fn confirm(&self) -> Option<&Confirm> {
        self.overlays.iter().rev().find_map(|overlay| match overlay {
            Overlay::Confirm(confirm) => Some(confirm),
            _ => None,
        })
    }

    pub fn confirm_choice(&self) -> Option<&str> {
        self.confirm().map(Confirm::choice)
    }

    /// The open drop-down searches the server and has fewer characters typed than it needs to ask: an empty list there
    /// is waiting for text, not without matches.
    pub fn dropdown_waits_for_text(&self) -> bool {
        self.dropdown().is_some_and(|drop| {
            self.is_search(&drop.field) && drop.filter.chars().count() < self.search_min_length(&drop.field)
        })
    }

    /// The choices of the open drop-down, narrowed by what was typed.
    pub fn dropdown_choices(&self) -> Vec<Choice> {
        let Some(drop) = self.dropdown() else {
            return Vec::new();
        };
        self.drop_choices(&drop.field, &drop.filter)
    }

    /// A drop-down's choices for what is typed: narrowed here, or as the server's search answered.
    fn drop_choices(&self, field: &str, filter: &str) -> Vec<Choice> {
        if self.is_search(field) {
            self.index.choices(field).to_vec()
        } else {
            filtered(self.index.choices(field), filter)
        }
    }

    pub fn row_at(&self, list_id: &str) -> usize {
        self.rows.get(list_id).copied().unwrap_or(0)
    }

    pub fn selected(&self, list_id: &str) -> Option<&BTreeSet<String>> {
        self.selected.get(list_id)
    }

    pub fn hscroll(&self, list_id: &str) -> usize {
        self.hscroll.get(list_id).copied().unwrap_or(0)
    }

    pub fn list(&self, list_id: &str) -> Option<&FormList> {
        self.index.lists.get(list_id)
    }

    pub fn page_index(&self, list_id: &str) -> usize {
        self.list(list_id)
            .map_or(0, |list| usize::try_from(list.paging.index).unwrap_or(0))
    }

    pub fn page_count(&self, list_id: &str) -> usize {
        self.list(list_id).map_or(1, page_count_of)
    }

    pub fn shell(&self) -> &ShellInfo {
        &self.shell
    }

    pub fn menu(&self) -> &[MenuItem] {
        &self.shell.menu
    }

    pub fn set_menu(&mut self, menu: Vec<MenuItem>) {
        self.shell.menu = menu;
    }

    pub fn keys(&self) -> &KeyMap {
        &self.keys
    }

    /// The key scheme in use.
    pub fn key_scheme(&self) -> KeyScheme {
        self.scheme
    }

    /// Use `scheme` whatever the user's `TUI_KEYS` says (`--keys`, an agent's Classic); None goes back to the
    /// user's. Nothing is saved.
    pub fn set_key_scheme_override(&mut self, scheme: Option<KeyScheme>) {
        self.scheme_override = scheme;
        self.rebuild_keys();
    }

    /// Modern's `j` and `k` are Down and Up in a list's or a tree's rows, as in vi; past the ends the keymap moves the
    /// focus on, as for the arrows.
    pub(super) fn vi_rows(&self, press: &KeyPress) -> KeyPress {
        match press.key {
            Key::Char('j') if press.is_plain() && self.scheme == KeyScheme::Modern => KeyPress::plain(Key::Down),
            Key::Char('k') if press.is_plain() && self.scheme == KeyScheme::Modern => KeyPress::plain(Key::Up),
            _ => press.clone(),
        }
    }

    /// The keymap of the scheme in effect, `--keys`'s, else the user's, else Modern, with the theme's rows over it.
    fn rebuild_keys(&mut self) {
        self.scheme = self.scheme_override.or(self.shell.key_scheme).unwrap_or_default();
        let overrides: Vec<(&str, &str)> = self.shell.keys.iter().map(|(a, c)| (a.as_str(), c.as_str())).collect();
        let (keys, skips) = scheme_keys(self.scheme, &overrides);
        self.keys = keys;
        self.key_skips = skips;
    }

    /// The palette's Keys row: use `scheme` from now on, and keep it as the user's `TUI_KEYS`.
    pub(super) fn choose_key_scheme(&mut self, scheme: KeyScheme) {
        self.shell.key_scheme = Some(scheme);
        self.scheme_override = None;
        self.rebuild_keys();
        self.keep_preference("TUI_KEYS", scheme.name());
        let palette = self
            .keys
            .chords(Action::Palette)
            .first()
            .map_or_else(|| "Ctrl+K".to_string(), |press| press.label());
        self.set_message(
            format!("Keys: {}. {palette}, then Keys, to change.", scheme.title()),
            false,
        );
    }

    /// Theme key rows that were not applied, for the log.
    pub fn key_skips(&self) -> &[MergeSkip] {
        &self.key_skips
    }

    pub fn numbering(&self) -> bool {
        self.numbering
    }

    /// The number `#` shows before a link, from 1.
    pub fn link_number(&self, link_id: &str) -> Option<usize> {
        self.numbering
            .then(|| {
                self.index
                    .link_order
                    .iter()
                    .position(|id| id == link_id)
                    .map(|at| at + 1)
            })
            .flatten()
    }

    pub fn typed_number(&self) -> &str {
        &self.typed_number
    }

    /// Whether the user asked for the drawer shown (F9); None follows the width.
    pub fn drawer_choice(&self) -> Option<bool> {
        self.drawer
    }

    pub fn drawer_focus(&self) -> Option<usize> {
        self.overlays.iter().find_map(|overlay| match overlay {
            Overlay::Drawer(at) => Some(*at),
            _ => None,
        })
    }

    /// A url the user asked to open in the browser (`o`), once.
    pub fn take_open(&mut self) -> Option<String> {
        self.open.take()
    }

    /// Ctrl+L asked for a full redraw, once.
    pub fn take_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    /// Alt+L showed the log pane.
    /// Whether Alt+L opens the log pane here; a session loop without a log says no.
    pub fn set_log_offered(&mut self, offered: bool) {
        self.log_offered = offered;
        self.log_open &= offered;
    }

    /// The username the login dialog starts with while no one is logged in.
    pub fn set_login_name(&mut self, name: Option<String>) {
        self.login_name = name.filter(|name| !name.is_empty());
    }

    pub fn log_open(&self) -> bool {
        self.log_open
    }

    pub fn quitting(&self) -> bool {
        self.quit
    }

    /// The focused field's widget.
    pub fn field_widget(&self, id: &str) -> Option<&Widget> {
        let node = id.split('#').next().unwrap_or(id);
        self.index.fields.get(node).map(|field| &field.widget)
    }

    /// Whether a choice field holds several values: a check group (more than one option) or a multiple drop-down. Its picker
    /// toggles with Space and closes with Enter, and its buffer is the values separated by commas.
    pub fn is_multi_choice(&self, id: &str) -> bool {
        match self.field_widget(id) {
            Some(Widget::Check { options, .. }) => options.len() > 1,
            Some(Widget::DropDown { multiple, .. }) => *multiple,
            _ => false,
        }
    }

    /// An `editor-type="md"` text area: its source is drawn styled, and Alt+B, Alt+I and Alt+K wrap the word at the cursor.
    pub fn markdown_field(&self, id: &str) -> bool {
        matches!(self.field_widget(id), Some(Widget::TextArea { editor_type, .. }) if editor_type == "md")
    }

    /// Whether the client takes the mouse: on unless the user turned it off (Alt+P, `TUI_MOUSE`).
    pub fn mouse_on(&self) -> bool {
        self.mouse.unwrap_or(true)
    }

    /// The mouse mode to write to the terminal, once each time it changes: the loops turn the reports on or off.
    pub fn take_mouse_mode(&mut self) -> Option<bool> {
        let on = self.mouse_on();
        (self.mouse_told != Some(on)).then(|| {
            self.mouse_told = Some(on);
            on
        })
    }

    /// The text a drag selected, for the loop to put on the clipboard.
    pub fn take_copy(&mut self) -> Option<String> {
        self.copy.take()
    }

    /// Put `text` on the clipboard through the loop, and say so: the count, a cut when it was longer than OSC 52
    /// carries, and the first time, how to fall back where the terminal does not take OSC 52.
    pub fn copy_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let total = text.chars().count();
        let mut kept = text;
        if kept.len() > COPY_MAX {
            let mut end = COPY_MAX;
            while !kept.is_char_boundary(end) {
                end -= 1;
            }
            kept.truncate(end);
        }
        let shown = kept.chars().count();
        let mut message = if shown < total {
            format!("Copied {shown} of {total} characters (the clipboard takes 64 KiB).")
        } else if total == 1 {
            "Copied 1 character.".to_string()
        } else {
            format!("Copied {total} characters.")
        };
        if !self.copied_before {
            message.push_str(
                " No clipboard? Shift+drag (Option+drag on macOS) selects in the terminal; Alt+P turns the mouse off.",
            );
            self.copied_before = true;
        }
        self.clip.clone_from(&kept);
        self.copy = Some(kept);
        self.set_message(message, false);
    }

    /// Ctrl+C outside a text field: the focused field's value, the current row's cells (tab between them), a link's
    /// text. It never quits; with nothing to copy it says what does.
    fn copy_focused(&mut self) {
        let text = self.focus().cloned().map_or_else(String::new, |item| match item.kind {
            FocusKind::List => {
                let at = self.row_at(&item.key);
                self.list_page(&item.key)
                    .get(at)
                    .map(|row| {
                        row.cells
                            .iter()
                            .map(|cell| cell.text().trim())
                            .collect::<Vec<_>>()
                            .join("\t")
                    })
                    .unwrap_or_default()
            }
            FocusKind::Link => self
                .index
                .links
                .get(&item.key)
                .map(|link| {
                    if link.text.is_empty() {
                        link.url.clone()
                    } else {
                        link.text.clone()
                    }
                })
                .unwrap_or_default(),
            FocusKind::Text | FocusKind::Check | FocusKind::Radio | FocusKind::DropDown => {
                self.buffer(&item.key).to_string()
            }
            _ => String::new(),
        });
        if text.trim().is_empty() {
            let quit = self
                .keys
                .chords(Action::Quit)
                .first()
                .map_or_else(|| "Ctrl+Q".to_string(), |press| press.label());
            self.set_message(format!("Nothing to copy here. {quit} quits."), false);
        } else {
            self.copy_text(text);
        }
    }

    /// A Markdown text area shows its preview rather than its source.
    pub fn previewing(&self, field: &str) -> bool {
        self.previews.contains(field)
    }

    /// The first line of a text area's source on screen: where it was last, moved so the cursor's row shows. The
    /// drawing clamps it to the text.
    pub fn area_top(&self, field: &str) -> usize {
        let top = self.area_tops.get(field).copied().unwrap_or(0);
        let Some(widget @ Widget::TextArea { .. }) = self.field_widget(field) else {
            return top;
        };
        let shown = crate::layout::text_area_rows(widget);
        let (row, _) = edit::row_col(self.buffer(field), self.cursor(field));
        if row < top {
            row
        } else if row >= top + shown {
            row + 1 - shown
        } else {
            top
        }
    }

    /// The first row of a Markdown preview on screen.
    pub fn preview_top(&self, field: &str) -> usize {
        self.preview_tops.get(field).copied().unwrap_or(0)
    }

    /// Scroll a text area by `rows` (up when negative): its preview when it shows one, else its source, the cursor kept
    /// in view. The wheel and PgUp/PgDn.
    pub fn scroll_area(&mut self, field: &str, rows: isize) {
        let shown = self.field_widget(field).map_or(1, crate::layout::text_area_rows);
        if self.previewing(field) {
            let top = self.preview_top(field).saturating_add_signed(rows);
            self.preview_tops.insert(field.to_string(), top);
            return;
        }
        let top = self.area_top(field).saturating_add_signed(rows);
        let lines = self.buffer(field).split('\n').count();
        let top = top.min(lines.saturating_sub(shown));
        self.area_tops.insert(field.to_string(), top);
        // The cursor moves into the rows shown, as a terminal pager moves its point.
        let text = self.buffer(field).to_string();
        let (row, col) = edit::row_col(&text, self.cursor(field));
        let row = row.clamp(top, top + shown - 1);
        self.cursors.insert(field.to_string(), edit::offset_of(&text, row, col));
    }

    /// Keep where a text area shows from, after its cursor moved, so it scrolls only as far as the cursor needs.
    fn follow_area_cursor(&mut self, field: &str) {
        if matches!(self.field_widget(field), Some(Widget::TextArea { .. })) {
            let top = self.area_top(field);
            self.area_tops.insert(field.to_string(), top);
        }
    }

    /// Whether a field shows now, by its `visible-when`.
    pub fn visible(&self, field_id: &str) -> bool {
        self.index.visible(field_id, &|key| self.edits.get(key).to_string())
    }

    /// The rows of a list's page, as the model has them.
    pub fn list_page(&self, list_id: &str) -> Vec<crate::model::ListRow> {
        self.list(list_id).map(|list| list.rows.clone()).unwrap_or_default()
    }

    // ---- keys ----

    /// One key. Returns true when the session should end.
    pub fn on_key(&mut self, press: &KeyPress) -> bool {
        if self.message_expired() {
            self.clear_message();
        }
        // Kiosk mode hides the rest of the screen, so its keys (Esc to leave) reach the kiosk component.
        if let Some(id) = self.kiosk_component() {
            if self.focus_id() != id {
                self.focus_key(&id);
            }
        }
        let handled = self.overlay_key(press)
            || self.assist_key(press)
            || self.number_key(press)
            || self.field_key(press)
            || self.list_key(press)
            || self.tree_key(press)
            || self.component_key(press);
        if !handled {
            if let Some(action) = self.keys.action(press) {
                self.act(action);
            }
        }
        self.quit
    }

    /// Pasted text, as one event: into the focused text field through its limits. A newline in a one-line field is a
    /// space, so a paste never submits a form.
    pub fn paste(&mut self, text: &str) {
        if self.component_paste(text) {
            return;
        }
        let Some(item) = self.focus().cloned().filter(|item| item.kind == FocusKind::Text) else {
            return;
        };
        let limits = self.limits(&item.key);
        let old = self.buffer(&item.key).to_string();
        let mut value = old.clone();
        let mut cursor = self.cursor(&item.key);
        let mut state = self.field_edits.remove(&item.key).unwrap_or_default();
        let before = (cursor, state.anchor);
        // A paste takes the place of the selection, and is one step to undo.
        if let Some((start, end)) = state.selection(cursor) {
            value = format!(
                "{}{}",
                edit::slice(&old, 0, start),
                edit::slice(&old, end, edit::chars_in(&old))
            );
            cursor = start;
        }
        state.anchor = None;
        edit::put_text(&mut value, &mut cursor, text, &limits);
        if let Some(op) = edit::diff_op(&old, &value, before.0, before.1, cursor, false) {
            state.history.record(op);
        }
        self.field_edits.insert(item.key.clone(), state);
        self.edits.set(&item.key, value);
        self.cursors.insert(item.key.clone(), cursor);
        self.after_edit(&item.key);
    }

    fn overlay_key(&mut self, press: &KeyPress) -> bool {
        match self.overlays.last().cloned() {
            Some(Overlay::Confirm(confirm)) => {
                self.confirm_key(confirm, press);
                true
            }
            Some(Overlay::Login(login)) => {
                self.login_key(login, press);
                true
            }
            Some(Overlay::Help) => {
                // A document's number opens it; any other key closes the help. Ctrl+Q still quits.
                self.overlays.pop();
                match &press.key {
                    Key::Char(ch) if press.is_plain() && ch.is_ascii_digit() => self.open_doc(*ch),
                    _ if self.keys.action(press) == Some(Action::Quit) => self.act(Action::Quit),
                    _ => {}
                }
                true
            }
            Some(Overlay::Info | Overlay::Messages) => {
                // Any key closes them; Ctrl+Q still quits.
                self.overlays.pop();
                if self.keys.action(press) == Some(Action::Quit) {
                    self.act(Action::Quit);
                }
                true
            }
            Some(Overlay::Menu(menu)) => {
                self.menu_key(menu, press);
                true
            }
            Some(Overlay::Palette(palette)) => {
                self.palette_key(palette, press);
                true
            }
            Some(Overlay::Search(text)) => {
                self.search_key(text, press);
                true
            }
            Some(Overlay::Editor(editor)) => {
                self.editor_key(editor, press);
                true
            }
            Some(Overlay::Columns(columns)) => {
                self.columns_key(columns, press);
                true
            }
            Some(Overlay::Prompt(prompt)) => {
                self.prompt_key(prompt, press);
                true
            }
            // the tray's Left and Right go between its tabs, also from an empty text field (Today opens on its quick
            // add, where the advertised key did nothing, ACC-UI-008); in a field with text they move the cursor
            Some(Overlay::Dialog(id))
                if id == shell::TRAY
                    && press.is_plain()
                    && (!self.focused_is_text() || self.buffer(self.focus_id()).is_empty()) =>
            {
                self.tray_key(press)
            }
            Some(Overlay::DropDown(drop)) => {
                self.dropdown_key(drop, press);
                true
            }
            Some(Overlay::Picker(picker)) => {
                self.picker_key(picker, press);
                true
            }
            Some(Overlay::Drawer(at)) => self.drawer_key(at, press),
            Some(Overlay::Dialog(_) | Overlay::Find(_) | Overlay::Row(..)) | None => false,
        }
    }

    /// With `#` on, digits name a link and Enter follows it.
    fn number_key(&mut self, press: &KeyPress) -> bool {
        if !self.numbering || self.focused_is_text() || !press.is_plain() {
            return false;
        }
        match &press.key {
            Key::Char(ch) if ch.is_ascii_digit() => {
                self.typed_number.push(*ch);
                self.set_message(format!("Follow link {}", self.typed_number), false);
                true
            }
            Key::Enter if !self.typed_number.is_empty() => {
                let number = std::mem::take(&mut self.typed_number);
                self.clear_message();
                let link = number
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .and_then(|at| self.index.link_order.get(at))
                    .cloned();
                match link {
                    Some(id) => self.follow_link(&id),
                    None => self.set_message(format!("There is no link {number}"), true),
                }
                true
            }
            Key::Backspace if !self.typed_number.is_empty() => {
                self.typed_number.pop();
                true
            }
            Key::Escape if !self.typed_number.is_empty() => {
                self.typed_number.clear();
                self.clear_message();
                true
            }
            _ => false,
        }
    }

    /// The focused field's own keys: typing in a text field, Space on a check, Left and Right on a radio.
    fn field_key(&mut self, press: &KeyPress) -> bool {
        let Some(item) = self.focus().cloned() else {
            return false;
        };
        match item.kind {
            FocusKind::Text
                if press.alt
                    && !press.ctrl
                    && matches!(press.key, Key::Char('b' | 'i' | 'k'))
                    && self.markdown_field(&item.key)
                    && !self.previewing(&item.key) =>
            {
                let Key::Char(kind) = press.key else {
                    return false;
                };
                let (text, cursor) = crate::markdown::wrap_word(self.buffer(&item.key), self.cursor(&item.key), kind);
                self.edits.set(&item.key, text);
                self.cursors.insert(item.key.clone(), cursor);
                self.after_edit(&item.key);
                self.follow_area_cursor(&item.key);
                true
            }
            FocusKind::Text
                if press.is_plain()
                    && matches!(press.key, Key::PageUp | Key::PageDown)
                    && matches!(self.field_widget(&item.key), Some(Widget::TextArea { .. })) =>
            {
                let shown = self.field_widget(&item.key).map_or(1, crate::layout::text_area_rows);
                let step = isize::try_from(shown).unwrap_or(1);
                self.scroll_area(&item.key, if press.key == Key::PageUp { -step } else { step });
                true
            }
            FocusKind::Text => {
                let limits = self.limits(&item.key);
                let mut text = self.buffer(&item.key).to_string();
                let mut cursor = self.cursor(&item.key);
                let mut state = self.field_edits.remove(&item.key).unwrap_or_default();
                let clip = self.clip.clone();
                let keys = FieldKeys {
                    clip: &clip,
                    select_all: self.scheme == KeyScheme::Modern,
                };
                let outcome = edit::apply_field(&mut text, &mut cursor, &mut state, press, &limits, keys);
                self.field_edits.insert(item.key.clone(), state);
                if let Some(copied) = outcome.copy.clone() {
                    self.copy_text(copied);
                }
                if let Some(message) = outcome.message {
                    self.set_message(message.to_string(), false);
                }
                match outcome.edited {
                    None | Some(Edited::No) => false,
                    Some(Edited::Moved) => {
                        self.cursors.insert(item.key.clone(), cursor);
                        self.follow_area_cursor(&item.key);
                        true
                    }
                    Some(Edited::Changed) => {
                        self.edits.set(&item.key, text);
                        self.cursors.insert(item.key.clone(), cursor);
                        self.after_edit(&item.key);
                        self.follow_area_cursor(&item.key);
                        true
                    }
                }
            }
            FocusKind::Check if press.is_plain() && press.key == Key::Char(' ') => {
                let choices = self.index.choices(&item.key).to_vec();
                match choices.as_slice() {
                    [one] => {
                        let value = if self.buffer(&item.key) == one.value {
                            String::new()
                        } else {
                            one.value.clone()
                        };
                        self.set_buffer(&item.key, value);
                    }
                    _ => self.open_dropdown(&item.key),
                }
                true
            }
            FocusKind::Radio if press.is_plain() && matches!(press.key, Key::Left | Key::Right) => {
                let choices = self.index.choices(&item.key).to_vec();
                if choices.is_empty() {
                    return true;
                }
                let at = choices.iter().position(|choice| choice.value == self.buffer(&item.key));
                let next = match (at, press.key == Key::Right) {
                    (None, _) => 0,
                    (Some(at), true) => (at + 1) % choices.len(),
                    (Some(at), false) => (at + choices.len() - 1) % choices.len(),
                };
                self.set_buffer(&item.key, choices[next].value.clone());
                true
            }
            FocusKind::DropDown if press.is_plain() && press.key == Key::Char(' ') => {
                self.open_dropdown(&item.key);
                true
            }
            _ => false,
        }
    }

    fn limits(&self, key: &str) -> Limits {
        let node = key.split('#').next().unwrap_or(key);
        let Some(field) = self.index.fields.get(node) else {
            return Limits::default();
        };
        let mask = match &field.widget {
            Widget::TextLine { mask, .. } => mask.clone(),
            _ => String::new(),
        };
        Limits {
            maxlength: usize::try_from(field.maxlength).unwrap_or(0),
            mask,
            multiline: matches!(field.widget, Widget::TextArea { .. }),
        }
    }

    /// Open the screen's text search (`/`), whatever has the focus: the agent's `find`, which must not type into a
    /// focused text field.
    pub fn open_text_search(&mut self) {
        self.overlays.push(Overlay::Search(String::new()));
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Next => self.move_focus(1),
            Action::Prev => self.move_focus(-1),
            Action::Activate => self.activate(),
            Action::Back => self.back(),
            Action::Cancel => self.cancel(),
            Action::Help => self.overlays.push(Overlay::Help),
            Action::Palette => self.open_palette(false),
            Action::GoTo => self.open_palette(true),
            Action::QuickCreate => self.open_quick_create(),
            Action::Actions => self.open_actions(),
            Action::FindText => self.overlays.push(Overlay::Search(String::new())),
            Action::FindNext => self.find_next(true),
            Action::FindPrev => self.find_next(false),
            Action::Messages => self.overlays.push(Overlay::Messages),
            Action::Tray => self.open_tray(0),
            Action::EditRow => self.edit_row(),
            Action::Info => self.overlays.push(Overlay::Info),
            Action::Prompt => self.prompt(),
            Action::Refresh => self.refresh(),
            Action::PagePrev => self.turn_page(-1),
            Action::PageNext => {
                // A wizard with no list to page: PgDn is Next.
                if self.target_list().is_none() && self.index.wizard.is_some() {
                    self.wizard_next();
                } else {
                    self.turn_page(1);
                }
            }
            Action::Drawer => self.toggle_drawer(),
            Action::Assist => self.toggle_assist(),
            Action::AssistStop => self.assist_stop(),
            Action::Submit => {
                let id = self.focus_id().to_string();
                if let Some(list) = self.find_open().map(str::to_string) {
                    self.submit_find(&list);
                } else {
                    self.submit_form_of(&id, None, false);
                }
            }
            Action::Select => self.toggle_row(),
            Action::SelectAll => self.select_all(),
            Action::Find => self.open_find(),
            Action::Sort => self.cycle_sort(),
            Action::View => self.cycle_view(),
            Action::NumberLinks => {
                self.numbering = !self.numbering;
                self.typed_number.clear();
            }
            Action::OpenBrowser => {
                let url = self.focused_url().unwrap_or_else(|| self.location_url());
                self.set_message(format!("Open in browser: {url}"), false);
                self.open = Some(url);
            }
            Action::Redraw => self.redraw = true,
            Action::Mouse => {
                let on = !self.mouse_on();
                self.mouse = Some(on);
                self.keep_preference("TUI_MOUSE", if on { "true" } else { "false" });
                let text = if on {
                    "Mouse on: click, scroll, drag to copy. Alt+P gives the terminal its own selection."
                } else {
                    "Mouse off: the terminal selects and copies. Alt+P turns the mouse on."
                };
                self.set_message(text.to_string(), false);
            }
            Action::Log if self.log_offered => self.log_open = !self.log_open,
            Action::Log => self.set_message("The log is not offered in this session.".to_string(), false),
            Action::Quit => {
                if self.edits.any_dirty() {
                    self.overlays.push(Overlay::Confirm(Confirm::leave(Then::Quit)));
                } else {
                    self.quit = true;
                }
            }
            Action::Tab(n) => {
                let url = self
                    .index
                    .tabs
                    .get(usize::from(n).saturating_sub(1))
                    .map(|tab| tab.url.clone());
                if let Some(url) = url {
                    self.follow_url(&url);
                }
            }
            Action::TabNext | Action::TabPrev => self.step_tab(action == Action::TabNext),
            Action::First | Action::Last => self.focus_end(action == Action::Last),
            Action::Copy => self.copy_focused(),
        }
    }

    /// Ctrl+PgDn and Ctrl+PgUp: the subscreen tab after or before the current one, round the end.
    fn step_tab(&mut self, forward: bool) {
        let tabs = &self.index.tabs;
        if tabs.is_empty() {
            return;
        }
        let count = tabs.len();
        let at = tabs.iter().position(|tab| tab.active).unwrap_or(0);
        let next = if forward {
            (at + 1) % count
        } else {
            (at + count - 1) % count
        };
        let url = tabs[next].url.clone();
        self.follow_url(&url);
    }

    /// Home and End: a list's first and last row of the page, else the first and last thing to focus.
    fn focus_end(&mut self, last: bool) {
        if let Some(item) = self.focus().cloned().filter(|item| item.kind == FocusKind::List) {
            let rows = self.list(&item.key).map_or(0, |list| list.rows.len());
            if rows > 0 {
                self.rows.insert(item.key, if last { rows - 1 } else { 0 });
                return;
            }
        }
        if !self.focus.is_empty() {
            self.focus_at = if last { self.focus.len() - 1 } else { 0 };
        }
    }

    fn activate(&mut self) {
        let Some(item) = self.focus().cloned() else {
            return;
        };
        match item.kind {
            FocusKind::DropDown => self.open_dropdown(&item.key),
            FocusKind::Link => self.follow_link(&item.key),
            FocusKind::List => {
                let at = self.row_at(&item.key);
                let editable = self.index.rows.contains_key(&(item.key.clone(), at));
                // Modern: Enter follows the row's first link, else edits the row (`e` always edits). Classic: a row
                // that has fields to edit opens its editor, else its first link (Right follows).
                if self.scheme == KeyScheme::Modern {
                    if !self.follow_row_link(&item.key, at) {
                        if editable {
                            self.edit_row();
                        } else {
                            self.nothing_to_open(&item.key, at);
                        }
                    }
                    return;
                }
                if editable {
                    self.edit_row();
                    return;
                }
                if !self.follow_row_link(&item.key, at) {
                    self.nothing_to_open(&item.key, at);
                }
            }
            FocusKind::Dialog => self.open_dialog(&item.key),
            FocusKind::Menu => self.open_button_menu(&item.key),
            FocusKind::Tree => self.tree_enter(&item.key),
            FocusKind::Toggle => self.toggle_box(&item.key),
            FocusKind::Edit => self.open_editor(&item.key),
            FocusKind::Component => {}
            FocusKind::Button => {
                if let Scope::Find(list) = &item.scope {
                    // the find dialog's own button (a header-field submit) finds, as Enter in its fields does
                    let list = list.clone();
                    self.submit_find(&list);
                } else if matches!(self.field_widget(&item.key), Some(Widget::Reset { .. })) {
                    self.reset_form(&item.key);
                } else {
                    self.submit_form_of(&item.key, None, false);
                }
            }
            FocusKind::Text | FocusKind::Check | FocusKind::Radio => {
                if let Scope::Find(list) = &item.scope {
                    let list = list.clone();
                    self.submit_find(&list);
                } else {
                    self.submit_form_of(&item.key, None, false);
                }
            }
        }
    }

    fn back(&mut self) {
        if self.edits.any_dirty() {
            self.overlays.push(Overlay::Confirm(Confirm::leave(Then::Back)));
            return;
        }
        self.go_back_now();
    }

    fn go_back_now(&mut self) {
        let Some(location) = self.nav.pop_history() else {
            self.set_message("This is the first screen.".to_string(), false);
            return;
        };
        self.overlays.clear();
        self.request_level(&location.path, location.parameters, Intent::Navigate { history: false });
    }

    /// Esc: close the top dialog, find or row editor. Then, in Modern, open the palette; in Classic, clear the message,
    /// else leave the edits after asking.
    fn cancel(&mut self) {
        if let Some(top) = self.overlays.last() {
            if matches!(top, Overlay::Dialog(_) | Overlay::Find(_) | Overlay::Row(..)) {
                self.overlays.pop();
                self.drop_closed_extras();
                self.rebuild_focus();
            }
            return;
        }
        if self.scheme == KeyScheme::Modern {
            self.clear_message();
            self.open_palette(false);
            return;
        }
        if !self.message().is_empty() {
            self.clear_message();
            return;
        }
        if self.edits.any_dirty() {
            self.overlays.push(Overlay::Confirm(Confirm::leave(Then::Nothing)));
        }
    }

    fn prompt(&mut self) {
        let Some(item) = self.focus().cloned() else {
            return;
        };
        match self.field_widget(&item.key).cloned() {
            Some(Widget::DropDown { .. } | Widget::Check { .. }) => self.open_dropdown(&item.key),
            Some(Widget::DateTime { format, date_type, .. }) if date_type != "time" => {
                let date = forms::date_of(self.buffer(&item.key), &format, &date_type);
                self.overlays.push(Overlay::Picker(Picker { key: item.key, date }));
            }
            Some(Widget::DateFind { format, date_type, .. }) => {
                let date = forms::date_of(self.buffer(&item.key), &format, &date_type);
                self.overlays.push(Overlay::Picker(Picker { key: item.key, date }));
            }
            Some(Widget::TextArea { editor_type, .. }) if editor_type == "md" => {
                // F4 on a Markdown text area shows its preview, and again its source (D12).
                if !self.previews.remove(&item.key) {
                    self.previews.insert(item.key.clone());
                }
            }
            Some(Widget::TextFind { .. }) => {
                // F4 on a text find cycles its operator.
                let field = item.key.split('#').next().unwrap_or(&item.key).to_string();
                let key = format!("{field}#op");
                let ops = ["contains", "equals", "begins", "empty"];
                let at = ops
                    .iter()
                    .position(|op| *op == self.buffer(&key))
                    .map_or(0, |at| (at + 1) % ops.len());
                self.set_buffer(&key, ops[at].to_string());
                self.set_message(format!("Find by: {}", ops[at]), false);
            }
            _ => {}
        }
    }

    /// F9: show the drawer and focus it; again, hide it. The choice is the user's `TUI_DRAWER_OPEN` preference.
    fn toggle_drawer(&mut self) {
        if self.drawer_focus().is_some() {
            self.overlays.retain(|overlay| !matches!(overlay, Overlay::Drawer(_)));
            self.drawer = Some(false);
            self.keep_drawer(false);
            return;
        }
        self.drawer = Some(true);
        self.keep_drawer(true);
        if !self.drawer_rows().is_empty() {
            self.overlays.push(Overlay::Drawer(0));
        }
    }

    // ---- navigation ----

    fn follow_link(&mut self, id: &str) {
        let Some(link) = self.index.links.get(id).cloned() else {
            return;
        };
        if link.disabled {
            self.set_message("This action is not available.".to_string(), true);
            return;
        }
        if !link.confirm.is_empty() {
            self.overlays.push(Overlay::Confirm(Confirm::ask(
                link.confirm.clone(),
                Then::FollowLink(link.id.clone()),
            )));
            return;
        }
        self.follow_link_now(id);
    }

    fn follow_link_now(&mut self, id: &str) {
        let Some(link) = self.index.links.get(id).cloned() else {
            return;
        };
        if link.kind == "image" || (link.browser && !link.posts() && !link.writes) {
            self.set_message(format!("Open in browser: {}", link.url), false);
            self.open = Some(link.url);
            return;
        }
        if self
            .index
            .wizard
            .as_ref()
            .is_some_and(|wizard| id == crate::layout::wizard_next(&wizard.id))
        {
            self.wizard_next();
            return;
        }
        if link.posts() {
            let location = Location::of_url(&link.url);
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
                    url: location.path,
                    method: "post".to_string(),
                    fields: location.parameters,
                },
            );
            return;
        }
        self.follow_url(&link.url);
    }

    /// Open a row by its link ([`row_link_target`]: another screen first), if any: a known link goes through
    /// `follow_link`, which asks its confirmation as a click on it does; else the url is followed.
    fn follow_row_link(&mut self, list: &str, at: usize) -> bool {
        let here = self.nav.location().path;
        let Some((id, url)) = self
            .list_page(list)
            .get(at)
            .and_then(|row| row_link_target(row, &here))
            .map(|(id, url)| (id.to_string(), url.to_string()))
        else {
            return false;
        };
        if self.index.links.contains_key(&id) {
            self.follow_link(&id);
        } else {
            self.follow_url(&url);
        }
        true
    }

    /// Right or Enter on a row with no link to open: say so, and where its other links are.
    pub(super) fn nothing_to_open(&mut self, list: &str, at: usize) {
        let others = self.list_page(list).get(at).is_some_and(|row| {
            row.cells
                .iter()
                .flat_map(Cell::nodes)
                .any(|node| matches!(node, Node::Link { .. }))
        });
        let text = if others {
            "Nothing to open on this row; F10 lists its actions."
        } else {
            "Nothing to open on this row."
        };
        self.set_message(text.to_string(), false);
    }

    /// Open a url: a screen in the client; an outside address or an explicit print view is the browser's.
    fn follow_url(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        if url.contains("://")
            || url.starts_with("mailto:")
            || Location::of_url(url).parameters.get("renderMode").map(String::as_str) == Some("html-print")
        {
            self.set_message(format!("Open in browser: {url}"), false);
            self.open = Some(url.to_string());
            return;
        }
        if self.edits.any_dirty() {
            self.overlays
                .push(Overlay::Confirm(Confirm::leave(Then::Navigate(url.to_string()))));
            return;
        }
        self.navigate(url, true);
    }

    /// Ask for a screen. With `history` the screen left is remembered for F3.
    pub fn navigate(&mut self, url: &str, history: bool) {
        let location = Location::of_url(url);
        self.request_level(&location.path, location.parameters, Intent::Navigate { history });
    }

    /// The current screen again, with its parameters. Record headers keep their cache; see [`Self::refresh`].
    pub fn reload(&mut self) {
        let location = self.nav.location();
        self.request_level(&location.path, location.parameters, Intent::Reload);
    }

    /// The current screen and its record headers again: F5, or a component's answer that reloads the screen.
    pub fn refresh(&mut self) {
        self.reload();
        self.reload_parents();
    }

    /// A record's header is a separate level. Refresh it after a write or explicit reload, while normal tab
    /// navigation can keep the cached header.
    fn reload_parents(&mut self) {
        let parents: Vec<(String, BTreeMap<String, String>)> = self
            .nav
            .parents()
            .iter()
            .map(|level| (level.model.screen.path.clone(), level.parameters.clone()))
            .collect();
        let page = self.path().to_string();
        for (path, parameters) in parents {
            let call = LinkCall::Parent {
                path: level_path(&path),
                page: page.clone(),
                parameters: parameters.clone(),
            };
            if let Some(id) = self.lanes.push(Purpose::Parent(path), call) {
                self.intents.insert(id, (Intent::Reload, parameters));
            }
        }
    }

    fn reload_with(&mut self, parameters: BTreeMap<String, String>) {
        let path = self.path().to_string();
        self.request_level(&path, parameters, Intent::Reload);
    }

    fn request_level(&mut self, path: &str, parameters: BTreeMap<String, String>, intent: Intent) {
        self.last_location = Some(Location {
            path: path.to_string(),
            parameters: parameters.clone(),
        });
        let mut sent = parameters.clone();
        if let Some(rows) = self.page_rows {
            sent.entry("pageSize".to_string()).or_insert_with(|| rows.to_string());
        }
        if self.mobile {
            sent.insert("_uiType".to_string(), "mobile".to_string());
        }
        let call = LinkCall::Level {
            path: shell::terminal_url(path),
            parameters: sent,
        };
        if let Some(id) = self.lanes.push(Purpose::Level, call) {
            self.intents.insert(id, (intent, parameters));
        }
    }

    fn request_shell(&mut self, path: &str) {
        self.lanes
            .push(Purpose::Shell, LinkCall::Shell { path: path.to_string() });
    }

    fn level_reply(&mut self, id: u64, result: Result<ScreenModel, LinkError>) {
        let (intent, parameters) = self
            .intents
            .remove(&id)
            .unwrap_or((Intent::Reload, self.nav.leaf_parameters().clone()));
        let model = match result {
            Ok(model) => model,
            Err(err) => {
                self.set_message(err.to_string(), true);
                self.assist_after_level(false);
                return;
            }
        };
        let navigate = matches!(intent, Intent::Navigate { .. });
        if let Intent::Navigate { history: true } = intent {
            if !self.model().screen.path.is_empty() {
                self.nav.push_history();
            }
        }
        let app_changed = app_root(&model.screen.path) != app_root(self.path());
        let path_changed = model.screen.path != self.path();
        // a code editor of the same screen keeps its cursor, scroll and undo; it starts over when its text is not its own
        // (another file), as Editor::of decides
        let kept_editors: Vec<(String, crate::components::ComponentState)> = if navigate && !path_changed {
            self.components
                .iter()
                .filter(|(id, _)| self.index.components.get(*id).is_some_and(|info| info.name == "code"))
                .map(|(id, state)| (id.clone(), state.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let missing = self.nav.set_leaf(model, parameters.clone());
        // each parent names the page it is loaded for, so one the user may not open alone answers as the page's frame
        let page = self.path().to_string();
        for path in missing {
            let call = LinkCall::Parent {
                path: level_path(&path),
                page: page.clone(),
                parameters: parameters.clone(),
            };
            if let Some(id) = self.lanes.push(Purpose::Parent(path), call) {
                self.intents.insert(id, (Intent::Reload, parameters.clone()));
            }
        }
        if navigate {
            self.overlays.clear();
            self.rows.clear();
            self.selected.clear();
            self.hscroll.clear();
            self.cursors.clear();
            self.field_edits.clear();
            self.extras.clear();
            self.fragments.clear();
            self.trees.clear();
            self.components.clear();
            self.timers.retain(|timer, _| !matches!(timer, Timer::Component(_)));
            self.shown_text.clear();
            self.collapsed.clear();
            if app_changed && !self.path().is_empty() {
                let path = self.path().to_string();
                self.request_shell(&path);
            }
        } else {
            // A find submit closes its dialog; paging keeps a list's row in range. A list's selection is kept by id.
            self.overlays
                .retain(|overlay| !matches!(overlay, Overlay::Find(_) | Overlay::DropDown(_)));
            self.put_fragments();
        }
        self.reindex(!navigate);
        self.after_screen(navigate, path_changed);
        for (id, state) in kept_editors {
            if self.index.components.get(&id).is_some_and(|info| info.name == "code") {
                self.components.insert(id, state);
            }
        }
        let keep_message = !self.message.text.is_empty() && !self.message_expired() && !navigate;
        if !keep_message {
            self.clear_message();
        }
        self.show_model_messages();
        // A save's message, unless the screen brought its own.
        if let Some(text) = self.after_level.take() {
            if self.message().is_empty() {
                self.set_message(text, false);
            }
        }
        self.assist_after_level(navigate);
    }

    /// The flash messages a transition left for this screen, and the errors its render added: a validation error (a
    /// find value that is not a date) is on the line too, as no submitted form is there to mark.
    fn show_model_messages(&mut self) {
        let messages = self.model().messages.clone();
        let mut errors = messages.errors.clone();
        errors.extend(
            messages
                .validation_errors
                .iter()
                .map(|err| err.message.clone())
                .filter(|text| !text.is_empty() && !messages.errors.contains(text)),
        );
        if !errors.is_empty() {
            self.set_message(distinct_join(&errors), true);
        } else if !messages.info.is_empty() {
            self.set_message(distinct_join(&messages.info), false);
        }
    }

    fn apply_shell(&mut self, shell: ShellInfo) {
        self.shell.keys.clone_from(&shell.keys);
        self.shell.key_scheme = shell.key_scheme;
        self.rebuild_keys();
        if self.mouse.is_none() {
            self.mouse = shell.mouse;
        }
        if self.drawer.is_none() {
            self.drawer = shell.drawer_open;
        }
        if !shell.counts.is_empty() {
            self.counts.clone_from(&shell.counts);
            self.set_timer(Timer::Counts, Instant::now() + self.counts_every());
        }
        self.shell = shell;
    }

    // ---- focus ----

    fn move_focus(&mut self, delta: isize) {
        if self.focus.is_empty() {
            return;
        }
        let len = self.focus.len() as isize;
        self.focus_at = (self.focus_at as isize + delta).rem_euclid(len) as usize;
    }

    fn focus_first_in_scope(&mut self) {
        self.focus_at = 0;
    }

    fn scope(&self) -> Scope {
        self.overlays
            .iter()
            .rev()
            .find_map(|overlay| match overlay {
                Overlay::Dialog(id) => Some(Scope::Dialog(id.clone())),
                Overlay::Find(id) => Some(Scope::Find(id.clone())),
                Overlay::Row(list, row) => Some(Scope::Row(list.clone(), *row)),
                _ => None,
            })
            .unwrap_or(Scope::Main)
    }

    /// The focus order of the current scope: what a dialog or the find dialog holds while it is open, else the
    /// screen, without fields their `visible-when` hides.
    fn rebuild_focus(&mut self) {
        let mut previous = self.focus_id().to_string();
        if previous.is_empty() {
            previous = std::mem::take(&mut self.focus_kept);
        }
        let scope = self.scope();
        let values = |key: &str| self.edits.get(key).to_string();
        self.focus = self
            .index
            .order
            .iter()
            .filter(|item| {
                item.scope == scope
                    && self.index.visible(&item.node, &values)
                    && item.boxes.iter().all(|id| !self.is_collapsed(id))
            })
            .cloned()
            .collect();
        let model_focus = self.model().focus.clone();
        self.focus_at = self
            .focus
            .iter()
            .position(|item| item.key == previous)
            .or_else(|| {
                self.focus
                    .iter()
                    .position(|item| item.key == model_focus && !model_focus.is_empty())
            })
            .unwrap_or(0);
        // nothing to focus yet: the next rebuild comes back to the key
        self.focus_kept = if self.focus.is_empty() { previous } else { String::new() };
    }

    /// The model changed: index it, take the values it sends, and rebuild the focus. With `keep`, edits that are still
    /// dirty stay.
    fn reindex(&mut self, keep: bool) {
        self.apply_views();
        self.index = Index::with_extras(self.nav.models(), &self.extras);
        self.edits.load(self.index.initial_values(), keep);
        let lists: Vec<(String, usize)> = self
            .index
            .lists
            .iter()
            .map(|(id, list)| (id.clone(), list.rows.len()))
            .collect();
        for (id, rows) in lists {
            let at = self.rows.entry(id).or_insert(0);
            *at = (*at).min(rows.saturating_sub(1));
        }
        self.rebuild_focus();
    }

    // ---- messages ----

    fn set_message(&mut self, text: String, error: bool) {
        self.message = Message {
            text,
            error,
            until: (!error).then(|| Instant::now() + Duration::from_secs(MESSAGE_SECONDS)),
        };
        // The history keeps what was said, not the prompts of typing a link number.
        if !self.message.text.starts_with("Follow link")
            && self.history.back().is_none_or(|last| last.text != self.message.text)
        {
            if self.history.len() == HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(Message {
                until: None,
                ..self.message.clone()
            });
        }
    }

    fn clear_message(&mut self) {
        self.message = Message::default();
    }

    fn focused_url(&self) -> Option<String> {
        let item = self.focus()?;
        match item.kind {
            FocusKind::Link => self.index.links.get(&item.key).map(|link| link.url.clone()),
            FocusKind::List => {
                let here = self.nav.location().path;
                self.list_page(&item.key)
                    .get(self.row_at(&item.key))
                    .and_then(|row| row_link_target(row, &here))
                    .map(|(_, url)| url.to_string())
            }
            _ => None,
        }
    }

    /// The screen's address with its parameters.
    pub fn location_url(&self) -> String {
        let location = self.nav.location();
        if location.parameters.is_empty() {
            return location.path;
        }
        let query: Vec<String> = location
            .parameters
            .iter()
            .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
            .collect();
        format!("{}?{}", location.path, query.join("&"))
    }
}

/// A declared action of a screen (an `ext-action` that opens a screen) as the terminal lists it: its parameters come from
/// the page's own parameters, and its condition is tested on them.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExtAction {
    pub title: String,
    pub url: String,
    /// `(target parameter, page parameter)`.
    pub parameters: Vec<(String, String)>,
    /// `(target parameter, fixed value)`.
    pub values: Vec<(String, String)>,
    /// `(field, op, value)` of its condition, if it has one.
    pub condition: Option<(String, String, String)>,
}

impl ExtAction {
    /// The url with its parameters from the page's, or None when the page lacks one or its condition fails.
    pub(crate) fn link(&self, page: &BTreeMap<String, String>) -> Option<String> {
        if let Some((field, op, value)) = &self.condition {
            let found = page.get(field).map(String::as_str).unwrap_or("");
            let truthy = !found.is_empty() && found != "false" && found != "N";
            let pass = match op.as_str() {
                "truthy" => truthy,
                "falsy" => !truthy,
                "equals" => found == value,
                "not-equals" => found != value,
                _ => false,
            };
            if !pass {
                return None;
            }
        }
        let mut query = Vec::new();
        for (name, from) in &self.parameters {
            let value = page.get(from).filter(|value| !value.is_empty())?;
            query.push(format!("{}={}", encode(name), encode(value)));
        }
        for (name, value) in &self.values {
            query.push(format!("{}={}", encode(name), encode(value)));
        }
        Some(if query.is_empty() {
            self.url.clone()
        } else {
            format!("{}?{}", self.url, query.join("&"))
        })
    }
}

/// A row of the drawer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawerRow {
    pub title: String,
    /// Where it goes; empty for a heading.
    pub url: String,
    /// 0 for an app or a heading, 1 for an item under it.
    pub depth: usize,
    /// A menu item whose link is disabled: drawn dimmed, with no `url`, so it is neither followed nor offered.
    pub disabled: bool,
}

impl DrawerRow {
    fn new(title: &str, url: &str, depth: usize) -> Self {
        Self {
            title: title.to_string(),
            url: url.to_string(),
            depth,
            disabled: false,
        }
    }

    fn of_item(item: &MenuItem, depth: usize) -> Self {
        if item.disabled {
            Self {
                disabled: true,
                ..Self::new(&item.title, "", depth)
            }
        } else {
            Self::new(&item.title, &item.url, depth)
        }
    }
}

/// The drawer's rows: each app menu item and the items under it. An item whose group is not that of the item before
/// it, at its level, has the group as a heading above it.
pub fn drawer_rows(menu: &[MenuItem]) -> Vec<DrawerRow> {
    let mut rows = Vec::new();
    menu_level(menu, 0, true, &mut rows);
    rows
}

fn menu_level(items: &[MenuItem], depth: usize, children: bool, rows: &mut Vec<DrawerRow>) {
    let mut group = "";
    for item in items {
        if !item.group.is_empty() && item.group != group {
            rows.push(DrawerRow::new(&item.group, "", depth));
        }
        group = &item.group;
        rows.push(DrawerRow::of_item(item, depth));
        if children && depth == 0 {
            menu_level(&item.children, 1, false, rows);
        }
    }
}

/// A screen on the path below the app's top-level screens and its subscreens, from the menu data: what the drawer lists
/// under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuLevel {
    /// The screen's path, with no query.
    pub path: String,
    /// The screen's title, which its drawer row has.
    pub title: String,
    pub items: Vec<MenuItem>,
    /// The title of the subscreen on the path, empty when none is.
    pub active_title: String,
}

/// The drawer's rows with the deeper levels of the path nested in them, as the browser's drawer (`moqui.drawerRows`)
/// opens the current path: each level's subscreens under the row of its screen, a level more indented. A screen whose
/// subscreen on the path has its own title (its own list) gives way to it: its row is left out and its subscreens take
/// its place, so the drawer never reads `Suppliers › Suppliers`. A level whose screen has no row (another app, or a
/// screen the menu does not list) adds nothing.
pub fn drawer_rows_with_levels(menu: &[MenuItem], levels: &[MenuLevel]) -> Vec<DrawerRow> {
    let mut rows = drawer_rows(menu);
    let path_of = |url: &str| {
        url.split(['?', '#'])
            .next()
            .unwrap_or(url)
            .trim_end_matches('/')
            .to_string()
    };
    for level in levels {
        let path = level.path.trim_end_matches('/');
        // the level's row: its url is the level's path, or goes on to a default subscreen under it (the server's
        // pathWithParams, /apps/People/Payroll/PayRuns for the level /apps/People/Payroll) and it has the level's title
        let candidate = |row: &DrawerRow| row.depth >= 1 && !row.url.is_empty();
        let under = format!("{path}/");
        let Some(at) = rows
            .iter()
            .rposition(|row| candidate(row) && path_of(&row.url) == path)
            .or_else(|| {
                rows.iter().rposition(|row| {
                    candidate(row) && row.title == level.title && path_of(&row.url).starts_with(&under)
                })
            })
        else {
            continue;
        };
        let depth = rows[at].depth;
        // the screen's own rows already there (none, unless two levels share a path) are replaced
        let end = (at + 1..rows.len())
            .find(|&next| rows[next].depth <= depth)
            .unwrap_or(rows.len());
        let mut nested = Vec::new();
        menu_level(&level.items, depth + 1, false, &mut nested);
        let gives_way = !level.active_title.is_empty() && level.active_title == rows[at].title;
        if gives_way {
            for row in &mut nested {
                row.depth = depth;
            }
            rows.splice(at..end, nested);
        } else {
            rows.splice(at + 1..end, nested);
        }
    }
    rows
}

/// Which drawer rows are the screen shown (`•`): a row whose path is a level shown (`paths`), or above one. Where rows
/// share a path, a saved find (`?formListFindId=`) beside its Find screen, the query decides too: a row whose query the
/// shown screen's `parameters` hold is the one shown, and the rows without one are when none does. Home is never shown.
pub fn drawer_active(rows: &[DrawerRow], paths: &[&str], parameters: &BTreeMap<String, String>) -> Vec<bool> {
    let split = |url: &str| -> (String, String) {
        let (path, query) = url.split_once('?').unwrap_or((url, ""));
        (path.to_string(), query.to_string())
    };
    let parts: Vec<(String, String)> = rows.iter().map(|row| split(&row.url)).collect();
    let holds = |query: &str| {
        crate::nav::query_parameters(query)
            .iter()
            .all(|(name, value)| parameters.get(name) == Some(value))
    };
    rows.iter()
        .zip(&parts)
        .map(|(row, (path, query))| {
            if row.url.is_empty() || (row.depth == 0 && path == "/apps") {
                return false;
            }
            let on_path = paths
                .iter()
                .any(|shown| shown == path || shown.starts_with(&format!("{path}/")));
            let mut sharing = parts.iter().filter(|(other, _)| other == path);
            if !on_path || sharing.clone().count() < 2 {
                return on_path;
            }
            if !query.is_empty() {
                return holds(query);
            }
            !sharing.any(|(_, other)| !other.is_empty() && holds(other))
        })
        .collect()
}

fn app_root(path: &str) -> String {
    path.split('/')
        .filter(|name| !name.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(byte)),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
