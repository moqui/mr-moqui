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

//! Call recording, sans-IO (plans/Comms-Framework.md W5, §6.4): the frames a room's tap hands out
//! ([`crate::tap::TapEvent`]) written as files, with no transcoding.
//!
//! - Each published track is its own file: a leg's microphone, screen audio or phone audio as Opus in Ogg
//!   ([`ogg::OggOpusWriter`]), its camera or screen as VP8 in WebM ([`webm::WebmWriter`]). A simulcast track is
//!   recorded at its best layer that sent a keyframe, switching up at a better layer's keyframe and down when its layer
//!   stops.
//! - The mixed audio of the whole call is decoded and mixed by an [`AudioRoom`] each 20 ms tick (its `all()` mix) and
//!   encoded with [`OpusEncoder`] into one more Ogg file.
//! - A track's timing is its RTP time. Frames that never came are a jump in the granule positions or block times,
//!   never made-up audio or video.
//! - Files are cut into segments: all of them at a pause (and the next period starts at the resume), and one track's at
//!   its size or time cap. Each closed segment ([`ClosedSegment`]) says its track, period, order, offset from its
//!   period's start and length, for a player that plays a period's tracks together.
//!
//! [`Recorder`] owns no thread, file system or clock: its caller passes the time (milliseconds since the recording
//! started) and a [`Spool`] that makes its files, and takes the closed segments to store (moqui-host
//! `src/rtc/recorder.rs`).

pub mod ogg;
pub mod webm;

use std::collections::HashMap;
use std::io::{self, Seek, Write};
use std::sync::Arc;

use crate::audio::opus::OpusEncoder;
use crate::audio::room::AudioRoom;
use crate::audio::FRAME;
use crate::sfu::Source;
use crate::tap::{Kind, OwnedFrame, TapEvent};
use ogg::{opus_packet_samples, OggOpusWriter};
use webm::WebmWriter;

/// The mixed audio's bitrate.
pub const MIX_BITRATE: i32 = 32_000;
/// The Opus encoder's delay at 48 kHz, the mixed file's pre-skip.
const MIX_PRE_SKIP: u16 = 312;
/// A layer silent this long gives way to the next best one that sends a keyframe.
const LAYER_SILENT_MS: u64 = 1_000;
/// Past this many times its cap, a video segment is cut without waiting for a keyframe (the next one starts at one).
const VIDEO_HARD_CAP: u64 = 2;

/// What to record and the limits.
#[derive(Debug, Clone, Copy)]
pub struct RecordConfig {
    /// Each track as its own file.
    pub tracks: bool,
    /// The mixed audio.
    pub mix: bool,
    /// A segment is cut at this size...
    pub max_segment_bytes: u64,
    /// ...or this length.
    pub max_segment_ms: u64,
    /// The recording stops at this total ([`RecordError::Limit`]).
    pub max_total_bytes: u64,
}

impl Default for RecordConfig {
    fn default() -> Self {
        RecordConfig {
            tracks: true,
            mix: true,
            max_segment_bytes: 64 * 1024 * 1024,
            max_segment_ms: 30 * 60 * 1000,
            max_total_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// Makes the recorder's files. Its caller keeps them where it likes (a spool directory; memory in tests).
pub trait Spool {
    type File: Write + Seek;
    /// A new file named `name` (unique within the recording).
    fn create(&mut self, name: &str) -> io::Result<Self::File>;
    /// A file that will not be closed as a segment (an empty one) is let go.
    fn discard(&mut self, name: &str);
}

/// The kind of a recorded track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackKind {
    Audio,
    Video,
    /// The mixed audio of the call.
    Mix,
}

impl TrackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TrackKind::Audio => "audio",
            TrackKind::Video => "video",
            TrackKind::Mix => "mix",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            TrackKind::Video => "video/webm",
            TrackKind::Audio | TrackKind::Mix => "audio/ogg",
        }
    }

    pub fn codec(self) -> &'static str {
        match self {
            TrackKind::Video => "vp8",
            TrackKind::Audio | TrackKind::Mix => "opus",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            TrackKind::Video => "webm",
            TrackKind::Audio | TrackKind::Mix => "ogg",
        }
    }
}

/// Which track: a leg's source (audio or video), or the mix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackId {
    pub leg: Option<Arc<str>>,
    pub source: Option<Source>,
    pub kind: TrackKind,
}

impl TrackId {
    fn mix() -> Self {
        TrackId {
            leg: None,
            source: None,
            kind: TrackKind::Mix,
        }
    }

    /// A file name part: letters, digits, `-` and `_` only.
    fn slug(&self) -> String {
        let leg: String = self
            .leg
            .as_deref()
            .unwrap_or("mix")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .take(40)
            .collect();
        match self.source {
            Some(source) => format!("{leg}-{}", source.as_str()),
            None => leg,
        }
    }
}

/// A segment file the recorder closed, for its caller to store.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedSegment {
    pub track: TrackId,
    /// The spool file's name.
    pub file: String,
    /// The recording's period (counted by the caller: each resume starts the next).
    pub period: u32,
    /// The segment's order in its track, from 1.
    pub seq: u32,
    /// When its first frame came, from its period's start.
    pub start_offset_ms: u64,
    /// When its first frame came and when it closed, from the recording's start.
    pub from_ms: u64,
    pub thru_ms: u64,
    /// Its length by the media's own timing.
    pub duration_ms: u64,
    pub bytes: u64,
    /// The track's last segment: its leg left, or the recording ended.
    pub track_ended: bool,
}

/// Why the recorder cannot go on.
#[derive(Debug)]
pub enum RecordError {
    /// A file could not be written.
    Io(io::Error),
    /// The recording reached its total size ([`RecordConfig::max_total_bytes`]).
    Limit,
    /// The mixed audio's encoder could not be made.
    Codec(String),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::Io(err) => write!(f, "a recording file could not be written: {err}"),
            RecordError::Limit => write!(f, "the recording reached its size limit"),
            RecordError::Codec(err) => write!(f, "the mixed audio could not be encoded: {err}"),
        }
    }
}

impl From<io::Error> for RecordError {
    fn from(err: io::Error) -> Self {
        RecordError::Io(err)
    }
}

enum Writer<F: Write + Seek> {
    Audio(OggOpusWriter<F>),
    Video(WebmWriter<F>),
}

impl<F: Write + Seek> Writer<F> {
    fn bytes(&self) -> u64 {
        match self {
            Writer::Audio(w) => w.bytes(),
            Writer::Video(w) => w.bytes(),
        }
    }

    fn duration_ms(&self) -> u64 {
        match self {
            Writer::Audio(w) => w.duration_ms(),
            Writer::Video(w) => w.duration_ms(),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Writer::Audio(w) => w.packets() == 0,
            Writer::Video(w) => w.frames() == 0,
        }
    }

    /// Closes the file; the bytes written in all.
    fn finish(self) -> io::Result<u64> {
        match self {
            Writer::Audio(w) => w.finish().map(|(_, bytes)| bytes),
            Writer::Video(w) => w.finish().map(|(_, bytes)| bytes),
        }
    }
}

/// An open segment of a track.
struct Open<F: Write + Seek> {
    writer: Writer<F>,
    file: String,
    seq: u32,
    from_ms: u64,
    /// The RTP time of the segment's first frame, in the track's clock, and that clock.
    base_rtp: i64,
    clock_rate: u32,
    /// The time base of the segment in milliseconds, moved when the video layer changes (each layer has its own RTP
    /// timeline).
    shift_ms: i64,
    /// Cut at the next keyframe (a video segment past its cap).
    cut: bool,
}

/// A track's state between segments: its next segment number and, for video, the layer recorded.
#[derive(Default)]
struct TrackState {
    seq: u32,
    layer: Option<Option<String>>,
    /// The wall time and media time of the last frame written, to carry the timing across a layer switch.
    last_wall_ms: u64,
    last_media_ms: i64,
}

/// The rank of a simulcast layer: a track without layers is its only one.
fn layer_rank(rid: Option<&str>) -> u8 {
    match rid {
        None => 4,
        Some("h") => 3,
        Some("m") => 2,
        Some("l") => 1,
        Some(_) => 0,
    }
}

/// Records a room's tracks and its mixed audio as segment files.
pub struct Recorder<S: Spool> {
    config: RecordConfig,
    spool: S,
    period: u32,
    period_start_ms: u64,
    paused: bool,
    open: HashMap<TrackId, Open<S::File>>,
    states: HashMap<TrackId, TrackState>,
    room: AudioRoom,
    encoder: Option<OpusEncoder>,
    /// The mixed audio's samples in the open mix segment.
    mix_samples: u64,
    closed: Vec<ClosedSegment>,
    stored_bytes: u64,
    next_serial: u32,
}

impl<S: Spool> Recorder<S> {
    /// A recorder in `period`, recording from `now_ms` unless `paused`.
    pub fn new(config: RecordConfig, spool: S, period: u32, paused: bool, now_ms: u64) -> Result<Self, RecordError> {
        let encoder = if config.mix {
            Some(OpusEncoder::new(MIX_BITRATE).map_err(|e| RecordError::Codec(e.0))?)
        } else {
            None
        };
        Ok(Recorder {
            config,
            spool,
            period,
            period_start_ms: now_ms,
            paused,
            open: HashMap::new(),
            states: HashMap::new(),
            room: AudioRoom::default(),
            encoder,
            mix_samples: 0,
            closed: Vec::new(),
            stored_bytes: 0,
            next_serial: 1,
        })
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn period(&self) -> u32 {
        self.period
    }

    /// The bytes of the closed segments and the open ones.
    pub fn total_bytes(&self) -> u64 {
        self.stored_bytes + self.open.values().map(|o| o.writer.bytes()).sum::<u64>()
    }

    /// The segments closed since the last call, for the caller to store.
    pub fn take_closed(&mut self) -> Vec<ClosedSegment> {
        std::mem::take(&mut self.closed)
    }

    /// What the room's tap gave: a frame to write (and to mix), or a leg that left (its tracks end).
    pub fn event(&mut self, now_ms: u64, event: &TapEvent) -> Result<(), RecordError> {
        match event {
            TapEvent::Frame(frame) => {
                if self.paused {
                    return Ok(());
                }
                if self.config.mix {
                    self.room.push(frame);
                }
                if self.config.tracks {
                    self.frame(now_ms, frame)?;
                }
                self.check_total()
            }
            TapEvent::Left(leg) => {
                self.room.remove_leg(leg);
                let ids: Vec<TrackId> = self
                    .open
                    .keys()
                    .filter(|id| id.leg.as_deref() == Some(&**leg))
                    .cloned()
                    .collect();
                for id in ids {
                    self.close(&id, now_ms, true)?;
                }
                self.states.retain(|id, _| id.leg.as_deref() != Some(&**leg));
                Ok(())
            }
        }
    }

    /// The next 20 ms of the mixed audio: decoded, mixed and encoded. The caller calls it every 20 ms (more than once to
    /// catch up after a stall).
    pub fn tick(&mut self, now_ms: u64) -> Result<(), RecordError> {
        if self.paused || !self.config.mix {
            return Ok(());
        }
        let pcm = self.room.tick().all().to_vec();
        let id = TrackId::mix();
        if !self.open.contains_key(&id) {
            if self.room.legs().next().is_none() {
                // nobody has been heard yet: the mix starts with the first voice
                return Ok(());
            }
            self.open_segment(&id, now_ms, 0, 48_000)?;
            self.mix_samples = 0;
        }
        let Some(encoder) = self.encoder.as_mut() else {
            return Ok(());
        };
        let packet = encoder.encode(&pcm).map_err(|e| RecordError::Codec(e.0))?.to_vec();
        if let Some(open) = self.open.get_mut(&id) {
            if let Writer::Audio(writer) = &mut open.writer {
                writer.write(self.mix_samples, FRAME as u32, &packet)?;
            }
        }
        self.mix_samples += FRAME as u64;
        self.cap(&id, now_ms, false)?;
        self.check_total()
    }

    /// Pauses: every open segment closes, and frames are dropped until [`Recorder::resume`].
    pub fn pause(&mut self, now_ms: u64) -> Result<(), RecordError> {
        if self.paused {
            return Ok(());
        }
        self.paused = true;
        self.close_all(now_ms, false)?;
        // the mix starts clean at the resume: what was buffered is from before the pause
        self.room = AudioRoom::default();
        Ok(())
    }

    /// Resumes in `period`, which starts now.
    pub fn resume(&mut self, now_ms: u64, period: u32) {
        if !self.paused {
            return;
        }
        self.paused = false;
        self.period = period;
        self.period_start_ms = now_ms;
        for state in self.states.values_mut() {
            state.layer = None;
        }
    }

    /// Ends the recording: every open segment closes as its track's last.
    pub fn finish(&mut self, now_ms: u64) -> Result<(), RecordError> {
        self.close_all(now_ms, true)
    }

    /// The files still open (to remove after a failure).
    pub fn open_files(&self) -> Vec<String> {
        self.open.values().map(|o| o.file.clone()).collect()
    }

    fn check_total(&self) -> Result<(), RecordError> {
        if self.total_bytes() > self.config.max_total_bytes {
            Err(RecordError::Limit)
        } else {
            Ok(())
        }
    }

    fn frame(&mut self, now_ms: u64, frame: &OwnedFrame) -> Result<(), RecordError> {
        let kind = match (frame.kind, frame.codec) {
            (Kind::Audio, "opus") => TrackKind::Audio,
            (Kind::Video, "vp8") => TrackKind::Video,
            // other codecs (VP9, H.264 later) are not recorded yet
            _ => return Ok(()),
        };
        let id = TrackId {
            leg: Some(frame.leg.clone()),
            source: Some(frame.source),
            kind,
        };
        match kind {
            TrackKind::Audio => self.audio(&id, now_ms, frame),
            _ => self.video(&id, now_ms, frame),
        }
    }

    fn audio(&mut self, id: &TrackId, now_ms: u64, frame: &OwnedFrame) -> Result<(), RecordError> {
        let Some(samples) = opus_packet_samples(&frame.data) else {
            return Ok(());
        };
        let clock = if frame.clock_rate == 0 {
            48_000
        } else {
            frame.clock_rate
        };
        if !self.open.contains_key(id) {
            self.open_segment(id, now_ms, frame.rtp_time as i64, clock)?;
        }
        let Some(open) = self.open.get_mut(id) else {
            return Ok(());
        };
        let elapsed = frame.rtp_time as i64 - open.base_rtp;
        if elapsed < 0 {
            return Ok(());
        }
        let start = elapsed as u64 * 48_000 / u64::from(open.clock_rate);
        if let Writer::Audio(writer) = &mut open.writer {
            writer.write(start, samples, &frame.data)?;
        }
        self.cap(id, now_ms, false)
    }

    fn video(&mut self, id: &TrackId, now_ms: u64, frame: &OwnedFrame) -> Result<(), RecordError> {
        let rid = frame.rid.clone();
        let state = self.states.entry(id.clone()).or_default();
        // which layer: the one recorded, or a better one (or any, when it went silent) at its keyframe
        let switch = match &state.layer {
            None => frame.keyframe,
            Some(current) if *current == rid => false,
            Some(current) => {
                frame.keyframe
                    && (layer_rank(rid.as_deref()) > layer_rank(current.as_deref())
                        || now_ms.saturating_sub(state.last_wall_ms) > LAYER_SILENT_MS)
            }
        };
        if switch {
            state.layer = Some(rid.clone());
        } else if state.layer.as_ref() != Some(&rid) {
            return Ok(());
        }
        let clock = if frame.clock_rate == 0 {
            90_000
        } else {
            frame.clock_rate
        };
        let (last_wall, last_media) = (state.last_wall_ms, state.last_media_ms);
        // a segment past its cap is cut at a keyframe, so the next one starts with one
        if frame.keyframe && self.open.get(id).is_some_and(|o| o.cut) {
            self.close(id, now_ms, false)?;
        }
        if !self.open.contains_key(id) {
            if !frame.keyframe {
                return Ok(());
            }
            self.open_segment(id, now_ms, frame.rtp_time as i64, clock)?;
        } else if switch {
            // the new layer's RTP timeline: carry on from the last frame, by the wall time between
            if let Some(open) = self.open.get_mut(id) {
                let at = last_media + now_ms.saturating_sub(last_wall) as i64;
                open.base_rtp = frame.rtp_time as i64;
                open.clock_rate = clock;
                open.shift_ms = at;
            }
        }
        let Some(open) = self.open.get_mut(id) else {
            return Ok(());
        };
        let media_ms = open.shift_ms + (frame.rtp_time as i64 - open.base_rtp) * 1000 / i64::from(open.clock_rate);
        if media_ms < 0 {
            return Ok(());
        }
        let written = match &mut open.writer {
            Writer::Video(writer) => writer.write(media_ms as u64, frame.keyframe, &frame.data)?,
            Writer::Audio(_) => false,
        };
        if written {
            if let Some(state) = self.states.get_mut(id) {
                state.last_wall_ms = now_ms;
                state.last_media_ms = media_ms;
            }
        }
        self.cap(id, now_ms, true)
    }

    /// A segment past its size or length cap is cut (a video one at its next keyframe, unless far past it).
    fn cap(&mut self, id: &TrackId, now_ms: u64, video: bool) -> Result<(), RecordError> {
        let Some(open) = self.open.get_mut(id) else {
            return Ok(());
        };
        let bytes = open.writer.bytes();
        let over =
            bytes >= self.config.max_segment_bytes || now_ms.saturating_sub(open.from_ms) >= self.config.max_segment_ms;
        if !over {
            return Ok(());
        }
        if video && bytes < self.config.max_segment_bytes.saturating_mul(VIDEO_HARD_CAP) {
            open.cut = true;
            return Ok(());
        }
        self.close(id, now_ms, false)
    }

    fn open_segment(&mut self, id: &TrackId, now_ms: u64, base_rtp: i64, clock_rate: u32) -> Result<(), RecordError> {
        let state = self.states.entry(id.clone()).or_default();
        state.seq += 1;
        let seq = state.seq;
        let file = format!("{}-{}-{}.{}", self.period, seq, id.slug(), id.kind.extension());
        let out = self.spool.create(&file)?;
        let writer = match id.kind {
            TrackKind::Video => Writer::Video(WebmWriter::new(out)),
            TrackKind::Audio => Writer::Audio(OggOpusWriter::new(out, self.next_serial, 0)?),
            TrackKind::Mix => Writer::Audio(OggOpusWriter::new(out, self.next_serial, MIX_PRE_SKIP)?),
        };
        self.next_serial += 1;
        self.open.insert(
            id.clone(),
            Open {
                writer,
                file,
                seq,
                from_ms: now_ms,
                base_rtp,
                clock_rate,
                shift_ms: 0,
                cut: false,
            },
        );
        Ok(())
    }

    fn close(&mut self, id: &TrackId, now_ms: u64, track_ended: bool) -> Result<(), RecordError> {
        let Some(open) = self.open.remove(id) else {
            return Ok(());
        };
        let Open {
            writer,
            file,
            seq,
            from_ms,
            ..
        } = open;
        if writer.is_empty() {
            drop(writer);
            self.spool.discard(&file);
            return Ok(());
        }
        let duration_ms = writer.duration_ms();
        let finished = writer.finish();
        let bytes = match finished {
            Ok(bytes) => bytes,
            Err(err) => {
                self.spool.discard(&file);
                return Err(err.into());
            }
        };
        self.stored_bytes += bytes;
        self.closed.push(ClosedSegment {
            track: id.clone(),
            file,
            period: self.period,
            seq,
            start_offset_ms: from_ms.saturating_sub(self.period_start_ms),
            from_ms,
            thru_ms: now_ms.max(from_ms),
            duration_ms,
            bytes,
            track_ended,
        });
        Ok(())
    }

    fn close_all(&mut self, now_ms: u64, track_ended: bool) -> Result<(), RecordError> {
        let ids: Vec<TrackId> = self.open.keys().cloned().collect();
        let mut first_error = None;
        for id in ids {
            // each file on its own: one that fails does not keep the others from closing
            if let Err(err) = self.close(&id, now_ms, track_ended) {
                first_error.get_or_insert(err);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

#[cfg(test)]
mod tests;
