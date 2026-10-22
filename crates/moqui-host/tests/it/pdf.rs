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

//! The Chromium PDF engine on its own: no database and no server. The browser is `pdf_chromium_path`, else the one in
//! `runtime/chromium`, else `chromium`, `chromium-browser` or `google-chrome` on the `PATH`; with none the test logs
//! that and passes.
#![cfg(feature = "pdf")]

use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::screen::PdfEngine;
use moqui_host::pdf::ChromiumPdf;

fn browser() -> Option<PathBuf> {
    if let Ok(found) = moqui_host::pdf::find_chromium(
        &crate::common::repo_runtime(),
        &moqui_lib::env::get_prop_or_env("pdf_chromium_path"),
    ) {
        return Some(found);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join("google-chrome"))
        .find(|candidate| candidate.is_file())
}

#[test]
fn an_html_print_document_becomes_pdf_and_reaches_no_network() {
    let Some(binary) = browser() else {
        eprintln!("no Chromium binary; skipped the PDF engine test");
        return;
    };
    // A listener the document points at. It must see no connection.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    let watcher = std::thread::spawn(move || {
        for _ in 0..200 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 512];
                let _ = stream.read(&mut buffer);
                counted.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });

    let temp = std::env::temp_dir().join(format!("moqui-pdf-test-{}", std::process::id()));
    let engine = ChromiumPdf::new(binary, temp.clone());
    let html = format!(
        "<!DOCTYPE html><html><head><style>@page {{ size: 8.5in 11in; margin: 0.5in }}</style>\
         <link rel=\"stylesheet\" href=\"http://127.0.0.1:{port}/style.css\"></head>\
         <body><h1>Invoice 100</h1><img src=\"http://127.0.0.1:{port}/logo.png\">\
         <script>fetch('http://127.0.0.1:{port}/fetch').catch(() => {{}})</script>\
         <script>try {{ new WebSocket('ws://127.0.0.1:{port}/socket') }} catch (err) {{}}</script>\
         <script>try {{ new WebSocket('ws://localhost:{port}/socket') }} catch (err) {{}}</script>\
         <table><thead><tr><th>Item</th></tr></thead><tbody><tr><td>Widget</td></tr></tbody></table></body></html>"
    );
    let first = engine.render(&html).expect("PDF bytes");
    assert!(first.starts_with(b"%PDF-"), "{:?}", &first[..first.len().min(16)]);
    // The same browser serves the next document.
    let second = engine.render("<p>second</p>").expect("second PDF");
    assert!(second.starts_with(b"%PDF-"));
    // Concurrent renders share the browser.
    let engine = Arc::new(engine);
    let threads: Vec<_> = (0..3)
        .map(|n| {
            let engine = Arc::clone(&engine);
            std::thread::spawn(move || engine.render(&format!("<p>page {n}</p>")))
        })
        .collect();
    for thread in threads {
        assert!(thread.join().unwrap().unwrap().starts_with(b"%PDF-"));
    }
    engine.shutdown();
    // After a shutdown (as after the browser exits) the next render starts it again.
    assert!(engine.render("<p>again</p>").unwrap().starts_with(b"%PDF-"));
    engine.shutdown();

    watcher.join().unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 0, "the document reached the network");
    let _ = std::fs::remove_dir_all(&temp);
}

/// The vendored pdf.js, as `PdfRenderer::preview` reads it through the resource facade.
fn pdf_library() -> moqui_core::screen::PdfLibrary {
    let libs = crate::common::repo_runtime().join("base-component/webroot/screen/webroot/libs/pdfjs");
    moqui_core::screen::PdfLibrary {
        main: std::fs::read_to_string(libs.join("pdf.min.mjs")).expect("pdf.min.mjs"),
        worker: std::fs::read_to_string(libs.join("pdf.worker.min.mjs")).expect("pdf.worker.min.mjs"),
    }
}

#[test]
fn a_pdf_previews_as_a_png_with_its_text_and_page_count() {
    let Some(binary) = browser() else {
        eprintln!("no Chromium binary; skipped the PDF preview test");
        return;
    };
    let temp = std::env::temp_dir().join(format!("moqui-pdf-preview-test-{}", std::process::id()));
    let engine = ChromiumPdf::new(binary, temp.clone());
    let pdf = engine
        .render(
            "<!DOCTYPE html><html><head><style>@page { size: 8.5in 11in; margin: 0.5in }</style></head>\
             <body><h1>Preview Invoice 100</h1><p>Widget, two each</p></body></html>",
        )
        .expect("PDF bytes");
    let library = pdf_library();
    let request = moqui_core::screen::PdfPreviewRequest {
        pdf: &pdf,
        library: &library,
        image: true,
        max_size: 400,
        text_pages: 5,
        max_chars: 10_000,
    };
    // The modules are imported from data: URLs while every request of the page is failed.
    let preview = engine.preview(&request).expect("a preview");
    assert!(
        preview.png.starts_with(b"\x89PNG\r\n\x1a\n"),
        "{:?}",
        &preview.png[..preview.png.len().min(8)]
    );
    assert_eq!(preview.pages, 1);
    // A letter page is taller than wide, so the height fits the box.
    assert_eq!(preview.height, 400, "{}x{}", preview.width, preview.height);
    assert!(
        preview.width > 250 && preview.width < 400,
        "{}x{}",
        preview.width,
        preview.height
    );
    assert!(preview.text.contains("Preview Invoice 100"), "{:?}", preview.text);
    assert!(preview.text.contains("Widget, two each"), "{:?}", preview.text);

    // Text only, capped.
    let text_only = engine
        .preview(&moqui_core::screen::PdfPreviewRequest {
            image: false,
            max_chars: 7,
            ..request
        })
        .expect("text");
    assert!(text_only.png.is_empty());
    assert_eq!(text_only.text, "Preview");

    // Not a PDF: an error, and the engine goes on.
    let bad = engine.preview(&moqui_core::screen::PdfPreviewRequest {
        pdf: b"not a pdf",
        ..request
    });
    assert!(bad.is_err());
    // Larger than preview-max-size: refused before the browser sees it.
    let small = ChromiumPdf::new(PathBuf::from("/nonexistent"), temp.clone()).with_preview_max_size(4);
    let refused = small.preview(&request).unwrap_err();
    assert!(refused.message().contains("preview-max-size"), "{}", refused.message());
    engine.shutdown();
    let _ = std::fs::remove_dir_all(&temp);
}
