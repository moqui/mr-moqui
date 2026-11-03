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

//! The node in process on the app scheme `moqui-node` (`http://moqui-node.localhost`), with no port
//! (plans/Mobile-Desktop.md §4 configuration 2, "One transport seam"): each request of the window goes to the node's
//! router in this process (`moqui_host::in_process`), on a worker thread (`crate::scheme`). The host keeps the node's
//! cookies for the window ([`super::cookies`]), so the pages sign in and keep a session as on the web.
//!
//! What a page sends is passed on but for what only the host may say: the cookies, the host, the forwarded-for and edge
//! link headers, and the encodings (the host hands the page a whole, plain body). A request another site started is
//! refused, but for a window's navigation. An attachment or a large file the window opens is saved by the host (`crate::download`); an event stream
//! cannot pass a scheme that answers whole bodies, and is refused, so the page uses its catch-up instead.

use moqui_client::proxy::{cross_site, header, response_headers};
use moqui_host::in_process::InProcess;
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager, UriSchemeContext, UriSchemeResponder};

use super::cookies::Jar;
use crate::scheme::{document_request, has_script_policy, plain, run};

/// The node's scheme, and its origin on Chromium.
pub(crate) const NODE_SCHEME: &str = "moqui-node";
pub(crate) const NODE_ORIGIN: &str = "http://moqui-node.localhost";
const NODE_HOST: &str = "moqui-node.localhost";

/// Request headers only the host may set, and the encodings it does not take.
const HOST_HEADERS: [&str; 16] = [
    "cookie",
    "host",
    "connection",
    "keep-alive",
    "upgrade",
    "te",
    "transfer-encoding",
    "content-length",
    "accept-encoding",
    "forwarded",
    "x-real-ip",
    "proxy-authorization",
    "login_key",
    "api_key",
    "x-moqui-tunnel",
    "proxy-connection",
];

/// The node in process and its cookies, on the app while it runs a node in process.
pub(crate) struct Local {
    pub(crate) jar: Jar,
    node: std::sync::OnceLock<InProcess>,
}

impl Local {
    pub(crate) fn new(jar: Jar) -> Self {
        Self {
            jar,
            node: std::sync::OnceLock::new(),
        }
    }

    /// The node booted: its app in process.
    pub(crate) fn started(&self, node: InProcess) {
        let _ = self.node.set(node);
    }

    pub(crate) fn node(&self) -> Option<&InProcess> {
        self.node.get()
    }
}

/// The scheme's handler, registered in every mode: with no node in process here it answers 404.
pub(crate) fn handle(
    context: UriSchemeContext<'_, tauri::DynRuntime>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    run(Box::new(move || {
        let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| answer(&app, request)));
        responder.respond(answered.unwrap_or_else(|_| {
            tracing::error!("A node scheme request panicked");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered")
        }));
    }));
}

/// Whether a request header goes on to the node: not one only the host sets, nor a forwarded-for or edge link one.
pub(crate) fn passed_on(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !HOST_HEADERS.contains(&name.as_str()) && !name.starts_with("x-forwarded-") && !name.starts_with("x-moqui-edge")
}

/// Whether a request is a window's `GET` (or `HEAD`) navigation to a `document`, which may come from another site:
/// the app's own splash page going to the node (with its `Origin`; Chromium sends no `Sec-Fetch-*` to a custom
/// scheme), as a link to a page does on the web. A page's other requests from another site are refused.
pub(crate) fn navigation(method: &str, document: bool) -> bool {
    document && matches!(method, "GET" | "HEAD")
}

fn answer(app: &AppHandle, request: tauri::http::Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(local) = app.try_state::<Local>() else {
        return plain(StatusCode::NOT_FOUND, "No node in this app");
    };
    let Some(node) = local.node() else {
        return plain(StatusCode::SERVICE_UNAVAILABLE, "The node is starting");
    };
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| "/".to_string(), |path| path.as_str().to_string());
    if !path.starts_with('/') || path.starts_with("//") || path.chars().any(char::is_control) {
        return plain(StatusCode::BAD_REQUEST, "Not a path on the node");
    }
    let document = document_request(request.headers());
    let sent: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
        .collect();
    if cross_site(&sent, NODE_ORIGIN) && !navigation(request.method().as_str(), document) {
        tracing::warn!("{path}: refused, a request from another site");
        return plain(StatusCode::FORBIDDEN, "A request from another site");
    }
    let mut headers: Vec<(String, String)> = sent.into_iter().filter(|(name, _)| passed_on(name)).collect();
    headers.push(("Host".to_string(), NODE_HOST.to_string()));
    if let Some(cookie) = local.jar.header() {
        headers.push(("Cookie".to_string(), cookie));
    }
    let method = request.method().as_str().to_string();
    let answer = match node.request(&method, &path, &headers, request.into_body()) {
        Ok(answer) => answer,
        Err(err) => return plain(StatusCode::SERVICE_UNAVAILABLE, &err.to_string()),
    };
    local.jar.take(
        answer
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value.as_str()),
    );
    let bare = path.split_once('?').map_or(path.as_str(), |(bare, _)| bare).to_string();
    if answer
        .header("content-type")
        .is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/event-stream"))
    {
        tracing::debug!("{bare}: an event stream is not served through the app scheme");
        // 503, as a node with too many streams answers: the Assist drawer polls events.json then
        return plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "No event streams in the app; the page catches up instead",
        );
    }
    if let Some(name) = crate::download::saved_by_host(document, answer.status, &answer.headers, &bare) {
        crate::download::save(app, name, move |out| answer.copy_to(out).map_err(|err| err.to_string()));
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
    }
    let status = answer.status;
    let shown = page_headers(&answer.headers);
    let body = match answer.bytes() {
        Ok(body) => body,
        Err(err) => {
            return plain(
                StatusCode::BAD_GATEWAY,
                &format!("The node's answer was cut off: {err}"),
            )
        }
    };
    let html = header(&shown, "content-type").is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/html"));
    let mut response = Response::builder().status(status);
    for (name, value) in &shown {
        if html && name.eq_ignore_ascii_case("content-security-policy") && !has_script_policy(value) {
            continue;
        }
        // Chromium puts the app's initialization scripts into every HTML answer of the scheme: a page's fetch of a
        // screen's HTML is handed on as text, so it is not changed
        if html && !document && name.eq_ignore_ascii_case("content-type") {
            response = response.header("Content-Type", "text/plain; charset=utf-8");
            continue;
        }
        response = response.header(name.as_str(), value.as_str());
    }
    response
        .body(body)
        .unwrap_or_else(|_| plain(StatusCode::BAD_GATEWAY, "The node's answer could not be passed on"))
}

/// The answer's headers the page gets (no cookies, lengths or encodings), with a redirect to the node's own loopback
/// address pointed at the scheme's origin: a page or link the node writes with its configured host and port.
pub(crate) fn page_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    let port = moqui_lib::env::get_prop_or_env("webapp_http_port");
    let port = port.trim();
    let shown = response_headers(headers, &format!("http://127.0.0.1:{port}"), NODE_ORIGIN);
    response_headers(&shown, &format!("http://localhost:{port}"), NODE_ORIGIN)
}

#[cfg(test)]
#[path = "scheme_tests.rs"]
mod tests;
