# Modern Rust Moqui Framework - MR Moqui

## Goals

- Keep XML as the spec language for configuration, entities, services, screens, SECAs, and data files, with only the adaptations required by a Rust runtime.
- Move the heavy lifting (entity facade, service facade, screen rendering, tooling, and performance-sensitive service implementations) into Rust.
- Use one language (JavaScript) for scripts and templates on the server, and for inline expressions on the client, so that XML Screens can become client-renderable. The engine is QuickJS.
- Default to an embedded Postgres (`postgresql_embedded`, theseus) when no external database is configured, and use an external Postgres when one is. The same engine backs the sim-mode overlay.
- Make OpenSearch optional from day one by planning a search interface that can be satisfied by Postgres full-text search and JSON operators.
- Preserve the ExecutionContext / facade shape that makes Moqui’s artifact model work.

## Significant Redesigns

Most of MR-Moqui is a port of the Java framework as it exists when the port begins. These are the places that change on purpose.

1. **SearchFacade replaces ElasticFacade.** Postgres full-text and JSON are the default backend. OpenSearch is optional. Plan: [`plans/archive/SearchFacade.md`](plans/archive/SearchFacade.md).
2. **qvt becomes server-static.** With `server-static` set for `qvt`, the server sends a data-free Vue template and the client merges data from `/actions` and from a separate form-list request. The attribute stays opt-in for this pass. Plan: [`plans/archive/QvtServerStatic.md`](plans/archive/QvtServerStatic.md).
3. **Feed registration commits with the entity change.** Entity create, update, and delete register document keys during the transaction. After the transaction-cache flush and before commit, on the enlisted connection, the feed inserts a pending row. Rollback drops it. Assembly and the index write run after commit on the service worker pool, for same-database Postgres, a second Postgres, and OpenSearch. An index failure leaves the pending row and does not roll the entity change back. Plan: [`plans/archive/SearchFacade.md`](plans/archive/SearchFacade.md).
4. **JavaScript for scripts and templates.** Scripts, XML action and screen macros, and text templates are JavaScript, initially on QuickJS. `ec.resource.expand()` is a template literal. Structured text uses a second renderer: `if`, `list`, and `assign`, compiled to one cached JavaScript function. A locked-down variant of that renderer is the sandbox for less-trusted templates. Plan: [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md) (Templating).
5. **Print HTML is its own render mode.** Server-side page layout is `html-print` (print CSS and `@page`). The old `xsl-fo` name selects that mode. One long-lived external Chromium process prints it to PDF, through `chromiumoxide`. An off-by-default Servo build is the in-process alternate where a child process is forbidden. Plan: [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md) (Tool components and print).
6. **Cluster coordination uses the shared database.** Instances find each other in a member table. Cache invalidation and notifications fan out over HTTPS, and that endpoint requires a client certificate whose key material is deployment configuration (member rows hold only key fingerprints). Service jobs stay on the database. There is no Hazelcast, Redis, or session-byte replication. Plan: [`plans/archive/Cluster.md`](plans/archive/Cluster.md).
7. **Sessions live in the database.** The session is `moqui.server.HttpSession`, cached through the entity one-cache and invalidated on the existing cluster topic. A flush overlays dirty fields onto the latest row and retries on `sessionRevision`. The default flush is eventually consistent. Guarantee mode reads the primary and flushes at login, logout, and the end of the request, so a load balancer without stickiness can fail over. Plan: [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md) (Sessions and login) and [`plans/archive/Cluster.md`](plans/archive/Cluster.md).
8. **Keys are configuration, and unset secrets are generated.** Secret key values (field encryption, session-cookie signing, the cluster certificate pair, later the SSH host key) are `default-property`s in the `entity_ds_crypt_pass` pattern: a `CHANGEME` value in `MoquiDefaultConf.xml` set by config file or environment, never database rows. A `runtime/.env` file in the root of the runtime directory is loaded at startup like environment variables and supports any conf property. A secret still at its `CHANGEME` default is generated with the OS CSPRNG and saved to `runtime/.env`, instead of running on the shipped default. Plan: [`plans/archive/MimoReview.md`](plans/archive/MimoReview.md) (Decisions 1–3).

## Native components

Components can supply linked Rust services, tools, resource schemes, template renderers and custom service runners,
as a Gradle build did for a Java Moqui component: a `Cargo.toml` and `src/` beside the `component.xml`, which
`./moq build` links into the binary. Service XML still defines services; component configuration selects
implementations, and scripts get tools with `ec.factory.getTool("Name")`. See
[Native component usage](plans/Native-Components-Usage.md), the [plan](plans/Native-Components.md), and
`runtime/base-component/example`, which is off by default (`./moq run -D example_disabled=false`).

## Build, Test, and Run

Very basic instructions from a fresh checkout. A Run & Deploy guide will come later.

Prerequisites: Rust 1.94 or newer (`rustc --version`). The first build downloads the
embedded-Postgres 18.6.0 archive. Node.js is optional; the JS client tests run under
`cargo test` only when `node` is on the path.

```sh
# Development build, binary at target/debug/moqui (for work on the compiled framework;
# ./moq runs it with --debug)
cargo build -p moqui

# Release build, binary at target/release/moqui, which ./moq uses by default. Strip psql (it links GPL readline)
# from the Postgres archive the binary embeds first, and check the result.
scripts/pg-payload.sh strip
cargo build --release -p moqui
scripts/pg-payload.sh check

# Tests (whole workspace) and lints
cargo test
cargo lint

# The Gate 3 tests, left out of the default run (embedded Postgres per test, Chromium)
cargo test -p moqui-host --test gate3 --test gate3_browser
```

To reuse compatible Cargo artifacts across checkouts, export one absolute target path
in the shell or agent environment, for example:

```sh
export CARGO_TARGET_DIR=/home/user/moqui/mr-moqui/target
./moq run --debug
```

Shared-cache mode additionally requires Python 3. `moq` serializes build-and-copy
and runs a private executable at `target/moq-bin/<profile>/moqui`. On checkout
changes it invalidates local/path packages only: Cargo can otherwise incorrectly
report another checkout's outputs fresh. Registry/git dependencies stay cached;
incremental builds remain enabled within the same checkout. Generated add-on source
(`target/moqui-app`) and runtimes stay checkout-local.

Use `moq` or the regression controller for every build into this shared target;
bare `cargo build` bypasses the checkout guard and build-and-copy lock. Keep compiler
versions and build flags consistent for best reuse. Existing local caches are not
removed automatically; `cargo clean` also clears the shared cache. Verify with
`python3 scripts/cargo-cache-test.py` (small, offline Rust fixtures).

Run from the repo root (paths default to `MOQUI_RUNTIME=runtime`,
`MOQUI_FRAMEWORK=<runtime>/base-component/framework`, embedded cluster in `runtime/db/embedded`).
Setting `entity_ds_db_conf` to `postgres`, `mysql` or `mysql8`, with the other `entity_ds_*`
properties, uses that server instead. The examples are in `runtime/conf/MoquiDevConf.xml`.

The `./moq` script runs the built binaries in place and replaces the Java Moqui Gradle
tasks (run, load, save and reload of the database, clean, OpenSearch, git across all repos,
and component get/create/zip). It needs only bash (3.2 or newer), so it works on a fresh
checkout on Linux or macOS. `./moq` with no arguments lists the tasks, and
`./moq help <task>` describes one. Task names match by partial parts, ignoring case, so
`./moq r-p` and `./moq runProduction` both mean `run-production`.

```sh
./moq build                    # cargo build --release -p moqui (--debug for target/debug/moqui), linking
                               # any component crates in runtime/component; tasks that run moqui build a
                               # missing binary themselves
./moq run                      # target/release/moqui with runtime/conf/MoquiDevConf.xml; --debug uses target/debug/moqui
./moq run -p 8081 -D entity_ds_db_conf=postgres
./moq run lint --screens       # moqui's own commands, with the same conf
./moq load                     # all data types; ./moq load seed seed-initial install
./moq test mantle-usl         # run a component's JavaScript specs; extra args go to moqui test (--spec, --db snapshot)
./moq save-db before-upgrade   # and later: ./moq reload-save before-upgrade
./moq git-status               # or any git command in every repo: ./moq git pull
./moq completion --install     # tab completion for bash or zsh
```

With no arguments the binary starts the HTTP server (HTTP on `8080` by default, HTTPS on `8443`, cluster
HTTPS on `8444`) using `runtime/base-component/framework/conf/MoquiDefaultConf.xml` merged with the runtime conf:
`runtime/conf/MoquiDevConf.xml` when present, or the file `MOQUI_CONF` names, relative to
the runtime directory or absolute (`./moq run -c FILE` sets it). `MOQUI_CONF` comes from the
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

For production packaging, deploy the release binary together with the `runtime/`
directory (the framework component is inside it) and run `./target/release/moqui` from the directory that
holds them (or point `MOQUI_RUNTIME` / `MOQUI_FRAMEWORK` at them), with
`MOQUI_CONF=conf/MoquiProductionConf.xml`. From a checkout, `./moq run-production` does that
with the release binary.

## Entity SQL names and existing Rust databases

Fresh schemas default to Java's physical naming: `UserAccount.userId` becomes
`USER_ACCOUNT.USER_ID`, stored as `user_account.user_id` on PostgreSQL. Every capital
gets a separator (`URLValue` becomes `U_R_L_VALUE`); logical entity/field API names
and query result keys remain unchanged. Explicit `table-name` and `column-name`
attributes bypass this conversion, but still follow the database's unquoted case
folding. Ordered database `name-replace` rules apply to columns, including explicit
overrides, not tables or result aliases. `use-pk-constraint-names` controls explicit
PK labels in Java mode. Runtime configuration is used for both schema creation
and queries.

**Before opening an existing Rust database**, back it up and set
`entity_sql_name_style=legacy` in the environment or `runtime/.env`, or put
`<entity-facade sql-name-style="legacy"/>` in the runtime configuration. This preserves
the former quoted names (`"UserAccount"."userId"`), including explicit overrides
and the old first-match replacements (which also affected table names).
The mode is selected when the model is loaded; restart after changing it. Java-style
and legacy naming cannot be mixed within one facade. Custom embedded applications
must pass the same configuration to the model and dialects (or use
`EntityFacade::open_components_with_conf`).

There is **no automatic rename, migration, or data copying**. Schema checks refuse
detected legacy table/column names before modifying that entity group's schema.
The check is a safety net for known old naming patterns, not a migration inventory;
it cannot identify arbitrary manually renamed tables. Normal queries never migrate
anything. To switch existing data to Java names, stop writers, inventory and resolve
name collisions, migrate tables/columns and dependent SQL offline from a backup,
then restart in `java` mode. Do not simply switch modes on a populated database.

This is table/column identifier compatibility, **not whole-schema parity**. SQL types,
constraint/index generation policies, schema/search-path selection and database
features retain the port's existing behavior. Handwritten SQL expressions must use
physical identifiers appropriate to the selected mode. Naming tests cover the
framework's ASCII names; non-ASCII case mappings are not guaranteed to match Java's
Character/Unicode mappings. Only PostgreSQL behavior is integration-tested here; MySQL and other
profiles have SQL-generation coverage.

## Implementation Order

The port is bottom-up. The entity facade and the service facade are finished before the first gate, a data file load. Plan: [`plans/archive/Implementation-Phases.md`](plans/archive/Implementation-Phases.md).

The architecture direction is [`plans/archive/Architecture-Proposal.md`](plans/archive/Architecture-Proposal.md). The crate cut is [`plans/archive/Crate-Layout.md`](plans/archive/Crate-Layout.md). Context objects are [`plans/archive/ContextObjects.md`](plans/archive/ContextObjects.md). Embedded Postgres is [`plans/archive/EmbeddedPostgres.md`](plans/archive/EmbeddedPostgres.md). TypeScript generation from definitions is [`plans/archive/TypeGeneration.md`](plans/archive/TypeGeneration.md). Cluster coordination is [`plans/archive/Cluster.md`](plans/archive/Cluster.md). SSH and SFTP crates are [`plans/archive/Ssh.md`](plans/archive/Ssh.md). The SSO relying party is [`plans/archive/Sso.md`](plans/archive/Sso.md). The architecture and tool-fit review of this plan set, with the deployment-configuration decisions, is [`plans/archive/MimoReview.md`](plans/archive/MimoReview.md).

## AI/LLM Policy

Moqui was originally designed and built largely to reduce the human pain and suffering involved with building and maintaining enterprise applications. The same philosophy applies to the world of AI, from LLMs to harnesses, and from harnesses used to make Moqui to those built into it.

This project is an AI-assisted rebuild of the manually written Moqui Framework in Rust. It would likely never be done without AI assistance migrating and filling in code. AI assistance to migrate existing components made for Moqui is also recommended. Without that being available, migration from Groovy to JavaScript would be a much more expensive thing to do and it is much less likely we'd ever do it.

LLMs are considered tools used by people. The person using the tool is just as responsible as with a text editor and keyboard. LLMs are the functional and moral equivalent of a hammer: they do not shake themselves, and are not a substitute or replacement for a responsible party.

Don't trust, verify. Use AI to help verify, but remember who is responsible, and who is a who.
