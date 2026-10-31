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

//! Voicemail's audio files (plans/Comms-Framework.md §5.3, V2; docs/interfaces/Voicemail.md): Ogg Opus read and
//! written with moqui-rtc's in-house Ogg writer (W5) and the M0 Opus codec, and WAV through `prompt.rs`. A message is
//! recorded as Ogg Opus (`voicemail.rs`); a greeting is uploaded as WAV or Ogg Opus and kept as Ogg Opus, which the
//! browser plays and the engine decodes for a caller. `org.moqui.impl.VoicemailServices.convert#GreetingAudio` is the
//! conversion, a Rust service this module registers as a framework extension (the `sip` feature).

use std::collections::BTreeMap;

use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_rtc::audio::opus::{OpusDecoder, OpusEncoder, VOICE_BITRATE};
use moqui_rtc::audio::FRAME;
use moqui_rtc::record::ogg::OggOpusWriter;

/// The encoder's delay in samples, which a player skips (RFC 7845 §4.2).
pub(crate) const PRE_SKIP: u16 = 312;
/// The rate everything here is in.
const RATE: usize = 48_000;
/// The longest greeting kept; longer is cut.
pub(crate) const MAX_GREETING_SECONDS: usize = 60;
/// Where greetings are kept; the conversion reads and writes nowhere else.
const GREETING_ROOT: &str = "dbresource://moqui/call/voicemail/greeting/";

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["sip"],
        register_services,
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn register_services(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, convert_greeting_audio)?;
    Ok(())
}

/// The packets of an Ogg file's first logical stream, its headers included, whole across pages (RFC 3533).
pub(crate) fn ogg_packets(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut packets = Vec::new();
    let mut partial: Vec<u8> = Vec::new();
    let mut serial = None;
    let mut rest = bytes;
    while !rest.is_empty() {
        let header = rest.get(..27).ok_or("a short Ogg page")?;
        if &header[..4] != b"OggS" {
            return Err("not an Ogg page".to_string());
        }
        let segments = usize::from(header[26]);
        let lacing = rest.get(27..27 + segments).ok_or("a short Ogg page")?;
        let body_len: usize = lacing.iter().map(|l| usize::from(*l)).sum();
        let body = rest
            .get(27 + segments..27 + segments + body_len)
            .ok_or("a short Ogg page")?;
        let page_serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
        rest = &rest[27 + segments + body_len..];
        if *serial.get_or_insert(page_serial) != page_serial {
            continue;
        }
        let mut at = 0;
        for size in lacing {
            let size = usize::from(*size);
            partial.extend_from_slice(&body[at..at + size]);
            at += size;
            if size < 255 {
                packets.push(std::mem::take(&mut partial));
            }
        }
    }
    Ok(packets)
}

/// An Ogg Opus file as 48 kHz mono, at most `max_seconds`: its `OpusHead`, its `OpusTags`, then its packets decoded,
/// less the pre-skip.
pub(crate) fn decode_ogg_opus(bytes: &[u8], max_seconds: usize) -> Result<Vec<i16>, String> {
    let packets = ogg_packets(bytes)?;
    let head = packets.first().ok_or("an empty Ogg file")?;
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return Err("not Ogg Opus".to_string());
    }
    let pre_skip = usize::from(u16::from_le_bytes([head[10], head[11]]));
    let mut decoder = OpusDecoder::new().map_err(|e| e.to_string())?;
    let limit = max_seconds * RATE + pre_skip;
    let mut out = Vec::new();
    // the second packet is OpusTags
    for packet in packets.iter().skip(2).filter(|p| !p.is_empty()) {
        decoder.decode(packet, &mut out).map_err(|e| e.to_string())?;
        if out.len() >= limit {
            break;
        }
    }
    out.truncate(limit);
    Ok(out.split_off(pre_skip.min(out.len())))
}

/// A greeting or a message as 48 kHz mono: WAV (16-bit PCM, at most a minute) or Ogg Opus (at most `max_seconds`).
pub(crate) fn decode_audio(bytes: &[u8], max_seconds: usize) -> Result<Vec<i16>, String> {
    if bytes.starts_with(b"RIFF") {
        super::prompt::wav_pcm48(bytes)
    } else if bytes.starts_with(b"OggS") {
        decode_ogg_opus(bytes, max_seconds)
    } else {
        Err("neither WAV nor Ogg".to_string())
    }
}

/// Writes 48 kHz mono audio as Ogg Opus, a frame (20 ms) at a time.
pub(crate) struct OpusFile<W: std::io::Write> {
    writer: OggOpusWriter<W>,
    encoder: OpusEncoder,
    frames: u64,
}

impl<W: std::io::Write> OpusFile<W> {
    pub(crate) fn new(out: W, serial: u32) -> Result<Self, String> {
        Ok(OpusFile {
            writer: OggOpusWriter::new(out, serial, PRE_SKIP).map_err(|e| e.to_string())?,
            encoder: OpusEncoder::new(VOICE_BITRATE).map_err(|e| e.to_string())?,
            frames: 0,
        })
    }

    /// One frame of [`FRAME`] samples (a shorter one is padded with silence).
    pub(crate) fn push(&mut self, pcm: &[i16]) -> Result<(), String> {
        let mut frame = [0i16; FRAME];
        let n = pcm.len().min(FRAME);
        frame[..n].copy_from_slice(&pcm[..n]);
        let packet = self.encoder.encode(&frame).map_err(|e| e.to_string())?;
        self.writer
            .write(self.frames * FRAME as u64, FRAME as u32, packet)
            .map_err(|e| e.to_string())?;
        self.frames += 1;
        Ok(())
    }

    /// The audio taken, in milliseconds.
    pub(crate) fn duration_ms(&self) -> u64 {
        self.frames * 20
    }

    /// The last page written, and the output back.
    pub(crate) fn finish(mut self) -> Result<W, String> {
        // the encoder's delay: a frame of silence flushes the last of the audio out of it
        let silence = [0i16; FRAME];
        if let Ok(packet) = self.encoder.encode(&silence) {
            let _ = self.writer.write(self.frames * FRAME as u64, FRAME as u32, packet);
        }
        self.writer.finish().map(|(out, _)| out).map_err(|e| e.to_string())
    }
}

/// 48 kHz mono audio as an Ogg Opus file.
pub(crate) fn encode_ogg_opus(pcm: &[i16], serial: u32) -> Result<Vec<u8>, String> {
    let mut file = OpusFile::new(Vec::new(), serial)?;
    for frame in pcm.chunks(FRAME) {
        file.push(frame)?;
    }
    file.finish()
}

fn greeting_location(location: &str) -> bool {
    location.strip_prefix(GREETING_ROOT).is_some_and(|rest| {
        !rest.is_empty()
            && !rest
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
    })
}

/// `convert#GreetingAudio`: the greeting at `sourceLocation` (WAV or Ogg Opus) as Ogg Opus at `targetLocation`, both
/// under the greetings' root; audio that does not read is the answer `badAudio`, not an error.
fn convert_greeting_audio(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let text = |key: &str| {
        parameters
            .get(key)
            .filter(|v| !is_empty(v))
            .map(to_plain_string)
            .unwrap_or_default()
    };
    let (source, target) = (text("sourceLocation"), text("targetLocation"));
    if !greeting_location(&source) || !greeting_location(&target) {
        return Err(LibError::new(format!(
            "{source} or {target} is not a voicemail greeting location"
        )));
    }
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("convert#GreetingAudio needs an execution context"))?;
    let bytes = factory
        .resource()
        .location_bytes(&source)
        .map_err(|e| LibError::new(format!("the greeting {source}: {}", e.message())))?
        .ok_or_else(|| LibError::new(format!("no greeting at {source}")))?;
    let mut out = BTreeMap::new();
    let pcm = match decode_audio(&bytes, MAX_GREETING_SECONDS) {
        Ok(pcm) if !pcm.is_empty() => pcm,
        Ok(_) => {
            out.insert("reasonCode".to_string(), Value::string("badAudio"));
            out.insert("errorText".to_string(), Value::string("no audio in it"));
            return Ok(out);
        }
        Err(err) => {
            out.insert("reasonCode".to_string(), Value::string("badAudio"));
            out.insert("errorText".to_string(), Value::string(&err));
            return Ok(out);
        }
    };
    let pcm = &pcm[..pcm.len().min(MAX_GREETING_SECONDS * RATE)];
    let serial = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1);
    let ogg = encode_ogg_opus(pcm, serial).map_err(LibError::new)?;
    factory
        .resource()
        .write_bytes(&target, &ogg)
        .map_err(|e| LibError::new(format!("the greeting {target} was not stored: {}", e.message())))?;
    out.insert(
        "durationMs".to_string(),
        Value::I64(i64::try_from(pcm.len() / 48).unwrap_or(i64::MAX)),
    );
    Ok(out)
}
