# Patched crates of the mobile app

`[patch.crates-io]` in `../Cargo.toml` replaces these crates for the mobile app only (its own workspace). Each is the
crate's published source, less its examples, with the changes below marked `MR Moqui patch` in the code. Kotlin files
are written into `gen/android/app/src/main/java/.../generated` by the crate's build script on each build, so a patch
there survives `cargo tauri android init`.

## wry 0.57.1

Android's WebView gives frames what a page has, unless told otherwise; wry 0.57.1 tells it otherwise for none
(docs/ui/Mobile-Client.md "One webview"). Without these, a cross-origin iframe of a server page would get Tauri's
invoke key and could call the app's commands as the page, with the account's device key behind them.

- `src/android/kotlin/RustWebView.kt`: the initialization scripts (`addDocumentStartJavaScript`) run in the app's own
  origins only (`PAGE_ORIGINS`: `http://moqui-shell.localhost`, `http://moqui-app.localhost`), not `*`.
- `src/android/kotlin/Ipc.kt` and `src/android/main_pipe.rs`: the IPC is a web message listener
  (`WebViewCompat.addWebMessageListener`) for those origins that drops a message from any frame but the main one,
  instead of `addJavascriptInterface`, which every frame gets. A WebView without web message listeners gets no IPC.
- `src/android/kotlin/RustWebViewClient.kt`: a frame's own navigation is not handed to the app's navigation handler,
  which decides the main frame's.
- `src/android/kotlin/proguard-wry.pro`: keeps `Ipc.attach`, which Rust calls.

To move to a newer wry: copy its published source here (less `examples` and their `[[example]]` tables), apply the
same changes, and check that a cross-origin iframe still cannot invoke (scripts/android-smoke.sh).
