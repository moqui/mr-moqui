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
//!   way.
//! - **No redirect status.** A `WebResourceResponse` refuses 3xx, so a document's redirect is answered as a page that
//!   goes there ([`MobileHooks`]); the bridge's `fetch` follows a redirect itself.
//! - **One init script for every page.** A server page's storage scope and the scopes to clear are asked of the host
//!   at its start ([`PAGE_PATH`]), as an app window's init script has them on the desktop.

use std::borrow::Cow;
use std::collections::HashMap;

use base64::Engine;
use moqui_client::assets::{asset, Asset, Assets};
use moqui_client_tauri::scheme::{answer, answered, plain, run, SchemeHooks};
use moqui_client_tauri::{ClientState, ShellFiles, APP_ORIGIN, SHELL_ORIGIN};
use serde_json::{json, Value};
use tauri::http::{Response, StatusCode};
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

/// The hooks of Android's WebView.
pub(crate) struct MobileHooks;

impl SchemeHooks for MobileHooks {
    fn adapt(&self, document: bool, response: Response<Vec<u8>>) -> Response<Vec<u8>> {
        redirect_page(document, response)
    }
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
/// base64. A redirect is answered as it is, for the bridge to follow.
#[tauri::command(async)]
pub(crate) fn scheme_fetch(
    app: AppHandle,
    webview: Webview,
    method: String,
    url: String,
    headers: HashMap<String, String>,
    body: Option<String>,
) -> Result<Value, String> {
    let page = webview.url().map_err(|err| err.to_string())?;
    let request = fetch_request(&page, &method, &url, &headers, body.as_deref())?;
    let origin = page.origin().ascii_serialization();
    let state = moqui_client_tauri::state(&app)?;
    let label = webview.label().to_string();
    let response = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        answer(&state, &label, request, &origin, &moqui_client_tauri::scheme::NoHooks)
    }))
    .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
    Ok(fetch_answer(&response))
}

/// The request of a page at `page` for [`scheme_fetch`]: on the page's own origin, which is one of the app's.
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
        request = request.header(name.as_str(), value.as_str());
    }
    request.body(body).map_err(|err| format!("Not a request: {err}"))
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
