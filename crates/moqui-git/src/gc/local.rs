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

//! `gc#GitRepositories` for `local` repositories, as git's `repack -a -d` with `--unpack-unreachable` works: each push
//! leaves a pack and each write through Moqui loose objects; a run puts what is reachable in one new pack and deletes
//! the packs and loose objects it replaced. A run holds the repository's lease and goes:
//!
//! 1. Pass over a repository with one pack and fewer than [`LOOSE_OBJECTS`] loose objects: there is little to gain.
//! 2. Under the repository's writer lock, read the refs and the roots the `db` gc keeps (changes in progress and of the
//!    reflog window), and list the packs (`objects/pack/pack-*.pack` with their `.idx`) and loose objects with their
//!    times. Then let go of the lock.
//! 3. Mark what the refs reach (a missing object there stops the run) and what the other roots reach: that goes in the
//!    new pack. Mark what the recent objects reach (a loose object, or an object of a pack, written in the last
//!    `git-server` `gc-prune-days`; a pack's objects are as old as the pack, as git has it): that is kept, loose. The
//!    rest is deleted.
//! 4. Write the new pack (packed deltas are copied as they are), index it, and move it into `objects/pack` (the pack
//!    before its index). An object kept loose that is only in a pack is written loose, its time set to the pack's.
//! 5. Under the lock again: when a ref moved since step 2 onto an object that would be deleted, delete nothing. Else
//!    delete the packs (each index before its pack) and loose objects of step 2 that are now in the new pack or not
//!    kept; one that cannot be deleted is logged and the rest still go. What was written since step 2 stays.
//! 6. Forget the open repository, so the next use opens the packs as they are now.
//!
//! A pack with a `.keep` file is left alone, as git leaves it: its objects are not repacked or deleted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, UNIX_EPOCH};

use gix::objs::Kind;
use gix::ObjectId;

use super::super::fs::FsStore;
use super::super::server::{pack_out, pack_threads};
use super::super::store::GitStore;
use super::super::{ops, Definition, GitProvider};
use super::{now_millis, GcOptions, GcReport};
use moqui_core::context::Error;
use moqui_core::entity::EntityFacade;

/// A repository with one pack (or none) is repacked only when it has at least this many loose objects (a write
/// through Moqui makes a few: a blob, its trees, a commit).
const LOOSE_OBJECTS: usize = 100;

/// A pack of the repository when the run began.
struct PackFile {
    pack: PathBuf,
    index: PathBuf,
    millis: i64,
    keep: bool,
    bytes: u64,
}

/// A loose object of the repository when the run began.
struct LooseFile {
    id: ObjectId,
    path: PathBuf,
    millis: i64,
    bytes: u64,
}

fn io_error(what: &str, path: &Path, err: impl std::fmt::Display) -> Error {
    Error::new(format!("{what} {}: {err}", path.display()))
}

fn millis_of(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// The packs (with an index) and loose objects under `objects`.
fn list_files(objects: &Path) -> Result<(Vec<PackFile>, Vec<LooseFile>), Error> {
    let mut packs = Vec::new();
    let pack_dir = objects.join("pack");
    match std::fs::read_dir(&pack_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(stem) = name.strip_prefix("pack-").and_then(|rest| rest.strip_suffix(".pack")) else {
                    continue;
                };
                // a pack without its index is still being moved in: not one of the repository's yet
                let index = pack_dir.join(format!("pack-{stem}.idx"));
                let (Ok(metadata), Ok(index_metadata)) = (entry.metadata(), std::fs::metadata(&index)) else {
                    continue;
                };
                packs.push(PackFile {
                    pack: entry.path(),
                    index,
                    millis: millis_of(&metadata),
                    keep: pack_dir.join(format!("pack-{stem}.keep")).exists(),
                    bytes: metadata.len() + index_metadata.len(),
                });
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(io_error("cannot list", &pack_dir, err)),
    }
    packs.sort_by(|a, b| a.pack.cmp(&b.pack));
    let mut loose = Vec::new();
    let entries = std::fs::read_dir(objects).map_err(|err| io_error("cannot list", objects, err))?;
    for entry in entries.flatten() {
        let prefix = entry.file_name().to_string_lossy().into_owned();
        if prefix.len() != 2 || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        for file in files.flatten() {
            let rest = file.file_name().to_string_lossy().into_owned();
            let Ok(id) = ObjectId::from_hex(format!("{prefix}{rest}").as_bytes()) else {
                continue;
            };
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            loose.push(LooseFile {
                id,
                path: file.path(),
                millis: millis_of(&metadata),
                bytes: metadata.len(),
            });
        }
    }
    Ok((packs, loose))
}

/// The objects of a pack and the bytes of each one's entry.
fn pack_objects(pack: &PackFile) -> Result<Vec<(ObjectId, u64)>, Error> {
    let index = gix::odb::pack::index::File::at(&pack.index, gix::hash::Kind::Sha1)
        .map_err(|err| io_error("cannot read the pack index", &pack.index, err))?;
    let mut entries: Vec<(u64, ObjectId)> = index.iter().map(|entry| (entry.pack_offset, entry.oid)).collect();
    entries.sort();
    // the pack ends with its 20-byte hash
    let end = std::fs::metadata(&pack.pack)
        .map(|metadata| metadata.len().saturating_sub(20))
        .unwrap_or(0);
    let mut objects = Vec::with_capacity(entries.len());
    for (at, (offset, id)) in entries.iter().enumerate() {
        let next = entries.get(at + 1).map_or(end, |next| next.0);
        objects.push((*id, next.saturating_sub(*offset)));
    }
    Ok(objects)
}

/// Mark what `roots` reach into `seen`: a commit's tree and parents, a tree's entries, a tag's target. A root or what
/// it reaches that is not there is an error with `strict`, except a parent (a shallow push leaves its parents out) and
/// a submodule's commit; otherwise it is passed over. An object in `seen` already is not walked again.
fn reach(
    store: &FsStore,
    repository_id: &str,
    roots: &[ObjectId],
    strict: bool,
    seen: &mut HashSet<ObjectId>,
) -> Result<(), Error> {
    let missing = |id: &ObjectId| {
        Error::new(format!(
            "git repository {repository_id} is missing object {id}, which a ref reaches; it is not collected (run \
             fsck#GitRepository)"
        ))
    };
    // (id, a blob by its tree entry, whether it must be there)
    let mut pending: Vec<(ObjectId, bool, bool)> = roots.iter().map(|id| (*id, false, strict)).collect();
    while let Some((id, blob, required)) = pending.pop() {
        if seen.contains(&id) {
            continue;
        }
        if blob {
            // no need to read a blob: that it is there is enough
            if store.object_size(&id)?.is_some() {
                seen.insert(id);
            } else if required {
                return Err(missing(&id));
            }
            continue;
        }
        let Some((kind, data)) = store.find(&id)? else {
            if required {
                return Err(missing(&id));
            }
            continue;
        };
        seen.insert(id);
        match kind {
            Kind::Commit => {
                let commit = ops::parse_commit(&id, &data)?;
                pending.push((commit.tree, false, required));
                pending.extend(commit.parents.into_iter().map(|parent| (parent, false, false)));
            }
            Kind::Tree => {
                let tree = gix::objs::TreeRef::from_bytes(&data, ops::HASH)
                    .map_err(|err| Error::new(format!("git tree {id}: {err}")))?;
                for entry in tree.entries {
                    if !entry.mode.is_commit() {
                        pending.push((entry.oid.to_owned(), !entry.mode.is_tree(), required));
                    }
                }
            }
            Kind::Tag => {
                let target = gix::objs::TagRef::from_bytes(&data, ops::HASH)
                    .map_err(|err| Error::new(format!("git tag {id}: {err}")))?
                    .target();
                pending.push((target, false, required));
            }
            Kind::Blob => {}
        }
    }
    Ok(())
}

/// A directory removed when the run is through.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_dir_all(&self.0) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("cannot remove the git gc directory {}: {err}", self.0.display());
            }
        }
    }
}

/// Write `objects` as one pack into `objects/pack` of the repository: written and indexed in a directory of its own,
/// then the pack and then its index moved in. The new pack's path, which is that of a pack already there when it has
/// the same name (the same objects, the same way).
fn write_new_pack(store: &FsStore, root: &Path, objects: &[ObjectId]) -> Result<PathBuf, Error> {
    let scratch = Scratch(
        root.join("objects")
            .join(format!("gc-{}-{}", std::process::id(), now_millis())),
    );
    std::fs::create_dir_all(&scratch.0).map_err(|err| io_error("cannot create", &scratch.0, err))?;
    let raw = scratch.0.join("tmp_pack");
    {
        let file = std::fs::File::create(&raw).map_err(|err| io_error("cannot create", &raw, err))?;
        let mut out = std::io::BufWriter::new(file);
        let deltas = pack_out::Deltas { ofs: true, thin: None };
        pack_out::write_pack(store, objects, deltas, pack_threads(), &mut out)?;
        std::io::Write::flush(&mut out).map_err(|err| io_error("cannot write", &raw, err))?;
    }
    let mut input =
        std::io::BufReader::new(std::fs::File::open(&raw).map_err(|err| io_error("cannot open", &raw, err))?);
    let interrupt = AtomicBool::new(false);
    let written = gix::odb::pack::Bundle::write_to_directory(
        &mut input,
        Some(&scratch.0),
        &mut gix::progress::Discard,
        &interrupt,
        None::<gix::objs::find::Never>,
        gix::hash::Kind::Sha1,
        gix::odb::pack::bundle::write::Options {
            thread_limit: pack_threads(),
            ..Default::default()
        },
    )
    .map_err(|err| io_error("cannot index the new pack in", &scratch.0, err))?;
    let (Some(data), Some(index)) = (written.data_path, written.index_path) else {
        return Err(Error::new(format!(
            "the new pack in {} was not written",
            scratch.0.display()
        )));
    };
    let pack_dir = root.join("objects").join("pack");
    std::fs::create_dir_all(&pack_dir).map_err(|err| io_error("cannot create", &pack_dir, err))?;
    let target_of = |file: &Path| {
        file.file_name()
            .map(|name| pack_dir.join(name))
            .ok_or_else(|| Error::new(format!("pack file {} has no name", file.display())))
    };
    let (pack_target, index_target) = (target_of(&data)?, target_of(&index)?);
    // the pack before its index: a reader finds a pack by its index
    let moved_pack = !pack_target.exists();
    if moved_pack {
        std::fs::rename(&data, &pack_target).map_err(|err| io_error("cannot move into the repository", &data, err))?;
    }
    if !index_target.exists() {
        if let Err(err) = std::fs::rename(&index, &index_target) {
            // a pack without its index is never read: do not leave it behind
            if moved_pack {
                let _ = std::fs::remove_file(&pack_target);
            }
            return Err(io_error("cannot move into the repository", &index, err));
        }
    }
    Ok(pack_target)
}

/// Write `id` loose with its time set to `millis` (that of the pack it was in), so it keeps its age.
fn unpack(store: &FsStore, root: &Path, id: &ObjectId, millis: i64) -> Result<(), Error> {
    let (kind, data) = store
        .find(id)?
        .ok_or_else(|| Error::new(format!("git object {id} is missing")))?;
    store.write(kind, &data)?;
    let hex = id.to_string();
    // a loose object is objects/<first two hex digits>/<the rest>
    let (directory, file) = hex.split_at(2);
    let path = root.join("objects").join(directory).join(file);
    let time = UNIX_EPOCH + Duration::from_millis(u64::try_from(millis).unwrap_or(0));
    // an object left with the time of now is only kept longer
    if let Err(err) = std::fs::File::open(&path).and_then(|file| file.set_modified(time)) {
        tracing::warn!("cannot set the time of {}: {err}", path.display());
    }
    Ok(())
}

fn remove(path: &Path, repository_id: &str) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(err) => {
            tracing::error!(repository = %repository_id, "git gc could not delete {}: {err}", path.display());
            false
        }
    }
}

/// One run over the `local` repository `definition`.
pub(super) fn collect(
    provider: &GitProvider,
    entity: &EntityFacade,
    definition: &Definition,
    options: &GcOptions,
) -> Result<GcReport, Error> {
    let id = definition.id.as_str();
    let root = definition.path.as_path();
    let mut report = GcReport::default();
    if !root.join("HEAD").is_file() {
        return Ok(report);
    }
    let began = now_millis();
    let cutoff = began - options.prune_millis;
    let store = FsStore::open(root)?;
    let objects_dir = root.join("objects");

    // the refs, the roots and the files, under the writer lock
    let lock = provider.lock_of(id);
    let (roots, packs, loose) = {
        let _turn = moqui_lib::sync::lock(&lock);
        let (packs, loose) = list_files(&objects_dir)?;
        (provider.gc_roots(entity, id, &store, options)?, packs, loose)
    };
    let bytes_before: u64 =
        packs.iter().map(|pack| pack.bytes).sum::<u64>() + loose.iter().map(|file| file.bytes).sum::<u64>();
    let repackable = packs.iter().filter(|pack| !pack.keep).count();
    if repackable < 2 && loose.len() < LOOSE_OBJECTS {
        report.objects = loose.len();
        report.stored_bytes = bytes_before;
        report.stopped = Some(format!(
            "passed over (packs {repackable}, loose objects {})",
            loose.len()
        ));
        return Ok(report);
    }

    // what each object is in, and its bytes
    let mut in_packs: HashMap<ObjectId, Vec<usize>> = HashMap::new();
    let mut bytes_of: HashMap<ObjectId, u64> = HashMap::new();
    let mut kept_by_keep: HashSet<ObjectId> = HashSet::new();
    for (at, pack) in packs.iter().enumerate() {
        for (object, bytes) in pack_objects(pack)? {
            if pack.keep {
                kept_by_keep.insert(object);
                continue;
            }
            in_packs.entry(object).or_default().push(at);
            bytes_of.entry(object).or_insert(bytes);
            if in_packs.len() > options.max_objects {
                return Err(Error::new(format!(
                    "the repository has more than git-server gc-max-objects ({}) objects",
                    options.max_objects
                )));
            }
        }
    }
    let loose_ids: HashSet<ObjectId> = loose.iter().map(|file| file.id).collect();
    for file in &loose {
        bytes_of.entry(file.id).or_insert(file.bytes);
    }
    let scanned: HashSet<ObjectId> = in_packs.keys().chain(loose_ids.iter()).copied().collect();
    report.scanned = scanned.union(&kept_by_keep).count();

    // what goes in the new pack: what the refs reach, then what the reflog window and changes in progress reach
    let mut live: HashSet<ObjectId> = HashSet::new();
    reach(&store, id, &roots.refs, true, &mut live)?;
    reach(&store, id, &roots.others, false, &mut live)?;
    // what is kept loose: what the recent objects reach (beyond the live)
    let mut recent: Vec<ObjectId> = loose
        .iter()
        .filter(|file| file.millis >= cutoff)
        .map(|file| file.id)
        .collect();
    for (object, holders) in &in_packs {
        if holders.iter().any(|at| packs[*at].millis >= cutoff) {
            recent.push(*object);
        }
    }
    let mut reached = live.clone();
    reach(&store, id, &recent, false, &mut reached)?;
    let held: HashSet<ObjectId> = reached.difference(&live).copied().collect();
    let pruned: HashSet<ObjectId> = scanned
        .iter()
        .filter(|object| !reached.contains(*object) && !kept_by_keep.contains(*object))
        .copied()
        .collect();
    report.kept = report.scanned - pruned.len();
    report.deleted_bytes = pruned.iter().filter_map(|object| bytes_of.get(object)).sum();
    if options.dry_run {
        report.deleted = pruned.len() as u64;
        report.objects = report.kept;
        report.stored_bytes = bytes_before.saturating_sub(report.deleted_bytes);
        return Ok(report);
    }

    // the new pack
    let mut packed: Vec<ObjectId> = live
        .iter()
        .filter(|object| !kept_by_keep.contains(*object))
        .copied()
        .collect();
    packed.sort();
    let new_pack = if packed.is_empty() {
        None
    } else {
        Some(write_new_pack(&store, root, &packed)?)
    };
    report.repacked = packed.len();
    // an object kept loose that is only in a pack is written loose, as old as its pack
    let mut spared: HashSet<usize> = HashSet::new();
    for object in &held {
        if loose_ids.contains(object) || kept_by_keep.contains(object) {
            continue;
        }
        let Some(holders) = in_packs.get(object) else {
            continue;
        };
        let millis = holders.iter().map(|at| packs[*at].millis).max().unwrap_or(began);
        if let Err(err) = unpack(&store, root, object, millis) {
            tracing::error!(repository = %id, "git gc could not write {object} loose, so its pack stays: {}", err.message());
            spared.extend(holders.iter().copied());
        }
    }
    #[cfg(test)]
    super::super::tests::local_gc_hook(id);

    // the files replaced, under the writer lock
    {
        let _turn = moqui_lib::sync::lock(&lock);
        let before: HashSet<ObjectId> = roots.refs.iter().copied().collect();
        let moved: Vec<ObjectId> = store
            .list_refs("refs/")?
            .into_iter()
            .map(|(_, tip)| tip)
            .filter(|tip| !before.contains(tip))
            .collect();
        if !moved.is_empty() {
            let mut now_reached = reached.clone();
            reach(&store, id, &moved, false, &mut now_reached)?;
            if pruned.iter().any(|object| now_reached.contains(object)) {
                tracing::warn!(repository = %id, "git gc deletes nothing: a ref moved onto an object it would delete");
                report.objects = report.scanned;
                report.stopped = Some("a ref moved onto an unreachable object while it ran: nothing deleted".into());
                provider.refresh(id);
                return Ok(report);
            }
        }
        let mut gone_packs: HashSet<usize> = HashSet::new();
        for (at, pack) in packs.iter().enumerate() {
            if pack.keep || spared.contains(&at) || new_pack.as_deref() == Some(pack.pack.as_path()) {
                continue;
            }
            // the index before its pack: a reader finds a pack by its index
            if remove(&pack.index, id) && remove(&pack.pack, id) {
                gone_packs.insert(at);
            }
        }
        let mut gone_loose: HashSet<ObjectId> = HashSet::new();
        for file in &loose {
            // a loose object kept is held; the rest is in the new pack (or a kept pack) or not reached
            if held.contains(&file.id) {
                continue;
            }
            if remove(&file.path, id) {
                gone_loose.insert(file.id);
                if let Some(dir) = file.path.parent() {
                    // the directory goes when it is empty
                    let _ = std::fs::remove_dir(dir);
                }
            }
        }
        let gone = |object: &ObjectId| {
            (!loose_ids.contains(object) || gone_loose.contains(object))
                && in_packs
                    .get(object)
                    .is_none_or(|holders| holders.iter().all(|at| gone_packs.contains(at)))
        };
        report.deleted = pruned.iter().filter(|object| gone(object)).count() as u64;
        report.deleted_bytes = pruned
            .iter()
            .filter(|object| gone(object))
            .filter_map(|object| bytes_of.get(object))
            .sum();
    }
    provider.refresh(id);
    report.objects = report.scanned - usize::try_from(report.deleted).unwrap_or(0);
    let (packs_after, loose_after) = list_files(&objects_dir)?;
    report.stored_bytes =
        packs_after.iter().map(|pack| pack.bytes).sum::<u64>() + loose_after.iter().map(|file| file.bytes).sum::<u64>();
    report.saved_bytes =
        i64::try_from(bytes_before).unwrap_or(i64::MAX) - i64::try_from(report.stored_bytes).unwrap_or(i64::MAX);
    Ok(report)
}
