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

- **Prerequisites:** CMake and Ninja, which build CEF's C++ wrapper (`brew install ninja`, or the `ninja-build` package),
  and on Linux the GTK 4 development packages.
- **CEF download:** the first build fetches the CEF binary distribution, about 1 GB, into the `tauri-cef` cache
  (`~/Library/Caches/tauri-cef` on macOS, `~/.cache/tauri-cef` on Linux). Set `CEF_PATH` to use another.
- **Driving the window:** in a development run, `MOQUI_DESKTOP_DEVTOOLS_PORT=9339 ./moq desktop` opens the Chromium
  DevTools protocol on that loopback port, for `chrome://inspect` or a CDP client. It takes effect from the second start
  after it is first set: the earlier start leaves Chromium's `devtools.remote_debugging.allowed` off in the profile, and
  the app turns it back on only once Chromium has started. A bundled app never opens it.
- **A server on the same runtime:** a development run refuses to start while a server answers on the runtime's own port
  (`webapp_http_port`, default 8080). Two nodes would share its Postgres.

A bundled app keeps its runtime in the app data directory (`~/Library/Application Support/org.moqui.desktop/runtime` on
macOS):

- The components and conf are links into the app's resources, so an upgrade replaces them.
- The database, `.env`, logs and the rest are written there and kept across upgrades.
- The Postgres socket is in `moqui-pg-<hash>` under the temp directory. The data directory's path has a space, and the
  embedded Postgres cannot take one for its socket (`socket_dir_for` in `crates/moqui-drivers/src/embedded.rs`).

## Test

```sh
cd crates/moqui-desktop && CARGO_TARGET_DIR=../../target cargo test
```
