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

//! Script files by location: their text kept and checked for changes, and a file run as a service body.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use moqui_lib::sync::lock;

/// Whether module values are kept for the run: the `resource-facade` `script-module-cache`, true unless `false` (read
/// once).
pub(crate) fn module_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::context::conf_env::setting("resource-facade@script-module-cache") != "false")
}

/// A script file's text by location, as the function [`crate::script::parse::file_function`] makes of it, with the
/// modified time it had and when that was last checked.
struct ScriptText {
    path: std::path::PathBuf,
    modified: Option<std::time::SystemTime>,
    checked: Instant,
    /// The text, with its hash made once and the location as its name.
    text: super::ScriptText,
    /// Whether the file's top level only declares ([`crate::script::parse::module_is_inert`]).
    inert: bool,
}

/// How long a script file's text is used before its modified time is looked at again.
const SCRIPT_CHECK: Duration = Duration::from_secs(2);

/// The first line the standard component library builder (`webroot/build/component-libs.mjs`) writes on a server
/// module, `component://<component>/script/libs/<id>.js`. The module is a bundle of reviewed npm packages whose top level
/// is one call that makes their exports, so [`crate::script::parse::module_is_inert`] refuses it; its value is kept for
/// the run as an inert module's is, so a service that uses a library does not run the whole bundle at each call.
const COMPONENT_LIBRARY_MARKER: &str = "/* moqui component library ";

/// A server module of the standard library builder: its marker line, in a component's `script/libs`.
fn component_library(path: &std::path::Path, text: &str) -> bool {
    text.starts_with(COMPONENT_LIBRARY_MARKER) && path.parent().is_some_and(|dir| dir.ends_with("script/libs"))
}

fn script_texts() -> &'static Mutex<HashMap<String, ScriptText>> {
    static TEXTS: std::sync::OnceLock<Mutex<HashMap<String, ScriptText>>> = std::sync::OnceLock::new();
    TEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn modified_of(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// Drop the kept text of every script file among `files` at once, rather than at the next check (plans/archive/design-dev-aide.md
/// 6.1, F6). Answers the script locations dropped.
pub fn forget_scripts(files: &[std::path::PathBuf]) -> Vec<String> {
    let canonical = |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let files: Vec<std::path::PathBuf> = files.iter().map(|file| canonical(file)).collect();
    let mut dropped = Vec::new();
    lock(script_texts()).retain(|location, kept| {
        let stale = files.contains(&canonical(&kept.path));
        if stale {
            dropped.push(location.clone());
        }
        !stale
    });
    dropped
}

/// [`read_script`], and whether the file's top level only declares, so its value may be kept for the run.
pub(crate) fn read_module(location: &str) -> Result<(super::ScriptText, bool), Error> {
    // a script file's unsaved or proposed text held on this thread (F14, for services): neither read from nor kept in
    // the cache of the files' texts
    if let Some(text) = crate::service::overlay::script_text(location) {
        let library = crate::context::locate(location).is_ok_and(|path| component_library(&path, &text));
        let inert = library || crate::script::parse::module_is_inert(&text);
        let text: Arc<str> = crate::script::parse::file_function(&text).into();
        return Ok((super::ScriptText::named(text, location), inert));
    }
    let production = crate::web::is_production();
    {
        let mut texts = lock(script_texts());
        if let Some(found) = texts.get_mut(location) {
            if production || found.checked.elapsed() < SCRIPT_CHECK {
                return Ok((found.text.clone(), found.inert));
            }
            if modified_of(&found.path) == found.modified {
                found.checked = Instant::now();
                return Ok((found.text.clone(), found.inert));
            }
        }
    }
    let path = crate::context::locate(location)?;
    let modified = modified_of(&path);
    let text = moqui_lib::vfs::read_to_string(&path)
        .map_err(|err| Error::new(format!("could not read script {location}")).with_source(err))?;
    let inert = component_library(&path, &text) || crate::script::parse::module_is_inert(&text);
    let text: Arc<str> = crate::script::parse::file_function(&text).into();
    let script = super::ScriptText::named(text, location);
    lock(script_texts()).insert(
        location.to_string(),
        ScriptText {
            path,
            modified,
            checked: Instant::now(),
            text: script.clone(),
            inert,
        },
    );
    Ok((script, inert))
}

/// Run a script file as a service body does: in the current context frame, with bare names.
///
/// # Errors
///
/// Returns an error when the file cannot be read or the script throws.
pub fn run_script_location(location: &str) -> Result<(), Error> {
    let (script, _) = read_module(location)?;
    eval_script_text(&script)
}
