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

//! Bytes read from anywhere in them, a window at a time: the content of a file, of a blob in the database
//! (`super::blob`), of a git object. A response streams one, and a `Range` request reads only the bytes it asks for.
//! A source is one version of its content: its length and tag do not change while it is read.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use moqui_lib::error::Error;

pub trait ByteSource: Send + Sync {
    /// The size in bytes.
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A strong validator of this content (an `ETag` without its quotes), when the source has one without reading
    /// the content: a content hash it is stored by, or the id of the version.
    fn tag(&self) -> Option<String> {
        None
    }

    /// Up to `max` bytes from `offset`: fewer only at the end, none past it.
    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Error>;

    /// Whether reading needs the server's execution context (the database), so a host reads it on a worker that has
    /// one rather than on any thread.
    fn needs_context(&self) -> bool {
        false
    }
}

/// Bytes in memory.
pub struct MemorySource {
    data: Arc<Vec<u8>>,
    tag: Option<String>,
}

impl MemorySource {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data: Arc::new(data),
            tag: None,
        }
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }
}

impl ByteSource for MemorySource {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn tag(&self) -> Option<String> {
        self.tag.clone()
    }

    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Error> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(self.data.len());
        let end = start.saturating_add(max).min(self.data.len());
        Ok(self.data[start..end].to_vec())
    }
}

/// A file on disk, at the length it had when opened.
pub struct FileSource {
    path: PathBuf,
    len: u64,
    tag: Option<String>,
}

impl FileSource {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, Error> {
        let path = path.into();
        let metadata =
            std::fs::metadata(&path).map_err(|err| Error::new(format!("cannot read {}: {err}", path.display())))?;
        Ok(Self {
            path,
            len: metadata.len(),
            tag: None,
        })
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn tag(&self) -> Option<String> {
        self.tag.clone()
    }

    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Error> {
        let io = |err: std::io::Error| Error::new(format!("cannot read {}: {err}", self.path.display()));
        if offset >= self.len {
            return Ok(Vec::new());
        }
        let want = usize::try_from((self.len - offset).min(max as u64)).unwrap_or(max);
        let mut file = std::fs::File::open(&self.path).map_err(io)?;
        file.seek(SeekFrom::Start(offset)).map_err(io)?;
        let mut buf = Vec::with_capacity(want);
        file.take(want as u64).read_to_end(&mut buf).map_err(io)?;
        Ok(buf)
    }
}

/// Content read whole on first use, for storage that cannot read part of a file: nothing is read for a `HEAD`, a 304,
/// or a response the client does not take.
pub struct LazySource {
    len: u64,
    tag: Option<String>,
    needs_context: bool,
    load: Box<dyn Fn() -> Result<Vec<u8>, Error> + Send + Sync>,
    data: std::sync::OnceLock<Result<Arc<Vec<u8>>, String>>,
}

impl LazySource {
    /// `len` bytes that `load` reads; `needs_context` when it reads the database.
    pub fn new(
        len: u64,
        needs_context: bool,
        load: impl Fn() -> Result<Vec<u8>, Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            len,
            tag: None,
            needs_context,
            load: Box::new(load),
            data: std::sync::OnceLock::new(),
        }
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }
}

impl ByteSource for LazySource {
    fn len(&self) -> u64 {
        self.len
    }

    fn tag(&self) -> Option<String> {
        self.tag.clone()
    }

    fn read_at(&self, offset: u64, max: usize) -> Result<Vec<u8>, Error> {
        let data = self
            .data
            .get_or_init(|| (self.load)().map(Arc::new).map_err(|err| err.message().to_string()))
            .as_ref()
            .map_err(|message| Error::new(message.clone()))?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(data.len());
        let end = start.saturating_add(max).min(data.len());
        Ok(data[start..end].to_vec())
    }

    fn needs_context(&self) -> bool {
        self.needs_context
    }
}

/// A [`ByteSource`] as a [`Read`], from `offset` to the end, a window at a time.
pub struct SourceReader {
    source: Arc<dyn ByteSource>,
    offset: u64,
    window: usize,
    buffer: Vec<u8>,
    at: usize,
}

/// The bytes a reader takes from its source at a time.
pub const READ_WINDOW: usize = 1024 * 1024;

impl SourceReader {
    pub fn new(source: Arc<dyn ByteSource>) -> Self {
        Self::from(source, 0)
    }

    pub fn from(source: Arc<dyn ByteSource>, offset: u64) -> Self {
        Self {
            source,
            offset,
            window: READ_WINDOW,
            buffer: Vec::new(),
            at: 0,
        }
    }
}

impl Read for SourceReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.buffer.len() {
            if self.offset >= self.source.len() {
                return Ok(0);
            }
            self.buffer = self
                .source
                .read_at(self.offset, self.window)
                .map_err(|err| std::io::Error::other(err.message().to_string()))?;
            self.at = 0;
            self.offset += self.buffer.len() as u64;
            if self.buffer.is_empty() {
                return Ok(0);
            }
        }
        let count = buf.len().min(self.buffer.len() - self.at);
        buf[..count].copy_from_slice(&self.buffer[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}

/// All of a source, for one small enough to hold.
pub fn read_all(source: &dyn ByteSource) -> Result<Vec<u8>, Error> {
    let len = usize::try_from(source.len()).map_err(|_| Error::new("too large to read into memory"))?;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let window = source.read_at(out.len() as u64, READ_WINDOW.max(len - out.len()).min(64 * 1024 * 1024))?;
        if window.is_empty() {
            return Err(Error::new(format!("the source ended at {} of {len} bytes", out.len())));
        }
        out.extend_from_slice(&window);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_read_windows_and_stop_at_the_end() {
        let data: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        let memory: Arc<dyn ByteSource> = Arc::new(MemorySource::new(data.clone()).with_tag("t1"));
        assert_eq!(memory.read_at(2990, 100).unwrap(), data[2990..]);
        assert!(memory.read_at(5000, 10).unwrap().is_empty());
        assert_eq!(memory.tag().as_deref(), Some("t1"));
        let path = std::env::temp_dir().join(format!("moqui-source-{}", std::process::id()));
        std::fs::write(&path, &data).unwrap();
        let file: Arc<dyn ByteSource> = Arc::new(FileSource::open(&path).unwrap());
        assert_eq!(file.len(), 3000);
        assert_eq!(file.read_at(100, 50).unwrap(), data[100..150]);
        let mut reader = SourceReader::from(Arc::clone(&file), 1000);
        reader.window = 7;
        let mut read = Vec::new();
        reader.read_to_end(&mut read).unwrap();
        assert_eq!(read, data[1000..]);
        assert_eq!(read_all(file.as_ref()).unwrap(), data);
        let _ = std::fs::remove_file(&path);
    }
}
