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

//! `file`, `runtime`, `component`, `dbresource`, and `http` resources. Other schemes are named errors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use moqui_lib::object::Value;
use moqui_lib::resource::{FileResource, Resource};

use super::error::Error;
use crate::entity::{DbResourceInfo, EntityFacade};
use crate::resource::textdiff::{self, TextDiff};

mod changes;
use changes::db_file_size;
#[cfg(feature = "git")]
pub(crate) use changes::millis_of;
pub(crate) use changes::version_author;
pub(crate) use changes::{apply_changes_each, last_change_by_stat, tree_size_by_walk};
pub use changes::{
    with_version_author, ChangeLogEntry, ChangeSetOutcome, ResourceChange, ResourceChangeInfo, VersionAuthor,
};

/// One version of a file, as Java's `ResourceReference.Version`: a `dbresource` history row, or a commit of a `git`
/// file. `version_date` is a Timestamp value. `note` is what was written with the version: the commit message (without
/// its `Moqui-User-Id` trailer) of a `git` file, the `versionNote` of a `dbresource` one.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceVersion {
    pub version_name: String,
    pub previous_version_name: Option<String>,
    pub version_date: Value,
    pub user_id: Option<String>,
    pub note: Option<String>,
}

/// A resolved resource with its logical location.
///
/// A database resource is a snapshot. Directory children are names only; their bytes are not loaded.
#[derive(Clone, Debug)]
pub struct ResourceRef {
    location: String,
    stored: Stored,
}

#[derive(Clone, Debug)]
enum Stored {
    File(FileResource),
    Db(DbSnap),
    /// Fetched on first use, so `get` itself opens no connection.
    Http(HttpRef),
    Native(Arc<dyn crate::native::ResourceProvider>),
}

/// An `http` or `https` location. The first read or existence check does the GET, and its outcome is kept.
#[derive(Clone, Debug)]
struct HttpRef {
    url: String,
    /// True when sim mode forbids the call.
    sim: bool,
    fetched: Arc<OnceLock<Result<String, String>>>,
}

impl HttpRef {
    fn fetch(&self) -> &Result<String, String> {
        self.fetched.get_or_init(|| http_get(&self.url, self.sim))
    }
}

/// GET through [`crate::outbound`]. Sim mode denies before anything is sent.
fn http_get(url: &str, sim: bool) -> Result<String, String> {
    if sim {
        return Err(format!(
            "HTTP GET {} denied because sim mode is active",
            crate::outbound::redacted(url)
        ));
    }
    let request = crate::outbound::Request {
        method: crate::outbound::Method::Get,
        url: url.to_string(),
        headers: Vec::new(),
        body: None,
        timeout: None,
    };
    match crate::outbound::send(request) {
        Ok(response) if (200..300).contains(&response.status) => Ok(response.body),
        Ok(response) => Err(format!("HTTP GET returned {}", response.status)),
        Err(err) => Err(err.message().to_string()),
    }
}

/// A `dbresource` node as found. A file's bytes are read when they are first asked for.
#[derive(Clone)]
struct DbSnap {
    exists: bool,
    is_file: bool,
    bytes: Arc<OnceLock<Result<Vec<u8>, String>>>,
    facade: Option<EntityFacade>,
    children: Vec<DbSnapChild>,
}

impl std::fmt::Debug for DbSnap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbSnap")
            .field("exists", &self.exists)
            .field("is_file", &self.is_file)
            .field("children", &self.children)
            .finish()
    }
}

impl DbSnap {
    /// A file under a directory read before, whose bytes are read when wanted.
    fn file(facade: Option<EntityFacade>, exists: bool) -> Self {
        Self {
            exists,
            is_file: exists,
            bytes: Arc::new(OnceLock::new()),
            facade,
            children: Vec::new(),
        }
    }

    fn bytes(&self, location: &str) -> Result<Vec<u8>, Error> {
        self.bytes
            .get_or_init(|| {
                let Some(facade) = &self.facade else {
                    return Ok(Vec::new());
                };
                // a file of a listing deleted since is no file now
                facade
                    .read_db_resource_bytes(location)
                    .map_err(|err| err.message().to_string())?
                    .ok_or_else(|| format!("not a file: {location}"))
            })
            .clone()
            .map_err(Error::new)
    }
}

#[derive(Clone, Debug)]
struct DbSnapChild {
    location: String,
    name: String,
    is_file: bool,
}

impl ResourceRef {
    pub fn location(&self) -> &str {
        &self.location
    }

    /// True when the storage keeps versions of this file (`ResourceReference.supportsVersion`).
    pub fn supports_version(&self) -> bool {
        match &self.stored {
            Stored::Db(_) => true,
            Stored::Native(provider) => provider.supports_version(&self.location),
            Stored::File(_) | Stored::Http(_) => false,
        }
    }

    pub fn exists(&self) -> bool {
        match &self.stored {
            Stored::Native(provider) => provider.stat(&self.location).ok().flatten().is_some(),
            Stored::File(file) => file.exists(),
            Stored::Db(snap) => snap.exists,
            Stored::Http(http) => http.fetch().is_ok(),
        }
    }

    pub fn is_file(&self) -> bool {
        match &self.stored {
            Stored::Native(provider) => provider
                .stat(&self.location)
                .ok()
                .flatten()
                .is_some_and(|stat| stat.is_file),
            Stored::File(file) => file.is_file(),
            Stored::Db(snap) => snap.exists && snap.is_file,
            Stored::Http(http) => http.fetch().is_ok(),
        }
    }

    pub fn is_directory(&self) -> bool {
        match &self.stored {
            Stored::Native(provider) => provider
                .stat(&self.location)
                .ok()
                .flatten()
                .is_some_and(|stat| !stat.is_file),
            Stored::File(file) => file.is_directory(),
            Stored::Db(snap) => snap.exists && !snap.is_file,
            Stored::Http(_) => false,
        }
    }

    /// Milliseconds since the epoch for a file resource. Other kinds have none, so a cache over them
    /// does not invalidate on change.
    pub fn last_modified(&self) -> Option<i64> {
        match &self.stored {
            Stored::Native(provider) => provider
                .stat(&self.location)
                .ok()
                .flatten()
                .and_then(|stat| stat.last_modified),
            Stored::File(file) => file.last_modified().ok(),
            Stored::Db(_) | Stored::Http(_) => None,
        }
    }

    pub fn read_text(&self) -> Result<String, Error> {
        match &self.stored {
            Stored::Native(provider) => String::from_utf8(provider.read_bytes(&self.location)?)
                .map_err(|_| Error::new(format!("resource is not utf-8: {}", self.location))),
            Stored::File(file) => file.read_text().map_err(Error::from),
            Stored::Http(http) => http
                .fetch()
                .clone()
                .map_err(|message| Error::new(format!("{message}: {}", self.location))),
            Stored::Db(snap) => {
                if !snap.exists || !snap.is_file {
                    return Err(Error::new(format!("not a file: {}", self.location)));
                }
                String::from_utf8(snap.bytes(&self.location)?)
                    .map_err(|_| Error::new(format!("dbresource is not utf-8: {}", self.location)))
            }
        }
    }

    /// The bytes of the file, whatever they hold.
    pub fn read_bytes(&self) -> Result<Vec<u8>, Error> {
        match &self.stored {
            Stored::Native(provider) => provider.read_bytes(&self.location),
            Stored::File(file) => file.read_bytes().map_err(Error::from),
            Stored::Http(http) => http
                .fetch()
                .clone()
                .map(String::into_bytes)
                .map_err(|message| Error::new(format!("{message}: {}", self.location))),
            Stored::Db(snap) => {
                if !snap.exists || !snap.is_file {
                    return Err(Error::new(format!("not a file: {}", self.location)));
                }
                snap.bytes(&self.location)
            }
        }
    }

    /// The size in bytes of a file resource: a file, a provider's file, or a `dbresource://` file (its
    /// `DbResourceFile.fileSize`, else the length of its data). A directory, a missing file and an `http` resource have
    /// none.
    pub fn size(&self) -> Option<u64> {
        match &self.stored {
            Stored::Native(provider) => provider.stat(&self.location).ok().flatten().map(|stat| stat.size),
            Stored::File(file) => file.size().ok(),
            Stored::Db(snap) => db_file_size(&self.location, snap),
            Stored::Http(_) => None,
        }
    }

    /// The path of a file resource, for a check that it stays inside its directory.
    pub fn file_path(&self) -> Option<&std::path::Path> {
        match &self.stored {
            Stored::File(file) => Some(file.path()),
            Stored::Db(_) | Stored::Http(_) | Stored::Native(_) => None,
        }
    }

    pub fn children(&self) -> Result<Vec<Self>, Error> {
        match &self.stored {
            Stored::Native(provider) => Ok(provider
                .list(&self.location)?
                .into_iter()
                .filter(|entry| entry.is_file)
                .map(|entry| Self {
                    location: entry.location,
                    stored: Stored::Native(Arc::clone(provider)),
                })
                .collect()),
            Stored::File(file) => {
                let files = file.children().map_err(Error::from)?;
                Ok(files
                    .into_iter()
                    .map(|file| {
                        let name = file.file_name().to_string();
                        let location = join_location(&self.location, &name);
                        Self {
                            location,
                            stored: Stored::File(file),
                        }
                    })
                    .collect())
            }
            Stored::Http(_) => Ok(Vec::new()),
            Stored::Db(snap) => Ok(snap
                .children
                .iter()
                .filter(|child| child.is_file)
                .map(|child| Self {
                    location: child.location.clone(),
                    stored: Stored::Db(DbSnap::file(snap.facade.clone(), true)),
                })
                .collect()),
        }
    }

    pub fn find_child_file(&self, relative: &str) -> Self {
        match &self.stored {
            Stored::Native(provider) => Self {
                location: if relative.is_empty() {
                    self.location.clone()
                } else {
                    join_location(&self.location, relative)
                },
                stored: Stored::Native(Arc::clone(provider)),
            },
            Stored::File(file) => {
                let file = file.find_child_file(relative);
                let location = if relative.is_empty() {
                    self.location.clone()
                } else {
                    join_location(&self.location, relative)
                };
                Self {
                    location,
                    stored: Stored::File(file),
                }
            }
            Stored::Http(_) => self.clone(),
            Stored::Db(snap) => {
                if relative.is_empty() {
                    return self.clone();
                }
                if let Some(child) = snap
                    .children
                    .iter()
                    .find(|child| child.is_file && child.name == relative)
                {
                    return Self {
                        location: child.location.clone(),
                        stored: Stored::Db(DbSnap::file(snap.facade.clone(), true)),
                    };
                }
                Self {
                    location: join_location(&self.location, relative),
                    stored: Stored::Db(DbSnap::file(snap.facade.clone(), false)),
                }
            }
        }
    }
}

/// What [`ResourceFacade::stat`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceStat {
    pub is_file: bool,
    pub size: u64,
    /// Milliseconds since the epoch, when the storage keeps one.
    pub last_modified: Option<i64>,
}

/// A file opened by [`ResourceFacade::open`].
#[derive(Clone)]
pub struct Opened {
    pub stat: ResourceStat,
    pub source: Arc<dyn crate::resource::source::ByteSource>,
}

/// One entry of [`ResourceFacade::list`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceEntry {
    pub name: String,
    pub location: String,
    pub is_file: bool,
}

fn io_error(location: &str, err: &std::io::Error) -> Error {
    Error::new(format!("{location}: {err}"))
}

pub struct ResourceFacade {
    runtime_dir: PathBuf,
    components: HashMap<String, PathBuf>,
    /// Shared with the factory. `None` until the host installs an entity facade.
    entity: Arc<RwLock<Option<EntityFacade>>>,
    providers: RwLock<HashMap<String, Arc<dyn crate::native::ResourceProvider>>>,
    /// The `git` scheme, once `MoquiDefaultConf.xml` names it; the `GitServices` reach it here.
    #[cfg(feature = "git")]
    git: OnceLock<crate::resource::git::GitProvider>,
}

impl ResourceFacade {
    pub(super) fn new(
        runtime_dir: PathBuf,
        components: HashMap<String, PathBuf>,
        entity: Arc<RwLock<Option<EntityFacade>>>,
    ) -> Self {
        Self {
            runtime_dir,
            components,
            entity,
            providers: RwLock::new(HashMap::new()),
            #[cfg(feature = "git")]
            git: OnceLock::new(),
        }
    }

    /// Install the `git` scheme (`resource-reference scheme="git"` in the conf).
    #[cfg(feature = "git")]
    pub(crate) fn install_git(&self) -> Result<(), Error> {
        let provider = crate::resource::git::GitProvider::new(Arc::clone(&self.entity), self.runtime_dir.clone());
        self.install_provider("git", Arc::new(provider.clone()))?;
        self.git
            .set(provider)
            .map_err(|_| Error::new("the git resource scheme is installed twice"))
    }

    /// The `git` scheme's provider, when it is installed.
    #[cfg(feature = "git")]
    pub fn git(&self) -> Option<&crate::resource::git::GitProvider> {
        self.git.get()
    }

    pub(crate) fn install_provider(
        &self,
        scheme: &str,
        provider: Arc<dyn crate::native::ResourceProvider>,
    ) -> Result<(), Error> {
        if matches!(
            scheme,
            "file" | "runtime" | "component" | "classpath" | "dbresource" | "http" | "https"
        ) {
            return Err(Error::new(format!("cannot replace built-in resource scheme {scheme}")));
        }
        let mut providers = moqui_lib::sync::write(&self.providers);
        if providers.contains_key(scheme) {
            return Err(Error::new(format!("duplicate resource scheme {scheme}")));
        }
        providers.insert(scheme.to_string(), provider);
        Ok(())
    }

    /// True when a provider (a component's scheme, or `git`) serves `location`.
    pub fn has_provider(&self, location: &str) -> bool {
        self.provider(location).is_some()
    }

    fn provider(&self, location: &str) -> Option<Arc<dyn crate::native::ResourceProvider>> {
        moqui_lib::sync::read(&self.providers).get(scheme_of(location)).cloned()
    }

    pub fn get(&self, location: &str) -> Result<ResourceRef, Error> {
        if location.is_empty() {
            return Err(Error::new("resource location is empty"));
        }
        if let Some(provider) = self.provider(location) {
            // A `#version` suffix names a version (`getLocationText`), not part of the path.
            return Ok(ResourceRef {
                location: strip_hash(location).to_string(),
                stored: Stored::Native(provider),
            });
        }
        let scheme = scheme_of(location);
        match scheme {
            "content" => Err(Error::new(format!("content resource is not available: {location}"))),
            "http" | "https" => Ok(ResourceRef {
                location: location.to_string(),
                stored: Stored::Http(HttpRef {
                    url: strip_hash(location).to_string(),
                    sim: self.installed().is_some_and(|facade| facade.sim_active()),
                    fetched: Arc::new(OnceLock::new()),
                }),
            }),
            "dbresource" => self.db_ref(strip_hash(location)),
            "file" => Ok(self.file_ref(location)),
            "runtime" => Ok(self.runtime_ref(location)),
            "component" => self.component_ref(location),
            // Compatibility alias for a path in the framework component (`service/`, `entity/`,
            // `data/`, `screen/`, `template/`). Seed email templates now use
            // `component://framework/screen/...`. Rows already loaded still say `classpath://screen/...`.
            "classpath" => Ok(ResourceRef {
                location: location.to_string(),
                stored: Stored::File(FileResource::new(
                    crate::context::locate(strip_hash(location))?.to_string_lossy(),
                )),
            }),
            other => Err(Error::new(format!(
                "scheme {other} is not supported for location {location}"
            ))),
        }
    }

    /// Write bytes to a file, creating its parent directories: a `dbresource://` row, or a file under the
    /// `file`, `runtime`, or `component` schemes.
    pub fn write_bytes(&self, location: &str, bytes: &[u8]) -> Result<(), Error> {
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, location)?;
        if let Some(provider) = self.provider(location) {
            return provider.write_bytes(location, bytes);
        }
        if scheme_of(location) == "dbresource" {
            return self
                .db_facade()?
                .write_db_resource(location, bytes)
                .map_err(Error::from);
        }
        let path = self.writable_path(location)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| io_error(location, &err))?;
        }
        std::fs::write(&path, bytes).map_err(|err| io_error(location, &err))
    }

    /// The file at a location to read a window at a time, for a response, with what it is: `None` when no file is
    /// there. A file on disk is read as it is sent; other storage on the first read, or a window at a time where it
    /// can (a content blob). A `#versionName` suffix opens that version on storage that keeps versions, as
    /// [`Self::location_bytes`] reads it.
    pub fn open(&self, location: &str) -> Result<Option<Opened>, Error> {
        if let Some((path, version_name)) = split_version(location) {
            if self.supports_version(path) {
                return Ok(self.read_version(path, version_name)?.map(|bytes| Opened {
                    stat: ResourceStat {
                        is_file: true,
                        size: bytes.len() as u64,
                        last_modified: None,
                    },
                    source: Arc::new(crate::resource::source::MemorySource::new(bytes)),
                }));
            }
        }
        if let Some(provider) = self.provider(location) {
            return Ok(provider.open(location)?.map(|(stat, source)| Opened { stat, source }));
        }
        if scheme_of(location) == "dbresource" {
            return Ok(self
                .db_facade()?
                .open_db_resource(location)
                .map_err(Error::from)?
                .map(|(stat, source)| Opened {
                    stat: ResourceStat {
                        is_file: true,
                        size: stat.size,
                        last_modified: stat.last_updated,
                    },
                    source,
                }));
        }
        let resource = self.get(location)?;
        if !resource.exists() || !resource.is_file() {
            return Ok(None);
        }
        if let Some(path) = resource.file_path() {
            let source = crate::resource::source::FileSource::open(path)?;
            let stat = ResourceStat {
                is_file: true,
                size: crate::resource::source::ByteSource::len(&source),
                last_modified: resource.last_modified(),
            };
            return Ok(Some(Opened {
                stat,
                source: Arc::new(source),
            }));
        }
        let bytes = resource.read_bytes()?;
        let stat = ResourceStat {
            is_file: true,
            size: bytes.len() as u64,
            last_modified: resource.last_modified(),
        };
        Ok(Some(Opened {
            stat,
            source: Arc::new(crate::resource::source::MemorySource::new(bytes)),
        }))
    }

    /// Write several files together: a change set of each repository (one commit for the files of a `git://`
    /// repository), else each as [`ResourceFacade::write_bytes`] does. A file that is not written is an error.
    pub fn write_many(&self, files: &[(String, Vec<u8>)]) -> Result<(), Error> {
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, files.first().map_or("", |(location, _)| location.as_str()))?;
        let mut by_root: indexmap::IndexMap<String, Vec<ResourceChange>> = indexmap::IndexMap::new();
        for (location, bytes) in files {
            let location = strip_hash(location);
            let under = location
                .split_once("://")
                .and_then(|(scheme, rest)| {
                    let (top, path) = rest.trim_start_matches('/').split_once('/')?;
                    Some((format!("{scheme}://{top}"), path.trim_matches('/').to_string()))
                })
                .filter(|(_, path)| !path.is_empty());
            match under.filter(|_| self.provider(location).is_some()) {
                Some((root, path)) => by_root.entry(root).or_default().push(ResourceChange::Write {
                    path,
                    bytes: bytes.clone(),
                }),
                None => self.write_bytes(location, bytes)?,
            }
        }
        for (root, changes) in by_root {
            let names: Vec<&str> = changes.iter().map(ResourceChange::path).collect();
            let message = match names.as_slice() {
                [one] => format!("Write {one}"),
                many => format!("Write {} files: {}", many.len(), many.join(", ")),
            };
            let outcome = self.apply_change_set(&root, &changes, Some(&message))?;
            if let Some((path, why)) = outcome.failures.first() {
                return Err(Error::new(format!("{root}/{path} was not written: {why}")));
            }
        }
        Ok(())
    }

    /// Write a file from `input` (of `size` bytes, when known), creating its parent directories: an upload streamed
    /// to storage, never all in memory where the storage can take it as it is read (a file, a `git://` content blob).
    pub fn write_from(&self, location: &str, input: &mut dyn std::io::Read, size: Option<u64>) -> Result<(), Error> {
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, location)?;
        if let Some(provider) = self.provider(location) {
            return provider.write_from(location, input, size);
        }
        if scheme_of(location) == "dbresource" {
            return self
                .db_facade()?
                .write_db_resource_from(location, input, size)
                .map_err(Error::from);
        }
        let path = self.writable_path(location)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| io_error(location, &err))?;
        }
        let mut file = std::fs::File::create(&path).map_err(|err| io_error(location, &err))?;
        std::io::copy(input, &mut file).map_err(|err| io_error(location, &err))?;
        Ok(())
    }

    /// The kind, size, and last-modified milliseconds of a location, or `None` when nothing is there.
    pub fn stat(&self, location: &str) -> Result<Option<ResourceStat>, Error> {
        if let Some(provider) = self.provider(location) {
            return provider.stat(location);
        }
        if scheme_of(location) == "dbresource" {
            return Ok(self
                .db_facade()?
                .stat_db_resource(location)
                .map_err(Error::from)?
                .map(|stat| ResourceStat {
                    is_file: stat.is_file,
                    size: stat.size,
                    last_modified: stat.last_updated,
                }));
        }
        let path = self.writable_path(location)?;
        let Ok(meta) = std::fs::metadata(&path) else {
            return Ok(None);
        };
        let last_modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok());
        Ok(Some(ResourceStat {
            is_file: meta.is_file(),
            size: if meta.is_file() { meta.len() } else { 0 },
            last_modified,
        }))
    }

    /// The files and directories directly under a directory location, sorted by name.
    pub fn list(&self, location: &str) -> Result<Vec<ResourceEntry>, Error> {
        let mut entries = if let Some(provider) = self.provider(location) {
            provider.list(location)?
        } else if scheme_of(location) == "dbresource" {
            let info = self.db_facade()?.read_db_resource(location).map_err(Error::from)?;
            if !info.exists || info.is_file {
                return Err(Error::new(format!("not a directory: {location}")));
            }
            info.children
                .into_iter()
                .map(|child| ResourceEntry {
                    location: join_location(location, &child.name),
                    name: child.name,
                    is_file: child.is_file,
                })
                .collect::<Vec<_>>()
        } else {
            let path = self.writable_path(location)?;
            let read = std::fs::read_dir(&path).map_err(|err| io_error(location, &err))?;
            let mut entries = Vec::new();
            for entry in read {
                let entry = entry.map_err(|err| io_error(location, &err))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_file = entry.file_type().map_err(|err| io_error(location, &err))?.is_file();
                entries.push(ResourceEntry {
                    location: join_location(location, &name),
                    name,
                    is_file,
                });
            }
            entries
        };
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    /// Create a directory and any missing parents.
    pub fn make_directory(&self, location: &str) -> Result<(), Error> {
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, location)?;
        if let Some(provider) = self.provider(location) {
            return provider.make_directory(location);
        }
        if scheme_of(location) == "dbresource" {
            return self.db_facade()?.make_db_directory(location).map_err(Error::from);
        }
        let path = self.writable_path(location)?;
        std::fs::create_dir_all(&path).map_err(|err| io_error(location, &err))
    }

    /// Delete a file, or an empty directory. False when nothing is there.
    pub fn delete(&self, location: &str) -> Result<bool, Error> {
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, location)?;
        if let Some(provider) = self.provider(location) {
            return provider.delete(location);
        }
        if scheme_of(location) == "dbresource" {
            let facade = self.db_facade()?;
            let info = facade.read_db_resource(location).map_err(Error::from)?;
            if info.exists && !info.is_file && !info.children.is_empty() {
                return Err(Error::new(format!("directory is not empty: {location}")));
            }
            return facade.delete_db_resource(location).map_err(Error::from);
        }
        let path = self.writable_path(location)?;
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return Ok(false);
        };
        let removed = if meta.is_dir() {
            std::fs::remove_dir(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map(|()| true).map_err(|err| io_error(location, &err))
    }

    /// Move a file or directory to a new location of the same kind of storage.
    pub fn rename(&self, location: &str, new_location: &str) -> Result<(), Error> {
        crate::entity::fence::check_effect(crate::entity::fence::Effect::Resource, location)?;
        if let Some(provider) = self.provider(location) {
            if scheme_of(location) != scheme_of(new_location) {
                return Err(Error::new("cannot rename between resource schemes"));
            }
            return provider.rename(location, new_location);
        }
        let (from_db, to_db) = (
            scheme_of(location) == "dbresource",
            scheme_of(new_location) == "dbresource",
        );
        if from_db != to_db {
            return Err(Error::new(format!(
                "cannot move between database and file resources: {location} to {new_location}"
            )));
        }
        if from_db {
            let facade = self.db_facade()?;
            if facade.stat_db_resource(location).map_err(Error::from)?.is_none() {
                return Err(Error::new(format!("resource not found: {location}")));
            }
            return facade.move_db_resource(location, new_location).map_err(Error::from);
        }
        let from = self.writable_path(location)?;
        let to = self.writable_path(new_location)?;
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|err| io_error(new_location, &err))?;
        }
        std::fs::rename(&from, &to).map_err(|err| io_error(location, &err))
    }

    /// The storage that keeps versions of `location`, if any.
    fn versioned(&self, location: &str) -> Result<Versioned, Error> {
        if let Some(provider) = self.provider(location) {
            return Ok(if provider.supports_version(location) {
                Versioned::Provider(provider)
            } else {
                Versioned::None
            });
        }
        if scheme_of(location) == "dbresource" {
            return Ok(Versioned::Db(self.db_facade()?));
        }
        Ok(Versioned::None)
    }

    /// True when the storage keeps versions of `location` (`ResourceReference.supportsVersion`): `dbresource` files,
    /// and a provider scheme that says so (`git`).
    pub fn supports_version(&self, location: &str) -> bool {
        let location = strip_hash(location);
        match self.provider(location) {
            Some(provider) => provider.supports_version(location),
            None => scheme_of(location) == "dbresource",
        }
    }

    /// One version of a file by name (`getVersion`), or `None`.
    pub fn version(&self, location: &str, version_name: &str) -> Result<Option<ResourceVersion>, Error> {
        let location = strip_hash(location);
        match self.versioned(location)? {
            Versioned::Provider(provider) => provider.version(location, version_name),
            Versioned::Db(facade) => facade.db_resource_version(location, version_name).map_err(Error::from),
            Versioned::None => Ok(None),
        }
    }

    /// The version the file has now (`getCurrentVersion`).
    pub fn current_version(&self, location: &str) -> Result<Option<ResourceVersion>, Error> {
        let location = strip_hash(location);
        match self.versioned(location)? {
            Versioned::Provider(provider) => provider.current_version(location),
            Versioned::Db(facade) => facade.db_resource_current_version(location).map_err(Error::from),
            Versioned::None => Ok(None),
        }
    }

    /// The first version of the file (`getRootVersion`).
    pub fn root_version(&self, location: &str) -> Result<Option<ResourceVersion>, Error> {
        let location = strip_hash(location);
        match self.versioned(location)? {
            Versioned::Provider(provider) => provider.root_version(location),
            Versioned::Db(facade) => facade.db_resource_root_version(location).map_err(Error::from),
            Versioned::None => Ok(None),
        }
    }

    /// Every version of the file, newest first (`getVersionHistory`). Empty when the storage keeps none.
    pub fn version_history(&self, location: &str) -> Result<Vec<ResourceVersion>, Error> {
        let location = strip_hash(location);
        match self.versioned(location)? {
            Versioned::Provider(provider) => provider.version_history(location),
            Versioned::Db(facade) => facade.db_resource_version_history(location).map_err(Error::from),
            Versioned::None => Ok(Vec::new()),
        }
    }

    /// The versions made from `version_name` (`getNextVersions`).
    pub fn next_versions(&self, location: &str, version_name: &str) -> Result<Vec<ResourceVersion>, Error> {
        let location = strip_hash(location);
        match self.versioned(location)? {
            Versioned::Provider(provider) => provider.next_versions(location, version_name),
            Versioned::Db(facade) => facade
                .db_resource_next_versions(location, version_name)
                .map_err(Error::from),
            Versioned::None => Ok(Vec::new()),
        }
    }

    /// The bytes of one version (`openStream(versionName)`), or `None` when there is no such version or the storage
    /// keeps no versions.
    pub fn read_version(&self, location: &str, version_name: &str) -> Result<Option<Vec<u8>>, Error> {
        let location = strip_hash(location);
        match self.versioned(location)? {
            Versioned::Provider(provider) => provider.read_version(location, version_name),
            Versioned::Db(facade) => facade
                .read_db_resource_version(location, version_name)
                .map_err(Error::from),
            Versioned::None => Ok(None),
        }
    }

    /// The text of one version (`getText(versionName)`); bytes that are not UTF-8 are decoded lossily.
    pub fn version_text(&self, location: &str, version_name: &str) -> Result<Option<String>, Error> {
        Ok(self
            .read_version(location, version_name)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// The text at a location, where a `#versionName` suffix reads that version, as Java's
    /// `ResourceFacade.getLocationText`. A suffix on storage that keeps no versions is ignored.
    pub fn location_text(&self, location: &str) -> Result<String, Error> {
        if let Some((path, version_name)) = split_version(location) {
            if self.supports_version(path) {
                return self
                    .version_text(path, version_name)?
                    .ok_or_else(|| Error::new(format!("version {version_name} not found: {path}")));
            }
        }
        self.get(location)?.read_text()
    }

    /// The bytes of the file at a location, where a `#versionName` suffix reads that version on storage that keeps
    /// versions (as [`Self::location_text`]; elsewhere the suffix is ignored). `None` when no file is there, or that
    /// version is not: `openStream()` of such a reference and `ec.web.sendResourceResponse` read this.
    pub fn location_bytes(&self, location: &str) -> Result<Option<Vec<u8>>, Error> {
        if let Some((path, version_name)) = split_version(location) {
            if self.supports_version(path) {
                return self.read_version(path, version_name);
            }
        }
        let resource = self.get(location)?;
        if !resource.exists() || resource.is_directory() {
            return Ok(None);
        }
        resource.read_bytes().map(Some)
    }

    /// The line diff from version `lhs` to version `rhs` of a file; an empty name is the current text. A version that
    /// does not exist is an error naming it.
    pub fn version_diff(&self, location: &str, lhs: &str, rhs: &str) -> Result<TextDiff, Error> {
        let location = strip_hash(location);
        let text = |name: &str| -> Result<String, Error> {
            if name.is_empty() {
                return self.get(location)?.read_text();
            }
            self.version_text(location, name)?
                .ok_or_else(|| Error::new(format!("version {name} not found: {location}")))
        };
        let (old, new) = (text(lhs)?, text(rhs)?);
        let name = |name: &str| {
            if name.is_empty() {
                "current".to_string()
            } else {
                name.to_string()
            }
        };
        Ok(textdiff::diff(&old, &new, &name(lhs), &name(rhs)))
    }

    fn db_facade(&self) -> Result<EntityFacade, Error> {
        self.installed()
            .ok_or_else(|| Error::new("database resource is not available"))
    }

    /// The path of a `file`, `runtime`, or `component` location. Other schemes are not writable.
    fn writable_path(&self, location: &str) -> Result<PathBuf, Error> {
        match scheme_of(location) {
            "file" | "runtime" | "component" => {
                let resource = self.get(location)?;
                resource
                    .file_path()
                    .map(std::path::Path::to_path_buf)
                    .ok_or_else(|| Error::new(format!("not a file location: {location}")))
            }
            other => Err(Error::new(format!("scheme {other} is not writable: {location}"))),
        }
    }

    fn installed(&self) -> Option<EntityFacade> {
        self.entity.read().unwrap_or_else(|err| err.into_inner()).clone()
    }

    fn db_ref(&self, location: &str) -> Result<ResourceRef, Error> {
        let Some(facade) = self.installed() else {
            return Err(Error::new("database resource is not available"));
        };
        let info = facade.db_resource_entry(location).map_err(Error::from)?;
        if !info.exists {
            return Err(Error::new(format!("resource not found: {location}")));
        }
        Ok(ResourceRef {
            location: location.to_string(),
            stored: Stored::Db(db_snap(location, &info, facade.clone())),
        })
    }

    fn file_ref(&self, location: &str) -> ResourceRef {
        let path = file_path(strip_hash(location));
        ResourceRef {
            location: location.to_string(),
            stored: Stored::File(FileResource::new(path.to_string_lossy())),
        }
    }

    fn runtime_ref(&self, location: &str) -> ResourceRef {
        let relative = strip_prefix(strip_hash(location), &["runtime://", "runtime:"]);
        let relative = relative.trim_start_matches('/');
        let path = if relative.is_empty() {
            self.runtime_dir.clone()
        } else {
            self.runtime_dir.join(relative)
        };
        ResourceRef {
            location: location.to_string(),
            stored: Stored::File(FileResource::new(path.to_string_lossy())),
        }
    }

    fn component_ref(&self, location: &str) -> Result<ResourceRef, Error> {
        let stripped = strip_prefix(strip_hash(location), &["component://", "component:"]);
        let stripped = stripped.trim_start_matches('/');
        if stripped.is_empty() {
            return Err(Error::new(format!("component location has no name: {location}")));
        }
        let (name, relative) = match stripped.split_once('/') {
            Some((name, relative)) => (name, relative.trim_start_matches('/')),
            None => (stripped, ""),
        };
        let Some(root) = self.components.get(name) else {
            return Err(Error::new(format!(
                "component {name} is not registered for location {location}"
            )));
        };
        let path = if relative.is_empty() {
            root.clone()
        } else {
            root.join(relative)
        };
        Ok(ResourceRef {
            location: location.to_string(),
            stored: Stored::File(FileResource::new(path.to_string_lossy())),
        })
    }
}

fn db_snap(location: &str, info: &DbResourceInfo, facade: EntityFacade) -> DbSnap {
    DbSnap {
        exists: info.exists,
        is_file: info.is_file,
        bytes: Arc::new(OnceLock::new()),
        facade: Some(facade),
        children: info
            .children
            .iter()
            .map(|child| DbSnapChild {
                location: join_location(location, &child.name),
                name: child.name.clone(),
                is_file: child.is_file,
            })
            .collect(),
    }
}

fn scheme_of(location: &str) -> &str {
    match location.find(':') {
        Some(index) if index > 1 && !location[..index].contains('/') => &location[..index],
        _ => "file",
    }
}

/// Where a location's versions are kept.
enum Versioned {
    Provider(Arc<dyn crate::native::ResourceProvider>),
    Db(EntityFacade),
    None,
}

/// `location#versionName` as the location and the version name; `None` without a non-empty suffix.
pub(crate) fn split_version(location: &str) -> Option<(&str, &str)> {
    match location.find('#') {
        Some(index) if index > 0 && index + 1 < location.len() => Some((&location[..index], &location[index + 1..])),
        _ => None,
    }
}

fn strip_hash(location: &str) -> &str {
    match location.find('#') {
        Some(index) if index > 0 => &location[..index],
        _ => location,
    }
}

fn strip_prefix<'a>(location: &'a str, prefixes: &[&str]) -> &'a str {
    for prefix in prefixes {
        if let Some(rest) = location.strip_prefix(prefix) {
            return rest;
        }
    }
    location
}

fn file_path(location: &str) -> PathBuf {
    let Some(rest) = location.strip_prefix("file:") else {
        return PathBuf::from(location);
    };
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    if rest.starts_with('/') {
        return PathBuf::from(rest);
    }
    match rest.find('/') {
        Some(index) => PathBuf::from(&rest[index..]),
        None => PathBuf::from(rest),
    }
}

fn join_location(parent: &str, child: &str) -> String {
    let parent = parent.trim_end_matches('/');
    let child = child.trim_start_matches('/');
    if parent.is_empty() {
        child.to_string()
    } else if child.is_empty() {
        parent.to_string()
    } else {
        format!("{parent}/{child}")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use moqui_lib::error::Error as LibError;
    use moqui_lib::object::Value;

    use super::*;
    use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
    use crate::entity::EntityGroup;
    use crate::transaction::{PoolConfig, TransactionManager};

    struct NoCancel;

    impl DbCancel for NoCancel {
        fn cancel(&self) -> Result<(), LibError> {
            Ok(())
        }
    }

    struct FakeConn;

    impl DbConnection for FakeConn {
        fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, LibError> {
            Ok(0)
        }

        fn query(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, LibError> {
            Ok(Vec::new())
        }

        fn query_once(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, LibError> {
            Ok(Vec::new())
        }

        fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), LibError> {
            Ok(())
        }

        fn next_row(&mut self) -> Result<Option<DbRow>, LibError> {
            Ok(None)
        }

        fn close_stream(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn stream_open(&self) -> bool {
            false
        }

        fn begin(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn commit(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn rollback(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn savepoint(&mut self, _name: &str) -> Result<(), LibError> {
            Ok(())
        }

        fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), LibError> {
            Ok(())
        }

        fn release_savepoint(&mut self, _name: &str) -> Result<(), LibError> {
            Ok(())
        }

        fn canceller(&self) -> Box<dyn DbCancel> {
            Box::new(NoCancel)
        }
    }

    struct FakeDriver;

    impl DbDriver for FakeDriver {
        fn connect(&self) -> Result<Box<dyn DbConnection>, LibError> {
            Ok(Box::new(FakeConn))
        }
    }

    fn facade() -> ResourceFacade {
        let driver: Arc<dyn DbDriver> = Arc::new(FakeDriver);
        let transactions = TransactionManager::new(
            driver,
            PoolConfig {
                min: 0,
                max: 2,
                reserve: 1,
                acquire_timeout: Duration::from_secs(5),
                ..Default::default()
            },
        )
        .unwrap();
        let entity = EntityFacade::open(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework"),
            "transactional",
            vec![EntityGroup::new("transactional", "postgres", transactions)],
        )
        .unwrap();
        ResourceFacade::new(
            PathBuf::from("/tmp/moqui-resource-test"),
            HashMap::new(),
            Arc::new(RwLock::new(Some(entity))),
        )
    }

    fn empty_facade() -> ResourceFacade {
        ResourceFacade::new(
            PathBuf::from("/tmp/moqui-resource-test"),
            HashMap::new(),
            Arc::new(RwLock::new(None)),
        )
    }

    #[test]
    fn dbresource_without_facade_is_not_available() {
        let err = empty_facade().get("dbresource://notes/hello.txt").unwrap_err();
        assert!(err.to_string().contains("not available"), "{err}");
    }

    #[test]
    fn missing_dbresource_names_the_location() {
        let resources = facade();
        let root = resources.get("dbresource://").unwrap();
        assert!(root.exists());
        assert!(root.is_directory());
        let location = "dbresource://notes/hello.txt";
        let err = resources.get(location).unwrap_err();
        assert!(err.to_string().contains("resource not found"), "{err}");
        assert!(err.to_string().contains(location), "{err}");
    }

    #[test]
    fn db_scheme_stays_unsupported() {
        let err = empty_facade().get("db://notes/hello.txt").unwrap_err();
        assert!(err.to_string().contains("scheme db"), "{err}");
        let err = facade().get("db://notes/hello.txt").unwrap_err();
        assert!(err.to_string().contains("scheme db"), "{err}");
    }
}
