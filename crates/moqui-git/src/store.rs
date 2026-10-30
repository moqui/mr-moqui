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

//! Where a repository's objects and refs live. Everything above this (paths, trees, commits, history) is written once
//! in `ops` over [`GitStore`]; a bare repository on disk ([`super::fs::FsStore`]) and rows in `GitObject` and `GitRef`
//! ([`super::db::DbStore`]) are the two kinds of storage.

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::Mutex;

use gix::objs::Kind;
use gix::ObjectId;

use moqui_core::context::Error;

/// A change to one ref: point `name` at `new` (`None`: delete it) if it still points at `expected` (`None`: the ref
/// must not exist).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefEdit {
    pub name: String,
    pub expected: Option<ObjectId>,
    pub new: Option<ObjectId>,
}

/// A ref that was created (`old` empty), moved, or deleted (`new` empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RefChange {
    pub name: String,
    pub old: Option<ObjectId>,
    pub new: Option<ObjectId>,
}

/// The refs that differ between two listings of [`GitStore::list_refs`], by name.
pub(crate) fn ref_changes(before: &[(String, ObjectId)], after: &[(String, ObjectId)]) -> Vec<RefChange> {
    let before: std::collections::BTreeMap<&str, ObjectId> =
        before.iter().map(|(name, id)| (name.as_str(), *id)).collect();
    let after: std::collections::BTreeMap<&str, ObjectId> =
        after.iter().map(|(name, id)| (name.as_str(), *id)).collect();
    let mut names: Vec<&str> = before.keys().chain(after.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter_map(|name| {
            let (old, new) = (before.get(name).copied(), after.get(name).copied());
            (old != new).then(|| RefChange {
                name: name.to_string(),
                old,
                new,
            })
        })
        .collect()
}

/// What became of one [`RefEdit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RefOutcome {
    Updated,
    /// The ref no longer pointed at the expected object: another writer moved it first.
    Stale,
    /// Not tried, as another edit of the same atomic update could not be made.
    Aborted,
}

/// How a store keeps an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Storage {
    /// The content itself.
    Raw,
    /// The zlib stream of the content, as a pack entry's body.
    Zlib,
    /// The zlib stream of git delta instructions against another object.
    Delta,
    /// A content blob (`moqui_core::resource::blob`): an object too large for a row.
    Blob,
}

impl Storage {
    /// The `GitObject.storage` value: empty for [`Storage::Raw`].
    pub(crate) fn name(self) -> Option<&'static str> {
        match self {
            Storage::Raw => None,
            Storage::Zlib => Some("zlib"),
            Storage::Delta => Some("delta"),
            Storage::Blob => Some("blob"),
        }
    }

    pub(crate) fn parse(text: Option<&str>) -> Result<Self, Error> {
        match text {
            None | Some("") => Ok(Storage::Raw),
            Some("zlib") => Ok(Storage::Zlib),
            Some("delta") => Ok(Storage::Delta),
            Some("blob") => Ok(Storage::Blob),
            Some(other) => Err(Error::new(format!("unknown git object storage {other}"))),
        }
    }
}

/// What a store knows of an object without reading its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectMeta {
    pub kind: Kind,
    pub size: u64,
    pub storage: Storage,
    /// The base of a delta.
    pub base: Option<ObjectId>,
    /// Deltas from this object to a whole one: 0 for a whole object.
    pub depth: u32,
    /// The size of what is stored.
    pub stored_bytes: u64,
}

/// An object as a store keeps it, for a pack to copy.
#[derive(Clone)]
pub(crate) enum Packed {
    /// The content, to compress.
    Whole { kind: Kind, data: Vec<u8> },
    /// The zlib stream of the `size` bytes of content: a pack entry's body as it is.
    Zlib { kind: Kind, size: u64, data: Vec<u8> },
    /// The zlib stream of `delta_size` bytes of git delta instructions that make the object from `base`.
    Delta {
        kind: Kind,
        base: ObjectId,
        delta_size: u64,
        data: Vec<u8>,
    },
    /// The content to compress as it is read: an object too large to hold.
    Stream {
        kind: Kind,
        source: std::sync::Arc<dyn moqui_core::resource::source::ByteSource>,
    },
}

/// An object to store, as a received pack has it. The id of a compressed one is known (the pack's index has it), and
/// its content was checked against it.
/// The compressed bytes are the pack's own, not copied, until a store keeps them.
#[derive(Clone, Debug)]
pub(crate) enum Incoming<'a> {
    Content {
        kind: Kind,
        data: Vec<u8>,
    },
    Zlib {
        id: ObjectId,
        kind: Kind,
        size: u64,
        data: std::borrow::Cow<'a, [u8]>,
    },
    /// The base is stored first, or is in the store already.
    Delta {
        id: ObjectId,
        kind: Kind,
        size: u64,
        base: ObjectId,
        depth: u32,
        delta_size: u64,
        data: std::borrow::Cow<'a, [u8]>,
    },
}

impl Incoming<'_> {
    /// The content of an object, for a store that keeps only content: a compressed one inflated, a delta applied to
    /// its base, which `find` reads. Its id.
    pub(crate) fn into_content(
        self,
        find: &dyn Fn(&ObjectId) -> Result<Found, Error>,
    ) -> Result<(ObjectId, Kind, Vec<u8>), Error> {
        let (id, kind, data) = match self {
            Incoming::Content { kind, data } => return Ok((object_id(kind, &data)?, kind, data)),
            Incoming::Zlib { id, kind, size, data } => (id, kind, inflate(&data, size)?),
            Incoming::Delta {
                id,
                kind,
                size,
                base,
                delta_size,
                data,
                ..
            } => {
                let (_, base_data) = find(&base)?
                    .ok_or_else(|| Error::new(format!("the delta base {base} of git object {id} is missing")))?;
                let delta = inflate(&data, delta_size)?;
                (id, kind, super::delta::apply(&base_data, &delta, Some(size))?)
            }
        };
        verify(id, kind, &data)?;
        Ok((id, kind, data))
    }
}

/// The `size` bytes a zlib stream holds; any other length is an error.
pub(crate) fn inflate(data: &[u8], size: u64) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    let expected = usize::try_from(size).map_err(|_| Error::new("a git object too large for memory"))?;
    let mut out = Vec::with_capacity(expected);
    flate2::read::ZlibDecoder::new(data)
        .take(size.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|err| Error::new(format!("inflating a git object: {err}")))?;
    if out.len() != expected {
        return Err(Error::new(format!(
            "a git object inflated to {} bytes, not {expected}",
            out.len()
        )));
    }
    Ok(out)
}

/// The zlib stream of `data`: level 6, or 1 for over 1 MiB, where the time costs more than the bytes saved.
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let level = if data.len() > 1024 * 1024 { 1 } else { 6 };
    let mut encoder =
        flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 2 + 64), flate2::Compression::new(level));
    // writing to a Vec cannot fail
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// That `data` is the content of the object `id`: a store's rows are checked as they are read, so damage (or a wrong
/// delta) is an error rather than a wrong file.
pub(crate) fn verify(id: ObjectId, kind: Kind, data: &[u8]) -> Result<(), Error> {
    let actual = object_id(kind, data)?;
    if actual != id {
        return Err(Error::new(format!(
            "git object {id} is damaged: its content hashes to {actual}"
        )));
    }
    Ok(())
}

/// An object's kind and content, when the store has it.
pub(crate) type Found = Option<(Kind, Vec<u8>)>;

/// What [`GitStore::find_many`] passes each object found to.
pub(crate) type EachObject<'a> = dyn FnMut(ObjectId, Kind, &[u8]) -> Result<(), Error> + 'a;

/// What [`GitStore::scan_commits`] passes each commit to: false stops the scan.
pub(crate) type EachCommit<'a> = dyn FnMut(ObjectId, &[u8]) -> Result<bool, Error> + 'a;

/// Objects by id and refs by full name (`refs/heads/main`). Objects are immutable, so a store may cache them.
pub(crate) trait GitStore: Send + Sync + std::fmt::Debug {
    /// The kind and raw content (no git header) of an object, or `None` when the store does not have it.
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error>;

    /// [`GitStore::find`] of many objects, each found one passed to `each` (in no set order); the ones the store does
    /// not have are passed over. A store of rows reads them a batch at a time.
    fn find_many(&self, ids: &[ObjectId], each: &mut EachObject<'_>) -> Result<(), Error> {
        for id in ids {
            if let Some((kind, data)) = self.find(id)? {
                each(*id, kind, &data)?;
            }
        }
        Ok(())
    }

    /// What the store knows of each of `ids` it has, without their content.
    fn meta_many(&self, ids: &[ObjectId]) -> Result<HashMap<ObjectId, ObjectMeta>, Error> {
        let mut found = HashMap::new();
        for id in ids {
            if let Some((kind, data)) = self.find(id)? {
                let size = data.len() as u64;
                found.insert(
                    *id,
                    ObjectMeta {
                        kind,
                        size,
                        storage: Storage::Raw,
                        base: None,
                        depth: 0,
                        stored_bytes: size,
                    },
                );
            }
        }
        Ok(found)
    }

    /// Each of `ids` as stored, for a pack, in no set order; one the store does not have is an error.
    fn packed_many(
        &self,
        ids: &[ObjectId],
        each: &mut dyn FnMut(ObjectId, Packed) -> Result<(), Error>,
    ) -> Result<(), Error> {
        for id in ids {
            let (kind, data) = self
                .find(id)?
                .ok_or_else(|| Error::new(format!("git object {id} is missing")))?;
            each(*id, Packed::Whole { kind, data })?;
        }
        Ok(())
    }

    /// Store objects as a received pack has them, the bases of deltas first, skipping those already there. How many
    /// were new.
    fn store_objects(&self, objects: &mut dyn Iterator<Item = Result<Incoming<'_>, Error>>) -> Result<usize, Error> {
        let mut contents = objects.map(|object| {
            let (_, kind, data) = object?.into_content(&|id| self.find(id))?;
            Ok((kind, data))
        });
        self.write_batch(&mut contents)
    }

    /// The content of an object to read a window at a time, when the store keeps it apart from its rows (a large
    /// object's content blob); `None` otherwise, and the caller reads it with [`GitStore::find`].
    fn content_source(
        &self,
        _id: &ObjectId,
    ) -> Result<Option<std::sync::Arc<dyn moqui_core::resource::source::ByteSource>>, Error> {
        Ok(None)
    }

    /// Note that `ids`, which the store has, are referenced again (by a push, a write), so a collection of
    /// unreachable objects running at the same time keeps them.
    fn freshen(&self, _ids: &[ObjectId]) -> Result<(), Error> {
        Ok(())
    }

    /// Every commit the store has, for a walk of much of the history, when the store can read them faster in one
    /// pass than one at a time; `each` returns false to stop. False when the store cannot, and nothing was passed to
    /// `each`.
    fn scan_commits(&self, _each: &mut EachCommit<'_>) -> Result<bool, Error> {
        Ok(false)
    }

    /// The size of an object's content without reading it all, where the store can.
    fn object_size(&self, id: &ObjectId) -> Result<Option<u64>, Error> {
        Ok(self.find(id)?.map(|(_, data)| data.len() as u64))
    }

    /// Which of `ids` the store has.
    fn contains_many(&self, ids: &[ObjectId]) -> Result<HashSet<ObjectId>, Error> {
        let mut found = HashSet::new();
        for id in ids {
            if self.object_size(id)?.is_some() {
                found.insert(*id);
            }
        }
        Ok(found)
    }

    /// The objects whose ids start with the hex digits `prefix`, at most `limit` of them.
    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error>;

    /// Store an object and answer its id. Writing an object that is already there is not an error.
    fn write(&self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error>;

    /// Store an object of `size` bytes read from `input`, and answer its id. A store that keeps large objects apart
    /// takes it as it is read; by default it is read into memory.
    fn write_stream(&self, kind: Kind, size: u64, input: &mut dyn std::io::Read) -> Result<ObjectId, Error> {
        let mut data = Vec::with_capacity(usize::try_from(size).unwrap_or(0).min(64 * 1024 * 1024));
        std::io::Read::read_to_end(
            &mut <&mut dyn std::io::Read as std::io::Read>::take(input, size.saturating_add(1)),
            &mut data,
        )
        .map_err(|err| Error::new(format!("reading a git object: {err}")))?;
        if data.len() as u64 != size {
            return Err(Error::new(format!("a git object of {} bytes, not {size}", data.len())));
        }
        self.write(kind, &data)
    }

    /// Store many objects, skipping those already there. How many were new.
    fn write_batch(&self, objects: &mut dyn Iterator<Item = Result<(Kind, Vec<u8>), Error>>) -> Result<usize, Error> {
        let mut written = 0;
        for object in objects {
            let (kind, data) = object?;
            let id = object_id(kind, &data)?;
            if self.object_size(&id)?.is_none() {
                self.write(kind, &data)?;
                written += 1;
            }
        }
        Ok(written)
    }

    /// The object a ref points to, or `None` when there is no such ref.
    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error>;

    /// The refs whose full names start with `prefix` (`refs/` for all of them) and the objects they point to, by
    /// name. Symbolic refs (`HEAD`) are not listed.
    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error>;

    /// Make `edits`, each only if its ref still points at its expected object. With `atomic` either every edit is
    /// made or none is: one that cannot be made is [`RefOutcome::Stale`] and the others [`RefOutcome::Aborted`].
    /// Without it each edit stands alone. One outcome per edit, in order.
    fn update_refs(&self, edits: &[RefEdit], atomic: bool) -> Result<Vec<RefOutcome>, Error>;

    /// The directory of a repository kept on disk, whose `objects/pack` a received pack is moved into.
    fn disk_path(&self) -> Option<&std::path::Path> {
        None
    }

    /// A handle on the object database of a repository on disk, for gix-pack's pack generator (which copies deltas
    /// from the packs there).
    fn pack_handle(&self) -> Option<gix::odb::store::Handle<std::sync::Arc<gix::odb::Store>>> {
        None
    }

    /// Point `name` at `new` if it still points at `expected` (`None`: the ref must not exist). False, and nothing
    /// changed, when another writer moved the ref first.
    fn compare_and_set_ref(&self, name: &str, expected: Option<ObjectId>, new: ObjectId) -> Result<bool, Error> {
        let edit = RefEdit {
            name: name.to_string(),
            expected,
            new: Some(new),
        };
        Ok(self.update_refs(&[edit], true)?.first() == Some(&RefOutcome::Updated))
    }
}

/// Whether `text` is a usable object id prefix: hex digits, 4 to 40 of them.
pub(crate) fn is_hex_prefix(text: &str) -> bool {
    (4..=40).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The SHA-1 id of an object, as git computes it.
pub(crate) fn object_id(kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
    gix::objs::compute_hash(super::ops::HASH, kind, data).map_err(|err| Error::new(format!("hashing a {kind}: {err}")))
}

/// A store in memory, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemoryStore {
    objects: Mutex<HashMap<ObjectId, (Kind, Vec<u8>)>>,
    refs: Mutex<BTreeMap<String, ObjectId>>,
}

#[cfg(test)]
impl GitStore for MemoryStore {
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        Ok(moqui_lib::sync::lock(&self.objects).get(id).cloned())
    }

    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        let prefix = prefix.to_ascii_lowercase();
        let mut found: Vec<ObjectId> = moqui_lib::sync::lock(&self.objects)
            .keys()
            .filter(|id| id.to_string().starts_with(&prefix))
            .copied()
            .collect();
        found.sort();
        found.truncate(limit);
        Ok(found)
    }

    fn write(&self, kind: Kind, data: &[u8]) -> Result<ObjectId, Error> {
        let id = object_id(kind, data)?;
        moqui_lib::sync::lock(&self.objects).insert(id, (kind, data.to_vec()));
        Ok(id)
    }

    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error> {
        Ok(moqui_lib::sync::lock(&self.refs).get(name).copied())
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
        Ok(moqui_lib::sync::lock(&self.refs)
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|(name, id)| (name.clone(), *id))
            .collect())
    }

    fn update_refs(&self, edits: &[RefEdit], atomic: bool) -> Result<Vec<RefOutcome>, Error> {
        let mut refs = moqui_lib::sync::lock(&self.refs);
        let stale: Vec<bool> = edits
            .iter()
            .map(|edit| refs.get(&edit.name).copied() != edit.expected)
            .collect();
        if atomic && stale.contains(&true) {
            return Ok(stale
                .iter()
                .map(|stale| if *stale { RefOutcome::Stale } else { RefOutcome::Aborted })
                .collect());
        }
        Ok(edits
            .iter()
            .map(|edit| {
                if refs.get(&edit.name).copied() != edit.expected {
                    return RefOutcome::Stale;
                }
                match edit.new {
                    Some(new) => refs.insert(edit.name.clone(), new),
                    None => refs.remove(&edit.name),
                };
                RefOutcome::Updated
            })
            .collect())
    }
}
