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

//! Archive and digest helpers for scripts that move data in and out of the system: exports with checksums and imports
//! of operator-supplied archives. Each reads its source as a stream, so a large file is never held in memory whole.
//! Hidden functions of the tools shim, which puts them on `ec.resource`:
//!
//! ```text
//! ec.resource.digest(location, "sha256" | "sha1")            -> lower-case hex of the file's bytes
//! ec.resource.digestText(text, "sha256" | "sha1")            -> lower-case hex of the text's UTF-8 bytes: a stable key
//!     for a value too long to be a key itself
//! ec.resource.gunzip(source, target, maxBytes)                 -> bytes written; refused past maxBytes
//! ec.resource.extractArchive(source, targetDir, limits)        -> {entries: [{path, size}], skipped: [{path, reason}]}
//!     a tar, gzip-compressed or not; regular files only, no absolute or parent paths, limits {maxBytes, maxEntries}
//! ec.resource.readLines(location, offset, maxLines, maxBytes)  -> {lines, nextOffset, done}
//!     lines from a byte offset, so a long file is read in checkpointed pieces
//! ec.resource.writeTarGz(target, [{path, location}])            -> entries written
//!     a gzip-compressed tar of files, the tar counterpart of writeZip
//! ```

use std::io::Read;

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostError, Method, Ret};

/// The archive functions of the tools shim.
pub(super) static METHODS: &[Method] = &[
    method!("tools", "resourceDigest", [Text, Text], resource_digest),
    method!("tools", "textDigest", [Text, Text], None, leaf, text_digest),
    method!("tools", "resourceGunzip", [Text, Text, Number], resource_gunzip),
    method!(
        "tools",
        "resourceExtractArchive",
        [Text, Text, OptValue],
        resource_extract_archive
    ),
    method!(
        "tools",
        "resourceReadLines",
        [Text, Number, Number, Number],
        resource_read_lines
    ),
    method!("tools", "resourceWriteTarGz", [Text, Value], resource_write_tar_gz),
];

/// The source at a location as a reader from `offset`, or an error naming the location when it is not there.
fn reader_at(location: &str, offset: u64) -> Result<crate::resource::source::SourceReader, HostError> {
    let opened = with_ec(|ec| ec.factory().resource().open(location))?
        .map_err(HostError::Context)?
        .ok_or_else(|| HostError::message(format!("no file at {location}")))?;
    Ok(crate::resource::source::SourceReader::from(opened.source, offset))
}

/// A reader that fails once more than `limit` bytes pass through it, so an archive cannot expand without bound.
struct Limited<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buf)?;
        if count as u64 > self.left {
            return Err(std::io::Error::other("the expanded data passes the size limit"));
        }
        self.left -= count as u64;
        Ok(count)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unknown_digest(other: &str) -> HostError {
    HostError::message(format!("unknown digest {other}: use sha256 or sha1"))
}

fn resource_digest(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use sha2::Digest;
    let location = call.text_or_empty(0);
    let algorithm = call.text_or_empty(1);
    let mut reader = reader_at(&location, 0)?;
    let mut buffer = vec![0u8; 64 * 1024];
    let read = |reader: &mut crate::resource::source::SourceReader, buffer: &mut [u8]| {
        reader
            .read(buffer)
            .map_err(|err| HostError::message(format!("cannot read {location}: {err}")))
    };
    match algorithm.as_str() {
        "sha256" => {
            let mut hasher = sha2::Sha256::new();
            loop {
                let count = read(&mut reader, &mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(hex(&hasher.finalize()).into())
        }
        "sha1" => {
            let mut hasher = sha1::Sha1::new();
            loop {
                let count = read(&mut reader, &mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            Ok(hex(&hasher.finalize()).into())
        }
        other => Err(unknown_digest(other)),
    }
}

fn text_digest(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use sha2::Digest;
    let text = call.text_or_empty(0);
    match call.text(1).unwrap_or_default() {
        "sha256" => Ok(hex(&sha2::Sha256::digest(text.as_bytes())).into()),
        "sha1" => Ok(hex(&sha1::Sha1::digest(text.as_bytes())).into()),
        other => Err(unknown_digest(other)),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn resource_gunzip(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = call.text_or_empty(0);
    let target = call.text_or_empty(1);
    let max_bytes = call.number(2).unwrap_or_default();
    let reader = reader_at(&source, 0)?;
    let mut limited = Limited {
        inner: flate2::read::GzDecoder::new(reader),
        left: max_bytes.max(0.0) as u64,
    };
    let limit = limited.left;
    let written = with_ec(|ec| ec.factory().resource().write_from(&target, &mut limited, None))?;
    written.map_err(|err| HostError::message(format!("cannot expand {source}: {}", err.message())))?;
    Ok(Value::F64((limit - limited.left) as f64).into())
}

/// The relative path of a tar entry when it is safe to write under the target: plain names only.
fn safe_path(path: &std::path::Path) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::Normal(name) => parts.push(name.to_str()?.to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// The number a value is, as a script number reads: none for anything else.
#[allow(clippy::cast_precision_loss)]
fn number_of(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::I32(number) => Some(f64::from(*number)),
        Value::I64(number) => Some(*number as f64),
        Value::F64(number) => Some(*number),
        _ => None,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn resource_extract_archive(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let source = call.text_or_empty(0);
    let target = call.text_or_empty(1);
    let limits = match call.value(2) {
        Some(Value::Map(map)) => Some(map.clone()),
        _ => None,
    };
    let limit = |name: &str, default: f64| number_of(limits.as_ref().and_then(|map| map.get(name))).unwrap_or(default);
    let max_bytes = limit("maxBytes", 4.0 * 1024.0 * 1024.0 * 1024.0).max(0.0) as u64;
    let max_entries = limit("maxEntries", 1_000_000.0).max(0.0) as usize;
    // gzip by its magic bytes, not by the name
    let mut head = [0u8; 2];
    let gzip = reader_at(&source, 0)?.read(&mut head).unwrap_or(0) == 2 && head == [0x1f, 0x8b];
    let reader = reader_at(&source, 0)?;
    let stream: Box<dyn Read> = if gzip {
        Box::new(flate2::read::GzDecoder::new(reader))
    } else {
        Box::new(reader)
    };
    let mut archive = tar::Archive::new(Limited {
        inner: stream,
        left: max_bytes,
    });
    let entries = archive
        .entries()
        .map_err(|err| HostError::message(format!("{source} is not a tar archive: {err}")))?;
    let base = target.trim_end_matches('/').to_string();
    let mut written = Vec::new();
    let mut skipped = Vec::new();
    let mut skip = |path: String, reason: &str| {
        let mut map = indexmap::IndexMap::default();
        map.insert("path".to_string(), Value::String(path));
        map.insert("reason".to_string(), Value::string(reason));
        skipped.push(Value::Map(map));
    };
    for entry in entries {
        let mut entry = entry.map_err(|err| HostError::message(format!("cannot read {source}: {err}")))?;
        let raw = entry
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        if !kind.is_file() {
            skip(raw, "not a regular file");
            continue;
        }
        let Some(path) = entry.path().ok().and_then(|p| safe_path(&p)) else {
            skip(raw, "an absolute or parent path");
            continue;
        };
        if written.len() >= max_entries {
            skip(path, "past the entry limit");
            continue;
        }
        let size = entry.size();
        let location = format!("{base}/{path}");
        let result = with_ec(|ec| ec.factory().resource().write_from(&location, &mut entry, Some(size)))?;
        if let Err(err) = result {
            return Err(HostError::message(format!("cannot write {path}: {}", err.message())));
        }
        let mut map = indexmap::IndexMap::default();
        map.insert("path".to_string(), Value::String(path));
        map.insert("size".to_string(), Value::I64(size as i64));
        written.push(Value::Map(map));
    }
    let mut result = indexmap::IndexMap::default();
    result.insert("entries".to_string(), Value::List(written));
    result.insert("skipped".to_string(), Value::List(skipped));
    Ok(Value::Map(result).into())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn resource_read_lines(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use std::io::BufRead;
    let location = call.text_or_empty(0);
    let start = call.number(1).unwrap_or_default().max(0.0) as u64;
    let max_lines = call.number(2).unwrap_or_default().max(1.0) as usize;
    let max_bytes = call.number(3).unwrap_or_default().max(1.0) as u64;
    let mut reader = reader_at(&location, start)?;
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut consumed = 0u64;
    let mut done = false;
    let mut buffered = std::io::BufReader::with_capacity(256 * 1024, &mut reader);
    'read: loop {
        let available = buffered
            .fill_buf()
            .map_err(|err| HostError::message(format!("cannot read {location}: {err}")))?;
        if available.is_empty() {
            if !current.is_empty() {
                lines.push(Value::String(String::from_utf8_lossy(&current).into_owned()));
                current.clear();
            }
            done = true;
            break;
        }
        let mut used = 0;
        for &b in available {
            used += 1;
            if b == b'\n' {
                if current.last() == Some(&b'\r') {
                    current.pop();
                }
                lines.push(Value::String(String::from_utf8_lossy(&current).into_owned()));
                current.clear();
                if lines.len() >= max_lines || consumed + used as u64 >= max_bytes {
                    buffered.consume(used);
                    consumed += used as u64;
                    break 'read;
                }
            } else {
                current.push(b);
                // One line may not grow without bound: a file with no line breaks is refused, not read whole.
                if current.len() as u64 > max_bytes {
                    return Err(HostError::message(format!(
                        "a line of {location} passes the {max_bytes}-byte limit"
                    )));
                }
            }
        }
        buffered.consume(used);
        consumed += used as u64;
    }
    let mut result = indexmap::IndexMap::default();
    result.insert("lines".to_string(), Value::List(lines));
    result.insert("nextOffset".to_string(), Value::I64((start + consumed) as i64));
    result.insert("done".to_string(), Value::Bool(done));
    Ok(Value::Map(result).into())
}

/// The text of a field of an entry, as the script gave it.
fn entry_text(entry: &Value, name: &str) -> String {
    match entry {
        Value::Map(map) => map
            .get(name)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn resource_write_tar_gz(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let target = call.text_or_empty(0);
    let entries = match call.value(1) {
        Some(Value::List(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    let mut count = 0f64;
    for item in &entries {
        let path = entry_text(item, "path");
        let location = entry_text(item, "location");
        let Some(path) = safe_path(std::path::Path::new(&path)) else {
            return Err(HostError::message(format!("{path} is not a relative path")));
        };
        let bytes = with_ec(|ec| ec.factory().resource().get(&location))?
            .and_then(|found| found.read_bytes())
            .map_err(HostError::Context)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, &path, bytes.as_slice())
            .map_err(|err| HostError::message(format!("cannot add {path}: {err}")))?;
        count += 1.0;
    }
    let encoder = builder
        .into_inner()
        .map_err(|err| HostError::message(format!("cannot finish the archive: {err}")))?;
    let bytes = encoder
        .finish()
        .map_err(|err| HostError::message(format!("cannot finish the archive: {err}")))?;
    let written = with_ec(|ec| ec.factory().resource().write_bytes(&target, &bytes))?;
    written.map_err(|err| HostError::message(err.message()))?;
    Ok(Value::F64(count).into())
}
