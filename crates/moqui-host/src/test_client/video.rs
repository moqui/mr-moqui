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

//! `moqui test-client --video` (docs/dev/Client-Testing.md): a video of each spec, from the CDP screencast of its
//! pages, and the timeline of its features on the same clock.
//!
//! The clock is Unix seconds from the start of the spec: a frame's time is its `metadata.timestamp` (when it was
//! drawn), or when it was read; a feature's end is when it finished and its start that less its duration. The
//! screencast sends a frame only when a page changes, so each frame lasts until the next one, and the last until the
//! end of the spec. Before the first frame the video is blank. With several pages open, the video shows the one opened
//! last that is still open; when it closes, the one before it again. The frames go to a directory under
//! `<out>/video`, and ffmpeg encodes them, through an `ffconcat` list with each frame's duration, into
//! `<out>/video/<Spec>.webm` (VP8) after the spec; the directory is removed then.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use moqui_devkit::testkit::{CaseResult, Outcome};
use serde_json::{json, Value};

use super::cdp::ScreencastFrame;
use super::load::file_part;

/// The frame rate of the encoded video.
const FPS: u32 = 25;

/// A frame's own timestamp is used when it is within this many seconds of when it was read.
const CLOCK_SLACK: f64 = 5.0;

/// The shortest last frame, so a frame at the very end still shows.
const MIN_LAST: f64 = 1.0 / FPS as f64;

/// The recording of the spec running, shared by the host and the spec's browser; `None` between specs.
pub(crate) type Shared = Arc<Mutex<Option<Recorder>>>;

/// One feature in `<Spec>.timeline.json`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TimelineEntry {
    pub(crate) feature: String,
    /// Seconds from the start of the video.
    pub(crate) start: f64,
    pub(crate) end: f64,
    /// `passed`, `skipped` (also an `ignore`) or `failed`.
    pub(crate) status: &'static str,
    /// The skip reason or the failure message.
    pub(crate) reason: Option<String>,
}

impl TimelineEntry {
    pub(crate) fn to_json(&self) -> Value {
        json!({ "feature": self.feature, "start": self.start, "end": self.end, "status": self.status,
            "reason": self.reason })
    }
}

/// The entry of a case that ended at `end` seconds and took `duration` seconds.
pub(crate) fn timeline_entry(case: &CaseResult, end: f64) -> TimelineEntry {
    let (status, reason) = match &case.outcome {
        Outcome::Passed => ("passed", None),
        Outcome::Failed(failure) => ("failed", Some(failure.message.clone())),
        Outcome::Ignored(reason) | Outcome::Skipped(reason) => ("skipped", Some(reason.clone())),
    };
    let end = end.max(0.0);
    TimelineEntry {
        feature: case.name.clone(),
        start: round_ms((end - case.duration.as_secs_f64()).max(0.0)),
        end: round_ms(end),
        status,
        reason,
    }
}

fn round_ms(seconds: f64) -> f64 {
    (seconds * 1000.0).round() / 1000.0
}

/// Unix seconds now.
pub(crate) fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// A frame's time in seconds from `t0`: its own timestamp when it has one near when it was read, else when it was read.
pub(crate) fn frame_time(timestamp: Option<f64>, received: f64, t0: f64) -> f64 {
    let at = timestamp
        .filter(|stamp| stamp.is_finite() && (stamp - received).abs() <= CLOCK_SLACK)
        .unwrap_or(received);
    (at - t0).max(0.0)
}

/// The `ffconcat` list of `frames` (each a time in seconds from the start and a file, in time order) for a video of
/// `end` seconds: `lead` from 0 to the first frame, each frame until the next, the last until `end`, and the last file
/// once more, since the concat demuxer gives the last entry no duration. Frames at the same time keep the later.
pub(crate) fn concat_list(frames: &[(f64, String)], end: f64, lead: Option<&str>) -> String {
    let mut out = String::from("ffconcat version 1.0\n");
    let Some((first, _)) = frames.first() else {
        return out;
    };
    let quote = |path: &str| format!("'{}'", path.replace('\'', r"'\''"));
    if let Some(lead) = lead {
        if *first > 0.0005 {
            let _ = writeln!(out, "file {}\nduration {:.3}", quote(lead), first);
        }
    }
    for (index, (at, path)) in frames.iter().enumerate() {
        let next = frames.get(index + 1).map(|(next, _)| *next);
        let duration = match next {
            Some(next) => next - at,
            None => (end - at).max(MIN_LAST),
        };
        if next.is_some() && duration < 0.0005 {
            continue;
        }
        let _ = writeln!(out, "file {}\nduration {:.3}", quote(path), duration);
    }
    if let Some((_, last)) = frames.last() {
        let _ = writeln!(out, "file {}", quote(last));
    }
    out
}

/// The width and height of a JPEG, from its start-of-frame segment.
pub(crate) fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    while at + 9 < data.len() {
        if data[at] != 0xFF {
            return None;
        }
        let marker = data[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        let length = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        // SOF0 to SOF15, less DHT (C4), JPG (C8) and DAC (CC)
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let height = u16::from_be_bytes([data[at + 5], data[at + 6]]);
            let width = u16::from_be_bytes([data[at + 7], data[at + 8]]);
            return Some((u32::from(width), u32::from(height)));
        }
        at += 2 + length;
    }
    None
}

/// The recording of one spec: the frames shown, the open pages, and the timeline.
pub(crate) struct Recorder {
    spec: String,
    video_dir: PathBuf,
    frames_dir: PathBuf,
    /// The start, Unix seconds.
    t0: f64,
    /// Encode the video: Chromium and ffmpeg were found.
    encode: bool,
    /// The frames of the video, by time.
    shown: Vec<(f64, String)>,
    /// The pages open, in the order opened; the last is shown.
    open: Vec<String>,
    /// Each page's latest frame, shown again when the page is shown again.
    last: HashMap<String, PathBuf>,
    written: usize,
    /// The largest frame, the video's size.
    size: (u32, u32),
    timeline: Vec<TimelineEntry>,
    /// When the spec's pages stopped, the end of the video; otherwise when it finishes.
    end: Option<f64>,
}

impl Recorder {
    /// Start recording spec `spec` now; its files go to `<out_dir>/video`. Without `encode` only the timeline is
    /// written.
    pub(crate) fn new(spec: &str, out_dir: &Path, encode: bool) -> Recorder {
        let video_dir = out_dir.join("video");
        let frames_dir = video_dir.join(format!(".frames-{}-{}", file_part(spec), std::process::id()));
        Recorder {
            spec: spec.to_string(),
            video_dir,
            frames_dir,
            t0: unix_now(),
            encode,
            shown: Vec::new(),
            open: Vec::new(),
            last: HashMap::new(),
            written: 0,
            size: (0, 0),
            timeline: Vec::new(),
            end: None,
        }
    }

    /// The spec's browser is about to close: the video ends now, not after the browser has gone.
    pub(crate) fn mark_end(&mut self) {
        self.end = Some(self.now());
    }

    /// Seconds since the start.
    pub(crate) fn now(&self) -> f64 {
        (unix_now() - self.t0).max(0.0)
    }

    fn latest(&self) -> f64 {
        self.shown.last().map_or(0.0, |(at, _)| *at)
    }

    /// A page opened: it is shown from its next frame.
    pub(crate) fn page_opened(&mut self, session: &str) {
        self.open.retain(|open| open != session);
        self.open.push(session.to_string());
    }

    /// A page closed: when it was shown, the page opened before it is shown again from now.
    pub(crate) fn page_closed(&mut self, session: &str) {
        let was_shown = self.open.last().is_some_and(|open| open == session);
        self.open.retain(|open| open != session);
        self.last.remove(session);
        if !was_shown {
            return;
        }
        let back = self.open.last().and_then(|open| self.last.get(open)).cloned();
        if let Some(path) = back {
            let at = self.now().max(self.latest());
            self.shown.push((at, path.display().to_string()));
        }
    }

    /// A screencast frame: written, and in the video when its page is the one shown.
    pub(crate) fn frame(&mut self, frame: ScreencastFrame) {
        if !self.encode {
            return;
        }
        if self.written == 0 {
            if let Err(err) = std::fs::create_dir_all(&self.frames_dir) {
                tracing::error!("--video of {}: {}: {err}", self.spec, self.frames_dir.display());
                self.encode = false;
                return;
            }
        }
        self.written += 1;
        let path = self.frames_dir.join(format!("frame-{:06}.jpg", self.written));
        if let Err(err) = std::fs::write(&path, &frame.data) {
            tracing::error!("--video of {}: {}: {err}", self.spec, path.display());
            return;
        }
        if let Some((width, height)) = jpeg_size(&frame.data) {
            self.size = (self.size.0.max(width), self.size.1.max(height));
        }
        if !self.open.contains(&frame.session) {
            // a late frame of a page already closed
            return;
        }
        self.last.insert(frame.session.clone(), path.clone());
        if self.open.last() == Some(&frame.session) {
            let at = frame_time(frame.timestamp, frame.received, self.t0).max(self.latest());
            self.shown.push((at, path.display().to_string()));
        }
    }

    /// A case finished now.
    pub(crate) fn case(&mut self, case: &CaseResult) {
        let entry = timeline_entry(case, self.now());
        self.timeline.push(entry);
    }

    /// The spec ended: write `<Spec>.timeline.json` and, when there were frames, encode `<Spec>.webm`. The video's
    /// path when one was written. Each failure is logged and the rest goes on.
    pub(crate) fn finish(self) -> Option<PathBuf> {
        let end = self.end.unwrap_or_else(|| self.now()).max(self.latest());
        let stem = file_part(&self.spec);
        if let Err(err) = std::fs::create_dir_all(&self.video_dir) {
            tracing::error!("--video of {}: {}: {err}", self.spec, self.video_dir.display());
            return None;
        }
        let timeline_path = self.video_dir.join(format!("{stem}.timeline.json"));
        let entries: Vec<Value> = self.timeline.iter().map(TimelineEntry::to_json).collect();
        match serde_json::to_string_pretty(&entries) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&timeline_path, text) {
                    tracing::error!("--video of {}: {}: {err}", self.spec, timeline_path.display());
                }
            }
            Err(err) => tracing::error!("--video of {}: the timeline: {err}", self.spec),
        }
        let video = if self.encode && !self.shown.is_empty() {
            let path = self.video_dir.join(format!("{stem}.webm"));
            match self.encode_to(&path, end) {
                Ok(()) => Some(path),
                Err(err) => {
                    tracing::error!("--video of {}: {err}", self.spec);
                    None
                }
            }
        } else {
            if self.encode {
                tracing::info!("--video of {}: no page was shown, so no video", self.spec);
            }
            None
        };
        if self.frames_dir.exists() {
            if let Err(err) = std::fs::remove_dir_all(&self.frames_dir) {
                tracing::error!("--video of {}: {}: {err}", self.spec, self.frames_dir.display());
            }
        }
        video
    }

    fn encode_to(&self, path: &Path, end: f64) -> Result<(), String> {
        // even, as yuv420p needs
        let width = self.size.0.max(16).div_ceil(2) * 2;
        let height = self.size.1.max(16).div_ceil(2) * 2;
        let blank = self.frames_dir.join("blank.jpg");
        let made_blank = ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c=white:s={width}x{height}"),
            "-frames:v",
            "1",
            &blank.display().to_string(),
        ]);
        // without a blank frame the first frame shows from the start, so the times still hold
        let lead = match made_blank {
            Ok(()) => blank.display().to_string(),
            Err(err) => {
                tracing::error!("--video of {}: the blank lead frame: {err}", self.spec);
                self.shown[0].1.clone()
            }
        };
        let list = self.frames_dir.join("frames.ffconcat");
        std::fs::write(&list, concat_list(&self.shown, end, Some(&lead)))
            .map_err(|err| format!("{}: {err}", list.display()))?;
        let filter = format!(
            "scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=white,fps={FPS},format=yuv420p"
        );
        ffmpeg(&[
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &list.display().to_string(),
            "-vf",
            &filter,
            // the repeated last file would otherwise add a second of its own
            "-t",
            &format!("{:.3}", end.max(MIN_LAST)),
            "-c:v",
            "libvpx",
            "-b:v",
            "2M",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            &path.display().to_string(),
        ])
    }
}

/// Run `ffmpeg -y` with `args`; its error output when it fails.
fn ffmpeg(args: &[&str]) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("ffmpeg did not start (is it installed and on the PATH?): {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Whether `ffmpeg` runs, or why not.
pub(crate) fn check_ffmpeg() -> Result<(), String> {
    ffmpeg(&["-version"]).map_err(|err| format!("{err}; install ffmpeg to record videos"))
}

#[cfg(test)]
#[path = "video_tests.rs"]
mod tests;
