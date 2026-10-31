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

//! Captions with a fake transcriber: silence and short noises make no chunk; chunks end at a pause or the longest
//! length; results go out in speaker and chunk order when they finish out of order (real Opus through the room, on
//! simulated time); a timeout makes captions unavailable once; backpressure drops the oldest waiting chunk and marks
//! its gap; and a provider that hangs never stops the loop taking the call's frames.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::llm_hooks::SpeechError;
use moqui_lib::sync::lock;
use moqui_rtc::audio::opus::{OpusEncoder, VOICE_BITRATE};
use moqui_rtc::audio::FRAME;
use moqui_rtc::tap::{Kind, Kinds as TapKinds, MediaTap, OwnedFrame, QueueTap, TapFrame, TapReceiver};
use moqui_rtc::Source;
use serde_json::Value as Json;

use super::*;

const STEP: Duration = Duration::from_millis(20);

fn tone(at: usize, amplitude: f32) -> Vec<i16> {
    (0..FRAME)
        .map(|i| {
            let t = (at * FRAME + i) as f32 / 48_000.0;
            (amplitude * (2.0 * std::f32::consts::PI * 300.0 * t).sin()) as i16
        })
        .collect()
}

fn leg(name: &str) -> Arc<str> {
    Arc::from(name)
}

fn settings() -> Settings {
    Settings {
        workers: 3,
        queue: 4,
        timeout: Duration::from_secs(10),
        ..Settings::default()
    }
}

/// Answers each call with `chunk <n>` (n from 1), or the error set for that call; the sim takes each call's delay.
struct Fake {
    calls: Mutex<u32>,
    delays: Vec<Duration>,
    fail: Vec<u32>,
}

impl Fake {
    fn new(delays: &[u64]) -> Self {
        Fake {
            calls: Mutex::new(0),
            delays: delays.iter().map(|ms| Duration::from_millis(*ms)).collect(),
            fail: Vec::new(),
        }
    }

    fn delay(&self, call: u32) -> Duration {
        self.delays
            .get(call as usize - 1)
            .copied()
            .unwrap_or(Duration::from_millis(10))
    }
}

impl Transcriber for Fake {
    fn transcribe(&self, wav: &[u8]) -> Result<String, SpeechError> {
        assert_eq!(&wav[..4], b"RIFF");
        let mut calls = lock(&self.calls);
        *calls += 1;
        if self.fail.contains(&calls) {
            return Err(SpeechError::new("failed", "the endpoint answered HTTP 500"));
        }
        Ok(format!("chunk {calls}"))
    }
}

/// Simulated time over a captioner: each started job finishes after the fake's delay for it.
struct Sim {
    now: Instant,
    fake: Fake,
    running: Vec<(Instant, u64, Result<String, SpeechError>)>,
    started: Vec<(Arc<str>, u64)>,
    out: Vec<Out>,
}

impl Sim {
    fn new(fake: Fake) -> Self {
        Sim {
            now: Instant::now(),
            fake,
            running: Vec::new(),
            started: Vec::new(),
            out: Vec::new(),
        }
    }

    /// One 20 ms turn of the loop, after the captioner ticked.
    fn turn(&mut self, captioner: &mut Captioner) {
        let now = self.now;
        let (due, rest): (Vec<_>, Vec<_>) = self.running.drain(..).partition(|(at, _, _)| *at <= now);
        self.running = rest;
        for (_, id, result) in due {
            captioner.pipeline.finish(id, result);
        }
        captioner.pipeline.expire(now);
        for job in captioner.pipeline.start(now, usize::MAX) {
            let result = self.fake.transcribe(&job.wav);
            let call = *lock(&self.fake.calls);
            self.started.push((job.leg.clone(), job.seq));
            self.running.push((now + self.fake.delay(call), job.id, result));
        }
        self.out.extend(captioner.pipeline.drain());
        self.now += STEP;
    }
}

fn captions(out: &[Out]) -> Vec<(String, u64, String)> {
    out.iter()
        .filter_map(|o| match o {
            Out::Caption { leg, seq, text } => Some((leg.to_string(), *seq, text.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn silence_and_a_short_noise_make_no_chunk() {
    let mut captioner = Captioner::new(&settings());
    let a = leg("A");
    for i in 0..150 {
        // silence, then room noise under the speech level, then a click of 100 ms
        let frame = match i {
            0..=49 => vec![0; FRAME],
            50..=139 => tone(i, 40.0),
            _ => tone(i, 8_000.0),
        };
        captioner.feed(&a, Some(&frame));
    }
    for _ in 0..50 {
        captioner.feed(&a, None);
    }
    assert!(captioner.pipeline.start(Instant::now(), usize::MAX).is_empty());
    assert!(captioner.pipeline.drain().is_empty());
}

#[test]
fn a_pause_or_the_longest_chunk_ends_a_chunk_as_16_khz_wav() {
    let mut captioner = Captioner::new(&Settings {
        max_chunk: Duration::from_secs(2),
        ..settings()
    });
    let a = leg("A");
    // one second of speech, then a pause
    for i in 0..50 {
        captioner.feed(&a, Some(&tone(i, 8_000.0)));
    }
    for _ in 0..30 {
        captioner.feed(&a, None);
    }
    let jobs = captioner.pipeline.start(Instant::now(), usize::MAX);
    assert_eq!(jobs.len(), 1);
    let wav = &jobs[0].wav;
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), SPEECH_RATE);
    // 1.6 s at 16 kHz (the speech and the pause that ended it)
    assert_eq!(wav.len(), 44 + 2 * 16_000 * 1_600 / 1_000);
    // five seconds of talking with no pause: cut at two seconds
    for i in 0..250 {
        captioner.feed(&a, Some(&tone(i, 8_000.0)));
    }
    let jobs = captioner.pipeline.start(Instant::now(), usize::MAX);
    assert_eq!(jobs.len(), 2, "two full chunks, the rest still open");
    assert!(jobs.iter().all(|job| job.wav.len() == 44 + 2 * 32_000));
    assert_eq!((jobs[0].seq, jobs[1].seq), (2, 3));
}

/// Real Opus frames of two speakers through the room, on simulated time: A speaks, then B, then A again; A's first
/// chunk takes longest, so B's and A's second finish first and wait for it.
#[test]
fn captions_go_out_in_speaker_and_chunk_order_when_they_finish_out_of_order() {
    let mut captioner = Captioner::new(&settings());
    let mut sim = Sim::new(Fake::new(&[3_000, 60, 40]));
    let (a, b) = (leg("A"), leg("B"));
    let mut encoders = [
        OpusEncoder::new(VOICE_BITRATE).unwrap(),
        OpusEncoder::new(VOICE_BITRATE).unwrap(),
    ];
    // who talks in each 20 ms: A 0-0.5 s, B 1.2-1.7 s, A 2.4-2.9 s; each leg sends silence otherwise
    for i in 0..250 {
        for (n, speaker) in [&a, &b].into_iter().enumerate() {
            let talking = match n {
                0 => (0..25).contains(&i) || (120..145).contains(&i),
                _ => (60..85).contains(&i),
            };
            let pcm = if talking { tone(i, 8_000.0) } else { vec![0; FRAME] };
            let data = encoders[n].encode(&pcm).unwrap().to_vec();
            captioner.take(&TapEvent::Frame(OwnedFrame {
                leg: speaker.clone(),
                kind: Kind::Audio,
                source: Source::Microphone,
                rid: None,
                codec: "opus",
                rtp_time: (i * FRAME) as u64,
                clock_rate: 48_000,
                data,
                keyframe: false,
                audio_level: None,
            }));
        }
        captioner.tick();
        sim.turn(&mut captioner);
    }
    assert_eq!(
        sim.started,
        vec![(a.clone(), 1), (b.clone(), 1), (a.clone(), 2)],
        "each speaker's chunks, numbered in order"
    );
    assert_eq!(
        captions(&sim.out),
        vec![
            ("A".to_string(), 1, "chunk 1".to_string()),
            ("B".to_string(), 1, "chunk 2".to_string()),
            ("A".to_string(), 2, "chunk 3".to_string()),
        ]
    );
}

#[test]
fn results_wait_for_every_earlier_chunk() {
    let mut pipeline = Pipeline::new(&settings());
    let (a, b) = (leg("A"), leg("B"));
    let now = Instant::now();
    let ids = [
        pipeline.submit(&a, vec![1]),
        pipeline.submit(&b, vec![2]),
        pipeline.submit(&a, vec![3]),
    ];
    assert_eq!(pipeline.start(now, usize::MAX).len(), 3);
    pipeline.finish(ids[2], Ok("third".to_string()));
    pipeline.finish(ids[1], Ok("second".to_string()));
    assert!(pipeline.drain().is_empty(), "the first is not back");
    pipeline.finish(ids[0], Ok("first".to_string()));
    assert_eq!(
        captions(&pipeline.drain()),
        vec![
            ("A".to_string(), 1, "first".to_string()),
            ("B".to_string(), 1, "second".to_string()),
            ("A".to_string(), 2, "third".to_string()),
        ]
    );
}

#[test]
fn a_provider_timeout_makes_captions_unavailable_once_and_a_late_answer_is_ignored() {
    let mut pipeline = Pipeline::new(&Settings {
        timeout: Duration::from_millis(500),
        workers: 1,
        ..settings()
    });
    let a = leg("A");
    let t0 = Instant::now();
    let first = pipeline.submit(&a, vec![1]);
    let second = pipeline.submit(&a, vec![2]);
    assert_eq!(pipeline.start(t0, usize::MAX).len(), 1);
    assert_eq!(pipeline.next_deadline(), Some(t0 + Duration::from_millis(500)));
    pipeline.expire(t0 + Duration::from_millis(499));
    assert!(pipeline.drain().is_empty());
    pipeline.expire(t0 + Duration::from_millis(500));
    assert_eq!(
        pipeline.drain(),
        vec![Out::Unavailable {
            reason: "timeout",
            message: "the provider did not answer in time".to_string()
        }]
    );
    // the worker is free again; the late answer changes nothing
    pipeline.finish(first, Ok("late".to_string()));
    let started = pipeline.start(t0 + Duration::from_millis(500), usize::MAX);
    assert_eq!(started.len(), 1);
    // a second failure says nothing new; a success brings captions back
    pipeline.finish(second, Err(SpeechError::new("failed", "HTTP 500")));
    assert!(pipeline.drain().is_empty());
    let third = pipeline.submit(&a, vec![3]);
    pipeline.start(t0 + Duration::from_secs(1), usize::MAX);
    pipeline.finish(third, Ok("back".to_string()));
    assert_eq!(
        pipeline.drain(),
        vec![
            Out::Available,
            Out::Caption {
                leg: a.clone(),
                seq: 3,
                text: "back".to_string()
            }
        ]
    );
}

#[test]
fn backpressure_drops_the_oldest_waiting_chunk_and_marks_its_gap() {
    let mut pipeline = Pipeline::new(&Settings {
        workers: 1,
        queue: 2,
        ..settings()
    });
    let (a, b) = (leg("A"), leg("B"));
    let now = Instant::now();
    let first = pipeline.submit(&a, vec![1]);
    assert_eq!(pipeline.start(now, usize::MAX).len(), 1);
    // the worker is busy: B1 and A2 wait, and B2 pushes B1 out
    pipeline.submit(&b, vec![2]);
    pipeline.submit(&a, vec![3]);
    pipeline.submit(&b, vec![4]);
    assert_eq!(pipeline.dropped(), 1);
    pipeline.finish(first, Ok("a one".to_string()));
    let mut out = pipeline.drain();
    for _ in 0..2 {
        let jobs = pipeline.start(now, usize::MAX);
        assert_eq!(jobs.len(), 1);
        pipeline.finish(jobs[0].id, Ok(format!("{} {}", jobs[0].leg, jobs[0].seq)));
        out.extend(pipeline.drain());
    }
    assert_eq!(
        out,
        vec![
            Out::Caption {
                leg: a.clone(),
                seq: 1,
                text: "a one".to_string()
            },
            Out::Gap { leg: b.clone(), seq: 1 },
            Out::Caption {
                leg: a.clone(),
                seq: 2,
                text: "A 2".to_string()
            },
            Out::Caption {
                leg: b.clone(),
                seq: 2,
                text: "B 2".to_string()
            },
        ]
    );
}

#[test]
fn the_settings_come_from_the_properties_within_bounds() {
    let props = |name: &str| {
        match name {
            "call_captions_chunk_seconds" => "99",
            "call_captions_queue" => "3",
            "call_captions_workers" => "0",
            "call_captions_store" => "true",
            "call_captions_provider" => "local",
            "call_captions_language" => "de",
            _ => "",
        }
        .to_string()
    };
    let settings = Settings::from_properties(&props);
    assert_eq!(settings.max_chunk, Duration::from_secs(30));
    assert_eq!((settings.queue, settings.workers), (3, 1));
    assert!(settings.store);
    assert_eq!(settings.provider, "local");
    assert_eq!(settings.language.as_deref(), Some("de"));
    assert_eq!(Settings::from_properties(&|_: &str| String::new()), Settings::default());
    // speech-local is not built in: its captions are unavailable with that reason
    let local = LocalTranscriber { language: None };
    assert_eq!(
        local.transcribe(&wav(&[0; 16], SPEECH_RATE)).unwrap_err().code,
        "notBuiltIn"
    );
}

// ---------- the thread ----------

impl Events for TapReceiver {
    fn next(&self, wait: Duration) -> Result<TapEvent, RecvTimeoutError> {
        self.recv_timeout(wait)
    }
}

/// Records what the loop sends; jobs run on threads of their own.
#[derive(Default, Clone)]
struct Recorded {
    frames: Arc<Mutex<Vec<Json>>>,
    stored: Arc<Mutex<Vec<String>>>,
}

impl Outside for Recorded {
    fn send(&mut self, frame: Json) {
        lock(&self.frames).push(frame);
    }

    fn speaker(&mut self, leg: &str) -> Speaker {
        Speaker {
            participant: Some(format!("P-{leg}")),
            name: Some(format!("Speaker {leg}")),
        }
    }

    fn store(&mut self, _leg: &str, _seq: u64, _speaker: &Speaker, text: &str) {
        lock(&self.stored).push(text.to_string());
    }

    fn spawn(&mut self, job: Box<dyn FnOnce() + Send>) {
        std::thread::spawn(job);
    }
}

/// A provider that never answers until the test lets it go.
struct Hung(Mutex<mpsc::Receiver<()>>);

impl Transcriber for Hung {
    fn transcribe(&self, _wav: &[u8]) -> Result<String, SpeechError> {
        let _ = lock(&self.0).recv();
        Err(SpeechError::new("failed", "let go"))
    }
}

fn send_frame(tap: &mut QueueTap, encoder: &mut OpusEncoder, speaker: &Arc<str>, i: usize, loud: bool) {
    let pcm = if loud { tone(i, 8_000.0) } else { vec![0; FRAME] };
    let data = encoder.encode(&pcm).unwrap().to_vec();
    tap.on_frame(&TapFrame {
        leg: speaker,
        kind: Kind::Audio,
        source: Source::Microphone,
        rid: None,
        codec: "opus",
        rtp_time: (i * FRAME) as u64,
        clock_rate: 48_000,
        data: &data,
        keyframe: false,
        audio_level: None,
    });
}

/// The provider hangs: the loop still takes every frame (the tap drops none), says captions are unavailable when the
/// chunk times out, and stops at once when told.
#[test]
fn a_hung_provider_never_stops_the_loop_taking_the_calls_frames() {
    let (mut tap, receiver) = QueueTap::channel(8, TapKinds::Audio);
    let (release, hold) = mpsc::channel();
    let transcriber: Arc<dyn Transcriber> = Arc::new(Hung(Mutex::new(hold)));
    let recorded = Recorded::default();
    let stop = Arc::new(AtomicBool::new(false));
    let thread = {
        let mut outside = recorded.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let settings = Settings {
                timeout: Duration::from_millis(150),
                ..settings()
            };
            run(&receiver, &mut outside, &transcriber, &settings, &stop);
            receiver
        })
    };
    let (a, b) = (leg("A"), leg("B"));
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).unwrap();
    // 0.4 s of A talking, then A leaves (its chunk goes out); B keeps sending for 0.4 s more
    for i in 0..20 {
        send_frame(&mut tap, &mut encoder, &a, i, true);
        std::thread::sleep(STEP);
    }
    tap.on_leave(&a);
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).unwrap();
    for i in 0..20 {
        send_frame(&mut tap, &mut encoder, &b, i, false);
        std::thread::sleep(STEP);
    }
    let frames = lock(&recorded.frames).clone();
    assert_eq!(
        frames
            .iter()
            .map(|f| (f["rtc"].as_str(), f["state"].as_str(), f["reason"].as_str()))
            .collect::<Vec<_>>(),
        vec![(Some("captions"), Some("unavailable"), Some("timeout"))],
        "{frames:?}"
    );
    let stopping = Instant::now();
    stop.store(true, Ordering::Release);
    let receiver = thread.join().unwrap();
    assert!(stopping.elapsed() < Duration::from_millis(200));
    assert_eq!(receiver.stats().dropped(), 0, "the tap never had to drop a frame");
    assert_eq!(receiver.stats().delivered(), 40);
    let _ = release.send(());
}

#[test]
fn a_caption_names_its_speaker_and_is_kept_only_when_asked() {
    let (mut tap, receiver) = QueueTap::channel(64, TapKinds::Audio);
    let recorded = Recorded::default();
    let stop = Arc::new(AtomicBool::new(false));
    let transcriber: Arc<dyn Transcriber> = Arc::new(Fake::new(&[]));
    let thread = {
        let mut outside = recorded.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let settings = Settings {
                store: true,
                ..settings()
            };
            run(&receiver, &mut outside, &transcriber, &settings, &stop);
        })
    };
    let a = leg("A");
    let mut encoder = OpusEncoder::new(VOICE_BITRATE).unwrap();
    for i in 0..20 {
        send_frame(&mut tap, &mut encoder, &a, i, true);
        std::thread::sleep(STEP);
    }
    tap.on_leave(&a);
    let waited = Instant::now();
    while lock(&recorded.frames).is_empty() && waited.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Ordering::Release);
    thread.join().unwrap();
    let frames = lock(&recorded.frames).clone();
    assert_eq!(frames.len(), 1, "{frames:?}");
    let caption = &frames[0];
    assert_eq!(
        (
            caption["rtc"].as_str(),
            caption["leg"].as_str(),
            caption["participant"].as_str(),
            caption["name"].as_str(),
            caption["seq"].as_u64(),
            caption["text"].as_str()
        ),
        (
            Some("caption"),
            Some("A"),
            Some("P-A"),
            Some("Speaker A"),
            Some(1),
            Some("chunk 1")
        )
    );
    assert_eq!(*lock(&recorded.stored), vec!["chunk 1".to_string()]);
}

#[test]
fn the_node_cap_limits_the_jobs_started_and_frees_a_place_as_each_ends() {
    let permits = Permits::default();
    assert_eq!(permits.reserve(3, 4), 3);
    assert_eq!(permits.reserve(3, 4), 1, "one place left under the cap");
    assert_eq!(permits.reserve(1, 4), 0);
    permits.release();
    assert_eq!(permits.reserve(2, 4), 1, "a place freed is taken again");
    for _ in 0..10 {
        permits.release();
    }
    assert_eq!(permits.reserve(9, 4), 4, "never below none");

    // a call's pipeline starts only what the node allows; the rest waits its turn
    let mut pipeline = Pipeline::new(&settings());
    let a = leg("A");
    for n in 0..3 {
        pipeline.submit(&a, vec![n]);
    }
    assert_eq!(pipeline.wanted(), 3);
    assert_eq!(pipeline.start(Instant::now(), 1).len(), 1);
    assert_eq!(pipeline.wanted(), 2);
    assert_eq!(pipeline.start(Instant::now(), usize::MAX).len(), 2);
    assert_eq!(pipeline.wanted(), 0);
}
