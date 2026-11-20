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

//! The archives the runner makes and reads: an artifact or a cache is a tar.gz of files and directories of the
//! workspace, by their paths in it. Packing reads nothing outside the workspace, and unpacking writes nothing outside
//! it.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

/// Write `files` (files and directories under `workspace`) to the tar.gz `out`, each by its path in the workspace
/// ([`pack_into`]).
///
/// # Errors
///
/// When a file is outside the workspace or cannot be read, or `out` cannot be written.
pub fn pack(workspace: &Path, files: &[PathBuf], out: &Path) -> Result<(), String> {
    let file = super::create_new(out)?;
    pack_into(workspace, files, file)
}

/// Write `files` (files and directories under `workspace`) as a tar.gz to `out`, each by its path in the workspace.
/// A named path is packed as what it resolves to, which must be in the workspace: one that is, or goes through, a link
/// out of it is refused. Under a directory a link is stored as a link and never followed, and a regular file is read
/// only with [`super::open_regular`] (`O_NOFOLLOW`); a FIFO, socket or device is left out (SEC-GIT-81).
///
/// # Errors
///
/// When a file is outside the workspace or cannot be read, or `out` cannot be written.
pub fn pack_into<W: Write>(workspace: &Path, files: &[PathBuf], out: W) -> Result<(), String> {
    let root = workspace
        .canonicalize()
        .map_err(|err| format!("{}: {err}", workspace.display()))?;
    let mut builder = tar::Builder::new(GzEncoder::new(out, Compression::fast()));
    builder.follow_symlinks(false);
    for path in files {
        let relative = path
            .strip_prefix(workspace)
            .map_err(|_| format!("{} is outside the workspace", path.display()))?;
        let real = path
            .canonicalize()
            .map_err(|err| format!("{}: {err}", relative.display()))?;
        if !real.starts_with(&root) {
            return Err(format!("{} is outside the workspace", relative.display()));
        }
        append_tree(&mut builder, &real, relative)?;
    }
    builder
        .into_inner()
        .and_then(GzEncoder::finish)
        .map(|_| ())
        .map_err(|err| format!("the archive was not written: {err}"))
}

/// Append `path` as `name`, and under a directory what it holds, by `symlink_metadata`: a directory is walked, a link
/// is stored as its text, a regular file is read through [`super::open_regular`], anything else is left out.
fn append_tree<W: Write>(builder: &mut tar::Builder<W>, path: &Path, name: &Path) -> Result<(), String> {
    let mut stack = vec![(path.to_path_buf(), name.to_path_buf())];
    while let Some((path, name)) = stack.pop() {
        let failed = |err: &dyn std::fmt::Display| format!("{}: {err}", name.display());
        let meta = path.symlink_metadata().map_err(|err| failed(&err))?;
        let kind = meta.file_type();
        if kind.is_dir() {
            if name != Path::new("") {
                let mut header = tar::Header::new_gnu();
                header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                builder
                    .append_data(&mut header, &name, std::io::empty())
                    .map_err(|err| failed(&err))?;
            }
            let mut entries: Vec<_> = std::fs::read_dir(&path)
                .map_err(|err| failed(&err))?
                .filter_map(Result::ok)
                .map(|entry| entry.file_name())
                .collect();
            entries.sort();
            for entry in entries.into_iter().rev() {
                stack.push((path.join(&entry), name.join(&entry)));
            }
        } else if kind.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|err| failed(&err))?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata_in_mode(&meta, tar::HeaderMode::Complete);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            builder
                .append_link(&mut header, &name, &target)
                .map_err(|err| failed(&err))?;
        } else if kind.is_file() {
            let mut file = super::open_regular(&path)?;
            builder.append_file(&name, &mut file).map_err(|err| failed(&err))?;
        }
    }
    Ok(())
}

/// The most bytes an archive unpacks to when the job does not say (`git-ci` `max-unpack-bytes`, sent as the job's
/// `limits.maxUnpackBytes`): 16 GiB, room for a build's `target` cache.
pub const DEFAULT_MAX_UNPACK_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// The most entries an archive unpacks: what `ec.resource.extractArchive` takes by default.
pub const MAX_UNPACK_ENTRIES: usize = 1_000_000;

/// Unpack the tar.gz `archive` into `workspace`, at most [`DEFAULT_MAX_UNPACK_BYTES`] and [`MAX_UNPACK_ENTRIES`]
/// ([`unpack_within`]). The count of entries written.
///
/// # Errors
///
/// When the archive does not read, an entry cannot be written, or the archive passes a limit.
pub fn unpack(archive: &Path, workspace: &Path) -> Result<usize, String> {
    unpack_within(archive, workspace, DEFAULT_MAX_UNPACK_BYTES, MAX_UNPACK_ENTRIES)
}

/// Where `path` resolves on the file system now: its longest prefix that exists, canonicalized (through every link in
/// it, the links earlier entries made included), with the rest put on by name, `..` going up from there. What does not
/// exist yet holds no link. `None` for a path that is not plain (a prefix or root in the rest).
fn resolve(path: &Path) -> Option<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let base = loop {
        if let Ok(canonical) = existing.canonicalize() {
            break canonical;
        }
        let last = existing.components().next_back()?;
        match last {
            std::path::Component::Normal(name) => rest.push(name.to_os_string()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => rest.push("..".into()),
            _ => return None,
        }
        if !existing.pop() {
            return None;
        }
    };
    let mut out = base;
    for part in rest.into_iter().rev() {
        if part == ".." {
            out.pop();
        } else {
            out.push(part);
        }
    }
    Some(out)
}

/// Whether a link's `target` stays inside `root` (the canonical workspace) once resolved from `from`, the directory it
/// is read from: a symbolic link's own directory in the workspace, a hard link's the workspace itself. An absolute
/// target never does. The resolution goes through the links already unpacked, so a chain (`deep -> .`, then
/// `deep/esc -> ..`) is seen for where it leads.
///
/// The target's own components are followed one at a time: one that exists is canonicalized (through a link), and a
/// `..` after one that does not exist yet is refused, since a later entry could make that component a link and move
/// where the `..` lands (`sub/p -> ../q/..`, then `q -> .`).
fn link_inside(root: &Path, from: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let Some(mut at) = resolve(from) else {
        return false;
    };
    let mut missing = false;
    for component in target.components() {
        match component {
            std::path::Component::Normal(name) => {
                let next = at.join(name);
                if !missing {
                    if let Ok(real) = next.canonicalize() {
                        at = real;
                        continue;
                    }
                    missing = true;
                }
                at = next;
            }
            std::path::Component::ParentDir => {
                if missing {
                    return false;
                }
                at.pop();
            }
            std::path::Component::CurDir => {}
            _ => return false,
        }
    }
    at.starts_with(root)
}

/// After an unpack: remove each link it made that now resolves outside `root`, as later entries can move where an
/// earlier link leads. The count removed.
pub(crate) fn remove_links_out(root: &Path, made: &[PathBuf]) -> usize {
    let mut removed = 0;
    for link in made {
        let out = link.canonicalize().is_ok_and(|real| !real.starts_with(root));
        if out {
            match std::fs::remove_file(link) {
                Ok(()) => removed += 1,
                Err(err) => tracing::error!(
                    "a cache's link out of the workspace, {}, was not removed: {err}",
                    link.display()
                ),
            }
        }
    }
    removed
}

/// [`unpack`] with the limits: the entries' bytes (`entry.size()`, counted before each is written) and their count.
/// An entry that is not a regular file, a directory or a link that stays inside the workspace (a device, a FIFO, a
/// sparse file, a link out) is skipped, and one whose path would leave the workspace too, as `tar` refuses it.
/// Past a limit the unpack stops with an error, which the runner logs as a cache not restored; what it wrote stays.
///
/// # Errors
///
/// When the archive does not read, an entry cannot be written, or the archive passes a limit (SEC-GIT-95).
pub fn unpack_within(archive: &Path, workspace: &Path, max_bytes: u64, max_entries: usize) -> Result<usize, String> {
    let file = File::open(archive).map_err(|err| format!("{}: {err}", archive.display()))?;
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    tar.set_overwrite(true);
    let root = workspace
        .canonicalize()
        .map_err(|err| format!("{}: {err}", workspace.display()))?;
    let (mut count, mut seen, mut bytes) = (0, 0usize, 0u64);
    let mut made_links: Vec<PathBuf> = Vec::new();
    for entry in tar
        .entries()
        .map_err(|err| format!("the archive does not read: {err}"))?
    {
        let mut entry = entry.map_err(|err| format!("the archive does not read: {err}"))?;
        seen += 1;
        if seen > max_entries {
            return Err(format!("the archive has more than {max_entries} entries"));
        }
        let kind = entry.header().entry_type();
        let wanted = match kind {
            tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::Directory => true,
            tar::EntryType::Symlink | tar::EntryType::Link => {
                // a hard link names a path from the archive's root, a symbolic one a path from its own directory
                let from = if kind == tar::EntryType::Link {
                    Some(root.clone())
                } else {
                    entry
                        .path()
                        .ok()
                        .map(|path| root.join(path.parent().unwrap_or(Path::new(""))))
                };
                let target = entry.link_name().ok().flatten().map(|target| target.into_owned());
                from.zip(target)
                    .is_some_and(|(from, target)| link_inside(&root, &from, &target))
            }
            _ => false,
        };
        if !wanted {
            continue;
        }
        bytes = bytes.saturating_add(entry.size());
        if bytes > max_bytes {
            return Err(format!("the archive unpacks to more than {max_bytes} bytes"));
        }
        // unpack_in answers false for a path that would leave the directory
        let link = (kind == tar::EntryType::Symlink)
            .then(|| entry.path().ok().map(|path| root.join(path)))
            .flatten();
        if entry
            .unpack_in(workspace)
            .map_err(|err| format!("an entry was not written: {err}"))?
        {
            count += 1;
            made_links.extend(link);
        }
    }
    Ok(count - remove_links_out(&root, &made_links))
}
