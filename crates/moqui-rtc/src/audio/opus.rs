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

//! Opus at 48 kHz mono, as the browsers send it (plans/archive/Comms-Framework.md §6.4), on opus-rs, a pure-Rust port of
//! libopus 1.6. The M0 spike (§6.2) chose it: its decoder matches libopus to float rounding on clean, lost (concealed),
//! FEC and DTX streams. Swapping the library is this file.

use super::codec::{Codec, CodecError};
use super::{to_i16, RATE};

/// The most samples one packet decodes to: 120 ms.
const MAX_PACKET_SAMPLES: usize = 5_760;
/// The largest packet (RFC 6716 §3.4).
const MAX_PACKET_BYTES: usize = 1_276;
/// The bitrate of the voice the server encodes (a SIP leg toward the browsers), as the browsers' default.
pub const VOICE_BITRATE: i32 = 32_000;
/// The loss the encoder spends in-band FEC for.
const EXPECTED_LOSS_PERCENT: i32 = 10;

/// Encodes 20 ms frames of 48 kHz mono speech, with in-band FEC.
pub struct OpusEncoder {
    inner: opus_rs::OpusEncoder,
    packet: Vec<u8>,
}

impl OpusEncoder {
    /// A voice encoder at `bitrate` bits a second.
    pub fn new(bitrate: i32) -> Result<Self, CodecError> {
        let mut inner = opus_rs::OpusEncoder::new(RATE as i32, 1, opus_rs::Application::Voip)
            .map_err(|e| CodecError(format!("opus encoder: {e}")))?;
        inner.bitrate_bps = bitrate;
        inner.use_inband_fec = true;
        inner.packet_loss_perc = EXPECTED_LOSS_PERCENT;
        Ok(OpusEncoder {
            inner,
            packet: vec![0; MAX_PACKET_BYTES],
        })
    }

    /// Codes one frame of [`super::FRAME`] samples (or another Opus frame size: 2.5 to 60 ms) to a packet.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<&[u8], CodecError> {
        let n = self
            .inner
            .encode_i16(pcm, pcm.len(), &mut self.packet)
            .map_err(|e| CodecError(format!("opus encode: {e}")))?;
        Ok(&self.packet[..n])
    }
}

/// Decodes Opus packets of any frame size to 48 kHz mono, conceals lost ones, and recovers them from the next packet's
/// in-band FEC.
pub struct OpusDecoder {
    inner: opus_rs::OpusDecoder,
    pcm: Vec<f32>,
    /// Packets refused as not Opus (RFC 6716 §3.4) or that made the library panic, on this leg.
    failures: u32,
}

/// Failures of one leg's packets past which its decoder decodes no more: each is concealed, and the log has the first
/// and the stop only (SEC-COM-87).
const MAX_FAILURES: u32 = 100;
/// The longest frame of an Opus packet, in bytes (RFC 6716 §3.4, R2).
const MAX_FRAME_BYTES: usize = 1275;

/// Whether `packet` is a well-formed Opus packet by RFC 6716 §3.4 (R1 to R7): its TOC byte, its frame count code and
/// each frame's length against the packet's, so the library never indexes past it (SEC-COM-87).
#[must_use]
pub fn packet_valid(packet: &[u8]) -> bool {
    let Some(&toc) = packet.first() else {
        return false;
    };
    // a frame's length: one byte below 252, else two (R4 and the length coding of §3.2.1)
    let length = |bytes: &[u8]| -> Option<(usize, usize)> {
        match *bytes.first()? {
            first if first < 252 => Some((usize::from(first), 1)),
            first => Some((usize::from(first) + 4 * usize::from(*bytes.get(1)?), 2)),
        }
    };
    let rest = &packet[1..];
    match toc & 0x3 {
        0 => rest.len() <= MAX_FRAME_BYTES,
        1 => rest.len().is_multiple_of(2) && rest.len() / 2 <= MAX_FRAME_BYTES,
        2 => length(rest).is_some_and(|(first, used)| {
            first <= MAX_FRAME_BYTES
                && first <= rest.len() - used
                && rest.len() - used - first <= MAX_FRAME_BYTES
        }),
        _ => {
            let Some(&count_byte) = rest.first() else {
                return false;
            };
            let (vbr, padded, count) = (count_byte & 0x80 != 0, count_byte & 0x40 != 0, usize::from(count_byte & 0x3f));
            // the frames' total duration is at most 120 ms (R5), in units of 2.5 ms
            let config = toc >> 3;
            let units = match config {
                0..=11 => [4, 8, 16, 24][usize::from(config % 4)],
                12..=15 => [4, 8][usize::from(config % 2)],
                _ => [1, 2, 4, 8][usize::from(config % 4)],
            };
            if count == 0 || count * units > 48 {
                return false;
            }
            let mut at = 1;
            let mut padding = 0usize;
            if padded {
                loop {
                    let Some(&byte) = rest.get(at) else {
                        return false;
                    };
                    at += 1;
                    padding += if byte == 255 { 254 } else { usize::from(byte) };
                    if byte != 255 {
                        break;
                    }
                }
            }
            let Some(mut left) = rest.len().checked_sub(at + padding) else {
                return false;
            };
            if !vbr {
                return left % count == 0 && left / count <= MAX_FRAME_BYTES;
            }
            for _ in 1..count {
                let Some((frame, used)) = rest.get(at..).and_then(length) else {
                    return false;
                };
                at += used;
                let Some(after) = left.checked_sub(used + frame) else {
                    return false;
                };
                if frame > MAX_FRAME_BYTES {
                    return false;
                }
                left = after;
            }
            left <= MAX_FRAME_BYTES
        }
    }
}

impl OpusDecoder {
    pub fn new() -> Result<Self, CodecError> {
        let inner = opus_rs::OpusDecoder::new(RATE as i32, 1).map_err(|e| CodecError(format!("opus decoder: {e}")))?;
        Ok(OpusDecoder {
            inner,
            pcm: vec![0.0; MAX_PACKET_SAMPLES],
            failures: 0,
        })
    }

    /// A call into the library, which reads a packet's own lengths with indexing that may panic on a hostile one
    /// (SEC-COM-87), on the media threads of a call: a panic is caught and the decoder made anew, since its state is
    /// not trusted after one, so one bad packet is one undecodable frame, not the end of the process or of the call.
    pub(crate) fn guarded<T>(&mut self, call: impl FnOnce(&mut opus_rs::OpusDecoder, &mut [f32]) -> T) -> Option<T> {
        let (inner, pcm) = (&mut self.inner, &mut self.pcm);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(inner, pcm))) {
            Ok(value) => Some(value),
            Err(_) => {
                self.failed("the opus decoder panicked on a packet; it is made anew and the packet dropped");
                if let Ok(fresh) = opus_rs::OpusDecoder::new(RATE as i32, 1) {
                    self.inner = fresh;
                }
                None
            }
        }
    }

    /// Count a failure of this leg's packets, logged the first time and when the leg stops being decoded.
    fn failed(&mut self, what: &str) {
        self.failures = self.failures.saturating_add(1);
        if self.failures == 1 {
            tracing::warn!("{what}");
        } else if self.failures == MAX_FAILURES {
            tracing::warn!("{what}; {MAX_FAILURES} such packets on this leg: it is not decoded any more");
        }
    }

    /// Whether this leg's packets are no longer decoded, past [`MAX_FAILURES`].
    pub fn stopped(&self) -> bool {
        self.failures >= MAX_FAILURES
    }

    /// Decodes a packet, appending its samples to `out`; the count appended.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<usize, CodecError> {
        if packet.is_empty() {
            return Err(CodecError("an empty opus packet".to_string()));
        }
        if self.stopped() {
            return Err(CodecError("opus decode: this leg's packets are no longer decoded".to_string()));
        }
        if !packet_valid(packet) {
            self.failed("an opus packet that is not one by RFC 6716 was refused");
            return Err(CodecError("opus decode: not an opus packet".to_string()));
        }
        let n = self
            .guarded(|inner, pcm| inner.decode(packet, MAX_PACKET_SAMPLES, pcm))
            .ok_or_else(|| CodecError("opus decode: the decoder panicked".to_string()))?
            .map_err(|e| CodecError(format!("opus decode: {e}")))?
            .min(self.pcm.len());
        out.extend(self.pcm[..n].iter().copied().map(to_i16));
        Ok(n)
    }

    /// Conceals a lost packet of `samples` (a multiple of 2.5 ms), appending them to `out`.
    pub fn conceal(&mut self, samples: usize, out: &mut Vec<i16>) {
        let samples = samples.min(MAX_PACKET_SAMPLES);
        let n = self
            .guarded(|inner, pcm| inner.decode(&[], samples, pcm).unwrap_or(0))
            .unwrap_or(0)
            .min(samples);
        out.extend(self.pcm[..n].iter().copied().map(to_i16));
        out.resize(out.len() + samples - n, 0);
    }

    /// Recovers a lost packet of `samples` from the in-band FEC of the packet after it (`next`, which is then decoded
    /// as usual), appending them to `out`. Concealed when `next` carries none.
    pub fn recover(&mut self, next: &[u8], samples: usize, out: &mut Vec<i16>) {
        let samples = samples.min(MAX_PACKET_SAMPLES);
        if self.stopped() || !packet_valid(next) {
            return self.conceal(samples, out);
        }
        match self
            .guarded(|inner, pcm| inner.decode_fec(next, samples, pcm))
            .unwrap_or(Err("the decoder panicked"))
        {
            Ok(n) => {
                let n = n.min(samples);
                out.extend(self.pcm[..n].iter().copied().map(to_i16));
                out.resize(out.len() + samples - n, 0);
            }
            Err(_) => self.conceal(samples, out),
        }
    }
}

/// Opus behind the [`Codec`] trait: a SIP leg that negotiated Opus.
pub struct OpusCodec {
    encoder: OpusEncoder,
    decoder: OpusDecoder,
}

impl OpusCodec {
    pub fn new() -> Result<Self, CodecError> {
        Ok(OpusCodec {
            encoder: OpusEncoder::new(VOICE_BITRATE)?,
            decoder: OpusDecoder::new()?,
        })
    }
}

impl Codec for OpusCodec {
    fn name(&self) -> &'static str {
        "opus"
    }

    fn rate(&self) -> u32 {
        RATE
    }

    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend_from_slice(self.encoder.encode(pcm)?);
        Ok(())
    }

    fn decode(&mut self, payload: &[u8], out: &mut Vec<i16>) -> Result<(), CodecError> {
        self.decoder.decode(payload, out).map(|_| ())
    }

    fn conceal(&mut self, samples: usize, out: &mut Vec<i16>) {
        self.decoder.conceal(samples, out);
    }
}
