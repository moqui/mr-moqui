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
    crate::hooks::resource_setting("dbresource-blob-threshold")
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
/// [`ResourceVersion`]: crate::resource::version::ResourceVersion
pub type DbResourceVersion = crate::resource::version::ResourceVersion;

struct Node {
    id: String,
    is_file: bool,
}

/// A node under a directory, with its name.
struct ChildNode {
    name: String,
    node: Node,
}

fn node_of(row: &EntityValue) -> Option<Node> {
    let id = row.get("resourceId").and_then(Value::as_str)?.to_string();
    let is_file = row
        .get("isFile")
        .and_then(Value::as_str)
        .is_some_and(|flag| flag.eq_ignore_ascii_case("Y"));
    Some(Node { id, is_file })
}

/// A directory this facade made lately ([`EntityFacade::make_directory`]).
struct MadeDirectory {
    parent: Option<String>,
    name: String,
    id: String,
    at: std::time::Instant,
}

/// The directories a facade made in the last [`MADE_FOR`], which a writer whose transaction reads from before their
/// commit finds here; and the lock under which a directory is made. A slot of the facade, so of its database.
#[derive(Default)]
struct MadeDirectories(std::sync::Arc<std::sync::Mutex<Vec<MadeDirectory>>>);

/// How long a directory made stays in [`MadeDirectories`]: longer than writers that started together wait on each
/// other.
const MADE_FOR: std::time::Duration = std::time::Duration::from_secs(60);

/// The order of two `resourceId`s, the older first. `DbResource` keeps no creation date (its `lastUpdatedStamp`
/// moves with a move), and its ids are sequenced, so a shorter id is the older one and ids of one length go in text
/// order: "99999" before "100000", where text order alone puts it after.
fn oldest_first(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
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
                bytes: self.read_bytes(&node.id)?,
                children: Vec::new(),
            },
            Some(_) => DbResourceInfo {
                exists: true,
                is_file: false,
                bytes: Vec::new(),
                children: self.list_directory(&parts)?,
            },
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
            Some(_) => DbResourceInfo {
                exists: true,
                is_file: false,
                bytes: Vec::new(),
                children: self.list_directory(&parts)?,
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
        // the file where it is, also under a duplicate of one of its directories (see `node_at`)
        let mut path = parts.clone();
        path.push(filename.clone());
        let id = match self.node_at(&path)? {
            Some(node) if !node.is_file => {
                return Err(Error::new(format!(
                    "Cannot write a file at {location}, it is a directory"
                )))
            }
            Some(node) => node.id,
            None => {
                let parent = self.directory_id(&parts)?;
                self.create_node(parent.as_deref(), &filename, true)?
            }
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
        // every copy a race made (see `node_at`), or the next oldest would show at the location again
        let nodes = self.nodes_at(&parts)?;
        if nodes.is_empty() {
            return Ok(false);
        }
        for node in &nodes {
            self.delete_node(node)?;
        }
        Ok(true)
    }

    /// Delete one `DbResource` row, with its file and history rows when it is a file.
    fn delete_node(&self, node: &Node) -> Result<(), Error> {
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
        self.forget_made(&node.id);
        Ok(())
    }

    /// The directories this facade's process made lately ([`MadeDirectories`]).
    fn made_directories(&self) -> std::sync::Arc<std::sync::Mutex<Vec<MadeDirectory>>> {
        self.with_slot_read(MadeDirectories::default, |made| std::sync::Arc::clone(&made.0))
    }

    /// Drop directory `id`, deleted or moved, from [`MadeDirectories`], so no writer takes it for its old path.
    fn forget_made(&self, id: &str) {
        moqui_lib::sync::lock(&self.made_directories()).retain(|dir| dir.id != id);
    }

    /// Give a `dbresource://` node a new parent and name, creating the new parent directories, as
    /// `DbResourceReference.move`. Nothing happens when there is no row at `location`.
    pub fn move_db_resource(&self, location: &str, new_location: &str) -> Result<(), Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        let nodes = if parts.is_empty() {
            Vec::new()
        } else {
            self.nodes_at(&parts)?
        };
        let Some(node) = nodes.first() else {
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
        // the copies a race made (see `node_at`) go with it: a directory's children into the one moved, a file's
        // copy (which a read never showed, as the oldest is read) deleted, so nothing is left at the old location
        for copy in &nodes[1..] {
            if copy.is_file {
                tracing::warn!(
                    "dbresource {location}: its copy DbResource {}, never read, is deleted",
                    copy.id
                );
                self.delete_node(copy)?;
            } else if node.is_file {
                tracing::warn!(
                    "dbresource {location}: its directory copy DbResource {} is left",
                    copy.id
                );
            } else if self.merge_into(&node.id, copy)? {
                self.delete_node(copy)?;
            }
        }
        let mut value = EntityValue::make(self, DB_RESOURCE)?;
        value.set("resourceId", Value::string(&node.id));
        value.set("parentResourceId", parent.map_or(Value::Null, Value::string));
        value.set("filename", Value::string(new_name));
        value.set("isFile", Value::string(if node.is_file { "Y" } else { "N" }));
        value.update(self)?;
        self.forget_made(&node.id);
        Ok(())
    }

    /// Move the children of directory `copy` into directory `into`, where `into` has no child of the same name. One it
    /// has is merged into it when both are directories, deleted when both are files (a read never showed it), and
    /// left in `copy` (logged) when they differ. True when `copy` has no children left, so the caller deletes it.
    fn merge_into(&self, into: &str, copy: &Node) -> Result<bool, Error> {
        let mut left = false;
        for child in self.child_nodes(Some(&copy.id))? {
            match self.find_node(Some(into), &child.name)? {
                None => {
                    let mut value = EntityValue::make(self, DB_RESOURCE)?;
                    value.set("resourceId", Value::string(&child.node.id));
                    value.set("parentResourceId", Value::string(into));
                    value.update(self)?;
                }
                Some(held) if !held.is_file && !child.node.is_file => {
                    if self.merge_into(&held.id, &child.node)? {
                        self.delete_node(&child.node)?;
                    } else {
                        left = true;
                    }
                }
                // a file the copy has of a name the directory has too was never read (the oldest copy's is)
                Some(held) if held.is_file && child.node.is_file => {
                    tracing::warn!(
                        "dbresource: {} (DbResource {}), never read as directory {into} has DbResource {} of that \
                         name, is deleted",
                        child.name,
                        child.node.id,
                        held.id
                    );
                    self.delete_node(&child.node)?;
                }
                Some(held) => {
                    tracing::warn!(
                        "dbresource: {} (DbResource {}) is not merged into directory {into}, which has DbResource {} \
                         of that name of the other kind",
                        child.name,
                        child.node.id,
                        held.id
                    );
                    left = true;
                }
            }
        }
        Ok(!left)
    }

    /// The node at a path, walking from the root: the oldest of the copies [`Self::nodes_at`] finds.
    fn node_at(&self, parts: &[String]) -> Result<Option<Node>, Error> {
        Ok(self.nodes_at(parts)?.into_iter().next())
    }

    /// Every node at a path, the one to use first. A directory made before [`Self::make_directory`] serialised it
    /// could be made twice by two first writes at once (the root has no unique key on its names, as
    /// `parentResourceId` is null there), each writing its file under its own copy; so a path is looked for under
    /// every directory of that name, the oldest first, and a file under the oldest copy is the one read.
    fn nodes_at(&self, parts: &[String]) -> Result<Vec<Node>, Error> {
        let mut parents: Vec<Option<String>> = vec![None];
        for (index, segment) in parts.iter().enumerate() {
            let mut found = Vec::new();
            for parent in &parents {
                found.extend(self.find_nodes(parent.as_deref(), segment)?);
            }
            if found.is_empty() {
                // made by this process after this transaction's snapshot ([`Self::make_directory`])
                for parent in &parents {
                    found.extend(
                        self.lately_made(parent.as_deref(), Some(segment))?
                            .into_iter()
                            .map(|child| child.node),
                    );
                }
            }
            if index + 1 == parts.len() {
                return Ok(found);
            }
            parents = found
                .into_iter()
                .filter(|node| !node.is_file)
                .map(|node| Some(node.id))
                .collect();
            if parents.is_empty() {
                return Ok(Vec::new());
            }
        }
        Ok(Vec::new())
    }

    /// The children of the directory at a path, merged over its copies (see [`Self::nodes_at`]): a name in more than
    /// one is listed once, as the copy a read would use. In filename order.
    fn list_directory(&self, parts: &[String]) -> Result<Vec<DbResourceChild>, Error> {
        let mut children: Vec<DbResourceChild> = Vec::new();
        for copy in self.nodes_at(parts)?.iter().filter(|node| !node.is_file) {
            for child in self.child_nodes(Some(&copy.id))? {
                if !children.iter().any(|held| held.name == child.name) {
                    children.push(DbResourceChild {
                        name: child.name,
                        is_file: child.node.is_file,
                    });
                }
            }
        }
        children.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(children)
    }

    /// The ID of the directory at a path, creating each missing one ([`Self::make_directory`]). `None` is the root.
    fn directory_id(&self, parts: &[String]) -> Result<Option<String>, Error> {
        let mut parent: Option<String> = None;
        for segment in parts {
            let node = match self.find_node(parent.as_deref(), segment)? {
                Some(node) => node,
                None => self.make_directory(parent.as_deref(), segment)?,
            };
            if node.is_file {
                return Err(Error::new(format!(
                    "Tried to find a directory in a path but found file instead at {segment} under DbResource {}",
                    parent.as_deref().unwrap_or("root")
                )));
            }
            parent = Some(node.id);
        }
        Ok(parent)
    }

    /// Make directory `name` under `parent`, which a find did not see, so two first writes at once do not each make
    /// their own copy. It is made in a transaction of its own, which commits before the caller goes on, so it is
    /// there for the next writer whether or not the caller's transaction commits; a directory left by a caller that
    /// rolls back is empty, as one `make_db_directory` makes is.
    ///
    /// In this process one thread at a time makes a directory, and the ones it made lately ([`MadeDirectories`]) are where the
    /// next looks first: its transaction may read from before the commit (a snapshot, as MySQL's and the test
    /// database's are), so a find alone would not see it. The lock is taken only once the transaction has its
    /// connection (the first find), so a thread waiting on it never holds back a connection the holder needs, and
    /// held until the commit. Another process can make it at the same time: under a parent the unique index
    /// (`DB_RES_PAR_FN`) refuses the second, and at the root, which it does not cover (the parent is null), both are
    /// made; so the node is read again after and the oldest taken.
    fn make_directory(&self, parent: Option<&str>, name: &str) -> Result<Node, Error> {
        let lately_made = self.made_directories();
        let mut held: Option<std::sync::MutexGuard<'_, Vec<MadeDirectory>>> = None;
        let made =
            self.transactions_for(DB_RESOURCE)?
                .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                    if let Some(node) = self.find_node(parent, name)? {
                        return Ok((node.id, false));
                    }
                    let mut made = moqui_lib::sync::lock(&lately_made);
                    made.retain(|dir| dir.at.elapsed() < MADE_FOR);
                    let lately = made
                        .iter()
                        .find(|dir| dir.parent.as_deref() == parent && dir.name == name)
                        .map(|dir| dir.id.clone());
                    held = Some(made);
                    match lately {
                        Some(id) => Ok((id, false)),
                        None => Ok((self.create_node(parent, name, false)?, true)),
                    }
                });
        if let (Some(made), Ok((id, true))) = (held.as_mut(), &made) {
            made.push(MadeDirectory {
                parent: parent.map(str::to_string),
                name: name.to_string(),
                id: id.clone(),
                at: std::time::Instant::now(),
            });
        }
        drop(held);
        if let Err(error) = &made {
            // most likely another process made it first; it is read below
            tracing::warn!(
                "dbresource: directory {name} under DbResource {} was not made: {error}",
                parent.unwrap_or("root")
            );
        }
        match (self.find_node(parent, name)?, made) {
            (Some(node), _) => Ok(node),
            // made, or made lately by another thread, but not seen by this transaction (a snapshot from before it)
            (None, Ok((id, _))) => Ok(Node { id, is_file: false }),
            (None, Err(error)) => Err(error),
        }
    }

    /// Skip the table check, for the in-memory test database, which has every table and no `information_schema`.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn assume_db_tables(&self) {
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

    /// The node named `filename` under `parent`: the oldest, when a race made several (see `node_at`).
    fn find_node(&self, parent: Option<&str>, filename: &str) -> Result<Option<Node>, Error> {
        Ok(self.find_nodes(parent, filename)?.into_iter().next())
    }

    /// Every node named `filename` under `parent`, the oldest first ([`oldest_first`]).
    fn find_nodes(&self, parent: Option<&str>, filename: &str) -> Result<Vec<Node>, Error> {
        let mut find = self.find(DB_RESOURCE)?.eq("filename", Value::string(filename));
        find = match parent {
            Some(parent) => find.eq("parentResourceId", Value::string(parent)),
            None => find.condition(Condition::is_null("parentResourceId")),
        };
        let mut nodes: Vec<Node> = find.list()?.iter().filter_map(node_of).collect();
        nodes.sort_by(|a, b| oldest_first(&a.id, &b.id));
        Ok(nodes)
    }

    /// The nodes under `parent` (`None`: the root) with their names, by name and then the oldest first.
    fn child_nodes(&self, parent: Option<&str>) -> Result<Vec<ChildNode>, Error> {
        let find = self.find(DB_RESOURCE)?;
        let find = match parent {
            Some(parent) => find.eq("parentResourceId", Value::string(parent)),
            None => find.condition(Condition::is_null("parentResourceId")),
        };
        let rows = find.list()?;
        let mut children: Vec<ChildNode> = rows
            .iter()
            .filter_map(|row| {
                let name = row.get("filename").and_then(Value::as_str)?.to_string();
                Some(ChildNode {
                    name,
                    node: node_of(row)?,
                })
            })
            .collect();
        for made in self.lately_made(parent, None)? {
            if !children.iter().any(|child| child.node.id == made.node.id) {
                children.push(made);
            }
        }
        children.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| oldest_first(&a.node.id, &b.node.id)));
        Ok(children)
    }

    /// The directories under `parent` (named `name`, when given) this process made lately ([`MadeDirectories`]) that are
    /// still there. A find in a transaction that reads from before their commit (a snapshot) does not see them, so a read
    /// right after [`Self::make_db_directory`] in the same transaction would find no directory; each is checked in a
    /// transaction of its own, so one deleted since, by another process too, is not taken.
    fn lately_made(&self, parent: Option<&str>, name: Option<&str>) -> Result<Vec<ChildNode>, Error> {
        let lately_made = self.made_directories();
        let candidates: Vec<(String, String)> = {
            let mut made = moqui_lib::sync::lock(&lately_made);
            made.retain(|dir| dir.at.elapsed() < MADE_FOR);
            made.iter()
                .filter(|dir| dir.parent.as_deref() == parent && name.is_none_or(|name| dir.name == name))
                .map(|dir| (dir.name.clone(), dir.id.clone()))
                .collect()
        };
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        self.transactions_for(DB_RESOURCE)?
            .run(crate::transaction::TransactionAttribute::ForceNew, None, || {
                let mut there = Vec::new();
                for (name, id) in candidates {
                    let Some(row) = self.find(DB_RESOURCE)?.eq("resourceId", Value::string(&id)).one()? else {
                        continue;
                    };
                    let same_place = row.get("parentResourceId").and_then(Value::as_str) == parent
                        && row.get("filename").and_then(Value::as_str) == Some(name.as_str());
                    if let (true, Some(node)) = (same_place, node_of(&row)) {
                        there.push(ChildNode { name, node });
                    }
                }
                Ok(there)
            })
    }

    /// The children under `parent` (`None`: the root), in filename order; a name made twice by a race is listed once.
    fn list_children(&self, parent: Option<&str>) -> Result<Vec<DbResourceChild>, Error> {
        let mut children: Vec<DbResourceChild> = Vec::new();
        for child in self.child_nodes(parent)? {
            if children.last().is_some_and(|held| held.name == child.name) {
                continue;
            }
            children.push(DbResourceChild {
                name: child.name,
                is_file: child.node.is_file,
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
        // a version copied with its history names its own author and date (crate::resource::version::with_version_author)
        let author = crate::resource::version::version_author().unwrap_or_default();
        let date = author
            .date
            .and_then(|millis| jiff::Timestamp::from_millisecond(millis).ok())
            .map_or_else(crate::hooks::now_effective_value, Value::Timestamp);
        history.set("versionDate", date);
        let user_id = author.user_id.or_else(|| (crate::hooks::get().user_id)());
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
#[path = "dbresource_tests.rs"]
mod tests;
