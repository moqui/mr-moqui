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

//! `org.moqui.call.SpeechServices` (plans/Comms-Framework.md §6.4, §10.1): speech to text for server code, through
//! [`crate::llm_hooks::transcribe`] (the model bound to the `transcribe` feature on an OpenAI-compatible endpoint).
//! Call captions, voicemail (V2) and recordings (W5) use it. Both services are internal (`allow-remote="false"`): the
//! audio location they read is the caller's to check. A transcription that does not work is an answer, not a failed
//! service: `reasonCode` (the [`crate::llm_hooks::SpeechError`] code) and `errorText`, so a job keeps its audio and
//! records the state.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::llm_hooks::{SpeechError, SpeechRequest};

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, transcribe_audio)?;
    rust_service!(map, get_speech_provider)?;
    Ok(())
}

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("speech to text needs an execution context"))
}

fn text(parameters: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    parameters
        .get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
}

fn refused(error: &SpeechError) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("reasonCode".to_string(), Value::string(error.code)),
        ("errorText".to_string(), Value::string(&error.message)),
    ])
}

/// `transcribe#Audio`: the audio at `audioLocation`, or in `audio` (bytes, or base64 text), as `text`.
fn transcribe_audio(
    parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    use base64::Engine;
    let factory = factory()?;
    let audio: Vec<u8> = match (text(parameters, "audioLocation"), parameters.get("audio")) {
        (Some(location), _) => match factory.resource().location_bytes(&location)? {
            Some(bytes) => bytes,
            None => return Ok(refused(&SpeechError::new("invalid", format!("no audio at {location}")))),
        },
        (None, Some(Value::Bytes(bytes))) => bytes.clone(),
        (None, Some(value)) if !is_empty(value) => {
            match base64::engine::general_purpose::STANDARD.decode(to_plain_string(value).trim()) {
                Ok(bytes) => bytes,
                Err(_) => return Ok(refused(&SpeechError::new("invalid", "audio is not base64"))),
            }
        }
        _ => {
            return Ok(refused(&SpeechError::new(
                "invalid",
                "audioLocation or audio is required",
            )))
        }
    };
    let media_type = text(parameters, "mediaType").unwrap_or_default();
    let language = text(parameters, "language");
    let prompt = text(parameters, "prompt");
    let user_id = text(parameters, "userId").or_else(|| crate::user::session::current_user_id().ok().flatten());
    let purpose = text(parameters, "purpose").unwrap_or_else(|| "transcribe#Audio".to_string());
    let timeout = parameters
        .get("timeoutSeconds")
        .map(to_plain_string)
        .and_then(|seconds| seconds.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(|seconds| std::time::Duration::from_secs(seconds.min(600)));
    let request = SpeechRequest {
        audio: &audio,
        media_type: &media_type,
        language: language.as_deref(),
        prompt: prompt.as_deref(),
        user_id: user_id.as_deref(),
        purpose: &purpose,
        timeout,
    };
    Ok(match crate::llm_hooks::transcribe(&factory, &request) {
        Ok(text) => BTreeMap::from([("text".to_string(), Value::string(text))]),
        Err(error) => refused(&error),
    })
}

/// `get#SpeechProvider`: whether speech to text works now, and what a consent notice names (`providerLabel`, and
/// `local` when the endpoint is on this deployment's own network).
fn get_speech_provider(
    _parameters: &BTreeMap<String, Value>,
    _facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = factory()?;
    Ok(match crate::llm_hooks::speech_provider(&factory) {
        Ok(provider) => BTreeMap::from([
            ("available".to_string(), Value::Bool(true)),
            ("providerLabel".to_string(), Value::string(provider.label)),
            ("local".to_string(), Value::Bool(provider.local)),
        ]),
        Err(error) => {
            let mut out = refused(&error);
            out.insert("available".to_string(), Value::Bool(false));
            out
        }
    })
}
