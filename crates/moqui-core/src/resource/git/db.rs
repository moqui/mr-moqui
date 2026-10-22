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

//! The `db` kind: a repository whose objects and refs are rows in `moqui.resource.GitObject` and `GitRef`, so the
//! database backup holds it. An object is stored as a pack holds it: the zlib stream of its content, or of git delta
//! instructions against another object of the repository (the deltas a push sent are kept). A clone copies those
//! bytes into its pack as they are; a read inflates them, applies the deltas and checks the content against its id.
//! Rows written before this format hold the content itself and are still read. Reads go a batch of rows at a time,
//! and decoded objects are kept in a cache shared by every `db` repository of the provider.
//!
//! An object written again, or referenced by a new one, is freshened (its `lastUpdatedStamp` set) when its stamp is
//! old, and `gc#GitRepositories` deletes only unreachable rows not touched since its cutoff, so a push that reuses an
//! object while a collection runs keeps it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use gix::objs::Kind;
use gix::ObjectId;
use moqui_lib::object::{basic_convert, Value};

use super::store::{
    deflate, inflate, is_hex_prefix, object_id, verify, EachCommit, EachObject, GitStore, Incoming, ObjectMeta, Packed,
    RefEdit, RefOutcome, Storage,
};
use crate::context::Error;
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::script::bounded::BoundedMap;

pub(crate) const GIT_OBJECT: &str = "moqui.resource.GitObject";
pub(crate) const GIT_REF: &str = "moqui.resource.GitRef";

/// Ids per `IN (...)` query, at most. A shorter list is padded (with its last id) to the next of [`IN_SIZES`], so the
/// database sees a few statements over and over rather than one per length.
const BATCH: usize = 500;
const IN_SIZES: [usize; 4] = [16, 64, 256, BATCH];

/// Bytes of object data one query reads, and one insert transaction writes, about.
const BATCH_BYTES: u64 = 8 * 1024 * 1024;

/// Delta hops from an object to a whole one a read follows before it calls the chain damaged.
const MAX_HOPS: usize = 100;

/// An object referenced again is freshened when its stamp is older than this.
const FRESHEN_AFTER_MILLIS: i64 = 60 * 60 * 1000;

/// The decoded objects a cache entry may be: trees and commits, which path lookups and history walks read over and
/// over, and the bases of deltas.
const CACHE_ENTRY_BYTES: usize = 1024 * 1024;
const CACHE_ENTRIES: usize = 65_536;

/// Decoded objects of every `db` repository of a provider, by repository and id: their kind and content.
pub(crate) type ObjectCache = Mutex<BoundedMap<(Arc<str>, ObjectId), (Kind, Arc<Vec<u8>>)>>;

/// The provider's cache: `git-server` `db-cache-bytes` (64 MiB) of decoded objects.
pub(crate) fn object_cache() -> Arc<ObjectCache> {
    let bytes = super::number_setting("db-cache-bytes", 64 * 1024 * 1024);
    Arc::new(Mutex::new(BoundedMap::with_bytes(CACHE_ENTRIES, bytes)))
}

/// Decoded objects by id: their kind and content.
type Decoded = HashMap<ObjectId, (Kind, Arc<Vec<u8>>)>;

/// The fields of a row read without its data.
const META_FIELDS: [&str; 9] = [
    "objectId",
    "objectType",
    "objectSize",
    "storage",
    "deltaBaseId",
    "deltaDepth",
    "deltaSize",
    "storedBytes",
    "blobId",
];

/// Objects over `git-server` `db-large-object` bytes (8 MiB) are content blobs, not row data.
fn large_object() -> u64 {
    super::number_setting("db-large-object", 8 * 1024 * 1024)
}

/// A `GitObject` row as read.
struct Row {
    id: ObjectId,
    meta: ObjectMeta,
    delta_size: u64,
    stamp: Option<i64>,
    data: Option<Vec<u8>>,
    blob_id: Option<String>,
}

pub(crate) struct DbStore {
    entity: EntityFacade,
    repository_id: Arc<str>,
    cache: Arc<ObjectCache>,
    /// `git-server` `db-write-format="raw"`: new rows hold the content itself, as before the zlib format, for a database read by
    /// an older version.
    write_raw: bool,
    large_object: u64,
}

impl std::fmt::Debug for DbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbStore")
            .field("repository_id", &self.repository_id)
            .finish()
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        value => Some(moqui_lib::object::to_plain_string(value)).filter(|text| !text.is_empty()),
    }
}

fn long(value: Option<&Value>) -> Option<i64> {
    match basic_convert(value?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn millis(value: Option<&Value>) -> Option<i64> {
    match basic_convert(value?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn stamp_value(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

fn kind_of(name: &str) -> Result<Kind, Error> {
    Kind::from_bytes(name.as_bytes()).map_err(|_| Error::new(format!("unknown git object type {name}")))
}

fn id_of(hex: &str) -> Result<ObjectId, Error> {
    ObjectId::from_hex(hex.as_bytes()).map_err(|err| Error::new(format!("GitObject id {hex}: {err}")))
}

fn as_u64(value: Option<i64>) -> u64 {
    value.and_then(|value| u64::try_from(value).ok()).unwrap_or_default()
}

fn i64_of(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// `ids` as the values of an `IN (...)`, padded with the last to one of [`IN_SIZES`].
fn in_values(ids: &[ObjectId]) -> Vec<Value> {
    let mut values: Vec<Value> = ids.iter().map(|id| Value::string(id.to_string())).collect();
    if let Some(last) = values.last().cloned() {
        let size = IN_SIZES
            .iter()
            .copied()
            .find(|size| *size >= values.len())
            .unwrap_or(values.len());
        values.resize(size, last);
    }
    values
}

impl DbStore {
    pub(crate) fn new(entity: EntityFacade, repository_id: &str, cache: Arc<ObjectCache>) -> Self {
        let write_raw = super::setting("db-write-format") == "raw";
        Self {
            entity,
            repository_id: Arc::from(repository_id),
            cache,
            write_raw,
            large_object: large_object(),
        }
    }

    fn cached(&self, id: &ObjectId) -> Option<(Kind, Arc<Vec<u8>>)> {
        moqui_lib::sync::lock(&self.cache).get(&(Arc::clone(&self.repository_id), *id))
    }

    fn remember(&self, id: ObjectId, kind: Kind, data: &Arc<Vec<u8>>) {
        if data.len() <= CACHE_ENTRY_BYTES {
            moqui_lib::sync::lock(&self.cache).insert_sized(
                (Arc::clone(&self.repository_id), id),
                (kind, Arc::clone(data)),
                data.len() + 64,
            );
        }
    }

    /// Forget cached objects of this repository, after a collection deleted rows.
    pub(crate) fn forget(&self, ids: &[ObjectId]) {
        let mut cache = moqui_lib::sync::lock(&self.cache);
        for id in ids {
            cache.remove(&(Arc::clone(&self.repository_id), *id));
        }
    }

    fn row_of(&self, value: &EntityValue) -> Result<Row, Error> {
        let hex = text(value.get("objectId")).unwrap_or_default();
        let id = id_of(&hex)?;
        let kind = kind_of(&text(value.get("objectType")).unwrap_or_default())?;
        let storage = Storage::parse(text(value.get("storage")).as_deref())?;
        let base = match text(value.get("deltaBaseId")) {
            Some(hex) => Some(id_of(&hex)?),
            None => None,
        };
        if storage == Storage::Delta && base.is_none() {
            return Err(Error::new(format!("GitObject {id} is a delta with no base")));
        }
        let blob_id = text(value.get("blobId"));
        if storage == Storage::Blob && blob_id.is_none() {
            return Err(Error::new(format!("GitObject {id} is a blob with no blobId")));
        }
        let size = as_u64(long(value.get("objectSize")));
        let data = match value.get("objectData") {
            None => None,
            Some(data) => match basic_convert(data, "java.sql.Blob").value {
                Value::Bytes(bytes) => Some(bytes),
                Value::Null => Some(Vec::new()),
                other => return Err(Error::new(format!("GitObject {id} data is {other:?}"))),
            },
        };
        let stored_bytes = long(value.get("storedBytes"))
            .map(|bytes| as_u64(Some(bytes)))
            .unwrap_or(size);
        Ok(Row {
            id,
            meta: ObjectMeta {
                kind,
                size,
                storage,
                base,
                depth: u32::try_from(long(value.get("deltaDepth")).unwrap_or(0)).unwrap_or(u32::MAX),
                stored_bytes,
            },
            delta_size: as_u64(long(value.get("deltaSize"))),
            stamp: millis(value.get("lastUpdatedStamp")),
            data,
            blob_id,
        })
    }

    fn blobs(&self) -> crate::resource::blob::BlobStore {
        crate::resource::blob::BlobStore::new(self.entity.clone())
    }

    /// The content blob a row of `blob` storage names.
    fn blob_of(&self, row: &Row) -> Result<crate::resource::blob::BlobInfo, Error> {
        let blob = row.blob_id.as_deref().unwrap_or_default();
        self.blobs().stat(blob)?.ok_or_else(|| {
            Error::new(format!(
                "the content blob {blob} of git object {} of repository {} is missing",
                row.id, self.repository_id
            ))
        })
    }

    /// The rows of `ids` (padded `IN` lists of at most [`BATCH`]), with their data or without.
    fn rows(&self, ids: &[ObjectId], with_data: bool) -> Result<Vec<Row>, Error> {
        let mut rows = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(BATCH) {
            let mut find = self
                .entity
                .find(GIT_OBJECT)?
                .eq("repositoryId", Value::string(self.repository_id.as_ref()))
                .condition(Condition::is_in("objectId", in_values(chunk)));
            for field in META_FIELDS {
                find = find.select_field(field);
            }
            find = find.select_field("lastUpdatedStamp");
            if with_data {
                find = find.select_field("objectData");
            }
            for value in find.list()?.iter() {
                rows.push(self.row_of(value)?);
            }
        }
        Ok(rows)
    }

    /// The rows of `ids` with their data: one query for a single id, else their sizes first, then the data in
    /// batches of about [`BATCH_BYTES`].
    fn data_rows(&self, ids: &[ObjectId]) -> Result<Vec<Row>, Error> {
        if ids.len() <= 1 {
            return self.rows(ids, true);
        }
        let metas = self.rows(ids, false)?;
        let mut rows = Vec::with_capacity(metas.len());
        let mut batch: Vec<ObjectId> = Vec::new();
        let mut bytes = 0u64;
        for meta in metas {
            if !batch.is_empty() && (bytes + meta.meta.stored_bytes > BATCH_BYTES || batch.len() >= BATCH) {
                rows.extend(self.rows(&batch, true)?);
                batch.clear();
                bytes = 0;
            }
            bytes += meta.meta.stored_bytes;
            batch.push(meta.id);
        }
        if !batch.is_empty() {
            rows.extend(self.rows(&batch, true)?);
        }
        Ok(rows)
    }

    /// The content of each of `ids` the repository has, a delta chain one batched level of bases at a time; each one
    /// checked against its id.
    fn resolve_many(&self, ids: &[ObjectId]) -> Result<Decoded, Error> {
        let mut done: Decoded = HashMap::new();
        let mut deltas: HashMap<ObjectId, Row> = HashMap::new();
        let mut wanted: Vec<ObjectId> = Vec::new();
        let mut asked: HashSet<ObjectId> = HashSet::new();
        for id in ids {
            if !asked.insert(*id) {
                continue;
            }
            match self.cached(id) {
                Some(found) => {
                    done.insert(*id, found);
                }
                None => wanted.push(*id),
            }
        }
        let mut hops = 0;
        while !wanted.is_empty() {
            hops += 1;
            if hops > MAX_HOPS {
                return Err(Error::new(format!(
                    "a delta chain of git repository {} is longer than {MAX_HOPS}",
                    self.repository_id
                )));
            }
            let mut next = Vec::new();
            for row in self.data_rows(&wanted)? {
                match row.meta.storage {
                    Storage::Blob => {
                        let info = self.blob_of(&row)?;
                        let content = crate::resource::source::read_all(self.blobs().source(&info).as_ref())?;
                        verify(row.id, row.meta.kind, &content)?;
                        done.insert(row.id, (row.meta.kind, Arc::new(content)));
                    }
                    Storage::Raw | Storage::Zlib => {
                        let data = row.data.as_deref().unwrap_or_default();
                        let content = if row.meta.storage == Storage::Raw {
                            data.to_vec()
                        } else {
                            inflate(data, row.meta.size)
                                .map_err(|err| Error::new(format!("GitObject {}: {err}", row.id)))?
                        };
                        verify(row.id, row.meta.kind, &content)?;
                        let content = Arc::new(content);
                        self.remember(row.id, row.meta.kind, &content);
                        done.insert(row.id, (row.meta.kind, content));
                    }
                    Storage::Delta => {
                        let base = row.meta.base.unwrap_or(row.id);
                        if !done.contains_key(&base) && !deltas.contains_key(&base) && asked.insert(base) {
                            match self.cached(&base) {
                                Some(found) => {
                                    done.insert(base, found);
                                }
                                None => next.push(base),
                            }
                        }
                        deltas.insert(row.id, row);
                    }
                }
            }
            wanted = next;
        }
        let pending: Vec<ObjectId> = deltas.keys().copied().collect();
        for id in pending {
            self.apply_chain(id, &mut deltas, &mut done)?;
        }
        let requested: HashSet<&ObjectId> = ids.iter().collect();
        done.retain(|id, _| requested.contains(id));
        Ok(done)
    }

    /// Apply the deltas from `id` down to an object in `done`, innermost first.
    fn apply_chain(&self, id: ObjectId, deltas: &mut HashMap<ObjectId, Row>, done: &mut Decoded) -> Result<(), Error> {
        let mut chain = vec![id];
        let mut current = id;
        while !done.contains_key(&current) {
            let row = deltas.get(&current).ok_or_else(|| {
                Error::new(format!(
                    "the delta base {current} of git object {id} of repository {} is missing",
                    self.repository_id
                ))
            })?;
            let base = row.meta.base.unwrap_or(current);
            if chain.contains(&base) {
                return Err(Error::new(format!(
                    "git object {id} of repository {} is in a delta cycle",
                    self.repository_id
                )));
            }
            if !done.contains_key(&base) {
                chain.push(base);
            }
            current = base;
            if chain.len() > MAX_HOPS {
                return Err(Error::new(format!(
                    "the delta chain of git object {id} is longer than {MAX_HOPS}"
                )));
            }
        }
        while let Some(link) = chain.pop() {
            if done.contains_key(&link) {
                continue;
            }
            let Some(row) = deltas.remove(&link) else {
                continue;
            };
            let base = row.meta.base.unwrap_or(link);
            let (_, base_data) = done
                .get(&base)
                .cloned()
                .ok_or_else(|| Error::new(format!("the delta base {base} of git object {link} is missing")))?;
            let delta = inflate(row.data.as_deref().unwrap_or_default(), row.delta_size)
                .map_err(|err| Error::new(format!("GitObject {link}: {err}")))?;
            let content = super::delta::apply(&base_data, &delta, Some(row.meta.size))
                .map_err(|err| Error::new(format!("GitObject {link}: {err}")))?;
            verify(link, row.meta.kind, &content)?;
            let content = Arc::new(content);
            self.remember(link, row.meta.kind, &content);
            done.insert(link, (row.meta.kind, content));
        }
        Ok(())
    }

    fn ref_row(&self, name: &str, for_update: bool) -> Result<Option<EntityValue>, Error> {
        Ok(self
            .entity
            .find(GIT_REF)?
            .eq("repositoryId", Value::string(self.repository_id.as_ref()))
            .eq("refName", Value::string(name))
            .for_update(for_update)
            .one()?)
    }

    /// Whether a locked ref row still points where `edit` expects.
    fn matches(row: Option<&EntityValue>, edit: &RefEdit) -> bool {
        row.and_then(|row| text(row.get("objectId"))) == edit.expected.map(|id| id.to_string())
    }

    /// Make one edit to its locked row (`None`: the ref is not there).
    fn apply(&self, row: Option<EntityValue>, edit: &RefEdit) -> Result<(), Error> {
        match (row, edit.new) {
            (Some(mut row), Some(new)) => {
                row.set("objectId", Value::string(new.to_string()));
                Ok(row.update(&self.entity)?)
            }
            (Some(row), None) => Ok(row.delete(&self.entity)?),
            (None, Some(new)) => {
                let mut row = EntityValue::make(&self.entity, GIT_REF)?;
                row.set("repositoryId", Value::string(self.repository_id.as_ref()));
                row.set("refName", Value::string(&edit.name));
                row.set("objectId", Value::string(new.to_string()));
                Ok(row.create(&self.entity)?)
            }
            (None, None) => Ok(()),
        }
    }

    /// Edits in one transaction: the rows are read for update (in name order, so two updates lock in the same order)
    /// and compared before anything is written, so a stale atomic update writes nothing and needs no rollback, which
    /// matters when the scope joins a caller's transaction.
    fn update_in_scope(
        &self,
        edits: &[(usize, &RefEdit)],
        atomic: bool,
        outcomes: &mut [RefOutcome],
    ) -> Result<(), Error> {
        let scope = self.entity.transactions_for(GIT_REF)?.scope(None)?;
        let mut rows = Vec::with_capacity(edits.len());
        for (index, edit) in edits {
            let row = self.ref_row(&edit.name, true)?;
            let stale = !Self::matches(row.as_ref(), edit);
            rows.push((*index, *edit, row, stale));
        }
        if atomic && rows.iter().any(|(_, _, _, stale)| *stale) {
            for (index, _, _, stale) in &rows {
                outcomes[*index] = if *stale { RefOutcome::Stale } else { RefOutcome::Aborted };
            }
            return Ok(scope.commit()?);
        }
        for (index, edit, row, stale) in rows {
            if stale {
                outcomes[index] = RefOutcome::Stale;
                continue;
            }
            self.apply(row, edit)?;
            outcomes[index] = RefOutcome::Updated;
        }
        Ok(scope.commit()?)
    }
}

/// A row as a collection scans it: no data.
#[derive(Clone, Debug)]
pub(crate) struct ScanRow {
    pub id: ObjectId,
    pub meta: ObjectMeta,
    /// `lastUpdatedStamp`, milliseconds.
    pub stamp: i64,
}

/// What a collection rewrites a row as.
pub(crate) enum Rewrite {
    /// The zlib stream of the content.
    Whole { data: Vec<u8> },
    /// The zlib stream of `delta_size` bytes of delta instructions against `base`.
    Delta {
        base: ObjectId,
        depth: u32,
        delta_size: u64,
        data: Vec<u8>,
    },
}

/// For `gc#GitRepositories`: what it reads and changes besides objects by id. Only it rewrites rows (holding the
/// repository's lease), and it deletes only rows not stamped since its cutoff.
impl DbStore {
    pub(crate) fn repository_id(&self) -> &str {
        &self.repository_id
    }

    pub(crate) fn entity(&self) -> &EntityFacade {
        &self.entity
    }

    /// Every row without its data, in id order, a page at a time.
    pub(crate) fn scan(&self, each: &mut dyn FnMut(ScanRow) -> Result<(), Error>) -> Result<(), Error> {
        let _rows = super::rows_guard();
        let mut after: Option<String> = None;
        loop {
            let mut find = self
                .entity
                .find(GIT_OBJECT)?
                .eq("repositoryId", Value::string(self.repository_id.as_ref()))
                .order_by("objectId", false)
                .limit(5000);
            if let Some(after) = &after {
                find = find.condition(Condition::greater("objectId", Value::string(after)));
            }
            for field in META_FIELDS {
                find = find.select_field(field);
            }
            let page = find.select_field("lastUpdatedStamp").list()?;
            if page.is_empty() {
                return Ok(());
            }
            for value in page.iter() {
                let row = self.row_of(value)?;
                after = Some(row.id.to_string());
                each(ScanRow {
                    id: row.id,
                    meta: row.meta,
                    stamp: row.stamp.unwrap_or(0),
                })?;
            }
        }
    }

    /// The rows stamped at or after `since` (milliseconds), without their data.
    pub(crate) fn touched_since(&self, since: i64) -> Result<Vec<ScanRow>, Error> {
        let _rows = super::rows_guard();
        let mut find = self
            .entity
            .find(GIT_OBJECT)?
            .eq("repositoryId", Value::string(self.repository_id.as_ref()))
            .condition(Condition::greater_equal("lastUpdatedStamp", stamp_value(since)));
        for field in META_FIELDS {
            find = find.select_field(field);
        }
        let list = find.select_field("lastUpdatedStamp").list()?;
        let mut rows = Vec::with_capacity(list.len());
        for value in list.iter() {
            let row = self.row_of(value)?;
            rows.push(ScanRow {
                id: row.id,
                meta: row.meta,
                stamp: row.stamp.unwrap_or(0),
            });
        }
        Ok(rows)
    }

    /// Delete those of `ids` stamped before `cutoff` (milliseconds), in one transaction. How many were deleted.
    pub(crate) fn delete_unreferenced(&self, ids: &[ObjectId], cutoff: i64) -> Result<u64, Error> {
        let _rows = super::rows_guard();
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.entity.transactions_for(GIT_OBJECT)?;
        let scope = tx.scope(None)?;
        let mut deleted = 0;
        for chunk in ids.chunks(BATCH) {
            let condition = Condition::and(vec![
                Condition::equals("repositoryId", Value::string(self.repository_id.as_ref())),
                Condition::is_in("objectId", in_values(chunk)),
                Condition::less("lastUpdatedStamp", stamp_value(cutoff)),
            ]);
            let statement = self.entity.with_entity(GIT_OBJECT, |definition, dialect, _| {
                moqui_db::sql::delete(definition, dialect, Some(&condition))
            })?;
            deleted += crate::entity::exec::execute_tx(&tx, &statement.sql, &statement.params)?;
        }
        scope.commit()?;
        self.forget(ids);
        Ok(deleted)
    }

    /// Store the object `id` as `rewrite`: the same content, which the caller checked.
    pub(crate) fn rewrite(&self, id: ObjectId, rewrite: Rewrite) -> Result<(), Error> {
        let _rows = super::rows_guard();
        let (storage, base, depth, delta_size, data) = match rewrite {
            Rewrite::Whole { data } => (Storage::Zlib, None, 0, None, data),
            Rewrite::Delta {
                base,
                depth,
                delta_size,
                data,
            } => (Storage::Delta, Some(base), depth, Some(delta_size), data),
        };
        let condition = Condition::and(vec![
            Condition::equals("repositoryId", Value::string(self.repository_id.as_ref())),
            Condition::equals("objectId", Value::string(id.to_string())),
        ]);
        let columns = [
            "storage",
            "deltaBaseId",
            "deltaDepth",
            "deltaSize",
            "storedBytes",
            "objectData",
        ];
        let statement = self.entity.with_entity(GIT_OBJECT, |definition, dialect, _| {
            moqui_db::sql::update(definition, dialect, &columns, Some(&condition))
        })?;
        let mut params = vec![
            storage.name().map_or(Value::Null, Value::string),
            base.map_or(Value::Null, |base| Value::string(base.to_string())),
            Value::I64(i64::from(depth)),
            delta_size.map_or(Value::Null, i64_of),
            i64_of(data.len() as u64),
            Value::Bytes(data),
        ];
        params.extend(statement.params);
        let tx = self.entity.transactions_for(GIT_OBJECT)?;
        let updated = crate::entity::exec::execute_tx(&tx, &statement.sql, &params)?;
        if updated == 0 {
            return Err(Error::new(format!(
                "GitObject {id} of {} is not there to rewrite",
                self.repository_id
            )));
        }
        Ok(())
    }

    /// Set the recorded delta depth of `ids` to `depth`.
    pub(crate) fn set_depth(&self, ids: &[ObjectId], depth: u32) -> Result<(), Error> {
        let _rows = super::rows_guard();
        let tx = self.entity.transactions_for(GIT_OBJECT)?;
        for chunk in ids.chunks(BATCH) {
            let condition = Condition::and(vec![
                Condition::equals("repositoryId", Value::string(self.repository_id.as_ref())),
                Condition::is_in("objectId", in_values(chunk)),
            ]);
            let statement = self.entity.with_entity(GIT_OBJECT, |definition, dialect, _| {
                moqui_db::sql::update(definition, dialect, &["deltaDepth"], Some(&condition))
            })?;
            let mut params = vec![Value::I64(i64::from(depth))];
            params.extend(statement.params);
            crate::entity::exec::execute_tx(&tx, &statement.sql, &params)?;
        }
        Ok(())
    }

    /// Decode every object of `ids`, checking each against its id: the ids that do not decode, with why.
    pub(crate) fn check(&self, ids: &[ObjectId]) -> Vec<(ObjectId, String)> {
        let _rows = super::rows_guard();
        // the rows themselves, not what was decoded from them before
        self.forget(ids);
        match self.resolve_many(ids) {
            Ok(found) => ids
                .iter()
                .filter(|id| !found.contains_key(*id))
                .map(|id| (*id, "not found".to_string()))
                .collect(),
            // one at a time, to name the bad ones
            Err(_) => ids
                .iter()
                .filter_map(|id| match self.resolve_many(std::slice::from_ref(id)) {
                    Ok(found) if found.contains_key(id) => None,
                    Ok(_) => Some((*id, "not found".to_string())),
                    Err(err) => Some((*id, err.message().to_string())),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
impl DbStore {
    /// Stamp every row of the repository at `millis`, as if written then.
    pub(crate) fn stamp_all(&self, millis: i64) {
        let _rows = super::rows_guard();
        let condition = Condition::equals("repositoryId", Value::string(self.repository_id.as_ref()));
        let statement = self
            .entity
            .with_entity(GIT_OBJECT, |definition, dialect, _| {
                moqui_db::sql::update(definition, dialect, &["lastUpdatedStamp"], Some(&condition))
            })
            .unwrap();
        let mut params = vec![stamp_value(millis)];
        params.extend(statement.params);
        let tx = self.entity.transactions_for(GIT_OBJECT).unwrap();
        crate::entity::exec::execute_tx(&tx, &statement.sql, &params).unwrap();
    }
}

/// An object ready to insert.
struct NewRow {
    id: ObjectId,
    kind: Kind,
    size: u64,
    storage: Storage,
    base: Option<ObjectId>,
    depth: u32,
    delta_size: u64,
    data: Vec<u8>,
    blob_id: Option<String>,
}

impl NewRow {
    fn whole(id: ObjectId, kind: Kind, size: u64, storage: Storage, data: Vec<u8>) -> Self {
        Self {
            id,
            kind,
            size,
            storage,
            base: None,
            depth: 0,
            delta_size: 0,
            data,
            blob_id: None,
        }
    }

    fn blob(id: ObjectId, kind: Kind, size: u64, blob_id: String) -> Self {
        let mut row = Self::whole(id, kind, size, Storage::Blob, Vec::new());
        row.blob_id = Some(blob_id);
        row
    }
}

impl DbStore {
    /// The row of an object to store: a large one's content into a blob (a compressed one inflated into it as it is
    /// read, so it is never all in memory).
    fn new_row(&self, incoming: Incoming<'_>) -> Result<NewRow, Error> {
        Ok(match incoming {
            Incoming::Content { kind, data } => {
                let id = object_id(kind, &data)?;
                let size = data.len() as u64;
                if size > self.large_object {
                    let blob = self.blobs().put_bytes(&data)?;
                    NewRow::blob(id, kind, size, blob.id)
                } else if self.write_raw {
                    NewRow::whole(id, kind, size, Storage::Raw, data)
                } else {
                    NewRow::whole(id, kind, size, Storage::Zlib, deflate(&data))
                }
            }
            Incoming::Zlib { id, kind, size, data } if size > self.large_object => {
                let mut writer = self.blobs().begin()?;
                let mut decoder = flate2::read::ZlibDecoder::new(&data[..]);
                std::io::copy(&mut decoder, &mut writer)
                    .map_err(|err| Error::new(format!("storing git object {id} as a blob: {err}")))?;
                let expected = crate::resource::blob::Expected {
                    id: None,
                    size: Some(size),
                };
                let blob = writer.finish(&expected)?;
                NewRow::blob(id, kind, size, blob.id)
            }
            Incoming::Zlib { id, kind, size, data } => NewRow::whole(id, kind, size, Storage::Zlib, data.into_owned()),
            Incoming::Delta {
                id,
                kind,
                size,
                base,
                depth,
                delta_size,
                data,
            } => NewRow {
                id,
                kind,
                size,
                storage: Storage::Delta,
                base: Some(base),
                depth,
                delta_size,
                data: data.into_owned(),
                blob_id: None,
            },
        })
    }
}

/// The insert of the store's own statements, made from the entity definition.
struct Statements {
    insert: String,
    columns: Vec<&'static str>,
}

impl DbStore {
    fn statements(&self) -> Result<Statements, Error> {
        self.entity
            .with_entity(GIT_OBJECT, |definition, dialect, _| {
                let mut columns = vec![
                    "repositoryId",
                    "objectId",
                    "objectType",
                    "objectSize",
                    "storage",
                    "deltaBaseId",
                    "deltaDepth",
                    "deltaSize",
                    "storedBytes",
                    "objectData",
                    "blobId",
                ];
                if definition.field("lastUpdatedStamp").is_some() {
                    columns.push("lastUpdatedStamp");
                }
                let mut insert = moqui_db::sql::insert(definition, dialect, &columns)?;
                // another writer may store the same object at the same time: the row is the same, so the second insert
                // does nothing (and only a duplicate key is passed over, never a missing delta base)
                let key = |name: &str| {
                    moqui_db::sql::quote_column(
                        dialect,
                        &definition
                            .field(name)
                            .map(|field| field.column_name.clone())
                            .unwrap_or_default(),
                    )
                };
                if moqui_db::sql::is_mysql(dialect) {
                    let column = key("repositoryId");
                    insert.push_str(&format!(" ON DUPLICATE KEY UPDATE {column} = {column}"));
                } else if dialect.name.starts_with("postgres") {
                    insert.push_str(&format!(
                        " ON CONFLICT ({}, {}) DO NOTHING",
                        key("repositoryId"),
                        key("objectId")
                    ));
                }
                Ok(Statements { insert, columns })
            })
            .map_err(Error::from)
    }

    fn insert_params(&self, row: &NewRow, columns: &[&str], stamp: &Value) -> Vec<Value> {
        columns
            .iter()
            .map(|column| match *column {
                "repositoryId" => Value::string(self.repository_id.as_ref()),
                "objectId" => Value::string(row.id.to_string()),
                "objectType" => Value::string(row.kind.to_string()),
                "objectSize" => i64_of(row.size),
                "storage" => row.storage.name().map_or(Value::Null, Value::string),
                "deltaBaseId" => row.base.map_or(Value::Null, |base| Value::string(base.to_string())),
                "deltaDepth" => Value::I64(i64::from(row.depth)),
                "deltaSize" => match row.storage {
                    Storage::Delta => i64_of(row.delta_size),
                    _ => Value::Null,
                },
                "storedBytes" => i64_of(row.data.len() as u64),
                "objectData" => match row.storage {
                    Storage::Blob => Value::Null,
                    _ => Value::Bytes(row.data.clone()),
                },
                "blobId" => row.blob_id.as_deref().map_or(Value::Null, Value::string),
                "lastUpdatedStamp" => stamp.clone(),
                _ => Value::Null,
            })
            .collect()
    }

    /// Set the stamp of those of `ids` stamped before `before`, in batches. How many rows were updated.
    fn freshen_rows(&self, ids: &[ObjectId], before: i64) -> Result<u64, Error> {
        if ids.is_empty() {
            return Ok(0);
        }
        let tx = self.entity.transactions_for(GIT_OBJECT)?;
        let now = stamp_value(now_millis());
        let mut updated = 0;
        for chunk in ids.chunks(BATCH) {
            let condition = Condition::and(vec![
                Condition::equals("repositoryId", Value::string(self.repository_id.as_ref())),
                Condition::is_in("objectId", in_values(chunk)),
                Condition::less("lastUpdatedStamp", stamp_value(before)),
            ]);
            let statement = self.entity.with_entity(GIT_OBJECT, |definition, dialect, _| {
                moqui_db::sql::update(definition, dialect, &["lastUpdatedStamp"], Some(&condition))
            })?;
            let mut params = vec![now.clone()];
            params.extend(statement.params);
            updated += crate::entity::exec::execute_tx(&tx, &statement.sql, &params)?;
        }
        Ok(updated)
    }

    /// Insert `chunk`, base before delta, and freshen the old rows of the objects already stored. How many were new.
    fn insert_chunk(&self, chunk: Vec<NewRow>, statements: &Statements) -> Result<usize, Error> {
        if chunk.is_empty() {
            return Ok(0);
        }
        let ids: Vec<ObjectId> = chunk.iter().map(|row| row.id).collect();
        let now = now_millis();
        let mut present: HashMap<ObjectId, Option<i64>> = self
            .rows(&ids, false)?
            .into_iter()
            .map(|row| (row.id, row.stamp))
            .collect();
        let stale: Vec<ObjectId> = present
            .iter()
            .filter(|(_, stamp)| stamp.is_none_or(|stamp| stamp < now - FRESHEN_AFTER_MILLIS))
            .map(|(id, _)| *id)
            .collect();
        if !stale.is_empty() && self.freshen_rows(&stale, now - FRESHEN_AFTER_MILLIS)? < stale.len() as u64 {
            // some were deleted by a collection meanwhile (or have no stamp): store them again
            let still: HashSet<ObjectId> = self.rows(&stale, false)?.into_iter().map(|row| row.id).collect();
            present.retain(|id, _| !stale.contains(id) || still.contains(id));
        }
        let tx = self.entity.transactions_for(GIT_OBJECT)?;
        let scope = tx.scope(None)?;
        let stamp = stamp_value(now);
        let mut written = 0;
        let mut seen = HashSet::new();
        for row in chunk {
            if present.contains_key(&row.id) || !seen.insert(row.id) {
                continue;
            }
            let params = self.insert_params(&row, &statements.columns, &stamp);
            written += usize::try_from(crate::entity::exec::execute_tx(&tx, &statements.insert, &params)?).unwrap_or(0);
        }
        scope.commit()?;
        Ok(written)
    }
}

impl GitStore for DbStore {
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        let _rows = super::rows_guard();
        Ok(self
            .resolve_many(std::slice::from_ref(id))?
            .remove(id)
            .map(|(kind, data)| (kind, Arc::unwrap_or_clone(data))))
    }

    fn find_many(&self, ids: &[ObjectId], each: &mut EachObject<'_>) -> Result<(), Error> {
        let _rows = super::rows_guard();
        // a batch at a time, so the decoded objects of a long list are not all in memory at once
        let mut batch: Vec<ObjectId> = Vec::new();
        let mut flush = |batch: &mut Vec<ObjectId>| -> Result<(), Error> {
            for (id, (kind, data)) in self.resolve_many(batch)? {
                each(id, kind, &data)?;
            }
            batch.clear();
            Ok(())
        };
        for id in ids {
            batch.push(*id);
            if batch.len() >= BATCH {
                flush(&mut batch)?;
            }
        }
        flush(&mut batch)
    }

    fn object_size(&self, id: &ObjectId) -> Result<Option<u64>, Error> {
        let _rows = super::rows_guard();
        Ok(self
            .rows(std::slice::from_ref(id), false)?
            .first()
            .map(|row| row.meta.size))
    }

    fn contains_many(&self, ids: &[ObjectId]) -> Result<HashSet<ObjectId>, Error> {
        let _rows = super::rows_guard();
        let mut found = HashSet::new();
        for chunk in ids.chunks(BATCH) {
            let rows = self
                .entity
                .find(GIT_OBJECT)?
                .eq("repositoryId", Value::string(self.repository_id.as_ref()))
                .condition(Condition::is_in("objectId", in_values(chunk)))
                .select_field("objectId")
                .list()?;
            for row in rows.iter() {
                if let Some(id) = text(row.get("objectId")).and_then(|hex| ObjectId::from_hex(hex.as_bytes()).ok()) {
                    found.insert(id);
                }
            }
        }
        Ok(found)
    }

    fn meta_many(&self, ids: &[ObjectId]) -> Result<HashMap<ObjectId, ObjectMeta>, Error> {
        let _rows = super::rows_guard();
        Ok(self
            .rows(ids, false)?
            .into_iter()
            .map(|row| (row.id, row.meta))
            .collect())
    }

    fn packed_many(
        &self,
        ids: &[ObjectId],
        each: &mut dyn FnMut(ObjectId, Packed) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let _rows = super::rows_guard();
        let mut found = 0;
        for chunk in ids.chunks(BATCH) {
            for mut row in self.data_rows(chunk)? {
                found += 1;
                let data = row.data.take().unwrap_or_default();
                let packed = match row.meta.storage {
                    Storage::Raw => Packed::Whole {
                        kind: row.meta.kind,
                        data,
                    },
                    Storage::Zlib => Packed::Zlib {
                        kind: row.meta.kind,
                        size: row.meta.size,
                        data,
                    },
                    Storage::Delta => Packed::Delta {
                        kind: row.meta.kind,
                        base: row.meta.base.unwrap_or(row.id),
                        delta_size: row.delta_size,
                        data,
                    },
                    Storage::Blob => Packed::Stream {
                        kind: row.meta.kind,
                        source: self.blobs().source(&self.blob_of(&row)?),
                    },
                };
                each(row.id, packed)?;
            }
        }
        if found < ids.iter().collect::<HashSet<_>>().len() {
            let present = self.contains_many(ids)?;
            if let Some(missing) = ids.iter().find(|id| !present.contains(*id)) {
                return Err(Error::new(format!("git object {missing} is missing")));
            }
        }
        Ok(())
    }

    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        let _rows = super::rows_guard();
        if !is_hex_prefix(prefix) {
            return Ok(Vec::new());
        }
        let rows = self
            .entity
            .find(GIT_OBJECT)?
            .eq("repositoryId", Value::string(self.repository_id.as_ref()))
            .condition(Condition::like(
                "objectId",
                Value::string(format!("{}%", prefix.to_ascii_lowercase())),
            ))
            .select_field("objectId")
            .order_by("objectId", false)
            .limit(i64::try_from(limit).unwrap_or(i64::MAX))
            .list()?;
        Ok(rows
            .iter()
            .filter_map(|row| text(row.get("objectId")).and_then(|hex| ObjectId::from_hex(hex.as_bytes()).ok()))
            .collect())
    }

    fn write(&self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
        let id = object_id(kind, data)?;
        let mut objects = std::iter::once(Ok(Incoming::Content {
            kind,
            data: data.to_vec(),
        }));
        self.store_objects(&mut objects)?;
        Ok(id)
    }

    /// A large object into a content blob as it is read, hashed as git hashes it on the way.
    fn write_stream(&self, kind: Kind, size: u64, input: &mut dyn std::io::Read) -> Result<ObjectId, Error> {
        if size <= self.large_object {
            let mut data = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
            std::io::Read::read_to_end(
                &mut <&mut dyn std::io::Read as std::io::Read>::take(input, size.saturating_add(1)),
                &mut data,
            )
            .map_err(|err| Error::new(format!("reading a git object: {err}")))?;
            if data.len() as u64 != size {
                return Err(Error::new(format!("a git object of {} bytes, not {size}", data.len())));
            }
            return self.write(kind, &data);
        }
        let _rows = super::rows_guard();
        let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
        hasher.update(&gix::objs::encode::loose_header(kind, size));
        let mut writer = self.blobs().begin()?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut read = 0u64;
        loop {
            let count = input
                .read(&mut buf)
                .map_err(|err| Error::new(format!("reading a git object: {err}")))?;
            if count == 0 {
                break;
            }
            read += count as u64;
            if read > size {
                return Err(Error::new(format!("a git object of more than {size} bytes")));
            }
            hasher.update(&buf[..count]);
            writer.write(&buf[..count])?;
        }
        if read != size {
            return Err(Error::new(format!("a git object of {read} bytes, not {size}")));
        }
        let id = hasher
            .try_finalize()
            .map_err(|err| Error::new(format!("hashing a git object: {err}")))?;
        if self.contains_many(std::slice::from_ref(&id))?.contains(&id) {
            // the writer, dropped, deletes what it wrote
            self.freshen(std::slice::from_ref(&id))?;
            return Ok(id);
        }
        let blob = writer.finish(&crate::resource::blob::Expected {
            id: None,
            size: Some(size),
        })?;
        let statements = self.statements()?;
        self.insert_chunk(vec![NewRow::blob(id, kind, size, blob.id)], &statements)?;
        Ok(id)
    }

    fn write_batch(&self, objects: &mut dyn Iterator<Item = Result<(Kind, Vec<u8>), Error>>) -> Result<usize, Error> {
        let mut incoming = objects.map(|object| object.map(|(kind, data)| Incoming::Content { kind, data }));
        self.store_objects(&mut incoming)
    }

    /// Chunks of about [`BATCH_BYTES`]: one query for the objects already there (freshened when old), then the new
    /// ones inserted in one transaction.
    fn store_objects(&self, objects: &mut dyn Iterator<Item = Result<Incoming<'_>, Error>>) -> Result<usize, Error> {
        let _rows = super::rows_guard();
        if !self.entity.table_ready(GIT_OBJECT) {
            return Err(Error::new("the GitObject table is not there"));
        }
        let statements = self.statements()?;
        let mut written = 0;
        let mut chunk: Vec<NewRow> = Vec::new();
        let mut bytes = 0u64;
        for object in objects {
            let row = self.new_row(object?)?;
            bytes += row.data.len() as u64;
            chunk.push(row);
            if chunk.len() >= BATCH || bytes >= BATCH_BYTES {
                written += self.insert_chunk(std::mem::take(&mut chunk), &statements)?;
                bytes = 0;
            }
        }
        written += self.insert_chunk(chunk, &statements)?;
        Ok(written)
    }

    fn content_source(&self, id: &ObjectId) -> Result<Option<Arc<dyn crate::resource::source::ByteSource>>, Error> {
        let _rows = super::rows_guard();
        match self.rows(std::slice::from_ref(id), false)?.into_iter().next() {
            Some(row) if row.meta.storage == Storage::Blob => Ok(Some(self.blobs().source(&self.blob_of(&row)?))),
            _ => Ok(None),
        }
    }

    fn freshen(&self, ids: &[ObjectId]) -> Result<(), Error> {
        let _rows = super::rows_guard();
        let before = now_millis() - FRESHEN_AFTER_MILLIS;
        let stale: Vec<ObjectId> = self
            .rows(ids, false)?
            .into_iter()
            .filter(|row| row.stamp.is_none_or(|stamp| stamp < before))
            .map(|row| row.id)
            .collect();
        self.freshen_rows(&stale, before)?;
        Ok(())
    }

    /// The commit rows by a scan in id order, a page at a time: commits are stored whole.
    fn scan_commits(&self, each: &mut EachCommit<'_>) -> Result<bool, Error> {
        let _rows = super::rows_guard();
        let mut after: Option<String> = None;
        loop {
            let mut find = self
                .entity
                .find(GIT_OBJECT)?
                .eq("repositoryId", Value::string(self.repository_id.as_ref()))
                .eq("objectType", Value::string("commit"))
                .order_by("objectId", false)
                .limit(1000);
            if let Some(after) = &after {
                find = find.condition(Condition::greater("objectId", Value::string(after)));
            }
            for field in META_FIELDS {
                find = find.select_field(field);
            }
            let page = find.select_field("objectData").list()?;
            if page.is_empty() {
                return Ok(true);
            }
            for value in page.iter() {
                let row = self.row_of(value)?;
                after = Some(row.id.to_string());
                let content = match row.meta.storage {
                    Storage::Raw => row.data.unwrap_or_default(),
                    Storage::Zlib => inflate(row.data.as_deref().unwrap_or_default(), row.meta.size)?,
                    Storage::Delta | Storage::Blob => match self.find(&row.id)? {
                        Some((_, data)) => data,
                        None => continue,
                    },
                };
                verify(row.id, Kind::Commit, &content)?;
                if !each(row.id, &content)? {
                    return Ok(true);
                }
            }
        }
    }

    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error> {
        let _rows = super::rows_guard();
        match self.ref_row(name, false)?.and_then(|row| text(row.get("objectId"))) {
            Some(hex) => ObjectId::from_hex(hex.as_bytes())
                .map(Some)
                .map_err(|err| Error::new(format!("GitRef {name} of {}: {err}", self.repository_id))),
            None => Ok(None),
        }
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
        let _rows = super::rows_guard();
        let mut find = self
            .entity
            .find(GIT_REF)?
            .eq("repositoryId", Value::string(self.repository_id.as_ref()))
            .order_by("refName", false);
        if !prefix.is_empty() {
            find = find.condition(Condition::like("refName", Value::string(format!("{prefix}%"))));
        }
        let mut refs = Vec::new();
        for row in find.list()?.iter() {
            let (Some(name), Some(hex)) = (text(row.get("refName")), text(row.get("objectId"))) else {
                continue;
            };
            // LIKE treats _ and % in the prefix as wildcards
            if !name.starts_with(prefix) {
                continue;
            }
            match ObjectId::from_hex(hex.as_bytes()) {
                Ok(id) => refs.push((name, id)),
                Err(err) => {
                    tracing::error!(repository = %self.repository_id, "GitRef {name} is not an object id: {err}")
                }
            }
        }
        Ok(refs)
    }

    /// Read each ref row for update, compare, then update, create or delete it: on Postgres the row lock (or the
    /// primary key, for a ref not there yet) keeps two writers from both moving a branch. An atomic update is one
    /// transaction; otherwise each edit has its own.
    fn update_refs(&self, edits: &[RefEdit], atomic: bool) -> Result<Vec<RefOutcome>, Error> {
        let _rows = super::rows_guard();
        let mut outcomes = vec![RefOutcome::Aborted; edits.len()];
        let mut ordered: Vec<(usize, &RefEdit)> = edits.iter().enumerate().collect();
        ordered.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        let groups: Vec<&[(usize, &RefEdit)]> = if atomic {
            vec![&ordered[..]]
        } else {
            ordered.chunks(1).collect()
        };
        for group in groups {
            if let Err(err) = self.update_in_scope(group, atomic, &mut outcomes) {
                // A duplicate key on create means another writer made the ref first: a lost race, not an error.
                let raced = group.iter().any(|(_, edit)| {
                    edit.expected.is_none()
                        && self
                            .read_ref(&edit.name)
                            .ok()
                            .flatten()
                            .is_some_and(|id| Some(id) != edit.new)
                });
                if !raced {
                    return Err(err);
                }
                for (index, edit) in group {
                    let now = self.read_ref(&edit.name)?;
                    outcomes[*index] = if now != edit.expected {
                        RefOutcome::Stale
                    } else {
                        RefOutcome::Aborted
                    };
                }
            }
        }
        Ok(outcomes)
    }
}
