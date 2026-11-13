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

//! One trait over the codecs a leg can speak (plans/archive/Comms-Framework.md §5.2): Opus, G.711 μ-law and A-law, G.722. A
//! SIP leg picks its codec by the SDP name it negotiated ([`by_name`]) and resamples between the codec's rate and the
//! mixer's 48 kHz.

use super::g711::{Alaw, Mulaw};
use super::g722::G722;
use super::opus::OpusCodec;

/// A codec failed on a payload (a malformed packet) or a frame (a length it cannot code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

/// A codec with its state: PCM at [`Codec::rate`] in and out, one RTP payload at a time.
pub trait Codec: Send {
    /// The SDP encoding name, lower case (`opus`, `pcmu`, `pcma`, `g722`).
    fn name(&self) -> &'static str;
    /// The PCM sample rate.
    fn rate(&self) -> u32;
    /// The RTP clock rate. G.722 keeps 8000 for its 16 kHz audio (RFC 3551 §4.5.2).
    fn clock_rate(&self) -> u32 {
        self.rate()
    }
    /// The static RTP payload type, for those that have one.
    fn payload_type(&self) -> Option<u8> {
        None
    }
    /// Codes one frame of mono PCM, appending the payload to `out`.
    fn encode(&mut self, pcm: &[i16], out: &mut Vec<u8>) -> Result<(), CodecError>;
    /// Decodes one payload, appending its PCM to `out`.
    fn decode(&mut self, payload: &[u8], out: &mut Vec<i16>) -> Result<(), CodecError>;
    /// Stands in for a lost payload of `samples`, appending them to `out`.
    fn conceal(&mut self, samples: usize, out: &mut Vec<i16>);
}

/// The codec of an SDP encoding name (any case), or `None` for one not built in.
pub fn by_name(name: &str) -> Option<Box<dyn Codec>> {
    let codec: Box<dyn Codec> = match name.to_ascii_lowercase().as_str() {
        "opus" => Box::new(OpusCodec::new().ok()?),
        "pcmu" => Box::new(Mulaw),
        "pcma" => Box::new(Alaw),
        "g722" => Box::new(G722::new()),
        _ => return None,
    };
    Some(codec)
}
