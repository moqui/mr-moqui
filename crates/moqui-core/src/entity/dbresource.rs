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

const DB_RESOURCE: &str = "moqui.resource.DbResource";
const DB_FILE: &str = "moqui.resource.DbResourceFile";
const DB_HISTORY: &str = "moqui.resource.DbResourceFileHistory";
const SEQUENCE: &str = "moqui.entity.SequenceValueItem";

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

/// One `DbResourceFileHistory` row, as Java's `ResourceReference.Version`. `version_date` is a Timestamp value.
#[derive(Clone, Debug, PartialEq)]
pub struct DbResourceVersion {
    pub version_name: String,
    pub previous_version_name: Option<String>,
    pub version_date: Value,
    pub user_id: Option<String>,
}

struct Node {
    id: String,
    is_file: bool,
}

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

    /// Create or replace a `dbresource://` file. Missing parent directories are created. A file in the path, or a
    /// directory at the location, is an error, as `DbResourceReference.putObject`.
    pub fn write_db_resource(&self, location: &str, bytes: &[u8]) -> Result<(), Error> {
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
        self.write_bytes(&id, &filename, bytes)
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
                size = self.read_bytes(&node.id)?.len() as u64;
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
        Ok(blob_bytes(row.get("fileData"), resource_id)?.unwrap_or_default())
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
    /// and the MIME type of `filename`; a later write keeps the replaced bytes in the current version's history row and
    /// adds the next version.
    fn write_bytes(&self, resource_id: &str, filename: &str, bytes: &[u8]) -> Result<(), Error> {
        let existing = self.find(DB_FILE)?.eq("resourceId", Value::string(resource_id)).one()?;
        let Some(existing) = existing else {
            let mut value = EntityValue::make(self, DB_FILE)?;
            value.set("resourceId", Value::string(resource_id));
            if let Some(mime_type) = moqui_lib::resource::content_type_for_name(filename) {
                value.set("mimeType", Value::string(mime_type));
            }
            value.set("fileData", Value::Bytes(bytes.to_vec()));
            value.set("versionName", Value::string(FIRST_VERSION));
            value.set("rootVersionName", Value::string(FIRST_VERSION));
            value.create(self)?;
            // no fileData: a history row holds the bytes only once it is a past version
            let mut history = self.new_history(resource_id, None)?;
            history.set("versionName", Value::string(FIRST_VERSION));
            history.create(self)?;
            return Ok(());
        };
        let current = text_of(existing.get("versionName"));
        if let Some(current) = &current {
            let past = self
                .find(DB_HISTORY)?
                .eq("resourceId", Value::string(resource_id))
                .eq("versionName", Value::string(current))
                .one()?;
            if past.is_some() {
                let mut past = EntityValue::make(self, DB_HISTORY)?;
                past.set("resourceId", Value::string(resource_id));
                past.set("versionName", Value::string(current));
                past.set("fileData", existing.get("fileData").cloned().unwrap_or(Value::Null));
                past.update(self)?;
            }
        }
        let mut history = self.new_history(resource_id, current.as_deref())?;
        history.set_sequenced_id_secondary(self)?;
        let next = text_of(history.get("versionName"))
            .ok_or_else(|| Error::new(format!("no next version name for DbResource {resource_id}")))?;
        history.create(self)?;
        let mut value = EntityValue::make(self, DB_FILE)?;
        value.set("resourceId", Value::string(resource_id));
        value.set("fileData", Value::Bytes(bytes.to_vec()));
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
        history.set("versionDate", crate::user::now_effective_value());
        let user_id = crate::user::current_user_id().ok().flatten();
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
    /// the file's data when it is the current version. `None` when there is no such version.
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
            return Ok(None);
        }
        if let Some(bytes) = blob_bytes(history.get("fileData"), &resource_id)? {
            return Ok(Some(bytes));
        }
        if text_of(file.get("versionName")).as_deref() != Some(version_name) {
            return Ok(None);
        }
        Ok(Some(
            blob_bytes(file.get("fileData"), &resource_id)?.unwrap_or_default(),
        ))
    }
}

const FIRST_VERSION: &str = "01";

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
    }
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
}
