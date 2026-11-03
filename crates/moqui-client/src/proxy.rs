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

//! A page's request forwarded to the server (plans/Mobile-Desktop.md §4, "One transport seam"). The page is on the
//! host's app scheme; its request goes to the same path on the account's server with the device key as the
//! `login_key` header, so the page has one origin, sends no cookies, needs no CSRF token (a header sign-in skips the
//! check) and never holds the key.
//!
//! Only plain request headers pass: the page cannot send a cookie, credentials of its own or an `Origin`. In the
//! answer the cookies are dropped and a redirect to the server is turned back into the app scheme's origin.
//!
//! The server lets a request with no `Origin` through, so the proxy is the cross-site check: a request another site
//! started (its `Origin` or `Sec-Fetch-Site` says so) is refused before it gets the key.

use std::time::Duration;

use moqui_lib::error::Error;

use crate::http::client_with;

/// Minutes a forwarded request may take with its body: a large download or upload is slow, not stuck.
const TIMEOUT_MINUTES: u64 = 10;

/// Request headers the page may send on; others are dropped. The `x-moqui-` ones are those the client sends; the edge
/// link's (`x-moqui-edge-*`, `x-moqui-tunnel`) are not among them.
const REQUEST_HEADERS: [&str; 14] = [
    "accept",
    "accept-language",
    "content-type",
    "range",
    "if-range",
    "if-none-match",
    "if-modified-since",
    "x-requested-with",
    "x-moqui-page-trace",
    "x-moqui-client-modes",
    "x-moqui-qvt",
    "x-moqui-preferences",
    "x-moqui-preview",
    "x-moqui-aide",
];

/// The methods a page may send.
const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// Response headers the page does not get: the cookies (the key is the session), the hop-by-hop ones, the body's
/// length and encoding (the host hands the page the whole, decoded body), a Basic challenge (the page has no password
/// to give) and, by prefix, the CORS grants (no other site reads through the app's origin).
const DROPPED_RESPONSE_HEADERS: [&str; 10] = [
    "set-cookie",
    "set-cookie2",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
    "content-encoding",
    "strict-transport-security",
    "alt-svc",
    "www-authenticate",
];

/// A request of the page.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    /// The path and query on the app scheme, `/apps/x?y=1`.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The server's answer, its headers already made safe for the page, with the body still to read.
pub struct Upstream {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    response: reqwest::blocking::Response,
}

impl Upstream {
    /// The whole body.
    ///
    /// # Errors
    ///
    /// Returns the transport's error when the body does not arrive.
    pub fn body(self) -> Result<Vec<u8>, Error> {
        self.response
            .bytes()
            .map(|bytes| bytes.to_vec())
            .map_err(|err| Error::new(err.to_string()))
    }

    /// Stream the body to `out`, for a download kept out of memory; answers the bytes written.
    ///
    /// # Errors
    ///
    /// Returns the transport's or the writer's error.
    pub fn copy_to(mut self, out: &mut dyn std::io::Write) -> Result<u64, Error> {
        self.response.copy_to(out).map_err(|err| Error::new(err.to_string()))
    }

    /// The file name of an attachment (`Content-Disposition: attachment`), which the host saves itself rather than
    /// hand the page a whole download in memory.
    pub fn attachment_name(&self) -> Option<String> {
        let disposition = header(&self.headers, "content-disposition")?;
        attachment_name(disposition)
    }
}

/// Forwards requests to the servers of the accounts.
pub struct Proxy {
    http: reqwest::blocking::Client,
}

impl Proxy {
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be made.
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            http: client_with(Duration::from_secs(TIMEOUT_MINUTES * 60))?,
        })
    }

    /// Forward `request` to `base`, with `key` when the account is signed in. `origin` is the app scheme's origin
    /// (`http://moqui-app.localhost`), where a redirect to the server is pointed back.
    ///
    /// # Errors
    ///
    /// Returns an error for a request another site started, a method a page does not send, a path that is not one on
    /// the server, or the transport's when the server does not answer.
    pub fn send(&self, base: &str, key: Option<&str>, origin: &str, request: Request) -> Result<Upstream, Error> {
        if cross_site(&request.headers, origin) {
            return Err(Error::new(format!("a request from another site: {}", request.path)));
        }
        let url = upstream_url(base, &request.path)?;
        let method_name = request.method.to_ascii_uppercase();
        if !METHODS.contains(&method_name.as_str()) {
            return Err(Error::new(format!("not a method a page sends: {}", request.method)));
        }
        let method = reqwest::Method::from_bytes(method_name.as_bytes())
            .map_err(|err| Error::new(format!("{}: {err}", request.method)))?;
        let mut builder = self.http.request(method, &url);
        for (name, value) in request_headers(&request.headers) {
            builder = builder.header(name, value);
        }
        if let Some(key) = key {
            builder = builder.header("login_key", key);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        let response = builder.send().map_err(|err| Error::new(format!("{url}: {err}")))?;
        let status = response.status().as_u16();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
            .collect();
        Ok(Upstream {
            status,
            headers: response_headers(&headers, base, origin),
            response,
        })
    }
}

/// Whether another site started the request: its `Origin` is not the app's, or its `Sec-Fetch-Site` is not
/// `same-origin` or `none` (typed in, or the app's own navigation). A request with neither is the app's.
pub fn cross_site(headers: &[(String, String)], origin: &str) -> bool {
    let other_origin = header(headers, "origin").is_some_and(|sent| !sent.eq_ignore_ascii_case(origin));
    let other_site = header(headers, "sec-fetch-site")
        .is_some_and(|site| !site.eq_ignore_ascii_case("same-origin") && !site.eq_ignore_ascii_case("none"));
    other_origin || other_site
}

/// The server url of a page's path: `base` and the path, which must be an absolute path on it (not `//host`, not a
/// url of its own), so a page can reach no other server through the proxy.
///
/// # Errors
///
/// Returns an error for a path that is not an absolute path.
pub fn upstream_url(base: &str, path: &str) -> Result<String, Error> {
    let plain = path.starts_with('/') && !path.starts_with("//") && !path.contains('\\') && !path.contains('#');
    if !plain || path.chars().any(char::is_control) {
        return Err(Error::new(format!("not a path on the server: {path}")));
    }
    Ok(format!("{base}{path}"))
}

/// The request headers the page may send on.
pub fn request_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| REQUEST_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
        .cloned()
        .collect()
}

/// The response headers the page gets, with a redirect to the server pointed at `origin`.
pub fn response_headers(headers: &[(String, String)], base: &str, origin: &str) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            !DROPPED_RESPONSE_HEADERS.contains(&name.as_str()) && !name.starts_with("access-control-")
        })
        .map(|(name, value)| {
            if name.eq_ignore_ascii_case("location") {
                (name.clone(), rewrite_location(value, base, origin))
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

/// A `Location` on the server as the same path under `origin`; one elsewhere (an SSO provider) as it is. The server
/// is matched by host and port, http or https alike (a server behind a TLS proxy may name itself with http), and its
/// base path (`https://host/moqui`) is taken off, as the page's paths do not carry it.
pub fn rewrite_location(location: &str, base: &str, origin: &str) -> String {
    // a path relative to the request resolves on the app's origin as on the server's
    let relative = !location.starts_with('/') && reqwest::Url::parse(location).is_err();
    let Ok(base_url) = reqwest::Url::parse(base) else {
        return location.to_string();
    };
    if relative {
        return location.to_string();
    }
    let Ok(target) = base_url.join(location) else {
        return location.to_string();
    };
    let web = |url: &reqwest::Url| matches!(url.scheme(), "http" | "https");
    let same_server = web(&target)
        && target.host_str().map(str::to_ascii_lowercase) == base_url.host_str().map(str::to_ascii_lowercase)
        && port_of(&target) == port_of(&base_url);
    if !same_server {
        return location.to_string();
    }
    // a path of the server outside its base path is no path of the page's: the server's own url, which opens outside
    let base_path = base_url.path().trim_end_matches('/');
    let Some(rest) = target.path().strip_prefix(base_path) else {
        return target.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return target.to_string();
    }
    let mut out = format!("{origin}{}", if rest.is_empty() { "/" } else { rest });
    if let Some(query) = target.query() {
        out.push('?');
        out.push_str(query);
    }
    if let Some(fragment) = target.fragment() {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// The port of a url as the server is reached: the one given, else the default of https, so `http://host` and
/// `https://host` match (a TLS proxy in front) and `https://host:443` matches `https://host`.
fn port_of(url: &reqwest::Url) -> u16 {
    url.port().unwrap_or(443)
}

/// The value of a header, by name in any case.
pub fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// The file name of an `attachment` disposition (`filename*=UTF-8''…` first, then `filename=`), as a plain name with
/// no directory in it; `download` when it names none.
#[must_use]
pub fn attachment_name(disposition: &str) -> Option<String> {
    let mut parts = disposition.split(';').map(str::trim);
    if !parts.next()?.eq_ignore_ascii_case("attachment") {
        return None;
    }
    let mut plain = None;
    let mut extended = None;
    for part in parts {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match name.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                let encoded = value.split_once("''").map_or(value, |(_, rest)| rest);
                extended = Some(percent_decode(encoded));
            }
            "filename" => plain = Some(value.to_string()),
            _ => {}
        }
    }
    let name = extended.or(plain).unwrap_or_default();
    let name = name.rsplit(['/', '\\']).next().unwrap_or("");
    // a name any desktop can save: no characters Windows refuses, no leading dot, no trailing dot or space
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_start_matches('.').trim_end_matches(['.', ' ']);
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered = stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.ends_with(|c: char| c.is_ascii_digit());
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered;
    Some(match (name.is_empty(), reserved) {
        (true, _) => "download".to_string(),
        (false, true) => format!("_{name}"),
        (false, false) => name.to_string(),
    })
}

/// `%XX` decoded as UTF-8, lossy.
fn percent_decode(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        if first == b'%' {
            if let Some(byte) = tail
                .get(..2)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                bytes.push(byte);
                rest = &tail[2..];
                continue;
            }
        }
        bytes.push(first);
        rest = tail;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
