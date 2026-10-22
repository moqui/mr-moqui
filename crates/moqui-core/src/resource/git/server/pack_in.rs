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

//! The pack a push sends, kept in quarantine until the push is accepted, as git's receive-pack does: gix-pack indexes
//! it (completing a thin pack with bases from the repository) into a directory of its own, the push's new objects are
//! read from there with the repository's under them ([`Quarantine`]), every object they refer to must be in one or the
//! other, and only an accepted push moves them into the repository. A rejected or failed push leaves nothing behind.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use gix::objs::Kind;
use gix::odb::pack::cache::lru::MemoryCappedHashmap;
use gix::odb::pack::Bundle;
use gix::ObjectId;

use gix::odb::pack::data::decode::header::ResolvedBase;
use gix::odb::pack::data::entry::Header;

use crate::context::Error;
use crate::resource::git::ops;
use crate::resource::git::store::{GitStore, Incoming, RefEdit, RefOutcome};

/// Memory for decoded delta bases while reading the quarantined pack.
const DELTA_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// A pushed delta is kept as one when its chain is at most `git-server` `db-delta-depth` (50) deep and its object at
/// most `db-delta-max-size` (16 MiB); others are stored whole, as are commits and tags.
fn delta_limits() -> (u32, u64) {
    (
        crate::resource::git::number_setting("db-delta-depth", 50),
        crate::resource::git::number_setting("db-delta-max-size", 16 * 1024 * 1024),
    )
}

fn pack_error(what: &str, err: impl std::fmt::Display) -> Error {
    Error::new(format!("{what}: {err}"))
}

/// Bases of a thin pack, looked up in the repository. A store error is kept and answered as a missing object, as the
/// lookup can only say yes or no; the caller reports the kept error.
struct StoreFind<'a> {
    store: &'a dyn GitStore,
    failed: Mutex<Option<Error>>,
}

impl gix::objs::Find for StoreFind<'_> {
    fn try_find<'a>(&self, id: &gix::hash::oid, buffer: &'a mut Vec<u8>) -> gix::Result<Option<gix::objs::Data<'a>>> {
        match self.store.find(&id.to_owned()) {
            Ok(Some((kind, data))) => {
                *buffer = data;
                Ok(Some(gix::objs::Data::new(buffer, kind, gix::hash::Kind::Sha1)))
            }
            Ok(None) => Ok(None),
            Err(err) => {
                *moqui_lib::sync::lock(&self.failed) = Some(err);
                Ok(None)
            }
        }
    }
}

/// A received pack, indexed in its quarantine directory, which is removed when this is dropped (after
/// [`Quarantine::accept`] moved the pack out of it, or not).
pub(crate) struct Quarantine<'a> {
    base: &'a dyn GitStore,
    dir: PathBuf,
    /// `None` for a pack of no objects.
    bundle: Option<Bundle>,
    files: Option<(PathBuf, PathBuf)>,
    cache: Mutex<(gix::zlib::Inflate, MemoryCappedHashmap)>,
    /// The objects of the repository the pushed ones refer to, found by [`Quarantine::check_closure`]: an accepted
    /// push freshens them.
    referenced: Mutex<Vec<ObjectId>>,
}

impl std::fmt::Debug for Quarantine<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Quarantine").field("dir", &self.dir).finish()
    }
}

impl Drop for Quarantine<'_> {
    fn drop(&mut self) {
        // the bundle maps files in the directory; unmap them first
        self.bundle = None;
        if let Err(err) = std::fs::remove_dir_all(&self.dir) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("cannot remove the git quarantine {}: {err}", self.dir.display());
            }
        }
    }
}

/// Read a pack from `input` into the new directory `dir`, completing a thin pack from `base`. `max_object` caps
/// the memory of one object (`git-server` `max-object-size`).
pub(crate) fn receive<'a>(
    base: &'a dyn GitStore,
    input: &mut dyn BufRead,
    dir: &Path,
    max_object: Option<usize>,
    threads: Option<usize>,
) -> Result<Quarantine<'a>, Error> {
    std::fs::create_dir_all(dir).map_err(|err| pack_error(&format!("cannot create {}", dir.display()), err))?;
    let mut quarantine = Quarantine {
        base,
        dir: dir.to_path_buf(),
        bundle: None,
        files: None,
        cache: Mutex::new((
            gix::zlib::Inflate::default(),
            MemoryCappedHashmap::new(DELTA_CACHE_BYTES),
        )),
        referenced: Mutex::new(Vec::new()),
    };
    let lookup = StoreFind {
        store: base,
        failed: Mutex::new(None),
    };
    let interrupt = AtomicBool::new(false);
    let written = Bundle::write_to_directory(
        input,
        Some(dir),
        &mut gix::progress::Discard,
        &interrupt,
        Some(&lookup),
        gix::hash::Kind::Sha1,
        gix::odb::pack::bundle::write::Options {
            thread_limit: threads,
            iteration_mode: gix::odb::pack::data::input::Mode::Verify,
            index_version: gix::odb::pack::index::Version::default(),
            alloc_limit_bytes: max_object,
            compression: gix::zlib::Compression::BEST_SPEED,
        },
    );
    if let Some(err) = moqui_lib::sync::lock(&lookup.failed).take() {
        return Err(err);
    }
    let written = written.map_err(|err| pack_error("reading the pushed pack", err))?;
    if let Some(keep) = &written.keep_path {
        let _ = std::fs::remove_file(keep);
    }
    if let (Some(data), Some(index)) = (written.data_path.clone(), written.index_path.clone()) {
        quarantine.bundle =
            Some(Bundle::at(&index, gix::hash::Kind::Sha1).map_err(|err| pack_error("opening the pushed pack", err))?);
        quarantine.files = Some((data, index));
    }
    Ok(quarantine)
}

impl Quarantine<'_> {
    /// The ids of the objects the pack holds.
    pub(crate) fn ids(&self) -> Vec<ObjectId> {
        self.bundle
            .as_ref()
            .map(|bundle| bundle.index.iter().map(|entry| entry.oid).collect())
            .unwrap_or_default()
    }

    fn find_in_pack(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        let Some(bundle) = &self.bundle else {
            return Ok(None);
        };
        if bundle.index.lookup(id).is_none() {
            return Ok(None);
        }
        let mut buf = Vec::new();
        let mut cache = moqui_lib::sync::lock(&self.cache);
        let (inflate, decoded) = &mut *cache;
        let found = bundle
            .find(id, &mut buf, inflate, decoded)
            .map_err(|err| pack_error(&format!("reading {id} from the pushed pack"), err))?;
        Ok(found.map(|(data, _)| (data.kind, data.data.to_vec())))
    }

    /// Check that the pushed objects are whole: each parses as its kind, and every object one refers to (a commit's
    /// tree and parents, a tree's entries, a tag's target) is in the pack or the repository. A parent that is in
    /// `shallow` (the pushing client is a shallow clone) may be missing. Submodule entries are passed over.
    pub(crate) fn check_closure(&self, shallow: &HashSet<ObjectId>) -> Result<(), Error> {
        let ids = self.ids();
        let in_pack: HashSet<ObjectId> = ids.iter().copied().collect();
        let mut outside: HashSet<ObjectId> = HashSet::new();
        let need = |id: ObjectId, outside: &mut HashSet<ObjectId>| {
            if !in_pack.contains(&id) {
                outside.insert(id);
            }
        };
        for id in &ids {
            if self.kind_in_pack(id)? == Some(Kind::Blob) {
                continue;
            }
            let (kind, data) = self
                .find_in_pack(id)?
                .ok_or_else(|| Error::new(format!("object {id} is listed in the pushed pack but cannot be read")))?;
            match kind {
                Kind::Commit => {
                    let commit = gix::objs::CommitRef::from_bytes(&data, ops::HASH)
                        .map_err(|err| pack_error(&format!("pushed commit {id} is malformed"), err))?;
                    need(commit.tree(), &mut outside);
                    for parent in commit.parents() {
                        if !shallow.contains(&parent) {
                            need(parent, &mut outside);
                        }
                    }
                }
                Kind::Tree => {
                    let tree = gix::objs::TreeRef::from_bytes(&data, ops::HASH)
                        .map_err(|err| pack_error(&format!("pushed tree {id} is malformed"), err))?;
                    for entry in tree.entries {
                        if !entry.mode.is_commit() {
                            need(entry.oid.to_owned(), &mut outside);
                        }
                    }
                }
                Kind::Tag => {
                    let tag = gix::objs::TagRef::from_bytes(&data, ops::HASH)
                        .map_err(|err| pack_error(&format!("pushed tag {id} is malformed"), err))?;
                    need(tag.target(), &mut outside);
                }
                Kind::Blob => {}
            }
        }
        let outside: Vec<ObjectId> = outside.into_iter().collect();
        for chunk in outside.chunks(500) {
            let present = self.base.contains_many(chunk)?;
            if let Some(missing) = chunk.iter().find(|id| !present.contains(*id)) {
                return Err(Error::new(format!(
                    "the push is missing object {missing}, which a pushed object refers to"
                )));
            }
        }
        *moqui_lib::sync::lock(&self.referenced) = outside;
        Ok(())
    }

    /// The kind of a pushed object from its entry's header (and those of its delta bases), without decoding it.
    fn kind_in_pack(&self, id: &ObjectId) -> Result<Option<Kind>, Error> {
        let Some(bundle) = &self.bundle else {
            return Ok(None);
        };
        let Some(index) = bundle.index.lookup(id) else {
            return Ok(None);
        };
        let entry = bundle
            .pack
            .entry(bundle.index.pack_offset_at_index(index))
            .map_err(|err| pack_error(&format!("reading {id} from the pushed pack"), err))?;
        let mut cache = moqui_lib::sync::lock(&self.cache);
        let outcome = bundle
            .pack
            .decode_header(entry, &mut cache.0, &|base| resolve_in(bundle, base))
            .map_err(|err| pack_error(&format!("reading {id} from the pushed pack"), err))?;
        Ok(Some(outcome.kind))
    }

    /// Move the pushed objects into the repository: the pack files into its `objects/pack` on disk, or each object
    /// into the store otherwise. How many objects.
    pub(crate) fn accept(&mut self) -> Result<usize, Error> {
        let ids = self.ids();
        let Some((data, index)) = self.files.clone() else {
            return Ok(0);
        };
        if let Some(repository) = self.base.disk_path() {
            let pack_dir = repository.join("objects").join("pack");
            std::fs::create_dir_all(&pack_dir)
                .map_err(|err| pack_error(&format!("cannot create {}", pack_dir.display()), err))?;
            // the pack before its index: a reader finds a pack by its index
            for file in [&data, &index] {
                let name = file
                    .file_name()
                    .ok_or_else(|| Error::new(format!("pushed pack file {} has no name", file.display())))?;
                let target = pack_dir.join(name);
                if target.exists() {
                    continue;
                }
                std::fs::rename(file, &target)
                    .map_err(|err| pack_error(&format!("cannot move {} into the repository", file.display()), err))?;
            }
            return Ok(ids.len());
        }
        let referenced = std::mem::take(&mut *moqui_lib::sync::lock(&self.referenced));
        self.base.freshen(&referenced)?;
        self.store_entries()?;
        Ok(ids.len())
    }

    /// Store the pushed objects as the pack has them, in its order (a delta's base is before it): a whole object's
    /// compressed entry as it is, a delta as a delta against its base, unless it is a commit or tag, its object is
    /// large or its chain deep (stored whole then). Objects the repository has already are freshened.
    fn store_entries(&self) -> Result<(), Error> {
        let Some(bundle) = &self.bundle else {
            return Ok(());
        };
        let (max_depth, max_size) = delta_limits();
        let mut entries: Vec<(u64, ObjectId)> = bundle
            .index
            .iter()
            .map(|entry| (entry.pack_offset, entry.oid))
            .collect();
        entries.sort_unstable_by_key(|(offset, _)| *offset);
        let by_offset: HashMap<u64, ObjectId> = entries.iter().map(|(offset, id)| (*offset, *id)).collect();
        let ids: Vec<ObjectId> = entries.iter().map(|(_, id)| *id).collect();
        let existing = self.base.meta_many(&ids)?;
        self.base.freshen(&existing.keys().copied().collect::<Vec<_>>())?;
        // how deep each stored object's delta chain is
        let mut depths: HashMap<ObjectId, u32> = existing.iter().map(|(id, meta)| (*id, meta.depth)).collect();
        let end = bundle.pack.pack_end() as u64;
        let mut at = 0;
        let mut objects = std::iter::from_fn(|| {
            while at < entries.len() {
                let (offset, id) = entries[at];
                let next = entries.get(at + 1).map_or(end, |(offset, _)| *offset);
                at += 1;
                if existing.contains_key(&id) {
                    continue;
                }
                return Some(self.incoming(bundle, offset, next, id, &by_offset, &mut depths, max_depth, max_size));
            }
            None
        });
        self.base.store_objects(&mut objects)?;
        Ok(())
    }

    /// The pushed object `id` whose entry is at `offset` (up to `next`), to store.
    #[allow(clippy::too_many_arguments)]
    fn incoming<'b>(
        &self,
        bundle: &'b Bundle,
        offset: u64,
        next: u64,
        id: ObjectId,
        by_offset: &HashMap<u64, ObjectId>,
        depths: &mut HashMap<ObjectId, u32>,
        max_depth: u32,
        max_size: u64,
    ) -> Result<Incoming<'b>, Error> {
        let read_error = |err: &dyn std::fmt::Display| pack_error(&format!("reading {id} from the pushed pack"), err);
        let entry = bundle.pack.entry(offset).map_err(|err| read_error(&err))?;
        let data = bundle
            .pack
            .entry_slice(entry.data_offset..next)
            .ok_or_else(|| Error::new(format!("the entry of {id} is outside the pushed pack")))?;
        let base = match entry.header {
            Header::Commit | Header::Tree | Header::Blob | Header::Tag => {
                depths.insert(id, 0);
                return Ok(Incoming::Zlib {
                    id,
                    kind: entry.header.as_kind().unwrap_or(Kind::Blob),
                    size: entry.decompressed_size,
                    data: std::borrow::Cow::Borrowed(data),
                });
            }
            Header::OfsDelta { base_distance } => offset
                .checked_sub(base_distance)
                .and_then(|base| by_offset.get(&base))
                .copied(),
            Header::RefDelta { base_id } => Some(base_id),
        };
        let delta_size = entry.decompressed_size;
        let outcome = {
            let mut cache = moqui_lib::sync::lock(&self.cache);
            bundle
                .pack
                .decode_header(entry, &mut cache.0, &|base| resolve_in(bundle, base))
                .map_err(|err| read_error(&err))?
        };
        let depth = base.and_then(|base| depths.get(&base)).map(|depth| depth + 1);
        let keep = matches!(outcome.kind, Kind::Tree | Kind::Blob)
            && outcome.object_size <= max_size
            && depth.is_some_and(|depth| depth <= max_depth);
        match (keep, base, depth) {
            (true, Some(base), Some(depth)) => {
                depths.insert(id, depth);
                Ok(Incoming::Delta {
                    id,
                    kind: outcome.kind,
                    size: outcome.object_size,
                    base,
                    depth,
                    delta_size,
                    data: std::borrow::Cow::Borrowed(data),
                })
            }
            _ => {
                // a whole object; a delta whose base comes later in the pack (allowed for a REF_DELTA) too
                let (kind, data) = self
                    .find_in_pack(&id)?
                    .ok_or_else(|| Error::new(format!("object {id} of the pushed pack cannot be read")))?;
                depths.insert(id, 0);
                Ok(Incoming::Content { kind, data })
            }
        }
    }
}

/// Where the base of a `REF_DELTA` entry is: in the pack (a thin pack's bases from the repository are added to it
/// as it is indexed).
fn resolve_in(bundle: &Bundle, base: &gix::hash::oid) -> gix::Result<Option<ResolvedBase>> {
    let Some(index) = bundle.index.lookup(base) else {
        return Ok(None);
    };
    let entry = bundle.pack.entry(bundle.index.pack_offset_at_index(index))?;
    Ok(Some(ResolvedBase::InPack(entry)))
}

/// The repository with the quarantined objects over it: what pre-receive rules and checks read before a push is
/// accepted. Read only.
impl GitStore for Quarantine<'_> {
    fn find(&self, id: &ObjectId) -> Result<Option<(Kind, Vec<u8>)>, Error> {
        match self.find_in_pack(id)? {
            Some(found) => Ok(Some(found)),
            None => self.base.find(id),
        }
    }

    fn object_size(&self, id: &ObjectId) -> Result<Option<u64>, Error> {
        match self.find_in_pack(id)? {
            Some((_, data)) => Ok(Some(data.len() as u64)),
            None => self.base.object_size(id),
        }
    }

    fn contains_many(&self, ids: &[ObjectId]) -> Result<HashSet<ObjectId>, Error> {
        let bundle = self.bundle.as_ref();
        let (here, rest): (Vec<ObjectId>, Vec<ObjectId>) = ids
            .iter()
            .partition(|id| bundle.is_some_and(|bundle| bundle.index.lookup(*id).is_some()));
        let mut found: HashSet<ObjectId> = here.into_iter().collect();
        found.extend(self.base.contains_many(&rest)?);
        Ok(found)
    }

    fn find_prefix(&self, prefix: &str, limit: usize) -> Result<Vec<ObjectId>, Error> {
        self.base.find_prefix(prefix, limit)
    }

    fn write(&self, _kind: Kind, _data: &[u8]) -> Result<ObjectId, Error> {
        Err(Error::new("a push's quarantine is read only"))
    }

    fn read_ref(&self, name: &str) -> Result<Option<ObjectId>, Error> {
        self.base.read_ref(name)
    }

    fn list_refs(&self, prefix: &str) -> Result<Vec<(String, ObjectId)>, Error> {
        self.base.list_refs(prefix)
    }

    fn update_refs(&self, _edits: &[RefEdit], _atomic: bool) -> Result<Vec<RefOutcome>, Error> {
        Err(Error::new("a push's quarantine is read only"))
    }
}
