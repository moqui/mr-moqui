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

//! Speech to text (plans/archive/Comms-Framework.md §6.4, §10.1): one audio file to text by the model bound to the `transcribe`
//! feature, on an OpenAI-compatible endpoint (`POST <endpoint>/v1/audio/transcriptions`, a multipart form). The
//! drawer's dictation ([`crate::assist`]) and the server's jobs (call captions, and later voicemail and recordings)
//! share [`transcribe_with`]; the jobs come through [`moqui_core::llm_hooks::transcribe`] to [`transcribe`], which
//! resolves the binding, checks the user's `api` quota (kept for [`QUOTA_TTL`], since a call's captions ask every few
//! seconds) and keeps each call as an `LlmCall` whose use is estimated (the endpoint reports no tokens).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm_hooks::{SpeechError, SpeechProvider, SpeechRequest};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use serde_json::Value as Json;

use crate::registry::Target;
use crate::run::{self, quietly};
use crate::usage::{CallRecord, CallSource};

/// The feature a transcription model is bound to.
pub const FEATURE: &str = "transcribe";
/// How long a provider may take when the caller sets no limit.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a user's quota answer is kept before it is checked again.
pub const QUOTA_TTL: Duration = Duration::from_secs(30);
/// The longest answer read.
const MAX_ANSWER: u64 = 1_000_000;

/// Whether `language` is a plain language tag (`en`, `pt-BR`); anything else is not sent.
#[must_use]
pub fn plain_language(language: &str) -> bool {
    !language.is_empty() && language.len() <= 8 && language.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

/// `request`'s media type, without parameters, when it is audio (or a WebM recording).
fn audio_type(media_type: &str) -> Option<String> {
    let media_type = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    (media_type.starts_with("audio/") || media_type == "video/webm").then_some(media_type)
}

/// The model bound to `transcribe`. Not configured when the facade is off or nothing is bound (a fallback model is a
/// chat model, not a speech one, so it is not used).
fn target(factory: &ExecutionContextFactory) -> Result<Target, SpeechError> {
    let llm = crate::facade(factory);
    if !llm.enabled() {
        return Err(SpeechError::new("notConfigured", "the llm-facade is disabled"));
    }
    let facade = run::facade_of(factory).map_err(|error| SpeechError::new("notConfigured", error.message()))?;
    let registry = quietly(|| llm.registry(&facade));
    if !registry.bindings.contains_key(FEATURE) {
        return Err(SpeechError::new(
            "notConfigured",
            "no model is bound to the transcribe feature",
        ));
    }
    registry
        .resolve_feature(FEATURE)
        .map_err(|error| SpeechError::new("notConfigured", error.message()))
}

/// The provider [`transcribe`] would use: the endpoint and model, and whether the endpoint is marked local.
///
/// # Errors
///
/// `notConfigured` as [`transcribe`].
pub fn provider(factory: &ExecutionContextFactory) -> Result<SpeechProvider, SpeechError> {
    let target = target(factory)?;
    Ok(SpeechProvider {
        label: format!("{} ({})", target.endpoint.id, target.model.name),
        local: target.endpoint.local,
    })
}

/// One transcription for a server job: the audio checked, the binding resolved, the user's `api` quota checked, the
/// call made with the caller's timeout and kept as an `LlmCall` (`source` api, `clientId` the request's purpose).
///
/// # Errors
///
/// A [`SpeechError`]: `invalid` audio, `notConfigured`, `quota`, `timeout` or `failed`.
pub fn transcribe(factory: &ExecutionContextFactory, request: &SpeechRequest<'_>) -> Result<String, SpeechError> {
    if request.audio.is_empty() {
        return Err(SpeechError::new("invalid", "there is no audio"));
    }
    let Some(media_type) = audio_type(request.media_type) else {
        return Err(SpeechError::new(
            "invalid",
            format!("{} is not audio", request.media_type),
        ));
    };
    let llm = crate::facade(factory);
    let max = llm.conf().assist.max_audio_bytes;
    if request.audio.len() as u64 > max {
        return Err(SpeechError::new("invalid", format!("the audio is over {max} bytes")));
    }
    let target = target(factory)?;
    let facade = run::facade_of(factory).map_err(|error| SpeechError::new("notConfigured", error.message()))?;
    if let Some(user_id) = request.user_id.filter(|id| !id.is_empty()) {
        if let Some(reason) = quota_exceeded(factory, user_id)? {
            return Err(SpeechError::new(
                "quota",
                format!("the audio was not transcribed: {reason}"),
            ));
        }
    }
    let request = SpeechRequest {
        media_type: &media_type,
        language: request.language.filter(|language| plain_language(language)),
        ..request.clone()
    };
    let timeout = request.timeout.unwrap_or(DEFAULT_TIMEOUT);
    let mut record = CallRecord::new(CallSource::Api, &target);
    record.user_id = request.user_id.map(str::to_string);
    record.client_id = Some(request.purpose.chars().take(60).collect()).filter(|purpose: &String| !purpose.is_empty());
    let started = Instant::now();
    let answered = transcribe_with(&target, &request);
    let elapsed = started.elapsed();
    record.duration_ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
    // the endpoint reports no tokens: estimated, the audio at about a token a kilobyte and the text at four characters
    record.usage_estimated = true;
    record.usage.input = u64::try_from(request.audio.len() / 1000).unwrap_or(0).max(1);
    let timed_out = answered.is_err() && elapsed + Duration::from_millis(50) >= timeout;
    match &answered {
        Ok(text) => {
            record.http_status = Some(200);
            record.finish = Some("stop".to_string());
            record.usage.output = u64::try_from(text.chars().count() / 4).unwrap_or(0);
        }
        Err(error) => {
            record.error_class = Some(if timed_out { "timeout" } else { "server" }.to_string());
            record.error_text = Some(error.message().chars().take(500).collect());
        }
    }
    if let Err(error) =
        quietly(|| crate::usage::record_call(&facade, &llm.conf().content_log, &target.model.prices, &record))
    {
        tracing::error!(
            "the transcription call for {} was not recorded: {error}",
            request.purpose
        );
    }
    answered.map_err(|error| {
        SpeechError::new(
            if timed_out { "timeout" } else { "failed" },
            format!("the audio was not transcribed: {}", error.message()),
        )
    })
}

/// Each user's last quota answer and when it was checked.
type QuotaAnswers = HashMap<String, (Instant, Option<String>)>;

/// Why `user_id` is over a quota now, as last checked within [`QUOTA_TTL`].
fn quota_exceeded(factory: &ExecutionContextFactory, user_id: &str) -> Result<Option<String>, SpeechError> {
    static CHECKED: OnceLock<Mutex<QuotaAnswers>> = OnceLock::new();
    let checked = CHECKED.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, answer)) = lock(checked).get(user_id) {
        if at.elapsed() < QUOTA_TTL {
            return Ok(answer.clone());
        }
    }
    let llm = crate::facade(factory);
    let facade = run::facade_of(factory).map_err(|error| SpeechError::new("notConfigured", error.message()))?;
    let status = quietly(|| crate::usage::check_quota(&facade, &llm.conf().limits, user_id, "api", run::now_millis()))
        .map_err(|error| SpeechError::new("failed", format!("the quota could not be read: {}", error.message())))?;
    let mut checked = lock(checked);
    checked.retain(|_, (at, _)| at.elapsed() < QUOTA_TTL);
    checked.insert(user_id.to_string(), (Instant::now(), status.exceeded.clone()));
    Ok(status.exceeded)
}

/// One transcription call to `target`: a multipart form with the model's name, the file, and the language and prompt
/// when given. The text of a JSON answer's `text`, or a plain answer whole.
///
/// # Errors
///
/// Returns an error when the address is refused, the endpoint cannot be reached or does not answer 200 in time.
pub fn transcribe_with(target: &Target, request: &SpeechRequest<'_>) -> Result<String, Error> {
    // the guard every model call has: a private address needs allow-private-network
    crate::provider::check_address(target).map_err(Error::new)?;
    let boundary = format!("moqui-{}", run::now_millis());
    let media_type = request.media_type;
    let extension = match media_type.rsplit('/').next().unwrap_or("webm") {
        "wave" | "x-wav" | "vnd.wave" => "wav",
        "mpeg" => "mp3",
        other => other,
    };
    let mut body: Vec<u8> = Vec::new();
    let mut field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    };
    field("model", &target.model.name);
    field("response_format", "json");
    if let Some(language) = request.language {
        field("language", language);
    }
    if let Some(prompt) = request.prompt.filter(|prompt| !prompt.trim().is_empty()) {
        field("prompt", &prompt.chars().take(1_000).collect::<String>());
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.{extension}\"\r\nContent-Type: {media_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(request.audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut headers = vec![(
        "Content-Type".to_string(),
        format!("multipart/form-data; boundary={boundary}"),
    )];
    if let Some(auth) = target.endpoint.auth() {
        headers.push(auth);
    }
    for (name, value) in &target.endpoint.headers {
        headers.retain(|(known, _)| !known.eq_ignore_ascii_case(name));
        headers.push((name.clone(), value.clone()));
    }
    let url = format!("{}/v1/audio/transcriptions", target.endpoint.url.trim_end_matches('/'));
    let mut response = moqui_core::outbound::open(moqui_core::outbound::OpenRequest {
        method: moqui_core::outbound::Method::Post,
        url,
        headers,
        body: Some(bytes::Bytes::from(body)),
        timeout: Some(request.timeout.unwrap_or(DEFAULT_TIMEOUT)),
        follow_redirects: false,
    })?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(&mut response.reader, MAX_ANSWER), &mut text)
        .map_err(|error| Error::new(format!("the answer could not be read: {error}")))?;
    if response.status != 200 {
        return Err(Error::new(format!("the endpoint answered HTTP {}", response.status)));
    }
    let answer: Json = serde_json::from_str(&text).unwrap_or(Json::String(text));
    Ok(match answer {
        Json::String(text) => text.trim().to_string(),
        other => other
            .get("text")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
    })
}

#[cfg(test)]
#[path = "speech_tests.rs"]
mod tests;
