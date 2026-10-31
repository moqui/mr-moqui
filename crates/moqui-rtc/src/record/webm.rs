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

//! VP8 in WebM (Matroska's EBML), written in house: the call recorder's video files (plans/Comms-Framework.md §6.4,
//! §10.1). The encoded frames go in as they came, never transcoded: one video track, a Cluster at each keyframe (and at
//! least every 30 seconds), each frame a SimpleBlock with its keyframe flag, times in milliseconds. The file starts at
//! a keyframe; frames before the first are dropped. On finish the Segment's size, the Duration, the Cues (each
//! cluster's time and place, for seeking) and a SeekHead are filled in. A gap is a jump in the block times.

use std::io::{self, Seek, SeekFrom, Write};

const EBML: u32 = 0x1A45_DFA3;
const EBML_VERSION: u32 = 0x4286;
const EBML_READ_VERSION: u32 = 0x42F7;
const EBML_MAX_ID_LENGTH: u32 = 0x42F2;
const EBML_MAX_SIZE_LENGTH: u32 = 0x42F3;
const DOC_TYPE: u32 = 0x4282;
const DOC_TYPE_VERSION: u32 = 0x4287;
const DOC_TYPE_READ_VERSION: u32 = 0x4285;
pub(crate) const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const VOID: u32 = 0xEC;
pub(crate) const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const MUXING_APP: u32 = 0x4D80;
const WRITING_APP: u32 = 0x5741;
pub(crate) const DURATION: u32 = 0x4489;
pub(crate) const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_UID: u32 = 0x73C5;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
pub(crate) const CLUSTER: u32 = 0x1F43_B675;
pub(crate) const TIMECODE: u32 = 0xE7;
pub(crate) const SIMPLE_BLOCK: u32 = 0xA3;
pub(crate) const CUES: u32 = 0x1C53_BB6B;
const CUE_POINT: u32 = 0xBB;
const CUE_TIME: u32 = 0xB3;
const CUE_TRACK_POSITIONS: u32 = 0xB7;
const CUE_TRACK: u32 = 0xF7;
const CUE_CLUSTER_POSITION: u32 = 0xF1;

/// The room kept after the Segment's header for the SeekHead written at the end.
const SEEK_HEAD_ROOM: usize = 96;
/// A cluster's blocks are at most this far from its time (SimpleBlock times are 16-bit signed).
const MAX_CLUSTER_MS: u64 = 30_000;
/// A cluster is buffered in memory up to this size before it is written.
const MAX_CLUSTER_BYTES: usize = 8 * 1024 * 1024;

/// An element's ID, as its bytes (the length is in the ID's own first byte).
fn id_bytes(id: u32) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    bytes[skip..].to_vec()
}

/// An element size as an EBML variable-length integer, in its shortest form.
fn size_bytes(size: u64) -> Vec<u8> {
    let mut length = 1;
    while length < 8 && size >= (1u64 << (7 * length)) - 1 {
        length += 1;
    }
    let marked = size | (1u64 << (7 * length));
    marked.to_be_bytes()[8 - length..].to_vec()
}

/// An element with its body.
fn element(id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend_from_slice(&size_bytes(body.len() as u64));
    out.extend_from_slice(body);
    out
}

fn uint(id: u32, value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    element(id, &bytes[skip..])
}

fn text(id: u32, value: &str) -> Vec<u8> {
    element(id, value.as_bytes())
}

/// A Void element of exactly `total` bytes (at least 9: its ID, an 8-byte size).
fn void(total: usize) -> Vec<u8> {
    let mut out = id_bytes(VOID);
    out.push(0x01);
    let body = total - 9;
    out.extend_from_slice(&(body as u64).to_be_bytes()[1..]);
    out.resize(total, 0);
    out
}

/// The width and height a VP8 keyframe states (RFC 6386 §9.1), `None` for another frame.
pub fn vp8_keyframe_size(frame: &[u8]) -> Option<(u16, u16)> {
    let header = frame.get(..10)?;
    if header[0] & 0x01 != 0 || header[3..6] != [0x9D, 0x01, 0x2A] {
        return None;
    }
    let width = u16::from_le_bytes([header[6], header[7]]) & 0x3FFF;
    let height = u16::from_le_bytes([header[8], header[9]]) & 0x3FFF;
    Some((width, height))
}

/// Writes one VP8 track as WebM.
pub struct WebmWriter<W: Write + Seek> {
    out: W,
    /// Where the Segment's size is (patched at the end), and where its body starts (positions are from there).
    segment_size_at: u64,
    segment_start: u64,
    seek_head_at: u64,
    info_at: u64,
    duration_at: u64,
    tracks_at: u64,
    started: bool,
    cluster_time: u64,
    cluster: Vec<u8>,
    cues: Vec<(u64, u64)>,
    last_time: Option<u64>,
    position: u64,
    frames: u64,
}

impl<W: Write + Seek> WebmWriter<W> {
    /// A writer on `out`; nothing is written until the first keyframe.
    pub fn new(out: W) -> Self {
        WebmWriter {
            out,
            segment_size_at: 0,
            segment_start: 0,
            seek_head_at: 0,
            info_at: 0,
            duration_at: 0,
            tracks_at: 0,
            started: false,
            cluster_time: 0,
            cluster: Vec::new(),
            cues: Vec::new(),
            last_time: None,
            position: 0,
            frames: 0,
        }
    }

    /// Takes a frame at `time_ms` from the file's start. Before the first keyframe, and a frame whose time is before
    /// the last one's, it is dropped: false.
    pub fn write(&mut self, time_ms: u64, keyframe: bool, frame: &[u8]) -> io::Result<bool> {
        if !self.started {
            let Some((width, height)) = keyframe.then(|| vp8_keyframe_size(frame)).flatten() else {
                return Ok(false);
            };
            self.start(width, height)?;
        }
        if self.last_time.is_some_and(|last| time_ms < last) {
            return Ok(false);
        }
        let new_cluster = self.cluster.is_empty()
            || keyframe
            || time_ms - self.cluster_time >= MAX_CLUSTER_MS
            || self.cluster.len() >= MAX_CLUSTER_BYTES;
        if new_cluster {
            self.close_cluster()?;
            self.cluster_time = time_ms;
            self.cluster.extend_from_slice(&uint(TIMECODE, time_ms));
            if keyframe {
                self.cues.push((time_ms, self.position - self.segment_start));
            }
        }
        let relative = (time_ms - self.cluster_time) as i16;
        let mut block = Vec::with_capacity(4 + frame.len());
        block.push(0x81); // track 1
        block.extend_from_slice(&relative.to_be_bytes());
        block.push(if keyframe { 0x80 } else { 0x00 });
        block.extend_from_slice(frame);
        self.cluster.extend_from_slice(&element(SIMPLE_BLOCK, &block));
        self.last_time = Some(time_ms);
        self.frames += 1;
        Ok(true)
    }

    /// Whether the first keyframe came (the file has begun).
    pub fn started(&self) -> bool {
        self.started
    }

    /// The bytes so far, the cluster in memory included.
    pub fn bytes(&self) -> u64 {
        self.position + self.cluster.len() as u64
    }

    /// The time of the last frame, in milliseconds from the start.
    pub fn duration_ms(&self) -> u64 {
        self.last_time.unwrap_or(0)
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Writes the last cluster, the Cues, the Duration, the SeekHead and the Segment's size, and gives the output back,
    /// with the bytes written in all.
    pub fn finish(mut self) -> io::Result<(W, u64)> {
        if !self.started {
            return Ok((self.out, 0));
        }
        self.close_cluster()?;
        let cues_at = self.position;
        let mut points = Vec::new();
        for (time, place) in &self.cues {
            let positions = [uint(CUE_TRACK, 1), uint(CUE_CLUSTER_POSITION, *place)].concat();
            let point = [uint(CUE_TIME, *time), element(CUE_TRACK_POSITIONS, &positions)].concat();
            points.extend_from_slice(&element(CUE_POINT, &point));
        }
        if !points.is_empty() {
            self.put(&element(CUES, &points))?;
        }
        let end = self.position;
        // the duration: the last frame's time and one more frame's worth, as a 64-bit float
        let duration = (self.last_time.unwrap_or(0) + 33) as f64;
        self.out.seek(SeekFrom::Start(self.duration_at))?;
        self.out.write_all(&duration.to_be_bytes())?;
        let mut seeks = Vec::new();
        let mut entries = vec![(INFO, self.info_at), (TRACKS, self.tracks_at)];
        if !points.is_empty() {
            entries.push((CUES, cues_at));
        }
        for (id, at) in entries {
            let body = [
                element(SEEK_ID, &id_bytes(id)),
                uint(SEEK_POSITION, at - self.segment_start),
            ]
            .concat();
            seeks.extend_from_slice(&element(SEEK, &body));
        }
        let head = element(SEEK_HEAD, &seeks);
        let mut room = head;
        match SEEK_HEAD_ROOM.checked_sub(room.len()) {
            Some(rest) if rest >= 9 => room.extend_from_slice(&void(rest)),
            _ => {}
        }
        // a SeekHead that does not fit its room leaves the Void there: players find the Cues by reading on
        if room.len() == SEEK_HEAD_ROOM {
            self.out.seek(SeekFrom::Start(self.seek_head_at))?;
            self.out.write_all(&room)?;
        }
        let size = end - self.segment_start;
        self.out.seek(SeekFrom::Start(self.segment_size_at))?;
        self.out.write_all(&(size | (1u64 << 56)).to_be_bytes())?;
        self.out.seek(SeekFrom::Start(end))?;
        self.out.flush()?;
        Ok((self.out, end))
    }

    fn start(&mut self, width: u16, height: u16) -> io::Result<()> {
        let header = [
            uint(EBML_VERSION, 1),
            uint(EBML_READ_VERSION, 1),
            uint(EBML_MAX_ID_LENGTH, 4),
            uint(EBML_MAX_SIZE_LENGTH, 8),
            text(DOC_TYPE, "webm"),
            uint(DOC_TYPE_VERSION, 4),
            uint(DOC_TYPE_READ_VERSION, 2),
        ]
        .concat();
        self.put(&element(EBML, &header))?;
        // the Segment, its size unknown until the end (8 bytes kept for it)
        self.put(&id_bytes(SEGMENT))?;
        self.segment_size_at = self.position;
        self.put(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])?;
        self.segment_start = self.position;
        self.seek_head_at = self.position;
        self.put(&void(SEEK_HEAD_ROOM))?;
        self.info_at = self.position;
        let duration_prefix = [id_bytes(DURATION), size_bytes(8)].concat();
        let info_head = [
            uint(TIMECODE_SCALE, 1_000_000),
            text(MUXING_APP, "moqui"),
            text(WRITING_APP, "moqui"),
        ]
        .concat();
        let mut info = info_head.clone();
        info.extend_from_slice(&duration_prefix);
        info.extend_from_slice(&0f64.to_be_bytes());
        let info = element(INFO, &info);
        // where the Duration's 8 bytes are: the Info's header, its first fields, the Duration's own header
        let info_header = info.len() - (info_head.len() + duration_prefix.len() + 8);
        self.duration_at = self.position + (info_header + info_head.len() + duration_prefix.len()) as u64;
        self.put(&info)?;
        self.tracks_at = self.position;
        let video = [
            uint(PIXEL_WIDTH, u64::from(width)),
            uint(PIXEL_HEIGHT, u64::from(height)),
        ]
        .concat();
        let entry = [
            uint(TRACK_NUMBER, 1),
            uint(TRACK_UID, 1),
            uint(TRACK_TYPE, 1),
            text(CODEC_ID, "V_VP8"),
            element(VIDEO, &video),
        ]
        .concat();
        self.put(&element(TRACKS, &element(TRACK_ENTRY, &entry)))?;
        self.started = true;
        Ok(())
    }

    fn close_cluster(&mut self) -> io::Result<()> {
        if self.cluster.is_empty() {
            return Ok(());
        }
        let body = std::mem::take(&mut self.cluster);
        self.put(&element(CLUSTER, &body))
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.position += bytes.len() as u64;
        Ok(())
    }
}
