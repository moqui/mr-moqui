# moqui-desktop

The desktop app: an MR Moqui node in the app's own process, shown in a [Tauri](https://tauri.app) 3 (alpha) window on
Chromium ([CEF](https://bitbucket.org/chromiumembedded/cef), through `tauri-runtime-cef`) on every desktop OS. This is the
first stage of configuration 2 in [Mobile-Desktop](../../plans/Mobile-Desktop.md):

- The node boots on a thread of the app (`moqui_host::start_embedded`) and listens on 127.0.0.1 only. The HTTPS
  listener is off, the cluster listener is on loopback, and search runs on Postgres.
- The window shows a splash page, then the node's `/qapps`, as a browser session on the node's own origin.
- Links to other sites, and `mailto:`, `tel:` and `sip:` links, open in the system browser or handler.
- Closing the window hides it to the tray. Quit (tray menu or ⌘Q) stops the node and its Postgres.
- The port is kept in the app's config directory, so the origin, and the pages' local storage, stay the same from run
  to run.

The second stage, with no port, is not built: the window loads through an app scheme into the router, and the node
socket is bridged.

The crate is not a member of the repository's workspace, so Tauri and CEF stay out of the server builds. It depends on
`target/moqui-app`, the crate `./moq` generates to link the framework and the component crates.

## Run

The Tauri CLI comes from cargo, so no Node is needed:

```sh
cargo install tauri-cli --version 3.0.0-alpha.5 --locked
./moq desktop          # cargo tauri dev, on this repository's runtime (MOQUI_RUNTIME for another)
./moq desktop build    # the bundles under target/release/bundle
```

- **Prerequisites:** CMake and Ninja, which build CEF's C++ wrapper, and on Linux the GTK 4 development packages and
  `patchelf`; the list and the Linux bundling workarounds are in [Build-Test](../../docs/Build-Test.md#desktop-app).
- **CEF download:** the first build fetches the CEF binary distribution, about 1 GB, into the `tauri-cef` cache
  (`~/Library/Caches/tauri-cef` on macOS, `~/.cache/tauri-cef` on Linux). Set `CEF_PATH` to use another.
- **Driving the window:** in a development run, `MOQUI_DESKTOP_DEVTOOLS_PORT=9339 ./moq desktop` opens the Chromium
  DevTools protocol on that loopback port, for `chrome://inspect` or a CDP client. It takes effect from the second start
  after it is first set: the earlier start leaves Chromium's `devtools.remote_debugging.allowed` off in the profile, and
  the app turns it back on only once Chromium has started. A bundled app never opens it.
- **A server on the same runtime:** a development run refuses to start while a server answers on the runtime's own port
  (`webapp_http_port`, default 8080). Two nodes would share its Postgres.

The runtime is, in order: `MOQUI_RUNTIME`; in a development run, this repository's `runtime`; the first one with a
`base-component` directory of `runtime` in the working directory, `runtime` beside the executable (for an AppImage, the
`.AppImage` file) and the executable's own directory, as `moqui` finds one; else the bundled runtime. So
`MR-Moqui_0.1.0_amd64.AppImage` run from a checkout's root, or with `MOQUI_RUNTIME` set, runs on that runtime and its
database. As in a development run, the app refuses to start while a server answers on that runtime's port.

A bundled app keeps its runtime in the app data directory (`~/Library/Application Support/org.moqui.desktop/runtime` on
macOS, `~/.local/share/org.moqui.desktop/runtime` on Linux):

- The components and conf are links into the app's resources, so an upgrade replaces them.
- The database, `.env`, logs and the rest are written there and kept across upgrades.
- The Postgres socket is in `moqui-pg-<hash>` under the temp directory. The data directory's path has a space, and the
  embedded Postgres cannot take one for its socket (`socket_dir_for` in `crates/moqui-drivers/src/embedded.rs`).

Chromium's process sandbox: the `.deb` installs `chrome-sandbox` setuid root, so it has one. An AppImage cannot (its
image is mounted `nosuid`), and where AppArmor restricts unprivileged user namespaces (Ubuntu 23.10 and later),
tauri-runtime-cef starts it with `--no-sandbox`. `./moq desktop build` takes out the hook Tauri adds to offer an
AppArmor profile on every start ([Build-Test](../../docs/Build-Test.md#desktop-app)).

## Test

```sh
cd crates/moqui-desktop && CARGO_TARGET_DIR=../../target cargo test
```
