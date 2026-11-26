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

//! `org.moqui.maps.MapSourceServices.read#ArchiveTile` (plans/Maps.md §5, "a later Rust endpoint returning individual
//! Z/X/Y tiles"): one tile of a registered PMTiles archive, for the LocalMap `tile` transition, which serves clients
//! with no PMTiles reader (an OpenLayers XYZ vector tile source, a terminal, another program) through the same resource
//! facade and the same per-request access check as the map-source transition: MapSources.js checks the caller and
//! hands this service the pinned location, tag and size, and this reads only the header, the directories the lookup
//! goes through and the tile, each by range ([`moqui_lib::pmtiles::locate_tile`]). Headers and decoded directories are
//! kept by the archive's tag (a content address, so a new revision is a new key and nothing is invalidated), within a
//! bound on directories and entries. It reads whatever location it is given, so it is no group's and not remote.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, LazyLock, Mutex};

use indexmap::IndexMap;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::pmtiles::{decode_directory, locate_tile, tile_id, Compression, Entry, Header, HEADER_LEN};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityFacade;
use moqui_core::resource::source::ByteSource;

use crate::group_db::{flag, number, text, Out};

/// The most bytes one directory may take, as stored and decompressed (a planet archive's largest leaves are a few MB).
const DIRECTORY_MAX: u64 = 16 * 1024 * 1024;
/// The most bytes a tile may take decompressed, when the client takes no gzip.
const TILE_MAX: u64 = 16 * 1024 * 1024;
/// The bound on what is kept: directories, and their entries in all (an entry is 24 bytes, so about 48 MB at most).
const CACHED_DIRECTORIES: usize = 256;
const CACHED_ENTRIES: usize = 2 * 1024 * 1024;
const CACHED_HEADERS: usize = 64;

pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    use moqui_core::rust_service;
    rust_service!(map, read_archive_tile)?;
    Ok(())
}

/// The headers and directories read, by the archive's tag (and a directory's offset), oldest used first.
struct Kept {
    headers: IndexMap<String, Header>,
    directories: IndexMap<String, Arc<Vec<Entry>>>,
    entries: usize,
}

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(|| {
    Mutex::new(Kept {
        headers: IndexMap::new(),
        directories: IndexMap::new(),
        entries: 0,
    })
});

fn with_kept<T>(body: impl FnOnce(&mut Kept) -> T) -> T {
    let mut guard = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    body(&mut guard)
}

fn kept_header(key: &str) -> Option<Header> {
    with_kept(|kept| {
        let header = kept.headers.shift_remove(key)?;
        kept.headers.insert(key.to_string(), header.clone());
        Some(header)
    })
}

fn keep_header(key: &str, header: &Header) {
    with_kept(|kept| {
        kept.headers.insert(key.to_string(), header.clone());
        while kept.headers.len() > CACHED_HEADERS {
            kept.headers.shift_remove_index(0);
        }
    });
}

fn kept_directory(key: &str) -> Option<Arc<Vec<Entry>>> {
    with_kept(|kept| {
        let entries = kept.directories.shift_remove(key)?;
        kept.directories.insert(key.to_string(), Arc::clone(&entries));
        Some(entries)
    })
}

fn keep_directory(key: &str, entries: &Arc<Vec<Entry>>) {
    with_kept(|kept| {
        if let Some(before) = kept.directories.insert(key.to_string(), Arc::clone(entries)) {
            kept.entries -= before.len();
        }
        kept.entries += entries.len();
        while kept.directories.len() > 1
            && (kept.directories.len() > CACHED_DIRECTORIES || kept.entries > CACHED_ENTRIES)
        {
            if let Some((_, dropped)) = kept.directories.shift_remove_index(0) {
                kept.entries -= dropped.len();
            }
        }
    });
}

/// The bytes of `length` at `offset`, all of them or an error: a source gives fewer only at its end.
fn read_exact(source: &dyn ByteSource, offset: u64, length: u64, what: &str) -> Result<Vec<u8>, LibError> {
    let wanted = usize::try_from(length).map_err(|_| LibError::new(format!("the {what} is too large")))?;
    let bytes = source.read_at(offset, wanted)?;
    if bytes.len() != wanted {
        return Err(LibError::new(format!("the archive's {what} is cut short")));
    }
    Ok(bytes)
}

/// `stored` decompressed by `compression`, at most `max` bytes.
fn decompress(stored: Vec<u8>, compression: Compression, max: u64, what: &str) -> Result<Vec<u8>, LibError> {
    match compression {
        Compression::None | Compression::Unknown => Ok(stored),
        Compression::Gzip => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(stored.as_slice())
                .take(max + 1)
                .read_to_end(&mut out)
                .map_err(|err| LibError::new(format!("the archive's {what} does not decompress: {err}")))?;
            if out.len() as u64 > max {
                return Err(LibError::new(format!("the archive's {what} is more than {max} bytes")));
            }
            Ok(out)
        }
        other => Err(LibError::new(format!(
            "the archive's {what} is compressed with {}, which is not read here",
            other.name()
        ))),
    }
}

/// What a tile request is answered with.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TileAnswer {
    /// The tile: its bytes, its content type, whether they are gzipped as sent (`Content-Encoding: gzip`), its ETag.
    Tile {
        bytes: Vec<u8>,
        content_type: &'static str,
        gzip: bool,
        etag: String,
    },
    /// The client has this tile at this revision already.
    NotModified { etag: String },
    /// No such tile in the archive; `vector` for an archive of vector tiles (204: an empty tile), else 404.
    Missing { vector: bool },
    /// A request this archive cannot answer: its status (404, 409) and why.
    Refused { status: i64, reason: String },
}

/// The request of one tile of the archive whose content is `source`, pinned as `tag` of `size` bytes.
pub(crate) struct TileRequest<'a> {
    pub tag: &'a str,
    pub size: u64,
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub accept_gzip: bool,
    pub if_none_match: &'a str,
}

/// The strong ETag of tile `tile_id` of the archive pinned as `tag`, for a gzipped or a plain body.
fn tile_etag(tag: &str, tile_id: u64, gzip: bool) -> String {
    format!("\"{tag}.{tile_id}{}\"", if gzip { ".gz" } else { "" })
}

/// Whether `if_none_match` (the header's text) names `etag` or is `*`.
fn matches_etag(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag))
}

/// The tile of `request` from `source`: the header and the directories by the archive's tag, kept between requests.
pub(crate) fn answer_tile(source: &dyn ByteSource, request: &TileRequest<'_>) -> Result<TileAnswer, LibError> {
    let tag = request.tag.trim();
    // the content is still the revision pinned: its size, and its strong tag where the storage has one
    if tag.is_empty() || source.len() != request.size || source.tag().is_some_and(|own| own != tag) {
        return Ok(TileAnswer::Refused {
            status: 409,
            reason: "the archive changed since it was registered".to_string(),
        });
    }
    let Ok(id) = tile_id(request.z, request.x, request.y) else {
        return Ok(TileAnswer::Refused {
            status: 404,
            reason: format!("there is no tile {}/{}/{}", request.z, request.x, request.y),
        });
    };
    // what is kept is the content's: its tag and its size
    let content = format!("{tag}:{}", request.size);
    let header = match kept_header(&content) {
        Some(header) => header,
        None => {
            let header = Header::parse(&read_exact(source, 0, HEADER_LEN as u64, "header")?)?;
            // an archive whose sections are not where its header says is read no further, and not kept
            if let Some(problem) = header.problems(request.size).into_iter().next() {
                return Err(LibError::new(format!("the archive cannot be read: {problem}")));
            }
            keep_header(&content, &header);
            header
        }
    };
    let vector = header.tile_type.is_vector();
    if request.z < header.min_zoom || request.z > header.max_zoom {
        return Ok(TileAnswer::Missing { vector });
    }
    let stored_gzip = header.tile_compression == Compression::Gzip;
    let gzip = stored_gzip && request.accept_gzip;
    let internal = header.internal_compression;
    let found = locate_tile(&header, id, |offset, length| {
        let key = format!("{content}@{offset}");
        if let Some(entries) = kept_directory(&key) {
            return Ok(entries);
        }
        if length > DIRECTORY_MAX {
            return Err(LibError::new(format!(
                "a directory of the archive is {length} bytes, more than the {DIRECTORY_MAX} read here"
            )));
        }
        let stored = read_exact(source, offset, length, "directory")?;
        let entries = Arc::new(decode_directory(&decompress(
            stored,
            internal,
            DIRECTORY_MAX,
            "directory",
        )?)?);
        keep_directory(&key, &entries);
        Ok(entries)
    })?;
    let Some((offset, length)) = found else {
        return Ok(TileAnswer::Missing { vector });
    };
    // a tile there and named by the client: not sent again (a missing tile is never Not Modified, whatever it names)
    let etag = tile_etag(tag, id, gzip);
    if !request.if_none_match.is_empty() && matches_etag(request.if_none_match, &etag) {
        return Ok(TileAnswer::NotModified { etag });
    }
    if u64::from(length) > TILE_MAX {
        return Err(LibError::new(format!(
            "a tile of the archive is {length} bytes, more than the {TILE_MAX} read here"
        )));
    }
    let stored = read_exact(source, offset, u64::from(length), "tile")?;
    let bytes = if stored_gzip && !gzip {
        decompress(stored, Compression::Gzip, TILE_MAX, "tile")?
    } else {
        stored
    };
    Ok(TileAnswer::Tile {
        bytes,
        content_type: header.tile_type.content_type(),
        gzip,
        etag,
    })
}

fn coordinate(parameters: &BTreeMap<String, Value>, key: &str) -> Option<u32> {
    // a y of an XYZ template may carry the tile's extension (12.mvt, 12.pbf, 12.png)
    let raw = text(parameters, key);
    let digits = raw.split('.').next().unwrap_or_default();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// `read#ArchiveTile`: tile `z`/`x`/`y` of the archive at `location` (pinned: `location#versionName` on storage that
/// keeps versions), registered as `tag` of `size` bytes, read as the framework (the caller checked the user). In:
/// `acceptGzip` (the client takes `Content-Encoding: gzip`), `ifNoneMatch`. Out: `status` (200, 204 for no tile of a
/// vector archive, 304, 404, 409 for content that is no longer the registered revision), `bytes`, `contentType`,
/// `contentEncoding` (gzip or empty), `etag` (strong: the tag, the tile ID and the encoding), `cacheControl` (private,
/// `resource-facade@pinned-max-age`, immutable) and `reason` for a refusal. A read that fails is a 404 with its reason,
/// logged.
pub(crate) fn read_archive_tile(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let mut out = Out::new();
    let refuse = |mut out: Out, status: i64, reason: String| {
        out.insert("status".to_string(), Value::I64(status));
        out.insert("reason".to_string(), Value::string(reason));
        Ok(out)
    };
    let location = text(parameters, "location");
    let tag = text(parameters, "tag");
    let size = number(parameters, "size").and_then(|size| u64::try_from(size).ok());
    let (Some(z), Some(x), Some(y), Some(size)) = (
        coordinate(parameters, "z").and_then(|z| u8::try_from(z).ok()),
        coordinate(parameters, "x"),
        coordinate(parameters, "y"),
        size,
    ) else {
        return refuse(out, 404, "not a tile's z, x and y".to_string());
    };
    if location.is_empty() {
        return refuse(out, 404, "no archive location".to_string());
    }
    let opened = ExecutionContext::with_current(|ec| {
        let _authz = ec.disable_authz();
        ec.factory().resource().open(&location)
    })
    .ok_or_else(|| LibError::new("no execution context to read the archive"))?;
    let source = match opened {
        Ok(Some(opened)) => opened.source,
        Ok(None) => return refuse(out, 409, "the archive is gone".to_string()),
        Err(err) => {
            tracing::warn!(location = %location, error = %err, "A map archive's tile could not be read");
            return refuse(out, 404, err.message().to_string());
        }
    };
    let if_none_match = text(parameters, "ifNoneMatch");
    let request = TileRequest {
        tag: &tag,
        size,
        z,
        x,
        y,
        accept_gzip: flag(parameters, "acceptGzip"),
        if_none_match: &if_none_match,
    };
    let answer = match answer_tile(source.as_ref(), &request) {
        Ok(answer) => answer,
        Err(err) => {
            tracing::warn!(location = %location, z, x, y, error = %err, "A map archive's tile could not be read");
            return refuse(out, 404, err.message().to_string());
        }
    };
    out.insert(
        "cacheControl".to_string(),
        Value::string(moqui_core::web::pinned_cache_control()),
    );
    match answer {
        TileAnswer::Tile {
            bytes,
            content_type,
            gzip,
            etag,
        } => {
            out.insert("status".to_string(), Value::I64(200));
            out.insert("bytes".to_string(), Value::Bytes(bytes));
            out.insert("contentType".to_string(), Value::string(content_type));
            out.insert(
                "contentEncoding".to_string(),
                Value::string(if gzip { "gzip" } else { "" }),
            );
            out.insert("etag".to_string(), Value::string(etag));
        }
        TileAnswer::NotModified { etag } => {
            out.insert("status".to_string(), Value::I64(304));
            out.insert("etag".to_string(), Value::string(etag));
        }
        TileAnswer::Missing { vector } => {
            out.insert("status".to_string(), Value::I64(if vector { 204 } else { 404 }));
        }
        TileAnswer::Refused { status, reason } => return refuse(out, status, reason),
    }
    Ok(out)
}

#[cfg(test)]
#[path = "maps_tiles_tests.rs"]
mod tests;
