# Modern Rust Moqui Framework - MR Moqui

## Quick Start

```sh
./moq build   # The Default package (docs/Packaging.md); --release for every feature; from full source
./moq load    # Load all data, including demo; skip this and production data loads on first run
./moq run     # Start moqui; logs to console, ctrl-c to stop; default ports 8080 and 8443
```

This builds the 'lite' binary and uses it to load data and run. Add `--release` to each of the 3 commands to build and use the release binary with the full toolset included: SSH/SFTP, OAuth2/OIDC, SMTP/IMAP/POP, Git, XLSX, and image processing.

## Status

Initial implementation of the framework is complete and various applications are functional. It is still in heavy development for full Moqui Java feature completeness, additional features new to MR Moqui, testing & bugfixes, and profiling & optimization.

**Mantle UDM** and **Mantle USL** are converted from Groovy to JavaScript in new fork repositories: `mr-mantle-udm` and `mr-mantle-usl`.

**SimpleScreens** and **MarbleERP** are being converted (not yet complete) and are also in new fork repositories: `MrSimpleScreens` and `MrMarbleERP`.

There is a new component called **IronSide Business OS Suite** (`ironstone-boss`) that is being designed and implemented (neither are complete). A high-level design can be seen in the `plan/initial-design.md` file.

The NextCloud-inspired **Commons & Comms** applications for groupware and communication have been designed and are being built first. These are a major upgrade to the long-neglected My Account app, fill some long-standing gaps in Moqui, and along with other new framework features and application functionality are intended for full cloud service replacement.

The `moq` script replaces our old Gradle tasks, and getting components is similar:

```sh
./moq component-get MrMarbleERP
./moq component-get ironstone-boss
```


## Goals

- Keep XML as the spec language for configuration, entities, services, screens, SECAs, and data files, with only the adaptations required by a Rust runtime.
- Move the heavy lifting (entity facade, service facade, screen rendering, tooling, and performance-sensitive service implementations) into Rust.
- Use one language (JavaScript) for scripts and templates on the server, and for inline expressions on the client, so that XML Screens can become client-renderable. The engine is QuickJS.
- Default to an embedded Postgres (`postgresql_embedded`, theseus) when no external database is configured, and use an external Postgres when one is. The same engine backs the sim-mode overlay.
- Make OpenSearch optional from day one by planning a search interface that can be satisfied by Postgres full-text search and JSON operators.
- Preserve the ExecutionContext / facade shape that makes Moqui’s artifact model work.

## Significant Redesigns vs Moqui Java

Most of MR-Moqui is a port of the Java framework as it exists when the port begins. These are the places that change on purpose.

1. **JavaScript for scripts and templates.** Scripts, XML action and screen macros, and text templates are JavaScript, initially on QuickJS. `ec.resource.expand()` is a template literal. Structured text uses a second renderer: `if`, `list`, and `assign`, compiled to one cached JavaScript function. A locked-down variant of that renderer is the sandbox for less-trusted templates. Plan: [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md) (Templating).
2. **SearchFacade replaces ElasticFacade.** Postgres full-text and JSON are the default backend. OpenSearch is optional. Plan: [`plans/archive/SearchFacade.md`](plans/archive/SearchFacade.md).
3. **qvt becomes server-static.** With `server-static` set for `qvt`, the server sends a data-free Vue template and the client merges data from `/actions` and from a separate form-list request. The attribute stays opt-in for this pass. Plan: [`plans/archive/QvtServerStatic.md`](plans/archive/QvtServerStatic.md).
4. **Print HTML is its own render mode.** Server-side page layout is `html-print` (print CSS and `@page`). The old `xsl-fo` name selects that mode. One long-lived external Chromium process prints it to PDF, through the host's own DevTools client; resource images are inlined first, and `/fop/<path>` sends a screen as PDF, as Java's FOP servlet did. Plan: [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md) (Tool components and print).
5. **Cluster coordination uses the shared database.** Instances find each other in a member table. Cache invalidation and notifications fan out over HTTPS, and that endpoint requires a client certificate whose key material is deployment configuration (member rows hold only key fingerprints). Service jobs stay on the database. There is no Hazelcast, Redis, or session-byte replication. Plan: [`plans/archive/Cluster.md`](plans/archive/Cluster.md).
6. **Sessions live in the database.** The session is `moqui.server.HttpSession`, cached through the entity one-cache and invalidated on the existing cluster topic. A flush overlays dirty fields onto the latest row and retries on `sessionRevision`. The default flush is eventually consistent. Guarantee mode reads the primary and flushes at login, logout, and the end of the request, so a load balancer without stickiness can fail over. Plan: [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md) (Sessions and login) and [`plans/archive/Cluster.md`](plans/archive/Cluster.md).
7. **Keys are configuration, and unset secrets are generated.** Secret key values (field encryption, session-cookie signing, the cluster certificate pair, later the SSH host key) are `default-property`s in the `entity_ds_crypt_pass` pattern: a `CHANGEME` value in `MoquiDefaultConf.xml` set by config file or environment, never database rows. A `runtime/.env` file in the root of the runtime directory is loaded at startup like environment variables and supports any conf property. As in Java, `default-property` is kept for what a deployment commonly sets (hosts, ports, URLs, credentials, secrets), named by a conf attribute as `${name}`; any other setting is a plain attribute of its element (`git-server`, `ssh-server`, `email-client` and so on), changed in a runtime or component conf. A secret still at its `CHANGEME` default is generated with the OS CSPRNG and saved to `runtime/.env`, instead of running on the shipped default. Plan: [`plans/archive/MimoReview.md`](plans/archive/MimoReview.md) (Decisions 1–3).
8. **Feed registration commits with the entity change.** Entity create, update, and delete register document keys during the transaction. After the transaction-cache flush and before commit, on the enlisted connection, the feed inserts a pending row. Rollback drops it. Assembly and the index write run after commit on the service worker pool, for same-database Postgres, a second Postgres, and OpenSearch. An index failure leaves the pending row and does not roll the entity change back. Plan: [`plans/archive/SearchFacade.md`](plans/archive/SearchFacade.md).

## Native components

Components can supply linked Rust services, tools, resource schemes, template renderers and custom service runners,
as a Gradle build did for a Java Moqui component: a `Cargo.toml` and `src/` beside the `component.xml`, which
`./moq build` links into the binary. Service XML still defines services; component configuration selects
implementations, and scripts get tools with `ec.factory.getTool("Name")`. See
[Native component usage](docs/Native-Components.md), the [plan](plans/archive/Native-Components.md), and
`runtime/base-component/example`, which also holds the screen archetype gallery (`/qapps/gallery`); it is on by default and
`./moq run -D example_disabled=true` leaves it out.

## Documentation

Reference guides for built features are in [`docs/`](docs/). Design, status and review documents are in [`plans/`](plans/), indexed by [`plans/README.md`](plans/README.md): what is still open, what comes next, the proposals, and the archive of built designs and finished reviews.

## Build, Test, and Run

Very basic instructions from a fresh checkout. A Run & Deploy guide will come later.

Prerequisites: Rust 1.94 or newer (`rustc --version`). A build with embedded Postgres embeds the
18.6.0 archive that `scripts/pg-payload.sh strip` downloads once and strips of psql (it links GPL
readline); `./moq build` runs that when the archive is missing, and a plain `cargo build` asks for it.
Node.js is optional; the JS client tests run under `cargo test` only when `node` is on the path.

The build packages are in [`docs/Packaging.md`](docs/Packaging.md): Default (`./moq build`, the web server on
embedded Postgres, PDF and SMTP), Debug (`--debug`), Maximal (`--release`, every feature) and Enterprise
(`--enterprise`, every feature with the databases external), plus the browser build.

```sh
scripts/pg-payload.sh strip            # once: the Postgres archive the binary embeds, without psql
cargo build -p moqui                   # Debug, every feature: target/debug/moqui
cargo build --profile lite --no-default-features -F lite -p moqui     # Default: target/lite/moqui
cargo build --release -p moqui         # Maximal: target/release/moqui
scripts/pg-payload.sh check            # before a distribution: no psql or readline in what is embedded

# Tests (whole workspace, every feature) and lints
cargo test
cargo lint
scripts/check-packages.sh              # clippy on each package's feature set

# The Gate 3 tests, left out of the default run (embedded Postgres per test, Chromium: ./moq chromium-download, or
# pdf_chromium_path, or chromium on the PATH)
cargo test -p moqui-host --test gate3 --test gate3_browser
```

Run from the repo root (paths default to `MOQUI_RUNTIME=runtime`, or the binary's own directory when it is a runtime
directory, embedded Postgres data in `runtime/db/embedded`, and the
binary unpacked into `runtime/install/postgres`, or under `MOQUI_INSTALL`).
Setting `entity_ds_db_conf` to `postgres`, `mysql` or `mysql8`, with the other `entity_ds_*`
properties, uses that server instead. The examples are in `runtime/conf/MoquiDevConf.xml`.

The `./moq` script runs the built binaries in place and replaces the Java Moqui Gradle
tasks (run, load, save and reload of the database, clean, OpenSearch, Chromium for PDF, git across all repos,
and component get/create/zip). It needs only bash (3.2 or newer), so it works on a fresh
checkout on Linux or macOS. `./moq` with no arguments lists the tasks, and
`./moq help <task>` describes one. Task names match by partial parts, ignoring case, so
`./moq r-p` and `./moq runProduction` both mean `run-production`.

```sh
./moq build                    # the Default package, target/lite/moqui (--debug, --release, --enterprise for
                               # the others), linking any component crates in runtime/component; tasks that
                               # run moqui build a missing binary themselves
./moq run                      # target/lite/moqui with runtime/conf/MoquiDevConf.xml; --release uses target/release/moqui
./moq run -p 8081 -D entity_ds_db_conf=postgres
./moq run lint --screens       # moqui's own commands, with the same conf
./moq load                     # all data types; ./moq load seed seed-initial install
./moq test mantle-usl         # run a component's JavaScript specs; extra args go to moqui test (--spec, --db snapshot)
./moq save-db before-upgrade   # and later: ./moq reload-save before-upgrade
./moq archive                  # mr-moqui-YYYYMMDD.zip (or --tgz): runtime/ without logs, with the binary in it
./moq git-status               # or any git command in every repo: ./moq git pull
./moq completion --install     # tab completion for bash or zsh
```

`-D create-or-update=true` opts into Java's read-first create-or-update (SELECT, then
CREATE or UPDATE). It defaults to off, preserving Rust's UPDATE-first optimization.

With no arguments the binary starts the HTTP server (HTTP on `8080` by default, HTTPS on `8443`, cluster
HTTPS on `8444`) using `runtime/base-component/framework/conf/MoquiDefaultConf.xml` merged with the runtime conf:
the file `MOQUI_CONF` names, relative to the runtime directory or absolute (`./moq run -c FILE` sets it); else the
instance's `runtime/MoquiConf.xml` when present; else `runtime/conf/MoquiDevConf.xml` when present. `MOQUI_CONF` comes from the
environment, not `runtime/.env`, which is read after the conf merge. Without the script,
`moqui help` lists the commands and `moqui help load` describes one:

```sh
cargo run -p moqui
# or: ./target/debug/moqui, and ./target/debug/moqui load for the default seed, seed-initial, install data
# ./target/debug/moqui --port 8081 --conf conf/MoquiProductionConf.xml
```

HTTPS certificates are looked up by the requested domain in `runtime/cert` (git-ignored), in any of these layouts:
nginx-proxy's `<domain>.crt` and `<domain>.key` (`dhparam.pem` files are ignored), certbot's or acme-companion's
`<domain>/fullchain.pem` and `privkey.pem`, or mkcert's `<domain>.pem` and `<domain>-key.pem`. A subdomain falls back to
`_wildcard.<parent>` (mkcert) and then `<parent>` (nginx-proxy), one label up only, and then `default` serves any
other name. A domain with no certificate gets a refused handshake. Files are read again within seconds of a change, so
renewals need no restart. In dev (`instance_purpose=dev`) an empty directory gets a self-signed `localhost`
certificate. Outside dev with no certificate, the HTTPS listener is not started, or the start fails when the port is set
explicitly. `--https-port N` or `webapp_https_listen_port` sets the port to listen on, and `off` disables it.

`webapp_https_port` is separate: it is the port clients see in generated URLs and https redirects (empty is 443), as in
Java Moqui. To make generated URLs use https, set `webapp_https_enabled=true`. Connecting straight to this listener, also
set `webapp_https_port=8443`. Behind a reverse proxy on 443, leave it empty.

The cluster listener's default port moved from `8443` to `8444` so the public HTTPS listener could have `8443`. Update
firewall rules and port mappings for members that do not set `cluster_https_port`.

For production packaging, `./moq archive` (with `--release` or `--enterprise` for those packages;
[`docs/Packaging.md`](docs/Packaging.md)) writes a zip, or with `--tgz` a gzipped tar, that expands to one directory:
the contents of `runtime/` (the framework component is inside it) without its logs, scratch files, saves and unpacked
Postgres, with the binary and `runtime/conf/MoquiProductionConf.xml` as `MoquiConf.xml` in it. The binary uses its own
directory as the runtime when that has `base-component/`, and merges the `MoquiConf.xml` there, so it runs as
`<dir>/moqui` from anywhere; `--no-data` leaves out the database and secrets. By hand: copy the binary into the runtime
directory, with a `MoquiConf.xml` there or `MOQUI_CONF=conf/MoquiProductionConf.xml`. From a checkout, `./moq run-production` runs the Default binary with the
production conf, or another with `--release` or `--enterprise`. `moqui version` prints a binary's package and
features.

## AI/LLM Policy

Moqui was originally designed and built largely to reduce the human pain and suffering involved with building and maintaining enterprise applications. The same philosophy applies to the world of AI, from LLMs to harnesses, and from harnesses used to make Moqui to those built into it.

This project is an AI-assisted rebuild of the manually written Moqui Framework in Rust. It would likely never be done without AI assistance migrating and filling in code. AI assistance to migrate existing components made for Moqui is also recommended. Without that being available, migration from Groovy to JavaScript would be a much more expensive thing to do and it is much less likely we'd ever do it.

LLMs are considered tools used by people. The person using the tool is just as responsible as with a text editor and keyboard. LLMs are the functional and moral equivalent of a hammer: they do not shake themselves, and are not a substitute or replacement for a responsible party.

Don't trust, verify. Use AI to help verify, but remember who is responsible, and who is a who.
