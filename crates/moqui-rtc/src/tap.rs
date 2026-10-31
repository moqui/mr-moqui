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

//! Media taken out of and put into a room (plans/Comms-Framework.md §5.1, §6.2): the seam the SIP mixer, recording and
//! captions use. The [`MediaTap`]s added to a room ([`crate::Sfu::add_tap`], several per room) see every frame a
//! participant publishes, after depacketizing and before forwarding, and every frame of an injected source. An injected
//! source ([`crate::Sfu::add_injected_source`]) is a participant with no browser: the frames its owner writes
//! ([`crate::Sfu::inject`]) are forwarded to every subscriber like a published track.
//!
//! A tap runs inside the media loop, so it must not block: it copies what it needs and hands it to its own bounded
//! queue (§6.4, "Queues for captions and recording"). [`QueueTap`] is that tap: a copy of each frame into a bounded
//! channel, dropped and counted when the consumer is behind, so a slow recorder or caption provider never holds up
//! forwarding. Its consumer ([`TapReceiver`]) decodes and mixes on its own thread ([`crate::audio`]). It also hears
//! when a leg leaves the room ([`TapEvent::Left`]), in order with the leg's frames, so a recorder can end the leg's
//! tracks and a mixer let its decoder go.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::Duration;

use crate::sfu::Source;

/// The kind of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Audio,
    Video,
}

/// A frame as a tap sees it.
#[derive(Debug)]
pub struct TapFrame<'a> {
    /// The publishing leg.
    pub leg: &'a Arc<str>,
    pub kind: Kind,
    pub source: Source,
    /// The simulcast layer, for video sent in layers.
    pub rid: Option<&'a str>,
    /// The codec name, for example `opus` or `vp8`.
    pub codec: &'static str,
    /// The RTP media time: `rtp_time / clock_rate` seconds.
    pub rtp_time: u64,
    pub clock_rate: u32,
    /// The encoded frame.
    pub data: &'a [u8],
    pub keyframe: bool,
    /// The audio level header extension, in negative dBov (0 is loudest, -127 silence).
    pub audio_level: Option<i8>,
}

/// Sees every frame published in a room.
pub trait MediaTap: Send {
    fn on_frame(&mut self, frame: &TapFrame<'_>);

    /// A leg left the room: a browser's peer was removed (it left, or a new generation of it took its place), or an
    /// injected source was. Its frames, if it comes back, start a new RTP timeline.
    fn on_leave(&mut self, _leg: &Arc<str>) {}

    /// True once the consumer is gone; the room then drops the tap.
    fn is_closed(&self) -> bool {
        false
    }
}

/// A frame copied out of the media loop.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedFrame {
    pub leg: Arc<str>,
    pub kind: Kind,
    pub source: Source,
    pub rid: Option<String>,
    pub codec: &'static str,
    pub rtp_time: u64,
    pub clock_rate: u32,
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub audio_level: Option<i8>,
}

impl OwnedFrame {
    pub fn copy_of(frame: &TapFrame<'_>) -> Self {
        OwnedFrame {
            leg: frame.leg.clone(),
            kind: frame.kind,
            source: frame.source,
            rid: frame.rid.map(str::to_string),
            codec: frame.codec,
            rtp_time: frame.rtp_time,
            clock_rate: frame.clock_rate,
            data: frame.data.to_vec(),
            keyframe: frame.keyframe,
            audio_level: frame.audio_level,
        }
    }
}

/// What a [`TapReceiver`] gives: a frame, or a leg that left.
#[derive(Debug, Clone, PartialEq)]
pub enum TapEvent {
    Frame(OwnedFrame),
    /// The leg left the room ([`MediaTap::on_leave`]); its frames before this came first.
    Left(Arc<str>),
}

/// Which frames a [`QueueTap`] copies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kinds {
    /// Audio only: the mixer and captions.
    Audio,
    /// Audio and video: the per-track recorder.
    All,
}

/// What a queue passed on and dropped.
#[derive(Debug, Default)]
pub struct TapStats {
    delivered: AtomicU64,
    dropped: AtomicU64,
}

impl TapStats {
    pub fn delivered(&self) -> u64 {
        self.delivered.load(Ordering::Relaxed)
    }

    /// Frames dropped because the consumer's queue was full (a leave is held until there is room, never dropped).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// A tap that copies each frame into a bounded queue, dropping (and counting) what does not fit.
pub struct QueueTap {
    kinds: Kinds,
    tx: SyncSender<TapEvent>,
    stats: Arc<TapStats>,
    /// Legs that left while the queue was full, sent before the next frame so none is lost.
    left: Vec<Arc<str>>,
    closed: bool,
}

impl QueueTap {
    /// A tap and its consumer's end, holding at most `capacity` frames.
    pub fn channel(capacity: usize, kinds: Kinds) -> (QueueTap, TapReceiver) {
        let (tx, rx) = sync_channel(capacity.max(1));
        let stats = Arc::new(TapStats::default());
        let tap = QueueTap {
            kinds,
            tx,
            stats: stats.clone(),
            left: Vec::new(),
            closed: false,
        };
        (tap, TapReceiver { rx, stats })
    }

    /// Sends the leaves held back by a full queue; false while it is still full.
    fn flush_left(&mut self) -> bool {
        while let Some(leg) = self.left.first() {
            match self.tx.try_send(TapEvent::Left(leg.clone())) {
                Ok(()) => {
                    self.left.remove(0);
                }
                Err(TrySendError::Full(_)) => return false,
                Err(TrySendError::Disconnected(_)) => {
                    self.closed = true;
                    return false;
                }
            }
        }
        true
    }
}

impl MediaTap for QueueTap {
    fn on_frame(&mut self, frame: &TapFrame<'_>) {
        if self.closed || (self.kinds == Kinds::Audio && frame.kind != Kind::Audio) {
            return;
        }
        if !self.flush_left() {
            if !self.closed {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        match self.tx.try_send(TapEvent::Frame(OwnedFrame::copy_of(frame))) {
            Ok(()) => {
                self.stats.delivered.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => self.closed = true,
        }
    }

    fn on_leave(&mut self, leg: &Arc<str>) {
        if self.closed {
            return;
        }
        self.left.push(leg.clone());
        self.flush_left();
    }

    /// Closed once a send found the consumer gone, or its end ([`TapReceiver`], which shares the stats) was dropped.
    fn is_closed(&self) -> bool {
        self.closed || Arc::strong_count(&self.stats) == 1
    }
}

/// The consumer's end of a [`QueueTap`]. Dropping it closes the tap.
pub struct TapReceiver {
    rx: Receiver<TapEvent>,
    stats: Arc<TapStats>,
}

impl TapReceiver {
    /// The next frame or leave, waiting up to `wait`. `Err(Disconnected)` once the tap is gone (its room closed).
    pub fn recv_timeout(&self, wait: Duration) -> Result<TapEvent, RecvTimeoutError> {
        self.rx.recv_timeout(wait)
    }

    /// The next frame or leave if one is waiting. `Err(Disconnected)` once the tap is gone.
    pub fn try_recv(&self) -> Result<TapEvent, TryRecvError> {
        self.rx.try_recv()
    }

    pub fn stats(&self) -> &TapStats {
        &self.stats
    }
}
