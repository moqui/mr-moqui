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
- **The bundled comms screens** (Chat, Mail, Compose, Calls, My devices) are the shell's, as on the desktop; an encrypted
  room in a server page offers Open in Chat, which shows it in the shell (`open_in_shell`). The app's crypto device is
  made when Chat opens; on a new phone an encrypted room says Set up this device until the key backup is restored with
  the recovery key or the phone is verified from another device ([Chat keys](../interfaces/Chat-Keys.md#a-new-device)).
- **The boundary is the command set.** Tauri counts both schemes' pages as local, so the one capability
  (`capabilities/main.json`) grants the app's commands to both and never to the web. The shell's commands (accounts,
  sign-in, SSO, sign-out, opening a page) check the page themselves (`ShellHost::is_shell_page`): the webview's URL on
  the shell's origin, and the nonce of the shell's current load, which the host hands the shell's document once it has
  loaded (only a document on the shell's origin takes it) and forgets at the next navigation or load, and which each
  shell command carries: the phone's own too (the notification check's `push_check`, `push_check_fix` and `push_test`,
  the room the shell shows, `chat_room_shown`, the calls', contacts' and dialer's; SEC-CLI-97). The room the shell
  showed goes with its page (`MobileHost::page_gone`), as its own word of leaving it comes after the navigation took
  the nonce. A server page has the sockets, the outbox and the drafts of its account, as a desktop app
  window does, the terminal's `/termws` with its binary frames among them (in base64 between the page and the host).
  The device key never reaches a page. The Terminal on a phone has a key bar above the keyboard, fits above the
  keyboard as it opens, and takes a pinch for its font size ([Terminal](Terminal.md#the-desktop-and-mobile-apps-and-phones)).
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
- **A page's dialogs and notifications say whose they are.** A page's `alert` and `confirm` are the app's own native
  dialogs, so each has a title naming who asks: the shell's the app's name, a server page's the host of its account's
  server (every account's pages share the one app origin, so the host tells wry's chrome client as it opens them,
  `RustWebChromeClient.pageServer`), a frame of another origin its own host. `prompt` is refused on the app's two
  origins, so a page of one account's server cannot ask in the app's dialog for another account's password. A page's
  `notify` is titled after its server too (`commands.rs` `notification_title`); only the shell's, which carries its
  nonce, shows as the app's (SEC-CLI-119).
- **A camera capture stays in the app.** A page's file input with `capture` (Chat's camera input) has the camera app
  write the photo or video to `captures/` in the app's cache, the FileProvider's one path, never to shared storage or
  the camera app's own place; a cancelled capture's file goes at once, the rest when the next webview starts (wry's
  patch, SEC-CLI-89, SEC-CLI-110).
- **A server page's storage scope** (MoquiLib `scopedKey`) and the scopes it is to clear are asked of the host as the
  page starts (`/__moqui-app/page`, the host's own path on the app origin), since one init script serves every page.

## Android's WebView and the schemes

- **No request bodies.** The WebView hands an app scheme a request's headers but not its body, so the bridge sends a
  page's `fetch` with a body (a transition's POST, an upload) to the host as a command (`scheme_fetch`), which answers it
  as the scheme would and follows a redirect in the page. Any other request but a GET or HEAD on the schemes (a form
  posted by navigation) is refused, 405, rather than sent on empty. The headers it hands are User-Agent, Accept,
  Referer, the client hints, `Upgrade-Insecure-Requests` on a navigation and `Origin` on a CORS-mode request; no
  `Sec-Fetch-*`, so a document is told by its Accept, as on the desktop without them. A `scheme_fetch` on the shell's
  origin is answered only with the nonce of the shell's current load (`scheme.rs` `refuse_fetch`), so a server page
  still running as the host shows the shell gets none of the shell's answers (its offline ones, the chat store's
  passphrase); and where any page's comes from is the host's to say: the caller's `Origin`, `Referer`, `Sec-Fetch-*`
  and `Service-Worker` are dropped, and the host sends the page's own `Origin`, `Sec-Fetch-Dest: empty` and
  `Sec-Fetch-Site: same-origin`, which holds as the IPC takes the main frame only (SEC-CLI-06).
- **No redirect status.** A scheme answer cannot be a 3xx, so a document's redirect is answered as a page that goes to
  its `Location`.
- **The shell's files** are embedded in the app (Tauri's assets of `target/mobile-shell`, which `./moq android` stages
  as the desktop bundle's) and served under the same CSP.

## Keys, store, conf and sign-in

- **Device keys** are sealed by an AES-GCM key of the Android Keystore (`KeyVault.kt`, through JNI), which never leaves
  it, and kept in the app's private files (`client/keys/<account>.sealed`). The Keystore is asked twice; a device that
  still cannot make its key keeps them in a plain owner-only file there, and the shell says so (`keyStore: file`). A
  later run with the Keystore seals any such plain key and removes it. The key is not limited to an unlocked device
  (`setUnlockedDeviceRequired`): the outbox and the coming call path (MT2) need the keys with the screen locked. What
  no background work reads, each account's chat store passphrases, is sealed by a second Keystore key that is
  (`client/keys/<account>.chat-store.unlocked`, Android 9 and later; one sealed by the first key before moves as it is
  read), so they do not open while the phone is locked and are never kept in memory.
- **A screen lock.** The shell's account menu warns when the phone has none (`KeyguardManager.isDeviceSecure()`), and
  a shared-device sign-in is refused there: the next person to pick the phone up would open the account in the app.
- **No backup.** `allowBackup` is off and `data_extraction_rules.xml` excludes every file from cloud backups and device
  transfers: the private files hold each account's store (its mail and chat) and the sealed keys, whose Keystore key
  would not come along anyway.
- **The store** of each account is under the app's data directory (`client/data/`), as on the desktop.
- **`<client-conf>`** is the build's `client-conf.xml`, read at build time; a build for a company's shared phones sets
  `device-mode="shared"`.
- **Sign-in** by password as on the desktop. **SSO** opens the system browser (an Android `VIEW` intent) and comes back
  as `org.moqui.mobile:/sso`, an intent filter of the activity, which the server allows by default
  (`webapp-list/webapp[webroot]@app-redirect-uris`).
- **Requests** are moqui-client's, in Rust, with TLS checked by Android's trust manager (rustls-platform-verifier). They
  do not go through Android's network security config: plain `http://` is refused but to this device
  (`account::base_url`), so the debug app reaches `http://localhost:8080` through `adb reverse tcp:8080 tcp:8080`. Start the server with the loopback HTTP settings in [Build-Test](../Build-Test.md#android-app). When the verifier cannot start the app makes no request, answers from the store, and the
  commands that need the server say so.

## Offline map packages

The phone keeps downloaded map regions as the desktop does ([Desktop-Client](Desktop-Client.md#offline-map-packages):
the store under `client/data/`, `packages-max-mb`, the checks, the shell's `/__moqui-app/maps/` path and the
commands, and the bundled offline Maps screen: [Maps, Offline packages](../tools/Maps.md#offline-packages)); what differs is who
runs the download (plans/Maps-Offline.md §2.4) and the screen's "Download only on Wi-Fi" choice, the phone's alone.

- **`PackageDownloadWorker`** (WorkManager, `PackageDownloadWorker.kt`, a `ListenableWorker` on a thread of its own, so
  one map download runs at a time and WorkManager's threads stay for `FlushWorker` and `RegisterWorker`; a second
  package's download waits until the first's run ends): `packages_download` hands it the account, package and revision
  (`MobileHost::start_package_download`, `PackageDownloadWorker.start` through JNI), one work per account and package, a
  new revision replacing the work of another, and a tap on Download again replacing a work that has not run (its "only
  on Wi-Fi" choice with it). It waits for a network, an unmetered one when the shell asks `wifiOnly` ("only on Wi-Fi"),
  and storage not low, and calls Rust (`Native.packageDownload`, `native::package_download`) with the process's client,
  made from the store when WorkManager started the process. A cut, a server error or WorkManager's stop (the network or
  Wi-Fi gone, its ten minutes up: `onStopped` pauses the run at its next range, `Native.packagePause`, its own run only:
  the work's id is the run's) keeps the part file, and the next run, backing off from 30 seconds, goes on from its
  length. Its 20th run tells Rust it is the last: a failure then (but a pause, which WorkManager runs again), no
  network, or an exception in the worker (`Native.packageGiveUp`) is made final (`retry` false), so the shell does not
  show a download nothing will run again; the part is kept for the next download asked. A cancel, a delete, and an
  account's data deleted cancel its works (`cancelUniqueWork`, `cancelAllWorkByTag`).
- **The bridge:** `moquiDesktop.packages` on the shell's page only (`list`, `download(packageId, revision, wifiOnly)`,
  `cancel`, `remove`, `status`, `addresses`), with the load's nonce.
- **The offline screen** in the one webview: the drawer's Maps opens Maps' Browse (the server's page, in the webview)
  when the server answers and the shell's offline Maps screen when it does not; a place saved there waits in the
  outbox, and `FlushWorker` sends it when the network is back.
- **Android's WebView and ranges:** the patched `RustWebViewClient.kt` hands a 206 body on as it starts at its range,
  and reads an intercepted body's length as an Int32, so no member past 2 GiB less a byte is unpacked (on every device,
  so a package is the same everywhere); the server's `maps@package-max-bytes` (2 GiB) keeps packages under it.

## Back

Android's back (the gesture or the button) asks the page first (`MainActivity.kt`'s back callback, added after
Tauri's own once the webview is made: `moqui.back.pop()`): an open dialog or menu, then a sheet, a details pane or thread, a room opened from the home's list, and
then the shell's own history all close one at a time. When the page closes nothing (the home with nothing open), the
WebView goes back in its history (a server page), and then back leaves the app as before. The room's back arrow says "Back
to chats" and a pane's "Back to the room". The page has 400 ms to answer, after which Tauri's back goes on. Asking the
page loses Android's predictive back animation (the preview of the screen back leads to).

## Lifecycle

- **Paused** (another app, the screen off): the mirror stops; the store, the sockets and the outbox's sender stay while
  Android keeps the process.
- **Resumed:** the outbox is woken, and the mirror reads ahead again for the shell's account.
- **In the background**, where Android freezes or ends the process, the outbox is sent by a WorkManager job
  (`FlushWorker.kt`): each time the activity stops, and when it finishes (kept by WorkManager before the process
  ends, and run a few seconds later in a process of its own), a job waits for a network and calls Rust (`Native.flush`,
  `native::flush_background`), which takes what is due for a minute (`moqui_client::sync::flush::flush_bounded`) and
  leaves the rest as it was. Past the minute it takes no operation and an upload stops between its chunks (the next run
  goes on from where the server is); each request's timeout is the time left and 30 seconds, so a run ends within about
  a minute and a half. When something is still due, or waits for a later time (a retry's backoff), the job runs again,
  at least a minute later and at most 12 times; a new go-to-background run replaces one not yet run, and has its own
  name, so a long wait of a run again never holds it up. In a process started for it, the client is made from the
  store as for a push; a 401 holds the operation until a sign-in or a renewed key, as in the app. Operations are
  claimed in the store, so the job and the app's own sender do not take one together; one cut off by the process's
  end is sent again at the next start, which the server takes once by its `clientRef` (a mail send on a draft found by
  its id rather than its `clientRef` could, rarely, go twice). A process that is ending makes no client for a run. The
  feeds and the mirror do not run in the background.
- **The activity ends** (backing out of the app): the client stops (`Client::shutdown`) and the app ends the process
  (`exit`), so a process Android would keep cached never has a stopped client; the next start, as after Android kills
  the process, runs the setup again, opens the stores and sends what the outbox kept. Only an activity that finished
  does this (`isFinishing` and not `isChangingConfigurations`, `ActivityEnd` read through JNI): the activity handles
  every configuration change itself (`configChanges`: orientation, size, density, font scale and weight, locale,
  layout direction, colour mode, keyboard, navigation, touchscreen, mcc and mnc), and one made again keeps the client.
- **A page's request with a body** (`scheme_fetch`) is made on Tauri's blocking pool: reqwest's blocking client
  panics on an async runtime's thread, so until MT2 every such POST from the shell answered 500.
- **Notifications:** the page bridge's `notify` shows a native notification (tauri-plugin-notification). Its `tag` (a
  room's `chat-<id>`) is the notification's id, so the same tag replaces it and `notifyClear(tag)` removes it; its
  `link` (any path on the account's server) is kept with it, with the account and the app's signature of both (an
  HMAC with the room links' key), and a tap opens the app at that path only when the signature checks and that account
  is the current one (`commands.rs` `tapped_path`). The activity is exported, and the notification plugin takes any
  app's intent with its extras as a tap, so another app's opens no page, only the app (SEC-CLI-88). The chat's own
  notifications, shown with no WebView, are below ([Chat notifications](#chat-notifications)).

## The incoming call (MT2)

A call rings on a locked phone without the WebView: native code shows it, and the WebView carries the media once it is
answered (plans/Mobile-Desktop.md MD6, MT2; a Rust native audio client is later). The Kotlin is in
`gen/android/app/src/main/java/org/moqui/mobile/`; the Rust is `src/calls.rs` and `src/native.rs`.

- **The ring** comes as the server's FCM data message (`kind` `ring`: `callSessionId`, `caller`, `video`, `expiresAt`,
  `ringSeconds`, `pushId`; [Qvt-Client, Native apps](Qvt-Client.md#install-and-web-push)) to `PushService`, which
  hands it to `CallManager` with no WebView, also when the push started the process. It is the account's whose
  registration `pushId` names (the `subscriptionId` its `registerClientPush` answered, kept in `client/push.json`); a
  ring that is no account's here is dropped. `CallManager` adds a self-managed call to Android Telecom
  (`CallConnectionService`, `android.telecom` directly: no new library or coroutines in the Gradle project Tauri made,
  its callbacks map one to one onto the JNI path, and Jetpack Core-Telecom wraps the same `ConnectionService` below
  Android 14), which gives it the audio focus and a headset's answer and hang-up. When Telecom asks for the ring's UI,
  the `phoneCall` foreground service (`CallService`) shows it: a call notification (CallStyle) with Answer and Decline,
  ringing with the ringtone, and a full-screen intent to `IncomingCallActivity`, the ring on the screen over the lock
  screen. Where Android 14+ does not let the app use full-screen intents (`canUseFullScreenIntent`), or the service
  cannot start, the ring is a notification of its own. It rings for `ringSeconds` from its arrival (the server's time
  left, not the phone's clock), until the server's `ringEnded` (one that comes first is remembered for two minutes),
  or the user's Answer or Decline.
- **Answer** opens the app over the lock screen (`MainActivity` with `setShowWhenLocked` and `setTurnScreenOn`) with
  `org.moqui.mobile:/call?callSessionId=...&nonce=...`. The activity is exported, so any app could send a call link:
  the nonce is one Rust made for this Answer (`Native.callLink`), and Rust takes the link once, within two minutes,
  only for that call (`CallLinks`); any other is refused, and a link the recent apps replay is dropped before tao sees
  it. The host makes the ring's account the current one and shows the shell at its call screen; the screen answers and
  joins through WebrootCall.js only when the host hands it the answer, once (`moquiDesktop.takeAnswer`,
  `call_take_answer`), so a link to a call screen on the web or in a page never answers. A process that was dead
  starts the activity cold: the link waits for the host, the shell starts signed in from the store and then joins.
- **A network change in a call.** `MainActivity`'s `NetworkWatch` (a `ConnectivityManager` default-network callback,
  `ACCESS_NETWORK_STATE`) tells the call's page when the phone's network changes during a call (Wi-Fi to mobile data
  and back): `moqui.call.networkChanged()` through the WebView, and the page restarts ICE on the same connection, with no
  new join or ring ([WebRTC, On a bad network](../interfaces/WebRTC.md#on-a-bad-network)). Not yet checked on a device.
- **The call in the app.** WebrootCall.js tells the host what the call does (`moquiDesktop.callState`, the shell's
  `call_state` command, then `CallManager.pageCallState` through JNI): `joined` once its media is up, `left` when it is
  left or could not be joined (an encrypted call this WebView cannot take, offline, not ringing for this account),
  `screen` and `offScreen` as its call screen shows and goes, and `held` when call waiting puts it on hold (the native
  code keeps its call; mapping it to Telecom's hold is the Android part of plans/Comms-Messaging.md phase 4). The Telecom call and the foreground service last while
  the page has the call; one the page has not joined in 30 seconds is given up, and a `ringEnded` (timed out,
  declined, canceled) ends one not yet joined. Over the lock screen the app shows only the call screen of a call
  answered on the ring screen: leaving it (another screen, a server page) shows the lock screen, which asks for the
  unlock, and the app stops showing over it when no such call is left. The ongoing call's Hang up, and a headset's or
  a car's hang-up through Telecom, have the page leave the call. Backing out of the app during a call puts it in the
  background, and the process does not end while a call or a decline is under way.
- **Decline** is the server's `declineCall` transition, posted from Rust with the ring's account's device key through
  the client's proxy (`Native.decline`, a JNI entry point, on a thread of the native code's), with no WebView; only
  that account's server hears of the call, and only its decline counts as told.
- **One client per process.** A push can start the process with no activity: Rust is then given the application
  context by `Native.init` (not tao's `ndk-context`, which tao sets once) and makes the client from the app's data
  directory; the app's setup takes the same one when the activity starts.
- **Push registration.** Firebase's token goes to Rust (`Native.pushToken`), kept in `client/push.json` with each
  account's registration (its `subscriptionId`), and registered with each account's server (`/qapps/registerClientPush`) once per
  token: when it comes, at the app's start for one not yet registered, and at each sign-in (a new key). It is taken
  back (`unregisterClientPush`) before a sign-out or a removal revokes the key. The server keeps one user per token, so
  of two accounts on one server the last registered gets the rings (and the other's registration is forgotten here).
  With the token go the account's own push public key and auth secret (`p256dhKey`, `authSecret`: a P-256 pair and
  secret of each account, `src/push.rs`, kept sealed by the Keystore as the device key is, under
  `<account>.push-keys`), so the server seals what it sends to that account's keys. Two servers can give the same
  `subscriptionId`, so a sealed push is the account's whose keys open it, never the current one by default; an
  unsealed ring (or its end) is taken only when one account can be its and that account registered no push keys, and
  a ring's end ends only that account's call ([Chat notifications](#chat-notifications)). An account whose registration
  another account of its server took (one user per FCM token) is not tried again until something changes. The
  registrations post with no lock held, merged after. One that fails schedules `RegisterWorker` (WorkManager, backing
  off from 30 seconds), which tries until each account's server answers; it is not scheduled at every start, which
  would start the process again after the user backed out. Firebase is asked for a token only once an account is
  signed in; at the last sign-out the token is forgotten here and Firebase deletes it and then its installation
  (`Push.stopFcm`, `FcmPush.stop`), so the phone stays registered with Google for no account, and the next sign-in's
  new token registers (SEC-CLI-121).
- **Permissions:** `POST_NOTIFICATIONS` (Android 13+) and `RECORD_AUDIO` are asked once an account is signed in, not at
  the first start's sign-in, after a dialog that says why: the microphone so a call can be answered on a locked phone,
  and notifications (messages, mentions and calls) when those are missing too; its Not now leaves out the
  notifications, and asks nothing when the dialog named the microphone alone (SEC-CLI-120). Android's permission
  dialog asks for the unlock first, so on a locked phone the Answer does not ask and the page asks after the unlock.
  `CAMERA` is asked at a video call's Answer or by the page. `READ_CONTACTS` is asked at the first contacts
  search ([The phone's dialer and contacts](#the-phones-dialer-and-contacts)). The location is asked at a chat
  location's first use, or before the first live location share with a dialog that says why ([Live location](#live-location)).
  `MANAGE_OWN_CALLS`,
  `FOREGROUND_SERVICE_PHONE_CALL` and `USE_FULL_SCREEN_INTENT` are granted at install; Android 14+ lets the user (and
  Google Play's policy) turn the full-screen intent off, so after the notifications question the app says why calls
  need it and opens its setting, once. The notification check fixes either later.
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
adb shell run-as org.moqui.mobile cat files/../client/push.json   # the account's registration, its pushId
adb shell am broadcast -f 32 -n org.moqui.mobile/.DebugRingReceiver --es kind ring --es callSessionId 100100 \
    --es pushId 100050 --es caller "'Joe Developer'" --es video false --el ringSeconds 40
adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind ringEnded --es callSessionId 100100 --es reason answered
adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind token --es token debug-token-1
```

The Kotlin rules (`CallRules`: the ring's time, the join watchdog, which `ringEnded` ends a call, which link shows
over the lock screen) have JVM unit tests: `./gradlew :app:testUniversalDebugUnitTest` in `gen/android`, with the
environment of `scripts/android-env.sh`.

A real call to ring: another user starts it on the local server (with `media-server@port` set, so it has a media node),
for example `curl -u joe.developer:moqui -H 'X-Requested-With: XMLHttpRequest' -X POST
https://localhost:8443/apps/BasicComms/Calls/startCall -d calleeUserIdList=EX_JOHN_DOE -d video=false`, then the ring
above with its `callSessionId`; with the app in the foreground the `/notws` ring reaches the shell too.

**What needs Firebase and a phone.** A real FCM message reaching the app, asleep or after the system ended its
process; the token's registration from Firebase; a ring on a phone's own lock screen and its full-screen-intent
setting; Bluetooth and speaker routing; two-way audio; and encrypted calls on a WebView 141+.

## During a call (phase 4)

The call's media stays the WebView's; the native code gives it what a phone's own calls have
([plans/Comms-Messaging.md §8.2, §8.3 items 4 to 7](../../plans/Comms-Messaging.md)). The Kotlin is `CallManager`,
`CallConnectionService`, `CallAudio`, `CallService` and `MainActivity`; the rules are `CallRules` (JVM-tested).

- **Every call is in Telecom.** A call rung here is an incoming self-managed call; one the page joins by itself
  (started from a chat, joined, moved to this phone) is placed as an outgoing self-managed call (`placeCall`), so it
  has the audio routes, a headset's hang-up, and the hold of another call. The page says what the call does
  (`call_state`): `joined` (with the call's name: the other people, or the room), `left`, `held`, `screen` and
  `offScreen` (with the call screen's mount token: an older screen's `offScreen` after a newer one's `screen` is
  dropped), and its kind, `video` or `voice` (sent once `joined` is told; one that comes first is kept for it). A car or
  a watch shows the call's name, and a neutral address, not a call id.
- **The page owns its calls.** When the page goes (every new document of the webview, `PageLoadEvent::Started`: a
  server page opened, a reload, the shell's own `location.reload()`, the webview's Back, an account switch, another
  account's ring answered, a renderer gone and loaded again; not a `pushState` or a hash), the calls it joined end
  here, held ones too; a ring, or a call answered on the phone and not joined yet, stays. A `joined` that was on its
  way from the page that went places nothing (the page's end and `call_state` hold one lock, and the native code
  counts the page's loads). Another account's ring answered, or its notification tapped, during a call switches the
  account and so drops the call this page was in without telling its server first; the server sees its leg go with the
  socket. Every way the page loses a call on hold (it ended, moved to another device, or was left) tells the app
  (`left`), and the ongoing notification's Hang up, and its person, are the call the app is in (not the one on hold):
  it ends the native call at once, the page leaving it; one answered on the phone and not joined yet goes with the
  answer the host kept for it.
- **Audio** goes through Telecom (`CallAudio`), each call with its own state, so a second ring does not reset the call
  the app is in, and only that call is routed: `CallEndpoint` on Android 14+, the `CallAudioState` routes before. A
  Bluetooth or wired headset that is there takes the call (also at its start, when Telecom names the earpiece first),
  one that comes takes it at once, and when it goes the call falls back to its kind's route: the earpiece for voice,
  the speaker for video. The call screen's audio button lists the routes (earpiece, speaker, headphones, Bluetooth;
  `call_audio`, `call_audio_route`, each read or asked on the main thread, the command waiting at most two seconds) and
  the user's choice holds for that call, asked again when the call is the app's again after a Swap. The page hears
  each change (`moqui.call.setHostAudio`), and Telecom's mute (a car, a watch) mutes the page (`setHostMute`); its
  first report is the call's state at its start, not a change, so a page that joined muted stays muted. The page's own
  mute is not shown by a car or a watch: a `ConnectionService` cannot set Telecom's mute (no `Connection.setMuted`).
- **The screen off at the ear:** a voice call on the earpiece, with its call screen showing and the app in front,
  holds the proximity wake lock; it follows the app going to the back and coming to the front.
- **Leave the app:** the call keeps going. The `phoneCall` foreground service also takes the `microphone` type once
  the app is in a call, and `camera` for a video call (Android 14 lets a service take them only from the app in front:
  a ring alone keeps `phoneCall`). A video call with the call screen showing (whichever call it shows, a swapped one
  too) goes to picture-in-picture when the user leaves the app (by itself on Android 12+, from `onUserLeaveHint`
  before) or presses Back past the page; the window has Mute and Hang up, the call screen shows only the video
  (`moqui.call.setHostPip`), and the window goes with the last joined call (not when the only call goes on hold). Its
  shape is a fixed 3:4.
- **Hold and call waiting:** the page's hold (Hold and answer, Swap) holds the Telecom call (`held`) and `joined`
  takes it off; Telecom's own hold (another call, a watch, a car) reaches the page (`moqui.call.hostHold`), which holds
  the call unless another is on hold already (the page holds one call at a time), so while one is held the call in use
  has no `CAPABILITY_HOLD` and Telecom does not count on a hold (it has it again after); a hold already on its way
  answers once, and one the server never answers gives up after 15 seconds. A second ring rings through Telecom.
  Answering it on the phone does not make it active while the first is: the page puts the first on hold (Hold and
  answer), and the second is active for Telecom as soon as the first is held, before the page's `joined`.
- **Move to this device** is the page's (`move#Call`): a call moved to the phone is joined there and placed in Telecom
  like any joined one, and one moved away from it is left, which ends its Telecom call.
- **The Telecom library.** These use `android.telecom` directly, and so does the calling account below:
  `androidx.core.telecom` has no managed account, and moving the self-managed calls alone is later work
  ([plans/Comms-Messaging.md §9.1](../../plans/Comms-Messaging.md)).

## Sharing the screen in a call

Android's WebView has no `getDisplayMedia`, so in the app **Share the screen** shares the phone's screen natively, as a
**screen leg**: a second leg of the participant (`CltScreen`, `add#ScreenLeg`) whose only track is the screen, sent by
the app's Rust rather than the WebView ([WebRTC, Presenting in a call](../interfaces/WebRTC.md#presenting-in-a-call);
plans/archive/Comms-Whiteboard.md §3.6, the spike's numbers in plans/archive/MM-Spike.md).

- **Capture.** The system's consent each time (`ScreenConsentActivity`, Android 14 refuses a projection kept from an
  earlier one), then a foreground service of type `mediaProjection` (`ScreenShareService`, before the projection is
  taken) with an ongoing notification whose **Stop sharing** ends the share. The projection draws into a
  VirtualDisplay on a MediaCodec VP8 encoder's input surface: the phone's hardware VP8 encoder where it has one, else
  the platform's `c2.android.vp8.encoder`. The capture keeps the screen's aspect with a long side of at most 1280
  (`ScreenRules`), at most 15 frames a second (`KEY_MAX_FPS_TO_ENCODER`), and repeats its last frame after 250 ms of a
  still screen so a keyframe asked for comes then. A rotation or a resize (`onCapturedContentResize`, the display's
  change before Android 14) starts the encoder again at the new size. The app's own `FLAG_SECURE` screens (an
  encrypted room, a view-once photo; [Screen security](#screen-security)) are black in the capture, Android's rule.
- **Send.** Each encoded frame goes over JNI (`Native.screenFrame`) to the leg's thread in Rust (`src/screen.rs`), a
  str0m client of the media server (the workspace's str0m, on the aws-lc-rs the app already links): one send-only VP8
  video, its candidates from the server's answer (a UDP socket for each local address that reaches one, IPv4 and IPv6,
  and ICE-TCP when the server offers it). The media server's keyframe requests ask the encoder for one
  (`PARAMETER_KEY_REQUEST_SYNC_FRAME`), and the TWCC estimate sets its bitrate (85 % of it, 150 kbps to 2.5 Mbps,
  within what the encoder takes).
- **Signalling through the page.** The page holds the call and its socket, so it signals for the leg: it makes the leg
  for its own leg, gets the app's offer (`screen_offer`), joins the leg on its socket, hands the server's answer and
  offers to the app (`screen_answer`, `screen_remote_offer`), asks for the stage with the leg, and publishes its video
  as the screen. No credential leaves the page, and the server checks the leg is the socket user's. The leg's frames
  never reach the page's own connection. The commands are the shell's (`moquiDesktop.screenStart` and the rest of
  `src/bridge.js`).
- **The end.** Stop (in the call or the notification), a take over or a host's Stop, the page leaving the call, its
  hold, or its socket gone end the share: the page sends the leg's `leave` and stops the app's capture, and the server
  ends a screen leg with its page leg in any case (the WebView is paused while the app is in the background, so the
  page cannot be relied on then). The app's leg ends by itself when its connection is lost for 10 s, and tells the page
  (`moqui.call.nativeScreen`).
- **An encrypted call.** In an end-to-end encrypted call the screen leg is the page device's, with a key index of its
  own (`add#ScreenLeg` with `sframe`), and its frames are SFrame-sealed in Rust exactly as the pages' worker seals
  theirs: the VP8 frame's first 10 bytes (a keyframe) or 3 in the clear as authenticated metadata, then the SFrame
  header, ciphertext and tag (AES_128_GCM_SHA256_128, the `sframe` crate). Its keys are the page's to make
  (`moqui.callKeys.addSender`): each rotation makes one for the screen leg too, sends it to the others' devices over Olm
  as its own, and then hands it to the app (`screen_key`, the first at once, a later one a second on). The leg sends
  nothing before its first key, and the page's own device is never sent a key of its own legs. An app without the
  command (an older build) is refused and the page says so.
- **Not yet.** There is no annotation over another app (Android would need a draw-over permission). A direct call
  moves to the media server before the share starts, as a browser's share does.
- **Tests.** The leg against the repository's media server on loopback, over UDP and ICE-TCP (`screen_tests.rs`, with
  `moqui-rtc` as a dev-dependency); the size and bitrate rules (`ScreenRulesTest`); on the emulator, consent for
  automation is `adb shell appops set org.moqui.mobile PROJECT_MEDIA allow` (and `appops reset` after).

## The phone's dialer and contacts

[plans/Comms-Messaging.md §9](../../plans/Comms-Messaging.md), phase 9. The Kotlin is `DialAccount` and `PhoneBook`;
the Rust is `src/dial.rs` and the commands `call_dial_account`, `call_dial_choice`, `call_dial_from`, `contacts_search`
and `contact_names`.

- **A calling account.** The app registers a managed `tel` `PhoneAccount` (`CAPABILITY_CALL_PROVIDER`, on
  `CallConnectionService`) while an account signed in here has a number to call from: the shell asks its Dialer's data
  as it loads for an account (`Root.qvue` `tellDialAccount`) and tells the host (`call_dial_account`), which keeps the
  accounts that can call (`client/dial.json`) and takes the account away when none can (a sign-out too). Android lists
  it under Calling accounts, where only the user turns it on; the notification check has a row for it, which opens that
  screen. Telecom tells the app whether it is on only with `READ_PHONE_NUMBERS`, which the app does not ask, so the row
  (read again at each open of the check) says it is on once a number came through it, and not known again after a
  number Telecom did not let through or a new registration. An account whose key was revoked elsewhere leaves the
  list at the next start.
- **A number dialed with it** (the phone's dialer, a contact's Call) reaches `CallConnectionService` as an outgoing
  call of that account: `DialAccount.dialed` hands the number to Rust (`Native.dialLink`, a nonce for it, two minutes,
  once), cancels Telecom's own call, and opens the app with `org.moqui.mobile:/dial?nonce=...`; when Android does not
  let the app come to the front by itself a notification hands it over on a tap. The link carries no number, and one the
  app did not make is refused; but any app may place a call with the account (`placeCall`, `ACTION_CALL`) and so
  choose the number, so nothing is called by itself (the decision is for cost and fraud safety). The host shows the
  Dialer with the number filled in, and the user presses Call, once: through `place#PhoneCall`, with the caller ID, the
  fraud limits and the emergency path. The Dialer is the current account's when it can call (or none can: it says
  why), the one account that can otherwise (made current); when several can and the current one cannot, the Dialer
  asks which (`call_dial_choice`, the user's pick `call_dial_from`, for two minutes and for the account it was shown
  on), and the app never switches accounts for a number on its own. A leading `*67` or `#31#` is Hide my number: the
  Dialer takes it off and sets Hide my number. The call then is the page's, as any call the page joins. Numbers
  Android treats as emergency numbers never come here: Telecom places them on the SIM.
- **Contacts** (`READ_CONTACTS`), read on the phone only, for the shell's page; nothing goes to the server. A phone
  room's numbers are named from them first (`contact_names`, `moqui.chat.nameRooms`), then from the server's
  `call-context` (`find#PhoneNames`); the new-chat pickers find them as the user types (`contacts_search`,
  `moqui.chat.contactOptions`, at least two characters, ten at most). The server is asked for every number of the
  rooms, not only those the phone does not know, so it cannot tell which are in the phone's contacts. The first search
  asks, after a dialog that says why; the names never ask (a room list opened is not a use), and the app does not ask
  again by itself: the notification check's Contacts row asks on a tap, or opens the app's settings once Android no
  longer asks. Contacts allowed later (the question, or Android's settings while the app was away) name the rooms again
  with no reload (`moqui.chat.contactsChanged`, told by `PhoneBook.check`). The phone's contacts name a person's rooms
  only; they group nothing (people across networks, [Comms](../tools/Comms.md#people-across-networks)). In a person's
  room the composer's network button opens their ways as a sheet from the bottom, and the People tab's card fills the
  details pane, its Add a full-screen dialog.

## Chat notifications

The chat's pushes are shown by Kotlin with no WebView, also when the push started the process
([plans/Comms-Messaging.md §7.4](../../plans/Comms-Messaging.md)). The Kotlin is `Push`, `ChatNotifications`,
`ChatRules`, `ChatActionReceiver`, `PushCheck`, `RegisterWorker` and, for UnifiedPush, `UnifiedPushes`,
`UnifiedPushService` and `RustKeyManager`; the Rust is `src/push.rs` and `src/native.rs`.

- **Sealed pushes, each account its own keys.** A device that registered its push keys gets only `kind`, `pushId` and
  `enc` in each FCM data message: every other field, a ring's too, is sealed to the account's keys (RFC 8291
  aes128gcm). `Push.onMessage` hands `enc` to Rust (`Native.openPush`, on a thread of its own that does no network
  work), which tries each account registered with that `pushId` and takes the one whose keys open it: two servers'
  ids may be the same, and a server cannot make a push that opens as another account's. An opened push keeps its outer
  `kind` (a sealed `notification` has it only outside). A push that is no account's here, or does not open, is dropped
  (logged); only a push with no `enc` at all (a device registered before it had keys) shows a bare "New message". A
  ring that does not open shows a missed-call notification rather than nothing, when its push id names an account here.
  Opening only reads keys: keys a
  server encrypts to are never replaced because the key store failed for a moment.
- **UnifiedPush** (a phone without Google Play; `org.unifiedpush.android:connector`): each account is an instance of
  its own with the distributor (ntfy, for one), whose endpoint Rust registers with that account's server
  (`provider=unifiedpush`) with the account's same keys. The instance is 32 random hex digits kept in `push.json`
  (`Native.instanceOf`, mapped back by `Native.accountOf`) and made again at a sign-in after a sign-out, never the
  account id, which would tell the distributor app the server's host and a hash to test usernames against. An earlier
  build's instances (the account ids) are taken back at the next `registerAll`; the account keeps its endpoint until
  the new instance's comes, which the server's registration then puts in place of the old one. The connector's
  receiver opens a message with its own default key manager, which has none of the account's keys, so the message
  comes still sealed and `UnifiedPushService` opens it in Rust (`Native.openFor`) with the account's keys; one the
  connector says it opened (sealed to its own keys) or that does not open is dropped. A build without Firebase takes
  the phone's default distributor by itself. Where Firebase works it stays: the notification check offers "Use
  UnifiedPush" only after asking, and "Use Firebase" to come back. The instances follow the accounts at each sign-in
  and sign-out (an account signing out has its instance taken back and no keys made again). The server sends no VAPID
  key to the connector (a distributor that requires one refuses: `VAPID_REQUIRED`, logged).
- **Channels:** Messages (and the test notification), Mentions, Texts (an SMS or other bridged room's), Missed calls
  and Other, beside the calls' ringing and ongoing ones; the user sets each one's sound and importance in Android's
  settings.
- **One notification per room of an account** (tag `chat-<account>/<room>`: another server's room of the same id is
  another), a MessagingStyle with the sender and the room, its count when more came, grouped under a summary per
  account, with a conversation shortcut (Android's conversation section). An encrypted room's push has no text ("open
  the app to read it"), and previews off on the server leave it out too. The room the app shows in front (the page
  tells: `chat_room_shown`) shows none.
- **Actions.** Reply (inline) and Mark read go to the account's outbox in Rust (`chat-send`, `chat-read`) and
  `FlushWorker` sends them, so they wait for the network; a reply that cannot be queued (the account is gone) shows
  "not sent" with its text and a tap to the room. An encrypted room's Reply opens the room, since the page encrypts.
  Mute 1 hour mutes the room on this phone. The account, room and message are the action intent's data, which the
  Reply's mutable intent cannot have filled in. A tap opens the room at the message
  (`org.moqui.mobile:/room?account=...&chatRoomId=...&eventSeq=...&sig=...`): the activity is exported, so the link is
  signed by Rust (`Native.roomLink`, an HMAC with the app's own link key) and another app's opens nothing.
- **A room's invite link** (plans/Comms-Messaging.md §18.3 phase 17). A link or QR code opens the room's join page in
  the phone's browser; on Android the page's "Open in the MR Moqui app" is `org.moqui.mobile:/join?host=<host>&token=<token>`
  (a `BROWSABLE` intent filter, `src/join.rs`). A web page makes it, so it is not signed, and it only opens a page: the
  app opens `/join/<token>` in the current account when it is on that host, where the person presses Join. It never
  switches the account (`join_action`, SEC-CLI-99): with another account here on that host the page shown says the
  invite is for it, to switch from the account menu and open the invite again; with none on that host it says so and
  adds none. Android App Links are not used: each server has its own host.
- **Clear and ack.** The server's `clear` (the room was read on another device) removes the room's notification, and
  the summary only when no other room's shows; a read in the app removes it too, Mark all read every room's of the
  account (the page's `notifyClear` reaches
  `ChatNotifications.clearRoom`, as the server sends no clear to the device that read). After a push is shown the app
  tells the server (`/qapps/ackClientPush` with its `ackId`), which the server's push health counts.
- **A sign-out** (each one, one the server did not hear included: no network, a key the server no longer takes) takes
  the account off the phone with no request (`MobileHost::account_signed_out`): its room notifications and their
  summary, its other ones (a test, a reminder: tagged `other-<account>/<tag>`), its rooms' conversation shortcuts
  (`chat-<account>-<room>`, long-lived, so Android keeps them past the dynamic list), a room of it shown
  (`ChatNotifications.forgetAccount`, SEC-CLI-100), its number for the phone's dialer, and its push registration,
  keys and UnifiedPush instance here. The server's take-back of the registration comes before it, when the server can
  be reached. A page's own notifications (`notify`, the notification plugin's) stay until tapped or dismissed.
- **The notification check** (the shell's account menu, "Notifications on this device"; host commands `push_check`,
  `push_check_fix`, `push_test`): the permission, each channel on or off, battery optimization (Android's own; a
  maker's task killer is not detected), the full-screen intent, the push service (Firebase, or the UnifiedPush
  distributor, with a Fix to pick one), whether the account's server has this device's registration, the last push
  received, a stop since, the calling account for the phone's dialer and the contacts; each item that is off has a Fix that opens its system setting (battery optimization
  asks in place). "Send a test notification" asks the server for one (`/qapps/testClientPush`, `send#TestPush`, once a
  device in 10 seconds) and shows the time its own push took to arrive (its ack id among the recent ones).
- **Reminders** (plans/Comms-Messaging.md phase 15). A message reminder comes as a `notification` push (topic
  `ChatReminder`, its own tag `chat-reminder-<id>`): this app shows its title ("Reminder: Ann in Team") in the Other
  channel and a tap opens the app, not the message. Its body (the message's start, in a plain room) and the link to the
  message wait for the app's next change.
- **Send later.** A long press on Send opens Send later, with a short vibration. In an encrypted room the app holds the
  message and sends it at its time only while its WebView runs; closed, it sends it when it next opens Chat (the
  message is then marked sent late for the sender).
- **Stopped.** A force-stopped app gets no push until it is opened again; the app says once, at the next open, that it
  was stopped (Android 11+, `ApplicationExitInfo` `REASON_USER_REQUESTED`, which some makers also give for a swipe from
  the recent apps, so the notice says "stopped", not "force-stopped").

A chat push without Firebase (debug builds), an opened payload as Rust gives it:

```sh
adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind debugChat --es payload \
    "'{\"kind\":\"message\",\"account\":\"chat.example.com-0123456789ab\",\"chatRoomId\":\"100\",\"eventSeq\":5,\"roomName\":\"Team\",\"senderName\":\"Ann\",\"snippet\":\"Hi\"}'"
```

## Cards and buttons

A message's suggestion buttons (a card's, or a text's chips, plans/Comms-Messaging.md §18.3 phase 13) are one row under
the bubble that scrolls sideways on a phone, and a card is at most 85 % of the screen's width; a carousel scrolls
sideways with snap on every width. A call button opens the dialer (`tel:`). The rest is Chat's
([Comms](../tools/Comms.md#cards)).

## Add to calendar

A chat event's Add to calendar (plans/Comms-Messaging.md §18.3 phase 19), with no app that keeps calendars, is
`moquiDesktop.calendarInsert({title, start, end, allDay, timeZone, place, description})` (the `calendar_insert` command,
`CalendarIntent.kt`): the phone's calendar app opens on a new event filled in (`ACTION_INSERT`), and the person saves it
there, so the app needs no calendar permission. It answers whether a calendar app took it; when none does (or in an APK
before it), Chat downloads the event's `.ics` file instead.

## Location and maps

A chat location's Use my current location (plans/Comms-Messaging.md §18.3 phase 20) is the WebView's
`navigator.geolocation`: the manifest has `ACCESS_COARSE_LOCATION` and `ACCESS_FINE_LOCATION`, and the patched wry
`RustWebChromeClient` asks for them at the first use (refused, Chat says how to allow it and offers to enter the place).
Open in maps hands a `geo:` URI to Android (`moquiDesktop.openExternal`, which takes `geo:` beside http, https, mailto,
tel and sip), so the person's maps app opens at the place. The app reads the position in the background only for a live
location share the person started, under its notification (below).

### Live location

A live location share started in Maps, a chat room or a record's page (Share my location with the customer) is sent from
the phone by the app, also while it is in the background (plans/Location-Sharing.md §3.4). Who sees it, its reach,
retention and precision are the guide [Maps, Live location](../tools/Maps.md#live-location); this is the app's part. The
Kotlin is `LocationShare.kt` (the object and `LocationShareService`), `LocationStops.kt` (the stops kept until the
server confirms them) and `LocationRules.kt` (JVM-tested); the Rust is `src/location.rs` with `moqui_client::location`.
`location_allow` and `location_share_start` run on Tauri's blocking pool: the share's read is reqwest's blocking client,
which panics on a runtime worker, and the person's answer is waited for there. The emulator walk
`scripts/android-location-share.sh` drives it ([Tests](#tests)).

- **Started by the person's Send in the app's own dialog.** A server page has `moquiDesktop.location` (`allow`, `start`,
  `stop`, `status`; the host commands `location_allow`, `location_share_start`, `location_share_stop`,
  `location_share_status`, each for the page's account while it is the current one). Browse's Share asks `allow` first:
  without the location permission the app shows a dialog that says why (to send where you are to the people you chose,
  until the time you chose, with a notification and Stop while it does), then Android's question: for an exact share
  precise and approximate (the person may give either; given approximate only it sends the phone's approximate fixes;
  precise is asked again only after a Not now), for an approximate share approximate alone, so where precise was never
  given the phone's own fixes are approximate (about 3 km² on Android 12+) before the server rounds them. Not now, or
  the dialog dismissed, answers no and no share is started. Once the share is made, `start` names it (its id alone): the
  host reads it from the server with the account's device key (`/qapps/clientLocationShare`, `get#AppShare`: whom with,
  until when, exact or approximate, its interval, its audience version, and that this device sends it), and the app asks
  in its own dialog: "Send your location from this phone to Sam Smith (sam) and Family (4 members) until Fri 09:00? It
  goes on while the app is in the background, with a notification and Stop.", Send or Not now. The words are the
  server's: each person by name and unique username, a group with the members it reaches, "anyone with the guest link",
  and the end with its weekday when it is not today. The dialog refuses touches through another app's window
  (`ObscuredTouches`, and Send's own check of `FLAG_WINDOW_IS_PARTIALLY_OBSCURED`), hides other apps' overlays while it
  shows (Android 12+, `HIDE_OVERLAY_WINDOWS`), keeps Send disabled for a second after it has the window's focus, marks
  its text sensitive for accessibility services (Android 14+), and goes with no answer after a minute. **That native
  dialog is the control**, and it guards two things: the start of sending from this phone, and the audience it sends to.
  A server page's script can reach the app's IPC and ask for a start, but nothing is sent from the phone in the
  background without the person's Send there. The share has an audience version (`LocationShare.audienceVersion`) that
  every change of its audience that can reach someone new raises (an entry added, a group added again, whose snapshot is
  written again; a revoke does not); the phone sends the version the person confirmed with each report, and the server
  refuses a report of another version, or of none from a share sent from a device, with 409 `AUDIENCE_CHANGED` before it
  writes or pushes anything. On that 409 the phone pauses the share, and a notification asks the person to open the app
  (at once when it is in front), where the same dialog asks again with the server's words (Send sends it to that
  version; Not now stops the share on the server; a share not read for want of a network stays paused for the next
  time). So no position the phone sends reaches an audience the person did not confirm, and a person added sees only
  what the server received after the audience reached them ([the reach rule](../tools/Maps.md#who-sees-what)). It does
  not guard what the server does with the reports it gets, nor the shares a browser or another device sends. Nor does it
  guard a page in front: with the location permission given, a server page the app shows gets the WebView's geolocation
  with no prompt (`RustWebChromeClient` answers for the app's own origins), as any browser page with the permission
  does, so a page in front can read the position and send it itself while it is open (the `unavailable` path's own
  sending; its reports carry the version the page read). Send starts `LocationShareService`, a foreground service of
  type `location` (`FOREGROUND_SERVICE_LOCATION`), only while the app is in front (`LocationShare.foreground`), with the
  permission and notifications allowed (`LocationRules.mayStart`); `start` answers `started`, `declined` (Not now: a
  share just made is stopped, and the page sends nothing) or `unavailable` (the page sends while it is open, as a
  browser does). So the app has no `ACCESS_BACKGROUND_LOCATION`: Android lets a foreground service started from the app
  in front keep reading the location once the app goes to the background. The service is not sticky: a process Android
  ended does not start it again, and the share goes stale until its end or a Stop.
- **The notification** (channel Location sharing, low importance, ongoing) lasts as long as the service sends: "Sharing
  your location with Sam and 2 others until 15:30" (several shares: the first's audience and the latest end), with
  Stop and a tap that opens the app. One the person dismisses (Android 14+ lets them) is posted again by its delete
  intent, and the half-minute check posts it again too; with notifications turned off (Android 13+ lets the person
  during a share) the check stops every share, on its server too, since none is sent without the notification showing.
- **Stop** (the notification's, notifications turned off, or the permission gone while the process lives on) is
  handled before anything else the service does: each share is no longer sent, and its stop on the server
  (`/qapps/stopClientLocation`, so the audience is told at once) is kept in the app's preferences with the share's end,
  tried at once on the location thread and then by `LocationStopWorker` (WorkManager, with a network, backing off from
  30 seconds, also after the process ended) until the server says it stopped it or has no such share, or the share's end
  has passed (`LocationRules.stopOutcome`); a "Stopping location sharing" notification shows while one waits. An answer
  that is not the transition's JSON (a sign-in page) is a signed-out account, as for a report; a stop let go so before
  the share's end leaves a notice ("your server could not be told; the share shows your last position until 15:30").
  A sign-out posts the account's shares' stops, and those still waiting, while its key is still held
  (`MobileHost::account_signing_out`), when the app can reach the server; and the server stops every active share of a
  device key it revokes (`stop#DeviceShares`), so a sign-out with no network stops them when its pending revoke reaches
  the server (SEC-CLI-108).
- **The fixes.** Android's `LocationManager`, no Google Play services: the platform's `fused` provider (Android 12+),
  else GPS for an exact share and the network's for an approximate one, else whichever is on, never GPS without the
  precise permission (a provider the permission does not reach waits, as with none); on Android 12+ a
  `LocationRequest` of high accuracy while an exact share is sent, else balanced. The interval is the shortest of the
  shares': 5 seconds while the phone moves, 30 while it is still (a speed of 0.8 m/s, or 25 m from the last fix), never
  under a share's own interval (`location@min-interval-seconds`), and an approximate share's own
  (`location@approximate-interval-seconds`). Each fix goes to Rust on the location service's own thread (never the
  native code's, which pushes, a call's hang-up and chat replies use), at most one report a share under way and one
  fix waiting behind it (`LocationRules.queueFix`): `Native.reportLocation`, the host command `report_location`. A
  `Reporter` a share keeps only the newest point not yet sent and posts it to `/qapps/reportClientLocation` through the
  client's proxy with the account's device key, so the share's device binding (`LocationShare.userLoginKeyId`) takes
  it, within 15 seconds (`moqui_client::location::POST_TIMEOUT`, not the proxy's ten minutes for a page's request: a
  half-open connection gives up and the point waits). A 429 waits `retryAfterSeconds`, a stale point or another 400 is
  passed over, `CLOCK_AHEAD` and `BEFORE_SHARE` send from then on with the server's time, a 408 or a 429 that is not the
  transition's (a proxy, the server's tarpit) waits its `Retry-After` (else the interval, or 30 seconds), no answer
  tries again after the interval; a point that waits is sent when its wait is over. A report on its way as the account's
  key is renewed is taken by the server from the key the renewal replaced, for that key's few minutes' grace
  (`LocationShare.renewedFromKeyId`); an `OTHER_DEVICE` that came while the key changed is tried again once with the new
  key too.
- **It stops** a share at its end, on a 410 or 404, when another device sends it (409 `OTHER_DEVICE`: a Send from here
  in a browser or another phone), at the account's sign-out or removal (`MobileHost::account_signing_out` and
  `account_signed_out`, `LocationShare.forgetAccount`), on Stop (the notification's or the page's), with notifications
  off, and when the location permission is taken away: Android ends the app's process then, so sending stops and the
  share goes stale (its audience sees the age) until its end or a Stop. With no share left the service and its
  notification end. Each share the service no longer sends goes from Rust's reporters too (`Native.forgetLocation`),
  and the page's `status` is the service's own list, so the page never says "sent from this phone" while nothing is.
  Send from this device on a share another device sends takes it over (`sendFromHere`) and starts the service. The app
  bar's chip says "sending from this phone" while the service sends, also after Browse closes.
- **Never logged.** A position goes to no log (Kotlin's `Log`, Rust's), crash report or telemetry: the logs name a share
  and an outcome only.
- **The notification check** lists the location permission (precise, approximate only, or not allowed, with a Fix that
  opens the app's settings) and, while the phone sends any, how many shares it sends.

## Screen security

`moquiDesktop.secureScreen(reason, on)` (the `secure_screen` command, `ScreenSecurity.kt`) turns Android's `FLAG_SECURE`
on for a reason, and off once no reason holds it: no screenshot, no screen recording and a blank recent-apps thumbnail.
Chat's view-once viewer holds `viewOnce` while it is open (plans/Comms-Messaging.md §18.3 phase 16); the reasons are a
set, so a screen-security setting of the app or of a room can hold its own. Phase 21's are `room:<chatRoomId>`, held by
Chat while an encrypted room is shown with Settings, Chat, Privacy on "in encrypted rooms" or the room's own Block
screenshots here, and `app:<account>` (the page's storage scope), from "everywhere in the app": WebrootChat.js applies it
when the shell loads and Chat opens, and the `CHAT_SCREEN_SECURITY` preference applier on each change, on and off at
once. Each account holds its own: any account set to everywhere keeps the flag on, and turning it off in that account
takes away only that account's reason. The `app:` reasons are remembered on the phone (`ScreenSecurity.kt`, its
SharedPreferences, one forgotten when it is turned off), so from the next start the flag is on before the first frame,
the splash and the account list included.

### One-time codes

A chat push of a text with a one-time code carries `otp {code}` (plans/Comms-Messaging.md §18.3 phase 21, §7.2 rule
10). `ChatNotifications.showRoom` then adds Copy code: a `COPY` broadcast to `ChatActionReceiver` with the code in the
action's data, which puts it on the clipboard (`ClipDescription.EXTRA_IS_SENSITIVE` on Android 13 and later, a "Code
copied" toast before it). The notification is `VISIBILITY_SECRET`, so a secure lock screen shows nothing of it and Copy
code needs the phone unlocked (a private one with a public version showed the code under Android's default lock screen
setting, SEC-CLI-115). An APK before this change shows the message without the button. It answers whether the flag is on; the
desktop app's (`bridge.rs` `secure_screen`) keeps the window out of screenshots and screen recording on macOS and
Windows, and a Linux desktop has no such member, where the viewer says a screenshot cannot be stopped (SEC-CLI-117). The app's store deletes
disappearing messages at their time as the desktop's does ([Desktop-Client](Desktop-Client.md)).

## Known gaps (MT1)

What fills each gap of MT1 and MT2 is MT3's record ([Mobile-Desktop](../../plans/Mobile-Desktop.md#android-track-md6)).

- The counts and presence (`setCounts`, `setPresence`) do nothing.
- A message reminder shows its title only and opens the app, not the message (above, Reminders).
- UnifiedPush is not checked with a distributor: the emulator has none, and its opening (the account's keys) is the
  Rust tests'. A real distributor (ntfy) delivering to a phone is not tried yet. A chat push has no sender avatar (a
  picture would need a fetch the push path does not make).
- Downloads and attachments a server page opens are not saved by the host, and an event stream is answered 503.
- A form posted by navigation is refused with 405 (above), not sent without its body; the bridge's `fetch` covers the
  webroot's requests.
- SSO is checked to the hand-back's intent and its refusal of an unknown sign-in; a sign-in through a provider waits
  for a server with one.
- The emulator image's WebView (133) is below the 141 encrypted calls need: the ring screen says so, and the page
  refuses such a call.

## Tests

- `cd crates/moqui-mobile && cargo test`: the page check, where a navigation goes, the
  sealed key store, the schemes' redirect and `fetch` handling, the build's conf, the bridge's split by origin, and live
  location's ask from the page and its reports through a stand-in poster (`location_tests.rs`).
- `cd crates/moqui-client-tauri && cargo test`: the schemes' answer, with no network.
- `scripts/check-android.sh`: clippy for both Android targets.
- `./gradlew :app:testUniversalDebugUnitTest` in `gen/android` (`ANDROID_HOME` set): the Kotlin rules' JVM tests
  (`PackageDownloadRules`: the map download's constraints and its resume; `CallRules`: the ring's time, the audio route,
  the screen at the ear, picture-in-picture, the service's types, the calling account's state, a contact's number, the
  contacts' question once; `ChatRules`: channels, tags, links, mute; `LocationRules`: live location's interval moving,
  still and approximate, its stop rules, its start only from the app in front, the provider and the notification's
  words).
- `cargo test -p moqui-client --lib location`: `report_location`'s reporter (the newest point only, 429, a stale point,
  the device's clock, a 410, 404, another device or a sign-out stopping it).
- Instrumented, on the emulator when asked: `CallTelecomTest` (a joined call in Telecom, held and resumed by the page,
  its audio on the earpiece for voice and the speaker for video, the page's choice holding, left); `DialAccountTest`
  (the calling account registered and taken away, a dialed number handed to the app by a nonce only with Telecom's call
  cancelled, a number the app does not call refused, a contact named and found); `ChatNotificationsTest`
  shows an opened push and checks its channel and
  actions, another account's room as another notification, a mention's channel, an encrypted room's lack of text, Reply
  and Mark read through a stand-in outbox (ids from the intent's data, not its extras), a
  reply that could not be queued, `clear`, Mute, the room in view, a push or ring that does not open, and an account's
  sign-out taking its notifications and shortcuts off and leaving another's. Build the
  test APK without the Rust tasks (the app's `.so` from
  `./moq android build --target x86_64 --apk` stays), install both, run it:

  ```sh
  ./gradlew :app:assembleUniversalDebug :app:assembleUniversalDebugAndroidTest -x rustBuildArm64Debug \
      -x rustBuildX86_64Debug -x rustBuildArmDebug -x rustBuildX86Debug -x rustBuildUniversalDebug
  adb install -r -g app/build/outputs/apk/universal/debug/app-universal-debug.apk
  adb install -r -g app/build/outputs/apk/androidTest/universal/debug/app-universal-debug-androidTest.apk
  adb shell am instrument -w org.moqui.mobile.test/androidx.test.runner.AndroidJUnitRunner   # all; -e class one
  ```
- `scripts/android-smoke.sh`: on the emulator when asked (not in `./moq test-all`): install, start, the shell's ready
  line in logcat, a screenshot, a notification tap another app makes up (`am start` with the plugin's extras) refused
  with no page opened, the frame check, then back out and start again. With a release APK
  (`MOQUI_MOBILE_APK`), which has no DevTools socket, it skips the frame check and checks instead that R8 kept the
  classes Rust calls by name (`dexdump`) and that the log has no JNI lookup failure. With `MOQUI_SMOKE_APP=/qapps/...` and an
  account signed in, the frame check opens that server page and puts the frames there, and checks that the page cannot
  call a shell command or go to the shell's origin itself.
- `scripts/android-maps-offline.sh`: the acceptance walk `MapsOfflinePackage` of offline map packages, on the emulator
  when asked (not in `./moq test-all` or `accept-all`; about half a minute once the server's data is saved). It starts
  the server half, the tools acceptance spec `MapsOfflinePackage` (a small archive registered as a basemap, a saved
  place, a postal code, Overture rows and the user's package of the region, built), with `moqui test --serve` on
  `127.0.0.1:18086`, which the app reaches through `adb reverse`; installs the debug APK with its data cleared; and
  drives the app over its WebView's DevTools socket (`scripts/android-maps-offline.py`): sign in, Offline maps,
  Download until installed; the network cut (airplane mode, Wi-Fi and data off, the `adb reverse` removed) and the app
  ended in the background and started again (a connection made through `adb reverse` outlives the rule's removal);
  the drawer's Maps opening the offline screen; the package opened, its tiles drawn (the packaged style's earth and
  water colours in the map's canvas), offline and its built date said; a saved place, a postal code, an Overture place
  and an address found; a place saved, waiting in the outbox and not on the server; the network back, the outbox
  sending it and the server listing it. Needs the debug APK, a `./moq build --debug` binary (or
  `MOQUI_WALK_PACKAGE=release`) and python3's websocket-client; the server's log is `target/maps-offline/server.log`.
- `scripts/android-location-share.sh`: the phone half of the acceptance walk `LocationShareWalk` of live location
  (plans/Location-Sharing.md LL7), on the emulator when asked (not in `./moq test-all` or `accept-all`; about 40 s once
  the server's data is saved). It serves the tools acceptance spec `LocationShareWalkFixtures` (loc.walk.ann, ben and cy
  in Walk family) with `moqui test --serve` on `127.0.0.1:18087`, reached through `adb reverse`; installs the debug APK
  with its permissions granted and its data cleared; and drives the app over its WebView's DevTools socket and Android's
  own UI (uiautomator, `scripts/android-location-share.py`): sign in as Ann, the drawer's Maps, Share my location with
  Walk family (the emulator's position by `adb emu geo fix`), the app's confirmation dialog read and answered Send once
  Send is enabled, Browse saying the phone sends it, the foreground notification naming Walk family, Ben's read given
  the phone's position and, with the app in the background, the moved one; then the notification's Stop in the shade,
  after which Ben's read lists nothing, the notification is gone and Ann has no active share. The browser half (three
  browsers, the push, the extension, a member taken out) is `./moq accept tools --spec LocationShareWalk`. Needs the
  debug APK, a `./moq build` binary (or `MOQUI_WALK_PACKAGE=debug`) and python3's websocket-client; the server's log is
  `target/location-walk/server.log`.
