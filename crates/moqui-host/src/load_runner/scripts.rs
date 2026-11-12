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

//! The load scripts a run from Tools may start (plans/archive/LoadRunner.md D2, D12, §10 Scripts): the
//! `<component>/test/load/<Name>.load.js` files of the loaded components, named by
//! `component://<component>/test/load/<Name>.load.js`. No script text comes from the screen, and no other file is a
//! script.

use std::path::{Path, PathBuf};

/// The directory of a component's load scripts.
const SCRIPT_DIR: &str = "test/load";
/// A load script's file name ends with this.
const SCRIPT_SUFFIX: &str = ".load.js";

/// A load script of a loaded component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadScript {
    /// `component://<component>/test/load/<Name>.load.js`.
    pub(crate) location: String,
    pub(crate) component: String,
    /// The file name without `.load.js`.
    pub(crate) name: String,
    pub(crate) file_name: String,
    pub(crate) path: PathBuf,
}

/// The load scripts of `components` (name and directory), by component in the order given and then by name. A
/// component whose `test/load` does not read is logged and the others still list.
pub(crate) fn scripts_in<'a>(components: impl IntoIterator<Item = (&'a str, &'a Path)>) -> Vec<LoadScript> {
    let mut scripts = Vec::new();
    for (component, location) in components {
        let dir = location.join(SCRIPT_DIR);
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                tracing::error!(dir = %dir.display(), error = %err, "Could not list a component's load scripts");
                continue;
            }
        };
        let mut found: Vec<LoadScript> = entries
            .flatten()
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| {
                let file_name = entry.file_name().to_str()?.to_string();
                let name = script_name(&file_name)?.to_string();
                Some(LoadScript {
                    location: format!("component://{component}/{SCRIPT_DIR}/{file_name}"),
                    component: component.to_string(),
                    name,
                    file_name,
                    path: entry.path(),
                })
            })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        scripts.extend(found);
    }
    scripts
}

/// The script `location` names among `components`, which must be a `component://<component>/test/load/<Name>.load.js`
/// of a loaded component whose file is there; the error says why not.
pub(crate) fn resolve<'a>(
    location: &str,
    components: impl IntoIterator<Item = (&'a str, &'a Path)>,
) -> Result<LoadScript, String> {
    let not_a_script = || format!("{location} is not a load script of a loaded component (test/load/*.load.js)");
    let rest = location.trim().strip_prefix("component://").ok_or_else(not_a_script)?;
    let (component, rest) = rest.split_once('/').ok_or_else(not_a_script)?;
    let file_name = rest
        .strip_prefix(SCRIPT_DIR)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or_else(not_a_script)?;
    if script_name(file_name).is_none() {
        return Err(not_a_script());
    }
    let (_, dir) = components
        .into_iter()
        .find(|(name, _)| *name == component)
        .ok_or_else(not_a_script)?;
    let path = dir.join(SCRIPT_DIR).join(file_name);
    if !path.is_file() {
        return Err(format!("{location}: no such load script"));
    }
    Ok(LoadScript {
        location: format!("component://{component}/{SCRIPT_DIR}/{file_name}"),
        component: component.to_string(),
        name: script_name(file_name).unwrap_or_default().to_string(),
        file_name: file_name.to_string(),
        path,
    })
}

/// The name of a load script file (`Name` of `Name.load.js`); None for another file, a hidden one, or a name with a
/// path in it.
fn script_name(file_name: &str) -> Option<&str> {
    let name = file_name.strip_suffix(SCRIPT_SUFFIX)?;
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.contains("..");
    plain.then_some(name)
}

#[cfg(test)]
#[path = "scripts_tests.rs"]
mod tests;
