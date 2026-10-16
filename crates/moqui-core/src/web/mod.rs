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
use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use indexmap::IndexMap;
use moqui_lib::object::Value;

pub mod error_info;
pub mod multipart;

use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::entity::query_rest;

/// The cap on a request body that is not multipart, unless `webapp_max_body` says otherwise.
const DEFAULT_MAX_BODY: usize = 10 * 1024 * 1024;
const PREFLIGHT_HEADERS: &str = "Accept,Accept-Encoding,Content-Type,Content-Encoding,Origin,X-Requested-With,Access-Control-Request-Method,Access-Control-Request-Headers,api_key,login_key,X-HTTP-Method-Override,moquiSessionToken,SessionToken,X-CSRF-Token,Authorization,A2A-Version";
const EXPOSE_HEADERS: &str =
    "Access-Control-Allow-Origin,Access-Control-Allow-Credentials,X-CSRF-Token,moquiSessionToken";

thread_local! {
    static WEB: RefCell<Option<WebSlot>> = const { RefCell::new(None) };
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
    pub body: Vec<u8>,
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
            body: Vec::new(),
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

/// One HTTP response. Repeated names are separate headers, so `Set-Cookie` can appear twice.
#[derive(Clone, Debug)]
pub struct WebResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl WebResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
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
    /// [`client_ip`] of the request, for the login `ipAllowed` check.
    client_ip: String,
    /// The first locale of the request's `Accept-Language`, as Java's `request.getLocale()`.
    locale: Option<String>,
    /// Scheme, host, and context path that links start with. Empty for a path-only link.
    base_url: String,
    sent: bool,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
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
            client_ip: String::new(),
            locale: None,
            base_url,
            sent: false,
            status: 200,
            headers: Vec::new(),
            body: Vec::new(),
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

/// Remember the full URL of the request and its `Referer`, Java's `getRequestUrl`, for artifact hits.
pub(crate) fn set_request_info(request: &WebRequest) {
    let host = request.header("host").unwrap_or("localhost");
    let scheme = if request.https { "https" } else { "http" };
    let mut url = format!("{scheme}://{host}{}", request.path);
    let query: Vec<String> = request
        .query
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }
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
            web.referrer = referrer;
            web.request_details = details;
            web.query_names = request.query.iter().map(|(name, _)| name.clone()).collect();
            web.client_ip = client_ip(request);
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
    send_bytes(200, "text/plain", text.as_bytes().to_vec());
}

/// `ec.web.sendResourceResponse(location, inline)`: the bytes at a resource location with the type of its file name,
/// shown inline or as an attachment. Nothing at the location, or a directory, is 404.
pub(crate) fn send_resource_response(location: &str, inline: bool) {
    let bytes = ExecutionContext::with_current(|ec| {
        let resource = ec.factory().resource().get(location).ok()?;
        if !resource.exists() {
            return None;
        }
        resource.read_bytes().ok()
    })
    .flatten();
    let Some(bytes) = bytes else {
        tracing::warn!("Sending not found response, resource not found at: {location}");
        send_bytes(
            404,
            "text/plain",
            format!("Resource not found at {location}").into_bytes(),
        );
        return;
    };
    let file_name = location.trim_end_matches('/').rsplit('/').next().unwrap_or(location);
    let mut headers = Vec::new();
    if let Some(content_type) = moqui_lib::resource::content_type_for_name(file_name) {
        headers.push(("Content-Type".to_string(), content_type.to_string()));
    }
    if inline {
        headers.push(("Content-Disposition".to_string(), "inline".to_string()));
        headers.push((
            "Cache-Control".to_string(),
            "max-age=86400, must-revalidate, public".to_string(),
        ));
    } else {
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
        headers.push((
            "Content-Disposition".to_string(),
            format!("attachment; filename=\"{safe}\"; filename*=utf-8''{encoded}"),
        ));
    }
    send_bytes_with_headers(200, headers, bytes);
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
            web.status = 200;
            web.headers.clear();
            web.body.clear();
        }
    });
}

/// What a request had written, set aside while something else uses the slot's output.
pub(crate) struct OutputStash((u16, Vec<(String, String)>, Vec<u8>, bool));

/// Take what was written so far and leave the slot empty, for output that is not the response.
pub(crate) fn stash_output() -> OutputStash {
    let taken = take_output();
    clear_output();
    OutputStash(taken)
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
    ExecutionContext::with_current(|ec| ec.message().errors().to_vec()).unwrap_or_default()
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

/// The largest request body: `webapp_max_body`, 10 MiB by default, and for a `multipart/form-data` body
/// `webapp_max_upload`.
pub fn body_limit(request: &WebRequest) -> usize {
    if is_multipart(request) {
        max_upload()
    } else {
        parse_size(&moqui_lib::env::get_prop_or_env("webapp_max_body")).unwrap_or(DEFAULT_MAX_BODY)
    }
}

pub(crate) fn is_multipart(request: &WebRequest) -> bool {
    request
        .header("content-type")
        .unwrap_or("")
        .trim_start()
        .get(..10)
        .is_some_and(|start| start.eq_ignore_ascii_case("multipart/"))
}

const DEFAULT_MAX_UPLOAD: usize = 100 * 1024 * 1024;

/// `webapp_max_upload`: bytes, or a number with a `k`, `m` or `g` suffix. 100 MiB when unset or not a size.
pub fn max_upload() -> usize {
    parse_size(&moqui_lib::env::get_prop_or_env("webapp_max_upload")).unwrap_or(DEFAULT_MAX_UPLOAD)
}

fn parse_size(text: &str) -> Option<usize> {
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
    moqui_lib::env::get_prop_or_env("webapp_upload_executable_allow") == "true"
}

/// Query, then form, then a JSON object. A JSON list is not merged into the map.
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
        let parsed = multipart::parse(request.header("content-type").unwrap_or(""), &request.body)
            .map_err(|err| BodyError::Multipart(err.to_string()))?;
        pairs.extend(parsed.fields);
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
    if media == "application/x-www-form-urlencoded" || (media.is_empty() && request.body.contains(&b'=')) {
        pairs.extend(form_pairs(&String::from_utf8_lossy(&request.body)));
        return Ok(ParsedBody {
            parameters: pairs_to_parameters(pairs),
            json_list,
            multipart: multipart_files,
        });
    }
    let mut parameters = pairs_to_parameters(pairs);
    // As Java's WebFacadeImpl: a body that is not a form is `_requestBodyText`, and a JSON one is read as well. An
    // object's fields are parameters, an array is `_requestBodyJsonList`, and JSON that does not parse is
    // `_requestBodyJsonParseError` for the screen or service to answer, not a refused request.
    let text = String::from_utf8_lossy(&request.body).into_owned();
    parameters.insert("_requestBodyText".to_string(), Value::String(text.clone()));
    if media == "application/json" || media == "text/json" || media.ends_with("+json") {
        match crate::service::parse_json(&text) {
            Ok(Value::Map(map)) => {
                for (key, value) in map {
                    parameters.insert(key, value);
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
fn pairs_to_parameters(pairs: Vec<(String, String)>) -> BTreeMap<String, Value> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, value) in pairs {
        grouped.entry(key).or_default().push(value);
    }
    grouped
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 {
                Value::string(values.remove(0))
            } else {
                Value::List(values.into_iter().map(Value::string).collect())
            };
            (key, value)
        })
        .collect()
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

/// First hop of `webapp_client_ip_header` when that property is set and the header is present.
pub fn client_ip(request: &WebRequest) -> String {
    let name = moqui_lib::env::get_prop_or_env("webapp_client_ip_header");
    if !name.is_empty() {
        if let Some(value) = request.header(&name) {
            let first = value.split(',').next().unwrap_or("").trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    request.peer_ip.clone()
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
    !matches!(
        moqui_lib::env::get_prop_or_env("webapp_require_session_token").as_str(),
        "false"
    )
}

fn handle_cors() -> bool {
    !matches!(moqui_lib::env::get_prop_or_env("webapp_handle_cors").as_str(), "false")
}

fn allow_origins() -> Vec<String> {
    moqui_lib::env::get_prop_or_env("webapp_allow_origins")
        .split(',')
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .collect()
}

/// Java's `WebUtilities.webSocketOriginAllowed`, which CORS follows too: no Origin, `*`, the request's own host, an
/// allowed origin or domain, or a subdomain of an allowed domain on a dot boundary (`moqui.org` allows `www.moqui.org`,
/// not `foo-moqui.org`). Case-insensitive.
pub fn origin_allowed_for(origin: Option<&str>, host: Option<&str>) -> bool {
    let Some(origin) = origin.map(str::trim).filter(|text| !text.is_empty()) else {
        return true;
    };
    let origin = origin.to_ascii_lowercase();
    let allow: Vec<String> = allow_origins().iter().map(|text| text.to_ascii_lowercase()).collect();
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

/// Screen-render headers, and CORS when the origin is allowed.
pub fn finish_response(request: &WebRequest, mut response: WebResponse, preflight: bool) -> WebResponse {
    push_unique(
        &mut response.headers,
        "Cache-Control",
        "no-cache, no-store, must-revalidate, private",
    );
    push_unique(
        &mut response.headers,
        "Content-Security-Policy",
        "frame-ancestors 'none'; form-action 'self';",
    );
    push_unique(&mut response.headers, "X-Frame-Options", "sameorigin");
    push_unique(&mut response.headers, "X-Content-Type-Options", "nosniff");
    // HSTS holds for every port of a host (RFC 6797), so on localhost a trusted dev certificate would send the plain
    // dev listener and any other local server to https for a year.
    if request.https && !is_localhost(request.header("host").unwrap_or("")) {
        push_unique(&mut response.headers, "Strict-Transport-Security", "max-age=31536000");
    }
    if handle_cors() {
        if let Some(origin) = request.header("origin").filter(|text| !text.is_empty()) {
            if origin_allowed_for(Some(origin), request.header("host")) {
                apply_cors(&mut response.headers, origin, preflight);
            }
        }
    }
    response
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

fn apply_cors(headers: &mut Vec<(String, String)>, origin: &str, preflight: bool) {
    headers.push(("Vary".to_string(), "Origin".to_string()));
    headers.push(("Access-Control-Allow-Credentials".to_string(), "true".to_string()));
    headers.push(("Access-Control-Allow-Origin".to_string(), origin.to_string()));
    if preflight {
        headers.push((
            "Access-Control-Allow-Headers".to_string(),
            PREFLIGHT_HEADERS.to_string(),
        ));
        headers.push((
            "Access-Control-Allow-Methods".to_string(),
            "GET,PUT,POST,PATCH,DELETE,OPTIONS".to_string(),
        ));
        headers.push(("Access-Control-Max-Age".to_string(), "3600".to_string()));
    } else {
        headers.push(("Access-Control-Expose-Headers".to_string(), EXPOSE_HEADERS.to_string()));
    }
}

fn push_unique(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    if headers.iter().any(|(key, _)| key.eq_ignore_ascii_case(name)) {
        return;
    }
    headers.push((name.to_string(), value.to_string()));
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

pub(crate) fn early_error(request: &WebRequest, status: u16, message: &str) -> WebResponse {
    finish_response(request, json_error(status, message), false)
}

/// The HTTP status of an error, by its [`ErrorKind`]. A general error is a 500 whatever its message says.
pub(crate) fn status_for(err: &Error) -> u16 {
    match err.kind() {
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
/// instances get the message.
pub(crate) fn error_text(status: u16, err: &Error) -> String {
    if status == 500 && is_production() {
        format!("internal error, id {}", log_error_id(err))
    } else {
        err.message().to_string()
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
        // The kind is found under a lib error that names it as its source.
        let wrapped = moqui_lib::error::Error::new("outer").with_source(Error::of_kind(ErrorKind::Unavailable, "full"));
        assert_eq!(super::status_for(&Error::from(wrapped)), 503);
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
