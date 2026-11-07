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

//! Smaller `ec` facades: resource and web. Cache, message, logger and user are in the host table (`script/host`).

use super::*;
use moqui_lib::sync::lock;

pub(super) fn resource_expand(ctx: Ctx<'_>, template: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        expand_here(&ctx, &template).map_err(|err| Exception::throw_message(&ctx, err.message()))
    })
}

/// `ec.resource.script(location)`: run the file in the script's own names and return its value, as Java does: what it
/// returns, or its last expression statement.
pub(super) fn resource_script<'js>(ctx: Ctx<'js>, location: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let (script, inert) = read_module(&location).map_err(|err| throw_context(&ctx, &err))?;
        let source = script.shared_text();
        let inert = inert && module_cache_on();
        // A module whose top level only declares gives the same value each time it runs: the value it gave earlier in
        // this outermost run is given again, its functions not made again (`parse::module_is_inert`).
        if inert {
            if let Some(kept) = super::kept_module(&ctx, &location, source) {
                return Ok(kept);
            }
        }
        let value = eval_compiled_text::<JsValue>(&ctx, &script, false).map_err(|err| throw_context(&ctx, &err))?;
        if inert {
            super::keep_module(&ctx, &location, Arc::clone(source), &value);
        }
        Ok(value)
    })
}

/// Whether module values are kept for the run: the `script_module_cache` property, true unless `false` (read once).
pub(crate) fn module_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| moqui_lib::env::get_prop_or_env("script_module_cache") != "false")
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

fn script_texts() -> &'static Mutex<HashMap<String, ScriptText>> {
    static TEXTS: std::sync::OnceLock<Mutex<HashMap<String, ScriptText>>> = std::sync::OnceLock::new();
    TEXTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn modified_of(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// Drop the kept text of every script file among `files` at once, rather than at the next check (plans/design-dev-aide.md
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

/// A script file to run, kept by location as Java's `resource.script*.location` caches and read again when the file
/// changes: its text as the body of a function called at once (see [`crate::script::parse::file_function`]), so it runs
/// in names of its own and evaluates to what it returns. The compiled form is kept by the runtime by text.
pub(super) fn read_script(location: &str) -> Result<Arc<str>, Error> {
    read_module(location).map(|(script, _)| Arc::clone(script.shared_text()))
}

/// [`read_script`], and whether the file's top level only declares, so its value may be kept for the run.
pub(crate) fn read_module(location: &str) -> Result<(super::ScriptText, bool), Error> {
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
    let text = std::fs::read_to_string(&path)
        .map_err(|err| Error::new(format!("could not read script {location}")).with_source(err))?;
    let inert = crate::script::parse::module_is_inert(&text);
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

/// `System.getProperty(name)`: a property or environment value, null when it is not set.
pub(super) fn system_get_property(ctx: Ctx<'_>, name: String) -> rquickjs::Result<Option<String>> {
    host_guard(&ctx, || {
        Ok(Some(moqui_lib::env::get_prop_or_env(&name)).filter(|value| !value.is_empty()))
    })
}
