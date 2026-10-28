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

pub mod calls;
pub mod extract;
pub mod resolve;
pub mod rows;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde::Serialize;

pub use self::extract::{ArtifactRow, FileIndex, FileInfo, RefRow, ToKind};
use self::resolve::Resolver;
use super::ArtifactKind;
use crate::context::ExecutionContextFactory;
use crate::entity::{fence, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

const FILE: &str = "moqui.devtools.DevArtifactFile";
const ARTIFACT: &str = "moqui.devtools.DevArtifact";
const REF: &str = "moqui.devtools.DevArtifactRef";
const CALL: &str = "moqui.devtools.DevArtifactCall";

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

    /// Read the file's artifacts and references.
    ///
    /// # Errors
    ///
    /// Returns why when the file does not read or does not parse.
    pub fn extract(&self) -> Result<(String, FileIndex), String> {
        let text = std::fs::read_to_string(&self.path).map_err(|err| err.to_string())?;
        let (display, mut info) = self.info();
        info.path = &display;
        let index = extract::extract(&info, &text)?;
        Ok((hash_of(&text), index))
    }

    /// The artifacts and references of `text` as this file's (an editor's unsaved buffer).
    ///
    /// # Errors
    ///
    /// Returns why when the text does not parse.
    pub fn extract_text(&self, text: &str) -> Result<FileIndex, String> {
        let (display, mut info) = self.info();
        info.path = &display;
        extract::extract(&info, text)
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
    // a data directory's other files (images, documents) are no data files
    if kind == ArtifactKind::Data && path.extension().is_none_or(|ext| ext != "xml") {
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
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().take(12).map(|byte| format!("{byte:02x}")).collect()
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
    pub resolver: Resolver,
    /// Files that did not read or parse, with why.
    pub problems: Vec<(String, String)>,
}

impl MemoryIndex {
    /// Read every component file of `factory`.
    #[must_use]
    pub fn build(factory: &ExecutionContextFactory) -> Self {
        Self::of_files(factory, &component_files(factory))
    }

    /// Read `files`.
    #[must_use]
    pub fn of_files(factory: &ExecutionContextFactory, files: &[Located]) -> Self {
        let mut index = Self::default();
        for file in files {
            match file.extract() {
                Ok((_, read)) => {
                    let path = file.path.display().to_string();
                    for artifact in &read.artifacts {
                        index.resolver.add(artifact);
                    }
                    index.artifacts.extend(read.artifacts);
                    index.refs.extend(read.refs.into_iter().map(|row| (path.clone(), row)));
                }
                Err(problem) => {
                    tracing::error!("Artifact index: {} not read: {problem}", file.path.display());
                    index.problems.push((file.path.display().to_string(), problem));
                }
            }
        }
        let exists = |location: &str| location_exists(factory, location);
        for (_, row) in &mut index.refs {
            index.resolver.resolve(row, &exists);
        }
        index
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
        let mut ids: HashSet<&str> = HashSet::from([id]);
        ids.extend(
            self.artifacts
                .iter()
                .filter(|artifact| artifact.parent.as_deref() == Some(id))
                .map(|artifact| artifact.id.as_str()),
        );
        self.refs
            .iter()
            .filter(|(_, row)| match direction {
                Direction::In => row.to_id.as_deref().is_some_and(|to| ids.contains(to)),
                Direction::Out => ids.contains(row.from.as_str()),
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
        let Ok(read) = located.extract_text(text) else {
            return false;
        };
        let path = located.path.display().to_string();
        self.artifacts.retain(|artifact| artifact.path != path);
        self.refs.retain(|(file, _)| *file != path);
        self.artifacts.extend(read.artifacts);
        self.refs.extend(read.refs.into_iter().map(|row| (path.clone(), row)));
        let mut resolver = Resolver::default();
        for artifact in &self.artifacts {
            resolver.add(artifact);
        }
        let exists = |location: &str| location_exists(factory, location);
        for (_, row) in &mut self.refs {
            resolver.resolve(row, &exists);
        }
        self.resolver = resolver;
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
            if artifact.id == text || artifact.name == text {
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
        "field" => 5,
        "anchor" => 6,
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
}

fn state(factory: &ExecutionContextFactory) -> Arc<IndexState> {
    factory.extension(|_| IndexState::default())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the index keeps rows on this instance: a dev instance with a database.
#[must_use]
pub fn enabled(factory: &ExecutionContextFactory) -> bool {
    super::is_dev_instance()
        && factory
            .entity()
            .is_some_and(|facade| !facade.is_model_only() && crate::entity::info::is_entity_defined(&facade, ARTIFACT))
}

/// Build the index in the background, when this is a dev instance with a database (a dev server's start).
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if !enabled(factory) {
        return;
    }
    listen(factory);
    calls::start(factory);
    let factory = Arc::clone(factory);
    let spawned = std::thread::Builder::new()
        .name("artifact-index".into())
        .spawn(move || {
            if let Err(err) = build(&factory) {
                tracing::error!("Artifact index: the build failed: {err}");
            }
        });
    if let Err(err) = spawned {
        tracing::error!("Artifact index: could not start the build: {err}");
    }
}

/// Keep the index current with each reload, once per factory.
fn listen(factory: &ExecutionContextFactory) {
    if state(factory).listening.swap(true, Ordering::SeqCst) {
        return;
    }
    super::reload::on_reload(factory, |factory, report| {
        if !state(factory).built.load(Ordering::SeqCst) {
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
        return Ok(());
    }
    listen(factory);
    calls::start(factory);
    build(factory).map(|_| ())
}

/// Remove every row of the index, so the next build reads every file.
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
    for entity in [REF, ARTIFACT, FILE] {
        facade.find(entity)?.disable_authz().delete_all()?;
    }
    *lock(&state.resolver) = Resolver::default();
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
    let started = std::time::Instant::now();
    let entered = factory.new_context();
    let _enter = entered.enter();
    let _authz = entered.disable_authz();
    let _lifted = fence::lift();
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
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
        let list = facade
            .find(ARTIFACT)?
            .select_field("artifactId")
            .select_field("artifactKind")
            .select_field("artifactName")
            .select_field("shortName")
            .list()?;
        for row in list.into_values() {
            resolver.add_parts(
                &text(&row, "artifactId"),
                &text(&row, "artifactKind"),
                &text(&row, "artifactName"),
                &text(&row, "shortName"),
            );
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
        let path = file.path.display().to_string();
        on_disk.insert(path.clone());
        let Ok(text) = std::fs::read_to_string(&file.path) else {
            continue;
        };
        if hashes.get(&path).is_some_and(|hash| *hash == hash_of(&text)) {
            report.unchanged += 1;
            continue;
        }
        changes.push(change_of(file, &text));
    }
    // the artifacts that are rows: service jobs, data documents, email templates, DbForm forms
    for source in if rows { rows::read(&facade) } else { Vec::new() } {
        if hashes.get(&source.path).is_some_and(|hash| *hash == source.hash) {
            report.unchanged += 1;
            continue;
        }
        changes.push(Change::Read {
            file: Located {
                path: PathBuf::from(&source.path),
                component: String::new(),
                location: source.path,
                kind: ArtifactKind::Other,
            },
            hash: source.hash,
            index: source.index,
        });
    }
    for path in hashes
        .keys()
        .filter(|path| !path.starts_with("db:") && !on_disk.contains(*path))
    {
        changes.push(Change::Removed(path.clone()));
    }
    report.read_millis = started.elapsed().as_millis();
    apply(factory, &facade, &state, changes, &mut report)?;
    state.built.store(true, Ordering::SeqCst);
    report.millis = started.elapsed().as_millis();
    tracing::info!(
        "Artifact index built in {} ms: {} files ({} read, {} unchanged, {} removed, {} stale), {} artifacts and {} references written",
        report.millis,
        report.files,
        report.read,
        report.unchanged,
        report.removed,
        report.stale,
        report.artifacts,
        report.refs
    );
    Ok(report)
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
            changes.extend(read_change(&file));
        } else {
            changes.push(Change::Removed(path.display().to_string()));
        }
    }
    let mut report = BuildReport {
        files: changes.len(),
        ..BuildReport::default()
    };
    apply(factory, &facade, &state, changes, &mut report)?;
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
    let changes: Vec<Change> = files.iter().filter_map(read_change).collect();
    let mut report = BuildReport {
        files: changes.len(),
        ..BuildReport::default()
    };
    apply(factory, &facade, &state, changes, &mut report)?;
    report.millis = started.elapsed().as_millis();
    Ok(report)
}

fn read_change(file: &Located) -> Option<Change> {
    match std::fs::read_to_string(&file.path) {
        Ok(text) => Some(change_of(file, &text)),
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

fn change_of(file: &Located, text: &str) -> Change {
    let display = file.path.display().to_string();
    let (_, mut info) = file.info();
    info.path = &display;
    let hash = hash_of(text);
    match extract::extract(&info, text) {
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
    let mut gone: BTreeSet<String> = BTreeSet::new();
    let mut added: Vec<ArtifactRow> = Vec::new();
    let mut changed_paths: HashSet<String> = HashSet::new();
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
            for old in old_artifacts(facade, &path)? {
                resolver.remove(&old);
                gone.insert(old.id);
            }
        }
        for change in &changes {
            if let Change::Read { index, .. } = change {
                for artifact in &index.artifacts {
                    resolver.add(artifact);
                    gone.remove(&artifact.id);
                    added.push(artifact.clone());
                }
            }
        }
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
            }
        }
    }
    let tx = facade.transactions_for(ARTIFACT)?;
    let writing = std::time::Instant::now();
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
            let _pipeline = crate::entity::exec::PipelineWrites::enter();
            for change in batch {
                write(change)?;
            }
            tx.finish_deferred()
        })
    };
    for batch in changes.chunks(100) {
        let outcomes: Vec<(&Change, Result<(), Error>)> = match run(batch) {
            Ok(()) => batch.iter().map(|change| (change, Ok(()))).collect(),
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
    }
    unresolve(facade, &tx, &gone, &changed_paths)?;
    resolve_added(factory, facade, state, &tx, &added, &changed_paths)?;
    report.write_millis = writing.elapsed().as_millis();
    Ok(())
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
    }
    let mut seen = HashSet::new();
    for (sequence, artifact) in index.artifacts.iter().enumerate() {
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
        row.set("filePath", Value::string(&path));
        row.set("lineNumber", Value::I64(i64::from(artifact.line)));
        row.set("columnNumber", Value::I64(i64::from(artifact.column)));
        row.set("summary", Value::string(&artifact.summary));
        if let Some(parent) = &artifact.parent {
            row.set("parentArtifactId", Value::string(parent));
        }
        row.set("sequenceNum", Value::I64(i64::try_from(sequence).unwrap_or(0)));
        if shared.contains(&artifact.id) {
            facade
                .find(ARTIFACT)?
                .eq("artifactId", Value::string(&artifact.id))
                .delete_all()?;
        }
        row.create(facade)?;
    }
    for (sequence, reference) in index.refs.iter().enumerate() {
        let mut row = facade.make_value(REF)?;
        row.set("filePath", Value::string(&path));
        row.set("refSeqId", Value::string(format!("{sequence:05}")));
        fill_ref(&mut row, reference);
        row.create(facade)?;
    }
    store_file(facade, file, hash, false, "", known)
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
    row.set("indexedDate", crate::user::now_value());
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
    facade.find(FILE)?.eq("filePath", Value::string(path)).delete_all()?;
    Ok(())
}

/// The references elsewhere to artifacts no file defines any more: unresolved now.
fn unresolve(
    facade: &EntityFacade,
    tx: &crate::transaction::TransactionManager,
    gone: &BTreeSet<String>,
    changed: &HashSet<String>,
) -> Result<(), Error> {
    let ids: Vec<&String> = gone.iter().collect();
    for chunk in ids.chunks(500) {
        let values: Vec<Value> = chunk.iter().map(|id| Value::string(id.as_str())).collect();
        let rows = facade
            .find(REF)?
            .condition(Condition::is_in("toArtifactId", values))
            .list()?;
        tx.run(TransactionAttribute::ForceNew, None, || {
            for mut row in rows.into_values() {
                if changed.contains(&text(&row, "filePath")) {
                    continue;
                }
                row.set("toArtifactId", Value::Null);
                row.set("resolved", Value::string("N"));
                row.update(facade)?;
            }
            Ok(())
        })?;
    }
    Ok(())
}

/// The unresolved references elsewhere that an added artifact may answer: resolved again.
fn resolve_added(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    state: &IndexState,
    tx: &crate::transaction::TransactionManager,
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
    if now_resolved.is_empty() {
        return Ok(());
    }
    tx.run(TransactionAttribute::ForceNew, None, || {
        for (mut row, reference) in now_resolved {
            row.set(
                "toArtifactId",
                reference.to_id.as_deref().map_or(Value::Null, Value::string),
            );
            row.set("resolved", Value::string("Y"));
            row.update(facade)?;
        }
        Ok(())
    })
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
    let narrow = |find: crate::entity::EntityFind| -> crate::entity::EntityFind {
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
            Condition::equals("shortName", Value::string(text_query.trim())),
        ]))
        .limit(50)
        .list()?;
    let mut candidates: Vec<ArtifactRow> = exact.values().iter().map(artifact_of).collect();
    if !query.is_empty() {
        // a key with a segment starting with each query segment's first letter, in order: the ranking does the rest
        let rest: String = query
            .iter()
            .skip(1)
            .filter_map(|segment| segment.chars().next())
            .map(|first| format!("% {first}"))
            .collect();
        let first = query[0].chars().next().unwrap_or('%');
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
    ensure(factory)?;
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let _lifted = fence::lift();
    let mut ids: Vec<Value> = vec![Value::string(id)];
    for child in facade
        .find(ARTIFACT)?
        .disable_authz()
        .eq("parentArtifactId", Value::string(id))
        .select_field("artifactId")
        .list()?
        .values()
    {
        ids.push(Value::string(text(child, "artifactId")));
    }
    let (near, far) = match direction {
        Direction::In => ("toArtifactId", "fromArtifactId"),
        Direction::Out => ("fromArtifactId", "toArtifactId"),
    };
    let rows = facade
        .find(REF)?
        .disable_authz()
        .condition(Condition::is_in(near, ids.clone()))
        .order_by("filePath", false)
        .order_by("lineNumber", false)
        .list()?;
    let calls = facade
        .find(CALL)?
        .disable_authz()
        .condition(Condition::is_in(near, ids))
        .list()?;
    let mut other_ids: BTreeSet<String> = BTreeSet::new();
    for row in rows.values().iter().chain(calls.values()) {
        let other = text(row, far);
        if !other.is_empty() {
            other_ids.insert(other);
        }
    }
    let others = artifacts_by_id(&facade, &other_ids)?;
    let mut call_counts: HashMap<(String, String), i64> = HashMap::new();
    for row in calls.values() {
        call_counts.insert(
            (text(row, "fromArtifactId"), text(row, "toArtifactId")),
            number(row, "callCount"),
        );
    }
    let mut entries: Vec<RefEntry> = Vec::new();
    for row in rows.values() {
        let reference = ref_of(row);
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
        });
    }
    if !kinds.is_empty() {
        entries.retain(|item| item.other.as_ref().is_some_and(|other| kinds.contains(&other.kind)));
    }
    Ok(entries)
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
