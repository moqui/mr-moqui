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

//! Property and environment lookup, and `${name}` expansion.
//!
//! Expansion is string replacement. It is not a script engine. Context expansion
//! (`ResourceFacade.expand`) stays in core.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::Error;
use crate::sync::lock;

static PROPERTIES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static DOTENV: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn with_properties<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut guard = lock(&PROPERTIES);
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Set a process property, the stand-in for `System.setProperty`.
///
/// An empty value is stored and then treated as missing by [`get_prop_or_env`].
/// Expansion cannot assign a name. This function is the explicit property table.
pub fn set_property(name: &str, value: impl Into<String>) {
    let value = value.into();
    with_properties(|map| {
        map.insert(name.to_string(), value);
    });
}

/// Remove a process property so a later lookup can see the environment.
pub fn clear_property(name: &str) {
    with_properties(|map| {
        map.remove(name);
    });
}

/// The process property table only. An empty value is missing.
///
/// This does not read the environment or dotenv. [`get_prop_or_env`] does, and that is why the runtime
/// conf uses this for `MOQUI_CONF`: `runtime/.env` is loaded later and must not select the conf file.
#[must_use]
pub fn get_property(name: &str) -> Option<String> {
    with_properties(|map| map.get(name).cloned()).filter(|value| !value.is_empty())
}

fn lookup_one(name: &str) -> Option<String> {
    let property = with_properties(|map| map.get(name).cloned());
    if let Some(value) = property {
        if !value.is_empty() {
            return Some(value);
        }
    }
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => return Some(value),
        _ => {}
    }
    let dotenv = with_dotenv(|map| map.get(name).cloned());
    if let Some(value) = dotenv {
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn with_dotenv<T>(f: impl FnOnce(&mut HashMap<String, String>) -> T) -> T {
    let mut guard = lock(&DOTENV);
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Parse `KEY=value` lines into the dotenv table.
///
/// A process property and a real environment variable both win over these
/// values. Blank lines and `#` comments are ignored. A matching pair of quotes
/// is stripped. This does not expand `$` inside a value. Startup loads
/// `runtime/.env` through [`load_dotenv_file`]; that call is not made here.
pub fn load_dotenv(text: &str) {
    for (key, value) in parse_dotenv(text) {
        with_dotenv(|map| {
            map.insert(key, value);
        });
    }
}

/// The `KEY=value` pairs of dotenv text, in order, read as [`load_dotenv`] reads them, without loading them.
#[must_use]
pub fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        let value = ['"', '\'']
            .into_iter()
            .find_map(|quote| value.strip_prefix(quote)?.strip_suffix(quote))
            .unwrap_or(value);
        pairs.push((key.to_string(), value.to_string()));
    }
    pairs
}

/// Drop parsed dotenv values so the next [`load_dotenv`] or [`load_dotenv_file`] is the only source.
pub fn clear_dotenv() {
    let mut guard = lock(&DOTENV);
    if let Some(map) = guard.as_mut() {
        map.clear();
    }
}

/// The first line of a dotenv file this process writes.
pub const DOTENV_HEADER: &str = "# This file holds secrets and must be kept with the instance backup.\n";

/// Set or replace each of `set` and remove each of `remove` in the dotenv file at `path` (`runtime/.env`), then in the
/// loaded dotenv table, so a lookup sees the change without a restart; a process property of the same name still wins.
///
/// Other lines, comments and their order are kept. A name already in the file keeps its place; a new one goes at the
/// end. A new file starts with [`DOTENV_HEADER`]. The file is written whole to a sibling and renamed over the old one,
/// with mode 0600 on Unix. A name with `=` or a line break, or a value with a line break, is refused before anything is
/// written, and the error never shows a value.
pub fn update_dotenv_file(path: &std::path::Path, set: &[(String, String)], remove: &[&str]) -> Result<(), Error> {
    if set.is_empty() && remove.is_empty() {
        return Ok(());
    }
    // one read-modify-write at a time in this process, so two writers do not lose each other's change
    static WRITING: Mutex<()> = Mutex::new(());
    let _writing = lock(&WRITING);
    // a symlinked .env stays a symlink: the file it points to is the one rewritten
    let resolved = if path.is_symlink() || path.exists() {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    let path = resolved.as_path();
    let fail = |what: &str| {
        Error::new(format!(
            "could not write runtime environment file {}: {what}",
            path.display()
        ))
    };
    if path.exists() && !path.is_file() {
        return Err(fail("path is a directory"));
    }
    for (name, value) in set {
        if name.is_empty() || name.contains(['=', '\n', '\r']) || value.contains(['\n', '\r']) {
            return Err(fail("invalid name or value"));
        }
    }
    let existing = if path.is_file() {
        std::fs::read_to_string(path).map_err(|err| {
            Error::new(format!("could not read runtime environment file {}", path.display())).with_source(err)
        })?
    } else {
        String::new()
    };
    let mut written: Vec<&str> = Vec::new();
    let mut text = String::with_capacity(existing.len() + 128);
    if existing.is_empty() {
        text.push_str(DOTENV_HEADER);
    }
    for line in existing.lines() {
        if let Some(key) = dotenv_key(line) {
            if remove.contains(&key) {
                continue;
            }
            if let Some((name, value)) = set.iter().find(|(name, _)| name == key) {
                // a name repeated in the file keeps its first place only
                if !written.contains(&name.as_str()) {
                    written.push(name);
                    text.push_str(&format!("{name}={value}\n"));
                }
                continue;
            }
        }
        text.push_str(line);
        text.push('\n');
    }
    for (name, value) in set {
        if !written.contains(&name.as_str()) {
            written.push(name);
            text.push_str(&format!("{name}={value}\n"));
        }
    }
    static PARTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let partial = path.with_file_name(format!(
        ".{}.part-{}-{}",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("env"),
        std::process::id(),
        PARTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let write = || -> std::io::Result<()> {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&partial)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&partial, path)?;
        // the rename itself survives a crash once the directory is synced (Unix; a directory cannot be opened on Windows)
        #[cfg(unix)]
        if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            std::fs::File::open(dir)?.sync_all()?;
        }
        Ok(())
    };
    if let Err(err) = write() {
        let _ = std::fs::remove_file(&partial);
        return Err(
            Error::new(format!("could not write runtime environment file {}", path.display())).with_source(err),
        );
    }
    with_dotenv(|map| {
        for name in remove {
            map.remove(*name);
        }
        for (name, value) in set {
            map.insert(name.clone(), value.clone());
        }
    });
    Ok(())
}

/// The name of a `KEY=value` line as [`parse_dotenv`] reads it, or `None` for a blank line or a comment.
fn dotenv_key(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line).trim();
    let (key, _) = line.split_once('=')?;
    Some(key.trim()).filter(|key| !key.is_empty())
}

pub fn load_dotenv_file(path: impl AsRef<std::path::Path>) -> Result<(), Error> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)
        .map_err(|err| Error::new(format!("could not read dotenv {}", path.display())).with_source(err))?;
    load_dotenv(&text);
    Ok(())
}

/// Process property, then environment, then dotenv, then the same lookup with `_` replaced by `.`.
///
/// A missing name is `""`. An empty property or variable is missing.
pub fn get_prop_or_env(name: &str) -> String {
    if let Some(value) = lookup_one(name) {
        return value;
    }
    if let Some(dot) = name.find('_') {
        if dot < name.len() {
            let dotted = name.replace('_', ".");
            if dotted != name {
                if let Some(value) = lookup_one(&dotted) {
                    return value;
                }
            }
        }
    }
    String::new()
}

/// Replace `${name}` from [`get_prop_or_env`].
///
/// `name` is an identifier or a dotted property name. The Groovy Elvis form that
/// `MoquiDefaultConf.xml` uses also works: `${a ?: b}` and `${a ?: 'text'}` give the
/// first term that is not empty, and terms can chain. Anything else inside `${}`
/// is left unchanged, since Java evaluates it as Groovy. A missing name expands to
/// empty. Text with no `${` is returned as it is.
pub fn expand(value: &str) -> Result<String, Error> {
    if value.is_empty() || !value.contains("${") {
        return Ok(value.to_string());
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some((head, tail)) = rest.split_once("${") {
        out.push_str(head);
        let expanded = tail
            .split_once('}')
            .and_then(|(body, after)| Some((expand_body(body)?, after)));
        if let Some((expanded, after)) = expanded {
            out.push_str(&expanded);
            rest = after;
        } else {
            // Not a body it expands: the text stays as it is.
            out.push_str("${");
            rest = tail;
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// The value of a `${}` body, or `None` when it is not a name or an Elvis chain.
fn expand_body(body: &str) -> Option<String> {
    let mut last = String::new();
    for term in body.split("?:") {
        let term = term.trim();
        last = if is_expand_name(term) {
            get_prop_or_env(term)
        } else {
            quoted_literal(term)?.to_string()
        };
        if !last.is_empty() {
            break;
        }
    }
    Some(last)
}

/// The text of `'text'` or `"text"`. No escapes.
fn quoted_literal(term: &str) -> Option<&str> {
    ['\'', '"'].into_iter().find_map(|quote| {
        term.strip_prefix(quote)?
            .strip_suffix(quote)
            .filter(|inner| !inner.contains(quote))
    })
}

fn is_expand_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
