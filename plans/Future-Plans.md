# Future plans

Digest of the unbuilt proposals in [proposals/](proposals/). None has code in the workspace beyond what is noted. Read the full document before starting any of them.

## Messaging — runtime-reflective FlatBuffers ([Messaging.md](proposals/Messaging.md), 1286 lines)
Binary request/response and WebSocket document subscriptions using FlatBuffers `.bfbs` schemas generated at runtime from service, screen-transition and Data Document definitions, with a Rust reflective reader, a browser reader/writer on Emscripten WASM, shared message buffers and large-scale fan-out. JSON and other contracts stay. Status: roadmap shows 0 of 6 phases done. Today `moqui-browser` speaks JSON ([`crates/moqui-browser/src/api.rs`](../crates/moqui-browser/src/api.rs)) and cites this plan for the binary forms. Prerequisites in place: Wasm browser build, cached QuickJS service functions, committed-update path. Go/no-go gates are in §19; capacity model in §12–13.

## Monitoring ([Monitoring.md](proposals/Monitoring.md), 110 lines)
Technology-independent requirements linking changes, telemetry, source-level diagnostics, business impact and recovery. Implementation delta: OpenTelemetry bridge over existing `tracing` spans, isolated telemetry schema, error fingerprints and profiles, server-JS profiling, regression baselines, incident screens, and an advisory-by-default recovery state machine. Status: requirements only; the artifact execution stack already opens `tracing` spans, which is the intended hook.

## Data load types ([Data.md](proposals/Data.md), 15 lines, "not fully verified")
Replace `seed`, `seed-initial`, `install`, `demo` with explicit ownership and sensitivity: seed (code-owned), configuration, secrets (deployment references only), transactions (PII needs fake replacements or exclusion), and a dev export that filters sensitive fields. Reuses the existing loader/writer and component ordering. Status: idea list; needs design before work.

## Cross-platform usability ([Usability.md](proposals/Usability.md), 15 lines)
Keep business behavior in `moqui-core`; platform hosts supply processes, storage, networking and lifecycle; desktop (native, webview or browser), embedded/POS, and browser-local WASM (built, see [Wasm](archive/Wasm.md)) with remote or local-only operation. Native-widget rendering would need a backend separate from the HTML/Vue macros. Status: direction only.

## Non-SQL entity datasources (no document yet)
Java serves a `<datasource object-factory="...">` group from a store that is not SQL: `ElasticDatasourceFactory` (Elastic or OpenSearch, about 1,200 lines in `org.moqui.impl.entity.elastic`) for the `logging` group, and `OrientDatasourceFactory`, which Java's own conf leaves commented out and which is not planned here. Today the `DatasourceFactory` trait and registry ([`crates/moqui-core/src/entity/datasource.rs`](../crates/moqui-core/src/entity/datasource.rs)) open only a SQL `EntityGroup` (a driver and a dialect), and a datasource whose factory is not registered is logged and its entities use the `transactional` group. An Elastic store would also need a non-SQL route through find, count, iterator and value writes, built on the existing `search` client. Low priority: in Java only `TestNoSqlEntity` uses `group="logging"` by default (moving `ArtifactHit` there is commented out), and a second SQL database for a group already works through `inline-jdbc`. Status: proposal only.
