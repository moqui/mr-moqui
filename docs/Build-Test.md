# Build and Test

How to build MR Moqui from source, the build packages and their features, the `./moq` tasks, the Rust tests and lints,
and the component specs that `moqui test` runs. Running and deploying are in [Run-Deploy](Run-Deploy.md), and the specs
that test a running server from outside are in [Client-Testing](dev/Client-Testing.md).

- [Build](#build)
- [Packages](#packages): [features](#features), [profiles](#profiles), the [embedded Postgres
  archive](#embedded-postgres-archive), the [instance archive](#instance-archive), [build cost](#build-cost)
- [The moq script](#the-moq-script)
- [Rust tests and lints](#rust-tests-and-lints)
- [Component specs](#component-specs): [running](#running), [a spec file](#a-spec-file),
  [conditions](#conditions), [helpers](#helpers), [mail](#mail), [notifications](#notifications), [agent
  evaluation](#agent-evaluation), [porting a Spock spec](#porting-a-spock-spec), [pitfalls](#pitfalls)

## Build

Prerequisites: Rust 1.95 or newer (`rustc --version`). A build with embedded Postgres embeds the 18.6.0 archive that
`scripts/pg-payload.sh strip` downloads once and strips of psql (it links GPL readline); `./moq build` runs that when
the archive is missing. Node is not needed to build or test.

```sh
./moq build                            # the Dev package, target/develop/moqui; --lite, --debug, --release or
                                       # --enterprise for the others; links any component crates in runtime/component
scripts/pg-payload.sh strip            # once: the Postgres archive the binary embeds, without psql
cargo build -p moqui                   # Debug, every feature: target/debug/moqui
cargo build --profile develop --no-default-features -F dev -p moqui   # Dev: target/develop/moqui
cargo build --profile lite --no-default-features -F lite -p moqui     # Lite: target/lite/moqui
cargo build --release -p moqui         # Maximal: target/release/moqui
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
| Dev | (none) | `develop` | `target/develop/moqui` | Lite's features with git hosting (`git`), the SSH server (`ssh`) and the dev tools (`devtools`) |
| Lite | `--lite` | `lite` | `target/lite/moqui` | The web server on embedded Postgres, PDF (through Chromium), outbound mail (SMTP), the terminal client (`moqui tui`) |
| Debug | `--debug` | `dev` | `target/debug/moqui` | Every feature and the dev tools, unoptimized framework code with debug info |
| Maximal | `--release` | `release` | `target/release/moqui` | Every feature, Postgres embedded |
| Enterprise | `--enterprise` | `enterprise` | `target/enterprise/moqui` | Every feature, with the databases external (Postgres and MySQL drivers, no embedded Postgres) |
| Browser | `scripts/wasm-build.sh [--release]` | `dev` / `release` | `target/browser/wasm-pkg/<profile>` | The framework on SQLite in a Web Worker; none of the optional features |

Plain `cargo build -p moqui`, `cargo test` and `cargo lint` use the Maximal feature set with `devtools`, so they cover
everything.

The Dev package is the default since development wants git and SSH (the dev-aide, the hosting, the SSH apps), which
Lite leaves out, and the packages a deployment uses should not carry the dev tools. Its profile has Lite's settings
and is named `develop`, since `dev` is Cargo's own debug profile, which the Debug package uses. A deployment builds
Lite, Maximal or Enterprise; `./moq build --release -- --features devtools` adds the dev tools to another package.

### Features

The framework features are in `crates/moqui-core`; the host features, and the packages, are in `crates/moqui-host`;
`crates/moqui` (and the crate `./moq build` generates in `target/moqui-app` for add-on component crates) forwards the
package features `dev`, `lite`, `enterprise` and `maximal`, `devtools`, and `inbound-smtp` for a lite build that wants
one. The agents are the `moqui-llm` crate, on `moqui-core`, which the host's `llm` feature links; core reaches them
through `moqui_core::llm_hooks`, so in a binary without them their services report that `llm` is not built in.

| Feature | Crate | What it gates | Dev | Lite | Enterprise | Maximal |
|---|---|---|---|---|---|---|
| `axum` | host | The HTTP server, its listeners and routes | yes | yes | yes | yes |
| `postgres` | host | The Postgres driver | yes | yes | yes | yes |
| `embedded-postgres` | host | The embedded Postgres clusters (entity, search, sim), `save-db`, `load-save`, backups, and `moqui test` when it opens a database (`--db fresh`, `snapshot`, or `existing`). `--db none` does not need it | yes | yes | | yes |
| `mysql` | host | The MySQL driver | | | yes | yes |
| `pdf` | host | The DevTools client of the external Chromium that prints `html-print` to PDF | yes | yes | yes | yes |
| `ssh` | host | The SFTP client, the built-in SSH server (SFTP, admin shell, git over SSH, and `apps` with `tui`), the SSH cluster channel | yes | | yes | yes |
| `sso` | host | OIDC and OAuth 2 logins (`org.moqui.sso.AuthServices`) | | | yes | yes |
| `print` | host | Network printing over IPP (`moqui-cups`) | | | yes | yes |
| `tui` | host | `moqui tui`, the terminal client of the `tui` render mode, on Revue ([TUI](ui/TUI.md)), and with `ssh` the SSH `apps` command; in lite too, for a quick terminal loop while developing and testing. Alone (`cargo build -p moqui-host --no-default-features --features tui`) it is the client of a running server, `moqui tui --server <url>`, with no database driver or HTTP server in the build | yes | yes | yes | yes |
| `test-client` | host | `moqui test-client`: a component's `test/client` specs against a running server, over HTTP, raw TCP or TLS, WebSocket and headless Chromium, with a load runner ([Client-Testing](dev/Client-Testing.md)); it opens no database and binds no listener of its own, so any package can test any server. Its `--serve` starts `moqui test --serve`, which needs `embedded-postgres` | yes | yes | yes | yes |
| `inbound-smtp` | host | The SMTP listener for hosted and shared mail accounts behind the edge gateway ([Mail-Inbound](interfaces/Mail-Inbound.md); includes `imap`); it starts only when `smtp_listen_port` is set | | | yes | yes |
| `devtools` | host, core | What only development needs ([design-dev-aide](../plans/design-dev-aide.md) 3.3): the framework's file watcher that reloads changed artifacts on a dev instance, and in the binary crates the dev-component crates of `runtime/dev-component`; in Dev and Debug | yes | | | |
| `smtp` | core | Outbound mail: `send#EmailMessage`, `send#EmailTemplate`, password reset and notification mail | yes | yes | yes | yes |
| `imap` | core | Inbound mail: `poll#EmailServer`, the mail client's sync, IDLE, rules and import (includes `smtp`) | | | yes | yes |
| `git` | core | `git://` locations, git drives, `org.moqui.impl.GitServices`, the git server over HTTP and SSH, LFS | yes | | yes | yes |
| `image` | core | `org.moqui.impl.ImageServices` (`convert#Image`) and elFinder thumbnails | | | yes | yes |
| `xlsx` | core | The `xlsx` render mode and `org.moqui.impl.SpreadsheetServices` | | | yes | yes |
| `llm` | host | The agents and the model providers (the `moqui-llm` crate, `org.moqui.agent.AgentAdminServices`; [LLM-A2A-Assist](../plans/LLM-A2A-Assist.md)) | yes | yes | yes | yes |
| `http`, `cluster-tls` | core | Remote REST, `http` locations, the OpenSearch client; the cluster certificate | yes | yes | yes | yes |

`mail` is `smtp` and `imap` together. The browser build has none of the core features.

#### When a feature is left out

The definitions still load, so components, screens and service files are the same in every package:

- A service whose Rust implementation belongs to a missing feature fails when called, with
  `service <name>: <feature> is not built into this host` in the message facade (`rust_services::missing_feature`).
- The host integrations report through the framework's existing messages: SFTP and SSO calls say no client is
  installed, network printing says it is not installed, and `cluster_transport=ssh` cannot start without the SSH
  server. A cluster peer that advertises `ssh://` gets an error naming the missing feature.
- An `xlsx` render fails with `xlsx is not built into this host`; an elFinder thumbnail is left out; a `git://`
  location has no provider, and `/git/...` is not routed.
- gitECA rules are still read and checked; nothing runs them, since there are no git events.
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

- `dev` (Debug): the workspace crates at opt-level 0 with debug info, the dependencies at opt-level 3.
- `lite` (Lite): `release` with `incremental = true` and 256 codegen units for the workspace and path crates
  (Cargo applies `incremental` only to those), so a rebuild after an edit to the framework is quick; the dependencies
  keep the release settings and are built once.
- `develop` (Dev): `lite`'s settings in a profile of its own, so its binary is `target/develop/moqui` beside Lite's.
- `release` (Maximal) and `enterprise`: `panic = "unwind"` (services unwind across the service boundary), otherwise
  Cargo's release defaults.

### Embedded Postgres archive

The `embedded-postgres` feature embeds the theseus Postgres archive (the version `.cargo/config.toml` forces through
`POSTGRESQL_VERSION`) and unpacks it once into `install/postgres/<version>` in the runtime directory (or under
`MOQUI_INSTALL`), shared by the entity, search and sim clusters. An install an earlier build unpacked in
`runtime/db/install` is moved there on the first start. `crates/moqui-host/build.rs` stages it from `~/.theseus/postgresql/postgresql-<version>-<target>.tar.gz`, or
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

## The moq script

The `./moq` script runs the built binaries in place and replaces the Java Moqui Gradle tasks: run, load, save and reload
of the database, clean, OpenSearch, Chromium for PDF, git across all repositories, and component get, create and zip.
It needs only bash (3.2 or newer), so it works on a fresh checkout on Linux or macOS. `./moq` with no arguments lists
the tasks, and `./moq help <task>` describes one. Task names match by partial parts, ignoring case, so `./moq r-p` and
`./moq runProduction` both mean `run-production`.

```sh
./moq build                    # the Dev package, target/develop/moqui (--lite, --debug, --release, --enterprise for
                               # the others), linking any component crates in runtime/component; tasks that
                               # run moqui build a missing binary themselves
./moq run                      # target/develop/moqui with runtime/conf/MoquiDevConf.xml; --release uses target/release/moqui
./moq run -p 8081 -D entity_ds_db_conf=postgres
./moq run lint --screens       # moqui's own commands, with the same conf
./moq load                     # all data types; ./moq load seed seed-initial install
./moq test mr-mantle           # run a component's JavaScript specs; extra args go to moqui test (--spec, --db snapshot)
./moq test-client framework    # a component's test/client specs against the running server (docs/dev/Client-Testing.md)
./moq test-all                 # every suite: cargo (with the opt-in host tests), moq, moqui test, test-client --serve
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
# pdf_chromium_path, or chromium on the PATH)
cargo test -p moqui-host --test gate3 --test gate3_browser
```

`scripts/check-packages.sh` runs Clippy with warnings denied on each feature set a build uses: the workspace
(Maximal with the dev tools, as `cargo lint`), the Dev, Lite, Maximal and Enterprise packages, the host with no features
(`cargo lint-min`) and with only the terminal client, and the framework with none (what the browser links). Then it
runs the scripted TUI screens, which `cargo test` leaves out. A gate that only one package sees still has to compile
there.

`./moq test-all` runs every suite in turn and lists which passed: `cargo test`, the opt-in host tests (Gate 3, the
browser and TUI screen tests), `scripts/moq-test.sh` (the tests of `./moq`), `moqui test` for every component, the demo component's specs (it is
off by default, so the suite turns it on), and the client specs. `./moq help test-all` lists them.

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
./moq test mr-mantle                                  # the Dev binary and the dev conf
moqui test mr-mantle                                  # every spec, fresh database
moqui test mr-mantle --db snapshot                    # load once, restore the saved dump on later runs
moqui test mr-mantle --spec OrderToCashBasicFlow      # one spec and the specs its dependsOn names
moqui test mr-mantle --spec OrderToCashBasicFlow --feature "ship Sales Order"
moqui test mr-mantle --junit build/test-results       # JUnit XML for CI
moqui test mr-mantle --coverage                       # the services no spec called, per service file
moqui test mr-mantle --keep --log-level info          # keep the scratch runtime, more on the console
```

With no component, every component that has `test/*.spec.js` runs, in load order. `./moq test --debug` uses the debug
binary.

| Option | |
|---|---|
| `--spec Name[,Name]` | The specs to run, by file name without `.spec.js`, with the specs their `dependsOn` names, in suite order. A `dependsOn` name the spec's own component lacks is found in the run's other components |
| `--feature text` | Only the features whose names contain the text |
| `--db fresh` | The default: load every data type into a scratch runtime, removed at the end unless `--keep`. Needs `embedded-postgres` |
| `--db snapshot` | As fresh, but save the load under `runtime/save` and restore it while the entity and data files and the binary are unchanged |
| `--db existing` | The runtime's embedded database as it is. The specs change its data |
| `--db none` | Boot the factory and skip Postgres, so `ec.resource` works and the entity facade stays unset. A build without `embedded-postgres` can run this. Every selected spec must set `database: false`; one that does not is named and the run stops before boot. When every selected spec sets it, and the run is not `--serve`, `--agents`, `--skills`, or `--coverage`, this is the default |
| `--junit dir` | `TEST-<component>.<Spec>.xml` per spec |
| `--serve [address]` | When the specs pass, serve the scratch database on the address (`127.0.0.1:0`, a free port, by default; or a port alone) until Ctrl-C, SIGTERM, or the end of standard input when it is a pipe; then remove the scratch runtime unless `--keep`. Not with `--db existing` or `--db none`. For client specs ([Client-Testing](dev/Client-Testing.md#--serve)) or a person |
| `--ready-file path` | With `--serve`: write the server's URL here once it listens |
| `--coverage`, `--coverage-report file` | The services of the components' `service/*.xml` that no spec called, with the total; the report adds call counts |
| `--log-level level` | Console level, `warn` by default. Everything goes to `runtime/log/moqui.log`, whose path the run prints |

The console has a line per feature, and the failures in full at the end. Any failed feature exits 1.

### Files and order

A spec is `<component>/test/<Name>.spec.js`. Its name is the file name. `test/suite.js`, `suite(["SpecA", "SpecB"])`,
gives the order; the other spec files run after it, by name. Each spec runs in its own execution context, on the one
database of the run, so a spec sees what the specs before it wrote.

A component's `test/MoquiConf.xml` merges after its `MoquiConf.xml` in `moqui test` runs only, for a test-only setting
or mount. `test/client/` holds client specs, which `moqui test` does not read.

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
  are not started without their setup.
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

The source is rewritten for this before it runs (`crates/moqui-core/src/testkit/rewrite.rs`), with no new lines, so error
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
