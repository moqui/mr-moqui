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

//! The recorder's session with frames in memory: segments handed to the store as spooled files, a pause and a
//! resume, a leg leaving, and the ends: a stop, a failed store and the size limit, each leaving no spool behind.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use moqui_rtc::audio::opus::OpusEncoder;
use moqui_rtc::audio::FRAME;
use moqui_rtc::record::{ClosedSegment, RecordConfig, TrackKind};

use super::{Control, Ended, Limits, Session, Store};
use crate::rtc::media::{Kind, OwnedFrame, Source, TapEvent};

/// A store that keeps each segment's bytes, read from its spooled file as it is handed over.
#[derive(Default)]
struct MemoryStore {
    stored: Vec<(ClosedSegment, Vec<u8>)>,
    fail_after: Option<usize>,
    failure: Option<String>,
    ends: Vec<(String, String)>,
}

impl Store for MemoryStore {
    fn put(&mut self, segment: ClosedSegment, path: PathBuf, _from: SystemTime) -> Result<(), String> {
        if self.fail_after.is_some_and(|n| self.stored.len() >= n) {
            self.failure = Some("the store refused it".to_string());
            return Err("the store refused it".to_string());
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        self.stored.push((segment, bytes));
        Ok(())
    }
    fn failure(&self) -> Option<String> {
        self.failure.clone()
    }
    fn drain(&mut self) {}
    fn end(&mut self, outcome: &str, text: &str) {
        self.ends.push((outcome.to_string(), text.to_string()));
    }
}

fn spool_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-recorder-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn limits(record: RecordConfig) -> Limits {
    Limits {
        queue_frames: 256,
        record,
    }
}

fn voice(count: usize) -> Vec<Vec<u8>> {
    let mut encoder = OpusEncoder::new(32_000).unwrap();
    (0..count)
        .map(|n| {
            let pcm: Vec<i16> = (0..FRAME)
                .map(|i| (((n * FRAME + i) as f64 / 48.0).sin() * 6_000.0) as i16)
                .collect();
            encoder.encode(&pcm).unwrap().to_vec()
        })
        .collect()
}

fn audio(leg: &str, rtp_time: u64, data: &[u8]) -> TapEvent {
    TapEvent::Frame(OwnedFrame {
        leg: Arc::from(leg),
        kind: Kind::Audio,
        source: Source::Microphone,
        rid: None,
        codec: "opus",
        rtp_time,
        clock_rate: 48_000,
        data: data.to_vec(),
        keyframe: false,
        audio_level: None,
    })
}

fn camera(leg: &str, rtp_time: u64, keyframe: bool) -> TapEvent {
    let mut data = if keyframe {
        vec![0x10, 0x02, 0x00, 0x9D, 0x01, 0x2A, 0x40, 0x01, 0xB4, 0x00]
    } else {
        vec![0x11, 0x02, 0x00]
    };
    data.resize(200, 0x5A);
    TapEvent::Frame(OwnedFrame {
        leg: Arc::from(leg),
        kind: Kind::Video,
        source: Source::Camera,
        rid: None,
        codec: "vp8",
        rtp_time,
        clock_rate: 90_000,
        data,
        keyframe,
        audio_level: None,
    })
}

#[test]
fn a_session_stores_each_track_and_the_mix_by_period_and_leaves_no_spool() {
    let dir = spool_dir("session");
    let mut session = Session::new(
        dir.clone(),
        MemoryStore::default(),
        limits(RecordConfig::default()),
        1,
        false,
    )
    .unwrap();
    let frames = voice(100);
    let mut now = 0;
    for n in 0..40u64 {
        session.event(now, &audio("a", n * 960, &frames[n as usize])).unwrap();
        session
            .event(now, &audio("b", 50_000 + n * 960, &frames[n as usize]))
            .unwrap();
        if n % 2 == 0 {
            session.event(now, &camera("a", n * 1_800, n == 0)).unwrap();
        }
        now += 20;
        session.tick_to(now).unwrap();
    }
    assert!(session.store.stored.is_empty(), "nothing closed while recording");
    assert!(session.control(now, Control::Pause).unwrap());
    let first: Vec<TrackKind> = session.store.stored.iter().map(|(s, _)| s.track.kind).collect();
    assert_eq!(first.len(), 4, "a and b's voices, a's camera and the mix: {first:?}");
    for (segment, bytes) in &session.store.stored {
        assert_eq!(segment.period, 1);
        let magic: &[u8] = if segment.track.kind == TrackKind::Video {
            &[0x1A, 0x45, 0xDF, 0xA3]
        } else {
            b"OggS"
        };
        assert_eq!(&bytes[..4], magic, "{}", segment.file);
        assert_eq!(bytes.len() as u64, segment.bytes);
    }
    // paused frames are not kept; the resume is period 2
    session.event(now, &audio("a", 40 * 960, &frames[40])).unwrap();
    now += 400;
    assert!(session.control(now, Control::Resume { period: 2 }).unwrap());
    for n in 60..80u64 {
        session.event(now, &audio("a", n * 960, &frames[n as usize])).unwrap();
        now += 20;
        session.tick_to(now).unwrap();
    }
    session.event(now, &TapEvent::Left(Arc::from("a"))).unwrap();
    let left = session.store.stored.last().unwrap().0.clone();
    assert_eq!(
        (left.track.leg.as_deref(), left.period, left.track_ended),
        (Some("a"), 2, true)
    );
    assert_eq!(left.duration_ms, 400);
    assert!(!session.control(now, Control::Stop).unwrap(), "a stop ends the loop");
    let (how, store) = session.end(now, Ended::Stopped);
    assert_eq!(how, Ended::Stopped);
    assert!(store.ends.is_empty(), "a stop the services asked for is not told back");
    assert!(store
        .stored
        .iter()
        .any(|(s, _)| s.track.kind == TrackKind::Mix && s.period == 2 && s.track_ended));
    assert!(!dir.exists(), "the spool is gone");
}

#[test]
fn a_store_that_fails_ends_the_recording_with_its_reason_and_removes_the_spool() {
    let dir = spool_dir("failed");
    let store = MemoryStore {
        fail_after: Some(0),
        ..MemoryStore::default()
    };
    let config = RecordConfig {
        max_segment_bytes: 400,
        mix: false,
        ..RecordConfig::default()
    };
    let mut session = Session::new(dir.clone(), store, limits(config), 1, false).unwrap();
    let frames = voice(30);
    let mut failed = None;
    for n in 0..30u64 {
        if let Err(how) = session.event(n * 20, &audio("a", n * 960, &frames[n as usize])) {
            failed = Some(how);
            break;
        }
    }
    let how = failed.expect("the store's refusal ends it");
    assert_eq!(how, Ended::Failed("the store refused it".to_string()));
    // the end is told to the services (here the store), and nothing is left in the spool
    let (ended, store) = session.end(700, how);
    assert!(matches!(ended, Ended::Failed(_)));
    assert_eq!(
        store.ends,
        vec![("failed".to_string(), "the store refused it".to_string())]
    );
    assert!(store.stored.is_empty());
    assert!(!dir.exists());
}

#[test]
fn the_size_limit_stops_the_recording_after_storing_what_it_has() {
    let dir = spool_dir("limit");
    let config = RecordConfig {
        mix: false,
        max_total_bytes: 1_500,
        ..RecordConfig::default()
    };
    let mut session = Session::new(dir.clone(), MemoryStore::default(), limits(config), 1, false).unwrap();
    let frames = voice(60);
    let mut how = None;
    for n in 0..60u64 {
        if let Err(end) = session.event(n * 20, &audio("a", n * 960, &frames[n as usize])) {
            how = Some(end);
            break;
        }
    }
    assert_eq!(how, Some(Ended::Limit));
    let (ended, store) = session.end(1_000, Ended::Limit);
    assert_eq!(ended, Ended::Limit);
    assert_eq!(store.stored.len(), 1, "the open segment is stored");
    assert_eq!(
        store.ends.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(),
        vec!["limit"]
    );
    assert!(!dir.exists());
}

#[test]
fn the_limits_come_from_the_properties() {
    let props = |name: &str| match name {
        "call_recording_queue_frames" => "1000".to_string(),
        "call_recording_segment_mb" => "8".to_string(),
        "call_recording_segment_minutes" => "5".to_string(),
        "call_recording_max_mb" => "x".to_string(),
        _ => String::new(),
    };
    let limits = Limits::from_properties(&props, true);
    assert_eq!(limits.queue_frames, 1_000);
    assert!(!limits.record.tracks && limits.record.mix);
    assert_eq!(limits.record.max_segment_bytes, 8 * 1024 * 1024);
    assert_eq!(limits.record.max_segment_ms, 5 * 60_000);
    assert_eq!(
        limits.record.max_total_bytes,
        2_048 * 1024 * 1024,
        "a bad number is the default"
    );
}
