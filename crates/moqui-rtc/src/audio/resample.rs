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

//! Streaming sample rate conversion between the codecs' rates and the mixer's (plans/archive/Comms-Framework.md §5.2): 8 kHz
//! G.711 and 16 kHz G.722 to and from 48 kHz. The ratios are small integers, so this is a polyphase FIR: a
//! Kaiser-windowed sinc low-pass at the lower rate's band edge, run as `up` phases and kept every `down`th output. In
//! house rather than `rubato`, whose general (and asynchronous) resamplers bring seven crates for what these fixed ratios
//! need.
//!
//! The filter has 288 taps at the higher rate (6 ms; a 3 ms delay), about 70 dB down in the stop band, and passes 3.3
//! kHz at 8 kHz and 7 kHz at 16 kHz.

/// The prototype filter's length at the up-sampled rate.
const TAPS: usize = 288;
/// The Kaiser window's beta: about 70 dB of stop band.
const KAISER_BETA: f64 = 7.0;
/// The cutoff, as a share of the lower rate's Nyquist frequency: the band edge sits in the transition band.
const CUTOFF: f64 = 0.92;

/// A resampler from one rate to another, keeping its history between calls.
pub struct Resampler {
    up: usize,
    down: usize,
    /// `phases[p][i]` is tap `p + i * up` of the prototype, scaled by `up`.
    phases: Vec<Vec<f32>>,
    /// The last inputs, as many as one phase has taps, in a ring stored twice.
    history: Vec<f32>,
    /// The ring index of the newest input.
    newest: usize,
    /// The next output's position at the up-sampled rate, counted from the newest input in `history`.
    position: usize,
}

impl Resampler {
    /// A resampler from `from` Hz to `to` Hz. Any rates work; a ratio in small integers keeps the filter small.
    pub fn new(from: u32, to: u32) -> Self {
        let g = gcd(from, to).max(1);
        let up = (to / g).max(1) as usize;
        let down = (from / g).max(1) as usize;
        let per_phase = TAPS.div_ceil(up);
        let n = per_phase * up;
        // the cutoff, in cycles per sample at the up-sampled rate
        let fc = CUTOFF * 0.5 / up.max(down) as f64;
        let mid = (n - 1) as f64 / 2.0;
        let i0_beta = bessel_i0(KAISER_BETA);
        let prototype: Vec<f64> = (0..n)
            .map(|k| {
                let t = k as f64 - mid;
                let sinc = if t == 0.0 {
                    2.0 * fc
                } else {
                    (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t)
                };
                let r = t / mid;
                let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
                sinc * window
            })
            .collect();
        // unity gain at DC for each phase
        let sum: f64 = prototype.iter().sum();
        let gain = up as f64 / sum;
        let phases = (0..up)
            .map(|p| (0..per_phase).map(|i| (prototype[p + i * up] * gain) as f32).collect())
            .collect();
        Resampler {
            up,
            down,
            phases,
            history: vec![0.0; 2 * per_phase],
            newest: 0,
            position: 0,
        }
    }

    /// The output samples `input` makes, appended to `out`. A 20 ms frame in gives a 20 ms frame out.
    pub fn process(&mut self, input: &[i16], out: &mut Vec<i16>) {
        let per_phase = self.history.len() / 2;
        for sample in input {
            // a ring stored twice, so the window of the last `per_phase` inputs is one slice, oldest first
            self.newest = (self.newest + 1) % per_phase;
            let value = f32::from(*sample);
            self.history[self.newest] = value;
            self.history[self.newest + per_phase] = value;
            let window = &self.history[self.newest + 1..self.newest + 1 + per_phase];
            // the outputs whose newest input this sample is: positions 0..up after it
            while self.position < self.up {
                let acc: f32 = self.phases[self.position]
                    .iter()
                    .zip(window.iter().rev())
                    .map(|(tap, x)| tap * x)
                    .sum();
                out.push(acc.round().clamp(-32_768.0, 32_767.0) as i16);
                self.position += self.down;
            }
            self.position -= self.up;
        }
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// The modified Bessel function of the first kind, order 0, by its series.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < sum * 1e-12 {
            break;
        }
    }
    sum
}
