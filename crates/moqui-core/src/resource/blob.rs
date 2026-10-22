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

//! Large content in the database: a `moqui.resource.ContentBlob` per content, by its SHA-256 (so the same bytes are
//! stored once), its bytes `ContentBlobChunk` rows of `resource-facade` `blob-chunk-size` (1 MiB). Git LFS objects, the large objects
//! of `db` git repositories and the binary files of `dbresource://` are blobs; a row refers to one by its `blobId`.
//! No Java counterpart. Guide: `docs/Git-Hosting.md`, Storage.
//!
//! A blob is written as a stream, each chunk in a transaction of its own (so a large file is not one transaction),
//! under an upload's own `storageKey` (`ContentBlobUpload`): inserting the `ContentBlob` row publishes it. Two
//! uploads of the same bytes at once, or of bytes already stored, end with one blob: the later one drops its chunks.
//! An upload a crash cut short is cleaned up by `gc#ContentBlobs`, which also deletes the blobs no row refers to (by a
//! relationship to `ContentBlob`) and that were not written or referenced again since its cutoff.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, to_plain_string, Value};
use sha2::{Digest, Sha256};

use super::source::ByteSource;
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

pub const CONTENT_BLOB: &str = "moqui.resource.ContentBlob";
pub const CONTENT_BLOB_CHUNK: &str = "moqui.resource.ContentBlobChunk";
pub const CONTENT_BLOB_UPLOAD: &str = "moqui.resource.ContentBlobUpload";

/// A blob referenced again is freshened when its stamp is older than this.
const FRESHEN_AFTER_MILLIS: i64 = 60 * 60 * 1000;

/// An upload with no chunk written for this long is abandoned.
const ABANDONED_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// A stored blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobInfo {
    /// The SHA-256 of the content, in lowercase hex.
    pub id: String,
    pub size: u64,
    storage_key: String,
    chunk_size: u64,
}

/// What an upload must turn out to be.
#[derive(Clone, Debug, Default)]
pub struct Expected {
    /// The SHA-256 in hex.
    pub id: Option<String>,
    pub size: Option<u64>,
}

/// The blob store of an entity facade.
#[derive(Clone)]
pub struct BlobStore {
    entity: EntityFacade,
    chunk_size: usize,
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn stamp(millis: i64) -> Value {
    basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        value => Some(to_plain_string(value)).filter(|text| !text.is_empty()),
    }
}

fn long(value: Option<&Value>) -> Option<i64> {
    match basic_convert(value?, "Long").value {
        Value::I64(number) => Some(number),
        _ => None,
    }
}

fn i64_of(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// Whether `id` is a SHA-256 in lowercase hex.
pub fn is_blob_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The SHA-256 of `data` in hex: the id it is stored by.
pub fn blob_id(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Authorization off for the store's own rows: the caller checked access to what refers to them.
fn guard() -> Option<crate::context::AuthzGuard> {
    crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz)
}

impl BlobStore {
    pub fn new(entity: EntityFacade) -> Self {
        let chunk_size = super::setting("blob-chunk-size")
            .parse::<usize>()
            .ok()
            .filter(|size| *size >= 4096)
            .unwrap_or(1024 * 1024);
        Self { entity, chunk_size }
    }

    /// In a transaction of its own, whatever the caller's.
    fn own_tx<T>(&self, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.entity
            .transactions_for(CONTENT_BLOB)?
            .run(TransactionAttribute::ForceNew, None, body)
    }

    fn info_of(row: &EntityValue) -> Option<BlobInfo> {
        Some(BlobInfo {
            id: text(row.get("blobId"))?,
            size: u64::try_from(long(row.get("blobSize"))?).ok()?,
            storage_key: text(row.get("storageKey"))?,
            chunk_size: u64::try_from(long(row.get("chunkSize")).unwrap_or(1))
                .unwrap_or(1)
                .max(1),
        })
    }

    /// The blob `id`, when it is stored.
    pub fn stat(&self, id: &str) -> Result<Option<BlobInfo>, Error> {
        let _guard = guard();
        if !is_blob_id(id) {
            return Ok(None);
        }
        Ok(self
            .entity
            .find(CONTENT_BLOB)?
            .eq("blobId", Value::string(id))
            .one()?
            .as_ref()
            .and_then(Self::info_of))
    }

    /// Note that `id` is referenced again, so a collection running now keeps it. False when it is not stored.
    pub fn freshen(&self, id: &str) -> Result<bool, Error> {
        let _guard = guard();
        let Some(row) = self.entity.find(CONTENT_BLOB)?.eq("blobId", Value::string(id)).one()? else {
            return Ok(false);
        };
        let old = basic_convert(row.get("lastUpdatedStamp").unwrap_or(&Value::Null), "Long").value;
        if matches!(old, Value::I64(old) if old >= now_millis() - FRESHEN_AFTER_MILLIS) {
            return Ok(true);
        }
        Ok(self.set_stamp(id, now_millis())? > 0)
    }

    /// Set the stamp of the blob `id` (an entity update of the stamp alone writes nothing, as in Java).
    fn set_stamp(&self, id: &str, millis: i64) -> Result<u64, Error> {
        let condition = Condition::equals("blobId", Value::string(id));
        let statement = self.entity.with_entity(CONTENT_BLOB, |definition, dialect, _| {
            moqui_db::sql::update(definition, dialect, &["lastUpdatedStamp"], Some(&condition))
        })?;
        let mut params = vec![stamp(millis)];
        params.extend(statement.params);
        let tx = self.entity.transactions_for(CONTENT_BLOB)?;
        self.own_tx(|| crate::entity::exec::execute_tx(&tx, &statement.sql, &params))
    }

    /// Start an upload of a blob.
    pub fn begin(&self) -> Result<BlobWriter, Error> {
        let _guard = guard();
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|err| Error::new(format!("could not read random bytes: {err}")))?;
        let key = hex(&random);
        self.own_tx(|| {
            let mut row = EntityValue::make(&self.entity, CONTENT_BLOB_UPLOAD)?;
            row.set("storageKey", Value::string(&key));
            row.set("status", Value::string("open"));
            row.set("startedDate", stamp(now_millis()));
            row.set("lastActivityDate", stamp(now_millis()));
            if let Some(user) = crate::user::current_user_id().ok().flatten() {
                row.set("userId", Value::string(user));
            }
            row.create(&self.entity)
        })?;
        Ok(BlobWriter {
            store: self.clone(),
            key,
            hasher: Sha256::new(),
            pending: Vec::with_capacity(self.chunk_size),
            seq: 0,
            received: 0,
            max: u64::MAX,
            done: false,
        })
    }

    /// Store what `input` gives, at most `max` bytes, checked against `expected`.
    pub fn put(&self, input: &mut dyn Read, expected: &Expected, max: u64) -> Result<BlobInfo, Error> {
        let mut writer = self.begin()?;
        writer.max = max;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let count = input
                .read(&mut buf)
                .map_err(|err| Error::new(format!("reading the content of a blob: {err}")))?;
            if count == 0 {
                break;
            }
            writer.write(&buf[..count])?;
        }
        writer.finish(expected)
    }

    pub fn put_bytes(&self, data: &[u8]) -> Result<BlobInfo, Error> {
        self.put(&mut &data[..], &Expected::default(), u64::MAX)
    }

    /// The content of a stored blob, read a chunk at a time.
    pub fn source(&self, info: &BlobInfo) -> Arc<dyn ByteSource> {
        Arc::new(BlobSource {
            store: self.clone(),
            info: info.clone(),
        })
    }

    /// The chunks `first..=last` of a blob, in order.
    fn chunks(&self, info: &BlobInfo, first: u64, last: u64) -> Result<Vec<Vec<u8>>, Error> {
        let _guard = guard();
        let seqs: Vec<Value> = (first..=last).map(i64_of).collect();
        let rows = self
            .entity
            .find(CONTENT_BLOB_CHUNK)?
            .eq("storageKey", Value::string(&info.storage_key))
            .condition(Condition::is_in("chunkSeq", seqs))
            .list()?;
        let mut by_seq: HashMap<u64, Vec<u8>> = HashMap::new();
        for row in rows.iter() {
            let seq = u64::try_from(long(row.get("chunkSeq")).unwrap_or(-1)).unwrap_or(u64::MAX);
            let data = match basic_convert(row.get("chunkData").unwrap_or(&Value::Null), "java.sql.Blob").value {
                Value::Bytes(bytes) => bytes,
                _ => Vec::new(),
            };
            by_seq.insert(seq, data);
        }
        (first..=last)
            .map(|seq| {
                by_seq
                    .remove(&seq)
                    .ok_or_else(|| Error::new(format!("chunk {seq} of blob {} is missing", info.id)))
            })
            .collect()
    }

    /// Up to `max` bytes of a blob from `offset`.
    pub fn read_range(&self, info: &BlobInfo, offset: u64, max: usize) -> Result<Vec<u8>, Error> {
        if offset >= info.size || max == 0 {
            return Ok(Vec::new());
        }
        let end = info.size.min(offset.saturating_add(max as u64));
        let first = offset / info.chunk_size;
        let last = (end - 1) / info.chunk_size;
        let mut out = Vec::with_capacity(usize::try_from(end - offset).unwrap_or(0));
        // a few chunks per query
        let mut seq = first;
        while seq <= last {
            let to = last.min(seq + 7);
            for (index, chunk) in self.chunks(info, seq, to)?.into_iter().enumerate() {
                let chunk_start = (seq + index as u64) * info.chunk_size;
                let from = usize::try_from(offset.saturating_sub(chunk_start))
                    .unwrap_or(0)
                    .min(chunk.len());
                let until = usize::try_from(end - chunk_start)
                    .unwrap_or(usize::MAX)
                    .min(chunk.len());
                out.extend_from_slice(&chunk[from..until.max(from)]);
            }
            seq = to + 1;
        }
        Ok(out)
    }

    /// All of a blob, for one small enough to hold.
    pub fn read(&self, id: &str) -> Result<Option<Vec<u8>>, Error> {
        let Some(info) = self.stat(id)? else {
            return Ok(None);
        };
        let size = usize::try_from(info.size).map_err(|_| Error::new("a blob too large for memory"))?;
        let data = self.read_range(&info, 0, size)?;
        if data.len() != size {
            return Err(Error::new(format!("blob {id} has {} of its {size} bytes", data.len())));
        }
        Ok(Some(data))
    }

    fn delete_chunks(&self, key: &str) -> Result<(), Error> {
        let _guard = guard();
        let condition = Condition::equals("storageKey", Value::string(key));
        let statement = self.entity.with_entity(CONTENT_BLOB_CHUNK, |definition, dialect, _| {
            moqui_db::sql::delete(definition, dialect, Some(&condition))
        })?;
        let tx = self.entity.transactions_for(CONTENT_BLOB_CHUNK)?;
        self.own_tx(|| {
            crate::entity::exec::execute_tx(&tx, &statement.sql, &statement.params)?;
            Ok(())
        })
    }

    fn delete_upload(&self, key: &str) -> Result<(), Error> {
        let _guard = guard();
        self.own_tx(|| {
            self.entity
                .find(CONTENT_BLOB_UPLOAD)?
                .eq("storageKey", Value::string(key))
                .delete_all()?;
            Ok(())
        })
    }

    /// The fields that refer to a blob: every relationship of an entity to `ContentBlob`.
    fn references(&self) -> Vec<(String, String)> {
        self.entity.with_model(|model| {
            let mut found = Vec::new();
            for entity in model.entities() {
                for relationship in &entity.relationships {
                    if relationship.related_entity_name == CONTENT_BLOB || relationship.related == CONTENT_BLOB {
                        if let Some((field, _)) = relationship.key_pairs.first() {
                            found.push((entity.full_name.clone(), field.clone()));
                        }
                    }
                }
            }
            found
        })
    }

    /// `gc#ContentBlobs`: delete the blobs no row refers to that were not written or referenced since `cutoff`
    /// (milliseconds), and the chunks of uploads abandoned for a day. (blobs deleted, bytes, uploads cleaned)
    pub fn collect(&self, cutoff: i64, dry_run: bool) -> Result<(u64, u64, u64), Error> {
        let _guard = guard();
        let references = self.references();
        let (mut deleted, mut bytes) = (0u64, 0u64);
        let mut after: Option<String> = None;
        loop {
            let mut find = self
                .entity
                .find(CONTENT_BLOB)?
                .condition(Condition::less("lastUpdatedStamp", stamp(cutoff)))
                .order_by("blobId", false)
                .limit(500);
            if let Some(after) = &after {
                find = find.condition(Condition::greater("blobId", Value::string(after)));
            }
            let page = find.list()?;
            if page.is_empty() {
                break;
            }
            let infos: Vec<BlobInfo> = page.iter().filter_map(Self::info_of).collect();
            after = infos.last().map(|info| info.id.clone());
            let ids: Vec<Value> = infos.iter().map(|info| Value::string(&info.id)).collect();
            let mut used: HashSet<String> = HashSet::new();
            for (entity, field) in &references {
                if !self.entity.table_ready(entity) {
                    continue;
                }
                for row in self
                    .entity
                    .find(entity)?
                    .condition(Condition::is_in(field.as_str(), ids.clone()))
                    .select_field(field)
                    .list()?
                    .iter()
                {
                    used.extend(text(row.get(field)));
                }
            }
            for info in infos.into_iter().filter(|info| !used.contains(&info.id)) {
                if dry_run {
                    deleted += 1;
                    bytes += info.size;
                    continue;
                }
                // the row first, only while it is still old (a reference made meanwhile freshened it); then its
                // chunks
                let condition = Condition::and(vec![
                    Condition::equals("blobId", Value::string(&info.id)),
                    Condition::less("lastUpdatedStamp", stamp(cutoff)),
                ]);
                let statement = self.entity.with_entity(CONTENT_BLOB, |definition, dialect, _| {
                    moqui_db::sql::delete(definition, dialect, Some(&condition))
                })?;
                let tx = self.entity.transactions_for(CONTENT_BLOB)?;
                let gone = self.own_tx(|| crate::entity::exec::execute_tx(&tx, &statement.sql, &statement.params));
                match gone {
                    Ok(1) => {
                        self.delete_chunks(&info.storage_key)?;
                        deleted += 1;
                        bytes += info.size;
                    }
                    Ok(_) => {}
                    // a row that refers to it now (a foreign key) keeps it
                    Err(err) => tracing::warn!("blob {} was not deleted: {}", info.id, err.message()),
                }
            }
        }
        let mut cleaned = 0;
        let abandoned = self
            .entity
            .find(CONTENT_BLOB_UPLOAD)?
            .condition(Condition::less(
                "lastActivityDate",
                stamp(now_millis() - ABANDONED_MILLIS),
            ))
            .list()?;
        for row in abandoned.iter() {
            let Some(key) = text(row.get("storageKey")) else {
                continue;
            };
            // an upload that published its blob and stopped before forgetting its row keeps its chunks
            let published = self
                .entity
                .find(CONTENT_BLOB)?
                .eq("storageKey", Value::string(&key))
                .one()?
                .is_some();
            if !dry_run {
                if !published {
                    self.delete_chunks(&key)?;
                }
                self.delete_upload(&key)?;
            }
            cleaned += 1;
        }
        Ok((deleted, bytes, cleaned))
    }
}

/// An upload of a blob in progress: [`BlobWriter::write`] the content, then [`BlobWriter::finish`]. Dropped before it
/// finishes, it deletes what it wrote.
pub struct BlobWriter {
    store: BlobStore,
    key: String,
    hasher: Sha256,
    pending: Vec<u8>,
    seq: u64,
    received: u64,
    max: u64,
    done: bool,
}

impl BlobWriter {
    /// Refuse more than `max` bytes.
    pub fn limit(mut self, max: u64) -> Self {
        self.max = max;
        self
    }

    /// The bytes written so far.
    pub fn received(&self) -> u64 {
        self.received
    }

    pub fn write(&mut self, mut bytes: &[u8]) -> Result<(), Error> {
        self.received += bytes.len() as u64;
        if self.received > self.max {
            return Err(Error::new(format!(
                "the content is over the limit of {} bytes",
                self.max
            )));
        }
        self.hasher.update(bytes);
        while !bytes.is_empty() {
            let room = self.store.chunk_size - self.pending.len();
            let take = room.min(bytes.len());
            self.pending.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.pending.len() == self.store.chunk_size {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Write the chunk gathered so far, in a transaction of its own.
    fn flush(&mut self) -> Result<(), Error> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let _guard = guard();
        let data = std::mem::replace(&mut self.pending, Vec::with_capacity(self.store.chunk_size));
        let (entity, key, seq) = (&self.store.entity, &self.key, self.seq);
        let chunk_size = self.store.chunk_size as u64;
        self.store.own_tx(|| {
            let mut chunk = EntityValue::make(entity, CONTENT_BLOB_CHUNK)?;
            chunk.set("storageKey", Value::string(key));
            chunk.set("chunkSeq", i64_of(seq));
            chunk.set("chunkData", Value::Bytes(data));
            chunk.create(entity)?;
            // now and then, so a collection sees the upload is alive
            if seq % 16 == 15 {
                if let Some(mut upload) = entity
                    .find(CONTENT_BLOB_UPLOAD)?
                    .eq("storageKey", Value::string(key))
                    .one()?
                {
                    upload.set("lastActivityDate", stamp(now_millis()));
                    upload.set("receivedSize", i64_of((seq + 1) * chunk_size));
                    upload.update(entity)?;
                }
            }
            Ok(())
        })?;
        self.seq += 1;
        Ok(())
    }

    /// Publish the blob, checked against `expected`: its id and size. When the same content is stored already, that
    /// blob (freshened), and what this upload wrote is deleted.
    pub fn finish(mut self, expected: &Expected) -> Result<BlobInfo, Error> {
        self.flush()?;
        let id = hex(&std::mem::take(&mut self.hasher).finalize());
        if let Some(size) = expected.size.filter(|size| *size != self.received) {
            return Err(Error::new(format!(
                "the content is {} bytes, not {size}",
                self.received
            )));
        }
        if let Some(want) = expected.id.as_deref().filter(|want| !want.eq_ignore_ascii_case(&id)) {
            return Err(Error::new(format!("the content's SHA-256 is {id}, not {want}")));
        }
        let store = self.store.clone();
        let _guard = guard();
        if let Some(existing) = store.stat(&id)? {
            store.freshen(&id)?;
            self.discard();
            return Ok(existing);
        }
        let info = BlobInfo {
            id: id.clone(),
            size: self.received,
            storage_key: self.key.clone(),
            chunk_size: store.chunk_size as u64,
        };
        let published = store.own_tx(|| {
            let mut row = EntityValue::make(&store.entity, CONTENT_BLOB)?;
            row.set("blobId", Value::string(&id));
            row.set("blobSize", i64_of(self.received));
            row.set("storageKind", Value::string("db"));
            row.set("storageKey", Value::string(&self.key));
            row.set("chunkSize", i64_of(store.chunk_size as u64));
            row.set("chunkCount", i64_of(self.seq));
            row.set("createdDate", stamp(now_millis()));
            row.create(&store.entity)
        });
        if let Err(err) = published {
            // another upload of the same content published first
            if let Some(existing) = store.stat(&id)? {
                self.discard();
                return Ok(existing);
            }
            return Err(err);
        }
        self.done = true;
        if let Err(err) = store.delete_upload(&self.key) {
            tracing::warn!("the upload of blob {id} was not forgotten: {}", err.message());
        }
        Ok(info)
    }

    /// Delete what this upload wrote.
    fn discard(&mut self) {
        self.done = true;
        if let Err(err) = self.store.delete_chunks(&self.key) {
            tracing::warn!("the chunks of upload {} were not deleted: {}", self.key, err.message());
        }
        if let Err(err) = self.store.delete_upload(&self.key) {
            tracing::warn!("the upload {} was not forgotten: {}", self.key, err.message());
        }
    }
}

impl std::io::Write for BlobWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        BlobWriter::write(self, buf).map_err(|err| std::io::Error::other(err.message().to_string()))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        if !self.done {
            self.discard();
        }
    }
}

/// A stored blob as a [`ByteSource`].
struct BlobSource {
    store: BlobStore,
    info: BlobInfo,
}

impl ByteSource for BlobSource {
    fn len(&self) -> u64 {
        self.info.size
    }

    fn tag(&self) -> Option<String> {
        Some(format!("sha256-{}", self.info.id))
    }

    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Error> {
        self.store.read_range(&self.info, offset, max)
    }

    fn needs_context(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (crate::context::ExecutionContext, BlobStore) {
        let context = crate::context::ExecutionContextFactory::test_context();
        let entity = crate::testing::open();
        context.factory().set_entity(entity.clone());
        let mut store = BlobStore::new(entity);
        store.chunk_size = 4096;
        (context, store)
    }

    fn rows(store: &BlobStore, entity: &str) -> usize {
        let _guard = guard();
        store.entity.find(entity).unwrap().list().unwrap().len()
    }

    #[test]
    fn blobs_are_stored_once_in_chunks_and_read_in_ranges() {
        let (context, store) = store();
        let _entered = context.enter();
        let data: Vec<u8> = (0..20_000u32).map(|n| (n * 7 % 251) as u8).collect();
        let info = store.put_bytes(&data).unwrap();
        assert_eq!(info.id, blob_id(&data));
        assert_eq!(info.size, 20_000);
        assert_eq!(rows(&store, CONTENT_BLOB_CHUNK), 5, "4096-byte chunks");
        assert_eq!(
            rows(&store, CONTENT_BLOB_UPLOAD),
            0,
            "the upload is forgotten once published"
        );
        assert_eq!(store.read(&info.id).unwrap().unwrap(), data);
        for (offset, max) in [
            (0, 10),
            (4090, 20),
            (8192, 4096),
            (19_990, 100),
            (3, 19_000),
            (20_000, 5),
        ] {
            let end = (offset + max).min(data.len());
            assert_eq!(
                store.read_range(&info, offset as u64, max).unwrap(),
                data[offset.min(end)..end],
                "{offset}+{max}"
            );
        }
        let source = store.source(&info);
        assert_eq!(source.tag(), Some(format!("sha256-{}", info.id)));
        assert_eq!(super::super::source::read_all(source.as_ref()).unwrap(), data);

        // the same bytes again are the same blob; the second upload's chunks are dropped
        let again = store.put(&mut &data[..], &Expected::default(), u64::MAX).unwrap();
        assert_eq!(again, info);
        assert_eq!(rows(&store, CONTENT_BLOB), 1);
        assert_eq!(rows(&store, CONTENT_BLOB_CHUNK), 5);

        // empty, an exact multiple of the chunk size, and checks against what was expected
        let empty = store.put_bytes(b"").unwrap();
        assert_eq!(store.read(&empty.id).unwrap().unwrap(), b"");
        let even = vec![9u8; 8192];
        assert_eq!(store.read(&store.put_bytes(&even).unwrap().id).unwrap().unwrap(), even);
        let wrong = Expected {
            id: Some(blob_id(b"something else")),
            size: None,
        };
        assert!(store.put(&mut &b"content"[..], &wrong, u64::MAX).is_err());
        let short = Expected {
            id: None,
            size: Some(3),
        };
        assert!(store.put(&mut &b"content"[..], &short, u64::MAX).is_err());
        assert!(
            store.put(&mut &data[..], &Expected::default(), 1000).is_err(),
            "over the limit"
        );
        assert_eq!(rows(&store, CONTENT_BLOB), 3, "a failed upload publishes nothing");
        assert_eq!(rows(&store, CONTENT_BLOB_CHUNK), 5 + 2, "and leaves no chunks");
        assert_eq!(rows(&store, CONTENT_BLOB_UPLOAD), 0);
    }

    #[test]
    fn a_collection_deletes_blobs_nothing_refers_to() {
        let (context, store) = store();
        let _entered = context.enter();
        let kept = store.put_bytes(b"recent").unwrap();
        let old = store.put_bytes(&vec![1u8; 10_000]).unwrap();
        // old enough: stamped long ago
        store.set_stamp(&old.id, now_millis() - 30 * ABANDONED_MILLIS).unwrap();
        // an upload abandoned a day ago
        let mut abandoned = store.begin().unwrap();
        abandoned.write(&vec![2u8; 5000]).unwrap();
        abandoned.flush().unwrap();
        let key = abandoned.key.clone();
        abandoned.done = true;
        drop(abandoned);
        {
            let _guard = guard();
            let mut row = store
                .entity
                .find(CONTENT_BLOB_UPLOAD)
                .unwrap()
                .eq("storageKey", Value::string(&key))
                .one()
                .unwrap()
                .unwrap();
            row.set("lastActivityDate", stamp(now_millis() - 2 * ABANDONED_MILLIS));
            row.update(&store.entity).unwrap();
        }
        let cutoff = now_millis() - ABANDONED_MILLIS;
        assert_eq!(store.collect(cutoff, true).unwrap(), (1, 10_000, 1), "a dry run counts");
        assert_eq!(rows(&store, CONTENT_BLOB), 2);
        assert_eq!(store.collect(cutoff, false).unwrap(), (1, 10_000, 1));
        assert!(store.stat(&old.id).unwrap().is_none());
        assert!(store.stat(&kept.id).unwrap().is_some());
        assert_eq!(rows(&store, CONTENT_BLOB_CHUNK), 1, "the recent blob's chunk only");
        assert_eq!(rows(&store, CONTENT_BLOB_UPLOAD), 0);
    }
}
