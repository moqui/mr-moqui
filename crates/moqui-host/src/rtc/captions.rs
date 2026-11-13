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

//! Live captions of a call (plans/archive/Comms-Framework.md §6.4, W6), on the call's media node. `start#Captions` (framework
//! CallCaptionServices) checks the policy and sends `startCaptions` through `send#MediaControl`; the node's
//! `rtc-signal` handler calls [`start`], which attaches a consumer ([`super::media::attach`], audio only) and runs one
//! thread for the call. Every leg is told captions are on and which provider hears the audio
//! (`{rtc: 'captions', state: 'on', provider, local}`, the notice) before the consumer is attached; the driver keeps it
//! and sends it to each leg that joins later, a guest's too ([`super::Command::RoomNotice`]). A notice the driver does
//! not take starts no captions. The thread:
//!
//! - every frame goes into an [`AudioRoom`] and every 20 ms the room ticks;
//! - each leg's own audio ([`AudioRoom::leg_audio`]) is gated by its level ([`SPEECH_DBOV`]) and cut into chunks at a
//!   pause ([`PAUSE`]) or at `call_captions_chunk_seconds` (5); a chunk with less than [`MIN_SPEECH`] of speech, and
//!   silence, makes nothing;
//! - a chunk is resampled to 16 kHz and WAV-encoded, and waits in a bounded queue (`call_captions_queue`, 6) for one
//!   of `call_captions_workers` (2) jobs on the service pool, each given `call_captions_timeout_seconds` (20); the
//!   node's calls together run at most half the pool's workers ([`Permits`]), so a provider that hangs cannot take it;
//! - the results go out in the order the chunks were cut (so in each speaker's order), as `rtc` frames to every leg of
//!   the call through the driver ([`super::Command::ToRoom`]): `{rtc: 'caption', leg, participant, name, seq, text}`.
//!
//! **Backpressure.** A full queue drops its oldest chunk, and that chunk's place in the order goes out as a gap
//! (`{rtc: 'caption', leg, seq, gap: true}`). **Failure.** A provider error or timeout goes out once as
//! `{rtc: 'captions', state: 'unavailable', reason, message}`; the next caption that works brings `state: 'on'` back.
//! Nothing here runs on the media thread or can make it wait: the tap drops frames when this thread is behind
//! (counted), and a provider that hangs holds a worker, never this loop. Captions stop with `stop#Captions`
//! ([`stop`]: `state: 'off'`) or when the call's room closes.
//!
//! The transcriber is a trait ([`Transcriber`]): the `moqui-llm` endpoint ([`moqui_core::llm_hooks::transcribe`]), or
//! `speech-local` (`call_captions_provider=local`, not built yet, see `crate::speech_local`). With
//! `call_captions_store=true` each caption is kept as a `CallTranscript` row (`store#CallTranscript`); by default
//! nothing is stored. Speakers are named by `get#CaptionSpeaker`. On an edge node (plans/Edge-Node.md §5.4) those two
//! run on its local node (delegated), and so does transcription (`transcribe#CaptionAudio`, [`DelegatedTranscriber`]),
//! where the call's rows and the agent runtime's quota are.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::llm_hooks::{SpeechError, SpeechRequest};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;
use moqui_rtc::audio::resample::Resampler;
use moqui_rtc::audio::room::AudioRoom;
use moqui_rtc::audio::{level_dbov, FRAME, FRAME_MS};
use serde_json::{json, Value as Json};

use super::media::{Kinds, TapEvent};
use super::{Command, NoticeKind};

/// A frame this loud or louder (RMS, dBov) is speech.
pub(crate) const SPEECH_DBOV: f32 = -50.0;
/// A pause this long ends a chunk.
pub(crate) const PAUSE: Duration = Duration::from_millis(600);
/// A chunk with less speech than this is dropped (a cough, a click).
pub(crate) const MIN_SPEECH: Duration = Duration::from_millis(300);
/// The rate a chunk is sent at.
pub(crate) const SPEECH_RATE: u32 = 16_000;
/// The frames the tap holds for this thread: 4 seconds of every leg's audio at five legs.
const TAP_QUEUE: usize = 1_000;
const TICK: Duration = Duration::from_millis(FRAME_MS);

/// The settings, from the properties (docs/Run-Deploy.md).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settings {
    /// The longest chunk.
    pub max_chunk: Duration,
    /// Chunks waiting for a worker; past this the oldest is dropped.
    pub queue: usize,
    /// Chunks being transcribed at once.
    pub workers: usize,
    /// How long one chunk may take.
    pub timeout: Duration,
    /// Keep each caption as a `CallTranscript`.
    pub store: bool,
    /// `llm` (the moqui-llm endpoint) or `local` (speech-local).
    pub provider: String,
    pub language: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_chunk: Duration::from_secs(5),
            queue: 6,
            workers: 2,
            timeout: Duration::from_secs(20),
            store: false,
            provider: "llm".to_string(),
            language: None,
        }
    }
}

impl Settings {
    pub(crate) fn from_properties(property: &dyn Fn(&str) -> String) -> Settings {
        let base = Settings::default();
        let number = |name: &str, low: u64, high: u64, default: u64| {
            property(name)
                .trim()
                .parse::<u64>()
                .map_or(default, |value| value.clamp(low, high))
        };
        let language = property("call_captions_language").trim().to_string();
        Settings {
            max_chunk: Duration::from_secs(number("call_captions_chunk_seconds", 1, 30, 5)),
            queue: number("call_captions_queue", 1, 100, base.queue as u64) as usize,
            workers: number("call_captions_workers", 1, 16, base.workers as u64) as usize,
            timeout: Duration::from_secs(number("call_captions_timeout_seconds", 1, 300, 20)),
            store: property("call_captions_store").trim() == "true",
            provider: match property("call_captions_provider").trim() {
                "local" => "local".to_string(),
                _ => "llm".to_string(),
            },
            language: (!language.is_empty()).then_some(language),
        }
    }
}

/// Speech to text of one chunk: 16 kHz mono WAV.
pub(crate) trait Transcriber: Send + Sync + 'static {
    fn transcribe(&self, wav: &[u8]) -> Result<String, SpeechError>;
}

// ---------- chunking ----------

/// One leg's audio cut into chunks of speech.
struct Chunker {
    resampler: Resampler,
    /// The chunk so far, at 16 kHz.
    samples: Vec<i16>,
    open: bool,
    frames: u32,
    speech: u32,
    pause: u32,
}

impl Chunker {
    fn new() -> Self {
        Chunker {
            resampler: Resampler::new(moqui_rtc::audio::RATE, SPEECH_RATE),
            samples: Vec::new(),
            open: false,
            frames: 0,
            speech: 0,
            pause: 0,
        }
    }

    /// The leg's next 20 ms (`None`: it sent nothing); a finished chunk at 16 kHz when this frame ends one.
    fn push(&mut self, pcm: Option<&[i16]>, max_chunk: Duration) -> Option<Vec<i16>> {
        let voiced = pcm.is_some_and(|pcm| level_dbov(pcm) >= SPEECH_DBOV);
        if !self.open {
            if !voiced {
                return None;
            }
            self.open = true;
            self.samples.clear();
            self.resampler = Resampler::new(moqui_rtc::audio::RATE, SPEECH_RATE);
            self.frames = 0;
            self.speech = 0;
            self.pause = 0;
        }
        match pcm {
            Some(pcm) => self.resampler.process(pcm, &mut self.samples),
            None => self.resampler.process(&[0; FRAME], &mut self.samples),
        }
        self.frames += 1;
        if voiced {
            self.speech += 1;
            self.pause = 0;
        } else {
            self.pause += 1;
        }
        let ms = |frames: u32| u128::from(frames) * u128::from(FRAME_MS);
        if ms(self.pause) >= PAUSE.as_millis() || ms(self.frames) >= max_chunk.as_millis() {
            return self.finish();
        }
        None
    }

    /// Ends the chunk (a pause, the longest chunk, the leg leaving): its samples, when it had enough speech.
    fn finish(&mut self) -> Option<Vec<i16>> {
        if !self.open {
            return None;
        }
        self.open = false;
        let enough = u128::from(self.speech) * u128::from(FRAME_MS) >= MIN_SPEECH.as_millis();
        enough.then(|| std::mem::take(&mut self.samples))
    }
}

/// 16-bit mono PCM as a WAV file.
pub(crate) fn wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

// ---------- the queue and the order ----------

/// A chunk to transcribe: its place in the call's order (`id`), its leg and its number among that leg's chunks.
pub(crate) struct Job {
    pub id: u64,
    pub leg: Arc<str>,
    pub seq: u64,
    pub wav: Vec<u8>,
}

enum Slot {
    Waiting,
    Running(Instant),
    Done(Result<String, SpeechError>),
    /// Dropped by backpressure: a gap.
    Dropped,
}

/// What goes out, in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Out {
    Caption {
        leg: Arc<str>,
        seq: u64,
        text: String,
    },
    Gap {
        leg: Arc<str>,
        seq: u64,
    },
    /// Captions work again after being unavailable.
    Available,
    Unavailable {
        reason: &'static str,
        message: String,
    },
}

/// The chunks of a call between being cut and going out: a bounded queue of waiting chunks, the running ones with
/// their deadlines, and the results held until every earlier chunk has its own.
pub(crate) struct Pipeline {
    queue: usize,
    workers: usize,
    timeout: Duration,
    next_id: u64,
    next_seq: HashMap<Arc<str>, u64>,
    waiting: VecDeque<Job>,
    /// Every chunk not yet out, by id: (leg, seq, state).
    slots: BTreeMap<u64, (Arc<str>, u64, Slot)>,
    running: usize,
    unavailable: bool,
    dropped: u64,
}

impl Pipeline {
    pub(crate) fn new(settings: &Settings) -> Self {
        Pipeline {
            queue: settings.queue.max(1),
            workers: settings.workers.max(1),
            timeout: settings.timeout,
            next_id: 1,
            next_seq: HashMap::new(),
            waiting: VecDeque::new(),
            slots: BTreeMap::new(),
            running: 0,
            unavailable: false,
            dropped: 0,
        }
    }

    /// A chunk of `leg` is cut: it waits for a worker. When the queue is full the oldest waiting chunk is dropped.
    pub(crate) fn submit(&mut self, leg: &Arc<str>, wav: Vec<u8>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let next = self.next_seq.entry(leg.clone()).or_insert(1);
        let seq = *next;
        *next += 1;
        if self.waiting.len() >= self.queue {
            if let Some(oldest) = self.waiting.pop_front() {
                self.dropped += 1;
                if let Some(slot) = self.slots.get_mut(&oldest.id) {
                    slot.2 = Slot::Dropped;
                }
            }
        }
        self.slots.insert(id, (leg.clone(), seq, Slot::Waiting));
        self.waiting.push_back(Job {
            id,
            leg: leg.clone(),
            seq,
            wav,
        });
        id
    }

    /// The chunks that could start now: free workers with a chunk waiting for each.
    pub(crate) fn wanted(&self) -> usize {
        self.workers.saturating_sub(self.running).min(self.waiting.len())
    }

    /// The chunks that start now, as workers are free and no more than `limit` (the node's free caption jobs,
    /// [`Outside::reserve`]); each must end by `now` plus the timeout.
    pub(crate) fn start(&mut self, now: Instant, limit: usize) -> Vec<Job> {
        let mut started = Vec::new();
        while self.running < self.workers && started.len() < limit {
            let Some(job) = self.waiting.pop_front() else { break };
            if let Some(slot) = self.slots.get_mut(&job.id) {
                slot.2 = Slot::Running(now + self.timeout);
            }
            self.running += 1;
            started.push(job);
        }
        started
    }

    /// A worker's answer. One after its chunk timed out is ignored (its worker was counted free then).
    pub(crate) fn finish(&mut self, id: u64, result: Result<String, SpeechError>) {
        if let Some(slot) = self.slots.get_mut(&id) {
            if matches!(slot.2, Slot::Running(_)) {
                slot.2 = Slot::Done(result);
                self.running = self.running.saturating_sub(1);
            }
        }
    }

    /// Chunks past their deadline time out.
    pub(crate) fn expire(&mut self, now: Instant) {
        for (_, _, slot) in self.slots.values_mut() {
            if let Slot::Running(deadline) = slot {
                if *deadline <= now {
                    *slot = Slot::Done(Err(SpeechError::new("timeout", "the provider did not answer in time")));
                    self.running = self.running.saturating_sub(1);
                }
            }
        }
    }

    /// What can go out now: the results of the chunks from the first on, up to the first still waiting or running.
    pub(crate) fn drain(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        while let Some(entry) = self.slots.first_entry() {
            if matches!(entry.get().2, Slot::Waiting | Slot::Running(_)) {
                break;
            }
            let (leg, seq, slot) = entry.remove();
            match slot {
                Slot::Done(Ok(text)) => {
                    if self.unavailable {
                        self.unavailable = false;
                        out.push(Out::Available);
                    }
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        out.push(Out::Caption { leg, seq, text });
                    }
                }
                Slot::Done(Err(error)) => {
                    if !self.unavailable {
                        self.unavailable = true;
                        out.push(Out::Unavailable {
                            reason: error.code,
                            message: error.message,
                        });
                    }
                }
                Slot::Dropped => out.push(Out::Gap { leg, seq }),
                Slot::Waiting | Slot::Running(_) => {}
            }
        }
        out
    }

    /// When the first running chunk times out.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.slots
            .values()
            .filter_map(|(_, _, slot)| match slot {
                Slot::Running(deadline) => Some(*deadline),
                _ => None,
            })
            .min()
    }

    /// Chunks dropped by backpressure so far.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}

// ---------- one call ----------

/// A call's captions without the threads: the room's audio, a chunker per leg and the pipeline.
pub(crate) struct Captioner {
    room: AudioRoom,
    chunkers: HashMap<Arc<str>, Chunker>,
    pub(crate) pipeline: Pipeline,
    max_chunk: Duration,
}

impl Captioner {
    pub(crate) fn new(settings: &Settings) -> Self {
        Captioner {
            room: AudioRoom::default(),
            chunkers: HashMap::new(),
            pipeline: Pipeline::new(settings),
            max_chunk: settings.max_chunk,
        }
    }

    /// A frame or a leave from the tap. A leg that leaves ends its chunk.
    pub(crate) fn take(&mut self, event: &TapEvent) {
        if let TapEvent::Left(leg) = event {
            if let Some(samples) = self.chunkers.get_mut(leg).and_then(Chunker::finish) {
                self.pipeline.submit(leg, wav(&samples, SPEECH_RATE));
            }
            self.chunkers.remove(leg);
        }
        self.room.take(event);
    }

    /// The next 20 ms of every leg: decoded, gated and chunked.
    pub(crate) fn tick(&mut self) {
        self.room.tick();
        let mut legs: Vec<Arc<str>> = self.room.legs().cloned().collect();
        legs.extend(
            self.chunkers
                .keys()
                .filter(|leg| !legs.contains(leg))
                .cloned()
                .collect::<Vec<_>>(),
        );
        legs.sort();
        legs.dedup();
        for leg in legs {
            let audio = self.room.leg_audio(&leg);
            let chunker = self.chunkers.entry(leg.clone()).or_insert_with(Chunker::new);
            if let Some(samples) = chunker.push(audio, self.max_chunk) {
                self.pipeline.submit(&leg, wav(&samples, SPEECH_RATE));
            }
            if audio.is_none() && !chunker.open {
                self.chunkers.remove(&leg);
            }
        }
    }

    /// A leg's next 20 ms straight to its chunker (the tests' way in, past the decoder).
    #[cfg(test)]
    pub(crate) fn feed(&mut self, leg: &Arc<str>, pcm: Option<&[i16]>) {
        let chunker = self.chunkers.entry(leg.clone()).or_insert_with(Chunker::new);
        if let Some(samples) = chunker.push(pcm, self.max_chunk) {
            self.pipeline.submit(leg, wav(&samples, SPEECH_RATE));
        }
    }
}

// ---------- the thread ----------

/// Where the frames come from: the tap's consumer, or a test's queue.
pub(crate) trait Events {
    fn next(&self, wait: Duration) -> Result<TapEvent, RecvTimeoutError>;
}

impl Events for super::media::Consumer {
    fn next(&self, wait: Duration) -> Result<TapEvent, RecvTimeoutError> {
        self.recv_timeout(wait)
    }
}

/// Who a leg is, for its captions.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Speaker {
    pub participant: Option<String>,
    pub name: Option<String>,
}

/// What the loop does outside itself: send a frame to the call's legs, name a speaker, keep a caption, run a job.
pub(crate) trait Outside {
    fn send(&mut self, frame: Json);
    fn speaker(&mut self, leg: &str) -> Speaker;
    fn store(&mut self, leg: &str, seq: u64, speaker: &Speaker, text: &str);
    /// How many of `wanted` jobs may start now; each one started is then given to [`Self::spawn`], which frees its
    /// place when the job ends. The node caps the jobs of all its calls ([`Permits`]); by default there is no cap.
    fn reserve(&mut self, wanted: usize) -> usize {
        wanted
    }
    fn spawn(&mut self, job: Box<dyn FnOnce() + Send>);
}

/// The caption jobs running on a node, across its calls, against a cap: a provider that hangs past the timeout keeps
/// its job (and its service-pool thread) until the call returns, and this keeps those from taking the whole pool.
#[derive(Default)]
pub(crate) struct Permits {
    used: Mutex<usize>,
}

impl Permits {
    /// Takes up to `wanted` places under `cap`; how many it took.
    pub(crate) fn reserve(&self, wanted: usize, cap: usize) -> usize {
        let mut used = lock(&self.used);
        let taken = wanted.min(cap.saturating_sub(*used));
        *used += taken;
        taken
    }

    /// Gives one place back.
    pub(crate) fn release(&self) {
        let mut used = lock(&self.used);
        *used = used.saturating_sub(1);
    }
}

/// Runs a call's captions until `stop` is set or the events end (the room closed).
pub(crate) fn run(
    events: &dyn Events,
    outside: &mut dyn Outside,
    transcriber: &Arc<dyn Transcriber>,
    settings: &Settings,
    stop: &AtomicBool,
) {
    let mut captioner = Captioner::new(settings);
    let (done_tx, done_rx) = mpsc::channel::<(u64, Result<String, SpeechError>)>();
    let mut speakers: HashMap<Arc<str>, Speaker> = HashMap::new();
    let mut next_tick = Instant::now() + TICK;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        // until the next tick, or sooner when a chunk times out first
        let until = captioner
            .pipeline
            .next_deadline()
            .map_or(next_tick, |deadline| deadline.min(next_tick));
        let wait = until.saturating_duration_since(Instant::now());
        match events.next(wait.max(Duration::from_millis(1))) {
            Ok(event) => captioner.take(&event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        if now >= next_tick {
            captioner.tick();
            next_tick += TICK;
            // far behind (a stall): skip ahead rather than ticking in a burst
            if now.saturating_duration_since(next_tick) > Duration::from_millis(200) {
                next_tick = now + TICK;
            }
        }
        while let Ok((id, result)) = done_rx.try_recv() {
            captioner.pipeline.finish(id, result);
        }
        captioner.pipeline.expire(now);
        let wanted = captioner.pipeline.wanted();
        let allowed = if wanted > 0 { outside.reserve(wanted) } else { 0 };
        for job in captioner.pipeline.start(now, allowed) {
            let done = done_tx.clone();
            let transcriber = Arc::clone(transcriber);
            outside.spawn(Box::new(move || {
                let result = transcriber.transcribe(&job.wav);
                if let Err(error) = &result {
                    tracing::debug!(
                        "call captions chunk {} of leg {} not transcribed: {error}",
                        job.seq,
                        job.leg
                    );
                }
                let _ = done.send((job.id, result));
            }));
        }
        for out in captioner.pipeline.drain() {
            send_out(outside, &mut speakers, settings, out);
        }
    }
    if captioner.pipeline.dropped() > 0 {
        tracing::info!(
            "call captions: {} chunks dropped while the provider was behind",
            captioner.pipeline.dropped()
        );
    }
}

fn send_out(outside: &mut dyn Outside, speakers: &mut HashMap<Arc<str>, Speaker>, settings: &Settings, out: Out) {
    match out {
        Out::Caption { leg, seq, text } => {
            let speaker = speakers
                .entry(leg.clone())
                .or_insert_with(|| outside.speaker(&leg))
                .clone();
            if settings.store {
                outside.store(&leg, seq, &speaker, &text);
            }
            outside.send(
                json!({"rtc": "caption", "leg": &*leg, "participant": speaker.participant,
                "name": speaker.name, "seq": seq, "text": text}),
            );
        }
        Out::Gap { leg, seq } => {
            let speaker = speakers
                .entry(leg.clone())
                .or_insert_with(|| outside.speaker(&leg))
                .clone();
            outside.send(
                json!({"rtc": "caption", "leg": &*leg, "participant": speaker.participant, "seq": seq, "gap": true}),
            );
        }
        Out::Available => outside.send(json!({"rtc": "captions", "state": "on"})),
        Out::Unavailable { reason, message } => {
            tracing::warn!("call captions unavailable: {message}");
            outside.send(json!({"rtc": "captions", "state": "unavailable", "reason": reason, "message": message}));
        }
    }
}

// ---------- on the media node ----------

/// What `startCaptions` carries: who turned them on (their quota), and what the notice names.
#[derive(Debug, Clone, Default)]
pub(crate) struct StartOptions {
    pub user_id: Option<String>,
    pub provider_label: String,
    pub local: bool,
}

fn calls_running() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static RUNNING: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    RUNNING.get_or_init(|| Mutex::new(HashMap::new()))
}

fn to_room(room: &str, frame: Json) {
    if let Err(code) = super::command(Command::ToRoom {
        room: room.to_string(),
        frame,
    }) {
        tracing::debug!("call captions frame for {room} not sent: {code}");
    }
}

/// Starts a call's captions on this node: every leg is told they are on and who transcribes (the notice), before the
/// tap is attached, so no audio is taken before the notice is out; then the thread runs until [`stop`] or the room
/// closes. Already running, the notice goes out again. A start that fails tells the legs captions are unavailable,
/// with the reason, and takes the notice down.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>, room: &str, options: StartOptions) {
    let settings = Settings::from_properties(&moqui_lib::env::get_prop_or_env);
    let notice = json!({"rtc": "captions", "state": "on", "provider": options.provider_label, "local": options.local});
    // the table stays locked while the notice is sent, so a stop that follows sends its "off" after it
    let mut table = lock(calls_running());
    if table.contains_key(room) {
        set_notice(room, notice, true);
        return;
    }
    if !set_notice(room, notice, true) {
        // no notice, no captions: the driver is gone or overloaded
        to_room(
            room,
            json!({"rtc": "captions", "state": "unavailable", "reason": "overloaded", "message": "the call's media server is busy"}),
        );
        return;
    }
    // a paired node's room (E12) is that node's call: its services go down its link
    let (paired, call) = moqui_core::edge::media::split_room(room);
    let owner = moqui_core::edge::media::room_owner(room);
    let transcriber: Arc<dyn Transcriber> = if settings.provider == "local" {
        Arc::new(LocalTranscriber {
            language: settings.language.clone(),
        })
    } else if paired.is_some() || moqui_core::edge::settings(factory).edge_node() {
        // the agent runtime, the quota and the LlmCall are the local node's (plans/Edge-Node.md §5.4)
        Arc::new(DelegatedTranscriber {
            factory: Arc::clone(factory),
            call: call.to_string(),
            owner: owner.clone(),
            user_id: options.user_id.clone(),
            language: settings.language.clone(),
            timeout: settings.timeout,
        })
    } else {
        Arc::new(LlmTranscriber {
            factory: Arc::clone(factory),
            user_id: options.user_id.clone(),
            language: settings.language.clone(),
            purpose: format!("captions:{call}"),
            timeout: settings.timeout,
        })
    };
    let stop = Arc::new(AtomicBool::new(false));
    table.insert(room.to_string(), Arc::clone(&stop));
    drop(table);
    let factory = Arc::clone(factory);
    let room_id = room.to_string();
    let call = call.to_string();
    // the thread attaches, so the caller (the rtc-signal handler) never waits on the driver
    let spawned = std::thread::Builder::new()
        .name("moqui-rtc-captions".to_string())
        .spawn(move || {
            match super::media::attach(&room_id, Kinds::Audio, TAP_QUEUE) {
                Ok(consumer) => {
                    let cap = (factory.service_pools().sizes().0 / 2).max(1);
                    let mut outside = NodeOutside {
                        factory: Arc::clone(&factory),
                        room: room_id.clone(),
                        call,
                        owner,
                        cap,
                    };
                    run(&consumer, &mut outside, &transcriber, &settings, &stop);
                    if consumer.dropped() > 0 {
                        tracing::info!("call {room_id} captions: {} frames dropped while behind", consumer.dropped());
                    }
                }
                Err(code) => {
                    tracing::warn!("call {room_id} captions did not start: {code}");
                    let mut table = lock(calls_running());
                    if table.get(&room_id).is_some_and(|flag| Arc::ptr_eq(flag, &stop)) {
                        table.remove(&room_id);
                        set_notice(
                            &room_id,
                            json!({"rtc": "captions", "state": "unavailable", "reason": code, "message": "the call's audio could not be reached"}),
                            false,
                        );
                    }
                }
            }
            let mut table = lock(calls_running());
            if table.get(&room_id).is_some_and(|flag| Arc::ptr_eq(flag, &stop)) {
                table.remove(&room_id);
            }
        });
    if let Err(err) = spawned {
        tracing::error!("call {room} captions did not start, the thread did not: {err}");
        let mut table = lock(calls_running());
        table.remove(room);
        set_notice(
            room,
            json!({"rtc": "captions", "state": "unavailable", "reason": "overloaded", "message": "the call's media server is busy"}),
            false,
        );
    }
}

/// Stops a call's captions on this node; the legs are told they are off, and later joiners get no notice. (Told here,
/// not by the thread as it ends, so a quick start again is not undone by the old thread.)
pub(crate) fn stop(room: &str) {
    if let Some(flag) = lock(calls_running()).remove(room) {
        flag.store(true, Ordering::Release);
    }
    set_notice(room, json!({"rtc": "captions", "state": "off"}), false);
}

/// A captions frame every leg gets now; kept (the notice that they are on), each leg gets it when it joins too, and
/// one not kept (off, unavailable at the start) takes that down. Whether the driver took it.
fn set_notice(room: &str, frame: Json, keep: bool) -> bool {
    match super::command(Command::RoomNotice {
        room: room.to_string(),
        kind: NoticeKind::Captions,
        frame,
        keep,
    }) {
        Ok(()) => true,
        Err(code) => {
            tracing::warn!("call captions notice for {room} not set: {code}");
            false
        }
    }
}

/// The caption jobs of every call on this node.
fn node_permits() -> &'static Permits {
    static PERMITS: OnceLock<Permits> = OnceLock::new();
    PERMITS.get_or_init(Permits::default)
}

/// The node's side: frames through the driver, speakers and stored captions through the services' entities, jobs on
/// the service pool.
struct NodeOutside {
    factory: Arc<ExecutionContextFactory>,
    /// The driver's room, keyed by node for a paired node's call (E12).
    room: String,
    /// The call's id, for the services.
    call: String,
    /// Whose call: a paired node's runs its services as that node's.
    owner: moqui_core::edge::owner::Owner,
    /// The most caption jobs of all calls at once: half the service pool's workers.
    cap: usize,
}

/// Gives a job's place back when the job ends, or when it is dropped without running (a full pool queue).
struct Release;

impl Drop for Release {
    fn drop(&mut self) {
        node_permits().release();
    }
}

impl Outside for NodeOutside {
    fn send(&mut self, frame: Json) {
        to_room(&self.room, frame);
    }

    fn speaker(&mut self, leg: &str) -> Speaker {
        let leg = leg.to_string();
        let owner = self.owner.clone();
        let found = self.factory.service_pools().call(move || {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            moqui_core::edge::owner::with(owner, || speaker_of(&leg))
        });
        found.unwrap_or_else(|err| {
            tracing::debug!("call captions speaker not found: {}", err.message());
            Speaker::default()
        })
    }

    fn store(&mut self, leg: &str, seq: u64, speaker: &Speaker, text: &str) {
        super::note(
            &self.factory,
            &self.room,
            STORE_SERVICE,
            vec![
                ("callSessionId", Value::string(&self.call)),
                ("callLegId", Value::string(leg)),
                ("segment", Value::I64(i64::try_from(seq).unwrap_or(0))),
                ("speaker", Value::string(speaker.name.clone().unwrap_or_default())),
                ("text", Value::string(text)),
            ],
        );
    }

    fn reserve(&mut self, wanted: usize) -> usize {
        node_permits().reserve(wanted, self.cap)
    }

    fn spawn(&mut self, job: Box<dyn FnOnce() + Send>) {
        let release = Release;
        self.factory.service_pools().spawn("call captions", move || {
            let _release = release;
            job();
            Ok(())
        });
    }
}

/// The participant of a leg and the name the call shows for them, by `get#CaptionSpeaker` (on an edge node, its local
/// node's, plans/Edge-Node.md §5.4).
fn speaker_of(leg: &str) -> Result<Speaker, moqui_core::context::Error> {
    let parameters = BTreeMap::from([("callLegId".to_string(), Value::string(leg))]);
    let out = service(SPEAKER_SERVICE, &parameters)?;
    let text = |name: &str| out.get(name).map(to_plain_string).filter(|value| !value.is_empty());
    Ok(Speaker {
        participant: text("callParticipantId"),
        name: text("name"),
    })
}

/// A service in this thread's context: its result, or its errors as one, kept apart from the context's own errors,
/// which stay.
fn service(
    name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>, moqui_core::context::Error> {
    ExecutionContext::with_current(|ec| ec.message().push_errors());
    let out = moqui_core::service::sync(name, parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let mut messages = ec.message();
        let errors = messages.errors().to_vec();
        messages.clear_errors();
        messages.pop_errors();
        errors
    })
    .unwrap_or_default();
    match out {
        Ok(_) if !errors.is_empty() => Err(moqui_core::context::Error::new(errors.join("; "))),
        other => other,
    }
}

const SPEAKER_SERVICE: &str = "org.moqui.call.CallCaptionServices.get#CaptionSpeaker";
const STORE_SERVICE: &str = "org.moqui.call.CallCaptionServices.store#CallTranscript";
const TRANSCRIBE_SERVICE: &str = "org.moqui.call.CallCaptionServices.transcribe#CaptionAudio";

/// The `moqui-llm` endpoint bound to `transcribe`.
struct LlmTranscriber {
    factory: Arc<ExecutionContextFactory>,
    user_id: Option<String>,
    language: Option<String>,
    purpose: String,
    timeout: Duration,
}

impl Transcriber for LlmTranscriber {
    fn transcribe(&self, wav: &[u8]) -> Result<String, SpeechError> {
        moqui_core::llm_hooks::transcribe(
            &self.factory,
            &SpeechRequest {
                audio: wav,
                media_type: "audio/wav",
                language: self.language.as_deref(),
                prompt: None,
                user_id: self.user_id.as_deref(),
                purpose: &self.purpose,
                timeout: Some(self.timeout),
            },
        )
    }
}

/// On an edge node, and for a paired node's call on an all-in-one node (E12): `transcribe#CaptionAudio`, which runs on
/// the local node (delegated), with the chunk as base64.
struct DelegatedTranscriber {
    factory: Arc<ExecutionContextFactory>,
    call: String,
    owner: moqui_core::edge::owner::Owner,
    user_id: Option<String>,
    language: Option<String>,
    timeout: Duration,
}

impl Transcriber for DelegatedTranscriber {
    fn transcribe(&self, wav: &[u8]) -> Result<String, SpeechError> {
        use base64::Engine as _;
        let mut parameters = BTreeMap::from([
            ("callSessionId".to_string(), Value::string(&self.call)),
            (
                "audioBase64".to_string(),
                Value::string(base64::engine::general_purpose::STANDARD.encode(wav)),
            ),
            (
                "timeoutSeconds".to_string(),
                Value::I64(i64::try_from(self.timeout.as_secs()).unwrap_or(20)),
            ),
        ]);
        if let Some(user) = &self.user_id {
            parameters.insert("userId".to_string(), Value::string(user));
        }
        if let Some(language) = &self.language {
            parameters.insert("language".to_string(), Value::string(language));
        }
        let run = || {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            moqui_core::edge::owner::with(self.owner.clone(), || service(TRANSCRIBE_SERVICE, &parameters))
        };
        // a caption job runs on the service pool, in a context of its own
        let out = if ExecutionContext::with_current(|_| ()).is_some() {
            run()
        } else {
            let context = self.factory.new_context();
            let _entered = context.enter();
            run()
        };
        let out = out.map_err(|err| SpeechError::new("failed", err.message().to_string()))?;
        let text = |name: &str| out.get(name).map(to_plain_string).unwrap_or_default();
        match text("errorCode").as_str() {
            "" => Ok(text("text")),
            code => Err(SpeechError::new(speech_code(code), text("errorMessage"))),
        }
    }
}

/// A refusal's code as `SpeechError` names it; one it does not know is `failed`.
fn speech_code(code: &str) -> &'static str {
    match code {
        "notBuiltIn" => "notBuiltIn",
        "notConfigured" => "notConfigured",
        "invalid" => "invalid",
        "quota" => "quota",
        "timeout" => "timeout",
        "notAllowed" => "notAllowed",
        _ => "failed",
    }
}

/// `speech-local`: not built yet.
struct LocalTranscriber {
    language: Option<String>,
}

impl Transcriber for LocalTranscriber {
    fn transcribe(&self, wav: &[u8]) -> Result<String, SpeechError> {
        crate::speech_local::transcribe(wav, self.language.as_deref())
    }
}

#[cfg(test)]
#[path = "captions_tests.rs"]
mod tests;
