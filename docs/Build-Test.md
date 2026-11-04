# Build and Test

How to build MR Moqui from source, the build packages and their features, the `./moq` tasks, the Rust tests and lints,
and the component specs that `moqui test` runs. Running and deploying are in [Run-Deploy](Run-Deploy.md), and the specs
that test a running server from outside are in [Client-Testing](dev/Client-Testing.md).

- [Build](#build)
- [Packages](#packages): [features](#features), [profiles](#profiles), the [embedded Postgres
  archive](#embedded-postgres-archive), the [instance archive](#instance-archive), [build cost](#build-cost), the [compile cache](#compile-cache), the
  [desktop app](#desktop-app), the [Android app](#android-app)
- [The moq script](#the-moq-script)
- [Rust tests and lints](#rust-tests-and-lints)
- [Component specs](#component-specs): [running](#running), [a spec file](#a-spec-file),
  [conditions](#conditions), [helpers](#helpers), [mail](#mail), [HTTP fixtures](#http-fixtures),
  [notifications](#notifications), [agent evaluation](#agent-evaluation), [porting a Spock spec](#porting-a-spock-spec),
  [pitfalls](#pitfalls)

## Build

Prerequisites: Rust 1.98 or newer (`rustc --version`). A build with embedded Postgres embeds the 18.6.0 archive that
`scripts/pg-payload.sh strip` downloads once and strips of psql (it links GPL readline); `./moq build` runs that when
the archive is missing. Node is not needed to build or test.

```sh
./moq build                            # Maximal with the dev tools, target/release/moqui; --lite, --debug or
                                       # --enterprise for the others; links any component crates in runtime/component
scripts/pg-payload.sh strip            # once: the Postgres archive the binary embeds, without psql
cargo build -p moqui                   # Debug, every feature: target/debug/moqui
cargo build --profile lite --no-default-features -F lite -p moqui     # Lite: target/lite/moqui
cargo build --release -p moqui         # Maximal with the dev tools: target/release/moqui
scripts/pg-payload.sh check            # before a distribution: no psql or readline in what is embedded
```

Tasks that run moqui build a missing binary themselves. `./moq build` also links the crates of native components
([Native-Components](dev/Native-Components.md)), which plain `cargo build` does not.

## Packages

A moqui binary is built as one of five packages, plus the browser build. They differ in the Cargo features they turn
on (what the binary can do), the Cargo profile they build with (how it is compiled), and where the binary lands.
`./moq build` picks one with a flag; every task that runs moqui (`run`, `load`, `test`, `save-db`, ...) takes the same
flag and uses that binary. `moqui version` prints a binary's package and features.

| Package | `./moq` flag | Profile | Binary | What it has |
|---|---|---|---|---|
| Lite | `--lite` | `lite` | `target/lite/moqui` | The web server on embedded Postgres, PDF (through Chromium), outbound mail (SMTP), the terminal client (`moqui tui`) |
| Debug | `--debug` | `dev` | `target/debug/moqui` | Every feature and the dev tools, unoptimized framework code with debug info |
| Maximal | (none) or `--release` | `release` | `target/release/moqui` | Every feature, Postgres embedded, and the dev tools (`devtools`) |
| Enterprise | `--enterprise` | `enterprise` | `target/enterprise/moqui` | Every feature, with the databases external (Postgres and MySQL drivers, no embedded Postgres) |
| Browser | `scripts/wasm-build.sh [--release]` | `dev` / `release` | `target/browser/wasm-pkg/<profile>` | The framework on SQLite in a Web Worker; none of the optional features |

Plain `cargo build -p moqui`, `cargo test` and `cargo lint` use the Maximal feature set with `devtools`, so they cover
everything.

Maximal is the default, with the dev tools: development wants git, SSH and the dev-aide, and one default build with
everything is simpler than a smaller one that saves some build time. The dev tools act only on a dev instance
([design-dev-aide](../plans/design-dev-aide.md) 3.3); a deployment that wants none builds Lite or Enterprise.
`./moq build --lite -- --features devtools` adds them to Lite. (A Dev package, Lite with git, SSH and the dev tools on
a `develop` profile, was the default before; it is commented out in `moq`, `.cargo/config.toml` and the crates'
features, and likely to be removed.)

### Features

The framework features are in `crates/moqui-core`; the host features, and the packages, are in `crates/moqui-host`;
`crates/moqui` (and the crate `./moq build` generates in `target/moqui-app` for add-on component crates) forwards the
package features `lite`, `enterprise` and `maximal`, `devtools`, and `inbound-smtp` for a lite build that wants
one. The agents are the `moqui-llm` crate, on `moqui-core`, which the host's `llm` feature links; core reaches them
through `moqui_core::llm_hooks`, so in a binary without them their services report that `llm` is not built in. The
framework crates above core ([Crate-Split](../plans/Crate-Split.md): `moqui-git`, `moqui-mail`) are linked the same
way by a host feature (`git`; `smtp`, with `imap` turning on the mail crate's own), and core reaches them through
`moqui_core::extension`; `moqui-report`, `moqui-collab`, `moqui-dav` and `moqui-devkit` are four more, linked in
every host build (the host's `xlsx` turns on the report crate's own, for the xlsx format of a scheduled report, and its
`devtools` the dev kit's `watch`, the file watcher), and the browser build links the reports and `moqui-dav` (the VCard
and ICalendar tools, elFinder) but not co-editing or the dev kit. The entity facade is the `moqui-entity` crate below core, in every package and in the browser
build, with no features of its own.

| Feature | Crate | What it gates | Lite | Enterprise | Maximal |
|---|---|---|---|---|---|
| `axum` | host | The HTTP server, its listeners and routes; HTTP/2 by ALPN on the HTTPS listener; the cluster port | yes | yes | yes |
| `postgres` | host (`moqui-drivers`) | The Postgres driver | yes | yes | yes |
| `embedded-postgres` | host (`moqui-drivers`) | The embedded Postgres clusters (entity, search, sim), `save-db`, `load-save`, backups, and `moqui test` when it opens a database (`--db fresh`, `snapshot`, or `existing`). `--db none` does not need it | yes | | yes |
| `mysql` | host (`moqui-drivers`) | The MySQL driver | | yes | yes |
| `pdf` | host | The DevTools client of the external Chromium that prints `html-print` to PDF | yes | yes | yes |
| `ssh` | host | The SFTP client, the built-in SSH server (SFTP, admin shell, git over SSH, and `apps` with `tui`), the SSH cluster channel | | yes | yes |
| `sso` | host | OIDC and OAuth 2 logins (`org.moqui.sso.AuthServices`) | | yes | yes |
| `print` | host | Network printing over IPP (`moqui-cups`) | | yes | yes |
| `tui` | host | `moqui tui`, the terminal client of the `tui` render mode, on Revue ([TUI](ui/TUI.md)), and with `ssh` the SSH `apps` command; in lite too, for a quick terminal loop while developing and testing. Alone (`cargo build -p moqui-host --no-default-features --features tui`) it is the client of a running server, `moqui tui --server <url>`, with no database driver or HTTP server in the build | yes | yes | yes |
| `test-client` | host | `moqui test-client`: a component's `test/client` specs against a running server, over HTTP, raw TCP or TLS, WebSocket and headless Chromium, with a load runner ([Client-Testing](dev/Client-Testing.md)); it opens no database and binds no listener of its own, so any package can test any server. Its `--serve` starts `moqui test --serve`, which needs `embedded-postgres` | yes | yes | yes |
| `inbound-smtp` | host | The SMTP listener for hosted and shared mail accounts behind the edge gateway ([Mail-Inbound](interfaces/Mail-Inbound.md); includes `imap`); it starts only when `smtp_listen_port` is set | | yes | yes |
| `rtc` | host (`moqui-rtc`) | Calls: the media socket and its SFU on str0m, and the call signaling on `/notws` ([WebRTC](interfaces/WebRTC.md)); the media socket starts only when `rtc_media_port` is set, and the call services run in every build (without it they refuse with `mediaUnavailable`). In Debug too | | yes | yes |
| `speech-local` | host | Local speech to text for call captions (Whisper on candle, [WebRTC](interfaces/WebRTC.md#captions)): not built yet, so off in every package; with or without it `call_captions_provider=local` answers "not built in". Captions go through the `llm` feature's `transcribe` model meanwhile | | | |
| `sip` | host (`moqui-sip`; includes `rtc`) | Phone calls over SIP trunks: the SIP engine on rsipstack, its trunk registration and the RTP of phone legs, joined to the call media ([SIP](interfaces/SIP.md)); it starts only when `sip_port` or `sip_tls_port` is set, and the voice services run in every build (without it `get#VoiceCapabilities` says not supported). In Debug too | | yes | yes |
| `channels` | host (`moqui-channels`) | Business messaging channels: the `sms` chat bridge adapter, SMS and MMS on the Telnyx Messaging API v2 ([SMS](interfaces/SMS.md)). The `/channels/` webhook and media routes are in every build with `axum` and answer only when `channels_webhooks_enabled` is true; without this feature no adapter answers them and an SMS bridge stays not connected. | | yes | yes |
| `xmpp` | host (`moqui-xmpp`; includes `axum`) | XMPP for local users: client streams with STARTTLS, direct TLS and WebSocket, SASL2 with SCRAM-SHA-256-PLUS and FAST, rooms as MUC, history, PEP and uploads over Moqui chat ([XMPP](interfaces/XMPP.md)); it starts only when `xmpp_domain` is set. In Debug too | | yes | yes |
| `grpc` | host (includes `telemetry`, `axum`) | gRPC on tonic: the cluster's protocol on the cluster port (`cluster_protocol`, [Cluster](../plans/archive/Cluster.md)); the host's gRPC listener: the OTLP/gRPC receiver, opened only when `<telemetry><receiver grpc-enabled="true">` is set, and with `edge` the direct PASS authority ([Edge-Node](../plans/Edge-Node.md) §5.5) | yes | yes | yes |
| `edge` | host (includes `grpc`) | The edge link ([Edge-Node](../plans/Edge-Node.md)): a local node's dialer when `edge_node_url` is set, and on an edge node (`node_role=edge`) the link's hand-off on the HTTPS listener, web access, delegated calls, the queue and replication; the PASS authority over gRPC (direct when `pass_authority_grpc_enabled=true`, and on the link) and scripts' `ec.service.passAuthority` | | yes | yes |
| `devtools` | host, moqui-devkit (`watch`) | What only development needs ([design-dev-aide](../plans/design-dev-aide.md) 3.3): the framework's file watcher that reloads changed artifacts on a dev instance, and in the binary crates the dev-component crates of `runtime/dev-component`; in Maximal (the default) and Debug | | | yes |
| `smtp` | host (`moqui-mail`) | Outbound mail: `send#EmailMessage`, `send#EmailTemplate`, password reset and notification mail | yes | yes | yes |
| `imap` | host (`moqui-mail`) | Inbound mail: `poll#EmailServer`, the mail client's sync, IDLE, rules and import (includes `smtp`) | | yes | yes |
| `git` | host (`moqui-git`) | `git://` locations, git drives, `org.moqui.impl.GitServices`, the git server over HTTP and SSH, LFS | | yes | yes |
| `image` | core | `org.moqui.impl.ImageServices` (`convert#Image`) and elFinder thumbnails | | yes | yes |
| `xlsx` | core, moqui-report | The `xlsx` render mode, `org.moqui.impl.SpreadsheetServices` and the xlsx format of a scheduled report | | yes | yes |
| `llm` | host | The agents and the model providers (the `moqui-llm` crate, `org.moqui.agent.AgentAdminServices`; [LLM-A2A-Assist](../plans/LLM-A2A-Assist.md)) | yes | yes | yes |
| `http`, `cluster-tls` | core | Remote REST, `http` locations, the OpenSearch client; the cluster certificate | yes | yes | yes |

`mail` is `smtp` and `imap` together. The browser build has none of the core features, and no framework crate.

#### When a feature is left out

The definitions still load, so components, screens and service files are the same in every package:

- A service whose Rust implementation belongs to a missing feature fails when called, with
  `service <name>: <feature> is not built into this host` in the message facade (`rust_services::missing_feature`).
- The host integrations report through the framework's existing messages: SFTP and SSO calls say no client is
  installed, network printing says it is not installed, and `cluster_transport=ssh` cannot start without the SSH
  server. A cluster peer that advertises `ssh://` gets an error naming the missing feature. Without `grpc` the
  cluster port speaks only HTTP/1.1, which every member still takes, and `cluster_protocol=grpc` fails at startup.
- An `xlsx` render fails with `xlsx is not built into this host`; an elFinder thumbnail is left out; a `git://`
  location has no provider, and `/git/...` is not routed.
- gitECA rules are still read and checked; nothing runs them, since there are no git events.
- Without `edge`, the `org.moqui.impl.EdgeServices` that need the link (pairing, clients, hosts, ping) fail with
  `edge link not built into this host` (`get#LinkStatus` answers state `notBuilt`), and `ec.service.passAuthority`
  throws `gRPC not built into this host`, so mr-pass takes REST.
- Without `embedded-postgres`, `entity_ds_db_conf` must name an external `postgres` or `mysql` database, the sim
  overlay is not available, and `save-db`, `load-save`, and `moqui test --db fresh` (which makes scratch embedded
  databases) report that this build has no embedded Postgres. `moqui test --db none` still runs specs that set
  `database: false`.

A spec file that needs a feature says so, and `moqui test` reports it as skipped in a binary without it:

```js
spec({
    requires: ["git"],
    ...
})
```

### Profiles

The profiles are in `.cargo/config.toml`, so the generated `target/moqui-app` crate builds the same way. Each package
has its own, so the artifacts of one never replace another's and switching packages does not rebuild.

- `dev` (Debug, and every `cargo test`): the workspace crates at opt-level 0 with debug info, the dependencies at
  opt-level 3 with line tables only (file and line in a backtrace). The debug info stays in the object files under
  `target/debug` (`split-debuginfo = "unpacked"`), where debuggers read it, instead of being copied into each binary:
  a host test binary was 1.5 GB, 83% of it debug info.
- `lite` (Lite): `release` with `incremental = true` and 256 codegen units for the workspace and path crates
  (Cargo applies `incremental` only to those), so a rebuild after an edit to the framework is quick; the dependencies
  keep the release settings and are built once.
- `release` (Maximal) and `enterprise`: `panic = "unwind"` (services unwind across the service boundary), otherwise
  Cargo's release defaults.

### Embedded Postgres archive

The `embedded-postgres` feature embeds the theseus Postgres archive (the version `.cargo/config.toml` forces through
`POSTGRESQL_VERSION`) and unpacks it once into `install/postgres/<version>` in the runtime directory (or under
`MOQUI_INSTALL`), shared by the entity, search and sim clusters. An install an earlier build unpacked in
`runtime/db/install` is moved there on the first start. `crates/moqui-drivers/build.rs` stages it from `~/.theseus/postgresql/postgresql-<version>-<target>.tar.gz`, or
the file `MOQUI_PG_ARCHIVE` names, and downloads nothing. It refuses an archive that still holds `bin/psql` or a
readline library (GPL, plans/archive/License-Review.md).

`scripts/pg-payload.sh strip` downloads the archive when it is missing (checking its sha256) and repacks it without
psql; `./moq build` runs it the first time. A plain `cargo build` of a package with embedded Postgres asks for it.
`scripts/pg-payload.sh check` checks an archive, and every one staged under `target/`, for psql and readline.

### Instance archive

`./moq archive [file]` (with the package flags and `--nobuild`) zips what one instance needs to run into `file`
(default `mr-moqui-YYYYMMDD.zip`; `--tgz`, or a `.tgz` or `.tar.gz` name, writes a gzipped tar), which expands to a
directory of the same name. That directory is a runtime directory with the binary in it:

```
mr-moqui-20261019/
  moqui            the package's binary
  MoquiConf.xml    runtime/conf/MoquiProductionConf.xml, or the --conf file
  README.txt       how to run it, the package, the commit, and what went in
  LICENSE.md
  .env, base-component/, component/, conf/, db/, opensearch/, ...
                   runtime/ without log/, tmp/, save/ (--with-save adds it), install/, db/install, .git,
                   component target/
  install/         not in the archive: the first start unpacks the embedded Postgres into install/postgres/
```

The binary takes the first directory with `base-component/` in it of: `runtime` in the working directory, `runtime`
beside the binary, and the binary's own directory (links resolved), so `<dir>/moqui` runs from anywhere. With no
`MOQUI_CONF`, it merges `<runtime>/MoquiConf.xml`, else `<runtime>/conf/MoquiDevConf.xml`. `--tgz` needs GNU or BSD tar.
The embedded database, OpenSearch and Chromium go in when they are in `runtime/`, so the embedded Postgres must be
stopped; `--no-data` leaves out the database, OpenSearch data, `.env` and `cert/` for a fresh instance. A binary runs
on the OS and CPU it was built for.

### Build cost

Measured on Oct 19, 2026, on one 12-thread Linux machine, Rust 1.99: a clean build of each package in its own target
directory, the binary, and two run-time checks. Before the packages there was one feature set, every build carrying
everything, and postgresql_embedded's `bundled` build script compiled a second HTTP and TLS stack.

| Build | Clean build (wall) | Compile CPU | Crates | Binary | Stripped | `.text` |
|---|---|---|---|---|---|---|
| Before (release, everything) | 171 s | 1394 s | 623 | 84 MB | 67.5 MB | 38.1 MiB |
| Maximal (`release`) | 151 s | 1091 s | 545 | 82.9 MB | 66.8 MB | 37.7 MiB |
| Enterprise | 141 s | 971 s | 522 | 68.4 MB | 53.1 MB | 36.4 MiB |
| Lite (`lite`, then Default) | 113 s | 1054 s | 397 | 64.6 MB | 53.1 MB | 28.2 MiB |
| Lite without `tui` | 105 s | 901 s | 375 | 58.4 MB | 48.4 MB | 25.2 MiB |

The two Lite rows (Lite was the Default package then) were measured on Oct 20, 2026, on the same machine, at commit 5a18e9c, after the terminal client
joined `lite`; the other rows are Oct 19's, on the code of that day, and were not measured again. The terminal client
(Revue, crossterm and their dependencies, 22 crates) adds 8 s to a clean build, 153 s of compile CPU, 6.2 MB to the
binary and 3.0 MiB of `.text`. Lite without it is `cargo build --profile lite --no-default-features -F
moqui-host/axum,moqui-host/embedded-postgres,moqui-host/pdf,moqui-host/smtp -p moqui`, for comparison only; it is not a
package. Without it, Lite still takes 293 s more compile CPU and 21 more crates than Oct 19's row; that came with
the code and dependencies added since, and is not broken down here.

The embedded Postgres archive is 11.9 MB of each binary that has it.

| Build | Rebuild after a one-line edit in moqui-core | Load seed, seed-initial, install (8141 records) |
|---|---|---|
| Maximal (`release`) | 60 s | 8.4 to 10.0 s |
| Default (`lite`, opt-level 3) | 11.3 s (Oct 20, with the terminal client; 11.6 s on Oct 19) | 8.6 s |
| Default at opt-level 2 / 1 | 11.6 s / 10.7 s | 8.3 s / 9.1 s |
| Debug (`dev`) | 8.8 s | 18.1 s |

The Default profile keeps opt-level 3: a lower one saves no rebuild time. Thin LTO on the release profile made the
binary larger (84.1 MB, 67.6 MB stripped) for a seed load of 8.0 to 8.3 s, within the noise of a load that waits on the
database, so it is off. Stripping symbols saves about 16 MB a binary; it is not done, so a backtrace in a log keeps its
function names.

What a clean build still spends: aws-lc-sys (the TLS crypto every package links) is the first link of the longest
chain, then rustls, reqwest and moqui-core itself; QuickJS is compiled twice because rquickjs's `macro` proc-macro links
its own copy; postgresql_embedded's build script still compiles reqwest and tokio (without TLS) though it no longer
uses them. `cargo build --timings` writes the chart to `target/cargo-timings/`.

### Compile cache

Each worktree of this repo has its own `target/`, so without a shared cache every worktree compiles the same
dependencies (aws-lc-sys, oxc, tokio, ...) again. [sccache](https://github.com/mozilla/sccache) shares them. It is
opt-in: with `MOQ_SCCACHE=1` in the environment (in `~/.bashrc`, say), the cargo runs of `./moq` (build, desktop,
test-all, and the build before a task that runs moqui) use sccache as the rustc wrapper, so a dependency compiled in
one worktree comes from the cache in the next. Without it, `./moq` leaves the wrapper alone, as before. A crate
compiled incrementally (the workspace and path crates in the `dev` and `lite` profiles) goes straight to rustc, so
incremental rebuilds after an edit work as before; profiles and `CARGO_INCREMENTAL` are not changed.

- With `MOQ_SCCACHE=1` and no sccache on the PATH, the build goes on without it, and `./moq` warns once with how to
  install it: `cargo binstall sccache`, `cargo install sccache --locked`, the distribution package (`brew`, `dnf` or
  `apt install sccache`), or a release binary from [github.com/mozilla/sccache](https://github.com/mozilla/sccache/releases).
- A wrapper already chosen is kept: `RUSTC_WRAPPER` or `CARGO_BUILD_RUSTC_WRAPPER` set (even to empty, which turns
  any wrapper off), or `rustc-wrapper` in a cargo config (`~/.cargo/config.toml` or a `.cargo/config.toml` at or
  above the repo).
- The cache directory and its size are sccache's own settings, such as `SCCACHE_DIR` and `SCCACHE_CACHE_SIZE`
  (default 10G); `sccache --show-stats` shows the hits.

The repo's `.cargo/config.toml` does not set `rustc-wrapper`: cargo fails at once when that binary is missing. To use
sccache outside `./moq` too, set `RUSTC_WRAPPER=sccache` in the shell.

### Desktop app

`./moq desktop` runs, and `./moq desktop build` bundles, the desktop app in `crates/moqui-desktop`: a node, or a client of a
remote server, in a Tauri 3 (alpha) window on Chromium (CEF). It is not a workspace member, so the server builds need none of this. What it is and
how it runs are in its [README](../crates/moqui-desktop/README.md) and [Mobile-Desktop](../plans/Mobile-Desktop.md).

Prerequisites, beyond those of the server build:

- **Tauri CLI**, the version `./moq desktop` checks for (`TAURI_CLI_VERSION` in `moq`), from cargo, so no Node:
  `cargo install tauri-cli --version 3.0.0-alpha.5 --locked`.
- **CMake, Ninja and a C++ compiler**, which build CEF's C++ wrapper (cef-dll-sys uses the Ninja generator).
- **Linux:** the GTK 4 development packages and pkg-config (gtk4-sys); `patchelf` for the AppImage; and `cc`, which
  quick-sharun and `./moq` use while bundling.
- **Network on the first build and for each AppImage:** the first build downloads the CEF binary distribution, about
  1 GB, into the `tauri-cef` cache (`~/.cache/tauri-cef` on Linux, `~/Library/Caches/tauri-cef` on macOS; `CEF_PATH`
  for another). The AppImage step downloads quick-sharun.sh, sharun and appimagetool each time.

```sh
# Ubuntu 26.04
sudo apt install build-essential pkg-config cmake ninja-build libgtk-4-dev patchelf
# macOS (with the Xcode command line tools)
brew install cmake ninja
cargo install tauri-cli --version 3.0.0-alpha.5 --locked
./moq desktop build                    # target/release/bundle: .deb and .AppImage on Linux, .app and .dmg on macOS
```

A bundle carries the `moqui` binary for the sidecar node: `./moq desktop build` builds the Maximal release binary (the
package the app links), stages it as `target/desktop-bin/moqui-<host triple>` and passes it as `bundle.externalBin`,
which puts it beside the app's executable as `moqui`. A development run uses `target/release/moqui` or
`target/debug/moqui` from `./moq build`. The bundle also carries Chromium's notices, CEF's `CREDITS.html` from the
`tauri-cef` cache, as `notices/chromium-credits.html` (`target/desktop-notices`). The checks on installed bundles are in
[Mobile-Desktop-Exit-Checks](../plans/Mobile-Desktop-Exit-Checks.md).

Signing and updates are on when their variables are set, else the bundles are unsigned with no updater; `./moq help
desktop` lists them:

- **macOS signing:** `APPLE_SIGNING_IDENTITY`, or `APPLE_CERTIFICATE` and `APPLE_CERTIFICATE_PASSWORD` (a base64
  .p12). The Tauri CLI signs the app, the sidecar and the CEF helpers with the hardened runtime and
  `entitlements.plist`.
- **Notarization:** `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`, or `APPLE_API_KEY` and
  `APPLE_API_ISSUER` (and `APPLE_API_KEY_PATH`). `./moq` checks that each set is complete.
- **Updates:** `MOQUI_DESKTOP_UPDATER_PUBKEY` (the public key of `cargo tauri signer generate`, or its file) and
  `MOQUI_DESKTOP_UPDATER_ENDPOINT` (https URLs, separated by spaces) add `plugins.updater` and
  `createUpdaterArtifacts` to the config; both or neither. `TAURI_SIGNING_PRIVATE_KEY` (the key or its file) and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` sign the artifacts.

On Linux, `./moq desktop build` builds with `cargo tauri build --no-bundle` and then runs `cargo tauri bundle`, with
these workarounds for tauri-bundler 3.0.0-alpha.4:

- Its `.deb` and AppImage steps copy CEF's `libEGL.so` and `libGLESv2.so`, which the CEF 154 Linux distribution no
  longer has. The bundle step's `CEF_PATH` is `target/desktop-cef`, links to the distribution's files with an empty
  shared library for each one missing.
- The quick-sharun.sh it downloads fetches two helper sources (`anylinux.c`, `gtk-class-fix.c`) that
  Anylinux-AppImages has since removed. `ANYLINUX_LIB_SOURCE` and `GTK_CLASS_FIX_SOURCE` point to the last commit that
  has them (`ANYLINUX_APPIMAGES_LIB` in `moq`).
- `STRACE_MODE=0`: quick-sharun would otherwise run the app, and so a node and its Postgres, for a few seconds to
  see what it loads (under `xvfb-run` when installed). Its `DEPLOY_CHROMIUM` rules cover those libraries without the
  run.
- The AppImage is made again without `fix-namespaces.hook`, from the AppDir the bundler leaves beside it, with
  `quick-sharun.sh --make-appimage`. Where AppArmor restricts unprivileged user namespaces (Ubuntu 23.10 and later)
  the hook offers on every start to add an AppArmor profile through pkexec. That fails on a path with a space, and
  would not matter anyway: tauri-runtime-cef starts an AppImage with `--no-sandbox` whenever
  `kernel.apparmor_restrict_unprivileged_userns` is 1. The `.deb` keeps Chromium's sandbox through a setuid
  `chrome-sandbox`. With updates on, the remade AppImage is signed again (`cargo tauri signer sign`), since the
  bundle step's signature is of the image it made.

### Android app

`./moq android` builds and runs the Android app in `crates/moqui-mobile`: the client of a server in a Tauri 3 (alpha)
app on Android's WebView (the wry runtime, not CEF), outside the workspace like the desktop app. What it is, and the
Gradle project it keeps in `gen/android`, are in its [README](../crates/moqui-mobile/README.md).

Prerequisites: the Tauri CLI of the desktop app, a JDK 17 or later, the Rust targets, and the Android SDK command-line
tools (no Android Studio), in `~/Android/Sdk` or `ANDROID_HOME`. The image has Play services, for FCM later.

```sh
rustup target add x86_64-linux-android aarch64-linux-android
sdkmanager 'platform-tools' 'emulator' 'platforms;android-37.0' 'build-tools;36.0.0' 'ndk;30.0.16248370' \
    'system-images;android-36;google_apis_playstore;x86_64'
./moq android emulator                       # create (once) and start the moqui-36 AVD, on KVM; adb emu kill
./moq android dev                            # build, install and run on it, or on a phone over adb
./moq android build --target x86_64 --apk    # debug APK; --release for a release one; aarch64 for phones
scripts/check-android.sh                     # clippy for both Android targets: moqui-client, moqui-client-tauri, the app
scripts/android-smoke.sh                     # when asked, not in test-all: ready line, frame check, relaunch
adb reverse tcp:8080 tcp:8080                # ./moq run on this machine as the app's http://localhost:8080
```

`./moq android` (but `init`) first stages the client's shell into `target/mobile-shell`, the same step and layout as
the desktop bundle's (`stage_client_shell`), which the app embeds as its Tauri assets. The app's host logic is tested on
the desktop: `cd crates/moqui-mobile && CARGO_TARGET_DIR=../../target cargo test` (a placeholder page stands in for an
unstaged shell), and the Tauri glue it shares with the desktop app in `crates/moqui-client-tauri` the same way. How the
app works on Android is [Mobile-Client](ui/Mobile-Client.md).

`scripts/android-env.sh`, which `./moq android` and `scripts/check-android.sh` source, finds the SDK, the newest NDK
in it and the JDK (on macOS `java_home`'s, else Android Studio's), and exports `ANDROID_HOME`, `NDK_HOME` and
`JAVA_HOME` for the Tauri CLI and the NDK's compilers for the C in aws-lc and SQLite; the emulator needs only the SDK.
The first Gradle build downloads Gradle and the Android Gradle plugin (and the SDK platform and build tools it compiles
against, `compileSdk` 37, when they are missing), and the rustls-platform-verifier Maven artifact from GitHub, pinned
to a commit of its archive.

The emulator runs in a hidden window (`-qt-hide-window`), not `-no-window`: `cargo tauri android dev` (cargo-mobile2)
waits for an emulator's boot animation to stop, and `-no-window` and `-no-boot-anim` turn it off, so it would wait
forever. With no display (no GUI session on macOS) `./moq android emulator` falls back to `-no-window`, and
`./moq android dev` refuses such an emulator; build an APK and `adb install` it instead. Every adb call names the
emulator's serial, so a phone connected too is no trouble.

#### Release builds and Google Play

`./moq android build --release` makes an AAB and APKs for each target (`--target aarch64` for phones, `--apk` or
`--aab` for one), minified by R8 (`gen/android/app/proguard-rules.pro` and wry's rules keep what Rust calls through
JNI). The version name is the crate's version (`crates/moqui-mobile/Cargo.toml`, passed as the Tauri config's
`version`) and the version code major·1000000 + minor·1000 + patch, so each release raises the crate's version. As
for the desktop, signing is on when its variables are set, else the build is unsigned, with a warning (an unsigned APK
does not install, and Play takes only a signed AAB):

- `MOQUI_ANDROID_KEYSTORE`: the upload key's keystore (`.jks` or `.p12`), an absolute path, kept out of git.
- `MOQUI_ANDROID_KEYSTORE_PASSWORD` and `MOQUI_ANDROID_KEY_ALIAS`; `MOQUI_ANDROID_KEY_PASSWORD` when the key's
  password is not the store's. Each password can instead be a file's first line (`MOQUI_ANDROID_KEYSTORE_PASSWORD_FILE`,
  `MOQUI_ANDROID_KEY_PASSWORD_FILE`). `./moq` checks that the set is complete.

A release build runs Gradle with no daemon (`-Dorg.gradle.daemon=false`), so no process is left with the passwords
in its environment, and refuses a prerelease version (`0.2.0-beta.1`), whose version code would be the release's.

```sh
keytool -genkeypair -keystore ~/keys/moqui-upload.jks -alias upload -keyalg RSA -keysize 4096 -validity 10000
read -rs -p 'Keystore password: ' MOQUI_ANDROID_KEYSTORE_PASSWORD && export MOQUI_ANDROID_KEYSTORE_PASSWORD
MOQUI_ANDROID_KEYSTORE=~/keys/moqui-upload.jks MOQUI_ANDROID_KEY_ALIAS=upload \
    ./moq android build --release --target aarch64 --aab
MOQUI_MOBILE_APK=crates/moqui-mobile/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk \
    scripts/android-smoke.sh   # a release APK for x86_64 on the emulator: R8 kept the JNI classes, the shell loads
                               # (adb uninstall org.moqui.mobile first if a debug build, signed by another key, is on it)
```

Firebase (native push) is in a build only with `gen/android/app/google-services.json` ([moqui-mobile
README](../crates/moqui-mobile/README.md)); a release for users wants it.

In the Play Console, once:

- **Play App Signing:** Google holds the app's signing key; the keystore above is the upload key, which Google can
  reset if it is lost. Register its certificate when the app is created, and keep the keystore and its passwords out
  of the repository.
- **The target SDK:** `targetSdk` in `gen/android/app/build.gradle.kts` (37); Play requires a recent one for new
  apps and updates each August.
- **Declarations:** the full-screen intent (`USE_FULL_SCREEN_INTENT`, for an incoming call: a calling app), and the
  foreground service of type `phoneCall` (`FOREGROUND_SERVICE_PHONE_CALL`, an ongoing call the user answered), each
  with a short video or description of the incoming call.
- **Data safety:** the app keeps on the device, not shared with anyone but the user's own server: chat messages and
  mail (each account's store, for offline use), contacts' names in them, files attached, and a device key (a
  credential, sealed by the Android Keystore); it records audio (and video) in calls only, sent to the user's server;
  it shows notifications; with Firebase, the FCM token goes to the user's server and Google. Data in transit is
  encrypted (TLS); the user can delete it by signing out on a shared device or removing the account. No backup is made
  (`allowBackup` off).
- **Remote code:** the app shows the user's server's screens in its WebView; whether that is fine under Play's device
  and network abuse policy is a decision for the release ([Mobile-Desktop §9](../plans/Mobile-Desktop.md), Android,
  open).

## The moq script

The `./moq` script runs the built binaries in place and replaces the Java Moqui Gradle tasks: run, load, save and reload
of the database, clean, OpenSearch, Chromium for PDF, git across all repositories, and component get, create and zip.
It needs only bash (3.2 or newer), so it works on a fresh checkout on Linux or macOS. `./moq` with no arguments lists
the tasks, and `./moq help <task>` describes one. Task names match by partial parts, ignoring case, so `./moq r-p` and
`./moq runProduction` both mean `run-production`.

```sh
./moq build                    # Maximal with the dev tools, target/release/moqui (--lite, --debug, --enterprise for
                               # the others), linking any component crates in runtime/component; tasks that
                               # run moqui build a missing binary themselves
./moq run                      # target/release/moqui with runtime/conf/MoquiDevConf.xml; --lite uses target/lite/moqui
./moq run -p 8081 -D entity_ds_db_conf=postgres
./moq run lint --screens       # moqui's own commands, with the same conf
./moq load                     # all data types; ./moq load seed seed-initial install
./moq test mr-mantle           # run a component's JavaScript specs; extra args go to moqui test (--spec, --db snapshot)
./moq test-client framework    # a component's test/client specs against the running server (docs/dev/Client-Testing.md)
./moq test-apps                # the application tests: the specs of runtime/component, no cargo
./moq test-framework           # the framework tests: cargo, moq, the specs of base-component and dev-component
./moq test-all                 # the whole regression suite: test-framework, then test-apps
./moq accept mr-boss           # a component's acceptance suite (acceptance/), only when asked
./moq accept-all               # every acceptance test, with the Rust opt-in and ignored tests
./moq save-db before-upgrade   # and later: ./moq reload-save before-upgrade
./moq archive                  # mr-moqui-YYYYMMDD.zip (or --tgz): runtime/ without logs, with the binary in it
./moq git-status               # or any git command in every repo: ./moq git pull
./moq completion --install     # tab completion for bash or zsh
```

The everyday tasks with their Gradle counterparts, and the runtime, conf and deployment detail, are in
[Run-Deploy](Run-Deploy.md#everyday-commands).

## Rust tests and lints

```sh
cargo test                             # the whole workspace, every feature
cargo lint                             # clippy on the workspace, warnings denied
cargo lint-min                         # clippy on the host with no features
scripts/check-packages.sh              # clippy on each package's feature set

# The Gate 3 tests, left out of the default run (embedded Postgres per test, Chromium: ./moq chromium-download, or
# moqui_chromium_path, or an installed Chromium or Chrome): acceptance tests of the framework, run by ./moq accept-all
cargo test -p moqui-host --test gate3
```

`scripts/check-packages.sh` runs Clippy with warnings denied on each feature set a build uses: the workspace
(Maximal with the dev tools, as `cargo lint`), the Lite, Maximal (with and without the dev tools) and Enterprise packages, the host with no features
(`cargo lint-min`) and with only the terminal client, and the framework with none (what the browser links). Then it
runs the scripted TUI screens, which `cargo test` leaves out. A gate that only one package sees still has to compile
there.

`./moq test-framework` runs the framework's suites in turn and lists which passed: `cargo test`,
`scripts/moq-test.sh` (the tests of `./moq`), `moqui test` for the components of `runtime/base-component` and
`runtime/dev-component`, the demo component's specs (it is off by default, so the suite turns it on), the offline client
specs of webroot and dev-aide, and the served client specs of those components. `./moq test-apps` runs `moqui test` and
`moqui test-client --serve` over the components of `runtime/component` only, with no cargo build or test; it builds the
binary only when it is missing. `./moq test-all` is both in turn. `./moq help <task>` lists each one's suites.

### The two suites

| | Regression suite | Acceptance suite |
|---|---|---|
| Purpose | Catch a regression while developing: does what I changed break a rule? | Validate the system: complete scenarios, access and isolation walks, one screen sweep per app and mode, browser walks, load and media |
| When | While working (the specs of what you touch) and before every commit | On a schedule, before a release, and when asked |
| Where | `test/**/*.spec.js`, `test/client/**/*.spec.js` | `acceptance/**/*.spec.js`, `acceptance/client/**/*.spec.js` |
| Command | `moqui test`, `moqui test-client`, `./moq test-apps`, `./moq test-framework`, `./moq test-all` | `./moq accept [component...]` (`moqui test --acceptance`, `moqui test-client --acceptance`), `./moq accept-all` |
| Failure | Blocks the commit | Reported; a test that fails on two runs in a row is fixed or deleted |

A spec is in one suite or the other by the directory it is in. No option of the regression commands pulls in an
acceptance test. A regression test checks one rule or a fixed bug: a service's result and refusals, a posting, a state
change, the one render of a screen with logic of its own, one golden-path walk per app. An acceptance test checks a
scenario or the system: a plan's "usable when" walk (server side or in a browser), an access or isolation walk over
every route and group, a sweep of every screen of an app in a mode, a browser walk of a flow, a load or media test.
Acceptance tests can be deleted freely, since nothing depends on them.

- **Discovery.** `moqui test` and `moqui test-client` never read `acceptance/`. With `--acceptance` they run only the
  acceptance specs (`acceptance/*.spec.js` and `acceptance/client/*.spec.js`, with `acceptance/suite.js` and
  `acceptance/client/suite.js` giving the order and the server). `--spec` and `dependsOn` resolve across both trees, so
  an acceptance spec can depend on a regression fixture. A spec's name is unique across both trees of a component.
- **Fixtures.** Acceptance fixtures live under `acceptance/`, never in `test/`. Helper libraries a spec loads by
  `component://<component>/test/...` stay in `test/`, for both suites.
- **Rust.** Rust has no such directory: its acceptance tests are `#[ignore = "..."]` (`cargo test-slow`) or opt-in
  `[[test]]`s (`gate3`, `gate3_browser`, `client_browser`, `tui_screens`). `./moq accept-all` runs them, and no
  regression command does.
- **Who runs it.** `./moq accept-all` on a schedule (nightly, or weekly while it is long) and before a release, with
  `--junit` for a dated summary. An agent runs an acceptance test only when the user asks, or when it adds or changes
  one (then that test, and its component's acceptance suite), never because code it uses changed (AGENTS.md).

### The regression budget

Each component declares its regression budget, in seconds of feature time (the serial sum, so parallel workers do not
hide growth), in `test/budget`, beside the specs it limits:

```
server = 195
client = 30
```

`server` limits `moqui test`, `client` `moqui test-client`. A run warns when a component's feature time goes over its
budget, and fails over 1.5 times it, or when one spec takes over 15 seconds. The margin keeps a slower machine from
failing a run; the warning keeps the growth visible. A new app adds its own budget. A budget is raised only with the
user's agreement, like any other performance regression; cut duplicates or move scenarios to acceptance instead.

### Rendering screens in tests

These rules are the regression suite's. A sweep has one place, `acceptance/`, and one per app per mode.

- No regression test walks every screen of an app or of all apps, or every screen in every render mode. These have been
  deleted before and keep coming back; do not add one. Rendering as such is the framework's (its goldens and `moqui lint
  --screens`).
- Across the regression suite a screen is rendered at most once in each render mode, and that one render carries every check
  of the screen (its content, its access, a value that must not show).
- A screen is rendered in more than one mode only where its modes differ: a screen with its own qvt or tui template
  (`.qvue`, `.tui.json`, or a `render-mode` section). Say in the check which template it covers.
What made this rule was a screenshot matrix of every Products page at three widths in both themes: 535 cases and 9
minutes for checks that did not depend on the width or the theme. No spec takes screenshots for review; check at a
width or in a theme only what changes there.

## Component specs

`moqui test` runs the JavaScript spec files in a component's `test/` directory. They run in the same script engine as
the component's services, on a database loaded with every data type, with Spock's conveniences: spec and feature
setup and cleanup, ordered features with shared state, `given`/`when`/`then`/`expect` blocks whose conditions fail with
the values of their parts, `where` tables, a suite order, data checks, pinned sequence ids, a pinned clock, screen and
REST renders, and features that run on several threads.

The specs of `runtime/component/mr-mantle/test` are the largest set, ported from the Java mantle-usl's Spock specs; its
`README.md` maps Groovy to spec files line by line. The design record is `plans/archive/TestHarness.md`.

Tests from outside the server, over HTTP, WebSocket and headless Chromium, with a load runner, are the specs of a
component's `test/client/`, which `moqui test-client` runs against a running server:
[Client-Testing](dev/Client-Testing.md).

### Running

```sh
./moq test mr-mantle                                  # the Maximal binary and the dev conf
moqui test mr-mantle                                  # every spec, fresh database
moqui test mr-mantle --db snapshot                    # load once, restore the saved dump on later runs
moqui test mr-mantle --spec OrderToCashBasicFlow      # one spec and the specs its dependsOn names
moqui test mr-mantle --spec OrderToCashBasicFlow --feature "ship Sales Order"
moqui test mr-mantle --junit build/test-results       # JUnit XML for CI
moqui test mr-mantle --coverage                       # the services no spec called, per service file
moqui test mr-mantle --keep --log-level info          # keep the scratch runtime, more on the console
```

With no component, every component that has `test/*.spec.js` runs, in load order. A run with no `--spec` leaves out
each fixture spec that a served client suite of its component names in `server.specs` (`test/client/suite.js`,
`acceptance/client/suite.js`), unless a spec that runs depends on it: `moqui test-client --serve` runs it on its
server, so a plain run would make the same data twice. `./moq test --debug` uses the debug
binary.

| Option | |
|---|---|
| `--spec Name[,Name]` | The specs to run, by file name without `.spec.js` (`component/Name` for one component's, where two have a spec of the name), with the specs their `dependsOn` names, in suite order. A `dependsOn` name the spec's own component lacks is found in the run's other components |
| `--feature text` | Only the features whose names contain the text |
| `--workers N` | Run the specs in N child processes, each on its own copy of the `--db snapshot` database, which the run makes once first. A component's `suite.js` chain and what a spec's `dependsOn` names stay in one worker; every other spec may run in any worker, on a database without the other specs' writes, so a spec that reads another spec's data names it in `dependsOn`. The summary, JUnit files and feature times are of the whole run. Not with `--serve` |
| `--acceptance` | The acceptance specs (`acceptance/`) in place of the regression specs; `--spec` and `dependsOn` resolve across both ([The two suites](#the-two-suites)) |
| `--db fresh` | The default: load every data type into a scratch runtime, removed at the end unless `--keep`. Needs `embedded-postgres` |
| `--db snapshot` | As fresh, but save the load under `runtime/save` and restore it while the components the run loads (which the conf and the instance purpose decide: a dev component loads only on a dev instance), their entity and data files and the crate version are unchanged. A relink keeps the snapshots; after a change to how the binary loads data, remove `runtime/save/test-*` |
| `--db existing` | The runtime's embedded database as it is. The specs change its data |
| `--db none` | Boot the factory and skip Postgres, so `ec.resource` works and the entity facade stays unset. A build without `embedded-postgres` can run this. Every selected spec must set `database: false`; one that does not is named and the run stops before boot. When every selected spec sets it, and the run is not `--serve`, `--agents`, `--skills`, or `--coverage`, this is the default |
| `--junit dir` | `TEST-<component>.<Spec>.xml` per spec |
| `--serve [address]` | When the specs pass, serve the scratch database on the address (`127.0.0.1:0`, a free port, by default; or a port alone) until Ctrl-C, SIGTERM, or the end of standard input when it is a pipe; then remove the scratch runtime unless `--keep`. With `--db existing` it serves the runtime's own database, for a runtime whose cluster was put in place beforehand (a copy of a loaded, stopped cluster in `db/embedded` starts in a fraction of a second, where a load or `--db snapshot` restore takes seconds to minutes); with no specs to run (each component named has none) the server starts at once, with no test factory booted before it. Not with `--db none`. For client specs ([Client-Testing](dev/Client-Testing.md#--serve)) or a person |
| `--ready-file path` | With `--serve`: write the server's URL here once it listens |
| `--coverage`, `--coverage-report file` | The services of the components' `service/*.xml` that no spec called, with the total; the report adds call counts |
| `--log-level level` | Console level, `warn` by default. Everything goes to `runtime/log/moqui.log`, whose path the run prints |

The console has a line per feature, and the failures in full at the end. Any failed feature exits 1.

### Files and order

A regression spec is `<component>/test/<Name>.spec.js`, and an acceptance spec `<component>/acceptance/<Name>.spec.js`, or in a subdirectory of `test/` to group a busy component's specs (mr-mantle
by service directory, `test/order/OrderToCashBasicFlow.spec.js`; mr-boss by screen directory). Its name is the file
name, wherever it is, so it must be unique in the component. `test/suite.js`, `suite(["SpecA", "SpecB"])`, gives the
order; the other spec files run after it, by name. Each spec runs in its own execution context, on the one
database of the run, so a spec sees what the specs before it wrote.

A component's `test/MoquiConf.xml` merges after its `MoquiConf.xml` in `moqui test` runs only, for a test-only setting
or mount. `test/client/` holds client specs and `test/agent/` agent tasks, which `moqui test` does not read as specs.

### A spec file

```js
const effectiveTime = Date.now()
let cartOrderId                                     // shared between features (Spock's @Shared)

spec({
    dependsOn: ["OrderProcureToPayBasicFlow"],      // run first when this spec is picked with --spec
    tempSequences: { start: 55500, banks: { "mantle.order.OrderHeader": 10 } },   // reset after cleanupSpec
    setupSpec() { ec.user.setEffectiveTime(effectiveTime) },                      // reset after the spec
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("create Sales Order", {
    when() {
        login("joe@public.com")
        const priceMap = call("mantle.product.PriceServices.get#ProductPrice", { productId: "DEMO_1_1" })
        cartOrderId = call("mantle.order.OrderServices.add#OrderProductQuantity", { productId: "DEMO_1_1", quantity: 1 }).orderId
        const dataCheckErrors = checkData(`<entity-facade-xml>
            <mantle.order.OrderHeader orderId="${cartOrderId}" entryDate="${effectiveTime}" grandTotal="200.68"/>
        </entity-facade-xml>`)
        return { priceMap, dataCheckErrors }         // what then() reads
    },
    then({ priceMap, dataCheckErrors }) {
        priceMap.price == 16.99
        dataCheckErrors.length == 0
    },
})

feature("call the REST API (#requestMethod, #screenPath)", {
    where: [
        { requestMethod: "post", screenPath: "s1/mantle/orders/55800/place", containsTextList: ['"statusChanged" : true'] },
    ],
    expect({ requestMethod, screenPath, containsTextList }) {
        const str = screenTest("rest").render(screenPath, null, requestMethod)
        containsTextList.every((text) => str.assertContains(text))
    },
})
```

#### `spec({...})`

Once per file:

- `dependsOn`: the specs that run first when this one is picked with `--spec`.
- `requires`: build features the spec needs, such as `["git"]` (`moqui version` lists a binary's; see
  [Packages](#packages)). In a binary without one of them the spec is reported as skipped, not run.
- `database: false`: the spec does not use the entity facade. `ec.entity`, `call`, and `screenTest` throw, and the
  error names `database: false`, even when the run opened a database. `ec.resource` stays. A run of only such specs
  uses `--db none` (see the option above). `checkData` uses the entity facade, so it fails in one of these specs.
  The screen template specs `ScreenHtml`, `ScreenQvt`, `DatePeriod` and `FieldTitle` are this kind: they read the
  template modules with `ec.resource.getLocationText` and evaluate them with `runInContext(source, sandbox)`.
- `stepwise`: after a failed feature, skip the rest (Spock's `@Stepwise`). Off by default.
- `tempSequences: { start, banks }`: sequenced ids from `start`, with the bank size for each entity, set before
  `setupSpec` and reset after `cleanupSpec`.
- `allowMessageErrors`, and `timeout` in seconds per block.
- `setupSpec`, `cleanupSpec`, `setup` and `cleanup`, as in Spock. Cleanups always run. These get no `ctx`.

#### `feature(name, {...})`

A statement at the top level of the file; features run in the order they are written.

- Blocks: `given`, then `when` with `then`, or `expect` alone, and `cleanup`, which always runs. Each block gets `ctx`:
  the fields of the `where` row and what the earlier blocks returned.
- `where`: an array of row objects, or a function that returns one. Each row is its own case, and `#field` in the name is
  the row's value.
- `ignore: "reason"`, `timeout`, `allowMessageErrors`.
- `requires: ["git"]`: build features the feature needs, as on `spec()`; in a binary without one it is reported as
  skipped. A `where` row may have its own `requires`, for a row that needs a feature the others do not.
- A case that does not say `requires`, and fails only because the binary reports that a tool is not built in
  (`git is not built into this host`, `scheme git is not supported`, and the same messages for the other tools), is
  skipped the same way. A stepwise file that is entirely about that tool says so on `spec()`, so its later features
  are not started without their setup. So is a case that fails only because PDF is not available, on a machine with
  no Chromium (`./moq chromium-download`, or `moqui_chromium_path`); the skip reason is that message.
- A feature with `then` and no `when`, or a block name that is not one of these, is an error when the file loads.

### Conditions

Each expression statement directly in `then` or `expect` is a condition, as in a Spock `then:` block. A false one fails
the feature and shows the value of each part:

```
Condition not satisfied:

priceMap.price == 16.99
|        |     |
|        16.98 false
{"price":"16.98","priceUomId":"USD"}
```

- `==`, `!=`, `<`, `<=`, `>` and `>=` compare as Groovy does: decimals, numbers and number text by value
  (`16.990 == 16.99`), and lists and maps by their items. `===` stays strict.
- `!x` and a bare value use Groovy truth: an empty list, map or string, and zero, are false.
- An assignment is not a condition, and a call that returns `undefined` passes.
- Inside an `if`, a loop or a callback, use `assert(condition, message)`.
- A feature also fails when it leaves errors in `ec.message`, unless `allowMessageErrors` is set. Messages are cleared
  between features.

The source is rewritten for this before it runs (`crates/moqui-devkit/src/testkit/rewrite.rs`), with no new lines, so error
line numbers are the file's. A `then` or `expect` the rewrite cannot see, such as a feature object built in a helper
function, is refused when the file loads rather than run with conditions that cannot fail. Use `where` rows instead.

### Helpers

`ec` is the full script `ec`, as in a service. The spec engine adds:

| Helper | |
|---|---|
| `call(name, params)` | A service call. It throws, naming the service and its `ec.message` errors, where `ec.service.sync()` would return null |
| `checkData(xml)` | `ec.entity.makeDataLoader().xmlText(xml).check()`: the messages of what did not match, each logged |
| `login(username, password = "moqui")`, `logout()` | `login` throws when the login fails |
| `screenTest(baseScreenPath)` | `ec.screen.makeTest().baseScreenPath(...)`. `render(path, params, method)` gives `output`, `errorMessages`, `status`, `renderTime`, `headers` (the response's, and `header(name)` by name in any case) and `assertContains(text)`; `renderMode("html-print")` and the other builder methods set the mode and root screen, `header(name, value)` a request header of every render, and `clientModes("qvue,qjs")` the `X-Moqui-Client-Modes` header `moqui.loadComponent` sends, so a spec checks the best client mode (`render("Gallery/QuickTask.qvt").header("X-Moqui-Qvt") == "qvue"`). As Java's `ScreenTestImpl`, a test render is not counted against tarpits |
| `parallel(n, fn, args, opts)` | Run `fn(threadNum, args)` on `n` threads, each with its own `ec`, and return the results. `opts`: `transaction` (default true), `timeout` (default 120 s) |
| `uploadedFile(fileName, text, contentType = "text/plain")` | The `FileItem` that services taking an upload expect |
| `setProperty(name, value)`, `withProperty(name, value, fn)` | A process property, as a conf property would set it (`System.getProperty` and `get_prop_or_env` read it); `null` clears it. `setProperty` holds for the rest of the spec, for a policy switch its features share such as `call_recording_enabled`; `withProperty` only while `fn` runs, then puts the value back, for one feature such as `call_captions_enabled` off. Every property a spec changed, by either, is put back as it was after `cleanupSpec`, and when the spec ends without reaching it |
| `assert(condition, message)` | A condition outside the top level of a block |
| `sum(list, field)`, `dec(value)` | A decimal sum of a field, and a value as a decimal |
| `log` | `ec.logger` |

`ec.user.setEffectiveTime(millis)` pins the clock for `ec.user.nowTimestamp`, date filters with no date, and the
`fromDate` defaults; the runner clears it after the spec. `ec.entity.tempSetSequencedIdPrimary` and
`tempResetSequencedIdPrimary` are what `tempSequences` calls.

### Mail

A spec checks a send that goes, with no mail server, through the mail capture: while it is on, `send#EmailMessage`,
and so `send#EmailTemplate` and the mail client's `send#Draft`, does all of a real send (the `EmailMessage` row, its
`ES_SENT` status and `sentDate`, the mail client's Sent folder) except connect, and keeps the message in memory. The
`EmailServer` must still have an SMTP host, as for a real send (an empty one sends nothing); it is never connected. A
spec turns it on for itself, and the conf property `mail_capture_sends=true` turns it on for a whole run
(`mail_capture_sends=true ./moq test ...`) or a server that must not send mail; it is off otherwise, so a spec that
checks a send that cannot connect sees one. The tool reads and clears what was captured:

```js
const capture = ec.factory.getTool("MailCapture")
spec({ setupSpec() { capture.enable() }, cleanupSpec() { capture.reset() } })
// in a feature
capture.clear()
const sent = call("org.moqui.impl.EmailClientServices.send#Draft", { emailMessageId: draftId })
capture.last().toList            // ["customer@example.com"]
```

| Method | |
|---|---|
| `getMessages()` | The captured messages, oldest first, each `{subject, fromAddress, fromName, toList, ccList, bccList, replyToList, body, bodyText, messageId, inReplyTo, references, attachmentNames, headers, capturedDate}` (the 500 most recent; `headers` the added header fields, such as a list email's List-Unsubscribe, name to value) |
| `last()`, `count()`, `clear()` | The newest one (or null), how many, and forget them |
| `enable()`, `disable()`, `reset()` | Capture on or off for this server whatever the property says, and back to the property; reset it when the spec ends, as the switch outlives the spec |
| `isEnabled()` | Whether a send now is captured |

The capture is a tool of the framework (`moqui_core::service::mail_capture`, in `MoquiDefaultConf.xml`).

### HTTP fixtures

A spec exercises a provider adapter (a payment, shipping or EDI gateway) through its production code with no network:
while the HTTP fixtures are enabled, every outbound HTTP request of the run's server (`ec.service.rest()` `call()` and
`send()`, `remote-rest` services, `http` locations) is answered by the first fixture it matches, in the order added, and
recorded. A request that matches none fails as a transport error naming it, so nothing goes out. The adapter keeps its
endpoint from its gateway configuration (a sandbox URL), and the fixture matches that URL; the adapter has no test branch.

```js
const http = ec.factory.getTool("HttpFixtures")
spec({ setupSpec() { http.enable() }, cleanupSpec() { http.reset() } })
// in a feature: an answer once, then a lost answer (the request went out, its answer did not come back)
http.add({ method: "POST", url: "https://apitest.example.com/v1/charges", status: 200, contentType: "application/json",
    body: JSON.stringify({ id: "ch_1" }), times: 1 })
http.add({ url: "https://apitest.example.com/v1/*", error: "operation timed out" })
// ... call the Mantle service that calls the adapter, then
http.requests()[0].headers["Idempotency-Key"]
```

| Method | |
|---|---|
| `add({method, url, bodyContains, status, body, headers, contentType, error, times})` | A fixture. `url` is exact, or a prefix when it ends with `*`; `method` and `bodyContains` narrow it; `status` is 200 by default; `error` fails the call as a transport error after recording it; `times` is how many requests it answers (absent or 0, any number) |
| `requests()`, `last()`, `count()` | The recorded requests, oldest first, each `{method, url, headers, body, fixture, at}`, `fixture` the index of the fixture that answered or -1 (the 500 most recent). The values of Authorization, Proxy-Authorization, Cookie and `*Api-Key` headers are `[redacted]` |
| `enable()`, `disable()`, `isEnabled()` | Answer and record; off by default. `enable()` is refused outside a `moqui test` run |
| `allowNetwork(urlPrefixes)` | Let requests to these URL prefixes go to the network while the fixtures are off (see below); an empty list closes it again |
| `clear()`, `reset()` | Forget the fixtures and requests; `reset()` also disables and forgets `allowNetwork`. Reset when the spec ends, as the switch outlives the spec |

The fixtures are a tool of the framework (`moqui_core::service::http_fixtures`, in `MoquiDefaultConf.xml`), on the
transport under `moqui_core::outbound`. They are process-wide, so while they are on a request sent with no execution
context (a worker thread's) is answered or refused too, and only the `moqui test` runner allows enabling them.

A `moqui test` run is closed to the network while the fixtures are off too: a request to anything but a loopback address
(localhost, `*.localhost`, 127.0.0.0/8, ::1) fails as a transport error naming its redacted URL, with a warning in the
log. A spec's gateway records outlive it, and a later spec's job over them (a details poll) would otherwise send their
spec credentials to a provider's sandbox. A spec that needs a real host opens it with `allowNetwork`, and resets after.

### Notifications

A `moqui test` run has no notification socket, and what is sent only to someone listening (the shell counts of
`org.moqui.impl.ShellServices.publish#ShellCounts`) is not sent at all. A spec that checks those notifications turns on
the notification capture, which listens on the notification topic as the server's socket does and keeps each message in
memory:

```js
const capture = ec.factory.getTool("NotificationCapture")
capture.enable()
try {
    // ... what publishes, then (the shell counts go after the commit, on a thread of the server) wait for them
    capture.getMessages("shellCounts").filter((row) => row.userIds.includes(userId)).length
} finally { capture.disable(); capture.clear() }
```

| Method | |
|---|---|
| `getMessages(topic?)` | The captured messages, oldest first, all or of one topic, each `{topic, subTopic, userIds, title, link, message, capturedDate}` (the 500 most recent) |
| `last(topic?)`, `count(topic?)`, `clear()` | The newest one (or null), how many, and forget them |
| `enable()`, `disable()`, `isEnabled()` | Listen or stop; disable it when the spec ends, as the listener outlives the spec |

The capture is a tool of the framework (`moqui_core::notify::capture`, in `MoquiDefaultConf.xml`).

### Agent evaluation

`--agents` runs a component's agent tasks instead of its specs: the files `test/agent/*.agent.js`, each a set of tasks
for one agent, run as real runs of the agent on the test database, with the agent engine started and the screen
sessions installed as a server has them (`plans/LLM-A2A-Assist.md`, section 9). The model is a live one or a recording:

```sh
moqui test tools --agents --llm-profile local-qwen --runs 3 --seed 7   # a model of the llm-facade conf
moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay   # no model
moqui test tools --agents --replay runtime/base-component/tools/test/agent/replay-small   # a small model in JSON tool mode
moqui test tools --agents --llm-profile local-qwen --feature "approval" --db snapshot
```

| Option | |
|---|---|
| `--llm-profile model` | The model the agent runs on: a `model` id of the `llm-facade` conf or an `LlmModel` row. The agent's feature (`assist`) is bound to it for the run and put back after. When the model's endpoint does not take a connection the run is skipped, not failed. An unknown id is an error |
| `--replay dir` | No model: the model replies recorded in `<dir>/<spec>/<task>-<n>.jsonl` (else `<task>-1.jsonl`) are played by the scripted fake provider, with the recorded model's settings, and the tools run live. The replay model makes no side call (`sideCalls` N: no intent hint, the deterministic compaction summary, no schema repair call), since a recording holds only the steps' replies. The directory is a profile's set: `replay/` is the profile `replay`, another `replay-<name>/` (or `<name>/`) the profile `replay-<name>`, with its own baseline; a task the set has no recording of is skipped |
| `--runs N` | Runs of each task, for pass@1 and pass^N. 1 by default |
| `--seed S` | Run n sends `seed` S + n - 1 to a Chat Completions model (in the model row's `apiConfigJson`; other protocols take none) |
| `--min-pass F` | The pass@1 a live run needs, 0.75 by default (6 of 8). A replay needs every task to pass |
| `--write-baseline` | Write `test/agent/baseline-<profile>.json` (`baseline-replay.json`, `baseline-replay-small.json` for the replay sets) from this run |

`--spec` picks agent spec files by name (without `.agent.js`), `--feature` tasks by name, and `--db` works as for the
specs. A live profile is a model of the conf, such as the commented `local-qwen` example of `MoquiDefaultConf.xml` (Qwen
in LM Studio at `http://127.0.0.1:1234`): copy it into the runtime's conf, or set the default endpoint's
`llm_openai_url` and `llm_openai_model` and use `--llm-profile default`.

#### A task file

```js
agentSpec({
    agent: "assist",             // the agent (AgentDefinition), assist by default
    surface: "drawer",           // drawer (default), chat, tui or job
    user: "john.doe",            // the user the tasks run as, unless a task names its own
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
    tasks: [
        {
            name: "multi-step with approval",
            start: "/apps/system/dashboard",         // the page the user is on; also { path, parameters, title }
            say: "Change the description of the user group AGENT_EVAL_GROUP to Renamed.",
            setup() { /* rows the task needs, with ec and the spec helpers */ },
            cleanup() { /* remove them */ },
            approvals: ["approve"],                  // scripted answers: "approve", { edit: {...} }, { reject: "why" }
            expect: {
                path: "/apps/system/Security/UserGroup/UserGroupDetail",
                parameters: { userGroupId: "AGENT_EVAL_GROUP" },
                held: { transition: "updateUserGroup", text: /Renamed/, status: "executed" },
                answer: /saved/i,
            },
            verify(run) { assert(run.calls.length <= 6, "too many calls") },
        },
    ],
})
```

A task has `say` (one message, or an array of messages) or `turns`, `[{ say, expect }]` with checks per turn, and
`answers`, the answers to the agent's questions in order. Each turn is a run in one conversation, sent with the
`<context>` tail the drawer sends (the time and the page). The checks are deterministic; an LLM judge is not built:

| Check | |
|---|---|
| `path`, `parameters`, `values` | The screen session's path, its parameters, and form values (`{Form: {field: value}}`, or `{field: value}` in any form) when the turn or task ends |
| `answer`, `answerNot` | Patterns the final answer matches, or must not |
| `called`, `notCalled` | Tools called: a name, or `{tool, args, result, error}` with `args` matched in part |
| `results` | `{tool, result}`: some result of the tool matches |
| `held`, `noWrite` | The approvals the run asked for (a number, or a match of `{tool, transition, text, status, channel}`: `text` is the summary and the changes, `Label: current → proposed`; `status` what became of it, such as executed, rejected or expired); no approval and no write tool call |
| `maxWrongCalls`, `maxToolCalls`, `questions`, `state` | Limits on failed calls and on calls, the questions asked, the run's end state |
| `noInvented` | The answer links nothing no tool returned (no `invented_reference`) |
| `verify(run)` | A function over what the run did (`answer`, `calls`, `held`, `questions`, `screen`, `turns`); `assert` fails it |

A value in a check is a string (equal), a `RegExp`, `null` (empty), or an object or array matched in part. A run that
does not end Completed fails its task. A write pauses the run for its approval, and the task's `approvals` answer them
in order as the user would in the drawer: `"approve"`; `{ reject: "feedback" }`, which the agent reads as its next task;
or `{ edit: { field: value } }`, Edit in screen: the write goes to its screen and the harness saves the form there as
the user, the values (by field name or label) over the proposed ones, so the agent reads the user's own save. A run
that waits for an approval the task has no answer for fails it.

#### Transcripts, measures and baselines

Each task run writes `runtime/log/agent-eval/<run>/<spec>/<task>-<n>.jsonl`: a header with the model's settings, then
the user's messages, each model reply (text, tool calls, usage) with its calls and their full results, the held writes,
the questions, the answer and the screen, and the result with its measures. The console has a line per task run, and a
summary per component, also written as `summary-<component>.json`: pass@1, pass^k, model and tool calls, wrong calls,
argument repairs, invented references, held writes against the expectation, the wrong calls' share of the tool calls, prompt, completion and cached tokens, the
cache-hit ratio, the prompt prefix size, compactions and wall time.

A transcript is also a recording: `--replay` takes its `model` lines (a line's `raw`, the text a model wrote in JSON tool
mode or a call it wrote as text, is played as that text) and compares its `tool` lines with what the tools
answer now, dates, clock times and long ids left out. A change shows as a `replay drift` note, so a change of a tool's
output is seen without a model; the checks decide whether it fails. To record a live run, copy its transcripts into
the replay directory.

A committed `test/agent/baseline-<profile>.json` holds each task's pass@1 and tokens a run, and the suite's wrong-call
share, invented references and cache-hit ratio. A run with lower success, or more than 20% more tokens, than the baseline
prints a `REGRESSION` line and fails; so does a task of the baseline that did not run, unless `--feature` or `--spec`
picked some. The tools component has `baseline-replay.json` (24 tasks) and `baseline-replay-small.json` (9, JSON tool
mode); the baselines of a live model (`baseline-local-qwen.json`, an 8B model's in JSON mode) are written by a live run
with `--write-baseline`.

The tools component's suite (`test/agent/`): `assist.agent.js` (21 tasks in the drawer) and `headless.agent.js` (3 on
the `a2a` surface, run through the run engine; the spec lets the assist agent serve that surface while it runs). The
surface of a spec is `drawer`, `chat`, `tui`, `job`, `a2a` or `mcp`; a headless one sends only the time in the tail.

The dev agent's tasks are in `runtime/dev-component/dev-aide/test/agent/dev.agent.js` (plans/design-dev-aide.md P4): a
field added to an upstream entity and shown on its form from the site component, a list column, where a service is used,
and a spec written from a Try in the sim. A dev component's agents run on a dev instance; the example component stands in
for the site component for the run, and the files a task writes are put back after it. With the hand-authored replies:
`moqui test dev-aide --agents --replay runtime/dev-component/dev-aide/test/agent/replay`. With a model (a key from the
environment, nightly and before a phase closes; the design's `--live` is `--llm-profile`):
`moqui test dev-aide --agents --llm-profile <profile> --runs 3 --min-pass 0.8`, a pass being the checks of the task: the
files proposed and written are the expected ones and check clean. Copy a passing run's transcript into the replay set to
keep it.

### Porting a Spock spec

How a Moqui Java Spock specification maps to a spec file, as used to port mr-mantle's flows:

| Spock / Groovy | spec file |
|---|---|
| `class X extends Specification` | `X.spec.js` with `spec({...})` |
| `@Shared String cartOrderId` | `let cartOrderId` at the top of the file |
| `def setupSpec()`, `setup()`, `cleanup()`, `cleanupSpec()` | the same names in `spec({...})` |
| `ec.entity.tempSetSequencedIdPrimary(name, 55500, 10)` in setupSpec, reset in cleanupSpec | `tempSequences: { start: 55500, banks: { name: 10 } }` |
| `ec.user.setEffectiveTime(new Timestamp(effectiveTime))` | `ec.user.setEffectiveTime(effectiveTime)` |
| `def "create Sales Order"() { when: ... then: ... }` | `feature("create Sales Order", { when() {...}, then(ctx) {...} })` |
| a local of `when:` read in `then:` | returned from `when()`, destructured in `then({ ... })` |
| `expect:` / `where:` table / `@Unroll` | `expect(row) {...}`, `where: [{...}, ...]`, `#field` in the name |
| `ec.service.sync().name(n).parameters([a:1]).call()` | `call(n, { a: 1 })` |
| `"""<entity-facade-xml>...${x}...</entity-facade-xml>"""` with `makeDataLoader().xmlText(..).check()` | `` checkData(`<entity-facade-xml>...${x}...`) `` |
| `dataCheckErrors.size() == 0` | `dataCheckErrors.length == 0` |
| `ec.user.loginUser("john.doe", "moqui")` | `login("john.doe")` |
| `logger.info(...)` | `log.info(...)` |
| `new Timestamp(effectiveTime)`, `"${effectiveTime}"` in data | `effectiveTime` (time stamps are epoch millis text in scripts and data) |
| `new Date(effectiveTime + n)` | `new Date(effectiveTime + n)` (converts to a time stamp) |
| `list.sum { it.availableToPromiseTotal }` | `sum(list, "availableToPromiseTotal")` |
| `a + b` on BigDecimal | `dec(a).add(b)`; `.subtract`, `.multiply`, `.negate()`, `.compare()` |
| `Thread.start {}` with a `CountDownLatch` | `parallel(5, (threadNum, args) => {...}, { effectiveTime })` |
| `ec.screen.makeTest().baseScreenPath("rest")` | `screenTest("rest")` |
| `for (x in list) assert ...` | `for (const x of list) assert(...)` |

### Pitfalls

- Outside a `then` or `expect` block, a line that starts with `[`, `(`, `/` or a template literal continues the line
  before it, since JavaScript inserts no semicolon there: `x = 1` then a line `[a, b].forEach(f)` is one expression.
  Start such a line with `;`, or put the list in a variable. In a `then` or `expect` block each line is its own
  condition, as in Spock: the rewrite ends the line before such a line when it ended an expression, so
  `/^Today · \d+$/.test(label)` on its own line is a regular expression, not a division.
- Decimals from services are objects: `+`, `<` and `Number()` on them throw outside a condition. Use their methods
  (`add`, `subtract`, `multiply`, `negate`, `compare`), or `dec`.
- `parallel` runs its function from its source text on other threads, so it cannot use the spec's variables. Pass what it
  needs in `args`, which is copied as JSON, and return values JSON can hold.
- Time stamps reach scripts and data as epoch millis text: write `entryDate="${effectiveTime}"`, and
  `new Date(effectiveTime + n)` converts to a time stamp.
- A data-dependent service on a product another spec uses can see that spec's inventory and reservations when it runs
  after it; give a new spec its own stock or its own product.
- Generic JavaScript linters report `no-unused-expressions` in `then` and `expect`; those expressions are the conditions.
