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

//! The framework and tools components' runtime files and a webroot stub, as this binary was built with them.
//!
//! The binary's code depends on these files (entity and service definitions, the default conf, seed data, templates,
//! the REST, login and apps screens). They may be changed in a runtime, which is intended, but a change that no longer
//! fits the binary fails in odd ways, so the host [`compare`]s the files on disk with these copies at startup and logs
//! each difference, then goes on with the files on disk. With no runtime at all it writes them out as a skeleton runtime
//! ([`write_skeleton`]), which boots, loads the seed data, answers `/rest`, `/Login`, `/status` and the html `/apps`
//! (System, Tools and Dashboards among them), and runs the terminal client.
//!
//! Four sets are packed by build.rs: [`Set::Framework`] and [`Set::Tools`], those components less their `test/` (with
//! the acceptance suite in `test/acceptance/`); [`Set::Webroot`], the webroot files `webroot-files.txt` names, a subset of the real ones (no fonts,
//! icons, `qapps` or full JavaScript libraries); and [`Set::Skeleton`], files only a new skeleton gets and nothing
//! compares (the placeholder page at `/`, which the web client's `qapps.xml` replaces).
//!
//! This crate has no framework dependency, so an edit to a framework file rebuilds it alone and relinks the binary.

use std::collections::BTreeSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The files, as build.rs packed them.
static ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded.tar.gz"));

/// The top-level directories of a whole component ([`Set::is_whole_component`]) that are not packed and not compared:
/// what only the tests read, the acceptance suite (`test/acceptance/`) among them.
pub const COMPONENT_SKIP: [&str; 1] = ["test"];

/// The framework files the binary has compiled in with `include_str!` and never reads from disk: an edit to one on
/// disk changes nothing (`crates/moqui-core/src/script/actions.rs` and `screen/generate.rs`, `screen/statics.rs`).
pub const COMPILED_IN: [&str; 4] = [
    "template/XmlActions.js",
    "template/TemplateSupport.js",
    "template/screen/ScreenSupport.js",
    "template/screen/ScreenQvtStatic.js",
];

/// Which of the packed sets a file belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Set {
    /// The framework component, relative to its directory.
    Framework,
    /// The tools component (System, Tools, Dashboards and the `moqui` REST API), relative to its directory.
    Tools,
    /// The webroot stub, relative to the webroot component's directory.
    Webroot,
    /// Files only a skeleton runtime gets, relative to the runtime directory: the sets' manifest
    /// (`components/components.xml`) and the placeholder page; never compared.
    Skeleton,
}

impl Set {
    /// The sets that are components, compared with a runtime's.
    pub const COMPONENTS: [Set; 3] = [Set::Framework, Set::Tools, Set::Webroot];

    fn prefix(self) -> &'static str {
        match self {
            Self::Framework => "framework/",
            Self::Tools => "tools/",
            Self::Webroot => "webroot/",
            Self::Skeleton => "skeleton/",
        }
    }

    /// The component name, or `skeleton`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Framework => "framework",
            Self::Tools => "tools",
            Self::Webroot => "webroot",
            Self::Skeleton => "skeleton",
        }
    }

    /// The set named `name` ([`Set::name`]).
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        [Self::Framework, Self::Tools, Self::Webroot, Self::Skeleton]
            .into_iter()
            .find(|set| set.name() == name)
    }

    /// Whether the set is a whole component less its [`COMPONENT_SKIP`] directories, so a file on disk that the binary
    /// does not have is a difference ([`Kind::Added`]); the webroot set is a subset.
    #[must_use]
    pub fn is_whole_component(self) -> bool {
        matches!(self, Self::Framework | Self::Tools)
    }
}

/// One packed file.
#[derive(Clone, Debug)]
pub struct Entry {
    pub set: Set,
    /// The path in its set, with `/` separators.
    pub path: String,
    pub bytes: Vec<u8>,
}

/// How a file on disk differs from the packed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The contents differ; the first line (from 1) that differs.
    Changed { first_line: usize },
    /// The packed file is not on disk.
    Missing,
    /// On disk and not packed (a whole component's set only; the webroot stub is a subset).
    Added,
    /// The file on disk could not be read.
    Unreadable(String),
}

/// One difference [`compare`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difference {
    pub set: Set,
    /// The path in its set, with `/` separators.
    pub path: String,
    pub kind: Kind,
}

impl Difference {
    /// Whether the binary uses its compiled-in copy of this file whatever is on disk ([`COMPILED_IN`]).
    #[must_use]
    pub fn is_compiled_in(&self) -> bool {
        is_compiled_in(self.set, &self.path)
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { set, path, kind } = self;
        let set = set.name();
        match kind {
            Kind::Changed { first_line } => write!(f, "{set} {path}: changed, first difference at line {first_line}")?,
            Kind::Missing => write!(f, "{set} {path}: missing on disk")?,
            Kind::Added => write!(f, "{set} {path}: added on disk, not in the binary")?,
            Kind::Unreadable(error) => write!(f, "{set} {path}: could not be read: {error}")?,
        }
        if self.is_compiled_in() {
            write!(f, " (ignored: the binary uses its built-in copy)")?;
        }
        Ok(())
    }
}

/// Whether `path` of `set` is one the binary has compiled in and never reads from disk.
#[must_use]
pub fn is_compiled_in(set: Set, path: &str) -> bool {
    set == Set::Framework && COMPILED_IN.contains(&path)
}

/// Every packed file, unpacked. Each call unpacks the archive again; nothing is kept.
///
/// # Errors
///
/// Returns an error when the archive cannot be read, which only a damaged binary would cause.
pub fn entries() -> io::Result<Vec<Entry>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(ARCHIVE));
    let mut entries = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry.path()?.to_string_lossy().replace('\\', "/");
        let Some((set, path)) = [Set::Framework, Set::Tools, Set::Webroot, Set::Skeleton]
            .into_iter()
            .find_map(|set| name.strip_prefix(set.prefix()).map(|path| (set, path.to_string())))
        else {
            continue;
        };
        let mut bytes = Vec::with_capacity(usize::try_from(entry.size()).unwrap_or(0));
        entry.read_to_end(&mut bytes)?;
        entries.push(Entry { set, path, bytes });
    }
    Ok(entries)
}

/// The sets' manifest in the Skeleton set, written into a runtime that has none.
const SETS_MANIFEST: &str = "components/components.xml";

/// Write a skeleton runtime into `runtime_dir`: the framework set into `components/base/framework`; the tools set into
/// `components/base/tools` when that is not there; the webroot set and the placeholder page into
/// `components/base/webroot` when that is not there; and the sets' manifest, `components/components.xml`, when it is not
/// there. A file already there is left as it is. Returns the files written.
///
/// # Errors
///
/// Returns an error naming the path when a directory or file cannot be written, or when the archive cannot be read.
pub fn write_skeleton(runtime_dir: &Path) -> io::Result<Vec<PathBuf>> {
    // the base set of moqui_core::context::layout, which this crate does not depend on
    let base = runtime_dir.join("components/base");
    let with_tools = !base.join("tools").exists();
    let with_webroot = !base.join("webroot").exists();
    let mut written = Vec::new();
    for entry in entries()? {
        let target = match entry.set {
            Set::Framework => base.join("framework").join(&entry.path),
            Set::Tools if with_tools => base.join("tools").join(&entry.path),
            Set::Webroot if with_webroot => base.join("webroot").join(&entry.path),
            Set::Skeleton if with_webroot || entry.path == SETS_MANIFEST => runtime_dir.join(&entry.path),
            Set::Tools | Set::Webroot | Set::Skeleton => continue,
        };
        if target.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| named(err, "could not create", parent))?;
        }
        std::fs::write(&target, &entry.bytes).map_err(|err| named(err, "could not write", &target))?;
        written.push(target);
    }
    Ok(written)
}

/// Compare each component in `dirs` (a set and the directory of that component in the runtime) with the packed files;
/// a set not named is not compared. The result is sorted by set and path; empty when all match. A file that cannot be
/// read is a [`Kind::Unreadable`] difference, and the rest are still compared.
///
/// # Errors
///
/// Returns an error only when the archive cannot be read.
pub fn compare(dirs: &[(Set, &Path)]) -> io::Result<Vec<Difference>> {
    let dir_of = |set: Set| dirs.iter().find(|(named, _)| *named == set).map(|(_, dir)| *dir);
    let mut differences = Vec::new();
    let mut packed = BTreeSet::new();
    for entry in entries()? {
        if entry.set == Set::Skeleton {
            continue;
        }
        let Some(dir) = dir_of(entry.set) else {
            continue;
        };
        packed.insert((entry.set, entry.path.clone()));
        let kind = match std::fs::read(dir.join(&entry.path)) {
            Ok(bytes) if bytes == entry.bytes => continue,
            Ok(bytes) => Kind::Changed {
                first_line: first_differing_line(&entry.bytes, &bytes),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => Kind::Missing,
            Err(err) => Kind::Unreadable(err.to_string()),
        };
        differences.push(Difference {
            set: entry.set,
            path: entry.path,
            kind,
        });
    }
    for (set, dir) in dirs.iter().filter(|(set, _)| set.is_whole_component()) {
        let mut on_disk = Vec::new();
        disk_files(*set, dir, "", &mut on_disk, &mut differences);
        for path in on_disk {
            if !packed.contains(&(*set, path.clone())) {
                differences.push(Difference {
                    set: *set,
                    path,
                    kind: Kind::Added,
                });
            }
        }
    }
    differences.sort_by(|a, b| (a.set, &a.path).cmp(&(b.set, &b.path)));
    Ok(differences)
}

/// A unified diff of the packed `path` of `set` (the old side) against the file under `dir` (the new side): `None`
/// when they are the same, a one-line note for a binary file.
///
/// # Errors
///
/// Returns an error when `path` is not packed in `set`, or the archive cannot be read. A file not on disk diffs
/// against nothing.
pub fn unified_diff(set: Set, path: &str, dir: &Path) -> io::Result<Option<String>> {
    let entry = entries()?
        .into_iter()
        .find(|entry| entry.set == set && entry.path == path)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} has no file {path}", set.name())))?;
    let disk = match std::fs::read(dir.join(path)) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(named(err, "could not read", &dir.join(path))),
    };
    if disk == entry.bytes {
        return Ok(None);
    }
    let (Ok(old), Ok(new)) = (std::str::from_utf8(&entry.bytes), std::str::from_utf8(&disk)) else {
        return Ok(Some(format!(
            "Binary files {0}/{path} (built in) and {0}/{path} (on disk) differ\n",
            set.name()
        )));
    };
    let diff = similar::TextDiff::from_lines(old, new);
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header(
            &format!("{}/{path} (built in)", set.name()),
            &format!("{}/{path} (on disk)", set.name()),
        )
        .to_string();
    Ok(Some(text))
}

/// The first line, from 1, where `a` and `b` differ.
fn first_differing_line(a: &[u8], b: &[u8]) -> usize {
    let common = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    a[..common].iter().filter(|byte| **byte == b'\n').count() + 1
}

/// The files under `dir`, as `/`-separated paths from the component directory of `set`, less dot files and, at the top,
/// the [`COMPONENT_SKIP`] directories. A directory that cannot be read is an [`Kind::Unreadable`] difference.
fn disk_files(set: Set, dir: &Path, prefix: &str, files: &mut Vec<String>, differences: &mut Vec<Difference>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            differences.push(Difference {
                set,
                path: if prefix.is_empty() {
                    ".".to_string()
                } else {
                    prefix.trim_end_matches('/').to_string()
                },
                kind: Kind::Unreadable(err.to_string()),
            });
            return;
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || (prefix.is_empty() && COMPONENT_SKIP.contains(&name.as_str())) {
            continue;
        }
        let path = format!("{prefix}{name}");
        let is_dir = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
        if is_dir {
            disk_files(set, &entry.path(), &format!("{path}/"), files, differences);
        } else {
            files.push(path);
        }
    }
}

fn named(err: io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(err.kind(), format!("{what} {}: {err}", path.display()))
}

#[cfg(test)]
mod tests;
