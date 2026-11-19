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

//! The app schemes on Android's WebView (plans/Mobile-Desktop.md §4 "One transport seam"), answered by
//! `moqui_client_tauri::scheme` as on the desktop, from the shell's files embedded in the app ([`EmbeddedShell`]).
//! What Android's WebView needs beside:
//!
//! - **No request bodies.** Its request interception sees a request's headers but not its body, so a page's `fetch`
//!   with a body comes to the host as a command instead ([`scheme_fetch`], the bridge's `fetch`), answered the same
//!   way: the shell's with its load's nonce, and with where it comes from set by the host, not the caller (SEC-CLI-06).
//! - **No redirect status.** A `WebResourceResponse` refuses 3xx, so a document's redirect is answered as a page that
//!   goes there ([`MobileHooks`]); the bridge's `fetch` follows a redirect itself.
//! - **One init script for every page.** A server page's storage scope and the scopes to clear are asked of the host
//!   at its start ([`PAGE_PATH`]), as an app window's init script has them on the desktop.
//! - **No `Sec-Fetch-*`.** The scheme hears whether the page itself asked from two marks, the main frame's document
//!   (wry's patch) and the bridge's on the page's own GETs, and tells the server the rest came from another site
//!   ([`page_site`], SEC-CLI-102).

use std::borrow::Cow;
use std::collections::HashMap;

use base64::Engine;
use moqui_client::assets::{asset, Asset, Assets};
use moqui_client::proxy::site_by_origin;
use moqui_client_tauri::scheme::{answer, answered, plain, run, SchemeHooks};
use moqui_client_tauri::{ClientState, ShellFiles, APP_ORIGIN, SHELL_ORIGIN};
use serde_json::{json, Value};
use tauri::http::{HeaderMap, Response, StatusCode};
use tauri::utils::assets::AssetKey;
use tauri::{AppHandle, UriSchemeContext, UriSchemeResponder, Webview};

/// The path on the app origin that answers a server page's storage scope and the scopes it is to clear, the host's own,
/// never forwarded.
pub(crate) const PAGE_PATH: &str = "/__moqui-app/page";

/// The longest body a page's `fetch` may send through the host, as the outbox's file limit.
const BODY_MAX: usize = 25 * 1024 * 1024;

/// The shell's files, embedded in the app by Tauri (`frontendDist`, the staged shell: `shell/` and `webroot/`).
pub(crate) struct EmbeddedShell {
    pub(crate) layout: Assets,
    pub(crate) files: Box<dyn tauri::Assets<tauri::DynRuntime>>,
}

impl ShellFiles for EmbeddedShell {
    fn is_static(&self, path: &str) -> bool {
        self.layout.is_static(path)
    }

    fn read(&self, path: &str) -> Option<Asset> {
        let file = self.layout.staged_path(path)?;
        let body = self.files.get(&AssetKey::from(file.as_str()))?;
        Some(asset(path, &file, body.into_owned()))
    }
}

/// The header wry's binding sets on the main frame's own document request, and only there (the patch's
/// `MAIN_FRAME_HEADER`, `patches/wry/src/android/binding.rs`, from the WebView's `isForMainFrame`).
pub(crate) const MAIN_FRAME_HEADER: &str = "x-moqui-main-frame";

/// The header the bridge adds to the page's own GET and HEAD `fetch` and `XMLHttpRequest` on its own origin
/// (`bridge.js`). A frame of another origin cannot send it: a header that is not CORS-safelisted needs a preflight,
/// which the scheme refuses (it takes GET and HEAD only), and a `no-cors` request drops it.
pub(crate) const OWN_HEADER: &str = "x-moqui-own";

/// The hooks of Android's WebView.
pub(crate) struct MobileHooks;

impl SchemeHooks for MobileHooks {
    fn adapt(&self, document: bool, response: Response<Vec<u8>>) -> Response<Vec<u8>> {
        redirect_page(document, response)
    }

    fn fetch_site(&self, headers: &HeaderMap, origin: &str) -> Option<&'static str> {
        Some(page_site(headers, origin))
    }
}

/// The `Sec-Fetch-Site` of a request of the WebView on `origin`, which hands the scheme no fetch metadata
/// (SEC-CLI-102): by its `Origin` where it sends one ([`site_by_origin`]); else `same-origin` for the main frame's own
/// document ([`MAIN_FRAME_HEADER`]) or the page's own fetch ([`OWN_HEADER`]); else `cross-site`, so the server refuses
/// a GET of a writing transition that a frame of another origin in the page started (a nested frame, an image). The
/// page's own requests that carry no mark (its stylesheets, scripts and images, a frame of its own origin) read as
/// `cross-site` too, which the server's rule takes only for a writing transition.
pub(crate) fn page_site(headers: &HeaderMap, origin: &str) -> &'static str {
    if let Some(sent) = headers.get("origin") {
        return site_by_origin(sent.to_str().ok(), origin);
    }
    if headers.contains_key(MAIN_FRAME_HEADER) || headers.contains_key(OWN_HEADER) {
        "same-origin"
    } else {
        "cross-site"
    }
}

/// The hooks of a page's request through the host ([`scheme_fetch`]): `same-origin`, which the host knows here as the
/// WebView's scheme requests do not tell it, since the IPC takes the main frame's messages only (wry's patch,
/// `patches/wry/src/android/kotlin/Ipc.kt`) and the request goes to the page's own origin ([`fetch_request`]); the
/// server's check of a writing GET reads it (SEC-CLI-06, SEC-CLI-102). A redirect is answered as it is, for the bridge.
pub(crate) struct FetchHooks;

impl SchemeHooks for FetchHooks {
    fn fetch_site(&self, _headers: &HeaderMap, _origin: &str) -> Option<&'static str> {
        Some("same-origin")
    }
}

/// The request headers a page's `fetch` through the host does not choose, the host's own to set ([`fetch_request`]):
/// where it comes from (`Origin`, `Referer`, `Sec-Fetch-*`) and whether it is a service worker's (SEC-CLI-06).
fn hosts_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    matches!(name.as_str(), "origin" | "referer" | "service-worker") || name.starts_with("sec-fetch-")
}

/// Whether a page's [`scheme_fetch`] from `origin` is refused before it is made (SEC-CLI-06): on the shell's origin,
/// unless it is the shell's current load (`shell_page`: its page and the load's nonce), so a server page still running
/// as the host shows the shell gets none of the shell's answers (its offline ones, the chat store's passphrase).
pub(crate) fn refuse_fetch(origin: &str, shell_page: bool) -> Result<(), String> {
    if origin == SHELL_ORIGIN && !shell_page {
        return Err("Not the shell's page".to_string());
    }
    Ok(())
}

/// A redirect as Android's WebView takes it: a document's as a page that goes to its `Location` at once, any other
/// as a 502 (a page's own fetch with a body follows it in the bridge, [`scheme_fetch`]).
pub(crate) fn redirect_page(document: bool, response: Response<Vec<u8>>) -> Response<Vec<u8>> {
    if !response.status().is_redirection() {
        return response;
    }
    let location = response
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (true, Some(location)) = (document, location) else {
        return plain(StatusCode::BAD_GATEWAY, "A redirect the app's webview cannot follow");
    };
    let quoted = html_attribute(&location);
    let body = format!(
        "<!DOCTYPE html><html><head><meta http-equiv=\"refresh\" content=\"0;url={quoted}\"></head>\
         <body><a href=\"{quoted}\">Continue</a></body></html>"
    );
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .body(body.into_bytes())
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"))
}

/// `text` for a double-quoted HTML attribute.
fn html_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// What a server page is told at its start: the current account's storage scope (MoquiLib `scopedKey`) and the scopes
/// of accounts deleted since a server page last asked, which it clears once.
pub(crate) fn page_info(state: &ClientState) -> Value {
    let scope = state.account_of(crate::host::MAIN).map(|(account, _)| account.id);
    let cleared = std::mem::take(&mut moqui_lib::sync::lock(&state.cleared).app);
    json!({ "storageScope": scope.unwrap_or_default(), "clearScopes": cleared })
}

/// The answer of the app's schemes to a request of the page `label` on `origin`.
fn respond(app: &AppHandle, label: &str, request: tauri::http::Request<Vec<u8>>, origin: &str) -> Response<Vec<u8>> {
    if origin == APP_ORIGIN && request.uri().path() == PAGE_PATH {
        let info = moqui_client_tauri::state(app).map(|state| page_info(&state));
        return match info {
            Ok(info) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .header("Cache-Control", "no-store")
                .body(info.to_string().into_bytes())
                .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered")),
            Err(err) => plain(StatusCode::NOT_FOUND, &err),
        };
    }
    // the WebView hands the scheme no body, and no Sec-Fetch-* headers (only User-Agent, Accept, Referer, the client
    // hints, and Origin on a CORS-mode request): a request with a body comes as scheme_fetch, and one that comes here
    // (a form posted by navigation) would reach the server empty, with nothing to tell where it came from
    if !matches!(*request.method(), tauri::http::Method::GET | tauri::http::Method::HEAD) {
        return plain(
            StatusCode::METHOD_NOT_ALLOWED,
            "The app's webview sends no request body this way; the page's fetch does",
        );
    }
    answered(app, label, request, origin, &MobileHooks)
}

/// `moqui-shell`: the shell's files, and the rest forwarded to the current account's server.
pub(crate) fn shell(
    context: UriSchemeContext<'_, tauri::DynRuntime>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    let label = context.webview_label().to_string();
    run(Box::new(move || {
        responder.respond(respond(&app, &label, request, SHELL_ORIGIN))
    }));
}

/// `moqui-app`: everything forwarded to the server of the current account.
pub(crate) fn app(
    context: UriSchemeContext<'_, tauri::DynRuntime>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    let label = context.webview_label().to_string();
    run(Box::new(move || {
        responder.respond(respond(&app, &label, request, APP_ORIGIN))
    }));
}

/// A page's request with a body, which Android's WebView does not hand the scheme: `method` to `url` (on the page's own
/// origin) with `headers` and `body` (base64), answered as the scheme would: `{status, headers, body}`, the body in
/// base64. A redirect is answered as it is, for the bridge to follow. The shell's carries the nonce of its load
/// ([`refuse_fetch`]), and where the request comes from is the host's to say ([`fetch_request`], [`FetchHooks`]),
/// never the caller's (SEC-CLI-06). The request is made on Tauri's blocking pool: reqwest's blocking client panics on
/// an async runtime's thread, where an async command runs.
#[tauri::command]
pub(crate) async fn scheme_fetch(
    app: AppHandle,
    webview: Webview,
    nonce: Option<String>,
    method: String,
    url: String,
    headers: HashMap<String, String>,
    body: Option<String>,
) -> Result<Value, String> {
    let page = webview.url().map_err(|err| err.to_string())?;
    let origin = page.origin().ascii_serialization();
    let shell_page = origin == SHELL_ORIGIN && {
        let state = moqui_client_tauri::state(&app)?;
        moqui_client_tauri::ShellHost::is_shell_page(state.shell.as_ref(), &webview, nonce.as_deref())
    };
    refuse_fetch(&origin, shell_page)?;
    let request = fetch_request(&page, &method, &url, &headers, body.as_deref())?;
    let label = webview.label().to_string();
    let answered = tauri::async_runtime::spawn_blocking(move || {
        let state = moqui_client_tauri::state(&app)?;
        let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            answer(&state, &label, request, &origin, &FetchHooks)
        }))
        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
        Ok::<Value, String>(fetch_answer(&response))
    })
    .await
    .map_err(|err| format!("The request did not finish: {err}"))?;
    answered
}

/// The request of a page at `page` for [`scheme_fetch`]: on the page's own origin, which is one of the app's, with the
/// caller's headers less those the host sets: `Origin` the page's own and `Sec-Fetch-Dest` `empty` (a fetch: never a
/// document, a frame or a worker's script), whatever the caller wrote ([`hosts_header`], SEC-CLI-06).
pub(crate) fn fetch_request(
    page: &tauri::Url,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&str>,
) -> Result<tauri::http::Request<Vec<u8>>, String> {
    let origin = page.origin().ascii_serialization();
    if origin != SHELL_ORIGIN && origin != APP_ORIGIN {
        return Err("Not a page of the app".to_string());
    }
    let target = page.join(url).map_err(|err| format!("Not a URL: {err}"))?;
    if target.origin() != page.origin() {
        return Err("A page's request goes to its own origin only".to_string());
    }
    // the base64 of BODY_MAX bytes at most, checked before it is decoded
    if body.is_some_and(|text| text.len() > BODY_MAX.div_ceil(3) * 4) {
        return Err("The body is too large".to_string());
    }
    let body = match body {
        Some(text) => base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(|_| "The body is not base64".to_string())?,
        None => Vec::new(),
    };
    if body.len() > BODY_MAX {
        return Err("The body is too large".to_string());
    }
    let method = tauri::http::Method::from_bytes(method.trim().to_ascii_uppercase().as_bytes())
        .map_err(|_| "Not a method".to_string())?;
    let mut request = tauri::http::Request::builder().method(method).uri(target.as_str());
    for (name, value) in headers {
        if !hosts_header(name) {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    request
        .header("Origin", origin.as_str())
        .header("Sec-Fetch-Dest", "empty")
        .body(body)
        .map_err(|err| format!("Not a request: {err}"))
}

/// A scheme's answer for the bridge's `fetch`.
pub(crate) fn fetch_answer(response: &Response<Vec<u8>>) -> Value {
    let headers: serde_json::Map<String, Value> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            let value: Cow<'_, str> = String::from_utf8_lossy(value.as_bytes());
            (name.to_string(), Value::String(value.into_owned()))
        })
        .collect();
    json!({
        "status": response.status().as_u16(),
        "headers": headers,
        "body": base64::engine::general_purpose::STANDARD.encode(response.body()),
    })
}

#[cfg(test)]
#[path = "scheme_tests.rs"]
mod tests;
