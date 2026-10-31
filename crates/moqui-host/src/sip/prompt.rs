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

//! A call's notices to a phone leg (docs/interfaces/SIP.md, "Notices"): what a browser sees as an `rtc` frame (the
//! recording, captions) a phone hears as a prompt. The voice services ask for one with `send#SipControl` (action
//! `prompt`, from `VoiceServices.notify#PhoneLeg`); each is a tone pattern of its own, then the deployment's recorded
//! audio when a property names a WAV file (`call_recording_prompt_audio`, `call_captions_prompt_audio`).

use std::sync::Arc;

use moqui_rtc::audio::resample::Resampler;

use super::leg::Prompt;

/// The rate prompts play at.
const RATE: u32 = 48_000;
/// The longest prompt audio played; longer is cut.
const MAX_AUDIO_SECONDS: usize = 60;
/// The silence between a notice's tones and its audio.
const GAP_MILLIS: u32 = 300;

/// What a phone leg is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notice {
    /// The call is recorded: one 1400 Hz tone.
    Recording,
    /// The call is recorded and each leg must acknowledge: the 1400 Hz tone twice, asking for 1.
    RecordingAck,
    /// Captions are on: two rising tones.
    Captions,
}

impl Notice {
    /// The notice of a `prompt` control's `notice`; none or another is the plain recording tone.
    pub(crate) fn from_code(code: &str) -> Notice {
        match code {
            "recordingAck" => Notice::RecordingAck,
            "captions" => Notice::Captions,
            _ => Notice::Recording,
        }
    }

    /// Its tone pattern: (Hz, milliseconds), 0 Hz a silence.
    pub(crate) fn tones(self) -> &'static [(f64, u32)] {
        match self {
            Notice::Recording => &[(1400.0, 400)],
            Notice::RecordingAck => &[(1400.0, 400), (0.0, 200), (1400.0, 400)],
            Notice::Captions => &[(900.0, 250), (0.0, 150), (1200.0, 250)],
        }
    }

    /// The property that names its recorded audio.
    pub(crate) fn audio_property(self) -> &'static str {
        match self {
            Notice::Recording | Notice::RecordingAck => "call_recording_prompt_audio",
            Notice::Captions => "call_captions_prompt_audio",
        }
    }
}

fn tone(hz: f64, millis: u32, out: &mut Vec<i16>) {
    let n = millis as usize * (RATE as usize / 1000);
    if hz <= 0.0 {
        out.resize(out.len() + n, 0);
        return;
    }
    out.extend((0..n).map(|i| (6000.0 * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(RATE)).sin()) as i16));
}

/// The notice's prompt: its tones, then `audio` (48 kHz mono) after a short silence.
pub(crate) fn notice_prompt(notice: Notice, audio: Option<&[i16]>) -> Prompt {
    let mut out = Vec::new();
    for (hz, millis) in notice.tones() {
        tone(*hz, *millis, &mut out);
    }
    if let Some(audio) = audio.filter(|a| !a.is_empty()) {
        tone(0.0, GAP_MILLIS, &mut out);
        out.extend_from_slice(audio);
    }
    Prompt(Arc::from(out))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A WAV file's audio as 48 kHz mono: 16-bit PCM, mono or stereo (mixed down), 8 to 48 kHz, at most a minute.
pub(crate) fn wav_pcm48(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV file".to_string());
    }
    let mut format = None;
    let mut data = None;
    let mut at = 12;
    while let (Some(id), Some(size)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let start = at + 8;
        let end = start.saturating_add(size as usize).min(bytes.len());
        let body = &bytes[start..end];
        match id {
            b"fmt " => format = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        at = end + (size as usize & 1);
    }
    let format = format.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    let (Some(tag), Some(channels), Some(rate), Some(bits)) = (
        u16_at(format, 0),
        u16_at(format, 2),
        u32_at(format, 4),
        u16_at(format, 14),
    ) else {
        return Err("a short fmt chunk".to_string());
    };
    // 1 is PCM; 0xFFFE (extensible) is taken when its other fields are 16-bit PCM
    if !(tag == 1 || tag == 0xFFFE) || bits != 16 {
        return Err(format!("not 16-bit PCM (format {tag}, {bits} bits)"));
    }
    if !(1..=2).contains(&channels) {
        return Err(format!("{channels} channels; mono or stereo only"));
    }
    if !(8_000..=48_000).contains(&rate) {
        return Err(format!("{rate} Hz; 8 to 48 kHz only"));
    }
    let frame = 2 * channels as usize;
    let mono: Vec<i16> = data
        .chunks_exact(frame)
        .take(MAX_AUDIO_SECONDS * rate as usize)
        .map(|f| {
            let left = i32::from(i16::from_le_bytes([f[0], f[1]]));
            if channels == 2 {
                ((left + i32::from(i16::from_le_bytes([f[2], f[3]]))) / 2) as i16
            } else {
                left as i16
            }
        })
        .collect();
    if rate == RATE {
        return Ok(mono);
    }
    let mut out = Vec::with_capacity(mono.len() * RATE as usize / rate as usize + 1);
    Resampler::new(rate, RATE).process(&mono, &mut out);
    Ok(out)
}
