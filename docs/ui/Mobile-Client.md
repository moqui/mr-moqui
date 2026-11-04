# The mobile client

The Android app ([crates/moqui-mobile](../../crates/moqui-mobile/README.md), MD6 and its first step MT1 in
[Mobile-Desktop](../../plans/Mobile-Desktop.md#android-track-md6)): the desktop app's client of a remote server
([Desktop-Client](Desktop-Client.md)) on a phone. The accounts, device keys, sign-in, the store, the offline answers,
the pushes, the outbox and drafts, the mirror and the device modes are the same code
([moqui-client](../../crates/moqui-client/README.md)), and so are the shell's commands and the schemes' answer
([moqui-client-tauri](../../crates/moqui-client-tauri/README.md)). This page is what differs on Android.

## One webview

Tauri on Android has one webview per activity, so the app has one, `main`, that shows either the bundled shell
(`http://moqui-shell.localhost`) or a page of the current account's server (`http://moqui-app.localhost`, the server's
own `/qapps`), never both:

- **A server page** opens in the same webview (the shell's `openRemote`, a link to a server path); it has a small button
  at the bottom left back to the shell (`moquiDesktop.backToShell`), and Android's back goes back through the pages.
  A sign-out, a switch of account or a removal shows the shell again.
- **The bundled comms screens** (Chat, Mail, Compose, Calls) are the shell's, as on the desktop; an encrypted room in a
  server page offers Open in Chat, which shows it in the shell (`open_in_shell`).
- **The boundary is the command set.** Tauri counts both schemes' pages as local, so the one capability
  (`capabilities/main.json`) grants the app's commands to both and never to the web. The shell's commands (accounts,
  sign-in, SSO, sign-out, opening a page) check the page themselves (`ShellHost::is_shell_page`): the webview's URL on
  the shell's origin and a load of the shell begun since the last navigation, so a server page that navigates to the
  shell cannot call them while it is still the page. A server page has the sockets, the outbox and the drafts of the
  current account, as a desktop app window does. The device key never reaches a page.
- **A server page's storage scope** (MoquiLib `scopedKey`) and the scopes it is to clear are asked of the host as the
  page starts (`/__moqui-app/page`, the host's own path on the app origin), since one init script serves every page.

## Android's WebView and the schemes

- **No request bodies.** The WebView hands an app scheme a request's headers but not its body, so the bridge sends a
  page's `fetch` with a body (a transition's POST, an upload) to the host as a command (`scheme_fetch`), which answers it
  as the scheme would and follows a redirect in the page. A form posted by navigation (no `fetch`) arrives without its
  body.
- **No redirect status.** A scheme answer cannot be a 3xx, so a document's redirect is answered as a page that goes to
  its `Location`.
- **The shell's files** are embedded in the app (Tauri's assets of `target/mobile-shell`, which `./moq android` stages
  as the desktop bundle's) and served under the same CSP.

## Keys, store, conf and sign-in

- **Device keys** are sealed by an AES-GCM key of the Android Keystore (`KeyVault.kt`, through JNI), which never leaves
  it, and kept in the app's private files (`client/keys/<account>.sealed`); a device that cannot make the Keystore key
  keeps them in a plain owner-only file there, and the shell says so (`keyStore: file`).
- **The store** of each account is under the app's data directory (`client/data/`), as on the desktop.
- **`<client-conf>`** is the build's `client-conf.xml`, read at build time; a build for a company's shared phones sets
  `device-mode="shared"`.
- **Sign-in** by password as on the desktop. **SSO** opens the system browser (an Android `VIEW` intent) and comes back
  as `org.moqui.mobile:/sso`, an intent filter of the activity, which the server allows by default
  (`webapp_app_redirect_uris`).
- **Requests** are moqui-client's, in Rust, with TLS checked by Android's trust manager (rustls-platform-verifier). They
  do not go through Android's network security config: plain `http://` is refused but to this device
  (`account::base_url`), so a development server is reached through `adb reverse tcp:8080 tcp:8080` as
  `http://localhost:8080`. When the verifier cannot start the app makes no request, answers from the store, and the
  commands that need the server say so.

## Lifecycle

- **Paused** (another app, the screen off): the mirror stops; the store, the sockets and the outbox's sender stay while
  Android keeps the process.
- **Resumed:** the outbox is woken, and the mirror reads ahead again for the shell's account.
- **The activity ends** (backing out of the app): the client stops (`Client::shutdown`) and Tauri ends the process; the
  next start, as after Android kills the process, opens the stores again and sends what the outbox kept.
- **Notifications:** the page bridge's `notify` shows a native notification (tauri-plugin-notification), asking for
  Android 13's permission the first time (not yet seen on the emulator: no page notified in the checks); a click opens
  the app, not the notification's link yet.

## Known gaps (MT1)

- No background flush: the outbox is sent while the process lives (WorkManager is next).
- A notification's link and tag are not used; the counts and presence (`setCounts`, `setPresence`) do nothing.
- Downloads and attachments a server page opens are not saved by the host, and an event stream is answered 503.
- A form posted by navigation loses its body (above); the bridge's `fetch` covers the webroot's requests.
- The device's name in the device list is "MR Moqui on Android", not the phone's model.
- SSO is checked to the hand-back's intent and its refusal of an unknown sign-in; a sign-in through a provider waits
  for a server with one.
- The emulator image's WebView (133) is below the 141 encrypted calls need (MT2).

## Tests

- `cd crates/moqui-mobile && CARGO_TARGET_DIR=../../target cargo test`: the page check, where a navigation goes, the
  sealed key store, the schemes' redirect and `fetch` handling, the build's conf, the bridge's split by origin.
- `cd crates/moqui-client-tauri && CARGO_TARGET_DIR=../../target cargo test`: the schemes' answer, with no network.
- `scripts/check-android.sh`: clippy for both Android targets.
- `scripts/android-smoke.sh`: on the emulator when asked (not in `./moq test-all`): install, start, the shell's ready
  line in logcat, a screenshot.
