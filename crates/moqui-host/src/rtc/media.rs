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

//! The hook points for a call's media on this node (plans/Comms-Framework.md §5.1, §6.4; the M0 gate in §6.2): what the
//! recorder (W5), captions (W6) and SIP legs (V1) build on. Both run on the caller's own thread and talk to the media
//! driver thread through its command queue, so nothing here can stall forwarding.
//!
//! **Taking frames out: [`attach`].** `attach(room, kinds, capacity)` adds a [`moqui_rtc::tap::QueueTap`] to the
//! room (the `callSessionId`) and gives a [`Consumer`]: every frame published in the room, a browser's or an injected
//! leg's, copied encoded ([`OwnedFrame`]: leg, kind, source, codec, RTP time, payload, audio level) into a queue of
//! `capacity` frames. When the consumer falls behind, frames are dropped and counted ([`Consumer::dropped`]); the media
//! thread never waits. A room has a consumer for each `attach`, and each sees every frame. Dropping the consumer
//! detaches it; when the room closes its queue disconnects (`recv_timeout` gives `Disconnected`).
//!
//! - The recorder takes `Kinds::All` and writes each track's Opus and VP8 as they come (§6.4: no transcoding).
//! - The mixed recording, captions and SIP legs take `Kinds::Audio` and feed a [`moqui_rtc::audio::room::AudioRoom`]
//!   on their own thread: `push` each frame, `tick` every 20 ms, then read the mixer (`minus(leg)` for a phone leg,
//!   `all()` for the mixed recording) or `leg_audio(leg)` for one speaker's captions.
//!
//! **Putting audio in: [`add_pcm_source`].** `add_pcm_source(room, leg)` adds an injected participant (a SIP leg's
//! voice, §5.1) and gives a [`PcmSource`]. Its owner pushes 20 ms of 48 kHz mono PCM at a time
//! ([`moqui_rtc::audio::FRAME`] samples, resampled from the phone's codec with [`moqui_rtc::audio::resample`]); it is
//! encoded to Opus on the caller's thread and queued, and the media thread injects one frame each 20 ms on its own
//! timer, at RTP times that follow its clock (a frame missing at its time is a gap the browsers conceal). After two
//! frames are queued, the timer starts. Every peer of the room hears it like a published track, and the room's
//! consumers see it too, so two phone legs hear each other through the mixer. Dropping the source removes the leg; so
//! does `send#MediaControl` closing the leg or the call.
//!
//! Both work on this node's rooms only: call them on the call's media node (`CallSession.mediaNodeId`, §6.3), where a
//! room is made for them when it has no leg yet. The errors are the driver's codes: `noMedia` (no media node here),
//! `overloaded` (its command queue is full), `timeout`, and the SFU's (`tooManyRooms`, `roomFull`, `staleGeneration`
//! for a leg that is a browser's).

#![allow(
    dead_code,
    reason = "hook points: the recorder (W5), captions (W6) and SIP legs (V1) call them"
)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::time::{Duration, Instant};

use moqui_rtc::audio::opus::{OpusEncoder, VOICE_BITRATE};
use moqui_rtc::audio::FRAME;
pub(crate) use moqui_rtc::tap::{Kinds, OwnedFrame};
use moqui_rtc::tap::{MediaTap, QueueTap, TapReceiver};
use moqui_rtc::{Sfu, TapId};

use super::{command, Command};

/// How long [`attach`] and [`add_pcm_source`] wait for the driver.
const REPLY_WAIT: Duration = Duration::from_secs(2);
/// The encoded frames a [`PcmSource`] queues for the media thread: 320 ms.
const SOURCE_QUEUE: usize = 16;
/// The frames queued before an injected leg's timer starts, which absorbs that much jitter in its pushes.
const PREFILL: usize = 2;
/// The frames an injected leg holds at most on the media thread; past this the oldest go.
const MAX_PENDING: usize = 8;
/// The length of a frame.
const TICK: Duration = Duration::from_millis(20);
/// Behind by more than this (the driver stalled), the timer skips ahead rather than bursting.
const MAX_BEHIND: Duration = Duration::from_millis(200);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A media command, from the callers here to the driver thread.
pub(crate) enum MediaCommand {
    Attach {
        room: String,
        id: u64,
        tap: Box<dyn MediaTap>,
        reply: SyncSender<Result<(), &'static str>>,
    },
    Detach {
        id: u64,
    },
    AddSource {
        room: String,
        leg: String,
        frames: Receiver<Vec<u8>>,
        reply: SyncSender<Result<(), &'static str>>,
    },
    RemoveSource {
        room: String,
        leg: String,
    },
}

/// The frames a consumer takes out of a room.
pub(crate) struct Consumer {
    room: String,
    id: u64,
    frames: TapReceiver,
}

/// Starts taking a room's frames: `kinds` of them, into a queue of `capacity`.
pub(crate) fn attach(room: &str, kinds: Kinds, capacity: usize) -> Result<Consumer, &'static str> {
    let (tap, frames) = QueueTap::channel(capacity, kinds);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (reply, answer) = sync_channel(1);
    command(Command::Media(MediaCommand::Attach {
        room: room.to_string(),
        id,
        tap: Box::new(tap),
        reply,
    }))?;
    wait(&answer)?;
    Ok(Consumer {
        room: room.to_string(),
        id,
        frames,
    })
}

impl Consumer {
    /// The next frame, waiting up to `wait`. `Err(Disconnected)` once the room closed.
    pub(crate) fn recv_timeout(&self, wait: Duration) -> Result<OwnedFrame, RecvTimeoutError> {
        self.frames.recv_timeout(wait)
    }

    /// The next frame if one is waiting. `Err(Disconnected)` once the room closed.
    pub(crate) fn try_recv(&self) -> Result<OwnedFrame, TryRecvError> {
        self.frames.try_recv()
    }

    /// The frames dropped because this consumer was behind.
    pub(crate) fn dropped(&self) -> u64 {
        self.frames.stats().dropped()
    }

    pub(crate) fn delivered(&self) -> u64 {
        self.frames.stats().delivered()
    }

    pub(crate) fn room(&self) -> &str {
        &self.room
    }
}

impl Drop for Consumer {
    fn drop(&mut self) {
        let _ = command(Command::Media(MediaCommand::Detach { id: self.id }));
    }
}

/// An injected leg's audio, pushed as PCM.
pub(crate) struct PcmSource {
    room: String,
    leg: String,
    encoder: OpusEncoder,
    frames: SyncSender<Vec<u8>>,
    dropped: u64,
}

/// Adds an injected leg to a room (made when it has no leg yet), whose audio the returned source pushes.
pub(crate) fn add_pcm_source(room: &str, leg: &str) -> Result<PcmSource, &'static str> {
    let encoder = OpusEncoder::new(VOICE_BITRATE).map_err(|_| "codec")?;
    let (tx, rx) = sync_channel(SOURCE_QUEUE);
    let (reply, answer) = sync_channel(1);
    command(Command::Media(MediaCommand::AddSource {
        room: room.to_string(),
        leg: leg.to_string(),
        frames: rx,
        reply,
    }))?;
    wait(&answer)?;
    Ok(PcmSource {
        room: room.to_string(),
        leg: leg.to_string(),
        encoder,
        frames: tx,
        dropped: 0,
    })
}

impl PcmSource {
    /// The next 20 ms: [`FRAME`] samples of 48 kHz mono. Err `badFrame` for another length, `full` when the media
    /// thread is behind (the frame is dropped and counted), `closed` when the leg or its room is gone.
    pub(crate) fn push(&mut self, pcm: &[i16]) -> Result<(), &'static str> {
        if pcm.len() != FRAME {
            return Err("badFrame");
        }
        let packet = self.encoder.encode(pcm).map_err(|_| "codec")?.to_vec();
        match self.frames.try_send(packet) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.dropped += 1;
                Err("full")
            }
            Err(TrySendError::Disconnected(_)) => Err("closed"),
        }
    }

    /// The frames dropped because the media thread was behind.
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(crate) fn leg(&self) -> &str {
        &self.leg
    }
}

impl Drop for PcmSource {
    fn drop(&mut self) {
        let _ = command(Command::Media(MediaCommand::RemoveSource {
            room: std::mem::take(&mut self.room),
            leg: std::mem::take(&mut self.leg),
        }));
    }
}

fn wait(answer: &Receiver<Result<(), &'static str>>) -> Result<(), &'static str> {
    match answer.recv_timeout(REPLY_WAIT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err("timeout"),
        Err(RecvTimeoutError::Disconnected) => Err("noMedia"),
    }
}

/// An injected leg's frames on the media thread, sent one each 20 ms.
pub(super) struct Pacer {
    pending: VecDeque<Vec<u8>>,
    next: Option<Instant>,
    rtp_time: u64,
}

impl Pacer {
    pub(super) fn new() -> Self {
        Pacer {
            pending: VecDeque::new(),
            next: None,
            rtp_time: 0,
        }
    }

    pub(super) fn push(&mut self, frame: Vec<u8>) {
        self.pending.push_back(frame);
        while self.pending.len() > MAX_PENDING {
            self.pending.pop_front();
        }
    }

    /// The frames due by `now`, each with its RTP time (48 kHz). A tick with nothing queued is a gap, its time passing.
    pub(super) fn due(&mut self, now: Instant, out: &mut Vec<(u64, Vec<u8>)>) {
        let mut next = match self.next {
            Some(next) => next,
            None if self.pending.len() >= PREFILL => now,
            None => return,
        };
        let behind = now.saturating_duration_since(next);
        if behind > MAX_BEHIND {
            let skipped = (behind.as_millis() / TICK.as_millis()) as u32;
            next += TICK * skipped;
            self.rtp_time += u64::from(skipped) * FRAME as u64;
        }
        while next <= now {
            if let Some(frame) = self.pending.pop_front() {
                out.push((self.rtp_time, frame));
            }
            self.rtp_time += FRAME as u64;
            next += TICK;
        }
        self.next = Some(next);
    }

    /// When the next frame is due, once started.
    pub(super) fn next_due(&self) -> Option<Instant> {
        self.next
    }
}

struct Source {
    room: String,
    leg: String,
    frames: Receiver<Vec<u8>>,
    pacer: Pacer,
}

/// The driver's half: the room taps by consumer id and the injected legs with their timers.
#[derive(Default)]
pub(super) struct Media {
    taps: HashMap<u64, (String, TapId)>,
    sources: Vec<Source>,
    due: Vec<(u64, Vec<u8>)>,
}

impl Media {
    pub(super) fn command(&mut self, sfu: &mut Sfu, now: Instant, command: MediaCommand) {
        match command {
            MediaCommand::Attach { room, id, tap, reply } => {
                let result = sfu.add_tap(&room, tap).map(|tap_id| {
                    self.taps.insert(id, (room, tap_id));
                });
                let _ = reply.try_send(result.map_err(|e| e.code()));
            }
            MediaCommand::Detach { id } => {
                if let Some((room, tap_id)) = self.taps.remove(&id) {
                    sfu.remove_tap(&room, tap_id);
                }
            }
            MediaCommand::AddSource {
                room,
                leg,
                frames,
                reply,
            } => {
                let result = sfu.add_injected_source(now, &room, &leg).map(|()| {
                    self.sources.retain(|s| !(s.room == room && s.leg == leg));
                    self.sources.push(Source {
                        room,
                        leg,
                        frames,
                        pacer: Pacer::new(),
                    });
                });
                let _ = reply.try_send(result.map_err(|e| e.code()));
            }
            MediaCommand::RemoveSource { room, leg } => self.remove_source(sfu, now, &room, &leg),
        }
    }

    fn remove_source(&mut self, sfu: &mut Sfu, now: Instant, room: &str, leg: &str) {
        let before = self.sources.len();
        self.sources.retain(|s| !(s.room == room && s.leg == leg));
        if self.sources.len() != before {
            sfu.remove_injected_source(now, room, leg);
        }
    }

    /// A leg closed by the call services: when it is injected, it goes.
    pub(super) fn leg_closed(&mut self, sfu: &mut Sfu, now: Instant, room: &str, leg: &str) {
        self.remove_source(sfu, now, room, leg);
    }

    /// The room closed: the SFU dropped its taps and injected legs, and so does this.
    pub(super) fn room_closed(&mut self, room: &str) {
        self.taps.retain(|_, (r, _)| r != room);
        self.sources.retain(|s| s.room != room);
    }

    /// Injects what is due of each injected leg, after taking in what its owner pushed. A leg whose owner is gone is
    /// removed.
    pub(super) fn turn(&mut self, sfu: &mut Sfu, now: Instant) {
        let mut gone = Vec::new();
        for source in &mut self.sources {
            loop {
                match source.frames.try_recv() {
                    Ok(frame) => source.pacer.push(frame),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        gone.push((source.room.clone(), source.leg.clone()));
                        break;
                    }
                }
            }
            self.due.clear();
            source.pacer.due(now, &mut self.due);
            for (rtp_time, frame) in self.due.drain(..) {
                if let Err(err) = sfu.inject(now, &source.room, &source.leg, rtp_time, &frame) {
                    tracing::debug!("call {} injected leg {}: {err}", source.room, source.leg);
                }
            }
        }
        for (room, leg) in gone {
            self.remove_source(sfu, now, &room, &leg);
        }
    }

    /// When the next injected frame is due.
    pub(super) fn next_due(&self) -> Option<Instant> {
        self.sources.iter().filter_map(|s| s.pacer.next_due()).min()
    }
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
