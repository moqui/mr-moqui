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

//! A session's output kept as an asciicast v2 file (plans/Terminal-Fancy2.md §4.5, D2): a header line, then an `"o"`
//! event for each piece of output and an `"r"` for each resize, with its time. What is typed is never recorded, and the
//! terminal's queries (device attributes, cursor and status reports, the color queries) are left out, so a playback
//! answers nothing. The server's pump writes a session in process, the holder a held one, so a restart of the server
//! loses nothing. At its size limit a recording stops. In history mode (the `thread` kind, D10) it keeps rolling
//! segments of at most [`HISTORY_MAX`] of output in all, the oldest removed first, at code-point edges. Nothing here reads
//! the factory, so the holder runs it too.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The output a history keeps in all, and in one segment.
pub(crate) const HISTORY_MAX: u64 = 8 * 1024 * 1024;
const HISTORY_SEGMENT: u64 = 1024 * 1024;
/// The longest piece held back at the end of a read: an escape sequence or a character the next read finishes.
const CARRY_MAX: usize = 4096;

/// What a recorder keeps, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    /// One file, stopped at `limit` bytes of output.
    File { path: PathBuf, limit: u64 },
    /// Segments `<stem>-<n>.cast` in a directory, the oldest removed past [`HISTORY_MAX`].
    History { dir: PathBuf, stem: String },
}

impl Keep {
    /// As the holder's protocol carries it.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::File { path, limit } => serde_json::json!({"path": path.to_string_lossy(), "limit": limit}),
            Self::History { dir, stem } => serde_json::json!({"dir": dir.to_string_lossy(), "stem": stem}),
        }
    }

    /// From the holder's protocol; `None` for none, or for one that names nothing.
    pub fn of_json(value: &serde_json::Value) -> Option<Self> {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
        };
        if let Some(path) = text("path") {
            let limit = value
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(256 * 1024 * 1024);
            return Some(Self::File {
                path: PathBuf::from(path),
                limit,
            });
        }
        Some(Self::History {
            dir: PathBuf::from(text("dir")?),
            stem: text("stem")?.to_string(),
        })
    }
}

/// What a write came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wrote {
    Kept,
    /// The file reached its limit and was closed: the recording stopped.
    Full,
}

pub(crate) struct Recorder {
    keep: Keep,
    out: Option<BufWriter<File>>,
    started: Instant,
    cols: u16,
    rows: u16,
    /// The output kept in the open file (a file recording: in all).
    written: u64,
    /// History: the closed segments, oldest first, with their output.
    segments: VecDeque<(PathBuf, u64)>,
    next_segment: u64,
    carry: Vec<u8>,
}

/// Make `dir` and its parents, `0700` each.
fn make_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.is_dir() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// A new file at `path`, `0600`, with the asciicast header of a `cols` by `rows` terminal.
fn open_file(path: &Path, cols: u16, rows: u16) -> io::Result<BufWriter<File>> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        make_dir(dir)?;
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    let mut out = BufWriter::new(file);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let header = serde_json::json!({"version": 2, "width": cols, "height": rows, "timestamp": timestamp,
        "env": {"TERM": "xterm-256color"}});
    writeln!(out, "{header}")?;
    out.flush()?;
    Ok(out)
}

impl Recorder {
    /// Start keeping a session's output as `keep` says, the terminal `cols` by `rows` now.
    pub fn start(keep: Keep, cols: u16, rows: u16) -> io::Result<Self> {
        let mut recorder = Self {
            keep,
            out: None,
            started: Instant::now(),
            cols,
            rows,
            written: 0,
            segments: VecDeque::new(),
            next_segment: 1,
            carry: Vec::new(),
        };
        if let Keep::History { dir, stem } = &recorder.keep {
            // segments of a history kept before (a thread terminal started again): kept, and counted
            recorder.segments = history_segments(dir, stem);
            let number = |path: &PathBuf| {
                path.file_stem()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.rsplit('-').next())
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0)
            };
            recorder.next_segment = recorder
                .segments
                .iter()
                .map(|(path, _)| number(path))
                .max()
                .unwrap_or(0)
                + 1;
        }
        recorder.open()?;
        Ok(recorder)
    }

    fn path(&self) -> PathBuf {
        match &self.keep {
            Keep::File { path, .. } => path.clone(),
            Keep::History { dir, stem } => dir.join(format!("{stem}-{:06}.cast", self.next_segment)),
        }
    }

    fn open(&mut self) -> io::Result<()> {
        self.out = Some(open_file(&self.path(), self.cols, self.rows)?);
        self.written = 0;
        Ok(())
    }

    fn event(&mut self, kind: &str, data: &str) -> io::Result<()> {
        let at = self.started.elapsed().as_secs_f64();
        let line = serde_json::json!([(at * 1_000_000.0).round() / 1_000_000.0, kind, data]);
        match self.out.as_mut() {
            Some(out) => writeln!(out, "{line}"),
            None => Ok(()),
        }
    }

    /// The program's output: kept but for the terminal's queries, a character or sequence split at the end held for
    /// the next. `Full` once a file recording reached its limit, after which it keeps nothing.
    pub fn output(&mut self, bytes: &[u8]) -> io::Result<Wrote> {
        if self.out.is_none() {
            return Ok(Wrote::Full);
        }
        let mut raw = std::mem::take(&mut self.carry);
        raw.extend_from_slice(bytes);
        let (kept, rest) = strip_queries(&raw, false);
        let (text, more) = whole_chars(kept);
        self.carry = more;
        self.carry.extend_from_slice(rest);
        if self.carry.len() > CARRY_MAX {
            // not a sequence after all: kept as it is
            let carried = std::mem::take(&mut self.carry);
            return self.keep_text(&String::from_utf8_lossy(&carried));
        }
        if text.is_empty() {
            return Ok(Wrote::Kept);
        }
        self.keep_text(&text)
    }

    fn keep_text(&mut self, text: &str) -> io::Result<Wrote> {
        self.event("o", text)?;
        self.written += text.len() as u64;
        if let Some(out) = self.out.as_mut() {
            out.flush()?;
        }
        match self.keep.clone() {
            Keep::File { limit, .. } if self.written >= limit => {
                self.finish()?;
                Ok(Wrote::Full)
            }
            Keep::History { .. } if self.written >= HISTORY_SEGMENT => {
                self.next_segment_file()?;
                Ok(Wrote::Kept)
            }
            _ => Ok(Wrote::Kept),
        }
    }

    /// History: close the segment, open the next, and remove the oldest past [`HISTORY_MAX`] in all.
    fn next_segment_file(&mut self) -> io::Result<()> {
        if let Some(mut out) = self.out.take() {
            out.flush()?;
        }
        self.segments.push_back((self.path(), self.written));
        self.next_segment += 1;
        while self.segments.iter().map(|(_, bytes)| bytes).sum::<u64>() + HISTORY_SEGMENT > HISTORY_MAX {
            match self.segments.pop_front() {
                Some((oldest, _)) => {
                    let _ = std::fs::remove_file(oldest);
                }
                None => break,
            }
        }
        self.open()
    }

    /// The terminal's new size.
    pub fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        if (cols, rows) == (self.cols, self.rows) {
            return Ok(());
        }
        (self.cols, self.rows) = (cols, rows);
        self.event("r", &format!("{cols}x{rows}"))?;
        match self.out.as_mut() {
            Some(out) => out.flush(),
            None => Ok(()),
        }
    }

    /// Close the file (what is held back is written as it is).
    pub fn finish(&mut self) -> io::Result<()> {
        if !self.carry.is_empty() && self.out.is_some() {
            let carried = std::mem::take(&mut self.carry);
            self.event("o", &String::from_utf8_lossy(&carried))?;
        }
        match self.out.take() {
            Some(mut out) => out.flush(),
            None => Ok(()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// The segments of the history `stem` in `dir`, oldest first, with the size of each file.
fn history_segments(dir: &Path, stem: &str) -> VecDeque<(PathBuf, u64)> {
    let prefix = format!("{stem}-");
    let mut found: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with(&prefix) && name.ends_with(".cast")
        })
        .map(|entry| (entry.path(), entry.metadata().map_or(0, |meta| meta.len())))
        .collect();
    found.sort();
    found.into()
}

/// The output a history keeps, oldest first, as one stream (a thread terminal started again shows it first).
pub(crate) fn history_output(dir: &Path, stem: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (path, _) in history_segments(dir, stem) {
        out.extend(cast_output(&path));
    }
    out
}

/// The output of an asciicast file: its `"o"` events' data, in order.
pub(crate) fn cast_output(path: &Path) -> Vec<u8> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        if let Ok(serde_json::Value::Array(event)) = serde_json::from_str::<serde_json::Value>(line) {
            if event.get(1).and_then(serde_json::Value::as_str) == Some("o") {
                out.extend_from_slice(
                    event
                        .get(2)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .as_bytes(),
                );
            }
        }
    }
    out
}

/// Remove the history `stem` in `dir`.
pub(crate) fn remove_history(dir: &Path, stem: &str) {
    for (path, _) in history_segments(dir, stem) {
        let _ = std::fs::remove_file(path);
    }
}

/// `bytes` split at its last whole character: the text, and the bytes of a character the next read finishes. A byte
/// that can never be a character is kept as U+FFFD.
fn whole_chars(bytes: Vec<u8>) -> (String, Vec<u8>) {
    match String::from_utf8(bytes) {
        Ok(text) => (text, Vec::new()),
        Err(err) => {
            let bytes = err.into_bytes();
            let valid = std::str::from_utf8(&bytes)
                .err()
                .map_or(bytes.len(), |err| err.valid_up_to());
            let tail_incomplete = std::str::from_utf8(&bytes[valid..])
                .err()
                .is_some_and(|err| err.error_len().is_none());
            if tail_incomplete && bytes.len() - valid < 4 {
                let (head, tail) = bytes.split_at(valid);
                (String::from_utf8_lossy(head).into_owned(), tail.to_vec())
            } else {
                (String::from_utf8_lossy(&bytes).into_owned(), Vec::new())
            }
        }
    }
}

/// Whether the CSI sequence with parameters `params` and final byte `last` asks the terminal for an answer: device
/// attributes (`c`, `>c`, `=c`), a status or cursor report (`5n`, `6n`, `?6n`), the terminal's version (`>q`), a mode's
/// state (`$p`), the window's size or place (`14t`, `16t`, `18t`, `19t`, `13t`, `11t`, `21t`).
fn csi_query(params: &[u8], last: u8) -> bool {
    match last {
        b'c' => true,
        b'n' => matches!(params, b"5" | b"6" | b"?6" | b"?15" | b"?25" | b"?26"),
        b'q' => params.starts_with(b">"),
        b'p' => params.ends_with(b"$"),
        b't' => matches!(params, b"11" | b"13" | b"14" | b"16" | b"18" | b"19" | b"21"),
        _ => false,
    }
}

/// Whether an OSC with `payload` asks for an answer: a color query (`10;?`, `11;?`, `12;?`, `4;n;?`) or the
/// clipboard's contents (`52;c;?`).
fn osc_query(payload: &[u8]) -> bool {
    payload.ends_with(b";?")
        && (payload.starts_with(b"10;")
            || payload.starts_with(b"11;")
            || payload.starts_with(b"12;")
            || payload.starts_with(b"4;")
            || payload.starts_with(b"52;"))
}

/// `bytes` without the terminal's queries ([`csi_query`], [`osc_query`]): the bytes kept, and the end of `bytes` held
/// back when a sequence is not finished there (none when `whole`, where an unfinished sequence is kept as it is). The
/// ring's replay to a new window takes them out too: an answer to an old query, typed into the program, is garbage.
pub(crate) fn strip_queries(bytes: &[u8], whole: bool) -> (Vec<u8>, &[u8]) {
    let mut kept = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != 0x1b {
            kept.push(bytes[at]);
            at += 1;
            continue;
        }
        let start = at;
        let Some(&kind) = bytes.get(at + 1) else {
            if whole {
                kept.push(bytes[at]);
                at += 1;
                continue;
            }
            return (kept, &bytes[start..]);
        };
        match kind {
            b'[' => {
                let mut end = at + 2;
                while end < bytes.len() && !(0x40..=0x7e).contains(&bytes[end]) {
                    end += 1;
                }
                if end >= bytes.len() {
                    if whole {
                        kept.extend_from_slice(&bytes[start..]);
                        return (kept, &[]);
                    }
                    return (kept, &bytes[start..]);
                }
                if !csi_query(&bytes[at + 2..end], bytes[end]) {
                    kept.extend_from_slice(&bytes[start..=end]);
                }
                at = end + 1;
            }
            b']' => {
                let mut end = at + 2;
                let mut close = None;
                while end < bytes.len() {
                    if bytes[end] == 0x07 {
                        close = Some((end, end + 1));
                        break;
                    }
                    if bytes[end] == 0x1b && bytes.get(end + 1) == Some(&b'\\') {
                        close = Some((end, end + 2));
                        break;
                    }
                    end += 1;
                }
                let Some((payload_end, after)) = close else {
                    if whole {
                        kept.extend_from_slice(&bytes[start..]);
                        return (kept, &[]);
                    }
                    return (kept, &bytes[start..]);
                };
                if !osc_query(&bytes[at + 2..payload_end]) {
                    kept.extend_from_slice(&bytes[start..after]);
                }
                at = after;
            }
            _ => {
                kept.push(bytes[at]);
                at += 1;
            }
        }
    }
    (kept, &[])
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
