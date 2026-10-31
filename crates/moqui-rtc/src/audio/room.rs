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

//! A room's audio as its consumers want it (plans/Comms-Framework.md §5.1, §6.4): the Opus frames a
//! [`crate::tap::QueueTap`] hands out, a [`LegDecoder`] per leg, and the [`Mixer`] filled each tick. The consumer owns
//! it on its own thread: it pushes the frames it receives as they come and calls [`AudioRoom::tick`] every 20 ms, then
//! reads what it needs (a phone leg's mix-minus, the whole mix, a leg's own audio for captions). A leg's microphone and
//! its screen share's audio are separate streams (each its own RTP timeline and decoder), mixed as one input of the
//! leg.

use std::collections::HashMap;
use std::sync::Arc;

use super::jitter::{JitterConfig, JitterStats, LegDecoder, Pulled};
use super::mixer::Mixer;
use super::FRAME;
use crate::sfu::Source;
use crate::tap::{Kind, OwnedFrame, TapEvent};

/// Ticks without a frame after which a stream's decoder is let go: 5 seconds.
const IDLE_TICKS: u64 = 250;

/// One audio track of a leg: its microphone (or a phone's voice), or its screen share's audio. Each has its own RTP
/// timeline, so each has its own decoder.
struct Stream {
    source: Source,
    decoder: LegDecoder,
    last_heard: u64,
    pcm: Vec<i16>,
    pulled: Pulled,
}

/// The decoded and mixed audio of one room.
pub struct AudioRoom {
    config: JitterConfig,
    legs: HashMap<Arc<str>, Vec<Stream>>,
    mixer: Mixer,
    ticks: u64,
    /// Frames that were not Opus audio, or whose stream's decoder could not be made.
    ignored: u64,
}

impl AudioRoom {
    pub fn new(config: JitterConfig) -> Self {
        AudioRoom {
            config,
            legs: HashMap::new(),
            mixer: Mixer::new(),
            ticks: 0,
            ignored: 0,
        }
    }

    /// What a tap's consumer received ([`crate::tap::TapReceiver`]): a frame is pushed, a leg that left is removed.
    pub fn take(&mut self, event: &TapEvent) {
        match event {
            TapEvent::Frame(frame) => self.push(frame),
            TapEvent::Left(leg) => self.remove_leg(leg),
        }
    }

    /// A frame from the room's tap. Video and codecs other than Opus are ignored.
    pub fn push(&mut self, frame: &OwnedFrame) {
        if frame.kind != Kind::Audio || frame.codec != "opus" {
            self.ignored += 1;
            return;
        }
        let streams = self.legs.entry(frame.leg.clone()).or_default();
        let stream = match streams.iter().position(|s| s.source == frame.source) {
            Some(at) => &mut streams[at],
            None => match LegDecoder::new(self.config) {
                Ok(decoder) => {
                    streams.push(Stream {
                        source: frame.source,
                        decoder,
                        last_heard: self.ticks,
                        pcm: vec![0; FRAME],
                        pulled: Pulled::Idle,
                    });
                    let at = streams.len() - 1;
                    &mut streams[at]
                }
                Err(err) => {
                    tracing::warn!("call audio: no decoder for leg {}: {err}", frame.leg);
                    if streams.is_empty() {
                        self.legs.remove(&frame.leg);
                    }
                    self.ignored += 1;
                    return;
                }
            },
        };
        stream.last_heard = self.ticks;
        // the Opus clock is 48 kHz; another clock rate is scaled to it
        let rtp_time = if frame.clock_rate == super::RATE || frame.clock_rate == 0 {
            frame.rtp_time
        } else {
            frame.rtp_time * u64::from(super::RATE) / u64::from(frame.clock_rate)
        };
        stream.decoder.push(rtp_time, &frame.data);
    }

    /// The next 20 ms: each stream's frame is pulled from its buffer and put in the mixer, a leg's streams as one
    /// input. Streams not heard for a while are let go.
    pub fn tick(&mut self) -> &mut Mixer {
        self.ticks += 1;
        let ticks = self.ticks;
        self.legs.retain(|_, streams| {
            streams.retain(|s| ticks - s.last_heard < IDLE_TICKS || s.decoder.is_playing());
            !streams.is_empty()
        });
        self.mixer.clear();
        for (name, streams) in &mut self.legs {
            for stream in streams {
                stream.pulled = stream.decoder.pull(&mut stream.pcm);
                if stream.pulled != Pulled::Idle {
                    self.mixer.add(name, &stream.pcm);
                }
            }
        }
        &mut self.mixer
    }

    /// The mixer as of the last tick.
    pub fn mixer(&mut self) -> &mut Mixer {
        &mut self.mixer
    }

    /// A leg's own audio as of the last tick (for captions), `None` when it had none: what the others hear of it, its
    /// streams together.
    pub fn leg_audio(&self, leg: &str) -> Option<&[i16]> {
        self.mixer.frame(leg)
    }

    /// One stream of a leg as of the last tick, `None` when it had none: a speaker's microphone without their screen
    /// share's audio.
    pub fn source_audio(&self, leg: &str, source: Source) -> Option<&[i16]> {
        self.legs
            .get(leg)?
            .iter()
            .find(|s| s.source == source && s.pulled != Pulled::Idle)
            .map(|s| s.pcm.as_slice())
    }

    /// The legs heard recently.
    pub fn legs(&self) -> impl Iterator<Item = &Arc<str>> {
        self.legs.keys()
    }

    /// A leg's buffer counts, its streams added together.
    pub fn stats(&self, leg: &str) -> Option<JitterStats> {
        let streams = self.legs.get(leg)?;
        Some(streams.iter().fold(JitterStats::default(), |sum, s| {
            let one = s.decoder.stats();
            JitterStats {
                played: sum.played + one.played,
                recovered: sum.recovered + one.recovered,
                concealed: sum.concealed + one.concealed,
                late: sum.late + one.late,
                dropped: sum.dropped + one.dropped,
            }
        }))
    }

    /// A leg left: its decoders go now rather than when idle, and if it comes back its new RTP timeline starts fresh.
    pub fn remove_leg(&mut self, leg: &str) {
        self.legs.remove(leg);
    }

    /// Frames ignored: not Opus audio, or no decoder.
    pub fn ignored(&self) -> u64 {
        self.ignored
    }
}

impl Default for AudioRoom {
    fn default() -> Self {
        Self::new(JitterConfig::default())
    }
}
