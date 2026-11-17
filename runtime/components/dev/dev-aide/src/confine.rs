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
//! programs, and a root's `target`). The runtime's conf files are for the full tier only.

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

/// Resolve `relative` (with `/` or `\` separators) in `root`, a canonical directory.
///
/// # Errors
///
/// Returns an error naming the rule a path breaks.
pub fn resolve(root: &Path, runtime: &Path, relative: &str) -> Result<Resolved, Error> {
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
    if let Ok(after_root) = canonical.strip_prefix(root) {
        if after_root
            .components()
            .any(|part| matches!(part, Component::Normal(name) if HIDDEN.iter().any(|hidden| name == *hidden)))
        {
            return Err(Error::new(format!(
                "{relative} is under .git, which the dev-aide does not open"
            )));
        }
        if after_root
            .components()
            .next()
            .is_some_and(|first| first.as_os_str() == "target")
        {
            return Err(Error::new(format!("{relative} is a build output (target)")));
        }
    }
    if let Ok(runtime) = runtime.canonicalize() {
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
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(label: &str) -> (Scratch, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("dev-aide-confine-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let runtime = base.join("runtime");
        let root = runtime.join("components/addon/site");
        std::fs::create_dir_all(root.join("service")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(runtime.join("db")).unwrap();
        std::fs::write(root.join("service/A.xml"), "<services/>").unwrap();
        let root = root.canonicalize().unwrap();
        (Scratch(base), root, runtime)
    }

    #[test]
    fn a_relative_path_resolves_inside_its_root() {
        let (_scratch, root, runtime) = scratch("ok");
        let found = resolve(&root, &runtime, "service/A.xml").unwrap();
        assert_eq!(found.path, root.join("service/A.xml"));
        assert_eq!(found.relative, "service/A.xml");
        // a file that does not exist yet, under a directory that does not either
        let new = resolve(&root, &runtime, "screen\\New/Page.xml").unwrap();
        assert_eq!(new.relative, "screen/New/Page.xml");
        assert!(new.path.starts_with(&root));
        assert_eq!(resolve(&root, &runtime, "").unwrap().path, root);
    }

    #[test]
    fn what_leads_out_or_is_denied_is_refused() {
        let (_scratch, root, runtime) = scratch("refused");
        for (path, why) in [
            ("../other/x.xml", ".."),
            ("service/../../x", ".."),
            ("/etc/passwd", "absolute"),
            ("\\\\server\\share\\x", "absolute"),
            ("\\\\?\\C:\\x", "absolute"),
            ("C:\\Windows\\x", "absolute"),
            ("c:/x", "absolute"),
            (".git/config", ".git"),
            ("service/.git/x", ".git"),
            ("target/debug/x", "target"),
            ("a\0b", "NUL"),
        ] {
            let err = resolve(&root, &runtime, path).unwrap_err().to_string();
            assert!(err.contains(why), "{path}: {err}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_leads_out_is_refused() {
        let (scratch, root, runtime) = scratch("symlink");
        let outside = scratch.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let err = resolve(&root, &runtime, "link/secret.txt").unwrap_err().to_string();
        assert!(err.contains("outside its root"), "{err}");
        // into the runtime's database, through a symlink inside the root
        std::os::unix::fs::symlink(runtime.join("db"), root.join("db-link")).unwrap();
        assert!(resolve(&root, &runtime, "db-link/x").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_root_resolves_to_where_it_leads() {
        let (_scratch, root, runtime) = scratch("symlink-inside");
        std::fs::create_dir_all(root.join("components/addon/upstream/entity")).unwrap();
        std::fs::create_dir_all(root.join("components/addon/my-site")).unwrap();
        std::os::unix::fs::symlink(
            root.join("components/addon/upstream"),
            root.join("components/addon/my-site/up"),
        )
        .unwrap();
        let found = resolve(&root, &runtime, "components/addon/my-site/up/entity/X.xml").unwrap();
        assert_eq!(found.relative, "components/addon/my-site/up/entity/X.xml");
        assert_eq!(found.real, "components/addon/upstream/entity/X.xml");
        assert_eq!(resolve(&root, &runtime, "service/A.xml").unwrap().real, "service/A.xml");
    }

    #[test]
    fn a_runtime_root_does_not_reach_the_denied_directories() {
        let (_scratch, _root, runtime) = scratch("runtime");
        let runtime_root = runtime.canonicalize().unwrap();
        // SEC-DEV-04: the saved databases, the SFTP root, the installed programs and copies of .env too
        for path in [
            "db/x",
            "log/moqui.log",
            ".env",
            "cert/key.pem",
            "txlog/x",
            "tmp/x",
            "save/copy/db",
            "sftp/x",
            "install/x",
            "opensearch/config/x",
            ".env.bak",
            ".env.local",
        ] {
            let err = resolve(&runtime_root, &runtime, path).unwrap_err().to_string();
            assert!(err.contains("does not open"), "{path}: {err}");
        }
        assert!(resolve(&runtime_root, &runtime, "conf/MoquiDevConf.xml").is_ok());
        assert!(resolve(&runtime_root, &runtime, "environment.md").is_ok());
    }

    /// SEC-DEV-04: the conf files are the full tier's; a root a non-admin adds is in the runtime or a root there is.
    #[test]
    fn conf_files_are_the_full_tier_s_and_a_non_admin_s_root_stays_near() {
        let (scratch, root, runtime) = scratch("conf");
        let runtime_root = runtime.canonicalize().unwrap();
        let conf = resolve(&runtime_root, &runtime, "conf/MoquiDevConf.xml").unwrap();
        assert!(hidden_below_full(&conf.path, &runtime));
        let service = resolve(&root, &runtime, "service/A.xml").unwrap();
        assert!(!hidden_below_full(&service.path, &runtime));
        let elsewhere = scratch.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let elsewhere = elsewhere.canonicalize().unwrap();
        assert!(root_location_open(&root.join("service"), &runtime, &[]));
        assert!(!root_location_open(&elsewhere, &runtime, &[]));
        assert!(!root_location_open(Path::new("/"), &runtime, &[&root]));
        assert!(root_location_open(&elsewhere, &runtime, &[&elsewhere]));
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_is_folded() {
        assert!(inside(Path::new("C:\\Work\\Site\\x.xml"), Path::new("c:\\work\\site")));
        assert!(!inside(
            Path::new("C:\\Work\\Sitex\\x.xml"),
            Path::new("c:\\work\\site")
        ));
    }

    #[test]
    fn binary_is_a_nul_byte_early() {
        assert!(is_binary(b"PNG\0\x01"));
        assert!(!is_binary("<a>ü</a>".as_bytes()));
    }
}
