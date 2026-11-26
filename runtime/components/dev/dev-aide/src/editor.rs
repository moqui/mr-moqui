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

//! The editor settings (plans/design-dev-aide.md 5.1, 16): the user's `DEV_AIDE_EDITOR` preference, JSON of the font
//! size, the tab size (none: CodeMirror's), wrap, white space shown, the keymap, and the keys of the code editor's
//! commands a user rebound. The Workspace hands them to the dev-aide's `m-code-editor`s
//! (`moqui.codeEditor.setSettings`, `WebrootCode.js`), which apply the keys over CodeMirror's keymap. The Workspace's
//! own commands (Go to file, Go to artifact, Search in files, Save) are shell key map actions (`moqui.keymap`), rebound
//! by the shell's `KEYMAP` preference (§3.5); Save is set there alone, the text editor taking its keys from it.
//! dev/Settings has the form.

use std::collections::{BTreeMap, BTreeSet};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde::{Deserialize, Serialize};

use crate::tier::{self, Tier};
use crate::workspace::{json_value, text};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_editor_settings), get_editor_settings)?;
    registry.service_function(
        moqui_core::service_location!(store_editor_settings),
        store_editor_settings,
    )?;
    registry.service_function(moqui_core::service_location!(store_editor_key), store_editor_key)
}

/// The preference that holds a user's editor settings.
pub const EDITOR: &str = "DEV_AIDE_EDITOR";

/// The keymaps a user picks from, the names of `moqui.codeEditor.KEYMAPS`. A keymap that needs a library of its own
/// (Vim) is added here and there with the library.
pub(crate) const KEYMAPS: &[&str] = &["default"];

/// A command of the code editor a key row rebinds (`moqui.codeEditor.COMMANDS`).
pub(crate) struct Command {
    pub name: &'static str,
    pub label: &'static str,
    /// Its keys when the user has not rebound it, in CodeMirror's form.
    pub keys: &'static str,
}

const fn command_of(name: &'static str, label: &'static str, keys: &'static str) -> Command {
    Command { name, label, keys }
}

/// The commands a user rebinds, with their default keys (CodeMirror's). Save is not one: it is the shell action
/// `devSave`, in the text editor too.
pub(crate) const COMMANDS: &[Command] = &[
    command_of("find", "Find in the file", "Mod-f"),
    command_of("toggleComment", "Comment the lines", "Mod-/"),
    command_of("completion", "Complete", "Ctrl-Space"),
    command_of("undo", "Undo", "Mod-z"),
    command_of("redo", "Redo", "Mod-y"),
    command_of("foldAll", "Fold all", "Ctrl-Alt-["),
    command_of("unfoldAll", "Unfold all", "Ctrl-Alt-]"),
];

/// A user's editor settings, as the preference holds them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct EditorSettings {
    pub font_size: u32,
    /// The tab size and the indent unit (that many spaces); none for CodeMirror's (a tab of 4, an indent of 2).
    pub tab_size: Option<u32>,
    pub wrap: bool,
    pub whitespace: bool,
    pub keymap: String,
    /// The keys of each command the user rebound, by its name; a command not here has its default keys.
    pub keys: BTreeMap<String, String>,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
            tab_size: None,
            wrap: false,
            whitespace: false,
            keymap: "default".into(),
            keys: BTreeMap::new(),
        }
    }
}

pub(crate) const DEFAULT_FONT_SIZE: u32 = 13;
pub(crate) const FONT_SIZES: std::ops::RangeInclusive<u32> = 8..=32;
pub(crate) const TAB_SIZES: std::ops::RangeInclusive<u32> = 1..=8;
/// The longest keys taken.
pub(crate) const MAX_KEYS_LEN: usize = 64;

const MODIFIERS: [&str; 7] = ["mod", "ctrl", "control", "alt", "shift", "meta", "cmd"];

/// The modifiers and the key of `keys` in CodeMirror's form (`Mod-Shift-f`, `Ctrl-Space`, `F2`, `Mod--`), or `None`
/// when it is not one: modifiers (Mod, Ctrl, Alt, Shift, Meta, Cmd, in any case) joined by `-`, then one key, in at
/// most [`MAX_KEYS_LEN`] characters.
fn parts(keys: &str) -> Option<(Vec<String>, &str)> {
    if keys.len() > MAX_KEYS_LEN {
        return None;
    }
    let (modifiers, key) = match keys.strip_suffix("--") {
        Some(rest) => (rest, "-"),
        None => keys.rsplit_once('-').unwrap_or(("", keys)),
    };
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return None;
    }
    let modifiers: Vec<String> = if modifiers.is_empty() {
        Vec::new()
    } else {
        modifiers.split('-').map(str::to_ascii_lowercase).collect()
    };
    modifiers
        .iter()
        .all(|part| MODIFIERS.contains(&part.as_str()))
        .then_some((modifiers, key))
}

pub(crate) fn valid_keys(keys: &str) -> bool {
    parts(keys).is_some()
}

/// The keys as they are pressed on a Mac (`mac`, where Mod is Cmd) or elsewhere (Mod is Ctrl): the modifiers in one
/// order and the key in lower case, so two ways of writing one key are the same.
fn pressed(keys: &str, mac: bool) -> Option<String> {
    let (modifiers, key) = parts(keys)?;
    let held: BTreeSet<&str> = modifiers
        .iter()
        .map(|part| match part.as_str() {
            "mod" if mac => "meta",
            "mod" | "control" => "ctrl",
            "cmd" => "meta",
            other => other,
        })
        .collect();
    let held: Vec<&str> = held.into_iter().collect();
    Some(format!("{}-{}", held.join("-"), key.to_ascii_lowercase()))
}

/// Whether two keys are the same key on a Mac or elsewhere.
fn same_keys(a: &str, b: &str) -> bool {
    [true, false]
        .into_iter()
        .any(|mac| matches!((pressed(a, mac), pressed(b, mac)), (Some(a), Some(b)) if a == b))
}

/// The Workspace's own commands, shell key map actions (scope `dev-aide`, `DevAide.js` WORKSPACE_KEYS): their id,
/// label and default keys in the shell's form. An editor command is not rebound to their keys.
pub(crate) const SHELL_ACTIONS: &[(&str, &str, &str)] = &[
    ("devSave", "Save", "Mod+S"),
    ("devGotoFile", "Go to file", "Mod+P"),
    ("devGotoArtifact", "Go to artifact", "Mod+Shift+O"),
    ("devSearchFiles", "Search in files", "Mod+Shift+F"),
];

/// The shell's preference of key map bindings, JSON {scope: {id: keys}}.
pub const KEYMAP: &str = "KEYMAP";

/// A shell key map's keys ("Mod+Shift+S", the chord "g s") as CodeMirror writes them ("Mod-Shift-s", "g s"), as
/// `DevAide.js` codeMirrorKeys does.
pub(crate) fn codemirror_keys(keys: &str) -> String {
    keys.split_whitespace()
        .map(|step| {
            let mut parts: Vec<&str> = match step.strip_suffix("++").filter(|rest| !rest.is_empty()) {
                Some(rest) => rest.split('+').chain(std::iter::once("+")).collect(),
                None => step.split('+').collect(),
            };
            let key = parts.pop().unwrap_or_default();
            let name = if key.chars().count() == 1 {
                key.to_lowercase()
            } else if key.eq_ignore_ascii_case("esc") || key.eq_ignore_ascii_case("escape") {
                "Escape".to_string()
            } else {
                key.to_string()
            };
            parts
                .iter()
                .map(|part| (*part).to_string())
                .chain(std::iter::once(name))
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The shell actions' labels and keys in CodeMirror's form: the `KEYMAP` preference's binding (`keymap`, its text), else
/// the default.
pub(crate) fn shell_keys(keymap: Option<&str>) -> Vec<(&'static str, String)> {
    let overrides: serde_json::Value = keymap
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(serde_json::Value::Null);
    SHELL_ACTIONS
        .iter()
        .map(|(id, label, keys)| {
            let bound = overrides["dev-aide"][*id]
                .as_str()
                .filter(|keys| !keys.trim().is_empty())
                .unwrap_or(keys);
            (*label, codemirror_keys(bound))
        })
        .collect()
}

fn command(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.name == name)
}

impl EditorSettings {
    /// The settings of a preference's text: the defaults for none, and for what does not parse (logged); a size out of
    /// range, an unknown keymap and a key row of an unknown command or with keys not in CodeMirror's form are left out.
    pub(crate) fn parse(text: Option<&str>) -> Self {
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            return Self::default();
        };
        let mut settings: Self = match serde_json::from_str(text) {
            Ok(settings) => settings,
            Err(err) => {
                tracing::warn!("the {EDITOR} preference is not the editor's settings, so they are the defaults: {err}");
                return Self::default();
            }
        };
        if !FONT_SIZES.contains(&settings.font_size) {
            settings.font_size = DEFAULT_FONT_SIZE;
        }
        settings.tab_size = settings.tab_size.filter(|size| TAB_SIZES.contains(size));
        if !KEYMAPS.contains(&settings.keymap.as_str()) {
            settings.keymap = Self::default().keymap;
        }
        settings.keys.retain(|name, keys| {
            command(name).is_some_and(|command| !same_keys(command.keys, keys)) && valid_keys(keys)
        });
        settings
    }

    /// The keys `command` runs on: the user's, else its default.
    pub(crate) fn keys_of(&self, command: &Command) -> String {
        self.keys
            .get(command.name)
            .cloned()
            .unwrap_or_else(|| command.keys.to_string())
    }

    /// [`Self::rebind_beside`] the shell actions' default keys, for the tests.
    #[cfg(test)]
    pub(crate) fn rebind(&mut self, name: &str, keys: &str) -> Result<(), String> {
        self.rebind_beside(name, keys, &shell_keys(None))
    }

    /// `keys` for the command `name`, or its default when empty; refused for an unknown command, keys not in
    /// CodeMirror's form, keys another command runs on (on a Mac, where Mod is Cmd, or elsewhere, where it is Ctrl), its
    /// default keys too, or the keys of a shell action (`shell`, each its label and keys in CodeMirror's form): the
    /// Workspace's own commands keep theirs.
    pub(crate) fn rebind_beside(&mut self, name: &str, keys: &str, shell: &[(&str, String)]) -> Result<(), String> {
        let Some(target) = command(name) else {
            return Err(format!("there is no editor command {name}"));
        };
        let keys = keys.trim();
        let default = keys.is_empty() || same_keys(keys, target.keys);
        let keys = if default { target.keys } else { keys };
        if !default && !valid_keys(keys) {
            return Err(format!(
                "{} is not a key as the editor writes one: modifiers (Mod, Ctrl, Alt, Shift) and a key joined by -, such as Mod-Shift-f",
                keys.chars().take(MAX_KEYS_LEN).collect::<String>()
            ));
        }
        if let Some(other) = COMMANDS
            .iter()
            .find(|other| other.name != name && same_keys(&self.keys_of(other), keys))
        {
            return Err(format!("{keys} is the key of {}: rebind that first", other.label));
        }
        if let Some((label, _)) = shell.iter().find(|(_, bound)| same_keys(bound, keys)) {
            return Err(format!(
                "{keys} is the key of the Workspace's {label}, a shell action (the KEYMAP preference rebinds it)"
            ));
        }
        if default {
            self.keys.remove(name);
        } else {
            self.keys.insert(name.to_string(), keys.to_string());
        }
        Ok(())
    }

    /// The settings and each command's row (its name, label, default keys and keys now), as the services answer.
    fn answer(&self) -> Parameters {
        let commands: Vec<serde_json::Value> = COMMANDS
            .iter()
            .map(|command| {
                serde_json::json!({
                    "command": command.name,
                    "label": command.label,
                    "defaultKeys": command.keys,
                    "keys": self.keys_of(command),
                })
            })
            .collect();
        let mut out = Parameters::new();
        out.insert("settings".into(), json_value(self));
        out.insert("commands".into(), json_value(&commands));
        out.insert("keymaps".into(), json_value(&KEYMAPS));
        out
    }
}

/// The current user's settings.
fn current() -> EditorSettings {
    EditorSettings::parse(moqui_core::user::get_preference(EDITOR).ok().flatten().as_deref())
}

fn store(settings: &EditorSettings) -> Result<(), LibError> {
    let text = serde_json::to_string(settings).map_err(|err| LibError::new(err.to_string()))?;
    moqui_core::user::set_preference(EDITOR, Some(text.as_str()))?;
    Ok(())
}

/// `get#EditorSettings`: the user's editor settings, each command's keys, and the keymaps to pick from. Any tier: they
/// are the user's own.
fn get_editor_settings(
    _ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    _parameters: &Parameters,
) -> Result<Parameters, LibError> {
    tier::require(Tier::View)?;
    Ok(current().answer())
}

/// A size of `name`: `None` when not given (kept), `Some(None)` when given empty (the default), else the number,
/// refused out of `range`.
fn size_of(
    parameters: &Parameters,
    name: &str,
    range: &std::ops::RangeInclusive<u32>,
) -> Result<Option<Option<u32>>, LibError> {
    if !parameters.contains_key(name) {
        return Ok(None);
    }
    let raw = text(parameters, name);
    if raw.trim().is_empty() {
        return Ok(Some(None));
    }
    match raw.trim().parse::<u32>() {
        Ok(value) if range.contains(&value) => Ok(Some(Some(value))),
        _ => Err(LibError::new(format!(
            "{name} is a whole number from {} to {}",
            range.start(),
            range.end()
        ))),
    }
}

/// `store#EditorSettings`: the font size, tab size, wrap, white space and keymap given (one given empty is its
/// default), the keys kept; refused out of range.
fn store_editor_settings(
    _ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    tier::require(Tier::View)?;
    let mut settings = current();
    if let Some(size) = size_of(parameters, "fontSize", &FONT_SIZES)? {
        settings.font_size = size.unwrap_or(DEFAULT_FONT_SIZE);
    }
    if let Some(size) = size_of(parameters, "tabSize", &TAB_SIZES)? {
        settings.tab_size = size;
    }
    let flag = |name: &str| {
        parameters
            .get(name)
            .map(|value| matches!(moqui_lib::object::to_plain_string(value).as_str(), "true" | "Y" | "y"))
    };
    if let Some(wrap) = flag("wrap") {
        settings.wrap = wrap;
    }
    if let Some(whitespace) = flag("whitespace") {
        settings.whitespace = whitespace;
    }
    let keymap = text(parameters, "keymap");
    if !keymap.is_empty() {
        if !KEYMAPS.contains(&keymap.as_str()) {
            return Err(LibError::new(format!(
                "there is no keymap {}: {}",
                keymap.chars().take(MAX_KEYS_LEN).collect::<String>(),
                KEYMAPS.join(", ")
            )));
        }
        settings.keymap = keymap;
    }
    store(&settings)?;
    Ok(settings.answer())
}

/// `store#EditorKey`: a command's keys (empty for its default), refused for keys another command has.
fn store_editor_key(
    _ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    tier::require(Tier::View)?;
    let mut settings = current();
    let keymap = moqui_core::user::get_preference(KEYMAP).ok().flatten();
    settings
        .rebind_beside(
            &text(parameters, "command"),
            &text(parameters, "keys"),
            &shell_keys(keymap.as_deref()),
        )
        .map_err(LibError::new)?;
    store(&settings)?;
    let mut out = settings.answer();
    out.insert("command".into(), Value::string(text(parameters, "command")));
    Ok(out)
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
