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

//! The embedded Postgres of a scratch runtime stopped however a run ends.
//!
//! `pg_ctl` starts the postmaster in a session of its own, so it outlives the test process: a run stopped by SIGINT,
//! SIGTERM or SIGHUP, or by Ctrl-C on Windows, skips the fixture's drop and leaves the postmaster running on a
//! `/tmp/mt-<pid>-<nanos>` directory. Once a scratch runtime is tracked, those signals stop its postmasters and
//! remove it (a kept one stays), then end the process. A run that is killed outright is cleaned up by the next one:
//! [`remove_stale`] stops the postmasters of the scratch runtimes whose process is gone.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use moqui_lib::sync::lock;

/// A tracked scratch runtime, the install whose `pg_ctl` stops its postmasters, and whether `--keep` keeps it.
struct Tracked {
    dir: PathBuf,
    install: PathBuf,
    keep: bool,
}

static TRACKED: Mutex<Vec<Tracked>> = Mutex::new(Vec::new());

/// Set once the `--serve` server is up: it stops on the signals itself, and removes the scratch runtime after.
static SERVING: AtomicBool = AtomicBool::new(false);

/// Stop the postmasters of `dir` and remove it, unless kept, when the process gets SIGINT, SIGTERM or SIGHUP.
pub(super) fn track(dir: &Path, install: PathBuf, keep: bool) {
    watch_signals();
    lock(&TRACKED).push(Tracked {
        dir: dir.to_path_buf(),
        install,
        keep,
    });
}

/// `dir` is removed, or about to be: a signal no longer touches it.
pub(super) fn untrack(dir: &Path) {
    lock(&TRACKED).retain(|tracked| tracked.dir != dir);
}

/// The `--serve` server handles the signals from here on.
pub(super) fn server_started() {
    SERVING.store(true, Ordering::Relaxed);
}

/// The data directories of a scratch runtime that a postmaster may run on: the entity cluster and each sim overlay.
fn data_dirs(dir: &Path) -> Vec<PathBuf> {
    let cluster = dir.join("db/embedded");
    let mut dirs = vec![cluster.join("data")];
    if let Ok(entries) = std::fs::read_dir(&cluster) {
        dirs.extend(
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_str().is_some_and(|name| name.starts_with("sim-")))
                .map(|entry| entry.path().join("data")),
        );
    }
    dirs
}

/// Stop each postmaster of a scratch runtime: at once when the runtime goes, cleanly when it is kept.
fn stop_postmasters(dir: &Path, install: &Path, keep: bool) {
    let mode = if keep { "fast" } else { "immediate" };
    for data_dir in data_dirs(dir) {
        crate::db::embedded::stop_postmaster(&data_dir, install, mode);
    }
}

/// The pid in a scratch runtime's name, `mt-<pid>-<nanos>`.
fn owner_pid(name: &str) -> Option<u32> {
    let (pid, nanos) = name.strip_prefix("mt-")?.split_once('-')?;
    if nanos.is_empty() || !nanos.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    pid.parse().ok()
}

/// Stop the postmasters of the scratch runtimes under `/tmp` whose process is gone, and remove those runtimes unless
/// kept with `--keep`. A runtime of a live process, or of another user, is left as it is.
pub(super) fn remove_stale(install: &Path) {
    let Ok(entries) = std::fs::read_dir("/tmp") else {
        return;
    };
    let own = std::process::id();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(owner_pid) else {
            continue;
        };
        let dir = entry.path();
        if pid == own || crate::db::embedded::process_live(pid) || !dir.is_dir() || !owned(&dir) {
            continue;
        }
        let keep = dir.join(KEEP_MARK).exists();
        stop_postmasters(&dir, install, keep);
        if keep {
            continue;
        }
        tracing::info!(
            "Removing the scratch runtime {} of process {pid}, which is gone",
            dir.display()
        );
        remove_scratch(&dir);
    }
}

/// The file in a scratch runtime kept with `--keep`, which [`remove_stale`] leaves in place.
pub(super) const KEEP_MARK: &str = ".moqui-test-keep";

/// Remove `dir`. A directory link inside it (a symlink, or a junction on Windows) is taken off first, so the
/// directory it names stays. What is left, the copied conf and the database, then goes. A directory that is already
/// gone is not an error. A link that is still there on Windows is left in place: removing the tree would follow it.
pub(super) fn remove_scratch(dir: &Path) {
    if !detach_links(dir) {
        tracing::warn!(
            "could not remove the scratch runtime {}: a link inside it is still there",
            dir.display()
        );
        return;
    }
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        // another run's sweep removed it first (test runs start side by side)
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!("could not remove the scratch runtime {}: {err}", dir.display()),
    }
}

/// `true` when every directory link under `dir` is gone. A real directory is left for [`std::fs::remove_dir_all`].
/// The names are what a scratch runtime links: a component set as one directory, each add-on component on its own,
/// the install and Chromium. The add-on directory itself is real, so its children are the links.
fn detach_links(dir: &Path) -> bool {
    let mut clear = true;
    for name in [
        "components/base",
        "components/dev",
        "components/test",
        "install",
        "chromium",
    ] {
        clear &= detach_dir(&dir.join(name));
    }
    // the old set directories, so a scratch runtime from before `components/` is still safe to remove
    for name in ["base-component", "dev-component"] {
        clear &= detach_dir(&dir.join(name));
    }
    for name in ["components/addon", "component"] {
        if let Ok(entries) = std::fs::read_dir(dir.join(name)) {
            for entry in entries.flatten() {
                clear &= detach_dir(&entry.path());
            }
        }
    }
    for name in ["MoquiConf.xml", "components/components.xml"] {
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        // a file link that did not go must not be followed either
        if is_link(&path) {
            clear = false;
        }
    }
    clear
}

/// Remove a directory link at `path`. A path that is not there counts as done. On Windows a link that
/// [`std::fs::remove_dir`] leaves behind stops the removal of the tree; elsewhere the tree removal does not follow a
/// link, so a link left here is still safe.
fn detach_dir(path: &Path) -> bool {
    if std::fs::symlink_metadata(path).is_err() {
        return true;
    }
    if std::fs::remove_dir(path).is_ok() {
        return true;
    }
    #[cfg(windows)]
    {
        !is_link(path)
    }
    #[cfg(not(windows))]
    {
        true
    }
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

#[cfg(unix)]
fn owned(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    // SAFETY: `geteuid` has no preconditions and does not fail.
    let uid = unsafe { libc::geteuid() };
    std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.uid() == uid)
}

/// This process can write in `dir`, which on Windows is how a scratch runtime of this user is told from another
/// user's. The probe file goes as soon as it is created.
#[cfg(windows)]
fn owned(dir: &Path) -> bool {
    let probe = dir.join(format!(".moqui-test-owned-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(not(any(unix, windows)))]
fn owned(_dir: &Path) -> bool {
    false
}

/// Clean up what is tracked, then end the process with the status of the signal.
fn interrupted(status: i32) -> ! {
    let tracked = std::mem::take(&mut *lock(&TRACKED));
    for Tracked { dir, install, keep } in tracked {
        eprintln!("Interrupted, stopping the test database in {}", dir.display());
        stop_postmasters(&dir, &install, keep);
        if !keep {
            remove_scratch(&dir);
        }
    }
    std::process::exit(status);
}

/// Listen for SIGINT, SIGTERM and SIGHUP on a thread of its own, once. The handlers are registered before this returns,
/// so no signal after it ends the process the default way.
#[cfg(unix)]
fn watch_signals() {
    static WATCHING: std::sync::Once = std::sync::Once::new();
    WATCHING.call_once(|| {
        use tokio::signal::unix::{signal, SignalKind};
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!("Not watching for signals, an interrupted run may leave its database running: {err}");
                return;
            }
        };
        let signals = {
            let _entered = runtime.enter();
            (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
                signal(SignalKind::hangup()),
            )
        };
        let (Ok(mut interrupt), Ok(mut terminate), Ok(mut hangup)) = signals else {
            tracing::warn!("Not watching for signals, an interrupted run may leave its database running");
            return;
        };
        let started = std::thread::Builder::new()
            .name("moqui-test-signals".to_string())
            .spawn(move || {
                runtime.block_on(async {
                    loop {
                        let status = tokio::select! {
                            _ = interrupt.recv() => 130,
                            _ = terminate.recv() => 143,
                            _ = hangup.recv() => 129,
                        };
                        if !SERVING.load(Ordering::Relaxed) {
                            interrupted(status);
                        }
                    }
                });
            });
        if let Err(err) = started {
            tracing::warn!("Not watching for signals, an interrupted run may leave its database running: {err}");
        }
    });
}

/// Listen for Ctrl-C, once. The handler is registered before this returns.
#[cfg(windows)]
fn watch_signals() {
    static WATCHING: std::sync::Once = std::sync::Once::new();
    WATCHING.call_once(|| {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!("Not watching for signals, an interrupted run may leave its database running: {err}");
                return;
            }
        };
        let ctrl = {
            let _entered = runtime.enter();
            match tokio::signal::windows::ctrl_c() {
                Ok(ctrl) => ctrl,
                Err(err) => {
                    tracing::warn!(
                        "Not watching for signals, an interrupted run may leave its database running: {err}"
                    );
                    return;
                }
            }
        };
        let started = std::thread::Builder::new()
            .name("moqui-test-signals".to_string())
            .spawn(move || {
                runtime.block_on(async {
                    let mut ctrl = ctrl;
                    loop {
                        if ctrl.recv().await.is_none() {
                            break;
                        }
                        if !SERVING.load(Ordering::Relaxed) {
                            interrupted(130);
                        }
                    }
                });
            });
        if let Err(err) = started {
            tracing::warn!("Not watching for signals, an interrupted run may leave its database running: {err}");
        }
    });
}

#[cfg(not(any(unix, windows)))]
fn watch_signals() {}

#[cfg(test)]
mod tests {
    use super::owner_pid;

    #[test]
    fn the_owner_pid_is_read_from_the_scratch_runtime_name() {
        assert_eq!(owner_pid("mt-1234-1a2b3c"), Some(1234));
        assert_eq!(owner_pid("mt-1234-"), None);
        assert_eq!(owner_pid("mt-1234-xyz"), None);
        assert_eq!(owner_pid("mt-abc-1a2b"), None);
        assert_eq!(owner_pid("moqui-1234-1a2b"), None);
    }
}
