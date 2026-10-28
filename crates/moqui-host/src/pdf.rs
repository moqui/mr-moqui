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

//! `PdfRenderer`'s engine: one long-lived headless Chromium, driven over the DevTools Protocol
//! (`plans/archive/Architecture-Proposal.md`, Tool components and print).
//!
//! The binary is the conf's `screen-facade` `pdf` `chromium-path` (`pdf_chromium_path`), else the one `./moq chromium-download` put in `runtime/chromium`, else `chromium`
//! then `chromium-browser` on the `PATH`; early in startup [`find_chromium`] (in [`crate::chromium`], which the browser
//! tests share) logs every one it finds and the one it uses. It is not vendored. When none is found the server still starts and PDF reports that it is not available. The browser starts on the first render and is kept; when it exits, the next
//! render starts it again.
//!
//! Each document gets its own page: the `html-print` text becomes the content of `about:blank`, every request the page
//! makes is failed (the document reaches no network, no server, and no session), `Page.printToPDF` runs with the
//! document's `@page` size, and the page closes. `pdf` `page-limit` pages print at once and `timeout` seconds bound
//! each one. The caller's thread blocks on the edge runtime until the bytes come back.
//!
//! A preview ([`moqui_core::screen::PdfEngine::preview`]) uses a page the same way, under the same limits: the page's
//! content is a `<canvas>`, and one `Runtime.evaluate` imports the vendored pdf.js modules from `data:` URLs (which the
//! request blocker never sees), runs its worker on the page's main thread, opens the PDF from bytes, draws page 1 to
//! the canvas, and reads the text of the first pages. No request leaves the page: there are no font, cmap or wasm
//! URLs, so the standard fonts come from the system and a JPEG 2000 image is left out. `pdf` `preview-max-size` bytes
//! is the largest PDF previewed.
//!
//! The protocol client is [`cdp`], on `tokio-tungstenite`. `chromiumoxide` was the planned crate, but it parses
//! messages through an untagged enum, which fails on every event with a float once the workspace's `serde_json` has
//! `arbitrary_precision`, so its pages never finish loading.

mod cdp;

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use moqui_lib::error::Error;
use serde_json::{json, Value};
use tokio::sync::{Mutex, Semaphore};

use self::cdp::Connection;
use crate::chromium::{devtools_url, find_chromium, launch_args, userns_restricted};

/// Default concurrent pages.
const PAGE_LIMIT: u64 = 4;
/// Default seconds a render may take.
const TIMEOUT_SECONDS: u64 = 60;
/// Default largest PDF previewed, in bytes.
const PREVIEW_MAX_SIZE: u64 = 25 * 1024 * 1024;
/// How long the browser may take to start listening.
const START_WAIT: Duration = Duration::from_secs(30);
/// Before [`launch_args`]: a proxy that is not there for every request but loopback, and WebRTC kept to it.
///
/// `Fetch` interception fails every HTTP request a page makes, but not a WebSocket or WebRTC connection. Those go to a
/// proxy that is not there, loopback included, and WebRTC may not use UDP outside the proxy, so they fail too.
const ISOLATION_ARGS: [&str; 3] = [
    "--proxy-server=127.0.0.1:9",
    "--proxy-bypass-list=<-loopback>",
    "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
];
/// What the page waits for before it prints: the load event, then the fonts.
const LOADED: &str = "new Promise((resolve) => { const done = () => document.fonts.ready.then(() => resolve(true)); \
    if (document.readyState === 'complete') done(); else window.addEventListener('load', done, { once: true }); })";

/// Find the binary and install the engine into core, or install the reason PDF is not available. The settings are the
/// conf's `screen-facade` `pdf` element. The returned guard removes the engine and stops the browser when it drops.
#[must_use = "dropping the guard stops the PDF browser"]
pub fn install(factory: &moqui_core::context::ExecutionContextFactory) -> PdfGuard {
    let runtime_dir = factory.runtime_dir();
    let mut configured = factory.conf_setting("screen-facade/pdf", "chromium-path");
    if configured.is_empty() {
        configured = moqui_lib::env::get_prop_or_env("pdf_chromium_path");
    }
    match find_chromium(runtime_dir, &configured) {
        Ok(binary) => {
            let temp_dir = runtime_dir.join("tmp");
            remove_orphan_profiles(&temp_dir);
            let pages = factory
                .conf_number("screen-facade/pdf", "page-limit", PAGE_LIMIT)
                .max(1);
            let timeout = factory
                .conf_number("screen-facade/pdf", "timeout", TIMEOUT_SECONDS)
                .max(1);
            let preview_max = factory.conf_number("screen-facade/pdf", "preview-max-size", PREVIEW_MAX_SIZE);
            let engine =
                Arc::new(ChromiumPdf::with_limits(binary, temp_dir, pages, timeout).with_preview_max_size(preview_max));
            moqui_core::screen::install_pdf_engine(engine.clone());
            PdfGuard { engine: Some(engine) }
        }
        Err(reason) => {
            moqui_core::screen::install_pdf_unavailable(reason);
            PdfGuard { engine: None }
        }
    }
}

/// Remove the `chromium-<pid>-<n>` profiles under `temp_dir` whose process is gone, such as those a server that
/// crashed left behind. A profile is removed when its browser stops, so one of a live process is in use.
fn remove_orphan_profiles(temp_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(temp_dir) else {
        return;
    };
    let own = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix("chromium-"))
            .and_then(|rest| rest.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own || process_live(pid) {
            continue;
        }
        let dir = entry.path();
        if let Err(err) = std::fs::remove_dir_all(&dir) {
            tracing::warn!("could not remove the old PDF browser profile {}: {err}", dir.display());
        }
    }
}

/// True while process `pid` exists.
fn process_live(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        return Path::new("/proc").join(pid.to_string()).exists();
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Removes the engine from core and stops its browser.
pub struct PdfGuard {
    engine: Option<Arc<ChromiumPdf>>,
}

impl Drop for PdfGuard {
    fn drop(&mut self) {
        moqui_core::screen::clear_pdf_engine();
        if let Some(engine) = self.engine.take() {
            engine.shutdown();
        }
    }
}

/// The engine.
pub struct ChromiumPdf {
    inner: Arc<Inner>,
}

struct Inner {
    binary: PathBuf,
    temp_dir: PathBuf,
    pages: Semaphore,
    timeout: Duration,
    preview_max: usize,
    live: Mutex<Option<Live>>,
}

/// A started browser.
struct Live {
    connection: Connection,
    process: ChromiumProcess,
}

impl ChromiumPdf {
    /// An engine for `binary` whose profiles go under `temp_dir`, with the default limits. Nothing starts until the
    /// first render.
    #[must_use]
    pub fn new(binary: PathBuf, temp_dir: PathBuf) -> Self {
        Self::with_limits(binary, temp_dir, PAGE_LIMIT, TIMEOUT_SECONDS)
    }

    /// [`Self::new`] with `pages` printing at once and `timeout` seconds for one document.
    #[must_use]
    pub fn with_limits(binary: PathBuf, temp_dir: PathBuf, pages: u64, timeout: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                binary,
                temp_dir,
                pages: Semaphore::new(usize::try_from(pages).unwrap_or(4)),
                timeout: Duration::from_secs(timeout),
                preview_max: usize::try_from(PREVIEW_MAX_SIZE).unwrap_or(usize::MAX),
                live: Mutex::new(None),
            }),
        }
    }

    /// This engine with `bytes` as the largest PDF it previews. Call it before the engine is shared.
    #[must_use]
    pub fn with_preview_max_size(mut self, bytes: u64) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.preview_max = usize::try_from(bytes).unwrap_or(usize::MAX);
        }
        self
    }

    /// The binary this engine starts.
    #[must_use]
    pub fn binary(&self) -> &Path {
        &self.inner.binary
    }

    /// Stop the browser, if one is running. A later render starts it again.
    pub fn shutdown(&self) {
        let inner = Arc::clone(&self.inner);
        let _ = crate::edge::block_on(async move {
            inner.live.lock().await.take();
        });
    }
}

impl moqui_core::screen::PdfEngine for ChromiumPdf {
    fn render_png(&self, html_document: &str) -> Result<Vec<u8>, moqui_core::context::Error> {
        let inner = Arc::clone(&self.inner);
        let html = html_document.to_string();
        crate::edge::block_on(async move { inner.render_output(html, true).await })
            .and_then(|rendered| rendered)
            .map_err(|err| moqui_core::context::Error::new(err.to_string()))
    }
    fn render(&self, html_print_document: &str) -> Result<Vec<u8>, moqui_core::context::Error> {
        let inner = Arc::clone(&self.inner);
        let html = html_print_document.to_string();
        crate::edge::block_on(async move { inner.render(html).await })
            .and_then(|rendered| rendered)
            .map_err(|err| moqui_core::context::Error::new(err.to_string()))
    }

    fn preview(
        &self,
        request: &moqui_core::screen::PdfPreviewRequest<'_>,
    ) -> Result<moqui_core::screen::PdfPreview, moqui_core::context::Error> {
        if request.pdf.len() > self.inner.preview_max {
            return Err(moqui_core::context::Error::new(format!(
                "the PDF is {} bytes, larger than the {} bytes previewed (pdf preview-max-size)",
                request.pdf.len(),
                self.inner.preview_max
            )));
        }
        let inner = Arc::clone(&self.inner);
        let expression = preview_expression(request);
        crate::edge::block_on(async move { inner.preview(expression).await })
            .and_then(|previewed| previewed)
            .map_err(|err| moqui_core::context::Error::new(err.to_string()))
    }
}

/// The script a preview page evaluates: it resolves to `{png, width, height, pages, text}`, `png` base64.
fn preview_expression(request: &moqui_core::screen::PdfPreviewRequest<'_>) -> String {
    let engine = base64::engine::general_purpose::STANDARD;
    let module = |source: &str| format!("data:text/javascript;base64,{}", engine.encode(source));
    let quoted = |text: String| serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "({PREVIEW_SCRIPT})({main}, {worker}, {pdf}, {image}, {max_size}, {text_pages}, {max_chars})",
        main = quoted(module(&request.library.main)),
        worker = quoted(module(&request.library.worker)),
        pdf = quoted(engine.encode(request.pdf)),
        image = request.image,
        max_size = request.max_size.clamp(16, 4096),
        text_pages = request.text_pages,
        max_chars = request.max_chars,
    )
}

/// The preview, as a function of the module URLs, the PDF in base64, and the limits. pdf.js finds its worker in
/// `globalThis.pdfjsWorker` and runs it on this thread (its "fake worker"), so no Worker script is fetched.
const PREVIEW_SCRIPT: &str = r##"async (mainUrl, workerUrl, pdfBase64, image, maxSize, textPages, maxChars) => {
    const pdfjs = await import(mainUrl);
    globalThis.pdfjsWorker = await import(workerUrl);
    const data = typeof Uint8Array.fromBase64 === "function" ? Uint8Array.fromBase64(pdfBase64)
        : Uint8Array.from(atob(pdfBase64), (c) => c.charCodeAt(0));
    const task = pdfjs.getDocument({ data, enableXfa: false, isEvalSupported: false, useWasm: false,
        useSystemFonts: true, stopAtErrors: false });
    const doc = await task.promise;
    try {
        const out = { png: "", width: 0, height: 0, pages: doc.numPages, text: "" };
        if (image && doc.numPages > 0) {
            const page = await doc.getPage(1);
            const base = page.getViewport({ scale: 1 });
            const viewport = page.getViewport({ scale: Math.min(maxSize / base.width, maxSize / base.height) });
            const canvas = document.querySelector("canvas") || document.body.appendChild(document.createElement("canvas"));
            canvas.width = Math.max(1, Math.floor(viewport.width));
            canvas.height = Math.max(1, Math.floor(viewport.height));
            const canvasContext = canvas.getContext("2d");
            await page.render({ canvas, canvasContext, viewport, background: "#ffffff" }).promise;
            out.png = canvas.toDataURL("image/png").replace(/^data:image\/png;base64,/, "");
            out.width = canvas.width;
            out.height = canvas.height;
        }
        let text = "";
        for (let n = 1; n <= Math.min(doc.numPages, textPages) && text.length < maxChars; n++) {
            const content = await (await doc.getPage(n)).getTextContent();
            for (const item of content.items) {
                if (typeof item.str === "string") text += item.str + (item.hasEOL ? "\n" : "");
            }
            text += "\n";
        }
        out.text = text.length > maxChars ? text.slice(0, maxChars) : text;
        return out;
    } finally {
        await task.destroy();
    }
}"##;

impl Inner {
    async fn render(&self, html: String) -> Result<Vec<u8>, Error> {
        self.render_output(html, false).await
    }

    async fn render_output(&self, html: String, png: bool) -> Result<Vec<u8>, Error> {
        let _permit = self
            .pages
            .acquire()
            .await
            .map_err(|_| Error::new("the PDF engine is shutting down"))?;
        let connection = self.connection().await?;
        let page = Page::open(&connection, self.timeout).await?;
        let printed = match tokio::time::timeout(self.timeout, page.print_output(&html, png)).await {
            Ok(printed) => printed,
            Err(_) => Err(Error::new(format!(
                "PDF render did not finish in {} seconds",
                self.timeout.as_secs()
            ))),
        };
        page.close().await;
        printed
    }

    async fn preview(&self, expression: String) -> Result<moqui_core::screen::PdfPreview, Error> {
        let _permit = self
            .pages
            .acquire()
            .await
            .map_err(|_| Error::new("the PDF engine is shutting down"))?;
        let connection = self.connection().await?;
        let page = Page::open(&connection, self.timeout).await?;
        let previewed = match tokio::time::timeout(self.timeout, page.preview(&expression)).await {
            Ok(previewed) => previewed,
            Err(_) => Err(Error::new(format!(
                "PDF preview did not finish in {} seconds",
                self.timeout.as_secs()
            ))),
        };
        page.close().await;
        previewed
    }

    /// The browser connection, starting the browser (or starting it again after it exited) as needed.
    async fn connection(&self) -> Result<Connection, Error> {
        let mut live = self.live.lock().await;
        if let Some(current) = live.as_mut() {
            if !current.connection.is_closed() && current.process.running() {
                return Ok(current.connection.clone());
            }
            tracing::warn!("the PDF browser exited; starting it again");
            live.take();
        }
        let binary = self.binary.clone();
        let temp_dir = self.temp_dir.clone();
        let (process, url) = tokio::task::spawn_blocking(move || ChromiumProcess::spawn(&binary, &temp_dir))
            .await
            .map_err(|err| Error::new(format!("the PDF browser start panicked: {err}")))??;
        let connection = Connection::connect(&url).await?;
        tracing::info!(binary = %self.binary.display(), "started the PDF browser");
        *live = Some(Live {
            connection: connection.clone(),
            process,
        });
        Ok(connection)
    }
}

/// One page, attached with a flat session. Its requests are failed by a task until the page closes.
struct Page {
    connection: Connection,
    target: String,
    session: String,
    timeout: Duration,
    blocker: tokio::task::JoinHandle<()>,
}

impl Page {
    async fn open(connection: &Connection, timeout: Duration) -> Result<Self, Error> {
        let created = connection
            .call(None, "Target.createTarget", json!({ "url": "about:blank" }), timeout)
            .await?;
        let target = text_of(&created, "targetId")?;
        let attached = connection
            .call(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target, "flatten": true }),
                timeout,
            )
            .await;
        let session = match attached.and_then(|attached| text_of(&attached, "sessionId")) {
            Ok(session) => session,
            Err(err) => {
                let _ = connection
                    .call(None, "Target.closeTarget", json!({ "targetId": target }), timeout)
                    .await;
                return Err(err);
            }
        };
        let mut events = connection.events(&session);
        let blocker_connection = connection.clone();
        let blocker_session = session.clone();
        let blocker = tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if event.get("method").and_then(Value::as_str) != Some("Fetch.requestPaused") {
                    continue;
                }
                let params = event.get("params").cloned().unwrap_or(Value::Null);
                let url = params.pointer("/request/url").and_then(Value::as_str).unwrap_or("");
                tracing::debug!(%url, "PDF page request blocked");
                let fail = json!({ "requestId": params.get("requestId"), "errorReason": "BlockedByClient" });
                let _ = blocker_connection
                    .call(Some(&blocker_session), "Fetch.failRequest", fail, timeout)
                    .await;
            }
        });
        Ok(Self {
            connection: connection.clone(),
            target,
            session,
            timeout,
            blocker,
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, Error> {
        self.connection
            .call(Some(&self.session), method, params, self.timeout)
            .await
    }

    async fn print_output(&self, html: &str, png: bool) -> Result<Vec<u8>, Error> {
        // With no patterns every request pauses; data: URLs never do.
        self.call("Fetch.enable", json!({})).await?;
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame = tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("the PDF page has no frame"))?
            .to_string();
        self.call("Page.setDocumentContent", json!({ "frameId": frame, "html": html }))
            .await
            .map_err(|err| Error::new(format!("could not load the html-print document: {err}")))?;
        let loaded = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": LOADED, "awaitPromise": true, "returnByValue": true }),
            )
            .await?;
        if let Some(thrown) = loaded.get("exceptionDetails") {
            return Err(Error::new(format!(
                "the html-print document did not finish loading: {thrown}"
            )));
        }
        if html.contains("name=\"moqui-report\"") {
            let ready = self.call("Runtime.evaluate", json!({"expression": "new Promise((resolve,reject)=>{const until=Date.now()+25000;const check=()=>{if(window.moquiReportError)reject(new Error(window.moquiReportError));else if(window.moquiReportRendered===true)resolve(true);else if(Date.now()>until)reject(new Error('report render timed out'));else setTimeout(check,20);};check();})", "awaitPromise":true,"returnByValue":true})).await?;
            if ready.get("exceptionDetails").is_some() {
                return Err(Error::new("dashboard charts did not finish rendering"));
            }
        }
        if png {
            let metrics = self.call("Page.getLayoutMetrics", json!({})).await?;
            let size = metrics.get("cssContentSize").or_else(|| metrics.get("contentSize"));
            let width = size
                .and_then(|s| s.get("width"))
                .and_then(Value::as_f64)
                .unwrap_or(1200.0)
                .clamp(1.0, 8192.0);
            let height = size
                .and_then(|s| s.get("height"))
                .and_then(Value::as_f64)
                .unwrap_or(800.0)
                .clamp(1.0, 16384.0);
            let captured = self.call("Page.captureScreenshot", json!({"format":"png","captureBeyondViewport":true,"clip":{"x":0,"y":0,"width":width,"height":height,"scale":1}})).await?;
            let data = captured
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::new("Chromium returned no PNG data"))?;
            return base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|error| Error::new(format!("Chromium returned invalid PNG data: {error}")));
        }
        let printed = self
            .call(
                "Page.printToPDF",
                json!({ "preferCSSPageSize": true, "printBackground": true, "transferMode": "ReturnAsBase64" }),
            )
            .await
            .map_err(|err| Error::new(format!("Chromium could not print the document: {err}")))?;
        let data = printed
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("Chromium returned no PDF data"))?;
        base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|err| Error::new(format!("Chromium returned PDF data that is not base64: {err}")))
    }

    /// Run [`PREVIEW_SCRIPT`] (inside `expression`) on a page whose content is a canvas.
    async fn preview(&self, expression: &str) -> Result<moqui_core::screen::PdfPreview, Error> {
        self.call("Fetch.enable", json!({})).await?;
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame = tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("the PDF page has no frame"))?
            .to_string();
        self.call(
            "Page.setDocumentContent",
            json!({ "frameId": frame, "html": "<!DOCTYPE html><html><body><canvas></canvas></body></html>" }),
        )
        .await?;
        let evaluated = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
            )
            .await?;
        if let Some(thrown) = evaluated.get("exceptionDetails") {
            let message = thrown
                .pointer("/exception/description")
                .or_else(|| thrown.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(Error::new(format!("pdf.js could not preview the PDF: {message}")));
        }
        let value = evaluated
            .pointer("/result/value")
            .ok_or_else(|| Error::new("the PDF preview returned nothing"))?;
        let number = |name: &str| {
            value
                .get(name)
                .and_then(|number| number.as_f64())
                .filter(|number| number.is_finite() && *number >= 0.0)
                .map_or(0, |number| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let whole = number as u32;
                    whole
                })
        };
        let png = match value.get("png").and_then(Value::as_str) {
            Some(data) if !data.is_empty() => base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|err| Error::new(format!("the PDF preview image is not base64: {err}")))?,
            _ => Vec::new(),
        };
        Ok(moqui_core::screen::PdfPreview {
            png,
            width: number("width"),
            height: number("height"),
            pages: number("pages"),
            text: value.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
        })
    }

    async fn close(self) {
        self.blocker.abort();
        self.connection.forget(&self.session);
        let _ = self
            .connection
            .call(
                None,
                "Target.closeTarget",
                json!({ "targetId": self.target }),
                self.timeout,
            )
            .await;
    }
}

fn text_of(value: &Value, name: &str) -> Result<String, Error> {
    value
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::new(format!("the browser answered without {name}")))
}

/// The browser process and its profile directory. Drop kills the process group and removes the profile.
struct ChromiumProcess {
    child: Child,
    profile: PathBuf,
}

impl ChromiumProcess {
    /// Start the browser with a fresh profile and wait for its DevTools address.
    fn spawn(binary: &Path, temp_dir: &Path) -> Result<(Self, String), Error> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let profile = temp_dir.join(format!(
            "chromium-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)
            .map_err(|err| Error::new(format!("could not create {}", profile.display())).with_source(err))?;
        let child = Command::new(binary)
            .args(ISOLATION_ARGS)
            .args(launch_args())
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .env_remove("NOTIFY_SOCKET")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|err| Error::new(format!("could not start {}", binary.display())).with_source(err))?;
        let mut process = Self { child, profile };
        let url = devtools_url(&process.profile, &mut process.child, START_WAIT).map_err(|err| {
            if userns_restricted() {
                Error::new(format!(
                    "{err}; AppArmor limits unprivileged user namespaces here, so {} may need an AppArmor profile to \
                     start its sandbox (./moq help chromium-download)",
                    binary.display()
                ))
            } else {
                err
            }
        })?;
        Ok((process, url))
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for ChromiumProcess {
    fn drop(&mut self) {
        // The renderer and GPU processes are in the same group.
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", self.child.id())])
            .stderr(Stdio::null())
            .status();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

#[cfg(test)]
mod tests {
    use super::remove_orphan_profiles;

    #[test]
    fn profiles_of_gone_processes_are_removed_at_install() {
        let temp = std::env::temp_dir().join(format!("moqui-pdf-sweep-{}", std::process::id()));
        let own = temp.join(format!("chromium-{}-0", std::process::id()));
        // Above the Linux pid limit, so no process has it.
        let gone = temp.join("chromium-99999999-3");
        let other = temp.join("chromium-cache");
        for dir in [&own, &gone, &other] {
            std::fs::create_dir_all(dir).unwrap();
        }
        remove_orphan_profiles(&temp);
        assert!(own.is_dir());
        assert!(!gone.exists());
        assert!(other.is_dir());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
