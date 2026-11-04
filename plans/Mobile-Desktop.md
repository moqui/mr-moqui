# Mobile and Desktop Clients

Status 2026-11-03: tentative implementation plan, reviewed against the code, [Edge-Node](Edge-Node.md) and [Comms-Framework](Comms-Framework.md). MD1 and MD2 are built as one app, [crates/moqui-desktop](../crates/moqui-desktop/README.md): Tauri 3 (alpha) on Chromium (CEF), with the node in the app's process and no port (the default; its window through the app scheme, the host keeping its cookies) or as the `moqui` sidecar on a loopback port, switched at run time (`MOQUI_DESKTOP_NODE`, or the tray's node item). The node in process prints PDF on the app's CEF when it has no Chromium of its own. What remains of MD1 and MD2 is their exit evidence on installed apps (§8), whose checks are in [Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md): pairing with an edge node and with an all-in-one node, mail across a sleep, calls, an upgrade that keeps the data, a backup restore, the startup and memory records, and Linux. MD3 is built in the same app, as its `client` mode ([moqui-desktop](../crates/moqui-desktop/README.md#client-of-a-server), [moqui-client](../crates/moqui-client/README.md)): device keys (renewed before they run out), password sign-in and SSO with the deep-link handoff, the app-scheme proxy, the host-held sockets, the bundled shell with BasicComms' chat panel, app windows, accounts, the device list (in the shell and in BOSS's Account settings) and the offline start, with an Android check of the client crate (`scripts/check-android.sh`). Its exit checks on installed apps are open. The Android check passes: the client crate and its dependencies (aws-lc's C code, the platform verifier) compile for `aarch64-linux-android` with NDK r30; the MD6 spike links it in a Tauri app on the Android emulator ([crates/moqui-mobile](../crates/moqui-mobile/README.md), §5). MD4 is built in the client mode ([Desktop-Client](../docs/ui/Desktop-Client.md)): device modes from `<client-conf>`, a SQLite store per account answering the comms transitions offline, the chat pushes and a mirror kept in it, the host's outbox and drafts with Compose idempotent by `clientRef` (`clientApi` 3), and Chat, Mail, Compose and Calls bundled in the shell from their shared source, with the shell owning the chat crypto device and the calls. Its exit checks on installed apps are open. MD5 is under way: the server's sync contracts (a mail change cursor, `clientRef` on more mutations, conflicts and chunked uploads, `clientApi` 4) are built ([Desktop-Client](../docs/ui/Desktop-Client.md#the-servers-sync-contracts)), with the chat changes feeds of stage 1. Build order:

1. A desktop local node in a Tauri app, linked as an Edge Client: in the app's own process (MD2, first stage) or as the `moqui` sidecar (MD1), one app with both. Built; the exit checks are open.
2. The same node with no port, through the app scheme (MD2, second stage). Built; the exit checks are open.
3. An offline-capable client of a remote server, with synchronization. The client of a remote server is built (MD3), and its offline data and bundled Comms (MD4); the exit checks of both are open. Reliable synchronization (MD5) is next.
4. An Android app on that client model, then iOS.

The in-process node came first because `moqui-host` already runs a node inside another program. `start_embedded` boots it on a thread and returns once it listens. Dropping the `Server` stops the listeners, the factory and then Postgres, in order, as `moqui tui --serve` does. Chromium, which MR Moqui already drives for PDF and the client tests, takes the place of the platform WebViews. It removes the WebKitGTK gaps of §3 (no WebRTC on Linux, slower rendering) and gives one engine on every desktop OS. The costs are an alpha runtime and about 170 MB more in each installer. The sidecar was then built beside it, as a switch rather than a fallback: the window, the page bridge, the tray, the OS integration and the backups serve both modes, and the sidecar adds the child supervision, readiness on `/health`, a stop when the app closes the node's stdin, and the takeover of a Postgres a killed node left running.

The mobile host is confirmed at MT5; Capacitor is the alternative. No packaged client or platform performance results are claimed here.

## 1. Direction and rationale

Build a locally installed desktop application with the existing Vue/Quasar UI and a Rust host, in this order:

1. **Desktop local node.** The app installs and supervises the regular `moqui` server as a sidecar process. It is the system of record, on embedded Postgres. It links as an Edge Client to an [edge node](Edge-Node.md), or to an all-in-one node: a full server that also runs the edge interfaces ([Edge-Node §10](Edge-Node.md#10-all-in-one-node-e12), E12). Its window shows the node's own apps.
2. **In-process full node.** The same node boots inside the app's process and opens no port, as `moqui tui` does.
3. **Offline-capable client.** A bundled client of a remote server, with local data, queued operations and synchronization.
4. **Mobile.** An Android app on the offline-capable client model, then iOS.

The first two steps give a person, household or small business the whole framework on their own computer. It is offline by holding the data, and it has public reach through the edge. The last two reach servers the user does not run on that computer, and phones.

The main reasons for this direction are:

- **Reuse:** preserve the Vue/Quasar components, JavaScript libraries, screen contracts, and application behavior already used by the web client. Avoid maintaining a second default UI in React or native widgets.
- **Rust performance and memory management:** put suitable storage, synchronization, cryptography, and local framework work in native Rust, consistent with the reasons for using Rust in MR Moqui. Control allocations, ownership, concurrency, and cache sizes in these paths.
- **Architectural flexibility:** a local node, a client with offline data and queued operations, and a remote client share one app and host. A complete server installation that the user manages separately should not be a prerequisite for opening the application.
- **Platform reach:**
  - Linux and macOS first, where the `moqui` binary is built and run today.
  - Windows after: embedded Postgres runs there, but it is a lower-priority target.
  - Android, then iOS.
  - Platform-specific work sits behind explicit adapters.

Tauri accepts web frontends and a Rust backend, with Swift/Kotlin available for platform integration. It uses the platform WebView, so the app does not ship its own browser engine. These are useful architectural properties, but they do not show that this application will start faster or use less memory than every alternative. [Tauri overview](https://v2.tauri.app/start/).

The Tauri build needs no Node. The `cargo tauri` CLI packages a static frontend directory, and `app.withGlobalTauri` gives the page `window.__TAURI__`, which fits the vendored global-script webroot as it is ([Node dependency](archive/NodeDependency.md)).

Here, a **bundled screen** is installed with the application and can open without downloading its UI from a server. It remains a Vue/Quasar DOM interface inside a WebView. Native OS behavior, such as incoming-call integration, comes from platform APIs; bundling a screen does not turn its controls into UIKit or Android widgets.

## 2. Existing foundation and gaps

The broader host-portability direction is in [Cross-Platform Usability](proposals/Usability.md). This plan makes the desktop/mobile client choice concrete while preserving the existing web and browser-local options.

The first five rows serve steps 1 and 2:

| Existing area | Useful starting point | Work still required |
|---|---|---|
| [Edge node](Edge-Node.md) | A local node behind NAT dials out to a cloud edge node, with no listening port. The edge node gives it public reach, stores and forwards mail and chat webhooks, and serves as the media node of calls. The System app's attach wizard pairs the two. A desktop or laptop local node is a listed topology. An all-in-one node, a full server that also runs the edge interfaces for its Edge Clients, is built (Edge-Node's E12). | Packaging a local node as a desktop application (§4, configuration 1). |
| [Run and deploy](../docs/Run-Deploy.md), [embedded Postgres](archive/EmbeddedPostgres.md) | `./moq archive --release --no-data` makes a runtime directory with the binary. Embedded Postgres is bundled in the build (no download at run time) and runs as a child postmaster under `runtime/db`. `save-db` and `load-save` dump and restore it. | The runtime's place in each OS's app data directory, its upgrade with the app, scheduled backups, and the Windows build of `moqui`. |
| [moqui tui](../docs/ui/TUI.md) | Locally, the client runs in the process on the runtime and opens no port unless `--serve`. With `--server`, it signs in through REST `login` with `createLoginKey=true` and keeps the login key. | The same in-process boot behind a WebView (§4, configuration 2). |
| [Native components](../docs/dev/Native-Components.md) | `./moq build` generates `target/moqui-app`, which names every component crate and calls `moqui_host::main()`. | An in-process app needs the same generated linking, with a library entry instead of `main`. |
| [Header sign-in](../docs/security/Attack-Surface.md) | A `login_key` header signs a request in and skips the CSRF token check. `/notws` accepts it, revalidates it every 60 seconds, and closes on revocation. `create#UserAccessToken` makes a described login key. | Device listing and revocation, and an SSO handoff: MR Moqui signs in through OIDC providers but is not an authorization server ([Sso](archive/Sso.md)). |
| [Webroot client and libraries](../runtime/base-component/webroot/screen/webroot/README.md) | Vue 3, Quasar 2, Moqui widgets, localization, rich text, charts, Markdown, and other browser libraries already form a complete web UI. | Steps 3 and 4 only; the local node serves its own `/qapps`. Package selected assets locally, and make paths, configuration, transport, and lifecycle host-independent. The `/qapps` shell is composed on the server for each request: `qapps.xml` picks scripts by signed-in user, reads the RUM settings through a service, and writes the settings JSON from preferences and `comms_app_root`. A bundled shell needs a static bootstrap and a settings request after sign-in. |
| [qvt client](../docs/ui/Qvt-Client.md) | Both server-merged templates and `server-static` compiled templates are supported. The latter separates a reusable render function from actions data. | Negotiate compatible client/template versions, package selected screens, and define which data/actions can work offline. |
| [BasicComms](../runtime/base-component/basic-comms/README.md) | Chat, inbox, and compose screens already use Quasar, with JSON transitions for data and mutations. | Extract reusable presentation and client logic without copying the application. Keep server-side services, authorization, and extensions authoritative. |
| [Chat store](../docs/ui/Qvt-Client.md#chat-store-and-presence) | `send#Message` takes a `clientRef` and answers a reused one with the message already sent (`duplicate`); `get#Timeline` pages by `eventSeq` (`afterSeq`); WebrootChat.js keeps an outbox that retries with backoff. | As built (MD4), the desktop client keeps the outbox in its store and sends it from the host; on the web it stays in `localStorage` under the account's scope. There is no changes feed across rooms, and mail has no sync cursor (§6). |
| [moqui-rtc](../crates/moqui-rtc/Cargo.toml) | The SFU is a sans-IO WebRTC endpoint on str0m, with an Opus encoder and decoder. Its tests hold the browser's SFrame frame layout to the independent `sframe` crate. | A client role: capture and playback, the client side of signaling, SFrame encryption in Rust (none encrypts call media today), and video if needed (§5). |
| [Shell and extension contracts](../docs/ui/Qvt-Client.md#shell-data-and-providers) | Shell discovery, app routes, counts, providers, and screen extensions allow the default client to work with different application compositions. | Preserve these contracts in the packaged shell instead of hardcoding a particular BOSS or BasicComms deployment. |
| [Core framework](../crates/moqui-core/Cargo.toml) | `moqui-core` has no listening socket or database driver; host responsibilities are already separate. | Audit the selected dependency/features set and supply native client storage, lifecycle, and execution adapters. |
| [Browser host](../crates/moqui-browser/src/host.rs) and [WASM work](archive/Wasm.md) | A local SQLite-backed framework host exists; its library also builds natively. It demonstrates local entity/service/screen execution without the normal server host. | It explicitly has no login flow and disables authorization for service calls/entity finds. It is groundwork, not a production authenticated offline client. |
| [PWA and Web Push](../docs/ui/Qvt-Client.md#install-and-web-push) | Versioned shell-asset caching and browser notifications already exist. | The current PWA is not an offline application/data replica and is not native APNs/FCM or incoming-call support. |

### qvt reuse

The two existing qvt paths need to remain distinct:

- **Server-merged qvt:** the server merges actions and templates, then the browser compiles/renders the Vue result. It is a useful online compatibility path.
- **Server-static qvt:** the server delivers compiled JavaScript registering a render function in `moqui.qvtStatic[path]`; the client obtains screen data from `<path>/actions` and list/tree data from the corresponding action endpoints. This is the better starting point for reusable templates and separately managed data.

“Server-static” does not mean offline, and does not mean that only data arrives from the server. The render function is executable JavaScript; actions may still require server services. A fully server-rendered HTML screen can also remain available in the remote surface where appropriate.

There is already a [precompiled component render bundle](../runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.render.js). [WebrootVue.qvt.js](../runtime/base-component/webroot/screen/webroot/js/WebrootVue.qvt.js) applies those renders when template hashes match, and [qapps.xml](../runtime/base-component/webroot/screen/webroot/qapps.xml) loads the bundle. Some prose in the qvt guide and the webroot README still describes that precompilation as future work. Implementation planning should use the current source and verify coverage, including late-registered components and packaged `.qvue` screens.

The current [MoquiLib loader](../runtime/base-component/webroot/screen/webroot/js/MoquiLib.js) uses relative URLs, same-origin credentials, CSRF tokens, and `new Function` when evaluating component scripts. A local application origin changes those assumptions. Precompile the bundled UI and introduce transport/configuration adapters. Copying the webroot directory does not produce a working authenticated client, and it does not remove the need for dynamic code in every remote screen.

### Comms reuse

For the bundled client (step 3), start with BasicComms chat and mail presentation, using their existing JSON contracts. Preserve `comms_app_root`, provider discovery, and `screen-extend`/extension actions so that the same installed client can connect to BasicComms or an application such as BOSS that mounts and extends it.

The JavaScript is not uniformly portable. [WebrootChat.js](../runtime/base-component/webroot/screen/webroot/js/WebrootChat.js) mixes reusable state handling with browser storage, connectivity, and visibility. [WebrootCall.js](../runtime/base-component/webroot/screen/webroot/js/WebrootCall.js) depends on browser media APIs and signaling. [WebrootCrypto.js](../runtime/base-component/webroot/screen/webroot/js/WebrootCrypto.js) uses the Matrix crypto WASM package, IndexedDB, and browser coordination primitives. Extract adapters around these responsibilities rather than rewriting all of them in Rust immediately.

## 3. Candidate research and disposition

| Candidate | Fit for this project | Disposition |
|---|---|---|
| **qvt client + Tauri 2** | Retains the Vue/DOM UI while adding a Rust host and desktop/mobile packaging. A natural place to integrate selected MR Moqui Rust capabilities, in the same process. Builds without Node. | Start here, with the desktop local node. Desktop remains a target regardless of mobile outcome. Mobile viability depends especially on comms and lifecycle integration. |
| **qvt client + Capacitor** | Preserves the same Vue/Quasar UI as a plain web directory (`webDir`), with a mature native plugin set, including an official push plugin. Quasar CLI's Capacitor mode would need the webroot converted to a Quasar CLI app, which this plan does not require. The Capacitor CLI runs on Node, which would enter the mobile build. Rust can still sit behind a custom Swift/Kotlin plugin. | Retain as the mobile option if it materially reduces integration or maintenance work. It does not replace the Tauri desktop direction. [Capacitor](https://capacitorjs.com/docs). |
| **NativeScript-Vue** | Vue programming model with native controls, but existing HTML/Quasar components and DOM libraries are not native-control components. | Consider only if native widget UI becomes a requirement worth a substantial presentation rewrite. [Vue integration](https://nativescript-vue.org/), [DOM and platform differences](https://nativescript-vue.org/docs/essentials/gotchas). |
| **React Native + Expo** | Native UI and a relevant media/calling ecosystem; custom native modules use development builds. WebViews can retain remote web applications. | Useful comparison for difficult mobile integrations, but a React/native UI rewrite gives up too much current UI reuse for the default direction. [Expo native customization](https://docs.expo.dev/workflow/customizing/), [React Native WebRTC](https://github.com/react-native-webrtc/react-native-webrtc). |
| **Ionic Vue / Framework7 Vue** | Both offer Vue mobile UI systems. | Additional UI migrations without a demonstrated advantage over the Quasar already in use. Capacitor does not require adopting Ionic UI. [Ionic Vue](https://ionicframework.com/docs/vue/overview), [Framework7 Vue](https://framework7.io/vue/introduction/). |
| **Rust-authored UI, e.g. Dioxus or Slint** | Can move UI authorship toward Rust, with a different component/rendering model. | Not the initial path: it reduces reuse of the current Vue/Quasar UI. Using Rust for the host does not require a Rust UI rewrite. [Dioxus](https://github.com/DioxusLabs/dioxus), [Slint](https://slint.dev/faqs). |

The Tauri client is the existing web frontend in a Tauri host, not a Quasar CLI mode. For the local node, the window shows the node's own pages. For the bundled client (MD3), stage its static files from `webroot/libs/MANIFEST.json` and the selected scripts. Avoid making a conversion of every existing web application to a new frontend toolchain a prerequisite.

### Platform facts that shape the design

- **Linux.** WebKitGTK ships with WebRTC disabled upstream ([WebKit bug 274711](https://bugs.webkit.org/show_bug.cgi?id=274711)); working setups use custom builds and a media permission handler in the embedding app ([Tauri discussion 8426](https://github.com/tauri-apps/tauri/discussions/8426)). Calls on Linux need another path (§5). WebKitGTK also renders more slowly than the other WebViews.
- **A Chromium runtime for Tauri** (`tauri-runtime-cef`) is in alpha: 3.0.0-alpha.6 (October 2026), on CEF 154 and `tauri` 3.0.0-alpha.5.
  - The desktop prototype uses it on every desktop OS. CEF re-launches the app's binary for its renderer, GPU and utility processes, and `cef_entry_point` sends those to CEF before the app starts.
  - The first build downloads the CEF distribution, about 1 GB.
  - Windows bundling is blocked upstream for now.
  - [tauri-runtime-cef](https://docs.rs/crate/tauri-runtime-cef/3.0.0-alpha.6).
- **Webviews.** Child webviews in one window are behind Tauri's `unstable` feature, on desktop only. Mobile has one webview per window; multiple windows on mobile are an open pull request ([tauri 14484](https://github.com/tauri-apps/tauri/pull/14484)). An iframe of the server inside the app's origin meets third-party cookie blocking in WKWebView and needs the server's `frame-ancestors`.
- **Mobile push and calls.** Tauri has no official APNs/FCM push plugin; third-party ones exist ([tauri-plugin-mobile-push](https://github.com/yanqianglu/tauri-plugin-mobile-push), [tauri-plugin-remote-push](https://docs.rs/crate/tauri-plugin-remote-push/latest)). Neither Tauri nor Capacitor has CallKit, Android Telecom or VoIP push built in.
- **Encrypted calls** require `RTCRtpScriptTransform`: Chromium 141 and later, including WebView2 and Android WebView ([intent to ship](https://groups.google.com/a/chromium.org/g/blink-dev/c/QeosFeu-d8A)), and Safari; check WKWebView.
- **Service worker and Web Push.** Do not count on them in a Tauri page. Native notifications come from the host or the node (§4).

## 4. Proposed architecture

One Tauri app and Rust host serve four configurations, built in the order of §1.

```mermaid
flowchart TB
    Window["Tauri window<br/>The qvt client"]
    Host["Tauri Rust host<br/>Supervisor, app scheme, commands"]
    Node["Local node: moqui-host and moqui-core<br/>Sidecar process (1), in process (2)"]
    PG["Embedded Postgres<br/>In the app data directory"]
    Edge["Edge node, or an all-in-one node<br/>(a full server running the edge interfaces)"]
    Store["Client store, outbox, crypto (3, 4)"]
    Server["Remote MR Moqui (3, 4)"]
    OS["OS integrations<br/>Tray, notifications, links / Kotlin / Swift"]
    Window -- "loopback HTTP (1)" --> Node
    Window -- "app scheme and commands (2, 3, 4)" --> Host
    Host -- "supervises (1), in process (2)" --> Node
    Node --> PG
    Node -- "edge link, dialed out" --> Edge
    Host --> Store
    Host --> Server
    Host --> OS
```

### Configuration 1: desktop local node, sidecar

The app runs MR Moqui itself as the system of record, for a person, household or small business running BOSS on their own machine. It needs no new sync protocol: the node holds the data.

- **Payload.** The app ships the package (Maximal or Enterprise) as a runtime directory, in the layout `./moq archive` makes, with the `moqui` binary as a [Tauri sidecar](https://v2.tauri.app/develop/sidecar/) (`bundle.externalBin`, built and staged by `./moq desktop build`). As built: the app ships Maximal, and the runtime in the OS's app data directory has its components and conf as links into the app's resources, while `.env` (with the generated secrets), `db` and logs are written there. The installed copy stays read-only. An upgrade replaces the binary and components and keeps the data.
- **Lifecycle.** The host starts `moqui` bound to loopback, waits until it answers (its `Listening on` line, then `/health` ready), and shows the node's `/qapps` in the window. On quit or OS shutdown it stops the node gracefully, which stops Postgres: the node runs with `moqui_stop_on_stdin_close` and the host holds its stdin, so the host ending in any way stops it, then SIGTERM and a kill. A Postgres whose owner process is gone (`moqui-owner.pid`) is taken over and stopped by the next start. A failed start, or a node that ends by itself, shows its reason on the splash page with a Restart button. One instance runs per user.
- **The window is a browser session on the node's own origin.** Sign-in, CSRF and sockets work as on the web, and every app on the node works. The window is Chromium (CEF) on every desktop OS, as in configuration 2. This step needs no bundled shell, version handshake or transport work. Through a capability the host adds at run time for the node's exact origin, the node's pages get a few commands and nothing else, through `window.moquiDesktop`: `notify`, `set_counts` (the tray and badge), `set_presence` and `open_external`. Saving files needs no command: a download from the node is Chromium's, with the save dialog.
- **Desktop features:**
  - a tray icon with unread counts and presence, and the app's menu: Show, Restart node, the node mode, the client item (a client of a server, configuration 3, which has none of the node's items or Backups), Start at login, Make default for email and calls, Check for updates, Backups, Quit;
  - closing to the tray, so mail and calls keep arriving;
  - autostart at login (off until turned on) and a single instance;
  - registration as the `mailto:`, `tel:` and `sip:` handler, opening the node's compose or dialer;
  - native notifications from the node's notification topics;
  - signed packages and updates.
- **First run.** Create the initial admin, as an empty production runtime asks, then pair with the edge through the [attach wizard](Edge-Node.md#57-attaching-a-local-node-system-app).
- **The edge.** The link dials out, so the computer needs no open port.
  - While the computer sleeps, the edge node queues mail and webhooks and shows its offline page. It delivers in order when the link comes back.
  - Calls have their media on the edge node.
  - Phones and other computers reach the node's apps through the edge node's web access. The loopback port is not exposed on the LAN.
- **The peer** is either an edge node (`node_role=edge`, MoquiEdgeConf.xml) or an all-in-one node (`node_role=all-in-one`, MoquiAllInOneConf.xml): a full server that also runs the edge interfaces for its Edge Clients. On an all-in-one node each name, address, number and bridge belongs to the server or to the paired node and is routed by owner ([Edge-Node §10](Edge-Node.md#10-all-in-one-node-e12), phase E12, built).
- **Backups.** The computer is the system of record, so schedule `save-db` dumps to a location the user chooses, and prove a restore. As built: the nightly backup job runs into `MR-Moqui Backups` in the user's Documents, or a folder chosen from the tray's Backups menu, through the properties `embedded_db_backup_scheduled` and `embedded_db_backup_dir`; the menu also has Back up now and Restore, which first backs up the data it replaces, then runs `moqui load-save`.
- **Sleep and network changes.** The edge link reconnects after an address change or a dropped mapping. Check scheduled jobs after a long sleep: the scheduler runs a missed job once on its next pass, and the host, seeing the clocks part after a sleep, asks the node's `/health` again.
- **PDF** needs Chromium (`moqui_chromium_path`). Point it at an installed Chrome or Edge; otherwise PDF is unavailable, as on a server without it. With the node in process and CEF in the app (configuration 2), the app could supply the node's PDF engine instead: a `PdfEngine` (`moqui_core::screen::pdf`) on CEF's `PrintToPDF` in a windowless browser. The desktop package ships Chromium as CEF anyway, for its window. As built: the node in process, when it finds no Chromium of its own, prints on a hidden CEF window driven over the DevTools Protocol with the server's own page sequence (`src/pdf.rs`, `moqui_host::pdf::print_page`); the bundle carries Chromium's notices ([License-Review](archive/License-Review.md)). A sidecar node still needs `moqui_chromium_path`.

### Configuration 2: full node in the app's process

- **Boot.** The app links `moqui-host` and the component crates through a generated crate, as `./moq build` does with a library entry in place of `main`. It boots the runtime in its own process, as `moqui tui` does.
- **Processes and ports.** Postgres is still a child process, but there is no separate `moqui` process and, by default, no port. An option opens the HTTP, SSH and cluster ports for a browser or other devices, as `moqui tui --serve` does.
- **The window** loads the node's pages through the app scheme (below), whose handler calls `moqui-host`'s axum router in process. Sockets are bridged. As built (`moqui_host::in_process`, `crates/moqui-desktop/src/node/scheme.rs`): the window is on `http://moqui-node.localhost`; a request goes through the same checks and router as a listener's, and a socket is an HTTP/1.1 connection over an in-memory pipe. The host keeps the node's cookies for the window, as a browser would, so the pages sign in with the node's own login (the initial admin too) and keep a person's session, CSRF token and `/termws`; a login key in a header would make the session a program's (no agent approvals). The tray's "Open ports for other devices" opens the ports for a browser or other devices.
- **What it removes:** process supervision, the port and its conflicts, and the HTTP hop on each request. It uses the same data directory as configuration 1, so an installed node moves from one to the other by an app upgrade.
- **The edge link** still dials out from the in-process node; nothing about the edge changes.

### One transport seam

From configuration 2 on, the window's pages come through a custom URI scheme of the host:

- In configuration 2 the handler calls the in-process router, with the node's cookies, which the host keeps.
- In configurations 3 and 4 it serves the bundled assets and forwards server requests, adding the account's `login_key` header.

Either way the page is on the app's own origin and never holds the credential; with a key (configurations 3 and 4) it needs no CSRF token (header sign-in skips the check), and with the node's cookies (configuration 2) it has the node's token, as on the web. In configuration 3 the handler can also answer from the local store without the page changing. As built for the client (MD3), there are two origins: the shell's (`moqui-shell`, which takes only the server's data, never its pages or scripts, and serves its own paths only from its staged files) and the app windows' (`moqui-app`, the server's own pages); the handler answers the shell's settings, apps and counts from what it kept when the server cannot be reached. As built for MD4, the handler writes the 200 JSON answers of the chat, mail and compose transitions into the account's store, with structured rows, and answers them from it when the server cannot be reached, marked `X-Moqui-Offline: 1` (its own 502 is `X-Moqui-Offline: unreachable`); the pages' outbox and drafts are host commands (`moquiDesktop.outbox`, `moquiDesktop.drafts`), and an app window's `open_in_shell` shows a comms screen in the shell. [Tauri URI scheme protocols](https://docs.rs/tauri/latest/tauri/struct.Builder.html#method.register_asynchronous_uri_scheme_protocol).

WebSockets cannot go through a custom scheme, and a page's WebSocket cannot set the `login_key` header (`api_key` in a query is refused). The host holds the sockets (`/notws` and `/callws`, and in process the collaboration and terminal sockets) and bridges their messages to the page, behind the socket adapter in MoquiLib (`moqui.openSocket`); as built, the frames reach the page as an evaluated call in its webview.

On CEF two things of a scheme's HTML answers need handling, as built: the runtime puts the app's initialization scripts into every `text/html` answer, so HTML a page fetches (a `.qvue`, a fragment) is handed on as text; and it adds those scripts' hashes to a page's CSP, creating a `script-src 'self'` where the server set none, so a server page's CSP with no script policy is not passed on.

Check these on each WebView in MD2:

- whether the scheme is a secure context (media capture, Web Crypto, workers);
- the CSP;
- Range requests for audio and video;
- response bodies, which the handler returns whole, so large downloads go through a native download command.

As checked on CEF 154 (macOS, a development run): the scheme is a secure context, with Web Crypto, workers, `getUserMedia` and `RTCRtpScriptTransform`; a Range request is answered 206; Chromium sends a scheme no `Sec-Fetch-*` headers, so the node's scheme takes a window's document `GET` from another origin (the splash page) and refuses other cross-origin requests. A response body is whole, so an attachment or a file over 16 MB a window opens is saved by the host, streamed to disk after the save dialog (`src/download.rs`), and an event stream, which never ends, is answered 503 (the Assist drawer then polls). Camera and microphone are granted to the app's own pages only (Alloy style would grant them to any page).

### Configuration 3: offline-capable client of a remote server

This is a bundled client of a server the user does not run on this computer: another node through its edge node, or a company server. It keeps local records, attachments, drafts, search, and durable queued mutations, and the server validates synchronized changes (§6). It starts as a remote client, with business operations on the server, and adds offline data from there. It is also the model of the mobile app. The desktop features of configuration 1 apply too.

#### Bundled shell and host contracts

Bundle startup, server/account selection, settings, navigation, and an initial BasicComms subset. Keep shell startup independent of network availability. Obtain authorized remote application entries, extension data and the shell settings from the selected server after login. Bundle the runtime-only Vue build and the precompiled component renders (`WebrootVue.qvt.render.js`), and precompile the bundled screens, so the shell runs under a CSP without `unsafe-eval`. As built, the shell's `.qvue` screens and BasicComms' chat panel are compiled into the app (`vue_check::precompiled_qvue`, `moqui.bundledComponent`), and a screen not bundled opens in an app window. As built for MD4, BasicComms' Chat, Inbox (Mail), Compose, Calls/Call and Calls/Dialer are bundled too, with WebrootCall.js's components precompiled (`WebrootCall.render.js`), tiptap and the crypto library; the CSP adds only `wasm-unsafe-eval` and one worker, the staged SFrame transform. The shell routes them under the account's comms app, and OS and notification links to them open there. An app that extends them (BOSS) keeps its `screen-extend` transitions, `ext-action` items and shell providers in the shell; its own screens, and BasicComms' others (Calls' History, Voicemail and Recordings, Settings, Home), open in app windows. The bundled screens are the app build's, so a server's newer screens show only in app windows until the app is updated.

#### Offline data (MD4, as built)

- **The store.** One SQLite file per account (`moqui-client` `store`, `data/<account>.db`, owner-only, WAL, migrated by `user_version`), opened while the account is listed: the cached answers, chat rooms and events with the `eventSeq` ranges held whole, mail folders, threads and messages, the outbox, drafts and their files. A size cap (`store-max-mb`) evicts the least recently read cached reads, never the outbox, drafts or their files.
- **Offline answers.** The chat, mail and compose transitions are answered from the rows through the same paths the screens call: a timeline page, room view, room list, a folder's thread page or a thread in the server's shape, a gap ending the paging. The screens do not fork.
- **Pushes and the mirror.** The chat pushes on the host's `/notws` are folded into the store as `get#Timeline` shows them, encrypted rooms too. When the shell's socket opens, the mirror reads ahead by device mode: a personal device all rooms and mail folders within `mirror-days`, a shared one the latest rooms and inbox threads; bounded and cancelled on a switch or sign-out.
- **The outbox and drafts are the host's.** Chat sends and reads, mail sends and thread state changes are kept in the store and sent by the host's flusher with WebrootChat's rules, while the window is hidden too; a mail send is the draft saved, its files uploaded and the send, each once by `clientRef`. WebrootChat's `localStorage` outbox and the drafts are moved in item by item. An internal message is not queued (its idempotency is MD5).
- **Device modes** from `<client-conf>`: `personal` keeps the account's data until the account is removed; `shared` deletes it on sign-out and revocation; `choose` has the sign-in ask.
- **Crypto and calls are the shell's.** The shell owns the account's chat crypto device, its store key kept in the OS key store so cached ciphertext decrypts offline; app windows make no device (`elsewhere`) and open encrypted rooms and calls in the shell (`open_in_shell`). The shell owns every call: it rings, and an app window rings silently and hands its calls to it.

The details are in [Desktop-Client](../docs/ui/Desktop-Client.md).

Define a small shared client API for requests, sockets, account identity, persistence, connectivity, notifications, files, deep links, and lifecycle events. Provide browser and Tauri implementations, and allow a Capacitor implementation later. Keep general UI state in Vue; keep durable client records and native operations behind these contracts. Use explicit result/error types and cancellation rather than exposing an arbitrary native service executor to JavaScript. [Calling Rust from Tauri](https://v2.tauri.app/develop/calling-rust/).

Keep Tauri and mobile platform dependencies at the host boundary. Follow [Rust practices](../docs/dev/Practices-Rust.md) and [Crate-Split](Crate-Split.md) when choosing module/crate placement. Put the Tauri host crate outside the workspace members, as component crates are: on Linux Tauri builds against the WebKitGTK and libsoup development packages, which `cargo test --workspace` and every server build would otherwise need. The client host of configurations 3 and 4 links neither the full server host nor browser-specific fetch/worker infrastructure. Extract reusable SQLite or runtime pieces from the browser work only where required.

#### Remote applications and authentication

Initially, load remote applications with their own server-compatible qvt runtime, the server's `/qapps` and its scripts, through the app scheme: in their own window on desktop, and in the one webview on mobile. This preserves broad screen compatibility while the bundled shell evolves. Sharing compiled remote screens inside the shell can be explored later with an explicit compatibility contract; it should not silently mix different Vue, Quasar, and Moqui widget versions.

Each installed client is a device with its own login key:

- Password sign-in (with MFA) posts through the host to REST `login` with `createLoginKey=true`, as `moqui tui --server` does. The host keeps the key in the OS key store: Keychain, Credential Manager, or Secret Service. As built, with `deviceName` the key is the device's (90 days, its last use noted), and a Linux desktop with no Secret Service keeps it in a file only the user can read, which the shell says.
- SSO opens the system browser and returns a one-time code to the app by deep link, which the host exchanges for a device key ([RFC 8252](https://www.rfc-editor.org/rfc/rfc8252)). As built: `/sso/appLogin` with an S256 challenge, a state and a registered redirect URI (`webapp_app_redirect_uris`, by default `org.moqui.desktop:/sso`); after the provider's round trip the browser asks the user to let the app in, and on that hands a single-use code to the app, which takes it with its verifier to `/sso/appToken`.
- The user lists and revokes devices in My Account (`get#UserDeviceKeys` and `revoke#UserDeviceKey`; the shell's Apps and devices, and BOSS's Account settings). Revocation already closes `/notws` within 60 seconds; requests then fail with 401 and the client returns to sign-in.

Define the remote API base URL, app root (`comms_app_root`), server identity, account identity, and supported client features explicitly. Handle key expiry, revocation, redirects, downloads, and logout.

Remote screens are the server's code, and the server is already trusted with the user's session; what a packaged client adds is native capability.

- Keep commands few and typed.
- Never expose the device key, the filesystem, the key store, or a shell to a page.
- On desktop, give remote applications a window with no command access, using Tauri's capability scopes by window and origin. As built, an app window has the socket commands only, which reach nothing its requests do not already reach with the key; Tauri counts the schemes' pages as local, so the capabilities name the windows, and each window's navigation stays on its origin.
- On mobile, where one webview shows both, the command set is the boundary.

[Tauri capabilities](https://v2.tauri.app/security/capabilities/).

Loading a remote `.qvue` script or compiled render function into the bundled shell would execute it in that shell's context. Any later shared-runtime approach must account for that privilege change as well as version compatibility; an origin allowlist alone does not isolate scripts after they are loaded into the privileged page.

For a Capacitor variant, retain the locally bundled shell. Capacitor documents its `server.url` and navigation configuration mainly for development and live reload, not as a production remote-shell architecture. An embedded browser also needs its own authentication/storage design. [Capacitor configuration](https://capacitorjs.com/docs/config), [InAppBrowser](https://capacitorjs.com/docs/apis/inappbrowser).

### Later: a selected local runtime in the client

Packaged definitions and services could execute through `moqui-core` in the client, with a local database and host capabilities, beside a remote server that stays the system of record. Build it only after the offline-capable client, from a proven subset.

- Opening a bundled chat screen should not require booting every framework tool, loading every component, or starting Postgres.
- A locally rendered business screen is not useful offline unless its actions, entities, services, and resources are also available locally.

## 5. Calls, Rust and mobile

Rust is a practical shared implementation language for storage, synchronization, networking, cryptography, parsing, and business rules on both Android and Apple platforms. Rust documents iOS and Android targets, while the platform SDKs/toolchains are still required. Keep the client host crates of configuration 3 building for `aarch64-linux-android` while they are written, so step 4 does not start with a port. That Rust itself supports a target does not prove that every crate or native dependency works there. [Rust Android targets](https://doc.rust-lang.org/rustc/platform-support/android.html), [Rust iOS targets](https://doc.rust-lang.org/rustc/platform-support/apple-ios.html).

For Tauri, keep portable work in Rust and use Kotlin/Java on Android and Swift on iOS for platform APIs and lifecycle entry points. Tauri's mobile plugins call Kotlin and Swift from Rust and send events to the page. A push handled while the WebView is suspended needs calls from Kotlin or Swift into Rust; those are the app's own JNI or FFI in the same process, so prove that path in MT2. None of this grants unlimited background execution. [Tauri mobile plugins](https://v2.tauri.app/develop/plugins/develop-mobile/).

If Capacitor is chosen for mobile, the same Rust library can be exposed through native plugins, potentially with UniFFI-generated Kotlin/Swift bindings. That adds binding, packaging, and lifecycle work: a Tauri plugin does not simply become a Capacitor plugin. [UniFFI](https://mozilla.github.io/uniffi-rs/latest/).

| Responsibility | Initial implementation direction |
|---|---|
| Screens, forms, responsive layouts | Existing Vue/Quasar and selected shared JavaScript. |
| Persistence, outbox, sync state, suitable computation | Native Rust, with bounded background work and an explicit account/server scope. |
| Push registration, secure credential storage, deep links, permissions, sharing | Existing suitable plugins or small platform adapters. |
| Incoming calls, audio sessions/routes, background transfers | OS-specific integration; Rust can share state/protocol logic. |
| Media capture, codecs, WebRTC transports | Browser media where the WebView has it. Where it does not, or where a call must run without the WebView, a native client in Rust, audio first: the SFU's str0m endpoint and Opus code, with the `sframe` crate for encryption. Capture, playback and the client side of signaling are new work. |

### Comms is the key gate, on desktop too

Calls on desktop depend on the WebView, in every configuration; with a local node their media is on the edge node.

| Desktop | WebView | Calls |
|---|---|---|
| Any, with CEF (the app) | Chromium 154 | Expected to work: WebRTC, and `RTCRtpScriptTransform` from Chromium 141, present on the app scheme. Camera and microphone are granted to the app's own pages only; a call is still to check on installed apps |
| Linux | WebKitGTK | None: no WebRTC upstream. Open calls in the system browser first; later a native audio client in Rust, or CEF |
| macOS | WKWebView | Check media permissions and `RTCRtpScriptTransform` |
| Windows | WebView2 (Chromium) | Expected to work: WebRTC, and `RTCRtpScriptTransform` from Chromium 141. Check |

Encrypted calls still fail closed in every case.

The current browser call and crypto implementation is useful starting code, not proof of a production phone client. Validate the following on real devices:

- microphone/camera permissions;
- Bluetooth and speaker routing, and interruptions;
- lock-screen behavior;
- push delivery and notification actions;
- behavior after resume and after the process is killed.

Incoming-call UI, wake-up/signaling, and the media engine are separate responsibilities. Android Telecom/CallKit integration does not itself implement WebRTC or the Moqui SFU protocol. A relevant reference is [react-native-callkeep](https://github.com/react-native-webrtc/react-native-callkeep), whose scope is native call integration, alongside the separate WebRTC library.

Native push needs a device-registration and delivery path using FCM/APNs as appropriate; the existing browser VAPID subscriptions are not interchangeable. Do not infer full push or VoIP support from a generic notification API. Even Capacitor's push plugin documents limits around iOS silent pushes and Android killed-process handling. [Tauri notifications](https://v2.tauri.app/plugin/notification/), [Capacitor push notifications](https://capacitorjs.com/docs/apis/push-notifications).

Android distinguishes deferred work, foreground services, and specific platform use cases; iOS background work is system-controlled. Design short resumable operations and native callbacks, with reliable state persisted before suspension. Rust threads and a WebSocket do not provide an always-running phone service. [Android background work](https://developer.android.com/develop/background-work), [Apple background strategies](https://developer.apple.com/documentation/backgroundtasks/choosing-background-strategies-for-your-app).

**Android first.** Android's WebView is Chromium: it has WebRTC and `RTCRtpScriptTransform` from 141, and it updates apart from the OS. Three platform pieces carry a call:

- An FCM high-priority message can wake the app.
- A self-managed call (Android Telecom, or the Jetpack Core-Telecom library) under a `phoneCall` foreground service can run with the screen locked.
- Android 14 limits the full-screen intent of an incoming-call screen to calling and alarm apps.

iOS follows with PushKit and CallKit.

**The Android spike (MD6 stage 5), 2026-11-03.** Tauri 3.0.0-alpha.5, the desktop app's version, builds and runs on Android on `tauri-runtime-wry` (the system WebView) with no change: [crates/moqui-mobile](../crates/moqui-mobile/README.md), a static page on the `moqui-36` emulator (API 36, x86_64) that shows an HTTPS request moqui-client made from Rust. `rustls_platform_verifier::android::init_with_env` (0.7's init; it has no `init_hosted`) runs in the app's setup with the JVM and context tao records in `ndk-context`, and the verifier's Kotlin component is its Maven artifact in the Gradle project: a valid certificate passes and an expired one is refused by Android's trust manager. The aarch64 build compiles too. The emulator image's WebView is 133, below the 141 encrypted calls need, so MT2 needs an updated WebView (Play Store) or a phone.

**Choosing the mobile host.** In both hosts, Telecom, CallKit and VoIP push are Kotlin and Swift the project owns. Tauri's advantage is that this code can call the Rust call client, crypto and sync in the same process, with no WebView and no binding layer. Its costs are third-party push plugins, one webview per window, and younger mobile tooling. Capacitor's costs are a bridge to the same Rust, and Node in the build.

### Crypto and media compatibility

The existing Matrix crypto WASM library has a Rust foundation. The upstream Matrix Rust SDK is used by Element X on iOS and Android, and includes a standalone crypto crate. That makes native crypto reuse a concrete candidate. However, MR Moqui's `/ChatKeys` transport and device model are not the Matrix client/server API, so adopting the complete Matrix client SDK would not implement them. [Matrix Rust SDK](https://github.com/matrix-org/matrix-rust-sdk), [existing chat-key design](../docs/interfaces/Chat-Keys.md).

Choose one crypto-state owner per packaged-client device. If using native crypto, verify key persistence, recovery, verification, concurrent access, and compatibility with existing browser devices; do not assume IndexedDB and native stores are interchangeable. A new native device identity may be safer than a first-release store migration. Keep the browser implementation usable for the web client. As built (MD4), the desktop client's shell owns the device, still WebrootCrypto in the page, named for the computer; app windows make none, and nothing is migrated (§9).

[WebrootCallKeys.js](../runtime/base-component/webroot/screen/webroot/js/WebrootCallKeys.js) requires browser transform/worker/crypto support for encrypted calls. Test the actual WebView capabilities on supported OS versions and preserve fail-closed behavior. If a native media engine is needed, its signaling, SFU behavior, and frame-encryption integration must interoperate with the existing [WebRTC design](../docs/interfaces/WebRTC.md); silently dropping encryption is not a fallback.

## 6. Offline behavior and data ownership

Configurations 1 and 2 are offline by holding the data; this section is about configuration 3 and the mobile app. Offline capability is an application contract, not a packaging feature. Start with a small useful set: opening the shell, viewing explicitly cached conversations or mail, editing drafts, and showing clearly pending operations. As built, MD4's scope is chat and full mail (below).

The tentative persistence/sync design should include:

- **A partitioned local store.**
  - Partition it by stable server and account identity.
  - Provide schema migrations, bounded attachment/cache retention, and deliberate sign-out/account-removal behavior.
  - Store credentials and encryption keys through an appropriate protected native store; SQLite alone does not provide an encryption policy.
  - Browser storage keys in the shared JavaScript assume one server per origin: the chat outbox (`moqui.chat.outbox.<userId>`) and the crypto device pointer are keyed by `userId` alone. In a packaged client every server shares the app's origin, so move the outbox into the host store keyed by server and account, and key the rest by both. As built (MD3), they and the drafts are under the account's storage scope (`moqui.scopedKey`); as built (MD4), the outbox and the drafts are in the account's store, moved in from the scoped `localStorage` item by item.
- Atomic recording of a local change and its outbox entry. Use stable operation IDs and server-side idempotency so retries after a lost acknowledgement do not duplicate sends or other effects.
- Explicit queued, sending, acknowledged, retryable-failure, and rejected states. Preserve failed drafts and identify the affected item; one independent failure must not stop unrelated synchronization. Respect real dependencies between operations.
- Cursor/revision-based incremental synchronization, deletion/tombstone handling, reconnect recovery, and a policy for expired cursors or required resynchronization. Avoid relying on wall-clock timestamps alone.
- Domain-specific conflict rules. Sending a message, editing a draft, changing a shared record, and transitioning a business document do not necessarily have the same merge semantics.
- Server reauthorization of every accepted mutation. Offline UI permissions are cached hints; access revocation and changed business state can reject a previously queued action. Define what happens to cached data after revocation when connectivity returns.

Chat already has the start of this: an idempotent send by `clientRef`, `eventSeq` paging with `afterSeq`, and a retrying outbox (§2). Build on those.

As built (MD4), the desktop client implements the first part:

- the partitioned store, one SQLite file per account, migrated by `user_version`, with a size cap and retention by device mode;
- offline reads of what was read or mirrored: the chat, mail and compose transitions answered from the stored rows, a gap in the held ranges ending the paging;
- the chat pushes folded into the store, and the heads read again after a drop;
- the host's outbox with explicit states (queued, sending, retry, held, failed, acknowledged), ordered per target, claimed atomically, recovered after a crash, with Retry and Discard in the screens; an operation and its files kept together;
- drafts kept offline, Chat's and Compose's, an email draft by its message;
- idempotent sends: chat's `send#Message` and, since MD4, Compose's `saveDraft`, `upload` and `send` by `clientRef` (`clientApi` 3), so a queued mail is sent once after a lost answer; `composeData` answers a draft's `clientRef`.

Still missing, for MD5:

- a changes feed across rooms: new rooms, membership, and edits, redactions and deletions as tombstones beyond the ranges the store holds. The server side is built (MD5 stage 1), as two feeds so a change of a big room's messages locks no member's counter: each user's (`ChatUserChange`, a counter per user in `ChatUserState`) for membership as state, room, settings, read and retention (a range tombstone), and each room's (`ChatRoomChange`, `ChatRoom.lastChangeSeq`) for messages changed after they were sent (edit, reaction, redaction, pin, link previews and polls), each coalesced per thing changed. The Chat screen's `chatChanges` (`afterSeq`, `limit`, `headsSince`) answers `{changes, lastSeq, more, reset, heads, headsSince}`, `heads` giving each room's newest event and message change since new messages are in no feed, and `roomChanges` (`chatRoomId`, `afterSeq`, `limit`) a room's; `chatUser:` pushes carry `changeSeq` and `chatRoom:` pushes `roomChangeSeq` ([Qvt-Client](../docs/ui/Qvt-Client.md#chat-store-and-presence), "The changes feeds"). Folding it into the store is stage 3;
- a change cursor for mail folders. Built on the server (MD5 stage 2): `mailChanges(afterSeq, limit)`, one change per thread with the thread's messages as they are and its removed messages as tombstones, `reset` for a cursor the log no longer covers, `unsupported` when an app's inbox source has none ([Desktop-Client](../docs/ui/Desktop-Client.md#the-servers-sync-contracts));
- resumable attachment uploads. Built on the server: `uploadChunk` with strict offsets and `uploadStatus`, then Compose's and Chat's `upload` by `uploadId`; unfinished uploads expire;
- the same idempotency for internal messages (not queued until then) and for mutations other than a chat send and a mail send. Built on the server for the other mutations: a general `clientRef` (`ClientRequestRef`) on Chat's edit, redact, react, vote and mark-read and the Inbox's state; IronStone's internal send is still to use it;
- conflict rules, for a draft edited in two places and a thread state changed on the server while a queued change waits. Built on the server: `baseStamp` on a draft's save and send (the server's draft wins, the client keeps its own as a copy) and `seen` on each state target (the server's state wins and is answered);
- the client's use of all of these (MD5 stage 3).

Design these contracts jointly with the client. Reconcile this work with [Messaging](proposals/Messaging.md), without making its entire proposal a prerequisite for the first client.

For a later local runtime in the client, audit SQLite dialect coverage and transaction semantics as well as schema compatibility. The existing browser host uses a shared SQLite handle, and nested logical transactions use savepoints rather than independent commits, which changes `requires-new` behavior. That implementation cannot be moved over unchanged and called equivalent to the server. Authentication/authorization, unavailable host tools, selected services, search, and resource access all need an explicit supported subset. See [WASM limitations and follow-up](archive/Wasm.md).

## 7. Performance and memory criteria

Performance is a reason to prefer this architecture and a criterion for its implementation. Native Rust can avoid unnecessary interpreter work for suitable operations and gives direct control over allocation lifetimes and resource bounds. In-process calls may remove HTTP and serialization layers from local execution. The benefit depends on where the work actually occurs and what crosses the frontend/backend boundary.

The UI still incurs WebView rendering, JavaScript garbage collection, and DOM costs. Tauri uses WebKitGTK on Linux, WKWebView on Apple platforms, WebView2 on Windows, and the system Android WebView. These differ in capability and performance; the Rust host's memory alone is not the application's memory footprint. A local node adds the `moqui` process (in configuration 1) and Postgres. [Tauri WebView versions](https://v2.tauri.app/reference/webview-versions/).

Use these implementation principles:

- Keep the client's startup small. Load editors, charts, diagrams, terminal support, crypto, and optional runtime pieces when required, preserving existing lazy-loading where possible.
- Avoid duplicating complete datasets in Rust, serialized IPC messages, and Vue stores. Page large lists, bound caches/queues, and batch appropriate updates. Use file/stream-oriented paths for large attachments where supported.
- Keep blocking database/framework work off the UI thread. Respect MR Moqui execution-context and thread ownership rules rather than placing all operations on an unbounded async executor.
- Avoid a second full crypto/runtime instance for the same local responsibility. If remote screens require a separate runtime for compatibility, measure that cost explicitly.
- Make cancellation, account switching, reconnects, and window closure release resources predictably. Track memory after repeated use, not only immediately after startup.

Establish release-build baselines before making numerical claims. Compare the same workload and data on named hardware/OS versions, in this order:

1. the web client against a local node;
2. the sidecar node (MD1);
3. the in-process node (MD2);
4. the offline-capable client (MD3–MD5).

Compare Capacitor on the same Android devices if that decision is active.

Record these measurements:

- cold and warm launch to a usable window, including Postgres for a local node;
- total memory of the app, node, Postgres and WebView together, at idle and after representative use;
- local query/action latency, and scroll/input responsiveness;
- IPC volume, and installed/download size;
- mobile wake-up and battery behavior.

Set budgets from those measurements. A smaller installer is useful but is not a memory or responsiveness measurement.

## 8. Tentative implementation plan

Milestones follow the build order of §1 and are ordered by dependencies, not fixed dates.

| Milestone | Scope | Exit evidence |
|---|---|---|
| **MD0 — Scope and baselines** | Supported OS versions; which package (Maximal or Enterprise) and components ship; the runtime layout in the app data directory and its upgrade; baseline workloads. Edge-Node's E12 (the all-in-one node) is built. | Agreed scope and budgets; a reproducible baseline of the web client against a local node. |
| **MD1 — Desktop local node, sidecar** | The Tauri app with the `moqui` sidecar and its runtime in the app data directory; start, readiness, and graceful stop; the window on the node's `/qapps` with its few commands; first run with the initial admin and the attach wizard; the desktop features of configuration 1; scheduled backups; signed packages and updates; Linux, then macOS. Pairing with an all-in-one node (Edge-Node's E12, built). | An installed app on a clean machine starts the node and pairs with an edge node, and with an all-in-one node. Mail sent while the computer sleeps arrives in order after it wakes. A call works with media on the edge, on Linux through the system browser. An app upgrade keeps the data. A backup restores. Startup and memory of node, Postgres and WebView together are recorded. |
| **MD2 — Full node in process** | Built: the node in process, with no port through the app scheme, its sockets bridged, its cookies kept by the host, optional ports, and PDF on CEF. Link `moqui-host` and the component crates into the app through a generated crate; boot in process; the app scheme to the in-process router; bridged sockets; the host signing the window in (as built, with the node's own session cookie, which it keeps); optional ports, as `moqui tui --serve`. | The MD1 checks pass with no `moqui` process and no port. An MD1 install upgrades in place. The secure-context, download and media checks are recorded for each WebView. Startup and memory are compared with MD1. |
| **MD3 — Bundled client of a remote server** | Built: the client mode, device keys, password sign-in, the proxy and sockets, the bundled shell, app windows, accounts; SSO with the deep-link handoff, the device list, the Android check. The qvt client bundled with a static shell bootstrap and one real BasicComms screen; server selection; device login keys; the app scheme proxy and the host-held `/notws`; remote applications in their own window; SSO with the deep-link handoff; the device list and revocation; shell/app discovery, BasicComms routing/extensions, and account switching. Preserve both qvt modes and HTML navigation where needed. Keep the client host crates building for Android. | The app opens its UI without a network, signs in, and shows server data. The shell runs under a CSP without `unsafe-eval`. Bundled and remote screens work against a real server; account switching, key expiry and revocation, links, downloads, and capability boundaries behave as specified. |
| **MD4 — Offline data and bundled Comms** | Built: device modes from `<client-conf>`; a SQLite store per account with migrations and a cap; the comms transitions answered offline from stored rows; pushes folded and a mirror; the host's outbox and drafts with a flusher, the chat outbox and drafts moved in; Compose idempotent by `clientRef` (`clientApi` 3); Chat, Mail, Compose and Calls bundled in the shell with offline and pending states; the shell owning the chat crypto device and the calls. Add durable client storage and migrations, and move the chat outbox into it. Package the selected chat/inbox/compose views from shared source. Provide local reads/drafts and explicit offline/pending presentation. Decide crypto-state ownership for this scope. | Previously selected content and drafts survive restart without a connection; outbox items queued before the move are kept; account/server separation holds; web and packaged clients still use the shared UI/contracts. A personal device keeps its data across sign-out and a shared one deletes it. |
| **MD5 — Reliable synchronization** | Add atomic outbox, idempotency/acknowledgements, incremental sync, deletions, conflicts, and rejected-operation handling. Implement necessary server changes. | Lost acknowledgements, disconnect/restart, duplicate events, revocation, and one bad independent item do not cause duplicate effects or block unrelated work. Encryption state survives the supported lifecycle. |
| **MD6 — Android app** | The MD3–MD5 client on Android through MT1–MT5 (below), then its distribution. | The MT2 call path works on a locked phone; offline reads, drafts and the outbox behave as on desktop; the host is selected from the recorded evidence. |
| **MD7 — Later** | iOS, repeating MT1 and MT2 with PushKit and CallKit. A selected local runtime in the client (§4): native host/storage adapters for a deliberately small component/service/screen subset, with authorization and verified transaction behavior. | iOS: as MD6. Local runtime: a selected real workflow runs after a disconnected restart from packaged definitions and local data; unsupported operations are explicit, and later synchronization passes server authorization. |

### Android track (MD6)

- **MT1.** Build the smallest Tauri Android app with the MD3–MD5 client host crates. Verify local shell startup, permissions, storage, deep links, and foreground/background transitions and process death.
- **MT2.** Exercise the highest-risk comms path on a locked Android phone:
  - An FCM high-priority message reaches native code.
  - Native code shows the incoming call through Android Telecom and starts it through the Rust call client, without the WebView, under a `phoneCall` foreground service.
  - Then check accepting/declining, Bluetooth and speaker routing, encrypted-call compatibility, and recovery after process death or a network change.
  - Record unsupported Android/WebView versions.
- **MT3.** Identify which gaps need a maintained existing plugin, a small Kotlin adapter, or a separate native media implementation. Evaluate plugin platform support and maintenance, not just whether a package with the right name exists.
- **MT4.** If those gaps favor Capacitor, reproduce the critical path with the same web assets and shared Rust/client contracts. Compare integration scope, lifecycle correctness, performance, and ongoing maintenance. Avoid porting the whole application merely to make this comparison.
- **MT5.** Select the mobile host from that evidence. Tauri desktop and the shared frontend/Rust work continue in either case. A successful foreground WebView demonstration alone is not the mobile acceptance gate.

### Validation and rollout

Use fast Rust unit tests for the host and the existing `moqui test`/`moqui test-client` facilities for shared logic and client contracts; Node is not a new test dependency. `moqui test-client` runs in Chromium, so it says nothing about WKWebView or WebKitGTK. Packaged smoke checks can use tauri-driver (WebDriver) on Linux and Windows; it has no macOS driver, so macOS and iOS need manual or XCUITest checks. Add focused boundary tests for sidecar lifecycle, account separation, command permissions, outbox idempotency, and lifecycle state handling. The Tauri build needs no Node; a Capacitor build would.

Use device and integration scenarios for the behavior they alone can establish. Follow [Build-Test](../docs/Build-Test.md#slow-tests): do not run slow tests or prolonged device/performance scenarios without the user's explicit permission for the needed scenario. This document does not authorize those runs.

Before mobile distribution, check the actual remote-application and local-code model against store requirements. Google Play distinguishes downloaded executable code from interpreted code while imposing requirements on both. [Google Play device and network abuse policy](https://support.google.com/googleplay/android-developer/answer/16559646). Apple's guidelines distinguish downloaded feature-changing code, minimum app functionality, and permitted HTML5/JavaScript software, and section 4.7 has additional conditions. [Apple App Review Guidelines, §§2.5.2, 4.2, 4.7](https://developer.apple.com/app-store/review/guidelines/). Validate this as a distribution decision. Do not assume that every remote screen is forbidden, or that any WebView container is automatically accepted.

## 9. Decisions to settle during implementation

**Local node (MD0–MD2), settled as built:**

- The runtime layout: the components and conf are links into the app's resources, so an upgrade replaces them with the binary; the data, `.env` and logs are kept in the app data directory.
- The node's secrets stay in the runtime's `.env` in the app data directory, as on a server.
- Close to the tray is the default, with no setting; autostart at login is off until the user turns it on. Scheduled jobs after a long sleep: a missed job runs once on the scheduler's next pass.
- Backups: nightly, with the framework's retention, to `MR-Moqui Backups` in the user's Documents or a folder chosen from the tray's Backups menu, which also has Back up now and Restore; a restore first takes a backup of the data it replaces (`-before-restore`).
- The generated crate of the in-process build is a `./moq` build target (`target/moqui-app`).

- The window of the node in process signs in with the node's own session cookie, which the host keeps in `node-cookies` (only the user can read it), not a login key: a key in a header makes a program's session, which may not decide an agent's approvals.
- The sidecar's Comms app: the node's pages report `calls.appRoot` (`set_comms_root`), kept in `comms-root`.
- PDF on the app's CEF for the node in process, where the node finds no Chromium; the bundle carries Chromium's notices.

- The descriptors a program hosting the node opens without close-on-exec (CEF's, a development run's DevTools listening socket among them) are marked close-on-exec before the embedded Postgres and the sidecar start (`close_fds`), so a postmaster left by an app that was killed holds none of them.

**Local node, open:**
- The all-in-one node's one open question ([Edge-Node §10](Edge-Node.md#10-all-in-one-node-e12)): a clustered server, which is Later. Its role is decided (`node_role=all-in-one`).
- Windows: the `moqui` binary and its process handling there. A restart the sidecar node asks for replaces the process in place only on Unix; elsewhere `moqui` starts a new process and the old one ends, so the host sees its child end while the new node is not its child (`run_again` in `moqui-host`).
- The upgrade path for a new Postgres major version in the app data directory.
- The backup and restore dialogs (`rfd`) need the desktop portal on Linux, or zenity or kdialog.

**Client (MD3), settled as built:**

- The client is a mode of the same app (`client`), on the crate `moqui-client`.
- Device keys: the REST login's `createLoginKey` with `deviceName` (90 days, `login-key/@device-expire-hours`), revoked by `logout` with `revokeLoginKey` or `revoke#UserDeviceKey`, renewed with less than a third of their life left (`renew#UserDeviceKey`, the old key ending ten minutes later); the shell's settings from `/qapps/clientConfig`, whose `server.clientApi` (3 since MD4) the shell compares with the app's.
- The key store on a Linux desktop with no Secret Service: a file only the user can read, which the shell says (and a development run's default).
- Packaged assets: the webroot's scripts and the libraries of `shell/libs.txt` as they are; the bundled screens precompiled from their own `.qvue` files (`shell/bundled-screens.txt`), checked by a golden test, so web and installed clients keep one source.
- App windows have the socket commands only.
- The SSO handoff: a confirmation in the browser before a code is given, a code good once for a minute with the PKCE verifier and redirect URI of its start, and a device key made with no session logged in or out.

**Client (MD4), settled as built:**

- The first offline release is chat and full mail: reading what was read or mirrored, drafts, and queued chat sends and reads, mail sends (with attachments) and thread state changes. Calls and anything else the server must do say they need a connection.
- Device modes are a build setting, `<client-conf device-mode>` (`personal`, `shared` or `choose`, where the sign-in asks), kept with the account: a build for a hot desk or a point of sale fixes `shared`, one for a person's own device `personal`.
- A personal account is never downgraded: signing in to it with shared chosen keeps it personal, with a notice, since trimming its mirror would lose what the user expects to keep.
- A shared account's data, keys and account entry are wiped on sign-out (after listing what is unsent) and on revocation, so the next person finds nothing; a personal account's are kept until it is removed, so a revoked or signed-out personal device still reads its history.
- The store is plain SQLite, owner-only, with no encryption of its own: the OS's disk encryption protects it at rest, and the keys are in the OS key store.
- Offline answers go through the same transitions the screens call, built from stored rows in the server's shape, so the web and the client keep one set of screens.
- The host owns and flushes the outbox, so sends go while the window is hidden and one flush serves the shell and the app windows of an account.
- The `localStorage` outbox and drafts move into the host with an item-by-item import, each removed from the browser only once the host has it, so nothing queued before the move is lost.
- The shell owns the chat crypto device; the host keeps its store key in the OS key store so cached ciphertext decrypts offline. There is no migration from an app-origin or browser device: the shell's is new, and its older room keys come from the key backup, since moving a store across origins risks two machines on one Olm state.
- The shell owns the calls, since it holds the device an encrypted call's keys need, and one owner per call keeps its media in one place; app windows ring silently and hand calls to it.
- Internal messages need a connection: their send has no `clientRef` idempotency yet, and a retry could send twice.

**Client (MD5), settled as built (server, stage 2):**

- The compatibility handshake stays `clientConfig`'s `server.clientApi`, now 4 (the changes feeds, `clientRef` on more mutations, `baseStamp` and `seen` conflicts, chunked uploads), which the shell compares with what it knows, asking for an update when the server is newer, and the store's `user_version`, which the client migrates.
- The mail cursor is per user, numbered when the user reads it, under a lock of the user's row, so its order is the order the changes committed and a writer never waits on it; a user with no client keeps no log. A change is the whole thread as it is when read, so a change read twice or out of date does no harm; deletions are the thread's removed messages. A cursor the kept log (30 days) no longer covers, or one read after the user's accounts changed, answers `reset`, and the client reads its folders again.
- One general mechanism for idempotency: a request's `clientRef` and its answer are kept in the request's transaction, and the same ref answers the kept answer with `duplicate`; a request that fails keeps nothing.
- Conflicts: a draft is the server's when it was saved since the client's copy (`baseStamp`, the time of its last content save), and the client keeps its own as a copy; a queued thread action is skipped when the state it saw changed, the server's state shown.
- Uploads go in chunks at strict offsets, answered with the bytes held, joined when complete and taken by `uploadId`; an upload is its user's only and expires.

**Client (MD5), open:**

- The client's use of the stage 1 and 2 contracts (stage 3); IronStone's inbox source answering `get#MailChanges` and its internal send by `clientRef`.
- The device-dependent Settings screens (My devices, the key backup, verification) in the shell; in app windows they refuse with `elsewhere`.

**Android (MD6), settled by the stage 5 spike:**

- The mobile app is `crates/moqui-mobile`, outside the workspace like the desktop app, on the desktop's Tauri (3.0.0-alpha.5) with the wry runtime, so the client glue can be shared; no fallback to Tauri 2 was needed. Its Gradle project, `gen/android`, is kept in git with the verifier's Maven artifact added; `./moq android` builds, runs and starts the emulator.

**Throughout:**

- The minimum OS/WebView matrix, especially for encrypted calls, and the acceptable native media work.
- The Linux call path: the system browser, a native audio client in Rust, or a Chromium runtime for Tauri.
- The measured latency/memory/startup budgets that define success, and which optional features stay unloaded by default.

The first implementation is one app on macOS and Linux, on CEF, with both node modes: the in-process node of MD2, with no port through the app scheme, and the sidecar of MD1, its runtime in the app data directory, edge pairing through the attach wizard, the page bridge, the tray, the OS integration, backups, PDF on CEF, and signing and updates from the build's environment. The client of a remote server (MD3) is built in the same app, with its offline data and bundled Comms (MD4). Next are the exit checks of MD1 to MD4 on installed apps ([Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md)), then MD5.
