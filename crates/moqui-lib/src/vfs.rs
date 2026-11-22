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

//! Files read from disk, or from an [`Overlay`] mounted on a directory where the disk has none: the framework, tools,
//! webroot and conf files built into the binary (`crates/moqui-embedded`), which the host mounts on the runtime, so a
//! runtime of empty directories reads them in place and a file on disk is used instead of the built-in one.
//!
//! The functions mirror `std::fs`, and readers of runtime artifacts (conf, components, entity, service and screen
//! definitions, data files, scripts, templates, static files) use them in place of it. The disk comes first; an
//! overlay is asked only when the disk has no such file or directory, and a directory lists the disk's entries and the
//! overlay's together. With nothing mounted each one is the `std::fs` call.

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

/// Files mounted on a directory. Paths are relative to it, with `/` separators; `""` is the directory itself.
pub trait Overlay: Send + Sync {
    /// The file at `path`; `None` when the overlay has no such file.
    ///
    /// # Errors
    ///
    /// Returns an error when the overlay has the file and cannot read it.
    fn disk_path(&self, _path: &str) -> Option<PathBuf> {
        None
    }

    fn read(&self, path: &str) -> io::Result<Option<Vec<u8>>>;

    /// Whether `path` is a file of the overlay.
    fn is_file(&self, path: &str) -> bool;

    /// Whether `path` is a directory of the overlay (`""` when it has any file).
    fn is_dir(&self, path: &str) -> bool;

    /// The children of the directory `dir`, each with whether it is a directory; empty when it is not one.
    fn children(&self, dir: &str) -> Vec<(String, bool)>;
}

struct Mount {
    shared: bool,
    /// The directory as given, made absolute and normalized ([`normalize`]).
    root: PathBuf,
    /// The directory with its links resolved, when it exists and differs from `root`.
    canonical: Option<PathBuf>,
    overlay: Arc<dyn Overlay>,
}

impl Mount {
    fn roots(&self) -> impl Iterator<Item = &PathBuf> {
        std::iter::once(&self.root).chain(self.canonical.as_ref())
    }
}

static MOUNTS: RwLock<Vec<Mount>> = RwLock::new(Vec::new());

/// Whether anything is mounted, checked before the lock so an instance with nothing mounted pays nothing.
static MOUNTED: AtomicBool = AtomicBool::new(false);

/// Mount `overlay` on the directory `root`, in place of one mounted there before.
pub fn mount(root: &Path, overlay: Arc<dyn Overlay>) {
    mount_layer(root, Some(overlay), false);
}

/// Replace the shared definition layer at this root. Shared mounts precede every embedded mount,
/// including mounts on an ancestor or descendant of this directory.
pub fn mount_shared(root: &Path, overlay: Option<Arc<dyn Overlay>>) {
    mount_layer(root, overlay, true);
}

fn mount_layer(root: &Path, overlay: Option<Arc<dyn Overlay>>, shared: bool) {
    let root = normalize(root);
    let canonical = std::fs::canonicalize(&root)
        .ok()
        .map(|path| normalize(&path))
        .filter(|path| *path != root);
    let mut mounts = MOUNTS.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    mounts.retain(|mount| mount.root != root || mount.shared != shared);
    let Some(overlay) = overlay else { return };
    let index = if shared { 0 } else { mounts.len() };
    mounts.insert(
        index,
        Mount {
            shared,
            root,
            canonical,
            overlay,
        },
    );
    MOUNTED.store(true, Ordering::Release);
}

/// Remove the overlay mounted on `root`; false when there was none.
pub fn unmount(root: &Path) -> bool {
    let root = normalize(root);
    let mut mounts = MOUNTS.write().unwrap_or_else(std::sync::PoisonError::into_inner);
    let before = mounts.len();
    mounts.retain(|mount| mount.root != root);
    MOUNTED.store(!mounts.is_empty(), Ordering::Release);
    mounts.len() != before
}

/// `path` made absolute (from the working directory) with `.` and `..` resolved by name, without touching the disk.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut normal = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other.as_os_str()),
        }
    }
    normal
}

/// `path` relative to `root` with `/` separators, when it is in it.
fn relative(path: &Path, root: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rest
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

/// What the overlays say about `path`: each one mounted on a directory that holds it, with the path relative to that
/// directory, and the names of the directories on the way to a mount below it (a mount on `a/b/c` makes `b` a child
/// directory of `a`).
struct Found {
    within: Vec<(Arc<dyn Overlay>, String)>,
    below: Vec<String>,
}

fn find(path: &Path) -> Option<Found> {
    if !MOUNTED.load(Ordering::Acquire) {
        return None;
    }
    let path = normalize(path);
    let mounts = MOUNTS.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut found = Found {
        within: Vec::new(),
        below: Vec::new(),
    };
    for mount in mounts.iter() {
        if let Some(rel) = mount.roots().find_map(|root| relative(&path, root)) {
            found.within.push((Arc::clone(&mount.overlay), rel));
            continue;
        }
        for root in mount.roots() {
            if let Ok(rest) = root.strip_prefix(&path) {
                if let Some(first) = rest.components().next() {
                    found.below.push(first.as_os_str().to_string_lossy().into_owned());
                    break;
                }
            }
        }
    }
    Some(found)
}

fn overlay_read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    let Some(found) = find(path) else {
        return Ok(None);
    };
    for (overlay, rel) in &found.within {
        if let Some(bytes) = overlay.read(rel)? {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

fn overlay_is_file(path: &Path) -> bool {
    find(path).is_some_and(|found| found.within.iter().any(|(overlay, rel)| overlay.is_file(rel)))
}

fn overlay_is_dir(path: &Path) -> bool {
    find(path)
        .is_some_and(|found| !found.below.is_empty() || found.within.iter().any(|(overlay, rel)| overlay.is_dir(rel)))
}

/// The bytes of the file at `path`, from disk, else from an overlay.
///
/// # Errors
///
/// The `std::fs::read` error when neither has the file, or the overlay's when it cannot read it.
pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    match std::fs::read(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => overlay_read(path)?.ok_or(err),
        other => other,
    }
}

/// The text of the file at `path`, from disk, else from an overlay.
///
/// # Errors
///
/// As [`read`], and `InvalidData` when the file is not UTF-8.
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let bytes = overlay_read(path)?.ok_or(err)?;
            String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
        }
        other => other,
    }
}

/// The file at `path` to read, from disk, else from an overlay.
///
/// # Errors
///
/// As [`read`].
pub fn open(path: impl AsRef<Path>) -> io::Result<Box<dyn Read + Send>> {
    let path = path.as_ref();
    match std::fs::File::open(path) {
        Ok(file) => Ok(Box::new(file)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let bytes = overlay_read(path)?.ok_or(err)?;
            Ok(Box::new(io::Cursor::new(bytes)))
        }
        Err(err) => Err(err),
    }
}

/// Whether `path` is a file or a directory, on disk or in an overlay.
#[must_use]
pub fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    path.exists() || overlay_is_file(path) || overlay_is_dir(path)
}

/// Whether `path` is a file, on disk or in an overlay where the disk has nothing there.
#[must_use]
pub fn is_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file(),
        Err(_) => overlay_is_file(path),
    }
}

/// Whether `path` is a directory, on disk or in an overlay where the disk has nothing there.
#[must_use]
pub fn is_dir(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_dir(),
        Err(_) => overlay_is_dir(path),
    }
}

/// Whether the file at `path` is read from an overlay: an overlay has it and the disk has nothing there.
#[must_use]
pub fn is_from_overlay(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    std::fs::symlink_metadata(path).is_err() && overlay_is_file(path)
}

/// When the file at `path` on disk was last changed; `None` for a file read from an overlay, which does not change
/// while the process runs, and when there is none.
#[must_use]
pub fn modified(path: impl AsRef<Path>) -> Option<SystemTime> {
    let path = path.as_ref();
    let physical = source_file(path)?;
    std::fs::metadata(physical).and_then(|meta| meta.modified()).ok()
}

/// Physical file selected from the local runtime or a directory overlay.
pub fn source_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let found = find(path)?;
    for (overlay, rel) in found.within {
        if overlay.is_file(&rel) {
            return overlay.disk_path(&rel);
        }
    }
    None
}

/// One entry of a directory [`read_dir`] lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    path: PathBuf,
    name: OsString,
    is_dir: bool,
}

impl DirEntry {
    /// The directory joined with the entry's name.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    #[must_use]
    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }

    /// Whether the entry is a directory, following a link on disk.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Whether the entry is not a directory.
    #[must_use]
    pub fn is_file(&self) -> bool {
        !self.is_dir
    }
}

/// The entries of the directory `dir`: the disk's, and the overlays' that the disk does not have, sorted by name. An
/// entry on disk keeps its own kind.
///
/// # Errors
///
/// The `std::fs::read_dir` error when the disk cannot list `dir` and no overlay has it as a directory.
pub fn read_dir(dir: impl AsRef<Path>) -> io::Result<Vec<DirEntry>> {
    let dir = dir.as_ref();
    let mut entries: Vec<DirEntry> = Vec::new();
    let disk_err = match std::fs::read_dir(dir) {
        Ok(listed) => {
            for entry in listed.flatten() {
                let path = entry.path();
                let is_dir = match entry.file_type() {
                    Ok(kind) if kind.is_symlink() => path.is_dir(),
                    Ok(kind) => kind.is_dir(),
                    Err(_) => path.is_dir(),
                };
                entries.push(DirEntry {
                    path,
                    name: entry.file_name(),
                    is_dir,
                });
            }
            None
        }
        Err(err) => Some(err),
    };
    let mut from_overlay = false;
    if let Some(found) = find(dir) {
        let mut add = |name: String, is_dir: bool| {
            let name = OsString::from(name);
            if !entries.iter().any(|entry| entry.name == name) {
                entries.push(DirEntry {
                    path: dir.join(&name),
                    name,
                    is_dir,
                });
            }
        };
        for (overlay, rel) in &found.within {
            if overlay.is_dir(rel) {
                from_overlay = true;
                for (name, is_dir) in overlay.children(rel) {
                    add(name, is_dir);
                }
            }
        }
        for name in found.below {
            from_overlay = true;
            add(name, true);
        }
    }
    if let Some(err) = disk_err {
        if !from_overlay {
            return Err(err);
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[cfg(test)]
#[path = "vfs_tests.rs"]
mod tests;
