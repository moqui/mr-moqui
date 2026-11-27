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

//! A map package's zip read with bounds (plans/Maps-Offline.md §2.4, Download): the server writes it with every member
//! stored (`ec.resource.writeZip`, zip64 past 4 GiB), so this reads only stored members, by the central directory,
//! and copies each member's bytes as a stream. No member is trusted for its name, kind or size: [`entries`] refuses a
//! compressed or encrypted member, a symlink, a directory, a name that is not a plain relative path, a repeated name,
//! and a central directory past its bound; the caller checks each against the manifest.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

/// The end of central directory record's signature, and its least size.
const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD_LEN: u64 = 22;
/// The zip64 end of central directory locator's and record's signatures.
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
/// A central directory header's and a local file header's signatures and fixed lengths.
const CENTRAL_SIG: u32 = 0x0201_4b50;
const CENTRAL_LEN: usize = 46;
const LOCAL_SIG: u32 = 0x0403_4b50;
const LOCAL_LEN: usize = 30;
/// The zip64 extended information extra field.
const ZIP64_EXTRA: u16 = 0x0001;
/// The longest comment an end record may have before it, which bounds the search for it.
const COMMENT_MAX: u64 = 65_535;
/// The most bytes the central directory may take, and the most members it may name: a package's members are its
/// basemaps, styles, sprites, fonts, overlays and lookups, a few hundred at most.
pub(crate) const CENTRAL_MAX: u64 = 16 * 1024 * 1024;
pub(crate) const ENTRIES_MAX: usize = 10_000;
/// The longest member path.
pub(crate) const PATH_MAX: usize = 512;

/// A member of the zip: its path, its size, and where its bytes start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) data_at: u64,
}

/// Why a zip is refused.
pub(crate) type Refused = String;

/// The members of the zip `file` of `len` bytes, each a stored, plain file ([`plain_path`]) whose bytes lie inside the
/// file; none repeated.
///
/// # Errors
///
/// Returns why the zip is refused, or the file's read error as text.
pub(crate) fn entries(file: &mut File, len: u64) -> Result<Vec<Entry>, Refused> {
    let (count, central_at, central_len) = central_directory(file, len)?;
    if central_len > CENTRAL_MAX || count > ENTRIES_MAX as u64 {
        return Err("the package's zip lists more than this app reads".to_string());
    }
    let end = central_at
        .checked_add(central_len)
        .filter(|end| *end <= len)
        .ok_or("the package's zip directory is outside the file")?;
    let mut central = vec![0u8; usize::try_from(central_len).map_err(|_| "the zip directory is too large")?];
    read_at(file, central_at, &mut central)?;
    let mut entries: Vec<Entry> = Vec::new();
    let mut at = 0usize;
    for _ in 0..count {
        let header = central
            .get(at..at + CENTRAL_LEN)
            .ok_or("the package's zip directory is cut short")?;
        if u32_at(header, 0) != CENTRAL_SIG {
            return Err("the package's zip directory is not one".to_string());
        }
        let flags = u16_at(header, 8);
        let method = u16_at(header, 10);
        let mut packed = u64::from(u32_at(header, 20));
        let mut size = u64::from(u32_at(header, 24));
        let name_len = usize::from(u16_at(header, 28));
        let extra_len = usize::from(u16_at(header, 30));
        let comment_len = usize::from(u16_at(header, 32));
        let made_by = u16_at(header, 4) >> 8;
        let attributes = u32_at(header, 38);
        let mut local_at = u64::from(u32_at(header, 42));
        let name_at = at + CENTRAL_LEN;
        let name = central
            .get(name_at..name_at + name_len)
            .ok_or("the package's zip directory is cut short")?;
        let extra = central
            .get(name_at + name_len..name_at + name_len + extra_len)
            .ok_or("the package's zip directory is cut short")?;
        at = name_at + name_len + extra_len + comment_len;
        let path = std::str::from_utf8(name)
            .map_err(|_| "a member of the package has a name that is not text".to_string())?
            .to_string();
        if !plain_path(&path) {
            return Err(format!(
                "a member of the package has a path outside it: {}",
                printable(&path)
            ));
        }
        if flags & 0x0001 != 0 {
            return Err(format!("{path} is encrypted"));
        }
        if method != 0 {
            return Err(format!("{path} is compressed; a package's members are stored"));
        }
        // a Unix mode in the high half of the external attributes (made by Unix, 3): no symlink, no directory
        let mode = attributes >> 16;
        if made_by == 3 && mode & 0o170_000 != 0 && mode & 0o170_000 != 0o100_000 {
            return Err(format!("{path} is not a plain file"));
        }
        if attributes & 0x10 != 0 {
            return Err(format!("{path} is a directory"));
        }
        // the zip64 sizes and offset, in the order the plain fields that need them were left at their marker
        if size == u64::from(u32::MAX) || packed == u64::from(u32::MAX) || local_at == u64::from(u32::MAX) {
            let mut values = zip64_values(extra).ok_or_else(|| format!("{path} has no zip64 sizes"))?;
            for field in [&mut size, &mut packed, &mut local_at] {
                if *field == u64::from(u32::MAX) {
                    *field = values.next().ok_or_else(|| format!("{path} has no zip64 sizes"))?;
                }
            }
        }
        if packed != size {
            return Err(format!("{path} is not stored as it is"));
        }
        if entries.iter().any(|entry| entry.path == path) {
            return Err(format!("{path} is in the package twice"));
        }
        let data_at = data_start(file, local_at, len, &path)?;
        let data_end = data_at.checked_add(size).filter(|data_end| *data_end <= end);
        if data_end.is_none() || data_at < local_at {
            return Err(format!("{path} lies outside the package's file"));
        }
        entries.push(Entry { path, size, data_at });
    }
    Ok(entries)
}

/// Copy `entry`'s bytes from `file` to `out`, at most its size: the bytes copied.
///
/// # Errors
///
/// Returns the read or write error as text.
pub(crate) fn copy_entry(file: &mut File, entry: &Entry, out: &mut dyn std::io::Write) -> Result<u64, String> {
    file.seek(SeekFrom::Start(entry.data_at))
        .map_err(|err| err.to_string())?;
    std::io::copy(&mut Read::take(&mut *file, entry.size), out).map_err(|err| err.to_string())
}

/// Whether `path` is a member path that stays in its directory: relative, `/`-separated, each part a plain name of
/// letters, digits, `.`, `-`, `_` and `@` that does not start with `.` (so no `.`, `..` or hidden file), no `\`, no
/// drive, no control character, at most [`PATH_MAX`] bytes.
pub(crate) fn plain_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= PATH_MAX
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'))
        })
}

/// A path for a log or a message: its printable characters only, cut short.
fn printable(path: &str) -> String {
    path.chars().filter(|c| !c.is_control()).take(120).collect()
}

/// The members' count, the central directory's offset and length, from the end record (or its zip64 record).
fn central_directory(file: &mut File, len: u64) -> Result<(u64, u64, u64), Refused> {
    if len < EOCD_LEN {
        return Err("the package's file is not a zip".to_string());
    }
    let tail_len = len.min(EOCD_LEN + COMMENT_MAX);
    let tail_at = len - tail_len;
    let mut tail = vec![0u8; usize::try_from(tail_len).map_err(|_| "the zip's end is too large")?];
    read_at(file, tail_at, &mut tail)?;
    let at = (0..=tail.len() - 22)
        .rev()
        .find(|&at| u32_at(&tail, at) == EOCD_SIG)
        .ok_or("the package's file is not a zip")?;
    let record = &tail[at..];
    let mut count = u64::from(u16_at(record, 10));
    let mut central_len = u64::from(u32_at(record, 12));
    let mut central_at = u64::from(u32_at(record, 16));
    // zip64: the locator just before the end record names the zip64 record, whose fields are the real ones
    let record_at = tail_at + at as u64;
    if (count == 0xFFFF || central_len == u64::from(u32::MAX) || central_at == u64::from(u32::MAX)) && record_at >= 20 {
        let mut locator = [0u8; 20];
        read_at(file, record_at - 20, &mut locator)?;
        if u32_at(&locator, 0) == EOCD64_LOCATOR_SIG {
            let zip64_at = u64_at(&locator, 8);
            let mut zip64 = [0u8; 56];
            if zip64_at.checked_add(56).is_none_or(|end| end > len) {
                return Err("the package's zip64 end is outside the file".to_string());
            }
            read_at(file, zip64_at, &mut zip64)?;
            if u32_at(&zip64, 0) != EOCD64_SIG {
                return Err("the package's zip64 end is not one".to_string());
            }
            count = u64_at(&zip64, 32);
            central_len = u64_at(&zip64, 40);
            central_at = u64_at(&zip64, 48);
        }
    }
    Ok((count, central_at, central_len))
}

/// Where a member's bytes start: after its local header, its name and its extra field.
fn data_start(file: &mut File, local_at: u64, len: u64, path: &str) -> Result<u64, Refused> {
    if local_at.checked_add(LOCAL_LEN as u64).is_none_or(|end| end > len) {
        return Err(format!("{path} lies outside the package's file"));
    }
    let mut local = [0u8; LOCAL_LEN];
    read_at(file, local_at, &mut local)?;
    if u32_at(&local, 0) != LOCAL_SIG {
        return Err(format!("{path} has no local header"));
    }
    let name_len = u64::from(u16_at(&local, 26));
    let extra_len = u64::from(u16_at(&local, 28));
    Ok(local_at + LOCAL_LEN as u64 + name_len + extra_len)
}

/// The values of a zip64 extended information field in `extra`, in order.
fn zip64_values(extra: &[u8]) -> Option<impl Iterator<Item = u64> + '_> {
    let mut at = 0usize;
    while at + 4 <= extra.len() {
        let id = u16_at(extra, at);
        let field_len = usize::from(u16_at(extra, at + 2));
        let body = extra.get(at + 4..at + 4 + field_len)?;
        if id == ZIP64_EXTRA {
            return Some(body.as_chunks::<8>().0.iter().map(|chunk| u64::from_le_bytes(*chunk)));
        }
        at += 4 + field_len;
    }
    None
}

fn read_at(file: &mut File, at: u64, buffer: &mut [u8]) -> Result<(), Refused> {
    file.seek(SeekFrom::Start(at))
        .and_then(|_| file.read_exact(buffer))
        .map_err(|err| format!("the package's file could not be read: {err}"))
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(value)
}
