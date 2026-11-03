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

//! The app schemes of the client (plans/Mobile-Desktop.md §4 "One transport seam"). Chromium calls a handler on the main
//! thread; each request goes to a worker thread here, which reads the shell's file or forwards the request to the
//! account's server with its key (`moqui_client::proxy`), and answers from there. The body is whole, as the runtime
//! takes it, so an attachment or a large file a window opens is saved by the host, streamed (`crate::download`); a
//! small download goes through Chromium's (bridge.rs `on_download`), which asks the scheme again.
//!
//! The shell's settings, apps and counts (`moqui_client::cache::PATHS`) are kept per account when the server answers
//! them, and answered from what was kept, marked `X-Moqui-Offline`, when it cannot be reached, so the shell opens with no
//! network. A 401 to a request with the key has the host check the key and, if it no longer signs in, forget it and
//! show the sign-in.

use moqui_client::account::Account;
use moqui_client::proxy::{header, Request};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, Manager, UriSchemeContext, UriSchemeResponder};

use super::{assets, Client, APP_ORIGIN, SHELL_ORIGIN};
use crate::scheme::{document_request, has_script_policy, plain, run};

/// `moqui-shell`: the shell's files, and the rest forwarded to the current account's server.
pub(crate) fn shell(
    context: UriSchemeContext<'_, tauri::DynRuntime>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    let label = context.webview_label().to_string();
    run(Box::new(move || {
        responder.respond(answered(&app, &label, request, SHELL_ORIGIN))
    }));
}

/// `moqui-app`: everything forwarded to the server of the window's account.
pub(crate) fn app(
    context: UriSchemeContext<'_, tauri::DynRuntime>,
    request: tauri::http::Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = context.app_handle().clone();
    let label = context.webview_label().to_string();
    run(Box::new(move || {
        responder.respond(answered(&app, &label, request, APP_ORIGIN))
    }));
}

/// [`answer`], or a 500 when it panics, so the page is not left waiting.
fn answered(app: &AppHandle, label: &str, request: tauri::http::Request<Vec<u8>>, origin: &str) -> Response<Vec<u8>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| answer(app, label, request, origin))).unwrap_or_else(
        |_| {
            tracing::error!("A client scheme request panicked");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered")
        },
    )
}

fn answer(app: &AppHandle, label: &str, request: tauri::http::Request<Vec<u8>>, origin: &str) -> Response<Vec<u8>> {
    let Some(client) = app.try_state::<Client>() else {
        return plain(StatusCode::NOT_FOUND, "Not a client of a server");
    };
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| "/".to_string(), |path| path.as_str().to_string());
    let bare = path.split_once('?').map_or(path.as_str(), |(bare, _)| bare).to_string();
    if origin == SHELL_ORIGIN && request.method() == tauri::http::Method::GET {
        if let Some(asset) = client.assets.read(&bare) {
            let mut response = Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", asset.content_type)
                .header("Cache-Control", "no-cache");
            if asset.page {
                response = response.header("Content-Security-Policy", assets::SHELL_CSP);
            }
            return response
                .body(asset.body)
                .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
        }
        if client.assets.file_of(&bare).is_some() {
            return plain(StatusCode::NOT_FOUND, "Not found");
        }
    }
    // The shell's origin has the shell's commands: the server's documents, frames, scripts and workers are never on it
    // (a link in the shell to the server's page opens in an app window), only its data.
    if origin == SHELL_ORIGIN && !forwarded_on_shell(request.headers()) {
        return plain(
            StatusCode::FORBIDDEN,
            "The server's pages open in an app window, not in the shell",
        );
    }
    let Some((account, key)) = client.account_of(label) else {
        return plain(StatusCode::UNAUTHORIZED, "No account is signed in");
    };
    let get = request.method() == tauri::http::Method::GET;
    let document = document_request(request.headers());
    let cached = get && moqui_client::cache::PATHS.contains(&bare.as_str());
    let forwarded = Request {
        method: request.method().as_str().to_string(),
        path,
        headers: request
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
            .collect(),
        body: request.into_body(),
    };
    let upstream = match client.proxy.send(&account.server, key.as_deref(), origin, forwarded) {
        Ok(upstream) => upstream,
        Err(err) => {
            tracing::info!("{} {bare}: {err}", account.server);
            if cached {
                if let Some(body) = client.cache.get(&account.id, &bare) {
                    return Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "application/json")
                        .header("X-Moqui-Offline", "1")
                        .body(body)
                        .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
                }
            }
            return plain(
                StatusCode::BAD_GATEWAY,
                &format!("The server could not be reached: {err}"),
            );
        }
    };
    // an event stream never ends, and a scheme answers whole bodies: 503, so the Assist drawer polls events.json
    if header(&upstream.headers, "content-type")
        .is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/event-stream"))
    {
        return plain(
            StatusCode::SERVICE_UNAVAILABLE,
            "No event streams in the app; the page catches up instead",
        );
    }
    // an attachment or a large file the window opens: saved by the host, streamed, and the page left as it is
    if let Some(name) = crate::download::saved_by_host(document, upstream.status, &upstream.headers, &bare) {
        crate::download::save(app, name, move |out| {
            upstream.copy_to(out).map_err(|err| err.to_string())
        });
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
    }
    let status = upstream.status;
    let headers = upstream.headers.clone();
    let body = match upstream.body() {
        Ok(body) => body,
        Err(err) => {
            return plain(
                StatusCode::BAD_GATEWAY,
                &format!("The server's answer was cut off: {err}"),
            )
        }
    };
    if status == 401 && key.is_some() {
        check_key(app, &account);
    }
    if cached && status == 200 && header(&headers, "content-type").is_some_and(|kind| kind.contains("json")) {
        client.cache.put(&account.id, &bare, &body);
    }
    let html = header(&headers, "content-type").is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/html"));
    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if html && name.eq_ignore_ascii_case("content-security-policy") && !has_script_policy(value) {
            continue;
        }
        // Chromium puts the app's initialization scripts into every HTML answer of the scheme: a page's fetch of a
        // screen's HTML (a .qvue, a fragment) is handed on as text, so it is not changed
        if html && !document && name.eq_ignore_ascii_case("content-type") {
            response = response.header("Content-Type", "text/plain; charset=utf-8");
            continue;
        }
        response = response.header(name.as_str(), value.as_str());
    }
    if origin == SHELL_ORIGIN {
        // the server's answer is data to the shell, never a script or style its origin would take
        response = response.header("X-Content-Type-Options", "nosniff");
    }
    response
        .body(body)
        .unwrap_or_else(|_| plain(StatusCode::BAD_GATEWAY, "The server's answer could not be passed on"))
}

/// A 401 to a request with the account's key: check that the key still signs in (the server says 401 also when it
/// could not check, and a page may ask for what needs another sign-in), and if it does not, sign the account out here.
pub(crate) fn check_key(app: &AppHandle, account: &Account) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    if client.checking.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let account = account.clone();
    run(Box::new(move || {
        let Some(client) = app.try_state::<Client>() else {
            return;
        };
        let still = client.keys.get(&account.id).map(|key| {
            std::panic::catch_unwind(|| moqui_client::login::client_config(&account.server, &key))
                .unwrap_or_else(|_| Err(moqui_client::Error::new("the check panicked")))
        });
        client.checking.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(Ok(None)) = still {
            tracing::info!(
                "The device key of {} no longer signs in; signing it out here",
                account.id
            );
            super::commands::forget_key(&app, &account.id);
        }
    }));
}

/// Whether the shell may have the server's answer to a request: its fetches and images, not a document, frame, script,
/// style or worker of the server's on the shell's origin.
pub(crate) fn forwarded_on_shell(headers: &tauri::http::HeaderMap) -> bool {
    let dest = headers
        .get("sec-fetch-dest")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    match dest.as_str() {
        "" => !document_request(headers),
        "empty" | "image" | "audio" | "video" | "track" | "font" => true,
        _ => false,
    }
}

#[cfg(test)]
#[path = "scheme_tests.rs"]
mod tests;
