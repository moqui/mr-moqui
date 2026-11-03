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

//! The PDF engine of the node in process, on the app's own Chromium (plans/Mobile-Desktop.md §4 configuration 1,
//! "PDF"): a hidden window that is never shown, driven over the DevTools Protocol through CEF
//! (`tauri_runtime_cef::WebviewCefExt`), with the server's own page sequence (`moqui_host::pdf::print_page` and
//! `preview_page`), so a PDF, a PNG and a preview come out as the headless Chromium makes them. Installed only where
//! the node found no Chromium of its own (`pdf_chromium_path`, `runtime/chromium` or one on the `PATH`), in process
//! only: a sidecar node is another process.
//!
//! As there, the document reaches no network: `Fetch` pauses every request, and the page fails each one. The window has
//! no capability (no command reaches the host from it), its navigation stays on `about:blank`, and it downloads
//! nothing. One document at a time, each within the server's `pdf` `timeout`. A call blocks its worker thread, never
//! the main or the CEF UI thread.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_host::pdf::DevtoolsPage;
use moqui_lib::error::Error;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_runtime_cef::{allocate_devtools_message_id, DevToolsProtocol, WebviewCefExt};

/// The hidden window's label.
const LABEL: &str = "pdf";
/// How long one document may take, as the server's `pdf` `timeout` default.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The largest PDF previewed, as the server's `pdf` `preview-max-size` default.
const PREVIEW_MAX: usize = 25 * 1024 * 1024;

/// The answers the hidden page's requests wait for, by message id.
type Waiting = Arc<Mutex<HashMap<i32, std::sync::mpsc::Sender<Result<Value, String>>>>>;

/// The engine: the app, the hidden page once it is made, and one document at a time.
pub(crate) struct CefPdf {
    app: AppHandle,
    page: Mutex<Option<Arc<CefPage>>>,
    one: Mutex<()>,
}

/// The hidden window and the answers its page's requests wait for.
struct CefPage {
    window: WebviewWindow,
    waiting: Waiting,
}

/// Install the engine for the node in process when it found no Chromium of its own.
pub(crate) fn install_if_none(app: &AppHandle) {
    if moqui_core::screen::pdf_engine_installed() {
        return;
    }
    tracing::info!("PDF on the app's own Chromium (CEF): the node found none of its own");
    moqui_core::screen::install_pdf_engine(Arc::new(CefPdf {
        app: app.clone(),
        page: Mutex::new(None),
        one: Mutex::new(()),
    }));
}

impl CefPdf {
    /// The hidden page, made on first use and again when its window is gone.
    fn page(&self) -> Result<Arc<CefPage>, Error> {
        let mut page = moqui_lib::sync::lock(&self.page);
        if let Some(current) = page.as_ref().filter(|_| self.app.get_webview_window(LABEL).is_some()) {
            return Ok(Arc::clone(current));
        }
        let window = WebviewWindowBuilder::new(
            &self.app,
            LABEL,
            WebviewUrl::External("about:blank".parse().map_err(|err| Error::new(format!("{err}")))?),
        )
        .title("MR Moqui PDF")
        .visible(false)
        .focused(false)
        .skip_taskbar(true)
        .on_navigation(|url| url.scheme() == "about")
        .on_download(|_, _| false)
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .build()
        .map_err(|err| Error::new(format!("the PDF window did not open: {err}")))?;
        let waiting: Waiting = Arc::default();
        let observer = observe(&window, Arc::clone(&waiting));
        window
            .on_dev_tools_protocol(observer)
            .map_err(|err| Error::new(format!("the PDF window has no DevTools protocol: {err}")))?;
        let made = Arc::new(CefPage { window, waiting });
        *page = Some(Arc::clone(&made));
        Ok(made)
    }

    /// Run `work` on the hidden page, one document at a time, within [`TIMEOUT`].
    fn with_page<T: Send>(
        &self,
        work: impl for<'a> FnOnce(
            &'a CefPage,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, Error>> + Send + 'a>>,
    ) -> Result<T, moqui_core::context::Error> {
        let _one = moqui_lib::sync::lock(&self.one);
        let page = self
            .page()
            .map_err(|err| moqui_core::context::Error::new(err.to_string()))?;
        let done = tauri::async_runtime::block_on(async {
            match tokio::time::timeout(TIMEOUT, work(&page)).await {
                Ok(done) => done,
                Err(_) => Err(Error::new(format!(
                    "PDF render did not finish in {} seconds",
                    TIMEOUT.as_secs()
                ))),
            }
        });
        done.map_err(|err| moqui_core::context::Error::new(err.to_string()))
    }
}

/// The hidden page's DevTools messages: an answer to one of its requests goes to the request waiting for it; a request
/// the page makes (`Fetch.requestPaused`) is failed, from a thread of its own (this runs on CEF's UI thread, which
/// must not wait).
fn observe(window: &WebviewWindow, waiting: Waiting) -> impl Fn(DevToolsProtocol) + Send + Sync + 'static {
    let window = window.clone();
    move |message| match message {
        DevToolsProtocol::MethodResult {
            message_id,
            success,
            result,
        } => {
            let Some(sender) = moqui_lib::sync::lock(&waiting).remove(&message_id) else {
                return;
            };
            let answer = if success {
                serde_json::from_slice(&result).map_err(|err| format!("the browser's answer is not JSON: {err}"))
            } else {
                Err(String::from_utf8_lossy(&result).into_owned())
            };
            let _ = sender.send(answer);
        }
        DevToolsProtocol::Event { method, params } if method == "Fetch.requestPaused" => {
            let params: Value = serde_json::from_slice(&params).unwrap_or(Value::Null);
            let url = params
                .pointer("/request/url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            tracing::debug!(%url, "PDF page request blocked");
            let fail = json!({ "requestId": params.get("requestId"), "errorReason": "BlockedByClient" });
            let window = window.clone();
            let _ = std::thread::Builder::new()
                .name("pdf-block".to_string())
                .spawn(move || {
                    if let Err(err) = send(&window, "Fetch.failRequest", fail).map(|_| ()) {
                        tracing::warn!("A PDF page request was not failed: {err}");
                    }
                });
        }
        _ => {}
    }
}

/// Send `method` to the page and answer its id; the answer is not waited for.
fn send(window: &WebviewWindow, method: &str, params: Value) -> Result<i32, String> {
    let id = allocate_devtools_message_id().map_err(|err| err.to_string())?;
    let message = json!({ "id": id, "method": method, "params": params }).to_string();
    window
        .send_dev_tools_message(message.as_bytes())
        .map_err(|err| err.to_string())?;
    Ok(id)
}

impl DevtoolsPage for CefPage {
    fn call<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, Error>> + Send + 'a>> {
        Box::pin(async move {
            let id = allocate_devtools_message_id().map_err(|err| Error::new(err.to_string()))?;
            let (sender, answer) = std::sync::mpsc::channel();
            moqui_lib::sync::lock(&self.waiting).insert(id, sender);
            let message = json!({ "id": id, "method": method, "params": params }).to_string();
            let window = self.window.clone();
            // the send waits for the event loop, and the answer for CEF: both off the async thread
            let answered = tauri::async_runtime::spawn_blocking(move || {
                window
                    .send_dev_tools_message(message.as_bytes())
                    .map_err(|err| err.to_string())?;
                answer
                    .recv_timeout(TIMEOUT)
                    .map_err(|_| "the browser did not answer".to_string())?
            })
            .await
            .map_err(|err| Error::new(err.to_string()))?;
            moqui_lib::sync::lock(&self.waiting).remove(&id);
            answered.map_err(|err| Error::new(format!("{method}: {err}")))
        })
    }
}

impl moqui_core::screen::PdfEngine for CefPdf {
    fn render(&self, html_print_document: &str) -> Result<Vec<u8>, moqui_core::context::Error> {
        let html = html_print_document.to_string();
        self.with_page(move |page| Box::pin(async move { moqui_host::pdf::print_page(page, &html, false).await }))
    }

    fn render_png(&self, html_document: &str) -> Result<Vec<u8>, moqui_core::context::Error> {
        let html = html_document.to_string();
        self.with_page(move |page| Box::pin(async move { moqui_host::pdf::print_page(page, &html, true).await }))
    }

    fn preview(
        &self,
        request: &moqui_core::screen::PdfPreviewRequest<'_>,
    ) -> Result<moqui_core::screen::PdfPreview, moqui_core::context::Error> {
        if request.pdf.len() > PREVIEW_MAX {
            return Err(moqui_core::context::Error::new(format!(
                "the PDF is {} bytes, larger than the {PREVIEW_MAX} bytes previewed",
                request.pdf.len()
            )));
        }
        let expression = moqui_host::pdf::preview_expression(request);
        self.with_page(move |page| Box::pin(async move { moqui_host::pdf::preview_page(page, &expression).await }))
    }
}
