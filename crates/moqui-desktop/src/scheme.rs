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

//! What the app's schemes share (plans/Mobile-Desktop.md §4 "One transport seam"): the client's two (`client::scheme`)
//! and the node's in process (`node::scheme`). Chromium calls a scheme's handler on the main thread; each request goes
//! to a worker thread here. On CEF two things of a scheme's HTML answers need handling: the runtime puts the app's
//! initialization scripts into every `text/html` answer, so HTML a page fetches is handed on as text
//! ([`document_request`]); and it adds those scripts' hashes to a page's CSP, so a server's CSP with no script policy
//! is not passed on ([`has_script_policy`]).

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::http::{Response, StatusCode};

/// The worker threads that answer the schemes' requests: a page loads a few dozen at once.
const WORKERS: usize = 8;

pub(crate) type Job = Box<dyn FnOnce() + Send>;

/// Run `job` on a worker; on a thread of its own if the workers cannot be started.
pub(crate) fn run(job: Job) {
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
pub(crate) fn document_request(headers: &tauri::http::HeaderMap) -> bool {
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
pub(crate) fn has_script_policy(csp: &str) -> bool {
    csp.split(';').any(|directive| {
        let name = directive.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        name == "script-src" || name == "script-src-elem" || name == "default-src"
    })
}

pub(crate) fn plain(status: StatusCode, text: &str) -> Response<Vec<u8>> {
    let mut response = Response::new(text.as_bytes().to_vec());
    *response.status_mut() = status;
    response.headers_mut().insert(
        tauri::http::header::CONTENT_TYPE,
        tauri::http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

#[cfg(test)]
#[path = "scheme_tests.rs"]
mod tests;
