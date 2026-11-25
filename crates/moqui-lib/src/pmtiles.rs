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

//! The header and metadata of a PMTiles version 3 archive (plans/Maps.md §4.1, §5; the format:
//! <https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md>), read when a map archive is registered: the 127
//! bytes at its start and its JSON metadata, checked so that an archive a map cannot draw is refused by name before a
//! browser asks for it. No ExecutionContext: the service that reads the bytes through the resource facade is
//! `moqui_tools::maps_archive`.

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

#[cfg(test)]
#[path = "pmtiles_tests.rs"]
mod tests;
