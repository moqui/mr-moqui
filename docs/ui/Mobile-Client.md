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
  A sign-out, a switch of account or a removal shows the shell again. A server page never goes to the shell's origin
  by itself (its link there goes nowhere), nor does a navigation while a load is under way; only the shell and the
  host go there (the host's own showing of the shell, until its load begins).
- **A server page is its account's.** The account it was opened for is noted when the host opens it, and the page has
  that account (its requests' key, its sockets, outbox and drafts) only while that account is the current one; none
  while a navigation is under way. A switch, sign-in or sign-out closes the server's pages before the current account
  changes. A socket whose account was read before the page's sockets were closed (a load, a switch) is refused as it
  starts, so none outlives its page.
- **The bundled comms screens** (Chat, Mail, Compose, Calls) are the shell's, as on the desktop; an encrypted room in a
  server page offers Open in Chat, which shows it in the shell (`open_in_shell`).
- **The boundary is the command set.** Tauri counts both schemes' pages as local, so the one capability
  (`capabilities/main.json`) grants the app's commands to both and never to the web. The shell's commands (accounts,
  sign-in, SSO, sign-out, opening a page) check the page themselves (`ShellHost::is_shell_page`): the webview's URL on
  the shell's origin, and the nonce of the shell's current load, which the host hands the shell's document once it has
  loaded (only a document on the shell's origin takes it) and forgets at the next navigation or load, and which each
  shell command carries. A server page has the sockets, the outbox and the drafts of its account, as a desktop app
  window does. The device key never reaches a page.
- **Only the main frame has the IPC.** wry 0.57.1 on Android gave every frame Tauri's initialization scripts (the invoke
  key among them) and its IPC object, so a cross-origin iframe of a server page could have called the app's commands as
  the page. The app patches wry (`crates/moqui-mobile/patches`, through `[patch.crates-io]`, so it survives `cargo tauri
  android init`): the scripts run in the app's two origins only, and the IPC is a web message listener for those
  origins that drops any message not from the main frame. The bridge defines nothing in a frame either. On the desktop
  CEF already runs Tauri's scripts in the main frame only and tags each IPC message with its frame's URL.
  `scripts/android-frames.py` (run by `scripts/android-smoke.sh`) checks it on the device. `./moq android dev` and
  `build` stop when the app's wry is not the patched copy. A WebView with no web message listeners (an old Android
  System WebView) has no IPC at all: the bridge then shows "This app needs a newer Android System WebView".
- **No window without a gesture.** The patch also turns off `javaScriptCanOpenWindowsAutomatically`, which wry turns
  on, so a page opens no window (and so sends the phone to the web or another app) on its own.
- **A server page's storage scope** (MoquiLib `scopedKey`) and the scopes it is to clear are asked of the host as the
  page starts (`/__moqui-app/page`, the host's own path on the app origin), since one init script serves every page.

## Android's WebView and the schemes

- **No request bodies.** The WebView hands an app scheme a request's headers but not its body, so the bridge sends a
  page's `fetch` with a body (a transition's POST, an upload) to the host as a command (`scheme_fetch`), which answers it
  as the scheme would and follows a redirect in the page. Any other request but a GET or HEAD on the schemes (a form
  posted by navigation) is refused, 405, rather than sent on empty. The headers it hands are User-Agent, Accept,
  Referer, the client hints, `Upgrade-Insecure-Requests` on a navigation and `Origin` on a CORS-mode request; no
  `Sec-Fetch-*`, so a document is told by its Accept, as on the desktop without them.
- **No redirect status.** A scheme answer cannot be a 3xx, so a document's redirect is answered as a page that goes to
  its `Location`.
- **The shell's files** are embedded in the app (Tauri's assets of `target/mobile-shell`, which `./moq android` stages
  as the desktop bundle's) and served under the same CSP.

## Keys, store, conf and sign-in

- **Device keys** are sealed by an AES-GCM key of the Android Keystore (`KeyVault.kt`, through JNI), which never leaves
  it, and kept in the app's private files (`client/keys/<account>.sealed`). The Keystore is asked twice; a device that
  still cannot make its key keeps them in a plain owner-only file there, and the shell says so (`keyStore: file`). A
  later run with the Keystore seals any such plain key and removes it. The key is not limited to an unlocked device
  (`setUnlockedDeviceRequired`): the outbox and the coming call path (MT2) need the keys with the screen locked.
- **No backup.** `allowBackup` is off and `data_extraction_rules.xml` excludes every file from cloud backups and device
  transfers: the private files hold each account's store (its mail and chat) and the sealed keys, whose Keystore key
  would not come along anyway.
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
- **In the background**, where Android freezes or ends the process, the outbox is sent by a WorkManager job
  (`FlushWorker.kt`): each time the app goes to the background (`ProcessLifecycleOwner`), a job waits for a network and
  calls Rust (`Native.flush`, `native::flush_background`), which sends what is due for at most a minute
  (`moqui_client::sync::flush::flush_bounded`; one operation taken goes to its end) and leaves the rest as it was. When
  something waits for a later time (a retry's backoff), the job runs again after it, at least a minute later and at most
  12 times. In a process started for it, the client is made from the store as for a push; a 401 holds the operation
  until a sign-in or a renewed key, as in the app. Operations are claimed in the store, so the job and the app's own
  sender never send one twice. The feeds and the mirror do not run in the background.
- **The activity ends** (backing out of the app): the client stops (`Client::shutdown`) and the app ends the process
  (`exit`), so a process Android would keep cached never has a stopped client; the next start, as after Android kills
  the process, runs the setup again, opens the stores and sends what the outbox kept. Only an activity that finished
  does this (`isFinishing` and not `isChangingConfigurations`, `ActivityEnd` read through JNI): the activity handles
  every configuration change itself (`configChanges`: orientation, size, density, font scale and weight, locale,
  layout direction, colour mode, keyboard, navigation, touchscreen, mcc and mnc), and one made again keeps the client.
- **A page's request with a body** (`scheme_fetch`) is made on Tauri's blocking pool: reqwest's blocking client
  panics on an async runtime's thread, so until MT2 every such POST from the shell answered 500.
- **Notifications:** the page bridge's `notify` shows a native notification (tauri-plugin-notification), asking for
  Android 13's permission the first time (not yet seen on the emulator: no page notified in the checks); a click opens
  the app, not the notification's link yet.

## The incoming call (MT2)

A call rings on a locked phone without the WebView: native code shows it, and the WebView carries the media once it is
answered (plans/Mobile-Desktop.md MD6, MT2; a Rust native audio client is later). The Kotlin is in
`gen/android/app/src/main/java/org/moqui/mobile/`; the Rust is `src/calls.rs` and `src/native.rs`.

- **The ring** comes as the server's FCM data message (`kind` `ring`: `callSessionId`, `caller`, `video`, `expiresAt`;
  [Qvt-Client, Native apps](Qvt-Client.md#install-and-web-push)) to `PushService`, which hands it to `CallManager` with
  no WebView, also when the push started the process. `CallManager` adds a self-managed call to Android Telecom
  (`CallConnectionService`, `android.telecom` directly: no new library or coroutines in the Gradle project Tauri made,
  its callbacks map one to one onto the JNI path, and Jetpack Core-Telecom wraps the same `ConnectionService` below
  Android 14), which gives it the audio focus and a headset's answer and hang-up. When Telecom asks for the ring's UI,
  the `phoneCall` foreground service (`CallService`) shows it: a call notification (CallStyle) with Answer and Decline,
  ringing with the ringtone, and a full-screen intent to `IncomingCallActivity`, the ring on the screen over the lock
  screen. Where Android 14+ does not let the app use full-screen intents (`canUseFullScreenIntent`), the ring is a
  heads-up notification. It rings until the server's `ringEnded`, its `expiresAt`, or the user's Answer or Decline.
- **Answer** opens the app over the lock screen (`MainActivity` with `setShowWhenLocked` and `setTurnScreenOn`) with
  the link `org.moqui.mobile:/call?callSessionId=...&video=...`; tao hands it to Rust as `RunEvent::Opened`, and the
  host shows the shell at the call's screen of the current account's comms app with `answer=true` (`client_take_open`,
  or the loaded shell poked), where `m-call` answers and joins through WebrootCall.js. A process that was dead starts
  the activity cold: the shell starts signed in from the store and then joins. The Telecom call and the foreground
  service last while the page has the call: WebrootCall.js tells the host when it joins and leaves
  (`moquiDesktop.callState`, the shell's `call_state` command, then `CallManager.pageCallState` through JNI), and the
  app stops showing over the lock screen when the call is over. The ongoing call's notification has Hang up, the same
  link with `hangUp=true`, which the shell's call leaves. Backing out of the app during a call puts it in the
  background rather than end the process.
- **Decline** is the server's `declineCall` transition, posted from Rust with the device key through the client's
  proxy (`Native.decline`, a JNI entry point, on a thread of the native code's), with no WebView. A ring does not say
  which account it is for, so each account with a key is asked, the current one first, until one's server knows the
  call.
- **One client per process.** A push can start the process with no activity: Rust is then given the application
  context by `Native.init` (not tao's `ndk-context`, which tao sets once) and makes the client from the app's data
  directory; the app's setup takes the same one when the activity starts.
- **Push registration.** Firebase's token goes to Rust (`Native.pushToken`), kept in `client/push.json` with the
  accounts it is registered with, and registered with each account's server (`/qapps/registerClientPush`) once per
  token: when it comes, at the app's start for one not yet registered, and at each sign-in (a new key). It is taken
  back (`unregisterClientPush`) before a sign-out or a removal revokes the key. The server keeps one user per token, so
  of two accounts on one server the last registered gets the rings.
- **Permissions:** `POST_NOTIFICATIONS` (Android 13+) and `RECORD_AUDIO` are asked once an account is signed in, not at
  the first start's sign-in; Android's permission dialog asks for the unlock first, so on a locked phone the Answer does
  not ask and the page asks after the unlock. `CAMERA` is asked at a video call's Answer or by the page.
  `MANAGE_OWN_CALLS`, `FOREGROUND_SERVICE_PHONE_CALL` and `USE_FULL_SCREEN_INTENT` are granted at install (Android 14+
  lets the user, and Google Play's policy, turn the full-screen intent off).
- **Firebase is optional at build time.** With `gen/android/app/google-services.json` the build applies the
  google-services plugin and adds firebase-messaging and `src/fcm` (`PushService`, its manifest); without it the app
  builds and runs with push off (`BuildConfig.FCM`, and `Push.kt` asks for `FcmPush` by name), and no Firebase class is
  in it.
- **Versions.** The app's minimum is Android 8.0 (API 26), where self-managed Telecom calls start; the `phoneCall`
  foreground service type is Android 10+ (earlier it has none); the full-screen intent check is Android 14+. Calls need
  WebRTC in Android System WebView; end-to-end encrypted calls need WebView 141 or later (`RTCRtpScriptTransform`):
  below it the ring screen says so, and the page refuses to join such a call (they fail closed). The emulator image's
  WebView is 133.

**A ring without Firebase (debug builds).** `src/debug`'s `DebugRingReceiver` runs the same handler as FCM's; only the
adb shell may send it (it holds `DUMP`). The tempwhitelist stands for the start of a foreground service that a
high-priority FCM message allows; `-f 32` reaches a force-stopped app, which FCM itself does not until the user starts
it again:

```sh
adb shell cmd deviceidle tempwhitelist -d 60000 org.moqui.mobile
adb shell am broadcast -f 32 -n org.moqui.mobile/.DebugRingReceiver --es kind ring --es callSessionId 100100 \
    --es caller "'Joe Developer'" --es video false --el ringSeconds 40
adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind ringEnded --es callSessionId 100100 --es reason answered
adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind token --es token debug-token-1
```

A real call to ring: another user starts it on the local server (with `rtc_media_port` set, so it has a media node),
for example `curl -u joe.developer:moqui -H 'X-Requested-With: XMLHttpRequest' -X POST
http://localhost:8080/apps/BasicComms/Calls/startCall -d calleeUserIdList=EX_JOHN_DOE -d video=false`, then the ring
above with its `callSessionId`; with the app in the foreground the `/notws` ring reaches the shell too.

**What needs Firebase and a phone.** A real FCM message reaching the app, asleep or after the system ended its
process; the token's registration from Firebase; a ring on a phone's own lock screen and its full-screen-intent
setting; Bluetooth and speaker routing; two-way audio; and encrypted calls on a WebView 141+.

## Known gaps (MT1)

What fills each gap of MT1 and MT2 is MT3's record ([Mobile-Desktop](../../plans/Mobile-Desktop.md#android-track-md6)).

- A notification's link and tag are not used; the counts and presence (`setCounts`, `setPresence`) do nothing.
- Downloads and attachments a server page opens are not saved by the host, and an event stream is answered 503.
- A form posted by navigation is refused with 405 (above), not sent without its body; the bridge's `fetch` covers the
  webroot's requests.
- SSO is checked to the hand-back's intent and its refusal of an unknown sign-in; a sign-in through a provider waits
  for a server with one.
- The emulator image's WebView (133) is below the 141 encrypted calls need: the ring screen says so, and the page
  refuses such a call.
- Telecom's hang-up from a headset or a car ends the native call but not the page's; the user leaves it in the app.

## Tests

- `cd crates/moqui-mobile && CARGO_TARGET_DIR=../../target cargo test`: the page check, where a navigation goes, the
  sealed key store, the schemes' redirect and `fetch` handling, the build's conf, the bridge's split by origin.
- `cd crates/moqui-client-tauri && CARGO_TARGET_DIR=../../target cargo test`: the schemes' answer, with no network.
- `scripts/check-android.sh`: clippy for both Android targets.
- `scripts/android-smoke.sh`: on the emulator when asked (not in `./moq test-all`): install, start, the shell's ready
  line in logcat, a screenshot, the frame check, then back out and start again. With a release APK
  (`MOQUI_MOBILE_APK`), which has no DevTools socket, it skips the frame check and checks instead that R8 kept the
  classes Rust calls by name (`dexdump`) and that the log has no JNI lookup failure. With `MOQUI_SMOKE_APP=/qapps/...` and an
  account signed in, the frame check opens that server page and puts the frames there, and checks that the page cannot
  call a shell command or go to the shell's origin itself.
