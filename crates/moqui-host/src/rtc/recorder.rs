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

//! The call recorder on the media node (plans/Comms-Framework.md W5, §6.4): one thread per recording going here.
//!
//! The call recording services (`org.moqui.impl.CallRecordingServices`) send `{action: 'recording', op, ...}` by
//! `send#MediaControl` to the call's media node; [`control`] takes it. Each carries the `notice` for the legs (the rtc
//! `recording` frame), which the driver sends to every leg of the room and again to each leg that joins later. `start`
//! starts a recorder thread: a [`media::Consumer`] of the room with every kind (`Kinds::All`) in a queue of
//! `call_recording_queue_frames`, feeding a [`moqui_rtc::record::Recorder`], which writes each track and, from an
//! `AudioRoom` ticked every 20 ms, the mixed audio. `pause`, `resume` and `stop` go to that thread. A leg that leaves
//! (`TapEvent::Left`) ends its tracks.
//!
//! Files are spooled under `runtime/tmp/call-recording/<callRecordingId>/`. A closed segment goes to a store thread
//! (a bounded queue) that hands it to `store#RecordingSegment`, which moves it to `dbresource://` and writes its rows.
//!
//! On an edge node (plans/Edge-Node.md §5.4, E7) the recording's rows are its local node's: each closed segment is
//! queued to the local node as an item of kind [`QUEUE_KIND`] with the file as its content (the spool file goes once
//! it is queued), and its end (`note#RecorderEnd`) under the same conversation key, the recording's id, so it never
//! overtakes a segment; the local node's `receive#EdgeRecording` stores them. Its segments are at most
//! [`EDGE_SEGMENT_MB`], well under the queue's content cap. On an all-in-one node (E12) the same goes for a paired
//! node's call, to that node ([`Keeper`]), and the server's own calls' recordings are stored here.
//!
//! A failure ends the recording, never the call: a queue that overflowed (more than [`MAX_DROPPED`] frames dropped),
//! a file that could not be written, a store that failed or fell behind. The segments already closed are stored, the
//! partial files removed, and `note#RecorderEnd` marks the recording failed with the reason; the call's media goes on.
//! Reaching `call_recording_max_mb`, or what was left of the owner's drive quota at the start (`maxBytes`), stops the
//! recording the same way, as a stop.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{
    channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use moqui_rtc::record::{ClosedSegment, RecordConfig, RecordError, Recorder, Spool};
use serde_json::Value as Json;

use super::media::{self, Kinds, TapEvent};
use super::{command, command_waiting, Command, NoticeKind};

/// Where the recorder spools its files, under the runtime directory: the one place `store#RecordingFile` takes a file
/// from (`RECORDING_SPOOL` in moqui-core `service/call_media.rs`).
pub(crate) const SPOOL: &str = "tmp/call-recording";
const STORE_SERVICE: &str = "org.moqui.impl.CallRecordingServices.store#RecordingSegment";
const END_SERVICE: &str = "org.moqui.impl.CallRecordingServices.note#RecorderEnd";
/// Frames the recorder may lose to a full queue before it fails.
pub(crate) const MAX_DROPPED: u64 = 100;
/// Closed segments waiting for the store; past this the recorder fails (the store fell behind).
const STORE_QUEUE: usize = 64;
/// How long the end of a recording waits for its segments to be stored.
const STORE_DRAIN: Duration = Duration::from_secs(60);
/// The mixed audio's tick.
const TICK_MS: u64 = 20;
/// Events taken in one turn before the tick is looked at again.
const BATCH: usize = 256;
/// How long a recording's notice waits for room in the media queue before it is logged as not taken.
const NOTICE_WAIT: Duration = Duration::from_millis(500);
/// How often the recorder asks the room's browsers for keyframes while a video track waits for one.
const KEYFRAME_ASK: Duration = Duration::from_secs(1);
/// The edge link's queue kind of an edge node's recordings (MoquiDefaultConf.xml edge).
pub(crate) const QUEUE_KIND: &str = "callRecording";
/// The largest segment an edge node's recorder makes, in MiB: half the queue's content cap
/// (`moqui_core::edge::queue::MAX_CONTENT_BYTES`), so a segment that runs past its limit by a frame still fits.
pub(crate) const EDGE_SEGMENT_MB: u64 = 32;

/// The recorder's limits, from the properties.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub queue_frames: usize,
    pub record: RecordConfig,
}

impl Limits {
    /// On an edge node a segment is at most [`EDGE_SEGMENT_MB`], which its queue carries to the local node.
    pub(crate) fn for_edge(mut self) -> Self {
        self.record.max_segment_bytes = self.record.max_segment_bytes.min(EDGE_SEGMENT_MB * 1024 * 1024);
        self
    }

    pub(crate) fn from_properties(property: &dyn Fn(&str) -> String, mixed_only: bool) -> Self {
        let number = |name: &str, default: u64| {
            property(name)
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .unwrap_or(default)
        };
        Limits {
            queue_frames: number("call_recording_queue_frames", 4_096).clamp(64, 1 << 20) as usize,
            record: RecordConfig {
                tracks: !mixed_only,
                mix: true,
                max_segment_bytes: number("call_recording_segment_mb", 64) * 1024 * 1024,
                max_segment_ms: number("call_recording_segment_minutes", 30) * 60_000,
                max_total_bytes: number("call_recording_max_mb", 2_048) * 1024 * 1024,
            },
        }
    }
}

/// What the services tell a recorder thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Pause,
    Resume { period: u32 },
    Stop,
}

/// The recorders' control senders: unbounded, apart from the media queue, so a Pause, Resume or Stop is never dropped
/// (a recorder takes them every turn, and there are few).
fn recorders() -> &'static Mutex<HashMap<String, Sender<Control>>> {
    static RECORDERS: OnceLock<Mutex<HashMap<String, Sender<Control>>>> = OnceLock::new();
    RECORDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Where a recording's segments and end go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Keeper {
    /// Stored here, by the call recording services: a normal node's, and an all-in-one node's own calls.
    Here,
    /// Queued for a local node: the paired node a call's control came from (E12), or `None` on a dedicated edge node,
    /// whose queue picks its only paired node.
    Queued(Option<Arc<str>>),
}

impl Keeper {
    /// The keeper of a recording whose control came from `node` over a link (`None`: from this node or its cluster).
    pub(crate) fn of(factory: &ExecutionContextFactory, node: Option<&str>) -> Self {
        match node {
            Some(node) => Keeper::Queued(Some(Arc::from(node))),
            None if moqui_core::edge::settings(factory).edge_node() => Keeper::Queued(None),
            None => Keeper::Here,
        }
    }
}

/// Whether a recording notice is kept for the legs that join later: until the recording is over (stopped or failed).
fn notice_kept(notice: &Json) -> bool {
    !matches!(notice.get("state").and_then(Json::as_str), Some("stopped" | "failed"))
}

/// A `recording` control message for this node: its notice to the room's legs, then its op for the recorder.
pub(crate) fn control(factory: &Arc<ExecutionContextFactory>, body: &Json) {
    let text = |k: &str| body.get(k).and_then(Json::as_str).unwrap_or_default().to_string();
    let (call, id) = (text("callSessionId"), text("callRecordingId"));
    if call.is_empty() || id.is_empty() {
        return;
    }
    // a paired node's call (its control came over its link, E12): its room and recorder are keyed by node, as its
    // ids are another database's, and what it records is queued to it
    let node = moqui_core::edge::signal::link_node(body);
    let room = moqui_core::edge::media::room_key(node, &call);
    let key = moqui_core::edge::media::room_key(node, &id);
    let keeper = Keeper::of(factory, node);
    if let Some(notice) = body.get("notice").filter(|n| n.is_object()) {
        // the legs must hear of a pause or a resume: waits briefly for room in the media queue, and is logged if not
        if let Err(code) = command_waiting(
            Command::RoomNotice {
                room: room.clone(),
                kind: NoticeKind::Recording,
                frame: notice.clone(),
                keep: notice_kept(notice),
            },
            NOTICE_WAIT,
        ) {
            tracing::warn!("call recording {id}: its notice to the call {room} was not taken: {code}");
        }
    }
    let period = body
        .get("period")
        .and_then(Json::as_u64)
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or(1);
    let op = text("op");
    match op.as_str() {
        "start" => {
            let paused = body.get("paused").and_then(Json::as_bool).unwrap_or(false);
            let mut limits = Limits::from_properties(&moqui_lib::env::get_prop_or_env, text("kind") == "mixed");
            if keeper != Keeper::Here {
                limits = limits.for_edge();
            }
            // what is left of the owner's drive quota, when it has one: the recording stops there as at its size limit
            if let Some(left) = body.get("maxBytes").and_then(Json::as_u64) {
                limits.record.max_total_bytes = limits.record.max_total_bytes.min(left.max(1));
            }
            start(factory, &room, &id, &key, keeper, limits, period, paused);
        }
        "pause" => tell(&key, Control::Pause),
        "resume" => tell(&key, Control::Resume { period }),
        "stop" => tell(&key, Control::Stop),
        _ => {}
    }
}

fn tell(key: &str, control: Control) {
    let id = moqui_core::edge::media::split_room(key).1;
    let sender = lock(recorders()).get(key).cloned();
    match sender {
        Some(tx) => {
            if tx.send(control).is_err() {
                tracing::warn!("call recording {id}: the recorder ended before it took {control:?}");
            }
        }
        None => tracing::warn!("call recording {id}: no recorder here for {control:?}"),
    }
}

/// Starts the recorder thread of a recording (once), by its `key` (its id, keyed by node for a paired node's).
#[allow(clippy::too_many_arguments)]
fn start(
    factory: &Arc<ExecutionContextFactory>,
    room: &str,
    id: &str,
    key: &str,
    keeper: Keeper,
    limits: Limits,
    period: u32,
    paused: bool,
) {
    let (tx, rx) = channel();
    {
        let mut map = lock(recorders());
        if map.contains_key(key) {
            return;
        }
        map.insert(key.to_string(), tx);
    }
    let factory = Arc::clone(factory);
    let (room, id, key) = (room.to_string(), id.to_string(), key.to_string());
    let spawned = std::thread::Builder::new()
        .name("moqui-call-recorder".to_string())
        .spawn({
            let (id, key) = (id.clone(), key.clone());
            move || {
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&factory, &room, &id, &key, &keeper, limits, period, paused, &rx)
                }));
                if ran.is_err() {
                    tracing::error!("call recording {id}: the recorder panicked");
                    end_service(&factory, &id, &keeper, "failed", "the recorder stopped unexpectedly");
                }
                lock(recorders()).remove(&key);
            }
        });
    if let Err(err) = spawned {
        lock(recorders()).remove(&key);
        tracing::error!("call recording {id}: the recorder thread did not start: {err}");
    }
}

/// Removes what earlier runs of this node left in the spool: no recorder of this process uses it yet.
pub(crate) fn sweep(runtime_dir: &Path) {
    let spool = runtime_dir.join(SPOOL);
    if spool.is_dir() {
        if let Err(err) = std::fs::remove_dir_all(&spool) {
            tracing::warn!("the call recording spool {} was not cleared: {err}", spool.display());
        }
    }
}

/// Where a closed segment goes: the store thread, or a test's list.
pub(crate) trait Store {
    /// Queues a closed segment's file to store. Err: the store failed earlier, or is too far behind.
    fn put(&mut self, segment: ClosedSegment, path: PathBuf, from: SystemTime) -> Result<(), String>;
    /// A store that failed, with why.
    fn failure(&self) -> Option<String>;
    /// Waits for what is queued to be stored (or to fail).
    fn drain(&mut self);
    /// The recording ended on its own: failed, or at its size limit.
    fn end(&mut self, outcome: &str, text: &str);
}

/// The recorder's files in its spool directory.
pub(crate) struct FileSpool {
    dir: PathBuf,
}

impl FileSpool {
    pub(crate) fn new(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(FileSpool { dir })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Spool for FileSpool {
    type File = std::io::BufWriter<std::fs::File>;

    fn create(&mut self, name: &str) -> std::io::Result<Self::File> {
        Ok(std::io::BufWriter::new(std::fs::File::create(self.path(name))?))
    }

    fn discard(&mut self, name: &str) {
        let _ = std::fs::remove_file(self.path(name));
    }
}

/// One recording on this node: the recorder, its spool and its store, on the caller's clock.
pub(crate) struct Session<S: Store> {
    recorder: Recorder<FileSpool>,
    store: S,
    dir: PathBuf,
    started: SystemTime,
    ticked_ms: u64,
}

/// Why a session ends.
#[derive(Debug, PartialEq)]
pub(crate) enum Ended {
    /// Stopped (by the services, or its room closed).
    Stopped,
    /// At its size limit.
    Limit,
    /// Failed, with why.
    Failed(String),
}

impl<S: Store> Session<S> {
    pub(crate) fn new(dir: PathBuf, store: S, limits: Limits, period: u32, paused: bool) -> Result<Self, String> {
        let spool =
            FileSpool::new(dir.clone()).map_err(|e| format!("the spool {} could not be made: {e}", dir.display()))?;
        let recorder = Recorder::new(limits.record, spool, period, paused, 0).map_err(|e| e.to_string())?;
        Ok(Session {
            recorder,
            store,
            dir,
            started: SystemTime::now(),
            ticked_ms: 0,
        })
    }

    /// A frame or a leave, at `now_ms` from the start.
    pub(crate) fn event(&mut self, now_ms: u64, event: &TapEvent) -> Result<(), Ended> {
        self.recorder.event(now_ms, event).map_err(ended)?;
        self.flush()
    }

    /// The mixed audio's ticks due by `now_ms` (several after a stall).
    pub(crate) fn tick_to(&mut self, now_ms: u64) -> Result<(), Ended> {
        while self.ticked_ms + TICK_MS <= now_ms {
            self.ticked_ms += TICK_MS;
            self.recorder.tick(self.ticked_ms).map_err(ended)?;
        }
        self.flush()
    }

    pub(crate) fn control(&mut self, now_ms: u64, control: Control) -> Result<bool, Ended> {
        match control {
            Control::Pause => self.recorder.pause(now_ms).map_err(ended)?,
            Control::Resume { period } => {
                self.recorder.resume(now_ms, period);
                self.ticked_ms = now_ms;
            }
            Control::Stop => return Ok(false),
        }
        self.flush().map(|()| true)
    }

    /// Whether a video track waited for a keyframe since the last call ([`Recorder::take_keyframe_wanted`]).
    pub(crate) fn take_keyframe_wanted(&mut self) -> bool {
        self.recorder.take_keyframe_wanted()
    }

    /// Hands the closed segments to the store; a store that failed or fell behind ends the recording.
    fn flush(&mut self) -> Result<(), Ended> {
        if let Some(failure) = self.store.failure() {
            return Err(Ended::Failed(failure));
        }
        for segment in self.recorder.take_closed() {
            let path = self.dir.join(&segment.file);
            self.store.put(segment, path, self.started).map_err(Ended::Failed)?;
        }
        Ok(())
    }

    /// Ends the recording: a stop or the limit closes and stores the open segments; a failure stores what was
    /// closed and removes the rest. The spool goes, and the services hear of an end they did not ask for. Gives how it
    /// ended, and the store.
    pub(crate) fn end(mut self, now_ms: u64, how: Ended) -> (Ended, S) {
        let how = match how {
            Ended::Failed(_) => how,
            _ => match self.recorder.finish(now_ms).map_err(ended).and_then(|()| self.flush()) {
                Ok(()) => how,
                Err(failed) => failed,
            },
        };
        self.store.drain();
        let how = match (how, self.store.failure()) {
            (Ended::Failed(text), _) => Ended::Failed(text),
            (_, Some(failure)) => Ended::Failed(failure),
            (other, None) => other,
        };
        if let Err(err) = std::fs::remove_dir_all(&self.dir) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("the call recording spool {} was not removed: {err}", self.dir.display());
            }
        }
        match &how {
            Ended::Failed(text) => self.store.end("failed", text),
            Ended::Limit => self.store.end("limit", "the recording reached its size limit"),
            Ended::Stopped => {}
        }
        (how, self.store)
    }
}

fn ended(err: RecordError) -> Ended {
    match err {
        RecordError::Limit => Ended::Limit,
        other => Ended::Failed(other.to_string()),
    }
}

/// The recorder thread: frames from the room, ticks every 20 ms, and the services' controls, until a stop, the room
/// closing, or a failure.
#[allow(clippy::too_many_arguments)]
fn run(
    factory: &Arc<ExecutionContextFactory>,
    room: &str,
    id: &str,
    key: &str,
    keeper: &Keeper,
    limits: Limits,
    period: u32,
    paused: bool,
    controls: &Receiver<Control>,
) {
    let store = ServiceStore::start(factory, id, keeper);
    let dir = factory.runtime_dir().join(SPOOL).join(sanitize(key));
    let consumer = match media::attach(room, Kinds::All, limits.queue_frames) {
        Ok(consumer) => consumer,
        Err(code) => {
            let mut store = store;
            store.end(
                "failed",
                &format!("the recorder could not reach the call's media ({code})"),
            );
            return;
        }
    };
    let mut session = match Session::new(dir, store, limits, period, paused) {
        Ok(session) => session,
        Err(text) => {
            end_service(factory, id, keeper, "failed", &text);
            return;
        }
    };
    let clock = Instant::now();
    let now_ms = || clock.elapsed().as_millis() as u64;
    let mut keyframes_asked: Option<Instant> = None;
    let how = loop {
        let mut stop = None;
        loop {
            match controls.try_recv() {
                Ok(control) => match session.control(now_ms(), control) {
                    Ok(true) => {}
                    Ok(false) => {
                        stop = Some(Ended::Stopped);
                        break;
                    }
                    Err(how) => {
                        stop = Some(how);
                        break;
                    }
                },
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    stop = Some(Ended::Stopped);
                    break;
                }
            }
        }
        if let Some(how) = stop {
            break how;
        }
        let now = now_ms();
        let wait = (session.ticked_ms + TICK_MS).saturating_sub(now).clamp(1, TICK_MS);
        let mut result = match consumer.recv_timeout(Duration::from_millis(wait)) {
            Ok(event) => session.event(now_ms(), &event),
            Err(RecvTimeoutError::Timeout) => Ok(()),
            // the room closed (the call ended): what was recorded is kept
            Err(RecvTimeoutError::Disconnected) => break Ended::Stopped,
        };
        for _ in 0..BATCH {
            if result.is_err() {
                break;
            }
            match consumer.try_recv() {
                Ok(event) => result = session.event(now_ms(), &event),
                Err(_) => break,
            }
        }
        let result = result.and_then(|()| session.tick_to(now_ms()));
        if let Err(how) = result {
            break how;
        }
        // a browser sends a keyframe only now and then unless asked: a file starts at one (a start, a resume, a cut)
        if session.take_keyframe_wanted() && keyframes_asked.is_none_or(|at| at.elapsed() >= KEYFRAME_ASK) {
            keyframes_asked = Some(Instant::now());
            if let Err(code) = command(Command::Keyframes { room: room.to_string() }) {
                tracing::debug!("call recording {id}: keyframes not asked for: {code}");
            }
        }
        if consumer.dropped() > MAX_DROPPED {
            break Ended::Failed(format!(
                "the recorder fell behind: its queue of {} frames was full",
                limits.queue_frames
            ));
        }
    };
    drop(consumer);
    let (how, _) = session.end(now_ms(), how);
    if how != Ended::Stopped {
        tracing::warn!("call recording {id} ended: {how:?}");
    }
}

fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn end_service(factory: &Arc<ExecutionContextFactory>, id: &str, keeper: &Keeper, outcome: &str, text: &str) {
    if let Keeper::Queued(node) = keeper {
        let payload = serde_json::json!({"op": "end", "callRecordingId": id, "outcome": outcome, "errorText": text});
        if let Err(err) = queue_to_local(factory, id, node.as_deref(), &format!("{id}/end"), &payload, None) {
            tracing::error!("call recording {id}: its end ({outcome}) was not queued for the local node: {err}");
        }
        return;
    }
    let parameters = std::collections::BTreeMap::from([
        ("callRecordingId".to_string(), Value::string(id)),
        ("outcome".to_string(), Value::string(outcome)),
        ("errorText".to_string(), Value::string(text)),
    ]);
    if let Err(err) = call_service(factory, END_SERVICE, parameters) {
        tracing::error!("call recording {id}: its end ({outcome}) was not recorded: {err}");
    }
}

/// A service as internal work on a service worker, waiting for it; its errors as one text.
fn call_service(
    factory: &Arc<ExecutionContextFactory>,
    name: &'static str,
    parameters: std::collections::BTreeMap<String, Value>,
) -> Result<(), String> {
    factory
        .service_pools()
        .call(move || {
            let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
            let out = moqui_core::service::sync(name, &parameters).map(|_| ());
            let errors = ExecutionContext::with_current(|ec| {
                let errors = ec.message().errors().to_vec();
                ec.message().clear();
                errors
            })
            .unwrap_or_default();
            match out {
                Err(err) => Err(err),
                Ok(()) if !errors.is_empty() => Err(moqui_core::context::Error::new(errors.join("; "))),
                Ok(()) => Ok(()),
            }
        })
        .map_err(|err| err.message().to_string())
}

/// A stored segment's job.
struct Job {
    segment: ClosedSegment,
    path: PathBuf,
    from: SystemTime,
}

/// The store thread: each closed segment through `store#RecordingSegment`, in order.
struct ServiceStore {
    factory: Arc<ExecutionContextFactory>,
    id: String,
    keeper: Keeper,
    jobs: Option<SyncSender<Job>>,
    failure: Arc<Mutex<Option<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl ServiceStore {
    fn start(factory: &Arc<ExecutionContextFactory>, id: &str, keeper: &Keeper) -> Self {
        let (tx, rx) = sync_channel::<Job>(STORE_QUEUE);
        let failure = Arc::new(Mutex::new(None));
        let thread = {
            let factory = Arc::clone(factory);
            let failure = Arc::clone(&failure);
            let id = id.to_string();
            let keeper = keeper.clone();
            std::thread::Builder::new()
                .name("moqui-call-recording-store".to_string())
                .spawn(move || {
                    for job in rx {
                        if lock(&failure).is_some() {
                            let _ = std::fs::remove_file(&job.path);
                            continue;
                        }
                        if let Err(err) = store_one(&factory, &id, &keeper, &job) {
                            tracing::error!("call recording {id}: segment {} not stored: {err}", job.segment.file);
                            let _ = std::fs::remove_file(&job.path);
                            lock(&failure).get_or_insert(format!("a segment was not stored: {err}"));
                        }
                    }
                })
                .ok()
        };
        if thread.is_none() {
            *lock(&failure) = Some("the recording's store thread did not start".to_string());
        }
        ServiceStore {
            factory: Arc::clone(factory),
            id: id.to_string(),
            keeper: keeper.clone(),
            jobs: Some(tx),
            failure,
            thread,
        }
    }
}

fn store_one(factory: &Arc<ExecutionContextFactory>, id: &str, keeper: &Keeper, job: &Job) -> Result<(), String> {
    let date = |ms: u64| {
        let at = job.from + Duration::from_millis(ms);
        let millis = at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        jiff::Timestamp::from_millisecond(millis).map_or(Value::Null, Value::Timestamp)
    };
    let s = &job.segment;
    let mut parameters = std::collections::BTreeMap::from([
        ("callRecordingId".to_string(), Value::string(id)),
        ("trackKind".to_string(), Value::string(s.track.kind.as_str())),
        ("codec".to_string(), Value::string(s.track.kind.codec())),
        ("contentType".to_string(), Value::string(s.track.kind.content_type())),
        ("segmentPeriod".to_string(), Value::I64(i64::from(s.period))),
        ("segmentSeq".to_string(), Value::I64(i64::from(s.seq))),
        (
            "startOffsetMs".to_string(),
            Value::I64(i64::try_from(s.start_offset_ms).unwrap_or(0)),
        ),
        (
            "durationMs".to_string(),
            Value::I64(i64::try_from(s.duration_ms).unwrap_or(0)),
        ),
        ("fromDate".to_string(), date(s.from_ms)),
        ("thruDate".to_string(), date(s.thru_ms)),
        ("tempPath".to_string(), Value::string(job.path.to_string_lossy())),
        ("trackEnded".to_string(), Value::Bool(s.track_ended)),
    ]);
    if let Some(leg) = &s.track.leg {
        parameters.insert("callLegId".to_string(), Value::string(&**leg));
    }
    if let Some(source) = s.track.source {
        parameters.insert("source".to_string(), Value::string(source.as_str()));
    }
    if let Keeper::Queued(node) = keeper {
        return queue_segment(factory, id, node.as_deref(), job, parameters);
    }
    call_service(factory, STORE_SERVICE, parameters)
}

/// On an edge node: the segment queued for the local node, with its file as the item's content, then the spool file
/// removed. Keyed by the recording and the file's name, so a segment queued twice is one item.
fn queue_segment(
    factory: &Arc<ExecutionContextFactory>,
    id: &str,
    node: Option<&str>,
    job: &Job,
    mut parameters: std::collections::BTreeMap<String, Value>,
) -> Result<(), String> {
    parameters.remove("tempPath");
    let content =
        std::fs::read(&job.path).map_err(|err| format!("the spooled segment {}: {err}", job.path.display()))?;
    let fields: serde_json::Map<String, Json> = parameters
        .iter()
        .map(|(name, value)| (name.clone(), moqui_lib::json::to_json(value)))
        .collect();
    let payload = serde_json::json!({"op": "segment", "parameters": fields});
    let key = format!("{id}/{}", job.segment.file);
    // the spool file goes once its content is queued; when queueing failed, the store fails the recording, which
    // removes it with the spool
    queue_to_local(factory, id, node, &key, &payload, Some(content))?;
    if let Err(err) = std::fs::remove_file(&job.path) {
        tracing::warn!(
            "call recording {id}: the spooled segment {} was not removed: {err}",
            job.path.display()
        );
    }
    Ok(())
}

/// An item of [`QUEUE_KIND`] for the local node `node` (on a dedicated edge node `None`, the only one paired), in the
/// recording's conversation, so its segments and its end arrive in order.
fn queue_to_local(
    factory: &Arc<ExecutionContextFactory>,
    id: &str,
    node: Option<&str>,
    key: &str,
    payload: &Json,
    content: Option<Vec<u8>>,
) -> Result<(), String> {
    let item = moqui_core::edge::queue::NewItem {
        kind: QUEUE_KIND.to_string(),
        conversation_key: format!("callRecording:{id}"),
        idempotency_key: key.to_string(),
        payload: Some(payload.to_string()),
        content,
        content_location: None,
    };
    let owner = Arc::clone(factory);
    let node = node.map(str::to_string);
    factory
        .service_pools()
        .call(move || {
            let facade = owner
                .entity()
                .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
            moqui_core::edge::queue::enqueue(&owner, &facade, node.as_deref(), &item)
                .map(drop)
                .map_err(|err| moqui_core::context::Error::new(err.message().to_string()))
        })
        .map_err(|err| err.message().to_string())
}

impl Store for ServiceStore {
    fn put(&mut self, segment: ClosedSegment, path: PathBuf, from: SystemTime) -> Result<(), String> {
        if let Some(failure) = self.failure() {
            return Err(failure);
        }
        let Some(jobs) = &self.jobs else {
            return Err("the recording's store is closed".to_string());
        };
        match jobs.try_send(Job { segment, path, from }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(job)) => {
                let _ = std::fs::remove_file(&job.path);
                Err(format!("the store fell behind: {STORE_QUEUE} segments were waiting"))
            }
            Err(TrySendError::Disconnected(job)) => {
                let _ = std::fs::remove_file(&job.path);
                Err("the recording's store stopped".to_string())
            }
        }
    }

    fn failure(&self) -> Option<String> {
        lock(&self.failure).clone()
    }

    fn drain(&mut self) {
        self.jobs.take();
        if let Some(thread) = self.thread.take() {
            let until = Instant::now() + STORE_DRAIN;
            while !thread.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                lock(&self.failure).get_or_insert("the recording's last segments were not stored in time".to_string());
            }
        }
    }

    fn end(&mut self, outcome: &str, text: &str) {
        end_service(&self.factory, &self.id, &self.keeper, outcome, text);
    }
}

#[cfg(test)]
#[path = "recorder_tests.rs"]
mod tests;
