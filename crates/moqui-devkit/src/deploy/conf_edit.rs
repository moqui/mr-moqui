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

//! The services of System › Conf's Settings and Edit tabs (docs/tools/Component-Deploy.md, "Viewing and editing the
//! conf"): the merged conf with where each value came from ([`conf_reload::view`]), and one conf file read, checked and
//! saved. Saving writes the file and reloads the conf, so what can be live is in effect at once and the rest is listed
//! as waiting for a restart. Viewing needs the `ADMIN` group; the file services also need the sealed `CONF_EDIT`
//! permission. A file is named by its place in the merge, never by a path from the request.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use sha2::{Digest, Sha256};

use moqui_core::context::conf_reload::{self, view};
use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_core::rust_service;

use super::services::{add_error, admin, json_value, parameter};

/// The permission that reads and saves the conf files.
pub const EDIT_PERMISSION: &str = "CONF_EDIT";

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, get_conf_view)?;
    rust_service!(map, get_conf_file)?;
    rust_service!(map, check_conf_file)?;
    rust_service!(map, save_conf_file)?;
    Ok(())
}

/// The factory when the user is in `ADMIN` and has `CONF_EDIT`; otherwise the error is added.
fn editor(what: &str) -> Option<Arc<ExecutionContextFactory>> {
    let factory = admin(what)?;
    if !moqui_core::user::has_permission(EDIT_PERMISSION).unwrap_or(false) {
        add_error(format!("{what} needs the {EDIT_PERMISSION} permission"));
        return None;
    }
    Some(factory)
}

/// The conf file at `fileIndex` of the merge (none named: [`default_index`]), with the list it is in; otherwise the
/// error is added.
fn file_at(
    factory: &ExecutionContextFactory,
    parameters: &BTreeMap<String, Value>,
) -> Option<(usize, Vec<moqui_core::context::ConfFile>)> {
    let files = match view::files(factory) {
        Ok(files) => files,
        Err(err) => {
            add_error(format!("The conf files could not be listed: {err}"));
            return None;
        }
    };
    let given = parameter(parameters, "fileIndex");
    let index = if given.is_empty() {
        default_index(&files)
    } else {
        given.parse::<usize>().ok().filter(|index| *index < files.len())
    };
    let Some(index) = index else {
        add_error(format!("No conf file {given}"));
        return None;
    };
    Some((index, files))
}

/// The file to open when none is named: the runtime conf, else the last file of the merge.
fn default_index(files: &[moqui_core::context::ConfFile]) -> Option<usize> {
    files
        .iter()
        .rposition(|file| file.role == moqui_core::context::ConfRole::Runtime)
        .or_else(|| files.len().checked_sub(1))
}

/// The hash of a file's text, for a save to tell whether the file changed since it was read.
fn text_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The text of the parameter `name`, as given: a file's text keeps its whitespace.
fn text_parameter(parameters: &BTreeMap<String, Value>, name: &str) -> String {
    parameters
        .get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

/// `get#ConfView`: the merged conf, each element and attribute with the file and line it came from, its class (live,
/// apply, restart), the default conf's comments, and the values in the files not running yet; secrets masked. Out:
/// `view`, `canEdit`.
fn get_conf_view(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = admin("Viewing the conf") else {
        return Ok(out);
    };
    match view::view(&factory) {
        Ok(found) => {
            out.insert("view".to_string(), json_value(&found));
        }
        Err(err) => add_error(format!("The conf files do not merge: {err}")),
    }
    out.insert(
        "canEdit".to_string(),
        Value::Bool(moqui_core::user::has_permission(EDIT_PERMISSION).unwrap_or(false)),
    );
    Ok(out)
}

/// `get#ConfFile`: one conf file's text, its hash, whether it can be saved, its elements by line with their classes
/// and the default conf's comments, its problems, and the schema digest for completion. Out: `file`, `text`,
/// `textHash`, `lines`, `problems`, `schema`.
fn get_conf_file(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = editor("Reading a conf file") else {
        return Ok(out);
    };
    let Some((index, files)) = file_at(&factory, parameters) else {
        return Ok(out);
    };
    let text = match std::fs::read_to_string(&files[index].path) {
        Ok(text) => text,
        Err(err) => {
            add_error(format!("{} could not be read: {err}", files[index].path.display()));
            return Ok(out);
        }
    };
    out.insert("file".to_string(), json_value(&view::file_infos(&files)[index]));
    out.insert("files".to_string(), json_value(&view::file_infos(&files)));
    out.insert("textHash".to_string(), Value::String(text_hash(&text)));
    check_into(&factory, index, &files[index].path, &text, &mut out, false);
    out.insert("text".to_string(), Value::String(text));
    let digest = crate::schema::digest(&crate::check::xsd_dir(&factory), "moqui-conf");
    out.insert(
        "schema".to_string(),
        digest.map_or(Value::Null, |digest| json_value(&*digest)),
    );
    Ok(out)
}

/// `check#ConfFile`: what saving `text` as the conf file would do: its problems (parse and root errors, schema
/// warnings), its elements by line, and the preview of the reload. Out: `problems`, `lines`, `report`.
fn check_conf_file(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = editor("Checking a conf file") else {
        return Ok(out);
    };
    let Some((index, files)) = file_at(&factory, parameters) else {
        return Ok(out);
    };
    let text = text_parameter(parameters, "text");
    check_into(&factory, index, &files[index].path, &text, &mut out, true);
    Ok(out)
}

/// The problems and lines of `text` as conf file `index` into `out`, and with `report` the preview of the reload.
/// Answers whether the text can be saved: it parses, its root is `moqui-conf`, and the files merge with it.
fn check_into(
    factory: &Arc<ExecutionContextFactory>,
    index: usize,
    path: &Path,
    text: &str,
    out: &mut BTreeMap<String, Value>,
    report: bool,
) -> bool {
    let (mut problems, savable) = match view::check_file(factory, index, text) {
        Ok(check) => {
            let savable = check.problems.is_empty();
            out.insert("lines".to_string(), json_value(&check.lines));
            if report {
                if let Some(found) = &check.report {
                    out.insert("report".to_string(), json_value(found));
                }
            }
            (json_list(&check.problems), savable)
        }
        Err(err) => (
            vec![serde_json::json!({"line": 1, "column": 1, "severity": "error",
                "message": format!("The conf files do not merge with this text: {err}")})],
            false,
        ),
    };
    if savable {
        // the schema and the references: warnings and errors that do not keep the file from loading
        problems.extend(json_list(&crate::check::check_text(factory, path, text)));
    }
    out.insert(
        "problems".to_string(),
        moqui_lib::json::from_json(&serde_json::Value::Array(problems)),
    );
    out.insert("savable".to_string(), Value::Bool(savable));
    savable
}

fn json_list<T: serde::Serialize>(items: &[T]) -> Vec<serde_json::Value> {
    items
        .iter()
        .filter_map(|item| serde_json::to_value(item).ok())
        .collect()
}

/// `save#ConfFile`: write the text to the conf file and reload the conf. Refused for the default conf, a file the
/// server cannot write, a file changed since it was read (`expectedHash`), and a text that does not parse, is not a
/// `moqui-conf`, or does not merge. Out: `report` (what the reload applied and what waits for a restart),
/// `textHash`, `restartAllowed`.
fn save_conf_file(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = BTreeMap::new();
    let Some(factory) = editor("Saving a conf file") else {
        return Ok(out);
    };
    let Some((index, files)) = file_at(&factory, parameters) else {
        return Ok(out);
    };
    let info = &view::file_infos(&files)[index];
    let path = &files[index].path;
    if !info.editable {
        add_error(if info.role == "default" {
            "The default conf is not saved here: override its settings in the runtime conf".to_string()
        } else {
            format!("{} cannot be written by the server", path.display())
        });
        return Ok(out);
    }
    let on_disk = std::fs::read_to_string(path).unwrap_or_default();
    if text_hash(&on_disk) != parameter(parameters, "expectedHash") {
        add_error(format!(
            "{} changed since it was read; read it again and make the change there",
            path.display()
        ));
        return Ok(out);
    }
    let text = text_parameter(parameters, "text");
    let mut checked = BTreeMap::new();
    if !check_into(&factory, index, path, &text, &mut checked, false) {
        add_error(format!(
            "{} is not saved: the text is not a conf file the server can merge",
            path.display()
        ));
        out.insert(
            "problems".to_string(),
            checked.remove("problems").unwrap_or(Value::Null),
        );
        return Ok(out);
    }
    if let Err(err) = write_replacing(path, &text) {
        add_error(format!("{} could not be written: {err}", path.display()));
        return Ok(out);
    }
    let user = moqui_core::user::session::current_user_id()
        .ok()
        .flatten()
        .unwrap_or_default();
    tracing::warn!(
        target: moqui_core::live_settings::AUDIT,
        "Conf file {} saved by user {user}",
        path.display()
    );
    out.insert("textHash".to_string(), Value::String(text_hash(&text)));
    match conf_reload::apply(&factory) {
        Ok(report) => {
            out.insert("report".to_string(), json_value(&report));
        }
        Err(err) => ExecutionContext::with_current(|ec| {
            ec.message().add_error(format!(
                "{} is saved, but the conf did not reload, so the running conf stays: {err}",
                path.display()
            ));
        })
        .unwrap_or_default(),
    }
    out.insert(
        "restartAllowed".to_string(),
        Value::Bool(crate::service::restart_allowed()),
    );
    Ok(out)
}

/// Write `text` to `path` through a file beside it renamed over it, so a reader never sees half a file; the file keeps
/// its permissions.
fn write_replacing(path: &Path, text: &str) -> std::io::Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.saving-{}", std::process::id()));
    std::fs::write(&temp, text)?;
    if let Ok(meta) = std::fs::metadata(path) {
        // best effort: the file's mode, as before the save
        let _ = std::fs::set_permissions(&temp, meta.permissions());
    }
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

#[cfg(test)]
#[path = "conf_edit_tests.rs"]
mod tests;
