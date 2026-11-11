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

//! The artifact index (plans/design-dev-aide.md 7.1, F11): every definition the components' files make, with its file
//! and line, the references between them, and on a dev instance the calls observed on the artifact stack. It serves
//! search, the reference lists, the Customizations list and the extension checks, go to definition, the agents'
//! `artifact` tool, MCP and `moqui refs`.
//!
//! On a server the index is rows (`moqui.devtools`, `DevToolsEntities.xml`), so the database filters, sorts and counts
//! and a restart keeps it: built in the background when a dev server starts (or at first use), a file whose hash is
//! unchanged is not read again, and each reload replaces the rows of the files it read. A file that does not parse
//! keeps its rows, marked stale, and the others go on. A model-only boot (`moqui refs`, `moqui check --extensions`)
//! reads the files into a [`MemoryIndex`] instead.

// on the artifact stack's hot path, so core's
pub use moqui_core::devtools::calls;
pub mod extract;
pub mod resolve;
pub mod rows;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use moqui_lib::sync::{lock, try_lock};
use serde::Serialize;

pub use self::extract::{ArtifactRow, BindingRow, FileIndex, FileInfo, RefRow, ToKind};
use self::resolve::Resolver;
use super::ArtifactKind;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{fence, EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;

const FILE: &str = "moqui.devtools.DevArtifactFile";
const ARTIFACT: &str = "moqui.devtools.DevArtifact";
const REF: &str = "moqui.devtools.DevArtifactRef";
const CALL: &str = "moqui.devtools.DevArtifactCall";
const BINDING: &str = "moqui.devtools.DevArtifactBinding";

/// The directories of a component the index does not walk.
const SKIPPED_DIRS: &[&str] = &[".git", "target", "node_modules", "libs", "build", "tmp"];

/// A file the index reads, located in its component.
#[derive(Clone, Debug)]
pub struct Located {
    pub path: PathBuf,
    pub component: String,
    /// `component://<component>/<path in it>`.
    pub location: String,
    pub kind: ArtifactKind,
}

impl Located {
    fn info(&self) -> (String, FileInfo<'_>) {
        let display = self.path.display().to_string();
        (
            display,
            FileInfo {
                path: "",
                location: &self.location,
                component: &self.component,
                kind: self.kind,
            },
        )
    }

    /// Read the file's artifacts and references, its data records by `factory`'s entity model.
    ///
    /// # Errors
    ///
    /// Returns why when the file does not read or does not parse.
    pub fn extract(&self, factory: &ExecutionContextFactory) -> Result<(String, FileIndex), String> {
        let text = std::fs::read_to_string(&self.path).map_err(|err| err.to_string())?;
        let index = self.extract_text(factory, &text)?;
        Ok((hash_of(&text), index))
    }

    /// The artifacts and references of `text` as this file's (an editor's unsaved buffer).
    ///
    /// # Errors
    ///
    /// Returns why when the text does not parse.
    pub fn extract_text(&self, factory: &ExecutionContextFactory, text: &str) -> Result<FileIndex, String> {
        let (display, mut info) = self.info();
        info.path = &display;
        extract::extract_with(&info, text, &FactoryModel(factory))
    }
}

/// The entity model of a factory, which names a data record's entity and primary key ([`extract::RecordModel`]).
struct FactoryModel<'a>(&'a ExecutionContextFactory);

impl extract::RecordModel for FactoryModel<'_> {
    fn keys(&self, name: &str) -> Option<(String, Vec<String>)> {
        self.0.entity()?.with_model(|model| {
            let entity = model.entity(name).ok()?;
            let pk = entity.pk_field_names().into_iter().map(str::to_string).collect();
            Some((entity.full_name.clone(), pk))
        })
    }

    fn related(&self, entity: &str, name: &str) -> Option<(String, Vec<(String, String)>)> {
        self.0.entity()?.with_model(|model| {
            let relationship = model.entity(entity).ok()?.relationship(name)?;
            // the full name: `related_entity_name` is the short one, which two packages may share
            Some((relationship.related.clone(), relationship.key_pairs.clone()))
        })
    }
}

/// Where `path` is: its component and kind, when it is in a loaded component and of a kind the index reads.
#[must_use]
pub fn locate(factory: &ExecutionContextFactory, path: &Path) -> Option<Located> {
    let path = canonical(path);
    factory
        .components()
        .iter()
        .find(|component| path.starts_with(canonical(&component.location)))
        .and_then(|component| locate_in(&component.name, &canonical(&component.location), &path))
}

/// `path` as a file of the component `name` at `root`, when it is of a kind the index reads.
#[must_use]
pub fn locate_in(name: &str, root: &Path, path: &Path) -> Option<Located> {
    let relative = path.strip_prefix(root).ok()?;
    if relative
        .components()
        .any(|part| SKIPPED_DIRS.iter().any(|skipped| part.as_os_str() == *skipped))
    {
        return None;
    }
    let kind = super::reload::kind_of(name, relative);
    extract::file_kind(kind)?;
    // a data directory's other files (images, documents) are no data files; of a template directory only the XML files
    // (widget templates) are read
    if matches!(kind, ArtifactKind::Data | ArtifactKind::ScreenText) && path.extension().is_none_or(|ext| ext != "xml")
    {
        return None;
    }
    let relative = relative.to_string_lossy().replace('\\', "/");
    Some(Located {
        location: format!("component://{name}/{relative}"),
        component: name.to_string(),
        kind,
        path: path.to_path_buf(),
    })
}

/// The files of every loaded component the index reads.
#[must_use]
pub fn component_files(factory: &ExecutionContextFactory) -> Vec<Located> {
    factory
        .components()
        .iter()
        .flat_map(|component| directory_files(&component.name, &component.location))
        .collect()
}

/// The files the index reads of the component `name` at `dir`, loaded or not.
#[must_use]
pub fn directory_files(name: &str, dir: &Path) -> Vec<Located> {
    let root = canonical(dir);
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.iter().filter_map(|path| locate_in(name, &root, path)).collect()
}

fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if path.is_dir() {
            if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name) {
                walk(&path, into);
            }
        } else {
            into.push(path);
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A short hash of a file's text: whether it changed since it was read.
#[must_use]
pub fn hash_of(text: &str) -> String {
    use sha2::{Digest, Sha256};
    // Version the extraction rules so persisted files are read again when they change: 2 added parameter artifacts,
    // 3 the transition anchors of a screen-extend, 4 relationships, indexes, view members, form fields, subscreens items,
    // ext items, widget templates, data records, service includes, a screen-extend's own transitions and forms, and ECA
    // keys by rule name or target, 5 the references' operations and origins, the bindings and the primary key mark of a
    // field, 6 screen paths (links, responses, forms' transitions, default items), directory subscreens, a subscreens
    // item's no-sub-path mark, the services a transition passes the request to, and the bindings of a form's fields,
    // 7 the marks of what a service, form or transition declares to be filled in (auto-parameters, implements,
    // auto-fields, extends, a transition's calls and field-maps), and the references of auto-parameters and extends.
    let digest = Sha256::digest(format!("7\n{text}").as_bytes());
    digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect()
}

/// What identifies the state of an index, answered with every read so a caller can say what it checked against.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStamp {
    /// A hash of each file's location and content hash: the same files give the same hash, on any server.
    pub hash: String,
    /// When a file was last read into the index, in epoch milliseconds (a removal does not move it).
    pub built_at: i64,
    pub files: usize,
}

impl IndexStamp {
    /// The stamp of `files` (each a location, or a path out of any component, and its [`hash_of`]).
    fn of(mut files: Vec<(String, String)>, built_at: i64) -> Self {
        use sha2::{Digest, Sha256};
        files.sort();
        let mut digest = Sha256::new();
        for (location, hash) in &files {
            digest.update(format!("{location}\t{hash}\n").as_bytes());
        }
        Self {
            hash: digest
                .finalize()
                .iter()
                .take(12)
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            built_at,
            files: files.len(),
        }
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Write `factor` copies of `files` under `dir`, each copy's entities in a package and its services under a path of
/// its own (`synth3.mantle.order`), for measuring the index on a corpus that many times the size. Answers the copies.
///
/// # Errors
///
/// Returns an error when a copy cannot be written.
pub fn synthetic(files: &[Located], dir: &Path, factor: usize) -> Result<Vec<Located>, Error> {
    let mut copies = Vec::with_capacity(files.len() * factor);
    for copy in 0..factor {
        let tag = format!("synth{copy}");
        for file in files {
            let Ok(text) = std::fs::read_to_string(&file.path) else {
                continue;
            };
            let relative = file
                .location
                .strip_prefix("component://")
                .and_then(|rest| rest.split_once('/'))
                .map_or(file.location.as_str(), |(_, rest)| rest);
            // the copy's own service path and entity package, so its ids are its own
            let (top, rest) = relative.split_once('/').unwrap_or((relative, ""));
            let relative = format!("{top}/{tag}/{}/{rest}", file.component);
            let text = text.replace("package=\"", &format!("package=\"{tag}."));
            let path = dir.join(&relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::new(format!("{}: {err}", parent.display())))?;
            }
            std::fs::write(&path, text).map_err(|err| Error::new(format!("{}: {err}", path.display())))?;
            copies.push(Located {
                location: format!("component://synthetic/{relative}"),
                component: "synthetic".into(),
                kind: file.kind,
                path,
            });
        }
    }
    Ok(copies)
}

/// The directory of each component `files` are in, in the order they first come: a file's path less its path in the
/// component. A file whose path does not end so (a copy written elsewhere) names none.
fn component_roots(files: &[Located]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for file in files {
        let Some((_, relative)) = file
            .location
            .strip_prefix("component://")
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        let Some(root) = file.path.ancestors().nth(relative.split('/').count()) else {
            continue;
        };
        if root.join(relative) == file.path && !roots.iter().any(|known| known == root) {
            roots.push(root.to_path_buf());
        }
    }
    roots
}

/// Whether a resource location names something that exists.
fn location_exists(factory: &ExecutionContextFactory, location: &str) -> bool {
    factory
        .resource()
        .get(location)
        .ok()
        .is_some_and(|resource| resource.exists())
}

// ---------------------------------------------------------------------------------------------------------------------
// The answers, alike for the rows and the memory index.

/// One reference in a list of what uses an artifact or what it uses.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefEntry {
    pub path: String,
    pub line: u32,
    pub ref_type: String,
    pub from: String,
    pub to_kind: String,
    pub to_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_id: Option<String>,
    pub resolved: bool,
    /// The artifact on the other side (the user for an incoming list, the used for an outgoing one), when indexed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other: Option<ArtifactRow>,
    /// How often the call was observed; 0 for a reference only read from the files.
    pub calls: i64,
    /// Observed only: no file names it.
    pub observed_only: bool,
    /// What it does to its target ([`RefRow::operation`]).
    pub operation: String,
    /// Where it was read from ([`RefRow::origin`]).
    pub origin: String,
    pub origin_hash: String,
}

/// One binding ([`BindingRow`]) with the file it is in.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingEntry {
    pub path: String,
    #[serde(flatten)]
    pub binding: BindingRow,
}

/// An extension of a site component (an `extend-entity`, a `screen-extend` and its anchors, an ECA rule on an
/// upstream service or entity) with whether its target is still there.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Extension {
    pub component: String,
    pub kind: String,
    pub path: String,
    pub line: u32,
    pub from: String,
    pub ref_type: String,
    pub target: String,
    pub scope: String,
    pub resolved: bool,
}

/// What is at a line of a file: the reference there (when there is one) and the artifact it names, or the artifact the
/// line defines.
pub type At = (Option<RefEntry>, Option<ArtifactRow>);

/// Which way a reference list goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// What uses the artifact.
    In,
    /// What the artifact uses.
    Out,
}

impl Direction {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.eq_ignore_ascii_case("out") {
            Self::Out
        } else {
            Self::In
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The memory index of a model-only boot.

/// The index of a model-only boot: every file read into memory, with no database.
#[derive(Clone, Debug, Default)]
pub struct MemoryIndex {
    pub artifacts: Vec<ArtifactRow>,
    /// Each reference with the file it is in.
    pub refs: Vec<(String, RefRow)>,
    /// Each binding with the file it is in.
    pub bindings: Vec<(String, BindingRow)>,
    pub resolver: Resolver,
    /// Files that did not read or parse, with why.
    pub problems: Vec<(String, String)>,
    /// Each artifact's position in `artifacts` by id, for [`Self::describe`]; made again by each read.
    positions: HashMap<String, usize>,
    /// Each file read by its path: its location and hash, for [`Self::stamp`].
    hashes: BTreeMap<String, (String, String)>,
    /// When the files were last read, in epoch milliseconds.
    changed_millis: i64,
}

impl MemoryIndex {
    /// Read every component file of `factory`.
    #[must_use]
    pub fn build(factory: &ExecutionContextFactory) -> Self {
        let mut index = Self::default();
        // the conf's screen mounts, so a path through `/apps/...` walks them
        let mounts = rows::conf_mounts(&moqui_core::screen::ScreenFacade::of(factory).conf().screens);
        index.take(mounts.path.clone(), mounts.path, mounts.hash, mounts.index);
        index.read_files(factory, &component_files(factory));
        index
    }

    /// Read `files`, with the screen mounts of their components, so a path through `/apps/...` walks them as a server's
    /// index does: when `factory` loaded every component the files are in (a project booted on its own runtime), the
    /// mounts of its merged conf, which boot merged with the runtime conf; else (a plain directory read on its own)
    /// those of the components' own `MoquiConf.xml`s ([`rows::directory_mounts`]).
    #[must_use]
    pub fn of_files(factory: &ExecutionContextFactory, files: &[Located]) -> Self {
        let mut index = Self::default();
        let roots = component_roots(files);
        let loaded = factory.components();
        let all_loaded = !roots.is_empty()
            && roots.iter().all(|root| {
                let root = canonical(root);
                loaded.iter().any(|component| canonical(&component.location) == root)
            });
        let mounts = if all_loaded {
            Some(rows::conf_mounts(
                &moqui_core::screen::ScreenFacade::of(factory).conf().screens,
            ))
        } else {
            rows::directory_mounts(&roots)
        };
        if let Some(mounts) = mounts {
            index.take(mounts.path.clone(), mounts.path, mounts.hash, mounts.index);
        }
        index.read_files(factory, files);
        index
    }

    /// Read `files` into the index, and resolve everything.
    fn read_files(&mut self, factory: &ExecutionContextFactory, files: &[Located]) {
        for file in files {
            match file.extract(factory) {
                Ok((hash, read)) => self.take(file.path.display().to_string(), file.location.clone(), hash, read),
                Err(problem) => self.problem(file, problem),
            }
        }
        self.resolve_all(&|location: &str| location_exists(factory, location));
    }

    /// The stamp of the files read.
    #[must_use]
    pub fn stamp(&self) -> IndexStamp {
        IndexStamp::of(self.hashes.values().cloned().collect(), self.changed_millis)
    }

    /// Keep that `file` did not read or parse, and its hash when its text reads: the stamp counts the file, as the
    /// rows of a server's index keep it (stale).
    fn problem(&mut self, file: &Located, problem: String) {
        let path = file.path.display().to_string();
        tracing::error!("Artifact index: {path} not read: {problem}");
        if let Ok(text) = std::fs::read_to_string(&file.path) {
            self.hashes
                .insert(path.clone(), (file.location.clone(), hash_of(&text)));
        }
        self.problems.push((path, problem));
    }

    /// Keep what the file `path` (at `location`, with `hash`) defines and refers to.
    fn take(&mut self, path: String, location: String, hash: String, read: FileIndex) {
        self.hashes.insert(path.clone(), (location, hash));
        self.take_rows(path, read);
    }

    /// Keep the rows of the file `path`, its hash left as it was (text not yet saved).
    fn take_rows(&mut self, path: String, read: FileIndex) {
        self.artifacts.extend(read.artifacts);
        self.refs.extend(read.refs.into_iter().map(|row| (path.clone(), row)));
        self.bindings
            .extend(read.bindings.into_iter().map(|row| (path.clone(), row)));
    }

    /// Know every artifact again, derive the entity-auto services the references need ([`Resolver::derive`], in place
    /// of the ones derived before), and resolve every reference and binding; the files are changed now.
    pub fn resolve_all(&mut self, exists: &dyn Fn(&str) -> bool) {
        self.artifacts
            .retain(|artifact| artifact.origin != extract::ORIGIN_DERIVED && !extract::is_expanded(&artifact.origin));
        self.bindings
            .retain(|(_, binding)| !extract::is_expanded(&binding.origin));
        let mut resolver = Resolver::default();
        for artifact in &self.artifacts {
            resolver.add(artifact);
        }
        let derived = resolver.derive(self.refs.iter().map(|(_, row)| row));
        for artifact in &derived {
            resolver.add(artifact);
        }
        self.artifacts.extend(derived);
        // what the services and forms declare to be filled in: in their files, from the artifacts known now
        for owner in resolver.expandable() {
            let expansion = resolver.expand(&owner);
            let Some(path) = resolver.owner_path(&owner) else {
                continue;
            };
            for (id, summary) in &expansion.summaries {
                for artifact in self
                    .artifacts
                    .iter_mut()
                    .filter(|artifact| artifact.id == *id && artifact.path == path)
                {
                    artifact.summary.clone_from(summary);
                }
            }
            for artifact in &expansion.artifacts {
                resolver.add(artifact);
            }
            self.artifacts.extend(expansion.artifacts);
            self.bindings
                .extend(expansion.bindings.into_iter().map(|binding| (path.clone(), binding)));
        }
        for (_, row) in &mut self.refs {
            resolver.resolve(row, exists);
        }
        let mut bindings = Vec::with_capacity(self.bindings.len());
        for (path, binding) in std::mem::take(&mut self.bindings) {
            bindings.extend(
                resolver
                    .resolve_bindings(vec![binding])
                    .into_iter()
                    .map(|binding| (path.clone(), binding)),
            );
        }
        self.bindings = bindings;
        self.resolver = resolver;
        self.position();
        self.changed_millis = now_millis();
    }

    /// The bindings made at `from` and to `to` (either left empty matches any), in file order.
    #[must_use]
    pub fn bindings_of(&self, from: &str, to: &str) -> Vec<BindingEntry> {
        self.bindings
            .iter()
            .filter(|(_, row)| (from.is_empty() || row.from == from) && (to.is_empty() || row.to_id == to))
            .map(|(path, row)| BindingEntry {
                path: path.clone(),
                binding: row.clone(),
            })
            .collect()
    }

    /// Each id's first row, as a find of the rows in order answers (an id defined in two components is the first's).
    fn position(&mut self) {
        let mut positions = HashMap::with_capacity(self.artifacts.len());
        for (at, artifact) in self.artifacts.iter().enumerate() {
            positions.entry(artifact.id.clone()).or_insert(at);
        }
        self.positions = positions;
    }

    /// The rows of the files `paths` read again from disk (`files` are the ones there now; a path with none was
    /// removed), the names resolved again. A file that does not read or parse loses its rows and is listed in
    /// [`Self::problems`]; the others go on. Answers how many files were read.
    pub fn replace_files(&mut self, factory: &ExecutionContextFactory, paths: &[String], files: &[Located]) -> usize {
        let gone: HashSet<&str> = paths.iter().map(String::as_str).collect();
        self.artifacts.retain(|artifact| !gone.contains(artifact.path.as_str()));
        self.refs.retain(|(file, _)| !gone.contains(file.as_str()));
        self.bindings.retain(|(file, _)| !gone.contains(file.as_str()));
        self.problems.retain(|(file, _)| !gone.contains(file.as_str()));
        self.hashes.retain(|file, _| !gone.contains(file.as_str()));
        for file in files {
            let path = file.path.display().to_string();
            match file.extract(factory) {
                Ok((hash, read)) => self.take(path, file.location.clone(), hash, read),
                Err(problem) => self.problem(file, problem),
            }
        }
        self.resolve_all(&|location: &str| location_exists(factory, location));
        files.len()
    }

    /// [`Self::find`] in `components` when not empty.
    #[must_use]
    pub fn find_in(&self, text: &str, kinds: &[String], components: &[String], limit: usize) -> Vec<ArtifactRow> {
        rank(
            self.artifacts
                .iter()
                .filter(|artifact| kinds.is_empty() || kinds.contains(&artifact.kind))
                .filter(|artifact| components.is_empty() || components.contains(&artifact.component))
                .cloned()
                .collect(),
            text,
            limit,
        )
    }

    /// The artifacts whose parent is `id` (an entity's fields, a screen's transitions and forms), in file order.
    #[must_use]
    pub fn children(&self, id: &str) -> Vec<ArtifactRow> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.parent.as_deref() == Some(id))
            .cloned()
            .collect()
    }

    /// The artifacts the file `path` defines at its top (no parent): a screen, an entity file's entities, a service
    /// file's services.
    #[must_use]
    pub fn in_file(&self, path: &str) -> Vec<ArtifactRow> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.path == path && artifact.parent.is_none())
            .cloned()
            .collect()
    }

    /// [`Self::refs`], keeping those whose other side is of `kinds` when not empty, as the database index's [`refs`].
    #[must_use]
    pub fn refs_of_kinds(&self, id: &str, direction: Direction, kinds: &[String]) -> Vec<RefEntry> {
        let mut entries = self.refs(id, direction);
        if !kinds.is_empty() {
            entries.retain(|item| item.other.as_ref().is_some_and(|other| kinds.contains(&other.kind)));
        }
        entries
    }

    /// The artifacts whose names match `text`, best first.
    #[must_use]
    pub fn find(&self, text: &str, kinds: &[String], limit: usize) -> Vec<ArtifactRow> {
        rank(
            self.artifacts
                .iter()
                .filter(|artifact| kinds.is_empty() || kinds.contains(&artifact.kind))
                .cloned()
                .collect(),
            text,
            limit,
        )
    }

    #[must_use]
    pub fn describe(&self, id: &str) -> Option<ArtifactRow> {
        if let Some(found) = self
            .positions
            .get(id)
            .and_then(|at| self.artifacts.get(*at))
            .filter(|artifact| artifact.id == id)
        {
            return Some(found.clone());
        }
        self.artifacts.iter().find(|artifact| artifact.id == id).cloned()
    }

    /// An artifact named by its id, or found by its name.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<ArtifactRow> {
        self.describe(name)
            .or_else(|| self.find(name, &[], 1).into_iter().next())
    }

    /// What uses `id`, or what it uses; for a screen or entity, its transitions, forms and fields too.
    #[must_use]
    pub fn refs(&self, id: &str, direction: Direction) -> Vec<RefEntry> {
        let parameters = with_parameters(id, &[]);
        let mut ids: HashSet<&str> = HashSet::from([id]);
        ids.extend(
            self.artifacts
                .iter()
                .filter(|artifact| artifact.parent.as_deref() == Some(id))
                .filter(|artifact| artifact.kind != "record" && (parameters || artifact.kind != "parameter"))
                .map(|artifact| artifact.id.as_str()),
        );
        self.refs
            .iter()
            .filter(|(_, row)| match direction {
                Direction::In => row.to_id.as_deref().is_some_and(|to| ids.contains(to)),
                Direction::Out => ids.contains(row.from.as_str()) && (parameters || !to_parameter(row)),
            })
            .map(|(path, row)| {
                let other = match direction {
                    Direction::In => self.describe(&row.from),
                    Direction::Out => row.to_id.as_deref().and_then(|to| self.describe(to)),
                };
                entry(path, row, other)
            })
            .collect()
    }

    /// The artifact or reference at `line` of `path`, for go to definition, as the database index's [`at`]: the
    /// reference on the line naming `word` (or the first on the line) and the artifact it resolves to; else the
    /// artifact defined on the line.
    #[must_use]
    pub fn at(&self, path: &str, line: u32, word: &str) -> Option<At> {
        let word = word.trim();
        let on_line: Vec<&(String, RefRow)> = self
            .refs
            .iter()
            .filter(|(file, row)| file == path && row.line == line)
            .collect();
        let chosen = on_line
            .iter()
            .find(|(_, row)| {
                !word.is_empty()
                    && (row.to_name == word || row.to_name.ends_with(&format!(".{word}")) || row.to_name.contains(word))
            })
            .or_else(|| on_line.first());
        if let Some((file, row)) = chosen {
            let target = row.to_id.as_deref().and_then(|id| self.describe(id));
            return Some((Some(entry(file, row, None)), target));
        }
        self.artifacts
            .iter()
            .find(|artifact| artifact.path == path && artifact.line == line)
            .map(|artifact| (None, Some(artifact.clone())))
    }

    /// `located`'s rows read again from `text` (an editor's buffer, saved or not) in place of the ones it had, the
    /// names resolved again; `false` when the text does not parse, and the old rows stay.
    pub fn update_text(&mut self, factory: &ExecutionContextFactory, located: &Located, text: &str) -> bool {
        let Ok(read) = located.extract_text(factory, text) else {
            return false;
        };
        let path = located.path.display().to_string();
        self.artifacts.retain(|artifact| artifact.path != path);
        self.refs.retain(|(file, _)| *file != path);
        self.bindings.retain(|(file, _)| *file != path);
        self.take_rows(path, read);
        self.resolve_all(&|location: &str| location_exists(factory, location));
        true
    }

    /// The extensions of `components` (every component when empty), broken ones first.
    #[must_use]
    pub fn extensions(&self, components: &[String]) -> Vec<Extension> {
        let by_id: HashMap<&str, &ArtifactRow> = self
            .artifacts
            .iter()
            .map(|artifact| (artifact.id.as_str(), artifact))
            .collect();
        let mut found: Vec<Extension> = self
            .refs
            .iter()
            .filter(|(_, row)| row.ref_type == "extend" || row.ref_type == "anchor")
            .filter_map(|(path, row)| {
                let from = by_id.get(row.from.as_str())?;
                (components.is_empty() || components.contains(&from.component)).then(|| extension(from, path, row))
            })
            .collect();
        found.sort_by(|a, b| {
            a.resolved
                .cmp(&b.resolved)
                .then_with(|| a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
        found
    }
}

/// A [`MemoryIndex`]'s references by the artifact on each side and its artifacts by their parent, for many reference lists
/// read in one pass (the docs of a whole tree): [`Self::refs`] answers as [`MemoryIndex::refs`] does, without a scan of
/// every reference for each artifact.
pub struct RefTable<'a> {
    index: &'a MemoryIndex,
    to: HashMap<&'a str, Vec<usize>>,
    from: HashMap<&'a str, Vec<usize>>,
    children: HashMap<&'a str, Vec<usize>>,
}

impl MemoryIndex {
    /// The reference table of this index.
    #[must_use]
    pub fn ref_table(&self) -> RefTable<'_> {
        let mut to: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut from: HashMap<&str, Vec<usize>> = HashMap::new();
        for (at, (_, row)) in self.refs.iter().enumerate() {
            if let Some(id) = row.to_id.as_deref() {
                to.entry(id).or_default().push(at);
            }
            from.entry(row.from.as_str()).or_default().push(at);
        }
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        for (at, artifact) in self.artifacts.iter().enumerate() {
            if let Some(parent) = artifact.parent.as_deref() {
                children.entry(parent).or_default().push(at);
            }
        }
        RefTable {
            index: self,
            to,
            from,
            children,
        }
    }
}

impl RefTable<'_> {
    /// The artifacts whose parent is `id`, in file order.
    #[must_use]
    pub fn children(&self, id: &str) -> Vec<ArtifactRow> {
        self.children
            .get(id)
            .map(|list| list.iter().map(|at| self.index.artifacts[*at].clone()).collect())
            .unwrap_or_default()
    }

    /// What uses `id`, or what it uses, as [`MemoryIndex::refs`].
    #[must_use]
    pub fn refs(&self, id: &str, direction: Direction) -> Vec<RefEntry> {
        let parameters = with_parameters(id, &[]);
        let mut ids: Vec<&str> = vec![id];
        if let Some(list) = self.children.get(id) {
            ids.extend(
                list.iter()
                    .map(|at| &self.index.artifacts[*at])
                    .filter(|artifact| artifact.kind != "record" && (parameters || artifact.kind != "parameter"))
                    .map(|artifact| artifact.id.as_str()),
            );
        }
        let by = match direction {
            Direction::In => &self.to,
            Direction::Out => &self.from,
        };
        let mut found: Vec<usize> = ids
            .iter()
            .filter_map(|id| by.get(id))
            .flatten()
            .copied()
            .filter(|at| direction == Direction::In || parameters || !to_parameter(&self.index.refs[*at].1))
            .collect();
        found.sort_unstable();
        found.dedup();
        found
            .into_iter()
            .map(|at| {
                let (path, row) = &self.index.refs[at];
                let other = match direction {
                    Direction::In => self.index.describe(&row.from),
                    Direction::Out => row.to_id.as_deref().and_then(|to| self.index.describe(to)),
                };
                entry(path, row, other)
            })
            .collect()
    }
}

fn entry(path: &str, row: &RefRow, other: Option<ArtifactRow>) -> RefEntry {
    RefEntry {
        path: path.to_string(),
        line: row.line,
        ref_type: row.ref_type.clone(),
        from: row.from.clone(),
        to_kind: row.to_kind.as_str().to_string(),
        to_name: row.to_name.clone(),
        to_id: row.to_id.clone(),
        resolved: row.resolved,
        other,
        calls: 0,
        observed_only: false,
        operation: row.operation.clone(),
        origin: row.origin.clone(),
        origin_hash: row.origin_hash.clone(),
    }
}

fn extension(from: &ArtifactRow, path: &str, row: &RefRow) -> Extension {
    Extension {
        component: from.component.clone(),
        kind: from.kind.clone(),
        path: path.to_string(),
        line: row.line,
        from: row.from.clone(),
        ref_type: row.ref_type.clone(),
        target: row.to_id.clone().unwrap_or_else(|| row.to_name.clone()),
        scope: row.to_scope.clone(),
        resolved: row.resolved,
    }
}

/// `candidates` that match `text`, best first, at most `limit`: an exact id or name first, then by
/// [`extract::match_score`] on the short name, then on the full name.
fn rank(candidates: Vec<ArtifactRow>, text: &str, limit: usize) -> Vec<ArtifactRow> {
    let text = text.trim();
    let query = extract::query_segments(text);
    let mut scored: Vec<(u32, ArtifactRow)> = candidates
        .into_iter()
        .filter_map(|artifact| {
            // a full name; a bare one (a subscreens item's, a transition's) is only a short name, so the kinds decide
            if artifact.id == text || (artifact.name == text && artifact.name != artifact.short_name) {
                return Some((0, artifact));
            }
            if artifact.short_name == text {
                return Some((1, artifact));
            }
            if query.is_empty() {
                return Some((1000, artifact));
            }
            let score = extract::match_score(&extract::search_key(&artifact.short_name), &query)
                .map(|score| score + 2)
                .or_else(|| {
                    extract::match_score(&extract::search_key(&artifact.name), &query).map(|score| score + 20)
                })?;
            Some((score, artifact))
        })
        .collect();
    scored.sort_by(|(a, left), (b, right)| {
        a.cmp(b)
            .then_with(|| kind_order(&left.kind).cmp(&kind_order(&right.kind)))
            .then_with(|| left.name.len().cmp(&right.name.len()))
            .then_with(|| left.name.cmp(&right.name))
    });
    scored.into_iter().take(limit).map(|(_, artifact)| artifact).collect()
}

/// The order of kinds among results of the same score: definitions people look for first.
fn kind_order(kind: &str) -> u8 {
    match kind {
        "entity" | "view-entity" => 0,
        "service" => 1,
        "screen" => 2,
        "transition" | "form" => 3,
        "field" | "relationship" | "index" | "view-member" | "form-field" => 5,
        "anchor" => 6,
        // a parameter's short name is a field's too often (`orderId`): it comes after every other kind but records
        "parameter" => 7,
        "record" => 8,
        _ => 4,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The rows of a server's index.

/// The factory's index state: the ids known, for resolving without asking the database, and whether it was built.
#[derive(Default)]
struct IndexState {
    resolver: Mutex<Resolver>,
    built: AtomicBool,
    /// One build or update at a time.
    running: Mutex<()>,
    listening: AtomicBool,
    /// The stamp of the rows, made at the first read after they changed.
    stamp: Mutex<Option<IndexStamp>>,
    /// Moved on each time the rows change, so a stamp made of rows read before a change is not kept.
    generation: AtomicU64,
    /// The entity facade's screen configuration generation ([`moqui_core::entity::screen_config_generation`]) when the
    /// `SubscreensItem` rows were last read: when it moves, they are read again ([`source_changes`]).
    screen_rows: AtomicU64,
    /// A conf reload changed the screens while a build or update held the lock: the conf's mounts are read before it
    /// lets go, or at the next use.
    conf_pending: AtomicBool,
    /// How many builds ran ([`builds`]).
    builds: AtomicU64,
    /// When a refresh of the sources last logged its failure, in epoch milliseconds: at most once a minute.
    logged_at: AtomicI64,
    /// The services and forms whose expanded rows were not all written: written again at the next update.
    retry: Mutex<HashSet<String>>,
}

impl IndexState {
    /// The rows changed (or are changing): the stamp is made again at the next read.
    fn rows_changed(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *lock(&self.stamp) = None;
    }
}

/// [`IndexState::rows_changed`] when dropped, so a write's end marks the change also when it returns early.
struct RowsChanged<'a>(&'a IndexState);

impl Drop for RowsChanged<'_> {
    fn drop(&mut self) {
        self.0.rows_changed();
    }
}

fn state(factory: &ExecutionContextFactory) -> Arc<IndexState> {
    factory.extension(|_| IndexState::default())
}

/// Whether the index keeps rows on this instance: a dev instance with a database.
#[must_use]
pub fn enabled(factory: &ExecutionContextFactory) -> bool {
    moqui_core::devtools::is_dev_instance()
        && factory.entity().is_some_and(|facade| {
            !facade.is_model_only() && moqui_core::entity::info::is_entity_defined(&facade, ARTIFACT)
        })
}

/// Build the index in the background, when this is a dev instance with a database (a dev server's start).
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if !enabled(factory) {
        return;
    }
    listen(factory);
    calls::start(factory);
    // the factory's shutdown waits for the build, which stops between batches, before the database stops
    let weak = Arc::downgrade(factory);
    let spawned = factory.spawn_background("artifact-index", move || {
        let Some(factory) = weak.upgrade() else {
            return;
        };
        if let Err(err) = build(&factory) {
            tracing::error!("Artifact index: the build failed: {err}");
        }
    });
    if let Err(err) = spawned {
        tracing::error!("Artifact index: could not start the build: {err}");
    }
}

/// Keep the index current with each reload, once per factory: the files a reload read, and the conf's screen mounts
/// after each conf reload that changed the screens (every way the conf is reloaded: the watcher, `reload#Conf`,
/// System › Conf, a pull, a component load, SIGHUP).
fn listen(factory: &Arc<ExecutionContextFactory>) {
    if state(factory).listening.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(factory);
    moqui_core::context::conf_reload::on_apply(
        factory,
        moqui_core::context::conf_reload::catalog::Group::Screens,
        move |_| {
            let Some(factory) = weak.upgrade() else {
                return;
            };
            if factory.is_shutting_down() {
                return;
            }
            // never waits on a build or update, which would hold the reload up: one that holds the lock reads them
            state(&factory).conf_pending.store(true, Ordering::SeqCst);
            refresh(&factory);
        },
    );
    super::reload::on_reload(factory, |factory, report| {
        if factory.is_shutting_down() || !state(factory).built.load(Ordering::SeqCst) {
            return;
        }
        let paths: Vec<PathBuf> = report.files.iter().map(|file| PathBuf::from(&file.path)).collect();
        if let Err(err) = update_files(factory, &paths) {
            tracing::error!("Artifact index: the update after a reload failed: {err}");
        }
    });
}

/// Build the index now unless it was built, as a query's first use does.
///
/// # Errors
///
/// Returns an error when the index is not kept here or its rows cannot be read at all.
pub fn ensure(factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
    if !enabled(factory) {
        return Err(Error::new(
            "the artifact index is kept only on a dev instance with a database (instance_purpose dev)",
        ));
    }
    if state(factory).built.load(Ordering::SeqCst) {
        // a conf reload left for later, or a SubscreensItem row written since the rows were read: read again before
        // the query, unless a build or update has the lock (it reads them before it lets go)
        if state(factory).conf_pending.load(Ordering::SeqCst) || screen_rows_moved(factory) {
            refresh(factory);
        }
        return Ok(());
    }
    listen(factory);
    calls::start(factory);
    build(factory).map(|_| ())
}

/// Remove every row of the index, so the next build reads every file.
///
/// The deletes commit in a transaction of their own. The build writes its rows in transactions of their own too
/// (`apply`), so deletes left in a caller's transaction (`rebuild#ArtifactIndex` runs in one) would hold the rows that
/// build writes again, and its inserts would wait on the caller's transaction, which waits on the build.
///
/// # Errors
///
/// Returns an error when the rows cannot be removed.
pub fn clear(factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
    let state = state(factory);
    let _running = lock(&state.running);
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    facade
        .transactions_for(ARTIFACT)?
        .run(TransactionAttribute::ForceNew, None, || {
            for entity in [REF, BINDING, ARTIFACT, FILE] {
                facade.find(entity)?.disable_authz().delete_all()?;
            }
            Ok(())
        })?;
    *lock(&state.resolver) = Resolver::default();
    state.rows_changed();
    Ok(())
}

/// Build the index now (with `full`, after removing every row, so every file is read), and keep it current from then.
///
/// # Errors
///
/// Returns an error when the index is not kept here or its rows cannot be read at all.
pub fn rebuild(factory: &Arc<ExecutionContextFactory>, full: bool) -> Result<BuildReport, Error> {
    if !enabled(factory) {
        return Err(Error::new(
            "the artifact index is kept only on a dev instance with a database (instance_purpose dev)",
        ));
    }
    if full {
        clear(factory)?;
    }
    listen(factory);
    calls::start(factory);
    build(factory)
}

/// What a build or an update did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildReport {
    pub files: usize,
    pub read: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub stale: usize,
    pub artifacts: usize,
    pub refs: usize,
    pub bindings: usize,
    /// Reading and parsing the files.
    pub read_millis: u128,
    /// Writing the rows.
    pub write_millis: u128,
    pub millis: u128,
}

/// Read every component file whose hash changed since the rows were written, and remove the rows of files gone.
///
/// # Errors
///
/// Returns an error when the rows cannot be read at all; a file that fails is logged and the others go on.
pub fn build(factory: &Arc<ExecutionContextFactory>) -> Result<BuildReport, Error> {
    build_files(factory, component_files(factory), true)
}

/// [`build`] over `files` (with `rows`, the database's row artifacts too); the rows of any other file are removed.
///
/// # Errors
///
/// Returns an error when the rows cannot be read at all.
pub fn build_files(
    factory: &Arc<ExecutionContextFactory>,
    files: Vec<Located>,
    rows: bool,
) -> Result<BuildReport, Error> {
    let state = state(factory);
    let _running = lock(&state.running);
    state.builds.fetch_add(1, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    // before the SubscreensItem rows are read: a write while they are read moves it again
    let screen_rows = moqui_core::entity::screen_config_generation(&facade);
    // what the rows hold: each file's hash, and the ids for resolving
    let mut hashes: HashMap<String, String> = HashMap::new();
    for row in facade
        .find(FILE)?
        .select_field("filePath")
        .select_field("fileHash")
        .list()?
        .into_values()
    {
        hashes.insert(text(&row, "filePath"), text(&row, "fileHash"));
    }
    {
        let mut resolver = Resolver::default();
        // a field's summary marks its primary key, and the derived artifacts are known by their origin and parent
        let list = facade
            .find(ARTIFACT)?
            .select_field("artifactId")
            .select_field("artifactKind")
            .select_field("artifactName")
            .select_field("shortName")
            .select_field("summary")
            .select_field("parentArtifactId")
            .select_field("origin")
            .select_field("marks")
            .select_field("filePath")
            .list()?;
        for row in list.into_values() {
            resolver.add(&artifact_of(&row));
        }
        *lock(&state.resolver) = resolver;
    }
    let mut report = BuildReport {
        files: files.len(),
        ..BuildReport::default()
    };
    let mut changes = Vec::new();
    let mut on_disk: HashSet<String> = HashSet::new();
    for file in &files {
        if factory.is_shutting_down() {
            tracing::info!("Artifact index: the build stopped for the shutdown; the next build reads the files left");
            return Ok(report);
        }
        let path = file.path.display().to_string();
        on_disk.insert(path.clone());
        let Ok(text) = std::fs::read_to_string(&file.path) else {
            continue;
        };
        if hashes.get(&path).is_some_and(|hash| *hash == hash_of(&text)) {
            report.unchanged += 1;
            continue;
        }
        changes.push(change_of(factory, file, &text));
    }
    // the artifacts that are rows: service jobs, data documents, email templates, DbForm forms, subscreens items; and
    // the conf's screen mounts
    let mut sources = Vec::new();
    if rows {
        sources = rows::read(&facade);
        let mounts = rows::conf_mounts(&moqui_core::screen::ScreenFacade::of(factory).conf().screens);
        on_disk.insert(mounts.path.clone());
        sources.push(mounts);
    }
    for source in sources {
        if hashes.get(&source.path).is_some_and(|hash| *hash == source.hash) {
            report.unchanged += 1;
            continue;
        }
        changes.push(source_change(source));
    }
    for path in hashes
        .keys()
        .filter(|path| !path.starts_with("db:") && *path != extract::CONF_PATH && !on_disk.contains(*path))
    {
        changes.push(Change::Removed(path.clone()));
    }
    report.read_millis = started.elapsed().as_millis();
    apply(factory, &facade, &state, changes, &mut report)?;
    if factory.is_shutting_down() {
        // not marked built: what the build did not write is read again at the next start, by its file's hash
        return Ok(report);
    }
    state.screen_rows.store(screen_rows, Ordering::SeqCst);
    state.built.store(true, Ordering::SeqCst);
    report.millis = started.elapsed().as_millis();
    tracing::info!(
        "Artifact index built in {} ms: {} files ({} read, {} unchanged, {} removed, {} stale), {} artifacts, {} references and {} bindings written",
        report.millis,
        report.files,
        report.read,
        report.unchanged,
        report.removed,
        report.stale,
        report.artifacts,
        report.refs,
        report.bindings
    );
    refresh_locked(factory, &facade, &state);
    Ok(report)
}

/// How many builds ran on this factory since it started (a full one, or one reading only the files changed).
#[must_use]
pub fn builds(factory: &ExecutionContextFactory) -> u64 {
    state(factory).builds.load(Ordering::SeqCst)
}

/// Read `paths` again (each reload's files), replacing their rows; a path gone removes its rows.
///
/// # Errors
///
/// Returns an error when the rows cannot be read at all.
pub fn update_files(factory: &Arc<ExecutionContextFactory>, paths: &[PathBuf]) -> Result<BuildReport, Error> {
    let state = state(factory);
    let _running = lock(&state.running);
    let started = std::time::Instant::now();
    // the index's own bookkeeping, in a context of its own whoever saved the file
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let mut changes = Vec::new();
    for path in paths {
        if path.is_file() {
            let Some(file) = locate(factory, path) else {
                continue;
            };
            changes.extend(read_change(factory, &file));
        } else {
            changes.push(Change::Removed(path.display().to_string()));
        }
    }
    let mut report = BuildReport {
        files: changes.len(),
        ..BuildReport::default()
    };
    apply(factory, &facade, &state, changes, &mut report)?;
    refresh_locked(factory, &facade, &state);
    report.millis = started.elapsed().as_millis();
    Ok(report)
}

/// Read `files` again, wherever they are, replacing their rows: [`update_files`] for files of no loaded component.
///
/// # Errors
///
/// Returns an error when the rows cannot be read at all.
pub fn update_located(factory: &Arc<ExecutionContextFactory>, files: &[Located]) -> Result<BuildReport, Error> {
    let state = state(factory);
    let _running = lock(&state.running);
    let started = std::time::Instant::now();
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let changes: Vec<Change> = files.iter().filter_map(|file| read_change(factory, file)).collect();
    let mut report = BuildReport {
        files: changes.len(),
        ..BuildReport::default()
    };
    apply(factory, &facade, &state, changes, &mut report)?;
    refresh_locked(factory, &facade, &state);
    report.millis = started.elapsed().as_millis();
    Ok(report)
}

/// Read again the sources that changed when no build or update holds the index's lock (a conf reload's hook, a query):
/// one that holds it reads them before it lets go ([`refresh_locked`]), so nothing waits behind it.
fn refresh(factory: &Arc<ExecutionContextFactory>) {
    let state = state(factory);
    let Some(_running) = try_lock(&state.running) else {
        return;
    };
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let Some(facade) = factory.entity() else {
        return;
    };
    refresh_locked(factory, &facade, &state);
}

/// With the index's lock held: read again the conf's mounts when a conf reload is pending, and the `SubscreensItem`
/// rows when one was written; what fails is left to be tried again at the next use, its failure logged at most once a
/// minute.
fn refresh_locked(factory: &ExecutionContextFactory, facade: &EntityFacade, state: &IndexState) {
    let conf = state.conf_pending.swap(false, Ordering::SeqCst);
    if let Err(err) = update_sources(factory, facade, state, conf) {
        if conf {
            state.conf_pending.store(true, Ordering::SeqCst);
        }
        log_throttled(
            state,
            &format!("Artifact index: the update of the conf mounts and SubscreensItem rows failed: {err}"),
        );
    }
}

/// With the index's lock held: replace the rows of the sources that changed ([`source_changes`]; with `conf`, the
/// conf's screen mounts too); a mount added, moved or removed walks the screen paths again (`apply`). Nothing before
/// the first build, which reads them all.
fn update_sources(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    conf: bool,
) -> Result<BuildReport, Error> {
    if !state.built.load(Ordering::SeqCst) || factory.is_shutting_down() {
        return Ok(BuildReport::default());
    }
    let started = std::time::Instant::now();
    let (changes, taken) = source_changes(factory, facade, state, conf);
    let mut report = BuildReport {
        files: changes.len(),
        ..BuildReport::default()
    };
    if let Err(err) = apply(factory, facade, state, changes, &mut report) {
        restore_screen_rows(state, taken);
        return Err(err);
    }
    report.millis = started.elapsed().as_millis();
    Ok(report)
}

/// Log `message` as an error unless one was logged in the last minute.
fn log_throttled(state: &IndexState, message: &str) {
    let now = now_millis();
    let last = state.logged_at.load(Ordering::SeqCst);
    if now - last >= 60_000
        && state
            .logged_at
            .compare_exchange(last, now, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        tracing::error!("{message}");
    }
}

/// Whether a `SubscreensItem` row was written since the index read them (a check of a counter, no lock).
fn screen_rows_moved(factory: &ExecutionContextFactory) -> bool {
    factory.entity().is_some_and(|facade| {
        moqui_core::entity::screen_config_generation(&facade) != state(factory).screen_rows.load(Ordering::SeqCst)
    })
}

/// The generation a refresh took in place of the one before it (old, new): put back when the refresh failed, so the
/// next use tries again (not when another write moved it meanwhile).
type Taken = Option<(u64, u64)>;

fn restore_screen_rows(state: &IndexState, taken: Taken) {
    if let Some((old, new)) = taken {
        let _ = state
            .screen_rows
            .compare_exchange(new, old, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// The sources whose rows are no longer what the index holds: with `conf`, the conf's screen mounts now
/// ([`rows::conf_mounts`], read after a conf reload), and the `SubscreensItem` rows when one was written since they
/// were read, with the generation taken for them. Each is compared with the hash of its pseudo file's row, so one that
/// did not change writes nothing.
fn source_changes(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    conf: bool,
) -> (Vec<Change>, Taken) {
    let mut sources = Vec::new();
    if conf {
        sources.push(rows::conf_mounts(
            &moqui_core::screen::ScreenFacade::of(factory).conf().screens,
        ));
    }
    // taken before the rows are read: a write while they are read moves it again
    let generation = moqui_core::entity::screen_config_generation(facade);
    let old = state.screen_rows.swap(generation, Ordering::SeqCst);
    let mut taken = None;
    if old != generation {
        match rows::read_entity(facade, rows::SUBSCREENS_ITEM) {
            Ok(source) => {
                sources.push(source);
                taken = Some((old, generation));
            }
            Err(err) => {
                restore_screen_rows(state, Some((old, generation)));
                log_throttled(
                    state,
                    &format!("Artifact index: the SubscreensItem rows were not read: {err}"),
                );
            }
        }
    }
    let mut changes = Vec::new();
    for source in sources {
        let stored = match stored_hash(facade, &source.path) {
            Ok(hash) => hash,
            Err(err) => {
                // written again: its rows are replaced whole, so a write of the same rows changes nothing
                tracing::error!("Artifact index: the hash of {} was not read: {err}", source.path);
                None
            }
        };
        if stored.as_deref() != Some(source.hash.as_str()) {
            changes.push(source_change(source));
        }
    }
    (changes, taken)
}

/// The hash the rows hold for the file or pseudo file `path`; none when it has no row.
fn stored_hash(facade: &EntityFacade, path: &str) -> Result<Option<String>, Error> {
    Ok(facade
        .find(FILE)?
        .eq("filePath", Value::string(path))
        .select_field("fileHash")
        .list()?
        .values()
        .first()
        .map(|row| text(row, "fileHash")))
}

/// The change that writes the rows of `source`, a pseudo file of no component.
fn source_change(source: rows::RowSource) -> Change {
    Change::Read {
        file: Located {
            path: PathBuf::from(&source.path),
            component: String::new(),
            location: source.path,
            kind: ArtifactKind::Other,
        },
        hash: source.hash,
        index: source.index,
    }
}

fn read_change(factory: &ExecutionContextFactory, file: &Located) -> Option<Change> {
    match std::fs::read_to_string(&file.path) {
        Ok(text) => Some(change_of(factory, file, &text)),
        Err(err) => {
            tracing::error!("Artifact index: {} not read: {err}", file.path.display());
            None
        }
    }
}

enum Change {
    Read {
        file: Located,
        hash: String,
        index: FileIndex,
    },
    Stale {
        file: Located,
        hash: String,
        problem: String,
    },
    Removed(String),
}

fn change_of(factory: &ExecutionContextFactory, file: &Located, text: &str) -> Change {
    let hash = hash_of(text);
    match file.extract_text(factory, text) {
        Ok(index) => Change::Read {
            file: file.clone(),
            hash,
            index,
        },
        Err(problem) => {
            tracing::error!(
                "Artifact index: {} does not parse, its rows are kept as stale: {problem}",
                file.path.display()
            );
            Change::Stale {
                file: file.clone(),
                hash,
                problem,
            }
        }
    }
}

/// Write `changes`: forget the old artifacts of each file and know the new ones, resolve the new references against
/// all of them, replace each file's rows in a transaction of its own, then mark the references to artifacts now gone
/// unresolved and resolve those an added artifact answers.
fn apply(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    mut changes: Vec<Change>,
    report: &mut BuildReport,
) -> Result<(), Error> {
    if changes.is_empty() {
        return Ok(());
    }
    state.rows_changed();
    // again at the end, however it ends: a read while the rows were written may have made it of the old ones
    let _changed = RowsChanged(state);
    let mut gone: BTreeSet<String> = BTreeSet::new();
    let mut added: Vec<ArtifactRow> = Vec::new();
    let mut changed_paths: HashSet<String> = HashSet::new();
    // the entities whose fields changed, by id: their derived entity-auto services are derived again
    let mut changed_entities: HashSet<String> = HashSet::new();
    // the entities, services and forms whose declarations changed: the expansions that read them are made again
    let mut touched: HashSet<String> = HashSet::new();
    let rewrites: Vec<Rewrite>;
    let derived: Derived;
    // whether a screen, subscreens item or transition was added, changed or removed: every screen path is walked again
    let screens_changed: bool;
    // ids another file defines too (an entity defined again): their row is replaced, the later definition winning
    let shared: HashSet<String>;
    // the files that have rows: only theirs are read and removed first
    let mut known: HashSet<String> = HashSet::new();
    {
        let paths: Vec<Value> = changes
            .iter()
            .map(|change| match change {
                Change::Read { file, .. } | Change::Stale { file, .. } => {
                    Value::string(file.path.display().to_string())
                }
                Change::Removed(path) => Value::string(path.as_str()),
            })
            .collect();
        for chunk in paths.chunks(500) {
            for row in facade
                .find(FILE)?
                .condition(Condition::is_in("filePath", chunk.to_vec()))
                .select_field("filePath")
                .list()?
                .values()
            {
                known.insert(text(row, "filePath"));
            }
        }
    }
    {
        // the screens, items and transitions before and after, by id and summary (an item's location, a transition's
        // services): a path goes elsewhere only when they differ
        let walked = |artifact: &ArtifactRow| {
            matches!(artifact.kind.as_str(), "screen" | "subscreen" | "transition")
                .then(|| (artifact.id.clone(), format!("{}|{}", artifact.summary, artifact.marks)))
        };
        let mut walked_before: HashSet<(String, String)> = HashSet::new();
        let mut walked_after: HashSet<(String, String)> = HashSet::new();
        let mut resolver = lock(&state.resolver);
        for change in &changes {
            let path = match change {
                Change::Read { file, .. } => file.path.display().to_string(),
                Change::Removed(path) => path.clone(),
                Change::Stale { .. } => continue,
            };
            changed_paths.insert(path.clone());
            if !known.contains(&path) {
                continue;
            }
            // one file's old rows that cannot be read leave its names in the resolver, not the update undone
            match old_artifacts(facade, &path) {
                Ok(old_rows) => {
                    for old in old_rows {
                        walked_before.extend(walked(&old));
                        touched.extend(declared_id(&old));
                        resolver.remove(&old);
                        if old.kind == "field" {
                            changed_entities.extend(old.parent.clone());
                        }
                        gone.insert(old.id);
                    }
                }
                Err(err) => tracing::error!("Artifact index: the old rows of {path} were not read: {err}"),
            }
        }
        for change in &changes {
            if let Change::Read { index, .. } = change {
                for artifact in &index.artifacts {
                    resolver.add(artifact);
                    walked_after.extend(walked(artifact));
                    touched.extend(declared_id(artifact));
                    if artifact.kind == "field" {
                        changed_entities.extend(artifact.parent.clone());
                    }
                    gone.remove(&artifact.id);
                    added.push(artifact.clone());
                }
            }
        }
        screens_changed = walked_before != walked_after;
        let waiting = waiting_refs(facade, &added, &changed_paths);
        derived = derive_in(&mut resolver, &changes, &waiting, &changed_entities, &added, &mut gone);
        added.extend(derived.rows.iter().cloned());
        touched.extend(derived.dropped.iter().cloned());
        touched.extend(
            derived
                .rows
                .iter()
                .filter(|row| row.kind == "service")
                .map(|row| row.id.clone()),
        );
        // an entity by its short name too, as a reader names it once the entity is gone; a screen-extend's form as the
        // base form it adds fields to
        let aliases: Vec<String> = touched
            .iter()
            .filter_map(|id| {
                if let Some(entity) = id.strip_prefix("entity:") {
                    let short = entity.rsplit('.').next().unwrap_or(entity);
                    return (short != entity).then(|| format!("entity:{short}"));
                }
                let (location, name) = id.strip_prefix("form:")?.rsplit_once('#')?;
                location
                    .contains("/screen-extend/")
                    .then(|| format!("form:{}#{name}", extract::extended_location(location)))
            })
            .collect();
        touched.extend(aliases);
        let retry = std::mem::take(&mut *lock(&state.retry));
        let mut made = expand_in(&mut resolver, &mut changes, &touched, &retry, &mut added, &mut gone);
        // an id another file still defines is not gone
        gone.retain(|id| !resolver.has(id));
        shared = added
            .iter()
            .filter(|artifact| resolver.count(&artifact.id) > 1)
            .map(|artifact| artifact.id.clone())
            .collect();
        let exists = |location: &str| location_exists(factory, location);
        for change in &mut changes {
            if let Change::Read { index, .. } = change {
                for row in &mut index.refs {
                    resolver.resolve(row, &exists);
                }
                index.bindings = resolver.resolve_bindings(std::mem::take(&mut index.bindings));
            }
        }
        for rewrite in &mut made {
            rewrite.expansion.bindings = resolver.resolve_bindings(std::mem::take(&mut rewrite.expansion.bindings));
        }
        rewrites = made;
    }
    if factory.is_shutting_down() {
        tracing::info!(
            "Artifact index: the rows of {} files were not written for the shutdown",
            changes.len()
        );
        return Ok(());
    }
    let tx = facade.transactions_for(ARTIFACT)?;
    let writing = std::time::Instant::now();
    // before the files' rows: a file that now defines a derived service's id takes its row
    if let Err(err) = tx.run(TransactionAttribute::ForceNew, None, || write_derived(facade, &derived)) {
        tracing::error!("Artifact index: the derived entity-auto services were not written: {err}");
    }
    // each owner's expansion in a transaction of its own: one that fails is that owner's alone
    for rewrite in &rewrites {
        if let Err(err) = tx.run(TransactionAttribute::ForceNew, None, || write_expanded(facade, rewrite)) {
            // the rows are written without a transaction of their own: the next update writes all of them again
            lock(&state.retry).insert(rewrite.owner.clone());
            tracing::error!(
                "Artifact index: the expansion of {} was not written, it is written again at the next update: {err}",
                rewrite.owner
            );
        }
    }
    // files a hundred to a transaction; a batch that fails is written again file by file, so one file's failure is
    // that file's alone
    let write = |change: &Change| -> Result<(), Error> {
        match change {
            Change::Read { file, hash, index } => {
                let path = file.path.display().to_string();
                write_file(facade, file, hash, index, &shared, known.contains(&path))
            }
            Change::Stale { file, hash, problem } => mark_stale(facade, file, hash, problem),
            Change::Removed(path) => remove_file(facade, path),
        }
    };
    let run = |batch: &[Change]| -> Result<(), Error> {
        tx.run(TransactionAttribute::ForceNew, None, || {
            // the rows go without waiting for each result, as a data file's do, and are all in before the commit
            let _pipeline = moqui_core::entity::exec::PipelineWrites::enter();
            for change in batch {
                write(change)?;
            }
            tx.finish_deferred()
        })
    };
    let mut left = changes.len();
    for batch in changes.chunks(100) {
        // between batches, and after one that failed as the database went away: the shutdown waits for this thread
        // before it stops the database, and the next build reads the files left by their hashes
        if factory.is_shutting_down() {
            tracing::info!("Artifact index: the rows of {left} files were not written for the shutdown");
            return Ok(());
        }
        let outcomes: Vec<(&Change, Result<(), Error>)> = match run(batch) {
            Ok(()) => batch.iter().map(|change| (change, Ok(()))).collect(),
            Err(_) if factory.is_shutting_down() => {
                tracing::info!("Artifact index: the rows of {left} files were not written for the shutdown");
                return Ok(());
            }
            Err(_) if batch.len() > 1 => batch
                .iter()
                .map(|change| (change, run(std::slice::from_ref(change))))
                .collect(),
            Err(err) => vec![(&batch[0], Err(err))],
        };
        for (change, outcome) in outcomes {
            match (outcome, change) {
                (Ok(()), Change::Read { index, .. }) => {
                    report.read += 1;
                    report.artifacts += index.artifacts.len();
                    report.refs += index.refs.len();
                    report.bindings += index.bindings.len();
                }
                (Ok(()), Change::Stale { .. }) => report.stale += 1,
                (Ok(()), Change::Removed(_)) => report.removed += 1,
                (Err(err), change) => {
                    let path = match change {
                        Change::Read { file, .. } | Change::Stale { file, .. } => file.path.display().to_string(),
                        Change::Removed(path) => path.clone(),
                    };
                    tracing::error!("Artifact index: the rows of {path} were not written: {err}");
                }
            }
        }
        left -= batch.len();
    }
    unresolve(facade, &tx, &gone, &changed_paths)?;
    resolve_added(factory, facade, state, &tx, &added, &changed_paths)?;
    // nothing to walk again when every file with rows was read now (a full build): each path was walked as it was read
    let read_now = changes
        .iter()
        .filter(|change| matches!(change, Change::Read { .. }))
        .count();
    if screens_changed && usize::try_from(facade.find(FILE)?.count()?).unwrap_or(0) > read_now {
        walk_again(factory, facade, state, &tx, &changed_paths)?;
    }
    report.write_millis = writing.elapsed().as_millis();
    Ok(())
}

/// The id an artifact's declaration counts toward for the expansions that read it ([`Resolver::dependents`]): an
/// entity, service or form itself, the entity of a field, the service of a parameter, the form of a form field. None for
/// a row expanded itself.
fn declared_id(artifact: &ArtifactRow) -> Option<String> {
    if extract::is_expanded(&artifact.origin) {
        return None;
    }
    match artifact.kind.as_str() {
        "entity" | "view-entity" | "service" | "form" => Some(artifact.id.clone()),
        "field" | "parameter" | "form-field" => artifact.parent.clone(),
        _ => None,
    }
}

/// The expansion of a service or form of a file not read now, written in place of its rows.
struct Rewrite {
    owner: String,
    path: String,
    expansion: resolve::Expansion,
    /// The files its rows were in before (another, when the definition the expansions read moved to another file).
    before: BTreeSet<String>,
}

/// The expansions an update makes ([`Resolver::expand`]): of each service and form the files read now define, added to
/// their files' rows; and of each one elsewhere that reads what changed (`touched`, [`Resolver::dependents`]) and
/// expands to other rows than before, answered to be written in place of its rows ([`write_expanded`]). The resolver
/// knows the new rows, and the ids no more are `gone`.
fn expand_in(
    resolver: &mut Resolver,
    changes: &mut [Change],
    touched: &HashSet<String>,
    retry: &HashSet<String>,
    added: &mut Vec<ArtifactRow>,
    gone: &mut BTreeSet<String>,
) -> Vec<Rewrite> {
    let mut read_owners: HashSet<String> = HashSet::new();
    for change in changes.iter_mut() {
        let Change::Read { file, index, .. } = change else {
            continue;
        };
        let path = file.path.display().to_string();
        // of a service defined in two files, the expansion is the definition's the expansions read, in its file
        let owners: Vec<String> = index
            .artifacts
            .iter()
            .filter(|artifact| resolver.is_expandable(&artifact.id))
            .filter(|artifact| resolver.owner_path(&artifact.id).as_deref() == Some(path.as_str()))
            .map(|artifact| artifact.id.clone())
            .collect();
        for owner in owners {
            let expansion = resolver.expand(&owner);
            for artifact in &expansion.artifacts {
                resolver.add(artifact);
                gone.remove(&artifact.id);
                added.push(artifact.clone());
            }
            for (id, summary) in &expansion.summaries {
                for artifact in index.artifacts.iter_mut().filter(|artifact| artifact.id == *id) {
                    artifact.summary.clone_from(summary);
                }
                resolver.set_stored_summary(id, summary);
            }
            index.artifacts.extend(expansion.artifacts);
            index.bindings.extend(expansion.bindings);
            read_owners.insert(owner);
        }
    }
    if touched.is_empty() && retry.is_empty() {
        return Vec::new();
    }
    // those that read what changed, those whose own definition changed elsewhere (an override gone), and those whose
    // rows were not all written last time
    let mut candidates: HashSet<String> = resolver.dependents(touched);
    candidates.extend(touched.iter().filter(|id| resolver.is_expandable(id)).cloned());
    candidates.extend(retry.iter().cloned());
    let mut dependents: Vec<String> = candidates
        .into_iter()
        .filter(|owner| !read_owners.contains(owner))
        .collect();
    dependents.sort();
    let mut rewrites = Vec::new();
    for owner in dependents {
        let before = resolver.expanded_rows(&owner).to_vec();
        let expansion = resolver.expand(&owner);
        let same = before.len() == expansion.artifacts.len()
            && expansion.summaries.is_empty()
            && before.iter().all(|old| {
                expansion
                    .artifacts
                    .iter()
                    .any(|new| new.id == old.id && new.summary == old.summary && new.origin == old.origin)
            });
        if same && !retry.contains(&owner) {
            continue;
        }
        let Some(path) = resolver
            .owner_path(&owner)
            .or_else(|| before.first().map(|row| row.path.clone()))
        else {
            continue;
        };
        for old in &before {
            resolver.remove(old);
            gone.insert(old.id.clone());
        }
        for new in &expansion.artifacts {
            resolver.add(new);
            gone.remove(&new.id);
            added.push(new.clone());
        }
        for (id, summary) in &expansion.summaries {
            resolver.set_stored_summary(id, summary);
        }
        rewrites.push(Rewrite {
            owner,
            path,
            expansion,
            before: before.iter().map(|row| row.path.clone()).collect(),
        });
    }
    rewrites
}

/// Replace the expanded rows of a service or form of a file not read now ([`expand_in`]): its artifacts and bindings of
/// an expanded origin.
fn write_expanded(facade: &EntityFacade, rewrite: &Rewrite) -> Result<(), Error> {
    let origins: Vec<Value> = extract::EXPANDED_ORIGINS
        .iter()
        .map(|origin| Value::string(*origin))
        .collect();
    for path in rewrite.before.iter().filter(|path| **path != rewrite.path) {
        for (entity, owner_field) in [(ARTIFACT, "parentArtifactId"), (BINDING, "fromArtifactId")] {
            facade
                .find(entity)?
                .eq("filePath", Value::string(path))
                .eq(owner_field, Value::string(&rewrite.owner))
                .condition(Condition::is_in("origin", origins.clone()))
                .delete_all()?;
        }
    }
    // the declared parameters whose summaries the merge changed
    for (id, summary) in &rewrite.expansion.summaries {
        if let Some(mut row) = facade.find(ARTIFACT)?.eq("artifactId", Value::string(id)).one()? {
            row.set("summary", Value::string(summary));
            row.update(facade)?;
        }
    }
    facade
        .find(ARTIFACT)?
        .eq("filePath", Value::string(&rewrite.path))
        .eq("parentArtifactId", Value::string(&rewrite.owner))
        .condition(Condition::is_in("origin", origins.clone()))
        .delete_all()?;
    facade
        .find(BINDING)?
        .eq("filePath", Value::string(&rewrite.path))
        .eq("fromArtifactId", Value::string(&rewrite.owner))
        .condition(Condition::is_in("origin", origins))
        .delete_all()?;
    // another row of an id is replaced, as a file's later definition replaces it
    let ids: HashSet<String> = rewrite.expansion.artifacts.iter().map(|row| row.id.clone()).collect();
    write_artifacts(facade, &rewrite.path, &rewrite.expansion.artifacts, &ids)?;
    // sequence ids of the owner's own, apart from the file's numbered ones
    let tag: String = hash_of(&rewrite.owner).chars().take(8).collect();
    for (sequence, binding) in rewrite.expansion.bindings.iter().enumerate() {
        let mut row = facade.make_value(BINDING)?;
        row.set("filePath", Value::string(&rewrite.path));
        row.set("bindingSeqId", Value::string(format!("{tag}{sequence:04}")));
        fill_binding(&mut row, binding);
        row.create(facade)?;
    }
    Ok(())
}

/// The derived entity-auto services an update changes ([`Resolver::derive`]).
struct Derived {
    /// The services whose rows go: each one a file now defines, or whose entity's fields changed.
    dropped: Vec<String>,
    /// The rows derived again or anew.
    rows: Vec<ArtifactRow>,
}

/// The unresolved references of files not read now that call an entity-auto service of an entity `added` defines (by
/// its name, or a parameter by its service): written before the entity was known, they need its derived service. One
/// that cannot be read is logged and left for the next full build.
fn waiting_refs(facade: &EntityFacade, added: &[ArtifactRow], changed: &HashSet<String>) -> Vec<RefRow> {
    let names: HashSet<String> = added
        .iter()
        .filter(|artifact| artifact.kind == "entity")
        .flat_map(Resolver::entity_auto_names)
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    let unresolved = || facade.find(REF).map(|find| find.eq("resolved", Value::string("N")));
    let mut lists = Vec::new();
    if names.len() > 2000 {
        // many names (a build): every unresolved reference in one read
        lists.push(unresolved().and_then(|find| find.list()));
    } else {
        let values: Vec<Value> = names.iter().map(|name| Value::string(name.as_str())).collect();
        for chunk in values.chunks(500) {
            for field in ["toName", "toScope"] {
                lists
                    .push(unresolved().and_then(|find| find.condition(Condition::is_in(field, chunk.to_vec())).list()));
            }
        }
    }
    let mut found = Vec::new();
    for list in lists {
        match list {
            Ok(rows) => {
                for row in rows.into_values() {
                    let calls = names.contains(&text(&row, "toName")) || names.contains(&text(&row, "toScope"));
                    if calls && !changed.contains(&text(&row, "filePath")) {
                        found.push(ref_of(&row));
                    }
                }
            }
            Err(err) => tracing::error!("Artifact index: the references waiting for an entity were not read: {err}"),
        }
    }
    found
}

/// Forget the derived services a file now defines or whose entity's fields changed (`changed_entities`), and derive
/// what the references read (and those `waiting` in other files) and the dropped services need, with the entity's
/// fields now: the resolver knows them, and the ids that are no more are `gone`.
fn derive_in(
    resolver: &mut Resolver,
    changes: &[Change],
    waiting: &[RefRow],
    changed_entities: &HashSet<String>,
    added: &[ArtifactRow],
    gone: &mut BTreeSet<String>,
) -> Derived {
    let defined: HashSet<&str> = added.iter().map(|artifact| artifact.id.as_str()).collect();
    let mut dropped = Vec::new();
    for id in resolver.derived_ids() {
        let rows = resolver.derived_rows(&id);
        // by the id, not a row's parent: rows read back from the database come in no set order (a parameter's parent is
        // the service)
        let entity = id
            .split_once('#')
            .map(|(_, entity)| format!("entity:{entity}"))
            .unwrap_or_default();
        if defined.contains(id.as_str()) || changed_entities.contains(&entity) {
            for row in &rows {
                resolver.remove(row);
                gone.insert(row.id.clone());
            }
            dropped.push(id);
        }
    }
    let read = changes
        .iter()
        .filter_map(|change| match change {
            Change::Read { index, .. } => Some(index.refs.iter()),
            _ => None,
        })
        .flatten()
        .chain(waiting);
    let mut rows = resolver.derive(read);
    // a service a file no longer defines, its entity-auto name now derived in its stead
    let undefined: Vec<String> = gone.iter().filter(|id| id.starts_with("service:")).cloned().collect();
    for id in dropped.iter().chain(&undefined) {
        let again = resolver.derived_service(id);
        let entity_known = again
            .first()
            .and_then(|row| row.parent.as_deref())
            .is_some_and(|entity| resolver.has(entity));
        if entity_known && !resolver.has(id) && !rows.iter().any(|row| row.id == *id) {
            rows.extend(again);
        }
    }
    for row in &rows {
        resolver.add(row);
        gone.remove(&row.id);
    }
    Derived { dropped, rows }
}

/// Replace the rows of the dropped and derived services in [`extract::DERIVED_PATH`].
fn write_derived(facade: &EntityFacade, derived: &Derived) -> Result<(), Error> {
    let services = derived.dropped.iter().cloned().chain(
        derived
            .rows
            .iter()
            .filter(|row| row.kind == "service")
            .map(|row| row.id.clone()),
    );
    for id in services {
        for field in ["artifactId", "parentArtifactId"] {
            facade
                .find(ARTIFACT)?
                .eq("filePath", Value::string(extract::DERIVED_PATH))
                .eq(field, Value::string(&id))
                .delete_all()?;
        }
    }
    // no file defines a derived id now: a row of one left by a file not yet written again (a file removed) goes first
    let ids: HashSet<String> = derived.rows.iter().map(|row| row.id.clone()).collect();
    write_artifacts(facade, extract::DERIVED_PATH, &derived.rows, &ids)
}

fn old_artifacts(facade: &EntityFacade, path: &str) -> Result<Vec<ArtifactRow>, Error> {
    Ok(facade
        .find(ARTIFACT)?
        .eq("filePath", Value::string(path))
        .list()?
        .into_values()
        .iter()
        .map(artifact_of)
        .collect())
}

fn write_file(
    facade: &EntityFacade,
    file: &Located,
    hash: &str,
    index: &FileIndex,
    shared: &HashSet<String>,
    known: bool,
) -> Result<(), Error> {
    let path = file.path.display().to_string();
    if known {
        facade
            .find(ARTIFACT)?
            .eq("filePath", Value::string(&path))
            .delete_all()?;
        facade.find(REF)?.eq("filePath", Value::string(&path)).delete_all()?;
        facade
            .find(BINDING)?
            .eq("filePath", Value::string(&path))
            .delete_all()?;
    }
    write_artifacts(facade, &path, &index.artifacts, shared)?;
    for (sequence, reference) in index.refs.iter().enumerate() {
        let mut row = facade.make_value(REF)?;
        row.set("filePath", Value::string(&path));
        row.set("refSeqId", Value::string(format!("{sequence:05}")));
        fill_ref(&mut row, reference);
        row.create(facade)?;
    }
    for (sequence, binding) in index.bindings.iter().enumerate() {
        let mut row = facade.make_value(BINDING)?;
        row.set("filePath", Value::string(&path));
        row.set("bindingSeqId", Value::string(format!("{sequence:05}")));
        fill_binding(&mut row, binding);
        row.create(facade)?;
    }
    store_file(facade, file, hash, false, "", known)
}

/// Write the artifact rows of `path` (a file's, or the derived ones of [`extract::DERIVED_PATH`]).
fn write_artifacts(
    facade: &EntityFacade,
    path: &str,
    artifacts: &[ArtifactRow],
    shared: &HashSet<String>,
) -> Result<(), Error> {
    let mut seen = HashSet::new();
    for (sequence, artifact) in artifacts.iter().enumerate() {
        // a file naming one id twice (a field repeated) keeps the first
        if !seen.insert(artifact.id.as_str()) {
            continue;
        }
        let mut row = facade.make_value(ARTIFACT)?;
        row.set("artifactId", Value::string(&artifact.id));
        row.set("artifactKind", Value::string(&artifact.kind));
        row.set("artifactName", Value::string(&artifact.name));
        row.set("shortName", Value::string(&artifact.short_name));
        row.set("searchKey", Value::string(extract::search_key(&artifact.short_name)));
        row.set("component", Value::string(&artifact.component));
        row.set("filePath", Value::string(path));
        row.set("lineNumber", Value::I64(i64::from(artifact.line)));
        row.set("columnNumber", Value::I64(i64::from(artifact.column)));
        row.set("summary", Value::string(&artifact.summary));
        if let Some(parent) = &artifact.parent {
            row.set("parentArtifactId", Value::string(parent));
        }
        row.set("sequenceNum", Value::I64(i64::try_from(sequence).unwrap_or(0)));
        if !artifact.origin.is_empty() {
            row.set("origin", Value::string(&artifact.origin));
        }
        if !artifact.marks.is_empty() {
            row.set("marks", Value::string(&artifact.marks));
        }
        if shared.contains(&artifact.id) {
            facade
                .find(ARTIFACT)?
                .eq("artifactId", Value::string(&artifact.id))
                .delete_all()?;
        }
        row.create(facade)?;
    }
    Ok(())
}

fn fill_ref(row: &mut EntityValue, reference: &RefRow) {
    row.set("fromArtifactId", Value::string(&reference.from));
    row.set(
        "toArtifactId",
        reference.to_id.as_deref().map_or(Value::Null, Value::string),
    );
    row.set("toKind", Value::string(reference.to_kind.as_str()));
    row.set("toName", Value::string(&reference.to_name));
    row.set("toScope", Value::string(&reference.to_scope));
    row.set("refType", Value::string(&reference.ref_type));
    row.set("lineNumber", Value::I64(i64::from(reference.line)));
    row.set("columnNumber", Value::I64(i64::from(reference.column)));
    row.set("resolved", Value::string(if reference.resolved { "Y" } else { "N" }));
    row.set("operation", Value::string(&reference.operation));
    row.set("origin", Value::string(&reference.origin));
    row.set("originHash", Value::string(&reference.origin_hash));
}

fn fill_binding(row: &mut EntityValue, binding: &BindingRow) {
    row.set("fromArtifactId", Value::string(&binding.from));
    row.set("toId", Value::string(&binding.to_id));
    row.set("direction", Value::string(&binding.direction));
    row.set("sourceKind", Value::string(&binding.source_kind));
    row.set("sourceText", Value::string(&binding.source_text));
    row.set(
        "sourceId",
        binding.source_id.as_deref().map_or(Value::Null, Value::string),
    );
    row.set("toPath", binding.to_path.as_deref().map_or(Value::Null, Value::string));
    row.set("origin", Value::string(&binding.origin));
    row.set("originHash", Value::string(&binding.origin_hash));
    row.set("lineNumber", Value::I64(i64::from(binding.line)));
}

fn store_file(
    facade: &EntityFacade,
    file: &Located,
    hash: &str,
    stale: bool,
    problem: &str,
    known: bool,
) -> Result<(), Error> {
    let path = file.path.display().to_string();
    if known {
        facade.find(FILE)?.eq("filePath", Value::string(&path)).delete_all()?;
    }
    let mut row = facade.make_value(FILE)?;
    row.set("filePath", Value::string(&path));
    row.set("location", Value::string(&file.location));
    row.set("component", Value::string(&file.component));
    row.set("fileKind", Value::string(extract::file_kind(file.kind).unwrap_or("")));
    row.set("fileHash", Value::string(hash));
    row.set("indexedDate", moqui_core::user::now_value());
    row.set("stale", Value::string(if stale { "Y" } else { "N" }));
    if !problem.is_empty() {
        row.set("problem", Value::string(problem));
    }
    row.create(facade)
}

fn mark_stale(facade: &EntityFacade, file: &Located, hash: &str, problem: &str) -> Result<(), Error> {
    store_file(facade, file, hash, true, problem, true)
}

fn remove_file(facade: &EntityFacade, path: &str) -> Result<(), Error> {
    facade
        .find(ARTIFACT)?
        .eq("filePath", Value::string(path))
        .delete_all()?;
    facade.find(REF)?.eq("filePath", Value::string(path)).delete_all()?;
    facade.find(BINDING)?.eq("filePath", Value::string(path)).delete_all()?;
    facade.find(FILE)?.eq("filePath", Value::string(path)).delete_all()?;
    Ok(())
}

/// The references elsewhere to artifacts no file defines any more: unresolved now.
fn unresolve(
    facade: &EntityFacade,
    tx: &moqui_core::transaction::TransactionManager,
    gone: &BTreeSet<String>,
    changed: &HashSet<String>,
) -> Result<(), Error> {
    let ids: Vec<&String> = gone.iter().collect();
    for chunk in ids.chunks(500) {
        let values: Vec<Value> = chunk.iter().map(|id| Value::string(id.as_str())).collect();
        let rows = facade
            .find(REF)?
            .condition(Condition::is_in("toArtifactId", values))
            .list()?
            .into_values();
        let rows: Vec<EntityValue> = rows
            .into_iter()
            .filter(|row| !changed.contains(&text(row, "filePath")))
            .map(|mut row| {
                row.set("toArtifactId", Value::Null);
                row.set("resolved", Value::string("N"));
                row
            })
            .collect();
        update_rows(facade, tx, rows, "reference");
    }
    Ok(())
}

/// The unresolved references elsewhere that an added artifact may answer: resolved again.
fn resolve_added(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    tx: &moqui_core::transaction::TransactionManager,
    added: &[ArtifactRow],
    changed: &HashSet<String>,
) -> Result<(), Error> {
    if added.is_empty() {
        return Ok(());
    }
    let names = Resolver::names_of(added);
    // many names (a build): every unresolved reference in one read
    let lists: Vec<Vec<EntityValue>> = if names.len() > 2000 {
        vec![facade
            .find(REF)?
            .eq("resolved", Value::string("N"))
            .list()?
            .into_values()]
    } else {
        let mut lists = Vec::new();
        for chunk in names.chunks(500) {
            let values: Vec<Value> = chunk.iter().map(|name| Value::string(name.as_str())).collect();
            lists.push(
                facade
                    .find(REF)?
                    .eq("resolved", Value::string("N"))
                    .condition(Condition::is_in("toName", values))
                    .list()?
                    .into_values(),
            );
        }
        lists
    };
    let exists = |location: &str| location_exists(factory, location);
    let mut now_resolved = Vec::new();
    {
        let resolver = lock(&state.resolver);
        for row in lists.into_iter().flatten() {
            if changed.contains(&text(&row, "filePath")) {
                continue;
            }
            let mut reference = ref_of(&row);
            resolver.resolve(&mut reference, &exists);
            if reference.resolved {
                now_resolved.push((row, reference));
            }
        }
    }
    let rows = now_resolved
        .into_iter()
        .map(|(mut row, reference)| {
            row.set(
                "toArtifactId",
                reference.to_id.as_deref().map_or(Value::Null, Value::string),
            );
            row.set("resolved", Value::string("Y"));
            row
        })
        .collect();
    update_rows(facade, tx, rows, "reference");
    Ok(())
}

/// Update `rows` (each changed already) in one transaction; when that fails, each in a transaction of its own, so a
/// row that fails is logged and the others are written.
fn update_rows(
    facade: &EntityFacade,
    tx: &moqui_core::transaction::TransactionManager,
    rows: Vec<EntityValue>,
    what: &str,
) {
    if rows.is_empty() {
        return;
    }
    let all = tx.run(TransactionAttribute::ForceNew, None, || {
        for row in &rows {
            row.clone().update(facade)?;
        }
        Ok(())
    });
    match all {
        Ok(()) => return,
        Err(err) if rows.len() == 1 => {
            tracing::error!(
                "Artifact index: a {what} row of {} was not updated: {err}",
                text(&rows[0], "filePath")
            );
            return;
        }
        // written again row by row
        Err(_) => {}
    }
    for row in rows {
        if let Err(err) = tx.run(TransactionAttribute::ForceNew, None, || row.clone().update(facade)) {
            tracing::error!(
                "Artifact index: a {what} row of {} was not updated: {err}",
                text(&row, "filePath")
            );
        }
    }
}

/// Every screen path and directory subscreen reference of the files not read now walked again against the screens,
/// items and transitions known now, resolved or not, and every form field's binding into a transition by its path: a
/// renamed or removed item, an item's new location, an item over a directory subscreen or a transition's new service
/// changes where a path goes, though no name it is written with changed. One read of the references (the columns a
/// walk needs) and one of the bindings; each walk is lookups in memory, and only the rows whose target changed are
/// written.
fn walk_again(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    tx: &moqui_core::transaction::TransactionManager,
    changed: &HashSet<String>,
) -> Result<(), Error> {
    // by the kind's index: the directory subscreens are among the screen references
    let kinds = [ToKind::ScreenPath, ToKind::Screen].map(|kind| Value::string(kind.as_str()));
    let mut find = facade.find(REF)?.condition(Condition::is_in("toKind", kinds.to_vec()));
    for field in [
        "filePath",
        "refSeqId",
        "fromArtifactId",
        "toArtifactId",
        "toKind",
        "toName",
        "toScope",
        "resolved",
        "origin",
    ] {
        find = find.select_field(field);
    }
    let rows = find.list()?.into_values();
    // each form field's bindings by a path, by its file, form and written id (two forms of a screen may post a field of
    // a name to one path), in slot order
    let mut paths: BTreeMap<(String, String, String), Vec<(usize, EntityValue)>> = BTreeMap::new();
    for row in facade
        .find(BINDING)?
        .condition(Condition::is_not_null("toPath"))
        .list()?
        .into_values()
    {
        let file = text(&row, "filePath");
        if changed.contains(&file) {
            continue;
        }
        let binding = binding_of(&row).binding;
        if let Some((written, slot)) = resolve::path_seed(&binding) {
            paths
                .entry((file, binding.from, written))
                .or_default()
                .push((slot, row));
        }
    }
    let exists = |location: &str| location_exists(factory, location);
    let mut moved = Vec::new();
    let mut rebound = Vec::new();
    let mut unbound = Vec::new();
    let mut bound = Vec::new();
    {
        let resolver = lock(&state.resolver);
        for mut row in rows {
            if changed.contains(&text(&row, "filePath")) {
                continue;
            }
            let mut reference = ref_of(&row);
            if reference.to_kind == ToKind::Screen && reference.origin != extract::ORIGIN_DIRECTORY {
                continue;
            }
            let before = (reference.to_id.clone(), reference.resolved);
            resolver.resolve(&mut reference, &exists);
            if (reference.to_id.clone(), reference.resolved) != before {
                row.set(
                    "toArtifactId",
                    reference.to_id.as_deref().map_or(Value::Null, Value::string),
                );
                row.set("resolved", Value::string(if reference.resolved { "Y" } else { "N" }));
                moved.push(row);
            }
        }
        // a transition reached that calls other services: a binding for each, the slots added or removed
        for ((file, _, _), mut slots) in paths {
            slots.sort_by_key(|(slot, _)| *slot);
            let Some((_, first)) = slots.first() else {
                continue;
            };
            let first_seq = text(first, "bindingSeqId");
            let now = resolver.path_bindings(&binding_of(first).binding);
            let mut old = slots.into_iter().map(|(_, row)| row);
            for (at, binding) in now.iter().enumerate() {
                match old.next() {
                    Some(mut row) => {
                        if text(&row, "toId") != binding.to_id
                            || binding.to_path.as_deref() != Some(text(&row, "toPath").as_str())
                        {
                            row.set("toId", Value::string(&binding.to_id));
                            row.set("toPath", binding.to_path.as_deref().map_or(Value::Null, Value::string));
                            rebound.push(row);
                        }
                    }
                    None => {
                        let mut row = facade.make_value(BINDING)?;
                        row.set("filePath", Value::string(&file));
                        row.set("bindingSeqId", Value::string(format!("{first_seq}-{at}")));
                        fill_binding(&mut row, binding);
                        bound.push(row);
                    }
                }
            }
            unbound.extend(old);
        }
    }
    update_rows(facade, tx, moved, "reference");
    update_rows(facade, tx, rebound, "binding");
    if !unbound.is_empty() || !bound.is_empty() {
        let written = tx.run(TransactionAttribute::ForceNew, None, || {
            for row in &unbound {
                row.clone().delete(facade)?;
            }
            for row in &bound {
                row.clone().create(facade)?;
            }
            Ok(())
        });
        if let Err(err) = written {
            tracing::error!("Artifact index: the bindings of a transition's services were not written again: {err}");
        }
    }
    Ok(())
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn number(row: &EntityValue, field: &str) -> i64 {
    row.get(field)
        .map(moqui_lib::object::to_plain_string)
        .and_then(|text| text.parse::<f64>().ok())
        .map_or(0, |value| value as i64)
}

fn artifact_of(row: &EntityValue) -> ArtifactRow {
    let parent = text(row, "parentArtifactId");
    ArtifactRow {
        id: text(row, "artifactId"),
        kind: text(row, "artifactKind"),
        name: text(row, "artifactName"),
        short_name: text(row, "shortName"),
        component: text(row, "component"),
        path: text(row, "filePath"),
        line: u32::try_from(number(row, "lineNumber")).unwrap_or(0),
        column: u32::try_from(number(row, "columnNumber")).unwrap_or(0),
        summary: text(row, "summary"),
        parent: (!parent.is_empty()).then_some(parent),
        origin: text(row, "origin"),
        marks: text(row, "marks"),
    }
}

fn ref_of(row: &EntityValue) -> RefRow {
    let to_id = text(row, "toArtifactId");
    RefRow {
        from: text(row, "fromArtifactId"),
        to_kind: ToKind::parse(&text(row, "toKind")).unwrap_or(ToKind::Location),
        to_name: text(row, "toName"),
        to_scope: text(row, "toScope"),
        ref_type: text(row, "refType"),
        line: u32::try_from(number(row, "lineNumber")).unwrap_or(0),
        column: u32::try_from(number(row, "columnNumber")).unwrap_or(0),
        to_id: (!to_id.is_empty()).then_some(to_id),
        resolved: text(row, "resolved") == "Y",
        operation: text(row, "operation"),
        origin: text(row, "origin"),
        origin_hash: text(row, "originHash"),
    }
}

fn binding_of(row: &EntityValue) -> BindingEntry {
    let source_id = text(row, "sourceId");
    BindingEntry {
        path: text(row, "filePath"),
        binding: BindingRow {
            from: text(row, "fromArtifactId"),
            to_id: text(row, "toId"),
            direction: text(row, "direction"),
            source_kind: text(row, "sourceKind"),
            source_text: text(row, "sourceText"),
            source_id: (!source_id.is_empty()).then_some(source_id),
            to_path: Some(text(row, "toPath")).filter(|path| !path.is_empty()),
            origin: text(row, "origin"),
            origin_hash: text(row, "originHash"),
            line: u32::try_from(number(row, "lineNumber")).unwrap_or(0),
        },
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Queries over the rows (plans/design-dev-aide.md 7.1 API).

/// The artifacts whose names match `text`, best first: segment and camel case matching (`OrdHdr`, `place#Order`), an
/// exact id or name first. `kinds` and `components` narrow it when not empty.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn find(
    factory: &Arc<ExecutionContextFactory>,
    text_query: &str,
    kinds: &[String],
    components: &[String],
    limit: usize,
) -> Result<Vec<ArtifactRow>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let query = extract::query_segments(text_query);
    let narrow = |find: moqui_core::entity::EntityFind| -> moqui_core::entity::EntityFind {
        let mut find = find.disable_authz();
        if !kinds.is_empty() {
            find = find.condition(Condition::is_in(
                "artifactKind",
                kinds.iter().map(|kind| Value::string(kind.as_str())).collect(),
            ));
        }
        if !components.is_empty() {
            find = find.condition(Condition::is_in(
                "component",
                components
                    .iter()
                    .map(|component| Value::string(component.as_str()))
                    .collect(),
            ));
        }
        find
    };
    let exact = narrow(facade.find(ARTIFACT)?)
        .condition(Condition::or(vec![
            Condition::equals("artifactId", Value::string(text_query.trim())),
            Condition::and(vec![
                Condition::equals("shortName", Value::string(text_query.trim())),
                Condition::not_equals("artifactKind", Value::string("parameter")),
            ]),
        ]))
        .limit(50)
        .list()?;
    let mut candidates: Vec<ArtifactRow> = exact.values().iter().map(artifact_of).collect();
    // the parameters of that short name apart, so the many (`orderId`) do not take the places of the fields
    let parameters = narrow(facade.find(ARTIFACT)?)
        .eq("shortName", Value::string(text_query.trim()))
        .eq("artifactKind", Value::string("parameter"))
        .limit(50)
        .list()?;
    candidates.extend(parameters.values().iter().map(artifact_of));
    if let [head, tail @ ..] = query.as_slice() {
        // a key with a segment starting with each query segment's first letter, in order: the ranking does the rest
        let rest: String = tail
            .iter()
            .filter_map(|segment| segment.chars().next())
            .map(|first| format!("% {first}"))
            .collect();
        let first = head.chars().next().unwrap_or('%');
        let like = narrow(facade.find(ARTIFACT)?)
            .condition(Condition::or(vec![
                Condition::like("searchKey", Value::string(format!("{first}{rest}%"))),
                Condition::like("searchKey", Value::string(format!("% {first}{rest}%"))),
            ]))
            .limit(20_000)
            .list()?;
        candidates.extend(like.values().iter().map(artifact_of));
    }
    // a qualified name (mantle.order.OrderHeader, a location): its text in the full name
    let trimmed = text_query.trim();
    if trimmed.contains(['.', '/', ':']) {
        let like = narrow(facade.find(ARTIFACT)?)
            .condition(Condition::like("artifactName", Value::string(format!("%{trimmed}%"))))
            .limit(500)
            .list()?;
        candidates.extend(like.values().iter().map(artifact_of));
    }
    let mut seen = HashSet::new();
    candidates.retain(|artifact| seen.insert(artifact.id.clone()));
    Ok(rank(candidates, text_query, limit))
}

/// Every artifact of `kinds` (in `components` when not empty), by name, at most `limit`: the lists of the dev-aide's
/// Entities, Services and ECA rules.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn of_kinds(
    factory: &Arc<ExecutionContextFactory>,
    kinds: &[String],
    components: &[String],
    limit: usize,
) -> Result<Vec<ArtifactRow>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let mut find = facade.find(ARTIFACT)?.disable_authz().condition(Condition::is_in(
        "artifactKind",
        kinds.iter().map(|kind| Value::string(kind.as_str())).collect(),
    ));
    if !components.is_empty() {
        find = find.condition(Condition::is_in(
            "component",
            components
                .iter()
                .map(|component| Value::string(component.as_str()))
                .collect(),
        ));
    }
    Ok(find
        .order_by("artifactName", false)
        .limit(i64::try_from(limit).unwrap_or(i64::MAX))
        .list()?
        .values()
        .iter()
        .map(artifact_of)
        .collect())
}

/// The artifact `id`.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn describe(factory: &Arc<ExecutionContextFactory>, id: &str) -> Result<Option<ArtifactRow>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    Ok(facade
        .find(ARTIFACT)?
        .disable_authz()
        .eq("artifactId", Value::string(id))
        .one()?
        .as_ref()
        .map(artifact_of))
}

/// An artifact named by its id, or the best match of its name.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn named(factory: &Arc<ExecutionContextFactory>, name: &str) -> Result<Option<ArtifactRow>, Error> {
    if let Some(found) = describe(factory, name)? {
        return Ok(Some(found));
    }
    Ok(find(factory, name, &[], &[], 1)?.into_iter().next())
}

/// The artifacts whose parent is `id`: an entity's fields (its extensions' too), a screen's transitions, forms and
/// anchors, in file order.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn children(factory: &Arc<ExecutionContextFactory>, id: &str) -> Result<Vec<ArtifactRow>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    Ok(facade
        .find(ARTIFACT)?
        .disable_authz()
        .eq("parentArtifactId", Value::string(id))
        .order_by("filePath", false)
        .order_by("sequenceNum", false)
        .list()?
        .values()
        .iter()
        .map(artifact_of)
        .collect())
}

/// The artifacts the file `path` defines at its top (no parent), as [`MemoryIndex::in_file`].
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn in_file(factory: &Arc<ExecutionContextFactory>, path: &str) -> Result<Vec<ArtifactRow>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let path = canonical(Path::new(path)).display().to_string();
    Ok(facade
        .find(ARTIFACT)?
        .disable_authz()
        .eq("filePath", Value::string(path))
        .order_by("sequenceNum", false)
        .list()?
        .values()
        .iter()
        .map(artifact_of)
        .filter(|artifact| artifact.parent.is_none())
        .collect())
}

/// The artifacts `ids`, by id.
fn artifacts_by_id(facade: &EntityFacade, ids: &BTreeSet<String>) -> Result<HashMap<String, ArtifactRow>, Error> {
    let mut found = HashMap::new();
    let ids: Vec<&String> = ids.iter().collect();
    for chunk in ids.chunks(500) {
        let values: Vec<Value> = chunk.iter().map(|id| Value::string(id.as_str())).collect();
        for row in facade
            .find(ARTIFACT)?
            .disable_authz()
            .condition(Condition::is_in("artifactId", values))
            .list()?
            .values()
        {
            let artifact = artifact_of(row);
            found.insert(artifact.id.clone(), artifact);
        }
    }
    Ok(found)
}

/// What uses `id` (`In`) or what it uses (`Out`), from the files and the calls observed; for a screen or an entity
/// its transitions, forms, anchors and fields count as it. `kinds` narrows the other side's kind when not empty.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn refs(
    factory: &Arc<ExecutionContextFactory>,
    id: &str,
    direction: Direction,
    kinds: &[String],
) -> Result<Vec<RefEntry>, Error> {
    refs_of_ids(factory, &[id], direction, kinds)
}

/// The ids in an `IN` list of one query at most.
const IN_CHUNK: usize = 500;

/// [`refs`] of several artifacts at once, in a few queries for all of them (an autodoc page of many services): the
/// references of each, those from the files in file and line order.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn refs_of_ids(
    factory: &Arc<ExecutionContextFactory>,
    ids: &[&str],
    direction: Direction,
    kinds: &[String],
) -> Result<Vec<RefEntry>, Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let parameters = ids.iter().any(|id| with_parameters(id, kinds));
    let mut near_list: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    for chunk in ids.chunks(IN_CHUNK) {
        for child in facade
            .find(ARTIFACT)?
            .disable_authz()
            .condition(Condition::is_in(
                "parentArtifactId",
                chunk.iter().map(|id| Value::string(*id)).collect(),
            ))
            .select_field("artifactId")
            .select_field("artifactKind")
            .list()?
            .values()
        {
            // a data file's records refer to nothing and nothing refers to them: not asked for, one id each
            let kind = text(child, "artifactKind");
            if kind != "record" && (parameters || kind != "parameter") {
                near_list.push(text(child, "artifactId"));
            }
        }
    }
    let (near, far) = match direction {
        Direction::In => ("toArtifactId", "fromArtifactId"),
        Direction::Out => ("fromArtifactId", "toArtifactId"),
    };
    let mut rows: Vec<EntityValue> = Vec::new();
    for chunk in near_list.chunks(IN_CHUNK) {
        rows.extend(
            facade
                .find(REF)?
                .disable_authz()
                .condition(Condition::is_in(
                    near,
                    chunk.iter().map(|id| Value::string(id.as_str())).collect(),
                ))
                .order_by("filePath", false)
                .order_by("lineNumber", false)
                .list()?
                .values()
                .iter()
                .cloned(),
        );
    }
    if near_list.len() > IN_CHUNK {
        rows.sort_by(|a, b| {
            text(a, "filePath")
                .cmp(&text(b, "filePath"))
                .then(number(a, "lineNumber").cmp(&number(b, "lineNumber")))
        });
    }
    // a call is observed under a transition's method-less id: the near ids by that form, to put back the indexed id
    let mut near_ids: HashMap<String, String> = HashMap::new();
    for indexed in near_list {
        near_ids.entry(observed_form(&indexed)).or_insert(indexed);
    }
    let observed_ids: Vec<&String> = near_ids.keys().collect();
    let mut calls: Vec<EntityValue> = Vec::new();
    for chunk in observed_ids.chunks(IN_CHUNK) {
        calls.extend(
            facade
                .find(CALL)?
                .disable_authz()
                .condition(Condition::is_in(
                    near,
                    chunk.iter().map(|observed| Value::string(observed.as_str())).collect(),
                ))
                .list()?
                .values()
                .iter()
                .cloned(),
        );
    }
    let mut observed: Vec<(String, String, i64)> = Vec::new();
    for row in &calls {
        let near_id = text(row, near);
        let near_id = near_ids.get(&near_id).cloned().unwrap_or(near_id);
        let far_id = indexed_transition(&facade, &text(row, far))?;
        let (from, to) = match direction {
            Direction::In => (far_id, near_id),
            Direction::Out => (near_id, far_id),
        };
        observed.push((from, to, number(row, "callCount")));
    }
    let mut other_ids: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let other = text(row, far);
        if !other.is_empty() {
            other_ids.insert(other);
        }
    }
    for (from, to, _) in &observed {
        other_ids.insert(if direction == Direction::In {
            from.clone()
        } else {
            to.clone()
        });
    }
    let others = artifacts_by_id(&facade, &other_ids)?;
    let mut call_counts: HashMap<(String, String), i64> = HashMap::new();
    for (from, to, count) in observed {
        *call_counts.entry((from, to)).or_insert(0) += count;
    }
    let mut entries: Vec<RefEntry> = Vec::new();
    for row in &rows {
        let reference = ref_of(row);
        if direction == Direction::Out && !parameters && to_parameter(&reference) {
            continue;
        }
        let other_id = text(row, far);
        let other = others.get(&other_id).cloned();
        let mut item = entry(&text(row, "filePath"), &reference, other);
        if let Some(to) = &reference.to_id {
            item.calls = call_counts.remove(&(reference.from.clone(), to.clone())).unwrap_or(0);
        }
        entries.push(item);
    }
    // calls no file names: a name made at run time
    for ((from, to), count) in call_counts {
        let other_id = if direction == Direction::In { &from } else { &to };
        let other = others.get(other_id).cloned();
        entries.push(RefEntry {
            path: other.as_ref().map(|artifact| artifact.path.clone()).unwrap_or_default(),
            line: other.as_ref().map_or(0, |artifact| artifact.line),
            ref_type: "observed".into(),
            to_kind: to.split(':').next().unwrap_or("").to_string(),
            to_name: to.split_once(':').map(|(_, name)| name.to_string()).unwrap_or_default(),
            from,
            to_id: Some(to),
            resolved: true,
            other,
            calls: count,
            observed_only: true,
            operation: "call".into(),
            origin: "observed".into(),
            origin_hash: String::new(),
        });
    }
    if !kinds.is_empty() {
        entries.retain(|item| item.other.as_ref().is_some_and(|other| kinds.contains(&other.kind)));
    }
    Ok(entries)
}

/// The value bindings into a call's parameters: those `from` makes (a service or screen's call), those into `to` (a
/// parameter id, `parameter:service:<name>@in.<field>`), or both. Nothing when neither is given.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn bindings(
    factory: &Arc<ExecutionContextFactory>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<BindingEntry>, Error> {
    if from.is_none() && to.is_none() {
        return Ok(Vec::new());
    }
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let mut find = facade.find(BINDING)?.disable_authz();
    if let Some(from) = from {
        find = find.eq("fromArtifactId", Value::string(from));
    }
    if let Some(to) = to {
        find = find.eq("toId", Value::string(to));
    }
    let rows = find.order_by("filePath", false).order_by("lineNumber", false).list()?;
    Ok(rows.values().iter().map(binding_of).collect())
}

/// Whether refs of `id` take in its parameters' (the field-maps of each call): for a parameter, or when `kinds` asks
/// for them. Else a service's callers, or what a caller uses, are not listed once more for each field-map.
fn with_parameters(id: &str, kinds: &[String]) -> bool {
    id.starts_with("parameter:") || kinds.iter().any(|kind| kind == "parameter")
}

/// Whether `row` is to a parameter: a field-map's.
fn to_parameter(row: &RefRow) -> bool {
    row.to_kind == ToKind::Parameter || row.to_id.as_deref().is_some_and(|to| to.starts_with("parameter:"))
}

/// The id a call on the artifact stack is observed under ([`moqui_core::devtools::calls::artifact_id`]): a transition's
/// without its method (`transition:<screen>#<name>`), since a frame names only the screen and the transition; any other
/// id as it is.
fn observed_form(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("transition:") {
        if let Some((screen, name)) = rest.rsplit_once('#') {
            if let Some((name, _method)) = name.rsplit_once(':') {
                return format!("transition:{screen}#{name}");
            }
        }
    }
    id.to_string()
}

/// The indexed id of an observed call's `id`: a method-less transition's is its `any` transition, else the first of
/// its methods; an id that is no such transition, or names none indexed, as it is.
fn indexed_transition(facade: &EntityFacade, id: &str) -> Result<String, Error> {
    if observed_form(id) != id || !id.starts_with("transition:") || !id.contains('#') {
        return Ok(id.to_string());
    }
    let found = facade
        .find(ARTIFACT)?
        .disable_authz()
        .condition(Condition::like("artifactId", Value::string(format!("{id}:%"))))
        .select_field("artifactId")
        .order_by("artifactId", false)
        .list()?;
    let prefix = format!("{id}:");
    let methods: Vec<String> = found
        .values()
        .iter()
        .map(|row| text(row, "artifactId"))
        .filter(|found| found.starts_with(&prefix))
        .collect();
    let any = format!("{id}:any");
    Ok(if methods.contains(&any) {
        any
    } else {
        methods.into_iter().next().unwrap_or_else(|| id.to_string())
    })
}

/// The artifact or reference at `line` of `path`, for go to definition: the reference on the line naming `word` (or
/// the first on the line), resolved to its artifact; else the artifact defined on the line.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn at(factory: &Arc<ExecutionContextFactory>, path: &str, line: u32, word: &str) -> Result<Option<At>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let path = canonical(Path::new(path)).display().to_string();
    let rows = facade
        .find(REF)?
        .disable_authz()
        .eq("filePath", Value::string(&path))
        .eq("lineNumber", Value::I64(i64::from(line)))
        .list()?;
    let word = word.trim();
    let chosen = rows
        .values()
        .iter()
        .find(|row| {
            let name = text(row, "toName");
            !word.is_empty() && (name == word || name.ends_with(&format!(".{word}")) || name.contains(word))
        })
        .or_else(|| rows.values().first())
        .map(ref_of);
    if let Some(reference) = chosen {
        let target = match &reference.to_id {
            Some(id) => describe(factory, id)?,
            None => None,
        };
        return Ok(Some((Some(entry(&path, &reference, None)), target)));
    }
    let defined = facade
        .find(ARTIFACT)?
        .disable_authz()
        .eq("filePath", Value::string(&path))
        .eq("lineNumber", Value::I64(i64::from(line)))
        .list()?;
    Ok(defined.values().first().map(|row| (None, Some(artifact_of(row)))))
}

/// The unresolved references of `path`.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn unresolved(factory: &Arc<ExecutionContextFactory>, path: &str) -> Result<Vec<RefEntry>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let path = canonical(Path::new(path)).display().to_string();
    Ok(facade
        .find(REF)?
        .disable_authz()
        .eq("filePath", Value::string(&path))
        .eq("resolved", Value::string("N"))
        .list()?
        .values()
        .iter()
        .map(|row| entry(&path, &ref_of(row), None))
        .collect())
}

/// The extensions made by `components` (every component when empty): `extend` and `anchor` references with whether
/// they resolve, broken first.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn extensions(factory: &Arc<ExecutionContextFactory>, components: &[String]) -> Result<Vec<Extension>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let rows = facade
        .find(REF)?
        .disable_authz()
        .condition(Condition::is_in(
            "refType",
            vec![Value::string("extend"), Value::string("anchor")],
        ))
        .list()?;
    let from_ids: BTreeSet<String> = rows.values().iter().map(|row| text(row, "fromArtifactId")).collect();
    let froms = artifacts_by_id(&facade, &from_ids)?;
    let mut found: Vec<Extension> = rows
        .values()
        .iter()
        .filter_map(|row| {
            let reference = ref_of(row);
            let from = froms.get(&reference.from)?;
            (components.is_empty() || components.contains(&from.component))
                .then(|| extension(from, &text(row, "filePath"), &reference))
        })
        .collect();
    found.sort_by(|a, b| {
        a.resolved
            .cmp(&b.resolved)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    Ok(found)
}

/// The stamp of the rows: each file's location and hash, and the last time a file's rows were written. A stamp made
/// while the rows changed is answered but not kept.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn stamp(factory: &Arc<ExecutionContextFactory>) -> Result<IndexStamp, Error> {
    ensure(factory)?;
    let state = state(factory);
    let generation = state.generation.load(Ordering::SeqCst);
    if let Some(stamp) = lock(&state.stamp).clone() {
        return Ok(stamp);
    }
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let mut files = Vec::new();
    let mut built_at = 0;
    for row in facade
        .find(FILE)?
        .disable_authz()
        .select_field("filePath")
        .select_field("location")
        .select_field("fileHash")
        .select_field("indexedDate")
        .list()?
        .values()
    {
        let location = text(row, "location");
        let location = if location.is_empty() {
            text(row, "filePath")
        } else {
            location
        };
        files.push((location, text(row, "fileHash")));
        if let Some(indexed) = row.get("indexedDate").and_then(moqui_core::user::value_millis) {
            built_at = built_at.max(indexed);
        }
    }
    let stamp = IndexStamp::of(files, built_at);
    let mut kept = lock(&state.stamp);
    // not kept when the rows changed while they were read
    if state.generation.load(Ordering::SeqCst) == generation {
        *kept = Some(stamp.clone());
    }
    Ok(stamp)
}

/// Counts of the index's rows by artifact kind, for Home and the Artifacts screen.
///
/// # Errors
///
/// Returns an error when the index is not kept here or cannot be read.
pub fn counts(factory: &Arc<ExecutionContextFactory>) -> Result<BTreeMap<String, i64>, Error> {
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let mut counts = BTreeMap::new();
    for row in facade
        .find(ARTIFACT)?
        .disable_authz()
        .select_field("artifactKind")
        .list()?
        .values()
    {
        *counts.entry(text(row, "artifactKind")).or_insert(0) += 1;
    }
    Ok(counts)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
mod parameter_tests;

#[cfg(test)]
mod kinds_tests;

#[cfg(test)]
mod derive_tests;

#[cfg(test)]
mod operation_tests;
