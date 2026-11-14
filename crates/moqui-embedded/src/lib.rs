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

//! The runtime files this binary was built with: the framework and tools components, a webroot stub and the runtime's
//! confs, and at the production and all [`Level`]s the whole webroot and the other components too.
//!
//! The binary's code depends on these files (entity and service definitions, the default conf, seed data, templates,
//! the REST, login and apps screens). A runtime reads each one that is not on disk from here (the host mounts them
//! with `moqui_lib::vfs`), so a runtime of empty directories boots, loads the seed data, answers `/rest`, `/Login`,
//! `/status` and the html `/apps` (System, Tools and Dashboards among them), and runs the terminal client; a file on
//! disk is used in place of the built-in one. A changed file that no longer fits the binary fails in odd ways, so the
//! host [`compare`]s the files on disk with these copies at startup and logs each difference. [`write_runtime`] writes
//! the built-in files that are not on disk (`moqui embedded extract`).
//!
//! Six sets are packed by build.rs ([`select`] has what each level takes): [`Set::Framework`] and [`Set::Tools`],
//! those components less their [`Level::component_skip`] directories; [`Set::Webroot`], at the framework level the
//! webroot files `webroot-files.txt` names, a subset of the real ones (no fonts, icons, `qapps` or full JavaScript
//! libraries), and above it the whole component; [`Set::Components`], every other component of the level's sets
//! (none at the framework level); [`Set::Skeleton`], files nothing compares (the sets' manifest and, at the framework
//! level, the placeholder page at `/`, which the web client's `qapps.xml` replaces); and [`Set::Conf`], the runtime's
//! `conf/` directory of example and starter confs. Each file is deflated on its own, so reading one does not unpack
//! the rest.
//!
//! This crate has no framework dependency, so an edit to a framework file rebuilds it alone and relinks the binary.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub mod select;

pub use select::Level;

/// The files, each deflated on its own, as build.rs packed them.
static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded.bin"));

/// One line per packed file: `name<TAB>offset<TAB>packed length<TAB>size`, sorted by name.
static INDEX: &str = include_str!(concat!(env!("OUT_DIR"), "/embedded.idx"));

/// The level this binary was built at (the features `production` and `all`).
pub const LEVEL: Level = Level::from_features(cfg!(feature = "production"), cfg!(feature = "all"));

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
    /// The webroot component, the stub or whole ([`Level::whole_webroot`]), relative to its directory.
    Webroot,
    /// The other components of the level's sets, relative to the runtime's `components` directory
    /// (`addon/mr-mantle/component.xml`); empty at the framework level.
    Components,
    /// Files only a skeleton runtime gets, relative to the runtime directory: the sets' manifest
    /// (`components/components.xml`) and the placeholder page; never compared.
    Skeleton,
    /// The runtime's example and starter confs, relative to its `conf` directory; never compared.
    Conf,
}

impl Set {
    /// Every set.
    pub const ALL: [Set; 6] = [
        Set::Framework,
        Set::Tools,
        Set::Webroot,
        Set::Components,
        Set::Skeleton,
        Set::Conf,
    ];

    /// The sets that are components, compared with a runtime's.
    pub const COMPONENTS: [Set; 4] = [Set::Framework, Set::Tools, Set::Webroot, Set::Components];

    /// The sets of the framework level's components: a difference in one of them is a warning at startup, where one in
    /// [`Set::Components`] is information.
    pub const FRAMEWORK: [Set; 3] = [Set::Framework, Set::Tools, Set::Webroot];

    fn prefix(self) -> &'static str {
        match self {
            Self::Framework => "framework/",
            Self::Tools => "tools/",
            Self::Webroot => "webroot/",
            Self::Components => "components/",
            Self::Skeleton => "skeleton/",
            Self::Conf => "conf/",
        }
    }

    /// The component name, or `components`, `skeleton` or `conf`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Framework => "framework",
            Self::Tools => "tools",
            Self::Webroot => "webroot",
            Self::Components => "components",
            Self::Skeleton => "skeleton",
            Self::Conf => "conf",
        }
    }

    /// The set named `name` ([`Set::name`]).
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|set| set.name() == name)
    }

    /// Whether the set is whole components less their [`Level::component_skip`] directories at this binary's level, so
    /// a file on disk that the binary does not have is a difference ([`Kind::Added`]); the framework level's webroot
    /// set is a subset.
    #[must_use]
    pub fn is_whole_component(self) -> bool {
        self.is_whole_at(LEVEL)
    }

    fn is_whole_at(self, level: Level) -> bool {
        match self {
            Self::Framework | Self::Tools | Self::Components => true,
            Self::Webroot => level.whole_webroot(),
            Self::Skeleton | Self::Conf => false,
        }
    }

    /// The directory of a runtime that the set's paths are relative to: the framework component is `framework_dir`
    /// (`components/base/framework` unless `MOQUI_FRAMEWORK` names another), tools and webroot are in the runtime's
    /// base set, the other components in its `components` directory, the skeleton files are relative to the runtime and
    /// the confs to its `conf` directory.
    #[must_use]
    pub fn dir_in(self, runtime_dir: &Path, framework_dir: &Path) -> PathBuf {
        // the base set of moqui_core::context::layout, which this crate does not depend on
        let components = runtime_dir.join("components");
        match self {
            Self::Framework => framework_dir.to_path_buf(),
            Self::Tools => components.join("base/tools"),
            Self::Webroot => components.join("base/webroot"),
            Self::Components => components,
            Self::Skeleton => runtime_dir.to_path_buf(),
            Self::Conf => runtime_dir.join("conf"),
        }
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
    /// The packed file is not on disk, so the runtime reads the built-in one.
    Missing,
    /// On disk and not packed, in a component the binary has (a whole component's set only; the framework level's
    /// webroot stub is a subset).
    Added,
    /// A component on disk, in a set the level packs, that the binary does not have ([`Set::Components`] only; the
    /// path is `<set>/<component>`).
    NotBuiltIn,
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
            Kind::Missing => write!(f, "{set} {path}: not on disk, the built-in one is used")?,
            Kind::Added => write!(f, "{set} {path}: added on disk, not in the binary")?,
            Kind::NotBuiltIn => write!(f, "{set} {path}: a component on disk that is not built into the binary")?,
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

/// Where one packed file is in the blob.
#[derive(Clone, Copy, Debug)]
struct Packed {
    offset: usize,
    len: usize,
    size: usize,
}

/// The packed files and the directories they make, by set.
#[derive(Default)]
struct Index {
    files: BTreeMap<(Set, &'static str), Packed>,
    /// Each directory of a set (`""` for the set's own) with its children: name, and whether it is a directory.
    dirs: BTreeMap<(Set, String), BTreeMap<String, bool>>,
}

impl Index {
    fn parse(text: &'static str) -> Self {
        let mut index = Index::default();
        for line in text.lines() {
            let mut fields = line.split('\t');
            let (Some(name), Some(offset), Some(len), Some(size)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let (Ok(offset), Ok(len), Ok(size)) = (offset.parse(), len.parse(), size.parse()) else {
                continue;
            };
            let Some((set, path)) = Set::ALL
                .into_iter()
                .find_map(|set| name.strip_prefix(set.prefix()).map(|path| (set, path)))
            else {
                continue;
            };
            index.files.insert((set, path), Packed { offset, len, size });
            let mut child = path;
            let mut is_dir = false;
            loop {
                let (parent, name) = child.rsplit_once('/').unwrap_or(("", child));
                let siblings = index.dirs.entry((set, parent.to_string())).or_default();
                let known = siblings.insert(name.to_string(), is_dir).is_some();
                if parent.is_empty() || known {
                    break;
                }
                child = parent;
                is_dir = true;
            }
        }
        index
    }
}

/// A packed blob with its index, at a level: the one built in ([`built_in`]), or one a test packs.
struct Archive {
    blob: &'static [u8],
    index_text: &'static str,
    index: Index,
    level: Level,
}

/// The archive built into this binary, its index read once.
fn built_in() -> &'static Archive {
    static BUILT_IN: OnceLock<Archive> = OnceLock::new();
    BUILT_IN.get_or_init(|| Archive::new(BLOB, INDEX, LEVEL))
}

impl Archive {
    fn new(blob: &'static [u8], index_text: &'static str, level: Level) -> Self {
        Self {
            blob,
            index_text,
            index: Index::parse(index_text),
            level,
        }
    }

    fn unpack(&self, packed: Packed) -> io::Result<Vec<u8>> {
        let slice = self
            .blob
            .get(packed.offset..packed.offset + packed.len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the built-in files are damaged"))?;
        let mut bytes = Vec::with_capacity(packed.size);
        flate2::read::DeflateDecoder::new(slice).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn file(&self, set: Set, path: &str) -> io::Result<Option<Vec<u8>>> {
        match self.index.files.get(&(set, path)) {
            Some(packed) => self.unpack(*packed).map(Some),
            None => Ok(None),
        }
    }

    fn is_dir(&self, set: Set, path: &str) -> bool {
        self.index
            .dirs
            .contains_key(&(set, path.trim_end_matches('/').to_string()))
    }

    fn children(&self, set: Set, dir: &str) -> Vec<(String, bool)> {
        self.index
            .dirs
            .get(&(set, dir.trim_end_matches('/').to_string()))
            .map(|children| children.iter().map(|(name, is_dir)| (name.clone(), *is_dir)).collect())
            .unwrap_or_default()
    }

    fn entries(&self) -> io::Result<Vec<Entry>> {
        self.index
            .files
            .iter()
            .map(|((set, path), packed)| {
                Ok(Entry {
                    set: *set,
                    path: (*path).to_string(),
                    bytes: self.unpack(*packed)?,
                })
            })
            .collect()
    }

    /// Each built-in component of [`Set::Components`], as `<set>/<component>`.
    fn components(&self) -> Vec<String> {
        let mut components = Vec::new();
        for (set, is_dir) in self.children(Set::Components, "") {
            if !is_dir {
                continue;
            }
            for (component, is_dir) in self.children(Set::Components, &set) {
                if is_dir {
                    components.push(format!("{set}/{component}"));
                }
            }
        }
        components
    }

    fn write_runtime(&self, runtime_dir: &Path, framework_dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for ((set, path), packed) in &self.index.files {
            let target = set.dir_in(runtime_dir, framework_dir).join(path);
            if target.exists() {
                continue;
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|err| named(err, "could not create", parent))?;
            }
            std::fs::write(&target, self.unpack(*packed)?).map_err(|err| named(err, "could not write", &target))?;
            written.push(target);
        }
        Ok(written)
    }

    fn compare(&self, dirs: &[(Set, &Path)]) -> io::Result<Vec<Difference>> {
        let dir_of = |set: Set| dirs.iter().find(|(named, _)| *named == set).map(|(_, dir)| *dir);
        let skip = self.level.component_skip();
        let mut differences = Vec::new();
        let mut packed = BTreeSet::new();
        for entry in self.entries()? {
            if !Set::COMPONENTS.contains(&entry.set) {
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
        for (set, dir) in dirs.iter().filter(|(set, _)| set.is_whole_at(self.level)) {
            let mut on_disk = Vec::new();
            if *set == Set::Components {
                let components = self.components();
                for component in &components {
                    let walk = DiskWalk { set: *set, skip };
                    walk.files(
                        &dir.join(component),
                        &format!("{component}/"),
                        true,
                        &mut on_disk,
                        &mut differences,
                    );
                }
                self.not_built_in(dir, &components, &mut differences);
            } else {
                DiskWalk { set: *set, skip }.files(dir, "", true, &mut on_disk, &mut differences);
            }
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

    /// The components in `components_dir`, in the sets the level packs, that the binary does not have.
    fn not_built_in(&self, components_dir: &Path, built_in: &[String], differences: &mut Vec<Difference>) {
        for set in self.level.component_sets() {
            let Ok(entries) = std::fs::read_dir(components_dir.join(set)) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !select::is_excluded(name, false))
                .filter(|name| *set != "base" || !select::OWN_SETS.contains(&name.as_str()))
                .collect();
            names.sort();
            for name in names {
                let path = format!("{set}/{name}");
                if !built_in.contains(&path) {
                    differences.push(Difference {
                        set: Set::Components,
                        path,
                        kind: Kind::NotBuiltIn,
                    });
                }
            }
        }
    }

    fn unified_diff(&self, set: Set, path: &str, dir: &Path) -> io::Result<Option<String>> {
        let packed = self
            .file(set, path)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} has no file {path}", set.name())))?;
        let disk = match std::fs::read(dir.join(path)) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(named(err, "could not read", &dir.join(path))),
        };
        if disk == packed {
            return Ok(None);
        }
        let (Ok(old), Ok(new)) = (std::str::from_utf8(&packed), std::str::from_utf8(&disk)) else {
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
}

/// The level this binary was built at.
#[must_use]
pub fn level() -> Level {
    built_in().level
}

/// The packed file `path` of `set`, unpacked; `None` when the set has no such file.
///
/// # Errors
///
/// Returns an error when the file cannot be unpacked, which only a damaged binary would cause.
pub fn file(set: Set, path: &str) -> io::Result<Option<Vec<u8>>> {
    built_in().file(set, path)
}

/// Whether `path` of `set` is a packed file.
#[must_use]
pub fn is_file(set: Set, path: &str) -> bool {
    built_in().index.files.contains_key(&(set, path))
}

/// Whether `path` of `set` is a directory of packed files; `""` is the set's own, when it has any.
#[must_use]
pub fn is_dir(set: Set, path: &str) -> bool {
    built_in().is_dir(set, path)
}

/// The children of the directory `dir` of `set` (`""` for the set's own), sorted by name, each with whether it is a
/// directory; empty when it is not a directory of packed files.
#[must_use]
pub fn children(set: Set, dir: &str) -> Vec<(String, bool)> {
    built_in().children(set, dir)
}

/// The components of [`Set::Components`], as `<set>/<component>` (`addon/mr-mantle`), sorted; none at the framework
/// level.
#[must_use]
pub fn components() -> Vec<String> {
    built_in().components()
}

/// Every packed file, unpacked, sorted by set and path. Each call unpacks them again; nothing is kept.
///
/// # Errors
///
/// Returns an error when a file cannot be unpacked, which only a damaged binary would cause.
pub fn entries() -> io::Result<Vec<Entry>> {
    built_in().entries()
}

/// The number of packed files and their total unpacked size, per set, without unpacking them.
#[must_use]
pub fn sizes() -> BTreeMap<Set, (usize, u64)> {
    let mut sizes: BTreeMap<Set, (usize, u64)> = BTreeMap::new();
    for ((set, _), packed) in &built_in().index.files {
        let size = sizes.entry(*set).or_default();
        size.0 += 1;
        size.1 += packed.size as u64;
    }
    sizes
}

/// The index and the blob as the binary has them, in that order: what the `runtime` checksum is over.
#[must_use]
pub fn packed() -> (&'static [u8], &'static [u8]) {
    let archive = built_in();
    (archive.index_text.as_bytes(), archive.blob)
}

/// Write each built-in file that is not on disk into the runtime `runtime_dir` whose framework component is
/// `framework_dir` ([`Set::dir_in`]), so the runtime no longer reads any from the binary: the framework, tools and the
/// webroot, the other built-in components, the sets' manifest, the placeholder page at `/` when the webroot has no
/// `qapps.xml`, and the confs. A file already there is left as it is. Returns the files written.
///
/// # Errors
///
/// Returns an error naming the path when a directory or file cannot be written, or when a file cannot be unpacked.
pub fn write_runtime(runtime_dir: &Path, framework_dir: &Path) -> io::Result<Vec<PathBuf>> {
    built_in().write_runtime(runtime_dir, framework_dir)
}

/// Compare each component set in `dirs` (a set and its directory in the runtime) with the packed files; a set not named
/// is not compared. The result is sorted by set and path; empty when all match. A file that cannot be read is a
/// [`Kind::Unreadable`] difference, and the rest are still compared. In [`Set::Components`] a file on disk is
/// [`Kind::Added`] only in a component the binary has, and a component it does not have is one [`Kind::NotBuiltIn`].
///
/// # Errors
///
/// Returns an error only when the archive cannot be read.
pub fn compare(dirs: &[(Set, &Path)]) -> io::Result<Vec<Difference>> {
    built_in().compare(dirs)
}

/// A unified diff of the packed `path` of `set` (the old side) against the file under `dir` (the new side): `None`
/// when they are the same, a one-line note for a binary file.
///
/// # Errors
///
/// Returns an error when `path` is not packed in `set`, or the archive cannot be read. A file not on disk diffs
/// against nothing.
pub fn unified_diff(set: Set, path: &str, dir: &Path) -> io::Result<Option<String>> {
    built_in().unified_diff(set, path, dir)
}

/// The first line, from 1, where `a` and `b` differ.
fn first_differing_line(a: &[u8], b: &[u8]) -> usize {
    let common = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    a[..common].iter().filter(|byte| **byte == b'\n').count() + 1
}

/// A walk of the files on disk of one component set, for [`Kind::Added`].
struct DiskWalk<'a> {
    set: Set,
    /// The directories left out at the top of a component.
    skip: &'a [&'a str],
}

impl DiskWalk<'_> {
    /// The files under `dir`, as `/`-separated paths after `prefix`, less the [`select::is_excluded`] names and, at the
    /// `top` of a component, the skipped directories. A directory that cannot be read is an [`Kind::Unreadable`]
    /// difference; a component not on disk at all has nothing added.
    fn files(&self, dir: &Path, prefix: &str, top: bool, files: &mut Vec<String>, differences: &mut Vec<Difference>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound && top => return,
            Err(err) => {
                differences.push(Difference {
                    set: self.set,
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
            if select::is_excluded(&name, top) || (top && self.skip.contains(&name.as_str())) {
                continue;
            }
            let path = format!("{prefix}{name}");
            let is_dir = std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir());
            if is_dir {
                self.files(&entry.path(), &format!("{path}/"), false, files, differences);
            } else {
                files.push(path);
            }
        }
    }
}

fn named(err: io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(err.kind(), format!("{what} {}: {err}", path.display()))
}

#[cfg(test)]
mod tests;
