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

//! The nonce of the shell's current load (plans/Mobile-Desktop.md "Remote applications and authentication"): the host
//! makes one as the shell's own page finishes loading in its window and hands it to that document
//! ([`nonce_script`]), and a shell command is taken only with it ([`crate::ShellHost::is_shell_page`]). Any other
//! document the window comes to show, a `data:` or `blob:` one or a page that took the shell's place, never has it, so
//! the window's capability is not the only line between a page and the shell's commands (plans/archive/Security-Review.md
//! SEC-CLI-07, SEC-CLI-97). The desktop's main window uses [`ShellLoad`]; the mobile app's one webview, which shows
//! the server's pages too, keeps the same nonce in its own page state.

use std::sync::Mutex;

use tauri::Url;

use crate::SHELL_ORIGIN;

/// What a window that shows the shell has loaded: whether its current document is the shell's page, and that load's
/// nonce once it finished.
#[derive(Debug, Default)]
pub struct ShellLoad {
    load: Mutex<Load>,
}

#[derive(Debug, Default)]
struct Load {
    /// The document whose load began last is the shell's page ([`is_shell_document`]).
    shell: bool,
    /// The nonce of its load, once it finished; none while it loads, after a navigation, and for any other document.
    nonce: Option<String>,
}

impl ShellLoad {
    /// A load of `url` began: its document is the page from now on, with no nonce yet.
    pub fn started(&self, url: &Url) {
        let mut load = moqui_lib::sync::lock(&self.load);
        load.shell = is_shell_document(url);
        load.nonce = None;
    }

    /// The load of `url` finished: a new nonce for the shell's page, to hand its document; none for any other.
    pub fn finished(&self, url: &Url) -> Option<String> {
        let mut load = moqui_lib::sync::lock(&self.load);
        if !load.shell || !is_shell_document(url) {
            load.nonce = None;
            return None;
        }
        let nonce = moqui_client::store::new_client_ref().ok()?;
        load.nonce = Some(nonce.clone());
        Some(nonce)
    }

    /// Whether a command from a window at `url` (its URL as the command came) with `nonce` is the shell's: the shell's
    /// page, whose current load's nonce it carries.
    pub fn is_shell(&self, url: Option<&Url>, nonce: Option<&str>) -> bool {
        let load = moqui_lib::sync::lock(&self.load);
        url.is_some_and(is_shell_document) && load.shell && load.nonce.is_some() && load.nonce.as_deref() == nonce
    }
}

/// Whether `url` is the shell's own page: `/` or `/index.html` on the shell's origin (any query); another path there is
/// a server's, forwarded, and never the shell.
pub fn is_shell_document(url: &Url) -> bool {
    url.origin().ascii_serialization() == SHELL_ORIGIN && matches!(url.path(), "/" | "/index.html")
}

/// What the host evaluates in the shell's document to hand it its nonce: the document takes it only on the shell's
/// origin, once (the bridge's `__moquiShellNonce`), so a document that came in its place gets nothing.
pub fn nonce_script(nonce: &str) -> String {
    let quoted = serde_json::to_string(nonce).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "if (location.origin === {origin:?} && window.__moquiShellNonce) window.__moquiShellNonce({quoted});",
        origin = SHELL_ORIGIN
    )
}

#[cfg(test)]
#[path = "shell_load_tests.rs"]
mod tests;
