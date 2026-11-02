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

//! The HTTP fixtures, for specs (no Java counterpart; docs/Build-Test.md, "HTTP fixtures"). While a spec has
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
//! The tool: `isEnabled()`, `enable()`, `disable()`, `reset()` (off, no fixtures, no requests), `add({method, url,
//! bodyContains, status, body, headers, contentType, error, times})` (`url` exact, or a prefix when it ends with `*`;
//! `times` 0 or absent is any number), `clear()` (the fixtures and requests), `requests()` (oldest first, each
//! `{method, url, headers, body, fixture, at}`, `fixture` the index of the fixture that answered or -1), `count()` and
//! `last()`.

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
    pub headers: Vec<(String, String)>,
    /// A transport error instead of an answer: the request went out and its answer was lost.
    pub error: Option<String>,
    /// How many requests it answers; 0 is any number.
    pub times: usize,
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
        Ok(Response {
            status: if fixture.status == 0 { 200 } else { fixture.status },
            bytes: fixture.body.clone().into_bytes(),
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

/// A header whose value is a credential, kept out of the recorded requests.
fn credential_header(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["authorization", "proxy-authorization", "cookie"].contains(&name.as_str()) || name.ends_with("api-key")
}

/// Whether the fixtures are on.
pub fn intercept_enabled() -> bool {
    FIXTURES.is_enabled()
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
    Ok(Fixture {
        method: text("method").to_ascii_uppercase(),
        url,
        body_contains: text("bodyContains"),
        status,
        body: text("body"),
        headers,
        error: (!error.is_empty()).then_some(error),
        times: number("times").map_or(0, |times| times as usize),
        used: 0,
    })
}

fn recorded_value(recorded: &Recorded) -> Value {
    let mut out = IndexMap::new();
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
             add(fixture: { method?: string; url: string; bodyContains?: string; status?: number; body?: string; \
             headers?: Record<string, string>; contentType?: string; error?: string; times?: number }): void; \
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
                fixtures.set_enabled(false)?;
                fixtures.clear();
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
