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

//! The delimited-text reader of the Geo data parsers (plans/archive/Maps.md §6.4): a bare file, or one member of a zip read as
//! it inflates, within [`Limits`] (the bytes it may give, the ratio of inflated to compressed bytes, and the entries
//! a zip may have), so a zip bomb or a runaway line ends the read; and its [`Lines`], with a leading UTF-8 byte order
//! mark taken off, each line bounded and checked as UTF-8 on its own.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::Error;

use super::RowError;

/// The local file header signature that starts a zip.
const ZIP_MAGIC: [u8; 4] = *b"PK\x03\x04";
/// Inflated bytes allowed past the ratio, so a small member of very regular text is not refused.
const RATIO_SLACK: u64 = 1 << 20;
/// The longest line read; a longer one is a rejected row and the read goes on at the next line.
pub const MAX_LINE: usize = 64 * 1024;

/// How much a file may give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes read from a bare file, or inflated from a zip member (`maps@import-max-bytes`).
    pub max_bytes: u64,
    /// The most inflated bytes for each compressed byte of a member, past a small slack.
    pub max_ratio: u64,
    /// The most entries a zip may have.
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 512 << 20,
            max_ratio: 100,
            max_entries: 1000,
        }
    }
}

fn file_error(path: &Path, err: std::io::Error) -> Error {
    Error::new(format!("could not read {}", path.display())).with_source(err)
}

/// Read the file at `path` as text: the zip member `member` when the file is a zip (its only file entry when `member`
/// is None), else the file itself; `body` reads it.
///
/// # Errors
///
/// The file does not open, the zip does not read, has too many entries or not the member, the member's declared size
/// or ratio is over the limits, or `body`'s error (which includes a limit passed while reading).
pub fn read_file<T>(
    path: &Path,
    member: Option<&str>,
    limits: &Limits,
    body: impl FnOnce(&mut dyn BufRead) -> Result<T, Error>,
) -> Result<T, Error> {
    let file = std::fs::File::open(path).map_err(|err| file_error(path, err))?;
    read_source(file, &path.display().to_string(), member, limits, body)
}

/// [`read_file`] of `source`, named `name` in errors.
///
/// # Errors
///
/// As [`read_file`].
pub fn read_source<S: Read + Seek, T>(
    mut source: S,
    name: &str,
    member: Option<&str>,
    limits: &Limits,
    body: impl FnOnce(&mut dyn BufRead) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut head = [0_u8; 4];
    let read = read_head(&mut source, &mut head).map_err(|err| read_error(name, err))?;
    let length = source.seek(SeekFrom::End(0)).map_err(|err| read_error(name, err))?;
    source.seek(SeekFrom::Start(0)).map_err(|err| read_error(name, err))?;
    if read == head.len() && head == ZIP_MAGIC {
        return read_zip_member(source, length, name, member, limits, body);
    }
    let bounded = Bounded {
        inner: source,
        read: 0,
        max: limits.max_bytes,
        ratio_max: None,
    };
    body(&mut BufReader::with_capacity(64 * 1024, bounded))
}

fn read_head(source: &mut impl Read, head: &mut [u8]) -> std::io::Result<usize> {
    let mut read = 0;
    while read < head.len() {
        match source.read(&mut head[read..])? {
            0 => break,
            count => read += count,
        }
    }
    Ok(read)
}

fn read_error(name: &str, err: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::new(format!("could not read {name}")).with_source(err)
}

/// The zip crate reads the whole central directory when the archive is opened, so the entry count is checked just
/// after; the directory itself is bounded by the file, which the download already capped.
fn read_zip_member<S: Read + Seek, T>(
    source: S,
    length: u64,
    name: &str,
    member: Option<&str>,
    limits: &Limits,
    body: impl FnOnce(&mut dyn BufRead) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut archive = zip::ZipArchive::new(source).map_err(|err| read_error(name, err))?;
    if archive.len() > limits.max_entries {
        return Err(Error::new(format!(
            "{name} has {} entries, more than the {} allowed",
            archive.len(),
            limits.max_entries
        )));
    }
    let index = match member {
        Some(member) => archive
            .index_for_name(member)
            .ok_or_else(|| Error::new(format!("{name} has no member {member}")))?,
        None => only_file_entry(&mut archive, name)?,
    };
    let entry = archive.by_index(index).map_err(|err| read_error(name, err))?;
    if entry.is_dir() {
        return Err(Error::new(format!("member {index} of {name} is a directory")));
    }
    let declared = entry.size();
    // the declared compressed size is the sender's word too: the file's own length bounds it
    let compressed = entry.compressed_size().min(length);
    if declared > limits.max_bytes {
        return Err(Error::new(format!(
            "a member of {name} inflates to {declared} bytes, more than the {} allowed",
            limits.max_bytes
        )));
    }
    let ratio_max = compressed.saturating_mul(limits.max_ratio).saturating_add(RATIO_SLACK);
    if declared > ratio_max {
        return Err(Error::new(format!(
            "a member of {name} inflates {compressed} bytes to {declared}, past the ratio allowed"
        )));
    }
    let bounded = Bounded {
        inner: entry,
        read: 0,
        max: limits.max_bytes,
        ratio_max: Some(ratio_max),
    };
    let mut reader = BufReader::with_capacity(64 * 1024, bounded);
    body(&mut reader)
}

/// The index of a zip's only file entry.
fn only_file_entry<S: Read + Seek>(archive: &mut zip::ZipArchive<S>, name: &str) -> Result<usize, Error> {
    let mut found = None;
    for index in 0..archive.len() {
        let is_dir = archive
            .by_index_raw(index)
            .map_err(|err| read_error(name, err))?
            .is_dir();
        if is_dir {
            continue;
        }
        if found.is_some() {
            return Err(Error::new(format!(
                "{name} has more than one file and names none to read"
            )));
        }
        found = Some(index);
    }
    found.ok_or_else(|| Error::new(format!("{name} has no file")))
}

/// A reader that fails once it has given more than `max` bytes, or more than `ratio_max` (the compressed size times
/// the ratio, the declared size of a member being the sender's word only).
struct Bounded<R> {
    inner: R,
    read: u64,
    max: u64,
    ratio_max: Option<u64>,
}

impl<R: Read> Read for Bounded<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buffer)?;
        self.read = self.read.saturating_add(count as u64);
        if self.read > self.max {
            return Err(std::io::Error::other(format!(
                "the file is larger than the {} bytes allowed",
                self.max
            )));
        }
        if self.ratio_max.is_some_and(|ratio_max| self.read > ratio_max) {
            return Err(std::io::Error::other("the member inflates past the ratio allowed"));
        }
        Ok(count)
    }
}

/// One line read by [`Lines`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// A line's number (from 1) and its text, with the line ending (and on the first line a byte order mark) taken
    /// off.
    Text(u64, String),
    /// A line that is too long or not UTF-8, passed over.
    Rejected(RowError),
}

/// The lines of a reader.
pub struct Lines<'a> {
    reader: &'a mut dyn BufRead,
    number: u64,
    buffer: Vec<u8>,
}

impl<'a> Lines<'a> {
    pub fn new(reader: &'a mut dyn BufRead) -> Self {
        Self {
            reader,
            number: 0,
            buffer: Vec::new(),
        }
    }

    /// The next line; None at the end.
    ///
    /// # Errors
    ///
    /// An I/O error, or a limit of the reader passed: the file is not whole.
    pub fn next_line(&mut self) -> Result<Option<Line>, Error> {
        self.buffer.clear();
        let mut too_long = false;
        let mut any = false;
        loop {
            let available = self
                .reader
                .fill_buf()
                .map_err(|err| Error::new(format!("could not read line {}", self.number + 1)).with_source(err))?;
            if available.is_empty() {
                break;
            }
            any = true;
            let (taken, ended) = match available.iter().position(|byte| *byte == b'\n') {
                Some(at) => (at + 1, true),
                None => (available.len(), false),
            };
            if !too_long {
                let room = MAX_LINE.saturating_sub(self.buffer.len());
                if taken > room + usize::from(ended) {
                    too_long = true;
                    // the line's start is kept for its rejection: its first columns name what it was about
                    self.buffer.extend_from_slice(available.get(..room).unwrap_or_default());
                } else {
                    self.buffer.extend_from_slice(&available[..taken]);
                }
            }
            self.reader.consume(taken);
            if ended {
                break;
            }
        }
        if !any {
            return Ok(None);
        }
        self.number += 1;
        let number = self.number;
        if too_long {
            return Ok(Some(Line::Rejected(RowError::new(
                number,
                "",
                format!("the line is longer than {MAX_LINE} bytes"),
                &String::from_utf8_lossy(&self.buffer),
            ))));
        }
        while matches!(self.buffer.last(), Some(b'\n' | b'\r')) {
            self.buffer.pop();
        }
        let bytes = if number == 1 {
            self.buffer.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&self.buffer)
        } else {
            &self.buffer
        };
        Ok(Some(match std::str::from_utf8(bytes) {
            Ok(text) => Line::Text(number, text.to_string()),
            Err(_) => Line::Rejected(RowError::new(
                number,
                "",
                "the line is not UTF-8 text",
                &String::from_utf8_lossy(bytes),
            )),
        }))
    }
}
