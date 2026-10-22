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

//! The agent mode (plans/archive/Screen-TUI-Impl.md Phase 3c, docs/TUI.md "For agents"): the same session, driven one command
//! line at a time by a program rather than a person. Each command runs its keys and requests on this thread, fires the
//! waits for typing to stop, and answers once the screen has settled: no timing to guess, no escape sequences to read.
//! The answer names the place, the focus, any overlay and the message, then gives the screen as text with the focused
//! element in `[[ ]]`; `--json` gives the same as one object, with the focusable elements. Elements are found by id,
//! by number, or by their label, so an agent never counts Tab presses.

use std::collections::BTreeMap;

use revue::layout::Rect;
use revue::render::{Buffer, Modifier};
use revue::widget::{RenderContext, View};
use serde_json::{json, Value};

use crate::forms::{parameter_keys, FocusItem, FocusKind, Scope};
use crate::keys::{Key, KeyPress, ACTIONS};
use crate::link::ScreenLink;
use crate::model::{walk, Widget};
use crate::session::{Overlay, Session};
use crate::state::{Confirm, ConfirmKind};
use crate::theme::Theme;

use super::AppView;

/// The login dialog's elements while it is on top: its three fields and its button.
const LOGIN_ELEMENTS: [(&str, &str); 4] = [
    ("login.username", "Username"),
    ("login.password", "Password"),
    ("login.code", "Code"),
    ("login.submit", "Log in"),
];

/// The commands, for `help` and for an unknown command's answer.
pub const AGENT_HELP: &str = "Commands (an element is its id, its number from `elements`, as #3, or text of its label):
  look                     the screen again
  key <chord>...           keys: F10 Enter Esc Tab Shift+Tab Up Down PgDn Space Ctrl+K Alt+M F4 ...
  type <text>              characters as typed; \\r is Enter, \\t is Tab
  go <path[?query]>        go to a screen
  focus <element>          move the focus to an element
  set <element> <value>    replace a field's value; a drop-down takes a choice's value or label
  do <element>             focus an element and press Enter: a button, link, list row, menu
  do Find [list]           open a list's find fields (set them, then do Find again to filter)
  find <text>              move to the text on the screen, a list row's included (as /)
  elements                 the focusable elements: number, kind, id, label, value
  size <cols>x<rows>       the drawing size (a tall one shows long lists whole)
  format text|json         how answers are written; --json on one command for that one
  help                     this list, and the screen's keys
  close                    end the session";

/// Where a session stands, for [`Agent::follow`]: the path and parameters, every field's value by buffer key, the open
/// dialogs and the focus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub path: String,
    pub parameters: BTreeMap<String, String>,
    pub values: BTreeMap<String, String>,
    pub dialogs: Vec<String>,
    pub focus: String,
}

/// A field whose value differs from what the screen came with ([`Agent::changed_fields`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangedField {
    pub dom_id: String,
    /// The request name the browser posts it by.
    pub name: String,
    pub label: String,
    pub key: String,
    pub original: String,
    pub proposed: String,
}

/// One command's answer: whether it did what was asked, and the text to write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAnswer {
    pub ok: bool,
    pub text: String,
}

pub struct Agent<L: ScreenLink> {
    view: AppView<L>,
    columns: u16,
    rows: u16,
    json: bool,
    closed: bool,
    /// Mark the elements that write with `[needs approval]`: an agent whose writes the server holds.
    marks_writes: bool,
    /// Notifications the link pushed since the last command, applied before the next answer.
    pushes: std::sync::mpsc::Receiver<crate::link::Push>,
    _subscription: Option<crate::link::Subscription>,
}

impl<L: ScreenLink> Agent<L> {
    /// A session on `path`, drawn at `columns` by `rows`, settled.
    pub fn open(link: L, path: &str, columns: u16, rows: u16) -> Self {
        let (push_tx, pushes) = std::sync::mpsc::channel();
        let subscription = link.subscribe(std::sync::Arc::new(move |push| {
            let _ = push_tx.send(push);
        }));
        let mut view = AppView::with_session(Session::start(path), link);
        // Plain text is all an agent reads; the theme only has to keep the focus in reverse video.
        view.set_theme(Theme::default());
        view.session_mut().resize(columns, rows);
        view.session_mut().set_pushed(subscription.is_some());
        let mut agent = Self {
            view,
            columns,
            rows,
            json: false,
            closed: false,
            marks_writes: false,
            pushes,
            _subscription: subscription,
        };
        agent.settle();
        agent
    }

    /// Answer in JSON from now on.
    pub fn set_json(&mut self, json: bool) {
        self.json = json;
    }

    /// Mark the buttons and links that write with `[needs approval]` in the text answers: for an agent whose renders
    /// hold every write for the user's approval (plans/LLM-A2A-Assist.md 4.7). JSON answers always say `writes`.
    pub fn set_marks_writes(&mut self, marks: bool) {
        self.marks_writes = marks;
    }

    /// The drawing size.
    pub fn drawing_size(&self) -> (u16, u16) {
        (self.columns, self.rows)
    }

    /// The user quit (Ctrl+Q) or sent `close`.
    pub fn closed(&self) -> bool {
        self.closed
    }

    pub fn session(&self) -> &Session {
        self.view.session()
    }

    /// The answer for the screen as it is: `look`.
    pub fn look(&self) -> AgentAnswer {
        self.answer(Ok(()), self.json)
    }

    /// Run one command line and answer it. A command that fails leaves the session as it was, as far as it got.
    pub fn run(&mut self, line: &str) -> AgentAnswer {
        let mut words: Vec<&str> = line.split_whitespace().collect();
        let json = match words.iter().position(|word| *word == "--json") {
            Some(at) => {
                words.remove(at);
                true
            }
            None => self.json,
        };
        let command = words.first().copied().unwrap_or("look");
        // The rest of the line after the command word, spaces kept: `type` and `set` values may hold them.
        let rest = line
            .trim_start()
            .strip_prefix(command)
            .unwrap_or("")
            .replace("--json", "")
            .trim()
            .to_string();
        if self.closed {
            return self.answer(Err("the session is closed".to_string()), json);
        }
        while let Ok(push) = self.pushes.try_recv() {
            self.view.session_mut().on_push(&push);
        }
        let result = match command {
            "look" => Ok(()),
            "key" | "keys" => self.keys(&words[1..]),
            "type" => {
                self.type_text(&unescape(&rest));
                Ok(())
            }
            "go" => self.go(&rest),
            "focus" => self.target(&rest).map(|key| {
                self.view.session_mut().focus_key(&key);
            }),
            "set" if self.login().is_some() => self.set_login(&rest),
            "set" => self.set(&rest),
            "do" if self.login().is_some() => self.do_login(&rest),
            "do" => match self.session().confirm().map(Confirm::choices) {
                // A confirm's choices are not elements: `do Yes` moves to the choice, then Enter.
                Some(choices) => self.confirm(choices, &rest),
                None => self.press_named(&rest),
            },
            "find" => {
                self.press(KeyPress::plain(Key::Char('/')));
                self.type_text(&rest);
                self.press(KeyPress::plain(Key::Enter));
                let message = self.session().message().to_string();
                if message.starts_with("Found") {
                    Ok(())
                } else {
                    Err(message)
                }
            }
            "elements" => {
                let text = self.elements_text();
                return if json {
                    self.answer(Ok(()), true)
                } else {
                    AgentAnswer {
                        ok: true,
                        text: format!("{text}\n--- ok"),
                    }
                };
            }
            "size" => self.size(&rest),
            "format" => match rest.as_str() {
                "json" => {
                    self.json = true;
                    Ok(())
                }
                "text" => {
                    self.json = false;
                    Ok(())
                }
                _ => Err("format is text or json".to_string()),
            },
            "help" => {
                let map = self.session().keys();
                let keys = ACTIONS
                    .into_iter()
                    .filter_map(|action| {
                        let chords: Vec<String> = map.chords(action).iter().map(|press| press.label()).collect();
                        (!chords.is_empty()).then(|| format!("  {:<16} {}", action.title(), chords.join(", ")))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                return AgentAnswer {
                    ok: true,
                    text: format!("{AGENT_HELP}\n\nKeys on this screen:\n{keys}\n--- ok"),
                };
            }
            "close" | "quit" => {
                self.closed = true;
                return AgentAnswer {
                    ok: true,
                    text: "closed\n--- ok".to_string(),
                };
            }
            other => Err(format!("unknown command {other}; try help")),
        };
        self.settle();
        self.answer(result, json)
    }

    /// Choose `wanted` in the open confirm dialog, by its text, and press Enter.
    fn confirm(&mut self, choices: &[&str], wanted: &str) -> Result<(), String> {
        let lower = wanted.to_lowercase();
        let Some(at) = choices.iter().position(|choice| choice.to_lowercase() == lower) else {
            return Err(format!("the confirm's choices are {}", choices.join(", ")));
        };
        for _ in 0..choices.len() {
            if self.session().confirm_choice() == Some(choices[at]) {
                break;
            }
            self.press(KeyPress::plain(Key::Right));
        }
        self.press(KeyPress::plain(Key::Enter));
        Ok(())
    }

    fn press(&mut self, press: KeyPress) {
        if self.view.on_key(&press) {
            self.closed = true;
        }
        self.settle();
    }

    fn keys(&mut self, chords: &[&str]) -> Result<(), String> {
        if chords.is_empty() {
            return Err("key needs at least one key".to_string());
        }
        let presses: Vec<KeyPress> = chords
            .iter()
            .map(|chord| KeyPress::parse(chord).ok_or_else(|| format!("no key {chord}")))
            .collect::<Result<_, _>>()?;
        for press in presses {
            self.press(press);
            if self.closed {
                break;
            }
        }
        Ok(())
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let key = match ch {
                '\r' | '\n' => Key::Enter,
                '\t' => Key::Tab,
                other => Key::Char(other),
            };
            self.press(KeyPress::plain(key));
        }
    }

    fn go(&mut self, path: &str) -> Result<(), String> {
        if !path.starts_with('/') {
            return Err("go takes a path from the root, as /apps/...".to_string());
        }
        self.view.session_mut().navigate(path, true);
        Ok(())
    }

    fn size(&mut self, text: &str) -> Result<(), String> {
        let (columns, rows) = text
            .split_once('x')
            .and_then(|(columns, rows)| Some((columns.parse::<u16>().ok()?, rows.parse::<u16>().ok()?)))
            .filter(|(columns, rows)| (40..=500).contains(columns) && (10..=500).contains(rows))
            .ok_or_else(|| "size is <cols>x<rows>, 40 to 500 by 10 to 500".to_string())?;
        self.columns = columns;
        self.rows = rows;
        self.view.session_mut().resize(columns, rows);
        Ok(())
    }

    /// `set <element> <value>`: the element is the longest leading run of words that names one, so a label with spaces
    /// works without quotes. When no run names one, the error is about the first word or words that name an element
    /// out of reach (a closed dialog's field, a list's find field), else about the whole line.
    fn set(&mut self, rest: &str) -> Result<(), String> {
        let words: Vec<&str> = rest.split_whitespace().collect();
        if words.len() < 2 {
            return Err("set takes an element and a value".to_string());
        }
        let found = (1..words.len()).rev().find_map(|split| {
            let key = self.resolve(&words[..split].join(" "))?;
            Some((key, words[split..].join(" ")))
        });
        let Some((key, value)) = found else {
            let hidden = (1..words.len()).rev().find_map(|split| self.hidden_place(&words[..split].join(" ")));
            // else the shortest leading words that are near a label, as the name meant
            let near = || {
                (1..words.len()).find_map(|split| {
                    let name = words[..split].join(" ");
                    (!self.suggest(&name).is_empty()).then(|| self.not_named(&name))
                })
            };
            return Err(hidden.or_else(near).unwrap_or_else(|| {
                format!("no element is named by the start of {rest:?}; write set <element> <value> with a label from elements")
            }));
        };
        self.set_key(&key, value)
    }

    /// Set one element, named whole (its label, id or #n), to `value`: the structured `set` of a caller that has the
    /// name and the value apart, so neither is guessed from the words.
    pub fn set_field(&mut self, element: &str, value: &str) -> AgentAnswer {
        if self.closed {
            return self.answer(Err("the session is closed".to_string()), self.json);
        }
        while let Ok(push) = self.pushes.try_recv() {
            self.view.session_mut().on_push(&push);
        }
        let result = if self.login().is_some() {
            self.set_login(&format!("{element} {value}"))
        } else {
            match self.resolve(element.trim()) {
                Some(key) => self.set_key(&key, value.to_string()),
                None => Err(self.not_here(element.trim())),
            }
        };
        self.settle();
        self.answer(result, self.json)
    }

    fn set_key(&mut self, key: &str, value: String) -> Result<(), String> {
        let key = key.to_string();
        let kind = self.item(&key).map(|item| item.kind);
        self.view.session_mut().focus_key(&key);
        match kind {
            Some(FocusKind::Text) => {
                self.view.session_mut().set_buffer(&key, value);
                Ok(())
            }
            Some(FocusKind::DropDown) => self.choose(&key, &value),
            Some(FocusKind::Check) => {
                if self.session().buffer(&key) != value {
                    self.press(KeyPress::plain(Key::Char(' ')));
                }
                Ok(())
            }
            _ => Err(format!("{key} is not a field to set; use do or key")),
        }
    }

    /// A drop-down's choice, as a person makes it: F4, the text to narrow the list, Enter. Then the value it took is
    /// checked against what was asked.
    fn choose(&mut self, key: &str, wanted: &str) -> Result<(), String> {
        self.press(KeyPress::plain(Key::F(4)));
        if !matches!(self.session().top_overlay(), Some(Overlay::DropDown(_))) {
            return Err(format!("{key} did not open its choices"));
        }
        self.type_text(wanted);
        let choices = self.session().dropdown_choices();
        let lower = wanted.to_lowercase();
        let Some(first) = choices.first().cloned() else {
            self.press(KeyPress::plain(Key::Escape));
            return Err(format!("no choice of {key} matches {wanted:?}"));
        };
        let exact = choices
            .iter()
            .position(|choice| choice.value == wanted || choice.label.to_lowercase() == lower);
        for _ in 0..exact.unwrap_or(0) {
            self.press(KeyPress::plain(Key::Down));
        }
        self.press(KeyPress::plain(Key::Enter));
        let took = self.session().buffer(key).to_string();
        let chosen = exact.and_then(|at| choices.get(at)).unwrap_or(&first);
        if took == chosen.value {
            Ok(())
        } else {
            Err(format!("{key} took {took:?}, not {wanted:?}"))
        }
    }

    fn item(&self, key: &str) -> Option<&FocusItem> {
        self.session().focus_order().iter().find(|item| item.key == key)
    }

    fn target(&self, text: &str) -> Result<String, String> {
        if text.is_empty() {
            return Err("name an element: its id, #n from elements, or its label".to_string());
        }
        self.resolve(text).ok_or_else(|| self.not_here(text))
    }

    /// `do <element>`: focus it and press Enter. `do Find` (or `do Find <list>`) where no element is named so opens a
    /// list's find fields, and in the open find it finds.
    fn press_named(&mut self, text: &str) -> Result<(), String> {
        let lower = text.trim().to_lowercase();
        let finding = matches!(self.session().top_overlay(), Some(Overlay::Find(_)));
        if finding && lower == "find" {
            return self.submit_find();
        }
        match self.resolve(text) {
            Some(key) => {
                self.view.session_mut().focus_key(&key);
                self.press(KeyPress::plain(Key::Enter));
                Ok(())
            }
            None if lower == "find" || lower.starts_with("find ") => self.open_find(text.trim()[4..].trim()),
            None => Err(self.not_here(text)),
        }
    }

    /// Open the find fields of the list `which` (its id or label), else of the first list that has them, as `f` on the
    /// list does.
    fn open_find(&mut self, which: &str) -> Result<(), String> {
        let index = self.session().index();
        let lists: Vec<(String, String)> = self
            .session()
            .focus_order()
            .iter()
            .filter(|item| item.kind == FocusKind::List)
            .filter(|item| index.lists.get(&item.key).is_some_and(|list| !list.header_fields.is_empty()))
            .map(|item| (item.key.clone(), self.label(item).to_lowercase()))
            .collect();
        let wanted = which.to_lowercase();
        let Some((list, _)) = lists
            .iter()
            .find(|(id, label)| wanted.is_empty() || id.to_lowercase() == wanted || label.contains(&wanted))
            .cloned()
        else {
            return Err(if lists.is_empty() {
                "no list here has find fields".to_string()
            } else {
                format!("no list {which:?} has find fields; the lists with find fields are {}",
                    lists.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", "))
            });
        };
        self.view.session_mut().focus_key(&list);
        self.press(KeyPress::plain(Key::Char('f')));
        if matches!(self.session().top_overlay(), Some(Overlay::Find(_))) {
            Ok(())
        } else {
            Err(format!("the find of {list} did not open"))
        }
    }

    /// In the open find, find: Enter in its first field (or its button), which reloads the list with the fields.
    fn submit_find(&mut self) -> Result<(), String> {
        let key = self
            .session()
            .focus_order()
            .iter()
            .find(|item| matches!(item.kind, FocusKind::Text | FocusKind::Button | FocusKind::Check))
            .map(|item| item.key.clone())
            .ok_or_else(|| "the find has no field to find from".to_string())?;
        self.view.session_mut().focus_key(&key);
        self.press(KeyPress::plain(Key::Enter));
        Ok(())
    }

    /// The error for a name that finds no element here: where it is when it is out of reach, else the nearest.
    fn not_here(&self, text: &str) -> String {
        self.hidden_place(text).unwrap_or_else(|| self.not_named(text))
    }

    /// Where an element named `text` is when it is not in reach now: a field of a closed dialog (open it with the
    /// dialog's button), or a find field of a list (open the find first).
    fn hidden_place(&self, text: &str) -> Option<String> {
        let wanted = text.trim().to_lowercase();
        if wanted.is_empty() {
            return None;
        }
        let session = self.session();
        let index = session.index();
        let open = session.dialogs_open();
        let finding = match session.top_overlay() {
            Some(Overlay::Find(list)) => Some(list.as_str()),
            _ => None,
        };
        let tidy = |label: String| label.trim_end_matches('…').trim().to_lowercase();
        for item in &index.order {
            if tidy(self.label(item)) != wanted && item.key.to_lowercase() != wanted {
                continue;
            }
            match &item.scope {
                Scope::Dialog(id) if !open.contains(&id.as_str()) => {
                    let title = index.dialogs.get(id).cloned().unwrap_or_else(|| id.clone());
                    let button = index
                        .order
                        .iter()
                        .find(|candidate| candidate.kind == FocusKind::Dialog && candidate.key == *id)
                        .map(|candidate| self.label(candidate).trim_end_matches('…').trim().to_string())
                        .filter(|label| !label.is_empty())
                        .unwrap_or_else(|| title.clone());
                    return Some(format!(
                        "{text} is a field of the dialog \"{title}\", which is closed; open it first with do \"{button}\", then set it"
                    ));
                }
                Scope::Find(list) if finding != Some(list.as_str()) => {
                    return Some(format!(
                        "{text} is a find field of the list {list}; do Find to open its find, set it, then do Find to filter"
                    ));
                }
                _ => {}
            }
        }
        None
    }

    /// The error for a name that finds no element, with the nearest labels when some are near.
    fn not_named(&self, text: &str) -> String {
        let near = self.suggest(text);
        if near.is_empty() {
            format!("no element named {text:?}; see elements")
        } else {
            format!("no element named {text:?}; did you mean: {}?", near.join(", "))
        }
    }

    /// Up to three elements whose labels are nearest to `text`, as `Label (kind)`: by edit distance over the lowercase
    /// labels, a shared start counting as nearer, none further than about half the text.
    pub fn suggest(&self, text: &str) -> Vec<String> {
        let wanted = text.trim().to_lowercase();
        if wanted.is_empty() {
            return Vec::new();
        }
        let limit = (wanted.chars().count() / 2).max(2);
        let mut scored: Vec<(usize, String)> = Vec::new();
        for item in self.session().focus_order() {
            let label = self.label(item);
            let lower = label.trim_end_matches('…').trim().to_lowercase();
            if lower.is_empty() {
                continue;
            }
            let distance = edit_distance(&wanted, &lower);
            let prefix = lower.starts_with(&wanted) || wanted.starts_with(&lower);
            let shared = lower
                .split_whitespace()
                .any(|word| wanted.split_whitespace().any(|w| w == word));
            if distance > limit && !prefix && !shared {
                continue;
            }
            let score = distance
                .saturating_sub(if prefix { limit } else { 0 })
                .saturating_sub(usize::from(shared));
            let shown = format!("{label} ({})", Self::kind_name(item.kind));
            if !scored.iter().any(|(_, seen)| *seen == shown) {
                scored.push((score, shown));
            }
        }
        scored.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        scored.into_iter().take(3).map(|(_, shown)| shown).collect()
    }

    /// An element by `#n`, by its key, or by its label: the same label first, then one that holds the text.
    fn resolve(&self, text: &str) -> Option<String> {
        let order = self.session().focus_order();
        if let Some(n) = text.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
            return order.get(n.checked_sub(1)?).map(|item| item.key.clone());
        }
        if let Some(item) = order.iter().find(|item| item.key == text) {
            return Some(item.key.clone());
        }
        let lower = text.to_lowercase();
        let labelled: Vec<(String, String)> = order
            .iter()
            .map(|item| (item.key.clone(), self.label(item).to_lowercase()))
            .collect();
        let tidy = |label: &str| label.trim_end_matches('…').trim().to_string();
        labelled
            .iter()
            .find(|(_, label)| tidy(label) == lower)
            .or_else(|| labelled.iter().find(|(key, _)| key.to_lowercase() == lower))
            .or_else(|| labelled.iter().find(|(_, label)| label.contains(&lower)))
            .map(|(key, _)| key.clone())
    }

    /// What an element says: a field's title, a link's or button's text, a dialog's title, a menu's text, else the text
    /// of its node.
    fn label(&self, item: &FocusItem) -> String {
        let index = self.session().index();
        let part = item.key.split_once('#').map(|(_, part)| part);
        let found = match item.kind {
            FocusKind::Text | FocusKind::DropDown | FocusKind::Check | FocusKind::Radio => {
                index.fields.get(&item.node).map(|field| match part {
                    Some(part) => format!("{} ({part})", field.title),
                    None => field.title.clone(),
                })
            }
            FocusKind::Link | FocusKind::Button => index.links.get(&item.key).map(|link| link.text.clone()),
            FocusKind::Dialog => index.dialogs.get(&item.key).cloned(),
            FocusKind::Menu => index.menus.get(&item.key).map(|menu| menu.text.clone()),
            _ => None,
        };
        found
            .filter(|label| !label.is_empty())
            .or_else(|| self.node_text(&item.node))
            .unwrap_or_else(|| item.key.clone())
    }

    fn node_text(&self, id: &str) -> Option<String> {
        let mut found = None;
        for model in self.session().nav().models() {
            walk(&model.c, &mut |node| {
                if found.is_none() && node.id() == id {
                    found = serde_json::to_value(node).ok();
                }
            });
        }
        let node = found?;
        ["text", "button", "title", "name"]
            .into_iter()
            .find_map(|name| node.get(name).and_then(Value::as_str).filter(|text| !text.is_empty()))
            .map(str::to_string)
    }

    fn kind_name(kind: FocusKind) -> &'static str {
        match kind {
            FocusKind::Text => "text",
            FocusKind::DropDown => "drop-down",
            FocusKind::Check => "check",
            FocusKind::Radio => "radio",
            FocusKind::Button => "button",
            FocusKind::Link => "link",
            FocusKind::List => "list",
            FocusKind::Dialog => "dialog",
            FocusKind::Menu => "menu",
            FocusKind::Tree => "tree",
            FocusKind::Toggle => "toggle",
            FocusKind::Edit => "edit",
            FocusKind::Component => "component",
        }
    }

    fn element(&self, n: usize, item: &FocusItem) -> Value {
        let session = self.session();
        let value = match item.kind {
            FocusKind::Text | FocusKind::DropDown | FocusKind::Check | FocusKind::Radio => {
                Some(session.buffer(&item.key).to_string())
            }
            _ => None,
        };
        let mut element = json!({
            "n": n,
            "kind": Self::kind_name(item.kind),
            "id": item.key,
            "label": self.label(item),
            "focused": item.key == session.focus_id(),
        });
        if let Some(value) = value {
            element["value"] = json!(value);
            if session.is_dirty(&item.key) {
                element["changed"] = json!(true);
            }
            if let Some(mark) = session.mark(&item.key) {
                element["error"] = json!(mark);
            }
            if let Some(field) = session.index().fields.get(&item.node) {
                if field.required {
                    element["required"] = json!(true);
                }
            }
        }
        if item.kind == FocusKind::List {
            if let Some(list) = session.list(&item.key) {
                element["rows"] = json!(list.rows.len());
                if !list.header_fields.is_empty() {
                    element["find"] = json!(true);
                }
            }
        }
        if self.writes(item) {
            element["writes"] = json!(true);
        }
        element
    }

    /// Whether pressing the element runs a transition that writes: a link to one, or a form's submit button whose
    /// form posts to one.
    pub fn writes(&self, item: &FocusItem) -> bool {
        let index = self.session().index();
        match item.kind {
            FocusKind::Link => index.links.get(&item.key).is_some_and(|link| link.writes),
            FocusKind::Button => {
                if let Some(link) = index.links.get(&item.key) {
                    return link.writes;
                }
                let submits = index
                    .fields
                    .get(&item.node)
                    .is_some_and(|field| matches!(field.widget, Widget::Submit { .. }));
                submits && index.form_of(&item.node).is_some_and(|form| form.writes)
            }
            _ => false,
        }
    }

    fn elements(&self) -> Vec<Value> {
        if let Some(login) = self.login() {
            return LOGIN_ELEMENTS
                .iter()
                .enumerate()
                .map(|(at, (id, label))| {
                    let mut element = json!({
                        "n": at + 1,
                        "kind": if at < 3 { "text" } else { "button" },
                        "id": id,
                        "label": label,
                        "focused": login.at == at,
                    });
                    match at {
                        0 => element["value"] = json!(login.username),
                        // The password is never in an answer: its length, as the dialog draws it.
                        1 => element["value"] = json!("•".repeat(login.password.chars().count())),
                        2 => element["value"] = json!(login.code),
                        _ => {}
                    }
                    element
                })
                .collect();
        }
        self.session()
            .focus_order()
            .iter()
            .enumerate()
            .map(|(at, item)| self.element(at + 1, item))
            .collect()
    }

    fn elements_text(&self) -> String {
        self.elements()
            .iter()
            .map(|element| {
                let mut line = format!(
                    "{}#{:<3} {:<9} {:<28} {}",
                    if element["focused"] == json!(true) { ">" } else { " " },
                    element["n"],
                    element["kind"].as_str().unwrap_or(""),
                    element["id"].as_str().unwrap_or(""),
                    element["label"].as_str().unwrap_or("")
                );
                if let Some(value) = element.get("value").and_then(Value::as_str) {
                    line.push_str(&format!(" = {value:?}"));
                }
                for flag in ["required", "changed"] {
                    if element.get(flag).is_some() {
                        line.push_str(&format!(" [{flag}]"));
                    }
                }
                if self.marks_writes && element.get("writes").is_some() {
                    line.push_str(" [needs approval]");
                }
                if let Some(error) = element.get("error").and_then(Value::as_str) {
                    line.push_str(&format!(" [error: {error}]"));
                }
                if let Some(rows) = element.get("rows") {
                    line.push_str(&format!(" ({rows} rows)"));
                }
                if element.get("find").is_some() {
                    line.push_str(" [find: do Find]");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn login(&self) -> Option<&crate::session::Login> {
        match self.session().top_overlay() {
            Some(Overlay::Login(login)) => Some(login),
            _ => None,
        }
    }

    /// One of [`LOGIN_ELEMENTS`] by `#n`, its id or its label (the same label, then one that holds the text).
    fn login_element(text: &str) -> Option<usize> {
        if let Some(n) = text.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
            return n.checked_sub(1).filter(|at| *at < LOGIN_ELEMENTS.len());
        }
        let lower = text.trim().to_lowercase();
        LOGIN_ELEMENTS
            .iter()
            .position(|(id, label)| *id == lower || label.to_lowercase() == lower)
            .or_else(|| {
                LOGIN_ELEMENTS
                    .iter()
                    .position(|(_, label)| !lower.is_empty() && label.to_lowercase().contains(&lower))
            })
    }

    /// `set` in the login dialog: `set Password secret`.
    fn set_login(&mut self, rest: &str) -> Result<(), String> {
        let (name, value) = rest
            .trim()
            .split_once(char::is_whitespace)
            .ok_or_else(|| "set takes an element and a value".to_string())?;
        match Self::login_element(name) {
            Some(at) if at < 3 => {
                self.view.session_mut().set_login_field(at, value.trim());
                Ok(())
            }
            _ => Err(format!("no login field named {name:?}; see elements")),
        }
    }

    /// `do Log in` sends the dialog; `do` on a field moves to it.
    fn do_login(&mut self, rest: &str) -> Result<(), String> {
        let at = Self::login_element(rest).ok_or_else(|| format!("no login element named {rest:?}; see elements"))?;
        if at == 3 {
            self.press(KeyPress::plain(Key::Enter));
            self.settle();
            return match self.login() {
                Some(login) if !login.error.is_empty() => Err(login.error.clone()),
                _ => Ok(()),
            };
        }
        let value = match (self.login(), at) {
            (Some(login), 0) => login.username.clone(),
            (Some(login), 1) => login.password.clone(),
            (Some(login), _) => login.code.clone(),
            (None, _) => String::new(),
        };
        self.view.session_mut().set_login_field(at, &value);
        Ok(())
    }

    /// Put values the user typed in the browser into the form the browser names `dom_id` (its qvt id: a form's
    /// `domId`, a list row's `<list>_<row>`, a list's find `<list>_header`), by the request names the browser posts;
    /// dependent fields follow, and the session settles. The names no field of that form takes are given back.
    ///
    /// # Errors
    ///
    /// No form on the screen has that id.
    pub fn set_values(&mut self, dom_id: &str, values: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
        let keys = self
            .form_keys(dom_id)
            .ok_or_else(|| format!("no form {dom_id:?} on this screen"))?;
        let mut ignored = Vec::new();
        for (name, value) in values {
            match keys.get(name) {
                Some(key) if self.session().buffer(key) != value => {
                    self.view.session_mut().set_buffer(key, value.clone());
                }
                Some(_) => {}
                None => ignored.push(name.clone()),
            }
        }
        self.settle();
        Ok(ignored)
    }

    /// The request names of a form's fields with their buffer keys, by the form's qvt id.
    fn form_keys(&self, dom_id: &str) -> Option<BTreeMap<String, String>> {
        let index = self.session().index();
        let fields: Vec<&crate::forms::FieldInfo> =
            if let Some(form) = index.forms.values().find(|form| form.dom_id == dom_id) {
                form.fields.iter().filter_map(|id| index.fields.get(id)).collect()
            } else {
                let list = dom_id.strip_suffix("_header").and_then(|list| {
                    index
                        .lists
                        .values()
                        .find(|item| !item.dom_id.is_empty() && item.dom_id == list)
                })?;
                index
                    .fields
                    .values()
                    .filter(|field| field.list.as_deref() == Some(list.id.as_str()))
                    .collect()
            };
        Some(
            fields
                .into_iter()
                .filter(|field| !matches!(field.widget, Widget::Password {} | Widget::File { .. }))
                .flat_map(parameter_keys)
                .collect(),
        )
    }

    /// The values of the forms that differ from what the screen came with, by form `dom_id` and request name: what a
    /// later session needs to stand where this one stands. A password and a file are left out.
    pub fn unsaved(&self) -> BTreeMap<String, BTreeMap<String, String>> {
        let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for change in self.changed_fields() {
            out.entry(change.dom_id)
                .or_default()
                .insert(change.name, change.proposed);
        }
        out
    }

    /// Each field value that differs from what the screen came with: its form's `dom_id`, request name, label, buffer
    /// key, the original and the value now. A password and a file are left out, and so is a form with no `dom_id`.
    pub fn changed_fields(&self) -> Vec<ChangedField> {
        let session = self.session();
        let index = session.index();
        let mut out = Vec::new();
        for field in index.fields.values() {
            if matches!(field.widget, Widget::Password {} | Widget::File { .. }) {
                continue;
            }
            let Some(dom_id) = self.dom_id_of(field) else {
                continue;
            };
            for (name, key) in parameter_keys(field) {
                if session.is_dirty(&key) {
                    out.push(ChangedField {
                        dom_id: dom_id.clone(),
                        name,
                        label: field.title.clone(),
                        original: session.original(&key).to_string(),
                        proposed: session.buffer(&key).to_string(),
                        key,
                    });
                }
            }
        }
        out
    }

    /// The value the screen came with for the request name `name` of the form `dom_id`.
    pub fn original_value(&self, dom_id: &str, name: &str) -> Option<String> {
        let keys = self.form_keys(dom_id)?;
        let key = keys.get(name)?;
        Some(self.session().original(key).to_string())
    }

    /// The qvt id of the form a field posts with, or of its list's find form.
    fn dom_id_of(&self, field: &crate::forms::FieldInfo) -> Option<String> {
        let index = self.session().index();
        if let Some(form) = field.form.as_ref().and_then(|form| index.forms.get(form)) {
            return (!form.dom_id.is_empty()).then(|| form.dom_id.clone());
        }
        let list = field.list.as_ref().and_then(|list| index.lists.get(list))?;
        (!list.dom_id.is_empty()).then(|| format!("{}_header", list.dom_id))
    }

    /// Where the session stands, to compare after a command ([`Self::follow`]).
    pub fn snapshot(&self) -> Snapshot {
        let session = self.session();
        let index = session.index();
        let values = index
            .fields
            .values()
            .flat_map(crate::forms::field_keys)
            .map(|key| {
                let value = session.buffer(&key).to_string();
                (key, value)
            })
            .collect();
        Snapshot {
            path: session.path().to_string(),
            parameters: session.parameters().clone(),
            values,
            dialogs: session.dialogs_open().into_iter().map(str::to_string).collect(),
            focus: session.focus_id().to_string(),
        }
    }

    /// What the user's page does to follow this session since `before` (plans/LLM-A2A-Assist.md 4.7), as
    /// `(event type, payload)`: `ui.navigate` when the path or its parameters changed, `ui.fill` for each value that
    /// changed (after a navigation, each one not as the screen came), `ui.dialog` for each dialog opened or closed,
    /// and `ui.focus` when the focus moved to another element. A list row's field follows only through the navigation
    /// after its save; a password, a file and a hidden field never follow.
    pub fn follow(&self, before: &Snapshot) -> Vec<(String, Value)> {
        let session = self.session();
        let index = session.index();
        let mut events = Vec::new();
        let navigated = session.path() != before.path || session.parameters() != &before.parameters;
        if navigated {
            events.push((
                "ui.navigate".to_string(),
                json!({"path": session.path(), "parameters": session.parameters(), "title": session.crumbs().join(" ▸ ")}),
            ));
        }
        let forms = self.form_order();
        for field in index.fields.values() {
            if matches!(
                field.widget,
                Widget::Password {} | Widget::File { .. } | Widget::Hidden { .. } | Widget::Display { .. }
            ) {
                continue;
            }
            let scope = index
                .order
                .iter()
                .find(|item| item.node == field.id)
                .map(|item| item.scope.clone())
                .unwrap_or_default();
            if matches!(scope, Scope::Row(..)) {
                continue;
            }
            for (name, key) in parameter_keys(field) {
                let value = session.buffer(&key);
                let changed = if navigated {
                    session.is_dirty(&key)
                } else {
                    before.values.get(&key).map(String::as_str) != Some(value)
                };
                if !changed {
                    continue;
                }
                let mut fill = self.field_place(field, &scope, &forms);
                fill["field"] = json!(name);
                fill["part"] = json!(key.split_once('#').map(|(_, part)| part));
                fill["value"] = json!(value);
                fill["label"] = json!(field.title);
                fill["tuiId"] = json!(key);
                events.push(("ui.fill".to_string(), fill));
            }
        }
        let open = session.dialogs_open();
        for id in &before.dialogs {
            if !open.contains(&id.as_str()) {
                events.push(("ui.dialog".to_string(), self.dialog_event(id, false)));
            }
        }
        for id in open {
            if !before.dialogs.iter().any(|seen| seen == id) {
                events.push(("ui.dialog".to_string(), self.dialog_event(id, true)));
            }
        }
        if !navigated && session.focus_id() != before.focus && !session.focus_id().is_empty() {
            if let Some(target) = self.focus_target() {
                events.push(("ui.focus".to_string(), target));
            }
        }
        events
    }

    fn dialog_event(&self, id: &str, open: bool) -> Value {
        json!({"id": id, "title": self.session().index().dialogs.get(id).cloned().unwrap_or_default(), "open": open})
    }

    /// The focused element as a follow event names it: its tui id, kind and label, and for a field its form and
    /// request name.
    pub fn focus_target(&self) -> Option<Value> {
        let session = self.session();
        let item = session.focus()?;
        let mut target = json!({"tuiId": item.key, "kind": Self::kind_name(item.kind), "label": self.label(item)});
        if let Some(field) = session.index().fields.get(&item.node) {
            let place = self.field_place(field, &item.scope, &self.form_order());
            for name in ["scope", "form", "domId", "occurrence", "row"] {
                target[name] = place[name].clone();
            }
            target["field"] = json!(parameter_keys(field)
                .into_iter()
                .find(|(_, key)| *key == item.key)
                .map(|(name, _)| name)
                .unwrap_or_else(|| field.name.clone()));
        }
        Some(target)
    }

    /// The forms in reading order, by id.
    fn form_order(&self) -> Vec<String> {
        let index = self.session().index();
        let mut order: Vec<String> = Vec::new();
        for item in &index.order {
            let form = index.fields.get(&item.node).and_then(|field| field.form.clone());
            if let Some(form) = form {
                if !order.contains(&form) {
                    order.push(form);
                }
            }
        }
        order
    }

    /// Where a field is, as a fill names it: the scope (`page`, `dialog`, `find`), the form's name, its qvt id and its
    /// occurrence among the forms of that name in reading order, and a list row.
    fn field_place(&self, field: &crate::forms::FieldInfo, scope: &Scope, forms: &[String]) -> Value {
        let index = self.session().index();
        let (scope_name, dialog, row) = match scope {
            Scope::Main => ("page", None, None),
            Scope::Dialog(id) => ("dialog", Some(id.clone()), None),
            Scope::Find(_) => ("find", None, None),
            Scope::Row(_, row) => ("row", None, Some(*row)),
        };
        let mut place = json!({"scope": scope_name, "row": row});
        if let Some(dialog) = dialog {
            place["dialog"] = json!(dialog);
        }
        if let Some(form) = field.form.as_ref().and_then(|form| index.forms.get(form)) {
            let occurrence = forms
                .iter()
                .filter_map(|id| index.forms.get(id))
                .take_while(|other| other.id != form.id)
                .filter(|other| other.name == form.name)
                .count();
            place["form"] = json!(form.name);
            place["domId"] = json!(form.dom_id);
            place["occurrence"] = json!(occurrence);
        } else if let Some(list) = field.list.as_ref().and_then(|list| index.lists.get(list)) {
            place["form"] = json!(format!("{}_header", list.name));
            place["domId"] = json!(if list.dom_id.is_empty() {
                String::new()
            } else {
                format!("{}_header", list.dom_id)
            });
            place["occurrence"] = json!(0);
        }
        place
    }

    /// Run what the keys queued, and the waits for typing to stop, until nothing is left to do.
    fn settle(&mut self) {
        for _ in 0..16 {
            self.view.run_requests();
            if !self.view.session_mut().fire_waits() {
                break;
            }
        }
    }

    fn overlay(&self) -> Option<String> {
        let session = self.session();
        let overlay = session.top_overlay()?;
        Some(match overlay {
            Overlay::Dialog(id) => format!(
                "dialog {:?}",
                session.index().dialogs.get(id).cloned().unwrap_or_else(|| id.clone())
            ),
            Overlay::Find(list) => format!("find {list:?}"),
            Overlay::DropDown(drop) => format!("drop-down {:?} filter {:?}", drop.field, drop.filter),
            Overlay::Picker(picker) => format!("date picker {:?}", picker.key),
            Overlay::Confirm(confirm) => format!(
                "confirm {:?}, choices {}, at {:?}",
                match &confirm.kind {
                    ConfirmKind::Ask(text) => text.as_str(),
                    ConfirmKind::Leave => "leave with unsaved changes?",
                    ConfirmKind::Approval(card) => card.lines.first().map_or("approve?", String::as_str),
                },
                confirm.choices().join("|"),
                confirm.choice()
            ),
            Overlay::Help => "help".to_string(),
            Overlay::Info => "info".to_string(),
            Overlay::Login(_) => "login".to_string(),
            Overlay::Drawer(_) => "drawer".to_string(),
            Overlay::Menu(menu) => format!(
                "menu, at {:?}",
                menu.items.get(menu.at).map(|item| item.label.as_str()).unwrap_or("")
            ),
            Overlay::Palette(palette) => format!("palette {:?}", palette.text),
            Overlay::Search(text) => format!("find text {text:?}"),
            Overlay::Messages => "messages".to_string(),
            Overlay::Editor(_) => "editor".to_string(),
            Overlay::Row(list, row) => format!("row editor {list:?} row {}", row + 1),
            Overlay::Columns(_) => "columns".to_string(),
            Overlay::Prompt(_) => "prompt".to_string(),
        })
    }

    /// The screen as text, with the cells drawn in reverse video (the focus) between `[[` and `]]`.
    fn screen_lines(&self) -> Vec<String> {
        let (columns, rows) = (self.columns, self.rows);
        let mut buffer = Buffer::new(columns, rows);
        {
            let mut ctx = RenderContext::new(&mut buffer, Rect::new(0, 0, columns, rows));
            self.view.render(&mut ctx);
        }
        let mut lines: Vec<String> = (0..rows)
            .map(|y| {
                let mut line = String::new();
                let mut reversed = false;
                for x in 0..columns {
                    let Some(cell) = buffer.get(x, y) else {
                        continue;
                    };
                    if cell.is_continuation() {
                        continue;
                    }
                    let reverse = cell.modifier.contains(Modifier::REVERSE);
                    if reverse != reversed {
                        line.push_str(if reverse { "[[" } else { "]]" });
                        reversed = reverse;
                    }
                    line.push(cell.symbol);
                }
                if reversed {
                    line.push_str("]]");
                }
                line.trim_end().to_string()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines
    }

    fn answer(&self, result: Result<(), String>, json: bool) -> AgentAnswer {
        let session = self.session();
        let focus = session
            .focus()
            .map(|item| (Self::kind_name(item.kind), item.key.clone(), self.label(item)));
        let message = session.message().to_string();
        let message_error = session.message_is_error();
        let ok = result.is_ok();
        if json {
            let value = json!({
                "ok": ok,
                "reason": result.as_ref().err(),
                "path": session.path(),
                "parameters": session.parameters(),
                "title": session.crumbs().join(" ▸ "),
                "focus": focus.as_ref().map(|(kind, id, label)| json!({"kind": kind, "id": id, "label": label})),
                "overlay": self.overlay(),
                "message": message,
                "messageError": message_error,
                "screen": self.screen_lines(),
                "elements": self.elements(),
            });
            return AgentAnswer {
                ok,
                text: value.to_string(),
            };
        }
        let mut out = Vec::new();
        out.push(format!(
            "path {}",
            path_with_query(session.path(), session.parameters())
        ));
        out.push(format!("title {}", session.crumbs().join(" ▸ ")));
        let focus_writes = self.marks_writes && session.focus().is_some_and(|item| self.writes(item));
        out.push(match &focus {
            Some((kind, id, label)) if focus_writes => format!("focus {kind} {id} {label:?} [needs approval]"),
            Some((kind, id, label)) => format!("focus {kind} {id} {label:?}"),
            None => "focus none".to_string(),
        });
        out.push(format!(
            "overlay {}",
            self.overlay().unwrap_or_else(|| "none".to_string())
        ));
        if !message.is_empty() {
            out.push(format!("{} {message}", if message_error { "error" } else { "message" }));
        }
        if self.marks_writes {
            let mut writing: Vec<String> = Vec::new();
            for item in session.focus_order().iter().filter(|item| self.writes(item)) {
                let label = self.label(item);
                if !writing.contains(&label) {
                    writing.push(label);
                }
            }
            if !writing.is_empty() {
                let more = writing.len().saturating_sub(WRITES_SHOWN);
                writing.truncate(WRITES_SHOWN);
                let tail = if more > 0 {
                    format!(" and {more} more")
                } else {
                    String::new()
                };
                out.push(format!("[needs approval] {}{tail}", writing.join(", ")));
            }
        }
        out.extend(collapse_blank(self.screen_lines()));
        out.push(match result {
            Ok(()) => "--- ok".to_string(),
            Err(reason) => format!("--- error {reason}"),
        });
        AgentAnswer {
            ok,
            text: out.join("\n"),
        }
    }
}

/// The writing elements a text answer names on its `[needs approval]` line.
const WRITES_SHOWN: usize = 8;

/// The Levenshtein distance of two texts, by characters.
fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if a == *b {
                previous
            } else {
                1 + previous.min(above).min(row[j])
            };
            previous = above;
        }
    }
    row[right.len()]
}

/// Runs of three or more empty rows as one line that counts them: they say nothing and cost an agent tokens. JSON
/// keeps every row, so a program can index them.
fn collapse_blank(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut blank = 0;
    let flush = |out: &mut Vec<String>, blank: usize| match blank {
        0 => {}
        1 | 2 => out.extend(std::iter::repeat_n(String::new(), blank)),
        n => out.push(format!("  ({n} empty rows)")),
    };
    for line in lines {
        if line.is_empty() {
            blank += 1;
            continue;
        }
        flush(&mut out, blank);
        blank = 0;
        out.push(line);
    }
    flush(&mut out, blank);
    out
}

fn path_with_query(path: &str, parameters: &BTreeMap<String, String>) -> String {
    if parameters.is_empty() {
        return path.to_string();
    }
    let query: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("{path}?{}", query.join("&"))
}

/// `\r`, `\n`, `\t` and `\\` in a `type` line.
fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}
