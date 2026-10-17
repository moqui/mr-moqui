# Wasm

Browser build of the portable framework: `wasm32-unknown-emscripten`, run in one dedicated Web Worker.

This reverses the earlier "no WASM target" decision in `plans/archive/Crate-Layout.md` and `plans/archive/Architecture-Proposal.md`. The goal is the browser-local runtime in `plans/proposals/Usability.md`. The server stays the authority, as `plans/proposals/Messaging.md` §8 describes. Local state is a cache plus pending requests. A local commit never grants authority to a server write.

Emscripten is chosen over `wasm32-unknown-unknown` for three reasons. QuickJS (`rquickjs-sys`) and SQLite are C, and emcc builds both. `std::fs` maps onto the Emscripten filesystem, so conf and component loading need no change. Emscripten generates the JavaScript bindings, Asyncify/JSPI, and the OPFS integration, the same way SQLite WASM does ([sqlite.org/wasm](https://sqlite.org/wasm)).

| Area                    | Documentation                                                                                                                                                                                          | Why it matters                                   |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------ |
| Rust target             | [Rust ](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-emscripten.html)[`wasm32-unknown-emscripten`](https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-emscripten.html) | Compiling Rust to WASM using Emscripten          |
| Emscripten build        | [Emscripten compiler documentation](https://emscripten.org/docs/compiling/index.html)                                                                                                                  | Linking, configuration, and generated JavaScript |
| Multithreading          | [Pthreads support](https://emscripten.org/docs/porting/pthreads.html)                                                                                                                                  | Native-style threads mapped to Web Workers       |
| Async execution         | [Asyncify and JSPI](https://emscripten.org/docs/porting/asyncify.html)                                                                                                                                 | Suspending and resuming WASM execution           |
| Thread coordination     | [Proxying API](https://emscripten.org/docs/api_reference/proxying.h.html)                                                                                                                              | Dispatching work between threads                 |
| Filesystems             | [File System API](https://emscripten.org/docs/api_reference/Filesystem-API.html)                                                                                                                       | Browser filesystem integration                   |
| Native interoperability | [Connecting C/C++ and JavaScript](https://emscripten.org/docs/porting/connecting_cpp_and_javascript/)                                                                                                  | Host function calls and generated bindings       |
| SQLite WASM             | [SQLite WASM and OPFS](https://sqlite.org/wasm/doc/trunk/persistence.md)                                                                                                                               | The pattern followed; OPFS VFS for persistence   |
| Moqui execution context | [Moqui ExecutionContext API](https://www.moqui.org/javadoc/org/moqui/context/ExecutionContext.html)                                                                                                    | Execution context, facades, and async semantics  |
| Moqui transactions      | [Moqui TransactionFacade API](https://www.moqui.org/javadoc/org/moqui/context/TransactionFacade.html)                                                                                                   | Transaction propagation and lifecycle            |

Emscripten does not turn threads, thread-local transaction state, sockets, or native database drivers into browser implementations. The browser build is the portable framework core plus explicit host adapters.

## What builds

| Crate | Browser build |
|---|---|
| `moqui-lib` | Builds with the `http` feature off. Blocking reqwest has no wasm backend. |
| `moqui-db` | Builds unchanged. |
| `moqui-core` | Builds with `default-features = false`. About 3.5k of 76k lines are gated (below). Entity, service, screen, script, transaction manager, search query building, notify, and user stay. |
| `moqui-host`, `moqui` | Server only. Tokio, axum, the Postgres and MySQL clients, embedded Postgres, russh, huskarl, and the PDF browser's DevTools client. |
| `moqui-browser` | New. The browser host: the counterpart of `moqui-host`. |

The blockers are dependencies, not threads. `std::thread::spawn` compiles for this target. Without pthreads it returns `Err` at runtime. The browser build must not start the service pools, the scheduler, the deadline timer, or the cluster sender. The dependencies that do not build, or must not be linked, are aws-lc-sys (through rustls, lettre, and rcgen), reqwest, imap, and the raw `TcpStream` in mail.

## moqui-core features

All three are default features, so `moqui-host` and the binary do not change. `moqui-browser` depends on `moqui-core` with `default-features = false`. A feature gate is used instead of a crate split: the gated code calls many core internals, and a split would make those `pub`.

| Feature | Links | Gated code | Off behavior |
|---|---|---|---|
| `mail` | lettre, imap, mail-parser, rustls, webpki-roots | `service/mail.rs` (SMTP send, IMAP and POP3 poll, MIME); the `ServiceKind::Mail` arm in `service/call.rs` | A mail service fails with "mail is not built into this host". Email-ECA rules still load. |
| `http` | `moqui-lib/http` (reqwest) | `service/remote.rs`, `shared_http_client`; the http location in `context/resource.rs`; `search/opensearch.rs` | `remote-rest` services and http locations fail clearly until phase W5 installs a browser `HttpClient`. |
| `cluster-tls` | rcgen | `cluster::generate_cert_pair`; the CHANGEME fill in `context/factory.rs` | A CHANGEME cluster cert slot is an error. Cluster start is already a host call. |

CI checks the boundary. `cargo tree -p moqui-core --no-default-features -e normal --target wasm32-unknown-emscripten` contains none of aws-lc-sys, ring, rustls, reqwest, or tokio.

## Execution model

- One dedicated Worker, one module instance, one `ExecutionContext` at a time. The UI thread never runs framework code.
- The 18 `thread_local!` sites (context, transaction, render state) are safe while calls are serialized. While a call is suspended in Asyncify or JSPI, the worker queues new calls and does not re-enter. A second request would otherwise see the first one's transaction.
- The factory takes an executor choice at start. `threads` is today's behavior. `inline` is the browser:
  - `async="true"` and `callFuture()` run after the caller's transaction completes, drained before the worker returns to JavaScript.
  - The scheduler does not start. Scheduled jobs are a server concern.
  - The request pool is `RequestPool::idle()`.
  - The transaction deadline timer does not start. Statement timeouts are not enforced. `DbCancel` is a no-op.
- `requires-new` and a suspended transaction need a second connection. SQLite in OPFS has one writer. This is open: measure, then either fail clearly or run the inner transaction inline under a savepoint.

## Storage and database

- SQLite is compiled by emcc into the same module, behind a `DbDriver` in `moqui-browser`. `DbDriver` is synchronous (`crates/moqui-core/src/driver.rs`), and so is SQLite in the same module, so database calls need no Asyncify.
- Persistence is the OPFS VFS, the `opfs-sahpool` pattern from SQLite WASM. It needs no cross-origin isolation.
- `MoquiDefaultConf.xml` has no `sqlite` `<database>` profile. One is added, with a dialect test pass over entity DDL, CRUD, view-entity, offset, and `FOR UPDATE` text.
- `framework/` and the needed `runtime/` conf and component XML ship as an Emscripten preload package, read-only in MEMFS.
- Writable files go to OPFS. Flush, quota, eviction, and recovery are explicit. Multi-tab use is one worker per origin, coordinated with Web Locks, or a second tab opens read-only.

## Host adapters

`moqui-browser` supplies what `moqui-host` supplies on the server:

- An `HttpClient` over `fetch`, for `remote-rest` services and http resource locations.
- WebSocket transport for `plans/proposals/Messaging.md`, with the same envelopes, schema ids, request ids, and revision rules as native callers.
- Timers, and `tracing` output to the console.
- Unsupported-operation errors for server-only facades: mail, SSH, PDF, the cluster, and SSO callbacks.

`fetch` and WebSocket go through Asyncify or JSPI. JSPI is smaller and faster but newer in browsers. Asyncify works everywhere and grows code size. The choice is made in W3 from measured size and the browser support matrix. Cancellation while suspended releases the context and rolls back the transaction.

## moqui-browser crate

- `crates/moqui-browser`, a bin target built only for `wasm32-unknown-emscripten`. It is a workspace member. Native `cargo check --workspace` builds it as an empty stub.
- Link flags live in `.cargo/config.toml` under `[target.wasm32-unknown-emscripten]`: `-sMODULARIZE`, `-sEXPORT_ES6`, `-sALLOW_MEMORY_GROWTH`, the Asyncify or JSPI flags, and the FS options.
- Exports are `extern "C"`: `init` (conf location and properties), `call_service`, `render_screen`, `entity_find`, and `free_handle`. Results cross as handles or byte buffers, not JSON strings, where `plans/proposals/Messaging.md` defines a binary form.
- A small JavaScript worker wrapper lives in the browser source tree. It owns the call queue, transfers `ArrayBuffer`s, and refreshes typed-array views after memory growth.
- The emsdk version is pinned next to the Rust 1.94 MSRV note in `Cargo.toml`.

## Phases

Each phase ends with a check that must pass.

**W0. Toolchain.** Pin emsdk. Add the rustup target and the target section of `.cargo/config.toml`. CI runs `cargo check -p moqui-lib -p moqui-db --target wasm32-unknown-emscripten`. This also unblocks Messaging Phase 0, whose FlatBuffers reader and writer are lib-only.

**W1. Feature gates.** Add `mail`, `http`, and `cluster-tls` to `moqui-core`, and stop enabling `moqui-lib/http` unconditionally. Gate the dispatch arms and the callers. Check: `cargo lint` and the test suite with default features. `cargo check -p moqui-core --no-default-features --target wasm32-unknown-emscripten` passes. The dependency-tree check passes. Risk: `rquickjs-sys` under emcc (bindgen or prebuilt bindings for this target).

**W2. Inline executor.** Add the start-time executor choice, the async-after-commit drain, scheduler off, and the no-deadline path. Check: the native core tests run a second time with the `inline` executor.

**W3. moqui-browser skeleton.** Add the crate, link flags, preload package, exports, and worker wrapper. Choose Asyncify or JSPI. Check: a headless-browser test boots the factory from the preloaded conf and renders one `text` screen with no database.

**W4. SQLite.** Add the emcc SQLite build, the OPFS VFS, the `sqlite` dialect profile, and the `DbDriver`. Check: framework seed data loads in the browser. Entity CRUD and a view-entity find pass. Data survives a reload.

**W5. Network adapters.** Add the `fetch` `HttpClient` and WebSocket transport. Check: a `remote-rest` service call against a running host succeeds. Cancellation while suspended rolls back and frees the context.

**W6. WebSocket transport.** Done: sockets in the worker (`js/moqui-ws.js`) with reconnect, backoff, and bounded queues. Frames enter the module between requests through ordinary exports, so the socket needs no JSPI. A `/notws` notification is published on the local `notification-message` topic, then posted to the UI. A binary frame waits behind a handle (`moqui_browser::ws::take_binary`) for the envelope reader. Local code queues outbound frames (`ws::send_text`, `ws::send_bytes`), and the worker sends them after each request. Check: `node scripts/wasm-ws-smoke.mjs <pkg>` (headless Chrome against a same-origin `/notws` mock).

**W6b. Messaging.** Wire the browser side of `plans/proposals/Messaging.md` §8 onto this transport: verified buffers behind handles, schema cache in OPFS, and offline mutations as pending requests submitted on reconnect. The binary frame handles are where its reader starts.

**W7 (optional). pthreads.** Serve COOP and COEP headers and run the `threads` executor on Emscripten pthreads. The non-pthread build stays.

## Status (2026-10-14)

W0 through W4 are in place. `scripts/wasm-build.sh` builds the module. `scripts/wasm-smoke.mjs` (Node, MEMFS) and `scripts/wasm-chrome-smoke.mjs` (headless Chrome worker, OPFS reload) pass. The release `.wasm` is 10.9 MB (3.9 MB gzipped). The `.data` preload is 1.7 MB.

Findings that change the text above:

- QuickJS needs rquickjs `bindgen` on this target; no prebuilt bindings ship. bindgen needs the emsdk sysroot, emsdk's clang builtin headers, and `-fvisibility=default`. `scripts/wasm-env.sh` sets them. Its build script does not rerun on an env change: `cargo clean -p rquickjs-sys --target wasm32-unknown-emscripten` after changing them.
- `-sSTACK_SIZE=8MB`. Emscripten's 64 KB default trips QuickJS's stack check on the first eval.
- The SQLite driver shares one handle. A `begin` while another logical connection has a transaction open is a savepoint inside it, so `callFuture()` and reserve transactions never wait on the single writer. A nested commit is undone if the outer transaction rolls back; that includes sequence refills and failed-login counts. Sequence ids stay unique because the refill floor is the in-memory bank.
- Without cluster-tls, CHANGEME cluster cert slots are left unset instead of failing boot.
- WasmFS OPFS needs pthreads or JSPI. W5 replaced the W4 snapshot with it (below).
- rusqlite stays on 0.38: servo-net pins it, and two libsqlite3-sys versions cannot both link sqlite3.

W5 is in place, on JSPI with no Asyncify build:

- `moqui_core::outbound`: `remote-rest` services and `http` resource locations send through an installed `HttpSender`, else reqwest when the `http` feature is built, else "not built into this host". Sim denial and URL redaction come first. The browser installs a sender over `fetch` (`crates/moqui-browser/src/fetch.rs`, the `__async` import in `emlib/moqui_fetch.js`). The request exports are `JSPI_EXPORTS` (`build.rs`) and return Promises; the worker queue serializes them, and a call that arrives while one is suspended is refused.
- A worker `cancel` op aborts the fetch in flight. The call fails, its transaction rolls back, and the next call runs. A failed remote call rolls back the write before it.
- WasmFS with the OPFS backend mounted at `/opfs`; `init` with `persist` puts the database there. Commits are durable with no flush step. WasmFS fails the rollback journal's unlink (`SQLITE_IOERR_DELETE`), so the journal mode is `PERSIST`.
- Every OPFS file operation suspends. Creating every framework table one autocommit at a time took 18 s on first load, so `check_tables` runs in one SQLite transaction (`SqliteDriver::bulk`): 2.0 s. A reload's check is 22 ms; a single-row store is about 35 ms (1 ms in memory).
- Release `.wasm` 11.1 MB (4.0 MB gzipped), about 0.2 MB more for WasmFS and the JSPI wrappers.
- The browser host also registers each runtime component's `service/` files, and a call whose inner service failed (errors in the message facade) is an error, not empty out parameters.
- Node 22 has no JSPI. `scripts/wasm-smoke.mjs` runs on emsdk's Node 24 with `--experimental-wasm-jspi`.

W6 (the WebSocket transport) is in place. The end-to-end check uses a Node mock of `/notws`, not moqui-host: the host would have to serve the package for the session cookie and the origin check to pass.

Open: no login flow in the browser host (calls run with authz disabled, except renders); Groovy services are skipped; rustc passes a deprecated `WASM_BIGINT` link flag; one worker per origin can hold the OPFS database (a second tab is expected to fail on the sync access handle; not yet tested); the emscripten FS JS API must not touch `/opfs` (those calls are not JSPI exports).

## Not in scope

- Host features in the browser: axum, the Postgres and MySQL drivers, embedded Postgres, SSH, the SSO callback, PDF, and SMTP or IMAP.
- Browser-local authority over committed server data.
- A crate split of the server-only code out of `moqui-core`. Feature gates cover it.
