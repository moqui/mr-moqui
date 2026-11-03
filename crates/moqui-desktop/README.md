# moqui-desktop

The desktop app: an MR Moqui node on this computer, shown in a [Tauri](https://tauri.app) 3 (alpha) window on Chromium
([CEF](https://bitbucket.org/chromiumembedded/cef), through `tauri-runtime-cef`) on every desktop OS. It is
configuration 1 (the `moqui` binary as a sidecar, MD1) and the first stage of configuration 2 (the node in the app's
process, MD2) in [Mobile-Desktop](../../plans/Mobile-Desktop.md), in one app with a switch at run time.

- The node listens on 127.0.0.1 only, on a port kept in `node-port` in the app's config directory, so the origin, and
  the pages' local storage, stay the same from run to run. The HTTPS listener is off, the cluster listener is on
  loopback, and search runs on Postgres.
- The window shows a splash page, then the node's `/qapps`, as a browser session on the node's own origin. Links to
  other sites, and `mailto:`, `tel:` and `sip:` links, open in the system browser or handler.
- Closing the window hides it to the tray. Quit (tray menu or ⌘Q) stops the node and its Postgres.

The second stage of configuration 2, with no port, is not built: the window loads through an app scheme into the
router, and the node socket is bridged.

The crate is not a member of the repository's workspace, so Tauri and CEF stay out of the server builds. It depends on
`target/moqui-app`, the crate `./moq` generates to link the framework and the component crates.

## Node modes and lifecycle

`src/node/`: in process (`in_process.rs`, `moqui_host::start_embedded` on a thread of the app, the default) or the
sidecar (`sidecar.rs`). The mode is `MOQUI_DESKTOP_NODE` (`sidecar` or `in-process`), else the `node-mode` file in the
app's config directory, which the tray's node item writes before it restarts the app, else in process.

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

## The page bridge

`src/bridge.rs` and `src/bridge.js`: an initialization script defines `window.moquiDesktop` on the node's loopback
origin, and the webroot client calls it where present ([webroot js README](../../runtime/base-component/webroot/screen/webroot/js/README.md)).

- **Commands:** `notify` (a native notification; a click shows the window at its link), `set_counts` (the shell's
  counts on the tray and the dock or launcher badge), `set_presence` (the user's presence on the tray), `open_external`
  (http, https, mailto, tel and sip only, never the node).
- **ACL:** the commands are in the app manifest (`build.rs`), so each is checked; the only capability that grants them
  is the one added at run time for the node's exact origin on the main window. `restart_node` is granted only to the
  splash page (`capabilities/splash.json`).
- **Downloads:** a download link, an export or an attachment from the node is Chromium's download: the save dialog
  asks where, it streams to disk with the page's cookies, and a notification says when it is done. Other downloads are
  refused.

## OS integration

`src/os/`, with tray items for the settings:

- **Single instance:** the first instance listens on a socket in the app config directory; a second launch hands it
  its links, which shows the window, and exits.
- **Start at login:** the tray's "Start at login", off until turned on; a LaunchAgent on macOS, an XDG autostart entry
  on Linux.
- **`mailto:`, `tel:`, `sip:`:** the package registers the app as a handler (`plugins.deep-link` in `tauri.conf.json`,
  `linux/main.desktop`). A mail link opens the Comms app's Compose filled in, a call link its Dialer, under
  `comms_app_root` (default `/qapps/BasicComms`). "Make default for email and calls" makes the app the default:
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
- **A server on the same runtime:** the app refuses to start while a server answers on the runtime's own port
  (`webapp_http_port`, default 8080). Two nodes would share its Postgres.

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

The host side of the stdin-close stop and the Postgres takeover are tested in their crates:
`cargo test -p moqui-host -p moqui-drivers`.
