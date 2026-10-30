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

//! What a file browser asks of the `dbresource://` tree: the last change of each entry of a folder in a few queries
//! (the size, the current version with its date, user and note), the bytes under a folder for a quota, the files under
//! a folder, and a delete of a folder with everything under it.

use std::collections::HashMap;

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::{db_parts, file_size_of, stamp_millis, text_of, DB_FILE, DB_HISTORY, DB_RESOURCE};
use crate::entity::facade::EntityFacade;
use crate::entity::value::EntityValue;
use crate::resource::version::ResourceChangeInfo;

/// How deep a walk of the tree goes, so a damaged tree that loops cannot run forever.
const MAX_DEPTH: usize = 64;

/// Ids per `IN` list of a query.
const IN_CHUNK: usize = 500;

/// One `DbResource` row under a directory.
struct Row {
    id: String,
    name: String,
    is_file: bool,
    stamp: Option<i64>,
}

impl EntityFacade {
    /// The id of the directory at `location`: `Ok(None)` for the root, an error when it is not a directory.
    fn directory_at(&self, location: &str) -> Result<Option<String>, Error> {
        let parts = db_parts(location)?;
        if parts.is_empty() {
            return Ok(None);
        }
        match self.node_at(&parts)? {
            Some(node) if !node.is_file => Ok(Some(node.id)),
            Some(_) => Err(Error::new(format!("not a directory: {location}"))),
            None => Err(Error::new(format!("resource not found: {location}"))),
        }
    }

    fn rows_under(&self, parent: Option<&str>) -> Result<Vec<Row>, Error> {
        let find = self
            .find(DB_RESOURCE)?
            .select_field("resourceId")
            .select_field("filename")
            .select_field("isFile")
            .select_field("lastUpdatedStamp");
        let find = match parent {
            Some(parent) => find.eq("parentResourceId", Value::string(parent)),
            None => find.condition(Condition::is_null("parentResourceId")),
        };
        let rows = find.order_by("filename", false).list()?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(Row {
                    id: text_of(row.get("resourceId"))?,
                    name: text_of(row.get("filename"))?,
                    is_file: text_of(row.get("isFile")).is_some_and(|flag| flag.eq_ignore_ascii_case("Y")),
                    stamp: stamp_millis(row.get("lastUpdatedStamp")),
                })
            })
            .collect())
    }

    /// The `DbResourceFile` rows of `ids` without their data, by id.
    fn file_rows(&self, ids: &[String]) -> Result<HashMap<String, EntityValue>, Error> {
        let mut found = HashMap::new();
        for chunk in ids.chunks(IN_CHUNK) {
            let rows = self
                .find(DB_FILE)?
                .select_field("resourceId")
                .select_field("fileSize")
                .select_field("versionName")
                .select_field("versionNote")
                .select_field("lastUpdatedStamp")
                .condition(Condition::is_in(
                    "resourceId",
                    chunk.iter().map(Value::string).collect(),
                ))
                .list()?;
            found.extend(
                rows.into_values()
                    .into_iter()
                    .filter_map(|row| text_of(row.get("resourceId")).map(|id| (id, row))),
            );
        }
        Ok(found)
    }

    /// The size of a file whose row has no `fileSize`: its data measured.
    fn measured_size(&self, resource_id: &str) -> Result<u64, Error> {
        let row = self.find(DB_FILE)?.eq("resourceId", Value::string(resource_id)).one()?;
        match row {
            Some(row) => file_size_of(&row, resource_id),
            None => Ok(0),
        }
    }

    /// The last change of every entry directly under the directory at `location`, by name, in name order: for a file
    /// its size and current version (date, user, note), for a directory the time of its row. Four queries however many
    /// entries there are (per 500 files), and one per file written before `fileSize` was kept.
    pub fn db_resource_child_changes(&self, location: &str) -> Result<Vec<(String, ResourceChangeInfo)>, Error> {
        self.ensure_db_tables()?;
        let parent = self.directory_at(location)?;
        let rows = self.rows_under(parent.as_deref())?;
        let ids: Vec<String> = rows
            .iter()
            .filter(|row| row.is_file)
            .map(|row| row.id.clone())
            .collect();
        let files = self.file_rows(&ids)?;
        // the history rows of the current versions only: by the files and the version names they are at
        let mut histories: HashMap<(String, String), EntityValue> = HashMap::new();
        let mut names: Vec<String> = files
            .values()
            .filter_map(|file| text_of(file.get("versionName")))
            .collect();
        names.sort();
        names.dedup();
        for chunk in ids.chunks(IN_CHUNK) {
            if names.is_empty() {
                break;
            }
            let found = self
                .find(DB_HISTORY)?
                .select_field("resourceId")
                .select_field("versionName")
                .select_field("versionDate")
                .select_field("userId")
                .select_field("versionNote")
                .condition(Condition::is_in(
                    "resourceId",
                    chunk.iter().map(Value::string).collect(),
                ))
                .condition(Condition::is_in(
                    "versionName",
                    names.iter().map(Value::string).collect(),
                ))
                .list()?;
            for row in found.into_values() {
                if let (Some(id), Some(name)) = (text_of(row.get("resourceId")), text_of(row.get("versionName"))) {
                    histories.insert((id, name), row);
                }
            }
        }
        let mut changes = Vec::with_capacity(rows.len());
        for row in rows {
            if !row.is_file {
                changes.push((
                    row.name,
                    ResourceChangeInfo {
                        is_file: false,
                        date: row.stamp,
                        ..ResourceChangeInfo::default()
                    },
                ));
                continue;
            }
            let file = files.get(&row.id);
            let size = match file {
                Some(file) if file.get("fileSize").is_some_and(|size| !size.is_null()) => file_size_of(file, &row.id)?,
                Some(_) => self.measured_size(&row.id)?,
                None => 0,
            };
            let version_name = file.and_then(|file| text_of(file.get("versionName")));
            let history = version_name
                .as_ref()
                .and_then(|name| histories.get(&(row.id.clone(), name.clone())));
            let date = history
                .and_then(|history| stamp_millis(history.get("versionDate")))
                .or_else(|| file.and_then(|file| stamp_millis(file.get("lastUpdatedStamp"))))
                .or(row.stamp);
            changes.push((
                row.name,
                ResourceChangeInfo {
                    is_file: true,
                    size: Some(size),
                    version_name,
                    date,
                    user_id: history.and_then(|history| text_of(history.get("userId"))),
                    note: history
                        .and_then(|history| text_of(history.get("versionNote")))
                        .or_else(|| file.and_then(|file| text_of(file.get("versionNote")))),
                },
            ));
        }
        Ok(changes)
    }

    /// The last change of the file or directory at `location`, as [`Self::db_resource_child_changes`] has it; `None`
    /// when nothing is there. The root has no time.
    pub fn db_resource_last_change(&self, location: &str) -> Result<Option<ResourceChangeInfo>, Error> {
        self.ensure_db_tables()?;
        let mut parts = db_parts(location)?;
        let Some(name) = parts.pop() else {
            return Ok(Some(ResourceChangeInfo::default()));
        };
        let parent = if parts.is_empty() {
            "dbresource://".to_string()
        } else {
            format!("dbresource://{}", parts.join("/"))
        };
        match self.directory_at(&parent) {
            Ok(_) => {}
            Err(_) => return Ok(None),
        }
        Ok(self
            .db_resource_child_changes(&parent)?
            .into_iter()
            .find(|(child, _)| *child == name)
            .map(|(_, change)| change))
    }

    /// The bytes of every file under `location` (a file: its own size), by `fileSize` where the rows have it.
    pub fn db_resource_tree_size(&self, location: &str) -> Result<u64, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        let start = if parts.is_empty() {
            None
        } else {
            match self.node_at(&parts)? {
                None => return Ok(0),
                Some(node) if node.is_file => {
                    let files = self.file_rows(std::slice::from_ref(&node.id))?;
                    return match files.get(&node.id) {
                        Some(file) if file.get("fileSize").is_some_and(|size| !size.is_null()) => {
                            file_size_of(file, &node.id)
                        }
                        _ => self.measured_size(&node.id),
                    };
                }
                Some(node) => Some(node.id),
            }
        };
        let mut total = 0u64;
        let mut pending = vec![(start, 0usize)];
        while let Some((parent, depth)) = pending.pop() {
            if depth > MAX_DEPTH {
                tracing::warn!(
                    location,
                    "dbresource tree is deeper than {MAX_DEPTH}; the size stops there"
                );
                continue;
            }
            let rows = self.rows_under(parent.as_deref())?;
            let ids: Vec<String> = rows
                .iter()
                .filter(|row| row.is_file)
                .map(|row| row.id.clone())
                .collect();
            let files = self.file_rows(&ids)?;
            for row in rows {
                if row.is_file {
                    total += match files.get(&row.id) {
                        Some(file) if file.get("fileSize").is_some_and(|size| !size.is_null()) => {
                            file_size_of(file, &row.id)?
                        }
                        _ => self.measured_size(&row.id)?,
                    };
                } else {
                    pending.push((Some(row.id), depth + 1));
                }
            }
        }
        Ok(total)
    }

    /// The paths, relative to `location`, of the files under the directory at `location`, at most `limit`, in name
    /// order by directory.
    pub fn db_resource_files_under(&self, location: &str, limit: usize) -> Result<Vec<String>, Error> {
        self.ensure_db_tables()?;
        let start = self.directory_at(location)?;
        let mut found = Vec::new();
        let mut pending = vec![(start, String::new(), 0usize)];
        while let Some((parent, prefix, depth)) = pending.pop() {
            if depth > MAX_DEPTH || found.len() >= limit {
                continue;
            }
            for row in self.rows_under(parent.as_deref())? {
                let path = if prefix.is_empty() {
                    row.name.clone()
                } else {
                    format!("{prefix}/{}", row.name)
                };
                if row.is_file {
                    if found.len() < limit {
                        found.push(path);
                    }
                } else {
                    pending.push((Some(row.id), path, depth + 1));
                }
            }
        }
        Ok(found)
    }

    /// Delete the file or directory at `location` with everything under it: each file's file and history rows, then
    /// the `DbResource` rows from the bottom up. How many nodes were deleted; 0 when nothing is there.
    pub fn delete_db_resource_tree(&self, location: &str) -> Result<usize, Error> {
        self.ensure_db_tables()?;
        let parts = db_parts(location)?;
        if parts.is_empty() {
            return Err(Error::new(format!("cannot delete the dbresource root: {location}")));
        }
        let Some(node) = self.node_at(&parts)? else {
            return Ok(0);
        };
        // every node under it, parents before children
        let mut nodes = vec![(node.id.clone(), node.is_file)];
        let mut index = 0;
        while index < nodes.len() {
            let (id, is_file) = nodes[index].clone();
            index += 1;
            if is_file {
                continue;
            }
            if nodes.len() > 1_000_000 {
                return Err(Error::new(format!("too many resources under {location}")));
            }
            for row in self.rows_under(Some(&id))? {
                nodes.push((row.id, row.is_file));
            }
        }
        for (id, is_file) in nodes.iter().rev() {
            if *is_file {
                self.find(DB_HISTORY)?
                    .eq("resourceId", Value::string(id))
                    .delete_all()?;
                self.find(DB_FILE)?.eq("resourceId", Value::string(id)).delete_all()?;
            }
            let mut value = EntityValue::make(self, DB_RESOURCE)?;
            value.set("resourceId", Value::string(id));
            value.delete(self)?;
        }
        Ok(nodes.len())
    }
}
