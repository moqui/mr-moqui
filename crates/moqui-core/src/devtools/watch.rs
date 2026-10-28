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

//! The dev file watcher (plans/design-dev-aide.md 6.3, F6): on a dev instance of a package with `devtools`, with
//! `dev-reload` on (it is unless `enabled="false"`), every component directory is watched, and a change reloads what
//! the file defines through [`super::reload_paths`], so an editor or an agent in a terminal gets live reload without
//! the dev-aide. Events are gathered until `debounce-ms` (300) pass without one, so an editor's write and rename is one
//! change. A write the dev-aide's save already reloaded is not reloaded again.
//!
//! Each directory is watched on its own, not recursively, so `.git`, `target`, `node_modules` and the like are never
//! watched; a directory created later is watched when it appears. A directory that cannot be watched is logged and the
//! others go on.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher as _};

use crate::context::ExecutionContextFactory;

/// Directory names never watched, anywhere in a component.
const SKIPPED: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "build",
    ".gradle",
    ".idea",
    ".venv",
    "__pycache__",
];

/// The running watcher; the factory keeps it, and it stops when the factory is gone.
pub struct DevWatcher {
    watcher: Mutex<notify::RecommendedWatcher>,
    watched: Mutex<BTreeSet<PathBuf>>,
}

/// Start watching `factory`'s components when the instance is dev and `dev-reload` is on. Answers whether it started.
pub fn start(factory: &Arc<ExecutionContextFactory>) -> bool {
    if !super::is_dev_instance() {
        return false;
    }
    if !factory.conf_flag("dev-reload", "enabled", true) {
        tracing::info!(
            "Dev reload is off (dev-reload enabled=false); changed files take effect as they do in production"
        );
        return false;
    }
    let debounce = Duration::from_millis(factory.conf_number("dev-reload", "debounce-ms", 300_u64));
    watch(factory, debounce)
}

/// Watch `factory`'s components and reload what changes, `debounce` after the last change of a burst.
pub(crate) fn watch(factory: &Arc<ExecutionContextFactory>, debounce: Duration) -> bool {
    let (sender, events) = channel();
    let watcher = match notify::recommended_watcher(sender) {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::error!(error = %err, "Could not start the dev file watcher; changed files will not reload");
            return false;
        }
    };
    let dev = Arc::new(DevWatcher {
        watcher: Mutex::new(watcher),
        watched: Mutex::new(BTreeSet::new()),
    });
    let started = std::time::Instant::now();
    for component in factory.components() {
        dev.watch_tree(&component.location);
    }
    let conf = factory.runtime_dir().join("conf");
    if conf.is_dir() {
        dev.watch_tree(&conf);
    }
    tracing::info!(
        "Dev reload: watching {} directories of {} components in {} ms; a changed file reloads what it defines",
        lock(&dev.watched).len(),
        factory.components().len(),
        started.elapsed().as_millis()
    );
    factory.extension::<Arc<DevWatcher>>(|_| Arc::clone(&dev));
    let weak = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("dev-reload".into())
        .spawn(move || run(&weak, &dev, &events, debounce));
    if let Err(err) = spawned {
        tracing::error!(error = %err, "Could not start the dev reload thread");
        return false;
    }
    true
}

impl DevWatcher {
    /// Watch `dir` and each directory under it but the skipped ones.
    fn watch_tree(&self, dir: &Path) {
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            if !self.watch_one(&dir) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                if is_dir && !skipped(&path) {
                    pending.push(path);
                }
            }
        }
    }

    fn watch_one(&self, dir: &Path) -> bool {
        if !lock(&self.watched).insert(dir.to_path_buf()) {
            return false;
        }
        match lock(&self.watcher).watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(error = %err, "Dev reload cannot watch {}; its files will not reload", dir.display());
                false
            }
        }
    }
}

fn run(
    factory: &Weak<ExecutionContextFactory>,
    dev: &DevWatcher,
    events: &Receiver<notify::Result<notify::Event>>,
    debounce: Duration,
) {
    let mut changed: BTreeSet<PathBuf> = BTreeSet::new();
    // when the changes are taken: `debounce` after the last change, however many other events (a file opened, as the
    // index build or an editor does) come meanwhile
    let mut due: Option<Instant> = None;
    loop {
        let wait = match due {
            Some(at) => at.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(5),
        };
        let event = if wait.is_zero() {
            Err(RecvTimeoutError::Timeout)
        } else {
            events.recv_timeout(wait)
        };
        match event {
            Ok(Ok(event)) => {
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                for path in event.paths {
                    if path.is_dir() {
                        // A new directory: watch it and what is in it, and take its files as changed.
                        if !skipped(&path) && !lock(&dev.watched).contains(&path) {
                            dev.watch_tree(&path);
                            collect_files(&path, &mut changed);
                        }
                        continue;
                    }
                    if !ignored_name(&path) {
                        changed.insert(path);
                    }
                }
                if !changed.is_empty() {
                    due = Some(Instant::now() + debounce);
                }
            }
            Ok(Err(err)) => tracing::warn!(error = %err, "Dev reload watch error"),
            Err(RecvTimeoutError::Timeout) => {
                due = None;
                let Some(factory) = factory.upgrade() else {
                    return;
                };
                if changed.is_empty() {
                    continue;
                }
                let paths: Vec<PathBuf> = std::mem::take(&mut changed)
                    .into_iter()
                    .filter(|path| !super::reload::already_reloaded(&factory, path))
                    .collect();
                if paths.is_empty() {
                    continue;
                }
                let report = super::reload_paths(&factory, &paths);
                if report.files.is_empty() {
                    continue;
                }
                let problems = report.problems().count();
                if problems > 0 {
                    tracing::warn!("Dev reload: {} ({problems} problems)", report.summary());
                    for problem in report.problems() {
                        tracing::warn!("  {}: {}", problem.path, problem.message);
                    }
                } else {
                    tracing::info!("Dev reload: {}", report.summary());
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn collect_files(dir: &Path, into: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !skipped(&path) {
                collect_files(&path, into);
            }
        } else if !ignored_name(&path) {
            into.insert(path);
        }
    }
}

fn skipped(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SKIPPED.contains(&name))
}

/// An editor's backup, swap or temporary file.
fn ignored_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    name.starts_with(".#")
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name == "4913"
        || (name.starts_with('.') && name.contains(".moqui-save"))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_directories_and_editor_files() {
        assert!(skipped(Path::new("/x/comp/.git")));
        assert!(skipped(Path::new("/x/comp/target")));
        assert!(!skipped(Path::new("/x/comp/service")));
        assert!(ignored_name(Path::new("/x/a.xml~")));
        assert!(ignored_name(Path::new("/x/.a.xml.swp")));
        assert!(ignored_name(Path::new("/x/.#a.xml")));
        assert!(ignored_name(Path::new("/x/4913")));
        assert!(!ignored_name(Path::new("/x/a.xml")));
    }
}
