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

//! Owned HTTP request and response. The host copies bytes in and writes bytes out.
//!
//! The slot is thread-local and cleared when its guard drops, including on panic.
//! A request-pool worker reuses its thread, so a leaked slot would leak into the next request.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use indexmap::IndexMap;
use moqui_lib::object::Value;

pub mod address;
pub mod error_info;
pub mod multipart;
pub mod servlet;
pub(crate) mod upstream;

use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::entity::query_rest;

/// The cap on a request body that is not multipart, unless `webapp_max_body` says otherwise.
const DEFAULT_MAX_BODY: usize = 10 * 1024 * 1024;

thread_local! {
    static WEB: RefCell<Option<WebSlot>> = const { RefCell::new(None) };
    /// Set by a login with credentials that were not valid, for the host's web protection to count.
    static AUTH_FAILED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the work on this thread as having tried credentials that were not valid.
pub(crate) fn note_auth_failure() {
    AUTH_FAILED.with(|flag| flag.set(true));
}

/// Whether the work on this thread tried credentials that were not valid since the last call, and forget it. The
/// host asks after a request's job; a worker's cleanup forgets it so it does not reach the next job.
pub fn take_auth_failure() -> bool {
    AUTH_FAILED.with(|flag| flag.replace(false))
}

/// One HTTP request, already read off the socket.
#[derive(Clone, Debug)]
pub struct WebRequest {
    pub method: String,
    pub path: String,
    pub https: bool,
    pub headers: BTreeMap<String, String>,
    /// Decoded query pairs in request order. A repeated name becomes a list parameter.
    pub query: Vec<(String, String)>,
    /// The query string as the client sent it, still encoded (the servlet's `getQueryString`), when the host has it.
    pub raw_query: Option<String>,
    pub body: Vec<u8>,
    /// A `multipart/form-data` body the host read as it arrived ([`multipart::MultipartReader`]), its large files
    /// spooled to disk; `body` is empty then.
    pub multipart: Option<std::sync::Arc<multipart::Parsed>>,
    pub peer_ip: String,
    pub session_cookie: Option<String>,
    pub visitor_cookie: Option<String>,
}

impl WebRequest {
    /// `GET` with an empty body. Headers are stored under lowercase names.
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            https: false,
            headers: BTreeMap::new(),
            query: Vec::new(),
            raw_query: None,
            body: Vec::new(),
            multipart: None,
            peer_ip: String::new(),
            session_cookie: None,
            visitor_cookie: None,
        }
    }

    /// Header value. The lookup lowercases `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn set_header(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.headers.insert(name.into().to_ascii_lowercase(), value.into());
    }
}

/// Session, visitor, and username for one HTTP log entry. Not response headers.
#[derive(Clone, Debug, Default)]
pub struct HttpLogIdentity {
    pub session: Option<String>,
    pub visitor_id: Option<String>,
    pub remote_user: Option<String>,
}

/// One HTTP response. Repeated names are separate headers, so `Set-Cookie` can appear twice. A large body is a
/// [`ResponseStream`] the host reads a window at a time as the client takes it, and `body` is empty.
#[derive(Clone, Debug)]
pub struct WebResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub stream: Option<ResponseStream>,
    /// Filled while the session is still bound. Empty when the request never bound one.
    pub http_log: HttpLogIdentity,
}

/// `length` bytes of `source` from `offset`: the body of a response too large to hold.
#[derive(Clone)]
pub struct ResponseStream {
    pub source: std::sync::Arc<dyn crate::resource::source::ByteSource>,
    pub offset: u64,
    /// [`ResponseStream::UNKNOWN_LENGTH`] reads until the source has no more.
    pub length: u64,
}

impl ResponseStream {
    /// The length of a stream that ends when its source does, such as a proxied body sent in chunks.
    pub const UNKNOWN_LENGTH: u64 = u64::MAX;
}

impl std::fmt::Debug for ResponseStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseStream")
            .field("offset", &self.offset)
            .field("length", &self.length)
            .finish()
    }
}

impl WebResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            stream: None,
            http_log: HttpLogIdentity::default(),
        }
    }

    /// The length of the body, streamed or not.
    pub fn body_len(&self) -> u64 {
        self.stream
            .as_ref()
            .map_or(self.body.len() as u64, |stream| stream.length)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

struct WebSlot {
    parameters: BTreeMap<String, Value>,
    /// Uploaded files by field name, and `_fileUploadList`.
    multipart: BTreeMap<String, Value>,
    /// What the last response saved for this page, taken from the session when first asked for.
    saved: Option<crate::user::session::SavedForPage>,
    /// The `Host` header of the request.
    host: String,
    /// The full URL of the request and its `Referer`, for an artifact hit. Empty with no socket.
    request_url: String,
    referrer: String,
    /// `getRequestDetails`: the request line, scheme, remote address, and headers, for the System dashboard.
    request_details: String,
    /// The names in the URL query string, which `getSecureRequestParameters` leaves out.
    query_names: Vec<String>,
    /// The method and the conditional headers of the request, for a file response (`sendResourceResponse`).
    conditional: crate::screen::static_files::Conditional,
    /// [`client_ip`] of the request, for the login `ipAllowed` check.
    client_ip: String,
    /// The first locale of the request's `Accept-Language`, as Java's `request.getLocale()`.
    locale: Option<String>,
    /// Scheme, host, and context path that links start with. Empty for a path-only link.
    base_url: String,
    /// Whether the request came over https; None with no request, as for a render from a job.
    secure: Option<bool>,
    /// The request itself and its raw body, for a web script's `request` ([`set_request`]). None with no socket.
    request: Option<std::sync::Arc<WebRequest>>,
    request_body: bytes::Bytes,
    /// The servlet API's request attributes: what a filter leaves for the servlet after it.
    attributes: BTreeMap<String, Value>,
    /// The servlet path and path info of the servlet the request went to ([`servlet::Match`]).
    servlet_path: String,
    path_info: Option<String>,
    /// The request path, Java's `getPathInfo`, for `sendError`.
    path: String,
    /// The `Accept` header, for `sendError`.
    accept: String,
    sent: bool,
    /// Whether the body was written through [`write_bytes`], which may go on writing after it committed.
    writing: bool,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    stream: Option<ResponseStream>,
    /// The user preferences this request set, by key (`None` removed), which the response tells the client
    /// (`X-Moqui-Preferences`, [`take_changed_preferences`]).
    changed_preferences: BTreeMap<String, Option<String>>,
}

/// Puts back the thread-local web slot that was there before the install, or clears it when there was
/// none, including when a panic unwinds. A render started from inside another render keeps the outer
/// one's slot this way.
pub struct WebGuard {
    previous: Option<WebSlot>,
}

impl Drop for WebGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        WEB.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Install the parameter map for this request. A second install replaces the first.
pub(crate) fn install(parameters: BTreeMap<String, Value>) -> WebGuard {
    install_with(parameters, String::new())
}

/// [`install`] with the base URL a screen render builds links from. A `makeRender` with no socket
/// fills the slot this way: the parameter map and base URL here, the session map on the session,
/// and the output writer is the slot's body (see [`start_output`] and [`write_output`]).
pub(crate) fn install_with(parameters: BTreeMap<String, Value>, base_url: String) -> WebGuard {
    let previous = WEB.with(|slot| {
        slot.borrow_mut().replace(WebSlot {
            parameters,
            multipart: BTreeMap::new(),
            saved: None,
            host: String::new(),
            request_url: String::new(),
            referrer: String::new(),
            request_details: String::new(),
            query_names: Vec::new(),
            conditional: Default::default(),
            client_ip: String::new(),
            locale: None,
            base_url,
            secure: None,
            request: None,
            request_body: bytes::Bytes::new(),
            attributes: BTreeMap::new(),
            servlet_path: String::new(),
            path_info: None,
            path: String::new(),
            accept: String::new(),
            changed_preferences: BTreeMap::new(),
            sent: false,
            writing: false,
            status: 200,
            headers: Vec::new(),
            body: Vec::new(),
            stream: None,
        })
    });
    WebGuard { previous }
}

/// One request parameter, without copying the others.
pub(crate) fn parameter(name: &str) -> Option<Value> {
    WEB.with(|slot| slot.borrow().as_ref().and_then(|web| web.parameters.get(name).cloned()))
}

pub(crate) fn parameters() -> BTreeMap<String, Value> {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| web.parameters.clone())
            .unwrap_or_default()
    })
}

/// Keep the uploaded files of this request for `ec.web.multiPartParameters`.
pub(crate) fn set_multipart(files: BTreeMap<String, Value>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.multipart = files;
        }
    });
}

/// The messages, errors and failed-submit parameters the last response saved, which this page shows. The
/// session gives them up the first time they are asked for in a request.
pub(crate) fn saved_for_page() -> crate::user::session::SavedForPage {
    let loaded = WEB.with(|slot| slot.borrow().as_ref().and_then(|web| web.saved.clone()));
    if let Some(saved) = loaded {
        return saved;
    }
    let taken = crate::user::session::take_saved_for_page();
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.saved = Some(taken.clone());
        }
    });
    taken
}

/// Lower-case hex of the time this process first asked, so every link made in one run shares it.
pub(crate) fn resource_distinct_value() -> String {
    static START: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    START
        .get_or_init(|| {
            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            format!("{millis:x}")
        })
        .clone()
}

pub(crate) fn set_host(host: &str) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.host = host.to_string();
        }
    });
}

/// The full URL of a request for artifact hits and the visit, Java's `getRequestUrl`: the scheme, the host, its port
/// unless it is 80 or 443, the path, and the query string as the client sent it. Without the raw query (a request the
/// host did not make) the pairs are encoded again.
fn full_url(request: &WebRequest) -> String {
    let host = request.header("host").unwrap_or("localhost");
    let scheme = if request.https { "https" } else { "http" };
    let name = host_without_port(host);
    let port = host[name.len()..]
        .strip_prefix(':')
        .filter(|port| !port.is_empty() && *port != "80" && *port != "443");
    let mut url = match port {
        Some(port) => format!("{scheme}://{name}:{port}{}", request.path),
        None => format!("{scheme}://{name}{}", request.path),
    };
    let query = match &request.raw_query {
        Some(raw) => raw.clone(),
        None => request
            .query
            .iter()
            .map(|(name, value)| {
                format!(
                    "{}={}",
                    moqui_lib::string::url_encode_if_needed(name),
                    moqui_lib::string::url_encode_if_needed(value)
                )
            })
            .collect::<Vec<_>>()
            .join("&"),
    };
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query);
    }
    url
}

/// What a Visit this request starts records of it (`user::session::create_visit`).
pub(crate) fn visit_request(request: &WebRequest) -> crate::user::session::VisitRequest {
    let locale = request
        .header("accept-language")
        .and_then(|header| header.split(',').next())
        .map(|tag| tag.split(';').next().unwrap_or("").trim().replace('-', "_"))
        .filter(|tag| !tag.is_empty() && tag != "*")
        .unwrap_or_default();
    crate::user::session::VisitRequest {
        url: full_url(request),
        referrer: request.header("referer").unwrap_or("").to_string(),
        user_agent: request.header("user-agent").unwrap_or("").to_string(),
        locale,
        client_ip: client_ip(request),
    }
}

pub(crate) fn set_request_info(request: &WebRequest) {
    let scheme = if request.https { "https" } else { "http" };
    let url = full_url(request);
    let referrer = request.header("referer").unwrap_or("").to_string();
    let mut details = format!(
        "Request: {} {url}\nScheme: {scheme}, Secure? {}\nRemote: {}\n",
        request.method, request.https, request.peer_ip
    );
    for (name, value) in &request.headers {
        // The credentials a header carries are not shown.
        let shown = if name == "authorization" || name == "cookie" {
            "(hidden)"
        } else {
            value.as_str()
        };
        details.push_str(&format!("Header: {name} = [{shown}]\n"));
    }
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.request_url = url;
            web.secure = Some(request.https);
            web.referrer = referrer;
            web.path = request.path.clone();
            web.accept = request.header("accept").unwrap_or("").to_string();
            web.request_details = details;
            web.query_names = request.query.iter().map(|(name, _)| name.clone()).collect();
            web.client_ip = client_ip(request);
            web.conditional = crate::screen::static_files::Conditional::of(request);
        }
    });
}

/// Java `getClientIp`: the [`client_ip`] of the current request. Empty with no request.
pub(crate) fn current_client_ip() -> String {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map_or_else(String::new, |web| web.client_ip.clone())
    })
}

/// `getSecureRequestParameters`: the parameters that did not come in the URL query string (the body, a multipart
/// part), so a value that must not be in a link, such as the SQL runner's statement, is not read from one.
pub(crate) fn secure_parameters() -> BTreeMap<String, Value> {
    WEB.with(|slot| {
        slot.borrow().as_ref().map_or_else(BTreeMap::new, |web| {
            web.parameters
                .iter()
                .filter(|(name, _)| !web.query_names.contains(name))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
    })
}

/// The text [`set_request_info`] kept, with the parameters. Empty with no request. A credential or other secret
/// parameter (the names a hit row leaves out) is not listed.
pub(crate) fn request_details() -> String {
    WEB.with(|slot| {
        slot.borrow().as_ref().map_or_else(String::new, |web| {
            let mut text = web.request_details.clone();
            for (name, value) in &web.parameters {
                if crate::context::secret_parameter(name) {
                    continue;
                }
                text.push_str(&format!(
                    "Parameter: {name} = {}\n",
                    moqui_lib::object::to_plain_string(value)
                ));
            }
            text
        })
    })
}

/// The URL [`set_request_info`] kept. None with no request, or for a render with no socket.
pub(crate) fn request_url() -> Option<String> {
    WEB.with(|slot| slot.borrow().as_ref().map(|web| web.request_url.clone()))
        .filter(|url| !url.is_empty())
}

pub(crate) fn referrer_url() -> Option<String> {
    WEB.with(|slot| slot.borrow().as_ref().map(|web| web.referrer.clone()))
        .filter(|url| !url.is_empty())
}

/// Keep the request's preferred locale from its `Accept-Language` header, such as `de-DE,de;q=0.9` as `de_DE`.
pub(crate) fn set_request_locale(accept_language: Option<&str>) {
    let locale = accept_language.and_then(|header| {
        header
            .split(',')
            .map(|part| part.split(';').next().unwrap_or("").trim())
            .find(|tag| !tag.is_empty() && *tag != "*")
            .map(|tag| tag.replace('-', "_"))
    });
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.locale = locale;
        }
    });
}

/// The request's preferred locale, when a request is in progress and named one.
pub(crate) fn request_locale() -> Option<String> {
    WEB.with(|slot| slot.borrow().as_ref().and_then(|web| web.locale.clone()))
}

/// The host name the request was for, with the port when asked.
pub(crate) fn host_name(with_port: bool) -> String {
    let host = WEB.with(|slot| slot.borrow().as_ref().map(|web| web.host.clone()).unwrap_or_default());
    if with_port {
        return host;
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|ch| ch.is_ascii_digit()) => name.to_string(),
        _ => host,
    }
}

pub(crate) fn multipart_parameters() -> BTreeMap<String, Value> {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| web.multipart.clone())
            .unwrap_or_default()
    })
}

/// Java's `WebUtilities.isSameOriginRedirect`: a path on this server, or an http(s) URL whose host is
/// `configured_host` (else the host of this request's base URL). Scripts, `//` URLs, user info, and control
/// characters are refused.
pub(crate) fn is_same_origin_redirect(url: &str, configured_host: Option<&str>) -> bool {
    let url = url.trim();
    if url.is_empty() || url.contains('\\') || url.chars().any(|ch| (ch as u32) < 32 || ch as u32 == 127) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("vbscript:")
        || url.starts_with("//")
    {
        return false;
    }
    if url.starts_with('/') {
        return true;
    }
    let Some(rest) = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")) else {
        return false;
    };
    let configured = configured_host.map_or_else(
        || {
            let base = base_url();
            let rest = base.split_once("://").map_or("", |(_, rest)| rest);
            rest.split(['/', ':']).next().unwrap_or("").to_string()
        },
        str::to_string,
    );
    if configured.is_empty() {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return false;
    }
    let host = authority.split(':').next().unwrap_or("");
    !host.is_empty() && host.eq_ignore_ascii_case(&configured)
}

/// Keep the request and its raw body for a web script's `request`. The parsed parameters are already in the slot.
pub(crate) fn set_request(request: std::sync::Arc<WebRequest>, body: bytes::Bytes) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.request = Some(request);
            web.request_body = body;
        }
    });
}

/// The request of this thread's web slot, None with no socket.
pub(crate) fn current_request() -> Option<std::sync::Arc<WebRequest>> {
    WEB.with(|slot| slot.borrow().as_ref().and_then(|web| web.request.clone()))
}

/// The raw body of the request, empty with none or for a multipart body (read as it arrived, files spooled).
pub(crate) fn request_body() -> bytes::Bytes {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| web.request_body.clone())
            .unwrap_or_default()
    })
}

/// The servlet path and path info of the servlet the request went to.
pub(crate) fn set_servlet_match(servlet_path: &str, path_info: Option<&str>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.servlet_path = servlet_path.to_string();
            web.path_info = path_info.map(str::to_string);
        }
    });
}

pub(crate) fn servlet_match() -> (String, Option<String>) {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| (web.servlet_path.clone(), web.path_info.clone()))
            .unwrap_or_default()
    })
}

/// A request attribute; None when unset.
pub(crate) fn attribute(name: &str) -> Option<Value> {
    WEB.with(|slot| slot.borrow().as_ref().and_then(|web| web.attributes.get(name).cloned()))
}

/// Set a request attribute, or remove it with None.
pub(crate) fn set_attribute(name: &str, value: Option<Value>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            match value {
                Some(value) => web.attributes.insert(name.to_string(), value),
                None => web.attributes.remove(name),
            };
        }
    });
}

pub(crate) fn attribute_names() -> Vec<String> {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| web.attributes.keys().cloned().collect())
            .unwrap_or_default()
    })
}

/// The servlet API's `setStatus`: the status of the response still to be written; a written one is left alone.
pub(crate) fn set_status(status: u16) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            if !web.sent {
                web.status = status;
            }
        }
    });
}

/// The servlet API's `setHeader`: replace every value of `name`, or remove them with None.
pub(crate) fn set_header(name: &str, value: Option<&str>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
            if let Some(value) = value {
                web.headers.push((name.to_string(), value.to_string()));
            }
        }
    });
}

/// The values of the response header `name` so far.
pub(crate) fn response_header_values(name: &str) -> Vec<String> {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| {
                web.headers
                    .iter()
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// The names of the response headers so far, each once.
pub(crate) fn response_header_names() -> Vec<String> {
    WEB.with(|slot| {
        let mut names: Vec<String> = Vec::new();
        if let Some(web) = slot.borrow().as_ref() {
            for (name, _) in &web.headers {
                if !names.iter().any(|known| known.eq_ignore_ascii_case(name)) {
                    names.push(name.clone());
                }
            }
        }
        names
    })
}

/// The servlet API's writer: append bytes to the body and commit the response, its status and headers as they are.
/// Nothing is written after a response another way sent (an error, a redirect, a stream).
pub(crate) fn write_bytes(bytes: &[u8]) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            if web.stream.is_some() || (web.sent && !web.writing) {
                return;
            }
            web.sent = true;
            web.writing = true;
            web.body.extend_from_slice(bytes);
        }
    });
}

/// Put a whole response in the slot, as the screen servlet made it, for the filters around it to see and change.
pub(crate) fn put_response(response: WebResponse) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.sent = true;
            web.status = response.status;
            web.headers = response.headers;
            web.body = response.body;
            web.stream = response.stream;
        }
    });
}

/// The response the slot holds, its status 200 when nothing set one.
pub(crate) fn take_response() -> WebResponse {
    let stream = take_stream();
    let (status, headers, body, _) = take_output();
    WebResponse {
        status,
        headers,
        body,
        stream,
        http_log: HttpLogIdentity::default(),
    }
}

/// Whether the request came over https, None with no request (a job, a render from a script).
pub(crate) fn request_secure() -> Option<bool> {
    WEB.with(|slot| slot.borrow().as_ref().and_then(|web| web.secure))
}

/// Java's `WebFacadeImpl.getWebappRootUrl`: empty, as the context path, when a path-only link will do, which is in a
/// request whose security matches `use_encryption`; otherwise the scheme and host. Those are the render's base URL
/// when it has one (an email, a render with no socket), else the conf `webapp`'s host and port. Never the request's
/// `Host` header, which the sender of the request picks (Parity-Diffs P8-08).
pub(crate) fn webapp_root_url(webapp_name: &str, require_full_url: bool, use_encryption: Option<bool>) -> String {
    let secure = request_secure();
    let require_encryption = use_encryption.unwrap_or(secure.unwrap_or(false));
    if !require_full_url && secure == Some(require_encryption) {
        return String::new();
    }
    let base = base_url();
    if base.starts_with("http://") || base.starts_with("https://") {
        return base.trim_end_matches('/').to_string();
    }
    let https = require_encryption || secure == Some(true);
    ExecutionContext::with_current(|ec| ec.factory().webapp_root_url_for(webapp_name, Some(https))).unwrap_or_default()
}

pub(crate) fn base_url() -> String {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|web| web.base_url.clone())
            .unwrap_or_default()
    })
}

/// Open the response body for a screen render: status, `Content-Type`, and an empty body that
/// [`write_output`] appends to. A response something already sent is left alone and `false` returns.
pub(crate) fn start_output(status: u16, content_type: &str) -> bool {
    WEB.with(|slot| {
        let mut web = slot.borrow_mut();
        let Some(web) = web.as_mut() else {
            return false;
        };
        if web.sent {
            return false;
        }
        web.sent = true;
        web.status = status;
        web.headers.push(("Content-Type".to_string(), content_type.to_string()));
        true
    })
}

/// Append text to the open body. Nothing happens before [`start_output`].
pub(crate) fn write_output(text: &str) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            if web.sent {
                web.body.extend_from_slice(text.as_bytes());
            }
        }
    });
}

/// Drop what was written to the body after its first `len` bytes ([`output_body_len`]).
pub(crate) fn truncate_output(len: usize) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.body.truncate(len);
        }
    });
}

/// Read the body so far. The closure must not write the response: the slot is borrowed while it runs.
pub(crate) fn with_body<R>(read: impl FnOnce(&[u8]) -> R) -> R {
    WEB.with(|slot| {
        let borrowed = slot.borrow();
        let bytes = borrowed.as_ref().map(|web| web.body.as_slice()).unwrap_or(b"");
        read(bytes)
    })
}

/// Replace the body from `start` to the end. Headers and the sent flag stay.
pub(crate) fn rewrite_tail(start: usize, bytes: &[u8]) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            let start = start.min(web.body.len());
            web.body.truncate(start);
            web.body.extend_from_slice(bytes);
        }
    });
}

/// Replace the whole body. Headers and the sent flag stay.
pub(crate) fn replace_body(body: Vec<u8>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.body = body;
        }
    });
}

/// Note a user preference this request set (`ec.user.setPreference`), for the response to tell the client. Nothing
/// outside a web request.
pub(crate) fn note_changed_preference(key: &str, value: Option<&str>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.changed_preferences
                .insert(key.to_string(), value.map(str::to_string));
        }
    });
}

/// The user preferences this request set, taken: what the `X-Moqui-Preferences` header of the response carries.
pub(crate) fn take_changed_preferences() -> BTreeMap<String, Option<String>> {
    WEB.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|web| std::mem::take(&mut web.changed_preferences))
            .unwrap_or_default()
    })
}

/// The `X-Moqui-Preferences` header value of `changed`: the JSON map of key to value (null for a removed one),
/// percent-encoded as `encodeURIComponent` encodes, so any text is a header value; the client reads it with
/// `decodeURIComponent` and applies each (`moqui.applyPreference`).
pub(crate) fn preferences_header(changed: &BTreeMap<String, Option<String>>) -> String {
    let json = serde_json::Value::Object(
        changed
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_ref()
                        .map_or(serde_json::Value::Null, |text| serde_json::Value::String(text.clone())),
                )
            })
            .collect(),
    )
    .to_string();
    let mut out = String::with_capacity(json.len() * 2);
    for byte in json.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub(crate) fn add_header(name: impl Into<String>, value: impl Into<String>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.headers.push((name.into(), value.into()));
        }
    });
}

/// `application/json`.
///
/// As Java's `sendJsonResponseInternal`: a string is JSON text already and is sent as it is. Otherwise the messages
/// are added as `messages` (or make the whole response when there is none), the errors as `errors` unless the map
/// has its own or is a JSON-RPC answer, and any error makes the status 500.
pub(crate) fn send_json_response(value: Value) {
    if let Value::String(text) = value {
        send_bytes(200, "application/json", text.into_bytes());
        return;
    }
    let messages = ExecutionContext::with_current(|ec| ec.message().messages().to_vec()).unwrap_or_default();
    let mut value = value;
    if !messages.is_empty() {
        match &mut value {
            Value::Null => {
                value = Value::Map(indexmap::IndexMap::from([(
                    "messages".to_string(),
                    Value::string(messages.join("\n")),
                )]));
            }
            Value::Map(map) if !map.contains_key("messages") => {
                map.insert("messages".to_string(), Value::string(messages.join("\n")));
            }
            _ => {}
        }
    }
    let errors = message_errors();
    let status = if errors.is_empty() {
        200
    } else {
        match &mut value {
            Value::Map(map) if !map.contains_key("errors") && !map.contains_key("jsonrpc") => {
                map.insert("errors".to_string(), Value::string(errors.join("\n")));
            }
            Value::Map(_) => {}
            _ => tracing::error!(
                "Error found when sending JSON, the JSON is not a map so no errors were added: {}",
                errors.join("; ")
            ),
        }
        500
    };
    if value.is_null() {
        send_bytes(status, "application/json", Vec::new());
        return;
    }
    send_bytes(status, "application/json", query_rest::encode_json(&value).into_bytes());
}

/// `ec.web.sendError(errorCode, message, throwable)`, as Java's `WebFacadeImpl.sendError`.
///
/// `text/html` in `Accept` writes the small HTML page, `application/json` or `text/json` writes Java's
/// single-quoted body, and anything else is `text/plain`. An empty message uses the throwable's message,
/// then the name of the code. The path is the request path.
pub(crate) fn send_error(error_code: u16, message: &str, throwable_message: Option<&str>) {
    let name = error_code_name(error_code);
    let message = if !message.is_empty() {
        message.to_string()
    } else if let Some(from_throwable) = throwable_message.filter(|text| !text.is_empty()) {
        from_throwable.to_string()
    } else {
        name.to_string()
    };
    let path = request_path();
    let accept = request_accept();
    let (content_type, body) = if accept.contains("text/html") {
        let mut body = format!(
            "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\"/><title>Error {error_code} {name}</title></head><body>\n<h2>Error {error_code} {name}</h2>\n<p>Problem accessing {}</p>\n",
            crate::screen::sri::encode_html(&path)
        );
        if !message.is_empty() {
            body.push_str(&format!(
                "<p>Reason: {}</p>\n",
                crate::screen::sri::encode_html(&message)
            ));
        }
        body.push_str("</body></html>\n");
        ("text/html;charset=UTF-8", body)
    } else if accept.contains("application/json") || accept.contains("text/json") {
        let body = format!(
            "{{'message':'{}','errorName':'{name}','error':{error_code},'path':'{}'}}",
            json_quote_as_string(&message),
            json_quote_as_string(&path)
        );
        ("application/json;charset=UTF-8", body)
    } else {
        ("text/plain;charset=UTF-8", format!("{error_code} {message} {path}"))
    };
    send_bytes_with_headers(
        error_code,
        vec![("Content-Type".to_string(), content_type.to_string())],
        body.into_bytes(),
    );
}

fn error_code_name(code: u16) -> &'static str {
    match code {
        401 => "Authentication Required",
        403 => "Access Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "",
    }
}

/// Jackson `JsonStringEncoder.quoteAsString`: the escapes inside the quotes, without the quotes.
fn json_quote_as_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{000c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn request_path() -> String {
    WEB.with(|slot| slot.borrow().as_ref().map(|web| web.path.clone()).unwrap_or_default())
}

fn request_accept() -> String {
    WEB.with(|slot| slot.borrow().as_ref().map(|web| web.accept.clone()).unwrap_or_default())
}

/// `{"errorCode","errors"}` as JSON.
pub(crate) fn send_json_error(status: u16, message: &str) {
    let mut map = IndexMap::new();
    map.insert("errorCode".to_string(), Value::I64(i64::from(status)));
    map.insert("errors".to_string(), Value::string(message));
    send_bytes(
        status,
        "application/json",
        query_rest::encode_json(&Value::Map(map)).into_bytes(),
    );
}

/// `text/plain` with status 200.
pub(crate) fn send_text_response(text: &str) {
    send_text_response_with(text, None, None);
}

/// `ec.web.sendTextResponse(text, contentType, filename)`, as Java: the text in UTF-8 with the content type (`text/plain`
/// when none is given; `;charset=UTF-8` is added unless it names one), and with a file name an attachment of that name.
pub(crate) fn send_text_response_with(text: &str, content_type: Option<&str>, file_name: Option<&str>) {
    let content_type = content_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("text/plain");
    let content_type = if content_type.to_ascii_lowercase().contains("charset=") {
        content_type.to_string()
    } else {
        format!("{content_type};charset=UTF-8")
    };
    let mut headers = vec![("Content-Type".to_string(), content_type)];
    match file_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(name) => headers.push(("Content-Disposition".to_string(), attachment_disposition(name))),
        None => headers.push(("Content-Disposition".to_string(), "inline".to_string())),
    }
    // As Java's `sendTextResponseInternal`: with an error in the message facade the errors go instead, with a 500.
    let errors = message_errors();
    if !errors.is_empty() {
        let body: String = errors.iter().map(|error| format!("{error}\n")).collect();
        send_bytes_with_headers(500, headers, body.into_bytes());
        return;
    }
    send_bytes_with_headers(200, headers, text.as_bytes().to_vec());
}

/// `attachment; filename="..."; filename*=utf-8''...`: the name with the characters a quoted header value cannot hold
/// replaced, and its percent-encoded UTF-8 form.
fn attachment_disposition(file_name: &str) -> String {
    let safe: String = file_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_graphic() && !matches!(ch, '"' | '\\' | ';') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = file_name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{safe}\"; filename*=utf-8''{encoded}")
}

/// `ec.web.sendResourceResponse(location, inline, fileName)`: the file at a resource location (a `#versionName` suffix
/// sends that version, on storage that keeps versions), with the type of its file name, shown inline or as an
/// attachment named `file_name` (the location's last segment when none is given, so a zip or a copy kept under a random
/// name downloads under its own name); a large one streams, and a `Range` or `If-None-Match` of the request is
/// answered. Nothing at the location, a directory, or a version that is not there is 404.
///
/// Only an application's own files (`component://`, `classpath://`) shown inline may be kept by a shared cache, as Java
/// sends every inline resource; anything else (a drive's or a record's file, a temporary zip) is `private, no-store`, so
/// a proxy or a shared browser keeps no copy of what a user was allowed to read.
pub(crate) fn send_resource_response(location: &str, inline: bool, file_name: Option<&str>) {
    let opened = ExecutionContext::with_current(|ec| ec.factory().resource().open(location)).transpose();
    let opened = match opened {
        Ok(Some(Some(opened))) => opened,
        Ok(_) => {
            tracing::warn!("Sending not found response, resource not found at: {location}");
            send_bytes(
                404,
                "text/plain",
                format!("Resource not found at {location}").into_bytes(),
            );
            return;
        }
        Err(err) => {
            tracing::warn!(
                "Sending error response, resource at {location} cannot be read: {}",
                err.message()
            );
            send_bytes(
                500,
                "text/plain",
                format!("Resource at {location} cannot be read").into_bytes(),
            );
            return;
        }
    };
    let base = location.split_once('#').map_or(location, |(base, _)| base);
    let last = base.trim_end_matches('/').rsplit('/').next().unwrap_or(base);
    let file_name = file_name.map(str::trim).filter(|name| !name.is_empty()).unwrap_or(last);
    // a type the name does not tell is bytes to save, never sniffed as a page
    let content_type = moqui_lib::resource::content_type_for_name(file_name)
        .or_else(|| moqui_lib::resource::content_type_for_name(last))
        .unwrap_or("application/octet-stream");
    let mut headers = vec![("Content-Type".to_string(), content_type.to_string())];
    let application_file = matches!(
        base.split_once("://").map(|(scheme, _)| scheme),
        Some("component" | "classpath")
    );
    if inline {
        headers.push(("Content-Disposition".to_string(), "inline".to_string()));
    } else {
        headers.push(("Content-Disposition".to_string(), attachment_disposition(file_name)));
    }
    // An inline file of the app gets the conf's `web-resource-inline` headers, as Java's; any other file is the user's.
    if inline && application_file {
        apply_conf_headers(None, "web-resource-inline", &mut headers, false);
    } else {
        headers.push(("Cache-Control".to_string(), "private, no-store".to_string()));
    }
    let conditional = WEB
        .with(|slot| slot.borrow().as_ref().map(|web| web.conditional.clone()))
        .unwrap_or_default();
    if let Err(err) = crate::screen::static_files::respond_opened(&conditional, headers, &opened) {
        tracing::warn!(
            "Sending error response, resource at {location} cannot be read: {}",
            err.message()
        );
        send_bytes(
            500,
            "text/plain",
            format!("Resource at {location} cannot be read").into_bytes(),
        );
    }
}

/// The signer for QZ Tray, from `qz-private-key.pem`. Java reads it once from the classpath at class load. This
/// looks in `classpath://` (the framework directory), then `classes/` and `conf/` under the runtime directory,
/// and keeps the first signer that loaded, so a key added later is found without a restart.
fn qz_signer() -> std::sync::Arc<moqui_lib::signer::SimpleSigner> {
    use moqui_lib::signer::SimpleSigner;
    use std::sync::{Arc, Mutex};
    static SIGNER: Mutex<Option<Arc<SimpleSigner>>> = Mutex::new(None);
    if let Some(signer) = SIGNER.lock().ok().and_then(|slot| slot.clone()) {
        return signer;
    }
    let mut candidates = Vec::new();
    if let Ok(path) = crate::context::locate("classpath://qz-private-key.pem") {
        candidates.push(path);
    }
    if let Some(runtime) = ExecutionContext::with_current(|ec| ec.factory().runtime_dir().to_path_buf()) {
        candidates.push(runtime.join("classes/qz-private-key.pem"));
        candidates.push(runtime.join("conf/qz-private-key.pem"));
    }
    let mut found = Arc::new(SimpleSigner::from_pem("qz-private-key.pem", ""));
    for path in candidates.iter().filter(|path| path.is_file()) {
        let signer = Arc::new(SimpleSigner::from_file(path));
        let loaded = signer.is_loaded();
        found = signer;
        if loaded {
            if let Ok(mut slot) = SIGNER.lock() {
                *slot = Some(Arc::clone(&found));
            }
            break;
        }
    }
    found
}

/// `ec.web.sendQzSignedResponse(message)`: the Base64 `SHA1withRSA` signature of `message` as `text/plain`.
/// A request with no user gets 401. A key that cannot be read gets 500 with the reason, as Java does.
pub(crate) fn send_qz_signed_response(message: &str) {
    match crate::user::current_user_id() {
        Ok(Some(_)) => {}
        _ => {
            send_bytes(401, "text/plain", b"Authentication required".to_vec());
            return;
        }
    }
    match qz_signer().sign(message) {
        Ok(signature) => send_bytes(200, "text/plain", signature.into_bytes()),
        Err(err) => {
            tracing::error!("Error signing QZ message, sending error response: {}", err.message());
            send_bytes(500, "text/plain", err.message().as_bytes().to_vec());
        }
    }
}

/// 302 with a `Location` header. A response that was already sent is left as it is.
pub(crate) fn send_redirect(location: &str) {
    WEB.with(|slot| {
        let mut web = slot.borrow_mut();
        let Some(web) = web.as_mut() else {
            return;
        };
        if web.sent {
            return;
        }
        web.sent = true;
        web.status = 302;
        web.headers.push(("Location".to_string(), location.to_string()));
    });
}

/// 205 with `X-Redirect-To`, the redirect a client router follows for a standalone render.
pub(crate) fn send_reset_content(location: &str) {
    WEB.with(|slot| {
        let mut web = slot.borrow_mut();
        let Some(web) = web.as_mut() else {
            return;
        };
        if web.sent {
            return;
        }
        web.sent = true;
        web.status = 205;
        web.headers.push(("X-Redirect-To".to_string(), location.to_string()));
    });
}

/// A response with its own headers and bytes, such as a static file. One already sent is left alone.
pub(crate) fn send_bytes_with_headers(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) {
    WEB.with(|slot| {
        let mut web = slot.borrow_mut();
        let Some(web) = web.as_mut() else {
            return;
        };
        if web.sent {
            return;
        }
        web.sent = true;
        web.status = status;
        web.headers.extend(headers);
        web.body = body;
    });
}

/// A response whose body is `stream`, read by the host as the client takes it. One already sent is left alone.
pub(crate) fn send_stream(status: u16, headers: Vec<(String, String)>, stream: ResponseStream) {
    WEB.with(|slot| {
        let mut web = slot.borrow_mut();
        let Some(web) = web.as_mut() else {
            return;
        };
        if web.sent {
            return;
        }
        web.sent = true;
        web.status = status;
        web.headers.extend(headers);
        web.body.clear();
        web.stream = Some(stream);
    });
}

/// Move the body stream out of the slot, when the response has one.
pub(crate) fn take_stream() -> Option<ResponseStream> {
    WEB.with(|slot| slot.borrow_mut().as_mut().and_then(|web| web.stream.take()))
}

fn send_bytes(status: u16, content_type: &str, body: Vec<u8>) {
    WEB.with(|slot| {
        let mut web = slot.borrow_mut();
        let Some(web) = web.as_mut() else {
            return;
        };
        if web.sent {
            return;
        }
        web.sent = true;
        web.status = status;
        web.headers.push(("Content-Type".to_string(), content_type.to_string()));
        web.body = body;
    });
}

/// The status the response has so far.
pub(crate) fn output_status() -> u16 {
    WEB.with(|slot| slot.borrow().as_ref().map_or(200, |web| web.status))
}

/// Whether a response was already written for this request.
/// The length of the response body so far. None with no web slot.
pub(crate) fn output_body_len() -> Option<usize> {
    WEB.with(|slot| slot.borrow().as_ref().map(|web| web.body.len()))
}

pub(crate) fn output_sent() -> bool {
    WEB.with(|slot| slot.borrow().as_ref().is_some_and(|web| web.sent))
}

/// Forget what was written, so a response can be sent after a body was taken for another use.
pub(crate) fn clear_output() {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.sent = false;
            web.writing = false;
            web.status = 200;
            web.headers.clear();
            web.body.clear();
            web.stream = None;
        }
    });
}

/// What a request had written, set aside while something else uses the slot's output.
pub(crate) struct OutputStash((u16, Vec<(String, String)>, Vec<u8>, bool), Option<ResponseStream>);

/// Take what was written so far and leave the slot empty, for output that is not the response.
pub(crate) fn stash_output() -> OutputStash {
    let stream = take_stream();
    let taken = take_output();
    clear_output();
    OutputStash(taken, stream)
}

/// Put back what [`stash_output`] took, dropping anything written since.
pub(crate) fn restore_output(stash: OutputStash) {
    let (status, headers, body, sent) = stash.0;
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.status = status;
            web.headers = headers;
            web.body = body;
            web.sent = sent;
            web.stream = stash.1;
        }
    });
}

/// Move the status, headers, body, and sent flag out of the slot.
pub(crate) fn take_output() -> (u16, Vec<(String, String)>, Vec<u8>, bool) {
    WEB.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(|web| {
                (
                    web.status,
                    std::mem::take(&mut web.headers),
                    std::mem::take(&mut web.body),
                    web.sent,
                )
            })
            .unwrap_or((200, Vec::new(), Vec::new(), false))
    })
}

fn message_errors() -> Vec<String> {
    // Java's `errorsString`: a validation error written with its field, form and service.
    ExecutionContext::with_current(|ec| ec.message().errors_text()).unwrap_or_default()
}

/// Parsed inputs for one request. A JSON list stays beside the parameter map.
pub(crate) struct ParsedBody {
    pub parameters: BTreeMap<String, Value>,
    pub json_list: Option<Vec<Value>>,
    /// Uploaded files by field name: one [`multipart::UploadedFile`] or a list of them, and `_fileUploadList`.
    pub multipart: BTreeMap<String, Value>,
}

/// Body larger than its limit, or a multipart body that cannot be read or holds a program.
pub(crate) enum BodyError {
    TooLarge,
    Multipart(String),
    /// A file part that starts like a program, when the web app does not allow one.
    Executable(String),
}

/// The largest request body: `web-protection` `requests` `max-body` (`webapp_max_body`, 10 MiB by default), and
/// for a `multipart/form-data` body `max-upload` (`webapp_max_upload`). The conf of the current context's factory,
/// or the properties with none.
pub fn body_limit(request: &WebRequest) -> usize {
    ExecutionContext::with_current(|ec| body_limit_for(Some(ec.factory().as_ref()), request))
        .unwrap_or_else(|| body_limit_for(None, request))
}

/// [`body_limit`] by the conf of `factory`.
pub fn body_limit_for(factory: Option<&crate::context::ExecutionContextFactory>, request: &WebRequest) -> usize {
    if is_multipart(request) {
        parse_size(&protection_setting(
            factory,
            "requests",
            "max-upload",
            "webapp_max_upload",
        ))
        .unwrap_or(DEFAULT_MAX_UPLOAD)
    } else {
        // a servlet that takes larger bodies says so (`max-body`), as a search proxy does for a bulk load
        if let Some(max) = factory.and_then(|factory| factory.webapp_scripts().max_body_for(&request.path)) {
            return max;
        }
        parse_size(&protection_setting(factory, "requests", "max-body", "webapp_max_body")).unwrap_or(DEFAULT_MAX_BODY)
    }
}

/// The `webroot` webapp's `web-protection` setting: the attribute `name` of its child `child` (empty for
/// `web-protection` itself), or the property `property` when the conf leaves it out or it expands to nothing.
pub fn protection_setting(
    factory: Option<&crate::context::ExecutionContextFactory>,
    child: &str,
    name: &str,
    property: &str,
) -> String {
    let configured = factory
        .map(|factory| factory.web_protection_setting("webroot", child, name))
        .unwrap_or_default();
    if !configured.is_empty() || property.is_empty() {
        return configured;
    }
    moqui_lib::env::get_prop_or_env(property).trim().to_string()
}

pub fn is_multipart(request: &WebRequest) -> bool {
    request
        .header("content-type")
        .unwrap_or("")
        .trim_start()
        .get(..10)
        .is_some_and(|start| start.eq_ignore_ascii_case("multipart/"))
}

const DEFAULT_MAX_UPLOAD: usize = 2 * 1024 * 1024 * 1024;

/// `webapp_max_upload`: bytes, or a number with a `k`, `m` or `g` suffix. 2 GiB when unset or not a size.
pub fn max_upload() -> usize {
    parse_size(&moqui_lib::env::get_prop_or_env("webapp_max_upload")).unwrap_or(DEFAULT_MAX_UPLOAD)
}

/// Bytes, or a number with a `k`, `m` or `g` suffix. `None` when `text` is not a size.
pub fn parse_size(text: &str) -> Option<usize> {
    let text = text.trim().to_ascii_lowercase();
    let (digits, unit) = match text.chars().last()? {
        'k' => (&text[..text.len() - 1], 1024),
        'm' => (&text[..text.len() - 1], 1024 * 1024),
        'g' => (&text[..text.len() - 1], 1024 * 1024 * 1024),
        _ => (text.as_str(), 1),
    };
    digits
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_mul(unit))
        .filter(|size| *size > 0)
}

fn upload_executable_allowed() -> bool {
    webroot_setting(None, "upload-executable-allow", "webapp_upload_executable_allow") == "true"
}

/// The `webroot` webapp's attribute `name` (Java's `WebappInfo`), or the property `property` when the conf leaves it out
/// or it expands to nothing. With no factory given, the current context's, else only the property.
fn webroot_setting(factory: Option<&crate::context::ExecutionContextFactory>, name: &str, property: &str) -> String {
    let configured = match factory {
        Some(factory) => factory.webapp_setting("webroot", name),
        None => ExecutionContext::with_current(|ec| ec.factory().webapp_setting("webroot", name)).unwrap_or_default(),
    };
    if !configured.is_empty() {
        return configured;
    }
    moqui_lib::env::get_prop_or_env(property).trim().to_string()
}

/// Query, then a form body. A JSON object fills a name only when those did not already set it.
/// A multipart text field is not added onto a name the query already has. A JSON list is not merged
/// into the map. A repeated name inside the query, or inside the form body, stays a list.
pub(crate) fn parse_body(request: &WebRequest) -> Result<ParsedBody, BodyError> {
    if request.body.len() > body_limit(request) {
        return Err(BodyError::TooLarge);
    }
    let content_type = request.header("content-type").unwrap_or("").to_ascii_lowercase();
    let media = content_type.split(';').next().unwrap_or("").trim();
    let mut pairs = request.query.clone();
    let mut json_list = None;
    let mut multipart_files = BTreeMap::new();
    if media.starts_with("multipart/") {
        let parsed = match &request.multipart {
            Some(parsed) => multipart::Parsed::clone(parsed),
            None => multipart::parse(request.header("content-type").unwrap_or(""), &request.body)
                .map_err(|err| BodyError::Multipart(err.to_string()))?,
        };
        // The query wins. A multipart text field of a name the query already set is left out,
        // so the parameter stays the query value and does not become a list of both.
        let query_names: HashSet<&str> = request.query.iter().map(|(name, _)| name.as_str()).collect();
        for (name, value) in parsed.fields {
            if !query_names.contains(name.as_str()) {
                pairs.push((name, value));
            }
        }
        if !upload_executable_allowed() {
            if let Some(file) = parsed.files.iter().find(|file| file.is_executable()) {
                return Err(BodyError::Executable(file.file_name.clone()));
            }
        }
        let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut all = Vec::new();
        for file in parsed.files {
            let name = file.field_name.clone();
            let value = file.into_value();
            all.push(value.clone());
            grouped.entry(name).or_default().push(value);
        }
        for (name, mut values) in grouped {
            multipart_files.insert(
                name,
                if values.len() == 1 {
                    values.remove(0)
                } else {
                    Value::List(values)
                },
            );
        }
        multipart_files.insert("_fileUploadList".to_string(), Value::List(all));
        return Ok(ParsedBody {
            parameters: pairs_to_parameters(pairs),
            json_list,
            multipart: multipart_files,
        });
    }
    if request.body.is_empty() {
        return Ok(ParsedBody {
            parameters: pairs_to_parameters(pairs),
            json_list,
            multipart: multipart_files,
        });
    }
    // Only a form type is a form, as the servlet parses one; a body with no type is no parameter at all.
    if media == "application/x-www-form-urlencoded" {
        pairs.extend(form_pairs(&String::from_utf8_lossy(&request.body)));
        return Ok(ParsedBody {
            parameters: pairs_to_parameters(pairs),
            json_list,
            multipart: multipart_files,
        });
    }
    let mut parameters = pairs_to_parameters(pairs);
    // As Java's WebFacadeImpl: a body of a text type (`ResourceReference.isTextContentType`) that is not a form is
    // `_requestBodyText`, and a JSON one is read as well. An object's fields are parameters, an array is
    // `_requestBodyJsonList`, and JSON that does not parse is `_requestBodyJsonParseError` for the screen or service to
    // answer, not a refused request. A binary body is no parameter.
    if !moqui_lib::resource::is_text_content_type(media) {
        return Ok(ParsedBody {
            parameters,
            json_list,
            multipart: multipart_files,
        });
    }
    let text = String::from_utf8_lossy(&request.body).into_owned();
    parameters.insert("_requestBodyText".to_string(), Value::String(text.clone()));
    if media == "application/json" || media == "text/json" || media.ends_with("+json") {
        match crate::service::parse_json(&text) {
            Ok(Value::Map(map)) => {
                for (key, value) in map {
                    // A name the query already set stays. This body is JSON, not a form.
                    parameters.entry(key).or_insert(value);
                }
            }
            Ok(Value::List(items)) => {
                // The REST batch form takes a list of objects.
                if items.iter().all(|item| matches!(item, Value::Map(_))) {
                    json_list = Some(items.clone());
                }
                parameters.insert("_requestBodyJsonList".to_string(), Value::List(items));
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!("Error parsing HTTP request body JSON: {}", err.message());
                parameters.insert("_requestBodyJsonParseError".to_string(), Value::string(err.message()));
            }
        }
    }
    Ok(ParsedBody {
        parameters,
        json_list,
        multipart: multipart_files,
    })
}

/// One value stays a string. A name that repeats becomes a list in request order.
/// A lone non-breaking space is Java's "no parameter" value (`WebUtilities.simplifyRequestParameters`): the html
/// remote options post it for an empty option, and the name is null, which still hides a JSON or saved value.
const NO_PARAMETER: &str = "\u{a0}";

fn pairs_to_parameters(pairs: Vec<(String, String)>) -> BTreeMap<String, Value> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in pairs {
        grouped.entry(key).or_default().push(value);
    }
    grouped
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 && values[0] == NO_PARAMETER {
                Value::Null
            } else if values.len() == 1 {
                Value::string(values.remove(0))
            } else {
                Value::List(values.into_iter().map(Value::string).collect())
            };
            (key, value)
        })
        .collect()
}

/// Multipart files fill names the query and the form body did not already set.
pub(crate) fn overlay_multipart(parameters: &mut BTreeMap<String, Value>, files: &BTreeMap<String, Value>) {
    for (name, value) in files {
        parameters.entry(name.clone()).or_insert_with(|| value.clone());
    }
}

/// Split `a=1&b=2` into decoded pairs.
pub fn form_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for part in text.split('&') {
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        pairs.push((percent_decode(key), percent_decode(value)));
    }
    pairs
}

/// Decode `+` and `%XX`. A malformed escape is kept as written.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() && hex_pair(bytes[index + 1], bytes[index + 2]).is_some() => {
                out.push(hex_pair(bytes[index + 1], bytes[index + 2]).unwrap_or(b'%'));
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    let high = char::from(high).to_digit(16)?;
    let low = char::from(low).to_digit(16)?;
    u8::try_from(high * 16 + low).ok()
}

/// `Authorization: Basic` user and password. The split is the first colon.
pub(crate) fn basic_credentials(header: &str) -> Option<(String, String)> {
    let rest = header.trim();
    let rest = rest
        .strip_prefix("Basic ")
        .or_else(|| rest.strip_prefix("basic "))
        .or_else(|| rest.strip_prefix("BASIC "))?;
    let bytes = STANDARD.decode(rest.trim()).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_string(), password.to_string()))
}

/// Cookie value for `name` from a `Cookie` header.
pub fn cookie_value(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let Some((key, value)) = part.trim().split_once('=') else {
            continue;
        };
        if key.trim() == name {
            return Some(value.trim().to_string());
        }
    }
    None
}

pub(crate) fn session_cookie_of(request: &WebRequest) -> Option<String> {
    if let Some(value) = request.session_cookie.as_ref().filter(|text| !text.is_empty()) {
        return Some(value.clone());
    }
    request
        .header("cookie")
        .and_then(|header| cookie_value(header, crate::user::SESSION_COOKIE_NAME))
}

pub(crate) fn visitor_cookie_of(request: &WebRequest) -> Option<String> {
    if let Some(value) = request.visitor_cookie.as_ref().filter(|text| !text.is_empty()) {
        return Some(value.clone());
    }
    request
        .header("cookie")
        .and_then(|header| cookie_value(header, crate::user::VISITOR_COOKIE_NAME))
}

/// The client's address: what the proxy in front names in `client-ip-header`, when that proxy is believed
/// ([`address::ClientAddressing`]), else the socket peer.
pub fn client_ip(request: &WebRequest) -> String {
    let addressing = client_addressing();
    if addressing.header().is_empty() {
        return request.peer_ip.clone();
    }
    let Ok(peer) = request.peer_ip.trim().parse::<std::net::IpAddr>() else {
        return request.peer_ip.clone();
    };
    let client = addressing.resolve(peer, request.header(addressing.header()));
    if client == peer.to_canonical() {
        request.peer_ip.clone()
    } else {
        client.to_string()
    }
}

/// The current factory's [`address::ClientAddressing`], read once; from the properties with no current context.
pub fn client_addressing() -> std::sync::Arc<address::ClientAddressing> {
    ExecutionContext::with_current(|ec| ec.factory().extension(address::ClientAddressing::of_factory))
        .unwrap_or_else(|| std::sync::Arc::new(address::ClientAddressing::of_properties()))
}

/// `WebUtilities.ipMatches`: comma separated patterns, each four dot separated parts of a number,
/// `*`, or a `low-high` range; an IPv6 pattern must equal the address.
pub(crate) fn ip_matches(patterns: &str, address: &str) -> bool {
    let canon = canonical_ip(address);
    if canon.is_empty() {
        return false;
    }
    patterns.split(',').map(str::trim).filter(|raw| !raw.is_empty()).any(|raw| {
        if raw == "*" || raw == "*.*.*.*" {
            return true;
        }
        let pattern = canonical_ip(raw);
        if pattern.contains(':') || canon.contains(':') {
            return pattern.eq_ignore_ascii_case(&canon);
        }
        let (pattern_parts, address_parts): (Vec<&str>, Vec<&str>) = (pattern.split('.').collect(), canon.split('.').collect());
        pattern_parts.len() == 4
            && address_parts.len() == 4
            && pattern_parts.iter().zip(&address_parts).all(|(part, value)| {
                let Ok(number) = value.parse::<u32>() else {
                    return false;
                };
                if *part == "*" {
                    return true;
                }
                match part.split_once('-') {
                    Some((low, high)) => matches!((low.parse::<u32>(), high.parse::<u32>()), (Ok(low), Ok(high)) if number >= low && number <= high),
                    None => part.parse::<u32>().is_ok_and(|expected| expected == number),
                }
            })
    })
}

fn canonical_ip(text: &str) -> String {
    let mut text = text.trim().trim_start_matches('[').trim_end_matches(']').to_string();
    if let Some(at) = text.find('%') {
        text.truncate(at);
    }
    if let Some(rest) = text.to_ascii_lowercase().strip_prefix("::ffff:") {
        if rest.contains('.') {
            return rest.to_string();
        }
    }
    text
}

pub(crate) fn require_session_token() -> bool {
    webroot_setting(None, "require-session-token", "webapp_require_session_token") != "false"
}

fn handle_cors(factory: Option<&crate::context::ExecutionContextFactory>) -> bool {
    webroot_setting(factory, "handle-cors", "webapp_handle_cors") != "false"
}

fn allow_origins(factory: Option<&crate::context::ExecutionContextFactory>) -> Vec<String> {
    webroot_setting(factory, "allow-origins", "webapp_allow_origins")
        .split(',')
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .collect()
}

/// [`origin_allowed_by`] with the current context's conf.
pub fn origin_allowed_for(origin: Option<&str>, host: Option<&str>) -> bool {
    origin_allowed_by(None, origin, host)
}

/// Java's `WebUtilities.webSocketOriginAllowed`, which CORS follows too: no Origin, `*`, the request's own host, an
/// allowed origin or domain, or a subdomain of an allowed domain on a dot boundary (`moqui.org` allows `www.moqui.org`,
/// not `foo-moqui.org`). Case-insensitive. The allowed origins are the `webroot` webapp's `allow-origins`.
pub fn origin_allowed_by(
    factory: Option<&crate::context::ExecutionContextFactory>,
    origin: Option<&str>,
    host: Option<&str>,
) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|text| !text.is_empty()) else {
        return true;
    };
    let origin = origin.to_ascii_lowercase();
    let allow: Vec<String> = allow_origins(factory)
        .iter()
        .map(|text| text.to_ascii_lowercase())
        .collect();
    if allow.iter().any(|entry| entry == "*") {
        return true;
    }
    let origin_domain = origin_host(&origin);
    if allow.iter().any(|entry| *entry == origin || *entry == origin_domain) {
        return true;
    }
    if host.is_some_and(|host| !origin_domain.is_empty() && origin_host(host) == origin_domain) {
        return true;
    }
    allow.iter().any(|entry| {
        let domain = origin_host(entry);
        !domain.is_empty()
            && (origin_domain == domain || (domain.contains('.') && origin_domain.ends_with(&format!(".{domain}"))))
    })
}

/// The host of an origin or `Host` value, without scheme, path or port.
fn origin_host(text: &str) -> String {
    let text = text.trim().to_ascii_lowercase();
    let text = text.split_once("://").map_or(text.as_str(), |(_, rest)| rest);
    let text = text.split('/').next().unwrap_or("");
    if let Some(bracketed) = text.strip_prefix('[') {
        return format!("[{}]", bracketed.split(']').next().unwrap_or(""));
    }
    text.split(':').next().unwrap_or("").to_string()
}

/// The `Origin` of a request CORS refuses, as `MoquiServlet.handleCors`: CORS is on, the request names an origin and
/// `origin_allowed_by` does not allow it. The caller answers 401 before the request runs.
///
/// The request's own site is never refused, also behind a proxy that rewrites `Host`: the origin may be the
/// `X-Forwarded-Host` of a proxy whose forwarding headers are believed, as Jetty's `ForwardedRequestCustomizer` made it
/// Java's server name, or the conf webapp's `http-host` or `https-host`.
pub(crate) fn refused_origin<'a>(
    factory: &crate::context::ExecutionContextFactory,
    request: &'a WebRequest,
) -> Option<&'a str> {
    if !handle_cors(Some(factory)) {
        return None;
    }
    let origin = request.header("origin").filter(|origin| !origin.trim().is_empty())?;
    if origin_allowed_by(Some(factory), Some(origin), request.header("host")) {
        return None;
    }
    let origin_domain = origin_host(origin);
    let forwarded = request
        .header("x-forwarded-host")
        .and_then(|value| value.split(',').next())
        .filter(|_| {
            request
                .peer_ip
                .trim()
                .parse::<std::net::IpAddr>()
                .is_ok_and(|peer| factory.extension(address::ClientAddressing::of_factory).believes(peer))
        });
    let configured = ["http-host", "https-host"].map(|name| factory.webapp_setting("webroot", name));
    let own = forwarded
        .into_iter()
        .chain(configured.iter().map(String::as_str))
        .any(|host| !host.trim().is_empty() && origin_host(host) == origin_domain);
    (!own).then_some(origin)
}

/// The conf's `screen-render` headers on every response, as the baseline under what the response set itself, and
/// `screen-secure` on https; then CORS when the origin is allowed.
pub fn finish_response(
    factory: &crate::context::ExecutionContextFactory,
    request: &WebRequest,
    mut response: WebResponse,
    preflight: bool,
) -> WebResponse {
    apply_conf_headers(Some(factory), "screen-render", &mut response.headers, true);
    // HSTS holds for every port of a host (RFC 6797), so on localhost a trusted dev certificate would send the plain
    // dev listener and any other local server to https for a year.
    if request.https && !is_localhost(request.header("host").unwrap_or("")) {
        apply_conf_headers(Some(factory), "screen-secure", &mut response.headers, true);
    }
    if handle_cors(Some(factory)) {
        if let Some(origin) = request.header("origin").filter(|text| !text.is_empty()) {
            if origin_allowed_by(Some(factory), Some(origin), request.header("host")) {
                apply_cors(factory, &mut response.headers, origin, preflight);
            }
        }
    }
    response
}

/// Java's `WebappInfo.addHeaders`: the conf's `response-header` entries for `kind` (`screen-render`, `cors-actual`). An
/// entry replaces the header, or adds a value when it says `add="true"`. With `only_missing`, a header the response
/// already has is left as it is. With no factory given, the current context's conf, else the shipped entries.
pub(crate) fn apply_conf_headers(
    factory: Option<&crate::context::ExecutionContextFactory>,
    kind: &str,
    headers: &mut Vec<(String, String)>,
    only_missing: bool,
) {
    let entries = match factory {
        Some(factory) => factory.response_headers("webroot", kind),
        None => ExecutionContext::with_current(|ec| ec.factory().response_headers("webroot", kind))
            .unwrap_or_else(|| crate::context::ExecutionContextFactory::default_response_headers(kind)),
    };
    for (name, value, add) in entries {
        if only_missing && headers.iter().any(|(key, _)| key.eq_ignore_ascii_case(&name)) {
            continue;
        }
        if !add {
            headers.retain(|(key, _)| !key.eq_ignore_ascii_case(&name));
        }
        headers.push((name, value));
    }
}

/// The conf's first `response-header` value of `name` for `kind`, such as a `Cache-Control`.
pub(crate) fn conf_header(kind: &str, name: &str) -> Option<String> {
    let mut headers = Vec::new();
    apply_conf_headers(None, kind, &mut headers, false);
    headers
        .into_iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

/// `localhost` or a name under it, with or without a port.
fn is_localhost(host: &str) -> bool {
    let host = host_without_port(host).to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// `example.com:8080` is `example.com`, and `[::1]:8080` is `[::1]`.
pub(crate) fn host_without_port(host: &str) -> &str {
    if host.starts_with('[') {
        return host.find(']').map_or(host, |end| &host[..=end]);
    }
    host.split(':').next().unwrap_or(host)
}

/// The allowed origin, then the conf's `cors-preflight` or `cors-actual` headers, as `MoquiServlet.handleCors`.
fn apply_cors(
    factory: &crate::context::ExecutionContextFactory,
    headers: &mut Vec<(String, String)>,
    origin: &str,
    preflight: bool,
) {
    headers.push(("Access-Control-Allow-Origin".to_string(), origin.to_string()));
    let kind = if preflight { "cors-preflight" } else { "cors-actual" };
    apply_conf_headers(Some(factory), kind, headers, false);
}

/// Names a response never carries: `WebUtilities.stripCredentialParameters`, compared without case.
pub(crate) const CREDENTIAL_PARAMETER_NAMES: [&str; 13] = [
    "password",
    "oldpassword",
    "newpassword",
    "newpasswordverify",
    "currentpassword",
    "resetpassword",
    "authpassword",
    "api_key",
    "login_key",
    "passwordsalt",
    "passwordhashtype",
    "passwordbase64",
    "passwordverify",
];

/// Remove credential parameters from a JSON value, at any depth. Java strips the top level of a parameter map; a
/// `UserAccount` value in a screen's data holds the same names as columns, so the nested maps lose them too.
pub(crate) fn strip_credential_parameters(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|key, _| !CREDENTIAL_PARAMETER_NAMES.contains(&key.to_ascii_lowercase().as_str()));
            map.values_mut().for_each(strip_credential_parameters);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_credential_parameters),
        _ => {}
    }
}

/// The JSON error response of `err`: its status and client text, and a `Retry-After` when the error says when the
/// client may come back (a tarpit), as Java adds one to every 429.
pub(crate) fn json_error_of(err: &Error) -> WebResponse {
    let status = status_for(err);
    let mut response = json_error(status, &error_text(status, err));
    add_retry_after(&mut response.headers, err);
    response
}

/// [`json_error_of`] into the thread's web slot.
pub(crate) fn send_json_error_of(err: &Error) {
    let status = status_for(err);
    send_json_error(status, &error_text(status, err));
    if let Some(seconds) = err.retry_after() {
        add_header("Retry-After", seconds.to_string());
    }
}

/// A `Retry-After` of `err`'s seconds, when it has some.
pub(crate) fn add_retry_after(headers: &mut Vec<(String, String)>, err: &Error) {
    if let Some(seconds) = err.retry_after() {
        headers.push(("Retry-After".to_string(), seconds.to_string()));
    }
}

pub(crate) fn json_error(status: u16, message: &str) -> WebResponse {
    let mut map = IndexMap::new();
    map.insert("errorCode".to_string(), Value::I64(i64::from(status)));
    map.insert("errors".to_string(), Value::string(message));
    let mut response = WebResponse::new(status, query_rest::encode_json(&Value::Map(map)).into_bytes());
    response
        .headers
        .push(("Content-Type".to_string(), "application/json".to_string()));
    response
}

/// Whether `err` or an error in its source chain is [`ErrorKind::Unavailable`]: the database could not be reached, or
/// the server cannot take the work now.
pub(crate) fn is_unavailable(err: &Error) -> bool {
    if err.kind() == ErrorKind::Unavailable {
        return true;
    }
    let mut source = std::error::Error::source(err);
    while let Some(found) = source {
        if found
            .downcast_ref::<Error>()
            .is_some_and(|context| context.kind() == ErrorKind::Unavailable)
        {
            return true;
        }
        source = found.source();
    }
    false
}

pub(crate) fn early_error(
    factory: &crate::context::ExecutionContextFactory,
    request: &WebRequest,
    status: u16,
    message: &str,
) -> WebResponse {
    finish_response(factory, request, json_error(status, message), false)
}

/// The HTTP status of an error, by its [`ErrorKind`]. A general error is a 500 whatever its message says, unless it
/// was caused by one that is unavailable (no database connection), which is the whole request's: a 503.
pub(crate) fn status_for(err: &Error) -> u16 {
    match err.kind() {
        ErrorKind::General | ErrorKind::RollbackOnly if is_unavailable(err) => 503,
        ErrorKind::Validation | ErrorKind::BadRequest => 400,
        ErrorKind::AuthenticationRequired => 401,
        ErrorKind::Unauthorized => 403,
        ErrorKind::NotFound => 404,
        ErrorKind::MethodNotAllowed => 405,
        ErrorKind::Tarpit => 429,
        ErrorKind::Unavailable => 503,
        ErrorKind::General | ErrorKind::RollbackOnly => 500,
    }
}

#[cfg(test)]
thread_local! {
    /// A test's `instance_purpose` for this thread only, so it does not change other tests' instance.
    pub(crate) static TEST_PRODUCTION: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether this instance runs with `instance_purpose` production.
pub(crate) fn is_production() -> bool {
    #[cfg(test)]
    if let Some(production) = TEST_PRODUCTION.with(std::cell::Cell::get) {
        return production;
    }
    moqui_lib::env::get_prop_or_env("instance_purpose") == "production"
}

/// The text a client gets for `err` sent with `status`. In production a 500's detail, which can hold SQL, file paths,
/// or script source, goes to the log under an error id, and the client gets only the id. Other statuses and other
/// instances get the message; a 500 is logged there too, as Java's `MoquiServlet` logs every internal error. The
/// message is its text only ([`crate::context::client_text`]): a script's stack and the Rust source location are in the
/// log, not the answer.
pub(crate) fn error_text(status: u16, err: &Error) -> String {
    if status != 500 {
        crate::context::client_text(err.message())
    } else if is_production() {
        format!("internal error, id {}", log_error_id(&err.diagnostic()))
    } else {
        tracing::error!("Internal error processing request: {}", err.diagnostic());
        crate::context::client_text(err.message())
    }
}

/// Log `detail` at error level under a new error id, in the form of the host's, and return the id.
pub(crate) fn log_error_id(detail: &dyn std::fmt::Display) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "{:x}-{:x}",
        jiff::Timestamp::now().as_millisecond(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    tracing::error!("request failed, error id {id}: {detail}");
    id
}

pub(crate) fn record_login_error(message: &str) {
    ExecutionContext::with_current(|ec| ec.message().add_error(message.to_string()));
}

#[cfg(test)]
mod tests {
    use super::{
        cookie_value, form_pairs, host_without_port, is_localhost, origin_allowed_for, pairs_to_parameters,
        percent_decode,
    };
    use moqui_lib::object::Value;

    #[test]
    fn send_error_follows_accept() {
        let sent = |accept: &str| {
            let _web = super::install(std::collections::BTreeMap::new());
            let mut request = super::WebRequest::new("GET", "/apps/ship");
            if !accept.is_empty() {
                request.set_header("accept", accept);
            }
            super::set_request_info(&request);
            super::send_error(404, "No labelUrl", None);
            let (status, headers, body, sent) = super::take_output();
            assert!(sent);
            assert_eq!(status, 404);
            (
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default(),
                String::from_utf8(body).unwrap(),
            )
        };
        let (html_type, html) = sent("text/html");
        assert_eq!(html_type, "text/html;charset=UTF-8");
        assert_eq!(
            html,
            "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\"/><title>Error 404 Not Found</title></head><body>\n<h2>Error 404 Not Found</h2>\n<p>Problem accessing /apps/ship</p>\n<p>Reason: No labelUrl</p>\n</body></html>\n"
        );
        let (json_type, json) = sent("application/json");
        assert_eq!(json_type, "application/json;charset=UTF-8");
        assert_eq!(
            json,
            "{'message':'No labelUrl','errorName':'Not Found','error':404,'path':'/apps/ship'}"
        );
        let (plain_type, plain) = sent("");
        assert_eq!(plain_type, "text/plain;charset=UTF-8");
        assert_eq!(plain, "404 No labelUrl /apps/ship");

        let _web = super::install(std::collections::BTreeMap::new());
        let mut request = super::WebRequest::new("GET", "/a<b");
        request.set_header("accept", "text/html, application/json");
        super::set_request_info(&request);
        super::send_error(404, "say \"hi\" <tag>", None);
        let (_, _, body, _) = super::take_output();
        let html = String::from_utf8(body).unwrap();
        assert!(html.contains("<p>Problem accessing /a&lt;b</p>"), "{html}");
        assert!(html.contains("<p>Reason: say &quot;hi&quot; &lt;tag&gt;</p>"), "{html}");

        let _web = super::install(std::collections::BTreeMap::new());
        let mut request = super::WebRequest::new("GET", "/apps/ship");
        request.set_header("accept", "application/json");
        super::set_request_info(&request);
        super::send_error(500, "", Some("boom\n\"x\""));
        let (_, _, body, _) = super::take_output();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "{'message':'boom\\n\\\"x\\\"','errorName':'Internal Server Error','error':500,'path':'/apps/ship'}"
        );
    }

    #[test]
    fn request_details_leave_out_credential_parameters() {
        let parameters = [("orderId", "O1"), ("password", "hunter2"), ("api_key", "k-1")]
            .into_iter()
            .map(|(name, value)| (name.to_string(), Value::string(value)))
            .collect();
        let _web = super::install(parameters);
        let details = super::request_details();
        assert!(details.contains("Parameter: orderId = O1"), "{details}");
        assert!(!details.contains("hunter2") && !details.contains("k-1"), "{details}");
    }

    /// The query wins over a JSON field and over a multipart field. A repeated name in the query,
    /// or in the form body, stays a list.
    #[test]
    fn the_query_wins_over_json_and_multipart() {
        let body = |request: &super::WebRequest| match super::parse_body(request) {
            Ok(parsed) => parsed,
            Err(_) => panic!("body refused"),
        };
        let mut json = super::WebRequest::new("POST", "/x");
        json.query.push(("orderId".to_string(), "1".to_string()));
        json.set_header("content-type", "application/json");
        json.body = br#"{"orderId":2,"note":"n"}"#.to_vec();
        let parsed = body(&json);
        assert_eq!(parsed.parameters.get("orderId"), Some(&Value::string("1")));
        assert_eq!(parsed.parameters.get("note"), Some(&Value::string("n")));

        let mut form = super::WebRequest::new("POST", "/x");
        form.query.push(("a".to_string(), "1".to_string()));
        form.query.push(("a".to_string(), "2".to_string()));
        form.set_header("content-type", "application/x-www-form-urlencoded");
        form.body = b"a=3&b=4".to_vec();
        let parsed = body(&form);
        assert_eq!(
            parsed.parameters.get("a"),
            Some(&Value::List(vec![
                Value::string("1"),
                Value::string("2"),
                Value::string("3")
            ]))
        );
        assert_eq!(parsed.parameters.get("b"), Some(&Value::string("4")));

        let mut upload = super::WebRequest::new("POST", "/x");
        upload.query.push(("name".to_string(), "x".to_string()));
        upload.query.push(("q".to_string(), "1".to_string()));
        upload.query.push(("q".to_string(), "2".to_string()));
        upload.set_header("content-type", "multipart/form-data; boundary=XyZ");
        upload.body = b"\
--XyZ\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nfrom-body\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\ny\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"q\"\r\n\r\nfrom-body\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"name\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nxyz\r\n\
--XyZ\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"b.bin\"\r\nContent-Type: application/octet-stream\r\n\r\nxyz\r\n\
--XyZ--\r\n"
            .to_vec();
        let parsed = body(&upload);
        assert_eq!(parsed.parameters.get("name"), Some(&Value::string("x")));
        assert_eq!(parsed.parameters.get("other"), Some(&Value::string("y")));
        assert_eq!(
            parsed.parameters.get("q"),
            Some(&Value::List(vec![Value::string("1"), Value::string("2")]))
        );
        let mut parameters = parsed.parameters.clone();
        super::overlay_multipart(&mut parameters, &parsed.multipart);
        assert_eq!(parameters.get("name"), Some(&Value::string("x")));
        assert!(
            parameters
                .get("doc")
                .is_some_and(|value| matches!(value, Value::Object(_))),
            "{parameters:?}"
        );
    }

    #[test]
    fn a_json_body_is_its_fields_a_list_or_a_parse_error_and_always_the_body_text() {
        let body = |text: &str| {
            let mut request = super::WebRequest::new("POST", "/x");
            request.set_header("content-type", "application/json");
            request.body = text.as_bytes().to_vec();
            match super::parse_body(&request) {
                Ok(parsed) => parsed,
                Err(_) => panic!("body refused"),
            }
        };
        let object = body(r#"{"a": 1}"#);
        assert_eq!(object.parameters.get("a"), Some(&Value::I64(1)));
        assert_eq!(
            object.parameters.get("_requestBodyText"),
            Some(&Value::string(r#"{"a": 1}"#))
        );
        let list = body("[1, 2]");
        assert!(matches!(list.parameters.get("_requestBodyJsonList"), Some(Value::List(items)) if items.len() == 2));
        assert!(list.json_list.is_none());
        let broken = body("{not json");
        assert!(broken.parameters.contains_key("_requestBodyJsonParseError"));
    }

    #[test]
    fn an_origin_is_allowed_as_java_allows_it() {
        let _lock = crate::user::session::cookie_property_lock();
        moqui_lib::env::set_property("webapp_allow_origins", "moqui.org, https://app.example:8443");
        // No Origin (not a browser), the request's own host, an allowed origin, and a subdomain on a dot boundary.
        assert!(origin_allowed_for(None, Some("site.test")));
        assert!(origin_allowed_for(Some("https://site.test"), Some("site.test:8080")));
        assert!(origin_allowed_for(Some("https://app.example:8443"), Some("other")));
        assert!(origin_allowed_for(Some("https://www.MOQUI.org"), Some("other")));
        assert!(!origin_allowed_for(Some("https://foo-moqui.org"), Some("other")));
        assert!(!origin_allowed_for(Some("https://evil.test"), Some("site.test")));
        moqui_lib::env::clear_property("webapp_allow_origins");
    }

    #[test]
    fn localhost_hosts_get_no_hsts() {
        assert!(is_localhost("localhost:8443"));
        assert!(is_localhost("LocalHost"));
        assert!(is_localhost("app.localhost:8443"));
        assert!(!is_localhost("localhost.example.com"));
        assert!(!is_localhost("[::1]:8443"));
        assert!(!is_localhost(""));
        assert_eq!(host_without_port("[::1]:8443"), "[::1]");
        assert_eq!(host_without_port("example.com:8080"), "example.com");
    }

    #[test]
    fn a_repeated_name_becomes_a_list() {
        let map = pairs_to_parameters(form_pairs("a=1&b=x&a=2&a=3"));
        assert_eq!(map.get("b"), Some(&Value::string("x")));
        assert_eq!(
            map.get("a"),
            Some(&Value::List(vec![
                Value::string("1"),
                Value::string("2"),
                Value::string("3")
            ]))
        );
    }

    /// A lone non-breaking space is no parameter, as Java's `simplifyRequestParameters`; the name stays, null.
    #[test]
    fn a_lone_non_breaking_space_is_a_null_parameter() {
        let map = pairs_to_parameters(form_pairs("statusId=%C2%A0&note=%C2%A0x&a=%C2%A0&a=1"));
        assert_eq!(map.get("statusId"), Some(&Value::Null));
        assert_eq!(map.get("note"), Some(&Value::string("\u{a0}x")));
        assert_eq!(
            map.get("a"),
            Some(&Value::List(vec![Value::string("\u{a0}"), Value::string("1")]))
        );
    }

    /// Only a text body is `_requestBodyText`, and only a form type is a form, as Java's `WebFacadeImpl` reads them.
    #[test]
    fn a_binary_or_untyped_body_is_no_parameter() {
        let parse = |content_type: Option<&str>, body: &[u8]| {
            let mut request = super::WebRequest::new("POST", "/x");
            if let Some(content_type) = content_type {
                request.set_header("content-type", content_type);
            }
            request.body = body.to_vec();
            match super::parse_body(&request) {
                Ok(parsed) => parsed.parameters,
                Err(_) => panic!("body refused"),
            }
        };
        assert!(parse(Some("application/octet-stream"), b"a=1\xff").is_empty());
        assert!(parse(None, b"a=1").is_empty());
        let text = parse(Some("text/plain; charset=utf-8"), b"a=1");
        assert_eq!(text.get("_requestBodyText"), Some(&Value::string("a=1")));
        assert_eq!(text.get("a"), None);
        assert_eq!(
            parse(Some("application/x-www-form-urlencoded"), b"a=1").get("a"),
            Some(&Value::string("1"))
        );
    }

    /// Java's `getRequestUrl`: the host without a port of 80 or 443, and the query string as the client sent it.
    #[test]
    fn the_request_url_keeps_the_query_as_sent_and_drops_a_default_port() {
        let mut request = super::WebRequest::new("GET", "/apps/Find");
        request.set_header("host", "site.test:80");
        request.query = form_pairs("q=a%20b&n=%0A");
        assert_eq!(super::full_url(&request), "http://site.test/apps/Find?q=a+b&n=%0A");
        request.raw_query = Some("q=a%20b&n=%0A".to_string());
        request.set_header("host", "site.test:8080");
        assert_eq!(
            super::full_url(&request),
            "http://site.test:8080/apps/Find?q=a%20b&n=%0A"
        );
        request.https = true;
        request.set_header("host", "[::1]:443");
        assert_eq!(super::full_url(&request), "https://[::1]/apps/Find?q=a%20b&n=%0A");
    }

    #[test]
    fn percent_decode_keeps_bad_escapes_and_never_panics() {
        assert_eq!(percent_decode("a%20b+c"), "a b c");
        assert_eq!(percent_decode("%a\u{e9}"), "%a\u{e9}");
        assert_eq!(percent_decode("tail%"), "tail%");
        assert_eq!(percent_decode("%zz%41"), "%zzA");
    }

    #[test]
    fn cookie_value_skips_parts_without_an_equals_sign() {
        assert_eq!(
            cookie_value("junk; moqui.session=abc; x=y", "moqui.session").as_deref(),
            Some("abc")
        );
        assert_eq!(cookie_value("a=b", "moqui.session"), None);
        // Two Cookie headers as the host joins them.
        assert_eq!(
            cookie_value("a=1; moqui.session=abc; b=2", "moqui.session").as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn a_kind_keeps_its_status_through_a_transaction_and_a_message_picks_none() {
        use crate::context::{Error, ErrorKind};
        use crate::transaction::TransactionAttribute;

        let facade = crate::testing::open();
        let tx = facade.transactions_for("moqui.basic.Enumeration").unwrap();
        let err = tx
            .run(TransactionAttribute::UseOrBegin, None, || -> Result<(), _> {
                Err(Error::of_kind(ErrorKind::Tarpit, "User u has accessed AT_SERVICE x too many times").into())
            })
            .unwrap_err();
        let err = Error::from(err);
        assert_eq!(err.kind(), ErrorKind::Tarpit, "{err}");
        assert_eq!(super::status_for(&err), 429);
        // A tarpit's seconds come through a lib error as the response's Retry-After.
        let wrapped =
            moqui_lib::error::Error::from(Error::of_kind(ErrorKind::Tarpit, "too many times").with_retry_after(42));
        let response = super::json_error_of(&Error::from(wrapped));
        assert_eq!(response.status, 429);
        assert_eq!(response.header("Retry-After"), Some("42"));
        assert_eq!(super::json_error_of(&err).header("Retry-After"), None);
        // The kind is found under a lib error that names it as its source.
        let wrapped = moqui_lib::error::Error::new("outer").with_source(Error::of_kind(ErrorKind::Unavailable, "full"));
        assert_eq!(super::status_for(&Error::from(wrapped)), 503);
        // An unavailable database under a general error is still the request's 503.
        let beneath = Error::new("find failed").with_source(Error::of_kind(ErrorKind::Unavailable, "no connection"));
        assert_eq!(super::status_for(&beneath), 503);
        let not_found = Error::new("service failed").with_source(Error::of_kind(ErrorKind::NotFound, "no row"));
        assert_eq!(super::status_for(&not_found), 500);
        // A general error is a 500 whatever its words.
        for message in [
            "Operation x not supported",
            "User is not authorized",
            "No value found",
            "tarpit",
        ] {
            assert_eq!(super::status_for(&Error::new(message)), 500, "{message}");
        }
    }

    #[test]
    fn a_production_500_names_only_an_error_id() {
        use crate::context::{Error, ErrorKind};

        let err = Error::new("SELECT * FROM SECRET_TABLE failed");
        super::TEST_PRODUCTION.with(|production| production.set(Some(true)));
        let hidden = super::error_text(500, &err);
        let shown = super::error_text(400, &Error::of_kind(ErrorKind::BadRequest, "bad name"));
        // Not the process property: a test that boots the default conf leaves it production.
        super::TEST_PRODUCTION.with(|production| production.set(Some(false)));
        let not_production = super::error_text(500, &err);
        super::TEST_PRODUCTION.with(|production| production.set(None));
        assert!(hidden.starts_with("internal error, id "), "{hidden}");
        assert!(!hidden.contains("SECRET_TABLE"), "{hidden}");
        assert_eq!(shown, "bad name");
        assert_eq!(not_production, "SELECT * FROM SECRET_TABLE failed");
    }
}
