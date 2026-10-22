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

//! PDF output for the `html-print` mode. The engine is the host's (`plans/archive/Architecture-Proposal.md`, Tool components
//! and print): one long-lived Chromium. Core has no browser, so the host installs a [`PdfEngine`] at startup. With
//! none installed, or a build without the `pdf` feature, a request for PDF bytes reports that PDF is not available.
//!
//! The engine's page makes no request, so an `<img>` whose `src` is a resource location (`component://`,
//! `dbresource://`, and the other schemes of the resource facade) is given its bytes as a `data:` URL first, as Java's
//! FOP resolved image URIs through the resource facade. An `http` or `https` source is not fetched.
//!
//! The same engine previews a PDF ([`PdfRenderer::preview`]): the vendored pdf.js (`webroot/libs/pdfjs`) runs in the
//! engine's page, draws page 1 to a PNG and reads the text of the first pages. An engine that has no page to run it in
//! keeps the default [`PdfEngine::preview`], which reports that previews are not supported.

use std::borrow::Cow;
use std::sync::{Arc, OnceLock, RwLock};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_lib::sync::{read, write};

use crate::context::{Error, ExecutionContext};

/// What a caller sees when it asks for PDF bytes and the host installed no engine.
pub const PDF_UNAVAILABLE: &str =
    "PDF is not available: no Chromium binary was found (run ./moq chromium-download or set pdf_chromium_path)";

/// Turns an `html-print` document into PDF bytes. It blocks the calling worker until the bytes come back, so a
/// caller renders the document first and holds no enlisted connection while it waits.
pub trait PdfEngine: Send + Sync {
    /// # Errors
    ///
    /// Returns an error when the browser fails, exits, or the render passes its timeout.
    fn render(&self, html_print_document: &str) -> Result<Vec<u8>, Error>;

    /// Page 1 of a PDF as a PNG, with the page count and the text of the first pages. Blocks like [`Self::render`].
    ///
    /// # Errors
    ///
    /// Returns an error when the engine cannot preview (the default), the PDF does not open, or the browser fails.
    fn preview(&self, request: &PdfPreviewRequest<'_>) -> Result<PdfPreview, Error> {
        let _ = request;
        Err(Error::new(PREVIEW_UNSUPPORTED))
    }
}

/// What the default [`PdfEngine::preview`] reports.
pub const PREVIEW_UNSUPPORTED: &str = "PDF preview is not supported by the installed PDF engine";

/// Where the vendored pdf.js modules are.
pub const PDFJS_LOCATION: &str = "component://webroot/screen/webroot/libs/pdfjs/pdf.min.mjs";
/// The pdf.js worker module, run on the page's main thread.
pub const PDFJS_WORKER_LOCATION: &str = "component://webroot/screen/webroot/libs/pdfjs/pdf.worker.min.mjs";

/// The source text of the pdf.js modules, given to the engine's page.
#[derive(Debug)]
pub struct PdfLibrary {
    /// `pdf.min.mjs`.
    pub main: String,
    /// `pdf.worker.min.mjs`.
    pub worker: String,
}

/// What to preview, and how much.
pub struct PdfPreviewRequest<'a> {
    /// The PDF.
    pub pdf: &'a [u8],
    /// pdf.js.
    pub library: &'a PdfLibrary,
    /// Whether to draw page 1; false reads only the text.
    pub image: bool,
    /// The longest side of the image, in pixels.
    pub max_size: u32,
    /// The pages whose text is read, from page 1.
    pub text_pages: u32,
    /// The most characters of text returned.
    pub max_chars: usize,
}

/// A preview: page 1 as a PNG (empty when no image was asked for), its size, the page count, and the text.
#[derive(Debug, Default)]
pub struct PdfPreview {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pages: u32,
    pub text: String,
}

enum Installed {
    Engine(Arc<dyn PdfEngine>),
    /// Why there is no engine, reported to every caller.
    Unavailable(String),
}

fn slot() -> &'static RwLock<Option<Installed>> {
    static SLOT: RwLock<Option<Installed>> = RwLock::new(None);
    &SLOT
}

/// Use `engine` for every PDF in this process.
pub fn install_pdf_engine(engine: Arc<dyn PdfEngine>) {
    *write(slot()) = Some(Installed::Engine(engine));
}

/// No engine; every caller gets `reason`, for example that the build left out the `pdf` feature.
pub fn install_pdf_unavailable(reason: impl Into<String>) {
    *write(slot()) = Some(Installed::Unavailable(reason.into()));
}

/// Remove the installed engine or reason.
pub fn clear_pdf_engine() {
    *write(slot()) = None;
}

/// The PDF renderer: the installed engine.
pub struct PdfRenderer;

impl PdfRenderer {
    /// Whether an engine is installed.
    #[must_use]
    pub fn available() -> bool {
        matches!(*read(slot()), Some(Installed::Engine(_)))
    }

    /// PDF bytes for an `html-print` document, its resource images inlined.
    ///
    /// # Errors
    ///
    /// Returns [`PDF_UNAVAILABLE`] (or the installed reason) when there is no engine, else the engine's error.
    pub fn render(html_print_document: &str) -> Result<Vec<u8>, Error> {
        let engine = match &*read(slot()) {
            Some(Installed::Engine(engine)) => Arc::clone(engine),
            Some(Installed::Unavailable(reason)) => return Err(Error::new(reason.clone())),
            None => return Err(Error::new(PDF_UNAVAILABLE)),
        };
        engine.render(&inline_images(html_print_document))
    }

    /// A preview of `pdf` from the installed engine: page 1 drawn to fit `max_size` pixels when `image` is set, and up
    /// to `max_chars` of the text of the first `text_pages` pages.
    ///
    /// # Errors
    ///
    /// Returns [`PDF_UNAVAILABLE`] (or the installed reason) when there is no engine, an error when pdf.js cannot be
    /// read, else the engine's error.
    pub fn preview(
        pdf: &[u8],
        image: bool,
        max_size: u32,
        text_pages: u32,
        max_chars: usize,
    ) -> Result<PdfPreview, Error> {
        let engine = match &*read(slot()) {
            Some(Installed::Engine(engine)) => Arc::clone(engine),
            Some(Installed::Unavailable(reason)) => return Err(Error::new(reason.clone())),
            None => return Err(Error::new(PDF_UNAVAILABLE)),
        };
        let library = pdf_library()?;
        engine.preview(&PdfPreviewRequest {
            pdf,
            library: &library,
            image,
            max_size,
            text_pages,
            max_chars,
        })
    }
}

/// pdf.js, read once through the resource facade. A failed read is not kept, so a later call tries again.
fn pdf_library() -> Result<Arc<PdfLibrary>, Error> {
    static LIBRARY: OnceLock<Arc<PdfLibrary>> = OnceLock::new();
    if let Some(library) = LIBRARY.get() {
        return Ok(Arc::clone(library));
    }
    let read = |location: &str| {
        let text = ExecutionContext::with_current(|ctx| {
            ctx.factory()
                .resource()
                .get(location)
                .and_then(|resource| resource.read_text())
        })
        .unwrap_or_else(|| Err(Error::new("no execution context is entered")));
        // A test context has no webroot component.
        #[cfg(test)]
        let text = text.or_else(|_| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../runtime/base-component/webroot/screen/webroot")
                .join(location.trim_start_matches("component://webroot/screen/webroot/"));
            std::fs::read_to_string(path).map_err(|err| Error::new(err.to_string()))
        });
        text.map_err(|err| Error::new(format!("pdf.js is not available at {location}: {}", err.message())))
    };
    let library = Arc::new(PdfLibrary {
        main: read(PDFJS_LOCATION)?,
        worker: read(PDFJS_WORKER_LOCATION)?,
    });
    Ok(Arc::clone(LIBRARY.get_or_init(|| library)))
}

/// The document with the `src` of each `<img>` that names a resource location replaced by a `data:` URL of the image.
/// A source that cannot be read, or is not an image Chromium shows, is left as written with a warning naming it.
fn inline_images(html: &str) -> Cow<'_, str> {
    let lower = html.to_ascii_lowercase();
    if !lower.contains("<img") {
        return Cow::Borrowed(html);
    }
    let mut out = String::with_capacity(html.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = lower[from..].find("<img") {
        let start = from + found + "<img".len();
        let Some((value, end)) = src_value(html, start) else {
            from = start;
            continue;
        };
        from = end;
        let location = decode_attribute(html[value.clone()].trim());
        if let Some(url) = data_url(&location) {
            out.push_str(&html[copied..value.start]);
            out.push_str(&url);
            copied = value.end;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(html);
    }
    out.push_str(&html[copied..]);
    Cow::Owned(out)
}

/// The byte range of the `src` value of the tag whose attributes start at `start`, and where the tag ends. `None`
/// when the tag has no `src`.
fn src_value(html: &str, start: usize) -> Option<(std::ops::Range<usize>, usize)> {
    let bytes = html.as_bytes();
    // `<imgx` is another element
    if !bytes
        .get(start)
        .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/')
    {
        return None;
    }
    let mut index = start;
    let mut src = None;
    loop {
        while bytes
            .get(index)
            .is_some_and(|byte| byte.is_ascii_whitespace() || *byte == b'/')
        {
            index += 1;
        }
        match bytes.get(index) {
            None => return None,
            Some(b'>') => return src.map(|range| (range, index + 1)),
            Some(_) => {}
        }
        let name_start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/'))
        {
            index += 1;
        }
        let name = &html[name_start..index];
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'=') {
            continue;
        }
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        let value = match bytes.get(index) {
            Some(quote @ (b'"' | b'\'')) => {
                let close = html[index + 1..].find(char::from(*quote))? + index + 1;
                let range = index + 1..close;
                index = close + 1;
                range
            }
            _ => {
                let value_start = index;
                while bytes
                    .get(index)
                    .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
                {
                    index += 1;
                }
                value_start..index
            }
        };
        if src.is_none() && name.eq_ignore_ascii_case("src") {
            src = Some(value);
        }
    }
}

/// An attribute value with the entities a template's HTML escaping writes turned back into text.
fn decode_attribute(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&#x27;", "'")
            .replace("&amp;", "&"),
    )
}

/// The `data:` URL for the image at `location`, or `None` to leave the source as written: a `data:` URL already, an
/// `http` or `https` URL, a path with no resource scheme (which the resource facade would read as a file), a location
/// that cannot be read, or bytes that are not an image.
fn data_url(location: &str) -> Option<String> {
    let scheme = match location.find(':') {
        Some(index) if index > 1 && !location[..index].contains('/') => location[..index].to_ascii_lowercase(),
        _ => {
            tracing::warn!(src = location, "PDF image not inlined: it is not a resource location");
            return None;
        }
    };
    if matches!(scheme.as_str(), "data" | "http" | "https") {
        if scheme != "data" {
            tracing::debug!(src = location, "PDF image not inlined: URLs are not fetched");
        }
        return None;
    }
    let read = ExecutionContext::with_current(|ctx| {
        ctx.factory()
            .resource()
            .get(location)
            .and_then(|resource| resource.read_bytes())
    });
    let bytes = match read {
        Some(Ok(bytes)) => bytes,
        Some(Err(err)) => {
            tracing::warn!(src = location, "PDF image not inlined: {}", err.message());
            return None;
        }
        None => {
            tracing::warn!(src = location, "PDF image not inlined: no execution context is entered");
            return None;
        }
    };
    let Some(mime) = image_type(&bytes) else {
        tracing::warn!(
            src = location,
            "PDF image not inlined: it is not a png, jpeg, gif, webp, bmp or svg image"
        );
        return None;
    };
    Some(format!("data:{mime};base64,{}", STANDARD.encode(&bytes)))
}

/// The media type of an image Chromium shows, from its leading bytes: png, jpeg, gif, webp or bmp, else svg text.
/// No image decoder is needed for this, so PDF does not depend on the `image` feature.
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        String::from_utf8_lossy(&bytes[..bytes.len().min(1024)])
            .contains("<svg")
            .then_some("image/svg+xml")
    }
}

/// Tests that install an engine hold this, since the slot is process-wide.
#[cfg(test)]
pub(crate) fn pdf_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    moqui_lib::sync::lock(&LOCK)
}
