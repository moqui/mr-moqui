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

//! The mix-minus mixer of a room (plans/Comms-Framework.md §5.1, §6.4): each 20 ms tick, the legs that have audio put
//! their frame, and each listener takes the sum of every frame but its own. A phone hears everyone but itself; the
//! mixed recording (W5) takes everyone ([`Mixer::all`]).
//!
//! Sums are kept in `i32`, so no partial sum overflows, and saturated to `i16` on the way out (counted in
//! [`Mixer::clipped`]). A leg that puts nothing in a tick is silent in it, so legs join, leave and miss frames with no
//! more than that.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{level_dbov, saturate, FRAME, SILENT_DBOV};

/// The frame a leg put this tick.
struct Input {
    pcm: Vec<i16>,
    level: f32,
}

/// One room's mixer.
pub struct Mixer {
    inputs: BTreeMap<Arc<str>, Input>,
    sum: Vec<i32>,
    out: Vec<i16>,
    clipped: u64,
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer {
            inputs: BTreeMap::new(),
            sum: vec![0; FRAME],
            out: vec![0; FRAME],
            clipped: 0,
        }
    }
}

impl Mixer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a tick: every leg is silent until it puts a frame.
    pub fn clear(&mut self) {
        self.inputs.clear();
        self.sum.fill(0);
    }

    /// A leg's frame for this tick ([`FRAME`] samples; a shorter one is padded with silence, a longer one cut). A
    /// second frame from the same leg in a tick replaces its first.
    pub fn put(&mut self, leg: &Arc<str>, pcm: &[i16]) {
        let n = pcm.len().min(FRAME);
        if let Some(old) = self.inputs.get(leg) {
            for (s, v) in self.sum.iter_mut().zip(&old.pcm) {
                *s -= i32::from(*v);
            }
        }
        for (s, v) in self.sum.iter_mut().zip(&pcm[..n]) {
            *s += i32::from(*v);
        }
        let mut frame = pcm[..n].to_vec();
        frame.resize(FRAME, 0);
        let level = level_dbov(&frame);
        self.inputs.insert(leg.clone(), Input { pcm: frame, level });
    }

    /// Adds a frame to what a leg put this tick, saturated: a leg that sends two audio tracks (a microphone and a
    /// screen share's audio) is one input, which its own mix-minus leaves out whole.
    pub fn add(&mut self, leg: &Arc<str>, pcm: &[i16]) {
        let Some(old) = self.inputs.get(leg) else {
            self.put(leg, pcm);
            return;
        };
        let mixed: Vec<i16> = old
            .pcm
            .iter()
            .zip(pcm.iter().copied().chain(std::iter::repeat(0)))
            .map(|(a, b)| saturate(i32::from(*a) + i32::from(b)))
            .collect();
        self.put(leg, &mixed);
    }

    /// A leg's frame this tick ([`FRAME`] samples), `None` when it put none.
    pub fn frame(&self, leg: &str) -> Option<&[i16]> {
        self.inputs.get(leg).map(|i| i.pcm.as_slice())
    }

    /// The legs that put a frame this tick.
    pub fn legs(&self) -> impl Iterator<Item = &Arc<str>> {
        self.inputs.keys()
    }

    /// The level of a leg's frame this tick in dBov, [`SILENT_DBOV`] when it put none.
    pub fn level(&self, leg: &str) -> f32 {
        self.inputs.get(leg).map_or(SILENT_DBOV, |i| i.level)
    }

    /// Everyone but `leg` (mix-minus), saturated: what that leg hears. Any name works; one that put nothing hears all.
    pub fn minus(&mut self, leg: &str) -> &[i16] {
        self.mix_without(Some(leg))
    }

    /// Everyone, saturated: the room's mix.
    pub fn all(&mut self) -> &[i16] {
        self.mix_without(None)
    }

    fn mix_without(&mut self, leg: Option<&str>) -> &[i16] {
        let own = leg.and_then(|l| self.inputs.get(l)).map(|i| i.pcm.as_slice());
        let mut clipped = 0;
        for (i, out) in self.out.iter_mut().enumerate() {
            let v = self.sum[i] - own.map_or(0, |o| i32::from(o[i]));
            *out = saturate(v);
            if i32::from(*out) != v {
                clipped += 1;
            }
        }
        self.clipped += clipped;
        &self.out
    }

    /// The samples saturated so far.
    pub fn clipped(&self) -> u64 {
        self.clipped
    }
}
