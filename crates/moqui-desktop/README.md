# moqui-desktop

The desktop app: an MR Moqui node on this computer, shown in a [Tauri](https://tauri.app) 3 (alpha) window on Chromium
([CEF](https://bitbucket.org/chromiumembedded/cef), through `tauri-runtime-cef`) on every desktop OS. It is
configuration 1 (the `moqui` binary as a sidecar, MD1) and configuration 2 (the node in the app's process with no port,
MD2) in [Mobile-Desktop](../../plans/Mobile-Desktop.md), in one app with a switch at run time, or with no node a client of
a remote server (configuration 3, MD3).

- The node in process has no port: the window reaches it through the app scheme `moqui-node`
  ([The node in process](#the-node-in-process)). The sidecar listens on 127.0.0.1 only, on a port kept in `node-port`
  in the app's config directory, so the origin, and the pages' local storage, stay the same from run to run. The
  HTTPS listener is off, and search runs on Postgres.
- The window shows a splash page, then the node's `/qapps`. Links to other sites, and `mailto:`, `tel:` and `sip:`
  links, open in the system browser or handler.
- Closing the window hides it to the tray. Quit (tray menu or ⌘Q) stops the node and its Postgres.

The `client` mode is configuration 3, a client of a remote server with no node here
([Client of a server](#client-of-a-server)). The checks on installed apps that close MD1 to MD3 are in
[Mobile-Desktop-Exit-Checks](../../plans/Mobile-Desktop-Exit-Checks.md).

The crate is not a member of the repository's workspace, so Tauri and CEF stay out of the server builds. It depends on
`target/moqui-app`, the crate `./moq` generates to link the framework and the component crates.

## Node modes and lifecycle

`src/node/`: in process (`in_process.rs`, `moqui_host::start_embedded_with` on a thread of the app, the default) or
the sidecar (`sidecar.rs`). The mode is `MOQUI_DESKTOP_NODE` (`sidecar`, `in-process` or `client`), else the `node-mode`
file in the app's config directory, which the tray's node and client items write before they restart the app, else in
process.

- **The sidecar binary:** `MOQUI_DESKTOP_MOQUI`; in a development run `target/release/moqui`, else
  `target/debug/moqui` (`./moq build`); in a bundle, `moqui` beside the app's executable (`bundle.externalBin`).
- **Start:** `MOQUI_RUNTIME` and the listen properties as `-D` arguments; started at its `Listening on` line once
  `/health` answers ready. Its output goes to the app's log. It runs in a process group of its own, so a ctrl-c to the
  app's group reaches only the app.
- **Stop:** the app holds the node's stdin open, and the node runs with `moqui_stop_on_stdin_close=true`
  (`crates/moqui-host/src/http.rs`), so closing it, or the app ending however it ends, stops the node gracefully.
  Then SIGTERM after 30 seconds, then a kill after 10 more.
- **Orphaned Postgres:** the process that starts a postmaster writes `moqui-owner.pid` beside `postmaster.pid`. A start
  that finds a live postmaster whose owner is gone takes it over and stops it on its own stop
  (`crates/moqui-drivers/src/embedded.rs`); one whose owner still runs (a server that `save-db` reaches) is only
  adopted.
- **Errors:** a start that fails, or a sidecar that ends by itself, shows the splash page with the reason (its last
  warnings and errors). The page's Restart button and the tray's Restart node start it again: a sidecar on the same
  port, a node in process with the app (the framework boots once per process). A restart the node asks for replaces
  the sidecar in place on Unix (same pid and pipes).

## The node in process

`src/node/scheme.rs` and `cookies.rs`: the node in process opens no HTTP or cluster port (`moqui_host::start_embedded_with`
with no ports); the window is on `http://moqui-node.localhost`, an app scheme whose handler serves each request through
the node's router in this process (`moqui_host::in_process`), on worker threads (`src/scheme.rs`).

- **Cookies:** the host keeps the node's session and visitor cookies for the window, in `node-cookies` in the app's
  config directory (only the user can read it): `Set-Cookie` is taken out of each answer and `Cookie` put into each
  request. The pages sign in with the node's own login page and keep a session as on the web (the CSRF token,
  `/termws`, a person's approvals), never see the cookie, and stay signed in across a restart while the node keeps the
  session.
- **Requests:** a page's headers go on but for what only the host may say (cookies, host, forwarded-for, the edge
  link's, the encodings). A request from another site is refused, but for a window's document `GET` (the splash page
  going to the node). A redirect to the node's loopback address stays on the scheme.
- **Sockets:** `/notws`, `/callws` and `/termws` of the node's pages are held by the host (`client/sockets.rs`, as a
  client's) and opened in process with the window's cookies; the terminal's binary frames cross the page bridge in
  base64.
- **Event streams** cannot pass a scheme that answers whole bodies: 503, so the Assist drawer polls its `events.json`.
- **Ports:** the tray's "Open ports for other devices" (`node-ports` in the config directory) restarts the node with its
  ports, as `moqui tui --serve` does, for a browser or another device; the window stays on the scheme.
- **PDF:** where the node finds no Chromium of its own (`pdf_chromium_path`, `runtime/chromium` or one on the `PATH`),
  it prints on the app's own CEF (`src/pdf.rs`): a hidden window with no capability, driven over the DevTools Protocol
  with the server's page sequence (`moqui_host::pdf::print_page`), its requests failed. The bundle carries Chromium's
  notices as `notices/chromium-credits.html`.

## The page bridge

`src/bridge.rs` and `src/bridge.js`: an initialization script defines `window.moquiDesktop` on the node's loopback
origin (and on a client's, [Client of a server](#client-of-a-server)), and the webroot client calls it where present ([webroot js README](../../runtime/base-component/webroot/screen/webroot/js/README.md)).

- **Commands:** `notify` (a native notification; a click shows the window at its link), `set_counts` (the shell's
  counts on the tray and the dock or launcher badge), `set_presence` (the user's presence on the tray), `open_external`
  (http, https, mailto, tel and sip only, never the node), `set_comms_root` (the node's Comms app, `calls.appRoot`,
  kept in `comms-root` for the `mailto:` and `tel:` links, which a sidecar's conf does not tell the host), and on the
  node in process the sockets.
- **ACL:** the commands are in the app manifest (`build.rs`), so each is checked; the only capability that grants them
  is the one added at run time for the node's exact origin on the main window (in a client, the shell's on it; the
  node in process's, whose scheme pages Tauri counts as local, names the main window). `restart_node` is granted only
  to the app's own pages (`capabilities/splash.json`), and refuses any page but the splash.
- **Camera and microphone:** granted to the app's own pages (the node's, a client's shell and app windows), where the OS
  asks once, and refused to any other page (`bridge::on_permission`).
- **Downloads:** a download link, an export or an attachment from the sidecar node is Chromium's download: the save
  dialog asks where, it streams to disk with the page's cookies, and a notification says when it is done. Other
  downloads are refused. From an app scheme (the node in process, a client), an attachment or a file over 16 MB a
  window opens is saved by the host instead (`src/download.rs`): the save dialog, the body streamed to a `.part` file
  renamed when whole, a notification, and the page left as it is; a scheme's answer is whole, so Chromium's download
  would hold it in memory.

## Client of a server

`src/client/` and `shell/`, on [moqui-client](../moqui-client/README.md): the `client` mode (`MOQUI_DESKTOP_NODE=client`,
or the tray's "Use as a client of a server", which writes `client` in `node-mode`) starts no node. The window shows the
bundled shell, which opens with no network; each account is a server and a user, with a device key the host keeps.

- **Schemes:** `moqui-shell` (`http://moqui-shell.localhost`) serves the shell's files (`shell/`, and the webroot's `js/`,
  `libs/` and `css/` of `shell/libs.txt`; a bundle's staged copy in `shell-assets/`) under a CSP without `unsafe-eval`
  (only `wasm-unsafe-eval`, for the crypto device's module), and forwards the server's data (its JSON, images, media and
  downloads; never its pages, frames or scripts, and nothing under its own paths, which are its files or a 404) to the
  current account's server; the host's own 502, the server not reached, is marked `X-Moqui-Offline: unreachable`; `moqui-app`
  (`http://moqui-app.localhost`) forwards everything, for the app windows (`remote-<n>`), which show the server's own
  `/qapps`. Each request carries the key in the `login_key` header; the pages hold no credential and send no cookie.
  How the schemes work around Chromium's handling of their HTML is in [Mobile-Desktop](../../plans/Mobile-Desktop.md#one-transport-seam).
- **Store:** each account's data is in its store (`moqui_client::store`, `client/data/<account>.db`), opened when
  first used (`client/stores.rs`), with the size cap of `<client-conf store-max-mb>`; the answers MD3 kept in
  `client/cache/` move into it once.
- **Offline:** the shell's `/qapps/clientConfig`, `/shellData` and `/shellCounts`, and the comms app's Chat
  (`chatData`, `rooms`, `roomView`, `room`, `timeline`), Mail (`inboxData`, `threads`, `thread`) and Compose
  (`composeData`) answers are kept when the server gives them (`client/offline.rs`, keyed less `_`,
  `moquiSessionToken` and `pageTraceId`), and answered, marked `X-Moqui-Offline`, when it cannot be reached. Timeline
  pages, a room view, the room list, a folder's thread pages (one store's cursor or mr-mantle's several) and a thread
  are built from the stored rows in the server's shape, so pages never asked for whole still answer and pushes show; a
  page in a gap (never fetched) needs the server (502), and `offline.gapBefore`/`gapAfter` say where the held stretch
  ends (`hasMore*` stay false there). The page's chat store passphrase (`/ChatKeys/storeKey`) is kept in the key store and
  answered offline to the shell only; a revoked device's is dropped.
- **Pushes and mirror:** the chat pushes on the account's `/notws` are written into the store (`client/push.rs`:
  edits, reactions and redactions folded into their message); a drop marks the timelines' heads stale. When the
  shell's `/notws` opens, `client/mirror.rs` reads ahead on a thread of its own: a personal device all rooms and mail
  folders back to `mirror-days`, a shared one the `prefetch-rooms` latest rooms and the inbox's first `prefetch-mail`
  threads; after a drop, each room's latest page again.
- **Outbox and drafts:** what a page sends offline is kept in the account's store and sent by the host
  (`client/outbox.rs`, `client/flush.rs`), on a thread of its own for every account signed in here, with WebrootChat's
  rules (no answer, 408, 429, 5xx again after 2, 5, 15, 60 s; 401 and 403 held until a sign-in or a renewed key; another
  4xx failed with the server's message; 50 waiting, 64 KB each): a chat message (`chat-send`, acknowledged by its answer
  and its pushed echo), a mark-read (`chat-read`), a compose (`mail-send`: the draft saved, its files uploaded, sent, each
  once by its `clientRef`) and a thread's read, flag, archive, move or trash (`mail-state`). A mark-read and a mail state
  change show in the stored rows at once. The pages call `moquiDesktop.outbox` (`add`, `list`, `retry`, `discard`, `on`
  for each change) and `moquiDesktop.drafts` (`get`, `list`, `put`, `remove`, `saveFile`), for the calling window's
  account; the shell's `outbox.unsent()` lists what a shared sign-out would delete. An internal message is not queued:
  its send is not taken once by a `clientRef` yet (MD5), so Compose keeps it as a draft until there is a connection.
  An operation held three times in a row fails, and a chat message with files fails after 20 hours. The drafts are
  capped at 500 and 16 MB of text, their and the sends' files at a quarter of `store-max-mb`.
- **Device mode:** `<client-conf device-mode>` in the runtime's merged conf (read without booting, `client/conf.rs`):
  `personal` or `shared` for every account, or `choose`, where the sign-in asks. The mode is kept with the account; one
  already personal stays so (the shell says why); one with none follows the build's mode. Sign-out or a revocation of
  a shared account deletes its data, chat store keys and entry, and the pages clear its scoped browser storage and
  IndexedDB (the shell as it loads, the next app window once); a personal one keeps them; removing an account always
  deletes them (`client/modes.rs`).
- **Renewal:** at start and every six hours, a key with less than a third of its life left is renewed
  (`client/renewal.rs`, `/qapps/renewClientDevice`), and its pages' sockets open again with the new one; the server
  ends the old key ten minutes later. The shell says to update the app when the server's `clientApi` is newer than it
  knows.
- **Sign-in:** the shell's Sign in (`shell/screens/SignIn.qvue`) has the host post the password once to the server's
  REST login with `createLoginKey` and a `deviceName`, with the second factor's code when it asks. Sign out revokes the
  key (`/rest/logout` with `revokeLoginKey`); a 401, or `/notws` closing for authentication, has the host check the key
  and, if it no longer signs in, forget it and show the sign-in.
- **Single sign-on:** the sign-in's "Single sign-on" lists the server's identity providers; one opens the system
  browser at the server's `/sso/appLogin`, where the user signs in and lets the app in, and the server hands the browser
  back to the app's own scheme, `org.moqui.desktop:/sso` (registered with the OS as the app's, beside `mailto:` and the
  others, and on Linux by an AppImage or development run when it starts a sign-in; not claimed by "Make default"; on
  macOS the app's bundle registers it, which is checked on the installed app only), with a code the app takes with its PKCE verifier for a device key. A hand-back
  the app is not waiting for is passed over.
- **Apps and devices:** the account menu lists the account's apps and devices signed in by a key of their own
  (`/qapps/clientDevices`), each to sign out; so does My Account's Account page in BOSS.
- **Key store:** the macOS Keychain or the Secret Service (`keyring`), else a file in `client/keys/` in the app's config
  directory that only the user can read, which the shell's account menu says. A development run uses the file unless
  `MOQUI_DESKTOP_KEY_STORE=os` (each build is a new unsigned binary the Keychain would ask about).
- **Sockets:** a page's `/notws` or `/callws` (`moquiDesktop.socket`, which `moqui.openSocket` uses) is held by the host
  with the key, and its frames are handed to the page.
- **Accounts:** the accounts are `client/accounts.json`; switching closes the other account's windows and sockets. The
  pages' browser storage of a server's data is under the account (`moquiDesktop.storageScope`, MoquiLib `scopedKey`).
- **Commands:** the main window (the shell) has the shell's commands, the page bridge's, the sockets, the outbox and the
  drafts; an app window has the sockets, the outbox and the drafts of its own account. Tauri counts the app's scheme pages as local, so the capabilities name the windows, and each window's
  navigation stays on its origin (the web opens in the system browser, a server page from the shell in an app window).
- **Downloads:** an attachment or a large file is saved by the host, streamed, as for the node in process (above); a
  small one is Chromium's. An event stream is answered 503, as on the node's scheme.
- **Bundled screens:** `shell/bundled-screens.txt` lists them (the shell's own, and BasicComms' chat panel, Chat, Inbox
  and Compose from their `.qvue` files, the web's own source); they are precompiled into `shell/js/bundled-screens.js`,
  checked by `cargo test -p moqui-core --lib bundled_screens_are_precompiled` (`UPDATE_GOLDEN=1` writes it again), with
  the libraries they load in `libs.txt`. The shell shows those Comms screens itself, under the account's comms app
  (`calls.appRoot`), and a link to them from the OS or a notification is kept until the shell takes it
  (`client/windows.rs` `open_page`); any other screen opens in an app window. An app that extends them (BOSS) keeps its
  transitions and `ext-action` items there.
- **Chat crypto:** the shell owns the account's crypto device ("MR Moqui desktop on <host>"); an app window makes and
  opens none (`window.__moquiCryptoInShell`), and an encrypted room there offers Open in Chat, which shows it in the
  shell for the window's account while it is current (`open_in_shell`).
- **Offline in the screens:** from `moqui.connection` (the host's `X-Moqui-Offline` answers), inert on the web: queued and
  failed sends, the Inbox's Outbox, "needs a connection", and the sign-out's list of what is unsent. Specs:
  `moqui test-client basic-comms --offline` (`ShellComms`).

## OS integration

`src/os/`, with tray items for the settings:

- **Single instance:** the first instance listens on a socket in the app config directory; a second launch hands it
  its links, which shows the window, and exits.
- **Start at login:** the tray's "Start at login", off until turned on; a LaunchAgent on macOS, an XDG autostart entry
  on Linux.
- **`mailto:`, `tel:`, `sip:`:** the package registers the app as a handler (`plugins.deep-link` in `tauri.conf.json`,
  `linux/main.desktop`). A mail link opens the Comms app's Compose filled in, a call link its Dialer, under
  `comms_app_root` (default `/qapps/BasicComms`); a client's under its server's (`calls.appRoot` of the client
  config): Compose in the shell, the Dialer in an app window. "Make default for email and calls" makes the app the default:
  LaunchServices on macOS (the installed app only), `xdg-mime default` on Linux.
- **First run:** once the initial admin is signed in on a node that is not paired and not an edge or all-in-one node,
  the window opens the attach wizard (`/qapps/system/EdgeNode/Pair`) once, kept by a marker file; the wizard can be
  left.
- **macOS:** `Info.plist` has the microphone and camera prompts, and `entitlements.plist` the entitlements calls need
  under the hardened runtime.

## Backups, restore and sleep

`src/backup.rs`, with a Backups menu in the tray: Back up now, choose and open the folder, Restore from backup, and the
last backup's time. The folder and whether the nightly backup runs are in `backups` in the app's config directory.

- **Nightly:** the node gets `embedded_db_backup_scheduled=true` and `embedded_db_backup_dir`, which run the
  framework's `backup_EmbeddedDatabase_daily` job into the folder, by default `MR-Moqui Backups` in Documents (a
  development run keeps `runtime/db/backup` and leaves the job's schedule as the repository's runtime has it). See [PostgresBackup](../../plans/archive/PostgresBackup.md).
- **Back up now:** a `pg_dump` of the live database (`moqui save-db`), named `moqui-<stamp>-manual.dump`, outside the
  nightly rotation.
- **Restore:** backs up the current data (`-before-restore`, and does not go on if that fails), stops the node, runs
  `moqui load-save`, and starts it again.
- **Sleep:** `src/wake.rs` compares the wall clock with the monotonic one; after a sleep it asks the node's `/health`
  again and logs the answer. A nightly backup missed in a sleep runs once soon after the wake.

The dialogs are `rfd`; on Linux they need the desktop portal (or zenity or kdialog).

## Signing and updates

`./moq desktop build` signs and adds updates only when the variables are set (`./moq help desktop`); without them the
bundles are unsigned and the app has no updater.

- **macOS signing:** `APPLE_SIGNING_IDENTITY`, or `APPLE_CERTIFICATE` and `APPLE_CERTIFICATE_PASSWORD` (a .p12). The
  bundler signs the app, the `moqui` sidecar and the CEF helpers.
- **Notarization:** `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID`, or `APPLE_API_KEY` and `APPLE_API_ISSUER` (and
  `APPLE_API_KEY_PATH`); the Tauri CLI also reads `APPLE_PROVIDER_SHORT_NAME`, for an Apple ID in more than one team.
- **Updates:** `MOQUI_DESKTOP_UPDATER_PUBKEY` (from `cargo tauri signer generate`) and `MOQUI_DESKTOP_UPDATER_ENDPOINT`
  (https URLs), with `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` to sign the artifacts. The
  tray's "Check for updates…" asks the feed; installing stops the node first, replaces the app and its sidecar, and
  restarts (`src/updates.rs`).

## Run

The Tauri CLI comes from cargo, so no Node is needed:

```sh
cargo install tauri-cli --version 3.0.0-alpha.5 --locked
./moq desktop                              # cargo tauri dev, on this repository's runtime (MOQUI_RUNTIME for another)
MOQUI_DESKTOP_NODE=sidecar ./moq desktop   # the node as target/release/moqui (./moq build first)
MOQUI_DESKTOP_NODE=client ./moq desktop    # a client of a remote server: sign in to one (./moq run for a local one)
./moq desktop build                        # the bundles under target/release/bundle
```

- **Prerequisites:** CMake and Ninja, which build CEF's C++ wrapper, and on Linux the GTK 4 development packages and
  `patchelf`; the list, the bundle's sidecar, signing and the Linux bundling workarounds are in
  [Build-Test](../../docs/Build-Test.md#desktop-app).
- **CEF download:** the first build fetches the CEF binary distribution, about 1 GB, into the `tauri-cef` cache
  (`~/Library/Caches/tauri-cef` on macOS, `~/.cache/tauri-cef` on Linux). Set `CEF_PATH` to use another.
- **Driving the window:** in a development run, `MOQUI_DESKTOP_DEVTOOLS_PORT=9339 ./moq desktop` opens the Chromium
  DevTools protocol on that loopback port, for `chrome://inspect` or a CDP client. It takes effect from the second start
  after it is first set: the earlier start leaves Chromium's `devtools.remote_debugging.allowed` off in the profile, and
  the app turns it back on only once Chromium has started. A bundled app never opens it.
- **A server on the same runtime:** a node's mode refuses to start while a server answers on the runtime's own port
  (`webapp_http_port`, default 8080). Two nodes would share its Postgres. The client mode has no runtime.

The runtime is, in order: `MOQUI_RUNTIME`; in a development run, this repository's `runtime`; the first one with a
`base-component` directory of `runtime` in the working directory, `runtime` beside the executable (for an AppImage, the
`.AppImage` file) and the executable's own directory, as `moqui` finds one; else the bundled runtime. So
`MR-Moqui_0.1.0_amd64.AppImage` run from a checkout's root, or with `MOQUI_RUNTIME` set, runs on that runtime and its
database.

A bundled app keeps its runtime in the app data directory (`~/Library/Application Support/org.moqui.desktop/runtime` on
macOS, `~/.local/share/org.moqui.desktop/runtime` on Linux):

- The components and conf are links into the app's resources, so an upgrade replaces them.
- The database, `.env` (with the node's generated secrets), logs and the rest are written there and kept across
  upgrades.
- The Postgres socket is in `moqui-pg-<hash>` under the temp directory. The data directory's path has a space, and the
  embedded Postgres cannot take one for its socket (`socket_dir_for` in `crates/moqui-drivers/src/embedded.rs`).

Chromium's process sandbox: the `.deb` installs `chrome-sandbox` setuid root, so it has one. An AppImage cannot (its
image is mounted `nosuid`), and where AppArmor restricts unprivileged user namespaces (Ubuntu 23.10 and later),
tauri-runtime-cef starts it with `--no-sandbox` ([Build-Test](../../docs/Build-Test.md#desktop-app)).

## Test

```sh
cd crates/moqui-desktop && CARGO_TARGET_DIR=../../target cargo test
```

The host side of the stdin-close stop, the Postgres takeover, the node in process (`in_process`) and the shared PDF
page sequence (`pdf`) are tested in their crates: `cargo test -p moqui-host -p moqui-drivers`; the client's proxy,
sockets, accounts and renewal in `cargo test -p moqui-client`, the device key renewal on the server in
`cargo test -p moqui-core --lib a_device_`, and the bundled screens in
`cargo test -p moqui-core --lib bundled_screens_are_precompiled`.
