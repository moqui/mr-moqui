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

//! Blocking HTTP. No `ExecutionContext`.
//!
//! [`sim_allows_http`] is a pure function. The caller passes `sim`.
//! `allow_in_sim` defaults to false, so a sim call is denied. There is no
//! provider allow-list. A denied call returns before a [`PreparedRequest`]
//! exists. The first [`Call::send`] builds a `reqwest::blocking` client,
//! which runs its own runtime on its own thread.
//!
//! Call it from a framework worker or a `spawn_blocking` thread, never from an
//! async task. On a Tokio worker a debug build panics (reqwest's check) and a
//! release build blocks the worker for the whole request. There is no guard
//! here, because Tokio does not say whether blocking is allowed on a thread and
//! a check for "inside a runtime" would also refuse `spawn_blocking`.
//!
//! TLS certificates and host names are verified. Java `RestClient` trusts any
//! certificate; a remote with a self-signed certificate needs it trusted at the OS.

use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};
use serde::Serialize;

use crate::error::Error;
use crate::sync::lock;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest response body read by default. A larger one is an error, not a truncated body, unless
/// [`RestClient::truncate_body`].
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

/// Whether an outbound call is allowed.
///
/// `sim` is supplied by the caller. This does not look up a Moqui context.
/// The call is allowed when `sim` is false. When `sim` is true it is denied
/// unless `allow_in_sim` is true.
fn sim_allows_http(sim: bool, allow_in_sim: bool) -> bool {
    !sim || allow_in_sim
}

/// HTTP method this client sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    /// The method for a name such as `put`, ignoring case. `None` when it is not one this client sends.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            "HEAD" => Some(Self::Head),
            "OPTIONS" => Some(Self::Options),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }

    fn as_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
            Self::Head => reqwest::Method::HEAD,
            Self::Options => reqwest::Method::OPTIONS,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Method, URL, headers, and body assembled before any socket is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    timeout: Duration,
}

impl PreparedRequest {
    pub fn method(&self) -> Method {
        self.method
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

/// Status code and response body text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestResponse {
    status: u16,
    body: String,
    bytes: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl RestResponse {
    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn body(&self) -> &str {
        &self.body
    }

    /// The body as received, for a body that is not text.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The response headers, names as the server sent them.
    pub fn response_headers(&self) -> &[(String, String)] {
        &self.headers
    }
}

/// A request for [`RestClient::open`]: the body as bytes, sent as they are with the headers given.
#[derive(Debug, Clone)]
pub struct OpenRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<bytes::Bytes>,
    /// The whole exchange's limit, the body's read included; `None` is the client's.
    pub timeout: Option<Duration>,
}

/// The status and headers of a response whose body is still to be read.
pub struct OpenResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// The `Content-Length`, when the server sent one and the body is not compressed.
    pub length: Option<u64>,
    pub reader: Box<dyn std::io::Read + Send>,
}

impl std::fmt::Debug for OpenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

/// Reusable blocking client. The reqwest runtime starts on the first send.
#[derive(Debug)]
pub struct RestClient {
    timeout: Duration,
    deny_internal: bool,
    max_response_bytes: u64,
    truncate_body: bool,
    max_redirects: usize,
    redirect_any_host: bool,
    connect_timeout: Duration,
    transport: OnceLock<reqwest::blocking::Client>,
}

impl Default for RestClient {
    fn default() -> Self {
        Self::new()
    }
}

impl RestClient {
    pub const fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            deny_internal: false,
            max_response_bytes: MAX_RESPONSE_BYTES,
            truncate_body: false,
            max_redirects: MAX_REDIRECTS,
            redirect_any_host: false,
            connect_timeout: CONNECT_TIMEOUT,
            transport: OnceLock::new(),
        }
    }

    /// How long a connection may take to open, 10 seconds by default.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        // The timeout is part of the transport, so one built before this is not kept.
        self.transport = OnceLock::new();
        self
    }

    /// Refuse a host that resolves to a loopback, private, link-local, shared (CGNAT), or unspecified address,
    /// including the cloud metadata address and those addresses inside IPv6. Use it for URLs a user or a remote
    /// system supplied. The framework's own calls to a local server leave it off.
    ///
    /// The check is in the client's resolver, so the addresses checked are the ones connected to: a name that
    /// resolves to a public address for the check and an internal one for the connect cannot get past it. A
    /// redirect is resolved the same way.
    #[must_use]
    pub fn deny_internal_addresses(mut self, deny: bool) -> Self {
        self.deny_internal = deny;
        // The resolver is part of the transport, so one built before this is not kept.
        self.transport = OnceLock::new();
        self
    }

    /// The largest response body read, 16 MB by default. A larger body is an error, unless [`truncate_body`]
    /// keeps its first bytes.
    ///
    /// [`truncate_body`]: Self::truncate_body
    #[must_use]
    pub fn max_response_bytes(mut self, max: u64) -> Self {
        self.max_response_bytes = max;
        self
    }

    /// Keep the first [`max_response_bytes`](Self::max_response_bytes) of a larger body instead of failing: for a
    /// caller that reads only the start of a page, such as its head.
    #[must_use]
    pub fn truncate_body(mut self, truncate: bool) -> Self {
        self.truncate_body = truncate;
        self
    }

    /// Follow at most `max` redirects (5 by default), to the same host only, or with `any_host` to any host. Never
    /// from https to http. With [`deny_internal_addresses`](Self::deny_internal_addresses) each hop is checked: a
    /// name by the resolver, an internal address written out ([`internal_literal`]) here.
    #[must_use]
    pub fn redirects(mut self, max: usize, any_host: bool) -> Self {
        self.max_redirects = max;
        self.redirect_any_host = any_host;
        // The redirect policy is part of the transport, so one built before this is not kept.
        self.transport = OnceLock::new();
        self
    }

    /// Full-response timeout. The default is 30 seconds.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn get(&self, url: impl Into<String>) -> Call<'_> {
        self.call(Method::Get, url)
    }

    pub fn post(&self, url: impl Into<String>) -> Call<'_> {
        self.call(Method::Post, url)
    }

    pub fn put(&self, url: impl Into<String>) -> Call<'_> {
        self.call(Method::Put, url)
    }

    pub fn patch(&self, url: impl Into<String>) -> Call<'_> {
        self.call(Method::Patch, url)
    }

    pub fn delete(&self, url: impl Into<String>) -> Call<'_> {
        self.call(Method::Delete, url)
    }

    /// A call with a method chosen at run time.
    pub fn request(&self, method: Method, url: impl Into<String>) -> Call<'_> {
        self.call(method, url)
    }

    /// Send a request that already passed [`sim_allows_http`].
    pub fn execute(&self, request: PreparedRequest) -> Result<RestResponse, Error> {
        let client = self.transport()?;
        let method = request.method;
        let url = request.url;
        let shown = redacted(&url);
        if self.deny_internal {
            // An address in the URL is not resolved, so the resolver does not see it.
            deny_internal_literal(&url)?;
        }
        let mut builder = client.request(method.as_reqwest(), &url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder
            .timeout(request.timeout)
            .send()
            .map_err(|err| Error::new(format!("HTTP {method} {shown} failed")).with_source(err.without_url()))?;
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
            .collect();
        let max = self.max_response_bytes;
        if !self.truncate_body && response.content_length().is_some_and(|length| length > max) {
            return Err(Error::new(format!(
                "HTTP {method} {shown} response is larger than the limit"
            )));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(response, max.saturating_add(1)), &mut bytes)
            .map_err(|err| Error::new(format!("HTTP {method} {shown} response body failed")).with_source(err))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max {
            if !self.truncate_body {
                return Err(Error::new(format!(
                    "HTTP {method} {shown} response is larger than the limit"
                )));
            }
            bytes.truncate(usize::try_from(max).unwrap_or(usize::MAX));
        }
        let body = String::from_utf8_lossy(&bytes).into_owned();
        Ok(RestResponse {
            status,
            body,
            bytes,
            headers,
        })
    }

    /// Send a request with a body of bytes and return as soon as the response headers are in: the body is a reader the
    /// caller drains, for a proxy that passes a large body on without holding it. `timeout` covers the whole exchange,
    /// the body's read included. The response size limit does not apply; the caller bounds what it reads.
    ///
    /// # Errors
    ///
    /// Returns an error when the URL or a header is not valid or the request fails before the response headers.
    pub fn open(&self, request: OpenRequest) -> Result<OpenResponse, Error> {
        let parsed = reqwest::Url::parse(&request.url)
            .map_err(|err| Error::new(format!("invalid URL {}", redacted(&request.url))).with_source(err))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::new(format!(
                "URL {} must be http or https",
                redacted(&request.url)
            )));
        }
        for (name, value) in &request.headers {
            check_header(name, value)?;
        }
        if self.deny_internal {
            deny_internal_literal(&request.url)?;
        }
        let client = self.transport()?;
        let method = request.method;
        let shown = redacted(&request.url);
        let mut builder = client.request(method.as_reqwest(), &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = builder
            .timeout(request.timeout.unwrap_or(self.timeout))
            .send()
            .map_err(|err| Error::new(format!("HTTP {method} {shown} failed")).with_source(err.without_url()))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string())))
            .collect();
        Ok(OpenResponse {
            status,
            headers,
            length: response.content_length(),
            reader: Box::new(response),
        })
    }

    fn call(&self, method: Method, url: impl Into<String>) -> Call<'_> {
        Call {
            client: self,
            method,
            url: url.into(),
            headers: Vec::new(),
            body: PendingBody::Absent,
            timeout: self.timeout,
            allow_in_sim: false,
        }
    }

    fn transport(&self) -> Result<&reqwest::blocking::Client, Error> {
        if let Some(client) = self.transport.get() {
            return Ok(client);
        }
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .connect_timeout(self.connect_timeout)
            .redirect(redirect_policy(
                self.max_redirects,
                self.redirect_any_host,
                self.deny_internal,
            ));
        if self.deny_internal {
            builder = builder.dns_resolver(std::sync::Arc::new(PublicOnly));
        }
        let built = builder
            .build()
            .map_err(|err| Error::new("could not build the HTTP client").with_source(err))?;
        Ok(self.transport.get_or_init(|| built))
    }
}

#[derive(Debug)]
enum PendingBody {
    Absent,
    Json(String),
    Text(String),
    Invalid(Error),
}

/// One call on a [`RestClient`]. `allow_in_sim` defaults to false.
#[derive(Debug)]
#[must_use = "call prepare or send to use this HTTP call"]
pub struct Call<'a> {
    client: &'a RestClient,
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: PendingBody,
    timeout: Duration,
    allow_in_sim: bool,
}

impl Call<'_> {
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// JSON body. Sets `content-type: application/json` unless one is already set.
    pub fn json<T: Serialize + ?Sized>(mut self, value: &T) -> Self {
        self.body = match serde_json::to_string(value) {
            Ok(text) => PendingBody::Json(text),
            Err(err) => PendingBody::Invalid(Error::new("could not encode the JSON body").with_source(err)),
        };
        self
    }

    /// A text body sent as given, such as newline-delimited JSON. Sets `content-type` unless one is
    /// already set.
    pub fn text(mut self, content_type: &str, body: impl Into<String>) -> Self {
        if !has_content_type(&self.headers) {
            self.headers
                .push(("content-type".to_string(), content_type.to_string()));
        }
        self.body = PendingBody::Text(body.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Allow this call when `sim` is true. Defaults to false.
    pub fn allow_in_sim(mut self, allow_in_sim: bool) -> Self {
        self.allow_in_sim = allow_in_sim;
        self
    }

    /// Build the request when the sim predicate allows it.
    ///
    /// A denial returns before [`PreparedRequest`] is constructed.
    pub fn prepare(self, sim: bool) -> Result<PreparedRequest, Error> {
        if !sim_allows_http(sim, self.allow_in_sim) {
            return Err(Error::new(format!(
                "HTTP {} {} denied because sim mode is active",
                self.method,
                redacted(&self.url)
            )));
        }
        self.into_prepared()
    }

    /// [`prepare`](Self::prepare), then send with blocking reqwest.
    pub fn send(self, sim: bool) -> Result<RestResponse, Error> {
        let client = self.client;
        let request = self.prepare(sim)?;
        client.execute(request)
    }

    fn into_prepared(self) -> Result<PreparedRequest, Error> {
        let parsed = reqwest::Url::parse(&self.url)
            .map_err(|err| Error::new(format!("invalid URL {}", redacted(&self.url))).with_source(err))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::new(format!("URL {} must be http or https", redacted(&self.url))));
        }
        for (name, value) in &self.headers {
            check_header(name, value)?;
        }
        let mut headers = self.headers;
        let body = match self.body {
            PendingBody::Absent => None,
            PendingBody::Invalid(err) => return Err(err),
            PendingBody::Json(text) => {
                if !has_content_type(&headers) {
                    headers.push(("content-type".to_string(), "application/json".to_string()));
                }
                Some(text)
            }
            PendingBody::Text(text) => Some(text),
        };
        Ok(PreparedRequest {
            method: self.method,
            url: self.url,
            headers,
            body,
            timeout: self.timeout,
        })
    }
}

/// Follow up to `max` redirects over the same or a safer scheme, and only to the same host unless `any_host`. A
/// redirect anywhere else comes back as the 3xx response, so a header meant for one server never goes to another.
/// With `deny_internal` a hop to an internal address written out is an error; the resolver checks a name.
fn redirect_policy(max: usize, any_host: bool, deny_internal: bool) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= max {
            return attempt.stop();
        }
        let next = attempt.url();
        if deny_internal && internal_literal(next) {
            let shown = redacted(next.as_str());
            return attempt.error(format!("{shown} is an internal address and is not allowed here"));
        }
        let origin = attempt.previous().first().cloned();
        let same_host = any_host
            || origin.as_ref().is_some_and(|first| {
                first.host_str() == next.host_str() && first.port_or_known_default() == next.port_or_known_default()
            });
        let downgrade = origin
            .as_ref()
            .is_some_and(|first| first.scheme() == "https" && next.scheme() != "https");
        if same_host && !downgrade && matches!(next.scheme(), "http" | "https") {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// `scheme://host/path` with no user, password, or query, for messages and logs.
pub fn redacted(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(parsed) => {
            let host = parsed.host_str().unwrap_or("");
            let port = parsed.port().map(|port| format!(":{port}")).unwrap_or_default();
            format!("{}://{host}{port}{}", parsed.scheme(), parsed.path())
        }
        Err(_) => "(invalid URL)".to_string(),
    }
}

/// Refuse a URL whose host is an internal IP address written out. A name is left to [`PublicOnly`].
fn deny_internal_literal(url: &str) -> Result<(), Error> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|err| Error::new(format!("invalid URL {}", redacted(url))).with_source(err))?;
    if parsed.host_str().is_none() {
        return Err(Error::new(format!("URL {} has no host", redacted(url))));
    }
    if internal_literal(&parsed) {
        return Err(Error::new(format!(
            "{} is an internal address and is not allowed here",
            redacted(url)
        )));
    }
    Ok(())
}

/// The IP address written as the host of `url`, read as the URL parser reads it (`http://2852039166/` and
/// `http://0xa9.254.169.254/` are 169.254.169.254), or `None` for a name or a URL that does not parse. No lookup.
pub fn host_address(url: &str) -> Option<std::net::IpAddr> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse()
        .ok()
}

/// Whether the host of `url` is an internal IP address written out (a name is not: only a lookup tells). The URL
/// parser has already put an IPv4 host in dotted form, however it was written.
pub fn internal_literal(url: &reqwest::Url) -> bool {
    url.host_str()
        .and_then(|host| {
            host.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(internal_address)
}

/// The resolver of a client that denies internal addresses: the system lookup, failing when any address is
/// internal. The lookup runs on its own thread, as reqwest's own resolver does, so it does not hold up the client's
/// runtime.
struct PublicOnly;

impl reqwest::dns::Resolve for PublicOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(Lookup::start(host))
    }
}

type LookupResult = Result<reqwest::dns::Addrs, Box<dyn std::error::Error + Send + Sync>>;

/// One lookup on a thread of its own, woken when the thread is done.
struct Lookup {
    shared: std::sync::Arc<std::sync::Mutex<LookupState>>,
}

#[derive(Default)]
struct LookupState {
    result: Option<LookupResult>,
    waker: Option<std::task::Waker>,
}

impl Lookup {
    fn start(host: String) -> Self {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(LookupState::default()));
        let filled = std::sync::Arc::clone(&shared);
        let finish = move |result: LookupResult| {
            let mut state = lock(&filled);
            state.result = Some(result);
            if let Some(waker) = state.waker.take() {
                waker.wake();
            }
        };
        let spawned = std::thread::Builder::new().name("moqui-http-dns".to_string()).spawn({
            let finish = finish.clone();
            move || finish(resolve_public(&host))
        });
        if let Err(err) = spawned {
            finish(Err(Box::new(err)));
        }
        Self { shared }
    }
}

impl std::future::Future for Lookup {
    type Output = LookupResult;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut state = lock(&self.shared);
        match state.result.take() {
            Some(result) => std::task::Poll::Ready(result),
            None => {
                state.waker = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    }
}

/// The addresses of `host`, or an error when any is internal: one internal address among public ones could still be
/// the one connected to.
fn resolve_public(host: &str) -> LookupResult {
    use std::net::ToSocketAddrs;

    let addresses: Vec<std::net::SocketAddr> = (host, 0).to_socket_addrs()?.collect();
    if addresses.iter().any(|address| internal_address(address.ip())) {
        return Err(format!("{host} is an internal address and is not allowed here").into());
    }
    Ok(Box::new(addresses.into_iter()))
}

/// Whether `address` is loopback, private, link-local, shared (CGNAT), unspecified or otherwise not public, also
/// inside IPv6: what [`RestClient::deny_internal_addresses`] refuses.
pub fn internal_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                // "this network" 0.0.0.0/8, and shared address space (CGNAT) 100.64.0.0/10
                || octets[0] == 0
                || (octets[0] == 100 && (octets[1] & 0xc0) == 64)
                // reserved 240.0.0.0/4, the broadcast address among them
                || octets[0] >= 240
        }
        std::net::IpAddr::V6(v6) => {
            let segments = v6.segments();
            let embedded = || {
                let [.., a, b, c, d] = v6.octets();
                internal_address(std::net::IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)))
            };
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // unique local fc00::/7 and link local fe80::/10
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                // IPv4-mapped ::ffff:0:0/96 and IPv4-compatible ::/96
                || (segments[..5] == [0; 5] && matches!(segments[5], 0 | 0xffff) && embedded())
                // NAT64: the well-known 64:ff9b::/96 by the address inside, the local-use 64:ff9b:1::/48 whole
                || (segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] && embedded())
                || segments[..3] == [0x64, 0xff9b, 1]
        }
    }
}

fn has_content_type(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
}

fn check_header(name: &str, value: &str) -> Result<(), Error> {
    HeaderName::try_from(name)
        .map_err(|err| Error::new(format!("invalid HTTP header name {name}")).with_source(err))?;
    HeaderValue::try_from(value)
        .map_err(|err| Error::new(format!("invalid HTTP header value for {name}")).with_source(err))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_true_and_allow_false_denies_and_does_not_produce_a_send() {
        assert!(!sim_allows_http(true, false));
        let client = RestClient::new();
        let error = client
            .post("http://example.test/denied")
            .json(&serde_json::json!({"name": "nope"}))
            .prepare(true)
            .expect_err("sim denies");
        assert!(error.message().contains("denied because sim mode is active"), "{error}");
        assert!(error.message().contains("http://example.test/denied"));
        assert!(client.transport.get().is_none());
    }

    #[test]
    fn redaction_drops_user_password_and_query() {
        assert_eq!(
            redacted("https://user:secret@example.test:8443/a/b?token=1#x"),
            "https://example.test:8443/a/b"
        );
        assert_eq!(redacted("not a url"), "(invalid URL)");
    }

    #[test]
    fn only_http_and_https_are_sent_and_methods_parse() {
        let client = RestClient::new();
        let err = client.get("file:///etc/passwd").prepare(false).expect_err("scheme");
        assert!(err.message().contains("http or https"), "{err}");
        assert_eq!(Method::parse("put"), Some(Method::Put));
        assert_eq!(Method::parse("TRACE"), None);
        let delete = client.delete("http://example.test/x").prepare(false).unwrap();
        assert_eq!(delete.method(), Method::Delete);
    }

    #[test]
    fn internal_addresses_are_recognized() {
        for text in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.9",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "0.1.2.3",
            "100.64.0.1",
            "100.127.255.254",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::808:808",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "ff02::1",
            "::ffff:224.0.0.251",
        ] {
            assert!(internal_address(text.parse().unwrap()), "{text}");
        }
        for text in [
            "93.184.216.34",
            "100.128.0.1",
            "100.63.255.255",
            "::ffff:93.184.216.34",
            "64:ff9b::808:808",
            "2606:4700::1111",
        ] {
            assert!(!internal_address(text.parse().unwrap()), "{text}");
        }
        let client = RestClient::new().deny_internal_addresses(true);
        for url in [
            "http://127.0.0.1:9/x",
            "http://0x7f.1:9/x",
            "http://[::ffff:7f00:1]:9/x",
            "http://[64:ff9b::a9fe:a9fe]/latest",
        ] {
            let request = client.get(url).prepare(false).unwrap();
            let err = client
                .execute(request)
                .expect_err("an internal literal is denied before a socket");
            assert!(err.message().contains("internal address"), "{url}: {err}");
        }
    }

    #[test]
    fn an_internal_literal_is_found_in_a_url_and_a_name_is_left_to_the_resolver() {
        for url in [
            "http://127.0.0.1/x",
            "http://0x7f.1/x",
            "https://[::1]:8443/",
            "http://169.254.169.254/latest",
            "http://224.0.0.1/",
            "http://[ff02::1]/",
        ] {
            assert!(internal_literal(&reqwest::Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "http://93.184.216.34/",
            "https://example.com/",
            "http://localhost/",
            "https://[2606:4700::1111]/",
        ] {
            assert!(!internal_literal(&reqwest::Url::parse(url).unwrap()), "{url}");
        }
    }

    /// A server on a loopback port answering each connection with the next of `responses`, the whole response as
    /// written. Its port.
    fn serve(responses: Vec<String>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (stream, response) in listener.incoming().zip(responses) {
                let Ok(mut stream) = stream else { return };
                let mut buffer = [0_u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let _ =
                    std::io::Write::write_all(&mut stream, response.replace("{port}", &port.to_string()).as_bytes());
            }
        });
        port
    }

    #[test]
    fn a_large_body_is_an_error_or_its_start_when_truncated() {
        let body = "x".repeat(2000);
        let ok = |body: &str| {
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        let port = serve(vec![ok(&body), ok(&body)]);
        let url = format!("http://127.0.0.1:{port}/page");
        let strict = RestClient::new().max_response_bytes(1000);
        let err = strict.get(&url).send(false).expect_err("over the limit");
        assert!(err.message().contains("larger than the limit"), "{err}");
        let truncating = RestClient::new().max_response_bytes(1000).truncate_body(true);
        let response = truncating.get(&url).send(false).expect("its start");
        assert_eq!(response.status(), 200);
        assert_eq!(response.bytes().len(), 1000);
    }

    #[test]
    fn a_redirect_to_another_host_is_followed_only_when_asked() {
        // 127.0.0.1 and localhost are two hosts to the redirect policy
        let moved = "HTTP/1.1 302 Found\r\nlocation: http://localhost:{port}/next\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let ok = "HTTP/1.1 200 OK\r\ncontent-length: 4\r\nconnection: close\r\n\r\nhere";
        let port = serve(vec![moved.to_string(), moved.to_string(), ok.to_string()]);
        let url = format!("http://127.0.0.1:{port}/first");
        let same_host = RestClient::new().get(&url).send(false).expect("the 3xx comes back");
        assert_eq!(same_host.status(), 302);
        let any_host = RestClient::new()
            .redirects(3, true)
            .get(&url)
            .send(false)
            .expect("followed");
        assert_eq!(any_host.status(), 200);
        assert_eq!(any_host.body(), "here");
    }

    /// The whole chain of `err`, for an error that comes from inside reqwest.
    fn chain(err: &Error) -> String {
        let mut text = err.to_string();
        let mut source = std::error::Error::source(err);
        while let Some(next) = source {
            text.push_str(": ");
            text.push_str(&next.to_string());
            source = next.source();
        }
        text
    }

    #[test]
    fn a_name_that_resolves_internal_is_refused_by_the_resolver() {
        // `localhost` is a name, so only the resolver sees that it is loopback; nothing listens on port 9.
        let client = RestClient::new().deny_internal_addresses(true);
        let err = client.get("http://localhost:9/x").send(false).expect_err("denied");
        assert!(
            chain(&err).contains("localhost is an internal address"),
            "{}",
            chain(&err)
        );
        assert!(resolve_public("localhost").is_err());
        // Off, the same name is resolved and connected to, and only the connect fails.
        let open = RestClient::new()
            .get("http://localhost:9/x")
            .send(false)
            .expect_err("nothing listens");
        assert!(!chain(&open).contains("internal address"), "{}", chain(&open));
    }

    #[test]
    fn sim_true_and_allow_true_prepares_a_request() {
        assert!(sim_allows_http(true, true));
        let client = RestClient::new();
        let request = client
            .get("http://example.test/allowed")
            .allow_in_sim(true)
            .prepare(true)
            .expect("allow in sim");
        assert_eq!(request.method(), Method::Get);
        assert_eq!(request.url(), "http://example.test/allowed");
        assert!(client.transport.get().is_none());
    }

    #[test]
    fn sim_false_allows() {
        assert!(sim_allows_http(false, false));
        let client = RestClient::new();
        let request = client.get("http://example.test/live").prepare(false).expect("not sim");
        assert_eq!(request.method(), Method::Get);
        assert_eq!(request.url(), "http://example.test/live");
        assert!(request.body().is_none());
        assert!(client.transport.get().is_none());
    }

    #[test]
    fn get_and_post_json_show_method_url_and_body() {
        let client = RestClient::new();
        let get = client
            .get("http://example.test/items")
            .header("accept", "application/json")
            .prepare(false)
            .expect("get");
        assert_eq!(get.method(), Method::Get);
        assert_eq!(get.url(), "http://example.test/items");
        assert_eq!(get.body(), None);
        assert_eq!(get.headers(), [("accept".to_string(), "application/json".to_string())]);

        let post = client
            .post("http://example.test/items")
            .header("x-request-id", "one")
            .json(&serde_json::json!({"name": "widget"}))
            .prepare(false)
            .expect("post");
        assert_eq!(post.method(), Method::Post);
        assert_eq!(post.url(), "http://example.test/items");
        assert_eq!(post.body(), Some(r#"{"name":"widget"}"#));
        assert_eq!(
            post.headers(),
            [
                ("x-request-id".to_string(), "one".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]
        );
        assert!(client.transport.get().is_none());
    }
}
