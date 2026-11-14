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

//! A tree at a revision as a zip or tar.gz file (`get#GitArchive`; plans/Dogfood.md, N-ARCHIVE): what a "Download"
//! button of a code page serves.
//!
//! The archive of one (repository, commit, path, format) is one file under `runtime/tmp/git-archive`, named by a
//! digest of those, written beside it and renamed into place, so callers asking for the same archive share the file
//! and none of them sees half of one. A file is kept [`KEEP`] from its last use and removed by a later call or at
//! start. What a call costs is bounded, and refused past the bound: the entries and trees read and their depth
//! ([`Limits::max_entries`], [`MAX_DEPTH`]), the bytes of content, the bytes the directory holds, and the archives
//! built at once. The tree is measured before anything is written.
//!
//! A tree is what someone pushed, so its names are not trusted ([`entry_name`]): one that could leave the archive's
//! directory when it is unpacked refuses the archive. A link is kept as a link entry, never followed here, and only
//! when its target is relative and stays inside the archive. Files kept in Git LFS are archived with their content;
//! a submodule is its empty directory.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use gix::objs::tree::EntryKind;
use gix::objs::Kind;
use gix::ObjectId;
use sha2::Digest;

use super::ops;
use super::store::GitStore;
use super::{GitProvider, Repo};
use moqui_core::context::Error;
use moqui_core::resource::source::SourceReader;

/// An archive file is removed this long after its last use.
const KEEP: Duration = Duration::from_secs(6 * 60 * 60);
/// A file still being written is removed after this long: its writer is gone.
const KEEP_PARTIAL: Duration = Duration::from_secs(60 * 60);
/// The deepest directory archived. git has no limit; no tree anyone works in comes near this one.
pub(crate) const MAX_DEPTH: usize = 64;
/// The longest link target kept.
const MAX_LINK_TARGET: u64 = 4096;
/// A file over this is read from its store a window at a time when the store can (a content blob).
const STREAM_OVER: u64 = 8 * 1024 * 1024;

/// `git-server` `archive-max-bytes` when the conf has none: 256 MiB of content.
pub(crate) const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// `git-server` `archive-max-files` when the conf has none.
pub(crate) const DEFAULT_MAX_ENTRIES: usize = 50_000;
/// `git-server` `archive-max-disk-bytes` when the conf has none: 2 GiB of archive files.
pub(crate) const DEFAULT_MAX_DISK_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// `git-server` `archive-max-concurrent` when the conf has none.
pub(crate) const DEFAULT_MAX_CONCURRENT: usize = 2;

/// The format of an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Zip,
    TarGz,
}

impl Format {
    /// `zip` (also when empty), or `tar.gz` (`tgz`).
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "zip" => Some(Self::Zip),
            "tar.gz" | "tgz" => Some(Self::TarGz),
            _ => None,
        }
    }

    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }

    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Self::Zip => "application/zip",
            Self::TarGz => "application/gzip",
        }
    }
}

/// What an archive may hold and cost.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// The most bytes of content, before compression (`git-server` `archive-max-bytes`).
    pub max_bytes: u64,
    /// The most entries: files, links, directories, and trees read (`git-server` `archive-max-files`).
    pub max_entries: usize,
    /// The most bytes of archive files kept at once (`git-server` `archive-max-disk-bytes`).
    pub max_disk_bytes: u64,
    /// The most archives measured and written at once (`git-server` `archive-max-concurrent`).
    pub max_concurrent: usize,
}

/// An archive, written now or before.
#[derive(Clone, Debug)]
pub(crate) struct Archived {
    pub path: PathBuf,
    pub commit: ObjectId,
    pub files: usize,
    /// The bytes of the files, before compression.
    pub content_bytes: u64,
    /// The size of the archive file.
    pub size: u64,
    /// The links left out: their targets are absolute or leave the archive.
    pub skipped: Vec<String>,
}

/// What one entry of the archive is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum What {
    File {
        executable: bool,
    },
    /// A link to this target, relative and inside the archive.
    Link(String),
    /// A submodule: its directory, empty.
    Directory,
}

#[derive(Clone, Debug)]
struct Entry {
    path: String,
    what: What,
    id: ObjectId,
}

/// The entries of a tree, measured.
#[derive(Debug, Default)]
struct Walked {
    entries: Vec<Entry>,
    content_bytes: u64,
    skipped: Vec<String>,
}

fn io_error(what: &str, path: &Path, err: impl std::fmt::Display) -> Error {
    Error::new(format!("{what} {}: {err}", path.display()))
}

/// A tree entry's name as an archive may hold it: UTF-8, not empty, not `.`, `..` or `.git` (in any case, as a file
/// system that ignores case reads it), and without `/`, `\` or NUL. git's own clients refuse to make such entries; a
/// pushed tree is whatever was pushed.
fn entry_name(raw: &[u8]) -> Option<&str> {
    let name = std::str::from_utf8(raw).ok()?;
    let reserved = name == "." || name == ".." || name.eq_ignore_ascii_case(".git");
    let unsafe_character = name.contains(['/', '\\', '\0']);
    (!name.is_empty() && !reserved && !unsafe_character).then_some(name)
}

/// Whether the link target `target`, of a link `depth` directories below the archive's top, names something inside
/// the archive: relative, with no `\` or NUL, and never climbing above the top by its `..` segments.
fn link_stays_inside(target: &str, depth: usize) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains(['\\', '\0']) {
        return false;
    }
    let mut depth = depth;
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => match depth.checked_sub(1) {
                Some(above) => depth = above,
                None => return false,
            },
            _ => depth += 1,
        }
    }
    true
}

/// The entries under `tree`, each directory's in git's order, measured by `size_of` (a file's content: an LFS
/// file's is its content's, not its pointer's). No recursion: a tree is as deep as whoever pushed it made it.
///
/// # Errors
///
/// A name an archive cannot hold or a name twice in one directory; and past the limits: more than
/// `limits.max_entries` entries and trees (a tree counts each time it is read, so one named by many entries is not
/// read without end), more than `limits.max_bytes` of content, a directory deeper than [`MAX_DEPTH`].
fn walk(
    store: &dyn GitStore,
    size_of: &dyn Fn(&ObjectId) -> Result<u64, Error>,
    tree: &ObjectId,
    limits: Limits,
) -> Result<Walked, Error> {
    let mut walked = Walked::default();
    let mut counted = 0usize;
    let count = |counted: &mut usize| -> Result<(), Error> {
        *counted += 1;
        if *counted > limits.max_entries {
            return Err(Error::new(format!(
                "the archive would hold more than {} entries",
                limits.max_entries
            )));
        }
        Ok(())
    };
    // (tree, the path of its directory with a trailing slash, how deep it is)
    let mut pending: Vec<(ObjectId, String, usize)> = vec![(*tree, String::new(), 0)];
    while let Some((tree, prefix, depth)) = pending.pop() {
        count(&mut counted)?;
        let entries = ops::read_tree(store, &tree)?.entries;
        let mut names: HashSet<&str> = HashSet::with_capacity(entries.len());
        let mut below = Vec::new();
        for entry in &entries {
            let Some(name) = entry_name(&entry.filename) else {
                return Err(Error::new(format!(
                    "the tree has an entry an archive cannot hold: {prefix}{}",
                    String::from_utf8_lossy(&entry.filename).escape_default()
                )));
            };
            if !names.insert(name) {
                return Err(Error::new(format!("the tree names {prefix}{name} more than once")));
            }
            let path = format!("{prefix}{name}");
            let what = match entry.mode.kind() {
                EntryKind::Tree => {
                    if depth + 1 > MAX_DEPTH {
                        return Err(Error::new(format!(
                            "the tree is deeper than {MAX_DEPTH} directories at {path}"
                        )));
                    }
                    below.push((entry.oid, format!("{path}/"), depth + 1));
                    continue;
                }
                EntryKind::Blob => What::File { executable: false },
                EntryKind::BlobExecutable => What::File { executable: true },
                EntryKind::Commit => What::Directory,
                EntryKind::Link => {
                    let short = store
                        .object_size(&entry.oid)?
                        .is_some_and(|size| size <= MAX_LINK_TARGET);
                    let target = if short {
                        String::from_utf8(ops::object(store, &entry.oid, Kind::Blob)?).ok()
                    } else {
                        None
                    };
                    match target.filter(|target| link_stays_inside(target, depth)) {
                        Some(target) => What::Link(target),
                        None => {
                            count(&mut counted)?;
                            walked.skipped.push(path);
                            continue;
                        }
                    }
                }
            };
            count(&mut counted)?;
            if matches!(what, What::File { .. }) {
                walked.content_bytes = walked.content_bytes.saturating_add(size_of(&entry.oid)?);
                if walked.content_bytes > limits.max_bytes {
                    return Err(Error::new(format!(
                        "the archive would hold more than {} bytes",
                        limits.max_bytes
                    )));
                }
            }
            walked.entries.push(Entry {
                path,
                what,
                id: entry.oid,
            });
        }
        // the last pushed is read next: the directories in their order
        pending.extend(below.into_iter().rev());
    }
    Ok(walked)
}

/// When `path` was last used: its modified time, which a reuse sets.
fn last_used(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

/// Remove the archive files of `dir` last used more than [`KEEP`] ago, and the partly written ones older than
/// [`KEEP_PARTIAL`]. One that cannot be removed is logged, and the rest still go. The bytes of the files left.
fn sweep(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut left = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let keep = if path.extension().is_some_and(|extension| extension == "tmp") {
            KEEP_PARTIAL
        } else {
            KEEP
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > keep);
        if !old {
            left = left.saturating_add(meta.len());
            continue;
        }
        if let Err(err) = std::fs::remove_file(&path) {
            tracing::error!("cannot remove the old git archive {}: {err}", path.display());
            left = left.saturating_add(meta.len());
        }
    }
    left
}

/// The directory the archives of a runtime are kept in.
fn archive_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tmp").join("git-archive")
}

/// At start: remove the archives of `runtime_dir` nobody used for [`KEEP`], and what a writer that died left.
pub(crate) fn sweep_at_start(runtime_dir: &Path) {
    sweep(&archive_dir(runtime_dir));
}

/// One of the archives built at once; given back when dropped.
struct BuildSlot<'a>(&'a AtomicUsize);

impl<'a> BuildSlot<'a> {
    fn take(building: &'a AtomicUsize, most: usize) -> Option<Self> {
        if building.fetch_add(1, Ordering::AcqRel) >= most.max(1) {
            building.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self(building))
    }
}

impl Drop for BuildSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl GitProvider {
    /// The content of a file entry to read, and its size: an LFS file's content and a large object's content blob a
    /// window at a time, anything else whole.
    fn archive_content(&self, repo: &Repo, id: &ObjectId) -> Result<(u64, Box<dyn Read>), Error> {
        if let Some(info) = self.lfs_content(repo, id)? {
            return Ok((info.size, Box::new(SourceReader::new(self.lfs_source(&info)?))));
        }
        let store = repo.store.as_ref();
        if store.object_size(id)?.is_some_and(|size| size > STREAM_OVER) {
            if let Some(source) = store.content_source(id)? {
                return Ok((source.len(), Box::new(SourceReader::new(source))));
            }
        }
        let data = ops::object(store, id, Kind::Blob)?;
        Ok((data.len() as u64, Box::new(std::io::Cursor::new(data))))
    }

    /// The archive of the tree at `path` (the whole tree for the empty path) of the commit `revision` names, its
    /// entries under `<top>/`: the file of an earlier call for the same archive, or one written now. `None` when the
    /// revision names nothing or the path is not a directory there.
    ///
    /// # Errors
    ///
    /// A tree [`walk`] refuses; as many archives being built as `limits.max_concurrent`; an archive directory that
    /// would pass `limits.max_disk_bytes`; and a file that cannot be written.
    pub(crate) fn archive(
        &self,
        repository_id: &str,
        revision: &str,
        path: &[String],
        top: &str,
        format: Format,
        limits: Limits,
    ) -> Result<Option<Archived>, Error> {
        let repo = self.repo(repository_id)?;
        let store = repo.store.as_ref();
        let Some(commit_id) = self.resolve_commit(repository_id, revision)? else {
            return Ok(None);
        };
        let commit = ops::read_commit(store, &commit_id)?;
        let Some((_, tree)) = ops::resolve(store, &commit.tree, path)?.filter(|(mode, _)| mode.is_tree()) else {
            return Ok(None);
        };
        let Some(_slot) = BuildSlot::take(&self.inner.archives_building, limits.max_concurrent) else {
            return Err(Error::new("other archives are being made now; ask again in a moment"));
        };
        // measured every time: the limits are the conf's of now, and the counts are part of the answer
        let walked = walk(store, &|id| self.file_size(&repo, id), &tree, limits)?;
        let files = walked
            .entries
            .iter()
            .filter(|entry| entry.what != What::Directory)
            .count();

        let dir = archive_dir(&self.inner.runtime_dir);
        std::fs::create_dir_all(&dir).map_err(|err| io_error("cannot create", &dir, err))?;
        let held = sweep(&dir);
        let key = sha2::Sha256::digest(
            format!(
                "1\0{repository_id}\0{commit_id}\0{}\0{top}\0{}",
                path.join("/"),
                format.extension()
            )
            .as_bytes(),
        );
        let key: String = key.iter().take(20).map(|byte| format!("{byte:02x}")).collect();
        let target = dir.join(format!("{key}.{}", format.extension()));
        let answer = |size: u64| Archived {
            path: target.clone(),
            commit: commit_id,
            files,
            content_bytes: walked.content_bytes,
            size,
            skipped: walked.skipped.clone(),
        };
        if let Ok(meta) = std::fs::metadata(&target) {
            // in use again: it is kept from now
            let touched = std::fs::File::options()
                .write(true)
                .open(&target)
                .and_then(|file| file.set_modified(SystemTime::now()));
            if let Err(err) = touched {
                tracing::warn!("the git archive {} was not marked as used: {err}", target.display());
            }
            return Ok(Some(answer(meta.len())));
        }
        // an archive is at most about as large as its content
        if held.saturating_add(walked.content_bytes) > limits.max_disk_bytes {
            return Err(Error::new(format!(
                "the archives kept already take {held} of {} bytes; ask again later",
                limits.max_disk_bytes
            )));
        }
        let mut random = [0u8; 8];
        getrandom::fill(&mut random)
            .map_err(|err| Error::new(format!("no random bytes for an archive name: {err}")))?;
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let partial = dir.join(format!("{key}.{suffix}.tmp"));
        let written = (|| {
            let file = std::fs::File::create(&partial).map_err(|err| io_error("cannot create", &partial, err))?;
            let out = std::io::BufWriter::new(file);
            let mut out = match format {
                Format::Zip => self.write_zip(&repo, out, top, &walked.entries, commit.seconds),
                Format::TarGz => self.write_tar_gz(&repo, out, top, &walked.entries, commit.seconds),
            }?;
            out.flush().map_err(|err| io_error("cannot write", &partial, err))?;
            drop(out);
            // whole, then in place: another caller writing the same archive puts the same bytes there
            std::fs::rename(&partial, &target).map_err(|err| io_error("cannot keep", &target, err))
        })();
        if let Err(err) = written {
            // a partly written archive is of no use to anyone
            let _ = std::fs::remove_file(&partial);
            return Err(err);
        }
        let size = last_used(&target)
            .and_then(|_| std::fs::metadata(&target).ok())
            .map_or(0, |meta| meta.len());
        Ok(Some(answer(size)))
    }

    fn write_zip<W: Write + std::io::Seek>(
        &self,
        repo: &Repo,
        out: W,
        top: &str,
        entries: &[Entry],
        seconds: i64,
    ) -> Result<W, Error> {
        let failed = |path: &str, err: &dyn std::fmt::Display| Error::new(format!("cannot archive {path}: {err}"));
        let stamp = zip_time(seconds);
        let options = |mode: u32, size: u64| {
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(stamp)
                .unix_permissions(mode)
                .large_file(size >= u64::from(u32::MAX))
        };
        let mut zip = zip::ZipWriter::new(out);
        for entry in entries {
            let name = format!("{top}/{}", entry.path);
            match &entry.what {
                What::Directory => zip
                    .add_directory(name.as_str(), options(0o755, 0))
                    .map_err(|err| failed(&entry.path, &err))?,
                What::Link(target) => zip
                    .add_symlink(name.as_str(), target.as_str(), options(0o777, 0))
                    .map_err(|err| failed(&entry.path, &err))?,
                What::File { executable } => {
                    let (size, mut content) = self.archive_content(repo, &entry.id)?;
                    let mode = if *executable { 0o755 } else { 0o644 };
                    zip.start_file(name.as_str(), options(mode, size))
                        .map_err(|err| failed(&entry.path, &err))?;
                    std::io::copy(&mut content, &mut zip).map_err(|err| failed(&entry.path, &err))?;
                }
            }
        }
        zip.finish().map_err(|err| failed(top, &err))
    }

    fn write_tar_gz<W: Write>(
        &self,
        repo: &Repo,
        out: W,
        top: &str,
        entries: &[Entry],
        seconds: i64,
    ) -> Result<W, Error> {
        let failed = |path: &str, err: &dyn std::fmt::Display| Error::new(format!("cannot archive {path}: {err}"));
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(out, flate2::Compression::default()));
        let header = |kind: tar::EntryType, mode: u32, size: u64| {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_mode(mode);
            header.set_size(size);
            header.set_mtime(u64::try_from(seconds).unwrap_or(0));
            header
        };
        for entry in entries {
            let name = format!("{top}/{}", entry.path);
            match &entry.what {
                What::Directory => {
                    let mut header = header(tar::EntryType::Directory, 0o755, 0);
                    tar.append_data(&mut header, format!("{name}/"), std::io::empty())
                        .map_err(|err| failed(&entry.path, &err))?;
                }
                What::Link(target) => {
                    let mut header = header(tar::EntryType::Symlink, 0o777, 0);
                    tar.append_link(&mut header, &name, target)
                        .map_err(|err| failed(&entry.path, &err))?;
                }
                What::File { executable } => {
                    let (size, content) = self.archive_content(repo, &entry.id)?;
                    let mode = if *executable { 0o755 } else { 0o644 };
                    let mut header = header(tar::EntryType::Regular, mode, size);
                    // exactly the size the header says, whatever the source gives
                    tar.append_data(&mut header, &name, content.take(size))
                        .map_err(|err| failed(&entry.path, &err))?;
                }
            }
        }
        let encoder = tar.into_inner().map_err(|err| failed(top, &err))?;
        encoder.finish().map_err(|err| failed(top, &err))
    }
}

/// A commit's time as a zip entry's (UTC); the zip epoch for a time zip cannot hold.
fn zip_time(seconds: i64) -> zip::DateTime {
    let Ok(stamp) = jiff::Timestamp::from_second(seconds) else {
        return zip::DateTime::default();
    };
    let at = stamp.to_zoned(jiff::tz::TimeZone::UTC).datetime();
    (|| {
        zip::DateTime::from_date_and_time(
            u16::try_from(at.year()).ok()?,
            u8::try_from(at.month()).ok()?,
            u8::try_from(at.day()).ok()?,
            u8::try_from(at.hour()).ok()?,
            u8::try_from(at.minute()).ok()?,
            u8::try_from(at.second()).ok()?,
        )
        .ok()
    })()
    .unwrap_or_default()
}

#[cfg(test)]
#[path = "archive_tests.rs"]
mod tests;
