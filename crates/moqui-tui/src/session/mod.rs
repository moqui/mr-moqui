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
use crate::edit::{self, Edited, Limits};
use crate::forms::{self, Extra, FocusItem, FocusKind, Index, Scope};
use crate::keys::{merge_keys, Action, Key, KeyMap, KeyPress, MergeSkip};
use crate::layout::PAGE_ROWS;
use crate::link::{row_link, LinkCall, LinkError, MenuItem, RawBody, Reply, ScreenLink, ShellInfo, TransitionOutcome};
use crate::model::{Choice, FormList, Node, ScreenModel, Widget};
use crate::nav::{level_path, Location, Nav};
use crate::state::{Confirm, ConfirmKind, Edits, Finished, Lanes, Purpose, Request, Then};

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

/// The login dialog after a 401 (section 1.7).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    pub password: String,
    pub code: String,
    /// 0 username, 1 password, 2 code.
    pub at: usize,
    pub error: String,
    /// The password was right and a code is wanted: the password is kept for the next try, and these factors take a
    /// sent code (Alt+S sends to the first).
    pub code_wanted: bool,
    pub sendable: Vec<crate::link::Sendable>,
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
    focus: Vec<FocusItem>,
    focus_at: usize,
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
            focus: Vec::new(),
            focus_at: 0,
            overlays: Vec::new(),
            rows: BTreeMap::new(),
            selected: BTreeMap::new(),
            hscroll: BTreeMap::new(),
            message: Message::default(),
            lanes: Lanes::default(),
            intents: BTreeMap::new(),
            submitted: None,
            after_level: None,
            keys: KeyMap::baseline(),
            key_skips: Vec::new(),
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
            if purpose != Purpose::Login {
                self.ask_login();
            }
            return;
        }
        match (purpose, reply) {
            (Purpose::Level, Reply::Level(result)) => self.level_reply(id, result),
            (Purpose::Parent(_), Reply::Level(result)) => {
                let parameters = self.intents.remove(&id).map(|(_, params)| params).unwrap_or_default();
                // A parent that does not load leaves the leaf as it is: the header is extra.
                if let Ok(model) = result {
                    self.nav.set_parent(model, parameters);
                    self.reindex(true);
                }
            }
            (Purpose::Transition, Reply::Transition(result)) => self.transition_reply(result),
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
            (Purpose::Fragment(id), Reply::Fragment(result)) => self.fragment_reply(&id, result),
            (Purpose::ValueUrl(field), Reply::Json(result)) => self.value_reply(&field, result),
            (Purpose::Palette, Reply::Json(result)) => self.palette_reply(result),
            (Purpose::Counts, Reply::Json(result)) => self.counts_reply(result),
            (Purpose::Tree(key), Reply::Json(result)) => self.tree_reply(&key, result),
            (Purpose::Component(id), Reply::Json(result)) => self.component_reply(&id, result),
            (Purpose::Menu, Reply::Json(result)) => self.menu_reply(result),
            (Purpose::Help, Reply::Json(result)) => self.doc_reply(result),
            (Purpose::Export, Reply::Raw(result)) => self.export_reply(result),
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

    /// The screen's place, `Gallery ▸ Detail`, with the leaf's title in place of its name (`JobRuns ▸ Job Run Detail`,
    /// not `JobRunDetail ▸ Job Run Detail`): the title line's breadcrumb before it is cut to the width.
    pub fn crumbs(&self) -> Vec<String> {
        let mut names: Vec<String> = self.model().screen.path_list.iter().skip(1).cloned().collect();
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

    /// Fire the waits for typing to stop (a server search's options, the palette's search) now rather than when they
    /// are due: the agent mode answers once the screen has settled. The counters are left to their time. Returns true
    /// when one fired.
    pub fn fire_waits(&mut self) -> bool {
        let waits: Vec<Timer> = self
            .timers
            .keys()
            .filter(|timer| !matches!(timer, Timer::Counts | Timer::Assist))
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

    /// Write field `at` of the login dialog on top (0 username, 1 password, 2 code) and move to it, as typing there
    /// does; false with no login dialog on top.
    pub fn set_login_field(&mut self, at: usize, value: &str) -> bool {
        let Some(Overlay::Login(login)) = self.overlays.last_mut() else {
            return false;
        };
        let text = match at {
            0 => &mut login.username,
            1 => &mut login.password,
            _ => &mut login.code,
        };
        *text = value.to_string();
        login.at = at.min(2);
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
        self.copy = Some(kept);
        self.set_message(message, false);
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
        let Some(item) = self.focus().cloned().filter(|item| item.kind == FocusKind::Text) else {
            return;
        };
        let limits = self.limits(&item.key);
        let mut value = self.buffer(&item.key).to_string();
        let mut cursor = self.cursor(&item.key);
        for ch in text.chars().filter(|ch| *ch != '\r') {
            let press = if ch == '\n' && limits.multiline {
                KeyPress::plain(Key::Enter)
            } else if ch == '\n' || ch == '\t' {
                KeyPress::plain(Key::Char(' '))
            } else {
                KeyPress::plain(Key::Char(ch))
            };
            edit::apply(&mut value, &mut cursor, &press, &limits);
        }
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
            Some(Overlay::Dialog(id)) if id == shell::TRAY && press.is_plain() && !self.focused_is_text() => {
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
                match edit::apply(&mut text, &mut cursor, press, &limits) {
                    Edited::No => false,
                    Edited::Moved => {
                        self.cursors.insert(item.key.clone(), cursor);
                        self.follow_area_cursor(&item.key);
                        true
                    }
                    Edited::Changed => {
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
            Action::Refresh => self.reload(),
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
                self.keep_preference("TUI_MOUSE", on);
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
                // A row that has fields to edit opens its editor; else its first link.
                let at = self.row_at(&item.key);
                if self.index.rows.contains_key(&(item.key.clone(), at)) {
                    self.edit_row();
                    return;
                }
                let rows = self.list_page(&item.key);
                let link = rows.get(at).and_then(row_link).map(str::to_string);
                if let Some(url) = link {
                    self.follow_url(&url);
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

    fn cancel(&mut self) {
        if let Some(top) = self.overlays.last() {
            if matches!(top, Overlay::Dialog(_) | Overlay::Find(_) | Overlay::Row(..)) {
                self.overlays.pop();
                self.drop_closed_extras();
                self.rebuild_focus();
            }
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
        if link.kind == "image" {
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

    /// Open a url: a screen in the client; an outside address is the browser's.
    fn follow_url(&mut self, url: &str) {
        if url.is_empty() {
            return;
        }
        if url.contains("://") || url.starts_with("mailto:") {
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

    /// The current screen again, with its parameters.
    pub fn reload(&mut self) {
        let location = self.nav.location();
        self.request_level(&location.path, location.parameters, Intent::Reload);
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
            path: path.to_string(),
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
        let missing = self.nav.set_leaf(model, parameters.clone());
        for path in missing {
            let call = LinkCall::Level {
                path: level_path(&path),
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
            self.extras.clear();
            self.fragments.clear();
            self.trees.clear();
            self.components.clear();
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

    /// The flash messages a transition left for this screen.
    fn show_model_messages(&mut self) {
        let messages = self.model().messages.clone();
        if !messages.errors.is_empty() {
            self.set_message(messages.errors.join("; "), true);
        } else if let Some(info) = messages.info.first() {
            self.set_message(info.clone(), false);
        }
    }

    fn apply_shell(&mut self, shell: ShellInfo) {
        let overrides: Vec<(&str, &str)> = shell.keys.iter().map(|(a, c)| (a.as_str(), c.as_str())).collect();
        let (keys, skips) = merge_keys(&KeyMap::baseline(), &overrides);
        self.keys = keys;
        self.key_skips = skips;
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
        let previous = self.focus_id().to_string();
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
            FocusKind::List => self
                .list_page(&item.key)
                .get(self.row_at(&item.key))
                .and_then(row_link)
                .map(str::to_string),
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

/// The drawer's rows as `(title, url, depth)`: each app menu item and the items under it.
pub fn drawer_rows(menu: &[MenuItem]) -> Vec<(String, String, usize)> {
    let mut rows = Vec::new();
    for item in menu {
        rows.push((item.title.clone(), item.url.clone(), 0));
        for child in &item.children {
            rows.push((child.title.clone(), child.url.clone(), 1));
        }
    }
    rows
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
