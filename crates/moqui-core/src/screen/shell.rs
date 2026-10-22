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

//! The suite shell of `/qapps` beyond the menu of one path (no Java counterpart). `sri.getShellData` answers the
//! `shellData` transition of the root screen, which the shell fetches once per page load: the apps the user may open
//! with their menus to depth two and their `<shell-providers>`, the shell's theme values and the first badge counts.
//! `sri.getShellSearch` answers `shellSearch`, the palette groups of the `palette-source` providers.
//!
//! The apps are the subscreens of the apps root in the set and order of the app list, each authorized as the menu
//! data authorizes a subscreen; the palette runs only the source services those apps register, never a name from the
//! request. An app root that does not load, or whose providers fail, is logged and left out, and a palette source that
//! fails is logged and its group left out (AGENTS.md failure tolerance). The JSON is described in
//! `webroot/screen/webroot/js/README.md` ("Shell data and providers").

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::string::url_encode_if_needed;

use super::definition::{ScreenDef, SubscreensItem};
use super::menu;
use super::render::RenderState;
use super::sri::{self, DEFAULT_QUASAR_THEME, QUASAR_THEME_TYPE};
use super::url::ScreenUrlInfo;
use crate::context::{Error, ExecutionContext};
use crate::{l10n, user, web};

type Item = IndexMap<String, Value>;

/// The apps root under the root screen, and the link base of the `/qapps` page.
const APPS_PATH: &str = "apps";
const LINK_BASE: &str = "/qapps";
/// The menu depth below an app root that the shell data lists (the palette's Screens group).
const MENU_DEPTH: usize = 2;
/// Rows of a palette group when the request gives no limit, and the most it may ask for.
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 50;

/// The `shell` keys and the theme value types of the user's qapps theme behind them. A screen path (true) goes to the
/// client as a link the user may open, or null; a subscreen name (false) as it is written, and per app as a
/// `recordScreens` entry ([`RECORD_SCREENS`]).
const SHELL_VALUES: [(&str, &str, bool); 8] = [
    ("trayToday", "STRT_TRAY_TODAY", true),
    ("trayChat", "STRT_TRAY_CHAT", true),
    ("trayNotifications", "STRT_TRAY_NOTIFICATIONS", true),
    ("quickCreate", "STRT_QUICK_CREATE", true),
    ("mailScreen", "STRT_MAIL_SCREEN", true),
    ("messagesScreen", "STRT_MESSAGES_SCREEN", true),
    ("chatterScreen", "STRT_CHATTER_SCREEN", false),
    ("recordFilesScreen", "STRT_RECORD_FILES_SCREEN", false),
];

/// The keys of an app's `recordScreens` and the theme value types that name the subscreen every app root mounts for
/// them (R5): the `chatter` and `record-files` elements load it under the current app root.
const RECORD_SCREENS: [(&str, &str); 2] = [
    ("chatter", "STRT_CHATTER_SCREEN"),
    ("recordFiles", "STRT_RECORD_FILES_SCREEN"),
];

// ---- the element ----

/// A provider that opens a screen: a `quick-create` kind, a `palette-action`, an `account-menu-item`.
#[derive(Clone, Debug, Default)]
pub struct ShellLink {
    /// The `kind` of a `quick-create`.
    pub kind: String,
    pub label: String,
    pub icon: String,
    /// A path under the apps root with an optional query string. Empty on a quick-create kind that opens the theme's
    /// `STRT_QUICK_CREATE` screen with `kind` as a parameter.
    pub screen: String,
    /// The `parameter` children: `name` and `value` as written.
    pub parameters: Vec<(String, String)>,
    pub sequence: Option<i64>,
    /// `requires-record` of a quick-create kind: offered only while a record is in the screen context.
    pub requires_record: bool,
}

/// A `today-source`: a service the Today panel calls.
#[derive(Clone, Debug, Default)]
pub struct TodaySource {
    pub service: String,
    pub label: String,
}

/// A `palette-source`: a palette group answered by a service, which stays on the server.
#[derive(Clone, Debug, Default)]
pub struct PaletteSource {
    pub group: String,
    pub label: String,
    pub icon: String,
    pub service: String,
    pub prefix: String,
    /// A regular expression the whole search text must match for the source to run, then with `idMatch`.
    pub id_pattern: String,
    pub sequence: Option<i64>,
}

/// A `key` of a `keymap`: the keys, what the help overlay shows, and the screen it opens or the client action it runs.
#[derive(Clone, Debug, Default)]
pub struct ShellKey {
    pub keys: String,
    pub label: String,
    pub screen: String,
    pub action: String,
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    pub scope: String,
    pub keys: Vec<ShellKey>,
}

/// An `assist-prompt`: a suggested prompt of the Assist drawer (plans/LLM-A2A-Assist.md 4.11) on the app's pages, or
/// on `screen` and below it, for records of `entity` when it is set; `send` sends it at once, else it fills the composer.
#[derive(Clone, Debug, Default)]
pub struct AssistPrompt {
    pub label: String,
    pub text: String,
    pub send: bool,
    pub entity: String,
    pub screen: String,
    pub sequence: Option<i64>,
}

/// A `response-action`: an action on an Assist answer (plans/LLM-A2A-Assist.md 4.11), a screen opened in the
/// quick-create frame with the answer's text in `text-parameter` (and the page's record when `requires-record`).
#[derive(Clone, Debug, Default)]
pub struct ResponseAction {
    pub label: String,
    pub icon: String,
    pub screen: String,
    pub text_parameter: String,
    pub requires_record: bool,
    pub sequence: Option<i64>,
}

/// The `shell-providers` element of an app root, read with the screen definition. Each list is in `sequence` order (one
/// without a sequence counts as 0, and ties keep the order written). Labels are localized and links made when the shell
/// data is.
#[derive(Clone, Debug, Default)]
pub struct ShellProviders {
    pub quick_create: Vec<ShellLink>,
    pub today_sources: Vec<TodaySource>,
    pub palette_actions: Vec<ShellLink>,
    pub palette_types: Vec<String>,
    pub palette_sources: Vec<PaletteSource>,
    pub notification_topics: Vec<String>,
    pub keymaps: Vec<Keymap>,
    pub account_menu_items: Vec<ShellLink>,
    pub assist_prompts: Vec<AssistPrompt>,
    pub response_actions: Vec<ResponseAction>,
}

fn attr(node: &MNode, name: &str) -> String {
    node.attribute(name)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn sequence_of(node: &MNode) -> Option<i64> {
    node.attribute("sequence").and_then(|text| text.trim().parse().ok())
}

fn by_sequence<T>(list: &mut [T], sequence: impl Fn(&T) -> Option<i64>) {
    list.sort_by_key(|item| sequence(item).unwrap_or(0));
}

impl ShellLink {
    fn of(node: &MNode) -> Self {
        Self {
            kind: attr(node, "kind"),
            label: attr(node, "label"),
            icon: attr(node, "icon"),
            screen: attr(node, "screen"),
            parameters: node
                .children_named("parameter")
                .iter()
                .map(|parameter| (attr(parameter, "name"), attr(parameter, "value")))
                .filter(|(name, _)| !name.is_empty())
                .collect(),
            sequence: sequence_of(node),
            requires_record: attr(node, "requires-record") == "true",
        }
    }
}

impl ShellProviders {
    /// The `shell-providers` of `screen`, or None without one. An element it does not know, and a palette source with
    /// no group or service, are logged with the screen's location and skipped.
    pub(super) fn of(screen: &MNode, location: &str) -> Option<Self> {
        let element = screen.first_named("shell-providers")?;
        let mut out = Self::default();
        for child in element.children() {
            match child.name().as_str() {
                "quick-create" => out.quick_create.push(ShellLink::of(&child)),
                "today-source" => out.today_sources.push(TodaySource {
                    service: attr(&child, "service"),
                    label: attr(&child, "label"),
                }),
                "palette-action" => out.palette_actions.push(ShellLink::of(&child)),
                "palette-type" => {
                    let doc_type = attr(&child, "doc-type");
                    if !doc_type.is_empty() && !out.palette_types.contains(&doc_type) {
                        out.palette_types.push(doc_type);
                    }
                }
                "palette-source" => {
                    let source = PaletteSource {
                        group: attr(&child, "group"),
                        label: attr(&child, "label"),
                        icon: attr(&child, "icon"),
                        service: attr(&child, "service"),
                        prefix: attr(&child, "prefix"),
                        id_pattern: attr(&child, "id-pattern"),
                        sequence: sequence_of(&child),
                    };
                    if source.group.is_empty() || source.service.is_empty() {
                        tracing::error!("{location}: a palette-source needs a group and a service; skipped");
                    } else {
                        out.palette_sources.push(source);
                    }
                }
                "notification-topic" => {
                    let topic = attr(&child, "topic");
                    if !topic.is_empty() {
                        out.notification_topics.push(topic);
                    }
                }
                "keymap" => out.keymaps.push(Keymap {
                    scope: attr(&child, "scope"),
                    keys: child
                        .children_named("key")
                        .iter()
                        .map(|key| ShellKey {
                            keys: attr(key, "keys"),
                            label: attr(key, "label"),
                            screen: attr(key, "screen"),
                            action: attr(key, "action"),
                        })
                        .filter(|key| !key.keys.is_empty())
                        .collect(),
                }),
                "account-menu-item" => out.account_menu_items.push(ShellLink::of(&child)),
                "assist-prompt" => {
                    let prompt = AssistPrompt {
                        label: attr(&child, "label"),
                        text: attr(&child, "text"),
                        send: attr(&child, "send") == "true",
                        entity: attr(&child, "entity"),
                        screen: attr(&child, "screen"),
                        sequence: sequence_of(&child),
                    };
                    if prompt.label.is_empty() || prompt.text.is_empty() {
                        tracing::error!("{location}: an assist-prompt needs a label and a text; skipped");
                    } else {
                        out.assist_prompts.push(prompt);
                    }
                }
                "response-action" => {
                    let action = ResponseAction {
                        label: attr(&child, "label"),
                        icon: attr(&child, "icon"),
                        screen: attr(&child, "screen"),
                        text_parameter: attr(&child, "text-parameter"),
                        requires_record: attr(&child, "requires-record") == "true",
                        sequence: sequence_of(&child),
                    };
                    if action.label.is_empty() || action.screen.is_empty() || action.text_parameter.is_empty() {
                        tracing::error!("{location}: a response-action needs a label, a screen and a text-parameter; skipped");
                    } else {
                        out.response_actions.push(action);
                    }
                }
                other => tracing::error!("{location}: shell-providers has no element {other}; skipped"),
            }
        }
        by_sequence(&mut out.quick_create, |link| link.sequence);
        by_sequence(&mut out.palette_actions, |link| link.sequence);
        by_sequence(&mut out.palette_sources, |source| source.sequence);
        by_sequence(&mut out.account_menu_items, |link| link.sequence);
        by_sequence(&mut out.assist_prompts, |prompt| prompt.sequence);
        by_sequence(&mut out.response_actions, |action| action.sequence);
        Some(out)
    }
}

// ---- the app roots ----

/// An app root the user may open.
struct App {
    item: SubscreensItem,
    def: Arc<ScreenDef>,
    /// The screens from the root screen down to the app root.
    defs: Vec<Arc<ScreenDef>>,
    /// The path names of the app root from the root screen.
    names: Vec<String>,
}

/// What the shell reads under the root screen of the request.
struct Shell<'a> {
    state: &'a RenderState,
    root: String,
    /// The path names of the apps root.
    apps_names: Vec<String>,
    link_base: String,
}

fn leave_app_out(name: &str, location: &str, err: &Error) {
    if !sri::is_security(err) {
        tracing::error!("Leaving app {name} [{location}] out of the shell: {err}");
    }
}

fn text_or_null(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        Value::string(text)
    }
}

fn number_or_null(number: Option<i64>) -> Value {
    number.map_or(Value::Null, Value::I64)
}

fn encoded_path(segments: &[String]) -> String {
    segments
        .iter()
        .map(|name| url_encode_if_needed(name))
        .collect::<Vec<_>>()
        .join("/")
}

impl<'a> Shell<'a> {
    fn new(state: &'a RenderState) -> Result<Self, Error> {
        let root = state
            .url_info
            .path_defs
            .first()
            .map(|def| def.location.clone())
            .ok_or_else(|| Error::new("no root screen"))?;
        Ok(Self {
            state,
            root,
            apps_names: vec![APPS_PATH.to_string()],
            link_base: LINK_BASE.to_string(),
        })
    }

    /// The app roots the user may open, in menu order: the subscreens of the apps root that are in its menu, take no
    /// parameters and that the user may view under it, as the app list shows them. One that does not load is logged and
    /// left out; without an apps root there are none.
    fn apps(&self) -> Vec<App> {
        let parents = match ScreenUrlInfo::resolve(&self.state.factory, &self.root, &self.apps_names, 0) {
            Ok(info) if info.path_defs.len() > self.apps_names.len() => {
                info.path_defs[..=self.apps_names.len()].to_vec()
            }
            Ok(_) => {
                tracing::error!("No apps root at /{} for the shell", self.apps_names.join("/"));
                return Vec::new();
            }
            Err(err) => {
                tracing::error!("No apps root at /{} for the shell: {err}", self.apps_names.join("/"));
                return Vec::new();
            }
        };
        let apps_def = Arc::clone(&parents[parents.len() - 1]);
        let mut out = Vec::new();
        for item in apps_def.subscreens.sorted() {
            if !item.menu_include || !item.is_valid_in_current_context() {
                continue;
            }
            match self.state.facade.screen_def(&self.state.factory, &item.location) {
                Ok(def) => {
                    if !def.parameters.is_empty() || !menu::permitted_under(&parents, &def) {
                        continue;
                    }
                    let mut defs = parents.clone();
                    defs.push(Arc::clone(&def));
                    let mut names = self.apps_names.clone();
                    names.push(item.name.clone());
                    out.push(App {
                        item: item.clone(),
                        def,
                        defs,
                        names,
                    });
                }
                Err(err) => leave_app_out(&item.name, &item.location, &err),
            }
        }
        out
    }

    /// The `/qapps` link of `screen`, a path under the apps root with an optional query string, with `parameters`
    /// added to the query; None when the user may not view that screen. One that names no screen is logged as `what`
    /// of `owner` and left out.
    fn link(&self, owner: &str, what: &str, screen: &str, parameters: &[(String, String)]) -> Option<String> {
        match self.try_link(screen, parameters) {
            Ok(found) => found,
            Err(err) => {
                if !sri::is_security(&err) {
                    tracing::error!("Leaving {what} {screen} of {owner} out of the shell: {err}");
                }
                None
            }
        }
    }

    fn try_link(&self, screen: &str, parameters: &[(String, String)]) -> Result<Option<String>, Error> {
        let (path, query) = screen.split_once('?').unwrap_or((screen, ""));
        let segments: Vec<String> = path
            .split('/')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
            .collect();
        if segments.is_empty() {
            return Err(Error::new("the screen path is empty"));
        }
        let mut names = self.apps_names.clone();
        names.extend(segments.iter().cloned());
        let info = ScreenUrlInfo::resolve(&self.state.factory, &self.root, &names, 0)?;
        // The screen the path names (not a default subscreen below it), under the screens above it.
        let at = names.len().min(info.path_defs.len() - 1);
        if !menu::permitted_under(&info.path_defs[..at], &info.path_defs[at]) {
            return Ok(None);
        }
        let mut query = query.to_string();
        for (name, value) in parameters {
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(&url_encode_if_needed(name));
            query.push('=');
            query.push_str(&url_encode_if_needed(value));
        }
        let mut url = format!("{}/{}", self.link_base, encoded_path(&segments));
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query);
        }
        Ok(Some(url))
    }

    /// The first value of a theme value type, None when the theme has none or it cannot be read (logged).
    fn theme_value(theme: &str, resource_type: &str) -> Option<String> {
        match sri::theme_resource_values(theme, resource_type) {
            Ok(values) => values.into_iter().next(),
            Err(err) => {
                tracing::error!("Could not read {resource_type} of theme {theme} for the shell: {err}");
                None
            }
        }
    }

    /// The `shell` theme values: links for the screen paths, the subscreen names as written, null when not set.
    fn shell_values(&self, theme: &str) -> Value {
        let mut out = Item::new();
        for (key, resource_type, is_path) in SHELL_VALUES {
            let value = match Self::theme_value(theme, resource_type) {
                None => Value::Null,
                Some(path) if is_path => self
                    .link(&format!("theme {theme}"), resource_type, &path, &[])
                    .map_or(Value::Null, Value::String),
                Some(name) => Value::String(name),
            };
            out.insert(key.to_string(), value);
        }
        Value::Map(out)
    }

    /// The record screens of an app (R5): for each of [`RECORD_SCREENS`] whose theme value names a subscreen that this
    /// app root mounts and the user may view there, its link and the render modes the client loads it in (a `.qvue`
    /// panel is asked for as one); null otherwise. An app without the subscreen is not an error (the tools have no
    /// Chatter); one whose screen does not load is logged and gets null.
    fn record_screens(&self, app: &App, names: &[(&str, Option<String>)]) -> Value {
        let mut out = Item::new();
        for (key, name) in names {
            let entry = name.as_deref().and_then(|name| self.record_screen(app, name));
            out.insert((*key).to_string(), entry.unwrap_or(Value::Null));
        }
        Value::Map(out)
    }

    fn record_screen(&self, app: &App, name: &str) -> Option<Value> {
        let item = app.def.subscreens.get(name)?;
        if !item.is_valid_in_current_context() {
            return None;
        }
        let def = match self.state.facade.screen_def(&self.state.factory, &item.location) {
            Ok(def) => def,
            Err(err) => {
                if !sri::is_security(&err) {
                    tracing::error!(
                        "Leaving record screen {name} [{}] of app {} out of the shell: {err}",
                        item.location,
                        app.item.name
                    );
                }
                return None;
            }
        };
        if !menu::permitted_under(&app.defs, &def) {
            return None;
        }
        let mut segments = app.names[self.apps_names.len()..].to_vec();
        segments.push(name.to_string());
        let mut entry = Item::new();
        entry.insert(
            "url".to_string(),
            Value::String(format!("{}/{}", self.link_base, encoded_path(&segments))),
        );
        entry.insert(
            "renderModes".to_string(),
            Value::List(def.render_modes.iter().map(Value::string).collect()),
        );
        Some(Value::Map(entry))
    }

    /// The entry of an app: the menu entry of its root with its menu to depth two, its providers and its record screens,
    /// or None when it is disabled or a required parameter is missing.
    fn app_entry(
        &self,
        app: &App,
        quick_create: Option<&str>,
        record_names: &[(&str, Option<String>)],
    ) -> Result<Option<Value>, Error> {
        let Some(mut out) = self.app_head(app)? else {
            return Ok(None);
        };
        let path = out.get("path").map(to_plain_string).unwrap_or_default();
        let mut menu = Vec::new();
        self.menu_level(&app.defs, &app.names, &path, 1, &mut menu);
        out.insert("menu".to_string(), Value::List(menu));
        out.insert("providers".to_string(), self.providers(app, quick_create)?);
        out.insert("recordScreens".to_string(), self.record_screens(app, record_names));
        Ok(Some(Value::Map(out)))
    }

    /// The name, title, image and path of an app as its menu entry gives them, or None when it is disabled or a required
    /// parameter is missing.
    fn app_head(&self, app: &App) -> Result<Option<Item>, Error> {
        let parents = &app.defs[..app.defs.len() - 1];
        let Some(Value::Map(entry)) =
            menu::subscreen_entry(self.state, parents, &self.apps_names, &self.link_base, &app.item, "")?
        else {
            return Ok(None);
        };
        if entry.get("disableLink") == Some(&Value::Bool(true)) {
            return Ok(None);
        }
        let mut out = Item::new();
        for key in ["name", "title", "image", "imageType", "path"] {
            out.insert(key.to_string(), entry.get(key).cloned().unwrap_or(Value::Null));
        }
        Ok(Some(out))
    }

    /// The menu entries below the last of `parents`, each followed by its own to [`MENU_DEPTH`], as the menu data lists
    /// a screen's subscreens. One that fails is logged and left out, and a disabled one is left out.
    fn menu_level(&self, parents: &[Arc<ScreenDef>], names: &[String], path: &str, depth: usize, out: &mut Vec<Value>) {
        let def = &parents[parents.len() - 1];
        for item in def.subscreens.sorted() {
            if !item.menu_include || !item.is_valid_in_current_context() {
                continue;
            }
            let entry = match menu::subscreen_entry(self.state, parents, names, path, item, "") {
                Ok(Some(Value::Map(entry))) => entry,
                Ok(_) => continue,
                Err(err) => {
                    menu::leave_out(&def.location, item, &err);
                    continue;
                }
            };
            if entry.get("disableLink") == Some(&Value::Bool(true)) {
                continue;
            }
            let item_path = entry.get("path").map(to_plain_string).unwrap_or_default();
            let mut row = Item::new();
            for key in ["title", "path", "image", "imageType"] {
                row.insert(key.to_string(), entry.get(key).cloned().unwrap_or(Value::Null));
            }
            row.insert(
                "depth".to_string(),
                Value::I64(i64::try_from(depth).unwrap_or(i64::MAX)),
            );
            out.push(Value::Map(row));
            if depth >= MENU_DEPTH {
                continue;
            }
            match self.state.facade.screen_def(&self.state.factory, &item.location) {
                Ok(child) => {
                    let mut defs = parents.to_vec();
                    defs.push(child);
                    let mut child_names = names.to_vec();
                    child_names.push(item.name.clone());
                    self.menu_level(&defs, &child_names, &item_path, depth + 1, out);
                }
                Err(err) => menu::leave_out(&def.location, item, &err),
            }
        }
    }

    /// A link provider as JSON with its `url`, or None when it opens nothing the user may view.
    fn link_entry(
        &self,
        app: &App,
        what: &str,
        link: &ShellLink,
        quick_create: Option<&str>,
    ) -> Result<Option<Item>, Error> {
        let owner = format!("app {}", app.item.name);
        let url = if link.screen.is_empty() {
            // A quick-create kind without a screen opens the theme's quick-create screen for its kind.
            let Some(screen) = quick_create.filter(|_| what == "quick-create" && !link.kind.is_empty()) else {
                return Ok(None);
            };
            let mut parameters = vec![("kind".to_string(), link.kind.clone())];
            parameters.extend(link.parameters.iter().cloned());
            self.link(&owner, what, screen, &parameters)
        } else {
            self.link(&owner, what, &link.screen, &link.parameters)
        };
        let Some(url) = url else {
            return Ok(None);
        };
        let mut out = Item::new();
        if what == "quick-create" {
            out.insert("kind".to_string(), text_or_null(&link.kind));
        }
        out.insert("label".to_string(), Value::string(l10n::localize(&link.label)?));
        out.insert("icon".to_string(), text_or_null(&link.icon));
        out.insert("url".to_string(), Value::String(url));
        if what == "quick-create" {
            out.insert("requiresRecord".to_string(), Value::Bool(link.requires_record));
        }
        out.insert("sequence".to_string(), number_or_null(link.sequence));
        Ok(Some(out))
    }

    fn link_list(
        &self,
        app: &App,
        what: &str,
        links: &[ShellLink],
        quick_create: Option<&str>,
    ) -> Result<Value, Error> {
        let mut out = Vec::new();
        for link in links {
            if let Some(entry) = self.link_entry(app, what, link, quick_create)? {
                out.push(Value::Map(entry));
            }
        }
        Ok(Value::List(out))
    }

    /// The providers of an app as the client gets them: labels localized, screens as links, no palette-source service.
    fn providers(&self, app: &App, quick_create: Option<&str>) -> Result<Value, Error> {
        let empty = ShellProviders::default();
        let providers = app.def.shell_providers.as_ref().unwrap_or(&empty);
        let owner = format!("app {}", app.item.name);
        let mut out = Item::new();
        out.insert(
            "quickCreate".to_string(),
            self.link_list(app, "quick-create", &providers.quick_create, quick_create)?,
        );
        let mut today = Vec::new();
        for source in providers
            .today_sources
            .iter()
            .filter(|source| !source.service.is_empty())
        {
            let mut entry = Item::new();
            entry.insert("service".to_string(), Value::string(&source.service));
            entry.insert("label".to_string(), Value::string(l10n::localize(&source.label)?));
            today.push(Value::Map(entry));
        }
        out.insert("todaySources".to_string(), Value::List(today));
        out.insert(
            "paletteActions".to_string(),
            self.link_list(app, "palette-action", &providers.palette_actions, None)?,
        );
        out.insert(
            "paletteTypes".to_string(),
            Value::List(providers.palette_types.iter().map(Value::string).collect()),
        );
        let mut sources = Vec::new();
        for source in &providers.palette_sources {
            let mut entry = Item::new();
            entry.insert("group".to_string(), Value::string(&source.group));
            entry.insert("label".to_string(), Value::string(source_label(source)?));
            entry.insert("icon".to_string(), text_or_null(&source.icon));
            entry.insert("prefix".to_string(), text_or_null(&source.prefix));
            entry.insert("idPattern".to_string(), text_or_null(&source.id_pattern));
            entry.insert("sequence".to_string(), number_or_null(source.sequence));
            sources.push(Value::Map(entry));
        }
        out.insert("paletteSources".to_string(), Value::List(sources));
        out.insert(
            "notificationTopics".to_string(),
            Value::List(providers.notification_topics.iter().map(Value::string).collect()),
        );
        let mut keymaps = Vec::new();
        for keymap in &providers.keymaps {
            let mut keys = Vec::new();
            for key in &keymap.keys {
                let url = if key.screen.is_empty() {
                    None
                } else {
                    match self.link(&owner, "key", &key.screen, &[]) {
                        Some(url) => Some(url),
                        // a key to a screen the user may not open is left out
                        None => continue,
                    }
                };
                let mut entry = Item::new();
                entry.insert("keys".to_string(), Value::string(&key.keys));
                entry.insert("label".to_string(), Value::string(l10n::localize(&key.label)?));
                entry.insert("url".to_string(), url.map_or(Value::Null, Value::String));
                entry.insert("action".to_string(), text_or_null(&key.action));
                keys.push(Value::Map(entry));
            }
            let mut entry = Item::new();
            entry.insert("scope".to_string(), Value::string(&keymap.scope));
            entry.insert("keys".to_string(), Value::List(keys));
            keymaps.push(Value::Map(entry));
        }
        out.insert("keymaps".to_string(), Value::List(keymaps));
        out.insert(
            "accountMenuItems".to_string(),
            self.link_list(app, "account-menu-item", &providers.account_menu_items, None)?,
        );
        // the Assist drawer's providers: a prompt names the page it applies to (the app's root when it names none)
        let app_url = format!(
            "{}/{}",
            self.link_base,
            encoded_path(app.names.get(self.apps_names.len()..).unwrap_or_default())
        );
        let mut prompts = Vec::new();
        for prompt in &providers.assist_prompts {
            let url = if prompt.screen.is_empty() {
                app_url.clone()
            } else {
                match self.link(&owner, "assist-prompt", &prompt.screen, &[]) {
                    Some(url) => url,
                    None => continue,
                }
            };
            let mut entry = Item::new();
            entry.insert("label".to_string(), Value::string(l10n::localize(&prompt.label)?));
            entry.insert("text".to_string(), Value::string(l10n::localize(&prompt.text)?));
            entry.insert("send".to_string(), Value::Bool(prompt.send));
            entry.insert("entity".to_string(), text_or_null(&prompt.entity));
            entry.insert("url".to_string(), Value::String(url));
            entry.insert("sequence".to_string(), number_or_null(prompt.sequence));
            prompts.push(Value::Map(entry));
        }
        out.insert("assistPrompts".to_string(), Value::List(prompts));
        let mut actions = Vec::new();
        for action in &providers.response_actions {
            let Some(url) = self.link(&owner, "response-action", &action.screen, &[]) else {
                continue;
            };
            let mut entry = Item::new();
            entry.insert("label".to_string(), Value::string(l10n::localize(&action.label)?));
            entry.insert("icon".to_string(), text_or_null(&action.icon));
            entry.insert("url".to_string(), Value::String(url));
            entry.insert("textParameter".to_string(), Value::string(&action.text_parameter));
            entry.insert("requiresRecord".to_string(), Value::Bool(action.requires_record));
            entry.insert("sequence".to_string(), number_or_null(action.sequence));
            actions.push(Value::Map(entry));
        }
        out.insert("responseActions".to_string(), Value::List(actions));
        Ok(Value::Map(out))
    }
}

/// The localized label of a palette source, its group name when it has none.
fn source_label(source: &PaletteSource) -> Result<String, Error> {
    l10n::localize(if source.label.is_empty() {
        &source.group
    } else {
        &source.label
    })
}

/// `sri.getShellData()`: the shell data of the `shellData` transition, or None when a 401 was sent for a request with
/// no user. `apps` are the app roots the user may open with their menus and providers, `shell` the shell's theme values
/// of the user's qapps theme (`DEFAULT_QUASAR` when none is set for the user), and `counts` those of
/// `org.moqui.impl.ShellServices.get#ShellCounts` for the user, null without a counts service.
///
/// # Errors
///
/// Returns an error when the root screen of the request is not known.
pub fn shell_data(state: &RenderState) -> Result<Option<Value>, Error> {
    let Some(user_id) = user::current_user_id()?.filter(|id| !id.is_empty()) else {
        web::send_json_error(401, "Authentication required");
        return Ok(None);
    };
    let shell = Shell::new(state)?;
    let theme = sri::user_theme_id(Some(&user_id), QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    let quick_create = Shell::theme_value(&theme, "STRT_QUICK_CREATE");
    let record_names: Vec<(&str, Option<String>)> = RECORD_SCREENS
        .iter()
        .map(|(key, resource_type)| {
            let name = Shell::theme_value(&theme, resource_type).filter(|name| !name.trim().is_empty());
            (*key, name.map(|name| name.trim().to_string()))
        })
        .collect();
    let mut apps = Vec::new();
    for app in shell.apps() {
        match shell.app_entry(&app, quick_create.as_deref(), &record_names) {
            Ok(Some(entry)) => apps.push(entry),
            Ok(None) => {}
            Err(err) => leave_app_out(&app.item.name, &app.item.location, &err),
        }
    }
    let mut out = Item::new();
    out.insert("apps".to_string(), Value::List(apps));
    out.insert("shell".to_string(), shell.shell_values(&theme));
    out.insert("tui".to_string(), tui_values(&theme));
    out.insert(
        "counts".to_string(),
        crate::service::shell::shell_counts(&user_id).map_or(Value::Null, Value::Map),
    );
    Ok(Some(Value::Map(out)))
}

/// The `tui` theme values for the terminal client: `breakpoints` (name to columns), `colors` (role to color) and `keys`
/// (action to chords), each from the `name = value` rows of its resource type in sequence; a later row of a name wins.
/// A row that does not read is logged and left out, so the client keeps its default.
fn tui_values(theme: &str) -> Value {
    let rows = |resource_type: &str| match sri::theme_resource_values(theme, resource_type) {
        Ok(values) => values,
        Err(err) => {
            tracing::error!("Could not read {resource_type} of theme {theme} for the shell: {err}");
            Vec::new()
        }
    };
    let pairs = |resource_type: &str| {
        rows(resource_type)
            .into_iter()
            .filter_map(|row| match row.split_once('=') {
                Some((name, value)) if !name.trim().is_empty() && !value.trim().is_empty() => {
                    Some((name.trim().to_string(), value.trim().to_string()))
                }
                _ => {
                    tracing::warn!("Theme {theme} {resource_type} value [{row}] is not name = value; it is left out");
                    None
                }
            })
            .collect::<Vec<_>>()
    };
    let mut breakpoints = Item::new();
    for (name, value) in pairs("STRT_TUI_BREAKPOINTS") {
        match value.parse::<i64>() {
            Ok(columns) if columns > 0 => {
                breakpoints.insert(name, Value::I64(columns));
            }
            _ => tracing::warn!("Theme {theme} TUI breakpoint {name} = {value} is not a column count; it is left out"),
        }
    }
    let map = |resource_type: &str| {
        Value::Map(
            pairs(resource_type)
                .into_iter()
                .map(|(name, value)| (name, Value::string(value)))
                .collect(),
        )
    };
    // The browser's brand colors (STRT_QUASAR_CONFIG `brand`, as qapps.xml merges it) are the roles' colors too,
    // under the theme's own STRT_TUI_COLORS, which win.
    let mut colors = Item::new();
    for text in rows("STRT_QUASAR_CONFIG") {
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(config) => {
                let brand = config.get("brand").and_then(serde_json::Value::as_object);
                for (name, value) in brand.into_iter().flatten() {
                    let role =
                        ["primary", "secondary", "positive", "negative", "warning", "info"].contains(&name.as_str());
                    if let (true, Some(color)) = (role, value.as_str()) {
                        colors.insert(name.clone(), Value::string(color));
                    }
                }
            }
            Err(err) => tracing::error!("Theme {theme} STRT_QUASAR_CONFIG is not JSON: {err}"),
        }
    }
    for (name, value) in pairs("STRT_TUI_COLORS") {
        colors.insert(name, Value::string(value));
    }
    let mut out = Item::new();
    out.insert("breakpoints".to_string(), Value::Map(breakpoints));
    out.insert("colors".to_string(), Value::Map(colors));
    out.insert("keys".to_string(), map("STRT_TUI_KEYS"));
    out.insert("drawerOpen".to_string(), flag_preference("TUI_DRAWER_OPEN"));
    out.insert("mouse".to_string(), flag_preference("TUI_MOUSE"));
    Value::Map(out)
}

/// A true or false preference of the user's terminal client: `TUI_DRAWER_OPEN` (F9's choice) and `TUI_MOUSE` (Alt+P's);
/// null when it is not set or not a boolean.
fn flag_preference(key: &str) -> Value {
    match user::get_preference(key) {
        Ok(Some(text)) => match text.trim() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            "" => Value::Null,
            other => {
                tracing::warn!("Preference {key} [{other}] is not true or false; it is left unset");
                Value::Null
            }
        },
        Ok(None) => Value::Null,
        Err(err) => {
            tracing::error!("Could not read preference {key} for the shell: {err}");
            Value::Null
        }
    }
}

/// The provider kinds of [`shell_providers`]: the keys of an app's `providers` in the shell data.
pub const PROVIDER_KINDS: [&str; 10] = [
    "quickCreate",
    "todaySources",
    "paletteActions",
    "paletteTypes",
    "paletteSources",
    "notificationTopics",
    "keymaps",
    "accountMenuItems",
    "assistPrompts",
    "responseActions",
];

/// `sri.getShellProviders(kind)`: the providers of the apps the user may open, as `getShellData` gives them but without
/// the menus, record screens and counts, `{apps: [{name, title, image, imageType, path, providers: {<kind>: [...]}}]}`;
/// with a `kind` (one of [`PROVIDER_KINDS`]) only that list in each app's `providers`. A screen that reads the providers
/// (the tray's Today and Notifications panels) asks for this on each load, so it is kept per session (visit), user,
/// locale and theme while the screens it was read from are current (a screen file that changes in development, an app
/// root added) for up to ten minutes, and dropped on a login or logout in the session. No user: no apps.
///
/// # Errors
///
/// Returns an error for a kind that is not a provider kind, or when the root screen of the request is not known.
pub fn shell_providers(state: &RenderState, kind: Option<&str>) -> Result<Value, Error> {
    let kind = kind.map(str::trim).filter(|kind| !kind.is_empty());
    if let Some(kind) = kind {
        if !PROVIDER_KINDS.contains(&kind) {
            return Err(Error::new(format!(
                "Shell provider kind {kind} is not one of {}",
                PROVIDER_KINDS.join(", ")
            )));
        }
    }
    let empty = || {
        let mut out = Item::new();
        out.insert("apps".to_string(), Value::List(Vec::new()));
        Value::Map(out)
    };
    let Some(user_id) = user::current_user_id()?.filter(|id| !id.is_empty()) else {
        return Ok(empty());
    };
    let theme = sri::user_theme_id(Some(&user_id), QUASAR_THEME_TYPE, DEFAULT_QUASAR_THEME);
    let key = format!(
        "{}{user_id}\u{1}{}\u{1}{theme}",
        shell_providers_prefix(user::session::visit_id()?.as_deref()),
        l10n::locale()
    );
    let all = match state.facade.cached_shell_providers(&state.factory, &key) {
        Some(found) => found,
        None => {
            let shell = Shell::new(state)?;
            let quick_create = Shell::theme_value(&theme, "STRT_QUICK_CREATE");
            let mut apps = Vec::new();
            let mut defs: Vec<Arc<ScreenDef>> = Vec::new();
            for app in shell.apps() {
                for def in &app.defs {
                    if !defs.iter().any(|kept| Arc::ptr_eq(kept, def)) {
                        defs.push(Arc::clone(def));
                    }
                }
                let entry = shell.app_head(&app).and_then(|head| {
                    head.map(|mut head| {
                        shell.providers(&app, quick_create.as_deref()).map(|providers| {
                            head.insert("providers".to_string(), providers);
                            head
                        })
                    })
                    .transpose()
                });
                match entry {
                    Ok(Some(entry)) => apps.push(Value::Map(entry)),
                    Ok(None) => {}
                    Err(err) => leave_app_out(&app.item.name, &app.item.location, &err),
                }
            }
            let mut out = Item::new();
            out.insert("apps".to_string(), Value::List(apps));
            let value = Value::Map(out);
            state.facade.put_shell_providers(key, value.clone(), defs);
            value
        }
    };
    let Some(kind) = kind else {
        return Ok(all);
    };
    // only the kind asked for in each app's providers
    let Value::Map(mut out) = all else {
        return Ok(empty());
    };
    if let Some(Value::List(apps)) = out.get_mut("apps") {
        for app in apps.iter_mut() {
            if let Value::Map(app) = app {
                if let Some(Value::Map(providers)) = app.get_mut("providers") {
                    providers.retain(|name, _| name == kind);
                }
            }
        }
    }
    Ok(Value::Map(out))
}

/// The start of the keys of [`shell_providers`] for a session (its visit; none outside a web request).
fn shell_providers_prefix(visit_id: Option<&str>) -> String {
    format!("{}\u{1}", visit_id.unwrap_or(""))
}

/// Drop the providers kept for the session of `visit_id`: a login or logout in it changes what the user may open.
pub(crate) fn forget_shell_providers(factory: &crate::context::ExecutionContextFactory, visit_id: Option<&str>) {
    super::ScreenFacade::of(factory).forget_shell_providers(&shell_providers_prefix(visit_id));
}

// ---- the palette search ----

/// A row of a source's answer with the keys the palette reads, as text or null.
fn row_of(row: &Value) -> Option<Value> {
    let Value::Map(row) = row else {
        return None;
    };
    let mut out = Item::new();
    for key in ["title", "subtitle", "icon", "url"] {
        let value = row
            .get(key)
            .filter(|value| !is_empty(value))
            .map_or(Value::Null, |value| Value::String(to_plain_string(value)));
        out.insert(key.to_string(), value);
    }
    Some(Value::Map(out))
}

/// Run one palette source: its group with up to `limit` rows, None when it answers none.
/// [`run_source`] as the app root that registered the source: its screen is pushed as the artifact the call runs
/// under, so the app's authorization record and its entity filters (the organization and personal contact filters of
/// the IronStone apps) apply to the source's finds, as they would in a screen of that app, and not those of the webroot
/// transition the search runs in.
fn run_source_in_app(
    app: &App,
    source: &PaletteSource,
    text: &str,
    limit: usize,
    doc_types: &[String],
    id_match: bool,
) -> Result<Option<Value>, Error> {
    let required = app.def.authentication == crate::screen::definition::Authentication::Required;
    ExecutionContext::with_current(|ec| {
        ec.push_checked("AT_XML_SCREEN", &app.def.location, "view", required, false, None)
    })
    .unwrap_or(Ok(()))?;
    let answer = run_source(source, text, limit, doc_types, id_match);
    ExecutionContext::with_current(|ec| {
        ec.untrack_hit();
        ec.pop()
    })
    .unwrap_or(Ok(()))?;
    answer
}

fn run_source(
    source: &PaletteSource,
    text: &str,
    limit: usize,
    doc_types: &[String],
    id_match: bool,
) -> Result<Option<Value>, Error> {
    let mut parameters = BTreeMap::new();
    parameters.insert("text".to_string(), Value::string(text));
    parameters.insert("limit".to_string(), Value::I64(i64::try_from(limit).unwrap_or(5)));
    parameters.insert(
        "docTypes".to_string(),
        Value::List(doc_types.iter().map(Value::string).collect()),
    );
    if id_match {
        parameters.insert("idMatch".to_string(), Value::Bool(true));
    }
    let answer = crate::service::shell::call_tolerant(&source.service, &parameters)?;
    let rows: Vec<Value> = match answer.get("rows") {
        Some(Value::List(rows)) => rows.iter().filter_map(row_of).take(limit).collect(),
        Some(value) if !is_empty(value) => return Err(Error::new("its rows are not a list")),
        _ => Vec::new(),
    };
    if rows.is_empty() {
        return Ok(None);
    }
    let mut out = Item::new();
    out.insert("group".to_string(), Value::string(&source.group));
    out.insert("label".to_string(), Value::string(source_label(source)?));
    out.insert("icon".to_string(), text_or_null(&source.icon));
    out.insert("prefix".to_string(), text_or_null(&source.prefix));
    out.insert("rows".to_string(), Value::List(rows));
    out.insert(
        "moreUrl".to_string(),
        answer
            .get("moreUrl")
            .filter(|value| !is_empty(value))
            .map_or(Value::Null, |value| Value::String(to_plain_string(value))),
    );
    Ok(Some(Value::Map(out)))
}

/// `sri.getShellSearch(text, groups, limit)`: the palette groups of the `shellSearch` transition, or None when a 401
/// was sent for a request with no user. The sources are the `palette-source` providers of the app roots the user may
/// open, in sequence order, narrowed to `groups` (a comma list of group names) when given. Each gets the text, the
/// limit (default 5) and the union of those apps' `palette-type` document types; one with an `id-pattern` runs only
/// when the whole text matches it, with `idMatch`. A source that fails is logged by group and service and left out,
/// and so is one that answers no rows. An empty text runs nothing.
///
/// # Errors
///
/// Returns an error when the root screen of the request is not known.
pub fn shell_search(state: &RenderState, text: &str, groups: &str, limit: &str) -> Result<Option<Value>, Error> {
    if user::current_username()?.is_none() {
        web::send_json_error(401, "Authentication required");
        return Ok(None);
    }
    let text = text.trim();
    let limit = limit
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|count| *count > 0)
        .map_or(DEFAULT_LIMIT, |count| count.min(MAX_LIMIT));
    let wanted: BTreeSet<&str> = groups
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    let mut out_groups = Vec::new();
    if !text.is_empty() {
        let shell = Shell::new(state)?;
        let apps = shell.apps();
        let mut doc_types: Vec<String> = Vec::new();
        let mut sources: Vec<(&App, &PaletteSource)> = Vec::new();
        for app in &apps {
            let Some(providers) = app.def.shell_providers.as_ref() else {
                continue;
            };
            for doc_type in &providers.palette_types {
                if !doc_types.contains(doc_type) {
                    doc_types.push(doc_type.clone());
                }
            }
            for source in &providers.palette_sources {
                // the same source registered by two apps runs once
                if !sources
                    .iter()
                    .any(|(_, other)| other.group == source.group && other.service == source.service)
                {
                    sources.push((app, source));
                }
            }
        }
        sources.sort_by_key(|(_, source)| source.sequence.unwrap_or(0));
        for (app_root, source) in sources {
            let app = app_root.item.name.as_str();
            if !wanted.is_empty() && !wanted.contains(source.group.as_str()) {
                continue;
            }
            let id_match = !source.id_pattern.is_empty();
            if id_match {
                match moqui_lib::object::regex_full_match(text, &source.id_pattern) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(err) => {
                        tracing::error!(
                            "Palette source {} ({}) of app {app} has an id-pattern that is not a regular expression: {err}",
                            source.group,
                            source.service
                        );
                        continue;
                    }
                }
            }
            match run_source_in_app(app_root, source, text, limit, &doc_types, id_match) {
                Ok(Some(group)) => out_groups.push(group),
                Ok(None) => {}
                Err(err) => tracing::error!(
                    "Leaving palette group {} ({}) of app {app} out of the search: {err}",
                    source.group,
                    source.service
                ),
            }
        }
    }
    let mut out = Item::new();
    out.insert("groups".to_string(), Value::List(out_groups));
    Ok(Some(Value::Map(out)))
}
