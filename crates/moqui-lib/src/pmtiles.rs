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

//! The header and metadata of a PMTiles version 3 archive (plans/archive/Maps.md §4.1, §5; the format:
//! <https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md>), read when a map archive is registered: the 127
//! bytes at its start and its JSON metadata, checked so that an archive a map cannot draw is refused by name before a
//! browser asks for it. No ExecutionContext: the service that reads the bytes through the resource facade is
//! `moqui_tools::maps_archive`.
//!
//! Also the archive's directories and tile IDs, for the Z/X/Y tile endpoint (`moqui_tools::maps_tiles`, plans/archive/Maps.md §5
//! "a later Rust endpoint returning individual Z/X/Y tiles"): a tile's ID in the format's Hilbert order ([`tile_id`]), a
//! directory decoded ([`decode_directory`]) and a tile found through the root and its leaves, run lengths included
//! ([`locate_tile`]); and an archive written whole from its tiles ([`write_archive`]), for an overlay generalized on the
//! server (`moqui_tools::maps_simplify`). Compression is the caller's: the reader is handed decompressed directories and
//! the writer compresses with the function it is given, so this crate takes no compression dependency. Written here
//! rather than on the `pmtiles` crate (MIT or Apache-2.0), whose reader is async over its own backends (Tokio, an HTTP
//! client or memory maps, `bytes`, `async-compression`) and does not fit the resource facade's synchronous `read_at`; the
//! format's directory is a few varints, and a reader of our own adds no dependency.

use crate::error::Error;

/// The length of a version 3 header.
pub const HEADER_LEN: usize = 127;
/// The root directory must end within the first 16 KiB, so a reader gets the header and the root in one request.
pub const ROOT_END_MAX: u64 = 16_384;
/// The highest zoom level the browser's reader (pmtiles, `zxyToTileId`) addresses: its tile ids are JavaScript numbers.
pub const MAX_ZOOM: u8 = 26;

/// The compression of the archive's directories and metadata, or of its tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Unknown,
    None,
    Gzip,
    Brotli,
    Zstd,
    Other(u8),
}

impl Compression {
    /// The header's byte of this compression.
    #[must_use]
    pub fn byte(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::None => 1,
            Self::Gzip => 2,
            Self::Brotli => 3,
            Self::Zstd => 4,
            Self::Other(byte) => byte,
        }
    }

    fn of(byte: u8) -> Self {
        match byte {
            0 => Self::Unknown,
            1 => Self::None,
            2 => Self::Gzip,
            3 => Self::Brotli,
            4 => Self::Zstd,
            other => Self::Other(other),
        }
    }

    /// The name a report shows: unknown, none, gzip, brotli, zstd, or the byte for another.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Unknown => "unknown".to_string(),
            Self::None => "none".to_string(),
            Self::Gzip => "gzip".to_string(),
            Self::Brotli => "brotli".to_string(),
            Self::Zstd => "zstd".to_string(),
            Self::Other(byte) => format!("type {byte}"),
        }
    }
}

/// The kind of tile an archive holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileType {
    Unknown,
    Mvt,
    Png,
    Jpeg,
    Webp,
    Avif,
    Other(u8),
}

impl TileType {
    /// The header's byte of this type.
    #[must_use]
    pub fn byte(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Mvt => 1,
            Self::Png => 2,
            Self::Jpeg => 3,
            Self::Webp => 4,
            Self::Avif => 5,
            Self::Other(byte) => byte,
        }
    }

    /// The content type a tile of this type is sent as.
    #[must_use]
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mvt => "application/vnd.mapbox-vector-tile",
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Unknown | Self::Other(_) => "application/octet-stream",
        }
    }

    fn of(byte: u8) -> Self {
        match byte {
            0 => Self::Unknown,
            1 => Self::Mvt,
            2 => Self::Png,
            3 => Self::Jpeg,
            4 => Self::Webp,
            5 => Self::Avif,
            other => Self::Other(other),
        }
    }

    /// The name a report shows: mvt, png, jpeg, webp, avif, unknown, or the byte for another.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Unknown => "unknown".to_string(),
            Self::Mvt => "mvt".to_string(),
            Self::Png => "png".to_string(),
            Self::Jpeg => "jpeg".to_string(),
            Self::Webp => "webp".to_string(),
            Self::Avif => "avif".to_string(),
            Self::Other(byte) => format!("type {byte}"),
        }
    }

    /// Vector tiles (Mapbox Vector Tiles), drawn with a style.
    #[must_use]
    pub fn is_vector(self) -> bool {
        self == Self::Mvt
    }

    /// Image tiles a browser decodes.
    #[must_use]
    pub fn is_raster(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::Webp | Self::Avif)
    }
}

/// A version 3 header, as stored (positions in degrees).
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub version: u8,
    pub root_offset: u64,
    pub root_length: u64,
    pub metadata_offset: u64,
    pub metadata_length: u64,
    pub leaf_offset: u64,
    pub leaf_length: u64,
    pub tile_data_offset: u64,
    pub tile_data_length: u64,
    pub addressed_tiles: u64,
    pub tile_entries: u64,
    pub tile_contents: u64,
    pub clustered: bool,
    pub internal_compression: Compression,
    pub tile_compression: Compression,
    pub tile_type: TileType,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
    pub center_zoom: u8,
    pub center_lon: f64,
    pub center_lat: f64,
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

fn degrees_at(bytes: &[u8], at: usize) -> f64 {
    let mut word = [0u8; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    f64::from(i32::from_le_bytes(word)) / 10_000_000.0
}

impl Header {
    /// The header in the first [`HEADER_LEN`] bytes of `bytes`.
    ///
    /// # Errors
    ///
    /// When there are fewer bytes than a header, they do not start with the magic `PMTiles`, or the version is not 3
    /// (an archive of version 1 or 2, which start `PM`, is named as such: `pmtiles convert` makes a version 3 one).
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() >= 2 && &bytes[..2] == b"PM" && bytes.get(2..7) != Some(b"Tiles".as_slice()) {
            let version = bytes.get(2).copied().unwrap_or_default();
            return Err(Error::new(format!(
                "a PMTiles archive of version {version}, which a map does not read: convert it to version 3 (pmtiles convert)"
            )));
        }
        if bytes.len() < HEADER_LEN {
            return Err(Error::new(format!(
                "not a PMTiles archive: {} bytes, fewer than a header's {HEADER_LEN}",
                bytes.len()
            )));
        }
        if &bytes[..7] != b"PMTiles" {
            return Err(Error::new("not a PMTiles archive: it does not start with PMTiles"));
        }
        let version = bytes[7];
        if version != 3 {
            return Err(Error::new(format!(
                "a PMTiles archive of version {version}, which a map does not read: it must be version 3"
            )));
        }
        Ok(Self {
            version,
            root_offset: u64_at(bytes, 8),
            root_length: u64_at(bytes, 16),
            metadata_offset: u64_at(bytes, 24),
            metadata_length: u64_at(bytes, 32),
            leaf_offset: u64_at(bytes, 40),
            leaf_length: u64_at(bytes, 48),
            tile_data_offset: u64_at(bytes, 56),
            tile_data_length: u64_at(bytes, 64),
            addressed_tiles: u64_at(bytes, 72),
            tile_entries: u64_at(bytes, 80),
            tile_contents: u64_at(bytes, 88),
            clustered: bytes[96] == 1,
            internal_compression: Compression::of(bytes[97]),
            tile_compression: Compression::of(bytes[98]),
            tile_type: TileType::of(bytes[99]),
            min_zoom: bytes[100],
            max_zoom: bytes[101],
            min_lon: degrees_at(bytes, 102),
            min_lat: degrees_at(bytes, 106),
            max_lon: degrees_at(bytes, 110),
            max_lat: degrees_at(bytes, 114),
            center_zoom: bytes[118],
            center_lon: degrees_at(bytes, 119),
            center_lat: degrees_at(bytes, 123),
        })
    }

    /// What is wrong with the header for an archive of `size` bytes, in words, each a reason a map cannot draw it; empty
    /// when nothing is. The zoom range, the bounds, the sections within the file (the root directory within its first
    /// 16 KiB), a tile type and an internal compression a browser reads.
    #[must_use]
    pub fn problems(&self, size: u64) -> Vec<String> {
        let mut out = Vec::new();
        if self.min_zoom > self.max_zoom {
            out.push(format!(
                "its minimum zoom {} is above its maximum zoom {}",
                self.min_zoom, self.max_zoom
            ));
        }
        if self.max_zoom > MAX_ZOOM {
            out.push(format!("its maximum zoom {} is above {MAX_ZOOM}", self.max_zoom));
        }
        let lon_ok = |lon: f64| (-180.0..=180.0).contains(&lon);
        let lat_ok = |lat: f64| (-90.0..=90.0).contains(&lat);
        if !(lon_ok(self.min_lon) && lon_ok(self.max_lon) && lat_ok(self.min_lat) && lat_ok(self.max_lat)) {
            out.push(format!(
                "its bounds {} {} {} {} are not longitudes and latitudes",
                self.min_lon, self.min_lat, self.max_lon, self.max_lat
            ));
        } else if self.min_lon > self.max_lon || self.min_lat > self.max_lat {
            out.push(format!(
                "its bounds {} {} {} {} have a minimum above a maximum",
                self.min_lon, self.min_lat, self.max_lon, self.max_lat
            ));
        }
        let sections = [
            ("root directory", self.root_offset, self.root_length),
            ("metadata", self.metadata_offset, self.metadata_length),
            ("leaf directories", self.leaf_offset, self.leaf_length),
            ("tile data", self.tile_data_offset, self.tile_data_length),
        ];
        for (name, offset, length) in sections {
            if length > 0 && offset < HEADER_LEN as u64 {
                out.push(format!(
                    "its {name} ({length} bytes at {offset}) starts inside the header"
                ));
            } else if length > 0 && offset.checked_add(length).is_none_or(|end| end > size) {
                out.push(format!(
                    "its {name} ({length} bytes at {offset}) runs past the end of the file ({size} bytes): the file is cut short"
                ));
            }
        }
        if self.root_length == 0 {
            out.push("it has no root directory".to_string());
        } else if self.root_offset.saturating_add(self.root_length) > ROOT_END_MAX {
            out.push(format!(
                "its root directory ends past the first {ROOT_END_MAX} bytes, where a reader looks for it"
            ));
        }
        if self.tile_data_length == 0 || self.addressed_tiles == 0 {
            out.push("it holds no tiles".to_string());
        }
        if !(self.tile_type.is_vector() || self.tile_type.is_raster()) {
            out.push(format!(
                "its tiles are of {} type, neither vector (mvt) nor an image a browser draws",
                self.tile_type.name()
            ));
        }
        // the browser's reader decompresses its directories with gzip (DecompressionStream) or reads them as they are
        if !matches!(self.internal_compression, Compression::None | Compression::Gzip) {
            out.push(format!(
                "its directories are compressed with {}, which the map's reader does not read (gzip or none)",
                self.internal_compression.name()
            ));
        }
        // vector tiles are decompressed by the reader (gzip) or read as they are; an image is never compressed again
        let tiles_readable = if self.tile_type.is_vector() {
            matches!(self.tile_compression, Compression::None | Compression::Gzip)
        } else {
            matches!(self.tile_compression, Compression::None | Compression::Unknown)
        };
        if (self.tile_type.is_vector() || self.tile_type.is_raster()) && !tiles_readable {
            out.push(format!(
                "its {} tiles are compressed with {}, which the map's reader does not read ({})",
                self.tile_type.name(),
                self.tile_compression.name(),
                if self.tile_type.is_vector() {
                    "gzip or none"
                } else {
                    "none"
                }
            ));
        }
        out
    }
}

/// What the archive's JSON metadata says that a layer shows or checks: its name, description, attribution, version and
/// the ids of its vector layers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub name: String,
    pub description: String,
    pub attribution: String,
    pub version: String,
    pub kind: String,
    pub vector_layers: Vec<String>,
}

impl Metadata {
    /// The metadata in its JSON text (decompressed).
    ///
    /// # Errors
    ///
    /// When the text is not a JSON object.
    pub fn parse(json: &[u8]) -> Result<Self, Error> {
        let value: serde_json::Value =
            serde_json::from_slice(json).map_err(|err| Error::new(format!("its metadata is not JSON: {err}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| Error::new("its metadata is not a JSON object"))?;
        let text = |key: &str| match object.get(key) {
            Some(serde_json::Value::String(value)) => value.trim().to_string(),
            Some(serde_json::Value::Number(value)) => value.to_string(),
            _ => String::new(),
        };
        let vector_layers = object
            .get("vector_layers")
            .and_then(serde_json::Value::as_array)
            .map(|layers| {
                layers
                    .iter()
                    .filter_map(|layer| layer.get("id").and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            name: text("name"),
            description: text("description"),
            attribution: text("attribution"),
            version: text("version"),
            kind: text("type"),
            vector_layers,
        })
    }
}

impl Header {
    /// The 127 bytes of this header, as [`Header::parse`] reads them (positions to 1e-7 degrees).
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[..7].copy_from_slice(b"PMTiles");
        bytes[7] = 3;
        let words = [
            self.root_offset,
            self.root_length,
            self.metadata_offset,
            self.metadata_length,
            self.leaf_offset,
            self.leaf_length,
            self.tile_data_offset,
            self.tile_data_length,
            self.addressed_tiles,
            self.tile_entries,
            self.tile_contents,
        ];
        for (index, word) in words.iter().enumerate() {
            let at = 8 + index * 8;
            bytes[at..at + 8].copy_from_slice(&word.to_le_bytes());
        }
        bytes[96] = u8::from(self.clustered);
        bytes[97] = self.internal_compression.byte();
        bytes[98] = self.tile_compression.byte();
        bytes[99] = self.tile_type.byte();
        bytes[100] = self.min_zoom;
        bytes[101] = self.max_zoom;
        let degrees = |value: f64| {
            // a position checked to be within ±180 degrees fits an i32 of 1e-7 degrees
            #[allow(clippy::cast_possible_truncation)]
            let e7 = (value * 10_000_000.0)
                .round()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
            e7.to_le_bytes()
        };
        bytes[102..106].copy_from_slice(&degrees(self.min_lon));
        bytes[106..110].copy_from_slice(&degrees(self.min_lat));
        bytes[110..114].copy_from_slice(&degrees(self.max_lon));
        bytes[114..118].copy_from_slice(&degrees(self.max_lat));
        bytes[118] = self.center_zoom;
        bytes[119..123].copy_from_slice(&degrees(self.center_lon));
        bytes[123..127].copy_from_slice(&degrees(self.center_lat));
        bytes
    }
}

// ===== tile IDs and directories =====

/// The most levels of directories a tile is looked up through: the root and at most two levels of leaves (pmtiles.js's
/// limit too); an archive nesting deeper is refused, not followed.
pub const MAX_DEPTH: usize = 3;
/// The most entries one directory read here may have (a planet archive's leaves hold some tens of thousands): a
/// directory claiming more is refused before anything is allocated for it.
pub const DIRECTORY_ENTRIES_MAX: u64 = 1 << 21;

/// A directory entry: a run of `run_length` tiles from `tile_id` with one content at `offset` (from the start of the
/// tile data) of `length` bytes, or with `run_length` 0 a leaf directory at `offset` from the start of the leaf
/// directories.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    pub offset: u64,
    pub length: u32,
    pub run_length: u32,
}

impl Entry {
    /// An entry that points to a leaf directory.
    #[must_use]
    pub fn is_leaf(&self) -> bool {
        self.run_length == 0
    }
}

/// The ID of tile `z`/`x`/`y` (y from the top, as XYZ tiles): the tiles of the zooms below it, then its position along
/// the Hilbert curve of its zoom (pmtiles' `zxyToTileId`).
///
/// # Errors
///
/// A zoom above [`MAX_ZOOM`] or an `x` or `y` outside the zoom's 2^z tiles.
pub fn tile_id(z: u8, x: u32, y: u32) -> Result<u64, Error> {
    if z > MAX_ZOOM {
        return Err(Error::new(format!("zoom {z} is above {MAX_ZOOM}")));
    }
    let n = 1u64 << z;
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    if x >= n || y >= n {
        return Err(Error::new(format!("tile {z}/{x}/{y} is outside zoom {z}")));
    }
    // the tiles of every lower zoom: (4^z - 1) / 3
    let mut id = ((1u64 << (2 * u32::from(z))) - 1) / 3;
    let mut s = n >> 1;
    while s > 0 {
        let rx = u64::from(x & s != 0);
        let ry = u64::from(y & s != 0);
        id += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = n - 1 - x;
                y = n - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s >>= 1;
    }
    Ok(id)
}

/// The zoom, x and y of tile ID `id` ([`tile_id`]'s inverse).
///
/// # Errors
///
/// An ID past the last tile of zoom [`MAX_ZOOM`].
pub fn tile_zxy(id: u64) -> Result<(u8, u32, u32), Error> {
    let mut first = 0u64;
    for z in 0..=MAX_ZOOM {
        let count = 1u64 << (2 * u32::from(z));
        if id < first + count {
            let n = 1u64 << z;
            let mut d = id - first;
            let (mut x, mut y) = (0u64, 0u64);
            let mut s = 1u64;
            while s < n {
                let rx = 1 & (d / 2);
                let ry = 1 & (d ^ rx);
                if ry == 0 {
                    if rx == 1 {
                        x = s - 1 - x;
                        y = s - 1 - y;
                    }
                    std::mem::swap(&mut x, &mut y);
                }
                x += s * rx;
                y += s * ry;
                d /= 4;
                s *= 2;
            }
            let x = u32::try_from(x).map_err(|_| Error::new("tile x"))?;
            let y = u32::try_from(y).map_err(|_| Error::new("tile y"))?;
            return Ok((z, x, y));
        }
        first += count;
    }
    Err(Error::new(format!("tile ID {id} is past zoom {MAX_ZOOM}")))
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Result<u64, Error> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let Some(&byte) = bytes.get(*at) else {
            return Err(Error::new("the directory is cut short"));
        };
        *at += 1;
        if shift > 63 || (shift == 63 && byte > 1) {
            return Err(Error::new("the directory has a number too large"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).unwrap_or_default() | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).unwrap_or_default());
}

/// The entries of a directory from its bytes (decompressed): the count, then the tile ID deltas, the run lengths, the
/// lengths and the offsets (0 for one right after the entry before, else the offset plus 1).
///
/// # Errors
///
/// A directory cut short, with more than [`DIRECTORY_ENTRIES_MAX`] entries, with tile IDs out of order, or with a
/// length or run length past 32 bits.
pub fn decode_directory(bytes: &[u8]) -> Result<Vec<Entry>, Error> {
    let mut at = 0usize;
    let count = read_varint(bytes, &mut at)?;
    // each entry takes at least four bytes, so a count past what the bytes could hold is a broken directory
    if count > DIRECTORY_ENTRIES_MAX || count.saturating_mul(4) > bytes.len() as u64 {
        return Err(Error::new(format!(
            "the directory claims {count} entries in {} bytes",
            bytes.len()
        )));
    }
    let count = usize::try_from(count).map_err(|_| Error::new("directory count"))?;
    let mut entries = vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0
        };
        count
    ];
    let mut last = 0u64;
    for (index, entry) in entries.iter_mut().enumerate() {
        let delta = read_varint(bytes, &mut at)?;
        if index > 0 && delta == 0 {
            return Err(Error::new("the directory's tile IDs are not in order"));
        }
        last = last
            .checked_add(delta)
            .ok_or_else(|| Error::new("the directory's tile IDs overflow"))?;
        entry.tile_id = last;
    }
    let small = |value: u64, what: &str| {
        u32::try_from(value).map_err(|_| Error::new(format!("the directory has a {what} past 32 bits")))
    };
    for entry in &mut entries {
        entry.run_length = small(read_varint(bytes, &mut at)?, "run length")?;
    }
    for entry in &mut entries {
        entry.length = small(read_varint(bytes, &mut at)?, "length")?;
    }
    for index in 0..entries.len() {
        let value = read_varint(bytes, &mut at)?;
        entries[index].offset = if value == 0 && index > 0 {
            let before = entries[index - 1];
            before.offset.saturating_add(u64::from(before.length))
        } else {
            value.saturating_sub(1)
        };
    }
    Ok(entries)
}

/// A directory's bytes (before compression) of `entries`, in tile ID order ([`decode_directory`]'s inverse).
#[must_use]
pub fn encode_directory(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::with_capacity(entries.len() * 6 + 4);
    write_varint(&mut out, entries.len() as u64);
    let mut last = 0u64;
    for entry in entries {
        write_varint(&mut out, entry.tile_id - last);
        last = entry.tile_id;
    }
    for entry in entries {
        write_varint(&mut out, u64::from(entry.run_length));
    }
    for entry in entries {
        write_varint(&mut out, u64::from(entry.length));
    }
    for (index, entry) in entries.iter().enumerate() {
        let follows = index > 0 && {
            let before = entries[index - 1];
            entry.offset == before.offset + u64::from(before.length)
        };
        write_varint(&mut out, if follows { 0 } else { entry.offset + 1 });
    }
    out
}

/// The entry of `entries` that holds tile `tile_id`: the one with that ID, else the last before it when it is a leaf
/// (the tile may be in that leaf) or a run that reaches it; `None` when no tile of that ID is here.
#[must_use]
pub fn find_entry(entries: &[Entry], tile_id: u64) -> Option<Entry> {
    let after = entries.partition_point(|entry| entry.tile_id <= tile_id);
    let entry = *entries.get(after.checked_sub(1)?)?;
    if entry.tile_id == tile_id || entry.is_leaf() || tile_id - entry.tile_id < u64::from(entry.run_length) {
        Some(entry)
    } else {
        None
    }
}

/// Where tile `tile_id` is in the archive of `header`: its absolute offset and length, or `None` when the archive has
/// no such tile. `directory(offset, length)` gives the entries of the directory at that absolute offset (read,
/// decompressed and decoded by the caller, which may cache them by the archive's revision and offset): the root first,
/// then each leaf the lookup goes through, at most [`MAX_DEPTH`] in all.
///
/// # Errors
///
/// What `directory` fails with, a leaf outside the leaf directories, a tile outside the tile data, or nesting deeper
/// than [`MAX_DEPTH`].
pub fn locate_tile<D>(header: &Header, tile_id: u64, mut directory: D) -> Result<Option<(u64, u32)>, Error>
where
    D: FnMut(u64, u64) -> Result<std::sync::Arc<Vec<Entry>>, Error>,
{
    let mut offset = header.root_offset;
    let mut length = header.root_length;
    for _ in 0..MAX_DEPTH {
        let entries = directory(offset, length)?;
        let Some(entry) = find_entry(&entries, tile_id) else {
            return Ok(None);
        };
        if entry.is_leaf() {
            if entry.offset.saturating_add(u64::from(entry.length)) > header.leaf_length {
                return Err(Error::new("a leaf directory is outside the archive's leaf directories"));
            }
            offset = header
                .leaf_offset
                .checked_add(entry.offset)
                .ok_or_else(|| Error::new("a leaf directory's offset overflows"))?;
            length = u64::from(entry.length);
            continue;
        }
        if entry.offset.saturating_add(u64::from(entry.length)) > header.tile_data_length {
            return Err(Error::new("a tile is outside the archive's tile data"));
        }
        let at = header
            .tile_data_offset
            .checked_add(entry.offset)
            .ok_or_else(|| Error::new("a tile's offset overflows"))?;
        return Ok(Some((at, entry.length)));
    }
    Err(Error::new(format!(
        "the archive's directories nest deeper than {MAX_DEPTH} levels"
    )))
}

// ===== writing an archive =====

/// What an archive written by [`write_archive`] says of its tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArchiveOptions {
    pub tile_type: TileType,
    /// The compression the tiles handed in already have.
    pub tile_compression: Compression,
    /// The compression `compress` gives the directories and the metadata.
    pub internal_compression: Compression,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// west, south, east, north
    pub bounds: [f64; 4],
    pub center_zoom: u8,
}

/// The directory layout of `entries`: the root's bytes, and the leaf directories' (empty when the root holds every
/// entry), each compressed; the root ends within the first [`ROOT_END_MAX`] bytes of the file.
fn directories(entries: &[Entry], compress: &dyn Fn(&[u8]) -> Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    let root_max = usize::try_from(ROOT_END_MAX).unwrap_or(usize::MAX) - HEADER_LEN;
    if entries.len() <= 16_384 {
        let root = compress(&encode_directory(entries));
        if root.len() <= root_max {
            return (root, Vec::new());
        }
    }
    let mut leaf_size = 4096usize.max(entries.len() / 3500);
    loop {
        let mut leaves = Vec::new();
        let mut root_entries = Vec::new();
        for chunk in entries.chunks(leaf_size) {
            let leaf = compress(&encode_directory(chunk));
            root_entries.push(Entry {
                tile_id: chunk[0].tile_id,
                offset: leaves.len() as u64,
                length: u32::try_from(leaf.len()).unwrap_or(u32::MAX),
                run_length: 0,
            });
            leaves.extend_from_slice(&leaf);
        }
        let root = compress(&encode_directory(&root_entries));
        if root.len() <= root_max || leaf_size >= entries.len() {
            return (root, leaves);
        }
        leaf_size += leaf_size / 5 + 1;
    }
}

/// A whole PMTiles version 3 archive of `tiles` (tile ID and content, in ascending tile ID order, each content already
/// compressed as `options.tile_compression` says) with `metadata` (JSON): header, root directory, metadata, leaf
/// directories, tile data. Consecutive tiles of the same content are one run (clustered). `compress` compresses the
/// directories and the metadata as `options.internal_compression` says.
///
/// # Errors
///
/// Tiles not in ascending tile ID order, none at all, or a tile larger than 4 GiB.
pub fn write_archive<I>(
    tiles: I,
    metadata: &[u8],
    options: &ArchiveOptions,
    compress: &dyn Fn(&[u8]) -> Vec<u8>,
) -> Result<Vec<u8>, Error>
where
    I: IntoIterator<Item = (u64, Vec<u8>)>,
{
    let mut entries: Vec<Entry> = Vec::new();
    let mut data: Vec<u8> = Vec::new();
    // where the last content written starts in data: it runs to data's end
    let mut last_start = 0usize;
    let mut addressed = 0u64;
    for (tile_id, content) in tiles {
        let length = u32::try_from(content.len()).map_err(|_| Error::new("a tile larger than 4 GiB"))?;
        if let Some(last) = entries.last_mut() {
            let next = last.tile_id + u64::from(last.run_length);
            if tile_id < next {
                return Err(Error::new("the archive's tiles are not in tile ID order"));
            }
            // the same content as the tile right before it: one run
            if tile_id == next && data[last_start..] == content[..] {
                last.run_length += 1;
                addressed += 1;
                continue;
            }
        }
        addressed += 1;
        last_start = data.len();
        entries.push(Entry {
            tile_id,
            offset: last_start as u64,
            length,
            run_length: 1,
        });
        data.extend_from_slice(&content);
    }
    if entries.is_empty() {
        return Err(Error::new("an archive needs at least one tile"));
    }
    let (root, leaves) = directories(&entries, compress);
    let metadata = compress(metadata);
    let header_len = HEADER_LEN as u64;
    let root_offset = header_len;
    let metadata_offset = root_offset + root.len() as u64;
    let leaf_offset = metadata_offset + metadata.len() as u64;
    let tile_data_offset = leaf_offset + leaves.len() as u64;
    let [west, south, east, north] = options.bounds;
    let header = Header {
        version: 3,
        root_offset,
        root_length: root.len() as u64,
        metadata_offset,
        metadata_length: metadata.len() as u64,
        leaf_offset,
        leaf_length: leaves.len() as u64,
        tile_data_offset,
        tile_data_length: data.len() as u64,
        addressed_tiles: addressed,
        tile_entries: entries.len() as u64,
        tile_contents: entries.len() as u64,
        clustered: true,
        internal_compression: options.internal_compression,
        tile_compression: options.tile_compression,
        tile_type: options.tile_type,
        min_zoom: options.min_zoom,
        max_zoom: options.max_zoom,
        min_lon: west,
        min_lat: south,
        max_lon: east,
        max_lat: north,
        center_zoom: options.center_zoom,
        center_lon: (west + east) / 2.0,
        center_lat: (south + north) / 2.0,
    };
    let mut out = Vec::with_capacity(usize::try_from(tile_data_offset).unwrap_or(0) + data.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(&root);
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&leaves);
    out.extend_from_slice(&data);
    Ok(out)
}

#[cfg(test)]
#[path = "pmtiles_tests.rs"]
mod tests;
