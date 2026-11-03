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

//! The node's cookies, kept by the host for the window (plans/Mobile-Desktop.md §4 configuration 2): the window's pages
//! are on the app scheme in process, where the host forwards each request to the node's router, so the host plays the
//! browser's part for the node's session and visitor cookies. The node signs the window's user in with its own login
//! page and session, as on the web (the CSRF token, `/termws`, a person's approvals), and the page never sees the cookie:
//! `Set-Cookie` is taken out of each answer and `Cookie` put into each request here.
//!
//! The cookies are kept in a file only the user can read (`node-cookies` in the app's config directory), so the window
//! stays signed in across a restart for as long as the node keeps the session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The node's cookies by name.
pub(crate) struct Jar {
    cookies: Mutex<BTreeMap<String, String>>,
    file: Option<PathBuf>,
}

impl Jar {
    /// The jar kept in `file`, with what it holds now.
    pub(crate) fn open(file: PathBuf) -> Self {
        let cookies = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            cookies: Mutex::new(cookies),
            file: Some(file),
        }
    }

    /// A jar kept in memory only.
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            cookies: Mutex::new(BTreeMap::new()),
            file: None,
        }
    }

    /// The `Cookie` header of a request, if there is a cookie.
    pub(crate) fn header(&self) -> Option<String> {
        let cookies = moqui_lib::sync::lock(&self.cookies);
        (!cookies.is_empty()).then(|| {
            cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }

    /// Take an answer's `Set-Cookie` values: each sets its cookie, or removes it when it has expired (`Max-Age` of 0 or
    /// less, or an empty value). Kept in the file when any changed.
    pub(crate) fn take<'a>(&self, set_cookies: impl IntoIterator<Item = &'a str>) {
        let mut changed = false;
        let saved = {
            let mut cookies = moqui_lib::sync::lock(&self.cookies);
            for line in set_cookies {
                let Some((name, value, expired)) = parse(line) else {
                    continue;
                };
                if expired {
                    changed |= cookies.remove(&name).is_some();
                } else if cookies.get(&name) != Some(&value) {
                    cookies.insert(name, value);
                    changed = true;
                }
            }
            changed.then(|| cookies.clone())
        };
        if let (Some(cookies), Some(file)) = (saved, &self.file) {
            save(file, &cookies);
        }
    }
}

/// The name and value of a `Set-Cookie` line, and whether it removes the cookie.
pub(crate) fn parse(line: &str) -> Option<(String, String, bool)> {
    let mut parts = line.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let (name, value) = (name.trim(), value.trim().trim_matches('"'));
    // a cookie name is a token; a value has no separators that would end it in a header
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_control() || " \t()<>@,;:\\\"/[]?={}".contains(c))
    {
        return None;
    }
    if value.chars().any(|c| c.is_control() || c == ';' || c == ',') {
        return None;
    }
    let expired = value.is_empty()
        || parts.any(|attribute| {
            let (key, value) = attribute.split_once('=').unwrap_or((attribute, ""));
            key.trim().eq_ignore_ascii_case("max-age") && value.trim().parse::<i64>().is_ok_and(|age| age <= 0)
        });
    Some((name.to_string(), value.to_string(), expired))
}

/// Write the cookies to `file`, only the user may read it.
fn save(file: &Path, cookies: &BTreeMap<String, String>) {
    let written = serde_json::to_vec(cookies)
        .map_err(std::io::Error::other)
        .and_then(|bytes| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let part = file.with_extension("part");
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            std::io::Write::write_all(&mut options.open(&part)?, &bytes)?;
            std::fs::rename(&part, file)
        });
    if let Err(err) = written {
        tracing::warn!("The node's cookies were not kept in {}: {err}", file.display());
    }
}

#[cfg(test)]
#[path = "cookies_tests.rs"]
mod tests;
