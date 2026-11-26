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
//! size, the tab size, wrap, white space shown, the keymap, and the keys of the commands a user rebound. The Workspace
//! hands them to `m-code-editor` (`moqui.codeEditor.setSettings`, `WebrootCode.js`), which applies the keys over its
//! default keymap, and binds its own commands (Go to file, Go to artifact, Search in files) by them; dev/Settings has
//! the form. Settings › Editor writes them through `store#EditorSettings` and `store#EditorKey`.

use std::collections::BTreeMap;

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

/// A command a key row rebinds.
pub(crate) struct Command {
    pub name: &'static str,
    pub label: &'static str,
    /// `workspace`: the Workspace binds it (its `onKey`); `editor`: `m-code-editor` (`moqui.codeEditor.COMMANDS`).
    pub scope: &'static str,
    /// Its keys when the user has not rebound it, in CodeMirror's form.
    pub keys: &'static str,
}

/// The commands a user rebinds, with their default keys: the Workspace's, then the editor's (CodeMirror's defaults).
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        name: "gotoFile",
        label: "Go to file",
        scope: "workspace",
        keys: "Mod-p",
    },
    Command {
        name: "gotoArtifact",
        label: "Go to artifact",
        scope: "workspace",
        keys: "Mod-Shift-o",
    },
    Command {
        name: "searchFiles",
        label: "Search in files",
        scope: "workspace",
        keys: "Mod-Shift-f",
    },
    Command {
        name: "save",
        label: "Save",
        scope: "editor",
        keys: "Mod-s",
    },
    Command {
        name: "find",
        label: "Find in the file",
        scope: "editor",
        keys: "Mod-f",
    },
    Command {
        name: "toggleComment",
        label: "Comment the lines",
        scope: "editor",
        keys: "Mod-/",
    },
    Command {
        name: "completion",
        label: "Complete",
        scope: "editor",
        keys: "Ctrl-Space",
    },
    Command {
        name: "undo",
        label: "Undo",
        scope: "editor",
        keys: "Mod-z",
    },
    Command {
        name: "redo",
        label: "Redo",
        scope: "editor",
        keys: "Mod-y",
    },
    Command {
        name: "foldAll",
        label: "Fold all",
        scope: "editor",
        keys: "Ctrl-Alt-[",
    },
    Command {
        name: "unfoldAll",
        label: "Unfold all",
        scope: "editor",
        keys: "Ctrl-Alt-]",
    },
];

/// A user's editor settings, as the preference holds them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct EditorSettings {
    pub font_size: u32,
    pub tab_size: u32,
    pub wrap: bool,
    pub whitespace: bool,
    pub keymap: String,
    /// The keys of each command the user rebound, by its name; a command not here has its default keys.
    pub keys: BTreeMap<String, String>,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            font_size: 13,
            tab_size: 4,
            wrap: false,
            whitespace: false,
            keymap: "default".into(),
            keys: BTreeMap::new(),
        }
    }
}

pub(crate) const FONT_SIZES: std::ops::RangeInclusive<u32> = 8..=32;
pub(crate) const TAB_SIZES: std::ops::RangeInclusive<u32> = 1..=8;

/// Whether `keys` is a key in CodeMirror's form: modifiers (Mod, Ctrl, Alt, Shift, Meta, Cmd, in any case) joined by
/// `-`, then one key (`Mod-Shift-f`, `Ctrl-Space`, `F2`, `Mod--`).
pub(crate) fn valid_keys(keys: &str) -> bool {
    let (modifiers, key) = match keys.strip_suffix("--") {
        Some(rest) => (rest, "-"),
        None => match keys.rsplit_once('-') {
            Some((modifiers, key)) => (modifiers, key),
            None => ("", keys),
        },
    };
    !key.is_empty()
        && !key.chars().any(char::is_whitespace)
        && (modifiers.is_empty()
            || modifiers.split('-').all(|part| {
                matches!(
                    part.to_ascii_lowercase().as_str(),
                    "mod" | "ctrl" | "control" | "alt" | "shift" | "meta" | "cmd"
                )
            }))
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
        let defaults = Self::default();
        if !FONT_SIZES.contains(&settings.font_size) {
            settings.font_size = defaults.font_size;
        }
        if !TAB_SIZES.contains(&settings.tab_size) {
            settings.tab_size = defaults.tab_size;
        }
        if !KEYMAPS.contains(&settings.keymap.as_str()) {
            settings.keymap = defaults.keymap;
        }
        settings
            .keys
            .retain(|name, keys| command(name).is_some_and(|command| command.keys != keys) && valid_keys(keys));
        settings
    }

    /// The keys `name` runs on: the user's, else its default.
    pub(crate) fn keys_of(&self, command: &Command) -> String {
        self.keys
            .get(command.name)
            .cloned()
            .unwrap_or_else(|| command.keys.to_string())
    }

    /// `keys` for the command `name`, or its default when empty; refused for an unknown command, keys not in
    /// CodeMirror's form, or keys another command runs on.
    pub(crate) fn rebind(&mut self, name: &str, keys: &str) -> Result<(), String> {
        let Some(target) = command(name) else {
            return Err(format!("there is no editor command {name}"));
        };
        let keys = keys.trim();
        if keys.is_empty() || keys == target.keys {
            self.keys.remove(name);
            return Ok(());
        }
        if !valid_keys(keys) {
            return Err(format!(
                "{keys} is not a key as the editor writes one: modifiers (Mod, Ctrl, Alt, Shift) and a key joined by -, such as Mod-Shift-f"
            ));
        }
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        if let Some(other) = COMMANDS
            .iter()
            .find(|other| other.name != name && same(&self.keys_of(other), keys))
        {
            return Err(format!("{keys} is the key of {}: rebind that first", other.label));
        }
        self.keys.insert(name.to_string(), keys.to_string());
        Ok(())
    }

    /// The settings and each command's row (its name, label, scope, default keys and keys now), as the services answer.
    fn answer(&self) -> Parameters {
        let commands: Vec<serde_json::Value> = COMMANDS
            .iter()
            .map(|command| {
                serde_json::json!({
                    "command": command.name,
                    "label": command.label,
                    "scope": command.scope,
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

/// `store#EditorSettings`: the font size, tab size, wrap, white space and keymap, the keys kept; refused out of range.
fn store_editor_settings(
    _ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    tier::require(Tier::View)?;
    let mut settings = current();
    let number = |name: &str, range: &std::ops::RangeInclusive<u32>| -> Result<Option<u32>, LibError> {
        let raw = text(parameters, name);
        if raw.trim().is_empty() {
            return Ok(None);
        }
        match raw.trim().parse::<u32>() {
            Ok(value) if range.contains(&value) => Ok(Some(value)),
            _ => Err(LibError::new(format!(
                "{name} is a whole number from {} to {}",
                range.start(),
                range.end()
            ))),
        }
    };
    if let Some(size) = number("fontSize", &FONT_SIZES)? {
        settings.font_size = size;
    }
    if let Some(size) = number("tabSize", &TAB_SIZES)? {
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
                "there is no keymap {keymap}: {}",
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
    settings
        .rebind(&text(parameters, "command"), &text(parameters, "keys"))
        .map_err(LibError::new)?;
    store(&settings)?;
    let mut out = settings.answer();
    out.insert("command".into(), Value::string(text(parameters, "command")));
    Ok(out)
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
