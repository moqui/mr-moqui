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

//! `dbresource://` rows in `moqui.resource.DbResource` and `DbResourceFile`.

use std::sync::atomic::Ordering;

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};

use super::facade::EntityFacade;
use super::value::EntityValue;

mod changes;

const DB_RESOURCE: &str = "moqui.resource.DbResource";
const DB_FILE: &str = "moqui.resource.DbResourceFile";
const DB_HISTORY: &str = "moqui.resource.DbResourceFileHistory";
const SEQUENCE: &str = "moqui.entity.SequenceValueItem";

/// Content over the conf's `resource-facade` `dbresource-blob-threshold` bytes (64 KiB) is a content blob (`crate::resource::blob`), not
/// `fileData`: stored once, read a window at a time, and a past version of it keeps the blob rather than a copy.
fn blob_threshold() -> u64 {
    crate::resource::setting("dbresource-blob-threshold")
        .parse::<u64>()
        .unwrap_or(64 * 1024)
}

/// What a write stores.
enum NewContent<'a> {
    Inline(&'a [u8]),
    Blob(crate::resource::blob::BlobInfo),
}

impl NewContent<'_> {
    fn size(&self) -> u64 {
        match self {
            NewContent::Inline(bytes) => bytes.len() as u64,
            NewContent::Blob(info) => info.size,
        }
    }
}

/// One `dbresource` node. `exists` is false when a path segment has no row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbResourceInfo {
    pub exists: bool,
    pub is_file: bool,
    pub bytes: Vec<u8>,
    pub children: Vec<DbResourceChild>,
}

/// A child name under a `dbresource` directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbResourceChild {
    pub name: String,
    pub is_file: bool,
}

/// What a file browser shows of a `dbresource` node: its kind, its last update in milliseconds, and its size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbResourceStat {
    pub is_file: bool,
    pub last_updated: Option<i64>,
    pub size: u64,
}

/// One `DbResourceFileHistory` row, as Java's `ResourceReference.Version`: the facade's [`ResourceVersion`].
///
/// [`ResourceVersion`]: crate::context::ResourceVersion
pub type DbResourceVersion = crate::context::ResourceVersion;

struct Node {
    id: String,
    is_file: bool,
}

/// A `dbresource` file opened to read a window at a time: what it is, and its content.
pub type OpenedDbFile = (DbResourceStat, std::sync::Arc<dyn crate::resource::source::ByteSource>);

impl EntityFacade {
    /// Read a `dbresource://` location. A missing row returns `exists: false` and does not error.
    pub fn read_db_resource(&self, location: &str) -> Result<DbResourceInfo, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        if parts.is_empty() {
            return Ok(DbResourceInfo {
                exists: true,
                is_file: false,
                bytes: Vec::new(),
                children: self.list_children(None)?,
            });
        }
        let mut parent: Option<String> = None;
        for (index, segment) in parts.iter().enumerate() {
            let Some(node) = self.find_node(parent.as_deref(), segment)? else {
                return Ok(DbResourceInfo {
                    exists: false,
                    is_file: false,
                    bytes: Vec::new(),
                    children: Vec::new(),
                });
            };
            if index + 1 == parts.len() {
                if node.is_file {
                    return Ok(DbResourceInfo {
                        exists: true,
                        is_file: true,
                        bytes: self.read_bytes(&node.id)?,
                        children: Vec::new(),
                    });
                }
                return Ok(DbResourceInfo {
                    exists: true,
                    is_file: false,
                    bytes: Vec::new(),
                    children: self.list_children(Some(&node.id))?,
                });
            }
            parent = Some(node.id);
        }
        Ok(DbResourceInfo {
            exists: false,
            is_file: false,
            bytes: Vec::new(),
            children: Vec::new(),
        })
    }

    /// [`EntityFacade::read_db_resource`] without a file's bytes, which
    /// [`EntityFacade::read_db_resource_bytes`] reads when they are wanted.
    pub fn db_resource_entry(&self, location: &str) -> Result<DbResourceInfo, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        if parts.is_empty() {
            return Ok(DbResourceInfo {
                exists: true,
                is_file: false,
                bytes: Vec::new(),
                children: self.list_children(None)?,
            });
        }
        Ok(match self.node_at(&parts)? {
            None => DbResourceInfo {
                exists: false,
                is_file: false,
                bytes: Vec::new(),
                children: Vec::new(),
            },
            Some(node) if node.is_file => DbResourceInfo {
                exists: true,
                is_file: true,
                bytes: Vec::new(),
                children: Vec::new(),
            },
            Some(node) => DbResourceInfo {
                exists: true,
                is_file: false,
                bytes: Vec::new(),
                children: self.list_children(Some(&node.id))?,
            },
        })
    }

    /// The bytes of the file at a `dbresource://` location, `None` when no file is there.
    pub fn read_db_resource_bytes(&self, location: &str) -> Result<Option<Vec<u8>>, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        match self.node_at(&parts)? {
            Some(node) if node.is_file => Ok(Some(self.read_bytes(&node.id)?)),
            _ => Ok(None),
        }
    }

    /// The file at a `dbresource://` location to read a window at a time: a content blob's chunks, or the row's
    /// bytes. `None` when no file is there.
    pub fn open_db_resource(&self, location: &str) -> Result<Option<OpenedDbFile>, Error> {
        let Some(stat) = self.stat_db_resource(location)?.filter(|stat| stat.is_file) else {
            return Ok(None);
        };
        let Some((resource_id, file)) = self.file_row(location)? else {
            return Ok(None);
        };
        let source: std::sync::Arc<dyn crate::resource::source::ByteSource> = match text_of(file.get("blobId")) {
            Some(blob_id) => {
                let blobs = crate::resource::blob::BlobStore::new(self.clone());
                let info = blobs
                    .stat(&blob_id)?
                    .ok_or_else(|| Error::new(format!("the content blob {blob_id} of {location} is missing")))?;
                blobs.source(&info)
            }
            None => std::sync::Arc::new(crate::resource::source::MemorySource::new(
                blob_bytes(file.get("fileData"), &resource_id)?.unwrap_or_default(),
            )),
        };
        Ok(Some((stat, source)))
    }

    /// [`EntityFacade::write_db_resource`] of what `input` gives (`size` bytes, when known): content over the blob
    /// threshold goes into a content blob as it is read, never all in memory.
    pub fn write_db_resource_from(
        &self,
        location: &str,
        input: &mut dyn std::io::Read,
        size: Option<u64>,
    ) -> Result<(), Error> {
        if size.is_some_and(|size| size > blob_threshold()) {
            let blobs = crate::resource::blob::BlobStore::new(self.clone());
            let expected = crate::resource::blob::Expected { id: None, size };
            let info = blobs.put(input, &expected, u64::MAX)?;
            return self.write_db_content(location, NewContent::Blob(info), None);
        }
        let mut bytes = Vec::new();
        input
            .read_to_end(&mut bytes)
            .map_err(|err| Error::new(format!("reading the content for {location}: {err}")))?;
        self.write_db_resource(location, &bytes)
    }

    /// Create or replace a `dbresource://` file. Missing parent directories are created. A file in the path, or a
    /// directory at the location, is an error, as `DbResourceReference.putObject`.
    pub fn write_db_resource(&self, location: &str, bytes: &[u8]) -> Result<(), Error> {
        self.write_db_resource_noted(location, bytes, None)
    }

    /// [`Self::write_db_resource`] with a note for the version it makes: `DbResourceFile.versionNote`, kept on the
    /// version's `DbResourceFileHistory` row too, so it stays with the version when a later write replaces it.
    pub fn write_db_resource_noted(&self, location: &str, bytes: &[u8], note: Option<&str>) -> Result<(), Error> {
        if bytes.len() as u64 > blob_threshold() {
            let info = crate::resource::blob::BlobStore::new(self.clone()).put_bytes(bytes)?;
            return self.write_db_content(location, NewContent::Blob(info), note);
        }
        self.write_db_content(location, NewContent::Inline(bytes), note)
    }

    fn write_db_content(&self, location: &str, content: NewContent<'_>, note: Option<&str>) -> Result<(), Error> {
        self.ensure_db_tables()?;
        let mut parts = db_parts(location)?;
        let Some(filename) = parts.pop() else {
            return Err(Error::new(format!("cannot write the dbresource root: {location}")));
        };
        let parent = self.directory_id(&parts)?;
        let id = match self.find_node(parent.as_deref(), &filename)? {
            Some(node) if !node.is_file => {
                return Err(Error::new(format!(
                    "Cannot write a file at {location}, it is a directory"
                )))
            }
            Some(node) => node.id,
            None => self.create_node(parent.as_deref(), &filename, true)?,
        };
        self.write_bytes(
            &id,
            &filename,
            content,
            note.map(str::trim).filter(|note| !note.is_empty()),
        )
    }

    /// The kind, time, and size of a `dbresource://` location, or `None` when a segment has no row. The root is a
    /// directory with no time. A file's time is its `DbResourceFile` update, as `DbResourceReference.getLastModified`.
    pub fn stat_db_resource(&self, location: &str) -> Result<Option<DbResourceStat>, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        if parts.is_empty() {
            return Ok(Some(DbResourceStat {
                is_file: false,
                last_updated: None,
                size: 0,
            }));
        }
        let Some(node) = self.node_at(&parts)? else {
            return Ok(None);
        };
        let resource = self
            .find(DB_RESOURCE)?
            .eq("resourceId", Value::string(&node.id))
            .one()?;
        let mut last_updated = resource
            .as_ref()
            .and_then(|row| stamp_millis(row.get("lastUpdatedStamp")));
        let mut size = 0;
        if node.is_file {
            if let Some(file) = self.find(DB_FILE)?.eq("resourceId", Value::string(&node.id)).one()? {
                last_updated = stamp_millis(file.get("lastUpdatedStamp")).or(last_updated);
                size = file_size_of(&file, &node.id)?;
            }
        }
        Ok(Some(DbResourceStat {
            is_file: node.is_file,
            last_updated,
            size,
        }))
    }

    /// Create the directories of a `dbresource://` location that are missing, as `findDirectoryId(path, true)`. A
    /// file in the path is an error.
    pub fn make_db_directory(&self, location: &str) -> Result<(), Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        self.directory_id(&parts).map(|_| ())
    }

    /// Delete a `dbresource://` node, with its file and history rows. A directory's children are not deleted here,
    /// as in `DbResourceReference.delete`. False when there is no row.
    pub fn delete_db_resource(&self, location: &str) -> Result<bool, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        if parts.is_empty() {
            return Err(Error::new(format!("cannot delete the dbresource root: {location}")));
        }
        let Some(node) = self.node_at(&parts)? else {
            return Ok(false);
        };
        if node.is_file {
            let histories = self
                .find(DB_HISTORY)?
                .eq("resourceId", Value::string(&node.id))
                .list()?;
            for history in histories.iter() {
                history.delete(self)?;
            }
            if let Some(file) = self.find(DB_FILE)?.eq("resourceId", Value::string(&node.id)).one()? {
                file.delete(self)?;
            }
        }
        let mut value = EntityValue::make(self, DB_RESOURCE)?;
        value.set("resourceId", Value::string(&node.id));
        value.delete(self)?;
        Ok(true)
    }

    /// Give a `dbresource://` node a new parent and name, creating the new parent directories, as
    /// `DbResourceReference.move`. Nothing happens when there is no row at `location`.
    pub fn move_db_resource(&self, location: &str, new_location: &str) -> Result<(), Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        let node = if parts.is_empty() { None } else { self.node_at(&parts)? };
        let Some(node) = node else {
            tracing::warn!("Could not find dbresource at [{location}]");
            return Ok(());
        };
        let mut new_parts = db_parts(new_location)?;
        let Some(new_name) = new_parts.pop() else {
            return Err(Error::new(format!(
                "No location specified, not moving resource at {location}"
            )));
        };
        let parent = self.directory_id(&new_parts)?;
        let mut value = EntityValue::make(self, DB_RESOURCE)?;
        value.set("resourceId", Value::string(&node.id));
        value.set("parentResourceId", parent.map_or(Value::Null, Value::string));
        value.set("filename", Value::string(new_name));
        value.set("isFile", Value::string(if node.is_file { "Y" } else { "N" }));
        value.update(self)?;
        Ok(())
    }

    /// The node at a path, walking from the root.
    fn node_at(&self, parts: &[String]) -> Result<Option<Node>, Error> {
        let mut parent: Option<String> = None;
        let mut found = None;
        for segment in parts {
            let Some(node) = self.find_node(parent.as_deref(), segment)? else {
                return Ok(None);
            };
            parent = Some(node.id.clone());
            found = Some(node);
        }
        Ok(found)
    }

    /// The ID of the directory at a path, creating each missing one. `None` is the root.
    fn directory_id(&self, parts: &[String]) -> Result<Option<String>, Error> {
        let mut parent: Option<String> = None;
        for segment in parts {
            if let Some(node) = self.find_node(parent.as_deref(), segment)? {
                if node.is_file {
                    return Err(Error::new(format!(
                        "Tried to find a directory in a path but found file instead at {segment} under DbResource {}",
                        parent.as_deref().unwrap_or("root")
                    )));
                }
                parent = Some(node.id);
            } else {
                parent = Some(self.create_node(parent.as_deref(), segment, false)?);
            }
        }
        Ok(parent)
    }

    /// Skip the table check, for the in-memory test database, which has every table and no `information_schema`.
    #[cfg(test)]
    pub(crate) fn assume_db_tables(&self) {
        self.inner.db_ready.store(true, Ordering::Release);
    }

    fn ensure_db_tables(&self) -> Result<(), Error> {
        if self.inner.db_ready.load(Ordering::Acquire) {
            return Ok(());
        }
        self.check_tables(&[SEQUENCE, DB_RESOURCE, DB_FILE, DB_HISTORY])?;
        self.inner.db_ready.store(true, Ordering::Release);
        Ok(())
    }

    fn find_node(&self, parent: Option<&str>, filename: &str) -> Result<Option<Node>, Error> {
        let mut find = self.find(DB_RESOURCE)?.eq("filename", Value::string(filename));
        find = match parent {
            Some(parent) => find.eq("parentResourceId", Value::string(parent)),
            None => find.condition(Condition::is_null("parentResourceId")),
        };
        let rows = find.list()?;
        let Some(row) = rows.get(0) else {
            return Ok(None);
        };
        let id = match row.get("resourceId").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => return Ok(None),
        };
        let is_file = row
            .get("isFile")
            .and_then(Value::as_str)
            .is_some_and(|flag| flag.eq_ignore_ascii_case("Y"));
        Ok(Some(Node { id, is_file }))
    }

    fn list_children(&self, parent: Option<&str>) -> Result<Vec<DbResourceChild>, Error> {
        let find = self.find(DB_RESOURCE)?;
        let find = match parent {
            Some(parent) => find.eq("parentResourceId", Value::string(parent)),
            None => find.condition(Condition::is_null("parentResourceId")),
        };
        let rows = find.order_by("filename", false).list()?;
        let mut children = Vec::new();
        for row in rows.iter() {
            let Some(name) = row.get("filename").and_then(Value::as_str) else {
                continue;
            };
            let is_file = row
                .get("isFile")
                .and_then(Value::as_str)
                .is_some_and(|flag| flag.eq_ignore_ascii_case("Y"));
            children.push(DbResourceChild {
                name: name.to_string(),
                is_file,
            });
        }
        Ok(children)
    }

    fn read_bytes(&self, resource_id: &str) -> Result<Vec<u8>, Error> {
        let row = self.find(DB_FILE)?.eq("resourceId", Value::string(resource_id)).one()?;
        let Some(row) = row else {
            return Ok(Vec::new());
        };
        self.file_bytes(&row, resource_id)
    }

    /// The bytes a file or history row holds: its content blob's, or its `fileData`; empty for none.
    fn file_bytes(&self, row: &EntityValue, resource_id: &str) -> Result<Vec<u8>, Error> {
        Ok(self.row_bytes(row, resource_id)?.unwrap_or_default())
    }

    /// The bytes a file or history row holds, `None` when it holds none.
    fn row_bytes(&self, row: &EntityValue, resource_id: &str) -> Result<Option<Vec<u8>>, Error> {
        match text_of(row.get("blobId")) {
            Some(blob_id) => crate::resource::blob::BlobStore::new(self.clone())
                .read(&blob_id)?
                .map(Some)
                .ok_or_else(|| {
                    Error::new(format!(
                        "the content blob {blob_id} of DbResource {resource_id} is missing"
                    ))
                }),
            None => blob_bytes(row.get("fileData"), resource_id),
        }
    }

    /// A new `DbResource` row under `parent`, its id.
    fn create_node(&self, parent: Option<&str>, filename: &str, is_file: bool) -> Result<String, Error> {
        let mut value = EntityValue::make(self, DB_RESOURCE)?;
        value.set_sequenced_id_primary(self)?;
        let id = value
            .get("resourceId")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("sequenced DbResource id is empty"))?
            .to_string();
        if let Some(parent) = parent {
            value.set("parentResourceId", Value::string(parent));
        }
        value.set("filename", Value::string(filename));
        value.set("isFile", Value::string(if is_file { "Y" } else { "N" }));
        value.create(self)?;
        Ok(id)
    }

    /// Write a file's bytes as `DbResourceReference.putObject`: the first write is version "01" with its history row
    /// and the MIME type of `filename`; a later write moves the replaced bytes into the current version's history row
    /// (as a reverse delta when [`past_payload`] allows) and adds the next version.
    fn write_bytes(
        &self,
        resource_id: &str,
        filename: &str,
        content: NewContent<'_>,
        note: Option<&str>,
    ) -> Result<(), Error> {
        let (file_data, blob_id) = match &content {
            NewContent::Inline(bytes) => (Value::Bytes(bytes.to_vec()), Value::Null),
            NewContent::Blob(info) => (Value::Null, Value::string(&info.id)),
        };
        let existing = self.find(DB_FILE)?.eq("resourceId", Value::string(resource_id)).one()?;
        let note_value = note.map_or(Value::Null, Value::string);
        let size = Value::I64(i64::try_from(content.size()).unwrap_or(i64::MAX));
        let Some(existing) = existing else {
            let mut value = EntityValue::make(self, DB_FILE)?;
            value.set("resourceId", Value::string(resource_id));
            if let Some(mime_type) = moqui_lib::resource::content_type_for_name(filename) {
                value.set("mimeType", Value::string(mime_type));
            }
            value.set("fileData", file_data);
            value.set("blobId", blob_id);
            value.set("fileSize", size);
            value.set("versionName", Value::string(FIRST_VERSION));
            value.set("rootVersionName", Value::string(FIRST_VERSION));
            value.set("versionNote", note_value.clone());
            value.create(self)?;
            // no fileData: a history row holds the bytes only once it is a past version
            let mut history = self.new_history(resource_id, None)?;
            history.set("versionName", Value::string(FIRST_VERSION));
            history.set("versionNote", note_value);
            history.create(self)?;
            return Ok(());
        };
        let current = text_of(existing.get("versionName"));
        // The next version's name first: the replaced version's delta is made against it.
        let mut history = self.new_history(resource_id, current.as_deref())?;
        history.set_sequenced_id_secondary(self)?;
        let next = text_of(history.get("versionName"))
            .ok_or_else(|| Error::new(format!("no next version name for DbResource {resource_id}")))?;
        if let Some(current) = &current {
            let past = self
                .find(DB_HISTORY)?
                .eq("resourceId", Value::string(resource_id))
                .eq("versionName", Value::string(current))
                .one()?;
            if past.is_some() {
                let mime = text_of(existing.get("mimeType"))
                    .or_else(|| moqui_lib::resource::content_type_for_name(filename).map(str::to_string));
                let mut past = EntityValue::make(self, DB_HISTORY)?;
                past.set("resourceId", Value::string(resource_id));
                past.set("versionName", Value::string(current));
                if let Some(old_blob) = text_of(existing.get("blobId")) {
                    // the past version keeps its blob, not a copy
                    past.set("isDiff", Value::string("N"));
                    past.set("fileData", Value::Null);
                    past.set("blobId", Value::string(old_blob));
                } else {
                    let (is_diff, data) = match (blob_bytes(existing.get("fileData"), resource_id)?, &content) {
                        (Some(old), NewContent::Inline(bytes)) => {
                            let (is_diff, data) = past_payload(mime.as_deref(), current, &next, &old, bytes);
                            (is_diff, Value::Bytes(data))
                        }
                        (Some(old), NewContent::Blob(_)) => (false, Value::Bytes(old)),
                        (None, _) => (false, Value::Null),
                    };
                    past.set("isDiff", Value::string(if is_diff { "Y" } else { "N" }));
                    past.set("fileData", data);
                }
                // the replaced version's note moves to its history row (a row written with it has it already)
                if let Some(past_note) = text_of(existing.get("versionNote")) {
                    past.set("versionNote", Value::string(past_note));
                }
                past.update(self)?;
            }
        }
        history.set("versionNote", note_value.clone());
        history.create(self)?;
        let mut value = EntityValue::make(self, DB_FILE)?;
        value.set("resourceId", Value::string(resource_id));
        value.set("fileData", file_data);
        value.set("blobId", blob_id);
        value.set("fileSize", size);
        value.set("versionNote", note_value);
        value.set("versionName", Value::string(&next));
        if text_of(existing.get("rootVersionName")).is_none() {
            value.set("rootVersionName", Value::string(current.unwrap_or(next)));
        }
        value.update(self)?;
        Ok(())
    }

    fn new_history(&self, resource_id: &str, previous: Option<&str>) -> Result<EntityValue, Error> {
        let mut history = EntityValue::make(self, DB_HISTORY)?;
        history.set("resourceId", Value::string(resource_id));
        history.set("previousVersionName", previous.map_or(Value::Null, Value::string));
        // a version copied with its history names its own author and date (crate::context::with_version_author)
        let author = crate::context::version_author().unwrap_or_default();
        let date = author
            .date
            .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
            .map_or_else(crate::user::now_effective_value, Value::Timestamp);
        history.set("versionDate", date);
        let user_id = author.user_id.or_else(|| crate::user::current_user_id().ok().flatten());
        history.set("userId", user_id.map_or(Value::Null, Value::string));
        history.set("isDiff", Value::string("N"));
        Ok(history)
    }

    /// The `DbResourceFile` row of the file at `location`, or `None` when there is no file there.
    fn file_row(&self, location: &str) -> Result<Option<(String, EntityValue)>, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        let node = if parts.is_empty() { None } else { self.node_at(&parts)? };
        let Some(node) = node.filter(|node| node.is_file) else {
            return Ok(None);
        };
        let row = self.find(DB_FILE)?.eq("resourceId", Value::string(&node.id)).one()?;
        Ok(row.map(|row| (node.id, row)))
    }

    /// A version of the file at `location`, as `DbResourceReference.getVersion`.
    pub fn db_resource_version(&self, location: &str, version_name: &str) -> Result<Option<DbResourceVersion>, Error> {
        let Some((resource_id, _)) = self.file_row(location)? else {
            return Ok(None);
        };
        let row = self
            .find(DB_HISTORY)?
            .eq("resourceId", Value::string(&resource_id))
            .eq("versionName", Value::string(version_name))
            .one()?;
        Ok(row.as_ref().map(version_of))
    }

    /// The current version of the file at `location`, `getCurrentVersion`.
    pub fn db_resource_current_version(&self, location: &str) -> Result<Option<DbResourceVersion>, Error> {
        self.named_version(location, "versionName")
    }

    /// The first version of the file at `location`, `getRootVersion`.
    pub fn db_resource_root_version(&self, location: &str) -> Result<Option<DbResourceVersion>, Error> {
        self.named_version(location, "rootVersionName")
    }

    fn named_version(&self, location: &str, field: &str) -> Result<Option<DbResourceVersion>, Error> {
        let Some((_, row)) = self.file_row(location)? else {
            return Ok(None);
        };
        match text_of(row.get(field)) {
            Some(name) => self.db_resource_version(location, &name),
            None => Ok(None),
        }
    }

    /// The versions of the file at `location` made from `version_name`, `getNextVersions`.
    pub fn db_resource_next_versions(
        &self,
        location: &str,
        version_name: &str,
    ) -> Result<Vec<DbResourceVersion>, Error> {
        let Some((resource_id, _)) = self.file_row(location)? else {
            return Ok(Vec::new());
        };
        let rows = self
            .find(DB_HISTORY)?
            .eq("resourceId", Value::string(&resource_id))
            .eq("previousVersionName", Value::string(version_name))
            .list()?;
        Ok(rows.iter().map(version_of).collect())
    }

    /// Every version of the file at `location`, newest first, `getVersionHistory`.
    pub fn db_resource_version_history(&self, location: &str) -> Result<Vec<DbResourceVersion>, Error> {
        let Some((resource_id, _)) = self.file_row(location)? else {
            return Ok(Vec::new());
        };
        let rows = self
            .find(DB_HISTORY)?
            .eq("resourceId", Value::string(&resource_id))
            .order_by("versionDate", true)
            .order_by("versionName", true)
            .list()?;
        Ok(rows.iter().map(version_of).collect())
    }

    /// The bytes of one version of the file at `location`, as `openStream(versionName)`: the history row's data, or
    /// the file's data when it is the current version. A version stored as a reverse delta (`isDiff="Y"`) is rebuilt
    /// from the nearest full copy after it; Java leaves that a TODO and answers null (Parity-Diffs, revision
    /// management). `None` when there is no such version, or when its delta chain is broken, which is logged.
    pub fn read_db_resource_version(&self, location: &str, version_name: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some((resource_id, file)) = self.file_row(location)? else {
            return Ok(None);
        };
        let Some(history) = self
            .find(DB_HISTORY)?
            .eq("resourceId", Value::string(&resource_id))
            .eq("versionName", Value::string(version_name))
            .one()?
        else {
            return Ok(None);
        };
        if text_of(history.get("isDiff")).as_deref() == Some("Y") {
            return match self.rebuild_version(&resource_id, &file, version_name) {
                Ok(text) => Ok(Some(text.into_bytes())),
                Err(err) => {
                    tracing::error!(
                        location,
                        version = version_name,
                        "cannot rebuild DbResource version: {err}"
                    );
                    Ok(None)
                }
            };
        }
        if let Some(bytes) = self.row_bytes(&history, &resource_id)? {
            return Ok(Some(bytes));
        }
        if text_of(file.get("versionName")).as_deref() != Some(version_name) {
            return Ok(None);
        }
        Ok(Some(self.file_bytes(&file, &resource_id)?))
    }

    /// The text of a delta-stored version: walk `previousVersionName` from the current version down to `target`, start
    /// from the nearest full text on that path (the file itself, or a past row stored whole), and apply each reverse
    /// delta from there down to `target`.
    fn rebuild_version(&self, resource_id: &str, file: &EntityValue, target: &str) -> Result<String, Error> {
        let rows: std::collections::HashMap<String, EntityValue> = self
            .find(DB_HISTORY)?
            .eq("resourceId", Value::string(resource_id))
            .list()?
            .into_values()
            .into_iter()
            .filter_map(|row| text_of(row.get("versionName")).map(|name| (name, row)))
            .collect();
        let current = text_of(file.get("versionName"))
            .ok_or_else(|| Error::new(format!("DbResource {resource_id} has no current version")))?;
        // newest first: path[0] is the current version, the last is `target`
        let mut path = vec![current];
        while path.last().map(String::as_str) != Some(target) {
            let name = path.last().map(String::as_str).unwrap_or_default();
            let previous = rows
                .get(name)
                .and_then(|row| text_of(row.get("previousVersionName")))
                .ok_or_else(|| Error::new(format!("version {target} is not before version {name}")))?;
            if path.len() > rows.len() || path.contains(&previous) {
                return Err(Error::new(format!("the version chain loops at {previous}")));
            }
            path.push(previous);
        }
        let full_text = |index: usize| -> Result<Option<String>, Error> {
            let bytes = if index == 0 {
                self.row_bytes(file, resource_id)?
            } else {
                let row = &rows[&path[index]];
                if text_of(row.get("isDiff")).as_deref() == Some("Y") {
                    return Ok(None);
                }
                self.row_bytes(row, resource_id)?
            };
            match bytes {
                Some(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| Error::new(format!("version {} is not UTF-8 text", path[index]))),
                None if index == 0 => Ok(Some(String::new())),
                None => Ok(None),
            }
        };
        let last = path.len() - 1;
        let mut start = None;
        for index in (0..last).rev() {
            if let Some(text) = full_text(index)? {
                start = Some((index, text));
                break;
            }
        }
        let (start, mut text) = start.ok_or_else(|| Error::new(format!("no full copy after version {target}")))?;
        for name in &path[start + 1..] {
            let row = &rows[name];
            if text_of(row.get("isDiff")).as_deref() != Some("Y") {
                return Err(Error::new(format!("version {name} is neither a delta nor a full copy")));
            }
            let patch = blob_bytes(row.get("fileData"), resource_id)?
                .ok_or_else(|| Error::new(format!("version {name} has no delta")))?;
            let patch =
                String::from_utf8(patch).map_err(|_| Error::new(format!("version {name}: delta is not UTF-8")))?;
            text = crate::resource::textdiff::apply_unified(&text, &patch)
                .map_err(|err| Error::new(format!("version {name}: {err}")))?;
        }
        Ok(text)
    }
}

impl EntityFacade {
    /// `migrate#DbResourceBlobs`: move the files and past versions written before content blobs, and over the blob
    /// threshold, into content blobs, and record the size of the others, until `deadline`. (moved, sized). A row that
    /// fails is logged and the others still move.
    pub fn migrate_db_resource_blobs(&self, deadline: std::time::Instant) -> Result<(u64, u64), Error> {
        self.ensure_db_tables()?;
        let blobs = crate::resource::blob::BlobStore::new(self.clone());
        let threshold = blob_threshold();
        let (mut moved, mut sized) = (0u64, 0u64);
        for entity in [DB_FILE, DB_HISTORY] {
            let rows = self.find(entity)?.condition(Condition::is_null("blobId")).list()?;
            for row in rows.iter() {
                if std::time::Instant::now() >= deadline {
                    return Ok((moved, sized));
                }
                let resource_id = text_of(row.get("resourceId")).unwrap_or_default();
                let Some(bytes) = blob_bytes(row.get("fileData"), &resource_id)? else {
                    continue;
                };
                let diff = text_of(row.get("isDiff")).as_deref() == Some("Y");
                let mut value = row.clone();
                if bytes.len() as u64 > threshold && !diff {
                    match blobs.put_bytes(&bytes) {
                        Ok(info) => {
                            value.set("blobId", Value::string(info.id));
                            value.set("fileData", Value::Null);
                            moved += 1;
                        }
                        Err(err) => {
                            tracing::error!("DbResource {resource_id} was not moved into a blob: {}", err.message());
                            continue;
                        }
                    }
                } else if entity == DB_FILE && row.get("fileSize").is_none_or(Value::is_null) {
                    sized += 1;
                } else {
                    continue;
                }
                if entity == DB_FILE {
                    value.set("fileSize", Value::I64(i64::try_from(bytes.len()).unwrap_or(i64::MAX)));
                }
                if let Err(err) = value.update(self) {
                    tracing::error!("DbResource {resource_id} was not updated: {}", err.message());
                }
            }
        }
        Ok((moved, sized))
    }
}

const FIRST_VERSION: &str = "01";

/// A past version is stored whole when its number is a multiple of this, so rebuilding any version applies at most
/// this many minus one deltas.
const FULL_COPY_EVERY: u32 = 20;

/// What a replaced version keeps in its history row: `(true, reverse delta)` or `(false, its bytes)`. The delta is a
/// unified diff from the `next` version's text back to `current`'s (`--- next` / `+++ current`), so the newest text
/// stays whole in `DbResourceFile` and an older one is rebuilt by walking back. Java has this as a TODO
/// (`DbResourceReference.putText`) and always stores the whole bytes. Stored whole: binary MIME types, bytes that are
/// not UTF-8, every [`FULL_COPY_EVERY`]th version, and a delta more than half the size of the text it replaces.
fn past_payload(mime: Option<&str>, current: &str, next: &str, old: &[u8], new: &[u8]) -> (bool, Vec<u8>) {
    let whole = || (false, old.to_vec());
    if mime.is_some_and(moqui_lib::resource::is_binary_content_type) {
        return whole();
    }
    if current
        .parse::<u32>()
        .map_or(true, |number| number % FULL_COPY_EVERY == 0)
    {
        return whole();
    }
    let (Ok(old_text), Ok(new_text)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return whole();
    };
    let patch = crate::resource::textdiff::unified(new_text, old_text, next, current);
    if patch.len() * 2 > old.len() {
        return whole();
    }
    (true, patch.into_bytes())
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        value => Some(moqui_lib::object::to_plain_string(value)).filter(|text| !text.is_empty()),
    }
}

fn version_of(row: &EntityValue) -> DbResourceVersion {
    DbResourceVersion {
        version_name: text_of(row.get("versionName")).unwrap_or_default(),
        previous_version_name: text_of(row.get("previousVersionName")),
        version_date: row.get("versionDate").cloned().unwrap_or(Value::Null),
        user_id: text_of(row.get("userId")),
        note: text_of(row.get("versionNote")),
    }
}

/// A file's size: `fileSize`, written with each write since it was added; a row written before it has its data measured
/// until its next write sets it.
fn file_size_of(file: &EntityValue, resource_id: &str) -> Result<u64, Error> {
    if let Value::I64(size) = file
        .get("fileSize")
        .map_or(Value::Null, |value| basic_convert(value, "Long").value)
    {
        return Ok(u64::try_from(size).unwrap_or(0));
    }
    Ok(blob_bytes(file.get("fileData"), resource_id)?.map_or(0, |bytes| bytes.len() as u64))
}

/// A `fileData` value as bytes; `None` when it is null.
fn blob_bytes(value: Option<&Value>, resource_id: &str) -> Result<Option<Vec<u8>>, Error> {
    let raw = value.cloned().unwrap_or(Value::Null);
    match basic_convert(&raw, "java.sql.Blob").value {
        Value::Bytes(bytes) => Ok(Some(bytes)),
        Value::String(text) => Ok(Some(text.into_bytes())),
        Value::Null => Ok(None),
        other => Err(Error::new(format!(
            "DbResource fileData for {resource_id} is {other:?}"
        ))),
    }
}

fn stamp_millis(value: Option<&Value>) -> Option<i64> {
    match basic_convert(value?, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

fn db_parts(location: &str) -> Result<Vec<String>, Error> {
    let rest = location
        .strip_prefix("dbresource://")
        .or_else(|| location.strip_prefix("dbresource:"))
        .ok_or_else(|| Error::new(format!("not a dbresource location: {location}")))?;
    let rest = rest.trim_start_matches('/');
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    Ok(rest
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use crate::entity::EntityFacade;

    fn facade() -> (EntityFacade, crate::context::ExecutionContext) {
        let facade = crate::testing::open();
        facade.assume_db_tables();
        let context = crate::context::ExecutionContextFactory::test_context();
        context.factory().set_entity(facade.clone());
        (facade, context)
    }

    #[test]
    fn first_write_is_version_01_with_its_history_row() {
        let (facade, context) = facade();
        let _entered = context.enter();
        facade.write_db_resource("dbresource://wiki/a.md", b"one").unwrap();
        let current = facade
            .db_resource_current_version("dbresource://wiki/a.md")
            .unwrap()
            .expect("current");
        assert_eq!(current.version_name, "01");
        assert_eq!(current.previous_version_name, None);
        assert!(!current.version_date.is_null());
        let root = facade
            .db_resource_root_version("dbresource://wiki/a.md")
            .unwrap()
            .expect("root");
        assert_eq!(root.version_name, "01");
        assert_eq!(
            facade.read_db_resource_version("dbresource://wiki/a.md", "01").unwrap(),
            Some(b"one".to_vec())
        );
    }

    #[test]
    fn a_later_write_keeps_the_past_version_and_adds_the_next() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let location = "dbresource://wiki/b.md";
        facade.write_db_resource(location, b"one").unwrap();
        facade.write_db_resource(location, b"two").unwrap();
        facade.write_db_resource(location, b"three").unwrap();
        let current = facade.db_resource_current_version(location).unwrap().expect("current");
        assert_eq!(current.version_name, "03");
        assert_eq!(current.previous_version_name.as_deref(), Some("02"));
        assert_eq!(
            facade
                .db_resource_root_version(location)
                .unwrap()
                .expect("root")
                .version_name,
            "01"
        );
        assert_eq!(
            facade.read_db_resource_version(location, "01").unwrap(),
            Some(b"one".to_vec())
        );
        assert_eq!(
            facade.read_db_resource_version(location, "02").unwrap(),
            Some(b"two".to_vec())
        );
        assert_eq!(
            facade.read_db_resource_version(location, "03").unwrap(),
            Some(b"three".to_vec())
        );
        assert_eq!(facade.read_db_resource_version(location, "09").unwrap(), None);
        assert_eq!(facade.read_db_resource(location).unwrap().bytes, b"three".to_vec());
        let names: Vec<String> = facade
            .db_resource_version_history(location)
            .unwrap()
            .into_iter()
            .map(|version| version.version_name)
            .collect();
        assert_eq!(names, ["03", "02", "01"]);
        assert_eq!(
            facade
                .db_resource_version(location, "02")
                .unwrap()
                .expect("02")
                .previous_version_name
                .as_deref(),
            Some("01")
        );
    }

    #[test]
    fn a_write_across_a_file_or_onto_a_directory_fails() {
        let (facade, context) = facade();
        let _entered = context.enter();
        facade.write_db_resource("dbresource://tree/file.txt", b"one").unwrap();
        let err = facade
            .write_db_resource("dbresource://tree/file.txt/below.txt", b"x")
            .unwrap_err();
        assert!(err.to_string().contains("found file instead"), "{err}");
        let err = facade.write_db_resource("dbresource://tree", b"x").unwrap_err();
        assert!(err.to_string().contains("is a directory"), "{err}");
        let tree = facade.read_db_resource("dbresource://tree").unwrap();
        assert!(tree.exists && !tree.is_file);
        assert_eq!(tree.children.len(), 1);
        let file = facade.read_db_resource("dbresource://tree/file.txt").unwrap();
        assert!(file.is_file && file.children.is_empty());
        assert_eq!(file.bytes, b"one".to_vec());
    }

    #[test]
    fn the_first_write_sets_the_mime_type_and_children_are_in_filename_order() {
        let (facade, context) = facade();
        let _entered = context.enter();
        for name in ["c.md", "a.json", "b.bin"] {
            facade
                .write_db_resource(&format!("dbresource://order/{name}"), b"x")
                .unwrap();
        }
        let names: Vec<String> = facade
            .read_db_resource("dbresource://order")
            .unwrap()
            .children
            .into_iter()
            .map(|child| child.name)
            .collect();
        assert_eq!(names, ["a.json", "b.bin", "c.md"]);
        let mime = |name: &str| {
            let (id, row) = facade.file_row(&format!("dbresource://order/{name}")).unwrap().unwrap();
            assert!(!id.is_empty());
            row.get("mimeType")
                .and_then(moqui_lib::object::Value::as_str)
                .map(str::to_string)
        };
        assert_eq!(mime("a.json").as_deref(), Some("application/json"));
        assert_eq!(mime("c.md").as_deref(), Some("text/plain"));
        assert_eq!(mime("b.bin"), None);
    }

    #[test]
    fn next_versions_are_the_versions_made_from_one() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let location = "dbresource://wiki/n.md";
        facade.write_db_resource(location, b"one").unwrap();
        facade.write_db_resource(location, b"two").unwrap();
        let next: Vec<String> = facade
            .db_resource_next_versions(location, "01")
            .unwrap()
            .into_iter()
            .map(|version| version.version_name)
            .collect();
        assert_eq!(next, ["02"]);
        assert!(facade.db_resource_next_versions(location, "02").unwrap().is_empty());
        assert!(facade
            .db_resource_next_versions("dbresource://wiki/none.md", "01")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn delete_removes_the_history_and_a_missing_file_has_no_versions() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let location = "dbresource://wiki/c.md";
        facade.write_db_resource(location, b"one").unwrap();
        facade.write_db_resource(location, b"two").unwrap();
        assert!(facade.delete_db_resource(location).unwrap());
        assert!(facade.db_resource_version_history(location).unwrap().is_empty());
        assert_eq!(facade.db_resource_current_version(location).unwrap(), None);
        let rows = facade
            .find("moqui.resource.DbResourceFileHistory")
            .unwrap()
            .list()
            .unwrap();
        assert!(rows.is_empty(), "{} history rows left", rows.len());
    }

    /// A page of numbered lines with line `edit` changed, big enough that a one-line change is stored as a delta.
    fn page(edit: usize) -> String {
        (1..=40)
            .map(|line| {
                if line == edit {
                    format!("line {line} edited in version {edit}\n")
                } else {
                    format!("line {line}\n")
                }
            })
            .collect()
    }

    fn history_rows(facade: &EntityFacade) -> Vec<(String, String, Option<Vec<u8>>)> {
        facade
            .find("moqui.resource.DbResourceFileHistory")
            .unwrap()
            .order_by("versionName", false)
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    super::text_of(row.get("versionName")).unwrap_or_default(),
                    super::text_of(row.get("isDiff")).unwrap_or_default(),
                    super::blob_bytes(row.get("fileData"), "test").unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn past_text_versions_are_reverse_deltas_and_every_version_reads_back() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let location = "dbresource://wiki/long.md";
        for version in 1..=25 {
            facade.write_db_resource(location, page(version).as_bytes()).unwrap();
        }
        for version in 1..=25 {
            let name = format!("{version:02}");
            let bytes = facade.read_db_resource_version(location, &name).unwrap();
            assert_eq!(bytes, Some(page(version).into_bytes()), "version {name}");
        }
        let rows = history_rows(&facade);
        assert_eq!(rows.len(), 25);
        for (name, is_diff, data) in &rows {
            let number: u32 = name.parse().unwrap();
            match number {
                25 => assert!(data.is_none(), "the current version's row holds no bytes"),
                20 => assert_eq!((is_diff.as_str(), data.as_deref()), ("N", Some(page(20).as_bytes()))),
                _ => {
                    assert_eq!(is_diff, "Y", "version {name}");
                    let delta = String::from_utf8(data.clone().unwrap()).unwrap();
                    assert!(
                        delta.starts_with(&format!("--- {:02}\n+++ {name}\n", number + 1)),
                        "{delta}"
                    );
                }
            }
        }
    }

    #[test]
    fn binary_bytes_and_large_rewrites_are_stored_whole() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let image = "dbresource://files/a.png";
        let big = page(0).repeat(3);
        facade.write_db_resource(image, big.as_bytes()).unwrap();
        facade
            .write_db_resource(image, (big.clone() + "x\n").as_bytes())
            .unwrap();
        let invalid = "dbresource://files/b.txt";
        let mut latin1 = page(0).into_bytes();
        latin1.push(0xE9);
        facade.write_db_resource(invalid, &latin1).unwrap();
        facade.write_db_resource(invalid, page(3).as_bytes()).unwrap();
        let rewrite = "dbresource://files/c.txt";
        facade.write_db_resource(rewrite, page(0).as_bytes()).unwrap();
        facade.write_db_resource(rewrite, b"something else entirely\n").unwrap();
        let first: Vec<_> = history_rows(&facade)
            .into_iter()
            .filter(|(name, ..)| name == "01")
            .collect();
        assert_eq!(first.len(), 3);
        for (_, is_diff, data) in first {
            assert_eq!(is_diff, "N");
            assert!(data.is_some());
        }
        assert_eq!(
            facade.read_db_resource_version(image, "01").unwrap(),
            Some(big.into_bytes())
        );
        assert_eq!(facade.read_db_resource_version(invalid, "01").unwrap(), Some(latin1));
        assert_eq!(
            facade.read_db_resource_version(rewrite, "01").unwrap(),
            Some(page(0).into_bytes())
        );
    }

    #[test]
    fn a_broken_delta_reads_as_no_version_and_the_others_still_read() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let location = "dbresource://wiki/broken.md";
        for version in 1..=4 {
            facade.write_db_resource(location, page(version).as_bytes()).unwrap();
        }
        let (resource_id, _) = facade.file_row(location).unwrap().unwrap();
        let mut row = crate::entity::EntityValue::make(&facade, "moqui.resource.DbResourceFileHistory").unwrap();
        row.set("resourceId", moqui_lib::object::Value::string(&resource_id));
        row.set("versionName", moqui_lib::object::Value::string("02"));
        row.set(
            "fileData",
            moqui_lib::object::Value::Bytes(b"@@ -1 +1 @@\n-nope\n+still nope\n".to_vec()),
        );
        row.update(&facade).unwrap();
        assert_eq!(
            facade.read_db_resource_version(location, "03").unwrap(),
            Some(page(3).into_bytes())
        );
        assert_eq!(facade.read_db_resource_version(location, "02").unwrap(), None);
        // 01 is rebuilt through 02's delta, so it is lost with it
        assert_eq!(facade.read_db_resource_version(location, "01").unwrap(), None);
        assert_eq!(
            facade.read_db_resource_version(location, "04").unwrap(),
            Some(page(4).into_bytes())
        );
    }

    #[test]
    fn large_content_is_a_content_blob_that_past_versions_keep() {
        let (facade, context) = facade();
        let _entered = context.enter();
        let location = "dbresource://media/clip.bin";
        let first: Vec<u8> = (0..200_000u32).map(|n| (n * 13 % 251) as u8).collect();
        let second: Vec<u8> = first.iter().rev().copied().collect();
        facade.write_db_resource(location, &first).unwrap();
        let (_, row) = facade.file_row(location).unwrap().unwrap();
        let blob = super::text_of(row.get("blobId")).expect("a blob");
        assert_eq!(blob, crate::resource::blob::blob_id(&first));
        assert!(row.get("fileData").is_none_or(moqui_lib::object::Value::is_null));
        assert_eq!(facade.stat_db_resource(location).unwrap().unwrap().size, 200_000);
        assert_eq!(facade.read_db_resource_bytes(location).unwrap().unwrap(), first);

        // a new version: the past one keeps its blob, no copy
        let mut input = &second[..];
        facade
            .write_db_resource_from(location, &mut input, Some(second.len() as u64))
            .unwrap();
        assert_eq!(facade.read_db_resource_bytes(location).unwrap().unwrap(), second);
        assert_eq!(
            facade.read_db_resource_version(location, "01").unwrap(),
            Some(first.clone())
        );
        let past: Vec<_> = history_rows(&facade)
            .into_iter()
            .filter(|(name, ..)| name == "01")
            .collect();
        assert_eq!(past.len(), 1);
        assert!(past[0].2.is_none(), "the past row holds no bytes of its own");
        let (stat, source) = facade.open_db_resource(location).unwrap().unwrap();
        assert_eq!(stat.size, second.len() as u64);
        assert_eq!(source.read_at(199_990, 100).unwrap(), second[199_990..]);
        // the same content elsewhere is the same blob
        facade.write_db_resource("dbresource://media/copy.bin", &first).unwrap();
        let blobs = facade.find("moqui.resource.ContentBlob").unwrap().list().unwrap();
        assert_eq!(blobs.len(), 2);

        // a row written before content blobs moves into one
        facade
            .write_db_resource("dbresource://media/old.bin", b"small")
            .unwrap();
        let (_, mut old) = facade.file_row("dbresource://media/old.bin").unwrap().unwrap();
        old.set("fileData", moqui_lib::object::Value::Bytes(second.clone()));
        old.set("fileSize", moqui_lib::object::Value::Null);
        old.update(&facade).unwrap();
        let (moved, _) = facade
            .migrate_db_resource_blobs(std::time::Instant::now() + std::time::Duration::from_secs(60))
            .unwrap();
        assert_eq!(moved, 1);
        let (_, old) = facade.file_row("dbresource://media/old.bin").unwrap().unwrap();
        assert_eq!(
            super::text_of(old.get("blobId")),
            Some(crate::resource::blob::blob_id(&second))
        );
        assert_eq!(
            facade
                .read_db_resource_bytes("dbresource://media/old.bin")
                .unwrap()
                .unwrap(),
            second
        );
        assert_eq!(
            facade
                .stat_db_resource("dbresource://media/old.bin")
                .unwrap()
                .unwrap()
                .size,
            200_000
        );
    }

    /// `ResourceRef::size` of a `dbresource://` file is its data length, for the file and for one found among a
    /// directory's children, whose bytes are read when asked for.
    #[test]
    fn a_db_resource_ref_has_its_size_and_a_child_its_bytes() {
        let (facade, context) = facade();
        let _entered = context.enter();
        facade
            .write_db_resource("dbresource://files/a·b.txt", "héllo".as_bytes())
            .unwrap();
        facade.write_db_resource("dbresource://files/empty.txt", b"").unwrap();
        let resources = context.factory().resource();
        let file = resources.get("dbresource://files/a·b.txt").unwrap();
        assert_eq!(file.size(), Some(6));
        assert_eq!(resources.get("dbresource://files/empty.txt").unwrap().size(), Some(0));
        let directory = resources.get("dbresource://files").unwrap();
        assert_eq!(directory.size(), None);
        let child = directory.find_child_file("a·b.txt");
        assert_eq!(child.size(), Some(6));
        assert_eq!(child.read_text().unwrap(), "héllo");
        let listed: Vec<_> = directory
            .children()
            .unwrap()
            .iter()
            .map(|child| (child.size(), child.read_bytes().unwrap().len()))
            .collect();
        assert!(
            listed.contains(&(Some(6), 6)) && listed.contains(&(Some(0), 0)),
            "{listed:?}"
        );
        assert_eq!(directory.find_child_file("missing.txt").size(), None);
    }
}
