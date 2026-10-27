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

//! HTTP of a client spec (plans/archive/ClientTestHarness.md 5.2): the server under test, sessions with a cookie jar and the
//! session token of their own, and requests with redirects off unless asked. The cookie jar keeps what `Set-Cookie`
//! says by name, domain and path, with expiry; the session token comes from the `moquiSessionToken` and `X-CSRF-Token`
//! response headers, an HTML page's `<meta name="moquiSessionToken">`, or a `/qapps` page's `moqui-config` JSON.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_lib::error::Error;
use serde_json::{json, Value};

/// The body kept of a response; `length` is the full size.
pub(crate) const MAX_BODY: usize = 16 * 1024 * 1024;

/// Redirects followed when a request asks (`follow: true`).
const MAX_REDIRECTS: usize = 10;

/// The server under test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Server {
    /// `https://host:port` and any path, no trailing slash.
    pub(crate) url: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) secure: bool,
}

impl Server {
    /// The server of `--server`. Plain `http` goes only to this machine unless `allow_http`, as for `moqui tui
    /// --server`: a login over it would cross the network in the clear.
    pub(crate) fn parse(text: &str, allow_http: bool) -> Result<Self, Error> {
        let url = reqwest::Url::parse(text.trim()).map_err(|err| Error::new(format!("--server {text}: {err}")))?;
        let host = url.host_str().unwrap_or("").to_string();
        let secure = match url.scheme() {
            "https" => true,
            "http" if allow_http || is_loopback(&host) => false,
            "http" => {
                return Err(Error::new(format!(
                    "--server {text}: plain http sends logins over the network; use https, http to this machine, or --allow-http"
                )))
            }
            other => return Err(Error::new(format!("--server {text}: {other} is not http or https"))),
        };
        if host.is_empty() {
            return Err(Error::new(format!("--server {text}: no host")));
        }
        let port = url.port_or_known_default().unwrap_or(if secure { 443 } else { 80 });
        let mut base = format!("{}://{host}", url.scheme());
        if let Some(port) = url.port() {
            base.push_str(&format!(":{port}"));
        }
        base.push_str(url.path().trim_end_matches('/'));
        Ok(Self {
            url: base,
            host: host.trim_start_matches('[').trim_end_matches(']').to_string(),
            port,
            secure,
        })
    }

    /// `path` on the server; a full URL is taken as it is.
    pub(crate) fn join(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{path}", self.url)
        } else {
            format!("{}/{path}", self.url)
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({ "url": self.url, "host": self.host, "port": self.port, "secure": self.secure })
    }
}

/// `localhost`, `127.0.0.0/8` or `::1`.
pub(crate) fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// One `Set-Cookie` header, as a response reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SetCookie {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) domain: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) max_age: Option<i64>,
    pub(crate) expires: Option<String>,
    pub(crate) secure: bool,
    pub(crate) http_only: bool,
    pub(crate) same_site: Option<String>,
}

impl SetCookie {
    pub(crate) fn parse(header: &str) -> Option<Self> {
        let mut parts = header.split(';');
        let (name, value) = parts.next()?.split_once('=')?;
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let mut cookie = Self {
            name: name.to_string(),
            value: value.trim().trim_matches('"').to_string(),
            ..Self::default()
        };
        for part in parts {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            let value = value.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "domain" if !value.is_empty() => {
                    cookie.domain = Some(value.trim_start_matches('.').to_ascii_lowercase());
                }
                "path" if value.starts_with('/') => cookie.path = Some(value.to_string()),
                "max-age" => cookie.max_age = value.parse().ok(),
                "expires" if !value.is_empty() => cookie.expires = Some(value.to_string()),
                "secure" => cookie.secure = true,
                "httponly" => cookie.http_only = true,
                "samesite" if !value.is_empty() => cookie.same_site = Some(value.to_string()),
                _ => {}
            }
        }
        Some(cookie)
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({
            "name": self.name, "value": self.value, "domain": self.domain, "path": self.path, "maxAge": self.max_age,
            "expires": self.expires, "secure": self.secure, "httpOnly": self.http_only, "sameSite": self.same_site,
        })
    }
}

/// Seconds since the epoch of an `Expires` date: `Wed, 21 Oct 2015 07:28:00 GMT`, or with dashes in the date.
fn expires_seconds(text: &str) -> Option<i64> {
    let parser = jiff::fmt::rfc2822::DateTimeParser::new();
    parser
        .parse_timestamp(text.trim())
        .or_else(|_| parser.parse_timestamp(text.trim().replace('-', " ")))
        .ok()
        .map(|stamp| stamp.as_second())
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// A cookie of the jar.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stored {
    name: String,
    value: String,
    domain: String,
    /// No `Domain` attribute: only the host that set it.
    host_only: bool,
    path: String,
    /// Seconds since the epoch; none for a session cookie.
    expiry: Option<i64>,
    secure: bool,
    http_only: bool,
}

/// The cookies of one session (RFC 6265 storage and matching, without the public suffix list).
#[derive(Clone, Debug, Default)]
pub(crate) struct CookieJar {
    cookies: Vec<Stored>,
}

impl CookieJar {
    /// Keep what a response to `url` set. A cookie for another domain is ignored; an expired one removes its match.
    pub(crate) fn store(&mut self, url: &reqwest::Url, cookie: &SetCookie, now: i64) {
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let (domain, host_only) = match &cookie.domain {
            Some(domain) if domain_match(&host, domain) => (domain.clone(), false),
            Some(_) => return,
            None => (host, true),
        };
        let path = cookie.path.clone().unwrap_or_else(|| default_path(url.path()));
        let expiry = match (cookie.max_age, cookie.expires.as_deref().and_then(expires_seconds)) {
            (Some(max_age), _) => Some(now + max_age),
            (None, Some(at)) => Some(at),
            (None, None) => None,
        };
        self.cookies
            .retain(|known| !(known.name == cookie.name && known.domain == domain && known.path == path));
        if expiry.is_some_and(|at| at <= now) {
            return;
        }
        self.cookies.push(Stored {
            name: cookie.name.clone(),
            value: cookie.value.clone(),
            domain,
            host_only,
            path,
            expiry,
            secure: cookie.secure,
            http_only: cookie.http_only,
        });
    }

    /// The `Cookie` header for a request to `url`: the cookies that match its host and path and have not expired, the
    /// longer paths first. A `Secure` cookie goes over https, and to this machine as a browser sends it.
    pub(crate) fn header(&self, url: &reqwest::Url, now: i64) -> Option<String> {
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let secure = url.scheme() == "https" || url.scheme() == "wss" || is_loopback(&host);
        let mut matched: Vec<&Stored> = self
            .cookies
            .iter()
            .filter(|cookie| {
                (if cookie.host_only {
                    cookie.domain == host
                } else {
                    domain_match(&host, &cookie.domain)
                }) && path_match(url.path(), &cookie.path)
                    && (secure || !cookie.secure)
                    && cookie.expiry.is_none_or(|at| at > now)
            })
            .collect();
        matched.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
        (!matched.is_empty()).then(|| {
            matched
                .iter()
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }

    pub(crate) fn clear(&mut self) {
        self.cookies.clear();
    }

    pub(crate) fn to_json(&self) -> Value {
        Value::Array(
            self.cookies
                .iter()
                .map(|cookie| {
                    json!({
                        "name": cookie.name, "value": cookie.value, "domain": cookie.domain, "path": cookie.path,
                        "expires": cookie.expiry, "secure": cookie.secure, "httpOnly": cookie.http_only,
                    })
                })
                .collect(),
        )
    }
}

fn domain_match(host: &str, domain: &str) -> bool {
    host == domain || (host.ends_with(domain) && host[..host.len() - domain.len()].ends_with('.'))
}

fn path_match(request: &str, cookie: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie) && (cookie.ends_with('/') || request[cookie.len()..].starts_with('/')))
}

/// The directory of a request path, the path a cookie without `Path` gets.
fn default_path(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(at) => path[..at].to_string(),
    }
}

/// The session token a response gives: its headers, then an HTML page's meta tag or `moqui-config` JSON.
pub(crate) fn session_token(headers: &[(String, String)], content_type: &str, body: &[u8]) -> Option<String> {
    let from_header = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("moquiSessionToken") || name.eq_ignore_ascii_case("x-csrf-token"))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if from_header.is_some() {
        return from_header;
    }
    if !content_type.to_ascii_lowercase().contains("html") {
        return None;
    }
    let html = String::from_utf8_lossy(body);
    meta_token(&html).or_else(|| config_token(&html))
}

/// `<meta name="moquiSessionToken" content="...">`, its attributes in either order.
fn meta_token(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(at) = rest.find("<meta") {
        let tag_end = rest[at..].find('>').map_or(rest.len(), |end| at + end);
        let tag = &rest[at..tag_end];
        if attribute(tag, "name").as_deref() == Some("moquiSessionToken") {
            return attribute(tag, "content").filter(|value| !value.is_empty());
        }
        rest = &rest[tag_end..];
    }
    None
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=\"");
    let start = tag.find(&marker)? + marker.len();
    let end = tag[start..].find('"')? + start;
    Some(tag[start..end].to_string())
}

/// The `moquiSessionToken` of the `/qapps` shell's `<script type="application/json" id="moqui-config">`.
fn config_token(html: &str) -> Option<String> {
    let at = html.find("id=\"moqui-config\"")?;
    let start = html[at..].find('>')? + at + 1;
    let end = html[start..].find("</script>")? + start;
    let config: Value = serde_json::from_str(&html[start..end]).ok()?;
    find_key(&config, "moquiSessionToken")
}

fn find_key(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .or_else(|| map.values().find_map(|item| find_key(item, key))),
        Value::Array(items) => items.iter().find_map(|item| find_key(item, key)),
        _ => None,
    }
}

/// A session: its cookies, its token, and the headers it sends on each request.
#[derive(Clone, Debug, Default)]
pub(crate) struct Session {
    pub(crate) jar: CookieJar,
    pub(crate) token: Option<String>,
    pub(crate) headers: Vec<(String, String)>,
}

impl Session {
    /// Take the cookies and the token of a response to `url`.
    pub(crate) fn absorb(&mut self, url: &str, headers: &[(String, String)], content_type: &str, body: &[u8]) {
        if let Ok(url) = reqwest::Url::parse(url) {
            let now = now_seconds();
            for (_, value) in headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            {
                if let Some(cookie) = SetCookie::parse(value) {
                    self.jar.store(&url, &cookie, now);
                }
            }
        }
        if let Some(token) = session_token(headers, content_type, body) {
            self.token = Some(token);
        }
    }

    pub(crate) fn cookie_header(&self, url: &str) -> Option<String> {
        reqwest::Url::parse(url)
            .ok()
            .and_then(|url| self.jar.header(&url, now_seconds()))
    }
}

/// A response as a spec sees it, before it becomes JSON.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    /// Up to [`MAX_BODY`].
    pub(crate) body: Vec<u8>,
    pub(crate) length: usize,
    pub(crate) elapsed: Duration,
}

impl Answer {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The response object of the client prelude, with `id` naming the body the host keeps for `save`.
    pub(crate) fn to_json(&self, id: u64) -> Value {
        let cookies: Vec<Value> = self
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .filter_map(|(_, value)| SetCookie::parse(value))
            .map(|cookie| cookie.to_json())
            .collect();
        json!({
            "id": id,
            "status": self.status,
            "ok": (200..300).contains(&self.status),
            "url": self.url,
            "location": self.header("location"),
            "contentType": self.header("content-type").unwrap_or(""),
            "timeMs": (self.elapsed.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0,
            "length": self.length,
            "text": String::from_utf8_lossy(&self.body),
            "headerList": self.headers.iter().map(|(name, value)| json!([name, value])).collect::<Vec<_>>(),
            "cookies": cookies,
            "magic": magic(&self.body),
        })
    }
}

/// The first bytes as text when they are printable, such as `%PDF` or `PK`, else as hex.
pub(crate) fn magic(body: &[u8]) -> String {
    let head = &body[..body.len().min(4)];
    if head.iter().all(|byte| byte.is_ascii_graphic() || *byte == b' ') {
        String::from_utf8_lossy(head).to_string()
    } else {
        head.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// What a request asks, from the spec's options (5.2).
#[derive(Debug, Default)]
pub(crate) struct Ask {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) opts: serde_json::Map<String, Value>,
}

impl Ask {
    fn text(&self, name: &str) -> Option<String> {
        match self.opts.get(name)? {
            Value::Null => None,
            Value::String(text) => Some(text.clone()),
            other => Some(other.to_string()),
        }
    }

    fn pairs(&self, name: &str) -> Vec<(String, String)> {
        match self.opts.get(name) {
            Some(Value::Object(map)) => map
                .iter()
                .filter(|(_, value)| !value.is_null())
                .flat_map(|(key, value)| match value {
                    Value::Array(items) => items.iter().map(|item| (key.clone(), plain(item))).collect::<Vec<_>>(),
                    other => vec![(key.clone(), plain(other))],
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub(crate) fn follow(&self) -> bool {
        self.opts.get("follow").and_then(Value::as_bool).unwrap_or(false)
    }

    pub(crate) fn timeout(&self, default: Duration) -> Duration {
        self.opts
            .get("timeout")
            .and_then(Value::as_f64)
            .filter(|seconds| *seconds > 0.0)
            .map_or(default, Duration::from_secs_f64)
    }

    /// The URL with `params` as its query.
    pub(crate) fn full_url(&self) -> Result<String, String> {
        let mut url = reqwest::Url::parse(&self.url).map_err(|err| format!("{}: {err}", self.url))?;
        let params = self.pairs("params");
        if !params.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(params.iter().map(|(key, value)| (key.as_str(), value.as_str())));
        }
        Ok(url.to_string())
    }
}

/// A JSON value as a parameter: text as it is, anything else as JSON.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Send `ask` on the session, following redirects when it says so, and take each answer's cookies and token.
pub(crate) fn send(
    client: &reqwest::blocking::Client,
    session: &mut Session,
    ask: &Ask,
    default_timeout: Duration,
) -> Result<Answer, String> {
    let mut url = ask.full_url()?;
    let mut method = ask.method.to_ascii_uppercase();
    let mut with_body = true;
    let timeout = ask.timeout(default_timeout);
    let started = Instant::now();
    let mut hops = 0;
    loop {
        let answer = send_once(client, session, ask, &method, &url, with_body, timeout, started)?;
        let content_type = answer.header("content-type").unwrap_or("").to_string();
        session.absorb(&answer.url, &answer.headers, &content_type, &answer.body);
        let redirect = matches!(answer.status, 301 | 302 | 303 | 307 | 308);
        let location = answer.header("location").map(str::to_string);
        match location {
            Some(location) if redirect && ask.follow() && hops < MAX_REDIRECTS => {
                hops += 1;
                url = reqwest::Url::parse(&answer.url)
                    .and_then(|base| base.join(&location))
                    .map_err(|err| format!("redirect to {location}: {err}"))?
                    .to_string();
                if answer.status == 303 || (matches!(answer.status, 301 | 302) && method != "GET" && method != "HEAD") {
                    method = "GET".to_string();
                    with_body = false;
                }
            }
            _ => return Ok(answer),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn send_once(
    client: &reqwest::blocking::Client,
    session: &Session,
    ask: &Ask,
    method: &str,
    url: &str,
    with_body: bool,
    timeout: Duration,
    started: Instant,
) -> Result<Answer, String> {
    let verb = reqwest::Method::from_bytes(method.as_bytes()).map_err(|err| format!("{method}: {err}"))?;
    let mut request = client.request(verb, url).timeout(timeout);
    let given = ask.pairs("headers");
    let has = |name: &str| given.iter().any(|(key, _)| key.eq_ignore_ascii_case(name));
    for (name, value) in &session.headers {
        if !has(name) {
            request = request.header(name, value);
        }
    }
    for (name, value) in &given {
        request = request.header(name, value);
    }
    if !has("cookie") {
        if let Some(cookie) = session.cookie_header(url) {
            request = request.header("Cookie", cookie);
        }
    }
    if matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
        let token = match ask.opts.get("csrf") {
            Some(Value::Bool(false)) => None,
            Some(Value::String(text)) => Some(text.clone()),
            _ => session.token.clone(),
        };
        if let Some(token) = token {
            request = request.header("X-CSRF-Token", token);
        }
    }
    match ask.text("accept").as_deref() {
        Some("json") => request = request.header("Accept", "application/json"),
        Some("html") => request = request.header("Accept", "text/html"),
        Some(other) => request = request.header("Accept", other),
        None => {}
    }
    if let Some(Value::Array(pair)) = ask.opts.get("basic") {
        let user = pair.first().map(plain).unwrap_or_default();
        let password = pair.get(1).map(plain).unwrap_or_default();
        request = request.basic_auth(user, Some(password));
    }
    if let Some(key) = ask.text("apiKey") {
        request = request.header("api_key", key);
    }
    if let Some(key) = ask.text("loginKey") {
        request = request.header("login_key", key);
    }
    if with_body {
        request = body(request, ask, &has)?;
    }
    let response = request
        .send()
        .map_err(|err| format!("{method} {url}: {}", error_chain(&err)))?;
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).to_string()))
        .collect();
    let (body, length) = read_body(response).map_err(|err| format!("{method} {url}: {err}"))?;
    Ok(Answer {
        status,
        url: final_url,
        headers,
        body,
        length,
        elapsed: started.elapsed(),
    })
}

/// The request body of `form`, `json`, `body` or `multipart`.
fn body(
    request: reqwest::blocking::RequestBuilder,
    ask: &Ask,
    has: &dyn Fn(&str) -> bool,
) -> Result<reqwest::blocking::RequestBuilder, String> {
    if ask.opts.get("form").is_some_and(|form| !form.is_null()) {
        let pairs = ask.pairs("form");
        let mut url = reqwest::Url::parse("http://form/").map_err(|err| err.to_string())?;
        url.query_pairs_mut()
            .extend_pairs(pairs.iter().map(|(key, value)| (key.as_str(), value.as_str())));
        let text = url.query().unwrap_or("").to_string();
        let request = if has("content-type") {
            request
        } else {
            request.header("Content-Type", "application/x-www-form-urlencoded")
        };
        return Ok(request.body(text));
    }
    if let Some(value) = ask.opts.get("json").filter(|value| !value.is_null()) {
        let request = if has("content-type") {
            request
        } else {
            request.header("Content-Type", "application/json")
        };
        return Ok(request.body(value.to_string()));
    }
    if let Some(Value::Object(parts)) = ask.opts.get("multipart") {
        let mut form = reqwest::blocking::multipart::Form::new();
        for (name, part) in parts {
            form = match part {
                Value::Object(file) => {
                    let bytes = match (file.get("base64").and_then(Value::as_str), file.get("text")) {
                        (Some(encoded), _) => {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map_err(|err| format!("multipart {name}: base64: {err}"))?
                        }
                        (None, Some(text)) => plain(text).into_bytes(),
                        (None, None) => Vec::new(),
                    };
                    let mut piece = reqwest::blocking::multipart::Part::bytes(bytes);
                    if let Some(file_name) = file.get("fileName").and_then(Value::as_str) {
                        piece = piece.file_name(file_name.to_string());
                    }
                    if let Some(content_type) = file.get("contentType").and_then(Value::as_str) {
                        piece = piece
                            .mime_str(content_type)
                            .map_err(|err| format!("multipart {name}: {err}"))?;
                    }
                    form.part(name.clone(), piece)
                }
                other => form.text(name.clone(), plain(other)),
            };
        }
        return Ok(request.multipart(form));
    }
    match ask.text("body") {
        Some(text) => Ok(request.body(text)),
        None => Ok(request),
    }
}

/// The body up to [`MAX_BODY`] and its full length.
fn read_body(mut response: reqwest::blocking::Response) -> std::io::Result<(Vec<u8>, usize)> {
    use std::io::Read as _;
    let mut kept = Vec::new();
    let mut length = 0;
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = response.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        length += read;
        let room = MAX_BODY.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..read.min(room)]);
    }
    Ok((kept, length))
}

/// The error and its causes, as reqwest's own text leaves the cause out.
pub(crate) fn error_chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> reqwest::Url {
        reqwest::Url::parse(text).unwrap()
    }

    #[test]
    fn the_server_url_joins_paths_and_plain_http_is_for_this_machine() {
        let server = Server::parse("http://localhost:8080/", false).unwrap();
        assert_eq!(server.url, "http://localhost:8080");
        assert_eq!(
            (server.host.as_str(), server.port, server.secure),
            ("localhost", 8080, false)
        );
        assert_eq!(server.join("/Login"), "http://localhost:8080/Login");
        assert_eq!(server.join("rest/s1"), "http://localhost:8080/rest/s1");
        assert_eq!(server.join("https://other.example/x"), "https://other.example/x");
        let nested = Server::parse("https://erp.example.com/moqui/", false).unwrap();
        assert_eq!(nested.join("/Login"), "https://erp.example.com/moqui/Login");
        assert_eq!((nested.port, nested.secure), (443, true));
        assert!(Server::parse("http://127.0.0.1:8081", false).is_ok());
        assert_eq!(Server::parse("http://[::1]:8081", false).unwrap().host, "::1");
        let refused = Server::parse("http://erp.example.com", false).unwrap_err();
        assert!(refused.to_string().contains("--allow-http"), "{refused}");
        assert!(Server::parse("http://erp.example.com", true).is_ok());
        assert!(Server::parse("ftp://localhost", true).is_err());
        assert!(is_loopback("127.0.0.2") && is_loopback("LOCALHOST") && !is_loopback("10.0.0.1"));
    }

    #[test]
    fn set_cookie_attributes_parse() {
        let cookie =
            SetCookie::parse("moqui.session=abc.def; Path=/; HttpOnly; SameSite=Lax; Max-Age=3600; Secure").unwrap();
        assert_eq!(cookie.name, "moqui.session");
        assert_eq!(cookie.value, "abc.def");
        assert_eq!(cookie.path.as_deref(), Some("/"));
        assert!(cookie.http_only && cookie.secure);
        assert_eq!(cookie.same_site.as_deref(), Some("Lax"));
        assert_eq!(cookie.max_age, Some(3600));
        let json = cookie.to_json();
        assert_eq!(json["httpOnly"], true);
        assert_eq!(json["sameSite"], "Lax");
        let dated = SetCookie::parse("a=\"1\"; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Domain=.Example.com").unwrap();
        assert_eq!(dated.value, "1");
        assert_eq!(dated.domain.as_deref(), Some("example.com"));
        assert_eq!(expires_seconds(dated.expires.as_deref().unwrap()), Some(1_445_412_480));
        assert_eq!(expires_seconds("Wed, 21-Oct-2015 07:28:00 GMT"), Some(1_445_412_480));
        assert!(SetCookie::parse("=nameless").is_none());
        assert!(SetCookie::parse("novalue").is_none());
    }

    #[test]
    fn the_jar_matches_domain_path_expiry_and_secure() {
        let mut jar = CookieJar::default();
        let now = 1_000_000;
        let site = url("http://shop.example.com/apps/Order/Find");
        jar.store(&site, &SetCookie::parse("session=1; Path=/").unwrap(), now);
        jar.store(&site, &SetCookie::parse("local=2").unwrap(), now);
        jar.store(
            &site,
            &SetCookie::parse("wide=3; Domain=example.com; Path=/apps").unwrap(),
            now,
        );
        jar.store(&site, &SetCookie::parse("other=4; Domain=other.com").unwrap(), now);
        jar.store(&site, &SetCookie::parse("brief=5; Max-Age=10").unwrap(), now);
        jar.store(&site, &SetCookie::parse("tls=6; Secure").unwrap(), now);
        // the default path is the directory of the request path
        assert_eq!(
            jar.header(&url("http://shop.example.com/apps/Order/Find"), now)
                .as_deref(),
            Some("local=2; brief=5; wide=3; session=1")
        );
        assert_eq!(
            jar.header(&url("http://shop.example.com/Login"), now).as_deref(),
            Some("session=1")
        );
        assert_eq!(
            jar.header(&url("http://api.example.com/apps"), now).as_deref(),
            Some("wide=3")
        );
        assert_eq!(jar.header(&url("http://example.com/applications"), now), None);
        assert_eq!(
            jar.header(&url("https://shop.example.com/apps/Order/x"), now + 20)
                .as_deref(),
            Some("local=2; tls=6; wide=3; session=1")
        );
        // a later cookie of the same name, domain and path replaces it; a past expiry removes it
        jar.store(&site, &SetCookie::parse("session=7; Path=/").unwrap(), now);
        assert_eq!(
            jar.header(&url("http://shop.example.com/"), now).as_deref(),
            Some("session=7")
        );
        jar.store(&site, &SetCookie::parse("session=; Path=/; Max-Age=0").unwrap(), now);
        assert_eq!(jar.header(&url("http://shop.example.com/"), now), None);
        // a Secure cookie goes to this machine over http, as a browser sends it
        let local = url("http://localhost:8080/");
        jar.store(
            &local,
            &SetCookie::parse("moqui.session=s; Path=/; Secure").unwrap(),
            now,
        );
        assert_eq!(jar.header(&local, now).as_deref(), Some("moqui.session=s"));
        jar.clear();
        assert_eq!(jar.header(&local, now), None);
    }

    #[test]
    fn the_session_token_comes_from_headers_meta_or_the_shell_config() {
        let headers = vec![("moquiSessionToken".to_string(), "TOK1".to_string())];
        assert_eq!(
            session_token(&headers, "application/json", b"{}").as_deref(),
            Some("TOK1")
        );
        let csrf = vec![("X-CSRF-Token".to_string(), "TOK2".to_string())];
        assert_eq!(session_token(&csrf, "", b"").as_deref(), Some("TOK2"));
        let page = br#"<html><head><meta charset="utf-8"><meta content="TOK3" name="moquiSessionToken"></head></html>"#;
        assert_eq!(
            session_token(&[], "text/html; charset=UTF-8", page).as_deref(),
            Some("TOK3")
        );
        let shell = br#"<body><script type="application/json" id="moqui-config">{"basePath":"/qapps","session":{"moquiSessionToken":"TOK4"}}</script></body>"#;
        assert_eq!(session_token(&[], "text/html", shell).as_deref(), Some("TOK4"));
        assert_eq!(session_token(&[], "application/json", page), None);
        assert_eq!(session_token(&[], "text/html", b"<html></html>"), None);
        let mut session = Session::default();
        let set = vec![
            (
                "Set-Cookie".to_string(),
                "moqui.session=x; Path=/; HttpOnly".to_string(),
            ),
            ("X-CSRF-Token".to_string(), "TOK5".to_string()),
        ];
        session.absorb("http://localhost:8080/rest/login", &set, "application/json", b"{}");
        assert_eq!(session.token.as_deref(), Some("TOK5"));
        assert_eq!(
            session.cookie_header("http://localhost:8080/apps").as_deref(),
            Some("moqui.session=x")
        );
    }

    #[test]
    fn magic_is_text_or_hex() {
        assert_eq!(magic(b"%PDF-1.7"), "%PDF");
        assert_eq!(magic(&[0x89, b'P', b'N', b'G']), "89504e47");
        assert_eq!(magic(b""), "");
    }
}
