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

//! Opus at 48 kHz mono, as the browsers send it (plans/Comms-Framework.md §6.4), on opus-rs, a pure-Rust port of
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
}

impl OpusDecoder {
    pub fn new() -> Result<Self, CodecError> {
        let inner = opus_rs::OpusDecoder::new(RATE as i32, 1).map_err(|e| CodecError(format!("opus decoder: {e}")))?;
        Ok(OpusDecoder {
            inner,
            pcm: vec![0.0; MAX_PACKET_SAMPLES],
        })
    }

    /// Decodes a packet, appending its samples to `out`; the count appended.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<usize, CodecError> {
        if packet.is_empty() {
            return Err(CodecError("an empty opus packet".to_string()));
        }
        let n = self
            .inner
            .decode(packet, MAX_PACKET_SAMPLES, &mut self.pcm)
            .map_err(|e| CodecError(format!("opus decode: {e}")))?
            .min(self.pcm.len());
        out.extend(self.pcm[..n].iter().copied().map(to_i16));
        Ok(n)
    }

    /// Conceals a lost packet of `samples` (a multiple of 2.5 ms), appending them to `out`.
    pub fn conceal(&mut self, samples: usize, out: &mut Vec<i16>) {
        let samples = samples.min(MAX_PACKET_SAMPLES);
        let n = self.inner.decode(&[], samples, &mut self.pcm).unwrap_or(0).min(samples);
        out.extend(self.pcm[..n].iter().copied().map(to_i16));
        out.resize(out.len() + samples - n, 0);
    }

    /// Recovers a lost packet of `samples` from the in-band FEC of the packet after it (`next`, which is then decoded
    /// as usual), appending them to `out`. Concealed when `next` carries none.
    pub fn recover(&mut self, next: &[u8], samples: usize, out: &mut Vec<i16>) {
        let samples = samples.min(MAX_PACKET_SAMPLES);
        match self.inner.decode_fec(next, samples, &mut self.pcm) {
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
