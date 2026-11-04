# moqui-desktop

The desktop app: an MR Moqui node on this computer, shown in a [Tauri](https://tauri.app) 3 (alpha) window on Chromium
([CEF](https://bitbucket.org/chromiumembedded/cef), through `tauri-runtime-cef`) on every desktop OS. It is
configuration 1 (the `moqui` binary as a sidecar, MD1) and configuration 2 (the node in the app's process with no port,
MD2) in [Mobile-Desktop](../../plans/Mobile-Desktop.md), in one app with a switch at run time, or with no node a client of
a remote server (configuration 3, MD3 and MD4).

- The node in process has no port: the window reaches it through the app scheme `moqui-node`
  ([The node in process](#the-node-in-process)). The sidecar listens on 127.0.0.1 only, on a port kept in `node-port`
  in the app's config directory, so the origin, and the pages' local storage, stay the same from run to run. The
  HTTPS listener is off, and search runs on Postgres.
- The window shows a splash page, then the node's `/qapps`. Links to other sites, and `mailto:`, `tel:` and `sip:`
  links, open in the system browser or handler.
- Closing the window hides it to the tray. Quit (tray menu or ⌘Q) stops the node and its Postgres.

The `client` mode is configuration 3, a client of a remote server with no node here that keeps each account's data for
offline use ([Client of a server](#client-of-a-server)). The checks on installed apps that close MD1 to MD4 are in
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
- **PDF:** where the node finds no Chromium of its own (`moqui_chromium_path`, `runtime/chromium` or an installed one),
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
bundled shell, which opens with no network; each account is a server and a user, with a device key the host keeps. The
full description is [docs/ui/Desktop-Client.md](../../docs/ui/Desktop-Client.md).

- **Schemes:** `moqui-shell` serves the shell's files under a CSP without `unsafe-eval` and forwards the server's data,
  never its pages or scripts (`scheme.rs`, `assets.rs`); `moqui-app` forwards everything, for the app windows, which
  show the server's own `/qapps`. Each request carries the key in the `login_key` header.
- **Accounts and keys:** sign-in by password or SSO (`shell/screens/SignIn.qvue`), `client/accounts.json`, the OS key
  store or a file only the user can read, renewal (`renewal.rs`), the account menu's Apps and devices.
- **Device mode:** `personal`, `shared` or `choose` from `<client-conf>` (below), kept with the account (`modes.rs`): a
  shared account's data and entry are deleted on sign-out or revocation, a personal one's kept until it is removed.
- **Store and offline:** one SQLite store per account (`stores.rs`); the comms transitions' answers and rows kept and
  answered offline, marked `X-Moqui-Offline` (`offline.rs`); the chat pushes folded in (`push.rs`); the mirror reads
  ahead when the shell's socket opens (`mirror.rs`).
- **Outbox and drafts:** chat sends and reads, mail sends and state changes queued in the store and sent by the host on
  a thread of its own (`outbox.rs`, `flush.rs`), with WebrootChat's retry rules.
- **Bundled screens:** `shell/bundled-screens.txt` (the shell's own, and BasicComms' chat panel, Chat, Inbox, Compose,
  Calls/Call and Calls/Dialer), precompiled into `shell/js/bundled-screens.js` with the libraries of `libs.txt`; any
  other screen opens in an app window (`windows.rs`). After a change to one:
  `UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled`.
- **Crypto and calls:** the shell owns the account's chat crypto device and its calls; an app window makes no device
  (`elsewhere`) and opens encrypted rooms and calls in the shell (`open_in_shell`).

### client-conf

What the client keeps for offline use is set by the `<client-conf>` element of the bundled runtime's merged conf, read
without booting a node (`client/conf.rs`), so each build sets its own in its runtime conf; the server does not read it.
The defaults, in `MoquiDefaultConf.xml`:

```xml
<client-conf device-mode="choose" mirror-days="90" prefetch-rooms="20" prefetch-mail="100" store-max-mb="512"/>
```

The attributes are in `moqui-conf-3.xsd` and
[Desktop-Client](../../docs/ui/Desktop-Client.md#device-modes-and-client-conf).

## OS integration

`src/os/`, with tray items for the settings:

- **Single instance:** the first instance listens on a socket in the app config directory; a second launch hands it
  its links, which shows the window, and exits.
- **Start at login:** the tray's "Start at login", off until turned on; a LaunchAgent on macOS, an XDG autostart entry
  on Linux.
- **`mailto:`, `tel:`, `sip:`:** the package registers the app as a handler (`plugins.deep-link` in `tauri.conf.json`,
  `linux/main.desktop`). A mail link opens the Comms app's Compose filled in, a call link its Dialer, under
  `comms_app_root` (default `/qapps/BasicComms`); a client's under its server's (`calls.appRoot` of the client
  config), both in the shell. "Make default for email and calls" makes the app the default:
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
  nightly rotation, with the content database's `moqui_content-<stamp>-manual.dump` beside it.
- **Restore:** backs up the current data (`-before-restore`, and does not go on if that fails), stops the node, runs
  `moqui load-save` on the chosen `moqui-` dump (which restores the content database from the dump beside it; another
  database's dump is refused), and starts it again.
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
sockets, accounts, renewal, store and conf in `cargo test -p moqui-client`, the device key renewal on the server in
`cargo test -p moqui-core --lib a_device_`, and the bundled screens in
`cargo test -p moqui-core --lib bundled_screens_are_precompiled`; the bundled Comms screens in the shell, in the page,
with `moqui test-client basic-comms --offline` (`ShellComms`, `ShellCalls`).
