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

//! A leg's Opus frames to one 20 ms frame of PCM a tick (plans/Comms-Framework.md §5.1, §6.4): a small jitter buffer
//! keyed by RTP time puts reordered frames back in order, and a gap is recovered from the next frame's in-band FEC when
//! it is there, or concealed. A leg that stops (it left, muted, or its browser sends DTX) is silent after a few
//! concealed frames, and starts again at its next frame.

use std::collections::BTreeMap;

use super::opus::OpusDecoder;
use super::{CodecError, FRAME, RATE};

/// A frame this far behind the one playing (1 s) is not late but a new RTP timeline (the sender restarted its
/// stream): the buffer starts over at it rather than dropping the leg's audio as late until it goes quiet.
const RESTART_BEHIND: u64 = RATE as u64;

/// How a leg's frames are buffered.
#[derive(Debug, Clone, Copy)]
pub struct JitterConfig {
    /// The frames held before a leg starts playing: its delay, which absorbs that much jitter.
    pub prefill: usize,
    /// The frames held at most; past this the oldest are dropped, so the delay stays bounded.
    pub max_frames: usize,
    /// The gaps concealed in a row before the leg counts as stopped (silent until it sends again).
    pub max_conceal: u32,
}

impl Default for JitterConfig {
    fn default() -> Self {
        JitterConfig {
            prefill: 2,
            max_frames: 10,
            max_conceal: 5,
        }
    }
}

/// What a tick gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pulled {
    /// Decoded audio.
    Played,
    /// A lost frame, recovered from the next frame's FEC.
    Recovered,
    /// A lost frame, concealed.
    Concealed,
    /// Nothing to play (not started, or stopped): silence, and the leg does not count toward a mix.
    Idle,
}

/// What a leg's buffer saw, for its quality notes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JitterStats {
    pub played: u64,
    pub recovered: u64,
    pub concealed: u64,
    /// Frames that came after their time had played.
    pub late: u64,
    /// Frames dropped because the buffer was full, or that did not decode.
    pub dropped: u64,
}

/// A leg's decoder and jitter buffer.
pub struct LegDecoder {
    decoder: OpusDecoder,
    config: JitterConfig,
    /// Frames waiting, by RTP time (48 kHz).
    frames: BTreeMap<u64, Vec<u8>>,
    /// The RTP time of the next sample to play; `None` while not playing.
    next: Option<u64>,
    /// Decoded samples not yet played (a frame longer than 20 ms leaves some).
    pcm: Vec<i16>,
    /// The samples of the last frame decoded, the length of a gap to conceal.
    last_samples: usize,
    misses: u32,
    stats: JitterStats,
}

impl LegDecoder {
    pub fn new(config: JitterConfig) -> Result<Self, CodecError> {
        Ok(LegDecoder {
            decoder: OpusDecoder::new()?,
            config,
            frames: BTreeMap::new(),
            next: None,
            pcm: Vec::with_capacity(2 * FRAME),
            last_samples: FRAME,
            misses: 0,
            stats: JitterStats::default(),
        })
    }

    pub fn stats(&self) -> JitterStats {
        self.stats
    }

    /// Whether the leg is playing (it has started and not stopped).
    pub fn is_playing(&self) -> bool {
        self.next.is_some()
    }

    /// A frame arrived, at `rtp_time` on the 48 kHz clock.
    pub fn push(&mut self, rtp_time: u64, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if let Some(next) = self.next.filter(|next| rtp_time < *next) {
            if next - rtp_time <= RESTART_BEHIND {
                self.stats.late += 1;
                return;
            }
            self.frames.clear();
            self.pcm.clear();
            self.next = None;
        }
        self.frames.entry(rtp_time).or_insert_with(|| data.to_vec());
        while self.frames.len() > self.config.max_frames {
            let Some((time, _)) = self.frames.pop_first() else {
                break;
            };
            self.stats.dropped += 1;
            if self.next.is_some_and(|next| next <= time) {
                self.next = self.frames.keys().next().copied();
            }
        }
    }

    /// The next 20 ms of the leg into `out` ([`FRAME`] samples): its audio, a recovered or concealed gap, or silence.
    pub fn pull(&mut self, out: &mut [i16]) -> Pulled {
        let mut pulled = Pulled::Played;
        if self.next.is_none() && self.pcm.is_empty() {
            if self.frames.len() < self.config.prefill.max(1) {
                out.fill(0);
                return Pulled::Idle;
            }
            self.next = self.frames.keys().next().copied();
            self.misses = 0;
        }
        while self.pcm.len() < out.len() {
            let Some(next) = self.next else { break };
            if let Some(data) = self.frames.remove(&next) {
                match self.decoder.decode(&data, &mut self.pcm) {
                    Ok(n) if n > 0 => {
                        self.last_samples = n;
                        self.next = Some(next + n as u64);
                        self.misses = 0;
                        self.stats.played += 1;
                    }
                    _ => {
                        // undecodable: a gap like a lost frame
                        self.stats.dropped += 1;
                        self.decoder.conceal(self.last_samples, &mut self.pcm);
                        self.next = Some(next + self.last_samples as u64);
                        pulled = Pulled::Concealed;
                    }
                }
                continue;
            }
            // a gap: stop after too many in a row, or skip a long one (a talk spurt after DTX) to the next frame
            self.misses += 1;
            let first = self.frames.keys().next().copied();
            let longest = u64::from(self.config.max_conceal) * self.last_samples as u64;
            let skip = first.filter(|f| f.saturating_sub(next) > longest);
            if let Some(first) = skip {
                self.next = Some(first);
                continue;
            }
            if self.misses > self.config.max_conceal {
                self.next = None;
                break;
            }
            let after = next + self.last_samples as u64;
            if let Some(following) = self.frames.get(&after) {
                self.decoder.recover(following, self.last_samples, &mut self.pcm);
                self.stats.recovered += 1;
                if pulled == Pulled::Played {
                    pulled = Pulled::Recovered;
                }
            } else {
                self.decoder.conceal(self.last_samples, &mut self.pcm);
                self.stats.concealed += 1;
                pulled = Pulled::Concealed;
            }
            self.next = Some(after);
        }
        let n = self.pcm.len().min(out.len());
        out[..n].copy_from_slice(&self.pcm[..n]);
        out[n..].fill(0);
        self.pcm.drain(..n);
        if n == 0 {
            return Pulled::Idle;
        }
        pulled
    }
}
