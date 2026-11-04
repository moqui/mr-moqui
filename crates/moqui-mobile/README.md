# moqui-mobile

The mobile app: the client of an MR Moqui server ([moqui-client](../moqui-client/README.md)) in a
[Tauri](https://tauri.app) app on the platform's WebView, Android first (MD6 in
[Mobile-Desktop](../../plans/Mobile-Desktop.md)). For now it is the stage 5 spike: a static page (`dist/`) that shows
the outcome of one HTTPS request moqui-client makes from Rust (`src/lib.rs`), with TLS checked by Android's trust
manager through rustls-platform-verifier.

- **Tauri version:** 3.0.0-alpha.5, the desktop app's, so the client glue can be shared, on `tauri-runtime-wry` (the
  system WebView) instead of the desktop's CEF. It builds and runs on Android as it is.
- **The TLS verifier:** `rustls_platform_verifier::android::init_with_env` (0.7 has no `init_hosted`; this takes the
  same JNI env and context) runs in the app's setup, with the JVM and application context tao records in `ndk-context`. Its Kotlin component
  is the Maven artifact `org.rustls:rustls-platform-verifier`, added in `gen/android/app/build.gradle.kts` at the
  version of `rustls-platform-verifier-android` in `Cargo.lock`, and kept from ProGuard in `proguard-rules.pro`.
- **`gen/android`** is the Gradle project `cargo tauri android init` made, kept in git with those changes; its
  `.gitignore` files leave out the build outputs and what the Tauri CLI writes on each build.

The crate is not a member of the repository's workspace. Its `Cargo.lock` is copied from the repository's by
`./moq android`, as the desktop app's.

## Run

Needs the Android SDK (`cmdline-tools`, `platform-tools`, `emulator`, a platform, an NDK and the
`system-images;android-36;google_apis_playstore;x86_64` image), a JDK 17 or later, the Tauri CLI of the desktop app and
the Rust targets ([Build-Test](../../docs/Build-Test.md#android-app)):

```sh
./moq android emulator                        # start the moqui-36 AVD in a hidden window (--window shows it); adb emu kill
./moq android dev                             # build, install and run on the emulator, or on a phone over adb
./moq android build --target x86_64 --apk     # a debug APK under gen/android/app/build/outputs/apk
adb logcat -s moqui-mobile                    # the app's log
```

`MOQUI_MOBILE_PROBE_URL`, set at build time, is the server the spike asks (default `https://example.com`).

## Test

```sh
scripts/check-android.sh    # clippy for aarch64-linux-android and x86_64-linux-android, on moqui-client and this crate
```
