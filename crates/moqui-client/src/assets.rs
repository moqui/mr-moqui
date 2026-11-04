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

//! The shell's own files, which the `moqui-shell` scheme serves with no network: the page and its scripts
//! (`crates/moqui-desktop/shell`) and the webroot's client and libraries it loads (`js/`, `libs/`, `css/`). A development
//! run reads them from the repository (the host names the directories, [`Assets::new`]); a bundle from `shell-assets/`
//! in its resources ([`Assets::staged`]), which `./moq desktop build` stages in the same layout (`shell/`, and
//! `webroot/js`, `webroot/libs`, `webroot/css`).

use std::path::{Path, PathBuf};

/// The content security policy of the shell's page: its own scripts only (from its own files' paths, not the server's
/// forwarded ones), no eval (only WebAssembly's compile, for the crypto device's module, `libs/matrix-sdk-crypto`), no
/// frames (a mail body's `srcdoc` frame, `m-html-view`, is not one, and takes this policy), one worker, its requests to
/// its own origin (which the host forwards) and to Tauri's IPC. Chromium adds the hashes of the app's initialization
/// scripts.
///
/// The one worker is an encrypted call's SFrame transform ([`SFRAME_WORKER`], `RTCRtpScriptTransform`), by its exact
/// path, a staged file (never a `blob:` or `data:` one, nor a path the host forwards). A call's media needs nothing more:
/// a `<video>` or `<audio>` of a `MediaStream` (`srcObject`) is not a fetch `media-src` governs, `RTCPeerConnection`
/// is not a fetch either, and its signaling goes over the account's `/notws`, which the host holds
/// (`moquiDesktop.socket`), so `connect-src` has no WebSocket.
pub const SHELL_CSP: &str = "default-src 'self'; \
     script-src http://moqui-shell.localhost/shell/ http://moqui-shell.localhost/js/ http://moqui-shell.localhost/libs/ 'wasm-unsafe-eval'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; \
     connect-src 'self' http://ipc.localhost; media-src 'self' blob:; frame-src 'none'; \
     worker-src http://moqui-shell.localhost/js/WebrootSFrameWorker.js; \
     object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

/// The script of an encrypted call's SFrame worker (`WebrootCallKeys.js` starts it for `RTCRtpScriptTransform`), the
/// one worker the shell's CSP allows.
pub const SFRAME_WORKER: &str = "/js/WebrootSFrameWorker.js";

/// The policy the SFrame worker's script is answered with, which a worker runs under: the SFrame code it imports
/// (`WebrootSFrame.js`) and nothing else, no requests (its keys come from the page by message), no workers of its own.
pub const WORKER_CSP: &str =
    "default-src 'none'; script-src http://moqui-shell.localhost/js/WebrootSFrame.js; worker-src 'none'; base-uri 'none'";

/// Where the shell's files are, and the libraries it may load (`shell/libs.txt`, which `./moq desktop build` stages).
pub struct Assets {
    shell: PathBuf,
    webroot: PathBuf,
    libs: Vec<String>,
}

/// A file of the shell, as the scheme answers it.
pub struct Asset {
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// The page: it gets the CSP.
    pub page: bool,
    /// The SFrame worker's script: it gets the worker's CSP.
    pub worker: bool,
}

impl Assets {
    /// The bundle's files, staged in `shell-assets/` of its resources.
    pub fn staged(resources: &Path) -> Self {
        let staged = resources.join("shell-assets");
        Self::new(staged.join("shell"), staged.join("webroot"))
    }

    /// The shell's files in `shell` (with its `libs.txt`) and the webroot's in `webroot`.
    pub fn new(shell: PathBuf, webroot: PathBuf) -> Self {
        let libs = std::fs::read_to_string(shell.join("libs.txt"))
            .unwrap_or_else(|err| {
                tracing::error!("The shell's libs.txt was not read: {err}");
                String::new()
            })
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();
        Self { shell, webroot, libs }
    }

    /// The shell's files as a host keeps them itself in the staged layout (the mobile app's, embedded in the app), with
    /// the libraries of `libs_txt` (the text of `shell/libs.txt`): [`Self::staged_path`] says where a path's file is,
    /// and the host reads it and makes its answer with [`asset`].
    pub fn embedded(libs_txt: &str) -> Self {
        let libs = libs_txt
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();
        Self {
            shell: PathBuf::from("shell"),
            webroot: PathBuf::from("webroot"),
            libs,
        }
    }

    /// Where the file of a request's path is in the staged layout of [`Self::embedded`] (`shell/<file>`,
    /// `webroot/js/...`, `webroot/libs/<lib>/...`, `webroot/css/...`), or none when it is not one of the shell's files.
    pub fn staged_path(&self, path: &str) -> Option<String> {
        let file = self.file_of(path)?;
        let parts: Vec<&str> = file.iter().filter_map(|part| part.to_str()).collect();
        Some(parts.join("/"))
    }

    /// Whether a path is one of the shell's own (its page, or under `/shell/`, `/js/`, `/libs/` or `/css/`): answered from
    /// its staged files or not found, never forwarded to the server.
    pub fn is_static(&self, path: &str) -> bool {
        if matches!(path, "/" | "/index.html") {
            return true;
        }
        let first = path
            .strip_prefix('/')
            .map_or("", |rest| rest.split('/').next().unwrap_or(""));
        matches!(first, "shell" | "js" | "libs" | "css")
    }

    /// The file of a request's path, or none when the path is not one of the shell's (the scheme then forwards it).
    pub(crate) fn file_of(&self, path: &str) -> Option<PathBuf> {
        let (root, rest) = match path {
            "/" | "/index.html" => (&self.shell, "index.html"),
            _ => {
                let (first, rest) = path.strip_prefix('/')?.split_once('/')?;
                match first {
                    "shell" => (&self.shell, rest),
                    // a library of libs.txt only, as a bundle has
                    "libs" if !self.libs.iter().any(|lib| rest.split('/').next() == Some(lib.as_str())) => return None,
                    "js" | "libs" | "css" => (&self.webroot, path.strip_prefix('/')?),
                    _ => return None,
                }
            }
        };
        let plain = !rest.is_empty()
            && rest
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', '\0', ':']));
        plain.then(|| root.join(rest))
    }

    /// The file of `path`, read; none when it is not a shell path or there is no such file.
    pub fn read(&self, path: &str) -> Option<Asset> {
        let file = self.file_of(path)?;
        let body = std::fs::read(&file).ok()?;
        let content_type = content_type(&file);
        Some(Asset {
            body,
            content_type,
            page: content_type.starts_with("text/html"),
            worker: path == SFRAME_WORKER,
        })
    }
}

/// The shell's file `body` of the request's `path`, kept as `file` (its extension gives the type), as the scheme
/// answers it: for a host that reads its files itself ([`Assets::embedded`]).
pub fn asset(path: &str, file: &str, body: Vec<u8>) -> Asset {
    let content_type = content_type(Path::new(file));
    Asset {
        body,
        content_type,
        page: content_type.starts_with("text/html"),
        worker: path == SFRAME_WORKER,
    }
}

/// The type of a file by its extension.
fn content_type(file: &Path) -> &'static str {
    match file.extension().and_then(|ext| ext.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;
