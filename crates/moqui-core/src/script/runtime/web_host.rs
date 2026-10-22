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

//! The servlet API's request and response for scripts, `ec.web.request` and `ec.web.response`, and the handler
//! objects of the conf's script servlets, filters and listeners ([`invoke`]). Guide: `docs/Web-Scripts.md`.
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

use super::*;

/// `ec.web.request`, `ec.web.response` and their prototypes. Each read of `ec.web.request` is a new object over the
/// same request; what a script keeps (attributes, the response) lives on the server.
const WEB_JS: &str = r#"(function (ec, host) {
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
            println: (text) => response.write((text ?? "") + "\n"), flush: () => {}, close: () => {} };
    };
    RS.getOutputStream = function () {
        const response = this;
        return { write: (bytes) => response.write(bytes), flush: () => {}, close: () => {} };
    };
    Object.defineProperty(RS, "writer", { get() { return this.getWriter(); } });
    Object.defineProperty(RS, "outputStream", { get() { return this.getOutputStream(); } });
    RS.flushBuffer = function () {};
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
})"#;

/// Calls a handler object's method for its kind: a servlet's `service`, else its `doGet` (`doPost`, ...) by method, a
/// filter's `doFilter` with the chain, a listener's method named for the event. The handler's `config` is set first.
const INVOKE_JS: &str = r#"(function (ec, chainNext) {
    const methods = { GET: "doGet", POST: "doPost", PUT: "doPut", DELETE: "doDelete", PATCH: "doPatch", HEAD: "doHead",
        OPTIONS: "doOptions" };
    function configOf(data) {
        const params = data.initParams || {};
        return { name: data.name, location: data.location, initParams: params,
            getInitParameter: (name) => params[name] ?? null, getInitParameterNames: () => Object.keys(params),
            getServletName: () => data.name, getFilterName: () => data.name };
    }
    return { invoke(handler, kind, data) {
        if (handler == null || typeof handler !== "object") {
            throw new Error(`${data.location} must evaluate to a handler object, as ({ doGet(request, response) {...} })`);
        }
        const config = configOf(data);
        if (Object.isExtensible(handler)) handler.config = config;
        if (kind === "listener") {
            const method = handler[data.event];
            if (typeof method === "function") method.call(handler, Object.assign({ type: data.event }, data.eventData || {}), config);
            return;
        }
        const request = ec.web.request, response = ec.web.response;
        if (kind === "filter") {
            if (typeof handler.doFilter !== "function") throw new Error(`${data.location} has no doFilter(request, response, chain)`);
            handler.doFilter(request, response, { doFilter: () => chainNext() }, config);
            return;
        }
        if (typeof handler.service === "function") { handler.service(request, response, config); return; }
        let name = methods[request.method];
        if (name === "doHead" && typeof handler.doHead !== "function") name = "doGet";
        if (name && typeof handler[name] === "function") { handler[name](request, response, config); return; }
        response.sendError(405, `HTTP method ${request.method} is not supported by this URL`);
    } };
})"#;

pub(super) fn install<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> Result<(), Error> {
    let define: Function = eval_js(ctx, WEB_JS)?;
    let ec: Object = caught(ctx, "ec", ctx.globals().get("ec"))?;
    caught(ctx, "web", define.call::<_, ()>((ec, host.clone())))
}

pub(super) fn methods<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> Result<(), Error> {
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, host.set($name, function))?;
        };
    }
    method!("webRequestInfo", web_request_info);
    method!("webBodyText", web_body_text);
    method!("webBodyBytes", web_body_bytes);
    method!("webAttribute", web_attribute);
    method!("webSetAttribute", web_set_attribute);
    method!("webAttributeNames", web_attribute_names);
    method!("webSessionId", web_session_id);
    method!("webSetStatus", web_set_status);
    method!("webStatus", web_status);
    method!("webSetHeader", web_set_header);
    method!("webAddHeader", web_add_header);
    method!("webHeaderValues", web_header_values);
    method!("webHeaderNames", web_header_names);
    method!("webWrite", web_write);
    method!("webCommitted", web_committed);
    method!("webSendRedirect", web_send_redirect);
    method!("webForward", web_forward);
    method!("restOpen", rest_open);
    method!("restReadBody", rest_read_body);
    Ok(())
}

/// Run the handler object at `location` for `kind` (`servlet`, `filter` or `listener`), in a context frame of its own.
/// `data` carries `name`, `location`, `initParams` and, for a listener, `event` and `eventData`.
///
/// # Errors
///
/// Returns an error when the file cannot be read, does not evaluate to an object, or the handler throws.
pub(crate) fn invoke(location: &str, kind: &str, data: Value) -> Result<(), Error> {
    let source = super::facades::read_script(location)?;
    let _frame = crate::script::ContextFrame::push();
    with_context(|ctx| {
        let script = ScriptText::named(source, location);
        let handler = eval_compiled_text::<JsValue>(ctx, &script, false)?;
        let invoker = engine_object(ctx, "web-invoke", |ctx| {
            let make: Function = eval_js(ctx, INVOKE_JS)?;
            let ec: Object = caught(ctx, "ec", ctx.globals().get("ec"))?;
            let next = caught(ctx, "chainNext", Function::new(ctx.clone(), chain_next))?;
            caught(ctx, "web-invoke", make.call::<_, Object>((ec, next)))
        })?;
        let call: Function = caught(ctx, "invoke", invoker.get("invoke"))?;
        let data = host_to_js(ctx, data).map_err(js_error)?;
        call.call::<_, ()>((handler, kind, data))
            .catch(ctx)
            .map_err(|caught| Error::script(caught.to_string()).with_kind(caught_kind(&caught)))
    })
}

/// `chain.doFilter()`: the next filter, or the servlet at the end of the chain.
fn chain_next(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        crate::web::servlet::chain_next().map_err(|err| throw_context(&ctx, &err))
    })
}

fn pairs_value<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> Value {
    Value::List(
        pairs
            .map(|(name, value)| Value::List(vec![Value::string(name), Value::string(value)]))
            .collect(),
    )
}

fn web_request_info<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let Some(request) = crate::web::current_request() else {
            return Ok(JsValue::new_null(ctx.clone()));
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
        host_to_js(&ctx, Value::Map(map))
    })
}

fn web_body_text(ctx: Ctx<'_>) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        Ok(String::from_utf8_lossy(&crate::web::request_body()).into_owned())
    })
}

fn web_body_bytes<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        host_to_js(&ctx, Value::Bytes(crate::web::request_body().to_vec()))
    })
}

fn web_attribute<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        host_to_js(&ctx, crate::web::attribute(&name).unwrap_or(Value::Null))
    })
}

fn web_set_attribute<'js>(ctx: Ctx<'js>, name: String, value: JsValue<'js>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let value = js_to_host(&ctx, &value)?;
        crate::web::set_attribute(&name, (!matches!(value, Value::Null)).then_some(value));
        Ok(())
    })
}

fn web_attribute_names(ctx: Ctx<'_>) -> rquickjs::Result<Vec<String>> {
    host_guard(&ctx, || Ok(crate::web::attribute_names()))
}

fn web_session_id(ctx: Ctx<'_>) -> rquickjs::Result<Option<String>> {
    host_guard(&ctx, || Ok(crate::user::http_log_identity().0))
}

fn web_set_status(ctx: Ctx<'_>, status: i32) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let status = u16::try_from(status)
            .ok()
            .filter(|status| (100..=999).contains(status))
            .ok_or_else(|| Exception::throw_message(&ctx, &format!("{status} is not an HTTP status")))?;
        crate::web::set_status(status);
        Ok(())
    })
}

fn web_status(ctx: Ctx<'_>) -> rquickjs::Result<i32> {
    host_guard(&ctx, || Ok(i32::from(crate::web::output_status())))
}

fn web_set_header(ctx: Ctx<'_>, name: String, value: Opt<String>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        check_header(&ctx, &name, value.0.as_deref().unwrap_or(""))?;
        crate::web::set_header(&name, value.0.as_deref());
        Ok(())
    })
}

fn web_add_header(ctx: Ctx<'_>, name: String, value: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        check_header(&ctx, &name, &value)?;
        crate::web::add_header(name, value);
        Ok(())
    })
}

/// A header name of token characters and a value with no line break, so a script cannot split the response.
fn check_header(ctx: &Ctx<'_>, name: &str, value: &str) -> rquickjs::Result<()> {
    let token = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte));
    if !token || value.contains(['\r', '\n', '\0']) {
        return Err(Exception::throw_message(
            ctx,
            &format!("not a valid response header: {name}"),
        ));
    }
    Ok(())
}

fn web_header_values(ctx: Ctx<'_>, name: String) -> rquickjs::Result<Vec<String>> {
    host_guard(&ctx, || Ok(crate::web::response_header_values(&name)))
}

fn web_header_names(ctx: Ctx<'_>) -> rquickjs::Result<Vec<String>> {
    host_guard(&ctx, || Ok(crate::web::response_header_names()))
}

fn web_write<'js>(ctx: Ctx<'js>, content: JsValue<'js>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        match js_to_host(&ctx, &content)? {
            Value::Bytes(bytes) => crate::web::write_bytes(&bytes),
            other => crate::web::write_bytes(moqui_lib::object::to_plain_string(&other).as_bytes()),
        }
        Ok(())
    })
}

fn web_committed(ctx: Ctx<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(crate::web::output_sent()))
}

fn web_send_redirect(ctx: Ctx<'_>, location: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        check_header(&ctx, "Location", &location)?;
        crate::web::send_redirect(&location);
        Ok(())
    })
}

/// `response.forward`: the status and headers, and a body kept by [`rest_open`] streamed, or bytes.
fn web_forward<'js>(ctx: Ctx<'js>, status: i32, headers: JsValue<'js>, body: JsValue<'js>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let status = u16::try_from(status)
            .ok()
            .filter(|status| (100..=999).contains(status))
            .ok_or_else(|| Exception::throw_message(&ctx, &format!("{status} is not an HTTP status")))?;
        let headers = header_pairs(&js_to_host(&ctx, &headers)?);
        if let Some(id) = body.as_int() {
            let stream = u32::try_from(id)
                .ok()
                .and_then(crate::web::upstream::stream_of)
                .ok_or_else(|| Exception::throw_message(&ctx, "the response body was already read or forwarded"))?;
            crate::web::send_stream(status, headers, stream);
        } else {
            let bytes = match js_to_host(&ctx, &body)? {
                Value::Bytes(bytes) => bytes,
                Value::Null => Vec::new(),
                other => moqui_lib::object::to_plain_string(&other).into_bytes(),
            };
            crate::web::send_bytes_with_headers(status, headers, bytes);
        }
        Ok(())
    })
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

/// `RestClient.send()`: the request sent, the status and headers back, the body kept on the server by its number.
fn rest_open<'js>(ctx: Ctx<'js>, request: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let Value::Map(request) = js_to_host(&ctx, &request)? else {
            return Err(Exception::throw_message(&ctx, "rest request is not an object"));
        };
        let text = |name: &str| match request.get(name) {
            Some(Value::Null) | None => None,
            Some(value) => Some(moqui_lib::object::to_plain_string(value)),
        };
        let method_name = text("method").unwrap_or_else(|| "GET".to_string()).to_ascii_uppercase();
        let method = crate::outbound::Method::parse(&method_name)
            .ok_or_else(|| Exception::throw_message(&ctx, &format!("HTTP method {method_name} is not supported")))?;
        let url = text("uri")
            .filter(|url| !url.is_empty())
            .ok_or_else(|| Exception::throw_message(&ctx, "no URI specified for REST call"))?;
        let headers = request.get("headers").map(header_pairs).unwrap_or_default();
        let body = if matches!(request.get("bodyFromRequest"), Some(Value::Bool(true))) {
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
        let sim = with_ec(&ctx, |ec| ec.factory().entity())?.is_some_and(|facade| facade.sim_active());
        if sim {
            return Err(Exception::throw_message(
                &ctx,
                &format!(
                    "HTTP {method} {} denied because sim mode is active",
                    crate::outbound::redacted(&url)
                ),
            ));
        }
        if let Err(refused) = crate::entity::fence::check_effect(
            crate::entity::fence::Effect::RemoteCall,
            &format!("HTTP {method} {}", crate::outbound::redacted(&url)),
        ) {
            return Err(Exception::throw_message(&ctx, refused.message()));
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
            Exception::throw_message(
                &ctx,
                &format!(
                    "Error calling HTTP request to {}: {}",
                    crate::outbound::redacted(&url),
                    err.message()
                ),
            )
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
        host_to_js(&ctx, Value::Map(map))
    })
}

/// The whole body of a response `send()` kept, as `{text, bytes}`.
fn rest_read_body<'js>(ctx: Ctx<'js>, id: i32) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let bytes = u32::try_from(id)
            .map_err(|_| Error::new("no such response body"))
            .and_then(|id| crate::web::upstream::read_all(id, MAX_READ_BODY))
            .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
        let mut map = indexmap::IndexMap::new();
        map.insert("text".to_string(), Value::string(String::from_utf8_lossy(&bytes)));
        map.insert("bytes".to_string(), Value::Bytes(bytes));
        host_to_js(&ctx, Value::Map(map))
    })
}
