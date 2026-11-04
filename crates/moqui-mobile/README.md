# moqui-mobile

The mobile app: the client of an MR Moqui server in a [Tauri](https://tauri.app) app on the platform's WebView, Android
first (MD6 in [Mobile-Desktop](../../plans/Mobile-Desktop.md)). It is the desktop app's client mode on a phone: the
bundled shell opens with no network, each account's data is kept in a store for offline use, and what is queued is
sent when the server can be reached. How it works on Android is [docs/ui/Mobile-Client.md](../../docs/ui/Mobile-Client.md).

- **Code:** `src/` is the phone's part: one webview (`host.rs`), the schemes on Android's WebView (`scheme.rs`), the
  Keystore key store (`keystore.rs`, `android.rs` and `gen/android/.../KeyVault.kt`), its own commands
  (`commands.rs`), the page bridge (`bridge.js`), the lifecycle (`lib.rs`), and the incoming call on a locked phone
  (`calls.rs`, `native.rs` and the Kotlin `CallManager`, `CallConnectionService`, `CallService`,
  `IncomingCallActivity`, `Native`, `Push`; [Mobile-Client](../../docs/ui/Mobile-Client.md#the-incoming-call-mt2)).
  The rest is shared with the desktop
  app: [moqui-client](../moqui-client/README.md) and [moqui-client-tauri](../moqui-client-tauri/README.md).
- **The shell** is the desktop's (`crates/moqui-desktop/shell`), staged by `./moq android` into
  `target/mobile-shell` and embedded as the app's Tauri assets (`frontendDist`). A build without it (`cargo test`)
  gets a placeholder page.
- **`client-conf.xml`**: the build's `<client-conf>`, read at build time.
- **Tauri** 3.0.0-alpha.5, the desktop app's, on `tauri-runtime-wry`. The TLS verifier is
  `rustls_platform_verifier::android::init_with_env` in the app's setup, its Kotlin component a Maven artifact in
  `gen/android/app/build.gradle.kts`; when it fails the app makes no request and answers from the store.
- **`gen/android`** is the Gradle project `cargo tauri android init` made, kept in git with the app's changes (the
  verifier's artifact, `KeyVault.kt`, the SSO intent filter, no backup, the window insets in `MainActivity.kt`, the
  ProGuard rules); its `.gitignore` files leave out what the Tauri CLI writes on each build.
- **Firebase** (native push) is in the build only with `gen/android/app/google-services.json`, the Firebase
  project's, which git ignores; without it push is off and the app still builds. Debug builds take a ring from adb
  (`DebugRingReceiver`, [Mobile-Client](../../docs/ui/Mobile-Client.md#the-incoming-call-mt2)).
- **`patches/wry`**: wry with Tauri's scripts and IPC for the main frame of the app's own origins only, through
  `[patch.crates-io]` ([patches/README.md](patches/README.md)).

The crate is not a member of the repository's workspace. Its `Cargo.lock` is copied from the repository's by
`./moq android`, as the desktop app's.

## Run

Needs the Android SDK, a JDK 17 or later, the Tauri CLI of the desktop app and the Rust targets
([Build-Test](../../docs/Build-Test.md#android-app)):

```sh
./moq android emulator                        # start the moqui-36 AVD in a hidden window (--window shows it)
./moq android dev                             # build, install and run on the emulator, or on a phone over adb
./moq android build --target x86_64 --apk     # a debug APK under gen/android/app/build/outputs/apk
adb reverse tcp:8080 tcp:8080                 # a server on this machine (./moq run) as the app's http://localhost:8080
adb logcat -s moqui-mobile                    # the app's log
```

## Test

```sh
cd crates/moqui-mobile && CARGO_TARGET_DIR=../../target cargo test   # the host logic, on the desktop
scripts/check-android.sh      # clippy for aarch64-linux-android and x86_64-linux-android
scripts/android-smoke.sh      # on the emulator, when asked: the shell's ready line, the frame check, a relaunch
```
