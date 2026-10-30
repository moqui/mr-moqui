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

//! The pack data a fetch sends: version 2, SHA-1, the planned objects. A repository on disk gives gix-pack's
//! generator its object database, so objects already stored as deltas in a pack go out as those deltas (with their
//! base in the same pack); a client that did not ask for `ofs-delta` gets whole objects. The `db` kind (and any other
//! store) writes objects as it stores them, read in batches on this thread (gix's generator reads on worker threads,
//! which have no Moqui context to read rows in): a compressed object is copied as it is, a delta goes out as a delta
//! against its base when the base is in the pack (written first when it comes later) or, in a thin pack, the client
//! has it, and is applied to its base otherwise.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use gix::objs::Kind;
use gix::odb::pack::data::entry::Header;
use gix::odb::pack::data::output::{self, Count};
use gix::ObjectId;

use crate::store::{deflate, GitStore, Packed};
use moqui_core::context::Error;
use moqui_core::resource::source::{ByteSource, READ_WINDOW};

/// Objects read per batch while a pack is written.
const WINDOW: usize = 256;

/// What the client takes beyond whole objects.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Deltas<'a> {
    /// `ofs-delta`: a delta may name its base by its offset in the pack.
    pub ofs: bool,
    /// `thin-pack`: a delta may be against an object the client has, which is not in the pack.
    pub thin: Option<&'a HashSet<ObjectId>>,
}

fn pack_error(what: &str, err: impl std::fmt::Display) -> Error {
    Error::new(format!("{what}: {err}"))
}

/// Write `objects` to `out` as a pack. Every object must be in the store: a pack's object count is written first.
pub(crate) fn write_pack(
    store: &dyn GitStore,
    objects: &[ObjectId],
    deltas: Deltas<'_>,
    threads: Option<usize>,
    out: &mut dyn Write,
) -> Result<(), Error> {
    let count = u32::try_from(objects.len()).map_err(|_| Error::new("too many objects for one pack"))?;
    if let (true, Some(handle)) = (deltas.ofs, store.pack_handle()) {
        return write_from_packs(handle, objects, count, threads, out);
    }
    write_stored(store, objects, count, deltas, out)
}

/// Through gix-pack's generator over a repository's object database, copying packed entries (deltas included).
fn write_from_packs(
    handle: gix::odb::store::Handle<std::sync::Arc<gix::odb::Store>>,
    objects: &[ObjectId],
    count: u32,
    threads: Option<usize>,
    out: &mut dyn Write,
) -> Result<(), Error> {
    let counts: Vec<Count> = objects
        .iter()
        .map(|id| Count {
            id: *id,
            entry_pack_location: output::count::PackLocation::NotLookedUp,
        })
        .collect();
    let entries = output::entry::iter_from_counts(
        counts,
        handle,
        Box::new(gix::progress::Discard),
        output::entry::iter_from_counts::Options {
            thread_limit: threads,
            mode: output::entry::iter_from_counts::Mode::PackCopyAndBaseObjects,
            allow_thin_pack: false,
            chunk_size: 64,
            version: gix::odb::pack::data::Version::V2,
            compression: gix::zlib::Compression::DEFAULT,
        },
    )
    .map_err(|err| pack_error("preparing the pack", err))?;
    let mut writer = output::bytes::FromEntriesIter::new(
        gix::parallel::InOrderIter::from(entries),
        out,
        count,
        gix::odb::pack::data::Version::V2,
        gix::hash::Kind::Sha1,
    );
    for written in writer.by_ref() {
        written.map_err(|err| pack_error("writing the pack", err))?;
    }
    let stats =
        gix::parallel::Finalize::finalize(writer.input.inner).map_err(|err| pack_error("writing the pack", err))?;
    if stats.missing_objects > 0 {
        return Err(Error::new(format!(
            "{} objects were missing while the pack was written",
            stats.missing_objects
        )));
    }
    Ok(())
}

/// A pack being written, hashed as it goes, with where each object went.
struct PackOut<'a> {
    out: gix::hash::io::Write<&'a mut dyn Write>,
    offset: u64,
    written: HashMap<ObjectId, u64>,
}

impl PackOut<'_> {
    fn entry(&mut self, id: ObjectId, header: Header, size: u64, data: &[u8]) -> Result<(), Error> {
        let io = |err: std::io::Error| pack_error("writing the pack", err);
        let length = header.write_to(size, &mut self.out).map_err(io)?;
        self.out.write_all(data).map_err(io)?;
        self.written.insert(id, self.offset);
        self.offset += (length + data.len()) as u64;
        Ok(())
    }

    /// An object compressed as it is read from `source`, a window at a time. Content that does not compress (a
    /// sample of its start, as media and archives) is written in stored blocks.
    fn stream_entry(&mut self, id: ObjectId, kind: Kind, source: &dyn ByteSource) -> Result<(), Error> {
        let io = |err: std::io::Error| pack_error("writing the pack", err);
        let size = source.len();
        let first = source.read_at(0, READ_WINDOW)?;
        let level = if deflate(&first).len() * 50 >= first.len() * 49 {
            flate2::Compression::none()
        } else {
            flate2::Compression::new(1)
        };
        let length = header_of(kind).write_to(size, &mut self.out).map_err(io)?;
        let mut counted = Counted {
            inner: &mut self.out,
            count: 0,
        };
        let mut encoder = flate2::write::ZlibEncoder::new(&mut counted, level);
        encoder.write_all(&first).map_err(io)?;
        let mut offset = first.len() as u64;
        while offset < size {
            let window = source.read_at(offset, READ_WINDOW)?;
            if window.is_empty() {
                return Err(Error::new(format!(
                    "git object {id} ended at {offset} of its {size} bytes"
                )));
            }
            encoder.write_all(&window).map_err(io)?;
            offset += window.len() as u64;
        }
        encoder.finish().map_err(io)?;
        let written = counted.count;
        self.written.insert(id, self.offset);
        self.offset += length as u64 + written;
        Ok(())
    }
}

/// A writer that counts what goes through it.
struct Counted<'a, W: Write> {
    inner: &'a mut W,
    count: u64,
}

impl<W: Write> Write for Counted<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.count += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn header_of(kind: Kind) -> Header {
    match kind {
        Kind::Commit => Header::Commit,
        Kind::Tree => Header::Tree,
        Kind::Blob => Header::Blob,
        Kind::Tag => Header::Tag,
    }
}

/// Objects as the store keeps them: a batch at a time, in order, a delta's base first when it is in the pack.
fn write_stored(
    store: &dyn GitStore,
    objects: &[ObjectId],
    count: u32,
    deltas: Deltas<'_>,
    out: &mut dyn Write,
) -> Result<(), Error> {
    let header = gix::odb::pack::data::header::encode(gix::odb::pack::data::Version::V2, count);
    let mut pack = PackOut {
        out: gix::hash::io::Write::new(out, gix::hash::Kind::Sha1),
        offset: header.len() as u64,
        written: HashMap::with_capacity(objects.len()),
    };
    pack.out
        .write_all(&header)
        .map_err(|err| pack_error("writing the pack", err))?;
    let in_pack: HashSet<ObjectId> = objects.iter().copied().collect();
    for window in objects.chunks(WINDOW) {
        let mut packed: HashMap<ObjectId, Packed> = HashMap::with_capacity(window.len());
        store.packed_many(window, &mut |id, object| {
            packed.insert(id, object);
            Ok(())
        })?;
        for id in window {
            if pack.written.contains_key(id) {
                continue;
            }
            let object = match packed.remove(id) {
                Some(object) => object,
                None => one_packed(store, id)?,
            };
            write_object(store, &mut pack, *id, object, &in_pack, deltas)?;
        }
    }
    if pack.written.len() != objects.len() {
        return Err(Error::new(format!(
            "the pack has {} objects, not the {count} its header says",
            pack.written.len()
        )));
    }
    let digest = pack
        .out
        .hash
        .try_finalize()
        .map_err(|err| pack_error("hashing the pack", err))?;
    let io = |err: std::io::Error| pack_error("writing the pack", err);
    pack.out.inner.write_all(digest.as_slice()).map_err(io)?;
    pack.out.inner.flush().map_err(io)
}

fn one_packed(store: &dyn GitStore, id: &ObjectId) -> Result<Packed, Error> {
    let mut found = None;
    store.packed_many(std::slice::from_ref(id), &mut |_, object| {
        found = Some(object);
        Ok(())
    })?;
    found.ok_or_else(|| Error::new(format!("git object {id} is missing")))
}

/// Write one object; for a delta whose base is in the pack and not written yet, the base (and its bases) first.
fn write_object(
    store: &dyn GitStore,
    pack: &mut PackOut<'_>,
    id: ObjectId,
    object: Packed,
    in_pack: &HashSet<ObjectId>,
    deltas: Deltas<'_>,
) -> Result<(), Error> {
    let mut chain = vec![(id, object)];
    while let Some((_, Packed::Delta { base, .. })) = chain.last() {
        let base = *base;
        if pack.written.contains_key(&base) || !in_pack.contains(&base) || chain.iter().any(|(id, _)| *id == base) {
            break;
        }
        chain.push((base, one_packed(store, &base)?));
    }
    while let Some((id, object)) = chain.pop() {
        match object {
            Packed::Whole { kind, data } => pack.entry(id, header_of(kind), data.len() as u64, &deflate(&data))?,
            Packed::Zlib { kind, size, data } => pack.entry(id, header_of(kind), size, &data)?,
            Packed::Stream { kind, source } => pack.stream_entry(id, kind, source.as_ref())?,
            Packed::Delta {
                kind,
                base,
                delta_size,
                data,
                ..
            } => {
                if let Some(at) = pack.written.get(&base).copied() {
                    let header = if deltas.ofs {
                        Header::OfsDelta {
                            base_distance: pack.offset - at,
                        }
                    } else {
                        Header::RefDelta { base_id: base }
                    };
                    pack.entry(id, header, delta_size, &data)?;
                } else if deltas.thin.is_some_and(|has| has.contains(&base)) && !in_pack.contains(&base) {
                    pack.entry(id, Header::RefDelta { base_id: base }, delta_size, &data)?;
                } else {
                    let (_, content) = store
                        .find(&id)?
                        .ok_or_else(|| Error::new(format!("git object {id} is missing")))?;
                    pack.entry(id, header_of(kind), content.len() as u64, &deflate(&content))?;
                }
            }
        }
    }
    Ok(())
}

/// Whether every object of `objects` that is not a commit or tree is in the store (commits and trees were read while
/// planning). The ids that are missing.
pub(crate) fn missing_objects(store: &dyn GitStore, objects: &[ObjectId]) -> Result<Vec<ObjectId>, Error> {
    let mut missing = Vec::new();
    for chunk in objects.chunks(500) {
        let present = store.contains_many(chunk)?;
        missing.extend(chunk.iter().filter(|id| !present.contains(*id)));
    }
    Ok(missing)
}

/// Read a pack written here back, for tests: every object's id and kind, in pack order.
#[cfg(test)]
pub(crate) fn read_back(pack: &[u8]) -> Vec<(ObjectId, Kind)> {
    let dir = std::env::temp_dir().join(format!(
        "moqui-pack-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos())
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let interrupt = std::sync::atomic::AtomicBool::new(false);
    let outcome = gix::odb::pack::Bundle::write_to_directory(
        &mut std::io::BufReader::new(pack),
        Some(&dir),
        &mut gix::progress::Discard,
        &interrupt,
        None::<gix::objs::find::Never>,
        gix::hash::Kind::Sha1,
        gix::odb::pack::bundle::write::Options::default(),
    )
    .unwrap();
    let bundle = outcome.to_bundle().unwrap().unwrap();
    let mut buf = Vec::new();
    let mut found = Vec::new();
    for entry in bundle.index.iter() {
        let (data, _) = bundle
            .find(
                &entry.oid,
                &mut buf,
                &mut gix::zlib::Inflate::default(),
                &mut gix::odb::pack::cache::Never,
            )
            .unwrap()
            .unwrap();
        found.push((entry.oid, data.kind));
    }
    let _ = std::fs::remove_dir_all(&dir);
    found
}
