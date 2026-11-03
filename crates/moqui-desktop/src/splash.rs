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

//! The splash page (`splash/index.html`) as a URL the window goes to: while the node starts, and with the error when
//! it fails, before or after it started. The message is in the URL, so the page has it when it loads rather than
//! from a script run on a page that may not have loaded yet.

use std::sync::OnceLock;

use tauri::{AppHandle, Url};

/// The app's own pages in a bundle: under CEF they are on `http://tauri.localhost` (`tauri_runtime_cef`'s
/// `custom_scheme_url`).
const BUNDLED: &str = "http://tauri.localhost/";
/// The page of the splash, under [`base`].
const PAGE: &str = "index.html";

/// Where the app's own pages are in this run, set once by [`init`].
static BASE: OnceLock<Url> = OnceLock::new();

/// Keep where the app's own pages are, before the window opens on them: `dev_url` in a development run, as Tauri
/// resolves `WebviewUrl::App` (`cargo tauri dev` serves `frontendDist` on a loopback port of its own when there is no
/// `devUrl`, and sets it), else the bundle's.
pub(crate) fn init(dev_url: Option<&Url>) {
    let _ = BASE.set(base_for(dev_url));
}

/// The app's pages' base for `dev_url`, as [`init`] keeps it.
pub(crate) fn base_for(dev_url: Option<&Url>) -> Url {
    dev_url
        .cloned()
        .unwrap_or_else(|| Url::parse(BUNDLED).expect("the bundle's URL is a URL"))
}

/// Where the app's own pages are in this run.
pub(crate) fn base() -> Url {
    BASE.get().cloned().unwrap_or_else(|| base_for(None))
}

/// The splash page, with `failed` as the error it shows; without, it says the node is starting.
pub(crate) fn url(failed: Option<&str>) -> Url {
    url_on(&base(), failed)
}

/// [`url`] with the app's pages at `base`.
pub(crate) fn url_on(base: &Url, failed: Option<&str>) -> Url {
    let mut url = base.join(PAGE).unwrap_or_else(|_| base.clone());
    if let Some(message) = failed {
        url.query_pairs_mut().append_pair("failed", message);
    }
    url
}

/// The splash page's Restart button: the tray's Restart node. Only the app's own pages may call it
/// (`capabilities/splash.json`); the node's origin has the bridge's commands and not this one.
#[tauri::command]
pub(crate) fn restart_node(app: AppHandle) {
    crate::restart(&app);
}

#[cfg(test)]
#[path = "splash_tests.rs"]
mod tests;
