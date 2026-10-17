# Cross-Platform Usability
Keep business behavior in `moqui-core`; platform hosts supply processes, storage, networking, lifecycle, and optional local databases, with remote-service or local-only operation as appropriate.
Separate portable JavaScript from server-facade scripts and browser/device code; sharing a language does not share host capabilities.
Linux, macOS, and Windows can run the existing threaded Rust/QuickJS design with platform-specific paths and database/process management.
Desktop presentation may be native widgets, an embedded webview, or a standalone web browser, backed by a local runtime or remote server.
Embedded devices and POS may also host the runtime or act as clients; isolate peripheral APIs and make storage, printing, and process tools capability-dependent.
Separate screen definitions and action/data bindings from presentation; native widgets need a rendering backend distinct from HTML/Vue macros.
Browser delivery must allow either a remote-server client or an entirely local WASM runtime, preserving as much entity, service, screen, search, and sim functionality as possible.
A browser-local host needs WASM-compatible core dependencies, JS execution/bindings, worker-based execution, and async host adapters without assuming native threads, sockets, or child processes.
Use OPFS-backed storage with a WASM database and matching dialect/transaction/search/sim implementations; handle quota, eviction, persistence requests, migrations, recovery, and multi-tab coordination.
Android and iOS presentation may likewise be native widgets, an embedded webview, or a standalone web browser; share contracts, not necessarily UI code.
Mobile hosts supply permission-scoped device APIs, secure storage, authentication, and networking that survives suspension, cancellation, and mutation retries.
Local mobile business execution requires selectively linking core/QuickJS without desktop host dependencies and supplying mobile-compatible persistence.
That persistence needs matching dialect, transaction, migration, search, and sim behavior; desktop embedded Postgres is not a mobile backend.
Package/cache code and definitions for fully offline startup; replace unavailable host capabilities with browser adapters or optional remote services, and add atomic outbox/sync/conflict handling plus server reauthorization when sharing data.
