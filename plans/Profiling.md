# Profiling

Status 2026-10-21: four rounds of profiling and optimization of entity, service, data load and XML/definition work against embedded Postgres, built and committed. Round 5 tested and measured against an external Postgres 18 in Docker ([Round 5](#round-5-external-postgres)), fixed what it found, and sends a data load's rows several to a statement ([Round 6](#round-6-multi-row-data-load-statements)). Round 7, on an M1 Pro (macOS), measured COPY and dropped it, added a multi-threaded benchmark and fixed the lock it showed, and cut what a transaction keeps per write ([Round 7](#round-7-macos-threads-copy-bookkeeping)); what is left leads the [Next steps](#next-steps). Round 10 measured QuickJS and its bindings with a new `js_speed` test and ranked what the profiles show ([Round 10](#round-10-javascript)); Round 11 fixed the five binding costs it found ([Round 11](#round-11-javascript-bindings)), Rounds 12 and 13 the per-row work of the html and qvt form-lists ([Round 12](#round-12-form-list-rows), [Round 13](#round-13-row-frames-and-qvt-row-values)). Companion to [Performance-Summary](Performance-Summary.md), which digests the performance plan; this document records what was measured and changed, how to measure, and what is left.

## How to measure

Seven opt-in speed tests in `crates/moqui-host/tests/` (each `test = false` and `#[ignore]`, so a default `cargo test` neither links nor runs them):

| Test | What it times | Needs |
|---|---|---|
| `entity_speed` | entity facade finds (cache, PK, list, count, iterate 100 rows) and writes (create, update, delete, `create_or_update`), in one transaction and in autocommit; then a cached find, a PK find and writes on 1, 2, 4 and 8 threads at once (`ENTITY_SPEED_THREADS`; `ENTITY_SPEED_THREADS_ONLY` runs that section alone) | embedded Postgres |
| `service_speed` | the `SpeedTest.xml` calls: entity-auto create/update/delete of `moqui.test.TestEntity` and `echo#Data`, plus the same entity calls direct, in one transaction and each in its own | embedded Postgres |
| `data_load_speed` | XML create and reload, CSV, JSON, nested records, and the framework seed data in `moqui load` order (first load and reload) | embedded Postgres |
| `xml_speed` | `MNode` parse, keep, read, lookup and drop over the 784 runtime XML files, and loader-style record elements | nothing |
| `definition_speed` | the framework entity model, the service definitions, and the base components' screen definitions | nothing |
| `js_speed` | QuickJS by the way the framework enters it: expressions (`from`, `condition`, `${}`), scripts, XML actions run directly, services (inline, script file, the service layer alone), the fancy template dialect (trusted and locked down), and the screens of `tests/fixtures/js-speed` in html, qvt and text; then a new thread's first script and first screen and the QuickJS heap each leaves (`JS_SPEED_ONLY` for sections, `JS_SPEED_ROW` for one row, as a profile wants) | embedded Postgres |
| `startup_speed` | a server's first start on an empty database (initdb, table check, empty-db load, second factory), restarts on the loaded database (server, and no listeners as `moqui tui`), the stop of each, and the factory boot alone; the repository's dev conf and components (`STARTUP_SPEED_COMPONENTS=base` for the base components only, `STARTUP_SPEED_LOAD` for the load types, `STARTUP_SPEED_LOG=info` for the phase lines) | embedded Postgres |

Run one with `CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test <name> -- --ignored --nocapture`; each file's header names its size variables. `SPEED_PG_HOST` (with `SPEED_PG_PORT`, `SPEED_PG_USER`, `SPEED_PG_PASSWORD`, `SPEED_PG_SSLMODE`, `SPEED_PG_SSLROOTCERT`) runs the three database tests on a new database of an external server instead, dropped at the end (`speed::SpeedDb`; the role needs `CREATEDB`). The shared harness (`tests/speed/mod.rs`) prints, per call: median time, allocations, bytes allocated, bytes kept after the batch (a cache or a leak), and SQL statements (`entity::statement_stats`, a round-trip count). The allocation columns come from a thread-local counting allocator and do not change from run to run, so they are the steady measure on a busy machine. QuickJS allocates with the C library's `malloc`, which that allocator does not see: `js_speed` adds a `js kept` column from `script::engine_memory` (QuickJS's own `JS_ComputeMemoryUsage`), and its `allocs` and `bytes` are the Rust side of the bindings only.

The speed tests take the conf's telemetry settings: on, at sample ratio 0 since Oct 27 (request and job roots kept, children only for error or slow traces). `MONITOR_SPEED_EXPORT=true` adds a recording exporter and prints the records and bytes produced. Set `telemetry_enabled=false` to time without telemetry, or `telemetry_sample_ratio=1.0` for full sampling; [Monitoring](Monitoring.md#overhead) has the measured cost. Rounds 1–13 predate telemetry.

Tools, as used here:
- **samply** for CPU (`samply record --save-only --unstable-presymbolicate`); needs `/proc/sys/kernel/perf_event_paranoid` at 1 or lower, which resets at reboot.
- **macOS `sample`** (`sample <pid> 2 -file out.txt`, no setup) for where threads wait: the leaf `__psynch_mutexwait` frames and the code above them name the lock. heaptrack and perf are Linux only.
- **heaptrack** for allocation sites: `heaptrack_print X.zst --flamegraph-cost-type allocations -F stacks.txt`, demangled with `c++filt`, filtered to the stacks under one benchmark row (its closure, or a function such as `ScreenFacade>::screen_def`). Identical row closures are folded into one symbol, so pick a row whose closure is unique.
- **perf stat** `-e instructions:u` with 1 and 6 batches, for a load-independent comparison (the difference over 5 is one batch, Postgres children included).
- **strace** `-ff -o DIR/t` for syscalls per round trip (`perf trace` has no tracefs access on the dev machine); the client thread is the file whose `sendto` carries `SET TIME ZONE`.
- **Comparing builds:** copy each build's test binaries aside under a tag, run them alternately, and take each row's best batch median. Wall times swing ±50–100% when other jobs run; allocation counts do not.
- **Waited round trips per call**: add a known delay on the server side (`tc qdisc add dev eth0 root netem delay 200us` in a container started with `--cap-add NET_ADMIN`, after `apt-get install iproute2`) and divide each row's added time by the delay. Connect to the container's bridge IP, not a published `127.0.0.1` port, which goes through docker's userland proxy.
- **Keep profiling output on disk** (`target/profiling/`), not in `/tmp`: `/tmp` is a RAM-backed tmpfs on the dev machine, and 23 GB of traces and binary copies there ran it out of memory during a workspace test build.

## What changed

Round 1 (commit `1cb412f`, started by Grok, reviewed and extended): the entity speed test, cached finds and autocommit writes.

Round 2 (commit `2f33ebb`): service speed test; rows mapped by moving values instead of cloning; binary timestamp decode; per-entity read shapes (column list and select head built once); cached per-entity read facts and implicit entity-auto service definitions; ECA and encryption copies only when an entity has rules or encrypted fields; Postgres parameters written straight into the send buffer.

Round 3 (commit `8b09f43`):
- **Driver on `tokio_postgres`** with `BEGIN` and `SET LOCAL statement_timeout` sent with the first statement of the transaction.
- **Deferred writes** (`DbConnection::execute_deferred` / `finish_deferred`): in a data file's transaction, creates and updates are sent without waiting; the next statement that waits, the loader's end of file, or `COMMIT` reports the first failure (and `COMMIT` refuses after one, since Postgres would roll back silently). The default implementation runs the statement at once, so MySQL and SQLite are unchanged.
- **`create_or_update` of a new row**: an `UPDATE` probe that matches nothing goes straight to `INSERT`, without the refresh `SELECT`.
- **Shared field names**: entity field maps are `FieldMap = BTreeMap<Arc<str>, Value>`, `DbRow` column names are `Arc<str>` shared per statement, and `EntityValue::set` takes the entity's own name instead of allocating one.
- Loader: shape cached per entity, records moved instead of cloned, attributes read in place, same-type values not converted.

Round 4 (commit `50a7e4b`, then the first-run prepare, shared record keys and definition-builder changes after it):
- **Upsert replaces try-insert.** A data load, CSV or ETL store of a record with its whole primary key is one `INSERT ... ON CONFLICT DO UPDATE` (MySQL `ON DUPLICATE KEY UPDATE`, SQLite `ON CONFLICT`), pipelined in a data load, wherever the write cannot differ from create-or-update: no create or update ECA rule, no field an update reads the stored row for (audit log, create-only, field defaults, optimistic lock), no data feed that wants the old values, no transaction cache or simulation, authorization off, and no non-key `statusId` (the entity-auto store checks a status change). Anything else takes the read-first store as before. `useTryInsert`, Java's `never-try-insert` and `EtlLoader::use_try_insert` are gone; `useTryInsert` parameters are accepted and ignored. An upsert's hit is a create's, and an explicit null in the record is written so an empty attribute still clears the field.
- **MNode.** The parser scans bytes, takes names and plain values as slices of the source, and counts line and column only where a node starts or an error is reported. Element and attribute names are interned `Rc<str>`; attributes are a small `Vec` instead of an `IndexMap`; one file location is shared per document; lookups (`children_named`, `first_named`, `first_named_attrs`, `write_string`) no longer copy. New borrowing calls: `has_name`, `with_name`, `with_attribute`, `attribute_is`, `has_attribute`, `child_count`, `for_each_child`, `for_each_attribute`, and `parse_flat` (an element with no children, read without a node).
- **Data loader** reads a flat record element with `parse_flat`, keys records by the entity's shared field names, and hands an upsert the record's map with no copy.
- **Postgres wire client** (`crates/moqui-drivers/src/pgwire.rs`): the protocol over a blocking Unix or TCP socket, TLS by rustls, `postgres-protocol` for message encoding, parsing and SCRAM. `tokio-postgres` and `tokio-postgres-rustls` are removed. A round trip is one write and one read. Every request ends with `Sync`, so an unanswered one (BEGIN, `SET LOCAL`, a deferred write, a statement close) is read and checked by the next call that waits.
- **First run prepares the statement** in the same round trip (Parse, Describe, Bind, Execute, Sync) where a prepare and a run took two; a first run that fails after parsing closes the statement so a later use prepares it again.
- **Definition builders**: the service loader's JavaScript syntax check reuses one parser arena per thread; the entity model copies an entity's nodes only when an `extend-entity` changes them; screen and service builders read names and children without copying.

## Results

Times are microseconds per call on the dev machine with embedded Postgres over a Unix socket, from alternating runs; allocations are exact. "Earliest" is the first recorded number for the row: round 2's start for entity rows and allocations, round 3's start for service and data load times, round 4's start for XML and definitions.

| Operation | Earliest | Now | Allocations earliest → now |
|---|---|---|---|
| entity one PK find, database | ~28 | 19.7 | 162 → 28 |
| entity cache hit | 1.21 | 0.82 | 31 → 12 |
| entity list 100 rows | 555 | 232 | 9,800 → 1,132 |
| entity create / update / delete (one transaction) | ~32 / 33 / 25 | 27 / 30 / 22 | 96 / 86 / 62 → 38 / 43 / 24 |
| service create, own transaction | 83 | 60 | 196 → 128 |
| service update / delete, one transaction | 37 / 28 | 33 / 24 | 137 / 106 → 66 / 42 |
| data load, XML record, new | 109 | 13 | 406 → 48 |
| data load, XML record, reload | 56 | 14 | 327 → 48 |
| data load, framework seed data, first load / reload | 146 / 68 | 46 / 21 | 576 / 497 → 87 / 74 |
| MNode parse, per file (784 files, 12.6 MB) | 176 | 76 | 2,723 → 608 |
| MNode retained memory, whole corpus | 78 MB | 39 MB | |
| framework entity model | 5.9 ms | 5.3 ms | 99k → 79k |
| service definitions | 9.7 ms | 8.7 ms | 79k → 39k (34 MB → 6.5 MB allocated) |
| screen definition, per screen | 131 | 112 | 1,282 → 467 |

Syscalls on the client thread for the same entity run: 96k with `tokio_postgres` (46k epoll waits, about 4 per round trip), 49k with the wire client (one `sendto` and one `recvfrom` per round trip).

What the numbers say:
- **Single statements are round-trip bound.** A PK find is about 20 µs of which the client's share is now small; halving allocations moved times by single digits. Over a network the round trip dominates more, so pipelining (deferred writes, BEGIN with the first statement, prepare with the first run) is worth more against an external server than these numbers show.
- **Data loads are where the design changed the outcome:** a record went from three waited statements to one pipelined statement, and the client work per record from 406 allocations to 48.
- **Memory.** Read rows share their column names, values share their entity's names, and parsed XML trees hold half what they did.

## Round 5: external Postgres

Setup: `postgres:18` (18.6) in Docker, default settings (shared_buffers 128 MB, fsync and synchronous_commit on), SCRAM auth, reached over the docker bridge (veth, ~40 µs ping RTT). Two containers: `moqui-pg18` with its data on disk (the dev conf's database, `runtime/conf/MoquiDevConf.xml`), and `moqui-pg18-mem` with `--tmpfs /var/lib/postgresql`, which matches the embedded test cluster (under `/tmp`, a tmpfs, so its fsync is free).

**Functional.** `./moq load` (8,740 records in 4.9 s), the server (REST reads and writes through mantle, tools and system screens) and `./moq test mr-mantle --db existing` (557 features, 61 specs, all passed in 74 s) work against `moqui-pg18`.

**Times** (µs per call, median batch; same binaries, run in turn):

| Operation | Embedded (socket) | Docker tmpfs | Docker disk | tmpfs + 200 µs delay | Waited round trips |
|---|---|---|---|---|---|
| entity one PK find | 20.9 | 28.8 | 34.3 | 240.6 | 1 |
| entity create / update / delete, one transaction | 27 / 31 / 23 | 40 / 42 / 33 | 43 / 50 / 38 | 253 / 257 / 246 | 1 each |
| entity create / update / delete, autocommit | 34 / 34 / 27 | 46 / 50 / 39 | 249 / 303 / 295 | 261 / 268 / 253 | 1 each |
| iterate 100 rows | 417 | 482 | 517 | 1565 | 5.4 |
| service create, one transaction / own transaction | 37 / 63 | 48 / 102 | 61 / 270 | 268 / 541 | 1 / 2 |
| data load, XML record, new / reload | 13.5 / 15.4 | 20.4 / 22.8 | 19.1 / 21.7 | 22.6 / 25.2 | 0.01 |
| framework seed data, first load / reload | 48 / 22 | 91 / 32 | 62 / 32 | 96 / 60 | 0.03 / 0.14 |

What the numbers say:
- **The pipelining works as designed.** Every single-statement call waits exactly one round trip; a service in its own transaction waits two (the statement, with `BEGIN` and `SET LOCAL` in the same write, and `COMMIT`); a 1,000-record data file waits about 11 (one per 128 records from `PIPELINE_DEPTH`, plus the end of the file). Allocations are the same as embedded.
- **TCP adds 8–15 µs per round trip** over the Unix socket on the same machine.
- **On disk, a commit is 250–300 µs** (WAL fsync) and dominates every autocommit and own-transaction row.
- **A data load is server bound.** Over TCP a record costs +7 µs more than over the socket, but the single backend used 4.07 s CPU in a 4.31 s run, so the client is waiting on the server. Holding deferred writes until 8 KB was buffered (one `sendto` per run instead of one per record: 12.4k sends for ~12k records) was 0–14% slower, as the server starts later; reverted. Fewer statements is the lever: multi-row upserts or `COPY`.
- **An iterate waits five round trips** for 100 rows: `DECLARE`, three `FETCH 50` (the third returns nothing, as a full batch cannot say it was the last), and `CLOSE`.

**TLS.** Against a server that requires TLS (`hostssl` only) with a certificate from a private CA, the driver fails with `invalid peer certificate: UnknownIssuer` (reported as a `postgres write` error): the external driver trusts only the `webpki-roots` public CAs, and there is no way to name a root certificate or to encrypt without verifying. A managed database with its own CA (AWS RDS, for one) cannot be reached over TLS. Without TLS, that server's refusal names `pg_hba.conf` and "no encryption", as it should. A server cannot be set to require SCRAM channel binding (only libpq's client-side `channel_binding=require` does that), so the missing `SCRAM-SHA-256-PLUS` refuses no server; it is hardening against a man in the middle only.

**Server restart under a running server** (`docker restart` takes 0.28 s):
- Requests right after the restart fail, about one per pooled connection, with `Connection reset by peer` (6 in a row here, then the 401 below), because a connection returned within `test_after_idle` (1 s) is handed out without `SELECT 1`. Each failure marks its connection dead, and requests succeed after that.
- Even with a 2 s wait, so every idle connection is tested, one request fails with `connection test failed`: `acquire` allows as many failed tests as there were idle connections, then returns the error instead of opening a new connection. When every idle connection is dead (any restart), that request fails although the server is up. `failed_checkout_test_discards_the_idle_connection` asserts this behaviour.
- That failure, in the login lookup, came back to the client as **401**, not 500 or 503.

**Round 5 fixes:**
- **Pool.** When as many tests have failed as connections were idle, the checkout opens a new connection instead of failing. A connection found dead (a failed statement or test, or the peek below) marks the time, and every connection idle since before then is tested whatever its age: a proxy, NAT or load balancer that drops connections says nothing until the client writes (docker's userland proxy on a published port does this). A checkout that cannot connect, or times out, is `ErrorKind::Unavailable`.
- **Idle peek.** The Postgres connection's `is_closed`, which the pool asks of each idle connection it takes, peeks without blocking at a TCP connection that has read nothing for 20 ms: the end of the stream, or an `ErrorResponse` among the waiting messages, is a closed connection; over TLS any waiting bytes are. A connection read from in the last 20 ms is not peeked, so a busy pool pays nothing (an alternating A/B run showed no change).
- **Status.** An `Unavailable` error anywhere in a request error's source chain answers 503, and a login the database could not serve is the request's error, not a failed login (no more 401). The request paths in `screen/mod.rs` (REST included) that hard-coded 500 use the same mapping.
- **TLS** (`db/pg_tls.rs`): `sslmode` `require` encrypts without checking the certificate, `verify-ca` checks the authority, and `verify-full`, empty, `allow` and `prefer` check the authority and the name (TLS is always required when on; there is no fallback to plain). `sslrootcert` (in `xa-properties`, a `jdbc-uri`, a search url, or the new `entity_ds_sslrootcert` property) is a PEM file of authorities to trust in place of the public ones, and makes `require` check as `verify-ca`, as libpq does. The handshake is run at connect, so a refused certificate reads `postgres connect: TLS handshake with <host>: invalid peer certificate: UnknownIssuer`.
- **Streams.** A stream's `DECLARE` goes out with its first `FETCH`, at the open (a refused query still fails there, with the `DECLARE`'s error), and `CLOSE` is not waited for. An iterate of 100 rows waits three round trips instead of five: 1,565 → 1,087 µs at 200 µs added delay, and 484 → 443 µs on the bridge.

Checked against the containers: a restart under a running server with requests sent as soon as Postgres is ready, 15 of 15 succeed (direct to the container, and through docker's proxy); while it is down or starting, requests answer 503 and succeed once it is up. `require`, `verify-ca` and `verify-full` with the private authority connect to a TLS-only server; `verify-full` with the public roots, and `require` with a root file that does not hold its authority, are refused with the handshake error; `./moq load seed -D entity_ds_host=... -D entity_ds_sslmode=verify-full -D entity_ds_sslrootcert=ca.crt` loads over TLS.

## Round 6: multi-row data load statements

A pipelined data load was server bound over TCP (round 5). pgbench in the container put the server's cost of a row at 9–16 µs as 100 single-row upserts in one pipeline, and 6 µs as one upsert of 100 rows.

What changed:
- **`DbConnection::execute_deferred_row(sql, params, key)`**: a deferred one-row `INSERT` or upsert the driver may send with the rows after it. `key` is a hash of the row's primary key (`value.rs` `pk_key`). The default sends it alone, so MySQL, SQLite and the test connections are as before. A data load's creates and upserts call it; its updates stay `execute_deferred` (each needs its own `zero_rows` check).
- **Postgres** (`RowBatch` in `postgres.rs`): in a transaction, rows of the same `INSERT ... VALUES (?, ...)` text are held and go out as `INSERT ... VALUES (...), (...)` with the statement's tail (`ON CONFLICT ... DO UPDATE`) unchanged. They go out when a row of another statement comes, when a row's key is among those held (one upsert cannot change a row twice; a hash that collides only splits the rows), when 100 are held (fewer for a wide entity: 30,000 parameters), and before any other call on the connection; a rollback drops them. A run is sent as statements of 100, 30, 10, 3 and 1 rows, each prepared once per connection. Rows go out in the order they came, so a row's foreign keys to rows before it hold; rows are never reordered across statements.
- **Statement cache** per connection 100 → 256 (pgjdbc's default). With five sizes per entity, a seed data reload at 100 pushed statements out and prepared them again with a waited round trip each, which made it 16% slower than three sizes instead of 21% faster.

Data load per record, µs (best of alternating runs; same binaries otherwise):

| Row | Embedded before → after | Docker tmpfs | tmpfs + 200 µs delay |
|---|---|---|---|
| XML create, new rows | 12.8 → 10.2 | 18.9 → 10.5 | 22.3 → 11.0 |
| XML reload, same values | 14.2 → 10.3 | 21.6 → 10.5 | 25.1 → 11.2 |
| CSV create | 12.7 → 9.0 | 19.4 → 9.2 | 22.4 → 9.6 |
| nested, type + 9 enums | 21.1 → 16.0 | 31.1 → 21.0 | 34.5 → 21.9 |
| framework seed data, first load | 45.6 → 48.2 (one sample a run) | 57.2 → 55.1 | 97.5 → 97.9 |
| framework seed data, reload | 20.6 → 18.7 | 28.9 → 22.9 | 59.8 → 51.3 |

`./moq load` of every data type into `moqui-pg18` (8,740 records): 4.87 s → 4.34 s. The seed first load gains little: its records of one entity often name different fields (each set of columns is its own statement), and the entities with ECA rules, status fields or audit logs take the read-first store.

## Round 7: macOS, threads, COPY, bookkeeping

Run on an M1 Pro (macOS, 8 cores, 32 GB), where the embedded cluster's `/tmp` is on the APFS disk, not a tmpfs: a commit there pays a real fsync. Allocation counts were the same as on the Linux machine on every row before the changes below; times were equal or faster, except anything that commits, which was 25–30 µs slower per commit (autocommit create 34 → 53 µs, service create in its own transaction 60 → 75 µs).

**COPY, measured and dropped.** A throwaway test against the embedded cluster timed 10,000 rows of eight columns in one transaction (best of 5, µs per row; the prototype is kept at `target/profiling/round7/copy-prototype.patch`, not committed):

| Variant | New rows | Reload (every row conflicts) |
|---|---|---|
| 100-row `INSERT ... ON CONFLICT DO UPDATE` (round 6) | 4.57 | 6.41 |
| COPY into a temp stage, `INSERT ... SELECT ... ON CONFLICT`, `TRUNCATE` | 4.51 | 6.25 |
| the same with an `ON COMMIT DELETE ROWS` stage and `DELETE` | 4.20 | 6.36 |
| COPY straight into the empty table | 2.72 | — |

Staging saves nothing, since the server's upsert of a row is the cost, not reading its parameters. Direct COPY is 40% cheaper, but only for rows known to be new; with a data load at about 10 µs a record, that would be 19% of an empty-table load, for a guarantee that no row is already there (or a savepoint and a retry when one is). `./moq load` of 8,740 records takes about 4.3 s, of which the writes are well under 0.5 s, so the time of a load is elsewhere (startup, the schema, the read-first stores). Not done.

**Threads.** `entity_speed` runs a cached PK find, a PK find in a transaction and create/update/delete in a transaction on 1, 2, 4 and 8 threads at once, each thread on its own connection (`speed::Rows::run_threads`: a barrier start, the slowest thread's time, each thread's own allocation counts summed). On eight threads the cached find fell to 418k calls/s from 1.04M on one: `sample` put nearly all the waiting in `EntityFacade::with_slot`, the facade-wide mutex that `read_meta`, `find_l10n` and the list cache lookup each took. Then in `L10nMemo::clone` and `ReadState::clone`, mutexes locked to clone a cached value that every thread clones.
- The facade slots are an `RwLock`; `with_slot_read` is the lookup path (read metas, l10n targets, read shapes, SQL text, the list and count cache lookups, views by member, the cluster link, loader shapes, implicit service definitions). `ByDefinition::get` takes `&self`, and `insert` drops an older generation.
- `L10nMemo` is a `OnceLock`, `ReadState` an `RwLock`. On macOS this also drops one allocation per value read (a std `Mutex` there allocates its pthread mutex): iterate 100 rows 1,503 → 1,303 allocations.

| Calls/s, best of 3 | 1 thread | 2 | 4 | 8 |
|---|---|---|---|---|
| cached PK find, before → after | 1.05M → 1.06M | 1.14M → 1.33M | 567k → 1.01M | 418k → 956k |
| PK find in a transaction | 74.5k → 74.9k | 93.7k → 98.2k | 111k → 117k | 141k → 161k |
| create, update, delete in a transaction | 41.4k → 41.9k | 73.0k → 75.0k | 89.0k → 94.2k | 106k → 119k |

The cached find still does not scale past two threads: what is left is the slots lock's reader count and the cache entry's atomics, one cache line every thread writes (see Next steps).

**Transaction bookkeeping.** A write in a transaction kept its whole field map until commit twice: in the frame's record for the end-of-frame cache eviction, and in the data-feed touch.
- A write of an entity with no cached find or count keeps no eviction record: the entity's frame entry is the existing "clear whole" state, which finds nothing to clear at the end unless another thread cached a find meanwhile. The operation is a `&'static str`, not a `String`.
- A data-feed touch keeps the values only when the feed configuration is unknown, older than the feed definitions, or has a document on the entity; the commit reads no values for any other.

| One transaction | Kept per write, before → after |
|---|---|
| entity update / delete | 1,188 → 110 / 1,104 → 27 bytes |
| entity `create_or_update` | 1,523 → 383 bytes |
| service create / update / delete | 1,277 → 215 / 1,191 → 117 / 1,269 → 207 bytes |
| commit of the batch (1,000 of each) | 1.61 → 0.72 ms |

Every write and every loaded record also allocates once less. A create still shows about 1.3 KB kept per call: the value the test keeps in its own list.

**A pipelining deadlock on macOS.** The workspace tests hung on macOS: a `moqui test` load's log-shipping thread blocked writing a large statement while its backend sat in `PARSE`. The wire client sends requests without waiting for the replies before them and read nothing until its write finished; once those replies filled the socket the server stopped reading, and each side waited on the other's write. A macOS Unix socket holds 8 KB each way (`net.local.stream.sendspace` and `recvspace`), against about 200 KB on Linux, so it showed there first; over TCP it needs megabytes of unread replies. `Wire::send_all` now writes with a 2 ms send timeout and, each time the write would block, reads what the server sent onto the read buffer, where the next read parses it (libpq's `pqSendSome` does the same). Over TLS those bytes are read and decrypted through the rustls connection, as `StreamOwned::read` would first retry the blocked write, and the timeout is set only for the write. A Unix or TCP socket has the timeout from the connect, so a round trip makes no more system calls than before (PK find 13.5 µs before and after). `a_write_reads_what_the_server_sends_while_it_waits` (Unix) and its TLS twin send 2 MB and 16 MB queries to a fake server that first writes as many bytes of notices; with the old write the Unix one times out. With it, `cargo test --workspace` runs to the end on macOS; what still fails there are Linux assumptions in tests (`/proc/<pid>`, binding `127.0.0.2`, `/tmp` against `/private/tmp`, `node --test` before Node 18).

**MNode copies.** `x.attribute("a").as_deref() == Some(..)`, `.attribute(..).is_some()` and `x.name() == ".."` in the screen, form, service and entity-model builders are `attribute_is`, `has_attribute` and `has_name`, and `is_true` and `bool_attr` read in place. Allocations: entity model 79,470 → 78,253, service definitions 38,690 → 38,661, per screen 467.5 → 464.1. The copies were a small part; the `match x.name().as_str()` arms and `for c in x.children()` loops were left, since closures there would cost readability for about another percent. Where the rest goes needs heaptrack on Linux.

## Round 8: MNode consumers at run time

A review of where the framework reads `MNode`s, for trees copied or parsed again where one shared, read-only tree would do. Definitions hold XML as text, since `MNode` is `Rc` and a cached definition must be `Send + Sync`; the copies that mattered were readers parsing that text, or the merged conf, on each request.
- **Root screen per request.** `screen::serve` called `root_screen_for_host`, which parsed the whole merged conf on every request to read the `root-screen` elements. They are read once per factory (`info::root_screens`); `getAllRootScreenLocations` reads the same list.
- **Server-static `actions` transition.** Each data request parsed the root widgets, every section's widgets (a `section-iterate` once per entry), every form and extension (for `wants_pass_through`) and the assembled form. A read-only reader now takes `XmlFragment::shared_node` / `FormInstance::shared_node`: the tree this thread parsed before, by the text's `Arc` (`definition::shared_parse`, at most 64 fragments and 256 KB of text per thread). The tree is shared, so it must not be changed or appended elsewhere; code that changes a tree still takes `node()`. `sri.getFormNode`/`getFormInstance` use it too.
- **Facts read at definition load.** A form's `extends`, `saved-finds` and saved-find key (`saved_finds::FindKeySource`, the raw `auto-fields-service`/`auto-fields-entity` names) are `FormDef` fields; the menu, `sri` saved-find calls, `extended_section_screen` (every form of the screen, for a section not found) and the static transition parsed the form for them. An aggregating form-list keeps its shown columns by `_uiType` (`form::ListColumns`) instead of parsing the assembled form per render.
- **Rule conditions.** An EECA, email ECA or git ECA condition was parsed and written out again on each check; it is kept as the `<and>` group a SECA keeps (`rule_actions::first_condition`) and runs through `eval_condition_group`.
- **Smaller.** `ec.screen.getFormNode`'s form XML is the assembled text, not parsed and written again; the macro walker and `node_object` read names, text and attributes borrowed (`MNode::with_text` is new).

The speed tests do not cover screen renders, so these have no before/after numbers; the five tests were run before and after for regressions: allocations were the same on every row but screen definitions (471.1 → 471.4 per screen, the new `FormDef` fields), and times within run-to-run noise (alternating runs of the entity rows overlap).

Then what that pass left:
- **One conf parse per thread.** The shared parse moved to `moqui-lib` as `MNode::parse_shared(location, text)`, found by the text's content (so any `&str` of the same document hits, not only one `Arc`), at most 64 documents and 1 MB of text per thread. The merged-conf readers (artifact hits, service and request pools, `ConfCache`, webapp scripts, template renderers, screen conf, token routes, root screens, LLM and search conf, web protection, load flags, datasources, service files) take it, so startup parses the conf once on its thread instead of about fifteen times. `XmlFragment::shared_node` and `FormInstance::shared_node` use it too. Every reader only reads.
- **`parseNode`** sends the root's `xml` only; the JS node wrapper writes an element below it, as `write_string` does, when its `toString()` is first called. It wrote every element's XML, each again inside each ancestor.
- **Builders and walkers.** `MNode::name_rc` (the interned name, no copy) for `match &*node.name_rc()`; `child_iter` and `named_iter` walk the children by index with no list copy and no borrow held between steps, for loops that use `?`, `continue` or `return`. Mechanical across the crates: `match x.name().as_str()` → `name_rc`, `x.name() == ".."` → `has_name`, `x.attribute("a").as_deref() == Some(..)` → `attribute_is`, `.is_some()` → `has_attribute`, a `match` on `x.attribute(..).as_deref()` whose arms do not return → `with_attribute`, and 111 read-only `for` loops over `children()`/`children_named()` (a loop whose body changes any node, or passes its parent elsewhere, keeps the copy). The entity model also built a set of every dictionary type name for each entity; it borrows them now. Allocations: entity model 91,327 → 84,115, service definitions 39,435 → 33,929, per screen 471.1 → 466.3, seed data first load 91.4 → 88.8 per record; times within noise.

## Round 9: startup

`startup_speed` (new) on the M1 Pro, full component set (framework, tools, webroot, example, mr-mantle, mr-marble, mr-boss), empty-db load `seed,seed-initial,install` (8,746 records), embedded Postgres in the test's own directory. From the phase lines (`STARTUP_SPEED_LOG=info`):

| Phase | First start | Restart |
|---|---|---|
| factory boot (conf merge, components, properties, caches) | 0.05 s | 4 ms |
| open the database (first: initdb and start; restart: start the postmaster) | 1.81 s | 128 ms |
| entity definitions (1,020, 339 views) | 36 ms | 34 ms |
| search attach, tools, ECA rules, service definitions | 0.2 s | ~25 ms |
| table check (680 tables, 2,099 indexes, the FKs) | 2.1 s | none |
| empty-db load | 2.3 s | none |
| second factory after the load, listeners | ~0.5 s | ~15 ms |
| **whole start, as the test times it** | **6.8 s** | **268 ms** (no listeners 264 ms) |
| stop | 142 ms | 142 ms |

A restart is mostly the postmaster's start and stop; the framework's own part is about 80 ms. The factory alone boots in 4 ms.

**Profile** (macOS `sample` of the first start; it suspends the process at each tick, so it overweights client CPU against time spent waiting on the postmaster, and each finding was checked against the unsampled phase lines):
- The table check is all waits on the server, one DDL statement at a time (`first_run` → `recvfrom`): server-bound.
- In the empty-db load, the data-feed commit hook (`feed::insert_pending`, the pending rows of the `MantleSearch` and mail feeds) dominated. Each write of a non-primary feed entity walks the document path back to its primary entity, and each step called `query_rest::relationship_pairs`, which cloned the whole relationship and built an `EntitySnap` of the related entity (its fields, masters and every relationship, hundreds on the mantle entities with their automatic reverse relationships) only to read its full name and the key pairs.

**Change:** `relationship_pairs` copies the related name and the key pairs only. Empty-db load 2.28–2.37 s → 1.13–1.19 s (two runs each); first start 6.80 s → 5.63 s (−17%); restarts unchanged (they write nothing).

**What is left, by expected payoff:**
1. **Table check, 2.1 s of a first start.** Server-bound DDL in sequence. Tables and their indexes could be created on several connections at once (FKs after, in sequence, since an FK locks both tables); pipelining saves only the ~20 µs round trip of each of ~5,000 statements.
2. **Data feed probes.** Each walk step and reread is a savepoint, the query and a release, three waited round trips (`feed::probe`); a savepoint and its release sent with the query would make it one. The walk also reads the parent's field names per step and the entity's pk names per touch.
3. **initdb, 1.8 s,** is the embedded cluster's first start; a template cluster copied in (as the test gates do) would skip it.
4. ~~Feeds reindexed on every start~~: not so. "Found index does not exist for DataFeed MoquiMailSearch / MantleSearch" is logged once, by the second factory of a first start: the empty-db load wrote the `DataFeed` records and no store has been made for their documents, so `indexOnStartEmpty` indexes them (160 ms, in the background), as Java does. Later restarts find every document's `document_index` row and start no job. (The line was read from the second factory's start, taken for a restart.) The line now names the documents it found missing. The same load also queued pending feed rows for the documents it wrote, which the feed workers deliver as well, so those documents are indexed twice on a first start; small, and not looked into further.

## Round 10: JavaScript

Measurement only; [Round 11](#round-11-javascript-bindings) has the changes. `js_speed` (new) on the M1 Pro, release with debug info, 2000 calls (services 1000, screens 200) x 5 batches; times are the median batch, allocations the Rust side. Profiles are macOS `sample` of one row at a time (`JS_SPEED_ROW`), read as inclusive shares of the test thread. Before it, the only script number was `echo#Data` in `service_speed`; no speed test rendered a screen or a template.

| Row | us/call | Rust allocs | What it says |
|---|---|---|---|
| expression `a`; `a + b * 2`; condition | 0.43; 0.64; 0.74 | 1-3 | the floor of an engine entry with a compiled expression is low |
| expression `item.name` (a context map) | 3.4 | 27 | a bare read of a map converts the map to JS |
| expression `itemList.length` (100 maps) | 264 | 2,108 | a bare read of a list converts it, and converts it back at the end |
| expression, new text each call | 0.66 | 3 | compiling a short expression costs almost nothing over a cached one |
| script, 1000 adds to a local / to a bare name | 18.6 / 865 | 0 / 4,003 | 19 ns per JS loop step; 0.85 us per bare-name read and write through the proxy |
| script, 100 decimal multiply + add | 113 | 605 | 1.1 us per decimal operation |
| script, 10 cached finds reading `description` | 189 | 3,060 | 19 us per find from JS; the XML `entity-find-one cache="true"` is 4.7, the Rust cache hit 0.8 |
| JS work: 1000 string concatenations; JSON round trip of 100 objects | 147; 115 | 0 | QuickJS's own speed; nothing to fix here |
| actions: empty; one set; four sets | 0.35; 1.3; 4.2 | 0-25 | |
| actions: iterate 100 context maps with if and set | 729 | 4,242 | 7.3 us per iteration; the same in one script element is 47 for the whole service |
| service noop#Inline; echo#Inline; echo#Script; echo#Data | 3.6; 10.3; 10.2; 10.8 | 33-100 | the service layer is a third of an inline service; a `.js` file costs what inline actions do |
| service loop#Actions / loop#Script (100 items) | 735 / 47 | 3,730 / 265 | XML iterate is 15 times the script |
| service call#Nested | 14.2 | 99 | a `service-call` from actions adds about 9 |
| template, fancy over 20 items, trusted / locked down | 105 / 88 | 433 / 176 | |
| screen Plain html; Labels (40 expanded labels); Single (8 fields) | 16; 215; 359 | 88-1,982 | 5 us per expanded label, 40 per form-single field |
| screen List html 20 / 100 rows | 2,865 / 13,773 | 15,525 / 76,607 | 136 us and 766 allocations (59 KB) per row of 7 display fields |
| screen List qvt / text, 100 rows | 23,014 / 4,825 | 85,235 / 47,219 | qvt is 230 us per row, text 48 |

What a thread pays: its first script is 3.4 ms (engine start, the context proxy and helpers) and leaves 560 KiB of QuickJS heap; its first screen (List html, 20 rows) is 10.6 ms more, starts a second engine (the screen units run nested) and brings the thread to 1.5 MiB. After all the rows the test thread's two engines hold 2.9 MiB. So 41 request and service workers that have each rendered a screen hold about 60 MiB of QuickJS heap, and every new worker thread's first request is about 14 ms slower.

What the profiles show, by expected payoff:
1. **A failed class check throws.** `js_value_to_host_in` asks each plain JS object, in turn, whether it is a `HostDecimal`, `HostEntity`, `HostList`, `HostUpload`, `HostStream`, `HostTool`, `HostCondition`, a typed array and an `ArrayBuffer`. rquickjs 0.14's `instance_of` calls `JS_GetOpaque2`, which on a miss makes a `TypeError` (a `vsnprintf` message and a backtrace) and then catches it; `JS_GetArrayBuffer` does the same. That is 34% of the list-read row, 25% of XML iterate and 18% of the trusted template. A plain object has class id `JS_CLASS_OBJECT` (1, the first class), which none of these are, so one `JS_GetClassID` check can skip them all; for other objects, `JS_GetOpaque` (which does not throw) with rquickjs's class id would do.
2. **Tracked values are converted back at the end of every script.** A list or map read by a bare name is converted to JS and tracked, and `flush_tracked` converts it back and writes it to the stack when the script ends, changed or not: 84% of `itemList.length`, 32% of XML iterate (with a flush per host call), 43% of the trusted template. A tracked value the script did not change needs no conversion: a cheap "touched" mark (a frozen-until-written copy, or a version on the JS object set by the write paths the bindings see) or a comparison against the host value before converting would take most of it.
3. **Localized fields from JS.** Reading `description` of an entity value from a script runs `localize_field`, a cached `LocalizedEntityField` find whose key is built and cloned each read: 81% of the cached-find row, about 15 us per field read. Remember per facade generation which entity fields have any localization rows (most have none), or cache the lookup per (entity, field, pk, locale).
4. **Bare names cost a proxy trap each.** A bare read or write goes through the `globalThis` proxy (`has`, then `get` or `set`), each a Rust callback with an atom-to-string conversion and a context-stack lookup: 0.85 us per `total += i`. Decimals also make a new `HostDecimal` object on every read of the name (28% of the decimal row). Keeping the converted JS value per name for the length of the script (as lists and maps already are) would cut the reads; the writes need the stack.
5. **Screens.** About half of a form-list row is the JS of the screen macros (`ScreenHtml.js`, `ScreenQvt.js`), with regular expressions (escaping, `replace`, `split`) about 11% of qvt. The rest is the `sri` calls: `lit` converts the field's element object (name, attrs, children and parent) into a `NodeLit` on every `getFieldValueString`, `getFieldValueClass` and similar call, 12-14% of the render, so a field costs its conversion once per row and per call. Give each element object of a unit an id and keep its `NodeLit` per unit, or pass the field's name and look it up in the form definition on the Rust side.
6. **Thread start.** 3.4 ms and 560 KiB for an engine, and a second engine for screens. Snapshotting the helpers is not possible in QuickJS, but the engine could be started when a worker thread starts rather than on its first request.

## Round 11: JavaScript bindings

The five binding costs of [Round 10](#round-10-javascript), fixed, on the same machine and test (`js_speed`, 2000 calls x 5 batches, medians). Allocations are the Rust side; the QuickJS heap after the rows is the same (2.9 MiB for the test thread's two engines).

| Row | us/call before | after | Rust allocs before → after |
|---|---|---|---|
| expression `item.name` (a context map) | 3.39 | 1.13 | 27 → 4 |
| expression `itemList.length` (100 maps) | 264 | 51 | 2,108 → 3 |
| script, walk 100 context maps | 272 | 55 | 2,108 → 3 |
| script, 1000 adds to a bare name | 865 | 464 | 4,003 → 4,003 |
| script, 100 decimal multiply + add | 113 | 72 | 605 → 408 |
| script, 10 cached finds reading `description` | 189 | 38 | 3,060 → 390 |
| actions, four sets (the `echo#Data` body) | 4.18 | 2.60 | 25 → 22 |
| actions, iterate 100 context maps with if and set | 729 | 276 | 4,242 → 2,121 |
| service `echo#Data` | 10.8 | 8.9 | 95 → 95 |
| service `loop#Actions` (100 items) | 735 | 355 | 3,730 → 3,714 |
| service `find#Cached` (10 cached finds) | 195 | 42 | 3,088 → 418 |
| template, fancy trusted over 20 items | 105 | 64 | 433 → 7 |
| screen Single html (form-single, 8 fields) | 359 | 216 | 1,982 → 820 |
| screen List html, 100 rows | 13,773 | 10,030 | 76,607 → 46,941 |
| screen List qvt, 100 rows | 23,014 | 17,654 | 85,235 → 45,769 |
| screen List text, 100 rows | 4,825 | 2,298 | 47,219 → 30,263 |
| a thread's first List html render | 10,596 | 9,335 | 16,349 → 10,869 |

What changed:
1. **No throwing class checks for plain objects.** `js_value_to_host_in` reads the object's class id (`JS_GetClassID`); a plain object (`JS_CLASS_OBJECT`) goes straight to its properties, skipping the host class, typed array, `ArrayBuffer` and `Date` checks, each of which made and dropped a `TypeError` with a backtrace on a miss. `js_bytes` asks `JS_IsArrayBuffer` first, so a decimal or other host object converts without a throw either (rquickjs gives all Rust classes one class id, so their `as_class` checks do not throw).
2. **Tracked values the script did not change are not converted back.** At a flush, `js_matches_host` compares the script's object with the stack's value in place (array lengths and items, property atoms in order, strings through `JS_ToCStringLen`, numbers by kind, decimals by value) and skips the conversion and the write when they agree; anything it cannot tell (bytes, host objects, Sets, tools maps, a number of another kind) converts as before. A bare read converts the stack's value by reference (`ContextStack::with_value`, `host_ref_to_js`) instead of copying it first, except a value holding a tools map, whose wrapper runs script code.
3. **Localized field reads are remembered.** `localize_field` keeps what each (entity, field, primary key, stored text, locale) found with the facade, until a write to `LocalizedEntityField` or `LocalizedMessage` (`entity::cache::localized_generation`, counted at the write and again at its transaction's end, as the feed definition generation is). Not in a simulation.
4. **Bare names resolve through an exotic object, not a Proxy.** `globalThis`'s prototype is a `HostGlobalNames`, whose get, set and has hooks QuickJS calls directly with the atom (the same hooks QuickJS runs a Proxy's traps behind), with the same answers the traps gave; the `has` hook asks `JS_GetOwnProperty` of the global object instead of calling `Object.prototype.hasOwnProperty` from Rust. A decimal read by a bare name is tracked as lists and maps are, so each read is the same object and not a new host object (`big == big` holds now; two names with equal decimals still do not, use `eq`).
5. **Screen element literals are made and converted once per unit.** `nodeLit` and `bareLit` (`ScreenSupport.js`), and the widget and field literals of the html and qvt modes (`wlit`, `fieldLitOf`), collect each distinct literal while a unit is generated (`beginLits`, `endLits`, `hoist`); the unit is `(function () { const $lits = sri.holdNodeLits([...]); return function ($depth) {...}; })()`, so the literals are made when an engine compiles the unit. `holdNodeLits` attaches each literal's `NodeLit` (and its parents' and children's) as a hidden host object and freezes the literal; `lit()` reads that instead of converting the object on every call and every form-list row. Generation outside a unit (`generate_xml`, the golden tests) is as before.

Checked with the moqui-core suite (1,864 tests), moqui-host's (the tools and system screens over HTTP among them), the webroot and framework template node tests, `./moq test mr-mantle mr-marble` (778 of 781 features, as before: the WorkTimeEntries CSV import feature fails on master too, only after the other specs), and new tests: a tracked value the script did not change keeps the stack value (a time stamp stays a time stamp), a translation written after a read is seen, and a bare name assigned many times does not grow the engine.

What is left, by expected payoff:
1. ~~Screen macros~~: [Round 12](#round-12-form-list-rows).
2. **Bare-name writes convert.** `write_name` converts every list or map written to a bare name to host values (in iterate, each row's entry), so host code that reads the stack directly sees it. A lazy write, converted at the next flush, would need every direct stack read to flush first.
3. **Atom to `String` per bare-name access.** The exotic hooks still make a Rust `String` of the name (4 allocations per `total += i`); a per-engine table from atom to name would remove them.
4. **Thread start.** 3.1 ms and 560 KiB for a thread's first engine, and a second engine for screen units: start the engine when a worker thread starts.

## Round 12: form-list rows

The html and qvt form-list macros per row, on `js_speed`'s List screen (100 rows of 7 display fields, 200 renders x 5 batches) and its Single screen, now 11 fields with a style, a tooltip, a currency, raw HTML and a line break, also rendered in qvt. QuickJS shows no JavaScript function names to `sample`, so the generated units were read (`generate_unit_source`) and each change measured; the host calls were summed from the profiles. `JS_SPEED_DUMP` (new) writes each screen's output, and the output with the macros as committed and as changed was the same for every screen, the session token aside.

| Screen | before Round 11 | after Round 11 | now |
|---|---|---|---|
| List html, 100 rows (ms) | 13.8 | 10.0 | 7.15 |
| List qvt, 100 rows (ms) | 23.0 | 17.7 | 11.7 |
| List html, 20 rows (ms) | 2.86 | 2.08 | 1.49 |
| Single html (us) | 359 (8 fields) | 216 (8 fields) | 267 (11 fields) |

A row's 7 cells made about 75 host calls (per cell `pushContext`, `popContext`, `getFieldValueString`, `getFieldValueClass`, `getFieldValuePlainString`, `ec.context.get("sectionEntryIndex")` and three writes, plus the row's frame), but after Round 11 those were only about a fifth of the row; about two thirds was QuickJS running the macro functions. What changed:
1. **No object per tag.** The html `display`, `rowCellOpen` and row `<tr>`, and the qvt `displayWidget` and `rowCellOpen`, write their attributes one by one (`a1(key, value)`, as `A` writes one) in `A`'s order, where each built an object for `A` to walk with `Object.keys`, and `cls` with a rest array, `filter` and `join`. The value's class, `currency` and the alignment are class names and are not escaped. This was most of the gain: html 10.0 to 7.25 ms.
2. **A display's attributes are read once.** A `WeakMap` by element keeps what `display` reads of it (text, currency, also-hidden, encode, style, tooltip, the field), since Round 11 made the elements one frozen object per unit. In qvt it also keeps the field's label, which ran a lookahead regular expression split of the name and an `ec.l10n.localize` call every row; the label is localized, so the map is new at each render's outermost unit, and a title with an expression is not kept.
3. **One host call for a field's values.** `sri.getFieldValueTexts(widget, withString)` gives the value string, the plain string and the value's class from one `get_field_value`, where `getFieldValueString`, `getFieldValuePlainString` and `getFieldValueClass` each looked the value up. The qvt `fields-initial` of a display uses it too.
4. **`sectionEntryIndex` once a unit.** `sec()` keeps it until the unit ends: a section-iterate renders each entry as a unit of its own in a frame of its own, and nothing else sets it.
5. **A test before each replace.** `t`, `sq` and the line breaks of a display replaced every value; now a regular expression test comes first. `indexOf` checks were tried for `e` and dropped: QuickJS's `indexOf` compares the needle at every position, so four of them over a row's `fields-initial` text cost more than the one regular expression scan.

What is left of a row, by the last profiles (html about 70 us, qvt 115):
1. **The interpreter.** About a fifth of the time is `JS_CallInternal` itself, the closures a cell runs (`fid`, `fnm`, `entry`, `owner`, the generated `cell` function with its `try`/`finally`). Fewer, larger runtime functions per cell (one call that writes a whole display cell) would cut it further.
2. ~~The row frame~~ and 3. ~~qvt `fields-initial`~~: [Round 13](#round-13-row-frames-and-qvt-row-values).

## Round 13: row frames and qvt row values

What Round 12 left of a row. Its List screen's notes now hold `& < > " ' \` and a line break on every fifth row, so the escaping is in the output compared; with the macros as committed and as changed, and before and after the Rust changes, every screen's output was the same.

| Screen | Round 12 | now | Rust allocations a render |
|---|---|---|---|
| List html, 100 rows (ms) | 7.08 | 6.3-6.7 | 45,641 → 28,707 |
| List qvt, 100 rows (ms) | 11.7 | 9.6-9.8 | 40,775 → 25,351 |
| List text, 100 rows (ms) | 2.44 | 1.95 | 30,263 → 18,979 |

What changed:
1. **A popped frame is not copied.** `pop_context_frame` cloned the frame's whole map to return it, also for the callers that drop it (every `sri.popContext`, each row's `endFormListRow`); a popped frame is the stack's alone unless a saved context holds it, so it is unwrapped (`Rc::try_unwrap`) and copied only then. `ContextFrame::take` gets the same, so `openFormList` no longer copies each row it builds either.
2. **A row frame is one map.** `ContextFrame::push_entries` pushes a frame made of an owned map (a `context` entry left out, as a put of it is refused). The row frame (`form_map_frame`) is the row's fields plus `_formMap`, one copy of them, where a put of each field copied them again; `openFormList` builds each row's frame from the item's fields, the entry, the index and has-next in one map, the three names formatted once a list instead of once a row.
3. **The qvt row's display values are read once.** A display's `fields-initial` value and its cell read the same values in the same row frame; the row keeps what `fields-initial` read (`st.rowTexts`, for a field part with no `set`, cleared before any sub-rows), and the cell takes it, so a row makes 7 `getFieldValueTexts` calls instead of 14.
4. **The qvt row's `m-form` tag in one pass.** `fields-initial` is written by `jsAttr`, which escapes each string for the string literal and the attribute with one regular expression test (their characters are apart) instead of a test for the literal and then a scan of the whole serialized text for the attribute; a key's text is kept, as it comes again on every row. The list url's parameter map is read once a list, and `orderByField` once a row.

**A plain `list` is the context's value.** `openFormList` read a `list` that is not an `entity-find` with `eval_value`, so the list went to JavaScript and back: a time stamp, date or time in its rows came back as its epoch milliseconds (text, class String, shown as `1709647620000`), and a long or an integral double as an integer (class Integer). Java's `getListObject` evaluates the name to the object itself and `getFieldValueString` formats a time stamp with the field's format. A `list` that is a plain name held in the context is now read as it is (an expression, or a name not in the context, is still evaluated): such a row's time stamp is `2024-03-05 14:07` with class Timestamp, and a long is a Long. A probe over the mr-mantle and mr-marble specs (137 form-list renders) found no form-list whose rows differ: their plain lists hold entity values, which the trip does not change, or were made by scripts, whose values were converted when written (a script's dates are millisecond text already). It matters for rows a host call put in the context with their types (a Rust service's out-map). The one visible difference in `js_speed` is the class of a script's integral number (`1.25 * 0`, held as a long): Long where the trip gave Integer, styled the same (`.display.Long`, `.display.Integer`).

What is left of a row: the interpreter running the macro closures (a fifth of the time), the regular expression tests that remain (the qvt `fields-initial` values, `t` and `e` of each cell), and the row frame's one copy.

## Behaviour changes to know

- A data-feed touch of an entity no feed document has keeps no values, once the feed configuration has been read and no definition was written since. If a definition that adds the entity is committed by another thread between the write and its commit, that commit feeds the write through the Postgres `xmin` probe (other databases miss it), as for a touch recorded without values.
- A write in a transaction of an entity with no cached find or count clears the entity's caches whole at the end of the frame, instead of evicting by the values written. Only a find cached by another thread during the frame is affected, and it is cleared rather than kept.

- A data load's failed row is reported for the statement of many rows it went out in: the server's message (which names the key for a constraint) is the same, at the same place (the next waited statement or the end of the file).

- `sslmode=require` on an external Postgres now encrypts without checking the server's certificate, as libpq and pgjdbc do (it checked authority and name before). `verify-full` or empty keeps the full check; `sslrootcert` makes `require` check the authority.
- A request that fails because the database cannot be reached answers 503 (it was 500, or 401 when it held credentials).

- An upserted data-load record records a `create` artifact hit whether or not the row existed, and runs no update ECA rules (it is only used where the entity has none).
- `create_or_update` of a new row no longer records a `refresh` hit.
- A deferred write's failure surfaces at the next statement of the transaction or at the end of the data file, not at the write itself. The message is the failed statement's.
- `EntityValue::fields()` returns `&FieldMap`, `set` takes `impl IntoFieldName`, `DbRow` names are `Arc<str>`, `run_matched_eca` takes a `FieldMap`; `string_map` and `field_map` convert. Add-on component crates outside the workspace need the same small changes.
- A list or map a script reads by a bare name and does not change keeps its host values on the stack: a time stamp in it stays a time stamp (it was written back as its text), and an `I64` stays an `I64`. A decimal read by a bare name twice in a script is the same object.
- `echo#Data` returns `timestampOut` as millisecond text, as the script layer converts dates on purpose; Java returns a `Timestamp`. Undecided.

## Next steps

**From round 5 (external Postgres), by expected payoff:**
1. ~~Pool after a server restart~~, ~~TLS roots and modes~~ and ~~iterate round trips~~: done, see Round 5 fixes.
2. **MySQL**: the same idle check (the `mysql` crate's connection), and `sslMode`/`useSSL` root certificates; neither was looked at.
3. ~~Multi-row upsert for data loads~~: done (round 6) for Postgres. MySQL could take the same `execute_deferred_row` (`ON DUPLICATE KEY UPDATE` allows a key twice in one statement). A record's fields differ between records of a seed file; writing the entity's whole column list would batch more but would clear the fields a record leaves out, so it is not done.
4. ~~Connection failure, cancel over TLS, a statement timeout that fires~~: done. `connections_fail_cleanly_when_the_server_ends_or_cancels_them` (embedded cluster, every `cargo test`) ends a session with `pg_terminate_backend` mid-transaction, mid-pipeline (rows held and writes unanswered) and mid-stream, and checks the error, `is_closed`, that nothing was written, and that the pool's next call has a new connection; it also fires a statement timeout and a cancel. Before a request on a connection quiet for 20 ms, the wire reads what the server sent since without blocking (`check_quiet`), so a session the server ended fails with its reason (`terminating connection due to administrator command`) instead of `Connection reset by peer`. `tests/it/pg_external.rs` (ignored; `PG_EXTERNAL_URL`) runs the timeout and the cancel against a real server, TLS included: it passed with `verify-full` and a private root, `require`, and no TLS.
5. **Poolers** (not tested; no PgBouncer image here). Statements are named and prepared per connection, so a transaction-mode pooler needs PgBouncer 1.21+ with `max_prepared_statements` set (protocol-level prepared statements), or session pooling. `BEGIN`, `SET LOCAL statement_timeout` and the first statement share a write and a transaction, which transaction pooling keeps together. Cancel requests go to the address the driver connected to, which PgBouncer forwards.

**External Postgres (before round 5)**, all looked at in rounds 5 and 6 above; kept for the record. What the embedded tests cannot show:
1. **TLS against a real server.** The client's TLS is tested only against an in-process server (SSLRequest, rustls handshake, startup, a query, Terminate). Test certificate verification with real roots, a server that requires TLS, and `sslmode`-style choices if any are wanted beyond "required".
2. **SCRAM channel binding.** The client offers `SCRAM-SHA-256` without channel binding; a server set to require `SCRAM-SHA-256-PLUS` would refuse it. Add `tls-server-end-point` binding from the rustls peer certificate if that matters.
3. **Latency.** Re-run `entity_speed`, `service_speed` and `data_load_speed` against a server one network hop away. The round-trip savings should grow: BEGIN pipelining saves one round trip per own-transaction service call, upsert pipelining all but one per data file, first-run prepare one per new statement per connection.
4. **Connection failure.** A server restart or a dropped TCP connection mid-transaction, mid-pipeline and mid-stream: the pool should discard the connection (`is_closed`), the frame should roll back, and the error should name the cause. Also `cancel` over TLS and a `statement_timeout` that fires.
5. **Poolers.** Named prepared statements need session pooling, or PgBouncer 1.21+ with protocol-level prepared statements. Check, and document the setting.

**Further work, by expected payoff:**
1. ~~COPY for bulk loads~~: measured in round 7. A staging table saves nothing; direct COPY saves 40% of the server's write only for rows known to be new. Not done. A load's time is mostly elsewhere: profile `./moq load` end to end first.
2. ~~Multi-threaded benchmark~~: done (round 7), and the slot mutex it showed is an `RwLock`. Cached finds still stop scaling past two threads, on the slots lock's reader count and the `MCache` entry's atomics: a per-thread memo of the definition lookups (read meta, l10n targets, read shape; keyed by a facade id and the derived generation) would take the shared lock out of a cache hit.
3. **Deferred writes on a cold statement** still prepare with a waited round trip (once per statement per connection). Queue the prepare with the write.
4. **Binary parameters** for integers, timestamps and numerics: less text formatting here and less parsing on the server.
5. ~~Remaining copying MNode calls in definition builders~~: done (round 8) for comparisons, compare-only matches and read-only child loops. What is left there is mostly the definitions' own `String` fields (an `attribute()` kept as a field has to be one).
6. **Nested data records** still go through an `MNode` per top-level element; a streaming builder could read them the way `parse_flat` reads flat ones.
7. **Reads before write** for entities that cannot upsert (audit-logged, status flows, data-feed entities) still cost one waited round trip each in a load.
8. **QuickJS for trivial actions.** About half of `echo#Data` is QuickJS running four `<set>` actions through bare names; a native path for trivial actions would change value types, so it needs a decision. [Round 11](#round-11-javascript-bindings) took `echo#Data` from 10.8 to 8.9 us; what is left of the script engine's own list is at its end.
9. ~~Transaction bookkeeping~~: done (round 7) for entities with no cached find and no feed document. A data-feed touch still keeps two `String`s per write; an entity with cached finds keeps the values written.
10. **A Java comparison.** Run the same `SpeedTest.xml` calls and a seed load on Moqui Java against the same Postgres, so the numbers above have a reference.

Known flaky test, not from this work: `service::coverage::tests::calls_are_counted_only_between_enable_and_finish` counts into a process-wide recorder and can race with tests running in parallel.
