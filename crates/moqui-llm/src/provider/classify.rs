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

//! How a call ends and whether it is tried again (plan 4.2, from Java's `LlmRetryClassifier`, extended for streams).
//!
//! A call finishes [`Finish::Stop`], with tool calls, cut at the output limit, over the context window, filtered,
//! refused, empty, canceled or with an error. An error has a class: rate limited, overloaded, server, timeout, network,
//! auth, bad request or quota; the first five are tried again, up to [`MAX_ATTEMPTS`] in all, after a backoff with
//! jitter or the server's `Retry-After`.

use std::time::Duration;

use serde_json::Value as Json;

/// Attempts of one call, the first included.
pub const MAX_ATTEMPTS: u32 = 3;

/// A `Retry-After` longer than this is not waited for: the call fails instead.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// The longest backoff without a `Retry-After`.
const MAX_BACKOFF: Duration = Duration::from_secs(20);

/// How a model call ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Finish {
    Stop,
    ToolCalls,
    /// The output limit cut the reply.
    Length,
    /// The prompt is over the model's context window.
    ContextOverflow,
    ContentFilter,
    /// The model declined.
    Refusal,
    /// No text and no tool call.
    Empty,
    Error(ProviderError),
    Canceled,
}

impl Finish {
    pub fn as_str(&self) -> &'static str {
        match self {
            Finish::Stop => "stop",
            Finish::ToolCalls => "tool_calls",
            Finish::Length => "length",
            Finish::ContextOverflow => "context_overflow",
            Finish::ContentFilter => "content_filter",
            Finish::Refusal => "refusal",
            Finish::Empty => "empty",
            Finish::Error(_) => "error",
            Finish::Canceled => "canceled",
        }
    }

    pub fn error(&self) -> Option<&ProviderError> {
        match self {
            Finish::Error(error) => Some(error),
            _ => None,
        }
    }

    /// Whether another attempt may succeed.
    pub fn is_retryable(&self) -> bool {
        self.error().is_some_and(|error| error.class.is_retryable())
    }
}

/// The class of a failed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorClass {
    RateLimited,
    Overloaded,
    Server,
    Timeout,
    Network,
    Auth,
    BadRequest,
    Quota,
    /// Becomes [`Finish::ContextOverflow`].
    ContextOverflow,
    /// Becomes [`Finish::ContentFilter`].
    ContentFilter,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorClass::RateLimited => "rate_limited",
            ErrorClass::Overloaded => "overloaded",
            ErrorClass::Server => "server",
            ErrorClass::Timeout => "timeout",
            ErrorClass::Network => "network",
            ErrorClass::Auth => "auth",
            ErrorClass::BadRequest => "bad_request",
            ErrorClass::Quota => "quota",
            ErrorClass::ContextOverflow => "context_overflow",
            ErrorClass::ContentFilter => "content_filter",
        }
    }

    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            ErrorClass::RateLimited
                | ErrorClass::Overloaded
                | ErrorClass::Server
                | ErrorClass::Timeout
                | ErrorClass::Network
        )
    }
}

/// A failed call: its class, the HTTP status, the provider's code and message, and how long the server asked to wait.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderError {
    pub class: ErrorClass,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl ProviderError {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            status: None,
            code: None,
            message: message.into(),
            retry_after: None,
        }
    }

    /// The finish of a call that ended with this error.
    pub fn into_finish(self) -> Finish {
        match self.class {
            ErrorClass::ContextOverflow => Finish::ContextOverflow,
            ErrorClass::ContentFilter => Finish::ContentFilter,
            _ => Finish::Error(self),
        }
    }
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.class.as_str())?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        if let Some(code) = &self.code {
            write!(f, " {code}")?;
        }
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

/// Words of an error that mean the prompt is over the context window: OpenAI, vLLM, llama.cpp, LM Studio, Anthropic,
/// Gemini and DeepSeek wordings.
const OVERFLOW_WORDS: [&str; 12] = [
    "context length",
    "context_length_exceeded",
    "maximum context",
    "too many tokens",
    "prompt is too long",
    "input token count",
    "exceeds the maximum",
    "exceeds the context",
    "context window",
    "context size",
    "exceed_context_size",
    "model_context_window_exceeded",
];

fn any_word(haystack: &str, words: &[&str]) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    words.iter().any(|word| haystack.contains(word))
}

/// The error object of a body, wherever the server put it: `{"error": {...}}` (OpenAI, Anthropic, vLLM),
/// `{"error": "text"}` (Ollama), or the fields at the top (vLLM's `{"object":"error","message":...}`).
pub fn error_fields(body: &Json) -> Option<(Option<String>, Option<String>, String)> {
    let text = |value: Option<&Json>| match value {
        Some(Json::String(text)) if !text.is_empty() => Some(text.clone()),
        Some(Json::Number(number)) => Some(number.to_string()),
        _ => None,
    };
    match body.get("error") {
        Some(Json::String(message)) => Some((None, None, message.clone())),
        Some(error @ Json::Object(_)) => Some((
            text(error.get("code")),
            text(error.get("type")).or_else(|| text(error.get("status"))),
            text(error.get("message")).unwrap_or_default(),
        )),
        _ if body.get("object").and_then(Json::as_str) == Some("error")
            || body.get("type").and_then(Json::as_str) == Some("error") =>
        {
            Some((
                text(body.get("code")),
                text(body.get("type")),
                text(body.get("message")).unwrap_or_default(),
            ))
        }
        _ => None,
    }
}

/// The class of an error from its HTTP status (none for one inside a stream) and the provider's code, type and message.
pub fn classify_error(status: Option<u16>, code: Option<&str>, kind: Option<&str>, message: &str) -> ErrorClass {
    let named = format!("{} {}", code.unwrap_or(""), kind.unwrap_or("")).to_ascii_lowercase();
    let all = format!("{named} {}", message.to_ascii_lowercase());
    let overflow = any_word(&all, &OVERFLOW_WORDS);
    let filtered = any_word(
        &all,
        &[
            "content_filter",
            "invalid_prompt",
            "content_policy",
            "content management policy",
        ],
    );
    if matches!(status, None | Some(400 | 413 | 422)) && overflow {
        return ErrorClass::ContextOverflow;
    }
    if matches!(status, None | Some(400)) && filtered {
        return ErrorClass::ContentFilter;
    }
    if status == Some(402) || any_word(&named, &["insufficient_quota", "billing", "quota_exceeded", "credit"]) {
        return ErrorClass::Quota;
    }
    if matches!(status, Some(401 | 403))
        || any_word(
            &named,
            &[
                "authentication",
                "permission",
                "invalid_api_key",
                "unauthorized",
                "forbidden",
            ],
        )
    {
        return ErrorClass::Auth;
    }
    if status == Some(429)
        || any_word(
            &all,
            &[
                "rate_limit",
                "rate-limit",
                "rate limit",
                "too many requests",
                "resource_exhausted",
            ],
        )
    {
        return ErrorClass::RateLimited;
    }
    if matches!(status, Some(503 | 529)) || any_word(&all, &["overloaded", "unavailable"]) {
        return ErrorClass::Overloaded;
    }
    if matches!(status, Some(408 | 504)) || any_word(&named, &["timeout", "deadline_exceeded"]) {
        return ErrorClass::Timeout;
    }
    match status {
        Some(409) | Some(500..=599) | None => ErrorClass::Server,
        Some(400..=499) => ErrorClass::BadRequest,
        // a 2xx with an error body, or an odd status
        _ => ErrorClass::Server,
    }
}

/// The error of an HTTP response that did not succeed (or of a 2xx body that is an error), with its `Retry-After`. A body
/// with a structured error is read by its code, type and message only, since the rest of a 400 may echo user text such
/// as "too many tokens"; a body without one is read whole.
pub fn classify_http(status: u16, headers: &[(String, String)], body: &str) -> ProviderError {
    let parsed = serde_json::from_str::<Json>(body).ok();
    let fields = parsed.as_ref().and_then(error_fields);
    let (code, kind, message) = match fields {
        Some((code, kind, message)) => (code, kind, message),
        None => {
            let text: String = body.trim().chars().take(500).collect();
            (None, None, text)
        }
    };
    let class = if status == 200 || (200..300).contains(&status) {
        classify_error(None, code.as_deref(), kind.as_deref(), &message)
    } else {
        classify_error(Some(status), code.as_deref(), kind.as_deref(), &message)
    };
    let message = if message.is_empty() {
        moqui_core::outbound::reason_phrase(status).to_string()
    } else {
        message
    };
    ProviderError {
        class,
        status: Some(status),
        code: code.or(kind),
        message,
        retry_after: retry_after(headers),
    }
}

/// The wait a response asks for: `retry-after-ms`, else `Retry-After` in seconds or as an HTTP date.
pub fn retry_after(headers: &[(String, String)]) -> Option<Duration> {
    let header = |name: &str| {
        headers
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
    };
    if let Some(ms) = header("retry-after-ms").and_then(|text| text.parse::<f64>().ok()) {
        return (ms.is_finite() && ms >= 0.0).then(|| Duration::from_millis(ms as u64));
    }
    let text = header("retry-after")?;
    if let Ok(seconds) = text.parse::<f64>() {
        return (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_millis((seconds * 1000.0) as u64));
    }
    let when = jiff::fmt::rfc2822::parse(&text).ok()?;
    let wait = when.timestamp().as_millisecond() - jiff::Timestamp::now().as_millisecond();
    Some(Duration::from_millis(u64::try_from(wait).unwrap_or(0)))
}

/// The wait before attempt `attempt + 1` after `error`, or `None` when it is not tried again: the error is not
/// retryable, the attempts are used up, or the server asks for a wait over [`MAX_RETRY_AFTER`]. Without a
/// `Retry-After` it is 1 s, then 2 s, each between half and all of that at random.
pub fn backoff(error: &ProviderError, attempt: u32) -> Option<Duration> {
    if !error.class.is_retryable() || attempt >= MAX_ATTEMPTS {
        return None;
    }
    if let Some(wait) = error.retry_after {
        if wait > MAX_RETRY_AFTER {
            return None;
        }
        return Some(wait + jitter(Duration::from_millis(250)));
    }
    let base = Duration::from_secs(1u64 << attempt.saturating_sub(1).min(5)).min(MAX_BACKOFF);
    Some(base / 2 + jitter(base / 2))
}

/// A random duration up to `most`.
fn jitter(most: Duration) -> Duration {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return most / 2;
    }
    let fraction = (u64::from_le_bytes(bytes) % 10_000) as f64 / 10_000.0;
    most.mul_f64(fraction)
}

/// A Chat Completions `finish_reason`.
pub fn chat_finish(reason: &str) -> Finish {
    match reason {
        "tool_calls" | "function_call" => Finish::ToolCalls,
        "length" | "max_tokens" => Finish::Length,
        "content_filter" => Finish::ContentFilter,
        // stop, eos (some local servers), and anything new
        _ => Finish::Stop,
    }
}

/// An Anthropic `stop_reason`.
pub fn anthropic_finish(reason: &str) -> Finish {
    match reason {
        "tool_use" => Finish::ToolCalls,
        "max_tokens" => Finish::Length,
        "refusal" => Finish::Refusal,
        "model_context_window_exceeded" => Finish::ContextOverflow,
        // end_turn, stop_sequence, pause_turn
        _ => Finish::Stop,
    }
}

/// The finish a call reports, checked against what it produced: a stop with complete tool calls is tool calls (local
/// servers say stop), and a stop or tool-calls finish with neither text nor a complete call is empty.
pub fn settle(finish: Finish, has_text: bool, complete_calls: usize) -> Finish {
    match finish {
        Finish::Stop | Finish::ToolCalls if complete_calls > 0 => Finish::ToolCalls,
        Finish::Stop | Finish::ToolCalls if has_text => Finish::Stop,
        Finish::Stop | Finish::ToolCalls => Finish::Empty,
        other => other,
    }
}

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;
