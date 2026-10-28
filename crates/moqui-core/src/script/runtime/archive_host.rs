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
//!
//! ```text
//! ec.resource.digest(location, "sha256" | "sha1")            -> lower-case hex of the file's bytes
//! ec.resource.gunzip(source, target, maxBytes)                 -> bytes written; refused past maxBytes
//! ec.resource.extractArchive(source, targetDir, limits)        -> {entries: [{path, size}], skipped: [{path, reason}]}
//!     a tar, gzip-compressed or not; regular files only, no absolute or parent paths, limits {maxBytes, maxEntries}
//! ec.resource.readLines(location, offset, maxLines, maxBytes)  -> {lines, nextOffset, done}
//!     lines from a byte offset, so a long file is read in checkpointed pieces
//! ec.resource.writeTarGz(target, [{path, location}])            -> entries written
//!     a gzip-compressed tar of files, the tar counterpart of writeZip
//! ```

use super::*;
use moqui_lib::object::Value;
use std::io::Read;

/// The source at a location as a reader from `offset`, or an error naming the location when it is not there.
fn reader_at(ctx: &Ctx<'_>, location: &str, offset: u64) -> rquickjs::Result<crate::resource::source::SourceReader> {
    let opened = with_ec(ctx, |ec| ec.factory().resource().open(location))?
        .map_err(|err| throw_context(ctx, &err))?
        .ok_or_else(|| Exception::throw_message(ctx, &format!("no file at {location}")))?;
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

fn resource_digest(ctx: Ctx<'_>, location: String, algorithm: String) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        use sha2::Digest;
        let mut reader = reader_at(&ctx, &location, 0)?;
        let mut buffer = vec![0u8; 64 * 1024];
        let read = |reader: &mut crate::resource::source::SourceReader, buffer: &mut [u8]| {
            reader
                .read(buffer)
                .map_err(|err| Exception::throw_message(&ctx, &format!("cannot read {location}: {err}")))
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
                Ok(hex(&hasher.finalize()))
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
                Ok(hex(&hasher.finalize()))
            }
            other => Err(Exception::throw_message(
                &ctx,
                &format!("unknown digest {other}: use sha256 or sha1"),
            )),
        }
    })
}

fn resource_gunzip(ctx: Ctx<'_>, source: String, target: String, max_bytes: f64) -> rquickjs::Result<f64> {
    host_guard(&ctx, || {
        let reader = reader_at(&ctx, &source, 0)?;
        let mut limited = Limited {
            inner: flate2::read::GzDecoder::new(reader),
            left: max_bytes.max(0.0) as u64,
        };
        let limit = limited.left;
        let written = with_ec(&ctx, |ec| {
            ec.factory().resource().write_from(&target, &mut limited, None)
        })?;
        written.map_err(|err| Exception::throw_message(&ctx, &format!("cannot expand {source}: {}", err.message())))?;
        Ok((limit - limited.left) as f64)
    })
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

fn resource_extract_archive<'js>(
    ctx: Ctx<'js>,
    source: String,
    target: String,
    limits: Opt<Object<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let limit = |name: &str, default: f64| {
            limits
                .0
                .as_ref()
                .and_then(|o| o.get::<_, Option<f64>>(name).ok().flatten())
                .unwrap_or(default)
        };
        let max_bytes = limit("maxBytes", 4.0 * 1024.0 * 1024.0 * 1024.0).max(0.0) as u64;
        let max_entries = limit("maxEntries", 1_000_000.0).max(0.0) as usize;
        // gzip by its magic bytes, not by the name
        let mut head = [0u8; 2];
        let gzip = reader_at(&ctx, &source, 0)?.read(&mut head).unwrap_or(0) == 2 && head == [0x1f, 0x8b];
        let reader = reader_at(&ctx, &source, 0)?;
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
            .map_err(|err| Exception::throw_message(&ctx, &format!("{source} is not a tar archive: {err}")))?;
        let base = target.trim_end_matches('/').to_string();
        let mut written = Vec::new();
        let mut skipped = Vec::new();
        let mut skip = |path: String, reason: &str| {
            let mut map = indexmap::IndexMap::new();
            map.insert("path".to_string(), Value::String(path));
            map.insert("reason".to_string(), Value::string(reason));
            skipped.push(Value::Map(map));
        };
        for entry in entries {
            let mut entry =
                entry.map_err(|err| Exception::throw_message(&ctx, &format!("cannot read {source}: {err}")))?;
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
            let result = with_ec(&ctx, |ec| {
                ec.factory().resource().write_from(&location, &mut entry, Some(size))
            })?;
            if let Err(err) = result {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("cannot write {path}: {}", err.message()),
                ));
            }
            let mut map = indexmap::IndexMap::new();
            map.insert("path".to_string(), Value::String(path));
            map.insert("size".to_string(), Value::I64(size as i64));
            written.push(Value::Map(map));
        }
        let mut result = indexmap::IndexMap::new();
        result.insert("entries".to_string(), Value::List(written));
        result.insert("skipped".to_string(), Value::List(skipped));
        host_to_js(&ctx, Value::Map(result))
    })
}

fn resource_read_lines<'js>(
    ctx: Ctx<'js>,
    location: String,
    offset: f64,
    max_lines: f64,
    max_bytes: f64,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let start = offset.max(0.0) as u64;
        let max_lines = max_lines.max(1.0) as usize;
        let max_bytes = max_bytes.max(1.0) as u64;
        let mut reader = reader_at(&ctx, &location, start)?;
        let mut lines = Vec::new();
        let mut current = Vec::new();
        let mut consumed = 0u64;
        let mut done = false;
        let mut buffered = std::io::BufReader::with_capacity(256 * 1024, &mut reader);
        use std::io::BufRead;
        'read: loop {
            let available = buffered
                .fill_buf()
                .map_err(|err| Exception::throw_message(&ctx, &format!("cannot read {location}: {err}")))?;
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
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("a line of {location} passes the {max_bytes}-byte limit"),
                        ));
                    }
                }
            }
            buffered.consume(used);
            consumed += used as u64;
        }
        let mut result = indexmap::IndexMap::new();
        result.insert("lines".to_string(), Value::List(lines));
        result.insert("nextOffset".to_string(), Value::I64((start + consumed) as i64));
        result.insert("done".to_string(), Value::Bool(done));
        host_to_js(&ctx, Value::Map(result))
    })
}

fn resource_write_tar_gz<'js>(ctx: Ctx<'js>, target: String, entries: rquickjs::Array<'js>) -> rquickjs::Result<f64> {
    host_guard(&ctx, || {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        let mut count = 0f64;
        for item in entries.iter::<Object>() {
            let item = item?;
            let path: String = item.get("path")?;
            let location: String = item.get("location")?;
            let Some(path) = safe_path(std::path::Path::new(&path)) else {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("{path} is not a relative path"),
                ));
            };
            let bytes = with_ec(&ctx, |ec| ec.factory().resource().get(&location))?
                .and_then(|found| found.read_bytes())
                .map_err(|err| throw_context(&ctx, &err))?;
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, &path, bytes.as_slice())
                .map_err(|err| Exception::throw_message(&ctx, &format!("cannot add {path}: {err}")))?;
            count += 1.0;
        }
        let encoder = builder
            .into_inner()
            .map_err(|err| Exception::throw_message(&ctx, &format!("cannot finish the archive: {err}")))?;
        let bytes = encoder
            .finish()
            .map_err(|err| Exception::throw_message(&ctx, &format!("cannot finish the archive: {err}")))?;
        let written = with_ec(&ctx, |ec| ec.factory().resource().write_bytes(&target, &bytes))?;
        written.map_err(|err| Exception::throw_message(&ctx, err.message()))?;
        Ok(count)
    })
}

pub(super) fn methods<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> Result<(), Error> {
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, host.set($name, function))?;
        };
    }
    method!("resourceDigest", resource_digest);
    method!("resourceGunzip", resource_gunzip);
    method!("resourceExtractArchive", resource_extract_archive);
    method!("resourceReadLines", resource_read_lines);
    method!("resourceWriteTarGz", resource_write_tar_gz);
    Ok(())
}
