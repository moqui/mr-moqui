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

//! The Rust body of `org.moqui.impl.ElFinderServices.run#Command`, a port of `ElFinderConnector.groovy`: the elFinder
//! 2.0 client-server commands over one volume rooted at `resourceRoot`. A `dbresource://` root reads and writes the
//! `DbResource` rows. A `file`, `runtime`, `component` or `classpath` root is the directory the resource facade
//! resolves it to. Any other scheme, `content://` included, is an error that names the location.
//!
//! A target is the path under the root in URL-safe Base64 with no padding, after a 3 character volume ID, as Java
//! hashes it, so the client's hashes stay valid. A hash that does not decode, or a path with a `..` segment, a backslash
//! turned slash, or a part with a colon, is an elFinder error response, never the root. `rm`, `rename` and `put` refuse
//! the root itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::alphabet::URL_SAFE;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;
use crate::web::multipart::UploadedFile;

#[cfg(test)]
const RUN_COMMAND: &str = "org.moqui.impl.ElFinderServices.run#Command";
/// The error for a target that does not decode or that leaves the volume.
const INVALID_TARGET: &str = "errFileNotFound";
/// The error for a destructive command on the volume root.
const ROOT_REFUSED: &str = "Operation not allowed on the root";

/// Decodes a hash with or without its padding.
const HASH_DECODE: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, run_command)?;
    Ok(())
}

fn run_command(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| match parameters.get(key) {
        Some(value) if !is_empty(value) => to_plain_string(value),
        _ => String::new(),
    };
    let resource_root = Some(text("resourceRoot"))
        .filter(|root| !root.is_empty())
        .unwrap_or_else(|| "dbresource://".to_string());
    let volume_id = Some(text("volumeId"))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| "db_".to_string());
    let connector = Connector {
        facade,
        resource_root,
        volume_id,
    };
    let cmd = text("cmd");
    let target = text("target");
    let other = match parameters.get("otherParameters") {
        Some(Value::Map(map)) => map.clone(),
        _ => IndexMap::new(),
    };
    tracing::info!(
        "elFinder cmd={cmd}, target={target}:{:?}:{:?}",
        unhash(&target),
        connector.location(&target)
    );
    connector.run_command(&cmd, target, &other)
}

/// Where an entry's bytes live.
#[derive(Clone, Debug)]
enum Store {
    Db,
    File(PathBuf),
}

/// One file or directory under the volume, by its logical location.
#[derive(Clone, Debug)]
struct Entry {
    location: String,
    store: Store,
}

struct Connector<'a> {
    facade: &'a EntityFacade,
    resource_root: String,
    volume_id: String,
}

impl Connector<'_> {
    /// In production, only `dbresource://` (and `content://`) roots accept write commands. Component and file roots
    /// stay browsable.
    fn writes_restricted(&self) -> bool {
        writes_restricted(crate::web::is_production(), &self.resource_root)
    }

    fn hash(&self, path: &str) -> String {
        format!("{}{}", self.volume_id, URL_SAFE_NO_PAD.encode(path.as_bytes()))
    }

    /// The location a hash names. An empty hash is the root. `None` for a hash that does not decode or a path that
    /// leaves the volume.
    fn location(&self, hashed: &str) -> Option<String> {
        if hashed.is_empty() {
            return Some(self.resource_root.clone());
        }
        self.join_under_root(&unhash(hashed)?)
    }

    /// Join a relative path onto the root. A parent segment or a part with a colon is `None`.
    fn join_under_root(&self, path: &str) -> Option<String> {
        let root = &self.resource_root;
        if path.is_empty() || path == "/" || path == "root" {
            return Some(root.clone());
        }
        let normalized = path.replace('\\', "/");
        let mut safe = Vec::new();
        for part in normalized.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." || part.contains(':') {
                tracing::warn!("ElFinder path rejected under {root}: {path}");
                return None;
            }
            safe.push(part);
        }
        if safe.is_empty() {
            return Some(root.clone());
        }
        Some(format!(
            "{root}{}{}",
            if root.ends_with('/') { "" } else { "/" },
            safe.join("/")
        ))
    }

    fn relative_to_root(&self, location: &str) -> String {
        let Some(path) = location.trim().strip_prefix(self.resource_root.as_str()) else {
            tracing::warn!(
                "Location [{location}] does not start resourceRoot [{}]! Returning full location as relative path to root",
                self.resource_root
            );
            return location.to_string();
        };
        let path = path.strip_suffix('/').unwrap_or(path);
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.is_empty() {
            "root".to_string()
        } else {
            path.to_string()
        }
    }

    fn is_root(&self, location: &str) -> bool {
        self.relative_to_root(location) == "root"
    }

    fn entry(&self, location: &str) -> Result<Entry, LibError> {
        if location.starts_with("dbresource:") {
            return Ok(Entry {
                location: location.to_string(),
                store: Store::Db,
            });
        }
        let path = ExecutionContext::with_current(|ec| {
            let resource = ec
                .factory()
                .resource()
                .get(location)
                .map_err(|err| LibError::new(err.message()))?;
            resource.file_path().map(Path::to_path_buf).ok_or_else(|| {
                LibError::new(format!(
                    "elFinder location is not a dbresource or file location: {location}"
                ))
            })
        })
        .ok_or_else(|| LibError::new("elFinder has no execution context"))??;
        Ok(Entry {
            location: location.to_string(),
            store: Store::File(path),
        })
    }

    fn child(&self, parent: &Entry, name: &str) -> Entry {
        let location = join_location(&parent.location, name);
        let store = match &parent.store {
            Store::Db => Store::Db,
            Store::File(path) => Store::File(path.join(name)),
        };
        Entry { location, store }
    }

    fn is_directory(&self, entry: &Entry) -> Result<bool, LibError> {
        match &entry.store {
            Store::Db => Ok(self
                .facade
                .stat_db_resource(&entry.location)?
                .is_some_and(|stat| !stat.is_file)),
            Store::File(path) => Ok(path.is_dir()),
        }
    }

    /// The children ordered by name, as the `DbResource` find orders them.
    fn children(&self, entry: &Entry) -> Result<Vec<Entry>, LibError> {
        let mut names = match &entry.store {
            Store::Db => {
                let info = self.facade.read_db_resource(&entry.location)?;
                if !info.exists || info.is_file {
                    return Ok(Vec::new());
                }
                info.children.into_iter().map(|child| child.name).collect::<Vec<_>>()
            }
            Store::File(path) => {
                if !path.is_dir() {
                    return Ok(Vec::new());
                }
                let mut names = Vec::new();
                for item in std::fs::read_dir(path).map_err(|err| io_error(&entry.location, &err))? {
                    let item = item.map_err(|err| io_error(&entry.location, &err))?;
                    names.push(item.file_name().to_string_lossy().into_owned());
                }
                names
            }
        };
        names.sort();
        Ok(names.iter().map(|name| self.child(entry, name)).collect())
    }

    fn has_child_directories(&self, entry: &Entry) -> Result<bool, LibError> {
        if !self.is_directory(entry)? {
            return Ok(false);
        }
        for child in self.children(entry)? {
            if self.is_directory(&child)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The elFinder file object: name, hash, the volume or parent hash, type, time, size, and the flags.
    fn info(&self, entry: &Entry) -> Result<Value, LibError> {
        let root = self.is_root(&entry.location);
        let relative = self.relative_to_root(&entry.location);
        let (is_dir, last_modified, size) = match &entry.store {
            Store::Db => {
                let stat = self.facade.stat_db_resource(&entry.location)?;
                match stat {
                    Some(stat) => (!stat.is_file, stat.last_updated.unwrap_or(0), stat.size),
                    None => (false, 0, 0),
                }
            }
            Store::File(path) => {
                let meta = std::fs::metadata(path).ok();
                let millis = meta
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
                let size = meta
                    .as_ref()
                    .filter(|meta| meta.is_file())
                    .map_or(0, std::fs::Metadata::len);
                (path.is_dir(), millis, size)
            }
        };
        let mut info = IndexMap::new();
        let name = if root {
            self.resource_root
                .strip_suffix('/')
                .unwrap_or(&self.resource_root)
                .to_string()
        } else {
            file_name(&entry.location).to_string()
        };
        info.insert("name".to_string(), Value::string(name));
        info.insert("hash".to_string(), Value::string(self.hash(&relative)));
        if root {
            info.insert("volumeid".to_string(), Value::string(&self.volume_id));
        } else {
            let parent = relative.rsplit_once('/').map_or("root", |(parent, _)| parent);
            info.insert("phash".to_string(), Value::string(self.hash(parent)));
        }
        let mime = if root || is_dir {
            "directory"
        } else {
            moqui_lib::resource::content_type_for_name(file_name(&entry.location)).unwrap_or("application/octet-stream")
        };
        info.insert("mime".to_string(), Value::string(mime));
        info.insert("ts".to_string(), Value::I64(last_modified));
        info.insert("size".to_string(), Value::I64(i64::try_from(size).unwrap_or(i64::MAX)));
        let dirs = if self.has_child_directories(entry)? { 1 } else { 0 };
        info.insert("dirs".to_string(), Value::I64(dirs));
        info.insert("read".to_string(), Value::I64(1));
        // Java also asks `supportsWrite()`, which is true for every location an entry can be: a `DbResource` or a file.
        info.insert(
            "write".to_string(),
            Value::I64(if self.writes_restricted() { 0 } else { 1 }),
        );
        info.insert("locked".to_string(), Value::I64(0));
        Ok(Value::Map(info))
    }

    fn location_info(&self, location: &str) -> Result<Value, LibError> {
        self.info(&self.entry(location)?)
    }

    fn files(&self, location: &str, tree: bool) -> Result<Vec<Value>, LibError> {
        let mut files = Vec::new();
        let current = self.entry(location)?;
        if self.is_directory(&current)? || self.is_root(&current.location) {
            files.push(self.info(&current)?);
        }
        if tree {
            let root = self.entry(&self.resource_root)?;
            files.extend(self.tree(&root, 0)?);
        }
        for child in self.children(&current)? {
            let info = self.info(&child)?;
            if !files.contains(&info) {
                files.push(info);
            }
        }
        Ok(files)
    }

    fn tree(&self, entry: &Entry, deep: u32) -> Result<Vec<Value>, LibError> {
        let mut dirs = Vec::new();
        for child in self.children(entry)? {
            if self.is_directory(&child)? {
                dirs.push(self.info(&child)?);
                if deep > 0 {
                    dirs.extend(self.tree(&child, deep - 1)?);
                }
            }
        }
        Ok(dirs)
    }

    fn parents(&self, location: &str) -> Result<Vec<Value>, LibError> {
        let start = self.entry(location)?;
        let mut tree: Vec<Value> = Vec::new();
        let mut dir = location.to_string();
        while !self.is_root(&dir) {
            let Some(parent) = parent_location(&dir) else {
                tracing::warn!("Got null parent for [{dir}], starting location [{location}]");
                break;
            };
            dir = parent;
            let entry = self.entry(&dir)?;
            tree.insert(0, self.info(&entry)?);
            for info in self.tree(&entry, 0)? {
                if !tree.contains(&info) {
                    tree.push(info);
                }
            }
        }
        if tree.is_empty() {
            tree.push(self.info(&start)?);
        }
        Ok(tree)
    }

    fn options(&self, location: &str) -> Value {
        let mut options = IndexMap::new();
        options.insert("seperator".to_string(), Value::string("/"));
        options.insert("path".to_string(), Value::string(location));
        let mut disabled = vec![
            "tmb",
            "size",
            "dim",
            "duplicate",
            "paste",
            "archive",
            "extract",
            "search",
            "resize",
            "netmount",
        ];
        if self.writes_restricted() {
            disabled.extend(["rm", "rename", "mkdir", "mkfile", "upload", "edit"]);
        }
        options.insert(
            "disabled".to_string(),
            Value::List(disabled.into_iter().map(Value::string).collect()),
        );
        Value::Map(options)
    }

    fn delete(&self, location: &str) -> Result<Vec<Value>, LibError> {
        let entry = self.entry(location)?;
        if self.is_own_directory(&entry)? {
            return self.delete_dir(&entry);
        }
        let mut deleted = Vec::new();
        if self.delete_entry(&entry)? {
            deleted.push(Value::string(self.hash(&self.relative_to_root(location))));
        }
        Ok(deleted)
    }

    /// A directory to delete the contents of. A symbolic link to a directory is not one: the link is
    /// removed, and what it points to, which may be outside the volume, is left alone.
    fn is_own_directory(&self, entry: &Entry) -> Result<bool, LibError> {
        match &entry.store {
            Store::Db => self.is_directory(entry),
            Store::File(path) => Ok(std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())),
        }
    }

    fn delete_dir(&self, dir: &Entry) -> Result<Vec<Value>, LibError> {
        let mut deleted = Vec::new();
        for child in self.children(dir)? {
            if self.is_own_directory(&child)? {
                deleted.extend(self.delete_dir(&child)?);
            } else if self.delete_entry(&child)? {
                deleted.push(Value::string(self.hash(&self.relative_to_root(&child.location))));
            }
        }
        if self.delete_entry(dir)? {
            deleted.push(Value::string(self.hash(&self.relative_to_root(&dir.location))));
        }
        Ok(deleted)
    }

    /// One file or empty directory, as `ResourceReference.delete`. The root of a `dbresource` volume is not deleted.
    fn delete_entry(&self, entry: &Entry) -> Result<bool, LibError> {
        match &entry.store {
            Store::Db => {
                if db_path_is_empty(&entry.location) {
                    return Ok(false);
                }
                self.facade.delete_db_resource(&entry.location)
            }
            Store::File(path) => Ok(if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
                std::fs::remove_dir(path).is_ok()
            } else {
                std::fs::remove_file(path).is_ok()
            }),
        }
    }

    fn make_directory(&self, dir: &Entry, name: &str) -> Result<Entry, LibError> {
        let entry = self.child(dir, name);
        match &entry.store {
            Store::Db => self.facade.make_db_directory(&entry.location)?,
            Store::File(path) => {
                if !path.is_dir() {
                    std::fs::create_dir(path).map_err(|err| io_error(&entry.location, &err))?;
                }
            }
        }
        Ok(entry)
    }

    fn make_file(&self, dir: &Entry, name: &str) -> Result<Entry, LibError> {
        let entry = self.child(dir, name);
        match &entry.store {
            Store::Db => self.facade.write_db_resource(&entry.location, &[])?,
            Store::File(path) => {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .map_err(|err| io_error(&entry.location, &err))?;
            }
        }
        Ok(entry)
    }

    fn put_bytes(&self, entry: &Entry, bytes: &[u8]) -> Result<(), LibError> {
        match &entry.store {
            Store::Db => self.facade.write_db_resource(&entry.location, bytes),
            Store::File(path) => std::fs::write(path, bytes).map_err(|err| io_error(&entry.location, &err)),
        }
    }

    fn read_text(&self, entry: &Entry) -> Result<String, LibError> {
        let bytes = match &entry.store {
            Store::Db => {
                let info = self.facade.read_db_resource(&entry.location)?;
                if info.exists && info.is_file {
                    info.bytes
                } else {
                    Vec::new()
                }
            }
            Store::File(path) => {
                if path.is_file() {
                    std::fs::read(path).map_err(|err| io_error(&entry.location, &err))?
                } else {
                    Vec::new()
                }
            }
        };
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn move_to(&self, entry: &Entry, new_location: &str) -> Result<(), LibError> {
        match &entry.store {
            Store::Db => self.facade.move_db_resource(&entry.location, new_location),
            Store::File(path) => {
                let Store::File(new_path) = self.entry(new_location)?.store else {
                    return Err(LibError::new(format!(
                        "Location [{new_location}] is not a file location, not moving resource at {}",
                        entry.location
                    )));
                };
                std::fs::rename(path, new_path).map_err(|err| io_error(&entry.location, &err))
            }
        }
    }

    fn run_command(
        &self,
        cmd: &str,
        mut target: String,
        other: &IndexMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, LibError> {
        let other_text = |key: &str| match other.get(key) {
            Some(value) if !is_empty(value) => to_plain_string(value),
            _ => String::new(),
        };
        let mut out = BTreeMap::new();
        let mut response = IndexMap::new();
        let fail = |response: &mut IndexMap<String, Value>, message: &str| {
            response.clear();
            response.insert("error".to_string(), Value::string(message));
        };
        let write_refused = |response: &mut IndexMap<String, Value>| {
            if self.writes_restricted() {
                fail(response, "Write not allowed for this resource root");
                true
            } else {
                false
            }
        };
        // An empty target is the root. rm reads only targets[], each resolved below.
        let location = match self.location(&target) {
            Some(location) => location,
            None if cmd == "rm" => String::new(),
            None => {
                fail(&mut response, INVALID_TARGET);
                out.insert("responseMap".to_string(), Value::Map(response));
                return Ok(out);
            }
        };
        match cmd {
            "file" => {
                out.insert("fileLocation".to_string(), Value::string(&location));
                out.insert("fileInline".to_string(), Value::Bool(other_text("download") != "1"));
            }
            "open" => {
                let init = other_text("init") == "1";
                let tree = other_text("tree") == "1";
                if init {
                    response.insert("api".to_string(), Value::string("2.0"));
                    response.insert("netDrivers".to_string(), Value::List(Vec::new()));
                    if target.is_empty() {
                        target = self.hash("root");
                    }
                }
                if target.is_empty() {
                    fail(&mut response, "File not found");
                } else {
                    response.insert("uplMaxSize".to_string(), Value::string("32M"));
                    response.insert("cwd".to_string(), self.location_info(&location)?);
                    response.insert("files".to_string(), Value::List(self.files(&location, tree)?));
                    response.insert("options".to_string(), self.options(&location));
                }
            }
            "tree" => {
                if target.is_empty() {
                    fail(&mut response, "errOpen");
                } else {
                    let entry = self.entry(&location)?;
                    let mut tree = vec![self.info(&entry)?];
                    tree.extend(self.tree(&entry, 0)?);
                    response.insert("tree".to_string(), Value::List(tree));
                }
            }
            "parents" => {
                response.insert("tree".to_string(), Value::List(self.parents(&location)?));
            }
            "ls" => {
                if target.is_empty() {
                    fail(&mut response, "errOpen");
                } else {
                    let dir = self.entry(&location)?;
                    let list = self
                        .children(&dir)?
                        .iter()
                        .map(|child| Value::string(file_name(&child.location)))
                        .collect();
                    response.insert("list".to_string(), Value::List(list));
                }
            }
            "mkdir" | "mkfile" => 'make: {
                if write_refused(&mut response) {
                    break 'make;
                }
                let name = safe_entry_name(&other_text("name"));
                if target.is_empty() {
                    fail(&mut response, "errOpen");
                    break 'make;
                }
                let Some(name) = name else {
                    let what = if cmd == "mkdir" { "directory" } else { "file" };
                    fail(&mut response, &format!("No name specified for new {what}"));
                    break 'make;
                };
                // Java's `supportsWrite()` refusal never applies: an entry is a `DbResource` or a file.
                let dir = self.entry(&location)?;
                let made = if cmd == "mkdir" {
                    self.make_directory(&dir, &name)?
                } else {
                    self.make_file(&dir, &name)?
                };
                response.insert("added".to_string(), Value::List(vec![self.info(&made)?]));
            }
            "rm" => 'rm: {
                if write_refused(&mut response) {
                    break 'rm;
                }
                let targets = match other.get("targets").filter(|value| !is_empty(value)) {
                    Some(value) => value.clone(),
                    None => other.get("targets[]").cloned().unwrap_or(Value::Null),
                };
                let targets = match targets {
                    Value::List(items) => items.iter().map(to_plain_string).collect(),
                    other => vec![to_plain_string(&other)],
                };
                // Every target resolves, and none is the root, before anything is deleted.
                let mut locations = Vec::new();
                for current in targets {
                    let Some(location) = self.location(&current) else {
                        fail(&mut response, INVALID_TARGET);
                        break 'rm;
                    };
                    if self.is_root(&location) {
                        fail(&mut response, ROOT_REFUSED);
                        break 'rm;
                    }
                    locations.push(location);
                }
                let mut removed = Vec::new();
                for location in locations {
                    tracing::info!("ElFinder rm {location}");
                    removed.extend(self.delete(&location)?);
                }
                response.insert("removed".to_string(), Value::List(removed));
            }
            "rename" => 'rename: {
                if write_refused(&mut response) {
                    break 'rename;
                }
                let name = safe_entry_name(&other_text("name"));
                if target.is_empty() {
                    fail(&mut response, "errOpen");
                    break 'rename;
                }
                let Some(name) = name else {
                    fail(&mut response, "No name specified for new directory");
                    break 'rename;
                };
                if self.is_root(&location) {
                    fail(&mut response, ROOT_REFUSED);
                    break 'rename;
                }
                let new_location = format!("{}{name}", location.rfind('/').map_or("", |slash| &location[..=slash]));
                self.move_to(&self.entry(&location)?, &new_location)?;
                response.insert(
                    "added".to_string(),
                    Value::List(vec![self.location_info(&new_location)?]),
                );
                response.insert("removed".to_string(), Value::List(vec![Value::string(&target)]));
            }
            "upload" => 'upload: {
                if write_refused(&mut response) {
                    break 'upload;
                }
                if target.is_empty() {
                    fail(&mut response, "errOpen");
                    break 'upload;
                }
                let dir = self.entry(&location)?;
                let mut added = Vec::new();
                for file in uploads(other) {
                    let Some(name) = safe_entry_name(&file.file_name) else {
                        continue;
                    };
                    tracing::info!("ElFinder upload {name} to {}", dir.location);
                    let entry = self.child(&dir, &name);
                    if self.is_root(&entry.location) {
                        fail(&mut response, ROOT_REFUSED);
                        break 'upload;
                    }
                    self.put_bytes(&entry, &file.bytes)?;
                    added.push(self.info(&entry)?);
                }
                response.insert("added".to_string(), Value::List(added));
            }
            "get" => {
                let entry = self.entry(&location)?;
                response.insert("content".to_string(), Value::string(self.read_text(&entry)?));
            }
            "put" => 'put: {
                if write_refused(&mut response) {
                    break 'put;
                }
                if self.is_root(&location) {
                    fail(&mut response, ROOT_REFUSED);
                    break 'put;
                }
                let entry = self.entry(&location)?;
                self.put_bytes(&entry, other_text("content").as_bytes())?;
                response.insert("changed".to_string(), Value::List(vec![self.info(&entry)?]));
            }
            _ => {}
        }
        out.insert("responseMap".to_string(), Value::Map(response));
        Ok(out)
    }
}

fn writes_restricted(production: bool, resource_root: &str) -> bool {
    production && !(resource_root.starts_with("dbresource://") || resource_root.starts_with("content://"))
}

/// The path a hash names, or `None` for anything that is not a hash of a UTF-8 path. The volume ID alone is the empty
/// path, the root, as Java decodes it.
fn unhash(hashed: &str) -> Option<String> {
    // NOTE: assumes a volume ID prefix with 3 characters
    let encoded = hashed.get(3..)?;
    let encoded = encoded.replace('.', "=").replace('+', "-").replace('/', "_");
    let bytes = HASH_DECODE.decode(encoded.as_bytes()).ok()?;
    String::from_utf8(bytes).ok()
}

/// The last part of a client-given name, or `None` for an empty name, `.`, `..`, or a name with a colon.
fn safe_entry_name(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." || name.contains(':') {
        return None;
    }
    Some(name.to_string())
}

/// The uploaded files of the request, `_fileUploadList` of `ec.web.parameters`.
fn uploads(other: &IndexMap<String, Value>) -> Vec<std::sync::Arc<UploadedFile>> {
    let Some(Value::List(items)) = other.get("_fileUploadList") else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Object(object) => object.downcast::<UploadedFile>(),
            _ => None,
        })
        .collect()
}

fn file_name(location: &str) -> &str {
    let trimmed = location.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn join_location(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// The location one level up, as `ResourceReference.getParent`: `None` at the top of a scheme.
fn parent_location(location: &str) -> Option<String> {
    let current = location.strip_suffix('/').unwrap_or(location);
    let prefix_len = match current.find("://") {
        Some(index) => index + 3,
        None => current.find(':').map_or(0, |index| index + 1),
    };
    let stripped = &current[prefix_len..];
    if stripped.is_empty() {
        return None;
    }
    let stripped = stripped.strip_prefix('/').unwrap_or(stripped);
    if stripped.contains('/') {
        current.rfind('/').map(|slash| current[..slash].to_string())
    } else {
        Some(current[..prefix_len].to_string())
    }
}

fn db_path_is_empty(location: &str) -> bool {
    location
        .strip_prefix("dbresource:")
        .is_some_and(|rest| rest.trim_matches('/').is_empty())
}

fn io_error(location: &str, err: &std::io::Error) -> LibError {
    LibError::new(format!("{err}: {location}"))
}

#[cfg(test)]
mod tests;
