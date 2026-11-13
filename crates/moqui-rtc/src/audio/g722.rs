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

//! G.722 at 64 kbit/s (payload type 9), the wideband codec of desk phones (plans/archive/Comms-Framework.md §5.2, V4): 16 kHz
//! audio split by a quadrature mirror filter into a lower band coded with 6-bit ADPCM and an upper band with 2-bit, one
//! byte per two samples. Written from the blocks of ITU-T G.722 (QMF, BLOCK 1 to 6 of each band, whose names the code
//! keeps), in house, as the published crate brings a media framework. Its RTP clock is 8000 (RFC 3551 §4.5.2).

use super::codec::{Codec, CodecError};

/// The PCM rate of G.722.
pub const RATE: u32 = 16_000;

/// The quadrature mirror filter, half of its 24 symmetric taps.
const QMF: [i32; 12] = [3, -11, 12, 32, -210, 951, 3876, -805, 362, -156, 53, -11];

/// Lower band quantizer decision levels (BLOCK 1L).
const Q6: [i32; 32] = [
    0, 35, 72, 110, 150, 190, 233, 276, 323, 370, 422, 473, 530, 587, 650, 714, 786, 858, 940, 1023, 1121, 1219, 1339,
    1458, 1612, 1765, 1980, 2195, 2557, 2919, 0, 0,
];
/// Lower band codes for a negative and a positive difference, by quantizer interval.
const ILN: [i32; 32] = [
    0, 63, 62, 31, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5,
    4, 0,
];
const ILP: [i32; 32] = [
    0, 61, 60, 59, 58, 57, 56, 55, 54, 53, 52, 51, 50, 49, 48, 47, 46, 45, 44, 43, 42, 41, 40, 39, 38, 37, 36, 35, 34,
    33, 32, 0,
];
/// Lower band log scale factor multipliers (BLOCK 3L), by RL42.
const WL: [i32; 8] = [-60, -30, 58, 172, 334, 538, 1198, 3042];
const RL42: [usize; 16] = [0, 7, 6, 5, 4, 3, 2, 1, 7, 6, 5, 4, 3, 2, 1, 0];
/// The inverse log table of the scale factor (BLOCK 3L and 3H).
const ILB: [i32; 32] = [
    2048, 2093, 2139, 2186, 2233, 2282, 2332, 2383, 2435, 2489, 2543, 2599, 2656, 2714, 2774, 2834, 2896, 2960, 3025,
    3091, 3158, 3228, 3298, 3371, 3444, 3520, 3597, 3676, 3756, 3838, 3922, 4008,
];
/// Lower band inverse quantizers: 4 bits for the predictor (BLOCK 2L), 6 bits for the output (BLOCK 5L).
const QM4: [i32; 16] = [
    0, -20456, -12896, -8968, -6288, -4240, -2584, -1200, 20456, 12896, 8968, 6288, 4240, 2584, 1200, 0,
];
const QM6: [i32; 64] = [
    -136, -136, -136, -136, -24808, -21904, -19008, -16704, -14984, -13512, -12280, -11192, -10232, -9360, -8576,
    -7856, -7192, -6576, -6000, -5456, -4944, -4464, -4008, -3576, -3168, -2776, -2400, -2032, -1688, -1360, -1040,
    -728, 24808, 21904, 19008, 16704, 14984, 13512, 12280, 11192, 10232, 9360, 8576, 7856, 7192, 6576, 6000, 5456,
    4944, 4464, 4008, 3576, 3168, 2776, 2400, 2032, 1688, 1360, 1040, 728, 432, 136, -432, -136,
];
/// Upper band: codes, inverse quantizer, and log scale factor multipliers (BLOCK 1H to 3H).
const IHN: [i32; 3] = [0, 1, 0];
const IHP: [i32; 3] = [0, 3, 2];
const QM2: [i32; 4] = [-7408, -1616, 7408, 1616];
const WH: [i32; 3] = [0, -214, 798];
const RH2: [usize; 4] = [2, 1, 2, 1];

fn sat(x: i32) -> i32 {
    x.clamp(-32_768, 32_767)
}

/// The adaptive predictor of one band (BLOCK 4).
#[derive(Clone, Default)]
struct Band {
    /// The signal estimate.
    s: i32,
    sp: i32,
    sz: i32,
    r: [i32; 3],
    a: [i32; 3],
    ap: [i32; 3],
    p: [i32; 3],
    d: [i32; 7],
    b: [i32; 7],
    bp: [i32; 7],
    sg: [i32; 7],
    /// The log scale factor and the scale factor.
    nb: i32,
    det: i32,
}

impl Band {
    fn new(det: i32) -> Self {
        Band { det, ..Band::default() }
    }

    /// LOGSCL and SCALEL (BLOCK 3): the scale factor follows the code's multiplier.
    fn scale(&mut self, multiplier: i32, limit: i32, shift_base: i32) {
        self.nb = ((self.nb * 127) >> 7) + multiplier;
        self.nb = self.nb.clamp(0, limit);
        let index = ((self.nb >> 6) & 31) as usize;
        let shift = shift_base - (self.nb >> 11);
        let wd3 = if shift < 0 {
            ILB[index] << -shift
        } else {
            ILB[index] >> shift
        };
        self.det = wd3 << 2;
    }

    /// BLOCK 4: the reconstructed signal and the pole and zero predictor updates, given the quantized difference.
    fn adapt(&mut self, d: i32) {
        // RECONS and PARREC
        self.d[0] = d;
        self.r[0] = sat(self.s + d);
        self.p[0] = sat(self.sz + d);

        // UPPOL2
        for i in 0..3 {
            self.sg[i] = self.p[i] >> 15;
        }
        let wd1 = sat(self.a[1] << 2);
        let wd2 = (if self.sg[0] == self.sg[1] { -wd1 } else { wd1 }).min(32_767);
        let mut wd3 = if self.sg[0] == self.sg[2] { 128 } else { -128 };
        wd3 += wd2 >> 7;
        wd3 += (self.a[2] * 32_512) >> 15;
        self.ap[2] = wd3.clamp(-12_288, 12_288);

        // UPPOL1
        self.sg[0] = self.p[0] >> 15;
        self.sg[1] = self.p[1] >> 15;
        let wd1 = if self.sg[0] == self.sg[1] { 192 } else { -192 };
        let wd2 = (self.a[1] * 32_640) >> 15;
        let limit = sat(15_360 - self.ap[2]);
        self.ap[1] = sat(wd1 + wd2).clamp(-limit, limit);

        // UPZERO
        let step = if d == 0 { 0 } else { 128 };
        self.sg[0] = d >> 15;
        for i in 1..7 {
            self.sg[i] = self.d[i] >> 15;
            let wd2 = if self.sg[i] == self.sg[0] { step } else { -step };
            let wd3 = (self.b[i] * 32_640) >> 15;
            self.bp[i] = sat(wd2 + wd3);
        }

        // DELAYA
        for i in (1..7).rev() {
            self.d[i] = self.d[i - 1];
            self.b[i] = self.bp[i];
        }
        for i in (1..3).rev() {
            self.r[i] = self.r[i - 1];
            self.p[i] = self.p[i - 1];
            self.a[i] = self.ap[i];
        }

        // FILTEP
        let wd1 = (self.a[1] * sat(self.r[1] + self.r[1])) >> 15;
        let wd2 = (self.a[2] * sat(self.r[2] + self.r[2])) >> 15;
        self.sp = sat(wd1 + wd2);

        // FILTEZ
        let mut sz = 0;
        for i in (1..7).rev() {
            sz += (self.b[i] * sat(self.d[i] + self.d[i])) >> 15;
        }
        self.sz = sat(sz);

        // PREDIC
        self.s = sat(self.sp + self.sz);
    }
}

/// The QMF delay line, newest two samples last.
#[derive(Clone, Default)]
struct Qmf {
    x: [i32; 24],
}

impl Qmf {
    fn push(&mut self, a: i32, b: i32) {
        self.x.copy_within(2.., 0);
        self.x[22] = a;
        self.x[23] = b;
    }

    /// The two polyphase sums: (the even taps on the odd samples, the odd taps on the even samples).
    fn sums(&self) -> (i32, i32) {
        let (mut even, mut odd) = (0, 0);
        for i in 0..12 {
            odd += self.x[2 * i] * QMF[i];
            even += self.x[2 * i + 1] * QMF[11 - i];
        }
        (even, odd)
    }
}

/// A G.722 encoder.
#[derive(Clone)]
pub struct G722Encoder {
    qmf: Qmf,
    low: Band,
    high: Band,
}

impl Default for G722Encoder {
    fn default() -> Self {
        G722Encoder {
            qmf: Qmf::default(),
            low: Band::new(32),
            high: Band::new(8),
        }
    }
}

impl G722Encoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Codes 16 kHz PCM, two samples a byte, appending to `out`. An odd last sample is coded with a silent partner.
    pub fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) {
        for pair in pcm.chunks(2) {
            let a = i32::from(pair[0]);
            let b = pair.get(1).map_or(0, |s| i32::from(*s));
            out.push(self.encode_pair(a, b));
        }
    }

    fn encode_pair(&mut self, a: i32, b: i32) -> u8 {
        // the transmit QMF
        self.qmf.push(a, b);
        let (even, odd) = self.qmf.sums();
        let xlow = (even + odd) >> 14;
        let xhigh = (even - odd) >> 14;

        // the lower band: SUBTRA, QUANTL (BLOCK 1L)
        let low = &mut self.low;
        let el = sat(xlow - low.s);
        let wd = if el >= 0 { el } else { -(el + 1) };
        let mut i = 1;
        while i < 30 && wd >= (Q6[i] * low.det) >> 12 {
            i += 1;
        }
        let ilow = if el < 0 { ILN[i] } else { ILP[i] };
        // INVQAL (BLOCK 2L), then BLOCK 3L and 4L
        let ril = (ilow >> 2) as usize;
        let dlow = (low.det * QM4[ril]) >> 15;
        low.scale(WL[RL42[ril]], 18_432, 8);
        low.adapt(dlow);

        // the upper band: SUBTRA, QUANTH (BLOCK 1H)
        let high = &mut self.high;
        let eh = sat(xhigh - high.s);
        let wd = if eh >= 0 { eh } else { -(eh + 1) };
        let mih = if wd >= (564 * high.det) >> 12 { 2 } else { 1 };
        let ihigh = if eh < 0 { IHN[mih] } else { IHP[mih] };
        // INVQAH (BLOCK 2H), then BLOCK 3H and 4H
        let dhigh = (high.det * QM2[ihigh as usize]) >> 15;
        high.scale(WH[RH2[ihigh as usize]], 22_528, 10);
        high.adapt(dhigh);

        ((ihigh << 6) | ilow) as u8
    }
}

/// A G.722 decoder.
#[derive(Clone)]
pub struct G722Decoder {
    qmf: Qmf,
    low: Band,
    high: Band,
}

impl Default for G722Decoder {
    fn default() -> Self {
        G722Decoder {
            qmf: Qmf::default(),
            low: Band::new(32),
            high: Band::new(8),
        }
    }
}

impl G722Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes bytes to 16 kHz PCM, two samples each, appending to `out`.
    pub fn decode(&mut self, payload: &[u8], out: &mut Vec<i16>) {
        for code in payload {
            let (a, b) = self.decode_byte(*code);
            out.push(a);
            out.push(b);
        }
    }

    fn decode_byte(&mut self, code: u8) -> (i16, i16) {
        let ilow = usize::from(code & 0x3F);
        let ihigh = usize::from(code >> 6);

        // the lower band: INVQBL and RECONS (BLOCK 5L), LIMIT (BLOCK 6L)
        let low = &mut self.low;
        let rlow = (low.s + ((low.det * QM6[ilow]) >> 15)).clamp(-16_384, 16_383);
        // INVQAL (BLOCK 2L), then BLOCK 3L and 4L
        let ril = ilow >> 2;
        let dlow = (low.det * QM4[ril]) >> 15;
        low.scale(WL[RL42[ril]], 18_432, 8);
        low.adapt(dlow);

        // the upper band: INVQAH and RECONS (BLOCK 2H, 5H), LIMIT (BLOCK 6H)
        let high = &mut self.high;
        let dhigh = (high.det * QM2[ihigh]) >> 15;
        let rhigh = (dhigh + high.s).clamp(-16_384, 16_383);
        high.scale(WH[RH2[ihigh]], 22_528, 10);
        high.adapt(dhigh);

        // the receive QMF
        self.qmf.push(rlow + rhigh, rlow - rhigh);
        let (first, second) = self.qmf.sums();
        (sat(first >> 11) as i16, sat(second >> 11) as i16)
    }
}

/// G.722 behind the [`Codec`] trait. A lost frame is silence; the decoder's state carries on from the last frame.
#[derive(Clone, Default)]
pub struct G722 {
    encoder: G722Encoder,
    decoder: G722Decoder,
}

impl G722 {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Codec for G722 {
    fn name(&self) -> &'static str {
        "g722"
    }

    fn rate(&self) -> u32 {
        RATE
    }

    fn clock_rate(&self) -> u32 {
        8_000
    }

    fn payload_type(&self) -> Option<u8> {
        Some(9)
    }

    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<(), CodecError> {
        self.encoder.encode(pcm, out);
        Ok(())
    }

    fn decode(&mut self, payload: &[u8], out: &mut Vec<i16>) -> Result<(), CodecError> {
        self.decoder.decode(payload, out);
        Ok(())
    }

    fn conceal(&mut self, samples: usize, out: &mut Vec<i16>) {
        out.resize(out.len() + samples, 0);
    }
}
