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

//! The app schemes of the client (plans/Mobile-Desktop.md §4 "One transport seam"). The webview calls a handler on the
//! main thread; each request goes to a worker thread ([`run`]), which reads the shell's file or forwards the request
//! to the account's server with its key (`moqui_client::proxy`), and answers from there ([`answer`]). The body is
//! whole, as the runtimes take it; what an app does with an answer before the page has it (the desktop saves an
//! attachment itself, Android's WebView takes no redirect) is its [`SchemeHooks`].
//!
//! The shell's settings, apps and counts, and the comms app's chat and mail transitions, are kept in the account's
//! store when the server answers them (`moqui_client::offline`), and answered from it, marked `X-Moqui-Offline`, when
//! it cannot be reached, so the shell and the comms screens open with no network. The page's chat store passphrase
//! (`/ChatKeys/storeKey`) is kept in the key store, and answered offline to the shell only. A 401 to a request with the
//! key has the host check the key and, if it no longer signs in, sign the account out here.
//!
//! Two things of a scheme's HTML answers need handling on CEF, and do no harm elsewhere: the runtime puts the app's
//! initialization scripts into every `text/html` answer, so HTML a page fetches is handed on as text
//! ([`document_request`]); and it adds those scripts' hashes to a page's CSP, so a server's CSP with no script policy
//! is not passed on ([`has_script_policy`]).

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use moqui_client::account::Account;
use moqui_client::assets;
use moqui_client::offline::{self, Kind};
use moqui_client::proxy::{header, Request, Upstream};
use tauri::http::{HeaderMap, Response, StatusCode};
use tauri::AppHandle;

use crate::{ClientState, SHELL_ORIGIN};

/// The page's chat store passphrase (WebrootCrypto.js `storeKeysOf`, docs/interfaces/Chat-Keys.md "Store key").
const STORE_KEY_PATH: &str = "/ChatKeys/storeKey";

/// The worker threads that answer the schemes' requests: a page loads a few dozen at once.
const WORKERS: usize = 8;

pub type Job = Box<dyn FnOnce() + Send>;

/// What an app does with a server's answer before the page has it.
pub trait SchemeHooks {
    /// Take an answer the host handles itself (the desktop saves an attachment or a large file a window opens, and
    /// answers the page 204); else hand it back.
    ///
    /// # Errors
    ///
    /// Returns the answer, untouched, when the host does not take it.
    fn take(&self, document: bool, upstream: Upstream, path: &str) -> Result<Response<Vec<u8>>, Box<Upstream>> {
        let _ = (document, path);
        Err(Box::new(upstream))
    }

    /// The answer as the app's webview can take it (Android's WebView refuses a redirect status).
    fn adapt(&self, document: bool, response: Response<Vec<u8>>) -> Response<Vec<u8>> {
        let _ = document;
        response
    }
}

/// The hooks of an app that changes nothing.
pub struct NoHooks;

impl SchemeHooks for NoHooks {}

/// Run `job` on a worker; on a thread of its own if the workers cannot be started.
pub fn run(job: Job) {
    static WORK: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    let sender = WORK.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut started = false;
        for index in 0..WORKERS {
            let receiver = Arc::clone(&receiver);
            let spawned = std::thread::Builder::new()
                .name(format!("scheme-{index}"))
                .spawn(move || loop {
                    let next = moqui_lib::sync::lock(&receiver).recv();
                    let Ok(job) = next else { return };
                    // one request that panics does not end the worker (Practices-Rust §8)
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                        tracing::error!("A scheme request panicked");
                    }
                });
            started |= spawned.is_ok();
        }
        started.then_some(sender)
    });
    let job = match sender {
        Some(sender) => match sender.send(job) {
            Ok(()) => return,
            Err(mpsc::SendError(job)) => job,
        },
        None => job,
    };
    if let Err(err) = std::thread::Builder::new().name("scheme".to_string()).spawn(job) {
        tracing::error!("A scheme request was not answered: {err}");
    }
}

/// Whether a request is for a document (a window's or a frame's page), not a page's fetch: `Sec-Fetch-Dest` when the
/// browser sends it, else an `Accept` that names XHTML, as a navigation's does and `fetch` does not.
pub fn document_request(headers: &HeaderMap) -> bool {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    match text("sec-fetch-dest").as_str() {
        "document" | "iframe" | "frame" => true,
        "" => text("accept").contains("application/xhtml+xml"),
        _ => false,
    }
}

/// Whether a server's CSP says what scripts may run (`script-src` or `default-src`). Chromium adds the hashes of the
/// app's initialization scripts to a page's CSP, and to one with no script policy it adds `script-src 'self'` with them,
/// which the server never set: its compiler build's eval and its pages' inline scripts would stop. So the host passes
/// on a server's page's CSP only when it has a script policy; without one the page's scripts run as they do on the web
/// (the rest of that policy, such as `frame-ancestors`, is about a browser's tabs, not an app window).
pub fn has_script_policy(csp: &str) -> bool {
    csp.split(';').any(|directive| {
        let name = directive.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        name == "script-src" || name == "script-src-elem" || name == "default-src"
    })
}

/// A plain text answer.
pub fn plain(status: StatusCode, text: &str) -> Response<Vec<u8>> {
    let mut response = Response::new(text.as_bytes().to_vec());
    *response.status_mut() = status;
    response.headers_mut().insert(
        tauri::http::header::CONTENT_TYPE,
        tauri::http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// [`answer`] for a page of `app`, or a 404 when the app is not a client; a 500 when it panics, so the page is not
/// left waiting.
pub fn answered(
    app: &AppHandle,
    label: &str,
    request: tauri::http::Request<Vec<u8>>,
    origin: &str,
    hooks: &dyn SchemeHooks,
) -> Response<Vec<u8>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match crate::state(app) {
        Ok(state) => answer(&state, label, request, origin, hooks),
        Err(_) => plain(StatusCode::NOT_FOUND, "Not a client of a server"),
    }))
    .unwrap_or_else(|_| {
        tracing::error!("A client scheme request panicked");
        plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered")
    })
}

/// The answer to a request of the page `label` on `origin` (the shell's or the server's pages').
pub fn answer(
    state: &ClientState,
    label: &str,
    request: tauri::http::Request<Vec<u8>>,
    origin: &str,
    hooks: &dyn SchemeHooks,
) -> Response<Vec<u8>> {
    let path = request
        .uri()
        .path_and_query()
        .map_or_else(|| "/".to_string(), |path| path.as_str().to_string());
    let bare = path.split_once('?').map_or(path.as_str(), |(bare, _)| bare).to_string();
    // The shell's own paths (its page, `/shell/`, `/js/`, `/libs/`, `/css/`) are its staged files or nothing: never
    // forwarded, so no script of the server's comes to the shell's origin by a path its CSP allows (a library not staged).
    if origin == SHELL_ORIGIN && state.files.is_static(&bare) {
        if request.method() == tauri::http::Method::GET {
            if let Some(asset) = state.files.read(&bare) {
                let mut response = Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", asset.content_type)
                    .header("Cache-Control", "no-cache");
                if asset.page {
                    response = response.header("Content-Security-Policy", assets::SHELL_CSP);
                } else if asset.worker {
                    response = response.header("Content-Security-Policy", assets::WORKER_CSP);
                }
                return response
                    .body(asset.body)
                    .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
            }
        }
        return plain(StatusCode::NOT_FOUND, "Not found");
    }
    // The shell's origin has the shell's commands: the server's documents, frames, scripts and workers are never on it
    // (a link in the shell to the server's page opens as the server's page), only its data.
    if origin == SHELL_ORIGIN && !forwarded_on_shell(request.headers()) {
        return plain(
            StatusCode::FORBIDDEN,
            "The server's pages open in an app window, not in the shell",
        );
    }
    let Some((account, key)) = state.account_of(label) else {
        return plain(StatusCode::UNAUTHORIZED, "No account is signed in");
    };
    let get = request.method() == tauri::http::Method::GET;
    let document = document_request(request.headers());
    let query = path.split_once('?').map_or("", |(_, query)| query).to_string();
    let kept = if get && !document {
        offline::classify(&bare, &state.core.comms_root_of(&account.id))
    } else {
        None
    };
    // the crypto device whose store passphrase is asked for
    let store_key_device = (request.method() == tauri::http::Method::POST && bare == STORE_KEY_PATH)
        .then(|| store_key_device(request.body()))
        .flatten();
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
    let sent = if state.shell.network_ready() {
        state
            .core
            .proxy()
            .send(&account.server, key.as_deref(), origin, forwarded)
            .map_err(|err| err.to_string())
    } else {
        Err("this app cannot make secure connections".to_string())
    };
    let upstream = match sent {
        Ok(upstream) => upstream,
        Err(err) => {
            tracing::info!("{} {bare}: {err}", account.server);
            let offline_body = match (kept, &store_key_device) {
                (Some(kind), _) => state
                    .core
                    .with_store(&account.id, |store| offline::answer(store, kind, &bare, &query))
                    .flatten(),
                // the passphrase only to the shell, which runs no server code
                (None, Some(device)) if origin == SHELL_ORIGIN => {
                    state.core.keys().chat_store_key(&account.id, device).map(|key| {
                        serde_json::json!({ "storeKey": key, "legacyStoreKey": null })
                            .to_string()
                            .into_bytes()
                    })
                }
                _ => None,
            };
            if let Some(body) = offline_body {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .header("Cache-Control", "no-store")
                    .header("X-Moqui-Offline", "1")
                    .body(body)
                    .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered"));
            }
            return unreachable(&err);
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
    let upstream = match hooks.take(document, upstream, &bare) {
        Ok(taken) => return taken,
        Err(upstream) => *upstream,
    };
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
        state.host.check_key(&account);
    }
    let json = header(&headers, "content-type").is_some_and(|kind| kind.contains("json"));
    if let (Some(kind), true, true) = (kept, status == 200, json) {
        if kind == Kind::Shell && bare == "/qapps/clientConfig" {
            state.core.config_seen(&account.id, &body);
        }
        state
            .core
            .with_store(&account.id, |store| offline::keep(store, kind, &bare, &query, &body));
    }
    if let (Some(device), true) = (&store_key_device, json) {
        store_key_answered(state, &account.id, device, &body);
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
    let response = response
        .body(body)
        .unwrap_or_else(|_| plain(StatusCode::BAD_GATEWAY, "The server's answer could not be passed on"));
    hooks.adapt(document, response)
}

/// A 401 to a request with the account's key: check that the key still signs in (the server says 401 also when it
/// could not check, and a page may ask for what needs another sign-in), and if it does not, sign the account out here.
pub fn check_key(app: &AppHandle, account: &Account) {
    let Ok(state) = crate::state(app) else {
        return;
    };
    if !state.shell.network_ready() || state.checking.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    let account = account.clone();
    run(Box::new(move || {
        let Ok(state) = crate::state(&app) else {
            return;
        };
        let still = state.core.keys().get(&account.id).map(|key| {
            std::panic::catch_unwind(|| moqui_client::login::client_config(&account.server, &key))
                .unwrap_or_else(|_| Err(moqui_client::Error::new("the check panicked")))
        });
        state.checking.store(false, Ordering::SeqCst);
        if let Some(Ok(None)) = still {
            tracing::info!(
                "The device key of {} no longer signs in; signing it out here",
                account.id
            );
            crate::commands::signed_out(&app, &account.id);
        }
    }));
}

/// The `deviceId` of a `/ChatKeys/storeKey` request's JSON body.
pub fn store_key_device(body: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    json["deviceId"]
        .as_str()
        .filter(|device| !device.is_empty())
        .map(str::to_string)
}

/// What the server answered to `/ChatKeys/storeKey` for `device`, for the key store: the passphrase to keep (a new
/// revision replaces the old), none to drop it (the device is revoked or unknown), or no change (another refusal).
pub fn store_key_change(body: &[u8]) -> Option<Option<String>> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    if let Some(key) = json["storeKey"].as_str().filter(|key| !key.is_empty()) {
        return Some(Some(key.to_string()));
    }
    matches!(json["reasonCode"].as_str(), Some("revoked" | "notFound")).then_some(None)
}

fn store_key_answered(state: &ClientState, account: &str, device: &str, body: &[u8]) {
    let changed = match store_key_change(body) {
        Some(Some(key)) => state.core.keys().set_chat_store_key(account, device, &key),
        Some(None) => state.core.keys().remove_chat_store_key(account, device),
        None => Ok(()),
    };
    if let Err(err) = changed {
        tracing::warn!("The chat store key of {account} was not kept: {err}");
    }
}

/// The host's answer when the server cannot be reached and nothing is kept for the request: 502, marked
/// `X-Moqui-Offline: unreachable`, which the pages take as offline (MoquiLib `noteAnswer`), unlike a 502 of the server's.
pub fn unreachable(err: &str) -> Response<Vec<u8>> {
    let mut response = plain(
        StatusCode::BAD_GATEWAY,
        &format!("The server could not be reached: {err}"),
    );
    response
        .headers_mut()
        .insert("X-Moqui-Offline", tauri::http::HeaderValue::from_static("unreachable"));
    response
}

/// Whether the shell may have the server's answer to a request: its fetches and images, not a document, frame, script,
/// style or worker of the server's on the shell's origin.
pub fn forwarded_on_shell(headers: &HeaderMap) -> bool {
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
