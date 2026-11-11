# Build and Test

How to build MR Moqui from source, the build packages and their features, the `./moq` tasks, the Rust tests and lints,
and running the component specs of `moqui test`. Running and deploying are in [Run-Deploy](Run-Deploy.md). The
JavaScript test APIs are in [JS-Testkit](dev/JS-Testkit.md) (the server specs of `moqui test`),
[JS-Test-Client](dev/JS-Test-Client.md) (the specs that test a running server from outside) and
[JS-Test-Load](dev/JS-Test-Load.md) (load scripts).

- [Build](#build): [Windows](#windows), [native code and C libraries](#native-code-and-c-libraries)
- [Packages](#packages): [features](#features), [profiles](#profiles), the [embedded Postgres
  archive](#embedded-postgres-archive), the [instance archive](#instance-archive), [build cost](#build-cost), the [compile cache](#compile-cache), the
  [desktop app](#desktop-app), the [Android app](#android-app)
- [The moq script](#the-moq-script)
- [Rust tests and lints](#rust-tests-and-lints)
- [Component specs](#component-specs): [running](#running), [files and order](#files-and-order),
  [a spec file](#a-spec-file) (the API is in [JS-Testkit](dev/JS-Testkit.md)), [agent evaluation](#agent-evaluation)

## Build

Prerequisites: Rust 1.99 or newer (`rustc --version`; `rust-toolchain.toml` pins 1.99.0, which quickjs-oxide
declares) and bash, on Linux, macOS, or Windows ([Windows](#windows) has what a Windows build needs). A build with
embedded Postgres embeds the 18.6.0 archive that `scripts/pg-payload.sh strip` downloads once and strips of psql (it
links GPL readline); `./moq build` runs that when the archive is missing. Node is not needed to build or test.

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

### Windows

- **Toolchain.** The host is `x86_64-pc-windows-msvc`. Visual Studio Build Tools supplies the C compiler (the
  "Desktop development with C++" workload; aws-lc and QuickJS compile C, and the embedded Postgres binaries load that
  workload's `VCRUNTIME140.dll`), and Git for Windows supplies bash. From PowerShell or cmd, `moq.cmd` runs `moq`; the
  binary is `target\release\moqui.exe`.
- **NASM.** The first build that needs it downloads NASM 3.02 to `%USERPROFILE%\.moqui\nasm` (aws-lc assembles with
  it). Set `MOQUI_NASM` to a directory that contains `nasm.exe`, or to `nasm.exe` itself, to use that copy instead. A
  `cargo build` that does not go through `moq` still needs NASM on `PATH`.
- **NASM may no longer be needed (to check on Windows).** What needed it was the second copy of aws-lc-sys, the one
  mail-auth built with its default features off. AWS-LC is now built once ([Native code and C
  libraries](#native-code-and-c-libraries)), and that one aws-lc-sys has the `prebuilt-nasm` feature aws-lc-rs turns
  on: on Windows x86-64 it links the objects it ships whenever `nasm` is not on `PATH`. To check, with no NASM on
  `PATH`: `cargo clean --release -p aws-lc-sys`, then `MOQ_NASM_CHECKED=1 ./moq build` (the variable skips `moq`'s
  NASM step). If that builds, `scripts/nasm.sh`, `ensure_nasm` in `moq` and these NASM notes can go.
- **Debug info.** The dev profile keeps debug info unpacked in the object files ([Profiles](#profiles)), which is
  not a Windows option, and cargo has no per-target profile, so `moq` sets `CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=packed`
  on Windows (a PDB beside the binary). A dev `cargo build` or `cargo test` that does not go through `moq` needs that
  variable set too.

### Native code and C libraries

MR Moqui stays in memory-safe Rust as far as it can. That also keeps the build and the deployment the same on every
platform.

- **No JIT.** The script engine, quickjs-oxide, interprets its bytecode and forbids `unsafe` in every crate. Generated
  machine code would make the code generator responsible for the engine's memory safety, and needs executable memory,
  which iOS refuses and macOS allows only with an entitlement. Heavy work in scripts moves to framework APIs or native
  services instead ([Practices-App-Dev](dev/Practices-App-Dev.md#heavy-work-scripts-or-rust)).
- **`unsafe` only where nothing else works,** such as a foreign-function boundary (the browser build's exports), with
  the reason beside it.
- **No C libraries in new dependencies.** Choose a pure-Rust crate, or a crate's pure-Rust backend.
  `cargo tree -e build -i cc` lists the crates that compile C. The exceptions:

| Library | For | Why |
|---|---|---|
| AWS-LC (`aws-lc-sys`, through `aws-lc-rs`) | Crypto: rustls (every TLS connection), SSH (russh), certificates (rcgen), WebRTC (str0m), DNSSEC (hickory), DKIM and ARC (mail-auth), and the framework's own signatures | Vetted constant-time C and assembly, the provider rustls defaults to. It is the one crypto library, built once: `deny.toml` refuses ring and a second `aws-lc-sys`, and a dependency that brings either is forked ([Forks](#forks)), pinned or replaced |
| jemalloc (`tikv-jemallocator`) | The binaries' global allocator, not on MSVC | 5 to 15% off script-heavy rows ([Profiling Round 37](../plans/archive/Profiling.md#round-37-the-allocator-measured-a-proposal-nothing-changed)); the alternatives are C too |
| SQLite (`rusqlite`, bundled) | The local databases of moqui-client and moqui-browser; not the server | No pure-Rust database matches it yet. Public domain; the browser build compiles it to WebAssembly |
| CEF, GTK 4, the system WebViews | The desktop and mobile app shells (Tauri) | A desktop or mobile UI runs on the platform's |

Programs that run as their own processes are not linked and are outside this list: the embedded Postgres, Chromium
for PDF, and the system `ssh` for git remotes.

#### Forks

Two dependencies are pinned by `rev` to a branch of moqui's fork, as quickjs-oxide is, because neither keeps ring out
through a feature. Each branch is one commit on the release we use; its `pr/` branch is the same commit on upstream's
own branch, for a pull request. Go back to the crates.io release once upstream publishes the change.

| Crate | Fork and branch (`pr/` branch) | Change |
|---|---|---|
| rsipstack 0.7.12 | [moqui/rsipstack](https://github.com/moqui/rsipstack) `moqui/0.7.12-crypto-features` (`pr/rustls-provider-features`) | `rustls` has no crypto provider of its own; `rustls-ring` (the default) and `rustls-aws-lc-rs` choose one. Its TLS configs come from rustls's process default, here the aws-lc-rs provider |
| mysql 28.0.4 | [moqui/rust-mysql-simple](https://github.com/moqui/rust-mysql-simple) `moqui/28.0.4-no-webpki` (`pr/no-webpki-0.22`) | `rustls` no longer pulls webpki 0.22 (unmaintained, links ring) for two error types: `TlsError::InvalidDnsName` holds rustls's own error, and the unused `TlsError::Pki` is gone |

Moving a pin: push the fork's branch first, then change the `rev` in `Cargo.toml` and run `cargo update -p <crate>`.

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

On Windows each of those binaries is `moqui.exe` (`target\release\moqui.exe` for Maximal).

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
framework crates above core ([Crate-Split](../plans/archive/Crate-Split.md): `moqui-git`, `moqui-mail`) are linked the
same way by a host feature (`git`; `smtp`, with `imap` turning on the mail crate's own), and core reaches them through
`moqui_core::extension`; `moqui-report`, `moqui-collab`, `moqui-tools`, `moqui-dav` and `moqui-devkit` are five more, linked in
every host build (the host's `xlsx` turns on the report crate's own, for the xlsx format of a scheduled report, and its
`devtools` the dev kit's `watch`, the file watcher), and the browser build links the reports and `moqui-dav` (the VCard
and ICalendar tools, elFinder) but not co-editing, the Tools app's database services or the dev kit. The entity facade is the `moqui-entity` crate below core, in every package and in the browser
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
| `test-client` | host | `moqui test-client`: a component's `test/client` specs against a running server, over HTTP, raw TCP or TLS, WebSocket and headless Chromium, with a load runner ([JS-Test-Client](dev/JS-Test-Client.md)), and `moqui test-load`, a component's `test/load` scripts ([Load-Runner](tools/Load-Runner.md)), and the server side of Tools › Load; the two commands open no database and bind no listener of their own, so any package can test any server. Its `--serve` starts `moqui test --serve`, which needs `embedded-postgres` | yes | yes | yes |
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
| `git` | host (`moqui-git`) | `git://` locations, git drives, `org.moqui.git.GitServices`, the git server over HTTP and SSH, LFS | | yes | yes |
| `image` | core | `org.moqui.impl.ImageServices` (`convert#Image`) and elFinder thumbnails | | yes | yes |
| `xlsx` | core, moqui-report | The `xlsx` render mode, `org.moqui.resource.SpreadsheetServices` and the xlsx format of a scheduled report | | yes | yes |
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
- Without `edge`, the `org.moqui.edge.EdgeServices` that need the link (pairing, clients, hosts, ping) fail with
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
  a host test binary was 1.5 GB, 83% of it debug info. Windows has no `unpacked`: there `moq` sets
  `CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=packed`, a PDB beside the binary ([Windows](#windows)).
- `lite` (Lite): `release` with `incremental = true` and 256 codegen units for the workspace and path crates
  (Cargo applies `incremental` only to those), so a rebuild after an edit to the framework is quick; the dependencies
  keep the release settings and are built once.
- `release` (Maximal) and `enterprise`: `panic = "unwind"` (services unwind across the service boundary) and one
  codegen unit for each of the script engine's crates (`quickjs-oxide`, `-core`, `-regexp`), otherwise Cargo's release
  defaults.

### Embedded Postgres archive

The `embedded-postgres` feature embeds the theseus Postgres archive (the version `.cargo/config.toml` forces through
`POSTGRESQL_VERSION`) and unpacks it once into `install/postgres/<version>` in the runtime directory (or under
`MOQUI_INSTALL`), shared by the entity, search and sim clusters. An install an earlier build unpacked in
`runtime/db/install` is moved there on the first start. `crates/moqui-drivers/build.rs` stages it from `~/.theseus/postgresql/postgresql-<version>-<target>.tar.gz` (`%USERPROFILE%\.theseus\postgresql` on Windows; the build reads `USERPROFILE`, not Git Bash's `HOME`), or
the file `MOQUI_PG_ARCHIVE` names, and downloads nothing. It refuses an archive that still holds `bin/psql` or a
readline library (GPL, plans/archive/License-Review.md).

`scripts/pg-payload.sh strip` downloads the archive when it is missing (checking its sha256) and repacks it without
psql; `./moq build` runs it the first time. curl uses `https_proxy` when it is set, and otherwise git's `http.proxy`.
A plain `cargo build` of a package with embedded Postgres asks for it.
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
chain, then rustls, reqwest and moqui-core itself; postgresql_embedded's build script still compiles reqwest and tokio (without TLS) though it no longer
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
It needs only bash (3.2 or newer), so it works on a fresh checkout on Linux, macOS, or Windows (Git Bash; `moq.cmd` from PowerShell or cmd). `./moq` with no arguments lists
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
./moq test-client framework    # a component's test/client specs against the running server (docs/dev/JS-Test-Client.md)
./moq test-load mr-marble --script OrderToCash   # a load script against the running server (docs/tools/Load-Runner.md)
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

### The script engine

The engine is [moqui/quickjs-oxide](https://github.com/moqui/quickjs-oxide), pinned by `rev` in `Cargo.toml`.
- **Building against a local checkout:** pass a patch on the command line, so that no committed file names a path:
  `cargo --config 'patch."https://github.com/moqui/quickjs-oxide".quickjs-oxide.path="../quickjs-oxide"' ...`.
- **Moving the pin:** the fork is pushed first. Then the `rev` changes and `cargo update -p quickjs-oxide` runs.
- **Its speed tests** are `crates/moqui-host/tests/js_speed.rs` and the other `*_speed.rs` tests
  ([plans/Profiling2.md](../plans/Profiling2.md) §2).
- **The fork's own tooling:** its benchmark tools, Sweep results and round records are in
  [moqui/quickjs-oxide-bench](https://github.com/moqui/quickjs-oxide-bench), checked out as `../quickjs-oxide-bench`.
  Measurement output goes there (its `rounds/` and `work/`), not under `target/`, which `cargo clean` removes.

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
- **Rust.** Rust has no such directory: its acceptance tests are `#[ignore = "..."]` (`cargo test-slow`, and
  moqui-core's sweep of screen generation on both script engines) or opt-in `[[test]]`s (`gate3`, `gate3_browser`, `client_browser`, `tui_screens`). `./moq accept-all` runs them, and no
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
REST renders, and features that run on several threads. This section is how to run them, their files and order; the
API a spec is written with is in [JS-Testkit](dev/JS-Testkit.md).

The specs of `runtime/component/mr-mantle/test` are the largest set, ported from the Java mantle-usl's Spock specs; its
`README.md` maps Groovy to spec files line by line. The design record is `plans/archive/TestHarness.md`.

Tests from outside the server, over HTTP, WebSocket and headless Chromium, with a load runner, are the specs of a
component's `test/client/`, which `moqui test-client` runs against a running server:
[JS-Test-Client](dev/JS-Test-Client.md). Load scripts are a component's `test/load/*.load.js`, which `moqui
test-load` and Tools › Load run: [JS-Test-Load](dev/JS-Test-Load.md) for writing one,
[Load-Runner](tools/Load-Runner.md) for running it and reading its result.

### Running

```sh
./moq test mr-mantle                                  # the Maximal binary and the dev conf
moqui test mr-mantle                                  # every spec, fresh database
moqui test mr-mantle --db snapshot                    # load once, restore the saved dump on later runs
moqui test mr-mantle --spec OrderToCashBasicFlow      # one spec and the specs its dependsOn names, run first
moqui test mr-mantle --spec OrderToCashBasicFlow --feature "ship Sales Order"
moqui test mr-mantle --junit build/test-results       # JUnit XML for CI
moqui test mr-mantle --coverage                       # the services no spec called, per service file
moqui test mr-mantle --keep --log-level info          # keep the scratch runtime, more on the console
```

With no component, every component that has `test/*.spec.js` runs, in load order, each component's specs in its
`suite.js` order and then by name; a spec whose `dependsOn` names a spec later in that order (in its own component or
another) waits for it, with the specs after it in its `suite.js` chain, and the rest keep their places. A `dependsOn`
cycle fails the run before any spec runs. So a spec's name need not sort after its fixture's: name the fixture in
`dependsOn`. `moqui test-client` orders its specs the same way, its components after their `component.xml`
`depends-on` ([JS-Test-Client](dev/JS-Test-Client.md#files-and-order)). A run with no `--spec` leaves out each fixture spec that a served client suite of its component names in `server.specs` (`test/client/suite.js`,
`acceptance/client/suite.js`), unless a spec that runs depends on it: `moqui test-client --serve` runs it on its
server, so a plain run would make the same data twice. `./moq test --debug` uses the debug
binary.

| Option | |
|---|---|
| `--spec Name[,Name]` | The specs to run, by file name without `.spec.js` (`component/Name` for one component's, where two have a spec of the name), with the specs their `dependsOn` names, each run before the spec that names it. With components named, a name is a spec of those components; a `dependsOn` name the spec's own component lacks is found in the other components of the pool, and its component joins the run (`moqui test mr-boss --spec X` brings the mr-pass specs X depends on) |
| `--feature text` | Only the features whose names contain the text |
| `--workers N` | Run the specs in N child processes, each on its own copy of the `--db snapshot` database, which the run makes once first. A component's `suite.js` chain and what a spec's `dependsOn` names stay in one worker; every other spec may run in any worker, on a database without the other specs' writes, so a spec that reads another spec's data names it in `dependsOn`. The summary, JUnit files and feature times are of the whole run. Not with `--serve` |
| `--acceptance` | The acceptance specs (`acceptance/`) in place of the regression specs; `--spec` and `dependsOn` resolve across both ([The two suites](#the-two-suites)) |
| `--db fresh` | The default: load every data type into a scratch runtime, removed at the end unless `--keep`. Needs `embedded-postgres` |
| `--db snapshot` | As fresh, but save the load under `runtime/save` and restore it while the components the run loads (which the conf and the instance purpose decide: a dev component loads only on a dev instance), their entity and data files and the crate version are unchanged. A relink keeps the snapshots; after a change to how the binary loads data, remove `runtime/save/test-*` |
| `--db existing` | The runtime's embedded database as it is. The specs change its data |
| `--db none` | Boot the factory and skip Postgres, so `ec.resource` works and the entity facade stays unset. A build without `embedded-postgres` can run this. Every selected spec must set `database: false`; one that does not is named and the run stops before boot. When every selected spec sets it, and the run is not `--serve`, `--agents`, `--skills`, or `--coverage`, this is the default |
| `--junit dir` | `TEST-<component>.<Spec>.xml` per spec |
| `--serve [address]` | When the specs pass, serve the scratch database on the address (`127.0.0.1:0`, a free port, by default; or a port alone) until Ctrl-C, SIGTERM, or the end of standard input when it is a pipe; then remove the scratch runtime unless `--keep`. With `--db existing` it serves the runtime's own database, for a runtime whose cluster was put in place beforehand (a copy of a loaded, stopped cluster in `db/embedded` starts in a fraction of a second, where a load or `--db snapshot` restore takes seconds to minutes); with no specs to run (each component named has none) the server starts at once, with no test factory booted before it. Not with `--db none`. For client specs ([JS-Test-Client](dev/JS-Test-Client.md#--serve)) or a person |
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

The API a spec is written with is in [JS-Testkit](dev/JS-Testkit.md):

- [`spec({...})`](dev/JS-Testkit.md#spec) and [`feature(name, {...})`](dev/JS-Testkit.md#featurename-) with their
  fields, the blocks and `ctx`, [`where` rows](dev/JS-Testkit.md#where-rows), and what is
  [skipped or ignored](dev/JS-Testkit.md#skipped-and-ignored);
- [conditions](dev/JS-Testkit.md#conditions): every expression statement of `then` and `expect` is one, compared
  as Groovy does, and [`assert`](dev/JS-Testkit.md#assert) for the places the rewrite does not reach;
- [the helpers](dev/JS-Testkit.md#helpers): `call`, `checkData`, `login`, `screenTest`, `parallel`, `uploadedFile`,
  `setProperty`, `withProperty`, `log`, `skip`, `dec`, `sum`, `runInContext`, and
  [the context stack](dev/JS-Testkit.md#the-context-stack);
- [specs with `database: false`](dev/JS-Testkit.md#specs-with-database-false);
- <a id="http-fixtures"></a>the capture tools: [mail](dev/JS-Testkit.md#mail),
  [HTTP fixtures](dev/JS-Testkit.md#http-fixtures) and [notifications](dev/JS-Testkit.md#notifications);
- [agent specs](dev/JS-Testkit.md#agent-specs), [porting a Spock spec](dev/JS-Testkit.md#porting-a-spock-spec) and
  [pitfalls](dev/JS-Testkit.md#pitfalls).

Client specs are in [JS-Test-Client](dev/JS-Test-Client.md) and load scripts in [JS-Test-Load](dev/JS-Test-Load.md).

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

A task file calls `agentSpec({...})` with its agent, surface, user and tasks; each task has what the user says, the
scripted approvals and answers, and its checks. The fields and the checks are in
[JS-Testkit, Agent specs](dev/JS-Testkit.md#agent-specs).

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
