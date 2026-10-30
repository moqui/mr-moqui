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

//! `multipart/form-data` bodies (RFC 7578): text fields and uploaded files.
//!
//! [`MultipartReader`] reads a body as it arrives, a chunk at a time: the host feeds it from the request stream, so an
//! upload is never all in memory. A file part is kept in memory up to [`Spool::memory_limit`], and past that in a file
//! of its own in [`Spool::dir`] (removed when the last reference to it goes), its size, SHA-256 and first bytes
//! (for the program check) taken on the way. A file part is an [`UploadedFile`], the value a form `file` widget's
//! field holds in `ec.web.multiPartParameters`. [`parse`] reads a body already in memory.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_lib::object::{HostObject, Value};
use sha2::{Digest, Sha256};

use crate::resource::source::{ByteSource, FileSource, MemorySource};

const MAX_PARTS: usize = 2000;
const MAX_PART_HEADERS: usize = 16 * 1024;

/// The first bytes of a file the program check looks at.
const HEAD_BYTES: usize = 16;

/// Windows PE, Linux ELF, Java class, macOS Mach-O and byte-swapped Mach-O.
const EXECUTABLE_MAGIC: [&[u8]; 7] = [
    b"MZ",
    &[0x7f, 0x45, 0x4c, 0x46],
    &[0xca, 0xfe, 0xba, 0xbe],
    &[0xfe, 0xed, 0xfa, 0xce],
    &[0xfe, 0xed, 0xfa, 0xcf],
    &[0xce, 0xfa, 0xed, 0xfe],
    &[0xcf, 0xfa, 0xed, 0xfe],
];

/// An uploaded file: the part's form field, its file name and type, and its content.
#[derive(Clone, Debug)]
pub struct UploadedFile {
    pub field_name: String,
    pub file_name: String,
    pub content_type: String,
    content: Content,
    size: u64,
    head: Vec<u8>,
    sha256: Option<String>,
}

#[derive(Clone, Debug)]
enum Content {
    Memory(Arc<[u8]>),
    Spooled(Arc<SpoolFile>),
}

/// A file part's content on disk, removed with the last reference to it.
#[derive(Debug)]
struct SpoolFile(PathBuf);

impl Drop for SpoolFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl UploadedFile {
    /// A file whose content is `bytes`.
    #[must_use]
    pub fn in_memory(field_name: &str, file_name: &str, content_type: &str, bytes: Vec<u8>) -> Self {
        Self {
            field_name: field_name.to_string(),
            file_name: file_name.to_string(),
            content_type: content_type.to_string(),
            size: bytes.len() as u64,
            head: bytes[..bytes.len().min(HEAD_BYTES)].to_vec(),
            sha256: None,
            content: Content::Memory(bytes.into()),
        }
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Whether the content is in memory (not spooled to a file).
    #[must_use]
    pub fn is_in_memory(&self) -> bool {
        matches!(self.content, Content::Memory(_))
    }

    /// The SHA-256 of the content in hex, taken as a spooled file arrived; worked out for one in memory.
    #[must_use]
    pub fn sha256(&self) -> String {
        match (&self.sha256, &self.content) {
            (Some(hash), _) => hash.clone(),
            (None, Content::Memory(bytes)) => crate::resource::blob::blob_id(bytes),
            (None, Content::Spooled(_)) => String::new(),
        }
    }

    /// All of the content, read from its file when it is spooled.
    ///
    /// # Errors
    ///
    /// Returns an error when the spool file cannot be read.
    pub fn read_bytes(&self) -> Result<Vec<u8>, moqui_lib::error::Error> {
        match &self.content {
            Content::Memory(bytes) => Ok(bytes.to_vec()),
            Content::Spooled(file) => std::fs::read(&file.0).map_err(|err| {
                moqui_lib::error::Error::new(format!("cannot read the upload {}: {err}", self.file_name))
            }),
        }
    }

    /// The content to read a window at a time.
    ///
    /// # Errors
    ///
    /// Returns an error when the spool file is gone.
    pub fn source(&self) -> Result<Arc<dyn ByteSource>, moqui_lib::error::Error> {
        Ok(match &self.content {
            Content::Memory(bytes) => Arc::new(MemorySource::new(bytes.to_vec())),
            Content::Spooled(file) => Arc::new(SpooledSource {
                file: FileSource::open(&file.0)?,
                _keep: Arc::clone(file),
            }),
        })
    }

    /// The content as a stream.
    ///
    /// # Errors
    ///
    /// Returns an error when the spool file cannot be opened.
    pub fn reader(&self) -> Result<Box<dyn Read + Send>, moqui_lib::error::Error> {
        Ok(match &self.content {
            Content::Memory(bytes) => Box::new(std::io::Cursor::new(Arc::clone(bytes))),
            Content::Spooled(file) => Box::new(SpooledReader {
                file: std::fs::File::open(&file.0).map_err(|err| {
                    moqui_lib::error::Error::new(format!("cannot read the upload {}: {err}", self.file_name))
                })?,
                _keep: Arc::clone(file),
            }),
        })
    }

    /// Whether the first bytes are those of a program (see `WebUtilities.isExecutable`).
    #[must_use]
    pub fn is_executable(&self) -> bool {
        is_executable(&self.head)
    }

    #[must_use]
    pub fn into_value(self) -> Value {
        Value::Object(HostObject::new(self))
    }
}

/// A spooled file as a source, which keeps the file while it is read.
struct SpooledSource {
    file: FileSource,
    _keep: Arc<SpoolFile>,
}

impl ByteSource for SpooledSource {
    fn len(&self) -> u64 {
        self.file.len()
    }

    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, moqui_lib::error::Error> {
        self.file.read_at(offset, max)
    }
}

/// A spooled file as a stream, which keeps the file while it is read.
struct SpooledReader {
    file: std::fs::File,
    _keep: Arc<SpoolFile>,
}

impl Read for SpooledReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

/// A shebang or one of the program headers. A ZIP is not a program.
#[must_use]
pub fn is_executable(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#!") || EXECUTABLE_MAGIC.iter().any(|magic| bytes.starts_with(magic))
}

/// The parts of a body, in order.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub fields: Vec<(String, String)>,
    pub files: Vec<UploadedFile>,
}

/// Why a body could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum MultipartError {
    /// No boundary in the content type, or the body does not follow it.
    Malformed(String),
    /// More parts than the parser allows.
    TooManyParts,
    /// A text field over the field limit.
    FieldTooLarge(String),
    /// A spool file could not be written.
    Io(String),
}

impl std::fmt::Display for MultipartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(why) => write!(f, "multipart body is not valid: {why}"),
            Self::TooManyParts => write!(f, "multipart body has more than {MAX_PARTS} parts"),
            Self::FieldTooLarge(name) => write!(f, "multipart field {name} is too large"),
            Self::Io(why) => write!(f, "multipart body could not be stored: {why}"),
        }
    }
}

/// The boundary from a `Content-Type` value, with or without quotes.
#[must_use]
pub fn boundary_of(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let (key, value) = part.trim().split_once('=')?;
        if key.trim().eq_ignore_ascii_case("boundary") {
            let value = value.trim().trim_matches('"');
            return Some(value.to_string()).filter(|text| !text.is_empty() && text.len() <= 200);
        }
    }
    None
}

/// Read every part of `body`, all in memory. Text fields are decoded as UTF-8, with replacement for bad bytes.
///
/// # Errors
///
/// Returns an error when there is no boundary, a part has no header end, or the closing boundary is missing.
pub fn parse(content_type: &str, body: &[u8]) -> Result<Parsed, MultipartError> {
    let mut reader = MultipartReader::new(content_type, None)?;
    reader.feed(body)?;
    reader.finish()
}

/// Where file parts go past a size, and how large a text field may be.
#[derive(Clone, Debug)]
pub struct Spool {
    /// The directory spool files are made in.
    pub dir: PathBuf,
    /// Bytes of a file part kept in memory; more goes to a spool file.
    pub memory_limit: usize,
    /// Bytes of a text field.
    pub field_limit: usize,
}

impl Spool {
    /// `runtime/tmp/upload` of `runtime_dir`: 256 KiB of a file in memory, fields of up to 1 MiB.
    #[must_use]
    pub fn under(runtime_dir: &Path) -> Self {
        Self {
            dir: runtime_dir.join("tmp").join("upload"),
            memory_limit: 256 * 1024,
            field_limit: 1024 * 1024,
        }
    }

    /// Remove what earlier runs left in the spool directory.
    pub fn sweep(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

enum State {
    /// Before the first boundary.
    Preamble,
    /// Just after a boundary: its end, or the closing `--`.
    Boundary,
    /// The headers of a part; the buffer starts at the boundary line's own line break.
    Headers,
    /// The data of a part; `skip` bytes at the start of the buffer are the headers' last line break.
    Data {
        skip: usize,
    },
    Done,
}

/// A part being read.
struct Part {
    name: String,
    /// `Some` for a file part, with its file name and type.
    file: Option<(String, String)>,
    memory: Vec<u8>,
    spooled: Option<(std::fs::File, PathBuf)>,
    size: u64,
    hasher: Sha256,
    head: Vec<u8>,
}

/// A `multipart/form-data` body read a chunk at a time ([`MultipartReader::feed`]), then [`MultipartReader::finish`].
pub struct MultipartReader {
    /// `--boundary`.
    delimiter: Vec<u8>,
    /// `\r\n--boundary`, which ends a part's data.
    data_end: Vec<u8>,
    state: State,
    buffer: Vec<u8>,
    parsed: Parsed,
    part: Option<Part>,
    count: usize,
    spool: Option<Spool>,
}

static SPOOL_FILES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    memchr::memmem::find(&haystack[from..], needle).map(|at| at + from)
}

impl MultipartReader {
    /// A reader of a body of `content_type`, spooling large file parts per `spool` (all in memory without one).
    ///
    /// # Errors
    ///
    /// Returns an error when the content type names no boundary.
    pub fn new(content_type: &str, spool: Option<Spool>) -> Result<Self, MultipartError> {
        let boundary = boundary_of(content_type)
            .ok_or_else(|| MultipartError::Malformed("no boundary in the content type".into()))?;
        let delimiter = format!("--{boundary}").into_bytes();
        let mut data_end = b"\r\n".to_vec();
        data_end.extend_from_slice(&delimiter);
        Ok(Self {
            delimiter,
            data_end,
            state: State::Preamble,
            buffer: Vec::new(),
            parsed: Parsed::default(),
            part: None,
            count: 0,
            spool,
        })
    }

    /// Read the next bytes of the body.
    ///
    /// # Errors
    ///
    /// Returns an error when the body does not follow its boundary, has too many parts, or a spool file cannot be
    /// written.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), MultipartError> {
        if matches!(self.state, State::Done) {
            return Ok(());
        }
        self.buffer.extend_from_slice(bytes);
        while self.step()? {}
        Ok(())
    }

    /// One step of the state machine on what is buffered: false when it needs more bytes.
    fn step(&mut self) -> Result<bool, MultipartError> {
        match self.state {
            State::Preamble => {
                let Some(at) = find(&self.buffer, &self.delimiter, 0) else {
                    // keep what may be the start of the delimiter
                    let keep = self.delimiter.len().saturating_sub(1).min(self.buffer.len());
                    let drop = self.buffer.len() - keep;
                    self.buffer.drain(..drop);
                    return Ok(false);
                };
                self.buffer.drain(..at + self.delimiter.len());
                self.state = State::Boundary;
                Ok(true)
            }
            State::Boundary => {
                if self.buffer.len() < 2 {
                    return Ok(false);
                }
                if self.buffer.starts_with(b"--") {
                    self.state = State::Done;
                    self.buffer.clear();
                    return Ok(false);
                }
                // the rest of the boundary line is white space, then the line break
                let Some(line_end) = find(&self.buffer, b"\r\n", 0) else {
                    if self.buffer.iter().any(|byte| !matches!(byte, b' ' | b'\t' | b'\r')) {
                        return Err(MultipartError::Malformed("text follows a boundary".into()));
                    }
                    return Ok(false);
                };
                if self.buffer[..line_end].iter().any(|byte| !matches!(byte, b' ' | b'\t')) {
                    return Err(MultipartError::Malformed("text follows a boundary".into()));
                }
                self.buffer.drain(..line_end);
                self.state = State::Headers;
                Ok(true)
            }
            State::Headers => {
                let Some(end) = find(&self.buffer, b"\r\n\r\n", 0) else {
                    if self.buffer.len() > MAX_PART_HEADERS + 4 {
                        return Err(MultipartError::Malformed("part headers are too large".into()));
                    }
                    return Ok(false);
                };
                let headers = if end < 2 {
                    Vec::new()
                } else {
                    self.buffer[2..end].to_vec()
                };
                if headers.len() > MAX_PART_HEADERS {
                    return Err(MultipartError::Malformed("part headers are too large".into()));
                }
                self.count += 1;
                if self.count > MAX_PARTS {
                    return Err(MultipartError::TooManyParts);
                }
                self.part = Some(Self::start_part(&headers)?);
                // the headers' last line break may be the line break of the next delimiter (an empty part)
                self.buffer.drain(..end + 2);
                self.state = State::Data { skip: 2 };
                Ok(true)
            }
            State::Data { skip } => match find(&self.buffer, &self.data_end, 0) {
                Some(at) => {
                    let data = self.buffer[skip.min(at)..at].to_vec();
                    self.write_data(&data)?;
                    self.end_part()?;
                    self.buffer.drain(..at + self.data_end.len());
                    self.state = State::Boundary;
                    Ok(true)
                }
                None => {
                    // all but what may be the start of the next delimiter
                    let keep = (self.data_end.len() - 1).max(skip);
                    if self.buffer.len() <= keep {
                        return Ok(false);
                    }
                    let until = self.buffer.len() - keep;
                    let data = self.buffer[skip.min(until)..until].to_vec();
                    self.write_data(&data)?;
                    self.buffer.drain(..until);
                    self.state = State::Data {
                        skip: skip.saturating_sub(until),
                    };
                    Ok(false)
                }
            },
            State::Done => Ok(false),
        }
    }

    fn start_part(headers: &[u8]) -> Result<Part, MultipartError> {
        let text = String::from_utf8_lossy(headers);
        let mut name = None;
        let mut file_name = None;
        let mut content_type = String::new();
        for line in text.split("\r\n") {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            if key.trim().eq_ignore_ascii_case("content-disposition") {
                name = disposition_param(value, "name");
                file_name = disposition_param(value, "filename");
            } else if key.trim().eq_ignore_ascii_case("content-type") {
                content_type = value.trim().to_string();
            }
        }
        let name = name.ok_or_else(|| MultipartError::Malformed("a part has no name".into()))?;
        Ok(Part {
            name,
            file: file_name.map(|file_name| {
                let content_type = if content_type.is_empty() {
                    "application/octet-stream".to_string()
                } else {
                    content_type
                };
                (file_name, content_type)
            }),
            memory: Vec::new(),
            spooled: None,
            size: 0,
            hasher: Sha256::new(),
            head: Vec::new(),
        })
    }

    fn write_data(&mut self, data: &[u8]) -> Result<(), MultipartError> {
        let spool = self.spool.clone();
        let Some(part) = self.part.as_mut() else {
            return Ok(());
        };
        if data.is_empty() {
            return Ok(());
        }
        part.size += data.len() as u64;
        if part.file.is_none() {
            if spool
                .as_ref()
                .is_some_and(|spool| part.memory.len() + data.len() > spool.field_limit)
            {
                return Err(MultipartError::FieldTooLarge(part.name.clone()));
            }
            part.memory.extend_from_slice(data);
            return Ok(());
        }
        if part.head.len() < HEAD_BYTES {
            let take = (HEAD_BYTES - part.head.len()).min(data.len());
            part.head.extend_from_slice(&data[..take]);
        }
        part.hasher.update(data);
        let io = |err: std::io::Error| MultipartError::Io(err.to_string());
        if let Some((file, _)) = part.spooled.as_mut() {
            return file.write_all(data).map_err(io);
        }
        match spool {
            Some(spool) if part.memory.len() + data.len() > spool.memory_limit => {
                std::fs::create_dir_all(&spool.dir).map_err(io)?;
                let path = spool.dir.join(format!(
                    "upload-{}-{}",
                    std::process::id(),
                    SPOOL_FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                ));
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
                let mut file = options.open(&path).map_err(io)?;
                file.write_all(&part.memory).map_err(io)?;
                file.write_all(data).map_err(io)?;
                part.memory = Vec::new();
                part.spooled = Some((file, path));
                Ok(())
            }
            _ => {
                part.memory.extend_from_slice(data);
                Ok(())
            }
        }
    }

    fn end_part(&mut self) -> Result<(), MultipartError> {
        let Some(part) = self.part.take() else {
            return Ok(());
        };
        match part.file {
            None => self
                .parsed
                .fields
                .push((part.name, String::from_utf8_lossy(&part.memory).into_owned())),
            // a file input left empty posts a part with an empty file name
            Some((file_name, _)) if file_name.is_empty() && part.size == 0 => {}
            Some((file_name, content_type)) => {
                let sha256: String = part
                    .hasher
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                let content = match part.spooled {
                    Some((mut file, path)) => {
                        // the file is removed with the last reference, from here on
                        let spooled = Arc::new(SpoolFile(path));
                        file.flush().map_err(|err| MultipartError::Io(err.to_string()))?;
                        Content::Spooled(spooled)
                    }
                    None => Content::Memory(part.memory.into()),
                };
                self.parsed.files.push(UploadedFile {
                    field_name: part.name,
                    file_name: base_name(&file_name),
                    content_type,
                    content,
                    size: part.size,
                    head: part.head,
                    sha256: Some(sha256),
                });
            }
        }
        Ok(())
    }

    /// The parts read, once the body has ended.
    ///
    /// # Errors
    ///
    /// Returns an error when the body ended before its closing boundary.
    pub fn finish(mut self) -> Result<Parsed, MultipartError> {
        match self.state {
            State::Done => Ok(std::mem::take(&mut self.parsed)),
            State::Preamble => Err(MultipartError::Malformed("no first boundary".into())),
            State::Boundary => Err(MultipartError::Malformed("a boundary line has no end".into())),
            State::Headers => Err(MultipartError::Malformed("a part has no end of headers".into())),
            State::Data { .. } => Err(MultipartError::Malformed("no closing boundary".into())),
        }
    }
}

impl Drop for MultipartReader {
    fn drop(&mut self) {
        // a part cut short leaves no spool file behind
        if let Some((_, path)) = self.part.take().and_then(|part| part.spooled) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A file name without the directories some browsers send.
fn base_name(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_string()
}

/// `name="x"` or `name=x` in a `Content-Disposition` value. A quoted value may hold `\"` and `\\`.
fn disposition_param(value: &str, wanted: &str) -> Option<String> {
    // A parameter starts after a semicolon.
    let (_, mut rest) = value.split_once(';')?;
    loop {
        let (key, after) = rest.trim_start().split_once('=')?;
        let key = key.trim();
        // what follows the value, where the next semicolon is looked for
        let next = if let Some(quoted) = after.strip_prefix('"') {
            let (parsed, after_quote) = unquote(quoted);
            if key.eq_ignore_ascii_case(wanted) {
                return Some(parsed);
            }
            after_quote
        } else {
            if key.eq_ignore_ascii_case(wanted) {
                let parsed = after.split_once(';').map_or(after, |(parsed, _)| parsed);
                return Some(parsed.trim().to_string());
            }
            after
        };
        (_, rest) = next.split_once(';')?;
    }
}

/// The text of a quoted value up to its closing quote (or the end), with `\"` and `\\` unescaped, and what follows it.
fn unquote(quoted: &str) -> (String, &str) {
    let mut out = String::new();
    let mut chars = quoted.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.clone().next() {
                Some(escaped @ ('"' | '\\')) => {
                    chars.next();
                    out.push(escaped);
                }
                _ => out.push('\\'),
            },
            '"' => break,
            other => out.push(other),
        }
    }
    (out, chars.as_str())
}

#[cfg(test)]
#[path = "multipart_tests.rs"]
mod tests;
