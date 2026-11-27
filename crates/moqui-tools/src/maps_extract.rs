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

//! `org.moqui.maps.MapSourceServices.estimate#PackageExtract` and `build#PackageExtract` (plans/archive/Maps-Offline.md §2.2;
//! docs/tools/Maps.md "Offline packages"): a region of a registered PMTiles archive, a box and a zoom range, cut into
//! an archive of its own for an offline map package (MapPackages.js, the MapPackageBuild ServiceJob).
//!
//! For each zoom the tiles of the box (across the antimeridian when its west is east of its east), sorted by tile ID (a
//! box is not one run of the Hilbert order), are found through the archive's directories as the tile endpoint finds
//! one ([`crate::maps_tiles`]: the header and the directories by range, kept by the archive's tag), then read by range
//! in batches by offset, so neighbouring tiles are one read, and handed as stored to the streaming writer
//! ([`moqui_lib::pmtiles::ArchiveWriter`]): the tile data goes to a part file as it comes, and a content the source
//! shares between tiles (the sea, empty land) is stored once and read once. The source's metadata JSON is copied whole.
//! The estimate reads only the directories: the tiles found, their contents and their bytes, so a package is sized,
//! and refused past its limit, before anything is cut.
//!
//! The archive is pinned: its size and tag (a content address) are checked when the cut starts and again when it ends,
//! and a file on disk by its modification time too, so an archive replaced while it was cut ends the run as failed,
//! never a mixed extract. The run's progress is written to its package's row in a transaction of its own between
//! tiles (asked at most once a second, written when its percent changes or about a minute has passed), and the row's
//! cancel read then. Both read whatever location they are given, so they are no group's and not remote: MapPackages.js
//! calls them after its check of the package's owner.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::pmtiles::{locate_tile, tile_id, ArchiveOptions, ArchiveWriter, Compression, Entry, Header, MAX_ZOOM};
use sha2::{Digest, Sha256};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::resource::source::ByteSource;
use moqui_core::transaction::TransactionAttribute;

use crate::group_db::{number, text, Out};
use crate::maps_tiles::{archive_directory, archive_header, content_key, decompress, read_exact, TILE_MAX};

const PACKAGE: &str = "moqui.maps.MapPackage";
/// The most tiles one extract may cover (the box's tiles over its zooms, counted before anything is read). One zoom is
/// found and written at a time: its tile IDs (8 bytes each) are held while it is, each tile found (a [`Located`], 24
/// bytes) is handed on as it is found, and the writer keeps a directory entry (24 bytes) for each tile that is not part
/// of a run and a bounded map of the contents it may share.
pub(crate) const MAX_TILES: u64 = 4_000_000;
/// The most bytes the source's metadata may take, as stored and decompressed.
const METADATA_MAX: u64 = 16 * 1024 * 1024;
/// A batch of tiles read at once: at most this many bytes, and tiles at most this far apart.
const BATCH_MAX: u64 = 4 * 1024 * 1024;
const BATCH_GAP: u64 = 64 * 1024;
/// How often at most a run reads its cancel (and writes its progress when its percent changed).
const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
/// How often a run writes its row with no change of percent: its `progressDate`, by which a stalled run is told
/// (MapPackages.js runEnded), so a slow zoom is no silence and a fast one no write a second.
const WORD_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// Where an extract is written: the package builds' folder of the runtime.
const TARGET_ROOT: &str = "runtime://tmp/map-packages/";
/// The latitude Web Mercator tiles end at.
const MERCATOR_MAX_LAT: f64 = 85.051_128_779_806_59;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, estimate_package_extract)?;
    rust_service!(map, build_package_extract)?;
    Ok(())
}

// ===== the box and its tiles =====

/// A box of WGS84 degrees; a west east of its east crosses the antimeridian.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Bbox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Bbox {
    /// The box, or what is wrong with it: longitudes within ±180, latitudes within ±90, its south below its north and
    /// its west not its east. A west of 180 is -180 and an east of -180 is 180 (the same meridian, on the side the box
    /// is), so neither takes a column beyond the antimeridian.
    pub(crate) fn new(west: f64, south: f64, east: f64, north: f64) -> Result<Self, String> {
        let lon = |value: f64| value.is_finite() && (-180.0..=180.0).contains(&value);
        let lat = |value: f64| value.is_finite() && (-90.0..=90.0).contains(&value);
        if !(lon(west) && lon(east) && lat(south) && lat(north)) {
            return Err("the box is not longitudes and latitudes".to_string());
        }
        if south >= north {
            return Err("the box's south is not below its north".to_string());
        }
        // a width of nothing; west of 180 and east of -180 is the same meridian too
        if (west - east).abs() < f64::EPSILON || (west == 180.0 && east == -180.0) {
            return Err("the box has no width".to_string());
        }
        let west = if west == 180.0 { -180.0 } else { west };
        let east = if east == -180.0 { 180.0 } else { east };
        Ok(Self {
            west,
            south,
            east,
            north,
        })
    }

    pub(crate) fn crosses_antimeridian(&self) -> bool {
        self.west > self.east
    }
}

fn tile_x(lon: f64, n: u32) -> u32 {
    let x = ((lon + 180.0) / 360.0 * f64::from(n)).floor();
    // within the zoom's columns, so the cast keeps the value
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let x = x.clamp(0.0, f64::from(n - 1)) as u32;
    x
}

fn tile_y(lat: f64, n: u32) -> u32 {
    let rad = lat.clamp(-MERCATOR_MAX_LAT, MERCATOR_MAX_LAT).to_radians();
    let y = ((1.0 - (rad.tan() + 1.0 / rad.cos()).ln() / std::f64::consts::PI) / 2.0 * f64::from(n)).floor();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let y = y.clamp(0.0, f64::from(n - 1)) as u32;
    y
}

/// The tiles of `bbox` at zoom `z`: its column ranges (one, or two across the antimeridian, merged where they meet) and
/// its row range, each inclusive. A box's east or south edge on a tile's edge does not take in the tile beyond it.
pub(crate) fn bbox_tiles(bbox: &Bbox, z: u8) -> (Vec<(u32, u32)>, (u32, u32)) {
    let n = 1u32 << z;
    // an edge exactly on a tile boundary ends at the tile before it
    let nudge = 1e-9;
    let west = tile_x(bbox.west, n);
    let east = tile_x(bbox.east - nudge, n).max(if bbox.crosses_antimeridian() { 0 } else { west });
    let rows = (
        tile_y(bbox.north, n),
        tile_y(bbox.south + nudge, n).max(tile_y(bbox.north, n)),
    );
    let mut columns = if bbox.crosses_antimeridian() {
        vec![(0, east), (west, n - 1)]
    } else {
        vec![(west, east)]
    };
    // across the antimeridian at a low zoom the two runs meet or overlap: one run
    if columns.len() == 2 && columns[0].1 + 1 >= columns[1].0 {
        columns = vec![(0, n - 1)];
    }
    (columns, rows)
}

/// How many tiles `bbox` covers at zoom `z`.
pub(crate) fn bbox_tile_count(bbox: &Bbox, z: u8) -> u64 {
    let (columns, (top, bottom)) = bbox_tiles(bbox, z);
    let width: u64 = columns.iter().map(|(from, to)| u64::from(to - from) + 1).sum();
    width * (u64::from(bottom - top) + 1)
}

/// The IDs of the tiles of `bbox` at zoom `z`, in ascending order.
///
/// # Errors
///
/// A zoom past [`MAX_ZOOM`].
pub(crate) fn bbox_tile_ids(bbox: &Bbox, z: u8) -> Result<Vec<u64>, LibError> {
    let (columns, (top, bottom)) = bbox_tiles(bbox, z);
    let mut ids = Vec::with_capacity(usize::try_from(bbox_tile_count(bbox, z)).unwrap_or(0));
    for (from, to) in columns {
        for x in from..=to {
            for y in top..=bottom {
                ids.push(tile_id(z, x, y)?);
            }
        }
    }
    ids.sort_unstable();
    Ok(ids)
}

// ===== the cut =====

/// What an extract asks: the box, the zooms, the tiles it may cover and the bytes it may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cut {
    pub bbox: Bbox,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub max_tiles: u64,
    /// The bytes the archive cut may take; 0 for no bound
    pub max_bytes: u64,
}

/// A cut refused or stopped: its code and why, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub code: &'static str,
    pub reason: String,
}

fn refused(code: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal {
        code,
        reason: reason.into(),
    }
}

/// A tile of the source found in the box: its ID and where its content is (absolute offset, length).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Located {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
}

/// What an estimate found: the tiles of the box the source has, their distinct contents and those contents' bytes, the
/// archive's bytes near enough (the tiles', the source's metadata and a few bytes a directory entry), and the zooms
/// cut (those of the request the source has).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Estimate {
    pub tiles: u64,
    pub contents: u64,
    pub tile_bytes: u64,
    pub bytes: u64,
    pub min_zoom: u8,
    pub max_zoom: u8,
}

/// The archive's tag and size the cut is pinned to: the source is still that content.
fn pinned(source: &dyn ByteSource, tag: &str, size: u64) -> bool {
    !tag.is_empty() && source.len() == size && source.tag().is_none_or(|own| own == tag)
}

/// The zooms of `cut` the archive has, or a refusal.
fn zooms_of(header: &Header, cut: &Cut) -> Result<(u8, u8), Refusal> {
    let low = cut.min_zoom.max(header.min_zoom);
    let high = cut.max_zoom.min(header.max_zoom).min(MAX_ZOOM);
    if low > high {
        return Err(refused(
            "NO_ZOOMS",
            format!(
                "the map archive has zooms {} to {}, none of {} to {}",
                header.min_zoom, header.max_zoom, cut.min_zoom, cut.max_zoom
            ),
        ));
    }
    Ok((low, high))
}

/// The tiles the box covers at the zooms, or a refusal past `cut.max_tiles` (counted before anything is read).
fn covered(cut: &Cut, (low, high): (u8, u8)) -> Result<u64, Refusal> {
    let covered: u64 = (low..=high).map(|z| bbox_tile_count(&cut.bbox, z)).sum();
    if covered > cut.max_tiles {
        return Err(refused(
            "TOO_MANY_TILES",
            format!(
                "the region covers {covered} tiles at zooms {low} to {high}, more than the {} a package may: choose a smaller region or fewer zooms",
                cut.max_tiles
            ),
        ));
    }
    Ok(covered)
}

/// Finds the source's tiles of a zoom of the box through its directories, in ascending tile ID order; the directories
/// met while walking the sorted IDs (a leaf serves a run of neighbours) kept here in front of the shared cache.
struct Finder<'a> {
    source: &'a dyn ByteSource,
    header: &'a Header,
    content: &'a str,
    near: HashMap<u64, Arc<Vec<Entry>>>,
}

impl<'a> Finder<'a> {
    fn new(source: &'a dyn ByteSource, header: &'a Header, content: &'a str) -> Self {
        Self {
            source,
            header,
            content,
            near: HashMap::new(),
        }
    }

    /// Each tile of the box at zoom `z` the source has, handed to `each`, which answers whether to go on: whether the
    /// zoom was walked whole.
    fn zoom(
        &mut self,
        bbox: &Bbox,
        z: u8,
        each: &mut dyn FnMut(u64, Option<Located>) -> Result<bool, LibError>,
    ) -> Result<bool, LibError> {
        let internal = self.header.internal_compression;
        for id in bbox_tile_ids(bbox, z)? {
            let (source, content, near) = (self.source, self.content, &mut self.near);
            let at = locate_tile(self.header, id, |offset, length| {
                if let Some(entries) = near.get(&offset) {
                    return Ok(Arc::clone(entries));
                }
                let entries = archive_directory(source, content, internal, offset, length)?;
                if near.len() >= 64 {
                    near.clear();
                }
                near.insert(offset, Arc::clone(&entries));
                Ok(entries)
            })?;
            let found = at.map(|(offset, length)| Located {
                tile_id: id,
                offset,
                length,
            });
            if !each(id, found)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// The bytes near enough of an archive of `tiles` tiles with `tile_bytes` of contents: the header, the source's
/// metadata as stored, and some 4 bytes an entry once gzipped.
fn archive_bytes(header: &Header, tiles: u64, tile_bytes: u64) -> u64 {
    tile_bytes + 127 + header.metadata_length + 4 * tiles
}

fn too_large(bytes: u64, max_bytes: u64) -> Option<Refusal> {
    (max_bytes > 0 && bytes > max_bytes).then(|| {
        refused(
            "TOO_LARGE",
            format!(
                "the region's map is about {bytes} bytes, more than the {max_bytes} left to the package (maps@package-max-bytes): choose a smaller region or fewer zooms"
            ),
        )
    })
}

/// What the estimate of `cut` from the archive `source`, pinned as `tag` of `size` bytes, finds; reads only the header
/// and directories, and holds no more than one zoom's tile IDs. A content is counted once where the archive stores
/// it after every content before it (an archive written in tile ID order, as a clustered one is) and as shared where it
/// is at or before the place of one already counted (the sea, empty land): near enough for an archive in another order.
pub(crate) fn estimate(
    source: &dyn ByteSource,
    tag: &str,
    size: u64,
    cut: &Cut,
) -> Result<Result<Estimate, Refusal>, LibError> {
    if !pinned(source, tag, size) {
        return Ok(Err(changed()));
    }
    let content = content_key(tag, size);
    let header = archive_header(source, &content, size)?;
    let zooms = match zooms_of(&header, cut).and_then(|zooms| covered(cut, zooms).map(|_| zooms)) {
        Ok(zooms) => zooms,
        Err(refusal) => return Ok(Err(refusal)),
    };
    // A clustered archive stores its contents in tile ID order, a shared one at its first tile's place: a content past
    // every one counted is new, one at or before is shared. Another archive's order says nothing of that: every tile whose
    // content is not the one before it counts, which errs high. Either way the cut's own byte check is the hard limit.
    let clustered = header.clustered;
    let (mut tiles, mut contents, mut tile_bytes, mut next_new) = (0u64, 0u64, 0u64, 0u64);
    let mut previous: Option<u64> = None;
    let mut finder = Finder::new(source, &header, &content);
    for z in zooms.0..=zooms.1 {
        finder.zoom(&cut.bbox, z, &mut |_, found| {
            if let Some(tile) = found {
                tiles += 1;
                let new = if clustered {
                    tile.offset >= next_new
                } else {
                    previous != Some(tile.offset)
                };
                if new {
                    contents += 1;
                    tile_bytes += u64::from(tile.length);
                    next_new = next_new.max(tile.offset + u64::from(tile.length));
                }
                previous = Some(tile.offset);
            }
            Ok(true)
        })?;
    }
    let found = Estimate {
        tiles,
        contents,
        tile_bytes,
        bytes: archive_bytes(&header, tiles, tile_bytes),
        min_zoom: zooms.0,
        max_zoom: zooms.1,
    };
    if let Some(refusal) = too_large(found.bytes, cut.max_bytes) {
        return Ok(Err(refusal));
    }
    Ok(Ok(found))
}

fn changed() -> Refusal {
    refused(
        "SOURCE_CHANGED",
        "the map archive was replaced while the package was cut: update the package to cut it again",
    )
}

/// What an extract wrote: its tiles and contents, its size and SHA-256 (hex), and the zooms cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Extracted {
    pub tiles: u64,
    pub contents: u64,
    pub size: u64,
    pub sha256: String,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// The ranges read for the tiles (each batch one, and a shared content read again alone)
    pub reads: u64,
    /// What the source's metadata says of its data: its attribution and license (empty when it says none)
    pub attribution: String,
    pub license: String,
}

/// Files that go when dropped unless kept: the part files of a cut that did not end.
struct Parts(Vec<PathBuf>);

impl Drop for Parts {
    fn drop(&mut self) {
        for path in &self.0 {
            if let Err(err) = std::fs::remove_file(path) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), "a map package's part file was not removed: {err}");
                }
            }
        }
    }
}

fn io(err: &std::io::Error) -> LibError {
    LibError::new(format!("writing the map extract: {err}"))
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // writing into a Vec does not fail
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// The most tiles handed the writer at once when none of them needs a read (a run of shared contents).
const PENDING_MAX: usize = 4096;

/// The tiles found, in tile ID order, handed to the writer a batch at a time: a batch is one range of the source's tile
/// data, its tiles at most [`BATCH_GAP`] apart and [`BATCH_MAX`] in all; a tile whose content the writer has already is
/// not read again (nor one already in the batch).
struct Cutter<W: Write> {
    writer: ArchiveWriter<W>,
    pending: Vec<Located>,
    batch: Option<Batch>,
    /// The contents of the batch, by their offset
    in_batch: HashSet<u64>,
    reads: u64,
}

/// A batch of tiles read with one range: its bytes from `start` to `end`.
struct Batch {
    start: u64,
    end: u64,
}

impl<W: Write> Cutter<W> {
    /// Tile `tile`: queued, the batch read and handed first when the tile is not in it and cannot join it.
    fn add(&mut self, source: &dyn ByteSource, tile: Located) -> Result<(), LibError> {
        if u64::from(tile.length) > TILE_MAX {
            return Err(LibError::new(format!(
                "a tile of the map archive is {} bytes, more than the {TILE_MAX} read here",
                tile.length
            )));
        }
        let shared = self.in_batch.contains(&tile.offset) || self.writer.has_key(tile.offset);
        if !shared {
            let end = tile.offset + u64::from(tile.length);
            let fits = self.batch.as_ref().is_some_and(|range| {
                tile.offset >= range.end && tile.offset - range.end <= BATCH_GAP && end - range.start <= BATCH_MAX
            });
            if !fits && self.batch.is_some() {
                self.flush(source)?;
            }
            match self.batch.as_mut() {
                Some(range) => range.end = end,
                None => {
                    self.batch = Some(Batch {
                        start: tile.offset,
                        end,
                    })
                }
            }
            self.in_batch.insert(tile.offset);
        }
        self.pending.push(tile);
        if self.batch.is_none() && self.pending.len() >= PENDING_MAX {
            self.flush(source)?;
        }
        Ok(())
    }

    /// Read the batch (one range) and hand the writer each pending tile in order. A shared content the writer no longer
    /// knows (its keys are bounded) is read again by itself.
    fn flush(&mut self, source: &dyn ByteSource) -> Result<(), LibError> {
        let (bytes, start) = match self.batch.take() {
            Some(range) => {
                self.reads += 1;
                (
                    read_exact(source, range.start, range.end - range.start, "tiles")?,
                    range.start,
                )
            }
            None => (Vec::new(), 0),
        };
        for tile in std::mem::take(&mut self.pending) {
            if self.in_batch.contains(&tile.offset) {
                let from = usize::try_from(tile.offset - start).map_err(|_| LibError::new("a tile's offset"))?;
                let to = from + tile.length as usize;
                self.writer.add_keyed(tile.tile_id, tile.offset, &bytes[from..to])?;
            } else if self.writer.has_key(tile.offset) {
                self.writer.repeat_keyed(tile.tile_id, tile.offset)?;
            } else {
                self.reads += 1;
                let alone = read_exact(source, tile.offset, u64::from(tile.length), "tile")?;
                self.writer.add_keyed(tile.tile_id, tile.offset, &alone)?;
            }
        }
        self.in_batch.clear();
        Ok(())
    }
}

/// Cut `cut` of the archive `source` (pinned as `tag` of `size` bytes) into a new archive at `target` (a file path):
/// its tiles as stored, the source's metadata JSON whole, directories gzipped; one zoom found and written at a time.
/// `still_pinned` is asked when the cut ends whether the location is still that content (reopened, its size, tag and
/// time); `progress(done, total)` (the tiles of the box walked, of all it covers) is called before the first read,
/// after each tile walked and after each zoom, and stops the cut when it answers false. A cut past `cut.max_bytes` stops as TOO_LARGE. A cut that
/// does not end leaves no file.
pub(crate) fn extract(
    source: &dyn ByteSource,
    tag: &str,
    size: u64,
    cut: &Cut,
    target: &Path,
    still_pinned: &mut dyn FnMut() -> bool,
    progress: &mut dyn FnMut(u64, u64) -> bool,
) -> Result<Result<Extracted, Refusal>, LibError> {
    if !pinned(source, tag, size) {
        return Ok(Err(changed()));
    }
    let content = content_key(tag, size);
    let header = archive_header(source, &content, size)?;
    let (zooms, total) = match zooms_of(&header, cut).and_then(|zooms| covered(cut, zooms).map(|total| (zooms, total)))
    {
        Ok(found) => found,
        Err(refusal) => return Ok(Err(refusal)),
    };
    // the source's metadata, its JSON copied whole
    if header.metadata_length > METADATA_MAX {
        return Err(LibError::new(format!(
            "the map archive's metadata is {} bytes, more than the {METADATA_MAX} read here",
            header.metadata_length
        )));
    }
    let metadata = decompress(
        read_exact(source, header.metadata_offset, header.metadata_length, "metadata")?,
        header.internal_compression,
        METADATA_MAX,
        "metadata",
    )?;
    let notice = serde_json::from_slice::<serde_json::Value>(&metadata).unwrap_or_default();
    let said = |key: &str| {
        notice
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io(&err))?;
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| LibError::new("the map extract's target has no file name"))?;
    let data_path = target.with_file_name(format!("{name}.data.part"));
    let final_part = target.with_file_name(format!("{name}.part"));
    let parts = Parts(vec![data_path.clone(), final_part.clone()]);
    let data = std::fs::File::create(&data_path).map_err(|err| io(&err))?;
    let mut cutter = Cutter {
        writer: ArchiveWriter::new(std::io::BufWriter::new(data)),
        pending: Vec::new(),
        batch: None,
        in_batch: HashSet::new(),
        reads: 0,
    };

    if !progress(0, total) {
        return Ok(Err(cancelled()));
    }
    let mut done = 0u64;
    let mut stop: Option<Refusal> = None;
    let mut finder = Finder::new(source, &header, &content);
    for z in zooms.0..=zooms.1 {
        let walked = finder.zoom(&cut.bbox, z, &mut |_, found| {
            done += 1;
            if let Some(tile) = found {
                let reads = cutter.reads;
                cutter.add(source, tile)?;
                // a batch was handed to the writer: its size so far
                if cutter.reads != reads {
                    let writer = &cutter.writer;
                    if let Some(refusal) = too_large(
                        archive_bytes(&header, writer.tile_count(), writer.data_len()),
                        cut.max_bytes,
                    ) {
                        stop = Some(refusal);
                        return Ok(false);
                    }
                }
            }
            // every tile walked, found or not: a walk of empty tiles with no batch still reports (the closure throttles)
            if !progress(done, total) {
                stop = Some(cancelled());
                return Ok(false);
            }
            Ok(true)
        })?;
        if !walked {
            break;
        }
        if !progress(done, total) {
            stop = Some(cancelled());
            break;
        }
    }
    if let Some(refusal) = stop {
        return Ok(Err(refusal));
    }
    cutter.flush(source)?;
    let Cutter { writer, reads, .. } = cutter;
    if writer.tile_count() == 0 {
        return Ok(Err(refused("NO_TILES", "the map archive has no tiles in the region")));
    }
    if let Some(refusal) = too_large(
        archive_bytes(&header, writer.tile_count(), writer.data_len()),
        cut.max_bytes,
    ) {
        return Ok(Err(refusal));
    }
    if !progress(total, total) {
        return Ok(Err(cancelled()));
    }
    let tiles = writer.tile_count();
    let options = ArchiveOptions {
        tile_type: header.tile_type,
        tile_compression: header.tile_compression,
        internal_compression: Compression::Gzip,
        min_zoom: zooms.0,
        max_zoom: zooms.1,
        bounds: if cut.bbox.crosses_antimeridian() {
            [-180.0, cut.bbox.south, 180.0, cut.bbox.north]
        } else {
            [cut.bbox.west, cut.bbox.south, cut.bbox.east, cut.bbox.north]
        },
        center_zoom: header.center_zoom.clamp(zooms.0, zooms.1),
    };
    let finished = writer.finish(&metadata, &options, &gzip)?;
    let contents = finished.header.tile_contents;
    let data = finished.data.into_inner().map_err(|err| io(err.error()))?;
    data.sync_all().map_err(|err| io(&err))?;
    drop(data);
    // the source still the content pinned, now the cut is made: else nothing is kept
    if !still_pinned() {
        return Ok(Err(changed()));
    }
    let mut hasher = Sha256::new();
    let mut out = std::io::BufWriter::new(std::fs::File::create(&final_part).map_err(|err| io(&err))?);
    out.write_all(&finished.prefix).map_err(|err| io(&err))?;
    hasher.update(&finished.prefix);
    let mut size_out = finished.prefix.len() as u64;
    let mut data = std::fs::File::open(&data_path).map_err(|err| io(&err))?;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = data.read(&mut buffer).map_err(|err| io(&err))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read]).map_err(|err| io(&err))?;
        hasher.update(&buffer[..read]);
        size_out += read as u64;
    }
    let file = out.into_inner().map_err(|err| io(err.error()))?;
    file.sync_all().map_err(|err| io(&err))?;
    drop(file);
    std::fs::rename(&final_part, target).map_err(|err| io(&err))?;
    drop(parts);
    Ok(Ok(Extracted {
        tiles,
        contents,
        size: size_out,
        sha256: hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect(),
        min_zoom: zooms.0,
        max_zoom: zooms.1,
        reads,
        attribution: said("attribution"),
        license: said("license"),
    }))
}

fn cancelled() -> Refusal {
    refused("CANCELLED", "the package was cancelled")
}

// ===== the services =====

/// A number parameter as a float; `None` when absent or not a number.
fn float(parameters: &BTreeMap<String, Value>, key: &str) -> Option<f64> {
    match parameters.get(key)? {
        Value::F64(value) => Some(*value),
        Value::I64(value) => Some(*value as f64),
        Value::I32(value) => Some(f64::from(*value)),
        Value::Null => None,
        other => to_plain_string(other).trim().parse().ok(),
    }
}

fn zoom(parameters: &BTreeMap<String, Value>, key: &str, default: u8) -> u8 {
    number(parameters, key)
        .map_or(i64::from(default), |value| value.clamp(0, i64::from(MAX_ZOOM)))
        .try_into()
        .unwrap_or(default)
}

/// The cut a request asks: its box (west, south, east, north), zooms (minZoom, maxZoom), maxTiles (at most
/// [`MAX_TILES`]) and maxBytes (none when 0 or absent).
fn cut_of(parameters: &BTreeMap<String, Value>) -> Result<Cut, Refusal> {
    let side = |key: &str| float(parameters, key).unwrap_or(f64::NAN);
    let bbox = Bbox::new(side("west"), side("south"), side("east"), side("north"))
        .map_err(|reason| refused("BBOX_INVALID", reason))?;
    let min_zoom = zoom(parameters, "minZoom", 0);
    let max_zoom = zoom(parameters, "maxZoom", 14);
    if min_zoom > max_zoom {
        return Err(refused(
            "ZOOMS_INVALID",
            format!("the lowest zoom {min_zoom} is above the highest {max_zoom}"),
        ));
    }
    let max_tiles = number(parameters, "maxTiles")
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0)
        .map_or(MAX_TILES, |value| value.min(MAX_TILES));
    let max_bytes = number(parameters, "maxBytes")
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0);
    Ok(Cut {
        bbox,
        min_zoom,
        max_zoom,
        max_tiles,
        max_bytes,
    })
}

/// The archive at `location` opened as the framework (the caller checked the user), still the content pinned: its size,
/// its tag where the storage has one, and with `modified` (a file on disk, which has no tag) its modification time to
/// the second.
fn open_pinned(
    location: &str,
    tag: &str,
    size: u64,
    modified: Option<i64>,
) -> Result<Result<Arc<dyn ByteSource>, Refusal>, LibError> {
    let opened = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().open(location)
    })
    .ok_or_else(|| LibError::new("no execution context to read the map archive"))??;
    let Some(opened) = opened else {
        return Ok(Err(refused("SOURCE_GONE", "the map archive is gone")));
    };
    let same_time = modified.is_none_or(|millis| {
        opened
            .stat
            .last_modified
            .is_some_and(|at| at.div_euclid(1000) == millis.div_euclid(1000))
    });
    if opened.stat.size != size || !same_time || !pinned(opened.source.as_ref(), tag, size) {
        return Ok(Err(changed()));
    }
    Ok(Ok(opened.source))
}

fn refusal_out(mut out: Out, refusal: &Refusal) -> Out {
    out.insert("valid".to_string(), Value::Bool(false));
    out.insert("reasonCode".to_string(), Value::string(refusal.code));
    out.insert("reason".to_string(), Value::string(refusal.reason.clone()));
    if refusal.code == "CANCELLED" {
        out.insert("cancelled".to_string(), Value::Bool(true));
    }
    out
}

fn count(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// The pinning of a request: location, tag, size and, for a file on disk, modifiedMillis.
fn source_of(parameters: &BTreeMap<String, Value>) -> Result<(String, String, u64, Option<i64>), Refusal> {
    let location = text(parameters, "location");
    let tag = text(parameters, "tag");
    let size = number(parameters, "size").and_then(|size| u64::try_from(size).ok());
    match size {
        Some(size) if !location.is_empty() && !tag.is_empty() => {
            Ok((location, tag, size, number(parameters, "modifiedMillis")))
        }
        _ => Err(refused("SOURCE_MISSING", "no map archive location, tag and size")),
    }
}

/// `estimate#PackageExtract`: what cutting the box and zooms of a request from the archive at `location` (pinned as
/// `tag` of `size` bytes, a file on disk by `modifiedMillis` too) would make, from its directories alone. Out: `valid`,
/// `tileCount`, `contentCount`, `tileBytes`, `bytes` (the archive's size near enough), `minZoom` and `maxZoom` (the
/// zooms of the request the archive has); refused (`reasonCode`, `reason`): BBOX_INVALID, ZOOMS_INVALID,
/// SOURCE_MISSING, SOURCE_GONE, SOURCE_CHANGED, NO_ZOOMS, TOO_MANY_TILES, TOO_LARGE (past `maxBytes`). An archive that
/// cannot be read is NOT_READ, logged.
pub(crate) fn estimate_package_extract(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = Out::new();
    let asked = cut_of(parameters).and_then(|cut| Ok((cut, source_of(parameters)?)));
    let (cut, (location, tag, size, modified)) = match asked {
        Ok(found) => found,
        Err(refusal) => return Ok(refusal_out(out, &refusal)),
    };
    let found = match open_pinned(&location, &tag, size, modified) {
        Ok(Ok(source)) => estimate(source.as_ref(), &tag, size, &cut),
        Ok(Err(refusal)) => Ok(Err(refusal)),
        Err(err) => Err(err),
    };
    let estimated = match found {
        Ok(Ok(estimated)) => estimated,
        Ok(Err(refusal)) => return Ok(refusal_out(out, &refusal)),
        Err(err) => {
            tracing::warn!(location = %location, error = %err, "A map archive could not be estimated for a package");
            return Ok(refusal_out(out, &not_read()));
        }
    };
    let mut out = out;
    out.insert("valid".to_string(), Value::Bool(true));
    out.insert("tileCount".to_string(), count(estimated.tiles));
    out.insert("contentCount".to_string(), count(estimated.contents));
    out.insert("tileBytes".to_string(), count(estimated.tile_bytes));
    out.insert("bytes".to_string(), count(estimated.bytes));
    out.insert("minZoom".to_string(), Value::I64(i64::from(estimated.min_zoom)));
    out.insert("maxZoom".to_string(), Value::I64(i64::from(estimated.max_zoom)));
    Ok(out)
}

/// The file an extract is written to: a `.pmtiles` location under [`TARGET_ROOT`], on disk.
fn target_path(location: &str) -> Result<PathBuf, Refusal> {
    let invalid = || {
        refused(
            "TARGET_INVALID",
            "the extract is written under runtime://tmp/map-packages/ only",
        )
    };
    let Some(rest) = location.strip_prefix(TARGET_ROOT) else {
        return Err(invalid());
    };
    if !rest.ends_with(".pmtiles")
        || rest.contains('#')
        || rest.contains('?')
        || rest.contains('\\')
        || rest
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(invalid());
    }
    ExecutionContext::with_current(|ec| ec.factory().resource().get(location))
        .and_then(Result::ok)
        .and_then(|found| found.file_path().map(Path::to_path_buf))
        .ok_or_else(invalid)
}

/// A cut that could not be read: the detail is logged where it failed, and the user is told no path of the server.
fn not_read() -> Refusal {
    refused("NOT_READ", "the map archive could not be read")
}

/// Read package `package_id`'s cancel in a transaction of its own and, when `write`, put `percent` and the time
/// (`progressDate`, the build's last word) on it; answer whether to go on: its `cancelRequested` is not Y, and with
/// `build_key` the row is still that build's (a run the package no longer holds, one ended as stale and built again,
/// stops and writes nothing). A row that cannot be read or written is logged and the run goes on.
fn report(facade: &EntityFacade, package_id: &str, build_key: &str, percent: i64, write: bool) -> bool {
    let mut cancelled = false;
    let written = facade.transactions_for(PACKAGE).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, None, || {
            let Some(mut row) = facade
                .find(PACKAGE)?
                .eq("packageId", Value::string(package_id))
                .for_update(write)
                .one()?
            else {
                cancelled = true;
                return Ok(());
            };
            let field = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
            if !build_key.is_empty() && field("buildKey") != build_key {
                cancelled = true;
                return Ok(());
            }
            cancelled = field("cancelRequested") == "Y";
            if !write {
                return Ok(());
            }
            row.set("progress", Value::I64(percent));
            // the build's last word, by which a stalled run is told (MapPackages.js runEnded)
            row.set("progressDate", Value::Timestamp(jiff::Timestamp::now()));
            row.update(facade)
        })
    });
    if let Err(err) = written {
        tracing::error!(error = %err, package = %package_id, "Could not write a map package's progress");
    }
    !cancelled
}

/// `build#PackageExtract`: the box and zooms of a request cut from the archive at `location` (pinned as
/// `estimate#PackageExtract` has it) into a new archive at `target` (a `.pmtiles` location under
/// runtime://tmp/map-packages/), its tiles as stored and the source's metadata whole; with `packageId` the run's
/// progress from `progressFrom` to `progressTo` (percent) is written to that package's row and its cancel read, between
/// batches (with `buildKey`, only while the row is still that build's: a run it no longer holds stops as CANCELLED). Out: `valid`, `size`, `sha256` (hex), `tileCount`, `contentCount`, `minZoom`, `maxZoom`, `reads` (the ranges
/// read for the tiles); refused as the estimate is, and TARGET_INVALID, NO_TILES, CANCELLED (`cancelled` true) and
/// SOURCE_CHANGED for an archive replaced while it was cut (checked again when the cut ends). A refused or failed cut
/// leaves no file.
pub(crate) fn build_package_extract(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = Out::new();
    let asked = cut_of(parameters).and_then(|cut| {
        let source = source_of(parameters)?;
        let target = target_path(&text(parameters, "target"))?;
        Ok((cut, source, target))
    });
    let (cut, (location, tag, size, modified), target) = match asked {
        Ok(asked) => asked,
        Err(refusal) => return Ok(refusal_out(out, &refusal)),
    };
    let source = match open_pinned(&location, &tag, size, modified) {
        Ok(Ok(source)) => source,
        Ok(Err(refusal)) => return Ok(refusal_out(out, &refusal)),
        Err(err) => {
            tracing::warn!(location = %location, error = %err, "A map archive could not be opened for a package");
            return Ok(refusal_out(out, &not_read()));
        }
    };
    let package_id = text(parameters, "packageId");
    let build_key = text(parameters, "buildKey");
    let from = number(parameters, "progressFrom").unwrap_or(0).clamp(0, 100);
    let to = number(parameters, "progressTo").unwrap_or(100).clamp(from, 100);
    let mut last_report: Option<std::time::Instant> = None;
    // the last percent written and when: the row is written again only when the percent changes or WORD_EVERY passed
    let mut last_written: Option<(i64, std::time::Instant)> = None;
    let mut progress = |done: u64, total: u64| {
        if package_id.is_empty() {
            return true;
        }
        if done < total && last_report.is_some_and(|last| last.elapsed() < REPORT_EVERY) {
            return true;
        }
        let now = std::time::Instant::now();
        last_report = Some(now);
        let share = if total == 0 { 1.0 } else { done as f64 / total as f64 };
        // a percent from 0 to 100
        #[allow(clippy::cast_possible_truncation)]
        let percent = (from + ((to - from) as f64 * share).round() as i64).clamp(from, to);
        let write = last_written.is_none_or(|(written, at)| written != percent || at.elapsed() >= WORD_EVERY);
        if write {
            last_written = Some((percent, now));
        }
        report(facade, &package_id, &build_key, percent, write)
    };
    let mut still_pinned = || matches!(open_pinned(&location, &tag, size, modified), Ok(Ok(_)));
    let made = extract(
        source.as_ref(),
        &tag,
        size,
        &cut,
        &target,
        &mut still_pinned,
        &mut progress,
    );
    let extracted = match made {
        Ok(Ok(extracted)) => extracted,
        Ok(Err(refusal)) => {
            if refusal.code == "SOURCE_CHANGED" {
                tracing::warn!(location = %location, "A map archive changed while a package was cut from it");
            }
            return Ok(refusal_out(out, &refusal));
        }
        Err(err) => {
            tracing::error!(location = %location, error = %err, "A map package's extract could not be cut");
            return Ok(refusal_out(out, &not_read()));
        }
    };
    let mut out = out;
    out.insert("valid".to_string(), Value::Bool(true));
    out.insert("size".to_string(), count(extracted.size));
    out.insert("sha256".to_string(), Value::string(extracted.sha256));
    out.insert("tileCount".to_string(), count(extracted.tiles));
    out.insert("contentCount".to_string(), count(extracted.contents));
    out.insert("minZoom".to_string(), Value::I64(i64::from(extracted.min_zoom)));
    out.insert("maxZoom".to_string(), Value::I64(i64::from(extracted.max_zoom)));
    out.insert("reads".to_string(), count(extracted.reads));
    out.insert("attribution".to_string(), Value::string(extracted.attribution));
    out.insert("license".to_string(), Value::string(extracted.license));
    Ok(out)
}

#[cfg(test)]
#[path = "maps_extract_tests.rs"]
mod tests;
