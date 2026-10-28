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
