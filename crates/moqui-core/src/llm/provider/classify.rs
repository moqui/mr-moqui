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
        crate::outbound::reason_phrase(status).to_string()
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
mod tests {
    use super::*;

    fn class(status: u16, body: &str) -> ErrorClass {
        classify_http(status, &[], body).class
    }

    #[test]
    fn http_errors_by_status_and_body() {
        let table: &[(u16, &str, ErrorClass)] = &[
            (
                400,
                r#"{"error":{"code":"context_length_exceeded","message":"This model's maximum context length is 8192 tokens"}}"#,
                ErrorClass::ContextOverflow,
            ),
            (
                400,
                r#"{"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error"}}"#,
                ErrorClass::ContextOverflow,
            ),
            (
                400,
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 215000 tokens > 200000 maximum"}}"#,
                ErrorClass::ContextOverflow,
            ),
            (
                413,
                r#"{"type":"error","error":{"type":"request_too_large","message":"Request exceeds the maximum allowed number of bytes"}}"#,
                ErrorClass::ContextOverflow,
            ),
            (
                400,
                r#"{"object":"error","message":"This model's maximum context length is 32768 tokens. However, you requested 40000 tokens","type":"BadRequestError","code":400}"#,
                ErrorClass::ContextOverflow,
            ),
            // a body with no structured error is read whole
            (
                400,
                "bad request: too many tokens in the prompt",
                ErrorClass::ContextOverflow,
            ),
            (
                400,
                r#"{"error":{"code":"invalid_value","message":"temperature must be at most 2"}}"#,
                ErrorClass::BadRequest,
            ),
            (
                400,
                r#"{"error":{"code":"content_filter","message":"The response was filtered"}}"#,
                ErrorClass::ContentFilter,
            ),
            (
                400,
                r#"{"error":{"code":"invalid_prompt","message":"Invalid prompt: flagged"}}"#,
                ErrorClass::ContentFilter,
            ),
            (
                401,
                r#"{"error":{"code":"invalid_api_key","message":"Incorrect API key provided"}}"#,
                ErrorClass::Auth,
            ),
            (
                403,
                r#"{"type":"error","error":{"type":"permission_error","message":"no access"}}"#,
                ErrorClass::Auth,
            ),
            (
                402,
                r#"{"type":"error","error":{"type":"billing_error","message":"credit balance too low"}}"#,
                ErrorClass::Quota,
            ),
            (
                429,
                r#"{"error":{"code":"insufficient_quota","type":"insufficient_quota","message":"You exceeded your current quota"}}"#,
                ErrorClass::Quota,
            ),
            (
                429,
                r#"{"error":{"code":"rate_limit_exceeded","message":"Rate limit reached"}}"#,
                ErrorClass::RateLimited,
            ),
            (429, "Too Many Requests", ErrorClass::RateLimited),
            (
                529,
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                ErrorClass::Overloaded,
            ),
            (503, "", ErrorClass::Overloaded),
            (
                500,
                r#"{"type":"error","error":{"type":"api_error","message":"Internal server error"}}"#,
                ErrorClass::Server,
            ),
            (502, "<html>Bad Gateway</html>", ErrorClass::Server),
            (504, "", ErrorClass::Timeout),
            (408, "", ErrorClass::Timeout),
            (409, "", ErrorClass::Server),
            (404, r#"{"error":"model 'qwen9' not found"}"#, ErrorClass::BadRequest),
            (422, r#"{"detail":[{"msg":"field required"}]}"#, ErrorClass::BadRequest),
            // an error body with 200, as some servers send a rate limit
            (
                200,
                r#"{"error":{"message":"Rate limit exceeded, retry later"}}"#,
                ErrorClass::RateLimited,
            ),
            (200, r#"{"error":{"message":"something broke"}}"#, ErrorClass::Server),
        ];
        for (status, body, expected) in table {
            assert_eq!(class(*status, body), *expected, "{status} {body}");
        }
        let error = classify_http(404, &[], r#"{"error":"model 'qwen9' not found"}"#);
        assert_eq!(error.message, "model 'qwen9' not found");
        assert_eq!(classify_http(503, &[], "").message, "Service Unavailable");
        assert!(!ErrorClass::BadRequest.is_retryable() && ErrorClass::Network.is_retryable());
        assert_eq!(
            classify_http(
                400,
                &[],
                r#"{"error":{"code":"context_length_exceeded","message":"x"}}"#
            )
            .into_finish(),
            Finish::ContextOverflow
        );
    }

    #[test]
    fn stream_errors_and_finish_reasons() {
        assert_eq!(
            classify_error(None, None, Some("overloaded_error"), "Overloaded"),
            ErrorClass::Overloaded
        );
        assert_eq!(
            classify_error(None, Some("server_error"), None, "The server had an error"),
            ErrorClass::Server
        );
        assert_eq!(
            classify_error(None, Some("rate_limit_exceeded"), None, ""),
            ErrorClass::RateLimited
        );
        assert_eq!(
            classify_error(None, None, None, "context window exceeded"),
            ErrorClass::ContextOverflow
        );
        let chat: &[(&str, Finish)] = &[
            ("stop", Finish::Stop),
            ("eos", Finish::Stop),
            ("tool_calls", Finish::ToolCalls),
            ("function_call", Finish::ToolCalls),
            ("length", Finish::Length),
            ("content_filter", Finish::ContentFilter),
        ];
        for (reason, finish) in chat {
            assert_eq!(&chat_finish(reason), finish, "{reason}");
        }
        let anthropic: &[(&str, Finish)] = &[
            ("end_turn", Finish::Stop),
            ("stop_sequence", Finish::Stop),
            ("pause_turn", Finish::Stop),
            ("tool_use", Finish::ToolCalls),
            ("max_tokens", Finish::Length),
            ("refusal", Finish::Refusal),
            ("model_context_window_exceeded", Finish::ContextOverflow),
        ];
        for (reason, finish) in anthropic {
            assert_eq!(&anthropic_finish(reason), finish, "{reason}");
        }
        assert_eq!(settle(Finish::Stop, false, 2), Finish::ToolCalls);
        assert_eq!(settle(Finish::ToolCalls, true, 0), Finish::Stop);
        assert_eq!(settle(Finish::Stop, false, 0), Finish::Empty);
        assert_eq!(settle(Finish::Length, false, 0), Finish::Length);
        assert_eq!(settle(Finish::Refusal, true, 0), Finish::Refusal);
    }

    #[test]
    fn backoff_honors_retry_after_and_stops_after_three_attempts() {
        let headers = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect()
        };
        assert_eq!(
            retry_after(&headers(&[("Retry-After", "7")])),
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            retry_after(&headers(&[("retry-after-ms", "1500"), ("retry-after", "9")])),
            Some(Duration::from_millis(1500))
        );
        let later = jiff::Timestamp::now() + jiff::SignedDuration::from_secs(30);
        let date = jiff::fmt::rfc2822::to_string(&later.to_zoned(jiff::tz::TimeZone::UTC)).unwrap();
        let wait = retry_after(&headers(&[("Retry-After", &date)])).unwrap();
        assert!(
            wait > Duration::from_secs(25) && wait <= Duration::from_secs(30),
            "{date} {wait:?}"
        );
        assert_eq!(retry_after(&headers(&[("Retry-After", "soon")])), None);

        let mut limited = classify_http(429, &headers(&[("retry-after", "2")]), "");
        let wait = backoff(&limited, 1).unwrap();
        assert!(
            wait >= Duration::from_secs(2) && wait <= Duration::from_millis(2250),
            "{wait:?}"
        );
        assert_eq!(backoff(&limited, MAX_ATTEMPTS), None);
        limited.retry_after = Some(Duration::from_secs(600));
        assert_eq!(backoff(&limited, 1), None);
        let server = ProviderError::new(ErrorClass::Server, "boom");
        for (attempt, base) in [(1, 1000), (2, 2000)] {
            let wait = backoff(&server, attempt).unwrap();
            assert!(
                wait >= Duration::from_millis(base / 2) && wait <= Duration::from_millis(base),
                "{attempt} {wait:?}"
            );
        }
        assert_eq!(backoff(&ProviderError::new(ErrorClass::Auth, "no"), 1), None);
    }
}
