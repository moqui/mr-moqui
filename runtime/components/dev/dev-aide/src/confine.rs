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

//! Path confinement (plans/design-dev-aide.md 4.1): a path the client names is relative to a root, and resolves inside
//! the root's canonical directory or not at all: no absolute path (`/x`, `C:\x`, `\\server\share`, `\\?\`), no `..`, no
//! symlink that leads out, nothing under `.git`, and nothing on the deny list (the runtime's database and its saved
//! copies, logs, transaction log, temporary files, `.env` and its copies, certificates, SFTP root and installed
//! programs, and a root's `target`, though not the loaded runtime or components when they sit there). The runtime's conf
//! files are for the full tier only.

use std::path::{Component, Path, PathBuf};

use moqui_lib::error::Error;

/// Names never shown or opened under a root.
pub const HIDDEN: &[&str] = &[".git"];

/// The runtime directories and files no root may reach, relative to the runtime directory: the database and its saved
/// copies (`save`), the logs, the transaction log, temporary files, the generated secrets (`.env`, and any copy of it
/// beside it, [`ENV_PREFIX`]), the certificates, the SFTP root, and the installed programs (`install`, `opensearch`).
pub const DENIED_RUNTIME: &[&str] = &[
    "db",
    "log",
    "txlog",
    "tmp",
    ".env",
    "cert",
    "save",
    "sftp",
    "install",
    "opensearch",
];

/// A name at the top of the runtime that starts so is the secrets file or a copy of it (`.env.bak`, `.env.local`).
pub const ENV_PREFIX: &str = ".env";

/// The runtime directory of the conf files (datasource passwords, the crypt pass when set there): read and written
/// only at the full tier ([`hidden_below_full`]).
pub const CONF: &str = "conf";

/// The runtime directory of the agents' worktrees (plans/design-dev-aide.md 21.8): reached only as their own roots.
pub const WORKTREES: &str = "worktrees";

/// A path the client may use, inside `root`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The full path (canonical up to the deepest part that exists).
    pub path: PathBuf,
    /// The path relative to the root, with `/` separators.
    pub relative: String,
    /// The path relative to the root where it really leads, a symlink on the way followed: what a lock by place (the
    /// upstream lock) is checked on.
    pub real: String,
}

/// Resolve `relative` (with `/` or `\` separators) in `root`, a canonical directory. `components` are the loaded
/// components' directories, which are not build output though a root's `target` holds them ([`build_output`]).
///
/// # Errors
///
/// Returns an error naming the rule a path breaks.
pub fn resolve(root: &Path, runtime: &Path, components: &[&Path], relative: &str) -> Result<Resolved, Error> {
    let parts = parts(relative)?;
    let mut path = root.to_path_buf();
    for part in &parts {
        path.push(part);
    }
    // the deepest part that exists, canonical, so a symlink anywhere on the way is followed
    let mut existing = path.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (
            existing.file_name().map(std::ffi::OsStr::to_os_string),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut canonical = existing
        .canonicalize()
        .map_err(|err| Error::new(format!("{relative}: {err}")))?;
    for name in rest.iter().rev() {
        canonical.push(name);
    }
    if !inside(&canonical, root) {
        return Err(Error::new(format!("{relative} leads outside its root")));
    }
    let runtime = runtime.canonicalize().ok();
    if let Ok(after_root) = canonical.strip_prefix(root) {
        if after_root
            .components()
            .any(|part| matches!(part, Component::Normal(name) if HIDDEN.iter().any(|hidden| name == *hidden)))
        {
            return Err(Error::new(format!(
                "{relative} is under .git, which the dev-aide does not open"
            )));
        }
    }
    if build_output(&canonical, root, runtime.as_deref(), components) {
        return Err(Error::new(format!("{relative} is a build output (target)")));
    }
    if let Some(runtime) = runtime {
        for denied in DENIED_RUNTIME {
            if inside(&canonical, &runtime.join(denied)) {
                return Err(Error::new(format!(
                    "{relative} is runtime/{denied}, which the dev-aide does not open"
                )));
            }
        }
        if env_copy(&canonical, &runtime) {
            return Err(Error::new(format!(
                "{relative} is a copy of runtime/.env, which the dev-aide does not open"
            )));
        }
        // the agents' worktrees are roots of their own (crate::worktree), not files of the tree that holds the runtime
        let worktrees = runtime.join(WORKTREES);
        if inside(&canonical, &worktrees) && !inside(root, &worktrees) {
            return Err(Error::new(format!(
                "{relative} is in runtime/{WORKTREES}, which are roots of their own"
            )));
        }
    }
    let real = canonical.strip_prefix(root).map_or_else(
        |_| parts.join("/"),
        |after| {
            after
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        },
    );
    Ok(Resolved {
        path: canonical,
        relative: parts.join("/"),
        real,
    })
}

/// Whether `path` (canonical, in `root`) is build output: under the root's `target`. The loaded runtime and components
/// are not, though they sit there: a disposable runtime under the repository's `target` (the testing skill's), or the
/// components a test run links into its scratch runtime from one, which the repository holds as their root
/// (crate::roots). A path in one of them is measured from the deepest that holds it, so only that one's own `target`
/// is refused. `runtime` and `path` are canonical (`path` up to its deepest existing part) and the components are made
/// so, so a symlink in one of them that leads to the root's other build output is measured from the root and refused.
fn build_output(path: &Path, root: &Path, runtime: Option<&Path>, components: &[&Path]) -> bool {
    let under_target = |base: &Path| {
        path.strip_prefix(base)
            .ok()
            .and_then(|after| after.components().next())
            .is_some_and(|first| first.as_os_str() == "target")
    };
    if !under_target(root) {
        return false;
    }
    let target = root.join("target");
    let loaded = components.iter().filter_map(|dir| dir.canonicalize().ok());
    let base = runtime
        .map(Path::to_path_buf)
        .into_iter()
        .chain(loaded)
        .filter(|dir| inside(dir, &target) && !inside(&target, dir) && inside(path, dir))
        .max_by_key(|dir| dir.components().count());
    base.is_none_or(|base| under_target(&base))
}

/// Whether `path` (canonical) is a name at the top of `runtime` (canonical) that starts with [`ENV_PREFIX`], or under one.
fn env_copy(path: &Path, runtime: &Path) -> bool {
    path.strip_prefix(runtime).is_ok_and(|after| {
        after
            .components()
            .next()
            .is_some_and(|first| first.as_os_str().to_string_lossy().starts_with(ENV_PREFIX))
    })
}

/// Whether `path` (a resolved one) is in the runtime's conf directory, which a tier below full does not read or write
/// (SEC-DEV-04): the conf files hold the datasource passwords.
#[must_use]
pub fn hidden_below_full(path: &Path, runtime: &Path) -> bool {
    runtime
        .canonicalize()
        .is_ok_and(|runtime| inside(path, &runtime.join(CONF)))
}

/// Whether `location` (canonical) may become a root of a user who is not an admin: a directory in the runtime or in a
/// root there is already, never one elsewhere on the host such as `/` or a home directory, which every tier would then
/// read (SEC-DEV-04).
#[must_use]
pub fn root_location_open(location: &Path, runtime: &Path, roots: &[&Path]) -> bool {
    let in_runtime = runtime.canonicalize().is_ok_and(|runtime| inside(location, &runtime));
    in_runtime || roots.iter().any(|root| inside(location, root))
}

/// The parts of a relative path, refusing an absolute one and `..`.
fn parts(relative: &str) -> Result<Vec<String>, Error> {
    if relative.contains('\0') {
        return Err(Error::new("a path with a NUL in it"));
    }
    let bytes = relative.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if relative.starts_with('/') || relative.starts_with('\\') || drive {
        return Err(Error::new(format!(
            "{relative} is absolute; paths are relative to their root"
        )));
    }
    let mut out = Vec::new();
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return Err(Error::new(format!("{relative} has .. in it"))),
            _ => out.push(part.to_string()),
        }
    }
    Ok(out)
}

/// Whether `path` is `dir` or under it. On Windows the comparison ignores case, as the file system does.
#[must_use]
pub fn inside(path: &Path, dir: &Path) -> bool {
    #[cfg(windows)]
    {
        let fold = |path: &Path| path.to_string_lossy().to_lowercase().replace('/', "\\");
        let (path, dir) = (fold(path), fold(dir));
        let dir = dir.trim_end_matches('\\');
        path == dir || path.starts_with(&format!("{dir}\\"))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(dir)
    }
}

/// Whether the first 8 KB of `bytes` look like a binary file (a NUL byte).
#[must_use]
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

#[cfg(test)]
#[path = "confine_tests.rs"]
mod tests;
