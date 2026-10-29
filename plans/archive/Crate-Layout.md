# Crate and Source Layout

How the MR-Moqui repository is split into Rust crates and source trees.

This follows `plans/archive/Architecture-Proposal.md`. It replaces the Gradle source sets in `moqui/framework` (`src/main/java`, `src/main/groovy`, `src/start/java`, `src/test/groovy`).

## Decision

Six crates, one workspace, and two data directories.

2026-10-28: a seventh, `crates/moqui-llm`. The agent runtime (`moqui_core::llm`, 79,000 lines) moved out of `moqui-core` onto it, so an edit there no longer rebuilds the framework. Core reaches it only through `moqui_core::llm_hooks`, and `moqui-host` links it under its `llm` feature.

```
mr-moqui/
  Cargo.toml                 # workspace
  crates/
    moqui-lib/               # no ExecutionContext; usable on its own
    moqui-db/                # entity model, SQL text, dialects; no ExecutionContext and no driver
    moqui-core/              # the framework; synchronous, multi-threaded
    moqui-host/              # process edge: HTTP server, drivers, process spawn
    moqui-browser/           # browser host: wasm32-unknown-emscripten worker, SQLite, fetch
    moqui/                   # binary: CLI and process entry
  framework/                 # framework component: XML, XSD, templates, default conf
  runtime/                   # instance directory: conf, webroot, tools, logs
```

Dependency direction is one way:

```
moqui (bin) → moqui-host → moqui-core → moqui-db → moqui-lib
moqui-browser → moqui-core (default-features = false) → moqui-db → moqui-lib
```

`moqui-core` is the main framework crate. `moqui-host` is the crate that sits on top of it. `moqui-db` holds the entity model and the SQL text, and does not know the facades. `moqui-lib` sits at the bottom and does not know the framework exists. `moqui-core` also depends on `moqui-lib` directly, the same way `moqui-host` depends on both `moqui-core` and `moqui-lib`.

There is no `src/start` tree and no interface-only tree. Each crate has one `src/` tree. `pub` is the supported API. Extension points that have a second implementation, or that the host must supply, are traits in the same module as the rest of that code.

## Why these cuts and no others

The current trees are language and packaging boundaries:

- `src/main/java` holds the public interfaces plus utilities (`org.moqui.*`, `org.moqui.util`, `org.moqui.resource`, `org.moqui.etl`, `org.moqui.jcache`).
- `src/main/groovy` holds `org.moqui.impl.*`.
- `src/start/java` is `MoquiStart`, a classloader whose job is to run an executable WAR (nested jars, then Jetty).
- `src/test/groovy` is the boot-the-framework suite.

Rust has one language, and dependencies are linked into the binary. Those two source sets do not come forward as directories.

Entity, service, and screen call each other through `ExecutionContext` (entity ECAs call services, services call entities, screens call both). Splitting them into crates makes a cycle, which Cargo will not allow. A traits-only crate plus impl crates would recreate the Java/Groovy split and still leave the cycle. They are modules inside `moqui-core`.

The cuts that are real:

1. Code with no framework type in its API or body can be used without starting Moqui. That is `moqui-lib`.
2. Entity model, dialect profiles, the condition tree, and SQL and DDL text have no `ExecutionContext` and no driver client. That is `moqui-db`. Search keeps its own SQL compiler in `moqui-core` and uses this crate for quoting and placeholders.
3. The process edge stays above the framework. That is `moqui-host`: the HTTP server, database driver connections, and process spawn.

Everything else stays in `moqui-core`, including the facades, XML Actions, screens, entity policy (authorization, ECAs, caches, feeds, the data loader), services, LLM and A2A logic, the JavaScript engine, and the framework's own threads.

## Threads, and why the host is separate

Moqui uses threads in many places: the request, async service calls, scheduled jobs, and other background workers. On the server those stay threads. The browser build (`wasm32-unknown-emscripten`, `plans/archive/Wasm.md`) runs one worker and selects the `inline` executor at factory start, so it starts none of those threads. `wasm32-wasip1` is not a target. The execution model in `plans/archive/Architecture-Proposal.md` is the rule: one `ExecutionContext` per thread, blocking calls. Entity, screen, and HTTP APIs return values. The one wait handle is service-facade `callFuture()`, a oneshot from a worker-pool thread.

`moqui-core` uses `std::thread`. The factory starts the worker threads. The scheduler lives in core. It sleeps, and on a tick it submits due jobs to the service worker pool. It does not run the job body. An async service call, and `callFuture()`, run on that pool (bounded, with a bounded queue) and get another context. Core may use `std::fs` and `std::time`.

`moqui-core` does not use Tokio, axum, the sync DB clients, or `postgresql_embedded`. Calling async clients from a facade would turn entity finds, service calls, and screen renders into futures, and a QuickJS host callback cannot suspend. The transaction manager and its pool live in core and call `DbDriver`. The host implements that trait with `postgres::Client` for embedded and external Postgres, and with the `mysql` client for MySQL. Each `postgres::Client` owns a `current_thread` runtime and drives it with `block_on` on connect, on every query, and on drop. The host does not own a Tokio handle. Those calls run on factory `std` threads. Building that runtime panics on a thread already inside one, so the embedded-Postgres startup bridge has exited before the pool connects, and the pool is not dropped on the axum runtime. Startup and shutdown go through `postgresql_embedded`. The socket and the settings are `plans/archive/EmbeddedPostgres.md`. The `http` feature on `moqui-lib` links blocking `reqwest`, which starts a runtime of its own. That runtime is not the axum runtime, and core does not call the axum handle for outbound HTTP.

The axum handler accepts the connection on Tokio and hands the request to a thread from the factory's request pool. It waits for the response. It does not await a framework future, and it does not use Tokio's `spawn_blocking` pool. That pool's cap is independent of the datasource pool. `POST /cluster/{topic}`, a WebSocket message, and an LLM or A2A turn that persists use that same handoff before any `DbDriver` call. A WebSocket message then runs on the service pool. Building a `postgres::Client` on an axum thread panics, because that thread is already inside a runtime.

## moqui-lib

Standalone utilities. No `ExecutionContext`, no facade, no `Moqui` static lookup. A program can depend on this crate and never link the framework.

The one place the current code cheats is `RestClient.refuseIfSim()`, which calls `Moqui.getExecutionContextFactory()`. That check moves to the framework caller (the LLM client and the remote service runners already know sim mode). The client itself stays unaware of the framework. Callers that want a policy pass a predicate into the client. The predicate is not a Moqui type. It default-denies while the context is in sim. Java `RestClient.allowInSim` is the LLM exception: provider calls set it, and the predicate allows that path. Every other caller stays denied.

| Module | Comes from | Notes |
|---|---|---|
| `error` | `BaseException` | Plain error type. Artifact exceptions stay in core. |
| `node` | `MNode` | XML tree, location, merge, and the writer. Parsing is filled in against the corpus. No XML crate. Future pass: the entity data loader (`moqui-core` `entity/load_feed.rs`) carries its own streaming XML and JSON scanner so it can load a large file one top-level element at a time. Move that pull tokenizer into this module so there is one XML grammar, and have `MNode::parse_text` and the loader share it. |
| `resource` | `ResourceReference`, file and memory refs | Trait plus file and memory implementations. Schemes are `file`, `runtime`, `component`, `http`, `https`, and `dbresource`. `db://` is not a second scheme. `content://` is an error that names the location. Classpath and jar are gone; the framework component is a directory. `getChildren` and `findChildFile` cover `file`, `runtime`, and `component`. `http` and `https` use the blocking client from phase 6. `dbresource://` lands with the entity facade in phase 4. |
| `object` | `ObjectUtilities` | Field coercion: decimals, timestamps, booleans, strings. `rust_decimal` and `jiff` 0.2 live here. `matches` is a private full-match helper in this module. There is no regex crate. |
| `string` | `StringUtilities` | Encode, decode, pretty-print, masks. Groovy class-name cleanup is not ported. |
| `collection` | `CollectionUtilities` | Map and list helpers the XML and entity code actually use. |
| `context_stack` | `ContextStack` | Stack of maps for variable scope. No engine trait. What each environment sees is `plans/archive/ContextObjects.md`. |
| `env` | `SystemBinding.getPropOrEnv` and `expand` | Property and environment lookup, and `${name}` expansion against those. The `runtime/.env` file is parsed here and loaded at startup like environment variables (real environment wins). This is string replacement, not a script engine. Context expansion (`ResourceFacade.expand`) stays in core. |
| `etl` | `SimpleEtl`, `FlatXmlExtractor` | Extract, transform, load pipeline. `EntityFind.extract` and `EntityFacade.makeEtlLoader`. `EntityDataLoaderImpl` does not use it. Phase 11. |
| `topic` | `SimpleTopic` | In-process publish. |
| `cache` | `MCache`, `MCacheManager` | Local cache. The core cache facade uses this directly. No JSR-107 surface. |
| `signer` | `SimpleSigner` | QZ Tray signer: RSA `SHA1withRSA` over a PEM. The Java caller is `WebFacadeImpl`. Lands in phase 8 with QZ Tray. Entity field encryption is the PBE in `EntityJavaUtil.enDeCrypt`, not this module. |
| `sgml` | `SimpleSgmlReader` | OFX-style SGML, 159 lines, no attributes. Its own function onto `Node`. It is not the framework XML parser. |
| `http` | new | `HttpClient` trait. |
| `rest` | `RestClient` | Blocking client. Feature `http`. |

`jiff` 0.2 is the timestamp and time-zone type. It includes the IANA database, which session and user time zones need. Postgres enables `with-jiff-0_2`. `mysql_common` decodes `time` 0.3 and `chrono` and has no jiff feature, so the MySQL driver maps the driver's date tuple onto `jiff`. The `time` and `chrono` features stay off.

`node` is the tree the rest of the framework calls. The FreeMarker half of `MNode` is not ported, and neither is the JDK SAX handler. Parsing is whatever the corpus needs, filled in when a file fails to load. The behavior to match, from the handler the corpus already goes through: UTF-8, an optional XML declaration, comments and processing instructions skipped, the element name stored raw (`getNodeNamespace` returns null today), attributes kept as written (`xmlns:xsi` and `xsi:noNamespaceSchemaLocation` are ordinary attributes), no schema fetch, no external entities, no DOCTYPE, character data including CDATA trimmed into the node's text, the five predefined entities and numeric character references expanded, and any other entity an error. The writer emits child text as CDATA. Line and column are tracked. `EntityValue`'s W3C DOM export is not ported. Acceptance is the corpus: framework conf, entity, service, and screen XML, and `mantle-udm` when it is loaded. Nothing else this runtime parses XML (`lettre`, `axum`, `postgres`, `reqwest`, `rquickjs`, `rustls`), so there is no XML crate. `sgml` stays its own reader. Gate 3 `html-print` emits print HTML. The name `xsl-fo` selects that mode. PDF is that HTML printed by `PdfRenderer`, as Tool components and print describes.

`cache` carries the `MCache` design. Rust is where the same algorithms get faster. The store is a concurrent map of entries. A get checks expiry using the access, creation, and update durations. `get(key, expireBeforeTime)` drops an entry whose last update is older than a known resource change, which is how screens and resources avoid a stale parse. Stats stay: gets, hits, misses, expires, evictions. `maxEntries` is enforced by a scheduled pass (default `maxCheckSeconds` 30), not on put. The pass first drops expired entries, which Java's pass leaves counting toward the limit, then removes the overflow, ordered by lowest `accessCount`, then oldest `lastAccessTime`. `MCacheManager` is a name-to-cache map, enough to use as a factory. Listeners, entry processors, write-through, and MX beans stay out.

`RestClient` is the proof that lib is useful alone: URI building, headers, JSON, retry, timeout, streaming, multipart. The `http` feature turns on `reqwest` with rustls, using the blocking client. `reqwest::blocking` runs requests on a Tokio runtime that client starts for itself. A call from a framework worker does not use the axum handle. A lib-only program can leave the feature off. `moqui-core` enables it and uses `RestClient` by default. A test, or a deployment with outbound HTTP disabled, passes another `HttpClient`. The sim-mode check stays at the framework caller, as the predicate described above: default-deny, with the LLM provider path allowed. Three breaks from Java `RestClient` are deliberate. TLS certificates and host names are verified, where Java trusts any certificate, so a remote with a self-signed certificate needs it trusted at the OS. Redirects stay on the same host and never go from https to http. A retry, when added, replays only idempotent methods, where Java replays any method on 429.

URL helpers, query-string handling, HMAC, and content-type sniffing from `WebUtilities` live in lib. The servlet containers, Jetty client, and session objects in that class are not ported.

Not ported, on purpose:

- `MClassLoader`. There is no classpath and no nested-jar loader.
- `ContextBinding`. It exists to wrap a Groovy `Binding`. The script context is the context stack plus the QuickJS context in core.
- `LiteStringMap`. Use `indexmap` until a profile says a special map is worth it.
- The JSR-107 surface on top of `MCache` (listeners, entry processors, write-through, MX beans). `MCache` and `MCacheManager` are ported. The cache facade in core uses them directly.

## moqui-db

Entity model and SQL text. No `ExecutionContext`, no facade, no driver client, and no Tokio. Depends only on `moqui-lib`. A test can render SQL for a definition and a dialect without starting the framework and without opening a connection.

| Piece | What it is |
|---|---|
| Entity model | Definitions parsed from `MNode`: dictionary types, fields, relationships, master definitions, view-entity structure. |
| Dialect profiles | The `<database>` block: SQL type names, join style, offset style, lateral style, full-text style, null handling, and the related flags. |
| Condition tree | The input to the renderer. `EntityFind` in core builds it. |
| SQL and DDL text | Insert, update, delete, select, count, joins, offset, and view-entity select. |
| Quoting and placeholders | Shared with search. Search's own SQL compiler stays in `moqui-core`. |

`moqui-core` keeps entity policy: authorization, ECAs, the entity cache, the transaction cache, sim, audit, feeds, and the data loader. It keeps `EntityValue`, `EntityFind`, the `DbDriver` trait, the transaction manager, and the pool. `moqui-host` keeps the driver implementations. Row values that cross `DbDriver` stay the plain values from `moqui-lib` (`rust_decimal` and `jiff`).

This is not a drivers crate. Postgres and MySQL clients stay in `moqui-host`.

## moqui-core

The framework. One `src/` tree, modules by facade. Public types are `pub`. Helpers are private. There is no `impl` tree.

Built-in behavior is a struct with methods. The Java interfaces on `EntityFind`, `ServiceCallSync`, `ScreenRender`, and the facades existed so a Groovy class could sit behind a Java type. A struct is the API.

A trait is used when conf selects an implementation, or when the implementation cannot live in this crate:

| Trait | Why it is a trait |
|---|---|
| `DbDriver` | Replaces `EntityDatasourceFactory` and the JDBC half of `TransactionInternal`. Embedded Postgres, external Postgres, and MySQL implement it in `moqui-host`. The transaction manager and its pool live in core and call this trait. The methods block and return rows. Later databases use the same trait. |
| `HttpClient` | Defined in lib. Core uses the blocking `RestClient` unless a test or conf supplies another. Outbound HTTP with no client returns a clear error. |
| `ServiceRunner` | Entity-auto, script, and remote runners are built in. Remote runners call `HttpClient`. |
| `SearchBackend` | Postgres SQL for the query and aggregation trees is in core. An OpenSearch mapping that only builds HTTP bodies can live here and use `HttpClient`. |
| `LlmProtocol`, `LlmTool` | More than one protocol and many tools. |
| `PrintClient` | The IPP client is in `moqui-host`. With none installed, a send reports that network printing is not installed. `plans/archive/Office-Image-Print.md`. |
| `PdfRenderer` | Optional. Core renders `html-print` without this trait. The host prints that HTML to PDF. The default engine is a long-lived Chromium process. `pdf-servo` is an in-process alternate. See Tool components and print in the architecture proposal. |
| `TemplateRenderer` | Conf maps a file extension to an implementation, as Java's `<template-renderer>` does. The implementations are `JsTemplateLiteral`, the fancy renderer, and the locked-down fancy renderer. All of them run on the QuickJS runtime. See Templating in the architecture proposal. |

The cache facade uses the `MCache` port directly. There is no second cache implementation. Outbound mail uses `lettre` directly. Neither is a trait. Distributed invalidation is a listener on the in-process topic. The member table, the key-material loading (configuration, not rows), and that listener are in core, beside the cache and the session. The HTTPS listener and the client calls are in `moqui-host`. There is no new crate. The protocol is `plans/archive/Cluster.md`. It is not a Redis client and not a second cache facade. Entity cache, this local cache, and the `screen-server-static` response header stay separate. The embedded SMTP server stays a disabled host feature. Inbound mail is `poll#EmailServer` in phase 10. Where Moqui already has an implementation because the general library was the wrong shape, that implementation is ported. `MNode`, `MCache`, and the transaction manager are the cases. An external crate earns its place when it is the engine (Postgres, QuickJS, axum) or when another crate already needed pulls it in.

QuickJS-NG (`rquickjs`) is the script and template engine. There is one language, so conf does not select a runner. Conf does select a `TemplateRenderer` by extension. XML action and screen macros are JavaScript modules on that runtime. Tests call the renderers and the macro modules directly. The split is `plans/archive/Architecture-Proposal.md`. What each one sees is `plans/archive/ContextObjects.md`.

`ExecutionContext` and `ExecutionContextFactory` are structs. The host builds the factory and passes the host implementations. `DbDriver` is a map: one entry per entity group, plus the search store when that store is a different database. It is not one trait object. `PdfRenderer` is passed when a renderer exists. Facades are fields. There is no `Moqui.getExecutionContextFactory()` global inside lib or core. The binary may keep a process-wide factory for the CLI. The factory starts the framework's worker threads.

`WebFacade` is a struct filled from an owned request value (method, path, query, headers, body) and it writes an owned response. It does not refer to axum or to a socket. `initWebFacade(HttpServletRequest, HttpServletResponse)` becomes `handle` on the request value. The host translates axum types at the edge. `ScreenFacade.makeRender()` fills the same struct from a parameter map, a session map, a base URL, and a writer, with no socket. Email bodies, `render#ScheduledScreen`, and `ScreenTest` use that fill.

### Modules

`context` holds the factory and the facades that are not large enough to be their own directory: artifact execution, cache (the facade over `MCache`), l10n, logger (over `tracing`), message, notification, resource (schemes `file`, `runtime`, `component`, `http`, `https`, and `dbresource`, and the facade), transaction (the facade, the manager, and the pool), user, web, conf loading, and component discovery.

`script` is the QuickJS runtime. `template` is the `TemplateRenderer` implementations on that runtime: `JsTemplateLiteral`, the fancy renderer (`if`, `list`, and `assign`, compiled to one cached JavaScript function), and the locked-down variant (the same compiler). The values each one sees are `plans/archive/ContextObjects.md`.

`entity` holds values, find, the `DbDriver` trait, ECAs, the data loader and writer, the entity cache, feeds, and dynamic views. The model, the condition tree, dialect profiles, and SQL and DDL text live in `moqui-db`. `EntityFind` calls that crate. Opening a connection is host.

`service` holds definitions, the call builders, runners, SECAs, email ECAs, REST API metadata, and job definitions, including the thread that sleeps, takes the job lock, writes the history row, and submits the body to the service pool. JSON-RPC dispatch is not included. The tick does not run the job. Cron expressions stay Quartz syntax, as `ScheduledJobRunner` documents. `croner` 4 parses them. The parser is built with optional seconds, `alternative_weekdays` (Sunday is 1), and `sloppy_ranges`, so a seeded expression such as `0 0/15 * * * ?` parses and a numeric day-of-week matches Quartz. `?`, `L`, `W`, and `#` are part of that parser. `getCronDescription` calls `describe()`. The crate's default `chrono` feature stays off and the `jiff` feature stays on.

`screen` holds definitions, forms, trees, URL info, and the QuickJS macro expansion to HTML and to client JavaScript.

`search` is the new facade from the architecture proposal, not a port of `ElasticFacade`. The API is `plans/archive/SearchFacade.md`. Postgres query and aggregation SQL is in core. Quoting and placeholders come from `moqui-db`. OpenSearch request bodies live here when they are just HTTP. Starting the OpenSearch process does not. When the entity datasource is Postgres, embedded or external, default search tables are the `search` schema on that database. Search is its own embedded cluster when the entity datasource is not Postgres, and an external Postgres when a search URL names one. Embedded search starts on Windows too. Windows connects over TCP to `127.0.0.1` on an ephemeral port, as `plans/archive/EmbeddedPostgres.md` specifies.

`llm` holds the client, conversation, protocol encoding, tool loop, and `a2a`. Provider calls go through `HttpClient`.

Other current classes, placed by the same rule:

| Current | Where |
|---|---|
| `EdiHandler` | `moqui-core`. It holds an `ExecutionContext`. |
| `RestSchemaUtil` | `moqui-core` (`service`). |
| `UserSecurityChecks` | `moqui-core` (`context` / user). |
| `JdbcExtractor` | The ETL interfaces stay in lib. JDBC extraction is host, next to `DbDriver`. |
| `ElasticSearchLogger` | Host, beside the OpenSearch process. |
| `ElFinderConnector` | Host, if the file API stays. It is an HTTP endpoint. |
| `MoquiShiroRealm`, `MoquiWebSecurityManager` | Not ported. User auth is the core user facade on `argon2` 0.6 with the `password-hash` feature. |
| `TransactionInternalBitronix` | Moqui's own transaction manager in core, against `DbDriver`, with this class's behavior as the reference: local one-phase commit, the pool, synchronizations, and suspend that parks the enlisted client on the transaction. The XA journal, crash recovery, and JMS are not ported. |
| `H2ServerToolFactory`, `JackrabbitRunToolFactory`, `JCSCacheToolFactory` | Not ported. `content://` resolves as an error. |
| `MCacheToolFactory` | The cache facade on the `MCache` port. |
| `SubEthaSmtpToolFactory` | Host, behind a feature, and disabled. Inbound mail is `poll#EmailServer` in phase 10. |
| Servlet and filter classes in `impl/webapp` | Host HTTP layer. The behavior they delegate (screen render, auth, REST, LLM, A2A, notifications) is core. |
| `src/main/resources/org/moqui/impl/*.groovy` (send and poll email) | Core service logic. Outbound mail uses `lettre` in core. The Groovy scripts are not kept as scripts. |

`rquickjs` links QuickJS-NG, which is C. It belongs in core because XML Actions and expressions are the framework. Each `ExecutionContext` owns the runtime for its thread. Functions called from JavaScript are synchronous and return values. Host callbacks assume `panic = unwind` and are wrapped in `catch_unwind`, as `plans/archive/Architecture-Proposal.md` specifies. Do not create a `moqui-js` crate.

Core can load components, parse XML, expand actions and screens, run JavaScript, and, through `moqui-db`, evaluate entity conditions and render SQL, with no database driver and no listen socket. Opening a connection, accepting a request, and spawning an external process are host. That is what keeps a core unit test free of those.

## moqui-core features

`mail` (lettre, imap, mail-parser, rustls), `http` (`moqui-lib/http`, reqwest), and `cluster-tls` (rcgen) are default features. They gate outbound and inbound mail, the remote REST runner and http resource locations, and the generated cluster certificate. `moqui-host` uses the defaults. `moqui-browser` turns them off, so no aws-lc, reqwest, or socket code reaches the browser build. These are features, not a crate split, because that code calls many core internals. `plans/archive/Wasm.md`.

## Databases

The entity engine is database-agnostic in the same way the Java framework is. `moqui-db` owns the dialect and the SQL text. `moqui-host` owns connections. Adding a database is a dialect profile in `moqui-db` plus a `DbDriver` in `moqui-host`. It is not a fork of the entity facade.

The dialect profile is the `<database>` block Moqui already has in `MoquiDefaultConf.xml`: SQL type names, join style, offset style, lateral style, full-text style, null handling, and the related flags. The SQL renderer in `moqui-db` takes a dialect. It does not import a driver. That crate renders SQL text at runtime, as the framework does now. There is no SQLx dependency and no `query!` compile-time macros.

`DbDriver` is the execution boundary: connect, checkout, begin, commit, rollback, savepoint, execute, query, and release. `query` returns rows; a streaming arm serves `entity.find.iterator`, the ETL JDBC extract, entity sync (the phase 11 port of `EntitySyncServices`), and large exports (Postgres named cursor inside the transaction, MySQL unbuffered). An open stream is that thread's one ordinary connection on the pool. It pins that connection until it closes, and `begin` while it is open uses that checkout. The pool in core checks a connection out on the first statement and enlists it for the life of the transaction. `begin` does not check out. A call outside a transaction checks out and returns. `suspend` parks that client on the transaction, off the free list and off the thread. `resume` checks the same client out again. `force-new` is that suspend followed by begin, on the same thread and the same `ExecutionContext`, so the thread holds one ordinary connection. The parked client still counts against the pool max. Ordinary checkouts cannot take the reserve. Checkout is `plans/archive/EmbeddedPostgres.md`. The transaction rules, including the reserve, the park budget, and the startup cap check, are the Transactions section of `plans/archive/Architecture-Proposal.md`. Rows come back as the plain values `moqui-lib`'s `object` module already uses (`rust_decimal` and `jiff`), not JSON. `postgres` decodes timestamps with `with-jiff-0_2`. The MySQL driver maps the driver's date tuple onto `jiff`. `with-time-0_3`, `with-chrono-0_4`, and the MySQL `time` and `chrono` features stay off. The driver does not know entity definitions. The datasource conf selects the driver at runtime. The factory holds the group map described above.

Search stays the path from the architecture proposal. When the entity datasource is Postgres, embedded or external, default search tables are the `search` schema on that database. A search URL is how search points at a different Postgres. When the entity datasource is not Postgres, search is its own embedded cluster. Naming an external database for a store does not also start an embedded postmaster for that store.

Initial drivers, all in `moqui-host` and all enabled in the default binary:

| Driver | Role | Dialect profile |
|---|---|---|
| Embedded Postgres (`postgresql_embedded` 0.21) | Default entity cluster when no external database is configured. Lifecycle only: start and stop, with `bundled`, `blocking`, `theseus`, and `tls-rustls-aws-lc-rs` (default features off), and `POSTGRESQL_VERSION` pinned to 18.6.0. Per-query traffic is `postgres::Client` with `NoTls`: the Unix socket on Unix, and TCP to `127.0.0.1` on an ephemeral port on Windows. The pool is the core transaction manager. Embedded search is the `search` schema on that cluster. The sim overlay is a second cluster, one schema per `TransactionCacheDb`. See `plans/archive/EmbeddedPostgres.md`. | `postgres` |
| Sync Postgres (`postgres` 0.19) | External Postgres, same client and same core pool | `postgres` |
| Sync MySQL (`mysql` 28) | External MySQL and MariaDB. The core pool checks connections out. `mysql::Pool` is not the pooling design. | `mysql8`, and the older `mysql` profile |

Each `postgres::Client` owns a `current_thread` Tokio runtime and calls `block_on` on it for connect, query, and drop. The host does not own that runtime. `DbDriver` methods run only on factory worker threads, never on an axum task. Pool sizes, the reserve, and timeouts are the Transactions section of `plans/archive/Architecture-Proposal.md`. Memory and speed testing follow. MySQL is a first target alongside Postgres, not a follow-up milestone. Each Postgres and MySQL connection keeps an LRU of prepared statements, cap 100, keyed by SQL text. `query(&str)` prepares a named statement and closes it on drop. It is not that cache. Entity SQL uses the LRU. `query_once` is the one-shot path.

Embedded Postgres traffic uses `NoTls`, on the Unix socket and on the Windows TCP connection to `127.0.0.1`. External Postgres TLS is `tokio-postgres-rustls` (`MakeRustlsConnect`) with its `aws-lc-rs` feature. External MySQL TLS is the `mysql` feature `rustls-tls` (aws-lc-rs). The `native-tls` feature stays off. On `mysql` 28 that feature is opt-in, not part of the default set. That is the same rustls provider family as `reqwest`'s `rustls-tls` feature. Database traffic does not also link native-tls or OpenSSL. `postgresql_embedded` uses `tls-rustls-aws-lc-rs` for the same reason, and its `native-tls` feature stays off.

The Java conf also defines databases to add later, still in `moqui-host`'s `db` module, as another dialect plus a `DbDriver`. Each one needs a synchronous `DbDriver` implementation; there is no SQLx dependency. SQLite is not a Moqui database and is not a target. Of the remaining Moqui databases, SQL Server, Oracle, DB2, and the rest each wait on a sync driver that implements the same trait. H2 is not on the list. Embedded Postgres replaces it. OrientDB appears only as a comment in the Java conf.

| Conf name | Database | When |
|---|---|---|
| `mssql` | SQL Server | Later, via a sync `DbDriver` |
| `oracle` | Oracle | Later, via a sync `DbDriver` |
| `db2` | DB2 | Later, same |
| `db2i` | DB2 for i | Later, same |
| `derby` | Derby | Later, same |
| `hsql` | HSQLDB | Later, same |

Cargo features may trim a driver out of a custom build. The default feature set includes embedded Postgres, Postgres, and MySQL. A MySQL entity database still needs Postgres or OpenSearch for search, and sim still needs the overlay cluster, so the default binary includes `postgresql_embedded`. A custom build may omit it only when search is an external Postgres or OpenSearch and the deployment does not use sim. The binary-size note in `plans/archive/Architecture-Proposal.md` is the same limit.

## moqui-host

The process edge. Depends on `moqui-core` and on `moqui-lib` with the `http` feature. This is the only crate that may use axum, Tokio, the sync DB clients (`postgres`, `mysql`), `postgresql_embedded`, process spawn, SSH (`russh`, `russh-sftp`), the OIDC client (`huskarl`), and the IPP client (`ipp`). `lettre` lives in core. `MCache` lives in lib. There is no `moka`, no `r2d2`, and no XML crate.

| Module | Role |
|---|---|
| `http` | axum routes and WebSocket upgrade. `extract::ws` needs axum's `ws` feature, which is not in the default set. Builds the core request value, calls core on a request-pool thread, writes the response. Replaces `MoquiServlet`, the auth filter, the LLM and A2A servlets, and the notification endpoint. The cluster endpoint from `plans/archive/Cluster.md` is a route here and uses that same handoff before any `DbDriver` call. A WebSocket message runs on the service pool. The client calls that send to peers are here too. The `/fop/*` and `/htmlr/*` servlets are not ported. |
| `db` | `DbDriver` implementations: embedded Postgres lifecycle, external Postgres, and external MySQL. The pool is the core transaction manager. Each method runs on the checked-out client and returns rows. Later databases from the table above land here. Embedded Postgres concurrency is this module, as in `plans/archive/EmbeddedPostgres.md`. |
| `mail` | The embedded SMTP server stays disabled, behind a feature. Inbound mail is `poll#EmailServer` (IMAP and POP3) in phase 10. Outbound mail is core. |
| `search` | OpenSearch process supervisor. Query construction stays in core. |
| `pdf` | `PdfRenderer`'s engine behind the core `PdfEngine` slot. Feature `pdf` (default) is a DevTools client on `tokio-tungstenite` that drives one long-lived external Chromium; `chromiumoxide` was dropped because its message parsing fails under `serde_json` `arbitrary_precision`. No browser fetcher is linked. Feature `pdf-servo` (off by default) links Servo for an in-process render where a child process is forbidden. The recipe, the process flags, and the page cap are Tool components and print in `plans/archive/Architecture-Proposal.md`. |
| `ssh` | `russh` 0.64 and `russh-sftp` 3. `russh` is `default-features = false` with `aws-lc-rs`, `flate2`, and `rsa`. `ring`, `des`, and `dsa` stay off. The SFTP client behind `moqui_core::sftp`, the SSH server (SFTP over a resource root, exec and the admin shell) when `ssh_port` is set, and the SSH cluster channel. `plans/archive/Ssh.md`. |
| `edge` | One small Tokio runtime for the async clients (PDF, SSH, SSO, IPP). A blocking caller spawns onto it and waits on a channel. |
| `sso` | `huskarl` 0.11, default features (the native JWS verifier), with an async `reqwest` transport. The OIDC and OAuth 2 client behind `moqui_core::sso`; the services and the account sync are core. `plans/archive/Sso.md`. |
| `ipp` | `ipp` 7 message encoding, no client features, posted with the async `reqwest` on the edge runtime. The network printing client behind `moqui_core::print` (`moqui-cups`); the print services are core. `plans/archive/Office-Image-Print.md`. |

Features on this crate, not more crates: `opensearch`, `inbound-smtp`, `pdf`, `pdf-servo`. Default features are embedded Postgres, external Postgres, MySQL, axum, and `pdf`. `pdf-servo` stays off. Chromium itself is not a crate and is not in the binary.

Drivers stay in `moqui-host`. The binary always wants the HTTP server and the initial drivers together. A crate of drivers without HTTP is not part of this layout.

## moqui binary

`crates/moqui` is a binary crate. It replaces `MoquiStart` and the `main` path on `org.moqui.Moqui`.

`MoquiStart` does three things: load jars from inside a WAR, parse CLI arguments, and start embedded Jetty. The first has no Rust equivalent. The second and third are `main`.

Commands match the current CLI shape: run the server, and `load` with the same flags the current `Moqui.loadData` accepts (`types`, `components`, `location`, `timeout`, `raw`, and the disable flags). `save-db` and `load-save` are the Gradle tasks of those names, against the embedded cluster, as `plans/archive/EmbeddedPostgres.md` describes. The binary constructs the host stack, builds an `ExecutionContextFactory`, and either serves or loads.

The binary stays thin. Framework behavior does not accumulate here. Extra in-process service functions that must be Rust (there is no Java service runner) are registered from the binary or from host into a core registry.

Dev layout is a checkout plus `framework/` and `runtime/` on disk, found from the working directory, which is how Moqui finds its runtime today. Embedding those directories in the binary is an optional later feature. It is not a source set.

## Data directories

These are not crates.

`framework/` is the framework component. It absorbs what is now on the framework classpath and what is framework-owned template source:

- `component.xml`
- `data/`, `entity/`, `service/`, `screen/`
- `xsd/`
- `template/` for the JavaScript macro modules (XML Actions, screens, forms) and the framework text templates the renderers run
- default `conf/MoquiDefaultConf.xml`

`runtime/` is the instance directory, as it is today:

- `conf/` for `MoquiDevConf.xml`, production conf, and local overrides
- `base-component/webroot/` for the browser assets (Vue, Quasar, `moqui-html.js`)
- `base-component/tools/` for the tools component XML
- `component/` for add-ons such as mantle, which stay outside this repository
- logs and temp files, gitignored as they are now. Session snapshots are database rows, not files under this directory (`plans/archive/Cluster.md`).

Screen macros that are framework code live under `framework/template/`. The runtime can still override a template by location. Instance conf stays in `runtime/conf/`.

## Tests

Unit tests live next to the code in the crate that owns it, so lib tests do not boot the framework and core tests do not open a socket.

Tests that start a factory, load `framework/`, and hit HTTP live under `crates/moqui/tests` (or `crates/moqui-host/tests` when they do not need the CLI). That replaces `src/test/groovy`.

## What this does not add

- A `moqui-api` crate, or any other interface-only crate.
- Per-facade crates (`moqui-entity`, `moqui-service`, `moqui-screen`).
- A `start` or `execWar` source tree.
- A port of the JSR-107 cache API, Shiro, the Bitronix XA journal, Jackrabbit, H2, JCS, or the servlet types. `MCache` is ported. The transaction manager is Moqui's own, against `DbDriver`, with Bitronix's behavior as the reference.
- A `moqui-js` crate, a crate per database, or a drivers-without-HTTP crate.
- A crate for the tool components. `moqui-sftp` and the SSO relying party (`plans/archive/Ssh.md`, `plans/archive/Sso.md`), and `moqui-poi`, `moqui-image`, and `moqui-cups` (`plans/archive/Office-Image-Print.md`), are framework code in these crates. `moqui-fop` is not ported. PDF is the host `pdf` module.
