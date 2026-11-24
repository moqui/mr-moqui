# Patched crates of the desktop app

`[patch.crates-io]` in `../Cargo.toml` replaces these crates for the desktop app only (its own workspace). Each is the
crate's published source, less its tests, with the changes below marked `MR Moqui patch` in the code.

## tauri-runtime-cef 3.0.0-alpha.7

On macOS every webview is CEF's Alloy style, which has no screen picker and refuses desktop capture
(docs/ui/Desktop-Client.md "Screen share"). The app shows its own list of displays and windows (`src/capture.rs`), and
the page asks for the one picked by its desktop media id (`getUserMedia` with `chromeMediaSource: "desktop"`).

- `src/cef_impl/client/permission.rs`: `grant_next_desktop_capture(within)` arms one grant; the next media access
  request for desktop capture alone (no camera or microphone with it) within that time, which the application's
  handler answers `Allow`, is granted. Any other desktop request keeps the published behavior: Chromium's picker under
  Chrome style, refused under Alloy style.
- `src/lib.rs`, `src/cef_impl/client/mod.rs`: the function exported.
