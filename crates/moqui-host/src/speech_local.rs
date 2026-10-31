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

//! Local speech to text (`speech-local`, plans/Comms-Framework.md §6.4, §10.1): Whisper on candle, with the audio never
//! leaving the server. Not built yet: the feature is off in every package, and with or without it this stub answers
//! `notBuiltIn`, so a deployment that picks it (`call_captions_provider=local`) sees captions unavailable with that
//! reason. Until it is built, speech to text goes through the `moqui-llm` endpoint
//! ([`moqui_core::llm_hooks::transcribe`]); an endpoint on the deployment's own network (marked local) keeps the audio
//! in house the same way.

use moqui_core::llm_hooks::SpeechError;

/// 16 kHz mono WAV to text.
pub(crate) fn transcribe(_wav: &[u8], _language: Option<&str>) -> Result<String, SpeechError> {
    Err(not_built_in())
}

fn not_built_in() -> SpeechError {
    SpeechError::new(
        "notBuiltIn",
        if cfg!(feature = "speech-local") {
            "local speech to text (speech-local) has no engine yet"
        } else {
            "local speech to text (speech-local) is not built in"
        },
    )
}
