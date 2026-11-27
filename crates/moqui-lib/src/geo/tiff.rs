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

//! A Cloud Optimized GeoTIFF's header (plans/archive/Maps.md §4.1, P4c), read by range before it is registered as a map layer,
//! as a PMTiles archive's is: the TIFF or BigTIFF header, its image file directories and the few tags a map needs, never
//! its tiles. A browser draws the file with geotiff.js through the map-source transition; this decides whether it may.
//!
//! - Bounded: at most [`MAX_IFDS`] directories (a directory seen twice is a loop, refused), [`MAX_ENTRIES`] entries in one,
//!   tag values read only for the small tags (dimensions, samples, compression, the GeoTIFF keys and the georeference,
//!   GDAL's no-data and metadata text, each with its own size limit) and at most [`MAX_READ`] bytes read in all past the
//!   first [`HEAD`]. Every offset and count is checked against the file's size before it is read, with no overflow. A
//!   tile's offsets and byte counts are not read: only how many there are, which must be the tiles the image has.
//! - Refused (a problem, in words): not a TIFF, strips rather than tiles (not cloud optimized), a tile size that is not
//!   a multiple of 16 or past 4096, a compression geotiff.js does not decode, a sample of another size than 8, 16, 32 or 64
//!   bits, bands of different sizes, no georeference, a rotated or sheared one, no GeoTIFF keys, a CRS that is not an EPSG
//!   code (user-defined), and an image larger than 2048 pixels with no overview (a map would read its full resolution at
//!   every zoom).
//! - Noted (still drawn): directories that are not at the file's start (more ranges for a map) and a raster of
//!   PixelIsPoint.

use std::collections::HashSet;

/// What is read at once from the start: a COG's directories and their small values are there (GDAL writes them so).
pub const HEAD: usize = 64 * 1024;
/// The most bytes read past the head, for directories or values a file put elsewhere.
pub const MAX_READ: u64 = 1024 * 1024;
pub const MAX_IFDS: usize = 64;
pub const MAX_ENTRIES: u64 = 1024;
/// The largest image side read (pixels), and the largest band count.
pub const MAX_SIDE: u64 = 1 << 24;
pub const MAX_BANDS: u64 = 64;

/// What a reader of the file's bytes answers: `len` bytes at `offset` (fewer only at the end of the file).
pub type ReadAt<'a> = dyn FnMut(u64, usize) -> Result<Vec<u8>, String> + 'a;

/// A reduced-resolution image of the file (an overview).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overview {
    pub width: u64,
    pub height: u64,
}

/// The header of a GeoTIFF as a map draws it.
#[derive(Clone, Debug, Default)]
pub struct CogInfo {
    pub big_tiff: bool,
    pub width: u64,
    pub height: u64,
    pub bands: u16,
    pub bits_per_sample: u16,
    /// 1 unsigned integer, 2 signed integer, 3 floating point
    pub sample_format: u16,
    pub compression: u16,
    pub photometric: u16,
    pub planar: u16,
    pub tile_width: u64,
    pub tile_height: u64,
    pub overviews: Vec<Overview>,
    pub masks: usize,
    /// The CRS's EPSG code (ProjectedCSTypeGeoKey, else GeographicTypeGeoKey)
    pub epsg: Option<u32>,
    pub geographic: bool,
    /// west, south, east, north in the CRS's units
    pub bounds: Option<[f64; 4]>,
    /// the size of a pixel in the CRS's units, x and y
    pub resolution: Option<[f64; 2]>,
    pub pixel_is_point: bool,
    /// GDAL_NODATA's text, as given
    pub nodata: Option<String>,
    /// GDAL_METADATA's STATISTICS_MINIMUM and STATISTICS_MAXIMUM of each band that has them (band number from 1)
    pub statistics: Vec<(u16, f64, f64)>,
    /// Why a map cannot draw it: empty when it can
    pub problems: Vec<String>,
    /// What it holds that a map draws anyway, in words
    pub notes: Vec<String>,
}

impl CogInfo {
    /// The compression's name, as geotiff.js knows them.
    #[must_use]
    pub fn compression_name(&self) -> &'static str {
        match self.compression {
            1 => "none",
            5 => "LZW",
            7 => "JPEG",
            8 | 32946 => "Deflate",
            32773 => "PackBits",
            34887 => "LERC",
            50000 => "Zstandard",
            50001 => "WebP",
            _ => "other",
        }
    }

    /// The sample type in words (`uint8`, `int16`, `float32`).
    #[must_use]
    pub fn sample_type(&self) -> String {
        let kind = match self.sample_format {
            2 => "int",
            3 => "float",
            _ => "uint",
        };
        format!("{kind}{}", self.bits_per_sample)
    }
}

const SUPPORTED_COMPRESSION: [u16; 9] = [1, 5, 7, 8, 32946, 32773, 34887, 50000, 50001];

/// The bytes of the file: the head kept, the rest read as asked within [`MAX_READ`].
struct Bytes<'a, 'b> {
    head: Vec<u8>,
    size: u64,
    read_at: &'a mut ReadAt<'b>,
    read: u64,
    little: bool,
}

impl Bytes<'_, '_> {
    fn get(&mut self, offset: u64, len: u64) -> Result<Vec<u8>, String> {
        let end = offset.checked_add(len).filter(|end| *end <= self.size).ok_or_else(|| {
            format!(
                "a value at {offset} of {len} bytes is past the file's end ({})",
                self.size
            )
        })?;
        if end <= self.head.len() as u64 {
            return Ok(self.head[offset as usize..end as usize].to_vec());
        }
        self.read = self.read.saturating_add(len);
        if self.read > MAX_READ {
            return Err(format!(
                "its directories need more than {MAX_READ} bytes read past its first {HEAD}"
            ));
        }
        let length = usize::try_from(len).map_err(|_| "a value too large to read".to_string())?;
        let bytes = (self.read_at)(offset, length)?;
        if bytes.len() != length {
            return Err(format!(
                "a read at {offset} answered {} bytes, not {length}",
                bytes.len()
            ));
        }
        Ok(bytes)
    }

    fn u16(&self, bytes: &[u8]) -> u16 {
        let pair = [bytes[0], bytes[1]];
        if self.little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    }

    fn u32(&self, bytes: &[u8]) -> u32 {
        let four = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if self.little {
            u32::from_le_bytes(four)
        } else {
            u32::from_be_bytes(four)
        }
    }

    fn u64(&self, bytes: &[u8]) -> u64 {
        let mut eight = [0u8; 8];
        eight.copy_from_slice(&bytes[..8]);
        if self.little {
            u64::from_le_bytes(eight)
        } else {
            u64::from_be_bytes(eight)
        }
    }

    fn f64(&self, bytes: &[u8]) -> f64 {
        f64::from_bits(self.u64(bytes))
    }
}

/// One directory entry: its tag, type, count and where its value is (inline bytes, or an offset).
#[derive(Clone, Debug)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u64,
    /// the entry's value field as stored (4 or 8 bytes)
    field: Vec<u8>,
}

/// The bytes of one value of a TIFF type
fn type_size(kind: u16) -> Option<u64> {
    Some(match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => return None,
    })
}

/// A directory's entries by tag, as read
struct Ifd {
    offset: u64,
    entries: Vec<Entry>,
}

impl Ifd {
    fn entry(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.tag == tag)
    }
}

/// The tags read
mod tag {
    pub const NEW_SUBFILE_TYPE: u16 = 254;
    pub const IMAGE_WIDTH: u16 = 256;
    pub const IMAGE_LENGTH: u16 = 257;
    pub const BITS_PER_SAMPLE: u16 = 258;
    pub const COMPRESSION: u16 = 259;
    pub const PHOTOMETRIC: u16 = 262;
    pub const STRIP_OFFSETS: u16 = 273;
    pub const SAMPLES_PER_PIXEL: u16 = 277;
    pub const PLANAR: u16 = 284;
    pub const TILE_WIDTH: u16 = 322;
    pub const TILE_LENGTH: u16 = 323;
    pub const TILE_OFFSETS: u16 = 324;
    pub const TILE_BYTE_COUNTS: u16 = 325;
    pub const SAMPLE_FORMAT: u16 = 339;
    pub const MODEL_PIXEL_SCALE: u16 = 33550;
    pub const MODEL_TIEPOINT: u16 = 33922;
    pub const MODEL_TRANSFORMATION: u16 = 34264;
    pub const GEO_KEY_DIRECTORY: u16 = 34735;
    pub const GDAL_METADATA: u16 = 42112;
    pub const GDAL_NODATA: u16 = 42113;
}

/// Read a GeoTIFF's header from a file of `size` bytes.
///
/// # Errors
/// The file as a whole is not a TIFF (wrong magic, truncated, a directory past its end, a loop, a limit passed): the
/// reason in words. A TIFF a map cannot draw is an `Ok` with its `problems`.
pub fn read_cog_header(read_at: &mut ReadAt<'_>, size: u64) -> Result<CogInfo, String> {
    if size < 8 {
        return Err("the file is too short to be a TIFF".to_string());
    }
    let head_len = usize::try_from(size.min(HEAD as u64)).unwrap_or(HEAD);
    let head = read_at(0, head_len)?;
    if head.len() < 8 {
        return Err("the file's start could not be read".to_string());
    }
    let little = match &head[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return Err("the file is not a TIFF (its first bytes are not II or MM)".to_string()),
    };
    let mut bytes = Bytes {
        head,
        size,
        read_at,
        read: 0,
        little,
    };
    let magic = bytes.u16(&bytes.head[2..4]);
    let (big, first) = match magic {
        42 => (false, u64::from(bytes.u32(&bytes.head[4..8]))),
        43 => {
            if bytes.head.len() < 16 || bytes.u16(&bytes.head[4..6]) != 8 || bytes.u16(&bytes.head[6..8]) != 0 {
                return Err("the file's BigTIFF header is not one".to_string());
            }
            (true, bytes.u64(&bytes.head[8..16]))
        }
        _ => return Err(format!("the file is not a TIFF (its version is {magic}, not 42 or 43)")),
    };
    let mut ifds = Vec::new();
    let mut seen = HashSet::new();
    let mut next = first;
    while next != 0 {
        if ifds.len() >= MAX_IFDS {
            return Err(format!("the file has more than {MAX_IFDS} image directories"));
        }
        if !seen.insert(next) {
            return Err("the file's image directories make a loop".to_string());
        }
        if next < 8 {
            return Err(format!("an image directory at {next} is inside the header"));
        }
        let (ifd, following) = read_ifd(&mut bytes, next, big)?;
        ifds.push(ifd);
        next = following;
    }
    let Some(main) = ifds.first() else {
        return Err("the file has no image".to_string());
    };
    let mut info = CogInfo {
        big_tiff: big,
        ..CogInfo::default()
    };
    describe_main(&mut bytes, main, &mut info)?;
    // the overviews and masks after it
    let mut last_offset = main.offset;
    let mut in_order = true;
    for ifd in &ifds[1..] {
        if ifd.offset < last_offset {
            in_order = false;
        }
        last_offset = ifd.offset;
        let kind = small(&mut bytes, ifd, tag::NEW_SUBFILE_TYPE)?.unwrap_or(0);
        if kind & 4 == 4 {
            info.masks += 1;
            continue;
        }
        let width = small(&mut bytes, ifd, tag::IMAGE_WIDTH)?.unwrap_or(0);
        let height = small(&mut bytes, ifd, tag::IMAGE_LENGTH)?.unwrap_or(0);
        if kind & 1 == 1 && width > 0 && height > 0 && width <= info.width && height <= info.height {
            if ifd.entry(tag::TILE_WIDTH).is_none() {
                info.problems.push("an overview is in strips, not tiles".to_string());
            }
            info.overviews.push(Overview { width, height });
        }
    }
    if !in_order || ifds.iter().any(|ifd| ifd.offset > HEAD as u64) {
        info.notes.push(
            "its image directories are not all at its start (not the COG layout): a map asks more ranges to read them"
                .to_string(),
        );
    }
    if info.width.max(info.height) > 2048 && info.overviews.is_empty() {
        info.problems.push(format!(
            "it is {} by {} pixels with no overview, so a map would read it whole at every zoom: add overviews (gdal_translate -of COG)",
            info.width, info.height
        ));
    }
    Ok(info)
}

/// One directory at `offset`: its entries, and the offset of the next (0 for none).
fn read_ifd(bytes: &mut Bytes<'_, '_>, offset: u64, big: bool) -> Result<(Ifd, u64), String> {
    let count_len = if big { 8 } else { 2 };
    let count_bytes = bytes.get(offset, count_len)?;
    let count = if big {
        bytes.u64(&count_bytes)
    } else {
        u64::from(bytes.u16(&count_bytes))
    };
    if count == 0 || count > MAX_ENTRIES {
        return Err(format!(
            "an image directory at {offset} has {count} entries (at most {MAX_ENTRIES} are read)"
        ));
    }
    let entry_len = if big { 20 } else { 12 };
    let next_len = if big { 8 } else { 4 };
    let table = bytes.get(offset + count_len, count * entry_len + next_len)?;
    let mut entries = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for index in 0..usize::try_from(count).unwrap_or(0) {
        let at = index * entry_len as usize;
        let raw = &table[at..at + entry_len as usize];
        let (count, field) = if big {
            (bytes.u64(&raw[4..12]), raw[12..20].to_vec())
        } else {
            (u64::from(bytes.u32(&raw[4..8])), raw[8..12].to_vec())
        };
        entries.push(Entry {
            tag: bytes.u16(&raw[0..2]),
            kind: bytes.u16(&raw[2..4]),
            count,
            field,
        });
    }
    let tail = &table[table.len() - next_len as usize..];
    let next = if big {
        bytes.u64(tail)
    } else {
        u64::from(bytes.u32(tail))
    };
    if next != 0 && next >= bytes.size {
        return Err(format!(
            "an image directory points to the next at {next}, past the file's end"
        ));
    }
    Ok((Ifd { offset, entries }, next))
}

/// An entry's value bytes, at most `max` of them (the count times its type's size); an entry past the limit is refused.
fn value_bytes(bytes: &mut Bytes<'_, '_>, entry: &Entry, max: u64) -> Result<Vec<u8>, String> {
    let size = type_size(entry.kind).ok_or_else(|| format!("tag {} has an unknown type {}", entry.tag, entry.kind))?;
    let len = entry.count.checked_mul(size).filter(|len| *len <= max).ok_or_else(|| {
        format!(
            "tag {}'s value of {} items is longer than is read",
            entry.tag, entry.count
        )
    })?;
    if len <= entry.field.len() as u64 {
        return Ok(entry.field[..len as usize].to_vec());
    }
    let offset = if entry.field.len() == 8 {
        bytes.u64(&entry.field)
    } else {
        u64::from(bytes.u32(&entry.field))
    };
    bytes.get(offset, len)
}

/// An entry's values as unsigned integers (BYTE, SHORT, LONG, LONG8), at most `max` of them.
fn integers(bytes: &mut Bytes<'_, '_>, entry: &Entry, max: u64) -> Result<Vec<u64>, String> {
    let size = type_size(entry.kind).unwrap_or(0);
    if !matches!(entry.kind, 1 | 3 | 4 | 16) {
        return Err(format!("tag {} is not an integer", entry.tag));
    }
    let raw = value_bytes(bytes, entry, max.saturating_mul(size))?;
    Ok(raw
        .chunks(size as usize)
        .map(|chunk| match entry.kind {
            1 => u64::from(chunk[0]),
            3 => u64::from(bytes.u16(chunk)),
            4 => u64::from(bytes.u32(chunk)),
            _ => bytes.u64(chunk),
        })
        .collect())
}

fn doubles(bytes: &mut Bytes<'_, '_>, entry: &Entry, max: u64) -> Result<Vec<f64>, String> {
    if entry.kind != 12 {
        return Err(format!("tag {} is not of doubles", entry.tag));
    }
    let raw = value_bytes(bytes, entry, max.saturating_mul(8))?;
    Ok(raw.chunks(8).map(|chunk| bytes.f64(chunk)).collect())
}

fn ascii(bytes: &mut Bytes<'_, '_>, entry: &Entry, max: u64) -> Result<String, String> {
    if entry.kind != 2 {
        return Err(format!("tag {} is not text", entry.tag));
    }
    let raw = value_bytes(bytes, entry, max)?;
    Ok(String::from_utf8_lossy(&raw).trim_end_matches('\0').trim().to_string())
}

/// A tag's one small integer, or None when the directory has no such tag
fn small(bytes: &mut Bytes<'_, '_>, ifd: &Ifd, tag: u16) -> Result<Option<u64>, String> {
    match ifd.entry(tag) {
        Some(entry) => Ok(integers(bytes, entry, 1)?.first().copied()),
        None => Ok(None),
    }
}

/// The full-resolution image's dimensions, samples, tiles and georeference.
fn describe_main(bytes: &mut Bytes<'_, '_>, ifd: &Ifd, info: &mut CogInfo) -> Result<(), String> {
    info.width = small(bytes, ifd, tag::IMAGE_WIDTH)?.unwrap_or(0);
    info.height = small(bytes, ifd, tag::IMAGE_LENGTH)?.unwrap_or(0);
    if info.width == 0 || info.height == 0 || info.width > MAX_SIDE || info.height > MAX_SIDE {
        return Err(format!(
            "its image is {} by {} pixels (each side from 1 to {MAX_SIDE})",
            info.width, info.height
        ));
    }
    let bands = small(bytes, ifd, tag::SAMPLES_PER_PIXEL)?.unwrap_or(1);
    if bands == 0 || bands > MAX_BANDS {
        return Err(format!("it has {bands} bands (from 1 to {MAX_BANDS} are read)"));
    }
    info.bands = u16::try_from(bands).unwrap_or(1);
    let bits = match ifd.entry(tag::BITS_PER_SAMPLE) {
        Some(entry) => integers(bytes, entry, MAX_BANDS)?,
        None => vec![1],
    };
    info.bits_per_sample = bits
        .first()
        .copied()
        .and_then(|bits| u16::try_from(bits).ok())
        .unwrap_or(0);
    if bits.iter().any(|each| *each != u64::from(info.bits_per_sample)) {
        info.problems
            .push("its bands are of different sample sizes".to_string());
    }
    if ![8, 16, 32, 64].contains(&info.bits_per_sample) {
        info.problems.push(format!(
            "its samples are {} bits (8, 16, 32 or 64 are drawn)",
            info.bits_per_sample
        ));
    }
    info.sample_format = match ifd.entry(tag::SAMPLE_FORMAT) {
        Some(entry) => integers(bytes, entry, MAX_BANDS)?
            .first()
            .and_then(|value| u16::try_from(*value).ok())
            .unwrap_or(1),
        None => 1,
    };
    if ![1, 2, 3].contains(&info.sample_format) {
        info.problems.push(format!(
            "its sample format is {} (unsigned, signed or floating point are drawn)",
            info.sample_format
        ));
    }
    info.compression = small(bytes, ifd, tag::COMPRESSION)?
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(1);
    if !SUPPORTED_COMPRESSION.contains(&info.compression) {
        info.problems.push(format!(
            "its compression ({}) is not one a browser decodes (none, LZW, JPEG, Deflate, PackBits, LERC, Zstandard, WebP)",
            info.compression
        ));
    }
    info.photometric = small(bytes, ifd, tag::PHOTOMETRIC)?
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(1);
    info.planar = small(bytes, ifd, tag::PLANAR)?
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(1);
    // tiles, not strips
    let tile_width = small(bytes, ifd, tag::TILE_WIDTH)?;
    let tile_height = small(bytes, ifd, tag::TILE_LENGTH)?;
    match (tile_width, tile_height) {
        (Some(width), Some(height)) if ifd.entry(tag::STRIP_OFFSETS).is_none() => {
            info.tile_width = width;
            info.tile_height = height;
            if width == 0 || height == 0 || width % 16 != 0 || height % 16 != 0 || width > 4096 || height > 4096 {
                info.problems.push(format!(
                    "its tiles are {width} by {height} pixels (a multiple of 16, at most 4096)"
                ));
            } else {
                let across = info.width.div_ceil(width);
                let down = info.height.div_ceil(height);
                let planes = if info.planar == 2 { bands } else { 1 };
                let expected = across * down * planes;
                for (name, tag) in [("offsets", tag::TILE_OFFSETS), ("byte counts", tag::TILE_BYTE_COUNTS)] {
                    match ifd.entry(tag) {
                        Some(entry) if entry.count == expected => {}
                        Some(entry) => info
                            .problems
                            .push(format!("it lists {} tile {name} for its {expected} tiles", entry.count)),
                        None => info.problems.push(format!("it has no tile {name}")),
                    }
                }
            }
        }
        _ => info.problems.push(
            "it is stored in strips, not tiles, so it is not cloud optimized: write it with gdal_translate -of COG"
                .to_string(),
        ),
    }
    // the GeoTIFF keys
    match ifd.entry(tag::GEO_KEY_DIRECTORY) {
        None => info
            .problems
            .push("it has no GeoTIFF keys, so where it is on the Earth is not known".to_string()),
        // SHORT values, as the GeoTIFF specification has the directory (a LONG or LONG8 one is not a GeoTIFF's)
        Some(entry) if entry.kind != 3 => info
            .problems
            .push("its GeoTIFF key directory is not of SHORT values".to_string()),
        Some(entry) => {
            let keys = integers(bytes, entry, 4 * 256)?;
            geo_keys(&keys, info);
        }
    }
    // the georeference: a pixel scale and a tiepoint, or a transformation with no rotation
    let scale = match ifd.entry(tag::MODEL_PIXEL_SCALE) {
        Some(entry) => Some(doubles(bytes, entry, 3)?),
        None => None,
    };
    let tiepoint = match ifd.entry(tag::MODEL_TIEPOINT) {
        Some(entry) => Some(doubles(bytes, entry, 6 * 64)?),
        None => None,
    };
    let transformation = match ifd.entry(tag::MODEL_TRANSFORMATION) {
        Some(entry) => Some(doubles(bytes, entry, 16)?),
        None => None,
    };
    let origin_scale = match (scale, tiepoint, transformation) {
        (Some(scale), Some(tie), _) if scale.len() >= 2 && tie.len() >= 6 => {
            if tie.len() > 6 {
                info.notes
                    .push("it has more than one tiepoint: the first is used".to_string());
            }
            Some((
                tie[3] - tie[0] * scale[0],
                tie[4] + tie[1] * scale[1],
                scale[0],
                scale[1],
            ))
        }
        (_, _, Some(matrix)) if matrix.len() == 16 => {
            if matrix[1] != 0.0 || matrix[4] != 0.0 {
                info.problems
                    .push("its georeference is rotated or sheared, which a map does not draw".to_string());
                None
            } else {
                Some((matrix[3], matrix[7], matrix[0], -matrix[5]))
            }
        }
        _ => {
            info.problems
                .push("it has no georeference (a pixel scale and a tiepoint, or a transformation)".to_string());
            None
        }
    };
    if let Some((x0, y0, sx, sy)) = origin_scale {
        let finite = [x0, y0, sx, sy].iter().all(|value| value.is_finite());
        if !finite || sx <= 0.0 || sy <= 0.0 {
            info.problems
                .push("its georeference is not a positive pixel size".to_string());
        } else {
            let (west, north) = (x0, y0);
            let east = west + sx * info.width as f64;
            let south = north - sy * info.height as f64;
            info.bounds = Some([west, south, east, north]);
            info.resolution = Some([sx, sy]);
        }
    }
    if let Some(entry) = ifd.entry(tag::GDAL_NODATA) {
        let text = ascii(bytes, entry, 64)?;
        if !text.is_empty() {
            info.nodata = Some(text);
        }
    }
    if let Some(entry) = ifd.entry(tag::GDAL_METADATA) {
        // a large metadata text is passed over, not refused: statistics are a convenience
        if let Ok(text) = ascii(bytes, entry, 64 * 1024) {
            info.statistics = gdal_statistics(&text, info.bands);
        }
    }
    Ok(())
}

/// The GeoKeyDirectory's keys: the model type, raster type and the EPSG code of its CRS.
fn geo_keys(keys: &[u64], info: &mut CogInfo) {
    if keys.len() < 4 || keys[0] != 1 {
        info.problems
            .push("its GeoTIFF key directory is not version 1".to_string());
        return;
    }
    let needed = usize::try_from(keys[3])
        .ok()
        .and_then(|count| count.checked_mul(4))
        .and_then(|values| values.checked_add(4));
    let Some(needed) = needed.filter(|needed| *needed <= keys.len()) else {
        info.problems
            .push("its GeoTIFF key directory is shorter than it says".to_string());
        return;
    };
    let count = (needed - 4) / 4;
    if keys.len() < needed {
        info.problems
            .push("its GeoTIFF key directory is shorter than it says".to_string());
        return;
    }
    let mut model = 0;
    let mut projected = None;
    let mut geographic = None;
    for key in keys[4..4 + count * 4].chunks(4) {
        // an inline SHORT value: location 0
        let value = if key[1] == 0 { Some(key[3]) } else { None };
        match key[0] {
            1024 => model = value.unwrap_or(0),
            1025 => info.pixel_is_point = value == Some(2),
            2048 => geographic = value,
            3072 => projected = value,
            _ => {}
        }
    }
    let code = if model == 2 {
        geographic
    } else {
        projected.or(geographic)
    };
    info.geographic = model == 2 || (projected.is_none() && geographic.is_some());
    match code {
        Some(32767) | None => info.problems.push(
            "its CRS is user-defined in its keys, not an EPSG code, so it is not drawn: write it with an EPSG CRS"
                .to_string(),
        ),
        Some(code) => info.epsg = u32::try_from(code).ok(),
    }
    if model == 3 {
        info.problems
            .push("its model is geocentric, which a map does not draw".to_string());
    }
    if info.pixel_is_point {
        info.notes
            .push("its raster is PixelIsPoint: drawn as areas, half a pixel apart".to_string());
    }
}

/// GDAL_METADATA's STATISTICS_MINIMUM and STATISTICS_MAXIMUM by band (sample 0 is band 1).
fn gdal_statistics(text: &str, bands: u16) -> Vec<(u16, f64, f64)> {
    let mut found: Vec<(u16, Option<f64>, Option<f64>)> = Vec::new();
    for item in text.split("<Item").skip(1) {
        let attribute = |name: &str| -> Option<String> {
            let marker = format!("{name}=\"");
            let at = item.find(&marker)? + marker.len();
            Some(item[at..].split('"').next()?.to_string())
        };
        let Some(name) = attribute("name") else { continue };
        let sample: u16 = attribute("sample").and_then(|sample| sample.parse().ok()).unwrap_or(0);
        if sample >= bands {
            continue;
        }
        let value: Option<f64> = item
            .split_once('>')
            .and_then(|(_, rest)| rest.split('<').next())
            .and_then(|value| value.trim().parse().ok())
            .filter(|value: &f64| value.is_finite());
        let band = sample + 1;
        let slot = match found.iter().position(|entry| entry.0 == band) {
            Some(at) => at,
            None => {
                found.push((band, None, None));
                found.len() - 1
            }
        };
        match name.as_str() {
            "STATISTICS_MINIMUM" => found[slot].1 = value,
            "STATISTICS_MAXIMUM" => found[slot].2 = value,
            _ => {}
        }
    }
    let mut out: Vec<(u16, f64, f64)> = found
        .into_iter()
        .filter_map(|(band, min, max)| Some((band, min?, max?)))
        .collect();
    out.sort_by_key(|entry| entry.0);
    out
}

#[cfg(test)]
#[path = "tiff_tests.rs"]
mod tests;
