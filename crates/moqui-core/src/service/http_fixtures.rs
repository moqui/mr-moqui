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

//! The HTTP fixtures, for specs (no Java counterpart; docs/dev/JS-Testkit.md, "HTTP fixtures"). While a spec has
//! `ec.factory.getTool('HttpFixtures')` enabled, every outbound HTTP request of the process, with or without an
//! execution context (`ec.service.rest()` `call()` and `send()`, `remote-rest` services, `http` locations), is answered by the first
//! fixture it matches, in the order they were added, and recorded; one that matches none fails as a transport error
//! naming it, so nothing reaches the network. A provider adapter is then exercised through its production code, with
//! its endpoint from its gateway configuration, against deterministic answers. A fixture with `error` fails as the
//! transport does when an answer is lost, after the request is recorded as sent (`plan/payments-shipping-edi.md` in
//! mr-mantle, Phase 0). Only a `moqui test` run may enable them ([`allow`], which the runner calls before boot); elsewhere
//! `enable()` is refused. The recorded headers have their credentials redacted (Authorization, Proxy-Authorization,
//! Cookie, and any `*Api-Key`).
//!
//! A test run is closed to the network also while the fixtures are off ([`network_refusal`]): a request to anything but
//! a loopback address (localhost, 127.0.0.0/8, ::1, `*.localhost`) fails as a transport error naming its redacted URL,
//! unless a spec opened its URL prefix with `allowNetwork([...])`. So a spec's leftover gateway, or a later spec's job
//! over its records, cannot reach a provider's sandbox with spec credentials.
//!
//! The tool: `isEnabled()`, `enable()`, `disable()`, `reset()` (off, no fixtures, no requests, no network opened: still
//! closed), `allowNetwork(urlPrefixes)` (a list; empty closes again), `add({method, url,
//! bodyContains, status, body, headers, contentType, error, times, ranges})` (`url` exact, or a prefix when it ends with
//! `*`; `body` text, or a `Uint8Array` for a binary answer such as a zip; `times` 0 or absent is any number; `ranges`
//! true answers a `Range` request with that part, 206, and an `If-Match` its `ETag` header fails with 412), `clear()`
//! (the fixtures and requests), `requests()` (oldest first, each `{method, url, headers, body, fixture, at}`,
//! `fixture` the index of the fixture that answered or -1), `count()` and `last()`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::context::{Error, ExecutionContext};
use crate::native::{NativeRegistry, Tool, ToolFactory};
use crate::outbound::{Request, Response};
use moqui_lib::error::Error as TransportError;

/// The class `<tool-factory>` names for the HTTP fixtures tool.
pub const HTTP_FIXTURES_CLASS: &str = "moqui_core::service::http_fixtures::HttpFixturesToolFactory";
/// The recorded requests a factory keeps; past it the oldest go.
pub const MAX_RECORDED: usize = 500;

/// One canned answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fixture {
    /// Upper case; empty matches any method.
    pub method: String,
    /// Exact, or a prefix when it ends with `*`.
    pub url: String,
    /// The request body must contain it; empty matches any body.
    pub body_contains: String,
    pub status: u16,
    pub body: String,
    /// A binary body (a `Uint8Array` given as `body`), answered in place of `body`'s text when not empty.
    pub body_bytes: Vec<u8>,
    pub headers: Vec<(String, String)>,
    /// A transport error instead of an answer: the request went out and its answer was lost.
    pub error: Option<String>,
    /// How many requests it answers; 0 is any number.
    pub times: usize,
    /// Answer as a file server does: a `Range` request with the part of the body it asks for (206 and `Content-Range`),
    /// a range past the end 416, an `If-Match` or `If-Unmodified-Since` that the fixture's `ETag` or `Last-Modified` header
    /// does not meet 412; a request with no usable `Range` gets the whole body.
    pub ranges: bool,
    used: usize,
}

impl Fixture {
    fn matches(&self, request: &Request) -> bool {
        let url_matches = match self.url.strip_suffix('*') {
            Some(prefix) => request.url.starts_with(prefix),
            None => request.url == self.url,
        };
        url_matches
            && (self.method.is_empty() || self.method == request.method.as_str())
            && (self.body_contains.is_empty()
                || request
                    .body
                    .as_deref()
                    .is_some_and(|body| body.contains(&self.body_contains)))
            && (self.times == 0 || self.used < self.times)
    }
}

/// A request a fixture answered or refused.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// The index of the fixture that answered.
    pub fixture: Option<usize>,
    pub at: i64,
}

/// Whether this process is a `moqui test` run, the only one that may enable the fixtures.
static ALLOWED: AtomicBool = AtomicBool::new(false);
/// The process's fixtures. Process-wide, so a request sent with no execution context is answered (or refused) as well:
/// while they are on, nothing goes to the network.
static FIXTURES: LazyLock<HttpFixtures> = LazyLock::new(HttpFixtures::default);

/// Let this process enable the fixtures: the `moqui test` runner calls it before it boots.
pub fn allow() {
    ALLOWED.store(true, Ordering::SeqCst);
}

/// Whether this process may enable the fixtures.
pub fn allowed() -> bool {
    ALLOWED.load(Ordering::SeqCst)
}

/// The process's fixtures.
pub fn fixtures() -> &'static HttpFixtures {
    &FIXTURES
}

/// Fixtures: whether they are on, the fixtures and the requests.
#[derive(Default)]
pub struct HttpFixtures {
    enabled: AtomicBool,
    fixtures: Mutex<Vec<Fixture>>,
    requests: Mutex<Vec<Recorded>>,
    /// URL prefixes a spec opened to the network while the fixtures are off, in a test run.
    network: Mutex<Vec<String>>,
}

impl HttpFixtures {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Turn them on, only in a `moqui test` run ([`allow`]), or off.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), Error> {
        if enabled && !allowed() {
            return Err(Error::new(
                "HttpFixtures can be enabled only in a moqui test run: they answer every outbound HTTP request",
            ));
        }
        self.enabled.store(enabled, Ordering::SeqCst);
        Ok(())
    }

    pub fn add(&self, fixture: Fixture) {
        lock(&self.fixtures).push(fixture);
    }

    pub fn clear(&self) {
        lock(&self.fixtures).clear();
        lock(&self.requests).clear();
    }

    pub fn requests(&self) -> Vec<Recorded> {
        lock(&self.requests).clone()
    }

    /// Open these URL prefixes to the network in a test run, replacing the ones opened before; empty closes it again.
    pub fn allow_network(&self, prefixes: Vec<String>) {
        *lock(&self.network) = prefixes.into_iter().filter(|prefix| !prefix.is_empty()).collect();
    }

    /// Whether a spec opened `url` to the network.
    fn network_open(&self, url: &str) -> bool {
        lock(&self.network).iter().any(|prefix| url.starts_with(prefix.as_str()))
    }

    /// Off, with no fixtures, requests or opened prefixes: in a test run the network stays closed.
    pub fn reset(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        self.clear();
        lock(&self.network).clear();
    }

    /// The answer to `request`: the first fixture it matches, or an error naming it.
    pub fn answer(&self, request: &Request) -> Result<Response, TransportError> {
        let found = {
            let mut fixtures = lock(&self.fixtures);
            fixtures
                .iter_mut()
                .enumerate()
                .find(|(_, fixture)| fixture.matches(request))
                .map(|(index, fixture)| {
                    fixture.used += 1;
                    (index, fixture.clone())
                })
        };
        self.record(request, found.as_ref().map(|(index, _)| *index));
        let Some((_, fixture)) = found else {
            return Err(TransportError::new(format!(
                "no HTTP fixture for {} {}",
                request.method,
                crate::outbound::redacted(&request.url)
            )));
        };
        if let Some(error) = fixture.error {
            return Err(TransportError::new(error));
        }
        let bytes = if fixture.body_bytes.is_empty() {
            fixture.body.clone().into_bytes()
        } else {
            fixture.body_bytes
        };
        let range = request
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("range"))
            .map(|(_, value)| value.as_str());
        if fixture.ranges {
            let status = if fixture.status == 0 { 200 } else { fixture.status };
            return Ok(ranged(&bytes, range, &request.headers, status, fixture.headers));
        }
        Ok(Response {
            status: if fixture.status == 0 { 200 } else { fixture.status },
            bytes,
            body: fixture.body,
            headers: fixture.headers,
        })
    }

    fn record(&self, request: &Request, fixture: Option<usize>) {
        let mut requests = lock(&self.requests);
        if requests.len() >= MAX_RECORDED {
            let over = requests.len() + 1 - MAX_RECORDED;
            requests.drain(..over);
        }
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
        requests.push(Recorded {
            method: request.method.as_str().to_string(),
            url: request.url.clone(),
            headers: request
                .headers
                .iter()
                .map(|(name, value)| {
                    let value = if credential_header(name) {
                        "[redacted]".to_string()
                    } else {
                        value.clone()
                    };
                    (name.clone(), value)
                })
                .collect(),
            body: request.body.clone(),
            fixture,
            at,
        });
    }
}

/// A ranged fixture's answer, as a file server's: `If-Match` (against its `ETag`) or `If-Unmodified-Since` (against its
/// `Last-Modified`) that does not hold is 412; the part of `bytes` a single `Range: bytes=...` asks for (`a-b`, `a-`,
/// `-n`) is 206 with `Content-Range`, and one that starts past the end 416; no range, one that does not parse, several,
/// or one whose end is before its start (RFC 9110: ignored) gets the whole body with the fixture's `status`.
fn ranged(
    bytes: &[u8],
    range: Option<&str>,
    request_headers: &[(String, String)],
    status: u16,
    mut headers: Vec<(String, String)>,
) -> Response {
    let header = |list: &[(String, String)], name: &str| {
        list.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
    };
    let precondition_failed = [("If-Match", "ETag"), ("If-Unmodified-Since", "Last-Modified")]
        .iter()
        .any(|(condition, validator)| match header(request_headers, condition) {
            Some(wanted) => wanted != "*" && header(&headers, validator).as_deref() != Some(wanted.as_str()),
            None => false,
        });
    if precondition_failed {
        return Response {
            status: 412,
            bytes: Vec::new(),
            body: String::new(),
            headers,
        };
    }
    let whole = |headers: Vec<(String, String)>| Response {
        status,
        bytes: bytes.to_vec(),
        body: String::new(),
        headers,
    };
    let total = bytes.len() as u64;
    let parsed = range
        .map(str::trim)
        .and_then(|range| range.strip_prefix("bytes="))
        .filter(|spec| !spec.contains(','))
        .and_then(|spec| spec.split_once('-'))
        .and_then(|(start, end)| match (start.trim(), end.trim()) {
            ("", suffix) => suffix
                .parse::<u64>()
                .ok()
                .filter(|n| *n > 0)
                .map(|n| (total.saturating_sub(n), total.saturating_sub(1))),
            (start, "") => start.parse::<u64>().ok().map(|a| (a, total.saturating_sub(1))),
            (start, end) => {
                let (start, end) = (start.parse::<u64>().ok()?, end.parse::<u64>().ok()?);
                (start <= end).then_some((start, end.min(total.saturating_sub(1))))
            }
        });
    let Some((start, end)) = parsed else {
        return whole(headers);
    };
    if start >= total {
        headers.push(("Content-Range".to_string(), format!("bytes */{total}")));
        return Response {
            status: 416,
            bytes: Vec::new(),
            body: String::new(),
            headers,
        };
    }
    let part = bytes
        .get(usize::try_from(start).unwrap_or(usize::MAX)..=usize::try_from(end).unwrap_or(usize::MAX))
        .unwrap_or_default()
        .to_vec();
    headers.push(("Content-Range".to_string(), format!("bytes {start}-{end}/{total}")));
    Response {
        status: 206,
        bytes: part,
        body: String::new(),
        headers,
    }
}

/// A header whose value is a credential, kept out of the recorded requests.
fn credential_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["authorization", "proxy-authorization", "cookie"].contains(&name.as_str()) || name.ends_with("api-key")
}

/// Whether the fixtures are on.
pub fn intercept_enabled() -> bool {
    FIXTURES.is_enabled()
}

/// Whether `url`'s host is a loopback address: localhost or a `.localhost` name, 127.0.0.0/8, or ::1.
pub fn is_loopback(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => false,
    }
}

/// The refusal of `url` in a closed test run: `closed` is a test run with the fixtures off, `open` whether a spec opened
/// it. A loopback address is never refused (a spec's own servers and the test server).
pub fn refusal_of(url: &str, closed: bool, open: bool) -> Option<TransportError> {
    if !closed || open || is_loopback(url) {
        return None;
    }
    let shown = crate::outbound::redacted(url);
    tracing::warn!("Refused outbound HTTP to {shown}: a moqui test run sends nothing to the network while no HTTP fixtures are on");
    Some(TransportError::new(format!(
        "outbound HTTP to {shown} refused: a moqui test run sends nothing to the network (HttpFixtures, docs/Build-Test.md)"
    )))
}

/// The transport's check before it sends anything: in a `moqui test` run ([`allowed`]) with the fixtures off, a request
/// to a non-loopback address a spec did not open fails. `None` sends (or lets the fixtures answer).
pub fn network_refusal(url: &str) -> Option<TransportError> {
    if !allowed() {
        return None;
    }
    refusal_of(url, !FIXTURES.is_enabled(), FIXTURES.network_open(url))
}

/// The fixtures' answer, when they are on: `None` sends as usual.
pub fn intercept(request: &Request) -> Option<Result<Response, TransportError>> {
    FIXTURES.is_enabled().then(|| FIXTURES.answer(request))
}

fn fixture_of(value: &Value) -> Result<Fixture, Error> {
    let Value::Map(map) = value else {
        return Err(Error::new(
            "HttpFixtures.add takes an object: {method, url, status, body, ...}",
        ));
    };
    let text = |name: &str| match map.get(name) {
        None | Some(Value::Null) => String::new(),
        Some(value) => to_plain_string(value),
    };
    let url = text("url");
    if url.is_empty() {
        return Err(Error::new("HttpFixtures.add needs a url"));
    }
    let number = |name: &str| text(name).trim().parse::<f64>().ok().filter(|value| *value >= 0.0);
    let status = match number("status") {
        None => 200,
        Some(status) if (100.0..1000.0).contains(&status) => status as u16,
        Some(status) => {
            return Err(Error::new(format!(
                "HttpFixtures.add status {status} is not an HTTP status"
            )))
        }
    };
    let mut headers = Vec::new();
    if let Some(Value::Map(pairs)) = map.get("headers") {
        for (name, value) in pairs {
            headers.push((name.clone(), to_plain_string(value)));
        }
    }
    let content_type = text("contentType");
    if !content_type.is_empty()
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("Content-Type".to_string(), content_type));
    }
    let error = text("error");
    let (body, body_bytes) = match map.get("body") {
        Some(Value::Bytes(bytes)) => (String::new(), bytes.clone()),
        _ => (text("body"), Vec::new()),
    };
    Ok(Fixture {
        method: text("method").to_ascii_uppercase(),
        url,
        body_contains: text("bodyContains"),
        status,
        body,
        body_bytes,
        headers,
        error: (!error.is_empty()).then_some(error),
        times: number("times").map_or(0, |times| times as usize),
        ranges: matches!(map.get("ranges"), Some(Value::Bool(true))),
        used: 0,
    })
}

fn recorded_value(recorded: &Recorded) -> Value {
    let mut out = IndexMap::default();
    out.insert("method".to_string(), Value::string(&recorded.method));
    out.insert("url".to_string(), Value::string(&recorded.url));
    out.insert(
        "headers".to_string(),
        Value::Map(
            recorded
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), Value::string(value)))
                .collect(),
        ),
    );
    out.insert(
        "body".to_string(),
        recorded.body.as_ref().map_or(Value::Null, Value::string),
    );
    out.insert(
        "fixture".to_string(),
        Value::I64(
            recorded
                .fixture
                .map_or(-1, |index| i64::try_from(index).unwrap_or(i64::MAX)),
        ),
    );
    out.insert("at".to_string(), Value::I64(recorded.at));
    Value::Map(out)
}

/// The framework's HTTP fixtures tool factory, registered with the mail capture.
pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.tool(HTTP_FIXTURES_CLASS, || Arc::new(HttpFixturesToolFactory))
}

/// `ec.factory.getTool('HttpFixtures')`.
pub struct HttpFixturesToolFactory;

impl ToolFactory for HttpFixturesToolFactory {
    fn name(&self) -> &str {
        "HttpFixtures"
    }

    fn get_instance(&self, _parameters: &[Value]) -> Result<Arc<dyn Tool>, Error> {
        Ok(Arc::new(HttpFixturesTool))
    }

    fn type_declaration(&self) -> Option<&'static str> {
        Some(
            "{ isEnabled(): boolean; enable(): void; disable(): void; reset(): void; \
             allowNetwork(urlPrefixes: string[]): void; \
             add(fixture: { method?: string; url: string; bodyContains?: string; status?: number; body?: string | Uint8Array; \
             headers?: Record<string, string>; contentType?: string; error?: string; times?: number; ranges?: boolean }): void; \
             clear(): void; requests(): { method: string; url: string; headers: Record<string, string>; \
             body: string | null; fixture: number; at: number }[]; count(): number; \
             last(): Record<string, unknown> | null }",
        )
    }
}

pub struct HttpFixturesTool;

impl Tool for HttpFixturesTool {
    fn methods(&self) -> &'static [&'static str] {
        &[
            "isEnabled",
            "enable",
            "disable",
            "reset",
            "allowNetwork",
            "add",
            "clear",
            "requests",
            "count",
            "last",
        ]
    }

    fn call(&self, _ec: &ExecutionContext, method: &str, args: &[Value]) -> Result<Value, Error> {
        let fixtures = fixtures();
        match method {
            "isEnabled" => Ok(Value::Bool(fixtures.is_enabled())),
            "enable" => {
                fixtures.set_enabled(true)?;
                Ok(Value::Null)
            }
            "disable" => {
                fixtures.set_enabled(false)?;
                Ok(Value::Null)
            }
            "reset" => {
                fixtures.reset();
                Ok(Value::Null)
            }
            "allowNetwork" => {
                let prefixes = match args.first() {
                    Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
                    None | Some(Value::Null) => Vec::new(),
                    Some(other) => vec![to_plain_string(other)],
                };
                fixtures.allow_network(prefixes);
                Ok(Value::Null)
            }
            "add" => {
                fixtures.add(fixture_of(args.first().unwrap_or(&Value::Null))?);
                Ok(Value::Null)
            }
            "clear" => {
                fixtures.clear();
                Ok(Value::Null)
            }
            "requests" => Ok(Value::List(fixtures.requests().iter().map(recorded_value).collect())),
            "count" => Ok(Value::I64(i64::try_from(fixtures.requests().len()).unwrap_or(i64::MAX))),
            "last" => Ok(fixtures.requests().last().map_or(Value::Null, recorded_value)),
            other => Err(Error::new(format!(
                "HttpFixtures has no method {other} ({} arguments)",
                args.len()
            ))),
        }
    }
}

#[cfg(test)]
#[path = "http_fixtures_tests.rs"]
mod tests;
