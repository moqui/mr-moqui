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
//! reads what it needs (a phone leg's mix-minus, the whole mix, a leg's own audio for captions).

use std::collections::HashMap;
use std::sync::Arc;

use super::jitter::{JitterConfig, JitterStats, LegDecoder, Pulled};
use super::mixer::Mixer;
use super::FRAME;
use crate::tap::{Kind, OwnedFrame};

/// Ticks without a frame after which a leg's decoder is let go: 5 seconds.
const IDLE_TICKS: u64 = 250;

struct LegAudio {
    decoder: LegDecoder,
    last_heard: u64,
    pcm: Vec<i16>,
    pulled: Pulled,
}

/// The decoded and mixed audio of one room.
pub struct AudioRoom {
    config: JitterConfig,
    legs: HashMap<Arc<str>, LegAudio>,
    mixer: Mixer,
    ticks: u64,
    /// Frames that were not Opus audio, or whose leg's decoder could not be made.
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

    /// A frame from the room's tap. Video and codecs other than Opus are ignored.
    pub fn push(&mut self, frame: &OwnedFrame) {
        if frame.kind != Kind::Audio || frame.codec != "opus" {
            self.ignored += 1;
            return;
        }
        let leg = match self.legs.entry(frame.leg.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => match LegDecoder::new(self.config) {
                Ok(decoder) => e.insert(LegAudio {
                    decoder,
                    last_heard: self.ticks,
                    pcm: vec![0; FRAME],
                    pulled: Pulled::Idle,
                }),
                Err(err) => {
                    tracing::warn!("call audio: no decoder for leg {}: {err}", frame.leg);
                    self.ignored += 1;
                    return;
                }
            },
        };
        leg.last_heard = self.ticks;
        // the Opus clock is 48 kHz; another clock rate is scaled to it
        let rtp_time = if frame.clock_rate == super::RATE || frame.clock_rate == 0 {
            frame.rtp_time
        } else {
            frame.rtp_time * u64::from(super::RATE) / u64::from(frame.clock_rate)
        };
        leg.decoder.push(rtp_time, &frame.data);
    }

    /// The next 20 ms: each leg's frame is pulled from its buffer and put in the mixer. Legs not heard for a while are
    /// let go.
    pub fn tick(&mut self) -> &mut Mixer {
        self.ticks += 1;
        let ticks = self.ticks;
        self.legs
            .retain(|_, l| ticks - l.last_heard < IDLE_TICKS || l.decoder.is_playing());
        self.mixer.clear();
        for (name, leg) in &mut self.legs {
            leg.pulled = leg.decoder.pull(&mut leg.pcm);
            if leg.pulled != Pulled::Idle {
                self.mixer.put(name, &leg.pcm);
            }
        }
        &mut self.mixer
    }

    /// The mixer as of the last tick.
    pub fn mixer(&mut self) -> &mut Mixer {
        &mut self.mixer
    }

    /// A leg's own audio as of the last tick (for captions), `None` when it had none.
    pub fn leg_audio(&self, leg: &str) -> Option<&[i16]> {
        self.legs
            .get(leg)
            .filter(|l| l.pulled != Pulled::Idle)
            .map(|l| l.pcm.as_slice())
    }

    /// The legs heard recently.
    pub fn legs(&self) -> impl Iterator<Item = &Arc<str>> {
        self.legs.keys()
    }

    /// A leg's buffer counts.
    pub fn stats(&self, leg: &str) -> Option<JitterStats> {
        self.legs.get(leg).map(|l| l.decoder.stats())
    }

    /// A leg left: its decoder goes now rather than when idle.
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
