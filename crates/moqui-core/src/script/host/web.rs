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

//! The servlet API's request and response for scripts, `ec.web.request` and `ec.web.response`: hidden functions of
//! the tools shim, and `web_js!`, which builds the request and response objects over them. The handler objects of the
//! conf's script servlets, filters and listeners are run by `script/runtime/web_host.rs`. Guide:
//! `docs/interfaces/Web-Scripts.md`.
//!
//! ```text
//! request:  method, requestURI, queryString, contextPath (""), servletPath, pathInfo, scheme, serverName, serverPort,
//!           remoteAddr, contentType, characterEncoding, contentLength (also as getX() and isSecure()),
//!           getRequestURL(), getHeader(name), getHeaders(name), getHeaderNames(), getCookies(),
//!           getParameter(name), getParameterValues(name), getParameterMap(), getParameterNames(),
//!           getAttribute(name), setAttribute(name, value), removeAttribute(name), getAttributeNames(),
//!           getSession() -> {id, getAttribute, setAttribute, removeAttribute} or null, bodyText(), bodyBytes()
//! response: setStatus(code), getStatus(), setHeader(name, value), addHeader(name, value), getHeader(name),
//!           getHeaders(name), getHeaderNames(), containsHeader(name), setContentType(type), getContentType(),
//!           write(text or Uint8Array), getWriter() -> {write, print, println, flush}, sendError(code, message),
//!           sendRedirect(url), isCommitted(), forward(restResponse, {skipHeaders: [...]})
//! ```

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostError, Method, Ret};

/// `(function (ec, host) { ... })`: `ec.web.request`, `ec.web.response` and their prototypes. Each read of
/// `ec.web.request` is a new object over the same request; what a script keeps (attributes, the response) lives on the
/// server.
macro_rules! web_js {
    () => {
        r#"(function (ec, host) {
    const HOP_BY_HOP = ["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "proxy-connection", "te",
        "trailer", "transfer-encoding", "upgrade", "content-length"];
    const firstOf = (value) => Array.isArray(value) ? (value.length ? value[0] : null) : value;
    const textOf = (value) => value == null ? null : String(value);

    function WebRequest(info) {
        Object.assign(this, info);
        this.contextPath = "";
        this.headerPairs = info.headers || [];
        delete this.headers;
    }
    const RQ = WebRequest.prototype;
    RQ.getMethod = function () { return this.method; };
    RQ.getRequestURI = function () { return this.requestURI; };
    RQ.getQueryString = function () { return this.queryString; };
    RQ.getContextPath = function () { return ""; };
    RQ.getServletPath = function () { return this.servletPath; };
    RQ.getPathInfo = function () { return this.pathInfo; };
    RQ.getScheme = function () { return this.scheme; };
    RQ.getServerName = function () { return this.serverName; };
    RQ.getServerPort = function () { return this.serverPort; };
    RQ.isSecure = function () { return this.secure; };
    RQ.getRemoteAddr = function () { return this.remoteAddr; };
    RQ.getContentType = function () { return this.contentType; };
    RQ.getCharacterEncoding = function () { return this.characterEncoding; };
    RQ.getContentLength = function () { return this.contentLength; };
    RQ.getRequestURL = function () {
        const port = this.serverPort, plain = (this.scheme === "https" && port === 443) || (this.scheme === "http" && port === 80);
        return this.scheme + "://" + this.serverName + (plain ? "" : ":" + port) + this.requestURI;
    };
    RQ.getHeaders = function (name) {
        const lower = String(name).toLowerCase();
        return this.headerPairs.filter(([key]) => key === lower).map(([, value]) => value);
    };
    RQ.getHeader = function (name) { const values = this.getHeaders(name); return values.length ? values[0] : null; };
    RQ.getIntHeader = function (name) { const value = this.getHeader(name); return value == null ? -1 : parseInt(value, 10); };
    RQ.getHeaderNames = function () { return [...new Set(this.headerPairs.map(([key]) => key))]; };
    RQ.getCookies = function () {
        const cookies = [];
        for (const header of this.getHeaders("cookie")) {
            for (const part of header.split(";")) {
                const at = part.indexOf("=");
                if (at > 0) cookies.push({ name: part.substring(0, at).trim(), value: part.substring(at + 1).trim() });
            }
        }
        return cookies;
    };
    RQ.getParameterMap = function () { return ec.web.requestParameters; };
    RQ.getParameterNames = function () { return Object.keys(ec.web.requestParameters); };
    RQ.getParameter = function (name) { return textOf(firstOf(ec.web.requestParameters[name])); };
    RQ.getParameterValues = function (name) {
        const value = ec.web.requestParameters[name];
        return value == null ? null : (Array.isArray(value) ? value.map(textOf) : [textOf(value)]);
    };
    RQ.getAttribute = function (name) { return host.webAttribute(String(name)); };
    RQ.setAttribute = function (name, value) { host.webSetAttribute(String(name), value); };
    RQ.removeAttribute = function (name) { host.webSetAttribute(String(name), null); };
    RQ.getAttributeNames = function () { return host.webAttributeNames(); };
    RQ.getSession = function () {
        const id = host.webSessionId();
        if (id == null) return null;
        const attributes = () => ec.web.sessionAttributes;
        return { id: id, getId: () => id, getAttribute: (name) => attributes()[name] ?? null,
            setAttribute: (name, value) => { attributes().put(String(name), value); },
            removeAttribute: (name) => { attributes().remove(String(name)); },
            getAttributeNames: () => Object.keys(attributes()) };
    };
    // the body as text (UTF-8) and as a Uint8Array; empty for a multipart body, whose files are ec.web.multiPartParameters
    RQ.bodyText = function () { return host.webBodyText(); };
    RQ.bodyBytes = function () { return host.webBodyBytes(); };

    function WebResponse() {}
    const RS = WebResponse.prototype;
    RS.setStatus = function (status) { host.webSetStatus(Number(status)); };
    RS.getStatus = function () { return host.webStatus(); };
    RS.setHeader = function (name, value) { host.webSetHeader(String(name), textOf(value)); };
    RS.addHeader = function (name, value) { host.webAddHeader(String(name), String(value)); };
    RS.setIntHeader = RS.setHeader;
    RS.setDateHeader = function (name, millis) { this.setHeader(name, new Date(Number(millis)).toUTCString()); };
    RS.getHeaders = function (name) { return host.webHeaderValues(String(name)); };
    RS.getHeader = function (name) { const values = this.getHeaders(name); return values.length ? values[0] : null; };
    RS.getHeaderNames = function () { return host.webHeaderNames(); };
    RS.containsHeader = function (name) { return this.getHeaders(name).length > 0; };
    RS.setContentType = function (contentType) { this.setHeader("Content-Type", contentType); };
    RS.getContentType = function () { return this.getHeader("Content-Type"); };
    RS.setCharacterEncoding = function (charset) {
        const type = this.getContentType();
        if (type) this.setContentType(type.split(";")[0].trim() + ";charset=" + charset);
    };
    RS.setContentLength = function () {};
    RS.write = function (content) { host.webWrite(content instanceof Uint8Array ? content : String(content ?? "")); };
    RS.getWriter = function () {
        const response = this;
        return { write: (text) => response.write(text), print: (text) => response.write(text),
            println: (text) => response.write((text ?? "") + "\n"), flush: () => host.webFlush(), close: () => {} };
    };
    RS.getOutputStream = function () {
        const response = this;
        return { write: (bytes) => response.write(bytes), flush: () => host.webFlush(), close: () => {} };
    };
    Object.defineProperty(RS, "writer", { get() { return this.getWriter(); } });
    Object.defineProperty(RS, "outputStream", { get() { return this.getOutputStream(); } });
    RS.flushBuffer = function () { host.webFlush(); };
    RS.isCommitted = function () { return host.webCommitted(); };
    RS.sendError = function (status, message) { ec.web.sendError(Number(status), message == null ? null : String(message), null); };
    RS.sendRedirect = function (location) { host.webSendRedirect(String(location)); };
    // The status, headers and body of a response from ec.service.rest()...send() (or call()), as the response to this
    // request. A body send() left on the server streams to the client as it comes and never enters the script. The
    // hop-by-hop headers and Content-Length are not passed on, nor the names in skipHeaders.
    RS.forward = function (upstream, options) {
        const skip = new Set(HOP_BY_HOP.concat(((options && options.skipHeaders) || []).map((name) => String(name).toLowerCase())));
        const headers = (upstream.headerPairs || []).filter(([name]) => !skip.has(String(name).toLowerCase()));
        if (upstream.bodyId != null) {
            const bodyId = upstream.bodyId;
            upstream.bodyId = null;
            host.webForward(Number(upstream.statusCode), headers, bodyId);
        } else {
            host.webForward(Number(upstream.statusCode), headers, upstream.bytes());
        }
    };

    const response = new WebResponse();
    Object.defineProperty(ec.web, "request", { get: () => {
        const info = host.webRequestInfo();
        return info == null ? null : new WebRequest(info);
    }, enumerable: false, configurable: true });
    Object.defineProperty(ec.web, "response", { get: () => host.webRequestInfo() == null ? null : response,
        enumerable: false, configurable: true });
    ec.web.getRequest = () => ec.web.request;
    ec.web.getResponse = () => ec.web.response;
})"#
    };
}
pub(super) use web_js;

/// The web functions of the tools shim.
pub(super) static METHODS: &[Method] = &[
    method!("tools", "webRequestInfo", [], web_request_info),
    method!("tools", "webBodyText", [], web_body_text),
    method!("tools", "webBodyBytes", [], web_body_bytes),
    method!("tools", "webAttribute", [Text], web_attribute),
    method!("tools", "webSetAttribute", [Text, Value], web_set_attribute),
    method!("tools", "webAttributeNames", [], web_attribute_names),
    method!("tools", "webSessionId", [], web_session_id),
    method!("tools", "webSetStatus", [Int], web_set_status),
    method!("tools", "webStatus", [], web_status),
    method!("tools", "webSetHeader", [Text, OptText], web_set_header),
    method!("tools", "webAddHeader", [Text, Text], web_add_header),
    method!("tools", "webHeaderValues", [Text], web_header_values),
    method!("tools", "webHeaderNames", [], web_header_names),
    method!("tools", "webWrite", [Value], web_write),
    method!("tools", "webCommitted", [], web_committed),
    method!("tools", "webFlush", [], web_flush),
    method!("tools", "webSendRedirect", [Text], web_send_redirect),
    method!("tools", "webForward", [Int, Value, Value], web_forward),
    method!("tools", "restOpen", [Value], rest_open),
    method!("tools", "restReadBody", [Int], rest_read_body),
];

fn texts(texts: Vec<String>) -> Ret {
    Ret::Value(Value::List(texts.into_iter().map(Value::String).collect()))
}

fn pairs_value<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> Value {
    Value::List(
        pairs
            .map(|(name, value)| Value::List(vec![Value::string(name), Value::string(value)]))
            .collect(),
    )
}

fn web_request_info(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Some(request) = crate::web::current_request() else {
        return Ok(Value::Null.into());
    };
    let (servlet_path, path_info) = crate::web::servlet_match();
    let scheme = if request.https { "https" } else { "http" };
    let host = request.header("host").unwrap_or("");
    let server_name = crate::web::host_without_port(host).to_string();
    let server_port = host
        .strip_prefix(server_name.as_str())
        .and_then(|rest| rest.strip_prefix(':'))
        .and_then(|port| port.parse::<i64>().ok())
        .unwrap_or(if request.https { 443 } else { 80 });
    let content_type = request.header("content-type").map(str::to_string);
    let charset = content_type.as_deref().and_then(|value| {
        value.split(';').skip(1).find_map(|part| {
            let (key, value) = part.split_once('=')?;
            key.trim()
                .eq_ignore_ascii_case("charset")
                .then(|| value.trim().trim_matches('"').to_string())
        })
    });
    let text = |value: Option<String>| value.map_or(Value::Null, Value::String);
    let mut map = indexmap::IndexMap::new();
    map.insert("method".to_string(), Value::string(request.method.to_ascii_uppercase()));
    map.insert("requestURI".to_string(), Value::string(&request.path));
    map.insert("queryString".to_string(), text(request.raw_query.clone()));
    map.insert("servletPath".to_string(), Value::string(servlet_path));
    map.insert("pathInfo".to_string(), text(path_info));
    map.insert("scheme".to_string(), Value::string(scheme));
    map.insert("secure".to_string(), Value::Bool(request.https));
    map.insert("serverName".to_string(), Value::string(server_name));
    map.insert("serverPort".to_string(), Value::I64(server_port));
    map.insert("remoteAddr".to_string(), Value::string(crate::web::current_client_ip()));
    map.insert("contentType".to_string(), text(content_type));
    map.insert("characterEncoding".to_string(), text(charset));
    map.insert(
        "contentLength".to_string(),
        request
            .header("content-length")
            .and_then(|value| value.trim().parse::<i64>().ok())
            .map_or(Value::I64(-1), Value::I64),
    );
    map.insert(
        "headers".to_string(),
        pairs_value(
            request
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        ),
    );
    Ok(Value::Map(map).into())
}

fn web_body_text(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(String::from_utf8_lossy(&crate::web::request_body()).into_owned().into())
}

fn web_body_bytes(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Value::Bytes(crate::web::request_body().to_vec()).into())
}

fn web_attribute(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::web::attribute(call.text(0).unwrap_or_default())
        .unwrap_or(Value::Null)
        .into())
}

fn web_set_attribute(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let value = call.value_or_null(1);
    crate::web::set_attribute(
        call.text(0).unwrap_or_default(),
        (!matches!(value, Value::Null)).then_some(value),
    );
    Ok(Ret::Undefined)
}

fn web_attribute_names(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(texts(crate::web::attribute_names()))
}

fn web_session_id(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::user::http_log_identity().session_id.into())
}

fn web_set_status(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let status = http_status(call.int(0).unwrap_or_default())?;
    crate::web::set_status(status);
    Ok(Ret::Undefined)
}

fn http_status(status: i32) -> Result<u16, HostError> {
    u16::try_from(status)
        .ok()
        .filter(|status| (100..=999).contains(status))
        .ok_or_else(|| HostError::message(format!("{status} is not an HTTP status")))
}

fn web_status(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Value::I32(i32::from(crate::web::output_status())).into())
}

fn web_set_header(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default();
    let value = call.text(1);
    check_header(name, value.unwrap_or(""))?;
    crate::web::set_header(name, value);
    Ok(Ret::Undefined)
}

fn web_add_header(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let value = call.text_or_empty(1);
    check_header(&name, &value)?;
    crate::web::add_header(name, value);
    Ok(Ret::Undefined)
}

/// A header name of token characters and a value with no line break, so a script cannot split the response.
fn check_header(name: &str, value: &str) -> Result<(), HostError> {
    let token = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
    if !token || value.contains(['\r', '\n', '\0']) {
        return Err(HostError::message(format!("not a valid response header: {name}")));
    }
    Ok(())
}

fn web_header_values(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(texts(crate::web::response_header_values(
        call.text(0).unwrap_or_default(),
    )))
}

fn web_header_names(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(texts(crate::web::response_header_names()))
}

fn web_write(call: &mut Call<'_>) -> Result<Ret, HostError> {
    match call.value_or_null(0) {
        Value::Bytes(bytes) => crate::web::write_bytes(&bytes),
        other => crate::web::write_bytes(moqui_lib::object::to_plain_string(&other).as_bytes()),
    }
    Ok(Ret::Undefined)
}

fn web_committed(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(crate::web::output_sent().into())
}

/// The writer's `flush` and `flushBuffer`: a response that may go as it is written (docs/ui/Screen-Output.md) sends its
/// head and what is written now; any other is still sent whole when the script ends.
fn web_flush(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::web::stream::flush();
    Ok(Ret::Undefined)
}

fn web_send_redirect(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    check_header("Location", &location)?;
    // Browsers drop tabs and other controls from a URL, so "/\t/host" would be read as "//host".
    if location.chars().any(char::is_control) {
        return Err(HostError::message("not a valid redirect location"));
    }
    crate::web::send_redirect(&location);
    Ok(Ret::Undefined)
}

/// `response.forward`: the status and headers, and a body kept by [`rest_open`] streamed, or bytes.
fn web_forward(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let status = http_status(call.int(0).unwrap_or_default())?;
    let headers = header_pairs(&call.value_or_null(1));
    let body = call.value_or_null(2);
    if let Value::I32(id) = body {
        let stream = u32::try_from(id)
            .ok()
            .and_then(crate::web::upstream::stream_of)
            .ok_or_else(|| HostError::message("the response body was already read or forwarded"))?;
        crate::web::send_stream(status, headers, stream);
    } else {
        let bytes = match body {
            Value::Bytes(bytes) => bytes,
            Value::Null => Vec::new(),
            other => moqui_lib::object::to_plain_string(&other).into_bytes(),
        };
        crate::web::send_bytes_with_headers(status, headers, bytes);
    }
    Ok(Ret::Undefined)
}

fn header_pairs(value: &Value) -> Vec<(String, String)> {
    let Value::List(pairs) = value else {
        return Vec::new();
    };
    pairs
        .iter()
        .filter_map(|pair| match pair {
            Value::List(pair) => match pair.as_slice() {
                [name, value] => Some((
                    moqui_lib::object::to_plain_string(name),
                    moqui_lib::object::to_plain_string(value),
                )),
                _ => None,
            },
            _ => None,
        })
        .filter(|(name, value)| !name.is_empty() && !value.contains(['\r', '\n', '\0']))
        .collect()
}

/// The largest body `text()` or `bytes()` reads from a response `send()` kept; `response.forward` has no limit.
const MAX_READ_BODY: u64 = 32 * 1024 * 1024;

/// A bounded multipart body rebuilt from spooled uploads. Raw multipart requests are not retained after parsing.
fn multipart_body(parts: &indexmap::IndexMap<String, Value>) -> Result<(String, bytes::Bytes), HostError> {
    const LIMIT: usize = 64 * 1024 * 1024;
    let boundary = crate::user::random_string(32).map_err(|error| HostError::message(error.message()))?;
    let mut body = Vec::new();
    for (name, value) in parts {
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(HostError::message("invalid multipart field name"));
        }
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes());
        let upload = if let Value::Object(object) = value {
            object.downcast::<crate::web::multipart::UploadedFile>()
        } else {
            None
        };
        if let Some(file) = upload {
            if file.size() > (LIMIT.saturating_sub(body.len())) as u64 || file.content_type.contains(['\r', '\n']) {
                return Err(HostError::message("multipart upload exceeds the transport limit"));
            }
            let filename = file.file_name.replace(['\"', '\\', '\r', '\n'], "_");
            body.extend_from_slice(
                format!(
                    "; filename=\"{filename}\"\r\nContent-Type: {}\r\n\r\n",
                    file.content_type
                )
                .as_bytes(),
            );
            body.extend_from_slice(&file.read_bytes().map_err(|error| HostError::message(error.message()))?);
        } else {
            let text = moqui_lib::object::to_plain_string(value);
            if text.len() > LIMIT.saturating_sub(body.len()) {
                return Err(HostError::message("multipart body exceeds the transport limit"));
            }
            body.extend_from_slice(b"\r\n\r\n");
            body.extend_from_slice(text.as_bytes());
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((
        format!("multipart/form-data; boundary={boundary}"),
        bytes::Bytes::from(body),
    ))
}

/// `RestClient.send()`: the request sent, the status and headers back, the body kept on the server by its number.
fn rest_open(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Value::Map(request) = call.value_or_null(0) else {
        return Err(HostError::message("rest request is not an object"));
    };
    let text = |name: &str| match request.get(name) {
        Some(Value::Null) | None => None,
        Some(value) => Some(moqui_lib::object::to_plain_string(value)),
    };
    let method_name = text("method").unwrap_or_else(|| "GET".to_string()).to_ascii_uppercase();
    let method = crate::outbound::Method::parse(&method_name)
        .ok_or_else(|| HostError::message(format!("HTTP method {method_name} is not supported")))?;
    let url = text("uri")
        .filter(|url| !url.is_empty())
        .ok_or_else(|| HostError::message("no URI specified for REST call"))?;
    let mut headers = request.get("headers").map(header_pairs).unwrap_or_default();
    let body = if let Some(Value::Map(parts)) = request.get("multipart") {
        let (content_type, body) = multipart_body(parts)?;
        headers.retain(|(name, _)| {
            !name.eq_ignore_ascii_case("content-type") && !name.eq_ignore_ascii_case("content-length")
        });
        headers.push(("Content-Type".to_string(), content_type));
        Some(body)
    } else if matches!(request.get("bodyFromRequest"), Some(Value::Bool(true))) {
        Some(crate::web::request_body()).filter(|body| !body.is_empty())
    } else {
        match request.get("bodyBytes") {
            Some(Value::Bytes(bytes)) => Some(bytes::Bytes::from(bytes.clone())),
            _ => text("body").filter(|body| !body.is_empty()).map(bytes::Bytes::from),
        }
    };
    let timeout = text("timeoutSeconds")
        .and_then(|seconds| seconds.parse::<f64>().ok())
        .filter(|seconds| *seconds > 0.0)
        .map(std::time::Duration::from_secs_f64);
    let sim = with_ec(|ec| ec.factory().entity())?.is_some_and(|facade| facade.sim_active());
    if sim {
        return Err(HostError::message(format!(
            "HTTP {method} {} denied because sim mode is active",
            crate::outbound::redacted(&url)
        )));
    }
    if let Err(refused) = crate::entity::fence::check_effect(
        crate::entity::fence::Effect::RemoteCall,
        &format!("HTTP {method} {}", crate::outbound::redacted(&url)),
    ) {
        return Err(HostError::message(refused.message()));
    }
    let opened = crate::outbound::open(crate::outbound::OpenRequest {
        method,
        url: url.clone(),
        headers,
        body,
        timeout,
        follow_redirects: !matches!(request.get("followRedirects"), Some(Value::Bool(false))),
    })
    .map_err(|err| {
        HostError::message(format!(
            "Error calling HTTP request to {}: {}",
            crate::outbound::redacted(&url),
            err.message()
        ))
    })?;
    let mut map = indexmap::IndexMap::new();
    map.insert("statusCode".to_string(), Value::I64(i64::from(opened.status)));
    map.insert(
        "reasonPhrase".to_string(),
        Value::string(crate::outbound::reason_phrase(opened.status)),
    );
    map.insert(
        "headers".to_string(),
        pairs_value(
            opened
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        ),
    );
    map.insert(
        "length".to_string(),
        opened
            .length
            .and_then(|length| i64::try_from(length).ok())
            .map_or(Value::Null, Value::I64),
    );
    let id = crate::web::upstream::register(opened.reader, opened.length);
    map.insert("bodyId".to_string(), Value::I64(i64::from(id)));
    Ok(Value::Map(map).into())
}

/// The whole body of a response `send()` kept, as `{text, bytes}`.
fn rest_read_body(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let bytes = u32::try_from(call.int(0).unwrap_or_default())
        .map_err(|_| crate::context::Error::new("no such response body"))
        .and_then(|id| crate::web::upstream::read_all(id, MAX_READ_BODY))
        .map_err(|err| HostError::message(err.message()))?;
    let mut map = indexmap::IndexMap::new();
    map.insert("text".to_string(), Value::string(String::from_utf8_lossy(&bytes)));
    map.insert("bytes".to_string(), Value::Bytes(bytes));
    Ok(Value::Map(map).into())
}
