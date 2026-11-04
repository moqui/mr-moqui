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

//! The app schemes of the client (plans/Mobile-Desktop.md §4 "One transport seam"), answered by
//! `moqui_client_tauri::scheme` on a worker thread: the shell's files, and the rest forwarded to the account's server
//! with its key, kept in the store and answered from it offline. What only the desktop does is its hook: an attachment
//! or a large file a window opens is saved by the host, streamed (`crate::download`), as the body is whole as CEF
//! takes it; a small download goes through Chromium's (bridge.rs `on_download`), which asks the scheme again.

use moqui_client::proxy::Upstream;
use moqui_client_tauri::scheme::{answered, run, SchemeHooks};
use tauri::http::{Response, StatusCode};
use tauri::{AppHandle, UriSchemeContext, UriSchemeResponder};

use super::{APP_ORIGIN, SHELL_ORIGIN};
use crate::scheme::plain;

/// The desktop's hook: the downloads the host saves.
struct Downloads<'a>(&'a AppHandle);

impl SchemeHooks for Downloads<'_> {
    fn take(&self, document: bool, upstream: Upstream, path: &str) -> Result<Response<Vec<u8>>, Box<Upstream>> {
        let Some(name) = crate::download::saved_by_host(document, upstream.status, &upstream.headers, path) else {
            return Err(Box::new(upstream));
        };
        crate::download::save(self.0, name, move |out| {
            upstream.copy_to(out).map_err(|err| err.to_string())
        });
        Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Vec::new())
            .unwrap_or_else(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Not answered")))
    }
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
        responder.respond(answered(&app, &label, request, SHELL_ORIGIN, &Downloads(&app)))
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
        responder.respond(answered(&app, &label, request, APP_ORIGIN, &Downloads(&app)))
    }));
}
