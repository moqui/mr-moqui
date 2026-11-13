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

//! G.711 μ-law (PCMU, payload type 0) and A-law (PCMA, 8), the trunks' codecs (plans/archive/Comms-Framework.md §5.2): one byte
//! a sample at 8 kHz, by the segment tables of ITU-T G.711. In house, as the published crates bring a media framework.

use super::codec::{Codec, CodecError};

/// The PCM rate of G.711.
pub const RATE: u32 = 8_000;

const MULAW_BIAS: i32 = 0x84;
const MULAW_CLIP: i32 = 32_635;

/// One sample to μ-law.
pub fn mulaw_encode(sample: i16) -> u8 {
    // a negative sample's magnitude is its one's complement, as in ITU-T G.191
    let (x, sign) = if sample < 0 {
        (i32::from(!sample), 0x80)
    } else {
        (i32::from(sample), 0)
    };
    let x = x.min(MULAW_CLIP) + MULAW_BIAS;
    // the segment is the highest set bit above bit 7: x is at least 0x84, at most 0x7FFF
    let exponent = (31 - (x >> 7).leading_zeros()) as i32;
    let mantissa = (x >> (exponent + 3)) & 0x0F;
    !(sign | (exponent << 4) | mantissa) as u8
}

/// One μ-law byte to a sample.
pub fn mulaw_decode(code: u8) -> i16 {
    let u = !code;
    let exponent = (u >> 4) & 0x07;
    let mantissa = i32::from(u & 0x0F);
    let magnitude = (((mantissa << 3) + MULAW_BIAS) << exponent) - MULAW_BIAS;
    (if u & 0x80 != 0 { -magnitude } else { magnitude }) as i16
}

/// The top of each A-law segment, on the 13-bit magnitude.
const ALAW_SEGMENT_END: [i32; 8] = [0x1F, 0x3F, 0x7F, 0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF];

/// One sample to A-law.
pub fn alaw_encode(sample: i16) -> u8 {
    let mut x = i32::from(sample) >> 3;
    let mask = if x >= 0 {
        0xD5
    } else {
        x = -x - 1;
        0x55
    };
    let Some(segment) = ALAW_SEGMENT_END.iter().position(|end| x <= *end) else {
        return (0x7F ^ mask) as u8;
    };
    let shift = if segment < 2 { 1 } else { segment };
    let code = ((segment as i32) << 4) | ((x >> shift) & 0x0F);
    (code ^ mask) as u8
}

/// One A-law byte to a sample.
pub fn alaw_decode(code: u8) -> i16 {
    let a = i32::from(code ^ 0x55);
    let mut t = (a & 0x0F) << 4;
    let segment = (a & 0x70) >> 4;
    match segment {
        0 => t += 8,
        1 => t += 0x108,
        _ => t = (t + 0x108) << (segment - 1),
    }
    (if a & 0x80 != 0 { t } else { -t }) as i16
}

/// PCMU behind the [`Codec`] trait. Stateless; a lost frame is silence.
pub struct Mulaw;

/// PCMA behind the [`Codec`] trait. Stateless; a lost frame is silence.
pub struct Alaw;

impl Codec for Mulaw {
    fn name(&self) -> &'static str {
        "pcmu"
    }

    fn rate(&self) -> u32 {
        RATE
    }

    fn payload_type(&self) -> Option<u8> {
        Some(0)
    }

    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend(pcm.iter().copied().map(mulaw_encode));
        Ok(())
    }

    fn decode(&mut self, payload: &[u8], out: &mut Vec<i16>) -> Result<(), CodecError> {
        out.extend(payload.iter().copied().map(mulaw_decode));
        Ok(())
    }

    fn conceal(&mut self, samples: usize, out: &mut Vec<i16>) {
        out.resize(out.len() + samples, 0);
    }
}

impl Codec for Alaw {
    fn name(&self) -> &'static str {
        "pcma"
    }

    fn rate(&self) -> u32 {
        RATE
    }

    fn payload_type(&self) -> Option<u8> {
        Some(8)
    }

    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<(), CodecError> {
        out.extend(pcm.iter().copied().map(alaw_encode));
        Ok(())
    }

    fn decode(&mut self, payload: &[u8], out: &mut Vec<i16>) -> Result<(), CodecError> {
        out.extend(payload.iter().copied().map(alaw_decode));
        Ok(())
    }

    fn conceal(&mut self, samples: usize, out: &mut Vec<i16>) {
        out.resize(out.len() + samples, 0);
    }
}
