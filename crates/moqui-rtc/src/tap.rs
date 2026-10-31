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
//! forwarding. Its consumer ([`TapReceiver`]) decodes and mixes on its own thread ([`crate::audio`]).

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

    /// Frames dropped because the consumer's queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// A tap that copies each frame into a bounded queue, dropping (and counting) what does not fit.
pub struct QueueTap {
    kinds: Kinds,
    tx: SyncSender<OwnedFrame>,
    stats: Arc<TapStats>,
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
            closed: false,
        };
        (tap, TapReceiver { rx, stats })
    }
}

impl MediaTap for QueueTap {
    fn on_frame(&mut self, frame: &TapFrame<'_>) {
        if self.closed || (self.kinds == Kinds::Audio && frame.kind != Kind::Audio) {
            return;
        }
        match self.tx.try_send(OwnedFrame::copy_of(frame)) {
            Ok(()) => {
                self.stats.delivered.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => self.closed = true,
        }
    }

    fn is_closed(&self) -> bool {
        self.closed
    }
}

/// The consumer's end of a [`QueueTap`]. Dropping it closes the tap.
pub struct TapReceiver {
    rx: Receiver<OwnedFrame>,
    stats: Arc<TapStats>,
}

impl TapReceiver {
    /// The next frame, waiting up to `wait`. `Err(Disconnected)` once the tap is gone (its room closed).
    pub fn recv_timeout(&self, wait: Duration) -> Result<OwnedFrame, RecvTimeoutError> {
        self.rx.recv_timeout(wait)
    }

    /// The next frame if one is waiting. `Err(Disconnected)` once the tap is gone.
    pub fn try_recv(&self) -> Result<OwnedFrame, TryRecvError> {
        self.rx.try_recv()
    }

    pub fn stats(&self) -> &TapStats {
        &self.stats
    }
}
