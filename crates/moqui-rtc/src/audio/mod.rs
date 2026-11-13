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

//! The call audio core (plans/archive/Comms-Framework.md §5.1, §5.2, §6.4; the M0 gate in §6.2): the codecs, resampling, a
//! decoder per leg with its jitter buffer, and the mix-minus mixer that SIP legs, the mixed recording (W5) and captions
//! (W6) share.
//!
//! Everything here works on 20 ms frames of 48 kHz mono `i16` PCM ([`RATE`], [`FRAME`]), the rate of the browsers'
//! Opus. A SIP leg's codec runs at its own rate and goes through [`resample`].
//!
//! - [`opus`]: the Opus encoder and decoder (opus-rs, a pure-Rust port of libopus 1.6), with loss concealment and
//!   in-band FEC.
//! - [`g711`] (μ-law and A-law) and [`g722`], in house; [`codec::by_name`] gives any of them behind one [`Codec`]
//!   trait, by its SDP name.
//! - [`resample`]: a polyphase windowed-sinc resampler between 8, 16 and 48 kHz.
//! - [`jitter::LegDecoder`]: a leg's Opus frames, reordered by RTP time, to one PCM frame a tick, concealing gaps.
//! - [`mixer::Mixer`]: each tick, everyone but a given leg, saturated to `i16`, with each leg's level.
//! - [`room::AudioRoom`]: the frames a room's tap hands out ([`crate::tap::OwnedFrame`]), decoded and mixed each tick.
//!
//! None of this runs on the media thread: a [`crate::tap::QueueTap`] copies the encoded frames out, and the consumer
//! decodes and mixes on its own thread. So a slow mixer drops frames at the queue, never stalls forwarding (§6.4).

pub mod codec;
pub mod g711;
pub mod g722;
pub mod jitter;
pub mod mixer;
pub mod opus;
pub mod resample;
pub mod room;

pub use codec::{Codec, CodecError};

/// The sample rate the mixer and the Opus codec work at.
pub const RATE: u32 = 48_000;
/// The samples of one 20 ms frame at [`RATE`].
pub const FRAME: usize = 960;
/// The length of a frame, and of a mixer tick.
pub const FRAME_MS: u64 = 20;
/// The level of silence, in dBov.
pub const SILENT_DBOV: f32 = -127.0;

/// The RMS level of `pcm` in dBov (0 is a full-scale square wave; [`SILENT_DBOV`] for silence).
pub fn level_dbov(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return SILENT_DBOV;
    }
    let energy: f64 = pcm.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    let rms = (energy / pcm.len() as f64).sqrt() / 32_768.0;
    if rms <= 0.0 {
        return SILENT_DBOV;
    }
    (20.0 * rms.log10()).max(f64::from(SILENT_DBOV)) as f32
}

/// `f32` PCM in [-1, 1] to `i16`, rounded and clipped.
pub fn to_i16(sample: f32) -> i16 {
    (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// Clips a sum to `i16`.
pub fn saturate(sample: i32) -> i16 {
    sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[cfg(test)]
mod tests;
