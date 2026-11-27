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

//! A server page's request to the web through the app (`moquiDesktop.webRequest`): a page that runs a script it does
//! not trust (a plugin in a sandboxed frame) relays the script's requests here, with the hosts the script may call.
//! The WebView's CORS does not apply, so the page can read an API that sends no CORS headers.
//!
//! The limits, checked here and not only in the page:
//! - **https only**, on port 443, with no user or password in the URL;
//! - **the host allow-list**: the URL's host must be one of `hosts` (`*.example.com` takes the subdomains of
//!   example.com, not example.com); no IP address, no `localhost` and none of the app's own hosts;
//! - **no cookies**: no cookie store, a `Cookie` header is dropped, and `Set-Cookie` is not handed back;
//! - each redirect is checked again (at most [`REDIRECTS_MAX`]);
//! - at most [`BODY_MAX`] bytes each way and [`TIMEOUT_SECONDS`] in all.
//!
//! Only a server page (`moqui-app`) in the main webview may ask (`commands::web_request`); the shell has no use for it.

use std::io::Read;
use std::time::Duration;

use serde::Serialize;
use tauri::Url;

/// The most hosts one request names.
pub(crate) const HOSTS_MAX: usize = 32;
/// The most bytes of a request or response body.
pub(crate) const BODY_MAX: usize = 8 * 1024 * 1024;
/// The most redirects one request follows.
pub(crate) const REDIRECTS_MAX: usize = 5;
/// The time limit of one request, redirects and the body included.
pub(crate) const TIMEOUT_SECONDS: u64 = 30;
const URL_MAX: usize = 8192;
const HEADERS_MAX: usize = 64;

/// The answer to the page: the status, the headers (lower-case names, no `Set-Cookie`), the body as text (UTF-8,
/// lossy) and the URL after redirects.
#[derive(Debug, Serialize)]
pub(crate) struct WebAnswer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub url: String,
}

/// One allow-list entry, checked: a lower-case DNS name with at least one dot, or `*.` and one.
pub(crate) fn host_entry(entry: &str) -> Result<String, String> {
    let entry = entry.trim().to_ascii_lowercase();
    let name = entry.strip_prefix("*.").unwrap_or(&entry);
    let labels_ok = name.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    });
    let all_digits = name.split('.').all(|label| label.chars().all(|c| c.is_ascii_digit()));
    if name.len() > 253 || !name.contains('.') || !labels_ok || all_digits || crate::host::own_host(name) {
        return Err(format!("Not a host the page may name: {entry}"));
    }
    Ok(entry)
}

/// The allow-list, checked: 1 to [`HOSTS_MAX`] entries.
pub(crate) fn host_list(hosts: &[String]) -> Result<Vec<String>, String> {
    if hosts.is_empty() || hosts.len() > HOSTS_MAX {
        return Err(format!("Name 1 to {HOSTS_MAX} hosts"));
    }
    hosts.iter().map(|entry| host_entry(entry)).collect()
}

/// Whether `host` (a URL's, lower case) is on the checked allow-list.
pub(crate) fn host_allowed(host: &str, allowed: &[String]) -> bool {
    allowed.iter().any(|entry| match entry.strip_prefix("*.") {
        Some(parent) => host.len() > parent.len() + 1 && host.ends_with(parent) && host[..host.len() - parent.len()].ends_with('.'),
        None => host == entry,
    })
}

/// `text` as a URL the request may go to: https, port 443, no user or password, a DNS host on the allow-list.
pub(crate) fn allowed_url(text: &str, allowed: &[String]) -> Result<Url, String> {
    if text.len() > URL_MAX {
        return Err("The URL is too long".to_string());
    }
    let url = Url::parse(text.trim()).map_err(|err| format!("Not a URL: {err}"))?;
    if url.scheme() != "https" {
        return Err("Only https".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("No user or password in the URL".to_string());
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err("Only port 443".to_string());
    }
    // domain() is none for an IP address
    match url.domain().map(str::to_ascii_lowercase) {
        Some(host) if host_allowed(&host, allowed) => Ok(url),
        Some(host) => Err(format!("The host {host} is not on the list")),
        None => Err("The URL must name a host, not an address".to_string()),
    }
}

/// Whether the page may set the request header `name` (lower case): not the cookies, the host, the body's framing or
/// the connection's.
pub(crate) fn header_allowed(name: &str) -> bool {
    !matches!(
        name,
        "cookie" | "cookie2" | "host" | "content-length" | "transfer-encoding" | "connection" | "keep-alive" | "upgrade"
            | "te" | "trailer" | "expect"
    ) && !name.starts_with("proxy-")
        && !name.starts_with("sec-")
}

/// The method, checked: GET, HEAD or POST.
pub(crate) fn method(text: &str) -> Result<reqwest::Method, String> {
    match text.trim().to_ascii_uppercase().as_str() {
        "" | "GET" => Ok(reqwest::Method::GET),
        "HEAD" => Ok(reqwest::Method::HEAD),
        "POST" => Ok(reqwest::Method::POST),
        other => Err(format!("Not a method the page may use: {other}")),
    }
}

/// The request, with its checks: see the module's text. Blocking: the command runs it off the main thread.
pub(crate) fn request(
    method_text: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    hosts: &[String],
) -> Result<WebAnswer, String> {
    let allowed = host_list(hosts)?;
    let mut url = allowed_url(url, &allowed)?;
    let mut method = method(method_text)?;
    if headers.len() > HEADERS_MAX {
        return Err(format!("At most {HEADERS_MAX} headers"));
    }
    let mut body = body.map(str::to_string);
    if body.as_ref().is_some_and(|body| body.len() > BODY_MAX) {
        return Err("The body is too long".to_string());
    }
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(TIMEOUT_SECONDS))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| format!("HTTP client: {err}"))?;
    for _ in 0..=REDIRECTS_MAX {
        let mut builder = client.request(method.clone(), url.clone());
        for (name, value) in headers {
            let name = name.trim().to_ascii_lowercase();
            if header_allowed(&name) {
                builder = builder.header(name.as_str(), value.as_str());
            }
        }
        if let Some(body) = &body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().map_err(|err| err.without_url().to_string())?;
        let status = response.status();
        if status.is_redirection() {
            if let Some(location) = response.headers().get(reqwest::header::LOCATION) {
                let location = location.to_str().map_err(|_| "A redirect to a URL that is not text".to_string())?;
                let next = url.join(location).map_err(|err| format!("A redirect to no URL: {err}"))?;
                url = allowed_url(next.as_str(), &allowed)?;
                // 307 and 308 keep the method and body; the rest are a GET
                if !matches!(status.as_u16(), 307 | 308) {
                    method = reqwest::Method::GET;
                    body = None;
                }
                continue;
            }
        }
        let answer_headers = response
            .headers()
            .iter()
            .filter(|(name, _)| !matches!(name.as_str(), "set-cookie" | "set-cookie2"))
            .filter_map(|(name, value)| value.to_str().ok().map(|value| (name.as_str().to_string(), value.to_string())))
            .collect();
        let mut bytes = Vec::new();
        response
            .take(BODY_MAX as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| format!("The answer was not read: {err}"))?;
        if bytes.len() > BODY_MAX {
            return Err("The answer is too long".to_string());
        }
        return Ok(WebAnswer {
            status: status.as_u16(),
            headers: answer_headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
            url: url.to_string(),
        });
    }
    Err(format!("More than {REDIRECTS_MAX} redirects"))
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
