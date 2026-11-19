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

//! What a process that was killed left (SEC-GIT-92): a push's quarantine (`objects/incoming-<pid>-<n>` of a
//! repository on disk, `runtime/tmp/git-<id>-incoming-<pid>-<n>` of a `db` one), gc's scratch pack directory
//! (`objects/gc-<pid>-<millis>`), and the scratch directories of copies and imports under `runtime/tmp`
//! (`git-<what>-<id>-<pid>-<n>`, `git-import-<id>-<pid>-<millis>`). A refused push and a gc that ends remove their
//! own; these sweeps remove one whose process is not this one nor a live process here, untouched for an hour, at start
//! and in each `gc#GitRepositories`. A directory that cannot be removed is logged and the others go on.

use std::path::Path;
use std::time::{Duration, SystemTime};

/// How long a directory of another process is left alone, whether or not that process can be seen: a member of a
/// cluster on shared storage has its own process ids.
pub(crate) const LEFT_FOR: Duration = Duration::from_secs(60 * 60);

/// The process id a scratch or quarantine name ends with (`...-<pid>-<n>`).
fn pid_of(name: &str) -> Option<u32> {
    let mut parts = name.rsplitn(3, '-');
    let last = parts.next()?;
    let pid = parts.next()?;
    parts.next()?;
    if last.is_empty() || !last.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

/// Whether process `pid` is running on this machine; true when that cannot be told (no `/proc`), so only the age
/// decides then.
fn alive(pid: u32) -> bool {
    let proc = Path::new("/proc");
    !proc.is_dir() || proc.join(pid.to_string()).exists()
}

/// The newest change of `dir` or of an entry directly in it.
fn last_touched(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).ok()?.modified().ok()?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(modified) = entry.metadata().ok().and_then(|meta| meta.modified().ok()) {
                newest = newest.max(modified);
            }
        }
    }
    Some(newest)
}

/// Whether the directory `path` named `name` is a dead process's, and old enough to go.
fn left_behind(path: &Path, name: &str, now: SystemTime) -> bool {
    let Some(pid) = pid_of(name) else {
        return false;
    };
    // a process here with the pid is taken as the writer still: a pid is reused, but then the directory waits only
    // until that process ends
    if pid == std::process::id() || alive(pid) {
        return false;
    }
    last_touched(path)
        .and_then(|touched| now.duration_since(touched).ok())
        .is_some_and(|age| age > LEFT_FOR)
}

fn remove(path: &Path) -> bool {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {
            tracing::info!("removed {}, which a git process that stopped left", path.display());
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            tracing::error!(
                "cannot remove {}, which a git process that stopped left: {err}",
                path.display()
            );
            false
        }
    }
}

/// Remove the quarantine and gc scratch directories a dead process left in a repository's `objects` directory. The
/// count removed.
pub(crate) fn sweep_objects(objects: &Path) -> usize {
    sweep(objects, |name| name.starts_with("incoming-") || name.starts_with("gc-"))
}

/// Remove the scratch and quarantine directories a dead process left in `runtime/tmp` (`git-*`; the archives'
/// `git-archive` has its own sweep). The count removed.
pub(crate) fn sweep_tmp(runtime_dir: &Path) -> usize {
    sweep(&runtime_dir.join("tmp"), |name| {
        name.starts_with("git-") && name != "git-archive"
    })
}

/// At start: `runtime/tmp`, and the `objects` directory of each repository under `runtime/git`.
pub(crate) fn sweep_at_start(runtime_dir: &Path) {
    let mut removed = sweep_tmp(runtime_dir);
    if let Ok(entries) = std::fs::read_dir(runtime_dir.join("git")) {
        for entry in entries.flatten() {
            removed += sweep_objects(&entry.path().join("objects"));
        }
    }
    if removed > 0 {
        tracing::warn!("removed {removed} git quarantine and scratch directories that stopped processes left");
    }
}

fn sweep(dir: &Path, wanted: impl Fn(&str) -> bool) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !wanted(&name) || !path.is_dir() || !left_behind(&path, &name, now) {
            continue;
        }
        if remove(&path) {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
#[path = "leftovers_tests.rs"]
mod tests;
